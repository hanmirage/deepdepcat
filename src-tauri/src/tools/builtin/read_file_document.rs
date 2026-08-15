//! Document text extraction for `read_file` (Code side) — PDF / docx / pptx.
//!
//! `read_file` previously reported binary for these formats. This module
//! detects a document by its magic bytes and dispatches to the right
//! extractor: docx reuses the shared `depwork::doc_read::extract_docx` (the
//! only `pub(crate)` extractor), PDF uses the mirrored pipeline in
//! `read_file_pdf`, and pptx is parsed here (zip of `ppt/slides/slideN.xml`,
//! scanning `<a:t>` text runs).

use crate::toolkit::ToolResult;
use crate::core::error::AppResult;
use std::io::{Cursor, Read};
use std::path::Path;

/// A document format `read_file` can extract text from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Pdf,
    Docx,
    Pptx,
}

/// Detect a document format from its magic bytes. `None` for images, plain
/// text, and unrecognised input (callers fall through to other branches).
pub fn detect_document(bytes: &[u8]) -> Option<DocumentKind> {
    if bytes.starts_with(b"%PDF-") {
        return Some(DocumentKind::Pdf);
    }
    // OOXML (docx/pptx/xlsx) are ZIP containers starting with PK\x03\x04.
    // Peek inside to distinguish docx (`word/document.xml`) from pptx
    // (`ppt/slides/`). xlsx is out of scope here.
    if bytes.starts_with(b"PK\x03\x04") {
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
        let names: Vec<String> = archive.file_names().map(|n| n.to_string()).collect();
        if names.iter().any(|n| n == "word/document.xml") {
            return Some(DocumentKind::Docx);
        }
        if names.iter().any(|n| n.starts_with("ppt/slides/")) {
            return Some(DocumentKind::Pptx);
        }
    }
    None
}

/// Extract text from a document and build the model-visible tool result.
///
/// `path` is the resolved absolute path (docx/pdf extractors open the file);
/// `bytes` is the already-read content (used by the pptx zip parser, avoids a
/// second disk read).
pub fn build_document_result(
    path: &Path,
    bytes: Vec<u8>,
    kind: DocumentKind,
) -> AppResult<ToolResult> {
    let extracted = match kind {
        DocumentKind::Pdf => crate::tools::builtin::read_file_pdf::extract_pdf_text(path)?,
        DocumentKind::Docx => crate::tools::builtin::depwork::doc_read::extract_docx(path)?,
        DocumentKind::Pptx => extract_pptx_text(&bytes).ok_or_else(|| {
            crate::core::error::AppError::Other("Not a valid pptx package".into())
        })?,
    };

    let label = match kind {
        DocumentKind::Pdf => "PDF",
        DocumentKind::Docx => "docx",
        DocumentKind::Pptx => "pptx",
    };
    if extracted.trim().is_empty() {
        return Ok(ToolResult::error(format!(
            "No extractable text found in {label} (scanned/image-based document? the image is transcribed automatically when attached to a message, or use OCR in Depwork mode)."
        )));
    }
    let summary = format!(
        "Document: {path:?} ({label}, {} chars)\n\n",
        extracted.chars().count()
    );
    let capped = crate::core::str_util::spill_tool_output(&format!("{summary}{extracted}"));
    Ok(ToolResult::success(capped))
}

/// Extract text runs from a pptx package (zip of `ppt/slides/slideN.xml`).
/// Each slide's `<a:t>…</a:t>` text is concatenated; slides separated by
/// blank lines. Returns `None` when the package has no readable slides.
fn extract_pptx_text(bytes: &[u8]) -> Option<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    // Collect slide entry names, sort by numeric suffix so document order
    // matches slide order.
    let mut slides: Vec<(usize, String)> = archive
        .file_names()
        .filter(|n| n.starts_with("ppt/slides/slide"))
        .filter(|n| n.ends_with(".xml"))
        .filter_map(|n| {
            let num: usize = n
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse()
                .ok()?;
            Some((num, n.to_string()))
        })
        .collect();
    slides.sort_by_key(|(num, _)| *num);

    let mut out = String::new();
    for (_, name) in slides {
        let mut entry = archive.by_name(&name).ok()?;
        let mut xml = String::new();
        entry.read_to_string(&mut xml).ok()?;
        let slide_text = extract_at_texts(&xml);
        if !slide_text.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&slide_text);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Scan XML for every `<a:t>…</a:t>` run and join their contents.
