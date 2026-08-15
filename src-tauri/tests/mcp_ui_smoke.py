"""Minimal stdio MCP server for the DeepDepCat MCP Apps smoke test.

Serves one tool `make_dashboard` whose result carries a `ui://` resource
(interactive HTML) — exactly the shape the MCP Apps extension specifies.
Hand-rolled JSON-RPC over stdio, no third-party deps.
"""

import json
import sys

UI_HTML = """<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Dashboard</title></head>
<body><h1>Smoke Dashboard</h1><p id="status">deepdepcat-ui-ok</p></body></html>"""


def respond(msg_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg_id, "result": result}) + "\n")
    sys.stdout.flush()


def handle(method, params, msg_id):
    if method == "initialize":
        respond(msg_id, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "ui-smoke", "version": "1.0.0"},
        })
        return
    if method == "notifications/initialized" or method == "ping":
        respond(msg_id, {})
        return
    if method == "tools/list":
        respond(msg_id, {
            "tools": [{
                "name": "make_dashboard",
                "description": "Render an interactive dashboard.",
                "inputSchema": {"type": "object", "properties": {}, "required": []},
                "_meta": {"ui": {"resourceUri": "ui://app/dashboard"}},
            }]
        })
        return
    if method == "resources/list":
        respond(msg_id, {
            "resources": [{
                "uri": "ui://app/dashboard",
                "name": "Dashboard UI",
                "mimeType": "text/html",
            }]
        })
        return
    if method == "prompts/list":
        respond(msg_id, {"prompts": []})
        return
    if method == "tools/call":
        respond(msg_id, {
            "content": [
                {"type": "text", "text": "dashboard rendered"},
                {"type": "resource", "resource": {"uri": "ui://app/dashboard", "mimeType": "text/html"}},
            ],
            "isError": False,
        })
        return
    if method == "resources/read":
        uri = params.get("uri") if params else None
        if uri == "ui://app/dashboard":
            respond(msg_id, {"contents": [{"uri": uri, "mimeType": "text/html", "text": UI_HTML}]})
        else:
            respond(msg_id, {"contents": []})
        return
    respond(msg_id, {})


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "id" not in msg:
            continue  # notification
        handle(msg.get("method", ""), msg.get("params"), msg["id"])


if __name__ == "__main__":
    main()
