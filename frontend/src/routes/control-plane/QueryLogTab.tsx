import { useId, useMemo, useState, type ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import dayjs from "dayjs";
import { Loader2, RotateCw, ScrollText } from "lucide-react";
import { toast } from "sonner";

import { ApiError } from "@/api/client";
import type { Engine } from "@/api/controlPlane";
import {
  controlPlaneEnginesListQueryLog,
  getControlPlaneEnginesQueryLogStatusQueryKey,
  useControlPlaneEnginesGetQueryLogEntry,
  useControlPlaneEnginesQueryLogStatus,
  useControlPlaneEnginesSetupQueryLog,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import type {
  ControlPlaneEnginesListQueryLogParams,
  QueryLogEntryDto,
  QueryLogStatusDto,
} from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListStates } from "@/components/explore/ListStates";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DateTimePicker } from "@/components/ui/date-time-picker";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge, type StatusTone } from "@/components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { WarningCallout, useCan } from "./externalEngine";
import { queryLogListKey } from "./queryKeys";

/** What an entry shows in place of text the server withheld (no `engine:audit`). */
export const HIDDEN_TEXT = "Hidden: needs engine:audit";

export const NO_EVENTS_SENTENCE = "No events yet. Events reach the log about a minute after a statement runs.";

export const NOT_SET_UP_SENTENCE =
  "The query log is not set up on this engine: its audit plugin or its log table is missing.";

export const SETUP_NEEDS_PERMISSION = "Setting it up needs the engine:update permission.";

const PAGE_SIZE = 50;

/** The time-range presets; `from` holds one of these tokens, or an RFC 3339 instant for a custom range. */
const PRESETS = {
  "1h": { label: "Last hour", ms: 60 * 60 * 1000 },
  "24h": { label: "Last 24 hours", ms: 24 * 60 * 60 * 1000 },
  "7d": { label: "Last 7 days", ms: 7 * 24 * 60 * 60 * 1000 },
  "30d": { label: "Last 30 days", ms: 30 * 24 * 60 * 60 * 1000 },
} as const;
type Preset = keyof typeof PRESETS;
const DEFAULT_PRESET: Preset = "24h";

function isPreset(v: string | null): v is Preset {
  return v !== null && Object.hasOwn(PRESETS, v);
}

const STATE_LABEL = { all: "All", ok: "Succeeded", error: "Failed" } as const;
type StateFilter = keyof typeof STATE_LABEL;

function isStateFilter(v: string | null): v is StateFilter {
  return v !== null && Object.hasOwn(STATE_LABEL, v);
}

/** The URL's filter params (`?from=&to=&user=&state=`), as written there. */
interface Filters {
  from: string | null;
  to: string | null;
  user: string | null;
  state: StateFilter;
}

const FILTER_PARAMS = ["from", "to", "user", "state"] as const;

function readFilters(sp: URLSearchParams): Filters {
  const state = sp.get("state");
  return {
    from: sp.get("from"),
    to: sp.get("to"),
    user: sp.get("user"),
    state: isStateFilter(state) ? state : "all",
  };
}

/** Which range the picker shows: a preset token, else custom when either end is an instant. */
function rangeOf(f: Filters): Preset | "custom" {
  if (isPreset(f.from) && !f.to) return f.from;
  if (f.from || f.to) return "custom";
  return DEFAULT_PRESET;
}

/** The request's parameters. A preset is resolved against `now`, fixed while the filters stay the same. */
function requestParams(f: Filters, now: number): ControlPlaneEnginesListQueryLogParams {
  const range = rangeOf(f);
  const params: ControlPlaneEnginesListQueryLogParams = { limit: PAGE_SIZE };
  if (range === "custom") {
    if (f.from) params.from = f.from;
    if (f.to) params.to = f.to;
  } else {
    params.from = new Date(now - PRESETS[range].ms).toISOString();
    params.to = new Date(now).toISOString();
  }
  if (f.user) params.user = f.user;
  if (f.state !== "all") params.state = f.state;
  return params;
}

function parseInstant(v: string | null): Date | null {
  if (!v) return null;
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? null : d;
}

function localTime(ts: string): string {
  const d = dayjs(ts);
  return d.isValid() ? d.format("YYYY-MM-DD HH:mm:ss") : ts;
}

function utcInstant(ts: string): string {
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toISOString();
}

