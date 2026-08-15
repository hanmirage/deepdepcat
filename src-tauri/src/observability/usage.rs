//! Session-level token usage tracking.
//!
//! Tracks token usage across:
//! - LLM API calls (prompt + completion + cached + reasoning)
//! - Tool executions (estimated tokens for tool results)
//! - Per-turn deltas (emitted as `StreamEvent::Usage`)
//!
//! The tracker is designed to be lightweight and lock-free for the common
//! read path (totals), with a mutex only for the write path.

use crate::core::types::TokenUsage;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Rolling window of cache accounting — the hit/miss tokens of the most
/// recent turns drive the live cache-hit ratio shown in the usage ring.
/// Windowed on purpose: a whole-session average is dominated by the first
/// request (which always misses) and hides a live prefix-stability problem.
const CACHE_RATIO_WINDOW: usize = 10;

/// Token usage delta for a single turn — the difference between
/// the start and end of one agent loop iteration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnDelta {
    /// Turn number (1-indexed).
    pub turn: u32,
    /// LLM usage for this turn.
    pub llm_usage: TokenUsage,
    /// Number of tool calls made.
    pub tool_calls: u32,
    /// Estimated tokens consumed by tool results.
    pub tool_result_tokens: u64,
    /// Wall-clock duration of the turn in milliseconds.
    pub duration_ms: u64,
}

/// Session-level usage tracker — accumulates token usage across the entire
/// session lifetime, broken down by turn and by tool.
///
/// Cloning creates a new handle to the same underlying tracker.
///
/// When a [`GlobalUsageStore`] is attached, every recorded delta is ALSO
/// persisted into the global aggregate (single-row SQLite) so usage is
/// durable across sessions and app restarts.
#[derive(Clone)]
pub struct SessionUsageTracker {
    inner: Arc<Mutex<Inner>>,
    /// Durable global aggregate — increments on every record when set.
    global: Option<crate::storage::database::GlobalUsageStore>,
}

struct Inner {
    /// Session ID for logging.
    session_id: String,
    /// Cumulative usage across all LLM calls.
    total_llm_usage: TokenUsage,
    /// Per-turn deltas.
    turns: Vec<TurnDelta>,
    /// Per-tool call count.
    tool_call_counts: HashMap<String, u32>,
    /// Per-tool estimated token usage.
    tool_token_usage: HashMap<String, u64>,
    /// Total number of tool calls.
    total_tool_calls: u32,
    /// Total estimated tool result tokens.
    total_tool_result_tokens: u64,
    /// Estimated context breakdown of the most recent request (overwritten
    /// every turn — reflects what the model sees RIGHT NOW).
    context_breakdown: ContextBreakdown,
    /// Input size (prompt tokens) of the MOST RECENT single LLM request.
    /// A turn can contain several requests (retries, recoveries); summing
    /// them (as the per-turn slot does) would overstate the current context
    /// occupancy by the full prompt of every extra call.
    last_request_prompt_tokens: u64,
    /// Per-turn `(cache_hit_tokens, cache_miss_tokens)` from providers that
    /// report them (DeepSeek), oldest first, capped at [`CACHE_RATIO_WINDOW`].
    recent_cache_accounting: VecDeque<(u64, u64)>,
}

