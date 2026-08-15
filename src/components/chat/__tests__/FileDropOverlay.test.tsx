/**
 * FileDropOverlay lifecycle tests — the native window drag-drop listener
 * must survive mount (it was previously unsubscribed the moment onFileDrop
 * resolved) and be torn down on unmount.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, act } from "@testing-library/react";
import { FileDropOverlay } from "@/components/chat/FileDropOverlay";
import { onFileDrop } from "@/lib/tauri";

vi.mock("@/lib/tauri", () => ({
  onEvent: vi.fn(() => Promise.resolve(() => {})),
  onFileDrop: vi.fn(() => Promise.resolve(() => {})),
}));

beforeEach(() => {
  (onFileDrop as unknown as ReturnType<typeof vi.fn>).mockClear();
});

describe("FileDropOverlay", () => {
  it("keeps the native drop listener alive after mount", async () => {
    const unlisten = vi.fn();
    (onFileDrop as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce(unlisten);
    const { unmount } = render(<FileDropOverlay mode="code" />);
    await act(async () => {});
    expect(unlisten).not.toHaveBeenCalled();
    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
