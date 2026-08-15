//! Hook runtime environment expansion and secret redaction.
//!
//! Hook commands/prompts/URLs are configured once but executed many times.
//! Supporting `$VAR` / `${VAR}` expansion at execution time lets users
//! reference secrets and environment state (API keys, home dirs, session
//! context) without hardcoding them in the hook config file.
//!
//! Expansion rules:
//! - `$VAR` and `${VAR}` expand from the process environment.
//! - `$$` produces a literal `$`.
//! - Unknown variables are left verbatim (a typo must not silently corrupt
//!   a hook command — the shell can still diagnose it).
//!
//! Redaction rules (used for log/display surfaces, never for execution):
//! - URL userinfo (`scheme://user:pass@`) is masked.
//! - Query parameter values whose key contains a sensitive keyword
//!   (`token`, `key`, `secret`, `password`, `auth`, ...) are masked.

use std::collections::HashSet;

/// Expand `$VAR` / `${VAR}` placeholders from the process environment.
pub fn expand_env(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '$' => {
                if chars.peek() == Some(&'$') {
                    chars.next();
                    out.push('$');
                    continue;
                }
                let name = if chars.peek() == Some(&'{') {
                    chars.next();
                    let mut name = String::new();
                    for c2 in chars.by_ref() {
                        if c2 == '}' {
                            break;
                        }
                        name.push(c2);
                    }
                    name
                } else {
                    let mut name = String::new();
                    while let Some(&c2) = chars.peek() {
                        if c2.is_ascii_alphanumeric() || c2 == '_' {
                            name.push(c2);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    name
                };

                if name.is_empty() {
                    out.push('$');
                } else if let Ok(value) = std::env::var(&name) {
                    out.push_str(&value);
                } else {
                    out.push('$');
                    out.push_str(&name);
                }
            }
            other => out.push(other),
        }
    }

    out
}

/// Mask secrets in a string for display/log purposes.
///
/// Covers URL userinfo and sensitive query parameters. Non-URL text is
/// returned unchanged — redaction is intentionally conservative.
pub fn redact_sensitive(input: &str) -> String {
    // Pass 1: mask URL userinfo (scheme://user:pass@ → scheme://***@).
    let mut masked = input.to_string();
    if let Some(scheme_end) = input.find("://") {
        let after = scheme_end + 3;
        if let Some(at) = input[after..].find('@') {
            let abs_at = after + at;
            if abs_at > after {
                masked.replace_range(after..abs_at, "***");
            }
        }
    }

    // Pass 2: mask sensitive query parameter values (?token=abc&key=def).
    match masked.find('?') {
        None => masked,
        Some(q) => {
            let mut out = String::with_capacity(masked.len());
            out.push_str(&masked[..=q]);
            let params = &masked[q + 1..];
            let (params, fragment) = match params.find('#') {
                Some(h) => (&params[..h], Some(&params[h..])),
                None => (params, None),
            };
            for (i, param) in params.split('&').enumerate() {
                if i > 0 {
                    out.push('&');
                }
                out.push_str(&mask_param(param));
            }
            if let Some(f) = fragment {
                out.push_str(f);
            }
            out
        }
    }
}

/// Mask a single query parameter `key=value` if the key is sensitive.
fn mask_param(param: &str) -> String {
    match param.find('=') {
        Some(i) if is_sensitive_key(&param[..i]) => format!("{}***", &param[..=i]),
        _ => param.to_string(),
    }
}

/// Keywords that mark a query parameter as sensitive.
fn is_sensitive_key(key: &str) -> bool {
    const SENSITIVE_KEYWORDS: &[&str] = &[
        "token",
        "key",
        "secret",
        "password",
        "passwd",
        "pwd",
        "auth",
        "api_key",
        "apikey",
        "signature",
        "credential",
        "access_token",
        "refresh_token",
        "code",
        "session",
    ];

    let lower = key.to_ascii_lowercase();
    SENSITIVE_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Names of environment variables whose values must not be surfaced
/// verbatim in previews (used by the hook preview command).
pub fn sensitive_env_names() -> HashSet<String> {
    std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| {
            let lower = k.to_ascii_lowercase();
            lower.contains("token")
                || lower.contains("secret")
                || lower.contains("password")
                || lower.contains("credential")
        })
        .collect()
}

/// Expand env vars for a UI preview, masking values of sensitive
/// environment variables with `***`.
pub fn preview_expansion(input: &str) -> String {
    let expanded = expand_env(input);
    let mut out = expanded;
    for name in sensitive_env_names() {
        if let Ok(value) = std::env::var(&name) {
            if !value.is_empty() {
                out = out.replace(&value, "***");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_simple_var() {
        std::env::set_var("DDC_HOOK_TEST_VAR_A", "hello");
        assert_eq!(expand_env("echo $DDC_HOOK_TEST_VAR_A"), "echo hello");
    }

    #[test]
    fn expands_braced_var() {
        std::env::set_var("DDC_HOOK_TEST_VAR_B", "world");
        assert_eq!(expand_env("echo ${DDC_HOOK_TEST_VAR_B}!"), "echo world!");
    }

    #[test]
    fn double_dollar_escapes() {
        assert_eq!(expand_env("cost is $$5"), "cost is $5");
    }

    #[test]
    fn unknown_var_left_verbatim() {
        assert_eq!(
            expand_env("echo $DDC_UNKNOWN_12345"),
            "echo $DDC_UNKNOWN_12345"
        );
    }

    #[test]
    fn empty_braces_left_alone() {
        assert_eq!(expand_env("echo ${}"), "echo $");
    }

    #[test]
    fn adjacent_text_not_consumed() {
        std::env::set_var("DDC_HOOK_TEST_VAR_C", "x");
        assert_eq!(expand_env("$DDC_HOOK_TEST_VAR_C/foo"), "x/foo");
    }

    #[test]
    fn redacts_sensitive_query_params() {
        let url = "https://hook.example.com/cb?token=abc123&q=hello&api_key=xyz";
        let redacted = redact_sensitive(url);
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("xyz"));
        assert!(redacted.contains("q=hello"));
        assert!(redacted.contains("token=***"));
    }

    #[test]
    fn redacts_userinfo() {
        let url = "https://user:pass123@hook.example.com/cb";
        let redacted = redact_sensitive(url);
        assert!(!redacted.contains("pass123"));
        assert!(redacted.contains("***@hook.example.com"));
    }

    #[test]
    fn redaction_is_conservative_for_plain_text() {
        let text = "echo hello world";
        assert_eq!(redact_sensitive(text), text);
    }

    #[test]
    fn redacts_middle_param() {
        let url = "https://hook.example.com/cb?signature=deadbeef&next=1";
        let redacted = redact_sensitive(url);
        assert!(!redacted.contains("deadbeef"));
        assert!(redacted.contains("signature=***"));
        assert!(redacted.contains("next=1"));
    }

    #[test]
    fn redacts_last_param_without_trailing_ampersand() {
        let url = "https://h.example/x?token=sekrit";
        let redacted = redact_sensitive(url);
        assert_eq!(redacted, "https://h.example/x?token=***");
    }
}
