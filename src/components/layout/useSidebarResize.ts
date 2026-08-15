/**
 * useSidebarResize — pointer-captured width drag for the app sidebar.
 *
 * Same pattern as usePanelResize (right panel): delta-based drag math that
 * stores the pointer start + starting width so the new width is
 * `start + (clientX - startX)` — direction-correct regardless of where the
 * sidebar sits in the viewport. Pointer capture routes every move/up to the
 * handle even when the pointer leaves the element, so no drag ever sticks.
 *
 * Collapsed-rail drags expand once the pointer crosses the minimum width;
 * dragging an expanded sidebar below the minimum collapses it back to the
 * rail.
 */

import { useCallback, useRef, useState, type PointerEvent } from "react";

const MIN_SIDEBAR_WIDTH = 200;
const MAX_SIDEBAR_WIDTH = 400;
const DEFAULT_SIDEBAR_WIDTH = 200;
/** Narrow icon-rail width when the sidebar is collapsed. */
const COLLAPSED_SIDEBAR_WIDTH = 48;

export function useSidebarResize(
  sidebarCollapsed: boolean,
  setSidebarCollapsed: (collapsed: boolean) => void,
) {
  const [sidebarWidth, setSidebarWidth] = useState(DEFAULT_SIDEBAR_WIDTH);
  const isDragging = useRef(false);
  const dragStart = useRef({ x: 0, width: DEFAULT_SIDEBAR_WIDTH });
  // Disable the width transition while dragging — otherwise every mousemove
  // triggers a 200ms animation that lags behind the cursor and feels janky.
  const [dragActive, setDragActive] = useState(false);
  const effectiveWidth = sidebarCollapsed ? COLLAPSED_SIDEBAR_WIDTH : sidebarWidth;

  const endDrag = useCallback((e?: PointerEvent<HTMLDivElement>) => {
    isDragging.current = false;
    setDragActive(false);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
    if (e && e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }, []);

  const handlePointerDown = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      isDragging.current = true;
      // Baseline is the width actually on screen — in the collapsed rail that
      // is the 48px rail, so drag-to-expand starts from a sensible point.
      dragStart.current = { x: e.clientX, width: effectiveWidth };
      setDragActive(true);
      e.currentTarget.setPointerCapture(e.pointerId);
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
    },
    [effectiveWidth],
  );

  const handlePointerMove = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if (!isDragging.current) return;
      const newWidth = dragStart.current.width + (e.clientX - dragStart.current.x);
      if (newWidth >= MIN_SIDEBAR_WIDTH) {
        // Drag right past the minimum → resize, and open from the collapsed
        // rail (drag-to-expand). setSidebarCollapsed(false) is a no-op when
        // already expanded, so it's safe to call on every move.
        setSidebarWidth(Math.min(newWidth, MAX_SIDEBAR_WIDTH));
        setSidebarCollapsed(false);
      } else if (!sidebarCollapsed) {
        // Expanded but dragged below the minimum → collapse to the icon rail.
        // Reset drag state AND release pointer capture before the rail
        // re-renders so body styles and flags never stick.
        setSidebarCollapsed(true);
        endDrag(e);
      }
      // Already collapsed and still below the minimum → keep dragging; the
      // rail stays put until the drag crosses the minimum and expands.
    },
    [setSidebarCollapsed, endDrag, sidebarCollapsed],
  );

  const handlePointerUp = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if (!isDragging.current) return;
      endDrag(e);
    },
    [endDrag],
  );

  return { effectiveWidth, dragActive, handlePointerDown, handlePointerMove, handlePointerUp };
}
