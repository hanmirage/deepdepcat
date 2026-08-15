/**
 * sessionTitle — auto-generate a session title from the first user message.
 *
 * New sessions start titled "New Session" (the Rust default). On the first
 * message we derive a short, readable title from the user's text so the
 * sidebar history shows something meaningful without an extra LLM round-trip.
 *
 * Only fires once per session: the check + update are both guarded by the
 * store's in-memory state, so retries / extra turns never rewrite the title.
 */

import { sessionApi } from "@/lib/tauri";

/** Default title the backend assigns to a brand-new session. */
const DEFAULT_TITLE = "New Session";

/** Trimmed title length — long enough to be useful, short enough to fit the sidebar. */
const MAX_TITLE_LENGTH = 28;

/** Normalize whitespace so a pasted multi-line message becomes one clean title. */
function normalize(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

/** Derive a title from a user message. Falls back to the raw text when stripping leaves nothing. */
export function deriveTitleFromMessage(text: string): string {
  const cleaned = normalize(text);
  if (!cleaned) return DEFAULT_TITLE;

  // Strip markdown / code fences / links so the title reads naturally.
  const stripped = cleaned
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[*_#>]/g, "")
    .replace(/\s+/g, " ")
    .trim();

  const base = stripped || cleaned;
  return base.length > MAX_TITLE_LENGTH
    ? `${base.slice(0, MAX_TITLE_LENGTH).trimEnd()}…`
    : base;
}

/**
 * Auto-title a session on its first message.
 *
 * `sessionId` and `currentTitle` come from the store. When the title is still
 * the backend default, this updates it to a summary of `firstMessage` and
 * returns the new title (so callers can sync their state). Returns `null`
 * when no update happened (already titled / non-Tauri fallback).
 */
export async function autoTitleSession(
  sessionId: string,
  currentTitle: string,
  firstMessage: string,
): Promise<string | null> {
  if (!sessionId || !firstMessage) return null;
  if (currentTitle && currentTitle !== DEFAULT_TITLE) return null;

  // Local fallback IDs (browser dev mode) have no backend session to update.
  if (sessionId.startsWith("local-") || sessionId.startsWith("depwork-")) return null;

  const title = deriveTitleFromMessage(firstMessage);
  if (!title || title === DEFAULT_TITLE) return null;

  try {
    await sessionApi.updateSessionTitle(sessionId, title);
    return title;
  } catch {
    // Best-effort — a failed title write shouldn't block sending the message.
    return null;
  }
}
