/**
 * ContextChips tests — file chips, image thumbnails, removal.
 */

import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import { ContextChips } from "@/components/chat/ContextChips";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("@/lib/tauri")>("@/lib/tauri");
  return {
    ...actual,
    toAssetUrl: (p: string) => p,
  };
});

function renderChips(chips: Parameters<typeof ContextChips>[0]["chips"], onRemove: () => void) {
  return render(
    <TooltipProvider>
      <ContextChips chips={chips} onRemove={onRemove} />
    </TooltipProvider>,
  );
}

describe("ContextChips", () => {
  it("renders a plain file chip with its name", () => {
    const { container } = renderChips(
      [{ id: "1", type: "file", name: "notes.md", path: "C:/notes.md" }],
      vi.fn(),
    );
    expect(container.textContent).toContain("notes.md");
  });

  it("shows an image thumbnail for picture files", () => {
    const { container } = renderChips(
      [{ id: "1", type: "file", name: "shot.png", path: "C:/shot.png" }],
      vi.fn(),
    );
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img?.getAttribute("src")).toBe("C:/shot.png");
  });

  it("removal button fires onRemove", () => {
    const onRemove = vi.fn();
    const { container } = renderChips(
      [{ id: "x", type: "file", name: "a.txt", path: "C:/a.txt" }],
      onRemove,
    );
    const btn = container.querySelector("button");
    fireEvent.click(btn!);
    expect(onRemove).toHaveBeenCalledWith("x");
  });
});
