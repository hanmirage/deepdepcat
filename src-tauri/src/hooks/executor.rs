//! Hook executor — runs hooks for a given event.
//!
//! Supports 4 execution types:
//! - **Command**: Runs a shell command. Exit code 0 = allow, non-zero = deny.
//! - **Prompt**: Sends a prompt to the LLM. Response parsed for allow/deny.
//! - **Agent**: Spawns a subagent to evaluate the event.
//! - **Http**: Sends an HTTP POST to a URL. Response body parsed for allow/deny.

use crate::hooks::eval::{AgentEvaluator, PromptEvaluator};
use crate::hooks::json_directive::{apply_directives, parse_directives};
use crate::hooks::registry::HookRegistry;
use crate::hooks::trust::HookTrustStore;
use crate::hooks::types::{GateOutcome, HookContext, HookDefinition, HookResult, HookType};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, info, warn};

// regex is used for wildcard pattern matching in conditions.

/// A sink for async-hook wake-ups: when an `async_rewake` hook exits with
/// code 2, the executor pushes the message here and the agent loop drains
/// it at its next iteration, so the model can react mid-turn.
pub trait AsyncHookWakeSink: Send + Sync {
    fn push_wake(&self, session_id: &str, message: String);
}

/// Default in-memory wake buffer — the AppState field
/// (`Arc<Mutex<HashMap<session, Vec<String>>>>`) backs the sink directly;
/// the impl is on the inner Mutex so `Arc<T> → Arc<dyn>` coercion works.
impl AsyncHookWakeSink for tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>> {
    fn push_wake(&self, session_id: &str, message: String) {
        if let Ok(mut map) = self.try_lock() {
            map.entry(session_id.to_string())
                .or_default()
                .push(message);
        } else {
            warn!(session_id, "Async hook wake buffer busy — wake dropped");
        }
    }
}

/// Windows PowerShell executable for command hooks — prefer PowerShell 7
/// (pwsh) when installed, fall back to the always-present Windows PowerShell
/// 5.1 (`powershell.exe`). A machine without pwsh must not silently fail
/// JSON-emitting hooks. Probed once, then cached.
fn win_powershell() -> &'static str {
    static HAS_PWSH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *HAS_PWSH.get_or_init(|| {
        std::process::Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }) {
        "pwsh"
    } else {
        "powershell"
    }
}

/// The hook executor — runs hooks and collects results.
#[derive(Clone)]
pub struct HookExecutor {
    registry: Arc<RwLock<HookRegistry>>,
    prompt_evaluator: Option<Arc<dyn PromptEvaluator>>,
    agent_evaluator: Option<Arc<dyn AgentEvaluator>>,
    trust_store: Option<Arc<HookTrustStore>>,
    wake_sink: Option<std::sync::Arc<dyn AsyncHookWakeSink>>,
}

impl HookExecutor {
    pub fn new(registry: Arc<RwLock<HookRegistry>>) -> Self {
        Self {
            registry,
            prompt_evaluator: None,
            agent_evaluator: None,
            trust_store: None,
            wake_sink: None,
        }
    }

    /// Attach the async-hook wake sink. Without one, `async_rewake` hooks
    /// still run in the background but their exit-2 messages are dropped.
    pub fn with_wake_sink(mut self, sink: std::sync::Arc<dyn AsyncHookWakeSink>) -> Self {
        self.wake_sink = Some(sink);
        self
    }

    /// Attach the persistent trust store. Hooks whose current content hash
    /// is not trusted are SKIPPED (never executed) — the Codex "review and
    /// trust before running" model.
    pub fn with_trust_store(mut self, store: Arc<HookTrustStore>) -> Self {
        self.trust_store = Some(store);
        self
    }

    /// Set the prompt evaluator for prompt-type hooks.
    pub fn with_prompt_evaluator(mut self, evaluator: Arc<dyn PromptEvaluator>) -> Self {
        self.prompt_evaluator = Some(evaluator);
        self
    }

    /// Set the agent evaluator for agent-type hooks.
    pub fn with_agent_evaluator(mut self, evaluator: Arc<dyn AgentEvaluator>) -> Self {
        self.agent_evaluator = Some(evaluator);
        self
    }

