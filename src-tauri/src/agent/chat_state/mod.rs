//! Chat state management — holds conversation history, tracks token usage,
//! and manages compaction triggers.
//!
//! Submodules:
//! - `snapshot` — state snapshot/restore/truncate for rewind and forking
//! - `tests` — unit tests for conversation repair

pub mod snapshot;

#[cfg(test)]
mod tests;

use crate::agent::token::estimate_conversation_tokens;
use crate::core::types::ToolDefinition;
use crate::core::types::{ContentPart, ConversationItem, TokenUsage, ToolCall};
use std::collections::{BTreeMap, BTreeSet};
use tracing::info;

/// The mutable conversation state for a single session.
#[derive(Clone)]
pub struct ChatState {
    /// The full conversation history (system + user + assistant + tool results).
    pub conversation: Vec<ConversationItem>,
    /// How many conversation items are already persisted to SQLite, i.e.
    /// the checkpoint for incremental appends. Structural rewrites
    /// (compaction, rewind/truncate, dangling-call repair, snip) reset it
    /// to 0 so the next persist rewrites the whole history; plain tail
    /// pushes keep it — the persist path then appends only the new tail.
    pub persisted_upto: usize,
    /// The system prompt for this session.
    pub system_prompt: String,
    /// The model ID being used.
    pub model: String,
    /// The provider name this session routes to (e.g. "deepseek", "openai").
    /// Propagated into `LlmRequest` so the client can bypass prefix matching.
    pub provider: Option<String>,
    /// Full-run trace id — one identifier for the whole task, emitted in
    /// stream events and re-broadcast by every protocol (chat / ACP / SSE).
    pub trace_id: Option<String>,
    /// The context window size (in tokens) for the model.
    pub context_window: u64,
    /// Accumulated token usage across all turns.
    pub total_usage: TokenUsage,
    /// Current prompt index (incremented per user turn).
    pub prompt_index: usize,
    /// Cached prompt texts for rewind preview.
    pub prompt_texts: Vec<String>,
    /// File paths the agent has edited during this session.
    pub agent_edited_paths: BTreeSet<String>,
    /// Resolved file path → whether the auto-pulled LSP diagnostics after
    /// the agent's last edit of that file were clean. Populated by the tool
    /// executor (tool_batch) after every successful write when an LSP server
    /// is already running; consumed by the verification gate (run.rs) as
    /// structured verification evidence — a clean pull means the file
    /// type-checks, an error pull means the edit is not verified.
    pub auto_diagnostics: BTreeMap<String, bool>,
    /// Whether compaction has occurred.
    pub last_compaction_index: Option<usize>,
    /// Prompt index at which the last compaction occurred.
    pub last_compaction_prompt_index: Option<usize>,
    /// Whether a compaction is pending (should be triggered before next API call).
    pub compaction_pending: bool,
    /// Intent decision of the last substantive message — short follow-ups
    /// ("继续", "再优化一下") inherit it instead of being re-classified as
    /// casual chat.
    pub last_intent_decision: Option<crate::agent::intent::IntentDecision>,
    /// Turn capture state — tracks which messages belong to the current turn.
    pub turn_capture: Option<TurnCapture>,
    /// Consecutive empty response count — tracks how many times the LLM
    /// returned empty content (reasoning-only or truly empty). Reset to 0
    /// when a non-empty response is received. Used by the recovery loop.
    pub empty_response_count: u32,
    /// Transient system messages — reminders, task notifications, truncation
    /// prompts, hook corrections. Included in API requests via
    /// [`ChatState::request_messages`] but NEVER persisted to the database,
    /// so restarted sessions don't replay stale guidance at the model.
    pub transient_system: Vec<String>,
    /// Transient images (multimodal tool results) — included in API requests
    /// via [`ChatState::request_messages`] but NEVER persisted. They reach the
    /// model once, in the request that follows the tool call that produced
    /// them, then are cleared on the next user turn.
    pub transient_images: Vec<ContentPart>,
    /// Images attached to the CURRENT user message by a multimodal main model
    /// path (send-time injection). Consumed ONCE by the first
    /// [`ChatState::request_messages`] call — never repeated, never persisted.
    /// Text-only models (DeepSeek) never set this: their pictures are
    /// transcribed to text before the agent loop runs.
    pub initial_image_parts: Vec<ContentPart>,
    /// `(name, path)` notes for images attached to the current user message by
    /// a TEXT-ONLY main model path (send-time transcription). Injected into
    /// subagent contexts when the parent spawns workers so they can
    /// `visual_describe` a picture by path. Turn-local like
    /// `initial_image_parts` — never persisted, cleared on rewind. Multimodal
    /// parent sessions never set this (their pictures travel as image parts).
    pub attached_image_notes: Vec<(String, String)>,
    /// Repeat-failure guard (ronx-style): tool_name + normalized args →
    /// consecutive failure count for THIS session. A call whose signature has
    /// failed twice is blocked before dispatch with a corrective hint, so the
    /// model stops burning tokens retrying the same doomed operation. Any
    /// successful call clears its signature's count. Not persisted.
    pub tool_failure_counts: std::collections::HashMap<String, u32>,
    /// Per-tool-name consecutive failure count (ANY arguments) for this
    /// session. Unlike `tool_failure_counts` (identical retry only), this
    /// catches a tool failing under many different arguments — e.g. `bash`
    /// with `mvn`, then `javac`, then `java`, all missing from PATH. Drives
    /// the strategy-switch nudge (#84): N consecutive failures on one tool
    /// means the approach is wrong, not the arguments. A single success on
    /// the tool clears its count. Not persisted.
    pub tool_name_failures: std::collections::HashMap<String, u32>,
    /// Whether the tier-1 soft usage warning has already been injected this
    /// session (report once at ~50% of the window, never nag repeatedly).
    pub soft_warning_sent: bool,
    /// Aggregate budget (chars) for tool results in the current tool batch —
    /// a parallel round of several 32k results must not flood the context
    /// window (DeepSeek R1 has a 64k window). Reset per batch; enforced by
    /// [`ChatState::cap_to_batch_budget`].
    pub tool_result_batch_budget: u64,
    /// Chars of tool-result content already injected this batch.
    pub tool_result_batch_used: u64,
}

