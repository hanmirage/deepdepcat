/**
 * Todo store (Zustand) — per-session task-progress list surfaced by the
 * backend `todo_write` tool.
 *
 * Fed by the `todo-list-updated` backend event (live updates while the
 * agent works) and a one-shot `get_session_todos` pull when a session
 * opens (restart-safe re-hydration). The panel renders from here.
 */

import { create } from "zustand";
import type { TodoItem } from "@/lib/tauri";

interface TodoState {
  /** session_id → todo list (empty/missing = no list). */
  bySession: Record<string, TodoItem[]>;
  /** Replace the whole list for a session (event snapshot / initial pull). */
  setSessionTodos: (sessionId: string, todos: TodoItem[]) => void;
  /** Drop a session's list (session switched away / deleted). */
  clearSession: (sessionId: string) => void;
}

export const useTodoStore = create<TodoState>((set) => ({
  bySession: {},
  setSessionTodos: (sessionId, todos) =>
    set((s) => ({ bySession: { ...s.bySession, [sessionId]: todos } })),
  clearSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.bySession)) return s;
      const bySession = { ...s.bySession };
      delete bySession[sessionId];
      return { bySession };
    }),
}));

/** Stable empty list — selectors must return referentially stable values
 *  or Zustand's subscription loops forever. */
const EMPTY_TODOS: TodoItem[] = [];

/** Selector — the todo list for one session (empty when none). */
export const selectSessionTodos =
  (sessionId: string | null | undefined) =>
  (s: TodoState): TodoItem[] => {
    const list = sessionId ? s.bySession[sessionId] : undefined;
    return list ?? EMPTY_TODOS;
  };
