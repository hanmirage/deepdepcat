/**
 * View slice — mode/sidebar/settings/debug/theme/system state + actions.
 */

import type { AgentStatus, SystemInfo } from "@/types";
import type { AppMode } from "@/config/constants";
import type { SettingsCategory } from "@/config/settings";
import { systemApi } from "@/lib/tauri";
import { loadPref, savePref, PREF_MODE, PREF_SIDEBAR, PREF_SIDEBAR_MANAGED, PREF_THEME, PREF_ACCENT } from "./prefs";
import type { AccentName, AppState, StoreGet, StoreSet } from "./types";

export function viewSlice(set: StoreSet, get: StoreGet): Partial<AppState> {
  return {
    // ── Initial state (view prefs loaded from localStorage) ─────
    mode: loadPref<AppMode>(PREF_MODE, "code"),
    sidebarCollapsed: loadPref<boolean>(PREF_SIDEBAR, false),
    sidebarUserManaged: loadPref<boolean>(PREF_SIDEBAR_MANAGED, false),
    isMaximized: false,
    settingsOpen: false,
    scheduledOpen: false,
    settingsCategory: null,
    debugMode: false,
    theme: loadPref<"light" | "dark">(PREF_THEME, "light"),
    accent: loadPref<AccentName>(PREF_ACCENT, "violet"),
    agentStatus: "idle",
    systemInfo: null,
    activeTaskId: null,

    // ── Actions ────────────────────────────────────────────────
    setMode: (mode) => {
      set({ mode });
      savePref(PREF_MODE, mode);
    },
    toggleSidebar: () =>
      set((s) => {
        const next = !s.sidebarCollapsed;
        savePref(PREF_SIDEBAR, next);
        savePref(PREF_SIDEBAR_MANAGED, true);
        return { sidebarCollapsed: next, sidebarUserManaged: true };
      }),
    setSidebarCollapsed: (collapsed) => {
      set({ sidebarCollapsed: collapsed });
      // Auto-collapse (narrow window) shouldn't override the user's persisted
      // choice — only persist when the user actually drove the change.
      if (get().sidebarUserManaged) {
        savePref(PREF_SIDEBAR, collapsed);
      }
    },
    openSettings: (category) => {
      set({ settingsOpen: true, settingsCategory: category ?? null });
    },
    setSettingsOpen: (open) => set({ settingsOpen: open }),
    openScheduled: () => set({ scheduledOpen: true, settingsOpen: false }),
    setScheduledOpen: (open) => set({ scheduledOpen: open }),
    clearSettingsCategory: () => set({ settingsCategory: null }),
    setActiveTaskId: (id) => set({ activeTaskId: id }),
    setDebugMode: (on) => {
      set({ debugMode: on });
      systemApi.setDebugMode(on).catch(() => {});
    },
    toggleTheme: () => {
      const next = get().theme === "light" ? "dark" : "light";
      set({ theme: next });
      savePref(PREF_THEME, next);
      document.documentElement.classList.toggle("dark", next === "dark");
    },
    setTheme: (theme) => {
      set({ theme });
      savePref(PREF_THEME, theme);
      document.documentElement.classList.toggle("dark", theme === "dark");
    },
    setAccent: (accent) => {
      set({ accent });
      savePref(PREF_ACCENT, accent);
      document.documentElement.dataset.accent = accent;
    },
    setAgentStatus: (status: AgentStatus) => set({ agentStatus: status }),
    setSystemInfo: (info: SystemInfo) => set({ systemInfo: info }),
    setIsMaximized: (v) => set({ isMaximized: v }),
  };
}
