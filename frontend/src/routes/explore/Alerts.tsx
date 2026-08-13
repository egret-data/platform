import { useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { Bell, FolderPlus, ListChecks, Search, Star, Trash2 } from "lucide-react";
import { toast } from "sonner";
import {
  useScheduleMutations,
  useSchedules,
  type ScheduleSummary,
  type ScheduleType,
} from "@/api/alerts";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Checkbox } from "@/components/ui/checkbox";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/components/ui/tooltip";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { ScheduleActionsMenu } from "@/components/alerts/ScheduleActionsMenu";
import { RunLogDrawer } from "@/components/alerts/RunLogDrawer";
import { humanizeCrontab } from "@/lib/humanizeCron";
import type { RunState } from "@/api/alerts";

/**
 * How each run state reads in the list.
 *
 * Superset's five states are kept verbatim because they are the vocabulary that
 * makes the page legible — `noop` and `grace` are the two distinct answers to
 * "why didn't I get an email?" ("it ran, the condition was false" vs "it would
 * have fired, the grace period suppressed it"), and showing either as a plain
 * success would lose the answer the page exists to give.
 */
const STATE_LABEL: Record<RunState, string> = {
  success: "Success",
  working: "Working",
  error: "Error",
  noop: "Not triggered",
  grace: "On grace",
};

const STATE_VARIANT: Record<RunState, "default" | "secondary" | "destructive" | "outline"> = {
  success: "default",
  working: "secondary",
  error: "destructive",
  noop: "outline",
  grace: "secondary",
};

/** Sentinel for the "has never run" facet option — `last_state` is null there,
 *  and Radix Select values must be non-empty strings. */
const STATE_NEVER = "__never__";

function StateBadge({ state }: { state: RunState | null | undefined }) {
  if (!state) return <span className="text-sm text-muted-foreground">Never run</span>;
  return <Badge variant={STATE_VARIANT[state]}>{STATE_LABEL[state]}</Badge>;
}

type SortKey = "name" | "last_state" | "next_run_at";

