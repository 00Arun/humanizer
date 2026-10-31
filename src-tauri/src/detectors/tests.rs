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
