//! Conversation compaction — multi-level context compression that reduces
//! conversation history length when approaching the context window limit.
//!
//! Architecture:
//!
//! - **item** — `CompactionItem` trait seam, abstracts `ConversationItem`
//! - **select** — tool-pair-safe split selection (`SplitPlan`)
//! - **sampler** — `CompactionSampler` trait + LLM implementation + error types
//! - **templates** — prompt templates for different compaction styles
//! - **history** — filtering, validation, orphan detection
//!
//! The `Compactor` struct ties it together. It supports one compaction
//! path — summarize the old prefix, keep the recent tail — with a `force`
//! mode that uses a tighter tail budget and harder tool-result filtering
//! (the tiered cache scheduling in `agent_loop/run.rs` drives both).
//!
//! With `two_pass_enabled`, a speculative prefire summary runs in the
//! background when usage approaches the threshold, reducing the latency
//! of the actual compaction pass. Stale prefires expire
//! ([`PREFIRE_STALE_AFTER`]) so an unconsumed summary neither blocks
//! future prefires nor feeds a pass with outdated content.
//!
//! The `code`/`inter`/`intra` module trees (full-replace / inter-turn /
//! intra-turn strategies) were removed as unwired dead code — the live
//! tiered cache scheduling lives in `agent_loop/run.rs` (50/60/80/90%)
//! and this `Compactor`.

pub mod history;
pub mod item;
pub mod sampler;
pub mod select;
pub mod templates;

#[cfg(test)]
mod live_smoke;

use std::sync::Arc;
use std::time::Duration;

use crate::agent::chat_state::ChatState;
use crate::agent::token::estimate_conversation_tokens;
use crate::core::error::{AppError, AppResult};
use crate::core::types::{ConversationItem, TokenUsage, ToolDefinition};
use crate::llm::client::LlmClient;
use crate::observability::usage::SessionUsageTracker;

use sampler::run_compaction_summary;
use sampler::LlmCompactionSampler;
use select::{select_by_token_budget, select_turns_to_compact};
use templates::{build_compaction_user_prompt, build_summary_message};

/// Summarize `filtered` conversation items in token-bounded chunks
/// (DivideAndConquer). A short history produces a single chunk — identical to
/// the previous single-pass behavior; a very long one is split so no single
/// LLM call exceeds the model context window and detail is preserved across
/// the chunk boundary. Chunk summaries are merged with `<chunk_summary>`
/// markers so the model can tell the parts apart.
///
/// Returns `None` when every chunk attempt fails to produce a usable summary;
/// the accompanying usage is the sum of every billed chunk attempt (even a
/// failed one) so the caller can record it into the session accounting.
async fn run_chunked_summary(
    sampler: &LlmCompactionSampler,
    filtered: &[crate::core::types::ConversationItem],
    cache_optimize: bool,
    system_prompt: &str,
    tools: &[crate::core::types::ToolDefinition],
    timeout: Duration,
    max_attempts: usize,
) -> AppResult<Option<(String, TokenUsage)>> {
    if filtered.is_empty() {
        return Ok(None);
    }

    // Anti-snowball: a prior compaction summary embedded in this history
    // carries its own user-query lines. Pull them out so the LLM is told not
    // to re-copy them into the fresh summary — otherwise the summary grows
    // every re-compaction round (summaries nested inside summaries).
    let prior_queries_raw: String = filtered
        .iter()
        .filter_map(|item| {
            if let crate::core::types::ConversationItem::System(s) = item {
                templates::extract_user_queries_from_summary(&s.content)
            } else {
                None
            }
        })
        .fold(String::new(), |mut acc, q| {
            if !acc.is_empty() {
                acc.push('\n');
            }
            acc.push_str(&q);
            acc
        });
    let prior_queries = (!prior_queries_raw.is_empty()).then_some(prior_queries_raw);
    // Pure instruction rides as the trailing user message; the session system
    // prompt + dropped chunks form the cache-hitting prefix.
    let instruction = templates::with_anti_copy_instruction(
        templates::COMPACTION_SYSTEM_PROMPT,
        prior_queries.is_some(),
    );

    // Plan chunks by estimated token size.
    let item_tokens: Vec<u32> = filtered
        .iter()
        .map(|i| crate::agent::token::estimate_item_tokens(i) as u32)
        .collect();
    let chunks = select::plan_compaction_chunks(
        filtered,
        &item_tokens,
        select::COMPACTION_CHUNK_TOKEN_BUDGET,
    );

    let mut merged = String::new();
    let mut total_usage = TokenUsage::default();
    for (i, range) in chunks.iter().enumerate() {
        let chunk_items = &filtered[range.start..range.end];
        let summary = if cache_optimize {
            run_compaction_summary(
                sampler,
                chunk_items,
                system_prompt,
                &instruction,
                tools,
                timeout,
                max_attempts,
            )
            .await?
        } else {
            let prompt = build_compaction_user_prompt(chunk_items);
            run_compaction_summary(
                sampler,
                chunk_items,
                templates::COMPACTION_SYSTEM_PROMPT,
                &prompt,
                &[],
                timeout,
                max_attempts,
            )
            .await?
        };
        let Some((summary, usage)) = summary else {
            return Ok(None);
        };
        total_usage.add(&usage);
        if chunks.len() > 1 {
            merged.push_str(&format!(
                "<chunk_summary index=\"{i}\">\n{summary}\n</chunk_summary>\n\n"
            ));
        } else {
            merged.push_str(&summary);
        }
    }

    // Preserve the prior user queries as a preamble so the original user
    // intents survive compaction without being duplicated in the body.
    let merged = templates::prepend_user_queries_preamble(&merged, prior_queries.as_deref());

    Ok(Some((merged, total_usage)))
}

