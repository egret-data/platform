import { useId, useMemo, type ReactNode } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronLeft, ChevronRight, History, RotateCw } from "lucide-react";
import dayjs from "dayjs";
import { useWorkflowListFlows, workflowListExecutions } from "@/api/generated/workflow/workflow";
import type { ExecutionListItemDto, FlowListItemDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/explore/EmptyState";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { SummaryStrip, type SummaryStripItem } from "@/components/explore/SummaryStrip";
import { StateChip } from "@/components/workflows/StateChip";
import { WorkerOffNotice } from "@/components/workflows/WorkerOffNotice";
import { formatDuration, isTerminalState } from "@/components/workflows/executionState";
import {
  ESTIMATED_DURATION_TITLE,
  executionDuration,
  executionStart,
  startTitle,
} from "@/components/workflows/executions/timing";
import { useNow } from "@/components/workflows/executions/polling";
import { TriggerLabel } from "@/components/workflows/executions/trigger";
import { knownNamespaces, namespaceContains } from "@/components/workflows/namespaceTree";
import { timeAgo } from "@/lib/relativeTime";
import {
  clearedFilters,
  EXECUTION_STATE_NAMES,
  isFiltering,
  LIST_POLL_MS,
  PAGE_SIZE,
  pageCount,
  readFilters,
  TIME_PRESETS,
  toQueryParams,
  withFilters,
  type ExecutionFilters,
} from "@/routes/workflows/executionsListModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** One page as the list shows it: the rows, the whole match's size, and whether the server's worker runs. */
interface ListPage {
  rows: ExecutionListItemDto[];
  total: number;
  workerEnabled: boolean;
}

/**
 * Reads one page. The time preset becomes `since` HERE, at fetch time, so each
 * poll asks for the window ending now while the query key (the filters, not
 * the timestamps) stays put. That is why this calls the orval fetcher inside
 * `useQuery` rather than `useWorkflowListExecutions`, whose params, and so
 * whose `since`, are fixed at render.
 */
async function fetchPage(
  ws: string,
  filters: ExecutionFilters,
  pins: { pinnedFlowId?: string; pinnedNamespace?: string },
  signal: AbortSignal,
): Promise<ListPage> {
  // `customInstance` throws `ApiError` on a non-2xx answer; the check narrows the union.
  // FE-DATA-2 exception: a generated fetcher called directly, for parameters computed at fetch time (`since` from the time preset).
  const res = await workflowListExecutions(ws, toQueryParams(filters, Date.now(), pins), { signal });
  if (res.status !== 200) throw new Error("The executions could not be loaded.");
  return { rows: res.data.rows, total: res.data.total, workerEnabled: res.data.worker_enabled };
}

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/** `SUCCESS` → `Success`, the chip's own word. */
const stateWord = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();

/**
 * The executions list (W7b spec §4.1), at `executions` and, with
 * `pinnedFlowId`, as the flow detail's Executions tab.
 *
 * - Every filter, the child switch and the page live in the URL
 *   (`executionsListModel.ts`, FE-PAGE-2).
 * - By default it hides loop iterations and subflow and flow-trigger
 *   children (`root_only`); a `CREATE_NEW_EXECUTION` retry stays, labelled
 *   Retry. "Show child executions" adds the hidden ones.
 * - It polls every 5 s while a row on the page is not terminal, and only while
 *   the browser tab is visible: react-query skips an interval tick while the
 *   document is hidden (`refetchIntervalInBackground: false`).
 * - On the flow's tab it has no page frame (the flow page owns the `h1`), no
 *   flow column, and no flow or namespace filter.
 * - With `pinnedNamespace`, as a namespace's Executions tab (W7d spec §4.2),
 *   it has no page frame either. It asks for the namespace and its children
 *   (`namespace_prefix`), shows no namespace filter, and keeps the flow
 *   column and a flow filter offering the namespace's flows.
 */
export default function ExecutionsList({
  pinnedFlowId,
  pinnedNamespace,
}: {
  pinnedFlowId?: string;
  pinnedNamespace?: string;
}) {
  const ws = useParams().workspace!;
  const base = `/explore/${ws}/workflows`;
  const flowPinned = pinnedFlowId !== undefined;
  const namespacePinned = pinnedNamespace !== undefined;
  const embedded = flowPinned || namespacePinned;

  const [params, setParams] = useSearchParams();
  const filters = useMemo(
    () => readFilters(params, { pinned: flowPinned, pinnedNamespace: namespacePinned }),
    [params, flowPinned, namespacePinned],
  );
  // A picker pushes an entry, so Back undoes it; the time inputs replace theirs.
  const setFilters = (patch: Partial<ExecutionFilters>, replace = false) =>
    setParams((p) => withFilters(p, patch), { replace });

  // The flow list feeds the namespace and flow pickers; the flow's tab shows
  // neither, and a namespace's tab only the flow picker, over its own flows.
  const flowsQuery = useWorkflowListFlows(ws, undefined, {
    query: { queryKey: workflowsKeys.flows(ws), enabled: !flowPinned },
  });
  const flows = useMemo(() => {
    const all = flowsQuery.data?.status === 200 ? flowsQuery.data.data : [];
    return pinnedNamespace === undefined ? all : all.filter((f) => namespaceContains(pinnedNamespace, f.namespace));
  }, [flowsQuery.data, pinnedNamespace]);

  const query = useQuery({
    queryKey: workflowsKeys.executions(ws, {
      ...filters,
      pinnedFlowId: pinnedFlowId ?? null,
      // Only when pinned, so the other lists' keys are as they were.
      ...(namespacePinned ? { pinnedNamespace } : {}),
    }),
    queryFn: ({ signal }) => fetchPage(ws, filters, { pinnedFlowId, pinnedNamespace }, signal),
    // A new page or filter keeps the old rows on screen until the answer lands.
    placeholderData: keepPreviousData,
    refetchInterval: (q) => (q.state.data?.rows.some((r) => !isTerminalState(r.state)) ? LIST_POLL_MS : false),
    refetchIntervalInBackground: false,
  });
  const page = query.data;
  const now = useNow();

  const refresh = (
    <>
      {page ? (
        <span data-testid="executions-updated" className="text-sm text-muted-foreground tabular-nums">
          Updated {formatDuration(now - query.dataUpdatedAt)} ago
        </span>
      ) : null}
      <ListToolbarButton icon={RotateCw} label="Refresh" onClick={() => void query.refetch()} />
    </>
  );

  const body = (
    <>
      {/* KI-013. Not on the flow's tab: the flow page above it says so already. */}
      {page && !page.workerEnabled && !flowPinned ? <WorkerOffNotice className="mb-3" /> : null}
      <FilterBar
        showNamespace={!embedded}
        showFlow={!flowPinned}
        filters={filters}
        setFilters={setFilters}
        flows={flows}
        trailing={embedded ? refresh : null}
      />
      {renderContent()}
    </>
  );

  function renderContent(): ReactNode {
    // Error first (FE-STATE-1), while nothing has loaded. Once a page has
    // loaded, a failed refresh keeps it on screen and says so (spec §4.1).
    if (query.isError && !page) {
      return (
        <Alert variant="destructive">
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>The executions could not be loaded: {errorMessage(query.error)}. Try again.</span>
            <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
              <RotateCw aria-hidden />
              Retry
            </Button>
          </AlertDescription>
        </Alert>
      );
    }
    if (!page) return <ListSkeleton cols={flowPinned ? 6 : 7} />;

    const refreshError = query.isError ? (
      <Alert variant="destructive" className="mb-3">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>
            The executions could not be refreshed: {errorMessage(query.error)}. What is shown was read at{" "}
            {new Date(query.dataUpdatedAt).toLocaleTimeString()}.
          </span>
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    ) : null;

    if (page.rows.length === 0) {
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
            <p className="text-sm text-muted-foreground">No executions match the current filters.</p>
            <Button variant="outline" size="sm" onClick={() => setFilters(clearedFilters(filters))}>
              Clear filters
            </Button>
          </div>
        );
      } else if (namespacePinned) {
        empty = (
          <EmptyState
            variant="dashed"
            icon={History}
            title="No executions in this namespace yet"
            description="Each run of a flow in this namespace, or in a namespace below it, appears here."
          />
        );
      } else if (embedded) {
        empty = (
          <EmptyState
            variant="dashed"
            icon={History}
            title="This flow has not run yet"
            description="Each run of this flow appears here: from its Run button, a schedule, a webhook or another flow."
          />
        );
      } else {
        empty = (
          <EmptyState
            variant="dashed"
            icon={History}
            title="No executions yet"
            description="An execution appears here each time a flow runs: from its Run button, a schedule, a webhook or another flow."
            action={
              <Button asChild>
                <Link to={`${base}/flows`}>Go to flows</Link>
              </Button>
            }
          />
        );
      }
      return (
        <>
          {refreshError}
          {empty}
        </>
      );
    }

    return (
      <>
        {refreshError}
        <div className="mb-3" data-testid="executions-summary">
          <SummaryStrip items={summaryItems(page, filters, setFilters)} />
        </div>
        <div aria-busy={query.isPlaceholderData} className={query.isPlaceholderData ? "opacity-60" : undefined}>
          <Table data-testid="executions-table">
            <TableHeader>
              <TableRow>
                <TableHead>ID</TableHead>
                {flowPinned ? null : <TableHead>Flow</TableHead>}
                <TableHead>Namespace</TableHead>
                <TableHead>State</TableHead>
                <TableHead>Start</TableHead>
                <TableHead>Duration</TableHead>
                <TableHead>Trigger</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {page.rows.map((row) => (
                <ExecutionRow key={row.id} row={row} base={base} showFlow={!flowPinned} now={now} />
              ))}
            </TableBody>
          </Table>
        </div>
        <Pager page={page} filters={filters} setFilters={setFilters} />
      </>
    );
  }

  if (embedded) return <div className="grid gap-0">{body}</div>;
  return (
    <ListPageShell title="Executions" toolbar={refresh}>
      {body}
    </ListPageShell>
  );
}

