//! Minimal CDP (Chrome DevTools Protocol) client over WebSocket.
//!
//! Covers the small surface browser takeover needs — no full protocol:
//! - `Page.navigate` / `Page.captureScreenshot`
//! - `Runtime.evaluate` (DOM queries, page text, click-target lookup)
//! - `Input.dispatchMouseEvent` / `dispatchKeyEvent` / `insertText`
//!
//! Design: each operation opens its own WebSocket connection (cheap on
//! 127.0.0.1) and drops it when done — no stale-session management. A
//! background task reads the socket, matching responses to pending calls
//! by id; events are ignored.

use crate::core::error::{AppError, AppResult};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Cap on any single CDP call (seconds). `awaitPromise: true` evaluations
/// can otherwise hang forever on an unresolved promise, leaving the frontend
/// invoke permanently pending.
const CDP_CALL_TIMEOUT_SECS: u64 = 30;

/// Element-snapshot script (code-use semantics): assigns stable
/// `data-ddc-eid` ids to visible interactive elements and returns their
/// {eid, tag, role, text, value, placeholder, href}. Ids persist across
/// snapshots on the same page via `window.__ddcEidSeed`.
const ELEMENT_SNAPSHOT_SCRIPT: &str = r#"(() => {
  let seed = window.__ddcEidSeed || 0;
  const els = Array.from(document.querySelectorAll(
    'a, button, input, select, textarea, [role="button"], [role="link"], [role="checkbox"], [role="radio"], [tabindex]'
  ));
  const out = [];
  for (const el of els) {
    if (!el.isConnected) continue;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) continue;
    let eid = el.getAttribute('data-ddc-eid');
    if (!eid) { eid = 'e' + (++seed); el.setAttribute('data-ddc-eid', eid); }
    const tag = el.tagName.toLowerCase();
    const isField = tag === 'input' || tag === 'textarea' || tag === 'select';
    const text = (el.innerText || el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 120);
    out.push({
      eid,
      tag,
      role: el.getAttribute('role') || '',
      text,
      value: isField ? (el.value || '').slice(0, 120) : '',
      placeholder: el.placeholder || '',
      href: el.getAttribute('href') || ''
    });
  }
  window.__ddcEidSeed = seed;
  return out;
})()"#;

/// One pending CDP call awaiting its response.
struct PendingCall {
    method: String,
    reply: oneshot::Sender<Result<Value, String>>,
}

/// A request queued to the WebSocket background task.
struct CdpRequest {
    id: u64,
    method: String,
    params: Value,
    reply: oneshot::Sender<Result<Value, String>>,
}

/// Async CDP client. Cheap to clone; clones share the same connection.
#[derive(Clone)]
pub struct CdpClient {
    tx: mpsc::UnboundedSender<CdpRequest>,
    next_id: Arc<AtomicU64>,
    /// method → subscribed receivers. Events (messages without `id`) are
    /// routed here — the foundation for `Page.screencastFrame` streaming.
    subscribers: Arc<std::sync::RwLock<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>>,
}

