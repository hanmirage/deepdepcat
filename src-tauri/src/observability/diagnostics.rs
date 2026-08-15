//! Anonymous diagnostics — silent tool-error aggregation + upload.
//!
//! Privacy-first error telemetry (opt-out, default on, like OpenAI's
//! "help improve the product" setting):
//! - Aggregates ONLY `(tool_name, error_kind, count)` — never tool arguments,
//!   never conversation content, never session ids, never anything that can
//!   identify the user or the machine (no client_id here — this is meant to
//!   be unlinkable).
//! - Uploads in small batches to `POST /api/v1/telemetry/collect` with
//!   `event_type = "diagnostics"`. That endpoint already exists and needs no
//!   auth; the payload is just counts.
//! - When the toggle is off, nothing is recorded and nothing is sent.
//!
//! This answers "which tools keep failing" without exposing any content.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, warn};

/// A single aggregated error counter row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCount {
    pub tool: String,
    pub error_kind: String,
    pub count: u32,
}

/// The global opt-in/opt-out toggle. Defaults to ON (respects the user's
/// choice made in Settings → Privacy). When OFF, `record` is a no-op and no
/// network request is ever made.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Set the diagnostics toggle from the Settings UI.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether diagnostics collection is currently on.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Aggregates error counts and flushes them to the server in batches.
pub struct DiagnosticsReporter {
    inner: Mutex<Inner>,
}

struct Inner {
    /// (tool_name, error_kind) → count
    counts: HashMap<(String, String), u32>,
}

impl DiagnosticsReporter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                counts: HashMap::new(),
            }),
        })
    }

    /// Record one tool error. No-op when diagnostics are disabled.
    pub fn record(&self, tool: &str, error_kind: &str) {
        if !is_enabled() {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            *inner
                .counts
                .entry((tool.to_string(), error_kind.to_string()))
                .or_insert(0) += 1;
        }
    }

    /// Drain accumulated counts into a payload (used by the flush task).
    fn drain(&self) -> Vec<DiagnosticCount> {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::with_capacity(inner.counts.len());
        for ((tool, kind), count) in inner.counts.drain() {
            out.push(DiagnosticCount {
                tool,
                error_kind: kind,
                count,
            });
        }
        out
    }

    /// Flush any accumulated counts to the server. Best-effort — a failed
    /// upload drops the batch (counts are diagnostics, not accounting).
    /// The server URL is read from the caller (AppState) so it always
    /// respects the user's configured backend.
    pub async fn flush_with(&self, server_url: &str) {
        if !is_enabled() {
            return;
        }
        let counts = self.drain();
        if counts.is_empty() {
            return;
        }
        if server_url.is_empty() {
            return;
        }
        let base = server_url.trim_end_matches('/').to_string();

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Diagnostics: failed to build HTTP client");
                return;
            }
        };

        // Deliberately anonymous: no session_id, no client_id, just counts.
        // Shape matches the server's `TelemetryEvent` model.
        let payload = serde_json::json!({
            "session_id": "diagnostics",
            "event_type": "diagnostics",
            "span": "",
            "event_name": "tool_errors",
            "data": {
                "tool_errors": counts,
            },
        });

        let resp = match client
            .post(format!("{base}/api/v1/telemetry/collect"))
            .json(&payload)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, "Diagnostics: upload failed (dropped batch)");
                return;
            }
        };
        if !resp.status().is_success() {
            debug!(
                status = %resp.status(),
                "Diagnostics: upload rejected (dropped batch)"
            );
        }
    }
}

/// Spawn a background flush loop that pushes aggregated counts periodically.
/// `server_url` is resolved on every tick from the provided closure, so the
/// loop always respects the user's current configured backend.
pub fn spawn_flush_loop<F>(reporter: Arc<DiagnosticsReporter>, interval: Duration, server_url: F)
where
    F: Fn() -> String + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            reporter.flush_with(&server_url()).await;
        }
    });
}
