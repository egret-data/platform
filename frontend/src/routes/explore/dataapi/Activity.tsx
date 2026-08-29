import { type ReactNode, useState } from "react";
import { useParams } from "react-router-dom";

import { useDataApiActivitySummary } from "@/api/generated/data-api/data-api";
import type { ActivityRange, ActivityRow } from "@/api/generated/model";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import ActivityCallList from "./ActivityCallList";
import { Caveat } from "./detail/Caveat";

const RANGES: ActivityRange[] = ["1h", "24h", "7d", "30d"];

/** Range preset -> its span in days, for comparing against `retention_days`. */
const RANGE_DAYS: Record<ActivityRange, number> = {
  "1h": 1 / 24,
  "24h": 1,
  "7d": 7,
  "30d": 30,
};

/**
 * Data API -> Activity. What `dataapi.access_log` can tell an operator: is
 * the API healthy, and which endpoint isn't — plus the `request_id` lookup
 * support uses when a caller quotes one out of an error body.
 *
 * A TOP-LEVEL section (not `useEndpointDetail()`): it is not scoped to one
 * endpoint.
 *
 * Deliberately NO time-series chart (spec §4.8) — this page pulls in no
 * charting stack.
 *
 * The backend sends raw counts, not rates, specifically so a rate's
 * denominator is always checkable here rather than hidden in a percentage
 * nobody can verify. See `formatRate`/`formatLatency` below for the two
 * honesty rules this file exists to hold: `—` (never `0%`) for a
 * zero-over-zero ratio, and "across successful calls" on every p50/p95.
 */
export default function Activity() {
  const ws = useParams().workspace!;
  const [range, setRange] = useState<ActivityRange>("24h");

  const summaryQuery = useDataApiActivitySummary(ws, { range }, { query: { retry: false } });
  const summary = summaryQuery.data?.status === 200 ? summaryQuery.data.data : undefined;
  const retentionDays = summary?.retention_days ?? null;

  // Either signal means the range produced calls: the grand total and the
  // per-endpoint rows come out of one `GROUPING SETS` query, so they agree —
  // and requiring BOTH would blank a page that has rows to show the day one of
  // them is wrong.
  const hasCalls = summary != null && (summary.total.calls > 0 || summary.endpoints.length > 0);

  // NOT `!hasCalls`. This is "the summary has SETTLED and says the range
  // produced nothing", which is a different claim from "we have not measured
  // yet" — and it is the one the call list gates its filter row on, so a range
  // switch (which puts the summary back into `isLoading`) must not read as an
  // empty range for the frame it is in flight.
  const rangeIsEmpty = summary != null && !hasCalls;

  return (
    // The canonical Explore list-page scaffold — `p-4` + a single `h1` — comes
    // from `ListPageShell`. Activity is its own top-level sidebar item, not a
    // section nested under a `DataApiLayout` shell (that shell, which used to
    // own the page's `h1` and add `p-6`, was removed once Data API got its own
    // sidebar group), so the heading and the padding still belong to this route
    // — they just come from the shared shell now.
    //
    // That ownership claim is the whole of what the previous comment here
    // decided, and it SURVIVES this conversion rather than being overturned by
    // it: the question it answered was whether this route has a heading at all,
    // not how the heading is spelled. It was written in #511 (2026-08-17), one
    // day before `ListPageShell` existed (#518, 2026-08-18), so it cannot have
    // been a decision to decline the shell. `dataapi/Clients.tsx` carries the
    // same paragraph and already resolves it this way; this page is the copy
    // that was not migrated with it. The old comment also called itself "same
    // as Charts.tsx/Datasets.tsx", and both of those moved to the shell too, so
    // its own claim of parity had become false.
    <ListPageShell title="Activity">
      {/* The 1h/24h/7d/30d strip stays in the body rather than moving to the
          shell's `toolbar` slot. §1.2 classes it a view SWITCH, and the
          consequence of that is it is never GATED on `hasData` — which is a
          rule about when it renders, not about where. It is also not a toolbar
          control in shape: `RangePicker` is a two-row block, a button strip
          over a `<details>` retention caveat, and dropping that into the
          header's `flex flex-wrap items-center` group is a layout change no
          jsdom test can check (§10). Left where it renders today, so this
          commit changes the frame and nothing else. */}
      <div className="mb-4">
        <RangePicker range={range} onChange={setRange} retentionDays={retentionDays} />
      </div>

      <div className="space-y-6">
        {/* Four states, in `ListStates`' order — `isError` BEFORE `hasData`, and
            error may never fold into empty (§2.1/§2.2). A 500 must not render
            as "No calls in this range."

            `isLoading` also covers "settled with no payload": react-query clears
            `isLoading` on ANY settle, so a result nobody has would otherwise
            fall straight into the empty branch and assert something about the
            world from nothing. The `!summaryQuery.isError` guard keeps the error
            branch reachable — `ListStates` checks loading first.

            `empty={null}`: this region has no empty state of its own. The
            page's ONE empty state belongs to the call list below, which is
            mounted in every state of this query — a second sentence here is the
            defect this task exists to remove. */}
        <ListStates
          isLoading={summaryQuery.isLoading || (!summary && !summaryQuery.isError)}
          isError={summaryQuery.isError}
          // The generated hook types its error as the response SHAPE; the
          // mutator throws the real `ApiError`. `ListStates` takes `unknown`.
          error={summaryQuery.error}
          errorText="Could not load activity statistics."
          hasData={hasCalls}
          skeletonCols={4}
          empty={null}
        >
          {summary && (
            <>
              <Tiles total={summary.total} />
              {/* Mounted only when there is a breakdown to show, so the table
                  has no empty state to own. `hasCalls` is also true when the
                  grand total is positive and this array is somehow not, and a
                  header row over nothing is not a better answer than no table. */}
              {summary.endpoints.length > 0 && <EndpointTable rows={summary.endpoints} />}
            </>
          )}
        </ListStates>

        {/* OUTSIDE the branch above, and that placement is the whole point.
            `range` is in the summary's query key, so every range switch puts it
            back into `isLoading`; mounted inside those children, this list
            unmounted on each switch and its four filter `useState`s reset —
            a request id the operator pasted, gone on the way to a wider window.
            Out here it also fetches in parallel with the summary instead of
            queueing behind it. */}
        <ActivityCallList ws={ws} range={range} rangeIsEmpty={rangeIsEmpty} />
      </div>
    </ListPageShell>
  );
}