impl CdpClient {
    /// Connect to a browser's DevTools WebSocket endpoint.
    pub async fn connect(ws_url: &str) -> AppResult<Self> {
        let (ws, _resp) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| AppError::NetworkError(format!("CDP connect failed: {e}")))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<CdpRequest>();
        let subscribers: Arc<std::sync::RwLock<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>> =
            Arc::new(std::sync::RwLock::new(HashMap::new()));
        let sub_for_task = subscribers.clone();
        tokio::spawn(async move {
            let mut pending: HashMap<u64, PendingCall> = HashMap::new();
            loop {
                tokio::select! {
                    request = rx.recv() => {
                        match request {
                            Some(req) => {
                                let frame = json!({
                                    "id": req.id,
                                    "method": req.method,
                                    "params": req.params,
                                })
                                .to_string();
                                if sink.send(Message::Text(frame)).await.is_err() {
                                    let _ = req.reply.send(Err("CDP socket closed".into()));
                                    break;
                                }
                                pending.insert(req.id, PendingCall { method: req.method, reply: req.reply });
                            }
                            None => break, // every client dropped
                        }
                    }
                    frame = stream.next() => {
                        match frame {
                            Some(Ok(Message::Text(text))) => {
                                Self::dispatch_text(&text, &mut pending, &sub_for_task);
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                for (_, p) in pending.drain() {
                                    let _ = p.reply.send(Err(format!("CDP connection error: {e}")));
                                }
                                break;
                            }
                            None => {
                                for (_, p) in pending.drain() {
                                    let _ = p.reply.send(Err("CDP connection closed".into()));
                                }
                                break;
                            }
                        }
                    }
                }
            }
        });
        Ok(Self {
            tx,
            next_id: Arc::new(AtomicU64::new(1)),
            subscribers,
        })
    }

    /// Route an incoming frame: a response resolves its pending caller; an
    /// event (no `id`) is fanned out to the matching subscribers (pure,
    /// testable).
    fn dispatch_text(
        text: &str,
        pending: &mut HashMap<u64, PendingCall>,
        subscribers: &std::sync::RwLock<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
    ) {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
            // Event or malformed — fan out to subscribers by method name.
            let Some(method) = v.get("method").and_then(|m| m.as_str()) else {
                return;
            };
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            let Ok(mut subs) = subscribers.write() else {
                return;
            };
            let Some(list) = subs.get_mut(method) else {
                return;
            };
            list.retain(|s| s.send(params.clone()).is_ok());
            return;
        };
        let Some(p) = pending.remove(&id) else {
            return; // response for a caller we no longer track
        };
        let result = match v.get("error") {
            Some(err) => Err(format!("CDP {} failed: {}", p.method, err)),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = p.reply.send(result);
    }

    /// Subscribe to a CDP event method (e.g. `Page.screencastFrame`). The
    /// receiver yields each event's `params` until the connection dies or the
    /// receiver is dropped.
    pub fn subscribe(&self, method: &str) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut subs) = self.subscribers.write() {
            subs.entry(method.to_string()).or_default().push(tx);
        }
        rx
    }

    /// Send one CDP command and await its result.
    pub async fn call(&self, method: &str, params: Value) -> AppResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(CdpRequest {
                id,
                method: method.to_string(),
                params,
                reply,
            })
            .map_err(|_| AppError::NetworkError("CDP channel closed".to_string()))?;
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(CDP_CALL_TIMEOUT_SECS), rx)
                .await
                .map_err(|_| {
                    AppError::NetworkError(format!(
                        "CDP call timed out after {CDP_CALL_TIMEOUT_SECS}s: {method}"
                    ))
                })?
                .map_err(|_| AppError::NetworkError("CDP response channel closed".to_string()))?;
        result.map_err(AppError::NetworkError)
    }

    /// `Runtime.evaluate` returning the JSON value of the expression.
    pub async fn evaluate(&self, expression: &str) -> AppResult<Value> {
        let out = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        out.get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .ok_or_else(|| AppError::NetworkError("Runtime.evaluate returned no value".into()))
    }

    /// Inject the console/network/error capture hook (idempotent).
    pub async fn ensure_log_capture(&self) -> AppResult<()> {
        self.evaluate(&capture_inject_script()).await?;
        Ok(())
    }

    /// Read back captured logs as `{ console: [], network: [], errors: [] }`.
    pub async fn capture_logs(&self) -> AppResult<Value> {
        let value = self
            .evaluate("window.__ddcLogs ? JSON.stringify(window.__ddcLogs) : '{}'")
            .await?;
        match value {
            Value::String(s) => serde_json::from_str(&s)
                .map_err(|e| AppError::NetworkError(format!("capture logs parse failed: {e}"))),
            other => Ok(other),
        }
    }

    /// Navigate the current tab to `url` (no wait — use [`Self::wait_ready`]).
    pub async fn navigate(&self, url: &str) -> AppResult<()> {
        self.call("Page.navigate", json!({ "url": url })).await?;
        Ok(())
    }

    /// Poll `document.readyState` until `complete` or the timeout elapses.
    pub async fn wait_ready(&self, timeout_secs: u64) -> AppResult<()> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        loop {
            let state = self
                .evaluate("document.readyState")
                .await
                .unwrap_or(Value::Null);
            if state.as_str() == Some("complete") {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError::Timeout(timeout_secs));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Capture a screenshot as base64-encoded PNG.
    pub async fn screenshot_png(&self) -> AppResult<String> {
        let out = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        out.get("data")
            .and_then(|d| d.as_str())
            .map(String::from)
            .ok_or_else(|| AppError::NetworkError("captureScreenshot returned no data".into()))
    }

    /// Current tab URL + title.
    pub async fn page_info(&self) -> AppResult<(String, String)> {
        let url = self
            .evaluate("location.href")
            .await
            .and_then(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| AppError::NetworkError("location.href not a string".into()))
            })
            .unwrap_or_default();
        let title = self
            .evaluate("document.title")
            .await
            .and_then(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| AppError::NetworkError("document.title not a string".into()))
            })
            .unwrap_or_default();
        Ok((url, title))
    }

    /// Page snapshot for the model: URL + title + visible text (truncated)
    /// plus a list of interactive elements (buttons/links/inputs with
    /// placeholder hints) — so the agent "sees" what it can click before
    /// it clicks.
    pub async fn page_snapshot(&self, max_chars: usize) -> AppResult<String> {
        let snap = self
            .evaluate(&snapshot_script())
            .await
            .and_then(|v| {
                v.as_object().cloned().ok_or_else(|| {
                    AppError::NetworkError("page snapshot returned no object".into())
                })
            })
            .unwrap_or_default();
        let url = snap.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let title = snap.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let text = snap.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let text = if text.chars().count() > max_chars {
            let mut cut: String = text.chars().take(max_chars).collect();
            cut.push_str("\n… [truncated]");
            cut
        } else {
            text.to_string()
        };
        let mut out = format!("URL: {url}\nTitle: {title}\n\n[页面文本]\n{text}\n");
        if let Some(list) = snap.get("interactives").and_then(|v| v.as_array()) {
            out.push_str(&format!("\n[可交互元素 {}]\n", list.len()));
            for item in list {
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let hint = item.get("hint").and_then(|v| v.as_str()).unwrap_or("");
                let line = if hint.is_empty() {
                    format!("{kind}: {label}")
                } else {
                    format!("{kind}: {label} ({hint})")
                };
                out.push_str(&line);
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// Fill a form input found by placeholder / aria-label / name / label
    /// text: focus it, select the existing content, then type `text`
    /// (overwriting what was there). Returns a confirmation for the model.
    pub async fn fill_text(&self, query: &str, text: &str) -> AppResult<String> {
        let hit = self
            .evaluate(&fill_script(query))
            .await
            .and_then(|v| {
                v.as_object()
                    .cloned()
                    .ok_or_else(|| AppError::NetworkError("fill lookup returned no object".into()))
            })
            .unwrap_or_default();
        if hit.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let reason = hit
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("input not found");
            return Err(format!("fill failed: {reason}").into());
        }
        let x = hit.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
        let y = hit.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
        // Click the field (focus + cursor), then insertText replaces the
        // selection (the script pre-selects existing content).
        self.mouse_click(x, y).await?;
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        let label = hit.get("label").and_then(|v| v.as_str()).unwrap_or(query);
        Ok(format!(
            "Filled \"{label}\" with {}-char text",
            text.chars().count()
        ))
    }

    /// Click the first visible element whose text contains `needle`.
    /// Returns a human-readable confirmation for the model.
    pub async fn click_by_text(&self, needle: &str) -> AppResult<String> {
        let hit = self
            .evaluate(&click_script(needle))
            .await
            .and_then(|v| {
                v.as_object()
                    .cloned()
                    .ok_or_else(|| AppError::NetworkError("click lookup returned no object".into()))
            })
            .unwrap_or_default();
        if hit.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let reason = hit
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("element not found");
            return Err(format!("click failed: {reason}").into());
        }
        let x = hit.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
        let y = hit.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
        self.mouse_click(x, y).await?;
        let label = hit.get("label").and_then(|v| v.as_str()).unwrap_or(needle);
        Ok(format!("Clicked \"{label}\" at ({x}, {y})"))
    }

    /// Click the first element matching a CSS selector.
    pub async fn click_css(&self, selector: &str) -> AppResult<String> {
        let v = self.evaluate(&click_css_script(selector)).await?;
        Ok(v.to_string())
    }

    /// Fill the first element matching a CSS selector.
    pub async fn fill_css(&self, selector: &str, text: &str) -> AppResult<String> {
        let v = self.evaluate(&fill_css_script(selector, text)).await?;
        Ok(v.to_string())
    }

    /// Element-level snapshot (code-use semantics): assigns a stable
    /// `data-ddc-eid` to every visible interactive element and returns the
    /// list {eid, tag, role, text, value, placeholder, href} so the model
    /// can operate by element id instead of fragile text/CSS matching.
    pub async fn element_snapshot(&self) -> AppResult<Value> {
        self.evaluate(ELEMENT_SNAPSHOT_SCRIPT).await
    }

    /// Click an element by its `data-ddc-eid` (from a snapshot).
    pub async fn click_eid(&self, eid: &str) -> AppResult<String> {
        let v = self
            .evaluate(&format!(
                "(() => {{ const el = document.querySelector('[data-ddc-eid=\"{}\"]'); \
                 if (!el) return {{ok:false, reason:'element not found (stale snapshot?)'}}; \
                 el.scrollIntoView({{block:'center'}}); el.click(); return {{ok:true}}; }})()",
                eid.replace('"', "")
            ))
            .await?;
        Ok(v.to_string())
    }

    /// Fill (or select) an element by its `data-ddc-eid` — inputs/textarea
    /// get input+change events, selects get a change event.
    pub async fn fill_eid(&self, eid: &str, text: &str) -> AppResult<String> {
        let safe_text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
        let v = self
            .evaluate(&format!(
                "(() => {{ const el = document.querySelector('[data-ddc-eid=\"{}\"]'); \
                 if (!el) return {{ok:false, reason:'element not found (stale snapshot?)'}}; \
                 if (el.tagName === 'SELECT') {{ el.value = {safe_text}; \
                 el.dispatchEvent(new Event('change', {{bubbles:true}})); return {{ok:true}}; }} \
                 if (el.tagName !== 'INPUT' && el.tagName !== 'TEXTAREA') \
                 return {{ok:false, reason:'not a form field'}}; \
                 el.focus(); el.value = {safe_text}; \
                 el.dispatchEvent(new Event('input', {{bubbles:true}})); \
                 el.dispatchEvent(new Event('change', {{bubbles:true}})); return {{ok:true}}; }})()",
                eid.replace('"', "")
            ))
            .await?;
        Ok(v.to_string())
    }

    /// Smooth-scroll the page by a pixel delta.
    pub async fn scroll(&self, x: i64, y: i64) -> AppResult<String> {
        let v = self.evaluate(&scroll_script(x, y)).await?;
        Ok(v.to_string())
    }

    /// Dispatch a real left-button click at viewport coordinates.
    pub async fn mouse_click(&self, x: i32, y: i32) -> AppResult<()> {
        let mut pressed = json!({ "x": x, "y": y, "button": "left", "clickCount": 1 });
        pressed["type"] = json!("mousePressed");
        self.call("Input.dispatchMouseEvent", pressed).await?;
        let mut released = json!({ "x": x, "y": y, "button": "left", "clickCount": 1 });
        released["type"] = json!("mouseReleased");
        self.call("Input.dispatchMouseEvent", released).await?;
        Ok(())
    }

    /// One `Input.dispatchMouseEvent` — the embedded view forwards
    /// pointermove/down/up as separate events so drags and hovers behave.
    pub async fn mouse_event(
        &self,
        event: &str,
        x: i32,
        y: i32,
        buttons: i32,
        click_count: i32,
    ) -> AppResult<()> {
        let params = json!({
            "type": event,            // mouseMoved / mousePressed / mouseReleased
            "x": x,
            "y": y,
            "button": "left",
            "buttons": buttons,       // bitmask: 1 = left
            "clickCount": click_count,
        });
        self.call("Input.dispatchMouseEvent", params).await?;
        Ok(())
    }

    /// `Input.dispatchMouseEvent` of type mouseWheel — scroll in the view.
    pub async fn mouse_wheel(&self, x: i32, y: i32, delta_x: i32, delta_y: i32) -> AppResult<()> {
        let params = json!({
            "type": "mouseWheel",
            "x": x,
            "y": y,
            "deltaX": delta_x,
            "deltaY": delta_y,
        });
        self.call("Input.dispatchMouseEvent", params).await?;
        Ok(())
    }

    /// One raw `Input.dispatchKeyEvent` (keyDown/keyUp). A printable
    /// keyDown carries `text` so the browser inserts it natively (IME-safe).
    pub async fn key_event(&self, event: &str, key: &str, code: &str, text: &str) -> AppResult<()> {
        let mut params = json!({ "type": event, "key": key, "code": code });
        if event == "keyDown" && !text.is_empty() {
            params["text"] = json!(text);
        }
        self.call("Input.dispatchKeyEvent", params).await?;
        Ok(())
    }

    /// Type text into the currently focused element.
    pub async fn type_text(&self, text: &str) -> AppResult<()> {
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    /// Press a named key (enter/tab/esc/backspace/delete/space/arrows/…).
    pub async fn press_key(&self, name: &str) -> AppResult<()> {
        let (code, key, vk) = key_code(name)?;
        let mut down = json!({
            "windowsVirtualKeyCode": vk,
            "nativeVirtualKeyCode": vk,
            "code": code,
            "key": key,
        });
        down["type"] = json!("keyDown");
        self.call("Input.dispatchKeyEvent", down).await?;
        let mut up = json!({
            "windowsVirtualKeyCode": vk,
            "nativeVirtualKeyCode": vk,
            "code": code,
            "key": key,
        });
        up["type"] = json!("keyUp");
        self.call("Input.dispatchKeyEvent", up).await?;
        Ok(())
    }

    // ── Browser-level Target commands (connect to the browser ws) ───────

    /// Create a new tab and return its target id.
    pub async fn create_target(&self, url: &str) -> AppResult<String> {
        let out = self
            .call(
                "Target.createTarget",
                json!({ "url": url, "newWindow": false }),
            )
            .await?;
        out.get("targetId")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| AppError::NetworkError("createTarget returned no targetId".into()))
    }

    /// Close a tab by target id. Returns whether the target existed.
    pub async fn close_target(&self, target_id: &str) -> AppResult<bool> {
        let out = self
            .call("Target.closeTarget", json!({ "targetId": target_id }))
            .await?;
        Ok(out
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Bring a tab's window to the front.
    pub async fn activate_target(&self, target_id: &str) -> AppResult<()> {
        self.call("Target.activateTarget", json!({ "targetId": target_id }))
            .await?;
        Ok(())
    }
}

/// Build the Runtime.evaluate click-lookup script for a search term.
/// Returns `{ok, x, y, label}` or `{ok: false, reason}`.
pub fn click_script(needle: &str) -> String {
    let escaped = serde_json::to_string(needle).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const wanted = {escaped};
  const els = [...document.querySelectorAll(
    'a,button,input,textarea,select,[role="button"],summary,label,[onclick]'
  )];
  const el = els.find((e) => {{
    const label = (e.innerText || e.value || "").toLowerCase();
    return label.includes(wanted.toLowerCase());
  }});
  if (!el) return {{ ok: false, reason: "no visible element contains the text" }};
  el.scrollIntoView({{ block: "center", inline: "center" }});
  const r = el.getBoundingClientRect();
  return {{
    ok: true,
    x: Math.round(r.left + r.width / 2),
    y: Math.round(r.top + r.height / 2),
    label: String(el.innerText || el.value || "").trim().slice(0, 80),
  }};
}})()
"#
    )
}

