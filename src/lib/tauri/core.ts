/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

import { convertFileSrc } from "@tauri-apps/api/core";
import { logError } from "@/lib/logger";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as dialogOpen } from "@tauri-apps/plugin-dialog";
import { readDir, readFile, readTextFile } from "@tauri-apps/plugin-fs";
import i18n from "@/i18n";

// ── Mock event bus (browser dev mode only) ─────────────────────

/**
 * When not running inside Tauri, `invoke()` and `listen()` are unavailable.
 * This mock bus lets the UI function in `npm run dev` (browser) for
 * layout/preview work. Real data only flows inside `npm run tauri dev`.
 */
const mockListeners = new Map<string, Set<(payload: unknown) => void>>();

/** Cancel function for the active mock stream. Set by chatApi.sendMessage, called by cancelOperation. */
let mockCancelStream: (() => void) | null = null;

/** Register the active mock stream's cancel function (browser dev mode). */
export function setMockCancelStream(fn: () => void): void {
  mockCancelStream = fn;
}

/** Cancel and clear the active mock stream (browser dev mode). */
export function clearMockCancelStream(): void {
  const fn = mockCancelStream;
  mockCancelStream = null;
  if (fn) fn();
}

/** Emit a mock event — triggers all listeners registered for `eventName`. */
export function mockEmit(eventName: string, payload: unknown): void {
  const set = mockListeners.get(eventName);
  if (set) for (const fn of set) fn(payload);
}

// ── Detect if running inside Tauri ───────────────────────────

/** True when the app is running inside the Tauri webview (not a browser). */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Convert a local file path to an asset URL loadable by <img src>. */
export function toAssetUrl(filePath: string): string {
  if (!isTauri) return filePath;
  return convertFileSrc(filePath);
}

// ── Event helpers ────────────────────────────────────────────

/**
 * Listen to a Tauri event from the Rust backend.
 * Returns an `unlisten` function for cleanup.
 *
 * @example
 * useEffect(() => {
 *   const unlisten = onEvent<ChatStreamEvent>("chat-stream", (e) => { ... });
 *   return () => { unlisten.then(fn => fn()); };
 * }, []);
 */
export async function onEvent<T>(
  eventName: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  if (!isTauri) {
    // Register on mock bus so mockEmit can drive the UI in browser dev mode
    const set = mockListeners.get(eventName) ?? new Set();
    set.add(handler as (payload: unknown) => void);
    mockListeners.set(eventName, set);
    return () => {
      set.delete(handler as (payload: unknown) => void);
      if (set.size === 0) mockListeners.delete(eventName);
    };
  }
  return listen<T>(eventName, (event) => handler(event.payload));
}

// ── Window controls (for custom title bar) ───────────────────

export const windowApi = {
  minimize: () => { if (isTauri) getCurrentWindow().minimize(); },
  toggleMaximize: () => { if (isTauri) getCurrentWindow().toggleMaximize(); },
  close: () => { if (isTauri) getCurrentWindow().close(); },
  isMaximized: () => isTauri ? getCurrentWindow().isMaximized() : Promise.resolve(false),
  onResized: (handler: () => void): Promise<() => void> => {
    if (!isTauri) return Promise.resolve(() => {});
    return getCurrentWindow().onResized(handler);
  },
};

// ── Workspace commands ─────────────────────────────────────

/** Workspace file entry — a single file or directory in the workspace. */
export interface WorkspaceFileEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number | null;
}

/** Open a native folder-picker dialog. Returns the selected path or null. */
export async function pickFolder(): Promise<string | null> {
  if (!isTauri) return null;
  const selected = await dialogOpen({
    directory: true,
    multiple: false,
    title: i18n.t("common.selectWorkspaceDir"),
  });
  return typeof selected === "string" ? selected : null;
}

/** Open a native multi-file picker. Returns selected file paths (may be empty). */
export async function pickFiles(): Promise<string[]> {
  if (!isTauri) return [];
  const selected = await dialogOpen({
    multiple: true,
    title: i18n.t("common.selectFiles"),
  });
  if (!selected) return [];
  return Array.isArray(selected) ? selected : [selected];
}

