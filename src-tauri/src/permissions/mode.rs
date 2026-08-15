//! Permission modes — control the overall security posture.
//!
//! Three runtime modes, canonical strings shared with the frontend
//! (`read_only | accept_edits | full_access` — see `types/permission.ts`):
//! - `ReadOnly` — read-only, no modifications (the planning posture)
//! - `AcceptEdits` (default) — auto-approves file-edit tools, prompts for
//!   dangerous operations (bash/network/sensitive)
//! - `FullAccess` — accepts all tool calls without asking
//!
//! Legacy spellings (`"plan"`, `"chat_only"`, `"manual"`, `"default"`,
//! `"bypass"`, `"auto"`, `"autoaccept"`, `"read-only"`, `"accept"`, …) still
//! parse so old config files, persisted sessions and clients keep working;
//! `as_str()` always emits the canonical three.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PermissionMode {
    /// Read-only mode — no write operations allowed. Covers the former
    /// `Plan` (read-only planning) and `ChatOnly` (no tools at all) modes.
    ReadOnly,
    /// Accept-edits mode — auto-approves file-edit tools, prompts for
    /// dangerous operations. The default posture.
    #[default]
    AcceptEdits,
    /// Full-access mode — accepts all tool calls without asking.
    /// Use with caution.
    FullAccess,
}

impl PermissionMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            // Read-only: the former "plan" and "chat_only" collapse here.
            "read_only" | "readonly" | "read-only" | "plan" | "chat_only" | "chat" | "chat-only" => {
                Self::ReadOnly
            }
            // Full access: the former "bypass" (and "auto") names.
            "full_access" | "full-access" | "bypass" | "auto" | "autoaccept" | "auto-accept" => {
                Self::FullAccess
            }
            // Accept edits is the default: "accept_edits", the retired
            // "manual" (ask-everything), and anything unrecognized.
            _ => Self::AcceptEdits,
        }
    }

    /// Canonical wire strings shared with the frontend. Any consumer that
    /// persists or sends a mode must use these; new aliases belong in
    /// [`PermissionMode::from_str`], not here.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::AcceptEdits => "accept_edits",
            Self::FullAccess => "full_access",
        }
    }

    /// Whether this mode auto-accepts all operations.
    pub fn auto_accepts(&self) -> bool {
        matches!(self, Self::FullAccess)
    }

    /// Whether this mode is read-only.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

// ── Durable mode persistence ─────────────────────────────────────────
// The user's selected mode survives app restarts. The frontend ALSO keeps
// its own copy (localStorage) and re-pushes it on mount; this file is the
// backend's authoritative store so mode changes made without the frontend
// (e.g. plan_mode tool, subagents) persist too.

/// File name of the persisted permission mode (inside the app data dir).
const MODE_FILE: &str = "permission_mode.json";

/// Persist the current permission mode (atomic write: tmp + rename).
pub fn persist_mode(app_data_dir: &std::path::Path, mode: PermissionMode) {
    let path = app_data_dir.join(MODE_FILE);
    let tmp = app_data_dir.join("permission_mode.json.tmp");
    if std::fs::write(&tmp, format!("\"{}\"", mode.as_str())).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Load the persisted permission mode. `None` when absent/corrupt — callers
/// fall back to the config default.
pub fn load_persisted_mode(app_data_dir: &std::path::Path) -> Option<PermissionMode> {
    let raw = std::fs::read_to_string(app_data_dir.join(MODE_FILE)).ok()?;
    let s: String = serde_json::from_str(&raw).ok()?;
    Some(PermissionMode::from_str(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_strings_round_trip() {
        // The three canonical wire strings (shared with the frontend) must
        // round-trip through from_str/as_str unchanged.
        for s in ["read_only", "accept_edits", "full_access"] {
            assert_eq!(
                PermissionMode::from_str(s).as_str(),
                s,
                "{s} must round-trip"
            );
        }
    }

    #[test]
    fn legacy_aliases_still_parse() {
        // Former "plan" and "chat_only" collapse into read-only.
        assert_eq!(PermissionMode::from_str("plan"), PermissionMode::ReadOnly);
        assert_eq!(PermissionMode::from_str("chat_only"), PermissionMode::ReadOnly);
        assert_eq!(PermissionMode::from_str("chat"), PermissionMode::ReadOnly);
        assert_eq!(PermissionMode::from_str("readonly"), PermissionMode::ReadOnly);
        assert_eq!(PermissionMode::from_str("read-only"), PermissionMode::ReadOnly);
        // Former "manual"/"default" (ask-everything) → the new default.
        assert_eq!(PermissionMode::from_str("manual"), PermissionMode::AcceptEdits);
        assert_eq!(PermissionMode::from_str("default"), PermissionMode::AcceptEdits);
        assert_eq!(PermissionMode::from_str("accept"), PermissionMode::AcceptEdits);
        assert_eq!(PermissionMode::from_str("accept-edits"), PermissionMode::AcceptEdits);
        // Former "bypass"/"auto" → full access.
        assert_eq!(PermissionMode::from_str("bypass"), PermissionMode::FullAccess);
        assert_eq!(PermissionMode::from_str("auto"), PermissionMode::FullAccess);
        assert_eq!(PermissionMode::from_str("autoaccept"), PermissionMode::FullAccess);
        assert_eq!(PermissionMode::from_str("auto-accept"), PermissionMode::FullAccess);
        // Garbage never escalates past the default (accept-edits).
        assert_eq!(PermissionMode::from_str("garbage"), PermissionMode::AcceptEdits);
        assert_eq!(PermissionMode::from_str(""), PermissionMode::AcceptEdits);
    }

    #[test]
    fn persist_and_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_persisted_mode(dir.path()).is_none());
        for mode in [
            PermissionMode::ReadOnly,
            PermissionMode::AcceptEdits,
            PermissionMode::FullAccess,
        ] {
            persist_mode(dir.path(), mode);
            assert_eq!(load_persisted_mode(dir.path()), Some(mode));
        }
        // Corrupt file → None (caller falls back to config default).
        std::fs::write(dir.path().join("permission_mode.json"), "not json").unwrap();
        assert!(load_persisted_mode(dir.path()).is_none());
    }

    #[test]
    fn read_only_and_full_access_classification() {
        assert!(PermissionMode::ReadOnly.is_read_only());
        assert!(!PermissionMode::AcceptEdits.is_read_only());
        assert!(!PermissionMode::FullAccess.is_read_only());
        assert!(PermissionMode::FullAccess.auto_accepts());
        assert!(!PermissionMode::AcceptEdits.auto_accepts());
        assert!(!PermissionMode::ReadOnly.auto_accepts());
    }
}
