/**
 * PermissionDialog tests — the permission-request → dialog render contract.
 *
 * Regression guard: the dialog must survive the request queue going from
 * EMPTY to NON-EMPTY. The sensitive-file guard used to run its useMemo
 * AFTER an early `return null`, which flipped the hook count between
 * renders and crashed the whole app (white screen, no error boundary).
 * The store transition is driven directly so the test pins the render
 * contract without depending on the Tauri event bus.
 */

import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { PermissionDialog } from "@/components/chat/PermissionDialog";
import { usePermissionStore } from "@/stores/permissionStore";
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
    onEvent: vi.fn(async () => () => {}),
    permissionApi: {
      ...actual.permissionApi,
      respond: vi.fn(async () => {}),
    },
  };
});

describe("PermissionDialog", () => {
  it("renders nothing without a request", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const { container } = render(<PermissionDialog />);
    expect(container.firstChild).toBeNull();
  });

  it("renders the permission card when a request arrives", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const { container } = render(<PermissionDialog />);
    expect(container.firstChild).toBeNull();

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-1",
        tool_name: "bash",
        args_summary: "cargo test",
        session_id: "session-1",
      });
    });

    expect(screen.getByText("bash")).toBeTruthy();
    expect(screen.getByText("cargo test")).toBeTruthy();
  });

  it("shows the grant scope and sends the chosen granularity on always allow", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const respond = vi.mocked(permissionApi.respond);
    respond.mockClear();

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-path",
        tool_name: "edit_file",
        args_summary: "src/main.rs",
        session_id: "session-1",
        grant_pattern: "path:D:/proj/src/main.rs",
        grant_scope: "路径 D:/proj/src/main.rs",
      });
    });
    render(<PermissionDialog />);

    fireEvent.click(screen.getByText("permission.alwaysAllow"));
    expect(screen.getByText(/「始终允许」将记住/)).toBeTruthy();
    expect(screen.getByText(/路径 D:\/proj\/src\/main\.rs/)).toBeTruthy();

    fireEvent.click(screen.getByText("整个工具"));
    expect(respond).toHaveBeenCalledWith("req-path", "always_allow", { scope: "tool" });
  });

  it("keeps the exact scope as the default always-allow choice", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const respond = vi.mocked(permissionApi.respond);
    respond.mockClear();

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-path-2",
        tool_name: "edit_file",
        args_summary: "src/main.rs",
        session_id: "session-1",
        grant_pattern: "path:D:/proj/src/main.rs",
        grant_scope: "路径 D:/proj/src/main.rs",
      });
    });
    render(<PermissionDialog />);

    fireEvent.click(screen.getByText("permission.alwaysAllow"));
    fireEvent.click(screen.getByText("仅本次范围"));
    expect(respond).toHaveBeenCalledWith("req-path-2", "always_allow", { scope: "pattern" });
  });

  it("sends the rejection reason when the user types one", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const respond = vi.mocked(permissionApi.respond);
    respond.mockClear();

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-deny",
        tool_name: "bash",
        args_summary: "rm -rf tmp",
        session_id: "session-1",
        grant_pattern: "cmd:rm",
        grant_scope: "bash 命令（rm 开头）",
      });
    });
    render(<PermissionDialog />);

    fireEvent.click(screen.getByText("permission.deny"));
    fireEvent.change(screen.getByPlaceholderText("为什么拒绝？（可选，将反馈给 agent）"), {
      target: { value: "不要删临时目录" },
    });
    fireEvent.click(screen.getByText("确认拒绝"));
    expect(respond).toHaveBeenCalledWith("req-deny", "deny", { reason: "不要删临时目录" });
  });

  it("responds immediately for whole-tool grants without a scope chooser", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const respond = vi.mocked(permissionApi.respond);
    respond.mockClear();

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-whole",
        tool_name: "todo_write",
        args_summary: "{}",
        session_id: "session-1",
        grant_pattern: "*",
        grant_scope: "整个工具 todo_write 的所有调用",
      });
    });
    render(<PermissionDialog />);

    fireEvent.click(screen.getByText("permission.alwaysAllow"));
    expect(respond).toHaveBeenCalledWith("req-whole", "always_allow", { scope: "pattern" });
  });

  it("only shows requests belonging to the active session", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    const { container, rerender } = render(<PermissionDialog sessionId="session-1" />);

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-other",
        tool_name: "edit_file",
        args_summary: "other session edit",
        session_id: "session-2",
      });
    });
    // Background session's request must NOT hijack this conversation.
    expect(container.firstChild).toBeNull();

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-own",
        tool_name: "edit_file",
        args_summary: "this session edit",
        session_id: "session-1",
      });
    });
    rerender(<PermissionDialog sessionId="session-1" />);
    expect(screen.getByText("this session edit")).toBeTruthy();
    expect(screen.queryByText("other session edit")).toBeNull();
  });

  it("shows a subagent badge for worker requests routed to the parent", () => {
    usePermissionStore.setState({ queue: [], responding: false });
    render(<PermissionDialog sessionId="session-1" />);

    act(() => {
      usePermissionStore.getState().enqueue({
        request_id: "req-sub",
        tool_name: "bash",
        args_summary: "cargo test",
        session_id: "worker-1",
        parent_session_id: "session-1",
      });
    });
    expect(screen.getByText("子代理")).toBeTruthy();
  });
});
