//! research_paper — parse a paper PDF and save it into the 调研资料夹.
//!
//! Extracts the text layer (reusing the shared read_file_pdf pipeline),
//! heuristically parses title/authors/abstract/DOI, and stores a structured
//! entry (source="paper") so later steps can cite it with an access date —
//! the 科研域「PDF 解析入库」gap.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::tools::builtin::read_file_pdf;
use crate::toolkit::ToolScope;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tauri::Manager;

/// Structured metadata parsed from a paper's first pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaperMeta {
    pub title: String,
    pub authors: String,
    pub abstract_text: String,
    pub doi: String,
}

/// Heuristic metadata parser — good enough for machine-readable papers;
/// scanned/no-text PDFs return empty fields (the caller should hint OCR).
pub fn parse_paper_metadata(text: &str) -> PaperMeta {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();

    // Title: first line that is long enough and not a page number/header.
    let title_line = lines
        .iter()
        .find(|l| l.chars().count() >= 8 && !l.chars().all(|c| c.is_numeric()))
        .copied()
        .unwrap_or("");
    let lower_line = title_line.to_lowercase();
    // Some extractors join PDF lines with spaces — cut at the abstract
    // heading and at the first author comma so the title stays clean.
    let title_cut = ["abstract", "摘要", " by ", "作者", " authors"]
        .iter()
        .filter_map(|m| lower_line.find(m))
        .min()
        .unwrap_or(title_line.len());
    let title = {
        let head = &title_line[..title_cut];
        let comma = head.find(", ");
        let end = match comma {
            Some(ci) => {
                // Joined-line text: the first author's "First Last" pair
                // precedes the first comma — drop both words.
                let before = &head[..ci];
                match before.rfind(' ').and_then(|i| before[..i].rfind(' ')) {
                    Some(i) => i,
                    None => ci,
                }
            }
            None => head.len(),
        };
        head[..end]
            .trim()
            .chars()
            .take(200)
            .collect::<String>()
    };

    // Authors: a compact line with comma-separated name patterns near the
    // title, or explicit "By/作者/Authors" lines.
    let mut authors = String::new();
    for line in lines.iter().take(30) {
        let lower = line.to_lowercase();
        if lower.starts_with("by ") || lower.starts_with("authors") || lower.starts_with("作者") {
            authors = line
                .split_once([':', '：'])
                .map(|(_, rest)| rest.trim())
                .unwrap_or(line)
                .trim()
                .chars()
                .take(300)
                .collect();
            break;
        }
        if line.contains(',')
            && line.chars().count() < 200
            && !lower.contains("http")
            && !line.trim_start().starts_with('1')
        {
            authors = line.chars().take(300).collect();
            break;
        }
    }
    // Single-line text (PDF extractor joined lines with spaces): derive
    // authors from the slice between the title and the abstract marker.
    if authors.is_empty() {
        if let Some(ab) = lower_line.find("abstract") {
            if title_cut < ab {
                let seg = title_line[title_cut..ab].trim();
                if seg.contains(',') {
                    authors = seg.chars().take(300).collect();
                }
            }
        }
    }

    // Abstract: between an Abstract/摘要 heading and the next section.
    let abstract_text = extract_abstract(text);

    // DOI: the first DOI-looking token.
    let doi = text
        .find("10.")
        .map(|start| {
            let end = text[start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == ')' || c == ']')
                .map(|i| start + i)
                .unwrap_or(text.len());
            text[start..end]
                .chars()
                .take(120)
                .collect::<String>()
        })
        .unwrap_or_default()
        .trim()
        .to_string();

    PaperMeta {
        title,
        authors,
        abstract_text,
        doi,
    }
}

/// Capture the abstract body between a heading and the next section.
fn extract_abstract(text: &str) -> String {
    let lower = text.to_lowercase();
    let start_markers = ["abstract", "摘要"];
    let start = start_markers
        .iter()
        .filter_map(|m| lower.find(m).map(|idx| idx + m.len()))
        .min();
    let Some(start) = start else {
        return String::new();
    };
    let body = text[start..].trim_start_matches(|c: char| c.is_whitespace() || c == ':' || c == '：');
    let lower_body = body.to_lowercase();
    let end_markers = ["introduction", "keywords", "1. ", "1 ", "引言", "关键词"];
    let end = end_markers
        .iter()
        .filter_map(|m| lower_body.find(m))
        .min()
        .unwrap_or(3000.min(body.len()));
    body[..end].trim().chars().take(3000).collect()
}

/// The 调研资料夹「论文 PDF 解析入库」tool.
pub struct ResearchPaperTool;

