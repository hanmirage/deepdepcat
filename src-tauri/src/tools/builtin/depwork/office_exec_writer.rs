//! Writer (WPS 文字 / MS Word) action arms for `office_automate`.

use crate::toolkit::{ToolContext, ToolResult};
use crate::core::error::AppResult;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Emitter;

use super::office_host::{bgr_value, bridge_failure, host_call, run_bridge, truncate};

/// Dispatch a writer-family action. Returns the tool result when handled,
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
        .unwrap_or_else(|| "active document".to_string());
    let empty = PathBuf::new();
    let target = path.unwrap_or(&empty);

    match action {
        "read" => {
            let result = run_bridge(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let text = result.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let paragraphs = result
                .get("paragraphs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Ok(ToolResult::success(format!(
                "--- Document: {}\n({paragraphs} paragraphs, {} chars, via office COM)\n\n{}",
                display_target,
                text.chars().count(),
                truncate(text, 60_000)
            )))
        }
        "read_paragraphs" => {
            if let Some(from) = args.get("from").and_then(|v| v.as_u64()) {
                config["from"] = json!(from);
            }
            if let Some(to) = args.get("to").and_then(|v| v.as_u64()) {
                config["to"] = json!(to);
            }
            let result = run_bridge(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
            let text = result
                .get("paragraphs")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            Ok(ToolResult::success(format!(
                "--- Paragraphs of: {}\n({count} total, via office COM)\n\n{}",
                display_target,
                truncate(&text, 60_000)
            )))
        }
        "replace" | "insert" | "delete" => {
            let para = args
                .get("para")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: para (1-based)".to_string())?;
            if para < 1 {
                return Err("para must be 1-based (≥ 1)".into());
            }
            let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
            config["para"] = json!(para);
            config["text"] = json!(text);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Action {action} @ paragraph {para} — saved in {}",
                display_target
            )))
        }
        "replace_all" => {
            let find = args
                .get("find")
                .and_then(|f| f.as_str())
                .ok_or_else(|| "Missing required parameter: find".to_string())?;
            let text = args
                .get("text")
                .and_then(|t| t.as_str())
                .ok_or_else(|| "Missing required parameter: text".to_string())?;
            config["find"] = json!(find);
            config["text"] = json!(text);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let found = result.get("found").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(ToolResult::success(format!(
                "Replaced occurrences of \"{find}\" (matches: {found}) — saved in {}",
                display_target
            )))
        }
        "set_style" => {
            let para = args
                .get("para")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: para (1-based)".to_string())?;
            let style = args
                .get("style")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "Missing required parameter: style".to_string())?;
            config["para"] = json!(para);
            config["style"] = json!(style);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Set paragraph {para} style to \"{style}\" — saved in {}",
                display_target
            )))
        }
        "set_font" => {
            let para = args
                .get("para")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: para (1-based)".to_string())?;
            config["para"] = json!(para);
            for (key, src) in [
                ("size", "size"),
                ("bold", "bold"),
                ("italic", "italic"),
                ("underline", "underline"),
                ("font_name", "font_name"),
                ("color", "color"),
            ] {
                if let Some(v) = args.get(src) {
                    config[key] = v.clone();
                }
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Styled paragraph {para} — saved in {}",
                display_target
            )))
        }
        "type_text" => {
            let text = args
                .get("text")
                .and_then(|t| t.as_str())
                .ok_or_else(|| "Missing required parameter: text".to_string())?;
            config["text"] = json!(text);
            if let Some(para) = args.get("para").and_then(|v| v.as_u64()) {
                config["para"] = json!(para);
            }
            if let Some(pace) = args.get("pace").and_then(|v| v.as_u64()) {
                config["pace"] = json!(pace);
            }
            if let Some(chunk) = args.get("chunk").and_then(|v| v.as_u64()) {
                config["chunk"] = json!(chunk);
            }
            // Live typing hint while the WPS window is being written to.
            let _ = context.app.emit(
                "office-typing",
                json!({ "active": true, "total": 0, "chars": 0, "target": display_target }),
            );
            let result = host_call(config)?;
            let _ = context
                .app
                .emit("office-typing", json!({ "active": false }));
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let chars = result.get("chars").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(ToolResult::success(format!(
                "Typed {chars} chars into {} — visible in the open WPS window",
                display_target
            )))
        }
        "add_paragraph" => {
            let text = args
                .get("text")
                .and_then(|t| t.as_str())
                .ok_or_else(|| "Missing required parameter: text".to_string())?;
            config["text"] = json!(text);
            push_position(args, config);
            for (key, src) in [
                ("bold", "bold"),
                ("size", "size"),
                ("color", "color"),
                ("italic", "italic"),
                ("underline", "underline"),
                ("font_name", "font_name"),
            ] {
                if let Some(v) = args.get(src) {
                    config[key] = v.clone();
                }
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Appended paragraph — saved in {}",
                display_target
            )))
        }
        "add_heading" => {
            let text = args
                .get("text")
                .and_then(|t| t.as_str())
                .ok_or_else(|| "Missing required parameter: text".to_string())?;
            config["text"] = json!(text);
            if let Some(level) = args.get("level").and_then(|v| v.as_u64()) {
                if !(1..=6).contains(&level) {
                    return Err("level must be 1-6".into());
                }
                config["level"] = json!(level);
            }
            push_position(args, config);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Added heading — saved in {}",
                display_target
            )))
        }
        "add_list" => {
            let items = args
                .get("items")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    "Missing required parameter: items (array of strings)".to_string()
                })?;
            if items.is_empty() {
                return Err("items must not be empty".into());
            }
            if items.iter().any(|i| !i.is_string()) {
                return Err("items must be an array of strings".into());
            }
            config["items"] = json!(items);
            if let Some(style) = args.get("list_style").and_then(|s| s.as_str()) {
                config["list_style"] = json!(style);
            }
            push_position(args, config);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let n = result.get("items").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(ToolResult::success(format!(
                "Added {n} list items — saved in {}",
                display_target
            )))
        }
        "add_table" => {
            let (rows, cols) = table_dims(args)?;
            config["rows"] = json!(rows);
            config["cols"] = json!(cols);
            if let Some(data) = args.get("data").and_then(|d| d.as_array()) {
                config["data"] = json!(data);
            }
            if let Some(header) = args.get("header") {
                config["header"] = header.clone();
            }
            if let Some(c) = args.get("header_color").and_then(|v| v.as_u64()) {
                config["header_color"] = json!(bgr_value(c));
            }
            push_position(args, config);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Inserted {rows}×{cols} table — saved in {}",
                display_target
            )))
        }
        "add_image" => {
            let raw = args
                .get("image_path")
                .and_then(|p| p.as_str())
                .ok_or_else(|| "Missing required parameter: image_path".to_string())?;
            let img = crate::tools::builtin::resolve_path(context.workspace.as_deref(), raw);
            if !img.exists() {
                return Err(format!("Image file not found: {}", img.display()).into());
            }
            config["image_path"] = json!(img.to_string_lossy());
            for (key, src) in [("width_cm", "width_cm"), ("height_cm", "height_cm")] {
                if let Some(v) = args.get(src).and_then(|v| v.as_f64()) {
                    if v > 0.0 {
                        config[key] = json!(v * 28.3465); // cm → pt
                    }
                }
            }
            push_position(args, config);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Inserted image — saved in {}",
                display_target
            )))
        }
        "page_break" => {
            push_position(args, config);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Inserted page break — saved in {}",
                display_target
            )))
        }
        "set_alignment" => {
            let para = args
                .get("para")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: para (1-based)".to_string())?;
            let align = args
                .get("align")
                .and_then(|a| a.as_str())
                .ok_or_else(|| "Missing required parameter: align".to_string())?;
            if !matches!(align, "left" | "center" | "right" | "justify") {
                return Err("align must be left|center|right|justify".into());
            }
            config["para"] = json!(para);
            config["align"] = json!(align);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Aligned paragraph {para} {align} — saved in {}",
                display_target
            )))
        }
        "set_line_spacing" => {
            let para = args
                .get("para")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: para (1-based)".to_string())?;
            let multiple = args
                .get("multiple")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "Missing required parameter: multiple (e.g. 1.5)".to_string())?;
            if multiple <= 0.0 {
                return Err("multiple must be > 0".into());
            }
            config["para"] = json!(para);
            config["multiple"] = json!(multiple);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Set paragraph {para} line spacing to {multiple}× — saved in {}",
                display_target
            )))
        }
        "set_paragraph_format" => {
            let para = args
                .get("para")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: para (1-based)".to_string())?;
            config["para"] = json!(para);
            for (key, src) in [
                ("space_before", "space_before"),
                ("space_after", "space_after"),
                ("first_line_indent", "first_line_indent"),
                ("left_indent", "left_indent"),
            ] {
                if let Some(v) = args.get(src).and_then(|v| v.as_f64()) {
                    config[key] = json!(v);
                }
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Formatted paragraph {para} — saved in {}",
                display_target
            )))
        }
        "clear_doc" => {
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Cleared document content — saved in {}",
                display_target
            )))
        }
        "save_as" | "export_pdf" => super::office_host::save_or_export(
            action,
            args,
            config,
            path,
            "writer",
            context,
            &display_target,
        ),
        other => Err(format!("Unknown writer action: {other}").into()),
    }
}

