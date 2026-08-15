/**
 * TaskPanel — the code-mode "任务" pane: the agent's plan + progress.
 *
 * A single pane showing the session goal on top and the todo tree below
 * (todo_write output). Pure plan view — execution evidence (subagents) lives
 * in its own SubagentPanel, so no worker rows here.
 *
 * Items: completed = struck through + muted, in_progress = pulsing highlight,
 * pending = plain. Parents are collapsible phases with a done/count badge;
 * high/medium priority items carry a colored dot. The header shows done/total.
 */

import { memo, useState, useEffect, useRef } from "react";
import {
  CheckCircle2,
  Circle,
  Clock,
  ListTodo,
  ChevronDown,
  ChevronRight,
  Target,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useTodoStore, selectSessionTodos } from "@/stores/todoStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { sessionApi, type TodoItem } from "@/lib/tauri";
import { cn } from "@/lib/utils";

interface TaskPanelProps {
  sessionId: string | null | undefined;
  /** Cap the number of visible roots (overflow collapses into a count). */
  maxRows?: number;
}

/** Cap on visible todo roots — overflow shows as a trailing count. */
const MAX_ROWS = 8;

/** Priority → dot color (only high/medium carry a dot; low is unmarked). */
const PRIORITY_DOT: Record<string, string> = {
  high: "bg-red-500",
  medium: "bg-amber-500",
};

type TodoTree = {
  roots: TodoItem[];
  children: Map<string, TodoItem[]>;
  byId: Map<string, TodoItem>;
};

/** Whether `item`'s parent chain is acyclic — a cycle (A→B→A) would recurse
 *  forever in the render tree, so cyclic parents fall back to the root level. */
function isSafeParent(item: TodoItem, byId: Map<string, TodoItem>): boolean {
  const seen = new Set<string>();
  let cur = item.parent_id;
  while (cur) {
    if (cur === item.id || seen.has(cur)) return false;
    seen.add(cur);
    const parent = byId.get(cur);
    if (!parent) return true; // chain ended at a missing parent — not a cycle
    cur = parent.parent_id;
  }
  return true;
}

/** Build a todo tree from a flat list: `parent_id` children hang under their
 *  parent; orphans (missing/unknown parent) and cyclic parents fall back to
 *  the root level. */
function buildTree(items: TodoItem[]): TodoTree {
  const byId = new Map(items.map((t) => [t.id, t]));
  const children = new Map<string, TodoItem[]>();
  const roots: TodoItem[] = [];
  for (const t of items) {
    const parent = t.parent_id && byId.has(t.parent_id) && isSafeParent(t, byId) ? t.parent_id : null;
    if (parent) {
      const arr = children.get(parent) ?? [];
      arr.push(t);
      children.set(parent, arr);
    } else {
      roots.push(t);
    }
  }
  return { roots, children, byId };
}

/** One todo tree node — phases (parents) collapse and show subtree progress;
 *  children indent under their phase. */