/**
 * The strip: the whole match's size, then the states on this page. A state
 * count is a quick filter, as Kestra's state chip is: it narrows the list to
 * that state, and a second press clears it.
 */
function summaryItems(
  page: ListPage,
  filters: ExecutionFilters,
  setFilters: (patch: Partial<ExecutionFilters>) => void,
): SummaryStripItem[] {
  const counts = new Map<string, number>();
  for (const r of page.rows) counts.set(r.state, (counts.get(r.state) ?? 0) + 1);
  const order = (s: string) => {
    const i = (EXECUTION_STATE_NAMES as readonly string[]).indexOf(s);
    return i < 0 ? EXECUTION_STATE_NAMES.length : i;
  };
  const items: SummaryStripItem[] = [{ label: "executions", singular: "execution", value: page.total }];
  if (page.total > page.rows.length) items.push({ label: "on this page", value: page.rows.length });
  for (const [state, n] of [...counts].sort((a, b) => order(a[0]) - order(b[0]) || a[0].localeCompare(b[0]))) {
    const only = filters.states.length === 1 && filters.states[0] === state;
    items.push({
      label: state.toLowerCase(),
      value: n,
      active: only,
      onClick: () => setFilters({ states: only ? [] : [state] }),
    });
  }
  return items;
}

/**
 * One row of the list. Exported for the namespace Overview's recent executions
 * (W7d), which show the same columns.
 */