fn push_position(args: &Value, config: &mut Value) {
    if let Some(pos) = args.get("position").and_then(|v| v.as_u64()) {
        config["position"] = json!(pos);
    }
}

/// Validate add_table dimensions: from `data` (2D array) or explicit
/// rows+cols.
pub fn table_dims(args: &Value) -> AppResult<(usize, usize)> {
    if let Some(data) = args.get("data").and_then(|d| d.as_array()) {
        if data.is_empty() {
            return Err("add_table: data must be a non-empty 2D array".into());
        }
        let rows = data.len();
        let cols = data
            .iter()
            .filter_map(|r| r.as_array())
            .map(|r| r.len())
            .max()
            .unwrap_or(0);
        return Ok((rows, cols));
    }
    let rows = args
        .get("rows")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "add_table needs data (2D array) or rows+cols".to_string())?
        as usize;
    let cols = args
        .get("cols")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "add_table needs data (2D array) or rows+cols".to_string())?
        as usize;
    if rows == 0 || cols == 0 {
        return Err("add_table: rows and cols must be ≥ 1".into());
    }
    Ok((rows, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_dims_from_data() {
        let args = json!({"data": [["a","b"],["c","d","e"]]});
        let (rows, cols) = table_dims(&args).expect("dims");
        assert_eq!((rows, cols), (2, 3));
    }

    #[test]
    fn table_dims_from_rows_cols() {
        let args = json!({"rows": 4, "cols": 3});
        let (rows, cols) = table_dims(&args).expect("dims");
        assert_eq!((rows, cols), (4, 3));
    }

    #[test]
    fn table_dims_rejects_empty() {
        assert!(table_dims(&json!({"data": []})).is_err());
        assert!(table_dims(&json!({"rows": 0, "cols": 3})).is_err());
        assert!(table_dims(&json!({})).is_err());
    }
}