impl ResearchPaperTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchPaperTool {
    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "research_paper"
    }

    fn description(&self) -> &str {
        "Parse a paper PDF and save it into the research folder: extracts \
         title/authors/abstract/DOI from the text layer (scanned pages need \
         OCR first) and stores a structured entry (source=paper) with the \
         abstract as snippet and an abstract preview as snapshot, so later \
         steps can cite it with an access date. Params: path (PDF file), \
         tags (optional comma-separated). Returns the parsed metadata + the \
         folder entry id."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the paper PDF"
                },
                "tags": {
                    "type": "string",
                    "description": "Optional comma-separated tags for the folder entry"
                }
            },
            "required": ["path"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Writes a folder entry — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn check_permissions(&self, _args: &Value, _context: &ToolContext) -> PermissionDecision {
        // Reads the PDF (read-only) and writes ONLY to its own research
        // folder — safe enough to self-approve like other research tools.
        PermissionDecision::Allow
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'path'".into()))?;
        let pdf = Path::new(path);
        if !pdf.is_file() {
            return Ok(ToolResult::error(format!("PDF 文件不存在: {path}")));
        }
        let text = read_file_pdf::extract_pdf_text(pdf).map_err(|e| e.to_string())?;
        if text.trim().is_empty() {
            return Ok(ToolResult::error(
                "PDF 无可提取文本（可能是扫描件），请先 OCR 再解析".to_string(),
            ));
        }
        let meta = parse_paper_metadata(&text);
        if meta.title.is_empty() {
            return Ok(ToolResult::error(
                "未能从 PDF 文本中识别标题，可能不是论文版式".to_string(),
            ));
        }

        let tags = args.get("tags").and_then(|t| t.as_str()).unwrap_or("");
        let snippet = if meta.abstract_text.is_empty() {
            text.chars().take(500).collect::<String>()
        } else {
            meta.abstract_text.chars().take(500).collect::<String>()
        };
        let snapshot = text.chars().take(3000).collect::<String>();
        let url = if meta.doi.is_empty() {
            path.to_string()
        } else {
            format!("https://doi.org/{}", meta.doi)
        };
        let source = "paper".to_string();

        let item_id = {
            let state = context.app.state::<crate::bootstrap::AppState>();
            // Idempotence: the same paper must not be inserted twice (the
            // agent may re-run the tool after a transient error). Match on
            // session + url + title.
            let existing = crate::storage::database::list_research_items(
                &state.db,
                &context.session_id,
                None,
                200,
            )
            .ok()
            .into_iter()
            .flatten()
            .find(|item| {
                item.url == url
                    && item.title.trim() == meta.title.trim()
                    && item.source == source
            })
            .map(|item| item.id);
            if let Some(id) = existing {
                return Ok(ToolResult::success(format!(
                    "论文已在资料夹（id={id}），跳过重复入库。\n标题：{}\nDOI：{}",
                    meta.title,
                    if meta.doi.is_empty() { "（未识别）" } else { &meta.doi }
                )));
            }
            crate::storage::database::insert_research_item(
                &state.db,
                &context.session_id,
                &meta.title,
                &url,
                &source,
                &snippet,
                &snapshot,
                tags,
            )
            .map_err(|e| e.to_string())?
        };

        Ok(ToolResult::success(format!(
            "已解析并入库论文 #{}：\n标题：{}\n作者：{}\nDOI：{}\n摘要：{}\n资料夹 id={}",
            item_id,
            meta.title,
            if meta.authors.is_empty() { "（未识别）" } else { &meta.authors },
            if meta.doi.is_empty() { "（未识别）" } else { &meta.doi },
            snippet,
            item_id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAPER: &str = r#"123
Attention Is All You Need
Ashish Vaswani, Noam Shazeer, Niki Parmar
Abstract
We propose a new simple network architecture, the Transformer.
Keywords: attention, transformer
1 Introduction
The dominant sequence transduction models are based on complex recurrent
or convolutional neural networks.
https://doi.org/10.48550/arXiv.1706.03762"#;

    #[test]
    fn parses_title_authors_abstract_and_doi() {
        let meta = parse_paper_metadata(PAPER);
        assert_eq!(meta.title, "Attention Is All You Need");
        assert!(meta.authors.contains("Vaswani"));
        assert!(meta.abstract_text.contains("Transformer"));
        assert!(!meta.abstract_text.contains("Introduction"));
        assert_eq!(meta.doi, "10.48550/arXiv.1706.03762");
    }

    #[test]
    fn empty_text_yields_empty_meta() {
        let meta = parse_paper_metadata("");
        assert!(meta.title.is_empty());
        assert!(meta.doi.is_empty());
    }

    #[test]
    fn chinese_paper_parses() {
        let text = "一种基于Transformer的摘要生成方法\n张三, 李四\n摘要\n本文提出一种端到端摘要生成方法。\n关键词：摘要、Transformer\n1 引言\n背景介绍。";
        let meta = parse_paper_metadata(text);
        assert!(meta.title.contains("Transformer"));
        assert!(meta.authors.contains("张三"));
        assert!(meta.abstract_text.contains("摘要生成方法"));
    }

    #[test]
    fn single_line_extraction_still_yields_clean_title_and_authors() {
        // Some PDF extractors join lines with spaces instead of newlines —
        // the heuristics must survive that (this is what the real smoke
        // produced: the whole page arrived as one long string).
        let text = "Attention Is All You Need Ashish Vaswani, Noam Shazeer, \
            Niki Parmar Abstract We propose the Transformer. \
            Keywords: attention 1 Introduction The dominant models are recurrent.";
        let meta = parse_paper_metadata(text);
        assert_eq!(meta.title, "Attention Is All You Need");
        assert!(meta.authors.contains("Vaswani"));
        assert!(meta.abstract_text.contains("Transformer"));
        assert!(!meta.abstract_text.contains("Introduction"));
    }
}
