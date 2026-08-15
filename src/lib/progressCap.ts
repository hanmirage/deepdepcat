/**
 * Streamed tool field caps — progress text/delta and streamed arguments
 * accumulate unboundedly on long-running tools (bash, browser control,
 * office automation). The UI only ever renders the TAIL (the interesting
 * part: progress lines, errors, final output), so keeping everything would
 * balloon memory, DOM size and re-render cost for zero display value.
 */

/** Max characters kept per streamed tool field. */
export const MAX_PROGRESS_CHARS = 32_000;

/**
 * Append `delta` to `prev`, keeping only the most recent `cap` characters.
 * Exported separately from the store so it is unit-testable.
 */
export function appendCapped(
  prev: string,
  delta: string,
  cap = MAX_PROGRESS_CHARS,
): string {
  if (!delta) return prev;
  const next = prev + delta;
  return next.length > cap ? next.slice(next.length - cap) : next;
}