/// A prefire summary produced by the background pre-compaction pass.
#[derive(Debug, Clone)]
pub struct PrefireSummary {
    /// The LLM-generated summary text.
    pub summary: String,
    /// The index in the conversation at which the prefire split occurred.
    pub split_idx: usize,
    /// When the summary was computed — prefires older than
    /// [`PREFIRE_STALE_AFTER`] are discarded: the conversation has moved on,
    /// and an unconsumed prefire must not block (or feed) a fresh pass.
    pub created_at: std::time::Instant,
}

/// Prefires older than this are stale: the summary was computed against an
/// old conversation prefix, and reusing it would compact the wrong content.
/// Also self-heals the "unconsumed prefire blocks all future prefires"
/// deadlock (tokens hovering below the compaction threshold kept a stored
/// prefire alive forever).
const PREFIRE_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

/// The compactor — summarizes old conversation history.
pub struct Compactor {
    llm_client: LlmClient,
    summarizer_model: String,
    /// Timeout for the LLM compaction call.
    timeout: Duration,
    /// Two-pass compaction: speculative prefire summary, stored when
    /// the background prefire task completes. Consumed by the actual
    /// compaction pass.
    prefire_summary: Arc<tokio::sync::Mutex<Option<PrefireSummary>>>,
    /// Whether two-pass compaction is enabled.
    two_pass_enabled: bool,
    /// Percentage of context window that triggers prefire (default 70).
    /// Compaction triggers at `threshold_percent` (default 80).
    prefire_threshold_percent: u8,
    /// Minimum total tokens worth compacting — skip tiny compactions
    /// where the LLM summarization cost is not worth the savings.
    min_compactable_tokens: u32,
    /// Maximum allowed ratio of (summary_tokens / compacted_tokens).
    /// If the summary is not substantially smaller than what it replaces,
    /// the compaction is discarded as insufficient.
    max_reduction_ratio: f64,
    /// Optional session usage tracker — records the usage of background
    /// prefire summaries so detached internal LLM calls are at least visible
    /// in the per-session stats (they still cannot reach
    /// `chat_state.total_usage`, which seeds the run budget; that remains a
    /// documented residual).
    usage_tracker: Option<SessionUsageTracker>,
}

impl Compactor {
    pub fn new(llm_client: LlmClient, summarizer_model: impl Into<String>) -> Self {
        Self {
            llm_client,
            summarizer_model: summarizer_model.into(),
            timeout: Duration::from_secs(60),
            prefire_summary: Arc::new(tokio::sync::Mutex::new(None)),
            two_pass_enabled: false,
            prefire_threshold_percent: 70,
            min_compactable_tokens: 200,
            max_reduction_ratio: 0.8,
            usage_tracker: None,
        }
    }

    /// Attach the session usage tracker — background prefire summaries then
    /// record their billed tokens into the per-session stats.
    pub fn with_usage_tracker(mut self, tracker: Option<SessionUsageTracker>) -> Self {
        self.usage_tracker = tracker;
        self
    }

    /// Enable two-pass compaction with a prefire at `prefire_threshold_percent`.
    pub fn with_two_pass(mut self, prefire_threshold_percent: u8) -> Self {
        self.two_pass_enabled = true;
        self.prefire_threshold_percent = prefire_threshold_percent;
        self
    }

