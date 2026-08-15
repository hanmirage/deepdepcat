//! PDF text extraction for `read_file` (Code side).
//!
//! Mirrors the pipeline in `depwork/doc_read.rs` — the doc_read entry is
//! private and the depwork files are not modified (separate ownership), so the
//! self-contained pipeline (string-literal scan + ToUnicode CMap decoding,
//! which handles CID/Chinese-encoded PDFs) is reproduced here. Depends only on
//! `lopdf`, already in Cargo.toml.

use crate::core::error::{AppError, AppResult};
use lopdf::Object;
use std::collections::HashMap;
use std::path::Path;

/// Extract plain text from a PDF file (all pages, joined with newlines).
pub fn extract_pdf_text(path: &Path) -> AppResult<String> {
    let (pages, _total) = extract_pdf_pages(path, None, None)?;
    Ok(pages
        .iter()
        .map(|p| p.text.clone())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// One decoded page of PDF text.
#[derive(Debug)]
struct PdfPage {
    text: String,
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
/// ranges). Real CMaps prefix sections with a count (`1 beginbfchar`) — the
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
fn extract_pdf_pages(
    path: &Path,
    start_page: Option<usize>,
    end_page: Option<usize>,
) -> AppResult<(Vec<PdfPage>, usize)> {
    let document = lopdf::Document::load(path)?;

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
        pages.push(PdfPage { text });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a real PDF with a ToUnicode CMap (maps 0x4E2D→中, 0x6587→文) so
    /// the extractor can decode CID-encoded Chinese — mirrors depwork's own
    /// `doc_read` test fixture (which is private; this is test-only code).
    fn build_chinese_pdf() -> (tempfile::TempDir, std::path::PathBuf) {
        use lopdf::{dictionary, Dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
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
        let content = Stream::new(
            Dictionary::new(),
            b"BT /F1 24 Tf 72 720 Td <4E2D6587> Tj ET".to_vec(),
        );
        let content_id = doc.add_object(content);
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => cid_font_id } },
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
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
    fn extract_pdf_text_recovers_cmap_chinese() {
        let (_dir, path) = build_chinese_pdf();
        let text = extract_pdf_text(&path).expect("pdf extracts");
        assert_eq!(text, "中文");
    }

    #[test]
    fn extract_pdf_text_errors_on_missing_file() {
        let result = extract_pdf_text(Path::new("C:/definitely/missing/file.pdf"));
        assert!(result.is_err(), "missing file must error");
    }
}