export function ExecutionRow({ row, base, showFlow, now }: { row: ExecutionListItemDto; base: string; showFlow: boolean; now: number }) {
  const start = executionStart(row);
  const duration = executionDuration(row, now);
  return (
    <TableRow>
      <TableCell>
        <Link
          className="font-mono text-xs hover:underline"
          to={`${base}/executions/${encodeURIComponent(row.id)}`}
          title={row.id}
        >
          {row.id.slice(0, 8)}
        </Link>
      </TableCell>
      {showFlow ? (
        <TableCell>
          {row.flow_id ? (
            <Link className="font-medium hover:underline" to={`${base}/flows/${encodeURIComponent(row.flow_id)}/edit`}>
              {row.flow_key}
            </Link>
          ) : (
            // A flow deleted for good is `flow_id: null`; its name is still what ran.
            row.flow_key
          )}
        </TableCell>
      ) : null}
      <TableCell className="text-muted-foreground">{row.namespace}</TableCell>
      <TableCell>
        <StateChip state={row.state} kind="execution" />
      </TableCell>
      <TableCell className="text-muted-foreground">
        <span title={startTitle(start)}>{start.kind === "created" ? "Not started" : timeAgo(start.iso)}</span>
      </TableCell>
      <TableCell className="tabular-nums">
        {duration === null ? (
          <span className="text-muted-foreground">—</span>
        ) : duration.estimated ? (
          <span title={ESTIMATED_DURATION_TITLE}>
            ≈ {formatDuration(duration.ms)}
            <span className="sr-only"> (estimated)</span>
          </span>
        ) : (
          formatDuration(duration.ms)
        )}
      </TableCell>
      <TableCell>
        <TriggerLabel row={row} />
      </TableCell>
    </TableRow>
  );
}

