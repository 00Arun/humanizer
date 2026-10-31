//! Document I/O. Word files are edited surgically: each paragraph's runs are
//! replaced with the new text while paragraph styles, numbering, headers,
//! footers, images and tables stay exactly as they were.

use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Docx,
    Txt,
    Md,
    Pdf,
}

#[derive(Debug, Clone)]
pub struct Paragraph {
    /// Markdown list/heading marker kept out of the rewrite.
    pub prefix: String,
    pub text: String,
    /// False for paragraphs we must not touch (images, fields, links…).
    pub editable: bool,
}

pub struct LoadedDoc {
    pub kind: DocKind,
    pub path: PathBuf,
    pub paragraphs: Vec<Paragraph>,
    docx: Option<DocxFile>,
}

struct DocxFile {
    bytes: Vec<u8>,
    xml: String,
    /// Byte ranges of the outermost <w:p> elements, in document order,
    /// aligned 1:1 with `LoadedDoc::paragraphs`.
    ranges: Vec<(usize, usize)>,
}

pub fn kind_of(path: &Path) -> Result<DocKind, String> {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("docx") => Ok(DocKind::Docx),
        Some("txt") => Ok(DocKind::Txt),
        Some("md") | Some("markdown") => Ok(DocKind::Md),
        Some("pdf") => Ok(DocKind::Pdf),
        _ => Err("Unsupported file type. Use .docx, .txt, .md or .pdf".into()),
    }
}

pub fn load(path: &Path) -> Result<LoadedDoc, String> {
    let kind = kind_of(path)?;
    match kind {
        DocKind::Docx => load_docx(path),
        DocKind::Txt | DocKind::Md => {
            let raw = fs::read_to_string(path).map_err(|e| format!("Cannot read file: {e}"))?;
            Ok(LoadedDoc { kind, path: path.to_path_buf(), paragraphs: split_plain(&raw, kind == DocKind::Md), docx: None })
        }
        DocKind::Pdf => {
            let raw = pdf_extract::extract_text(path).map_err(|e| format!("Cannot read PDF text: {e}"))?;
            let normalized = normalize_pdf_text(&raw);
            Ok(LoadedDoc { kind, path: path.to_path_buf(), paragraphs: split_plain(&normalized, false), docx: None })
        }
    }
}

impl LoadedDoc {
    pub fn plain_text(&self) -> String {
        self.paragraphs.iter().map(|p| format!("{}{}", p.prefix, p.text)).collect::<Vec<_>>().join("\n\n")
    }

    pub fn word_count(&self) -> usize {
        self.paragraphs.iter().map(|p| text::word_count(&p.text)).sum()
    }

