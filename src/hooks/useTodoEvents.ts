/**
 * useTodoEvents — subscribes to the backend todo pipeline.
 *
 * - Mount/session change: pulls the persisted list once
 *   (`get_session_todos` — restart-safe re-hydration).
 * - `todo-list-updated`: the todo_write tool emitted a live snapshot.
 *
 * All writes go through todoStore so the TaskPanel renders the live state.
 */

import { useEffect } from "react";
import { isTauri, sessionApi, type TodoItem } from "@/lib/tauri";
import { useTodoStore } from "@/stores/todoStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";

/** Live event shape from todo_write.rs (TodoListEvent). */
interface TodoListEvent {
  session_id: string;
  todos: TodoItem[];
}

export function useTodoEvents(sessionId: string | null | undefined) {
  useEffect(() => {
    if (!sessionId) return;

    if (isTauri) {
      // One-shot pull — the persisted list from before a restart.
      void sessionApi
        .getSessionTodos(sessionId)
        .then((todos) => useTodoStore.getState().setSessionTodos(sessionId, todos))
        .catch(() => {
          // Backend unavailable — the event listeners will populate later.
        });
    }

  }, [sessionId]);

  useTauriEvent<TodoListEvent>("todo-list-updated", (e) => {
    if (e.session_id === sessionId) {
      useTodoStore.getState().setSessionTodos(sessionId, e.todos);
    }
  });
}