/// Hard cap on transient system messages — oldest entries are dropped
/// beyond this (each is a few hundred tokens at most).
const MAX_TRANSIENT_SYSTEM: usize = 32;

/// Hard cap on transient images per turn — a batch reading many pictures at
/// once must not flood the next request. Oldest images are dropped first.
const MAX_TRANSIENT_IMAGES: usize = 5;

/// Fraction of the context window reserved for ONE batch of tool results.
/// 50% keeps a single parallel round (5 × 32k) from overflowing a 64k R1
/// window or drowning the next reasoning round in raw output. The
/// conversation still grows across rounds — that is compaction's job — but
/// no single request-build can be flooded.
const TOOL_RESULT_BUDGET_FRACTION: f64 = 0.5;
/// Floor for the per-batch tool-result budget (chars) — small windows still
/// get a usable budget before the cap engages.
const TOOL_RESULT_BUDGET_MIN: u64 = 16_384;
/// Ceiling for the per-batch tool-result budget (chars) — huge windows do
/// not need more raw output than this in one round.
const TOOL_RESULT_BUDGET_MAX: u64 = 96_000;

/// Per-batch tool-result budget (chars) for the given context window.
/// Returns `0` — disabling the cap — when the window is uninitialized.
fn tool_result_batch_budget_for(context_window: u64) -> u64 {
    if context_window == 0 {
        return 0;
    }
    ((context_window as f64 * TOOL_RESULT_BUDGET_FRACTION) as u64)
        .clamp(TOOL_RESULT_BUDGET_MIN, TOOL_RESULT_BUDGET_MAX)
}

/// Whether a system message is transient guidance that must not survive a
/// restart — used to scrub legacy persisted sessions written by builds that
/// stored reminders/notifications in the conversation.
fn is_transient_system_text(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("[SYSTEM REMINDER]")
        || t.starts_with("<task-notification")
        || t.starts_with("<user_query")
        || t.starts_with("You haven't updated your TODO list recently")
        || t.starts_with("Your previous response was cut off because it hit the output token limit")
        || t.starts_with("Provide a concise conclusion of the remaining")
        || t.starts_with("A Stop hook requested changes")
}

