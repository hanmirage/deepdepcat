/**
 * ErrorBoundary tests — render-error containment and reset-on-key.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { ErrorBoundary } from "@/components/ErrorBoundary";

function Bomb({ shouldThrow }: { shouldThrow: boolean }) {
  if (shouldThrow) throw new Error("boom");
  return <div>正常内容</div>;
}

describe("ErrorBoundary", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders children when nothing throws", () => {
    render(
      <ErrorBoundary>
        <Bomb shouldThrow={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText("正常内容")).toBeInTheDocument();
  });

  it("catches a render error instead of white-screening", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <ErrorBoundary>
        <Bomb shouldThrow />
      </ErrorBoundary>,
    );
    expect(screen.getByText(/boom/)).toBeInTheDocument();
    expect(screen.queryByText("正常内容")).toBeNull();
    spy.mockRestore();
  });

  it("recovers when the reset key changes", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { rerender } = render(
      <ErrorBoundary resetKey="a">
        <Bomb shouldThrow />
      </ErrorBoundary>,
    );
    expect(screen.getByText(/boom/)).toBeInTheDocument();

    rerender(
      <ErrorBoundary resetKey="b">
        <Bomb shouldThrow={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText("正常内容")).toBeInTheDocument();
    spy.mockRestore();
  });

  it("renders the custom fallback when provided", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <ErrorBoundary fallback={(error, retry) => (
        <div>
          <span>自定义兜底:{error.message}</span>
          <button onClick={retry}>retry-now</button>
        </div>
      )}>
        <Bomb shouldThrow />
      </ErrorBoundary>,
    );
    expect(screen.getByText("自定义兜底:boom")).toBeInTheDocument();
    expect(screen.queryByText(/此区域渲染出错/)).toBeNull();
    spy.mockRestore();
  });
});
