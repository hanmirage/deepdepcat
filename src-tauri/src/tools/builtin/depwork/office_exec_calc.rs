//! Calc (WPS 表格 / MS Excel) action arms for `office_automate`.

use crate::toolkit::{ToolContext, ToolResult};
use crate::core::error::AppResult;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::office_host::{bgr_value, bridge_failure, host_call, run_bridge, truncate};

/// Dispatch a calc-family action. Returns the tool result when handled,
/// or the action name when this family does not know it.
pub fn run(
    action: &str,
    args: &Value,
    config: &mut Value,
    path: Option<&PathBuf>,
    context: &ToolContext,
) -> AppResult<ToolResult> {
    let display_target = path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "active workbook".to_string());
    let empty = PathBuf::new();
    let target = path.unwrap_or(&empty);

    match action {
        "read_cells" => {
            let sheet = args
                .get("sheet")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1);
            push_sheet(args, config);
            let result = run_bridge(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let sheet_default = sheet.to_string();
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or(&sheet_default);
            let rows = result
                .get("rows")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let text = result
                .get("rows")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            Ok(ToolResult::success(format!(
                "--- Spreadsheet: {}\n(sheet \"{resolved}\", {rows} rows, via office COM)\n\n{}",
                display_target,
                truncate(&text, 60_000)
            )))
        }
        "read_cell" => {
            push_sheet(args, config);
            let row = args
                .get("row")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: row".to_string())?;
            let col = args
                .get("col")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: col".to_string())?;
            config["row"] = json!(row);
            config["col"] = json!(col);
            let result = run_bridge(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let value = result.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let formula = result.get("formula").and_then(|v| v.as_str()).unwrap_or("");
            let row_default = row.to_string();
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or(&row_default);
            Ok(ToolResult::success(format!(
                "--- Cell (row {row}, col {col}, sheet \"{resolved}\") in {}: \"{}\"{}",
                display_target,
                value,
                if formula.is_empty() {
                    String::new()
                } else {
                    format!(" (formula: {formula})")
                }
            )))
        }
        "list_sheets" => {
            let result = run_bridge(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let count = result
                .get("sheet_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let text = result
                .get("sheets")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            Ok(ToolResult::success(format!(
                "--- Sheets of: {}\n({count} sheets, via office COM)\n\n{}",
                display_target,
                truncate(&text, 20_000)
            )))
        }
        "write_cell" | "set_formula" => {
            push_sheet(args, config);
            let row = args
                .get("row")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: row".to_string())?;
            let col = args
                .get("col")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: col".to_string())?;
            config["row"] = json!(row);
            config["col"] = json!(col);
            if action == "write_cell" {
                let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
                config["text"] = json!(text);
            } else {
                let formula = args
                    .get("formula")
                    .and_then(|f| f.as_str())
                    .ok_or_else(|| {
                        "Missing required parameter: formula (e.g. =SUM(A1:A10))".to_string()
                    })?;
                config["formula"] = json!(formula);
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let what = if action == "write_cell" {
                "Wrote"
            } else {
                "Set formula on"
            };
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let at = if resolved.is_empty() {
                String::new()
            } else {
                format!(" in sheet \"{resolved}\"")
            };
            Ok(ToolResult::success(format!(
                "{what} cell ({row}, {col}){at} — saved in {}",
                display_target
            )))
        }
        "write_range" => {
            push_sheet(args, config);
            let range_ref = args
                .get("range_ref")
                .and_then(|r| r.as_str())
                .ok_or_else(|| "Missing required parameter: range_ref (e.g. A1)".to_string())?;
            let data = args
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| "Missing required parameter: data (2D array)".to_string())?;
            if data.is_empty() {
                return Err("data must be a non-empty 2D array".into());
            }
            if data.iter().any(|r| !r.is_array()) {
                return Err("data must be a 2D array of strings".into());
            }
            config["range_ref"] = json!(range_ref);
            config["data"] = json!(data);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let cells = result.get("cells").and_then(|v| v.as_u64()).unwrap_or(0);
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let at = if resolved.is_empty() {
                String::new()
            } else {
                format!(" in sheet \"{resolved}\"")
            };
            Ok(ToolResult::success(format!(
                "Wrote {cells} cells starting at {range_ref}{at} — saved in {}",
                display_target
            )))
        }
        "merge_cells" | "unmerge_cells" | "clear_range" => {
            push_sheet(args, config);
            let range_ref = args
                .get("range_ref")
                .and_then(|r| r.as_str())
                .ok_or_else(|| "Missing required parameter: range_ref (e.g. A1:C5)".to_string())?;
            config["range_ref"] = json!(range_ref);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let at = if resolved.is_empty() {
                String::new()
            } else {
                format!(" (sheet \"{resolved}\")")
            };
            Ok(ToolResult::success(format!(
                "{action} {range_ref}{at} — saved in {}",
                display_target
            )))
        }
        "add_sheet" => {
            if let Some(name) = args.get("name").and_then(|n| n.as_str()) {
                config["name"] = json!(name);
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let name = result.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let index = result.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let where_at = if index > 0 {
                format!(" (index {index})")
            } else {
                String::new()
            };
            Ok(ToolResult::success(format!(
                "Added worksheet \"{name}\"{where_at} — saved in {}",
                display_target
            )))
        }
        "rename_sheet" | "remove_sheet" => {
            push_sheet(args, config);
            if let Some(name) = args.get("name").and_then(|n| n.as_str()) {
                config["name"] = json!(name);
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let at = if resolved.is_empty() {
                String::new()
            } else {
                format!(" (sheet \"{resolved}\")")
            };
            Ok(ToolResult::success(format!(
                "{action}{at} — saved in {}",
                display_target
            )))
        }
        "set_column_width" | "set_row_height" => {
            push_sheet(args, config);
            if action == "set_column_width" {
                let col = match args.get("col").and_then(|v| v.as_str()) {
                    Some(c) => c.to_string(),
                    None => args
                        .get("col")
                        .and_then(|v| v.as_u64())
                        .map(|n| n.to_string())
                        .ok_or_else(|| {
                            "Missing required parameter: col (letters like \"A\" or number)"
                                .to_string()
                        })?,
                };
                let width = args
                    .get("width")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| "Missing required parameter: width".to_string())?;
                if width <= 0.0 {
                    return Err("width must be > 0".into());
                }
                config["col"] = json!(col);
                config["width"] = json!(width);
            } else {
                let row = args
                    .get("row")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "Missing required parameter: row".to_string())?;
                let height = args
                    .get("height")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| "Missing required parameter: height".to_string())?;
                if height <= 0.0 {
                    return Err("height must be > 0".into());
                }
                config["row"] = json!(row);
                config["height"] = json!(height);
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let at = if resolved.is_empty() {
                String::new()
            } else {
                format!(" (sheet \"{resolved}\")")
            };
            Ok(ToolResult::success(format!(
                "{action}{at} — saved in {}",
                display_target
            )))
        }
        "set_cell_style" => {
            push_sheet(args, config);
            let row = args
                .get("row")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: row".to_string())?;
            let col = args
                .get("col")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: col".to_string())?;
            config["row"] = json!(row);
            config["col"] = json!(col);
            for (key, src) in [
                ("bold", "bold"),
                ("italic", "italic"),
                ("font_size", "font_size"),
                ("wrap", "wrap"),
                ("align", "align"),
            ] {
                if let Some(v) = args.get(src) {
                    config[key] = v.clone();
                }
            }
            if let Some(c) = args.get("font_color").and_then(|v| v.as_u64()) {
                config["font_color"] = json!(bgr_value(c));
            }
            if let Some(c) = args.get("bg_color").and_then(|v| v.as_u64()) {
                config["bg_color"] = json!(bgr_value(c));
            }
            if let Some(a) = args.get("align").and_then(|v| v.as_str()) {
                if !matches!(a, "left" | "center" | "right" | "justify") {
                    return Err("align must be left|center|right|justify".into());
                }
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let resolved = result
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let at = if resolved.is_empty() {
                String::new()
            } else {
                format!(" (sheet \"{resolved}\")")
            };
            Ok(ToolResult::success(format!(
                "Styled cell ({row}, {col}){at} — saved in {}",
                display_target
            )))
        }
        "save_as" | "export_pdf" => super::office_host::save_or_export(
            action,
            args,
            config,
            path,
            "calc",
            context,
            &display_target,
        ),
        other => Err(format!("Unknown calc action: {other}").into()),
    }
}

fn push_sheet(args: &Value, config: &mut Value) {
    let sheet = args
        .get("sheet")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1);
    config["sheet"] = json!(sheet);
    if let Some(name) = args.get("sheet_name").and_then(|v| v.as_str()) {
        if !name.trim().is_empty() {
            config["sheet_name"] = json!(name);
        }
    }
}