function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return "—";
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} min ${total % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

function formatCount(n: number | null | undefined): string {
  return n === null || n === undefined ? "—" : n.toLocaleString("en-US");
}

function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = n / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v.toFixed(1)} ${units[u]}`;
}

/** The person the statement ran for, then the session user. */
function userOf(e: QueryLogEntryDto): string {
  return e.authorizedUser || e.user || "—";
}

/** The session user, only when it is not the person shown as User (a delegated query). */
function connectedAs(e: QueryLogEntryDto): string | null {
  return e.authorizedUser && e.user && e.user !== e.authorizedUser ? e.user : null;
}

/**
 * The server drops `statement` and `errorMessage` together for a caller
 * without `engine:audit` (spec, "Decided on 2026-11-11" 1). A statement is
 * always there for one who has it, so an absent statement is the sign the
 * text was withheld; `errorMessage` alone is also absent on every success.
 */
function textWithheld(e: QueryLogEntryDto): boolean {
  return e.statement === undefined;
}

function stateBadge(state: string | null | undefined) {
  if (!state) return <span className="text-muted-foreground">—</span>;
  const [tone, word]: [StatusTone, string] =
    state === "ERR" ? ["destructive", "Failed"] : state === "EOF" || state === "OK" ? ["success", "Succeeded"] : ["neutral", state];
  return (
    <StatusBadge tone={tone} title={state}>
      {word}
    </StatusBadge>
  );
}

/** The statement's first non-blank line, with an ellipsis when more follows. */
function firstLine(statement: string, truncated: boolean): string {
  const lines = statement.split("\n");
  const idx = lines.findIndex((l) => l.trim() !== "");
  if (idx < 0) return "";
  const more = truncated || lines.slice(idx + 1).some((l) => l.trim() !== "");
  return `${lines[idx].trim()}${more ? " …" : ""}`;
}

function isNotReady(error: unknown): boolean {
  return error instanceof ApiError && error.status === 409 && error.code === "query_log_not_ready";
}

/**
 * Engine detail → **Query log** (provisioned StarRocks engines only): the
 * engine's own audit table, read through egret (spec
 * 2026-11-09-starrocks-audit-platform-design, section 6). A status line from
 * `/query-log/status`, the filters kept in the URL, the statements newest
 * first with "Load more", and a side panel with one statement whole.
 */
export function QueryLogTab({ engine }: { engine: Pick<Engine, "id" | "securityUpgradeNeeded"> }) {
  const [sp, setSp] = useSearchParams();
  const filters = readFilters(sp);
  const filtered = FILTER_PARAMS.some((k) => sp.has(k));
  const selected = sp.get("entry");

  const update = (patch: Record<string, string | null>) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        for (const [k, v] of Object.entries(patch)) {
          if (v === null || v === "") n.delete(k);
          else n.set(k, v);
        }
        return n;
      },
      { replace: true },
    );

  const clearFilters = () => update(Object.fromEntries(FILTER_PARAMS.map((k) => [k, null])));

  return (
    <div className="space-y-4">
      <QueryLogStatusLine engine={engine} />
      <QueryLogFilters filters={filters} update={update} />
      <QueryLogTable
        engineId={engine.id}
        filters={filters}
        filtered={filtered}
        onClear={clearFilters}
        onOpen={(queryId) => update({ entry: queryId })}
      />
      <QueryLogEntryPanel engineId={engine.id} queryId={selected} onClose={() => update({ entry: null })} />
    </div>
  );
}

function QueryLogStatusLine({ engine }: { engine: Pick<Engine, "id" | "securityUpgradeNeeded"> }) {
  const status = useControlPlaneEnginesQueryLogStatus<QueryLogStatusDto, ApiError>(engine.id, {
    query: { select: (res) => res.data as QueryLogStatusDto, retry: false },
  });

  if (status.isError) {
    return (
      <Alert variant="destructive" data-testid="query-log-status">
        <AlertDescription className="flex flex-wrap items-center gap-2">
          {status.error.message || "Could not read the query log's status."}
          <RetryButton onRetry={() => void status.refetch()} busy={status.isFetching} />
        </AlertDescription>
      </Alert>
    );
  }
  if (status.isPending) return <Skeleton className="h-5 w-80" data-testid="query-log-status-loading" />;

  const s = status.data;
  switch (s.state) {
    case "ready":
      return (
        <p className="text-sm text-muted-foreground" data-testid="query-log-status">
          {s.lastEventAt ? (
            <>
              Kept {s.retentionDays} days. Latest event:{" "}
              <time dateTime={s.lastEventAt} title={utcInstant(s.lastEventAt)} className="tabular-nums">
                {localTime(s.lastEventAt)}
              </time>
              .
            </>
          ) : (
            NO_EVENTS_SENTENCE
          )}
        </p>
      );
    case "notSetUp":
      return <NotSetUpCallout engineId={engine.id} />;
    case "pluginNotShipped":
      return (
        <WarningCallout testId="query-log-status">
          This engine's image has no audit plugin.{" "}
          {engine.securityUpgradeNeeded ? (
            <>
              <strong>Upgrade security</strong>, in the security warning on this page, moves it to egret's image,
              and the Query log starts after that.
            </>
          ) : (
            <>The Query log starts once the engine runs egret's image.</>
          )}
        </WarningCallout>
      );
    case "unreachable":
    default:
      return (
        <Alert variant="destructive" data-testid="query-log-status">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            {s.message || "The engine did not answer."}
            <RetryButton onRetry={() => void status.refetch()} busy={status.isFetching} />
          </AlertDescription>
        </Alert>
      );
  }
}

function RetryButton({ onRetry, busy }: { onRetry: () => void; busy: boolean }) {
  return (
    <Button type="button" variant="outline" size="sm" onClick={onRetry} disabled={busy}>
      {busy ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : <RotateCw className="h-4 w-4" aria-hidden />}
      Retry
    </Button>
  );
}

function NotSetUpCallout({ engineId }: { engineId: string }) {
  const queryClient = useQueryClient();
  const canUpdate = useCan("engine:update");
  const setup = useControlPlaneEnginesSetupQueryLog<ApiError>({
    mutation: {
      onSuccess: (res) => {
        // The answer carries no `lastEventAt`: `/status` is read again for the line above.
        void queryClient.invalidateQueries({ queryKey: getControlPlaneEnginesQueryLogStatusQueryKey(engineId) });
        void queryClient.invalidateQueries({ queryKey: queryLogListKey(engineId) });
        if (res.status === 200 && res.data.state === "pluginNotShipped") {
          toast.success("The log table is set up. This engine's image has no audit plugin yet.");
        } else {
          toast.success("Query log set up.");
        }
      },
      onError: (e) => toast.error(e.message || "Setting up the query log failed. Try again."),
    },
  });

  return (
    <WarningCallout testId="query-log-status">
      {NOT_SET_UP_SENTENCE}
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!canUpdate || setup.isPending}
          onClick={() => setup.mutate({ id: engineId })}
        >
          {setup.isPending && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          Set up query log
        </Button>
        {!canUpdate && <span>{SETUP_NEEDS_PERMISSION}</span>}
      </div>
    </WarningCallout>
  );
}

function QueryLogFilters({
  filters,
  update,
}: {
  filters: Filters;
  update: (patch: Record<string, string | null>) => void;
}) {
  const rangeId = useId();
  const fromId = useId();
  const toId = useId();
  const userId = useId();
  const stateId = useId();
  const range = rangeOf(filters);

  // The User field is typed here and written to the URL on Enter or on leaving
  // it; it follows the URL when the URL changes (Clear filters, Back).
  const [userText, setUserText] = useState(filters.user ?? "");
  const [userShown, setUserShown] = useState(filters.user);
  if (userShown !== filters.user) {
    setUserShown(filters.user);
    setUserText(filters.user ?? "");
  }
  const commitUser = () => {
    const v = userText.trim();
    if (v !== (filters.user ?? "")) update({ user: v || null });
  };

  const pickRange = (v: string) => {
    if (v === "custom") {
      const now = Date.now();
      update({
        from: new Date(now - PRESETS[DEFAULT_PRESET].ms).toISOString(),
        to: new Date(now).toISOString(),
      });
    } else {
      update({ from: v === DEFAULT_PRESET ? null : v, to: null });
    }
  };

  return (
    <div className="flex flex-wrap items-end gap-3">
      <div className="space-y-1.5">
        <Label htmlFor={rangeId}>Time range</Label>
        <Select value={range} onValueChange={pickRange}>
          <SelectTrigger id={rangeId} className="w-44">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {(Object.keys(PRESETS) as Preset[]).map((p) => (
              <SelectItem key={p} value={p}>
                {PRESETS[p].label}
              </SelectItem>
            ))}
            <SelectItem value="custom">Custom</SelectItem>
          </SelectContent>
        </Select>
      </div>
      {range === "custom" && (
        <>
          <div className="space-y-1.5">
            <Label htmlFor={fromId}>From</Label>
            <DateTimePicker
              id={fromId}
              value={parseInstant(filters.from)}
              onChange={(d) => update({ from: d ? d.toISOString() : null })}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={toId}>To</Label>
            <DateTimePicker
              id={toId}
              value={parseInstant(filters.to)}
              placeholder="Now"
              onChange={(d) => update({ to: d ? d.toISOString() : null })}
            />
          </div>
        </>
      )}
      <form
        className="space-y-1.5"
        onSubmit={(e) => {
          e.preventDefault();
          commitUser();
        }}
      >
        <Label htmlFor={userId}>User</Label>
        <Input
          id={userId}
          value={userText}
          placeholder="Any user"
          className="w-48"
          onChange={(e) => setUserText(e.target.value)}
          onBlur={commitUser}
        />
      </form>
      <div className="space-y-1.5">
        <Label htmlFor={stateId}>State</Label>
        <Select value={filters.state} onValueChange={(v) => update({ state: v === "all" ? null : v })}>
          <SelectTrigger id={stateId} className="w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {(Object.keys(STATE_LABEL) as StateFilter[]).map((s) => (
              <SelectItem key={s} value={s}>
                {STATE_LABEL[s]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </div>
  );
}

function QueryLogTable({
  engineId,
  filters,
  filtered,
  onClear,
  onOpen,
}: {
  engineId: string;
  filters: Filters;
  filtered: boolean;
  onClear: () => void;
  onOpen: (queryId: string) => void;
}) {
  // A page asks for one window. The first page resolves a preset range
  // against the moment it is fetched and hands its window on with the cursor,
  // so "Load more" pages through the same window (and a refetch, which starts
  // again at the first page, resolves it afresh for every page).
  type PageParam = { cursor: string; from?: string; to?: string } | undefined;
  const list = useInfiniteQuery({
    queryKey: queryLogListKey(engineId, {
      from: filters.from ?? undefined,
      to: filters.to ?? undefined,
      user: filters.user ?? undefined,
      state: filters.state,
    }),
    // FE-DATA-2 exception: the generated fetcher, because "Load more" carries a cursor across pages.
    queryFn: async ({ pageParam }: { pageParam: PageParam }) => {
      const params = requestParams(filters, Date.now());
      if (pageParam) {
        params.cursor = pageParam.cursor;
        params.from = pageParam.from;
        params.to = pageParam.to;
      }
      const res = await controlPlaneEnginesListQueryLog(engineId, params);
      // `apiFetch` throws on any non-2xx (mutator.ts); this narrows the union.
      if (res.status !== 200) throw new Error("Could not load the query log.");
      return { ...res.data, window: { from: params.from, to: params.to } };
    },
    initialPageParam: undefined as PageParam,
    getNextPageParam: (last): PageParam =>
      last.nextCursor ? { cursor: last.nextCursor, ...last.window } : undefined,
    retry: false,
  });

  const entries = useMemo(() => list.data?.pages.flatMap((p) => p.entries) ?? [], [list.data]);

  if (list.isError && isNotReady(list.error)) {
    return (
      <EmptyState
        variant="dashed"
        icon={ScrollText}
        title="The query log table does not exist yet"
        description="Set up the query log, then statements appear here about a minute after they run."
      />
    );
  }

  return (
    <ListStates
      isLoading={list.isPending}
      isError={list.isError}
      error={list.error}
      errorText="Could not load the query log."
      hasData={entries.length > 0}
      skeletonCols={8}
      empty={
        filtered ? (
          <EmptyState
            variant="dashed"
            icon={ScrollText}
            title="No statements match these filters"
            action={
              <Button type="button" variant="outline" size="sm" onClick={onClear}>
                Clear filters
              </Button>
            }
          />
        ) : (
          <EmptyState
            variant="dashed"
            icon={ScrollText}
            title="No statements in the last 24 hours"
            description="Events reach the log about a minute after a statement runs."
          />
        )
      }
    >
      <div className="space-y-3">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Time</TableHead>
              <TableHead>User</TableHead>
              <TableHead>Connected as</TableHead>
              <TableHead>State</TableHead>
              <TableHead>Command</TableHead>
              <TableHead className="text-right">Duration</TableHead>
              <TableHead className="text-right">Rows returned</TableHead>
              <TableHead>Statement</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {entries.map((e) => (
              <QueryLogRow key={e.queryId} entry={e} onOpen={() => onOpen(e.queryId)} />
            ))}
          </TableBody>
        </Table>
        <div className="flex items-center justify-between gap-2 text-sm text-muted-foreground">
          <span>
            Showing <span className="tabular-nums">{entries.length}</span>{" "}
            {entries.length === 1 ? "statement" : "statements"}
            {list.hasNextPage ? " so far" : ""}
          </span>
          {list.hasNextPage && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={list.isFetchingNextPage}
              onClick={() => void list.fetchNextPage()}
            >
              {list.isFetchingNextPage && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              Load more
            </Button>
          )}
        </div>
      </div>
    </ListStates>
  );
}

function QueryLogRow({ entry: e, onOpen }: { entry: QueryLogEntryDto; onOpen: () => void }) {
  return (
    <TableRow>
      <TableCell className="whitespace-nowrap">
        <button
          type="button"
          className="text-left tabular-nums underline-offset-4 hover:underline focus-visible:underline"
          title={utcInstant(e.timestamp)}
          onClick={onOpen}
        >
          {localTime(e.timestamp)}
        </button>
      </TableCell>
      <TableCell>{userOf(e)}</TableCell>
      <TableCell className="text-muted-foreground">{connectedAs(e) ?? ""}</TableCell>
      <TableCell>{stateBadge(e.state)}</TableCell>
      <TableCell>{e.command ?? "—"}</TableCell>
      <TableCell className="text-right tabular-nums">{formatMs(e.queryTimeMs)}</TableCell>
      <TableCell className="text-right tabular-nums">{formatCount(e.returnRows)}</TableCell>
      <TableCell className="max-w-md">
        {textWithheld(e) ? (
          <span className="text-muted-foreground italic">{HIDDEN_TEXT}</span>
        ) : (
          <span className="block truncate font-mono text-xs" title={e.statement ?? undefined}>
            {firstLine(e.statement ?? "", e.statementTruncated)}
          </span>
        )}
      </TableCell>
    </TableRow>
  );
}

function QueryLogEntryPanel({
  engineId,
  queryId,
  onClose,
}: {
  engineId: string;
  queryId: string | null;
  onClose: () => void;
}) {
  const detail = useControlPlaneEnginesGetQueryLogEntry<QueryLogEntryDto, ApiError>(engineId, queryId ?? "", {
    query: { enabled: queryId !== null, select: (res) => res.data as QueryLogEntryDto, retry: false },
  });

  return (
    <Sheet open={queryId !== null} onOpenChange={(open) => !open && onClose()}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>Statement</SheetTitle>
          <SheetDescription className="font-mono text-xs break-all">{queryId}</SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-auto">
          {detail.isError ? (
            <Alert variant="destructive">
              <AlertDescription>{detail.error.message || "Could not load this statement."}</AlertDescription>
            </Alert>
          ) : detail.isPending ? (
            <div className="space-y-2" data-testid="query-log-entry-loading">
              <Skeleton className="h-4 w-2/3" />
              <Skeleton className="h-4 w-1/2" />
              <Skeleton className="h-24 w-full" />
            </div>
          ) : (
            <EntryDetail entry={detail.data} />
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}

function EntryDetail({ entry: e }: { entry: QueryLogEntryDto }) {
  const withheld = textWithheld(e);
  const facts: [string, ReactNode][] = [
    ["Time", <span title={utcInstant(e.timestamp)}>{localTime(e.timestamp)}</span>],
    ["User", userOf(e)],
    ["Connected as", e.user ?? "—"],
    ["State", stateBadge(e.state)],
    ["Command", e.command ?? "—"],
    ["Catalog", e.catalog ?? "—"],
    ["Database", e.db ?? "—"],
    ["Client IP", e.clientIp ?? "—"],
    ["Query source", e.querySource ?? "—"],
    ["Duration", formatMs(e.queryTimeMs)],
    ["Rows scanned", formatCount(e.scanRows)],
    ["Bytes scanned", formatBytes(e.scanBytes)],
    ["Rows returned", formatCount(e.returnRows)],
    ["CPU time", e.cpuCostNs === null || e.cpuCostNs === undefined ? "—" : formatMs(Math.round(e.cpuCostNs / 1e6))],
    ["Memory", formatBytes(e.memCostBytes)],
  ];

  return (
    <div className="space-y-4">
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-sm">
        {facts.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{k}</dt>
            <dd className="tabular-nums break-all">{v}</dd>
          </div>
        ))}
      </dl>
      <section className="space-y-1.5" aria-label="Error">
        <h3 className="text-sm font-medium">Error</h3>
        {withheld ? (
          <p className="text-sm text-muted-foreground italic">{HIDDEN_TEXT}</p>
        ) : e.errorMessage ? (
          <pre className="rounded bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all" data-testid="query-log-error">
            {e.errorMessage}
          </pre>
        ) : (
          <p className="text-sm text-muted-foreground">None</p>
        )}
      </section>
      <section className="space-y-1.5" aria-label="Statement">
        <h3 className="text-sm font-medium">Statement</h3>
        {withheld ? (
          <p className="text-sm text-muted-foreground italic">{HIDDEN_TEXT}</p>
        ) : (
          <pre className="rounded bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all" data-testid="query-log-statement">
            {e.statement}
          </pre>
        )}
      </section>
    </div>
  );
}
