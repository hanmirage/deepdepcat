/**
 * McpAppView — renders an MCP Apps interactive HTML payload in a sandboxed
 * iframe (MCP Apps extension) with the bidirectional `ui/` JSON-RPC bridge
 * over postMessage (spec 2026-01-26).
 *
 * ## Host side (this component)
 * - `ui/initialize` handshake → `McpUiInitializeResult` (hostCapabilities:
 *   serverTools/serverResources proxying, hostContext: theme/locale/…).
 * - After `ui/notifications/initialized` → sends `ui/notifications/tool-input`
 *   (the tool arguments that produced this view) then
 *   `ui/notifications/tool-result` (the execution result).
 * - `tools/call` + `resources/read` from the app are proxied to the MCP
 *   server the view came from (via the `mcp_app_proxy` Tauri command).
 * - `ui/notifications/size-changed` resizes the frame (clamped).
 * - `ui/resource-teardown` and `ping` are answered.
 *
 * ## Security model
 * - iframe `sandbox` WITHOUT `allow-same-origin` — opaque origin, no host
 *   DOM/cookies/localStorage access.
 * - Only messages whose `event.source === frame.contentWindow` are accepted,
 *   and only JSON-RPC-shaped objects.
 * - A CSP `<meta>` is injected into the document: declared `_meta.ui.csp`
 *   domains are allowed, otherwise a restrictive default (no external
 *   network/scripts/frames).
 * - The proxy only accepts `tools/call` and `resources/read`, bound to the
 *   view's origin server.
 */

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { logDebug } from "@/lib/logger";
import { RefreshCw, RotateCw, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import { mcpApi } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";

export interface McpAppData {
  server: string;
  resource_uri: string;
  html: string;
  is_error: boolean;
  csp?: Record<string, unknown>;
}

interface McpAppViewProps {
  app: McpAppData;
  /** Raw JSON string of the tool call arguments that produced this view. */
  argumentsJson?: string;
  /** The tool execution result text (sent to the view as tool-result). */
  resultText?: string;
}

/** iframe sandbox flags: scripts + forms + modals + popups. Explicitly NO
 *  allow-same-origin — the app stays isolated in an opaque origin. */
const IFRAME_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups";

/** Protocol version we speak (MCP Apps spec 2026-01-26). */
export const UI_PROTOCOL_VERSION = "2026-01-26";

/** Max height a view can claim via size-changed (the tool card shows a
 *  compact inline view; fullscreen is a future display mode). */
const MAX_VIEW_HEIGHT = 640;
const MIN_VIEW_HEIGHT = 80;
const DEFAULT_VIEW_HEIGHT = 300;

/** Pure — build the CSP policy for the rendered document from the server's
 *  `_meta.ui.csp` declaration. No declaration → restrictive default. */
export function buildCspPolicy(csp?: Record<string, unknown>): string {
  const dom = (key: string): string[] => {
    const v = csp?.[key];
    return Array.isArray(v) ? v.filter((d): d is string => typeof d === "string") : [];
  };
  const connect = dom("connectDomains");
  const resources = dom("resourceDomains");
  const frames = dom("frameDomains");
  const bases = dom("baseUriDomains");

  const join = (list: string[], fallback: string): string =>
    list.length > 0 ? list.join(" ") : fallback;

  return [
    `default-src 'none'`,
    `script-src 'unsafe-inline' ${join(resources, "'none'")}`,
    `style-src 'unsafe-inline' ${join(resources, "'none'")}`,
    `img-src data: ${join(resources, "")}`,
    `font-src data: ${join(resources, "")}`,
    `connect-src ${join(connect, "'none'")}`,
    `frame-src ${join(frames, "'none'")}`,
    `base-uri ${join(bases, "'none'")}`,
    `object-src 'none'`,
    `form-action 'none'`,
  ].join("; ");
}

/** Pure — inject the CSP meta into the app's HTML (before the document's
 *  own meta tags so nothing external can load first). */
export function injectCspIntoHtml(html: string, csp?: Record<string, unknown>): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${buildCspPolicy(csp)}">`;
  const headMatch = /<head[^>]*>/i.exec(html);
  if (headMatch) {
    const at = headMatch.index + headMatch[0].length;
    return html.slice(0, at) + meta + html.slice(at);
  }
  // No <head> — prepend a minimal document head.
  return `<!DOCTYPE html><html><head>${meta}</head><body>` + html + "</body></html>";
}

