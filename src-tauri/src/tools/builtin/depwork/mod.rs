//! Depwork tools — document automation for knowledge workers.
//!
//! These tools are only available in Depwork mode (see `ToolScope::Depwork`):
//! document reading/generation, table processing, presentation generation,
//! batch file operations, desktop UI automation, web research, media
//! processing, OCR and chart rendering. Code mode never sees them.

pub mod batch_file;
pub mod browser_control;
pub mod card;
pub mod chart;
pub mod citation_link;
pub mod content_pack;
pub mod doc_consistency;
pub mod doc_read;
pub mod docx_edit;
pub mod docx_generate;
pub mod docx_search;
pub mod live_doc_write;
pub mod media;
pub mod ocr;
pub mod office_automate;
pub mod office_exec_calc;
pub mod office_exec_impress;
pub mod office_exec_writer;
pub mod office_host;
pub mod office_params;
pub mod office_scripts;
pub mod pdf_generate;
pub mod pdf_tools;
pub mod permissions;
pub mod ppt_generate;
pub mod research;
pub mod research_paper;
pub mod store_research;
pub mod table_process;
pub mod ui_automate;
pub mod web_fetch;
pub mod web_open;
pub mod xlsx_generate;

pub use batch_file::BatchFileTool;
pub use browser_control::BrowserControlTool;
pub use chart::ChartGenerateTool;
pub use doc_read::DocReadTool;
pub use docx_edit::DocxEditTool;
pub use docx_generate::DocxGenerateTool;
pub use docx_search::DocxSearchTool;
pub use live_doc_write::LiveDocWriteTool;
pub use media::{MediaConvertTool, MediaProbeTool};
pub use ocr::OcrImageTool;
pub use office_automate::OfficeAutomateTool;
pub use pdf_generate::PdfGenerateTool;
pub use pdf_tools::PdfToolsTool;
pub use ppt_generate::PptGenerateTool;
pub use research::{
    ResearchClipTool, ResearchExportTool, ResearchFolderSearchTool, ResearchListTool,
    ResearchOpenAccessTool, ResearchRemoveTool, ResearchReportTool, ResearchSaveTool,
    ResearchSearchTool,
};
pub use research_paper::ResearchPaperTool;
pub use store_research::{StoreResearchGeoTool, StoreResearchMapTool, StoreResearchXhsTool};
pub use table_process::TableProcessTool;
pub use ui_automate::UiAutomateTool;
pub use web_fetch::WebFetchTool;
pub use web_open::WebOpenTool;
pub use xlsx_generate::XlsxGenerateTool;

/// Match a `w:p` OPEN tag, tolerating the attributes real Word docs attach
/// (`w14:paraId`, `w:rsidR`, …) — an exact `== "w:p"` silently misses every
/// attributed paragraph (doc_read returns empty text, docx_edit mis-indexes).
/// `w:pPr` / `w:pStyle` share the `w:p` prefix and must be rejected.
pub(crate) fn is_paragraph_open_tag(lower: &str) -> bool {
    lower.starts_with("w:p") && matches!(lower.as_bytes().get(3), None | Some(b' '))
}

/// One inline run with emphasis flags, parsed from Markdown.
#[derive(Clone)]
pub(crate) struct InlineRun {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) code: bool,
}

