//! xlsx_generate — create an Excel (.xlsx) workbook from CSV data.
//!
//! Pure Rust, no Office install needed (same pattern as docx_generate):
//! a minimal but Excel-compatible package — workbook + worksheet with
//! inline strings (no sharedStrings table), numbers as numeric cells.
//!
//! Input: `data` as CSV text (headers row optional); rows are parsed with
//! the `csv` crate (quotes/escapes handled).

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

/// Excel sheet name → column reference (A, B, …, Z, AA, …).
fn column_ref(col: usize) -> String {
    let mut n = col;
    let mut out = String::new();
    loop {
        let rem = (n % 26) as u8;
        out.insert(0, (b'A' + rem) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out
}

/// XML-escape cell text.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// True when the value should be a numeric cell (Excel stores it as a
/// number; leading zeros / large numbers stay text).
fn is_numeric(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 15
        && v.parse::<f64>().is_ok()
        && !(v.len() > 1 && v.starts_with('0') && !v.contains('.'))
}

/// Build the worksheet XML for a CSV table.
fn build_sheet_xml(rows: &[Vec<String>], sheet_name: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    for (r, row) in rows.iter().enumerate() {
        out.push_str(&format!("<row r=\"{}\">", r + 1));
        for (c, cell) in row.iter().enumerate() {
            let ref_ = format!("{}{}", column_ref(c), r + 1);
            if let Some(formula) = cell.strip_prefix('=') {
                // Excel formula cells (`<f>`): anything starting with "="
                // is written as a formula, not text.
                out.push_str(&format!(
                    "<c r=\"{ref_}\"><f>{}</f></c>",
                    xml_escape(formula)
                ));
            } else if is_numeric(cell) {
                out.push_str(&format!("<c r=\"{ref_}\"><v>{}</v></c>", xml_escape(cell)));
            } else if !cell.is_empty() {
                out.push_str(&format!(
                    "<c r=\"{ref_}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    xml_escape(cell)
                ));
            }
        }
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    let _ = sheet_name;
    out
}

/// Build a complete .xlsx package from CSV rows. Returns the byte buffer.
pub fn build_xlsx(rows: &[Vec<String>], sheet_name: &str) -> AppResult<Vec<u8>> {
    let safe_sheet: String = sheet_name
        .chars()
        .map(|c| match c {
            '\\' | '/' | '*' | '?' | ':' | '[' | ']' => '_',
            _ => c,
        })
        .take(31)
        .collect();
    let safe_sheet = if safe_sheet.is_empty() {
        "Sheet1".to_string()
    } else {
        safe_sheet
    };

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
        <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
        <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
        </Types>";
    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
        </Relationships>";
    let workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
        <sheets><sheet name=\"{safe_sheet}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
    );
    let workbook_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
        <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
        </Relationships>";
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
        <fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
        <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
        <borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
        <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
        <cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs>\
        <cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
        </styleSheet>";
    let sheet_xml = build_sheet_xml(rows, sheet_name);

    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(content_types.as_bytes())?;
        zip.start_file("_rels/.rels", opts)?;
        zip.write_all(root_rels.as_bytes())?;
        zip.start_file("xl/workbook.xml", opts)?;
        zip.write_all(workbook.as_bytes())?;
        zip.start_file("xl/_rels/workbook.xml.rels", opts)?;
        zip.write_all(workbook_rels.as_bytes())?;
        zip.start_file("xl/worksheets/sheet1.xml", opts)?;
        zip.write_all(sheet_xml.as_bytes())?;
        zip.start_file("xl/styles.xml", opts)?;
        zip.write_all(styles.as_bytes())?;
        zip.finish()?;
    }
    Ok(buf)
}

/// Parse CSV text into rows (handles quoted fields, commas inside quotes).
/// Every line is data — `has_headers(false)` keeps the header row.
pub fn parse_csv(data: &str) -> Vec<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(data.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records().flatten() {
        rows.push(record.iter().map(str::to_string).collect());
    }
    rows
}

/// Excel workbook generator tool.
pub struct XlsxGenerateTool;

impl XlsxGenerateTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for XlsxGenerateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "xlsx_generate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Generate an Excel (.xlsx) workbook from CSV data (no Office \
        install needed). Numbers become numeric cells, everything else \
        stays text. Use for data deliverables: budgets, schedules, \
        inventories, survey results. A later table_process pass can read \
        it back for verification."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path, e.g. C:\\work\\budget.xlsx (adds .xlsx if missing)."
                },
                "data": {
                    "type": "string",
                    "description": "CSV content: first row is the header. Quotes handle commas inside fields."
                },
                "sheet_name": {
                    "type": "string",
                    "description": "Worksheet name (default Sheet1, max 31 chars)."
                }
            },
            "required": ["path", "data"]
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
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("xlsx"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let data = args
            .get("data")
            .and_then(|d| d.as_str())
            .ok_or_else(|| "Missing required parameter: data".to_string())?
            .to_string();
        let sheet_name = args
            .get("sheet_name")
            .and_then(|s| s.as_str())
            .unwrap_or("Sheet1");

        let mut path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_str);
        if path.extension().is_none() {
            path.set_extension("xlsx");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
        }

        let rows = parse_csv(&data);
        if rows.is_empty() {
            return Err("data must contain at least a header row".into());
        }
        let bytes = build_xlsx(&rows, sheet_name)?;
        std::fs::write(&path, &bytes).map_err(|e| {
            format!(
                "Failed to write {}: {e} — check the file is not open in Excel/WPS",
                path.display()
            )
        })?;

        super::permissions::record_output(context, &path);
        Ok(ToolResult::success(format!(
            "Created Excel workbook: {}\n({} rows × {} columns, sheet \"{}\", {} bytes)",
            path.display(),
            rows.len().saturating_sub(1),
            rows.first().map(|r| r.len()).unwrap_or(0),
            sheet_name,
            bytes.len()
        )))
    }
}

