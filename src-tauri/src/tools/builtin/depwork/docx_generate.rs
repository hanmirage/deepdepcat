//! docx_generate — create a Word document from a Markdown outline.
//!
//! Renders headings (# .. ######), bullet lists, numbered lists, plain
//! paragraphs, and images (`![alt](path)`) into a minimal, Word-compatible
//! `.docx` package (no external tools required).

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Create a Word document from Markdown content.
pub struct DocxGenerateTool;

impl DocxGenerateTool {
    pub fn new() -> Self {
        Self
    }
}

/// One embedded image: resolved file, extension, intrinsic size.
#[derive(Clone, Debug)]
struct ImageSpec {
    /// Relationship id inside word/_rels/document.xml.rels ("rIdImg1").
    id: String,
    path: PathBuf,
    ext: String,
    width: u32,
    height: u32,
}

/// Max rendered width in EMU (6 inches = 914400 EMU/inch).
const MAX_EMU_W: u64 = 6 * 914400;

/// Escape XML special characters for document.xml text runs.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Read intrinsic pixel size from PNG (IHDR) or JPEG (SOF) headers.
fn read_image_size(bytes: &[u8], ext: &str) -> Option<(u32, u32)> {
    match ext {
        "png" => read_png_size(bytes),
        "jpg" | "jpeg" => read_jpeg_size(bytes),
        _ => None,
    }
}

fn read_png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

fn read_jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // Standalone markers and RSTn have no payload.
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) || marker == 0xFF {
            i += 2;
            continue;
        }
        // SOFn (excluding DHT/EOI/DAC/DNL).
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if seg_len < 2 {
            return None;
        }
        i += 2 + seg_len;
    }
    None
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Render one run as a `w:r`, with rPr for bold / italic / monospace.
fn render_run(run: &super::InlineRun) -> String {
    let rpr = if run.code {
        "<w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/></w:rPr>"
    } else if run.bold && run.italic {
        "<w:rPr><w:b/><w:i/></w:rPr>"
    } else if run.bold {
        "<w:rPr><w:b/></w:rPr>"
    } else if run.italic {
        "<w:rPr><w:i/></w:rPr>"
    } else {
        ""
    };
    format!(
        "<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
        xml_escape(&run.text)
    )
}

/// Render one paragraph of Markdown to a `w:p` element.
fn render_paragraph(line: &str) -> String {
    let trimmed = line.trim_end();
    let runs: String = super::parse_inline(trimmed).iter().map(render_run).collect();
    format!(
        "<w:p><w:pPr><w:spacing w:after=\"120\"/></w:pPr>{runs}</w:p>"
    )
}

/// Render a centered inline picture paragraph (max 6 inches wide).
fn render_image_paragraph(img: &ImageSpec, index: usize) -> String {
    let (mut cx, mut cy) = (img.width as u64 * 9525, img.height as u64 * 9525);
    if cx > MAX_EMU_W {
        cy = cy * MAX_EMU_W / cx;
        cx = MAX_EMU_W;
    }
    let (cx, cy) = (cx.max(1), cy.max(1));
    format!(
        "<w:p><w:pPr><w:jc w:val=\"center\"/><w:spacing w:before=\"120\" w:after=\"120\"/></w:pPr>\
        <w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
        <wp:extent cx=\"{cx}\" cy=\"{cy}\"/>\
        <wp:docPr id=\"{index}\" name=\"Picture {index}\"/>\
        <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
        <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{index}\" name=\"Picture {index}\"/><pic:cNvPicPr/></pic:nvPicPr>\
        <pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
        <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
        <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
        </a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>",
        index = index,
        rid = img.id
    )
}

/// Render a heading line (`#`..`######`) to a `w:p` with outline level.
fn render_heading(line: &str) -> String {
    let level = line.chars().take_while(|&c| c == '#').count().min(6);
    // Word heading style names: Heading1..Heading6. Explicit pStyle makes
    // the document navigable via the navigation pane.
    let style = format!("Heading{level}");
    let runs: String = super::parse_inline(line[level..].trim())
        .iter()
        .map(render_run)
        .collect();
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/><w:spacing w:before=\"240\" w:after=\"120\"/></w:pPr>{runs}</w:p>"
    )
}

