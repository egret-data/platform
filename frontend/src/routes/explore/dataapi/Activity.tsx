import { type ReactNode, useRef, useState } from "react";
import { AlertTriangle, OctagonAlert } from "lucide-react";
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
import { cn } from "@/lib/utils";
import ActivityCallList, { type ActivityCallListHandle } from "./ActivityCallList";
import { Caveat } from "./detail/Caveat";
import { Forbidden, isForbidden, refusalReason } from "@/routes/Forbidden";

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

  // A one-way COMMAND to the call list, not a filter lifted up here. The four
  // filters stay `useState` inside `ActivityCallList` on purpose — that is what
  // makes them survive a range switch — and "show me this endpoint's calls" is
  // an event, not a second copy of one of them. Lifting `endpointId` would put
  // one of the four somewhere else than its three siblings and invite the next
  // reader to "finish the job" by lifting the rest, which is the shipped bug
  // this list was hoisted out of `ListStates` to fix.
  const callList = useRef<ActivityCallListHandle>(null);

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

  // The server refused the page's own read: the shared refusal, not a red
  // box with "forbidden" under a title bar that offers actions (KI-231).
  if (isForbidden(summaryQuery.error)) {
    return <Forbidden reason={refusalReason(summaryQuery.error, "Viewing Data API activity needs the data_api_endpoint:read permission in this workspace.")} />;
  }

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
              {summary.endpoints.length > 0 && (
                <EndpointTable
                  rows={summary.endpoints}
                  onShowCalls={(id) => callList.current?.showCallsFor(id)}
                />
              )}
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
        <ActivityCallList ref={callList} ws={ws} range={range} rangeIsEmpty={rangeIsEmpty} />
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

/**
 * The share of calls that returned `>= 400` at which an endpoint is called
 * failing rather than merely noisy.
 *
 * Where the number comes from, since the data alone cannot supply it. The
 * seeded workspace's endpoints sit at 0.0% and at 18.8%/20.0% with nothing
 * between, so the distribution fixes only that the line lies somewhere in that
 * gap — any cut inside it separates the same two groups, which means picking
 * the cut from THIS data would be fitting a threshold to one week of one
 * workspace. It is taken from the error-budget convention instead: a 99%
 * availability target spends a whole month's budget in about seven hours at
 * 5%, which is why 5% is the usual "this is an incident" line. It also sits far
 * enough above routine 4xx traffic (see `errorRateTier`) that a healthy API
 * does not trip it.
 *
 * No minimum-sample rule guards it: 1 error out of 20 calls is 5% and is
 * marked. That is deliberate — the `Calls` column sits immediately to the left
 * of the rate, so the denominator is on screen to be checked, which is the
 * same reason the backend sends raw counts instead of rates at all.
 */
export const ERROR_RATE_ALERT = 0.05;

export type ErrorRateTier = "unmeasured" | "none" | "elevated" | "failing";

/**
 * Three states, and the middle one earns its place by being FEW rather than
 * MANY. Nothing more than that.
 *
 * An earlier draft of this comment claimed the third tier was forced by
 * `errors` counting 4xx as well as 5xx
 * (`count(*) FILTER (WHERE al.status_code >= 400)`,
 * `crates/egret-dataapi/src/activity.rs:207`). The premise is true and the
 * conclusion does not follow, so it is recorded here as a wrong turn rather
 * than deleted: **a threshold on a rate cannot separate 4xx from 5xx.** One 500
 * in 1000 calls is 0.1% and lands in `elevated`; six hundred 404s from one
 * misconfigured consumer is 60% and lands in `failing`. Whatever this ladder
 * separates, it is not client mistakes from server faults.
 *
 * What it does separate is a trickle from a flood, which is worth three states
 * because the operator's next action differs across all three: `none` is the
 * endpoint to skip, `elevated` is the one to keep an eye on, `failing` is the
 * one to open now. Two states would force the trickle to be either invisible
 * or alarming, and both are wrong.
 *
 * Separating 4xx from 5xx is a real and different want, and it needs data this
 * page does not have: `ActivityRow` carries one `errors` count and no 5xx
 * breakdown. Doing it properly means a second `count(*) FILTER (WHERE
 * al.status_code >= 500)` in that query, a field on the DTO, and a decision
 * about which of the two the rate column reports — not a different threshold
 * here.
 */
export function errorRateTier(errors: number, calls: number): ErrorRateTier {
  if (calls === 0) return "unmeasured";
  if (errors === 0) return "none";
  return errors / calls >= ERROR_RATE_ALERT ? "failing" : "elevated";
}

