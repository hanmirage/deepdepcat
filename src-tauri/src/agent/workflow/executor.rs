//! Workflow executor — the harness that owns coordination for
//! fan-out / loop-until-done / adversarial-review specs.

use super::{
    parse_loop_status, pipeline_levels, resolve_agent_type, resolve_isolation, LoopStatus,
    ReviewSpec, StepOutcome, WorkflowOutcome, WorkflowProgress, WorkflowSpec, WorkflowStep,
    DEFAULT_ROUND_TIMEOUT_SECS, DEFAULT_STEP_TIMEOUT_SECS, MAX_LOOP_ROUNDS, MAX_PARALLEL,
    MAX_STEP_TURNS, MAX_WORKFLOW_STEPS,
};
use crate::agent::chat_state::ChatState;
use crate::agent::multi_agent::{
    IsolationMode, MultiAgentCoordinator, SubagentConfig, SubagentResult, SubagentType,
};
use crate::toolkit::ToolContext;
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use crate::core::stream::emit_stream;
use crate::core::types::StreamEvent;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

/// Worker results are truncated before they ever reach the parent context —
/// the parent gets the shape, not the process.
const RESULT_TRUNCATE: usize = 3000;
/// Previous-round history kept for the next loop round.
const ROUND_HISTORY_TRUNCATE: usize = 2000;
/// Final summary cap.
const SUMMARY_TRUNCATE: usize = 4000;

/// Run a workflow spec to completion (bounded by caps and cancellations).
///
/// `workflow_id` identifies this run for progress persistence; `resume`
/// carries an interrupted workflow's saved progress so only the unfinished
/// steps re-run. On cancellation the partial progress is saved to the store
/// and the outcome carries a `resume_id` for a later resume.
pub async fn run_workflow(
    workflow_id: String,
    spec: WorkflowSpec,
    resume: Option<WorkflowProgress>,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
) -> AppResult<WorkflowOutcome> {
    match spec {
        WorkflowSpec::FanOut {
            steps,
            synthesize,
            verify,
            max_parallel,
        } => {
            run_fan_out(
                workflow_id,
                steps,
                synthesize,
                verify,
                max_parallel,
                resume,
                ctx,
                coordinator,
            )
            .await
        }
        WorkflowSpec::Pipeline {
            steps,
            synthesize,
            verify,
            max_parallel,
        } => {
            run_pipeline(
                workflow_id,
                steps,
                synthesize,
                verify,
                max_parallel,
                resume,
                ctx,
                coordinator,
            )
            .await
        }
        WorkflowSpec::LoopUntilDone {
            task,
            stop_condition,
            agent_type,
            model,
            max_rounds,
            max_turns,
        } => {
            run_loop_until_done(
                workflow_id,
                task,
                stop_condition,
                agent_type.as_deref(),
                model.as_deref(),
                max_rounds,
                max_turns,
                resume,
                ctx,
                coordinator,
            )
            .await
        }
        WorkflowSpec::AdversarialReview {
            task,
            acceptance,
            edited_paths,
        } => {
            run_adversarial_review(
                workflow_id,
                task,
                acceptance.as_deref(),
                edited_paths.unwrap_or_default(),
                ctx,
                coordinator,
            )
            .await
        }
    }
}

/// Save interrupted workflow progress to the store (cancel → resume).
fn save_progress(
    ctx: &ToolContext,
    workflow_id: &str,
    spec: WorkflowSpec,
    completed: Vec<StepOutcome>,
    round: u32,
    previous: Option<String>,
) {
    let state = ctx.app.state::<AppState>();
    state
        .workflow_store
        .put(WorkflowProgress {
            workflow_id: workflow_id.to_string(),
            spec,
            completed,
            round,
            previous,
        });
}

/// The session's cancellation token — a cancelled parent stops the whole
/// workflow between/inside worker spawns (workers check it themselves).
async fn session_cancel(ctx: &ToolContext) -> CancellationToken {
    let state = ctx.app.state::<AppState>();
    let token = state
        .cancellation_tokens
        .lock()
        .await
        .get(&ctx.session_id)
        .cloned();
    token.unwrap_or_else(CancellationToken::new)
}

