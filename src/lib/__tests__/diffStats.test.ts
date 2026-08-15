/**
 * diffStats tests — live diff counters for streaming write tools.
 */

import { describe, it, expect } from "vitest";
import { computeDiffStats } from "@/lib/diffStats";

describe("computeDiffStats", () => {
  it("write_file: counts lines of the streamed content (live growth)", () => {
    const partial = computeDiffStats("write_file", JSON.stringify({ path: "a.txt", content: "line1\nline2\n" }));
    expect(partial).toEqual({ added: 3, removed: 0 });
    const complete = computeDiffStats(
      "write_file",
      JSON.stringify({ path: "a.txt", content: "line1\nline2\nline3\n" }),
    );
    expect(complete).toEqual({ added: 4, removed: 0 });
  });

  it("write_file: null before any content arrives", () => {
    expect(computeDiffStats("write_file", JSON.stringify({ path: "a.txt" }))).toBeNull();
  });

  it("edit_file: null until both texts are present", () => {
    expect(
      computeDiffStats("edit_file", JSON.stringify({ path: "a.txt", old_text: "old" })),
    ).toBeNull();
  });

  it("edit_file: computes even with partial new_text (streaming)", () => {
    const stats = computeDiffStats(
      "edit_file",
      JSON.stringify({ path: "a.txt", old_text: "a\nb\nc", new_text: "a\nX\n" }),
    );
    expect(stats).not.toBeNull();
    expect(stats!.added).toBeGreaterThan(0);
  });

  it("edit_file: exact counts on complete texts", () => {
    const stats = computeDiffStats(
      "edit_file",
      JSON.stringify({ path: "a.txt", old_text: "keep\nremove\nkeep", new_text: "keep\nkeep\nnewline" }),
    );
    // keep→keep same; remove→keep +1/-1; keep→newline +1/-1.
    expect(stats).toEqual({ added: 2, removed: 2 });
  });

  it("non-write tools return null", () => {
    expect(computeDiffStats("bash", JSON.stringify({ command: "ls" }))).toBeNull();
    expect(computeDiffStats("read_file", JSON.stringify({ path: "a.txt" }))).toBeNull();
  });

  it("unparseable arguments return null", () => {
    expect(computeDiffStats("write_file", "not-json{")).toBeNull();
  });
});
