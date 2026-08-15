//! Skill types.

use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Ecosystem limits (aligned with the harness-compat reference) ─────────
/// Max frontmatter bytes read from a SKILL.md.
pub const MAX_FRONTMATTER_BYTES: usize = 4096;
/// Max body bytes peeked for description derivation.
pub const MAX_BODY_PEEK_BYTES: usize = 2048;
/// Max skill name length.
pub const MAX_NAME_LEN: usize = 64;
/// Max description length.
pub const MAX_DESCRIPTION_LEN: usize = 1024;

/// Vendor-shipped default skills that must never leak in from the
/// ecosystem directories. Dual condition: the physical path contains the
/// Claude vendor segment (`/.claude/`) AND the name matches — a user's own
/// `~/.deepdepcat/skills/shell` is untouched.
pub fn is_vendor_default_skill(path: &Path, name: &str) -> bool {
    let path_str = path.to_string_lossy();
    let in_claude = path_str.contains("/.claude/") || path_str.contains("\\.claude\\");
    let lower = name.to_ascii_lowercase();
    const CLAUDE_DEFAULTS: &[&str] = &["pdf", "docx", "xlsx", "pptx", "skill-creator"];
    in_claude && CLAUDE_DEFAULTS.contains(&lower.as_str())
}

/// The source of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Bundled,
    File,
    Plugin,
}

/// A skill definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The skill content (system prompt or instruction set).
    pub content: String,
    /// Optional model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional restricted tool list.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_tools: Vec<String>,
    /// Optional permission mode override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Glob patterns for conditional activation (e.g. `**/*.rs`, `src/**`).
    /// When non-empty, the skill only activates if a workspace file matches.
    ///
    /// Patterns starting with `!` are negation patterns — a file matching
    /// a negation pattern deactivates the skill even if it also matches
    /// a positive pattern.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub paths: Vec<String>,
    /// Work modes this skill belongs to (`code` / `depwork`). Empty = all
    /// modes. A skill declared for the other mode is never injected.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub work_modes: Vec<String>,
    /// Keywords that activate this skill when the user's message mentions
    /// them (`when-to-use` frontmatter). Empty = not keyword-activated — a
    /// skill with neither `paths` nor `when_to_use` is unconditionally
    /// active in its modes.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub when_to_use: Vec<String>,
    /// The source of this skill.
    pub source: SkillSource,
    /// The file path (for file-based skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Whether the skill is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Compiled path pattern — either positive (match) or negative (negate).
#[derive(Debug, Clone)]
struct CompiledPattern {
    pattern: Pattern,
    is_negation: bool,
}

impl Skill {
    /// Check if this skill should be active given a set of workspace file paths.
    ///
    /// Returns `true` when:
    /// - `paths` is empty (unconditional skill), or
    /// - any file path matches at least one positive glob pattern AND
    ///   does not match any negation pattern.
    ///
    /// Paths are normalized to be relative to the workspace root before
    /// matching, so patterns like `src/**/*.rs` work regardless of the
    /// absolute path prefix.
    pub fn matches_paths(&self, workspace_files: &[String]) -> bool {
        if self.paths.is_empty() {
            return true;
        }

        // Compile patterns once.
        let compiled: Vec<CompiledPattern> = self
            .paths
            .iter()
            .filter_map(|p| {
                let (pattern_str, is_neg) = if let Some(stripped) = p.strip_prefix('!') {
                    (stripped, true)
                } else {
                    (p.as_str(), false)
                };
                Pattern::new(pattern_str)
                    .ok()
                    .map(|pattern| CompiledPattern {
                        pattern,
                        is_negation: is_neg,
                    })
            })
            .collect();

        let positive: Vec<&CompiledPattern> = compiled.iter().filter(|c| !c.is_negation).collect();
        let negative: Vec<&CompiledPattern> = compiled.iter().filter(|c| c.is_negation).collect();

        for file in workspace_files {
            let normalized = normalize_path(file);

            // Check positive patterns — at least one must match.
            let matched = positive.iter().any(|c| c.pattern.matches(&normalized));
            if !matched {
                continue;
            }

            // Check negation patterns — if any matches, this file doesn't qualify.
            let negated = negative.iter().any(|c| c.pattern.matches(&normalized));
            if negated {
                continue;
            }

            return true;
        }

        false
    }

    /// Check if this skill should be active given file paths relative to
    /// a workspace root.
    ///
    /// This is the preferred entry point when the workspace root is known,
    /// as it ensures patterns match against relative paths.
    pub fn matches_paths_relative(&self, files: &[String], workspace_root: &Path) -> bool {
        if self.paths.is_empty() {
            return true;
        }

        let relative_files: Vec<String> = files
            .iter()
            .filter_map(|f| {
                let path = Path::new(f);
                path.strip_prefix(workspace_root)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .or_else(|| {
                        // If the path is already relative, use it directly.
                        if path.is_relative() {
                            Some(path.to_string_lossy().replace('\\', "/"))
                        } else {
                            None
                        }
                    })
            })
            .collect();

        self.matches_paths(&relative_files)
    }
}

/// Normalize a file path for glob matching:
/// - Convert backslashes to forward slashes
/// - Strip leading "./" if present
fn normalize_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("./") {
        stripped.to_string()
    } else {
        normalized
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(paths: Vec<String>) -> Skill {
        Skill {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: String::new(),
            content: String::new(),
            model: None,
            allowed_tools: vec![],
            permission_mode: None,
            paths,
            work_modes: vec![],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        }
    }

    #[test]
    fn empty_paths_matches_always() {
        let skill = make_skill(vec![]);
        assert!(skill.matches_paths(&[]));
        assert!(skill.matches_paths(&["foo.rs".to_string()]));
    }

    #[test]
    fn glob_pattern_matches_rust_files() {
        let skill = make_skill(vec!["**/*.rs".to_string()]);
        assert!(skill.matches_paths(&["src/main.rs".to_string()]));
        assert!(skill.matches_paths(&["lib.rs".to_string()]));
        assert!(!skill.matches_paths(&["main.py".to_string()]));
    }

    #[test]
    fn multiple_patterns_any_match() {
        let skill = make_skill(vec!["**/*.rs".to_string(), "**/*.ts".to_string()]);
        assert!(skill.matches_paths(&["app.ts".to_string()]));
        assert!(skill.matches_paths(&["lib.rs".to_string()]));
        assert!(!skill.matches_paths(&["app.py".to_string()]));
    }

    #[test]
    fn negation_pattern_excludes() {
        let skill = make_skill(vec!["**/*.rs".to_string(), "!**/test_*.rs".to_string()]);
        assert!(skill.matches_paths(&["src/main.rs".to_string()]));
        assert!(!skill.matches_paths(&["src/test_helper.rs".to_string()]));
        assert!(!skill.matches_paths(&["tests/test_foo.rs".to_string()]));
    }

    #[test]
    fn backslash_paths_normalized() {
        let skill = make_skill(vec!["src/**/*.rs".to_string()]);
        assert!(skill.matches_paths(&["src\\main.rs".to_string()]));
    }

    #[test]
    fn relative_path_matching() {
        let skill = make_skill(vec!["src/**/*.rs".to_string()]);
        let root = Path::new("/home/user/project");
        let files = vec!["/home/user/project/src/main.rs".to_string()];
        assert!(skill.matches_paths_relative(&files, root));
    }
}
