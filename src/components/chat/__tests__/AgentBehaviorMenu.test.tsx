/**
 * AgentBehaviorMenu tests — the input-bar permission-mode selector.
 *
 * One pill button with three permission modes (只读 / 接受编辑 / 完全放行).
 * Execution strategy and persona moved to Settings → Agent.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { AgentBehaviorMenu } from "@/components/chat/AgentBehaviorMenu";
import { useChatStore } from "@/stores/chatStore";
import { useAppStore } from "@/stores/appStore";
import { permissionApi } from "@/lib/tauri";

vi.mock("react-i18next", () => ({
  initReactI18next: { type: "3rdParty", init: () => {} },
  useTranslation: () => ({
    t: (key: string, opts?: { defaultValue?: string }) => opts?.defaultValue ?? key,
  }),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: false,
    sessionApi: {
      ...actual.sessionApi,
      getSession: vi.fn(async () => null),
    },
    permissionApi: {
      ...actual.permissionApi,
      setMode: vi.fn(async () => {}),
    },
  };
});

function openMenu() {
  const trigger = screen.getByRole("button", { name: "chat.interactionMode" });
  fireEvent.pointerDown(trigger);
}

beforeEach(() => {
  useAppStore.setState({ mode: "code" });
  useChatStore.setState({ interactionMode: "accept_edits", currentSessionId: null });
  localStorage.clear();
  vi.mocked(permissionApi.setMode).mockClear();
});

describe("AgentBehaviorMenu", () => {
  it("switches the permission mode locally when no session exists yet", () => {
    render(<AgentBehaviorMenu />);
    openMenu();
    fireEvent.click(screen.getByText("chat.modeFullAccess"));

    expect(useChatStore.getState().interactionMode).toBe("full_access");
    // No session → the choice stays LOCAL (writing it without a session id
    // would leak into the global mode and every other conversation).
    expect(permissionApi.setMode).not.toHaveBeenCalled();
  });

  it("persists the permission mode to the backend for the active session", () => {
    useChatStore.setState({ currentSessionId: "s1" });
    render(<AgentBehaviorMenu />);
    openMenu();
    fireEvent.click(screen.getByText("chat.modeFullAccess"));

    expect(useChatStore.getState().interactionMode).toBe("full_access");
    expect(permissionApi.setMode).toHaveBeenCalledWith("full_access", "s1");
  });

  it("shows only the permission modes — execution strategy and persona moved out", () => {
    render(<AgentBehaviorMenu />);
    openMenu();

    // The trigger label duplicates one of the option labels (default
    // accept_edits), so use getAllByText for presence checks.
    for (const label of ["chat.modeReadOnly", "chat.modeAcceptEdits", "chat.modeFullAccess"]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
    // Execution strategy + persona now live in Settings → Agent.
    expect(screen.queryByText("chat.agentMode")).toBeNull();
    expect(screen.queryByText("chat.agentPersona")).toBeNull();
  });
});
