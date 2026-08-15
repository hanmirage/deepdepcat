//! web_fetch — fetch a web page and extract its readable text (Depwork only).
//!
//! Grabs the page with a browser-ish user agent, extracts the `<title>` and
//! converts the body to plain text. Used for web research, data gathering and
//! multi-page information aggregation — no browser automation required.
//!
//! Example:
//! - web_fetch url="https://example.com/news" max_chars=8000

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

const DEFAULT_UA: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) ",
    "AppleWebKit/537.36 (KHTML, like Gecko) ",
    "Chrome/126.0 Safari/537.36"
);
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Lightweight reachability probe — shorter than a full fetch.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// Redirects are followed one hop at a time and EVERY hop is re-validated —
/// a public first hop must not bounce us to an internal address. Shared by
/// the full fetch and the lightweight reachability probe.
fn ssrf_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > 5 {
            return attempt.error("too many redirects");
        }
        match crate::hooks::ssrf::validate_fetch_url(attempt.url().as_str()) {
            Ok(()) => attempt.follow(),
            Err(reason) => {
                attempt.error(format!("redirect target blocked by SSRF guard: {reason}"))
            }
        }
    })
}

/// Web page fetcher.
pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

/// Normalize a URL: add `https://` when no scheme is present.
pub fn resolve_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// Extract the `<title>` element content (case-insensitive, handles
/// newlines/whitespace inside the tag).
pub fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title").and_then(|i| {
        let after = &html[i + 6..];
        after.find('>').map(|j| i + 6 + j + 1)
    });
    let end = start.and_then(|s| lower[s..].find("</title>").map(|j| s + j));
    match (start, end) {
        (Some(s), Some(e)) if e > s => html[s..e].trim().to_string(),
        _ => String::new(),
    }
}

/// Convert raw HTML response into readable text: title + body (truncated).
pub fn html_to_text(raw: &[u8], max_chars: usize) -> String {
    let body = html2text::from_read(raw, 800).unwrap_or_else(|_| "".to_string());
    truncate_text(&body, max_chars)
}

pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n… [truncated]");
    out
}

/// Fetch a web page and return `(resolved_url, title, body_text)`.
///
/// Body is converted to readable text and truncated to `max_chars`. Shared
/// by `web_fetch_depwork` (returns formatted text) and `research_clip`
/// (stores the page into the 资料夹) so both paths get identical SSRF
/// guards, redirect re-validation and error classification.
pub(crate) async fn fetch_web_page(
    url: &str,
    max_chars: usize,
) -> AppResult<(String, String, String)> {
    let target = resolve_url(url);
    if target.is_empty() {
        return Err("Missing required parameter: url".into());
    }

    // SSRF guard (shared with the code-side fetch): never reach internal
    // addresses, loopback, or cloud metadata endpoints — the URL comes from
    // the model and is untrusted. Fails closed: an unsafe URL is an error.
    if let Err(reason) = crate::hooks::ssrf::validate_fetch_url(&target) {
        return Err(format!("SSRF guard rejected URL: {reason}").into());
    }

    // Redirects are followed one hop at a time and EVERY hop is re-validated
    // — a public first hop must not bounce us to an internal address.
    let redirect_policy = ssrf_redirect_policy();

    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(DEFAULT_UA)
        .redirect(redirect_policy)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let resp = client.get(&target).send().await.map_err(|e| {
        // The raw reqwest error ("timeout", TLS errors, DNS/connect) is
        // terse and loses context — classify it for the model so it can
        // react (retry, switch site, or report the real cause).
        let hint = if e.is_timeout() {
            " (timeout: the site is slow or unreachable — retry once, or try another source)"
        } else if e.is_connect() {
            " (connection failed: check DNS/network or the URL)"
        } else if e.is_request() {
            " (request error: bad URL or TLS/certificate issue)"
        } else {
            ""
        };
        format!("Failed to fetch {target}: {e}{hint}")
    })?;
    if !resp.status().is_success() {
        // Non-2xx is usually an anti-bot block (403/429) or a missing page —
        // tell the model which, so it can pick a different source instead of
        // blindly retrying the same URL.
        let hint = match resp.status().as_u16() {
            401 | 403 | 429 => " (likely blocked by the site's anti-bot protection)",
            404 => " (page not found — check the URL)",
            5..=599 => " (server error — site may be down)",
            _ => "",
        };
        return Err(format!("HTTP {} for {target}{hint}", resp.status()).into());
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "Page exceeds size limit ({} bytes > {MAX_BYTES}): {target}",
            bytes.len()
        )
        .into());
    }
    let (html, _) = encoding_rs::UTF_8.decode_without_bom_handling(&bytes);
    let title = extract_title(&html);
    let body = html_to_text(bytes.as_ref(), max_chars);
    Ok((target, title, body))
}

/// Reachability verdict for a URL — used to reject dead links at the source
/// (research_save) without over-rejecting real sources behind anti-bot walls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UrlVerdict {
    /// 2xx/3xx — the URL is alive.
    Reachable,
    /// 404/410 — the page is explicitly gone.
    NotFound,
    /// DNS failure / connection refused — the URL cannot be reached at all.
    Unreachable,
    /// 403/429/5xx — likely blocked or temporarily down; may still be real.
    Blocked,
    /// Request timed out — network is slow or unreachable right now.
    TimedOut,
}

/// Map an HTTP status code to a verdict (used for the GET fallback too).
pub(crate) fn classify_status(code: u16) -> UrlVerdict {
    match code {
        200..=399 => UrlVerdict::Reachable,
        404 | 410 => UrlVerdict::NotFound,
        _ => UrlVerdict::Blocked,
    }
}

fn classify_error(e: &reqwest::Error) -> UrlVerdict {
    if e.is_timeout() {
        UrlVerdict::TimedOut
    } else {
        UrlVerdict::Unreachable
    }
}

