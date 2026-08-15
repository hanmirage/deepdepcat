//! Global usage aggregate — cumulative, durable, cross-session token
//! accounting.
//!
//! Backed by the `usage_aggregate` singleton row (id = 1). Every LLM call
//! and tool result increments it, but NOT on every call: deltas accumulate
//! in memory and flush to the single-row UPSERT in batches (or on read), so
//! the global SQLite write hotspot of "one UPSERT per API call" is gone.
//! Nothing is lost on session switch or app restart — `get()` flushes
//! before reading, and the pending delta is also flushed on the next write
//! batch.

use crate::core::types::TokenUsage;
use crate::storage::database::Database;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Cumulative usage across all sessions, all time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_read_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub tool_calls: u64,
    pub tool_result_tokens: u64,
    pub turns: u64,
}

/// Flush the pending delta to SQLite once this many record operations have
/// accumulated (each flushed batch = one UPSERT instead of one per call).
const FLUSH_THRESHOLD_OPS: u64 = 32;

/// Increments the singleton aggregate row in the database, in memory-first
/// batches.
#[derive(Clone)]
pub struct GlobalUsageStore {
    db: std::sync::Arc<Database>,
    /// In-memory pending deltas (lock-free atomics — record paths are hot).
    pending_ops: Arc<AtomicU64>,
    pending_prompt: Arc<AtomicU64>,
    pending_completion: Arc<AtomicU64>,
    pending_cached: Arc<AtomicU64>,
    pending_reasoning: Arc<AtomicU64>,
    pending_hit: Arc<AtomicU64>,
    pending_miss: Arc<AtomicU64>,
    pending_tool_calls: Arc<AtomicU64>,
    pending_tool_result: Arc<AtomicU64>,
    pending_turns: Arc<AtomicU64>,
}