    /// Execute all hooks for a given event.
    ///
    /// Hooks are cloned out of the registry under a short-lived write lock,
    /// then executed without holding the lock — so hook commands that take
    /// seconds (or time out) never block concurrent hook registration. The
    /// write lock (vs read) is required because `once` hooks are claimed
    /// ATOMICALLY here: removed from the registry when their condition
    /// matches, so a parallel tool batch cannot clone + execute them once
    /// per tool.
    ///
    /// For blocking events (PreToolUse, PreLLMCall, PreCompaction),
    /// iteration stops at the first `allow: false`. Hook failures
    /// (timeout, crash) are recorded but treated as allow (fail-open).
    ///
    /// Hooks are deduplicated by `(hook_type, content, condition)` — if
    /// multiple hooks of the same type have the same content and condition,
    /// only the first one executes.
    pub async fn execute(&self, context: &HookContext) -> Vec<HookResult> {
        let hooks = {
            let mut registry = self.registry.write().unwrap_or_else(|e| e.into_inner());
            let all = registry.get_hooks(&context.event).to_vec();
            // `once` hooks must be claimed ATOMICALLY: a parallel tool batch
            // runs concurrent execute() calls, and if each clones the same
            // `once` hook before any removal lands, it fires once per tool
            // instead of once total. Claim matching once hooks here, under
            // the registry lock, so only one caller ever gets them.
            // Condition-nonmatching ones stay registered for a later event.
            for hook in &all {
                if hook.once {
                    let matches = hook
                        .condition
                        .as_deref()
                        .map(|c| self.evaluate_condition(c, context))
                        .unwrap_or(true);
                    if matches {
                        registry.remove_hooks_by_key(&context.event, &hook.dedup_key());
                    }
                }
            }
            all
        };

        if hooks.is_empty() {
            return vec![];
        }

        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for hook in &hooks {
            if !hook.enabled {
                continue;
            }

            // Hash-based trust gate: an untrusted hook must not execute.
            // Skipped hooks fail open for blocking events (the operation
            // proceeds without the hook's gate), matching "skipped until
            // trusted" semantics.
            if let Some(ref store) = self.trust_store {
                if !store.is_trusted(hook) {
                    debug!(
                        event = %context.event,
                        fingerprint = %crate::hooks::trust::fingerprint(hook),
                        "Hook is not trusted — skipping execution"
                    );
                    continue;
                }
            }

            // Deduplication: skip if we've already executed an identical hook.
            let dedup_key = hook.dedup_key();
            if !seen.insert(dedup_key.clone()) {
                debug!(key = %dedup_key, "Skipping duplicate hook");
                continue;
            }

            if let Some(ref condition) = hook.condition {
                if !self.evaluate_condition(condition, context) {
                    debug!(condition = %condition, "Hook condition not met");
                    continue;
                }
            }

            let result = self.execute_hook(hook, context).await;
            results.push(result.clone());

            if context.event.is_blocking() && !result.allow {
                warn!(reason = ?result.deny_reason, "Hook denied operation");
                break;
            }
        }

        results
    }

    /// Execute hooks for a blocking event and return a gate decision.
    ///
    /// Returns `Ok(GateOutcome)` when all hooks allow (or no hooks are
    /// registered) — the outcome carries any rewritten tool input and
    /// injected context the hooks produced.
    /// Returns `Err(reason)` when any hook explicitly denies.
    /// Hook failures (timeout, crash) are fail-open and do **not**
    /// produce an `Err` — they are logged and the gate stays open.
    /// Blocking errors (exit code 2) produce an `Err` so the harness can
    /// surface the error to the agent.
    pub async fn execute_gate(&self, context: &HookContext) -> Result<GateOutcome, String> {
        let results = self.execute(context).await;
        for result in &results {
            if !result.allow {
                return Err(result
                    .deny_reason
                    .clone()
                    .unwrap_or_else(|| "Denied by hook".to_string()));
            }
        }
        for result in &results {
            if let Some(ref reason) = result.blocking_error {
                return Err(format!("Blocking hook error: {reason}"));
            }
        }
        let mut outcome = GateOutcome::default();
        for result in &results {
            if let Some(ref input) = result.updated_input {
                outcome.updated_input = Some(input.clone());
            }
            if let Some(ref context) = result.additional_context {
                outcome.additional_context.push(context.clone());
            }
        }
        Ok(outcome)
    }

    /// Execute hooks for an observe-only event (PostToolUse, etc.).
    ///
    /// Results are logged but never block the operation.
    pub async fn execute_observe(&self, context: &HookContext) {
        let results = self.execute(context).await;
        for result in &results {
            if let Some(ref error) = result.error {
                warn!(error = %error, "Hook execution error");
            }
            if let Some(ref output) = result.output {
                debug!(output = %output, "Hook output");
            }
        }
    }

