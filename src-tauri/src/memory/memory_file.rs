//! Dual-layer MEMORY.md — user-level and project-level standing memory.
//!
//! The agent writes long-term notes here via `memory_write`. Both files use
//! the managed-section pattern: everything between the
//! `<!-- managed:memory -->` markers is agent-managed bullets; the user's
//! own hand-written content outside the markers is preserved verbatim.
//! Writes are atomic (same-dir temp + rename), deduplicated, and capped at
//! `MAX_ENTRIES` bullets (newest wins).

use crate::core::error::AppResult;
use crate::workspace::project_files::{read_file_lossy, user_deepdepcat_dir};
use std::path::{Path, PathBuf};

/// Managed-section markers — only text between these is agent-writable.
pub const MANAGED_START: &str = "<!-- managed:memory -->";
pub const MANAGED_END: &str = "<!-- /managed:memory -->";
/// Max bullets kept per file (oldest dropped first).
pub const MAX_ENTRIES: usize = 200;
/// Max chars per entry — long notes get truncated to a one-line bullet.
const ENTRY_MAX_CHARS: usize = 400;

/// User-level memory file: `~/.deepdepcat/MEMORY.md` (workspace-independent).
pub fn user_memory_path() -> PathBuf {
    user_deepdepcat_dir().join("MEMORY.md")
}

/// Project-level memory file: `<workspace>/.deepdepcat/MEMORY.md`.
pub fn project_memory_path(workspace: &Path) -> PathBuf {
    workspace.join(".deepdepcat").join("MEMORY.md")
}

/// Read a MEMORY.md file (native-encoding fallback), if it exists.
pub fn read_memory_file(path: &Path) -> Option<String> {
    if path.exists() {
        read_file_lossy(path)
    } else {
        None
    }
}

/// Load the user-level MEMORY.md (injected into the system prompt as
/// `## User Memory`).
pub fn load_user_memory() -> Option<String> {
    read_memory_file(&user_memory_path())
}

/// Normalize a memory entry into a one-line bullet body: trim, strip a
/// leading dash if the model already wrote one, collapse whitespace, cap
/// length. Empty input yields an empty string (callers reject it).
pub fn normalize_entry(entry: &str) -> String {
    let cleaned = entry
        .trim_start_matches(|c: char| c.is_whitespace() || c == '-')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cleaned.chars().take(ENTRY_MAX_CHARS).collect()
}

/// Append `entry` as a bullet inside the managed section of `original`,
/// deduplicating against existing bullets (exact or one-way containment for
/// entries ≥16 chars) and keeping at most `MAX_ENTRIES` (newest wins).
/// Everything outside the markers is preserved verbatim.
pub fn append_managed_entry(original: &str, entry: &str) -> String {
    let entry = normalize_entry(entry);
    if entry.is_empty() {
        return original.to_string();
    }
    let (prefix, mut bullets, suffix) = split_managed(original);
    if !bullets.iter().any(|b| is_duplicate(b, &entry)) {
        // Bullets are stored WITHOUT the dash — `rebuild` adds the `- `
        // prefix when rendering. Pushing a pre-dashed bullet here would
        // double the dash on the next round-trip.
        bullets.push(entry.clone());
    }
    while bullets.len() > MAX_ENTRIES {
        bullets.remove(0);
    }
    rebuild(prefix, bullets, suffix)
}

/// Split `original` into (text before the managed section, bullets inside,
/// text after the section). Without markers the whole text is user prefix.
fn split_managed(original: &str) -> (String, Vec<String>, String) {
    if let (Some(s), Some(e)) = (original.find(MANAGED_START), original.find(MANAGED_END)) {
        let end = e + MANAGED_END.len();
        let prefix = original[..s].to_string();
        let body = &original[s + MANAGED_START.len()..e];
        let suffix = original[end..].to_string();
        let bullets: Vec<String> = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("<!--"))
            .map(|l| l.trim_start_matches('-').trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        (prefix, bullets, suffix)
    } else {
        (original.trim_end().to_string(), Vec::new(), String::new())
    }
}

