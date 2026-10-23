import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search, Trash2, Webhook } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { FolderToolbar } from "@/components/explore/FolderToolbar";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { EmptyState } from "@/components/explore/EmptyState";
import { BulkToggle, BulkBar } from "@/components/explore/BulkBar";
import { BookmarkToggle, BookmarkStar } from "@/components/explore/BookmarkToggle";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { folderCollapseKey as sharedFolderCollapseKey, groupByFolder, showFolderSections, useCollapsed, UNCATEGORIZED_KEY } from "@/components/explore/folders";
import { useFoldersFoldersList } from "@/api/generated/folders/folders";
import { useFolderMutations } from "@/api/folders";
import { useDatabases } from "@/api/databases";
import { useDataApiListEndpoints } from "@/api/generated/data-api/data-api";
// Mutations come from the hand-written wrapper, never from the generated hooks:
// the wrapper is what owns the cache invalidation (see `src/api/dataApi.ts`).
// Wired to the generated hook directly, "Endpoint disabled" toasts over a row
// whose badge still reads Live for the next 30 seconds.
import { useBookmarkEndpoint, useDeleteEndpoint, useTrashEndpoint, useUnbookmarkEndpoint, useUpdateEndpoint } from "@/api/dataApi";
import type { DataApiEndpointDto, DataApiEndpointListItemDto } from "@/api/generated/model";
import { DownloadOpenApiDialog } from "./DownloadOpenApiDialog";
import { EndpointActionsMenu } from "./EndpointActionsMenu";
import { EndpointRowStateBadge, pendingRoute } from "./endpointState";
import { EntityLink } from "@/components/ui/entity-link";

// Folders share the generic router (egret-bi's `kind_perms`); this is the
// `{kind}` URL segment data-api registers there, matching
// `crates/egret-bi/src/routes.rs`'s `kind_perms("data_api_endpoints")`.
const FOLDER_KIND = "data_api_endpoints";

/**
 * Where `useCollapsed` persists which sections this page's visitor left shut.
 *
 * Exported, and a function rather than a literal, because the tests have to
 * SEED it: the "rows are not stranded behind a stale collapse state" guard
 * writes a collapsed Uncategorized into localStorage before mounting. Spelled
 * out a second time over there, a change to this format would land the seed on
 * a key nothing reads — and that test would keep passing, because a seed that
 * misses looks exactly like a page that correctly ignored it. Deriving it from
 * here is what keeps the guard able to fail.
 *
 * The FORMAT now lives in `showFolderSections`' module beside the collapse
 * hook that consumes it, because the same argument applies across the seven
 * grouped lists and not only across this page and its test. This stays as a
 * one-argument alias so `FOLDER_KIND` is still bound in exactly one place.
 */
export const folderCollapseKey = (ws: string) => sharedFolderCollapseKey(ws, FOLDER_KIND);


/**
 * `deleted_at` is a sort key and not merely a column because the Trash is sorted
 * by the timestamp it SHOWS. The two views render different last columns —
 * `updated_at` (Last edited) in the normal list, `deleted_at` (Trashed) in the
 * trash — and leaving the trash on `updated_at` produced a table whose visible
 * timestamps disagreed with its own row order: trash A (edited Aug 1) at 10:00,
 * then B (edited Jul 1) at 11:00, and A sorted above B while the column said
 * the opposite. An invisible sort key is unarguable-with, which is worse than a
 * wrong one you can click.
 */
type SortKey = "name" | "state" | "updated_at" | "deleted_at";

/** The default sort for each view — the column that view actually displays. */
const DEFAULT_SORT: Record<"list" | "trash", SortState<SortKey>> = {
  list: { key: "updated_at", dir: "desc" },
  trash: { key: "deleted_at", dir: "desc" },
};

const ENGINE_LABEL: Record<string, string> = { trino: "Trino", starrocks: "StarRocks" };