    /// Execute hooks for an observe-only event and return the injected
    /// `additionalContext` payloads (in execution order). Plain
    /// `execute_observe` discards them — callers that want to surface
    /// hook-provided context to the model use this variant.
    pub async fn execute_observe_collect(&self, context: &HookContext) -> Vec<String> {
        let results = self.execute(context).await;
        results
            .iter()
            .filter_map(|r| r.additional_context.clone())
            .collect()
    }

    /// Execute the Stop hook at the end of an agent turn.
    ///
    /// Stop hooks can deny continuation by returning `Err(reason)` —
    /// the caller injects `reason` as a correction prompt and runs
    /// another loop iteration. `Ok(())` means the turn may end.
    ///
    /// Aggregation: explicit `deny` wins over `blocking_error` (exit code 2),
    /// which wins over plain failures. Plain failures stay fail-open and do
    /// not produce an `Err` — they are logged only.
    pub async fn execute_stop_hooks(&self, context: &HookContext) -> Result<(), String> {
        let results = self.execute(context).await;

        for result in &results {
            if !result.allow {
                return Err(result
                    .deny_reason
                    .clone()
                    .unwrap_or_else(|| "Denied by hook".to_string()));
            }
        }
        for result in &results {
            if let Some(ref reason) = result.blocking_error {
                return Err(format!("Blocking hook error: {reason}"));
            }
        }
        Ok(())
    }

    /// Execute a single hook — dispatching async hooks to the background.
    pub(super) async fn execute_hook(
        &self,
        hook: &HookDefinition,
        context: &HookContext,
    ) -> HookResult {
        // Async hooks only apply to non-blocking events: the loop must never
        // hand a permission gate to a background task. The hook runs in the
        // background; an exit-2 blocking error wakes the loop when the hook
        // opted into `async_rewake`.
        if hook.async_hook && !context.event.is_blocking() {
            let this = self.clone();
            let hook = hook.clone();
            let context = context.clone();
            let sink = self.wake_sink.clone();
            tokio::spawn(async move {
                let result = this.execute_hook_sync(&hook, &context).await;
                if let Some(ref message) = result.blocking_error {
                    // Wake only when the hook opted into `async_rewake` — an
                    // exit-2 error on an async_rewake=false hook must stay
                    // silent (it is a background observation, not a loop
                    // interrupt). Without this gate, every failing async
                    // hook spuriously wakes the agent loop.
                    if hook.async_rewake {
                        if let Some(ref sink) = sink {
                            sink.push_wake(&context.session_id, message.clone());
                            return;
                        }
                        warn!(
                            session_id = %context.session_id,
                            %message,
                            "Async hook exited with code 2 but no wake sink is attached"
                        );
                    } else {
                        warn!(
                            session_id = %context.session_id,
                            %message,
                            "Async hook exited with code 2 (async_rewake disabled)"
                        );
                    }
                }
                if let Some(ref err) = result.error {
                    warn!(
                        session_id = %context.session_id,
                        error = %err,
                        "Async hook failed"
                    );
                }
            });
            return HookResult::allow();
        }
        self.execute_hook_sync(hook, context).await
    }

    /// Execute a single hook to completion (sync path, including the
    /// structured JSON directive application).
    async fn execute_hook_sync(&self, hook: &HookDefinition, context: &HookContext) -> HookResult {
        let timeout = Duration::from_millis(hook.timeout_ms.unwrap_or(30_000));

        let future = async {
            match hook.hook_type {
                HookType::Command => self.execute_command(hook, context).await,
                HookType::Prompt => self.execute_prompt(hook, context).await,
                HookType::Agent => self.execute_agent(hook, context).await,
                HookType::Http => self.execute_http(hook, context).await,
            }
        };

        // Isolate a panicking hook in addition to the timeout: a hook that
        // unwinds (rather than returning HookResult::error) must not take
        // down the whole dispatch loop or starve the hooks behind it.
        let mut result = match AssertUnwindSafe(tokio::time::timeout(timeout, future))
            .catch_unwind()
            .await
        {
            Ok(Ok(r)) => r,
            // Fail-open: a timed-out hook must NOT block the operation.
            // The timeout is logged so the user can see it in debug trace.
            Ok(Err(_)) => {
                warn!(
                    timeout_ms = timeout.as_millis(),
                    "Hook timed out — failing open"
                );
                HookResult::error(format!("Hook timed out after {}ms", timeout.as_millis()))
            }
            Err(_) => {
                warn!(
                    hook = %crate::hooks::trust::fingerprint(hook),
                    "Hook panicked — isolating and failing open"
                );
                HookResult::error("Hook panicked".to_string())
            }
        };

        // Structured JSON output protocol: a hook can rewrite the tool
        // input (PreToolUse), inject context, or override the verdict —
        // regardless of which execution type produced the output.
        if let Some(ref output) = result.output.clone() {
            if let Some(directives) = parse_directives(output, &context.event) {
                apply_directives(&mut result, directives);
            }
        }

        result
    }

