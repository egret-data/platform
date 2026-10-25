import { useEffect, useMemo, useState } from "react";
import { Search, Star, Trash2, FolderPlus, ListChecks, BadgeCheck } from "lucide-react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { BulkMoveToFolderMenu, moveEach } from "@/components/explore/BulkMoveToFolderMenu";
import { useDashboards, useDashboardMutations, type DashboardSummary } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { ConfirmFolderDeleteDialog, ConfirmTrashDialog } from "@/components/explore/ConfirmActionDialog";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { EmptyState } from "@/components/explore/EmptyState";
import { DashboardActionsMenu } from "@/components/chart/dashboard/DashboardActionsMenu";
import { ShareDialog } from "@/components/sharing/ShareDialog";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { folderCollapseKey, groupByFolder, showFolderSections, useCollapsed, UNCATEGORIZED_KEY } from "@/components/explore/folders";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { EntityLink } from "@/components/ui/entity-link";
import { ScrollX } from "@/components/ui/scroll-cue";

type SortKey = "name" | "card_count" | "updated_at";
const DASHBOARD_NOUN = { singular: "dashboard", plural: "dashboards" };

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading, isError, error } = useDashboards(ws, { trashed: showTrash });
  const { create, remove, duplicate, trash, restore, addBookmark, removeBookmark } = useDashboardMutations(ws);
  const { data: folders = [] } = useFolders(ws, "dashboards");
  const setFolder = useSetFolder(ws, "dashboards");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "dashboards");
  const collapse = useCollapsed(folderCollapseKey(ws, "dashboards"));
  const [searchParams] = useSearchParams();
  const focusFolder = searchParams.get("folder");
  // The info sidebar's "Saved in" link lands here. Reveal that section once on arrival; the
  // visitor's own collapse choices afterwards are theirs and are not re-applied.
  useEffect(() => { if (focusFolder) collapse.expand(focusFolder); }, [focusFolder, collapse]);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<DashboardSummary | null>(null);
  const [shareTarget, setShareTarget] = useState<DashboardSummary | null>(null);
  // Moving to the Trash and deleting a folder are confirmed first (KI-075): what is being
  // trashed — one row, or the bulk selection — and which folder is being deleted.
  const [pendingTrash, setPendingTrash] = useState<DashboardSummary | "bulk" | null>(null);
  const [pendingFolderDelete, setPendingFolderDelete] = useState<{ id: string; name: string } | null>(null);
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
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "card_count") cmp = a.card_count - b.card_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, onlyBookmarked, query, sort]);

  /**
   * Whether to offer the mutating affordances at all — duplicate, move, trash, delete, create,
   * folders, bulk. Withheld rather than disabled, for the reason the Edit button on the detail
   * page is: a greyed control advertises a capability the reader does not have, and here the
   * controls are DESTRUCTIVE. A reader used to be able to select twenty dashboards and press
   * "Move to Trash" before learning the server refuses.
   *
   * It cannot be derived from the session: `me.permissions` is built from PLATFORM grants only,
   * so workspace-scoped `dashboard:manage` never appears there. The server answers instead, and
   * the answer is the same on every row (workspace-scoped) — reading row 0 is not a shortcut.
   *
   * `data`, not `rows`: `rows` is search-filtered, so typing a query that matches nothing would
   * otherwise revoke the toolbar.
   *
   * The empty list is the one case with no answer — there is no row to carry it. Defaulting to
   * `true` there leaves the PRE-EXISTING behaviour intact for a reader in an empty workspace
   * (click Create, get 403); defaulting to `false` would hide Create from a legitimate author
   * in a fresh workspace, which blocks real work. Wrong-but-recoverable beats blocking.
   */
  const canManage = data && data.length > 0 ? data[0].can_manage !== false : true;

  const filtering = query.trim() !== "" || onlyBookmarked;
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
  async function onCreate() {
    if (!name.trim()) { toast.error("Name required"); return; }
    // The description key is OMITTED when blank rather than sent as "". The backend column is
    // nullable and the info sidebar keys its placeholder on there being no description; a blank
    // string would persist an empty description block instead.
    const desc = description.trim();
    const body = desc ? { name: name.trim(), description: desc } : { name: name.trim() };
    try { const d = await create.mutateAsync(body); setOpen(false); setName(""); setDescription(""); nav(`/explore/${ws}/dashboards/${d.id}`); }
    catch (e) { toast.error(String(e)); }
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
    if (moved > 0) toast.success(`Moved ${moved} ${moved === 1 ? "dashboard" : "dashboards"}`);
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

  const hasData = !!data && data.length > 0;

  const renderRow = (d: DashboardSummary) => (
    <tr key={d.id} className="border-t hover:bg-accent"
      draggable={!showTrash && canManage} onDragStart={!showTrash && canManage ? (e) => e.dataTransfer.setData("text/plain", d.id) : undefined}>
      {bulk.bulkMode && !showTrash && <td className="py-1"><Checkbox checked={bulk.selected.has(d.id)} onCheckedChange={() => bulk.toggle(d.id)} aria-label={`Select ${d.name}`} /></td>}
      <td className="py-1 pl-6">
        <span className="inline-flex items-center gap-1">
          {!showTrash && (
            <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${d.bookmarked ? "unbookmark" : "bookmark"} ${d.name}`}
              onClick={() => guard((d.bookmarked ? removeBookmark : addBookmark).mutateAsync(d.id))}>
              <Star className={`h-4 w-4 ${d.bookmarked ? "fill-highlight text-highlight" : "text-muted-foreground"}`} />
            </Button>
          )}
          <EntityLink to={`/explore/${ws}/dashboards/${d.id}`}>{d.name}</EntityLink>
          {/* Same mark the datasets list uses (Datasets.tsx), so "trusted" reads identically
              wherever content is listed rather than being a per-page invention. */}
          {d.certified && <BadgeCheck className="h-3.5 w-3.5 text-primary" aria-label="Certified" />}
        </span>
      </td>
      <td className="text-muted-foreground tabular-nums">{d.card_count}</td>
      <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
      <td>
        <DashboardActionsMenu
          ws={ws} dash={d} trashed={showTrash} folders={folders} canManage={canManage}
          onDuplicate={(x) => guard(duplicate.mutateAsync(x.id).then((c) => nav(`/explore/${ws}/dashboards/${c.id}`)))}
          onMove={(x, fid) => setFolder.mutateAsync({ id: x.id, folder_id: fid }).catch((e) => toast.error(String(e)))}
          onNewFolder={(x) => setFolderDialog({ mode: "move", dash: x })}
          onTrash={setPendingTrash}
          onRestore={(x) => guard(restore.mutateAsync(x.id))}
          onDeletePermanent={setPendingDelete}
          // Sharing hands out grants, so it belongs with the manage actions, not with Open.
          onShare={canManage ? setShareTarget : undefined}
        />
      </td>
    </tr>
  );

  return (
    <ListPageShell
      title={showTrash ? "Trash" : "Dashboards"}
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
          {!showTrash && hasData && canManage && (
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
            onClick={() => { const next = !showTrash; setShowTrash(next); if (next) { setOnlyBookmarked(false); bulk.setBulkMode(false); } }}
          />
          {!showTrash && canManage && (
            <ListToolbarButton
              icon={FolderPlus}
              label="New folder"
              onClick={() => setFolderDialog({ mode: "new" })}
            />
          )}
        </>
      }
      action={
        <Dialog open={open && canManage} onOpenChange={setOpen}>
          {/* Nothing is created in the Trash (KI-074). */}
          {canManage && !showTrash && <DialogTrigger asChild><Button>+ Dashboard</Button></DialogTrigger>}
          <DialogContent>
            <DialogHeader><DialogTitle>New dashboard</DialogTitle></DialogHeader>
            <div className="flex flex-col gap-3 py-2">
              <Input autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") onCreate(); }} />
              <label className="flex flex-col gap-1 text-xs text-muted-foreground">
                Description
                <textarea
                  aria-label="Description"
                  placeholder="It's optional but oh, so helpful"
                  className="h-20 w-full resize-none rounded-md border bg-background px-3 py-2 text-sm text-foreground"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                />
              </label>
              <div className="flex justify-end">
                <Button onClick={onCreate} disabled={create.isPending}>Create</Button>
              </div>
            </div>
          </DialogContent>
        </Dialog>
      }
    >
      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load dashboards."
        hasData={hasData}
        skeletonCols={4}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              title="No dashboards yet."
              description="Compose charts into a shareable dashboard."
              action={<Button onClick={() => setOpen(true)}>Create your first dashboard</Button>}
            />
          )
        }
      >
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search dashboards…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {bulk.bulkMode && bulk.someSelected && !showTrash && canManage && (
              <>
                <BulkMoveToFolderMenu
                  count={bulk.selectedIds.length}
                  folders={folders}
                  onMove={doBulkMoveTo}
                  onNewFolder={() => setFolderDialog({ mode: "bulkmove" })}
                />
                <Button variant="destructive" size="sm" onClick={() => setPendingTrash("bulk")}>
                  Move {bulk.selectedIds.length} to Trash
                </Button>
              </>
            )}
          </div>

          <div className="mb-3">
            <SummaryStrip
              items={[
                { label: "dashboards", singular: "dashboard", value: rows.length },
                // The Trash is a flat list with no folders on show, so it counts none (KI-074).
                ...(folders.length && !showTrash ? [{ label: "folders", singular: "folder", value: folders.length }] : []),
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
                  <SortableHeader sortKey="card_count" label="Cards" current={sort} onSort={onSort} />
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
                        // Folder share/rename/delete and the drop target are all workspace
                        // mutations, so they travel with the same gate as the row menu.
                        onShare={group.folder && canManage ? () => setFolderShare({ id: group.folder!.id, name: group.folder!.name }) : undefined}
                        onRename={group.folder && canManage ? () => setFolderDialog({ mode: "rename", folder: group.folder! }) : undefined}
                        onDelete={group.folder && canManage ? () => setPendingFolderDelete(group.folder!) : undefined}
                        onDropItem={canManage ? (id) => setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }).catch((e) => toast.error(String(e))) : undefined}
                      />
                      {!isCollapsed && group.rows.map(renderRow)}
                    </tbody>
                  );
                })
              )}
            </table>
            </ScrollX>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked dashboards." : "No dashboards match the current filters."}</p>
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
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.dash!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(String(e)));
          else if (d.mode === "bulkmove") doBulkMoveToNew(name);
          else createFolder.mutateAsync(name).then(() => toast.success("Folder created")).catch((e) => toast.error(String(e)));
        }}
      />

      <ConfirmTrashDialog
        pending={pendingTrash === "bulk" ? { count: bulk.selectedIds.length } : pendingTrash}
        noun={DASHBOARD_NOUN}
        onCancel={() => setPendingTrash(null)}
        onConfirm={() => {
          const target = pendingTrash;
          setPendingTrash(null);
          if (target === "bulk") void doBulkTrash();
          else if (target) guard(trash.mutateAsync(target.id).then(() => toast.success("Moved to Trash")));
        }}
      />

      <ConfirmFolderDeleteDialog
        folder={pendingFolderDelete}
        itemCount={(data ?? []).filter((d) => d.folder_id === pendingFolderDelete?.id).length}
        noun={DASHBOARD_NOUN}
        onCancel={() => setPendingFolderDelete(null)}
        onConfirm={() => {
          const target = pendingFolderDelete;
          setPendingFolderDelete(null);
          if (target) removeFolder.mutateAsync(target.id).then(() => toast.success("Folder deleted")).catch((e) => toast.error(String(e)));
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
    </ListPageShell>
  );
}
