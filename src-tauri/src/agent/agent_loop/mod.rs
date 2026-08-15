//! Agent loop — the core while(true) execution cycle.
//!
//! Implements a 7-phase loop:
//!
//! 1. **Context management** — check token budget, trigger compaction if needed
//! 2. **Build request** — assemble system prompt, messages, tool definitions
//! 3. **LLM call** — stream the model response
//! 4. **Parse response** — extract text content and tool calls
//! 5. **Tool execution** — run tools with PreToolUse hooks (gate) and PostToolUse (observe)
//! 6. **Append results** — add tool results to conversation
//! 7. **Loop decision** — continue if tool calls were made, else stop
//!
//! Submodules:
//! - `run` — the `run()` entry point
//! - `tool_batch` — batch tool execution with hook gates
//! - `reflexion` — self-critique after tool execution rounds
//! - `evaluator` — independent evaluator review (EvaluatorQa mode)
//! - `recovery` — doom-loop, empty-response, and soft-termination recovery

pub(crate) mod evaluator;
mod gates;
mod recovery;
mod reflexion;
mod run;
mod tool_batch;
mod verification;

use crate::agent::compaction::Compactor;
use crate::agent::context::ContextBuilder;
use crate::core::error::{AppError, AppResult};
use crate::core::stream::emit_stream;
use crate::core::types::{StreamEvent, TokenUsage, ToolCall};
use crate::hooks::HookExecutor;
use crate::llm::client::LlmClient;
use crate::llm::models::ModelCatalog;
use crate::llm::provider::ChunkStream;
use crate::llm::sampler::DoomLoopDetector;
use crate::llm::streaming::{StreamChunk, ToolCallAccumulator};
use crate::observability::usage::SessionUsageTracker;
use crate::tools::dispatch::ToolDispatcher;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Emit a coalesced text delta event to the frontend.
fn emit_text_delta(app: &AppHandle, turn_id: &str, text: String) {
    emit_stream(
        app,
        StreamEvent::TextDelta {
            turn_id: turn_id.to_string(),
            text,
        },
    );
}

/// Emit a coalesced reasoning delta event to the frontend.
fn emit_reasoning_delta(app: &AppHandle, turn_id: &str, text: String) {
    emit_stream(
        app,
        StreamEvent::ReasoningDelta {
            turn_id: turn_id.to_string(),
            text,
        },
    );
}

/// The reasoning pattern the agent uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AgentLoopMode {
    /// Standard ReAct loop — think, act, observe, repeat.
    #[default]
    Standard,
    /// Plan-Execute — first generate a plan, then execute each step.
    PlanExecute,
    /// Reflexion — self-critique after each tool execution round.
    Reflexion,
    /// Coordinator — manage parallel subagent workers.
    Coordinator,
    /// Evaluator-QA — a generator turn followed by an INDEPENDENT evaluator
    /// subagent (isolated context, read-only + verification tools, skeptical
    /// prompt). Failing reviews feed back into the generator for another
    /// fix round, up to a bounded number of review rounds.
    EvaluatorQa,
    /// Goal — the generator keeps working until an INDEPENDENT evaluator
    /// confirms the session goal is achieved (or the budget is exhausted).
    /// Same evaluator machinery as EvaluatorQa, but the review criterion is
    /// the session goal (<current-goal>) instead of the last user prompt.
    Goal,
}