/** Parsed inbound app message (validated shape). */
export type AppMessage =
  | { kind: "request"; id: number; method: string; params: Record<string, unknown> }
  | { kind: "notification"; method: string; params: Record<string, unknown> };

/** Pure — validate the shape of a message coming from the app iframe.
 *  Returns null for anything that is not a JSON-RPC message. */
export function parseAppMessage(raw: unknown): AppMessage | null {
  if (typeof raw !== "object" || raw === null) return null;
  const msg = raw as Record<string, unknown>;
  if (msg.jsonrpc !== "2.0" || typeof msg.method !== "string") return null;
  const params = (msg.params ?? {}) as Record<string, unknown>;
  if (typeof params !== "object" || params === null) return null;
  if (typeof msg.id === "number") {
    return { kind: "request", id: msg.id, method: msg.method, params };
  }
  return { kind: "notification", method: msg.method, params };
}

function McpAppViewImpl({ app, argumentsJson, resultText }: McpAppViewProps) {
  const { t } = useTranslation();
  const sessionId = useChatStore((s) => s.currentSessionId);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const [nonce, setNonce] = useState(0);
  const [height, setHeight] = useState(DEFAULT_VIEW_HEIGHT);
  const initializedRef = useRef(false);

  const reload = useCallback(() => {
    initializedRef.current = false;
    setHeight(DEFAULT_VIEW_HEIGHT);
    setNonce((n) => n + 1);
  }, []);

  const frameKey = useMemo(
    () => `${app.resource_uri}::${nonce}`,
    [app.resource_uri, nonce],
  );

  const srcdoc = useMemo(() => injectCspIntoHtml(app.html, app.csp), [app.html, app.csp]);

  /** Post a JSON-RPC message to the app. */
  const postToApp = useCallback((message: unknown) => {
    frameRef.current?.contentWindow?.postMessage(message, "*");
  }, []);

  /** Send a response to an inbound request id. */
  const respond = useCallback(
    (id: number, result: unknown) => {
      postToApp({ jsonrpc: "2.0", id, result });
    },
    [postToApp],
  );

  const respondError = useCallback(
    (id: number, code: number, message: string) => {
      postToApp({ jsonrpc: "2.0", id, error: { code, message } });
    },
    [postToApp],
  );

  /** After the app reports initialized, feed it the tool input + result
   *  (the view exists BECAUSE the tool already executed). */
  const sendToolNotifications = useCallback(() => {
    if (argumentsJson) {
      let parsed: Record<string, unknown> = {};
      try {
        const v = JSON.parse(argumentsJson);
        if (typeof v === "object" && v !== null) parsed = v;
      } catch {
        parsed = {};
      }
      postToApp({
        jsonrpc: "2.0",
        method: "ui/notifications/tool-input",
        params: { arguments: parsed },
      });
    }
    if (resultText !== undefined) {
      postToApp({
        jsonrpc: "2.0",
        method: "ui/notifications/tool-result",
        params: {
          content: [{ type: "text", text: resultText }],
          isError: app.is_error,
        },
      });
    }
  }, [argumentsJson, resultText, app.is_error, postToApp]);

  const handleMessage = useCallback(
    async (event: MessageEvent) => {
      // Only the view's own iframe may talk to us — everything else (the
      // webview page itself, other windows) is ignored.
      if (event.source !== frameRef.current?.contentWindow) return;

      const msg = parseAppMessage(event.data);
      if (!msg) return;

      if (msg.kind === "notification") {
        if (msg.method === "ui/notifications/initialized") {
          if (!initializedRef.current) {
            initializedRef.current = true;
            sendToolNotifications();
          }
        } else if (msg.method === "ui/notifications/size-changed") {
          const h = msg.params.height;
          if (typeof h === "number" && Number.isFinite(h)) {
            setHeight(Math.min(MAX_VIEW_HEIGHT, Math.max(MIN_VIEW_HEIGHT, Math.round(h))));
          }
        } else if (msg.method === "notifications/message") {
          logDebug("MCP app", app.server, msg.params);
          // Forward the app's log/console output to the backend replay-exact
          // event log (debug aid). Best-effort, never breaks the bridge.
          try {
            void mcpApi
              .logApp(
                app.server,
                String((msg.params as { level?: unknown } | undefined)?.level ?? "info"),
                String((msg.params as { message?: unknown } | undefined)?.message ?? ""),
                sessionId ?? undefined,
              )
              .catch(() => {});
          } catch {
            // Ignore — logging is a byproduct.
          }
        }
        return;
      }

      switch (msg.method) {
        case "ui/initialize": {
          const dark = document.documentElement.classList.contains("dark");
          respond(msg.id, {
            protocolVersion: UI_PROTOCOL_VERSION,
            hostCapabilities: {
              serverTools: {},
              serverResources: {},
              logging: {},
            },
            hostInfo: { name: "DeepDepCat", version: "1.0.0" },
            hostContext: {
              theme: dark ? "dark" : "light",
              displayMode: "inline",
              platform: "desktop",
              locale: navigator.language,
              timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
            },
          });
          break;
        }
        case "ping": {
          respond(msg.id, {});
          break;
        }
        case "ui/resource-teardown": {
          respond(msg.id, {});
          break;
        }
        case "tools/call":
        case "resources/read": {
          try {
            const result = await mcpApi.proxyAppRequest(app.server, msg.method, msg.params);
            respond(msg.id, result);
          } catch (e) {
            respondError(
              msg.id,
              -32603,
              e instanceof Error ? e.message : "MCP Apps proxy failed",
            );
          }
          break;
        }
        default: {
          // Spec: hosts MAY ignore unknown methods.
          if (typeof msg.id === "number") respondError(msg.id, -32601, `Method not found: ${msg.method}`);
        }
      }
    },
    [app.server, respond, respondError, sendToolNotifications],
  );

  useEffect(() => {
    window.addEventListener("message", handleMessage);
    return () => window.removeEventListener("message", handleMessage);
  }, [handleMessage]);

  return (
    <div className="overflow-hidden rounded-md border border-border/70 bg-background">
      {/* Header: server · uri + reload */}
      <div className="flex items-center gap-2 border-b border-border/60 bg-muted/30 px-2 py-1">
        <Server className="h-3 w-3 shrink-0 text-muted-foreground/60" />
        <span className="shrink-0 rounded bg-primary/10 px-1.5 py-0.5 font-mono text-[10px] text-primary">
          {app.server}
        </span>
        <span
          className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground/60"
          title={app.resource_uri}
        >
          {app.resource_uri}
        </span>
        <button
          onClick={reload}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
          title={t("chat.mcpAppReload", { defaultValue: "重新加载" })}
        >
          <RefreshCw className="h-3 w-3" />
        </button>
      </div>

      {/* The sandboxed app itself */}
      <iframe
        key={frameKey}
        ref={frameRef}
        sandbox={IFRAME_SANDBOX}
        srcDoc={srcdoc}
        title={`${app.server} app`}
        className="block w-full bg-white transition-[height] duration-150"
        style={{ height }}
        // srcdoc only — never follow links or load remote frames.
        // (allow-popups-to-escape-sandbox intentionally omitted.)
      />
    </div>
  );
}

/** @internal export for tests */
export const McpAppView = memo(McpAppViewImpl);

/** @internal export for tests — mirrors the iframe sandbox contract. */
export const MCP_APP_SANDBOX = IFRAME_SANDBOX;

/** Render an inline placeholder when a server attached a UI but it cannot
 *  be shown (e.g. oversized HTML was dropped server-side). */
export function McpAppErrorHint() {
  const { t } = useTranslation();
  return (
    <p className="flex items-center gap-1.5 text-[11px] text-muted-foreground/70">
      <RotateCw className="h-3 w-3" />
      {t("chat.mcpAppUnavailable", { defaultValue: "MCP 应用界面不可用" })}
    </p>
  );
}
