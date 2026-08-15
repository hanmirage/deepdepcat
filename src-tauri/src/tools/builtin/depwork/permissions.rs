//! Depwork permission helpers — the low-friction policy for document
//! automation.
//!
//! Product posture: Depwork's job is *producing* documents. The permission
//! system protects pre-existing USER content, not the agent's own output.
//!
//! Decision table for a write target:
//! - Path does not exist → `Allow` (pure creation — the daily case)
//! - Path exists AND was produced earlier by this session → `Allow`
//!   (the agent editing its own draft — also the daily case)
//! - Path exists and was NOT produced by the session → `Ask`
//!   (touching a pre-existing user file — the case that deserves a prompt)
//!
//! `is_read_only()` classification stays the read-side gate; these helpers
//! implement the write-side gate shared by all Depwork file tools.

use crate::toolkit::{PermissionDecision, ToolContext};
use crate::bootstrap::AppState;
use std::path::{Path, PathBuf};
use tauri::Manager;

/// Resolve a Depwork output target the way `execute()` does:
/// workspace-relative → absolute, optional missing extension appended.
/// Pure — no AppHandle needed (the ToolContext only contributes workspace).
pub fn resolve_target(workspace: Option<&Path>, raw: &str, ext: Option<&str>) -> PathBuf {
    let mut path = crate::tools::builtin::resolve_path(workspace, raw);
    if let Some(e) = ext {
        if path.extension().is_none() {
            path.set_extension(e);
        }
    }
    path
}

/// Classify a write target per the decision table above.
pub fn write_target_decision(context: &ToolContext, target: &Path) -> PermissionDecision {
    if target_allowed(target.exists(), is_session_output(context, target)) {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Ask
    }
}

/// Pure decision: a target is prompt-free when it does not exist yet (pure
/// creation) or the session already produced it (own draft).
fn target_allowed(exists: bool, is_own_output: bool) -> bool {
    !exists || is_own_output
}

/// Whether `path` was produced earlier by this session.
pub fn is_session_output(context: &ToolContext, target: &Path) -> bool {
    let state = context.app.state::<AppState>();
    let guard = state
        .session_outputs
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = normalized(target);
    guard
        .get(&context.session_id)
        .map(|list| list.iter().any(|p| normalized(p) == key))
        .unwrap_or(false)
}

/// Record an output path as produced by this session. Call AFTER the
/// write succeeded.
pub fn record_output(context: &ToolContext, path: &Path) {
    let state = context.app.state::<AppState>();
    let mut guard = state
        .session_outputs
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = normalized(path);
    let list = guard.entry(context.session_id.clone()).or_default();
    if !list.iter().any(|p| normalized(p) == key) {
        list.push(path.to_path_buf());
    }
}

/// Case-insensitive absolute key for path matching (Windows drive letters
/// arrive in mixed case; canonicalize resolves `..` and symlinks).
fn normalized(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    abs.to_string_lossy().to_lowercase()
}

/// office_automate's read actions — document inspection never mutates.
pub fn is_office_read_action(action: &str) -> bool {
    matches!(
        action,
        "read" | "read_paragraphs" | "list_sheets" | "read_cells" | "read_cell" | "read_slides"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_read_action_classification() {
        for action in [
            "read",
            "read_paragraphs",
            "list_sheets",
            "read_cells",
            "read_cell",
            "read_slides",
        ] {
            assert!(is_office_read_action(action), "{action} must be read");
        }
        for action in [
            "replace",
            "save_as",
            "write_cell",
            "add_slide",
            "export_pdf",
            "type_text",
        ] {
            assert!(!is_office_read_action(action), "{action} must be write");
        }
    }

    #[test]
    fn normalized_keys_are_absolute_and_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.docx");
        std::fs::write(&file, b"x").unwrap();
        let a = normalized(&file);
        // Same file via a differently-cased relative spelling must collide
        // on case-insensitive platforms; on case-sensitive ones canonicalize
        // still collapses it to the same absolute path.
        let parent = dir.path().to_string_lossy().to_string();
        let upper = format!("{}/REPORT.DOCX", parent.replace('\\', "/"));
        let b = normalized(Path::new(&upper));
        assert_eq!(a, b, "same file must normalize identically");
    }

    #[test]
    fn target_allowed_decision_table() {
        // New file → allow; own output → allow; user file → ask.
        assert!(target_allowed(false, false), "new file never prompts");
        assert!(target_allowed(false, true), "new file never prompts");
        assert!(target_allowed(true, true), "own draft never prompts");
        assert!(!target_allowed(true, false), "user file must ask");
    }

    #[test]
    fn resolve_target_extends_and_resolves() {
        let p = resolve_target(None, "out/report", Some("docx"));
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("docx"));
        let p2 = resolve_target(None, "out/report.docx", Some("docx"));
        assert_eq!(p2.extension().and_then(|e| e.to_str()), Some("docx"));
        let p3 = resolve_target(None, "out/report.pdf", Some("docx"));
        assert_eq!(p3.extension().and_then(|e| e.to_str()), Some("pdf"));
    }
}
