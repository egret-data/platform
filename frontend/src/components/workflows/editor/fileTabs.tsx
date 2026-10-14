import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import type { Enablement } from "@/components/workflows/draftState";
// A type only: the hook itself lives in the Files chunk (`editor/files/FileSessions.tsx`).
import type { NamespaceFile } from "@/components/workflows/namespaces/files/useNamespaceFile";
import { closeTab, openTab, renameTabs, type Layout, type TabId } from "@/components/workflows/panels/layout";

/**
 * THE FLOW EDITOR'S NAMESPACE FILES (W7d spec §4.3, "In the flow editor"):
 * each open file is a tab `file:<path>` of the multi-panel frame, beside
 * Code. This module is the light half, in the flow pages' chunk: tab ids,
 * where each open file's state is kept, and the page's handle on them. The
 * explorer, the file editor and Monaco load with the tabs' bodies
 * (`editor/files/`), through `lazy()` only (`scripts/assert-workflows-lazy.mjs`).
 */

/** The tab id prefix of a namespace file (W7d spec §4.3). */
export const FILE_TAB_PREFIX = "file:";

/** `file:scripts/a.py` for the stored path `/scripts/a.py`. */
export const fileTabId = (path: string): TabId => `${FILE_TAB_PREFIX}${path.replace(/^\/+/, "")}`;

/** The stored path (`/scripts/a.py`) of a file tab, or null for any other tab. */
export function filePathOf(tab: TabId): string | null {
  return tab.startsWith(FILE_TAB_PREFIX) && tab.length > FILE_TAB_PREFIX.length ? `/${tab.slice(FILE_TAB_PREFIX.length)}` : null;
}

/** `a.py` of `/scripts/a.py`. */
export const fileNameOf = (path: string) => path.slice(path.lastIndexOf("/") + 1);

const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

