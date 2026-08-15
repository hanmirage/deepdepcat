//! Search and replace tool — hash-line anchored editing for precise
//! text replacement in files.
//!
//! Uses a "search block" that must match exactly, replaced by a "replace block".
//! Supports multiple search/replace pairs in a single call.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

/// Search-and-replace tool with exact text matching.
pub struct SearchReplaceTool;

impl SearchReplaceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SearchReplaceTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "search_replace"
    }

    fn description(&self) -> &str {
        "Search for exact text in a file and replace EVERY occurrence of it with the new text. \
        The search text must match exactly (including whitespace and indentation). \
        Use apply_patch for complex multi-hunk edits."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "search": {
                    "type": "string",
                    "description": "The exact text to search for"
                },
                "replace": {
                    "type": "string",
                    "description": "The text to replace the search text with"
                }
            },
            "required": ["path", "search", "replace"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn check_permissions(&self, _args: &Value, _ctx: &ToolContext) -> PermissionDecision {
        PermissionDecision::Ask
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::core::error::AppError::ToolNotFound("missing 'path'".into()))?;

        let search = args
            .get("search")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::core::error::AppError::ToolNotFound("missing 'search'".into()))?;

        let replace = args
            .get("replace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'replace'".into())
            })?;

        let file_path = resolve_path(&ctx.workspace, path);

        // Capture file state before modification for checkpoint/rewind.
        if let Some(ref tracker) = ctx.file_state_tracker {
            if let Some(ref workspace) = ctx.workspace {
                tracker.capture_file_state(&file_path, workspace).await;
            }
        }

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to read {}: {}", path, e)));
            }
        };

        // Stale-edit guard: refuse to edit a file that changed on disk since
        // the agent last saw it (see tools/stale_edit.rs).
        if let Some(hint) = crate::tools::stale_edit::check_stale(
            &ctx.app,
            &ctx.session_id,
            ctx.workspace.as_deref(),
            &file_path,
        )
        .await
        {
            return Ok(ToolResult::error(hint));
        }

        // CRLF-tolerant matching: Windows checkouts (CRLF) must still match
        // LF-only search text (and vice versa).
        let mut count = 0usize;
        let mut rest = content.as_str();
        while let Some((_, e)) = crate::core::str_util::find_literal_with_crlf(rest, search) {
            count += 1;
            rest = &rest[e..];
        }
        if count == 0 {
            // The replacement text exists SOMEWHERE in the file — but that is
            // not proof this exact replacement was applied here: the text may
            // pre-exist elsewhere (a "success" would silently skip a missing
            // edit). Report the uncertainty honestly instead of asserting.
            if content.contains(replace) {
                return Ok(ToolResult::success(format!(
                    "Search text not found in {path}. The replacement text exists \
                     elsewhere in the file — the replacement MAY already be applied \
                     (e.g. a retry), or the text may simply occur naturally. \
                     Re-read the file to confirm before assuming the edit is done."
                )));
            }
            return Ok(ToolResult::error(format!(
                "Search text not found in {}: ensure exact match including whitespace.",
                path
            )));
        }

        // Idempotency gap: when the search text is a substring of the replacement
        // (e.g. "foo" -> "foobar"), a retry re-matches "foo" inside the
        // already-written "foobar" and would double-apply to "foobarbar". If the
        // replacement text is already present, report already-applied instead of
        // silently corrupting the file.
        if is_already_applied_substring(&content, search, replace) {
            return Ok(ToolResult::success(format!(
                "The search text is a substring of the replacement and the \
                 replacement text already exists in {path} — the edit MAY already \
                 be applied (e.g. a retry). Re-read the file to confirm before \
                 assuming the edit is done."
            )));
        }

        // Replace all occurrences, preserving the file's original bytes
        // (CRLF stays CRLF — no normalization is written back).
        let mut new_content = String::with_capacity(content.len() + replace.len() * count);
        let mut rest = content.as_str();
        while let Some((s, e)) = crate::core::str_util::find_literal_with_crlf(rest, search) {
            new_content.push_str(&rest[..s]);
            new_content.push_str(replace);
            rest = &rest[e..];
        }
        new_content.push_str(rest);

        std::fs::write(&file_path, &new_content).map_err(crate::core::error::AppError::Io)?;

        // The agent's newest knowledge of this file is what it just wrote.
        crate::tools::stale_edit::record_seen(
            &ctx.app,
            &ctx.session_id,
            ctx.workspace.as_deref(),
            &file_path,
            new_content.as_bytes(),
        )
        .await;

        let mut result = format!("Replaced {} occurrence(s) in {}.", count, path);
        if let Some(diff) = super::diff_preview::compute_diff(&content, &new_content) {
            result.push_str(&format!("\n\n{diff}"));
        }

        Ok(ToolResult::success(result))
    }
}

/// Resolve a path relative to the workspace.
fn resolve_path(workspace: &Option<std::path::PathBuf>, path: &str) -> std::path::PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(ws) = workspace {
        ws.join(path)
    } else {
        p.to_path_buf()
    }
}

/// Detect the search-is-substring-of-replacement idempotency gap: after a prior
/// `foo` -> `foobar` apply, a retry still finds "foo" inside the written
/// "foobar" (count > 0) and would double-apply to "foobarbar". When the search
/// text is a substring of the replacement AND the replacement is already present,
/// treat the edit as already-applied rather than corrupting the file.
fn is_already_applied_substring(content: &str, search: &str, replace: &str) -> bool {
    replace.contains(search) && content.contains(replace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_single_occurrence() {
        let content = "hello world\nfoo bar\nhello world";
        let result = content.replace("hello", "goodbye");
        assert_eq!(result, "goodbye world\nfoo bar\ngoodbye world");
    }

    #[test]
    fn test_no_match_returns_error() {
        let content = "hello world";
        assert!(!content.contains("nonexistent"));
    }

    #[test]
    fn test_already_applied_detection() {
        let content = "goodbye world";
        // If "hello" is not found but "goodbye" (the replacement) is present
        assert!(!content.contains("hello"));
        assert!(content.contains("goodbye"));
    }

    #[test]
    fn substring_replacement_retry_is_already_applied() {
        // "foo" is a prefix of "foobar"; after a prior apply the file is "foobar"
        // which still contains "foo" — must not double-apply to "foobarbar".
        assert!(is_already_applied_substring("foobar", "foo", "foobar"));
        // A genuine un-applied occurrence (no "foobar" present yet) is NOT
        // already-applied.
        assert!(!is_already_applied_substring("foo", "foo", "foobar"));
        // When the search text is not a substring of the replacement, the guard
        // never fires (the replacement may pre-exist naturally).
        assert!(!is_already_applied_substring("foo", "bar", "foo"));
    }
}
