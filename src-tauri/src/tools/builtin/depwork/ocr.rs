//! ocr_image — extract text from images with Tesseract (Depwork only).
//!
//! Runs the `tesseract` binary when installed and returns the recognized
//! text. Chinese support via the `chi_sim` language pack. When Tesseract is
//! missing the tool returns a clear installation hint.
//!
//! Examples:
//! - ocr_image image="scan.png" lang="chi_sim"
//! - ocr_image image="screenshot.jpg" lang="eng"

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Build the tesseract CLI arguments. Pure — unit-testable.
pub fn build_ocr_args(image: &str, out_base: &str, lang: &str) -> Vec<String> {
    vec![
        image.to_string(),
        out_base.to_string(),
        "-l".to_string(),
        lang.to_string(),
        "--psm".to_string(),
        "3".to_string(),
    ]
}

/// Clean recognized text: collapse blank lines, trim, cap length.
pub fn clean_ocr_text(raw: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for line in raw.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let mut out = out.trim_end().to_string();
    if out.chars().count() > max_chars {
        let mut trimmed: String = out.chars().take(max_chars).collect();
        trimmed.push_str("\n… [truncated]");
        out = trimmed;
    }
    out
}

/// Find tesseract on PATH (exposed for tests).
pub fn find_tesseract() -> Option<PathBuf> {
    super::media::find_binary("tesseract")
}

/// OCR tool.
pub struct OcrImageTool;

impl OcrImageTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for OcrImageTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "ocr_image"
    }

    fn description(&self) -> &str {
        "Extract text from an image (screenshots, scans, photos of documents) \
         using Tesseract OCR. Parameters: image (required, path to png/jpg/...), \
         lang (optional, default \"eng\"; \"chi_sim\" for simplified Chinese, \
         \"eng+chi_sim\" for both). Requires Tesseract on PATH \
         (winget install tesseract / scoop install tesseract)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "image": { "type": "string", "description": "Image file path." },
                "lang": {
                    "type": "string",
                    "description": "OCR language(s): eng, chi_sim, eng+chi_sim (default eng)."
                }
            },
            "required": ["image"]
        })
    }

    /// Pure read — never prompts.
    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let image = args
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: image".to_string())?;
        let lang = args
            .get("lang")
            .and_then(|v| v.as_str())
            .unwrap_or("eng")
            .to_string();
        let image_path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), image);
        if !image_path.is_file() {
            return Err(format!("File not found: {}", image_path.display()).into());
        }
        let bin = find_tesseract().ok_or_else(|| {
            "tesseract not found on PATH. Install it first, e.g. \
             `winget install UB-Mannheim.TesseractOCR`, then restart the app."
                .to_string()
        })?;
        let image_str = image_path.to_string_lossy().to_string();
        let out = tokio::task::spawn_blocking(move || {
            let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
            let out_base = dir.path().join("ocr").to_string_lossy().to_string();
            let args = build_ocr_args(&image_str, &out_base, &lang);
            let output = std::process::Command::new(&bin)
                .args(&args)
                .output()
                .map_err(|e| format!("Failed to run tesseract: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                return Err(format!(
                    "tesseract failed (is language pack '{lang}' installed?): {stderr}"
                ));
            }
            let txt_path = format!("{out_base}.txt");
            let raw = std::fs::read_to_string(&txt_path)
                .map_err(|e| format!("No OCR output produced: {e}"))?;
            let text = clean_ocr_text(&raw, 10_000);
            if text.is_empty() {
                return Ok("No text recognized in the image".to_string());
            }
            Ok(format!(
                "Recognized text ({} chars):\n{text}",
                text.chars().count()
            ))
        })
        .await
        .map_err(|e| format!("ocr task panicked: {e}"))??;
        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_args_are_ordered_correctly() {
        let args = build_ocr_args("img.png", "out_base", "chi_sim");
        assert_eq!(
            args,
            vec!["img.png", "out_base", "-l", "chi_sim", "--psm", "3"]
        );
    }

    #[test]
    fn clean_ocr_collapses_blank_lines() {
        let raw = "Hello\n\n\nWorld\n\n  \n";
        assert_eq!(clean_ocr_text(raw, 1000), "Hello\nWorld");
    }

    #[test]
    fn clean_ocr_truncates_long_text() {
        let raw = "x".repeat(5000);
        let out = clean_ocr_text(&raw, 100);
        assert!(out.starts_with("xxx"));
        assert!(out.contains("truncated"));
        assert!(out.chars().count() <= 120);
    }

    #[test]
    fn clean_ocr_keeps_chinese_text() {
        let raw = "这是中文内容\n第二行\n";
        let out = clean_ocr_text(raw, 1000);
        assert!(out.contains("这是中文内容"));
        assert!(out.contains("第二行"));
    }
}
