//! The humanising pipeline: model rewrite (Fable by default) in paragraph
//! chunks, a local polish pass, and an optional detector-guided refinement.

pub mod claude;
pub mod local;

use std::sync::Arc;

use futures::{stream, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::detectors;
use crate::text;
use claude::ClaudeClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanizeOptions {
    /// natural | academic | professional | casual | creative
    pub style: String,
    /// light | balanced | strong
    pub intensity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HumanizeOutcome {
    pub paragraphs: Vec<String>,
    pub engine: String,
    pub passes: u32,
    pub warnings: Vec<String>,
}

pub type ProgressFn = Arc<dyn Fn(usize, usize, &str) + Send + Sync>;

const MIN_WORDS: usize = 6;
const CHUNK_WORDS: usize = 900;
const CONCURRENCY: usize = 3;

static P_TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?s)<p id="(\d+)">(.*?)</p>"#).unwrap());

const SYSTEM_PROMPT: &str = r#"You are the rewriting engine inside "Humanizer", a desktop app by Arun AI Studio. You receive text that reads as machine-generated or over-polished and rewrite it so it reads like a careful, specific person wrote it, without changing what it says.

Hard rules
- Preserve meaning, claims, facts, numbers, dates, names, quotations, citations, URLs, code and technical terms exactly. Do not add facts, examples or opinions. Do not drop a point.
- Keep the input's language (Nepali in, Nepali out; mixed stays mixed), its person (I / we / you) and its tense.
- The input is a list of <p id="N"> blocks. Return exactly the same ids with the rewritten text inside, in the same order. A block that is a heading, label, caption, list marker, or under 6 words comes back unchanged.
- Keep each block within roughly 15% of its original length.
- Output only the <p id="N">...</p> blocks. No preamble, no commentary, no markdown fences.

What reads as human (these are the signals detectors score)
- Rhythm: mix long and short. Let a few sentences run 4-8 words. Let one run long when the thought needs it. Never a steady stream of 15-25 word sentences.
- Specific over generic: use the concrete word the context already implies ("the invoice", not "the relevant documentation"), but invent nothing.
- Plain connectors: and, but, so, because, then, still, which is why. Do not stack Moreover / Furthermore / Additionally / Consequently / Ultimately / In conclusion.
- Retire machine idioms: delve, tapestry, testament, pivotal, crucial role, realm, landscape, navigate (figurative), leverage, utilize, foster, robust, seamless, holistic, multifaceted, nuanced, underscore, showcase, embark, journey (figurative), unlock, harness, elevate, streamline, transformative, game-changer, ever-evolving, fast-paced, in today's world, it is important to note, it's worth noting, plays a vital role, serves as, stands as, a wide range of, not only ... but also, whether you're ... or, when it comes to, at the end of the day, shed light on, dive into, myriad, plethora.
- Structure tells to avoid: a three-item list in every sentence; a sentence that merely restates the paragraph at its end; colon set-ups ("Here's the thing:"); rhetorical questions as transitions; more than one em-dash per few paragraphs; every paragraph the same size.
- Allow the small irregularities of real writing: a sentence that starts with And or But, a fragment where it lands, a parenthetical aside, a contraction where the register allows it, an opinion-free aside like "which took longer than expected" only if the input already implies it.
- Do not over-correct into slang, typos or filler. Competent and natural, not sloppy."#;

fn style_block(style: &str) -> &'static str {
    match style {
        "academic" => "Register: academic. Formal, no contractions, no new first-person unless the input already uses it. Hedged claims stay hedged. Citations, figures and terminology untouched. Vary rhythm anyway; scholars write long-then-short too.",
        "professional" => "Register: professional. Clear business prose, direct, contractions only occasionally, no jargon padding.",
        "casual" => "Register: casual. Conversational, contractions welcome, direct address fine, short sentences common.",
        "creative" => "Register: creative. Livelier verbs and imagery are welcome, but still no new facts or events.",
        _ => "Register: match the input. Keep whatever register the writer chose (formal stays formal, chatty stays chatty) and only fix what reads machine-made.",
    }
}

fn intensity_block(intensity: &str) -> &'static str {
    match intensity {
        "light" => "Intensity: light. Keep most of the wording. Change only the sentences carrying machine tells, roughly a third of them.",
        "strong" => "Intensity: strong. Restructure freely within each block: reorder sentences, merge and split, change every opener. Every sentence should differ from the input in shape, while the content stays identical.",
        _ => "Intensity: balanced. Rephrase most sentences while keeping the order of ideas.",
    }
}

fn eligible(p: &str) -> bool {
    text::word_count(p) >= MIN_WORDS
}