function Pager({
  page,
  filters,
  setFilters,
}: {
  page: ListPage;
  filters: ExecutionFilters;
  setFilters: (patch: Partial<ExecutionFilters>) => void;
}) {
  const pages = pageCount(page.total);
  if (pages <= 1 && filters.page <= 1) return null;
  const first = (filters.page - 1) * PAGE_SIZE + 1;
  const last = first + page.rows.length - 1;
  return (
    <div className="mt-3 text-sm text-muted-foreground">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="tabular-nums">{`Showing ${first}–${last} of ${page.total}`}</span>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={filters.page <= 1}
            onClick={() => setFilters({ page: filters.page - 1 })}
          >
            <ChevronLeft aria-hidden />
            Previous
          </Button>
          <span className="tabular-nums">{`Page ${filters.page} of ${pages}`}</span>
          <Button
            variant="outline"
            size="sm"
            disabled={filters.page >= pages}
            onClick={() => setFilters({ page: filters.page + 1 })}
          >
            Next
            <ChevronRight aria-hidden />
          </Button>
        </div>
      </div>
    </div>
  );
}

/** A UTC instant as a `datetime-local` value, in the viewer's zone. */
const toLocalInput = (iso: string) => (iso ? dayjs(iso).format("YYYY-MM-DDTHH:mm") : "");
/** A `datetime-local` value, read in the viewer's zone, as a UTC instant. */
const fromLocalInput = (v: string) => {
  const d = v ? new Date(v) : null;
  return d && !Number.isNaN(d.getTime()) ? d.toISOString() : "";
};

/**
 * The filter row, Kestra's executions filters on egret's controls: state
 * (several), namespace with its child namespaces, flow, time window, and the
 * child switch.
 */
