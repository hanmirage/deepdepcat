//! docx_edit — paragraph-level editing of `.docx` files.
//!
//! The WordAgent approach edits documents inside the Word host plugin;
//! DeepDepCat has no Word host, so this tool edits the `.docx` package
//! directly (parse `word/document.xml` → locate paragraphs → modify →
//! rewrite the zip). Word-compatible output, no Office install needed.
//!
//! Actions:
//! - `list`      — enumerate paragraphs (index + preview) so the model can
//!   locate a target before editing
//! - `replace`   — replace a paragraph's text (keep its style/formatting)
//! - `insert`    — insert a new paragraph BEFORE the given index
//! - `delete`    — remove the paragraph at the given index

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::{Read, Write};

/// One paragraph: byte range in `word/document.xml` + extracted text.
#[derive(Debug, Clone)]
pub struct Paragraph {
    /// Byte offset of `<w:p` (inclusive).
    pub start: usize,
    /// Byte offset just past `</w:p>` (exclusive).
    pub end: usize,
    /// Extracted plain text of the paragraph (w:t runs concatenated).
    pub text: String,
}

/// Escape XML special characters for text runs.
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Decode the five XML entities (text extracted from `w:t` runs).
pub fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Scan `word/document.xml` for paragraphs (`w:p` elements) with their byte
/// ranges and extracted text. `w:p` does not nest, so open/close scanning
/// is safe; `w:pPr`/`w:pStyle` share the `w:p` prefix and are excluded.
pub fn scan_paragraphs(xml: &str) -> Vec<Paragraph> {
    let bytes = xml.as_bytes();
    let mut out: Vec<Paragraph> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Read the tag name.
        let tag_end = xml[i + 1..]
            .find('>')
            .map(|p| i + 1 + p)
            .unwrap_or(bytes.len());
        let tag = &xml[i + 1..tag_end];
        let lower = tag.to_ascii_lowercase();

        if super::is_paragraph_open_tag(&lower) {
            // Opening paragraph tag — find its matching close.
            let mut j = tag_end + 1;
            while j < bytes.len() {
                if bytes[j] != b'<' {
                    j += 1;
                    continue;
                }
                let close_end = xml[j + 1..]
                    .find('>')
                    .map(|p| j + 1 + p)
                    .unwrap_or(bytes.len());
                let close_tag = &xml[j + 1..close_end];
                let close_lower = close_tag.to_ascii_lowercase();
                if close_lower.starts_with("/w:p") && !close_lower.starts_with("/w:ppr") {
                    let text = extract_paragraph_text(xml, i, close_end);
                    out.push(Paragraph {
                        start: i,
                        end: close_end + 1, // past `>`
                        text,
                    });
                    i = close_end + 1;
                    break;
                }
                j = close_end + 1;
            }
            continue;
        }
        i = tag_end + 1;
    }
    out
}

/// Extract the text of one paragraph (all `w:t` runs, joined without
/// separators — the source XML already contains the runs' spacing).
fn extract_paragraph_text(xml: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    let bytes = xml.as_bytes();
    let mut i = start;
    while i < end {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let tag_end = xml[i + 1..end].find('>').map(|p| i + 1 + p).unwrap_or(end);
        let tag = &xml[i + 1..tag_end];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("w:t") && !lower.starts_with("w:tab") && !lower.starts_with("w:tc") {
            // Text content follows until </w:t> (or the end of the range).
            let content_start = tag_end + 1;
            if content_start < end {
                let close = xml[content_start..end].find("</w:t>");
                let content_end = close.map(|p| content_start + p).unwrap_or(end);
                out.push_str(&xml_unescape(&xml[content_start..content_end]));
                i = content_end;
                continue;
            }
        }
        i = tag_end + 1;
    }
    out
}

