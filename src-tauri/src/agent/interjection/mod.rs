//! Interjection Core — structured system prompt injection framework.
//!
//! Multiple sources (memory, skills, permissions, todos, subagent status,
//! system reminders) inject fragments into the system prompt. Without
//! coordination, these fragments can duplicate, conflict, or bloat the
//! prompt. This module provides a registry that:
//!
//! 1. Collects interjections from all sources
//! 2. Sorts by priority (higher = injected first)
//! 3. Deduplicates by `dedup_key` (first occurrence wins)
//! 4. Merges into a single coherent prompt fragment
//!
//! This replaces the scattered injection logic that was previously inlined
//! in `ContextBuilder` and `system_reminder.rs`.

use std::collections::HashSet;

/// Priority of an interjection source. Higher values are injected first
/// (closer to the top of the system prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InterjectionPriority {
    /// Normal — standard context (e.g., memory injection, skill activation).
    Normal = 1,
    /// High — actionable guidance (e.g., todo nudges, stale session).
    High = 2,
}

/// A single interjection — a fragment of text to inject into the system prompt.
#[derive(Debug, Clone)]
pub struct Interjection {
    /// Priority — higher values are injected first.
    pub priority: InterjectionPriority,
    /// The text content to inject.
    pub content: String,
    /// Deduplication key — if two interjections have the same key, only the
    /// first (by priority order) is kept. Empty string means no dedup.
    pub dedup_key: String,
}

impl Interjection {
    /// Create a new interjection with the given priority and content.
    pub fn new(
        _source: impl Into<String>,
        priority: InterjectionPriority,
        content: impl Into<String>,
    ) -> Self {
        Self {
            priority,
            content: content.into(),
            dedup_key: String::new(),
        }
    }

    /// Set the deduplication key.
    pub fn with_dedup_key(mut self, key: impl Into<String>) -> Self {
        self.dedup_key = key.into();
        self
    }
}

/// Registry that collects, sorts, deduplicates, and merges interjections.
#[derive(Debug, Default)]
pub struct InterjectionRegistry {
    interjections: Vec<Interjection>,
    /// Maximum registered interjections before the oldest are dropped.
    /// Mirrors the upstream `push_capped` semantics: bounded memory, and
    /// the most recent high-priority guidance always survives.
    max_interjections: usize,
}

impl InterjectionRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    /// Create a registry with a bounded interjection capacity.
    pub fn with_capacity(max_interjections: usize) -> Self {
        Self {
            interjections: Vec::new(),
            max_interjections: max_interjections.max(1),
        }
    }

    /// Register an interjection.
    ///
    /// When the registry is at capacity, the oldest interjections are
    /// dropped so the newest guidance stays visible.
    pub fn register(&mut self, interjection: Interjection) {
        self.interjections.push(interjection);
        if self.interjections.len() > self.max_interjections {
            let excess = self.interjections.len() - self.max_interjections;
            self.interjections.drain(..excess);
        }
    }

    /// Collect all registered interjections as `(dedup_key, content)` pairs,
    /// sorted by priority (descending) and deduplicated by `dedup_key`.
    ///
    /// The key is surfaced so consumers can emit a replay event for WHICH
    /// nudge fired, without carrying the full (large) guidance text.
    ///
    /// **Consumes the registry** — collected interjections are cleared so
    /// one-shot guidance (todo gates, background subagent signals) is
    /// injected exactly once instead of re-appearing every request build.
    pub fn collect_fragments(&mut self) -> Vec<(String, String)> {
        // Sort by priority descending (highest priority first).
        let mut sorted = std::mem::take(&mut self.interjections);
        sorted.sort_by_key(|i| std::cmp::Reverse(i.priority));

        // Deduplicate by dedup_key (first occurrence wins after sorting).
        let mut seen_keys: HashSet<String> = HashSet::new();
        let mut fragments: Vec<(String, String)> = Vec::new();

        for interjection in sorted {
            if !interjection.dedup_key.is_empty()
                && !seen_keys.insert(interjection.dedup_key.clone())
            {
                // Duplicate key — skip this interjection.
                continue;
            }
            if !interjection.content.is_empty() {
                fragments.push((interjection.dedup_key, interjection.content));
            }
        }

        fragments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_never_zero() {
        let registry = InterjectionRegistry::with_capacity(0);
        assert!(registry.max_interjections >= 1);
    }
}
