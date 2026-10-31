//! The offline rule engine. Used on its own when no API key is configured and
//! as a light polish pass after the model rewrite.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::text;

/// (pattern, replacement) pairs for machine idioms. Case-insensitive; a
/// leading capital is restored when the match started a sentence.
const SWAPS: &[(&str, &str)] = &[
    (r"\bdelve into\b", "look at"), (r"\bdelves into\b", "looks at"), (r"\bdelving into\b", "looking at"),
    (r"\bin the realm of\b", "in"), (r"\ba testament to\b", "a sign of"), (r"\bis a testament to\b", "shows"),
    (r"\bplays a (?:crucial|vital|pivotal|key) role in\b", "matters for"),
    (r"\bit is important to note that\b", ""), (r"\bit's important to note that\b", ""),
    (r"\bit is worth noting that\b", ""), (r"\bit's worth noting that\b", ""),
    (r"\bit is essential to\b", "you need to"), (r"\bit is crucial to\b", "you need to"),
    (r"\bin today's (?:(?:fast-paced|rapidly evolving|ever-evolving|digital|modern|competitive|dynamic|globalized)\s+)*(?:world|landscape|age|era|environment|economy|market|society)\b", "these days"),
    (r"\bin today's world\b", "these days"), (r"\bever-evolving\b", "changing"), (r"\brapidly evolving\b", "fast-changing"),
    (r"\bnavigate the (?:complexities|challenges|intricacies) of\b", "deal with"), (r"\bnavigate\b", "handle"), (r"\bnavigating\b", "handling"),
    (r"\bsignificantly (enhance|improve|increase|reduce|boost|impact)", "$1"), (r"\bthe complexities of\b", "the hard parts of"),
    (r"\bleverage\b", "use"), (r"\bleveraging\b", "using"), (r"\bleverages\b", "uses"),
    (r"\butilize\b", "use"), (r"\butilizing\b", "using"), (r"\butilizes\b", "uses"), (r"\butilization\b", "use"),
    (r"\bfacilitate\b", "help"), (r"\bfacilitates\b", "helps"),
    (r"\bfoster\b", "build"), (r"\bfostering\b", "building"), (r"\bfosters\b", "builds"),
    (r"\bseamlessly\b", "smoothly"), (r"\bseamless\b", "smooth"), (r"\bholistic\b", "broad"),
    (r"\bmultifaceted\b", "complex"), (r"\bunderscores\b", "shows"), (r"\bunderscore\b", "show"),
    (r"\bshowcases\b", "shows"), (r"\bshowcase\b", "show"), (r"\bembark on\b", "start"), (r"\bembarking on\b", "starting"),
    (r"\bharness\b", "use"), (r"\bharnessing\b", "using"), (r"\bstreamline\b", "simplify"), (r"\bstreamlining\b", "simplifying"),
    (r"\btransformative\b", "far-reaching"), (r"\bgame-changer\b", "big deal"), (r"\bcutting-edge\b", "new"),
    (r"\bmyriad of\b", "many"), (r"\ba myriad\b", "many"), (r"\bmyriad\b", "many"), (r"\ba plethora of\b", "plenty of"),
    (r"\bshed light on\b", "explain"), (r"\bsheds light on\b", "explains"), (r"\bdive into\b", "get into"), (r"\bdeep dive into\b", "close look at"),
    (r"\bstands as\b", "is"), (r"\bserves as a\b", "is a"), (r"\bserves as an\b", "is an"),
    (r"\bin an era (?:where|of)\b", "now that"), (r"\bwhen it comes to\b", "with"), (r"\bat the end of the day\b", "in the end"),
    (r"\bmeticulously\b", "carefully"), (r"\bmeticulous\b", "careful"), (r"\bintricacies\b", "details"), (r"\bintricate\b", "detailed"),
    (r"\bempower\b", "let"), (r"\bempowers\b", "lets"), (r"\bempowering\b", "letting"), (r"\brevolutionize\b", "change"),
    (r"\bunparalleled\b", "unmatched"), (r"\bnestled\b", "set"), (r"\bbustling\b", "busy"), (r"\bbeacon of\b", "example of"),
    (r"\benhance\b", "improve"), (r"\benhances\b", "improves"), (r"\benhancing\b", "improving"),
    (r"\bensure that\b", "make sure"), (r"\bensure\b", "make sure"), (r"\bensures\b", "makes sure"),
    (r"\bcomprehensive\b", "thorough"), (r"\brobust\b", "solid"), (r"\boptimal\b", "best"), (r"\bpivotal\b", "key"),
    (r"\bexceptional\b", "real"), (r"\binformed decisions\b", "better calls"), (r"\bsustainable growth\b", "steady growth"),
    (r"\bunlock (?:their|its|your) full potential\b", "do far more"), (r"\bcultivates a culture of\b", "builds a habit of"),
    (r"\bcultivate a culture of\b", "build a habit of"),
    (r"\bcommence\b", "start"), (r"\bendeavor\b", "effort"), (r"\bnumerous\b", "many"), (r"\bprior to\b", "before"),
    (r"\bsubsequently\b", "later"), (r"\bin order to\b", "to"), (r"\bdue to the fact that\b", "because"),
    (r"\bthe fact that\b", "that"), (r"\bin essence\b", "basically"), (r"\bessentially\b", "basically"),
    (r"\badditionally,\s*", "Also, "), (r"\bfurthermore,\s*", "And "), (r"\bmoreover,\s*", "Plus, "),
    (r"\bconsequently,\s*", "So "), (r"\bthus,\s*", "so "), (r"\bultimately,\s*", "In the end, "),
    (r"\bin conclusion,\s*", ""), (r"\bin summary,\s*", ""), (r"\bto summarize,\s*", ""), (r"\boverall,\s*", ""),
    (r"\bfirstly,\s*", "First, "), (r"\bsecondly,\s*", "Second, "), (r"\blastly,\s*", "Last, "),
    (r"\bnot only (.+?),? but also\b", "$1, and"),
];

