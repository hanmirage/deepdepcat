//! Cross-cutting reminders for tool outputs.
//!
//! Provides contextual hints wrapped in `<system-reminder>` tags that are
//! appended to tool outputs before being sent to the model — mirroring the
//! upstream reminders architecture (LSP diagnostics, task completion, skill
//! discovery).
//!
//! Each reminder is a small evaluator: given the tool that just ran and its
//! output, it may produce a hint string. The dispatcher collects all
//! reminders from registered evaluators and appends them to the tool result.

/// Wrap plain text in `<system-reminder>` tags.
///
/// Input:  `"Some reminder text"`
/// Output: `"<system-reminder>\nSome reminder text\n</system-reminder>"`
pub fn wrap_reminder(text: &str) -> String {
    format!("<system-reminder>\n{text}\n</system-reminder>")
}

/// Append wrapped reminders to a tool output string.
///
/// Returns output unchanged when no reminders were produced. Each reminder
/// is individually wrapped, then joined and appended after a blank line.
pub fn format_with_reminders(output: String, reminders: Vec<String>) -> String {
    if reminders.is_empty() {
        return output;
    }
    let wrapped: Vec<String> = reminders.iter().map(|r| wrap_reminder(r)).collect();
    let joined = wrapped.join("\n\n");
    if output.is_empty() {
        joined
    } else {
        format!("{output}\n\n{joined}")
    }
}

/// The cross-cutting reminder evaluator.
///
/// Implementations inspect the tool call and produce a hint when relevant.
/// Evaluators are registered on the dispatcher and run after every tool call.
#[async_trait::async_trait]
pub trait Reminder: Send + Sync {
    /// Evaluate the tool call and return a reminder hint, or `None`.
    async fn evaluate(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        output: &str,
        session_id: &str,
        workspace: Option<&std::path::Path>,
    ) -> Option<String>;
}

/// Reminder: warn when a file tool produced empty output.
///
/// Reading an empty file is often a signal the file was just created, is
/// generated, or the path is wrong — the model benefits from a nudge.
pub struct EmptyOutputReminder;

#[async_trait::async_trait]
impl Reminder for EmptyOutputReminder {
    async fn evaluate(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        output: &str,
        _session_id: &str,
        _workspace: Option<&std::path::Path>,
    ) -> Option<String> {
        if tool_name == "read_file" && output.trim().is_empty() {
            // A windowed read (offset/limit) with no output usually means the
            // offset is beyond the file's line count, not an empty file —
            // telling the model "the file is empty" sent it down the
            // "unreadable/encoding problem" path (session 2d02f3dc). The
            // tool itself now returns an explicit offset message; only a
            // FULL read that yields nothing is a genuinely empty file.
            let windowed = args
                .get("offset")
                .and_then(|v| v.as_u64())
                .map(|o| o > 0)
                .unwrap_or(false)
                || args.get("limit").and_then(|v| v.as_u64()).is_some();
            return Some(if windowed {
                "The requested line window is empty — the offset likely exceeds \
                 the file's line count. Re-read with offset 0 to see the actual \
                 content before concluding anything about the file."
                    .to_string()
            } else {
                "The file you read is empty. Verify the path is correct; an \
                 unexpectedly empty file may indicate it was just created, is \
                 generated at build time, or contains only whitespace."
                    .to_string()
            });
        }
        if tool_name == "list_dir" && output.trim().is_empty() {
            return Some(
                "The directory listing is empty. Confirm the path exists and \
                 is not a file; empty directories are a valid but notable result."
                    .to_string(),
            );
        }
        None
    }
}

/// Reminder: surface project skill guidance when work falls under a skill.
///
/// Wired directly to the skill activation engine: whenever at least one
/// skill is active, its guidance is appended to tool outputs so the model
/// keeps the project's skills in view while working.
///
/// The same guidance is injected at most once per [`SKILL_GUIDANCE_INTERVAL`]
/// per session — tool results in the same turn would otherwise each carry a
/// full copy (N tools × full skill text = massive context bloat, and a
/// repeated instruction block becomes echo bait for the model to parrot
/// verbatim). New skill activations change the content hash and re-emit
/// immediately.
pub struct SkillGuidanceReminder {
    engine: std::sync::Arc<crate::skills::activation::SkillActivationEngine>,
    /// Work mode filter — skills declared for the other mode are skipped.
    work_mode: crate::toolkit::WorkMode,
    /// session_id → (content hash, last emission time).
    last_emitted: std::sync::Mutex<std::collections::HashMap<String, (u64, std::time::Instant)>>,
}

