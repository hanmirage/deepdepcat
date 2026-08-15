/**
 * Update + feature-flag slice — release checks, silent staging, force flow.
 */

import { logWarn } from "@/lib/logger";
import { updateApi, featureFlagApi, type UpdateInfo } from "@/lib/tauri";
import type { AppState, StoreGet, StoreSet } from "./types";

export function updateSlice(set: StoreSet, get: StoreGet): Partial<AppState> {
  return {
    // ── Update initial ──────────────────────────────────────────
    updateInfo: null,
    updateChecking: false,
    updateDownloading: false,
    updateProgress: null,
    updateError: null,
    forceUpdate: false,
    updateInstalled: false,
    silentUpdate: { state: "idle", version: null },
    featureFlags: {},

    // ── Actions ────────────────────────────────────────────────
    checkForUpdate: async () => {
      set({ updateChecking: true, updateError: null });
      try {
        const info = await updateApi.checkForUpdate();
        set({ updateChecking: false });
        get().handleUpdateInfo(info);
      } catch (e) {
        set({ updateChecking: false, updateError: String(e) });
      }
    },

    // Route an update: silent releases download themselves in the background
    // (staged for install on exit — zero user interaction); regular releases
    // surface the title-bar download button. A `force` release is mandatory:
    // it must NOT auto-download silently — it renders a blocking update screen
    // so the user can't keep running an unsupported version.
    handleUpdateInfo: (info) => {
      if (!info) {
        // Nothing new (or check failed) — keep any already-staged silent state.
        set({ updateInfo: null });
        return;
      }
      if (info.force) {
        set({ updateInfo: info, forceUpdate: true });
        return;
      }
      if (info.silent) {
        set({ updateInfo: null });
        void get().downloadSilentUpdate();
      } else {
        set({ updateInfo: info });
      }
    },

    // Download + stage a silent update for install on the next app exit.
    downloadSilentUpdate: async () => {
      const st = get().silentUpdate;
      if (st.state !== "idle") return; // already downloading/staged
      set({ silentUpdate: { state: "downloading", version: st.version } });
      try {
        // Watchdog: the backend download has its own stall timeouts now, but
        // belt-and-braces — never leave the UI stuck in "downloading".
        const version = await Promise.race([
          updateApi.downloadSilent(),
          new Promise<null>((resolve) => setTimeout(() => resolve(null), 12 * 60_000)),
        ]);
        if (version) {
          set({ silentUpdate: { state: "staged", version } });
        } else {
          // Nothing to download (check raced / no silent release / watchdog
          // fired) — reset; the periodic check retries.
          set({ silentUpdate: { state: "idle", version: null } });
        }
      } catch (e) {
        logWarn("appStore", "Silent update download failed:", e);
        // Non-fatal: retry soon (30s) instead of waiting for the next hourly
        // check — downloads behind flaky networks benefit from quick retries.
        set({ silentUpdate: { state: "idle", version: null } });
        setTimeout(() => {
          if (get().silentUpdate.state === "idle") {
            void get().downloadSilentUpdate();
          }
        }, 30_000);
      }
    },

    downloadAndInstallUpdate: async () => {
      set({ updateDownloading: true, updateError: null, updateProgress: null });
      try {
        await updateApi.downloadAndInstall();
        // Keep the force dialog visible — the running process is still the old
        // version, so prompt for a restart instead of dropping the user back
        // into an unsupported build.
        set({ updateDownloading: false, updateInstalled: true });
      } catch (e) {
        set({ updateDownloading: false, updateError: String(e) });
      }
    },

    relaunchApp: async () => {
      try {
        await updateApi.relaunch();
      } catch (e) {
        set({ updateError: String(e) });
      }
    },

    clearUpdateError: () => set({ updateError: null }),

    // ── Feature flags ──────────────────────────────────────────
    setFeatureFlag: async (key, enabled) => {
      await featureFlagApi.set(key, enabled);
      set((s) => ({ featureFlags: { ...s.featureFlags, [key]: enabled } }));
    },
  };
}
