import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search, Star } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { useCharts, useChartMutations, type Chart } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ChartActionsMenu } from "@/components/chart/ChartActionsMenu";
import { BulkToggle } from "@/components/explore/BulkBar";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";

type SortKey = "name" | "viz_type" | "updated_at";
const vizMeta = (v: string) => CHART_TYPES.find((t) => t.key === v);
const vizLabel = (v: string) => vizMeta(v)?.name ?? v;

export default function Charts() {
  const ws = useParams().workspace!;
  const [showTrash, setShowTrash] = useState(false);
  const { data: charts, isLoading } = useCharts(ws, { trashed: showTrash });
  const { remove, duplicate, trash, restore, addBookmark, removeBookmark } = useChartMutations(ws);
  const { data: folders = [] } = useFolders(ws, "charts");
  const setFolder = useSetFolder(ws, "charts");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "charts");
  const collapse = useCollapsed(`folders:${ws}:charts`);

  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState("all");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<Chart | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (charts ?? []).filter((c) => {
      if (onlyBookmarked && !c.bookmarked) return false;
      if (typeFilter !== "all" && c.viz_type !== typeFilter) return false;
      if (!q) return true;
      return [c.name, vizLabel(c.viz_type)].join(" ").toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "viz_type") cmp = vizLabel(a.viz_type).localeCompare(vizLabel(b.viz_type));
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [charts, onlyBookmarked, query, typeFilter, sort]);

  const bulk = useBulkSelect(rows);
  const filtering = query.trim() !== "" || typeFilter !== "all" || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  async function doBulkTrash() {
    for (const id of bulk.selectedIds) { try { await trash.mutateAsync(id); } catch (e) { toast.error(String(e)); } }
    toast.success("Moved to Trash"); bulk.clear();
  }
  async function doBulkMove() {
    const name = prompt("Move to folder (name; blank = Uncategorized)");
    if (name === null) return;
    const trimmed = name.trim();
    try {
      let folderId: string | null = null;
      if (trimmed !== "") {
        const existing = folders.find((f) => f.name.toLowerCase() === trimmed.toLowerCase());
        folderId = existing ? existing.id : (await createFolder.mutateAsync(trimmed)).id;
      }
      for (const id of bulk.selectedIds) await setFolder.mutateAsync({ id, folder_id: folderId });
    } catch (e) { toast.error(String(e)); }
    bulk.clear();
  }

  const hasData = !!charts && charts.length > 0;
  const presentTypes = CHART_TYPES.filter((t) => (charts ?? []).some((c) => c.viz_type === t.key));

  const renderRow = (c: Chart) => {
    const Icon = vizMeta(c.viz_type)?.icon;
    return (
      <tr key={c.id} className="border-t hover:bg-accent"
        draggable={!showTrash} onDragStart={!showTrash ? (e) => e.dataTransfer.setData("text/plain", c.id) : undefined}>
        {bulk.bulkMode && !showTrash && <td className="py-1"><Checkbox checked={bulk.selected.has(c.id)} onCheckedChange={() => bulk.toggle(c.id)} aria-label={`Select ${c.name}`} /></td>}
        <td className="py-1 pl-6">
          <span className="inline-flex items-center gap-1">
            {!showTrash && (
              <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${c.bookmarked ? "unbookmark" : "bookmark"} ${c.name}`}
                onClick={() => guard((c.bookmarked ? removeBookmark : addBookmark).mutateAsync(c.id))}>
                <Star className={`h-4 w-4 ${c.bookmarked ? "fill-amber-400 text-amber-400" : "text-muted-foreground"}`} />
              </Button>
            )}
            <Link className="inline-flex items-center gap-2 font-medium hover:underline" to={`/explore/${ws}/charts/${c.id}`}>
              {Icon && <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />}
              {c.name}
            </Link>
          </span>
        </td>
        <td className="text-muted-foreground">{vizLabel(c.viz_type)}</td>
        <td className="text-muted-foreground"><RelativeTime iso={c.updated_at} /></td>
        <td>
          <ChartActionsMenu ws={ws} chart={c} trashed={showTrash} folders={folders}
            onDuplicate={(x) => duplicate.mutateAsync(x.id).then(() => toast.success("Duplicated")).catch((e) => toast.error(String(e)))}
            onMove={(x, fid) => setFolder.mutateAsync({ id: x.id, folder_id: fid }).catch((e) => toast.error(String(e)))}
            onNewFolder={(x) => { const n = prompt("New folder name"); if (n) createFolder.mutateAsync(n).then((f) => setFolder.mutateAsync({ id: x.id, folder_id: f.id })).catch((e) => toast.error(String(e))); }}
            onTrash={(x) => trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")).catch((e) => toast.error(String(e)))}
            onRestore={(x) => restore.mutateAsync(x.id).catch((e) => toast.error(String(e)))}
            onDeletePermanent={(x) => setPendingDelete(x)} />
        </td>
      </tr>
    );
  };

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Charts"}</h1>
        <div className="flex items-center gap-2">
          {!showTrash && (
            <Button variant={onlyBookmarked ? "default" : "outline"} onClick={() => setOnlyBookmarked((v) => !v)}>★ Bookmarks</Button>
          )}
          <Button variant={showTrash ? "default" : "outline"} onClick={() => { const next = !showTrash; setShowTrash(next); if (next) { setOnlyBookmarked(false); bulk.setBulkMode(false); } }}>
            {showTrash ? "← Charts" : "Trash"}
          </Button>
          {!showTrash && hasData && (
            <BulkToggle active={bulk.bulkMode} onToggle={() => bulk.setBulkMode(!bulk.bulkMode)} />
          )}
          {!showTrash && (
            <Button variant="outline" onClick={() => { const n = prompt("New folder name"); if (n && n.trim()) createFolder.mutateAsync(n.trim()).then(() => toast.success("Folder created")).catch((e) => toast.error(String(e))); }}>+ New folder</Button>
          )}
          <Button asChild><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search charts…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {!showTrash && (
              <Select value={typeFilter} onValueChange={setTypeFilter}>
                <SelectTrigger className="h-9 w-44"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All types</SelectItem>
                  {presentTypes.map((t) => <SelectItem key={t.key} value={t.key}>{t.name}</SelectItem>)}
                </SelectContent>
              </Select>
            )}
            {bulk.bulkMode && bulk.someSelected && !showTrash && (
              <>
                <Button variant="outline" size="sm" onClick={doBulkMove}>
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
                  {bulk.bulkMode && !showTrash && <th className="w-8 py-1"><Checkbox checked={bulk.allSelected} onCheckedChange={bulk.toggleAll} aria-label="Select all" /></th>}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="viz_type" label="Type" current={sort} onSort={onSort} />
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
                        onRename={group.folder ? () => {
                          const n = prompt("Rename folder", group.folder!.name);
                          if (n) renameFolder.mutateAsync({ id: group.folder!.id, name: n }).catch((e) => toast.error(String(e)));
                        } : undefined}
                        onDelete={group.folder ? () => removeFolder.mutateAsync(group.folder!.id).then(() => toast.success("Folder deleted")).catch((e) => toast.error(String(e))) : undefined}
                        onDropItem={(id) => setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }).catch((e) => toast.error(String(e)))}
                      />
                      {!isCollapsed && group.rows.map(renderRow)}
                    </tbody>
                  );
                })
              )}
            </table>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked charts." : "No charts match the current filters."}</p>
          )}
        </>
      ) : showTrash ? (
        <p className="text-muted-foreground">Trash is empty.</p>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No charts yet. Build a chart from a dataset.</p>
          <Button asChild className="mt-3"><Link to={`/explore/${ws}/charts/new`}>Create your first chart</Link></Button>
        </div>
      )}

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => { if (!pendingDelete) return; await guard(remove.mutateAsync(pendingDelete.id)); setPendingDelete(null); }}
      />
    </div>
  );
}
