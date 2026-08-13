import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search, Star, Trash2, FolderPlus, ListChecks } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/components/ui/tooltip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { useCharts, useChartMutations, type Chart } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ChartActionsMenu } from "@/components/chart/ChartActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
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
  const { data: charts, isLoading, isError, error } = useCharts(ws, { trashed: showTrash });
  const { remove, duplicate, trash, restore, addBookmark, removeBookmark } = useChartMutations(ws);
  const { data: folders = [] } = useFolders(ws, "charts");
  const setFolder = useSetFolder(ws, "charts");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "charts");
  const collapse = useCollapsed(`folders:${ws}:charts`);

  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState(FACET_ALL);
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<Chart | null>(null);
  const [shareTarget, setShareTarget] = useState<Chart | null>(null);
  const [folderShare, setFolderShare] = useState<{ id: string; name: string } | null>(null);
  const [folderDialog, setFolderDialog] = useState<{ mode: "new" | "rename" | "move" | "bulkmove"; chart?: Chart; folder?: { id: string; name: string } } | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (charts ?? []).filter((c) => {
      if (onlyBookmarked && !c.bookmarked) return false;
      if (typeFilter !== FACET_ALL && c.viz_type !== typeFilter) return false;
      if (!q) return true;
      return [c.name, vizLabel(c.viz_type)].join(" ").toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "viz_type") cmp = vizLabel(a.viz_type).localeCompare(vizLabel(b.viz_type));
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [charts, onlyBookmarked, query, typeFilter, sort]);

  const bulk = useBulkSelect(rows);
  const filtering = query.trim() !== "" || typeFilter !== FACET_ALL || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
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
                <Star className={`h-4 w-4 ${c.bookmarked ? "fill-highlight text-highlight" : "text-muted-foreground"}`} />
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
            onNewFolder={(c) => setFolderDialog({ mode: "move", chart: c })}
            onTrash={(x) => trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")).catch((e) => toast.error(String(e)))}
            onRestore={(x) => restore.mutateAsync(x.id).catch((e) => toast.error(String(e)))}
            onDeletePermanent={(x) => setPendingDelete(x)}
            onShare={(x) => setShareTarget(x)} />
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
          <Button asChild><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : isError ? (
        <Alert variant="destructive">
          <AlertDescription>{(error as Error)?.message ?? "Failed to load charts."}</AlertDescription>
        </Alert>
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search charts…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {!showTrash && (
              <FacetSelect
                label="Type"
                value={typeFilter}
                onChange={setTypeFilter}
                options={presentTypes.map((t) => ({ value: t.key, label: t.name }))}
              />
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

          <div className="mb-3">
            <SummaryStrip
              items={[
                { label: "charts", value: rows.length },
                ...(folders.length ? [{ label: "folders", value: folders.length }] : []),
              ]}
            />
          </div>

          {rows.length > 0 ? (
            <div className="overflow-x-auto">
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

      <FolderNameDialog
        open={!!folderDialog}
        title={folderDialog?.mode === "rename" ? "Rename folder" : folderDialog?.mode === "move" ? "Move to new folder" : folderDialog?.mode === "bulkmove" ? "Move to folder" : "New folder"}
        submitLabel={folderDialog?.mode === "rename" ? "Rename" : folderDialog?.mode === "move" || folderDialog?.mode === "bulkmove" ? "Move" : "Create"}
        initialValue={folderDialog?.mode === "rename" ? folderDialog.folder!.name : ""}
        onOpenChange={(o) => { if (!o) setFolderDialog(null); }}
        onSubmit={(name) => {
          const d = folderDialog!;
          if (d.mode === "rename") renameFolder.mutateAsync({ id: d.folder!.id, name }).catch((e) => toast.error(String(e)));
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.chart!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMove(name);
          else createFolder.mutateAsync(name).then(() => toast.success("Folder created")).catch((e) => toast.error(String(e)));
        }}
      />

      {shareTarget && (
        <ShareDialog
          ws={ws}
          kind="chart"
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
