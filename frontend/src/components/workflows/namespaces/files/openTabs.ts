/**
 * The namespace page's open file tabs, remembered per workspace and
 * namespace in `localStorage` (W7d spec §4.3). They are a viewer preference
 * (FE-DATA-3): paths and which one is in front, never a file's content.
 * Every access is wrapped, so the page works without storage.
 */
export interface OpenTabs {
  tabs: string[];
  active: string | null;
}

export const OPEN_TABS_KEY_PREFIX = "egret.wf.ns-files.tabs.";

const keyOf = (ws: string, namespace: string) => `${OPEN_TABS_KEY_PREFIX}${ws}.${namespace}`;

export function readOpenTabs(ws: string, namespace: string): OpenTabs {
  try {
    const raw = localStorage.getItem(keyOf(ws, namespace));
    if (!raw) return { tabs: [], active: null };
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return { tabs: [], active: null };
    const { tabs, active } = parsed as { tabs?: unknown; active?: unknown };
    const paths = Array.isArray(tabs) ? [...new Set(tabs.filter((t): t is string => typeof t === "string" && t.startsWith("/") && !t.endsWith("/")))] : [];
    return { tabs: paths, active: typeof active === "string" && paths.includes(active) ? active : (paths[0] ?? null) };
  } catch {
    return { tabs: [], active: null };
  }
}

export function writeOpenTabs(ws: string, namespace: string, state: OpenTabs): void {
  try {
    if (state.tabs.length === 0) localStorage.removeItem(keyOf(ws, namespace));
    else localStorage.setItem(keyOf(ws, namespace), JSON.stringify(state));
  } catch {
    // Storage full or blocked: the tabs are simply not remembered.
  }
}
