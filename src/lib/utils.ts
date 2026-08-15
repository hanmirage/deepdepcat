/**
 * Utility helpers shared across the app.
 */

import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * Merge Tailwind CSS class names intelligently.
 * Combines `clsx` (conditional classes) with `twMerge` (conflict resolution).
 *
 * @example cn("px-2", condition && "bg-red-500", "px-4") → "bg-red-500 px-4"
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * Whether a keydown event originated from an editable element (input,
 * textarea, select, contenteditable) or an IME composition.
 *
 * Global window keydown handlers (permission dialog, plan approval, …) MUST
 * guard with this: pressing Enter inside a text field is typing, not a
 * shortcut — without the guard, a user typing feedback could accidentally
 * approve a tool call or a plan.
 */
export function isEditableKeyEvent(e: KeyboardEvent): boolean {
  if (e.isComposing) return true;
  const target = e.target;
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  // isContentEditable is not implemented in jsdom — check the attribute
  // directly so tests and runtime agree.
  if (target.isContentEditable === true) return true;
  const contenteditable = target.getAttribute("contenteditable");
  return contenteditable !== null && contenteditable !== "false";
}

/**
 * Format a timestamp to a relative "time ago" string.
 * e.g. "2 min ago", "1 hour ago", "Just now"
 *
 * `t` (optional) localizes the units — pass the i18n `t` function and the
 * result uses the app language; without it the English fallback is used
 * (keeps the pure function testable without a translation context).
 */
export function timeAgo(
  date: string | number | Date,
  t?: (key: string, opts?: Record<string, unknown>) => string,
): string {
  const now = Date.now();
  const then = new Date(date).getTime();
  const diff = Math.floor((now - then) / 1000);

  if (diff < 10) return t?.("sidebar.timeJustNow") ?? "Just now";
  if (diff < 60) return t?.("sidebar.timeSeconds", { count: diff }) ?? `${diff}s ago`;
  const minutes = Math.floor(diff / 60);
  if (minutes < 60)
    return t?.("sidebar.timeMinutes", { count: minutes }) ?? `${minutes}m ago`;
  const hours = Math.floor(diff / 3600);
  if (hours < 24) return t?.("sidebar.timeHours", { count: hours }) ?? `${hours}h ago`;
  const days = Math.floor(diff / 86400);
  return t?.("sidebar.timeDays", { count: days }) ?? `${days}d ago`;
}

/**
 * Shorten a long path for display.
 * e.g. "/Users/hanzi/Projects/DeepDepCat" → "~/Projects/DeepDepCat"
 */
export function shortPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, "~").replace(/^\/home\/[^/]+/, "~");
}

/**
 * Format a timestamp to HH:mm time string.
 * e.g. 14:30, 09:05
 */
export function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  const hours = date.getHours().toString().padStart(2, "0");
  const minutes = date.getMinutes().toString().padStart(2, "0");
  return `${hours}:${minutes}`;
}

/** Conversation-day grouping for the message-list dividers. */
export type DayGroup = "today" | "yesterday" | "earlier";

/** Classify a timestamp into today / yesterday / earlier (local days). */
export function dayGroupLabel(timestamp: number, now = Date.now()): DayGroup {
  const dayStart = (x: Date) =>
    new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diffDays = Math.round(
    (dayStart(new Date(now)) - dayStart(new Date(timestamp))) / 86_400_000,
  );
  if (diffDays <= 0) return "today";
  if (diffDays === 1) return "yesterday";
  return "earlier";
}
