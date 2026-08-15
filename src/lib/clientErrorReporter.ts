/**
 * Client-side error reporter — lightweight, privacy-first cloud reporting
 * for NON-fatal desktop failures (render errors, unhandled rejections).
 *
 * Data model: reuses the existing anonymous telemetry endpoint
 * `POST {serverUrl}/api/v1/telemetry/collect` with
 * `event_type = "client_error"`. Payloads never include conversation
 * content, file contents, or user input — only a compact message, an
 * optional stack fragment, and page/version metadata.
 *
 * Privacy: respects the Settings → Privacy diagnostics toggle (default on).
 * When off, nothing is recorded and no network request is made.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri/core";
import { deviceAuthApi, diagnosticsApi } from "@/lib/tauri/api/identity";
import { systemApi } from "@/lib/tauri/api/session";

const DEFAULT_SERVER_URL = "https://deepdepcat.hsmiai.xyz";
const MAX_MESSAGE = 500;
const MAX_STACK = 1000;
const MAX_SOURCE = 200;
const COOLDOWN_MS = 10 * 60 * 1000;

let enabled = true;
let serverUrl = DEFAULT_SERVER_URL;
let appVersion = "";
const lastSent = new Map<string, number>();

/** Turn reporting on/off (mirrors the Settings → Privacy toggle). */
export function setClientErrorReporting(on: boolean): void {
  enabled = on;
}

/** Normalize an unknown thrown value into a compact message. */
function errorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message || err.name || "Unknown error";
  if (err && typeof err === "object") {
    const msg = (err as { message?: unknown }).message;
    if (typeof msg === "string" && msg) return msg;
  }
  return String(err ?? "Unknown error");
}

interface ReportExtra {
  stack?: string;
  source?: string;
  line?: number;
  col?: number;
}

/**
 * Report one client error. Best-effort and deduplicated per
 * (context, message) with a 10-minute cooldown; a failure to send is
 * swallowed so reporting can never break the app.
 */
export function reportClientError(context: string, err: unknown, extra?: ReportExtra): void {
  if (!enabled) return;
  const message = errorMessage(err).slice(0, MAX_MESSAGE);
  const key = `${context}:${message}`;
  const now = Date.now();
  const last = lastSent.get(key) ?? 0;
  if (now - last < COOLDOWN_MS) return;
  lastSent.set(key, now);

  const payload = {
    session_id: "client_error",
    event_type: "client_error",
    span: "",
    event_name: context,
    data: {
      message,
      source: extra?.source?.slice(0, MAX_SOURCE) || undefined,
      line: extra?.line,
      col: extra?.col,
      stack: extra?.stack?.slice(0, MAX_STACK) || undefined,
      url: typeof window !== "undefined" ? window.location.pathname : undefined,
      app_version: appVersion || undefined,
    },
  };
  void send(payload);
}

async function send(payload: unknown): Promise<void> {
  try {
    // The telemetry server sends no CORS headers, so a browser `fetch` from
    // the webview (origin tauri://localhost) is always blocked — route the
    // POST through Rust (reqwest, native HTTP) instead. Browser dev mode has
    // no backend to invoke, so skip silently (the fetch would just CORS-fail).
    if (!isTauri) return;
    await invoke("submit_client_error", { serverUrl, payload });
  } catch {
    // best-effort — never surface reporter failures
  }
}

/**
 * Install global handlers and resolve the privacy toggle / server URL /
 * app version once at startup. Safe to call multiple times.
 */
export function initClientErrorReporter(): void {
  void diagnosticsApi
    .getEnabled()
    .then(setClientErrorReporting)
    .catch(() => {});
  void deviceAuthApi
    .getDefaultServerUrl()
    .then((url) => {
      if (url) serverUrl = url;
    })
    .catch(() => {});
  void systemApi
    .getSystemInfo()
    .then((info) => {
      appVersion = info.app_version ?? "";
    })
    .catch(() => {});

  window.addEventListener("error", (event: ErrorEvent) => {
    reportClientError("window_error", event.message || event.error, {
      source: event.filename || undefined,
      line: event.lineno || undefined,
      col: event.colno || undefined,
    });
  });
  window.addEventListener("unhandledrejection", (event: PromiseRejectionEvent) => {
    reportClientError("unhandled_rejection", event.reason);
  });
}

/** Test-only reset — clears state so tests start from a known baseline. */
export function resetClientErrorReporterForTest(): void {
  enabled = true;
  serverUrl = DEFAULT_SERVER_URL;
  appVersion = "";
  lastSent.clear();
}