/// Detect a numbered-list line ("1. text", "10) text"). Returns the text
/// after the marker, or None for anything that is not a numbered item.
fn parse_numbered(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let rest = &line[i..];
    if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
        Some(r)
    } else {
        None
    }
}

/// Render a list item (bullet `- ` or numbered `1.` / `1)`) to a `w:p` with a
/// numPr reference. Real numbering lives in word/numbering.xml (numId 1 =
/// bullet, numId 2 = decimal) — without it Word renders list markers as plain
/// text.
fn render_list_item(line: &str) -> String {
    let (num_id, text) = if let Some(rest) = line.strip_prefix("- ") {
        ("1", rest.trim_end())
    } else if let Some(rest) = parse_numbered(line) {
        ("2", rest.trim_end())
    } else {
        return String::new();
    };
    let runs: String = super::parse_inline(text).iter().map(render_run).collect();
    format!(
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{num_id}\"/></w:numPr><w:spacing w:after=\"60\"/></w:pPr>{runs}</w:p>"
    )
}

/// Replace `![alt](path)` lines with `[[IMG:n]]` placeholders and collect the
/// resolved image specs. Missing image files are a hard error (better than a
/// silently broken document).
fn parse_images(markdown: &str, workspace: Option<&Path>) -> AppResult<(String, Vec<ImageSpec>)> {
    let mut images: Vec<ImageSpec> = Vec::new();
    let mut out = String::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("![") {
            if let Some(close) = rest.find("](") {
                if let Some(open) = rest[close + 2..].rfind(')') {
                    let rel = &rest[close + 2..close + 2 + open];
                    let path = crate::tools::builtin::resolve_path(workspace, rel);
                    if !path.is_file() {
                        return Err(format!("Image file not found: {}", path.display()).into());
                    }
                    let bytes = std::fs::read(&path)?;
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("png")
                        .to_ascii_lowercase();
                    let (width, height) = read_image_size(&bytes, &ext).unwrap_or((500, 375));
                    let idx = images.len();
                    images.push(ImageSpec {
                        id: format!("rIdImg{}", idx + 1),
                        path,
                        ext,
                        width,
                        height,
                    });
                    out.push_str(&format!("[[IMG:{idx}]]\n"));
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok((out, images))
}

/// True when a line looks like a markdown table row (`| a | b |`).
fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.ends_with('|')
}

/// Render a markdown table block (header row, separator row, data rows)
/// into a Word `w:tbl` element. The header row is bold; every cell gets a
/// thin border so the table reads as a table in Word.
fn render_table(lines: &[&str]) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in lines {
        let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
        let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
        // Separator row: `|---|---|` (dashes/colons/spaces only).
        if cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
        {
            continue;
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    // Usable A4 width in twips (page 11906 - margins 2×1440).
    // A min-width floor would push the total past the A4 text width once the
    // table has many columns (12+ × 800 > 9026 twips) — let the table stay
    // inside the page instead of overflowing the margin.
    let col_width = (9026 / cols).max(1).to_string();

    let mut out = String::from(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/>\
        <w:tblBorders>\
        <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
        <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
        <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
        <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
        <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
        <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
        </w:tblBorders></w:tblPr>",
    );
    for _ in 0..cols {
        out.push_str(&format!("<w:gridCol w:w=\"{col_width}\"/>"));
    }
    for (ri, row) in rows.iter().enumerate() {
        out.push_str("<w:tr>");
        for ci in 0..cols {
            let cell = row.get(ci).map(|s| s.as_str()).unwrap_or("");
            let bold = ri == 0 && rows.len() > 1;
            let runs: String = super::parse_inline(cell)
                .iter()
                .map(|r| {
                    let mut r2 = r.clone();
                    if bold {
                        r2.bold = true;
                    }
                    render_run(&r2)
                })
                .collect();
            out.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{col_width}\" w:type=\"dxa\"/></w:tcPr>\
                 <w:p><w:pPr><w:spacing w:after=\"20\"/></w:pPr>{runs}</w:p></w:tc>",
            ));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    out
}

/// Render Markdown content to a sequence of `w:p` / `w:tbl` elements.
fn render_body(markdown: &str, images: &[ImageSpec]) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut body = String::new();
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if is_table_row(trimmed) {
            let mut table_lines: Vec<&str> = Vec::new();
            while i < lines.len() && is_table_row(lines[i].trim_start()) {
                table_lines.push(lines[i].trim());
                i += 1;
            }
            let table = render_table(&table_lines);
            if !table.is_empty() {
                body.push_str(&table);
                body.push('\n');
            }
            continue;
        }
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("[[IMG:") {
            if let Some(end) = rest.find("]]") {
                if let Ok(idx) = rest[..end].parse::<usize>() {
                    if let Some(img) = images.get(idx) {
                        body.push_str(&render_image_paragraph(img, idx + 1));
                        i += 1;
                        continue;
                    }
                }
            }
        }
        if trimmed.starts_with('#') {
            body.push_str(&render_heading(trimmed));
        } else if trimmed.starts_with("- ") || parse_numbered(trimmed).is_some() {
            body.push_str(&render_list_item(trimmed));
        } else {
            body.push_str(&render_paragraph(lines[i]));
        }
        i += 1;
    }
    body
}

/// Build the full document.xml for a title + markdown body.
fn build_document_xml(title: &str, markdown: &str, images: &[ImageSpec]) -> String {
    let title_para = format!(
        "<w:p><w:pPr><w:pStyle w:val=\"Title\"/><w:spacing w:after=\"240\"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>",
        xml_escape(title)
    );
    // NOTE: use concat! here — a string-literal line-continuation (`\`) would
    // swallow the newline AND the leading indentation, gluing `main"xmlns:r`
    // together and producing malformed XML that Word refuses to open.
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"",
            " xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"",
            " xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"",
            " xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"",
            " xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">",
            "<w:body>{title_para}{body}",
            "<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/></w:sectPr>",
            "</w:body></w:document>"
        ),
        title_para = title_para,
        body = render_body(markdown, images)
    )
}

