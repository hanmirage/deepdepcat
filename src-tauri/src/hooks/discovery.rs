//! Hook discovery — auto-discovers hooks from TOML configuration files.
//!
//! Scans two locations for hook definitions:
//! 1. **User-level**: `~/.deepdepcat/hooks.toml` (or platform equivalent)
//! 2. **Project-level**: `<workspace>/.deepdepcat/hooks.toml`
//!
//! Project hooks are additive to user hooks — they don't replace them.
//!
//! ## Config Format
//!
//! ```toml
//! [[hooks]]
//! event = "PreToolUse"
//! type = "command"
//! command = "echo 'pre-tool'"
//! timeout_ms = 5000
//!
//! [[hooks]]
//! event = "PostToolUse"
//! type = "http"
//! url = "https://example.com/webhook"
//!
//! [[hooks]]
//! event = "PostToolUse"
//! type = "command"
//! command = "npm test 2>&1; if ($LASTEXITCODE -ne 0) { exit 2 }"
//! shell = "powershell"
//! async = true          # run in the background, never block the loop
//! async_rewake = true   # exit code 2 wakes the agent mid-turn to fix it
//! once = true           # auto-remove after one execution
//! ```
//!
//! ## Structured JSON output protocol
//!
//! A hook (any type) may return JSON on stdout / HTTP body to express more
//! than plain allow/deny:
//!
//! ```json
//! {
//!   "hookSpecificOutput": {
//!     "hookEventName": "PreToolUse",
//!     "permissionDecision": "allow",
//!     "updatedInput": { "command": "rm --dry-run *" },
//!     "additionalContext": "linter: 3 warnings"
//!   }
//! }
//! ```
//!
//! - `permissionDecision` + `updatedInput` apply to PreToolUse: the hook
//!   can rewrite the tool input before the permission system and tool see
//!   it (the model's original args stay for audit/failure-guard purposes).
//! - `additionalContext` applies to UserMessage / PostToolUse /
//!   PostToolUseFailure: the text is injected as a transient system
//!   reminder, visible to the model on the next request, never persisted.
//! - The flat form `{"allow": false, "reason": "...", "updatedInput": ...,
//!   "additionalContext": "..."}` is also accepted (HTTP-style hooks).
//! - Exit codes keep their meaning: 0 = success, 2 = blocking error
//!   (surfaced to the agent), anything else = denial.

use crate::core::error::{AppError, AppResult};
use crate::hooks::registry::HookRegistry;
use crate::hooks::types::{HookDefinition, HookEvent, HookType};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{info, warn};

/// Raw hook definition as it appears in TOML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawHookDefinition {
    pub event: String,
    #[serde(rename = "type")]
    pub hook_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
    #[serde(rename = "if", skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shell: Option<String>,
    #[serde(rename = "async", default = "default_false")]
    pub async_hook: bool,
    #[serde(default = "default_false")]
    pub async_rewake: bool,
    #[serde(default = "default_false")]
    pub once: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

/// Wrapper for the TOML config file format.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HooksConfig {
    #[serde(default)]
    pub hooks: Vec<RawHookDefinition>,
}

/// Discover and load hooks from config files.
///
/// Scans user-level and project-level `hooks.toml` files, converts them to
/// runtime [`HookDefinition`] values, and registers them in the registry.
///
/// Returns the number of hooks registered.
pub fn discover_and_register(
    registry: &mut HookRegistry,
    app_data_dir: &Path,
    workspace: Option<&Path>,
    enable_project_hooks: bool,
) -> AppResult<usize> {
    let mut count = 0;

    // User-level hooks
    let user_path = app_data_dir.join("hooks.toml");
    if user_path.exists() {
        match load_hooks_file(&user_path) {
            Ok(hooks) => {
                info!(path = %user_path.display(), count = hooks.len(), "Loaded user hooks");
                for raw in hooks {
                    match convert_hook(raw) {
                        Ok(def) => {
                            registry.register(def);
                            count += 1;
                        }
                        Err(e) => {
                            warn!(error = %e, "Skipping invalid hook definition");
                        }
                    }
                }
            }
            Err(e) => {
                warn!(path = %user_path.display(), error = %e, "Failed to load hooks file");
            }
        }
    }

    // Project-level hooks — gated by config. A project's hooks execute
    // arbitrary commands with full tool context, so they are OPT-IN:
    // disabled by default and only loaded after the user explicitly
    // enables them in Settings → Hooks.
    if enable_project_hooks {
        if let Some(ws) = workspace {
            let project_path = ws.join(".deepdepcat").join("hooks.toml");
            if project_path.exists() {
                match load_hooks_file(&project_path) {
                    Ok(hooks) => {
                        info!(path = %project_path.display(), count = hooks.len(), "Loaded project hooks");
                        for raw in hooks {
                            match convert_hook(raw) {
                                Ok(def) => {
                                    registry.register(def);
                                    count += 1;
                                }
                                Err(e) => {
                                    warn!(error = %e, "Skipping invalid project hook definition");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(path = %project_path.display(), error = %e, "Failed to load project hooks");
                    }
                }
            }
        }
    }

    Ok(count)
}

/// List raw hook definitions from an arbitrary TOML file — used by the
/// settings page to AUDIT project-level hooks (read-only; project hooks
/// are never editable from the UI, only disable-able via the master
/// switch).
pub fn list_hooks_file(path: &Path) -> AppResult<Vec<HookDefinition>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("Failed to read {}: {e}", path.display())))?;
    let config: HooksConfig = toml::from_str(&content)
        .map_err(|e| AppError::Config(format!("Failed to parse {}: {e}", path.display())))?;
    config
        .hooks
        .into_iter()
        .map(convert_hook)
        .collect::<AppResult<Vec<_>>>()
}