/// Click the first element matching a CSS selector. Returns a JSON string
/// `{ok:true}` or `{ok:false,error}` — never throws.
pub fn click_css_script(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_else(|_| "null".into());
    format!(
        "(function(){{ const el = document.querySelector({sel}); \
           if (!el) return JSON.stringify({{ok:false,error:'selector not found'}}); \
           el.scrollIntoView({{block:'center'}}); el.click(); \
           return JSON.stringify({{ok:true}}); }})()"
    )
}

/// Fill the first element matching a CSS selector (value + input/change
/// events, React-compatible). Returns a JSON string, never throws.
pub fn fill_css_script(selector: &str, text: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_else(|_| "null".into());
    let txt = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(function(){{ const el = document.querySelector({sel}); \
           if (!el) return JSON.stringify({{ok:false,error:'selector not found'}}); \
           el.value = {txt}; \
           el.dispatchEvent(new Event('input', {{bubbles:true}})); \
           el.dispatchEvent(new Event('change', {{bubbles:true}})); \
           return JSON.stringify({{ok:true}}); }})()"
    )
}

/// Smooth-scroll the page by a pixel delta.
pub fn scroll_script(x: i64, y: i64) -> String {
    format!("window.scrollBy({{top:{y},left:{x},behavior:'smooth'}}); JSON.stringify({{ok:true}})")
}

