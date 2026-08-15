/**
 * Permission store (Zustand).
 *
 * Holds a QUEUE of pending permission requests from the backend. The backend
 * emits one `permission_request` event per tool that needs approval, and it can
 * fire several in a row (parallel agents, rapid tool calls). We keep them in
 * order and show one at a time — resolving the front request unblocks the next.
 *
 * The dialog reads `queue[0]` (the current request); when the user clicks
 * Allow / Always allow / Deny, `respond` sends the decision for that request,
 * then advances to the next.
 */

import { create } from "zustand";
import { logError } from "@/lib/logger";
import type {
  PermissionRequest,
  PermissionDecision,
  PermissionDecisionOptions,
} from "@/types";
import { permissionApi } from "@/lib/tauri";

/** One Auto-Review denial surfaced to the user (override = exact-action
 *  session grant, one-retry semantics). */
export interface AutoReviewDenial {
  session_id: string;
  tool_name: string;
  args: Record<string, unknown>;
  reason: string;
}

/** Backend gives up on an unanswered permission request after 30s
 *  (tools/dispatch.rs) but does NOT tell the frontend — a stale request from
 *  a background session would otherwise sit in the queue forever and pop up
 *  as a dead dialog the moment the user switches back to that session.
 *  Frontend prunes requests older than this. */
const PERMISSION_TTL_MS = 30_000;

interface PermissionState {
  /** Requests waiting for the user's decision, in arrival order. */
  queue: PermissionRequest[];

  /** Enqueue timestamp per request id — drives expiry pruning. */
  enqueuedAt: Record<string, number>;

  /** True while a decision is being sent — ignores further responses. */
  responding: boolean;

  /** Auto-Review denials waiting for a user decision (card queue). */
  denials: AutoReviewDenial[];

  /** Push a request onto the queue (called by the event listener hook). */
  enqueue: (req: PermissionRequest) => void;

  /** Drop requests the backend already timed out on (older than TTL). */
  pruneExpired: () => void;

  /** Send the user's decision for the head-of-queue request and advance.
   *  Pass `requestId` to answer a specific (already visible) request. */
  respond: (
    decision: PermissionDecision,
    opts?: PermissionDecisionOptions,
    requestId?: string,
  ) => Promise<void>;

  /** Push an Auto-Review denial (dedup + cap the queue). */
  enqueueDenial: (denial: AutoReviewDenial) => void;
  /** Remove a denial card. */
  dismissDenial: (key: string) => void;
  /** Override one denial ("仍要允许一次") — exact-action session grant. */
  overrideDenial: (denial: AutoReviewDenial) => Promise<void>;
}

/** Stable identity for a denial — dedup against event re-delivery. */
export function denialKey(d: AutoReviewDenial): string {
  return `${d.session_id}|${d.tool_name}|${JSON.stringify(d.args ?? {})}`;
}

export const usePermissionStore = create<PermissionState>((set, get) => ({
  queue: [],
  enqueuedAt: {},
  responding: false,
  denials: [],

  enqueue: (req) => {
    // Prune stale requests first — a background session's request that
    // already timed out on the backend must not accumulate forever.
    get().pruneExpired();
    set((s) => {
      // Avoid duplicate entries for the same request (e.g. event re-delivery).
      if (s.queue.some((q) => q.request_id === req.request_id)) return s;
      return {
        queue: [...s.queue, req],
        enqueuedAt: { ...s.enqueuedAt, [req.request_id]: Date.now() },
      };
    });
  },

  pruneExpired: () =>
    set((s) => {
      const now = Date.now();
      const fresh = s.queue.filter(
        (q) => (s.enqueuedAt[q.request_id] ?? now) > now - PERMISSION_TTL_MS,
      );
      if (fresh.length === s.queue.length) return s;
      const removed = new Set(
        s.queue.filter((q) => !fresh.includes(q)).map((q) => q.request_id),
      );
      const enqueuedAt = { ...s.enqueuedAt };
      for (const id of removed) delete enqueuedAt[id];
      return { queue: fresh, enqueuedAt };
    }),

  respond: async (decision, opts, requestId) => {
    const req = (requestId
      ? get().queue.find((q) => q.request_id === requestId)
      : get().queue[0]);
    if (!req || get().responding) return;

    // Lock so a double-click / key+button race can't send twice.
    set({ responding: true });

    try {
      await permissionApi.respond(req.request_id, decision, opts);
    } catch (e) {
      logError("permissionStore", "Failed to send decision:", e);
    } finally {
      // Advance: drop the resolved request and show the next one.
      set((s) => {
        const enqueuedAt = { ...s.enqueuedAt };
        delete enqueuedAt[req.request_id];
        return {
          queue: s.queue.filter((q) => q.request_id !== req.request_id),
          responding: false,
          enqueuedAt,
        };
      });
    }
  },

  enqueueDenial: (denial) =>
    set((s) => {
      const key = denialKey(denial);
      if (s.denials.some((d) => denialKey(d) === key)) return s;
      return { denials: [...s.denials, denial].slice(-5) };
    }),

  dismissDenial: (key) =>
    set((s) => ({ denials: s.denials.filter((d) => denialKey(d) !== key) })),

  overrideDenial: async (denial) => {
    try {
      await permissionApi.overrideAutoReviewDenial(
        denial.session_id,
        denial.tool_name,
        denial.args,
      );
    } finally {
      get().dismissDenial(denialKey(denial));
    }
  },
}));

/** Whether a request belongs to the conversation the user is viewing:
 *  its own session id, or the parent session of a subagent request. */
export function requestBelongsToSession(
  req: PermissionRequest,
  sessionId: string | null | undefined,
): boolean {
  if (!sessionId) return true;
  return req.session_id === sessionId || req.parent_session_id === sessionId;
}

/** The requests visible in the given session, in arrival order. Requests
 *  from other (background) sessions are never shown in the foreground
 *  dialog — they keep their own 30s backend timeout instead of hijacking
 *  the current conversation's permission queue. */
export function visiblePermissionRequests(
  queue: PermissionRequest[],
  sessionId: string | null | undefined,
): PermissionRequest[] {
  if (!sessionId) return queue;
  return queue.filter((q) => requestBelongsToSession(q, sessionId));
}

/** Denials visible in the given session (same scoping as permission
 *  requests: subagent denials carry their parent's session). */
export function visibleDenials(
  denials: AutoReviewDenial[],
  sessionId: string | null | undefined,
): AutoReviewDenial[] {
  if (!sessionId) return denials;
  return denials.filter((d) => d.session_id === sessionId);
}

/**
 * Derived selector — the request to display (head of queue). `queue[0]` is a
 * stable reference for a given queue head, so subscribing to it via Zustand's
 * useSyncExternalStore never triggers an infinite re-render.
 */
export function useCurrentPermissionRequest(
  sessionId?: string | null,
): PermissionRequest | null {
  const queue = usePermissionStore((s) => s.queue);
  const visible = visiblePermissionRequests(queue, sessionId);
  return visible.length > 0 ? visible[0] : null;
}
