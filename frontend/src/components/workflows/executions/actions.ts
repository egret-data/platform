import type { ExecutionDto } from "@/api/generated/model";
import type { Enablement } from "@/components/workflows/draftState";
import { executionLive, isTerminalState, type LiveExecution } from "@/components/workflows/executionState";

/**
 * WHEN RESUME, REPLAY AND KILL ARE OFFERED (W7b spec §5.2; Kill: workflow kill
 * spec §7). All need `workflow:manage`, which the page reads from
 * `ExecutionDto.can_manage` and which hides them all; these rules decide,
 * for a user who has it, whether each is offered, and say why not
 * (FE-ACTION-4).
 */

/**
 * Resume is shown exactly while the execution waits (spec §5.2). The route
 * releases every paused `Pause` at once, whatever its bounds (Kestra's
 * `ExecutionService.resume`; the user's ruling of 2026-10-11); see
 * `canResume` for the one wait it does not end.
 */
export function showsResume(execution: Pick<ExecutionDto, "state">): boolean {
  return execution.state === "PAUSED";
}

/** Spec §5.2 (fix round 2), verbatim. Also this module's marker in `scripts/assert-workflows-lazy.mjs`. */
export const LOOP_WAIT_REASON = "Waiting on a loop condition, not a Pause task";

/**
 * Is a `Pause` task waiting? A `PAUSED` run whose `resume_with` the engine
 * wrote for a `Pause` (`NEVER`, or `TERMINAL:*` for a bounded one) -- the
 * engine's own answer (`executor::Resume::wire`), not a re-derivation from the
 * source. A `LoopUntil` waiting for its next iteration carries `ITERATE` and
 * puts the execution in `PAUSED` too, but a resume does not end it (Kestra
 * resumes `Pause` tasks only), and the route answers 409.
 */
function aPauseIsWaiting(execution: Pick<ExecutionDto, "runs">): boolean {
  return execution.runs.some(
    (r) => r.state === "PAUSED" && (r.resume_with === "NEVER" || (r.resume_with ?? "").startsWith("TERMINAL:")),
  );
}

/** Resume, for an execution that `showsResume`: enabled when a `Pause` task waits (spec §5.2). */
export function canResume(execution: Pick<ExecutionDto, "runs">): Enablement {
  return aPauseIsWaiting(execution) ? { ok: true } : { ok: false, reason: LOOP_WAIT_REASON };
}

/**
 * Replay: shown once the execution has finished (spec §5.2), as Kill is shown
 * until it has (KI-193): a running execution's header holds the actions that
 * apply to it, not a disabled Replay with a reason wrapping under it. The
 * server's other refusals (a loop iteration, a disabled or trashed flow) are
 * its own to state; the dialog shows them in its words.
 */
export function showsReplay(execution: Pick<ExecutionDto, "state">): boolean {
  return isTerminalState(execution.state);
}

/** Why Replay is disabled on an execution whose flow was deleted for good (KI-297). */
export const REPLAY_FLOW_DELETED_REASON = "The flow was deleted permanently, so this execution cannot be replayed";

/**
 * Replay, for an execution that `showsReplay`: disabled once its flow was deleted permanently
 * (`flow_id: null`), where the route can only answer 404 "not found: flow" (KI-297). A trashed
 * flow still has its id, and the route says in its own words why it refuses.
 */
export function canReplay(execution: Pick<ExecutionDto, "flow_id">): Enablement {
  return execution.flow_id == null ? { ok: false, reason: REPLAY_FLOW_DELETED_REASON } : { ok: true };
}

/**
 * Kill (workflow kill spec §7, D6): shown while the execution has not ended,
 * and while an ended one still has a live `afterExecution:` run (end hooks
 * D15, Q3), within `AFTER_RUN_FOLLOW_MS`. `KILLING` is live, not terminal, so
 * the button stays, disabled, until the worker has stopped the running task
 * and the execution is `KILLED`.
 */
export function showsKill(execution: LiveExecution, now: number): boolean {
  return executionLive(execution, now);
}

/** Why Kill is disabled while `KILLING`. */
export const KILL_IN_PROGRESS_REASON = "The kill was sent; a worker is stopping the running task";

/**
 * Kill, for an execution that `showsKill`: every live state, `PAUSED`
 * included, except one already being killed. A kill of an ended execution's
 * `afterExecution:` run leaves the execution's own state alone, so that one is
 * read from the runs.
 */
export function canKill(execution: Pick<LiveExecution, "state" | "runs">): Enablement {
  const killing = execution.state === "KILLING" || (isTerminalState(execution.state) && execution.runs.some((r) => r.state === "KILLING"));
  return killing ? { ok: false, reason: KILL_IN_PROGRESS_REASON } : { ok: true };
}

/** What the page says after killing the `afterExecution:` run of an ended execution. */
export const AFTER_RUN_KILL_FEEDBACK = "Stopping the afterExecution tasks";

/**
 * What the page says after a kill, from the state the route answered: `KILLED`
 * when nothing was running, `KILLING` while a worker still stops a task. The
 * route answers a phase kill (`afterRunKill`: the execution had already ended)
 * with the execution's own, unchanged state, so that state says nothing about
 * the kill.
 */
export function killFeedback(answeredState: string, afterRunKill = false): string {
  if (afterRunKill) return AFTER_RUN_KILL_FEEDBACK;
  return answeredState === "KILLED" ? "Killed the execution" : "Killing the execution: the running task is being stopped";
}

/** `FAILED` → `Failed`, the state chip's own word. */
const stateWord = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();

/** The states a resumed execution can land in that are not a success. */
const NOT_A_SUCCESS = new Set(["FAILED", "KILLED", "CANCELLED"]);

/**
 * What the page says after a resume, from the state the route ANSWERED, never
 * from the request alone (FE-COPY-2): "Resumed" only when the execution is no
 * longer paused, the outcome when it then ended badly, and a defensive note
 * when it is somehow still paused (the waker or another resume raced it).
 */
export function resumeFeedback(answeredState: string): { tone: "success" | "warning"; message: string } {
  if (answeredState === "PAUSED") {
    return { tone: "warning", message: "Resume was sent, but the execution is still paused. Refresh to see its latest state." };
  }
  if (NOT_A_SUCCESS.has(answeredState)) {
    return { tone: "warning", message: `Resumed, but the execution then ended ${stateWord(answeredState)}.` };
  }
  return { tone: "success", message: "Resumed the execution" };
}
