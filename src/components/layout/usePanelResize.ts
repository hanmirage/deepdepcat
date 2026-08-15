import { useCallback, useRef, type PointerEvent } from "react";

import {
  MIN_RIGHT_PANEL_WIDTH,
  MAX_RIGHT_PANEL_WIDTH,
} from "@/stores/rightPanelStore";

/**
 * Pointer-captured width drag (same pattern as the sidebar handle). The
 * caller clamps/persists via `onWidthChange`; the drag never leaves a stuck
 * state because every pointer-up/cancel path releases capture and resets
 * the body cursor.
 */
export function usePanelResize(
  width: number,
  onWidthChange: (width: number) => void,
) {
  const isDragging = useRef(false);
  const dragStart = useRef({ x: 0, width });

  const endDrag = useCallback((e?: PointerEvent<HTMLDivElement>) => {
    isDragging.current = false;
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
    if (e && e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }, []);

  const handlePointerDown = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      isDragging.current = true;
      dragStart.current = { x: e.clientX, width };
      e.currentTarget.setPointerCapture(e.pointerId);
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
    },
    [width],
  );

  const handlePointerMove = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if (!isDragging.current) return;
      const newWidth =
        dragStart.current.width + (dragStart.current.x - e.clientX);
      onWidthChange(
        Math.min(MAX_RIGHT_PANEL_WIDTH, Math.max(MIN_RIGHT_PANEL_WIDTH, newWidth)),
      );
    },
    [onWidthChange],
  );

  const handlePointerUp = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if (!isDragging.current) return;
      endDrag(e);
    },
    [endDrag],
  );

  return { handlePointerDown, handlePointerMove, handlePointerUp };
}
