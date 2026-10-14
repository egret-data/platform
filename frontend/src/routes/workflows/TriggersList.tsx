import { lazy, Suspense, useEffect, useId, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { keepPreviousData } from "@tanstack/react-query";
import { ChevronDown, ChevronLeft, ChevronRight, Power, PowerOff, RotateCw, Search, X, Zap } from "lucide-react";
import { ApiError } from "@/api/client";
import type { TriggerKindDto, TriggerPageDto, WorkflowListTriggersParams } from "@/api/generated/model";
import { useWorkflowListFlows, useWorkflowListNamespaces, useWorkflowListTriggers } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { EmptyState } from "@/components/explore/EmptyState";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import type { Enablement } from "@/components/workflows/draftState";
import { namespaceContains } from "@/components/workflows/namespaceTree";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { TriggerBanners } from "@/components/workflows/triggers/TriggerBanners";
import { selectionKey, TriggersTable, type TableRowData } from "@/components/workflows/triggers/TriggersTable";
import { bannersFor, kindLabel, MANAGE_REASON } from "@/components/workflows/triggers/triggersModel";
import { useTriggerSwitch } from "@/components/workflows/triggers/useTriggerSwitch";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/**
 * The drawer and the enable dialog, through `lazy()` only: they bring
 * `triggerYaml.ts` (the `yaml` library) and the dialog's form, which the list
 * itself never needs (`scripts/assert-workflows-lazy.mjs`, flagged `editor`).
 */
const GlobalTriggerDrawer = lazy(() => import("@/components/workflows/triggers/GlobalTriggerDrawer"));
const EnableTriggerDialog = lazy(() =>
  import("@/components/workflows/triggers/EnableTriggerDialog").then((m) => ({ default: m.EnableTriggerDialog })),
);

/** Rows per page (spec §4.4). */
export const TRIGGERS_PAGE_SIZE = 50;

const KINDS: TriggerKindDto[] = ["schedule", "webhook", "flow", "other"];
const STATES = [
  { value: "enabled", label: "Enabled" },
  { value: "disabled", label: "Disabled" },
];

/** How long the search box waits for the typing to stop before it asks (each ask parses every flow in scope). */
const SEARCH_DEBOUNCE_MS = 250;

/**
 * The page's filters, all in the URL (FE-PAGE-2):
 *
 * | key | value | default (absent) |
 * |---|---|---|
 * | `namespace` | a namespace, with every namespace below it | every namespace |
 * | `flow` | a flow's uuid | every flow |
 * | `kind` | kinds, one comma list | every kind |
 * | `state` | `enabled` or `disabled` | both |
 * | `q` | a substring of the trigger id, flow id or namespace | none |
 * | `page` | 1-based page | 1 |
 * | `trigger` | `<flow uuid>/<trigger id>`: the open drawer | closed |
 *
 * A value the page does not know (a kind or state from an old link) is read
 * as absent rather than sent, since the server answers it 400.
 */
type Filters = { namespace: string; flow: string; kinds: TriggerKindDto[]; state: string; q: string; page: number };

/** The note for one unreadable flow (FE-COPY-4); also this module's bundle marker (`scripts/assert-workflows-lazy.mjs`). */
export const UNPARSABLE_ONE = "1 flow could not be read; its triggers are not listed";

function readFilters(p: URLSearchParams): Filters {
  const kinds = (p.get("kind") ?? "")
    .split(",")
    .filter((k, i, all): k is TriggerKindDto => (KINDS as string[]).includes(k) && all.indexOf(k) === i);
  const state = p.get("state") ?? "";
  const page = Number.parseInt(p.get("page") ?? "1", 10);
  return {
    namespace: p.get("namespace") ?? "",
    flow: p.get("flow") ?? "",
    kinds,
    state: STATES.some((s) => s.value === state) ? state : "",
    q: p.get("q") ?? "",
    page: Number.isFinite(page) && page >= 1 ? page : 1,
  };
}

/** The request for these filters: an unset filter is left out, never sent empty. */
function toParams(f: Filters): WorkflowListTriggersParams {
  return {
    namespace_prefix: f.namespace || undefined,
    flow_id: f.flow || undefined,
    kind: f.kinds.length > 0 ? f.kinds.join(",") : undefined,
    state: f.state || undefined,
    q: f.q.trim() || undefined,
    limit: TRIGGERS_PAGE_SIZE,
    offset: (f.page - 1) * TRIGGERS_PAGE_SIZE,
  };
}

/** `prev` with `patch` written in; any filter change but the page's own goes back to the first page. */
function withFilters(prev: URLSearchParams, patch: Partial<Filters>): URLSearchParams {
  const next = new URLSearchParams(prev);
  const set = (key: string, value: string) => (value ? next.set(key, value) : next.delete(key));
  if (patch.namespace !== undefined) set("namespace", patch.namespace);
  if (patch.flow !== undefined) set("flow", patch.flow);
  if (patch.kinds !== undefined) set("kind", patch.kinds.join(","));
  if (patch.state !== undefined) set("state", patch.state);
  if (patch.q !== undefined) set("q", patch.q);
  set("page", patch.page !== undefined && patch.page > 1 ? String(patch.page) : "");
  return next;
}

const isFiltering = (f: Filters) => !!(f.namespace || f.flow || f.kinds.length > 0 || f.state || f.q.trim());

const CLEARED: Partial<Filters> = { namespace: "", flow: "", kinds: [], state: "", q: "" };

/** A failed read, in words (FE-COPY-2): the server's sentence, ending once, or what to do. */
const readError = (e: unknown) =>
  e instanceof ApiError && e.message ? `${e.message.replace(/\.+$/, "")}.` : "the request failed. Check your connection and try again.";

/** The flows of a 422 `triggers_too_large` (D9): narrowing by namespace is the way out. */
const isTooLarge = (e: unknown) => e instanceof ApiError && e.status === 422 && e.code === "triggers_too_large";

/** `?trigger=<flow uuid>/<trigger id>`, split at the first slash (a uuid has none). */
function readOpen(value: string | null): { flowUuid: string; triggerId: string } | null {
  if (!value) return null;
  const at = value.indexOf("/");
  if (at <= 0 || at === value.length - 1) return null;
  return { flowUuid: value.slice(0, at), triggerId: value.slice(at + 1) };
}

/**
 * The workspace's Triggers page (W7e spec §4.4), `workflows/triggers`: every
 * trigger of every flow not in the trash, filtered by namespace (with the
 * namespaces below it), flow, kind, Enabled / Disabled and a search, a page
 * of 50 at a time; the flow tab's switches and bulk Enable / Disable over a
 * selection that may span flows; and the details drawer.
 *
 * The page does not learn `workflow:manage` from a flag of its own: a viewer
 * without it gets every switchable row refused `forbidden` (§3.5), and that is
 * what turns selection and bulk off here.
 */
export default function TriggersList() {
  const ws = useParams().workspace!;
  const base = `/explore/${ws}/workflows`;
  const [params, setParams] = useSearchParams();
  const filters = useMemo(() => readFilters(params), [params]);
  const request = useMemo(() => toParams(filters), [filters]);
  // A picker pushes an entry, so Back undoes it; the search box replaces its own.
  const setFilters = (patch: Partial<Filters>, replace = false) => setParams((p) => withFilters(p, patch), { replace });

  const query = useWorkflowListTriggers(ws, request, {
    query: { queryKey: workflowsKeys.triggers(ws, request), placeholderData: keepPreviousData },
  });
  const page: TriggerPageDto | undefined = query.data?.status === 200 ? query.data.data : undefined;
  const tooLarge = isTooLarge(query.error);

  // The selection belongs to the page and filters it was made on; another page or filter starts empty.
  const scope = JSON.stringify(request);
  const [picked, setPicked] = useState<{ scope: string; keys: Set<string> }>({ scope, keys: new Set() });
  // Dropped, not set aside: coming back to a page does not bring an old selection back.
  if (picked.scope !== scope) setPicked({ scope, keys: new Set() });
  const selection = picked.scope === scope ? picked.keys : new Set<string>();
  const setSelection = (keys: Set<string>) => setPicked({ scope, keys });
  const sw = useTriggerSwitch(ws, { onBulkAnswered: () => setPicked({ scope: "", keys: new Set() }) });
  const reasonsId = useId();

  const namespaceRef = useRef<HTMLDivElement>(null);
  // D9: the way out of "too many flows" is the namespace filter, so the focus goes there.
  useEffect(() => {
    if (tooLarge) namespaceRef.current?.querySelector<HTMLElement>("button")?.focus();
  }, [tooLarge, query.errorUpdatedAt]);

  const open = readOpen(params.get("trigger"));
  const detailsHref = (row: TableRowData) => {
    const next = new URLSearchParams(params);
    next.set("trigger", `${row.flow_uuid}/${row.trigger_id}`);
    return `?${next.toString()}`;
  };
  const closeDrawer = () =>
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete("trigger");
      return next;
    });

  function renderContent(): ReactNode {
    // Error first (FE-STATE-1), while nothing has loaded for these filters.
    if (query.isError && !page) {
      if (tooLarge) {
        return (
          <Alert variant="destructive">
            <AlertDescription>The triggers could not be listed: {readError(query.error)}</AlertDescription>
          </Alert>
        );
      }
      return (
        <Alert variant="destructive">
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>The triggers could not be loaded: {readError(query.error)}</span>
            <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
              <RotateCw aria-hidden />
              Retry
            </Button>
          </AlertDescription>
        </Alert>
      );
    }
    if (!page) return <ListSkeleton cols={11} />;

    const rows: TableRowData[] = page.rows;
    const manage = !rows.some((r) => r.toggle_refusal === "forbidden");
    // A trigger id may repeat within a flow (`duplicate_id`); the switch is per id, so it is selected once.
    const selected = rows.filter((r, i) => selection.has(selectionKey(r)) && rows.findIndex((o) => selectionKey(o) === selectionKey(r)) === i);
    const bulk: Enablement = !manage
      ? { ok: false, reason: MANAGE_REASON }
      : selected.length === 0
        ? { ok: false, reason: "Select triggers first" }
        : { ok: true };
    const flowName = new Map(page.rows.map((r) => [r.flow_uuid, `${r.namespace}.${r.flow_id}`]));
    // D18: a flow whose state is disabled fires none of its triggers, though each stays switched on.
    const disabledFlows = new Set(page.rows.filter((r) => r.flow_disabled).map((r) => r.flow_uuid));

    const notes = (
      <>
        <TriggerBanners banners={bannersFor(page)} />
        {page.unparsable > 0 ? (
          <Alert role="status">
            <AlertDescription>
              {page.unparsable === 1 ? UNPARSABLE_ONE : `${page.unparsable} flows could not be read; their triggers are not listed`}
            </AlertDescription>
          </Alert>
        ) : null}
        {query.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              The triggers could not be refreshed: {readError(query.error)} What is shown is the last answer.
            </AlertDescription>
          </Alert>
        ) : null}
      </>
    );

    if (rows.length === 0) {
      let empty: ReactNode;
      if (page.total > 0) {
        // A shared link to a page that no longer exists.
        empty = (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm text-muted-foreground">This page is past the end of the list.</p>
            <Button variant="outline" size="sm" onClick={() => setFilters({ page: 1 })}>
              Go to the first page
            </Button>
          </div>
        );
      } else if (isFiltering(filters)) {
        empty = (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm text-muted-foreground">No triggers match these filters</p>
            <Button variant="outline" size="sm" onClick={() => setFilters(CLEARED)}>
              Clear filters
            </Button>
          </div>
        );
      } else {
        empty = (
          <EmptyState
            variant="dashed"
            icon={Zap}
            title="No triggers yet"
            description="A trigger appears here when a flow declares one under triggers: in its YAML."
            action={
              <Button asChild>
                <Link to={`${base}/flows`}>Go to flows</Link>
              </Button>
            }
          />
        );
      }
      return (
        <div className="grid gap-3">
          {notes}
          {empty}
        </div>
      );
    }

    return (
      <div className="grid gap-3">
        {notes}
        <div data-testid="triggers-summary">
          <SummaryStrip items={[{ label: "triggers", singular: "trigger", value: page.total }]} />
        </div>
        <div className="flex flex-col items-start gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <ReasonButton enablement={bulk} reasonId={`${reasonsId}-enable`} variant="outline" busy={sw.anyBusy} onClick={() => sw.switchMany(selected, true)}>
              <Power aria-hidden />
              Enable
            </ReasonButton>
            <ReasonButton enablement={bulk} reasonId={`${reasonsId}-disable`} variant="outline" busy={sw.anyBusy} onClick={() => sw.switchMany(selected, false)}>
              <PowerOff aria-hidden />
              Disable
            </ReasonButton>
            {selected.length > 0 ? (
              <span className="text-sm text-muted-foreground tabular-nums">
                {selected.length === 1 ? "1 trigger selected" : `${selected.length} triggers selected`}
              </span>
            ) : null}
          </div>
          <DisabledReasons
            actions={[
              { id: `${reasonsId}-enable`, label: "Enable", enablement: bulk },
              { id: `${reasonsId}-disable`, label: "Disable", enablement: bulk },
            ]}
          />
        </div>
        {sw.refusals.length > 0 ? (
          <Alert variant="destructive" data-testid="bulk-refusals">
            <AlertDescription className="grid gap-1">
              <div className="flex items-start justify-between gap-2">
                <p>{sw.refusals.length === 1 ? "1 trigger was not switched:" : `${sw.refusals.length} triggers were not switched:`}</p>
                <Button type="button" size="iconSm" variant="ghost" aria-label="Dismiss" onClick={sw.clearRefusals}>
                  <X aria-hidden />
                </Button>
              </div>
              <ul className="list-disc pl-5" role="list">
                {sw.refusals.map((r) => (
                  <li key={`${r.flow_uuid}/${r.trigger_id}`}>
                    <span className="font-mono">{flowName.get(r.flow_uuid) ?? r.flow_uuid}</span> ·{" "}
                    <span className="font-mono">{r.trigger_id}</span>: {r.message}
                  </li>
                ))}
              </ul>
            </AlertDescription>
          </Alert>
        ) : null}
        <div aria-busy={query.isPlaceholderData} className={query.isPlaceholderData ? "opacity-60" : undefined}>
          <TriggersTable
            rows={rows}
            variant="global"
            base={base}
            manage={manage}
            selection={selection}
            onSelectionChange={setSelection}
            onToggle={sw.toggle}
            isBusy={sw.isBusy}
            // The list leaves out flows in the trash (§3.5).
            contextOf={(row) => ({ flowDisabled: disabledFlows.has(row.flow_uuid), flowDeleted: false })}
            detailsHref={detailsHref}
          />
        </div>
        <Pager page={page} current={filters.page} onPage={(n) => setFilters({ page: n })} />
      </div>
    );
  }

  return (
    <ListPageShell
      title="Triggers"
      toolbar={<ListToolbarButton icon={RotateCw} label="Refresh" onClick={() => void query.refetch()} />}
    >
      <FilterBar ws={ws} filters={filters} setFilters={setFilters} namespaceRef={namespaceRef} />
      {renderContent()}
      {open ? (
        <Suspense fallback={null}>
          <GlobalTriggerDrawer
            key={`${open.flowUuid}/${open.triggerId}`}
            ws={ws}
            flowUuid={open.flowUuid}
            triggerId={open.triggerId}
            onClose={closeDrawer}
          />
        </Suspense>
      ) : null}
      {sw.question ? (
        <Suspense fallback={null}>
          <EnableTriggerDialog
            key={sw.question.rows.map(selectionKey).join(",")}
            open
            count={sw.question.rows.length}
            bulk={sw.question.bulk}
            schedules={sw.question.schedules}
            busy={sw.answering}
            onCancel={sw.dismiss}
            onEnable={(choice) => void sw.answer(choice)}
          />
        </Suspense>
      ) : null}
    </ListPageShell>
  );
}

