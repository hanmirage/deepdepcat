//! Permission checker — the main entry point for permission decisions.
//!
//! Combines rules, mode, filesystem validation, and bash security into
//! a single check function.

use crate::core::config::PermissionsSection;
use crate::permissions::denial::DenialTracker;
use crate::permissions::filesystem::FilesystemValidator;
use crate::permissions::mode::PermissionMode;
use crate::permissions::network::NetworkPolicyChecker;
use crate::permissions::rules::{
    parse_rule, AgentPermissionRules, PermissionRule, RuleAction, RuleSet,
};
use crate::permissions::security::bash::{BashSecurity, Severity};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

// Re-exported so `permissions::checker::PermissionResult` (the historical
// path used by dispatch/scheduler) keeps resolving after the type moved to
// `result.rs`. The `pub use` also brings it into scope for this module.
pub use crate::permissions::result::PermissionResult;

/// The permission checker — combines all security layers.
pub struct PermissionChecker {
    /// Settings rules (allow/deny/ask from config) — behind a lock so
    /// `reload_rules` hot-swaps them without a restart (the governance
    /// command writes config.toml then swaps the in-memory set).
    rules: std::sync::RwLock<RuleSet>,
    /// Project-level rules loaded from `.claude/settings.json` and
    /// `settings.local.json` permissions (project settings override the
    /// global config, matching the Claude Code project > user > managed >
    /// default precedence). Kept as an ordered list of per-file layers —
    /// `settings.local.json` overrides `settings.json` on conflict, so the
    /// last file with a matching rule wins.
    project_rules: Mutex<Vec<Vec<PermissionRule>>>,
    fs_validator: FilesystemValidator,
    bash_security: BashSecurity,
    denial_tracker: Mutex<DenialTracker>,
    network: NetworkPolicyChecker,
}

impl Clone for PermissionChecker {
    /// Snapshot clone — copies rules/mode/validators but gives the clone its
    /// own denial tracker (the scheduler runner's unattended gate must not
    /// consume the interactive session's denial budget, and vice versa).
    fn clone(&self) -> Self {
        let project_rules = self
            .project_rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Self {
            rules: std::sync::RwLock::new(
                self.rules.read().unwrap_or_else(|e| e.into_inner()).clone(),
            ),
            project_rules: Mutex::new(project_rules),
            fs_validator: self.fs_validator.clone(),
            bash_security: self.bash_security.clone(),
            denial_tracker: Mutex::new(DenialTracker::new(
                self.denial_tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .max_consecutive(),
            )),
            network: self.network.clone(),
        }
    }
}

impl PermissionChecker {
    pub fn new(config: PermissionsSection) -> Self {
        Self {
            rules: std::sync::RwLock::new(RuleSet::new(&config)),
            project_rules: Mutex::new(Vec::new()),
            fs_validator: FilesystemValidator::new(),
            bash_security: BashSecurity::new(),
            denial_tracker: Mutex::new(DenialTracker::new(config.max_consecutive_denials)),
            network: NetworkPolicyChecker::new(
                &config.network_policy_mode,
                config.network_policy_domains.clone(),
                config.network_allow_private,
            ),
        }
    }

    /// Load `.claude/settings.json` (and `settings.local.json`) permissions
    /// into the project rule layer. Called when the workspace changes; the
    /// rules replace the previous project's (never accumulate across
    /// projects). Each file becomes one layer — a rule in
    /// `settings.local.json` overrides the same call's rule in
    /// `settings.json` (later file wins).
    pub fn load_project_settings(&self, workspace: &Path) {
        let mut layers = Vec::new();
        for file in [".claude/settings.json", ".claude/settings.local.json"] {
            let mut rules = Vec::new();
            let path = workspace.join(file);
            let Ok(text) = std::fs::read_to_string(&path) else {
                layers.push(rules);
                continue;
            };
            let Ok(json) = serde_json::from_str::<Value>(&text) else {
                layers.push(rules);
                continue;
            };
            let Some(perms) = json.get("permissions") else {
                layers.push(rules);
                continue;
            };
            let collect = |key: &str, action: RuleAction| -> Vec<PermissionRule> {
                perms
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.as_str())
                            .filter_map(|s| parse_rule(s, action))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            rules.extend(collect("allow", RuleAction::Allow));
            rules.extend(collect("deny", RuleAction::Deny));
            rules.extend(collect("ask", RuleAction::Ask));
            layers.push(rules);
        }
        *self.project_rules.lock().unwrap_or_else(|e| e.into_inner()) = layers;
    }

    /// Check permissions for a tool call.
    ///
    /// `read_only` is the tool's own `is_read_only()` classification — the
    /// single source of truth for what counts as a read operation. Rule
    /// layers and modes consume it instead of maintaining their own lists.
    /// `session_id` scopes the denial tracker so one session's denials never
    /// trip another session's gate (the unattended scheduler passes a fixed
    /// key of its own).
    pub fn check(
        &self,
        tool_name: &str,
        args: &Value,
        read_only: bool,
        session_id: &str,
    ) -> PermissionResult {
        self.check_with_mode(tool_name, args, read_only, session_id, self.mode())
    }

