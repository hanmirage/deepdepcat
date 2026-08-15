//! doc_read — extract text content from common document formats.
//!
//! Supported formats:
//! - `.txt` / `.md` / `.csv` — plain text (CSV shows headers + row count)
//! - `.docx` — Word documents (paragraph text, tables flattened)
//! - `.pdf` — PDF text extraction with ToUnicode CMap decoding (Chinese /
//!   CID-encoded PDFs decode correctly), page-range reads, and optional
//!   structured metadata.
//!
//! PDF extraction pipeline (per page):
//!   1. scan the content stream for string literals (`(…)` and `<hex>`
//!      operands of Tj / TJ), unescaping `\(` `\)` `\\` and octal runs
//!   2. build a CID→Unicode map from every font's ToUnicode CMap
//!      (beginbfchar / beginbfrange)
//!   3. decode each string: exact CMap hit → UTF-16BE (BOM) → per-glyph
//!      CMap (2-byte CID, then 1-byte) → Latin-1 fallback

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use async_trait::async_trait;
use lopdf::Object;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// Extract text from a document file.
pub struct DocReadTool;

impl DocReadTool {
    pub fn new() -> Self {
        Self
    }
}

/// Decode one XML entity name ("amp", "lt", "#x4E2D", "#20013") to its
/// character. Unknown names return None (the caller keeps the raw text).
fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        _ => {}
    }
    let code = if let Some(hex) = name.strip_prefix("#x") {
        u32::from_str_radix(hex, 16).ok()
    } else if let Some(dec) = name.strip_prefix('#') {
        dec.parse::<u32>().ok()
    } else {
        None
    };
    code.and_then(char::from_u32)
}

/// Extract plain text from a `.docx` file (Word).
///
/// Unzips the package, parses `word/document.xml`, and concatenates all
/// `w:t` runs with paragraph breaks. Tables are flattened row by row.
/// XML entities inside `w:t` runs are decoded (`&amp;` → `&`, `&#x4E2D;` → 中)
/// so the model sees the text Word intended, not the serialized form.
pub(crate) fn extract_docx(path: &Path) -> AppResult<String> {
    let file = std::fs::File::open(path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid docx package: {e}"))?;
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|e| format!("docx has no word/document.xml: {e}"))?;
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut document, &mut xml)
        .map_err(|e| format!("Failed to read docx XML: {e}"))?;

    let mut out = String::new();
    let mut in_paragraph = false;
    let mut in_row = false;
    // Accumulated entity name after '&' (until ';').
    let mut entity_buf: Option<String> = None;
    let mut chars = xml.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                let tag: String = chars.by_ref().take_while(|&c| c != '>').collect();
                let lower = tag.to_ascii_lowercase();
                // NOTE: `</w:pPr>` and `<w:pStyle/>` share the `w:p` prefix —
                // match paragraph elements exactly, not by prefix. `lower` is
                // already lowercased, so exclusion patterns must be too.
                if lower.starts_with("/w:p") && !lower.starts_with("/w:ppr") {
                    out.push('\n');
                    in_paragraph = false;
                } else if super::is_paragraph_open_tag(&lower) {
                    in_paragraph = true;
                } else if lower.starts_with("/w:tr") {
                    out.push('\n');
                    in_row = false;
                } else if lower.starts_with("w:tr") && !lower.starts_with("w:trpr") {
                    in_row = true;
                } else if lower.starts_with("/w:tc") && in_row {
                    out.push_str(" | ");
                } else if lower.starts_with("w:t") && !lower.starts_with("w:tab") {
                    // text content follows raw (possibly escaped) until </w:t>
                }
            }
            '>' => {}
            c if in_paragraph => {
                if let Some(buf) = entity_buf.as_mut() {
                    if c == ';' {
                        let name = buf.clone();
                        match decode_entity(&name) {
                            Some(decoded) => out.push(decoded),
                            None => {
                                out.push('&');
                                out.push_str(&name);
                                out.push(';');
                            }
                        }
                        entity_buf = None;
                    } else if buf.len() < 12 {
                        buf.push(c);
                    } else {
                        // Not a real entity — emit the buffer verbatim.
                        out.push('&');
                        out.push_str(buf);
                        out.push(c);
                        entity_buf = None;
                    }
                } else if c == '&' {
                    entity_buf = Some(String::new());
                } else {
                    out.push(c);
                }
            }
            _ => {}
        }
    }
    Ok(out.trim().to_string())
}