/// Sum the per-call usage persisted on assistant messages — exact API
/// numbers beat the bytes/4 estimate for a restored session. Returns `None`
/// when any assistant message lacks usage (legacy rows predate per-message
/// accounting) or the recorded total is zero (columns never populated), so
/// the caller falls back to the estimate.
fn exact_history_tokens(items: &[ConversationItem]) -> Option<u64> {
    let mut total = 0u64;
    for item in items {
        if let ConversationItem::Assistant(a) = item {
            let usage = a.usage.as_ref()?;
            total += usage.prompt_tokens + usage.completion_tokens;
        }
    }
    (total > 0).then_some(total)
}

/// Tracks which conversation items belong to the current turn without
/// cloning every pushed item. Uses offset-based capture.
#[derive(Debug, Clone)]
pub struct TurnCapture {
    /// Index into `conversation` where this turn's messages start.
    pub turn_start_offset: usize,
    /// Messages saved from before a conversation replacement (compaction).
    pub pre_replacement_messages: Vec<ConversationItem>,
    /// Whether compaction occurred during this capture.
    pub compaction_occurred: bool,
}

impl ChatState {
    /// Runtime invariants — cheap debug-build checks that catch state
    /// corruption (audit: total_usage reset, prompt_index/prompt_texts
    /// desync) at the mutation site instead of a distant symptom. The
    /// counters that seed the session budget and rewind must stay consistent
    /// with each other and with the conversation they describe.
    #[cfg(debug_assertions)]
    fn check_invariants(&self) {
        assert_eq!(
            self.prompt_index,
            self.prompt_texts.len(),
            "prompt_index {} desynced from prompt_texts.len() {}",
            self.prompt_index,
            self.prompt_texts.len()
        );
        assert!(
            self.persisted_upto <= self.conversation.len(),
            "persisted_upto {} exceeds conversation.len() {}",
            self.persisted_upto,
            self.conversation.len()
        );
    }

    #[cfg(not(debug_assertions))]
    fn check_invariants(&self) {}

    /// Create a new chat state with the given model and context window.
    /// Test-only convenience: production callers always carry the session's
    /// provider hint via [`Self::with_provider`] (a dropped provider hint
    /// silently re-routes custom models to the first enabled provider).
    #[cfg(test)]
    pub fn new(model: impl Into<String>, context_window: u64) -> Self {
        Self::with_provider(model, context_window, None)
    }

    /// Create a new chat state with an explicit provider hint.
    pub fn with_provider(
        model: impl Into<String>,
        context_window: u64,
        provider: Option<String>,
    ) -> Self {
        Self {
            conversation: Vec::new(),
            persisted_upto: 0,
            system_prompt: String::new(),
            model: model.into(),
            provider,
            trace_id: None,
            context_window,
            total_usage: TokenUsage::default(),
            prompt_index: 0,
            prompt_texts: Vec::new(),
            agent_edited_paths: BTreeSet::new(),
            auto_diagnostics: BTreeMap::new(),
            last_compaction_index: None,
            last_compaction_prompt_index: None,
            compaction_pending: false,
            last_intent_decision: None,
            turn_capture: None,
            empty_response_count: 0,
            transient_system: Vec::new(),
            transient_images: Vec::new(),
            initial_image_parts: Vec::new(),
            attached_image_notes: Vec::new(),
            tool_failure_counts: std::collections::HashMap::new(),
            tool_name_failures: std::collections::HashMap::new(),
            soft_warning_sent: false,
            tool_result_batch_budget: 0,
            tool_result_batch_used: 0,
        }
    }