/// Load raw hook definitions from a TOML file.
fn load_hooks_file(path: &Path) -> AppResult<Vec<RawHookDefinition>> {
    let content = std::fs::read_to_string(path)?;
    let config: HooksConfig = toml::from_str(&content)?;
    Ok(config.hooks)
}

/// Convert a raw config hook definition to a runtime hook definition.
///
/// Parses the event name string into a [`HookEvent`] and the type string
/// into a [`HookType`]. Returns an error if either is unrecognized.
fn convert_hook(raw: RawHookDefinition) -> AppResult<HookDefinition> {
    let event = parse_hook_event(&raw.event)?;
    let hook_type = parse_hook_type(&raw.hook_type)?;

    // Validate that the required field for the hook type is present.
    match hook_type {
        HookType::Command => {
            if raw.command.is_none() {
                return Err(AppError::Config(format!(
                    "Command hook for event '{}' has no command field",
                    raw.event
                )));
            }
        }
        HookType::Prompt => {
            if raw.prompt.is_none() {
                return Err(AppError::Config(format!(
                    "Prompt hook for event '{}' has no prompt field",
                    raw.event
                )));
            }
        }
        HookType::Http => {
            if raw.url.is_none() {
                return Err(AppError::Config(format!(
                    "HTTP hook for event '{}' has no url field",
                    raw.event
                )));
            }
        }
        HookType::Agent => {} // Agent hooks use context, no required field
    }

    Ok(HookDefinition {
        event,
        hook_type,
        command: raw.command,
        prompt: raw.prompt,
        url: raw.url,
        condition: raw.condition,
        timeout_ms: raw.timeout_ms,
        shell: raw.shell,
        async_hook: raw.async_hook,
        async_rewake: raw.async_rewake,
        once: raw.once,
        enabled: raw.enabled,
    })
}

