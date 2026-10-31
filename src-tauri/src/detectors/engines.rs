//! The five local detection engines. Each is deterministic and offline.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;

use super::{EngineReport, Flagged, Signal};
use crate::text::{self, mean, score, std_dev, zipf};

fn sig(label: &str, value: String, note: &str) -> Signal {
    Signal { label: label.into(), value, note: note.into() }
}

fn report(id: &str, name: &str, description: &str, ai_score: f64, weight: f64, signals: Vec<Signal>, flagged: Vec<Flagged>) -> EngineReport {
    EngineReport {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        ai_score: (ai_score.clamp(0.0, 100.0) * 10.0).round() / 10.0,
        weight,
        signals,
        flagged,
        error: None,
    }
}

fn too_short(id: &str, name: &str, description: &str, why: &str) -> EngineReport {
    EngineReport {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        ai_score: 50.0,
        weight: 0.0,
        signals: vec![],
        flagged: vec![],
        error: Some(why.into()),
    }
}

const TRANSITION_STARTS: &[&str] = &[
    "furthermore", "moreover", "additionally", "however", "consequently", "therefore", "thus",
    "ultimately", "overall", "in conclusion", "in summary", "importantly", "notably", "firstly",
    "secondly", "finally", "lastly", "in addition", "as a result", "on the other hand", "in essence",
];

// ---------------------------------------------------------------------------
// 1. Burstiness: variance of sentence rhythm
// ---------------------------------------------------------------------------
pub fn burstiness(text_in: &str) -> EngineReport {
    const ID: &str = "burstiness";
    const NAME: &str = "Rhythm & Burstiness";
    const DESC: &str = "Measures how much sentence length varies. People write in bursts: a long sentence, then a short one. Models drift toward uniform 15-25 word sentences.";
    let sents = text::sentences(text_in);
    if sents.len() < 4 {
        return too_short(ID, NAME, DESC, "Needs at least 4 sentences.");
    }
    let lens: Vec<f64> = sents.iter().map(|s| text::word_count(s) as f64).collect();
    let m = mean(&lens);
    let sd = std_dev(&lens);
    let cv = if m > 0.0 { sd / m } else { 0.0 };
    let near_mean = lens.iter().filter(|&&l| (l - m).abs() <= m * 0.25).count() as f64 / lens.len() as f64;
    let short = lens.iter().filter(|&&l| l <= 8.0).count() as f64 / lens.len() as f64;
    let long = lens.iter().filter(|&&l| l >= 30.0).count() as f64 / lens.len() as f64;

    // Human prose: CV ~0.55-0.75. Machine prose: CV ~0.25-0.45.
    let s_cv = score((0.50 - cv) / 0.08);
    let s_uniform = score((near_mean - 0.55) / 0.10);
    let s_short = score((0.12 - short) / 0.08);
    let ai = 0.55 * s_cv + 0.25 * s_uniform + 0.20 * s_short;

    // Flag runs of 3+ consecutive sentences within 25% of the mean length.
    let mut flagged = Vec::new();
    let mut run_start = 0;
    for i in 0..=lens.len() {
        let in_run = i < lens.len() && (lens[i] - m).abs() <= m * 0.25;
        if !in_run {
            if i - run_start >= 3 {
                for j in run_start..i {
                    if flagged.len() < 8 {
                        flagged.push(Flagged {
                            sentence: sents[j].clone(),
                            score: 65,
                            reason: format!("Part of a run of {} same-length sentences (~{} words each)", i - run_start, lens[j] as usize),
                        });
                    }
                }
            }
            run_start = i + 1;
        }
    }

    let weight = if sents.len() < 8 { 0.6 } else { 1.0 };
    report(ID, NAME, DESC, ai, weight, vec![
        sig("Sentence length variation (CV)", format!("{cv:.2}"), "Human writing usually sits above 0.50; machine text often below 0.45."),
        sig("Average sentence length", format!("{m:.1} words"), ""),
        sig("Sentences near the average", format!("{:.0}%", near_mean * 100.0), "High uniformity is a machine tell."),
        sig("Short sentences (≤8 words)", format!("{:.0}%", short * 100.0), "People use punchy short sentences; models rarely do."),
        sig("Long sentences (≥30 words)", format!("{:.0}%", long * 100.0), ""),
    ], flagged)
}