/// Replace the text of one paragraph (keep its pPr/formatting): the FIRST
/// `w:t` run gets the new text, later `w:t` runs are emptied. When the
/// paragraph has no text run at all, one is inserted before `</w:p>`.
/// Returns the paragraph's XML after the edit.
fn replace_paragraph_text(para_xml: &str, new_text: &str) -> String {
    let escaped = xml_escape(new_text);
    let bytes = para_xml.as_bytes();
    // Every w:t content range (start of content, end before `</w:t>`).
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let tag_end = para_xml[i + 1..]
            .find('>')
            .map(|p| i + 1 + p)
            .unwrap_or(bytes.len());
        let tag = &para_xml[i + 1..tag_end];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("w:t") && !lower.starts_with("w:tab") && !lower.starts_with("w:tc") {
            let content_start = tag_end + 1;
            let close = para_xml[content_start..].find("</w:t>");
            let content_end = close.map(|p| content_start + p).unwrap_or(para_xml.len());
            runs.push((content_start, content_end));
            i = content_end;
            continue;
        }
        i = tag_end + 1;
    }

    if runs.is_empty() {
        // No text run — insert one before </w:p>.
        let close_tag = para_xml.rfind("</w:p>").unwrap_or(para_xml.len());
        return format!(
            "{}{}{}{}{}",
            &para_xml[..close_tag],
            "<w:r><w:t>",
            escaped,
            "</w:t></w:r>",
            &para_xml[close_tag..]
        );
    }

    // Build once from the ORIGINAL string (byte offsets stay valid): keep
    // everything after the first run, minus the extra runs' contents.
    let first = runs[0];
    let mut tail = String::new();
    let mut gap_start = first.1;
    for &(cs, ce) in runs.iter().skip(1) {
        tail.push_str(&para_xml[gap_start..cs]);
        gap_start = ce;
    }
    tail.push_str(&para_xml[gap_start..]);
    format!("{}{}{}", &para_xml[..first.0], escaped, tail)
}

/// One table element: byte range in `word/document.xml`.
#[derive(Debug, Clone)]
pub struct Table {
    /// Byte offset of `<w:tbl` (inclusive).
    pub start: usize,
    /// Byte offset just past `</w:tbl>` (exclusive).
    pub end: usize,
    pub rows: usize,
    pub cols: usize,
}

/// Scan `word/document.xml` for tables (`w:tbl` elements). `w:tbl` does not
/// nest, so open/close scanning is safe.
pub fn scan_tables(xml: &str) -> Vec<Table> {
    let bytes = xml.as_bytes();
    let mut out: Vec<Table> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let tag_end = xml[i + 1..]
            .find('>')
            .map(|p| i + 1 + p)
            .unwrap_or(bytes.len());
        let tag = &xml[i + 1..tag_end];
        let lower = tag.to_ascii_lowercase();

        if lower == "w:tbl" {
            // Find the matching close (scan for `</w:tbl>` — no nesting).
            let close = xml[tag_end + 1..].find("</w:tbl>");
            let close_end = close.map(|p| tag_end + 1 + p).unwrap_or(bytes.len());
            let range = &xml[i..close_end];
            let rows = count_tags(range, "w:tr");
            let cols = range
                .split("w:tr")
                .skip(1)
                .map(|seg| count_tags(seg, "w:tc"))
                .max()
                .unwrap_or(0);
            out.push(Table {
                start: i,
                end: close_end + "</w:tbl>".len(),
                rows,
                cols,
            });
            i = close_end;
            continue;
        }
        i = tag_end + 1;
    }
    out
}

/// Count occurrences of an exact opening tag in a slice.
fn count_tags(xml: &str, tag: &str) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    let bytes = xml.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let tag_end = xml[i + 1..]
            .find('>')
            .map(|p| i + 1 + p)
            .unwrap_or(bytes.len());
        if xml[i + 1..tag_end].eq_ignore_ascii_case(tag) {
            count += 1;
        }
        i = tag_end + 1;
    }
    count
}

/// One physical table cell plus its grid position.
struct LocatedCell {
    start: usize,
    end: usize,
    /// First grid column occupied (0-based, gridSpan-aware).
    grid_start: usize,
    /// Number of grid columns spanned (gridSpan, default 1).
    grid_span: usize,
    /// True when this cell is the continuation of a vertical merge.
    vmerge_cont: bool,
}

/// Read a physical cell's tcPr: gridSpan value and whether it is a vertical
/// merge continuation. Returns (span, vmerge_continuation).
fn cell_grid_props(cell_xml: &str) -> (usize, bool) {
    let mut span = 1usize;
    let mut vmerge_cont = false;
    if let Some(pr_start) = cell_xml.find("<w:tcPr>") {
        let rel = &cell_xml[pr_start + 8..];
        let pr_end = rel.find("</w:tcPr>").unwrap_or(rel.len());
        let props = &rel[..pr_end];
        if let Some(pos) = props.find("<w:gridSpan") {
            if let Some(rest) = props[pos..].split("w:val=\"").nth(1) {
                if let Some(num) = rest.split('"').next() {
                    span = num.parse::<usize>().unwrap_or(1).max(1);
                }
            }
        }
        vmerge_cont = props.contains("<w:vMerge/>") || props.contains("<w:vMerge />");
    }
    (span, vmerge_cont)
}

