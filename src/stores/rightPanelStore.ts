/**
 * Right panel store — an event-driven transient pane stack, shared by BOTH
 * product modes but with every transient signal keyed per mode.
 *
 * `panes` holds only the transient context panes — activity / files /
 * browser / plan. The last entry in `panes` is the focused one.
 * - The panel is collapsed by default and collapsible via the title bar or
 *   the header chevron.
 * - The activity pane (tasks / subagents) is transient: it appears while the
 *   agent is dispatching (`notifyActivity`) and auto-clears when the agent
 *   returns to idle (`clearActivity`).
 * - Agent activity pulses the title-bar badge and auto-shows the activity
 *   pane; a user dismiss suppresses auto-showing for the rest of the run.
 * - Chat context jumps: file targets reveal the file pane with a pending
 *   selection (per mode).
 * - Width follows content: one transient ≈300, two ≈720, browser 1080.
 *
 * ## Code / depwork isolation
 * `panes`, `pendingFile`, `pendingPreview`, `activitySignal`,
 * `autoOpenSuppressed` and `width` are ALL per-mode: one mode's browser
 * target, badge pulse, dismiss, or width never bleeds into the other mode.
 * Only `open` (the physical panel) is shared.
 */

import { create } from "zustand";
import type { AppMode } from "@/config/constants";

export type RightPaneId =
  | "activity"
  | "files"
  | "browser"
  | "preview"
  | "plan"
  | "subagents"
  | "task";

/** Drag range for the right-panel width (mirrors the drawer's MIN/MAX). */
export const MIN_RIGHT_PANEL_WIDTH = 280;
export const MAX_RIGHT_PANEL_WIDTH = 1280;
export const DEFAULT_RIGHT_PANEL_WIDTH = 300;
/** Auto-expanded width when two panes are open side by side. */
export const TWO_PANE_WIDTH = 720;
/** Auto-expanded width when the browser pane is open (needs web width). */
export const BROWSER_PANE_WIDTH = 1080;
/** Maximum transient panes visible at once. */
export const MAX_PANES = 2;

const PREF_RIGHT_PANEL = "deepdepcat.rightPanelOpen";
const PREF_RIGHT_PANEL_WIDTH = "deepdepcat.rightPanelWidth";
const PREF_RIGHT_PANEL_PANES = "deepdepcat.rightPanelPanes";
const PREF_RIGHT_PANEL_LEGACY_PANE = "deepdepcat.rightPanelPane";
const PREF_RIGHT_PANEL_LEGACY_TAB = "deepdepcat.rightPanelTab";
/** True when the user closed the panel — activity auto-open is suppressed
 *  until they open it again. Persisted so a collapse survives restarts. */
const PREF_RIGHT_PANEL_SUPPRESSED = "deepdepcat.rightPanelSuppressed";

interface RightPanelPanesState {
  code: RightPaneId[];
  depwork: RightPaneId[];
}

interface PendingFileState {
  code: string | null;
  depwork: string | null;
}

/** Product-preview target handed over by the agent (`dev-browser-open`),
 *  consumed by the HtmlPreviewPane when it mounts — the one-shot event can
 *  fire before the pane exists, so the target must survive until then. */
export interface PendingPreviewOpen {
  url: string | null;
  path: string | null;
}

/** Per-mode width — the browser pane widens one mode to 1080; the other mode
 *  must not inherit it. */
type ModeWidthState = Record<AppMode, number>;
/** Per-mode badge — a background dispatch in the OTHER mode must not pulse
 *  this mode's badge. */
type ModeSignalState = Record<AppMode, boolean>;
/** Per-mode auto-open suppression — dismissing in one mode must not silence
 *  the other mode's auto-open for the run. */
type ModeSuppressState = Record<AppMode, boolean>;

/** The transient stack starts empty. */
const DEFAULT_RIGHT_PANEL_PANES: RightPanelPanesState = {
  code: [],
  depwork: [],
};

const DEFAULT_PENDING_FILE: PendingFileState = {
  code: null,
  depwork: null,
};

const DEFAULT_PENDING_PREVIEW: Record<AppMode, PendingPreviewOpen | null> = {
  code: null,
  depwork: null,
};

const DEFAULT_MODE_WIDTH: ModeWidthState = {
  code: DEFAULT_RIGHT_PANEL_WIDTH,
  depwork: DEFAULT_RIGHT_PANEL_WIDTH,
};

const DEFAULT_MODE_SIGNAL: ModeSignalState = { code: false, depwork: false };
const DEFAULT_MODE_SUPPRESS: ModeSuppressState = { code: false, depwork: false };

/** Pane inventory per mode — the shared set plus the mode-specific entry. */
export const PANES_BY_MODE: Record<AppMode, RightPaneId[]> = {
  code: ["activity", "files", "browser", "preview", "plan", "subagents", "task"],
  depwork: ["activity", "files", "browser", "preview", "plan", "subagents", "task"],
};

