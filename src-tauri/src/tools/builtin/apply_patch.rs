//! Apply patch tool — applies unified diff patches to files.
//!
//! Parses a unified diff format patch and applies it to the target file.
//! Supports hunks with context lines, added lines, and removed lines.
//! Includes fuzzy matching when context lines don't exactly match.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

/// Apply a unified diff patch to a file.
pub struct ApplyPatchTool;

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a file. The patch must be in standard \
        unified diff format with @@ hunk headers. Supports multiple hunks \
        per file; context lines must match exactly (case-insensitive \
        fallback on the full line)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to patch"
                },
                "patch": {
                    "type": "string",
                    "description": "The unified diff patch content"
                }
            },
            "required": ["path", "patch"]
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

        let patch = args
            .get("patch")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::core::error::AppError::ToolNotFound("missing 'patch'".into()))?;

        let file_path = resolve_path(&ctx.workspace, path);

        // Capture file state before modification for checkpoint/rewind, matching
        // the other mutating file tools (write_file/edit_file/search_replace).
        if let Some(ref tracker) = ctx.file_state_tracker {
            if let Some(ref workspace) = ctx.workspace {
                tracker.capture_file_state(&file_path, workspace).await;
            }
        }

        // Read current content
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to read {}: {}", path, e)));
            }
        };

        // Stale-edit guard: refuse to patch a file that changed on disk
        // since the agent last saw it (see tools/stale_edit.rs).
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

        // Parse and apply the patch
        match apply_patch(&content, patch) {
            Ok(new_content) => {
                if new_content == content {
                    return Ok(ToolResult::success(
                        "Patch applied — no changes (patch is already applied).",
                    ));
                }
                std::fs::write(&file_path, &new_content)
                    .map_err(crate::core::error::AppError::Io)?;
                // The agent's newest knowledge of this file is what it just wrote.
                crate::tools::stale_edit::record_seen(
                    &ctx.app,
                    &ctx.session_id,
                    ctx.workspace.as_deref(),
                    &file_path,
                    new_content.as_bytes(),
                )
                .await;
                let mut result = format!(
                    "Patch applied successfully to {} ({} hunks).",
                    path,
                    patch.matches("@@").count() / 2
                );
                if let Some(diff) = super::diff_preview::compute_diff(&content, &new_content) {
                    result.push_str(&format!("\n\n{diff}"));
                }
                Ok(ToolResult::success(result))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to apply patch: {}", e))),
        }
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

/// Parsed hunk from a unified diff.
struct Hunk {
    old_start: usize,
    lines: Vec<HunkLine>,
}

enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

/// Apply a unified diff patch to content.
fn apply_patch(content: &str, patch: &str) -> Result<String, String> {
    let hunks = parse_patch(patch)?;

    // Preserve the file's line endings. `str::lines()` + `join("\n")` would
    // strip \r\n and rewrite a CRLF file entirely to LF even for a one-line
    // patch; splitting on '\n' with a trailing-\r strip keeps the original
    // bytes intact.
    let newline = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = split_lines(content);

    // Each hunk header's line numbers refer to the ORIGINAL file. Every
    // applied hunk shifts the working buffer, so subsequent hunks must be
    // anchored at old_start + cumulative_shift (delta of lines added minus
    // removed by the hunks applied so far).
    let mut shift: isize = 0;
    for hunk in &hunks {
        shift += apply_hunk(&mut lines, hunk, shift)?;
    }

    Ok(lines.join(newline))
}

/// Split into lines, treating both "\n" and "\r\n" as terminators while
/// preserving empty trailing lines. Unlike `str::lines`, this does not drop a
/// trailing terminator, so blank lines at the end of a file survive a patch.
fn split_lines(content: &str) -> Vec<String> {
    content
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s).to_string())
        .collect()
}