/// The cell of the table at `table_idx`, grid row `row` and grid column `col`
/// (all 0-based). Returns the cell's XML range + text. Merged cells
/// (gridSpan/vMerge) are addressed by GRID coordinates: `col` resolves to
/// whichever physical cell covers that grid column.
fn locate_cell(
    xml: &str,
    table_idx: usize,
    row: usize,
    col: usize,
) -> Result<(usize, usize, String), String> {
    let tables = scan_tables(xml);
    let Some(t) = tables.get(table_idx) else {
        return Err(format!(
            "Table index {table_idx} out of range (document has {} tables)",
            tables.len()
        ));
    };
    let range = &xml[t.start..t.end];

    // Split into rows.
    let mut rows: Vec<(usize, usize)> = Vec::new();
    {
        let bytes = range.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let tag_end = range[i + 1..]
                .find('>')
                .map(|p| i + 1 + p)
                .unwrap_or(bytes.len());
            if range[i + 1..tag_end].eq_ignore_ascii_case("w:tr") {
                let close = range[tag_end + 1..].find("</w:tr>");
                let close_end = close.map(|p| tag_end + 1 + p).unwrap_or(bytes.len());
                rows.push((i, close_end));
                i = close_end;
                continue;
            }
            i = tag_end + 1;
        }
    }
    let Some((row_start, row_end)) = rows.get(row) else {
        return Err(format!(
            "Row {row} out of range (table {table_idx} has {} rows)",
            rows.len()
        ));
    };
    let row_range = &range[*row_start..*row_end];

    // Split the row into cells, tracking grid columns across gridSpans.
    let mut cells: Vec<LocatedCell> = Vec::new();
    let mut grid_col = 0usize;
    {
        let bytes = row_range.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let tag_end = row_range[i + 1..]
                .find('>')
                .map(|p| i + 1 + p)
                .unwrap_or(bytes.len());
            if row_range[i + 1..tag_end].eq_ignore_ascii_case("w:tc") {
                let close = row_range[tag_end + 1..].find("</w:tc>");
                let close_end = close.map(|p| tag_end + 1 + p).unwrap_or(bytes.len());
                let cell_xml = &row_range[i..close_end];
                let (grid_span, vmerge_cont) = cell_grid_props(cell_xml);
                cells.push(LocatedCell {
                    start: i,
                    end: close_end,
                    grid_start: grid_col,
                    grid_span,
                    vmerge_cont,
                });
                grid_col += grid_span;
                i = close_end;
                continue;
            }
            i = tag_end + 1;
        }
    }
    let Some(cell) = cells
        .iter()
        .find(|c| c.grid_start <= col && col < c.grid_start + c.grid_span)
    else {
        let max_grid = cells
            .last()
            .map(|c| c.grid_start + c.grid_span)
            .unwrap_or(0);
        if col >= max_grid {
            return Err(format!(
                "Column {col} out of range (row {row} has {max_grid} grid columns)"
            ));
        }
        return Err(format!(
            "Column {col} falls inside a merged cell (row {row}) — use the cell's first grid column"
        ));
    };
    if cell.vmerge_cont {
        return Err(format!(
            "Cell ({row}, {col}) is a vertical-merge continuation — edit the merged cell's first row instead"
        ));
    }

    let cell_xml = &row_range[cell.start..cell.end];
    let text = extract_paragraph_text(cell_xml, 0, cell_xml.len());
    let abs_start = t.start + *row_start + cell.start;
    let abs_end = t.start + *row_start + cell.end;
    Ok((abs_start, abs_end, text))
}

/// Apply a table edit to the document XML.
pub fn apply_table_edit(
    xml: &str,
    action: &str,
    table: usize,
    row: usize,
    col: usize,
    new_text: &str,
) -> Result<String, String> {
    match action {
        "set_cell" => {
            let (cell_start, cell_end, _) = locate_cell(xml, table, row, col)?;
            let cell_xml = &xml[cell_start..cell_end];
            let edited = replace_paragraph_text(cell_xml, new_text);
            Ok(format!(
                "{}{}{}",
                &xml[..cell_start],
                edited,
                &xml[cell_end..]
            ))
        }
        other => Err(format!("Unknown table action: {other} (set_cell)")),
    }
}

