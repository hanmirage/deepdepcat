import { test } from "node:test";
import assert from "node:assert/strict";

// Deterministic timer test: the wait exceeds the timer deadline, so the
// timer always fires before the assertion (Node runs timers in deadline
// order). This is the FIXED baseline — the flaky variant lives only in the
// fix-test-flake fixture.
test("async value settles within the window", async () => {
  let settled = false;
  const timer = setTimeout(() => {
    settled = true;
  }, 5);
  await new Promise((resolve) => setTimeout(resolve, 10));
  clearTimeout(timer);
  assert.equal(settled, true);
});
