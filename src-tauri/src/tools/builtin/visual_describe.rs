//! visual_describe tool — the text main model's on-demand eye.
//!
//! Attached images are automatically transcribed when the user sends them,
//! but a text model (DeepSeek) may need MORE than the automatic description:
//! the exact error text in a screenshot, the fine print on a diagram, the
//! layout of a UI mock. This tool lets the model re-invoke the configured
//! vision endpoint with a custom question and get a fresh, targeted answer.
//!
//! It shares the transcription cache with the automatic pipeline, keyed by
//! (session, image bytes, prompt) — asking the SAME question about the SAME
//! image again is free; a NEW question gets a fresh vision call.

use crate::agent::image_transcribe::{
    transcribe_image_with_prompt, validate_vision_config, ImageInput,
    VISION_DESCRIBE_DEFAULT_PROMPT,
};
use crate::core::image_codec::ImageRegion;
use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager as _;

/// Maximum image size accepted for on-demand description — reading a huge
/// file into memory and shipping it to the vision API wastes both. Larger
/// files are rejected with a clear error instead of being processed.
const MAX_VISION_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

pub struct VisualDescribeTool;

impl VisualDescribeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VisualDescribeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for VisualDescribeTool {
    fn name(&self) -> &str {
        "visual_describe"
    }

    fn description(&self) -> &str {
        "Look at an image file in detail and return a precise text description \
         from the configured vision model. Attached pictures are already \
         described automatically when they arrive — use this tool only when \
         you need MORE detail: exact error text, fine print, specific elements \
         of a screenshot, diagram or UI. Ask a targeted question in `prompt`; \
         use `region` to zoom into a small part of a large image (a single \
         error line, a button, a chart corner)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the image file to describe. Relative paths resolve against the workspace root."
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional targeted question for the vision model (e.g. 'Transcribe all visible text exactly as written'). Defaults to a high-precision full description."
                },
                "region": {
                    "type": "string",
                    "description": "Optional crop region to zoom into, as 'x,y,w,h' in pixels, or '10%,20%,30%,40%' as fractions of the image. Use when the full image is too dense to read a specific part — e.g. one error line or a small UI element."
                }
            },
            "required": ["path"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path = args.get("path").and_then(|p| p.as_str()).ok_or_else(|| {
            crate::core::error::AppError::Parse("Missing 'path' parameter".into())
        })?;
        let prompt = args
            .get("prompt")
            .and_then(|p| p.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(VISION_DESCRIBE_DEFAULT_PROMPT)
            .to_string();

        // Optional zoom region — crop + upscale before the vision call so the
        // model can read a small part of a dense image. Malformed input is a
        // model-visible tool error (it should retry with a valid region).
        let region_str = args
            .get("region")
            .and_then(|r| r.as_str())
            .filter(|s| !s.trim().is_empty());
        let region = match region_str {
            Some(s) => match ImageRegion::parse(s) {
                Some(r) => Some(r),
                None => {
                    return Ok(ToolResult::error(format!(
                        "Invalid 'region' '{s}' — expected 'x,y,w,h' (pixels) or \
                         '10%,20%,30%,40%' (fractions of the image)."
                    )));
                }
            },
            None => None,
        };

        // Resolve vision config at execution time (hot-reloaded, unlike a
        // registration-time snapshot) so an unconfigured vision model fails
        // with an actionable message instead of a confusing API error.
        {
            let state = context.app.state::<crate::bootstrap::AppState>();
            let cfg = state.config()?;
            if let Err(e) = validate_vision_config(&cfg) {
                return Ok(ToolResult::error(e.to_string()));
            }
        }

        let path_buf = super::resolve_path(context.workspace.as_deref(), path);
        let meta = match std::fs::metadata(&path_buf) {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file '{path}': {e}"
                )));
            }
        };
        if meta.len() > MAX_VISION_IMAGE_BYTES {
            return Ok(ToolResult::error(format!(
                "Image file '{path}' is too large ({} MB) — visual_describe accepts files up to 10 MB",
                meta.len() / (1024 * 1024)
            )));
        }
        let bytes = match std::fs::read(&path_buf) {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file '{path}': {e}"
                )));
            }
        };
        let Some(mime) = crate::core::image_codec::sniff_mime(&bytes) else {
            return Ok(ToolResult::error(format!(
                "Not an image file ({path}) — visual_describe only accepts \
                 PNG/JPEG/WebP/GIF/BMP/TIFF/ICO."
            )));
        };

        let state = context.app.state::<crate::bootstrap::AppState>();
        // Subagent executions pass their parent session as a cache fallback:
        // a picture the parent already described (same image, same question)
        // is answered from the shared cache instead of re-invoking the vision
        // API. Results are still cached under the subagent's own session.
        let parent_sessions: Vec<&str> = context
            .parent_session_id
            .as_deref()
            .map(|s| vec![s])
            .unwrap_or_default();
        match transcribe_image_with_prompt(
            &state,
            &context.session_id,
            ImageInput {
                mime: mime.to_string(),
                bytes,
            },
            &prompt,
            &parent_sessions,
            region,
        )
        .await
        {
            Ok((description, usage)) => {
                // The vision call is a real LLM call — record its billed
                // tokens into the session stats (threaded via ToolContext)
                // so tool-time vision usage is not invisible.
                if let Some(ref tracker) = context.usage_tracker {
                    tracker.record_llm_usage(0, &usage);
                }
                let labelled = if prompt == VISION_DESCRIBE_DEFAULT_PROMPT && region.is_none() {
                    description
                } else {
                    let location = match region_str {
                        Some(s) => format!("{path} (region {s})"),
                        None => path.to_string(),
                    };
                    format!("{location}\n\n{description}")
                };
                Ok(ToolResult::success(labelled))
            }
            Err(e) => Ok(ToolResult::error(format!("Vision model call failed: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_read_only_are_stable() {
        let tool = VisualDescribeTool::new();
        assert_eq!(tool.name(), "visual_describe");
        assert!(tool.is_read_only(), "describe never mutates state");
    }

    #[test]
    fn parameters_require_path() {
        let tool = VisualDescribeTool::new();
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert_eq!(required[0], "path");
        assert!(params["properties"]["prompt"].is_object());
        assert!(params["properties"]["region"].is_object());
    }

    #[test]
    fn region_parsing_roundtrip() {
        let px = ImageRegion::parse("10,20,300,400").unwrap();
        assert_eq!(px.cache_fragment(), "px:10,20,300,400");
        let rel = ImageRegion::parse("10%,20%,30%,40%").unwrap();
        assert!(rel.cache_fragment().starts_with("rel:"));
    }

    #[test]
    fn unconfigured_vision_fails_with_guidance() {
        // validate_vision_config is shared with the transcription pipeline;
        // its guidance text is stable enough to assert on.
        let mut cfg = crate::core::config::AppConfig::default();
        cfg.vision.enabled = false;
        let err = validate_vision_config(&cfg).unwrap_err();
        assert!(err.to_string().contains("No vision model is configured"));
        assert!(err.to_string().contains("Settings → Vision model"));
    }
}