impl AgentLoopMode {
    /// Stable string identifier for logging and event payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::PlanExecute => "plan_execute",
            Self::Reflexion => "reflexion",
            Self::Coordinator => "coordinator",
            Self::EvaluatorQa => "evaluator_qa",
            Self::Goal => "goal",
        }
    }

    /// Additional system prompt text for this mode.
    pub fn system_prompt_suffix(&self) -> &'static str {
        match self {
            Self::Standard => {
                "\n\n## Interaction Style\n\
                Work in the open: pair tool calls with a short, plain sentence\n\
                about what you are doing — not why you chose the tool, and never\n\
                \"per my guidelines\". Keep notes to one line; routine reads and\n\
                obvious follow-ups need no narration. After a tool completes,\n\
                state what you found only when it changes the next step."
            }
            Self::PlanExecute => {
                "\n\n## Reasoning Mode: Plan-Execute\n\
                Investigate first, then write a defensible plan: read the \
                files the task touches before planning, then present \
                BACKGROUND, APPROACH (chosen vs rejected + why), KEY FILES \
                (only ones you actually read), numbered STEPS, OUT OF SCOPE, \
                ASSUMPTIONS, and how you will VERIFY the result. Then execute \
                each step in order, checking off completed steps as you go."
            }
            Self::Reflexion => {
                "\n\n## Reasoning Mode: Reflexion\n\
                After each action you take, briefly reflect on whether the action\n\
                was effective and what you should do differently next time.\n\
                Use these reflections to improve your approach."
            }
            Self::Coordinator => {
                "\n\n## Reasoning Mode: Coordinator\n\
                You orchestrate complex tasks with parallel subagents. Drive \
                the four-phase workflow, advancing through \
                <coordinator_phase> as you go:\n\
                1. RESEARCH — spawn explore workers to map the relevant \
                code and report findings (wait for their results).\n\
                2. SYNTHESIS — combine the findings into an implementation \
                design.\n\
                3. IMPLEMENTATION — delegate the code changes to general \
                workers (declare their write paths to avoid conflicts), then \
                integrate their results.\n\
                4. VERIFICATION — spawn an evaluator worker to verify the \
                integrated work before you conclude.\n\
                The phase machine advances automatically when each phase's \
                workers finish. Synthesize results from subagents into a \
                coherent response; never claim work a worker did not report."
            }
            Self::EvaluatorQa => {
                "\n\n## Reasoning Mode: Evaluator-QA\n\
                You are the generator in a generate-review loop. Complete the\n\
                task with file tools as usual. When you finish, an INDEPENDENT\n\
                evaluator subagent reviews your work against the task (isolated\n\
                context, read-only + verification tools, skeptical by design).\n\
                If the review returns FAIL, address every finding precisely\n\
                (exact file paths, line numbers, repro steps) and fix the code —\n\
                do NOT argue with the review or claim it is wrong without\n\
                evidence. Iterate until the review passes or the loop cap is\n\
                reached. Never mention the reviewer as a subagent to the user."
            }
            Self::Goal => {
                "\n\n## Reasoning Mode: Goal\n\
                You work toward the session goal (<current-goal>) until it is\n\
                achieved. When you believe the goal is done, do NOT stop on\n\
                your own judgment alone: an INDEPENDENT evaluator will check\n\
                the goal against the actual state and return PASS or FAIL. If\n\
                it returns FAIL, address every finding precisely (exact paths,\n\
                line numbers, repro steps) and keep working until it passes or\n\
                the budget is exhausted. Never mention the reviewer as a\n\
                subagent to the user."
            }
        }
    }
}

/// Configuration for the agent loop.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    pub max_turns: u32,
    pub auto_compact_threshold_percent: u8,
    pub temperature: Option<f32>,
    /// The reasoning pattern to use.
    pub mode: AgentLoopMode,
    /// Maximum consecutive permission denials before terminating the loop.
    pub max_consecutive_denials: u32,
    /// deepseek-native: reasoning effort level for DeepSeek thinking mode.
    /// Set to Some("medium"/"high"/"max") to enable; None to disable.
    /// Only affects requests sent to the deepseek provider.
    pub reasoning_effort: Option<String>,
    /// Session-level total token limit (0 = unlimited).
    pub session_token_limit: u64,
    /// Session-level total cost limit in USD (0.0 = unlimited).
    pub session_cost_limit: f64,
    /// Wall-clock timeout for one loop invocation in seconds
    /// (None = unlimited).
    pub run_timeout_secs: Option<u64>,
    /// Optional per-turn OUTPUT token cap (None = unlimited). When set,
    /// every request carries this max_tokens and truncation recovery does
    /// NOT escalate past it (the cap is the user's explicit intent).
    pub turn_output_token_limit: Option<u64>,
    /// Agent deny chain (own definition denies + inherited ancestor
    /// denies) carried by this loop. Spawned children — including the
    /// independent evaluator — must inherit it (M9 hard veto).
    pub agent_deny_rules: Vec<String>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 50,
            auto_compact_threshold_percent: 80,
            temperature: None,
            mode: AgentLoopMode::Standard,
            max_consecutive_denials: 5,
            reasoning_effort: Some("max".into()),
            session_token_limit: 0,
            session_cost_limit: 0.0,
            run_timeout_secs: None,
            turn_output_token_limit: None,
            agent_deny_rules: Vec::new(),
        }
    }
}

