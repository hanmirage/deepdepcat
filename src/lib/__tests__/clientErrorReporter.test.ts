import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const getEnabledMock = vi.fn();
const getDefaultServerUrlMock = vi.fn();
const getSystemInfoMock = vi.fn();
const invokeMock = vi.fn();

vi.mock("@/lib/tauri/api/identity", () => ({
  diagnosticsApi: { getEnabled: () => getEnabledMock() },
  deviceAuthApi: { getDefaultServerUrl: () => getDefaultServerUrlMock() },
}));

vi.mock("@/lib/tauri/api/session", () => ({
  systemApi: { getSystemInfo: () => getSystemInfoMock() },
}));

// The reporter routes through a Rust command (reqwest, no CORS) instead of a
// browser fetch — mock invoke and force the Tauri path.
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock("@/lib/tauri/core", () => ({ isTauri: true }));

import {
  initClientErrorReporter,
  reportClientError,
  resetClientErrorReporterForTest,
  setClientErrorReporting,
} from "@/lib/clientErrorReporter";

/** The payload passed as the second arg of an invoke("submit_client_error", …). */
function sentPayload(i: number): Record<string, unknown> {
  return (invokeMock.mock.calls[i][1] as { payload: Record<string, unknown> }).payload;
}

describe("clientErrorReporter", () => {
  beforeEach(() => {
    resetClientErrorReporterForTest();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    getEnabledMock.mockReset().mockResolvedValue(true);
    getDefaultServerUrlMock.mockReset().mockResolvedValue("https://deepdepcat.hsmiai.xyz");
    getSystemInfoMock.mockReset().mockResolvedValue({ app_version: "1.1.7" });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("respects the diagnostics opt-out", async () => {
    setClientErrorReporting(false);
    reportClientError("ui_render", new Error("boom"));
    await new Promise((r) => setTimeout(r, 0));
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("posts a compact payload through the Rust telemetry command", async () => {
    reportClientError("ui_render", new Error("boom"));
    await new Promise((r) => setTimeout(r, 0));
    expect(invokeMock).toHaveBeenCalledTimes(1);
    const [command, args] = invokeMock.mock.calls[0];
    expect(command).toBe("submit_client_error");
    const payload = (args as { payload: Record<string, unknown> }).payload;
    expect(payload.event_type).toBe("client_error");
    expect(payload.event_name).toBe("ui_render");
    const data = payload.data as Record<string, unknown>;
    expect(data.message).toBe("boom");
    expect(data.stack).toBeUndefined();
  });

  it("deduplicates identical errors within the cooldown window", async () => {
    reportClientError("ui_render", new Error("same"));
    reportClientError("ui_render", new Error("same"));
    reportClientError("other", new Error("same"));
    await new Promise((r) => setTimeout(r, 0));
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("installs global handlers that report window errors and rejections", async () => {
    initClientErrorReporter();
    await new Promise((r) => setTimeout(r, 0));

    window.dispatchEvent(new ErrorEvent("error", { message: "global boom", filename: "app.ts", lineno: 3, colno: 7 }));
    const rejection = new Event("unhandledrejection") as PromiseRejectionEvent;
    Object.defineProperty(rejection, "reason", { value: new Error("rejected") });
    window.dispatchEvent(rejection);
    await new Promise((r) => setTimeout(r, 0));

    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(sentPayload(0).event_name).toBe("window_error");
    const d0 = sentPayload(0).data as Record<string, unknown>;
    expect(d0.source).toBe("app.ts");
    expect(d0.line).toBe(3);
    expect(sentPayload(1).event_name).toBe("unhandled_rejection");
    expect((sentPayload(1).data as Record<string, unknown>).message).toBe("rejected");
  });
});
