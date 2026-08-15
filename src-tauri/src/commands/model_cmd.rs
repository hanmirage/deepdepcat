//! Model discovery commands — provider `/models` lists fetched natively.
//!
//! Provider model-list endpoints (OpenAI-compatible, Anthropic, Gemini) do
//! not send CORS headers, so a webview `fetch()` is always blocked. These
//! commands use native HTTP (reqwest), matching how chat requests already
//! work. URL construction is protocol-aware so the same base URL works with
//! or without the `/v1` prefix (relay stations / local servers / official
//! APIs), and the response body is returned raw — the frontend keeps the
//! single parser for all response shapes.

use std::time::Duration;

/// One HTTP request candidate: URL plus headers.
struct ModelsCandidate {
    url: String,
    headers: Vec<(&'static str, String)>,
}

/// Build protocol-aware model-list request candidates for a base URL.
///
/// Normalization rules:
/// - trailing slashes are stripped;
/// - a base that already ends in `/models` is used verbatim;
/// - OpenAI-compatible (`openai`/`responses`/`custom`) prefers `/v1/models`
///   and falls back to `/models` (some relays expose only the bare path);
/// - Anthropic uses `/v1/models` (or `/models` when the base already has `/v1`);
/// - Gemini uses `/v1beta/models?key=...` (or `/models` when the base has
///   `/v1beta`).
fn models_candidates(base_url: &str, api_key: &str, api_format: &str) -> Vec<ModelsCandidate> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    match api_format {
        "anthropic" => {
            let path = if base.ends_with("/v1") {
                "models"
            } else {
                "v1/models"
            };
            candidates.push(ModelsCandidate {
                url: format!("{base}/{path}"),
                headers: vec![
                    ("x-api-key", api_key.to_string()),
                    ("anthropic-version", "2023-06-01".to_string()),
                ],
            });
        }
        "gemini" => {
            let query = format!("key={}", percent_encode(api_key));
            if base.ends_with("/v1beta") {
                candidates.push(ModelsCandidate {
                    url: format!("{base}/models?{query}"),
                    headers: Vec::new(),
                });
            } else {
                candidates.push(ModelsCandidate {
                    url: format!("{base}/v1beta/models?{query}"),
                    headers: Vec::new(),
                });
                candidates.push(ModelsCandidate {
                    url: format!("{base}/v1/models?{query}"),
                    headers: Vec::new(),
                });
            }
        }
        _ => {
            let auth = if api_key.is_empty() {
                Vec::new()
            } else {
                vec![("Authorization", format!("Bearer {api_key}"))]
            };
            if base.ends_with("/models") {
                candidates.push(ModelsCandidate {
                    url: base.to_string(),
                    headers: auth,
                });
            } else if base.ends_with("/v1") {
                candidates.push(ModelsCandidate {
                    url: format!("{base}/models"),
                    headers: auth,
                });
            } else {
                candidates.push(ModelsCandidate {
                    url: format!("{base}/v1/models"),
                    headers: auth.clone(),
                });
                candidates.push(ModelsCandidate {
                    url: format!("{base}/models"),
                    headers: auth,
                });
            }
        }
    }
    candidates
}

/// Percent-encode a value for a URL query string (RFC 3986 unreserved set).
fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Fetch a provider's model list over native HTTP (no CORS).
///
/// Tries each normalized URL candidate in order; the first successful
/// response is returned as raw JSON. Auth failures (401/403) abort instead
/// of falling through to the next candidate. Non-2xx responses are returned
/// as readable errors for the settings UI.
#[tauri::command]
pub async fn fetch_provider_models(
    base_url: String,
    api_key: String,
    api_format: String,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let candidates = models_candidates(&base_url, &api_key, &api_format);
    if candidates.is_empty() {
        return Err("base_url is empty".to_string());
    }

    let mut last_error = String::new();
    for candidate in candidates {
        let mut req = client.get(&candidate.url);
        for (name, value) in &candidate.headers {
            req = req.header(*name, value);
        }

        let resp = match req.send().await {
            Ok(resp) => resp,
            Err(e) => {
                last_error = format!("{}: {e}", candidate.url);
                continue;
            }
        };

        if resp.status().is_success() {
            return resp
                .json::<serde_json::Value>()
                .await
                .map_err(|e| format!("Failed to parse model list: {e}"));
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(format!("HTTP {status}: {body}"));
        }
        last_error = format!("{}: HTTP {status} {body}", candidate.url);
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(candidates: &[ModelsCandidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.url.as_str()).collect()
    }

    #[test]
    fn openai_prefers_v1_models_without_v1_base() {
        let candidates = models_candidates("https://api.example.com", "sk-1", "openai");
        assert_eq!(
            urls(&candidates),
            vec![
                "https://api.example.com/v1/models",
                "https://api.example.com/models",
            ]
        );
        assert_eq!(
            candidates[0].headers[0],
            ("Authorization", "Bearer sk-1".to_string())
        );
    }

    #[test]
    fn openai_with_v1_base_uses_models_once() {
        let candidates = models_candidates("https://api.example.com/v1/", "sk-1", "responses");
        assert_eq!(urls(&candidates), vec!["https://api.example.com/v1/models"]);
    }

    #[test]
    fn openai_without_key_sends_no_auth_header() {
        let candidates = models_candidates("https://api.example.com/v1", "", "custom");
        assert!(candidates[0].headers.is_empty());
    }

    #[test]
    fn base_already_ending_in_models_is_used_verbatim() {
        let candidates = models_candidates("https://relay.example.com/v1/models", "sk-1", "openai");
        assert_eq!(
            urls(&candidates),
            vec!["https://relay.example.com/v1/models"]
        );
    }

    #[test]
    fn anthropic_uses_v1_models_or_dedupes_existing_v1() {
        let candidates = models_candidates("https://api.anthropic.com", "sk-ant", "anthropic");
        assert_eq!(
            urls(&candidates),
            vec!["https://api.anthropic.com/v1/models"]
        );
        assert_eq!(candidates[0].headers[0].0, "x-api-key");
        assert_eq!(candidates[0].headers[1].0, "anthropic-version");

        let deduped = models_candidates("https://api.anthropic.com/v1", "sk-ant", "anthropic");
        assert_eq!(urls(&deduped), vec!["https://api.anthropic.com/v1/models"]);
    }

    #[test]
    fn gemini_uses_v1beta_and_key_query() {
        let candidates = models_candidates(
            "https://generativelanguage.googleapis.com",
            "k ey/1",
            "gemini",
        );
        assert_eq!(
            urls(&candidates),
            vec![
                "https://generativelanguage.googleapis.com/v1beta/models?key=k%20ey%2F1",
                "https://generativelanguage.googleapis.com/v1/models?key=k%20ey%2F1",
            ]
        );

        let deduped = models_candidates(
            "https://generativelanguage.googleapis.com/v1beta",
            "k",
            "gemini",
        );
        assert_eq!(
            urls(&deduped),
            vec!["https://generativelanguage.googleapis.com/v1beta/models?key=k"]
        );
    }

    #[test]
    fn empty_base_yields_no_candidates() {
        assert!(models_candidates("", "k", "openai").is_empty());
        assert!(models_candidates("   /", "k", "anthropic").is_empty());
    }
}