/// Emit one workflow progress event (the tool card shows done/total live).
fn emit_progress(app: &AppHandle, ctx: &ToolContext, done: usize, total: usize, step_id: &str) {
    emit_stream(
        app,
        StreamEvent::ToolCallProgress {
            turn_id: ctx.turn_id.clone(),
            call_id: ctx.call_id.clone(),
            name: "workflow".to_string(),
            kind: "custom".to_string(),
            delta: Some(
                json!({
                    "done": done,
                    "total": total,
                    "step_id": step_id,
                })
                .to_string(),
            ),
            total_bytes: None,
        },
    );
}

/// Spawn one worker with workflow defaults (no completion surfacing — the
/// parent only sees the summarized outcome).
#[allow(clippy::too_many_arguments)]
async fn spawn_one(
    task: &str,
    agent_type: &SubagentType,
    model: Option<&str>,
    max_turns: u32,
    timeout_secs: u64,
    paths: Option<Vec<String>>,
    isolation: IsolationMode,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
    cancel: &CancellationToken,
) -> AppResult<SubagentResult> {
    let parent_state = ChatState::with_provider(
        ctx.model.clone(),
        coordinator.default_context_window(),
        ctx.provider.clone(),
    );
    let config = SubagentConfig {
        agent_type: agent_type.clone(),
        task: task.to_string(),
        model: model.map(str::to_string),
        max_turns,
        depth: ctx.agent_depth + 1,
        background: false,
        surface_completion: false,
        isolation,
        timeout_secs: Some(timeout_secs),
        task_id: None,
        call_id: Some(ctx.call_id.clone()),
        fork: false,
        fork_context: Vec::new(),
        work_mode: Some(ctx.work_mode.as_str().to_string()),
        session_id: Some(ctx.session_id.clone()),
        paths,
        image_notes: ctx.attached_images.clone(),
        // The parent's deny chain must survive the workflow tool — a
        // fan-out/loop worker is still a child of the same agent contract
        // (M9 hard veto propagation).
        inherited_denies: ctx.agent_deny_rules.clone(),
    };
    coordinator
        .spawn_subagent_with_cancel(&config, &parent_state, &ctx.app, cancel)
        .await
}

fn truncate(text: &str, max: usize) -> String {
    let clean = crate::core::str_util::strip_tool_call_markup(text);
    let clean = clean.trim();
    if clean.chars().count() <= max {
        clean.to_string()
    } else {
        let truncated: String = clean.chars().take(max).collect();
        format!("{truncated}\n…[truncated]")
    }
}

fn outcome_from_result(id: &str, result: AppResult<SubagentResult>) -> StepOutcome {
    match result {
        Ok(r) => StepOutcome {
            id: id.to_string(),
            success: r.success,
            result: truncate(&r.response, RESULT_TRUNCATE),
            error: r.error,
            edited_files: r.modified_files,
            tokens: r.usage.total(),
        },
        Err(e) => StepOutcome {
            id: id.to_string(),
            success: false,
            result: String::new(),
            error: Some(e.to_string()),
            edited_files: Vec::new(),
            tokens: 0,
        },
    }
}

