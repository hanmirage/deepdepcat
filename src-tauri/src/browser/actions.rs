//! Profile-keyed browser actions.
//!
//! Every operation selects its session by profile key (`*_for`), with
//! default-profile wrappers for the frontend takeover browser and tests.
//! A slow browser only ever stalls its own session — other profiles keep
//! running because CDP calls happen outside the manager lock.

use super::{
    cdp::CdpClient, session, BrowserManager, DEFAULT_PROFILE, EVENT_TAKEOVER_REQUESTED,
    EVENT_TAKEOVER_RESUMED,
};
use crate::core::error::{AppError, AppResult};
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::Emitter;
use tokio::sync::oneshot;

impl BrowserManager {
    /// Navigate the default (frontend takeover) browser.
    pub async fn navigate(&self, url: &str) -> AppResult<String> {
        self.navigate_for(DEFAULT_PROFILE, url).await
    }

    /// Navigate a profile's browser and wait for the page to load.
    pub async fn navigate_for(&self, profile: &str, url: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.navigate(url).await?;
        // 60s cap: slow networks (e.g. 25KB/s uplinks) can take a while for
        // heavy pages — wait_ready returns early once the page completes.
        client.wait_ready(60).await?;
        let (page_url, title) = client.page_info().await?;
        Ok(format!("Navigated to {page_url} — {title}"))
    }

    /// Page snapshot of the default browser.
    #[cfg(test)]
    pub async fn read_page(&self, max_chars: usize) -> AppResult<String> {
        self.read_page_for(DEFAULT_PROFILE, max_chars).await
    }