    /// Tier-2 cache saver: cheaply snip STALE tool results (older than the
    /// recent tail) down to a short summary without any LLM round-trip.
    /// Keeps the conversation structure (tool call + result pair) intact so
    /// the prefix shape doesn't break more than necessary.
    ///
    /// Two aging tiers, mirroring reference soft-trim + hard-clear:
    /// - **Soft-trim** (stale region before the 16k tail): results over
    ///   `SNIP_MAX_CHARS` keep a head+tail window so recent enough details
    ///   survive while the bulk is dropped.
    /// - **Hard-clear** (very stale, older than an 8k sub-boundary): results
    ///   over `CLEAR_MIN_CHARS` are replaced with a compact placeholder —
    ///   they are far enough back that only the fact they existed matters.
    pub async fn snip_stale_tool_results(&self, chat_state: &mut ChatState) {
        let total = chat_state.conversation.len();
        if total < 8 {
            return;
        }
        // Stale region = everything before the recent window-scaled tail.
        let tail = tail_budget_for(chat_state.context_window, false);
        let split = select_by_token_budget(&chat_state.conversation, tail);
        if split < 2 {
            return;
        }
        // Very-stale sub-region = older than half the tail within the
        // stale region — these get hard-cleared rather than soft-trimmed.
        let hard_split = select_by_token_budget(&chat_state.conversation[..split], tail / 2);
        let mut changed = false;
        for (i, item) in chat_state.conversation[..split].iter_mut().enumerate() {
            if let crate::core::types::ConversationItem::ToolResult(tr) = item {
                if let Some(new_content) = age_tool_result_content(i < hard_split, &tr.content) {
                    tr.content = new_content;
                    changed = true;
                }
            }
        }
        if changed {
            // The database still holds the full unsnipped contents — the
            // persist checkpoint is stale, force a full rewrite.
            chat_state.persisted_upto = 0;
            tracing::info!(
                split,
                hard_split,
                "Tier-2: snipped stale tool results (cache-friendly context management)"
            );
        }
    }

    /// Maybe start a background prefire summarization.
    ///
    /// Called from the agent loop's Phase 1. When the estimated tokens
    /// exceed the prefire threshold (but not yet the compaction threshold),
    /// a background task summarizes the old conversation prefix and stores
    /// the result. The actual `compact_if_needed` call consumes this
    /// prefire summary if available, avoiding a duplicate LLM call.
    ///
    /// `estimated` is the FULL request estimate (conversation + transient +
    /// tail + goal) — the conversation-only estimate undercounts on
    /// tail-heavy sessions, deferring the prefire past where the compaction
    /// threshold would fire and losing the prefire → consume win.
    pub async fn maybe_prefire(
        &self,
        chat_state: &ChatState,
        threshold_percent: u8,
        estimated: u64,
    ) {
        if !self.two_pass_enabled {
            return;
        }

        let prefire_threshold =
            chat_state.context_window * self.prefire_threshold_percent as u64 / 100;
        let compact_threshold = chat_state.context_window * threshold_percent as u64 / 100;

        if estimated < prefire_threshold || estimated >= compact_threshold {
            return;
        }

        // A FRESH prefire blocks a new one; a stale one is discarded so the
        // slot re-arms — an unconsumed prefire must not block all future
        // prefires forever (tokens can hover below the compaction threshold
        // indefinitely, leaving the stored summary permanently unused).
        {
            let mut slot = self.prefire_summary.lock().await;
            if let Some(existing) = slot.as_ref() {
                if existing.created_at.elapsed() < PREFIRE_STALE_AFTER {
                    return;
                }
                tracing::debug!("Discarding stale prefire summary — re-arming prefire");
                *slot = None;
            }
        }

        let total = chat_state.conversation.len();
        if total < 4 {
            return;
        }

        let split_idx = select_by_token_budget(
            &chat_state.conversation,
            tail_budget_for(chat_state.context_window, false),
        );
        let old_messages = chat_state.conversation[..split_idx].to_vec();

        let llm = self.llm_client.clone();
        let model = self.summarizer_model.clone();
        let timeout = self.timeout;
        let prefire = self.prefire_summary.clone();
        let filter_config = history::FilterConfig::default();
        let usage_tracker = self.usage_tracker.clone();

        tracing::info!(
            estimated_tokens = estimated,
            prefire_threshold,
            "Starting background prefire compaction"
        );

        tokio::spawn(async move {
            let filtered_old = history::filter_history(&old_messages, &filter_config);
            let user_prompt = build_compaction_user_prompt(&filtered_old);

            let sampler = LlmCompactionSampler::new(llm, &model);
            match run_compaction_summary(
                &sampler,
                &filtered_old,
                templates::COMPACTION_SYSTEM_PROMPT,
                &user_prompt,
                &[],
                timeout,
                sampler::MAX_COMPACTION_ATTEMPTS,
            )
            .await
            {
                Ok(Some((summary, usage))) => {
                    *prefire.lock().await = Some(PrefireSummary {
                        summary,
                        split_idx,
                        created_at: std::time::Instant::now(),
                    });
                    // The prefire runs on a detached background task with no
                    // `chat_state` handle, so its billed tokens cannot reach
                    // `total_usage` (the run-budget seed). The per-session
                    // usage TRACKER is thread-safe and IS recorded here —
                    // the tokens are no longer invisible in the usage stats.
                    if let Some(ref tracker) = usage_tracker {
                        tracker.record_llm_usage(0, &usage);
                    }
                    tracing::info!("Prefire compaction complete");
                }
                Ok(None) => {
                    tracing::warn!("Prefire compaction produced no usable summary");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Prefire compaction failed");
                }
            }
        });
    }

