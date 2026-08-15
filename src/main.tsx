import React from "react";
import ReactDOM from "react-dom/client";
import "./i18n";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { initClientErrorReporter } from "./lib/clientErrorReporter";
// Product fonts — bundled locally (fontsource), offline-safe. Inter carries
// the UI / latin / digits; the CJK fallback chain below still resolves to
// system fonts. JetBrains Mono covers code / filenames / numbers.
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "./index.css";

// Non-fatal client errors (render failures, unhandled rejections) go to the
// anonymous telemetry endpoint — opt-out via Settings → Privacy.
initClientErrorReporter();

// Top-level crash containment: an error thrown while mounting App (store
// hooks, theme, onboarding) must not white-screen the window. Panels and
// messages already have their own boundaries; this is the last resort.
function rootFallback(error: Error, retry: () => void) {
  return (
    <div
      role="alert"
      className="flex h-screen w-screen flex-col items-center justify-center gap-3 bg-background px-6 text-center"
    >
      <p className="text-sm font-medium text-foreground">
        {`应用启动失败：${error.message}`}
      </p>
      <button
        type="button"
        onClick={retry}
        className="rounded-lg border border-border px-4 py-2 text-xs text-foreground transition-colors hover:bg-secondary"
      >
        重试
      </button>
    </div>
  );
}

// Disable the WebView's native right-click context menu in production builds
// (it would otherwise expose "Inspect"/DevTools and look non-native). Dev mode
// keeps the menu so we can still debug.
if (import.meta.env.PROD) {
  window.addEventListener("contextmenu", (e) => e.preventDefault());
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary fallback={rootFallback}>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
