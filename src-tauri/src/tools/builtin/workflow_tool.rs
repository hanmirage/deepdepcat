//! Workflow tool — harness-in-code multi-agent orchestration.
//!
//! The model submits a structured workflow (fan-out / loop-until-done /
//! adversarial review) in ONE call; the Rust harness owns the coordination
//! and only the summarized outcome reaches the parent context. This is the
//! DeepDepCat version of Anthropic's Dynamic Workflows: orchestration lives
//! in code, judgment lives in the model.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::agent::workflow::{executor, WorkflowSpec};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;

pub struct WorkflowTool;

impl WorkflowTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        "workflow"
    }

    fn description(&self) -> &str {
        "Run a multi-agent WORKFLOW with the harness owning the coordination. \
         Use for large tasks that need parallel independent work, dependency-\
         ordered sequential work, iterative convergence, or independent \
         verification — NOT for small focused tasks (do those directly).\n\n\
         Four modes:\n\
         1. fan_out — split work into parallel steps (each a complete \
         self-contained brief; see the `agent` tool's brief contract), run \
         them concurrently (max_parallel), optionally synthesize the results \
         into one deliverable, optionally verify them with an independent \
         evaluator (acceptance criteria checked per item).\n\
         2. pipeline — dependency-ordered steps: set each step's depends_on \
         to the ids it must wait for. The harness topologically sorts them \
         into levels, runs each level in parallel (max_parallel), and appends \
         each step's dependencies' truncated results to its brief. Use for \
         sequential work with real orderings (a game: render → input → \
         collision → scoring).\n\
         3. loop_until_done — repeatedly run a worker until it reports \
         WORKFLOW_STATUS: DONE (the harness parses this; workers are \
         instructed to include the marker), bounded by max_rounds. Ideal for \
         flaky-test hunts, iterative research, or convergence tasks.\n\
         4. adversarial_review — one independent evaluator pass over work \
         (task + optional acceptance criteria + edited files), with per-\
         criterion PASS/FAIL evidence.\n\n\
         Cap the scale: at most 20 steps, 8 parallel, 6 loop rounds. Prefer \
         worktree isolation (isolation: \"worktree\") when steps write \
         overlapping areas."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["fan_out", "pipeline", "loop_until_done", "adversarial_review"],
                    "description": "Which workflow pattern to run."
                },
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "Stable step id for progress and the report."},
                            "task": {"type": "string", "description": "Complete self-contained brief (objective, output format, boundaries, background)."},
                            "agent_type": {"type": "string", "enum": ["explore", "plan", "evaluator", "general"], "description": "Worker type (default general)."},
                            "model": {"type": "string", "description": "Optional per-step model override."},
                            "max_turns": {"type": "integer", "description": "Optional per-step turn cap (default 30)."},
                            "timeout_secs": {"type": "integer", "description": "Optional per-step wall-clock timeout (default 180)."},
                            "paths": {"type": "array", "items": {"type": "string"}, "description": "Files this step will WRITE (parallel conflict preflight)."},
                            "isolation": {"type": "string", "enum": ["worktree"], "description": "\"worktree\" runs the step in a git worktree."},
                            "depends_on": {"type": "array", "items": {"type": "string"}, "description": "pipeline: ids of steps this step waits for; runs only after all are done, and its brief gets their results."}
                        },
                        "required": ["id", "task"]
                    }
                },
                "synthesize": {"type": "string", "description": "fan_out / pipeline: prompt for the synthesis worker that merges step results."},
                "verify": {"type": "object", "properties": {"acceptance": {"type": "string"}}, "description": "fan_out / pipeline: optional independent adversarial verification."},
                "max_parallel": {"type": "integer", "description": "fan_out / pipeline: max concurrent steps (default 4, cap 8)."},
                "task": {"type": "string", "description": "loop_until_done / adversarial_review: the task brief."},
                "stop_condition": {"type": "string", "description": "loop_until_done: the objective criterion the loop converges on."},
                "agent_type": {"type": "string", "enum": ["explore", "plan", "evaluator", "general"], "description": "loop_until_done: worker type (default general)."},
                "model": {"type": "string", "description": "Optional worker model override."},
                "max_rounds": {"type": "integer", "description": "loop_until_done: round cap (default 6)."},
                "max_turns": {"type": "integer", "description": "Optional per-round turn cap."},
                "acceptance": {"type": "string", "description": "adversarial_review: explicit acceptance criteria."},
                "edited_paths": {"type": "array", "items": {"type": "string"}, "description": "adversarial_review: files under review."},
                "resume": {"type": "string", "description": "Resume an INTERRUPTED workflow by its resume id (from a cancelled workflow's result). Uses the saved spec — other fields are ignored."}
            },
            "required": ["mode"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let state = context.app.state::<AppState>();
        let coordinator = &state.coordinator;
        if !coordinator.is_enabled() {
            return Err(AppError::MultiAgent(
                "Multi-agent mode is disabled in configuration".to_string(),
            ));
        }

        // A fresh workflow gets a new id; a resume reuses the interrupted
        // workflow's saved spec and progress (skips the completed steps).
        let workflow_id = crate::core::ids::generate_id();
        let resume_id = args
            .get("resume")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let (spec, resume) = match &resume_id {
            Some(id) => {
                let progress = state.workflow_store.get(id).ok_or_else(|| {
                    AppError::Parse(format!(
                        "Workflow resume failed: no interrupted workflow with id {id}"
                    ))
                })?;
                (progress.spec.clone(), Some(progress))
            }
            None => {
                let spec: WorkflowSpec = serde_json::from_value(args)
                    .map_err(|e| AppError::Parse(format!("Invalid workflow spec: {e}")))?;
                (spec, None)
            }
        };

        let outcome =
            executor::run_workflow(workflow_id, spec, resume, context, coordinator).await?;
        let content = serde_json::to_string_pretty(&outcome).unwrap_or_else(|_| {
            "Workflow completed but the report could not be serialized".to_string()
        });
        // A failed workflow is an error result — the loop treats it as
        // feedback (findings in the content) rather than a silent success.
        Ok(ToolResult {
            content,
            is_error: !outcome.success,
            metadata: None,
            image: None,
        })
    }
}