/// Parse a hook event name string into a [`HookEvent`].
fn parse_hook_event(s: &str) -> AppResult<HookEvent> {
    // Case-insensitive match with alias convergence: third-party configs use
    // snake_case/camelCase or per-operation names (beforeShellExecution,
    // afterFileEdit, ...) — all collapse onto our canonical events.
    let lower = s.to_lowercase();
    let lower = lower.replace(['_', '-'], "");
    match lower.as_str() {
        "sessionstart" => Ok(HookEvent::SessionStart),
        "sessionend" => Ok(HookEvent::SessionEnd),
        "sessionpause" => Ok(HookEvent::SessionPause),
        "sessionresume" => Ok(HookEvent::SessionResume),
        "agentloopstart" => Ok(HookEvent::AgentLoopStart),
        "agentloopend" => Ok(HookEvent::AgentLoopEnd),
        "agentloopturn" => Ok(HookEvent::AgentLoopTurn),
        "agentloopturnend" => Ok(HookEvent::AgentLoopTurnEnd),
        "stop" => Ok(HookEvent::Stop),
        "stopfailure" => Ok(HookEvent::StopFailure),
        "subagentstart" => Ok(HookEvent::SubagentStart),
        "subagentstop" | "subagentend" => Ok(HookEvent::SubagentStop),
        "taskupdated" | "backgroundtaskupdated" => Ok(HookEvent::TaskUpdated),
        "taskcompleted" | "backgroundtaskcompleted" | "taskfinished" => {
            Ok(HookEvent::TaskCompleted)
        }
        // Third-party per-operation aliases → PreToolUse.
        "pretooluse" | "preshellexecution" | "premcpexecution" | "beforereadfile" => {
            Ok(HookEvent::PreToolUse)
        }
        // Third-party per-operation aliases → PostToolUse.
        "posttooluse"
        | "aftershellexecution"
        | "aftermcpexecution"
        | "afterfileedit"
        | "afteragentresponse"
        | "afteragentthought" => Ok(HookEvent::PostToolUse),
        "posttoolusefailure" | "toolfailure" => Ok(HookEvent::PostToolUseFailure),
        "posttoolbatch" | "toolbatch" => Ok(HookEvent::PostToolBatch),
        "toolerror" => Ok(HookEvent::ToolError),
        "prellmcall" => Ok(HookEvent::PreLLMCall),
        "postllmcall" => Ok(HookEvent::PostLLMCall),
        "llmstreamstart" => Ok(HookEvent::LLMStreamStart),
        "llmstreamend" => Ok(HookEvent::LLMStreamEnd),
        "usermessage" | "userpromptsubmit" | "beforesubmitprompt" => Ok(HookEvent::UserMessage),
        "assistantmessage" => Ok(HookEvent::AssistantMessage),
        "userinputrequested" => Ok(HookEvent::UserInputRequested),
        "precompaction" | "precompact" => Ok(HookEvent::PreCompaction),
        "postcompaction" | "postcompact" => Ok(HookEvent::PostCompaction),
        "error" => Ok(HookEvent::Error),
        "fatalerror" => Ok(HookEvent::FatalError),
        "filechanged" => Ok(HookEvent::FileChanged),
        "filecreated" => Ok(HookEvent::FileCreated),
        "filedeleted" => Ok(HookEvent::FileDeleted),
        "permissiondenied" => Ok(HookEvent::PermissionDenied),
        "permissionasked" | "permissionrequest" => Ok(HookEvent::PermissionAsked),
        "notification" | "notify" => Ok(HookEvent::Notification),
        "memorystored" => Ok(HookEvent::MemoryStored),
        "memorysearched" => Ok(HookEvent::MemorySearched),
        "mcpserverconnected" => Ok(HookEvent::McpServerConnected),
        "mcpserverdisconnected" => Ok(HookEvent::McpServerDisconnected),
        _ => Err(AppError::Config(format!("Unknown hook event: '{s}'"))),
    }
}

/// Parse a hook type string into a [`HookType`].
fn parse_hook_type(s: &str) -> AppResult<HookType> {
    match s.to_lowercase().as_str() {
        "command" | "cmd" => Ok(HookType::Command),
        "prompt" => Ok(HookType::Prompt),
        "agent" => Ok(HookType::Agent),
        "http" | "webhook" => Ok(HookType::Http),
        _ => Err(AppError::Config(format!("Unknown hook type: '{s}'"))),
    }
}

/// Serialize a runtime hook definition back to its raw TOML form.
fn to_raw_hook(def: &HookDefinition) -> RawHookDefinition {
    RawHookDefinition {
        event: def.event.as_str().to_string(),
        hook_type: match def.hook_type {
            HookType::Command => "command",
            HookType::Prompt => "prompt",
            HookType::Agent => "agent",
            HookType::Http => "http",
        }
        .to_string(),
        command: def.command.clone(),
        prompt: def.prompt.clone(),
        url: def.url.clone(),
        condition: def.condition.clone(),
        timeout_ms: def.timeout_ms,
        shell: def.shell.clone(),
        async_hook: def.async_hook,
        async_rewake: def.async_rewake,
        once: def.once,
        enabled: def.enabled,
    }
}

/// Load the user-level hooks.toml file (creating an empty config if missing).
fn load_user_hooks_file(app_data_dir: &Path) -> AppResult<HooksConfig> {
    let path = app_data_dir.join("hooks.toml");
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        return toml::from_str(&content)
            .map_err(|e| AppError::Config(format!("Failed to parse hooks.toml: {e}")));
    }
    Ok(HooksConfig::default())
}

