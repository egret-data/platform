import { useMemo, useState } from "react";
import { FolderPlus, ListChecks, Loader2, Search, Star, Trash2 } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";

import { type AppSummary, useAppCapabilities, useAppMutations, useApps, useCreateApp } from "@/api/apps";
import { ApiError } from "@/api/client";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { AppActionsMenu } from "@/components/apps/AppActionsMenu";
import { RenameAppDialog } from "@/components/apps/RenameAppDialog";
import { BulkMoveToFolderMenu, moveEach } from "@/components/explore/BulkMoveToFolderMenu";
import { EmptyState } from "@/components/explore/EmptyState";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { folderCollapseKey, groupByFolder, showFolderSections, useCollapsed, UNCATEGORIZED_KEY } from "@/components/explore/folders";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ShareDialog } from "@/components/sharing/ShareDialog";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useBulkSelect } from "@/hooks/useBulkSelect";

type SortKey = "name" | "status" | "updated_at";

/** The server's own words (`ApiError.message` is the body's `error` field), without the
 *  `Error: ` prefix `String(e)` adds. Duplicate in particular can be refused for a row whose
 *  `can_write` is true — see `onDuplicate` below — and the reason is the only explanation. */
const errText = (e: unknown) => String((e as Error)?.message ?? e);