/// Probe whether a URL is reachable without downloading the page. SSRF-safe
/// (initial URL + every redirect hop re-validated). HEAD first — lightweight;
/// any non-2xx HEAD falls back to a range-limited GET so a HEAD quirk (405,
/// server-drops-HEAD) never causes a false rejection.
pub(crate) async fn verify_url_reachable(url: &str) -> AppResult<UrlVerdict> {
    let target = resolve_url(url);
    if let Err(reason) = crate::hooks::ssrf::validate_fetch_url(&target) {
        return Err(format!("SSRF guard rejected URL: {reason}").into());
    }
    let client = reqwest::Client::builder()
        .timeout(VERIFY_TIMEOUT)
        .user_agent(DEFAULT_UA)
        .redirect(ssrf_redirect_policy())
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let head = client.head(&target).send().await;
    match head {
        Ok(r) if r.status().is_success() => Ok(UrlVerdict::Reachable),
        Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND
            || r.status() == reqwest::StatusCode::GONE =>
        {
            Ok(UrlVerdict::NotFound)
        }
        Ok(_) => {
            // Non-2xx HEAD (405 / 403 / 5xx / …) — trust a range-limited GET.
            let get = client
                .get(&target)
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send()
                .await;
            match get {
                Ok(g) => Ok(classify_status(g.status().as_u16())),
                Err(e) => Ok(classify_error(&e)),
            }
        }
        Err(e) => Ok(classify_error(&e)),
    }
}

async fn fetch_web(url: &str, max_chars: usize) -> AppResult<String> {
    let (target, title, body) = fetch_web_page(url, max_chars).await?;
    let mut out = String::new();
    if !title.is_empty() {
        out.push_str(&format!("# {title}\n\n"));
    }
    out.push_str(&format!(
        "Fetched {target} ({} chars)\n\n",
        body.chars().count()
    ));
    out.push_str(&body);
    Ok(out)
}

#[async_trait]
impl Tool for WebFetchTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        // Distinct name from the code-workspace `web_fetch` so the two
        // implementations coexist (same-name registration overwrites).
        // Depwork's variant keeps its browser-UA/title/error-classification
        // behavior; the code variant (with SSRF guards) is restored in the
        // code workspace.
        "web_fetch_depwork"
    }

    fn description(&self) -> &str {
        "Fetch a web page and return its title plus readable body text. \
         Parameters: url (required, http(s) or bare domain), \
         max_chars (optional, default 20000, truncates the body). \
         Good for research, data extraction and summarizing online documents. \
         Does not interact with the browser — plain HTTP fetch."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Page URL (https:// prefix optional)."
                },
                "max_chars": {
                    "type": "number",
                    "description": "Maximum body characters to return (default 20000)."
                }
            },
            "required": ["url"]
        })
    }

    /// Pure read — never prompts.
    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let url = args
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| "Missing required parameter: url".to_string())?
            .to_string();
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(20_000)
            .clamp(500, 100_000) as usize;
        let out = fetch_web(&url, max_chars).await?;
        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_url_adds_https() {
        assert_eq!(resolve_url("example.com/a"), "https://example.com/a");
        assert_eq!(resolve_url("http://example.com"), "http://example.com");
        assert_eq!(resolve_url("  "), "");
    }

    #[test]
    fn title_extraction_handles_attributes_and_case() {
        let html = "<HTML><HEAD><TITLE Foo=\"bar\">  My &amp; Page </TITLE></HEAD></HTML>";
        assert_eq!(extract_title(html), "My &amp; Page");
        assert_eq!(extract_title("<p>no title</p>"), "");
    }

    #[test]
    fn truncate_limits_chars() {
        assert_eq!(truncate_text("hello", 10), "hello");
        let out = truncate_text("hello world", 5);
        assert!(out.starts_with("hello"));
        assert!(out.contains("truncated"));
    }

    #[test]
    fn html_conversion_extracts_body_text() {
        let html = b"<html><body><h1>Title</h1><p>Hello <b>world</b></p></body></html>";
        let text = html_to_text(html, 1000);
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn classify_status_maps_codes_to_verdicts() {
        assert_eq!(classify_status(200), UrlVerdict::Reachable);
        assert_eq!(classify_status(301), UrlVerdict::Reachable);
        assert_eq!(classify_status(404), UrlVerdict::NotFound);
        assert_eq!(classify_status(410), UrlVerdict::NotFound);
        assert_eq!(classify_status(403), UrlVerdict::Blocked);
        assert_eq!(classify_status(429), UrlVerdict::Blocked);
        assert_eq!(classify_status(500), UrlVerdict::Blocked);
    }

    #[tokio::test]
    async fn verify_rejects_loopback_and_internal_targets() {
        for target in [
            "http://127.0.0.1:9/",
            "http://169.254.169.254/latest/meta-data",
            "http://192.168.0.10/x",
        ] {
            let err = verify_url_reachable(target).await.unwrap_err();
            assert!(
                err.to_string().contains("SSRF"),
                "expected SSRF rejection for {target}, got: {err}"
            );
        }
    }

    #[tokio::test]
    async fn fetch_rejects_loopback_and_internal_targets() {
        // SSRF guard: the fetch must refuse local/private targets outright
        // instead of connecting to them (a local server test would now
        // prove the opposite — the guard is what we assert).
        for target in [
            "http://127.0.0.1:9/",
            "http://169.254.169.254/latest/meta-data",
            "http://192.168.0.10/x",
        ] {
            let err = fetch_web(target, 2000).await.unwrap_err();
            assert!(
                err.to_string().contains("SSRF"),
                "expected SSRF rejection for {target}, got: {err}"
            );
        }
    }
}