/** Old 5-tab ids → pane ids (localStorage migration for existing users). */
const LEGACY_TAB_TO_PANE: Record<string, RightPaneId> = {
  tasks: "activity",
  agent: "activity",
  workspace: "files",
  files: "files",
};

function loadPref<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

function savePref(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage may be unavailable */
  }
}

function mapToPane(mode: AppMode, raw: unknown): RightPaneId | null {
  const candidate = String(raw);
  if (PANES_BY_MODE[mode].includes(candidate as RightPaneId)) {
    return candidate as RightPaneId;
  }
  const legacy = LEGACY_TAB_TO_PANE[candidate];
  if (legacy && PANES_BY_MODE[mode].includes(legacy)) return legacy;
  return null;
}

function normalizeList(mode: AppMode, raw: unknown): RightPaneId[] {
  const list = Array.isArray(raw) ? raw : raw != null ? [raw] : [];
  const mapped = list.map((item) => mapToPane(mode, item));
  const dedup: RightPaneId[] = [
    ...new Set(mapped.filter((p): p is RightPaneId => p !== null)),
  ];
  return dedup.slice(0, MAX_PANES);
}

function normalizePanes(value: unknown): RightPanelPanesState {
  const v = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  return {
    code: normalizeList("code", v.code),
    depwork: normalizeList("depwork", v.depwork),
  };
}

function loadPanesPref(): RightPanelPanesState {
  const raw = loadPref<unknown>(PREF_RIGHT_PANEL_PANES, null);
  if (raw) return normalizePanes(raw);
  const single = loadPref<unknown>(PREF_RIGHT_PANEL_LEGACY_PANE, null);
  if (single) return normalizePanes(single);
  const legacy = loadPref<unknown>(PREF_RIGHT_PANEL_LEGACY_TAB, null);
  if (legacy) return normalizePanes(legacy);
  return DEFAULT_RIGHT_PANEL_PANES;
}

function clampWidth(w: number): number {
  return Math.min(MAX_RIGHT_PANEL_WIDTH, Math.max(MIN_RIGHT_PANEL_WIDTH, w));
}

/** Per-mode width pref — a legacy plain-number value is migrated to both
 *  modes. */
function loadWidthPref(): ModeWidthState {
  const raw = loadPref<unknown>(PREF_RIGHT_PANEL_WIDTH, null);
  if (typeof raw === "number") {
    const w = clampWidth(raw);
    return { code: w, depwork: w };
  }
  if (raw && typeof raw === "object") {
    const v = raw as Record<string, unknown>;
    return {
      code: clampWidth(typeof v.code === "number" ? v.code : DEFAULT_RIGHT_PANEL_WIDTH),
      depwork: clampWidth(typeof v.depwork === "number" ? v.depwork : DEFAULT_RIGHT_PANEL_WIDTH),
    };
  }
  return { ...DEFAULT_MODE_WIDTH };
}

/** Per-mode suppression pref — a legacy plain-boolean value is migrated to
 *  both modes. */
function loadSuppressPref(): ModeSuppressState {
  const raw = loadPref<unknown>(PREF_RIGHT_PANEL_SUPPRESSED, null);
  if (typeof raw === "boolean") {
    return { code: raw, depwork: raw };
  }
  if (raw && typeof raw === "object") {
    const v = raw as Record<string, unknown>;
    return { code: v.code === true, depwork: v.depwork === true };
  }
  return { ...DEFAULT_MODE_SUPPRESS };
}

/** Width the panel should be given the transient stack — the browser needs
 *  the most, empty defaults to the standard width. */
function targetWidth(panes: RightPaneId[]): number {
  if (panes.includes("browser") || panes.includes("preview")) return BROWSER_PANE_WIDTH;
  if (panes.length >= 2) return TWO_PANE_WIDTH;
  return DEFAULT_RIGHT_PANEL_WIDTH;
}

/** Width after adding a pane: expand to the content's need. */
function widthForPanes(panes: RightPaneId[], current: number): number {
  return Math.max(current, targetWidth(panes));
}

/** Width after removing a pane: shrink back to the content's need. */
function widthAfterClose(panes: RightPaneId[], current: number): number {
  return Math.min(current, targetWidth(panes));
}

interface RightPanelStore {
  open: boolean;
  width: ModeWidthState;
  /** Transient panes (0-2, last = focused), per mode. */
  panes: RightPanelPanesState;
  /** File revealed from a chat target, per mode (consumed by the files pane). */
  pendingFile: PendingFileState;
  /** Dev-browser target from `dev-browser-open`, per mode (consumed by the
   *  preview pane on mount). */
  pendingPreview: Record<AppMode, PendingPreviewOpen | null>;
  /** True while unviewed agent activity exists in the mode — pulses the rail
   *  badge for that mode only. */
  activitySignal: ModeSignalState;
  /** True after the user dismissed the drawer — auto-open stops for this
   *  mode's run. */
  autoOpenSuppressed: ModeSuppressState;