    /// Page snapshot of a profile's browser — visible text (truncated) plus
    /// the interactive elements (buttons/links/inputs) it can click/fill.
    pub async fn read_page_for(&self, profile: &str, max_chars: usize) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.page_snapshot(max_chars).await
    }

    /// Length of a profile's current page visible text — a cheap probe for
    /// "has the JS rendered yet?" polls from research tools.
    pub async fn body_text_len_for(&self, profile: &str) -> AppResult<usize> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        let v = client
            .evaluate("document.body ? document.body.innerText.length : 0")
            .await?;
        Ok(v.as_u64().unwrap_or(0) as usize)
    }

    /// Fill a form input in the default browser.
    #[cfg(test)]
    pub async fn fill(&self, query: &str, text: &str) -> AppResult<String> {
        self.fill_for(DEFAULT_PROFILE, query, text).await
    }

    /// Fill a form input in a profile's browser (placeholder/aria-label/
    /// name/label match).
    pub async fn fill_for(&self, profile: &str, query: &str, text: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.fill_text(query, text).await
    }

    /// Screenshot of the default browser as base64 PNG.
    pub async fn screenshot_png(&self) -> AppResult<String> {
        self.screenshot_for(DEFAULT_PROFILE).await
    }

    /// Screenshot of a profile's browser as base64 PNG.
    pub async fn screenshot_for(&self, profile: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.screenshot_png().await
    }

    /// Forward one input event to a profile's browser — the embedded view
    /// drives the real page. `kind` routes the payload:
    /// - "mouse": `event` ∈ move/down/up, `x`/`y`/`buttons`/`click_count`
    /// - "wheel": `x`/`y`/`delta_x`/`delta_y`
    /// - "key": `event` ∈ down/up, `key`/`code`, optional `text` on keyDown
    /// - "text": `text` via `Input.insertText` (IME-safe insertion)
    #[allow(clippy::too_many_arguments)]
    pub async fn input_for(
        &self,
        profile: &str,
        kind: &str,
        x: i32,
        y: i32,
        buttons: i32,
        click_count: i32,
        delta_x: i32,
        delta_y: i32,
        event: &str,
        key: &str,
        code: &str,
        text: &str,
    ) -> AppResult<()> {
        let key_p = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key_p).await?;
        let client = CdpClient::connect(&ws_url).await?;
        match kind {
            "mouse" => client.mouse_event(event, x, y, buttons, click_count).await,
            "wheel" => client.mouse_wheel(x, y, delta_x, delta_y).await,
            "key" => client.key_event(event, key, code, text).await,
            "text" => client.type_text(text).await,
            other => Err(AppError::NetworkError(format!(
                "unknown input kind '{other}'"
            ))),
        }
    }

    /// Console/network/error logs of the default browser, plus the
    /// persisted log file path.
    pub async fn capture_logs(&self) -> AppResult<(serde_json::Value, PathBuf)> {
        self.capture_logs_for(DEFAULT_PROFILE).await
    }

    /// Enable + return a profile's page console/network/error logs, then
    /// append them to the profile's persistent JSONL log file (bounded,
    /// rotated when oversized) so post-mortem debugging survives browser
    /// restarts.
    pub async fn capture_logs_for(&self, profile: &str) -> AppResult<(serde_json::Value, PathBuf)> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.ensure_log_capture().await?;
        let logs = client.capture_logs().await?;
        let dir = self.app_data_dir.join("browser-logs");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{key}.jsonl"));
        persist_logs_file(&path, &logs, BROWSER_LOG_MAX_BYTES)?;
        Ok((logs, path))
    }

    /// List what the profile's isolated download directory holds — the
    /// download-event equivalent: new files appear here the moment a
    /// download lands, `.crdownload` marks an in-progress download, and
    /// the agent learns the real filename without guessing.
    pub async fn downloads_for(&self, profile: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let dir = {
            let guard = self.sessions.lock().await;
            guard
                .get(&key)
                .map(|s| s.download_dir.clone())
                .ok_or_else(|| {
                    AppError::NetworkError(format!(
                        "browser not running for profile '{key}' — call start first"
                    ))
                })?
        };
        Ok(format_downloads(&list_downloads(&dir)))
    }

    /// Click an element by visible text in the default browser.
    #[cfg(test)]
    pub async fn click_by_text(&self, needle: &str) -> AppResult<String> {
        self.click_by_text_for(DEFAULT_PROFILE, needle).await
    }

    /// Click an element by visible text in a profile's browser.
    pub async fn click_by_text_for(&self, profile: &str, needle: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.click_by_text(needle).await
    }

    /// Element-level snapshot of a profile's page — stable `data-ddc-eid`
    /// ids for every visible interactive element (code-use semantics).
    pub async fn element_snapshot_for(&self, profile: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        let value = client.element_snapshot().await?;
        let text = serde_json::to_string(&value).unwrap_or_else(|_| "[]".to_string());
        Ok(crate::core::str_util::truncate_at_char_boundary(&text, 8000).to_string())
    }

    /// Click an element by its snapshot id in a profile's browser.
    pub async fn click_eid_for(&self, profile: &str, eid: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.click_eid(eid).await
    }

    /// Fill (or select) an element by its snapshot id in a profile's browser.
    pub async fn fill_eid_for(&self, profile: &str, eid: &str, text: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.fill_eid(eid, text).await
    }

    /// Type text into the focused element of the default browser.
    #[cfg(test)]
    pub async fn type_text(&self, text: &str) -> AppResult<String> {
        self.type_text_for(DEFAULT_PROFILE, text).await
    }

    /// Type text into the focused element of a profile's browser.
    pub async fn type_text_for(&self, profile: &str, text: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.type_text(text).await?;
        Ok(format!("Typed {} characters", text.chars().count()))
    }

    /// Press a named key in the default browser.
    #[cfg(test)]
    pub async fn press_key(&self, name: &str) -> AppResult<String> {
        self.press_key_for(DEFAULT_PROFILE, name).await
    }

    /// Press a named key in a profile's browser.
    pub async fn press_key_for(&self, profile: &str, name: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.press_key(name).await?;
        Ok(format!("Pressed {name}"))
    }

    /// Execute arbitrary JavaScript on a profile's page (JSON, truncated).
    pub async fn eval_js_for(&self, profile: &str, expression: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        let v = client.evaluate(expression).await?;
        let text = serde_json::to_string(&v).unwrap_or_default();
        Ok(crate::core::str_util::truncate_at_char_boundary(&text, 4000).to_string())
    }

    /// Click the first element matching a CSS selector in a profile.
    pub async fn click_css_for(&self, profile: &str, selector: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.click_css(selector).await
    }

    /// Fill the first element matching a CSS selector in a profile.
    pub async fn fill_css_for(
        &self,
        profile: &str,
        selector: &str,
        text: &str,
    ) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.fill_css(selector, text).await
    }

    /// Scroll a profile's page (direction or "x,y" pixels).
    pub async fn scroll_for(&self, profile: &str, direction: &str) -> AppResult<String> {
        let (x, y) = parse_scroll(direction)?;
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        client.scroll(x, y).await?;
        Ok(format!("Scrolled {direction}"))
    }

    /// Poll until a CSS selector or text appears in a profile's browser.
    pub async fn wait_for_profile(
        &self,
        profile: &str,
        selector: Option<&str>,
        text: Option<&str>,
        timeout_secs: u64,
    ) -> AppResult<String> {
        if selector.is_none() && text.is_none() {
            return Err("wait_for needs 'selector' or 'text'".into());
        }
        let key = session::sanitize_profile_key(profile)?;
        let ws_url = self.require_ws(&key).await?;
        let client = CdpClient::connect(&ws_url).await?;
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.max(1));
        loop {
            let hit = if let Some(sel) = selector {
                let expr = format!(
                    "!!document.querySelector({})",
                    serde_json::to_string(sel).unwrap_or_else(|_| "null".into())
                );
                client.evaluate(&expr).await?.as_bool().unwrap_or(false)
            } else if let Some(needle) = text {
                let expr = format!(
                    "document.body ? document.body.innerText.includes({}) : false",
                    serde_json::to_string(needle).unwrap_or_else(|_| "null".into())
                );
                client.evaluate(&expr).await?.as_bool().unwrap_or(false)
            } else {
                false
            };
            if hit {
                return Ok("Condition met".to_string());
            }
            if std::time::Instant::now() >= deadline {
                return Ok(format!("Timed out after {timeout_secs}s"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    /// Pause a profile's session for the user: emit the takeover event,
    /// wait until the user clicks "继续" (or the timeout elapses).
    pub async fn handoff_for(
        &self,
        profile: &str,
        reason: &str,
        timeout_secs: u64,
        app: &tauri::AppHandle,
    ) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        self.ensure_handoff_visible(&key).await?;
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.sessions.lock().await;
            let session = guard
                .get_mut(&key)
                .ok_or_else(|| AppError::NetworkError("browser not running".into()))?;
            session.takeover = Some(tx);
            session.takeover_reason = Some(reason.to_string());
        }
        let _ = app.emit(
            EVENT_TAKEOVER_REQUESTED,
            json!({ "reason": reason, "profile": key }),
        );
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx).await;
        match &outcome {
            Ok(Ok(())) | Ok(Err(_)) => {
                // Success or channel error: the takeover is settled — clear it.
                let mut guard = self.sessions.lock().await;
                if let Some(s) = guard.get_mut(&key) {
                    s.takeover = None;
                    s.takeover_reason = None;
                }
            }
            Err(_) => {
                // Timed out: KEEP the pending takeover so `resume_for` can
                // still complete it when the user clicks "继续" — the error
                // text below tells them to resume or restart the task.
            }
        }
        let _ = app.emit(EVENT_TAKEOVER_RESUMED, json!({}));
        match outcome {
            Ok(Ok(())) => Ok("User finished the takeover; continuing".to_string()),
            Ok(Err(_)) => Err("Takeover channel closed unexpectedly".into()),
            Err(_) => Err(format!(
                "Takeover timed out after {timeout_secs}s — the agent browser session \
                 is still running; ask the user to resume or restart the task"
            )
            .into()),
        }
    }

    /// Complete a pending takeover of the default browser.
    pub async fn resume(&self) -> bool {
        self.resume_for(DEFAULT_PROFILE).await
    }

    /// Complete a pending takeover of a profile's browser.
    pub async fn resume_for(&self, profile: &str) -> bool {
        let Ok(key) = session::sanitize_profile_key(profile) else {
            return false;
        };
        let mut guard = self.sessions.lock().await;
        match guard.get_mut(&key).and_then(|s| s.takeover.take()) {
            Some(tx) => {
                let _ = tx.send(());
                true
            }
            None => false,
        }
    }

    // ── Multi-tab ────────────────────────────────────────────────────────

    /// Tab list of the default browser.
    #[cfg(test)]
    pub async fn tabs(&self) -> AppResult<String> {
        self.tabs_for(DEFAULT_PROFILE).await
    }

    /// Tab list of a profile's browser as a model-readable text report.
    pub async fn tabs_for(&self, profile: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        self.refresh_targets(&key).await;
        let (targets, current) = {
            let guard = self.sessions.lock().await;
            let s = guard
                .get(&key)
                .ok_or_else(|| AppError::NetworkError("browser not running".into()))?;
            (s.targets.clone(), s.current_target_id.clone())
        };
        if targets.is_empty() {
            return Ok("No tabs open".to_string());
        }
        let mut out = format!("标签页 {}（当前标 *）\n", targets.len());
        for t in &targets {
            let marker = if Some(t.id.as_str()) == current.as_deref() {
                "*"
            } else {
                " "
            };
            let title = if t.title.is_empty() {
                "(无标题)"
            } else {
                &t.title
            };
            out.push_str(&format!("[{marker}] {title} — {}\n  id={}\n", t.url, t.id));
        }
        Ok(out)
    }

    /// Open a new tab in the default browser.
    #[cfg(test)]
    pub async fn tab_new(&self, url: &str) -> AppResult<String> {
        self.tab_new_for(DEFAULT_PROFILE, url).await
    }

    /// Structured tab snapshot for the frontend tab strip. Empty when the
    /// browser is not running (or its endpoint is gone).
    pub async fn tabs_snapshot(&self) -> AppResult<Vec<crate::browser::BrowserTab>> {
        self.tabs_snapshot_for(DEFAULT_PROFILE).await
    }

    /// Structured tab snapshot for a profile's browser (frontend tab strip).
    pub async fn tabs_snapshot_for(
        &self,
        profile: &str,
    ) -> AppResult<Vec<crate::browser::BrowserTab>> {
        let key = session::sanitize_profile_key(profile)?;
        if !self.sessions.lock().await.contains_key(&key) {
            return Ok(Vec::new());
        }
        self.refresh_targets(&key).await;
        let (targets, current) = {
            let guard = self.sessions.lock().await;
            let s = guard.get(&key).expect("checked above");
            (s.targets.clone(), s.current_target_id.clone())
        };
        Ok(targets
            .iter()
            .map(|t| crate::browser::BrowserTab {
                id: t.id.clone(),
                title: t.title.clone(),
                url: t.url.clone(),
                active: Some(t.id.as_str()) == current.as_deref(),
            })
            .collect())
    }

    /// Open a new tab in a profile's browser and switch to it.
    pub async fn tab_new_for(&self, profile: &str, url: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let browser_ws = {
            let guard = self.sessions.lock().await;
            let s = guard
                .get(&key)
                .ok_or_else(|| AppError::NetworkError("browser not running".into()))?;
            s.browser_ws_url.clone()
        };
        let client = CdpClient::connect(&browser_ws).await?;
        let id = client.create_target(url).await?;
        {
            let mut guard = self.sessions.lock().await;
            if let Some(s) = guard.get_mut(&key) {
                s.current_target_id = Some(id.clone());
            }
        }
        // Pull the new tab's ws url in, then wait for its load.
        self.refresh_targets(&key).await;
        let ws = self
            .target_ws(&key, &id)
            .await?
            .ok_or_else(|| AppError::NetworkError("new tab not found in target list".into()))?;
        let page = CdpClient::connect(&ws).await?;
        page.wait_ready(60).await?;
        let (page_url, title) = page.page_info().await?;
        Ok(format!("Opened tab: {title} — {page_url} (id={id})"))
    }

    /// Switch the active tab of the default browser.
    #[cfg(test)]
    pub async fn tab_switch(&self, target_id: &str) -> AppResult<String> {
        self.tab_switch_for(DEFAULT_PROFILE, target_id).await
    }

    /// Switch the active tab of a profile's browser.
    pub async fn tab_switch_for(&self, profile: &str, target_id: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        self.refresh_targets(&key).await;
        let (browser_ws, exists) = {
            let guard = self.sessions.lock().await;
            let s = guard
                .get(&key)
                .ok_or_else(|| AppError::NetworkError("browser not running".into()))?;
            let exists = s.targets.iter().any(|t| t.id == target_id);
            (s.browser_ws_url.clone(), exists)
        };
        if !exists {
            return Err(format!("No tab with id {target_id} — list tabs first").into());
        }
        let client = CdpClient::connect(&browser_ws).await?;
        client.activate_target(target_id).await?;
        {
            let mut guard = self.sessions.lock().await;
            if let Some(s) = guard.get_mut(&key) {
                s.current_target_id = Some(target_id.to_string());
            }
        }
        Ok(format!("Switched to tab {target_id}"))
    }

    /// Close a tab of the default browser.
    #[cfg(test)]
    pub async fn tab_close(&self, target_id: Option<&str>) -> AppResult<String> {
        self.tab_close_for(DEFAULT_PROFILE, target_id).await
    }

    /// Close a tab of a profile's browser (default: the current one).
    pub async fn tab_close_for(&self, profile: &str, target_id: Option<&str>) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        self.refresh_targets(&key).await;
        let (browser_ws, id, count) = {
            let guard = self.sessions.lock().await;
            let s = guard
                .get(&key)
                .ok_or_else(|| AppError::NetworkError("browser not running".into()))?;
            let id = match target_id {
                Some(explicit) => explicit.to_string(),
                None => s
                    .current_target_id
                    .clone()
                    .ok_or_else(|| AppError::NetworkError("no current tab".into()))?,
            };
            (s.browser_ws_url.clone(), id, s.targets.len())
        };
        if count <= 1 {
            return Err("Cannot close the last tab — stop the browser instead".into());
        }
        let client = CdpClient::connect(&browser_ws).await?;
        let closed = client.close_target(&id).await?;
        if !closed {
            return Err(format!("Tab {id} did not close").into());
        }
        self.refresh_targets(&key).await;
        {
            let mut guard = self.sessions.lock().await;
            if let Some(s) = guard.get_mut(&key) {
                if s.current_target_id.as_deref() == Some(id.as_str()) {
                    s.current_target_id = s.targets.first().map(|t| t.id.clone());
                }
            }
        }
        Ok(format!("Closed tab {id}"))
    }
}