/// Format a table listing for the model.
pub fn format_table_list(xml: &str) -> String {
    let tables = scan_tables(xml);
    let mut out = String::new();
    for (ti, t) in tables.iter().enumerate() {
        out.push_str(&format!("[table {ti}] {r}×{c}\n", r = t.rows, c = t.cols));
        // Preview up to 3 rows × 3 cells.
        for r in 0..t.rows.min(3) {
            let mut cells = Vec::new();
            for c in 0..t.cols.min(3) {
                match locate_cell(xml, ti, r, c) {
                    Ok((_, _, text)) => {
                        let preview: String = text.chars().take(24).collect();
                        cells.push(preview);
                    }
                    Err(_) => cells.push("?".to_string()),
                }
            }
            out.push_str(&format!("  row {r}: {}\n", cells.join(" | ")));
        }
    }
    out
}

/// Apply a paragraph edit to the document XML.
///
/// `para` is 0-based. Actions:
/// - replace: `new_text` (old_text ignored — whole-paragraph replace)
/// - insert: `new_text` inserted as a new paragraph BEFORE `para`
/// - delete: paragraph removed
pub fn apply_edit(xml: &str, action: &str, para: usize, new_text: &str) -> Result<String, String> {
    let paragraphs = scan_paragraphs(xml);
    let Some(p) = paragraphs.get(para) else {
        return Err(format!(
            "Paragraph index {para} out of range (document has {} paragraphs)",
            paragraphs.len()
        ));
    };

    match action {
        "list" => Ok(xml.to_string()),
        "replace" => {
            let edited = replace_paragraph_text(&xml[p.start..p.end], new_text);
            Ok(format!("{}{}{}", &xml[..p.start], edited, &xml[p.end..]))
        }
        "insert" => {
            let new_para = format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", xml_escape(new_text));
            Ok(format!(
                "{}{}{}",
                &xml[..p.start],
                new_para,
                &xml[p.start..]
            ))
        }
        "delete" => Ok(format!("{}{}", &xml[..p.start], &xml[p.end..])),
        other => Err(format!(
            "Unknown action: {other} (list|replace|insert|delete)"
        )),
    }
}

/// Byte offset of the first text run (`<w:r ...>`/`<w:r>`), skipping
/// property tags (`w:rPr`, `w:rStyle`, `w:rFonts`). Returns the XML length
/// when the paragraph has no run.
fn first_run_start(xml: &str) -> usize {
    let lower = xml.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0usize;
    while i + 3 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'w' && bytes[i + 2] == b':' && bytes[i + 3] == b'r'
        {
            let after = &lower[i + 4..];
            let next = after.chars().next().unwrap_or('>');
            // Run tag: `>` or whitespace followed by a lowercase attr name
            // (`rsidR`). Property tags (`rPr`/`rStyle`/`rFonts`) start with
            // an uppercase letter and are skipped.
            if next == '>'
                || (next.is_whitespace()
                    && after.chars().nth(1).is_some_and(|c| c.is_ascii_lowercase()))
            {
                return i;
            }
        }
        i += 1;
    }
    xml.len()
}

/// Extract the first run's `<w:rPr>…</w:rPr>` (formatting) so tracked
/// insertions/deletions inherit the paragraph's run style.
fn first_run_rpr(xml: &str) -> String {
    let start = first_run_start(xml);
    if start >= xml.len() {
        return String::new();
    }
    let lower = xml[start..].to_ascii_lowercase();
    let Some(open) = lower.find("<w:rpr>") else {
        return String::new();
    };
    let after_open = open + "<w:rpr>".len();
    let Some(close) = lower[after_open..].find("</w:rpr>") else {
        return String::new();
    };
    xml[start + open..start + after_open + close + "</w:rpr>".len()].to_string()
}

/// Replace one paragraph's text as a tracked change: the old text becomes
/// a `<w:del>` (deleted) run, the new text an `<w:ins>` (inserted) run.
/// Word/WPS render both with revision marks instead of silently replacing.
pub fn replace_paragraph_revision(para_xml: &str, new_text: &str) -> String {
    let old_text = extract_paragraph_text(para_xml, 0, para_xml.len());
    let head_end = first_run_start(para_xml);
    let head = &para_xml[..head_end];
    let rpr = first_run_rpr(para_xml);
    let id1 = crate::core::ids::generate_id();
    let id2 = crate::core::ids::generate_id();
    let date = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let author = "DeepDepCat";
    let deleted = if old_text.trim().is_empty() {
        String::new()
    } else {
        format!(
            "<w:del w:id=\"{id1}\" w:author=\"{author}\" w:date=\"{date}\">\
             <w:r>{rpr}<w:delText xml:space=\"preserve\">{}</w:delText></w:r></w:del>",
            xml_escape(&old_text)
        )
    };
    let inserted = format!(
        "<w:ins w:id=\"{id2}\" w:author=\"{author}\" w:date=\"{date}\">\
         <w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:ins>",
        xml_escape(new_text)
    );
    format!("{head}{deleted}{inserted}</w:p>")
}

