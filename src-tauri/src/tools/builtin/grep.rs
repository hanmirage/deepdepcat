//! Grep tool — searches file contents using regex patterns.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct GrepTool {
    max_output_chars: usize,
}

impl GrepTool {
    pub fn new(max_output_chars: usize) -> Self {
        Self { max_output_chars }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using a regex pattern. Searches recursively through files in the given path. Returns matching lines with file paths and line numbers."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "The directory or file to search in. Defaults to current directory."
                },
                "include": {
                    "type": "string",
                    "description": "File glob pattern to include (e.g. '*.rs', '*.ts'). Defaults to all files."
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Whether to search case-insensitively. Defaults to false."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches to return. Defaults to 100."
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
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
        let include = args.get("include").and_then(|i| i.as_str());
        let case_insensitive = args
            .get("case_insensitive")
            .and_then(|c| c.as_bool())
            .unwrap_or(false);
        let max_results = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .unwrap_or(100) as usize;

        let mut regex_builder = RegexBuilder::new(pattern);
        regex_builder.case_insensitive(case_insensitive);
        let regex = match regex_builder.build() {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(format!("Invalid regex pattern: {}", e))),
        };

        let include_pattern = match include {
            Some(p) => match glob::Pattern::new(p) {
                Ok(compiled) => Some(compiled),
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Invalid include glob pattern '{}': {}",
                        p, e
                    )))
                }
            },
            None => None,
        };

        let search_path = if PathBuf::from(path).is_absolute() {
            PathBuf::from(path)
        } else {
            context
                .workspace
                .as_ref()
                .map(|w| w.join(path))
                .unwrap_or_else(|| PathBuf::from(path))
        };

        let mut matches = Vec::new();
        let mut files_searched = 0;
        let mut visited_dirs = std::collections::HashSet::new();

        self.search_recursive(
            &search_path,
            &regex,
            include_pattern.as_ref(),
            &mut matches,
            &mut files_searched,
            max_results,
            &mut visited_dirs,
            0,
        )?;

        if matches.is_empty() {
            return Ok(ToolResult::success(format!(
                "No matches found. Searched {} files.",
                files_searched
            )));
        }

        let mut output = String::new();
        for m in &matches {
            output.push_str(&format!("{}:{}: {}\n", m.file, m.line_num, m.line));
        }

        output.push_str(&format!(
            "\nFound {} matches in {} files (searched {} files)",
            matches.len(),
            matches
                .iter()
                .map(|m| &m.file)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            files_searched
        ));

        if output.len() > self.max_output_chars {
            output = format!(
                "{}\n\n...(output truncated, showing {} of {} chars)",
                crate::core::str_util::truncate_at_char_boundary(&output, self.max_output_chars),
                self.max_output_chars,
                output.len()
            );
        }

        Ok(ToolResult::success(output))
    }
}

struct GrepMatch {
    file: String,
    line_num: usize,
    line: String,
}

impl GrepTool {
    #[allow(clippy::too_many_arguments)]
    fn search_recursive(
        &self,
        path: &PathBuf,
        regex: &Regex,
        include_pattern: Option<&glob::Pattern>,
        matches: &mut Vec<GrepMatch>,
        files_searched: &mut usize,
        max_results: usize,
        visited_dirs: &mut std::collections::HashSet<PathBuf>,
        depth: usize,
    ) -> AppResult<()> {
        if matches.len() >= max_results {
            return Ok(());
        }

        if path.is_file() {
            self.search_file(path, regex, matches, max_results)?;
            *files_searched += 1;
            return Ok(());
        }

        if !path.is_dir() {
            return Ok(());
        }

        // Depth cap + canonical visited set: a directory symlink cycle
        // (a -> b -> a) or a pathological nesting must never blow the stack
        // or loop forever. Canonicalization resolves both symlinks and
        // Windows junctions, so the SAME physical directory reached through
        // different link paths is visited once.
        if depth >= MAX_DIR_DEPTH {
            return Ok(());
        }
        if let Ok(canonical) = path.canonicalize() {
            if !visited_dirs.insert(canonical) {
                return Ok(());
            }
        }

        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        for entry in entries.filter_map(|e| e.ok()) {
            if matches.len() >= max_results {
                break;
            }

            let entry_path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with('.') {
                continue;
            }
            if entry_path.is_dir() && IGNORED_DIRS.contains(&name_str.as_ref()) {
                continue;
            }

            if entry_path.is_dir() {
                self.search_recursive(
                    &entry_path,
                    regex,
                    include_pattern,
                    matches,
                    files_searched,
                    max_results,
                    visited_dirs,
                    depth + 1,
                )?;
            } else if entry_path.is_file() {
                if let Some(pattern) = include_pattern {
                    if !pattern.matches(&name_str) {
                        continue;
                    }
                }

                self.search_file(&entry_path, regex, matches, max_results)?;
                *files_searched += 1;
            }
        }

        Ok(())
    }

    fn search_file(
        &self,
        path: &PathBuf,
        regex: &Regex,
        matches: &mut Vec<GrepMatch>,
        max_results: usize,
    ) -> AppResult<()> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        for (i, line) in content.lines().enumerate() {
            if matches.len() >= max_results {
                break;
            }
            if regex.is_match(line) {
                matches.push(GrepMatch {
                    file: path.to_string_lossy().to_string(),
                    line_num: i + 1,
                    line: line.trim().to_string(),
                });
            }
        }

        Ok(())
    }
}

use regex::RegexBuilder;

/// Maximum directory nesting depth for recursive search (guards against
/// pathological trees and symlink cycles before the visited-set kicks in).
const MAX_DIR_DEPTH: usize = 16;

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
    ".gradle",
    ".idea",
    ".vscode",
];
