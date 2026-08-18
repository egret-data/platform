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
  const claimed = new Set<string>();
  for (const f of sorted) {
    const fr = byId.get(f.id) ?? [];
    claimed.add(f.id);
    if (hideEmpty && fr.length === 0) continue;
    out.push({ folder: f, rows: fr });
  }
  // Any bucket no folder claimed holds rows whose `folder_id` names a folder
  // that is not in `folders` — deleted, still loading, or another entity type.
  // They used to vanish: bucketed under a key the output loop never read, so the
  // row could not be seen, opened, or moved out of the folder that no longer
  // exists. Uncategorized is where a row with no *usable* folder belongs.
  for (const [id, orphans] of byId) {
    if (!claimed.has(id)) uncat.push(...orphans);
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
  /** Force one key open, idempotently. Used by the "Saved in" deep link: arriving at
   *  ?folder=<id> must reveal that section even when the visitor last left it collapsed.
   *  Returns the same Set reference when nothing changes, so it cannot loop a caller's effect. */
  const expand = useCallback((key: string) => {
    setSet((s) => {
      if (!s.has(key)) return s;
      const n = new Set(s);
      n.delete(key);
      try { localStorage.setItem(storageKey, JSON.stringify([...n])); } catch { /* ignore */ }
      return n;
    });
  }, [storageKey]);
  return { isCollapsed, toggle, expand };
}
