//! batch_file — batch file operations for office workflows.
//!
//! Actions:
//! - `rename` — rename files in a directory using a template
//!   (`{index}` = zero-padded position, `{name}` = original stem,
//!   `{date}` = today) with an optional prefix/suffix.
//! - `copy` / `move` — copy or move files matching an extension list into
//!   an output directory (flattened).
//! - `sort` — group files into subfolders by extension.
//! - `text_replace` — replace a string in text files (default text
//!   extensions; docx/pdf binaries are skipped).
//!
//! Examples:
//! - rename dir "C:\invoices" template "2026_{index}_{name}" ext "pdf"
//! - copy dir "C:\raw" to "C:\processed" ext "xlsx,csv"
//! - sort dir "C:\downloads"

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

/// Batch file operations.
pub struct BatchFileTool;

impl BatchFileTool {
    pub fn new() -> Self {
        Self
    }
}

fn list_files(dir: &Path, exts: &[String]) -> AppResult<Vec<std::path::PathBuf>> {
    let entries = std::fs::read_dir(dir)?;
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if exts.is_empty() {
            files.push(path);
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if exts.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_extensions(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| {
            s.split(',')
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an extension is treated as plain text by `text_replace` when no
/// explicit `ext` filter is given (binary formats are never touched).
fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "toml"
            | "yaml"
            | "yml"
            | "log"
            | "html"
            | "htm"
            | "css"
            | "xml"
            | "ini"
            | "cfg"
    )
}

fn action_text_replace(args: &Value, context: &ToolContext) -> AppResult<String> {
    let dir_str = args
        .get("dir")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "Missing required parameter: dir".to_string())?;
    let find = args
        .get("find")
        .and_then(|f| f.as_str())
        .filter(|f| !f.is_empty())
        .ok_or_else(|| "Missing required parameter: find".to_string())?;
    let replace = args.get("replace").and_then(|r| r.as_str()).unwrap_or("");
    let exts = parse_extensions(args, "ext");

    let dir = crate::tools::builtin::resolve_path(context.workspace.as_deref(), dir_str);
    let files = list_files(&dir, &exts)?;
    if files.is_empty() {
        return Ok("No text files matched".to_string());
    }

    let mut changed = 0;
    let mut total = 0;
    for path in &files {
        if exts.is_empty() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !is_text_ext(ext) {
                continue;
            }
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        total += 1;
        if content.contains(find) {
            std::fs::write(path, content.replace(find, replace))?;
            changed += 1;
        }
    }
    Ok(format!(
        "Replaced '{find}' in {changed}/{total} text files under {}",
        dir.display()
    ))
}

fn render_template(template: &str, index: usize, name: &str) -> String {
    let now = chrono::Local::now();
    let mut out = template
        .replace("{index}", &format!("{index:03}"))
        .replace("{name}", name)
        .replace("{date}", &now.format("%Y-%m-%d").to_string());
    // A bare `{...}` template yields an empty stem — fall back to the name.
    if out.trim().is_empty() {
        out = name.to_string();
    }
    out
}

fn action_rename(args: &Value, context: &ToolContext) -> AppResult<String> {
    let dir_str = args
        .get("dir")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "Missing required parameter: dir".to_string())?;
    let template = args
        .get("template")
        .and_then(|t| t.as_str())
        .unwrap_or("{index}_{name}");
    let exts = parse_extensions(args, "ext");

    let dir = crate::tools::builtin::resolve_path(context.workspace.as_deref(), dir_str);
    let files = list_files(&dir, &exts)?;
    if files.is_empty() {
        return Ok("No files matched".to_string());
    }

    let mut renamed = 0;
    for (i, path) in files.iter().enumerate() {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let new_stem = render_template(template, i + 1, stem);
        let new_name = if ext.is_empty() {
            new_stem
        } else {
            format!("{new_stem}.{ext}")
        };
        let new_path = path.with_file_name(&new_name);
        if new_path != *path {
            std::fs::rename(path, &new_path)?;
            renamed += 1;
        }
    }
    Ok(format!(
        "Renamed {renamed}/{len} files in {dir} using template '{template}'",
        len = files.len(),
        dir = dir.display()
    ))
}