/// Build word/styles.xml — explicit Title/Heading1-6 definitions so the
/// document looks right even when opened outside Word's default template.
fn build_styles_xml() -> String {
    let mut headings = String::new();
    for (level, size) in [(1, 64u32), (2, 52), (3, 48), (4, 44), (5, 44), (6, 44)] {
        headings.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\"><w:name w:val=\"heading {level}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:spacing w:before=\"240\" w:after=\"120\"/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/><w:color w:val=\"2F5496\"/></w:rPr></w:style>",
            level - 1
        ));
    }
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:eastAsia=\"宋体\" w:cs=\"Calibri\"/><w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"360\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>",
            "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>",
            "<w:style w:type=\"paragraph\" w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"240\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"44\"/><w:szCs w:val=\"44\"/></w:rPr></w:style>",
            "{headings}",
            "</w:styles>"
        ),
        headings = headings
    )
}

/// Build word/numbering.xml — abstract definitions + concrete nums.
/// numId 1 = bullet (•), numId 2 = decimal ("1."). ilvl 1 exists on the
/// decimal definition so future nesting can grow without rewriting.
fn build_numbering_xml() -> String {
    concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
        "<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
        "<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>",
        "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"•\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\"/></w:rPr></w:lvl>",
        "</w:abstractNum>",
        "<w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/>",
        "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl>",
        "<w:lvl w:ilvl=\"1\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%2.\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"1440\" w:hanging=\"360\"/></w:pPr></w:lvl>",
        "</w:abstractNum>",
        "<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
        "<w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num>",
        "</w:numbering>"
    )
        .to_string()
}

/// Write a `.docx` package to disk (zip: content types, rels, styles,
/// numbering, document, embedded images).
pub fn write_docx(
    path: &Path,
    title: &str,
    markdown: &str,
    workspace: Option<&Path>,
) -> AppResult<()> {
    write_docx_with_template(path, title, markdown, workspace, None)
}