impl SessionUsageTracker {
    /// Create a new tracker for the given session.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                session_id: session_id.into(),
                total_llm_usage: TokenUsage::default(),
                turns: Vec::new(),
                tool_call_counts: HashMap::new(),
                tool_token_usage: HashMap::new(),
                total_tool_calls: 0,
                total_tool_result_tokens: 0,
                context_breakdown: ContextBreakdown::default(),
                last_request_prompt_tokens: 0,
                recent_cache_accounting: VecDeque::new(),
            })),
            global: None,
        }
    }

    /// Attach a durable global aggregate — all subsequent records also
    /// increment it (persisted to SQLite).
    pub fn with_global(mut self, global: crate::storage::database::GlobalUsageStore) -> Self {
        self.global = Some(global);
        self
    }

    /// Flush any pending global-aggregate deltas to SQLite.
    ///
    /// The global store batches writes (FLUSH_THRESHOLD_OPS) so a session's
    /// trailing <32 operations live only in memory until the next flush.
    /// Called before the cumulative usage is read and when a session tracker
    /// is dropped — otherwise that last batch is invisible to `get_global_usage`
    /// and permanently lost on app exit.
    pub fn flush_global(&self) {
        if let Some(ref g) = self.global {
            g.flush_pending();
        }
    }

    /// Record LLM usage for a specific turn.
    ///
    /// `turn == 0` is the "no turn" channel used by non-streaming calls
    /// (force_final_answer, reflexion critiques, compaction summaries):
    /// usage counts toward the session total and global aggregate but does
    /// NOT create or pollute a per-turn slot — the old behavior mapped
    /// turn 0 onto turn 1's slot, double-counting it (#88 audit M35).
    pub fn record_llm_usage(&self, turn: u32, usage: &TokenUsage) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let session_id = inner.session_id.clone();
        inner.total_llm_usage.add(usage);
        inner.last_request_prompt_tokens = usage.prompt_tokens;

        // Cache accounting (DeepSeek reports hit/miss on every request) —
        // keep a rolling window of the recent turns so the live hit ratio
        // reflects current prefix stability, not the first-request miss.
        if let (Some(hit), Some(miss)) = (
            usage.prompt_cache_hit_tokens,
            usage.prompt_cache_miss_tokens,
        ) {
            inner.recent_cache_accounting.push_back((hit, miss));
            while inner.recent_cache_accounting.len() > CACHE_RATIO_WINDOW {
                inner.recent_cache_accounting.pop_front();
            }
        }

        // Turn 0 = outside the turn sequence (non-streaming internals):
        // totals only, no per-turn slot.
        if turn > 0 {
            // Find or create the turn delta
            let turn_delta = Self::get_or_create_turn(&mut inner.turns, turn);
            turn_delta.llm_usage.add(usage);
        }

        if let Some(ref g) = self.global {
            g.add_llm(usage);
        }

        debug!(
            session_id = %session_id,
            turn,
            prompt = usage.prompt_tokens,
            completion = usage.completion_tokens,
            "Recorded LLM usage"
        );
    }

    /// Record a tool execution.
    pub fn record_tool_call(&self, turn: u32, tool_name: &str, result_tokens: u64) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let session_id = inner.session_id.clone();
        inner.total_tool_calls += 1;
        inner.total_tool_result_tokens += result_tokens;

        *inner
            .tool_call_counts
            .entry(tool_name.to_string())
            .or_insert(0) += 1;
        *inner
            .tool_token_usage
            .entry(tool_name.to_string())
            .or_insert(0) += result_tokens;

        if let Some(ref g) = self.global {
            g.add_tool(result_tokens);
        }

        let turn_delta = Self::get_or_create_turn(&mut inner.turns, turn);
        turn_delta.tool_calls += 1;
        turn_delta.tool_result_tokens += result_tokens;

        debug!(
            session_id = %session_id,
            turn,
            tool = tool_name,
            result_tokens,
            "Recorded tool call"
        );
    }

    /// Record the estimated context breakdown of the current request — what
    /// the model sees right now, split by category (system prompt, skills,
    /// tool definitions, conversation, tool results). Overwrites the previous
    /// turn's breakdown.
    pub fn record_context_breakdown(&self, breakdown: ContextBreakdown) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .context_breakdown = breakdown;
    }

    /// Get a summary of the session usage.
    pub fn summary(&self) -> SessionUsageSummary {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // The CURRENT context occupancy = the MOST RECENT single request's
        // input size (every request re-sends the whole history, so this is
        // the real "how full is my context right now" number). The per-turn
        // slot ACCUMULATES every request in the turn (recovery retries,
        // mid-turn internal calls) and would overstate the occupancy.
        // Cumulative totals are NOT comparable to the model window —
        // Claude-style indicators show this value, not the session's
        // accumulated token sum.
        let current_context_tokens = inner.last_request_prompt_tokens;
        let (recent_hit, recent_miss) = inner
            .recent_cache_accounting
            .iter()
            .fold((0u64, 0u64), |(h, m), (hit, miss)| (h + hit, m + miss));
        // Per-request cache history for the usage page's prefix-stability view.
        // A request "invalidated" the prefix when it MISSED heavily right after
        // a request that HIT heavily — the first request of a session always
        // misses (no prefix exists yet) and is not flagged.
        let cache_history: Vec<CacheRequest> = inner
            .recent_cache_accounting
            .iter()
            .enumerate()
            .map(|(i, (hit, miss))| {
                let hit = *hit;
                let miss = *miss;
                let total = hit + miss;
                let ratio = if total > 0 {
                    hit as f64 / total as f64
                } else {
                    1.0
                };
                let prev_ok = i > 0
                    && {
                        let (ph, pm) = inner.recent_cache_accounting[i - 1];
                        let pt = ph + pm;
                        pt > 0 && ph as f64 / pt as f64 > 0.5
                    };
                CacheRequest {
                    hit_tokens: hit,
                    miss_tokens: miss,
                    invalidated: prev_ok && ratio < 0.2,
                }
            })
            .collect();
        SessionUsageSummary {
            session_id: inner.session_id.clone(),
            total_prompt_tokens: inner.total_llm_usage.prompt_tokens,
            total_completion_tokens: inner.total_llm_usage.completion_tokens,
            total_cached_read_tokens: inner.total_llm_usage.cached_read_tokens.unwrap_or(0),
            total_reasoning_tokens: inner.total_llm_usage.reasoning_tokens.unwrap_or(0),
            total_tool_calls: inner.total_tool_calls,
            total_tool_result_tokens: inner.total_tool_result_tokens,
            turn_count: inner.turns.len() as u32,
            context_window: 0,
            current_context_tokens,
            context_breakdown: inner.context_breakdown.clone(),
            total_cache_hit_tokens: inner.total_llm_usage.prompt_cache_hit_tokens.unwrap_or(0),
            total_cache_miss_tokens: inner.total_llm_usage.prompt_cache_miss_tokens.unwrap_or(0),
            cache_hit_ratio: if recent_hit + recent_miss > 0 {
                Some(recent_hit as f64 / (recent_hit + recent_miss) as f64)
            } else {
                None
            },
            cache_history,
        }
    }

    /// Get or create a turn delta for the given turn number.
    fn get_or_create_turn(turns: &mut Vec<TurnDelta>, turn: u32) -> &mut TurnDelta {
        // Ensure the vector is large enough — each new slot records its
        // 1-indexed turn number so per-turn stats are not all zeros.
        while turns.len() < turn as usize {
            let idx = turns.len() as u32 + 1;
            turns.push(TurnDelta {
                turn: idx,
                ..Default::default()
            });
        }
        // Turn is 1-indexed, vector is 0-indexed
        &mut turns[(turn as usize).saturating_sub(1)]
    }
}

