//! List directory tool — lists the contents of a directory.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
pub struct ListDirTool;

impl ListDirTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the contents of a directory. Shows files and subdirectories with their types and sizes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list. Relative paths resolve against the workspace root. Use '.' for the workspace root."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Lists one extra level — the direct children of each subdirectory — NOT a full recursive tree. Defaults to false. For deep trees, walk each subdirectory explicitly."
                }
            },
            "required": ["path"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'path'".into()))?;
        let recursive = args
            .get("recursive")
            .and_then(|r| r.as_bool())
            .unwrap_or(false);

        let path_buf = super::resolve_path(context.workspace.as_deref(), path);

        if !path_buf.exists() {
            return Ok(ToolResult::error(format!("Directory not found: {}", path)));
        }

        if !path_buf.is_dir() {
            return Ok(ToolResult::error(format!("Not a directory: {}", path)));
        }

        let mut output = String::new();
        let mut entries: Vec<_> = match std::fs::read_dir(&path_buf) {
            Ok(d) => d.filter_map(|e| e.ok()).collect(),
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read directory: {}",
                    e
                )))
            }
        };

        // Sort: directories first, then files, alphabetically
        entries.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        for entry in &entries {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

            if is_dir {
                output.push_str(&format!("📁 {}/\n", name_str));
                if recursive {
                    let sub_path = path_buf.join(name_str.as_ref());
                    if let Ok(sub_entries) = std::fs::read_dir(&sub_path) {
                        for sub_entry in sub_entries.filter_map(|e| e.ok()) {
                            let sub_name = sub_entry.file_name();
                            let sub_is_dir =
                                sub_entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            if sub_is_dir {
                                output.push_str(&format!("  📁 {}/\n", sub_name.to_string_lossy()));
                            } else {
                                output.push_str(&format!("  📄 {}\n", sub_name.to_string_lossy()));
                            }
                        }
                    }
                }
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let size_str = if size < 1024 {
                    format!("{} B", size)
                } else if size < 1024 * 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
                };
                output.push_str(&format!("📄 {} ({})\n", name_str, size_str));
            }
        }

        Ok(ToolResult::success(output))
    }
}
