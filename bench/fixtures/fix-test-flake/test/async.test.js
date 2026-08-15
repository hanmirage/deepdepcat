import { test } from "node:test";
import assert from "node:assert/strict";

// Deliberately flaky: the timer race makes this fail intermittently.
test("async value settles within the window", async () => {
  let settled = false;
  const timer = setTimeout(() => {
    settled = true;
  }, 5);
  await new Promise((resolve) => setTimeout(resolve, 4));
  clearTimeout(timer);
  assert.equal(settled, true);
});