    /// Execute a command-type hook. Exit code 0 = allow, non-zero = deny.
    async fn execute_command(&self, hook: &HookDefinition, context: &HookContext) -> HookResult {
        let command = match &hook.command {
            Some(c) => c,
            None => return HookResult::error("Command hook has no command"),
        };

        // Substitute context variables in the command, then expand
        // runtime environment variables ($VAR / ${VAR}).
        let command = crate::hooks::env_expand::expand_env(&self.substitute_vars(command, context));

        // Shell selection: Claude-style `shell: "powershell"` is honored on
        // Windows too (pwsh). cmd /C mangles JSON quotes in arguments, so
        // JSON-emitting hooks need a real shell — plain text hooks keep the
        // lightweight cmd default.
        let (shell, pre_args): (String, Vec<&str>) = if cfg!(target_os = "windows") {
            match hook.shell.as_deref() {
                Some(s)
                    if s.eq_ignore_ascii_case("powershell")
                        || s.eq_ignore_ascii_case("pwsh") =>
                {
                    (
                        win_powershell().to_string(),
                        vec!["-NoProfile", "-NonInteractive", "-Command"],
                    )
                }
                _ => ("cmd".to_string(), vec!["/C"]),
            }
        } else {
            (
                hook.shell.clone().unwrap_or_else(|| "bash".to_string()),
                vec!["-c"],
            )
        };

        let display_command = crate::hooks::env_expand::redact_sensitive(&command);
        info!(shell = %shell, command = %display_command, "Executing command hook");

        let mut cmd = tokio::process::Command::new(&shell);
        crate::core::proc::no_window_tokio(&mut cmd);
        // Kill the child when the timeout drops this future — otherwise a
        // timed-out hook leaves the subprocess running as an orphan.
        cmd.kill_on_drop(true);
        cmd.args(&pre_args).arg(&command);
        let output = cmd.output().await;

        match output {
            Ok(output) => {
                let stdout = crate::core::encoding::decode_native_output(&output.stdout);
                let stderr = crate::core::encoding::decode_native_output(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                match exit_code {
                    // Exit 0: success — allow.
                    0 => HookResult::allow().with_output(stdout),
                    // Exit 2: blocking error — the hook ran but hit an
                    // error that must be surfaced to the agent (Stop hooks
                    // use this to inject a correction and continue the loop).
                    2 => {
                        HookResult::blocking_error(if stderr.is_empty() { stdout } else { stderr })
                    }
                    // Any other non-zero: denial.
                    _ => HookResult::deny(format!(
                        "Command exited with code {}: {}",
                        exit_code,
                        if stderr.is_empty() { stdout } else { stderr }
                    )),
                }
            }
            // Fail-open: if the command binary itself can't be launched
            // (missing cmd/bash, permission denied), don't block the tool.
            Err(e) => {
                warn!(error = %e, "Failed to spawn hook command — failing open");
                HookResult::error(format!("Failed to execute command: {}", e))
            }
        }
    }

    /// Execute a prompt-type hook by sending it to an LLM evaluator.
    ///
    /// If no evaluator is set, the hook fails open (allows the operation)
    /// and logs a warning so the user knows the hook was not evaluated.
    async fn execute_prompt(&self, hook: &HookDefinition, context: &HookContext) -> HookResult {
        let prompt = match &hook.prompt {
            Some(p) => p,
            None => return HookResult::error("Prompt hook has no prompt"),
        };

        let prompt = self.substitute_vars(prompt, context);
        let prompt = crate::hooks::env_expand::expand_env(&prompt);

        match &self.prompt_evaluator {
            Some(evaluator) => evaluator.evaluate(&prompt, context).await,
            None => {
                warn!(prompt = %prompt, "Prompt hook has no evaluator — failing open");
                HookResult::allow()
            }
        }
    }

    /// Execute an agent-type hook by spawning a subagent evaluator.
    ///
    /// If no evaluator is set, the hook fails open.
    async fn execute_agent(&self, hook: &HookDefinition, context: &HookContext) -> HookResult {
        match &self.agent_evaluator {
            Some(evaluator) => evaluator.evaluate(hook, context).await,
            None => {
                warn!("Agent hook has no evaluator — failing open");
                HookResult::allow()
            }
        }
    }

    /// Execute an HTTP-type hook.
    async fn execute_http(&self, hook: &HookDefinition, context: &HookContext) -> HookResult {
        let url = match &hook.url {
            Some(u) => u,
            None => return HookResult::error("HTTP hook has no URL"),
        };

        // SSRF guard: never POST to internal addresses, loopback, or
        // cloud metadata endpoints. Fails closed — an unsafe URL is an
        // error, not a permission grant. Env expansion runs BEFORE the
        // guard so a variable resolving to an internal host is still
        // rejected (fail-closed ordering).
        let url = crate::hooks::env_expand::expand_env(&self.substitute_vars(url, context));
        if let Err(reason) = crate::hooks::ssrf::validate_hook_url(&url) {
            let display_url = crate::hooks::env_expand::redact_sensitive(&url);
            warn!(url = %display_url, reason = %reason, "HTTP hook URL rejected by SSRF guard");
            return HookResult::error(format!("SSRF guard rejected hook URL: {reason}"));
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // Never follow redirects: the SSRF guard validated the ORIGINAL
            // URL only, and a trusted endpoint could 302 to an internal
            // address (loopback, cloud metadata). A redirect surfaces as a
            // non-success status → fail-closed error.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();

        // Cap the payload like `substitute_vars` does for command/prompt
        // hooks: tool args/results can be unbounded (grep over a large tree,
        // a write_file with a big buffer) and must not be serialized to a
        // remote endpoint verbatim — memory/bandwidth spike and full tool
        // output exfiltration. Truncation is marked so the endpoint can tell.
        let capped_args = context.tool_args.as_ref().map(|v| {
            crate::core::str_util::truncate_at_char_boundary(&v.to_string(), MAX_PAYLOAD_SIZE)
                .to_string()
        });
        let capped_result = context.tool_result.as_ref().map(|s| {
            let capped = crate::core::str_util::truncate_at_char_boundary(s, MAX_PAYLOAD_SIZE);
            if capped.len() < s.len() {
                format!("{capped}...(truncated)")
            } else {
                capped.to_string()
            }
        });
        let payload = serde_json::json!({
            "event": context.event.as_str(),
            "session_id": context.session_id,
            "tool_name": context.tool_name,
            "tool_args": capped_args,
            "tool_result": capped_result,
            "data": context.data,
        });

        match client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();

                if status.is_success() {
                    // The unified JSON directive parser (in `execute_hook`)
                    // applies `allow`/`permissionDecision`/`updatedInput`/
                    // `additionalContext` from the body — keeping one
                    // protocol across all hook types.
                    HookResult::allow().with_output(body)
                } else {
                    HookResult::error(format!("HTTP {} : {}", status, body))
                }
            }
            Err(e) => HookResult::error(format!("HTTP request failed: {}", e)),
        }
    }

