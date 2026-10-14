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
  saveAll: () => Promise<void>;
  saveAllState: Enablement;
  /** A close waiting for "Discard and close", by stored path. */
  closing: string | null;
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
  namespace,
  readOnly,
  flowLayout,
}: {
  ws: string;
  namespace: string;
  readOnly: boolean;
  flowLayout: FlowLayoutHandle;
}): FilesHostState {
  const [store] = useState(() => new FileSessionStore());
  // Re-renders when any open file's state changes; `dirtyKey` below is read fresh each render.
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const { layout, setLayout } = flowLayout;
  const latest = useRef(layout);
  useEffect(() => {
    latest.current = layout;
  });

  const openPaths = useMemo(
    () => layout.panels.flatMap((p) => p.tabs).map(filePathOf).filter((p): p is string => p !== null),
    [layout],
  );
  const dirtyKey = store.entries().filter(([p, f]) => f.dirty && openPaths.includes(p)).map(([p]) => p).sort().join("\n");
  const dirtyPaths = useMemo(() => (dirtyKey ? dirtyKey.split("\n") : []), [dirtyKey]);

  /** A layout change that would drop a file tab with unsaved edits, held for the question. */
  const [pending, setPending] = useState<{ next: Layout; path: string } | null>(null);
  const guardedSetLayout = useCallback(
    (next: Layout) => {
      const kept = new Set(next.panels.flatMap((p) => p.tabs));
      const dropped = latest.current.panels
        .flatMap((p) => p.tabs)
        .filter((t) => !kept.has(t))
        .map(filePathOf)
        .find((p) => p !== null && store.get(p)?.dirty);
      if (dropped) setPending({ next, path: dropped });
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
  const saveAll = useCallback(async () => {
    if (readOnly || saving) return;
    const dirty = store.entries().filter(([p, f]) => f.dirty && openPaths.includes(p));
    if (dirty.length === 0) return;
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
            : `${failed.join(", ")} were not saved. Their tabs say why; fix that, then save again.`,
        );
      }
    } finally {
      setSaving(false);
    }
  }, [readOnly, saving, store, openPaths]);

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
    closing: pending?.path ?? null,
    confirmClose: () => {
      if (pending) setLayout(pending.next);
      setPending(null);
    },
    cancelClose: () => setPending(null),
  };
}

/** "Close a.py without saving?": asked when a file tab with unsaved edits is closed. */
export function DiscardFileDialog({ path, onConfirm, onCancel }: { path: string | null; onConfirm: () => void; onCancel: () => void }) {
  return (
    <AlertDialog open={path !== null} onOpenChange={(o) => (o ? undefined : onCancel())}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Close {path ? fileNameOf(path) : ""} without saving?</AlertDialogTitle>
          <AlertDialogDescription>It has unsaved edits. Closing it discards them; the saved revision is kept.</AlertDialogDescription>
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
