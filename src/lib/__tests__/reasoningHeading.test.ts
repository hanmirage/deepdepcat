import { describe, it, expect } from "vitest";
import { reasoningHeading } from "@/lib/reasoningHeading";

describe("reasoningHeading", () => {
  it("extracts an ATX heading", () => {
    expect(reasoningHeading("## 分析需求\n一些思考")).toBe("分析需求");
  });

  it("extracts a setext heading", () => {
    expect(reasoningHeading("设计思路\n========\n细节")).toBe("设计思路");
  });

  it("extracts a bold-line heading", () => {
    expect(reasoningHeading("**结论**\n内容")).toBe("结论");
  });

  it("extracts an HTML heading", () => {
    expect(reasoningHeading("<h2>实现方案</h2>\n内容")).toBe("实现方案");
  });

  it("cleans inline markdown from the heading", () => {
    expect(reasoningHeading("## 检查 `fn` 与 [链接](https://x)")).toBe("检查 fn 与 链接");
  });

  it("returns null when no heading exists", () => {
    expect(reasoningHeading("纯文本思考")).toBeNull();
    expect(reasoningHeading("")).toBeNull();
  });
});
