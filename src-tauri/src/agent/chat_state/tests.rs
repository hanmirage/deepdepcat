use super::*;
use crate::core::types::ToolCall;

#[test]
fn tool_results_by_call_id_maps_executed_errors() {
    let mut state = ChatState::new("test-model", 128_000);
    state
        .conversation
        .push(ConversationItem::tool_result("tc-ok", "passed"));
    state
        .conversation
        .push(ConversationItem::tool_result_error("tc-fail", "exit 1"));
    state.conversation.push(ConversationItem::user("unrelated"));

    let map = state.tool_results_by_call_id();
    assert_eq!(map.get("tc-ok"), Some(&(false, "passed".to_string())));
    assert_eq!(map.get("tc-fail"), Some(&(true, "exit 1".to_string())));
    // Non-tool items are skipped.
    assert_eq!(map.len(), 2);
}

#[test]
fn tool_results_by_call_id_is_empty_without_results() {
    let mut state = ChatState::new("test-model", 128_000);
    state.conversation.push(ConversationItem::Assistant(
        crate::core::types::AssistantMessage {
            content: "hi".into(),
            tool_calls: vec![],
            model: None,
            usage: None,
            reasoning_content: None,
        },
    ));
    assert!(state.tool_results_by_call_id().is_empty());
}

