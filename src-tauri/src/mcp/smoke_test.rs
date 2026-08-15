//! MCP Apps smoke test — full real-transport round trip against a live
//! stdio MCP server (`tests/mcp_ui_smoke.py`).
//!
//! Verifies the production path end to end: stdio spawn + initialize
//! handshake → `tools/list` (with `_meta.ui`) → `tools/call` returning a
//! `ui://` resource block → `resources/read` fetching the HTML → the
//! `McpAppPayload` carried out of `call_tool_detailed`.
//!
//! Requires a local Python 3 interpreter; skipped (passes with a note) when
//! unavailable so CI stays green on machines without Python.

use crate::mcp::client::McpClient;
use std::collections::HashMap;

fn python_path() -> Option<String> {
    for candidate in ["python", "python3"] {
        if std::process::Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }
    None
}

#[tokio::test]
async fn ui_resource_round_trip_over_stdio() {
    let Some(python) = python_path() else {
        eprintln!("skipping MCP Apps smoke test — no Python interpreter found");
        return;
    };

    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mcp_ui_smoke.py");
    let args: Vec<String> = vec![script.to_string()];

    let client = McpClient::new_stdio("ui-smoke", &python, &args, &HashMap::new())
        .await
        .expect("stdio MCP server connects and initializes");

    // tools/call → ui:// resource block → resources/read → HTML payload.
    let outcome = client
        .call_tool_detailed("make_dashboard", serde_json::json!({}))
        .await
        .expect("tool call succeeds");

    assert_eq!(outcome.content, "dashboard rendered");
    assert!(!outcome.is_error);
    let app = outcome.app.expect("UI payload surfaced");
    assert_eq!(app.resource_uri, "ui://app/dashboard");
    assert!(app.html.contains("deepdepcat-ui-ok"));
    assert!(app.html.contains("<h1>Smoke Dashboard</h1>"));

    client.close().await.expect("client closes cleanly");
}
