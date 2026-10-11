import { useId, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { ChevronDown, ChevronRight, RotateCcw, X } from "lucide-react";
import type { TaskRunDto } from "@/api/generated/model";
import { useElementSize } from "@/components/chart/useElementSize";
import { IconButton } from "@/components/ui/icon-button";
import { StateChip, executionStateTone, type StateTone } from "@/components/workflows/StateChip";
import { formatDuration } from "@/components/workflows/executionState";
import {
  axisTicks,
  markerReason,
  visibleRows,
  type GanttAttempt,
  type GanttModel,
  type GanttRow,
} from "@/components/workflows/executions/gantt/ganttModel";
import { ESTIMATED_DURATION_TITLE } from "@/components/workflows/executions/timing";
import { fullTimestamp } from "@/lib/relativeTime";
import { cn } from "@/lib/utils";

/**
 * THE GANTT, DRAWN (W7b spec §6). DOM elements, not a chart library, as
 * Kestra's `Gantt.vue` does, so every bar sits in a row a keyboard and a
 * screen reader can reach.
 *
 * - The rows are a `grid` (FE-A11Y-3): one tab stop, arrow keys between rows,
 *   Home and End, Enter or Space opens the run, Escape closes it; Right and
 *   Left expand and collapse a row with children.
 * - A bar's colour is never the only word on its state (FE-A11Y-4): the row
 *   carries the state chip (word and glyph), the attempt count, and a
 *   screen-reader sentence for the timeline cell.
 * - Colours are token classes; the hatch of an estimated bar is
 *   `var(--token)` in an inline style (FE-VISUAL-1, FE-VISUAL-2).
 */

/** The fill of a bar, per state tone. */
const TONE_FILL: Record<StateTone, string> = {
  muted: "bg-muted-foreground/50",
  primary: "bg-primary",
  success: "bg-success",
  highlight: "bg-highlight",
  destructive: "bg-destructive",
  neutral: "bg-muted-foreground",
};
/** The same tone as a CSS variable, for the hatch, which is a gradient and cannot be a class. */
const TONE_VAR: Record<StateTone, string> = {
  muted: "var(--muted-foreground)",
  primary: "var(--primary)",
  success: "var(--success)",
  highlight: "var(--highlight)",
  destructive: "var(--destructive)",
  neutral: "var(--muted-foreground)",
};

const hatch = (tone: StateTone): CSSProperties => ({
  backgroundImage: `repeating-linear-gradient(135deg, ${TONE_VAR[tone]} 0 3px, transparent 3px 6px)`,
  borderColor: TONE_VAR[tone],
});

/** Task, state, duration, timeline. The task column gives way first; the timeline keeps its room. */
const COLUMNS = "grid grid-cols-[minmax(9rem,16rem)_7.5rem_6.5rem_minmax(14rem,1fr)] items-center";

/** What a run's duration cell says: `3 s`, `3 s so far`, `≈ 3 s`, or `—` for a marker. */
function RowDuration({ row }: { row: GanttRow }) {
  if (!row.bar) return <span className="text-muted-foreground">—</span>;
  const text = formatDuration(row.bar.end - row.bar.start);
  if (row.estimated) {
    return (
      <span title={ESTIMATED_DURATION_TITLE} className="flex flex-col leading-tight">
        <span>≈ {text}</span>
        <span className="text-xs text-muted-foreground">estimated</span>
      </span>
    );
  }
  return <>{row.bar.open ? `${text} so far` : text}</>;
}

const stateWord = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();

/** The timeline cell in words, for a screen reader: where the bar is and what each attempt did. */
function describeTimeline(row: GanttRow, axisStart: number): string {
  if (row.marker || !row.bar) return markerReason(row.state);
  const from = formatDuration(row.bar.start - axisStart);
  const span = row.bar.open ? `from ${from}, still running` : `from ${from} to ${formatDuration(row.bar.end - axisStart)}`;
  const attempts =
    row.attempts.length > 1
      ? ` ${row.attempts
          .map((a, i) => `Attempt ${i + 1}: ${stateWord(a.state)}${a.end !== undefined ? `, ${formatDuration(a.end - a.start)}` : ", running"}.`)
          .join(" ")}`
      : "";
  return `${row.estimated ? "Estimated, " : ""}${span}.${attempts}`;
}

function TimelineCell({ row, axis, ticks }: { row: GanttRow; axis: GanttModel["axis"]; ticks: { at: number }[] }) {
  const span = axis.end - axis.start;
  const pct = (t: number) => ((t - axis.start) / span) * 100;
  const tone = executionStateTone(row.state);
  const bar = row.bar;
  return (
    <div role="gridcell" className="relative h-full">
      <span className="sr-only">{describeTimeline(row, axis.start)}</span>
      {ticks.map((t) => (
        <span key={t.at} aria-hidden className="absolute inset-y-0 w-px bg-border" style={{ left: `${pct(t.at)}%` }} />
      ))}
      {bar ? (
        <span
          aria-hidden
          data-testid={`gantt-bar-${row.runIx}`}
          data-estimated={row.estimated || undefined}
          data-open={bar.open || undefined}
          title={row.estimated ? ESTIMATED_DURATION_TITLE : undefined}
          className={cn(
            "absolute top-1/2 h-3 min-w-0.5 -translate-y-1/2 rounded-sm",
            row.estimated ? "border border-dashed" : row.attempts.length > 0 ? `${TONE_FILL[tone]} opacity-35` : TONE_FILL[tone],
            bar.open && "rounded-r-none",
          )}
          style={{
            left: `${pct(bar.start)}%`,
            width: `${pct(bar.end) - pct(bar.start)}%`,
            ...(row.estimated ? hatch(tone) : {}),
          }}
        />
      ) : null}
      {bar
        ? row.attempts.map((a, i) => (
            <AttemptSegment key={i} attempt={a} index={i} runIx={row.runIx} estimated={row.estimated} barEnd={bar.end} pct={pct} />
          ))
        : null}
      {row.marker && row.markerAt !== undefined ? (
        <span
          aria-hidden
          data-testid={`gantt-marker-${row.runIx}`}
          title={markerReason(row.state)}
          className="absolute top-1/2 size-2.5 -translate-x-1/2 -translate-y-1/2 rotate-45 border-2 border-muted-foreground bg-background"
          style={{ left: `${pct(row.markerAt)}%` }}
        />
      ) : null}
    </div>
  );
}

function AttemptSegment({
  attempt,
  index,
  runIx,
  estimated,
  barEnd,
  pct,
}: {
  attempt: GanttAttempt;
  index: number;
  runIx: number;
  estimated: boolean;
  barEnd: number;
  pct: (t: number) => number;
}) {
  const tone = executionStateTone(attempt.state);
  const end = attempt.end ?? barEnd;
  return (
    <span
      aria-hidden
      data-testid={`gantt-attempt-${runIx}-${index}`}
      data-state={attempt.state}
      title={`Attempt ${index + 1}: ${stateWord(attempt.state)}`}
      // A one-pixel gap on the right, so two attempts that touch are still two.
      className={cn(
        "absolute top-1/2 h-3 min-w-0.5 -translate-y-1/2 rounded-sm border-r border-background",
        estimated ? "border border-dashed" : TONE_FILL[tone],
      )}
      style={{ left: `${pct(attempt.start)}%`, width: `${pct(end) - pct(attempt.start)}%`, ...(estimated ? hatch(tone) : {}) }}
    />
  );
}

export interface GanttViewProps {
  model: GanttModel;
  /** The run whose panel is open (`?run=`), if any. */
  selectedRunIx: number | null;
  /** Opens a run's panel, or closes it with `null`. */
  onSelect: (runIx: number | null) => void;
}

export function GanttView({ model, selectedRunIx, onSelect }: GanttViewProps) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set());
  const [focusIx, setFocusIx] = useState<number | null>(selectedRunIx);
  const rowEls = useRef(new Map<number, HTMLDivElement>());
  const timelineHead = useRef<HTMLDivElement>(null);
  const size = useElementSize(timelineHead);
  // jsdom has no layout and measures 0; a real browser never draws the chart that narrow.
  const width = size && size.width > 0 ? size.width : 600;
  const ticks = axisTicks(model.axis, width);
  const span = model.axis.end - model.axis.start;

  const rows = visibleRows(model.rows, collapsed);
  const tabStop = rows.some((r) => r.runIx === focusIx) ? focusIx : (rows[0]?.runIx ?? null);

  const focusRow = (ix: number) => {
    setFocusIx(ix);
    rowEls.current.get(ix)?.focus();
  };
  const toggle = (ix: number, open?: boolean) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      const shut = open === undefined ? !prev.has(ix) : !open;
      if (shut) next.add(ix);
      else next.delete(ix);
      return next;
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>, row: GanttRow, i: number) => {
    const isCollapsed = collapsed.has(row.runIx);
    switch (e.key) {
      case "ArrowDown":
        if (rows[i + 1]) focusRow(rows[i + 1].runIx);
        break;
      case "ArrowUp":
        if (rows[i - 1]) focusRow(rows[i - 1].runIx);
        break;
      case "Home":
        if (rows[0]) focusRow(rows[0].runIx);
        break;
      case "End":
        if (rows.length) focusRow(rows[rows.length - 1].runIx);
        break;
      case "ArrowRight":
        if (row.hasChildren && isCollapsed) toggle(row.runIx, true);
        break;
      case "ArrowLeft":
        if (row.hasChildren && !isCollapsed) toggle(row.runIx, false);
        else if (row.parentIx !== null) focusRow(row.parentIx);
        break;
      case "Enter":
      case " ":
        onSelect(row.runIx);
        break;
      case "Escape":
        if (selectedRunIx === null) return;
        onSelect(null);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const pct = (t: number) => ((t - model.axis.start) / span) * 100;

  return (
    <div className="overflow-x-auto rounded-md border">
      <div role="grid" aria-label="Task runs over time" aria-rowcount={rows.length + 1} className="min-w-[36rem] text-sm">
        <div role="row" aria-rowindex={1} className={cn(COLUMNS, "h-8 border-b bg-muted text-xs text-muted-foreground")}>
          <div role="columnheader" className="px-3">
            Task
          </div>
          <div role="columnheader" className="px-2">
            State
          </div>
          <div role="columnheader" className="px-2">
            Duration
          </div>
          <div role="columnheader" ref={timelineHead} className="relative h-full overflow-hidden">
            <span className="sr-only">Timeline</span>
            {ticks.map((t, i) => {
              const at = pct(t.at);
              return (
                <span
                  key={t.at}
                  aria-hidden
                  data-testid="gantt-tick"
                  className={cn(
                    "absolute top-1/2 -translate-y-1/2 whitespace-nowrap tabular-nums",
                    i === 0 ? "pl-1" : at > 92 ? "-translate-x-full pr-1" : "-translate-x-1/2",
                  )}
                  style={{ left: `${at}%` }}
                >
                  {t.label}
                </span>
              );
            })}
          </div>
        </div>
        {rows.map((row, i) => {
          const selected = row.runIx === selectedRunIx;
          const isCollapsed = collapsed.has(row.runIx);
          return (
            <div
              key={row.runIx}
              role="row"
              aria-rowindex={i + 2}
              aria-selected={selected}
              data-run-ix={row.runIx}
              tabIndex={row.runIx === tabStop ? 0 : -1}
              ref={(el) => {
                if (el) rowEls.current.set(row.runIx, el);
                else rowEls.current.delete(row.runIx);
              }}
              onFocus={() => setFocusIx(row.runIx)}
              onClick={() => onSelect(selected ? null : row.runIx)}
              onKeyDown={(e) => onKeyDown(e, row, i)}
              className={cn(
                COLUMNS,
                "h-9 cursor-pointer border-b border-l-2 last:border-b-0 hover:bg-muted",
                "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none focus-visible:ring-inset",
                selected ? "border-l-primary bg-muted" : "border-l-transparent",
              )}
            >
              <div role="gridcell" className="flex min-w-0 items-center gap-1 pr-2" style={{ paddingLeft: `calc(${row.depth} * 1rem + 0.25rem)` }}>
                {row.hasChildren ? (
                  <IconButton
                    tabIndex={-1}
                    aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${row.taskId}`}
                    aria-expanded={!isCollapsed}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggle(row.runIx);
                    }}
                    className="size-5"
                  >
                    {isCollapsed ? <ChevronRight aria-hidden className="size-4" /> : <ChevronDown aria-hidden className="size-4" />}
                  </IconButton>
                ) : (
                  <span aria-hidden className="w-5 shrink-0" />
                )}
                <span className="truncate font-mono text-xs" title={row.value ? `${row.taskId} ${row.value}` : row.taskId}>
                  {row.taskId}
                  {row.value ? (
                    <>
                      {" "}
                      <span className="text-muted-foreground">{row.value}</span>
                    </>
                  ) : null}
                </span>
                {row.attempts.length > 1 ? (
                  <span className="ml-auto flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground tabular-nums" title="Retried">
                    <RotateCcw aria-hidden className="size-3" />
                    {row.attempts.length} attempts
                  </span>
                ) : null}
              </div>
              <div role="gridcell" className="px-2">
                <StateChip state={row.state} kind="execution" />
              </div>
              <div role="gridcell" className="px-2 tabular-nums">
                <RowDuration row={row} />
              </div>
              <TimelineCell row={row} axis={model.axis} ticks={ticks} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** What the run panel's slots get: the run, for Tasks 5 and 6 to filter logs and read outputs by. */
export interface GanttRunSlots {
  logs?: (run: TaskRunDto) => ReactNode;
  outputs?: (run: TaskRunDto) => ReactNode;
}

function Fact({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline gap-1.5">
      <dt className="text-muted-foreground">{term}</dt>
      <dd className="tabular-nums">{children}</dd>
    </div>
  );
}

/**
 * The selected run, below the chart (spec §6.4): its times and attempts, its
 * message, then its logs and outputs. Kestra's `TaskRunDetails.vue` opens
 * under the row; egret opens one panel under the chart, so the bars keep their
 * places while a run is read.
 */
export function GanttRunPanel({ row, onClose, slots }: { row: GanttRow; onClose: () => void; slots?: GanttRunSlots }) {
  const headingId = useId();
  const run = row.run;
  return (
    <section aria-labelledby={headingId} data-testid="gantt-run-panel" className="mt-4 rounded-md border p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h2 id={headingId} className="truncate font-mono text-base font-semibold">
            {row.taskId}
            {row.value ? (
              <>
                {" "}
                <span className="font-normal text-muted-foreground">{row.value}</span>
              </>
            ) : null}
          </h2>
          <StateChip state={row.state} kind="execution" />
        </div>
        {/* "Close the task run" is also this module's marker in `scripts/assert-workflows-lazy.mjs`. */}
        <IconButton aria-label="Close the task run" onClick={onClose} className="size-8">
          <X aria-hidden className="size-4" />
        </IconButton>
      </div>
      <dl className="mt-2 flex flex-wrap gap-x-6 gap-y-1 text-sm">
        {row.bar ? (
          <>
            <Fact term="Started">
              {run.started_at ? fullTimestamp(run.started_at) : `≈ ${fullTimestamp(new Date(row.bar.start).toISOString())}`}
            </Fact>
            <Fact term="Ended">
              {run.ended_at
                ? fullTimestamp(run.ended_at)
                : row.bar.open
                  ? "Not yet"
                  : `≈ ${fullTimestamp(new Date(row.bar.end).toISOString())}`}
            </Fact>
            <Fact term="Duration">
              <RowDuration row={row} />
            </Fact>
          </>
        ) : (
          <Fact term="Timing">{markerReason(row.state)}</Fact>
        )}
        <Fact term="Run">#{row.runIx}</Fact>
      </dl>
      {row.attempts.length > 0 ? (
        <ol aria-label="Attempts" className="mt-3 flex flex-wrap gap-2 text-sm">
          {row.attempts.map((a, i) => (
            <li key={i} className="flex items-center gap-1.5 rounded-md border px-2 py-1">
              <span className="text-muted-foreground">Attempt {i + 1}</span>
              <StateChip state={a.state} kind="execution" />
              <span className="tabular-nums">{a.end !== undefined ? formatDuration(a.end - a.start) : "running"}</span>
            </li>
          ))}
        </ol>
      ) : null}
      {run.message ? (
        <div className="mt-3">
          <h3 className="text-sm font-medium">Message</h3>
          <pre className="mt-1 whitespace-pre-wrap break-words rounded-md bg-muted p-2 font-mono text-xs">{run.message}</pre>
        </div>
      ) : null}
      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        <section aria-label="Logs" data-testid="gantt-run-logs">
          <h3 className="text-sm font-medium">Logs</h3>
          <div className="mt-1">{slots?.logs ? slots.logs(run) : <SlotPlaceholder what="This run's logs" />}</div>
        </section>
        <section aria-label="Outputs" data-testid="gantt-run-outputs">
          <h3 className="text-sm font-medium">Outputs</h3>
          <div className="mt-1">{slots?.outputs ? slots.outputs(run) : <SlotPlaceholder what="This run's outputs" />}</div>
        </section>
      </div>
    </section>
  );
}

function SlotPlaceholder({ what }: { what: string }) {
  return <p className="text-sm text-muted-foreground">{what} arrive in a later release.</p>;
}
