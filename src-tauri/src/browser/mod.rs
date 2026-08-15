//! Browser takeover — real Chromium sessions the agent can drive.
//!
//! Desktop-native, modeled on the "let the agent use a real browser"
//! approach:
//!
//! - **Profile-keyed sessions** — every conversation drives its own browser
//!   (`session-<id>` profile) with its own `--user-data-dir`, so cookies,
//!   logins and downloads never leak between conversations or into the
//!   user's personal browser. The frontend takeover browser uses the
//!   `takeover` profile.
//! - **Headless mode** — `start_for(.., headless: true)` launches without a
//!   window for unattended batch automation; CDP screenshots and JS still
//!   work.
//! - **Human-in-the-loop** — on captchas, logins or confirmations the agent
//!   calls `handoff`; the session pauses, the frontend shows a takeover
//!   banner, the user acts in the real window, then clicks "继续" and the
//!   agent resumes where it stopped.
//!
//! All state lives behind one `Mutex<HashMap<profile, BrowserSession>>`;
//! CDP calls run outside the lock so a slow browser never blocks another
//! session or the takeover channel.

pub mod actions;
pub mod cdp;
pub mod launch;
pub mod screencast;
pub mod session;

use crate::core::error::{AppError, AppResult};
use cdp::CdpClient;
use session::BrowserSession;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::Mutex;

/// Re-exported session types for commands/tools.
pub use session::{session_profile_key, BrowserStatus, LaunchOptions, DEFAULT_PROFILE};

/// One browser tab for the frontend tab strip.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrowserTab {
    pub id: String,
    pub title: String,
    pub url: String,
    /// Whether this is the session's active tab.
    pub active: bool,
}

/// How long `handoff` waits for the user before giving up (default).
pub const TAKEOVER_DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Event emitted when the agent pauses for a user takeover.
pub const EVENT_TAKEOVER_REQUESTED: &str = "browser-takeover-requested";
/// Event emitted when the takeover is resolved (resume or timeout).
pub const EVENT_TAKEOVER_RESUMED: &str = "browser-takeover-resumed";
/// Event emitted when a browser session starts/stops — the frontend opens
/// its live "browser" pane on `running` for the current session's profile.
pub const EVENT_BROWSER_STATUS_CHANGED: &str = "browser-status-changed";

/// Broadcast a browser-status-changed event so the frontend can open/close
/// its live "browser" pane for this profile. Called by the browser_control
/// tool and the takeover commands after a start/stop.
pub fn broadcast_status_changed(app: &tauri::AppHandle, profile: &str, running: bool) {
    use tauri::Emitter as _;
    let _ = app.emit(
        EVENT_BROWSER_STATUS_CHANGED,
        serde_json::json!({ "profile": profile, "running": running }),
    );
}

/// Global browser manager — one session per profile key.
pub struct BrowserManager {
    sessions: Mutex<HashMap<String, BrowserSession>>,
    app_data_dir: PathBuf,
}