    /// Writes the document with `new_texts` (aligned with `paragraphs`) to
    /// `out`. The output format follows the extension of `out`.
    pub fn save_with(&self, new_texts: &[String], out: &Path) -> Result<(), String> {
        let out_kind = kind_of(out)?;
        match (out_kind, &self.docx) {
            (DocKind::Docx, Some(d)) => write_docx_surgical(d, &self.paragraphs, new_texts, out),
            (DocKind::Docx, None) => {
                let paras: Vec<String> = self.paragraphs.iter().zip(new_texts).map(|(p, t)| format!("{}{}", p.prefix, t)).collect();
                write_new_docx(&paras, out)
            }
            (DocKind::Txt | DocKind::Md, _) => {
                let body = self.paragraphs.iter().zip(new_texts).map(|(p, t)| format!("{}{}", p.prefix, t)).collect::<Vec<_>>().join("\n\n");
                fs::write(out, body).map_err(|e| format!("Cannot write file: {e}"))
            }
            (DocKind::Pdf, _) => Err("Saving as PDF is not supported; choose .docx or .txt".into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Plain text / markdown / pdf
// ---------------------------------------------------------------------------
static MD_PREFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\s*(?:#{1,6}|[-*+]|>|\d+[.)])\s+)").unwrap());

fn split_plain(raw: &str, markdown: bool) -> Vec<Paragraph> {
    let raw = raw.replace("\r\n", "\n");
    let mut out = Vec::new();
    for block in raw.split("\n\n") {
        let block = block.trim_end();
        if block.trim().is_empty() {
            continue;
        }
        if markdown {
            // Lists: one paragraph per line so markers survive.
            let lines: Vec<&str> = block.lines().collect();
            let listy = lines.len() > 1 && lines.iter().all(|l| MD_PREFIX_RE.is_match(l));
            if listy {
                for l in lines {
                    out.push(md_para(l));
                }
                continue;
            }
            if block.trim_start().starts_with("```") {
                out.push(Paragraph { prefix: String::new(), text: block.to_string(), editable: false });
                continue;
            }
        }
        out.push(if markdown { md_para(block) } else { Paragraph { prefix: String::new(), text: block.trim().to_string(), editable: true } });
    }
    out
}

fn md_para(block: &str) -> Paragraph {
    if let Some(c) = MD_PREFIX_RE.captures(block) {
        let prefix = c[1].to_string();
        Paragraph { text: block[prefix.len()..].trim().to_string(), prefix, editable: true }
    } else {
        Paragraph { prefix: String::new(), text: block.trim().to_string(), editable: true }
    }
}

/// PDF extractors emit one line per visual line; join lines inside a
/// paragraph and keep blank lines as paragraph breaks.
fn normalize_pdf_text(raw: &str) -> String {
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in raw.replace("\r\n", "\n").lines() {
        let l = line.trim();
        if l.is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() {
            if cur.ends_with('-') {
                cur.pop();
            } else {
                cur.push(' ');
            }
        }
        cur.push_str(l);
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    paras.join("\n\n")
}

// ---------------------------------------------------------------------------
// DOCX
// ---------------------------------------------------------------------------
static WT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<w:t(?:\s[^>]*)?>(.*?)</w:t>|<w:t(?:\s[^>]*)?/>|<w:tab\s*/>|<w:br\s*/>|<w:cr\s*/>").unwrap());
static PPR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<w:pPr\s*/>|<w:pPr(?:\s[^>]*)?>.*?</w:pPr>").unwrap());
static RPR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<w:rPr(?:\s[^>]*)?>.*?</w:rPr>").unwrap());

const UNTOUCHABLE: &[&str] = &[
    "<w:drawing", "<w:pict", "<w:object", "<w:fldChar", "<w:fldSimple", "<w:instrText", "<w:hyperlink",
    "<w:footnoteReference", "<w:endnoteReference", "<w:commentReference", "<w:sdt", "<m:oMath", "<w:txbxContent",
];

fn load_docx(path: &Path) -> Result<LoadedDoc, String> {
    let bytes = fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).map_err(|_| "Not a valid .docx file")?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|_| "Not a valid .docx file (missing word/document.xml)")?
        .read_to_string(&mut xml)
        .map_err(|e| e.to_string())?;

    let ranges = paragraph_ranges(&xml);
    let paragraphs = ranges
        .iter()
        .map(|&(s, e)| {
            let slice = &xml[s..e];
            let txt = extract_text(slice);
            let editable = !UNTOUCHABLE.iter().any(|t| slice.contains(t)) && !txt.trim().is_empty();
            Paragraph { prefix: String::new(), text: txt, editable }
        })
        .collect();
    Ok(LoadedDoc { kind: DocKind::Docx, path: path.to_path_buf(), paragraphs, docx: Some(DocxFile { bytes, xml, ranges }) })
}

/// Byte ranges of outermost `<w:p>` elements (paragraphs nested in text
/// boxes are left to their parent).
fn paragraph_ranges(xml: &str) -> Vec<(usize, usize)> {
    let b = xml.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &xml[i..];
        if rest.starts_with("<?") || rest.starts_with("<!") {
            i += rest.find('>').map(|p| p + 1).unwrap_or(rest.len());
            continue;
        }
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[..=close];
        let is_end = tag.starts_with("</");
        let self_closing = tag.ends_with("/>");
        let name_start = if is_end { 2 } else { 1 };
        let name: &str = tag[name_start..]
            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .next()
            .unwrap_or("");
        if name == "w:p" {
            if is_end {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        out.push((start, i + close + 1));
                    }
                }
            } else if self_closing {
                if depth == 0 {
                    out.push((i, i + close + 1));
                }
            } else {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
        }
        i += close + 1;
    }
    out
}

fn extract_text(p_xml: &str) -> String {
    let mut s = String::new();
    for cap in WT_RE.captures_iter(p_xml) {
        let whole = cap.get(0).unwrap().as_str();
        if whole.starts_with("<w:tab") {
            s.push('\t');
        } else if whole.starts_with("<w:br") || whole.starts_with("<w:cr") {
            s.push('\n');
        } else if let Some(t) = cap.get(1) {
            s.push_str(&unescape(t.as_str()));
        }
    }
    s
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Rebuilds a paragraph: same `<w:p>` attributes, same `<w:pPr>`, the first
/// run's `<w:rPr>` applied to one new run holding the text.
fn rebuild_paragraph(p_xml: &str, new_text: &str) -> String {
    let open_end = p_xml.find('>').map(|i| i + 1).unwrap_or(0);
    let open_tag = p_xml[..open_end].replace("/>", ">");
    let ppr = PPR_RE.find(p_xml).map(|m| m.as_str()).unwrap_or("");
    let after_ppr = PPR_RE.find(p_xml).map(|m| m.end()).unwrap_or(open_end);
    let rpr = RPR_RE.find(&p_xml[after_ppr..]).map(|m| m.as_str()).unwrap_or("");

    let mut runs = String::new();
    for (i, line) in new_text.split('\n').enumerate() {
        if i > 0 {
            runs.push_str("<w:r>");
            runs.push_str(rpr);
            runs.push_str("<w:br/></w:r>");
        }
        for (j, seg) in line.split('\t').enumerate() {
            if j > 0 {
                runs.push_str("<w:r>");
                runs.push_str(rpr);
                runs.push_str("<w:tab/></w:r>");
            }
            if !seg.is_empty() {
                runs.push_str("<w:r>");
                runs.push_str(rpr);
                runs.push_str("<w:t xml:space=\"preserve\">");
                runs.push_str(&escape(seg));
                runs.push_str("</w:t></w:r>");
            }
        }
    }
    format!("{open_tag}{ppr}{runs}</w:p>")
}

fn write_docx_surgical(d: &DocxFile, paras: &[Paragraph], new_texts: &[String], out: &Path) -> Result<(), String> {
    let mut xml = String::with_capacity(d.xml.len() + 4096);
    let mut cursor = 0;
    for (i, &(s, e)) in d.ranges.iter().enumerate() {
        xml.push_str(&d.xml[cursor..s]);
        let p = &paras[i];
        let nt = &new_texts[i];
        if p.editable && nt.trim() != p.text.trim() {
            xml.push_str(&rebuild_paragraph(&d.xml[s..e], nt));
        } else {
            xml.push_str(&d.xml[s..e]);
        }
        cursor = e;
    }
    xml.push_str(&d.xml[cursor..]);

    let mut archive = ZipArchive::new(Cursor::new(&d.bytes)).map_err(|e| e.to_string())?;
    let file = fs::File::create(out).map_err(|e| format!("Cannot create output file: {e}"))?;
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.name() == "word/document.xml" {
            zw.start_file("word/document.xml", opts).map_err(|e| e.to_string())?;
            zw.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
        } else {
            zw.raw_copy_file(entry).map_err(|e| e.to_string())?;
        }
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// A fresh, minimal .docx (used when the source was .txt/.md/.pdf or text box).
pub fn write_new_docx(paragraphs: &[String], out: &Path) -> Result<(), String> {
    let body: String = paragraphs
        .iter()
        .map(|p| {
            let runs: Vec<String> = p
                .split('\n')
                .map(|l| format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", escape(l)))
                .collect();
            format!("<w:p>{}</w:p>", runs.join("<w:r><w:br/></w:r>"))
        })
        .collect();
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>"#;

    let file = fs::File::create(out).map_err(|e| format!("Cannot create output file: {e}"))?;
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in [
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/document.xml", document.as_str()),
        ("word/styles.xml", styles),
    ] {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        zw.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub fn rebuild_paragraph_for_test(p_xml: &str, new_text: &str) -> String {
    rebuild_paragraph(p_xml, new_text)
}
