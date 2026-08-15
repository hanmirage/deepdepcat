//! ppt_generate — create a PowerPoint (.pptx) deck from a Markdown outline.
//!
//! Outline format:
//! ```markdown
//! # Slide Title
//! - bullet one
//! - bullet two
//! ![图表说明](C:\charts\pie.png)
//!
//! ## Second Slide Title
//! - another bullet
//! ```
//! `#`/`##` start a new slide (title); `-` items become body bullets.
//! Lines in `![alt](path)` form become PICTURES on the slide (embedded into
//! the package, aspect ratio preserved, relative paths resolved against the
//! output file's directory).
//! Generates a minimal, PowerPoint-compatible package with one layout.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One body item on a slide: a text bullet or an embedded picture.
#[derive(Debug, Clone, PartialEq)]
pub enum SlideItem {
    Text { sub: bool, text: String },
    Image { path: PathBuf, alt: String },
}

/// Create a PowerPoint presentation from a Markdown outline.
pub struct PptGenerateTool;

impl PptGenerateTool {
    pub fn new() -> Self {
        Self
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Parse `![alt](path)` — returns (alt, raw path).
fn parse_image_line(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix("![")?;
    let close = rest.find(']')?;
    let alt = rest[..close].trim().to_string();
    let rest2 = rest[close + 1..].trim();
    let rest2 = rest2.strip_prefix('(')?.trim_start();
    let path = rest2.strip_suffix(')')?;
    Some((alt, path.trim().to_string()))
}

/// One parsed slide: title, body items, optional speaker notes and a
/// per-slide transition.
struct ParsedSlide {
    title: String,
    items: Vec<SlideItem>,
    notes: Option<String>,
    transition: Option<String>,
}

/// Parse the outline into slides.
///
/// `#`/`##`/... start a new slide (title); `- ` / `* ` items become body
/// bullets (or pictures when in `![alt](path)` form). A bullet indented by
/// 2+ spaces is a second-level item — indentation is measured on the RAW
/// line before trimming, so the hierarchy survives the parse. `> notes`
/// blockquote lines become the slide's speaker notes; `<!-- transition:x -->`
/// sets its transition.
fn parse_outline(markdown: &str) -> Vec<ParsedSlide> {
    let mut slides: Vec<ParsedSlide> = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(title) = trimmed.strip_prefix('#') {
            // Strip ALL leading `#` (##, ###, ...) — a single strip would
            // leave `## Details` as the literal title `# Details`.
            let title = title.trim_start_matches('#').trim();
            if title.is_empty() {
                continue;
            }
            slides.push(ParsedSlide {
                title: title.to_string(),
                items: Vec::new(),
                notes: None,
                transition: None,
            });
        } else if let Some(notes) = trimmed.strip_prefix('>') {
            let note = notes.trim();
            if !note.is_empty() {
                if let Some(slide) = slides.last_mut() {
                    match &mut slide.notes {
                        Some(existing) => {
                            existing.push('\n');
                            existing.push_str(note);
                        }
                        None => slide.notes = Some(note.to_string()),
                    }
                }
            }
        } else if let Some(tr) = parse_transition_comment(trimmed) {
            if let Some(slide) = slides.last_mut() {
                slide.transition = Some(tr);
            }
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            if let Some(slide) = slides.last_mut() {
                let is_sub = line.len() - line.trim_start().len() >= 2;
                slide
                    .items
                    .push(item_from_text(rest.trim().to_string(), is_sub));
            }
        } else if !slides.is_empty() {
            // Plain paragraph inside a slide — treated as a bullet (or an
            // image line when in `![alt](path)` form).
            if let Some(slide) = slides.last_mut() {
                let is_sub = line.len() - line.trim_start().len() >= 2;
                slide
                    .items
                    .push(item_from_text(trimmed.to_string(), is_sub));
            }
        }
    }
    slides
}

/// Recognize `<!-- transition:xxx -->` (xxx: fade/cut/push/wipe/random).
fn parse_transition_comment(trimmed: &str) -> Option<String> {
    let inner = trimmed
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();
    inner
        .strip_prefix("transition:")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn item_from_text(text: String, is_sub: bool) -> SlideItem {
    match parse_image_line(&text) {
        Some((alt, raw_path)) => SlideItem::Image {
            path: PathBuf::from(raw_path),
            alt,
        },
        None => SlideItem::Text { sub: is_sub, text },
    }
}

/// One embedded picture on a slide: alt text, media part name, the
/// relationship id the slide XML must reference, and the rendered size in
/// EMUs (aspect ratio preserved from the source file).
#[derive(Clone)]
struct SlideImage {
    media_name: String,
    r_id: u32,
    cx: i64,
    cy: i64,
}

/// Render one inline run as an `a:r` run (PPTX text). `sz` is the font size
/// in hundredths of a point (3600 = 36pt; 1800 = PPT's default body size).
fn render_ppt_run(run: &crate::tools::builtin::depwork::InlineRun, sz: u32) -> String {
    let rpr = if run.code {
        format!("<a:rPr lang=\"zh-CN\" sz=\"{sz}\"><a:latin typeface=\"Consolas\"/></a:rPr>")
    } else {
        let b = if run.bold { " b=\"1\"" } else { "" };
        let i = if run.italic { " i=\"1\"" } else { "" };
        format!("<a:rPr lang=\"zh-CN\" sz=\"{sz}\"{b}{i}/>")
    };
    format!("<a:r>{rpr}<a:t>{}</a:t></a:r>", xml_escape(&run.text))
}

/// Build one slide XML (title placeholder + body textbox + pictures).
///
/// `images` carries this slide's pictures in item order; their rIds start
/// at 2 (rId1 is the slide layout relationship). Pictures are siblings of
/// the body textbox inside the shape tree. An optional transition is
/// emitted between `cSld` and the closing `sld`.
fn build_slide_xml(
    slide_index: usize,
    title: &str,
    items: &[SlideItem],
    images: &[SlideImage],
    transition: Option<&str>,
) -> String {
    // Multi-line titles become `<a:br/>`-separated runs (a bare `\n` inside
    // `<a:t>` would collapse to a space in PowerPoint). Inline Markdown is
    // rendered per line (title runs are bold).
    let title_runs: Vec<String> = title
        .split('\n')
        .map(|line| {
            super::parse_inline(line)
                .iter()
                .map(|r| {
                    let mut r2 = r.clone();
                    r2.bold = true; // PPT titles are bold by convention
                    render_ppt_run(&r2, 3600)
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .collect();
    let title_xml = title_runs.join("<a:br/>");

    let mut paragraphs = String::new();
    let mut pics = String::new();
    let mut pic_id = 4u32;
    let mut image_idx = 0usize;
    for item in items.iter() {
        match item {
            SlideItem::Text { sub, text } => {
                // Second-level items indent further and drop the hanging indent.
                let (mar_l, indent, level) = if *sub {
                    ("571500", "0", " level=\"1\"")
                } else {
                    ("285750", "-285750", "")
                };
                let runs: String = super::parse_inline(text)
                    .iter()
                    .map(|r| render_ppt_run(r, 1800))
                    .collect();
                paragraphs.push_str(&format!(
                    "<a:p><a:pPr marL=\"{mar_l}\" indent=\"{indent}\"{level}/>{runs}</a:p>"
                ));
            }
            SlideItem::Image { alt, .. } => {
                let img = images.get(image_idx);
                image_idx += 1;
                let Some(img) = img else { continue };
                pics.push_str(&format!(
                    "<p:pic><p:nvPicPr><p:cNvPr id=\"{pic_id}\" name=\"Picture {pic_id}\" descr=\"{}\"/>\
                     <p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
                     <p:blipFill><a:blip r:embed=\"rId{}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
                     <p:spPr><a:xfrm><a:off x=\"457200\" y=\"2133600\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
                     <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
                    xml_escape(alt),
                    img.r_id,
                    img.cx,
                    img.cy,
                ));
                pic_id += 1;
            }
        }
    }
    let body_xml = if items.is_empty() {
        String::new()
    } else {
        format!(
            "<a:sp><a:nvSpPr><a:cNvPr id=\"3\" name=\"Content Placeholder {slide_index}\"/><a:cNvSpPr txBox=\"1\"/><a:nvPr/></a:nvSpPr>\
             <a:spPr><a:xfrm><a:off x=\"457200\" y=\"2133600\"/><a:ext cx=\"11277600\" cy=\"4267200\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></a:spPr>\
             <a:txBody><a:bodyPr rtlCol=\"0\" anchor=\"t\" wrap=\"square\"><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</a:txBody></a:sp>"
        )
    };
    let transition_xml = transition.map(transition_xml).unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title Placeholder {slide_index}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><a:spPr><a:xfrm><a:off x="457200" y="457200"/><a:ext cx="11277600" cy="1371600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></a:spPr><p:txBody><a:bodyPr rtlCol="0" anchor="t" wrap="square"><a:normAutofit/></a:bodyPr><a:lstStyle/><a:p>{title_xml}</a:p></p:txBody></p:sp>{body_xml}{pics}
</p:spTree></p:cSld>{transition_xml}</p:sld>"#
    )
}

/// The `<p:transition>` XML for a named transition (defaults to fade).
fn transition_xml(name: &str) -> String {
    let inner = match name.trim() {
        "cut" => "<p:cut/>",
        "push" => "<p:push dir=\"l\"/>",
        "wipe" => "<p:wipe dir=\"l\"/>",
        "random" => "<p:random/>",
        _ => "<p:fade/>",
    };
    format!("<p:transition spd=\"med\" advClick=\"1\">{inner}</p:transition>")
}

/// Build a speaker-notes slide (`notesSlideN.xml`) carrying the notes text.
fn build_notes_slide_xml(notes: &str) -> String {
    let paragraphs: String = notes
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let runs: String = crate::tools::builtin::depwork::parse_inline(l.trim())
                .iter()
                .map(|r| render_ppt_run(r, 1200))
                .collect();
            format!("<a:p>{runs}</a:p>")
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Notes Placeholder 2"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="685800" y="457200"/><a:ext cx="5486400" cy="6858000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr wrap="none"><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody></p:sp>
</p:spTree></p:cSld>
<p:clrMapOvr><a:overrideClrMapping bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr>
</p:notes>"#
    )
}

/// Build the presentation XML referencing all slides.
fn build_presentation_xml(slide_count: usize) -> String {
    let slides: String = (1..=slide_count)
        .map(|i| format!("<p:sldId id=\"{i}\" r:id=\"rId{i}\"/>"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdMaster"/></p:sldMasterIdLst>
<p:sldIdLst>{slides}</p:sldIdLst>
<p:sldSz cx="12192000" cy="6858000"/>
<p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#
    )
}

/// Write a `.pptx` package to disk.
pub fn write_pptx(path: &Path, title: &str, markdown: &str) -> AppResult<usize> {
    write_pptx_with_accent(path, title, markdown, None)
}

/// Like [`write_pptx`], with an optional accent color (`#RRGGBB`) injected
/// into the deck theme (accent1 + dark2) so slides inherit the brand color.
pub fn write_pptx_with_accent(
    path: &Path,
    title: &str,
    markdown: &str,
    accent: Option<&str>,
) -> AppResult<usize> {
    let mut slides = parse_outline(markdown);
    if slides.is_empty() {
        // A heading-less outline (bare `- item` list): the `title` parameter
        // becomes the first slide's title so the deck is never empty.
        let items: Vec<SlideItem> = markdown
            .lines()
            .filter_map(|l| {
                let trimmed = l.trim();
                trimmed
                    .strip_prefix("- ")
                    .or_else(|| trimmed.strip_prefix("* "))
                    .map(|rest| {
                        item_from_text(rest.trim().to_string(), l.len() - l.trim_start().len() >= 2)
                    })
            })
            .collect();
        if items.is_empty() {
            return Err("No slides found — outline must start with a # heading".into());
        }
        slides.push(ParsedSlide {
            title: title.to_string(),
            items,
            notes: None,
            transition: None,
        });
    }

    let effective = slides;

    // ── Collect pictures + prepare media parts ─────────────────────────
    // Relative image paths resolve against the output file's directory.
    let base_dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut media_names: Vec<String> = Vec::new();
    let mut slides_images: Vec<Vec<SlideImage>> = vec![Vec::new(); effective.len()];
    for (si, slide) in effective.iter().enumerate() {
        let mut r_id = 2u32;
        for item in slide.items.iter() {
            let SlideItem::Image { path, .. } = item else {
                continue;
            };
            let src = if path.is_absolute() {
                path.clone()
            } else {
                base_dir.join(path)
            };
            if !src.is_file() {
                continue;
            }
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_ascii_lowercase();
            let media_name = format!("image{}.{}", media_names.len() + 1, ext);
            let (cx, cy) = image_size_emu(&src);
            media_names.push(media_name.clone());
            slides_images[si].push(SlideImage {
                media_name,
                r_id,
                cx,
                cy,
            });
            r_id += 1;
        }
    }

    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    zip.start_file("[Content_Types].xml", options)?;
    zip.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
        <Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
        <Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
        <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>",
    )?;
    for i in 1..=effective.len() {
        zip.write_all(
            format!("<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>").as_bytes(),
        )?;
    }
    for (i, slide) in effective.iter().enumerate() {
        if slide.notes.is_some() {
            let idx = i + 1;
            zip.write_all(
                format!("<Override PartName=\"/ppt/notesSlides/notesSlide{idx}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml\"/>").as_bytes(),
            )?;
        }
    }
    let mut seen_exts: Vec<String> = Vec::new();
    for name in &media_names {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_string())
            .unwrap_or_default();
        if !seen_exts.contains(&ext) {
            seen_exts.push(ext.clone());
            zip.write_all(
                format!(
                    "<Default Extension=\"{ext}\" ContentType=\"image/{mime}\"/>",
                    mime = mime_for_ext(&ext)
                )
                .as_bytes(),
            )?;
        }
    }
    zip.write_all(b"</Types>")?;

    zip.start_file("_rels/.rels", options)?;
    zip.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>\
        </Relationships>",
    )?;

    zip.start_file("ppt/presentation.xml", options)?;
    zip.write_all(build_presentation_xml(effective.len()).as_bytes())?;

    zip.start_file("ppt/_rels/presentation.xml.rels", options)?;
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdMaster" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
{rels}</Relationships>"#,
            rels = (1..=effective.len())
                .map(|i| format!(
                    "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>"
                ))
                .collect::<String>()
        )
        .as_bytes(),
    )?;

