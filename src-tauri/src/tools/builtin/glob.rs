//! Glob tool — finds files matching a glob pattern.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Supports ** for recursive matching, * for any chars, and ? for single char."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern (e.g. '**/*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "The base directory to search from. Defaults to workspace root."
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|p| p.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'pattern'".into()))?;
        let base_path = args
            .get("path")
            .and_then(|p| p.as_str())
            .map(PathBuf::from)
            .or_else(|| context.workspace.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let mut results = Vec::new();
        self.search(&base_path, pattern, &mut results, 0);

        if results.is_empty() {
            Ok(ToolResult::success("No files found matching the pattern."))
        } else {
            let mut output = format!("Found {} files:\n", results.len());
            for path in &results {
                output.push_str(&format!("  {}\n", path));
            }
            Ok(ToolResult::success(output))
        }
    }
}

impl GlobTool {
    fn search(&self, base: &PathBuf, pattern: &str, results: &mut Vec<String>, depth: u8) {
        if depth > 20 || results.len() > 1000 {
            return;
        }

        let parts: Vec<&str> = pattern.split('/').collect();

        if parts.is_empty() {
            return;
        }

        self.search_segments(base, &parts, results, depth);
    }

    fn search_segments(
        &self,
        current: &PathBuf,
        segments: &[&str],
        results: &mut Vec<String>,
        depth: u8,
    ) {
        if depth > 20 || results.len() > 1000 {
            return;
        }

        if segments.is_empty() {
            if current.is_file() {
                results.push(current.to_string_lossy().to_string());
            }
            return;
        }

        let segment = segments[0];
        let rest = &segments[1..];

        if segment == "**" {
            self.search_segments(current, rest, results, depth + 1);

            if let Ok(entries) = std::fs::read_dir(current) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy();
                        if !name_str.starts_with('.') && !IGNORED_DIRS.contains(&name_str.as_ref())
                        {
                            self.search_segments(&path, segments, results, depth + 1);
                        }
                    }
                }
            }
        } else if segment.contains('*')
            || segment.contains('?')
            || segment.contains('{')
            || segment.contains('[')
        {
            let pattern = match glob::Pattern::new(segment) {
                Ok(p) => p,
                Err(_) => return,
            };
            if let Ok(entries) = std::fs::read_dir(current) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if pattern.matches(&name_str) {
                        let path = entry.path();
                        if rest.is_empty() {
                            if path.is_file() {
                                results.push(path.to_string_lossy().to_string());
                            }
                        } else if path.is_dir() {
                            self.search_segments(&path, rest, results, depth + 1);
                        }
                    }
                }
            }
        } else {
            let next = current.join(segment);
            if next.exists() {
                if rest.is_empty() {
                    if next.is_file() {
                        results.push(next.to_string_lossy().to_string());
                    }
                } else if next.is_dir() {
                    self.search_segments(&next, rest, results, depth + 1);
                }
            }
        }
    }
}

const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".next",
    ".nuxt",
];
