import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, waitFor, screen, act } from "@testing-library/react";
import { AppShell } from "../AppShell";
import { useAppStore } from "@/stores/appStore";
import { useChatStore } from "@/stores/chatStore";
import { useTodoStore } from "@/stores/todoStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";

vi.mock("/icon.png", () => ({ default: "icon.png" }));
vi.mock("/icon-code.png", () => ({ default: "icon-code.png" }));
vi.mock("/icon-depwork.png", () => ({ default: "icon-depwork.png" }));
vi.mock("/icon-idle.png", () => ({ default: "icon-idle.png" }));
vi.mock("/blink-1.png", () => ({ default: "blink-1.png" }));
vi.mock("/blink-2.png", () => ({ default: "blink-2.png" }));
vi.mock("/blink-3.png", () => ({ default: "blink-3.png" }));

/** Handlers captured by event name — tests drive backend events by firing
 *  the captured handler (mirrors chatStore.test's chat-stream capture). */
const capturedHandlers = vi.hoisted(() => ({
  current: new Map<string, (payload: unknown) => void>(),
}));

vi.mock("@/lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("@/lib/tauri")>("@/lib/tauri");
  return {
    ...actual,
    onEvent: vi.fn((name: string, handler: (payload: unknown) => void) => {
      capturedHandlers.current.set(name, handler);
      return Promise.resolve(() => {});
    }),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue([]),
}));

vi.mock("@/hooks/useWindowControls", () => ({
  useWindowControls: () => ({
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
  }),
}));

beforeEach(() => {
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.hasPointerCapture = vi.fn(() => true);
  Element.prototype.releasePointerCapture = vi.fn();
  useAppStore.setState({
    sidebarCollapsed: false,
    sidebarUserManaged: false,
  });
  useRightPanelStore.setState({ open: false });
  useTodoStore.setState({ bySession: {} });
  capturedHandlers.current.clear();
  vi.stubGlobal("innerWidth", 1280);
  window.innerWidth = 1280;
});

describe("AppShell sidebar drag", () => {
  it("code mode shows the project selector and a single conversation list", () => {
    useAppStore.setState({
      mode: "code",
      workspacePath: null,
      workspaces: [],
    });
    render(<AppShell />);

    // The old Projects/Chats tabs are gone; the project lives in a dropdown.
    expect(
      screen.getByRole("button", { name: "浏览并打开项目…" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("对话")).not.toBeInTheDocument();
    expect(screen.queryByText("项目")).not.toBeInTheDocument();
  });

  it("pointerdown + move resizes the sidebar", async () => {
    const { container } = render(<AppShell />);
    const handle = container.querySelector(".drag-handle");
    expect(handle).not.toBeNull();
    const sidebarBox = container.querySelector("div.relative.shrink-0") as HTMLElement;
    expect(sidebarBox.style.width).toBe("200px");

    fireEvent.pointerDown(handle!, { clientX: 200, pointerId: 1 });
    fireEvent.pointerMove(handle!, { clientX: 260, pointerId: 1 });
    await waitFor(() => {
      expect(sidebarBox.style.width).toBe("260px");
    });
  });

  it("drag below minimum collapses", async () => {
    const { container } = render(<AppShell />);
    const handle = container.querySelector(".drag-handle") as HTMLElement;
    fireEvent.pointerDown(handle, { clientX: 260, pointerId: 1 });
    fireEvent.pointerMove(handle, { clientX: 50, pointerId: 1 });
    await waitFor(() => {
      expect(useAppStore.getState().sidebarCollapsed).toBe(true);
    });
  });

  it("collapsed rail: small drag stays collapsed, continued drag expands", async () => {
    useAppStore.setState({ sidebarCollapsed: true, sidebarUserManaged: true });
    const { container } = render(<AppShell />);
    const handle = container.querySelector(".drag-handle") as HTMLElement;
    const sidebarBox = container.querySelector("div.relative.shrink-0") as HTMLElement;

    fireEvent.pointerDown(handle, { clientX: 50, pointerId: 1 });
    // +60px → 108px, still below the 200px minimum: must stay collapsed and
    // keep dragging (regression: this used to endDrag and kill the drag).
    fireEvent.pointerMove(handle, { clientX: 110, pointerId: 1 });
    await waitFor(() => {
      expect(useAppStore.getState().sidebarCollapsed).toBe(true);
    });
    // Continue dragging to cross the minimum → expands with the new width.
    fireEvent.pointerMove(handle, { clientX: 300, pointerId: 1 });
    await waitFor(() => {
      expect(useAppStore.getState().sidebarCollapsed).toBe(false);
      expect(sidebarBox.style.width).toBe("298px");
    });
    fireEvent.pointerUp(handle, { pointerId: 1 });
  });

  it("collapsed rail still renders the handle and drag-right expands", async () => {
    useAppStore.setState({ sidebarCollapsed: true, sidebarUserManaged: true });
    const { container } = render(<AppShell />);
    const handle = container.querySelector(".drag-handle") as HTMLElement;
    expect(handle).not.toBeNull();
    const sidebarBox = container.querySelector("div.relative.shrink-0") as HTMLElement;
    expect(sidebarBox.style.width).toBe("48px");

    fireEvent.pointerDown(handle, { clientX: 50, pointerId: 1 });
    fireEvent.pointerMove(handle, { clientX: 250, pointerId: 1 });
    await waitFor(() => {
      expect(useAppStore.getState().sidebarCollapsed).toBe(false);
      expect(sidebarBox.style.width).toBe("248px");
    });
  });

  it("auto-collapse respects a user-managed expanded state on narrow windows", async () => {
    useAppStore.setState({ sidebarUserManaged: true, sidebarCollapsed: false });
    window.innerWidth = 1000;
    window.dispatchEvent(new Event("resize"));
    await waitFor(() => {
      expect(useAppStore.getState().sidebarCollapsed).toBe(false);
    });
  });
});

describe("AppShell todo pipeline wiring", () => {
  it("subscribes at the shell: a todo-list-updated event reaches the store and auto-opens the task pane with no TaskPanel mounted", async () => {
    useAppStore.setState({ mode: "code" });
    useChatStore.setState({ currentSessionId: "s1", isStreaming: false });
    useRightPanelStore.setState({
      open: false,
      panes: { code: [], depwork: [] },
      pendingPreview: { code: null, depwork: null },
      autoOpenSuppressed: { code: false, depwork: false },
      activitySignal: { code: false, depwork: false },
    });

    render(<AppShell />);

    const fire = capturedHandlers.current.get("todo-list-updated");
    expect(fire).toBeDefined();
    act(() => {
      fire!({
        session_id: "s1",
        todos: [{ id: "t1", content: "step one", status: "pending" }],
      });
    });

    // Regression guard: the todo subscription lives at the shell, NOT gated
    // behind the task pane's own mount — otherwise the code-mode task pane
    // never auto-opens (todos only load while the pane is open, and the pane
    // only opens when todos appear).
    await waitFor(() => {
      expect(useTodoStore.getState().bySession["s1"]).toHaveLength(1);
      expect(useRightPanelStore.getState().open).toBe(true);
      expect(useRightPanelStore.getState().panes.code).toContain("task");
    });
  });
});
