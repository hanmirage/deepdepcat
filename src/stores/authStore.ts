/**
 * Auth store — global login state via direct email+password against the
 * website account system.
 *
 * Flow:
 *   1. User enters email+password in the login dialog.
 *   2. Rust `login_with_password` posts to the website's `/api/auth/login`
 *      (gets a website JWT) then to `/api/v1/auth/web-session` (resolves the
 *      account identity). Both calls happen server-side in reqwest (no CORS).
 *   3. On success, persist the token → logged in.
 *   4. On app restart, verify the persisted token against `/api/v1/auth/verify`
 *      (the backend accepts website JWTs there, so the login survives restarts).
 *
 * Token stored in the OS keyring (Tauri):
 *   auth_store_token / auth_load_token / auth_delete_token — a website JWT.
 *   Legacy `deepdepcat.auth.token` in localStorage is migrated on first
 *   load and then removed (never written again in the desktop app).
 *   deepdepcat.auth.user      — { username, user_id, avatar } (avatar is an
 *                                absolute URL — relative cloud paths are
 *                                resolved against serverUrl on save/load)
 *   deepdepcat.auth.serverUrl — configured server URL (synced from settingsStore)
 */

import { create } from "zustand";
import { deviceAuthApi, authKeyringApi, isTauri, TOKEN_STORAGE_KEY } from "@/lib/tauri";

export type AuthStatus = "idle" | "logged_in" | "verifying" | "error";

/** Classified login failure — the UI translates this into friendly copy. */
export type AuthErrorKind =
  | "none"
  | "network"
  | "expired"
  | "rejected"
  | "session"
  | "account"
  | "rate_limited"
  | "unknown";

export interface AuthUserInfo {
  username: string;
  user_id: string | null;
  /** Account avatar URL from the website ("" when unset — UI falls back to the default icon). */
  avatar?: string | null;
}

const USER_KEY = "deepdepcat.auth.user";
const SERVER_KEY = "deepdepcat.auth.serverUrl";

const DEFAULT_SERVER_URL = "https://deepdepcat.hsmiai.xyz";

// ── Helpers ────────────────────────────────────────────────

/** Load the persisted access token — OS keyring first, then the legacy
 *  localStorage slot (migrated out on the next successful save). */
async function loadPersistedToken(): Promise<string | null> {
  const token = await authKeyringApi.loadToken();
  if (token) return token;
  if (!isTauri) return null;
  try { return localStorage.getItem(TOKEN_STORAGE_KEY); } catch { return null; }
}
async function savePersistedToken(token: string): Promise<void> {
  await authKeyringApi.storeToken(token);
  // One-time migration: drop the legacy localStorage copy once the keyring
  // write succeeds (browser dev mode keeps the localStorage fallback).
  if (isTauri) {
    try { localStorage.removeItem(TOKEN_STORAGE_KEY); } catch { /* ignore */ }
  }
}
async function clearPersistedToken(): Promise<void> {
  await authKeyringApi.deleteToken();
  try {
    localStorage.removeItem(TOKEN_STORAGE_KEY);
    localStorage.removeItem(USER_KEY);
  } catch { /* ignore */ }
}
function loadPersistedUser(): AuthUserInfo | null {
  try {
    const s = localStorage.getItem(USER_KEY);
    if (!s) return null;
    const u: AuthUserInfo = JSON.parse(s);
    // Normalize any legacy relative avatar path into an absolute URL so the
    // webview can load it (older builds stored the raw `/uploads/...` path).
    if (u.avatar && !/^https?:\/\//i.test(u.avatar)) {
      u.avatar = resolveAvatarUrl(u.avatar, loadPersistedServerUrl());
    }
    return u;
  } catch { return null; }
}
function savePersistedUser(user: AuthUserInfo) {
  try { localStorage.setItem(USER_KEY, JSON.stringify(user)); } catch { /* ignore */ }
}
function loadPersistedServerUrl(): string {
  try { return localStorage.getItem(SERVER_KEY) || DEFAULT_SERVER_URL; } catch { return DEFAULT_SERVER_URL; }
}
function savePersistedServerUrl(url: string) {
  try { localStorage.setItem(SERVER_KEY, url); } catch { /* ignore */ }
}

/**
 * Resolve a cloud avatar path into an absolute URL the webview can load.
 * The website returns a relative path like `/uploads/avatars/x.jpg`; inside
 * the Tauri webview a bare relative src resolves against `tauri://localhost`
 * and fails, so we prefix the server origin. Absolute URLs pass through.
 */