#[test]
fn repair_inserts_synthetic_results_for_dangling_calls() {
    let mut state = ChatState::new("test-model", 128_000);
    state.conversation.push(ConversationItem::user("hello"));
    state.conversation.push(ConversationItem::Assistant(
        crate::core::types::AssistantMessage {
            content: "let me check".into(),
            tool_calls: vec![ToolCall {
                id: "tc-1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
            model: None,
            usage: None,
            reasoning_content: None,
        },
    ));
    // No tool result for tc-1 — dangling!
    state.repair_dangling_tool_calls();

    // Should have inserted a synthetic tool result
    assert_eq!(state.conversation.len(), 3);
    assert!(matches!(
        &state.conversation[2],
        ConversationItem::ToolResult(tr) if tr.tool_call_id == "tc-1" && tr.is_error
    ));
}

#[test]
fn repair_is_noop_on_clean_conversation() {
    let mut state = ChatState::new("test-model", 128_000);
    state.conversation.push(ConversationItem::user("hi"));
    state.conversation.push(ConversationItem::Assistant(
        crate::core::types::AssistantMessage {
            content: "hello".into(),
            tool_calls: vec![],
            model: None,
            usage: None,
            reasoning_content: None,
        },
    ));
    let len_before = state.conversation.len();
    state.repair_dangling_tool_calls();
    assert_eq!(state.conversation.len(), len_before);
}

#[test]
fn repair_is_idempotent() {
    let mut state = ChatState::new("test-model", 128_000);
    state.conversation.push(ConversationItem::user("hello"));
    state.conversation.push(ConversationItem::Assistant(
        crate::core::types::AssistantMessage {
            content: "checking".into(),
            tool_calls: vec![ToolCall {
                id: "tc-1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
            model: None,
            usage: None,
            reasoning_content: None,
        },
    ));
    state.repair_dangling_tool_calls();
    let len_after_first = state.conversation.len();
    state.repair_dangling_tool_calls();
    assert_eq!(state.conversation.len(), len_after_first);
}

#[test]
fn repair_dedupes_duplicate_call_ids_in_one_assistant_message() {
    let mut state = ChatState::new("test-model", 128_000);
    state.conversation.push(ConversationItem::Assistant(
        crate::core::types::AssistantMessage {
            content: "".into(),
            tool_calls: vec![
                ToolCall {
                    id: "dup-1".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
                ToolCall {
                    id: "dup-1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\": \"x\"}".into(),
                },
            ],
            model: None,
            usage: None,
            reasoning_content: None,
        },
    ));
    state.repair_dangling_tool_calls();

    // One declaration survives; the duplicate is dropped; the surviving
    // call gets exactly one synthetic result.
    let assistant = state.conversation.iter().find_map(|item| match item {
        ConversationItem::Assistant(a) => Some(a),
        _ => None,
    });
    let tool_calls = assistant.expect("assistant").tool_calls.clone();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "dup-1");
    let results = state
        .conversation
        .iter()
        .filter(|item| matches!(item, ConversationItem::ToolResult(_)))
        .count();
    assert_eq!(results, 1, "duplicate id must not produce two results");
}

#[test]
fn repair_dedupes_across_messages_and_drops_orphan_results() {
    let mut state = ChatState::new("test-model", 128_000);
    let assistant = |id: &str| ConversationItem::Assistant(
        crate::core::types::AssistantMessage {
            content: "".into(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: "bash".into(),
                arguments: "{}".into(),
            }],
            model: None,
            usage: None,
            reasoning_content: None,
        },
    );
    state.conversation.push(assistant("tc-a"));
    state.conversation.push(ConversationItem::tool_result("tc-a", "ok"));
    // Historical pollution: tc-a declared + answered AGAIN.
    state.conversation.push(assistant("tc-a"));
    state.conversation.push(ConversationItem::tool_result("tc-a", "ok again"));
    // Orphan result with no declaration at all.
    state.conversation.push(ConversationItem::tool_result("ghost", "nope"));

    state.repair_dangling_tool_calls();

    let calls: Vec<String> = state
        .conversation
        .iter()
        .flat_map(|item| match item {
            ConversationItem::Assistant(a) => a
                .tool_calls
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(calls, vec!["tc-a".to_string()]);
    let results: Vec<&str> = state
        .conversation
        .iter()
        .filter_map(|item| match item {
            ConversationItem::ToolResult(tr) => Some(tr.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results, vec!["tc-a"]);
}

#[test]
fn transient_system_caps_oversized_entries() {
    let mut state = ChatState::new("test-model", 128_000);
    // A huge background-subagent result must be truncated at the transient
    // guard so it cannot bloat every subsequent request.
    let big = "r".repeat(100_000);
    state.push_transient_system(big);
    let stored = state.request_messages();
    assert_eq!(stored.len(), 1);
    let text = match &stored[0] {
        ConversationItem::System(s) => s.content.clone(),
        other => panic!("expected system message, got {other:?}"),
    };
    assert!(
        text.len() < 100_000,
        "oversized transient must be truncated (stored {})",
        text.len()
    );
    assert!(
        text.contains(crate::core::str_util::TOOL_OUTPUT_TRUNCATED_HINT),
        "truncated entry must carry the tail hint"
    );
}

#[test]
fn transient_system_passes_small_entries_through() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_transient_system("verify the change");
    let stored = state.request_messages();
    let text = match &stored[0] {
        ConversationItem::System(s) => s.content.clone(),
        other => panic!("expected system message, got {other:?}"),
    };
    assert_eq!(text, "verify the change");
}

#[test]
fn push_user_message_clears_transient_system() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_transient_system("[SYSTEM REMINDER] don't re-summarize");
    assert!(!state.transient_system.is_empty());
    state.push_user_message("new turn");
    assert!(
        state.transient_system.is_empty(),
        "a new user turn must drop one-shot reminders from the previous turn"
    );
}

#[test]
fn transient_system_dedups_identical_entries() {
    let mut state = ChatState::new("test-model", 128_000);
    // The anti-summary reminder is pushed after every serial tool result —
    // identical text must not stack (one injection per turn carries it).
    state.push_transient_system("[SYSTEM REMINDER] don't re-summarize");
    state.push_transient_system("[SYSTEM REMINDER] don't re-summarize");
    assert_eq!(state.transient_system.len(), 1);
    // Non-consecutive identical pushes are deduped too.
    state.push_transient_system("different guidance");
    state.push_transient_system("[SYSTEM REMINDER] don't re-summarize");
    assert_eq!(state.transient_system.len(), 2);
}

#[test]
fn full_request_estimate_counts_transient_and_tail() {
    let mut state = ChatState::new("test-model", 128_000);
    state.set_system_prompt("sys");
    state
        .conversation
        .push(ConversationItem::user("hello world"));
    state.push_transient_system("transient guidance text");
    let base = state.estimated_request_tokens(&[]);
    let full = state.estimated_full_request_tokens("sys", Some("tail text"), 100, &[]);
    assert!(
        full > base,
        "full estimate must count transient + tail + allowance (base {base}, full {full})"
    );
    // Without tail/allowance the two only differ by the transient.
    let no_tail = state.estimated_full_request_tokens("sys", None, 0, &[]);
    assert!(
        no_tail > base,
        "transient system entries must be included (base {base}, no_tail {no_tail})"
    );
}

#[test]
fn transient_system_still_caps_at_max_within_turn() {
    let mut state = ChatState::new("test-model", 128_000);
    // MAX_TRANSIENT_SYSTEM is 32 — push more to verify the cap still holds
    // within a single turn (the clear only fires on a NEW user turn).
    for i in 0..40 {
        state.push_transient_system(format!("reminder {i}"));
    }
    assert!(
        state.transient_system.len() <= 32,
        "transient system capped: {}",
        state.transient_system.len()
    );
}

#[test]
fn push_transient_image_then_request_messages_contains_image_user() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_transient_image("image/png", "aGVsbG8=");
    let stored = state.request_messages();
    let last = stored.last().expect("image user message appended");
    match last {
        ConversationItem::User(u) => {
            assert!(
                matches!(&u.content[0], ContentPart::Image { media_type, .. } if media_type == "image/png")
            );
        }
        other => panic!("expected user message, got {other:?}"),
    }
}

#[test]
fn push_user_message_clears_transient_images() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_transient_image("image/png", "aGVsbG8=");
    assert!(!state.transient_images.is_empty());
    state.push_user_message("new turn");
    assert!(
        state.transient_images.is_empty(),
        "a new user turn must drop transient images"
    );
}

#[test]
fn transient_images_capped_at_max() {
    let mut state = ChatState::new("test-model", 128_000);
    for i in 0..7 {
        state.push_transient_image("image/png", format!("data-{i}"));
    }
    assert_eq!(
        state.transient_images.len(),
        5,
        "only the newest MAX_TRANSIENT_IMAGES survive"
    );
    // The oldest (data-0, data-1) are dropped; data-6 is the newest.
    let newest = state.transient_images.last().unwrap();
    let ContentPart::Image { data, .. } = newest else {
        panic!("expected image part");
    };
    assert_eq!(data, "data-6");
}

#[test]
fn initial_image_parts_consumed_once_by_request_messages() {
    let mut state = ChatState::new("test-model", 128_000);
    state.set_initial_image_parts(vec![ContentPart::Image {
        source_type: "base64".into(),
        media_type: "image/png".into(),
        data: "aGVsbG8=".into(),
    }]);
    // First request carries the image as a trailing user message.
    let first = state.request_messages();
    assert!(first.iter().any(|m| matches!(m, ConversationItem::User(u)
        if u.content.iter().any(|p| matches!(p, ContentPart::Image { .. })))));
    // Second request must NOT repeat it — the image is consumed exactly once.
    let second = state.request_messages();
    assert!(
        second.iter().all(|m| !matches!(m, ConversationItem::User(u)
            if u.content.iter().any(|p| matches!(p, ContentPart::Image { .. })))),
        "send-time images must appear in exactly one request"
    );
}

fn tool_call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "read_file".into(),
        arguments: "{}".into(),
    }
}

#[test]
fn tool_result_batch_budget_passes_within_budget_untouched() {
    let mut state = ChatState::new("test-model", 128_000);
    let budget = tool_result_batch_budget_for(128_000);
    assert_eq!(budget, 64_000);
    state.push_assistant_message("batch", vec![tool_call("t1")], None, None);
    // A result at exactly the budget passes through unchanged.
    let big = "x".repeat(budget as usize);
    state.push_tool_result("t1", big.clone(), false);
    assert!(matches!(
        &state.conversation[1],
        ConversationItem::ToolResult(tr) if tr.content == big
    ));
}

#[test]
fn tool_result_batch_budget_suppresses_when_exhausted() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_assistant_message("batch", vec![tool_call("t1"), tool_call("t2")], None, None);
    let budget = tool_result_batch_budget_for(128_000);
    state.push_tool_result("t1", "x".repeat(budget as usize), false);
    // Budget is now fully used — the second result is suppressed.
    state.push_tool_result("t2", "Y".repeat(5_000), false);
    assert!(matches!(
        &state.conversation[2],
        ConversationItem::ToolResult(tr) if tr.content.contains("suppressed")
    ));
    assert!(
        !tr_content(&state.conversation[2]).contains('Y'),
        "suppressed result must not carry the original content"
    );
}