function NewAppDialog({
  ws,
  open,
  onOpenChange,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const create = useCreateApp(ws);
  const [name, setName] = useState("");

  function close(next: boolean) {
    if (!next) setName("");
    onOpenChange(next);
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    try {
      const created = await create.mutateAsync({ name: name.trim() });
      close(false);
      navigate(`/explore/${ws}/apps/${created.id}/edit`);
    } catch (err) {
      toast.error(String((err as Error)?.message ?? err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent>
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Create app</DialogTitle>
            <DialogDescription>
              A data app is a marimo notebook that runs SQL as whoever is looking at it. You edit a draft;
              publishing it makes it the version people open.
            </DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-1.5">
            <Label htmlFor="app-name">Name</Label>
            <Input
              id="app-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Revenue explorer"
              autoFocus
            />
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || name.trim() === ""}>
              {create.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function StatusBadge({ app }: { app: AppSummary }) {
  return app.latest_version_no == null ? (
    <Badge variant="outline">Draft only</Badge>
  ) : (
    <Badge variant="secondary">Published v{app.latest_version_no}</Badge>
  );
}

/**
 * The name cell's three targets (spec §4.1): the published app when there is one; else the
 * editor, for someone who may edit; else plain text. A reader of a never-published app has
 * nowhere to go — the viewer has no version to show and the draft is not theirs to read — so a
 * link there would only lead to a refusal.
 */
function AppName({ ws, app }: { ws: string; app: AppSummary }) {
  if (app.latest_version_no != null) {
    return <Link className="font-medium hover:underline" to={`/explore/${ws}/apps/${app.id}`}>{app.name}</Link>;
  }
  if (app.can_write) {
    return <Link className="font-medium hover:underline" to={`/explore/${ws}/apps/${app.id}/edit`}>{app.name}</Link>;
  }
  return <span className="font-medium">{app.name}</span>;
}

export default function Apps() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading, isError, error } = useApps(ws, { trashed: showTrash });
  const capabilities = useAppCapabilities(ws);
  const { trash, restore, remove, duplicate, addBookmark, removeBookmark } = useAppMutations(ws);
  const { data: folders = [] } = useFolders(ws, "apps");
  const setFolder = useSetFolder(ws, "apps");
  const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "apps");
  const collapse = useCollapsed(folderCollapseKey(ws, "apps"));
  const [creating, setCreating] = useState(false);
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<AppSummary | null>(null);
  const [shareTarget, setShareTarget] = useState<AppSummary | null>(null);
  const [renameTarget, setRenameTarget] = useState<AppSummary | null>(null);
  const [folderShare, setFolderShare] = useState<{ id: string; name: string } | null>(null);
  const [folderDialog, setFolderDialog] = useState<{ mode: "new" | "rename" | "move" | "bulkmove"; app?: AppSummary; folder?: { id: string; name: string } } | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((a) => {
      if (onlyBookmarked && !a.bookmarked) return false;
      if (!q) return true;
      return a.name.toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      // Draft-only sorts below every published version.
      else if (sort.key === "status") cmp = (a.latest_version_no ?? 0) - (b.latest_version_no ?? 0);
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, onlyBookmarked, query, sort]);

  /**
   * Whether to offer the WORKSPACE-level affordances — `+ App`, New folder, and a folder
   * section's rename / delete / share / drop target. Withheld rather than disabled, for the
   * reason Dashboards gives: a greyed control advertises a capability the reader does not have.
   *
   * Everything about a single app is gated on that app's own `can_write` / `can_manage`
   * instead (the row menu, the checkbox, dragging, dropping it on a folder), because app
   * permissions are granted per app through shares and differ row to row. This flag is only for
   * what is not about one row, and the server answers it directly (`GET …/apps/capabilities`:
   * workspace-level `app:write`, the check `POST /apps` and the folder routes make).
   *
   * It used to be inferred from the rows — "can write some app here" — which was wrong for a
   * share-only collaborator (a writable row, and still no right to create) and had no answer at
   * all for an empty list.
   *
   * No answer is not a yes: while the request is loading, or if it failed, these controls stay
   * hidden. Showing a control the server then refuses is the defect this exists to remove.
   */
  const canCreate = capabilities.data?.can_create === true;

  const filtering = query.trim() !== "" || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  /**
   * Whether this table is drawn with folder SECTION HEADERS at all. The gate and its full
   * argument live in `showFolderSections` (`src/components/explore/folders.ts`).
   *
   * One flag, read by BOTH the render loop below and `visibleRows` — they are two derivations
   * of the same question, and gating only the render strands rows behind a collapse state
   * restored from localStorage after the last folder was deleted.
   */
  const showFolderHeaders = showFolderSections(folders, showTrash);

  /**
   * The rows actually ON SCREEN — not `rows`, which is only what survived filtering and
   * sorting: a collapsed folder's rows are in `groups` but not in front of the user.
   */
  const visibleRows = useMemo(
    () =>
      // Keyed off the same flag the table renders from, deliberately. Without a header there is
      // no collapse control, so nothing CAN be collapsed and `rows` is what is on screen — that
      // covers the trash (a flat table) and the folderless workspace alike.
      showFolderHeaders
        ? groups.flatMap((g) => (collapse.isCollapsed(g.folder?.id ?? UNCATEGORIZED_KEY) ? [] : g.rows))
        : rows,
    [showFolderHeaders, rows, groups, collapse],
  );

  /**
   * What `useBulkSelect` is fed: the rows on screen that the caller may WRITE. The hook's own
   * doc (`src/hooks/useBulkSelect.ts`) is why it must be no more than what is rendered; the
   * `can_write` filter is the same argument one step further. Only these rows draw a checkbox,
   * so only these may be reachable by "Select all" or counted in "Move n to Trash" — a
   * read-only row swept into the selection would be counted next to an action the server
   * refuses for it.
   */
  const selectableRows = useMemo(() => visibleRows.filter((a) => a.can_write), [visibleRows]);

  // One list behind both the count and the trash: `bulk.selectedIds` is derived from exactly
  // what was passed in here, so "Move n to Trash" cannot name one number next to an action
  // that reaches a different set.
  const bulk = useBulkSelect(selectableRows);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(errText(e)));

  /**
   * `can_write` on the row is necessary for Duplicate but not sufficient: the server also wants
   * workspace-level write, which a share-only collaborator lacks and which no field in the list
   * reports. So the item is offered on `can_write` and a refusal arrives here. The server's
   * whole answer to that refusal is the word "forbidden", so a 403 is spelled out; anything else
   * keeps the server's words. Navigation is chained on success only — a refused duplicate stays
   * on the list.
   */
  const onDuplicate = (a: AppSummary) =>
    duplicate
      .mutateAsync(a.id)
      .then((c) => nav(`/explore/${ws}/apps/${c.id}/edit`))
      .catch((e) =>
        toast.error(
          e instanceof ApiError && e.status === 403
            ? "You can edit this app, but making a copy needs permission to create apps in this workspace."
            : errText(e),
        ),
      );

  async function doBulkTrash() {
    let moved = 0;
    for (const id of bulk.selectedIds) {
      try { await trash.mutateAsync(id); moved += 1; } catch (e) { toast.error(errText(e)); }
    }
    // Not unconditional: with every call refused, "Moved to Trash" would sit beside the errors
    // and contradict them.
    if (moved > 0) toast.success("Moved to Trash");
    bulk.clear();
  }
  /**
   * Move the selected rows to a folder that already exists (`null` = out of any folder). Each
   * row is its own request and its own verdict, like the bulk trash: one refusal is reported and
   * the rest still move, and the success toast counts only what moved.
   */
  async function doBulkMoveTo(folderId: string | null) {
    const moved = await moveEach(
      bulk.selectedIds,
      (id) => setFolder.mutateAsync({ id, folder_id: folderId }),
      (e) => toast.error(errText(e)),
    );
    if (moved > 0) toast.success(`Moved ${moved} ${moved === 1 ? "app" : "apps"}`);
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
    } catch (e) { toast.error(errText(e)); return; }
    await doBulkMoveTo(folderId);
  }

  const hasData = !!data && data.length > 0;

  const renderRow = (a: AppSummary) => {
    // Per row, like everything else about one app: moving it to a folder is a write on it.
    const draggable = !showTrash && a.can_write;
    return (
      <tr key={a.id} className="border-t hover:bg-accent"
        draggable={draggable} onDragStart={draggable ? (e) => e.dataTransfer.setData("text/plain", a.id) : undefined}>
        {/* The cell is always there in bulk mode so the columns line up; the checkbox is not. */}
        {bulk.bulkMode && !showTrash && (
          <td className="py-1">
            {a.can_write && <Checkbox checked={bulk.selected.has(a.id)} onCheckedChange={() => bulk.toggle(a.id)} aria-label={`Select ${a.name}`} />}
          </td>
        )}
        <td className="py-1 pl-6">
          <span className="inline-flex items-center gap-1">
            {/* Not gated on a capability: bookmarking is the caller's own, and needs only read. */}
            {!showTrash && (
              <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${a.bookmarked ? "unbookmark" : "bookmark"} ${a.name}`}
                onClick={() => guard((a.bookmarked ? removeBookmark : addBookmark).mutateAsync(a.id))}>
                <Star className={`h-4 w-4 ${a.bookmarked ? "fill-highlight text-highlight" : "text-muted-foreground"}`} />
              </Button>
            )}
            <AppName ws={ws} app={a} />
          </span>
          {a.description && (
            // Indented past the star (w-7 + gap-1) so it sits under the name, not under the star.
            <div className={`text-xs text-muted-foreground ${showTrash ? "" : "pl-8"}`}>{a.description}</div>
          )}
        </td>
        <td><StatusBadge app={a} /></td>
        <td className="text-muted-foreground"><RelativeTime iso={a.updated_at} /></td>
        <td>
          <AppActionsMenu
            ws={ws} app={a} trashed={showTrash} folders={folders}
            onRename={setRenameTarget}
            onDuplicate={onDuplicate}
            onMove={(x, fid) => setFolder.mutateAsync({ id: x.id, folder_id: fid }).catch((e) => toast.error(errText(e)))}
            // Creating a folder is the workspace's permission, not this row's.
            onNewFolder={canCreate ? (x) => setFolderDialog({ mode: "move", app: x }) : undefined}
            onShare={setShareTarget}
            onTrash={(x) => guard(trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")))}
            onRestore={(x) => guard(restore.mutateAsync(x.id))}
            onDeletePermanent={setPendingDelete}
          />
        </td>
      </tr>
    );
  };

  return (
    <ListPageShell
      title={showTrash ? "Trash" : "Apps"}
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
          {/* Offered when a row on screen can be selected at all. `|| bulk.bulkMode` keeps the
              way OUT: collapsing the last folder with a writable row, or a search that matches
              none, would otherwise remove the only control that leaves bulk mode. */}
          {!showTrash && (selectableRows.length > 0 || bulk.bulkMode) && (
            <ListToolbarButton
              icon={ListChecks}
              label="Bulk select"
              active={bulk.bulkMode}
              onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
            />
          )}
          {/* NOT gated on `hasData`: this is a view switch, not a filter. It selects a
              different server query, and an empty app list says nothing about whether the
              trash is empty. Gated, it would be the only way OUT of an empty trash and would
              vanish exactly when it is the only control left. */}
          <ListToolbarButton
            icon={Trash2}
            label="Trash"
            active={showTrash}
            filled
            onClick={() => { const next = !showTrash; setShowTrash(next); if (next) { setOnlyBookmarked(false); bulk.setBulkMode(false); } }}
          />
          {!showTrash && canCreate && (
            <ListToolbarButton
              icon={FolderPlus}
              label="New folder"
              onClick={() => setFolderDialog({ mode: "new" })}
            />
          )}
        </>
      }
      action={canCreate ? <Button onClick={() => setCreating(true)}>+ App</Button> : undefined}
    >
      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load apps."
        hasData={hasData}
        skeletonCols={4}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              title="No apps yet."
              description={
                canCreate
                  ? "Create one to build a dashboard in code that queries your data as each viewer."
                  : "Apps shared with you will appear here."
              }
              action={canCreate ? <Button onClick={() => setCreating(true)}>Create your first app</Button> : undefined}
            />
          )
        }
      >
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search apps…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {bulk.bulkMode && bulk.someSelected && !showTrash && (
              <>
                {/* Moving is each row's own `can_write`, but MAKING a folder is the workspace's
                    permission, so "New folder…" is the only item that needs `canCreate`. */}
                <BulkMoveToFolderMenu
                  count={bulk.selectedIds.length}
                  folders={folders}
                  onMove={doBulkMoveTo}
                  onNewFolder={canCreate ? () => setFolderDialog({ mode: "bulkmove" }) : undefined}
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
                { label: "apps", singular: "app", value: rows.length },
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
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="status" label="Status" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              {/* Flat wherever there is nothing to head — see `showFolderHeaders`: the trash
                  always, and a workspace that has no folders at all. */}
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
                        // Folder share/rename/delete are workspace mutations, so they travel with
                        // `canCreate`. The drop target does not: dropping moves ONE app, which
                        // is that app's own `can_write` — already what makes its row draggable.
                        onShare={group.folder && canCreate ? () => setFolderShare({ id: group.folder!.id, name: group.folder!.name }) : undefined}
                        onRename={group.folder && canCreate ? () => setFolderDialog({ mode: "rename", folder: group.folder! }) : undefined}
                        onDelete={group.folder && canCreate ? () => removeFolder.mutateAsync(group.folder!.id).then(() => toast.success("Folder deleted")).catch((e) => toast.error(errText(e))) : undefined}
                        onDropItem={(id) => setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }).catch((e) => toast.error(errText(e)))}
                      />
                      {!isCollapsed && group.rows.map(renderRow)}
                    </tbody>
                  );
                })
              )}
            </table>
            </div>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked apps." : "No apps match the current filters."}</p>
          )}
      </ListStates>

      <NewAppDialog ws={ws} open={creating} onOpenChange={setCreating} />

      {renameTarget && (
        <RenameAppDialog
          ws={ws}
          app={renameTarget}
          open={renameTarget !== null}
          onOpenChange={(o) => { if (!o) setRenameTarget(null); }}
        />
      )}

      <FolderNameDialog
        open={!!folderDialog}
        title={folderDialog?.mode === "rename" ? "Rename folder" : folderDialog?.mode === "move" ? "Move to new folder" : folderDialog?.mode === "bulkmove" ? "Move to new folder" : "New folder"}
        submitLabel={folderDialog?.mode === "rename" ? "Rename" : folderDialog?.mode === "move" || folderDialog?.mode === "bulkmove" ? "Move" : "Create"}
        initialValue={folderDialog?.mode === "rename" ? folderDialog.folder!.name : ""}
        onOpenChange={(o) => { if (!o) setFolderDialog(null); }}
        onSubmit={(name) => {
          const d = folderDialog!;
          if (d.mode === "rename") renameFolder.mutateAsync({ id: d.folder!.id, name }).catch((e) => toast.error(errText(e)));
          else if (d.mode === "move") createFolder.mutateAsync(name).then((f) => setFolder.mutateAsync({ id: d.app!.id, folder_id: f.id })).then(() => toast.success("Moved")).catch((e) => toast.error(errText(e)));
          else if (d.mode === "bulkmove") doBulkMoveToNew(name);
          else createFolder.mutateAsync(name).then(() => toast.success("Folder created")).catch((e) => toast.error(errText(e)));
        }}
      />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? Its draft and every published version go with it. This cannot be undone.`}
        onConfirm={async () => { if (!pendingDelete) return; await guard(remove.mutateAsync(pendingDelete.id)); setPendingDelete(null); }}
      />

      {shareTarget && (
        <ShareDialog
          ws={ws}
          kind="app"
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
