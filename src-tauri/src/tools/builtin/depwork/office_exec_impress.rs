//! Impress (WPS 演示 / MS PowerPoint) action arms for `office_automate`.

use crate::toolkit::{ToolContext, ToolResult};
use crate::core::error::AppResult;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::office_host::{bgr_value, bridge_failure, host_call, run_bridge, truncate};

/// Dispatch an impress-family action. Returns the tool result when handled,
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
        .unwrap_or_else(|| "active presentation".to_string());
    let empty = PathBuf::new();
    let target = path.unwrap_or(&empty);

    match action {
        "read_slides" => {
            let result = run_bridge(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
            let text = result
                .get("slides")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .enumerate()
                        .filter_map(|(i, s)| {
                            s.as_str()
                                .map(|t| format!("--- Slide {} ---\n{}", i + 1, t))
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .unwrap_or_default();
            Ok(ToolResult::success(format!(
                "--- Presentation: {}\n({count} slides, via office COM)\n\n{}",
                display_target,
                truncate(&text, 60_000)
            )))
        }
        "add_slide" => {
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1);
            let title = args.get("title").and_then(|t| t.as_str()).unwrap_or("");
            let body = args.get("body").and_then(|t| t.as_str()).unwrap_or("");
            config["index"] = json!(index);
            config["title"] = json!(title);
            config["body"] = json!(body);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Added slide at position {index} — saved in {}",
                display_target
            )))
        }
        "remove_slide" => {
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: index (1-based)".to_string())?
                .max(1);
            config["index"] = json!(index);
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Removed slide {index} — saved in {}",
                display_target
            )))
        }
        "set_slide_content" => {
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: index (1-based)".to_string())?
                .max(1);
            config["index"] = json!(index);
            for (key, src) in [("title", "title"), ("body", "body")] {
                if let Some(v) = args.get(src).and_then(|v| v.as_str()) {
                    config[key] = json!(v);
                }
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Updated slide {index} content — saved in {}",
                display_target
            )))
        }
        "add_textbox" | "add_shape" => {
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: index (1-based)".to_string())?
                .max(1);
            config["index"] = json!(index);
            for (key, src) in [
                ("x", "x"),
                ("y", "y"),
                ("width", "width"),
                ("height", "height"),
                ("text", "text"),
                ("shape", "shape"),
            ] {
                if let Some(v) = args.get(src) {
                    config[key] = v.clone();
                }
            }
            if let Some(s) = args.get("font_size").and_then(|v| v.as_f64()) {
                config["font_size"] = json!(s);
            }
            if let Some(b) = args.get("bold").and_then(|v| v.as_bool()) {
                config["bold"] = json!(b);
            }
            if let Some(c) = args.get("font_color").and_then(|v| v.as_u64()) {
                config["font_color"] = json!(bgr_value(c));
            }
            if let Some(c) = args.get("fill_color").and_then(|v| v.as_u64()) {
                config["fill_color"] = json!(bgr_value(c));
            }
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "{action} on slide {index} — saved in {}",
                display_target
            )))
        }
        "set_slide_bg" => {
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: index (1-based)".to_string())?
                .max(1);
            let color = args
                .get("color")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: color (0xRRGGBB)".to_string())?;
            config["index"] = json!(index);
            config["color"] = json!(bgr_value(color));
            let result = host_call(config)?;
            if let Some(err) = bridge_failure(&result, action, target) {
                return Ok(ToolResult::error(err));
            }
            Ok(ToolResult::success(format!(
                "Set slide {index} background — saved in {}",
                display_target
            )))
        }
        "add_image" => {
            let index = args
                .get("index")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: index (1-based)".to_string())?
                .max(1);
            let raw = args
                .get("image_path")
                .and_then(|p| p.as_str())
                .ok_or_else(|| "Missing required parameter: image_path".to_string())?;
            let img = crate::tools::builtin::resolve_path(context.workspace.as_deref(), raw);
            if !img.exists() {
                return Err(format!("Image file not found: {}", img.display()).into());
            }
            config["index"] = json!(index);
            config["image_path"] = json!(img.to_string_lossy());
            for (key, src) in [
                ("x", "x"),
                ("y", "y"),
                ("width", "width"),
                ("height", "height"),
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
                "Inserted image on slide {index} — saved in {}",
                display_target
            )))
        }
        "save_as" | "export_pdf" => super::office_host::save_or_export(
            action,
            args,
            config,
            path,
            "impress",
            context,
            &display_target,
        ),
        other => Err(format!("Unknown impress action: {other}").into()),
    }
}