    /// The full permission pipeline under an EXPLICIT effective mode.
    ///
    /// The dispatcher passes the session's effective mode (a fresh
    /// session-scoped override wins over the global mode) so a worker
    /// spawned from a plan-mode parent can actually write — the rule layer
    /// previously only saw the GLOBAL mode and denied every non-read tool.
    pub fn check_with_mode(
        &self,
        tool_name: &str,
        args: &Value,
        read_only: bool,
        session_id: &str,
        mode: crate::permissions::mode::PermissionMode,
    ) -> PermissionResult {
        self.check_with_agent_rules(tool_name, args, read_only, session_id, mode, None)
    }

    /// The full permission pipeline under an explicit mode plus an agent's
    /// own rules (`.deepdepcat/agents/*.md` permissions). Agent denies are
    /// a hard veto; agent allows/asks refine the normal layers.
    pub fn check_with_agent_rules(
        &self,
        tool_name: &str,
        args: &Value,
        read_only: bool,
        session_id: &str,
        mode: crate::permissions::mode::PermissionMode,
        agent_rules: Option<&AgentPermissionRules>,
    ) -> PermissionResult {
        // ── Bash: evaluate each statement of a compound command ─────────
        // `git pull && rm -rf src` is split so the rules, grants, and
        // security layers each see one statement — a `Bash(git *)` allow
        // rule or `cmd:git` grant can no longer mask a destructive tail.
        let result = if tool_name == "bash" {
            self.check_bash_segments(args, read_only, session_id, mode, agent_rules)
        } else {
            self.check_single(tool_name, args, read_only, session_id, mode, agent_rules)
        };

        // ── Layer 4: Denial Tracking ──────────────────────────────────
        match result {
            PermissionResult::Allow => {
                // Recovery tools are NEVER blocked by the cooldown: a session
                // paused for repeated denials must still be able to exit plan
                // mode, enter it, or ask the user — blocking the escape hatch
                // turned the 60s pause into a deadlock (a PlanExecute session
                // whose `exit_plan_mode` was itself denied by the cooldown
                // stayed read-only until the timer expired and the model
                // happened to retry).
                if self.check_denial_limit(session_id) && !Self::is_recovery_tool(tool_name) {
                    PermissionResult::Deny(
                        "Too many consecutive permission denials — paused for ~60s. \
                         Review what you are doing; the gate auto-resets after the \
                         cooldown."
                            .to_string(),
                    )
                } else {
                    self.record_success(session_id);
                    PermissionResult::Allow
                }
            }
            other => other,
        }
    }

    /// Whether a bare whole-tool deny rule removes `tool_name` from the
    /// model's tool list (settings or project layer). Recovery tools stay
    /// available no matter what — removing the escape hatch would turn a
    /// deny into a deadlock.
    pub fn is_tool_removed(&self, tool_name: &str) -> bool {
        if matches!(tool_name, "ask_user" | "enter_plan_mode" | "exit_plan_mode") {
            return false;
        }
        if self
            .rules
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .has_bare_tool_deny(tool_name)
        {
            return true;
        }
        let project = self.project_rules.lock().unwrap_or_else(|e| e.into_inner());
        project.iter().rev().any(|layer| {
            layer.iter().any(|r| {
                r.action == crate::permissions::rules::RuleAction::Deny
                    && r.pattern == "*"
                    && r.tool.eq_ignore_ascii_case(tool_name)
            })
        })
    }

    /// Whether a tool is a recovery/escape hatch that must stay usable even
    /// during the denial cooldown.
    fn is_recovery_tool(tool_name: &str) -> bool {
        matches!(tool_name, "exit_plan_mode" | "enter_plan_mode" | "ask_user")
    }

    /// Split a bash command into statements and evaluate each through the
    /// full permission pipeline. Precedence across statements mirrors the
    /// layers: deny > ask > allow — any statement that denies denies the
    /// whole command, any statement that asks makes the whole command ask.
    fn check_bash_segments(
        &self,
        args: &Value,
        read_only: bool,
        session_id: &str,
        mode: crate::permissions::mode::PermissionMode,
        agent_rules: Option<&AgentPermissionRules>,
    ) -> PermissionResult {
        let Some(command) = args.get("command").and_then(|c| c.as_str()) else {
            return self.check_single("bash", args, read_only, session_id, mode, agent_rules);
        };
        let mut any_ask = false;
        for segment in self.bash_security.split_commands(command) {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            let seg_args = serde_json::json!({ "command": segment });
            match self.check_single("bash", &seg_args, read_only, session_id, mode, agent_rules) {
                PermissionResult::Deny(reason) => return PermissionResult::Deny(reason),
                PermissionResult::Ask => any_ask = true,
                PermissionResult::Allow => {}
            }
        }
        if any_ask {
            PermissionResult::Ask
        } else {
            PermissionResult::Allow
        }
    }