/// The main agent loop — runs the conversation cycle.
pub struct AgentLoop {
    llm_client: LlmClient,
    tool_dispatcher: ToolDispatcher,
    compactor: Compactor,
    context_builder: ContextBuilder,
    config: AgentLoopConfig,
    /// Hook executor — gates tools with PreToolUse, observes with PostToolUse.
    hook_executor: HookExecutor,
    /// Usage tracker — records token usage per turn and per tool.
    usage_tracker: Option<SessionUsageTracker>,
    /// Shared interjection registry — per-run transient guidance merged into
    /// the dynamic context (todo nudges, background subagent signals).
    interjections:
        Option<Arc<tokio::sync::Mutex<crate::agent::interjection::InterjectionRegistry>>>,
    /// Consecutive requests' cache prefix shapes — `(previous, current)` —
    /// lets us DIAGNOSE why a DeepSeek prefix-cache miss happened (system
    /// prompt changed? tool schema changed? cache expired?). The PREVIOUS
    /// shape is kept because `record_request_shape` runs before the request
    /// and `diagnose_cache_miss` after it: comparing the request against
    /// itself (the old single-slot design) always reported "evicted/
    /// expired" and made the system/tools-changed branches dead code.
    /// Plain std Mutex: held for nanoseconds from async code — a tokio
    /// Mutex's `blocking_lock()` PANICS inside the runtime (crash reports
    /// 0.1.8/0.1.9: "Cannot block the current thread from within a
    /// runtime").
    cache_shape: std::sync::Mutex<Option<(Option<CacheShape>, CacheShape)>>,
    /// Model catalog — source of truth for per-model pricing (cost guard).
    /// Owned (not shared) so each loop gets a fresh, correctly-populated
    /// builtin catalog; runtime-registered custom models fall back to the
    /// heuristic via `ModelCatalog::pricing`.
    model_catalog: ModelCatalog,
}

/// FNV-1a 64-bit — fast, deterministic hash for cache-shape fingerprinting.
fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fingerprint of the stable request prefix (system prompt + tool schemas).
/// Comparing consecutive fingerprints explains cache misses.
#[derive(Debug, Clone, Copy)]
pub struct CacheShape {
    system_hash: u64,
    tools_hash: u64,
}

impl AgentLoop {
    /// Fingerprint the current request prefix and store it for the next
    /// miss-diagnosis. Call right before the LLM request is built.
    pub fn record_request_shape(
        &self,
        system_prompt: &str,
        tool_defs: &[crate::core::types::ToolDefinition],
    ) {
        let tools_bytes = serde_json::to_vec(tool_defs).unwrap_or_default();
        let shape = CacheShape {
            system_hash: fnv64(system_prompt.as_bytes()),
            tools_hash: fnv64(&tools_bytes),
        };
        let mut guard = self.cache_shape.lock().unwrap_or_else(|e| e.into_inner());
        // Slide the window: the previous request's shape becomes `prev`, so
        // the next miss diagnosis compares two CONSECUTIVE requests instead
        // of the request against itself.
        let previous = guard.as_ref().map(|(_, current)| *current);
        *guard = Some((previous, shape));
    }

    /// Diagnose why the last request's prefix-cache missed, by comparing the
    /// shape of the previous request with the current one. Returns a
    /// human-readable reason, or `None` when there is nothing to report
    /// (first request, no miss, or shape stable).
    pub fn diagnose_cache_miss(&self, cache_miss_tokens: u64) -> Option<String> {
        let (prev, current) = (*self.cache_shape.lock().unwrap_or_else(|e| e.into_inner()))?;
        classify_cache_miss(
            prev.map(|s| (s.system_hash, s.tools_hash)),
            current.system_hash,
            current.tools_hash,
            cache_miss_tokens,
        )
    }
}