function RangePicker({
  range,
  onChange,
  retentionDays,
}: {
  range: ActivityRange;
  onChange: (r: ActivityRange) => void;
  retentionDays: number | null;
}) {
  return (
    <div data-testid="activity-range" className="space-y-1.5">
      <div className="flex items-center gap-2">
        {RANGES.map((r) => {
          // The SELECTED preset is never disabled, even when it exceeds
          // retention. `retention_days: 0` (retention off / not configured)
          // otherwise disables every button including the one currently in
          // effect, which reads as a broken control rather than as a bound —
          // and leaves the user unable to re-press the range they are looking
          // at. A range that IS selected is by definition reachable.
          const disabled = range !== r && retentionDays != null && RANGE_DAYS[r] > retentionDays;
          return (
            <Button
              key={r}
              type="button"
              size="sm"
              variant={range === r ? "default" : "outline"}
              disabled={disabled}
              title={disabled ? `Disabled — data older than the retention window is not kept` : undefined}
              onClick={() => onChange(r)}
            >
              {r}
            </Button>
          );
        })}
      </div>
      {retentionDays != null && (
        // §4.4: the consequence in the summary, the reason behind it. The
        // load-bearing half — how long data is kept — is on screen with nothing
        // clicked; what is collapsed is WHY presets are disabled. A full-width
        // paragraph under a four-button strip was the whole of it before.
        <Caveat
          className="text-xs"
          summary={`Data is kept for ${retentionDays} day${retentionDays === 1 ? "" : "s"}`}
        >
          Ranges longer than that are disabled above so a thinning tail is never mistaken for
          falling traffic.
        </Caveat>
      )}
    </div>
  );
}

/** `denominator === 0` renders `—`, never `0%` — zero of zero is not zero,
 * and `0%` would read as "broken" when the truth is "nothing to divide". */
function formatRate(numerator: number, denominator: number): string {
  if (denominator === 0) return "—";
  return `${((numerator / denominator) * 100).toFixed(1)}%`;
}

