//! Skill activation engine — manages conditional skill activation based on
//! workspace file changes.
//!
//! When a tool touches a file (read/write/edit), the activation engine checks
//! if any skills should be activated based on their `paths` patterns.
//! Activated skills have their content injected into the agent's context.
//!
//! Path matching uses gitignore-style semantics:
//! - Patterns match against paths relative to the workspace root.
//! - Negation patterns (`!` prefix) exclude files from activating a skill.
//! - Backslashes are normalized to forward slashes for cross-platform matching.

use crate::skills::types::Skill;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// The skill activation engine — tracks which skills are active based on
/// workspace file changes.
#[derive(Clone)]
pub struct SkillActivationEngine {
    all_skills: Arc<RwLock<Vec<Skill>>>,
    active_skill_ids: Arc<RwLock<HashSet<String>>>,
    touched_files: Arc<RwLock<Vec<String>>>,
    /// The workspace root, used to convert absolute paths to relative
    /// before glob matching.
    workspace_root: Arc<RwLock<Option<PathBuf>>>,
}

impl SkillActivationEngine {
    /// Create a new skill activation engine.
    pub fn new() -> Self {
        Self {
            all_skills: Arc::new(RwLock::new(Vec::new())),
            active_skill_ids: Arc::new(RwLock::new(HashSet::new())),
            touched_files: Arc::new(RwLock::new(Vec::new())),
            workspace_root: Arc::new(RwLock::new(None)),
        }
    }

    /// Load all available skills.
    pub async fn load_skills(&self, skills: Vec<Skill>) {
        let mut all = self.all_skills.write().await;
        *all = skills;
    }

    /// Snapshot of every loaded skill — used to render the prompt-side skill
    /// inventory. A clone, so callers never hold the lock across awaits.
    pub async fn all_skills(&self) -> Vec<Skill> {
        self.all_skills.read().await.clone()
    }

    /// Record that a file was touched (read, written, or modified).
    ///
    /// This triggers re-evaluation of which skills should be active.
    /// The path is stored as-is; matching converts to relative if
    /// a workspace root is set.
    pub async fn record_file_touch(&self, path: &Path) {
        let path_str = path.to_string_lossy().to_string();

        {
            let mut touched = self.touched_files.write().await;
            if !touched.contains(&path_str) {
                touched.push(path_str);
            }
        }

        // Re-evaluate skill activation.
        self.evaluate_activation().await;
    }

    /// Evaluate which skills should be active based on touched files.
    async fn evaluate_activation(&self) {
        let skills = self.all_skills.read().await;
        let touched = self.touched_files.read().await;
        let workspace_root = self.workspace_root.read().await;
        let touched_refs: Vec<String> = touched.clone();
        drop(touched);

        let mut active = self.active_skill_ids.write().await;

        for skill in skills.iter() {
            if !skill.enabled {
                continue;
            }

            let matched = if skill.paths.is_empty() {
                // No path criterion. A skill that only declares `when-to-use`
                // keywords is gated by the user message (activate_for_message)
                // — only a skill with NEITHER paths NOR keywords is
                // unconditionally active.
                skill.when_to_use.is_empty()
            } else if let Some(ref root) = *workspace_root {
                skill.matches_paths_relative(&touched_refs, root)
            } else {
                skill.matches_paths(&touched_refs)
            };

            if matched {
                active.insert(skill.id.clone());
            }
        }
    }

    /// Reset ALL activation state (active ids + touched files) to empty.
    ///
    /// Called once per TURN (this engine is a single app-global Arc shared by
    /// every session and work mode) so a skill activated by a previous message
    /// or workspace does not leak into later conversations. `activate_for_message`
    /// re-adds keyword skills per message; `record_file_touch` re-adds
    /// path-based skills as tools touch matching files during the turn.
    pub async fn reset_activation(&self) {
        self.active_skill_ids.write().await.clear();
        self.touched_files.write().await.clear();
    }

    /// Set the workspace root used to relativize absolute tool paths before
    /// glob matching. Without it, path patterns like `docs/**/*.md` never
    /// match an absolute path (`C:/proj/docs/guide.md`) because the glob is
    /// full-string.
    pub async fn set_workspace_root(&self, workspace: PathBuf) {
        *self.workspace_root.write().await = Some(workspace);
    }

    /// Activate skills whose `when-to-use` keywords appear in the user's
    /// message (case-insensitive substring match, OR across keywords).
    /// Mode-gated: a skill declared for another work mode never activates
    /// here. Called at turn start so a content skill (e.g. a 小红书 template
    /// with no `paths`) activates as soon as the user asks for that content,
    /// not only after a file touch.
    pub async fn activate_for_message(
        &self,
        message: &str,
        mode: crate::toolkit::WorkMode,
    ) {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return;
        }
        let lower = trimmed.to_lowercase();
        let skills = self.all_skills.read().await;
        let mut active = self.active_skill_ids.write().await;
        for skill in skills.iter() {
            if !skill.enabled {
                continue;
            }
            if !skill.work_modes.is_empty()
                && !skill.work_modes.iter().any(|m| m == mode.as_str())
            {
                continue;
            }
            if skill.when_to_use.iter().any(|k| {
                let k = k.trim().to_lowercase();
                !k.is_empty() && lower.contains(&k)
            }) {
                active.insert(skill.id.clone());
            }
        }
    }

    /// Get the content of all active skills, concatenated.
    ///
    /// This content is injected into the agent's system prompt. Template
    /// variables (`${{ SKILL_DIR }}`, `${{ SESSION_ID }}`, ...) are rendered
    /// per session at injection time. Skills declared for a different work
    /// mode than `mode` are skipped (empty `work_modes` = all modes).
    pub async fn get_active_skills_content(
        &self,
        session_id: Option<&str>,
        mode: crate::toolkit::WorkMode,
    ) -> String {
        let active_ids = self.active_skill_ids.read().await;
        let skills = self.all_skills.read().await;
        let workspace_root = self.workspace_root.read().await;

        let mut content = String::new();
        for skill in skills.iter() {
            if active_ids.contains(&skill.id)
                && (skill.work_modes.is_empty()
                    || skill.work_modes.iter().any(|m| m == mode.as_str()))
            {
                if !content.is_empty() {
                    content.push_str("\n\n---\n\n");
                }
                let vars = crate::skills::template::vars_for_skill(
                    skill.file_path.as_deref(),
                    session_id,
                    workspace_root.as_deref(),
                    "",
                );
                let rendered = crate::skills::template::render_skill_content(&skill.content, &vars);
                content.push_str(&format!("## Skill: {}\n\n{}", skill.name, rendered));
            }
        }

        content
    }
}

