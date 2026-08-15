//! Run-loop shared state — counters, phase decisions, and the mutable loop
//! state bundle (`LoopState`) for the orchestrator and its phase helpers.

use crate::agent::agent_loop::verification::VerificationTier;
use crate::agent::budget::BudgetTracker;
use crate::agent::intent::IntentResult;
use crate::agent::system_reminder::ReminderState;
use crate::core::error::AppResult;
use crate::core::types::tool::{ToolCall, ToolDefinition};
use crate::core::types::TokenUsage;
use crate::llm::provider::LlmRequest;
use crate::llm::sampler::DoomLoopSignal;
use std::collections::HashMap;
use std::path::PathBuf;

/// How many times a doom-loop detection may resample before the model is
/// forced to conclude (mirrors DoomLoopRecoveryPolicy.max_retries).
pub(super) const MAX_DOOM_RETRIES: u32 = 2;
/// PreLLMCall denials are capped too — a hook that blocks every request
/// must not spin the loop forever (no other cap existed).
pub(super) const MAX_PRE_LLM_DENIALS: u32 = 3;

/// Outcome of one phase method: keep the loop running or exit with the
/// given loop result. Every exit funnels into the single housekeeping path
/// in `run_inner` (rewind snapshot + AgentLoopEnd hook never skipped).
pub(super) enum LoopAction {
    Continue,
    Break(AppResult<String>),
}

/// Mutable counters shared by the stop-path gate chain (Phase 5c–5e'').
/// Bundled so the chain's state is explicit and `run_inner` stays lean.
#[derive(Default)]
pub(super) struct StopGateCounters {
    pub(super) todo_gate_fires: u32,
    pub(super) stop_nudges: u32,
    pub(super) narration_fires: u32,
    pub(super) bg_nudge_fires: u32,
    pub(super) verify_gate_fires: u32,
    pub(super) plan_gate_fires: u32,
    pub(super) stop_hook_fires: u32,
    pub(super) evaluator_rounds: u32,
    pub(super) goal_checks: u32,
}

