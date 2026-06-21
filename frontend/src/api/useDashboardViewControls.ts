import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";

export interface DashboardViewControls {
  refreshSeconds: number | null;
  setRefreshSeconds: (s: number | null) => void;
  isFullscreen: boolean;
  setFullscreen: (v: boolean) => void;
  refreshNonce: number;
}

/** Dashboard view controls synced to the URL: ?refresh=<seconds> and ?fullscreen=true. */
export function useDashboardViewControls(): DashboardViewControls {
  const [params, setParams] = useSearchParams();
  const raw = params.get("refresh");
  const refreshSeconds = raw && /^\d+$/.test(raw) && Number(raw) > 0 ? Number(raw) : null;
  const isFullscreen = params.get("fullscreen") === "true";
  const [refreshNonce, setRefreshNonce] = useState(0);

  const setRefreshSeconds = (s: number | null) =>
    setParams((p) => { const n = new URLSearchParams(p); if (s) n.set("refresh", String(s)); else n.delete("refresh"); return n; }, { replace: true });
  const setFullscreen = (v: boolean) =>
    setParams((p) => { const n = new URLSearchParams(p); if (v) n.set("fullscreen", "true"); else n.delete("fullscreen"); return n; }, { replace: true });

  useEffect(() => {
    if (!refreshSeconds) return;
    const t = setInterval(() => setRefreshNonce((n) => n + 1), refreshSeconds * 1000);
    return () => clearInterval(t);
  }, [refreshSeconds]);

  return { refreshSeconds, setRefreshSeconds, isFullscreen, setFullscreen, refreshNonce };
}
