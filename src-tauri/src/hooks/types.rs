//! Hook types — events, definitions, and contexts.
//!
//! 20+ hook events covering:
//! - Session lifecycle (SessionStart, SessionEnd)
//! - Agent loop (AgentLoopStart, AgentLoopEnd, AgentLoopTurn)
//! - Tool execution (PreToolUse, PostToolUse, ToolError)
//! - LLM calls (PreLLMCall, PostLLMCall)
//! - User interaction (UserMessage, AssistantMessage)
//! - Compaction (PreCompaction, PostCompaction)
//! - Errors (Error, FatalError)
//! - File changes (FileChanged, FileCreated, FileDeleted)
//! - Permission (PermissionDenied, PermissionAsked)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// All possible hook events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    // Session lifecycle
    SessionStart,
    SessionEnd,
    SessionPause,
    SessionResume,

    // Agent loop
    AgentLoopStart,
    AgentLoopEnd,
    AgentLoopTurn,
    AgentLoopTurnEnd,
    /// Fired after each agent turn completes — blocking hook that can deny
    /// continuation (inject correction prompt and continue the loop).
    Stop,
    /// Fired when an agent turn ends due to an API error.
    StopFailure,

    // Subagent lifecycle
    /// Fired when a subagent is spawned.
    SubagentStart,
    /// Fired when a subagent finishes (success or failure).
    SubagentStop,

    // Background tasks
    /// Fired when a background task changes state (killed, exited).
    TaskUpdated,
    /// Fired when a background task completes naturally.
    TaskCompleted,

    // Tool execution
    PreToolUse,
    PostToolUse,
    /// Fired after a tool call FAILS (is_error = true) — distinct from
    /// PostToolUse so failure-only pipelines don't parse success payloads.
    PostToolUseFailure,
    /// Fired after a full batch of parallel/serial tool calls resolves,
    /// before the next model call.
    PostToolBatch,
    ToolError,

    // LLM calls
    PreLLMCall,
    PostLLMCall,
    LLMStreamStart,
    LLMStreamEnd,

    // User interaction
    UserMessage,
    AssistantMessage,
    UserInputRequested,

    // Compaction
    PreCompaction,
    PostCompaction,

    // Errors
    Error,
    FatalError,

    // File changes
    FileChanged,
    FileCreated,
    FileDeleted,

    // Permission
    PermissionDenied,
    PermissionAsked,
    /// Product-level notification (permission waits, scheduled-run
    /// completions, breaker trips) — observe-only.
    Notification,

    // Memory
    MemoryStored,
    MemorySearched,

    // MCP
    McpServerConnected,
    McpServerDisconnected,
}

impl HookEvent {
    /// Whether this event is a pre-event (can block the operation).
    ///
    /// `PreCompaction` is deliberately NOT blocking: every caller (manual
    /// compaction, the loop's context phase, and the emergency path) invokes
    /// it through `execute_observe` — it is a lifecycle NOTIFICATION, not a
    /// gate. Marking it blocking forced `async` PreCompaction hooks onto the
    /// synchronous path, stalling compaction for up to the 30s timeout.
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::PreToolUse | Self::PreLLMCall | Self::Stop)
    }

    /// Whether this event can carry tool-input rewriting directives.
    pub fn is_pre_tool(&self) -> bool {
        matches!(self, Self::PreToolUse)
    }

    /// Convert to string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::SessionPause => "SessionPause",
            Self::SessionResume => "SessionResume",
            Self::AgentLoopStart => "AgentLoopStart",
            Self::AgentLoopEnd => "AgentLoopEnd",
            Self::AgentLoopTurn => "AgentLoopTurn",
            Self::AgentLoopTurnEnd => "AgentLoopTurnEnd",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::TaskUpdated => "TaskUpdated",
            Self::TaskCompleted => "TaskCompleted",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PostToolBatch => "PostToolBatch",
            Self::ToolError => "ToolError",
            Self::PreLLMCall => "PreLLMCall",
            Self::PostLLMCall => "PostLLMCall",
            Self::LLMStreamStart => "LLMStreamStart",
            Self::LLMStreamEnd => "LLMStreamEnd",
            Self::UserMessage => "UserMessage",
            Self::AssistantMessage => "AssistantMessage",
            Self::UserInputRequested => "UserInputRequested",
            Self::PreCompaction => "PreCompaction",
            Self::PostCompaction => "PostCompaction",
            Self::Error => "Error",
            Self::FatalError => "FatalError",
            Self::FileChanged => "FileChanged",
            Self::FileCreated => "FileCreated",
            Self::FileDeleted => "FileDeleted",
            Self::PermissionDenied => "PermissionDenied",
            Self::PermissionAsked => "PermissionAsked",
            Self::Notification => "Notification",
            Self::MemoryStored => "MemoryStored",
            Self::MemorySearched => "MemorySearched",
            Self::McpServerConnected => "McpServerConnected",
            Self::McpServerDisconnected => "McpServerDisconnected",
        }
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The type of hook execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookType {
    /// Execute a shell command.
    Command,
    /// Send a prompt to an LLM.
    Prompt,
    /// Spawn an agent to handle the event.
    Agent,
    /// Send an HTTP request.
    Http,
}

