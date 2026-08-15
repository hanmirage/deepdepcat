//! browser_control — drive a real Chromium browser via CDP (both modes).
//!
//! Gives the agent a real logged-in-style browser for tasks that need one:
//! navigating, clicking by visible text, filling forms, typing,
//! screenshotting — plus a **human-in-the-loop handoff** for captchas,
//! logins and confirmations. Available in both Code and Depwork.
//!
//! The browser runs in a dedicated agent profile (logins persist across
//! sessions) and is visible on screen — the user can always watch it in the
//! right panel's live "browser" pane or take over. `handoff` pauses the
//! agent until the user finishes acting in the window and clicks
//! "我已接管完成，继续" in the app.
//!
//! The visual loop: `read_page` returns the interactive-element list,
//! page-changing actions auto-save a screenshot whose path the model can
//! `visual_describe` — 看→点→看.
//!
//! Actions:
//! - `start` — launch the agent browser (optionally opening `url`)
//! - `stop` — close the agent browser
//! - `status` — running state + current URL/title
//! - `navigate` — go to `url` (waits for load)
//! - `read_page` — URL + title + visible text + interactive elements
//! - `tabs` / `tab_new` / `tab_switch` / `tab_close` — multi-tab control
//! - `screenshot` — save a PNG to the app data dir; returns the path
//! - `downloads` — list the isolated download dir (newest first; `.crdownload` = in progress)
//! - `logs` — read console/network/error logs AND persist them to disk
//! - `click` — click the first element whose visible text contains `text`
//! - `fill` — fill the input matching `into` (placeholder/label/name)
//! - `type` — type into the currently focused element
//! - `press` — press a named key (enter/tab/esc/backspace/delete/space/arrows/…)
//! - `handoff` — pause; wait for the user to act in the browser window
//! - `wait` — sleep `ms` milliseconds

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::browser::session::{sanitize_profile_key, session_profile_key};
use crate::browser::{BrowserManager, LaunchOptions, TAKEOVER_DEFAULT_TIMEOUT_SECS};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Browser takeover tool.
pub struct BrowserControlTool;

impl BrowserControlTool {
    pub fn new() -> Self {
        Self
    }
}

/// Fetch the shared browser manager from the tool context.
fn manager(context: &ToolContext) -> AppResult<std::sync::Arc<BrowserManager>> {
    use tauri::Manager as _;
    let state = context.app.state::<AppState>();
    Ok(state.browser.clone())
}

/// Save a base64 PNG screenshot into the app data dir; returns the path.
pub(crate) async fn save_screenshot(context: &ToolContext, base64_png: &str) -> AppResult<String> {
    use tauri::Manager as _;
    let dir = context
        .app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(e.to_string()))?
        .join("browser-screenshots");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("browser-{stamp}.png"));
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_png)
        .map_err(|e| AppError::Other(format!("screenshot decode failed: {e}")))?;
    std::fs::write(&path, bytes)?;
    Ok(path.to_string_lossy().to_string())
}

