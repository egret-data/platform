import { useCallback, useState } from "react";
import type { EntityKind, Folder, FolderKind } from "@/api/folders";

export type { EntityKind, Folder, FolderKind };

export interface FolderGroup<T> { folder: Folder | null; rows: T[] }

/** Key for the folderless bucket, whose `FolderGroup.folder` is `null` and so
 *  has no id of its own to key collapse state by. Exported rather than retyped
 *  at each call site: a page that spells it one way in its render loop and
 *  another in its visible-rows derivation would collapse the section on screen
 *  while still treating its rows as selectable — silently, and only for the
 *  folderless bucket. */
export const UNCATEGORIZED_KEY = "__uncat__";

/**
 * Where `useCollapsed` persists which sections a list's visitor left shut.
 *
 * A function here rather than a template literal at each call site, because a
 * page that spells the key one way in its component and another in its test
 * seeds localStorage where nothing reads it — and the test that depends on the
 * seed then PASSES, because a seed that misses is indistinguishable from a
 * page that correctly ignored it. `Endpoints.tsx` had already exported its own
 * one-argument version of exactly this, for exactly that reason; this is that
 * function with the kind lifted into a parameter, and its export is now a thin
 * wrapper, so the seven grouped lists cannot drift apart on the format.
 *
 * `FolderKind`, not `EntityKind`: `data_api_endpoints` has folders without a
 * `PATCH …/{id}/folder` route, so it exists only in the wider type.
 */
export const folderCollapseKey = (ws: string, kind: FolderKind) => `folders:${ws}:${kind}`;

/**
 * Whether a list is drawn with folder SECTION HEADERS at all — which is not
 * the same question as whether it is grouped. [`groupByFolder`] always returns
 * at least the folderless bucket, so a workspace that has never made a folder
 * still gets one group; heading it turns a plain list into "Uncategorized (1)",
 * a collapsible section introducing a taxonomy the workspace does not use.
 * "Uncategorized" only carries information beside a NAMED folder; on its own it
 * labels the whole list after the one thing it is not, and its collapse
 * control's only effect is to hide everything.
 *
 * Read off `folders`, **not** off `groups.length`: with a filter engaged
 * `groupByFolder` drops empty named folders, so a search matching only
 * folderless rows would collapse `groups` to one and make the header blink out
 * mid-keystroke, then back on deletion of the last character.
 *
 * The trash is never sectioned. Folders organise work in progress; a trashed
 * row's folder is what it will go BACK to, not where it is now, and grouping
 * the trash would hide rows behind collapsible headers in the one view whose
 * whole job is to let the author find something they deleted.
 *
 * **Feed the SAME flag to the render loop and to whatever `useBulkSelect` is
 * given.** Those are two derivations of one question, and gating only the
 * render leaves a stale collapse state — `useCollapsed` restores it from
 * localStorage, and it outlives the folder that produced it — hiding rows from
 * "Select all" that are in front of the user, with no header left to expand.
 */
export function showFolderSections(folders: Folder[], showTrash = false): boolean {
  return !showTrash && folders.length > 0;
}

/** Bucket rows by folder_id; folders A→Z, Uncategorized (null) last.
 *  hideEmpty drops empty named folders (used while a search/filter is active). */
export function groupByFolder<T extends { folder_id?: string | null }>(
  rows: T[], folders: Folder[], hideEmpty: boolean,
): FolderGroup<T>[] {
  // A `folder_id` naming a folder that is not in `folders` — deleted, still
  // loading, or another entity type — is not a usable folder, so such a row is
  // Uncategorized. Deciding that HERE, in the same pass that buckets the rest,
  // is what keeps the caller's sort: every consumer sorts before grouping, and
  // these rows take their place among the folderless ones in input order rather
  // than being appended after them. They used to vanish entirely, bucketed under
  // a key the output loop never read, so the row could not be seen, opened, or
  // moved out of the folder that no longer exists.
  const known = new Set(folders.map((f) => f.id));
  const byId = new Map<string, T[]>();
  const uncat: T[] = [];
  for (const r of rows) {
    if (r.folder_id && known.has(r.folder_id)) {
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
