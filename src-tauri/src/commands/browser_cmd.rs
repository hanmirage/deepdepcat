//! Browser takeover commands — frontend control of the agent browser
//! (DevBrowserCard "接管浏览器" mode).

use crate::browser::{
    BrowserStatus, BrowserTab, LaunchOptions, DEFAULT_PROFILE, TAKEOVER_DEFAULT_TIMEOUT_SECS,
};
use crate::bootstrap::AppState;
use serde_json::Value;
use tauri::State;

/// Launch the agent browser. Optionally opens `url`; `headless` starts
/// without a window (batch automation) and `profile` picks a specific
/// isolated profile (default: `takeover`).
#[tauri::command]
pub async fn browser_takeover_start(
    url: Option<String>,
    headless: Option<bool>,
    profile: Option<String>,
    state: State<'_, AppState>,
) -> Result<BrowserStatus, String> {
    let opts = LaunchOptions {
        profile: profile.unwrap_or_else(|| crate::browser::DEFAULT_PROFILE.to_string()),
        headless: headless.unwrap_or(false),
    };
    state
        .browser
        .start_for(url.as_deref(), opts.clone())
        .await
        .map_err(String::from)?;
    Ok(state.browser.status_for(&opts.profile).await)
}

/// Stop the agent browser. Returns whether one was running.
#[tauri::command]
pub async fn browser_takeover_stop(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.browser.stop().await)
}

/// Current session status (running + live URL/title + takeover state).
/// `profile` selects the browser session (default: `takeover` — the agent's
/// per-conversation browser is `session-<id>`).
#[tauri::command]
pub async fn browser_takeover_status(
    profile: Option<String>,
    state: State<'_, AppState>,
) -> Result<BrowserStatus, String> {
    Ok(state.browser.status_for(&profile.unwrap_or_else(|| DEFAULT_PROFILE.to_string())).await)
}

/// Navigate the agent browser to `url` and wait for the page to load.
#[tauri::command]
pub async fn browser_takeover_navigate(
    url: String,
    state: State<'_, AppState>,
) -> Result<BrowserStatus, String> {
    state.browser.navigate(&url).await.map_err(String::from)?;
    Ok(state.browser.status().await)
}

/// Capture a screenshot of the agent browser as base64 PNG (UI preview).
#[tauri::command]
pub async fn browser_takeover_screenshot(state: State<'_, AppState>) -> Result<String, String> {
    state.browser.screenshot_png().await.map_err(String::from)
}

/// Return the agent browser's captured console/network/error logs
/// (injects the capture hook on first call; persisted to disk alongside).
#[tauri::command]
pub async fn browser_takeover_logs(state: State<'_, AppState>) -> Result<Value, String> {
    let (logs, _persisted) = state.browser.capture_logs().await.map_err(String::from)?;
    Ok(logs)
}

/// Complete a pending user takeover ("我已接管完成，继续").
/// Returns whether a takeover was actually pending.
#[tauri::command]
pub async fn browser_takeover_resume(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.browser.resume().await)
}

/// Structured tab list for the frontend tab strip (empty when not running).
#[tauri::command]
pub async fn browser_tabs(state: State<'_, AppState>) -> Result<Vec<BrowserTab>, String> {
    state.browser.tabs_snapshot().await.map_err(String::from)
}

/// Open a new tab (optionally at `url`) and switch to it; returns the tab list.
#[tauri::command]
pub async fn browser_tab_new(
    url: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<BrowserTab>, String> {
    state
        .browser
        .tab_new_for(DEFAULT_PROFILE, url.as_deref().unwrap_or("about:blank"))
        .await
        .map_err(String::from)?;
    state.browser.tabs_snapshot().await.map_err(String::from)
}

/// Switch the active tab; returns the tab list.
#[tauri::command]
pub async fn browser_tab_switch(
    target_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<BrowserTab>, String> {
    state
        .browser
        .tab_switch_for(DEFAULT_PROFILE, &target_id)
        .await
        .map_err(String::from)?;
    state.browser.tabs_snapshot().await.map_err(String::from)
}

/// Close a tab (the active one when None); returns the tab list.
#[tauri::command]
pub async fn browser_tab_close(
    target_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<BrowserTab>, String> {
    state
        .browser
        .tab_close_for(DEFAULT_PROFILE, target_id.as_deref())
        .await
        .map_err(String::from)?;
    state.browser.tabs_snapshot().await.map_err(String::from)
}

/// The default takeover timeout, exposed for the frontend countdown label.
#[tauri::command]
pub async fn browser_takeover_default_timeout() -> Result<u64, String> {
    Ok(TAKEOVER_DEFAULT_TIMEOUT_SECS)
}

/// Start streaming live frames for the given browser session (idempotent).
/// The frontend starts this when the embedded view is visible and stops it
/// on unmount, so frames only flow while someone is looking. `profile`
/// defaults to `takeover`; the agent's per-conversation browser is
/// `session-<id>`.
#[tauri::command]
pub async fn browser_screencast_start(
    app: tauri::AppHandle,
    profile: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let key = profile.unwrap_or_else(|| DEFAULT_PROFILE.to_string());
    state.screencast.start(app, &state.browser, &key).await.map_err(String::from)
}

/// Stop the given browser session's frame stream.
#[tauri::command]
pub async fn browser_screencast_stop(
    profile: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let key = profile.unwrap_or_else(|| DEFAULT_PROFILE.to_string());
    state.screencast.stop(&key);
    Ok(())
}

/// Forward one input event to a browser session — the embedded view drives
/// the real page. `kind` ∈ mouse/wheel/key/text (see `input_for`).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn browser_takeover_input(
    state: State<'_, AppState>,
    kind: String,
    event: Option<String>,
    x: Option<i32>,
    y: Option<i32>,
    buttons: Option<i32>,
    click_count: Option<i32>,
    delta_x: Option<i32>,
    delta_y: Option<i32>,
    key: Option<String>,
    code: Option<String>,
    text: Option<String>,
    profile: Option<String>,
) -> Result<(), String> {
    let profile = profile.unwrap_or_else(|| DEFAULT_PROFILE.to_string());
    state
        .browser
        .input_for(
            &profile,
            &kind,
            x.unwrap_or(0),
            y.unwrap_or(0),
            buttons.unwrap_or(0),
            click_count.unwrap_or(0),
            delta_x.unwrap_or(0),
            delta_y.unwrap_or(0),
            event.as_deref().unwrap_or(""),
            key.as_deref().unwrap_or(""),
            code.as_deref().unwrap_or(""),
            text.as_deref().unwrap_or(""),
        )
        .await
        .map_err(String::from)
}