    /// Run compaction if needed.
    ///
    /// Returns `Some(compacted_tokens)` when compaction occurred, or `None`
    /// when no compaction was needed. `force` selects the tighter tail
    /// budget and skips the threshold check (tier-4 forced compaction).
    /// `estimated_request_override` supplies the FULL request estimate
    /// (system + conversation + tools + transient + tail + goal/guidance)
    /// computed by the loop: the plain conversation-based estimate
    /// undercounts by thousands of tokens, which delayed the 80% tier until
    /// the prompt-too-long emergency path.
    #[allow(clippy::too_many_arguments)]
    pub async fn compact_if_needed(
        &self,
        chat_state: &mut ChatState,
        tools: &[ToolDefinition],
        threshold_percent: u8,
        force: bool,
        cache_optimize: bool,
        estimated_request_override: Option<u64>,
        memory: std::sync::Arc<crate::memory::store::MemoryStore>,
        workspace: Option<&std::path::Path>,
    ) -> AppResult<Option<u64>> {
        let threshold = chat_state.context_window * threshold_percent as u64 / 100;

        // The threshold skip uses the FULL request estimate in BOTH paths —
        // the conversation-only estimate undercounts the per-request tail /
        // transient guidance / goal allowance and would skip compaction in
        // the 80-90% window, deferring every compact to the 90% force tier
        // (requests run near-overflow, attention diluted). The caller
        // (phase_context) computes the override AFTER tier-2 pruning, so the
        // skip still implements dsh's prune → re-measure → summarize
        // gradient: a prune that already brought the request under threshold
        // skips the LLM summary entirely.
        let estimated = estimated_request_override
            .unwrap_or_else(|| chat_state.estimated_request_tokens(tools));
        if !force && estimated < threshold {
            return Ok(None);
        }

        tracing::info!(
            estimated_tokens = estimated,
            threshold = threshold,
            "Starting conversation compaction"
        );

        let total = chat_state.conversation.len();
        if total < 4 {
            return Ok(None);
        }

        // Cache-first tail budget, scaled to the session's context window:
        // a FIXED 16k tail is wrong at both extremes — on a 1M window it
        // discards ~98% of the conversation into a summary (massive
        // fidelity loss), while on a 32k window it retains half the history
        // (compaction barely saves anything). `force` keeps even less (the
        // aggressive tail) and snips tool results harder.
        let tail_budget = tail_budget_for(chat_state.context_window, force);
        let filter_config = if force {
            history::FilterConfig {
                tool_result_max_chars: 300,
                ..Default::default()
            }
        } else {
            history::FilterConfig::default()
        };

        // Check for a prefire summary from the background pre-compaction
        // pass. A STALE prefire is discarded (treated as absent) — it was
        // computed against an older conversation and must not be reused.
        let prefire = self.prefire_summary.lock().await.take().filter(|p| {
            if p.created_at.elapsed() >= PREFIRE_STALE_AFTER {
                tracing::debug!("Discarding stale prefire summary in compaction pass");
                false
            } else {
                true
            }
        });

        // Resolve the split point BEFORE any LLM call so the guards can
        // veto the compaction without burning summary fees. `force` ignores
        // the prefire split (a non-force cut that would leave the context
        // over budget after an emergency pass) and recomputes with the
        // tighter force tail budget.
        let split_idx = match &prefire {
            Some(p) if !force && p.split_idx <= total => p.split_idx,
            _ => select_by_token_budget(&chat_state.conversation, tail_budget),
        };

        // Guard: skip compaction when the compacted prefix is too small —
        // the LLM summarization cost is not worth the token savings.
        let compacted_tokens = estimate_conversation_tokens(&chat_state.conversation[..split_idx]);
        if compacted_tokens < self.min_compactable_tokens as u64 {
            tracing::debug!(
                compacted_tokens,
                min_compactable = self.min_compactable_tokens,
                "Compaction skipped — compactable prefix below minimum"
            );
            return Ok(None);
        }

        let (summary, usage, split_idx) = if let Some(prefire) = prefire {
            if !force && prefire.split_idx <= total {
                tracing::info!(
                    prefire_split = prefire.split_idx,
                    "Using prefire summary from background pre-compaction"
                );
                (prefire.summary, TokenUsage::default(), prefire.split_idx)
            } else {
                tracing::info!(
                    prefire_split = prefire.split_idx,
                    force,
                    "Prefire split ignored — recomputing split for this pass"
                );
                let (s, usage) = self
                    .summarize_old(
                        &chat_state.conversation[..split_idx],
                        &filter_config,
                        false,
                        cache_optimize,
                        &chat_state.system_prompt,
                        tools,
                    )
                    .await?;
                (s, usage, split_idx)
            }
        } else {
            // Standard compaction — DivideAndConquer over the filtered
            // history: split into token-bounded chunks and summarize each
            // independently. A short history produces a single chunk (the
            // common case); a very long one avoids an oversized single LLM
            // call that risks context overflow and detail loss.
            let (s, usage) = self
                .summarize_old(
                    &chat_state.conversation[..split_idx],
                    &filter_config,
                    true,
                    cache_optimize,
                    &chat_state.system_prompt,
                    tools,
                )
                .await?;
            (s, usage, split_idx)
        };

        // The summarization LLM calls were billed — record them into the
        // session total even when the compaction is later rejected by the
        // reduction guard or the orphan check (the tokens were spent either
        // way). This keeps the session usage/cost limits honest on the
        // "compaction attempted but discarded" path.
        if usage.total() > 0 {
            chat_state.total_usage.add(&usage);
        }

        let recent_messages = &chat_state.conversation[split_idx..];

        // Guard: reject the compaction when the summary is not substantially
        // smaller than the messages it replaces (wasted LLM cost).
        let summary_tokens = estimate_conversation_tokens(&[build_summary_message(&summary)]);
        if summary_tokens > (compacted_tokens as f64 * self.max_reduction_ratio) as u64 {
            tracing::warn!(
                compacted_tokens,
                summary_tokens,
                max_ratio = self.max_reduction_ratio,
                "Compaction discarded — insufficient reduction"
            );
            return Ok(None);
        }

        // Build the compacted conversation
        let mut new_conversation = vec![build_summary_message(&summary)];
        new_conversation.extend(recent_messages.iter().cloned());

        // Validate no orphans
        if let Err(e) = history::validate_no_orphans(&new_conversation) {
            tracing::warn!(error = %e, "Orphaned tool results detected after compaction — skipping");
            return Ok(None);
        }

        // Externalize: before the old prefix is discarded, pull out the
        // conversation-unique details the summary may not cover (decisions,
        // temporary constraints, unfinished work, key file paths) into
        // learnings — the compression loss is caught by external recall.
        // Background, never blocks the compaction.
        let drop = chat_state.conversation[..split_idx].to_vec();
        self.externalize_drop(
            chat_state.provider.clone(),
            drop,
            Some(memory.clone()),
            workspace.map(|p| p.to_path_buf()),
        );

        chat_state.replace_conversation(new_conversation);
        chat_state.compaction_pending = false;

        tracing::info!(
            compacted_tokens = compacted_tokens,
            new_length = chat_state.conversation.len(),
            "Compaction complete"
        );

        Ok(Some(compacted_tokens))
    }

