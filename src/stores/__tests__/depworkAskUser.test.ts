/**
 * Depwork ask-user chain — store actions + session routing.
 *
 * Depwork sessions previously answered ask_user requests through the code
 * store (wrong session, wrong dialog). These tests lock the per-session
 * routing: depwork session ids land in depworkChatStore, everything else
 * stays in chatStore.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useChatStore } from "@/stores/chatStore";
import { useAppStore } from "@/stores/appStore";
import { useAskUserEvents } from "@/hooks/useAskUserEvents";
import type { UserAskRequest } from "@/lib/tauri";

const { capturedHandler, respondMock } = vi.hoisted(() => ({
  capturedHandler: { current: null as ((payload: unknown) => void) | null },
  respondMock: vi.fn(async () => true),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    onEvent: vi.fn(async (_name: string, handler: (payload: unknown) => void) => {
      capturedHandler.current = handler;
      return () => {};
    }),
    askUserApi: {
      respond: respondMock,
    },
  };
});

const REQ: UserAskRequest = {
  request_id: "r1",
  session_id: "dw-1",
  question: "确认继续？",
  options: ["是", "否"],
};

describe("depworkChatStore ask-user actions", () => {
  beforeEach(() => {
    useDepworkChatStore.setState({ pendingAskUser: null });
    respondMock.mockClear();
  });

  it("stores a pending ask-user request", () => {
    useDepworkChatStore.getState().setPendingAskUser(REQ);
    expect(useDepworkChatStore.getState().pendingAskUser).toEqual(REQ);
  });

  it("replies to the backend with the request id and clears state", async () => {
    useDepworkChatStore.getState().setPendingAskUser(REQ);
    await useDepworkChatStore.getState().respondAskUser("自定义答复");
    expect(respondMock).toHaveBeenCalledWith("r1", "自定义答复");
    expect(useDepworkChatStore.getState().pendingAskUser).toBeNull();
  });
});

describe("useAskUserEvents session routing", () => {
  beforeEach(() => {
    capturedHandler.current = null;
    useAppStore.setState({ mode: "depwork" });
    useDepworkChatStore.setState({ currentSessionId: "dw-1", pendingAskUser: null });
    useChatStore.setState({ pendingAskUser: null });
  });

  it("routes the current depwork session's request into depworkChatStore", () => {
    renderHook(() => useAskUserEvents());
    act(() => capturedHandler.current!(REQ));
    expect(useDepworkChatStore.getState().pendingAskUser).toEqual(REQ);
    expect(useChatStore.getState().pendingAskUser).toBeNull();
  });

  it("routes a background depwork session's request into depworkChatStore too", () => {
    // A parallel (non-current) depwork session asks a question while depwork
    // mode is showing a different session. The reply resumes the backend by
    // request_id, so the ask must land in the visible store — routing it to
    // chatStore would make it invisible and the agent would hang for its
    // 5-minute timeout.
    useAppStore.setState({ mode: "depwork" });
    const bgReq: UserAskRequest = { ...REQ, session_id: "dw-2" };
    renderHook(() => useAskUserEvents());
    act(() => capturedHandler.current!(bgReq));
    expect(useDepworkChatStore.getState().pendingAskUser).toEqual(bgReq);
    expect(useChatStore.getState().pendingAskUser).toBeNull();
  });

  it("keeps code-mode requests in chatStore", () => {
    useAppStore.setState({ mode: "code" });
    renderHook(() => useAskUserEvents());
    act(() => capturedHandler.current!(REQ));
    expect(useChatStore.getState().pendingAskUser).toEqual(REQ);
    expect(useDepworkChatStore.getState().pendingAskUser).toBeNull();
  });
});
