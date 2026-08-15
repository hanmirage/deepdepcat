import { describe, it, expect } from "vitest";
import { mapDragEvent } from "@/lib/tauri";

describe("mapDragEvent", () => {
  it("drop carries the real paths", () => {
    expect(mapDragEvent("drop", ["C:\\a.png", "D:\\b.txt"])).toEqual({
      type: "drop",
      paths: ["C:\\a.png", "D:\\b.txt"],
    });
  });

  it("enter maps to over (hovering state) — NOT leave", () => {
    // The regression: `enter` (file entering the window, paths present) used
    // to fall through to `leave`, hiding the drop overlay the instant a drag
    // began. It must map to `over` so the overlay stays visible.
    expect(mapDragEvent("enter", ["C:\\a.png"])).toEqual({ type: "over" });
  });

  it("over maps to over", () => {
    expect(mapDragEvent("over", [])).toEqual({ type: "over" });
  });

  it("leave maps to leave", () => {
    expect(mapDragEvent("leave", [])).toEqual({ type: "leave" });
  });
});