impl Default for SkillActivationEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a tool name is a file-touching tool.
pub fn is_file_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file" | "write_file" | "edit_file" | "search_replace" | "list_dir"
    )
}

/// Extract the file path from a tool call's arguments.
pub fn extract_file_path(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "search_replace" => {
            args.get("path").and_then(|p| p.as_str()).map(String::from)
        }
        "list_dir" => args.get("path").and_then(|p| p.as_str()).map(String::from),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{Skill, SkillSource};
    use crate::toolkit::WorkMode;

    fn skill(id: &str, when_to_use: Vec<String>, work_modes: Vec<String>) -> Skill {
        Skill {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            content: String::new(),
            model: None,
            allowed_tools: vec![],
            permission_mode: None,
            paths: vec![],
            work_modes,
            when_to_use,
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn message_keyword_activates_matching_skill() {
        let engine = SkillActivationEngine::new();
        engine
            .load_skills(vec![skill(
                "xiaohongshu",
                vec!["小红书".to_string(), "笔记".to_string()],
                vec!["depwork".to_string()],
            )])
            .await;
        engine
            .activate_for_message("帮我写一篇小红书笔记", WorkMode::Depwork)
            .await;
        assert!(engine.active_skill_ids.read().await.contains("xiaohongshu"));
    }

    #[tokio::test]
    async fn message_keyword_respects_work_mode() {
        let engine = SkillActivationEngine::new();
        engine
            .load_skills(vec![skill(
                "xiaohongshu",
                vec!["小红书".to_string()],
                vec!["depwork".to_string()],
            )])
            .await;
        // A Code session must not activate a depwork-only skill even when the
        // message mentions its keyword.
        engine
            .activate_for_message("帮我写一篇小红书笔记", WorkMode::Code)
            .await;
        assert!(!engine.active_skill_ids.read().await.contains("xiaohongshu"));
    }

    #[tokio::test]
    async fn empty_message_activates_nothing() {
        let engine = SkillActivationEngine::new();
        engine
            .load_skills(vec![skill("x", vec!["小红书".to_string()], vec![])])
            .await;
        engine.activate_for_message("", WorkMode::Depwork).await;
        assert!(engine.active_skill_ids.read().await.is_empty());
    }

    #[tokio::test]
    async fn keyword_gated_skill_not_unconditionally_active() {
        // A skill with `when-to-use` but no `paths` must NOT be active until
        // the message mentions its keywords (no path touch, no keyword → off).
        let engine = SkillActivationEngine::new();
        engine
            .load_skills(vec![skill(
                "zhihu",
                vec!["知乎".to_string()],
                vec![],
            )])
            .await;
        let active = engine.active_skill_ids.read().await;
        assert!(!active.contains("zhihu"), "keyword-gated skill starts off");
    }

    #[tokio::test]
    async fn reset_clears_activation_between_turns() {
        // The engine is app-global and shared across sessions: a skill
        // activated by one message must not leak into the next turn/session.
        let engine = SkillActivationEngine::new();
        engine
            .load_skills(vec![skill(
                "xiaohongshu",
                vec!["小红书".to_string()],
                vec!["depwork".to_string()],
            )])
            .await;
        engine
            .activate_for_message("帮我写一篇小红书笔记", WorkMode::Depwork)
            .await;
        assert!(engine.active_skill_ids.read().await.contains("xiaohongshu"));

        // Next turn: reset wipes it; a message without the keyword stays off.
        engine.reset_activation().await;
        assert!(engine.active_skill_ids.read().await.is_empty());
        engine
            .activate_for_message("帮我修个 bug", WorkMode::Depwork)
            .await;
        assert!(
            !engine.active_skill_ids.read().await.contains("xiaohongshu"),
            "skill must not leak across turns"
        );
    }

    #[tokio::test]
    async fn absolute_tool_path_activates_with_workspace_root() {
        // `workspace_root` must be set for path patterns to match ABSOLUTE
        // tool paths (which is what the dispatcher resolves). Without it, a
        // pattern `docs/**/*.md` never matches `C:/proj/docs/guide.md`.
        let mut s = skill("docs", vec![], vec![]);
        s.paths = vec!["docs/**/*.md".to_string()];
        let engine = SkillActivationEngine::new();
        engine.load_skills(vec![s]).await;
        engine
            .set_workspace_root(std::path::PathBuf::from(r"C:\proj"))
            .await;
        engine
            .record_file_touch(std::path::Path::new(r"C:\proj\docs\guide.md"))
            .await;
        assert!(
            engine.active_skill_ids.read().await.contains("docs"),
            "absolute path must activate the path skill once workspace_root is set"
        );
    }
}