/// Apply a paragraph edit with Word revision marks (`track_changes`).
/// `list`/`list_tables`/`set_cell` fall back to the plain path.
pub fn apply_edit_tracked(
    xml: &str,
    action: &str,
    para: usize,
    new_text: &str,
) -> Result<String, String> {
    let paragraphs = scan_paragraphs(xml);
    let Some(p) = paragraphs.get(para) else {
        return Err(format!(
            "Paragraph index {para} out of range (0..{})",
            paragraphs.len()
        ));
    };
    let id = crate::core::ids::generate_id();
    let date = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    match action {
        "replace" => {
            let edited = replace_paragraph_revision(&xml[p.start..p.end], new_text);
            Ok(format!("{}{}{}", &xml[..p.start], edited, &xml[p.end..]))
        }
        "insert" => {
            let new_para = format!(
                "<w:p><w:ins w:id=\"{id}\" w:author=\"DeepDepCat\" w:date=\"{date}\">\
                 <w:r><w:t>{}</w:t></w:r></w:ins></w:p>",
                xml_escape(new_text)
            );
            Ok(format!(
                "{}{}{}",
                &xml[..p.start],
                new_para,
                &xml[p.start..]
            ))
        }
        "delete" => {
            let para_xml = &xml[p.start..p.end];
            let head_end = first_run_start(para_xml);
            let runs = para_xml[head_end..]
                .strip_suffix("</w:p>")
                .unwrap_or(&para_xml[head_end..]);
            let deleted_runs = runs
                .replace("<w:t", "<w:delText")
                .replace("</w:t>", "</w:delText>");
            let new_para = format!(
                "{}<w:del w:id=\"{id}\" w:author=\"DeepDepCat\" w:date=\"{date}\">\
                 {deleted_runs}</w:del></w:p>",
                &para_xml[..head_end]
            );
            Ok(format!("{}{}{}", &xml[..p.start], new_para, &xml[p.end..]))
        }
        other => apply_edit(xml, other, para, new_text),
    }
}

/// Read `word/document.xml` from a `.docx` package.
pub fn read_document_xml(path: &std::path::Path) -> Result<String, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid docx package: {e}"))?;
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|e| format!("docx has no word/document.xml: {e}"))?;
    let mut xml = String::new();
    document
        .read_to_string(&mut xml)
        .map_err(|e| format!("Failed to read docx XML: {e}"))?;
    Ok(xml)
}

/// Rewrite a `.docx` package with an edited `word/document.xml`.
pub fn write_document_xml(path: &std::path::Path, new_xml: &str) -> Result<(), String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid docx package: {e}"))?;

    // Write to a temp file first, then replace (never corrupt the original
    // if the write fails midway).
    let dir = std::env::temp_dir();
    let tmp_path = dir.join(format!(
        "docx_edit_{}.docx",
        crate::core::ids::generate_id()
    ));
    let out = std::fs::File::create(&tmp_path).map_err(|e| format!("temp file: {e}"))?;
    let mut writer = zip::ZipWriter::new(out);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let names: Vec<String> = (0..archive.len())
        .map(|i| {
            archive
                .by_index(i)
                .map(|f| f.name().to_string())
                .unwrap_or_default()
        })
        .collect();

    for name in names {
        let is_document = name == "word/document.xml";
        if is_document {
            writer
                .start_file(&name, options)
                .map_err(|e| e.to_string())?;
            writer
                .write_all(new_xml.as_bytes())
                .map_err(|e| e.to_string())?;
        } else {
            let mut entry = archive
                .by_name(&name)
                .map_err(|e| format!("read {}: {e}", name))?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            writer
                .start_file(&name, options)
                .map_err(|e| e.to_string())?;
            writer.write_all(&buf).map_err(|e| e.to_string())?;
        }
    }
    writer.finish().map_err(|e| e.to_string())?;
    drop(archive);

    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!(
            "Cannot replace {}: {e} — is it open in Word/WPS?",
            path.display()
        )
    })
}

/// Format a paragraph listing for the model (index + preview).
pub fn format_paragraph_list(paragraphs: &[Paragraph]) -> String {
    let mut out = String::new();
    for (i, p) in paragraphs.iter().enumerate() {
        let preview: String = p.text.chars().take(80).collect();
        let preview = if p.text.chars().count() > 80 {
            format!("{preview}…")
        } else {
            preview
        };
        out.push_str(&format!("[{i}] {preview}\n"));
    }
    out
}

/// Document edit tool.
pub struct DocxEditTool;

