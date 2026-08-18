import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { Search, Webhook } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { EmptyState } from "@/components/explore/EmptyState";
import { BulkToggle, BulkBar } from "@/components/explore/BulkBar";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { useFoldersFoldersList } from "@/api/generated/folders/folders";
import { useDatabases, type DatabaseConn } from "@/api/databases";
import { useDataApiListEndpoints } from "@/api/generated/data-api/data-api";
// Mutations come from the hand-written wrapper, never from the generated hooks:
// the wrapper is what owns the cache invalidation (see `src/api/dataApi.ts`).
// Wired to the generated hook directly, "Endpoint disabled" toasts over a row
// whose badge still reads Live for the next 30 seconds.
import { useCreateEndpoint, useDeleteEndpoint } from "@/api/dataApi";
import type { DataApiEndpointDto } from "@/api/generated/model";
import { DownloadOpenApiDialog } from "./DownloadOpenApiDialog";
import { EndpointActionsMenu } from "./EndpointActionsMenu";

// Folders share the generic router (egret-bi's `kind_perms`); this is the
// `{kind}` URL segment data-api registers there, matching
// `crates/egret-bi/src/routes.rs`'s `kind_perms("data_api_endpoints")`.
const FOLDER_KIND = "data_api_endpoints";

/** `useCollapsed`'s key for the folderless group (`groupByFolder`'s `folder: null`). */
const UNCATEGORIZED_KEY = "__uncat__";

type SortKey = "name" | "state" | "updated_at";

// live is the only state that earns emphasis (--highlight is reserved for a
// page's single primary accent, not a state chip) — draft and disabled both
// read as muted/negative, never as something to celebrate.
const STATE_TONE: Record<string, string> = {
  live: "bg-success/15 text-success",
  draft: "bg-muted text-muted-foreground",
  disabled: "bg-destructive/15 text-destructive",
};

const ENGINE_LABEL: Record<string, string> = { trino: "Trino", starrocks: "StarRocks" };

/**
 * The list DTO has no version count — only `live_version_id`, a nullable id.
 * Showing it as its own "Version" column was tried and dropped (see
 * `docs/en/specs/2026-08-16-data-api-p5-design.md` §2.3): for `live` and
 * `draft` the id fully tracks the state — a fresh draft is created with it
 * null (migration 0121), and `EndpointStore::publish` (`lifecycle.rs`) is the
 * only place that ever sets it, atomically with `state = 'live'` — so
 * labelling those two states alone would only repeat the badge's own colour
 * in words.
 *
 * `disabled` is the one state where the id carries real information:
 * `EndpointStore::disable` (`lifecycle.rs`) has no state guard and never
 * touches `live_version_id`, so a draft can be disabled directly — a
 * `disabled` row can mean "was serving, paused" or "never served at all",
 * and those need different next actions (Enable vs. Publish). Folding both
 * into a bare "Disabled" badge would read as "currently serving" in the
 * worse of the two cases.
 */
function stateBadgeLabel(e: DataApiEndpointDto): string {
  if (e.state === "disabled") {
    return e.live_version_id ? "Disabled · was live" : "Disabled · never published";
  }
  return e.state.length ? e.state.charAt(0).toUpperCase() + e.state.slice(1) : e.state;
}

