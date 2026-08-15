import { describe, it, expect } from "vitest";
import {
  IncrementalSplitter,
  splitBlocks,
  type Block,
} from "@/components/chat/StreamingMarkdown";

/**
 * Feed a content stream in chunks through the incremental splitter and
 * return its final state — must equal a single full `splitBlocks` pass.
 */
function feedInChunks(chunks: string[]): [Block[], Block | null] {
  const splitter = new IncrementalSplitter();
  let content = "";
  let result: [Block[], Block | null] = [[], null];
  for (const chunk of chunks) {
    content += chunk;
    result = splitter.feed(content);
  }
  return result;
}

function expectIncrementalEqualsFull(chunks: string[]) {
  const full = chunks.join("");
  expect(feedInChunks(chunks)).toEqual(splitBlocks(full));
}

describe("IncrementalSplitter", () => {
  it("matches full split for plain paragraph streaming (char by char)", () => {
    const text = "First paragraph with some words.\n\nSecond paragraph here.\n\nThird one.";
    expectIncrementalEqualsFull([...text]);
  });

  it("matches full split for word-chunk streaming", () => {
    const text = "Hello world.\n\nAnother block with more content.\n\nFinal block.";
    expectIncrementalEqualsFull(text.split(" "));
  });

  it("matches full split when a fenced code block streams line by line", () => {
    const text = "Intro line.\n\n```ts\nconst x = 1;\nconsole.log(x);\n```\n\nOutro.";
    expectIncrementalEqualsFull(text.split("\n"));
  });

  it("matches full split when the fence opener arrives in pieces", () => {
    // "```" split across chunks must still open the fence once.
    const pieces = ["Intro.\n\n``", "`ts\ncode", " line\n", "```\n\nOutro."];
    expectIncrementalEqualsFull(pieces);
  });

  it("matches full split for pure code replies (content starts in a fence)", () => {
    const text = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```";
    expectIncrementalEqualsFull([...text]);
  });

  it("matches full split for blank-line-heavy output", () => {
    const text = "a\n\n\nb\n\nc\n\n\n\n\nd";
    expectIncrementalEqualsFull([...text]);
  });

  it("matches full split beyond the active-block cap", () => {
    // Push past MAX_ACTIVE_CHARS (6000) with lines, then a newline split.
    const longLine = "x".repeat(3000);
    const parts = [
      `${longLine}\n${longLine}\n`,
      `\n`,
      "after split\n\nnext block",
    ];
    expectIncrementalEqualsFull(parts);
  });

  it("matches full split for a single unbounded giant line", () => {
    const giant = "y".repeat(7000);
    expectIncrementalEqualsFull([giant.slice(0, 3500), giant.slice(3500)]);
  });

  it("resets when the content is replaced (new turn)", () => {
    const splitter = new IncrementalSplitter();
    splitter.feed("old content that was longer");
    // Replacement: shorter, not a prefix.
    const [blocks, active] = splitter.feed("new");
    expect(blocks).toEqual([]);
    expect(active).toEqual({ text: "new" });
  });

  it("idempotent on repeated feeds of the same content", () => {
    const splitter = new IncrementalSplitter();
    const text = "One block here.\n\nTwo block here.";
    splitter.feed(text);
    const again = splitter.feed(text);
    expect(again).toEqual(splitBlocks(text));
  });

  it("stamps monotonic source offsets as stable keys; the active tail has none", () => {
    // Long paragraphs so the splitter crosses MIN_SPLIT_LENGTH.
    const para = "P".repeat(300);
    const text = `${para}\n\n${para}\n\n${para}`;
    const [blocks, active] = splitBlocks(text);
    // Two completed blocks (the third paragraph stays the active tail).
    expect(blocks.length).toBe(2);
    expect(blocks[0].offset).toBe(0);
    // Offsets grow strictly — each completed block's start never moves, so
    // React reconciles under a stable key instead of remounting.
    expect(blocks[1].offset).toBeGreaterThan(blocks[0].offset!);
    expect(active.offset).toBeUndefined();
  });
});
