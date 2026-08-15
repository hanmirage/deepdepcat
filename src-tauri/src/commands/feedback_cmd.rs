//! Cloud feedback commands — user message feedback (like/dislike) uploaded
//! to the website's `/api/feedback` (public endpoint, no auth required).
//!
//! The website API has no CORS configuration, so the desktop app must call
//! it from the Rust side (reqwest — native HTTP, unaffected by CORS).

use tauri::State;

use crate::bootstrap::AppState;

/// Args for submitting user feedback about an assistant message.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SubmitFeedbackArgs {
    pub server_url: String,
    /// 1-5 rating (5 = like, 1 = dislike in the UI).
    pub rating: u8,
    /// Feedback text (message excerpt) — must be ≥ 5 chars per the API.
    pub message: String,
    /// "bug" | "feature" | "general" | "praise" | "subscribe".
    pub category: String,
}

/// Submit feedback to the website's public `/api/feedback` endpoint.
///
/// Best-effort: failures are returned to the caller (the UI ignores them —
/// feedback must never block or bother the user).
#[tauri::command]
pub async fn submit_feedback(
    args: SubmitFeedbackArgs,
    _state: State<'_, AppState>,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let url = format!("{}/api/feedback", args.server_url);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "rating": args.rating,
            "message": args.message,
            "category": args.category,
        }))
        .send()
        .await
        .map_err(|e| format!("Feedback request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    Ok(())
}
