//! Embedded PowerShell scripts + Office format constants for the COM
//! automation bridge. Kept separate so `office_host.rs` stays focused on
//! process plumbing.

use std::path::Path;

pub const BRIDGE_SCRIPT: &str = include_str!("../../../../assets/office/office_bridge.ps1");
pub const HOST_SCRIPT: &str = include_str!("../../../../assets/office/office_host.ps1");
pub const HOST_CALC_SCRIPT: &str = include_str!("../../../../assets/office/ddc_office_host_calc.ps1");
pub const HOST_IMPRESS_SCRIPT: &str =
    include_str!("../../../../assets/office/ddc_office_host_impress.ps1");

/// Word SaveAs2 format constants (VBA-compatible, WPS implements them).
const WDFORMAT_DOCX: i64 = 16;
const WDFORMAT_PDF: i64 = 17;
const WDFORMAT_RTF: i64 = 6;
const WDFORMAT_TXT: i64 = 5;
const WDFORMAT_HTML: i64 = 10;
const WDFORMAT_XPS: i64 = 18;
const WDFORMAT_ODT: i64 = 23;
const WDFORMAT_DOC: i64 = 0;

/// Excel SaveAs2 format constants.
const XL_OPENXML_WORKBOOK: i64 = 51;
const XL_WORKBOOK_NORMAL: i64 = -4143;
const XL_CSV_UTF8: i64 = 62;

/// PowerPoint SaveAs format constants.
const PP_OPENXML_PRESENTATION: i64 = 1;
const PP_POWERPOINT_7: i64 = 2;

/// SaveAs2 format code for a file extension within an app family.
pub fn save_format_for(app: &str, ext: &str) -> Option<i64> {
    match app {
        "calc" => match ext {
            "xlsx" => Some(XL_OPENXML_WORKBOOK),
            "xls" => Some(XL_WORKBOOK_NORMAL),
            "csv" => Some(XL_CSV_UTF8),
            _ => None,
        },
        "impress" => match ext {
            "pptx" => Some(PP_OPENXML_PRESENTATION),
            "ppt" => Some(PP_POWERPOINT_7),
            _ => None,
        },
        _ => match ext {
            "docx" => Some(WDFORMAT_DOCX),
            "doc" => Some(WDFORMAT_DOC),
            "pdf" => Some(WDFORMAT_PDF),
            "rtf" => Some(WDFORMAT_RTF),
            "txt" => Some(WDFORMAT_TXT),
            "html" | "htm" => Some(WDFORMAT_HTML),
            "xps" => Some(WDFORMAT_XPS),
            "odt" => Some(WDFORMAT_ODT),
            _ => None,
        },
    }
}

/// Infer the office app family from a file extension.
pub fn app_for_extension(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "xls" | "xlsx" | "et" | "csv" => "calc",
        "ppt" | "pptx" | "dps" => "impress",
        _ => "writer",
    }
}

