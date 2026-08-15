//! Conversation filtering — strips redundant content from old turns.

use super::types::FilterConfig;
use crate::core::types::ConversationItem;

/// Whether a system message is an ephemeral `<task-notification>`.
///
/// These are instant-event notifications (background task completions)
/// produced by the notification subsystem; once the turn is summarized
/// they carry no information and would only consume context.
fn is_ephemeral_notification(content: &str) -> bool {
    content.contains("<task-notification>")
}

/// Filter conversation history items according to the config.
///
/// Returns a new vector with filtered items. The original items are
/// not modified.
pub fn filter_history(items: &[ConversationItem], config: &FilterConfig) -> Vec<ConversationItem> {
    let mut result = Vec::with_capacity(items.len());
    let mut seen_system = false;

    for item in items {
        match item {
            ConversationItem::System(s) => {
                // Drop ephemeral task notifications from old turns — they
                // are instant events with no meaning once summarized.
                if config.drop_task_notifications && is_ephemeral_notification(&s.content) {
                    continue;
                }
                if config.dedup_system_messages && seen_system {
                    continue;
                }
                seen_system = true;
                result.push(item.clone());
            }
            ConversationItem::Reasoning(_) if config.strip_old_reasoning => {
                // Skip reasoning content from old turns
            }
            ConversationItem::ToolResult(tr)
                if config.truncate_old_tool_results
                    && tr.content.len() > config.tool_result_max_chars =>
            {
                let truncated = crate::core::str_util::truncate_at_char_boundary(
                    &tr.content,
                    config.tool_result_max_chars,
                );
                result.push(ConversationItem::tool_result(
                    &tr.tool_call_id,
                    format!("{}...(truncated)", truncated),
                ));
            }
            _ => {
                result.push(item.clone());
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ConversationItem, ReasoningMessage, ToolCall};

    #[test]
    fn filter_strips_reasoning() {
        let items = vec![
            ConversationItem::user("hello"),
            ConversationItem::Reasoning(ReasoningMessage {
                content: "thinking...".into(),
                encrypted_content: None,
            }),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "response".into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];

        let filtered = filter_history(&items, &FilterConfig::default());
        assert_eq!(filtered.len(), 2);
        assert!(!filtered
            .iter()
            .any(|i| matches!(i, ConversationItem::Reasoning(_))));
    }

    #[test]
    fn filter_truncates_long_tool_results() {
        let long_content = "x".repeat(1000);
        let items = vec![
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "checking".into(),
                tool_calls: vec![ToolCall {
                    id: "tc-1".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                }],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
            ConversationItem::tool_result("tc-1", &long_content),
        ];

        let config = FilterConfig {
            tool_result_max_chars: 100,
            ..Default::default()
        };
        let filtered = filter_history(&items, &config);
        if let ConversationItem::ToolResult(tr) = &filtered[1] {
            assert!(tr.content.len() < long_content.len());
            assert!(tr.content.contains("truncated"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn filter_dedups_system_messages() {
        let items = vec![
            ConversationItem::system("sys1"),
            ConversationItem::system("sys2"),
            ConversationItem::user("hello"),
        ];

        let filtered = filter_history(&items, &FilterConfig::default());
        assert_eq!(filtered.len(), 2);
        if let ConversationItem::System(s) = &filtered[0] {
            assert_eq!(s.content, "sys1");
        }
    }

    #[test]
    fn filter_drops_ephemeral_task_notifications() {
        let items = vec![
            ConversationItem::system(
                "<task-notification>\n<task-id>bg-1</task-id>\n</task-notification>",
            ),
            ConversationItem::system("durable guidance"),
            ConversationItem::user("hello"),
        ];

        let filtered = filter_history(&items, &FilterConfig::default());
        assert_eq!(filtered.len(), 2);
        assert!(!filtered.iter().any(
            |i| matches!(i, ConversationItem::System(s) if s.content.contains("task-notification"))
        ));
    }

    #[test]
    fn filter_keeps_task_notifications_when_disabled() {
        let items = vec![ConversationItem::system(
            "<task-notification>\n<task-id>bg-1</task-id>\n</task-notification>",
        )];
        let config = FilterConfig {
            drop_task_notifications: false,
            ..Default::default()
        };
        let filtered = filter_history(&items, &config);
        assert_eq!(filtered.len(), 1);
    }
}