// ---------------------------------------------------------------------------
// 2. Lexical predictability: a perplexity proxy from word frequencies
// ---------------------------------------------------------------------------
pub fn lexical(text_in: &str) -> EngineReport {
    const ID: &str = "lexical";
    const NAME: &str = "Lexical Predictability";
    const DESC: &str = "A perplexity proxy. Scores every word against a 47k-word frequency table: models favour safe, mid-frequency 'polished' vocabulary with little spread; people mix very common words with genuinely rare ones.";
    let words = text::words(text_in);
    if words.len() < 40 {
        return too_short(ID, NAME, DESC, "Needs at least 40 words.");
    }
    let z: Vec<f64> = words.iter().map(|w| zipf(w) as f64).collect();
    let z_mean = mean(&z);
    let z_sd = std_dev(&z);
    let rare = z.iter().filter(|&&v| v < 3.5).count() as f64 / z.len() as f64;
    let polished = words
        .iter()
        .zip(&z)
        .filter(|(w, &v)| w.len() >= 7 && (3.6..=5.2).contains(&v))
        .count() as f64
        / words.len() as f64;
    // Moving-window type-token ratio (window 100) is length-independent.
    let win = 100.min(words.len());
    let mut ttrs = Vec::new();
    let mut i = 0;
    while i + win <= words.len() {
        let set: HashSet<&String> = words[i..i + win].iter().collect();
        ttrs.push(set.len() as f64 / win as f64);
        i += win;
    }
    let ttr = mean(&ttrs);

    let s_spread = score((1.42 - z_sd) / 0.07);
    let s_polished = score((polished - 0.13) / 0.03);
    let s_rare = score((0.05 - rare) / 0.025);
    let ai = 0.40 * s_spread + 0.40 * s_polished + 0.20 * s_rare;

    // Flag sentences densest in polished mid-frequency vocabulary.
    let mut scored: Vec<(f64, String)> = text::sentences(text_in)
        .into_iter()
        .filter_map(|s| {
            let ws = text::words(&s);
            if ws.len() < 6 {
                return None;
            }
            let p = ws.iter().filter(|w| w.len() >= 7 && (3.6..=5.2).contains(&(zipf(w) as f64))).count() as f64 / ws.len() as f64;
            Some((p, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let flagged = scored
        .into_iter()
        .take_while(|(p, _)| *p >= 0.22)
        .take(6)
        .map(|(p, s)| Flagged { sentence: s, score: (50.0 + p * 150.0).min(95.0) as u8, reason: format!("{:.0}% formal mid-frequency vocabulary", p * 100.0) })
        .collect();

    let weight = if words.len() < 120 { 0.6 } else { 0.9 };
    report(ID, NAME, DESC, ai, weight, vec![
        sig("Word-frequency spread (σ Zipf)", format!("{z_sd:.2}"), "People mix the very common with the genuinely rare; models cluster in the middle."),
        sig("Mean word frequency (Zipf)", format!("{z_mean:.2}"), ""),
        sig("Polished mid-frequency words", format!("{:.1}%", polished * 100.0), "Long, safe, 'essay' vocabulary such as 'significant', 'various', 'essential'."),
        sig("Rare words (Zipf < 3.5)", format!("{:.1}%", rare * 100.0), "Includes names and jargon; higher reads more human."),
        sig("Type-token ratio (100-word windows)", format!("{ttr:.2}"), ""),
    ], flagged)
}

// ---------------------------------------------------------------------------
// 3. Stylometry: voice fingerprint
// ---------------------------------------------------------------------------
static CONTRACTION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b\w+(?:n't|'re|'ve|'ll|'d|'m|'s)\b").unwrap());
static FIRST_PERSON_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(i|i'm|i've|i'd|i'll|me|my|we|we're|we've|our|us)\b").unwrap());

pub fn stylometry(text_in: &str) -> EngineReport {
    const ID: &str = "stylometry";
    const NAME: &str = "Stylometric Fingerprint";
    const DESC: &str = "Voice-level features: contractions, first-person presence, how sentences open, punctuation variety and average word length. Model prose is longer-worded, opener-repetitive and punctuation-poor.";
    let sents = text::sentences(text_in);
    let words = text::words(text_in);
    if sents.len() < 3 || words.len() < 40 {
        return too_short(ID, NAME, DESC, "Needs at least 3 sentences and 40 words.");
    }
    let n_words = words.len() as f64;
    let avg_word_len = words.iter().map(|w| w.chars().count() as f64).sum::<f64>() / n_words;
    let contractions = CONTRACTION_RE.find_iter(text_in).count() as f64 / n_words * 100.0;
    let first_person = FIRST_PERSON_RE.find_iter(text_in).count() as f64 / n_words * 100.0;

    let openers: Vec<String> = sents
        .iter()
        .filter_map(|s| text::words(s).first().cloned())
        .collect();
    let opener_set: HashSet<&String> = openers.iter().collect();
    let opener_diversity = opener_set.len() as f64 / openers.len().max(1) as f64;
    let transition_openers = sents
        .iter()
        .filter(|s| {
            let l = s.to_lowercase();
            TRANSITION_STARTS.iter().any(|t| l.starts_with(t))
        })
        .count() as f64
        / sents.len() as f64;

    let punct_types = [';', ':', '(', '!', '?', '"', '“', '—', '-', '…']
        .iter()
        .filter(|&&p| text_in.contains(p))
        .count();

    let s_wordlen = score((avg_word_len - 4.75) / 0.18);
    let s_contr = score((0.6 - contractions) / 0.5);
    let s_open = score((0.72 - opener_diversity) / 0.08);
    let s_trans = score((transition_openers - 0.12) / 0.06);
    let s_punct = score((3.5 - punct_types as f64) / 1.0);
    let s_fp = score((0.5 - first_person) / 0.8);
    let ai = 0.28 * s_wordlen + 0.16 * s_contr + 0.20 * s_open + 0.16 * s_trans + 0.10 * s_punct + 0.10 * s_fp;

    let mut flagged = Vec::new();
    let mut counts: HashMap<&String, usize> = HashMap::new();
    for o in &openers {
        *counts.entry(o).or_default() += 1;
    }
    for (s, o) in sents.iter().zip(&openers) {
        let l = s.to_lowercase();
        if let Some(t) = TRANSITION_STARTS.iter().find(|t| l.starts_with(*t)) {
            if flagged.len() < 8 {
                flagged.push(Flagged { sentence: s.clone(), score: 70, reason: format!("Opens with the stock transition “{t}”") });
            }
        } else if counts[o] >= 3 && openers.len() >= 6 && flagged.len() < 8 {
            flagged.push(Flagged { sentence: s.clone(), score: 55, reason: format!("Opener “{o}” is reused {} times", counts[o]) });
        }
    }

    report(ID, NAME, DESC, ai, 0.9, vec![
        sig("Average word length", format!("{avg_word_len:.2} chars"), "Human prose averages ~4.4-4.7; model prose ~4.9-5.3."),
        sig("Contractions per 100 words", format!("{contractions:.1}"), "Formal human writing can be low too, so this is weighted lightly."),
        sig("Sentence-opener diversity", format!("{:.0}%", opener_diversity * 100.0), "Share of sentences that start with a distinct word."),
        sig("Sentences opening with a transition", format!("{:.0}%", transition_openers * 100.0), "“Furthermore”, “Moreover”, “Ultimately”…"),
        sig("Punctuation variety", format!("{punct_types} types"), "Dashes, brackets, semicolons, questions, quotes."),
        sig("First-person per 100 words", format!("{first_person:.1}"), ""),
    ], flagged)
}

// ---------------------------------------------------------------------------
// 4. Patterns: machine-idiom lexicon
// ---------------------------------------------------------------------------
static EMDASH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"—|\s-\s|--").unwrap());
static NOT_ONLY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bnot only\b.{3,120}?\bbut (?:also )?").unwrap());

pub fn patterns(text_in: &str) -> EngineReport {
    const ID: &str = "patterns";
    const NAME: &str = "Machine Idioms";
    const DESC: &str = "Counts words and phrases that language models reach for far more often than people (“delve”, “tapestry”, “it's important to note”, “not only … but also”, em-dash chains).";
    let n_words = text::word_count(text_in);
    if n_words < 30 {
        return too_short(ID, NAME, DESC, "Needs at least 30 words.");
    }
    let hits = text::lexicon_hits(text_in);
    let weighted: f64 = hits.iter().map(|(_, w)| *w as f64).sum();
    let per_1000 = weighted / n_words as f64 * 1000.0;
    let emdash = EMDASH_RE.find_iter(text_in).count() as f64 / n_words as f64 * 100.0;
    let not_only = NOT_ONLY_RE.find_iter(text_in).count();

    let s_lex = score((per_1000 - 7.0) / 3.5);
    let s_dash = score((emdash - 0.8) / 0.4);
    let ai = 0.80 * s_lex + 0.15 * s_dash + 0.05 * score((not_only as f64 - 0.5) / 0.5);

    let mut flagged = Vec::new();
    for s in text::sentences(text_in) {
        let h = text::lexicon_hits(&s);
        if h.is_empty() {
            continue;
        }
        let w: u8 = h.iter().map(|(_, w)| *w).sum();
        let list: Vec<&str> = h.iter().map(|(p, _)| *p).collect::<HashSet<_>>().into_iter().collect();
        flagged.push(Flagged {
            sentence: s,
            score: (45 + w * 12).min(95),
            reason: format!("Machine idiom: {}", list.join(", ")),
        });
    }
    flagged.sort_by(|a, b| b.score.cmp(&a.score));
    flagged.truncate(10);

    let mut top: HashMap<&str, usize> = HashMap::new();
    for (p, _) in &hits {
        *top.entry(*p).or_default() += 1;
    }
    let mut top: Vec<(&str, usize)> = top.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let top_str = top.iter().take(6).map(|(p, c)| format!("{p} ×{c}")).collect::<Vec<_>>().join(", ");

    report(ID, NAME, DESC, ai, 1.2, vec![
        sig("Idiom density (weighted per 1,000 words)", format!("{per_1000:.1}"), "Human prose typically scores under 5; model prose 10+."),
        sig("Idioms found", if top_str.is_empty() { "none".into() } else { top_str }, ""),
        sig("Em-dashes per 100 words", format!("{emdash:.2}"), ""),
        sig("“Not only … but also” constructions", not_only.to_string(), ""),
    ], flagged)
}

// ---------------------------------------------------------------------------
// 5. Structure: templates and parallelism
// ---------------------------------------------------------------------------
static TRICOLON_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b\w+(?: \w+)?, \w+(?: \w+)?, and \w+").unwrap());
static COLON_SETUP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[^:]{3,40}:\s").unwrap());
static RHETORICAL_Q_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:but |so |and )?(?:what|why|how|who|where|is|are|does|do|can|could|should|will)\b.{0,100}\?$").unwrap());
static WRAPUP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:overall|ultimately|in conclusion|in summary|in short|by \w+ing|this (?:approach|balance|shift|combination|ensures|makes|allows|means|highlights|underscores)|together,|as such|in doing so)").unwrap());

pub fn structure(text_in: &str) -> EngineReport {
    const ID: &str = "structure";
    const NAME: &str = "Structural Templates";
    const DESC: &str = "Looks for the shapes models fall into: equal-length paragraphs, a wrap-up sentence closing every paragraph, three-item lists everywhere, colon set-ups and rhetorical questions as transitions.";
    let paras = text::paragraphs(text_in);
    let sents = text::sentences(text_in);
    if sents.len() < 4 {
        return too_short(ID, NAME, DESC, "Needs at least 4 sentences.");
    }
    let n_s = sents.len() as f64;
    let tricolons = TRICOLON_RE.find_iter(text_in).count() as f64 / n_s;
    let colon_setups = sents.iter().filter(|s| COLON_SETUP_RE.is_match(s)).count() as f64 / n_s;
    let rhetorical = sents.iter().filter(|s| RHETORICAL_Q_RE.is_match(s)).count() as f64 / n_s;

    let mut wrapups = 0usize;
    let mut flagged = Vec::new();
    for p in &paras {
        let ps = text::sentences(p);
        if ps.len() >= 3 {
            if let Some(last) = ps.last() {
                if WRAPUP_RE.is_match(last) {
                    wrapups += 1;
                    if flagged.len() < 8 {
                        flagged.push(Flagged { sentence: last.clone(), score: 68, reason: "Paragraph closes with a summarising wrap-up sentence".into() });
                    }
                }
            }
        }
    }
    let multi_paras = paras.iter().filter(|p| text::sentences(p).len() >= 3).count();
    let wrap_rate = if multi_paras > 0 { wrapups as f64 / multi_paras as f64 } else { 0.0 };

    let para_lens: Vec<f64> = paras.iter().map(|p| text::word_count(p) as f64).filter(|&l| l >= 15.0).collect();
    let para_cv = if para_lens.len() >= 3 { std_dev(&para_lens) / mean(&para_lens).max(1.0) } else { f64::NAN };

    for s in &sents {
        if flagged.len() >= 10 {
            break;
        }
        if TRICOLON_RE.is_match(s) && text::word_count(s) < 40 {
            flagged.push(Flagged { sentence: s.clone(), score: 55, reason: "Three-item parallel list (tricolon)".into() });
        } else if COLON_SETUP_RE.is_match(s) {
            flagged.push(Flagged { sentence: s.clone(), score: 55, reason: "Colon set-up (“The result: …”)".into() });
        } else if RHETORICAL_Q_RE.is_match(s) {
            flagged.push(Flagged { sentence: s.clone(), score: 50, reason: "Rhetorical question used as a transition".into() });
        }
    }

    let s_tri = score((tricolons - 0.12) / 0.07);
    let s_wrap = score((wrap_rate - 0.35) / 0.18);
    let s_colon = score((colon_setups - 0.08) / 0.05);
    let s_rhet = score((rhetorical - 0.05) / 0.04);
    let s_para = if para_cv.is_nan() { 50.0 } else { score((0.42 - para_cv) / 0.10) };
    let ai = 0.30 * s_tri + 0.25 * s_wrap + 0.15 * s_colon + 0.10 * s_rhet + 0.20 * s_para;

    let weight = if paras.len() < 2 { 0.6 } else { 0.8 };
    report(ID, NAME, DESC, ai, weight, vec![
        sig("Three-item lists per sentence", format!("{tricolons:.2}"), "“fast, reliable, and secure” in every other sentence is a template."),
        sig("Paragraphs ending on a wrap-up line", format!("{:.0}%", wrap_rate * 100.0), "“Overall, …”, “By doing so, …”, “This ensures …”."),
        sig("Colon set-ups", format!("{:.0}% of sentences", colon_setups * 100.0), ""),
        sig("Rhetorical-question transitions", format!("{:.0}% of sentences", rhetorical * 100.0), ""),
        sig("Paragraph-length variation (CV)", if para_cv.is_nan() { "n/a".into() } else { format!("{para_cv:.2}") }, "Equal-sized paragraphs are a machine tell."),
    ], flagged)
}
