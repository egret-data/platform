import { useMemo, useState } from "react";
import { toast } from "sonner";
import { ChevronDown, ChevronRight, MoreHorizontal, Pencil, Trash2 } from "lucide-react";
import {
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle,
  AlertDialogDescription, AlertDialogFooter, AlertDialogCancel, AlertDialogAction,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  useSavedQueries, useUpdateSavedQuery, useDeleteSavedQuery,
  type SavedQuery, type DatabaseListItem,
} from "@/api/sqllab";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

function dbName(databases: DatabaseListItem[] | undefined, id: string | null): string {
  if (!id) return "—";
  return databases?.find((d) => d.id === id)?.name ?? `${id.slice(0, 8)}…`;
}

/** Sidebar-adapted collapsible folder header (div-based; FolderHeaderRow is
 *  `<tr>`-based and only renders inside a table). Mirrors its callbacks. */
function FolderHeader({ name, count, collapsed, onToggle, onShare, onRename, onDelete, onDropItem }: {
  name: string; count: number; collapsed: boolean; onToggle: () => void;
  onShare?: () => void; onRename?: () => void; onDelete?: () => void; onDropItem?: (id: string) => void;
}) {
  const Chevron = collapsed ? ChevronRight : ChevronDown;
  return (
    <div
      className="flex items-center gap-1 bg-muted/40 px-3 py-1.5"
      onDragOver={onDropItem ? (e) => e.preventDefault() : undefined}
      onDrop={onDropItem ? (e) => { const id = e.dataTransfer.getData("text/plain"); if (id) onDropItem(id); } : undefined}
    >
      <button
        className="inline-flex min-w-0 flex-1 items-center gap-1 text-left text-sm font-medium hover:underline"
        onClick={onToggle}
        aria-label={`Toggle ${name}`}
      >
        <Chevron className="h-4 w-4 shrink-0 text-muted-foreground" />
        <span className="truncate">{name}</span>
        <span className="shrink-0 text-muted-foreground">({count})</span>
      </button>
      {(onShare || onRename || onDelete) && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="sm" className="h-6 w-6 shrink-0 p-0" aria-label={`Folder ${name} actions`}>
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {onShare && <DropdownMenuItem onClick={onShare}>Share…</DropdownMenuItem>}
            {onRename && <DropdownMenuItem onClick={onRename}>Rename</DropdownMenuItem>}
            {onDelete && <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>Delete</DropdownMenuItem>}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </div>
  );
}

export function SavedTab({ ws, databases, onOpen }: {
  ws: string;
  databases: DatabaseListItem[] | undefined;
  onOpen: (sql: string, dbId: string | null) => void;
}) {
  const { data } = useSavedQueries(ws);
  const update = useUpdateSavedQuery(ws);
  const del = useDeleteSavedQuery(ws);

  const { data: folders = [] } = useFolders(ws, "saved_queries");
  const setFolder = useSetFolder(ws, "saved_queries");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "saved_queries");
  const collapse = useCollapsed(`folders:${ws}:saved_queries`);

  const [editing, setEditing] = useState<SavedQuery | null>(null);
  const [deleting, setDeleting] = useState<SavedQuery | null>(null);
  const [shareTarget, setShareTarget] = useState<SavedQuery | null>(null);
  const [folderShare, setFolderShare] = useState<{ id: string; name: string } | null>(null);
  const [folderDialog, setFolderDialog] = useState<{
    mode: "rename" | "move";
    query?: SavedQuery;
    folder?: { id: string; name: string };
  } | null>(null);

  const items = data ?? [];
  const groups = useMemo(() => groupByFolder(items, folders, false), [items, folders]);

  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  const renderRow = (q: SavedQuery) => (
    <div
      key={q.id}
      className="group flex items-start gap-2 px-3 py-2 hover:bg-accent"
      draggable
      onDragStart={(e) => e.dataTransfer.setData("text/plain", q.id)}
    >
      <button className="min-w-0 flex-1 text-left" onClick={() => onOpen(q.sql, q.database_id)}>
        <div className="flex items-center gap-2 text-sm">
          <span className="font-semibold">{q.name}</span>
          <span className="text-xs text-muted-foreground">{dbName(databases, q.database_id)}</span>
          <span className="text-xs text-muted-foreground" title={fullTimestamp(q.updated_at)}>updated {timeAgo(q.updated_at)}</span>
        </div>
        <div className="truncate text-xs text-muted-foreground" title={q.description || q.sql}>{q.description || q.sql}</div>
      </button>
      <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
        <button className="rounded p-1 hover:bg-background" title="Edit" aria-label="Edit" onClick={() => setEditing(q)}><Pencil className="h-4 w-4" /></button>
        <button className="rounded p-1 text-red-600 hover:bg-background" title="Delete" aria-label="Delete" onClick={() => setDeleting(q)}><Trash2 className="h-4 w-4" /></button>
        <RowActionsMenu>
          <MoveToFolderItems
            folders={folders}
            currentFolderId={q.folder_id}
            onMove={(fid) => guard(setFolder.mutateAsync({ id: q.id, folder_id: fid }))}
            onNewFolder={() => setFolderDialog({ mode: "move", query: q })}
          />
          <DropdownMenuItem onClick={() => setShareTarget(q)}>Share…</DropdownMenuItem>
        </RowActionsMenu>
      </div>
    </div>
  );

  if (items.length === 0) return <div className="p-4 text-center text-sm text-muted-foreground">No saved queries.</div>;

  return (
    <>
      {folders.length === 0 ? (
        <div className="divide-y">{items.map(renderRow)}</div>
      ) : (
        groups.map((group) => {
          const key = group.folder?.id ?? "__uncat__";
          const isCollapsed = collapse.isCollapsed(key);
          return (
            <div key={key}>
              <FolderHeader
                name={group.folder?.name ?? "Uncategorized"}
                count={group.rows.length}
                collapsed={isCollapsed}
                onToggle={() => collapse.toggle(key)}
                onShare={group.folder ? () => setFolderShare({ id: group.folder!.id, name: group.folder!.name }) : undefined}
                onRename={group.folder ? () => setFolderDialog({ mode: "rename", folder: group.folder! }) : undefined}
                onDelete={group.folder ? () => guard(removeFolder.mutateAsync(group.folder!.id).then(() => toast.success("Folder deleted"))) : undefined}
                onDropItem={(id) => guard(setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }))}
              />
              {!isCollapsed && <div className="divide-y">{group.rows.map(renderRow)}</div>}
            </div>
          );
        })
      )}

      <SaveQueryDialog
        open={editing != null}
        onOpenChange={(o) => { if (!o) setEditing(null); }}
        mode="edit"
        initialName={editing?.name ?? ""}
        initialDescription={editing?.description ?? ""}
        onSave={async (name, description) => {
          if (!editing) return;
          await update.mutateAsync({ id: editing.id, database_id: editing.database_id, name, sql: editing.sql, description });
          toast.success("Updated");
          setEditing(null);
        }}
      />

      <AlertDialog open={deleting != null} onOpenChange={(o) => { if (!o) setDeleting(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete saved query?</AlertDialogTitle>
            <AlertDialogDescription>“{deleting?.name}” will be permanently deleted.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => { if (deleting) del.mutate(deleting.id); setDeleting(null); }}
            >Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <FolderNameDialog
        open={!!folderDialog}
        title={folderDialog?.mode === "rename" ? "Rename folder" : "Move to new folder"}
        submitLabel={folderDialog?.mode === "rename" ? "Rename" : "Move"}
        initialValue={folderDialog?.mode === "rename" ? folderDialog.folder!.name : ""}
        onOpenChange={(o) => { if (!o) setFolderDialog(null); }}
        onSubmit={(name) => {
          const d = folderDialog!;
          if (d.mode === "rename")
            guard(renameFolder.mutateAsync({ id: d.folder!.id, name }));
          else
            guard(
              createFolder
                .mutateAsync(name)
                .then((f) => setFolder.mutateAsync({ id: d.query!.id, folder_id: f.id }))
                .then(() => toast.success("Moved")),
            );
        }}
      />

      {shareTarget && (
        <ShareDialog
          ws={ws}
          kind="saved_query"
          id={shareTarget.id}
          name={shareTarget.name}
          folderId={shareTarget.folder_id}
          open={shareTarget !== null}
          onOpenChange={(o) => { if (!o) setShareTarget(null); }}
        />
      )}

      {folderShare && (
        <ShareDialog
          ws={ws}
          kind="folder"
          id={folderShare.id}
          name={folderShare.name}
          open={folderShare !== null}
          onOpenChange={(o) => { if (!o) setFolderShare(null); }}
        />
      )}
    </>
  );
}
