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

/** A span as people read it: `45 s`, `1 min 5 s`, `2 h 3 min`. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  if (total < 60) return `${total} s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} min ${total % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}
