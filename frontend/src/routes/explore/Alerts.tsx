import { useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { Bell, FolderPlus, ListChecks, Search, Star, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { BulkMoveToFolderMenu, moveEach } from "@/components/explore/BulkMoveToFolderMenu";
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
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { EmptyState } from "@/components/explore/EmptyState";
import { folderCollapseKey, groupByFolder, showFolderSections, useCollapsed, UNCATEGORIZED_KEY } from "@/components/explore/folders";
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
  const {
    data: primaryData,
    isLoading: primaryLoading,
    isError: primaryIsError,
    error: primaryError,
  } = useSchedules(ws, tab, showTrash);
  // A `dataapi_alert` rides on this same tab, badge-distinguished, per spec
  // §5.8 ("the same Alerts list ... with a badge distinguishing the type —
  // not a third tab"). The backend's `type` filter is exact-match
  // (`repo.rs`: `WHERE ... type = $2`), so there is no single request that
  // returns "alert or dataapi_alert" — fetched as its own call and merged
  // below, gated to the Alerts tab. Re-filtered to rows that actually claim
  // `type === "dataapi_alert"` rather than trusted blindly: the server
  // already guarantees that, but a defensive filter here is cheap and is
  // what keeps this merge inert on the Reports tab and in every existing
  // fixture that never sets this type.
  const {
    data: dataapiRaw,
    isLoading: dataapiLoading,
    isError: dataapiIsError,
    error: dataapiError,
  } = useSchedules(ws, "dataapi_alert", showTrash);
  const data = useMemo(() => {
    const base = primaryData ?? [];
    if (tab !== "alert") return base;
    return [...base, ...(dataapiRaw ?? []).filter((s) => s.type === "dataapi_alert")];
  }, [primaryData, dataapiRaw, tab]);
  const isLoading = tab === "alert" ? primaryLoading || dataapiLoading : primaryLoading;
  const isError = tab === "alert" ? primaryIsError || dataapiIsError : primaryIsError;
  const error = tab === "alert" ? (primaryError ?? dataapiError) : primaryError;
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
  const collapse = useCollapsed(folderCollapseKey(ws, "alerts"));

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
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
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

  const filtering = query.trim() !== "" || stateFilter !== FACET_ALL || onlyBookmarked;
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
    if (moved > 0) toast.success(`Moved ${moved} ${moved === 1 ? "item" : "items"}`);
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
    } catch (e) {
      toast.error(String(e));
      return;
    }
    await doBulkMoveTo(folderId);
  }

  const newHref = `/explore/${ws}/alerts/new${tab === "report" ? "?type=report" : ""}`;
  const newDataapiHref = `/explore/${ws}/alerts/new?type=dataapi_alert`;
  // "+ Alert" stays exactly the entry point it always was — same text, same
  // href, still a plain `<Link>` (§3.3: an entry point is a link, not an
  // imperative push, so it can be middle-clicked/opened in a new tab). A
  // dataapi_alert gets its OWN link beside it rather than folding both
  // choices into one control: the alternative (a dropdown trigger in place of
  // the link) would turn "+ Alert" into a button with no href of its own,
  // which is the one thing about it every existing list test pins.
  const newButton = !canManage ? null : (
    <div className="flex items-center gap-2">
      <Button asChild>
        <Link to={newHref}>+ {typeLabel === "alert" ? "Alert" : "Report"}</Link>
      </Button>
      {tab === "alert" && (
        <Button asChild variant="outline">
          <Link to={newDataapiHref}>+ Data API alert</Link>
        </Button>
      )}
    </div>
  );

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
            {/* The Alerts tab now mixes two schedule types (spec §5.8) that can
                share a name — each has its own name-uniqueness namespace
                server-side ("Name already used by a schedule of this type") —
                so this is the one thing on the row that tells them apart.
                `variant="outline"`, not `variant="success"`: that variant is a
                hardcoded `bg-green-600` reserved elsewhere in this app for an
                actual state, and a type is not a status. Those state badges
                (Endpoints/Versions/Clients' "Live"/"Active") no longer read
                `bg-success/15 text-success`, as this comment used to say —
                that pair measures 3.72:1 against WCAG 1.4.3's 4.5:1, and they
                now share `dataapi/chipTone.ts`, which keeps the OKLCH tint and
                puts the ink in `--foreground`. The point this comment is making
                survives the change: they wear a STATE tone from the token set,
                and `variant="success"`'s hardcoded green is not one. */}
            {s.type === "dataapi_alert" && (
              <Badge variant="outline" className="ml-1.5 align-middle">
                Data API
              </Badge>
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
    <ListPageShell
      title={showTrash ? "Trash" : "Alerts & Reports"}
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
          {/* NOT gated on `hasData`: a view switch, not a filter. Gated, it
              would be the only way OUT of an empty trash and would vanish
              exactly when it is the only control left. */}
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
          {!showTrash && canManage && (
            <ListToolbarButton
              icon={FolderPlus}
              label="New folder"
              onClick={() => setFolderDialog({ mode: "new" })}
            />
          )}
        </>
      }
      action={!showTrash ? newButton : null}
    >
      {/* A page-level view switch, not a shell slot: it selects which kind of
          schedule the list shows and belongs above the table, where the page
          has always drawn it. */}
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

      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load schedules."
        hasData={hasData}
        skeletonCols={6}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              icon={Bell}
              title={`No ${typeLabel}s yet.`}
              description={
                tab === "alert"
                  ? "An alert runs SQL on a schedule and notifies you when the result meets your condition."
                  : "A report delivers a chart or dashboard on a schedule."
              }
              action={
                canManage ? (
                  <Button asChild>
                    <Link to={newHref}>Create your first {typeLabel}</Link>
                  </Button>
                ) : null
              }
            />
          )
        }
      >
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
                { label: `${typeLabel}s`, singular: typeLabel, value: counts.total },
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
                  ? [{ label: "folders", singular: "folder", value: folders.length }]
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
      </ListStates>

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
          else if (d.mode === "bulkmove") doBulkMoveToNew(name);
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
    </ListPageShell>
  );
}

/** Exported for the run-log drawer, which shows the same vocabulary. */
export { STATE_LABEL, STATE_VARIANT, StateBadge };