  toggle: (mode: AppMode) => void;
  setOpen: (open: boolean, mode: AppMode) => void;
  /** Open/focus a pane (rail click). Closes it when it was the only pane. */
  openPane: (mode: AppMode, pane: RightPaneId) => void;
  /** Remove one pane from the drawer; dismisses when the drawer would empty. */
  closePane: (mode: AppMode, pane: RightPaneId) => void;
  /** User dismissed the drawer — suppress auto-open until opened again. */
  dismiss: (mode: AppMode) => void;
  setWidth: (mode: AppMode, width: number) => void;
  /** Chat jump: open the files pane and request a file selection. */
  revealFile: (mode: AppMode, path: string) => void;
  clearPendingFile: (mode: AppMode) => void;
  /** Stash a dev-browser target for the pane to consume on mount. */
  setPendingPreview: (mode: AppMode, payload: PendingPreviewOpen) => void;
  clearPendingPreview: (mode: AppMode) => void;
  /** Agent activity hook: badge + one-shot auto-open to the activity pane. */
  notifyActivity: (mode: AppMode) => void;
  /** Agent dispatch ended — drop the transient activity pane. */
  clearActivity: (mode: AppMode) => void;
  /** Subagent dispatch hook: badge + one-shot auto-open to the subagents pane. */
  notifySubagents: (mode: AppMode) => void;
  /** Dispatch ended — drop the transient subagents pane. */
  clearSubagents: (mode: AppMode) => void;
  /** Task-plan hook (code): badge + one-shot auto-open to the task pane. */
  notifyTask: (mode: AppMode) => void;
  /** Task-plan ended — drop the transient task pane. */
  clearTask: (mode: AppMode) => void;
  /** Remove a state pane. */
  removePane: (mode: AppMode, pane: RightPaneId) => void;
  clearActivitySignal: (mode: AppMode) => void;
}