/// `fan_out`: parallel workers (bounded by the semaphore) → optional
/// synthesis worker → optional adversarial evaluator. `resume` skips steps
/// already completed successfully; cancelled/failed ones re-run.
#[allow(clippy::too_many_arguments)]
async fn run_fan_out(
    workflow_id: String,
    steps: Vec<WorkflowStep>,
    synthesize: Option<String>,
    verify: Option<ReviewSpec>,
    max_parallel: Option<usize>,
    resume: Option<WorkflowProgress>,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
) -> AppResult<WorkflowOutcome> {
    if steps.is_empty() {
        return Err(AppError::Parse(
            "workflow fan_out needs at least one step".into(),
        ));
    }
    if steps.len() > MAX_WORKFLOW_STEPS {
        return Err(AppError::Parse(format!(
            "workflow fan_out supports at most {MAX_WORKFLOW_STEPS} steps"
        )));
    }
    let parallel = max_parallel.unwrap_or(4).clamp(1, MAX_PARALLEL);
    let cancel = session_cancel(ctx).await;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let total = steps.len();
    // The spec snapshot is stored with the progress so a cancelled workflow
    // resumes without the model re-sending the whole spec.
    let spec_snapshot = WorkflowSpec::FanOut {
        steps: steps.clone(),
        synthesize: synthesize.clone(),
        verify: verify.clone(),
        max_parallel,
    };
    // Resume: skip steps that already completed successfully.
    let completed_success: Vec<String> = resume
        .as_ref()
        .map(|p| {
            p.completed
                .iter()
                .filter(|s| s.success)
                .map(|s| s.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let pending: Vec<WorkflowStep> = steps
        .into_iter()
        .filter(|s| !completed_success.contains(&s.id))
        .collect();
    let step_ids: Vec<String> = pending.iter().map(|s| s.id.clone()).collect();
    let mut step_outcomes: Vec<StepOutcome> = resume.map(|p| p.completed).unwrap_or_default();

    // True fan-out: every pending step starts in parallel, the semaphore
    // bounds how many run at once. join_all preserves input order.
    let futures: Vec<_> = pending
        .into_iter()
        .map(|step| {
            let semaphore = semaphore.clone();
            let cancel = cancel.clone();
            let ctx = ctx.clone();
            async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|e| AppError::Internal(format!("Workflow semaphore closed: {e}")))?;
                let agent_type = resolve_agent_type(step.agent_type.as_deref());
                let max_turns = step
                    .max_turns
                    .unwrap_or(MAX_STEP_TURNS)
                    .clamp(1, MAX_STEP_TURNS);
                let timeout = step.timeout_secs.unwrap_or(DEFAULT_STEP_TIMEOUT_SECS);
                let isolation = resolve_isolation(step.isolation.as_deref());
                let paths = step.paths.clone();
                spawn_one(
                    &step.task,
                    &agent_type,
                    step.model.as_deref(),
                    max_turns,
                    timeout,
                    paths,
                    isolation,
                    &ctx,
                    coordinator,
                    &cancel,
                )
                .await
            }
        })
        .collect();

    let results = futures_util::future::join_all(futures).await;
    for (index, (result, step_id)) in results.into_iter().zip(step_ids).enumerate() {
        let outcome = outcome_from_result(&step_id, result);
        emit_progress(&ctx.app, ctx, step_outcomes.len() + index + 1, total, &step_id);
        step_outcomes.push(outcome);
    }

    let mut total_tokens: u64 = step_outcomes.iter().map(|s| s.tokens).sum();
    let done_count = step_outcomes.iter().filter(|s| s.success).count();

    // Cancelled: save the partial progress and return a partial outcome for a
    // later resume — skip synthesis/verify (unfinished steps can't be merged
    // meaningfully, and a cancelled synthesis would burn tokens for nothing).
    if cancel.is_cancelled() {
        save_progress(
            ctx,
            &workflow_id,
            spec_snapshot,
            step_outcomes.clone(),
            0,
            None,
        );
        return Ok(WorkflowOutcome {
            mode: "fan_out".to_string(),
            steps: step_outcomes,
            summary: format!(
                "Workflow cancelled — {done_count}/{total} steps completed. \
                 Resume it with the workflow tool using resume=\"{workflow_id}\"."
            ),
            success: false,
            total_tokens,
            resume_id: Some(workflow_id),
        });
    }

    let mut all_ok = step_outcomes.iter().all(|s| s.success);
    let mut summary = String::new();
    synthesize_and_verify(
        &mut summary,
        &mut all_ok,
        &mut total_tokens,
        &step_outcomes,
        synthesize.as_deref(),
        verify.as_ref(),
        ctx,
        coordinator,
        &cancel,
    )
    .await;

    // Completed — clear the saved progress (a resume is no longer valid).
    ctx.app.state::<AppState>().workflow_store.remove(&workflow_id);

    Ok(WorkflowOutcome {
        mode: "fan_out".to_string(),
        steps: step_outcomes,
        summary,
        success: all_ok,
        total_tokens,
        resume_id: None,
    })
}

/// Shared fan_out/pipeline tail: run the optional synthesis worker, then the
/// optional adversarial evaluator, mutating `summary`/`all_ok`/`total_tokens`
/// in place. Extracted so `fan_out` and `pipeline` share the identical
/// synthesis + independent-review contract.
#[allow(clippy::too_many_arguments)]
async fn synthesize_and_verify(
    summary: &mut String,
    all_ok: &mut bool,
    total_tokens: &mut u64,
    step_outcomes: &[StepOutcome],
    synthesize: Option<&str>,
    verify: Option<&ReviewSpec>,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
    cancel: &CancellationToken,
) {
    if let Some(synth_prompt) = synthesize {
        let mut body = String::from(
            "Synthesize the following worker results into the requested deliverable. \
             Be concrete and cite which step produced what.\n\n",
        );
        body.push_str(synth_prompt);
        body.push_str("\n\n## Worker results\n");
        for s in step_outcomes {
            body.push_str(&format!(
                "\n### {} (success: {})\n{}\n",
                s.id, s.success, s.result
            ));
        }
        match spawn_one(
            &body,
            &SubagentType::General,
            None,
            20,
            DEFAULT_STEP_TIMEOUT_SECS,
            None,
            IsolationMode::None,
            ctx,
            coordinator,
            cancel,
        )
        .await
        {
            Ok(r) => {
                *total_tokens += r.usage.total();
                if !r.success {
                    *all_ok = false;
                }
                *summary = truncate(&r.response, SUMMARY_TRUNCATE);
            }
            Err(e) => {
                *all_ok = false;
                *summary = format!("Synthesis failed: {e}");
            }
        }
    } else {
        let joined = step_outcomes
            .iter()
            .map(|s| format!("{}: {}", s.id, s.result))
            .collect::<Vec<_>>()
            .join("\n");
        *summary = truncate(&joined, SUMMARY_TRUNCATE);
    }

    if let Some(review) = verify {
        let review_task = build_review_task(summary.as_str(), review.acceptance.as_deref());
        match spawn_one(
            &review_task,
            &SubagentType::Evaluator,
            None,
            20,
            DEFAULT_STEP_TIMEOUT_SECS,
            None,
            IsolationMode::None,
            ctx,
            coordinator,
            cancel,
        )
        .await
        {
            Ok(r) => {
                *total_tokens += r.usage.total();
                let verdict =
                    crate::agent::agent_loop::evaluator::parse_evaluator_report(&r.response);
                let (review_ok, body) = match verdict {
                    crate::agent::agent_loop::evaluator::EvaluatorVerdict::Pass => {
                        (true, "Verification: PASS".to_string())
                    }
                    crate::agent::agent_loop::evaluator::EvaluatorVerdict::Fail { findings } => {
                        (false, findings)
                    }
                };
                if !review_ok {
                    *all_ok = false;
                }
                let merged = format!("{}\n\n## Independent review\n{body}", summary.as_str());
                *summary = truncate(&merged, SUMMARY_TRUNCATE);
            }
            Err(e) => {
                *all_ok = false;
                let merged = format!(
                    "{}\n\n## Independent review failed\n{e}",
                    summary.as_str()
                );
                *summary = truncate(&merged, SUMMARY_TRUNCATE);
            }
        }
    }
}

/// Build a pipeline step's brief, appending the truncated results of its
/// completed dependencies so the worker sees the context it depends on.
fn build_pipeline_task(
    task: &str,
    depends_on: &[String],
    results: &HashMap<String, StepOutcome>,
) -> String {
    if depends_on.is_empty() {
        return task.to_string();
    }
    let mut out = String::from(task);
    out.push_str("\n\n## Dependencies' results\n");
    for dep in depends_on {
        match results.get(dep) {
            Some(r) if r.success => {
                out.push_str(&format!("\n### {dep}\n{}\n", r.result));
            }
            Some(r) => {
                out.push_str(&format!(
                    "\n### {dep} (FAILED: {})\n{}\n",
                    r.error.as_deref().unwrap_or("unknown error"),
                    r.result
                ));
            }
            None => {
                out.push_str(&format!("\n### {dep} (no result available)\n"));
            }
        }
    }
    out
}

/// `pipeline`: dependency-ordered execution. Steps are topologically sorted
/// into levels; each level runs in parallel (bounded by the semaphore), and a
/// step's brief auto-appends the truncated results of its completed
/// dependencies. `resume` skips steps already completed successfully.
#[allow(clippy::too_many_arguments)]
async fn run_pipeline(
    workflow_id: String,
    steps: Vec<WorkflowStep>,
    synthesize: Option<String>,
    verify: Option<ReviewSpec>,
    max_parallel: Option<usize>,
    resume: Option<WorkflowProgress>,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
) -> AppResult<WorkflowOutcome> {
    if steps.is_empty() {
        return Err(AppError::Parse(
            "workflow pipeline needs at least one step".into(),
        ));
    }
    if steps.len() > MAX_WORKFLOW_STEPS {
        return Err(AppError::Parse(format!(
            "workflow pipeline supports at most {MAX_WORKFLOW_STEPS} steps"
        )));
    }
    let parallel = max_parallel.unwrap_or(4).clamp(1, MAX_PARALLEL);
    let cancel = session_cancel(ctx).await;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let total = steps.len();

    // Validate the DAG up front — a malformed spec must fail before any
    // worker spawns.
    let levels = pipeline_levels(&steps).map_err(AppError::Parse)?;

    let completed_success: Vec<String> = resume
        .as_ref()
        .map(|p| {
            p.completed
                .iter()
                .filter(|s| s.success)
                .map(|s| s.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut outcomes: Vec<StepOutcome> = resume.map(|p| p.completed).unwrap_or_default();
    let mut results_by_id: HashMap<String, StepOutcome> = outcomes
        .iter()
        .map(|o| (o.id.clone(), o.clone()))
        .collect();

    let spec_snapshot = WorkflowSpec::Pipeline {
        steps: steps.clone(),
        synthesize: synthesize.clone(),
        verify: verify.clone(),
        max_parallel,
    };

    let mut cancelled = false;
    for level in &levels {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        let pending: Vec<&WorkflowStep> = level
            .iter()
            .filter(|&&i| !completed_success.contains(&steps[i].id))
            .map(|&i| &steps[i])
            .collect();
        if pending.is_empty() {
            continue;
        }
        // Snapshot the completed prefix ONCE per level — all of a level's
        // steps share the same dependency results.
        let results_snapshot = results_by_id.clone();
        let futures: Vec<_> = pending
            .into_iter()
            .map(|step| {
                let task = build_pipeline_task(&step.task, &step.depends_on, &results_snapshot);
                let agent_type = resolve_agent_type(step.agent_type.as_deref());
                let max_turns = step
                    .max_turns
                    .unwrap_or(MAX_STEP_TURNS)
                    .clamp(1, MAX_STEP_TURNS);
                let timeout = step.timeout_secs.unwrap_or(DEFAULT_STEP_TIMEOUT_SECS);
                let isolation = resolve_isolation(step.isolation.as_deref());
                let paths = step.paths.clone();
                let model = step.model.clone();
                let step_id = step.id.clone();
                let semaphore = semaphore.clone();
                let cancel = cancel.clone();
                let ctx = ctx.clone();
                async move {
                    let _permit = semaphore.acquire_owned().await.map_err(|e| {
                        AppError::Internal(format!("Workflow semaphore closed: {e}"))
                    })?;
                    let r = spawn_one(
                        &task,
                        &agent_type,
                        model.as_deref(),
                        max_turns,
                        timeout,
                        paths,
                        isolation,
                        &ctx,
                        coordinator,
                        &cancel,
                    )
                    .await;
                    Ok::<_, AppError>((step_id, r))
                }
            })
            .collect();

        let results = futures_util::future::join_all(futures).await;
        for res in results {
            let (step_id, r) = res?;
            let outcome = outcome_from_result(&step_id, r);
            emit_progress(&ctx.app, ctx, outcomes.len() + 1, total, &step_id);
            results_by_id.insert(step_id, outcome.clone());
            outcomes.push(outcome);
        }
    }

    let mut total_tokens: u64 = outcomes.iter().map(|s| s.tokens).sum();
    let done_count = outcomes.iter().filter(|s| s.success).count();

    if cancelled || cancel.is_cancelled() {
        save_progress(ctx, &workflow_id, spec_snapshot, outcomes.clone(), 0, None);
        return Ok(WorkflowOutcome {
            mode: "pipeline".to_string(),
            steps: outcomes,
            summary: format!(
                "Workflow cancelled — {done_count}/{total} steps completed. \
                 Resume it with the workflow tool using resume=\"{workflow_id}\"."
            ),
            success: false,
            total_tokens,
            resume_id: Some(workflow_id),
        });
    }

    let mut all_ok = outcomes.iter().all(|s| s.success);
    let mut summary = String::new();
    synthesize_and_verify(
        &mut summary,
        &mut all_ok,
        &mut total_tokens,
        &outcomes,
        synthesize.as_deref(),
        verify.as_ref(),
        ctx,
        coordinator,
        &cancel,
    )
    .await;

    // Completed — clear the saved progress (a resume is no longer valid).
    ctx.app.state::<AppState>().workflow_store.remove(&workflow_id);

    Ok(WorkflowOutcome {
        mode: "pipeline".to_string(),
        steps: outcomes,
        summary,
        success: all_ok,
        total_tokens,
        resume_id: None,
    })
}

/// `loop_until_done`: repeatedly spawn a worker; the HARNESS parses the
/// terminal marker (WORKFLOW_STATUS: DONE) and the round cap bounds it.
/// `resume` continues from the saved round history after a cancellation.
#[allow(clippy::too_many_arguments)]
async fn run_loop_until_done(
    workflow_id: String,
    task: String,
    stop_condition: String,
    agent_type: Option<&str>,
    model: Option<&str>,
    max_rounds: Option<u32>,
    max_turns: Option<u32>,
    resume: Option<WorkflowProgress>,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
) -> AppResult<WorkflowOutcome> {
    let rounds = max_rounds
        .unwrap_or(MAX_LOOP_ROUNDS)
        .clamp(1, MAX_LOOP_ROUNDS);
    let agent = resolve_agent_type(agent_type);
    let turns = max_turns.unwrap_or(20).clamp(1, MAX_STEP_TURNS);
    let cancel = session_cancel(ctx).await;
    // Resume: continue from the saved round history.
    let mut previous: Option<String> = resume.as_ref().and_then(|p| p.previous.clone());
    let mut total_tokens: u64 = 0;
    let start_round = resume.as_ref().map(|p| p.round).unwrap_or(0) + 1;

    for round in start_round..=rounds {
        if cancel.is_cancelled() {
            // Save partial progress for a later resume instead of dropping it.
            save_progress(
                ctx,
                &workflow_id,
                WorkflowSpec::LoopUntilDone {
                    task: task.clone(),
                    stop_condition: stop_condition.clone(),
                    agent_type: agent_type.map(str::to_string),
                    model: model.map(str::to_string),
                    max_rounds,
                    max_turns,
                },
                Vec::new(),
                round - 1,
                previous.clone(),
            );
            return Ok(WorkflowOutcome {
                mode: "loop_until_done".to_string(),
                steps: vec![],
                summary: format!(
                    "Workflow cancelled after {} rounds. Resume it with the workflow \
                     tool using resume=\"{workflow_id}\".",
                    round - 1
                ),
                success: false,
                total_tokens,
                resume_id: Some(workflow_id),
            });
        }
        let round_task = build_loop_task(&task, &stop_condition, round, previous.as_deref());
        match spawn_one(
            &round_task,
            &agent,
            model,
            turns,
            DEFAULT_ROUND_TIMEOUT_SECS,
            None,
            IsolationMode::None,
            ctx,
            coordinator,
            &cancel,
        )
        .await
        {
            Ok(r) => {
                total_tokens += r.usage.total();
                emit_progress(
                    &ctx.app,
                    ctx,
                    round as usize,
                    rounds as usize,
                    &format!("round-{round}"),
                );
                match parse_loop_status(&r.response) {
                    LoopStatus::Done => {
                        let summary = truncate(&r.response, SUMMARY_TRUNCATE);
                        // Completed — clear the saved progress.
                        ctx.app.state::<AppState>().workflow_store.remove(&workflow_id);
                        return Ok(WorkflowOutcome {
                            mode: "loop_until_done".to_string(),
                            steps: vec![StepOutcome {
                                id: format!("round-{round}"),
                                success: r.success,
                                result: truncate(&r.response, RESULT_TRUNCATE),
                                error: r.error,
                                edited_files: r.modified_files,
                                tokens: r.usage.total(),
                            }],
                            summary,
                            success: r.success,
                            total_tokens,
                            resume_id: None,
                        });
                    }
                    LoopStatus::Continue(_) => {
                        previous = Some(truncate(&r.response, ROUND_HISTORY_TRUNCATE));
                    }
                }
            }
            Err(e) => {
                return Ok(WorkflowOutcome {
                    mode: "loop_until_done".to_string(),
                    steps: vec![StepOutcome {
                        id: format!("round-{round}"),
                        success: false,
                        result: String::new(),
                        error: Some(e.to_string()),
                        edited_files: Vec::new(),
                        tokens: 0,
                    }],
                    summary: format!("Loop failed at round {round}: {e}"),
                    success: false,
                    total_tokens,
                    resume_id: None,
                });
            }
        }
    }

    Ok(WorkflowOutcome {
        mode: "loop_until_done".to_string(),
        steps: vec![StepOutcome {
            id: "rounds".to_string(),
            success: false,
            result: previous.clone().unwrap_or_default(),
            error: Some(format!(
                "Reached the {rounds}-round cap without WORKFLOW_STATUS: DONE"
            )),
            edited_files: Vec::new(),
            tokens: 0,
        }],
        summary: format!(
            "Reached the {rounds}-round cap — the stop condition was not met. \
             Last round result:\n{}",
            previous.clone().unwrap_or_default()
        ),
        success: false,
        total_tokens,
        resume_id: None,
    })
}

/// Build the per-round worker brief — includes the stop condition contract
/// the harness parses (`WORKFLOW_STATUS: DONE|CONTINUE`).
pub fn build_loop_task(
    task: &str,
    stop_condition: &str,
    round: u32,
    previous: Option<&str>,
) -> String {
    let mut out = format!(
        "{task}\n\n\
         Stop condition: {stop_condition}\n\
         This is round {round} of a loop.\n"
    );
    if let Some(prev) = previous {
        out.push_str(&format!("\n## Previous round result\n{prev}\n"));
    }
    out.push_str(
        "\nWork on the task and assess the stop condition against the ACTUAL \
         current state. End your report with exactly one line:\n\
         WORKFLOW_STATUS: DONE   (if the stop condition is met, with evidence)\n\
         WORKFLOW_STATUS: CONTINUE  (if more work remains, with what you did this round)\n",
    );
    out
}

/// `adversarial_review`: one independent evaluator pass with per-criterion
/// verdicts (M2 rubric contract).
async fn run_adversarial_review(
    _workflow_id: String,
    task: String,
    acceptance: Option<&str>,
    edited_paths: Vec<String>,
    ctx: &ToolContext,
    coordinator: &MultiAgentCoordinator,
) -> AppResult<WorkflowOutcome> {
    let cancel = session_cancel(ctx).await;
    let review_task = build_review_task_with_paths(&task, acceptance, &edited_paths);
    match spawn_one(
        &review_task,
        &SubagentType::Evaluator,
        None,
        20,
        DEFAULT_STEP_TIMEOUT_SECS,
        None,
        IsolationMode::None,
        ctx,
        coordinator,
        &cancel,
    )
    .await
    {
        Ok(r) => {
            let verdict = crate::agent::agent_loop::evaluator::parse_evaluator_report(&r.response);
            let (success, summary) = match verdict {
                crate::agent::agent_loop::evaluator::EvaluatorVerdict::Pass => {
                    (true, "Independent review: PASS".to_string())
                }
                crate::agent::agent_loop::evaluator::EvaluatorVerdict::Fail { findings } => {
                    (false, findings)
                }
            };
            Ok(WorkflowOutcome {
                mode: "adversarial_review".to_string(),
                steps: vec![StepOutcome {
                    id: "review".to_string(),
                    success,
                    result: truncate(&r.response, RESULT_TRUNCATE),
                    error: r.error,
                    edited_files: r.modified_files,
                    tokens: r.usage.total(),
                }],
                summary: truncate(&summary, SUMMARY_TRUNCATE),
                success,
                total_tokens: r.usage.total(),
                resume_id: None,
            })
        }
        Err(e) => Ok(WorkflowOutcome {
            mode: "adversarial_review".to_string(),
            steps: vec![],
            summary: format!("Review failed: {e}"),
            success: false,
            total_tokens: 0,
            resume_id: None,
        }),
    }
}

/// Evaluator brief over an already-synthesized summary (fan-out verify).
fn build_review_task(summary: &str, acceptance: Option<&str>) -> String {
    let acceptance_section = acceptance
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| format!("\n\n## Acceptance criteria\n{c}"))
        .unwrap_or_default();
    format!(
        "Independently review the following work against its acceptance criteria. \
         For EACH criterion report a separate line with PASS/FAIL and evidence \
         (command/output or file:line). Do NOT modify any files.\n\n\
         ## Work under review\n{summary}{acceptance_section}\n\n\
         End your report with 'VERDICT: PASS' or 'VERDICT: FAIL' followed by \
         'FINDINGS:' and a bullet list of concrete evidence."
    )
}

/// Evaluator brief for the standalone review mode.
fn build_review_task_with_paths(
    task: &str,
    acceptance: Option<&str>,
    edited_paths: &[String],
) -> String {
    let acceptance_section = acceptance
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| format!("\n\n## Acceptance criteria\n{c}"))
        .unwrap_or_default();
    let targets = if edited_paths.is_empty() {
        "the changes are in the workspace — locate them with read/search tools.".to_string()
    } else {
        format!(
            "the following files:\n{}",
            edited_paths
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "Independently review the work done for the following task against its \
         acceptance criteria. For EACH criterion report a separate line with \
         PASS/FAIL and evidence (command/output or file:line). Verify against \
         the actual code and a real run (tests/build/LSP diagnostics). Do NOT \
         modify any files.\n\n## Task\n{task}{acceptance_section}\n\n## Changes\n{targets}\n\n\
         End your report with 'VERDICT: PASS' or 'VERDICT: FAIL' followed by \
         'FINDINGS:' and a bullet list of concrete evidence."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_task_includes_stop_condition_contract() {
        let task = build_loop_task("fix flaky test", "tests pass 3x", 2, Some("round 1: x"));
        assert!(task.contains("Stop condition: tests pass 3x"));
        assert!(task.contains("round 2"));
        assert!(task.contains("Previous round result"));
        assert!(task.contains("WORKFLOW_STATUS: DONE"));
        assert!(task.contains("WORKFLOW_STATUS: CONTINUE"));
    }

    #[test]
    fn truncate_strips_tool_markup_and_caps() {
        let long = format!("<tool_calls>junk</tool_calls>{}", "x".repeat(5000));
        let out = truncate(&long, 100);
        assert!(!out.contains("<tool_calls>"));
        assert!(out.chars().count() <= 100 + "…[truncated]".len());
    }

    /// REAL DeepSeek smoke: the loop-worker contract (`WORKFLOW_STATUS:
    /// DONE|CONTINUE`) must hold on live model output — runs only when
    /// DEEPSEEK_API_KEY is set
    /// (`cargo test --lib -- --ignored real_deepseek_loop_contract_smoke --nocapture`).
    #[tokio::test]
    #[ignore = "requires a real DEEPSEEK_API_KEY"]
    async fn real_deepseek_loop_contract_smoke() {
        use crate::core::config::ProviderConfig;
        use crate::core::types::ConversationItem;
        use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
        use crate::llm::client::LlmClient;
        use crate::llm::provider::{LlmProvider, LlmRequest};
        use crate::llm::retry::RetryConfig;
        use std::sync::Arc;

        let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        };
        let provider = ProviderConfig {
            name: "deepseek".to_string(),
            api_key_env: String::new(),
            api_key: Some(key),
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            protocol: None,
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig {
                max_retries: 1,
                base_delay: std::time::Duration::from_millis(300),
                max_delay: std::time::Duration::from_secs(3),
                fallback_models: vec![],
            },
            true,
            Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
                failure_threshold: 3,
                open_timeout_secs: 10,
            })),
        );

        // The same loop-worker prompt a real workflow run constructs.
        let prompt = build_loop_task(
            "Find why the login test flakes and fix it",
            "login test passes 3x consecutively",
            1,
            None,
        );
        let req = LlmRequest {
            model: "deepseek-chat".to_string(),
            provider: Some("deepseek".to_string()),
            messages: vec![ConversationItem::user(prompt)],
            tools: vec![],
            system_prompt: String::new(),
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(400),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };
        let resp = client
            .complete(&req)
            .await
            .expect("live DeepSeek loop-contract call must succeed");
        let report = resp.content.trim();
        eprintln!("report: {report:?}");
        assert!(!report.is_empty(), "worker must produce a report");
        // The harness parses the marker — it must be parseable either way.
        let status = parse_loop_status(report);
        eprintln!("parsed status: {status:?}");
        match status {
            LoopStatus::Done => assert!(
                report.to_uppercase().contains("WORKFLOW_STATUS"),
                "DONE must come from an explicit marker"
            ),
            LoopStatus::Continue(_) => {}
        }
    }
}