/// Minimum gap between identical skill-guidance injections per session.
const SKILL_GUIDANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

impl SkillGuidanceReminder {
    pub fn new(
        engine: std::sync::Arc<crate::skills::activation::SkillActivationEngine>,
        work_mode: crate::toolkit::WorkMode,
    ) -> Self {
        Self {
            engine,
            work_mode,
            last_emitted: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn content_hash(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }
}

#[async_trait::async_trait]
impl Reminder for SkillGuidanceReminder {
    async fn evaluate(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
        _output: &str,
        session_id: &str,
        _workspace: Option<&std::path::Path>,
    ) -> Option<String> {
        let guidance = self
            .engine
            .get_active_skills_content(Some(session_id), self.work_mode)
            .await;
        if guidance.trim().is_empty() {
            return None;
        }

        let hash = Self::content_hash(&guidance);
        let now = std::time::Instant::now();
        let mut state = self.last_emitted.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((last_hash, last_at)) = state.get(session_id) {
            if *last_hash == hash && now.duration_since(*last_at) < SKILL_GUIDANCE_INTERVAL {
                return None;
            }
        }
        state.insert(session_id.to_string(), (hash, now));
        drop(state);

        Some(format!(
            "Active project skills apply to this work:\n{guidance}"
        ))
    }
}

/// Reminder: detect task completion signals in bash output and nudge
/// verification before the agent declares the task done.
///
/// Mirrors the upstream task-completion surface: a command that reports
/// success is not proof the user's goal is met. The reminder steers the
/// model toward the verification-first discipline (CONSTRAINT 7): confirm
/// the change works, review the diff, then summarize concrete results.
///
/// False-positive guards: only `bash` output is scanned, and `test result:
/// ok` with zero passing tests (e.g. an empty test binary) does not count
/// as a completion signal.
pub struct CompletionSignalReminder;

impl CompletionSignalReminder {
    fn detect_signal(output: &str) -> Option<&'static str> {
        let lower = output.to_lowercase();
        if let Some(idx) = lower.find("test result: ok") {
            let tail = &lower[idx..];
            // "0 passed" must be matched as a WHOLE count, not a substring of
            // "10 passed"/"20 passed" (a run with 10 passing tests would
            // otherwise be read as zero). The cargo/pytest format always has a
            // space before the count, so a leading-space check disambiguates.
            if tail.contains("passed") && !tail.contains(" 0 passed") {
                return Some("tests passed");
            }
        }
        const SIGNALS: [&str; 5] = [
            "all tests passed",
            "build successful",
            "build succeeded",
            "compilation finished",
            "done. all tasks completed",
        ];
        SIGNALS.iter().find(|s| lower.contains(**s)).copied()
    }
}

#[async_trait::async_trait]
impl Reminder for CompletionSignalReminder {
    async fn evaluate(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
        output: &str,
        _session_id: &str,
        _workspace: Option<&std::path::Path>,
    ) -> Option<String> {
        if tool_name != "bash" {
            return None;
        }
        Self::detect_signal(output).map(|signal| {
            format!(
                "Task completion signal detected ({signal}). The command \
                 reported success — but that is not proof the user's goal is \
                 met. Before declaring the task done: re-read the goal, verify \
                 the change actually works (build/run/test), review the diff, \
                 and only then summarize concrete results for the user."
            )
        })
    }
}

/// Reminder: after a write tool edits a file, query the LSP server for
/// diagnostics and surface errors as a `<system-reminder>`.
///
/// Mirrors the upstream LSP diagnostics reminder: the model learns about
/// type/compile errors without running a build. Only errors/warnings are
/// surfaced — capped to avoid flooding the context.
///
/// Since Round 16 this reminder also attempts a throttled COLD START: the
/// first edit after a session starts tries `get_or_init` (bounded by a
/// timeout); if the server binary is missing or the start fails, the
/// attempt is remembered per workspace and retried at most every
/// [`COLD_START_RETRY_INTERVAL`] — a missing server never blocks or
/// retries on every single edit.
const MAX_REMINDER_DIAGNOSTICS: usize = 8;
/// How long to wait for a cold-started LSP server before giving up.
const COLD_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Minimum gap between cold-start attempts per workspace.
const COLD_START_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

pub struct DiagnosticsReminder {
    manager: std::sync::Arc<crate::tools::builtin::lsp::LspManager>,
    /// workspace → last cold-start attempt time (throttles retries).
    last_cold_start:
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, std::time::Instant>>,
}

impl DiagnosticsReminder {
    pub fn new(manager: std::sync::Arc<crate::tools::builtin::lsp::LspManager>) -> Self {
        Self {
            manager,
            last_cold_start: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Attempt a cold start, respecting the per-workspace throttle.
    async fn try_cold_start(&self, workspace: &std::path::Path) {
        {
            let mut attempts = self
                .last_cold_start
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(last) = attempts.get(workspace) {
                if last.elapsed() < COLD_START_RETRY_INTERVAL {
                    return;
                }
            }
            attempts.insert(workspace.to_path_buf(), std::time::Instant::now());
        }
        match tokio::time::timeout(COLD_START_TIMEOUT, self.manager.get_or_init(workspace)).await {
            Ok(Ok(_)) => {
                tracing::info!(workspace = %workspace.display(), "LSP server cold-started by diagnostics reminder");
            }
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "LSP cold start failed (server may be missing)");
            }
            Err(_) => {
                tracing::debug!(workspace = %workspace.display(), "LSP cold start timed out");
            }
        }
    }
}

#[async_trait::async_trait]
impl Reminder for DiagnosticsReminder {
    async fn evaluate(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        _output: &str,
        _session_id: &str,
        workspace: Option<&std::path::Path>,
    ) -> Option<String> {
        // Only write-like tools that carry a `path` argument trigger this.
        if !matches!(
            tool_name,
            "edit_file" | "write_file" | "search_replace" | "apply_patch"
        ) {
            return None;
        }
        let path = args.get("path").and_then(|p| p.as_str())?;
        let file = std::path::Path::new(path);
        file.extension()?;

        // Locate the workspace: absolute paths must live under an active
        // workspace; relative paths resolve against the agent's configured
        // workspace root first (that is the root the edit tools resolved
        // them against), falling back to the first active client's root.
        let workspaces = self.manager.client_workspaces();
        let workspace = if file.is_absolute() {
            workspaces
                .iter()
                .find(|ws| file.starts_with(ws))
                .cloned()
                .or_else(|| {
                    // Absolute path outside any running client — use the
                    // agent's workspace if it contains the file.
                    workspace
                        .filter(|ws| file.starts_with(ws))
                        .map(|ws| ws.to_path_buf())
                })
        } else {
            workspace
                .map(|ws| ws.to_path_buf())
                .or_else(|| workspaces.first().cloned())
        };

        // Throttled cold start: no client running for this workspace yet —
        // try to start one once per COLD_START_RETRY_INTERVAL. The lsp tool
        // itself still cold-starts unconditionally; this path only happens
        // on the first edit of a session.
        let workspace = match workspace {
            Some(ws) => ws,
            None => return None,
        };
        if self.manager.get(&workspace).is_none() {
            self.try_cold_start(&workspace).await;
        }

        let client = self.manager.get(&workspace)?;
        let file_abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            workspace.join(file)
        };
        let language_id = crate::tools::builtin::lsp::client::language_id_for_path(&file_abs);

