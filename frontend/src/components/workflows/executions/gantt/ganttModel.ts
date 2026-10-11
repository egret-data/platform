import type { ExecutionDto, TaskRunDto } from "@/api/generated/model";
import { isTerminalState } from "@/components/workflows/executionState";
import { hasRun } from "@/components/workflows/executions/timing";

/**
 * THE GANTT, AS DATA (W7b spec §6). Pure: an execution and a clock in, rows
 * and an axis out, all instants in milliseconds since the epoch. The view
 * (`GanttView.tsx`) only places what this decides.
 *
 * **Which time each bar is drawn from, per field** (spec §6.2, §6.3, and the
 * table in `timing.ts`). A null `started_at` means one of four things, so it
 * is never read as "old row" on its own:
 *
 * | the run | start | end |
 * |---|---|---|
 * | never ran (no `RUNNING` in `history`, no `started_at`): `CREATED`, `SKIPPED`, cancelled at `CREATED` | — a marker, never a bar | — |
 * | ran, `started_at` recorded | `started_at` | `ended_at`; else now while live |
 * | ran, `started_at` null (before migration 0180, or in flight when it was deployed) | `created_at` + `attempts[0].started_at`, else the run's own `created_at` (a flowable has no attempts) — estimated | `ended_at` when recorded (real); else `updated_at` once `ended_seq` is set (or the run is terminal) — estimated; else now |
 *
 * The estimated recipe is `crates/egret-workflow/tests/executions_routes.rs`
 * `gantt_rows`, which reads the same DTO the same way.
 *
 * **Attempts** are milliseconds after the EXECUTION's `created_at` (a `Tick`
 * in this engine). Each keeps the state its own history ended in, so a failed
 * attempt that a retry followed stays visibly failed.
 */

/** What the model reads of an execution: `ExecutionDto` fits. */
export type GanttExecution = Pick<ExecutionDto, "state" | "history" | "created_at" | "updated_at" | "started_at" | "ended_at" | "runs">;

export interface GanttBar {
  start: number;
  end: number;
  /** Not ended: `end` is the clock, and moves with it. */
  open: boolean;
}

export interface GanttAttempt {
  start: number;
  /** Absent while the attempt is still open: it runs to the bar's end. */
  end?: number;
  /** The state the attempt's own history ended in. */
  state: string;
}

export interface GanttRow {
  runIx: number;
  depth: number;
  /** The parent row's `run_ix`, or `null` at the top level (also for a parent that is not in the list). */
  parentIx: number | null;
  taskId: string;
  value: string | null;
  state: string;
  /** Absent exactly when `marker` is set. */
  bar?: GanttBar;
  attempts: GanttAttempt[];
  /** A fallback stood in for a missing start or end (spec §6.3). */
  estimated: boolean;
  /** The run never ran: a marker at `markerAt`, not a bar. */
  marker: boolean;
  markerAt?: number;
  hasChildren: boolean;
  run: TaskRunDto;
}

export interface GanttModel {
  axis: { start: number; end: number };
  /** Depth-first, siblings by `run_ix`. */
  rows: GanttRow[];
  /** The execution is not terminal: open bars and the axis follow the clock. */
  live: boolean;
  /** Some row is estimated, so the tab carries the note. */
  estimated: boolean;
}

const parse = (iso: string | null | undefined): number | undefined => (iso ? Date.parse(iso) : undefined);

function lastState(history: readonly string[], fallback: string): string {
  return history.length > 0 ? history[history.length - 1] : fallback;
}

/** One run's times, before nesting. */
function timeRun(run: TaskRunDto, origin: number, now: number) {
  const ended = parse(run.ended_at);
  if (!hasRun(run)) {
    // A run that never ran gets no bar, estimated or not: it sits where it was
    // decided (its recorded end, or its last write once terminal), or where it
    // was created while nothing has happened to it yet.
    const decided = ended ?? (run.ended_seq != null || isTerminalState(run.state) ? parse(run.updated_at) : undefined);
    return { marker: true, markerAt: decided ?? Date.parse(run.created_at), estimated: false, attempts: [] as GanttAttempt[] };
  }

  const recordedStart = parse(run.started_at);
  const firstAttempt = run.attempts[0];
  const start = recordedStart ?? (firstAttempt ? origin + firstAttempt.started_at : Date.parse(run.created_at));
  let estimated = recordedStart === undefined;

  let end: number;
  let open = false;
  if (ended !== undefined) end = ended;
  else if (run.ended_seq != null || isTerminalState(run.state)) {
    // `updated_at` moves on every write; it is an end only once the run has one.
    end = Date.parse(run.updated_at);
    estimated = true;
  } else {
    end = now;
    open = true;
  }
  end = Math.max(end, start);

  const attempts = run.attempts.map((a, i): GanttAttempt => {
    const aStart = origin + a.started_at;
    const state = lastState(a.history, run.state);
    if (a.ended_at != null) return { start: aStart, end: origin + a.ended_at, state };
    // An attempt recorded before attempt ends existed: a retry's attempt ends
    // no later than the next one starts, and the last one with the run.
    const next = run.attempts[i + 1];
    if (next) return { start: aStart, end: origin + next.started_at, state };
    return open ? { start: aStart, state } : { start: aStart, end, state };
  });

  return { marker: false, bar: { start, end, open }, estimated, attempts };
}