/// Save a hook definition to the user-level hooks.toml.
///
/// If a hook with the same (event, type, content) already exists, it is
/// replaced. Returns the total hook count after saving.
pub fn save_hook(app_data_dir: &Path, definition: &HookDefinition) -> AppResult<usize> {
    let path = app_data_dir.join("hooks.toml");
    let mut config = load_user_hooks_file(app_data_dir)?;

    let raw = to_raw_hook(definition);
    let content_key = match definition.hook_type {
        HookType::Command => raw.command.clone(),
        HookType::Prompt => raw.prompt.clone(),
        HookType::Agent => raw.prompt.clone(),
        HookType::Http => raw.url.clone(),
    };

    // Replace existing hook with the same (event, type, content).
    let mut replaced = false;
    for hook in config.hooks.iter_mut() {
        let same_key = match hook.hook_type.as_str() {
            "command" => hook.command == raw.command,
            "prompt" | "agent" => hook.prompt == raw.prompt,
            "http" => hook.url == raw.url,
            _ => false,
        };
        if hook.event == raw.event && hook.hook_type == raw.hook_type && same_key {
            *hook = raw.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        config.hooks.push(raw);
    }

    let content = toml::to_string(&config)
        .map_err(|e| AppError::Internal(format!("Failed to serialize hooks: {e}")))?;
    std::fs::write(&path, content)?;

    let _ = content_key;
    Ok(config.hooks.len())
}

/// Delete a hook from the user-level hooks.toml by (event, type, content).
pub fn delete_hook(
    app_data_dir: &Path,
    event: &str,
    hook_type: &str,
    content: &str,
) -> AppResult<usize> {
    let path = app_data_dir.join("hooks.toml");
    let mut config = load_user_hooks_file(app_data_dir)?;

    config.hooks.retain(|hook| {
        !(hook.event.eq_ignore_ascii_case(event)
            && hook.hook_type.eq_ignore_ascii_case(hook_type)
            && match hook.hook_type.as_str() {
                "command" => hook.command.as_deref() == Some(content),
                "prompt" | "agent" => hook.prompt.as_deref() == Some(content),
                "http" => hook.url.as_deref() == Some(content),
                _ => false,
            })
    });

    let serialized = toml::to_string(&config)
        .map_err(|e| AppError::Internal(format!("Failed to serialize hooks: {e}")))?;
    std::fs::write(&path, serialized)?;

    Ok(config.hooks.len())
}

/// List all hooks from the user-level hooks.toml as runtime definitions.
pub fn list_hooks(app_data_dir: &Path) -> AppResult<Vec<HookDefinition>> {
    let config = load_user_hooks_file(app_data_dir)?;
    config
        .hooks
        .into_iter()
        .map(convert_hook)
        .collect::<AppResult<Vec<_>>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_toml_hooks() {
        let toml_str = r#"
[[hooks]]
event = "PreToolUse"
type = "command"
command = "echo pre"
timeout_ms = 5000

[[hooks]]
event = "PostToolUse"
type = "http"
url = "https://example.com/hook"
"#;
        let config: HooksConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.len(), 2);
        assert_eq!(config.hooks[0].event, "PreToolUse");
        assert_eq!(config.hooks[0].command, Some("echo pre".to_string()));
        assert_eq!(
            config.hooks[1].url,
            Some("https://example.com/hook".to_string())
        );
    }

    #[test]
    fn convert_command_hook() {
        let raw = RawHookDefinition {
            event: "PreToolUse".to_string(),
            hook_type: "command".to_string(),
            command: Some("echo hello".to_string()),
            prompt: None,
            url: None,
            condition: None,
            timeout_ms: Some(10_000),
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        };
        let def = convert_hook(raw).unwrap();
        assert_eq!(def.event, HookEvent::PreToolUse);
        assert_eq!(def.hook_type, HookType::Command);
        assert_eq!(def.command, Some("echo hello".to_string()));
    }

    #[test]
    fn convert_http_hook() {
        let raw = RawHookDefinition {
            event: "post_tool_use".to_string(),
            hook_type: "http".to_string(),
            command: None,
            prompt: None,
            url: Some("https://example.com".to_string()),
            condition: None,
            timeout_ms: None,
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        };
        let def = convert_hook(raw).unwrap();
        assert_eq!(def.event, HookEvent::PostToolUse);
        assert_eq!(def.hook_type, HookType::Http);
    }

    #[test]
    fn convert_missing_command_fails() {
        let raw = RawHookDefinition {
            event: "PreToolUse".to_string(),
            hook_type: "command".to_string(),
            command: None,
            prompt: None,
            url: None,
            condition: None,
            timeout_ms: None,
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        };
        assert!(convert_hook(raw).is_err());
    }

    #[test]
    fn convert_unknown_event_fails() {
        let raw = RawHookDefinition {
            event: "NonExistentEvent".to_string(),
            hook_type: "command".to_string(),
            command: Some("echo".to_string()),
            prompt: None,
            url: None,
            condition: None,
            timeout_ms: None,
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        };
        assert!(convert_hook(raw).is_err());
    }

    #[test]
    fn parses_async_hook_flags() {
        let toml_str = r#"
[[hooks]]
event = "PostToolUse"
type = "command"
command = "npm test"
async = true
async_rewake = true
"#;
        let config: HooksConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.len(), 1);
        assert!(config.hooks[0].async_hook);
        assert!(config.hooks[0].async_rewake);
        let def = convert_hook(config.hooks.into_iter().next().unwrap()).unwrap();
        assert!(def.async_hook);
        assert!(def.async_rewake);
    }

    #[test]
    fn parses_once_flag() {
        let toml_str = r#"
[[hooks]]
event = "PreToolUse"
type = "command"
command = "echo once"
once = true
"#;
        let config: HooksConfig = toml::from_str(toml_str).unwrap();
        assert!(config.hooks[0].once);
        let def = convert_hook(config.hooks.into_iter().next().unwrap()).unwrap();
        assert!(def.once);
    }

    #[test]
    fn discover_from_file() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();

        let hooks_toml = r#"