/**
 * The ink each tier is painted in. Exported because
 * `tests/e2e/activity-error-rate-severity.spec.ts` measures what these actually
 * PAINT, in a real browser, over the surface they sit on — jsdom resolves no
 * `oklch()` and would return the same answer with the tokens swapped
 * (`docs/en/ui-conventions.md` §10).
 *
 * `--accent` is not among them: it is reserved for hover/focus, and a reading
 * permanently painted in the hover colour reads as permanently hovered.
 *
 * **These four are ink on the UNWASHED card, and they stay that way** — this
 * is not the `bg-<token>/15 text-<token>` chip that `chipTone.ts` had to flatten
 * to `--foreground`, and the same treatment here would be a regression, not a
 * fix. Measured on the same instrument and the same run as that table:
 * `text-success` 4.54 light / 7.57 dark, `text-destructive` 4.81 / 6.09,
 * `text-foreground` 18.03 / 17.24, `text-muted-foreground` 6.34 / 6.89 — every
 * tier clears WCAG 1.4.3's 4.5:1 already, because nothing is washing the
 * background out from under them. It is the /15 wash that costs `--success` its
 * pass (4.54 → 3.72), and there is no wash here.
 *
 * Flattening all four to `--foreground` would also destroy what this column is
 * FOR: four tiers in one ink are one tier. The chip can afford to lose its hue
 * because a word or a status code sits inside it; a rate cell is a bare number,
 * and severity is the only thing the ink is saying. It is not saying it alone —
 * form and two icons and an `sr-only` word carry it without colour — but
 * removing the hue removes an ordering rather than a decoration.
 *
 * `elevated` is NOT amber ink, and that is a measurement rather than a
 * preference. `--highlight` is a FILL token — `oklch(0.72 …)` in light,
 * `oklch(0.78 …)` in dark, and its own comment in `index.css` describes it as a
 * badge/marker colour with `--highlight-foreground` as the ink to put ON it.
 * Used as ink on the card it measures 2.56:1 in Chrome, against WCAG 1.4.3's
 * 4.5:1 — the browser spec above rejected that first draft, which every jsdom
 * class assertion had passed. There is no amber INK in this token set (the
 * `-foreground` half is near-black and fails in dark), so the middle tier keeps
 * the amber as a wash and a border and puts the NUMBER in body ink. Its
 * severity is carried by form, which is the channel that does not depend on a
 * hue being legible.
 */
export const ERROR_RATE_INK: Record<ErrorRateTier, string> = {
  unmeasured: "text-muted-foreground",
  none: "text-success",
  elevated: "text-foreground",
  failing: "text-destructive",
};

/**
 * The FORM half, which is the half that survives greyscale, a projector and
 * every kind of colour blindness: nothing, a soft outline, a full-strength
 * outline. Read down the column it is an ordering even with every colour
 * removed.
 *
 * NEITHER marked tier is filled, and that too came back from the browser rather
 * than from taste. `bg-destructive/15 text-destructive` — then the tinted-chip
 * idiom of `Clients.tsx`, of `ActivityCallList`'s own status column and of a
 * dozen other places, since retired in Data API in favour of `--foreground` ink
 * on the same tint (`chipTone.ts`) — measures 3.68:1 in light mode: the wash
 * lifts the background out from under an ink that clears 4.81:1 on the bare
 * card. The
 * solid alternative (`bg-destructive text-destructive-foreground`, what
 * `Button variant="destructive"` ships) inverts the problem and measures 2.83:1
 * in DARK. Ink on the unwashed card is the only treatment that clears WCAG
 * 1.4.3 in both themes, so the fill is spent on the border instead, where no
 * text sits on it. The severity ordering is carried by border strength, by ink,
 * and by two different icons — see `ERROR_RATE_ICON`.
 */
export const ERROR_RATE_PILL: Record<ErrorRateTier, string> = {
  unmeasured: "",
  none: "",
  elevated: "border border-highlight/50",
  failing: "border border-destructive font-semibold",
};

/**
 * Two different icons, not one icon in two colours. This is the only channel
 * that separates `elevated` from `failing` for a reader who has neither colour
 * nor the `sr-only` word — a greyscale print, a failing display, a colour-blind
 * reader glancing at the column. A triangle warns; an octagon is the shape
 * every road on earth uses for STOP.
 */
const ERROR_RATE_ICON: Record<"elevated" | "failing", typeof AlertTriangle> = {
  elevated: AlertTriangle,
  failing: OctagonAlert,
};

/** And the WORD, which is the only half that reaches a screen reader at all.
 * Rendered `sr-only`: the number is already on screen and repeating "failing"
 * in ink beside it would cost a column's width on every row. */
/**
 * The short form, shown VISIBLY beside the figure in a tile.
 *
 * A tile has no pill — a `text-xs` outline inside a `text-2xl` figure is a
 * badge pretending to be a headline — and `ERROR_RATE_INK.elevated` is
 * `text-foreground`, the tile's own ink. So an elevated tile rendered like a
 * cell would have carried exactly ONE channel: a small triangle. Review caught
 * that; the two tile tests only covered `failing` and `none`, where the ink
 * does differ, so nothing here would have.
 *
 * The word is the second channel, and at tile scale it fits where it would not
 * in a table column. It replaces the `sr-only` span rather than doubling it: a
 * screen reader must not read "Elevated" twice.
 */
const ERROR_RATE_LABEL: Record<"elevated" | "failing", string> = {
  elevated: "Elevated",
  failing: "Failing",
};

const ERROR_RATE_WORD: Record<ErrorRateTier, string> = {
  unmeasured: "not measurable",
  none: "no errors",
  elevated: "elevated error rate",
  failing: "failing",
};

