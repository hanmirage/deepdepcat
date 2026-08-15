//! Prompt queue — backpressure-aware queue for user prompts.
//!
//! When the agent is busy processing a turn, incoming prompts are queued
//! and replayed in order after the current turn completes. The queue has
//! a bounded capacity to provide backpressure — if the queue is full,
//! new prompts are rejected with an error instead of growing unbounded.
//!
//! Production only uses `push`/`pop` (commands/chat.rs replays the queue);
//! the in-place `edit`/`cancel`, the running-state machine and the
//! `wait_for_prompt` notifier were removed as unwired dead code.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Maximum number of queued prompts before rejecting new ones.
const DEFAULT_MAX_QUEUE_SIZE: usize = 16;

/// Per-item queue metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEntry {
    /// Stable identifier (reuses the prompt's unique ID).
    pub id: String,
    /// The prompt text.
    pub text: String,
    /// Position in the queue (0-based).
    pub position: usize,
}

/// Backpressure-aware prompt queue.
///
/// Producers push prompts via [`push`]. Consumers drain via [`pop`].
/// When the queue is full, [`push`] returns an error, providing
/// backpressure to the caller.
pub struct PromptQueue {
    /// Internal deque holding queued prompts.
    entries: VecDeque<PromptEntry>,
    /// Maximum queue capacity.
    max_size: usize,
}

impl PromptQueue {
    /// Create a new queue with the default max size.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_QUEUE_SIZE)
    }

    /// Create a new queue with a custom max size.
    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_size.min(64)),
            max_size,
        }
    }

    /// Push a new prompt onto the queue.
    ///
    /// Returns `Err` if the queue is full.
    pub fn push(
        &mut self,
        id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), QueueError> {
        if self.entries.len() >= self.max_size {
            return Err(QueueError::Full {
                capacity: self.max_size,
            });
        }

        let id = id.into();
        let position = self.entries.len();
        self.entries.push_back(PromptEntry {
            id,
            text: text.into(),
            position,
        });
        Ok(())
    }

    /// Pop the next prompt from the queue, re-indexing the remainder.
    ///
    /// If the queue is empty, returns `None`.
    pub fn pop(&mut self) -> Option<PromptEntry> {
        let mut entry = self.entries.pop_front()?;
        entry.position = 0;
        for (i, e) in self.entries.iter_mut().enumerate() {
            e.position = i;
        }
        Some(entry)
    }
}

impl Default for PromptQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from the prompt queue.
#[derive(Debug, Clone, thiserror::Error)]
pub enum QueueError {
    /// Queue is at capacity.
    #[error("prompt queue is full (capacity: {capacity})")]
    Full { capacity: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop() {
        let mut q = PromptQueue::new();
        q.push("p1", "first").unwrap();
        q.push("p2", "second").unwrap();

        let p = q.pop().unwrap();
        assert_eq!(p.id, "p1");
        assert_eq!(p.text, "first");

        let p = q.pop().unwrap();
        assert_eq!(p.id, "p2");
        assert_eq!(p.text, "second");
    }

    #[test]
    fn queue_full_rejects() {
        let mut q = PromptQueue::with_capacity(2);
        q.push("p1", "a").unwrap();
        q.push("p2", "b").unwrap();
        assert!(q.push("p3", "c").is_err());
    }

    #[test]
    fn pop_reindexes_positions() {
        let mut q = PromptQueue::new();
        q.push("p1", "first").unwrap();
        q.push("p2", "second").unwrap();
        q.pop().unwrap();

        // After the first pop, p2 moves to position 0.
        let p = q.pop().unwrap();
        assert_eq!(p.id, "p2");
        assert_eq!(p.position, 0);
    }
}