        let diags = match client.diagnostics(&file_abs, language_id).await {
            Ok(d) => d,
            Err(_) => return None,
        };

        // Only errors and warnings, capped.
        let relevant: Vec<&crate::tools::builtin::lsp::LspDiagnostic> = diags
            .iter()
            .filter(|d| matches!(d.severity.as_str(), "error" | "warning"))
            .take(MAX_REMINDER_DIAGNOSTICS)
            .collect();
        if relevant.is_empty() {
            return None;
        }

        let errors = relevant.iter().filter(|d| d.severity == "error").count();
        let warnings = relevant.len() - errors;
        let file_display = file_abs.to_string_lossy();
        let mut hint = format!(
            "LSP diagnostics for {file_display} ({errors} error(s), {warnings} warning(s) shown):"
        );
        for d in &relevant {
            hint.push_str(&format!(
                "\n- [{}] line {}: {}",
                d.severity,
                d.line + 1,
                d.message
            ));
        }
        hint.push_str(
            "\nAddress the errors before declaring the edit done; run the lsp tool for full diagnostics.",
        );
        Some(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrap_reminder_adds_tags() {
        let result = wrap_reminder("Some reminder text");
        assert_eq!(
            result,
            "<system-reminder>\nSome reminder text\n</system-reminder>"
        );
    }

    #[test]
    fn format_with_reminders_wraps_and_appends() {
        let output = "file content here".to_string();
        let reminders = vec!["File is empty.".to_string(), "Check path.".to_string()];
        let result = format_with_reminders(output, reminders);
        assert!(result.starts_with("file content here\n\n"));
        assert!(result.contains("<system-reminder>\nFile is empty.\n</system-reminder>"));
        assert!(result.contains("<system-reminder>\nCheck path.\n</system-reminder>"));
    }

    #[test]
    fn format_with_reminders_returns_unchanged_when_empty() {
        let output = "file content here".to_string();
        let result = format_with_reminders(output.clone(), vec![]);
        assert_eq!(result, output);
    }

    #[tokio::test]
    async fn empty_output_reminder_fires_on_empty_read() {
        let reminder = EmptyOutputReminder;
        let hit = reminder
            .evaluate("read_file", &json!({"path": "a.txt"}), "", "s1", None)
            .await;
        assert!(hit.is_some());
        let miss = reminder
            .evaluate(
                "read_file",
                &json!({"path": "a.txt"}),
                "content",
                "s1",
                None,
            )
            .await;
        assert!(miss.is_none());
        let hit_dir = reminder
            .evaluate("list_dir", &json!({"path": "/tmp/x"}), "", "s1", None)
            .await;
        assert!(hit_dir.is_some());
        let miss_other = reminder
            .evaluate("bash", &json!({"command": "ls"}), "", "s1", None)
            .await;
        assert!(miss_other.is_none());
    }

    #[tokio::test]
    async fn skill_guidance_reminder_injects_active_skills() {
        use crate::skills::types::{Skill, SkillSource};

        let engine = std::sync::Arc::new(crate::skills::activation::SkillActivationEngine::new());
        let reminder =
            SkillGuidanceReminder::new(engine.clone(), crate::toolkit::WorkMode::Code);
        let miss = reminder
            .evaluate("edit_file", &json!({}), "ok", "s1", None)
            .await;
        assert!(miss.is_none(), "no active skills → no reminder");

        engine
            .load_skills(vec![Skill {
                id: "s1".to_string(),
                name: "Rust".to_string(),
                description: String::new(),
                content: "Follow rust best practices.".to_string(),
                model: None,
                allowed_tools: vec![],
                permission_mode: None,
                paths: vec![],
                work_modes: vec![],
                when_to_use: vec![],
                source: SkillSource::Bundled,
                file_path: None,
                enabled: true,
            }])
            .await;
        engine
            .record_file_touch(std::path::Path::new("src/main.rs"))
            .await;

        let hit = reminder
            .evaluate("edit_file", &json!({}), "ok", "s1", None)
            .await;
        let hint = hit.expect("active skill → reminder");
        assert!(hint.contains("Follow rust best practices."));

        // Dedup: the identical guidance must not be re-injected for the
        // same session within the interval (a 6-tool turn must not carry
        // 6 copies of the skill text).
        let second = reminder
            .evaluate("edit_file", &json!({}), "ok", "s1", None)
            .await;
        assert!(
            second.is_none(),
            "identical guidance deduplicated per session"
        );
    }

    #[tokio::test]
    async fn skill_guidance_reemits_when_content_changes() {
        use crate::skills::types::{Skill, SkillSource};

        let engine = std::sync::Arc::new(crate::skills::activation::SkillActivationEngine::new());
        let reminder =
            SkillGuidanceReminder::new(engine.clone(), crate::toolkit::WorkMode::Code);

        engine
            .load_skills(vec![Skill {
                id: "s1".to_string(),
                name: "Rust".to_string(),
                description: String::new(),
                content: "Follow rust best practices.".to_string(),
                model: None,
                allowed_tools: vec![],
                permission_mode: None,
                paths: vec![],
                work_modes: vec![],
                when_to_use: vec![],
                source: SkillSource::Bundled,
                file_path: None,
                enabled: true,
            }])
            .await;
        engine
            .record_file_touch(std::path::Path::new("src/main.rs"))
            .await;

        let first = reminder
            .evaluate("edit_file", &json!({}), "ok", "s1", None)
            .await;
        assert!(first.is_some());

        // A second active skill changes the content → new hash → re-emit.
        engine
            .load_skills(vec![Skill {
                id: "s2".to_string(),
                name: "Debug".to_string(),
                description: String::new(),
                content: "Use systematic debugging.".to_string(),
                model: None,
                allowed_tools: vec![],
                permission_mode: None,
                paths: vec![],
                work_modes: vec![],
                when_to_use: vec![],
                source: SkillSource::Bundled,
                file_path: None,
                enabled: true,
            }])
            .await;
        engine
            .record_file_touch(std::path::Path::new("src/debug.rs"))
            .await;

        let second = reminder
            .evaluate("edit_file", &json!({}), "ok", "s1", None)
            .await;
        assert!(second.is_some(), "content change → re-injected");
        assert!(second.unwrap().contains("Use systematic debugging."));
    }

    #[tokio::test]
    async fn completion_signal_fires_on_passing_tests() {
        let reminder = CompletionSignalReminder;
        let hit = reminder
            .evaluate(
                "bash",
                &json!({"command": "cargo test"}),
                "test result: ok. 12 passed; 0 failed; 0 ignored",
                "s1",
                None,
            )
            .await;
        assert!(hit.is_some());
        let hint = hit.unwrap();
        assert!(hint.contains("tests passed"));
        assert!(hint.contains("verify"));
    }

    #[tokio::test]
    async fn completion_signal_ignores_zero_tests() {
        let reminder = CompletionSignalReminder;
        let hit = reminder
            .evaluate(
                "bash",
                &json!({"command": "cargo test"}),
                "test result: ok. 0 passed; 0 failed; 0 ignored",
                "s1",
                None,
            )
            .await;
        assert!(
            hit.is_none(),
            "zero passing tests is not a completion signal"
        );
    }

    #[tokio::test]
    async fn completion_signal_fires_on_double_digit_pass_count() {
        // "10 passed" contains "0 passed" as a substring — a naive substring
        // check would read a passing run as zero tests and never fire.
        let reminder = CompletionSignalReminder;
        let hit = reminder
            .evaluate(
                "bash",
                &json!({"command": "cargo test"}),
                "test result: ok. 10 passed; 0 failed; 0 ignored",
                "s1",
                None,
            )
            .await;
        assert!(hit.is_some(), "10 passed is a passing run, not zero tests");
    }

    #[tokio::test]
    async fn completion_signal_fires_on_build_success() {
        let reminder = CompletionSignalReminder;
        let hit = reminder
            .evaluate(
                "bash",
                &json!({"command": "npm run build"}),
                "✓ built in 3.2s\nBUILD SUCCESSFUL",
                "s1",
                None,
            )
            .await;
        assert!(hit.is_some());
    }

    #[tokio::test]
    async fn completion_signal_scoped_to_bash_only() {
        let reminder = CompletionSignalReminder;
        let hit = reminder
            .evaluate(
                "read_file",
                &json!({"path": "a.txt"}),
                "test result: ok. 5 passed",
                "s1",
                None,
            )
            .await;
        assert!(hit.is_none(), "signals only scanned on bash output");
        let miss = reminder
            .evaluate(
                "bash",
                &json!({"command": "ls"}),
                "test result: FAILED. 1 passed; 1 failed",
                "s1",
                None,
            )
            .await;
        assert!(miss.is_none());
    }
}
