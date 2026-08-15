//! Browser session identity, launch options and status.
//!
//! Sessions are keyed by a **profile** — the same key always maps to the
//! same `--user-data-dir` (cookies/logins/downloads). Agent conversations
//! derive their key from the conversation id so parallel conversations
//! never share login state; the frontend takeover browser uses `takeover`.

use crate::core::error::AppResult;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Profile key of the frontend takeover browser (visible, user-driven).
pub const DEFAULT_PROFILE: &str = "takeover";

/// The key an agent conversation uses for its own isolated browser.
pub fn session_profile_key(session_id: &str) -> String {
    format!("session-{}", session_id.trim())
}

/// How a browser should be launched.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Profile key — becomes the `browser-profiles/<key>` user-data dir.
    pub profile: String,
    /// `true` launches without a window (unattended batch automation).
    pub headless: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            profile: DEFAULT_PROFILE.to_string(),
            headless: false,
        }
    }
}

/// One running browser session (profile-keyed).
pub(crate) struct BrowserSession {
    pub(crate) profile: String,
    pub(crate) headless: bool,
    /// Isolated download dir — browser downloads never hit the user's
    /// personal Downloads folder.
    pub(crate) download_dir: PathBuf,
    /// DevTools HTTP port — used to re-list tabs via `GET /json`.
    pub(crate) port: u16,
    /// Browser-level ws — `Target.*` commands (create/close/activate tab).
    pub(crate) browser_ws_url: String,
    /// Page targets (tabs) in tab order, refreshed on demand.
    pub(crate) targets: Vec<crate::browser::launch::PageTarget>,
    /// The tab CDP operations currently address.
    pub(crate) current_target_id: Option<String>,
    pub(crate) child: Option<tokio::process::Child>,
    /// Pending user-takeover handoff, if any.
    pub(crate) takeover: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) takeover_reason: Option<String>,
}

/// Snapshot of a browser session for commands/frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserStatus {
    pub running: bool,
    pub url: Option<String>,
    pub title: Option<String>,
    pub awaiting_takeover: bool,
    pub takeover_reason: Option<String>,
    pub profile: Option<String>,
    pub headless: bool,
    pub download_dir: Option<String>,
}

/// The canonical "not running" status payload.
pub(crate) fn stopped_status() -> BrowserStatus {
    BrowserStatus {
        running: false,
        url: None,
        title: None,
        awaiting_takeover: false,
        takeover_reason: None,
        profile: None,
        headless: false,
        download_dir: None,
    }
}

/// Turn a user/agent-supplied profile name into a safe directory key.
///
/// Keeps `[A-Za-z0-9._-]`, collapses runs of other characters into `-`,
/// caps at 64 chars. Empty input errors — callers must never silently fall
/// back to a shared profile (that would break isolation).
pub fn sanitize_profile_key(raw: &str) -> AppResult<String> {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
        if out.len() >= 64 {
            break;
        }
    }
    if pending_dash && !out.is_empty() {
        out.push('-');
    }
    if out.is_empty() {
        return Err("browser profile must be non-empty (letters/digits/._- allowed)".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_is_prefixed_and_safe() {
        assert_eq!(session_profile_key("conv-123"), "session-conv-123");
        assert_eq!(
            sanitize_profile_key("session-conv-123").unwrap(),
            "session-conv-123"
        );
    }

    #[test]
    fn sanitize_keeps_safe_and_collapses_unsafe() {
        assert_eq!(sanitize_profile_key("abc").unwrap(), "abc");
        assert_eq!(sanitize_profile_key("a b/c").unwrap(), "a-b-c");
        assert_eq!(sanitize_profile_key("a..b").unwrap(), "a..b");
        assert_eq!(sanitize_profile_key("  leading").unwrap(), "leading");
    }

    #[test]
    fn sanitize_rejects_empty_and_all_unsafe() {
        assert!(sanitize_profile_key("").is_err());
        assert!(sanitize_profile_key("   ").is_err());
        assert!(sanitize_profile_key("中文").is_err());
    }

    #[test]
    fn sanitize_caps_length() {
        let long = "a".repeat(200);
        let key = sanitize_profile_key(&long).unwrap();
        assert!(key.len() <= 64);
    }

    #[test]
    fn stopped_status_is_not_running() {
        let s = stopped_status();
        assert!(!s.running);
        assert!(!s.awaiting_takeover);
        assert!(!s.headless);
        assert_eq!(s.profile, None);
        assert_eq!(s.download_dir, None);
    }
}