impl BrowserManager {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            app_data_dir,
        }
    }

    /// Launch the default (frontend takeover) browser — test-only; commands
    /// go through `start_for` with explicit options.
    #[cfg(test)]
    pub async fn start(&self, url: Option<&str>) -> AppResult<String> {
        self.start_for(url, LaunchOptions::default()).await
    }

    /// Launch a profile-keyed browser session (isolated profile dir,
    /// optional headless). Idempotent when the same mode is already
    /// running; switching modes requires stopping first.
    pub async fn start_for(&self, url: Option<&str>, opts: LaunchOptions) -> AppResult<String> {
        let key = session::sanitize_profile_key(&opts.profile)?;
        let mut guard = self.sessions.lock().await;
        if let Some(existing) = guard.get(&key) {
            if existing.headless == opts.headless {
                return Ok(format!(
                    "Agent browser already running for profile '{key}' ({}), not relaunched",
                    mode_label(existing.headless)
                ));
            }
            return Err(format!(
                "browser already running for profile '{key}' in {} mode — \
                 stop it first to switch",
                mode_label(existing.headless)
            )
            .into());
        }
        let exe = launch::find_browser_exe().ok_or_else(|| {
            AppError::NetworkError("no Chromium-family browser found (Edge/Chrome required)".into())
        })?;
        let port = launch::pick_free_port()?;
        let profile_dir = self.app_data_dir.join("browser-profiles").join(&key);
        let download_dir = profile_dir.join("downloads");
        std::fs::create_dir_all(&download_dir)?;
        let mut child = spawn_browser(&exe, port, &profile_dir, &download_dir, opts.headless)?;
        let (browser_ws_url, page_ws, page_id) =
            launch::wait_for_devtools(port).await.inspect_err(|_| {
                let _ = child.start_kill();
            })?;
        guard.insert(
            key.clone(),
            BrowserSession {
                profile: key.clone(),
                headless: opts.headless,
                download_dir,
                port,
                browser_ws_url,
                targets: vec![launch::PageTarget {
                    id: page_id.clone(),
                    title: String::new(),
                    url: String::new(),
                    ws_url: page_ws,
                }],
                current_target_id: Some(page_id),
                child: Some(child),
                takeover: None,
                takeover_reason: None,
            },
        );
        drop(guard);
        if let Some(target) = url.filter(|u| !u.trim().is_empty()) {
            self.navigate_for(&key, target).await?;
        }
        tracing::info!(
            port,
            profile = %key,
            headless = opts.headless,
            "Browser session started"
        );
        Ok(format!(
            "Agent browser started (profile: {key}, {})",
            mode_label(opts.headless)
        ))
    }

    /// Stop the default browser. Returns whether one was running.
    pub async fn stop(&self) -> bool {
        self.stop_for(DEFAULT_PROFILE).await
    }

    /// Stop a profile's browser (kills the process tree).
    pub async fn stop_for(&self, profile: &str) -> bool {
        let Ok(key) = session::sanitize_profile_key(profile) else {
            return false;
        };
        let mut guard = self.sessions.lock().await;
        let Some(session) = guard.remove(&key) else {
            return false;
        };
        if let Some(mut child) = session.child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        tracing::info!(profile = %key, "Browser session stopped");
        true
    }

    /// Whether a session exists for the default profile (test-only).
    #[cfg(test)]
    pub async fn is_running(&self) -> bool {
        self.is_running_for(DEFAULT_PROFILE).await
    }

    /// Whether a session exists for a profile (test-only).
    #[cfg(test)]
    pub async fn is_running_for(&self, profile: &str) -> bool {
        match session::sanitize_profile_key(profile) {
            Ok(key) => self.sessions.lock().await.contains_key(&key),
            Err(_) => false,
        }
    }

    /// Current status of the default browser.
    pub async fn status(&self) -> BrowserStatus {
        self.status_for(DEFAULT_PROFILE).await
    }

    /// Current status of a profile's browser + live page info
    /// (best-effort URL/title).
    pub async fn status_for(&self, profile: &str) -> BrowserStatus {
        let Ok(key) = session::sanitize_profile_key(profile) else {
            return session::stopped_status();
        };
        let (ws_url, takeover, reason, headless, download_dir, profile_key) = {
            let guard = self.sessions.lock().await;
            match guard.get(&key) {
                Some(s) => {
                    let ws = s.current_target_id.as_ref().and_then(|id| {
                        s.targets
                            .iter()
                            .find(|t| &t.id == id)
                            .map(|t| t.ws_url.clone())
                    });
                    (
                        ws,
                        s.takeover.is_some(),
                        s.takeover_reason.clone(),
                        s.headless,
                        Some(s.download_dir.clone()),
                        Some(s.profile.clone()),
                    )
                }
                None => (None, false, None, false, None, None),
            }
        };
        let Some(ws_url) = ws_url else {
            return session::stopped_status();
        };
        let (url, title) = match CdpClient::connect(&ws_url).await {
            Ok(client) => match client.page_info().await {
                Ok((url, title)) => (url, title),
                Err(_) => {
                    // CDP answered the handshake but the page call failed —
                    // the browser process died mid-flight. Report honestly
                    // and recycle the dead session.
                    return self.recycle_dead(&key).await;
                }
            },
            Err(_) => {
                // The CDP endpoint is gone — the browser process died or the
                // session was torn down externally. Recycle the session state
                // so a later start works.
                return self.recycle_dead(&key).await;
            }
        };
        BrowserStatus {
            running: true,
            url: (!url.is_empty()).then_some(url),
            title: (!title.is_empty()).then_some(title),
            awaiting_takeover: takeover,
            takeover_reason: reason,
            profile: profile_key,
            headless,
            download_dir: download_dir.map(|d| d.to_string_lossy().to_string()),
        }
    }

    /// The CDP endpoint is gone — the browser process died or the session
    /// was torn down externally. Recycle: kill the child and drop the
    /// session so a later `start` works, then report stopped.
    async fn recycle_dead(&self, profile: &str) -> BrowserStatus {
        let Ok(key) = session::sanitize_profile_key(profile) else {
            return session::stopped_status();
        };
        let mut guard = self.sessions.lock().await;
        let Some(session) = guard.remove(&key) else {
            return session::stopped_status();
        };
        if let Some(mut child) = session.child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        tracing::warn!(
            profile = %key,
            "Browser session recycled — CDP endpoint unreachable, process likely died"
        );
        session::stopped_status()
    }

    /// ws url of a profile's current tab, or "not running" error.
    pub(crate) async fn require_ws(&self, profile: &str) -> AppResult<String> {
        let key = session::sanitize_profile_key(profile)?;
        let guard = self.sessions.lock().await;
        let s = guard.get(&key).ok_or_else(|| {
            AppError::NetworkError(format!(
                "browser not running for profile '{key}' — call start first"
            ))
        })?;
        s.current_target_id
            .as_ref()
            .and_then(|id| {
                s.targets
                    .iter()
                    .find(|t| &t.id == id)
                    .map(|t| t.ws_url.clone())
            })
            .ok_or_else(|| AppError::NetworkError(format!("no current tab for profile '{key}'")))
    }

    /// Refresh the tab list of a profile from DevTools; repair the
    /// current-tab pointer if it points at a closed tab.
    pub(crate) async fn refresh_targets(&self, profile: &str) {
        let port = {
            let guard = self.sessions.lock().await;
            guard.get(profile).map(|s| s.port)
        };
        let Some(port) = port else {
            return;
        };
        let Ok(targets) = launch::fetch_page_targets(port).await else {
            return;
        };
        let mut guard = self.sessions.lock().await;
        if let Some(s) = guard.get_mut(profile) {
            let keep = s
                .current_target_id
                .as_ref()
                .map(|id| targets.iter().any(|t| &t.id == id))
                .unwrap_or(false);
            s.targets = targets;
            if !keep {
                s.current_target_id = s.targets.first().map(|t| t.id.clone());
            }
        }
    }

    /// ws url of a specific target id in a profile, if present.
    pub(crate) async fn target_ws(
        &self,
        profile: &str,
        target_id: &str,
    ) -> AppResult<Option<String>> {
        let guard = self.sessions.lock().await;
        Ok(guard.get(profile).and_then(|s| {
            s.targets
                .iter()
                .find(|t| t.id == target_id)
                .map(|t| t.ws_url.clone())
        }))
    }

    /// Gate: handoff requires a **visible** session — a headless browser has
    /// no window for the user to take over. Pure state check so it stays
    /// unit-testable without a Tauri app handle.
    pub(crate) async fn ensure_handoff_visible(&self, profile: &str) -> AppResult<()> {
        let headless = {
            let guard = self.sessions.lock().await;
            guard.get(profile).map(|s| s.headless)
        };
        match headless {
            None => Err(AppError::NetworkError(format!(
                "browser not running for profile '{profile}'"
            ))),
            Some(true) => Err(AppError::NetworkError(format!(
                "profile '{profile}' is headless — handoff needs a visible window; \
                 stop it and start without headless for user takeover"
            ))),
            Some(false) => Ok(()),
        }
    }
}

