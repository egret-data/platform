import { useCallback, useEffect, useRef, useState } from "react";

export interface SqlTabState { id: string; title: string; sql: string; dbId: string | null; limit: number; autoRun?: boolean }

const DEFAULT_LIMIT = 1000;
const newId = () => Math.random().toString(36).slice(2, 10);
const freshTab = (n: number): SqlTabState => ({ id: newId(), title: `Query ${n}`, sql: "SELECT 1", dbId: null, limit: DEFAULT_LIMIT });

function load(ws: string): { tabs: SqlTabState[]; activeId: string } {
  try {
    const tabs = JSON.parse(localStorage.getItem(`sqllab:tabs:${ws}`) || "[]") as SqlTabState[];
    if (Array.isArray(tabs) && tabs.length) {
      const active = localStorage.getItem(`sqllab:active:${ws}`) || tabs[0].id;
      return { tabs, activeId: tabs.some((t) => t.id === active) ? active : tabs[0].id };
    }
  } catch { /* fall through */ }
  const t = freshTab(1);
  return { tabs: [t], activeId: t.id };
}

export function useTabs(ws: string) {
  const [tabs, setTabs] = useState<SqlTabState[]>(() => load(ws).tabs);
  const [activeId, setActiveId] = useState<string>(() => load(ws).activeId);
  const counter = useRef(tabs.length);

  useEffect(() => { localStorage.setItem(`sqllab:tabs:${ws}`, JSON.stringify(tabs)); }, [ws, tabs]);
  useEffect(() => { localStorage.setItem(`sqllab:active:${ws}`, activeId); }, [ws, activeId]);

  const addTab = useCallback(() => {
    counter.current += 1;
    const t = freshTab(counter.current);
    setTabs((cur) => [...cur, t]);
    setActiveId(t.id);
  }, []);
  const closeTab = useCallback((id: string) => {
    setTabs((cur) => {
      if (cur.length <= 1) return cur;
      const next = cur.filter((t) => t.id !== id);
      setActiveId((a) => (a === id ? next[0].id : a));
      return next;
    });
  }, []);
  const renameTab = useCallback((id: string, title: string) => setTabs((cur) => cur.map((t) => (t.id === id ? { ...t, title } : t))), []);
  const patchTab = useCallback((id: string, partial: Partial<SqlTabState>) => setTabs((cur) => cur.map((t) => (t.id === id ? { ...t, ...partial } : t))), []);
  const openTab = useCallback((init: Partial<SqlTabState> & { title: string }) => {
    const t: SqlTabState = { ...freshTab(0), ...init, id: newId() };
    setTabs((cur) => [...cur, t]);
    setActiveId(t.id);
  }, []);

  return { tabs, activeId, setActive: setActiveId, addTab, closeTab, renameTab, patchTab, openTab };
}
