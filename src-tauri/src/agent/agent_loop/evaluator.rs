//! Independent evaluator review — the generator-evaluator loop.
//!
//! In `AgentLoopMode::EvaluatorQa`, when the generator is about to stop, an
//! INDEPENDENT evaluator subagent reviews the produced work. The evaluator:
//!
//! - runs in a **fresh, isolated context** — it never sees the generator's
//!   reasoning, only the task and the files the generator touched;
//! - holds a **skeptical review contract** (see `build_subagent_prompt` in
//!   spawn.rs) — the generator's self-report is untrusted, every claim must
//!   be verified against the actual code or a real run;
//! - has **verification-only tools** — read-only inspection + bash (run
//!   tests/builds) + LSP diagnostics; it can never mutate the codebase;
//! - prefers the verify-role model when configured (same "judge seat" as the
//!   Reflexion critique).
//!
//! A FAIL verdict injects the findings back into the generator's context and
//! forces one more fix round, up to [`MAX_EVALUATOR_ROUNDS`]. A PASS (or the
//! round cap) lets the turn end.

use crate::agent::chat_state::ChatState;
use crate::agent::multi_agent::{SubagentConfig, SubagentResult, SubagentType};
use crate::core::error::AppResult;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// How many FAIL-driven fix rounds run at most after the initial review.
/// Bounded so a stubborn generator cannot loop forever on reviewer feedback.
pub const MAX_EVALUATOR_ROUNDS: u32 = 2;

/// The outcome of an evaluator review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluatorVerdict {
    /// The work satisfies the task's acceptance criteria.
    Pass,
    /// The work does not satisfy them; `findings` carries the reviewer's
    /// concrete evidence (file:line + what was run).
    Fail { findings: String },
}

/// Parse the evaluator's report. The contract (build_subagent_prompt) is a
/// `VERDICT: PASS|FAIL` line plus a `FINDINGS:` bullet list. Parsing is
/// tolerant: a report without an explicit verdict is treated as FAIL (the
/// generator must not stop on an unreviewed state), and the full report text
/// becomes the findings when no section marker exists.
pub fn parse_evaluator_report(report: &str) -> EvaluatorVerdict {
    for line in report.lines() {
        let trimmed = line.trim();
        // Case- AND whitespace-insensitive on purpose: models may emit
        // `VERDICT: pass`, `verdict : PASS`, `Verdict:PASS`, etc. Everything
        // runs on the uppercased form so byte indexes stay consistent.
        let upper = trimmed.to_uppercase();
        if let Some(idx) = upper.find("VERDICT") {
            let tail = upper[idx + "VERDICT".len()..].trim_start();
            if let Some(rest) = tail.strip_prefix(':') {
                let verdict = rest.trim();
                if verdict.starts_with("PASS") {
                    return EvaluatorVerdict::Pass;
                }
                return EvaluatorVerdict::Fail {
                    findings: report.trim().to_string(),
                };
            }
        }
    }
    // No verdict line — treat as FAIL: a report that cannot be parsed is not
    // a license to stop. The raw report is the findings.
    EvaluatorVerdict::Fail {
        findings: report.trim().to_string(),
    }
}

