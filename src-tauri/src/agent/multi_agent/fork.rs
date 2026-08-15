//! Fork subagent — creates a child agent that shares the parent's prompt
//! cache for cost-efficient context inheritance.
//!
//! Unlike a regular subagent (which gets an independent context), a fork
//! subagent constructs its messages to maximize API prompt cache hits:
//! the system prompt, user messages, and assistant turn are identical to
//! the parent's, so the API charges only for the new fork instructions.

use crate::core::types::{ContentPart, ConversationItem};

/// XML tags whose content is stripped from messages during fork context
/// normalization — these blocks are re-injected by the child session's
/// system prompt builder, so including them is pure duplication.
const FORK_NOISE_TAGS: &[&str] = &[
    "system-reminder",
    "system_reminder",
    "user_info",
    "git_status",
    "project_layout",
    "attached_files",
];

/// Strip known noise XML blocks from a text fragment.
fn strip_noise_tags(text: &str) -> String {
    let mut result = text.to_string();
    for tag in FORK_NOISE_TAGS {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = result.find(&open) {
            let Some(after_open) = result[start..].find(&close) else {
                break;
            };
            let end = start + after_open + close.len();
            result.replace_range(start..end, "");
        }
    }
    result
}

/// Extract plain text from a conversation item (tool calls excluded).
///
/// The parent's reasoning content is deliberately NOT forwarded: a worker
/// must execute its assigned task, not continue the parent's internal
/// train of thought — inherited reasoning is how fork workers drift into
/// acting like the parent (scope creep).
fn item_text(item: &ConversationItem) -> String {
    let raw = match item {
        ConversationItem::System(s) => s.content.clone(),
        ConversationItem::User(u) => u
            .content
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        ConversationItem::Assistant(a) => a.content.clone(),
        ConversationItem::ToolResult(tr) => tr.content.clone(),
        // Reasoning content is deliberately never forwarded (see fn doc) —
        // this arm is a defensive fallback so a stray call site cannot leak
        // the parent's internal chain of thought to a worker.
        ConversationItem::Reasoning(_) => String::new(),
    };
    strip_noise_tags(&raw)
}

/// Human-readable role label for a conversation item.
fn role_label(item: &ConversationItem) -> &'static str {
    match item {
        ConversationItem::System(_) => "System",
        ConversationItem::User(_) => "User",
        ConversationItem::Assistant(_) => "Assistant",
        ConversationItem::ToolResult(_) => "ToolResult",
        ConversationItem::Reasoning(_) => "Reasoning",
    }
}

