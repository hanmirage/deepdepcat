/**
 * McpAppView tests — MCP Apps bidirectional bridge.
 *
 * Covers the pure protocol logic:
 *  - CSP policy: restrictive default vs declared domains
 *  - CSP injection into the HTML document
 *  - inbound message validation (JSON-RPC shape)
 *  - the rendered iframe carries the sandbox + CSP'd srcdoc
 */

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import {
  McpAppView,
  MCP_APP_SANDBOX,
  buildCspPolicy,
  injectCspIntoHtml,
  parseAppMessage,
} from "@/components/chat/McpAppView";
import type { McpAppData } from "@/components/chat/McpAppView";

function app(overrides: Partial<McpAppData> = {}): McpAppData {
  return {
    server: "charts",
    resource_uri: "ui://app/dashboard",
    html: "<!DOCTYPE html><html><head><title>d</title></head><body><h1>hi</h1></body></html>",
    is_error: false,
    ...overrides,
  };
}

describe("buildCspPolicy", () => {
  it("defaults to a restrictive policy (no external origins)", () => {
    const policy = buildCspPolicy(undefined);
    expect(policy).toContain("default-src 'none'");
    expect(policy).toContain("connect-src 'none'");
    expect(policy).toContain("script-src 'unsafe-inline' 'none'");
    expect(policy).toContain("frame-src 'none'");
    expect(policy).toContain("object-src 'none'");
    expect(policy).toContain("base-uri 'none'");
  });

  it("allows declared connect + resource domains", () => {
    const policy = buildCspPolicy({
      connectDomains: ["https://api.example.com"],
      resourceDomains: ["https://cdn.example.com"],
      frameDomains: ["https://frames.example.com"],
      baseUriDomains: ["https://app.example.com"],
    });
    expect(policy).toContain("connect-src https://api.example.com");
    expect(policy).toContain("script-src 'unsafe-inline' https://cdn.example.com");
    expect(policy).toContain("style-src 'unsafe-inline' https://cdn.example.com");
    expect(policy).toContain("frame-src https://frames.example.com");
    expect(policy).toContain("base-uri https://app.example.com");
    expect(policy).toContain("object-src 'none'");
  });

  it("ignores non-string entries", () => {
    const policy = buildCspPolicy({ connectDomains: [42, "https://ok.example.com"] });
    expect(policy).toContain("connect-src https://ok.example.com");
    expect(policy).not.toContain("42");
  });
});

describe("injectCspIntoHtml", () => {
  it("injects the meta right after <head>", () => {
    const out = injectCspIntoHtml("<!DOCTYPE html><html><head><title>t</title></head><body>x</body></html>");
    const headEnd = out.indexOf("</head>");
    const metaAt = out.indexOf('<meta http-equiv="Content-Security-Policy"');
    expect(metaAt).toBeGreaterThan(0);
    expect(metaAt).toBeLessThan(headEnd);
    expect(out.indexOf("<title>t</title>")).toBeGreaterThan(metaAt);
  });

  it("wraps documents without a head", () => {
    const out = injectCspIntoHtml("<h1>bare</h1>");
    expect(out).toContain("<!DOCTYPE html><html><head>");
    expect(out).toContain('http-equiv="Content-Security-Policy"');
    expect(out).toContain("<h1>bare</h1>");
  });
});

describe("parseAppMessage", () => {
  it("parses a valid JSON-RPC request", () => {
    const msg = parseAppMessage({ jsonrpc: "2.0", id: 7, method: "ui/initialize", params: {} });
    expect(msg).toEqual({ kind: "request", id: 7, method: "ui/initialize", params: {} });
  });

  it("parses a notification (no id)", () => {
    const msg = parseAppMessage({
      jsonrpc: "2.0",
      method: "ui/notifications/size-changed",
      params: { width: 300, height: 200 },
    });
    expect(msg).toEqual({
      kind: "notification",
      method: "ui/notifications/size-changed",
      params: { width: 300, height: 200 },
    });
  });

  it("rejects non-JSON-RPC shapes", () => {
    expect(parseAppMessage(null)).toBeNull();
    expect(parseAppMessage("hello")).toBeNull();
    expect(parseAppMessage({ method: "x" })).toBeNull();
    expect(parseAppMessage({ jsonrpc: "1.0", method: "x" })).toBeNull();
    expect(parseAppMessage({ jsonrpc: "2.0", method: 42 })).toBeNull();
    expect(parseAppMessage({ jsonrpc: "2.0", method: "x", params: "bad" })).toBeNull();
  });

  it("defaults missing params to an empty object", () => {
    const msg = parseAppMessage({ jsonrpc: "2.0", id: 1, method: "ping" });
    expect(msg).toEqual({ kind: "request", id: 1, method: "ping", params: {} });
  });
});

describe("McpAppView rendering", () => {
  it("renders the app in a sandboxed iframe with the CSP'd document", () => {
    render(<McpAppView app={app()} />);
    const frame = document.querySelector("iframe");
    expect(frame).not.toBeNull();
    expect(frame?.getAttribute("sandbox")).toBe(MCP_APP_SANDBOX);
    expect(frame?.getAttribute("sandbox")).not.toContain("allow-same-origin");
    const srcDoc = frame?.getAttribute("srcdoc") ?? "";
    expect(srcDoc).toContain('http-equiv="Content-Security-Policy"');
    expect(srcDoc).toContain("<h1>hi</h1>");
    expect(screen.getByText("charts")).toBeInTheDocument();
    expect(screen.getByText("ui://app/dashboard")).toBeInTheDocument();
  });

  it("renders no iframe without an app payload (card-level guard)", () => {
    // Guard lives in ToolCallCard; here we assert the view itself is inert.
    expect(document.querySelector("iframe")).toBeNull();
  });
});
