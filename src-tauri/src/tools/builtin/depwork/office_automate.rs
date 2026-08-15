//! office_automate — operate Office documents through the office
//! application itself (COM automation, bridged by PowerShell).
//!
//! ProgID detection: `KWPS.Application` (WPS 文字) / `KET.Application`
//! (WPS 表格) / `KWPP.Application` (WPS 演示) first, falling back to the
//! Microsoft ProgIDs. The scripts speak the VBA object model, which WPS
//! implements compatibly.
//!
//! Write actions run through a persistent host process so the user's open
//! window repaints live; read actions use one-shot bridge processes.
//! The file-level tools (docx_edit / table_process) remain the fallback
//! when no office application is installed.
//!
//! Capability groups (ported from the office-automation reference design):
//! - Writer: read/read_paragraphs, paragraph replace/insert/delete/append,
//!   typewriter, find+replace, styles, fonts, headings, lists, TABLES
//!   (data + header styling), images, page breaks, alignment/line spacing/
//!   paragraph format, clear, save_as/export_pdf
//! - Calc: sheets list/add/rename/remove, cell/range read-write, formulas,
//!   merge/unmerge, clear range, column width/row height, cell styles,
//!   save_as/export_pdf
//! - Impress: slides list/add/remove, slide content, textboxes, autoshapes,
//!   slide backgrounds, save_as/export_pdf

pub use super::office_host::{fallback_hint, host_call};

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Office COM automation tool.
pub struct OfficeAutomateTool;