/// After an action that changes the page, capture + persist a screenshot so
/// the model can `visual_describe` the result — the "看→点→看" loop.
/// Best-effort: any failure yields an empty suffix (the action result still
/// stands).
async fn auto_screenshot_suffix(
    context: &ToolContext,
    mgr: &BrowserManager,
    profile: &str,
    enabled: bool,
) -> String {
    if !enabled {
        return String::new();
    }
    match mgr.screenshot_for(profile).await {
        Ok(png) => match save_screenshot(context, &png).await {
            Ok(path) => format!("\n[截图] {path} — 可调 visual_describe 查看当前页面"),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    }
}

#[async_trait]
impl Tool for BrowserControlTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::All
    }

    fn name(&self) -> &str {
        "browser_control"
    }

    fn description(&self) -> &str {
        "Drive a real Chromium browser (dedicated agent profile, logins persist): \
         navigate/click/fill/type/screenshot pages that block plain HTTP fetches or \
         need a logged-in session. Each conversation owns an ISOLATED browser \
         (its own profile: cookies/logins/downloads never leak across sessions or \
         into your personal browser). Actions: start(url?, headless?, profile?), \
         stop, status, navigate(url), \
         read_page(max_chars) — returns the page text PLUS the interactive-element \
         list (buttons/links/inputs with placeholders) so you know what you can \
         click/fill; snapshot — element-level list with stable eids (use \
         click_eid(id)/fill_eid(id,text) for exact element targeting instead of \
         text/CSS matching); downloads — list files that landed in this profile's \
         isolated download dir (⏳ = still downloading); logs — read the \
         page's console/network/error logs and persist them to \
         browser-logs/<profile>.jsonl; tabs, tab_new(url), tab_switch(id), tab_close(id?) — the active \
         tab follows tab_switch/tab_new; screenshot, click(text), fill(into=placeholder/\
         label text, text), type(text), press(key), handoff(reason) — handoff PAUSES \
         and waits for the user to handle a captcha/login/confirmation in the visible \
         browser window, then continues after they click continue; wait(ms). navigate/\
         click/fill/type/press auto-save a screenshot and return its path \
         (visual_describe it; pass auto_screenshot=false to skip). The browser window \
         is visible by default — never touch content the user is working on. For \
         UNATTENDED batch automation pass headless=true (no window; handoff is \
         rejected in headless). profile defaults to this conversation's isolated \
         profile; pass profile=\"shared\" to reuse one login across your own \
         conversations."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "stop", "status", "navigate", "read_page", "snapshot", "screenshot", "downloads", "logs", "tabs", "tab_new", "tab_switch", "tab_close", "click", "click_css", "click_eid", "fill", "fill_css", "fill_eid", "type", "press", "handoff", "wait", "wait_for", "eval", "scroll"],
                    "description": "Operation to perform."
                },
                "url": { "type": "string", "description": "Target URL (start/navigate/tab_new)." },
                "headless": { "type": "boolean", "description": "Launch without a window for unattended batch automation (start only, default false)." },
                "profile": { "type": "string", "description": "Profile key for session isolation (start only; default = this conversation's own profile)." },
                "max_chars": { "type": "integer", "description": "Max page-text characters (read_page, default 8000)." },
                "text": { "type": "string", "description": "Visible text to click (click), text to type (type) or text to enter into the field (fill)." },
                "into": { "type": "string", "description": "Form field to fill, matched by placeholder/aria-label/name/label text (fill)." },
                "auto_screenshot": { "type": "boolean", "description": "Save a screenshot after navigate/click/fill/type/press and return its path (default true)." },
                "id": { "type": "string", "description": "Tab target id from tabs (tab_switch/tab_close) or element eid from snapshot (click_eid/fill_eid)." },
                "key": { "type": "string", "description": "Key name: enter/tab/esc/backspace/delete/space/up/down/left/right/home/end/pageup/pagedown (press)." },
                "reason": { "type": "string", "description": "Why the user must take over (handoff)." },
                "timeout_secs": { "type": "integer", "description": "Max seconds to wait for the user (handoff, default 600)." },
                  "ms": { "type": "integer", "description": "Milliseconds to wait (wait)." }
                  ,
                  "selector": { "type": "string", "description": "CSS selector (click_css/fill_css/wait_for)." },
                  "expression": { "type": "string", "description": "JavaScript to evaluate on the page (eval)." },
                  "direction": { "type": "string", "description": "Scroll direction: up/down/left/right/top/bottom or 'x,y' pixels (scroll)." }
              },
            "required": ["action"]
        })
    }

    /// Read actions never prompt — per-call read classification consumed by
    /// the permission pipeline (read_only flag for THIS invocation).
    fn is_read_only_call(&self, args: &Value) -> bool {
        permission_for_action(args.get("action").and_then(|a| a.as_str()))
            == PermissionDecision::Allow
    }

    /// One shared browser — parallel tool calls would fight over the
    /// session, so the dispatcher serializes this tool.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?;
        let mgr = manager(context)?;
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let headless = args
            .get("headless")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let profile_arg = args.get("profile").and_then(|v| v.as_str()).unwrap_or("");
        let profile = resolve_profile(
            &context.session_id,
            (!profile_arg.is_empty()).then_some(profile_arg),
        )?;
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(8000) as usize;
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let into = args
            .get("into")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let auto_shot = args
            .get("auto_screenshot")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("user action required")
            .to_string();
        let timeout = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(TAKEOVER_DEFAULT_TIMEOUT_SECS);
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ms = args.get("ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let expression = args
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut out = match action {
            "start" => {
                let out = mgr
                    .start_for(
                        (!url.is_empty()).then_some(url.as_str()),
                        LaunchOptions {
                            profile: profile.clone(),
                            headless,
                        },
                    )
                    .await?;
                // The frontend opens its live "browser" pane on this signal.
                crate::browser::broadcast_status_changed(&context.app, &profile, true);
                out
            }
            "stop" => {
                let was = mgr.stop_for(&profile).await;
                if was {
                    crate::browser::broadcast_status_changed(&context.app, &profile, false);
                    format!("Agent browser stopped (profile: {profile})")
                } else {
                    format!("No agent browser was running for profile {profile}")
                }
            }
            "status" => {
                let s = mgr.status_for(&profile).await;
                if !s.running {
                    format!("Agent browser is not running for profile {profile} — use start first")
                } else {
                    let page = match (s.url.as_deref(), s.title.as_deref()) {
                        (Some(u), Some(t)) if !t.is_empty() => format!("{u} — {t}"),
                        (Some(u), _) => u.to_string(),
                        _ => "page loaded".to_string(),
                    };
                    let mode = if s.headless { "headless" } else { "visible" };
                    let dl = s
                        .download_dir
                        .as_deref()
                        .map(|d| format!(", downloads: {d}"))
                        .unwrap_or_default();
                    if s.awaiting_takeover {
                        format!("Agent browser running ({page}, {mode}{dl}) — AWAITING USER TAKEOVER: {}",
                                s.takeover_reason.as_deref().unwrap_or("user action"))
                    } else {
                        format!("Agent browser running ({page}, {mode}{dl})")
                    }
                }
            }
            "navigate" => {
                if url.trim().is_empty() {
                    return Err("Missing required parameter: url".into());
                }
                mgr.navigate_for(&profile, &url).await?
            }
            "read_page" => mgr.read_page_for(&profile, max_chars).await?,
            "tabs" => mgr.tabs_for(&profile).await?,
            "tab_new" => {
                if url.trim().is_empty() {
                    return Err("Missing required parameter: url".into());
                }
                mgr.tab_new_for(&profile, &url).await?
            }
            "tab_switch" => {
                if id.is_empty() {
                    return Err("Missing required parameter: id".into());
                }
                mgr.tab_switch_for(&profile, &id).await?
            }
            "tab_close" => {
                let target = (!id.is_empty()).then_some(id.as_str());
                mgr.tab_close_for(&profile, target).await?
            }
            "screenshot" => {
                let png = mgr.screenshot_for(&profile).await?;
                let path = save_screenshot(context, &png).await?;
                format!(
                    "Screenshot saved to {path} (use visual_describe on this path to see the page)"
                )
            }
            "downloads" => mgr.downloads_for(&profile).await?,
            "logs" => {
                let (logs, persisted) = mgr.capture_logs_for(&profile).await?;
                let pretty = serde_json::to_string_pretty(&logs).unwrap_or_default();
                format!(
                    "浏览器日志已持久化到 {}：\n{}",
                    persisted.display(),
                    crate::core::str_util::truncate_at_char_boundary(&pretty, 4000)
                )
            }
            "click" => {
                if text.trim().is_empty() {
                    return Err("Missing required parameter: text".into());
                }
                mgr.click_by_text_for(&profile, &text).await?
            }
            "snapshot" => mgr.element_snapshot_for(&profile).await?,
            "click_eid" => {
                if id.trim().is_empty() {
                    return Err("Missing required parameter: id (element eid)".into());
                }
                mgr.click_eid_for(&profile, &id).await?
            }
            "fill" => {
                if into.trim().is_empty() {
                    return Err("Missing required parameter: into".into());
                }
                if text.is_empty() {
                    return Err("Missing required parameter: text".into());
                }
                mgr.fill_for(&profile, &into, &text).await?
            }
            "fill_eid" => {
                if id.trim().is_empty() {
                    return Err("Missing required parameter: id (element eid)".into());
                }
                mgr.fill_eid_for(&profile, &id, &text).await?
            }
            "type" => {
                if text.is_empty() {
                    return Err("Missing required parameter: text".into());
                }
                mgr.type_text_for(&profile, &text).await?
            }
            "press" => {
                if key.is_empty() {
                    return Err("Missing required parameter: key".into());
                }
                mgr.press_key_for(&profile, &key).await?
            }
            "handoff" => {
                mgr.handoff_for(&profile, &reason, timeout.max(1), &context.app)
                    .await?
            }
            "eval" => mgr.eval_js_for(&profile, &expression).await?,
            "click_css" => mgr.click_css_for(&profile, &selector).await?,
            "fill_css" => mgr.fill_css_for(&profile, &selector, &text).await?,
            "scroll" => mgr.scroll_for(&profile, &direction).await?,
            "wait_for" => {
                mgr.wait_for_profile(
                    &profile,
                    (!selector.is_empty()).then_some(selector.as_str()),
                    (!text.is_empty()).then_some(text.as_str()),
                    timeout.clamp(1, 120),
                )
                .await?
            }
            "wait" => {
                tokio::time::sleep(std::time::Duration::from_millis(ms.min(300_000))).await;
                format!("Waited {ms} ms")
            }
            other => {
                return Err(format!(
                    "Unknown action: {other}. Use start/stop/status/navigate/read_page/screenshot/downloads/logs/tabs/tab_new/tab_switch/tab_close/click/click_css/fill/fill_css/type/press/handoff/wait/wait_for/eval/scroll"
                )
                .into());
            }
        };
        // 看→点→看 loop: page-changing actions leave a screenshot path the
        // model can visual_describe.
        if matches!(action, "navigate" | "click" | "fill" | "type" | "press") {
            out.push_str(&auto_screenshot_suffix(context, &mgr, &profile, auto_shot).await);
        }
        Ok(ToolResult::success(out))
    }
}

