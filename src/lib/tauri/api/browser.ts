/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";
import { MOCK_BROWSER_STATUS } from "../mock";
import type { BrowserStatus } from "../types";

/** Agent tool → dev-browser window handoff (`dev-browser-open` event). */
export interface DevBrowserOpenEvent {
  url?: string | null;
  path?: string | null;
}

/** One live frame relayed from the takeover browser (`browser-screencast-frame`). */
export interface BrowserScreencastFrame {
  profile: string;
  /** Base64 JPEG, no `data:` prefix. */
  jpeg: string;
  /** Page viewport width (CSS px) — scale clicks by renderedWidth / vw. */
  vw: number;
  /** Page viewport height (CSS px). */
  vh: number;
  seq: number;
}

/** Backend push when a browser session starts/stops (`browser-status-changed`).
 *  The live "browser" pane follows `profile === sessionBrowserProfile(id)`. */
export interface BrowserStatusChangedEvent {
  profile: string;
  running: boolean;
}

/** One tab in the real browser (frontend tab strip). */
export interface BrowserTab {
  id: string;
  title: string;
  url: string;
  active: boolean;
}

export const BROWSER_SCREENCAST_FRAME_EVENT = "browser-screencast-frame";
export const BROWSER_STATUS_CHANGED_EVENT = "browser-status-changed";

/** The browser profile key a conversation's agent browser lives under
 *  (mirrors Rust `session_profile_key`). */
export function sessionBrowserProfile(sessionId: string | null | undefined): string {
  return sessionId ? `session-${sessionId}` : "";
}

export type BrowserInputKind = "mouse" | "wheel" | "key" | "text";

/** One input event forwarded to the takeover browser. */
export interface BrowserInputPayload {
  kind: BrowserInputKind;
  /** mouse: move/down/up; key: down/up. */
  event?: string;
  x?: number;
  y?: number;
  /** Mouse button bitmask (1 = left). */
  buttons?: number;
  clickCount?: number;
  deltaX?: number;
  deltaY?: number;
  key?: string;
  code?: string;
  text?: string;
  profile?: string;
}

export const browserApi = {
  start: (url?: string): Promise<BrowserStatus> =>
    isTauri
      ? invoke<BrowserStatus>("browser_takeover_start", { url: url ?? null })
      : Promise.resolve(MOCK_BROWSER_STATUS),

  stop: (): Promise<boolean> =>
    isTauri ? invoke<boolean>("browser_takeover_stop") : Promise.resolve(false),

  status: (profile?: string): Promise<BrowserStatus> =>
    isTauri
      ? invoke<BrowserStatus>("browser_takeover_status", { profile: profile ?? null })
      : Promise.resolve(MOCK_BROWSER_STATUS),

  navigate: (url: string): Promise<BrowserStatus> =>
    isTauri
      ? invoke<BrowserStatus>("browser_takeover_navigate", { url })
      : Promise.resolve(MOCK_BROWSER_STATUS),

  screenshot: (): Promise<string> =>
    isTauri ? invoke<string>("browser_takeover_screenshot") : Promise.resolve(""),

  logs: (): Promise<{ console: unknown[]; network: unknown[]; errors: unknown[] }> =>
    isTauri
      ? invoke("browser_takeover_logs")
      : Promise.resolve({ console: [], network: [], errors: [] }),

  resume: (): Promise<boolean> =>
    isTauri ? invoke<boolean>("browser_takeover_resume") : Promise.resolve(false),

  /** Structured tab list for the real browser (frontend tab strip). */
  tabs: (): Promise<BrowserTab[]> =>
    isTauri ? invoke<BrowserTab[]>("browser_tabs") : Promise.resolve([]),

  /** Open a new tab (optionally at `url`) and switch to it. */
  tabNew: (url?: string): Promise<BrowserTab[]> =>
    isTauri
      ? invoke<BrowserTab[]>("browser_tab_new", { url: url ?? null })
      : Promise.resolve([]),

  /** Switch the active tab. */
  tabSwitch: (targetId: string): Promise<BrowserTab[]> =>
    isTauri
      ? invoke<BrowserTab[]>("browser_tab_switch", { targetId })
      : Promise.resolve([]),

  /** Close a tab (the active one when omitted). */
  tabClose: (targetId?: string): Promise<BrowserTab[]> =>
    isTauri
      ? invoke<BrowserTab[]>("browser_tab_close", { targetId: targetId ?? null })
      : Promise.resolve([]),

  /** Start live frame streaming for a browser session (idempotent). */
  screencastStart: (profile?: string): Promise<void> =>
    isTauri
      ? invoke("browser_screencast_start", { profile: profile ?? null })
      : Promise.resolve(),

  /** Stop a browser session's frame stream. */
  screencastStop: (profile?: string): Promise<void> =>
    isTauri
      ? invoke("browser_screencast_stop", { profile: profile ?? null })
      : Promise.resolve(),

  /** Forward one input event (mouse/wheel/key/text) to the takeover browser. */
  input: (payload: BrowserInputPayload): Promise<void> =>
    isTauri
      ? invoke("browser_takeover_input", {
          kind: payload.kind,
          event: payload.event ?? null,
          x: payload.x ?? null,
          y: payload.y ?? null,
          buttons: payload.buttons ?? null,
          clickCount: payload.clickCount ?? null,
          deltaX: payload.deltaX ?? null,
          deltaY: payload.deltaY ?? null,
          key: payload.key ?? null,
          code: payload.code ?? null,
          text: payload.text ?? null,
          profile: payload.profile ?? null,
        })
      : Promise.resolve(),
};
