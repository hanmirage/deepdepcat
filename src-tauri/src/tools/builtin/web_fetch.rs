//! Web fetch tool — retrieves web page content as text/markdown.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct WebFetchTool {
    max_output_chars: usize,
}

/// Hard ceiling on fetched bytes, enforced DURING the download (streamed
/// read, not post-hoc truncation). Matches the Depwork fetch's cap.
const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;

impl WebFetchTool {
    pub fn new(max_output_chars: usize) -> Self {
        Self { max_output_chars }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch the content of a web page. Returns the page content as text (HTML tags stripped). Useful for reading documentation, APIs, and articles."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length in characters. Defaults to 10000."
                }
            },
            "required": ["url"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    /// Network fetches hit transient failures (DNS, TLS, timeouts) — one
    /// automatic retry is safe because the operation is idempotent.
    fn is_retryable(&self, _error: &crate::core::error::AppError) -> bool {
        true
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let url = args
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'url'".into()))?;
        let max_length = args
            .get("max_length")
            .and_then(|m| m.as_u64())
            .unwrap_or(10000) as usize;

        // SSRF guard: never fetch internal addresses, loopback, or cloud
        // metadata endpoints — the URL comes from the model and is
        // untrusted. Fails closed: an unsafe URL is an error, not a fetch.
        if let Err(reason) = crate::hooks::ssrf::validate_fetch_url(url) {
            return Ok(ToolResult::error(format!(
                "SSRF guard rejected URL: {reason}"
            )));
        }

        // Redirects are followed one hop at a time and EVERY hop is
        // re-validated — a public first hop must not bounce us to an
        // internal address.
        let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                return attempt.error("too many redirects");
            }
            match crate::hooks::ssrf::validate_fetch_url(attempt.url().as_str()) {
                Ok(()) => attempt.follow(),
                Err(reason) => {
                    attempt.error(format!("redirect target blocked by SSRF guard: {reason}"))
                }
            }
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("DeepDepCat/1.0")
            .redirect(redirect_policy)
            .build()?;

        let response = client.get(url).send().await;

        match response {
            Ok(mut resp) => {
                let status = resp.status();
                if !status.is_success() {
                    return Ok(ToolResult::error(format!(
                        "HTTP {}: {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown")
                    )));
                }

                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("text/plain")
                    .to_string();

                // Stream the body with a hard byte ceiling: a "limit" only
                // applies AFTER download, so an unbounded fetch would slurp
                // a multi-GB page into memory before discarding it. Reading
                // chunk-by-chunk keeps memory flat and fails fast past the
                // ceiling (same cap as the Depwork fetch).
                let mut body = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(|e| {
                    crate::core::error::AppError::Internal(format!(
                        "Failed to read response body: {e}"
                    ))
                })? {
                    if body.len() + chunk.len() > MAX_FETCH_BYTES {
                        return Ok(ToolResult::error(format!(
                            "Page exceeds size limit ({} bytes): {}",
                            MAX_FETCH_BYTES, url
                        )));
                    }
                    body.extend_from_slice(&chunk);
                }
                let body = String::from_utf8_lossy(&body).into_owned();

                let text = if content_type.contains("text/html") {
                    match html2text::from_read(body.as_bytes(), 80) {
                        Ok(text) => text,
                        Err(e) => {
                            tracing::warn!(error = %e, "HTML to text conversion failed");
                            body
                        }
                    }
                } else {
                    body
                };

                let limit = max_length.min(self.max_output_chars);
                let truncated = if text.len() > limit {
                    format!(
                        "{}\n\n...(content truncated, {} of {} chars)",
                        crate::core::str_util::truncate_at_char_boundary(&text, limit),
                        limit,
                        text.len()
                    )
                } else {
                    text
                };

                Ok(ToolResult::success(format!(
                    "URL: {}\nStatus: {}\nContent-Type: {}\n\n{}",
                    url, status, content_type, truncated
                )))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to fetch URL: {}", e))),
        }
    }
}
