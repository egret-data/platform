import { useCallback, useState } from "react";
import type { EntityKind, Folder } from "@/api/folders";

export type { EntityKind, Folder };

export interface FolderGroup<T> { folder: Folder | null; rows: T[] }

/** Bucket rows by folder_id; folders A→Z, Uncategorized (null) last.
 *  hideEmpty drops empty named folders (used while a search/filter is active). */
export function groupByFolder<T extends { folder_id?: string | null }>(
  rows: T[], folders: Folder[], hideEmpty: boolean,
): FolderGroup<T>[] {
  const byId = new Map<string, T[]>();
  const uncat: T[] = [];
  for (const r of rows) {
    if (r.folder_id) {
      const arr = byId.get(r.folder_id) ?? [];
      arr.push(r);
      byId.set(r.folder_id, arr);
    } else {
      uncat.push(r);
    }
  }
  const sorted = [...folders].sort((a, b) => a.name.localeCompare(b.name));
  const out: FolderGroup<T>[] = [];
  for (const f of sorted) {
    const fr = byId.get(f.id) ?? [];
    if (hideEmpty && fr.length === 0) continue;
    out.push({ folder: f, rows: fr });
  }
  if (uncat.length > 0 || !hideEmpty) out.push({ folder: null, rows: uncat });
  return out;
}

/** Per-list collapse state persisted in localStorage. Default expanded. */
export function useCollapsed(storageKey: string) {
  const [set, setSet] = useState<Set<string>>(() => {
    try { return new Set(JSON.parse(localStorage.getItem(storageKey) ?? "[]")); } catch { return new Set(); }
  });
  const isCollapsed = useCallback((key: string) => set.has(key), [set]);
  const toggle = useCallback((key: string) => {
    setSet((s) => {
      const n = new Set(s);
      if (n.has(key)) n.delete(key); else n.add(key);
      try { localStorage.setItem(storageKey, JSON.stringify([...n])); } catch { /* ignore */ }
      return n;
    });
  }, [storageKey]);
  return { isCollapsed, toggle };
}
