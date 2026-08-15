import { describe, it, expect, beforeEach, vi } from "vitest";
import { useAuthStore } from "@/stores/authStore";

const memory = new Map<string, string>();

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: true,
    TOKEN_STORAGE_KEY: "deepdepcat.auth.token",
    authKeyringApi: {
      storeToken: vi.fn(async (token: string) => {
        memory.set("token", token);
      }),
      loadToken: vi.fn(async () => memory.get("token") ?? null),
      deleteToken: vi.fn(async () => {
        memory.delete("token");
      }),
    },
    deviceAuthApi: {
      ...actual.deviceAuthApi,
      loginWithPassword: vi.fn(async () => ({
        access_token: "jwt-1",
        token_type: "Bearer",
        expires_in: 3600,
        user_id: "u1",
        username: "test",
      })),
      verifyToken: vi.fn(async () => ({
        valid: true,
        user_id: "u1",
        expires_at: null,
      })),
      revokeToken: vi.fn(async () => {}),
    },
  };
});

describe("authStore keyring persistence", () => {
  beforeEach(() => {
    memory.clear();
    localStorage.clear();
    useAuthStore.setState({
      user: null,
      serverUrl: "https://deepdepcat.hsmiai.xyz",
      status: "idle",
      error: null,
      errorKind: "none",
      loginLoading: false,
    });
  });

  it("stores the login token in the keyring and drops the legacy localStorage copy", async () => {
    localStorage.setItem("deepdepcat.auth.token", "legacy-jwt");
    const ok = await useAuthStore.getState().loginWithPassword("a@b.com", "secret1");
    expect(ok).toBe(true);
    expect(memory.get("token")).toBe("jwt-1");
    expect(localStorage.getItem("deepdepcat.auth.token")).toBeNull();
    expect(useAuthStore.getState().status).toBe("logged_in");
  });

  it("restores login from the keyring on init and verifies the token", async () => {
    memory.set("token", "jwt-1");
    localStorage.setItem(
      "deepdepcat.auth.user",
      JSON.stringify({ username: "test", user_id: "u1", avatar: null }),
    );
    useAuthStore.setState({ user: { username: "test", user_id: "u1", avatar: null } });
    await useAuthStore.getState().init();
    expect(useAuthStore.getState().status).toBe("logged_in");
    expect(await useAuthStore.getState().accessToken()).toBe("jwt-1");
  });

  it("still accepts a legacy localStorage token until the next save migrates it", async () => {
    localStorage.setItem("deepdepcat.auth.token", "legacy-jwt");
    localStorage.setItem(
      "deepdepcat.auth.user",
      JSON.stringify({ username: "test", user_id: "u1", avatar: null }),
    );
    useAuthStore.setState({ user: { username: "test", user_id: "u1", avatar: null } });
    await useAuthStore.getState().init();
    expect(useAuthStore.getState().status).toBe("logged_in");
    expect(await useAuthStore.getState().accessToken()).toBe("legacy-jwt");
  });

  it("clears the keyring token on logout", async () => {
    memory.set("token", "jwt-1");
    await useAuthStore.getState().logout();
    expect(memory.has("token")).toBe(false);
    expect(useAuthStore.getState().user).toBeNull();
    expect(useAuthStore.getState().status).toBe("idle");
  });
});