/// Parse a unified diff patch into hunks.
fn parse_patch(patch: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
        if line.starts_with("@@") {
            if let Some(h) = current_hunk.take() {
                hunks.push(h);
            }
            let hunk = parse_hunk_header(line)?;
            current_hunk = Some(hunk);
        } else if let Some(ref mut hunk) = current_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(HunkLine::Add(rest.to_string()));
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(HunkLine::Remove(rest.to_string()));
            } else if line.starts_with(' ') || line.is_empty() {
                hunk.lines.push(HunkLine::Context(
                    line.strip_prefix(' ').unwrap_or("").to_string(),
                ));
            }
            // Skip header lines (---, +++, etc.)
        }
    }

    if let Some(h) = current_hunk {
        hunks.push(h);
    }

    if hunks.is_empty() {
        return Err("No hunks found in patch".to_string());
    }

    Ok(hunks)
}

/// Parse a @@ hunk header line.
fn parse_hunk_header(line: &str) -> Result<Hunk, String> {
    // Format: @@ -start,count +start,count @@
    let line = line.trim_start_matches("@@").trim();
    let parts: Vec<&str> = line.split("@@").collect();
    let header = parts[0].trim();

    let old_part = header
        .split_whitespace()
        .find(|s| s.starts_with('-'))
        .ok_or("Missing old file range in hunk header")?;
    let new_part = header
        .split_whitespace()
        .find(|s| s.starts_with('+'))
        .ok_or("Missing new file range in hunk header")?;

    let (old_start, _) = parse_range(&old_part[1..])?;
    let _ = parse_range(&new_part[1..])?;

    Ok(Hunk {
        old_start,
        lines: Vec::new(),
    })
}

/// Parse a range like "5,3" into (start, count).
fn parse_range(s: &str) -> Result<(usize, usize), String> {
    if let Some((start_str, count_str)) = s.split_once(',') {
        Ok((
            start_str
                .parse()
                .map_err(|_| format!("Invalid range start: {}", start_str))?,
            count_str
                .parse()
                .map_err(|_| format!("Invalid range count: {}", count_str))?,
        ))
    } else {
        Ok((s.parse().map_err(|_| format!("Invalid range: {}", s))?, 1))
    }
}