/// Run the independent evaluator review for the current turn.
///
/// Builds an Evaluator subagent with an isolated context: the task
/// (original user prompt) plus the list of files the generator touched —
/// never the generator's conversation. The verdict is parsed from the
/// reviewer's report. On spawn/review failure the review is skipped
/// (Err) rather than blocking the turn.
///
/// `acceptance` carries the user's stated definition-of-done (e.g.
/// "tests pass") when one was extracted from the request — the reviewer
/// verifies against explicit criteria instead of inferring them.
///
/// A free function (not an `AgentLoop` method) on purpose: the spawned
/// evaluator constructs its own `AgentLoop` (Standard mode), and tying this
/// to `AgentLoop` would create a compiler-visible recursive async future.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_evaluator_review(
    app: &AppHandle,
    session_id: &str,
    task: &str,
    edited_paths: &[String],
    work_mode: crate::toolkit::WorkMode,
    agent_deny_rules: Vec<String>,
    cancellation_token: &CancellationToken,
    acceptance: Option<&str>,
) -> AppResult<EvaluatorVerdict> {
    let state = app.state::<crate::bootstrap::AppState>();
    let coordinator = &state.coordinator;
    if !coordinator.is_enabled() {
        // Do NOT return `Pass` here — the work was not independently
        // reviewed, so claiming "passed" is a lie that silently disables the
        // quality backstop. Surface it as an Err so the caller logs and ends
        // the turn as "review skipped", never as "review passed".
        return Err(crate::core::error::AppError::internal(
            "independent evaluator review skipped: multi-agent is disabled",
        ));
    }

    let model = coordinator
        .role_model(&SubagentType::Evaluator, None)
        .unwrap_or_else(|| coordinator.default_model().to_string());

    let review_targets = if edited_paths.is_empty() {
        "the changes are in the workspace — locate them with read/search tools.".to_string()
    } else {
        format!(
            "the following files were changed by the generator:\n{}",
            edited_paths
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    // Explicit acceptance criteria make the review verifiable instead
    // of inferred (evaluator-optimizer pattern: clear evaluation
    // criteria are a precondition for useful feedback).
    let acceptance_section = acceptance
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| format!("\n\n## Acceptance criteria\n{c}"))
        .unwrap_or_default();

    let review_task = format!(
        "Independently review the work done for the following task.\n\n\
             ## Task\n{task}\n{acceptance_section}\n\n\
             ## Generator's changes\n{review_targets}\n\n\
             Verify every acceptance criterion against the actual code and a \
             real run (tests/build/LSP diagnostics). For EACH acceptance \
             criterion report a separate line with its own verdict and \
             evidence — PASS with the command/output or file:line that \
             proves it, FAIL with the file:line or failing output. End with \
             the overall evaluator contract verdict. Do NOT modify any files."
    );

    let config = SubagentConfig {
        agent_type: SubagentType::Evaluator,
        task: review_task,
        model: Some(model.clone()),
        max_turns: 20,
        depth: 1,
        background: false,
        surface_completion: false,
        isolation: crate::agent::multi_agent::IsolationMode::None,
        timeout_secs: Some(600),
        task_id: None,
        call_id: None,
        fork: false,
        fork_context: Vec::new(),
        work_mode: Some(work_mode.as_str().to_string()),
        session_id: Some(session_id.to_string()),
        paths: None,
        image_notes: Vec::new(),
        // Even an independent evaluator is a child of the same agent
        // contract — the parent's deny chain applies to it too (M9 hard
        // veto propagation; the read-only gate remains the first line).
        inherited_denies: agent_deny_rules,
    };

    info!(
        session_id,
        edited_files = edited_paths.len(),
        "Running independent evaluator review"
    );

    // Isolated parent state — the evaluator must NOT inherit the
    // generator's conversation (that is what makes it independent).
    let parent_state = ChatState::with_provider(
        model.clone(),
        coordinator.default_context_window(),
        coordinator.default_provider().map(str::to_string),
    );

    let result: SubagentResult = coordinator
        .spawn_subagent_with_cancel(&config, &parent_state, app, cancellation_token)
        .await?;

    let verdict = parse_evaluator_report(&result.response);
    match &verdict {
        EvaluatorVerdict::Pass => info!(session_id, "Evaluator review: PASS"),
        EvaluatorVerdict::Fail { findings } => info!(
            session_id,
            findings_len = findings.len(),
            "Evaluator review: FAIL"
        ),
    }
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_pass_is_pass() {
        let report = "VERDICT: PASS\nFINDINGS:";
        assert_eq!(parse_evaluator_report(report), EvaluatorVerdict::Pass);
    }

    #[test]
    fn explicit_fail_carries_findings() {
        let report = "VERDICT: FAIL\nFINDINGS:\n- [CRITICAL] src/main.rs:12 — x";
        match parse_evaluator_report(report) {
            EvaluatorVerdict::Fail { findings } => assert!(findings.contains("src/main.rs:12")),
            EvaluatorVerdict::Pass => panic!("FAIL report must not parse as PASS"),
        }
    }

    #[test]
    fn verdict_is_case_and_whitespace_tolerant() {
        assert_eq!(
            parse_evaluator_report("  verdict :  pass  "),
            EvaluatorVerdict::Pass
        );
    }

    #[test]
    fn no_verdict_line_defaults_to_fail() {
        // A report that cannot be parsed is NOT a license to stop — the
        // generator must not end the turn on an unreviewed state.
        let report = "I reviewed everything and it looks great.";
        assert_eq!(
            parse_evaluator_report(report),
            EvaluatorVerdict::Fail {
                findings: report.to_string()
            }
        );
    }

    #[test]
    fn fail_without_findings_still_fails() {
        assert_eq!(
            parse_evaluator_report("VERDICT: FAIL"),
            EvaluatorVerdict::Fail {
                findings: "VERDICT: FAIL".to_string()
            }
        );
    }

    /// REAL DeepSeek smoke: the evaluator contract (`VERDICT: PASS|FAIL` +
    /// `FINDINGS:` list) must parse from live model output — runs only when
    /// DEEPSEEK_API_KEY is set.
    #[tokio::test]
    #[ignore = "requires a real DEEPSEEK_API_KEY"]
    async fn real_deepseek_evaluator_contract_smoke() {
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

        // The same review contract the real evaluator subagent receives.
        let review_task = "Independently review the work done for the following task.\n\n\
             ## Task\nFix the login 401 by handling expired tokens.\n\n\
             ## Acceptance criteria\ntests pass\n\n\
             ## Generator's changes\n- src/auth.rs\n- tests/auth_test.rs\n\n\
             Verify every acceptance criterion against the actual code and a \
             real run (tests/build/LSP diagnostics). Report per the evaluator \
             contract. Do NOT modify any files.\n\n\
             Evaluator contract: end your report with a line \
             'VERDICT: PASS' or 'VERDICT: FAIL' followed by 'FINDINGS:' and \
             a bullet list of concrete evidence (file:line + what was run).";

        let req = LlmRequest {
            model: "deepseek-chat".to_string(),
            provider: Some("deepseek".to_string()),
            messages: vec![ConversationItem::user(review_task.to_string())],
            tools: vec![],
            system_prompt: String::new(),
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(300),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };
        let resp = client
            .complete(&req)
            .await
            .expect("live DeepSeek evaluator-contract call must succeed");
        let report = resp.content.trim();
        eprintln!("report: {report:?}");
        assert!(!report.is_empty(), "evaluator must produce a report");
        let verdict = parse_evaluator_report(report);
        eprintln!("parsed verdict: {verdict:?}");
        // The contract must yield a decisive verdict either way — a report
        // the parser cannot read as PASS is FAIL (never an unreviewed stop).
        match verdict {
            EvaluatorVerdict::Pass => {
                assert!(
                    report.to_uppercase().contains("VERDICT"),
                    "PASS parse must come from an explicit verdict line"
                );
            }
            EvaluatorVerdict::Fail { findings } => {
                assert!(
                    !findings.trim().is_empty(),
                    "FAIL must carry findings for the generator"
                );
            }
        }
    }
}