impl Drop for BrowserManager {
    fn drop(&mut self) {
        // App exiting: best-effort kill (no async in Drop). The children die
        // with the parent anyway on most platforms; this covers the rest.
        let inner = self.sessions.try_lock();
        if let Ok(mut guard) = inner {
            for session in guard.values_mut() {
                if let Some(child) = session.child.as_mut() {
                    let _ = child.start_kill();
                }
            }
        }
    }
}

/// Human-readable mode label for messages/status.
fn mode_label(headless: bool) -> &'static str {
    if headless {
        "headless"
    } else {
        "visible"
    }
}

/// Spawn the browser process with remote debugging on the chosen port.
/// Profile dir, download dir and headless mode are all part of the launch
/// boundary — the agent never touches the user's personal browser.
fn spawn_browser(
    exe: &std::path::Path,
    port: u16,
    profile_dir: &std::path::Path,
    download_dir: &std::path::Path,
    headless: bool,
) -> AppResult<tokio::process::Child> {
    use std::process::Stdio;
    let mut command = tokio::process::Command::new(exe);
    command
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(format!(
            "--download-default-directory={}",
            download_dir.display()
        ))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-features=Translate,TranslateUI")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if headless {
        command
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--window-size=1280,900");
    }
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW — no console flash on launch. tokio's Command
        // exposes creation_flags natively on Windows (no CommandExt import).
        command.creation_flags(0x0800_0000);
    }
    let child = command.spawn().map_err(AppError::Io)?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_manager_is_not_running() {
        let m = BrowserManager::new(std::env::temp_dir());
        let status = m.status().await;
        assert!(!status.running);
        assert!(!status.awaiting_takeover);
        assert!(!status.headless);
        assert_eq!(status.profile, None);
    }

    #[tokio::test]
    async fn resume_without_session_is_false() {
        let m = BrowserManager::new(std::env::temp_dir());
        assert!(!m.resume().await);
    }

    #[tokio::test]
    async fn operations_require_a_running_session() {
        let m = BrowserManager::new(std::env::temp_dir());
        assert!(m.navigate("https://example.com").await.is_err());
        assert!(m.read_page(100).await.is_err());
        assert!(m.screenshot_png().await.is_err());
        assert!(m.click_by_text("go").await.is_err());
        assert!(m.type_text("hi").await.is_err());
        assert!(m.press_key("enter").await.is_err());
        assert!(!m.stop().await, "stop on empty manager returns false");
    }

    #[tokio::test]
    async fn keyed_operations_require_their_own_session() {
        let m = BrowserManager::new(std::env::temp_dir());
        let err = m
            .navigate_for("session-a", "https://example.com")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("session-a"),
            "error must name the profile"
        );
        assert!(!m.stop_for("session-b").await);
        assert!(!m.resume_for("session-c").await);
        assert!(!m.is_running_for("session-a").await);
    }

    #[tokio::test]
    async fn handoff_gate_rejects_headless_and_missing_sessions() {
        let m = BrowserManager::new(std::env::temp_dir());
        let err = m.ensure_handoff_visible("none").await.unwrap_err();
        assert!(err.to_string().contains("not running"));

        m.sessions
            .lock()
            .await
            .insert("batch-x".into(), fake_session(true, &m.app_data_dir));
        let err = m.ensure_handoff_visible("batch-x").await.unwrap_err();
        assert!(err.to_string().contains("headless"));

        m.sessions
            .lock()
            .await
            .insert("vis-x".into(), fake_session(false, &m.app_data_dir));
        assert!(m.ensure_handoff_visible("vis-x").await.is_ok());
    }

    /// A session record without a live process — enough for state-level
    /// unit tests (no real browser launch).
    fn fake_session(headless: bool, app_data_dir: &std::path::Path) -> BrowserSession {
        BrowserSession {
            profile: String::new(),
            headless,
            download_dir: app_data_dir.join("downloads"),
            port: 0,
            browser_ws_url: String::new(),
            targets: Vec::new(),
            current_target_id: None,
            child: None,
            takeover: None,
            takeover_reason: None,
        }
    }

    /// End-to-end smoke: launches a REAL Edge/Chrome, drives it over CDP,
    /// and tears it down. Manual — `cargo test --lib -- --ignored browser::`
    /// (requires a Chromium-family browser and network).
    #[tokio::test]
    #[ignore = "launches a real browser — manual smoke test"]
    async fn smoke_launch_navigate_read_and_screenshot() {
        let dir = std::env::temp_dir().join(format!("browser-smoke-{}", std::process::id()));
        let m = BrowserManager::new(dir);
        m.start(Some("https://example.com")).await.expect("launch");
        let status = m.status().await;
        assert!(status.running, "session must be running");
        let snap = m.read_page(600).await.expect("read_page");
        assert!(
            snap.contains("Learn more") && snap.contains("iana.org"),
            "snapshot must list the example.com link, got: {snap}"
        );
        let png = m.screenshot_png().await.expect("screenshot");
        assert!(png.len() > 1000, "PNG base64 must be non-trivial");

        // 看→点→看 loop on an offline form page (network-independent):
        // snapshot sees the fields, fill targets them by placeholder.
        let form = "<form><input placeholder=\"用户名\"><input type=\"password\" \
                    placeholder=\"密码\"><button>登录</button></form>";
        let data_url = format!("data:text/html;charset=utf-8,{}", urlencoding::encode(form));
        m.navigate(&data_url).await.expect("navigate data form");
        let snap2 = m.read_page(400).await.expect("read_page 2");
        assert!(
            snap2.contains("用户名") && snap2.contains("密码框") && snap2.contains("登录"),
            "snapshot must classify the form fields, got: {snap2}"
        );
        m.fill("用户名", "admin").await.expect("fill username");
        m.fill("密码", "secret123").await.expect("fill password");

        // Multi-tab: open a second tab, switch between them, close it.
        // Edge may open its own first-run tab (sync-confirmation), so assert
        // count RELATIVELY, never on absolute numbers.
        let tabs_before = m.tabs().await.expect("tabs");
        assert!(
            tabs_before.contains("data:text/html"),
            "tab list must show the form tab, got: {tabs_before}"
        );
        let before_count = tabs_before.matches("id=").count();
        // The form tab is current right now — remember it for the switch-back.
        let tab1_id = {
            let guard = m.sessions.lock().await;
            guard
                .get(DEFAULT_PROFILE)
                .unwrap()
                .current_target_id
                .clone()
                .unwrap()
        };
        m.tab_new("https://example.org").await.expect("tab_new");
        let tabs_two = m.tabs().await.expect("tabs 2");
        assert_eq!(
            tabs_two.matches("id=").count(),
            before_count + 1,
            "one new tab expected, got: {tabs_two}"
        );
        assert!(tabs_two.contains("example.org"), "new tab must be current");
        let target_id = {
            let guard = m.sessions.lock().await;
            guard
                .get(DEFAULT_PROFILE)
                .unwrap()
                .current_target_id
                .clone()
                .unwrap()
        };
        m.tab_switch(&tab1_id).await.expect("tab_switch");
        let snap_back = m.read_page(300).await.expect("read_page after switch");
        assert!(
            snap_back.contains("data:text/html"),
            "must be back on tab 1"
        );
        m.tab_close(Some(&target_id)).await.expect("tab_close");
        // Tab teardown is async — the /json list may briefly keep the
        // closing tab; poll until the count drops back.
        let tabs_one = {
            let mut last = String::new();
            for _ in 0..10 {
                last = m.tabs().await.expect("tabs 4");
                if last.matches("id=").count() == before_count {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            last
        };
        assert_eq!(
            tabs_one.matches("id=").count(),
            before_count,
            "tab count must return, got: {tabs_one}"
        );

        assert!(m.stop().await, "stop must report a running session");
        assert!(!m.is_running().await, "session must be gone after stop");
    }

    /// Manual smoke for the new headless path — same session machinery, no
    /// window. `cargo test --lib -- --ignored browser::headless`
    #[tokio::test]
    #[ignore = "launches a real browser — manual smoke test"]
    async fn smoke_headless_start_navigate_and_screenshot() {
        let dir = std::env::temp_dir().join(format!("browser-headless-{}", std::process::id()));
        let m = BrowserManager::new(dir);
        let opts = LaunchOptions {
            profile: "batch-a".into(),
            headless: true,
        };
        m.start_for(Some("https://example.com"), opts)
            .await
            .expect("headless launch");
        let status = m.status_for("batch-a").await;
        assert!(status.running, "headless session must be running");
        assert!(status.headless, "status must report headless");
        let png = m
            .screenshot_for("batch-a")
            .await
            .expect("headless screenshot");
        assert!(png.len() > 1000, "PNG base64 must be non-trivial");
        let snap = m
            .read_page_for("batch-a", 600)
            .await
            .expect("headless read");
        assert!(
            snap.contains("iana.org"),
            "headless page must render, got: {snap}"
        );
        assert!(m.stop_for("batch-a").await, "headless stop");
        assert!(!m.is_running_for("batch-a").await);
    }
}
