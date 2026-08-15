/** In-memory promise cache — BUG: concurrent same-key calls both invoke
 *  the fetcher (the in-flight check is missing), so N parallel callers
 *  cause N duplicate fetches. */
const inflight = new Map();

export async function cachedFetch(key, fetcher) {
  // BUG: the dedup check was removed — every caller fetches.
  const promise = fetcher();
  inflight.set(key, promise);
  promise.finally(() => inflight.delete(key));
  return promise;
}
