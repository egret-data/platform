import { useCallback, useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";

export interface DashboardViewControls {
  refreshSeconds: number | null;
  setRefreshSeconds: (s: number | null) => void;
  isFullscreen: boolean;
  setFullscreen: (v: boolean) => void;
  refreshNonce: number;
  /** Epoch ms of the last refresh, scheduled or asked for. The countdown in RefreshMenu measures
   *  from here. It changes at most once per period — at the same moment the nonce bumps and the
   *  whole route re-renders anyway — so exposing it costs no render the dashboard was not already
   *  paying for. The per-SECOND tick deliberately lives in RefreshMenu, not here: this hook is
   *  called from DashboardView, which re-renders every card tile. */
  refreshedAt: number;
  /** Re-run every card now. Bumps the SAME nonce the interval bumps, so no consumer has to learn
   *  the difference between a scheduled refresh and an asked-for one. */
  refreshNow: () => void;
}

/** Dashboard view controls synced to the URL: ?refresh=<seconds> and ?fullscreen=true. */
export function useDashboardViewControls(): DashboardViewControls {
  const [params, setParams] = useSearchParams();
  const raw = params.get("refresh");
  const refreshSeconds = raw && /^\d+$/.test(raw) && Number(raw) > 0 ? Number(raw) : null;
  const isFullscreen = params.get("fullscreen") === "true";
  // Nonce and origin are ONE piece of state on purpose: every refresh is both "re-run the card
  // queries" (the nonce, which is a react-query key) and "restart the countdown" (the stamp).
  // Keeping them together makes it impossible to bump one and forget the other.
  const [refresh, setRefresh] = useState(() => ({ nonce: 0, at: Date.now() }));

  const setRefreshSeconds = (s: number | null) =>
    setParams((p) => { const n = new URLSearchParams(p); if (s !== null && s > 0) n.set("refresh", String(s)); else n.delete("refresh"); return n; }, { replace: true });
  const setFullscreen = (v: boolean) =>
    setParams((p) => { const n = new URLSearchParams(p); if (v) n.set("fullscreen", "true"); else n.delete("fullscreen"); return n; }, { replace: true });
  const refreshNow = useCallback(() => setRefresh((r) => ({ nonce: r.nonce + 1, at: Date.now() })), []);

  useEffect(() => {
    if (!refreshSeconds) return;
    // Restart the countdown origin when the period changes: a freshly chosen "5 minutes" must read
    // 5:00, not inherit however much of the period it replaced had already elapsed.
    setRefresh((r) => ({ nonce: r.nonce, at: Date.now() }));
    const t = setInterval(refreshNow, refreshSeconds * 1000);
    return () => clearInterval(t);
  }, [refreshSeconds, refreshNow]);

  return {
    refreshSeconds, setRefreshSeconds, isFullscreen, setFullscreen,
    refreshNonce: refresh.nonce, refreshedAt: refresh.at, refreshNow,
  };
}
