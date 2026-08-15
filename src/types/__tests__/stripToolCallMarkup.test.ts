import { describe, expect, it } from "vitest";
import { stripToolCallMarkup } from "@/types/chat";

describe("stripToolCallMarkup", () => {
  it("strips DeepSeek DSML root blocks while keeping surrounding prose", () => {
    const text =
      "E2E 全部通过。 " +
      "<｜DSML｜tool_calls> <｜DSML｜invoke name=\"edit_file\"> " +
      "<｜DSML｜parameter name=\"path\" string=\"true\">admin.html</｜DSML｜parameter> " +
      "</｜DSML｜invoke> </｜DSML｜tool_calls>" +
      " 完成。";
    const cleaned = stripToolCallMarkup(text);
    expect(cleaned).toContain("E2E 全部通过");
    expect(cleaned).toContain("完成");
    expect(cleaned).not.toContain("｜DSML｜");
    expect(cleaned).not.toContain("invoke");
    expect(cleaned).not.toContain("parameter");
  });

  it("strips the legacy function_calls root name", () => {
    const text =
      "<｜DSML｜function_calls> <｜DSML｜invoke name=\"bash\">{ \"command\": \"ls\" }</｜DSML｜invoke> </｜DSML｜function_calls>";
    expect(stripToolCallMarkup(text).trim()).toBe("");
  });

  it("strips the ASCII ||DSML|| variant", () => {
    const text =
      "a <||DSML||tool_calls><||DSML||invoke name=\"x\"/></||DSML||tool_calls> b";
    const cleaned = stripToolCallMarkup(text);
    expect(cleaned).toContain("a");
    expect(cleaned).toContain("b");
    expect(cleaned).not.toContain("DSML");
  });

  it("strips the DOUBLE fullwidth-bar variant emitted by deepseek-v4-flash", () => {
    // Real wire bytes (2026-08-11): each side carries TWO U+FF5C bars.
    const text =
      "柔化阴影。" +
      "<＜＜DSML＞＞tool_calls>\n" +
      "<＜＜DSML＞＞invoke name=\"read_file\">\n" +
      "<＜＜DSML＞＞parameter name=\"path\" string=\"true\">css/style.css</＜＜DSML＞＞parameter>\n" +
      "</＜＜DSML＞＞invoke>\n" +
      "</＜＜DSML＞＞tool_calls>";
    const cleaned = stripToolCallMarkup(text);
    expect(cleaned).toContain("柔化阴影");
    expect(cleaned).not.toContain("DSML");
    expect(cleaned).not.toContain("invoke");
    expect(cleaned).not.toContain("parameter");
  });

  it("strips the Hangzhou-numeral variant", () => {
    const text =
      "前 <〡DSML〡tool_calls><〡DSML〡invoke name=\"read_file\"/></〡DSML〡tool_calls> 后";
    const cleaned = stripToolCallMarkup(text);
    expect(cleaned).toContain("前");
    expect(cleaned).toContain("后");
    expect(cleaned).not.toContain("DSML");
    expect(cleaned).not.toContain("invoke");
  });

  it("matrix: every known DSML delimiter variant strips", () => {
    const variants = [
      "<｜DSML｜tool_calls><｜DSML｜invoke name=\"x\"/></｜DSML｜tool_calls>",
      "<||DSML||tool_calls><||DSML||invoke name=\"x\"/></||DSML||tool_calls>",
      "<〡DSML〡tool_calls><〡DSML〡invoke name=\"x\"/></〡DSML〡tool_calls>",
      "<＜＜DSML＞＞tool_calls><＜＜DSML＞＞invoke name=\"x\"/></＜＜DSML＞＞tool_calls>",
      "<tool_calls><invoke name=\"x\"/></tool_calls>",
    ];
    for (const v of variants) {
      const cleaned = stripToolCallMarkup(`a ${v} b`);
      expect(cleaned).toContain("a");
      expect(cleaned).toContain("b");
      expect(cleaned).not.toContain("DSML");
      expect(cleaned).not.toContain("invoke");
    }
  });

  it("strips orphan DSML fragments and closers", () => {
    expect(stripToolCallMarkup("done </｜DSML｜tool_calls>").trim()).toBe("done");
    expect(stripToolCallMarkup("x <｜DSML｜invoke name=\"y\"/> z")).toContain("x");
    expect(stripToolCallMarkup("x <｜DSML｜invoke name=\"y\"/> z")).not.toContain("invoke");
  });

  it("keeps plain text untouched", () => {
    expect(stripToolCallMarkup("普通正文")).toBe("普通正文");
  });
});
