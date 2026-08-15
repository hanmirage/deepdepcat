//! web_open — open a URL in the system default browser (Depwork only).
//!
//! Detached launch: the tool returns immediately, the browser keeps running
//! independently. Used to hand off results, dashboards or reports for manual
//! review, and to drive flows that need a human in the loop.
//!
//! Example:
//! - web_open url="https://report.example.com/dashboard"

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Opens URLs in the default browser.
pub struct WebOpenTool;

impl WebOpenTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WebOpenTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "web_open"
    }

    fn description(&self) -> &str {
        "Open a URL in the system default browser (detached — returns immediately). \
         Only http/https/mailto URLs are accepted. Parameters: url (required). \
         Use for dashboard hand-off, manual review or logins; combine with \
         ui_automate for scripted browser sessions."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to open (https:// prefix optional; only http/https/mailto schemes are accepted)."
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let url = args
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| "Missing required parameter: url".to_string())?;
        let target = resolve_open_target(url)?;
        let target_clone = target.clone();
        let opened = tokio::task::spawn_blocking(move || open::that_detached(&target_clone))
            .await
            .map_err(|e| format!("open task panicked: {e}"))?
            .map_err(|e| format!("Failed to open {target}: {e}"))?;
        let _ = opened;
        Ok(ToolResult::success(format!(
            "Opened {target} in the default browser"
        )))
    }
}

/// Schemes allowed for browser hand-off. Arbitrary schemes (file:,
/// javascript:, data:, …) are rejected — opening them could reach local
/// files or execute code through the browser. Schemeless input gets the
/// `https://` prefix, and a `host:port` prefix ("localhost:3000",
/// "example.com:8080") is treated as schemeless, not as a scheme.
fn resolve_open_target(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("Missing required parameter: url".into());
    }
    let Some((head, tail)) = trimmed.split_once(':') else {
        return Ok(super::web_fetch::resolve_url(trimmed));
    };
    let scheme = head.to_ascii_lowercase();
    if matches!(scheme.as_str(), "http" | "https" | "mailto") {
        return Ok(trimmed.to_string());
    }
    // `host:port` inputs carry a colon but no scheme; only treat the prefix
    // as a port when it is all digits up to the next `/`.
    let port_end = tail.find('/').unwrap_or(tail.len());
    let port = &tail[..port_end];
    if port.is_ascii() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(super::web_fetch::resolve_url(trimmed));
    }
    Err(format!(
        "Unsupported URL scheme '{scheme}' — only http, https and mailto are allowed"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelisted_schemes_resolve() {
        assert_eq!(
            resolve_open_target("https://example.com/a").unwrap(),
            "https://example.com/a"
        );
        assert_eq!(
            resolve_open_target("http://example.com/a").unwrap(),
            "http://example.com/a"
        );
        assert_eq!(
            resolve_open_target("mailto:user@example.com").unwrap(),
            "mailto:user@example.com"
        );
        // Schemeless input gets the https prefix.
        assert_eq!(
            resolve_open_target("example.com/dash").unwrap(),
            "https://example.com/dash"
        );
        // `host:port` is schemeless, not a scheme.
        assert_eq!(
            resolve_open_target("localhost:3000").unwrap(),
            "https://localhost:3000"
        );
        assert_eq!(
            resolve_open_target("example.com:8080/x").unwrap(),
            "https://example.com:8080/x"
        );
    }

    #[test]
    fn unsafe_schemes_are_rejected() {
        for url in [
            "file:///C:/Windows/system32/cmd.exe",
            "javascript:alert(1)",
            "data:text/html,<script>1</script>",
            "C:\\Users\\me\\secret.txt",
            "ftp://example.com/x",
        ] {
            assert!(
                resolve_open_target(url).is_err(),
                "scheme must be rejected: {url}"
            );
        }
    }

    #[test]
    fn empty_url_is_rejected() {
        assert!(resolve_open_target("").is_err());
        assert!(resolve_open_target("   ").is_err());
    }
}