    /// Produce the summary text for `conversation[..split_idx]` with the
    /// given filter. `chunked` selects the DivideAndConquer path (standard
    /// compaction); the single-shot path serves prefire fallbacks.
    async fn summarize_old(
        &self,
        conversation: &[crate::core::types::ConversationItem],
        filter_config: &history::FilterConfig,
        chunked: bool,
        cache_optimize: bool,
        system_prompt: &str,
        tools: &[crate::core::types::ToolDefinition],
    ) -> AppResult<(String, TokenUsage)> {
        let filtered = history::filter_history(conversation, filter_config);
        let sampler = LlmCompactionSampler::new(self.llm_client.clone(), &self.summarizer_model);
        let (summary, usage) = if chunked {
            run_chunked_summary(
                &sampler,
                &filtered,
                cache_optimize,
                system_prompt,
                tools,
                self.timeout,
                sampler::MAX_COMPACTION_ATTEMPTS,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal("Compaction failed to produce a usable summary".into())
            })?
        } else if cache_optimize {
            // Cache-aware summarization (dsh discipline): the summary call
            // reuses the SESSION's system prompt + tool definitions and feeds
            // the dropped prefix as raw messages, so its prompt prefix hits
            // the session's KV cache instead of starting cold. The pure
            // instruction rides as the trailing user message.
            run_compaction_summary(
                &sampler,
                &filtered,
                system_prompt,
                templates::COMPACTION_SYSTEM_PROMPT,
                tools,
                self.timeout,
                sampler::MAX_COMPACTION_ATTEMPTS,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal("Compaction failed to produce a usable summary".into())
            })?
        } else {
            // Plain path (DeepSeek optimization off / non-DeepSeek model):
            // standalone summarizer prompt + serialized history.
            let prompt = build_compaction_user_prompt(&filtered);
            run_compaction_summary(
                &sampler,
                &filtered,
                templates::COMPACTION_SYSTEM_PROMPT,
                &prompt,
                &[],
                self.timeout,
                sampler::MAX_COMPACTION_ATTEMPTS,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal("Compaction failed to produce a usable summary".into())
            })?
        };
        Ok((summary, usage))
    }

    /// Run a token-budget-based compaction using the split selector.
    ///
    /// This is more precise than the fraction-based approach: it
    /// keeps as many recent items as fit within a token budget.
    pub async fn compact_with_budget(
        &self,
        chat_state: &mut ChatState,
        target_tokens: u32,
        min_compactable: u32,
        memory: Option<std::sync::Arc<crate::memory::store::MemoryStore>>,
        workspace: Option<&std::path::Path>,
    ) -> AppResult<Option<u64>> {
        let items = &chat_state.conversation;
        let token_counts: Vec<u32> = items
            .iter()
            .map(|i| crate::agent::token::estimate_item_tokens(i) as u32)
            .collect();

        let plan =
            match select_turns_to_compact(items, &token_counts, target_tokens, min_compactable) {
                Some(p) => p,
                None => return Ok(None),
            };

        let old_messages = &chat_state.conversation[..plan.split_idx];
        let recent_messages = &chat_state.conversation[plan.split_idx..];

        // Guard: skip compaction when the compactable prefix is too small.
        // The caller-supplied `min_compactable` may intentionally lower the
        // threshold (emergency compaction after prompt-too-long), so the
        // effective floor is the smaller of the two.
        let effective_min = min_compactable.min(self.min_compactable_tokens);
        if plan.tokens_to_compact < effective_min {
            tracing::debug!(
                compacted_tokens = plan.tokens_to_compact,
                min_compactable = effective_min,
                "Budget compaction skipped — compactable prefix below minimum"
            );
            return Ok(None);
        }

        let user_prompt = build_compaction_user_prompt(old_messages);

        let sampler = LlmCompactionSampler::new(self.llm_client.clone(), &self.summarizer_model);
        let (summary, usage) = run_compaction_summary(
            &sampler,
            old_messages,
            templates::COMPACTION_SYSTEM_PROMPT,
            &user_prompt,
            &[],
            self.timeout,
            sampler::MAX_COMPACTION_ATTEMPTS,
        )
        .await?
        .ok_or_else(|| {
            AppError::Internal("Budget compaction failed to produce a usable summary".into())
        })?;

        // Billed compaction calls must count toward the session total even
        // when the resulting summary is later rejected by the guards.
        if usage.total() > 0 {
            chat_state.total_usage.add(&usage);
        }

        let mut new_conversation = vec![build_summary_message(&summary)];
        new_conversation.extend(recent_messages.iter().cloned());

        // Guard: reject the compaction when the summary is not substantially
        // smaller than the messages it replaces.
        let summary_tokens = estimate_conversation_tokens(&new_conversation[..1]);
        if summary_tokens > (plan.tokens_to_compact as f64 * self.max_reduction_ratio) as u64 {
            tracing::warn!(
                compacted_tokens = plan.tokens_to_compact,
                summary_tokens,
                max_ratio = self.max_reduction_ratio,
                "Budget compaction discarded — insufficient reduction"
            );
            return Ok(None);
        }

        if let Err(e) = history::validate_no_orphans(&new_conversation) {
            tracing::warn!(error = %e, "Orphaned tool results after budget compaction — skipping");
            return Ok(None);
        }

        // Externalize the dropped prefix before it is discarded — same
        // contract as compact_if_needed, so the emergency prompt-too-long
        // path does not lose conversation-unique details (decisions,
        // temporary constraints, unfinished work, key file paths).
        let drop = chat_state.conversation[..plan.split_idx].to_vec();
        self.externalize_drop(
            chat_state.provider.clone(),
            drop,
            memory.clone(),
            workspace.map(|p| p.to_path_buf()),
        );

        let compacted_tokens = plan.tokens_to_compact as u64;
        chat_state.replace_conversation(new_conversation);
        chat_state.compaction_pending = false;

        Ok(Some(compacted_tokens))
    }

    /// Spawn background externalization of a dropped conversation prefix into
    /// learnings — the compression loss is caught by external recall instead
    /// of silently vanishing. Never blocks the compaction. Skips silently
    /// when no memory store is available (e.g. smoke tests).
    fn externalize_drop(
        &self,
        provider: Option<String>,
        drop: Vec<ConversationItem>,
        memory: Option<std::sync::Arc<crate::memory::store::MemoryStore>>,
        workspace: Option<std::path::PathBuf>,
    ) {
        let llm = self.llm_client.clone();
        let model = self.summarizer_model.clone();
        tauri::async_runtime::spawn(async move {
            let critical =
                crate::memory::learning::extract_critical_from_drop(&llm, &model, provider.as_deref(), &drop)
                    .await;
            if critical.is_empty() {
                return;
            }
            // Externalized learnings are project-level knowledge — store
            // globally (no session) so they are retrievable from any session,
            // mirroring the workspace learnings file. Dedupe against the
            // store first, exactly like run_learning_pass: repeated
            // compactions otherwise re-store overlapping "unfinished work /
            // decisions" items as duplicate rows that surface as repeated
            // injected memories.
            if let Some(memory) = memory {
                let existing: Vec<String> = memory
                    .search_by_category("learning", 5000)
                    .map(|ms| ms.into_iter().map(|m| m.content).collect())
                    .unwrap_or_default();
                let fresh = crate::memory::learning::dedupe_learnings(critical.clone(), &existing);
                for c in &fresh {
                    let _ = memory.store(c, "learning", None, None);
                }
            }
            if let Some(path) = crate::memory::learning::learnings_path(workspace.as_deref()) {
                let _ = crate::memory::learning::persist_learnings_file(&path, &critical);
            }
        });
    }
}