    /// Run the full permission pipeline (Phases 1-9) for a single tool call.
    /// For bash this is invoked per statement by `check` — each statement is
    /// matched against rules and security phases independently.
    fn check_single(
        &self,
        tool_name: &str,
        args: &Value,
        read_only: bool,
        session_id: &str,
        mode: crate::permissions::mode::PermissionMode,
        agent_rules: Option<&AgentPermissionRules>,
    ) -> PermissionResult {
        // ── Phase 1: Agent contract — deny veto (highest precedence) ──
        // An agent definition deny — or a deny inherited from a parent
        // agent chain — beats every other layer, including project and
        // settings allows. The agent's own contract is a hard boundary.
        if let Some(agent) = agent_rules {
            if agent.deny.iter().any(|r| r.matches(tool_name, args)) {
                self.record_denial(session_id);
                return PermissionResult::Deny("Denied by agent permission rule".to_string());
            }
        }

        // ── Phase 2: Project rules (project settings override the global
        // config, deny beats allow) ────────────────────────────────────
        // An explicit project Allow marks the call as pre-approved; the
        // security phases below still run (bash safety, fs validation).
        let mut project_allowed = false;
        {
            let project = self.project_rules.lock().unwrap_or_else(|e| e.into_inner());
            for layer in project.iter().rev() {
                let mut deny_pattern: Option<String> = None;
                let mut has_allow = false;
                let mut has_ask = false;
                for rule in layer.iter() {
                    if rule.matches(tool_name, args) {
                        match rule.action {
                            RuleAction::Deny => {
                                deny_pattern.get_or_insert_with(|| rule.pattern.clone());
                            }
                            RuleAction::Allow => {
                                has_allow = true;
                            }
                            RuleAction::Ask => {
                                has_ask = true;
                            }
                        }
                    }
                }
                if let Some(pattern) = deny_pattern {
                    self.record_denial(session_id);
                    return PermissionResult::Deny(format!(
                        "Denied by project permission rule ({})",
                        pattern
                    ));
                }
                if has_ask {
                    if mode.auto_accepts() {
                        project_allowed = true;
                    } else {
                        return PermissionResult::Ask;
                    }
                } else if has_allow {
                    project_allowed = true;
                }
                if deny_pattern.is_some() || has_ask || has_allow {
                    break;
                }
            }
        }

        // ── Phase 3: Agent allows ──────────────────────────────────────
        // An agent-specific allow pre-approves matching calls, but never
        // overrides a project/settings DENY (those phases already returned
        // above) and never skips the security phases below.
        let agent_allowed =
            agent_rules.is_some_and(|agent| agent.allow.iter().any(|r| r.matches(tool_name, args)));

        // ── Phase 4: Settings rules + mode ─────────────────────────────
        let rule_action = self
            .rules
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .check_with_mode(tool_name, args, read_only, mode);

        match rule_action {
            RuleAction::Deny => {
                self.record_denial(session_id);
                return PermissionResult::Deny("Denied by permission rule".to_string());
            }
            RuleAction::Allow => {
                // Still need to check filesystem and bash security
            }
            RuleAction::Ask => {
                // A project-level allow overrides the ask (the user wrote
                // the rule into the project settings deliberately).
                if mode.auto_accepts() || project_allowed || agent_allowed {
                    // Fall through to security checks
                } else {
                    // Still need to check for dangerous patterns
                    if self.is_dangerous(tool_name, args) {
                        return PermissionResult::Deny(
                            "Operation is potentially dangerous".to_string(),
                        );
                    }
                    // Built-in safe-command auto-allow: read-only bash
                    // invocations (`ls`, `git status`, `cargo check`, …)
                    // pass without a prompt. Only implicit asks qualify —
                    // an explicit `ask` rule in settings keeps prompting.
                    if self.is_safe_bash_auto_allow(tool_name, args) {
                        // Fall through to security checks (defense in depth).
                    } else {
                        return PermissionResult::Ask;
                    }
                }
            }
        }

        // ── Phase 5: Agent asks ────────────────────────────────────────
        // An agent that explicitly asks for a call forces a prompt even if
        // a settings rule would allow it. Full-access mode still wins — the
        // user explicitly chose to stop prompting.
        if let Some(agent) = agent_rules {
            if !mode.auto_accepts() && agent.ask.iter().any(|r| r.matches(tool_name, args)) {
                return PermissionResult::Ask;
            }
        }

        // ── Phase 6: Filesystem validation ─────────────────────────────
        if let Some(path) = self.extract_path(tool_name, args) {
            match self.fs_validator.validate(&path) {
                crate::permissions::filesystem::ValidationResult::Allow => {}
                crate::permissions::filesystem::ValidationResult::Deny(reason) => {
                    self.record_denial(session_id);
                    return PermissionResult::Deny(reason);
                }
                crate::permissions::filesystem::ValidationResult::Ask => {
                    return PermissionResult::Ask;
                }
            }
        }

        // ── Phase 7: Sensitive-file preflight (VS Code's guard) ────────
        // Editing a secret-bearing file (.env, *.pem, keys, credentials)
        // ALWAYS asks — even in auto-accept / accept-edits modes — so the
        // change is seen before it lands. Runs AFTER the fs validator so
        // hard deny zones (~/.ssh, /etc) keep their Deny precedence. Read
        // access is unaffected; only writes gate.
        if !read_only && self.is_sensitive_edit(tool_name, args) {
            return PermissionResult::Ask;
        }

        // ── Phase 8: Bash security + network ────────────────────────────
        if tool_name == "bash" {
            if let Some(command) = args.get("command").and_then(|c| c.as_str()) {
                // Unified bash security: dangerous first (stricter wins),
                // then suspicious — one severity model, no layer ordering
                // where a Suspicious base verdict shadows a Dangerous one.
                match self.bash_security.analyze(command) {
                    Severity::Safe => {}
                    Severity::Dangerous(reason) => {
                        self.record_denial(session_id);
                        return PermissionResult::Deny(format!("Dangerous command: {}", reason));
                    }
                    Severity::Suspicious(_reason) => {
                        return PermissionResult::Ask;
                    }
                }

                // Network command policy (command-level block/allowlist).
                if let Some(reason) = self.network.check(command) {
                    self.record_denial(session_id);
                    return PermissionResult::Deny(reason);
                }

                // Read-only mode validation (for plan mode).
                if self.mode() == PermissionMode::ReadOnly {
                    match self.bash_security.validate_read_only(command) {
                        Severity::Safe => {}
                        Severity::Dangerous(reason) => {
                            return PermissionResult::Deny(reason);
                        }
                        Severity::Suspicious(_reason) => {
                            return PermissionResult::Ask;
                        }
                    }
                }
            }
        }

        // ── Phase 9: Default allow — no rule, security, or mode gate
        // denied/asked this call; it is allowed.
        PermissionResult::Allow
    }

