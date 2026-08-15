//! CDP screencast — live frame streaming for the embedded dev browser.
//!
//! The takeover browser is a real Chromium driven over CDP. `Page.startScreencast`
//! makes the browser push JPEG frames (~10fps) over the DevTools socket; this
//! controller relays each frame to the frontend as a `browser-screencast-frame`
//! event, so the right-panel dev browser renders the LIVE page instead of a
//! static screenshot. Input forwarding (mouse/key) lives in the commands layer.
//!
//! One long-lived task per profile; `start` is idempotent and `stop` cancels
//! the task. Every frame MUST be acked (`Page.screencastFrameAck`) or the
//! browser stops sending.

use crate::browser::cdp::CdpClient;
use crate::browser::BrowserManager;
use crate::core::error::AppResult;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio_util::sync::CancellationToken;

/// Event emitted once per screencast frame (base64 JPEG + viewport size).
pub const EVENT_SCREENCAST_FRAME: &str = "browser-screencast-frame";

/// One frame relayed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ScreencastFramePayload {
    pub profile: String,
    /// Base64 JPEG (no `data:` prefix — the frontend prepends it).
    pub jpeg: String,
    /// Page viewport width in CSS pixels — the frontend maps clicks by
    /// scaling its rendered width against this.
    pub vw: u32,
    /// Page viewport height in CSS pixels.
    pub vh: u32,
    /// Monotonic frame counter (frontend render throttle / debug).
    pub seq: u64,
}

/// Controls per-profile screencast tasks.
pub struct ScreencastController {
    /// profile → stop token of its frame task.
    running: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl Default for ScreencastController {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreencastController {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start streaming frames for a profile (idempotent). The caller must
    /// already have launched the browser session for that profile.
    pub async fn start(
        &self,
        app: tauri::AppHandle,
        manager: &BrowserManager,
        profile: &str,
    ) -> AppResult<()> {
        if self.is_running(profile) {
            return Ok(());
        }
        let ws = manager.require_ws(profile).await?;
        let client = CdpClient::connect(&ws).await?;
        let mut frames = client.subscribe("Page.screencastFrame");
        client.call("Page.enable", serde_json::json!({})).await?;

        let token = CancellationToken::new();
        self.running
            .lock()
            .unwrap()
            .insert(profile.to_string(), token.clone());

        let app = app;
        let profile_owned = profile.to_string();
        let cancel = token.clone();
        let running = self.running.clone();
        tauri::async_runtime::spawn(async move {
            let _ = client
                .call(
                    "Page.startScreencast",
                    serde_json::json!({
                        "format": "jpeg",
                        "quality": 60,
                        "maxWidth": 1024,
                        "everyNthFrame": 6,
                    }),
                )
                .await;
            let mut seq: u64 = 0;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    frame = frames.recv() => {
                        let Some(params) = frame else { break };
                        // Ack first — the browser stops streaming otherwise.
                        if let Some(sid) = params.get("sessionId").and_then(|s| s.as_str()) {
                            let _ = client
                                .call("Page.screencastFrameAck", serde_json::json!({ "sessionId": sid }))
                                .await;
                        }
                        let Some(jpeg) = params.get("data").and_then(|d| d.as_str()) else {
                            continue;
                        };
                        if jpeg.is_empty() {
                            continue;
                        }
                        let (vw, vh) = viewport_from(&params);
                        seq += 1;
                        let _ = app.emit(
                            EVENT_SCREENCAST_FRAME,
                            ScreencastFramePayload {
                                profile: profile_owned.clone(),
                                jpeg: jpeg.to_string(),
                                vw,
                                vh,
                                seq,
                            },
                        );
                    }
                }
            }
            let _ = client
                .call("Page.stopScreencast", serde_json::json!({}))
                .await;
            // The frame task ended on its own (stream closed / browser died) —
            // drop the running marker so a later `start` is not a silent no-op.
            running.lock().unwrap().remove(&profile_owned);
        });
        Ok(())
    }

    /// Stop a profile's frame task (best-effort; idempotent).
    pub fn stop(&self, profile: &str) {
        if let Some(token) = self.running.lock().unwrap().remove(profile) {
            token.cancel();
        }
    }

    pub fn is_running(&self, profile: &str) -> bool {
        self.running.lock().unwrap().contains_key(profile)
    }
}

/// Extract the page viewport size from a screencast frame's metadata.
fn viewport_from(params: &serde_json::Value) -> (u32, u32) {
    let vp = params
        .get("metadata")
        .and_then(|m| m.get("viewport"))
        .unwrap_or(&serde_json::Value::Null);
    let w = vp.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let h = vp.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_extracted_from_metadata() {
        let params = serde_json::json!({
            "metadata": {
                "viewport": { "x": 0, "y": 0, "width": 1280, "height": 900, "scale": 1 }
            }
        });
        assert_eq!(viewport_from(&params), (1280, 900));
    }

    #[test]
    fn viewport_defaults_to_zero_when_missing() {
        assert_eq!(viewport_from(&serde_json::json!({})), (0, 0));
        assert_eq!(
            viewport_from(&serde_json::json!({ "metadata": {} })),
            (0, 0)
        );
    }

    #[tokio::test]
    async fn stop_without_start_is_noop() {
        let c = ScreencastController::new();
        c.stop("takeover");
        assert!(!c.is_running("takeover"));
    }

    /// End-to-end screencast smoke: launch a REAL Edge/Chrome, start the
    /// frame stream, receive + ack at least one frame, tear down.
    /// `cargo test --lib -- --ignored browser::screencast`
    #[tokio::test]
    #[ignore = "launches a real browser — manual smoke test"]
    async fn smoke_screencast_streams_frames() {
        use crate::browser::{BrowserManager, DEFAULT_PROFILE};
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("browser-sc-{}", std::process::id()));
        let m = BrowserManager::new(dir);
        m.start(Some("https://example.com")).await.expect("launch");
        let ws = m.require_ws(DEFAULT_PROFILE).await.expect("ws url");
        let client = CdpClient::connect(&ws).await.expect("cdp connect");
        let mut frames = client.subscribe("Page.screencastFrame");
        client
            .call("Page.enable", serde_json::json!({}))
            .await
            .expect("enable page");
        client
            .call(
                "Page.startScreencast",
                serde_json::json!({ "format": "jpeg", "quality": 60, "maxWidth": 1024 }),
            )
            .await
            .expect("start screencast");
        let frame = tokio::time::timeout(Duration::from_secs(8), frames.recv())
            .await
            .expect("frame timeout")
            .expect("frame stream closed");
        let jpeg = frame["data"].as_str().expect("jpeg data");
        assert!(jpeg.len() > 100, "frame must be a real JPEG, got {} chars", jpeg.len());
        let sid = frame["sessionId"].as_str().expect("session id");
        client
            .call("Page.screencastFrameAck", serde_json::json!({ "sessionId": sid }))
            .await
            .expect("ack");
        client
            .call("Page.stopScreencast", serde_json::json!({}))
            .await
            .expect("stop screencast");
        assert!(m.stop().await, "stop must report the running session");
    }
}
