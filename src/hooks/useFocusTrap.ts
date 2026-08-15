/**
 * useFocusTrap — traps keyboard focus inside a modal container.
 *
 * The decision cards (PermissionDialog / AskUserDialog / PlanApprovalPanel)
 * declare `role="dialog" aria-modal="true"` but are plain divs — without a
 * trap, Tab escapes into the background conversation. This hook:
 *   - remembers the previously focused element (so focus can be restored)
 *   - on mount, focuses the first focusable element inside the container
 *   - traps Tab / Shift+Tab to the container's focusable set
 *   - restores focus to the remembered element on unmount
 *
 * Radix Dialog (ui/dialog.tsx) already has its own focus scope; only the
 * hand-rolled decision cards need this.
 */

import { useEffect, useRef } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function useFocusTrap<T extends HTMLElement>(
  enabled: boolean,
): React.RefObject<T> {
  const ref = useRef<T>(null);
  const prevFocus = useRef<Element | null>(null);

  useEffect(() => {
    if (!enabled) return;
    const container = ref.current;
    if (!container) return;

    prevFocus.current = document.activeElement;

    // Move focus in (first focusable, else the container itself) so Tab
    // starts trapped rather than escaping to the background.
    const first = container.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? container).focus();

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      const focusables = Array.from(
        container.querySelectorAll<HTMLElement>(FOCUSABLE),
      ).filter((el) => el.offsetParent !== null);
      if (focusables.length === 0) {
        e.preventDefault();
        return;
      }
      const firstEl = focusables[0];
      const lastEl = focusables[focusables.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === firstEl || !container.contains(active))) {
        e.preventDefault();
        lastEl.focus();
      } else if (!e.shiftKey && active === lastEl) {
        e.preventDefault();
        firstEl.focus();
      }
    };

    container.addEventListener("keydown", onKeyDown);
    return () => {
      container.removeEventListener("keydown", onKeyDown);
      prevFocus.current instanceof HTMLElement && prevFocus.current.focus();
    };
  }, [enabled]);

  return ref;
}
