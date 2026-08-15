//! Dynamic workflow orchestration — harness-in-code multi-agent patterns.
//!
//! The model submits a structured [`WorkflowSpec`] in ONE tool call; the
//! Rust harness owns the coordination (parallel spawn, synthesis, loop
//! termination, adversarial review). The parent context only sees the
//! summarized outcome — orchestration details never live in the model's
//! window (Anthropic Dynamic Workflows principle, adapted to a structured
//! DSL instead of a JS runtime).

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub mod executor;

/// Hard caps — a runaway workflow must not burn the whole session budget.
pub const MAX_WORKFLOW_STEPS: usize = 20;
pub const MAX_PARALLEL: usize = 8;
pub const MAX_LOOP_ROUNDS: u32 = 6;
pub const MAX_STEP_TURNS: u32 = 30;
/// Default wall-clock timeout for one fan-out step.
pub const DEFAULT_STEP_TIMEOUT_SECS: u64 = 180;
/// Default wall-clock timeout for one loop round.
pub const DEFAULT_ROUND_TIMEOUT_SECS: u64 = 120;

/// The workflow the model asks the harness to run.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WorkflowSpec {
    /// Parallel fan-out (optionally synthesized and adversarially verified).
    FanOut {
        steps: Vec<WorkflowStep>,
        #[serde(default)]
        synthesize: Option<String>,
        #[serde(default)]
        verify: Option<ReviewSpec>,
        #[serde(default)]
        max_parallel: Option<usize>,
    },
    /// Dependency-ordered pipeline (DAG): steps are topologically sorted into
    /// levels; each step's brief receives its dependencies' truncated results.
    /// A step with no `depends_on` runs in the first level; every other step
    /// runs only after every id it lists completed.
    Pipeline {
        steps: Vec<WorkflowStep>,
        #[serde(default)]
        synthesize: Option<String>,
        #[serde(default)]
        verify: Option<ReviewSpec>,
        #[serde(default)]
        max_parallel: Option<usize>,
    },
    /// Loop spawning a worker until the report says DONE (or round cap).
    LoopUntilDone {
        task: String,
        stop_condition: String,
        #[serde(default)]
        agent_type: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        max_rounds: Option<u32>,
        #[serde(default)]
        max_turns: Option<u32>,
    },
    /// One independent adversarial review (evaluator seat, rubric-checked).
    AdversarialReview {
        task: String,
        #[serde(default)]
        acceptance: Option<String>,
        #[serde(default)]
        edited_paths: Option<Vec<String>>,
    },
}

/// One fan-out step — a self-contained worker brief.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowStep {
    /// Stable id for progress events and the outcome report.
    pub id: String,
    /// Complete self-contained brief (the worker cannot see the parent
    /// conversation — same contract as the `agent` tool).
    pub task: String,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// File paths this worker intends to WRITE (parallel conflict preflight).
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// `"worktree"` runs this worker in a git worktree.
    #[serde(default)]
    pub isolation: Option<String>,
    /// Step ids this step waits on (pipeline mode). The step runs only after
    /// every listed id completed; its brief auto-appends their truncated
    /// results. Empty = no dependency (runs in the first level).
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Optional adversarial verification for a fan-out's output.
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewSpec {
    #[serde(default)]
    pub acceptance: Option<String>,
}

/// One worker's outcome inside a workflow.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StepOutcome {
    pub id: String,
    pub success: bool,
    /// Truncated result — the parent gets the shape, not the full process.
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited_files: Vec<String>,
    pub tokens: u64,
}

/// The structured outcome handed back to the parent agent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowOutcome {
    pub mode: String,
    pub steps: Vec<StepOutcome>,
    /// Synthesis / loop-final / review summary (truncated).
    pub summary: String,
    pub success: bool,
    pub total_tokens: u64,
    /// Present when the workflow was interrupted (cancelled) — the model can
    /// resume it by calling the workflow tool with this id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_id: Option<String>,
}

/// Progress of an interrupted workflow — enough to resume from where it
/// stopped (cancel → user says continue → workflow tool with the resume id).
#[derive(Debug, Clone)]
pub struct WorkflowProgress {
    pub workflow_id: String,
    /// The original spec — fan_out consumes `steps` during execution, so the
    /// spec must be stored separately for resume.
    pub spec: WorkflowSpec,
    /// Resolved step outcomes (fan_out) — success ones are skipped on resume.
    pub completed: Vec<StepOutcome>,
    /// Completed loop rounds (loop_until_done).
    pub round: u32,
    /// Previous round history (loop_until_done resume).
    pub previous: Option<String>,
}

