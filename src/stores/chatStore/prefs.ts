/**
 * Chat store — split by concern (types / prefs / mode detection / stream state).
 */
export const PREF_MODEL = "deepdepcat.chat.model";
export const PREF_MODE = "deepdepcat.chat.interaction";
export const PREF_REASONING = "deepdepcat.chat.reasoning";
export const PREF_AGENT_MODE = "deepdepcat.chat.agent-mode";
export const PREF_AGENT_PERSONA = "deepdepcat.chat.agent-persona";

export const DEPWORK_PREF_MODEL = "deepdepcat.depwork.model";
export const DEPWORK_PREF_MODE = "deepdepcat.depwork.interaction";
export const DEPWORK_PREF_AGENT_PERSONA = "deepdepcat.depwork.agent-persona";

export function loadPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
export function savePref(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage may be unavailable */
  }
}

// ── Auto mode detection keyword tables ───────────────────────────
//
// Rule 1: keywords in the user message trigger an automatic mode switch.
// Rule 2: confirmation keywords switch from plan → confirm.
// manualOverride tracks whether the user picked a mode themselves —
