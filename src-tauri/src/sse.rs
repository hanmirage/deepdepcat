//! Real SSE transport — streams the app's raw `chat-stream` events over
//! HTTP Server-Sent Events on loopback, so the frontend (and any local
//! client) consumes the same wire the agent loop emits, without Tauri IPC.
//!
//! Server binds to 127.0.0.1 on a DYNAMIC port; the port is exposed through
//! `get_sse_port` so the webview can open an EventSource. CORS is permissive
//! for loopback (the webview origin differs from `http://127.0.0.1`).

use axum::extract::State as AxumState;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::stream::Stream;
use std::convert::Infallible;
use tokio::sync::broadcast;
use tracing::{error, info};

/// Broadcast capacity — a fast agent turn can emit hundreds of deltas.
const BUS_CAPACITY: usize = 4096;

/// Event name used on the wire (`event: chat-stream`).
const CHAT_STREAM_EVENT: &str = "chat-stream";

/// The raw-event hub: any producer (`emit_raw`) fans out to every connected
/// EventSource subscriber.
#[derive(Clone)]
pub struct SseHub {
    tx: broadcast::Sender<String>,
}

impl Default for SseHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SseHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Publish one pre-serialized event payload (the raw Tauri
    /// `chat-stream` JSON) to every subscriber as an SSE line.
    pub fn emit_raw(&self, payload: &str) {
        let line = format!("event: {CHAT_STREAM_EVENT}\ndata: {payload}\n\n");
        let _ = self.tx.send(line);
    }

    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

/// Split an SSE-formatted line into `(event, data)` — mirrors the ACP
/// parser so both transports share one wire contract.
pub fn parse_sse_line(line: &str) -> (String, String) {
    let mut event = String::new();
    let mut data = String::new();
    for field in line.split('\n') {
        if let Some(value) = field.strip_prefix("event:") {
            event = value.trim().to_string();
        } else if let Some(value) = field.strip_prefix("data:") {
            data = value.trim().to_string();
        }
    }
    (event, data)
}

/// The SSE handler: subscribes at connect time and forwards every
/// `chat-stream` line until the client disconnects.
async fn chat_stream_handler(
    AxumState(hub): AxumState<SseHub>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = hub.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    let (event, data) = parse_sse_line(&line);
                    let mut sse_event = Event::default().data(data);
                    if !event.is_empty() {
                        sse_event = sse_event.event(event);
                    }
                    yield Ok(sse_event);
                }
                // A slow subscriber missed deltas — skip forward instead of
                // closing (the stream is lossy by design for live UI).
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Loopback-permissive CORS: the Tauri webview origin (tauri://localhost or
/// http://localhost:1420 in dev) differs from `http://127.0.0.1:<port>`, and
/// EventSource responses need `Access-Control-Allow-Origin` to be readable.
async fn cors_layer(request: axum::extract::Request, next: Next) -> Result<Response, StatusCode> {
    if request.method() == axum::http::Method::OPTIONS {
        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, OPTIONS")
            .header("Access-Control-Allow-Headers", "content-type")
            .body(axum::body::Body::empty())
            .unwrap());
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    Ok(response)
}

/// Build the SSE router (loopback only; the caller binds the listener).
pub fn router(hub: SseHub) -> Router {
    Router::new()
        .route("/sse/chat-stream", get(chat_stream_handler))
        .layer(middleware::from_fn(cors_layer))
        .with_state(hub)
}

/// Bind 127.0.0.1 (port `0` = dynamic) and serve the SSE router in the
/// background. Returns the actual bound port.
pub async fn serve(hub: SseHub, port: u16) -> std::io::Result<u16> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?.port();
    info!(port = actual, "SSE transport listening on loopback");
    tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(listener, router(hub)).await {
            error!(error = %e, "SSE transport server exited");
        }
    });
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_line_roundtrips() {
        let line = "event: chat-stream\ndata: {\"type\":\"text_delta\",\"text\":\"hi\"}\n\n";
        let (event, data) = parse_sse_line(line);
        assert_eq!(event, "chat-stream");
        assert!(data.contains("\"text\":\"hi\""));
    }

    #[test]
    fn hub_emits_and_subscriber_receives() {
        let hub = SseHub::new();
        let mut rx = hub.subscribe();
        hub.emit_raw("{\"type\":\"turn_start\"}");
        let line = rx.try_recv().expect("line delivered");
        assert!(line.starts_with("event: chat-stream\ndata: "));
    }
}
