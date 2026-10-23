import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search, Star, Trash2, FolderPlus, ListChecks } from "lucide-react";
import { toast } from "sonner";
import { BulkMoveToFolderMenu, moveEach } from "@/components/explore/BulkMoveToFolderMenu";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { EmptyState } from "@/components/explore/EmptyState";
import { useCharts, useChartMutations, type Chart } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ChartActionsMenu } from "@/components/chart/ChartActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { folderCollapseKey, groupByFolder, showFolderSections, useCollapsed, UNCATEGORIZED_KEY } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { EntityLink } from "@/components/ui/entity-link";
import { ScrollX } from "@/components/ui/scroll-cue";

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
  const collapse = useCollapsed(folderCollapseKey(ws, "charts"));

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

  const filtering = query.trim() !== "" || typeFilter !== FACET_ALL || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  /**
   * Whether this table is drawn with folder SECTION HEADERS at all. The gate
   * and its full argument live in `showFolderSections`
   * (`src/components/explore/folders.ts`); `dataapi/Endpoints.tsx` and
   * `sqllab/SavedTab.tsx` had it and these five did not, so a workspace with no
   * folders met a lone collapsible "Uncategorized (1)" introducing a taxonomy
   * it is not using.
   *
   * One flag, read by BOTH the render loop below and `visibleRows` — they are
   * two derivations of the same question, and gating only the render strands
   * rows behind a collapse state restored from localStorage after the last
   * folder was deleted.
   */
  const showFolderHeaders = showFolderSections(folders, showTrash);

  /**
   * The rows actually ON SCREEN — not `rows`, which is only what survived
   * filtering and sorting: a collapsed folder's rows are in `groups` but not in
   * front of the user.
   *
   * This is what `useBulkSelect` must be fed; the full argument for why — and
   * what breaks when a caller passes more — lives on the hook itself
   * (`src/hooks/useBulkSelect.ts`), because that is where the next caller will
   * be reading.
   */
  const visibleRows = useMemo(
    () =>
      // Keyed off the same flag the table renders from, deliberately. Without
      // a header there is no collapse control, so nothing CAN be collapsed and
      // `rows` is what is on screen — that covers the trash (a flat table) and
      // the folderless workspace alike.
      showFolderHeaders
        ? groups.flatMap((g) => (collapse.isCollapsed(g.folder?.id ?? UNCATEGORIZED_KEY) ? [] : g.rows))
        : rows,
    [showFolderHeaders, rows, groups, collapse],
  );

  // One list behind both the count and the trash: `bulk.selectedIds` is derived
  // from exactly what was passed in here, so "Move n to Trash" cannot name one
  // number next to an action that reaches a different set.
  const bulk = useBulkSelect(visibleRows);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  async function doBulkTrash() {
    for (const id of bulk.selectedIds) { try { await trash.mutateAsync(id); } catch (e) { toast.error(String(e)); } }
    toast.success("Moved to Trash"); bulk.clear();
  }
  /**
   * Move the selected rows to a folder that already exists (`null` = out of any folder): one
   * request and one verdict per row. See `BulkMoveToFolderMenu` for why this is a picker.
   */
  async function doBulkMoveTo(folderId: string | null) {
    const moved = await moveEach(
      bulk.selectedIds,
      (id) => setFolder.mutateAsync({ id, folder_id: folderId }),
      (e) => toast.error(String(e)),
    );
    if (moved > 0) toast.success(`Moved ${moved} ${moved === 1 ? "chart" : "charts"}`);
    bulk.clear();
  }
  /** "New folder…" from the bulk menu: make the folder, then move into it. A name that already
   *  exists (case-insensitively) is that folder, not a duplicate of it. */
  async function doBulkMoveToNew(name: string) {
    const trimmed = name.trim();
    let folderId: string;
    try {
      const existing = folders.find((f) => f.name.toLowerCase() === trimmed.toLowerCase());
      folderId = existing ? existing.id : (await createFolder.mutateAsync(trimmed)).id;
    } catch (e) { toast.error(String(e)); return; }
    await doBulkMoveTo(folderId);
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
            <EntityLink className="inline-flex items-center gap-2" to={`/explore/${ws}/charts/${c.id}`}>
              {Icon && <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />}
              {c.name}
            </EntityLink>
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
    <ListPageShell
      title={showTrash ? "Trash" : "Charts"}
      toolbar={
        <>
          {!showTrash && (
            <ListToolbarButton
              icon={Star}
              label="Bookmarks"
              active={onlyBookmarked}
              filled
              onClick={() => setOnlyBookmarked((v) => !v)}
            />
          )}
          {!showTrash && hasData && (
            <ListToolbarButton
              icon={ListChecks}
              label="Bulk select"
              active={bulk.bulkMode}
              onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
            />
          )}
          {/* NOT gated on `hasData`: this is a view switch, not a filter. It
              selects a different server query, and an empty dataset list says
              nothing about whether the trash is empty. Gated, it would be the
              only way OUT of an empty trash and would vanish exactly when it is
              the only control left. */}
          <ListToolbarButton
            icon={Trash2}
            label="Trash"
            active={showTrash}
            filled
            onClick={() => {
              const next = !showTrash;
              setShowTrash(next);
              if (next) {
                setOnlyBookmarked(false);
                bulk.setBulkMode(false);
              }
            }}
          />
          {!showTrash && (
            <ListToolbarButton
              icon={FolderPlus}
              label="New folder"
              onClick={() => setFolderDialog({ mode: "new" })}
            />
          )}
        </>
      }
      action={<Button asChild><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>}
    >
      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load charts."
        hasData={hasData}
        skeletonCols={4}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              title="No charts yet."
              description="Build a chart from a dataset."
              action={
                <Button asChild>
                  <Link to={`/explore/${ws}/charts/new`}>Create your first chart</Link>
                </Button>
              }
            />
          )
        }
      >
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
                <BulkMoveToFolderMenu
                  count={bulk.selectedIds.length}
                  folders={folders}
                  onMove={doBulkMoveTo}
                  onNewFolder={() => setFolderDialog({ mode: "bulkmove" })}
                />
                <Button variant="destructive" size="sm" onClick={doBulkTrash}>
                  Move {bulk.selectedIds.length} to Trash
                </Button>
              </>
            )}
          </div>

          <div className="mb-3">
            <SummaryStrip
              items={[
                { label: "charts", singular: "chart", value: rows.length },
                ...(folders.length ? [{ label: "folders", singular: "folder", value: folders.length }] : []),
              ]}
            />
          </div>

          {rows.length > 0 ? (
            <ScrollX>
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
              {/* Flat wherever there is nothing to head — see
                  `showFolderHeaders`: the trash always, and a workspace that has
                  no folders at all. */}
              {!showFolderHeaders ? (
                <tbody>{rows.map(renderRow)}</tbody>
              ) : (
                groups.map((group) => {
                  const key = group.folder?.id ?? UNCATEGORIZED_KEY;
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
            </ScrollX>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked charts." : "No charts match the current filters."}</p>
          )}
      </ListStates>

      <FolderNameDialog
        open={!!folderDialog}
        title={folderDialog?.mode === "rename" ? "Rename folder" : folderDialog?.mode === "move" ? "Move to new folder" : folderDialog?.mode === "bulkmove" ? "Move to new folder" : "New folder"}
        submitLabel={folderDialog?.mode === "rename" ? "Rename" : folderDialog?.mode === "move" || folderDialog?.mode === "bulkmove" ? "Move" : "Create"}
        initialValue={folderDialog?.mode === "rename" ? folderDialog.folder!.name : ""}
        onOpenChange={(o) => { if (!o) setFolderDialog(null); }}
        onSubmit={(name) => {
          const d = folderDialog!;
          if (d.mode === "rename") renameFolder.mutateAsync({ id: d.folder!.id, name }).catch((e) => toast.error(String(e)));
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.chart!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMoveToNew(name);
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
    </ListPageShell>
  );
}