impl OfficeAutomateTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for OfficeAutomateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "office_automate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Operate Office documents through the office application itself \
        (COM automation) — read/edit documents that are open in WPS or \
        MS Word/Excel/PowerPoint, including .wps/.et/.dps files. App family \
        is inferred from the file extension (writer: docx/doc/wps; calc: \
        xlsx/xls/et/csv; impress: pptx/ppt/dps), overridable with app=. \
        Writer actions: read, read_paragraphs (from/to), replace, insert, \
        delete, type_text (typewriter — appends in small chunks so the user \
        SEES it written in the open window), replace_all (find+replace), \
        set_style, set_font (size/bold/italic/underline/font_name/color \
        0xRRGGBB), add_paragraph (append, with formatting), add_heading \
        (level 1-6), add_list (items + bullet/number), add_table (2D data \
        or rows+cols; header bold+shaded, header_color), add_image \
        (image_path, width_cm/height_cm), page_break, set_alignment, \
        set_line_spacing (multiple), set_paragraph_format (space_before/\
        after, first_line_indent, left_indent), clear_doc, save_as, \
        export_pdf. Calc actions: list_sheets, read_cells, read_cell, \
        write_cell, write_range (2D data at range_ref; target a sheet by \
        NAME via sheet_name — indexes shift after add_sheet), set_formula, \
        merge_cells/unmerge_cells, clear_range, add_sheet (name), \
        rename_sheet, remove_sheet, set_column_width, set_row_height, \
        set_cell_style (bold/italic/font_size/font_color/bg_color/align/\
        wrap), save_as, export_pdf. Impress actions: read_slides, \
        add_slide,         remove_slide, set_slide_content (title/body), \
        add_textbox (x/y/width/height points + font), add_shape (rectangle/\
        oval/diamond/triangle/star/arrow_right… + fill_color), add_image \
        (index + image_path + x/y/width/height points), set_slide_bg \
        (color), save_as, export_pdf. position inserts before a paragraph \
        (1-based); omitted appends at the end. Requires WPS Office or MS \
        Office installed; falls back to the file-level tools (docx_edit / \
        table_process) otherwise."
    }

    fn parameters(&self) -> Value {
        super::office_params::schema()
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Read actions (inspect a document) never prompt — per-call read
    /// classification; everything that mutates or saves prompts.
    fn is_read_only_call(&self, args: &Value) -> bool {
        args.get("action")
            .and_then(|a| a.as_str())
            .is_some_and(super::permissions::is_office_read_action)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?
            .to_ascii_lowercase();

        let requested_app = args
            .get("app")
            .and_then(|a| a.as_str())
            .unwrap_or("auto")
            .to_ascii_lowercase();
        const VALID_APPS: [&str; 6] = ["auto", "writer", "calc", "impress", "word", "wps"];
        if !VALID_APPS.contains(&requested_app.as_str()) {
            return Err(format!(
                "Invalid app: {requested_app} (auto|writer|calc|impress|word|wps)"
            )
            .into());
        }

        // ── detect: no path needed, app family selects the probe ──
        if action == "detect" {
            let app = if requested_app == "auto" {
                "writer"
            } else {
                requested_app.as_str()
            };
            let config = json!({ "action": "detect", "app": app });
            let result = super::office_host::run_bridge(&config)?;
            if let Some(err) = super::office_host::bridge_error(&result) {
                if err == "NO_OFFICE" {
                    return Ok(ToolResult::error(super::office_host::fallback_hint(
                        "detect",
                        std::path::Path::new(""),
                    )));
                }
                return Ok(ToolResult::error(err));
            }
            let name = result.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let version = result
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            return Ok(ToolResult::success(format!(
                "Office automation available: {name} (v{version})",
            )));
        }

        // Path is OPTIONAL for writer actions: omitted or "active" targets
        // the user's CURRENT document (ActiveDocument). File existence is
        // only required when a real path is given.
        let path: Option<PathBuf> = match args.get("path").and_then(|p| p.as_str()) {
            Some(p) => {
                if p == "active" {
                    None
                } else {
                    let resolved =
                        crate::tools::builtin::resolve_path(context.workspace.as_deref(), p);
                    if !resolved.exists() {
                        return Err(format!("File not found: {}", resolved.display()).into());
                    }
                    Some(resolved)
                }
            }
            None => None,
        };
        let app = if matches!(requested_app.as_str(), "auto" | "word" | "wps") {
            path.as_deref()
                .map(super::office_host::app_for_extension)
                .unwrap_or("writer")
        } else {
            requested_app.as_str()
        };
        let mut config = json!({ "action": action, "app": app });
        if let Some(p) = &path {
            config["path"] = json!(p.to_string_lossy());
        }

        match app {
            "calc" => {
                super::office_exec_calc::run(&action, &args, &mut config, path.as_ref(), context)
            }
            "impress" => {
                super::office_exec_impress::run(&action, &args, &mut config, path.as_ref(), context)
            }
            _ => {
                super::office_exec_writer::run(&action, &args, &mut config, path.as_ref(), context)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_offers_all_actions() {
        let schema = crate::tools::builtin::depwork::office_params::schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        for a in [
            "add_table",
            "add_heading",
            "add_list",
            "add_image",
            "add_paragraph",
            "write_range",
            "set_formula",
            "merge_cells",
            "add_sheet",
            "set_cell_style",
            "remove_slide",
            "add_textbox",
            "add_shape",
            "set_slide_bg",
            "read_paragraphs",
            "list_sheets",
            "export_pdf",
        ] {
            assert!(
                actions.iter().any(|v| v.as_str() == Some(a)),
                "schema missing action {a}"
            );
        }
    }

    #[test]
    fn schema_documents_table_parameters() {
        let schema = crate::tools::builtin::depwork::office_params::schema();
        for p in [
            "data",
            "rows",
            "cols",
            "header",
            "header_color",
            "range_ref",
            "formula",
        ] {
            assert!(
                schema["properties"][p].is_object(),
                "schema missing parameter {p}"
            );
        }
    }

    #[test]
    fn description_advertises_core_capabilities() {
        let tool = OfficeAutomateTool;
        let desc = tool.description();
        for k in [
            "add_table",
            "write_range",
            "set_formula",
            "merge_cells",
            "add_slide",
            "type_text",
        ] {
            assert!(desc.contains(k), "description missing {k}");
        }
    }
}
