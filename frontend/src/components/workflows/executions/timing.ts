import { isTerminalState } from "@/components/workflows/executionState";
import { fullTimestamp } from "@/lib/relativeTime";

/**
 * WHEN AN EXECUTION RAN, AND FOR HOW LONG: the one place the list, the detail
 * header and the gantt read it from (W7b spec §3.2, §6.3).
 *
 * Migration 0180 added `started_at` and `ended_at`. Each null means something
 * different, so each field is judged on its own, never "both null means old":
 *
 * | `started_at` | `ended_at` | history has `RUNNING` | what it is |
 * |---|---|---|---|
 * | set | set | — | exact |
 * | set | null | — | still running or paused (ticks to now) |
 * | null | set | yes | in flight when 0180 was deployed: the end is real, the start is not |
 * | null | null | yes, row terminal | recorded before 0180 (legacy) |
 * | null | null | yes, row live | in flight at deploy and still going |
 * | null | any | no | never ran: CREATED/QUEUED, SKIPPED, or cancelled by an SLA at CREATED |
 *
 * A missing start of a row that ran falls back to `created_at`, and a missing
 * end of a terminal row to `updated_at` (the old recipe); either fallback makes
 * the result `estimated`. A row that never ran has no duration at all.
 */
export interface TimedExecution {
  state: string;
  /** The state names, in order (`ExecutionListItemDto.history`). */
  history: readonly string[];
  started_at?: string | null;
  ended_at?: string | null;
  created_at: string;
  updated_at: string;
}

export interface ExecutionDuration {
  ms: number;
  /** A fallback timestamp stood in for a missing one. */
  estimated: boolean;
  /** Not terminal: measured to `now`, and moves with it. */
  running: boolean;
}

/** Whether the execution ever entered `RUNNING`. A recorded start is proof enough. */
export function hasRun(row: Pick<TimedExecution, "history" | "started_at">): boolean {
  return !!row.started_at || row.history.includes("RUNNING");
}

/** The execution's duration at `now` (ms since the epoch), or `null` when it never ran. */
export function executionDuration(row: TimedExecution, now: number): ExecutionDuration | null {
  if (!hasRun(row)) return null;
  const running = !isTerminalState(row.state);
  const start = row.started_at ?? row.created_at;
  let estimated = !row.started_at;
  let end: number;
  if (row.ended_at) end = Date.parse(row.ended_at);
  else if (running) end = now;
  else {
    end = Date.parse(row.updated_at);
    estimated = true;
  }
  return { ms: Math.max(0, end - Date.parse(start)), estimated, running };
}

export interface ExecutionStart {
  iso: string;
  /** `started`: recorded. `estimated`: ran, start unrecorded, `created_at` shown. `created`: never ran. */
  kind: "started" | "estimated" | "created";
}

/** What a "start" column shows: the recorded start, or `created_at` saying why. */
export function executionStart(row: TimedExecution): ExecutionStart {
  if (row.started_at) return { iso: row.started_at, kind: "started" };
  return { iso: row.created_at, kind: hasRun(row) ? "estimated" : "created" };
}

/**
 * What an estimated duration says on hover. Shared so the list, the detail
 * header and the gantt word it the same way. Also this module's marker in
 * `scripts/assert-workflows-lazy.mjs`.
 */
export const ESTIMATED_DURATION_TITLE =
  "Estimated: this execution's start or end was not recorded, so its creation and last update stand in.";

/** What a start says on hover: the exact time, and why it is not a recorded start when it is not. */
export function startTitle(start: ExecutionStart): string {
  if (start.kind === "started") return fullTimestamp(start.iso);
  if (start.kind === "estimated") return `Created ${fullTimestamp(start.iso)}. Estimated: the start was not recorded.`;
  return `Created ${fullTimestamp(start.iso)}; not started yet.`;
}
