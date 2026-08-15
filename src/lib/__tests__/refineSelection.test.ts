import { describe, expect, it } from "vitest";
import {
  buildRefineDraft,
  MAX_REFINE_SELECTION_CHARS,
} from "@/lib/refineSelection";

describe("buildRefineDraft", () => {
  it("wraps the selection as a scoped refine request", () => {
    const draft = buildRefineDraft("把这段改成正式语气");
    expect(draft).toContain("对下面这段内容做定点修改");
    expect(draft).toContain("把这段改成正式语气");
    expect(draft.endsWith("\n\n")).toBe(true);
  });

  it("trims surrounding whitespace", () => {
    const draft = buildRefineDraft("   hello  \n");
    expect(draft).toContain("\n\nhello\n\n");
    expect(draft).not.toContain("   hello");
  });

  it("returns empty string for empty selection", () => {
    expect(buildRefineDraft("")).toBe("");
    expect(buildRefineDraft("   ")).toBe("");
  });

  it("caps oversized selections with a truncation marker", () => {
    const long = "x".repeat(MAX_REFINE_SELECTION_CHARS + 500);
    const draft = buildRefineDraft(long);
    expect(draft).toContain("（已截断）");
    expect(draft.length).toBeLessThan(MAX_REFINE_SELECTION_CHARS + 200);
  });
});
