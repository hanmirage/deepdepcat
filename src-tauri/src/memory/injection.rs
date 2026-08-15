//! Memory injector — automatically injects relevant memories into
//! the agent's context.
//!
//! Before each API call, the injector searches for memories relevant
//! to the user's message and appends them to the system prompt.
//!
//! Injection is budget-aware: memories are injected in relevance order
//! (highest score first) up to a total character budget, and each entry
//! is truncated to a per-entry cap. This keeps long-term memory useful
//! without flooding the context window with low-signal noise.

use crate::core::error::AppResult;
use crate::memory::search::MemorySearcher;

/// Default total budget for injected memory content (characters).
const DEFAULT_MAX_TOTAL_CHARS: usize = 2_000;
/// Default per-entry cap (characters) — longer memories are truncated
/// with an ellipsis marker.
const DEFAULT_MAX_ENTRY_CHARS: usize = 600;
/// Display snippet cap (characters) — what the frontend shows in the
/// "memory referenced" marker.
const SNIPPET_CHARS: usize = 80;
/// Cap for the category label rendered next to each entry — an oversized
/// category would flood the context with noise.
const MAX_CATEGORY_CHARS: usize = 32;
/// Ellipsis marker appended when content is truncated.
const ELLIPSIS: &str = "…";

/// Summary of what was injected — surfaced to the UI so the user can see
/// that their memories were actually used ("已引用记忆 · …").
#[derive(Debug, Clone)]
pub struct InjectionSummary {
    /// Number of memory entries injected.
    pub count: usize,
    /// First injected entry, truncated — the visible reference.
    pub snippet: String,
}

/// The memory injector — auto-injects memories into context.
#[derive(Clone)]
pub struct MemoryInjector {
    searcher: MemorySearcher,
    enabled: bool,
    /// Total character budget for all injected memories.
    max_total_chars: usize,
    /// Per-entry character cap.
    max_entry_chars: usize,
}

impl MemoryInjector {
    pub fn new(searcher: MemorySearcher, enabled: bool) -> Self {
        Self {
            searcher,
            enabled,
            max_total_chars: DEFAULT_MAX_TOTAL_CHARS,
            max_entry_chars: DEFAULT_MAX_ENTRY_CHARS,
        }
    }

    /// Hot-update the searcher weights (config UI sliders).
    pub fn update_weights(&self, weights: crate::memory::search::SearchWeights) {
        self.searcher.update_weights(weights);
    }

    /// Access the underlying searcher (hot-update knobs).
    pub fn searcher(&self) -> &MemorySearcher {
        &self.searcher
    }

    /// Build a memory context block for the given user message.
    ///
    /// Memories are injected highest-score-first; entries that would
    /// exceed the total budget are dropped entirely, and entries longer
    /// than the per-entry cap are truncated. The returned summary (when
    /// Some) lets the UI show "memory referenced" feedback.
    pub async fn build_context(
        &self,
        user_message: &str,
    ) -> AppResult<(String, Option<InjectionSummary>)> {
        if !self.enabled {
            return Ok((String::new(), None));
        }

        let results = self.searcher.search(user_message).await?;

        if results.is_empty() {
            return Ok((String::new(), None));
        }

        let mut context = String::from("## Relevant Memories\n\n");
        let mut budget = self.max_total_chars;
        let mut count = 0usize;
        let mut snippet = String::new();

        for result in results {
            let content = result.memory.content.trim();
            if content.is_empty() {
                continue;
            }
            let entry = truncate_entry(content, self.max_entry_chars);
            if entry.len() > budget {
                continue;
            }
            budget -= entry.len();
            count += 1;
            if snippet.is_empty() {
                snippet = truncate_entry(&entry, SNIPPET_CHARS);
            }
            let category = truncate_entry(&result.memory.category, MAX_CATEGORY_CHARS);
            context.push_str(&format!("{count}. [{category}] {entry}\n"));
            if budget == 0 {
                break;
            }
        }

        if count == 0 {
            return Ok((String::new(), None));
        }

        Ok((context, Some(InjectionSummary { count, snippet })))
    }
}

/// Truncate text to `max_chars`, appending an ellipsis marker when content
/// was cut (the marker is reserved inside the budget).
fn truncate_entry(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let mut end = max_chars.saturating_sub(ELLIPSIS.len());
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = content[..end].to_string();
    out.push_str(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_entry_cuts_long_content() {
        let long = "x".repeat(1_000);
        let cut = truncate_entry(&long, 600);
        assert_eq!(cut.len(), 600);
        assert_eq!(cut, format!("{}…", "x".repeat(597)));
    }

    #[test]
    fn truncate_entry_keeps_short_content() {
        assert_eq!(truncate_entry("short", 600), "short");
    }

    #[test]
    fn truncate_entry_handles_char_boundaries() {
        let emoji = "😀".repeat(400); // 4 bytes per char, 1600 bytes
        let cut = truncate_entry(&emoji, 600);
        assert!(cut.is_char_boundary(cut.len()));
        assert_eq!(cut, format!("{}…", "😀".repeat(149)));
    }

    #[test]
    fn truncate_entry_never_empty_on_positive_budget() {
        let cut = truncate_entry("abcdef", 2);
        assert_eq!(cut, "…");
    }

    #[test]
    fn category_is_capped() {
        let long = "x".repeat(200);
        let cat = truncate_entry(&long, MAX_CATEGORY_CHARS);
        assert_eq!(cat.len(), MAX_CATEGORY_CHARS);
        assert!(cat.ends_with(ELLIPSIS));
    }
}
