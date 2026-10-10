import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search, Star, Trash2, FolderPlus, ListChecks, BadgeCheck } from "lucide-react";
import { toast } from "sonner";
import { BulkMoveToFolderMenu, moveEach } from "@/components/explore/BulkMoveToFolderMenu";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Badge } from "@/components/ui/badge";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { EmptyState } from "@/components/explore/EmptyState";
import { useDatabases } from "@/api/sqllab";
import { useDatasets, useDataset, useDatasetListMutations, useDeleteDataset, type DatasetSummary } from "@/api/datasets";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
import { DatasetActionsMenu } from "@/components/dataset/DatasetActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
import { TagDialog } from "@/components/access/TagEditor";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader } from "@/components/explore/SortableHeader";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { folderCollapseKey, groupByFolder, showFolderSections, useCollapsed, UNCATEGORIZED_KEY } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";

type SortKey = "name" | "column_count" | "updated_at";

function sourceOf(d: DatasetSummary): string {
  return d.kind === "virtual" ? "SQL" : [d.catalog, d.schema, d.table].filter(Boolean).join(".");
}

export default function Datasets() {
  const ws = useParams().workspace!;
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading, isError, error } = useDatasets(ws, { trashed: showTrash });
  const { data: databases } = useDatabases(ws);
  const { trash, restore, addBookmark, removeBookmark } = useDatasetListMutations(ws);
  const del = useDeleteDataset(ws);
  const { data: folders = [] } = useFolders(ws, "datasets");
  const setFolder = useSetFolder(ws, "datasets");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "datasets");
  const collapse = useCollapsed(folderCollapseKey(ws, "datasets"));

  const [createOpen, setCreateOpen] = useState(false);
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState(FACET_ALL);
  const [dbFilter, setDbFilter] = useState(FACET_ALL);
  const [sort, setSort] = useState<{ key: SortKey; dir: "asc" | "desc" }>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<DatasetSummary | null>(null);
  const [shareTarget, setShareTarget] = useState<DatasetSummary | null>(null);
  const [tagsTarget, setTagsTarget] = useState<DatasetSummary | null>(null);
  const [folderShare, setFolderShare] = useState<{ id: string; name: string } | null>(null);
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
      if (typeFilter !== FACET_ALL && d.kind !== typeFilter) return false;
      if (dbFilter !== FACET_ALL && d.database_id !== dbFilter) return false;
      if (!q) return true;
      const hay = [d.name, d.description, d.schema ?? "", d.table ?? "", d.catalog ?? "", dbName(d.database_id)]
        .join(" ").toLowerCase();
      return hay.includes(q);
    });
    r = [...r].sort((a, b) => {
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "column_count") cmp = a.column_count - b.column_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, databases, onlyBookmarked, query, typeFilter, dbFilter, sort]);

  const filtering = query.trim() !== "" || typeFilter !== FACET_ALL || dbFilter !== FACET_ALL || onlyBookmarked;
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

  const hasData = !!data && data.length > 0;
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  function setSortKey(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
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
    if (moved > 0) toast.success(`Moved ${moved} ${moved === 1 ? "dataset" : "datasets"}`);
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

  const renderRow = (d: DatasetSummary) => (
    <tr key={d.id} className="border-t hover:bg-accent"
      draggable={!showTrash} onDragStart={!showTrash ? (e) => e.dataTransfer.setData("text/plain", d.id) : undefined}>
      {bulk.bulkMode && !showTrash && <td className="py-1"><Checkbox checked={bulk.selected.has(d.id)} onCheckedChange={() => bulk.toggle(d.id)} aria-label={`Select ${d.name}`} /></td>}
      <td className="py-1 pl-6">
        <span className="inline-flex items-center gap-1">
          {!showTrash && (
            <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${d.bookmarked ? "unbookmark" : "bookmark"} ${d.name}`}
              onClick={() => guard((d.bookmarked ? removeBookmark : addBookmark).mutateAsync(d.id))}>
              <Star className={`h-4 w-4 ${d.bookmarked ? "fill-highlight text-highlight" : "text-muted-foreground"}`} />
            </Button>
          )}
          <Link className="font-medium hover:underline" to={d.id}>{d.name}</Link>
          {d.certified && <BadgeCheck className="h-3.5 w-3.5 text-primary" aria-label="Certified" />}
        </span>
      </td>
      <td><Badge variant="secondary">{d.kind}</Badge></td>
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
        onShare={(x) => setShareTarget(x)}
        onTags={(x) => setTagsTarget(x)} /></td>
    </tr>
  );

  return (
    <ListPageShell
      title={showTrash ? "Trash" : "Datasets"}
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
      action={!showTrash ? <Button onClick={() => setCreateOpen(true)}>Create</Button> : null}
    >
      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load datasets."
        hasData={hasData}
        skeletonCols={6}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              title="No datasets yet."
              description="A dataset is a named, reusable source for charts."
              action={<Button onClick={() => setCreateOpen(true)}>Create your first dataset</Button>}
            />
          )
        }
      >
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search name, schema, table…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {!showTrash && (
              <>
                <FacetSelect
                  label="Type"
                  value={typeFilter}
                  onChange={setTypeFilter}
                  options={[
                    { value: "physical", label: "Physical" },
                    { value: "virtual", label: "Virtual" },
                  ]}
                />
                <FacetSelect
                  label="Database"
                  value={dbFilter}
                  onChange={setDbFilter}
                  options={(databases ?? []).map((d) => ({ value: d.id, label: `${d.name} (${d.engine})` }))}
                />
              </>
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
                { label: "datasets", singular: "dataset", value: rows.length },
                ...(folders.length ? [{ label: "folders", singular: "folder", value: folders.length }] : []),
              ]}
            />
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
                        colSpan={bulk.bulkMode ? 8 : 7}
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
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked datasets." : "No datasets match the current filters."}</p>
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
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.ds!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMoveToNew(name);
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

      {tagsTarget && (
        <TagDialog
          ws={ws}
          kind="dataset"
          id={tagsTarget.id}
          name={tagsTarget.name}
          open={tagsTarget !== null}
          onOpenChange={(o) => { if (!o) setTagsTarget(null); }}
        />
      )}
    </ListPageShell>
  );
}
