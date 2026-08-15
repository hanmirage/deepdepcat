/** Legacy callback-style file loader (kept for compatibility). */
export function loadConfig(path, callback) {
  // In a real project this would read a file; the fixture simulates it.
  const fake = { retries: 3, timeoutMs: 1000 };
  setTimeout(() => callback(null, fake), 1);
}

export function loadConfigWithRetry(path, attempts, callback) {
  let remaining = attempts;
  const tryOnce = () => {
    loadConfig(path, (err, config) => {
      if (err && remaining > 0) {
        remaining -= 1;
        setTimeout(tryOnce, 10);
        return;
      }
      callback(err, config);
    });
  };
  tryOnce();
}
