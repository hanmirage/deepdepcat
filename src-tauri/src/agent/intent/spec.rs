//! Task-spec injection + delegation advice.
use super::signals::*;
use super::types::*;

pub fn build_task_spec(
    result: &IntentResult,
    message: &str,
    decision: &IntentDecision,
) -> Option<String> {
    if !result.intent.is_actionable() {
        return None;
    }
    let mut lines = vec![format!("<intent>{}</intent>", decision.intent.as_str())];
    if let Some(acc) = &result.acceptance_hint {
        lines.push(format!("<acceptance>{acc}</acceptance>"));
    }
    if decision.complexity != TaskComplexity::Low {
        lines.push(format!(
            "<complexity>{}</complexity>",
            decision.complexity.as_str()
        ));
    }
    if decision.needs_planning {
        lines.push(
            "<planning_required>This is a multi-step task. BEFORE making any \
               changes, call todo_write to lay out the execution plan (one todo \
               item per step), then execute the steps in order — updating todo \
               status as each completes. The todo list is the user's task panel: \
               keep it accurate.\n\
               For a LONG task (a game, a system, a multi-file build): \
               (1) order the steps by dependency and set each step's depends_on \
               to the ids it must wait for — never start a step before its \
               dependencies are completed; (2) give each step a verify command \
               (test/lint/typecheck/run) and mark it completed only when that \
               passes; (3) build in vertical slices — each phase must leave the \
               project runnable, not a pile of half-written layers.</planning_required>"
                .to_string(),
        );
    }
    if decision.multi_intent {
        let sub = split_sub_asks(message);
        if sub.len() > 1 {
            lines.push(format!(
                "<multi_intent>This message contains {} distinct asks. Treat \
                   each ask as a separate todo item — call todo_write with one \
                   item per ask. If the asks are independent and large enough \
                   to parallelize, delegate them per the delegation advice \
                   below; otherwise execute them in order yourself. Do not \
                   merge or drop any ask.</multi_intent>",
                sub.len()
            ));
        }
    }
    if let Some((tier, reason)) = delegation_advice(decision, message) {
        lines.push(format!("<delegation>{}</delegation>", tier.as_str()));
        lines.push(format!("<delegation_reason>{reason}</delegation_reason>"));
    }
    Some(format!(
        "\n\n<task-spec>\n{}\n</task-spec>",
        lines.join("\n")
    ))
}

/// How a task should be executed: directly by the main agent, or delegated
/// to parallel workers. The system derives a suggestion from task-scale
/// signals; the model follows it or overrides it with justification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationTier {
    /// Small, single-purpose — do it directly, no subagents.
    Direct,
    /// Multi-part — 2-3 parallel workers when the parts are independent.
    Parallel2_3,
    /// Large with many independent parts — 3-5 parallel workers.
    Parallel3_5,
}

impl DelegationTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Parallel2_3 => "parallel_2_3",
            Self::Parallel3_5 => "parallel_3_5",
        }
    }
}

/// The delegation suggestion injected into `<task-spec>`, or `None` when
/// the task is unremarkable (no advice beats noise).
///
/// `needs_subagents` (from the routing decision) selects the parallel tier;
/// the anti-overreach light-task check still forces `direct` for small
/// single-file edits, so a router misjudgment can never over-delegate.
pub const DELEGATION_REASON_DIRECT: &str =
    "small, single-purpose change — spawning a subagent costs more than \
     the work itself; do it directly";
pub const DELEGATION_REASON_PARALLEL_2_3: &str =
    "multi-part task — if the parts are independent, delegate to 2-3 \
     parallel workers via the agent tool (decompose: true); if they \
     depend on each other, work through them sequentially yourself";
pub const DELEGATION_REASON_PARALLEL_3_5: &str =
    "large task with many independent parts — delegate to 3-5 parallel \
     workers via the agent tool (decompose: true); re-plan if it would \
     need more";

pub fn delegation_advice(
    decision: &IntentDecision,
    message: &str,
) -> Option<(DelegationTier, &'static str)> {
    if !decision.intent.is_actionable() {
        return None;
    }
    // The anti-overreach check runs FIRST: a router misjudgment can never
    // over-delegate a genuinely tiny edit.
    if light_task_signal(message, decision.intent) {
        return Some((DelegationTier::Direct, DELEGATION_REASON_DIRECT));
    }
    if decision.needs_subagents {
        return Some(match decision.complexity {
            TaskComplexity::High => (DelegationTier::Parallel3_5, DELEGATION_REASON_PARALLEL_3_5),
            _ => (DelegationTier::Parallel2_3, DELEGATION_REASON_PARALLEL_2_3),
        });
    }
    None
}