/// Rebuild the full file from prefix + bullets + suffix.
fn rebuild(prefix: String, bullets: Vec<String>, suffix: String) -> String {
    let body = bullets
        .iter()
        .map(|b| format!("- {b}"))
        .collect::<Vec<_>>()
        .join("\n");
    if suffix.is_empty() {
        format!("{prefix}\n\n{MANAGED_START}\n{body}\n{MANAGED_END}\n")
    } else {
        format!("{prefix}{MANAGED_START}\n{body}\n{MANAGED_END}{suffix}")
    }
}

/// Case-insensitive duplicate check: exact match, or one side contains the
/// other when the longer side is substantial (≥16 chars).
fn is_duplicate(existing: &str, entry: &str) -> bool {
    let a = existing.trim().to_lowercase();
    let b = entry.to_lowercase();
    a == b || (a.len() >= 16 && a.contains(&b)) || (b.len() >= 16 && b.contains(&a))
}

/// Atomic write: temp file in the same directory, then rename over the
/// target — a crash mid-write can never corrupt the memory file.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Append a memory entry to the given MEMORY.md file (creating it and the
/// parent directory as needed). Returns the final file path.
pub fn write_memory_entry(path: &Path, entry: &str) -> AppResult<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let current = read_memory_file(path).unwrap_or_default();
    let updated = append_managed_entry(&current, entry);
    atomic_write(path, &updated).map_err(|e| {
        crate::core::error::AppError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to write memory file {}: {e}", path.display()),
        ))
    })?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let path = tmp.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
        tmp
    }

    #[test]
    fn normalize_entry_makes_one_line_bullet_body() {
        assert_eq!(
            normalize_entry("  - 记住：\n  用户偏好深色模式  "),
            "记住： 用户偏好深色模式"
        );
        assert_eq!(normalize_entry("already - fine"), "already - fine");
        assert_eq!(normalize_entry("   "), "");
        let long = "x".repeat(600);
        assert_eq!(normalize_entry(&long).chars().count(), ENTRY_MAX_CHARS);
    }

    #[test]
    fn append_creates_managed_section_and_preserves_user_text() {
        let original = "我的笔记：用户喜欢深色模式。";
        let out = append_managed_entry(original, "项目使用 Rust 与 Tauri");
        assert!(out.contains("我的笔记：用户喜欢深色模式。"));
        assert!(out.contains(MANAGED_START));
        assert!(out.contains("- 项目使用 Rust 与 Tauri"));
        assert!(out.contains(MANAGED_END));
    }

    #[test]
    fn append_dedupes_exact_and_contained_entries() {
        let original = "头部\n\n<!-- managed:memory -->\n- 项目使用 Rust 与 Tauri\n<!-- /managed:memory -->\n\n尾部";
        let out = append_managed_entry(original, "项目使用 Rust 与 Tauri");
        assert_eq!(out.matches("- 项目使用 Rust 与 Tauri").count(), 1);
        let contained = append_managed_entry(original, "项目使用 Rust 与 Tauri 部署到 Debian");
        assert_eq!(
            contained.lines().filter(|l| l.starts_with("- ")).count(),
            1,
            "contained entry must dedupe"
        );
        let fresh = append_managed_entry(original, "部署目标是 Debian 服务器");
        assert_eq!(
            fresh.lines().filter(|l| l.starts_with("- ")).count(),
            2,
            "new entry must append"
        );
    }

    #[test]
    fn append_caps_entries_newest_wins() {
        let mut original = String::new();
        for i in 0..(MAX_ENTRIES + 10) {
            original = append_managed_entry(&original, &format!("entry {i}"));
        }
        assert_eq!(
            original
                .lines()
                .filter(|l| l.starts_with("- entry "))
                .count(),
            MAX_ENTRIES
        );
        assert!(original.contains("- entry 209"), "newest must survive");
        assert!(!original.contains("- entry 0"), "oldest must be dropped");
    }

    #[test]
    fn write_memory_entry_roundtrip_and_atomic() {
        let tmp = make_dir_with(&[]);
        let path = project_memory_path(tmp.path());
        write_memory_entry(&path, "第一条约记").unwrap();
        write_memory_entry(&path, "第二条：架构用 Rust").unwrap();
        let content = read_memory_file(&path).unwrap();
        assert!(content.contains("- 第一条约记"));
        assert!(content.contains("- 第二条：架构用 Rust"));
        assert!(!path.with_extension("md.tmp").exists(), "no temp leftover");
    }

    #[test]
    fn read_missing_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_memory_file(&tmp.path().join("nope.md")).is_none());
    }
}