    /// Evaluate a condition expression against the context.
    ///
    /// Supports multiple pattern styles:
    /// - `tool_name == "bash"` — exact match
    /// - `tool_name != "read_file"` — negated exact match
    /// - `Bash(git *)` — tool name + argument pattern (wildcard)
    /// - `Write(src/**)` — tool name + path pattern
    /// - Empty string — matches all
    fn evaluate_condition(&self, condition: &str, context: &HookContext) -> bool {
        let condition = condition.trim();

        // Empty condition matches everything.
        if condition.is_empty() {
            return true;
        }

        // Simple equality: tool_name == "bash"
        if let Some(rest) = condition.strip_prefix("tool_name == ") {
            let expected = rest.trim_matches('"');
            return context.tool_name.as_deref() == Some(expected);
        }

        // Simple inequality: tool_name != "read_file"
        if let Some(rest) = condition.strip_prefix("tool_name != ") {
            let expected = rest.trim_matches('"');
            return context.tool_name.as_deref() != Some(expected);
        }

        // Pattern matching: ToolName(pattern) or ToolName(*)
        if let Some((tool_pattern, arg_pattern)) = parse_pattern_condition(condition) {
            // Check tool name match.
            if let Some(ref tool_name) = context.tool_name {
                if !wildcard_match(tool_pattern, tool_name) {
                    return false;
                }
            } else {
                return false;
            }

            // Check argument pattern (if any) against the per-tool argument
            // text via the shared tool-pattern DSL.
            if arg_pattern != "*" {
                let args = context.tool_args.clone().unwrap_or(serde_json::Value::Null);
                return crate::core::pattern::glob_match(
                    arg_pattern,
                    &crate::core::pattern::extract_arg_text(tool_pattern, &args),
                );
            }

            return true;
        }

        // Default: always true (unknown condition format).
        true
    }