/// A hook definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// The event that triggers this hook.
    pub event: HookEvent,
    /// The type of hook.
    #[serde(rename = "type")]
    pub hook_type: HookType,
    /// Shell command to execute (for Command type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// LLM prompt (for Prompt type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// HTTP URL (for Http type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Condition expression (evaluated to decide if the hook runs).
    /// Supports patterns like `Bash(git *)`, `Write(src/**)`, or empty (matches all).
    /// Serialized as `condition`; `if` (TOML legacy) also accepted on read.
    #[serde(alias = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Shell to use for Command type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Run this hook asynchronously (non-blocking) — the loop continues
    /// while the hook runs in the background. Serialized as `async`.
    /// Only honored for non-blocking events (PostToolUse, …); blocking
    /// events (PreToolUse, Stop, …) always wait.
    #[serde(rename = "async", default = "default_false", skip_serializing_if = "is_false")]
    pub async_hook: bool,
    /// When an async hook exits with code 2 (blocking error), wake the
    /// running agent loop and inject the message so the model can react
    /// (Claude `asyncRewake` semantics).
    #[serde(default = "default_false", skip_serializing_if = "is_false")]
    pub async_rewake: bool,
    /// Auto-remove this hook after one execution (Claude `once`
    /// semantics) — for one-time initialization or verification hooks.
    #[serde(default = "default_false", skip_serializing_if = "is_false")]
    pub once: bool,
    /// Whether the hook is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_false() -> bool {
    false
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl HookDefinition {
    /// Compute a deduplication key for this hook.
    ///
    /// Hooks with the same type, CONDITION, and content are duplicates. The
    /// condition is part of the key: two hooks with the same command but
    /// different conditions are DISTINCT (each applies to its own matcher) —
    /// without it, the first hook's failed condition would still claim the
    /// key and skip the second, and a `once` hook would delete its sibling.
    pub fn dedup_key(&self) -> String {
        let content = match &self.hook_type {
            HookType::Command => self.command.as_deref().unwrap_or(""),
            HookType::Prompt => self.prompt.as_deref().unwrap_or(""),
            HookType::Agent => self.prompt.as_deref().unwrap_or(""),
            HookType::Http => self.url.as_deref().unwrap_or(""),
        };
        let condition = self.condition.as_deref().unwrap_or("");
        format!("{:?}:{content}:{condition}", self.hook_type)
    }
}

fn default_true() -> bool {
    true
}

/// Context passed to a hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// The event that triggered the hook.
    pub event: HookEvent,
    /// The session ID.
    pub session_id: String,
    /// The tool name (for tool-related events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// The tool arguments (for tool-related events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_args: Option<Value>,
    /// The tool result (for PostToolUse).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<String>,
    /// Additional data.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub data: HashMap<String, Value>,
}

impl HookContext {
    pub fn new(event: HookEvent, session_id: impl Into<String>) -> Self {
        Self {
            event,
            session_id: session_id.into(),
            tool_name: None,
            tool_args: None,
            tool_result: None,
            data: HashMap::new(),
        }
    }