#[test]
fn tool_result_batch_budget_truncates_partial_overage() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_assistant_message("batch", vec![tool_call("t1"), tool_call("t2")], None, None);
    // First result uses half the budget; the second needs 40k but only 32k
    // remains → truncated to the remaining 32k + an explicit hint.
    state.push_tool_result("t1", "a".repeat(32_000), false);
    state.push_tool_result("t2", "b".repeat(40_000), false);
    let tr2 = tr_content(&state.conversation[2]);
    assert!(
        tr2.starts_with(&"b".repeat(32_000)),
        "head of the overage must be preserved within the remaining budget"
    );
    assert!(tr2.contains("suppressed"));
    // Budget fully used now — a third result is suppressed outright.
    state.push_tool_result("t3", "c".repeat(1_000), false);
    assert!(tr_content(&state.conversation[3]).contains("suppressed"));
}

#[test]
fn tool_result_batch_budget_resets_per_batch() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_assistant_message("batch 1", vec![tool_call("t1"), tool_call("t2")], None, None);
    state.push_tool_result("t1", "x".repeat(40_000), false);
    state.push_tool_result("t2", "y".repeat(40_000), false);
    assert!(tr_content(&state.conversation[2]).contains("suppressed"));
    // A new assistant message with tool calls resets the budget — a fresh
    // result at the full budget passes through again.
    state.push_assistant_message("batch 2", vec![tool_call("t3")], None, None);
    let big = "z".repeat(tool_result_batch_budget_for(128_000) as usize);
    state.push_tool_result("t3", big.clone(), false);
    assert!(matches!(
        &state.conversation[4],
        ConversationItem::ToolResult(tr) if tr.content == big
    ));
}

