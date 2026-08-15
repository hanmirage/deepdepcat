//! Workspace structure snapshot — a compact summary of the project layout
//! injected into the agent's dynamic context.
//!
//! The agent knows the workspace *path* but not what the project looks like.
//! This module scans the top level (cheap) and a depth-bounded set of source
//! files (filtered by the detected `ProjectType`) so the model can orient
//! itself without listing the whole tree itself. The snapshot is cached and
//! only re-scanned when the workspace root's mtime changes.

use std::path::Path;
use std::time::SystemTime;

use crate::core::types::ProjectType;

/// Maximum top-level entries rendered (dirs and files).
pub const MAX_TOP_LEVEL_ENTRIES: usize = 60;
/// Maximum source files rendered (recursive, depth-bounded).
pub const MAX_SOURCE_FILES: usize = 60;
/// Maximum recursion depth for the source-file scan.
const MAX_SCAN_DEPTH: usize = 3;

/// Directories never descended into (mirrors `memory/watcher.rs` ignore set).
const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "dist",
    ".next",
    "__pycache__",
    "build",
    ".deepdepcat",
    ".claude",
];

/// A snapshot of the workspace layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectStructure {
    /// Top-level entries, sorted; directories carry a trailing `/`.
    pub top_level: Vec<String>,
    /// Source files relative to the workspace root (e.g. `src/main.rs`).
    pub source_files: Vec<String>,
    /// mtime of the workspace root at snapshot time — cache invalidation key.
    pub root_mtime: Option<SystemTime>,
    /// True when a cap was hit and the rendered output carries a truncation note.
    pub truncated: bool,
}

/// Walk the workspace and produce a compact structure summary.
///
/// Top level is a single `read_dir`; the source scan is depth-bounded and
/// filtered by the project type's extensions. `Unknown` project types skip
/// the recursive source scan entirely.
pub fn scan_project_structure(workspace: &Path, project_type: &ProjectType) -> ProjectStructure {
    let root_mtime = workspace.metadata().and_then(|m| m.modified()).ok();
    let mut truncated = false;

    let mut top_level = Vec::new();
    if let Ok(entries) = std::fs::read_dir(workspace) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if IGNORED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let suffix = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            if top_level.len() >= MAX_TOP_LEVEL_ENTRIES {
                truncated = true;
                break;
            }
            top_level.push(format!("{name}{suffix}"));
        }
    }
    top_level.sort();

    let mut source_files = Vec::new();
    if *project_type != ProjectType::Unknown {
        let exts = project_type.source_extensions();
        if !exts.is_empty() {
            let mut stack = vec![(workspace.to_path_buf(), 0u32)];
            while let Some((dir, depth)) = stack.pop() {
                if depth > MAX_SCAN_DEPTH as u32 {
                    continue;
                }
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Ok(ft) = entry.file_type() else {
                        continue;
                    };
                    let name = entry.file_name().to_string_lossy().to_string();
                    let rel = entry
                        .path()
                        .strip_prefix(workspace)
                        .unwrap_or(&entry.path())
                        .to_string_lossy()
                        .replace('\\', "/");
                    if ft.is_dir() {
                        if !IGNORED_DIRS.contains(&name.as_str())
                            && source_files.len() < MAX_SOURCE_FILES
                        {
                            stack.push((entry.path(), depth + 1));
                        }
                    } else if let Some(ext) = Path::new(&name).extension().and_then(|e| e.to_str())
                    {
                        if exts.contains(&ext) && source_files.len() < MAX_SOURCE_FILES {
                            source_files.push(rel);
                        }
                    }
                    if source_files.len() >= MAX_SOURCE_FILES {
                        truncated = true;
                        break;
                    }
                }
            }
            source_files.sort();
            source_files.truncate(MAX_SOURCE_FILES);
        }
    }

    ProjectStructure {
        top_level,
        source_files,
        root_mtime,
        truncated,
    }
}

/// Render the snapshot as the `## Project Structure` injection block.
impl std::fmt::Display for ProjectStructure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Top-level: {}", self.top_level.join(", "))?;
        if !self.source_files.is_empty() {
            writeln!(f, "Source files: {}", self.source_files.join(", "))?;
        }
        if self.truncated {
            writeln!(f, "... (truncated)")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_top_level_and_source_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();

        let s = scan_project_structure(dir.path(), &ProjectType::Rust);
        assert!(s.top_level.contains(&"src/".to_string()));
        assert!(s.top_level.contains(&"Cargo.toml".to_string()));
        assert!(s.source_files.contains(&"src/main.rs".to_string()));
        assert!(!s.truncated);
    }

    #[test]
    fn ignores_common_build_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("node_modules/x.js"), "").unwrap();
        std::fs::write(dir.path().join("target/main.rs"), "").unwrap();

        let s = scan_project_structure(dir.path(), &ProjectType::Rust);
        assert!(!s.top_level.iter().any(|e| e.starts_with("node_modules")));
        assert!(!s.top_level.iter().any(|e| e.starts_with("target")));
        assert!(!s.top_level.iter().any(|e| e.starts_with(".git")));
        assert!(!s.source_files.iter().any(|f| f.starts_with("node_modules")));
    }

    #[test]
    fn unknown_type_skips_source_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("random.txt"), "").unwrap();
        let s = scan_project_structure(dir.path(), &ProjectType::Unknown);
        assert!(s.source_files.is_empty());
        assert!(!s.top_level.is_empty());
    }

    #[test]
    fn truncates_large_top_level() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..100 {
            std::fs::write(dir.path().join(format!("file{i}.txt")), "").unwrap();
        }
        let s = scan_project_structure(dir.path(), &ProjectType::Unknown);
        assert!(s.truncated);
        assert!(s.top_level.len() <= MAX_TOP_LEVEL_ENTRIES);
    }

    #[test]
    fn display_renders_sections() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("src.rs"), "").unwrap();
        let s = scan_project_structure(dir.path(), &ProjectType::Rust);
        let out = s.to_string();
        assert!(out.contains("Top-level:"));
        assert!(out.contains("src.rs"));
    }
}
