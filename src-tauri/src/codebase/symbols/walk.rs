use std::path::{Path, PathBuf};

use super::types::Language;

/// Walk a directory recursively, yielding all source files.
pub fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if !root.is_dir() {
        return files;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip common non-source directories
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(
                name,
                ".git" | "node_modules" | "target" | "dist" | "build" | "__pycache__" | ".next"
            ) {
                continue;
            }
            files.extend(walkdir(&path));
        } else if path.is_file() {
            let lang = Language::from_path(&path);
            if lang != Language::Unknown {
                files.push(path);
            }
        }
    }

    files
}
