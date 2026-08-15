/**
 * MainPanel — center content area.
 *
 * Switches between two views based on the active mode:
 * - code    → ChatView (AI coding assistant — programming-focused)
 * - depwork → DepworkView (document workspace — document-focused, TBD)
 */

import { useAppStore } from "@/stores/appStore";
import { ChatView } from "@/components/views/ChatView";
import { DepworkView } from "@/components/views/DepworkView";
import { SettingsView } from "@/components/views/SettingsView";
import { ScheduledView } from "@/components/scheduled/ScheduledView";
import { DebugPanel } from "@/components/debug/DebugPanel";

export function MainPanel() {
  const mode = useAppStore((s) => s.mode);
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const scheduledOpen = useAppStore((s) => s.scheduledOpen);
  const debugMode = useAppStore((s) => s.debugMode);

  return (
    <main className="flex flex-1 flex-col overflow-hidden bg-[hsl(var(--card))] m-2 rounded-lg border border-[hsl(var(--border))] shadow-[var(--shadow-paper-md)]">
      <div className="flex flex-1 flex-col overflow-hidden">
        {settingsOpen ? (
          <SettingsView />
        ) : scheduledOpen ? (
          <ScheduledView />
        ) : (
          <>
            {mode === "code" && <ChatView />}
            {mode === "depwork" && <DepworkView />}
          </>
        )}
      </div>
      {debugMode && <DebugPanel />}
    </main>
  );
}
