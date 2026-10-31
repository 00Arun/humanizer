//! Humanizer's own AI-text detection suite: five independent statistical
//! engines plus an optional model-based judge, combined into one report.
//!
//! The engines score the signals published research says detectors rely on:
//! sentence-rhythm variance (burstiness), lexical predictability (a perplexity
//! proxy), stylometric fingerprints, over-used machine idioms, and structural
//! templates. Each produces a 0-100 "machine-likeness" score and the sentences
//! that drove it.

pub mod engines;
pub mod judge;

use serde::{Deserialize, Serialize};

use crate::text;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub label: String,
    pub value: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flagged {
    pub sentence: String,
    pub score: u8,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineReport {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 0 = reads human, 100 = reads machine-generated.
    pub ai_score: f64,
    /// Weight in the ensemble; lowered when the sample is too small.
    pub weight: f64,
    pub signals: Vec<Signal>,
    pub flagged: Vec<Flagged>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionReport {
    pub overall: f64,
    pub verdict: String,
    pub confidence: String,
    pub word_count: usize,
    pub sentence_count: usize,
    pub note: String,
    pub engines: Vec<EngineReport>,
}

/// Runs the five local engines (no network).
pub fn detect_local(text: &str) -> DetectionReport {
    let engines = vec![
        engines::burstiness(text),
        engines::lexical(text),
        engines::stylometry(text),
        engines::patterns(text),
        engines::structure(text),
    ];
    combine(text, engines)
}

pub fn combine(text: &str, engines: Vec<EngineReport>) -> DetectionReport {
    let word_count = text::word_count(text);
    let sentence_count = text::sentences(text).len();

    let usable: Vec<&EngineReport> = engines.iter().filter(|e| e.error.is_none()).collect();
    let total_w: f64 = usable.iter().map(|e| e.weight).sum();
    let overall = if total_w > 0.0 {
        usable.iter().map(|e| e.ai_score * e.weight).sum::<f64>() / total_w
    } else {
        50.0
    };
    let overall = (overall * 10.0).round() / 10.0;

    // Real human prose scores roughly 10-35 on this suite; 0 is not a thing.
    let verdict = if overall <= 25.0 {
        "Reads as human-written"
    } else if overall < 40.0 {
        "Human range"
    } else if overall < 60.0 {
        "Mixed / uncertain"
    } else {
        "Likely AI-generated"
    }
    .to_string();

    let (confidence, note) = if word_count < 60 {
        ("low", "Fewer than 60 words: statistical signals are unreliable on very short text.")
    } else if word_count < 150 {
        ("medium", "Short sample. Scores firm up above ~150 words.")
    } else {
        ("high", "")
    };

    DetectionReport {
        overall,
        verdict,
        confidence: confidence.to_string(),
        word_count,
        sentence_count,
        note: note.to_string(),
        engines,
    }
}

#[cfg(test)]
mod tests;
