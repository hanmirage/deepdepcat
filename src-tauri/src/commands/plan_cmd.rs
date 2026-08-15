//! Plan-approval commands — the frontend's side of the "pause & plan" loop.

use crate::bootstrap::AppState;
use crate::permissions::plan::PlanDecision;
use tauri::State;

/// Respond to a plan-approval request emitted by the backend.
///
/// `decision` is "approve" or "reject". A rejection carries optional
/// `feedback` which is returned to the agent (sanitized) so it can revise
/// the plan and re-submit.
#[tauri::command]
pub async fn respond_plan_approval(
    request_id: String,
    decision: String,
    feedback: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let resolved = match decision.as_str() {
        "approve" => {
            state
                .respond_plan_approval(&request_id, PlanDecision::Approved)
                .await
        }
        "reject" => {
            state
                .respond_plan_approval(
                    &request_id,
                    PlanDecision::Rejected(feedback.unwrap_or_default()),
                )
                .await
        }
        _ => return Err(format!("Unknown plan decision: {decision}")),
    };
    if resolved.is_none() {
        return Err("No pending plan approval with that request id".to_string());
    }
    Ok(())
}