export default function Alerts() {
  const ws = useParams().workspace!;
  const nav = useNavigate();

  // The tab lives in the URL so a report list can be linked, bookmarked and
  // reached with the back button — `?type=report`.
  const [params, setParams] = useSearchParams();
  const tab: ScheduleType =
    params.get("type") === "report" || params.get("new") === "report" ? "report" : "alert";
  const setTab = (t: ScheduleType) =>
    setParams(
      (p) => {
        if (t === "alert") p.delete("type");
        else p.set("type", t);
        return p;
      },
      { replace: true },
    );

  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading, isError, error } = useSchedules(ws, tab, showTrash);
  const {
    remove,
    runNow,
    setActive,
    trash,
    restore,
    addBookmark,
    removeBookmark,
  } = useScheduleMutations(ws);

  const { data: folders = [] } = useFolders(ws, "alerts");
  const setFolder = useSetFolder(ws, "alerts");
  const {
    create: createFolder,
    rename: renameFolder,
    remove: removeFolder,
  } = useFolderMutations(ws, "alerts");
  const collapse = useCollapsed(`folders:${ws}:alerts`);

  const [runLogFor, setRunLogFor] = useState<ScheduleSummary | null>(null);
  const [pendingDelete, setPendingDelete] = useState<ScheduleSummary | null>(null);
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [stateFilter, setStateFilter] = useState(FACET_ALL);
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "name", dir: "asc" });
  const [folderDialog, setFolderDialog] = useState<
    | { mode: "new" | "rename" | "move" | "bulkmove"; row?: ScheduleSummary; folder?: { id: string; name: string } }
    | null
  >(null);

  const all = useMemo(() => data ?? [], [data]);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = all.filter((s) => {
      if (onlyBookmarked && !s.bookmarked) return false;
      if (stateFilter === STATE_NEVER && s.last_state) return false;
      if (stateFilter !== FACET_ALL && stateFilter !== STATE_NEVER && s.last_state !== stateFilter)
        return false;
      if (!q) return true;
      return [s.name, s.description, s.owners.map((o) => o.email).join(" ")]
        .join(" ")
        .toLowerCase()
        .includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "last_state")
        cmp = (a.last_state ?? "").localeCompare(b.last_state ?? "");
      // Paused rows have no next run; sorting them last keeps "what fires
      // soonest?" at the top in ascending order.
      else cmp = (a.next_run_at ?? "￿").localeCompare(b.next_run_at ?? "￿");
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [all, onlyBookmarked, query, stateFilter, sort]);

  const bulk = useBulkSelect(rows);
  const filtering = query.trim() !== "" || stateFilter !== FACET_ALL || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  const counts = useMemo(() => {
    const active = all.filter((s) => s.active).length;
    return {
      total: all.length,
      active,
      paused: all.length - active,
      failing: all.filter((s) => s.last_state === "error").length,
    };
  }, [all]);

  /**
   * Whether to offer the mutating affordances at all. Withheld rather than
   * disabled: a greyed Delete advertises a capability the reader does not have,
   * and these controls are destructive.
   *
   * It cannot be derived from the session — `me.permissions` carries PLATFORM
   * grants only, so workspace-scoped `alert:manage` never appears there. The
   * server answers on every row (the answer is workspace-wide, so row 0 is not
   * a shortcut). An empty list has no row to carry it; defaulting to `true`
   * there means a legitimate author in a fresh workspace can still press New
   * and, if wrong, gets a 403 — blocking real work would be the worse error.
   */
  const canManage = all.length > 0 ? all[0].can_manage !== false : true;
  const typeLabel = tab === "alert" ? "alert" : "report";
  const hasData = all.length > 0;

  const presentStates = useMemo(() => {
    const opts: { value: string; label: string }[] = (Object.keys(STATE_LABEL) as RunState[])
      .filter((s) => all.some((r) => r.last_state === s))
      .map((s) => ({ value: s, label: STATE_LABEL[s] }));
    if (all.some((r) => !r.last_state)) opts.push({ value: STATE_NEVER, label: "Never run" });
    return opts;
  }, [all]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  async function doBulkTrash() {
    for (const id of bulk.selectedIds) {
      try {
        await trash.mutateAsync(id);
      } catch (e) {
        toast.error(String(e));
      }
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
    } catch (e) {
      toast.error(String(e));
    }
    bulk.clear();
  }

  const newHref = `/explore/${ws}/alerts/new${tab === "report" ? "?type=report" : ""}`;
  const newButton = canManage ? (
    <Button asChild>
      <Link to={newHref}>+ {typeLabel === "alert" ? "Alert" : "Report"}</Link>
    </Button>
  ) : null;

  const renderRow = (s: ScheduleSummary) => (
    <tr
      key={s.id}
      className="border-t hover:bg-accent"
      draggable={!showTrash}
      onDragStart={!showTrash ? (e) => e.dataTransfer.setData("text/plain", s.id) : undefined}
    >
      {bulk.bulkMode && !showTrash && (
        <td className="py-1">
          <Checkbox
            checked={bulk.selected.has(s.id)}
            onCheckedChange={() => bulk.toggle(s.id)}
            aria-label={`Select ${s.name}`}
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
              aria-label={`${s.bookmarked ? "unbookmark" : "bookmark"} ${s.name}`}
              onClick={() =>
                guard((s.bookmarked ? removeBookmark : addBookmark).mutateAsync(s.id))
              }
            >
              <Star
                className={`h-4 w-4 ${s.bookmarked ? "fill-highlight text-highlight" : "text-muted-foreground"}`}
              />
            </Button>
          )}
          <span className="min-w-0">
            {/* The name opens the editor, the way every sibling list opens its
                object. In the trash there is nothing to edit, so it stays text. */}
            {showTrash ? (
              <span className="font-medium">{s.name}</span>
            ) : (
              <Link
                className="font-medium hover:underline"
                to={`/explore/${ws}/alerts/${s.id}`}
              >
                {s.name}
              </Link>
            )}
            {s.description && (
              <span className="block truncate text-xs text-muted-foreground">{s.description}</span>
            )}
          </span>
        </span>
      </td>
      <td>
        {canManage && !showTrash ? (
          <Switch
            checked={s.active}
            disabled={setActive.isPending}
            aria-label={`${s.active ? "Pause" : "Activate"} ${s.name}`}
            onCheckedChange={(active) =>
              setActive.mutateAsync({ id: s.id, active }).catch((e) => toast.error(String(e)))
            }
          />
        ) : s.active ? (
          <span className="text-xs text-muted-foreground">Active</span>
        ) : (
          <Badge variant="outline">Paused</Badge>
        )}
      </td>
      <td>
        {/* The badge opens the run log: "why is this red?" is the question it
            provokes, so it should be the thing you click. */}
        <button
          type="button"
          className="rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={() => setRunLogFor(s)}
          aria-label={`Run log for ${s.name}`}
        >
          <StateBadge state={s.last_state} />
          {s.last_eval_at && (
            <RelativeTime iso={s.last_eval_at} className="block text-xs text-muted-foreground" />
          )}
        </button>
      </td>
      <td className="text-muted-foreground">
        <span title={s.crontab}>{humanizeCrontab(s.crontab)}</span>
        <span className="block text-xs">{s.timezone}</span>
      </td>
      <td className="text-muted-foreground">
        {s.next_run_at ? <RelativeTime iso={s.next_run_at} /> : "—"}
      </td>
      <td className="text-muted-foreground">{s.owners.map((o) => o.email).join(", ")}</td>
      <td>
        <ScheduleActionsMenu
          schedule={s}
          canManage={canManage}
          trashed={showTrash}
          running={runNow.isPending}
          folders={folders}
          onRunLog={setRunLogFor}
          onEdit={(x) => nav(`/explore/${ws}/alerts/${x.id}`)}
          onTrash={(x) =>
            trash
              .mutateAsync(x.id)
              .then(() => toast.success("Moved to Trash"))
              .catch((e) => toast.error(String(e)))
          }
          onRestore={(x) => guard(restore.mutateAsync(x.id))}
          onDeletePermanent={setPendingDelete}
          onMove={(x, fid) => guard(setFolder.mutateAsync({ id: x.id, folder_id: fid }))}
          onNewFolder={(x) => setFolderDialog({ mode: "move", row: x })}
          onRunNow={(x) =>
            runNow
              .mutateAsync(x.id)
              .then((run) => toast.success(`Ran now: ${STATE_LABEL[run.state]}`))
              .catch((e) => toast.error(String(e)))
          }
        />
      </td>
    </tr>
  );

  const colSpan = bulk.bulkMode && !showTrash ? 8 : 7;

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between gap-2">
        <h1 className="text-lg font-semibold">
          {showTrash ? "Trash" : "Alerts & Reports"}
        </h1>
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
          {!showTrash && hasData && canManage && (
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
          {!showTrash && canManage && (
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
          {!showTrash && newButton}
        </div>
      </div>

      <Tabs
        value={tab}
        onValueChange={(v) => {
          setTab(v as ScheduleType);
          setStateFilter(FACET_ALL);
        }}
        className="mb-3"
      >
        <TabsList>
          <TabsTrigger value="alert">Alerts</TabsTrigger>
          <TabsTrigger value="report">Reports</TabsTrigger>
        </TabsList>
      </Tabs>

      {isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(error as Error)?.message ?? "Failed to load schedules."}
          </AlertDescription>
        </Alert>
      ) : isLoading ? (
        <ListSkeleton rows={6} cols={6} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="pl-8"
                placeholder={`Search ${typeLabel}s…`}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
            {presentStates.length > 0 && (
              <FacetSelect
                label="Last run"
                value={stateFilter}
                onChange={setStateFilter}
                options={presentStates}
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
                { label: `${typeLabel}s`, value: counts.total },
                // In the bin, "active" and "failing" describe a schedule the
                // user has discarded — true of the row, but not an answer to
                // anything they are asking here. Only the count survives.
                ...(showTrash
                  ? []
                  : [
                      { label: "active", value: counts.active },
                      { label: "paused", value: counts.paused },
                    ]),
                ...(folders.length && !showTrash
                  ? [{ label: "folders", value: folders.length }]
                  : []),
                // Clicking the failing count filters to it — the one number on
                // this page you act on the moment it is non-zero.
                ...(counts.failing && !showTrash
                  ? [
                      {
                        label: "failing",
                        value: counts.failing,
                        active: stateFilter === "error",
                        onClick: () => setStateFilter((f) => (f === "error" ? FACET_ALL : "error")),
                      },
                    ]
                  : []),
              ]}
            />
          </div>

          {rows.length > 0 ? (
            <div className="overflow-x-auto">
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
                    <th className="py-1 font-medium">Active</th>
                    <SortableHeader
                      sortKey="last_state"
                      label="Last run"
                      current={sort}
                      onSort={onSort}
                    />
                    <th className="py-1 font-medium">Schedule</th>
                    <SortableHeader
                      sortKey="next_run_at"
                      label="Next run"
                      current={sort}
                      onSort={onSort}
                    />
                    <th className="py-1 font-medium">Owners</th>
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
                          colSpan={colSpan}
                          collapsed={isCollapsed}
                          onToggle={() => collapse.toggle(key)}
                          onRename={
                            group.folder && canManage
                              ? () => setFolderDialog({ mode: "rename", folder: group.folder! })
                              : undefined
                          }
                          onDelete={
                            group.folder && canManage
                              ? () =>
                                  removeFolder
                                    .mutateAsync(group.folder!.id)
                                    .then(() => toast.success("Folder deleted"))
                                    .catch((e) => toast.error(String(e)))
                              : undefined
                          }
                          onDropItem={(id) =>
                            guard(setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }))
                          }
                        />
                        {!isCollapsed && group.rows.map(renderRow)}
                      </tbody>
                    );
                  })
                )}
              </table>
            </div>
          ) : (
            <p className="text-muted-foreground">
              {onlyBookmarked
                ? `No bookmarked ${typeLabel}s.`
                : `No ${typeLabel}s match the current filters.`}
            </p>
          )}
        </>
      ) : showTrash ? (
        <p className="text-muted-foreground">Trash is empty.</p>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <Bell className="mx-auto mb-2 h-8 w-8" />
          <p>
            No {typeLabel}s yet.{" "}
            {tab === "alert"
              ? "An alert runs SQL on a schedule and notifies you when the result meets your condition."
              : "A report delivers a chart or dashboard on a schedule."}
          </p>
          {canManage && (
            <Button asChild className="mt-3">
              <Link to={newHref}>Create your first {typeLabel}</Link>
            </Button>
          )}
        </div>
      )}

      <FolderNameDialog
        open={!!folderDialog}
        title={
          folderDialog?.mode === "rename"
            ? "Rename folder"
            : folderDialog?.mode === "move" || folderDialog?.mode === "bulkmove"
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
            guard(renameFolder.mutateAsync({ id: d.folder!.id, name }));
          else if (d.mode === "move")
            guard(
              createFolder
                .mutateAsync(name)
                .then((f) => setFolder.mutateAsync({ id: d.row!.id, folder_id: f.id }))
                .then(() => toast.success("Moved")),
            );
          else if (d.mode === "bulkmove") doBulkMove(name);
          else guard(createFolder.mutateAsync(name).then(() => toast.success("Folder created")));
        }}
      />

      {runLogFor && (
        <RunLogDrawer ws={ws} schedule={runLogFor} onClose={() => setRunLogFor(null)} />
      )}

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => !o && setPendingDelete(null)}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? Its run history and recipients go with it. This cannot be undone.`}
        onConfirm={async () => {
          if (pendingDelete) await guard(remove.mutateAsync(pendingDelete.id));
          setPendingDelete(null);
        }}
      />
    </div>
  );
}

/** Exported for the run-log drawer, which shows the same vocabulary. */
export { STATE_LABEL, STATE_VARIANT, StateBadge };