/// Hard cap on saved interrupted workflows per process — cancelled workflows
/// that are never resumed must not accumulate unboundedly in a long session.
const MAX_SAVED_WORKFLOWS: usize = 20;

/// Inner state — the progress map plus insertion order for eviction.
struct WorkflowStoreInner {
    progress: HashMap<String, WorkflowProgress>,
    order: std::collections::VecDeque<String>,
}

/// In-memory store for interrupted workflow progress (cancel → resume).
/// Session-scoped like `BackgroundTaskRegistry` — a workflow is a running
/// task within one process, not a durable artifact.
pub struct WorkflowStore {
    inner: Arc<Mutex<WorkflowStoreInner>>,
}

impl Default for WorkflowStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(WorkflowStoreInner {
                progress: HashMap::new(),
                order: std::collections::VecDeque::new(),
            })),
        }
    }
}

impl WorkflowStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Save (or overwrite) an interrupted workflow's progress.
    pub fn put(&self, progress: WorkflowProgress) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .progress
            .insert(progress.workflow_id.clone(), progress.clone());
        inner.order.push_back(progress.workflow_id.clone());
        // Evict oldest beyond the cap, skipping ids already removed.
        while inner.progress.len() > MAX_SAVED_WORKFLOWS {
            if let Some(oldest) = inner.order.pop_front() {
                inner.progress.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Get a workflow's saved progress, if any.
    pub fn get(&self, workflow_id: &str) -> Option<WorkflowProgress> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .get(workflow_id)
            .cloned()
    }

    /// Drop a workflow's progress (completed, or superseded).
    pub fn remove(&self, workflow_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.progress.remove(workflow_id);
    }
}

/// Resolve a subagent type string to the enum (same mapping as `agent`).
pub fn resolve_agent_type(value: Option<&str>) -> crate::agent::multi_agent::SubagentType {
    match value.unwrap_or("general") {
        "explore" => crate::agent::multi_agent::SubagentType::Explore,
        "plan" => crate::agent::multi_agent::SubagentType::Plan,
        "evaluator" => crate::agent::multi_agent::SubagentType::Evaluator,
        "general" => crate::agent::multi_agent::SubagentType::General,
        other => crate::agent::multi_agent::SubagentType::Custom(other.to_string()),
    }
}

/// Resolve isolation mode ("worktree" or anything else → None).
pub fn resolve_isolation(value: Option<&str>) -> crate::agent::multi_agent::IsolationMode {
    match value {
        Some("worktree") => crate::agent::multi_agent::IsolationMode::Worktree,
        _ => crate::agent::multi_agent::IsolationMode::None,
    }
}

/// The loop worker's terminal signal — parsed by the HARNESS, never left to
/// the parent model to self-assess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStatus {
    Done,
    Continue(String),
}

/// Parse `WORKFLOW_STATUS: DONE|CONTINUE` from a loop worker's report.
/// Whitespace/case tolerant; an absent or unparseable marker is treated as
/// Continue (safe direction — the round cap still bounds it).
pub fn parse_loop_status(report: &str) -> LoopStatus {
    for line in report.lines() {
        let upper = line.trim().to_uppercase();
        if let Some(idx) = upper.find("WORKFLOW_STATUS") {
            let tail = upper[idx + "WORKFLOW_STATUS".len()..].trim_start();
            if let Some(rest) = tail.strip_prefix(':') {
                let status = rest.trim();
                if status.starts_with("DONE") {
                    return LoopStatus::Done;
                }
                return LoopStatus::Continue(report.trim().to_string());
            }
        }
    }
    LoopStatus::Continue(report.trim().to_string())
}

