/**
 * AppState — the full global store contract, assembled from slices.
 *
 * Slices live beside this file (view/workspace/update/init); appStore.ts
 * combines them into one Zustand store so every existing selector keeps
 * working unchanged.
 */

import type { AgentStatus, SystemInfo } from "@/types";
import type { AppMode } from "@/config/constants";
import type { SettingsCategory } from "@/config/settings";
import type {
  WorkspaceFileEntry,
  UpdateInfo,
  UpdateProgress,
} from "@/lib/tauri";

/** UI accent presets — each maps to a `--primary`/`--ring` hue override in
 *  index.css via the `data-accent` attribute on <html>. */
export type AccentName = "violet" | "blue" | "teal" | "green" | "amber" | "rose";

export interface AppState {
  // ── View state ──────────────────────────────────────────────
  mode: AppMode;
  /** Left sidebar collapsed to a narrow icon rail (auto-collapses on small windows). */
  sidebarCollapsed: boolean;
  /** True once the user manually toggled the sidebar — auto-collapse stops fighting them. */
  sidebarUserManaged: boolean;
  isMaximized: boolean;
  settingsOpen: boolean;
  /** Scheduled tasks (定时任务) page visibility. */
  scheduledOpen: boolean;
  /** Settings category to land on when the settings view opens (null = last/default). */
  settingsCategory: SettingsCategory | null;
  debugMode: boolean;

  // ── Depwork task selection (sidebar Groups tab) ─────────────
  activeTaskId: string | null;

  // ── Theme ──────────────────────────────────────────────────
  theme: "light" | "dark";
  /** UI accent color preset — sets the app's primary/ring/status hue. */
  accent: AccentName;

  // ── System ─────────────────────────────────────────────────
  agentStatus: AgentStatus;
  systemInfo: SystemInfo | null;

  // ── Workspace ──────────────────────────────────────────────
  workspacePath: string | null;
  /** All known workspaces (multi-project sidebar list, persisted). */
  workspaces: string[];
  workspaceFiles: WorkspaceFileEntry[];
  workspaceLoading: boolean;

  // ── Update ──────────────────────────────────────────────────
  updateInfo: UpdateInfo | null;
  updateChecking: boolean;
  updateDownloading: boolean;
  updateProgress: UpdateProgress | null;
  updateError: string | null;
  /** True when a mandatory (force) update is available — renders a blocking
   *  update screen so the user can't keep running an unsupported version. */
  forceUpdate: boolean;
  /** True once a force update finished installing — the dialog switches to a
   *  "restart now" prompt so the old process does not keep running. */
  updateInstalled: boolean;
  /** Silent (backend-only) update flow: downloaded in the background,
   *  installed when the app exits. Never prompts the user. */
  silentUpdate: {
    state: "idle" | "downloading" | "staged";
    version: string | null;
  };

  // ── Feature flags ───────────────────────────────────────────
  featureFlags: Record<string, boolean>;
  setFeatureFlag: (key: string, enabled: boolean) => Promise<void>;

  // ── View actions ───────────────────────────────────────────
  setMode: (mode: AppMode) => void;
  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  /** Open the settings view, optionally landing on a specific category. */
  openSettings: (category?: SettingsCategory) => void;
  setSettingsOpen: (open: boolean) => void;
  openScheduled: () => void;
  setScheduledOpen: (open: boolean) => void;
  /** Clear the one-shot settings category (consumed by SettingsView). */
  clearSettingsCategory: () => void;
  setDebugMode: (on: boolean) => void;
  toggleTheme: () => void;
  setTheme: (theme: "light" | "dark") => void;
  setAccent: (accent: AccentName) => void;
  setAgentStatus: (status: AgentStatus) => void;
  setSystemInfo: (info: SystemInfo) => void;
  setIsMaximized: (v: boolean) => void;
  setActiveTaskId: (id: string | null) => void;

  // ── Workspace actions ──────────────────────────────────────
  openWorkspaceDialog: () => Promise<void>;
  setWorkspacePath: (path: string | null) => void;
  /** Select an existing workspace: switches the global workspace AND the
   *  sidebar's session filter. Adds to the list if unknown. */
  selectWorkspace: (path: string) => Promise<void>;
  /** Remove a workspace from the sidebar list (not the disk). When the
   *  active one is removed, the next remaining becomes active. */
  removeWorkspace: (path: string) => Promise<void>;
  refreshWorkspaceFiles: () => Promise<void>;

  // ── Update actions ─────────────────────────────────────────
  checkForUpdate: () => Promise<void>;
  /** Route an update: silent → auto background download; manual → title-bar button. */
  handleUpdateInfo: (info: UpdateInfo | null) => void;
  /** Download + stage a silent update for the next app exit. */
  downloadSilentUpdate: () => Promise<void>;
  downloadAndInstallUpdate: () => Promise<void>;
  /** Restart the app after a force update installed (prompts the user first). */
  relaunchApp: () => Promise<void>;
  clearUpdateError: () => void;

  // ── Async init ──────────────────────────────────────────────
  initSystem: () => Promise<void>;
}

/** Zustand `set` narrowed to partial updates (slice-friendly). */
export type StoreSet = (
  partial: Partial<AppState> | ((state: AppState) => Partial<AppState>),
) => void;

/** Zustand `get` — the fully assembled store (slices may call each other). */
export type StoreGet = () => AppState;