/// Tail budget for compaction, scaled to the session's context window.
///
/// A fixed 16k tail is wrong at both extremes: on a 1M-token window it
/// discards ~98% of the conversation into a summary (massive fidelity
/// loss), while on a 32k window it retains half the history (compaction
/// barely saves anything). A ~10% slice — clamped to [8k, 64k]; forced
/// compaction halves it to [4k, 32k] — keeps the preserved recent context
/// proportional to the model's actual window while bounding the worst case
/// in either direction.
fn tail_budget_for(context_window: u64, force: bool) -> u64 {
    let fraction = context_window / 10;
    if force {
        (fraction / 2).clamp(4_096, 32_768)
    } else {
        fraction.clamp(8_192, 65_536)
    }
}

/// Age one stale tool-result's content per the tier-2 aging policy.
///
/// - `is_very_stale` + content over [`CLEAR_MIN_CHARS`] → replaced with a
///   compact placeholder (hard-clear): only the fact the result existed is
///   worth keeping that far back.
/// - Stale (but not very) + content over [`SNIP_MAX_CHARS`] → soft-trim to a
///   head + tail window so recent-enough details survive.
/// - Otherwise unchanged (`None`).
///
/// Pure and unit-testable; used by [`Compactor::snip_stale_tool_results`].
fn age_tool_result_content(is_very_stale: bool, content: &str) -> Option<String> {
    if is_very_stale && content.len() > CLEAR_MIN_CHARS {
        return Some(format!(
            "[Tool result omitted — too old to retain ({} chars)]",
            content.len()
        ));
    }
    if content.len() > SNIP_MAX_CHARS {
        let chars: Vec<char> = content.chars().collect();
        let head: String = chars.iter().take(SNIP_MAX_CHARS).collect();
        let tail: String = chars
            .iter()
            .skip(chars.len().saturating_sub(SNIP_TAIL_CHARS))
            .collect();
        return Some(format!(
            "{head}\n…[truncated by tier-2 context management ({})]\n…{tail}",
            content.len()
        ));
    }
    None
}