function TodoNode({ item, tree, depth }: { item: TodoItem; tree: TodoTree; depth: number }) {
  const [collapsed, setCollapsed] = useState(false);
  // Depth backstop — a cycle escaped the isSafeParent guard (or a deeply
  // nested list came in) must not recurse forever.
  if (depth > 10) return null;
  const children = tree.children.get(item.id) ?? [];
  const hasChildren = children.length > 0;
  const isDone = item.status === "completed";
  const isActive = item.status === "in_progress";
  const childDone = children.filter((c) => c.status === "completed").length;
  const priorityDot = item.priority ? PRIORITY_DOT[item.priority] : null;
  const unmetDeps = (item.depends_on ?? []).filter((id) => {
    const dep = tree.byId.get(id);
    return !dep || dep.status !== "completed";
  });
  return (
    <li>
      <div
        className={cn(
          "flex items-start gap-1.5 text-[11px] leading-snug",
          isDone ? "text-muted-foreground/45" : "text-foreground/80",
        )}
        style={{ paddingLeft: depth * 12 }}
      >
        {hasChildren ? (
          <button
            onClick={() => setCollapsed((c) => !c)}
            className="mt-0.5 shrink-0"
            aria-expanded={!collapsed}
          >
            {collapsed ? (
              <ChevronRight className="h-3 w-3 text-muted-foreground/60" />
            ) : (
              <ChevronDown className="h-3 w-3 text-muted-foreground/60" />
            )}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}
        {isDone ? (
          <CheckCircle2 className="mt-0.5 h-3 w-3 shrink-0 text-green-600 dark:text-green-400" />
        ) : isActive ? (
          <Circle className="mt-0.5 h-3 w-3 shrink-0 animate-pulse text-primary" />
        ) : (
          <Circle className="mt-0.5 h-3 w-3 shrink-0 text-muted-foreground/40" />
        )}
        {priorityDot && (
          <span
            className={cn("mt-[5px] h-1.5 w-1.5 shrink-0 rounded-full", priorityDot)}
            title={item.priority}
          />
        )}
        <span className="min-w-0 flex-1">
          <span className={cn("break-words", isDone && "line-through")}>
            {item.content}
          </span>
          {item.verify && (
            <span className="mt-0.5 block truncate font-mono text-[9px] text-muted-foreground/50">
              verify: {item.verify}
            </span>
          )}
          {!isDone && unmetDeps.length > 0 && (
            <span
              className={cn(
                "mt-0.5 flex items-center gap-1 font-mono text-[9px] leading-tight",
                isActive
                  ? "text-amber-600 dark:text-amber-400"
                  : "text-muted-foreground/50"
              )}
            >
              <Clock className="h-2.5 w-2.5 shrink-0" />
              <span className="truncate">
                {isActive ? "依赖未完成" : "等待"} {unmetDeps.join(", ")}
              </span>
            </span>
          )}
        </span>
        {hasChildren && (
          <span className="ml-auto shrink-0 rounded bg-muted px-1 py-0.5 font-mono text-[9px] tabular-nums text-muted-foreground/60">
            {childDone}/{children.length}
          </span>
        )}
      </div>
      {hasChildren && !collapsed && (
        <ul className="mt-0.5 space-y-0.5">
          {children.map((c) => (
            <TodoNode key={c.id} item={c} tree={tree} depth={depth + 1} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** The session goal card above the todo tree. */
function GoalCard({ goal }: { goal: string }) {
  return (
    <div className="flex items-start gap-1.5 rounded-md bg-primary/5 px-2 py-1.5">
      <Target className="mt-0.5 h-3 w-3 shrink-0 text-primary" />
      <span className="min-w-0 flex-1 text-[11px] leading-snug text-foreground/80">
        {goal}
      </span>
    </div>
  );
}

/** The collapsible todo tree with its done/total header badge. */
function TodoListSection({
  tree,
  todos,
  expanded,
  onToggle,
  maxRows = MAX_ROWS,
}: {
  tree: TodoTree;
  todos: TodoItem[];
  expanded: boolean;
  onToggle: () => void;
  maxRows?: number;
}) {
  const { t } = useTranslation();
  const done = todos.filter((x) => x.status === "completed").length;
  const shownRoots = tree.roots.slice(0, maxRows);
  const overflow = tree.roots.length - shownRoots.length;

  return (
    <>
      <button
        onClick={onToggle}
        aria-expanded={expanded}
        className="flex w-full items-center gap-2 text-left"
      >
        {expanded ? (
          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground/60" />
        ) : (
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground/60" />
        )}
        <ListTodo className="h-3.5 w-3.5 shrink-0 text-muted-foreground/60" />
        <span className="shrink-0 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
          {t("chat.todoList", { defaultValue: "任务清单" })}
        </span>
        <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-muted-foreground/70">
          {done}/{todos.length}
        </span>
      </button>

      {expanded && todos.length > 0 && (
        <>
          <ul className="space-y-0.5">
            {shownRoots.map((root) => (
              <TodoNode key={root.id} item={root} tree={tree} depth={0} />
            ))}
          </ul>
          {overflow > 0 && (
            <p className="text-[10px] text-muted-foreground/50">
              {t("chat.todoMore", { count: overflow, defaultValue: "…还有 {{count}} 项" })}
            </p>
          )}
        </>
      )}
    </>
  );
}

function TaskPanelImpl({ sessionId, maxRows = MAX_ROWS }: TaskPanelProps) {
  const { t } = useTranslation();
  const todos = useTodoStore(selectSessionTodos(sessionId));
  const [goal, setGoal] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(true);
  // The session this panel's goal fetches are FOR. Each request captures the
  // id at call time and drops the response if the panel has moved on — a
  // single boolean alive-flag is NOT enough (switching sessions re-arms it
  // before the old session's late response arrives, letting it overwrite).
  const goalSessionRef = useRef<string | null | undefined>(null);

  // Session goal — fetch on session change, refresh when the plan updates.
  useEffect(() => {
    goalSessionRef.current = sessionId;
    if (!sessionId) {
      setGoal(null);
      return;
    }
    void sessionApi
      .getGoal(sessionId)
      .then((g) => {
        if (goalSessionRef.current === sessionId) setGoal(g);
      })
      .catch(() => {
        /* best-effort */
      });
  }, [sessionId]);
  useTauriEvent<{ session_id: string }>("todo-list-updated", (e) => {
    if (!sessionId || e.session_id !== sessionId) return;
    void sessionApi
      .getGoal(sessionId)
      .then((g) => {
        if (goalSessionRef.current === sessionId) setGoal(g);
      })
      .catch(() => {});
  });

  if (todos.length === 0 && !goal) {
    return (
      <p className="px-1 py-2 text-[11px] text-muted-foreground/60">
        {t("task.empty")}
      </p>
    );
  }

  const tree = buildTree(todos);

  return (
    <div className="space-y-2 p-3">
      {goal && <GoalCard goal={goal} />}
      <TodoListSection
        tree={tree}
        todos={todos}
        expanded={expanded}
        onToggle={() => setExpanded((e) => !e)}
        maxRows={maxRows}
      />
    </div>
  );
}

export const TaskPanel = memo(TaskPanelImpl);
