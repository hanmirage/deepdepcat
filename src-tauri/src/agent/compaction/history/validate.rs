//! Post-compaction validation — ensures structural invariants hold.

use crate::core::types::ConversationItem;

/// Validate that a compacted conversation has no orphaned tool results.
///
/// An orphaned tool result is one whose `tool_call_id` has no matching
/// assistant message with that tool call.
pub fn validate_no_orphans(items: &[ConversationItem]) -> Result<(), String> {
    let mut answered_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for item in items {
        if let ConversationItem::Assistant(a) = item {
            for tc in &a.tool_calls {
                answered_ids.insert(&tc.id);
            }
        }
    }

    for item in items {
        if let ConversationItem::ToolResult(tr) = item {
            if !answered_ids.contains(tr.tool_call_id.as_str()) {
                return Err(format!(
                    "Orphaned tool result: {} has no matching assistant tool call",
                    tr.tool_call_id
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ToolCall;

    #[test]
    fn validate_passes_on_clean_conversation() {
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
            ConversationItem::tool_result("tc-1", "result"),
        ];

        assert!(validate_no_orphans(&items).is_ok());
    }

    #[test]
    fn validate_fails_on_orphan() {
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
            ConversationItem::tool_result("tc-orphan", "result"),
        ];

        let err = validate_no_orphans(&items);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("tc-orphan"));
    }
}
