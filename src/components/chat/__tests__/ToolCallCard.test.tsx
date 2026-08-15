/**
 * ToolCallCard tests — bare terminal-log tool line (NO container).
 *
 * Covers the visual contract:
 *  - running: spinner + text shimmer on the narrative, live elapsed
 *  - done: muted dot + completed verb + diff badge
 *  - error: red cross icon + failed verb + error summary
 *  - argument highlighting: paths mono/sky, commands mono
 *  - NO badge/chip chrome — the bare row ends in a terminal-prompt `>`
 *  - NO container classes (no border/background/blur/rounded)
 */

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ToolCallCard, liveTail } from "@/components/chat/ToolCallCard";
import type { ToolCallState } from "@/types";

function tool(
  name: string,
  status: ToolCallState["status"],
  args = "",
  result?: string,
): ToolCallState {
  return {
    id: `t-${name}-${status}`,
    name,
    arguments: args,
    status,
    result,
    startedAt: Date.now() - 42_000,
  };
}

function cardClasses(): string[] {
  const btn = screen.getByRole("button");
  return btn.className.split(" ");
}

describe("ToolCallCard — narrative", () => {
  it("bash running: 执行中 shimmer + live target + prompt chevron", () => {
    render(<ToolCallCard tool={tool("bash", "running", JSON.stringify({ command: "npm test" }))} />);

    // The running state word (not the whole narrative) carries the shimmer.
    expect(screen.getByText("执行中").className).toContain("text-shimmer");
    // The target is visible while running ("执行中 npm test"); full args
    // and result stay locked until completion.
    expect(screen.getByText("npm test")).toBeInTheDocument();
    // The bare row ends in a terminal-prompt `>` (expand affordance).
    expect(screen.getByText(">")).toBeInTheDocument();

    // Spinner present.
    expect(document.querySelector(".animate-spin")).not.toBeNull();
  });

  it("bash done: target appears after completion", () => {
    render(
      <ToolCallCard
        tool={tool("bash", "done", JSON.stringify({ command: "npm test" }), "ok")}
      />,
    );
    expect(screen.getByText("已执行")).toBeInTheDocument();
    const arg = screen.getByText("npm test");
    expect(arg.className).toContain("font-mono");
    // Done words no longer shimmer.
    expect(screen.getByText("已执行").className).not.toContain("text-shimmer");
  });

  it("edit_file done: 已编辑 + file target + diff badge + elapsed", () => {
    render(
      <ToolCallCard
        tool={tool(
          "edit_file",
          "done",
          JSON.stringify({
            path: "src/ihrm.html",
            old_text: "a\nb",
            new_text: "a\nc\nz",
          }),
        )}
      />,
    );
    expect(screen.getByText("已编辑")).toBeInTheDocument();
    expect(screen.getByText("ihrm.html")).toBeInTheDocument();
    // +2 -1 diff stats (one replaced line + one added line).
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.getByText("-1")).toBeInTheDocument();
    // Frozen elapsed time (00:42).
    expect(screen.getByText("00:42")).toBeInTheDocument();
  });

  it("read_file running: 读取中 verb + live target", () => {
    render(
      <ToolCallCard tool={tool("read_file", "running", JSON.stringify({ path: "src/main.tsx" }))} />,
    );
    expect(screen.getByText("读取中")).toBeInTheDocument();
    expect(screen.getByText(/main\.tsx/)).toBeInTheDocument();
  });

  it("write tools: bare row shows edit verb + target, no badge chrome", () => {
    const writeTool = tool("edit_file", "running", JSON.stringify({ path: "src/a.ts" }));
    const { rerender } = render(<ToolCallCard tool={writeTool} />);
    expect(screen.getByText("编辑中")).toBeInTheDocument();
    // The bare row never adds a write badge.
    expect(screen.queryByText("写入")).toBeNull();
    rerender(<ToolCallCard tool={{ ...writeTool, status: "done" }} />);
    expect(screen.getByText("已编辑")).toBeInTheDocument();
  });

  it("read tools never carry any badge chrome", () => {
    render(<ToolCallCard tool={tool("read_file", "running")} />);
    expect(screen.queryByText("写入")).toBeNull();
    expect(screen.queryByText("shell")).toBeNull();
  });

  it("read_file done: target appears with its highlight", () => {
    render(
      <ToolCallCard tool={tool("read_file", "done", JSON.stringify({ path: "src/main.tsx" }))} />,
    );
    const arg = screen.getByText(/main\.tsx/);
    expect(arg.className).toContain("font-mono");
    expect(arg.className).toContain("text-sky-600");
  });

  it("grep patterns get the amber highlight", () => {
    render(
      <ToolCallCard tool={tool("grep", "done", JSON.stringify({ pattern: "AuthProvider" }))} />,
    );
    const arg = screen.getByText(/AuthProvider/);
    expect(arg.className).toContain("text-amber-600");
  });

  it("agent running: 派发中 + live task target", () => {
    render(
      <ToolCallCard
        tool={tool("agent", "running", JSON.stringify({ agent_type: "explore", task: "分析项目结构" }))}
      />,
    );
    expect(screen.getByText("派发中")).toBeInTheDocument();
    // No type-label badge — the bare row shows the task as target.
    expect(screen.queryByText("探查")).toBeNull();
    expect(screen.getByText(/分析项目结构/)).toBeInTheDocument();
  });

  it("error: failed verb + failure summary", () => {
    render(
      <ToolCallCard
        tool={tool("bash", "error", JSON.stringify({ command: "ls" }), "command not found: lss")}
      />,
    );
    expect(screen.getByText("执行失败")).toBeInTheDocument();
    expect(document.querySelector(".text-destructive")).not.toBeNull();
    // The error auto-expands, so the message appears in BOTH the row
    // summary and the expanded details.
    expect(screen.getAllByText(/command not found/).length).toBeGreaterThan(0);
  });

  it("no shell badge on file tools", () => {
    render(<ToolCallCard tool={tool("read_file", "done", JSON.stringify({ path: "a.ts" }))} />);
    expect(screen.queryByText("shell")).toBeNull();
  });

  it("bare row — NO container styling (no border/background-box/blur)", () => {
    render(<ToolCallCard tool={tool("read_file", "running", JSON.stringify({ path: "a.ts" }))} />);
    const cls = cardClasses().join(" ");
    expect(cls).not.toContain("border");
    expect(cls).not.toContain("backdrop-blur");
    expect(cls).not.toContain("rounded");
    // Hover affordance is fine; a real background box is not.
    expect(cls).not.toContain("bg-background");
    expect(cls).not.toContain("bg-muted/15");
  });

  it("MCP tools use the shared MCP verb", () => {
    render(<ToolCallCard tool={tool("mcp__charts__dashboard", "running")} />);
    expect(screen.getByText("调用 MCP 中")).toBeInTheDocument();
  });

  it("errors auto-expand the details", () => {
    render(
      <ToolCallCard
        tool={tool("bash", "error", JSON.stringify({ command: "test" }), "line1\nline2\nline3")}
      />,
    );
    // Only the expanded result block contains the second line.
    expect(screen.getByText(/line2/)).toBeInTheDocument();
  });

  it("long results can be expanded in place", () => {
    const longResult = `${"x".repeat(850)}\nTAIL_MARKER`;
    render(<ToolCallCard tool={tool("agent", "done", JSON.stringify({ task: "test" }), longResult)} />);
    expect(screen.queryByText(/TAIL_MARKER/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /工具调用：agent/ }));
    fireEvent.click(screen.getByRole("button", { name: "展开" }));
    expect(screen.getByText(/TAIL_MARKER/)).toBeInTheDocument();
    // Expanded = a definite fixed-height container with inner scroll,
    // never a content-driven stretch.
    expect(document.querySelector("pre")?.className).toContain("h-64");
  });

  it("long command arguments wrap inside the container instead of overflowing", () => {
    const longCommand =
      "node --check 'D:\\测试\\js\\main.js' && echo ok && " +
      "node --check 'D:\\测试\\css\\style.css' && echo TAIL_CMD";
    render(
      <ToolCallCard
        tool={tool("agent", "done", JSON.stringify({ command: longCommand }), "ok")}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /工具调用：agent/ }));

    const dd = document.querySelector<HTMLElement>(`dd[title*="TAIL_CMD"]`);
    expect(dd).not.toBeNull();
    expect(dd?.className).toContain("break-all");
    expect(dd?.className).not.toContain("truncate");
  });

  it("finished tools show real durations past ten minutes", () => {
    render(
      <ToolCallCard
        tool={{
          ...tool("bash", "done", JSON.stringify({ command: "test" }), "ok"),
          startedAt: Date.now() - 15 * 60 * 1000,
        }}
      />,
    );
    expect(screen.getByText("15:00")).toBeInTheDocument();
  });

  it("row aria-label carries the tool status", () => {
    render(<ToolCallCard tool={tool("bash", "running")} />);
    expect(screen.getByRole("button", { name: /— 运行中/ })).toBeInTheDocument();
  });

  it("done tools show a compact result summary in the folded row", () => {
    render(
      <ToolCallCard
        tool={tool("bash", "done", JSON.stringify({ command: "test" }), "deploy complete")}
      />,
    );
    // The folded row carries the outcome; the details block stays unmounted.
    expect(screen.getByText(/deploy complete/)).toBeInTheDocument();
    expect(document.querySelector("dl")).toBeNull();
  });

  it("a long result does NOT leak into the folded row", () => {
    render(
      <ToolCallCard
        tool={tool("bash", "done", JSON.stringify({ command: "test" }), "x".repeat(850))}
      />,
    );
    // A content dump stays behind the chevron — no summary in the row.
    expect(screen.queryByText(/x{10,}/)).toBeNull();
  });

  it("folds back when a running tool completes (settle)", () => {
    const running = tool("agent", "running", JSON.stringify({ task: "test" }));
    const { rerender } = render(<ToolCallCard tool={running} />);
    // User expands it to watch the streaming args.
    fireEvent.click(screen.getByRole("button", { name: /工具调用：agent/ }));
    expect(document.querySelector("dl")).not.toBeNull();
    // Tool completes — the card folds back to the one-line summary.
    rerender(<ToolCallCard tool={{ ...running, status: "done", result: "ok" }} />);
    expect(document.querySelector("dl")).toBeNull();
  });
});

