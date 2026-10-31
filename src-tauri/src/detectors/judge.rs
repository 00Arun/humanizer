//! The model-based "AI Judge" engine: asks the configured Claude model for a
//! calibrated probability plus the sentences that gave it away.

use serde::Deserialize;

use super::{EngineReport, Flagged, Signal};
use crate::humanizer::claude::ClaudeClient;

const SYSTEM: &str = r#"You are the AI-text judge inside "Humanizer" by Arun AI Studio. Estimate the probability (0-100) that the text the user sends was produced by a language model rather than written by a person.

Weigh: sentence-rhythm variance, lexical choices (safe polished vocabulary vs. specific or idiosyncratic words), formulaic transitions and closers, structural templates (tricolons, wrap-up sentences, colon set-ups), specificity of detail, and whether a recognisable voice is present. Be calibrated: polished writing is not automatically machine-made, and informal or imperfect text is not automatically human. Non-native phrasing is not evidence of AI.

Respond with JSON only, no prose, no code fences:
{"score": <integer 0-100>, "verdict": "<human|mixed|ai>", "reasons": ["<short reason>", ...], "flagged": [{"sentence": "<exact sentence copied from the text>", "reason": "<why>"}]}
At most 5 reasons and 8 flagged sentences."#;

#[derive(Deserialize)]
struct JudgeJson {
    score: f64,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    reasons: Vec<String>,
    #[serde(default)]
    flagged: Vec<JudgeFlag>,
}

#[derive(Deserialize)]
struct JudgeFlag {
    sentence: String,
    #[serde(default)]
    reason: String,
}

const ID: &str = "ai_judge";
const NAME: &str = "AI Judge (model-based)";
const DESC: &str = "A frontier model reads the text as an expert reviewer would and returns a calibrated estimate with reasons. Independent from the statistical engines.";

pub async fn judge(client: &ClaudeClient, text: &str) -> EngineReport {
    let user = format!("<text>\n{}\n</text>", text.trim());
    match client.complete(SYSTEM, &user, 4000, Some("medium")).await {
        Ok(raw) => match parse(&raw) {
            Ok(j) => EngineReport {
                id: ID.into(),
                name: NAME.into(),
                description: DESC.into(),
                ai_score: j.score.clamp(0.0, 100.0),
                weight: 1.6,
                signals: std::iter::once(Signal { label: "Verdict".into(), value: j.verdict, note: format!("Model: {}", client.model) })
                    .chain(j.reasons.into_iter().enumerate().map(|(i, r)| Signal { label: format!("Reason {}", i + 1), value: r, note: String::new() }))
                    .collect(),
                flagged: j.flagged.into_iter().map(|f| Flagged { sentence: f.sentence, score: 75, reason: f.reason }).collect(),
                error: None,
            },
            Err(e) => error_report(format!("Could not parse judge response: {e}")),
        },
        Err(e) => error_report(e),
    }
}

fn parse(raw: &str) -> Result<JudgeJson, String> {
    let trimmed = raw.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    // Tolerate any stray prose around the JSON object.
    let start = trimmed.find('{').ok_or("no JSON object")?;
    let end = trimmed.rfind('}').ok_or("no JSON object")?;
    serde_json::from_str(&trimmed[start..=end]).map_err(|e| e.to_string())
}

/// The same judgement from the offline model. Smaller models are unreliable
/// at JSON, so this asks for a plain "Score: N" line plus short reasons.
pub async fn judge_local(llm: std::sync::Arc<crate::humanizer::local_llm::LocalLlm>, text: String) -> EngineReport {
    const SYSTEM: &str = "You are an AI-text detector. Read the text and estimate how likely it was written by an AI language model rather than a person. Consider: uniform sentence lengths, stock transitions (furthermore, moreover, in conclusion), generic polished vocabulary, three-item lists, wrap-up sentences, lack of specific detail or personal voice. Polished writing is not automatically AI. Answer in exactly this format and nothing else:\nScore: <number 0-100>\nReasons: <one or two short sentences>";
    // Keep well inside the local context window.
    let words: Vec<&str> = text.split_whitespace().collect();
    let sample = if words.len() > 900 { words[..900].join(" ") } else { text.clone() };
    let name = llm.name.clone();
    let res = tokio::task::spawn_blocking(move || llm.generate(SYSTEM, &sample, 160, 0.2, 7)).await;
    match res {
        Ok(Ok(raw)) => {
            let lower = raw.to_lowercase();
            let score = lower
                .split("score")
                .nth(1)
                .and_then(|s| s.trim_start_matches(|c: char| !c.is_ascii_digit()).split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse::<f64>().ok()));
            let Some(score) = score else {
                return error_report(format!("Local judge gave no score: {}", raw.chars().take(80).collect::<String>()));
            };
            let reasons = raw
                .splitn(2, |c: char| c == 'R' || c == 'r')
                .nth(1)
                .map(|r| r.trim_start_matches("easons").trim_start_matches(':').trim().to_string())
                .unwrap_or_default();
            EngineReport {
                id: ID.into(),
                name: "AI Judge (local model)".into(),
                description: DESC.into(),
                ai_score: score.clamp(0.0, 100.0),
                weight: 1.0,
                signals: vec![Signal { label: "Reasons".into(), value: reasons, note: format!("Model: {name} (offline)") }],
                flagged: vec![],
                error: None,
            }
        }
        Ok(Err(e)) => error_report(e),
        Err(e) => error_report(e.to_string()),
    }
}

pub fn error_report(msg: String) -> EngineReport {
    EngineReport {
        id: ID.into(),
        name: NAME.into(),
        description: DESC.into(),
        ai_score: 50.0,
        weight: 0.0,
        signals: vec![],
        flagged: vec![],
        error: Some(msg),
    }
}
