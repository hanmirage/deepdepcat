//! Project files — DEEPDEPCAT.md instruction & memory discovery (P1-7).
//!
//! Implements the `docs/WORKDIR_LAYOUT.md` contract:
//! - Own filename `DEEPDEPCAT.md` first (project root + `.deepdepcat/`),
//!   `instructions.md` kept as a legacy alias; `CLAUDE.md` family only
//!   falls back when no own instruction file exists.
//! - User-level root `~/.deepdepcat/` (`DEEPDEPCAT_HOME` redirects it),
//!   merged below project-level content.
//! - All reads go through `decode_native_output` (UTF-8 → GBK → UTF-16
//!   fallback) so non-UTF-8 instruction files never mangle into garbage.
//! - `AGENTS.md` keeps its memory semantics (it is NOT an instruction).

use std::path::{Path, PathBuf};

/// User-level deepdepcat root: `DEEPDEPCAT_HOME` env wins, else `~/.deepdepcat`.
pub fn user_deepdepcat_dir() -> PathBuf {
    std::env::var_os("DEEPDEPCAT_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".deepdepcat")
        })
}

/// Read a file with native-encoding fallback (UTF-8 → GBK → UTF-16).
pub fn read_file_lossy(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(crate::core::encoding::decode_native_output(&bytes))
}

/// Own instruction candidates (project root + `.deepdepcat/`), in priority
/// order. `instructions.md` is the legacy alias of `DEEPDEPCAT.md`.
pub fn own_instruction_candidates(workspace: &Path) -> Vec<PathBuf> {
    vec![
        workspace.join("DEEPDEPCAT.md"),
        workspace.join(".deepdepcat").join("DEEPDEPCAT.md"),
        workspace.join(".deepdepcat").join("instructions.md"),
    ]
}

/// Fallback instruction family — the ecosystem files (Claude Code &
/// Codex): `AGENTS.md` is Codex's project instruction file and `CLAUDE.md`
/// family is Claude Code's. Read ONLY when no own instruction file exists.
/// Looked up at the root, `.deepdepcat/` and `.claude/`.
pub fn fallback_instruction_candidates(workspace: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in [
        "CLAUDE.md",
        "CLAUDE.local.md",
        "Claude.md",
        "AGENT.md",
        "AGENTS.md",
        "Agents.md",
    ] {
        for base in [
            workspace.to_path_buf(),
            workspace.join(".deepdepcat"),
            workspace.join(".claude"),
        ] {
            out.push(base.join(name));
        }
    }
    out
}

/// Ecosystem rules directories (Claude Code / Cursor style), scanned on
/// top of the instruction files regardless of own-instruction presence —
/// matching the harness-compatibility stance: recognize, don't require.
pub fn ecosystem_rules(workspace: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for dir in [
        workspace.join(".claude").join("rules"),
        workspace.join(".cursor").join("rules"),
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().map(|e| e == "md").unwrap_or(false))
            .collect();
        files.sort();
        for path in files {
            if let Some(content) = read_file_lossy(&path) {
                out.push((path, content));
            }
        }
    }
    out
}

/// User-level instruction candidates, lowest priority (merged below project).
pub fn user_instruction_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.push(user_deepdepcat_dir().join("DEEPDEPCAT.md"));
    if let Some(home) = dirs::home_dir() {
        // Ecosystem compatibility: user-level CLAUDE.md from Claude Code.
        out.push(home.join(".claude").join("CLAUDE.md"));
    }
    out
}

/// Load project instructions, lowest → highest priority order:
/// user-level (DEEPDEPCAT.md, then ~/.claude/CLAUDE.md) → project own
/// (DEEPDEPCAT.md family) → fallback ecosystem (CLAUDE.md family + Codex
/// AGENTS.md, only when no own) → ecosystem rules dirs (.claude/rules,
/// .cursor/rules, always scanned).
pub fn load_project_instructions(workspace: &Path) -> Vec<(PathBuf, String)> {
    let mut instructions = Vec::new();

    for path in user_instruction_candidates() {
        if path.exists() {
            if let Some(content) = read_file_lossy(&path) {
                instructions.push((path, content));
            }
        }
    }

    let own: Vec<PathBuf> = own_instruction_candidates(workspace)
        .into_iter()
        .filter(|p| p.exists())
        .collect();

    if !own.is_empty() {
        for path in own {
            if let Some(content) = read_file_lossy(&path) {
                instructions.push((path, content));
            }
        }
    } else {
        for path in fallback_instruction_candidates(workspace) {
            if path.exists() {
                if let Some(content) = read_file_lossy(&path) {
                    instructions.push((path, content));
                }
            }
        }
    }

    instructions.extend(ecosystem_rules(workspace));

    instructions
}

/// Load project memory: `.deepdepcat/MEMORY.md` (own). AGENTS.md is part
/// of the instruction fallback family (Codex semantics), not memory.
pub fn load_project_memory(workspace: &Path) -> Option<String> {
    let path = workspace.join(".deepdepcat").join("MEMORY.md");
    if path.exists() {
        read_file_lossy(&path)
    } else {
        None
    }
}