[[hooks]]
event = "PreToolUse"
type = "command"
command = "echo pre"

[[hooks]]
event = "PostToolUse"
type = "http"
url = "https://example.com"
"#;
        std::fs::write(app_data.join("hooks.toml"), hooks_toml).unwrap();

        let mut registry = HookRegistry::new();
        let count = discover_and_register(&mut registry, app_data, None, false).unwrap();
        assert_eq!(count, 2);
        assert_eq!(registry.get_hooks(&HookEvent::PreToolUse).len(), 1);
        assert_eq!(registry.get_hooks(&HookEvent::PostToolUse).len(), 1);
    }

    #[test]
    fn discover_project_hooks_additive() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();

        // User-level hook
        let user_toml = r#"
[[hooks]]
event = "PreToolUse"
type = "command"
command = "echo user"
"#;
        std::fs::write(app_data.join("hooks.toml"), user_toml).unwrap();

        // Project-level hook
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".deepdepcat")).unwrap();
        let project_toml = r#"
[[hooks]]
event = "PostToolUse"
type = "command"
command = "echo project"
"#;
        std::fs::write(
            workspace.path().join(".deepdepcat").join("hooks.toml"),
            project_toml,
        )
        .unwrap();

        let mut registry = HookRegistry::new();
        let count =
            discover_and_register(&mut registry, app_data, Some(workspace.path()), true).unwrap();
        assert_eq!(count, 2);
        assert_eq!(registry.get_hooks(&HookEvent::PreToolUse).len(), 1);
        assert_eq!(registry.get_hooks(&HookEvent::PostToolUse).len(), 1);
    }

    #[test]
    fn project_hooks_are_skipped_when_disabled() {
        // Security gate: project hooks execute arbitrary commands, so the
        // default (opt-in) posture must skip them entirely.
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();

        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".deepdepcat")).unwrap();
        std::fs::write(
            workspace.path().join(".deepdepcat").join("hooks.toml"),
            r#"
[[hooks]]
event = "PreToolUse"
type = "command"
command = "echo project"
"#,
        )
        .unwrap();

        let mut registry = HookRegistry::new();
        let count =
            discover_and_register(&mut registry, app_data, Some(workspace.path()), false).unwrap();
        assert_eq!(count, 0);
        assert!(registry.get_hooks(&HookEvent::PreToolUse).is_empty());
    }

    #[test]
    fn discover_skips_invalid_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();

        let hooks_toml = r#"
[[hooks]]
event = "PreToolUse"
type = "command"
command = "echo ok"

[[hooks]]
event = "UnknownEvent"
type = "command"
command = "echo bad"
"#;
        std::fs::write(app_data.join("hooks.toml"), hooks_toml).unwrap();

        let mut registry = HookRegistry::new();
        let count = discover_and_register(&mut registry, app_data, None, false).unwrap();
        assert_eq!(count, 1); // Only the valid hook is registered
    }

    #[test]
    fn parse_event_case_insensitive() {
        assert!(parse_hook_event("pretooluse").is_ok());
        assert!(parse_hook_event("PreToolUse").is_ok());
        assert!(parse_hook_event("PRE_TOOL_USE").is_ok());
        assert!(parse_hook_event("post_tool_use").is_ok());
    }
}