/// Topologically sort pipeline steps into dependency levels.
///
/// Each level's steps are mutually independent and may run in parallel; level
/// N+1 runs only after every level-N step finished. Returns `Err` on a cycle,
/// a `depends_on` id that names no step, or a duplicate step id.
pub fn pipeline_levels(steps: &[WorkflowStep]) -> Result<Vec<Vec<usize>>, String> {
    let ids: HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    if ids.len() != steps.len() {
        return Err("pipeline step ids must be unique".to_string());
    }

    // Validate references and build the dependency edges.
    let mut indegree = vec![0usize; steps.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for (i, s) in steps.iter().enumerate() {
        for dep in &s.depends_on {
            let j = *ids.get(dep.as_str()).ok_or_else(|| {
                format!("step \"{}\" depends on unknown step \"{}\"", s.id, dep)
            })?;
            if i == j {
                return Err(format!("step \"{}\" depends on itself", s.id));
            }
            dependents[j].push(i);
            indegree[i] += 1;
        }
    }

    // Kahn's algorithm, level by level.
    let mut queue: std::collections::VecDeque<usize> = (0..steps.len())
        .filter(|&i| indegree[i] == 0)
        .collect();
    let mut levels: Vec<Vec<usize>> = Vec::new();
    let mut remaining = steps.len();
    while !queue.is_empty() {
        let mut level = Vec::with_capacity(queue.len());
        let mut next = std::collections::VecDeque::new();
        for i in queue {
            level.push(i);
            remaining -= 1;
            for &d in &dependents[i] {
                indegree[d] -= 1;
                if indegree[d] == 0 {
                    next.push_back(d);
                }
            }
        }
        levels.push(level);
        queue = next;
    }
    if remaining != 0 {
        return Err("pipeline has a dependency cycle".to_string());
    }
    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_parses_all_three_modes() {
        let fan: WorkflowSpec = serde_json::from_str(
            r#"{
                "mode": "fan_out",
                "steps": [{"id":"a","task":"survey A"},{"id":"b","task":"survey B","agent_type":"explore","isolation":"worktree"}],
                "synthesize": "merge",
                "verify": {"acceptance":"tests pass"},
                "max_parallel": 3
            }"#,
        )
        .unwrap();
        match fan {
            WorkflowSpec::FanOut {
                steps,
                synthesize,
                verify,
                max_parallel,
            } => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[1].agent_type.as_deref(), Some("explore"));
                assert_eq!(steps[1].isolation.as_deref(), Some("worktree"));
                assert_eq!(synthesize.as_deref(), Some("merge"));
                assert!(verify.is_some());
                assert_eq!(max_parallel, Some(3));
            }
            _ => panic!("expected fan_out"),
        }

        let loop_spec: WorkflowSpec = serde_json::from_str(
            r#"{"mode":"loop_until_done","task":"fix","stop_condition":"green","max_rounds":3}"#,
        )
        .unwrap();
        assert!(matches!(loop_spec, WorkflowSpec::LoopUntilDone { .. }));

        let review: WorkflowSpec = serde_json::from_str(
            r#"{"mode":"adversarial_review","task":"review","acceptance":"ok","edited_paths":["src/a.rs"]}"#,
        )
        .unwrap();
        assert!(matches!(review, WorkflowSpec::AdversarialReview { .. }));
    }

    #[test]
    fn pipeline_spec_parses_with_dependencies() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{
                "mode": "pipeline",
                "steps": [
                    {"id":"a","task":"render loop"},
                    {"id":"b","task":"collision","depends_on":["a"]},
                    {"id":"c","task":"scoring","depends_on":["a","b"]}
                ],
                "synthesize": "merge",
                "verify": {"acceptance":"runs"}
            }"#,
        )
        .unwrap();
        match spec {
            WorkflowSpec::Pipeline {
                steps,
                synthesize,
                verify,
                ..
            } => {
                assert_eq!(steps.len(), 3);
                assert!(steps[0].depends_on.is_empty());
                assert_eq!(steps[1].depends_on, vec!["a".to_string()]);
                assert_eq!(
                    steps[2].depends_on,
                    vec!["a".to_string(), "b".to_string()]
                );
                assert_eq!(synthesize.as_deref(), Some("merge"));
                assert!(verify.is_some());
            }
            _ => panic!("expected pipeline"),
        }
    }

    fn step(id: &str, depends_on: &[&str]) -> WorkflowStep {
        WorkflowStep {
            id: id.to_string(),
            task: format!("task {id}"),
            agent_type: None,
            model: None,
            max_turns: None,
            timeout_secs: None,
            paths: None,
            isolation: None,
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn pipeline_levels_topologically_sorts() {
        let steps = vec![
            step("a", &[]),
            step("b", &["a"]),
            step("c", &["a", "b"]),
            step("d", &["a"]),
        ];
        let levels = pipeline_levels(&steps).unwrap();
        assert_eq!(levels, vec![vec![0], vec![1, 3], vec![2]]);
    }

    #[test]
    fn pipeline_levels_detects_cycle() {
        let steps = vec![step("a", &["b"]), step("b", &["a"])];
        assert!(pipeline_levels(&steps).unwrap_err().contains("cycle"));
    }

    #[test]
    fn pipeline_levels_detects_unknown_dependency() {
        let steps = vec![step("a", &["ghost"])];
        assert!(pipeline_levels(&steps).unwrap_err().contains("unknown step"));
    }

    #[test]
    fn pipeline_levels_detects_self_dependency() {
        let steps = vec![step("a", &["a"])];
        assert!(pipeline_levels(&steps).unwrap_err().contains("itself"));
    }

    #[test]
    fn pipeline_levels_detects_duplicate_ids() {
        let steps = vec![step("a", &[]), step("a", &[])];
        assert!(pipeline_levels(&steps).unwrap_err().contains("unique"));
    }

    #[test]
    fn loop_status_parsing_is_tolerant() {
        assert_eq!(
            parse_loop_status("done.\nWORKFLOW_STATUS: DONE"),
            LoopStatus::Done
        );
        assert_eq!(
            parse_loop_status("  workflow_status :  done  "),
            LoopStatus::Done
        );
        assert!(matches!(
            parse_loop_status("progress\nWORKFLOW_STATUS: CONTINUE"),
            LoopStatus::Continue(_)
        ));
        // Absent marker → Continue (round cap bounds it).
        assert!(matches!(
            parse_loop_status("I made progress but not done"),
            LoopStatus::Continue(_)
        ));
    }

    #[test]
    fn agent_type_resolution_matches_agent_tool() {
        assert!(matches!(
            resolve_agent_type(Some("explore")),
            crate::agent::multi_agent::SubagentType::Explore
        ));
        assert!(matches!(
            resolve_agent_type(Some("market_manager")),
            crate::agent::multi_agent::SubagentType::Custom(name) if name == "market_manager"
        ));
        assert!(matches!(
            resolve_agent_type(None),
            crate::agent::multi_agent::SubagentType::General
        ));
    }

    fn sample_progress(id: &str) -> WorkflowProgress {
        WorkflowProgress {
            workflow_id: id.to_string(),
            spec: WorkflowSpec::AdversarialReview {
                task: "review".into(),
                acceptance: None,
                edited_paths: None,
            },
            completed: vec![StepOutcome {
                id: "a".into(),
                success: true,
                result: "done".into(),
                error: None,
                edited_files: vec!["src/a.rs".into()],
                tokens: 100,
            }],
            round: 2,
            previous: Some("history".into()),
        }
    }

    #[test]
    fn workflow_store_put_get_roundtrip() {
        let store = WorkflowStore::new();
        assert!(store.get("wf-1").is_none());
        store.put(sample_progress("wf-1"));
        let got = store.get("wf-1").expect("progress stored");
        assert_eq!(got.workflow_id, "wf-1");
        assert_eq!(got.round, 2);
        assert_eq!(got.completed.len(), 1);
        assert_eq!(got.completed[0].id, "a");
        assert_eq!(got.previous.as_deref(), Some("history"));
    }

    #[test]
    fn workflow_store_put_overwrites_same_id() {
        let store = WorkflowStore::new();
        store.put(sample_progress("wf-1"));
        let mut newer = sample_progress("wf-1");
        newer.round = 4;
        store.put(newer);
        assert_eq!(store.get("wf-1").map(|p| p.round), Some(4));
    }

    #[test]
    fn workflow_store_remove_drops_progress() {
        let store = WorkflowStore::new();
        store.put(sample_progress("wf-1"));
        store.remove("wf-1");
        assert!(store.get("wf-1").is_none());
    }

    #[test]
    fn workflow_store_evicts_oldest_beyond_cap() {
        let store = WorkflowStore::new();
        for i in 0..(MAX_SAVED_WORKFLOWS + 5) {
            store.put(sample_progress(&format!("wf-{i}")));
        }
        // The oldest are evicted; the newest survive.
        assert!(store.get("wf-0").is_none());
        assert!(store.get("wf-4").is_none());
        assert!(store.get(&format!("wf-{}", MAX_SAVED_WORKFLOWS + 4)).is_some());
        assert_eq!(store.get("wf-5").is_some(), true, "newer entry kept");
    }
}