export default function Endpoints() {
  const ws = useParams().workspace!;
  const nav = useNavigate();

  const { data: envelope, isLoading, isError, error } = useDataApiListEndpoints(ws);
  // useMemo (not a plain ternary) so a falsy envelope doesn't hand `rows`'
  // useMemo a fresh `[]` identity on every render.
  const endpoints = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const { data: foldersEnvelope } = useFoldersFoldersList(ws, FOLDER_KIND);
  const folders = useMemo(() => (foldersEnvelope?.status === 200 ? foldersEnvelope.data : []), [foldersEnvelope]);

  const { data: databases } = useDatabases(ws);

  const createMutation = useCreateEndpoint();
  const deleteMutation = useDeleteEndpoint();

  const collapse = useCollapsed(`folders:${ws}:${FOLDER_KIND}`);

  const [query, setQuery] = useState("");
  // ONE piece of state behind two controls: the SummaryStrip chip (a shortcut)
  // and the State facet (the discoverable control). Two filters that can
  // disagree is worse than one that is hard to find.
  const [stateFilter, setStateFilter] = useState<string | null>(null);
  const [engineFilter, setEngineFilter] = useState<string | null>(null);
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [createOpen, setCreateOpen] = useState(false);
  const [downloadOpen, setDownloadOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<DataApiEndpointDto | null>(null);
  const [bulkDeleteOpen, setBulkDeleteOpen] = useState(false);

  // `database_id` -> engine, so the Engine column and the Engine facet read the
  // same source rather than each doing its own lookup.
  const engineOf = useMemo(() => {
    const byId = new Map((databases ?? []).map((d) => [d.id, d.engine]));
    return (databaseId: string) => byId.get(databaseId);
  }, [databases]);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = endpoints.filter((e) => {
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
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [endpoints, query, stateFilter, engineFilter, engineOf, sort]);

  const filtering = query.trim() !== "" || stateFilter !== null || engineFilter !== null;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  /**
   * The rows actually ON SCREEN — not `rows`, which is only what survived
   * filtering and sorting: `groupByFolder` drops orphan-`folder_id` rows, and a
   * collapsed folder's rows are in `groups` but not in front of the user.
   *
   * This is what `useBulkSelect` must be fed; the full argument for why — and
   * what breaks when a caller passes more — lives on the hook itself
   * (`src/hooks/useBulkSelect.ts`), because that is where the next caller will
   * be reading.
   */
  const visibleRows = useMemo(
    () => groups.flatMap((g) => (collapse.isCollapsed(g.folder?.id ?? UNCATEGORIZED_KEY) ? [] : g.rows)),
    [groups, collapse],
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

  async function doBulkDelete() {
    // The same `selectedIds` the BulkBar counted — this closure holds THIS
    // render's `bulk`, so the array cannot change under the loop.
    const ids = bulk.selectedIds;
    // The dialog is open across renders, and `selectedIds` is re-derived from
    // the visible rows on each one — a refetch that drops the selected rows (or
    // a folder collapsed behind the dialog) can empty it between "Delete
    // selected" and the confirm click. Without this the run does nothing and
    // then claims "Deleted 0 endpoints".
    if (ids.length === 0) {
      bulk.clear();
      return;
    }
    let failed = 0;
    for (const id of ids) {
      try {
        await deleteMutation.mutateAsync({ workspace: ws, id });
      } catch (err) {
        failed += 1;
        toast.error(String(err));
      }
    }
    if (failed === 0) toast.success(`Deleted ${ids.length} endpoint${ids.length === 1 ? "" : "s"}`);
    bulk.clear();
  }

  const renderRow = (e: DataApiEndpointDto) => (
    <tr key={e.id} className="border-t hover:bg-accent">
      {bulk.bulkMode && (
        <td className="py-1">
          <Checkbox
            checked={bulk.selected.has(e.id)}
            onCheckedChange={() => bulk.toggle(e.id)}
            aria-label={`Select ${e.name}`}
          />
        </td>
      )}
      <td className="py-1 pl-6">
        <Link className="font-medium hover:underline" to={`/explore/${ws}/data-api/endpoints/${e.id}`}>
          {e.name}
        </Link>
      </td>
      <td className="text-muted-foreground">
        <span className="font-mono text-xs uppercase">{e.method}</span>{" "}
        <span className="font-mono text-xs">/{e.path.replace(/^\//, "")}</span>
      </td>
      <td>
        <Badge className={STATE_TONE[e.state] ?? "bg-muted text-muted-foreground"}>
          {stateBadgeLabel(e)}
        </Badge>
      </td>
      <td className="text-muted-foreground">{engineLabel(e.database_id)}</td>
      <td className="text-muted-foreground"><RelativeTime iso={e.updated_at} className="block" /></td>
      <td>
        <EndpointActionsMenu ws={ws} endpoint={e} onDelete={() => setPendingDelete(e)} />
      </td>
    </tr>
  );

  // The canonical Explore list-page scaffold — `p-4` + a single `h1` — now
  // comes from `ListPageShell` rather than being hand-written here. Endpoints
  // is a top-level sidebar item, not a section nested under a `DataApiLayout`
  // shell (that shell, which used to own the page's `h1` and add `p-6`, was
  // removed once Data API got its own sidebar group), so the heading and the
  // padding still belong to this route — they just come from the shared shell.
  return (
    <ListPageShell
      title="Endpoints"
      toolbar={hasData ? <BulkToggle active={bulk.bulkMode} onToggle={() => bulk.setBulkMode(!bulk.bulkMode)} /> : undefined}
      action={
        <>
          <Button variant="outline" onClick={() => setDownloadOpen(true)}>Download OpenAPI</Button>
          <Button onClick={() => setCreateOpen(true)}>+ Endpoint</Button>
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
          <EmptyState
            variant="dashed"
            icon={Webhook}
            title="No endpoints yet"
            description="Publish a query as a REST endpoint."
            action={<Button onClick={() => setCreateOpen(true)}>Create your first endpoint</Button>}
          />
        }
      >
        <div className="mb-3 flex flex-wrap items-center gap-2">
          <div className="relative max-w-xs flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input className="pl-8" placeholder="Search endpoints…" value={query} onChange={(e) => setQuery(e.target.value)} />
          </div>
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
            items={[
              { label: "endpoints total", value: endpoints.length },
              { label: "live", value: live.length, onClick: () => toggleStateFilter("live"), active: stateFilter === "live" },
              { label: "draft", value: draft.length, onClick: () => toggleStateFilter("draft"), active: stateFilter === "draft" },
              { label: "disabled", value: disabled.length, onClick: () => toggleStateFilter("disabled"), active: stateFilter === "disabled" },
            ]}
          />
        </div>

        {/* Delete is the only bulk action this page can offer: moving to a
            folder needs `folder_id` on `UpdateEndpointInput` and moving to
            trash needs the soft-delete migration, neither of which exists yet.
            A disabled button for either would be worse than its absence. */}
        <BulkBar count={bulk.selectedIds.length}>
          <Button variant="destructive" size="sm" onClick={() => setBulkDeleteOpen(true)}>Delete selected</Button>
        </BulkBar>

        {rows.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  {bulk.bulkMode && (
                    <th className="w-8 py-1">
                      <Checkbox checked={bulk.allSelected} onCheckedChange={bulk.toggleAll} aria-label="Select all" />
                    </th>
                  )}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <th className="px-2 font-medium">Method &amp; path</th>
                  <SortableHeader sortKey="state" label="State" current={sort} onSort={onSort} />
                  <th className="px-2 font-medium">Engine</th>
                  <SortableHeader sortKey="updated_at" label="Last edited" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              {groups.map((group) => {
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
              })}
            </table>
          </div>
        ) : (
          <p className="text-muted-foreground">No endpoints match the current filters.</p>
        )}
      </ListStates>

      <CreateEndpointDialog
        ws={ws}
        open={createOpen}
        onOpenChange={setCreateOpen}
        databases={databases ?? []}
        create={createMutation}
        onCreated={(id) => nav(`/explore/${ws}/data-api/endpoints/${id}`)}
      />

      <DownloadOpenApiDialog ws={ws} open={downloadOpen} onOpenChange={setDownloadOpen} />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete endpoint"
        description={`Delete "${pendingDelete?.name ?? ""}"? Any deployed client calling its URL will start getting 404s. This cannot be undone.`}
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

      <ConfirmDeleteDialog
        open={bulkDeleteOpen}
        onOpenChange={setBulkDeleteOpen}
        title="Delete endpoints"
        description={`Delete ${bulk.selectedIds.length} endpoint${bulk.selectedIds.length === 1 ? "" : "s"}? Any deployed client calling their URLs will start getting 404s. This cannot be undone.`}
        onConfirm={doBulkDelete}
      />
    </ListPageShell>
  );
}

/**
 * Per the Alerts lesson (#485): a create dialog collects only the fields a
 * row cannot exist without, then hands off to the real editor. Everything
 * else (description, params, output shape, cache, limits…) is edited on the
 * detail route — this dialog must never grow into a second editor.
 *
 * `sql` is here too, alongside name/path/method/database — `CreateEndpointInput`
 * requires it (no `#[serde(default)]`, unlike the config fields), so a row
 * cannot exist without it either. Kept to a single-line starting point; the
 * real query editor is the detail route's Definition section.
 */
function CreateEndpointDialog({ ws, open, onOpenChange, databases, create, onCreated }: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  databases: DatabaseConn[];
  create: ReturnType<typeof useCreateEndpoint>;
  onCreated: (id: string) => void;
}) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [method, setMethod] = useState("GET");
  const [databaseId, setDatabaseId] = useState("");
  const [sql, setSql] = useState("");

  useEffect(() => {
    if (open) {
      setName("");
      setPath("");
      setMethod("GET");
      setDatabaseId("");
      setSql("");
    }
  }, [open]);

  const canSubmit = name.trim() !== "" && path.trim() !== "" && databaseId !== "" && sql.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    try {
      const res = await create.mutateAsync({
        workspace: ws,
        data: { name: name.trim(), path: path.trim(), method, database_id: databaseId, sql: sql.trim() },
      });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — the check just gives TS a
      // narrower type than the response union declares.
      if (res.status !== 201) { toast.error("Could not create endpoint."); return; }
      toast.success("Endpoint created");
      onOpenChange(false);
      onCreated(res.data.id);
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>New endpoint</DialogTitle></DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="ep-name">Name</Label>
            <Input id="ep-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Daily report" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ep-path">Path</Label>
            <Input id="ep-path" value={path} onChange={(e) => setPath(e.target.value)} placeholder="reports/daily" />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="ep-method">Method</Label>
              <Select value={method} onValueChange={setMethod}>
                <SelectTrigger id="ep-method" aria-label="Method"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="GET">GET</SelectItem>
                  <SelectItem value="POST">POST</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ep-db">Database</Label>
              <Select value={databaseId} onValueChange={setDatabaseId}>
                <SelectTrigger id="ep-db" aria-label="Database">
                  <SelectValue placeholder="Choose a database" />
                </SelectTrigger>
                <SelectContent>
                  {databases.map((d) => (
                    <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ep-sql">SQL</Label>
            <Textarea id="ep-sql" className="font-mono text-xs" rows={4} value={sql} onChange={(e) => setSql(e.target.value)} placeholder="SELECT …" />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={!canSubmit || create.isPending}>
            {create.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