function resolveAvatarUrl(avatar: string | null | undefined, serverUrl: string): string | null {
  if (!avatar) return null;
  if (/^https?:\/\//i.test(avatar)) return avatar;
  return `${serverUrl.replace(/\/+$/, "")}${avatar.startsWith("/") ? avatar : `/${avatar}`}`;
}

/** Classify a login failure into a friendly category. */
function classifyAuthError(e: unknown): { kind: AuthErrorKind; detail: string } {
  const raw = String(e);
  const lower = raw.toLowerCase();
  if (e === "invalid_credentials") return { kind: "rejected", detail: raw };
  if (e === "invalid_session") return { kind: "session", detail: raw };
  if (e === "account_unavailable") return { kind: "account", detail: raw };
  if (raw.startsWith("rate_limited:")) {
    return { kind: "rate_limited", detail: raw.slice("rate_limited:".length) };
  }
  if (
    lower.includes("fetch") ||
    lower.includes("network") ||
    lower.includes("connect") ||
    lower.includes("timed out") ||
    lower.includes("timeout")
  ) {
    return { kind: "network", detail: raw };
  }
  return { kind: "unknown", detail: raw };
}

// ── Auth store ────────────────────────────────────────────

interface AuthState {
  user: AuthUserInfo | null;
  serverUrl: string;

  status: AuthStatus;
  error: string | null;
  /** Classified failure (see AuthErrorKind) — drives friendly UI copy. */
  errorKind: AuthErrorKind;
  /** True while a direct email+password login is in flight. */
  loginLoading: boolean;

  // Actions
  /** Direct email+password login against the website account system. */
  loginWithPassword: (email: string, password: string) => Promise<boolean>;
  logout: () => Promise<void>;
  verifyLogin: () => Promise<boolean>;
  init: () => Promise<void>;
  setServerUrl: (url: string) => void;
  /** Clear a classified failure — call when a dialog surfaces an error
   *  condition (open/close) so a stale error from another surface (e.g.
   *  onboarding) doesn't leak into this dialog's first render. */
  clearError: () => void;
  /** Rename the display name on the website account (cloud sync). */
  updateProfile: (name: string) => Promise<boolean>;
  /** Upload an avatar image to the website account (cloud sync). */
  uploadAvatar: (filePath: string) => Promise<boolean>;

  /** The persisted access token (may be expired — verify before use). */
  accessToken: () => Promise<string | null>;
}

export const useAuthStore = create<AuthState>()((set, get) => ({
  user: loadPersistedUser(),
  serverUrl: loadPersistedServerUrl(),
  status: "idle",
  error: null,
  errorKind: "none",
  loginLoading: false,

  setServerUrl: (url) => {
    savePersistedServerUrl(url);
    set({ serverUrl: url });
  },
  clearError: () => set({ error: null, errorKind: "none" }),
  accessToken: () => loadPersistedToken(),

  // ── Init: called once on app startup ────────────────────
  init: async () => {
    // Sync server URL from settings (user-configurable backend).
    try {
      const { useSettingsStore } = await import("@/stores/settingsStore");
      const settingsServerUrl = useSettingsStore.getState().general.serverUrl;
      if (settingsServerUrl) {
        savePersistedServerUrl(settingsServerUrl);
        set({ serverUrl: settingsServerUrl });
      }
    } catch { /* settings store unavailable — keep persisted/default */ }

    const token = await loadPersistedToken();
    const state = get();
    if (state.user && token) {
      try {
        set({ status: "verifying" });
        const resp = await deviceAuthApi.verifyToken(state.serverUrl, token);
        if (resp.valid) {
          // Refresh the avatar from the cloud on startup ("" clears to default).
          if (resp.avatar !== undefined) {
            const updated = { ...state.user, avatar: resolveAvatarUrl(resp.avatar, state.serverUrl) };
            savePersistedUser(updated);
            set({ user: updated });
          }
          set({ status: "logged_in", error: null });
          return;
        }
        // Token was explicitly reported invalid (revoked/expired) — clear it.
        await clearPersistedToken();
        set({ user: null, status: "idle", error: null });
        return;
      } catch {
        // Any failure here (offline, timeout, server 5xx, gateway error) is
        // treated as transient — keep the persisted token so a momentary
        // outage doesn't log the user out. The backend reports invalid tokens
        // as `valid: false` (HTTP 200), NOT as an HTTP error, so there is no
        // legitimate "HTTP error = invalid token" case to clear on.
        // Matches verifyLogin()'s behavior below.
        set({ status: "logged_in", error: null });
        return;
      }
    }
    // Persisted user but no token (keyring wiped/lost) — that's a phantom
    // login: every authed call would fail with an empty token. Clear it so
    // the footer doesn't show "logged in" for credentials that don't exist.
    if (state.user && !token) {
      await clearPersistedToken();
      set({ user: null, status: "idle", error: null });
      return;
    }
    set({ status: "idle" });
  },

  // ── Verify persisted login (re-run from SidebarFooter) ──
  verifyLogin: async () => {
    const token = await loadPersistedToken();
    const state = get();
    if (state.user && token) {
      try {
        set({ status: "verifying", error: null });
        const resp = await deviceAuthApi.verifyToken(state.serverUrl, token);
        if (resp.valid) {
          // Refresh the avatar from the cloud when it changed server-side.
          if (resp.avatar !== undefined) {
            const updated = { ...state.user, avatar: resolveAvatarUrl(resp.avatar, state.serverUrl) };
            savePersistedUser(updated);
            set({ user: updated });
          }
          set({ status: "logged_in", error: null });
          return true;
        }
        await clearPersistedToken();
        set({ user: null, status: "idle", error: null });
        return false;
      } catch {
        // Transient failure — keep the token, don't log out.
        set({ status: "logged_in", error: null });
        return true;
      }
    }
    // Persisted user with no token = phantom login (same as init) — clear it.
    if (state.user && !token) {
      await clearPersistedToken();
      set({ user: null, status: "idle", error: null });
    }
    return false;
  },

  // ── Direct email+password login (website account) ──────
  loginWithPassword: async (email, password) => {
    const state = get();
    if (state.loginLoading) return false;

    set({ loginLoading: true, error: null, errorKind: "none" });
    try {
      const resp = await deviceAuthApi.loginWithPassword(state.serverUrl, email, password);
      await savePersistedToken(resp.access_token);
      const userInfo: AuthUserInfo = {
        username: resp.username || email,
        user_id: resp.user_id || null,
        avatar: resolveAvatarUrl(resp.avatar, state.serverUrl),
      };
      savePersistedUser(userInfo);
      set({
        user: userInfo,
        status: "logged_in",
        error: null,
        errorKind: "none",
        loginLoading: false,
      });
      return true;
    } catch (e) {
      const { kind, detail } = classifyAuthError(e);
      set({ status: "error", error: detail, errorKind: kind, loginLoading: false });
      return false;
    }
  },

  logout: async () => {
    const state = get();
    const token = await loadPersistedToken();
    if (token) {
      try { await deviceAuthApi.revokeToken(state.serverUrl, token); } catch { /* ignore */ }
    }
    await clearPersistedToken();
    set({ user: null, status: "idle", error: null });
  },

  // ── Rename display name (cloud sync) ─────────────────────
  updateProfile: async (name) => {
    const state = get();
    const token = await loadPersistedToken();
    if (!state.user || !token) return false;
    const trimmed = name.trim();
    if (!trimmed) return false;
    try {
      const serverName = await deviceAuthApi.updateUserProfile(state.serverUrl, token, trimmed);
      const updated = { ...state.user, username: serverName || trimmed };
      savePersistedUser(updated);
      set({ user: updated, error: null, errorKind: "none" });
      return true;
    } catch (e) {
      const { kind, detail } = classifyAuthError(e);
      set({ error: detail, errorKind: kind });
      return false;
    }
  },

  // ── Upload avatar (cloud sync) ───────────────────────────
  uploadAvatar: async (filePath) => {
    const state = get();
    const token = await loadPersistedToken();
    if (!state.user || !token) return false;
    try {
      const avatarPath = await deviceAuthApi.uploadAvatar(state.serverUrl, token, filePath);
      const updated = { ...state.user, avatar: resolveAvatarUrl(avatarPath, state.serverUrl) };
      savePersistedUser(updated);
      set({ user: updated, error: null, errorKind: "none" });
      return true;
    } catch (e) {
      const { kind, detail } = classifyAuthError(e);
      set({ error: detail, errorKind: kind });
      return false;
    }
  },
}));