const CONTRACTIONS: &[(&str, &str)] = &[
    (r"\bdo not\b", "don't"), (r"\bdoes not\b", "doesn't"), (r"\bdid not\b", "didn't"), (r"\bcannot\b", "can't"),
    (r"\bcould not\b", "couldn't"), (r"\bwould not\b", "wouldn't"), (r"\bshould not\b", "shouldn't"), (r"\bwill not\b", "won't"),
    (r"\bis not\b", "isn't"), (r"\bare not\b", "aren't"), (r"\bwas not\b", "wasn't"), (r"\bwere not\b", "weren't"),
    (r"\bhas not\b", "hasn't"), (r"\bhave not\b", "haven't"), (r"\bhad not\b", "hadn't"),
    (r"\bit is\b", "it's"), (r"\bthat is\b", "that's"), (r"\bthere is\b", "there's"), (r"\bwhat is\b", "what's"),
    (r"\bI am\b", "I'm"), (r"\bwe are\b", "we're"), (r"\byou are\b", "you're"), (r"\bthey are\b", "they're"),
    (r"\bI have\b", "I've"), (r"\bwe have\b", "we've"), (r"\byou have\b", "you've"), (r"\bthey have\b", "they've"),
    (r"\bI will\b", "I'll"), (r"\bwe will\b", "we'll"), (r"\byou will\b", "you'll"), (r"\bthey will\b", "they'll"), (r"\bit will\b", "it'll"),
    (r"\bI would\b", "I'd"), (r"\bwe would\b", "we'd"), (r"\byou would\b", "you'd"),
];

static SWAP_RES: Lazy<Vec<(Regex, &'static str)>> = Lazy::new(|| {
    SWAPS.iter().map(|(p, r)| (Regex::new(&format!("(?i){p}")).unwrap(), *r)).collect()
});
static CONTRACTION_RES: Lazy<Vec<(Regex, &'static str)>> = Lazy::new(|| {
    CONTRACTIONS.iter().map(|(p, r)| (Regex::new(p).unwrap(), *r)).collect()
});
static PREAMBLE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?is)^\s*(?:here(?:'s| is)(?: the| your| a)? (?:rewritten|revised|humanized|humanised|edited)[^:\n]*:|sure[,!][^\n]*\n|certainly[,!][^\n]*\n)\s*").unwrap()
});
static MULTI_SPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]{2,}").unwrap());
static SPACE_PUNCT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+([,.;:!?])").unwrap());
static AN_CONSONANT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b([Aa])n ([b-df-hj-np-tv-z])").unwrap());
static A_VOWEL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b([Aa]) ([aeio])").unwrap());
static DOUBLE_PUNCT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([,.;:])\s*,").unwrap());

