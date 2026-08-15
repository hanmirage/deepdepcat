/**
 * MessageList compaction-divider tests — which turn a compaction record
 * belongs to (the divider renders above that turn's user message).
 */

import { describe, it, expect } from "vitest";
import { compactionForMessage } from "@/components/chat/MessageList";
import type { CompactionRecord } from "@/stores/chatStore/types";

const record = (at: number, tokens = 1000): CompactionRecord => ({
  tokens,
  summary: "s",
  at,
});

describe("compactionForMessage", () => {
  it("matches a record inside the window after the user message", () => {
    const records = [record(1_605_000)];
    expect(compactionForMessage(records, 1_000_000, 1_600_000)?.tokens).toBe(1000);
  });

  it("does not match records older than the previous message", () => {
    const records = [record(900_000)];
    expect(compactionForMessage(records, 1_000_000, 1_600_000)).toBeNull();
  });

  it("does not match records too late after the message", () => {
    const records = [record(1_600_000 + 20_000)];
    expect(compactionForMessage(records, 1_000_000, 1_600_000)).toBeNull();
  });

  it("picks the newest matching record", () => {
    const records = [record(1_500_000, 500), record(1_200_000, 800)];
    expect(compactionForMessage(records, 1_000_000, 1_600_000)?.tokens).toBe(500);
  });

  it("returns null when no records exist", () => {
    expect(compactionForMessage([], undefined, 1_000_000)).toBeNull();
  });
});
