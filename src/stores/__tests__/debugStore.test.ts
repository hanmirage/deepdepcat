import { describe, it, expect, beforeEach } from "vitest";
import { useDebugStore } from "@/stores/debugStore";
import type { DebugEvent } from "@/types";

function makeEvent(type: string, overrides: Partial<DebugEvent> = {}): DebugEvent {
  return {
    type: type as DebugEvent["type"],
    session_id: "test-session",
    timestamp: Date.now(),
    ...overrides,
  } as DebugEvent;
}

describe("debugStore", () => {
  beforeEach(() => {
    useDebugStore.setState({
      events: [],
      paused: false,
      activeFilters: new Set(),
    });
  });

  describe("initial state", () => {
    it("starts with empty events", () => {
      expect(useDebugStore.getState().events).toEqual([]);
    });

    it("starts not paused", () => {
      expect(useDebugStore.getState().paused).toBe(false);
    });

    it("starts with no filters", () => {
      expect(useDebugStore.getState().activeFilters.size).toBe(0);
    });
  });

  describe("addEvent", () => {
    it("adds an event to the log", () => {
      const event = makeEvent("agent_turn_start", { turn: 1, mode: "standard" });
      useDebugStore.getState().addEvent(event);
      expect(useDebugStore.getState().events).toHaveLength(1);
      expect(useDebugStore.getState().events[0]).toEqual(event);
    });

    it("appends events in order", () => {
      const e1 = makeEvent("agent_turn_start", { turn: 1, mode: "standard" });
      const e2 = makeEvent("agent_turn_end", { turn: 1, duration_ms: 100 });

      useDebugStore.getState().addEvent(e1);
      useDebugStore.getState().addEvent(e2);

      expect(useDebugStore.getState().events).toHaveLength(2);
      expect(useDebugStore.getState().events[0]).toEqual(e1);
      expect(useDebugStore.getState().events[1]).toEqual(e2);
    });

    it("does not add events when paused", () => {
      useDebugStore.getState().togglePause();
      const event = makeEvent("agent_turn_start", { turn: 1, mode: "standard" });
      useDebugStore.getState().addEvent(event);
      expect(useDebugStore.getState().events).toHaveLength(0);
    });

    it("caps at 500 events (FIFO)", () => {
      for (let i = 0; i < 510; i++) {
        useDebugStore.getState().addEvent(
          makeEvent("agent_turn_start", { turn: i, mode: "standard" }),
        );
      }
      expect(useDebugStore.getState().events).toHaveLength(500);
    });
  });

  describe("clearEvents", () => {
    it("removes all events", () => {
      useDebugStore.getState().addEvent(makeEvent("agent_turn_start", { turn: 1, mode: "standard" }));
      useDebugStore.getState().clearEvents();
      expect(useDebugStore.getState().events).toHaveLength(0);
    });
  });

  describe("togglePause", () => {
    it("toggles paused state", () => {
      expect(useDebugStore.getState().paused).toBe(false);
      useDebugStore.getState().togglePause();
      expect(useDebugStore.getState().paused).toBe(true);
      useDebugStore.getState().togglePause();
      expect(useDebugStore.getState().paused).toBe(false);
    });
  });

  describe("toggleFilter", () => {
    it("adds a filter type", () => {
      useDebugStore.getState().toggleFilter("agent_turn_start");
      expect(useDebugStore.getState().activeFilters.has("agent_turn_start")).toBe(true);
    });

    it("removes an existing filter", () => {
      useDebugStore.getState().toggleFilter("agent_turn_start");
      useDebugStore.getState().toggleFilter("agent_turn_start");
      expect(useDebugStore.getState().activeFilters.has("agent_turn_start")).toBe(false);
    });
  });

  describe("clearFilters", () => {
    it("removes all filters", () => {
      useDebugStore.getState().toggleFilter("agent_turn_start");
      useDebugStore.getState().toggleFilter("llm_call_start");
      useDebugStore.getState().clearFilters();
      expect(useDebugStore.getState().activeFilters.size).toBe(0);
    });
  });
});