    // ── Master → layout → slide relationship chain ─────────────────────
    // PowerPoint/WPS validate this chain on open: a slide without a layout
    // (and a layout without its master) triggers a "repair" prompt or blank
    // renders. Every slide gets its own rels pointing at the single layout,
    // which points at the master, which lists the layout by id.
    zip.start_file("ppt/slideMasters/slideMaster1.xml", options)?;
    zip.write_all(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:overrideClrMapping bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr><p:sldLayoutIdLst><p:sldLayoutId id="1" r:id="rIdLayout1"/></p:sldLayoutIdLst></p:sldMaster>"#.as_bytes(),
    )?;

    zip.start_file("ppt/slideMasters/_rels/slideMaster1.xml.rels", options)?;
    zip.write_all(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdLayout1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
<Relationship Id="rIdTheme1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#.as_bytes(),
    )?;

    // ── Theme ──────────────────────────────────────────────────────
    // PowerPoint/WPS REJECT a deck whose slide master has no theme
    // relationship (WPS COM fails with 0xFFF4001A on open) — the theme
    // carries the color/font/effect schemes the layout inherits. This
    // minimal theme (validated against a real WPS open) keeps the master
    // chain complete.
    zip.start_file("ppt/theme/theme1.xml", options)?;
    let mut theme = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="DeepDepCat">
<a:themeElements>
<a:clrScheme name="DeepDepCat">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="1F497D"/></a:dk2>
<a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4F81BD"/></a:accent1>
<a:accent2><a:srgbClr val="C0504D"/></a:accent2>
<a:accent3><a:srgbClr val="9BBB59"/></a:accent3>
<a:accent4><a:srgbClr val="8064A2"/></a:accent4>
<a:accent5><a:srgbClr val="4BACC6"/></a:accent5>
<a:accent6><a:srgbClr val="F79646"/></a:accent6>
<a:hlink><a:srgbClr val="0000FF"/></a:hlink>
<a:folHlink><a:srgbClr val="800080"/></a:folHlink>
</a:clrScheme>
<a:fontScheme name="DeepDepCat">
<a:majorFont><a:latin typeface="Arial"/><a:ea typeface="SimSun"/><a:cs typeface=""/></a:majorFont>
<a:minorFont><a:latin typeface="Arial"/><a:ea typeface="SimSun"/><a:cs typeface=""/></a:minorFont>
</a:fontScheme>
<a:fmtScheme name="DeepDepCat">
<a:fillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:fillStyleLst>
<a:lnStyleLst>
<a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln>
</a:lnStyleLst>
<a:effectStyleLst>
<a:effectStyle><a:effectLst/></a:effectStyle>
</a:effectStyleLst>
<a:bgFillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:bgFillStyleLst>
</a:fmtScheme>
</a:themeElements>
</a:theme>"#
        .to_string();
    if let Some(accent) = accent {
        let clean = accent.trim_start_matches('#').to_uppercase();
        if clean.len() == 6 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
            theme = theme.replace(
                "<a:accent1><a:srgbClr val=\"4F81BD\"/>",
                &format!("<a:accent1><a:srgbClr val=\"{clean}\"/>"),
            );
            theme = theme.replace(
                "<a:dk2><a:srgbClr val=\"1F497D\"/>",
                &format!("<a:dk2><a:srgbClr val=\"{clean}\"/>"),
            );
        }
    }
    zip.write_all(theme.as_bytes())?;

    zip.start_file("ppt/slideLayouts/slideLayout1.xml", options)?;
    zip.write_all(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank">
<p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:overrideClrMapping bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr></p:sldLayout>"#.as_bytes(),
    )?;

    zip.start_file("ppt/slideLayouts/_rels/slideLayout1.xml.rels", options)?;
    zip.write_all(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"#.as_bytes(),
    )?;

    for (i, slide) in effective.iter().enumerate() {
        let slide_index = i + 1;
        let slide_images = slides_images[i].clone();
        zip.start_file(format!("ppt/slides/slide{slide_index}.xml"), options)?;
        zip.write_all(
            build_slide_xml(
                slide_index,
                &slide.title,
                &slide.items,
                &slide_images,
                slide.transition.as_deref(),
            )
            .as_bytes(),
        )?;

        zip.start_file(
            format!("ppt/slides/_rels/slide{slide_index}.xml.rels"),
            options,
        )?;
        let image_rels: String = slide_images
            .iter()
            .map(|img| {
                format!(
                    "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{}\"/>",
                    img.r_id, img.media_name
                )
            })
            .collect();
        let notes_rel = if slide.notes.is_some() {
            format!(
                "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\" Target=\"../notesSlides/notesSlide{slide_index}.xml\"/>",
                2 + slide_images.len()
            )
        } else {
            String::new()
        };
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
{image_rels}{notes_rel}</Relationships>"#
            )
            .as_bytes(),
        )?;

        if let Some(notes) = &slide.notes {
            zip.start_file(
                format!("ppt/notesSlides/notesSlide{slide_index}.xml"),
                options,
            )?;
            zip.write_all(build_notes_slide_xml(notes).as_bytes())?;
        }
    }

    // ── Media parts: copy the picture bytes into the package ───────────
    for (name, src) in media_names
        .iter()
        .zip(media_sources(&effective, &base_dir).iter())
    {
        let bytes = std::fs::read(src)?;
        zip.start_file(format!("ppt/media/{name}"), options)?;
        zip.write_all(&bytes)?;
    }

    zip.finish()?;
    Ok(effective.len())
}