/// Write a `.docx` package; when `template` points to an existing docx, its
/// `word/styles.xml` and `word/numbering.xml` are copied so the generated
/// document inherits the template's styles (headings, fonts, spacing).
/// Falls back to the built-in styles when the template lacks a part.
pub fn write_docx_with_template(
    path: &Path,
    title: &str,
    markdown: &str,
    workspace: Option<&Path>,
    template: Option<&Path>,
) -> AppResult<()> {
    if let Some(tpl) = template {
        if !tpl.is_file() {
            return Err(crate::core::error::AppError::Internal(format!(
                "Template docx not found: {}",
                tpl.display()
            )));
        }
    }
    let styles_xml = template
        .and_then(|t| read_docx_part(t, "word/styles.xml"))
        .unwrap_or_else(build_styles_xml);
    let numbering_xml = template
        .and_then(|t| read_docx_part(t, "word/numbering.xml"))
        .unwrap_or_else(build_numbering_xml);

    let (processed, images) = parse_images(markdown, workspace)?;
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    // Content types — static parts plus one Default per image extension.
    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
        <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
        <Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>",
    );
    let mut seen_exts: Vec<&str> = Vec::new();
    for img in &images {
        if !seen_exts.contains(&img.ext.as_str()) {
            content_types.push_str(&format!(
                "<Default Extension=\"{}\" ContentType=\"{}\"/>",
                img.ext,
                mime_for(&img.ext)
            ));
            seen_exts.push(&img.ext);
        }
    }
    content_types.push_str("</Types>");

    zip.start_file("[Content_Types].xml", options)?;
    zip.write_all(content_types.as_bytes())?;

    zip.start_file("_rels/.rels", options)?;
    zip.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
        </Relationships>",
    )?;

    // Image media parts + document relationships (styles + numbering always
    // referenced, so the rels file is always written).
    for (i, img) in images.iter().enumerate() {
        let media_path = format!("word/media/image{}.{}", i + 1, img.ext);
        zip.start_file(&media_path, options)?;
        zip.write_all(&std::fs::read(&img.path)?)?;
    }
    let mut rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
        <Relationship Id=\"rIdNumbering\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
    );
    for img in &images {
        let n = &img.id[6..]; // "rIdImg1" → "1"
        rels.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image{}.{}\"/>",
            img.id, n, img.ext
        ));
    }
    rels.push_str("</Relationships>");
    zip.start_file("word/_rels/document.xml.rels", options)?;
    zip.write_all(rels.as_bytes())?;

    zip.start_file("word/styles.xml", options)?;
    zip.write_all(styles_xml.as_bytes())?;

    zip.start_file("word/numbering.xml", options)?;
    zip.write_all(numbering_xml.as_bytes())?;

    zip.start_file("word/document.xml", options)?;
    zip.write_all(build_document_xml(title, &processed, &images).as_bytes())?;

    zip.finish()?;
    Ok(())
}

/// Read one part from an existing docx package (e.g. `word/styles.xml`).
fn read_docx_part(path: &Path, part: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut member = archive.by_name(part).ok()?;
    let mut content = String::new();
    std::io::Read::read_to_string(&mut member, &mut content).ok()?;
    Some(content)
}

