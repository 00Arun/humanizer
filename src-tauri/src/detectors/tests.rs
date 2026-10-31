//! Sanity checks: a typical model-written passage must score clearly higher
//! than a typical human one, and the offline humanizer must lower the score.

use super::detect_local;
use crate::humanizer::local::{humanize, LocalOptions};

const AI_TEXT: &str = "In today's fast-paced digital landscape, effective communication plays a crucial role in fostering collaboration and driving organizational success. It is important to note that leveraging modern tools can significantly enhance productivity across diverse teams. Furthermore, organizations that embrace innovative strategies are better positioned to navigate the complexities of an ever-evolving market. Additionally, a comprehensive approach to professional development ensures that employees remain engaged, motivated, and aligned with core objectives. Ultimately, by prioritizing transparency, accountability, and continuous improvement, businesses can unlock their full potential and achieve sustainable growth.

Moreover, the integration of data-driven insights serves as a testament to the transformative power of technology. Teams that utilize robust analytics frameworks are able to make informed decisions, streamline operations, and deliver exceptional value to stakeholders. It is essential to recognize that this holistic methodology not only improves efficiency but also cultivates a culture of innovation. In conclusion, embracing these multifaceted strategies is pivotal for any organization seeking to thrive in the modern era.";

const HUMAN_TEXT: &str = "I didn't expect the meeting to run long. It did. Forty minutes in, Priya was still walking us through the invoice mess from March, and honestly, nobody could follow the spreadsheet she'd shared because half the columns were hidden. So we stopped. Dave suggested we just print the thing (yes, print it) and mark it up with pens, which felt absurd until it worked.

Here's what we actually found: three vendors had been paid twice, one hadn't been paid at all since January, and the \"approved\" column was mostly blank. Nobody's fault, really. The old system let anyone edit anything. By five we had a list, a plan, and a mild headache. I went home and didn't think about invoices again until the next morning, when Priya sent a one-line email: \"Found two more.\"";

#[test]
fn ai_text_scores_higher_than_human_text() {
    let ai = detect_local(AI_TEXT);
    let human = detect_local(HUMAN_TEXT);
    println!("AI overall={} human overall={}", ai.overall, human.overall);
    for e in &ai.engines {
        println!("  AI   {:<28} {:>5}  (w {})", e.name, e.ai_score, e.weight);
    }
    for e in &human.engines {
        println!("  HUM  {:<28} {:>5}  (w {})", e.name, e.ai_score, e.weight);
    }
    assert!(ai.overall > 60.0, "AI sample should read machine-like, got {}", ai.overall);
    assert!(human.overall < 40.0, "human sample should read human, got {}", human.overall);
}

#[test]
fn offline_humanizer_lowers_score() {
    let before = detect_local(AI_TEXT);
    let opts = LocalOptions { contractions: true, restructure: true };
    let out: Vec<String> = crate::text::paragraphs(AI_TEXT).iter().map(|p| humanize(p, &opts)).collect();
    let joined = out.join("\n\n");
    let after = detect_local(&joined);
    println!("offline rewrite: {} -> {}\n{}", before.overall, after.overall, joined);
    assert!(after.overall < before.overall - 10.0);
    assert!(!joined.to_lowercase().contains("leverag"));
    assert!(!joined.contains("It is important to note"));
}

#[test]
fn sentence_splitter_handles_abbreviations_and_decimals() {
    let s = crate::text::sentences("Dr. Smith paid 3.5 percent, e.g. in March. Then he left! Did he? Yes.");
    assert_eq!(s.len(), 4, "{s:?}");
}