// ── PDF extraction ────────────────────────────────────────────

/// One decoded page of PDF text.
#[derive(Debug)]
pub(crate) struct PdfPage {
    page_no: usize,
    pub(crate) text: String,
}

/// Scan a content stream for string literals — the `(…)` and `<hex>`
/// operands of the Tj / TJ text operators. Escapes are unescaped.
fn extract_content_strings(content: &[u8], out: &mut Vec<Vec<u8>>) {
    let mut i = 0usize;
    while i < content.len() {
        match content[i] {
            b'(' => {
                let mut j = i + 1;
                let mut s: Vec<u8> = Vec::new();
                while j < content.len() {
                    match content[j] {
                        b'\\' => {
                            if j + 1 < content.len() {
                                let e = content[j + 1];
                                match e {
                                    b'n' => s.push(b'\n'),
                                    b'r' => s.push(b'\r'),
                                    b't' => s.push(b'\t'),
                                    b'b' => s.push(0x08),
                                    b'f' => s.push(0x0C),
                                    b'(' => s.push(b'('),
                                    b')' => s.push(b')'),
                                    b'\\' => s.push(b'\\'),
                                    b'0'..=b'7' => {
                                        // Octal escape — up to 3 digits.
                                        let mut val: u8 = 0;
                                        let mut k = j + 1;
                                        let mut digits = 0;
                                        while k < content.len()
                                            && digits < 3
                                            && (b'0'..=b'7').contains(&content[k])
                                        {
                                            val =
                                                val.wrapping_mul(8).wrapping_add(content[k] - b'0');
                                            k += 1;
                                            digits += 1;
                                        }
                                        s.push(val);
                                        // k is the first unconsumed index; the
                                        // shared `j += 2` below must land there.
                                        j = k - 2;
                                    }
                                    _ => s.push(e),
                                }
                                j += 2;
                            } else {
                                j += 1;
                            }
                        }
                        b')' => {
                            j += 1;
                            break;
                        }
                        _ => {
                            s.push(content[j]);
                            j += 1;
                        }
                    }
                }
                out.push(s);
                i = j;
            }
            b'<' => {
                // Hex string — whitespace is ignored between digits.
                let mut j = i + 1;
                let mut s: Vec<u8> = Vec::new();
                let mut hi: Option<u8> = None;
                while j < content.len() && content[j] != b'>' {
                    let v = match content[j] {
                        b'0'..=b'9' => content[j] - b'0',
                        b'a'..=b'f' => content[j] - b'a' + 10,
                        b'A'..=b'F' => content[j] - b'A' + 10,
                        _ => {
                            j += 1;
                            continue;
                        }
                    };
                    match hi {
                        None => hi = Some(v),
                        Some(h) => {
                            s.push((h << 4) | v);
                            hi = None;
                        }
                    }
                    j += 1;
                }
                out.push(s);
                i = j + 1;
            }
            _ => i += 1,
        }
    }
}

/// Parse `<hex>` bytes from a CMap token (`<4E2D>`).
fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.starts_with('<') || !s.ends_with('>') {
        return None;
    }
    let inner: String = s[1..s.len() - 1]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !inner.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(inner.len() / 2);
    let bytes = inner.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// Decode a CMap destination value (usually UTF-16BE bytes).
