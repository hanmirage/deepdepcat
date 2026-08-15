/**
 * App — root React component.
 *
 * Responsibilities:
 * - Initialize system (fetch agent status, system info from backend)
 * - Apply theme on mount
 * - Render the AppShell layout
 * - Register global keyboard shortcuts
 */

import { useEffect } from "react";
import { AppShell } from "@/components/layout/AppShell";
import { OnboardingFlow } from "@/components/onboarding/OnboardingFlow";
import { CrashReportDialog } from "@/components/chat/CrashReportDialog";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useOnboardingStore } from "@/stores/onboardingStore";
import { useTheme } from "@/hooks/useTheme";
import { useAgentStatus } from "@/hooks/useAgentStatus";
import { useDebugEvents } from "@/hooks/useDebugEvents";
import { useTaskEvents } from "@/hooks/useTaskEvents";
import { useTaskSystemNotifications } from "@/hooks/useTaskSystemNotifications";
import { useAgentNotifications } from "@/hooks/useAgentNotifications";
import { useRunningSessions } from "@/hooks/useRunningSessions";
import { useNotificationStore } from "@/stores/useNotificationStore";
import { TooltipProvider } from "@/components/ui/tooltip";
import { newChatInCurrentMode } from "@/lib/newChat";
import { focusChatTextarea } from "@/lib/refineSelection";

export default function App() {
  const completed = useOnboardingStore((s) => s.completed);
  const mode = useAppStore((s) => s.mode);
  const initSystem = useAppStore((s) => s.initSystem);
  const setMode = useAppStore((s) => s.setMode);
  const toggleRightPanel = useRightPanelStore((s) => s.toggle);
  const setDebugMode = useAppStore((s) => s.setDebugMode);
  const debugMode = useAppStore((s) => s.debugMode);
  const openSettings = useAppStore((s) => s.openSettings);

  // Initialize theme + system on mount
  useTheme();
  useAgentStatus();
  useDebugEvents();
  // Live task list (sidebar task section) — task-update / scheduler events.
  useTaskEvents();
  // Task notifications: cross-session center + desktop toasts.
  const subscribeNotifications = useNotificationStore((s) => s.subscribe);
  useTaskSystemNotifications();
  // Agent notifications: background agent completion toasts (with summary).
  useAgentNotifications();
  // Persistence watcher: background main-agent turns (list + completion toasts).
  useRunningSessions();

  useEffect(() => subscribeNotifications(), [subscribeNotifications]);

  useEffect(() => {
    initSystem();
  }, [initSystem]);

  // Global keyboard shortcuts
  useEffect(() => {
    // A modal dialog (aria-modal) owns keyboard focus — global shortcuts that
    // would yank focus away (Cmd+L) must not fire while one is open. Other
    // shortcuts (N/B/1/2/,) stay available: they're app-level, not focus-
    // stealing, and the decision cards don't claim them.
    const isModalOpen = () =>
      !!document.querySelector('[aria-modal="true"]');

    const handler = (e: KeyboardEvent) => {
      // Cmd/Ctrl+N → new conversation in the CURRENT product mode (the
      // sidebar button and this shortcut share one path).
      if ((e.metaKey || e.ctrlKey) && e.key === "n") {
        e.preventDefault();
        newChatInCurrentMode();
      }
      // Cmd/Ctrl+B → toggle right panel
      if ((e.metaKey || e.ctrlKey) && e.key === "b") {
        e.preventDefault();
        toggleRightPanel(mode);
      }
      // Cmd/Ctrl+Shift+D → toggle debug mode
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "D") {
        e.preventDefault();
        setDebugMode(!debugMode);
      }
      // Cmd/Ctrl+1/2 → switch modes
      if ((e.metaKey || e.ctrlKey) && ["1", "2"].includes(e.key)) {
        e.preventDefault();
        const modes = ["code", "depwork"] as const;
        setMode(modes[parseInt(e.key) - 1]);
      }
      // Cmd/Ctrl+L → focus the chat textarea (escape from search/panels
      // back to typing). Ignored while a modal dialog is open — the dialog's
      // own trap owns focus.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l") {
        if (!isModalOpen()) {
          e.preventDefault();
          focusChatTextarea();
        }
      }
      // Cmd/Ctrl+, → open settings (standard app-wide shortcut).
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        openSettings();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [mode, setMode, toggleRightPanel, setDebugMode, debugMode, openSettings]);

  return (
    <TooltipProvider delayDuration={300}>
      {completed ? <AppShell /> : <OnboardingFlow />}
      <CrashReportDialog />
    </TooltipProvider>
  );
}