    /// Substitute context variables in a string.
    ///
    /// Injected payloads (tool args) are capped at [`MAX_PAYLOAD_SIZE`] to
    /// keep hook stdin/URLs bounded — oversized tool outputs must not be
    /// echoed to hook processes verbatim.
    fn substitute_vars(&self, input: &str, context: &HookContext) -> String {
        let mut result = input.to_string();

        result = result.replace("{session_id}", &context.session_id);
        result = result.replace("{event}", context.event.as_str());

        if let Some(ref tool_name) = context.tool_name {
            result = result.replace("{tool_name}", tool_name);
        }

        if let Some(ref tool_args) = context.tool_args {
            let args_str = tool_args.to_string();
            let capped =
                crate::core::str_util::truncate_at_char_boundary(&args_str, MAX_PAYLOAD_SIZE);
            result = result.replace("{tool_args}", capped);
        }

        result
    }
}

/// Maximum serialized size for hook input payloads in bytes (128 KB).
const MAX_PAYLOAD_SIZE: usize = 128 * 1024;

/// Parse a pattern condition like `Bash(git *)` or `Write(src/**)`.
///
/// Returns `Some((tool_pattern, arg_pattern))` if the condition is a valid pattern,
/// or `None` if it's not a pattern condition.
fn parse_pattern_condition(condition: &str) -> Option<(&str, &str)> {
    let condition = condition.trim();

    // Find the opening parenthesis.
    let start = condition.find('(')?;
    let end = condition.rfind(')')?;

    // Must end with ')'.
    if end != condition.len() - 1 {
        return None;
    }

    let tool_pattern = condition[..start].trim();
    let arg_pattern = condition[start + 1..end].trim();

    if tool_pattern.is_empty() {
        return None;
    }

    Some((tool_pattern, arg_pattern))
}