fn action_copy_or_move(args: &Value, context: &ToolContext, is_move: bool) -> AppResult<String> {
    let dir_str = args
        .get("dir")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "Missing required parameter: dir".to_string())?;
    let to_str = args
        .get("to")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "Missing required parameter: to".to_string())?;
    let exts = parse_extensions(args, "ext");

    let dir = crate::tools::builtin::resolve_path(context.workspace.as_deref(), dir_str);
    let to = crate::tools::builtin::resolve_path(context.workspace.as_deref(), to_str);
    let files = list_files(&dir, &exts)?;
    if files.is_empty() {
        return Ok("No files matched".to_string());
    }
    std::fs::create_dir_all(&to)?;

    let verb = if is_move { "Moved" } else { "Copied" };
    for path in &files {
        let target = to.join(
            path.file_name()
                .ok_or_else(|| "Invalid file name".to_string())?,
        );
        if is_move {
            std::fs::rename(path, &target)?;
        } else {
            std::fs::copy(path, &target)?;
        }
    }
    Ok(format!(
        "{verb} {len} files → {}",
        to.display(),
        len = files.len()
    ))
}

fn action_sort(args: &Value, context: &ToolContext) -> AppResult<String> {
    let dir_str = args
        .get("dir")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "Missing required parameter: dir".to_string())?;
    let dir = crate::tools::builtin::resolve_path(context.workspace.as_deref(), dir_str);
    let files = list_files(&dir, &[])?;
    if files.is_empty() {
        return Ok("No files in directory".to_string());
    }

    let mut grouped: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
        std::collections::BTreeMap::new();
    for path in &files {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_else(|| "noext".to_string());
        grouped.entry(ext).or_default().push(path.clone());
    }
    for (ext, group) in &grouped {
        let target = dir.join(ext);
        std::fs::create_dir_all(&target)?;
        for path in group {
            let file_name = path
                .file_name()
                .ok_or_else(|| AppError::Path(format!("No file name for {}", path.display())))?;
            let dest = target.join(file_name);
            std::fs::rename(path, &dest)?;
        }
    }
    Ok(format!(
        "Grouped {len} files into {} extension folders",
        grouped.len(),
        len = files.len()
    ))
}

#[async_trait]
impl Tool for BatchFileTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "batch_file"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Batch file operations for office workflows. Actions: \
         rename (dir + template with {index}/{name}/{date}, optional ext filter), \
         copy|move (dir → to, optional ext list like \"pdf,docx\"), \
         sort (group files into per-extension subfolders), \
         text_replace (dir + find + replace, optional ext filter; plain \
         text files only, binaries skipped)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                      "enum": ["rename", "copy", "move", "sort", "text_replace"],
                      "description": "Operation to perform."
                  },
                  "find": {
                      "type": "string",
                      "description": "String to find (text_replace)."
                  },
                  "replace": {
                      "type": "string",
                      "description": "Replacement string (text_replace)."
                  },
                "dir": {
                    "type": "string",
                    "description": "Source directory."
                },
                "to": {
                    "type": "string",
                    "description": "Destination directory (copy/move)."
                },
                "template": {
                    "type": "string",
                    "description": "Rename template: {index} {name} {date} (e.g. \"2026_{index}_{name}\")."
                },
                "ext": {
                    "type": "string",
                    "description": "Extension filter, comma-separated, no dots (e.g. \"pdf,docx\"). Empty = all files."
                }
            },
            "required": ["action", "dir"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?;
        let result = match action {
            "rename" => action_rename(&args, context)?,
            "copy" => action_copy_or_move(&args, context, false)?,
            "move" => action_copy_or_move(&args, context, true)?,
            "sort" => action_sort(&args, context)?,
            "text_replace" => action_text_replace(&args, context)?,
            other => {
                return Err(format!(
                    "Unknown action: {other}. Use rename/copy/move/sort/text_replace"
                )
                .into())
            }
        };
        Ok(ToolResult::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_renders_placeholders() {
        assert_eq!(
            render_template("{index}_{name}", 3, "invoice"),
            "003_invoice"
        );
        assert_eq!(render_template("pre_{name}", 1, "data"), "pre_data");
        assert_eq!(render_template("", 1, "data"), "data");
    }

    #[test]
    fn list_files_filters_extensions() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.pdf"), "x").expect("write");
        std::fs::write(dir.path().join("b.docx"), "x").expect("write");
        std::fs::write(dir.path().join("c.txt"), "x").expect("write");

        let pdf = list_files(dir.path(), &["pdf".to_string()]).expect("list");
        assert_eq!(pdf.len(), 1);
        assert!(pdf[0].to_string_lossy().ends_with("a.pdf"));

        let all = list_files(dir.path(), &[]).expect("list");
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn text_ext_classification_skips_binaries() {
        assert!(is_text_ext("md"));
        assert!(is_text_ext("CSV"));
        assert!(!is_text_ext("docx"));
        assert!(!is_text_ext("pdf"));
        assert!(!is_text_ext("png"));
    }
}