describe("ToolCallCard — MCP Apps", () => {
  it("renders the interactive app in a sandboxed iframe when mcpApp is present", () => {
    render(
      <ToolCallCard
        tool={{
          ...tool("mcp__dashboard", "done", "{}", "dashboard ready"),
          mcpApp: {
            server: "charts",
            resource_uri: "ui://app/dashboard",
            html: "<!DOCTYPE html><html><body><h1>hi</h1></body></html>",
            is_error: false,
          },
        }}
      />,
    );

    const frame = document.querySelector("iframe");
    expect(frame).not.toBeNull();
    expect(frame?.getAttribute("sandbox")).toContain("allow-scripts");
    // Isolation contract: NO same-origin — the app must run in an opaque origin.
    expect(frame?.getAttribute("sandbox")).not.toContain("allow-same-origin");
    expect(frame?.getAttribute("srcdoc")).toContain("<h1>hi</h1>");
    expect(screen.getByText("charts")).toBeInTheDocument();
  });

  it("auto-expands when the app arrives", () => {
    render(
      <ToolCallCard
        tool={{
          ...tool("mcp__pdf", "done", "{}"),
          mcpApp: {
            server: "pdf",
            resource_uri: "ui://app/pdf",
            html: "<html></html>",
            is_error: false,
          },
        }}
      />,
    );
    // The iframe is only rendered inside the expanded content.
    expect(document.querySelector("iframe")).not.toBeNull();
  });

  it("shows no iframe without an app payload", () => {
    render(<ToolCallCard tool={tool("mcp__plain", "done", "{}", "no ui")} />);
    expect(document.querySelector("iframe")).toBeNull();
  });
});