fn decode_cmap_dst(dst: &[u8]) -> String {
    if !dst.is_empty() && dst.len().is_multiple_of(2) {
        let mut out = String::new();
        let mut ok = true;
        for chunk in dst.chunks_exact(2) {
            let u = u16::from_be_bytes([chunk[0], chunk[1]]);
            if u == 0 || (0xD800..=0xDFFF).contains(&u) {
                ok = false;
                break;
            }
            match char::from_u32(u as u32) {
                Some(c) => out.push(c),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return out;
        }
    }
    // Latin-1 fallback.
    dst.iter()
        .map(|&b| char::from_u32(b as u32).unwrap_or('?'))
        .collect()
}

/// Parse a ToUnicode CMap stream into a byte→Unicode mapping.
/// Handles `beginbfchar` (single pairs) and `beginbfrange` (expanded
/// ranges, e.g. `<4E00> <4E5F> <9AD9>` → 0x4E00→高, 0x4E01→竤, …).
///
/// Real CMaps prefix the sections with a count (`1 beginbfchar`) — the
/// section markers are matched anywhere in the line, not at line start.
fn parse_cmap(data: &[u8]) -> HashMap<Vec<u8>, String> {
    let text = String::from_utf8_lossy(data);
    let mut map: HashMap<Vec<u8>, String> = HashMap::new();
    let mut in_bfchar = false;
    let mut in_bfrange = false;

    for line in text.lines() {
        let line = line.trim();
        if line.contains("beginbfchar") {
            in_bfchar = true;
            in_bfrange = false;
            continue;
        }
        if line.contains("endbfchar") {
            in_bfchar = false;
            continue;
        }
        if line.contains("beginbfrange") {
            in_bfrange = true;
            in_bfchar = false;
            continue;
        }
        if line.contains("endbfrange") {
            in_bfrange = false;
            continue;
        }

        if in_bfchar {
            let mut it = line.split_whitespace();
            if let (Some(src), Some(dst)) = (it.next(), it.next()) {
                if let (Some(src_b), Some(dst_b)) = (parse_hex_bytes(src), parse_hex_bytes(dst)) {
                    map.insert(src_b, decode_cmap_dst(&dst_b));
                }
            }
        } else if in_bfrange {
            let mut it = line.split_whitespace();
            if let (Some(lo), Some(hi), Some(dst)) = (it.next(), it.next(), it.next()) {
                if let (Some(lo_b), Some(hi_b), Some(dst_b)) = (
                    parse_hex_bytes(lo),
                    parse_hex_bytes(hi),
                    parse_hex_bytes(dst),
                ) {
                    if lo_b.is_empty() || lo_b.len() != hi_b.len() || dst_b.is_empty() {
                        continue;
                    }
                    let mut code = lo_b.clone();
                    let mut dst_val = dst_b.clone();
                    // Malformed CMaps can declare lo > hi — bound the
                    // expansion so a hostile document can't hang us.
                    let mut steps = 0u32;
                    loop {
                        map.insert(code.clone(), decode_cmap_dst(&dst_val));
                        if code == hi_b {
                            break;
                        }
                        steps += 1;
                        if steps > 4096 {
                            break;
                        }
                        // Big-endian +1 on both the code and the destination.
                        for i in (0..code.len()).rev() {
                            let (v, carry) = code[i].overflowing_add(1);
                            code[i] = v;
                            if !carry {
                                break;
                            }
                        }
                        for i in (0..dst_val.len()).rev() {
                            let (v, carry) = dst_val[i].overflowing_add(1);
                            dst_val[i] = v;
                            if !carry {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    map
}

/// Collect the ToUnicode mappings of every font used on a page.
fn page_tounicode(
    document: &lopdf::Document,
    page_id: lopdf::ObjectId,
) -> HashMap<Vec<u8>, String> {
    let mut map = HashMap::new();
    // lopdf 0.44 returns (direct resources, dependency object ids).
    let Ok((resources, _deps)) = document.get_page_resources(page_id) else {
        return map;
    };
    let Some(resources) = resources else {
        return map;
    };
    let Ok(fonts) = resources.get(b"Font") else {
        return map;
    };
    let Ok(font_dict) = fonts.as_dict() else {
        return map;
    };
    for (_, font_ref) in font_dict.iter() {
        // Font entries are either indirect references or inline dicts.
        // lopdf's get_object returns &Object.
        let font_obj = match font_ref {
            Object::Reference(id) => match document.get_object(*id) {
                Ok(obj) => obj,
                Err(_) => continue,
            },
            other => other,
        };
        let Ok(font_dict) = font_obj.as_dict() else {
            continue;
        };
        let Ok(tounicode) = font_dict.get(b"ToUnicode") else {
            continue;
        };
        let Ok(stream_ref) = tounicode.as_reference() else {
            continue;
        };
        let Ok(stream_obj) = document.get_object(stream_ref) else {
            continue;
        };
        let Ok(stream) = stream_obj.as_stream() else {
            continue;
        };
        if let Ok(data) = stream.decompressed_content() {
            map.extend(parse_cmap(&data));
        }
    }
    map
}

/// Decode one byte string from the content stream to text.
///
/// Priority: exact CMap hit → UTF-16BE with BOM → per-glyph CMap
/// (2-byte CID, then 1-byte code) → Latin-1 fallback.
fn decode_pdf_string(s: &[u8], cmap: &HashMap<Vec<u8>, String>) -> String {
    if s.is_empty() {
        return String::new();
    }
    if let Some(decoded) = cmap.get(s) {
        return decoded.clone();
    }
    if s.len() >= 4 && s[0] == 0xFE && s[1] == 0xFF {
        // Explicit UTF-16BE BOM.
        let mut out = String::new();
        for chunk in s[2..].chunks_exact(2) {
            let u = u16::from_be_bytes([chunk[0], chunk[1]]);
            if u != 0 {
                if let Some(c) = char::from_u32(u as u32) {
                    out.push(c);
                }
            }
        }
        return out;
    }
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        if i + 1 < s.len() {
            if let Some(d) = cmap.get(&s[i..i + 2]) {
                out.push_str(d);
                i += 2;
                continue;
            }
        }
        if let Some(d) = cmap.get(&s[i..i + 1]) {
            out.push_str(d);
            i += 1;
            continue;
        }
        out.push(char::from_u32(s[i] as u32).unwrap_or('?'));
        i += 1;
    }
    out
}

/// Extract text from `start_page..=end_page` (1-based, None = all pages).
/// Returns the pages plus the document's total page count.
pub(crate) fn extract_pdf_pages(
    path: &Path,
    start_page: Option<usize>,
    end_page: Option<usize>,
) -> AppResult<(Vec<PdfPage>, usize)> {
    let document = lopdf::Document::load(path)?;

    // lopdf 0.44 get_pages(): page_number → page ObjectId. Sort by page
    // number so extraction follows document order.
    let mut page_ids: Vec<(u32, lopdf::ObjectId)> = document
        .get_pages()
        .iter()
        .map(|(page_no, page_id)| (*page_no, *page_id))
        .collect();
    page_ids.sort_by_key(|(page_no, _)| *page_no);
    let total = page_ids.len();

    let mut pages = Vec::new();
    for (idx, (_, page_id)) in page_ids.iter().enumerate() {
        let page_no = idx + 1;
        if let Some(s) = start_page {
            if page_no < s {
                continue;
            }
        }
        if let Some(e) = end_page {
            if page_no > e {
                continue;
            }
        }
        let mut strings = Vec::new();
        let content = document.get_page_content(*page_id);
        extract_content_strings(&content, &mut strings);
        let cmap = page_tounicode(&document, *page_id);
        let text: String = strings
            .iter()
            .map(|s| decode_pdf_string(s, &cmap))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        pages.push(PdfPage { page_no, text });
    }

    if pages.is_empty() {
        return Err(AppError::Other(
            "No pages matched the requested range".into(),
        ));
    }
    let total_chars: usize = pages.iter().map(|p| p.text.chars().count()).sum();
    if total_chars == 0 {
        return Err(AppError::Other(
            "No extractable text found (scanned PDF? try the OCR tool)".into(),
        ));
    }
    Ok((pages, total))
}

/// Detect encoding when a text file is not valid UTF-8 (GBK fallback).
fn read_text_file(path: &Path) -> AppResult<String> {
    let bytes = std::fs::read(path)?;
    Ok(crate::core::encoding::decode_native_output(&bytes))
}

/// Default cap on characters injected into context per read.
const DEFAULT_MAX_CHARS: usize = 60_000;

/// Truncate to `max` chars, appending a marker that reports the total.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!(
        "{head}\n\n… [truncated: full document is {} chars, showing first {max}]",
        s.chars().count()
    )
}

#[async_trait]
impl Tool for DocReadTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "doc_read"
    }

    fn description(&self) -> &str {
        "Extract text content from a document file for office work. \
        Supports .txt, .md, .csv, .docx (Word), and .pdf (text, with ToUnicode \
        decoding — Chinese and CID-encoded PDFs work). \
        PDFs support start_page / end_page (1-based, inclusive) to read a \
        range instead of the whole file; include_meta=true (PDF only) returns \
        structured JSON (pages, chars, paragraphs, preview) instead of raw text. \
        Use this before writing, summarizing, or comparing documents."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the document file."
                },
                "start_page": {
                    "type": "integer",
                    "description": "PDF only — first page to read (1-based, inclusive)."
                },
                "end_page": {
                    "type": "integer",
                    "description": "PDF only — last page to read (1-based, inclusive)."
                },
                "include_meta": {
                    "type": "boolean",
                    "description": "PDF only — return structured JSON metadata (pages/chars/paragraphs/preview) instead of the raw text."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Cap on characters injected into context (default 60000, min 1000). Bypassed when include_meta=true (the metadata path does not truncate)."
                }
            },
            "required": ["path"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_str);
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()).into());
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // Page range + metadata apply to PDFs only.
        let start_page = args
            .get("start_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let end_page = args
            .get("end_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let include_meta = args
            .get("include_meta")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Context guard: cap injected characters on EVERY path (txt/docx/pdf).
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_CHARS)
            .max(1_000);

        // PDF path builds its own header (page range + count) — handled
        // separately so the shared wrapper below stays single-level.
        if ext == "pdf" {
            let (pages, total_pages) = extract_pdf_pages(&path, start_page, end_page)?;
            let first = pages.first().map(|p| p.page_no).unwrap_or(0);
            let last = pages.last().map(|p| p.page_no).unwrap_or(0);
            let extracted_chars: usize = pages.iter().map(|p| p.text.chars().count()).sum();

            let body = if include_meta {
                let full: String = pages
                    .iter()
                    .map(|p| p.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                let paragraphs = full.lines().filter(|l| !l.trim().is_empty()).count();
                let preview: String = full.chars().take(500).collect();
                json!({
                    "path": path.to_string_lossy(),
                    "pages": total_pages,
                    "page_range": { "start": first, "end": last },
                    "chars": extracted_chars,
                    "paragraphs": paragraphs,
                    "preview": preview,
                })
                .to_string()
            } else {
                let mut out = String::new();
                for page in &pages {
                    if pages.len() > 1 {
                        out.push_str(&format!("\n\n[Page {}]\n", page.page_no));
                    }
                    out.push_str(&page.text);
                }
                truncate_chars(&out, max_chars)
            };

            let header = if total_pages > 1 {
                format!(
                    "--- Document: {path}\n(extracted {extracted_chars} chars, pages {first}-{last} of {total_pages})\n\n",
                    path = path.display()
                )
            } else {
                format!(
                    "--- Document: {path}\n(extracted {extracted_chars} chars)\n\n",
                    path = path.display()
                )
            };
            return Ok(ToolResult::success(format!("{header}{}", body.trim())));
        }

        let content = match ext.as_str() {
            "txt" | "md" | "markdown" | "csv" | "log" | "json" | "yml" | "yaml" | "toml" => {
                let text = read_text_file(&path)?;
                let body = if ext == "csv" {
                    let rows = text.lines().count().saturating_sub(1);
                    let header = text.lines().next().unwrap_or("");
                    format!("CSV file — {rows} data rows\nHeaders: {header}\n\n{text}")
                } else {
                    text
                };
                truncate_chars(&body, max_chars)
            }
            "docx" => truncate_chars(&extract_docx(&path)?, max_chars),
            other => {
                return Err(format!(
                    "Unsupported document format: .{other}. Supported: .txt .md .csv .docx .pdf"
                )
                .into())
            }
        };

        Ok(ToolResult::success(format!(
            "--- Document: {}\n(extracted {} chars)\n\n{}",
            path.display(),
            content.chars().count(),
            content
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── docx extraction (unchanged behavior) ───────────────────

    #[test]
    fn docx_extraction_handles_paragraphs() {
        let xml = concat!(
            "<?xml version=\"1.0\"?><w:document><w:body>",
            "<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>Second</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.docx");
        let file = std::fs::File::create(&path).expect("create");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("start");
        std::io::Write::write_all(&mut zip, xml.as_bytes()).expect("write");
        zip.finish().expect("finish");

        let text = extract_docx(&path).expect("extract");
        assert!(text.contains("Hello World"));
        assert!(text.contains("Second"));
        assert_eq!(text.matches('\n').count(), 1, "one paragraph break");
    }

    #[test]
    fn docx_extraction_handles_attributed_paragraphs() {
        // Real Word docs attach w14:paraId / w14:textId (and w:rsidR) to
        // <w:p> — an exact `== "w:p"` match silently drops these paragraphs.
        let xml = concat!(
            "<?xml version=\"1.0\"?><w:document><w:body>",
            "<w:p w14:paraId=\"4B7A2E11\" w14:textId=\"77777777\" w:rsidR=\"00C95278\"><w:r><w:t>Attributed paragraph</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>Plain paragraph</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.docx");
        let file = std::fs::File::create(&path).expect("create");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("start");
        std::io::Write::write_all(&mut zip, xml.as_bytes()).expect("write");
        zip.finish().expect("finish");

        let text = extract_docx(&path).expect("extract");
        assert!(text.contains("Attributed paragraph"));
        assert!(text.contains("Plain paragraph"));
    }

    #[test]
    fn docx_extraction_with_properties_and_namespace() {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>",
            "<w:p><w:pPr><w:pStyle w:val=\"Title\"/><w:spacing w:after=\"240\"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>",
            "<w:p><w:pPr><w:spacing w:after=\"120\"/></w:pPr><w:r><w:t xml:space=\"preserve\">Body para</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t2.docx");
        let file = std::fs::File::create(&path).expect("create");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("start");
        std::io::Write::write_all(&mut zip, xml.as_bytes()).expect("write");
        zip.finish().expect("finish");

        let text = extract_docx(&path).expect("extract");
        assert!(text.contains("Quarterly Report"));
        assert!(text.contains("Body para"));
    }

    #[test]
    fn docx_extraction_unescapes_entities() {
        let xml = concat!(
            "<?xml version=\"1.0\"?><w:document><w:body>",
            "<w:p><w:r><w:t>Tom &amp; Jerry</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>&lt;tag&gt; &quot;quoted&quot; &apos;sq&apos;</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>&#x4E2D;&#x6587; and &#20013;&#25991;</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>unknown &nbsp; entity stays raw &foo;</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("entities.docx");
        let file = std::fs::File::create(&path).expect("create");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("start");
        std::io::Write::write_all(&mut zip, xml.as_bytes()).expect("write");
        zip.finish().expect("finish");

        let text = extract_docx(&path).expect("extract");
        assert!(text.contains("Tom & Jerry"), "named escape: {text}");
        assert!(
            text.contains("<tag> \"quoted\" 'sq'"),
            "named escapes: {text}"
        );
        assert!(text.contains("中文 and 中文"), "numeric escapes: {text}");
        assert!(text.contains("&nbsp;"), "unknown named entity stays raw");
        assert!(text.contains("&foo;"), "unknown entity stays raw");
        assert!(!text.contains("&amp;"), "no raw amp left: {text}");
    }

    // ── PDF string scanner ─────────────────────────────────────

    #[test]
    fn content_scanner_collects_tj_and_tj_arrays() {
        let stream =
            b"BT /F1 24 Tf 72 720 Td (Hello World) Tj ET\nBT /F2 12 Tf 0 0 Td [(A) (B) (C)] TJ ET";
        let mut strings = Vec::new();
        extract_content_strings(stream, &mut strings);
        assert_eq!(strings.len(), 4);
        assert_eq!(strings[0], b"Hello World");
        assert_eq!(strings[1], b"A");
        assert_eq!(strings[2], b"B");
        assert_eq!(strings[3], b"C");
    }

    #[test]
    fn content_scanner_unescapes_literals_and_octals() {
        let stream = b"(a\\(b\\)c\\\\d) Tj (\\050 octal) Tj";
        let mut strings = Vec::new();
        extract_content_strings(stream, &mut strings);
        assert_eq!(strings[0], b"a(b)c\\d");
        // \050 is the escaped `(`; the closing `)` is the string delimiter
        // and is NOT part of the content.
        assert_eq!(strings[1], b"( octal");
    }

    #[test]
    fn content_scanner_reads_hex_strings() {
        let stream = b"<4E2D6587> Tj";
        let mut strings = Vec::new();
        extract_content_strings(stream, &mut strings);
        assert_eq!(strings[0], vec![0x4E, 0x2D, 0x65, 0x87]);
    }

    // ── ToUnicode CMap ─────────────────────────────────────────

    #[test]
    fn cmap_parses_bfchar_and_bfrange() {
        let cmap = b"\
/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfchar
<4E2D> <4E2D>
endbfchar
1 beginbfrange
<4E30> <4E32> <4E30>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end";

        let map = parse_cmap(cmap);
        // bfchar: CID 4E2D → 中 (U+4E2D).
        assert_eq!(map.get(&vec![0x4E, 0x2D]).map(|s| s.as_str()), Some("中"));
        // bfrange: 4E30 → 丰, 4E31 → 丱, 4E32 → 串 (each +1 from 4E30).
        assert_eq!(map.get(&vec![0x4E, 0x30]).map(|s| s.as_str()), Some("丰"));
        assert_eq!(map.get(&vec![0x4E, 0x31]).map(|s| s.as_str()), Some("丱"));
        assert_eq!(map.get(&vec![0x4E, 0x32]).map(|s| s.as_str()), Some("串"));
    }

    #[test]
    fn decode_string_uses_cmap_then_utf16_then_latin1() {
        let mut cmap = HashMap::new();
        cmap.insert(vec![0x4E, 0x2D], "中".to_string());
        cmap.insert(vec![0x65, 0x87], "文".to_string());

        // Per-glyph 2-byte CID decoding.
        assert_eq!(decode_pdf_string(&[0x4E, 0x2D, 0x65, 0x87], &cmap), "中文");
        // UTF-16BE with BOM.
        assert_eq!(
            decode_pdf_string(&[0xFE, 0xFF, 0x4E, 0x2D, 0x65, 0x87], &cmap),
            "中文"
        );
        // Latin-1 fallback for plain ASCII without a CMap.
        assert_eq!(decode_pdf_string(b"hello", &HashMap::new()), "hello");
    }

    // ── End-to-end PDF extraction ──────────────────────────────

    /// Build a two-page PDF: page 1 = ASCII "Hello World" (Latin-1 font,
    /// no CMap); page 2 = CID font with a ToUnicode CMap mapping
    /// <4E2D>→中 and <6587>→文, content stream `(4E2D6587) Tj`.
    fn build_test_pdf() -> (tempfile::TempDir, std::path::PathBuf) {
        use lopdf::{dictionary, Dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");

        // Font with ToUnicode (used by page 2).
        let cmap_stream = Stream::new(
            Dictionary::new(),
            b"beginbfchar\n<4E2D> <4E2D>\n<6587> <6587>\nendbfchar\n".to_vec(),
        );
        let cmap_id = doc.add_object(cmap_stream);
        let cid_font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "HeiseiMin-W3",
            "Encoding" => "Identity-H",
            "ToUnicode" => cmap_id,
        });
        // Plain Latin-1 font (page 1) — no ToUnicode.
        let latin_font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });

        let content1 = Stream::new(
            Dictionary::new(),
            b"BT /F1 24 Tf 72 720 Td (Hello World) Tj ET".to_vec(),
        );
        let content1_id = doc.add_object(content1);
        // CID bytes for 中文 (0x4E2D 0x6587) via the hex string operator.
        let content2 = Stream::new(
            Dictionary::new(),
            b"BT /F1 24 Tf 72 720 Td <4E2D6587> Tj ET".to_vec(),
        );
        let content2_id = doc.add_object(content2);

        let pages_id = doc.new_object_id();

        let page1_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => latin_font_id } },
            "Contents" => content1_id,
        });
        let page2_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => cid_font_id } },
            "Contents" => content2_id,
        });

        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page1_id), Object::Reference(page2_id)],
                "Count" => 2,
            }),
        );

        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.pdf");
        doc.save(&path).expect("save pdf");
        (dir, path)
    }

    #[test]
    fn pdf_extracts_ascii_and_cmap_chinese_pages() {
        let (_dir, path) = build_test_pdf();
        let (pages, total) = extract_pdf_pages(&path, None, None).expect("extract");
        assert_eq!(total, 2);
        assert_eq!(pages.len(), 2);
        assert!(pages[0].text.contains("Hello World"));
        assert_eq!(pages[1].text, "中文");
    }

    #[test]
    fn pdf_page_range_filters_pages() {
        let (_dir, path) = build_test_pdf();
        let (pages, total) = extract_pdf_pages(&path, Some(2), Some(2)).expect("extract");
        assert_eq!(total, 2);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].page_no, 2);
        assert_eq!(pages[0].text, "中文");
    }

    #[test]
    fn pdf_rejects_empty_range() {
        let (_dir, path) = build_test_pdf();
        let err = extract_pdf_pages(&path, Some(99), Some(100)).expect_err("must error");
        assert!(err.to_string().contains("No pages matched"));
    }

    #[test]
    fn truncate_marks_overflow() {
        let long: String = "字".repeat(5000);
        let out = truncate_chars(&long, 1000);
        assert!(out.contains("truncated: full document is 5000 chars"));
        assert!(out.chars().count() < 1100);
        // Short content passes through untouched.
        assert_eq!(truncate_chars("short", 1000), "short");
    }
}