/// Parse a scroll direction ("up"/"down"/"left"/"right"/"top"/"bottom" or
/// "x,y" pixel deltas) into a pixel delta.
fn parse_scroll(direction: &str) -> AppResult<(i64, i64)> {
    match direction.trim().to_lowercase().as_str() {
        "up" => Ok((0, -600)),
        "down" => Ok((0, 600)),
        "left" => Ok((-600, 0)),
        "right" => Ok((600, 0)),
        "top" => Ok((0, -100_000)),
        "bottom" => Ok((0, 100_000)),
        other => {
            let parts: Vec<&str> = other.split(',').collect();
            if parts.len() == 2 {
                if let (Ok(x), Ok(y)) = (
                    parts[0].trim().parse::<i64>(),
                    parts[1].trim().parse::<i64>(),
                ) {
                    return Ok((x, y));
                }
            }
            Err(format!(
                "Invalid scroll direction '{direction}' — use up/down/left/right/top/bottom or 'x,y'"
            )
            .into())
        }
    }
}

// ── Downloads & persistent logs ─────────────────────────────────────────

/// Cap per JSONL log file before it rotates to `<name>.old.jsonl`.
pub const BROWSER_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// One entry in a profile's isolated download directory.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadInfo {
    pub name: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
    pub in_progress: bool,
}

