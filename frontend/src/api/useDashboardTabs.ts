import { useCallback } from "react";
import { useSearchParams } from "react-router-dom";

/** URL-synced active dashboard tab, addressed by position (`?tab=<n>`). Clamped to
 *  [0, max(0, tabCount-1)] so a stale/out-of-range link never selects a missing tab. */
export function useDashboardTabs(tabCount: number) {
  const [params, setParams] = useSearchParams();
  const maxPos = Math.max(0, tabCount - 1);
  const raw = Number(params.get("tab"));
  const activeTabPos = Number.isFinite(raw) && raw > 0 ? Math.min(raw, maxPos) : 0;

  const setActiveTabPos = useCallback((pos: number) => {
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      if (pos <= 0) next.delete("tab");
      else next.set("tab", String(pos));
      return next;
    }, { replace: true });
  }, [setParams]);

  return { activeTabPos, setActiveTabPos };
}
