//! Turn selection for compaction — finds a safe split point that keeps
//! recent items within a token budget and compacts the rest.
//!
//! The split point must respect the tool-pair invariant: an assistant item
//! with tool calls and its subsequent tool results must stay together.
//! Splitting between them would produce orphaned tool results that the
//! model API rejects.

use crate::core::types::ConversationItem;

/// Output of [`select_turns_to_compact`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitPlan {
    /// Compact items at indices `0..split_idx`. Keep `split_idx..total`.
    pub split_idx: usize,
    /// Sum of tokens in the compactable region.
    pub tokens_to_compact: u32,
}

/// A contiguous run of items to summarize in one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRange {
    /// Start index (inclusive).
    pub start: usize,
    /// End index (exclusive).
    pub end: usize,
}

/// Default token budget per compaction chunk. Keeps each LLM summarization
/// call comfortably within the model context window while still amortizing
/// the cost over fewer calls than item-by-item.
pub const COMPACTION_CHUNK_TOKEN_BUDGET: u32 = 8_000;

/// Split `items` into contiguous chunks, each under `chunk_budget` tokens.
///
/// Used by DivideAndConquer compaction: an overly long conversation gets
/// summarized in several independent passes instead of one oversized call
/// that risks context overflow and detail loss. A single chunk is produced
/// when the total stays under the budget (the common case) — this is a
/// pure function, so it is unit-testable without a live LLM.
///
/// A single item larger than the budget gets its own chunk (never split
/// mid-item — an item is atomic for summarization).
pub fn plan_compaction_chunks(
    items: &[ConversationItem],
    item_tokens: &[u32],
    chunk_budget: u32,
) -> Vec<ChunkRange> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut chunks: Vec<ChunkRange> = Vec::new();
    let mut start = 0usize;
    let mut acc: u32 = 0;
    let mut i = 0usize;
    while i < items.len() {
        let tokens = item_tokens[i];
        // Flush the current chunk when adding this item would exceed the
        // budget AND we already have items in it. Snap the boundary forward
        // so an assistant-with-tool-calls and its results stay in ONE chunk
        // (a summarizer that sees a dangling tool call, or an orphaned result
        // with no caller, cannot recover the pairing — see the prefix/tail
        // `snap_to_safe_boundary` for the same invariant).
        if acc > 0 && acc.saturating_add(tokens) > chunk_budget {
            let end = snap_to_safe_boundary(items, i);
            chunks.push(ChunkRange { start, end });
            start = end;
            acc = 0;
            i = end;
            continue;
        }
        acc = acc.saturating_add(tokens);
        i += 1;
    }
    if start < items.len() {
        chunks.push(ChunkRange {
            start,
            end: items.len(),
        });
    }
    chunks
}

/// Decide where to split the conversation for compaction.
///
/// Algorithm:
/// 1. Walk backward from the newest item, accumulating "keep" tokens.
/// 2. The candidate split index is the first one where adding more would
///    exceed `target_tokens`.
/// 3. **Snap forward** to a safe boundary: if the split would orphan tool
///    results, walk forward until past the matching tool-result items.
/// 4. Return `None` if the compactable region's token count is below
///    `min_compactable` — not worth an LLM call.
pub fn select_turns_to_compact(
    items: &[ConversationItem],
    token_counts: &[u32],
    target_tokens: u32,
    min_compactable: u32,
) -> Option<SplitPlan> {
    debug_assert_eq!(
        token_counts.len(),
        items.len(),
        "token counts and items must have the same length"
    );

    let total = items.len();
    if total == 0 {
        return None;
    }

    // Step 1: Walk backward, sum "keep" tokens until target is reached.
    let mut kept = 0u32;
    let mut split_idx = total;

    while split_idx > 0 {
        let next_tokens = token_counts[split_idx - 1];
        if kept + next_tokens > target_tokens {
            break;
        }
        kept += next_tokens;
        split_idx -= 1;
    }

    // Step 2: Don't compact if we'd keep everything.
    if split_idx == 0 {
        return None;
    }

    // Step 3: Snap forward to a safe boundary.
    split_idx = snap_to_safe_boundary(items, split_idx);

    // Step 4: Check minimum compactable tokens.
    let tokens_to_compact: u32 = token_counts[..split_idx].iter().sum();
    if tokens_to_compact < min_compactable {
        return None;
    }

    Some(SplitPlan {
        split_idx,
        tokens_to_compact,
    })
}

/// Walk the split index forward until it lands on a safe boundary.
///
/// The split must not fall between an assistant-with-tool-calls and its
/// tool results. If it does, walk forward past the tool results.
fn snap_to_safe_boundary(items: &[ConversationItem], mut split_idx: usize) -> usize {
    // The split must not land on a tool result — if it does, walk forward
    // past all tool results that belong to the preceding assistant.
    while split_idx < items.len() {
        match &items[split_idx] {
            ConversationItem::ToolResult(_) => {
                split_idx += 1;
            }
            ConversationItem::Assistant(a) if !a.tool_calls.is_empty() => {
                // We landed on an assistant with tool calls — the tool results
                // following it must be kept together. Walk forward.
                split_idx += 1;
            }
            _ => break,
        }
    }
    split_idx
}