/// Load the user profile: `~/.deepdepcat/USER.md` (user-level, workspace
/// independent). The file may contain a `<!-- managed -->` section that the
/// agent may rewrite (see `user_profile_update`); everything outside it is
/// the user's own hand-written content and is injected verbatim.
pub fn load_user_profile() -> Option<String> {
    let path = user_deepdepcat_dir().join("USER.md");
    if path.exists() {
        read_file_lossy(&path)
    } else {
        None
    }
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
    fn own_instructions_win_over_fallback() {
        let tmp = make_dir_with(&[
            (".deepdepcat/DEEPDEPCAT.md", "own rules"),
            ("CLAUDE.md", "claude rules"),
        ]);
        let found = load_project_instructions(tmp.path());
        // Own file must be present and the project CLAUDE.md fallback must
        // NOT be loaded (a user-level ~/.claude/CLAUDE.md may still merge).
        assert!(found.iter().any(|(_, c)| c.contains("own rules")));
        assert!(
            !found.iter().any(|(_, c)| c.contains("claude rules")),
            "fallback must not load when own exists"
        );
    }

    #[test]
    fn fallback_loads_when_no_own_instructions() {
        let tmp = make_dir_with(&[("CLAUDE.md", "claude rules")]);
        let found = load_project_instructions(tmp.path());
        // NOTE: the user-level candidates may include a real ~/.claude/CLAUDE.md
        // on dev machines — assert presence of the project file, not a count.
        assert!(
            found.iter().any(|(_, c)| c.contains("claude rules")),
            "project CLAUDE.md must be loaded as fallback"
        );
    }

    #[test]
    fn legacy_instructions_md_is_an_own_candidate() {
        let tmp = make_dir_with(&[(".deepdepcat/instructions.md", "legacy")]);
        let found = load_project_instructions(tmp.path());
        assert!(found.iter().any(|(p, _)| p.ends_with("instructions.md")));
    }

    #[test]
    fn project_root_deepdepcat_md_is_loaded() {
        let tmp = make_dir_with(&[("DEEPDEPCAT.md", "root rules")]);
        let found = load_project_instructions(tmp.path());
        assert!(found.iter().any(|(_, c)| c.contains("root rules")));
    }

    #[test]
    fn user_level_instructions_merge_below_project() {
        // Redirect the user root to a temp dir via DEEPDEPCAT_HOME.
        let user = tempfile::tempdir().unwrap();
        std::fs::write(user.path().join("DEEPDEPCAT.md"), "user rules").unwrap();
        std::env::set_var("DEEPDEPCAT_HOME", user.path());
        let tmp = make_dir_with(&[(".deepdepcat/DEEPDEPCAT.md", "project rules")]);
        let found = load_project_instructions(tmp.path());
        std::env::remove_var("DEEPDEPCAT_HOME");
        assert_eq!(
            found[0].1, "user rules",
            "user-level merges first (lowest priority)"
        );
        assert!(
            found.iter().any(|(_, c)| c.contains("project rules")),
            "project rules must merge"
        );
    }

    #[test]
    fn gbk_instructions_are_decoded() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("DEEPDEPCAT.md");
        let content = "中文指令：记得使用 GBK 编码的规则文件也要能读";
        let (gbk, _, _) = encoding_rs::GBK.encode(content);
        std::fs::write(&path, gbk.as_ref()).unwrap();
        let found = load_project_instructions(tmp.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].1.contains("中文指令"), "GBK content must decode");
    }

    #[test]
    fn memory_only_reads_own_memory_md() {
        let tmp = make_dir_with(&[
            (".deepdepcat/MEMORY.md", "memory content"),
            ("AGENTS.md", "agents content"),
        ]);
        let memory = load_project_memory(tmp.path());
        assert_eq!(memory.unwrap(), "memory content");

        // AGENTS.md is an INSTRUCTION (Codex semantics), not memory.
        let tmp2 = make_dir_with(&[("AGENTS.md", "agents content")]);
        assert!(load_project_memory(tmp2.path()).is_none());
        let instructions = load_project_instructions(tmp2.path());
        assert!(instructions
            .iter()
            .any(|(_, c)| c.contains("agents content")));
    }

    #[test]
    fn agents_md_loads_as_fallback_instruction() {
        let tmp = make_dir_with(&[("AGENTS.md", "codex project rules")]);
        let found = load_project_instructions(tmp.path());
        assert!(
            found.iter().any(|(_, c)| c.contains("codex project rules")),
            "Codex AGENTS.md must load when no own instruction exists"
        );
    }

    #[test]
    fn ecosystem_rules_dirs_are_scanned() {
        let tmp = make_dir_with(&[
            (".claude/rules/b.md", "claude rule b"),
            (".claude/rules/a.md", "claude rule a"),
            (".cursor/rules/c.md", "cursor rule c"),
            ("DEEPDEPCAT.md", "own"),
        ]);
        let found = load_project_instructions(tmp.path());
        // Rules load even when an own instruction file exists (harness
        // compatibility: recognize, don't require). Sorted within each dir.
        assert!(found.iter().any(|(_, c)| c.contains("claude rule a")));
        assert!(found.iter().any(|(_, c)| c.contains("claude rule b")));
        assert!(found.iter().any(|(_, c)| c.contains("cursor rule c")));
        assert!(found.iter().any(|(_, c)| c.contains("own")));
    }
}
