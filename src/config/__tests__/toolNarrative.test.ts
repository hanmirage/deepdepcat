/**
 * toolNarrative tests — the unified tool-card narrative source.
 *
 * Covers:
 *  - verb i18n keys per state (running/done/error) with fallback
 *  - shell badge detection
 *  - subagent type labels
 *  - target extraction per tool family (files, patterns, commands, tasks)
 *  - elapsed formatting
 */

import { describe, it, expect } from "vitest";
import {
  toolVerbKey,
  isShellTool,
  agentTypeLabelKey,
  isCustomSpecialist,
  extractTarget,
  formatElapsedMs,
} from "@/config/toolNarrative";

describe("toolVerbKey", () => {
  it("returns the per-state i18n key for known tools", () => {
    expect(toolVerbKey("edit_file", "running")).toBe("toolCall.verbEditRunning");
    expect(toolVerbKey("edit_file", "done")).toBe("toolCall.verbEditDone");
    expect(toolVerbKey("edit_file", "error")).toBe("toolCall.verbEditError");
  });

  it("maps bash and run_command to the same verb set", () => {
    expect(toolVerbKey("bash", "running")).toBe("toolCall.verbBashRunning");
    expect(toolVerbKey("run_command", "done")).toBe("toolCall.verbBashDone");
  });

  it("falls back to task_manage verbs for unknown tools", () => {
    expect(toolVerbKey("unknown_tool", "running")).toBe("toolCall.verbTaskRunning");
  });

  it("maps family tools to their shared verb set", () => {
    expect(toolVerbKey("apply_patch", "running")).toBe("toolCall.verbPatchRunning");
    expect(toolVerbKey("research_search", "done")).toBe("toolCall.verbResearchDone");
    expect(toolVerbKey("scheduler_create", "error")).toBe("toolCall.verbScheduleError");
    expect(toolVerbKey("dev_browser_open", "running")).toBe("toolCall.verbDevBrowserRunning");
  });

  it("maps dynamic MCP tools to the MCP verb set", () => {
    expect(toolVerbKey("mcp__charts__dashboard", "running")).toBe("toolCall.verbMcpRunning");
    expect(toolVerbKey("mcp__charts__dashboard", "done")).toBe("toolCall.verbMcpDone");
  });
});

describe("isShellTool", () => {
  it("flags bash and run_command", () => {
    expect(isShellTool("bash")).toBe(true);
    expect(isShellTool("run_command")).toBe(true);
    expect(isShellTool("edit_file")).toBe(false);
  });
});

describe("agentTypeLabelKey", () => {
  it("maps built-in types to their label keys", () => {
    expect(agentTypeLabelKey("explore")).toBe("toolCall.agentTypeExplore");
    expect(agentTypeLabelKey("plan")).toBe("toolCall.agentTypePlan");
    expect(agentTypeLabelKey("general")).toBe("toolCall.agentTypeGeneral");
  });

  it("falls back to general for custom types", () => {
    expect(agentTypeLabelKey("code-reviewer")).toBe("toolCall.agentTypeGeneral");
  });
});

describe("isCustomSpecialist", () => {
  it("flags custom specialist agents", () => {
    expect(isCustomSpecialist("市场经理")).toBe(true);
    expect(isCustomSpecialist("PPT 专家")).toBe(true);
    expect(isCustomSpecialist("文档撰写")).toBe(true);
  });

  it("rejects built-in worker types", () => {
    expect(isCustomSpecialist("general")).toBe(false);
    expect(isCustomSpecialist("explore")).toBe(false);
    expect(isCustomSpecialist("plan")).toBe(false);
    expect(isCustomSpecialist("evaluator")).toBe(false);
  });
});

describe("extractTarget", () => {
  it("extracts the file name for file tools", () => {
    expect(extractTarget("edit_file", { path: "src\\dir\\ihrm.html" })).toBe("ihrm.html");
    expect(extractTarget("read_file", { path: "/a/b/main.tsx" })).toBe("main.tsx");
  });

  it("extracts patterns for grep/glob", () => {
    expect(extractTarget("grep", { pattern: "AuthProvider" })).toBe("AuthProvider");
    expect(extractTarget("mcp__charts__dashboard", {})).toBe("dashboard");
    expect(extractTarget("research_search", { query: "LLM agents 2026" })).toBe("LLM agents 2026");
    expect(extractTarget("dev_browser_open", { url: "https://example.com/report" })).toBe("report");
  });

  it("extracts and clips shell commands", () => {
    const long = "npm run build -- --watch --mode production".repeat(2);
    const target = extractTarget("bash", { command: long });
    expect(target).not.toBeNull();
    expect(target!.length).toBeLessThanOrEqual(49);
  });

  it("extracts URLs and queries", () => {
    expect(extractTarget("web_fetch", { url: "https://x.dev/a/b" })).toBe("b");
    expect(extractTarget("web_search", { query: "deepseek pricing" })).toBe("deepseek pricing");
  });

  it("extracts and clips subagent tasks", () => {
    expect(extractTarget("agent", { task: "分析项目结构" })).toBe("分析项目结构");
    const longTask = "任务".repeat(30);
    expect(extractTarget("agent", { task: longTask })!.length).toBeLessThanOrEqual(45);
  });

  it("strips evaluator template boilerplate from agent tasks", () => {
    const evaluatorTask =
      "Independently review the work done for the following task.\n\n" +
      "## Task\n自检测一下你的各项功能正常不\n\n" +
      "## Generator's changes\n- D:\\测试\\.ddc_selftest_tmp.txt\n\n" +
      "Verify every acceptance criterion against the actual code and a real run " +
      "(tests/build/LSP diagnostics). Report per the evaluator contract. " +
      "Do NOT modify any files.";
    expect(extractTarget("agent", { task: evaluatorTask })).toBe(
      "自检测一下你的各项功能正常不",
    );
  });

  it("falls back to prefix clipping when the task has no template", () => {
    const plain = "修复登录模块超时问题，同时补充单元测试".repeat(3);
    const target = extractTarget("agent", { task: plain })!;
    expect(target.length).toBeLessThanOrEqual(45);
    expect(target.startsWith("修复登录模块超时问题")).toBe(true);
  });

  it("returns null when no target / unknown tool", () => {
    expect(extractTarget("read_file", {})).toBeNull();
    expect(extractTarget("mystery", { a: 1 })).toBeNull();
  });
});

describe("formatElapsedMs", () => {
  it("formats mm:ss and h:mm:ss", () => {
    expect(formatElapsedMs(42_000)).toBe("00:42");
    expect(formatElapsedMs(3_600_000 + 61_000)).toBe("1:01:01");
    expect(formatElapsedMs(0)).toBe("00:00");
  });
});
