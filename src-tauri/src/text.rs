//! Shared text utilities: tokenising, sentence splitting, the AI-idiom lexicon
//! and the embedded word-frequency table used as a perplexity proxy.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;

static WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Za-z][A-Za-z'’-]*").unwrap());

/// Zipf frequency (log10 occurrences per billion words) for ~47k English words.
/// Source: OpenSubtitles 2018 frequency list (hermitdave/FrequencyWords, CC-BY-SA 4.0).
pub static WORD_FREQ: Lazy<HashMap<&'static str, f32>> = Lazy::new(|| {
    include_str!("../data/wordfreq.txt")
        .lines()
        .filter_map(|l| {
            let mut it = l.split(' ');
            Some((it.next()?, it.next()?.parse().ok()?))
        })
        .collect()
});

pub fn words(text: &str) -> Vec<String> {
    WORD_RE
        .find_iter(text)
        .map(|m| m.as_str().replace('’', "'").to_lowercase())
        .collect()
}

pub fn word_count(text: &str) -> usize {
    WORD_RE.find_iter(text).count()
}

/// Zipf frequency of a word, or a low default for unknown (rare) tokens.
pub fn zipf(word: &str) -> f32 {
    WORD_FREQ.get(word).copied().unwrap_or(2.2)
}

const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "fig", "no", "al",
    "inc", "ltd", "co", "approx", "dept", "est", "u.s", "u.k",
];

/// Splits text into sentences. Handles common abbreviations, decimals,
/// closing quotes/brackets, and treats newlines as hard boundaries.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '.' || c == '!' || c == '?' {
                // swallow runs like "?!" or "..."
                let mut j = i;
                while j + 1 < chars.len() && matches!(chars[j + 1], '.' | '!' | '?') {
                    j += 1;
                }
                // closing quotes / brackets belong to this sentence
                while j + 1 < chars.len() && matches!(chars[j + 1], '"' | '”' | '\'' | '’' | ')' | ']') {
                    j += 1;
                }
                let next_is_space = j + 1 >= chars.len() || chars[j + 1].is_whitespace();
                let decimal = c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
                let mut is_abbrev = false;
                if c == '.' {
                    let mut k = i;
                    while k > 0 && (chars[k - 1].is_alphanumeric() || chars[k - 1] == '.') {
                        k -= 1;
                    }
                    let tok: String = chars[k..i].iter().collect::<String>().to_lowercase();
                    is_abbrev = ABBREVIATIONS.contains(&tok.as_str())
                        || (tok.len() == 1 && tok.chars().all(|c| c.is_alphabetic()));
                }
                if next_is_space && !decimal && !is_abbrev {
                    let s: String = chars[start..=j].iter().collect();
                    let s = s.trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                    }
                    start = j + 1;
                }
                i = j + 1;
                continue;
            }
            i += 1;
        }
        if start < chars.len() {
            let s: String = chars[start..].iter().collect();
            let s = s.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
    }
    out
}

pub fn paragraphs(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Words and phrases that language models over-use relative to people.
/// Weight 2 = strong tell, 1 = mild tell.
pub const AI_LEXICON: &[(&str, u8)] = &[
    ("delve", 2), ("delves", 2), ("delving", 2), ("tapestry", 2), ("testament to", 2),
    ("pivotal", 2), ("crucial role", 2), ("vital role", 2), ("plays a crucial", 2),
    ("in the realm of", 2), ("realm", 1), ("landscape", 1), ("ever-evolving", 2),
    ("rapidly evolving", 2), ("fast-paced", 2), ("in today's", 2), ("digital age", 2),
    ("it is important to note", 2), ("it's important to note", 2), ("it is worth noting", 2),
    ("it's worth noting", 2), ("it is essential", 2), ("it is crucial", 2),
    ("navigate", 1), ("navigating", 1), ("leverage", 2), ("leveraging", 2), ("utilize", 2),
    ("utilizing", 2), ("foster", 1), ("fostering", 1), ("robust", 1), ("seamless", 2),
    ("seamlessly", 2), ("holistic", 2), ("multifaceted", 2), ("nuanced", 1), ("underscore", 2),
    ("underscores", 2), ("showcase", 1), ("showcasing", 1), ("embark", 2), ("embarking", 2),
    ("journey", 1), ("unlock", 1), ("harness", 2), ("harnessing", 2), ("elevate", 1),
    ("streamline", 2), ("transformative", 2), ("game-changer", 2), ("game changer", 2),
    ("cutting-edge", 2), ("groundbreaking", 1), ("paradigm", 1), ("synergy", 2),
    ("meticulous", 1), ("meticulously", 2), ("comprehensive", 1), ("intricate", 1),
    ("intricacies", 2), ("myriad", 2), ("plethora", 2), ("furthermore", 1), ("moreover", 1),
    ("additionally", 1), ("consequently", 1), ("ultimately", 1), ("in conclusion", 2),
    ("in summary", 2), ("to summarize", 2), ("overall,", 1), ("a wide range of", 1),
    ("not only", 1), ("but also", 1), ("whether you're", 2), ("when it comes to", 1),
    ("at the end of the day", 1), ("dive into", 2), ("deep dive", 2), ("shed light on", 2),
    ("stands as", 2), ("serves as a", 2), ("serves as an", 2), ("the world of", 1),
    ("in an era", 2), ("look no further", 2), ("that being said", 1), ("with that said", 1),
    ("by leveraging", 2), ("aims to", 1), ("strives to", 1), ("remember that", 1),
    ("keep in mind", 1), ("enhance", 1), ("enhancing", 1), ("ensure", 1), ("ensuring", 1),
    ("empower", 2), ("empowering", 2), ("revolutionize", 2), ("revolutionizing", 2),
    ("unleash", 2), ("unparalleled", 2), ("vibrant", 1), ("bustling", 2), ("nestled", 2),
    ("beacon", 2), ("captivating", 1), ("compelling", 1), ("invaluable", 1), ("indelible", 2),
    ("profound", 1), ("notably", 1), ("arguably", 1), ("importantly", 1), ("significantly", 1),
    ("firstly", 1), ("secondly", 1), ("lastly", 1), ("in essence", 2), ("essentially", 1),
];

/// Returns (phrase, weight) hits found in the text, lower-cased matching.
pub fn lexicon_hits(text: &str) -> Vec<(&'static str, u8)> {
    let lower = text.to_lowercase().replace('’', "'");
    let mut hits = Vec::new();
    for (phrase, w) in AI_LEXICON {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(phrase) {
            let abs = from + pos;
            let end = abs + phrase.len();
            let before_ok = abs == 0 || !lower.as_bytes()[abs - 1].is_ascii_alphanumeric();
            let after_ok = end >= lower.len() || !lower.as_bytes()[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                hits.push((*phrase, *w));
            }
            from = end;
        }
    }
    hits
}

pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

pub fn std_dev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64).sqrt()
}

/// Logistic squashing to a 0-100 score. `x` is a z-like distance from the
/// decision boundary; positive means "more machine-like".
pub fn score(x: f64) -> f64 {
    100.0 / (1.0 + (-x).exp())
}
