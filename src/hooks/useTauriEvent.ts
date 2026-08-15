/**
 * useTauriEvent — race-safe subscription to a Tauri/backend event.
 *
 * Replaces the hand-rolled `onEvent(...).then(fn => ...)` lifecycle that
 * was copied (and occasionally inverted) across a dozen hooks/components:
 *   - the listener stays registered until unmount / `enabled` flips false
 *   - unmounting BEFORE the subscription resolves cancels it immediately
 *     (no leaked listener, no setState after unmount)
 *   - the handler is read through a ref, so it always sees the latest
 *     closure without re-subscribing on every render
 *
 * Usage:
 *   useTauriEvent("chat-stream", handler);                    // onEvent
 *   useTauriEvent(subscribeFn, handler, enabled);             // custom
 */

import { useEffect, useRef } from "react";
import { onEvent } from "@/lib/tauri";

export type TauriEventSubscribe<T> = (
  handler: (payload: T) => void,
) => Promise<() => void>;

export function useTauriEvent<T>(
  eventName: string,
  handler: (payload: T) => void,
  enabled?: boolean,
): void;
export function useTauriEvent<T>(
  subscribe: TauriEventSubscribe<T>,
  handler: (payload: T) => void,
  enabled?: boolean,
): void;
export function useTauriEvent<T>(
  eventOrSubscribe: string | TauriEventSubscribe<T>,
  handler: (payload: T) => void,
  enabled = true,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    if (!enabled) return;
    const subscribe =
      typeof eventOrSubscribe === "string"
        ? (h: (payload: T) => void) => onEvent<T>(eventOrSubscribe, h)
        : eventOrSubscribe;

    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void subscribe((payload) => {
      if (cancelled) return;
      handlerRef.current(payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [eventOrSubscribe, enabled]);
}
