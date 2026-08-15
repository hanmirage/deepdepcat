//! pdf_generate — create a PDF document from Markdown content.
//!
//! Pure Rust (lopdf), no Office install needed. Renders a simplified
//! markdown subset (headings, bullet/numbered lists, paragraphs) onto A4
//! pages with pagination.
//!
//! Text encoding: everything is emitted as UTF-16BE through a Type0
//! composite font (Identity-H) named after a standard Asian CID font
//! (STSong-Light). The font is NOT embedded — viewers (browsers, most
//! PDF readers) fall back to a system font, which keeps Chinese/ASCII
//! mixed documents working without shipping font files. For
//! print-quality embedding, use office_automate export_pdf (needs
//! WPS/Office).

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use serde_json::{json, Value};
use std::path::Path;

// ── Layout constants (A4, points) ─────────────────────────────
const PAGE_W: f64 = 595.0;
const PAGE_H: f64 = 842.0;
const MARGIN: f64 = 50.0;
const BODY_SIZE: f64 = 11.0;
const BODY_LINE: f64 = 16.0;
const H1_SIZE: f64 = 17.0;
const H2_SIZE: f64 = 14.0;
const LIST_INDENT: f64 = 18.0;

/// A laid-out text element.
#[derive(Clone)]
pub(crate) struct Element {
    text: String,
    size: f64,
    /// Extra left indent (list items).
    indent: f64,
    /// Bullet marker for list items ("" for plain).
    marker: String,
    /// Vertical gap before this element.
    gap_before: f64,
}

