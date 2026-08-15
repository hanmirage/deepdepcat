//! Reminder text and tail-guidance constants for the run loop.

/// Build the post-compaction activity reminder — running background tasks
/// and the declared session goal, so compaction never drops "live" state.
pub(super) fn build_activity_reminder(
    running_tasks: &[crate::tools::background::BackgroundTask],
    goal: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !running_tasks.is_empty() {
        let mut lines = Vec::new();
        for t in running_tasks {
            let command: String = t.command.chars().take(120).collect();
            lines.push(format!("- `{}` ({})", command, t.id));
        }
        parts.push(format!(
            "<system-reminder>\n## Running Background Tasks\n{}\n</system-reminder>",
            lines.join("\n")
        ));
    }
    if let Some(g) = goal {
        if !g.trim().is_empty() {
            parts.push(format!(
                "<system-reminder>\n## Current Goal\n{}\n</system-reminder>",
                g
            ));
        }
    }
    parts.join("\n")
}

/// Budget for the per-request tail guidance not measured directly at the
/// compaction estimate point — goal text is measured precisely, and this
/// constant covers interjection fragments (todo gates, background signals,
/// evaluator findings) which are collected into the request a step later.
/// Over-allowance slightly early-triggers compaction (safe); a zero
/// allowance would quietly resurrect the prompt-too-long emergency path.
pub(super) const TAIL_GUIDANCE_ALLOWANCE_TOKENS: u64 = 2048;

/// Plan-mode workflow guidance — injected as a TRAILING user message while
/// the permission mode is read-only (never the system prompt: the system
/// prompt must stay byte-stable for the DeepSeek prefix cache, and the
/// workflow text appearing/disappearing with the mode flip would otherwise
/// invalidate the whole cached prefix). Mirrors the 6-step plan workflow
/// and the pause-for-approval contract of `exit_plan_mode`.
pub(super) const PLAN_MODE_WORKFLOW: &str = r#"
<plan_mode_workflow>
Plan mode is active — the environment is READ-ONLY. No file changes are allowed.
Follow this workflow:
1. Thoroughly explore the codebase to understand existing patterns before designing anything.
2. Identify similar features and architecture; understand the trade-offs.
3. If the approach is genuinely ambiguous, ask the user (ask_user) — one focused question.
4. Design a concrete implementation strategy with specific files and changes.
5. Write the full plan into the `plan` argument of `exit_plan_mode`.
6. When ready, call exit_plan_mode to PAUSE and present the plan for approval.
The user's approval is a hard gate: you may not start coding until they approve.
If the plan is rejected with feedback, revise the plan to address it and call
exit_plan_mode again.
</plan_mode_workflow>"#;