#[test]
fn tool_result_batch_budget_disabled_without_window() {
    let mut state = ChatState::new("test-model", 0);
    // Uninitialized window → budget 0 → no cap applied.
    state.push_assistant_message("batch", vec![tool_call("t1")], None, None);
    let content = "k".repeat(10_000);
    state.push_tool_result("t1", content.clone(), false);
    assert!(matches!(
        &state.conversation[1],
        ConversationItem::ToolResult(tr) if tr.content == content
    ));
}

#[test]
fn tool_result_budget_scales_with_window_and_clamps() {
    // 32k window → floor.
    assert_eq!(tool_result_batch_budget_for(32_768), 16_384);
    // 64k window (DeepSeek R1) → 32k.
    assert_eq!(tool_result_batch_budget_for(65_536), 32_768);
    // 128k window (DeepSeek V3) → 64k.
    assert_eq!(tool_result_batch_budget_for(128_000), 64_000);
    // 1M window → capped.
    assert_eq!(tool_result_batch_budget_for(1_000_000), 96_000);
    // Uninitialized window disables the cap.
    assert_eq!(tool_result_batch_budget_for(0), 0);
}

fn tr_content(item: &ConversationItem) -> &str {
    match item {
        ConversationItem::ToolResult(tr) => &tr.content,
        _ => panic!("expected ToolResult item"),
    }
}

#[test]
fn replace_conversation_keeps_cumulative_usage() {
    let mut state = ChatState::new("test-model", 128_000);
    state.push_user_message("first");
    state.push_assistant_message("reply", vec![], None, None);
    // Simulate accumulated billing across prior turns.
    state
        .total_usage
        .add(&crate::core::types::TokenUsage {
            prompt_tokens: 50_000,
            completion_tokens: 10_000,
            ..Default::default()
        });
    let before = state.total_usage.prompt_tokens;
    // Compaction replaces the conversation with a summary + tail.
    state.replace_conversation(vec![ConversationItem::system("summary")]);
    // The cumulative billed prompt tokens must survive — re-estimating to the
    // compacted size would erase the pre-compaction spend and under-seed the
    // session budget.
    assert_eq!(state.total_usage.prompt_tokens, before);
}