/// A summary of session-level usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUsageSummary {
    pub session_id: String,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_cached_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub total_tool_calls: u32,
    pub total_tool_result_tokens: u64,
    pub turn_count: u32,
    /// The session's current model context window in tokens — set by the
    /// command layer from the live `ChatState` (updates on model switch).
    /// `0` means unknown (model not in catalog) — UI falls back to a
    /// default budget.
    pub context_window: u64,
    /// Current context occupancy — the most recent request's input size
    /// (tokens actually in context right now). This is what a Claude-style
    /// usage indicator displays; cumulative totals are a different metric.
    pub current_context_tokens: u64,
    /// Estimated split of the current context by category (system prompt /
    /// skills / tool definitions / conversation / tool results). Approximate
    /// bytes-per-token estimates, not API-billed numbers.
    pub context_breakdown: ContextBreakdown,
    /// Cumulative prefix-cache hit tokens (DeepSeek). `0` when the provider
    /// never reports cache accounting.
    pub total_cache_hit_tokens: u64,
    /// Cumulative prefix-cache miss tokens (DeepSeek).
    pub total_cache_miss_tokens: u64,
    /// Live prefix-cache hit ratio — hit/(hit+miss) over the most recent
    /// [`CACHE_RATIO_WINDOW`] turns. `None` when no cache accounting has
    /// been reported yet. The ring shows this so a session with an unstable
    /// prefix (misses on every request) surfaces immediately instead of
    /// hiding behind a whole-session average.
    pub cache_hit_ratio: Option<f64>,
    /// Per-request cache accounting over the recent window (oldest first) —
    /// the usage page renders this as the prefix-stability strip, flagging
    /// requests whose heavy miss right after a hit means the prefix broke.
    pub cache_history: Vec<CacheRequest>,
}

/// One request's prefix-cache accounting for the usage-page history strip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRequest {
    pub hit_tokens: u64,
    pub miss_tokens: u64,
    /// True when this request MISSED heavily right after a request that HIT
    /// heavily — the prompt/context changed and the DeepSeek prefix was
    /// invalidated from that point. The first request of a session always
    /// misses (no prefix exists) and is never flagged.
    pub invalidated: bool,
}

