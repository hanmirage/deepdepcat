//! Compaction item helpers.
//!
//! The `CompactionItem` trait seam and `CompactionRole` enum were removed
//! as unwired dead code — the live `Compactor` (compaction/mod.rs) operates
//! directly on `ConversationItem`. Only the per-item token counter that
//! `select_by_token_budget` depends on remains.

use crate::core::types::ConversationItem;

/// Compute per-item token counts for a conversation slice.
pub fn item_token_counts(items: &[ConversationItem]) -> Vec<u32> {
    items
        .iter()
        .map(|i| crate::agent::token::estimate_item_tokens(i) as u32)
        .collect()
}
