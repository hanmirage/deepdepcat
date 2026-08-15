//! dev_browser_open — hand a target to the in-app preview pane.
//!
//! Both local generated reports (HTML files) and live external URLs are
//! handed to the in-app preview pane (the rebuilt, Claude-Preview-style dev
//! browser): local HTML renders in a sandboxed srcdoc frame, external URLs
//! are embedded in an iframe. The system browser is only a MANUAL fallback
//! (the pane's header button) for sites that forbid embedding — it is never
//! opened automatically.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Emitter;

/// Payload handed to the dev-browser window over `dev-browser-open`.
#[derive(Debug, Clone, Serialize)]
pub struct DevBrowserOpenPayload {
    pub url: Option<String>,
    pub path: Option<String>,
}

/// Validate the tool args into a window payload.
pub fn validate_target(
    url: Option<&str>,
    path: Option<&str>,
) -> Result<DevBrowserOpenPayload, String> {
    match (url, path) {
        (Some(_), Some(_)) => Err("Provide only one of url or path".to_string()),
        (None, None) => Err("Provide url or path".to_string()),
        (Some(raw), None) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err("url is empty".to_string());
            }
            if trimmed.chars().any(char::is_whitespace) {
                return Err("url must not contain whitespace".to_string());
            }
            if trimmed.starts_with("//") {
                return Ok(DevBrowserOpenPayload {
                    url: Some(format!("https:{trimmed}")),
                    path: None,
                });
            }
            let normalized = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                trimmed.to_string()
            } else {
                // Schemeless input gets the https:// prefix, but a bare
                // scheme (javascript:, file:, data:) must be rejected —
                // opening those could reach local files or execute code.
                let Some((head, tail)) = trimmed.split_once(':') else {
                    return Ok(DevBrowserOpenPayload {
                        url: Some(format!("https://{trimmed}")),
                        path: None,
                    });
                };
                let port_end = tail.find('/').unwrap_or(tail.len());
                let port = &tail[..port_end];
                if port.is_ascii() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
                    format!("https://{trimmed}")
                } else {
                    return Err(format!(
                        "Unsupported URL scheme '{}' — only http and https are allowed",
                        head
                    ));
                }
            };
            Ok(DevBrowserOpenPayload {
                url: Some(normalized),
                path: None,
            })
        }
        (None, Some(raw)) => {
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                return Err("path must be absolute".to_string());
            }
            if !path.exists() {
                return Err(format!("path does not exist: {}", path.display()));
            }
            if !path.is_file() {
                return Err("path is not a file".to_string());
            }
            Ok(DevBrowserOpenPayload {
                url: None,
                path: Some(path.to_string_lossy().to_string()),
            })
        }
    }
}

/// Open the dev-browser window and load the given target.
pub struct DevBrowserOpenTool;

impl DevBrowserOpenTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DevBrowserOpenTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::All
    }

    fn name(&self) -> &str {
        "dev_browser_open"
    }

    fn description(&self) -> &str {
        "Preview a target in the app's in-app preview pane. Parameters: path \
         (absolute local file path of a generated HTML report/dashboard — \
         rendered in a sandboxed frame inside the app) OR url (http/https — \
         embedded in an in-app iframe; sites that forbid embedding offer a \
         manual open-in-system-browser fallback). Use this to show generated \
         HTML dashboards/reports or a live URL to the user inside the app."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "External URL (http/https) — embedded in the in-app preview pane."
                },
                "path": {
                    "type": "string",
                    "description": "Absolute path of a local HTML report to preview in the in-app preview pane."
                }
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let url = args.get("url").and_then(|v| v.as_str());
        let path = args.get("path").and_then(|v| v.as_str());
        let payload = validate_target(url, path)?;

        // Both local reports and external URLs are handed to the in-app
        // preview pane — the pane renders local HTML in a sandboxed frame and
        // embeds external URLs in an iframe. The system browser is never
        // opened automatically.
        let app = context.app.clone();
        let payload_clone = payload.clone();
        tauri::async_runtime::spawn(async move {
            // Let the frontend mount its listener before the one-shot emit.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let _ = app.emit("dev-browser-open", payload_clone);
        });

        let target = payload
            .url
            .unwrap_or_else(|| payload.path.unwrap_or_default());
        Ok(ToolResult::success(format!("Opened preview: {target}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_is_normalized_with_https_prefix() {
        let p = validate_target(Some("example.com/dash"), None).unwrap();
        assert_eq!(p.url.as_deref(), Some("https://example.com/dash"));
        assert_eq!(p.path, None);
    }

    #[test]
    fn unsafe_or_invalid_targets_are_rejected() {
        assert!(validate_target(Some("javascript:alert(1)"), None).is_err());
        assert!(validate_target(Some("foo bar"), None).is_err());
        assert!(validate_target(Some(""), None).is_err());
        assert!(validate_target(Some("a"), Some("C:\\x.html")).is_err());
        assert!(validate_target(None, None).is_err());
    }
}
