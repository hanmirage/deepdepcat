//! docx_search — search a `.docx` document at paragraph granularity.
//!
//! The WordAgent counterpart of `search_document`: instead of reading the
//! whole document, this tool returns the paragraphs that match the query
//! (with their indexes), so the model can locate content cheaply and then
//! read/edit exactly those paragraphs with docx_edit.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Read;

use super::docx_edit::{scan_paragraphs, xml_escape};

/// Default cap on returned hits (paragraphs).
const MAX_RESULTS_DEFAULT: usize = 20;

/// Find paragraphs matching ALL query terms (case-insensitive).
///
/// Returns `(index, text)` pairs for every matching paragraph.
pub fn search_paragraphs(xml: &str, terms: &[String], max_results: usize) -> Vec<(usize, String)> {
    let paragraphs = scan_paragraphs(xml);
    let lower_terms: Vec<String> = terms
        .iter()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if lower_terms.is_empty() {
        return Vec::new();
    }

    let mut hits = Vec::new();
    for (i, p) in paragraphs.iter().enumerate() {
        let lower = p.text.to_lowercase();
        if lower_terms.iter().all(|t| lower.contains(t)) {
            hits.push((i, p.text.clone()));
            if hits.len() >= max_results {
                break;
            }
        }
    }
    hits
}

/// Document search tool.
pub struct DocxSearchTool;

impl DocxSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DocxSearchTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "docx_search"
    }

    fn description(&self) -> &str {
        "Search a Word (.docx) document for paragraphs matching keywords. \
        Returns matching paragraph indexes and text (case-insensitive; \
        multiple space-separated terms must ALL match). Use it to locate \
        content cheaply before reading or editing with doc_read / docx_edit."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the .docx file."
                },
                "query": {
                    "type": "string",
                    "description": "Space-separated keywords; a paragraph must contain ALL of them."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum hits to return (default 20)."
                }
            },
            "required": ["path", "query"]
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
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| "Missing required parameter: query".to_string())?
            .to_string();
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(MAX_RESULTS_DEFAULT)
            .max(1);

        let terms: Vec<String> = query.split_whitespace().map(str::to_string).collect();
        if terms.is_empty() {
            return Err("query must contain at least one keyword".into());
        }

        // .docx: parse the package; .txt/.md: plain text fallback.
        let (xml, total_paragraphs) = {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "docx" {
                let file = std::fs::File::open(&path)
                    .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
                let mut archive = zip::ZipArchive::new(file)
                    .map_err(|e| format!("Not a valid docx package: {e}"))?;
                let mut document = archive
                    .by_name("word/document.xml")
                    .map_err(|e| format!("docx has no word/document.xml: {e}"))?;
                let mut xml = String::new();
                document
                    .read_to_string(&mut xml)
                    .map_err(|e| format!("Failed to read docx XML: {e}"))?;
                let count = scan_paragraphs(&xml).len();
                (xml, count)
            } else {
                let bytes = std::fs::read(&path)?;
                let text = crate::core::encoding::decode_native_output(&bytes);
                // Treat each line as a paragraph for the plain-text path.
                let count = text.lines().count();
                let escaped: String = text
                    .lines()
                    .map(|l| format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", xml_escape(l)))
                    .collect::<Vec<_>>()
                    .join("");
                (escaped, count)
            }
        };

        let hits = search_paragraphs(&xml, &terms, max_results);

        if hits.is_empty() {
            return Ok(ToolResult::success(format!(
                "--- Document: {}\nNo paragraphs matched \"{}\" ({} paragraphs scanned)",
                path.display(),
                query,
                total_paragraphs
            )));
        }

        let mut out = format!(
            "--- Document: {}\n{} of {} paragraphs match \"{}\":\n\n",
            path.display(),
            hits.len(),
            total_paragraphs,
            query
        );
        for (idx, text) in hits {
            let preview: String = text.chars().take(200).collect();
            let preview = if text.chars().count() > 200 {
                format!("{preview}…")
            } else {
                preview
            };
            out.push_str(&format!("[{idx}] {preview}\n"));
        }
        out.push_str(
            "\nUse docx_edit (replace/insert/delete with these indexes) to modify matches.",
        );
        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = concat!(
        "<w:document><w:body>",
        "<w:p><w:r><w:t>Quarterly Report Q3</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>Revenue grew 12%</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>Costs stayed flat</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>Revenue outlook for Q4</w:t></w:r></w:p>",
        "</w:body></w:document>"
    );

    #[test]
    fn finds_case_insensitive_matches() {
        let hits = search_paragraphs(SAMPLE_XML, &["revenue".to_string()], 20);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, 1);
        assert_eq!(hits[1].0, 3);
    }

    #[test]
    fn all_terms_must_match() {
        let hits = search_paragraphs(SAMPLE_XML, &["revenue".to_string(), "q4".to_string()], 20);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 3);
    }

    #[test]
    fn respects_max_results() {
        let hits = search_paragraphs(SAMPLE_XML, &["r".to_string()], 2);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn empty_terms_return_nothing() {
        let hits = search_paragraphs(SAMPLE_XML, &[], 20);
        assert!(hits.is_empty());
    }

    #[test]
    fn unescape_entities_before_matching() {
        // &amp; in XML must match as & in text.
        let xml = "<w:document><w:body><w:p><w:r><w:t>R&amp;D budget</w:t></w:r></w:p></w:body></w:document>";
        let hits = search_paragraphs(xml, &["r&d".to_string()], 20);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "R&D budget");
    }
}
