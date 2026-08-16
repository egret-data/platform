import { type ReactNode, useState } from "react";
import { useParams } from "react-router-dom";

import { useDataApiActivitySummary } from "@/api/generated/data-api/data-api";
import type { ActivityRange, ActivityRow } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
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

  const summaryErrorMessage =
    (summaryQuery.error as { message?: string } | null)?.message ??
    "Could not load activity statistics.";

  return (
    <div className="p-4">
      <div className="mb-4">
        <h1 className="text-lg font-semibold">Activity</h1>
      </div>

      <div className="mb-4">
        <RangePicker range={range} onChange={setRange} retentionDays={retentionDays} />
      </div>

      {/* Two failure modes, handled separately (resolution #3): a 500 must
          never render as "no calls yet". */}
      {summaryQuery.isError ? (
        <Alert variant="destructive">
          <AlertDescription>{summaryErrorMessage}</AlertDescription>
        </Alert>
      ) : summaryQuery.isLoading || !summary ? (
        <ListSkeleton cols={4} />
      ) : (
        <div className="space-y-6">
          <Tiles total={summary.total} />
          <EndpointTable rows={summary.endpoints} />
        </div>
      )}

      <div className="mt-8">
        <ActivityCallList ws={ws} range={range} />
      </div>
    </div>
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
    <div className="space-y-1">
      <div className="flex items-center gap-2">
        {RANGES.map((r) => {
          const disabled = retentionDays != null && RANGE_DAYS[r] > retentionDays;
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
        <p className="text-xs text-muted-foreground">
          Data is kept for {retentionDays} day{retentionDays === 1 ? "" : "s"}; ranges longer than
          that are disabled above so a thinning tail is never mistaken for falling traffic.
        </p>
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
    <div className="space-y-1.5">
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
            {matched.length === 0 && !unmatched && (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                  No calls in this range.
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </div>
      <p className="text-xs text-muted-foreground">
        Rows here can sum to less than the total above: any request that matched no endpoint is
        grouped into the final "No matching endpoint" row rather than the per-endpoint count — the
        screen isn't inconsistent, those calls just never resolved to one.
      </p>
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