/// Turn a user-facing 0xRRGGBB color into the COM long value
/// (BGR for Word/Excel, RGB for PowerPoint — identical bit layout).
pub fn bgr_value(rgb: u64) -> i64 {
    let rgb = (rgb & 0xFF_FFFF) as i64;
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb & 0xFF_0000) >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_format_mapping() {
        assert_eq!(save_format_for("writer", "docx"), Some(16));
        assert_eq!(save_format_for("writer", "pdf"), Some(17));
        assert_eq!(save_format_for("writer", "odt"), Some(23));
        assert_eq!(save_format_for("calc", "xlsx"), Some(51));
        assert_eq!(save_format_for("calc", "csv"), Some(62));
        assert_eq!(save_format_for("impress", "pptx"), Some(1));
        assert_eq!(save_format_for("writer", "bmp"), None);
        assert_eq!(save_format_for("calc", "docx"), None);
    }

    #[test]
    fn app_inferred_from_extension() {
        let cases: [(&str, &str); 6] = [
            ("a.docx", "writer"),
            ("b.wps", "writer"),
            ("c.xlsx", "calc"),
            ("d.et", "calc"),
            ("e.csv", "calc"),
            ("f.pptx", "impress"),
        ];
        for (name, expected) in cases {
            assert_eq!(app_for_extension(Path::new(name)), expected, "{name}");
        }
        assert_eq!(app_for_extension(Path::new("no-ext")), "writer");
    }

    #[test]
    fn bridge_script_uses_safe_single_arg_pattern() {
        // The script file may be checked out with CRLF or LF line endings —
        // normalize before asserting so the contract (single JSON arg, no
        // shell interpolation) holds regardless of checkout line endings.
        let normalized = BRIDGE_SCRIPT.replace("\r\n", "\n");
        assert!(normalized.contains("param(\n  [string]$ArgsJson"));
        assert!(normalized.contains("ConvertFrom-Json"));
    }

    #[test]
    fn host_script_contains_all_new_writer_actions() {
        for a in [
            "add_paragraph",
            "add_heading",
            "add_list",
            "add_table",
            "add_image",
            "page_break",
            "set_alignment",
            "set_line_spacing",
            "set_paragraph_format",
            "clear_doc",
        ] {
            assert!(HOST_SCRIPT.contains(a), "host script missing {a}");
        }
    }

    #[test]
    fn host_calc_script_contains_all_new_calc_actions() {
        for a in [
            "write_range",
            "set_formula",
            "merge_cells",
            "unmerge_cells",
            "clear_range",
            "add_sheet",
            "rename_sheet",
            "remove_sheet",
            "set_column_width",
            "set_row_height",
            "set_cell_style",
        ] {
            assert!(HOST_CALC_SCRIPT.contains(a), "calc script missing {a}");
        }
    }

    #[test]
    fn host_calc_script_resolves_sheets_by_name_and_reports_index() {
        assert!(
            HOST_CALC_SCRIPT.contains("sheet_name"),
            "calc script must support sheet_name"
        );
        assert!(
            HOST_CALC_SCRIPT.contains("Select-Worksheet"),
            "calc script must resolve sheets via a shared helper"
        );
        assert!(
            HOST_CALC_SCRIPT.contains("sheet_index = $ws.Index"),
            "calc script must report the resolved sheet index"
        );
    }

    #[test]
    fn host_script_closes_documents_it_opened_itself() {
        for marker in [
            "$toolOpened",
            "wb.Close($true)",
            "pres.Close",
            "doc.Close(1)",
            "DisplayAlerts",
            "app.Quit()",
        ] {
            assert!(
                HOST_SCRIPT.contains(marker),
                "host script missing lock-release marker {marker}"
            );
        }
    }

    #[test]
    fn bridge_script_closes_documents_it_opened_itself() {
        for marker in ["$openedHere", "wb.Close($false)", "doc.Close(0)", "app.Quit()"] {
            assert!(
                BRIDGE_SCRIPT.contains(marker),
                "bridge script missing read-close marker {marker}"
            );
        }
    }

    #[test]
    fn host_impress_script_contains_all_new_impress_actions() {
        for a in [
            "remove_slide",
            "set_slide_content",
            "add_textbox",
            "add_shape",
            "set_slide_bg",
            "export_pdf",
            "add_image",
        ] {
            assert!(
                HOST_IMPRESS_SCRIPT.contains(a),
                "impress script missing {a}"
            );
        }
    }

    #[test]
    fn bridge_script_contains_read_actions() {
        for a in ["read_paragraphs", "list_sheets", "read_cell"] {
            assert!(BRIDGE_SCRIPT.contains(a), "bridge script missing {a}");
        }
    }

    #[test]
    fn host_script_recreates_com_on_stale_wps_object() {
        assert!(
            HOST_SCRIPT.contains("0x800706BA|0x80010108|RPC|disconnect|0xFFF4001A|0x80004005"),
            "stale WPS object errors must be in the recreate whitelist"
        );
        assert!(
            HOST_SCRIPT.contains("$app = $null"),
            "reconnect drops the stale app reference"
        );
    }

    #[test]
    fn scripts_open_office_at_fixed_window_size() {
        // 由本工具启动的办公窗口固定 1300x900；附加到已开窗口不 resize。
        for script in [HOST_SCRIPT, BRIDGE_SCRIPT] {
            assert!(script.contains("$launched = $false"), "launch flag");
            assert!(script.contains("$win.Width = 1300"), "window width");
            assert!(script.contains("$win.Height = 900"), "window height");
        }
    }

    #[test]
    fn host_script_retries_export_pdf_on_fresh_instance() {
        // WPS quirk: ExportAsFixedFormat fails (0x80004005) on a freshly
        // launched instance — retry after a settle, then fall back to
        // SaveAs2(wdFormatPDF=17) for writer; calc retries once.
        assert!(HOST_SCRIPT.contains("Start-Sleep -Milliseconds 1500"));
        assert!(HOST_SCRIPT.contains("$doc.SaveAs2([string]$config.path, 17)"));
        assert!(HOST_CALC_SCRIPT.contains("Start-Sleep -Milliseconds 1500"));
    }

    #[test]
    fn host_script_skips_trailing_save_after_export_pdf() {
        // WPS quirk: Save() fails after ExportAsFixedFormat — the trailing
        // save must be skipped for export_pdf in all three families (and
        // wrapped in try/catch so WPS's occasional 保存失败 never interrupts
        // the flow or blocks the window-size step).
        assert!(HOST_SCRIPT
            .contains("if ($action -ne \"export_pdf\") { try { $doc.Save() } catch { } }"));
        assert!(HOST_SCRIPT
            .contains("if ($action -ne \"export_pdf\") { try { $wb.Save() } catch { } }"));
        assert!(HOST_SCRIPT
            .contains("if ($action -ne \"export_pdf\") { try { $pres.Save() } catch { } }"));
    }

    #[test]
    fn bgr_conversion_matches_office_long() {
        assert_eq!(bgr_value(0xFF0000), 0xFF);
        assert_eq!(bgr_value(0x0000FF), 0xFF0000);
        assert_eq!(bgr_value(0x336699), 0x996633);
    }
}
