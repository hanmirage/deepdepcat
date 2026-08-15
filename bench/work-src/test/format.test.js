import { test } from "node:test";
import assert from "node:assert/strict";
import { formatDate } from "../src/format.js";

test("formatDate pads single-digit day", () => {
  assert.equal(formatDate(new Date(2026, 0, 5)), "2026-01-05");
});