/// Match a string against a wildcard pattern.
///
/// `*` matches any sequence of characters.
/// `?` matches a single character.
/// Tool names are matched case-insensitively (`Bash` ≡ `bash`), consistent
/// with the permission rule layer — hook conditions like `Bash(rm *)` must
/// behave exactly like permission rules or gates silently no-op.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    // Convert wildcard pattern to regex.
    let regex_pattern = pattern
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");

    let regex_pattern = format!(
        "(?i)^{}$",
        regex_pattern.replace('?', ".") // Handle ? wildcard
    );

    regex::Regex::new(&regex_pattern)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::types::{HookDefinition, HookEvent, HookType};

    fn make_command_hook(cmd: &str, timeout_ms: Option<u64>) -> HookDefinition {
        HookDefinition {
            event: HookEvent::PreToolUse,
            hook_type: HookType::Command,
            command: Some(cmd.to_string()),
            prompt: None,
            url: None,
            condition: None,
            timeout_ms,
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        }
    }

    fn make_shell_command_hook(
        cmd: &str,
        shell: Option<&str>,
        timeout_ms: Option<u64>,
    ) -> HookDefinition {
        HookDefinition {
            shell: shell.map(str::to_string),
            ..make_command_hook(cmd, timeout_ms)
        }
    }

    fn executor_with(hooks: Vec<HookDefinition>) -> HookExecutor {
        let mut registry = HookRegistry::new();
        for def in hooks {
            registry.register(def);
        }
        HookExecutor::new(Arc::new(RwLock::new(registry)))
    }

    #[tokio::test]
    async fn no_hooks_allows_gate() {
        let executor = HookExecutor::new(Arc::new(RwLock::new(HookRegistry::new())));
        let ctx = HookContext::new(HookEvent::PreToolUse, "test-session");
        assert!(executor.execute_gate(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn exit_zero_allows() {
        let executor = executor_with(vec![make_command_hook(
            if cfg!(windows) { "echo ok" } else { "true" },
            None,
        )]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("read_file", serde_json::json!({"path": "test.txt"}));
        assert!(executor.execute_gate(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn non_zero_exit_denies() {
        let executor = executor_with(vec![make_command_hook(
            if cfg!(windows) { "exit /b 1" } else { "exit 1" },
            None,
        )]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("bash", serde_json::json!({"command": "rm -rf /"}));
        let result = executor.execute_gate(&ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exit"));
    }

    #[tokio::test]
    async fn timeout_fails_open() {
        // A command that sleeps longer than the timeout.
        // Under fail-open the gate must stay open (Ok).
        let executor = executor_with(vec![make_command_hook(
            if cfg!(windows) {
                "ping -n 10 127.0.0.1"
            } else {
                "sleep 10"
            },
            Some(100), // 100ms timeout
        )]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("slow_tool", serde_json::json!({}));
        let result = executor.execute_gate(&ctx).await;
        assert!(result.is_ok(), "timeout must fail open, got: {:?}", result);
    }

    #[tokio::test]
    async fn condition_filters_hook() {
        let mut registry = HookRegistry::new();
        registry.register(HookDefinition {
            event: HookEvent::PreToolUse,
            hook_type: HookType::Command,
            command: Some(if cfg!(windows) { "exit /b 1" } else { "exit 1" }.to_string()),
            condition: Some(r#"tool_name == "bash""#.to_string()),
            ..make_command_hook("", None)
        });
        let executor = HookExecutor::new(Arc::new(RwLock::new(registry)));

        // Tool name "read_file" does NOT match condition "bash" — hook skipped.
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("read_file", serde_json::json!({}));
        assert!(executor.execute_gate(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn exit_two_produces_blocking_error() {
        let executor = executor_with(vec![make_command_hook(
            if cfg!(windows) { "exit /b 2" } else { "exit 2" },
            None,
        )]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("bash", serde_json::json!({"command": "deploy"}));
        let result = executor.execute_gate(&ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Blocking hook error"), "got: {err}");
    }

    #[tokio::test]
    async fn json_directives_rewrite_input() {
        let payload = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"rm --dry-run *"}}}"#;
        let (cmd, shell) = if cfg!(windows) {
            (
                format!("Write-Output '{payload}'"),
                Some("powershell"),
            )
        } else {
            (format!("echo '{payload}'"), None)
        };
        let executor = executor_with(vec![make_shell_command_hook(&cmd, shell, None)]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("bash", serde_json::json!({"command": "rm *"}));
        let outcome = executor.execute_gate(&ctx).await.expect("gate must allow");
        assert_eq!(
            outcome.updated_input,
            Some(serde_json::json!({"command": "rm --dry-run *"}))
        );
    }

    #[tokio::test]
    async fn json_directives_deny() {
        let payload = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"policy"}}"#;
        let (cmd, shell) = if cfg!(windows) {
            (
                format!("Write-Output '{payload}'"),
                Some("powershell"),
            )
        } else {
            (format!("echo '{payload}'"), None)
        };
        let executor = executor_with(vec![make_shell_command_hook(&cmd, shell, None)]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("bash", serde_json::json!({"command": "rm *"}));
        let result = executor.execute_gate(&ctx).await;
        assert_eq!(result.unwrap_err(), "policy");
    }

    #[tokio::test]
    async fn json_additional_context_collected() {
        let payload = r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"linter: 3 warnings"}}"#;
        let (cmd, shell) = if cfg!(windows) {
            (
                format!("Write-Output '{payload}'"),
                Some("powershell"),
            )
        } else {
            (format!("echo '{payload}'"), None)
        };
        let mut registry = HookRegistry::new();
        registry.register(HookDefinition {
            event: HookEvent::PostToolUse,
            hook_type: HookType::Command,
            command: Some(cmd),
            shell: shell.map(str::to_string),
            ..make_command_hook("", None)
        });
        let executor = HookExecutor::new(Arc::new(RwLock::new(registry)));
        let ctx = HookContext::new(HookEvent::PostToolUse, "s1")
            .with_tool("edit_file", serde_json::json!({"path": "a.rs"}))
            .with_result("ok");
        let contexts = executor.execute_observe_collect(&ctx).await;
        assert_eq!(contexts, vec!["linter: 3 warnings".to_string()]);
    }

    #[tokio::test]
    async fn async_hook_rewake_wakes_sink() {
        let (cmd, shell) = if cfg!(windows) {
            (
                "Start-Sleep -Milliseconds 200; Write-Error 'tests failed'; exit 2".to_string(),
                Some("powershell"),
            )
        } else {
            ("sleep 0.2; echo 'tests failed' >&2; exit 2".to_string(), None)
        };
        let mut registry = HookRegistry::new();
        registry.register(HookDefinition {
            event: HookEvent::PostToolUse,
            hook_type: HookType::Command,
            command: Some(cmd),
            shell: shell.map(str::to_string),
            async_hook: true,
            async_rewake: true,
            ..make_command_hook("", None)
        });
        let buffer: std::sync::Arc<
            tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
        > = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let executor = HookExecutor::new(Arc::new(RwLock::new(registry)))
            .with_wake_sink(buffer.clone());
        let ctx = HookContext::new(HookEvent::PostToolUse, "s1")
            .with_tool("edit_file", serde_json::json!({"path": "a.rs"}))
            .with_result("ok");
        // Non-blocking: observe returns immediately, the hook runs in bg.
        executor.execute_observe(&ctx).await;
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        let wakes = buffer.lock().await;
        let msgs = wakes.get("s1").expect("wake must be queued");
        assert!(
            msgs.iter().any(|m| m.contains("tests failed")),
            "wake must carry the hook message: {msgs:?}"
        );
    }

    #[tokio::test]
    async fn async_hook_without_rewake_stays_silent() {
        let (cmd, shell) = if cfg!(windows) {
            ("exit 2".to_string(), Some("powershell"))
        } else {
            ("exit 2".to_string(), None)
        };
        let mut registry = HookRegistry::new();
        registry.register(HookDefinition {
            event: HookEvent::PostToolUse,
            hook_type: HookType::Command,
            command: Some(cmd),
            shell: shell.map(str::to_string),
            async_hook: true,
            async_rewake: false,
            ..make_command_hook("", None)
        });
        let buffer: std::sync::Arc<
            tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
        > = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let executor = HookExecutor::new(Arc::new(RwLock::new(registry)))
            .with_wake_sink(buffer.clone());
        let ctx = HookContext::new(HookEvent::PostToolUse, "s1")
            .with_tool("edit_file", serde_json::json!({}))
            .with_result("ok");
        executor.execute_observe(&ctx).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(buffer.lock().await.is_empty(), "no wake expected");
    }

    #[tokio::test]
    async fn async_flag_is_ignored_for_blocking_events() {
        let executor = executor_with(vec![HookDefinition {
            async_hook: true,
            async_rewake: false,
            ..make_command_hook(if cfg!(windows) { "exit /b 1" } else { "exit 1" }, None)
        }]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("bash", serde_json::json!({"command": "rm *"}));
        // Blocking events always wait: the async flag must not bypass the gate.
        assert!(executor.execute_gate(&ctx).await.is_err());
    }

    #[tokio::test]
    async fn once_hook_removes_itself_after_execution() {
        let executor = executor_with(vec![HookDefinition {
            once: true,
            ..make_command_hook(if cfg!(windows) { "exit /b 1" } else { "exit 1" }, None)
        }]);
        let ctx = HookContext::new(HookEvent::PreToolUse, "s1")
            .with_tool("bash", serde_json::json!({"command": "echo hi"}));
        // First execution: the once-hook runs and denies.
        assert!(executor.execute_gate(&ctx).await.is_err());
        // Second execution: the hook is gone — the gate allows.
        assert!(executor.execute_gate(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn deny_wins_over_blocking_error() {
        let mut registry = HookRegistry::new();
        registry.register(HookDefinition {
            event: HookEvent::Stop,
            hook_type: HookType::Command,
            command: Some(if cfg!(windows) { "exit /b 2" } else { "exit 2" }.to_string()),
            ..make_command_hook("", None)
        });
        registry.register(HookDefinition {
            event: HookEvent::Stop,
            hook_type: HookType::Command,
            command: Some(if cfg!(windows) { "exit /b 1" } else { "exit 1" }.to_string()),
            ..make_command_hook("", None)
        });
        let executor = HookExecutor::new(Arc::new(RwLock::new(registry)));
        let ctx = HookContext::new(HookEvent::Stop, "s1");
        let result = executor.execute_stop_hooks(&ctx).await;
        let err = result.unwrap_err();
        assert!(
            !err.contains("Blocking hook error"),
            "deny must win over blocking error, got: {err}"
        );
    }

    #[tokio::test]
    async fn stop_hook_blocking_error_continues_loop() {
        let executor = executor_with(vec![HookDefinition {
            event: HookEvent::Stop,
            hook_type: HookType::Command,
            command: Some(if cfg!(windows) { "exit /b 2" } else { "exit 2" }.to_string()),
            ..make_command_hook("", None)
        }]);
        let ctx = HookContext::new(HookEvent::Stop, "s1");
        let result = executor.execute_stop_hooks(&ctx).await;
        assert!(result.is_err(), "blocking error must surface as Err");
        assert!(result.unwrap_err().contains("Blocking hook error"));
    }

    #[test]
    fn wildcard_match_is_case_insensitive_for_tool_names() {
        // `Bash(...)` in a hook condition must match the lowercase tool
        // name the dispatcher reports, exactly like permission rules.
        assert!(wildcard_match("Bash", "bash"));
        assert!(wildcard_match("bash", "Bash"));
        assert!(wildcard_match("Bash(rm *)", "bash(rm *)"));
        assert!(!wildcard_match("Edit", "read_file"));
    }
}
