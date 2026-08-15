/**
 * sessionTracker tests — the crash-recovery session persistence.
 *
 * Covers:
 *  - tracker subscription persists the last active session (both stores)
 *  - store isolation: code vs depwork are remembered separately
 *  - clearMessages (null id) does not clobber the remembered session
 *  - overwriting the session id updates the remembered session
 *  - prepareCrashRecovery restores / degrades / clears the key
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import {
  PREF_LAST_SESSION,
  startSessionTracking,
  _resetSessionTrackingForTest,
  prepareCrashRecovery,
} from "@/lib/sessionTracker";

function loadStoredSession() {
  const raw = localStorage.getItem(PREF_LAST_SESSION);
  return raw ? (JSON.parse(raw) as { mode: "code" | "depwork"; sessionId: string }) : null;
}

function setLastSessionStorage(ref: { mode: "code" | "depwork"; sessionId: string }) {
  localStorage.setItem(PREF_LAST_SESSION, JSON.stringify(ref));
}

describe("sessionTracker", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    _resetSessionTrackingForTest();
    localStorage.clear();
    useChatStore.setState({ currentSessionId: null });
    useDepworkChatStore.setState({ currentSessionId: null });
  });

  describe("tracking subscription", () => {
    it("persists the code session when currentSessionId changes", async () => {
      await startSessionTracking();
      useChatStore.setState({ currentSessionId: "s1" });
      expect(loadStoredSession()).toEqual({ mode: "code", sessionId: "s1" });
    });

    it("persists the depwork session when currentSessionId changes", async () => {
      await startSessionTracking();
      useDepworkChatStore.setState({ currentSessionId: "d1" });
      expect(loadStoredSession()).toEqual({ mode: "depwork", sessionId: "d1" });
    });

    it("overwrites when a new session becomes active", async () => {
      await startSessionTracking();
      useChatStore.setState({ currentSessionId: "s1" });
      useChatStore.setState({ currentSessionId: "s2" });
      expect(loadStoredSession()).toEqual({ mode: "code", sessionId: "s2" });
    });

    it("does not write when currentSessionId goes null (clearMessages)", async () => {
      setLastSessionStorage({ mode: "code", sessionId: "s1" });
      await startSessionTracking();
      useChatStore.setState({ currentSessionId: null });
      // Remembered session is untouched by clearing the active one.
      expect(loadStoredSession()).toEqual({ mode: "code", sessionId: "s1" });
    });

    it("is idempotent — calling startSessionTracking twice registers once", async () => {
      await startSessionTracking();
      await startSessionTracking();
      useChatStore.setState({ currentSessionId: "s1" });
      expect(loadStoredSession()).toEqual({ mode: "code", sessionId: "s1" });
    });
  });

  describe("prepareCrashRecovery", () => {
    it("restores the remembered session and clears the key", async () => {
      setLastSessionStorage({ mode: "code", sessionId: "s1" });
      const selectById = vi.fn().mockResolvedValue(undefined);
      const result = await prepareCrashRecovery(selectById);
      expect(result).toBe(true);
      expect(selectById).toHaveBeenCalledWith("s1");
      expect(loadStoredSession()).toBeNull();
    });

    it("returns false when no session was remembered", async () => {
      const selectById = vi.fn().mockResolvedValue(undefined);
      const result = await prepareCrashRecovery(selectById);
      expect(result).toBe(false);
      expect(selectById).not.toHaveBeenCalled();
    });

    it("degrades silently when the restore fails, and still clears the key", async () => {
      setLastSessionStorage({ mode: "code", sessionId: "deleted" });
      const selectById = vi.fn().mockRejectedValue(new Error("404"));
      const result = await prepareCrashRecovery(selectById);
      expect(result).toBe(false);
      expect(loadStoredSession()).toBeNull();
    });
  });
});