/// Apply a single hunk to the lines vector.
///
/// `shift` is the cumulative line delta from previously applied hunks (see
/// [`apply_patch`]); returns this hunk's own delta (adds − removes) so the
/// caller can anchor the next hunk correctly.
fn apply_hunk(lines: &mut Vec<String>, hunk: &Hunk, shift: isize) -> Result<isize, String> {
    let base = if hunk.old_start > 0 {
        hunk.old_start - 1
    } else {
        0
    };
    let shifted = base as isize + shift;
    if shifted < 0 {
        return Err(format!(
            "Hunk position drifted out of file at original line {}",
            hunk.old_start
        ));
    }
    let mut line_idx = shifted as usize;
    let hunk_lines = hunk.lines.iter();

    for hl in hunk_lines {
        if line_idx >= lines.len() {
            // We're past the end — only adds are valid
            match hl {
                HunkLine::Add(text) => {
                    lines.push(text.clone());
                    line_idx += 1;
                }
                HunkLine::Context(_) | HunkLine::Remove(_) => {
                    return Err(format!(
                        "Hunk extends past end of file at line {}",
                        line_idx + 1
                    ));
                }
            }
            continue;
        }

        match hl {
            HunkLine::Context(expected) => {
                let actual = &lines[line_idx];
                // Exact match first; only a case-insensitive match of the
                // FULL line is tolerated as fallback (never trim-based —
                // whitespace differences must not anchor a hunk to the
                // wrong position and silently corrupt the file).
                if actual != expected && !actual.eq_ignore_ascii_case(expected) {
                    return Err(format!(
                        "Context mismatch at line {}: expected '{}', got '{}'",
                        line_idx + 1,
                        expected,
                        actual
                    ));
                }
                line_idx += 1;
            }
            HunkLine::Remove(expected) => {
                let actual = &lines[line_idx];
                if actual != expected {
                    return Err(format!(
                        "Remove mismatch at line {}: expected '{}', got '{}'",
                        line_idx + 1,
                        expected,
                        actual
                    ));
                }
                lines.remove(line_idx);
            }
            HunkLine::Add(text) => {
                lines.insert(line_idx, text.clone());
                line_idx += 1;
            }
        }
    }

    let delta: isize = hunk
        .lines
        .iter()
        .map(|l| match l {
            HunkLine::Add(_) => 1,
            HunkLine::Remove(_) => -1,
            HunkLine::Context(_) => 0,
        })
        .sum();
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_hunk_header() {
        let hunk = parse_hunk_header("@@ -5,3 +5,4 @@").unwrap();
        assert_eq!(hunk.old_start, 5);
    }

    #[test]
    fn parse_single_line_hunk() {
        let hunk = parse_hunk_header("@@ -5 +5,2 @@").unwrap();
        assert_eq!(hunk.old_start, 5);
    }

    #[test]
    fn apply_simple_patch() {
        let content = "line1\nline2\nold line\nline4\nline5";
        let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,5 +1,5 @@\n line1\n line2\n-old line\n+new line\n line4\n line5";
        let result = apply_patch(content, patch).unwrap();
        assert!(result.contains("new line"));
        assert!(!result.contains("old line"));
    }

    #[test]
    fn apply_addition_patch() {
        let content = "a\nb\nc";
        let patch = "@@ -1,3 +1,4 @@\n a\n b\n c\n+d";
        let result = apply_patch(content, patch).unwrap();
        assert_eq!(result, "a\nb\nc\nd");
    }

    #[test]
    fn apply_removal_patch() {
        let content = "a\nb\nc";
        let patch = "@@ -1,3 +1,2 @@\n a\n-b\n c";
        let result = apply_patch(content, patch).unwrap();
        assert_eq!(result, "a\nc");
    }

    #[test]
    fn already_applied_patch_errors() {
        // When the patch was already applied, the remove line won't match.
        // The tool should return an error (which the caller surfaces to the LLM).
        let content = "a\nnew\nb";
        let patch = "@@ -1,3 +1,3 @@\n a\n-old\n+new\n b";
        let result = apply_patch(content, patch);
        assert!(result.is_err());
    }

    #[test]
    fn multi_hunk_offsets_recalculated_after_first_hunk() {
        // The second hunk's header references ORIGINAL line 4 ("d"), but the
        // first hunk already inserted a line above it. Without incremental
        // offset recalculation the second hunk anchors at the wrong line
        // (and would silently corrupt or error on the wrong content).
        let content = "a\nb\nc\nd\ne\nf";
        let patch = "@@ -1,2 +1,3 @@\n a\n+x\n b\n@@ -4,2 +4,1 @@\n-d\n e";
        let result = apply_patch(content, patch).unwrap();
        assert_eq!(result, "a\nx\nb\nc\ne\nf");
    }

    #[test]
    fn context_mismatch_whitespace_is_not_fuzzy_anchored() {
        // A context line with different indentation must NOT anchor the hunk
        // (old trim-based fuzzy matching silently accepted it and edited the
        // wrong location).
        let content = "a\n  indented\nc";
        let patch = "@@ -1,3 +1,3 @@\n a\n indented\n-c\n+d";
        let result = apply_patch(content, patch);
        assert!(result.is_err(), "indentation drift must be an error");
    }

    #[test]
    fn context_case_mismatch_falls_back_on_full_line() {
        // Exact match preferred; only a full-line case-insensitive match is
        // tolerated (whitespace differences are still rejected).
        let content = "a\nVERSION\nc";
        let patch = "@@ -1,3 +1,3 @@\n a\n version\n-c\n+d";
        let result = apply_patch(content, patch).unwrap();
        assert_eq!(result, "a\nVERSION\nd");
    }

    #[test]
    fn preserves_crlf_line_endings() {
        // A one-line patch must NOT rewrite the whole file's CRLF endings to LF.
        let content = "a\r\nb\r\nold line\r\nc";
        let patch = "@@ -1,4 +1,4 @@\n a\n b\n-old line\n+new line\n c";
        let result = apply_patch(content, patch).unwrap();
        assert_eq!(result, "a\r\nb\r\nnew line\r\nc");
    }

    #[test]
    fn preserves_trailing_blank_lines() {
        // Trailing blank lines must survive a patch (str::lines would drop them).
        let content = "a\nb\n\n";
        let patch = "@@ -1,2 +1,3 @@\n a\n b\n+c";
        let result = apply_patch(content, patch).unwrap();
        assert_eq!(result, "a\nb\nc\n\n");
    }
}