impl GlobalUsageStore {
    pub fn new(db: std::sync::Arc<Database>) -> Self {
        Self {
            db,
            pending_ops: Arc::new(AtomicU64::new(0)),
            pending_prompt: Arc::new(AtomicU64::new(0)),
            pending_completion: Arc::new(AtomicU64::new(0)),
            pending_cached: Arc::new(AtomicU64::new(0)),
            pending_reasoning: Arc::new(AtomicU64::new(0)),
            pending_hit: Arc::new(AtomicU64::new(0)),
            pending_miss: Arc::new(AtomicU64::new(0)),
            pending_tool_calls: Arc::new(AtomicU64::new(0)),
            pending_tool_result: Arc::new(AtomicU64::new(0)),
            pending_turns: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Add an LLM usage delta (one model call = one turn). Cheap: bumps
    /// atomics; the SQLite write happens at the flush threshold.
    pub fn add_llm(&self, usage: &TokenUsage) {
        self.pending_prompt
            .fetch_add(usage.prompt_tokens, Ordering::Relaxed);
        self.pending_completion
            .fetch_add(usage.completion_tokens, Ordering::Relaxed);
        self.pending_cached
            .fetch_add(usage.cached_read_tokens.unwrap_or(0), Ordering::Relaxed);
        self.pending_reasoning
            .fetch_add(usage.reasoning_tokens.unwrap_or(0), Ordering::Relaxed);
        self.pending_hit.fetch_add(
            usage.prompt_cache_hit_tokens.unwrap_or(0),
            Ordering::Relaxed,
        );
        self.pending_miss.fetch_add(
            usage.prompt_cache_miss_tokens.unwrap_or(0),
            Ordering::Relaxed,
        );
        self.pending_turns.fetch_add(1, Ordering::Relaxed);
        self.maybe_flush();
    }

    /// Add a tool execution (result estimated tokens). Same memory-first
    /// batching as [`Self::add_llm`].
    pub fn add_tool(&self, result_tokens: u64) {
        self.pending_tool_calls.fetch_add(1, Ordering::Relaxed);
        self.pending_tool_result
            .fetch_add(result_tokens, Ordering::Relaxed);
        self.maybe_flush();
    }

    /// Flush when the pending operation count crosses the threshold. The
    /// counter is never reset to zero here — the flush itself zeroes the
    /// pending counters under the DB lock, and the ops counter is reset by
    /// the swap inside `flush_pending` (see below).
    fn maybe_flush(&self) {
        if self.pending_ops.fetch_add(1, Ordering::Relaxed) + 1 >= FLUSH_THRESHOLD_OPS {
            self.flush_pending();
        }
    }

    /// Swap the pending counters and UPSERT them into the singleton row in
    /// ONE write. Called at the threshold and before every read.
    pub fn flush_pending(&self) {
        let (prompt, completion, cached, reasoning, hit, miss, tool_calls, tool_result, turns, ops) = (
            self.pending_prompt.swap(0, Ordering::Relaxed),
            self.pending_completion.swap(0, Ordering::Relaxed),
            self.pending_cached.swap(0, Ordering::Relaxed),
            self.pending_reasoning.swap(0, Ordering::Relaxed),
            self.pending_hit.swap(0, Ordering::Relaxed),
            self.pending_miss.swap(0, Ordering::Relaxed),
            self.pending_tool_calls.swap(0, Ordering::Relaxed),
            self.pending_tool_result.swap(0, Ordering::Relaxed),
            self.pending_turns.swap(0, Ordering::Relaxed),
            self.pending_ops.swap(0, Ordering::Relaxed),
        );
        if prompt
            + completion
            + cached
            + reasoning
            + hit
            + miss
            + tool_calls
            + tool_result
            + turns
            + ops
            == 0
        {
            return;
        }
        let result = self.db.conn().and_then(|conn| {
            conn.execute(
                "INSERT INTO usage_aggregate (id, prompt_tokens, completion_tokens,
                     cached_read_tokens, reasoning_tokens, cache_hit_tokens,
                     cache_miss_tokens, tool_calls, tool_result_tokens, turns, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                     prompt_tokens = prompt_tokens + excluded.prompt_tokens,
                     completion_tokens = completion_tokens + excluded.completion_tokens,
                     cached_read_tokens = cached_read_tokens + excluded.cached_read_tokens,
                     reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens,
                     cache_hit_tokens = cache_hit_tokens + excluded.cache_hit_tokens,
                     cache_miss_tokens = cache_miss_tokens + excluded.cache_miss_tokens,
                     tool_calls = tool_calls + excluded.tool_calls,
                     tool_result_tokens = tool_result_tokens + excluded.tool_result_tokens,
                     turns = turns + excluded.turns,
                     updated_at = excluded.updated_at",
                rusqlite::params![
                    prompt as i64,
                    completion as i64,
                    cached as i64,
                    reasoning as i64,
                    hit as i64,
                    miss as i64,
                    tool_calls as i64,
                    tool_result as i64,
                    turns as i64,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map(|_| ())
            .map_err(Into::into)
        });
        if let Err(e) = result {
            // Restore the swapped deltas so the batch is retried on the next
            // flush instead of being silently dropped (a crash or DB error
            // must not lose usage accounting).
            tracing::warn!(error = %e, "Failed to flush usage aggregate — deltas retained for retry");
            self.pending_prompt.fetch_add(prompt, Ordering::Relaxed);
            self.pending_completion
                .fetch_add(completion, Ordering::Relaxed);
            self.pending_cached.fetch_add(cached, Ordering::Relaxed);
            self.pending_reasoning
                .fetch_add(reasoning, Ordering::Relaxed);
            self.pending_hit.fetch_add(hit, Ordering::Relaxed);
            self.pending_miss.fetch_add(miss, Ordering::Relaxed);
            self.pending_tool_calls
                .fetch_add(tool_calls, Ordering::Relaxed);
            self.pending_tool_result
                .fetch_add(tool_result, Ordering::Relaxed);
            self.pending_turns.fetch_add(turns, Ordering::Relaxed);
            self.pending_ops.fetch_add(ops, Ordering::Relaxed);
        }
    }

    /// Read the current aggregate (zeroed if the row does not exist yet).
    /// Flushes pending deltas first so the durable number is never behind
    /// the in-memory one.
    pub fn get(&self) -> GlobalUsage {
        self.flush_pending();
        self.db
            .conn()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT prompt_tokens, completion_tokens, cached_read_tokens,
                            reasoning_tokens, cache_hit_tokens, cache_miss_tokens,
                            tool_calls, tool_result_tokens, turns
                     FROM usage_aggregate WHERE id = 1",
                    [],
                    |row| {
                        Ok(GlobalUsage {
                            prompt_tokens: row.get::<_, i64>(0)? as u64,
                            completion_tokens: row.get::<_, i64>(1)? as u64,
                            cached_read_tokens: row.get::<_, i64>(2)? as u64,
                            reasoning_tokens: row.get::<_, i64>(3)? as u64,
                            cache_hit_tokens: row.get::<_, i64>(4)? as u64,
                            cache_miss_tokens: row.get::<_, i64>(5)? as u64,
                            tool_calls: row.get::<_, i64>(6)? as u64,
                            tool_result_tokens: row.get::<_, i64>(7)? as u64,
                            turns: row.get::<_, i64>(8)? as u64,
                        })
                    },
                )
                .ok()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> std::sync::Arc<Database> {
        let dir = std::env::temp_dir().join(format!(
            "ddc-usage-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("test.db"), true).unwrap();
        db.run_migrations().unwrap();
        std::sync::Arc::new(db)
    }

    #[test]
    fn aggregate_accumulates_across_calls() {
        let store = GlobalUsageStore::new(test_db());

        assert_eq!(store.get().prompt_tokens, 0);

        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            cached_read_tokens: Some(20),
            prompt_cache_hit_tokens: Some(10),
            prompt_cache_miss_tokens: Some(90),
            ..Default::default()
        };
        store.add_llm(&usage);
        store.add_llm(&usage);
        store.add_tool(400);
        store.add_tool(600);

        let g = store.get();
        assert_eq!(g.prompt_tokens, 200);
        assert_eq!(g.completion_tokens, 100);
        assert_eq!(g.cached_read_tokens, 40);
        assert_eq!(g.cache_hit_tokens, 20);
        assert_eq!(g.cache_miss_tokens, 180);
        assert_eq!(g.turns, 2);
        assert_eq!(g.tool_calls, 2);
        assert_eq!(g.tool_result_tokens, 1000);
        assert_eq!(
            g.prompt_tokens + g.completion_tokens + g.tool_result_tokens,
            1300
        );
    }

    #[test]
    fn reads_flush_pending_before_returning() {
        let store = GlobalUsageStore::new(test_db());

        let usage = TokenUsage {
            prompt_tokens: 7,
            ..Default::default()
        };
        store.add_llm(&usage);
        // Below the threshold — nothing written yet.
        assert_eq!(
            store
                .db
                .conn()
                .unwrap()
                .query_row(
                    "SELECT prompt_tokens FROM usage_aggregate WHERE id = 1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok(),
            None
        );
        // A read flushes, so the durable row now exists.
        assert_eq!(store.get().prompt_tokens, 7);
    }

    #[test]
    fn threshold_flushes_in_batches() {
        let store = GlobalUsageStore::new(test_db());

        let usage = TokenUsage {
            prompt_tokens: 1,
            ..Default::default()
        };
        for _ in 0..FLUSH_THRESHOLD_OPS {
            store.add_llm(&usage);
        }
        // Threshold crossed → flushed without a read.
        assert_eq!(store.get().prompt_tokens, FLUSH_THRESHOLD_OPS);
    }
}
