//! Configuration types for history filtering.

/// Configuration for history filtering.
#[derive(Debug, Clone)]
pub struct FilterConfig {
    /// Whether to strip reasoning content from old turns.
    pub strip_old_reasoning: bool,
    /// Whether to truncate long tool results in old turns.
    pub truncate_old_tool_results: bool,
    /// Maximum length for truncated tool results (in chars).
    pub tool_result_max_chars: usize,
    /// Whether to remove duplicate system messages.
    pub dedup_system_messages: bool,
    /// Whether to drop ephemeral `<task-notification>` system messages
    /// from old turns. These are instant-event notifications (background
    /// task completions) that have no meaning once summarized.
    pub drop_task_notifications: bool,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            strip_old_reasoning: true,
            truncate_old_tool_results: true,
            tool_result_max_chars: 500,
            dedup_system_messages: true,
            drop_task_notifications: true,
        }
    }
}
