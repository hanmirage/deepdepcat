/**
 * useDelayedUnmount — keep a component mounted briefly after `show` turns
 * false so a CSS exit animation can play before the element unmounts.
 *
 * Returns `true` while the element should stay mounted. Typical use:
 *
 *   const mounted = useDelayedUnmount(show, 150);
 *   if (!mounted) return null;
 *   return <div className={cn("animate-in ...", !show && "animate-out fade-out ...")}>…</div>
 */

import { useEffect, useState } from "react";

export function useDelayedUnmount(show: boolean, delayMs: number): boolean {
  const [mounted, setMounted] = useState(show);

  useEffect(() => {
    if (show) {
      setMounted(true);
      return;
    }
    const t = window.setTimeout(() => setMounted(false), delayMs);
    return () => window.clearTimeout(t);
  }, [show, delayMs]);

  return mounted;
}