/// Pure classification of a prefix-cache miss — unit-testable without an
/// AgentLoop instance.
fn classify_cache_miss(
    prev: Option<(u64, u64)>,
    system_hash: u64,
    tools_hash: u64,
    cache_miss_tokens: u64,
) -> Option<String> {
    if cache_miss_tokens == 0 {
        return None;
    }
    let Some((prev_system, prev_tools)) = prev else {
        // First LLM call of the session — misses are expected.
        return None;
    };
    if system_hash != prev_system {
        return Some(format!(
            "prefix-cache miss ({cache_miss_tokens} tokens): the system prompt changed since the \
             previous request (stable-prefix rule broken — check mode switches or user-profile edits)"
        ));
    }
    if tools_hash != prev_tools {
        return Some(format!(
            "prefix-cache miss ({cache_miss_tokens} tokens): the tool schema changed since the \
             previous request (MCP server connected/disconnected or tool set changed)"
        ));
    }
    // Shape identical yet missed — cache eviction / expiry / restart.
    Some(format!(
        "prefix-cache miss ({cache_miss_tokens} tokens): shape unchanged — the cached prefix was \
         evicted or expired (long idle gap or server-side eviction)"
    ))
}

/// Drop tool calls that never received a terminating `ToolCallEnd` —
/// a mid-stream error leaves their arguments truncated, and dispatching
/// them (e.g. an `edit_file` with a cut-off path) would execute garbage.
/// Calls that completed before the error keep their full arguments.
fn drop_unfinished_tool_calls(accumulators: &mut [ToolCallAccumulator], completed: &[bool]) {
    for (i, acc) in accumulators.iter_mut().enumerate() {
        if !completed.get(i).copied().unwrap_or(false) {
            acc.name.clear();
            acc.arguments.clear();
        }
    }
}

/// Whether a stream chunk is a provider-level failure that must abort the
/// turn. Returns the error message when the chunk is fatal.
///
/// A `StreamChunk::Error` means the provider aborted the response (SSE
/// buffer overflow, `response.failed`, explicit stream error). Partial
/// output that already reached the frontend stays visible, but the turn
/// must NOT accept it as a successful result — the loop's error paths
/// (retry classification, error event, exit housekeeping) only engage when
/// `parse_stream` returns Err (audit H12).
fn fatal_stream_error(chunk: &StreamChunk) -> Option<String> {
    match chunk {
        StreamChunk::Error { message } => Some(message.clone()),
        _ => None,
    }
}

impl AgentLoop {
    /// Create a new agent loop with the given dependencies.
    pub fn new(
        llm_client: LlmClient,
        tool_dispatcher: ToolDispatcher,
        compactor: Compactor,
        context_builder: ContextBuilder,
        config: AgentLoopConfig,
        hook_executor: HookExecutor,
    ) -> Self {
        Self {
            llm_client,
            tool_dispatcher,
            compactor,
            context_builder,
            config,
            hook_executor,
            usage_tracker: None,
            interjections: None,
            cache_shape: std::sync::Mutex::new(None),
            model_catalog: ModelCatalog::new(),
        }
    }

    /// Set the usage tracker for recording token usage.
    pub fn with_usage_tracker(mut self, tracker: SessionUsageTracker) -> Self {
        self.usage_tracker = Some(tracker);
        self
    }

    /// Set the shared interjection registry for transient per-turn guidance.
    pub fn with_interjections(
        mut self,
        registry: Arc<tokio::sync::Mutex<crate::agent::interjection::InterjectionRegistry>>,
    ) -> Self {
        self.interjections = Some(registry);
        self
    }

    /// Register a transient interjection into the run's registry (if wired).
    pub async fn register_interjection(
        &self,
        interjection: crate::agent::interjection::Interjection,
    ) {
        if let Some(ref registry) = self.interjections {
            registry.lock().await.register(interjection);
        }
    }

