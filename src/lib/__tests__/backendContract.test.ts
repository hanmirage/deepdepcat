/**
 * Frontend ↔ backend API contract tests.
 *
 * Mocks the Tauri IPC bridge (`invoke`) and verifies every frontend API
 * method calls the correct command name with the correct arguments, and
 * parses the backend response. This catches the whole class of "frontend
 * calls the wrong command / wrong args" bugs (e.g. the synthetic model-id
 * bug that made every DeepSeek request 400).
 *
 * The command names asserted here must match `invoke_handler` in
 * `src-tauri/src/lib.rs` and the `#[tauri::command]` functions.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";

// ── Mock the Tauri IPC layer BEFORE importing the api module ──
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
  convertFileSrc: (p: string) => p,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
    isMaximized: vi.fn().mockResolvedValue(false),
    onResized: vi.fn(),
  }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-fs", () => ({
  readDir: vi.fn(),
  readTextFile: vi.fn(),
}));

// Must be imported AFTER the mocks are registered.
import * as tauri from "@/lib/tauri";

function expectInvoke(command: string, args?: Record<string, unknown>) {
  expect(invokeMock).toHaveBeenCalledTimes(1);
  const [cmd, payload] = invokeMock.mock.calls[0];
  expect(cmd).toBe(command);
  if (args) {
    expect(payload).toEqual(args);
  }
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue({});
});

describe("systemApi contract", () => {
  it("getSystemInfo → get_system_info", async () => {
    invokeMock.mockResolvedValue({ os: "windows", arch: "x86_64", cpu_count: 8, total_memory_mb: 16384, app_version: "0.1.0" });
    const r = await tauri.systemApi.getSystemInfo();
    expectInvoke("get_system_info");
    expect(r.os).toBe("windows");
  });

  it("getAgentStatus → get_agent_status (string encoding)", async () => {
    invokeMock.mockResolvedValue("thinking");
    const r = await tauri.systemApi.getAgentStatus();
    expectInvoke("get_agent_status");
    expect(r).toBe("thinking");
  });

  it("setAgentStatus → set_agent_status with string status", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.systemApi.setAgentStatus("tool_running");
    expectInvoke("set_agent_status", { status: "tool_running" });
  });

  it("setDebugMode → set_debug_mode", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.systemApi.setDebugMode(true);
    expectInvoke("set_debug_mode", { enabled: true });
  });

  it("cancelOperation → cancel_operation", async () => {
    invokeMock.mockResolvedValue(true);
    await tauri.systemApi.cancelOperation("s1");
    expectInvoke("cancel_operation", { sessionId: "s1" });
  });
});

describe("chatApi contract", () => {
  it("sendMessage → send_chat_message with session/message/mode/workMode", async () => {
      invokeMock.mockResolvedValue("turn-1");
    const r = await tauri.chatApi.sendMessage("s1", "hello", "plan_execute", "depwork", undefined, "high", "代码评审");
      expectInvoke("send_chat_message", {
        sessionId: "s1",
        message: "hello",
        mode: "plan_execute",
        workMode: "depwork",
        contextChips: undefined,
        reasoningMode: "high",
        agentName: "代码评审",
      });
      expect(r).toBe("turn-1");
    });

  it("sendMessage passes workMode through when omitted", async () => {
    invokeMock.mockResolvedValue("turn-1");
    await tauri.chatApi.sendMessage("s1", "hello");
    expectInvoke("send_chat_message", {
      sessionId: "s1",
      message: "hello",
      mode: undefined,
        workMode: undefined,
        contextChips: undefined,
        reasoningMode: undefined,
        agentName: null,
      });
  });
});

describe("sessionApi contract", () => {
  it("createSession → create_session with model+provider", async () => {
    invokeMock.mockResolvedValue({ id: "s1", model: "deepseek-v4-flash", provider: "deepseek", title: "New Session", created_at: "", updated_at: "", status: "active", turn_count: 0, total_usage: null, workspace_path: null, system_prompt: "", is_streaming: false });
    const r = await tauri.sessionApi.createSession("deepseek-v4-flash", "deepseek");
    expectInvoke("create_session", { model: "deepseek-v4-flash", provider: "deepseek", workspacePath: undefined, workMode: undefined });
    expect(r.id).toBe("s1");
  });

  it("createSession(workMode) → create_session with depwork mode", async () => {
    invokeMock.mockResolvedValue({ id: "s2", model: "m", provider: "p", title: "New Session", created_at: "", updated_at: "", status: "active", turn_count: 0, total_usage: null, workspace_path: null, system_prompt: "", work_mode: "depwork", is_streaming: false });
    const r = await tauri.sessionApi.createSession("m", "p", undefined, "depwork");
    expectInvoke("create_session", { model: "m", provider: "p", workspacePath: undefined, workMode: "depwork" });
    expect(r.work_mode).toBe("depwork");
  });

  it("listSessions → list_sessions", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.sessionApi.listSessions(50);
    expectInvoke("list_sessions", { limit: 50 });
  });

  it("getSessionMessages → get_session_messages", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.sessionApi.getSessionMessages("s1");
    expectInvoke("get_session_messages", { sessionId: "s1" });
  });

  it("updateSessionTitle → update_session_title", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.sessionApi.updateSessionTitle("s1", "New Title");
    expectInvoke("update_session_title", { sessionId: "s1", title: "New Title" });
  });

  it("updateSessionModel → update_session_model", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.sessionApi.updateSessionModel("s1", "deepseek-v4-pro");
    expectInvoke("update_session_model", { sessionId: "s1", model: "deepseek-v4-pro" });
  });

  it("deleteMessage → delete_message", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.sessionApi.deleteMessage("s1", "text");
    expectInvoke("delete_message", { sessionId: "s1", userContent: "text" });
  });

  it("getSessionUsage → get_session_usage", async () => {
    invokeMock.mockResolvedValue({ session_id: "s1", total_prompt_tokens: 100, total_completion_tokens: 50, total_cached_read_tokens: 0, total_reasoning_tokens: 0, total_tool_calls: 1, total_tool_result_tokens: 0, turn_count: 1, context_window: 1000000, current_context_tokens: 100 });
    const r = await tauri.sessionApi.getSessionUsage("s1");
    expectInvoke("get_session_usage", { sessionId: "s1" });
    expect(r.current_context_tokens).toBe(100);
  });

  it("getGlobalUsage → get_global_usage", async () => {
    invokeMock.mockResolvedValue({ prompt_tokens: 1, completion_tokens: 1, cached_read_tokens: 0, reasoning_tokens: 0, cache_hit_tokens: 0, cache_miss_tokens: 0, tool_calls: 0, tool_result_tokens: 0, turns: 1 });
    const r = await tauri.sessionApi.getGlobalUsage();
    expectInvoke("get_global_usage");
    expect(r.turns).toBe(1);
  });

  it("setGoal → set_session_goal", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.sessionApi.setGoal("s1", "goal");
    expectInvoke("set_session_goal", { sessionId: "s1", goal: "goal" });
  });
});

describe("toolApi / agentApi contract", () => {
  it("agentApi.listActiveWorkers → list_active_workers", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.agentApi.listActiveWorkers();
    expectInvoke("list_active_workers");
  });

  it("runningSessionsApi.list → list_running_sessions", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.runningSessionsApi.list();
    expectInvoke("list_running_sessions");
  });
});

describe("hookApi / skillsApi contract", () => {
  it("hookApi.list → list_hooks", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.hookApi.list();
    expectInvoke("list_hooks");
  });

  it("skillsApi.list → list_skills", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.skillsApi.list();
    expectInvoke("list_skills");
  });

  it("skillsApi.list(workMode) → list_skills with mode filter", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.skillsApi.list("depwork");
    expect(invokeMock).toHaveBeenCalledWith("list_skills", { workMode: "depwork" });
  });
});

describe("mcpApi contract", () => {
  it("listServers → list_mcp_servers", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.mcpApi.listServers();
    expectInvoke("list_mcp_servers");
  });

  it("listCredentials → list_mcp_credentials", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.mcpApi.listCredentials();
    expectInvoke("list_mcp_credentials");
  });

  it("saveCredential → save_mcp_credential with renewal fields", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.mcpApi.saveCredential(
      "srv",
      "https://srv.example",
      "tok",
      "Bearer",
      "2026-09-01T00:00:00Z",
      "refresh",
      "https://srv.example/oauth/token",
      "client-1",
    );
    expectInvoke("save_mcp_credential", {
      serverName: "srv",
      serverUrl: "https://srv.example",
      accessToken: "tok",
      tokenType: "Bearer",
      expiresAt: "2026-09-01T00:00:00Z",
      refreshToken: "refresh",
      tokenEndpoint: "https://srv.example/oauth/token",
      clientId: "client-1",
    });
  });

  it("deleteCredential → delete_mcp_credential", async () => {
    invokeMock.mockResolvedValue(true);
    const removed = await tauri.mcpApi.deleteCredential("srv", "https://srv.example");
    expectInvoke("delete_mcp_credential", {
      serverName: "srv",
      serverUrl: "https://srv.example",
    });
    expect(removed).toBe(true);
  });

  it("logApp → mcp_app_log", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.mcpApi.logApp("srv", "warn", "boom", "s1");
    expectInvoke("mcp_app_log", {
      server: "srv",
      level: "warn",
      message: "boom",
      sessionId: "s1",
    });
  });

  it("getTools → get_mcp_tools", async () => {
    invokeMock.mockResolvedValue([]);
    await tauri.mcpApi.getTools("server-1");
    expectInvoke("get_mcp_tools", { serverName: "server-1" });
  });
});

describe("configApi contract", () => {
  it("getConfig → get_config", async () => {
    invokeMock.mockResolvedValue({ llm: {} });
    const r = await tauri.configApi.getConfig();
    expectInvoke("get_config");
    expect(r.llm).toBeDefined();
  });
});

describe("crashApi / diagnosticsApi contract", () => {
  it("getPending → get_pending_crash", async () => {
    invokeMock.mockResolvedValue(null);
    const r = await tauri.crashApi.getPending();
    expectInvoke("get_pending_crash");
    expect(r).toBeNull();
  });

  it("getEnabled → get_diagnostics_enabled", async () => {
    invokeMock.mockResolvedValue(true);
    const r = await tauri.diagnosticsApi.getEnabled();
    expectInvoke("get_diagnostics_enabled");
    expect(r).toBe(true);
  });
});

describe("askUserApi / elicitationApi contract", () => {
  it("askUserApi.respond → respond_to_user_input", async () => {
    invokeMock.mockResolvedValue(true);
    await tauri.askUserApi.respond("req-1", "answer");
    expectInvoke("respond_to_user_input", { requestId: "req-1", response: "answer" });
  });

  it("elicitationApi.respond → respond_elicitation", async () => {
    invokeMock.mockResolvedValue(true);
    await tauri.elicitationApi.respond("elic-1", "accept", { value: 1 });
    expectInvoke("respond_elicitation", { elicitationId: "elic-1", action: "accept", content: { value: 1 } });
  });
});

describe("workspaceFileApi contract", () => {
  it("open → open_workspace_file with reveal=false", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.workspaceFileApi.open("C:\\ws\\report.docx");
    expectInvoke("open_workspace_file", { path: "C:\\ws\\report.docx", reveal: false });
  });

  it("reveal → open_workspace_file with reveal=true", async () => {
    invokeMock.mockResolvedValue(undefined);
    await tauri.workspaceFileApi.reveal("C:\\ws\\report.docx");
    expectInvoke("open_workspace_file", { path: "C:\\ws\\report.docx", reveal: true });
  });
});
