//! Skill template rendering — `${{ var }}` substitution in SKILL.md content.
//!
//! Skills are static instruction files, but their instructions often need
//! session context: where the skill lives, the current session ID, the
//! workspace root. Placeholders are substituted at injection time (not at
//! load time) so one skill file works across sessions and workspaces.
//!
//! Syntax:
//! - `${{ SKILL_DIR }}` — directory containing the skill file
//! - `${{ SESSION_ID }}` — current agent session ID
//! - `${{ WORKSPACE }}` — workspace root path (or "none")
//! - `${{ ARGS }}` — invocation arguments (empty string by default)
//! - `${{ DATE }}` — current UTC date (YYYY-MM-DD)
//! - `$${{` — escape, produces a literal `${{`
//!
//! Unknown placeholders are left verbatim so a typo cannot silently
//! corrupt instructions.

use std::path::Path;

/// Variables available to skill templates.
#[derive(Debug, Clone, Default)]
pub struct SkillVars {
    /// Absolute path of the directory containing the skill file.
    pub skill_dir: Option<String>,
    /// Current session ID.
    pub session_id: Option<String>,
    /// Workspace root path.
    pub workspace: Option<String>,
    /// Invocation arguments.
    pub args: String,
}

/// Render `${{ VAR }}` placeholders in a skill body.
pub fn render_skill_content(content: &str, vars: &SkillVars) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(start) = rest.find("${{") {
        // Escape: `$${{` produces a literal `${{` — the placeholder is
        // preceded by a `$`, so drop one dollar and keep `{{` verbatim.
        if start >= 1 && rest.as_bytes()[start - 1] == b'$' {
            out.push_str(&rest[..start - 1]);
            out.push_str("${{");
            rest = &rest[start + 3..];
            continue;
        }

        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                let rendered = resolve(name, vars);
                out.push_str(&rendered);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("${{");
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Resolve a single variable name.
fn resolve(name: &str, vars: &SkillVars) -> String {
    let upper = name.to_ascii_uppercase();
    match upper.as_str() {
        // Claude Code compatibility aliases — ecosystem skills reference
        // ${CLAUDE_SKILL_DIR} / ${CLAUDE_SESSION_ID} in their scripts.
        "SKILL_DIR" | "CLAUDE_SKILL_DIR" => vars.skill_dir.clone().unwrap_or_default(),
        "SESSION_ID" | "CLAUDE_SESSION_ID" => vars.session_id.clone().unwrap_or_default(),
        "WORKSPACE" => vars.workspace.clone().unwrap_or_else(|| "none".to_string()),
        "ARGS" => vars.args.clone(),
        "DATE" => chrono::Utc::now().format("%Y-%m-%d").to_string(),
        _ => name.to_string(),
    }
}

/// Build vars for a file-based skill.
pub fn vars_for_skill(
    file_path: Option<&str>,
    session_id: Option<&str>,
    workspace: Option<&Path>,
    args: &str,
) -> SkillVars {
    let skill_dir = file_path
        .map(Path::new)
        .and_then(|p| p.parent())
        .map(|d| d.to_string_lossy().into_owned());
    SkillVars {
        skill_dir,
        session_id: session_id.map(|s| s.to_string()),
        workspace: workspace.map(|w| w.to_string_lossy().into_owned()),
        args: args.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_known_vars() {
        let vars = SkillVars {
            skill_dir: Some("/tmp/skills/commit".into()),
            session_id: Some("sess-123".into()),
            workspace: Some("/work/proj".into()),
            args: "feat: x".into(),
        };
        let out = render_skill_content(
            "dir=${{ SKILL_DIR }}, id=${{SESSION_ID}}, ws=${{ WORKSPACE }}, a=${{ ARGS }}",
            &vars,
        );
        assert_eq!(
            out,
            "dir=/tmp/skills/commit, id=sess-123, ws=/work/proj, a=feat: x"
        );
    }

    #[test]
    fn unknown_var_left_verbatim() {
        let out = render_skill_content("x=${{ NOPE }}", &SkillVars::default());
        assert_eq!(out, "x=NOPE");
    }

    #[test]
    fn escape_produces_literal() {
        let out = render_skill_content("$${{ SKILL_DIR }}", &SkillVars::default());
        assert_eq!(out, "${{ SKILL_DIR }}");
    }

    #[test]
    fn unclosed_brace_is_kept() {
        let out = render_skill_content("x=${{ NO_CLOSE", &SkillVars::default());
        assert_eq!(out, "x=${{ NO_CLOSE");
    }

    #[test]
    fn date_renders_iso() {
        let vars = SkillVars::default();
        let out = render_skill_content("d=${{ DATE }}", &vars);
        assert!(out.starts_with("d=20"));
        assert_eq!(out.len(), "d=20YY-MM-DD".len());
    }

    #[test]
    fn vars_for_skill_derives_dir() {
        let vars = vars_for_skill(Some("/a/b/c/SKILL.md"), Some("s1"), None, "");
        assert_eq!(vars.skill_dir.as_deref(), Some("/a/b/c"));
        assert_eq!(vars.session_id.as_deref(), Some("s1"));
        assert_eq!(vars.workspace.as_deref(), None);
    }
}