/**
 * The row badge, whole — word, tint and glyph.
 *
 * Shared with the editor header (`endpointState.tsx`), which is the point: the
 * list said "Live" on every row while the six tabs an author edits on said
 * nothing, and two surfaces describing one endpoint must not be free to
 * describe it differently. The reasoning for the `disabled` split — and for
 * why `live`/`draft` need none on a LIST row — moved there with it.
 *
 * The WORD was shared first and the visuals were not, so the fork simply moved:
 * this file kept a local `STATE_TONE` painting `live` as `bg-success/15
 * text-success`, which measures 3.72:1 against WCAG 1.4.3's 4.5:1, while the
 * header pill one click away had already been fixed to `--foreground` ink plus
 * a per-tier glyph. Both halves now come from the same module.
 *
 * The header's word is still longer, because the shell has already loaded the
 * version history and can name the served version and say whether the draft
 * has moved past it. A list row knows neither, says neither, and — see
 * `endpointStateOnlyTier` — cannot be made to guess.
 */

export default function Endpoints() {
  const ws = useParams().workspace!;

  /**
   * Which of the TWO lists this page is showing. Not a filter over one list:
   * `GET /endpoints` excludes trashed rows by default and returns only trashed
   * ones under `?trashed=true`, so the two views are different server queries
   * with different cache entries.
   */
  const [showTrash, setShowTrash] = useState(false);

  const { data: envelope, isLoading, isError, error } = useDataApiListEndpoints(ws, { trashed: showTrash });
  // useMemo (not a plain ternary) so a falsy envelope doesn't hand `rows`'
  // useMemo a fresh `[]` identity on every render.
  const endpoints = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const { data: foldersEnvelope } = useFoldersFoldersList(ws, FOLDER_KIND);
  const folders = useMemo(() => (foldersEnvelope?.status === 200 ? foldersEnvelope.data : []), [foldersEnvelope]);

  const { data: databases } = useDatabases(ws);

  const deleteMutation = useDeleteEndpoint();
  // The page's own trash mutation, used ONLY by the bulk action — a row's own
  // Move to Trash is fired by `EndpointActionsMenu`'s instance, so `isPending`
  // there stays scoped to the row that was clicked.
  const trashMutation = useTrashEndpoint();
  // Two mutations, one control: which of them a star fires is decided per row
  // from that row's own `bookmarked`, never from a page-level "mode".
  const bookmarkMutation = useBookmarkEndpoint();
  const unbookmarkMutation = useUnbookmarkEndpoint();
  const { create: createFolder } = useFolderMutations(ws, FOLDER_KIND);
  // The page's own copy of the endpoint PATCH, used ONLY by the folder dialog's
  // "move to a new folder" path — the folder has to exist before the row can be
  // put in it, and the create half lives here with the dialog. A row's direct
  // moves are fired by `EndpointActionsMenu`'s own instance, so `isPending`
  // there stays scoped to the row that was clicked.
  const updateMutation = useUpdateEndpoint();

  const collapse = useCollapsed(folderCollapseKey(ws));

  const [query, setQuery] = useState("");
  // ONE piece of state behind two controls: the SummaryStrip chip (a shortcut)
  // and the State facet (the discoverable control). Two filters that can
  // disagree is worse than one that is hard to find.
  const [stateFilter, setStateFilter] = useState<string | null>(null);
  const [engineFilter, setEngineFilter] = useState<string | null>(null);
  // The page owns the bookmarked-only filter, not `BookmarkToggle` — the page
  // is what applies it to the rows, so the state lives where it is used.
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  /**
   * Optimistic bookmark values, keyed by endpoint id, overriding the list's
   * `bookmarked` until the server's own answer catches up.
   *
   * Without it the star is neither optimistic nor confirmed: a click waits for
   * the POST, then for the invalidated list to refetch — two round trips — and
   * the glyph does not move in between. The visible slowness is the smaller
   * half. The larger half is that `e.bookmarked` is still the OLD value while
   * that runs, so an impatient second click computes the same direction again
   * and re-fires the mutation it just fired, instead of undoing it. The
   * override is what both the glyph and the next click read, so the second
   * click toggles back.
   *
   * Chosen over disabling the star while pending because `isPending` on these
   * shared hooks is per-HOOK, not per-row — using it would freeze all thirty
   * stars on the page while one is in flight — and because tracking pending
   * ids to avoid that costs exactly as much state as this map while leaving
   * the user with a control that ignores them rather than one that responds.
   *
   * An optimistic flip is only honest if it can be taken back, so a rejected
   * mutation drops the override and the glyph returns to what the server last
   * said. On SUCCESS the override stays: it already holds the value the
   * refetch is about to bring, and clearing it early would flash the star back
   * to its old state until the list lands.
   */
  const [bookmarkOverride, setBookmarkOverride] = useState<Record<string, boolean>>({});
  const [sort, setSort] = useState<SortState<SortKey>>(DEFAULT_SORT.list);
  const [downloadOpen, setDownloadOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<DataApiEndpointDto | null>(null);
  /**
   * The folder-name dialog, in one of two modes.
   *
   * `"new"` just creates a folder. `"move"` creates one AND puts `endpoint`
   * into it — the row that opened it is carried here rather than in a separate
   * piece of state, so the dialog cannot be open in "move" mode with nothing
   * to move.
   */
  const [folderDialog, setFolderDialog] = useState<
    { mode: "new" } | { mode: "move"; endpoint: DataApiEndpointDto } | null
  >(null);

  // `database_id` -> engine, so the Engine column and the Engine facet read the
  // same source rather than each doing its own lookup.
  const engineOf = useMemo(() => {
    const byId = new Map((databases ?? []).map((d) => [d.id, d.engine]));
    return (databaseId: string) => byId.get(databaseId);
  }, [databases]);

  /** This row's bookmark state as the USER last left it — override first, server otherwise. */
  const isBookmarked = useCallback(
    (e: DataApiEndpointListItemDto) => bookmarkOverride[e.id] ?? e.bookmarked,
    [bookmarkOverride],
  );

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = endpoints.filter((e) => {
      if (onlyBookmarked && !isBookmarked(e)) return false;
      if (stateFilter && e.state !== stateFilter) return false;
      if (engineFilter && engineOf(e.database_id) !== engineFilter) return false;
      if (!q) return true;
      return [e.name, e.path].join(" ").toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "state") cmp = a.state.localeCompare(b.state);
      // `?? ""` and not a non-null assertion: `deleted_at` is nullable on the
      // DTO, and the cell above already renders "—" for a trash row that
      // somehow arrived without one. Sorting must not throw where the table
      // renders.
      else if (sort.key === "deleted_at") cmp = (a.deleted_at ?? "").localeCompare(b.deleted_at ?? "");
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [endpoints, query, onlyBookmarked, isBookmarked, stateFilter, engineFilter, engineOf, sort]);

  const filtering = query.trim() !== "" || stateFilter !== null || engineFilter !== null || onlyBookmarked;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  /**
   * Whether the table is drawn with folder SECTION HEADERS at all — which is
   * not the same question as whether it is grouped. The full argument, and the
   * reasons it is read off `folders` rather than off `groups.length` and never
   * applied to the trash, now live on `showFolderSections`
   * (`src/components/explore/folders.ts`).
   *
   * This page and `sqllab/SavedTab.tsx` were the only two that had the gate;
   * `Charts.tsx`, `Datasets.tsx`, `Dashboards.tsx`, `Notebooks.tsx` and
   * `Alerts.tsx` headed the folderless bucket unconditionally until the gate
   * was lifted into the shared helper all seven now call. Nothing is left to
   * "restore consistency" with by deleting it.
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
      // the folderless workspace alike. Reading the collapse state here anyway
      // would strand rows: `useCollapsed` restores from localStorage, so a
      // visitor who collapsed Uncategorized while the workspace still had
      // folders would come back after the last one was deleted to two rows on
      // screen that "Select all" cannot reach and no header left to expand.
      showFolderHeaders
        ? groups.flatMap((g) => (collapse.isCollapsed(g.folder?.id ?? UNCATEGORIZED_KEY) ? [] : g.rows))
        : rows,
    [showFolderHeaders, rows, groups, collapse],
  );

  // One list behind both the count and the delete: `bulk.selectedIds` is
  // derived from exactly what was passed in here, so the strip cannot say "3
  // selected" next to a button that acts on a different 4.
  const bulk = useBulkSelect(visibleRows);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  function toggleStateFilter(s: string) {
    setStateFilter((f) => (f === s ? null : s));
  }

  /**
   * Enter or leave the trash, clearing every control the trash view does not
   * render.
   *
   * A filter left engaged behind a control that is no longer on screen is the
   * worst kind: the trash would render "No endpoints match the current
   * filters" over rows that are genuinely there, with nothing visible to
   * un-set. Bulk mode is dropped for the same reason — its toggle is gone, so
   * a `bulkMode` left true would strand a checkbox column the user cannot
   * dismiss (the same failure the `hasData` effect below exists to prevent).
   */
  function toggleTrash() {
    const next = !showTrash;
    setShowTrash(next);
    // Both directions, unconditionally. The sort has to follow the view because
    // the two views do not render the same last column — see `DEFAULT_SORT`.
    // Resetting only on the way IN would strand a trash sort (`deleted_at`) on
    // the normal list, where no header claims it, which is the same defect
    // mirrored.
    setSort(next ? DEFAULT_SORT.trash : DEFAULT_SORT.list);
    if (next) {
      setOnlyBookmarked(false);
      setStateFilter(null);
      setEngineFilter(null);
      bulk.setBulkMode(false);
    }
  }

  const hasData = endpoints.length > 0;

  // Bulk mode's only OFF switch is a toggle that renders on `hasData`. Delete
  // the last endpoint (in bulk, or one at a time from the row menu) and the
  // toggle unmounts while `bulkMode` stays true — the checkbox column then
  // reappears, unasked, the moment a new endpoint is created.
  const { bulkMode, setBulkMode } = bulk;
  useEffect(() => {
    if (!hasData && bulkMode) setBulkMode(false);
  }, [hasData, bulkMode, setBulkMode]);

  const live = endpoints.filter((e) => e.state === "live");
  const draft = endpoints.filter((e) => e.state === "draft");
  const disabled = endpoints.filter((e) => e.state === "disabled");

  function engineLabel(databaseId: string): string {
    const engine = engineOf(databaseId);
    if (!engine) return "—";
    return ENGINE_LABEL[engine] ?? engine;
  }

  // Only engines the workspace actually has a database for: a facet offering a
  // value that can never match any row is a dead control.
  const engineOptions = useMemo(() => {
    const seen = new Set<string>();
    return (databases ?? []).flatMap((d) =>
      seen.has(d.engine) ? [] : (seen.add(d.engine), [{ value: d.engine, label: ENGINE_LABEL[d.engine] ?? d.engine }]),
    );
  }, [databases]);

  /**
   * The selected rows that are `live`, and therefore cannot be trashed.
   *
   * Bulk Move to Trash is offered only when this is empty. Trashing the rest
   * and leaving the author to work out which ones silently failed is the
   * behaviour this replaces: a run over a mixed selection would half-succeed,
   * and the toast could only report a number.
   */
  const liveSelected = useMemo(
    () => visibleRows.filter((e) => bulk.selected.has(e.id) && e.state === "live"),
    [visibleRows, bulk.selected],
  );

  async function doBulkTrash() {
    // The same `selectedIds` the BulkBar counted — this closure holds THIS
    // render's `bulk`, so the array cannot change under the loop.
    const ids = bulk.selectedIds;
    // Both guards are defence, not decoration. `selectedIds` is re-derived from
    // the visible rows every render, so it can empty out under a refetch; and
    // the liveness re-check is not redundant with the disabled button, because
    // a refetch can move a row into `live` while the selection stands. A run
    // that does nothing and then toasts "Moved 0 endpoints to Trash" would be
    // worse than one that fails.
    if (ids.length === 0 || liveSelected.length > 0) {
      // Saying nothing here was the bug. From the author's side the button was
      // enabled, they clicked it, and N rows silently stayed put — no toast, no
      // selection, nothing to act on. Not claiming a false "Moved 0 to Trash"
      // was right; being silent about it was not, so each of the two re-checks
      // reports the reason it tripped and what to do about it.
      toast.error(
        liveSelected.length > 0
          ? `Nothing moved — ${liveSelected.length} of the selected endpoint${liveSelected.length === 1 ? " is" : "s are"} now live. Disable ${liveSelected.length === 1 ? "it" : "them"} first.`
          : "Nothing moved — the selected endpoints are no longer on this page. Select them again.",
      );
      bulk.clear();
      return;
    }
    let failed = 0;
    for (const id of ids) {
      try {
        await trashMutation.mutateAsync({ workspace: ws, id });
      } catch (err) {
        failed += 1;
        toast.error(String(err));
      }
    }
    if (failed === 0) toast.success(`Moved ${ids.length} endpoint${ids.length === 1 ? "" : "s"} to Trash`);
    bulk.clear();
  }

  /**
   * Star a row, or un-star it. The DIRECTION is read from this row's own
   * state and the id sent is this row's own id — a star that fired a fixed id
   * would look identical on a one-row list and wrong on every other.
   */
  async function toggleBookmark(e: DataApiEndpointListItemDto) {
    // From the OVERRIDE-aware value, so a second click while the first is
    // still in flight reverses it instead of repeating it.
    const next = !isBookmarked(e);
    setBookmarkOverride((prev) => ({ ...prev, [e.id]: next }));
    try {
      const mutation = next ? bookmarkMutation : unbookmarkMutation;
      await mutation.mutateAsync({ workspace: ws, id: e.id });
    } catch (err) {
      // Roll back — but only if THIS click's value is still the one on screen.
      // A slow rejection must not drag the star away from a later click that
      // has already moved it somewhere else.
      setBookmarkOverride((prev) => {
        if (prev[e.id] !== next) return prev;
        const rest = { ...prev };
        delete rest[e.id];
        return rest;
      });
      toast.error(String(err));
    }
  }

  /**
   * The folder dialog's submit: create the folder, and in `"move"` mode put the
   * row that opened it inside.
   *
   * The move is a second request on purpose — the create response is the only
   * place the new folder's id exists, so the PATCH cannot be sent until it has
   * come back. `folder_id` is the ONLY key in that PATCH body: `folder_id` is
   * `Option<Option<Uuid>>` server-side, so a body carrying the row's other
   * fields would re-submit whatever the list last fetched for them.
   */
  async function submitFolderDialog(name: string) {
    // Read from THIS render's state: `FolderNameDialog` closes itself right
    // after calling us, which sets `folderDialog` back to null.
    const d = folderDialog;
    if (!d) return;
    try {
      const folder = await createFolder.mutateAsync(name);
      if (d.mode === "move") {
        await updateMutation.mutateAsync({ workspace: ws, id: d.endpoint.id, data: { folder_id: folder.id } });
        toast.success("Moved");
      } else {
        toast.success("Folder created");
      }
    } catch (err) {
      toast.error(String(err));
    }
  }

  const renderRow = (e: DataApiEndpointListItemDto) => {
    const pending = pendingRoute(e, e.method, e.path);
    return (
      <tr key={e.id} className="border-t hover:bg-accent">
        {bulk.bulkMode && !showTrash && (
          <td className="py-1">
            <Checkbox
              checked={bulk.selected.has(e.id)}
              onCheckedChange={() => bulk.toggle(e.id)}
              aria-label={`Select ${e.name}`}
            />
          </td>
        )}
        <td className="py-1 pl-6">
          <span className="inline-flex items-center gap-1">
            {/* No star in the trash: `bookmarked` is a shortcut back to a row the
                author works with, and a trashed endpoint is not one. The POST
                would also 404 against a filtered-out row. */}
            {!showTrash && (
              <BookmarkStar bookmarked={isBookmarked(e)} name={e.name} onToggle={() => void toggleBookmark(e)} />
            )}
            <EntityLink to={`/explore/${ws}/data-api/endpoints/${e.id}`}>
              {e.name}
            </EntityLink>
          </span>
        </td>
        {/* The URL callers use — the LIVE route, which a draft edit does not
            move (KI-023) — and, under it, the draft's new one while that move
            waits for a publish. An endpoint never published shows its draft URL,
            the only one it has. */}
        <td className="text-muted-foreground">
          <span className="font-mono text-xs uppercase">{e.live_method ?? e.method}</span>{" "}
          <span className="font-mono text-xs">/{(e.live_path ?? e.path).replace(/^\//, "")}</span>
          {pending && (
            <span className="block text-xs" data-testid={`endpoint-pending-route-${e.id}`}>
              Pending publish:{" "}
              <span className="font-mono uppercase">{e.method}</span>{" "}
              <span className="font-mono">/{e.path.replace(/^\//, "")}</span>
            </span>
          )}
        </td>
        <td>
          <EndpointRowStateBadge endpoint={e} />
        </td>
        <td className="text-muted-foreground">{engineLabel(e.database_id)}</td>
        {/* The trash ages by when it was TRASHED, not when it was last edited —
            that is the column a Trash screen is read down, and it is why
            `deleted_at` is a timestamp on the DTO rather than a bare boolean. */}
        <td className="text-muted-foreground">
          {showTrash
            ? (e.deleted_at ? <RelativeTime iso={e.deleted_at} className="block" /> : "—")
            : <RelativeTime iso={e.updated_at} className="block" />}
        </td>
        <td>
          <EndpointActionsMenu
            ws={ws}
            endpoint={e}
            trashed={showTrash}
            folders={folders}
            onNewFolder={() => setFolderDialog({ mode: "move", endpoint: e })}
            onDelete={() => setPendingDelete(e)}
          />
        </td>
      </tr>
    );
  };

  // The canonical Explore list-page scaffold — `p-4` + a single `h1` — now
  // comes from `ListPageShell` rather than being hand-written here. Endpoints
  // is a top-level sidebar item, not a section nested under a `DataApiLayout`
  // shell (that shell, which used to own the page's `h1` and add `p-6`, was
  // removed once Data API got its own sidebar group), so the heading and the
  // padding still belong to this route — they just come from the shared shell.
  return (
    <ListPageShell
      title={showTrash ? "Trash" : "Endpoints"}
      toolbar={
        <>
          {/* Filters and bulk mode still render only on `hasData` — controls
              over zero rows can do nothing (§1.2 of docs/en/ui-conventions.md)
              — and never in the trash, which offers none of them. */}
          {!showTrash && hasData && (
            <>
              <BookmarkToggle active={onlyBookmarked} onToggle={() => setOnlyBookmarked((v) => !v)} />
              <BulkToggle active={bulk.bulkMode} onToggle={() => bulk.setBulkMode(!bulk.bulkMode)} />
              <FolderToolbar onNewFolder={() => setFolderDialog({ mode: "new" })} />
            </>
          )}
          {/* Trash is the odd one out in this row and the only one whose glyph
              reads as "destroy something": the three above change what this
              list SHOWS, it changes which list is being shown, and next to
              them at an identical size it is one mis-click from a filter.
              `role="separator"` rather than a bare styled div so the boundary
              is announced and not merely drawn — the same idiom
              `RefreshMenu.tsx` and `DashboardActionsMenu` use between a menu's
              safe and unsafe halves.

              The divider and Trash are ONE flex item, not two siblings, and
              that is a wrapping fix rather than a nesting preference. The
              shell's action group is `flex flex-wrap` (`ListPageShell.tsx`), so
              as siblings the line could break between them — measured live at a
              320px viewport, it did: the first row ended on a 1px hairline with
              nothing after it and Trash dropped to the next line, which reads
              as a stray mark rather than as a boundary. A flex item wraps whole,
              so there is no width at which they can be separated.

              The divider still appears and disappears with the three controls
              above — same condition, so the trash view and an empty list, where
              Trash stands alone, still cannot show a divider with nothing to
              its left. Trash itself stays UNGATED: it does not filter the rows
              in front of the user, it switches to a different server query, and
              an empty endpoint list says nothing about whether the trash is
              empty. Gated, it would also be the way OUT of an empty trash,
              removed exactly when it is needed. */}
          <div className="flex items-center gap-2">
            {!showTrash && hasData && (
              <div role="separator" aria-orientation="vertical" className="mx-1 h-5 w-px bg-border" />
            )}
            <ListToolbarButton icon={Trash2} label="Trash" active={showTrash} filled onClick={toggleTrash} />
          </div>
        </>
      }
      action={
        <>
          <Button variant="outline" onClick={() => setDownloadOpen(true)}>Download OpenAPI</Button>
          {/* A link, not a dialog trigger: creating an endpoint is its own page
              (`endpoints/new`), so it can be opened in a new tab and reloaded.
              Absent in the trash: the trash is where authoring is undone. */}
          {!showTrash && <Button asChild><Link to={`/explore/${ws}/data-api/endpoints/new`}>+ Endpoint</Link></Button>}
        </>
      }
    >
      <ListStates
        isLoading={isLoading}
        isError={isError}
        // The generated hook types its error as `ErrorBody` (the response
        // SHAPE), but the mutator (`mutator.ts`) throws the real `ApiError` it
        // parsed that body into — `ListStates` takes `unknown` and does the
        // `(error as Error)?.message || errorText` read itself.
        error={error}
        errorText="Could not load endpoints. Try again in a moment."
        hasData={hasData}
        skeletonCols={5}
        empty={
          showTrash ? (
            // Not the dashed first-run card: an empty trash is a normal, good
            // state, and offering "Create your first endpoint" from inside the
            // trash would answer a question nobody asked.
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              icon={Webhook}
              title="No endpoints yet"
              description="Publish a query as a REST endpoint."
              action={<Button asChild><Link to={`/explore/${ws}/data-api/endpoints/new`}>Create your first endpoint</Link></Button>}
            />
          )
        }
      >
        <div className="mb-3 flex flex-wrap items-center gap-2">
          <div className="relative max-w-xs flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input className="pl-8" placeholder="Search endpoints…" value={query} onChange={(e) => setQuery(e.target.value)} />
          </div>
          {!showTrash && (
            <>
              <FacetSelect
                label="State"
                value={stateFilter ?? FACET_ALL}
                onChange={(v) => setStateFilter(v === FACET_ALL ? null : v)}
                options={[
                  { value: "live", label: "live" },
                  { value: "draft", label: "draft" },
                  { value: "disabled", label: "disabled" },
                ]}
              />
              <FacetSelect
                label="Engine"
                value={engineFilter ?? FACET_ALL}
                onChange={(v) => setEngineFilter(v === FACET_ALL ? null : v)}
                options={engineOptions}
              />
            </>
          )}
        </div>

        <div className="mb-3">
          {/* All four numbers describe the SAME set — the whole list, unfiltered
              — so the row always adds up. It used to read `rows.length` (post
              filter) beside three counts taken from the raw list, so filtering
              by Engine produced "1 endpoints · 3 live · 5 draft · 2 disabled".
              Chosen over "make all four count the filtered set" because these
              three are also the state FILTER's chips: recomputed against the
              filtered rows they would read 0 for every state except the active
              one, and a chip labelled "0 draft" that reveals five drafts when
              clicked is worse than a total. The filtered count is not lost —
              the table below is the filtered set, with its own empty line. */}
          <SummaryStrip
            items={
              // In the trash the three state chips are also the state FILTER's
              // controls, and that filter is not offered here — a chip that
              // cannot be clicked into anything is a dead control, and a "0
              // live" chip over a list the API guarantees holds no live rows
              // (the schema's own CHECK) states a constraint as if it were a
              // measurement.
              showTrash
                ? [{ label: "in Trash", value: endpoints.length }]
                : [
                    { label: "endpoints total", singular: "endpoint total", value: endpoints.length },
                    { label: "live", value: live.length, onClick: () => toggleStateFilter("live"), active: stateFilter === "live" },
                    { label: "draft", value: draft.length, onClick: () => toggleStateFilter("draft"), active: stateFilter === "draft" },
                    { label: "disabled", value: disabled.length, onClick: () => toggleStateFilter("disabled"), active: stateFilter === "disabled" },
                  ]
            }
          />
        </div>

        {/* Move to Trash is the ONLY bulk action, exactly as in `Datasets.tsx`,
            `Charts.tsx` and `Dashboards.tsx`. A bulk permanent delete used to
            sit here, and it undid single-row deletion's whole reason for moving
            behind the trash: it destroyed served URLs and their published
            version history in one click, `live` rows included, which is the
            same act the row menu now charges three deliberate steps for and
            strictly more dangerous for being N at a time. The replacement path
            exists — bulk trash, then delete from the trash — so nothing became
            impossible, only deliberate. A BULK move between folders is still
            not wired; a disabled button for it would be worse than its
            absence. */}
        <BulkBar count={bulk.selectedIds.length}>
          <Button
            variant="outline"
            size="sm"
            // All-or-nothing, not best-effort: see `liveSelected`.
            disabled={liveSelected.length > 0}
            onClick={() => void doBulkTrash()}
          >
            Move {bulk.selectedIds.length} to Trash
          </Button>
          {liveSelected.length > 0 && (
            // The COUNT, not just "some are live": the author has to know how
            // much of the selection to go and disable.
            <span className="text-xs text-muted-foreground">
              {liveSelected.length} live — disable {liveSelected.length === 1 ? "it" : "them"} first
            </span>
          )}
        </BulkBar>

        {rows.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  {bulk.bulkMode && !showTrash && (
                    <th className="w-8 py-1">
                      <Checkbox checked={bulk.allSelected} onCheckedChange={bulk.toggleAll} aria-label="Select all" />
                    </th>
                  )}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <th className="px-2 font-medium">Method &amp; path</th>
                  <SortableHeader sortKey="state" label="State" current={sort} onSort={onSort} />
                  <th className="px-2 font-medium">Engine</th>
                  {showTrash ? (
                    <SortableHeader sortKey="deleted_at" label="Trashed" current={sort} onSort={onSort} />
                  ) : (
                    <SortableHeader sortKey="updated_at" label="Last edited" current={sort} onSort={onSort} />
                  )}
                  <th className="w-8" />
                </tr>
              </thead>
              {/* Flat wherever there is nothing to head — see
                  `showFolderHeaders`. That is the trash, always (folders
                  organise work in progress; a trashed row's folder is what it
                  will go BACK to, not where it is now, and grouping the trash
                  by it would hide rows behind collapsible headers in the one
                  view whose whole job is to let the author find something they
                  deleted) and a workspace that has no folders at all. */}
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
                        colSpan={bulk.bulkMode ? 7 : 6}
                        collapsed={isCollapsed}
                        onToggle={() => collapse.toggle(key)}
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
            {onlyBookmarked ? "No bookmarked endpoints." : "No endpoints match the current filters."}
          </p>
        )}
      </ListStates>

      <FolderNameDialog
        open={folderDialog !== null}
        title={folderDialog?.mode === "move" ? "Move to new folder" : "New folder"}
        submitLabel={folderDialog?.mode === "move" ? "Move" : "Create"}
        onOpenChange={(o) => { if (!o) setFolderDialog(null); }}
        onSubmit={(name) => void submitFolderDialog(name)}
      />

      <DownloadOpenApiDialog ws={ws} open={downloadOpen} onOpenChange={setDownloadOpen} />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        // Unconditional, not a `showTrash` ternary: the row menu offers
        // permanent delete ONLY in the trash, so this dialog has exactly one
        // caller and a second branch would be dead copy claiming to describe a
        // path that no longer exists.
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => {
          if (!pendingDelete) return;
          try {
            await deleteMutation.mutateAsync({ workspace: ws, id: pendingDelete.id });
            toast.success("Endpoint deleted");
          } catch (err) {
            toast.error(String(err));
          }
          setPendingDelete(null);
        }}
      />
    </ListPageShell>
  );
}