/// Resolve the browser profile key for a tool call: an explicit `profile`
/// wins, otherwise the conversation's own isolated profile (`session-<id>`).
fn resolve_profile(session_id: &str, explicit: Option<&str>) -> AppResult<String> {
    let raw = match explicit {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => session_profile_key(session_id),
    };
    sanitize_profile_key(&raw)
}

/// Permission policy for an action — a pure function so it stays testable
/// without a Tauri app handle (Windows test threads cannot create a Wry
/// event loop for `tauri::test::mock_app`).
fn permission_for_action(action: Option<&str>) -> PermissionDecision {
    match action {
        Some("status") | Some("read_page") | Some("screenshot") | Some("downloads")
        | Some("logs") | Some("wait") | Some("wait_for") | Some("tabs") => {
            PermissionDecision::Allow
        }
        _ => PermissionDecision::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_policy_reads_are_allowed() {
        for action in [
            "status",
            "read_page",
            "screenshot",
            "downloads",
            "logs",
            "wait",
            "wait_for",
            "tabs",
        ] {
            assert_eq!(
                permission_for_action(Some(action)),
                PermissionDecision::Allow,
                "{action} must be allowed"
            );
        }
        for action in [
            "start",
            "stop",
            "navigate",
            "tab_new",
            "tab_switch",
            "tab_close",
            "click",
            "click_css",
            "fill",
            "fill_css",
            "type",
            "press",
            "handoff",
            "eval",
            "scroll",
        ] {
            assert_eq!(
                permission_for_action(Some(action)),
                PermissionDecision::Ask,
                "{action} must ask"
            );
        }
        assert_eq!(permission_for_action(None), PermissionDecision::Ask);
        assert_eq!(permission_for_action(Some("nope")), PermissionDecision::Ask);
    }

    #[test]
    fn browser_tool_is_not_concurrency_safe() {
        let tool = BrowserControlTool::new();
        assert!(!tool.is_concurrency_safe());
    }

    #[test]
    fn browser_tool_is_available_in_both_modes() {
        // The real-browser tool must be usable in Code AND Depwork (the
        // frontend's live "browser" pane mirrors it in either mode).
        let tool = BrowserControlTool::new();
        assert!(matches!(
            tool.scope(),
            crate::toolkit::ToolScope::All
        ));
    }

    #[test]
    fn profile_key_defaults_to_the_conversation() {
        assert_eq!(resolve_profile("conv-42", None).unwrap(), "session-conv-42");
        assert_eq!(
            resolve_profile("conv-42", Some("")).unwrap(),
            "session-conv-42",
            "empty explicit profile falls back to the session"
        );
    }

    #[test]
    fn explicit_profile_wins_and_is_sanitized() {
        assert_eq!(
            resolve_profile("conv-42", Some(" shared ")).unwrap(),
            "shared"
        );
        assert_eq!(resolve_profile("conv-42", Some("a/b:c")).unwrap(), "a-b-c");
        assert!(
            resolve_profile("conv-42", Some("  ")).is_ok(),
            "whitespace falls back"
        );
        assert!(resolve_profile("conv-42", Some("中文")).is_err());
    }
}