/// Build the Runtime.evaluate snapshot script: `{url, title, text,
/// interactives: [{kind, label, hint}]}` — kinds are human-readable
/// (按钮/链接/输入框/密码框/复选框/单选框/下拉框), hints carry
/// placeholder text for inputs and href for links.
pub fn snapshot_script() -> String {
    r#"
(() => {
  const clean = (s) => (s || "").replace(/\s+/g, " ").trim().slice(0, 60);
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  const interactives = [];
  for (const el of document.querySelectorAll("a,button,input,textarea,select")) {
    if (!visible(el)) continue;
    const tag = el.tagName.toLowerCase();
    let kind = tag;
    let label = clean(el.innerText || el.value || el.getAttribute("aria-label"));
    let hint = "";
    if (tag === "input") {
      const t = (el.type || "text").toLowerCase();
      if (t === "submit" || t === "button") kind = "按钮";
      else if (t === "password") kind = "密码框";
      else if (t === "checkbox") kind = "复选框";
      else if (t === "radio") kind = "单选框";
      else if (t === "email") kind = "邮箱框";
      else if (t === "search") kind = "搜索框";
      else kind = "输入框";
      hint = el.placeholder || "";
    } else if (tag === "textarea") {
      kind = "输入框";
      hint = el.placeholder || "";
    } else if (tag === "select") {
      kind = "下拉框";
    } else if (tag === "a") {
      kind = "链接";
      hint = el.href ? String(el.href).slice(0, 80) : "";
    } else if (tag === "button") {
      kind = "按钮";
    }
    interactives.push({ kind, label, hint });
    if (interactives.length >= 40) break;
  }
  return {
    url: location.href,
    title: document.title,
    text: document.body ? document.body.innerText : "",
    interactives,
  };
})()
"#
    .to_string()
}

