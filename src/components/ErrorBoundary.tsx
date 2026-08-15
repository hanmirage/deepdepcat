/**
 * ErrorBoundary — per-region crash containment.
 *
 * A rendering error in one message/section must never white-screen the
 * whole app (the pre-boundary behavior). The boundary resets when
 * `resetKey` changes (e.g. a new message id / settings category), so the
 * user can keep working without a full reload.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import i18n from "@/i18n";
import { reportClientError } from "@/lib/clientErrorReporter";
import { logError } from "@/lib/logger";

export interface ErrorBoundaryProps {
  children: ReactNode;
  /** Reset the error state when this key changes. */
  resetKey?: string | number | null;
  /** Custom fallback (defaults to a compact inline card with retry). */
  fallback?: (error: Error, retry: () => void) => ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    logError("ErrorBoundary", error, info.componentStack);
    reportClientError("ui_render", error, { stack: info.componentStack ?? undefined });
  }

  componentDidUpdate(prevProps: ErrorBoundaryProps): void {
    if (prevProps.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null });
    }
  }

  private retry = (): void => {
    this.setState({ error: null });
  };

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    if (this.props.fallback) return this.props.fallback(error, this.retry);
    return (
      <div
        role="alert"
        className="m-2 flex items-center justify-between gap-3 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
      >
        <span className="min-w-0 break-words">
          {i18n.t("common.renderError", { message: error.message, defaultValue: `此区域渲染出错：${error.message}` })}
        </span>
        <button
          type="button"
          onClick={this.retry}
          className="shrink-0 rounded border border-destructive/30 px-2 py-1 transition-colors hover:bg-destructive/10"
        >
          {i18n.t("common.retry", { defaultValue: "重试" })}
        </button>
      </div>
    );
  }
}
