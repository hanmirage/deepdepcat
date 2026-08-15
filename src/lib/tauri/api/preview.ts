/**
 * Tauri API bridge — preview-pane commands.
 * The rebuilt dev browser reads a local HTML report and renders it in a
 * sandboxed srcdoc iframe; external URLs open the system default handler.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";

export interface PreviewTarget {
  html: string;
  filename: string;
}

export const previewApi = {
  /** Read a local HTML target for the sandboxed preview frame. */
  readPreviewTarget: (path: string): Promise<PreviewTarget> =>
    isTauri
      ? invoke<PreviewTarget>("read_preview_target", { path })
      : Promise.resolve({ html: "<html><body></body></html>", filename: "preview.html" }),

  /** Open a preview target in the system default handler. */
  openExternal: (target: string): Promise<void> =>
    isTauri
      ? invoke("open_preview_external", { target })
      : Promise.resolve(),
};
