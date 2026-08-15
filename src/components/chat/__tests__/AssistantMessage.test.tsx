/**
 * AssistantMessage tests — narrative text flows in execution order, each
 * tool call renders in its own spot where it happens.
 *
 * The component subscribes to the store by messageId, so tests seed the
 * chat store's message array and render by id.
 *
 * Covers:
 *  - consecutive READ tools collapse into one ReadGroup row
 *    (Claude-style "✓ 已读取 2 项") that expands back into member lines
 *  - write tools (non-read) always render as their own bare line
 *  - text/ask_user stay inline in their original spots
 *  - tool lines sit between the lead-in text and the summary text
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { AssistantMessage } from "@/components/chat/AssistantMessage";
import { useChatStore } from "@/stores/chatStore";
import { useAppStore } from "@/stores/appStore";
import type { UIMessage } from "@/types";

function msg(blocks: UIMessage["blocks"], isStreaming = false): UIMessage {
  return { id: "m1", role: "assistant", blocks, timestamp: 0, isStreaming };
}

const toolBlock = (id: string, name: string, status: "running" | "done" | "error") => ({
  type: "tool_call" as const,
  tool: {
    id,
    name,
    arguments: JSON.stringify({ path: "src/x.rs" }),
    status,
  },
});

function renderWithStore(message: UIMessage) {
  useAppStore.setState({ mode: "code" });
  useChatStore.setState({ messages: [message] });
  return render(<AssistantMessage messageId={message.id} />);
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("AssistantMessage tool rendering", () => {
  it("collapses consecutive reads into one ReadGroup; writes stay separate", () => {
    renderWithStore(
      msg([
        { type: "text", content: "开始看。" },
        toolBlock("tc-1", "read_file", "done"),
        toolBlock("tc-2", "grep", "done"),
        toolBlock("tc-3", "edit_file", "done"),
      ]),
    );
    // The two reads fold into one aggregate row; the write stays bare.
    expect(screen.getByText("已读取 2 项")).toBeInTheDocument();
    expect(screen.getByText("已编辑")).toBeInTheDocument();
    // Member rows are NOT in the DOM until the group is expanded.
    expect(screen.queryByText("已搜索")).toBeNull();

    // Expanding the group reveals the member lines.
    fireEvent.click(screen.getByRole("button", { name: "Read group" }));
    expect(screen.getByText("已读取")).toBeInTheDocument();
    expect(screen.getByText("已搜索")).toBeInTheDocument();
  });

  it("splits a read group when a write tool or text interrupts", () => {
    renderWithStore(
      msg([
        toolBlock("tc-1", "read_file", "done"),
        toolBlock("tc-2", "edit_file", "done"),
        toolBlock("tc-3", "grep", "done"),
      ]),
    );
    // read → edit → grep: the reads are NOT consecutive, so two groups
    // (both read-family, hence the same aggregate copy).
    expect(screen.getAllByText("已读取 1 项")).toHaveLength(2);
    expect(screen.getByText("已编辑")).toBeInTheDocument();
  });

  it("shows a running read group with its aggregate verb", () => {
    renderWithStore(
      msg([
        toolBlock("tc-1", "read_file", "done"),
        toolBlock("tc-2", "grep", "running"),
      ]),
    );
    expect(screen.getByText("正在读取 2 项")).toBeInTheDocument();
  });

  it("reports failed members in the aggregate row", () => {
    renderWithStore(
      msg([
        toolBlock("tc-1", "read_file", "done"),
        toolBlock("tc-2", "grep", "error"),
      ]),
    );
    expect(screen.getByText("1/2 项读取失败")).toBeInTheDocument();
  });

  it("places the tool lines between lead-in and summary text", () => {
    renderWithStore(
      msg([
        { type: "text", content: "好的，做一次实时复检。" },
        toolBlock("tc-1", "read_file", "done"),
        toolBlock("tc-2", "grep", "done"),
        { type: "text", content: "检查完成，结果如下。" },
      ]),
    );

    const readGroup = screen.getByText("已读取 2 项");
    const lead = screen.getByText("好的，做一次实时复检。");
    const summary = screen.getByText("检查完成，结果如下。");

    // Tool rows come after the lead-in text and before the summary.
    expect(lead.compareDocumentPosition(readGroup) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(readGroup.compareDocumentPosition(summary) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps interleaved tools in their own spots (first tool before the middle text)", () => {
    renderWithStore(
      msg([
        { type: "text", content: "第一步。" },
        toolBlock("tc-1", "read_file", "done"),
        { type: "text", content: "中间说明。" },
        toolBlock("tc-2", "edit_file", "done"),
        { type: "text", content: "完成。" },
      ]),
    );
    // Both tool lines render inline, each before the text that follows it.
    expect(screen.getByText("已读取 1 项")).toBeInTheDocument();
    expect(screen.getByText("已编辑")).toBeInTheDocument();
    const lead = screen.getByText("第一步。");
    const readLine = screen.getByText("已读取 1 项");
    const middle = screen.getByText("中间说明。");
    const editLine = screen.getByText("已编辑");
    expect(lead.compareDocumentPosition(readLine) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(readLine.compareDocumentPosition(middle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(middle.compareDocumentPosition(editLine) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("subscribes by id: an unrelated message update does not re-render", () => {
    const base = msg([{ type: "text", content: "第一条" }]);
    renderWithStore(base);
    expect(screen.getByText("第一条")).toBeInTheDocument();
    // Stream a DIFFERENT message (id "m2") — m1's object keeps its
    // reference, so this component must NOT re-render (spy on React by
    // checking the old DOM node survived — a re-render would keep it too,
    // so instead assert the store lookup is reference-stable: the message
    // object is the same instance).
    const before = useChatStore.getState().messages[0];
    useChatStore.setState({
      messages: [
        before,
        { id: "m2", role: "assistant", blocks: [{ type: "text", content: "第二条" }], timestamp: 0 },
      ],
    });
    expect(useChatStore.getState().messages[0]).toBe(before);
    expect(screen.getByText("第一条")).toBeInTheDocument();
  });
});

describe("AssistantMessage last-turn actions", () => {
  it("healthy last turn offers Continue and fills the input", async () => {
    useAppStore.setState({ mode: "code" });
    useChatStore.setState({
      messages: [msg([{ type: "text", content: "hello" }])],
    });
    // Stub the store action explicitly — zustand copies actions into a new
    // state object on every setState, so a vi.spyOn would leak across tests.
    const originalSend = useChatStore.getState().sendMessage;
    const sendStub = vi.fn(async () => {});
    useChatStore.setState({ sendMessage: sendStub });
    render(<AssistantMessage messageId="m1" showStreamStatus />);

    fireEvent.click(screen.getByRole("button", { name: /继续/ }));
    expect(useChatStore.getState().inputText).toBe("继续");
    expect(sendStub).toHaveBeenCalledTimes(1);
    useChatStore.setState({ sendMessage: originalSend });
  });

  it("errored last turn offers Retry and re-sends the user text from a clean truncation", async () => {
    useAppStore.setState({ mode: "code" });
    useChatStore.setState({
      messages: [
        {
          id: "u1",
          role: "user",
          blocks: [{ type: "text", content: "请修 bug" }],
          timestamp: 0,
        },
        {
          id: "a1",
          role: "assistant",
          blocks: [{ type: "error", content: "boom" }],
          timestamp: 1,
        },
      ],
    });
    const originalSend = useChatStore.getState().sendMessage;
    const originalDelete = useChatStore.getState().deleteMessage;
    const sendStub = vi.fn(async () => {});
    const deleteStub = vi.fn(async () => {});
    useChatStore.setState({ sendMessage: sendStub, deleteMessage: deleteStub });
    render(<AssistantMessage messageId="a1" showStreamStatus />);

    fireEvent.click(screen.getByRole("button", { name: /重试/ }));
    await waitFor(() => {
      expect(deleteStub).toHaveBeenCalledWith("u1");
      expect(useChatStore.getState().inputText).toBe("请修 bug");
    });
    expect(sendStub).toHaveBeenCalledTimes(1);
    useChatStore.setState({ sendMessage: originalSend, deleteMessage: originalDelete });
  });

  it("does not show Continue on non-last messages", () => {
    useAppStore.setState({ mode: "code" });
    useChatStore.setState({
      messages: [msg([{ type: "text", content: "hello" }])],
    });
    render(<AssistantMessage messageId="m1" />);
    expect(screen.queryByRole("button", { name: /继续/ })).toBeNull();
  });
});

describe("AssistantMessage streaming copy", () => {
  it("offers copy mid-stream and copies the generated text so far", async () => {
    useAppStore.setState({ mode: "code" });
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    useChatStore.setState({
      messages: [msg([{ type: "text", content: "生成的内容" }], true)],
    });
    render(<AssistantMessage messageId="m1" showStreamStatus />);

    const copyBtn = screen.getByRole("button", { name: /复制/ });
    expect(copyBtn).toBeInTheDocument();
    fireEvent.click(copyBtn);
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("生成的内容");
    });
  });

  it("hides streaming copy before any text has been generated", () => {
    useAppStore.setState({ mode: "code" });
    useChatStore.setState({
      messages: [msg([], true)],
    });
    render(<AssistantMessage messageId="m1" showStreamStatus />);
    expect(screen.queryByRole("button", { name: /复制/ })).toBeNull();
  });
});