/// Source paths of every picture, in the order media names were assigned.
fn media_sources(slides: &[ParsedSlide], base_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for slide in slides {
        for item in slide.items.iter() {
            if let SlideItem::Image { path, .. } = item {
                let src = if path.is_absolute() {
                    path.clone()
                } else {
                    base_dir.join(path)
                };
                if src.is_file() {
                    out.push(src);
                }
            }
        }
    }
    out
}

/// Picture size in EMUs, preserving aspect ratio: content-area width
/// (12.4") scaled, capped by the content-area height (4.7").
fn image_size_emu(src: &Path) -> (i64, i64) {
    let full_w = 11277600i64;
    let max_h = 4267200i64;
    let Ok(reader) = image::ImageReader::open(src) else {
        return (full_w, max_h);
    };
    let Ok(reader) = reader.with_guessed_format() else {
        return (full_w, max_h);
    };
    let Ok((w, h)) = reader.into_dimensions() else {
        return (full_w, max_h);
    };
    if w == 0 || h == 0 {
        return (full_w, max_h);
    }
    let mut cx = full_w;
    let mut cy = (full_w * h as i64) / w as i64;
    if cy > max_h {
        // Preserve aspect ratio when capping the height — shrinking only cy
        // stretches the picture horizontally.
        cx = (max_h * w as i64) / h as i64;
        cy = max_h;
    }
    (cx, cy)
}