/// Parse inline Markdown into runs: `**bold**`, `*italic*`, `` `code` ``.
/// Unclosed delimiters stay literal (a stray `**` is not silently dropped).
pub(crate) fn parse_inline(text: &str) -> Vec<InlineRun> {
    let mut runs = Vec::new();
    let mut plain = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Inline code span: `...`
        if c == '`' {
            if let Some(end_rel) = chars[i + 1..].iter().position(|&x| x == '`') {
                let code: String = chars[i + 1..i + 1 + end_rel].iter().collect();
                if !plain.is_empty() {
                    runs.push(InlineRun { text: std::mem::take(&mut plain), bold: false, italic: false, code: false });
                }
                runs.push(InlineRun { text: code, bold: false, italic: false, code: true });
                i += 2 + end_rel;
                continue;
            }
        }
        // Bold: **...**
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            let mut end = None;
            let mut j = i + 2;
            while j + 1 < chars.len() {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    end = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = end {
                let bold: String = chars[i + 2..end].iter().collect();
                if !plain.is_empty() {
                    runs.push(InlineRun { text: std::mem::take(&mut plain), bold: false, italic: false, code: false });
                }
                runs.push(InlineRun { text: bold, bold: true, italic: false, code: false });
                i = end + 2;
                continue;
            }
        }
        // Italic: *x* (single star, not part of `**`)
        if c == '*' && chars.get(i + 1) != Some(&'*') {
            if let Some(end_rel) = chars[i + 1..].iter().position(|&x| x == '*') {
                let end = i + 1 + end_rel;
                let italic: String = chars[i + 1..end].iter().collect();
                if !plain.is_empty() {
                    runs.push(InlineRun { text: std::mem::take(&mut plain), bold: false, italic: false, code: false });
                }
                runs.push(InlineRun { text: italic, bold: false, italic: true, code: false });
                i = end + 1;
                continue;
            }
        }
        plain.push(c);
        i += 1;
    }
    if !plain.is_empty() {
        runs.push(InlineRun { text: plain, bold: false, italic: false, code: false });
    }
    runs
}

use crate::tools::registry::ToolRegistry;
use std::sync::Arc;

/// Register all Depwork-only tools.
pub fn register_depwork_tools(registry: &ToolRegistry) {
    // Dynamic count: the registry is shared with built-in tools, so the
    // delta between before/after registration is the Depwork-only total
    // (a hardcoded constant drifts when tools are added).
    let before = registry.len();
    registry.register(Arc::new(DocReadTool::new()));
    registry.register(Arc::new(doc_consistency::DocConsistencyTool::new()));
    registry.register(Arc::new(DocxGenerateTool::new()));
    registry.register(Arc::new(DocxEditTool::new()));
    registry.register(Arc::new(DocxSearchTool::new()));
    registry.register(Arc::new(XlsxGenerateTool::new()));
    registry.register(Arc::new(OfficeAutomateTool::new()));
    registry.register(Arc::new(TableProcessTool::new()));
    registry.register(Arc::new(PdfGenerateTool::new()));
    registry.register(Arc::new(PdfToolsTool::new()));
    registry.register(Arc::new(PptGenerateTool::new()));
    registry.register(Arc::new(BatchFileTool::new()));
    registry.register(Arc::new(BrowserControlTool::new()));
    registry.register(Arc::new(UiAutomateTool::new()));
    registry.register(Arc::new(WebFetchTool::new()));
    registry.register(Arc::new(WebOpenTool::new()));
    registry.register(Arc::new(MediaProbeTool::new()));
    registry.register(Arc::new(MediaConvertTool::new()));
    registry.register(Arc::new(OcrImageTool::new()));
    registry.register(Arc::new(ChartGenerateTool::new()));
    registry.register(Arc::new(card::CardGenerateTool::new()));
    registry.register(Arc::new(citation_link::CitationLinkTool::new()));
    registry.register(Arc::new(content_pack::ContentPackTool::new()));
    registry.register(Arc::new(LiveDocWriteTool::new()));
    registry.register(Arc::new(StoreResearchMapTool::new()));
    registry.register(Arc::new(StoreResearchXhsTool::new()));
    registry.register(Arc::new(StoreResearchGeoTool::new()));
    registry.register(Arc::new(ResearchSearchTool::new()));
    registry.register(Arc::new(ResearchSaveTool::new()));
    registry.register(Arc::new(ResearchListTool::new()));
    registry.register(Arc::new(ResearchRemoveTool::new()));
    registry.register(Arc::new(ResearchExportTool::new()));
    registry.register(Arc::new(ResearchFolderSearchTool::new()));
    registry.register(Arc::new(ResearchClipTool::new()));
    registry.register(Arc::new(ResearchReportTool::new()));
    registry.register(Arc::new(ResearchOpenAccessTool::new()));
    registry.register(Arc::new(ResearchPaperTool::new()));
    tracing::info!(
        "Registered {} depwork tools",
        registry.len().saturating_sub(before)
    );
}
