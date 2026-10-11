import type { LogLineDto, TaskRunDto } from "@/api/generated/model";

/**
 * THE LOG VIEW'S ROWS (W7b spec §7), a pure model: lines grouped by task run,
 * flattened into rows of known height for the windowed list.
 *
 * Every height is exact, not measured: a message keeps its whitespace and does
 * not wrap (`white-space: pre`, scrolled sideways), so a line is as tall as its
 * message has lines. That is what lets ten thousand lines be placed without
 * rendering them.
 */

/** One text line of a message (`leading-5`). */
export const LINE_PX = 20;
/** A line row's vertical padding, top and bottom together (`py-0.5`). */
export const LINE_PAD_PX = 4;
/** A group's header row. */
export const HEADER_PX = 32;

/** The group of the lines about the execution rather than one run. */
export const EXECUTION_GROUP = "execution";

export interface LogGroup {
  /** The task run id, or `EXECUTION_GROUP`. */
  key: string;
  /** `null` for the execution's own lines. */
  taskId: string | null;
  runIx: number | null;
  /** The loop value, when the run is an iteration. */
  value: string | null;
  lines: LogLineDto[];
  /** More than one attempt: each line then says which. */
  multiAttempt: boolean;
}

export type LogRow =
  | { kind: "group"; key: string; group: LogGroup; collapsed: boolean }
  | { kind: "line"; key: string; line: LogLineDto; showAttempt: boolean };

export function lineHeight(message: string): number {
  let n = 1;
  for (let i = message.indexOf("\n"); i !== -1; i = message.indexOf("\n", i + 1)) n++;
  return n * LINE_PX + LINE_PAD_PX;
}

export const rowHeight = (row: LogRow): number => (row.kind === "group" ? HEADER_PX : lineHeight(row.line.message));

function attemptsDiffer(lines: LogLineDto[]): boolean {
  const first = lines.find((l) => l.attempt != null)?.attempt;
  return lines.some((l) => l.attempt != null && l.attempt !== first);
}

/**
 * Lines by task run: the execution's own lines first, then each run in
 * `run_ix` order (the order the execution planned them), whatever order their
 * lines arrived in. A run the execution does not list (yet) follows, in the
 * order its first line came. Lines keep id order within a group.
 */
export function groupLines(lines: LogLineDto[], runs: TaskRunDto[] = []): LogGroup[] {
  const byId = new Map(runs.map((r) => [r.id, r]));
  const groups = new Map<string, LogGroup>();
  for (const l of lines) {
    const key = l.task_run_id ?? EXECUTION_GROUP;
    let g = groups.get(key);
    if (!g) {
      const run = l.task_run_id ? byId.get(l.task_run_id) : undefined;
      g = {
        key,
        taskId: key === EXECUTION_GROUP ? null : (l.task_id ?? run?.task_id ?? null),
        runIx: run?.run_ix ?? null,
        value: run?.value ?? null,
        lines: [],
        multiAttempt: (run?.attempts.length ?? 0) > 1,
      };
      groups.set(key, g);
    }
    g.lines.push(l);
  }
  const all = [...groups.values()];
  for (const g of all) g.multiAttempt ||= attemptsDiffer(g.lines);
  const rank = (g: LogGroup) => (g.key === EXECUTION_GROUP ? -1 : (g.runIx ?? Number.MAX_SAFE_INTEGER));
  // `sort` is stable, so unknown runs keep the order their first line came in.
  return all.sort((a, b) => rank(a) - rank(b));
}

export interface LogRows {
  rows: LogRow[];
  /** `offsets[i]` is row `i`'s top; `offsets[rows.length]` is the total height. */
  offsets: number[];
}

/**
 * Grouped: a header per group, then its lines unless collapsed. Ungrouped (one
 * run's panel): the lines alone, in id order.
 */
export function buildRows(
  lines: LogLineDto[],
  { grouped, runs, collapsed }: { grouped: boolean; runs?: TaskRunDto[]; collapsed?: ReadonlySet<string> },
): LogRows {
  const rows: LogRow[] = [];
  if (grouped) {
    for (const group of groupLines(lines, runs)) {
      const isCollapsed = collapsed?.has(group.key) ?? false;
      rows.push({ kind: "group", key: `g:${group.key}`, group, collapsed: isCollapsed });
      if (isCollapsed) continue;
      for (const line of group.lines) rows.push({ kind: "line", key: `l:${line.id}`, line, showAttempt: group.multiAttempt });
    }
  } else {
    const multi = (runs ?? []).some((r) => r.attempts.length > 1) || attemptsDiffer(lines);
    for (const line of lines) rows.push({ kind: "line", key: `l:${line.id}`, line, showAttempt: multi });
  }
  const offsets = new Array<number>(rows.length + 1);
  offsets[0] = 0;
  for (let i = 0; i < rows.length; i++) offsets[i + 1] = offsets[i] + rowHeight(rows[i]);
  return { rows, offsets };
}

/** The index of the row that covers `y` (the last row whose top is at or above it). */
export function rowAt(offsets: number[], y: number): number {
  let lo = 0;
  let hi = offsets.length - 2;
  if (hi < 0) return 0;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (offsets[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}