    /// Get the current permission mode.
    pub fn mode(&self) -> PermissionMode {
        self.rules.read().unwrap_or_else(|e| e.into_inner()).mode()
    }

    /// Force-read-only validation of a bash command, regardless of the
    /// current permission mode.
    ///
    /// Used by EVALUATOR subagents (#88 audit H11): their bash must never
    /// mutate the codebase — `echo x > file` / `rm` / `mv` would bypass the
    /// edit-evidence gates (bash writes never enter agent_edited_paths, so
    /// a modifying evaluator was invisible to verification). Returns `true`
    /// when the command is read-only-safe.
    pub fn is_read_only_bash(&self, command: &str) -> bool {
        matches!(
            self.bash_security.validate_read_only(command),
            Severity::Safe | Severity::Suspicious(_)
        )
    }

    /// Set the permission mode.
    pub fn set_mode(&self, mode: PermissionMode) {
        self.rules
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .set_mode(mode);
    }

    /// Hot-swap the settings rules from a config section — used by the
    /// governance command after persisting `config.toml`, so rule changes
    /// apply to running sessions immediately (no restart).
    pub fn reload_rules(&self, config: &crate::core::config::PermissionsSection) {
        *self.rules.write().unwrap_or_else(|e| e.into_inner()) = RuleSet::new(config);
    }

    fn extract_path(&self, tool_name: &str, args: &Value) -> Option<String> {
        // Single source of truth: every path-bearing tool (core reads,
        // core writes, Depwork readers/writers) is validated here — deny
        // zones, traversal, symlink parents and ask zones cannot be
        // escaped by using a variant tool or a Depwork surface.
        let key = crate::core::pattern::tool_path_field(tool_name)?;
        args.get(key)
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
    }

    fn is_dangerous(&self, tool_name: &str, args: &Value) -> bool {
        if tool_name == "bash" {
            if let Some(command) = args.get("command").and_then(|c| c.as_str()) {
                return self
                    .bash_security
                    .split_commands(command)
                    .iter()
                    .any(|seg| {
                        let seg = seg.trim();
                        !seg.is_empty()
                            && matches!(self.bash_security.analyze(seg), Severity::Dangerous(_))
                    });
            }
        }
        false
    }

    /// Whether this write tool call targets a sensitive file (see
    /// `permissions::sensitive`) — the edit must be confirmed manually.
    fn is_sensitive_edit(&self, tool_name: &str, args: &Value) -> bool {
        crate::permissions::sensitive::is_sensitive_edit_call(tool_name, args)
    }

    /// Whether this Ask can be satisfied by the built-in bash safe-command
    /// whitelist: only implicit asks qualify — a rule-layer `ask` matched by
    /// an explicit settings rule always prompts.
    fn is_safe_bash_auto_allow(&self, tool_name: &str, args: &Value) -> bool {
        if tool_name != "bash" {
            return false;
        }
        if self
            .rules
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ask_rule_matched(tool_name, args)
        {
            return false;
        }
        args.get("command")
            .and_then(|c| c.as_str())
            .map(|cmd| self.bash_security.is_safe_command(cmd))
            .unwrap_or(false)
    }

    fn record_denial(&self, session_id: &str) {
        if let Ok(tracker) = self.denial_tracker.lock() {
            tracker.record_denial(session_id);
        }
    }

    fn record_success(&self, session_id: &str) {
        if let Ok(tracker) = self.denial_tracker.lock() {
            tracker.record_success(session_id);
        }
    }

    fn check_denial_limit(&self, session_id: &str) -> bool {
        if let Ok(tracker) = self.denial_tracker.lock() {
            tracker.exceeded_limit(session_id)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> PermissionChecker {
        PermissionChecker::new(crate::core::config::PermissionsSection::default())
    }

    fn bash_args(command: &str) -> Value {
        serde_json::json!({ "command": command })
    }

    #[test]
    fn write_tools_all_extract_path_for_fs_validation() {
        // Regression for #88 audit H9: search_replace / apply_patch are
        // write tools but were missing from extract_path, so deny rules and
        // traversal checks never applied to them. In AutoAccept mode the
        // rule layer falls through to filesystem validation — every
        // path-carrying write tool must feed the validator there.
        let c = checker();
        c.set_mode(crate::permissions::mode::PermissionMode::FullAccess);
        let home = dirs::home_dir().unwrap();
        let ssh_key = home.join(".ssh").join("id_rsa");
        for tool in ["search_replace", "apply_patch"] {
            let args = serde_json::json!({ "path": ssh_key.to_string_lossy() });
            let result = c.check(tool, &args, false, "test");
            assert!(
                matches!(result, PermissionResult::Deny(_)),
                "{tool} writing into ~/.ssh must be denied in AutoAccept, got {result:?}"
            );
        }
        // Sanity: the read tools keep extracting too.
        for tool in ["read_file", "edit_file", "write_file"] {
            let args = serde_json::json!({ "path": "C:\\safe\\file.txt" });
            let extracted = c.extract_path(tool, &args);
            assert!(extracted.is_some(), "{tool} must extract path");
        }
    }

    #[test]
    fn project_settings_deny_rule_wins() {
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(rm -rf *)"]}}"#,
        )
        .unwrap();

        c.load_project_settings(dir.path());
        // Matched by the project deny rule → denied, even though "rm" alone
        // would not trip the base bash security patterns.
        let result = c.check("bash", &bash_args("rm -rf C:\\temp\\x"), false, "test");
        assert!(matches!(result, PermissionResult::Deny(_)));
    }

    #[test]
    fn project_settings_allow_rule_bypasses_ask() {
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.local.json"),
            r#"{"permissions":{"allow":["Bash(echo *)"]}}"#,
        )
        .unwrap();

        c.load_project_settings(dir.path());
        // `echo hi` is not dangerous; without the allow rule the default
        // mode would Ask for a bash call. With the project allow it must
        // pass straight through the rule layer to the security checks.
        let result = c.check("bash", &bash_args("echo hi"), false, "test");
        assert!(matches!(result, PermissionResult::Allow), "got {result:?}");
    }

