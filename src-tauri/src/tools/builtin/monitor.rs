//! Monitor tool — event monitoring with frequency limiting.
//!
//! Allows the agent to watch for events (file changes, task completions,
//! diagnostic changes) and report them back, with rate limiting to prevent
//! flooding the conversation with events.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use tauri::Emitter;

/// A monitored event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoredEvent {
    /// Event type (e.g., "file_changed", "task_completed", "diagnostic").
    pub event_type: String,
    /// Event payload (free-form JSON).
    pub payload: Value,
    /// Timestamp (epoch millis).
    pub timestamp_ms: u64,
}

/// Configuration for the monitor.
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// Maximum events to retain per session bucket.
    pub max_buffer_size: usize,
    /// Event types to watch (empty = all).
    pub watch_types: Vec<String>,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 100,
            watch_types: Vec::new(),
        }
    }
}

/// Shared event buffer — stores recent events per session so one session's
/// activity never leaks into another session's monitor view (subagents and
/// background tasks previously wrote into one global queue).
#[derive(Clone)]
pub struct EventBuffer {
    buckets: Arc<RwLock<HashMap<String, VecDeque<MonitoredEvent>>>>,
    config: MonitorConfig,
}

impl EventBuffer {
    /// Create a new event buffer with the given config.
    pub fn new(config: MonitorConfig) -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Push a new event into the buffer of the given session.
    pub fn push(&self, session_id: &str, event: MonitoredEvent) {
        // Filter by type if configured.
        if !self.config.watch_types.is_empty()
            && !self.config.watch_types.contains(&event.event_type)
        {
            return;
        }

        let mut buckets = self.buckets.write().unwrap_or_else(|e| e.into_inner());
        let queue = buckets
            .entry(session_id.to_string())
            .or_insert_with(|| VecDeque::with_capacity(self.config.max_buffer_size));
        if queue.len() >= self.config.max_buffer_size {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    /// Drain up to `n` events from the head of the session's buffer, leaving
    /// any overflow buffered for the next poll — a busy buffer must not lose
    /// events to a silent truncation. An emptied bucket is dropped so
    /// finished sessions release their memory.
    pub fn drain_head(&self, session_id: &str, n: usize) -> Vec<MonitoredEvent> {
        let mut buckets = self.buckets.write().unwrap_or_else(|e| e.into_inner());
        let take = n.min(buckets.get(session_id).map_or(0, VecDeque::len));
        let events: Vec<MonitoredEvent> = buckets
            .get_mut(session_id)
            .map_or(Vec::new(), |q| q.drain(..take).collect());
        if buckets.get(session_id).is_some_and(VecDeque::is_empty) {
            buckets.remove(session_id);
        }
        events
    }

    /// Peek at the most recent N events of the session without draining.
    pub fn recent(&self, session_id: &str, n: usize) -> Vec<MonitoredEvent> {
        let buckets = self.buckets.read().unwrap_or_else(|e| e.into_inner());
        buckets
            .get(session_id)
            .map_or(Vec::new(), |q| q.iter().rev().take(n).cloned().collect())
    }
}

/// Monitor tool — polls the event buffer and reports events to the agent.
pub struct MonitorTool {
    buffer: EventBuffer,
}

impl MonitorTool {
    /// Create a new monitor tool with the given event buffer.
    pub fn new(buffer: EventBuffer) -> Self {
        Self { buffer }
    }
}

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str {
        "monitor"
    }

    fn description(&self) -> &str {
        "Monitor events from the event buffer. Returns recent events \
        (file changes, task completions, diagnostics) since the last call. \
        Use this to check for changes while waiting on long-running operations."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_events": {
                    "type": "integer",
                    "description": "Maximum number of events to return (default 10)",
                    "default": 10
                },
                "drain": {
                    "type": "boolean",
                    "description": "Whether to remove events from the buffer after reading (default true)",
                    "default": true
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        // Debug/observability aid for coding workflows (event buffer of
        // tool/file activity) — not part of Depwork's office toolset.
        crate::toolkit::ToolScope::Code
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let max_events = args
            .get("max_events")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let drain = args.get("drain").and_then(|v| v.as_bool()).unwrap_or(true);

        let events: Vec<MonitoredEvent> = if drain {
            // Take the head up to max_events; overflow stays buffered for
            // the next poll instead of being silently dropped. Only this
            // session's bucket is touched — other sessions' events are
            // never visible here.
            self.buffer.drain_head(&ctx.session_id, max_events)
        } else {
            self.buffer.recent(&ctx.session_id, max_events)
        };

        if events.is_empty() {
            return Ok(ToolResult::success("No events to report."));
        }

        let lines: Vec<String> = events
            .iter()
            .map(|e| {
                format!(
                    "[{}] {}: {}",
                    e.timestamp_ms,
                    e.event_type,
                    serde_json::to_string(&e.payload).unwrap_or_default()
                )
            })
            .collect();

        let _ = ctx.app.emit(
            "monitor-events-read",
            &serde_json::json!({
                "count": events.len(),
            }),
        );

        Ok(ToolResult::success(format!(
            "{} event(s):\n{}",
            events.len(),
            lines.join("\n")
        )))
    }
}