    pub fn with_tool(mut self, name: impl Into<String>, args: Value) -> Self {
        self.tool_name = Some(name.into());
        self.tool_args = Some(args);
        self
    }

    pub fn with_result(mut self, result: impl Into<String>) -> Self {
        self.tool_result = Some(result.into());
        self
    }

    pub fn with_data(mut self, key: impl Into<String>, value: Value) -> Self {
        self.data.insert(key.into(), value);
        self
    }
}

/// The result of a hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    /// Whether the hook allows the operation to proceed.
    pub allow: bool,
    /// Optional reason for denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_reason: Option<String>,
    /// Blocking error — the operation should stop and the error be surfaced
    /// to the agent (exit code 2 semantics). Unlike a denial this is not a
    /// permission verdict; it signals "the hook itself errored" and the
    /// harness may continue the loop after correcting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_error: Option<String>,
    /// Optional output from the hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Whether the hook execution failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// PreToolUse only: rewritten tool input (Claude `updatedInput`
    /// semantics). The harness dispatches the tool with these args instead
    /// of the model's original ones, so a hook can force a safe variant
    /// (e.g. `rm *` → `rm * --dry-run`) before the permission system sees
    /// the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    /// Context to inject into the conversation for the model (Claude
    /// `additionalContext` semantics). Injected as a transient system
    /// message so the next model request sees it without persisting it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

impl Default for HookResult {
    fn default() -> Self {
        Self {
            allow: true,
            deny_reason: None,
            blocking_error: None,
            output: None,
            error: None,
            updated_input: None,
            additional_context: None,
        }
    }
}

impl HookResult {
    pub fn allow() -> Self {
        Self::default()
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            deny_reason: Some(reason.into()),
            blocking_error: None,
            output: None,
            error: None,
            updated_input: None,
            additional_context: None,
        }
    }

    pub fn blocking_error(msg: impl Into<String>) -> Self {
        Self {
            allow: true,
            deny_reason: None,
            blocking_error: Some(msg.into()),
            output: None,
            error: None,
            updated_input: None,
            additional_context: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            allow: true,
            deny_reason: None,
            blocking_error: None,
            output: None,
            error: Some(msg.into()),
            updated_input: None,
            additional_context: None,
        }
    }

    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }
}

/// The aggregate outcome of a blocking-hook gate (e.g. PreToolUse).
#[derive(Debug, Clone, Default)]
pub struct GateOutcome {
    /// Last hook-provided rewritten input for the tool call (if any).
    pub updated_input: Option<Value>,
    /// All `additionalContext` payloads collected from the executed hooks,
    /// in execution order.
    pub additional_context: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_hook(command: &str, condition: Option<&str>) -> HookDefinition {
        HookDefinition {
            event: HookEvent::PreToolUse,
            hook_type: HookType::Command,
            command: Some(command.into()),
            prompt: None,
            url: None,
            condition: condition.map(String::from),
            timeout_ms: None,
            shell: None,
            async_hook: false,
            async_rewake: false,
            once: false,
            enabled: true,
        }
    }

    #[test]
    fn dedup_key_distinguishes_conditions() {
        // Two hooks with the same command but DIFFERENT conditions are
        // distinct — the first hook's failed condition must not claim the
        // dedup key and skip the second, and a `once` removal must not
        // delete the sibling.
        let rm = command_hook("/usr/local/bin/guard.sh {tool_args}", Some("Bash(rm *)"));
        let write = command_hook("/usr/local/bin/guard.sh {tool_args}", Some("Write(src/**.py)"));
        assert_ne!(
            rm.dedup_key(),
            write.dedup_key(),
            "same command with different conditions must not collapse"
        );

        // Identical hooks (same command, same condition) still dedup.
        let rm2 = command_hook("/usr/local/bin/guard.sh {tool_args}", Some("Bash(rm *)"));
        assert_eq!(rm.dedup_key(), rm2.dedup_key());
        // A hook with no condition is distinct from one that has one.
        assert_ne!(
            command_hook("x", None).dedup_key(),
            command_hook("x", Some("Bash(rm *)")).dedup_key()
        );
    }
}
