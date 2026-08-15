/**
 * Persisted-preference helpers shared by the appStore slices.
 */

export const PREF_THEME = "deepdepcat.theme";
export const PREF_ACCENT = "deepdepcat.accent";
export const PREF_MODE = "deepdepcat.mode";
export const PREF_SIDEBAR = "deepdepcat.sidebarCollapsed";
export const PREF_SIDEBAR_MANAGED = "deepdepcat.sidebarUserManaged";
export const PREF_WORKSPACE = "deepdepcat.workspace";
export const PREF_WORKSPACES = "deepdepcat.workspaces";

export function loadPref<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function savePref(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage may be unavailable */
  }
}

/**
 * Compare two semver-ish version strings ("0.1.6" vs "0.1.10"). Returns:
 * >0 if a > b, <0 if a < b, 0 if equal. Non-numeric segments are dropped.
 */
export function compareVersions(a: string, b: string): number {
  const num = (s: string) => s.split(".").map((p) => parseInt(p, 10) || 0);
  const pa = num(a);
  const pb = num(b);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}