#[async_trait]
impl Tool for DocxGenerateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "docx_generate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Generate a Word (.docx) document from Markdown content. \
        Renders headings, bullet/numbered lists, paragraphs, and images \
        (`![alt](path)` embeds the image, max 6 inches wide) with proper \
        Word styles (Title, Heading1-6, ListBullet). Use for reports, \
        meeting minutes, and proposal drafts — the deliverable opens in Word."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path, e.g. C:\\work\\report.docx (adds .docx if missing)."
                },
                "title": {
                    "type": "string",
                    "description": "Document title (Word Title style)."
                },
                  "content": {
                      "type": "string",
                      "description": "Markdown body: # headings, - bullets, 1. numbered, plain paragraphs, ![alt](image path) images."
                  },
                  "template": {
                      "type": "string",
                      "description": "Optional path to an existing .docx whose styles are reused (headings/fonts/spacing)."
                  }
              },
            "required": ["path", "title", "content"]
        })
    }

    /// Self-approval: creating a NEW file (or overwriting this session's
    /// own draft) skips the prompt; touching a pre-existing user file asks.
    /// Runs after the unified pipeline's deny rules — it can only lift Ask.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("path").and_then(|p| p.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("docx"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let title = args
            .get("title")
            .and_then(|t| t.as_str())
            .ok_or_else(|| "Missing required parameter: title".to_string())?;
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let mut path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_str);
        if path.extension().is_none() {
            path.set_extension("docx");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create output folder {}: {e}", parent.display()))?;
        }

        // Failure diagnostics: the raw std::io::Error ("拒绝访问" on Windows)
        // carries no path, so a user cannot tell WHY the write failed. Attach
        // the target path and the usual suspects (file locked by Word/WPS,
        // read-only folder, antivirus/OneDrive lock).
        let template = args
            .get("template")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty())
            .map(|t| crate::tools::builtin::resolve_path(context.workspace.as_deref(), t));
        write_docx_with_template(
            &path,
            title,
            content,
            context.workspace.as_deref(),
            template.as_deref(),
        )
        .map_err(|e| {
            format!(
                "Failed to write {}: {e} — check the file is not open in Word/WPS, \
                   the folder is writable, and no antivirus is locking it",
                path.display()
            )
        })?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        super::permissions::record_output(context, &path);
        Ok(ToolResult::success(format!(
            "Created Word document: {}\n({bytes} bytes, Word-compatible)",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal-but-valid PNG header for size probing.
    fn minimal_png(width: u32, height: u32) -> Vec<u8> {
        let mut b = vec![0u8; 24];
        b[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        b[12..16].copy_from_slice(b"IHDR");
        b[16..20].copy_from_slice(&width.to_be_bytes());
        b[20..24].copy_from_slice(&height.to_be_bytes());
        b
    }

    #[test]
    fn heading_renders_with_style() {
        let xml = render_heading("# Chapter One");
        assert!(xml.contains("Heading1"));
        assert!(xml.contains("Chapter One"));
    }

    #[test]
    fn parse_numbered_detects_markers() {
        assert_eq!(parse_numbered("1. first"), Some("first"));
        assert_eq!(parse_numbered("10) tenth"), Some("tenth"));
        assert_eq!(parse_numbered("1.first"), None, "missing space");
        assert_eq!(parse_numbered("abc"), None);
        assert_eq!(parse_numbered("- bullet"), None);
        assert_eq!(parse_numbered(""), None);
    }

    #[test]
    fn numbered_list_renders_with_numpr() {
        let xml = render_list_item("1. first item");
        assert!(xml.contains("<w:numId w:val=\"2\"/>"), "decimal num: {xml}");
        assert!(xml.contains("first item"));
        assert!(!xml.contains("1."), "marker text must not leak: {xml}");
        // Bullet keeps numId 1.
        let bullet = render_list_item("- bullet");
        assert!(bullet.contains("<w:numId w:val=\"1\"/>"));
        // Non-list lines render nothing (safety).
        assert_eq!(render_list_item("plain text"), "");
    }

    #[test]
    fn table_renders_with_borders_and_bold_header() {
        let md = "| 名称 | 数量 |\n|---|---|\n| 苹果 | 3 |\n| 梨 | 5 |";
        let body = render_body(md, &[]);
        assert!(body.contains("<w:tbl>"), "table element");
        assert!(body.contains("<w:tr>"), "rows");
        assert!(body.contains("苹果"));
        assert!(body.contains("梨"));
        // Header row bold; separator row skipped.
        assert!(body.contains("<w:rPr><w:b/></w:rPr>"), "bold header");
        assert!(!body.contains("---"), "separator row must not render");
        // Cells are escaped.
        let md2 = "| a < b |\n|---|\n| x |";
        let body2 = render_body(md2, &[]);
        assert!(body2.contains("a &lt; b"));
    }

    #[test]
    fn table_mixed_with_text_renders_in_order() {
        let md = "前文\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n后文";
        let body = render_body(md, &[]);
        assert!(body.contains("前文"));
        assert!(body.contains("<w:tbl>"));
        assert!(body.contains("后文"));
        assert!(body.find("前文").unwrap() < body.find("<w:tbl>").unwrap());
        assert!(body.find("<w:tbl>").unwrap() < body.find("后文").unwrap());
    }

    #[test]
    fn document_xml_is_well_formed() {
        // Regression: the xmlns attributes were glued together (`main"xmlns:r`)
        // by a string-literal line continuation, producing XML that Word
        // refuses to open. The document root must have space-separated attrs.
        let xml = build_document_xml("T", "# H\n- bullet", &[]);
        assert!(
            xml.contains("wordprocessingml/2006/main\" xmlns:r="),
            "xmlns attrs must be space-separated: {xml}"
        );
        assert!(
            !xml.contains("main\"xmlns:"),
            "no glued attributes allowed: {xml}"
        );
        assert!(xml.starts_with("<?xml version=\"1.0\""));
        assert!(xml.ends_with("</w:document>"));
        // Body renders inside the document element.
        assert!(xml.contains("<w:body>"));
        assert!(xml.contains("</w:body></w:document>"));
    }

    #[test]
    fn xml_special_chars_escaped() {
        let xml = render_paragraph("a & b < c > d");
        assert!(xml.contains("a &amp; b &lt; c &gt; d"));
    }

    #[test]
    fn inline_markdown_renders_emphasis_runs() {
        let xml = render_paragraph("**bold** text `code` *italic*");
        assert!(
            xml.contains("<w:rPr><w:b/></w:rPr>"),
            "bold run carries bold rPr: {xml}"
        );
        assert!(
            xml.contains("<w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/></w:rPr>"),
            "code run carries monospace rPr: {xml}"
        );
        assert!(xml.contains("<w:rPr><w:i/></w:rPr>"), "italic run has rPr");
        // No literal markdown delimiters leak into the document.
        assert!(!xml.contains("**"), "no literal bold delimiters");
        assert!(!xml.contains('`'), "no literal code backticks");
    }

    #[test]
    fn unclosed_markdown_delimiters_stay_literal() {
        let xml = render_paragraph("unclosed **bold and a stray `tick");
        assert!(xml.contains("**bold"), "unclosed bold stays literal");
        assert!(xml.contains("`tick"), "unclosed backtick stays literal");
    }

    #[test]
    fn png_size_reads_ihdr() {
        let png = minimal_png(640, 480);
        assert_eq!(read_image_size(&png, "png"), Some((640, 480)));
        assert_eq!(read_image_size(b"not a png", "png"), None);
    }

    #[test]
    fn jpeg_size_reads_sof() {
        // SOI + APP0(len 16) + SOF0(seg len 0x11, precision 08, h 0x01E0, w 0x0280)
        let mut jpg = vec![0xFFu8, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpg.extend(vec![0u8; 16]);
        jpg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80]);
        jpg.extend(vec![0u8; 12]); // remaining SOF0 body (3 components)
        assert_eq!(read_image_size(&jpg, "jpg"), Some((640, 480)));
    }

    #[test]
    fn parse_images_collects_and_places_holders() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pic.png"), minimal_png(100, 50)).expect("write");
        let md = "# Head\n\n![chart](pic.png)\n\n- item\n";
        let (processed, images) = parse_images(md, Some(dir.path())).expect("parse");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].width, 100);
        assert!(processed.contains("[[IMG:0]]"));
        assert!(processed.contains("# Head"));
        assert!(!processed.contains("![chart]"));
    }

    #[test]
    fn parse_images_errors_on_missing_file() {
        let err = parse_images("![x](nope.png)", None).expect_err("must fail");
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn generate_and_roundtrip_docx() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report.docx");
        write_docx(
            &path,
            "Quarterly Report",
            "# Summary\n\nThis is a paragraph.\n\n- point one\n- point two\n\n1. first\n2. second\n\n## Details",
            None,
        )
        .expect("write");

        assert!(path.exists());
        // The generated package must be re-readable by our own extractor.
        let text = super::super::doc_read::extract_docx(&path).expect("roundtrip");
        assert!(text.contains("Quarterly Report"));
        assert!(text.contains("point one"));
        assert!(text.contains("first"));
        assert!(text.contains("Summary"));
    }

    #[test]
    fn package_contains_styles_and_numbering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("parts.docx");
        write_docx(&path, "T", "1. a\n2. b", None).expect("write");

        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");

        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).expect("entry").name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "word/styles.xml"), "{names:?}");
        assert!(names.iter().any(|n| n == "word/numbering.xml"), "{names:?}");

        let mut styles = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("word/styles.xml").expect("styles"),
            &mut styles,
        )
        .expect("read");
        assert!(styles.contains("styleId=\"Heading1\""));
        assert!(styles.contains("styleId=\"Title\""));
        assert!(styles.contains("eastAsia"));

        let mut numbering = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("word/numbering.xml").expect("numbering"),
            &mut numbering,
        )
        .expect("read");
        assert!(numbering.contains("<w:num w:numId=\"2\">"));
        assert!(numbering.contains("w:numFmt w:val=\"decimal\""));

        let mut doc_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("word/document.xml").expect("doc"),
            &mut doc_xml,
        )
        .expect("read");
        assert!(doc_xml.contains("<w:numId w:val=\"2\"/>"));
        assert!(doc_xml.contains("a"));
        assert!(!doc_xml.contains("1. a"), "raw marker must not appear");

        // Rels must reference both parts or Word drops them.
        let mut rels = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("word/_rels/document.xml.rels")
                .expect("rels"),
            &mut rels,
        )
        .expect("read");
        assert!(rels.contains("relationships/styles"));
        assert!(rels.contains("relationships/numbering"));

        // Content types too.
        let mut types = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("[Content_Types].xml").expect("types"),
            &mut types,
        )
        .expect("read");
        assert!(types.contains("wordprocessingml.styles+xml"));
        assert!(types.contains("wordprocessingml.numbering+xml"));
    }

    #[test]
    fn generate_docx_with_embedded_image() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("chart.png"), minimal_png(800, 400)).expect("write");
        let path = dir.path().join("with_img.docx");
        write_docx(
            &path,
            "Image Doc",
            "# Title\n\n![chart](chart.png)\n\nBody text",
            Some(dir.path()),
        )
        .expect("write");

        // Media part exists inside the package.
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut has_media = false;
        for i in 0..archive.len() {
            let entry = archive.by_index(i).expect("entry");
            if entry.name() == "word/media/image1.png" {
                has_media = true;
                break;
            }
        }
        assert!(has_media, "media/image1.png must be embedded");

        // Document XML references the image via r:embed.
        let mut doc_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("word/document.xml").expect("doc"),
            &mut doc_xml,
        )
        .expect("read");
        assert!(doc_xml.contains("rIdImg1"));
        assert!(doc_xml.contains("<wp:inline"));

        // The relationship file must map rIdImg1 → the embedded media part —
        // hand-assembled XML, the most likely place to drift.
        let mut rels_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("word/_rels/document.xml.rels")
                .expect("rels"),
            &mut rels_xml,
        )
        .expect("read");
        assert!(rels_xml.contains("rIdImg1"));
        assert!(rels_xml.contains("relationships/image"));
        assert!(rels_xml.contains("media/image1.png"));

        // Content types must declare the png default or Word refuses the file.
        let mut types_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("[Content_Types].xml").expect("types"),
            &mut types_xml,
        )
        .expect("read");
        assert!(types_xml.contains("Extension=\"png\""));
        assert!(types_xml.contains("image/png"));

        // Extractor still reads the text around the image.
        let text = super::super::doc_read::extract_docx(&path).expect("extract");
        assert!(text.contains("Body text"));
        assert!(text.contains("Title"));
    }

    #[test]
    fn template_styles_are_copied_into_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("base.docx");
        write_docx(&base, "Base", "body", None).expect("base");

        // Patch the template's styles.xml with a sentinel so the copy is
        // provable (the built-in styles are static and identical).
        {
            let file = std::fs::File::open(&base).expect("open");
            let mut archive = zip::ZipArchive::new(file).expect("zip");
            let out = std::fs::File::create(dir.path().join("patched.docx")).expect("create");
            let mut writer = zip::ZipWriter::new(out);
            let options = zip::write::SimpleFileOptions::default();
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).expect("entry");
                let name = entry.name().to_string();
                writer.start_file(&name, options).expect("start");
                if name == "word/styles.xml" {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut entry, &mut s).expect("read");
                    writer
                        .write_all(
                            s.replace("<w:styles", "<w:styles><!--TEMPLATE_SENTINEL-->")
                                .as_bytes(),
                        )
                        .expect("write");
                } else {
                    std::io::copy(&mut entry, &mut writer).expect("copy");
                }
            }
            writer.finish().expect("finish");
        }

        let patched = dir.path().join("patched.docx");
        let out_path = dir.path().join("out.docx");
        write_docx_with_template(&out_path, "T", "body", None, Some(&patched))
            .expect("write with template");
        let styles = read_docx_part(&out_path, "word/styles.xml").expect("styles part");
        assert!(
            styles.contains("TEMPLATE_SENTINEL"),
            "template styles must be copied into the output"
        );
    }
}
