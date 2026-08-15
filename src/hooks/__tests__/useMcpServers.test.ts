import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act, waitFor } from "@testing-library/react";
import { useMcpServers } from "@/hooks/useMcpServers";

const { capturedHandler } = vi.hoisted(() => ({
  capturedHandler: { current: null as ((payload: unknown) => void) | null },
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: true,
    onEvent: vi.fn(async (_name: string, handler: (payload: unknown) => void) => {
      capturedHandler.current = handler;
      return () => {};
    }),
    mcpApi: {
      listServers: vi.fn(async () => [
        {
          name: "srv",
          type: "stdio",
          command: "npx",
          args: [],
          env: {},
          url: null,
          enabled: true,
        },
      ]),
      listConnected: vi.fn(async () => []),
      listCredentials: vi.fn(async () => []),
      getTools: vi.fn(async () => []),
      connect: vi.fn(async () => {}),
      disconnect: vi.fn(async () => {}),
      addServer: vi.fn(async () => {}),
      removeServer: vi.fn(async () => {}),
      saveCredential: vi.fn(async () => {}),
      deleteCredential: vi.fn(async () => {}),
    },
  };
});

describe("useMcpServers", () => {
  beforeEach(() => {
    capturedHandler.current = null;
    vi.clearAllMocks();
  });

  it("applies backend mcp-status-changed events live", async () => {
    const { result } = renderHook(() => useMcpServers());
    await waitFor(() => expect(result.current.servers).toHaveLength(1));

    act(() => {
      capturedHandler.current?.({ name: "srv", status: "error", error: "boom" });
    });
    expect(result.current.servers[0].status).toBe("error");
    expect(result.current.servers[0].errorMessage).toBe("boom");

    act(() => {
      capturedHandler.current?.({ name: "srv", status: "connected", tools: 2 });
    });
    expect(result.current.servers[0].status).toBe("connected");
    expect(result.current.servers[0].errorMessage).toBeNull();
  });

  it("refetches tools when the backend reports a connect", async () => {
    const getTools = vi.mocked(
      (await import("@/lib/tauri")).mcpApi.getTools,
    );
    const { result } = renderHook(() => useMcpServers());
    await waitFor(() => expect(result.current.servers).toHaveLength(1));

    act(() => {
      capturedHandler.current?.({ name: "srv", status: "connected", tools: 1 });
    });
    await waitFor(() => expect(getTools).toHaveBeenCalledWith("srv"));
    expect(result.current.servers[0].status).toBe("connected");
  });

  it("tracks credentialed servers after save and delete", async () => {
    const { result } = renderHook(() => useMcpServers());
    await waitFor(() => expect(result.current.servers).toHaveLength(1));

    const server = result.current.servers[0];
    await act(async () => {
      await result.current.saveCredential(server, {
        tokenEndpoint: "https://srv.example/oauth/token",
        clientId: "client-1",
        accessToken: "tok",
        refreshToken: "refresh",
        tokenType: "Bearer",
        expiresAt: "",
      });
    });
    expect(result.current.credentialed).toContain("srv");

    await act(async () => {
      await result.current.deleteCredential(server);
    });
    expect(result.current.credentialed).not.toContain("srv");
  });
});
