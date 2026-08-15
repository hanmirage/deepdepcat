/**
 * useAutoReviewEvents — subscribes to backend Auto-Review denial events.
 *
 * The backend emits `auto-review-denied` whenever the independent reviewer
 * (or its circuit breaker) refuses a gray-zone action. The payload carries
 * the exact tool + args so the user can override with a one-retry session
 * grant.
 */

import { usePermissionStore, type AutoReviewDenial } from "@/stores/permissionStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";

export function useAutoReviewEvents() {
  const enqueue = usePermissionStore((s) => s.enqueueDenial);

  useTauriEvent<AutoReviewDenial>("auto-review-denied", (event) => {
    enqueue(event);
  });
}
