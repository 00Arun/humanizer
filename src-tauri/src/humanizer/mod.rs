//! The humanising pipeline. Three engines, chosen by the caller:
//!   * `Api`   – Claude (Fable by default) over HTTP, paragraph chunks in parallel
//!   * `Local` – an in-process open-weight model via llama.cpp, fully offline
//!   * `Rules` – the deterministic rule engine, no model at all
//! Every path ends with the local polish pass, and Strong intensity adds a
//! detector-guided second pass.

pub mod claude;
pub mod local;
pub mod local_llm;

use std::sync::Arc;

use futures::{stream, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::detectors;
use crate::text;
use claude::ClaudeClient;
use local_llm::LocalLlm;

#[derive(Clone)]
pub enum Engine {
    Api(Arc<ClaudeClient>),
    Local(Arc<LocalLlm>),
    Rules,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanizeOptions {
    /// natural | academic | professional | casual | creative
    pub style: String,
    /// light | balanced | strong
    pub intensity: String,
    /// Fixed seed for reproducible tests; None = fresh randomness every run,
    /// so clicking Humanize again produces a different rewrite.
    #[serde(default)]
    pub seed: Option<u32>,
}

/// Where real human writing sits on our 0-100 scale. The loop stops once a
/// text is at or below this; pushing further mostly degrades the prose.
pub const HUMAN_TARGET: f64 = 25.0;

fn run_seed(opts: &HumanizeOptions) -> u32 {
    opts.seed.unwrap_or_else(|| {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7);
        (nanos % 900_000) + 1
    })
}

fn rounds_for(intensity: &str) -> usize {
    match intensity {
        "light" => 1,
        "strong" => 4,
        _ => 3,
    }
}

/// Paragraph indices that still contain a sentence our detectors flagged.
fn flagged_paragraphs(paragraphs: &[String], report: &detectors::DetectionReport) -> Vec<usize> {
    let flagged: Vec<&str> = report.engines.iter().flat_map(|e| e.flagged.iter().map(|f| f.sentence.as_str())).collect();
    let mut idx: Vec<usize> = paragraphs
        .iter()
        .enumerate()
        .filter(|(_, p)| eligible(p) && flagged.iter().any(|s| p.contains(s)))
        .map(|(i, _)| i)
        .collect();
    if idx.is_empty() {
        // Nothing sentence-level to point at; work on every eligible paragraph.
        idx = paragraphs.iter().enumerate().filter(|(_, p)| eligible(p)).map(|(i, _)| i).collect();
    }
    idx
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

/// Shorter instructions for the 4B local model: one paragraph in, one out.
/// Small models default to "polished essay" voice, so this leans hard the
/// other way: plain words, short sentences, no decoration.
const LOCAL_SYSTEM_PROMPT: &str = r#"You rewrite one paragraph so it sounds like a real person talking plainly, not like an essay or a machine.

Rules:
- Keep the exact meaning, facts, names, numbers, dates, quotes and the same language. Add nothing new. Drop nothing. Keep the same person (I/we/you) and tense. Keep about the same length.
- Use everyday words. Do NOT make the text fancier, more formal, or more "polished" than the input. Do not add adjectives or imagery that were not there.
- Mix sentence lengths: several short sentences (3-8 words) and one or two longer ones.
- Use plain connectors: and, but, so, because, then. Start some sentences with And, But or So.
- No em-dashes (—). No colons used as set-ups. No three-item lists unless the input had them.
- Banned words: delve, tapestry, testament, pivotal, crucial, vital, essential, realm, landscape, leverage, utilize, foster, robust, seamless, holistic, nuanced, underscore, showcase, embark, harness, elevate, streamline, transformative, majestic, breathtaking, awe-inspiring, serene, tranquil, invaluable, profoundly, renowned, countless, furthermore, moreover, additionally, consequently, ultimately, in conclusion, it is important to note, serve as, stand as, not only ... but also, a wide range of.
- Do not end with a sentence that sums up the paragraph.
- Output ONLY the rewritten paragraph. No title, no quotes, no notes.

Example
Input: In today's fast-paced world, effective communication plays a crucial role in fostering collaboration. Furthermore, leveraging modern tools can significantly enhance productivity across diverse teams.
Output: Clear communication matters more than ever. It keeps teams working together. And the right tools help, sometimes a lot, especially when the team is spread out."#;

const LOCAL_RETRY_HINT: &str = "Your previous attempt sounded MORE like a machine than the input: too formal, decorated, evenly paced. Try again with simpler words, shorter sentences, and no added adjectives.";

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

/// Humanises a list of paragraphs with the given engine.
pub async fn humanize_paragraphs(engine: Engine, paragraphs: Vec<String>, opts: &HumanizeOptions, progress: ProgressFn) -> HumanizeOutcome {
    let contractions = opts.style != "academic";
    match engine {
        Engine::Rules => rules_only(paragraphs, opts, contractions, progress),
        Engine::Local(llm) => local_ai(llm, paragraphs, opts, contractions, progress).await,
        Engine::Api(client) => api(client, paragraphs, opts, contractions, progress).await,
    }
}

fn rules_only(paragraphs: Vec<String>, opts: &HumanizeOptions, contractions: bool, progress: ProgressFn) -> HumanizeOutcome {
    progress(0, 1, "Rewriting with rules only");
    let lo = local::LocalOptions { contractions, restructure: opts.intensity != "light" };
    let out: Vec<String> = paragraphs.iter().map(|p| local::humanize(p, &lo)).collect();
    progress(1, 1, "Done");
    let mut warnings = vec!["No AI engine available: used the rule engine only. In Settings, download the offline AI model (one-time, 2.5 GB) or add an Anthropic API key for a real rewrite.".into()];
    if out == paragraphs {
        warnings.push("The rule engine found nothing it could safely change in this text.".into());
    }
    HumanizeOutcome { paragraphs: out, engine: "local-rules".into(), passes: 1, warnings }
}

// ---------------------------------------------------------------------------
// Local AI
// ---------------------------------------------------------------------------
async fn local_ai(llm: Arc<LocalLlm>, paragraphs: Vec<String>, opts: &HumanizeOptions, contractions: bool, progress: ProgressFn) -> HumanizeOutcome {
    let system = format!("{LOCAL_SYSTEM_PROMPT}\n{}\n{}", style_block(&opts.style), intensity_block(&opts.intensity));
    let mut warnings = Vec::new();
    let mut current = paragraphs.clone();

    // How many candidates to draw per paragraph; the detectors pick the winner.
    let candidates = match opts.intensity.as_str() {
        "light" => 1,
        "strong" => 3,
        _ => 2,
    };
    let seed = run_seed(opts);
    let polish_all = |ps: &[String]| -> Vec<String> {
        ps.iter().map(|p| if eligible(p) { local::polish(p, contractions && opts.intensity != "light") } else { p.clone() }).collect()
    };
    let score_of = |ps: &[String]| detectors::detect_local(&polish_all(ps).join("\n\n")).overall;

    let start_score = score_of(&current);
    let (out, failed, reasons) = local_pass(&llm, &system, &current, None, candidates, seed, progress.clone(), "Rewriting with local AI").await;
    if failed > 0 {
        warnings.push(format!("{failed} paragraph(s) fell back to the rule engine: {}.", reasons.join("; ")));
    }
    let mut passes = 1;
    // Keep the first pass only if the text as a whole did not get worse.
    let mut best_score = start_score;
    let s1 = score_of(&out);
    if s1 <= start_score + 0.5 {
        current = out;
        best_score = s1;
    }

    // Keep going while the text still reads machine-like: rewrite the
    // paragraphs that carry flagged sentences, keep the result only if the
    // overall score improves, stop at the human band or when stuck.
    let rounds = rounds_for(&opts.intensity);
    while passes < rounds && best_score > HUMAN_TARGET {
        let report = detectors::detect_local(&polish_all(&current).join("\n\n"));
        let targets = flagged_paragraphs(&current, &report);
        let feedback = format!(
            "Another pass. The text still reads machine-like ({:.0}/100): uniform sentence lengths, stock phrasing, tidy structure. Rewrite with more varied rhythm, plainer and more specific wording, and different sentence openers.",
            report.overall
        );
        let (refined, _, _) = local_pass_subset(&llm, &system, &current, &targets, Some(&feedback), candidates, seed.wrapping_add(passes as u32 * 97), progress.clone(), &format!("Pass {} with local AI", passes + 1)).await;
        passes += 1;
        let s = score_of(&refined);
        if s + 0.5 < best_score {
            current = refined;
            best_score = s;
        } else {
            break;
        }
    }

    if current == paragraphs {
        warnings.push(format!("Nothing changed: no rewrite scored more human than the text you gave ({start_score:.0}/100). It is already in the human band, or try Strong intensity."));
    } else if best_score > HUMAN_TARGET + 15.0 {
        warnings.push(format!("Still {best_score:.0}/100 after {passes} passes. Click Humanize again for fresh attempts, or switch to Strong intensity."));
    }

    let out = polish_all(&current);
    progress(1, 1, "Done");
    HumanizeOutcome { paragraphs: out, engine: format!("local-ai ({})", llm.name), passes: passes as u32, warnings }
}

#[allow(clippy::too_many_arguments)]
async fn local_pass(llm: &Arc<LocalLlm>, system: &str, paragraphs: &[String], feedback: Option<&str>, candidates: usize, seed: u32, progress: ProgressFn, stage: &str) -> (Vec<String>, usize, Vec<String>) {
    let targets: Vec<usize> = paragraphs.iter().enumerate().filter(|(_, p)| eligible(p)).map(|(i, _)| i).collect();
    local_pass_subset(llm, system, paragraphs, &targets, feedback, candidates, seed, progress, stage).await
}

#[allow(clippy::too_many_arguments)]
async fn local_pass_subset(llm: &Arc<LocalLlm>, system: &str, paragraphs: &[String], targets: &[usize], feedback: Option<&str>, candidates: usize, seed: u32, progress: ProgressFn, stage: &str) -> (Vec<String>, usize, Vec<String>) {
    let total = targets.len();
    let mut out = paragraphs.to_vec();
    let mut failed = 0;
    let mut reasons: Vec<String> = Vec::new();
    progress(0, total, stage);
    for (n, &i) in targets.iter().enumerate() {
        let llm = llm.clone();
        let system = match feedback {
            Some(f) => format!("{system}\n{f}"),
            None => system.to_string(),
        };
        let original = paragraphs[i].clone();
        let seed = seed.wrapping_add(i as u32 * 1_000);
        let rewritten = tokio::task::spawn_blocking(move || rewrite_one(&llm, &system, &original, seed, candidates))
            .await
            .unwrap_or_else(|e| Err(format!("worker panicked: {e}")));
        match rewritten {
            Ok(t) => out[i] = t,
            Err(why) => {
                // The model could not beat the original; the rule engine still can.
                failed += 1;
                if !reasons.contains(&why) {
                    reasons.push(why);
                }
                out[i] = local::humanize(&paragraphs[i], &local::LocalOptions { contractions: true, restructure: true });
            }
        }
        progress(n + 1, total, stage);
    }
    (out, failed, reasons)
}

/// Machine-likeness of one paragraph, after the polish pass, as the gate sees it.
fn gate_score(p: &str) -> f64 {
    detectors::detect_local(&local::polish(p, true)).overall
}

static QUOTE_WRAP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^\s*["“]([\s\S]*)["”]\s*$"#).unwrap());
static LABEL_LINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?im)^\s*(?:rewritten(?: paragraph| text| version)?|revised(?: paragraph| text)?|output|answer|humanized(?: text)?)\s*:\s*").unwrap());

/// One paragraph through the local model, gated by our own detectors. Draws
/// `candidates` versions (plus one corrective retry if they all read worse
/// than the input), keeps the one our detectors score most human, and only
/// returns it if it is at least as human as the input.
fn rewrite_one(llm: &LocalLlm, system: &str, original: &str, seed: u32, candidates: usize) -> Result<String, String> {
    let words = text::word_count(original).max(1);
    let max_new = ((words as f64 * 2.4) as usize + 48).min(1400);
    let baseline = gate_score(original);
    let temps = [0.7f32, 0.9, 0.6, 0.8];
    let mut best: Option<(f64, String)> = None;
    let mut last_err: Option<String> = None;
    let mut invalid = 0usize;
    let mut attempt = 0u32;
    let mut planned = candidates.clamp(1, 3);
    while (attempt as usize) < planned.min(4) {
        let worse_so_far = best.as_ref().map_or(false, |(s, _)| *s > baseline);
        let sys = if attempt > 0 && worse_so_far { format!("{system}\n\n{LOCAL_RETRY_HINT}") } else { system.to_string() };
        let temp = temps[attempt as usize % temps.len()];
        match llm.generate(&sys, original, max_new, temp, seed.wrapping_add(attempt)) {
            Ok(raw) => match validate(&raw, original) {
                Some(t) => {
                    let score = gate_score(&t);
                    if best.as_ref().map_or(true, |(s, _)| score < *s) {
                        best = Some((score, t));
                    }
                }
                None => invalid += 1,
            },
            Err(e) => last_err = Some(e),
        }
        attempt += 1;
        // All planned candidates read worse than the input: one corrective retry.
        if attempt as usize == planned && planned < 4 && best.as_ref().map_or(true, |(s, _)| *s > baseline + 2.0) {
            planned += 1;
        }
    }
    // Near the human band every point counts; further out a tiny loss is a
    // fair price for a genuinely different rewrite.
    let tolerance = if baseline <= HUMAN_TARGET + 15.0 { 0.0 } else { 2.0 };
    match best {
        Some((score, t)) if score <= baseline + tolerance => Ok(t),
        Some((score, _)) => Err(format!("no candidate read more human than the input ({score:.0} vs {baseline:.0})")),
        None => Err(match last_err {
            Some(e) => format!("model error: {e}"),
            None => format!("{invalid} candidate(s) failed validation"),
        }),
    }
}

fn validate(raw: &str, original: &str) -> Option<String> {
    let mut t = raw.trim().to_string();
    t = LABEL_LINE_RE.replace_all(&t, "").to_string();
    if let Some(c) = QUOTE_WRAP_RE.captures(&t) {
        t = c[1].to_string();
    }
    // Small models sometimes append notes after a blank line; keep the first block.
    if let Some(idx) = t.find("\n\n") {
        let (first, rest) = t.split_at(idx);
        let rest_l = rest.to_lowercase();
        if rest_l.contains("note") || rest_l.contains("changes") || rest_l.contains("i ") || text::word_count(rest) < text::word_count(first) / 2 {
            t = first.to_string();
        }
    }
    t = t.replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    if t.contains("<p") || t.contains("```") || t.is_empty() {
        return None;
    }
    let ow = text::word_count(original) as f64;
    let nw = text::word_count(&t) as f64;
    if nw < ow * 0.55 || nw > ow * 1.6 {
        return None;
    }
    if t.trim() == original.trim() {
        return None;
    }
    Some(t)
}

// ---------------------------------------------------------------------------
// Claude API
// ---------------------------------------------------------------------------
async fn api(client: Arc<ClaudeClient>, paragraphs: Vec<String>, opts: &HumanizeOptions, contractions: bool, progress: ProgressFn) -> HumanizeOutcome {
    let mut warnings = Vec::new();
    let mut current = paragraphs.clone();
    let system = format!("{SYSTEM_PROMPT}\n\n{}\n{}", style_block(&opts.style), intensity_block(&opts.intensity));

    let polish_all = |ps: &[String]| -> Vec<String> {
        ps.iter().map(|p| if eligible(p) { local::polish(p, contractions && opts.intensity != "light") } else { p.clone() }).collect()
    };
    let score_of = |ps: &[String]| detectors::detect_local(&polish_all(ps).join("\n\n")).overall;

    let (rewritten, mut errs) = model_pass(&client, &system, &current, None, progress.clone(), "Rewriting").await;
    warnings.append(&mut errs);
    let mut passes = 1;
    // Accept the first pass only if it is not worse than the input.
    let mut best_score = score_of(&current);
    let s1 = score_of(&rewritten);
    if s1 <= best_score + 2.0 {
        current = rewritten;
        best_score = s1;
    }

    let rounds = rounds_for(&opts.intensity);
    while passes < rounds && best_score > HUMAN_TARGET {
        let report = detectors::detect_local(&polish_all(&current).join("\n\n"));
        let mut flagged: Vec<String> = report
            .engines
            .iter()
            .flat_map(|e| e.flagged.iter().map(|f| format!("- \"{}\" ({})", f.sentence, f.reason)))
            .collect();
        flagged.dedup();
        flagged.truncate(14);
        let feedback = format!(
            "Another pass. Our detector still scores the previous rewrite at {:.0}/100 machine-likeness. The sentences below were flagged; fix those patterns wherever they occur, vary rhythm and openers more, and keep every rule.\n{}",
            report.overall,
            flagged.join("\n")
        );
        let (refined, mut errs2) = model_pass(&client, &system, &current, Some(&feedback), progress.clone(), &format!("Pass {}", passes + 1)).await;
        warnings.append(&mut errs2);
        passes += 1;
        let s = score_of(&refined);
        if s + 0.5 < best_score {
            current = refined;
            best_score = s;
        } else {
            break;
        }
    }

    if current == paragraphs {
        warnings.push("Nothing changed: no rewrite scored more human than the text you gave. It is already in the human band, or try Strong intensity.".into());
    }

    let out = polish_all(&current);
    progress(1, 1, "Done");
    HumanizeOutcome { paragraphs: out, engine: client.model.clone(), passes: passes as u32, warnings }
}

/// One model pass over all eligible paragraphs, chunked and run concurrently.
async fn model_pass(client: &Arc<ClaudeClient>, system: &str, paragraphs: &[String], feedback: Option<&str>, progress: ProgressFn, stage: &str) -> (Vec<String>, Vec<String>) {
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
        let ow = text::word_count(&originals[i]).max(1) as f64;
        let nw = text::word_count(t) as f64;
        if nw < ow * 0.45 || nw > ow * 1.8 {
            continue;
        }
        out.push((i, t.to_string()));
    }
    out
}

#[cfg(test)]
pub fn debug_local_system(style: &str, intensity: &str) -> String {
    format!("{LOCAL_SYSTEM_PROMPT}\n{}\n{}", style_block(style), intensity_block(intensity))
}
#[cfg(test)]
pub fn debug_validate(raw: &str, original: &str) -> Option<String> {
    validate(raw, original)
}