/** "a.py", "a.py and b.py", "a.py, b.py and c.py". */
export function nameList(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** `layout` without any file tab. */
function withoutFileTabs(layout: Layout): Layout {
  let next = layout;
  for (const tab of layout.panels.flatMap((p) => p.tabs)) if (filePathOf(tab) !== null) next = closeTab(next, tab);
  return next;
}

/**
 * Each open file's state, by stored path: a `useNamespaceFile` per open file
 * tab, kept by `FileSessions` (mounted by the page, not by the tab). The frame
 * mounts only each panel's selected tab (W7a §5.1, Mounting), so a file's
 * unsaved text cannot live in its tab: here it survives the tab being hidden,
 * moved or split, and "Save all files" reaches files whose tab is not shown.
 */
export class FileSessionStore {
  private sessions = new Map<string, NamespaceFile>();
  private listeners = new Set<() => void>();
  private version = 0;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getVersion = () => this.version;

  get = (path: string): NamespaceFile | undefined => this.sessions.get(path);

  entries(): [string, NamespaceFile][] {
    return [...this.sessions.entries()];
  }

  /**
   * Keep `file` for `path`. A value equal field by field to the one kept
   * changes nothing: `useNamespaceFile` returns a new object every render,
   * with stable callbacks, so this is what stops a render loop.
   */
  set(path: string, file: NamespaceFile): void {
    const prev = this.sessions.get(path);
    if (prev && (Object.keys(file) as (keyof NamespaceFile)[]).every((k) => Object.is(prev[k], file[k]))) return;
    this.sessions.set(path, file);
    this.bump();
  }

  delete(path: string): void {
    if (this.sessions.delete(path)) this.bump();
  }

  private bump() {
    this.version++;
    for (const l of this.listeners) l();
  }
}

/** What the Files tab and the file tabs get from the page, as `EditorContext.files`. */
export type FilesHost = {
  ws: string;
  /** The flow's namespace, as saved. */
  namespace: string;
  /** No `workflow:manage`: files open read-only and nothing is written (W7d spec §4.5). */
  readOnly: boolean;
  store: FileSessionStore;
  /** Open `path` as a tab in the panel holding Code, or select its tab. */
  open: (path: string) => void;
  /** Close `path`'s tab, asking first when it has unsaved edits. */
  close: (path: string) => void;
  /** The tree moved `from` (a file, or a folder ending in `/`) to `to`: its open tabs follow. */
  moved: (from: string, to: string) => void;
  /** The tree deleted `path` (a file, or a folder): its open tabs close. */
  deleted: (path: string) => void;
  /** The open files with unsaved edits, by stored path. */
  dirtyPaths: readonly string[];
};

type FlowLayoutHandle = { layout: Layout; setLayout: (next: Layout) => void; revealCode: () => void };

export type FilesHostState = {
  host: FilesHost;
  /** The stored paths the layout has a tab for, in layout order. */
  openPaths: string[];
  /** The page's layout handle, whose `setLayout` asks before closing a file tab with unsaved edits. */
  flowLayout: FlowLayoutHandle;
  /** Resolves true when every file with unsaved edits was saved. */
  saveAll: () => Promise<boolean>;
  saveAllState: Enablement;
  /** The files with unsaved edits a close waits on ("Discard and close"), by stored path; null when none. */
  closing: string[] | null;
  confirmClose: () => void;
  cancelClose: () => void;
};

/**
 * The page's side of the file tabs. Flow Save and file save stay apart (W7d
 * spec §4.3): nothing here touches the flow, and the flow's Save touches no
 * file. "Save all files" saves each open file with unsaved edits, each with
 * its own revision; a 409 `file_changed` opens that file's stale dialog
 * (`FileSessions`).
 */
export function useFilesHost({
  ws,
  flowKey,
  namespace,
  readOnly,
  flowLayout,
}: {
  ws: string;
  /** The flow's id: another flow is another set of tabs, never a namespace change. */
  flowKey: string;
  namespace: string;
  readOnly: boolean;
  flowLayout: FlowLayoutHandle;
}): FilesHostState {
  // One store per flow: another flow's sessions are never this flow's (fix round 2).
  const [stores, setStores] = useState(() => ({ flowKey, store: new FileSessionStore() }));
  let store = stores.store;
  if (stores.flowKey !== flowKey) {
    store = new FileSessionStore();
    setStores({ flowKey, store });
  }
  // Re-renders when any open file's state changes; `dirtyKey` below is read fresh each render.
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const { layout, setLayout } = flowLayout;
  const latest = useRef(layout);
  useEffect(() => {
    latest.current = layout;
  });

  // The flow moved to another namespace (a save of its `namespace:`, or someone else's, read
  // back): the open file tabs belong to the old one. They are closed in this very render (React's
  // pattern for state that follows a prop: the render is redone before anything commits), so no
  // file session ever starts for the new namespace under an old tab, and nothing re-points.
  // Another FLOW is not a move: its layout and tabs are its own, so the owner is only reset.
  // (The page remounts per flow, `FlowDetailLoader key={id}`; this holds without that.)
  const [owner, setOwner] = useState({ flowKey, namespace });
  const [moveNotice, setMoveNotice] = useState<{ from: string; to: string; lost: string[] } | null>(null);
  if (owner.flowKey !== flowKey) {
    setOwner({ flowKey, namespace });
  } else if (owner.namespace !== namespace) {
    setOwner({ flowKey, namespace });
    const next = withoutFileTabs(layout);
    if (next !== layout) {
      const lost = layout.panels.flatMap((p) => p.tabs).map(filePathOf).filter((p): p is string => p !== null && !!store.get(p)?.dirty);
      setLayout(next);
      setMoveNotice({ from: owner.namespace, to: namespace, lost: lost.map(fileNameOf) });
    }
  }
  useEffect(() => {
    if (!moveNotice) return;
    toast.info(
      `The flow is now in ${moveNotice.to}, so the file tabs of ${moveNotice.from} were closed.` +
        (moveNotice.lost.length > 0 ? ` Unsaved edits to ${nameList(moveNotice.lost)} were discarded.` : ""),
    );
  }, [moveNotice]);

  const openPaths = useMemo(
    () => layout.panels.flatMap((p) => p.tabs).map(filePathOf).filter((p): p is string => p !== null),
    [layout],
  );
  const dirtyKey = store.entries().filter(([p, f]) => f.dirty && openPaths.includes(p)).map(([p]) => p).sort().join("\n");
  const dirtyPaths = useMemo(() => (dirtyKey ? dirtyKey.split("\n") : []), [dirtyKey]);

  /**
   * A close that would drop file tabs with unsaved edits, held for the question: every tab it
   * drops, and which of them hold unsaved edits (all are named).
   */
  const [pending, setPending] = useState<{ dropped: TabId[]; dirty: string[] } | null>(null);
  const guardedSetLayout = useCallback(
    (next: Layout) => {
      const kept = new Set(next.panels.flatMap((p) => p.tabs));
      const dropped = latest.current.panels.flatMap((p) => p.tabs).filter((t) => !kept.has(t));
      const dirty = dropped.map(filePathOf).filter((p): p is string => p !== null && !!store.get(p)?.dirty);
      if (dirty.length > 0) setPending({ dropped, dirty });
      else setLayout(next);
    },
    [setLayout, store],
  );


  const open = useCallback((path: string) => {
    const next = openTab(latest.current, fileTabId(path));
    if (next !== latest.current) setLayout(next);
  }, [setLayout]);
  const close = useCallback((path: string) => {
    const next = closeTab(latest.current, fileTabId(path));
    if (next !== latest.current) guardedSetLayout(next);
  }, [guardedSetLayout]);
  const moved = useCallback((from: string, to: string) => {
    const remap = (tab: TabId) => {
      const p = filePathOf(tab);
      if (p === null) return tab;
      if (p === from) return fileTabId(to);
      return from.endsWith("/") && p.startsWith(from) ? fileTabId(to + p.slice(from.length)) : tab;
    };
    const next = renameTabs(latest.current, remap);
    if (next !== latest.current) setLayout(next);
  }, [setLayout]);
  const deleted = useCallback((path: string) => {
    // The tree has already said that unsaved edits go with the file.
    let next = latest.current;
    for (const tab of next.panels.flatMap((p) => p.tabs)) {
      const p = filePathOf(tab);
      if (p !== null && (p === path || (path.endsWith("/") && p.startsWith(path)))) next = closeTab(next, tab);
    }
    if (next !== latest.current) setLayout(next);
  }, [setLayout]);

  const host = useMemo<FilesHost>(
    () => ({ ws, namespace, readOnly, store, open, close, moved, deleted, dirtyPaths }),
    [ws, namespace, readOnly, store, open, close, moved, deleted, dirtyPaths],
  );

  const [saving, setSaving] = useState(false);
  const openRef = useRef(openPaths);
  useEffect(() => {
    openRef.current = openPaths;
  });
  /** The pass running now, if any, and the files it saves. */
  const running = useRef<{ done: Promise<boolean>; paths: ReadonlySet<string> } | null>(null);
  /** One pass over `dirty` (open files with unsaved edits), each saved with its own revision. */
  const savePass = useCallback(async (dirty: [string, NamespaceFile][]): Promise<boolean> => {
    if (dirty.length === 0) return true;
    setSaving(true);
    try {
      const results = await Promise.all(dirty.map(async ([p, f]) => [p, await f.save()] as const));
      const saved = results.filter(([, ok]) => ok).length;
      const failed = results.filter(([, ok]) => !ok).map(([p]) => fileNameOf(p));
      if (saved > 0) toast.success(`Saved ${saved} ${plural(saved, "file", "files")}.`);
      // A changed file asks its own question (the stale dialog); any other failure is shown in its tab.
      if (failed.length > 0) {
        toast.error(
          failed.length === 1
            ? `${failed[0]} was not saved. Its tab says why; fix that, then save again.`
            : `${nameList(failed)} were not saved. Their tabs say why; fix that, then save again.`,
        );
      }
      return failed.length === 0;
    } finally {
      setSaving(false);
    }
  }, []);
  /**
   * Save every open file with unsaved edits. A call made while a pass runs waits for it, then
   * saves what is still unsaved, so "Save files first" never ends quietly because a Save all
   * was already running (fix round 2).
   */
  const saveAll = useCallback(async (): Promise<boolean> => {
    if (readOnly) return false;
    // A file the running pass saved still reads as unsaved until React publishes its new state,
    // so it is skipped here rather than saved twice (the second save would carry the old
    // revision and be refused).
    const covered = new Set<string>();
    while (running.current) {
      const { done, paths } = running.current;
      if (!(await done.catch(() => false))) return false;
      for (const p of paths) covered.add(p);
    }
    const dirty = store.entries().filter(([p, f]) => f.dirty && openRef.current.includes(p) && !covered.has(p));
    const done = savePass(dirty);
    const entry = { done, paths: new Set(dirty.map(([p]) => p)) };
    running.current = entry;
    try {
      return await done;
    } finally {
      if (running.current === entry) running.current = null;
    }
  }, [readOnly, savePass, store]);

  const saveAllState: Enablement = saving
    ? { ok: false, reason: "Saving…" }
    : dirtyPaths.length === 0
      ? { ok: false, reason: "No unsaved files" }
      : { ok: true };

  const guardedLayout = useMemo(() => ({ ...flowLayout, setLayout: guardedSetLayout }), [flowLayout, guardedSetLayout]);

  return {
    host,
    openPaths,
    flowLayout: guardedLayout,
    saveAll,
    saveAllState,
    closing: pending?.dirty ?? null,
    // Applied to the layout as it is now, not as it was when the question opened.
    confirmClose: () => {
      if (pending) {
        let next = latest.current;
        for (const tab of pending.dropped) next = closeTab(next, tab);
        if (next !== latest.current) setLayout(next);
      }
      setPending(null);
    },
    cancelClose: () => setPending(null),
  };
}

/** "Close a.py without saving?": asked when file tabs with unsaved edits are closed; names every one. */
export function DiscardFileDialog({ paths, onConfirm, onCancel }: { paths: readonly string[] | null; onConfirm: () => void; onCancel: () => void }) {
  const many = (paths?.length ?? 0) > 1;
  return (
    <AlertDialog open={paths !== null} onOpenChange={(o) => (o ? undefined : onCancel())}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Close {nameList((paths ?? []).map(fileNameOf))} without saving?</AlertDialogTitle>
          <AlertDialogDescription>
            {many
              ? "They have unsaved edits. Closing them discards those edits; the saved revisions are kept."
              : "It has unsaved edits. Closing it discards them; the saved revision is kept."}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Keep editing</AlertDialogCancel>
          <Button type="button" variant="destructive" onClick={onConfirm}>
            Discard and close
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/**
 * Asked before a flow save that moves the flow to another namespace while open files have
 * unsaved edits: those files belong to the old namespace, and their tabs close when the flow
 * moves (W7d Task 7, fix round 1).
 */
export function NamespaceChangeDialog({
  ask,
  busy,
  onSaveFiles,
  onDiscard,
  onCancel,
}: {
  ask: { from: string; to: string; files: readonly string[] } | null;
  busy: boolean;
  onSaveFiles: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}) {
  const names = nameList((ask?.files ?? []).map(fileNameOf));
  const many = (ask?.files.length ?? 0) > 1;
  return (
    <AlertDialog open={ask !== null} onOpenChange={(o) => (o || busy ? undefined : onCancel())}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Save the open files before the flow moves to {ask?.to}?</AlertDialogTitle>
          <AlertDialogDescription>
            This save moves the flow from namespace {ask?.from} to {ask?.to}. The open files belong to {ask?.from}, and their tabs
            close when the flow moves. {names} {many ? "have" : "has"} unsaved edits: save {many ? "them" : "it"} into {ask?.from} first, or
            discard {many ? "those edits" : "the edits"}.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <Button type="button" variant="destructive" disabled={busy} onClick={onDiscard}>
            Discard file changes
          </Button>
          <Button type="button" disabled={busy} onClick={onSaveFiles}>
            Save files first
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