/// Parse the markdown subset into layout elements.
fn parse_markdown(content: &str) -> Vec<Element> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("###") {
            if rest.starts_with(' ') {
                out.push(Element {
                    text: rest.trim().to_string(),
                    size: H2_SIZE,
                    indent: 0.0,
                    marker: String::new(),
                    gap_before: 10.0,
                });
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("##") {
            if rest.starts_with(' ') {
                out.push(Element {
                    text: rest.trim().to_string(),
                    size: H2_SIZE,
                    indent: 0.0,
                    marker: String::new(),
                    gap_before: 10.0,
                });
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            if rest.starts_with(' ') {
                out.push(Element {
                    text: rest.trim().to_string(),
                    size: H1_SIZE,
                    indent: 0.0,
                    marker: String::new(),
                    gap_before: 14.0,
                });
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            out.push(Element {
                text: rest.trim().to_string(),
                size: BODY_SIZE,
                indent: LIST_INDENT,
                marker: "• ".to_string(),
                gap_before: 2.0,
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("* ") {
            out.push(Element {
                text: rest.trim().to_string(),
                size: BODY_SIZE,
                indent: LIST_INDENT,
                marker: "• ".to_string(),
                gap_before: 2.0,
            });
            continue;
        }
        // Numbered list: "1. text".
        if let Some((num, rest)) = trimmed.split_once(". ") {
            if num.chars().all(|c| c.is_ascii_digit()) {
                out.push(Element {
                    text: rest.trim().to_string(),
                    size: BODY_SIZE,
                    indent: LIST_INDENT,
                    marker: format!("{num}. "),
                    gap_before: 2.0,
                });
                continue;
            }
        }
        out.push(Element {
            text: line.trim().to_string(),
            size: BODY_SIZE,
            indent: 0.0,
            marker: String::new(),
            gap_before: 4.0,
        });
    }
    out
}

/// Estimated width of a char in the STSong-Light font (no embedding → no
/// real metrics). Latin ≈ 0.5em, CJK ≈ 1.0em — good enough for wrapping.
fn char_width(c: char, size: f64) -> f64 {
    if c.is_ascii() {
        size * 0.5
    } else {
        size
    }
}

/// Wrap an element's text into lines that fit `max_width`.
fn wrap(text: &str, size: f64, max_width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut width = 0.0f64;
    for ch in text.chars() {
        let w = char_width(ch, size);
        if width + w > max_width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            width = 0.0;
        }
        current.push(ch);
        width += w;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Encode text as a UTF-16BE hex string for PDF content streams.
fn utf16be_hex(text: &str) -> String {
    let mut out = String::from("<");
    for unit in text.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out.push('>');
    out
}

/// Build the content stream for one page's elements (starting at `y`).
/// Returns (stream_text, next_y).
fn build_page_content(elements: &[(Element, Vec<String>)], start_y: f64) -> (String, f64) {
    let mut out = String::new();
    out.push_str("BT /F1 ");
    let mut y = start_y;
    for (el, lines) in elements {
        for (i, line) in lines.iter().enumerate() {
            let indent = el.indent;
            let text = if i == 0 && !el.marker.is_empty() {
                format!("{}{}", el.marker, line)
            } else {
                line.clone()
            };
            out.push_str(&format!("{:.1} Tf\n", el.size));
            out.push_str(&format!("1 0 0 1 {:.1} {:.1} Tm\n", MARGIN + indent, y));
            out.push_str(&format!("{}{} Tj\n", utf16be_hex(&text), ""));
            y -= BODY_LINE.max(el.size + 4.0);
        }
        y -= el.gap_before;
    }
    out.push_str("ET");
    (out, y)
}

/// Build a full PDF document from parsed elements.
pub(crate) fn build_pdf_document(elements: &[Element]) -> AppResult<Document> {
    let mut doc = Document::with_version("1.5");

    // Type0 composite font (STSong-Light, Identity-H, UTF-16BE) — not
    // embedded; viewers fall back to a system font. Built in dependency
    // order: descriptor → CID font → Type0 wrapper (no forward refs).
    let descriptor_id = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "STSong-Light",
        "Flags" => 4,
        "FontBBox" => vec![Object::Real(-35.0), Object::Real(-275.0), Object::Real(1000.0), Object::Real(880.0)],
        "ItalicAngle" => 0,
        "Ascent" => 859,
        "Descent" => -140,
        "CapHeight" => 780,
        "StemV" => 90,
    });
    let cid_font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "STSong-Light",
        "CIDSystemInfo" => dictionary! {
            "Registry" => "Adobe",
            "Ordering" => "UniGB-UCS2-H",
            "Supplement" => 0,
        },
        "FontDescriptor" => Object::Reference(descriptor_id),
        "DW" => 1000,
        "CIDToGIDMap" => "Identity",
    });
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "STSong-Light",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid_font_id)],
    });

    // Lay out pages: paginate elements into page groups.
    let content_width = PAGE_W - 2.0 * MARGIN;
    let mut pages: Vec<Vec<(Element, Vec<String>)>> = Vec::new();
    let mut current: Vec<(Element, Vec<String>)> = Vec::new();
    let mut current_y = PAGE_H - MARGIN - 24.0; // title headroom
    let max_bottom = MARGIN;

    for el in elements.iter() {
        let lines = wrap(&el.text, el.size, content_width - el.indent);
        let height = lines.len() as f64 * BODY_LINE.max(el.size + 4.0) + el.gap_before;
        if current_y - height < max_bottom && !current.is_empty() {
            pages.push(std::mem::take(&mut current));
            current_y = PAGE_H - MARGIN - 24.0;
        }
        current.push((el.clone(), lines));
        current_y -= height;
    }
    if !current.is_empty() {
        pages.push(current);
    }
    if pages.is_empty() {
        pages.push(Vec::new());
    }

    // Build page objects.
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for page_elems in &pages {
        let (stream_text, _) = build_page_content(page_elems, PAGE_H - MARGIN - 24.0);
        let content_id = doc.add_object(Stream::new(Dictionary::new(), stream_text.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(PAGE_W as f32), Object::Real(PAGE_H as f32)],
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => font_id },
            },
            "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
    }

    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => pages.len() as i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    Ok(doc)
}

/// PDF generator tool.
pub struct PdfGenerateTool;

impl PdfGenerateTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PdfGenerateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "pdf_generate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Generate a PDF document from Markdown content (pure Rust, no \
        Office install needed). Renders headings, bullet/numbered lists and \
        paragraphs onto A4 pages with automatic pagination; Chinese and \
        mixed-language text is supported (viewers fall back to a system \
        font — for print-quality embedding use office_automate export_pdf \
        with WPS/Office installed). Use for reports, one-pagers, meeting \
        minutes delivery."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path, e.g. C:\\work\\report.pdf (adds .pdf if missing)."
                },
                "content": {
                    "type": "string",
                    "description": "Markdown body: # headings, ## subheadings, - bullets, 1. numbered, plain paragraphs."
                }
            },
            "required": ["path", "content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Self-approval: creating a NEW file (or overwriting this session's
    /// own draft) skips the prompt; touching a pre-existing user file asks.
    /// Runs after the unified pipeline's deny rules — it can only lift Ask.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("path").and_then(|p| p.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("pdf"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let mut path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_str);
        if path.extension().is_none() {
            path.set_extension("pdf");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
        }

        let elements = parse_markdown(content);
        if elements.is_empty() {
            return Err("content is empty — nothing to render".into());
        }
        let mut doc = build_pdf_document(&elements)?;
        doc.save(&path)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        super::permissions::record_output(context, &path);
        Ok(ToolResult::success(format!(
            "Created PDF document: {}\n({bytes} bytes, {} layout elements)",
            path.display(),
            elements.len()
        )))
    }
}