/// All mutable state the loop carries across phases and iterations.
///
/// Previously `run_inner` declared 20+ `let mut` locals and threaded them
/// through phase calls as 8–31 individual `&mut` arguments — the mechanical
/// plumbing that made the loop feel unmanageable. Bundled here, phases take
/// `&mut LoopState` and destructure only the fields they touch, so a phase
/// signature says what it changes and `run_inner` reads like orchestration,
/// not variable juggling. Run-scoped constants (app, session_id, turn_id,
/// model, …) stay as explicit arguments — they never mutate.
pub(super) struct LoopState {
    // ── Request building (Phase 1–2) ────────────────────────────────
    /// The per-turn dynamic context (git, time, structure, memory) —
    /// refreshed each iteration so a long turn reasons against CURRENT
    /// state, not a run-start snapshot. Rebuilt from this + `tail_suffix`
    /// whenever `augmented_message` needs refreshing.
    pub(super) dynamic_ctx: Option<String>,
    /// Everything appended after the dynamic context — the task-spec and the
    /// UserMessage-hook guidance. Built once per turn; re-appended over the
    /// fresh `dynamic_ctx` on each iteration.
    pub(super) tail_suffix: Option<String>,
    /// The combined per-request tail (`dynamic_ctx` + `tail_suffix`),
    /// injected as a trailing user message. Recomposed by
    /// [`compose_augmented_message`].
    pub(super) augmented_message: Option<String>,
    pub(super) tool_defs: Vec<ToolDefinition>,
    pub(super) system_prompt: String,
    pub(super) request: Option<LlmRequest>,
    // ── Intent (Phase 0.5 / 1; goal drift in Phase 5) ───────────────
    pub(super) intent_result: IntentResult,
    pub(super) goal_drift_count: u32,
    // ── Parsed LLM output (Phase 4) ─────────────────────────────────
    pub(super) accumulated_text: String,
    pub(super) accumulated_reasoning: String,
    pub(super) accumulated_tool_calls: Vec<ToolCall>,
    pub(super) finish_reason: String,
    pub(super) usage: TokenUsage,
    pub(super) doom_signal: Option<DoomLoopSignal>,
    // ── LLM retry / recovery (Phase 3–4) ────────────────────────────
    pub(super) max_tokens_override: Option<u64>,
    pub(super) prompt_too_long_retries: u32,
    /// Retries for an API-side `MaxTokensExceeded` REJECTION — the provider
    /// rejected our max_tokens as above its ceiling. Recovery clamps to the
    /// reported ceiling (never escalates). Split from the truncation counter
    /// so the two recovery kinds never consume each other's budget (audit:
    /// the shared counter let rejection retries starve truncation recovery).
    pub(super) max_tokens_reject_retries: u32,
    /// Retries for OUTPUT truncation (`finish_reason=length` mid-tool-call or
    /// mid-prose). Recovery escalates max_tokens up the ladder, capped by the
    /// user-set `turn_output_token_limit` when present.
    pub(super) max_tokens_truncation_retries: u32,
    pub(super) pre_llm_denials: u32,
    pub(super) doom_retries: u32,
    pub(super) system_resource_retries: u32,
    // ── Tool execution / verification (Phase 5) ─────────────────────
    pub(super) run_has_tool_activity: bool,
    pub(super) consecutive_denials: u32,
    pub(super) edited_files: Vec<PathBuf>,
    /// A successful MUTATING bash command (sed/cat >/tee/…) ran this run —
    /// a file write the loop cannot attribute to a specific path. It must
    /// still defeat the completion brake and arm the verify/acceptance
    /// gates like a code edit would, or a "改完不验证就宣称完成" summary
    /// ships unverified code (audit: bash-writes-bypass-brake).
    pub(super) bash_wrote_files: bool,
    pub(super) verification_tier: VerificationTier,
    pub(super) verification_failed: bool,
    pub(super) executed_results: HashMap<String, (bool, String)>,
    pub(super) executed_results_scanned_len: usize,
    // ── Skeleton / convergence (Phase 5) ────────────────────────────
    pub(super) decompose_suggested: bool,
    pub(super) exploration_rounds: u32,
    pub(super) exploration_nudges: u32,
    pub(super) todo_plan_nudged: bool,
    pub(super) todo_sync_nudged: bool,
    /// Whether the todo-ordering nudge fired this run — a plan that marks a
    /// step in_progress/completed while a `depends_on` step is unfinished is
    /// out of order and gets ONE nudge to reorder before continuing.
    pub(super) todo_order_nudged: bool,
    pub(super) last_narration_text: Option<String>,
    pub(super) repeat_narration_streak: u32,
    pub(super) reflexion_rounds: u32,
    /// Whether the early code-verify nudge fired this run — after the model
    /// edits code files with no static verification evidence (no auto-LSP),
    /// nudge it ONCE to run the project's typecheck before it concludes.
    pub(super) code_verify_nudged: bool,
    // ── Plan-Execute gate (Phase 0.5) ───────────────────────────────
    pub(super) plan_phase_active: bool,
    pub(super) plan_approved_this_run: bool,
    // ── Loop bookkeeping ────────────────────────────────────────────
    pub(super) budget: BudgetTracker,
    pub(super) reminder_state: ReminderState,
    pub(super) counters: StopGateCounters,
}

/// Decision returned by [`AgentLoop::run_stop_gates`].
pub(super) enum StopGateDecision {
    /// A nudge forced another model turn.
    Continue,
    /// The turn may end normally (the caller emits TurnEnd).
    Stop,
}

/// Compose the per-request tail from the dynamic context and the static
/// suffix (task-spec + hook guidance). `None` components are dropped.
pub(super) fn compose_augmented_message(
    dynamic_ctx: &Option<String>,
    tail_suffix: &Option<String>,
) -> Option<String> {
    match (dynamic_ctx, tail_suffix) {
        (Some(d), Some(s)) => Some(format!("{d}{s}")),
        (Some(d), None) => Some(d.clone()),
        (None, Some(s)) => Some(s.clone()),
        (None, None) => None,
    }
}
