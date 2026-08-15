/**
 * BrowserLivePane tests — the live mirror of the agent's real browser:
 * - starts/stops screencast for the session profile on mount/unmount
 * - renders the latest screencast frame for the matching profile
 * - ignores frames from other profiles
 * - shows the takeover banner when the agent hands off to the user
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";
import { BrowserLivePane } from "@/components/preview/BrowserLivePane";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

const capturedHandlers = vi.hoisted(() => ({
  current: new Map<string, (payload: unknown) => void>(),
}));

const browserApiMock = vi.hoisted(() => ({
  screencastStart: vi.fn(async () => {}),
  screencastStop: vi.fn(async () => {}),
  status: vi.fn(async () => ({
    running: true,
    url: "https://example.com",
    title: "Example",
    awaiting_takeover: false,
    takeover_reason: null,
    profile: "session-s1",
    headless: false,
    download_dir: null,
  })),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    onEvent: vi.fn((name: string, handler: (payload: unknown) => void) => {
      capturedHandlers.current.set(name, handler);
      return Promise.resolve(() => {});
    }),
    browserApi: browserApiMock,
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

beforeEach(() => {
  capturedHandlers.current.clear();
  browserApiMock.screencastStart.mockClear();
  browserApiMock.screencastStop.mockClear();
  browserApiMock.status.mockClear();
  useChatStore.setState({ currentSessionId: "s1", isStreaming: false });
  useDepworkChatStore.setState({ currentSessionId: null, isStreaming: false });
});

function fire(name: string, payload: unknown) {
  act(() => {
    capturedHandlers.current.get(name)?.(payload);
  });
}

describe("BrowserLivePane", () => {
  it("starts screencast and polls status for the session profile on mount", async () => {
    render(<BrowserLivePane mode="code" />);
    await waitFor(() => {
      expect(browserApiMock.screencastStart).toHaveBeenCalledWith("session-s1");
      expect(browserApiMock.status).toHaveBeenCalledWith("session-s1");
    });
  });

  it("renders the live frame for the matching profile", async () => {
    render(<BrowserLivePane mode="code" />);
    fire("browser-screencast-frame", {
      profile: "session-s1",
      jpeg: "AAAA",
      vw: 800,
      vh: 600,
      seq: 1,
    });
    await waitFor(() => {
      const img = document.querySelector("img");
      expect(img).not.toBeNull();
      expect(img!.getAttribute("src")).toBe("data:image/jpeg;base64,AAAA");
    });
  });

  it("ignores frames from other profiles", async () => {
    render(<BrowserLivePane mode="code" />);
    fire("browser-screencast-frame", {
      profile: "takeover",
      jpeg: "BBBB",
      vw: 800,
      vh: 600,
      seq: 1,
    });
    await waitFor(() => {
      expect(browserApiMock.status).toHaveBeenCalled();
    });
    expect(document.querySelector("img")).toBeNull();
  });

  it("stops screencast on unmount", () => {
    const { unmount } = render(<BrowserLivePane mode="code" />);
    unmount();
    expect(browserApiMock.screencastStop).toHaveBeenCalledWith("session-s1");
  });

  it("shows the takeover banner for the matching profile", async () => {
    render(<BrowserLivePane mode="code" />);
    fire("browser-takeover-requested", { reason: "captcha", profile: "session-s1" });
    await waitFor(() => {
      expect(screen.getByText("takeover.title")).toBeTruthy();
    });
  });
});
