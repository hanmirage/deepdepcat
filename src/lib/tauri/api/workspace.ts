/**
 * Tauri API bridge — workspace file actions (open / reveal).
 * Backend: `open_workspace_file` in src-tauri/src/commands/system.rs.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";

/** Workspace file actions used by the Depwork preview panel. */
export const workspaceFileApi = {
  /**
   * Open a workspace file with the system default app.
   * Fails when the path is outside the current workspace (backend-validated).
   */
  open: (path: string) =>
    isTauri
      ? invoke<void>("open_workspace_file", { path, reveal: false })
      : Promise.resolve(),

  /** Reveal a workspace file in the system file manager. */
  reveal: (path: string) =>
    isTauri
      ? invoke<void>("open_workspace_file", { path, reveal: true })
      : Promise.resolve(),
};