describe("liveTail — stream length cap", () => {
  it("passes short streams through untouched", () => {
    expect(liveTail("ok")).toBe("ok");
    expect(liveTail("x".repeat(800))).toBe("x".repeat(800));
  });

  it("keeps only the tail of long streams with an omitted note", () => {
    const raw = "head-noise\n" + "body".repeat(400);
    const out = liveTail(raw);
    expect(out.length).toBeLessThanOrEqual(800 + 40); // note + tail
    expect(out).toContain("前面省略");
    expect(out.endsWith("bodybodybodybody")).toBe(true);
    expect(out).not.toContain("head-noise");
  });
});

describe("ToolCallCard — tool-family cards", () => {
  it("bash failure: exit-code pill + amber target on the folded row", () => {
    const { container } = render(
      <ToolCallCard
        tool={tool(
          "bash",
          "done",
          JSON.stringify({ command: "npm run build" }),
          "error text\n\nExit code: 1",
        )}
      />,
    );
    // Ran-but-failed: the folded row's target goes amber (status is still done).
    expect(container.querySelector(".text-amber-600")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /工具调用：bash/ }));
    expect(screen.getByText(/退出 1/)).toBeInTheDocument();
    // The command shows in the expanded card (in addition to the row target).
    expect(screen.getAllByText("npm run build").length).toBeGreaterThanOrEqual(1);
  });

  it("bash success shows the exit-0 pill, no failure styling", () => {
    const { container } = render(
      <ToolCallCard
        tool={tool("bash", "done", JSON.stringify({ command: "echo hi" }), "hi\n\nExit code: 0")}
      />,
    );
    expect(container.querySelector(".text-amber-600")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /工具调用：bash/ }));
    expect(screen.getByText(/退出码 0/)).toBeInTheDocument();
  });

  it("grep results group per file with line numbers", () => {
    const result =
      "src/a.ts:10: const x = 1\nsrc/a.ts:20: const y = 2\nsrc/b.ts:5: let z = 3\n\nFound 3 matches in 2 files (searched 10 files)";
    render(
      <ToolCallCard
        tool={tool("grep", "done", JSON.stringify({ pattern: "const" }), result)}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /工具调用：grep/ }));
    expect(screen.getByText("src/a.ts")).toBeInTheDocument();
    expect(screen.getByText("src/b.ts")).toBeInTheDocument();
    expect(screen.getByText("10")).toBeInTheDocument();
  });

  it("a long grep result keeps its hit-count in the folded row", () => {
    const result = `${"m".repeat(500)}\n\nFound 12 matches in 3 files (searched 20 files)`;
    render(
      <ToolCallCard
        tool={tool("grep", "done", JSON.stringify({ pattern: "m" }), result)}
      />,
    );
    expect(screen.getByText(/Found 12 matches in 3 files/)).toBeInTheDocument();
  });
});