/// Estimated token split of the current request context, by category.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub system_prompt_tokens: u64,
    pub skill_tokens: u64,
    pub tool_definition_tokens: u64,
    pub conversation_tokens: u64,
    pub tool_result_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary() {
        let tracker = SessionUsageTracker::new("test-session");
        tracker.record_llm_usage(
            1,
            &TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 500,
                cached_read_tokens: Some(100),
                reasoning_tokens: Some(50),
                prompt_cache_hit_tokens: Some(800),
                prompt_cache_miss_tokens: Some(200),
            },
        );
        tracker.record_tool_call(1, "bash", 2000);

        let summary = tracker.summary();
        assert_eq!(summary.session_id, "test-session");
        assert_eq!(summary.total_prompt_tokens, 1000);
        assert_eq!(summary.total_completion_tokens, 500);
        assert_eq!(summary.total_cached_read_tokens, 100);
        assert_eq!(summary.total_reasoning_tokens, 50);
        assert_eq!(summary.total_tool_calls, 1);
        assert_eq!(summary.total_tool_result_tokens, 2000);
        assert_eq!(summary.turn_count, 1);
        assert_eq!(summary.total_cache_hit_tokens, 800);
        assert_eq!(summary.total_cache_miss_tokens, 200);
        let ratio = summary
            .cache_hit_ratio
            .expect("cache ratio must be present");
        assert!((ratio - 0.8).abs() < 1e-9, "hit/(hit+miss) must be 0.8");
    }

    #[test]
    fn cache_ratio_is_none_without_cache_accounting() {
        // Providers that never report hit/miss must surface "unknown",
        // not a fake 0% or 100% — the ring hides the row then.
        let tracker = SessionUsageTracker::new("test-session");
        tracker.record_llm_usage(
            1,
            &TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                cached_read_tokens: None,
                reasoning_tokens: None,
                prompt_cache_hit_tokens: None,
                prompt_cache_miss_tokens: None,
            },
        );
        assert_eq!(tracker.summary().cache_hit_ratio, None);
        assert_eq!(tracker.summary().total_cache_hit_tokens, 0);
    }

    #[test]
    fn cache_ratio_rolls_over_the_recent_window() {
        // The live ratio reflects the LAST CACHE_RATIO_WINDOW turns, not
        // the whole session: 12 turns of 100% misses (the first request in
        // a fresh prefix always misses) followed by 3 turns of 100% hits
        // must show ~30% — an entire-session average would hide the
        // improvement behind the earlier misses.
        let tracker = SessionUsageTracker::new("test-session");
        for turn in 1..=CACHE_RATIO_WINDOW + 3 {
            // First CACHE_RATIO_WINDOW turns miss (a fresh prefix always
            // misses), then 3 turns hit — the window must show 3/10.
            let hit = if turn <= CACHE_RATIO_WINDOW { 0 } else { 1000 };
            let miss = if turn <= CACHE_RATIO_WINDOW { 1000 } else { 0 };
            tracker.record_llm_usage(
                turn as u32,
                &TokenUsage {
                    prompt_tokens: 1000,
                    completion_tokens: 10,
                    cached_read_tokens: None,
                    reasoning_tokens: None,
                    prompt_cache_hit_tokens: Some(hit),
                    prompt_cache_miss_tokens: Some(miss),
                },
            );
        }
        let summary = tracker.summary();
        let ratio = summary.cache_hit_ratio.expect("ratio must be present");
        let expected = 3.0 / CACHE_RATIO_WINDOW as f64;
        assert!(
            (ratio - expected).abs() < 1e-9,
            "rolling ratio must be {expected}, got {ratio}"
        );
        // Cumulative totals still count everything.
        assert_eq!(summary.total_cache_hit_tokens, 3000);
        assert_eq!(summary.total_cache_miss_tokens, 10_000);
    }

    #[test]
    fn cache_history_flags_prefix_invalidation() {
        let tracker = SessionUsageTracker::new("test-session");
        let usage = |hit: u64, miss: u64| TokenUsage {
            prompt_tokens: hit + miss,
            completion_tokens: 10,
            cached_read_tokens: None,
            reasoning_tokens: None,
            prompt_cache_hit_tokens: Some(hit),
            prompt_cache_miss_tokens: Some(miss),
        };
        // Turn 1: fresh prefix — always misses, never flagged.
        tracker.record_llm_usage(1, &usage(0, 1000));
        // Turn 2: hits heavily — the prefix is stable and working.
        tracker.record_llm_usage(2, &usage(1000, 0));
        // Turn 3: misses heavily right after a hit — the prefix BROKE (a
        // prompt/context change invalidated it from that point).
        tracker.record_llm_usage(3, &usage(0, 1000));
        let summary = tracker.summary();
        assert_eq!(summary.cache_history.len(), 3);
        assert!(
            !summary.cache_history[0].invalidated,
            "the first request of a session always misses — not a break"
        );
        assert!(
            !summary.cache_history[1].invalidated,
            "a hit is not a prefix break"
        );
        assert!(
            summary.cache_history[2].invalidated,
            "a heavy miss right after a hit = the prefix was invalidated"
        );
    }

    #[test]
    fn current_context_reflects_last_request_not_turn_sum() {
        // A turn with multiple LLM calls (retries, recoveries): the context
        // occupancy must show the LAST request's input size, not the sum of
        // every full-context request in the turn (which overstates it).
        let tracker = SessionUsageTracker::new("test-session");
        tracker.record_llm_usage(
            1,
            &TokenUsage {
                prompt_tokens: 30_000,
                completion_tokens: 100,
                ..Default::default()
            },
        );
        tracker.record_llm_usage(
            1,
            &TokenUsage {
                prompt_tokens: 31_000,
                completion_tokens: 50,
                ..Default::default()
            },
        );
        let summary = tracker.summary();
        assert_eq!(summary.current_context_tokens, 31_000);
        assert_eq!(
            summary.total_prompt_tokens, 61_000,
            "totals still accumulate"
        );
        // No requests recorded → unknown (0), not a stale turn sum.
        let empty = SessionUsageTracker::new("fresh").summary();
        assert_eq!(empty.current_context_tokens, 0);
    }

    #[test]
    fn flush_global_persists_trailing_below_threshold_deltas() {
        // A session's last <32 operations live only in the global store's
        // in-memory atomics until a flush. `flush_global` must persist them,
        // so the cumulative settings page never under-reports a session tail.
        let dir = std::env::temp_dir().join(format!(
            "ddc-usage-flush-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = std::sync::Arc::new(
            crate::storage::database::Database::open(&dir.join("test.db"), false).unwrap(),
        );
        db.run_migrations().unwrap();
        let global = crate::storage::database::GlobalUsageStore::new(db.clone());
        let tracker = SessionUsageTracker::new("s1").with_global(global);

        // A handful of operations — well below the 32-op flush threshold.
        for _ in 0..3 {
            tracker.record_llm_usage(
                1,
                &TokenUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    ..Default::default()
                },
            );
        }
        // Nothing in the DB yet.
        let before = crate::storage::database::GlobalUsageStore::new(db.clone()).get();
        assert_eq!(before.prompt_tokens, 0, "nothing flushed below threshold yet");

        // A read via the tracker's own global does NOT happen on its own —
        // the command layer calls flush_global explicitly. Verify it lands.
        tracker.flush_global();
        let after = crate::storage::database::GlobalUsageStore::new(db.clone()).get();
        assert_eq!(after.prompt_tokens, 300);
        assert_eq!(after.completion_tokens, 150);
        assert_eq!(after.turns, 3);
        // Release the DB handles before removing the temp dir (Windows locks
        // open database files).
        drop(tracker);
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn turn_zero_records_totals_without_creating_per_turn_slot() {
        // Non-streaming internals (force_final_answer, reflexion, prefire)
        // record on turn 0: totals + global aggregate must count them, but
        // no per-turn slot may be created or polluted — the old behavior
        // mapped turn 0 onto turn 1's slot and double-counted it
        // (#88 audit M35).
        let tracker = SessionUsageTracker::new("test-session");
        tracker.record_llm_usage(
            0,
            &TokenUsage {
                prompt_tokens: 500,
                completion_tokens: 100,
                cached_read_tokens: None,
                reasoning_tokens: None,
                prompt_cache_hit_tokens: None,
                prompt_cache_miss_tokens: None,
            },
        );
        let summary = tracker.summary();
        assert_eq!(summary.total_prompt_tokens, 500);
        assert_eq!(summary.total_completion_tokens, 100);
        assert_eq!(summary.turn_count, 0, "turn-0 usage must not create a turn");

        // A real turn-1 record after that must NOT inherit the turn-0 usage.
        tracker.record_llm_usage(
            1,
            &TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 50,
                cached_read_tokens: None,
                reasoning_tokens: None,
                prompt_cache_hit_tokens: None,
                prompt_cache_miss_tokens: None,
            },
        );
        let summary = tracker.summary();
        assert_eq!(summary.total_prompt_tokens, 1500, "totals accumulate");
        assert_eq!(summary.turn_count, 1);
    }
}
