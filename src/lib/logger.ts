/**
 * Minimal logging facade — the ONLY place `console` is allowed.
 *
 * Keeps error observability in a desktop app while satisfying the
 * no-console lint rule: every log site goes through these helpers, and
 * debug-level output is compiled out of production builds.
 */

/* eslint-disable no-console */

export function logError(context: string, message?: unknown, ...rest: unknown[]): void {
  console.error(`[${context}]`, message, ...rest);
}

export function logWarn(context: string, message?: unknown, ...rest: unknown[]): void {
  console.warn(`[${context}]`, message, ...rest);
}

export function logInfo(context: string, message?: unknown, ...rest: unknown[]): void {
  console.info(`[${context}]`, message, ...rest);
}

/** Debug-only log — stripped from production bundles by Vite's define. */
export function logDebug(context: string, message?: unknown, ...rest: unknown[]): void {
  if (import.meta.env.DEV) {
    console.info(`[${context}]`, message, ...rest);
  }
}

/* eslint-enable no-console */
