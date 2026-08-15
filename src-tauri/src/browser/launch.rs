//! Browser discovery + launch for browser takeover.
//!
//! Launches a system Chromium-family browser (Edge first on Windows — it is
//! preinstalled) in a **dedicated agent profile** with a remote debugging
//! port, then waits for the DevTools HTTP endpoint to come up.

use crate::core::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::Value;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

/// Poll interval / overall budget for the DevTools endpoint.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// One page-level DevTools target (a browser tab).
#[derive(Debug, Clone, Serialize)]
pub struct PageTarget {
    pub id: String,
    pub title: String,
    pub url: String,
    pub ws_url: String,
}

/// Common install locations of Chromium-family browsers.
fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local);
        candidates.push(base.join("Google/Chrome/Application/chrome.exe"));
        candidates.push(base.join("Microsoft/Edge/Application/msedge.exe"));
    }
    #[cfg(windows)]
    {
        candidates.extend(
            [
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            ]
            .iter()
            .map(PathBuf::from),
        );
    }
    #[cfg(target_os = "macos")]
    {
        candidates.extend(
            [
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            ]
            .iter()
            .map(PathBuf::from),
        );
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        candidates.extend(
            [
                "/usr/bin/google-chrome",
                "/usr/bin/microsoft-edge",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
            ]
            .iter()
            .map(PathBuf::from),
        );
    }
    candidates
}

/// First existing browser executable, if any.
pub fn find_browser_exe() -> Option<PathBuf> {
    candidate_paths().into_iter().find(|p| p.is_file())
}

/// Grab a free localhost TCP port (best effort — race risk is negligible).
pub fn pick_free_port() -> AppResult<u16> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| AppError::NetworkError(e.to_string()))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| AppError::NetworkError(e.to_string()))
}

/// List all page targets (tabs) of the DevTools endpoint — `GET /json`.
pub async fn fetch_page_targets(port: u16) -> AppResult<Vec<PageTarget>> {
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/json"))
        .send()
        .await
        .map_err(|e| AppError::NetworkError(format!("DevTools /json failed: {e}")))?;
    let Value::Array(targets) = resp
        .json::<Value>()
        .await
        .map_err(|e| AppError::NetworkError(format!("DevTools /json parse failed: {e}")))?
    else {
        return Err(AppError::NetworkError(
            "DevTools /json returned no array".into(),
        ));
    };
    let mut out = Vec::new();
    for target in targets {
        if target.get("type").and_then(|v| v.as_str()) != Some("page") {
            continue;
        }
        let Some(ws_url) = target.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) else {
            continue;
        };
        out.push(PageTarget {
            id: target
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            title: target
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            url: target
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            ws_url: ws_url.to_string(),
        });
    }
    Ok(out)
}

/// Wait for the DevTools endpoint, then return `(browser_ws, page_ws,
/// page_id)` for the first page target (fresh profiles always open one tab).
///
/// `Page.*` / `Input.*` CDP domains only exist on page-level targets; the
/// browser-level ws is kept for `Target.*` commands (tab create/close).
pub async fn wait_for_devtools(port: u16) -> AppResult<(String, String, String)> {
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    // Phase 1: browser-level endpoint must come up (readiness signal) and
    // yield the browser ws url.
    let browser_ws = loop {
        if let Ok(resp) = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/json/version"))
            .send()
            .await
        {
            if let Ok(Value::Object(map)) = resp.json::<Value>().await {
                if let Some(ws) = map.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                    break ws.to_string();
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::NetworkError(format!(
                "DevTools endpoint did not come up on port {port} within {STARTUP_TIMEOUT:?}"
            )));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    // Phase 2: find a page target (fresh profiles always open one tab).
    loop {
        if let Ok(targets) = fetch_page_targets(port).await {
            if let Some(first) = targets.into_iter().next() {
                return Ok((browser_ws, first.ws_url, first.id));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::NetworkError(format!(
                "No page target appeared on port {port} within {STARTUP_TIMEOUT:?}"
            )));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_non_empty() {
        assert!(!candidate_paths().is_empty(), "must always probe paths");
    }

    #[test]
    fn pick_free_port_returns_a_port() {
        let port = pick_free_port().expect("a free port");
        assert!(port > 0);
    }
}
