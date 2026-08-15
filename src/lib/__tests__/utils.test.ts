import { describe, it, expect } from "vitest";
import { cn, timeAgo, shortPath, isEditableKeyEvent, dayGroupLabel } from "@/lib/utils";

function keyEvent(target: EventTarget | null, opts: Partial<KeyboardEvent> = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", opts);
  Object.defineProperty(event, "target", { value: target, configurable: true });
  return event;
}

describe("cn", () => {
  it("merges plain class strings", () => {
    expect(cn("px-2", "py-1")).toBe("px-2 py-1");
  });

  it("resolves Tailwind conflicts (later wins)", () => {
    expect(cn("px-2", "px-4")).toBe("px-4");
  });

  it("handles conditional classes", () => {
    const falsy = false;
    const truthy = true;
    expect(cn("base", falsy && "no", truthy && "yes")).toBe("base yes");
  });

  it("handles undefined and null gracefully", () => {
    expect(cn("base", undefined, null, "end")).toBe("base end");
  });

  it("handles arrays and objects (clsx style)", () => {
    expect(cn(["a", { b: true, c: false }])).toBe("a b");
  });
});

describe("timeAgo", () => {
  it("returns 'Just now' for very recent times", () => {
    const now = Date.now();
    expect(timeAgo(now)).toBe("Just now");
  });

  it("returns seconds format", () => {
    const tenSecondsAgo = Date.now() - 10 * 1000;
    const result = timeAgo(tenSecondsAgo);
    expect(result).toMatch(/^\d+s ago$/);
  });

  it("returns minutes format", () => {
    const twoMinAgo = Date.now() - 2 * 60 * 1000;
    const result = timeAgo(twoMinAgo);
    expect(result).toMatch(/^\d+m ago$/);
  });

  it("returns hours format", () => {
    const threeHoursAgo = Date.now() - 3 * 60 * 60 * 1000;
    const result = timeAgo(threeHoursAgo);
    expect(result).toMatch(/^\d+h ago$/);
  });

  it("accepts string dates", () => {
    const date = new Date(Date.now() - 5 * 1000).toISOString();
    expect(timeAgo(date)).toBe("Just now");
  });
});

describe("dayGroupLabel", () => {
  it("labels the same local day as today", () => {
    const now = new Date(2026, 7, 7, 18, 0).getTime();
    expect(dayGroupLabel(new Date(2026, 7, 7, 1, 0).getTime(), now)).toBe("today");
    expect(dayGroupLabel(now, now)).toBe("today");
  });

  it("labels yesterday across midnight", () => {
    const now = new Date(2026, 7, 7, 9, 0).getTime();
    expect(dayGroupLabel(new Date(2026, 7, 6, 23, 59).getTime(), now)).toBe("yesterday");
  });

  it("labels anything older as earlier", () => {
    const now = new Date(2026, 7, 7, 9, 0).getTime();
    expect(dayGroupLabel(new Date(2026, 7, 5, 0, 0).getTime(), now)).toBe("earlier");
    expect(dayGroupLabel(new Date(2026, 6, 1).getTime(), now)).toBe("earlier");
  });

  it("treats a future timestamp (clock skew) as today", () => {
    const now = new Date(2026, 7, 7, 9, 0).getTime();
    expect(dayGroupLabel(new Date(2026, 7, 8, 0, 0).getTime(), now)).toBe("today");
  });
});

describe("shortPath", () => {
  it("replaces /Users/username with ~", () => {
    expect(shortPath("/Users/hanzi/projects/app")).toBe("~/projects/app");
  });

  it("replaces /home/username with ~", () => {
    expect(shortPath("/home/hanzi/projects/app")).toBe("~/projects/app");
  });

  it("leaves other paths unchanged", () => {
    expect(shortPath("C:/Users/hanzi/app")).toBe("C:/Users/hanzi/app");
  });
});

describe("isEditableKeyEvent", () => {
  it("flags input/textarea/select targets", () => {
    expect(isEditableKeyEvent(keyEvent(document.createElement("input")))).toBe(true);
    expect(isEditableKeyEvent(keyEvent(document.createElement("textarea")))).toBe(true);
    expect(isEditableKeyEvent(keyEvent(document.createElement("select")))).toBe(true);
  });

  it("passes non-editable targets", () => {
    expect(isEditableKeyEvent(keyEvent(document.createElement("button")))).toBe(false);
    expect(isEditableKeyEvent(keyEvent(document.createElement("div")))).toBe(false);
    expect(isEditableKeyEvent(keyEvent(document.body))).toBe(false);
  });

  it("flags contenteditable targets", () => {
    const div = document.createElement("div");
    div.setAttribute("contenteditable", "true");
    expect(isEditableKeyEvent(keyEvent(div))).toBe(true);
    const falseDiv = document.createElement("div");
    falseDiv.setAttribute("contenteditable", "false");
    expect(isEditableKeyEvent(keyEvent(falseDiv))).toBe(false);
  });

  it("flags IME composition", () => {
    expect(
      isEditableKeyEvent(keyEvent(document.body, { isComposing: true })),
    ).toBe(true);
  });

  it("ignores null targets", () => {
    expect(isEditableKeyEvent(keyEvent(null))).toBe(false);
  });
});
