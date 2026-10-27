/**
 * The execution states after which nothing changes: Kestra's
 * `State.Type.isTerminated()`, as `State::is_terminated` in
 * `crates/egret-workflow/src/models/state.rs` lists them. A page polling an
 * execution stops at one of these.
 */
const TERMINAL_STATES = new Set(["FAILED", "WARNING", "SUCCESS", "KILLED", "CANCELLED", "RETRIED", "SKIPPED", "RESUBMITTED"]);

export function isTerminalState(state: string): boolean {
  return TERMINAL_STATES.has(state);
}

/**
 * How long after it ended a terminal execution is still followed for a live
 * run. An `afterExecution:` run starts right after the execution ends and is
 * short; an execution can also hold a run that never ends (a non-KILLED `Exit`
 * beside a flowable sibling leaves a main-list run `RUNNING`), and following
 * that for ever would poll for ever. Past this window the page treats the
 * execution as finished.
 */
export const AFTER_RUN_FOLLOW_MS = 30 * 60 * 1000;

/** What `executionLive` reads of an execution (`ExecutionDto` fits). */
export interface LiveExecution {
  state: string;
  ended_at?: string | null;
  updated_at: string;
  runs: readonly { state: string }[];
}

/**
 * Is the execution still changing: not terminal, or terminal with a run that
 * is not (an `afterExecution:` run), for `AFTER_RUN_FOLLOW_MS` after it ended.
 * The window starts at `ended_at`, or at `updated_at` when no end is recorded.
 */
export function executionLive(execution: LiveExecution, now: number): boolean {
  if (!isTerminalState(execution.state)) return true;
  if (!execution.runs.some((r) => !isTerminalState(r.state))) return false;
  const ended = Date.parse(execution.ended_at ?? execution.updated_at);
  return Number.isFinite(ended) && now - ended < AFTER_RUN_FOLLOW_MS;
}

/** A span as people read it: `45 s`, `1 min 5 s`, `2 h 3 min`. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  if (total < 60) return `${total} s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} min ${total % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}
