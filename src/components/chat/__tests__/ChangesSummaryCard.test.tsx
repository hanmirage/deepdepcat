import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChangesSummaryCard } from "@/components/chat/ChangesSummaryCard";

const changes = [
  { path: "src/app.tsx", oldText: "const a = 1;", newText: "const a = 2;" },
  { path: "src/utils/new.ts", oldText: "", newText: "export const x = 1;" },
];

describe("ChangesSummaryCard", () => {
  it("renders the file list with stats", () => {
    render(<ChangesSummaryCard changes={changes} />);
    // Two-segment paths render in full; longer ones shorten to the tail.
    expect(screen.getByText("src/app.tsx")).toBeTruthy();
    expect(screen.getByText("utils/new.ts")).toBeTruthy();
    // The pure-add file shows +1, the edit shows +1 -1 (stats appear both
    // per-file and in the card header).
    expect(screen.getAllByText("+1").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("-1").length).toBeGreaterThanOrEqual(1);
  });

  it("renders nothing for an empty change list", () => {
    const { container } = render(<ChangesSummaryCard changes={[]} />);
    expect(container.firstChild).toBeNull();
  });

  it("expands a file to show its diff", () => {
    render(<ChangesSummaryCard changes={[changes[0]]} />);
    // Diff content is hidden until the row is expanded.
    expect(screen.queryByText("const a = 2;")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /src\/app.tsx/ }));
    expect(screen.getByText("const a = 2;")).toBeTruthy();
    expect(screen.getByText("const a = 1;")).toBeTruthy();
  });
});
