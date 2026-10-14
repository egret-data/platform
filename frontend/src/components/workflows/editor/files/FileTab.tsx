import { useState, useSyncExternalStore } from "react";
import { FileX } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/explore/EmptyState";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { NamespaceFileView } from "@/components/workflows/namespaces/files/NamespaceFileEditor";
import { RevisionsDrawer } from "@/components/workflows/namespaces/files/RevisionsDrawer";

/** The create page has no files: a store that holds none. */
const NO_STORE = { subscribe: () => () => {}, get: () => undefined };

/**
 * ONE `file:<path>` TAB OF THE FLOW EDITOR (W7d spec §4.3): the file's view
 * over the state the page keeps for it (`FileSessionStore`), so its unsaved
 * text outlives this tab being hidden, moved or split.
 *
 * - Ctrl/Cmd+S here saves this file only (the view keeps the key from the
 *   flow's own shortcut). The flow's Save never saves it.
 * - A file that is not there any more (deleted or moved by someone else, or
 *   named by an old stored layout) says so and offers to close the tab.
 * - The 409 question is asked by the page's `FileSessions`, not here: a save
 *   from "Save all files" can be refused while this tab is not shown.
 *
 * Reached through `lazy()` only (`registerEditorTabs.tsx`), with the view
 * and Monaco's wrapper (`scripts/assert-workflows-lazy.mjs`).
 */
export function FileTab({ ctx, path }: { ctx: EditorContext; path: string }) {
  const host = ctx.files;
  const store = host?.store ?? NO_STORE;
  const file = useSyncExternalStore(store.subscribe, () => store.get(path), () => store.get(path));
  const [revisions, setRevisions] = useState(false);

  if (!host) {
    return (
      <div className="p-4">
        <EmptyState
          variant="dashed"
          icon={FileX}
          title="Files open once the flow is created"
          description="Namespace files belong to the flow's namespace. Create the flow, then open its namespace's files here."
        />
      </div>
    );
  }
  if (!file) {
    return (
      <div className="space-y-2 p-3" data-testid="file-tab-loading">
        <Skeleton className="h-4 w-2/3" />
        <Skeleton className="h-4 w-1/2" />
      </div>
    );
  }
  if (file.status === "error" && file.errorStatus === 404) {
    return (
      <div className="p-4">
        <EmptyState
          variant="dashed"
          icon={FileX}
          title={`${path} is not in ${host.namespace} any more`}
          description="It was deleted or moved after this tab was opened. If it was moved, open it again from the Files tab."
          action={
            <Button type="button" size="sm" variant="outline" onClick={() => host.close(path)}>
              Close this tab
            </Button>
          }
        />
      </div>
    );
  }
  return (
    <>
      <NamespaceFileView
        ws={host.ws}
        namespace={host.namespace}
        path={path}
        file={file}
        readOnly={host.readOnly}
        onOpenRevisions={() => setRevisions(true)}
        staleDialog={false}
      />
      {revisions ? (
        <RevisionsDrawer
          ws={host.ws}
          namespace={host.namespace}
          path={path}
          readOnly={host.readOnly}
          // A restore wrote a new revision: an unedited copy reads it; unsaved edits are kept.
          onRestored={() => {
            if (!file.dirty) void file.reload();
          }}
          onClose={() => setRevisions(false)}
        />
      ) : null}
    </>
  );
}
