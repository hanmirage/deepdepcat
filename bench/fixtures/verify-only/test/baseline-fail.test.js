import { test } from "node:test";
import assert from "node:assert/strict";

// Deliberately failing baseline test: verify-only must REPORT this failure
// truthfully and must NOT fix it (zero file changes is an acceptance).
test("baseline failure is reported truthfully", () => {
  assert.equal(1 + 1, 3);
});
