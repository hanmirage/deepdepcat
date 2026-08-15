//! Edit file tool — search-and-replace editing within a file.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct EditFileTool;

impl EditFileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing specific text. Finds the old text and replaces it with the new text. The old text must match exactly (including whitespace)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit. Relative paths resolve against the workspace root."
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The new text to replace it with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'path'".into()))?;
        let old_text = args
            .get("old_text")
            .and_then(|t| t.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'old_text'".into()))?;
        let new_text = args
            .get("new_text")
            .and_then(|t| t.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'new_text'".into()))?;

        let file_path = super::resolve_path(context.workspace.as_deref(), path);

        // Capture file state before modification for checkpoint/rewind.
        if let Some(ref tracker) = context.file_state_tracker {
            if let Some(ref workspace) = context.workspace {
                tracker.capture_file_state(&file_path, workspace).await;
            }
        }

        let content =
            std::fs::read_to_string(&file_path).map_err(crate::core::error::AppError::Io)?;

        // Stale-edit guard: the file changed on disk since the agent last
        // saw it — the model may hold assumptions about a state that no
        // longer exists (see tools/stale_edit.rs).
        if let Some(hint) = crate::tools::stale_edit::check_stale(
            &context.app,
            &context.session_id,
            context.workspace.as_deref(),
            &file_path,
        )
        .await
        {
            return Ok(ToolResult::error(hint));
        }

        // CRLF-tolerant match: files checked out with CRLF must still match
        // an LF-only old_text (and vice versa). Exact-match-only broke on
        // Windows checkouts ("Text not found" while the text is visible).
        let Some((start, end)) = crate::core::str_util::find_literal_with_crlf(&content, old_text)
        else {
            return Ok(ToolResult::error(format!(
                "Text not found in file: '{}'",
                old_text.chars().take(80).collect::<String>()
            )));
        };

        // Ensure the match is unique even with the CRLF-tolerant scan.
        let rest_after = &content[end..];
        if crate::core::str_util::find_literal_with_crlf(rest_after, old_text).is_some() {
            return Ok(ToolResult::error(
                "Found multiple occurrences of the text. Please provide more context to make the match unique."
                    .to_string(),
            ));
        }

        // Replace only the matched span — byte offsets are in the ORIGINAL
        // content, so no CRLF/LF normalization is written back.
        let new_content = format!("{}{}{}", &content[..start], new_text, &content[end..]);
        std::fs::write(&file_path, &new_content)?;

        // The agent's newest knowledge of this file is what it just wrote.
        crate::tools::stale_edit::record_seen(
            &context.app,
            &context.session_id,
            context.workspace.as_deref(),
            &file_path,
            new_content.as_bytes(),
        )
        .await;

        let mut result = format!(
            "Successfully edited {}: 1 replacement made",
            file_path.display()
        );
        if let Some(diff) = super::diff_preview::compute_diff(&content, &new_content) {
            result.push_str(&format!("\n\n{diff}"));
        }

        Ok(ToolResult::success(result))
    }
}