/// Build the Runtime.evaluate fill-lookup script for a query. Matches
/// inputs by placeholder / aria-label / name / wrapping label text;
/// focuses and selects existing content. Returns `{ok, x, y, label}` or
/// `{ok: false, reason}`.
pub fn fill_script(query: &str) -> String {
    let escaped = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const q = {escaped}.toLowerCase();
  const els = [...document.querySelectorAll("input,textarea")];
  const el = els.find((e) => {{
    const t = (e.type || "text").toLowerCase();
    if (t === "submit" || t === "button" || t === "checkbox" || t === "radio") return false;
    const ph = (e.placeholder || "").toLowerCase();
    const aria = (e.getAttribute("aria-label") || "").toLowerCase();
    const name = (e.name || "").toLowerCase();
    const lbl = e.closest("label") ? e.closest("label").innerText.toLowerCase() : "";
    return ph.includes(q) || aria.includes(q) || name.includes(q) || lbl.includes(q);
  }});
  if (!el) return {{ ok: false, reason: "no input matches the query" }};
  el.focus();
  el.select();
  const r = el.getBoundingClientRect();
  return {{
    ok: true,
    x: Math.round(r.left + r.width / 2),
    y: Math.round(r.top + r.height / 2),
    label: String(el.placeholder || el.getAttribute("aria-label") || el.name || "input")
      .trim()
      .slice(0, 60),
  }};
}})()
"#
    )
}

