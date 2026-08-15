/**
 * Workspace slice — project selection + file listing state/actions.
 */

import { logError } from "@/lib/logger";
import { systemApi, pickFolder, listWorkspaceFiles } from "@/lib/tauri";
import { loadPref, savePref, PREF_WORKSPACE, PREF_WORKSPACES } from "./prefs";
import type { AppState, StoreGet, StoreSet } from "./types";

export function workspaceSlice(set: StoreSet, get: StoreGet): Partial<AppState> {
  return {
    // ── Workspace initial ──────────────────────────────────────
    workspacePath: loadPref<string | null>(PREF_WORKSPACE, null),
    workspaces: loadPref<string[]>(PREF_WORKSPACES, []),
    workspaceFiles: [],
    workspaceLoading: false,

    // ── Actions ────────────────────────────────────────────────
    openWorkspaceDialog: async () => {
      const path = await pickFolder();
      if (!path) return;
      // Update the backend workspace so the agent operates in this directory.
      // A failure here still keeps the frontend workspace (best-effort sync),
      // but must not surface an unhandled rejection.
      try {
        await systemApi.setWorkspace(path);
      } catch (e) {
        logError("openWorkspaceDialog", "Failed to set backend workspace:", e);
      }
      set({ workspacePath: path, workspaceLoading: true });
      savePref(PREF_WORKSPACE, path);
      // Register in the multi-workspace list (dedup, newest first).
      const workspaces = [path, ...get().workspaces.filter((w) => w !== path)];
      set({ workspaces });
      savePref(PREF_WORKSPACES, workspaces);
      try {
        const files = await listWorkspaceFiles(path);
        set({ workspaceFiles: files, workspaceLoading: false });
      } catch (e) {
        logError("openWorkspaceDialog", "Failed to list files:", e);
        set({ workspaceFiles: [], workspaceLoading: false });
      }
    },

    setWorkspacePath: (path) => {
      set({ workspacePath: path });
      savePref(PREF_WORKSPACE, path);
      // Keep the backend in sync when the workspace is cleared.
      systemApi.setWorkspace(path).catch(() => {});
    },

    selectWorkspace: async (path) => {
      const workspaces = get().workspaces;
      const known = workspaces.includes(path)
        ? workspaces
        : [path, ...workspaces];
      if (!workspaces.includes(path)) {
        set({ workspaces: known });
        savePref(PREF_WORKSPACES, known);
      }
      // Update the backend workspace; a failure keeps the frontend workspace
      // (best-effort sync) but must not surface an unhandled rejection —
      // callers invoke this via `void`.
      try {
        await systemApi.setWorkspace(path);
      } catch (e) {
        logError("selectWorkspace", "Failed to set backend workspace:", e);
      }
      set({ workspacePath: path, workspaceLoading: true });
      savePref(PREF_WORKSPACE, path);
      try {
        const files = await listWorkspaceFiles(path);
        set({ workspaceFiles: files, workspaceLoading: false });
      } catch (e) {
        logError("selectWorkspace", "Failed to list files:", e);
        set({ workspaceFiles: [], workspaceLoading: false });
      }
    },

    removeWorkspace: async (path) => {
      const workspaces = get().workspaces.filter((w) => w !== path);
      set({ workspaces });
      savePref(PREF_WORKSPACES, workspaces);
      // Removing the active workspace falls back to the next remaining one.
      if (get().workspacePath === path) {
        const next = workspaces[0] ?? null;
        try {
          await systemApi.setWorkspace(next);
        } catch (e) {
          logError("removeWorkspace", "Failed to set backend workspace:", e);
        }
        set({ workspacePath: next });
        savePref(PREF_WORKSPACE, next);
        if (next) {
          try {
            const files = await listWorkspaceFiles(next);
            set({ workspaceFiles: files });
          } catch {
            set({ workspaceFiles: [] });
          }
        } else {
          set({ workspaceFiles: [] });
        }
      }
    },

    refreshWorkspaceFiles: async () => {
      const path = get().workspacePath;
      if (!path) return;
      set({ workspaceLoading: true });
      try {
        const files = await listWorkspaceFiles(path);
        set({ workspaceFiles: files, workspaceLoading: false });
      } catch {
        set({ workspaceLoading: false });
      }
    },
  };
}