function FilterBar({
  showNamespace,
  showFlow,
  filters,
  setFilters,
  flows,
  trailing,
}: {
  /** False on a flow's or a namespace's own tab. */
  showNamespace: boolean;
  /** False on a flow's own tab. */
  showFlow: boolean;
  filters: ExecutionFilters;
  setFilters: (patch: Partial<ExecutionFilters>, replace?: boolean) => void;
  flows: FlowListItemDto[];
  trailing: ReactNode;
}) {
  const id = useId();
  const namespaceOptions = useMemo(() => {
    const known = knownNamespaces(flows);
    // A shared link can name a namespace no flow lives in any more; it stays selectable.
    if (filters.namespace && !known.includes(filters.namespace)) known.push(filters.namespace);
    return known.map((ns) => ({ value: ns, label: ns }));
  }, [flows, filters.namespace]);
  const flowOptions = useMemo(() => {
    const opts = flows
      .map((f) => ({ value: f.id, label: `${f.namespace}.${f.flow_id}` }))
      .sort((a, b) => a.label.localeCompare(b.label));
    if (filters.flow && !opts.some((o) => o.value === filters.flow)) opts.push({ value: filters.flow, label: filters.flow });
    return opts;
  }, [flows, filters.flow]);

  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      <StatePicker states={filters.states} onChange={(states) => setFilters({ states })} />
      {showNamespace ? (
        <>
          <FacetSelect
            label="Namespace"
            value={filters.namespace || FACET_ALL}
            onChange={(v) => setFilters({ namespace: v === FACET_ALL ? "" : v, subNamespaces: v === FACET_ALL ? false : filters.subNamespaces })}
            options={namespaceOptions}
          />
          {filters.namespace ? (
            <div className="flex items-center gap-1.5">
              <Checkbox
                id={`${id}-subns`}
                checked={filters.subNamespaces}
                onCheckedChange={(c) => setFilters({ subNamespaces: c === true })}
              />
              <Label htmlFor={`${id}-subns`} className="text-sm font-normal">
                Include child namespaces
              </Label>
            </div>
          ) : null}
        </>
      ) : null}
      {showFlow ? (
        <FacetSelect
          label="Flow"
          value={filters.flow || FACET_ALL}
          onChange={(v) => setFilters({ flow: v === FACET_ALL ? "" : v })}
          options={flowOptions}
        />
      ) : null}
      <FacetSelect
        label="Time"
        value={filters.time || FACET_ALL}
        onChange={(v) => setFilters({ time: v === FACET_ALL ? "" : (v as ExecutionFilters["time"]), since: "", until: "" })}
        options={[...TIME_PRESETS.map((p) => ({ value: p.value, label: p.label })), { value: "custom", label: "Custom" }]}
      />
      {filters.time === "custom" ? (
        <>
          <div className="flex items-center gap-1.5">
            <Label htmlFor={`${id}-since`} className="text-sm font-normal">
              From
            </Label>
            <Input
              id={`${id}-since`}
              type="datetime-local"
              className="h-9 w-auto"
              value={toLocalInput(filters.since)}
              onChange={(e) => setFilters({ since: fromLocalInput(e.target.value), until: filters.until, time: "custom" }, true)}
            />
          </div>
          <div className="flex items-center gap-1.5">
            <Label htmlFor={`${id}-until`} className="text-sm font-normal">
              To
            </Label>
            <Input
              id={`${id}-until`}
              type="datetime-local"
              className="h-9 w-auto"
              value={toLocalInput(filters.until)}
              onChange={(e) => setFilters({ until: fromLocalInput(e.target.value), since: filters.since, time: "custom" }, true)}
            />
          </div>
        </>
      ) : null}
      <div className="flex items-center gap-2">
        <Switch
          id={`${id}-child`}
          checked={filters.showChildren}
          onCheckedChange={(c) => setFilters({ showChildren: c })}
        />
        <Label htmlFor={`${id}-child`} className="text-sm font-normal">
          Show child executions
        </Label>
      </div>
      {trailing ? <div className="ml-auto flex items-center gap-2">{trailing}</div> : null}
    </div>
  );
}

/** Several states at once, sent as one comma list. The menu stays open while picking. */
function StatePicker({ states, onChange }: { states: string[]; onChange: (states: string[]) => void }) {
  const summary =
    states.length === 0 ? "All" : states.length <= 2 ? states.map(stateWord).join(", ") : `${states.length} selected`;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" className="h-9">
          <span className="text-muted-foreground">State:</span>
          {summary}
          <ChevronDown aria-hidden className="opacity-50" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-80 overflow-y-auto">
        {states.length > 0 ? (
          <>
            <DropdownMenuItem onSelect={() => onChange([])}>Show all states</DropdownMenuItem>
            <DropdownMenuSeparator />
          </>
        ) : null}
        {EXECUTION_STATE_NAMES.map((s) => (
          <DropdownMenuCheckboxItem
            key={s}
            checked={states.includes(s)}
            onSelect={(e) => e.preventDefault()}
            onCheckedChange={(on) => onChange(on ? [...states, s] : states.filter((x) => x !== s))}
          >
            <StateChip state={s} kind="execution" />
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