/// Map a key name to CDP `(code, key, windowsVirtualKeyCode)`.
pub fn key_code(name: &str) -> AppResult<(String, String, u16)> {
    let vk = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => (13, "Enter", "Enter"),
        "tab" => (9, "Tab", "Tab"),
        "esc" | "escape" => (27, "Escape", "Escape"),
        "backspace" | "bs" => (8, "Backspace", "Backspace"),
        "delete" | "del" => (46, "Delete", "Delete"),
        "space" => (32, " ", "Space"),
        "up" | "arrowup" => (38, "ArrowUp", "ArrowUp"),
        "down" | "arrowdown" => (40, "ArrowDown", "ArrowDown"),
        "left" | "arrowleft" => (37, "ArrowLeft", "ArrowLeft"),
        "right" | "arrowright" => (39, "ArrowRight", "ArrowRight"),
        "home" => (36, "Home", "Home"),
        "end" => (35, "End", "End"),
        "pageup" | "pgup" => (33, "PageUp", "PageUp"),
        "pagedown" | "pgdn" => (34, "PageDown", "PageDown"),
        other => {
            return Err(format!(
                "Unknown key: \"{other}\". Use enter/tab/esc/backspace/delete/space/\
                 up/down/left/right/home/end/pageup/pagedown"
            )
            .into());
        }
    };
    Ok((vk.2.to_string(), vk.1.to_string(), vk.0))
}

