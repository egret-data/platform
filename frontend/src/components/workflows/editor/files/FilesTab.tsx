import { FolderTree } from "lucide-react";
import { EmptyState } from "@/components/explore/EmptyState";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { NamespaceFiles } from "@/components/workflows/namespaces/files/NamespaceFiles";

/**
 * THE EDITOR'S FILES TAB (W7d spec §4.3, "In the flow editor"): the tree of
 * the flow's namespace, Task 6's `NamespaceFiles` in host mode. A file it
 * opens becomes a `file:<path>` tab beside Code (`FilesHost.open`); the page
 * keeps the open files, so the tree here is told which have unsaved edits
 * (moving one is refused) and follows its moves and deletes into the tabs.
 *
 * Reached through `lazy()` only (`registerEditorTabs.tsx`), so the explorer
 * stays out of the flow pages' chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export function FilesTab({ ctx }: { ctx: EditorContext }) {
  const host = ctx.files;
  if (!host) {
    return (
      <div className="p-4">
        <EmptyState
          variant="dashed"
          icon={FolderTree}
          title="Files open once the flow is created"
          description="Namespace files belong to the flow's namespace. Create the flow, then open its namespace's files here."
        />
      </div>
    );
  }
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col p-3" data-testid="editor-files-tab">
      <NamespaceFiles
        ws={host.ws}
        namespace={host.namespace}
        readOnly={host.readOnly}
        onOpenFile={host.open}
        dirtyPaths={host.dirtyPaths}
        onPathMoved={host.moved}
        onPathDeleted={host.deleted}
        // A restore from the tree: an open, unedited file tab reads the new
        // revision (as a restore from the tab's own Revisions does); unsaved
        // edits are kept, and their save meets the stale-save question.
        onPathRestored={(p) => {
          const file = host.store.get(p);
          if (file && !file.dirty) void file.reload();
        }}
      />
    </div>
  );
}