#[test]
fn docx_paragraph_rebuild_keeps_properties() {
    let xml = r#"<w:p w:rsidR="1"><w:pPr><w:pStyle w:val="Heading1"/><w:rPr><w:b/></w:rPr></w:pPr><w:r><w:rPr><w:i/></w:rPr><w:t>Old </w:t></w:r><w:r><w:t>text</w:t></w:r></w:p>"#;
    let rebuilt = crate::documents::rebuild_paragraph_for_test(xml, "New & improved\ttext");
    assert!(rebuilt.starts_with(r#"<w:p w:rsidR="1"><w:pPr><w:pStyle w:val="Heading1"/>"#));
    assert!(rebuilt.contains(r#"<w:r><w:rPr><w:i/></w:rPr><w:t xml:space="preserve">New &amp; improved</w:t></w:r>"#));
    assert!(rebuilt.contains("<w:tab/>"));
    assert!(rebuilt.ends_with("</w:p>"));
}

#[test]
fn docx_round_trip_keeps_structure_and_replaces_text() {
    let dir = std::env::temp_dir().join(format!("humanizer-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.docx");
    let out = dir.join("out.docx");
    let paras = vec![
        "Heading".to_string(),
        "This is the first paragraph of the document, and it is long enough to be rewritten.".to_string(),
        "Second paragraph with an ampersand & a <tag> that must stay escaped in the output.".to_string(),
    ];
    crate::documents::write_new_docx(&paras, &src).unwrap();

    let doc = crate::documents::load(&src).unwrap();
    assert_eq!(doc.paragraphs.len(), 3);
    assert_eq!(doc.paragraphs[2].text, paras[2]);

    let mut new_texts: Vec<String> = doc.paragraphs.iter().map(|p| p.text.clone()).collect();
    new_texts[1] = "Rewritten first paragraph.".into();
    doc.save_with(&new_texts, &out).unwrap();

    let again = crate::documents::load(&out).unwrap();
    assert_eq!(again.paragraphs.len(), 3);
    assert_eq!(again.paragraphs[0].text, "Heading");
    assert_eq!(again.paragraphs[1].text, "Rewritten first paragraph.");
    assert_eq!(again.paragraphs[2].text, paras[2]);
    let _ = std::fs::remove_dir_all(&dir);
}

const ESSAY: &str = "Mountains are one of the most beautiful gifts of nature. They are very tall and covered with green forests, rocks, and snow. Mountains provide fresh air, clean water, and shelter to many animals and birds. They are also important sources of rivers. Many people visit mountains to enjoy their beauty and peaceful environment. Nepal is a beautiful country with many high mountains, including Mount Everest, the highest mountain in the world. Mountains also attract climbers and tourists from different countries. We should protect mountains by keeping them clean and planting trees. Mountains make our planet beautiful and wonderful.";

#[test]
fn offline_humanizer_changes_plain_essay_too() {
    let opts = LocalOptions { contractions: true, restructure: true };
    let out = humanize(ESSAY, &opts);
    println!("essay rewrite:\n{out}");
    assert_ne!(out, ESSAY, "offline engine must change something even without machine idioms");
    let before = detect_local(ESSAY).overall;
    let after = detect_local(&out).overall;
    println!("essay score {before} -> {after}");
    assert!(after < before, "score should drop: {before} -> {after}");
    let wc = |s: &str| crate::text::word_count(s) as f64;
    assert!((wc(&out) - wc(ESSAY)).abs() / wc(ESSAY) < 0.2, "length must stay within 20%");
    assert!(out.contains("Everest") && out.contains("Nepal"));
}

/// Runs the full local pipeline (prompt, validation, detector gate, best-of-N,
/// polish) on every catalog model that is present on this machine.
#[test]
fn local_model_pipeline_lowers_score() {
    use crate::humanizer::{humanize_paragraphs, local_llm, Engine, HumanizeOptions};
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = std::path::PathBuf::from(home).join("Library/Application Support/com.arunaistudio.humanizer/models");
    let only = std::env::var("HUMANIZER_TEST_TIER").ok();
    let present: Vec<&local_llm::ModelSpec> = local_llm::CATALOG
        .iter()
        .filter(|m| dir.join(m.file).is_file() && only.as_deref().map_or(true, |t| t == m.tier))
        .collect();
    if present.is_empty() {
        eprintln!("skipping: no catalog model under {}", dir.display());
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    for spec in present {
        let llm = std::sync::Arc::new(local_llm::LocalLlm::load(&dir.join(spec.file)).expect("load model"));
        println!("===== {} =====", spec.label);
        for (sample, name, min_drop) in [(ESSAY, "essay", 0.0), (AI_TEXT, "ai-sample", 15.0)] {
            let opts = HumanizeOptions { style: "natural".into(), intensity: "balanced".into(), seed: Some(1) };
            let progress: crate::humanizer::ProgressFn = std::sync::Arc::new(|_, _, _| {});
            let t0 = std::time::Instant::now();
            let out = rt.block_on(humanize_paragraphs(Engine::Local(llm.clone()), crate::text::paragraphs(sample), &opts, progress));
            let text = out.paragraphs.join("\n\n");
            let before = detect_local(sample).overall;
            let after = detect_local(&text).overall;
            println!("[{name}] {:.1}s engine {} passes {} warnings {:?}\n{text}\nscore {before} -> {after}\n", t0.elapsed().as_secs_f64(), out.engine, out.passes, out.warnings);
            if name == "essay" {
                assert!(text.contains("Everest") && text.contains("Nepal"), "lost a fact");
            }
            // The model must actually run; a silent fallback to rules is a failure here.
            assert!(!out.warnings.iter().any(|w| w.contains("model error")), "[{}/{name}] model error: {:?}", spec.tier, out.warnings);
            assert!(out.engine.starts_with("local-ai"), "engine was {}", out.engine);
            assert!(after <= before - min_drop, "[{}/{name}] expected a drop of {min_drop}+: {before} -> {after}", spec.tier);

            // Feeding the result back in (what a user does when clicking again)
            // must never make it worse, and a fresh seed must not be a no-op
            // unless the text is already in the human band.
            let opts2 = HumanizeOptions { style: "natural".into(), intensity: "balanced".into(), seed: Some(2) };
            let progress2: crate::humanizer::ProgressFn = std::sync::Arc::new(|_, _, _| {});
            let again = rt.block_on(humanize_paragraphs(Engine::Local(llm.clone()), crate::text::paragraphs(&text), &opts2, progress2));
            let text2 = again.paragraphs.join("\n\n");
            let after2 = detect_local(&text2).overall;
            println!("[{name}] second click: {after} -> {after2} warnings {:?}", again.warnings);
            assert!(after2 <= after + 0.5, "[{}/{name}] second pass got worse: {after} -> {after2}", spec.tier);
        }
    }
}

/// The in-app downloader: cancel part-way, then resume to completion.
/// Uses a 19 MB public GGUF so the test stays quick.
#[test]
fn model_download_cancels_and_resumes() {
    use crate::humanizer::local_llm::{download, Downloader, DownloadProgress};
    use std::sync::{atomic::Ordering, Arc, Mutex};
    const URL: &str = "https://huggingface.co/ggml-org/models/resolve/main/tinyllamas/stories15M-q4_0.gguf";
    const SIZE: u64 = 19_077_344;
    let dir = std::env::temp_dir().join(format!("humanizer-dl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("tiny.gguf");
    let http = reqwest::Client::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Pass 1: cancel once ~2 MB have arrived.
    let dl = Arc::new(Downloader::default());
    let dl2 = dl.clone();
    let r1 = rt.block_on(download(http.clone(), "light", URL, &dest, dl.clone(), move |p: DownloadProgress| {
        if !p.done && p.downloaded > 2_000_000 {
            dl2.cancel.store(true, Ordering::SeqCst);
        }
    }));
    assert!(r1.is_err(), "first pass should have been cancelled");
    let part = dest.with_extension("gguf.part");
    let have = part.metadata().map(|m| m.len()).unwrap_or(0);
    println!("cancelled with {have} bytes in {}", part.display());
    assert!(have > 1_000_000 && have < SIZE, "partial file expected, got {have}");
    assert!(!dest.exists());

    // Pass 2: resume to completion; the first progress report must start past the partial size.
    let first = Arc::new(Mutex::new(None::<u64>));
    let f2 = first.clone();
    let r2 = rt.block_on(download(http, "light", URL, &dest, dl.clone(), move |p: DownloadProgress| {
        if !p.done {
            f2.lock().unwrap().get_or_insert(p.downloaded);
        }
    }));
    assert!(r2.is_ok(), "resume failed: {r2:?}");
    assert_eq!(dest.metadata().unwrap().len(), SIZE);
    assert!(!part.exists());
    if let Some(f) = *first.lock().unwrap() {
        assert!(f >= have, "resume did not continue from the partial file ({f} < {have})");
    }
    assert!(!dl.active.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn debug_gemma_raw_outputs() {
    use crate::humanizer::local_llm::LocalLlm;
    let home = std::env::var("HOME").unwrap_or_default();
    let path = std::path::PathBuf::from(home).join("Library/Application Support/com.arunaistudio.humanizer/models/gemma-3-12b-it-qat-Q4_K_M.gguf");
    let llm = LocalLlm::load(&path).expect("load");
    let system = crate::humanizer::debug_local_system("natural", "balanced");
    for seed in [1u32, 2, 1000] {
        let t0 = std::time::Instant::now();
        let raw = llm.generate(&system, ESSAY, 300, 0.7, seed);
        println!("--- seed {seed} ({:.1}s) ---\n{:?}\n", t0.elapsed().as_secs_f64(), raw.as_ref().map(|s| s.chars().take(600).collect::<String>()));
        if let Ok(r) = &raw {
            println!("validate -> {:?}", crate::humanizer::debug_validate(r, ESSAY).map(|t| (detect_local(&crate::humanizer::local::polish(&t, true)).overall, t.chars().take(120).collect::<String>())));
        }
    }
    println!("baseline gate score: {}", detect_local(&crate::humanizer::local::polish(ESSAY, true)).overall);
}

/// Multi-turn chat with streaming on the light model (skips if absent).
#[test]
fn local_chat_streams_and_remembers_context() {
    use crate::humanizer::local_llm::{LocalLlm, DEFAULT_MODEL_FILE};
    let home = std::env::var("HOME").unwrap_or_default();
    let path = std::path::PathBuf::from(home).join("Library/Application Support/com.arunaistudio.humanizer/models").join(DEFAULT_MODEL_FILE);
    if !path.is_file() {
        eprintln!("skipping: light model not present");
        return;
    }
    let llm = LocalLlm::load(&path).expect("load");
    let system = "You are a helpful assistant. Answer briefly.";
    let turns = vec![
        ("user".to_string(), "My name is Arun and my favourite mountain is Annapurna. Reply with one short sentence.".to_string()),
        ("assistant".to_string(), "Nice to meet you, Arun. Annapurna is a great choice.".to_string()),
        ("user".to_string(), "What is my favourite mountain? Answer with just its name.".to_string()),
    ];
    let mut pieces = 0usize;
    let mut streamed = String::new();
    let reply = llm
        .generate_chat(system, &turns, 60, 0.2, 3, &mut |piece| {
            pieces += 1;
            streamed.push_str(piece);
            true
        })
        .expect("chat");
    println!("chat reply ({pieces} pieces): {reply:?}");
    assert!(pieces >= 2, "expected streamed pieces");
    assert_eq!(streamed.trim(), reply.trim());
    assert!(reply.to_lowercase().contains("annapurna"), "lost conversation context: {reply}");
}