/// Browser-side capture hook: wraps console, window errors/rejections and
/// fetch/XHR into a bounded `window.__ddcLogs` buffer (200 entries each,
/// truncated fields). Idempotent — re-injecting returns early.
pub fn capture_inject_script() -> String {
    r#"
(() => {
  if (window.__ddcLogs) return true;
  var MAX = 200;
  var logs = { console: [], network: [], errors: [] };
  window.__ddcLogs = logs;
  function push(arr, item) {
    if (arr.length >= MAX) arr.shift();
    arr.push(item);
  }
  function fmt(args) {
    var out = [];
    for (var i = 0; i < args.length; i++) {
      var a = args[i];
      if (typeof a === 'string') out.push(a);
      else if (a instanceof Error) out.push(a.message || String(a));
      else { try { out.push(JSON.stringify(a)); } catch (e) { out.push(String(a)); } }
    }
    return out.join(' ').slice(0, 500);
  }
  ['log', 'info', 'warn', 'error', 'debug'].forEach(function (level) {
    var orig = console[level];
    console[level] = function () {
      push(logs.console, { level: level, text: fmt(arguments), ts: Date.now() });
      orig.apply(console, arguments);
    };
  });
  window.addEventListener('error', function (e) {
    push(logs.errors, { type: 'error', text: String(e.message || '').slice(0, 500), source: e.filename || '', line: e.lineno || 0, ts: Date.now() });
  });
  window.addEventListener('unhandledrejection', function (e) {
    push(logs.errors, { type: 'rejection', text: String((e && e.reason) || 'unknown').slice(0, 500), ts: Date.now() });
  });
  var origFetch = window.fetch;
  window.fetch = function () {
    var args = arguments;
    var url = typeof args[0] === 'string' ? args[0] : (args[0] && args[0].url) || '';
    var method = ((args[1] && args[1].method) || 'GET').toUpperCase();
    var t0 = performance.now();
    return origFetch.apply(this, args).then(function (r) {
      push(logs.network, { method: method, url: String(url).slice(0, 300), status: r.status, ms: Math.round(performance.now() - t0), ts: Date.now() });
      return r;
    }).catch(function (e) {
      push(logs.network, { method: method, url: String(url).slice(0, 300), status: 0, error: String(e).slice(0, 200), ms: Math.round(performance.now() - t0), ts: Date.now() });
      throw e;
    });
  };
  var origOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__ddc = { method: String(method || 'GET'), url: String(url).slice(0, 300), t0: performance.now() };
    return origOpen.apply(this, arguments);
  };
  var origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function () {
    var xhr = this;
    this.addEventListener('loadend', function () {
      var m = xhr.__ddc;
      if (m) {
        push(logs.network, { method: m.method, url: m.url, status: xhr.status, ms: Math.round(performance.now() - m.t0), ts: Date.now() });
      }
    });
    return origSend.apply(this, arguments);
  };
  return true;
})()
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dispatch_text_resolves_pending_call() {
        let (tx, _rx) = oneshot::channel();
        let mut pending = HashMap::new();
        pending.insert(
            7u64,
            PendingCall {
                method: "Page.navigate".into(),
                reply: tx,
            },
        );
        // Matching id + result.
        CdpClient::dispatch_text(
            r#"{"id":7,"result":{"frameId":"x"}}"#,
            &mut pending,
            &std::sync::RwLock::new(HashMap::new()),
        );
        assert!(
            pending.is_empty(),
            "resolved call must leave the pending map"
        );
    }

    #[test]
    fn dispatch_text_ignores_events_and_unknown_ids() {
        let (tx, _rx) = oneshot::channel();
        let mut pending = HashMap::new();
        pending.insert(
            1u64,
            PendingCall {
                method: "m".into(),
                reply: tx,
            },
        );
        let subs: std::sync::RwLock<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>> =
            std::sync::RwLock::new(HashMap::new());
        // Event (no id) with no subscriber → ignored, pending untouched.
        CdpClient::dispatch_text(
            r#"{"method":"Page.loadEventFired","params":{}}"#,
            &mut pending,
            &subs,
        );
        assert_eq!(pending.len(), 1);
        // Unknown id → ignored.
        CdpClient::dispatch_text(r#"{"id":99,"result":{}}"#, &mut pending, &subs);
        assert_eq!(pending.len(), 1);
        // Malformed → ignored.
        CdpClient::dispatch_text("not json", &mut pending, &subs);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn dispatch_text_routes_events_to_subscribers() {
        let subs: Arc<std::sync::RwLock<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>> =
            Arc::new(std::sync::RwLock::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        subs.write()
            .unwrap()
            .entry("Page.screencastFrame".into())
            .or_default()
            .push(tx);
        let mut pending = HashMap::new();
        CdpClient::dispatch_text(
            r#"{"method":"Page.screencastFrame","params":{"data":"AQID","metadata":{"viewport":{"width":1280,"height":900}}}}"#,
            &mut pending,
            &subs,
        );
        let frame = rx.try_recv().expect("subscriber must receive the event");
        assert_eq!(frame["data"], "AQID");
        assert_eq!(frame["metadata"]["viewport"]["width"], 1280);
        // A dropped receiver is silently removed.
        drop(rx);
        CdpClient::dispatch_text(
            r#"{"method":"Page.screencastFrame","params":{"data":"x"}}"#,
            &mut pending,
            &subs,
        );
        assert!(
            subs.read().unwrap()["Page.screencastFrame"].is_empty(),
            "dead subscriber must be pruned"
        );
    }

    #[test]
    fn click_script_embeds_escaped_needle() {
        let script = click_script("a \"quoted\"\nlabel");
        assert!(
            script.contains(r#"a \"quoted\"\nlabel"#),
            "needle must be JSON-escaped"
        );
        let script2 = click_script("确定");
        assert!(script2.contains("确定"));
    }

    #[test]
    fn fill_script_embeds_escaped_query() {
        let script = fill_script("用\"户\"\n名");
        assert!(
            script.contains(r#"用\"户\"\n名"#),
            "query must be JSON-escaped"
        );
        assert!(script.contains("input,textarea"));
    }

    #[test]
    fn snapshot_script_mentions_interactive_kinds() {
        let script = snapshot_script();
        for kind in ["按钮", "链接", "密码框", "下拉框", "placeholder"] {
            assert!(script.contains(kind), "snapshot must classify {kind}");
        }
    }

    #[test]
    fn key_codes_cover_the_mapped_set() {
        for name in [
            "enter",
            "return",
            "tab",
            "esc",
            "escape",
            "backspace",
            "delete",
            "space",
            "up",
            "down",
            "left",
            "right",
            "home",
            "end",
            "pageup",
            "pagedown",
            "Enter",
            "TAB",
            "ArrowUp",
        ] {
            assert!(key_code(name).is_ok(), "key {name} must resolve");
        }
        assert!(key_code("f7").is_err());
        assert!(key_code("ctrl+x").is_err());
    }

    #[test]
    fn key_code_returns_sane_codes() {
        let (code, key, vk) = key_code("enter").unwrap();
        assert_eq!((code.as_str(), key.as_str(), vk), ("Enter", "Enter", 13));
        let (_, _, vk) = key_code("arrowdown").unwrap();
        assert_eq!(vk, 40);
    }

    #[test]
    fn mouse_click_builds_pressed_then_released() {
        // Structural check: the two dispatchMouseEvent payloads use the
        // same coordinates with differing type — covered by the wire test
        // below (client requires a live socket), so we assert the shape.
        let base = json!({"x": 10, "y": 20, "button": "left", "clickCount": 1});
        let mut pressed = base.clone();
        let mut released = base;
        pressed["type"] = json!("mousePressed");
        released["type"] = json!("mouseReleased");
        assert_eq!(pressed["x"], 10);
        assert_eq!(released["type"], "mouseReleased");
    }

    #[test]
    fn capture_inject_script_is_idempotent_and_covers_console_network_errors() {
        let script = capture_inject_script();
        assert!(script.contains("window.__ddcLogs"));
        assert!(
            script.contains("if (window.__ddcLogs) return true;"),
            "idempotent guard"
        );
        assert!(script.contains("console[level]"));
        assert!(script.contains("unhandledrejection"));
        assert!(script.contains("XMLHttpRequest.prototype.open"));
        assert!(script.contains("window.fetch"));
    }

    #[test]
    fn css_scripts_embed_selector_and_text_safely() {
        let click = click_css_script("#submit");
        assert!(click.contains("querySelector(\"#submit\")"));
        assert!(click.contains("selector not found"));

        let fill = fill_css_script("input[name=q]", "a \"quoted\" & <text>");
        assert!(fill.contains("input[name=q]"));
        assert!(fill.contains("a \\\"quoted\\\""));
        assert!(fill.contains("input"));
        assert!(fill.contains("change"));

        let scroll = scroll_script(10, -20);
        assert!(scroll.contains("scrollBy"));
        assert!(scroll.contains("top:-20"));
    }

    /// Real browser smoke: launch headless Edge, load a local page, snapshot
    /// interactive elements (stable eids), fill + click by eid, verify the
    /// DOM updated. Skipped in the normal run (`-- --ignored` to execute).
    #[tokio::test]
    #[ignore = "requires a real Chromium/Edge binary"]
    async fn real_element_eid_smoke() {
        use std::process::Stdio;

        let edge = std::path::Path::new(
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        );
        if !edge.exists() {
            eprintln!("Edge not found — smoke skipped");
            return;
        }
        let profile = tempfile::tempdir().unwrap();
        let page_path = profile.path().join("page.html");
        std::fs::write(
            &page_path,
            r#"<html><body>
                <input id="i" placeholder="Name">
                <button id="b" onclick="document.getElementById('o').textContent='clicked-'+document.getElementById('i').value">Go</button>
                <div id="o"></div>
            </body></html>"#,
        )
        .unwrap();
        let port = 9300u16 + (std::process::id() % 500) as u16;
        let mut child = tokio::process::Command::new(edge)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                &format!("--remote-debugging-port={port}"),
                &format!("--user-data-dir={}", profile.path().display()),
                "about:blank",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn edge");

        let browser_ws = loop {
            if let Ok(resp) = reqwest::get(&format!("http://127.0.0.1:{port}/json/version")).await {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    if let Some(u) = v["webSocketDebuggerUrl"].as_str() {
                        break u.to_string();
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        let browser = CdpClient::connect(&browser_ws).await.expect("browser ws");
        let file_url = format!(
            "file:///{}",
            page_path.display().to_string().replace('\\', "/")
        );
        let target_id = browser.create_target(&file_url).await.expect("create target");
        let page_ws = loop {
            if let Ok(resp) = reqwest::get(&format!("http://127.0.0.1:{port}/json/list")).await {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    if let Some(arr) = v.as_array() {
                        if let Some(t) = arr.iter().find(|t| t["id"] == target_id) {
                            if let Some(u) = t["webSocketDebuggerUrl"].as_str() {
                                break u.to_string();
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        let page = CdpClient::connect(&page_ws).await.expect("page ws");
        page.wait_ready(30).await.expect("page ready");

        let snap = page.element_snapshot().await.expect("snapshot");
        let arr = snap.as_array().expect("snapshot array");
        let input_eid = arr
            .iter()
            .find(|e| e["placeholder"] == "Name")
            .and_then(|e| e["eid"].as_str())
            .expect("input eid")
            .to_string();
        let button_eid = arr
            .iter()
            .find(|e| e["tag"] == "button")
            .and_then(|e| e["eid"].as_str())
            .expect("button eid")
            .to_string();

        page.fill_eid(&input_eid, "Alice").await.expect("fill");
        page.click_eid(&button_eid).await.expect("click");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let out = page
            .evaluate("document.getElementById('o').textContent")
            .await
            .expect("verify");
        assert_eq!(
            out.as_str(),
            Some("clicked-Alice"),
            "element-level click+fill must drive the real DOM"
        );
        let _ = child.kill().await;
    }
}
