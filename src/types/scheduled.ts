/**
 * Scheduled agent tasks (定时任务) — mirrors Rust structs in
 * `src-tauri/src/automation/`.
 */

/** Schedule specification — tagged union, matches Rust ScheduleSpec. */
export type ScheduleSpec =
  | { kind: "interval"; every_secs: number }
  | { kind: "daily"; time: string };

/** A persisted scheduled agent task. */
export interface ScheduledTask {
  id: string;
  name: string;
  prompt: string;
  schedule: ScheduleSpec;
  /** Working directory; empty = the current workspace. */
  project_path: string;
  /** Run in an isolated git worktree (git repos only). */
  use_worktree: boolean;
  /** Persistent mode: the agent reuses one session across fires (常驻). */
  persistent: boolean;
  /** The session a persistent agent owns (null before its first fire). */
  persistent_session_id: string | null;
  work_mode: "code" | "depwork";
  /** Model override; empty = configured default. */
  model: string;
  active: boolean;
  last_run_at_ms: number | null;
  run_count: number;
  created_at: string;
  updated_at: string;
}

export type ScheduledRunStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "skipped"
  | "cancelled";

/** One execution record (the Scheduled inbox). */
export interface ScheduledRun {
  id: string;
  task_id: string;
  session_id: string | null;
  status: ScheduledRunStatus;
  started_at: string;
  finished_at: string | null;
  summary: string;
  error: string;
  worktree_path: string;
}
