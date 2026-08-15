use serde::{Deserialize, Serialize};

use crate::core::types::TokenUsage;

/// Workspace isolation mode for a subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    /// No isolation — runs in the same workspace as the parent.
    #[default]
    None,
    /// Git worktree isolation — runs in a separate worktree.
    Worktree,
}

/// The type of subagent to spawn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SubagentType {
    /// General-purpose agent (inherits all tools).
    #[default]
    General,
    /// Explore agent (read-only, no project memory).
    Explore,
    /// Plan agent (read-only, no git context).
    Plan,
    /// Evaluator agent — INDEPENDENT reviewer for the generate-review loop:
    /// isolated context (never sees the generator's reasoning), read-only +
    /// verification tools (bash to run tests, LSP diagnostics), and a
    /// skeptical prompt that grades against the task rather than trusting
    /// the generator's self-report. Never edits files.
    Evaluator,
    /// Custom agent (defined by user via roles).
    Custom(String),
}

impl SubagentType {
    /// Whether this type should only have read-only tools.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Explore | Self::Plan)
    }

    /// String representation for event payloads.
    pub fn as_str(&self) -> &str {
        match self {
            Self::General => "general",
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Evaluator => "evaluator",
            Self::Custom(name) => name,
        }
    }
}

/// Configuration for spawning a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    pub agent_type: SubagentType,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub max_turns: u32,
    pub depth: u32,
    /// Whether this subagent runs in the background (non-blocking).
    /// When true, the tool returns immediately with a task ID and the
    /// result is injected into the parent's next conversation turn.
    #[serde(default)]
    pub background: bool,
    /// Whether the subagent's completion is surfaced to the parent agent.
    /// Harness-internal subagents (e.g. coordinator workers) set this to
    /// `false` so the parent never sees their completion reminder.
    #[serde(default = "default_true")]
    pub surface_completion: bool,
    /// Worktree isolation mode. `Worktree` runs the subagent in a git
    /// worktree (requires a git workspace); creation failures fall back
    /// to the parent workspace.
    #[serde(default)]
    pub isolation: IsolationMode,
    /// Wall-clock timeout for the whole subagent execution (main turn +
    /// follow-ups). `None` = unlimited (until max_turns or cancellation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Pre-assigned task ID (background subagents). When set, the worker
    /// registry keys on this ID so `task_stop`/`send_message` can target
    /// the worker by the task ID the tool returned to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The parent's `agent` tool call ID — emitted on subagent lifecycle
    /// events so the frontend can link a spawned worker back to the tool
    /// call that created it ("" when unknown, e.g. decompose workers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// Fork mode: the subagent inherits a compressed snapshot of the parent
    /// conversation (`fork_context`) so it starts with the exploration
    /// background instead of a blank slate.
    #[serde(default)]
    pub fork: bool,
    /// Parent conversation snapshot for fork mode. Only meaningful when
    /// `fork` is true; the snapshot is compressed via `fork_context()`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fork_context: Vec<crate::core::types::ConversationItem>,
    /// Product work mode inherited from the parent agent ("code"/"depwork").
    /// The subagent's tool registry is filtered and its context builder is
    /// seeded with this mode so a depwork parent never spawns a code-mode
    /// child (and vice versa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_mode: Option<String>,
    /// Parent session id — emitted on subagent lifecycle events so the
    /// frontend can route them to the right mode's store (only the store
    /// for the session that spawned the worker may render them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// File paths this worker intends to WRITE (for write-conflict preflight
    /// against parallel siblings). Empty = unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// `(name, path)` notes for images attached to the parent's current user
    /// message (text-only main model path). Non-fork subagents get these
    /// injected into their task context so they can `visual_describe` a
    /// picture by path. Fork subagents already inherit them via the forked
    /// conversation; multimodal parents never populate this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_notes: Vec<(String, String)>,
    /// Deny rules inherited from the parent agent chain (raw `Tool(pattern)`
    /// strings). Merged into the child's agent rule set at spawn, and
    /// forwarded again to grandchildren — a hard deny propagates through
    /// the whole nesting tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherited_denies: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            agent_type: SubagentType::General,
            task: String::new(),
            model: None,
            max_turns: 20,
            depth: 0,
            background: false,
            surface_completion: true,
            isolation: IsolationMode::None,
            timeout_secs: None,
            task_id: None,
            call_id: None,
            fork: false,
            fork_context: Vec::new(),
            work_mode: None,
            session_id: None,
            paths: None,
            image_notes: Vec::new(),
            inherited_denies: Vec::new(),
        }
    }
}

/// The result of a subagent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub response: String,
    pub modified_files: Vec<String>,
    pub usage: TokenUsage,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Definition of a worker in coordinator mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerDefinition {
    /// Human-readable name for this worker.
    pub name: String,
    /// The task this worker should accomplish.
    pub task: String,
    /// The type of subagent to spawn.
    #[serde(default)]
    pub agent_type: SubagentType,
    /// Optional model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Max turns for this worker.
    #[serde(default = "default_worker_turns")]
    pub max_turns: u32,
    /// File paths this worker intends to WRITE (for write-conflict preflight).
    /// Empty/absent = unknown — treated as "may touch anything" when checking
    /// parallel workers against each other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

fn default_worker_turns() -> u32 {
    15
}

/// Result of a completed background subagent.
///
/// Collected by `MultiAgentCoordinator::drain_background_results()` and
/// injected into the parent agent's conversation at the start of the next
/// turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundSubagentResult {
    /// The task ID assigned when the background subagent was spawned.
    pub task_id: String,
    /// The original task description.
    pub task: String,
    /// The subagent's result.
    pub result: SubagentResult,
    /// Whether the completion should be surfaced to the parent agent.
    /// `false` for harness-internal subagents — the result is collected
    /// but never injected into the parent conversation.
    pub surface_completion: bool,
    /// The session that spawned this background subagent — results are
    /// drained per-session so concurrent sessions never cross-contaminate.
    pub session_id: String,
}