/** Open a native image picker (JPG/PNG/WebP/GIF). Returns the selected path or null. */
export async function pickImage(): Promise<string | null> {
  if (!isTauri) return null;
  const selected = await dialogOpen({
    multiple: false,
    title: i18n.t("common.selectAvatarImage"),
    filters: [
      { name: i18n.t("common.image"), extensions: ["jpg", "jpeg", "png", "webp", "gif"] },
    ],
  });
  return typeof selected === "string" ? selected : null;
}

/** Open a native multi-folder picker. Returns selected folder paths (may be empty). */
export async function pickFolders(): Promise<string[]> {
  if (!isTauri) return [];
  const selected = await dialogOpen({
    directory: true,
    multiple: true,
    title: i18n.t("common.selectFolders"),
  });
  if (!selected) return [];
  return Array.isArray(selected) ? selected : [selected];
}

/** A file dragged over the window: hover state or an actual drop. */
export type FileDragEvent =
  | { type: "over"; paths?: undefined }
  | { type: "leave"; paths?: undefined }
  | { type: "drop"; paths: string[] };

/**
 * Listen for native file drags onto the app window (Tauri) — the payload
 * carries REAL file paths, unlike the browser's dataTransfer which only
 * exposes file names. Returns an unlisten function.
 */
export function onFileDrop(cb: (e: FileDragEvent) => void): Promise<() => void> {
  if (!isTauri) return Promise.resolve(() => {});
  let win: ReturnType<typeof getCurrentWindow> | undefined;
  try {
    win = getCurrentWindow();
  } catch {
    return Promise.resolve(() => {});
  }
  if (!win || typeof win.onDragDropEvent !== "function") {
    return Promise.resolve(() => {});
  }
  return win.onDragDropEvent((event) => {
    const { type, paths } = event.payload as {
      type: "enter" | "over" | "drop" | "leave";
      paths?: string[];
    };
    cb(mapDragEvent(type, paths ?? []));
  });
}

/**
 * Map a native Tauri drag-drop event to the app-level FileDragEvent.
 *
 * Tauri emits `enter` (file entered the window, paths present), `over`
 * (hovering inside), `drop` (released, paths present) and `leave`. Both
 * `enter` and `over` mean "dragging over the window" — mapping them to the
 * same `over` state keeps the drop-overlay visible the whole time the file
 * is over the app (previously `enter` fell into `leave`, hiding the overlay
 * the instant a drag began). `drop` carries the real paths.
 */
export function mapDragEvent(
  type: "enter" | "over" | "drop" | "leave",
  paths: string[],
): FileDragEvent {
  switch (type) {
    case "drop":
      return { type: "drop", paths };
    case "enter":
    case "over":
      return { type: "over" };
    case "leave":
      return { type: "leave" };
  }
}

/** List files in a workspace directory (one level deep). */
export async function listWorkspaceFiles(dirPath: string): Promise<WorkspaceFileEntry[]> {
  if (!isTauri) return [];
  try {
    const entries = await readDir(dirPath);
    const result: WorkspaceFileEntry[] = [];
    for (const entry of entries) {
      // Skip hidden files/dirs (starting with .)
      if (entry.name.startsWith(".")) continue;
      // Skip node_modules, target, dist, .git-like heavy dirs
      if (entry.isDirectory && (entry.name === "node_modules" || entry.name === "target" || entry.name === "dist")) continue;
      result.push({
        name: entry.name,
        path: `${dirPath}/${entry.name}`,
        isDir: entry.isDirectory,
        size: null,
      });
    }
    // Sort: directories first, then files, alphabetically
    result.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    return result;
  } catch (e) {
    logError("listWorkspaceFiles", "Error:", e);
    return [];
  }
}

/** Read a text file from the workspace. */
export async function readWorkspaceTextFile(filePath: string): Promise<string> {
  if (!isTauri) return "";
  return readTextFile(filePath);
}

/** Read a binary file from the workspace (e.g. docx/pptx/xlsx bytes for the
 *  document viewers). Browser dev mode has no filesystem — callers must
 *  handle the null case (show a "desktop only" hint). */
export async function readWorkspaceBinaryFile(filePath: string): Promise<Uint8Array | null> {
  if (!isTauri) return null;
  return readFile(filePath);
}

