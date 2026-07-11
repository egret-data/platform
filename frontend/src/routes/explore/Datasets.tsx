import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search, Star, Trash2, FolderPlus, ListChecks } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/components/ui/tooltip";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { useDatabases } from "@/api/sqllab";
import { useDatasets, useDataset, useDatasetListMutations, useDeleteDataset, type DatasetSummary } from "@/api/datasets";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
import { DatasetActionsMenu } from "@/components/dataset/DatasetActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";

type SortKey = "name" | "column_count" | "updated_at";

function sourceOf(d: DatasetSummary): string {
  return d.kind === "virtual" ? "SQL" : [d.catalog, d.schema, d.table].filter(Boolean).join(".");
}

export default function Datasets() {
  const ws = useParams().workspace!;
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading } = useDatasets(ws, { trashed: showTrash });
  const { data: databases } = useDatabases(ws);
  const { trash, restore, addBookmark, removeBookmark } = useDatasetListMutations(ws);
  const del = useDeleteDataset(ws);
  const { data: folders = [] } = useFolders(ws, "datasets");
  const setFolder = useSetFolder(ws, "datasets");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "datasets");
  const collapse = useCollapsed(`folders:${ws}:datasets`);

  const [createOpen, setCreateOpen] = useState(false);
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState("all");
  const [dbFilter, setDbFilter] = useState("all");
  const [sort, setSort] = useState<{ key: SortKey; dir: "asc" | "desc" }>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<DatasetSummary | null>(null);
  const [shareTarget, setShareTarget] = useState<DatasetSummary | null>(null);
  const [folderDialog, setFolderDialog] = useState<{ mode: "new" | "rename" | "move" | "bulkmove"; ds?: DatasetSummary; folder?: { id: string; name: string } } | null>(null);
  const [dupId, setDupId] = useState<string | null>(null);
  const dup = useDataset(ws, dupId);

  const dbName = (id: string) => {
    const d = databases?.find((x) => x.id === id);
    return d ? `${d.name} (${d.engine})` : id.slice(0, 8);
  };

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((d) => {
      if (onlyBookmarked && !d.bookmarked) return false;
      if (typeFilter !== "all" && d.kind !== typeFilter) return false;
      if (dbFilter !== "all" && d.database_id !== dbFilter) return false;
      if (!q) return true;
      const hay = [d.name, d.description, d.schema ?? "", d.table ?? "", d.catalog ?? "", dbName(d.database_id)]
        .join(" ").toLowerCase();
      return hay.includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "column_count") cmp = a.column_count - b.column_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, databases, onlyBookmarked, query, typeFilter, dbFilter, sort]);

  const bulk = useBulkSelect(rows);
  const filtering = query.trim() !== "" || typeFilter !== "all" || dbFilter !== "all" || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  const hasData = !!data && data.length > 0;
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  function setSortKey(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
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

  const renderRow = (d: DatasetSummary) => (
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
          <Link className="font-medium hover:underline" to={d.id}>{d.name}</Link>
        </span>
      </td>
      <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{d.kind}</span></td>
      <td className="text-muted-foreground">{dbName(d.database_id)}</td>
      <td className="font-mono text-xs text-muted-foreground">{sourceOf(d)}</td>
      <td className="tabular-nums">{d.column_count}</td>
      <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
      <td><DatasetActionsMenu ws={ws} ds={d} trashed={showTrash} folders={folders}
        onDuplicate={(x) => setDupId(x.id)}
        onMove={(x, fid) => setFolder.mutateAsync({ id: x.id, folder_id: fid }).catch((e) => toast.error(String(e)))}
        onNewFolder={(x) => setFolderDialog({ mode: "move", ds: x })}
        onTrash={(x) => trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")).catch((e) => toast.error(String(e)))}
        onRestore={(x) => restore.mutateAsync(x.id).catch((e) => toast.error(String(e)))}
        onDeletePermanent={(x) => setPendingDelete(x)}
        onShare={(x) => setShareTarget(x)} /></td>
    </tr>
  );

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Datasets"}</h1>
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
          {!showTrash && <Button onClick={() => setCreateOpen(true)}>Create</Button>}
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={6} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search name, schema, table…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {!showTrash && (
              <>
                <Select value={typeFilter} onValueChange={setTypeFilter}>
                  <SelectTrigger className="h-9 w-36"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">All types</SelectItem>
                    <SelectItem value="physical">Physical</SelectItem>
                    <SelectItem value="virtual">Virtual</SelectItem>
                  </SelectContent>
                </Select>
                <Select value={dbFilter} onValueChange={setDbFilter}>
                  <SelectTrigger className="h-9 w-48"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">All databases</SelectItem>
                    {(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}
                  </SelectContent>
                </Select>
              </>
            )}
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
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={setSortKey} />
                  <th>Kind</th>
                  <th>Database</th>
                  <th>Source</th>
                  <SortableHeader sortKey="column_count" label="Columns" current={sort} onSort={setSortKey} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={setSortKey} />
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
                        colSpan={bulk.bulkMode ? 8 : 7}
                        collapsed={isCollapsed}
                        onToggle={() => collapse.toggle(key)}
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
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked datasets." : "No datasets match the current filters."}</p>
          )}
        </>
      ) : showTrash ? (
        <p className="text-muted-foreground">Trash is empty.</p>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No datasets yet. A dataset is a named, reusable source for charts.</p>
          <Button className="mt-3" onClick={() => setCreateOpen(true)}>Create your first dataset</Button>
        </div>
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
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.ds!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMove(name);
          else createFolder.mutateAsync(name).then(() => toast.success("Folder created")).catch((e) => toast.error(String(e)));
        }}
      />

      <CreateDatasetDialog ws={ws} open={createOpen} onOpenChange={setCreateOpen} />

      {dup.data && (
        <CreateDatasetDialog
          key={`dup-${dup.data.id}`}
          ws={ws}
          open
          onOpenChange={(o) => { if (!o) setDupId(null); }}
          initialTab="sql"
          initialSql={dup.data.sql ?? ""}
          initialDbId={dup.data.database_id}
          initialName={`copy of ${dup.data.name}`}
        />
      )}

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => { if (!pendingDelete) return; await guard(del.mutateAsync(pendingDelete.id)); setPendingDelete(null); }}
      />

      {shareTarget && (
        <ShareDialog
          ws={ws}
          kind="dataset"
          id={shareTarget.id}
          name={shareTarget.name}
          folderId={shareTarget.folder_id ?? null}
          open={shareTarget !== null}
          onOpenChange={(o) => { if (!o) setShareTarget(null); }}
        />
      )}
    </div>
  );
}