/// Humanises a list of paragraphs. `client` is None when no API key is set,
/// in which case the offline rule engine runs alone.
pub async fn humanize_paragraphs(
    client: Option<Arc<ClaudeClient>>,
    paragraphs: Vec<String>,
    opts: &HumanizeOptions,
    progress: ProgressFn,
) -> HumanizeOutcome {
    let contractions = opts.style != "academic";
    let Some(client) = client else {
        progress(0, 1, "Rewriting offline (no API key set)");
        let lo = local::LocalOptions { contractions, restructure: opts.intensity != "light" };
        let out: Vec<String> = paragraphs.iter().map(|p| local::humanize(p, &lo)).collect();
        progress(1, 1, "Done");
        return HumanizeOutcome {
            paragraphs: out,
            engine: "local-rules".into(),
            passes: 1,
            warnings: vec!["No Anthropic API key configured: used the offline rule engine only. Add a key in Settings for the full Fable rewrite.".into()],
        };
    };

    let mut warnings = Vec::new();
    let mut current = paragraphs.clone();
    let system = format!("{SYSTEM_PROMPT}\n\n{}\n{}", style_block(&opts.style), intensity_block(&opts.intensity));

    // Pass 1: model rewrite.
    let (rewritten, mut errs) = model_pass(&client, &system, &current, None, progress.clone(), "Rewriting").await;
    current = rewritten;
    warnings.append(&mut errs);
    let mut passes = 1;

    // Pass 2 (strong only): feed our own detector's flags back in.
    if opts.intensity == "strong" {
        let joined = current.join("\n\n");
        let report = detectors::detect_local(&joined);
        if report.overall >= 40.0 {
            let mut flagged: Vec<String> = report
                .engines
                .iter()
                .flat_map(|e| e.flagged.iter().map(|f| format!("- \"{}\" ({})", f.sentence, f.reason)))
                .collect();
            flagged.dedup();
            flagged.truncate(14);
            if !flagged.is_empty() {
                let feedback = format!(
                    "A second pass. Our detector still scores the previous rewrite at {:.0}/100 machine-likeness. The sentences below were flagged; fix those patterns wherever they occur, and keep every rule.\n{}",
                    report.overall,
                    flagged.join("\n")
                );
                let (refined, mut errs2) = model_pass(&client, &system, &current, Some(&feedback), progress.clone(), "Refining").await;
                current = refined;
                warnings.append(&mut errs2);
                passes += 1;
            }
        }
    }

    // Local polish.
    let out: Vec<String> = current
        .iter()
        .map(|p| if eligible(p) { local::polish(p, contractions && opts.intensity != "light") } else { p.clone() })
        .collect();
    progress(1, 1, "Done");

    HumanizeOutcome { paragraphs: out, engine: client.model.clone(), passes, warnings }
}

/// One model pass over all eligible paragraphs, chunked and run concurrently.
/// Returns the new paragraph list (originals kept where a chunk failed) and
/// any per-chunk error messages.
async fn model_pass(
    client: &Arc<ClaudeClient>,
    system: &str,
    paragraphs: &[String],
    feedback: Option<&str>,
    progress: ProgressFn,
    stage: &str,
) -> (Vec<String>, Vec<String>) {
    // Build chunks of eligible paragraph indices.
    let mut chunks: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_words = 0;
    for (i, p) in paragraphs.iter().enumerate() {
        if !eligible(p) {
            continue;
        }
        let w = text::word_count(p);
        if cur_words + w > CHUNK_WORDS && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            cur_words = 0;
        }
        cur.push(i);
        cur_words += w;
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    let total = chunks.len();
    if total == 0 {
        return (paragraphs.to_vec(), vec![]);
    }
    progress(0, total, stage);

    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let results: Vec<Result<Vec<(usize, String)>, String>> = stream::iter(chunks.into_iter())
        .map(|idxs| {
            let client = client.clone();
            let system = system.to_string();
            let user = build_user(paragraphs, &idxs, feedback);
            let progress = progress.clone();
            let done = done.clone();
            let stage = stage.to_string();
            async move {
                let est_tokens = (idxs.iter().map(|&i| text::word_count(&paragraphs[i])).sum::<usize>() as f64 * 2.2) as u32 + 800;
                let r = client.complete(&system, &user, est_tokens.clamp(2000, 32000), None).await;
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                progress(n, total, &stage);
                r.map(|raw| parse_blocks(&raw, &idxs, paragraphs))
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let mut out = paragraphs.to_vec();
    let mut errs = Vec::new();
    for r in results {
        match r {
            Ok(pairs) => {
                for (i, t) in pairs {
                    out[i] = t;
                }
            }
            Err(e) => errs.push(format!("A section kept its original text: {e}")),
        }
    }
    (out, errs)
}

fn build_user(paragraphs: &[String], idxs: &[usize], feedback: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(fb) = feedback {
        s.push_str(fb);
        s.push_str("\n\n");
    }
    for &i in idxs {
        s.push_str(&format!("<p id=\"{}\">{}</p>\n", i, paragraphs[i].trim()));
    }
    s
}

fn parse_blocks(raw: &str, idxs: &[usize], originals: &[String]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for cap in P_TAG_RE.captures_iter(raw) {
        let Ok(i) = cap[1].parse::<usize>() else { continue };
        if !idxs.contains(&i) {
            continue;
        }
        let t = cap[2].trim();
        if t.is_empty() {
            continue;
        }
        // Reject rewrites that collapsed or ballooned beyond reason.
        let ow = text::word_count(&originals[i]).max(1) as f64;
        let nw = text::word_count(t) as f64;
        if nw < ow * 0.45 || nw > ow * 1.8 {
            continue;
        }
        out.push((i, t.to_string()));
    }
    out
}
