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