/// Find a safe split point keeping the RECENT tail within `budget` tokens
/// (ronx cache-first design): walk backwards accumulating per-item tokens
/// until the budget is consumed; the split is the index where the tail
/// begins. Falls back to a fraction when items are empty.
pub fn select_by_token_budget(items: &[ConversationItem], budget: u64) -> usize {
    let total = items.len();
    if total == 0 {
        return 0;
    }
    let counts = super::item::item_token_counts(items);
    let mut acc: u64 = 0;
    let mut start = total;
    for (i, c) in counts.iter().enumerate().rev() {
        acc += *c as u64;
        if acc > budget {
            break;
        }
        start = i;
    }
    snap_to_safe_boundary(items, start.min(total - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ConversationItem, ToolCall};

    fn make_conversation(n: usize) -> Vec<ConversationItem> {
        (0..n)
            .flat_map(|i| {
                vec![
                    ConversationItem::user(format!("user msg {i}")),
                    ConversationItem::Assistant(crate::core::types::AssistantMessage {
                        content: format!("assistant reply {i}"),
                        tool_calls: vec![],
                        model: None,
                        usage: None,
                        reasoning_content: None,
                    }),
                ]
            })
            .collect()
    }

    #[test]
    fn select_returns_none_for_empty() {
        let plan = select_turns_to_compact(&[], &[], 1000, 10);
        assert!(plan.is_none());
    }

    #[test]
    fn select_returns_none_when_too_few_compactable() {
        let items = make_conversation(4);
        let counts: Vec<u32> = items.iter().map(|_| 100u32).collect();

        // Set high target so we keep everything
        let plan = select_turns_to_compact(&items, &counts, u32::MAX, 10);
        assert!(plan.is_none());
    }

    #[test]
    fn snap_avoids_orphaned_tool_results() {
        let mut items = make_conversation(4);
        // Insert an assistant with tool calls followed by tool results
        let tc = ToolCall {
            id: "tc-1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        };
        let split_point = 4;
        items.insert(
            split_point,
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "checking".into(),
                tool_calls: vec![tc],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        );
        items.insert(
            split_point + 1,
            ConversationItem::tool_result("tc-1", "result"),
        );

        // Snap should move past the tool result
        let snapped = snap_to_safe_boundary(&items, split_point);
        assert!(snapped > split_point + 1);
    }

    #[test]
    fn chunk_plan_single_chunk_when_under_budget() {
        let items = make_conversation(3);
        let counts: Vec<u32> = items.iter().map(|_i| 100u32).collect();
        let chunks = plan_compaction_chunks(&items, &counts, 10_000);
        assert_eq!(chunks.len(), 1, "small history stays one chunk");
        assert_eq!(chunks[0], ChunkRange { start: 0, end: 6 });
    }

    #[test]
    fn chunk_plan_splits_over_budget() {
        let items = make_conversation(6); // 12 items
        let counts: Vec<u32> = items.iter().map(|_i| 100u32).collect();
        let chunks = plan_compaction_chunks(&items, &counts, 500);
        // 12 items × 100 tokens / 500 budget → ≥ 3 chunks, none exceeds budget.
        assert!(chunks.len() >= 3, "must split, got {}", chunks.len());
        for c in &chunks {
            let sum: u32 = counts[c.start..c.end].iter().sum();
            assert!(sum <= 500, "chunk {:?} exceeds budget", c);
        }
        // Contiguous + cover everything.
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks.last().unwrap().end, 12);
    }

    #[test]
    fn chunk_plan_oversized_item_gets_own_chunk() {
        // One giant item forces a single-item chunk.
        let items = make_conversation(2);
        let counts: Vec<u32> = vec![10_000, 10, 10, 10];
        let chunks = plan_compaction_chunks(&items, &counts, 500);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].end - chunks[0].start == 1, "giant item alone");
        for c in &chunks {
            let sum: u32 = counts[c.start..c.end].iter().sum();
            assert!(sum <= 10_000, "chunk {:?} exceeds its largest item", c);
        }
    }

    #[test]
    fn chunk_plan_empty_input() {
        assert!(plan_compaction_chunks(&[], &[], 500).is_empty());
    }

    #[test]
    fn chunk_plan_keeps_tool_call_and_result_together() {
        // A tool_call/result pair must never be split across chunks — the
        // summarizer would see a dangling call and an orphaned result.
        let mut items = make_conversation(3);
        let tc = ToolCall {
            id: "tc-1".into(),
            name: "grep".into(),
            arguments: "{}".into(),
        };
        items.insert(
            2,
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "searching".into(),
                tool_calls: vec![tc],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        );
        items.insert(3, ConversationItem::tool_result("tc-1", "big result"));
        // A tight budget forces a split near the tool pair.
        let counts: Vec<u32> = items.iter().map(|_| 100u32).collect();
        let chunks = plan_compaction_chunks(&items, &counts, 200);
        for c in &chunks {
            let has_assistant = (c.start..c.end).contains(&2);
            let has_result = (c.start..c.end).contains(&3);
            assert_eq!(
                has_assistant, has_result,
                "tool call and result must not split across chunks: {c:?}"
            );
        }
    }
}