/** Namespace (a picker over the workspace's namespaces, FE-FORM-3), flow, kind, state and search. */
function FilterBar({
  ws,
  filters,
  setFilters,
  namespaceRef,
}: {
  ws: string;
  filters: Filters;
  setFilters: (patch: Partial<Filters>, replace?: boolean) => void;
  namespaceRef: RefObject<HTMLDivElement | null>;
}) {
  const id = useId();
  const namespacesQuery = useWorkflowListNamespaces(ws, undefined, { query: { queryKey: workflowsKeys.namespaces(ws) } });
  const flowsQuery = useWorkflowListFlows(ws, undefined, { query: { queryKey: workflowsKeys.flows(ws) } });

  const namespaceOptions = useMemo(() => {
    const known = (namespacesQuery.data?.status === 200 ? namespacesQuery.data.data : []).map((n) => n.namespace).sort((a, b) => a.localeCompare(b));
    // A shared link can name a namespace that has since emptied; it stays selectable.
    if (filters.namespace && !known.includes(filters.namespace)) known.push(filters.namespace);
    return known.map((ns) => ({ value: ns, label: ns }));
  }, [namespacesQuery.data, filters.namespace]);

  const flowOptions = useMemo(() => {
    const flows = flowsQuery.data?.status === 200 ? flowsQuery.data.data : [];
    const opts = flows
      .filter((f) => !filters.namespace || namespaceContains(filters.namespace, f.namespace))
      .map((f) => ({ value: f.id, label: `${f.namespace}.${f.flow_id}` }))
      .sort((a, b) => a.label.localeCompare(b.label));
    if (filters.flow && !opts.some((o) => o.value === filters.flow)) opts.push({ value: filters.flow, label: filters.flow });
    return opts;
  }, [flowsQuery.data, filters.namespace, filters.flow]);

  // The search box writes the URL once the typing stops; the URL wins when it changes otherwise
  // (Clear filters, Back).
  const [draft, setDraft] = useState(filters.q);
  const written = useRef(filters.q);
  useEffect(() => {
    if (filters.q !== written.current) {
      written.current = filters.q;
      setDraft(filters.q);
    }
  }, [filters.q]);
  useEffect(() => {
    if (draft === written.current) return;
    const t = window.setTimeout(() => {
      written.current = draft;
      setFilters({ q: draft }, true);
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(t);
    // `setFilters` is a new function each render; the draft alone decides when to write.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft]);

  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      <div className="relative max-w-xs flex-1">
        <Label htmlFor={`${id}-search`} className="sr-only">
          Search triggers
        </Label>
        <Search
          aria-hidden="true"
          className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          id={`${id}-search`}
          className="pl-8"
          placeholder="Search trigger id, flow or namespace…"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
        />
      </div>
      <div ref={namespaceRef} className="contents">
        <FacetSelect
          label="Namespace"
          value={filters.namespace || FACET_ALL}
          // A flow picked outside the new namespace would match nothing, so the flow goes with it.
          onChange={(v) => setFilters({ namespace: v === FACET_ALL ? "" : v, flow: "" })}
          options={namespaceOptions}
        />
      </div>
      <FacetSelect
        label="Flow"
        value={filters.flow || FACET_ALL}
        onChange={(v) => setFilters({ flow: v === FACET_ALL ? "" : v })}
        options={flowOptions}
      />
      <KindPicker kinds={filters.kinds} onChange={(kinds) => setFilters({ kinds })} />
      <FacetSelect
        label="State"
        value={filters.state || FACET_ALL}
        onChange={(v) => setFilters({ state: v === FACET_ALL ? "" : v })}
        options={STATES}
      />
    </div>
  );
}

/** Several kinds at once, sent as one comma list. The menu stays open while picking. */
function KindPicker({ kinds, onChange }: { kinds: TriggerKindDto[]; onChange: (kinds: TriggerKindDto[]) => void }) {
  const summary = kinds.length === 0 ? "All" : kinds.length <= 2 ? kinds.map(kindLabel).join(", ") : `${kinds.length} selected`;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" className="h-9">
          <span className="text-muted-foreground">Kind:</span>
          {summary}
          <ChevronDown aria-hidden className="opacity-50" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {kinds.length > 0 ? (
          <>
            <DropdownMenuItem onSelect={() => onChange([])}>Show every kind</DropdownMenuItem>
            <DropdownMenuSeparator />
          </>
        ) : null}
        {KINDS.map((k) => (
          <DropdownMenuCheckboxItem
            key={k}
            checked={kinds.includes(k)}
            onSelect={(e) => e.preventDefault()}
            onCheckedChange={(on) => onChange(on ? [...kinds, k] : kinds.filter((x) => x !== k))}
          >
            {kindLabel(k)}
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Pager({ page, current, onPage }: { page: TriggerPageDto; current: number; onPage: (n: number) => void }) {
  const pages = Math.max(1, Math.ceil(page.total / TRIGGERS_PAGE_SIZE));
  if (pages <= 1 && current <= 1) return null;
  const first = (current - 1) * TRIGGERS_PAGE_SIZE + 1;
  const last = first + page.rows.length - 1;
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-muted-foreground">
      <span className="tabular-nums">{`Showing ${first}–${last} of ${page.total}`}</span>
      <div className="flex items-center gap-2">
        <Button variant="outline" size="sm" disabled={current <= 1} onClick={() => onPage(current - 1)}>
          <ChevronLeft aria-hidden />
          Previous
        </Button>
        <span className="tabular-nums">{`Page ${current} of ${pages}`}</span>
        <Button variant="outline" size="sm" disabled={current >= pages} onClick={() => onPage(current + 1)}>
          Next
          <ChevronRight aria-hidden />
        </Button>
      </div>
    </div>
  );
}
