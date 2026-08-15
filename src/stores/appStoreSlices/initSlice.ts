/**
 * Init slice — one-time startup orchestration (system info, workspace
 * restore, model loading, update event wiring).
 */

import { logWarn } from "@/lib/logger";
import {
  systemApi,
  onEvent,
  isTauri,
  updateApi,
  featureFlagApi,
  type UpdateInfo,
  type UpdateProgress,
} from "@/lib/tauri";
import { startSessionTracking } from "@/lib/sessionTracker";
import { useSettingsStore } from "@/stores/settingsStore";
import { useAuthStore } from "@/stores/authStore";
import { compareVersions, loadPref, PREF_WORKSPACE } from "./prefs";
import type { AppState, StoreGet, StoreSet } from "./types";

async function initSystemInfo(set: StoreSet) {
  try {
    const agentStatus = await systemApi.getAgentStatus();
    const info = await systemApi.getSystemInfo();
    set({ agentStatus, systemInfo: info });
  } catch (e) {
    logWarn("initSystem", "Running outside Tauri or error:", e);
  }

  // Initialize settings store so provider config + models are available
  // before ChatView calls loadModels().
  await useSettingsStore.getState().init();
}

async function restoreWorkspace(set: StoreSet, get: StoreGet) {
  // Restore the persisted workspace (survives restarts). The store's initial
  // state already read it from localStorage, so a naive comparison would
  // always be equal and never fire — the backend workspace is in-memory only
  // (state.workspace), so re-push it on every startup and refresh the files.
  const persistedWs = loadPref<string | null>(PREF_WORKSPACE, null);
  if (!persistedWs) return;
  // Await the backend grant (fs scope authorization happens inside
  // set_workspace) so the readDir in refreshWorkspaceFiles doesn't race it.
  await systemApi.setWorkspace(persistedWs).catch(() => {});
  await get().refreshWorkspaceFiles();
}

async function loadModelsAndAuth() {
  // Load available models into both chat stores so the input bar's model
  // selector has data. Errors are non-fatal (falls back to backend models).
  // Dynamic imports: appStore sits at the top of the module graph and the
  // chat stores depend on it — a static edge would create a circular
  // import (appStore → chatStore → appStore), which TDZ-breaks module
  // initialization (depworkChatStore's factory runs before chatStore's
  // module body executes).
  await Promise.allSettled([
    import("@/stores/chatStore").then((m) => m.useChatStore.getState().loadModels()),
    import("@/stores/depworkChatStore").then((m) => m.useDepworkChatStore.getState().loadModels()),
  ]);

  // Initialize auth store — verifies persisted token on startup.
  useAuthStore.getState().init().catch(() => {});
}

async function loadFeatureFlags(set: StoreSet) {
  try {
    const flags = await featureFlagApi.list();
    const map: Record<string, boolean> = {};
    for (const f of flags) map[f.key] = f.enabled;
    set({ featureFlags: map });
  } catch {
    // Non-fatal — flags default to enabled in the backend.
  }
}

function subscribeUpdateEvents(set: StoreSet, get: StoreGet) {
  // Subscribe to update progress events.
  onEvent<UpdateProgress>("update-progress", (progress) => {
    set({ updateProgress: progress });
    if (progress.phase === "error") {
      set({ updateError: progress.message, updateDownloading: false });
    }
  });

  // Periodic backend check found a new release → route it (silent updates
  // auto-download; regular ones show the title-bar download button).
  onEvent<UpdateInfo>("update-available", (info) => {
    get().handleUpdateInfo(info);
  });
}

function checkPendingSilent(set: StoreSet, get: StoreGet) {
  // If a silent update was staged on a previous exit but the app launched
  // at a NEWER version than the staged one, the backend already cleaned it
  // up at startup — just refresh the local state to match.
  if (!isTauri) return;
  updateApi
    .hasPendingSilent()
    .then((version) => {
      if (!version) return;
      const sys = get().systemInfo;
      const current = sys?.app_version ?? "";
      const stagedNewer = current && version && compareVersions(version, current) > 0;
      set({
        silentUpdate: stagedNewer
          ? { state: "staged", version }
          : { state: "idle", version: null },
      });
    })
    .catch(() => {});
}

export function initSlice(set: StoreSet, get: StoreGet): Partial<AppState> {
  return {
    initSystem: async () => {
      await initSystemInfo(set);
      await restoreWorkspace(set, get);
      await loadModelsAndAuth();
      await loadFeatureFlags(set);
      subscribeUpdateEvents(set, get);
      checkPendingSilent(set, get);

      // Silently check for updates on startup (offline/error are non-fatal).
      get().checkForUpdate().catch(() => {});

      // Remember the last active session so a crash can restore it (idempotent —
      // the crash dialog also registers it defensively at mount).
      await startSessionTracking();

      // NOTE: agent-status-changed events are handled by the useAgentStatus hook.
      // Do NOT subscribe here — it would create a duplicate listener with no cleanup.
    },
  };
}