export const useRightPanelStore = create<RightPanelStore>((set, get) => {
  /** Auto-open/shove a transient pane on an activity signal (shared by the
   *  activity and subagent panes). Closed → open once unless the user
   *  dismissed for the run (badge only); open → ensure the pane is stacked. */
  const signalPane = (mode: AppMode, pane: RightPaneId) => {
    const s = get();
    const has = s.panes[mode].includes(pane);
    const next = has ? s.panes[mode] : [...s.panes[mode], pane].slice(-MAX_PANES);
    const panes = { ...s.panes, [mode]: next };
    const signal = { ...s.activitySignal, [mode]: true };
    if (!s.open) {
      if (s.autoOpenSuppressed[mode]) {
        set({ activitySignal: signal });
        return;
      }
      savePref(PREF_RIGHT_PANEL_PANES, panes);
      savePref(PREF_RIGHT_PANEL, true);
      const width = { ...s.width, [mode]: widthForPanes(next, s.width[mode]) };
      savePref(PREF_RIGHT_PANEL_WIDTH, width);
      set({ open: true, panes, width, activitySignal: signal });
      return;
    }
    if (!has) {
      savePref(PREF_RIGHT_PANEL_PANES, panes);
      const width = { ...s.width, [mode]: widthForPanes(next, s.width[mode]) };
      savePref(PREF_RIGHT_PANEL_WIDTH, width);
      set({ panes, width });
    }
    set({ activitySignal: signal });
  };

  return {
  // The panel starts collapsed and remembers the user's choice — a collapse
  // (or explicit close) is restored on the next launch, never re-opened by a
  // refresh.
  open: loadPref<boolean>(PREF_RIGHT_PANEL, false),
  width: loadWidthPref(),
  panes: loadPanesPref(),
  pendingFile: DEFAULT_PENDING_FILE,
  pendingPreview: { ...DEFAULT_PENDING_PREVIEW },
  activitySignal: { ...DEFAULT_MODE_SIGNAL },
  autoOpenSuppressed: loadSuppressPref(),

  toggle: (mode) =>
    set((s) => {
      const next = !s.open;
      savePref(PREF_RIGHT_PANEL, next);
      const suppress = { ...s.autoOpenSuppressed, [mode]: next ? false : true };
      savePref(PREF_RIGHT_PANEL_SUPPRESSED, suppress);
      return { open: next, autoOpenSuppressed: suppress };
    }),

  setOpen: (open, mode) => {
    const suppress = { ...get().autoOpenSuppressed, [mode]: open ? false : true };
    savePref(PREF_RIGHT_PANEL_SUPPRESSED, suppress);
    savePref(PREF_RIGHT_PANEL, open);
    set({ open, autoOpenSuppressed: suppress });
  },

  openPane: (mode, pane) => {
    const s = get();
    const without = s.panes[mode].filter((p) => p !== pane);
    const next = [...without, pane].slice(-MAX_PANES);
    const panes = { ...s.panes, [mode]: next };
    savePref(PREF_RIGHT_PANEL_PANES, panes);
    savePref(PREF_RIGHT_PANEL, true);
    const suppress = { ...s.autoOpenSuppressed, [mode]: false };
    savePref(PREF_RIGHT_PANEL_SUPPRESSED, suppress);
    const width = { ...s.width, [mode]: widthForPanes(next, s.width[mode]) };
    savePref(PREF_RIGHT_PANEL_WIDTH, width);
    set({ open: true, panes, width, autoOpenSuppressed: suppress });
  },

  closePane: (mode, pane) => {
    const s = get();
    const next = s.panes[mode].filter((p) => p !== pane);
    const panes = { ...s.panes, [mode]: next };
    savePref(PREF_RIGHT_PANEL_PANES, panes);
    const width = { ...s.width, [mode]: widthAfterClose(next, s.width[mode]) };
    savePref(PREF_RIGHT_PANEL_WIDTH, width);
    set({ panes, width });
  },

  dismiss: (mode) => {
    const suppress = { ...get().autoOpenSuppressed, [mode]: true };
    savePref(PREF_RIGHT_PANEL, false);
    savePref(PREF_RIGHT_PANEL_SUPPRESSED, suppress);
    set({ open: false, autoOpenSuppressed: suppress });
  },

  setWidth: (mode, width) => {
    const clamped = Math.min(
      MAX_RIGHT_PANEL_WIDTH,
      Math.max(MIN_RIGHT_PANEL_WIDTH, width),
    );
    const next = { ...get().width, [mode]: clamped };
    set({ width: next });
    savePref(PREF_RIGHT_PANEL_WIDTH, next);
  },

  revealFile: (mode, path) => {
    const s = get();
    const without = s.panes[mode].filter((p) => p !== "files");
    const next = [...without, "files" as RightPaneId].slice(-MAX_PANES);
    const panes = { ...s.panes, [mode]: next };
    savePref(PREF_RIGHT_PANEL_PANES, panes);
    savePref(PREF_RIGHT_PANEL, true);
    const suppress = { ...s.autoOpenSuppressed, [mode]: false };
    savePref(PREF_RIGHT_PANEL_SUPPRESSED, suppress);
    const width = { ...s.width, [mode]: widthForPanes(next, s.width[mode]) };
    savePref(PREF_RIGHT_PANEL_WIDTH, width);
    // Depwork: a chat file jump must actually select the file in the depwork
    // workspace — `pendingFile.depwork` alone was a dead end (WorkspacePanel
    // never consumed it), so route the jump into the depwork store directly.
    if (mode === "depwork") {
      void import("@/stores/depworkStore").then((m) => {
        const name = path.split(/[\\/]/).pop() ?? path;
        m.useDepworkStore
          .getState()
          .selectFile({ name, path, isDir: false, size: null });
      });
    }
    set({
      open: true,
      panes,
      width,
      autoOpenSuppressed: suppress,
      pendingFile: { ...s.pendingFile, [mode]: path },
    });
  },

  clearPendingFile: (mode) => {
    const s = get();
    if (s.pendingFile[mode] === null) return;
    set({ pendingFile: { ...s.pendingFile, [mode]: null } });
  },

  setPendingPreview: (mode, payload) =>
    set((s) => ({ pendingPreview: { ...s.pendingPreview, [mode]: payload } })),

  clearPendingPreview: (mode) =>
    set((s) => ({ pendingPreview: { ...s.pendingPreview, [mode]: null } })),

  notifyActivity: (mode) => signalPane(mode, "activity"),
  clearActivity: (mode) => get().removePane(mode, "activity"),
  notifySubagents: (mode) => signalPane(mode, "subagents"),
  clearSubagents: (mode) => get().removePane(mode, "subagents"),
  notifyTask: (mode) => signalPane(mode, "task"),
  clearTask: (mode) => get().removePane(mode, "task"),

  removePane: (mode, pane) => {
    const s = get();
    const without = s.panes[mode].filter((p) => p !== pane);
    if (without.length === s.panes[mode].length) return;
    const panes = { ...s.panes, [mode]: without };
    savePref(PREF_RIGHT_PANEL_PANES, panes);
    const width = { ...s.width, [mode]: widthAfterClose(without, s.width[mode]) };
    savePref(PREF_RIGHT_PANEL_WIDTH, width);
    set({ panes, width });
  },

  clearActivitySignal: (mode) =>
    set((s) => ({ activitySignal: { ...s.activitySignal, [mode]: false } })),
  };
});