/// Test helpers (exported for the test module).
pub(crate) fn _parse_for_test(content: &str) -> Vec<Element> {
    parse_markdown(content)
}

/// Build + save a small test PDF (shared with pdf_tools tests).
#[cfg(test)]
pub(crate) fn make_test_pdf(path: &std::path::Path, title: &str) -> AppResult<()> {
    let els = parse_markdown(&format!("# {title}\n正文内容"));
    let mut doc = build_pdf_document(&els)?;
    doc.save(path)
        .map_err(|e| crate::core::error::AppError::Other(format!("save: {e}")))?;
    Ok(())
}

pub(crate) fn _wrap_for_test(text: &str, size: f64, width: f64) -> Vec<String> {
    wrap(text, size, width)
}

pub(crate) fn _utf16be_for_test(text: &str) -> String {
    utf16be_hex(text)
}

// Keep Path import used for potential future helper.
#[allow(dead_code)]
fn _path_helper(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_parsing_kinds() {
        let els = _parse_for_test("# 标题\n正文段落\n- 列表项\n1. 编号项\n\n## 小节");
        assert_eq!(els.len(), 5);
        assert_eq!(els[0].text, "标题");
        assert_eq!(els[0].size, H1_SIZE);
        assert_eq!(els[1].text, "正文段落");
        assert_eq!(els[2].marker, "• ");
        assert_eq!(els[3].marker, "1. ");
        assert_eq!(els[4].text, "小节");
    }

    #[test]
    fn wrapping_respects_width() {
        let lines = _wrap_for_test("一二三四五六七八九十", BODY_SIZE, 5.0 * BODY_SIZE);
        assert!(lines.len() >= 2, "CJK wraps at 1em/char");
        let ascii = _wrap_for_test("hello world", BODY_SIZE, BODY_SIZE * 4.0);
        assert!(ascii.len() >= 2, "latin wraps at 0.5em/char");
    }

    #[test]
    fn utf16be_encoding() {
        assert_eq!(_utf16be_for_test("中文"), "<4E2D6587>");
        assert_eq!(_utf16be_for_test("A"), "<0041>");
    }

    #[test]
    fn builds_multi_page_pdf() {
        let content = (0..80)
            .map(|i| format!("这是第 {} 段正文内容，用于填充页面测试分页行为。", i))
            .collect::<Vec<_>>()
            .join("\n\n");
        let els = _parse_for_test(&content);
        let doc = build_pdf_document(&els).expect("build");
        // Pages tree exists with Count > 1.
        let pages = doc.get_pages();
        assert!(
            pages.len() > 1,
            "long content paginates, got {}",
            pages.len()
        );
    }

    #[test]
    fn writes_parseable_pdf_file() {
        let els = _parse_for_test("# 测试\n正文内容");
        let mut doc = build_pdf_document(&els).expect("build");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.pdf");
        doc.save(&path).expect("save");
        // lopdf can reload what it wrote.
        let reloaded = Document::load(&path).expect("reload");
        assert!(!reloaded.get_pages().is_empty());
    }

    #[test]
    fn ascii_only_pdf_is_utf16be_text() {
        let els = _parse_for_test("Hello PDF");
        let mut doc = build_pdf_document(&els).expect("build");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.pdf");
        doc.save(&path).expect("save");
        let reloaded = Document::load(&path).expect("reload");
        let pages = reloaded.get_pages();
        let (_, page_id) = pages.iter().next().expect("page");
        let content = reloaded.get_page_content(*page_id);
        let text = String::from_utf8_lossy(&content);
        assert!(text.contains("<0048"), "UTF-16BE hex for 'H'");
        assert!(!text.contains("STSong-Light")); // font name lives in the font dict, not the stream
    }
}
