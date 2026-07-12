import { useMemo, useState } from "react";
import { Search, Star, Trash2, FolderPlus, ListChecks } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDashboards, useDashboardMutations, type DashboardSummary } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/components/ui/tooltip";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { DashboardActionsMenu } from "@/components/chart/dashboard/DashboardActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";

type SortKey = "name" | "card_count" | "updated_at";

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading } = useDashboards(ws, { trashed: showTrash });
  const { create, remove, duplicate, trash, restore, addBookmark, removeBookmark } = useDashboardMutations(ws);
  const { data: folders = [] } = useFolders(ws, "dashboards");
  const setFolder = useSetFolder(ws, "dashboards");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "dashboards");
  const collapse = useCollapsed(`folders:${ws}:dashboards`);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<DashboardSummary | null>(null);
  const [shareTarget, setShareTarget] = useState<DashboardSummary | null>(null);
  const [folderShare, setFolderShare] = useState<{ id: string; name: string } | null>(null);
  const [folderDialog, setFolderDialog] = useState<{ mode: "new" | "rename" | "move" | "bulkmove"; dash?: DashboardSummary; folder?: { id: string; name: string } } | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((d) => {
      if (onlyBookmarked && !d.bookmarked) return false;
      if (!q) return true;
      return d.name.toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "card_count") cmp = a.card_count - b.card_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, onlyBookmarked, query, sort]);

  const bulk = useBulkSelect(rows);
  const filtering = query.trim() !== "" || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  async function onCreate() {
    if (!name.trim()) { toast.error("Name required"); return; }
    try { const d = await create.mutateAsync({ name: name.trim() }); setOpen(false); setName(""); nav(`/explore/${ws}/dashboards/${d.id}`); }
    catch (e) { toast.error(String(e)); }
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  async function doBulkTrash() {
    for (const id of bulk.selectedIds) { try { await trash.mutateAsync(id); } catch (e) { toast.error(String(e)); } }
    toast.success("Moved to Trash"); bulk.clear();
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

  const hasData = !!data && data.length > 0;

  const renderRow = (d: DashboardSummary) => (
    <tr key={d.id} className="border-t hover:bg-accent"
      draggable={!showTrash} onDragStart={!showTrash ? (e) => e.dataTransfer.setData("text/plain", d.id) : undefined}>
      {bulk.bulkMode && !showTrash && <td className="py-1"><Checkbox checked={bulk.selected.has(d.id)} onCheckedChange={() => bulk.toggle(d.id)} aria-label={`Select ${d.name}`} /></td>}
      <td className="py-1 pl-6">
        <span className="inline-flex items-center gap-1">
          {!showTrash && (
            <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${d.bookmarked ? "unbookmark" : "bookmark"} ${d.name}`}
              onClick={() => guard((d.bookmarked ? removeBookmark : addBookmark).mutateAsync(d.id))}>
              <Star className={`h-4 w-4 ${d.bookmarked ? "fill-amber-400 text-amber-400" : "text-muted-foreground"}`} />
            </Button>
          )}
          <Link className="font-medium hover:underline" to={`/explore/${ws}/dashboards/${d.id}`}>{d.name}</Link>
        </span>
      </td>
      <td className="text-muted-foreground tabular-nums">{d.card_count}</td>
      <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
      <td>
        <DashboardActionsMenu
          ws={ws} dash={d} trashed={showTrash} folders={folders}
          onDuplicate={(x) => guard(duplicate.mutateAsync(x.id).then((c) => nav(`/explore/${ws}/dashboards/${c.id}`)))}
          onMove={(x, fid) => setFolder.mutateAsync({ id: x.id, folder_id: fid }).catch((e) => toast.error(String(e)))}
          onNewFolder={(x) => setFolderDialog({ mode: "move", dash: x })}
          onTrash={(x) => guard(trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")))}
          onRestore={(x) => guard(restore.mutateAsync(x.id))}
          onDeletePermanent={setPendingDelete}
          onShare={setShareTarget}
        />
      </td>
    </tr>
  );

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Dashboards"}</h1>
        <div className="flex items-center gap-2">
          {!showTrash && (
            <Tooltip><TooltipTrigger asChild>
              <Button size="icon" variant={onlyBookmarked ? "secondary" : "outline"} aria-label="Bookmarks"
                onClick={() => setOnlyBookmarked((v) => !v)}>
                <Star className={`h-4 w-4 ${onlyBookmarked ? "fill-current" : ""}`} />
              </Button>
            </TooltipTrigger><TooltipContent side="bottom">Bookmarks</TooltipContent></Tooltip>
          )}
          {!showTrash && hasData && (
            <Tooltip><TooltipTrigger asChild>
              <Button size="icon" variant={bulk.bulkMode ? "secondary" : "outline"} aria-label="Bulk select"
                onClick={() => bulk.setBulkMode(!bulk.bulkMode)}>
                <ListChecks className="h-4 w-4" />
              </Button>
            </TooltipTrigger><TooltipContent side="bottom">Bulk select</TooltipContent></Tooltip>
          )}
          <Tooltip><TooltipTrigger asChild>
            <Button size="icon" variant={showTrash ? "secondary" : "outline"} aria-label="Trash"
              onClick={() => { const next = !showTrash; setShowTrash(next); if (next) { setOnlyBookmarked(false); bulk.setBulkMode(false); } }}>
              <Trash2 className={`h-4 w-4 ${showTrash ? "fill-current" : ""}`} />
            </Button>
          </TooltipTrigger><TooltipContent side="bottom">Trash</TooltipContent></Tooltip>
          {!showTrash && (
            <Tooltip><TooltipTrigger asChild>
              <Button size="icon" variant="outline" aria-label="New folder"
                onClick={() => setFolderDialog({ mode: "new" })}>
                <FolderPlus className="h-4 w-4" />
              </Button>
            </TooltipTrigger><TooltipContent side="bottom">New folder</TooltipContent></Tooltip>
          )}
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild><Button>+ Dashboard</Button></DialogTrigger>
            <DialogContent>
              <DialogHeader><DialogTitle>New dashboard</DialogTitle></DialogHeader>
              <div className="flex gap-2 py-2">
                <Input autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") onCreate(); }} />
                <Button onClick={onCreate} disabled={create.isPending}>Create</Button>
              </div>
            </DialogContent>
          </Dialog>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search dashboards…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {bulk.bulkMode && bulk.someSelected && !showTrash && (
              <>
                <Button variant="outline" size="sm" onClick={() => setFolderDialog({ mode: "bulkmove" })}>
                  Move {bulk.selectedIds.length} to folder…
                </Button>
                <Button variant="destructive" size="sm" onClick={doBulkTrash}>
                  Move {bulk.selectedIds.length} to Trash
                </Button>
              </>
            )}
          </div>

          {rows.length > 0 ? (
            <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  {bulk.bulkMode && !showTrash && <th className="w-8 py-1"><Checkbox checked={bulk.allSelected} onCheckedChange={bulk.toggleAll} aria-label="Select all" /></th>}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="card_count" label="Cards" current={sort} onSort={onSort} />
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
                        colSpan={bulk.bulkMode ? 5 : 4}
                        collapsed={isCollapsed}
                        onToggle={() => collapse.toggle(key)}
                        onShare={group.folder ? () => setFolderShare({ id: group.folder!.id, name: group.folder!.name }) : undefined}
                        onRename={group.folder ? () => setFolderDialog({ mode: "rename", folder: group.folder! }) : undefined}
                        onDelete={group.folder ? () => removeFolder.mutateAsync(group.folder!.id).then(() => toast.success("Folder deleted")).catch((e) => toast.error(String(e))) : undefined}
                        onDropItem={(id) => setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }).catch((e) => toast.error(String(e)))}
                      />
                      {!isCollapsed && group.rows.map(renderRow)}
                    </tbody>
                  );
                })
              )}
            </table>
            </div>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked dashboards." : "No dashboards match the current filters."}</p>
          )}
        </>
      ) : (
        <p className="text-muted-foreground">{showTrash ? "Trash is empty." : "No dashboards yet."}</p>
      )}

      <FolderNameDialog
        open={!!folderDialog}
        title={folderDialog?.mode === "rename" ? "Rename folder" : folderDialog?.mode === "move" ? "Move to new folder" : folderDialog?.mode === "bulkmove" ? "Move to folder" : "New folder"}
        submitLabel={folderDialog?.mode === "rename" ? "Rename" : folderDialog?.mode === "move" || folderDialog?.mode === "bulkmove" ? "Move" : "Create"}
        initialValue={folderDialog?.mode === "rename" ? folderDialog.folder!.name : ""}
        onOpenChange={(o) => { if (!o) setFolderDialog(null); }}
        onSubmit={(name) => {
          const d = folderDialog!;
          if (d.mode === "rename") renameFolder.mutateAsync({ id: d.folder!.id, name }).catch((e) => toast.error(String(e)));
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.dash!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMove(name);
          else createFolder.mutateAsync(name).then(() => toast.success("Folder created")).catch((e) => toast.error(String(e)));
        }}
      />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => { if (!pendingDelete) return; await guard(remove.mutateAsync(pendingDelete.id)); setPendingDelete(null); }}
      />

      {shareTarget && (
        <ShareDialog
          ws={ws}
          kind="dashboard"
          id={shareTarget.id}
          name={shareTarget.name}
          folderId={shareTarget.folder_id ?? null}
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
    </div>
  );
}