/// Normalize the parent's conversation history into a compact fork context.
///
/// Strategy (from upstream patterns):
/// - Keep the last 3 turns verbatim for context coherence.
/// - Summarize earlier turns into a brief statistical overview.
/// - Remove redundant XML blocks (system-reminder, git-status).
/// - Place the task prompt last (maximizes recency attention).
pub fn normalize_fork_context(messages: &[ConversationItem], task: &str) -> Vec<ConversationItem> {
    let mut result = Vec::new();

    // System placeholder — the fork agent's system prompt will be set by the
    // caller. We inject a minimal context marker here.
    result.push(ConversationItem::system(
        "You are a fork subagent — a focused worker delegated one specific task. \
         You are NOT the main assistant. Complete the assigned task only; do not \
         broaden scope beyond what was asked. Follow the task instructions at the \
         end of this message.",
    ));

    // Build the background context from the parent's messages.
    let total = messages.len();
    let recent_count = total.min(6); // Last 3 turns ≈ 6 messages (user+assistant pairs)

    let mut context_parts = Vec::new();

    // Summary of earlier messages.
    if total > recent_count {
        let earlier = &messages[..total - recent_count];
        let user_count = earlier
            .iter()
            .filter(|m| matches!(m, ConversationItem::User(_)))
            .count();
        let assistant_count = earlier
            .iter()
            .filter(|m| matches!(m, ConversationItem::Assistant(_)))
            .count();
        let tool_count = earlier
            .iter()
            .filter_map(|m| match m {
                ConversationItem::Assistant(a) => Some(a.tool_calls.len()),
                _ => None,
            })
            .sum::<usize>();

        context_parts.push(format!(
            "## Earlier conversation (summarized)\n\
             {user_count} user messages, {assistant_count} assistant responses, \
             {tool_count} tool calls were made before this point.\n"
        ));
    }

    // Recent turns — kept verbatim for context coherence.
    if recent_count > 0 {
        let recent = &messages[total - recent_count..];
        let mut recent_text = String::from("## Recent conversation\n");
        for msg in recent {
            // Reasoning items are skipped outright — inherited chain-of-thought
            // is how fork workers drift into acting like the parent.
            if matches!(msg, ConversationItem::Reasoning(_)) {
                continue;
            }
            let role = role_label(msg);
            let content = item_text(msg).chars().take(500).collect::<String>();
            recent_text.push_str(&format!("[{role}]: {content}\n"));
        }
        context_parts.push(recent_text);
    }

    // Task prompt — placed last for maximum recency attention. Sanitized:
    // the task is user-influenced text (parent model tool argument) and
    // must not forge harness frames or template placeholders.
    let safe_task = crate::agent::sanitize::sanitize_injection_slot(task);
    context_parts.push(format!("## Task\n{safe_task}"));

    // Combine all context into a single user message.
    result.push(ConversationItem::user(context_parts.join("\n\n")));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_empty_conversation() {
        let result = normalize_fork_context(&[], "do something");
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], ConversationItem::System(_)));
        assert!(matches!(result[1], ConversationItem::User(_)));
        assert!(item_text(&result[1]).contains("do something"));
    }

    #[test]
    fn normalize_keeps_recent_verbatim() {
        let messages = vec![
            ConversationItem::user("old message 1"),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "old response 1".into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
            ConversationItem::user("recent message"),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "recent response".into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        let result = normalize_fork_context(&messages, "do the task");
        // Should contain the recent message verbatim
        let combined: String = result.iter().map(item_text).collect();
        assert!(combined.contains("recent message"));
    }

    #[test]
    fn normalize_drops_parent_reasoning() {
        let messages = vec![
            ConversationItem::user("question"),
            ConversationItem::Reasoning(crate::core::types::ReasoningMessage {
                content: "internal chain of thought that must not leak".to_string(),
                encrypted_content: None,
            }),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "answer".into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        let result = normalize_fork_context(&messages, "do the task");
        let combined: String = result.iter().map(item_text).collect();
        assert!(
            !combined.contains("internal chain of thought"),
            "parent reasoning must not reach the fork context"
        );
        assert!(combined.contains("answer"), "assistant content still forwarded");
    }

    #[test]
    fn normalize_summarizes_earlier() {
        let assistant = |content: &str| {
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: content.into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            })
        };
        let messages = vec![
            ConversationItem::user("msg 1"),
            assistant("resp 1"),
            ConversationItem::user("msg 2"),
            assistant("resp 2"),
            ConversationItem::user("msg 3"),
            assistant("resp 3"),
            ConversationItem::user("recent msg"),
            assistant("recent resp"),
        ];
        let result = normalize_fork_context(&messages, "do the task");
        let combined: String = result.iter().map(item_text).collect();
        // Should contain summary statistics for earlier messages
        assert!(combined.contains("summarized") || combined.contains("Earlier"));
    }

    #[test]
    fn strips_system_reminder_blocks() {
        let text = "Do the work.\n<system-reminder>hidden reminder</system-reminder>\nDone.";
        let cleaned = strip_noise_tags(text);
        assert!(!cleaned.contains("hidden reminder"));
        assert!(cleaned.contains("Do the work."));
        assert!(cleaned.contains("Done."));
    }

    #[test]
    fn strips_underscore_reminder_variant() {
        let text = "<system_reminder>variant</system_reminder>rest";
        let cleaned = strip_noise_tags(text);
        assert_eq!(cleaned, "rest");
    }

    #[test]
    fn reasoning_content_is_not_forwarded_to_fork() {
        // The parent's internal train of thought must not reach the worker —
        // inherited reasoning is how fork workers drift into acting like the
        // parent (scope creep).
        let messages = vec![
            ConversationItem::user("investigate the auth module"),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "The auth module uses JWT in src/auth.rs".to_string(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: Some(
                    "I should check the token refresh path next, and also whether \
                     the middleware order matters..."
                        .to_string(),
                ),
            }),
        ];
        let result = normalize_fork_context(&messages, "fix the JWT bug");
        let combined: String = result.iter().map(item_text).collect();
        assert!(
            combined.contains("The auth module uses JWT in src/auth.rs"),
            "concluded content must be forwarded"
        );
        assert!(
            !combined.contains("token refresh path"),
            "reasoning must not be forwarded: {combined}"
        );
    }

    #[test]
    fn normalizing_fork_strips_reminders() {
        let messages = vec![
            ConversationItem::user("hello <system-reminder>internal</system-reminder>"),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "response".into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        let result = normalize_fork_context(&messages, "task");
        let combined: String = result.iter().map(item_text).collect();
        assert!(!combined.contains("internal"));
        assert!(combined.contains("hello"));
    }
}