/**
 * An error rate, with its severity in three channels at once — colour, form,
 * and a word.
 *
 * `0.0%` is deliberately NOT a dash. `—` on this table already means "not
 * configured / nothing to divide" (the `Cache-hit rate` column), so an
 * empty-looking cell is a claim about the absence of a measurement. A zero
 * error rate is a measurement, and the best one there is; it gets the success
 * ink so that it reads as good rather than as blank.
 */
function ErrorRate({
  errors,
  calls,
  size,
}: {
  errors: number;
  calls: number;
  /** `tile` is a 2xl figure — a `text-xs` pill inside it would be a badge
   * pretending to be a headline, so the tile skips the pill and spends the
   * second channel on a visible word instead (see `ERROR_RATE_LABEL`). */
  size: "cell" | "tile";
}) {
  const tier = errorRateTier(errors, calls);
  if (tier === "unmeasured") {
    return <span className={ERROR_RATE_INK.unmeasured}>{formatRate(errors, calls)}</span>;
  }
  const marked = tier === "elevated" || tier === "failing";
  const Icon = marked ? ERROR_RATE_ICON[tier] : null;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 tabular-nums",
        ERROR_RATE_INK[tier],
        size === "cell" && marked && "rounded px-1.5 py-0.5 text-xs font-medium",
        size === "cell" && ERROR_RATE_PILL[tier],
      )}
    >
      {Icon && (
        <Icon className={size === "tile" ? "h-5 w-5 shrink-0" : "h-3 w-3 shrink-0"} aria-hidden />
      )}
      <span>{formatRate(errors, calls)}</span>
      {size === "tile" && marked ? (
        <span className="text-xs font-medium">{ERROR_RATE_LABEL[tier]}</span>
      ) : (
        /* A comma, not an em dash: `—` is this table's word for "no
           measurement" and the sr-only text must not put one inside a cell
           whose whole point is that it HAS one. */
        <span className="sr-only">, {ERROR_RATE_WORD[tier]}</span>
      )}
    </span>
  );
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
      <Tile
        testId="tile-error-rate"
        label="Error rate"
        value={<ErrorRate errors={total.errors} calls={total.calls} size="tile" />}
      />
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
function EndpointTable({
  rows,
  onShowCalls,
}: {
  rows: ActivityRow[];
  /** Narrow the call list below to one endpoint's calls. The reason this table
   * takes a callback at all: naming the failing endpoint and then leaving the
   * reader to find that same name by hand in a dropdown further down the page
   * is the monitoring loop broken in the middle. */
  onShowCalls: (endpointId: string) => void;
}) {
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
              <EndpointRow
                key={r.endpoint_id}
                row={r}
                name={r.endpoint_name ?? r.endpoint_id!}
                onShowCalls={() => onShowCalls(r.endpoint_id!)}
              />
            ))}
            {/* No `onShowCalls`: this bucket is `endpoint_id: null`, and there
                is no id for the filter below to take. A control that looks like
                the ones above it but cannot do what they do is worse than the
                plain text it replaced. */}
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

function EndpointRow({
  row,
  name,
  muted,
  onShowCalls,
}: {
  row: ActivityRow;
  name: string;
  muted?: boolean;
  onShowCalls?: () => void;
}) {
  return (
    <TableRow>
      <TableCell className={muted ? "text-muted-foreground" : "font-medium"}>
        {onShowCalls ? (
          // A real `<button>`, which is `EngineMonitor.tsx`'s idiom for exactly
          // this job (its query-id cell), and not a `<td>` with `role="button"`
          // + `tabIndex` + a hand-rolled Enter/Space handler. The pattern this
          // repo has twice had to repair — `ChartTable`'s and `PivotTable`'s
          // sort headers — was a `<th onClick>` that a keyboard could not
          // reach; the fix both times was a real button, which gets focus,
          // Enter AND Space from the browser rather than from a handler that
          // can implement one key and forget the other.
          //
          // The whole row is deliberately NOT the target: four of its five
          // cells are numbers a reader may want to select and copy, and a
          // row-wide click target eats that.
          <button
            type="button"
            className="text-left hover:underline focus-visible:underline focus-visible:outline-none"
            // Names the endpoint, not just "Show calls": a table of eight rows
            // otherwise offers eight controls with one accessible name. The
            // visible text is contained in it, so the label still matches what
            // a speech-input user would say (WCAG 2.5.3).
            aria-label={`Show calls for ${name}`}
            onClick={onShowCalls}
          >
            {name}
          </button>
        ) : (
          name
        )}
      </TableCell>
      <TableCell className="text-right tabular-nums">{row.calls}</TableCell>
      <TableCell className="text-right tabular-nums">
        <ErrorRate errors={row.errors} calls={row.calls} size="cell" />
      </TableCell>
      <TableCell className="text-right tabular-nums">{formatLatency(row.p50_ms, row.p95_ms)}</TableCell>
      <TableCell className="text-right tabular-nums">{formatRate(row.cache_hits, row.cacheable_calls)}</TableCell>
    </TableRow>
  );
}