pub struct LocalOptions {
    pub contractions: bool,
    pub restructure: bool,
}

/// Full offline humanisation of one paragraph.
pub fn humanize(paragraph: &str, opts: &LocalOptions) -> String {
    if text::word_count(paragraph) < 6 {
        return paragraph.to_string();
    }
    let mut out = swap_idioms(paragraph);
    out = soften_dashes(&out);
    if opts.contractions {
        out = contract(&out);
    }
    if opts.restructure {
        out = vary_rhythm(&out);
    }
    tidy(&out)
}

/// Light pass after a model rewrite: strips preambles, fixes spacing, and
/// catches any idiom the model let slip.
pub fn polish(paragraph: &str, contractions: bool) -> String {
    let mut out = PREAMBLE_RE.replace(paragraph, "").to_string();
    out = swap_idioms(&out);
    if contractions {
        out = contract_light(&out);
    }
    tidy(&out)
}

fn swap_idioms(s: &str) -> String {
    let mut out = s.to_string();
    for (re, rep) in SWAP_RES.iter() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                let m = caps.get(0).unwrap();
                let start = m.start();
                let at_sentence_start = start == 0
                    || out[..start].trim_end().ends_with(['.', '!', '?', '\n'])
                    || out[..start].trim_end().is_empty();
                let mut r = if rep.contains('$') {
                    let mut dst = String::new();
                    caps.expand(rep, &mut dst);
                    dst
                } else {
                    rep.to_string()
                };
                if at_sentence_start {
                    let mut c = r.chars();
                    if let Some(f) = c.next() {
                        r = f.to_uppercase().collect::<String>() + c.as_str();
                    }
                }
                r
            })
            .to_string();
    }
    // A removed opener can leave a lowercase sentence start: "it is important to note that the sky" -> "the sky".
    recapitalize_sentences(&out)
}

/// Capitalises the first letter of each sentence (an idiom removed at a
/// sentence start leaves a lowercase opener). Abbreviations such as "e.g."
/// or "U.S." do not count as sentence ends.
fn recapitalize_sentences(s: &str) -> String {
    const ABBREV: &[&str] = &["e.g", "i.e", "etc", "vs", "mr", "mrs", "ms", "dr", "prof", "no", "fig", "al", "approx", "u.s", "u.k"];
    let mut out = String::with_capacity(s.len());
    let mut cap_next = true;
    let mut token = String::new();
    for ch in s.chars() {
        if cap_next && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            cap_next = false;
        } else {
            out.push(ch);
        }
        match ch {
            '.' => {
                let t = token.to_lowercase();
                let abbrev = ABBREV.contains(&t.as_str()) || (t.len() == 1 && t.chars().all(|c| c.is_alphabetic()));
                if !abbrev {
                    cap_next = true;
                }
                token.push('.');
            }
            '!' | '?' | '\n' => {
                cap_next = true;
                token.clear();
            }
            c if c.is_whitespace() => token.clear(),
            c if matches!(c, '"' | '“' | '(' | '\'' | '‘') => {}
            c => {
                token.push(c);
                cap_next = false;
            }
        }
    }
    out
}