    /// Restore a chat state from persisted conversation history.
    ///
    /// Automatically repairs dangling tool calls (assistant tool calls
    /// without matching tool results) that may result from a crash or
    /// cancellation mid-tool-execution. Legacy persisted sessions are also
    /// scrubbed of transient system guidance (reminders, notifications)
    /// that older builds wrote to the database.
    pub fn from_history(
        conversation: Vec<ConversationItem>,
        model: impl Into<String>,
        context_window: u64,
        provider: Option<String>,
    ) -> Self {
        let conversation: Vec<ConversationItem> = conversation
            .into_iter()
            .filter(|item| match item {
                ConversationItem::System(msg) => !is_transient_system_text(&msg.content),
                _ => true,
            })
            .collect();
        // Prefer the EXACT per-call usage persisted on each assistant
        // message (the API's real numbers) over the bytes/4 estimate —
        // the estimate overwrote the exact values on restore. Falls back
        // to the estimate for legacy sessions whose rows predate
        // per-message usage accounting.
        let total_tokens = exact_history_tokens(&conversation)
            .unwrap_or_else(|| estimate_conversation_tokens(&conversation));
        let mut state = Self::with_provider(model, context_window, provider);
        // Track the DB row count BEFORE repair: `repair_dangling_tool_calls`
        // may insert synthetic rows the database does not have, so the
        // persisted checkpoint must point at the original rows — the next
        // persist then appends exactly the synthetic insertions.
        let db_message_count = conversation.len();
        state.conversation = conversation;
        state.total_usage.prompt_tokens = total_tokens;
        state.repair_dangling_tool_calls();
        state.persisted_upto = db_message_count.min(state.conversation.len());
        // Rebuild rewind bookkeeping from the loaded conversation. A restored
        // session must know how many user turns it holds — prompt_index drives
        // turn_count persistence and the rewind guard (`prompt_index >= target`),
        // and prompt_texts feeds truncate_from_user_message. `with_provider`
        // left both at 0, so a reloaded session reported 0 turns: rewind
        // silently skipped the conversation truncation and the model kept
        // reasoning from stale "I already did X" memory.
        state.prompt_texts = state
            .conversation
            .iter()
            .filter_map(|item| match item {
                ConversationItem::User(u) => Some(
                    u.content
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        state.prompt_index = state.prompt_texts.len();
        state.check_invariants();
        state
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = prompt.into();
    }

    /// Push a user message and begin a new turn.
    pub fn push_user_message(&mut self, content: impl Into<String>) {
        // A new user turn clears transient state — images were attached to the
        // previous turn's tool results, and transient system reminders
        // (recovery prompts, "don't summarize again" nudges) belong to the
        // turn that produced them. Without the clear, one-shot reminders
        // replay into every later request until the 32-entry cap.
        self.transient_images.clear();
        self.transient_system.clear();
        // A new turn starts a fresh per-batch tool-result budget — restored
        // sessions carry stale budget state that must not leak across turns.
        self.tool_result_batch_budget = tool_result_batch_budget_for(self.context_window);
        self.tool_result_batch_used = 0;
        let msg = ConversationItem::user(content);

        // Record prompt text for rewind
        if let ConversationItem::User(u) = &msg {
            if let Some(ContentPart::Text { text }) = u.content.first() {
                self.prompt_texts.push(text.clone());
            }
        }

        // Begin turn capture
        self.turn_capture = Some(TurnCapture {
            turn_start_offset: self.conversation.len(),
            pre_replacement_messages: Vec::new(),
            compaction_occurred: false,
        });

        self.conversation.push(msg);
        self.prompt_index += 1;
        self.check_invariants();
    }

    /// Push an assistant message (with optional tool calls and reasoning content).
    ///
    /// The `usage` is stored on the message (for persistence) but NOT added
    /// to `total_usage` here — the agent loop records usage exactly once per
    /// LLM call at stream completion (run.rs / recovery.rs call
    /// `total_usage.add` before pushing). Adding here too double-counted
    /// every call (#88 audit H5: persisted session usage, the frontend
    /// usage display, and the token stats sent to the model were all ×2).
    pub fn push_assistant_message(
        &mut self,
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
        reasoning_content: Option<String>,
    ) {
        // An assistant message carrying tool calls starts a NEW tool batch —
        // reset the aggregate result budget so each round gets a fresh
        // allowance for its results.
        if !tool_calls.is_empty() {
            self.tool_result_batch_budget = tool_result_batch_budget_for(self.context_window);
            self.tool_result_batch_used = 0;
        }
        let msg = ConversationItem::Assistant(crate::core::types::AssistantMessage {
            content: content.into(),
            tool_calls,
            model: Some(self.model.clone()),
            usage: usage.clone(),
            reasoning_content,
        });

        self.conversation.push(msg);
    }

    /// Push a tool result.
    ///
    /// Results are capped against the current batch's aggregate budget (see
    /// [`ChatState::cap_to_batch_budget`]) so a parallel round cannot flood
    /// the next request with raw output.
    pub fn push_tool_result(
        &mut self,
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) {
        let content = self.cap_to_batch_budget(content.into());
        let msg = if is_error {
            ConversationItem::tool_result_error(tool_call_id, content)
        } else {
            ConversationItem::tool_result(tool_call_id, content)
        };
        self.conversation.push(msg);
    }

    /// Cap a tool result against the remaining per-batch budget, tracking
    /// what has been injected this batch. When the budget is exhausted,
    /// later results are suppressed with an explicit hint so the model
    /// adjusts its calls instead of blindly retrying a call whose output it
    /// never saw. A `budget` of 0 (uninitialized window) passes everything.
    fn cap_to_batch_budget(&mut self, content: String) -> String {
        let budget = self.tool_result_batch_budget;
        if budget == 0 {
            return content;
        }
        let remaining = budget.saturating_sub(self.tool_result_batch_used);
        let total = content.chars().count() as u64;
        if total <= remaining {
            self.tool_result_batch_used += total;
            return content;
        }
        self.tool_result_batch_used = budget;
        let shown = remaining as usize;
        if shown == 0 {
            return "[Tool result suppressed — this round's combined tool output \
                    exceeded the budget. Re-run the call with narrower parameters \
                    (offset/limit/include/max_results) if you still need this data.]"
                .to_string();
        }
        let head: String = content.chars().take(shown).collect();
        format!(
            "{head}\n\n...({} of {} chars suppressed by this round's combined \
             tool-output budget)",
            total - remaining,
            total
        )
    }

    /// Push a TRANSIENT system message.
    ///
    /// Reaches the model in the next API request (via `request_messages`)
    /// but is never persisted — reminders, task notifications, truncation
    /// prompts and hook corrections would otherwise replay stale guidance
    /// into every restored session.
    pub fn push_transient_system(&mut self, content: impl Into<String>) {
        let text = content.into();
        if text.trim().is_empty() {
            return;
        }
        // Truncate BEFORE dedup so the comparison is like-for-like: a >32k
        // entry is stored truncated, so the raw text must be truncated before
        // checking for a duplicate — otherwise a repeated large push never
        // matches and stacks toward the cap.
        let text = crate::core::str_util::truncate_tool_output(&text);
        // Identical-text dedup: repeated pushes of the SAME guidance (e.g.
        // the anti-summary reminder after every serial tool result, or a
        // repeated recovery nudge) must not stack in the tail — one
        // injection per turn carries the message. Distinct texts (varying
        // tool summaries, per-task notifications) are all preserved.
        if self
            .transient_system
            .iter()
            .any(|existing| existing == &text)
        {
            return;
        }
        if self.transient_system.len() >= MAX_TRANSIENT_SYSTEM {
            self.transient_system.remove(0);
        }
        self.transient_system.push(text);
    }

    /// Inject a TRANSIENT image (multimodal tool result — read_file reading a
    /// picture for a vision-capable main model).
    ///
    /// Reaches the model in the next API request (via `request_messages`) but
    /// is never persisted, exactly like [`ChatState::push_transient_system`].
    /// The image is cleared on the next user turn, so it is visible only to
    /// the model turn that follows the tool call that read it.
    pub fn push_transient_image(&mut self, media_type: impl Into<String>, data: impl Into<String>) {
        let data = data.into();
        if data.trim().is_empty() {
            return;
        }
        if self.transient_images.len() >= MAX_TRANSIENT_IMAGES {
            self.transient_images.remove(0);
        }
        self.transient_images.push(ContentPart::Image {
            source_type: "base64".to_string(),
            media_type: media_type.into(),
            data,
        });
    }

    /// Attach images to the CURRENT user turn (multimodal main model path).
    /// Consumed once by the next [`ChatState::request_messages`] call.
    pub fn set_initial_image_parts(&mut self, parts: Vec<ContentPart>) {
        self.initial_image_parts = parts;
    }

    /// Build the messages for an API request: the persisted conversation
    /// plus any pending transient system messages (appended at the end).
    /// Initial (send-time) images are appended as a trailing user message
    /// exactly once — the first request of the turn.
    pub fn request_messages(&mut self) -> Vec<ConversationItem> {
        // Pre-size so the transient appends below never reallocate — the
        // conversation clone itself is unavoidable (the API hands the model
        // an owned message list per request).
        let mut msgs = self.conversation.clone();
        msgs.reserve(self.transient_system.len() + self.transient_images.len() + 1);
        for text in &self.transient_system {
            msgs.push(ConversationItem::system(text));
        }
        if !self.transient_images.is_empty() {
            msgs.push(ConversationItem::user_with_parts(
                self.transient_images.clone(),
            ));
        }
        if !self.initial_image_parts.is_empty() {
            let parts = std::mem::take(&mut self.initial_image_parts);
            msgs.push(ConversationItem::user_with_parts(parts));
        }
        msgs
    }

    /// Replace the entire conversation (used during compaction).
    pub fn replace_conversation(&mut self, new_conversation: Vec<ConversationItem>) {
        // If turn capture is active, save the pre-replacement messages
        if let Some(capture) = &mut self.turn_capture {
            let start = capture.turn_start_offset;
            capture
                .pre_replacement_messages
                .extend(self.conversation[start..].to_vec());
            capture.compaction_occurred = true;
            capture.turn_start_offset = new_conversation.len();
        }

        self.last_compaction_index = Some(self.conversation.len());
        self.last_compaction_prompt_index = Some(self.prompt_index);
        self.conversation = new_conversation;
        // The tail invariant is broken — the whole history must be rewritten.
        self.persisted_upto = 0;

        // total_usage.prompt_tokens is the CUMULATIVE billed prompt tokens
        // (add()ed per LLM call) that seeds the next run's session budget.
        // It is deliberately NOT re-estimated to the compacted size here —
        // that would erase the pre-compaction spend and let a session
        // repeatedly re-approach its configured token/cost cap.
        self.check_invariants();
    }

    /// Estimate tokens for a full API request (system + conversation + tools).
    pub fn estimated_request_tokens(&self, tools: &[ToolDefinition]) -> u64 {
        crate::agent::token::estimate_request_tokens(&self.system_prompt, &self.conversation, tools)
    }

    /// Estimate tokens for the FULL request actually sent — system prompt +
    /// conversation + tools + transient system messages + the per-request
    /// tail (dynamic context / task-spec) + goal/guidance allowance.
    ///
    /// [`Self::estimated_request_tokens`] undercounts on long turns: dynamic
    /// context (project structure, skills inventory, memory injection) and
    /// transient guidance can add thousands of tokens beyond the
    /// conversation. The tiered compaction thresholds are driven by this
    /// honest number so compaction triggers before the API rejects the
    /// request, not after the prompt-too-long emergency path.
    pub fn estimated_full_request_tokens(
        &self,
        system_prompt: &str,
        tail: Option<&str>,
        tail_extra_tokens: u64,
        tools: &[ToolDefinition],
    ) -> u64 {
        let mut total =
            crate::agent::token::estimate_request_tokens(system_prompt, &self.conversation, tools);
        for text in &self.transient_system {
            total += crate::agent::token::estimate_text_tokens(text);
        }
        if let Some(tail) = tail {
            total += crate::agent::token::estimate_text_tokens(tail);
        }
        total += tail_extra_tokens;
        total
    }

    /// Record that the agent edited a file path.
    pub fn record_edited_path(&mut self, path: impl Into<String>) {
        self.agent_edited_paths.insert(path.into());
    }

    /// Record the outcome of an auto-pulled LSP diagnostics run for a file
    /// the agent just edited (see [`ChatState::auto_diagnostics`]).
    pub fn record_auto_diagnostics(&mut self, path: impl Into<String>, clean: bool) {
        self.auto_diagnostics.insert(path.into(), clean);
    }

    /// Get a snapshot of the conversation for API request building.
    pub fn conversation_snapshot(&self) -> &[ConversationItem] {
        &self.conversation
    }

    /// Map tool-call id → (is_error, content) of its executed result.
    ///
    /// Built from the executed `ToolResult` items already in the
    /// conversation (the authoritative success/failure flag the dispatcher
    /// stored). The agent loop uses it to tell "verification succeeded"
    /// apart from "verification ran but failed" — a non-zero test/build
    /// must not count as verified. The content lets the loop inspect
    /// results like `lsp` diagnostics, which report errors as SUCCESS with
    /// error text in the content.
    pub fn tool_results_by_call_id(&self) -> std::collections::HashMap<String, (bool, String)> {
        self.conversation
            .iter()
            .filter_map(|item| match item {
                ConversationItem::ToolResult(tr) => {
                    Some((tr.tool_call_id.clone(), (tr.is_error, tr.content.clone())))
                }
                _ => None,
            })
            .collect()
    }

    /// Recall (delete) a user message and everything that followed it.
    ///
    /// Matches the last user message whose plain text equals `content`,
    /// truncates the conversation at that point, and rewinds usage/prompt
    /// bookkeeping. Returns `false` when no matching message exists.
    pub fn truncate_from_user_message(&mut self, content: &str) -> bool {
        let mut target: Option<usize> = None;
        for (i, item) in self.conversation.iter().enumerate().rev() {
            if let ConversationItem::User(u) = item {
                let text: String = u
                    .content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if text == content {
                    target = Some(i);
                    break;
                }
            }
        }

        let Some(idx) = target else { return false };

        self.conversation.truncate(idx);
        self.persisted_upto = 0;
        self.prompt_texts = self
            .conversation
            .iter()
            .filter_map(|item| match item {
                ConversationItem::User(u) => Some(
                    u.content
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        self.prompt_index = self.prompt_texts.len();
        self.turn_capture = None;
        self.check_invariants();
        true
    }

    /// Repair tool-call integrity in the conversation.
    ///
    /// Two failure classes are repaired before every request build:
    /// 1. Dangling calls — assistant tool calls whose IDs have no matching
    ///    `ToolResult` (crash between push and result, user cancel). A
    ///    synthetic error result is inserted so the LLM receives a
    ///    well-formed conversation.
    /// 2. Duplicate/orphan entries — the same `call_id` declared twice
    ///    (streams occasionally repeat a call id), duplicate tool results,
    ///    or results for ids never declared. OpenAI-compatible APIs reject
    ///    these with HTTP 400 "Duplicate 'call_id'", so duplicates are
    ///    dropped (first declaration / first result wins).
    ///
    /// Idempotent — calling it on a clean conversation is a cheap no-op.
    pub fn repair_dangling_tool_calls(&mut self) {
        let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut answered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut dropped = 0usize;

        // Pass 1 — dedupe declarations and results, drop orphans.
        let mut cleaned: Vec<ConversationItem> = Vec::with_capacity(self.conversation.len());
        for item in std::mem::take(&mut self.conversation) {
            match item {
                ConversationItem::Assistant(mut a) => {
                    let mut kept: Vec<ToolCall> = Vec::with_capacity(a.tool_calls.len());
                    for tc in a.tool_calls {
                        if declared.insert(tc.id.clone()) {
                            kept.push(tc);
                        } else {
                            dropped += 1;
                        }
                    }
                    a.tool_calls = kept;
                    cleaned.push(ConversationItem::Assistant(a));
                }
                ConversationItem::ToolResult(tr) => {
                    if declared.contains(&tr.tool_call_id)
                        && answered.insert(tr.tool_call_id.clone())
                    {
                        cleaned.push(ConversationItem::ToolResult(tr));
                    } else {
                        dropped += 1;
                    }
                }
                other => cleaned.push(other),
            }
        }

        // Pass 2 — synthetic results for calls still missing an answer.
        let mut insertions: Vec<(usize, ConversationItem)> = Vec::new();
        for (i, item) in cleaned.iter().enumerate() {
            if let ConversationItem::Assistant(a) = item {
                let dangling: Vec<&ToolCall> = a
                    .tool_calls
                    .iter()
                    .filter(|tc| !answered.contains(&tc.id))
                    .collect();
                if dangling.is_empty() {
                    continue;
                }
                // Insert synthetic tool results right after this assistant message.
                for (offset, tc) in dangling.iter().enumerate() {
                    let synthetic = ConversationItem::tool_result_error(
                        &tc.id,
                        "[Tool execution was interrupted — the process may have been \
                         cancelled or crashed. The tool did not produce a result.]",
                    );
                    insertions.push((i + 1 + offset, synthetic));
                }
            }
        }

        // Apply insertions in reverse order so indices remain valid.
        let repaired_count = insertions.len();
        for (idx, item) in insertions.into_iter().rev() {
            cleaned.insert(idx, item);
        }
        self.conversation = cleaned;
        self.check_invariants();
        if dropped == 0 && repaired_count == 0 {
            return;
        }
        // Synthetic rows do not exist in the database — full rewrite on
        // the next persist (or an append from the pre-repair checkpoint,
        // see `from_history`).
        self.persisted_upto = 0;

        info!(
            repaired_count,
            dropped,
            "Repaired dangling or duplicate tool calls in conversation"
        );
    }
}