/// MIME type for a media file extension.
fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "png",
        "jpg" | "jpeg" => "jpeg",
        "gif" => "gif",
        "bmp" => "bmp",
        "tiff" | "tif" => "tiff",
        "webp" => "webp",
        "svg" => "svg+xml",
        _ => "png",
    }
}

#[async_trait]
impl Tool for PptGenerateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "ppt_generate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Generate a PowerPoint (.pptx) presentation from a Markdown outline. \
        Each # or ## heading starts a new slide (its text = slide title); \
        - items become body bullets. Lines in ![alt](path) form insert a \
        PICTURE on the slide — the image file is embedded into the pptx \
        (aspect ratio preserved; relative paths resolve against the output \
        file's directory; missing files are skipped). `> notes` lines become \
        the slide's speaker notes; `<!-- transition:fade|cut|push|wipe|random -->` \
        sets a per-slide transition. Use for meeting decks \
        and report summaries — the deliverable opens in PowerPoint."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path, e.g. C:\\work\\deck.pptx (adds .pptx if missing)."
                },
                "title": {
                    "type": "string",
                    "description": "Deck title (used when the outline has no first heading)."
                },
                  "outline": {
                      "type": "string",
                      "description": "Markdown outline: # headings start slides, - items are bullets, > notes become speaker notes, <!-- transition:x --> sets a per-slide transition."
                  },
                  "accent": {
                      "type": "string",
                      "description": "Brand accent color (#RRGGBB) injected into the deck theme (accent1 + dark2)."
                  }
              },
            "required": ["path", "outline"]
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
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("pptx"));
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
            .unwrap_or("Presentation");
        let outline = args
            .get("outline")
            .and_then(|o| o.as_str())
            .ok_or_else(|| "Missing required parameter: outline".to_string())?;

        let mut path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_str);
        if path.extension().is_none() {
            path.set_extension("pptx");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let accent = args
            .get("accent")
            .and_then(|a| a.as_str())
            .filter(|a| !a.trim().is_empty());
        let slide_count = match accent {
            Some(a) => write_pptx_with_accent(&path, title, outline, Some(a))?,
            None => write_pptx(&path, title, outline)?,
        };
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        super::permissions::record_output(context, &path);
        Ok(ToolResult::success(format!(
            "Created presentation: {} ({slide_count} slides, {bytes} bytes, PowerPoint-compatible)",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_ppt_run_emits_emphasis_and_escapes() {
        let bold = render_ppt_run(
            &crate::tools::builtin::depwork::InlineRun { text: "bold".into(), bold: true, italic: false, code: false },
            1800,
        );
        assert!(bold.contains("b=\"1\""), "bold run: {bold}");
        let code = render_ppt_run(
            &crate::tools::builtin::depwork::InlineRun { text: "fn()".into(), bold: false, italic: false, code: true },
            1800,
        );
        assert!(code.contains("Consolas"), "code run uses monospace: {code}");
        let plain = render_ppt_run(
            &crate::tools::builtin::depwork::InlineRun { text: "a & b".into(), bold: false, italic: false, code: false },
            1800,
        );
        assert!(plain.contains("a &amp; b"), "text escaped: {plain}");
    }

    #[test]
    fn outline_parses_slides_and_bullets() {
        let slides = parse_outline("# Intro\n- a\n- b\n\n## Details\n- c\n");
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].title, "Intro");
        assert_eq!(
            slides[0].items,
            vec![
                SlideItem::Text {
                    sub: false,
                    text: "a".to_string()
                },
                SlideItem::Text {
                    sub: false,
                    text: "b".to_string()
                },
            ]
        );
        // Regression: `##` headings must not keep a literal `#` prefix.
        assert_eq!(slides[1].title, "Details");
        assert_eq!(
            slides[1].items,
            vec![SlideItem::Text {
                sub: false,
                text: "c".to_string()
            }]
        );
    }

    #[test]
    fn heading_levels_all_start_slides() {
        let slides = parse_outline("# One\n### Three\n##### Five\n");
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[0].title, "One");
        assert_eq!(slides[1].title, "Three");
        assert_eq!(slides[2].title, "Five");
    }

    #[test]
    fn indented_bullets_are_second_level() {
        let slides = parse_outline("# Slide\n- top\n  - sub\n    - deeper\n");
        assert_eq!(slides.len(), 1);
        assert_eq!(
            slides[0].items,
            vec![
                SlideItem::Text {
                    sub: false,
                    text: "top".to_string()
                },
                SlideItem::Text {
                    sub: true,
                    text: "sub".to_string()
                },
                SlideItem::Text {
                    sub: true,
                    text: "deeper".to_string()
                },
            ]
        );
    }

    #[test]
    fn star_lists_parse_like_dashes() {
        let slides = parse_outline("# Slide\n* a\n  * b\n");
        assert_eq!(
            slides[0].items,
            vec![
                SlideItem::Text {
                    sub: false,
                    text: "a".to_string()
                },
                SlideItem::Text {
                    sub: true,
                    text: "b".to_string()
                },
            ]
        );
    }

    #[test]
    fn image_lines_become_picture_items() {
        let slides =
            parse_outline("# 封面\n![猫](C:\\tmp\\cat.png)\n\n# 数据\n- 说明\n![](.\\chart.png)\n");
        assert_eq!(
            slides[0].items,
            vec![SlideItem::Image {
                path: PathBuf::from("C:\\tmp\\cat.png"),
                alt: "猫".to_string(),
            }]
        );
        assert_eq!(
            slides[1].items,
            vec![
                SlideItem::Text {
                    sub: false,
                    text: "说明".to_string()
                },
                SlideItem::Image {
                    path: PathBuf::from(".\\chart.png"),
                    alt: String::new()
                },
            ]
        );
    }

    #[test]
    fn empty_outline_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.pptx");
        assert!(write_pptx(&path, "T", "no headings here").is_err());
    }

    #[test]
    fn headingless_outline_uses_title_as_first_slide() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bare.pptx");
        let count = write_pptx(&path, "Fallback Title", "- item one\n- item two\n").expect("write");
        assert_eq!(count, 1);
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut slide1 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").expect("slide1"),
            &mut slide1,
        )
        .expect("read");
        assert!(
            slide1.contains("Fallback Title"),
            "title becomes the slide title"
        );
        assert!(slide1.contains("item one"));
    }

    #[test]
    fn generate_pptx_roundtrip_via_zip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("deck.pptx");
        let count = write_pptx(
            &path,
            "Deck",
            "# Market Overview\n- growth 20%\n- profit up\n\n# Risks\n- competition\n",
        )
        .expect("write");
        assert_eq!(count, 2);

        // Package integrity: both slide parts must be present and readable.
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut slide1 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").expect("slide1"),
            &mut slide1,
        )
        .expect("read");
        assert!(slide1.contains("Market Overview"));
        let mut slide2 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide2.xml").expect("slide2"),
            &mut slide2,
        )
        .expect("read");
        assert!(slide2.contains("Risks"));
    }

    #[test]
    fn pptx_has_complete_layout_relationship_chain() {
        // PowerPoint/WPS validate master → layout → slide on open; a missing
        // link prompts a "repair" dialog. Every slide must carry its layout
        // rels, the layout must reference the master, and the master must
        // list the layout id.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chain.pptx");
        write_pptx(&path, "T", "# One\n- a\n# Two\n").expect("write");

        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");

        let mut master = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("ppt/slideMasters/slideMaster1.xml")
                .expect("master"),
            &mut master,
        )
        .expect("read");
        assert!(master.contains("sldLayoutIdLst"), "master lists its layout");

        for slide in ["slide1", "slide2"] {
            let mut rels = String::new();
            std::io::Read::read_to_string(
                &mut archive
                    .by_name(&format!("ppt/slides/_rels/{slide}.xml.rels"))
                    .expect("slide rels"),
                &mut rels,
            )
            .expect("read");
            assert!(
                rels.contains("slideLayout"),
                "{slide} must reference a layout"
            );
        }

        let mut layout_rels = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("ppt/slideLayouts/_rels/slideLayout1.xml.rels")
                .expect("layout rels"),
            &mut layout_rels,
        )
        .expect("read");
        assert!(
            layout_rels.contains("slideMaster"),
            "layout references master"
        );

        let mut master_rels = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("ppt/slideMasters/_rels/slideMaster1.xml.rels")
                .expect("master rels"),
            &mut master_rels,
        )
        .expect("read");
        assert!(
            master_rels.contains("slideLayout"),
            "master references layout"
        );
        assert!(
            master_rels.contains("relationships/theme"),
            "master MUST reference a theme — PowerPoint/WPS reject decks whose master has no theme (WPS COM: 0xFFF4001A)"
        );

        // The theme part itself must exist and be a well-formed theme root.
        let mut theme = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/theme/theme1.xml").expect("theme"),
            &mut theme,
        )
        .expect("read");
        assert!(theme.starts_with("<?xml"), "theme is xml");
        assert!(theme.contains("<a:themeElements>"), "theme has elements");
    }

    #[test]
    fn pptx_embeds_picture_with_rels_and_content_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 2x1 tiny PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x28, 0x02, 0x12, 0x19, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x66, 0x7E, 0x37,
            0xA9, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let img_path = dir.path().join("cat.png");
        std::fs::write(&img_path, png).expect("write png");

        let path = dir.path().join("deck.pptx");
        let count = write_pptx(
            &path,
            "Deck",
            "# 封面\n![测试图片](cat.png)\n\n# 第二页\n- 文字\n",
        )
        .expect("write");
        assert_eq!(count, 2);

        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");

        // Media part embedded.
        let mut media = Vec::new();
        std::io::Read::read_to_end(
            &mut archive.by_name("ppt/media/image1.png").expect("media"),
            &mut media,
        )
        .expect("read media");
        assert_eq!(media, png, "media bytes match source");

        // Slide 1 XML has a p:pic referencing rId2.
        let mut slide1 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").expect("slide1"),
            &mut slide1,
        )
        .expect("read");
        assert!(slide1.contains("<p:pic>"), "slide has a picture");
        assert!(
            slide1.contains("r:embed=\"rId2\""),
            "picture embeds the media"
        );

        // Slide 1 rels map rId2 → media.
        let mut rels = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("ppt/slides/_rels/slide1.xml.rels")
                .expect("rels"),
            &mut rels,
        )
        .expect("read");
        assert!(rels.contains("relationships/image"), "image relationship");
        assert!(rels.contains("rId2"), "rId2 declared");
        assert!(rels.contains("../media/image1.png"), "media target");

        // Content types declare the png default.
        let mut types = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("[Content_Types].xml").expect("types"),
            &mut types,
        )
        .expect("read");
        assert!(types.contains("Extension=\"png\""), "png content type");

        // Slide 2 has no picture.
        let mut slide2 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide2.xml").expect("slide2"),
            &mut slide2,
        )
        .expect("read");
        assert!(!slide2.contains("<p:pic>"), "slide 2 stays text-only");
    }

    #[test]
    fn missing_image_file_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("deck.pptx");
        let count =
            write_pptx(&path, "Deck", "# 封面\n![不存在](missing.png)\n- 文字\n").expect("write");
        assert_eq!(count, 1);
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut slide1 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").expect("slide1"),
            &mut slide1,
        )
        .expect("read");
        assert!(!slide1.contains("<p:pic>"), "missing image produces no pic");
        assert!(slide1.contains(">文字<"), "text bullet still present");
    }

    #[test]
    fn second_level_bullets_render_with_level_attribute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("levels.pptx");
        write_pptx(&path, "T", "# Slide\n- top\n  - sub\n").expect("write");
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut slide1 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").expect("slide1"),
            &mut slide1,
        )
        .expect("read");
        assert!(slide1.contains("level=\"1\""), "sub-bullet has level=1");
        assert!(slide1.contains(">top<"), "top bullet present");
        assert!(slide1.contains(">sub<"), "sub bullet present");
    }

    #[test]
    fn accent_color_reaches_theme_xml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("brand.pptx");
        write_pptx_with_accent(&path, "Deck", "# One\n- a", Some("#112233")).expect("write");
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut theme = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/theme/theme1.xml").expect("theme"),
            &mut theme,
        )
        .expect("read");
        assert!(
            theme.contains("<a:accent1><a:srgbClr val=\"112233\"/>"),
            "accent1 replaced: {theme}"
        );
        assert!(
            theme.contains("<a:dk2><a:srgbClr val=\"112233\"/>"),
            "dark2 replaced"
        );
    }

    #[test]
    fn parse_outline_parses_notes_and_transition() {
        let slides = parse_outline(
            "# 趋势\n- 数据\n> 备注：讲数据来源。\n> 强调增速。\n<!-- transition:wipe -->\n\n# 结论\n- 总结\n",
        );
        assert_eq!(slides.len(), 2);
        assert_eq!(
            slides[0].notes.as_deref(),
            Some("备注：讲数据来源。\n强调增速。")
        );
        assert_eq!(slides[0].transition.as_deref(), Some("wipe"));
        assert_eq!(slides[1].notes, None);
        assert_eq!(slides[1].transition, None);
        // notes / transition lines are not body items.
        assert_eq!(slides[0].items.len(), 1);
    }

    #[test]
    fn generate_pptx_with_notes_and_transition() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.pptx");
        write_pptx(
            &path,
            "Deck",
            "# 第一页\n- a\n> 这一页的备注。\n<!-- transition:fade -->\n\n# 第二页\n- b\n",
        )
        .expect("write");

        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");

        let mut slide1 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").expect("slide1"),
            &mut slide1,
        )
        .expect("read");
        assert!(slide1.contains("<p:transition"), "slide 1 has a transition");
        assert!(slide1.contains("<p:fade/>"), "fade transition");

        let mut slide2 = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide2.xml").expect("slide2"),
            &mut slide2,
        )
        .expect("read");
        assert!(!slide2.contains("<p:transition"), "slide 2 stays plain");

        let mut notes = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("ppt/notesSlides/notesSlide1.xml")
                .expect("notes"),
            &mut notes,
        )
        .expect("read");
        assert!(notes.contains("这一页的备注。"), "notes text present");

        let mut rels = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name("ppt/slides/_rels/slide1.xml.rels")
                .expect("rels"),
            &mut rels,
        )
        .expect("read");
        assert!(
            rels.contains("relationships/notesSlide"),
            "notes relationship"
        );
        assert!(
            rels.contains("../notesSlides/notesSlide1.xml"),
            "notes target"
        );

        let mut types = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("[Content_Types].xml").expect("types"),
            &mut types,
        )
        .expect("read");
        assert!(
            types.contains("notesSlide"),
            "notesSlide content type declared"
        );
    }
}