    /// Collect the merged per-turn guidance from the interjection registry.
    ///
    /// Called at every request build (not once per run) so interjections
    /// registered mid-run (todo gates, background subagent signals) reach
    /// the model in the very next turn. Each fragment is returned
    /// separately — the loop renders every one as its own `<user_query>`
    /// message so the model can address sources independently. Collected
    /// interjections are consumed (one-shot guidance is never replayed).
    pub async fn interjection_guidance(&self) -> Vec<(String, String)> {
        match &self.interjections {
            Some(registry) => registry.lock().await.collect_fragments(),
            None => Vec::new(),
        }
    }

    /// Parse the SSE stream into accumulated text, tool calls, and usage.
    ///
    /// Deltas are forwarded straight through [`emit_stream`] — the backend
    /// no longer paces text. The frontend reducer accumulates by seq and
    /// smooths the reveal per frame; non-text events preserve ordering
    /// because every event carries its own sequence number.
    async fn parse_stream(
        &self,
        stream: &mut ChunkStream,
        app: &AppHandle,
        turn_id: &str,
        session_id: &str,
        cancellation_token: &CancellationToken,
        trace_id: Option<String>,
    ) -> AppResult<(
        String,
        String,
        Vec<ToolCall>,
        String,
        TokenUsage,
        Option<crate::llm::sampler::DoomLoopSignal>,
    )> {
        let mut accumulated_text = String::new();
        let mut accumulated_reasoning = String::new();
        let mut tool_call_accumulators: Vec<ToolCallAccumulator> = Vec::new();
        // Whether each accumulator index received its terminating
        // `ToolCallEnd` — only calls marked complete may survive a
        // mid-stream error (a truncated call must never reach dispatch with
        // half-built arguments).
        let mut tool_call_completed: Vec<bool> = Vec::new();
        let mut finish_reason = String::new();
        let mut usage = TokenUsage::default();
        // No backend pacing: every visible delta is emitted as it arrives —
        // the frontend typewriter is the single smoothing layer (double
        // pacing duplicated throttle work and made the stream feel laggy).
        // Hides provider tool-call protocol blocks (`<tool_calls>...` / DSML)
        // from the live stream so they never flash in the UI; storage is
        // sanitized separately at finalize (strip_tool_call_markup).
        let mut markup_guard = crate::core::str_util::StreamMarkupGuard::new();
        // Reasoning shares the same protocol-block hiding: DeepSeek's
        // thinking mode often drafts tool-call XML inside reasoning_content
        // before emitting the real structured calls. Without a separate
        // guard, that draft leaks into the live reasoning stream.
        let mut reasoning_markup_guard = crate::core::str_util::StreamMarkupGuard::new();
        let mut doom_detector = DoomLoopDetector::new();
        let mut doom_signal: Option<crate::llm::sampler::DoomLoopSignal> = None;

        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            if cancellation_token.is_cancelled() {
                return Err(AppError::Cancelled);
            }

            match chunk_result {
                Ok(chunk) => {
                    // Handle text/reasoning with coalescing — skip flush for these
                    match &chunk {
                        StreamChunk::TextDelta { text } => {
                            accumulated_text.push_str(text);
                            let visible = markup_guard.visible(&accumulated_text, text);
                            if doom_signal.is_none() {
                                let joined = if visible.after.is_empty() {
                                    visible.before.to_string()
                                } else {
                                    format!("{}{}", visible.before, visible.after)
                                };
                                if let Some(signal) = doom_detector.push(&joined) {
                                    doom_signal = Some(signal);
                                }
                            }
                            if !visible.before.is_empty() {
                                emit_text_delta(app, turn_id, visible.before.to_string());
                            }
                            if !visible.after.is_empty() {
                                emit_text_delta(app, turn_id, visible.after.to_string());
                            }
                            continue;
                        }
                        // deepseek-native: reasoning content delta (thinking mode)
                        StreamChunk::ReasoningDelta { text } => {
                            accumulated_reasoning.push_str(text);
                            let visible =
                                reasoning_markup_guard.visible(&accumulated_reasoning, text);
                            if !visible.before.is_empty() {
                                emit_reasoning_delta(app, turn_id, visible.before.to_string());
                            }
                            if !visible.after.is_empty() {
                                emit_reasoning_delta(app, turn_id, visible.after.to_string());
                            }
                            continue;
                        }
                        _ => {}
                    }

                    if let Some(message) = fatal_stream_error(&chunk) {
                        emit_stream(
                            app,
                            StreamEvent::Error {
                                turn_id: turn_id.to_string(),
                                session_id: session_id.to_string(),
                                message: message.clone(),
                                trace_id: trace_id.clone(),
                            },
                        );
                        drop_unfinished_tool_calls(
                            &mut tool_call_accumulators,
                            &tool_call_completed,
                        );
                        return Err(AppError::LlmStreaming(message));
                    }

                    match chunk {
                        StreamChunk::ToolCallStart { index, id, name } => {
                            while tool_call_accumulators.len() <= index {
                                tool_call_accumulators.push(ToolCallAccumulator::default());
                                tool_call_completed.push(false);
                            }
                            tool_call_accumulators[index].id = id.clone();
                            tool_call_accumulators[index].name = name.clone();

                            emit_stream(
                                app,
                                StreamEvent::ToolCallStart {
                                    turn_id: turn_id.to_string(),
                                    call_id: id,
                                    name,
                                },
                            );
                        }
                        StreamChunk::ToolCallDelta { index, arguments } => {
                            while tool_call_accumulators.len() <= index {
                                tool_call_accumulators.push(ToolCallAccumulator::default());
                                tool_call_completed.push(false);
                            }
                            tool_call_accumulators[index].arguments.push_str(&arguments);

                            let call_id = tool_call_accumulators[index].id.clone();
                            emit_stream(
                                app,
                                StreamEvent::ToolCallDelta {
                                    turn_id: turn_id.to_string(),
                                    call_id,
                                    arguments,
                                },
                            );
                        }
                        StreamChunk::ToolCallEnd { index } => {
                            if index < tool_call_accumulators.len() {
                                if index < tool_call_completed.len() {
                                    tool_call_completed[index] = true;
                                }
                                let acc = &tool_call_accumulators[index];
                                if !acc.name.is_empty() {
                                    info!(
                                        tool = %acc.name,
                                        args_len = acc.arguments.len(),
                                        "Tool call finalized from stream"
                                    );
                                }
                            }
                        }
                        StreamChunk::Usage { usage: u } => {
                            usage.add(&u);
                        }
                        StreamChunk::Finish { reason } => {
                            finish_reason = reason;
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    // Transport-level stream failure (connection reset /
                    // body read error mid-response). Same rule as the
                    // provider-level `StreamChunk::Error` (audit H12): the
                    // partial response must NOT be accepted as a successful
                    // turn. Previously this only dropped the unfinished tool
                    // calls and kept consuming — a proxy that dropped the
                    // connection mid-stream left a half answer treated as
                    // final, skipping every verification gate.
                    emit_stream(
                        app,
                        StreamEvent::Error {
                            turn_id: turn_id.to_string(),
                            session_id: session_id.to_string(),
                            message: e.to_string(),
                            trace_id: trace_id.clone(),
                        },
                    );
                    drop_unfinished_tool_calls(&mut tool_call_accumulators, &tool_call_completed);
                    return Err(e);
                }
            }
        }

        // Finalize accumulated tool calls
        let accumulated_tool_calls: Vec<ToolCall> =
            crate::llm::streaming::ToolCallAccumulator::dedupe_tool_calls_by_id(
                tool_call_accumulators
                    .into_iter()
                    .filter(|tc| !tc.name.is_empty())
                    .map(|tc| ToolCall {
                        id: if tc.id.is_empty() {
                            crate::core::ids::tool_call_id()
                        } else {
                            tc.id
                        },
                        name: tc.name,
                        arguments: if tc.arguments.is_empty() {
                            "{}".to_string()
                        } else {
                            tc.arguments
                        },
                    })
                    .collect(),
            );

        Ok((
            accumulated_text,
            accumulated_reasoning,
            accumulated_tool_calls,
            finish_reason,
            usage,
            doom_signal.or_else(|| doom_detector.take_signal()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv64_is_deterministic_and_sensitive() {
        assert_eq!(fnv64(b"hello"), fnv64(b"hello"));
        assert_ne!(fnv64(b"hello"), fnv64(b"hello!"));
        assert_ne!(fnv64(b""), fnv64(b" "));
    }

    #[test]
    fn cache_diagnosis_classifies_system_change() {
        let reason = classify_cache_miss(
            Some((fnv64(b"old"), fnv64(b"tools"))),
            fnv64(b"new"),
            fnv64(b"tools"),
            5000,
        );
        let reason = reason.expect("must diagnose");
        assert!(reason.contains("system prompt changed"), "{reason}");
    }

    #[test]
    fn cache_diagnosis_classifies_tools_change() {
        let reason = classify_cache_miss(
            Some((fnv64(b"sys"), fnv64(b"old-tools"))),
            fnv64(b"sys"),
            fnv64(b"new-tools"),
            5000,
        );
        let reason = reason.expect("must diagnose");
        assert!(reason.contains("tool schema changed"), "{reason}");
    }

    #[test]
    fn cache_diagnosis_classifies_eviction() {
        let reason = classify_cache_miss(
            Some((fnv64(b"same"), fnv64(b"same"))),
            fnv64(b"same"),
            fnv64(b"same"),
            100,
        );
        let reason = reason.expect("must diagnose");
        assert!(reason.contains("evicted or expired"), "{reason}");
    }

    #[test]
    fn cache_diagnosis_silent_on_hit_or_first() {
        assert!(classify_cache_miss(
            Some((fnv64(b"s"), fnv64(b"s"))),
            fnv64(b"s"),
            fnv64(b"s"),
            0
        )
        .is_none());
        assert!(classify_cache_miss(None, fnv64(b"s"), fnv64(b"s"), 1).is_none());
    }

    #[test]
    fn stream_error_drops_unfinished_tool_calls_only() {
        // A truncated stream must not dispatch half-built tool calls — only
        // calls that received their ToolCallEnd survive the error.
        let mut accs = vec![
            ToolCallAccumulator {
                id: "call_1".into(),
                name: "edit_file".into(),
                arguments: r#"{"path":"src/a.rs","old":"o"#.into(),
            },
            ToolCallAccumulator {
                id: "call_2".into(),
                name: "bash".into(),
                arguments: r#"{"command":"cargo test"}"#.into(),
            },
            ToolCallAccumulator {
                id: "call_3".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"b.rs"}"#.into(),
            },
        ];
        // Only call_2 completed (index 1).
        let completed = vec![false, true, false];
        drop_unfinished_tool_calls(&mut accs, &completed);
        assert!(accs[0].name.is_empty() && accs[0].arguments.is_empty());
        assert!(accs[2].name.is_empty() && accs[2].arguments.is_empty());
        assert_eq!(accs[1].name, "bash");
        assert!(accs[1].arguments.contains("cargo test"));
    }

    #[test]
    fn stream_error_chunk_is_fatal_for_the_turn() {
        // A provider error chunk must abort the turn — partial output is
        // never a successful result (audit H12). Only the Error variant is
        // fatal; every other chunk passes through.
        assert_eq!(
            fatal_stream_error(&StreamChunk::Error {
                message: "response.failed".into()
            })
            .as_deref(),
            Some("response.failed")
        );
        assert!(fatal_stream_error(&StreamChunk::TextDelta {
            text: "partial".into()
        })
        .is_none());
        assert!(fatal_stream_error(&StreamChunk::Usage {
            usage: TokenUsage::default()
        })
        .is_none());
        assert!(fatal_stream_error(&StreamChunk::Finish {
            reason: "length".into()
        })
        .is_none());
    }

    #[test]
    fn agent_loop_mode_strings_are_stable() {
        let modes = [
            (AgentLoopMode::Standard, "standard"),
            (AgentLoopMode::PlanExecute, "plan_execute"),
            (AgentLoopMode::Reflexion, "reflexion"),
            (AgentLoopMode::Coordinator, "coordinator"),
            (AgentLoopMode::EvaluatorQa, "evaluator_qa"),
            (AgentLoopMode::Goal, "goal"),
        ];
        for (mode, expected) in modes {
            assert_eq!(mode.as_str(), expected);
        }
    }
}