fn extract_at_texts(xml: &str) -> String {
    let mut out = String::new();
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i + 4 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'a' && bytes[i + 2] == b':' && bytes[i + 3] == b't'
        {
            // Skip past `<a:t>` (possibly with attributes / self-close).
            let Some(close) = find_tag_end(bytes, i) else {
                break;
            };
            // `<a:t ...>` opens a run; `<a:t/>` is empty and closed.
            if close > i && bytes[close - 1] == b'/' {
                i = close + 1;
                continue;
            }
            let Some(end) = find_subsequence(bytes, close + 1, b"</a:t>") else {
                break;
            };
            out.push_str(&xml[close + 1..end]);
            i = end + 6;
        } else {
            i += 1;
        }
    }
    out
}

/// Find the `>` that closes the tag starting at `start` (which is `<`).
fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find `needle` in `haystack` starting at `from`.
fn find_subsequence(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    let mut i = from;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn detect_document_recognizes_formats() {
        // PDF magic.
        assert_eq!(detect_document(b"%PDF-1.7 ..."), Some(DocumentKind::Pdf));
        // docx: zip with word/document.xml.
        let dir = std::env::temp_dir().join("deepdepcat-detect-test");
        std::fs::create_dir_all(&dir).unwrap();
        let docx_path = dir.join("sample.docx");
        write_zip(
            &docx_path,
            &[(
                "word/document.xml",
                "<w:document><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:document>",
            )],
        );
        let docx_bytes = std::fs::read(&docx_path).unwrap();
        assert_eq!(detect_document(&docx_bytes), Some(DocumentKind::Docx));
        // pptx: zip with ppt/slides/slide1.xml.
        let pptx_path = dir.join("sample.pptx");
        write_zip(
            &pptx_path,
            &[("ppt/slides/slide1.xml", "<p:sld><a:t>Hello</a:t></p:sld>")],
        );
        let pptx_bytes = std::fs::read(&pptx_path).unwrap();
        assert_eq!(detect_document(&pptx_bytes), Some(DocumentKind::Pptx));
        // Non-documents.
        assert_eq!(detect_document(b"plain text, not a doc"), None);
        assert_eq!(detect_document(b""), None);
    }

    #[test]
    fn extract_pptx_text_collects_slide_runs() {
        let dir = std::env::temp_dir().join("deepdepcat-pptx-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deck.pptx");
        write_zip(
            &path,
            &[
                (
                    "ppt/slides/slide1.xml",
                    "<p:sld><a:p><a:r><a:t>First slide</a:t></a:r></a:p></p:sld>",
                ),
                (
                    "ppt/slides/slide2.xml",
                    "<p:sld><a:p><a:r><a:t>Second</a:t></a:r></a:p></p:sld>",
                ),
            ],
        );
        let bytes = std::fs::read(&path).unwrap();
        let text = extract_pptx_text(&bytes).expect("slides extracted");
        assert!(text.contains("First slide"));
        assert!(text.contains("Second"));
        // Slides are separated (multi-line output).
        assert!(text.contains('\n'));
    }

    #[test]
    fn build_document_result_extracts_docx() {
        let dir = std::env::temp_dir().join("deepdepcat-docx-result-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.docx");
        write_zip(
            &path,
            &[(
                "word/document.xml",
                "<w:document><w:p><w:r><w:t>文档内容正文</w:t></w:r></w:p></w:document>",
            )],
        );
        let bytes = std::fs::read(&path).unwrap();
        let result = build_document_result(&path, bytes, DocumentKind::Docx).unwrap();
        assert!(!result.is_error, "docx must extract: {:?}", result.content);
        assert!(result.content.contains("文档内容正文"));
        assert!(result.content.contains("docx"), "summary labels the format");
    }

    #[test]
    fn build_document_result_errors_on_empty_text() {
        // A docx whose document.xml has no text → model-visible error.
        let dir = std::env::temp_dir().join("deepdepcat-docx-empty-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.docx");
        write_zip(
            &path,
            &[("word/document.xml", "<w:document><w:p/></w:document>")],
        );
        let bytes = std::fs::read(&path).unwrap();
        let result = build_document_result(&path, bytes, DocumentKind::Docx).unwrap();
        assert!(result.is_error);
        assert!(
            result.content.contains("No extractable text"),
            "got: {}",
            result.content
        );
    }
}
