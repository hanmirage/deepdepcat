/**
 * conversationConvert tests — restoring backend ConversationItem[] into
 * UIMessage[].
 *
 * The backend splits one agent turn into multiple assistant items (one per
 * tool loop). The converter must merge them back into a single UIMessage —
 * exactly what the streaming path shows — so a restored turn doesn't render
 * as many separate message rows (each with its own action bar).
 */

import { describe, it, expect } from "vitest";
import { conversationItemsToUIMessages } from "@/lib/conversationConvert";

describe("conversationItemsToUIMessages", () => {
  it("merges consecutive assistant items of one turn into a single message", () => {
    const items = [
      { role: "user", content: [{ text: "帮我改文件" }] },
      // Tool loop 1 — narrative + a tool call.
      { role: "assistant", content: "先看一下", tool_calls: [{ id: "c1", name: "read_file", arguments: "{}" }] },
      { role: "tool_result", tool_call_id: "c1", content: "file content", is_error: false },
      // Tool loop 2 — narrative + another tool call.
      { role: "assistant", content: "改一下", tool_calls: [{ id: "c2", name: "edit_file", arguments: "{}" }] },
      { role: "tool_result", tool_call_id: "c2", content: "ok", is_error: false },
      // Final narrative, no tools.
      { role: "assistant", content: "改完了" },
    ];

    const messages = conversationItemsToUIMessages(items);

    // user + ONE merged assistant message — not three.
    expect(messages).toHaveLength(2);
    expect(messages[0].role).toBe("user");
    expect(messages[1].role).toBe("assistant");

    const blocks = messages[1].blocks;
    const textBlocks = blocks.filter((b) => b.type === "text");
    const toolBlocks = blocks.filter((b) => b.type === "tool_call");
    expect(textBlocks.map((b) => b.content)).toEqual(["先看一下", "改一下", "改完了"]);
    expect(toolBlocks).toHaveLength(2);

    // Tool results attach to the matching tool_call within the merged message.
    const c1 = toolBlocks.find((b) => b.tool.id === "c1");
    const c2 = toolBlocks.find((b) => b.tool.id === "c2");
    expect(c1?.tool.result).toBe("file content");
    expect(c2?.tool.result).toBe("ok");
  });

  it("a user message closes the previous turn", () => {
    const items = [
      { role: "user", content: [{ text: "Q1" }] },
      { role: "assistant", content: "A1" },
      { role: "user", content: [{ text: "Q2" }] },
      { role: "assistant", content: "A2" },
    ];

    const messages = conversationItemsToUIMessages(items);

    expect(messages).toHaveLength(4);
    expect(messages.map((m) => m.role)).toEqual(["user", "assistant", "user", "assistant"]);
    const texts = messages
      .filter((m) => m.role === "assistant")
      .map((m) => m.blocks.filter((b) => b.type === "text").map((b) => b.content).join(""));
    expect(texts).toEqual(["A1", "A2"]);
  });

  it("drops system/reasoning items", () => {
    const items = [
      { role: "system", content: "sys" },
      { role: "user", content: [{ text: "hi" }] },
      { role: "reasoning", content: "thinking..." },
      { role: "assistant", content: "hello" },
    ];

    const messages = conversationItemsToUIMessages(items);
    expect(messages.map((m) => m.role)).toEqual(["user", "assistant"]);
  });
});
