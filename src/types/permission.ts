/**
 * Permission types — mirror the Rust backend's permission structs.
 *
 * Backend uses `#[serde(rename_all = "snake_case")]` so TypeScript
 * sees snake_case field names and string values.
 */

/** User's decision on a permission request.
 *  Mirrors `agent::types::PermissionDecision` (serde snake_case).
 *  `always_allow` records a durable grant; `session_allow` records a
 *  grant scoped to the current session only. */
export type PermissionDecision = "allow" | "always_allow" | "session_allow" | "deny";

/** Extra options for a permission decision.
 *  `scope` picks what an "always/session allow" remembers: the exact
 *  pattern shown in the dialog, or the whole tool (`*`). `reason` is an
 *  optional rejection message fed back to the agent. */
export interface PermissionDecisionOptions {
  scope?: "pattern" | "tool";
  reason?: string;
}

/** Permission mode — controls how the backend handles tool calls.
 *  Mirrors `state::PermissionMode`. Values are the exact strings sent to
 *  `set_permission_mode`; the Rust side maps them to its 3-variant mode. */
export type PermissionMode = "read_only" | "accept_edits" | "full_access";

/** A permission request emitted by the backend before a tool executes.
 *  Mirrors the Rust `PermissionRequest` (session.rs) that the backend
 *  actually emits via the `permission_request` event:
 *    request_id, tool_name, args_summary, session_id, grant_pattern,
 *    grant_scope.
 *  The ACP path's richer camelCase fields are kept optional for forward
 *  compatibility — PermissionDialog falls back to args_summary when they're
 *  absent. */
export interface PermissionRequest {
  request_id: string;
  tool_name: string;
  args_summary: string;
  session_id: string;
  /** The session that spawned the executing agent as a SUBAGENT — lets the
   *  dialog route the prompt to the parent conversation the user is
   *  actually looking at (`undefined` for a main session). */
  parent_session_id?: string;
  /** The grant identity an "always allow" would record (`cmd:git`,
   *  `path:...`, `mcp:server`, or `*`). Optional for ACP-path requests. */
  grant_pattern?: string;
  /** Human-readable scope of `grant_pattern`, shown before the user
   *  commits to "always allow". */
  grant_scope?: string;
  /** Optional — set only by the ACP event path. */
  agent_name?: string;
  tool_call_id?: string;
  summary?: string;
  detail?: string;
}

/** A plan submitted by the agent via `exit_plan_mode` — parked until the
 *  user approves or rejects it (backend emits `plan-approval-request`). */
export interface PlanApprovalRequest {
  request_id: string;
  session_id: string;
  /** The plan text the agent wrote (rendered verbatim). */
  plan: string;
  /** Workspace git-change summary at submission time ("" lines). */
  changed_files: string[];
  /** Unix seconds when the plan was submitted. */
  created_at: number;
}

/** A user interaction the agent is parked on — "waiting for you" status
 *  (backend emits `pending-interactions` with the full per-session list). */
export interface PendingInteraction {
  kind: "permission" | "plan" | "question";
  request_id: string;
  summary: string;
  since: number;
}
