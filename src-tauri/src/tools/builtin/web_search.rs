//! Web search tool — searches the web for information.
//!
//! Uses the Bing RSS output endpoint (`format=rss`) — free, no API key,
//! ad-free, and parseable without an HTML parser. Replaces the DuckDuckGo
//! instant-answer API, which is unreachable from some networks.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

/// Decode a standard-base64 string (Bing redirect `u=` parameter).
///
/// Decodes in groups of 4 characters, handling `=` padding, to avoid
/// accumulator overflow for inputs longer than 4 bytes.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut table = [0xFFu8; 256];
    for (i, c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        table[*c as usize] = i as u8;
    }
    let chars: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if chars.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(chars.len() / 4 * 3);
    let mut i = 0;
    while i + 4 <= chars.len() {
        if chars[i] == b'=' {
            break;
        }
        let mut val: u32 = 0;
        let mut count = 0;
        while count < 4 && i < chars.len() && chars[i] != b'=' {
            let v = table[chars[i] as usize];
            if v == 0xFF {
                return None;
            }
            val = (val << 6) | v as u32;
            count += 1;
            i += 1;
        }
        if count < 4 {
            // Final partial group: 2 chars → 1 byte, 3 chars → 2 bytes.
            if count >= 2 {
                out.push((val >> 4) as u8);
            }
            if count >= 3 {
                out.push((val >> 2) as u8);
            }
            break;
        }
        out.push((val >> 16) as u8);
        out.push((val >> 8) as u8);
        out.push(val as u8);
    }
    Some(out)
}

/// Resolve the real destination URL from a Bing redirect link.
///
/// RSS links sometimes point at `https://cn.bing.com/ck/a?...&u=a1aHR0cDovL...`
/// where `u` holds the base64-encoded target URL.
fn resolve_bing_link(raw: &str) -> String {
    if let Some(encoded) = raw.split('&').find_map(|part| part.strip_prefix("u=")) {
        let padded = format!("{}{}", encoded, "=".repeat((4 - encoded.len() % 4) % 4));
        if let Some(decoded) = base64_decode(&padded) {
            if let Ok(url) = String::from_utf8(decoded) {
                if url.starts_with("http") {
                    return url;
                }
            }
        }
    }
    raw.to_string()
}

/// Unescape common XML entities.
fn unescape_xml(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Extract the content between `open` and `close` tags inside `block`.
fn extract_tag<'a>(block: &'a str, open: &str, close: &str) -> &'a str {
    block
        .find(open)
        .and_then(|i| {
            let content = &block[i + open.len()..];
            content.find(close).map(|j| &content[..j])
        })
        .unwrap_or("")
}

/// Parse a Bing RSS response into (title, url, snippet) triples.
///
/// The RSS structure is stable enough for a targeted parser: each `<item>`
/// block carries `<title>`, `<link>` and `<description>`. Shared with the
/// depwork `research_search` web source.
pub(crate) fn parse_bing_rss(body: &str, max_results: usize) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<item>") {
        let after = &rest[start + "<item>".len()..];
        let end = after.find("</item>").unwrap_or(after.len());
        let block = &after[..end];
        rest = &after[end..];

        let title = extract_tag(block, "<title>", "</title>");
        let link = extract_tag(block, "<link>", "</link>");

        if title.trim().is_empty() && link.trim().is_empty() {
            continue;
        }

        let desc = extract_tag(block, "<description>", "</description>");
        let title = unescape_xml(title.trim()).trim().to_string();
        let link = resolve_bing_link(unescape_xml(link.trim()).trim());
        let desc = unescape_xml(desc.trim()).trim().to_string();

        results.push((title, link, desc));
        if results.len() >= max_results {
            break;
        }
    }
    results
}