fn soften_dashes(s: &str) -> String {
    // Keep at most one em-dash per paragraph; turn the rest into commas or periods.
    let mut count = 0;
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let is_dash = chars[i] == '—' || (chars[i] == '-' && i + 1 < chars.len() && chars[i + 1] == '-');
        if is_dash {
            count += 1;
            if chars[i] == '-' {
                i += 1;
            }
            if count == 1 {
                out.push('—');
            } else {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(", ");
                while i + 1 < chars.len() && chars[i + 1] == ' ' {
                    i += 1;
                }
            }
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn contract(s: &str) -> String {
    let mut out = s.to_string();
    for (re, rep) in CONTRACTION_RES.iter() {
        out = re.replace_all(&out, *rep).to_string();
    }
    // "It's" at sentence start after lowercase replacement.
    recapitalize_sentences(&out)
}

/// Only the most natural contractions, roughly every other opportunity.
fn contract_light(s: &str) -> String {
    let mut out = s.to_string();
    let mut toggle = false;
    for (re, rep) in CONTRACTION_RES.iter().take(16) {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                toggle = !toggle;
                if toggle { rep.to_string() } else { caps[0].to_string() }
            })
            .to_string();
    }
    recapitalize_sentences(&out)
}

/// Breaks very long sentences and merges pairs of very short ones so the
/// length distribution stops looking uniform.
fn vary_rhythm(p: &str) -> String {
    let sents = text::sentences(p);
    if sents.len() < 2 {
        return p.to_string();
    }
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sents.len() {
        let s = &sents[i];
        let n = text::word_count(s);
        // Split long sentences, and every other medium one, so lengths spread out.
        if n >= 30 || (n >= 18 && i % 2 == 1) {
            if let Some((a, b)) = split_long(s) {
                out.push(a);
                out.push(b);
                i += 1;
                continue;
            }
        }
        if n <= 7 && i + 1 < sents.len() && text::word_count(&sents[i + 1]) <= 9 && out.len() % 2 == 0 {
            let first = s.trim_end_matches(['.', '!', '?']).to_string();
            let mut second = sents[i + 1].clone();
            if let Some(c) = second.chars().next() {
                if c.is_uppercase() && !second.starts_with('I') {
                    second = c.to_lowercase().collect::<String>() + &second[c.len_utf8()..];
                }
            }
            out.push(format!("{first}, and {second}"));
            i += 2;
            continue;
        }
        out.push(s.clone());
        i += 1;
    }
    out.join(" ")
}

fn split_long(s: &str) -> Option<(String, String)> {
    let candidates = [", which ", "; ", ", and ", ", but ", ", so ", ", while ", ", whereas ", ", because ", " because ", ", since ", ", as "];
    let mid = s.len() / 2;
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        for (pos, _) in s.match_indices(c) {
            // Both halves must be real sentences (≥5 words) and the cut must
            // not land inside a list ("x, y, and z").
            let left = &s[..pos];
            let right = &s[pos + c.len()..];
            if text::word_count(left) < 5 || text::word_count(right) < 5 {
                continue;
            }
            if c == ", and " && left.rsplit(", ").next().map_or(false, |seg| text::word_count(seg) <= 3) {
                continue;
            }
            if best.map_or(true, |(bp, _)| (pos as isize - mid as isize).abs() < (bp as isize - mid as isize).abs()) {
                best = Some((pos, c));
            }
        }
    }
    let (pos, sep) = best?;
    let a = s[..pos].trim_end().to_string() + ".";
    let rest = s[pos + sep.len()..].trim();
    let b = match sep {
        ", which " => format!("That {rest}"),
        ", and " => format!("And {rest}"),
        ", but " => format!("But {rest}"),
        ", so " => format!("So {rest}"),
        ", while " | ", whereas " => format!("Meanwhile, {rest}"),
        ", because " | " because " | ", since " | ", as " => format!("That's because {rest}"),
        _ => {
            let mut c = rest.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => return None,
            }
        }
    };
    Some((a, b))
}

fn tidy(s: &str) -> String {
    let mut out = MULTI_SPACE_RE.replace_all(s, " ").to_string();
    out = SPACE_PUNCT_RE.replace_all(&out, "$1").to_string();
    out = DOUBLE_PUNCT_RE.replace_all(&out, "$1").to_string();
    // Article agreement after a word swap ("an changing market", "a improved plan").
    out = AN_CONSONANT_RE.replace_all(&out, "$1 $2").to_string();
    out = A_VOWEL_RE.replace_all(&out, "${1}n $2").to_string();
    out.trim().to_string()
}
