import { describe, it, expect } from "vitest";
import { detectMode, normalizeInteractionMode } from "@/stores/chatStore/modeDetect";

describe("detectMode (Depwork)", () => {
  it("maps research intents to 接受编辑", () => {
    for (const text of [
      "调研一下竞品的定价",
      "找几篇文献做综述",
      "查一下市场分析资料",
      "research this topic",
    ]) {
      expect(detectMode("depwork", text)).toBe("accept_edits");
    }
  });

  it("maps content-creation intents to 接受编辑", () => {
    for (const text of [
      "写一篇小红书文案",
      "做一个产品发布 PPT",
      "写视频脚本",
      "generate a deck",
    ]) {
      expect(detectMode("depwork", text)).toBe("accept_edits");
    }
  });

  it("keeps plain chat in 只读", () => {
    expect(detectMode("depwork", "你好")).toBe("read_only");
    expect(detectMode("depwork", "今天天气怎么样")).toBe("read_only");
  });
});

describe("detectMode (Code)", () => {
  it("drops read-only exploration into read_only", () => {
    for (const text of ["看看这个项目", "分析一下架构", "review 一下代码"]) {
      expect(detectMode("code", text)).toBe("read_only");
    }
  });

  it("keeps work intents on accept_edits", () => {
    expect(detectMode("code", "实现登录功能")).toBe("accept_edits");
    expect(detectMode("code", "你好")).toBe("accept_edits");
  });
});

describe("normalizeInteractionMode", () => {
  it("passes current modes through unchanged", () => {
    expect(normalizeInteractionMode("read_only")).toBe("read_only");
    expect(normalizeInteractionMode("accept_edits")).toBe("accept_edits");
    expect(normalizeInteractionMode("full_access")).toBe("full_access");
  });

  it("migrates legacy read-only values", () => {
    expect(normalizeInteractionMode("plan")).toBe("read_only");
    expect(normalizeInteractionMode("chat_only")).toBe("read_only");
  });

  it("migrates legacy ask/default values to accept_edits", () => {
    expect(normalizeInteractionMode("confirm")).toBe("accept_edits");
    expect(normalizeInteractionMode("manual")).toBe("accept_edits");
    expect(normalizeInteractionMode("default")).toBe("accept_edits");
  });

  it("migrates legacy full-access values", () => {
    expect(normalizeInteractionMode("auto")).toBe("full_access");
    expect(normalizeInteractionMode("bypass")).toBe("full_access");
  });

  it("falls back to accept_edits for anything unrecognized (including null)", () => {
    expect(normalizeInteractionMode("garbage")).toBe("accept_edits");
    expect(normalizeInteractionMode(null)).toBe("accept_edits");
    expect(normalizeInteractionMode(undefined)).toBe("accept_edits");
  });
});
