//! Intent types — user-intent categories and routing decisions.
/// High-level user intent categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIntent {
    /// Casual talk, greeting, thanks — no work requested.
    Chat,
    /// A question about something (code, concept, project).
    Question,
    /// Wants to understand the codebase before acting.
    Exploration,
    /// Writes, edits, or builds code.
    CodingTask,
    /// Fixes an error, bug, or failing behavior.
    DebuggingTask,
    /// Writes documentation.
    Documentation,
    /// Wants a plan/design/proposal first.
    Planning,
    /// Asks for a review of existing code.
    Review,
    /// Wants research / investigation / source gathering (Depwork 调研).
    Research,
    /// Wants creative or content output (Depwork 自媒体: 文案/脚本/PPT/卡片).
    ContentCreation,
}

impl UserIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserIntent::Chat => "chat",
            UserIntent::Question => "question",
            UserIntent::Exploration => "exploration",
            UserIntent::CodingTask => "coding_task",
            UserIntent::DebuggingTask => "debugging_task",
            UserIntent::Documentation => "documentation",
            UserIntent::Planning => "planning",
            UserIntent::Review => "review",
            UserIntent::Research => "research",
            UserIntent::ContentCreation => "content_creation",
        }
    }

    /// Whether this intent expects the agent to DO work (vs just talk).
    pub fn is_actionable(&self) -> bool {
        matches!(
            self,
            UserIntent::CodingTask
                | UserIntent::DebuggingTask
                | UserIntent::Documentation
                | UserIntent::Planning
                | UserIntent::Review
                | UserIntent::Research
                | UserIntent::ContentCreation
        )
    }

    /// Whether this intent implies code-focused work that should involve
    /// file tools — used by goal-drift detection.
    pub fn is_code_work(&self) -> bool {
        matches!(
            self,
            UserIntent::CodingTask
                | UserIntent::DebuggingTask
                | UserIntent::Review
                | UserIntent::Research
        )
    }
}

/// Result of classifying a user message.
#[derive(Debug, Clone)]
pub struct IntentResult {
    pub intent: UserIntent,
    /// Auto-drafted session goal (first meaningful sentence, truncated).
    pub goal_draft: Option<String>,
    /// Extracted acceptance hint (e.g. "tests pass", "no build errors").
    pub acceptance_hint: Option<String>,
}

/// Task-scale estimate — the execution profile for a user message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    /// Small, single-purpose (one file, one concern).
    Low,
    /// Multi-part but tractable in one agent context.
    Medium,
    /// Large, cross-cutting work — planning and/or delegation likely pay off.
    High,
}

impl TaskComplexity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Structured routing decision for a user message — what the agent is
/// being asked, how big it is, and how it should be executed.
///
/// Produced by the bounded LLM router (or the heuristic fallback). Drives:
/// - `<complexity>` / `<planning_required>` injection into `<task-spec>`
/// - delegation advice (parallel subagents vs direct execution)
/// - the task panel: `needs_planning` forces a `todo_write` plan first
#[derive(Debug, Clone)]
pub struct IntentDecision {
    pub intent: UserIntent,
    pub complexity: TaskComplexity,
    /// Multi-step work: require an explicit plan (`todo_write`) before
    /// executing — the task panel becomes the execution spine.
    pub needs_planning: bool,
    /// Parallel subagents are likely worthwhile (delegation advice).
    pub needs_subagents: bool,
    /// The message contains several distinct asks (numbered list or
    /// connector-separated clauses) — track them as separate todos.
    pub multi_intent: bool,
}

impl IntentDecision {
    /// A zero-feature decision (low complexity, no gates) — test helper.
    #[cfg(test)]
    pub fn of(intent: UserIntent) -> Self {
        Self {
            intent,
            complexity: TaskComplexity::Low,
            needs_planning: false,
            needs_subagents: false,
            multi_intent: false,
        }
    }
}