impl DocxEditTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DocxEditTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "docx_edit"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Edit a Word (.docx) file at paragraph or TABLE granularity (no Word \
        install needed — edits the file directly). Paragraph actions: list \
        (enumerate paragraphs with indexes), replace (rewrite one \
        paragraph's text, keeping its style), insert (add a paragraph \
        before the given index), delete (remove a paragraph). Table actions: \
          list_tables (enumerate tables with dimensions + cell previews), \
          set_cell (set one cell's text: table/row/col, all 0-based). Use \
          list / list_tables first to find indexes. Set track_changes=true \
          to mark replace/insert/delete as Word revisions (w:ins/w:del) \
          instead of silently overwriting."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the .docx file."
                },
                "action": {
                    "type": "string",
                    "enum": ["list", "replace", "insert", "delete", "list_tables", "set_cell"],
                    "description": "Paragraph actions: list/replace/insert/delete. Table actions: list_tables/set_cell."
                },
                "para": {
                    "type": "integer",
                    "description": "0-based paragraph index (from the list action)."
                },
                "text": {
                    "type": "string",
                    "description": "New paragraph / cell text (replace/insert/set_cell)."
                },
                "track_changes": {
                    "type": "boolean",
                    "description": "Mark the change as a Word revision (w:ins/w:del) instead of silently overwriting. Works with replace/insert/delete paragraph actions."
                },
                "table": {
                    "type": "integer",
                    "description": "0-based table index (set_cell, from list_tables)."
                },
                "row": {
                    "type": "integer",
                    "description": "0-based row within the table (set_cell)."
                },
                "col": {
                    "type": "integer",
                    "description": "0-based column within the table (set_cell)."
                }
            },
            "required": ["path", "action"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Self-approval: editing the agent's OWN session output (its draft)
    /// skips the prompt; editing a pre-existing user file asks. Runs after
    /// the unified pipeline's deny rules — it can only lift Ask.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("path").and_then(|p| p.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target = super::permissions::resolve_target(context.workspace.as_deref(), raw, None);
        super::permissions::write_target_decision(context, &target)
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
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?
            .to_ascii_lowercase();
        let para = args
            .get("para")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let track_changes = args
            .get("track_changes")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let xml = read_document_xml(&path)?;
        let paragraphs = scan_paragraphs(&xml);

        if action == "list" {
            return Ok(ToolResult::success(format!(
                "--- Document: {}\n({} paragraphs)\n\n{}",
                path.display(),
                paragraphs.len(),
                format_paragraph_list(&paragraphs)
            )));
        }

        if action == "list_tables" {
            let tables = format_table_list(&xml);
            return Ok(ToolResult::success(format!(
                "--- Document: {}\n{}\n\nUse set_cell (table/row/col, 0-based) to edit cells.",
                path.display(),
                if tables.is_empty() {
                    "(no tables)".to_string()
                } else {
                    tables
                }
            )));
        }

        if action == "set_cell" {
            let table = args
                .get("table")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: table (0-based)".to_string())?
                as usize;
            let row = args
                .get("row")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: row (0-based)".to_string())?
                as usize;
            let col = args
                .get("col")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: col (0-based)".to_string())?
                as usize;
            let new_xml = apply_table_edit(&xml, "set_cell", table, row, col, text)
                .map_err(|e| format!("Edit failed: {e}"))?;
            write_document_xml(&path, &new_xml).map_err(|e| format!("Write failed: {e}"))?;
            return Ok(ToolResult::success(format!(
                "--- Document: {}\nSet cell ({table}, {row}, {col}) — saved.\n\n{}",
                path.display(),
                format_table_list(&new_xml)
            )));
        }

        let Some(para) = para else {
            return Err("Missing required parameter: para (paragraph index)".into());
        };
        let new_xml = if track_changes && matches!(action.as_str(), "replace" | "insert" | "delete")
        {
            apply_edit_tracked(&xml, &action, para, text)
                .map_err(|e| format!("Edit failed: {e}"))?
        } else {
            apply_edit(&xml, &action, para, text).map_err(|e| format!("Edit failed: {e}"))?
        };
        write_document_xml(&path, &new_xml).map_err(|e| format!("Write failed: {e}"))?;

        // Report the result with the updated listing.
        let after = scan_paragraphs(&new_xml);
        let count_before = paragraphs.len();
        let count_after = after.len();
        Ok(ToolResult::success(format!(
            "--- Document: {}\nAction: {action} @ paragraph {para}{}\nParagraphs: {count_before} → {count_after}\n\n{}",
            path.display(),
            if action == "replace" {
                format!(" (text set to {})", text.chars().take(60).collect::<String>())
            } else {
                String::new()
            },
            format_paragraph_list(&after)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = concat!(
        "<?xml version=\"1.0\"?><w:document><w:body>",
        "<w:p><w:pPr><w:pStyle w:val=\"Title\"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>First body paragraph</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>Second</w:t></w:r><w:r><w:t xml:space=\"preserve\"> part</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>  </w:t></w:r></w:p>",
        "</w:body></w:document>"
    );

    #[test]
    fn scan_finds_paragraphs_with_text() {
        let ps = scan_paragraphs(SAMPLE_XML);
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0].text, "Quarterly Report");
        assert_eq!(ps[1].text, "First body paragraph");
        // Multi-run paragraphs concatenate.
        assert_eq!(ps[2].text, "Second part");
        assert_eq!(ps[3].text, "  ");
    }

    #[test]
    fn scan_finds_attributed_paragraphs() {
        // Real Word docs attach w14:paraId / w14:textId (and w:rsidR) to
        // <w:p> — an exact `== "w:p"` match silently drops these paragraphs.
        let xml = concat!(
            "<?xml version=\"1.0\"?><w:document><w:body>",
            "<w:p w14:paraId=\"4B7A2E11\" w14:textId=\"77777777\" w:rsidR=\"00C95278\"><w:r><w:t>Attributed paragraph</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>Plain paragraph</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let ps = scan_paragraphs(xml);
        assert_eq!(ps.len(), 2, "attributed + plain paragraphs");
        assert_eq!(ps[0].text, "Attributed paragraph");
        assert_eq!(ps[1].text, "Plain paragraph");
    }

    #[test]
    fn replace_keeps_style_and_sets_text() {
        let _ps = scan_paragraphs(SAMPLE_XML);
        let edited = apply_edit(SAMPLE_XML, "replace", 1, "Rewritten body").expect("replace");
        let after = scan_paragraphs(&edited);
        assert_eq!(after[1].text, "Rewritten body");
        // Style survived (Title pPr untouched on paragraph 0).
        assert!(edited.contains("w:pStyle"));
        assert!(edited.contains("Quarterly Report"));
    }

    #[test]
    fn tracked_replace_marks_del_and_ins() {
        let edited = apply_edit_tracked(SAMPLE_XML, "replace", 1, "Rewritten body")
            .expect("tracked replace");
        assert!(edited.contains("<w:del "), "old text must be a deletion");
        assert!(edited.contains("<w:ins "), "new text must be an insertion");
        assert!(edited.contains("<w:delText"), "deleted runs use delText");
        assert!(edited.contains("Rewritten body"));
        // Paragraph structure survives scanning (4 paragraphs stay).
        assert_eq!(scan_paragraphs(&edited).len(), 4);
    }

    #[test]
    fn tracked_insert_wraps_new_paragraph() {
        let edited =
            apply_edit_tracked(SAMPLE_XML, "insert", 1, "Inserted para").expect("tracked insert");
        assert!(edited.contains("<w:ins "));
        assert!(edited.contains("Inserted para"));
        assert_eq!(scan_paragraphs(&edited).len(), 5);
    }

    #[test]
    fn tracked_delete_preserves_text_as_deltext() {
        let before = scan_paragraphs(SAMPLE_XML);
        let edited = apply_edit_tracked(SAMPLE_XML, "delete", 1, "").expect("tracked delete");
        assert!(edited.contains("<w:del "));
        assert!(
            edited.contains("<w:delText"),
            "deleted paragraph text kept as delText"
        );
        assert!(edited.contains(&before[1].text));
        // The paragraph shell remains so Word can show the revision.
        assert_eq!(scan_paragraphs(&edited).len(), 4);
    }

    #[test]
    fn replace_escapes_xml() {
        let _ps = scan_paragraphs(SAMPLE_XML);
        let edited = apply_edit(SAMPLE_XML, "replace", 2, "a < b & c \"d\" 'e'").expect("replace");
        let after = scan_paragraphs(&edited);
        assert_eq!(after[2].text, "a < b & c \"d\" 'e'");
        assert!(!edited[..after[2].start].contains("<w:t>a < b"));
    }

    #[test]
    fn insert_adds_paragraph_before_index() {
        let edited = apply_edit(SAMPLE_XML, "insert", 1, "Inserted before second").expect("insert");
        let after = scan_paragraphs(&edited);
        assert_eq!(after.len(), 5);
        assert_eq!(after[1].text, "Inserted before second");
        assert_eq!(after[2].text, "First body paragraph");
    }

    #[test]
    fn delete_removes_paragraph() {
        let edited = apply_edit(SAMPLE_XML, "delete", 0, "").expect("delete");
        let after = scan_paragraphs(&edited);
        assert_eq!(after.len(), 3);
        assert_eq!(after[0].text, "First body paragraph");
    }

    #[test]
    fn out_of_range_is_error() {
        assert!(apply_edit(SAMPLE_XML, "delete", 99, "").is_err());
        assert!(apply_edit(SAMPLE_XML, "unknown", 0, "").is_err());
    }

    const TABLE_XML: &str = concat!(
        "<w:document><w:body>",
        "<w:p><w:r><w:t>Intro</w:t></w:r></w:p>",
        "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr>",
        "<w:tr><w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
        "</w:body></w:document>"
    );

    #[test]
    fn scan_tables_finds_dimensions() {
        let tables = scan_tables(TABLE_XML);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows, 2);
        assert_eq!(tables[0].cols, 2);
    }

    #[test]
    fn locate_cell_reads_text() {
        let (_, _, text) = locate_cell(TABLE_XML, 0, 1, 1).expect("cell");
        assert_eq!(text, "B2");
    }

    #[test]
    fn set_cell_edits_in_place() {
        let edited = apply_table_edit(TABLE_XML, "set_cell", 0, 0, 1, "B1-updated").expect("edit");
        let (_, _, text) = locate_cell(&edited, 0, 0, 1).expect("cell");
        assert_eq!(text, "B1-updated");
        // The other cell is untouched.
        let (_, _, a1) = locate_cell(&edited, 0, 0, 0).expect("cell");
        assert_eq!(a1, "A1");
        // Paragraphs outside the table are untouched.
        let ps = scan_paragraphs(&edited);
        assert_eq!(ps[0].text, "Intro");
    }

    #[test]
    fn table_out_of_range_is_error() {
        assert!(apply_table_edit(TABLE_XML, "set_cell", 5, 0, 0, "x").is_err());
        assert!(apply_table_edit(TABLE_XML, "set_cell", 0, 5, 0, "x").is_err());
        assert!(apply_table_edit(TABLE_XML, "set_cell", 0, 0, 5, "x").is_err());
    }

    const MERGED_TABLE_XML: &str = concat!(
        "<w:document><w:body>",
        "<w:tbl>",
        "<w:tr><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr>",
        "<w:p><w:r><w:t>MERGED</w:t></w:r></w:p></w:tc></w:tr>",
        "<w:tr><w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc></w:tr>",
        "</w:tbl>",
        "</w:body></w:document>"
    );

    #[test]
    fn set_cell_hits_gridspan_cell_by_grid_column() {
        // Grid column 1 lies inside the merged cell covering grid columns 0-1.
        let edited = apply_table_edit(MERGED_TABLE_XML, "set_cell", 0, 0, 1, "MERGED-updated")
            .expect("edit");
        let (_, _, text) = locate_cell(&edited, 0, 0, 1).expect("cell");
        assert_eq!(text, "MERGED-updated");
        // gridSpan property survives the rewrite.
        assert!(edited.contains("<w:gridSpan w:val=\"2\"/>"));
    }

    #[test]
    fn set_cell_grid_column_out_of_range_errors() {
        let err = apply_table_edit(MERGED_TABLE_XML, "set_cell", 0, 0, 2, "x").unwrap_err();
        assert!(err.contains("out of range"), "{err}");
    }

    const VMERGE_XML: &str = concat!(
        "<w:document><w:body><w:tbl>",
        "<w:tr><w:tc><w:tcPr><w:vMerge w:val=\"restart\"/></w:tcPr>",
        "<w:p><w:r><w:t>TOP</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>R1C2</w:t></w:r></w:p></w:tc></w:tr>",
        "<w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr>",
        "<w:p><w:r><w:t>cont</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>R2C2</w:t></w:r></w:p></w:tc></w:tr>",
        "</w:tbl></w:body></w:document>"
    );

    #[test]
    fn vmerge_continuation_errors_with_hint() {
        let err = apply_table_edit(VMERGE_XML, "set_cell", 0, 1, 0, "x").unwrap_err();
        assert!(err.contains("vertical-merge continuation"), "{err}");
        // The restart cell in row 0 still edits normally.
        let edited = apply_table_edit(VMERGE_XML, "set_cell", 0, 0, 0, "TOP-2").expect("edit");
        let (_, _, text) = locate_cell(&edited, 0, 0, 0).expect("cell");
        assert_eq!(text, "TOP-2");
    }
}
