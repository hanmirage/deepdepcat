/**
 * CrashReportDialog tests — the crash-report dialog behavior.
 *
 * Covers:
 *  - renders nothing when there is no pending crash
 *  - shows the privacy statement + two opt-in options when a crash is pending
 *  - "send error only" submits without conversation
 *  - "attach conversation" exports and submits with conversation
 *  - dismiss clears the pending crash
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CrashReportDialog } from "@/components/chat/CrashReportDialog";
import { crashApi, sessionApi, type PendingCrash } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";
import { PREF_LAST_SESSION } from "@/lib/sessionTracker";

const pending: PendingCrash = {
  client_id: "test-client",
  app_version: "0.1.0",
  os: "windows",
  arch: "x86_64",
  pid: 42,
  panic_message: "panic: index out of bounds",
  backtrace: "stack frame 0\nstack frame 1",
  timestamp: "20260801_000000",
};

describe("CrashReportDialog", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    // Reset any session id left over from a previous test.
    useChatStore.setState({ currentSessionId: null });
    localStorage.removeItem(PREF_LAST_SESSION);
  });

  it("renders nothing when there is no pending crash", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(null);
    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("shows privacy statement + both options when a crash is pending", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    render(<CrashReportDialog />);

    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
    expect(screen.getByText(/非常尊重您的隐私/)).toBeInTheDocument();
    expect(screen.getByText(/仅发送报错代码/)).toBeInTheDocument();
    expect(screen.getByText(/携带 JSON 对话文件/)).toBeInTheDocument();
  });

  it("sends error code only (no conversation) when that option is chosen", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    vi.spyOn(crashApi, "dismissPending").mockResolvedValue();
    const exportSpy = vi.spyOn(crashApi, "exportSessionConversation").mockResolvedValue("[]");
    const submitSpy = vi
      .spyOn(crashApi, "submit")
      .mockResolvedValue({ status: "accepted", crash_id: 1 });

    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());

    // "仅发送报错代码" is the default selection — click send.
    await userEvent.click(screen.getByRole("button", { name: /发送报告/ }));
    await waitFor(() =>
      expect(submitSpy).toHaveBeenCalledWith(
        "https://deepdepcat.hsmiai.xyz",
        false,
        null,
      ),
    );
    expect(exportSpy).not.toHaveBeenCalled();
    // Successful send closes the dialog.
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("exports + sends conversation when that option is chosen", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    vi.spyOn(crashApi, "dismissPending").mockResolvedValue();
    // A crash happens with an active session — the export path needs it.
    useChatStore.setState({ currentSessionId: "session-1" });
    const exportSpy = vi.spyOn(crashApi, "exportSessionConversation").mockResolvedValue('{"m":1}');
    const submitSpy = vi
      .spyOn(crashApi, "submit")
      .mockResolvedValue({ status: "accepted", crash_id: 2 });

    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());

    // Select "携带 JSON 对话文件", then send.
    await userEvent.click(screen.getByText(/携带 JSON 对话文件/));
    await userEvent.click(screen.getByRole("button", { name: /发送报告/ }));

    await waitFor(() =>
      expect(submitSpy).toHaveBeenCalledWith(
        "https://deepdepcat.hsmiai.xyz",
        true,
        '{"m":1}',
      ),
    );
    expect(exportSpy).toHaveBeenCalled();
  });

  it("sends bare crash (no conversation) when opted into conversation but no session", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    vi.spyOn(crashApi, "dismissPending").mockResolvedValue();
    // No active session — currentSessionId is null (default).
    const exportSpy = vi.spyOn(crashApi, "exportSessionConversation").mockResolvedValue("[]");
    const submitSpy = vi
      .spyOn(crashApi, "submit")
      .mockResolvedValue({ status: "accepted", crash_id: 3 });

    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());

    // Select "携带 JSON 对话文件", then send.
    await userEvent.click(screen.getByText(/携带 JSON 对话文件/));
    await userEvent.click(screen.getByRole("button", { name: /发送报告/ }));

    // Bare crash is still sent, but without conversation flag.
    await waitFor(() =>
      expect(submitSpy).toHaveBeenCalledWith(
        "https://deepdepcat.hsmiai.xyz",
        false,
        null,
      ),
    );
    expect(exportSpy).not.toHaveBeenCalled();
  });

  it("dismisses and clears the pending crash", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    const dismissSpy = vi.spyOn(crashApi, "dismissPending").mockResolvedValue();
    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());

    await userEvent.click(screen.getByRole("button", { name: /暂不发送/ }));
    await waitFor(() => expect(dismissSpy).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("restores the last session when a crash is pending and one was remembered", async () => {
    localStorage.setItem(
      PREF_LAST_SESSION,
      JSON.stringify({ mode: "code", sessionId: "s1" }),
    );
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    const getSessionSpy = vi
      .spyOn(sessionApi, "getSession")
      .mockResolvedValue({
        id: "s1",
        title: "T",
        model: "deepseek-chat",
        provider: "deepseek",
        status: "idle",
        created_at: "",
        updated_at: "",
        total_usage: { prompt_tokens: 0, completion_tokens: 0 },
        turn_count: 0,
        system_prompt: "",
        work_mode: "code",
      });
    vi.spyOn(sessionApi, "getSessionMessages").mockResolvedValue([]);

    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
    await waitFor(() => expect(getSessionSpy).toHaveBeenCalledWith("s1"));
    expect(screen.getByText(/已恢复上次会话/)).toBeInTheDocument();
    // The restored session is the current one — the tracker re-remembers it
    // (so a later crash can restore it again).
    await waitFor(() =>
      expect(localStorage.getItem(PREF_LAST_SESSION)).toBe(
        JSON.stringify({ mode: "code", sessionId: "s1" }),
      ),
    );
  });

  it("does not restore when no session was remembered", async () => {
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    const getSessionSpy = vi.spyOn(sessionApi, "getSession");

    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());

    expect(getSessionSpy).not.toHaveBeenCalled();
    expect(screen.queryByText(/已恢复上次会话/)).not.toBeInTheDocument();
  });

  it("restore failure does not block the send flow", async () => {
    localStorage.setItem(
      PREF_LAST_SESSION,
      JSON.stringify({ mode: "code", sessionId: "deleted" }),
    );
    vi.spyOn(crashApi, "getPending").mockResolvedValue(pending);
    vi.spyOn(crashApi, "dismissPending").mockResolvedValue();
    vi.spyOn(sessionApi, "getSession").mockRejectedValue(new Error("404"));
    const submitSpy = vi
      .spyOn(crashApi, "submit")
      .mockResolvedValue({ status: "accepted", crash_id: 9 });

    render(<CrashReportDialog />);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());

    // The bare crash can still be sent even though restore failed.
    await userEvent.click(screen.getByRole("button", { name: /发送报告/ }));
    await waitFor(() =>
      expect(submitSpy).toHaveBeenCalledWith(
        "https://deepdepcat.hsmiai.xyz",
        false,
        null,
      ),
    );
    // The deleted session is not retried.
    expect(localStorage.getItem(PREF_LAST_SESSION)).toBeNull();
  });
});