/// Render an HTML snippet as plain text, stripping tags.
pub(crate) fn strip_html(snippet: &str) -> String {
    if snippet.is_empty() {
        return String::new();
    }
    html2text::from_read(snippet.as_bytes(), 80)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| snippet.to_string())
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns numbered results with titles, URLs, and snippets, \
         plus a References table. Cite sources in your reply using [n] markers."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results. Defaults to 5."
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    /// Search requests hit transient network failures — one automatic
    /// retry is safe because the operation is idempotent.
    fn is_retryable(&self, _error: &crate::core::error::AppError) -> bool {
        true
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'query'".into()))?;
        let max_results = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .unwrap_or(5) as usize;

        let url = format!(
            "https://cn.bing.com/search?q={}&format=rss",
            urlencoding::encode(query)
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            )
            .build()?;

        let response = client.get(&url).send().await;

        match response {
            Ok(resp) => {
                if !resp.status().is_success() {
                    return Ok(ToolResult::error(format!(
                        "Search failed: HTTP {}",
                        resp.status()
                    )));
                }

                let body = match resp.text().await {
                    Ok(b) => b,
                    Err(e) => {
                        return Ok(ToolResult::error(format!(
                            "Failed to read search response: {}",
                            e
                        )))
                    }
                };

                let parsed = parse_bing_rss(&body, max_results);
                if parsed.is_empty() {
                    return Ok(ToolResult::success(format!(
                        "No results found for: '{}'",
                        query
                    )));
                }

                // Numbered results + a references table so the model can
                // cite sources precisely ([n] markers) instead of echoing
                // bare URLs — attribution makes claims traceable.
                let mut results: Vec<String> = Vec::new();
                let mut references: Vec<String> = Vec::new();
                for (i, (title, link, snippet)) in parsed.into_iter().enumerate() {
                    let n = i + 1;
                    let snippet = strip_html(&snippet);
                    if snippet.is_empty() {
                        results.push(format!("[{}] {}\n   URL: {}", n, title, link));
                    } else {
                        results.push(format!(
                            "[{}] {}\n   URL: {}\n   {}",
                            n, title, link, snippet
                        ));
                    }
                    references.push(format!("[{}] {} — {}", n, title, link));
                }

                Ok(ToolResult::success(format!(
                    "Search results for '{}':\n\n{}\n\n## References\n{}\n\n\
                     Cite sources in your reply using the [n] markers above \
                     (e.g. \"per [1]\"). Do not invent citations that are not \
                     in the References list.",
                    query,
                    results.join("\n\n"),
                    references.join("\n")
                )))
            }
            Err(e) => Ok(ToolResult::error(format!("Search request failed: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_items() {
        let rss = r#"<rss version="2.0"><channel><title>Bing</title>
        <item><title>Rust 编程语言</title><link>https://www.rust-lang.org/</link>
        <description>Rust 官网</description></item>
        <item><title>Second</title><link>https://example.com/a?b=1&amp;c=2</link>
        <description><b>bold</b> text</description></item>
        </channel></rss>"#;
        let results = parse_bing_rss(rss, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "Rust 编程语言");
        assert_eq!(results[0].1, "https://www.rust-lang.org/");
        assert_eq!(results[1].1, "https://example.com/a?b=1&c=2");
    }

    #[test]
    fn respects_max_results() {
        let mut rss = String::from("<rss>");
        for i in 0..10 {
            rss.push_str(&format!(
                "<item><title>t{i}</title><link>https://example.com/{i}</link><description>d</description></item>"
            ));
        }
        rss.push_str("</rss>");
        assert_eq!(parse_bing_rss(&rss, 3).len(), 3);
    }

    #[test]
    fn resolves_redirect_links() {
        // base64("https://target.example/page") — Bing redirects carry the
        // real destination in the `u=` parameter.
        let encoded = "aHR0cHM6Ly90YXJnZXQuZXhhbXBsZS9wYWdl";
        let decoded = match base64_decode(encoded) {
            Some(d) => d,
            None => panic!("base64_decode returned None for {encoded:?}"),
        };
        assert_eq!(
            String::from_utf8(decoded).unwrap(),
            "https://target.example/page"
        );
        let raw = format!("https://cn.bing.com/ck/a?x=1&u={encoded}");
        assert_eq!(resolve_bing_link(&raw), "https://target.example/page");
        assert_eq!(
            resolve_bing_link("https://direct.example/x"),
            "https://direct.example/x"
        );
    }

    #[test]
    fn unescapes_entities() {
        assert_eq!(
            unescape_xml("a &amp; b &lt;c&gt; &#39;d&#39;"),
            "a & b <c> 'd'"
        );
    }

    #[test]
    fn parse_bing_rss_resolves_redirect_links() {
        let encoded = "aHR0cHM6Ly90YXJnZXQuZXhhbXBsZS9wYWdl";
        let rss = format!(
            r#"<item><title>T</title><link>https://cn.bing.com/ck/a?x=1&u={encoded}</link><description>d</description></item>"#
        );
        let results = parse_bing_rss(&rss, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "https://target.example/page");
    }

    #[test]
    fn parse_bing_rss_empty_body_yields_no_results() {
        assert!(parse_bing_rss("<rss><channel></channel></rss>", 5).is_empty());
        assert!(parse_bing_rss("", 5).is_empty());
    }

    #[test]
    fn strip_html_cleans_snippets() {
        let s = strip_html("<b>bold</b> &amp; <a href='x'>link text</a>");
        assert!(s.contains("bold"));
        assert!(s.contains("link text"));
        assert!(s.contains('&'));
        assert!(!s.contains('<'));
    }
}