/** `p50_ms`/`p95_ms` are `null` when there was no successful-call population
 * to compute a percentile over — the absence of a measurement, not a zero
 * one. */
function formatLatency(p50Ms: number | null | undefined, p95Ms: number | null | undefined): string {
  if (p50Ms == null || p95Ms == null) return "—";
  return `${Math.round(p50Ms)} / ${Math.round(p95Ms)} ms`;
}

function Tiles({ total }: { total: ActivityRow }) {
  // Cache-hit rate gets its own binding (not folded into a shared call) so
  // the Task 9 mutation check has one obvious line to flip: swap this for
  // `total.cacheable_calls === 0 ? "0%" : formatRate(...)` and the
  // "renders a dash" test must fail.
  const cacheHitDisplay = formatRate(total.cache_hits, total.cacheable_calls);

  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      <Tile testId="tile-calls" label="Calls" value={total.calls.toLocaleString()} />
      <Tile testId="tile-error-rate" label="Error rate" value={formatRate(total.errors, total.calls)} />
      <Tile
        testId="tile-latency"
        label="Latency"
        value={formatLatency(total.p50_ms, total.p95_ms)}
        caption="p50 / p95, across successful calls"
      />
      <Tile testId="tile-cache-hit" label="Cache-hit rate" value={cacheHitDisplay} />
    </div>
  );
}

function Tile({
  testId,
  label,
  value,
  caption,
}: {
  testId: string;
  label: string;
  value: ReactNode;
  caption?: string;
}) {
  return (
    <div data-testid={testId} className="rounded-lg border p-3">
      <p className="text-xs uppercase tracking-wide text-muted-foreground">{label}</p>
      <p className="mt-1 text-2xl font-semibold tabular-nums">{value}</p>
      {caption && <p className="mt-0.5 text-xs text-muted-foreground">{caption}</p>}
    </div>
  );
}

/**
 * The per-endpoint breakdown. The final row is `endpoint_id: null` — every
 * request in range that resolved no endpoint, grouped into one bucket by the
 * same `GROUPING SETS` query that produces the grand total. `ActivityRow`
 * carries no `request_method`/`request_path` (only `ActivityCallDto`, the
 * call-list's per-call shape, does), so unlike the call list below, this
 * aggregate row can only be labelled, not itself identified by a single
 * request — it may summarize many different paths.
 */
function EndpointTable({ rows }: { rows: ActivityRow[] }) {
  const matched = rows.filter((r) => r.endpoint_id != null);
  const unmatched = rows.find((r) => r.endpoint_id == null);

  return (
    <div data-testid="activity-per-endpoint" className="space-y-1.5">
      <h2 className="text-sm font-semibold">Per endpoint</h2>
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Endpoint</TableHead>
              <TableHead className="text-right">Calls</TableHead>
              <TableHead className="text-right">Error rate</TableHead>
              <TableHead className="text-right">p50 / p95</TableHead>
              <TableHead className="text-right">Cache-hit rate</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {matched.map((r) => (
              <EndpointRow key={r.endpoint_id} row={r} name={r.endpoint_name ?? r.endpoint_id!} />
            ))}
            {unmatched && <EndpointRow key="unmatched" row={unmatched} name="No matching endpoint" muted />}
          </TableBody>
        </Table>
      </div>
      <Caveat className="text-xs" summary="Rows here can sum to less than the total above">
        Any request that matched no endpoint is grouped into the final "No matching endpoint" row
        rather than the per-endpoint count — the screen isn't inconsistent, those calls just never
        resolved to one.
      </Caveat>
    </div>
  );
}

function EndpointRow({ row, name, muted }: { row: ActivityRow; name: string; muted?: boolean }) {
  return (
    <TableRow>
      <TableCell className={muted ? "text-muted-foreground" : "font-medium"}>{name}</TableCell>
      <TableCell className="text-right tabular-nums">{row.calls}</TableCell>
      <TableCell className="text-right tabular-nums">{formatRate(row.errors, row.calls)}</TableCell>
      <TableCell className="text-right tabular-nums">{formatLatency(row.p50_ms, row.p95_ms)}</TableCell>
      <TableCell className="text-right tabular-nums">{formatRate(row.cache_hits, row.cacheable_calls)}</TableCell>
    </TableRow>
  );
}