export function buildGantt(execution: GanttExecution, now: number): GanttModel {
  const origin = Date.parse(execution.created_at);
  const live = !isTerminalState(execution.state);

  const byIx = new Map(execution.runs.map((r) => [r.run_ix, r]));
  const children = new Map<number | null, TaskRunDto[]>();
  for (const r of execution.runs) {
    const parent = r.parent_ix != null && byIx.has(r.parent_ix) ? r.parent_ix : null;
    const list = children.get(parent) ?? [];
    list.push(r);
    children.set(parent, list);
  }
  for (const list of children.values()) list.sort((a, b) => a.run_ix - b.run_ix);

  const rows: GanttRow[] = [];
  const seen = new Set<number>();
  const visit = (r: TaskRunDto, depth: number, parentIx: number | null) => {
    if (seen.has(r.run_ix)) return;
    seen.add(r.run_ix);
    const kids = children.get(r.run_ix) ?? [];
    rows.push({
      runIx: r.run_ix,
      depth,
      parentIx,
      taskId: r.task_id,
      value: r.value ?? null,
      state: r.state,
      hasChildren: kids.length > 0,
      run: r,
      ...timeRun(r, origin, now),
    });
    for (const k of kids) visit(k, depth + 1, r.run_ix);
  };
  for (const r of children.get(null) ?? []) visit(r, 0, null);
  // A parent_ix cycle has no root; its runs are still shown, at the top level.
  for (const r of [...execution.runs].sort((a, b) => a.run_ix - b.run_ix)) visit(r, 0, null);

  // The axis: the execution's start to its end, or to now while it runs, widened to hold every bar and marker.
  let start = parse(execution.started_at) ?? origin;
  let end = live ? now : (parse(execution.ended_at) ?? Date.parse(execution.updated_at));
  for (const row of rows) {
    const from = row.bar?.start ?? row.markerAt;
    const to = row.bar?.end ?? row.markerAt;
    if (from !== undefined) start = Math.min(start, from);
    if (to !== undefined) end = Math.max(end, to);
  }
  // A zero-width axis would divide by zero.
  end = Math.max(end, start + 1);

  return { axis: { start, end }, rows, live, estimated: rows.some((r) => r.estimated) };
}

/** The rows a collapse leaves on screen: every descendant of a collapsed row is hidden. */
export function visibleRows(rows: readonly GanttRow[], collapsed: ReadonlySet<number>): GanttRow[] {
  const hidden = new Set<number>();
  const out: GanttRow[] = [];
  for (const row of rows) {
    if (row.parentIx !== null && (hidden.has(row.parentIx) || collapsed.has(row.parentIx))) {
      hidden.add(row.runIx);
      continue;
    }
    out.push(row);
  }
  return out;
}

/**
 * Why a run has a marker and no bar. Also this module's marker in
 * `scripts/assert-workflows-lazy.mjs`.
 */
export function markerReason(state: string): string {
  if (state === "SKIPPED") return "Skipped: it never ran.";
  if (state === "CANCELLED" || state === "KILLED") return "Cancelled before it started.";
  if (isTerminalState(state)) return "Ended without running.";
  return "Not started yet.";
}

/** Round tick steps, smallest first. */
const STEPS_MS = [
  100, 200, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000,
  3_600_000, 7_200_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000,
];
/** The least room a tick label gets, in pixels: "2 min 30 s" in `text-xs` with a gap. */
export const MIN_TICK_GAP_PX = 90;

/** Elapsed time as a tick says it: `0 s`, `0.2 s`, `45 s`, `1 min 30 s`, `2 h`, `1 h 30 min`. */
function tickLabel(ms: number): string {
  if (ms < 60_000) return `${Number((ms / 1000).toFixed(1))} s`;
  const totalS = Math.round(ms / 1000);
  if (totalS < 3600) {
    const m = Math.floor(totalS / 60);
    const s = totalS % 60;
    return s ? `${m} min ${s} s` : `${m} min`;
  }
  const totalM = Math.round(totalS / 60);
  const h = Math.floor(totalM / 60);
  const m = totalM % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

/**
 * The axis ticks for a chart `widthPx` wide: the smallest round step whose
 * labels stay `MIN_TICK_GAP_PX` apart, labelled as time since the axis start.
 */
export function axisTicks(axis: { start: number; end: number }, widthPx: number): { at: number; label: string }[] {
  const span = axis.end - axis.start;
  const least = (span * MIN_TICK_GAP_PX) / Math.max(widthPx, 1);
  const step = STEPS_MS.find((s) => s >= least) ?? Math.ceil(least / 86_400_000) * 86_400_000;
  const ticks: { at: number; label: string }[] = [];
  for (let t = 0; t <= span; t += step) ticks.push({ at: axis.start + t, label: tickLabel(t) });
  return ticks;
}