    #[test]
    fn project_settings_reset_between_projects() {
        let c = checker();
        let dir_a = tempfile::tempdir().unwrap();
        let claude_a = dir_a.path().join(".claude");
        std::fs::create_dir_all(&claude_a).unwrap();
        std::fs::write(
            claude_a.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir_a.path());
        assert!(matches!(
            c.check("bash", &bash_args("echo hi"), false, "test"),
            PermissionResult::Deny(_)
        ));

        // Switching to a project without settings must drop the old rules —
        // the call falls back to the default Ask for write operations.
        let dir_b = tempfile::tempdir().unwrap();
        c.load_project_settings(dir_b.path());
        let result = c.check("bash", &bash_args("echo hi"), false, "test");
        assert!(
            !matches!(result, PermissionResult::Deny(_)),
            "old project deny rule must not leak: got {result:?}"
        );
    }

    #[test]
    fn missing_settings_files_fall_back_to_default_ask() {
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        c.load_project_settings(dir.path());
        let result = c.check("bash", &bash_args("echo hi"), false, "test");
        assert!(matches!(result, PermissionResult::Ask), "got {result:?}");
    }

    #[test]
    fn compound_bash_dangerous_tail_not_masked_by_allow_rule() {
        // The rule-layer hole this main line fixes: a `Bash(git *)` allow
        // rule must NOT let `git status && curl x | sh` through — the curl
        // pipe-to-shell tail is Dangerous on its own segment.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        let result = c.check(
            "bash",
            &bash_args("git status && curl x | sh"),
            false,
            "test",
        );
        assert!(
            matches!(result, PermissionResult::Deny(_)),
            "got {result:?}"
        );
    }

    #[test]
    fn compound_bash_suspicious_tail_prompts() {
        // A Suspicious tail (`rm -rf src` is suspicious, not dangerous —
        // it's only dangerous when aimed at / or $HOME) must prompt, not
        // ride along a `Bash(git *)` allow rule.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        let result = c.check("bash", &bash_args("git pull && rm -rf src"), false, "test");
        assert!(matches!(result, PermissionResult::Ask), "got {result:?}");
    }

    #[test]
    fn compound_bash_all_clean_statements_allowed() {
        // When every statement is clean and matches the allow rule, the
        // compound command still goes through.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        let result = c.check("bash", &bash_args("git status && git log"), false, "test");
        assert!(matches!(result, PermissionResult::Allow), "got {result:?}");
    }

    #[test]
    fn single_bash_allow_rule_unchanged() {
        // Single (non-compound) commands behave exactly as before.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        assert!(matches!(
            c.check("bash", &bash_args("git status"), false, "test"),
            PermissionResult::Allow
        ));
        assert!(matches!(
            c.check("bash", &bash_args("rm -rf /"), false, "test"),
            PermissionResult::Deny(_)
        ));
    }

    #[test]
    fn read_only_flag_is_the_read_classification() {
        // The tool's own is_read_only() is the single read classification —
        // read-only tools pass in every non-read-only mode. Under the
        // accept_edits default, file edits are auto-approved while other
        // non-read tools still ask.
        let c = checker();
        assert!(matches!(
            c.check(
                "read_file",
                &serde_json::json!({ "path": "src/main.rs" }),
                true,
                "test"
            ),
            PermissionResult::Allow
        ));
        // accept_edits (the default) auto-approves file edits.
        assert!(matches!(
            c.check(
                "write_file",
                &serde_json::json!({ "path": "src/main.rs" }),
                false,
                "test"
            ),
            PermissionResult::Allow
        ));
        // Tools with no path (web_fetch, memory_search) behave the same way.
        assert!(matches!(
            c.check(
                "web_fetch",
                &serde_json::json!({ "url": "https://a.b" }),
                true,
                "test"
            ),
            PermissionResult::Allow
        ));
        // A non-read, non-edit tool still asks.
        assert!(matches!(
            c.check(
                "todo_write",
                &serde_json::json!({ "todos": [] }),
                false,
                "test"
            ),
            PermissionResult::Ask
        ));
    }

    #[test]
    fn project_deny_beats_read_only_allow() {
        // A read-only tool must still be denied by a project deny rule —
        // read classification is NOT a bypass for rule-layer denials.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"deny":["Read(**/.env)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        let result = c.check(
            "read_file",
            &serde_json::json!({ "path": "a/.env" }),
            true,
            "test",
        );
        assert!(
            matches!(result, PermissionResult::Deny(_)),
            "got {result:?}"
        );
    }

    #[test]
    fn read_only_mode_uses_flag_instead_of_list() {
        // Read-only mode: any tool classified read-only passes; everything
        // else is denied — including tools missing from any hardcoded list.
        let c = checker();
        c.set_mode(crate::permissions::mode::PermissionMode::ReadOnly);
        assert!(matches!(
            c.check(
                "read_file_pdf",
                &serde_json::json!({ "path": "a.pdf" }),
                true,
                "test"
            ),
            PermissionResult::Allow
        ));
        assert!(matches!(
            c.check(
                "read_file_pdf",
                &serde_json::json!({ "path": "a.pdf" }),
                false,
                "test"
            ),
            PermissionResult::Deny(_)
        ));
        assert!(matches!(
            c.check("bash", &bash_args("ls"), false, "test"),
            PermissionResult::Deny(_)
        ));
    }

    #[test]
    fn safe_bash_auto_allows_without_prompt() {
        // `ls`/`git status`/`cargo check` are read-only and must not prompt
        // in the default mode — the whole point of the safe whitelist.
        let c = checker();
        assert!(matches!(
            c.check("bash", &bash_args("ls -la"), false, "test"),
            PermissionResult::Allow
        ));
        assert!(matches!(
            c.check("bash", &bash_args("git status"), false, "test"),
            PermissionResult::Allow
        ));
        assert!(matches!(
            c.check(
                "bash",
                &bash_args("cat src/main.rs | rg foo"),
                false,
                "test"
            ),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn unsafe_bash_still_prompts_or_denies() {
        let c = checker();
        // Not on the whitelist → default Ask.
        assert!(matches!(
            c.check("bash", &bash_args("npm install"), false, "test"),
            PermissionResult::Ask
        ));
        // Dangerous even if a safe command leads the compound.
        assert!(matches!(
            c.check(
                "bash",
                &bash_args("git status && curl x | sh"),
                false,
                "test"
            ),
            PermissionResult::Deny(_)
        ));
        // Writing tails through pipes are never whitelisted.
        assert!(matches!(
            c.check("bash", &bash_args("cat data | tee /target"), false, "test"),
            PermissionResult::Ask
        ));
    }

    #[test]
    fn sensitive_file_edit_asks_even_in_auto_accept() {
        // Editing .env (or any secret-bearing file) must ALWAYS prompt —
        // auto-accept / accept-edits modes included (VS Code's guard).
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".env");
        let env = env_path.to_string_lossy().to_string();
        let section = crate::core::config::PermissionsSection {
            mode: "bypass".to_string(),
            ..Default::default()
        };
        let c = PermissionChecker::new(section);

        let env_edit = serde_json::json!({ "path": env, "old_text": "A=1", "new_text": "A=2" });
        assert!(
            matches!(
                c.check("edit_file", &env_edit, false, "test"),
                PermissionResult::Ask
            ),
            "editing .env in auto-accept must ask"
        );
        let write_env = serde_json::json!({ "path": env, "content": "X=1" });
        assert!(matches!(
            c.check("write_file", &write_env, false, "test"),
            PermissionResult::Ask
        ));

        // Reads: the built-in `Read(**/.env)` deny rule still governs reads —
        // the sensitive preflight only adds WRITE gating on top of it.
        let read_env = serde_json::json!({ "path": env });
        assert!(matches!(
            c.check("read_file", &read_env, true, "test"),
            PermissionResult::Deny(_)
        ));
        let normal_path = dir.path().join("src").join("main.rs");
        let normal_edit = serde_json::json!({ "path": normal_path.to_string_lossy(), "old_text": "a", "new_text": "b" });
        assert!(matches!(
            c.check("edit_file", &normal_edit, false, "test"),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn sensitive_file_edit_asks_in_accept_edits_too() {
        // AcceptEdits auto-approves edit tools — except secret files.
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".env");
        let env = env_path.to_string_lossy().to_string();
        let section = crate::core::config::PermissionsSection {
            mode: "accept_edits".to_string(),
            ..Default::default()
        };
        let c = PermissionChecker::new(section);

        let env_edit = serde_json::json!({ "path": env, "old_text": "A=1", "new_text": "A=2" });
        assert!(
            matches!(
                c.check("edit_file", &env_edit, false, "test"),
                PermissionResult::Ask
            ),
            "AcceptEdits must not silently edit .env"
        );
        let normal_path = dir.path().join("src").join("main.rs");
        let normal_edit = serde_json::json!({ "path": normal_path.to_string_lossy(), "old_text": "a", "new_text": "b" });
        assert!(matches!(
            c.check("edit_file", &normal_edit, false, "test"),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn sensitive_depwork_writes_ask_even_in_bypass() {
        // The sensitive-file red line must cover Depwork file writers too:
        // a bypass-mode agent must still not silently create/overwrite
        // .env / key files through the document tools.
        let section = crate::core::config::PermissionsSection {
            mode: "bypass".to_string(),
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        let env_str = env.to_string_lossy().to_string();

        // path-key writers (docx_generate / ppt / xlsx / pdf / card ...)
        assert!(matches!(
            c.check(
                "docx_generate",
                &serde_json::json!({ "path": env_str }),
                false,
                "test"
            ),
            PermissionResult::Ask
        ));
        // output-key writers (chart_generate / media / pdf_tools)
        assert!(matches!(
            c.check(
                "chart_generate",
                &serde_json::json!({ "output": env_str }),
                false,
                "test"
            ),
            PermissionResult::Ask
        ));
        // output_path-key writers (table_process)
        assert!(matches!(
            c.check(
                "table_process",
                &serde_json::json!({ "output_path": env_str }),
                false,
                "test"
            ),
            PermissionResult::Ask
        ));
        // Ordinary Depwork writes stay auto-allowed in bypass.
        let normal = dir.path().join("report.docx");
        assert!(matches!(
            c.check(
                "docx_generate",
                &serde_json::json!({ "path": normal.to_string_lossy() }),
                false,
                "test"
            ),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn depwork_writes_hit_filesystem_deny_zones() {
        // Depwork writers must not escape the filesystem validator: a
        // docx_generate targeting ~/.ssh is hard-denied, even in bypass.
        let section = crate::core::config::PermissionsSection {
            mode: "bypass".to_string(),
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        let home = dirs::home_dir().unwrap();
        let target = home.join(".ssh").join("evil.docx");
        assert!(matches!(
            c.check(
                "docx_generate",
                &serde_json::json!({ "path": target.to_string_lossy() }),
                false,
                "test"
            ),
            PermissionResult::Deny(_)
        ));
    }

    #[test]
    fn explicit_ask_rule_beats_safe_whitelist() {
        // A user-configured `ask` rule keeps prompting even for a command on
        // the safe whitelist — explicit policy wins over the built-in list.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"ask":["Bash(ls *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        // Project ask rules return Ask directly from layer 0.
        assert!(matches!(
            c.check("bash", &bash_args("ls -la"), false, "test"),
            PermissionResult::Ask
        ));

        // Settings-level ask rules (config.ask) must also prompt.
        let section = crate::core::config::PermissionsSection {
            ask: vec!["Bash(cat *)".to_string()],
            ..Default::default()
        };
        let c2 = PermissionChecker::new(section);
        assert!(matches!(
            c2.check("bash", &bash_args("cat main.rs"), false, "test"),
            PermissionResult::Ask
        ));
    }

    #[test]
    fn denial_limit_is_session_scoped() {
        // One session's denial burst must not trip another session's gate —
        // sessions share the checker but keep isolated denial budgets.
        let section = crate::core::config::PermissionsSection {
            max_consecutive_denials: 2,
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        // Project deny rules record a denial for the session they denied.
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());
        for _ in 0..2 {
            assert!(matches!(
                c.check("bash", &bash_args("echo hi"), false, "sess_a"),
                PermissionResult::Deny(_)
            ));
        }
        // Session A is now at its limit: an otherwise-allowable call denies.
        assert!(matches!(
            c.check("bash", &bash_args("ls"), false, "sess_a"),
            PermissionResult::Deny(_)
        ));
        // Session B shares the checker but has its own budget: `ls` allows.
        assert!(matches!(
            c.check("bash", &bash_args("ls"), false, "sess_b"),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn cooldown_never_blocks_recovery_tools() {
        // A tripped session must still be able to exit plan mode, enter it,
        // or ask the user — blocking the escape hatch turned the 60s pause
        // into a deadlock (the real session where `exit_plan_mode` itself
        // was denied by the cooldown and the agent stayed read-only).
        let section = crate::core::config::PermissionsSection {
            max_consecutive_denials: 2,
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());

        // Trip the session with two rule denials.
        for _ in 0..2 {
            assert!(matches!(
                c.check("bash", &bash_args("echo hi"), false, "sess"),
                PermissionResult::Deny(_)
            ));
        }
        // Control: an otherwise-allowable call is now cooldown-denied.
        assert!(matches!(
            c.check("bash", &bash_args("ls"), false, "sess"),
            PermissionResult::Deny(_)
        ));
        // Recovery tools stay usable during the cooldown.
        assert!(
            matches!(
                c.check("exit_plan_mode", &serde_json::json!({}), true, "sess"),
                PermissionResult::Allow
            ),
            "exit_plan_mode must never be blocked by the cooldown"
        );
        assert!(matches!(
            c.check("enter_plan_mode", &serde_json::json!({}), true, "sess"),
            PermissionResult::Allow
        ));
        assert!(matches!(
            c.check("ask_user", &serde_json::json!({}), true, "sess"),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn session_override_accept_edits_wins_over_global_plan() {
        // Plan-mode delegation: the parent is globally read-only, but a
        // worker spawned to execute the task gets an accept-edits session
        // override. The rule layer must honor the override — otherwise the
        // worker's edits are denied by the global plan and delegation is
        // pointless.
        let section = crate::core::config::PermissionsSection {
            mode: "plan".to_string(),
            ..Default::default()
        };
        let c = PermissionChecker::new(section);

        // Global plan: writes denied.
        assert!(matches!(
            c.check_with_mode(
                "edit_file",
                &serde_json::json!({ "path": "a.rs" }),
                false,
                "worker",
                crate::permissions::mode::PermissionMode::ReadOnly
            ),
            PermissionResult::Deny(_)
        ));
        // Session override accept-edits: edits auto-approved, safe reads
        // approved, nothing auto-denied by the global plan.
        assert!(matches!(
            c.check_with_mode(
                "edit_file",
                &serde_json::json!({ "path": "a.rs" }),
                false,
                "worker",
                crate::permissions::mode::PermissionMode::AcceptEdits
            ),
            PermissionResult::Allow
        ));
        assert!(matches!(
            c.check_with_mode(
                "bash",
                &bash_args("ls"),
                false,
                "worker",
                crate::permissions::mode::PermissionMode::AcceptEdits
            ),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn local_project_settings_override_settings_json() {
        // `settings.local.json` is the user's own override file: a rule
        // there beats the same call's rule in `settings.json` (later file
        // wins), in both directions.
        let c = checker();
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        std::fs::write(
            claude.join("settings.local.json"),
            r#"{"permissions":{"allow":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        c.load_project_settings(dir.path());
        assert!(
            matches!(
                c.check("bash", &bash_args("echo hi"), false, "test"),
                PermissionResult::Allow
            ),
            "local allow must override settings.json deny"
        );

        // And the reverse: a local deny overrides a settings.json allow.
        let c2 = checker();
        let dir2 = tempfile::tempdir().unwrap();
        let claude2 = dir2.path().join(".claude");
        std::fs::create_dir_all(&claude2).unwrap();
        std::fs::write(
            claude2.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        std::fs::write(
            claude2.join("settings.local.json"),
            r#"{"permissions":{"deny":["Bash(echo *)"]}}"#,
        )
        .unwrap();
        c2.load_project_settings(dir2.path());
        assert!(
            matches!(
                c2.check("bash", &bash_args("echo hi"), false, "test"),
                PermissionResult::Deny(_)
            ),
            "local deny must override settings.json allow"
        );
    }

    #[test]
    fn agent_deny_veto_beats_settings_allow() {
        // An agent definition deny must win over a global allow — the
        // agent's own contract is a hard boundary.
        let section = crate::core::config::PermissionsSection {
            allow: vec!["Bash(git *)".to_string()],
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        let agent = crate::permissions::rules::AgentPermissionRules::from_lists(
            &[],
            &["Bash(git *)".to_string()],
            &[],
        );
        assert!(matches!(
            c.check_with_agent_rules(
                "bash",
                &bash_args("git push origin main"),
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Deny(_)
        ));
    }

    #[test]
    fn agent_deny_stays_scoped_to_matching_calls() {
        let c = checker();
        let agent = crate::permissions::rules::AgentPermissionRules::from_lists(
            &[],
            &["Bash(rm *)".to_string()],
            &[],
        );
        assert!(matches!(
            c.check_with_agent_rules(
                "bash",
                &bash_args("rm -rf src"),
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Deny(_)
        ));
        // The deny never leaks to unrelated calls.
        assert!(matches!(
            c.check_with_agent_rules(
                "read_file",
                &serde_json::json!({ "path": "src/main.rs" }),
                true,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn agent_deny_applies_per_bash_statement() {
        let c = checker();
        let agent = crate::permissions::rules::AgentPermissionRules::from_lists(
            &[],
            &["Bash(rm *)".to_string()],
            &[],
        );
        // The compound command's rm tail is denied even though git pull
        // alone would pass.
        assert!(matches!(
            c.check_with_agent_rules(
                "bash",
                &bash_args("git pull && rm -rf src"),
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Deny(_)
        ));
    }

    #[test]
    fn agent_allow_preapproves_settings_ask() {
        let section = crate::core::config::PermissionsSection {
            ask: vec!["Bash(npm *)".to_string()],
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        let agent = crate::permissions::rules::AgentPermissionRules::from_lists(
            &["Bash(npm *)".to_string()],
            &[],
            &[],
        );
        assert!(matches!(
            c.check_with_agent_rules(
                "bash",
                &bash_args("npm install"),
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Allow
        ));
    }

    #[test]
    fn agent_ask_forces_prompt_despite_settings_allow() {
        let section = crate::core::config::PermissionsSection {
            allow: vec!["Bash(npm *)".to_string()],
            ..Default::default()
        };
        let c = PermissionChecker::new(section);
        let agent = crate::permissions::rules::AgentPermissionRules::from_lists(
            &[],
            &[],
            &["Bash(npm *)".to_string()],
        );
        assert!(matches!(
            c.check_with_agent_rules(
                "bash",
                &bash_args("npm install"),
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Ask
        ));
    }

    #[test]
    fn agent_allow_never_bypasses_sensitive_file_gate() {
        let c = checker();
        let agent = crate::permissions::rules::AgentPermissionRules::from_lists(
            &["Edit(*)".to_string()],
            &[],
            &[],
        );
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".env");
        let env = env_path.to_string_lossy().to_string();
        let args = serde_json::json!({ "path": env, "old_text": "A=1", "new_text": "A=2" });
        assert!(matches!(
            c.check_with_agent_rules(
                "edit_file",
                &args,
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Ask
        ));
    }

    #[test]
    fn inherited_parent_denies_propagate_to_worker() {
        // A worker without its own definition still carries the parent's
        // deny chain — compiled as denies-only agent rules.
        let c = checker();
        let mut agent = crate::permissions::rules::AgentPermissionRules::default();
        agent.merge_denies(&["Edit(**/.env)".to_string()]);
        assert!(matches!(
            c.check_with_agent_rules(
                "edit_file",
                &serde_json::json!({ "path": "a/.env", "old_text": "x", "new_text": "y" }),
                false,
                "s1",
                crate::permissions::mode::PermissionMode::AcceptEdits,
                Some(&agent)
            ),
            PermissionResult::Deny(_)
        ));
    }
}
