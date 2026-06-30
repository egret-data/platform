import { useMemo, useState } from "react";
import { Search, Star, Trash2, FolderPlus, ListChecks, Loader2 } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import {
  type Engine,
  type NotebookSummary,
  useNotebooks,
  useCreateNotebook,
  useUpdateNotebook,
  useDeleteNotebook,
  useTrashNotebook,
  useRestoreNotebook,
  useAddNotebookBookmark,
  useRemoveNotebookBookmark,
} from "@/api/notebooks";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

type SortKey = "name" | "updated_at";

// ── New notebook dialog ───────────────────────────────────────────────────────

function NewNotebookDialog({
  ws,
  open,
  onOpenChange,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const createMutation = useCreateNotebook(ws);

  const [name, setName] = useState("");
  const [engine, setEngine] = useState<Engine>("daft");

  function handleOpenChange(next: boolean) {
    if (!next) {
      setName("");
      setEngine("daft");
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    try {
      const created = await createMutation.mutateAsync({
        name: name.trim() || undefined,
        engine,
      });
      handleOpenChange(false);
      navigate(`/explore/${ws}/notebooks/${created.id}`);
    } catch (err) {
      toast.error(String(err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Create notebook</DialogTitle>
            <DialogDescription>
              Give your notebook a name and pick an execution engine.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="nb-name">Name</Label>
              <Input
                id="nb-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Untitled"
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="nb-engine">Engine</Label>
              <Select value={engine} onValueChange={(v) => setEngine(v as Engine)}>
                <SelectTrigger id="nb-engine" className="w-full">
                  <SelectValue placeholder="Select an engine" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="daft">Daft</SelectItem>
                  <SelectItem value="spark_connect">Spark Connect</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Rename dialog ─────────────────────────────────────────────────────────────

function RenameNotebookDialog({
  ws,
  notebook,
  open,
  onOpenChange,
}: {
  ws: string;
  notebook: NotebookSummary;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const updateMutation = useUpdateNotebook(ws, notebook.id);
  const [name, setName] = useState(notebook.name);

  function handleOpenChange(next: boolean) {
    if (!next) setName(notebook.name);
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    try {
      await updateMutation.mutateAsync({ name: name.trim() });
      handleOpenChange(false);
    } catch (err) {
      toast.error(String(err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rename notebook</DialogTitle>
            <DialogDescription>
              Enter a new name for &laquo;{notebook.name}&raquo;.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-1.5">
            <Label htmlFor="rename-nb-name">Name</Label>
            <Input
              id="rename-nb-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              autoFocus
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={updateMutation.isPending}>
              {updateMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Per-row actions menu ──────────────────────────────────────────────────────

function NotebookActionsMenu({
  ws,
  nb,
  trashed,
  folders,
  onRename,
  onMove,
  onNewFolder,
  onTrash,
  onRestore,
  onDeletePermanent,
}: {
  ws: string;
  nb: NotebookSummary;
  trashed: boolean;
  folders: import("@/api/folders").Folder[];
  onRename: (nb: NotebookSummary) => void;
  onMove: (nb: NotebookSummary, folderId: string | null) => void;
  onNewFolder: (nb: NotebookSummary) => void;
  onTrash: (nb: NotebookSummary) => void;
  onRestore: (nb: NotebookSummary) => void;
  onDeletePermanent: (nb: NotebookSummary) => void;
}) {
  const nav = useNavigate();
  if (trashed) {
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => onRestore(nb)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          className="text-destructive focus:text-destructive"
          onClick={() => onDeletePermanent(nb)}
        >
          Delete permanently
        </DropdownMenuItem>
      </RowActionsMenu>
    );
  }
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/notebooks/${nb.id}`)}>
        Open
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => onRename(nb)}>Rename</DropdownMenuItem>
      <MoveToFolderItems
        folders={folders}
        currentFolderId={nb.folder_id ?? null}
        onMove={(fid) => onMove(nb, fid)}
        onNewFolder={() => onNewFolder(nb)}
      />
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(nb)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function Notebooks() {
  const ws = useParams().workspace!;

  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading } = useNotebooks(ws, { trashed: showTrash });

  const { data: folders = [] } = useFolders(ws, "notebooks");
  const setFolder = useSetFolder(ws, "notebooks");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "notebooks");
  const collapse = useCollapsed(`folders:${ws}:notebooks`);

  const trashNotebook = useTrashNotebook(ws);
  const restoreNotebook = useRestoreNotebook(ws);
  const addBookmark = useAddNotebookBookmark(ws);
  const removeBookmark = useRemoveNotebookBookmark(ws);
  const deleteMutation = useDeleteNotebook(ws);

  const [newOpen, setNewOpen] = useState(false);
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [renameTarget, setRenameTarget] = useState<NotebookSummary | null>(null);
  const [pendingDelete, setPendingDelete] = useState<NotebookSummary | null>(null);
  const [folderDialog, setFolderDialog] = useState<{
    mode: "new" | "rename" | "move" | "bulkmove";
    nb?: NotebookSummary;
    folder?: { id: string; name: string };
  } | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((n) => {
      if (onlyBookmarked && !n.bookmarked) return false;
      if (!q) return true;
      return n.name.toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      const cmp =
        sort.key === "name"
          ? a.name.localeCompare(b.name)
          : a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, onlyBookmarked, query, sort]);

  const bulk = useBulkSelect(rows);
  const filtering = query.trim() !== "" || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  const hasData = !!data && data.length > 0;

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }

  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  async function doBulkTrash() {
    for (const id of bulk.selectedIds) {
      try { await trashNotebook.mutateAsync(id); } catch (e) { toast.error(String(e)); }
    }
    toast.success("Moved to Trash");
    bulk.clear();
  }

  async function doBulkMove(name: string) {
    const trimmed = name.trim();
    try {
      const existing = folders.find((f) => f.name.toLowerCase() === trimmed.toLowerCase());
      const folderId = existing ? existing.id : (await createFolder.mutateAsync(trimmed)).id;
      for (const id of bulk.selectedIds) await setFolder.mutateAsync({ id, folder_id: folderId });
    } catch (e) { toast.error(String(e)); }
    bulk.clear();
  }

  const renderRow = (nb: NotebookSummary) => (
    <tr
      key={nb.id}
      className="border-t hover:bg-accent"
      draggable={!showTrash}
      onDragStart={!showTrash ? (e) => e.dataTransfer.setData("text/plain", nb.id) : undefined}
    >
      {bulk.bulkMode && !showTrash && (
        <td className="py-1">
          <Checkbox
            checked={bulk.selected.has(nb.id)}
            onCheckedChange={() => bulk.toggle(nb.id)}
            aria-label={`Select ${nb.name}`}
          />
        </td>
      )}
      <td className="py-1 pl-6">
        <span className="inline-flex items-center gap-1">
          {!showTrash && (
            <Button
              size="sm"
              variant="ghost"
              className="h-7 w-7 p-0"
              aria-label={`${nb.bookmarked ? "unbookmark" : "bookmark"} ${nb.name}`}
              onClick={() =>
                guard((nb.bookmarked ? removeBookmark : addBookmark).mutateAsync(nb.id))
              }
            >
              <Star
                className={`h-4 w-4 ${nb.bookmarked ? "fill-amber-400 text-amber-400" : "text-muted-foreground"}`}
              />
            </Button>
          )}
          <Link
            className="font-medium hover:underline"
            to={`/explore/${ws}/notebooks/${nb.id}`}
          >
            {nb.name}
          </Link>
        </span>
      </td>
      <td>
        <Badge variant="secondary">{nb.engine}</Badge>
      </td>
      <td>
        {nb.is_public ? (
          <Badge variant="outline">Public</Badge>
        ) : (
          <span className="text-muted-foreground">Private</span>
        )}
      </td>
      <td className="text-muted-foreground">
        <RelativeTime iso={nb.updated_at} />
      </td>
      <td>
        <NotebookActionsMenu
          ws={ws}
          nb={nb}
          trashed={showTrash}
          folders={folders}
          onRename={setRenameTarget}
          onMove={(x, fid) =>
            setFolder.mutateAsync({ id: x.id, folder_id: fid }).catch((e) => toast.error(String(e)))
          }
          onNewFolder={(x) => setFolderDialog({ mode: "move", nb: x })}
          onTrash={(x) => guard(trashNotebook.mutateAsync(x.id).then(() => toast.success("Moved to Trash")))}
          onRestore={(x) => guard(restoreNotebook.mutateAsync(x.id))}
          onDeletePermanent={setPendingDelete}
        />
      </td>
    </tr>
  );

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Notebooks"}</h1>
        <div className="flex items-center gap-2">
          {!showTrash && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant={onlyBookmarked ? "secondary" : "outline"}
                  aria-label="Bookmarks"
                  onClick={() => setOnlyBookmarked((v) => !v)}
                >
                  <Star className={`h-4 w-4 ${onlyBookmarked ? "fill-current" : ""}`} />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom">Bookmarks</TooltipContent>
            </Tooltip>
          )}
          {!showTrash && hasData && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant={bulk.bulkMode ? "secondary" : "outline"}
                  aria-label="Bulk select"
                  onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
                >
                  <ListChecks className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom">Bulk select</TooltipContent>
            </Tooltip>
          )}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size="icon"
                variant={showTrash ? "secondary" : "outline"}
                aria-label="Trash"
                onClick={() => {
                  const next = !showTrash;
                  setShowTrash(next);
                  if (next) {
                    setOnlyBookmarked(false);
                    bulk.setBulkMode(false);
                  }
                }}
              >
                <Trash2 className={`h-4 w-4 ${showTrash ? "fill-current" : ""}`} />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Trash</TooltipContent>
          </Tooltip>
          {!showTrash && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant="outline"
                  aria-label="New folder"
                  onClick={() => setFolderDialog({ mode: "new" })}
                >
                  <FolderPlus className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom">New folder</TooltipContent>
            </Tooltip>
          )}
          <Button onClick={() => setNewOpen(true)}>+ Notebook</Button>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={5} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="pl-8"
                placeholder="Search notebooks…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
            {bulk.bulkMode && bulk.someSelected && !showTrash && (
              <>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setFolderDialog({ mode: "bulkmove" })}
                >
                  Move {bulk.selectedIds.length} to folder…
                </Button>
                <Button variant="destructive" size="sm" onClick={doBulkTrash}>
                  Move {bulk.selectedIds.length} to Trash
                </Button>
              </>
            )}
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  {bulk.bulkMode && !showTrash && (
                    <th className="w-8 py-1">
                      <Checkbox
                        checked={bulk.allSelected}
                        onCheckedChange={bulk.toggleAll}
                        aria-label="Select all"
                      />
                    </th>
                  )}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <th>Engine</th>
                  <th>Visibility</th>
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              {showTrash ? (
                <tbody>{rows.map(renderRow)}</tbody>
              ) : (
                groups.map((group) => {
                  const key = group.folder?.id ?? "__uncat__";
                  const isCollapsed = collapse.isCollapsed(key);
                  return (
                    <tbody key={key}>
                      <FolderHeaderRow
                        name={group.folder?.name ?? "Uncategorized"}
                        count={group.rows.length}
                        colSpan={bulk.bulkMode ? 6 : 5}
                        collapsed={isCollapsed}
                        onToggle={() => collapse.toggle(key)}
                        onRename={
                          group.folder
                            ? () => setFolderDialog({ mode: "rename", folder: group.folder! })
                            : undefined
                        }
                        onDelete={
                          group.folder
                            ? () =>
                                removeFolder
                                  .mutateAsync(group.folder!.id)
                                  .then(() => toast.success("Folder deleted"))
                                  .catch((e) => toast.error(String(e)))
                            : undefined
                        }
                        onDropItem={(id) =>
                          setFolder
                            .mutateAsync({ id, folder_id: group.folder?.id ?? null })
                            .catch((e) => toast.error(String(e)))
                        }
                      />
                      {!isCollapsed && group.rows.map(renderRow)}
                    </tbody>
                  );
                })
              )}
            </table>
          ) : (
            <p className="text-muted-foreground">
              {onlyBookmarked
                ? "No bookmarked notebooks."
                : "No notebooks match the current filters."}
            </p>
          )}
        </>
      ) : (
        <p className="text-muted-foreground">{showTrash ? "Trash is empty." : "No notebooks yet."}</p>
      )}

      <NewNotebookDialog ws={ws} open={newOpen} onOpenChange={setNewOpen} />

      {renameTarget && (
        <RenameNotebookDialog
          key={renameTarget.id}
          ws={ws}
          notebook={renameTarget}
          open
          onOpenChange={(o) => {
            if (!o) setRenameTarget(null);
          }}
        />
      )}

      <FolderNameDialog
        open={!!folderDialog}
        title={
          folderDialog?.mode === "rename"
            ? "Rename folder"
            : folderDialog?.mode === "move"
              ? "Move to new folder"
              : folderDialog?.mode === "bulkmove"
                ? "Move to folder"
                : "New folder"
        }
        submitLabel={
          folderDialog?.mode === "rename"
            ? "Rename"
            : folderDialog?.mode === "move" || folderDialog?.mode === "bulkmove"
              ? "Move"
              : "Create"
        }
        initialValue={folderDialog?.mode === "rename" ? folderDialog.folder!.name : ""}
        onOpenChange={(o) => {
          if (!o) setFolderDialog(null);
        }}
        onSubmit={(name) => {
          const d = folderDialog!;
          if (d.mode === "rename")
            renameFolder
              .mutateAsync({ id: d.folder!.id, name })
              .catch((e) => toast.error(String(e)));
          else if (d.mode === "move")
            createFolder
              .mutateAsync(name)
              .then((f) => setFolder.mutateAsync({ id: d.nb!.id, folder_id: f.id }))
              .then(() => toast.success("Moved"))
              .catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMove(name);
          else
            createFolder
              .mutateAsync(name)
              .then(() => toast.success("Folder created"))
              .catch((e) => toast.error(String(e)));
        }}
      />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => {
          if (!o) setPendingDelete(null);
        }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => {
          if (!pendingDelete) return;
          await guard(deleteMutation.mutateAsync(pendingDelete.id));
          setPendingDelete(null);
        }}
      />
    </div>
  );
}
