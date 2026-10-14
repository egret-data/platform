import { useEffect, useLayoutEffect, useSyncExternalStore } from "react";
import type { FileSessionStore } from "@/components/workflows/editor/fileTabs";
import { FileConflictDialog } from "@/components/workflows/namespaces/files/NamespaceFileEditor";
import { useNamespaceFile } from "@/components/workflows/namespaces/files/useNamespaceFile";

/**
 * THE OPEN FILES OF THE FLOW EDITOR (W7d spec §4.3): one `useNamespaceFile`
 * per `file:<path>` tab of the layout, mounted by the page for as long as
 * the tab is open, whichever tab is shown. Each publishes its state to the
 * page's {@link FileSessionStore}, which the tabs, their dots and "Save all
 * files" read.
 *
 * It also asks the 409 `file_changed` question (W7a's stale-save dialog),
 * for the first open file that has one: a save refused while its tab is not
 * shown (from "Save all files") still asks, and two are never asked at once.
 *
 * The page reaches this module through `lazy()` only, so the hook and its
 * API helpers stay out of the flow pages' chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export default function FileSessions({ ws, namespace, paths, store }: { ws: string; namespace: string; paths: readonly string[]; store: FileSessionStore }) {
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const asking = paths.find((p) => store.get(p)?.conflict != null) ?? null;
  return (
    <>
      {paths.map((p) => (
        <FileSession key={p} ws={ws} namespace={namespace} path={p} store={store} asking={p === asking} />
      ))}
    </>
  );
}

function FileSession({ ws, namespace, path, store, asking }: { ws: string; namespace: string; path: string; store: FileSessionStore; asking: boolean }) {
  const file = useNamespaceFile(ws, namespace, path);
  // Every render: the store ignores a value equal to the one it has.
  useLayoutEffect(() => store.set(path, file));
  useEffect(() => () => store.delete(path), [store, path]);
  return asking ? <FileConflictDialog path={path} file={file} /> : null;
}