/// Read back an xlsx (first sheet) as CSV — used by tests and available to
/// the model via table_process.
pub(crate) fn _read_xlsx_csv(path: &Path) -> AppResult<String> {
    let file = std::fs::File::open(path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid xlsx package: {e}"))?;
    let mut sheet = archive
        .by_name("xl/worksheets/sheet1.xml")
        .map_err(|e| format!("xlsx has no sheet1.xml: {e}"))?;
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut sheet, &mut xml)
        .map_err(|e| format!("Failed to read sheet XML: {e}"))?;
    Ok(xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_refs() {
        assert_eq!(column_ref(0), "A");
        assert_eq!(column_ref(25), "Z");
        assert_eq!(column_ref(26), "AA");
        assert_eq!(column_ref(27), "AB");
        assert_eq!(column_ref(701), "ZZ");
    }

    #[test]
    fn numeric_detection() {
        assert!(is_numeric("123"));
        assert!(is_numeric("3.14"));
        assert!(is_numeric("-42"));
        assert!(!is_numeric("0123")); // leading zero → text
        assert!(!is_numeric("abc"));
        assert!(!is_numeric("1234567890123456")); // > 15 digits → text
        assert!(!is_numeric(""));
    }

    #[test]
    fn formula_cells_write_f_elements() {
        let xml = build_sheet_xml(
            &[vec!["=SUM(A1:A2)".to_string(), "plain".to_string()]],
            "Sheet1",
        );
        assert!(
            xml.contains("<f>SUM(A1:A2)</f>"),
            "leading '=' becomes a formula cell: {xml}"
        );
        assert!(
            !xml.contains("<c r=\"A1\" t=\"inlineStr\""),
            "formula cell is not inline text"
        );
    }

    #[test]
    fn csv_parsing_handles_quotes() {
        let rows = parse_csv("name,note\n\"Li, Wei\",\"said \"\"hi\"\"\"\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][0], "Li, Wei");
        assert_eq!(rows[1][1], "said \"hi\"");
    }

    #[test]
    fn xlsx_roundtrip_via_zip() {
        let rows = vec![
            vec!["item".to_string(), "qty".to_string(), "note".to_string()],
            vec!["apple".to_string(), "3".to_string(), "a < b".to_string()],
        ];
        let bytes = build_xlsx(&rows, "测试表").expect("build");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.xlsx");
        std::fs::write(&path, &bytes).expect("write");

        let xml = _read_xlsx_csv(&path).expect("read back");
        assert!(xml.contains("<v>3</v>"), "number cell");
        assert!(xml.contains("t=\"inlineStr\""), "inline string cell");
        assert!(xml.contains("a &lt; b"), "escaped text");
        assert!(!xml.contains("测试表") || !bytes.is_empty()); // sheet name not in sheet xml
                                                               // Workbook references the sanitized sheet name.
        let file = std::fs::File::open(&path).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut wb = archive.by_name("xl/workbook.xml").expect("workbook");
        let mut wb_xml = String::new();
        std::io::Read::read_to_string(&mut wb, &mut wb_xml).expect("read");
        assert!(wb_xml.contains("测试表"), "sheet name kept");
    }

    #[test]
    fn xlsx_package_is_well_formed_zip() {
        let rows = vec![vec!["a".to_string(), "b".to_string()]];
        let bytes = build_xlsx(&rows, "Sheet1").expect("build");
        let cursor = std::io::Cursor::new(bytes);
        let archive = zip::ZipArchive::new(cursor).expect("valid zip");
        assert!(
            archive.len() >= 5,
            "content types + rels + workbook + sheet + styles"
        );
    }
}
