/**
 * StreamTokenCounter tests — Claude-style live token estimate.
 */

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { StreamTokenCounter } from "@/components/chat/StreamTokenCounter";
import type { UIMessage } from "@/types";

function msgWithText(len: number): UIMessage {
  return {
    id: "a1",
    role: "assistant",
    blocks: [{ type: "text", content: "x".repeat(len) }],
    timestamp: 0,
  };
}

describe("StreamTokenCounter", () => {
  it("shows nothing before any text arrives", () => {
    const { container } = render(<StreamTokenCounter message={msgWithText(0)} />);
    expect(container.textContent).toBe("");
  });

  it("estimates Latin tokens from text growth (chars / 4)", () => {
    const { container, rerender } = render(<StreamTokenCounter message={msgWithText(0)} />);
    rerender(<StreamTokenCounter message={msgWithText(90)} />);
    // 90 ASCII chars / 4 = 22.5 → 23 tok.
    expect(container.textContent).toContain("23 tok");
  });

  it("accumulates across growth steps", () => {
    const { container, rerender } = render(<StreamTokenCounter message={msgWithText(0)} />);
    rerender(<StreamTokenCounter message={msgWithText(45)} />);
    rerender(<StreamTokenCounter message={msgWithText(120)} />);
    // 120 ASCII chars total / 4 = 30 tok (accumulated, not per-step).
    expect(container.textContent).toContain("30 tok");
  });

  it("counts reasoning blocks toward the estimate", () => {
    const msg: UIMessage = {
      id: "a1",
      role: "assistant",
      blocks: [
        { type: "reasoning", content: "y".repeat(30) },
        { type: "text", content: "x".repeat(30) },
      ],
      timestamp: 0,
    };
    const { container, rerender } = render(
      <StreamTokenCounter message={msgWithText(0)} />,
    );
    rerender(<StreamTokenCounter message={msg} />);
    // 60 ASCII chars / 4 = 15 tok (reasoning + text).
    expect(container.textContent).toContain("15 tok");
  });

  it("weights CJK chars higher than Latin (fewer chars per token)", () => {
    const cjkMsg: UIMessage = {
      id: "a1",
      role: "assistant",
      blocks: [{ type: "text", content: "你".repeat(60) }],
      timestamp: 0,
    };
    const { container, rerender } = render(
      <StreamTokenCounter message={msgWithText(0)} />,
    );
    rerender(<StreamTokenCounter message={cjkMsg} />);
    // 60 CJK chars / 0.7 ≈ 86 tok — vs 15 tok for 60 ASCII (chars / 4),
    // reflecting DeepSeek's 1–2 tokens per hanzi.
    expect(container.textContent).toContain("86 tok");
  });

  it("baselines at mount — counts only growth after mounting", () => {
    const { container, rerender } = render(
      <StreamTokenCounter message={msgWithText(30)} />,
    );
    rerender(<StreamTokenCounter message={msgWithText(60)} />);
    // Mounted with 30 chars already present → only the +30 growth counts:
    // 30 / 4 = 7.5 → 8 tok.
    expect(container.textContent).toContain("8 tok");
  });
});
