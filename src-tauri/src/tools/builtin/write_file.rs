//! Write file tool — creates or overwrites a file with the given content.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
pub struct WriteFileTool;

impl WriteFileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, or overwrites it if it does (but a write to an EXISTING file is refused when the file changed on disk since your last read — re-read it first). Creates parent directories as needed. For very large content, write it in pieces: create the file with this tool first, then append sections with edit_file/search_replace — a single oversized content argument can be cut off by the output token limit."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write. Relative paths resolve against the workspace root."
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
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
        let path = args.get("path").and_then(|p| p.as_str()).ok_or_else(|| {
            crate::core::error::AppError::Parse("Missing 'path' parameter".into())
        })?;
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::Parse("Missing 'content' parameter".into())
            })?;

        let path_buf = super::resolve_path(context.workspace.as_deref(), path);

        // Capture file state before modification for checkpoint/rewind.
        if let Some(ref tracker) = context.file_state_tracker {
            if let Some(ref workspace) = context.workspace {
                tracker.capture_file_state(&path_buf, workspace).await;
            }
        }

        // Stale-edit guard: refuse to overwrite an existing file whose
        // content changed since the agent last read it — the model may be
        // clobbering changes it has never seen (see tools/stale_edit.rs).
        if path_buf.exists() {
            if let Some(hint) = crate::tools::stale_edit::check_stale(
                &context.app,
                &context.session_id,
                context.workspace.as_deref(),
                &path_buf,
            )
            .await
            {
                return Ok(ToolResult::error(hint));
            }
        }

        // Read the previous content BEFORE the write so the diff preview
        // shows the actual change (reading after the write would diff the
        // new content against itself and always be empty).
        let previous = if path_buf.exists() {
            std::fs::read_to_string(&path_buf).ok()
        } else {
            None
        };

        // Create parent directories if needed
        if let Some(parent) = path_buf.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let existed = path_buf.exists();

        std::fs::write(&path_buf, content)?;

        // The agent's newest knowledge of this file is what it just wrote.
        crate::tools::stale_edit::record_seen(
            &context.app,
            &context.session_id,
            context.workspace.as_deref(),
            &path_buf,
            content.as_bytes(),
        )
        .await;

        let line_count = content.lines().count();
        let byte_count = content.len();

        let mut result = format!(
            "Successfully wrote {} bytes ({} lines) to {} ({})",
            byte_count,
            line_count,
            path,
            if existed { "overwritten" } else { "created" }
        );
        // New files have no previous content to diff against — only show a
        // diff when an existing file was overwritten.
        if let Some(previous) = previous {
            if let Some(diff) = super::diff_preview::compute_diff(&previous, content) {
                result.push_str(&format!("\n\n{diff}"));
            }
        }

        Ok(ToolResult::success(result))
    }
}