/// Scan a download directory (newest modified first). Missing/unreadable
/// directories yield an empty list — never an error.
pub fn list_downloads(dir: &Path) -> Vec<DownloadInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<DownloadInfo> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let meta = e.metadata().ok()?;
            let in_progress = name.ends_with(".crdownload");
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            Some(DownloadInfo {
                name,
                size_bytes: meta.len(),
                modified_ms,
                in_progress,
            })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.modified_ms));
    out
}

/// Model-readable download report.
pub fn format_downloads(list: &[DownloadInfo]) -> String {
    if list.is_empty() {
        return "下载目录为空（还没有文件落盘）".to_string();
    }
    let mut out = format!("下载目录 {} 个文件（新→旧）：\n", list.len());
    for d in list {
        let state = if d.in_progress {
            "⏳ 下载中"
        } else {
            "✓ 完成"
        };
        let size = if d.size_bytes >= 1024 * 1024 {
            format!("{:.1} MB", d.size_bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.0} KB", d.size_bytes as f64 / 1024.0)
        };
        out.push_str(&format!("[{state}] {size} — {}\n", d.name));
    }
    out
}

/// Append one JSON log record per capture to a JSONL file; rotate to
/// `<name>.old.jsonl` when the file exceeds `max_bytes`. Best-effort-safe:
/// a failed persist must not break the live log read-back.
pub fn persist_logs_file(path: &Path, logs: &serde_json::Value, max_bytes: u64) -> AppResult<()> {
    if let Ok(meta) = path.metadata() {
        if meta.len() > max_bytes {
            let _ = std::fs::rename(path, path.with_extension("old.jsonl"));
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(logs)?;
    writeln!(f, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_direction_parsing() {
        assert_eq!(parse_scroll("up").unwrap(), (0, -600));
        assert_eq!(parse_scroll("DOWN").unwrap(), (0, 600));
        assert_eq!(parse_scroll("100,200").unwrap(), (100, 200));
        assert!(parse_scroll("diagonal").is_err());
        assert!(parse_scroll("1,2,3").is_err());
    }

    #[test]
    fn downloads_list_newest_first_and_flags_in_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("report.pdf");
        let live = tmp.path().join("archive.zip.crdownload");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&live, b"yy").unwrap();
        // Give the two files distinct mtimes so ordering is deterministic.
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let _ = std::fs::File::options()
            .write(true)
            .open(&old)
            .and_then(|f| f.set_modified(past));

        let list = list_downloads(tmp.path());
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "archive.zip.crdownload");
        assert!(list[0].in_progress);
        assert!(!list[1].in_progress);
        let text = format_downloads(&list);
        assert!(text.contains("⏳ 下载中"));
        assert!(text.contains("✓ 完成"));
        assert!(text.contains("archive.zip.crdownload"));
    }

    #[test]
    fn downloads_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let text = format_downloads(&list_downloads(&tmp.path().join("nope")));
        assert!(text.contains("下载目录为空"));
    }

    #[test]
    fn persist_logs_appends_and_rotates() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.jsonl");
        persist_logs_file(&path, &json!({"console": []}), 10_000).unwrap();
        persist_logs_file(&path, &json!({"errors": ["boom"]}), 10_000).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2, "one JSON line per capture");
        assert!(content.contains("boom"));

        // Oversized file rotates to .old.jsonl, then a fresh line is appended.
        persist_logs_file(&path, &json!({"x": "y"}), 1).unwrap();
        assert!(
            path.with_extension("old.jsonl").exists(),
            "rotation must happen"
        );
        let fresh = std::fs::read_to_string(&path).unwrap();
        assert_eq!(fresh.lines().count(), 1);
    }
}