/// Largest tool-result kept whole in the stale region (soft-trim threshold).
const SNIP_MAX_CHARS: usize = 400;
/// Results this large in the very-stale region get hard-cleared.
const CLEAR_MIN_CHARS: usize = 1_000;
/// Tail window preserved by soft-trim.
const SNIP_TAIL_CHARS: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_plan_fields() {
        let plan = select::SplitPlan {
            split_idx: 5,
            tokens_to_compact: 1000,
        };
        assert_eq!(plan.split_idx, 5);
        assert_eq!(plan.tokens_to_compact, 1000);
    }

    #[test]
    fn two_pass_disabled_by_default() {
        let compactor = Compactor::new(
            LlmClient::new(
                vec![],
                crate::llm::retry::RetryConfig::default(),
                false,
                std::sync::Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                    crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
                )),
            ),
            "test-model",
        );
        assert!(!compactor.two_pass_enabled);
        assert_eq!(compactor.prefire_threshold_percent, 70);
    }

    #[test]
    fn with_two_pass_enables_and_sets_threshold() {
        let compactor = Compactor::new(
            LlmClient::new(
                vec![],
                crate::llm::retry::RetryConfig::default(),
                false,
                std::sync::Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                    crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
                )),
            ),
            "test-model",
        )
        .with_two_pass(65);
        assert!(compactor.two_pass_enabled);
        assert_eq!(compactor.prefire_threshold_percent, 65);
    }

    #[tokio::test]
    async fn prefire_summary_starts_empty() {
        let compactor = Compactor::new(
            LlmClient::new(
                vec![],
                crate::llm::retry::RetryConfig::default(),
                false,
                std::sync::Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                    crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
                )),
            ),
            "test-model",
        );
        assert!(compactor.prefire_summary.lock().await.is_none());
    }

    #[tokio::test]
    async fn prefire_summary_can_be_set_and_consumed() {
        let compactor = Compactor::new(
            LlmClient::new(
                vec![],
                crate::llm::retry::RetryConfig::default(),
                false,
                std::sync::Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                    crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
                )),
            ),
            "test-model",
        );
        *compactor.prefire_summary.lock().await = Some(PrefireSummary {
            summary: "test summary".to_string(),
            split_idx: 5,
            created_at: std::time::Instant::now(),
        });
        let taken = compactor.prefire_summary.lock().await.take();
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().summary, "test summary");
        // Second take returns None
        assert!(compactor.prefire_summary.lock().await.is_none());
    }

    #[test]
    fn age_small_result_is_unchanged() {
        assert!(age_tool_result_content(false, "short").is_none());
        assert!(age_tool_result_content(true, "short").is_none());
    }

    #[test]
    fn age_stale_large_result_soft_trims_head_and_tail() {
        let big = "x".repeat(SNIP_MAX_CHARS + 1_000);
        let aged = age_tool_result_content(false, &big).expect("large stale result must trim");
        assert!(aged.len() < big.len(), "trimmed must be smaller");
        assert!(aged.contains("truncated by tier-2 context management"));
        assert!(
            aged.starts_with(&"x".repeat(SNIP_MAX_CHARS)),
            "head window must be preserved"
        );
        assert!(
            aged.ends_with(&"x".repeat(SNIP_TAIL_CHARS)),
            "tail window must be preserved"
        );
    }

    #[test]
    fn age_very_stale_large_result_hard_clears() {
        let big = "y".repeat(CLEAR_MIN_CHARS + 500);
        let aged = age_tool_result_content(true, &big).expect("very stale large must clear");
        assert!(
            aged.contains("too old to retain"),
            "placeholder must mark the clear, got: {aged}"
        );
        assert!(!aged.contains('y'), "content must be gone, got: {aged}");
        assert!(aged.len() < 100, "placeholder must be tiny");
    }

    #[test]
    fn tail_budget_scales_with_window_and_clamps() {
        // Small windows keep the minimum tail (8k / 4k force).
        assert_eq!(tail_budget_for(32_768, false), 8_192);
        assert_eq!(tail_budget_for(32_768, true), 4_096);
        // Mid windows follow the ~10% fraction (128k → 12.8k ≈ old 16k).
        assert_eq!(tail_budget_for(128_000, false), 12_800);
        assert_eq!(tail_budget_for(128_000, true), 6_400);
        // Huge windows cap at 64k / 32k — never summarize 98% of a 1M
        // window into oblivion, never spend unbounded summarization cost.
        assert_eq!(tail_budget_for(1_000_000, false), 65_536);
        assert_eq!(tail_budget_for(1_000_000, true), 32_768);
        // Forced tail is strictly smaller than the standard one.
        for window in [16_384, 64_000, 256_000, 2_000_000] {
            assert!(tail_budget_for(window, true) <= tail_budget_for(window, false));
        }
    }
}
