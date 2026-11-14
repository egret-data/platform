import type { ExecutionDto, ExecutionListItemDto } from "@/api/generated/model";
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

/**
 * RESTART, CHANGE STATUS AND CHANGE STATE (execution-ops spec D10, D11, D14).
 * All need `workflow:manage`, which hides them like the others.
 */

/** The four states a state change can set (`StateChangeTarget`), in the order the select lists them. */
const CHANGE_TARGETS = ["SUCCESS", "WARNING", "FAILED", "CANCELLED"] as const;
export type ChangeTarget = (typeof CHANGE_TARGETS)[number];

/** The targets a select offers: the four, minus the state the run or execution is in. */
export function changeTargets(current: string): ChangeTarget[] {
  return CHANGE_TARGETS.filter((s) => s !== current);
}

type LoopFields = Pick<ExecutionListItemDto, "parent_execution_id" | "loop_index">;

/** A loop iteration: a child with a `loop_index`. A subflow child has a parent but no index. */
export function isLoopIteration(execution: LoopFields): boolean {
  return execution.parent_execution_id != null && execution.loop_index != null;
}

const iterationReason = (execution: LoopFields, action: string) =>
  `This is an iteration of a loop. ${action} the loop's execution ${execution.parent_execution_id} instead`;

/** Restart is shown for an execution that failed or waits (D10); a running or finished-well one has nothing to restart. */
export function showsRestart(execution: Pick<ExecutionDto, "state">): boolean {
  return execution.state === "FAILED" || execution.state === "PAUSED";
}

/** Restart, for an execution that `showsRestart`: not a loop iteration, which only its loop's execution restarts. */
export function canRestart(execution: LoopFields): Enablement {
  return isLoopIteration(execution) ? { ok: false, reason: iterationReason(execution, "Restart") } : { ok: true };
}

/** Change status and Change state: an ended execution, `KILLED` excepted (D11). */
export function showsChangeStatus(execution: Pick<ExecutionDto, "state">): boolean {
  return isTerminalState(execution.state) && execution.state !== "KILLED";
}

export const showsChangeTaskRunState = showsChangeStatus;

/** Change status, for an execution that `showsChangeStatus`. */
export function canChangeStatus(execution: LoopFields): Enablement {
  return isLoopIteration(execution) ? { ok: false, reason: iterationReason(execution, "Change the status of") } : { ok: true };
}

/** Change state of a task run, for an execution that `showsChangeTaskRunState`. */
export function canChangeTaskRunState(execution: LoopFields): Enablement {
  return isLoopIteration(execution) ? { ok: false, reason: iterationReason(execution, "Change the task run state in") } : { ok: true };
}

const NOT_RESTARTABLE = "The execution has not ended, or it was killed, so it cannot be changed. Refresh to see its latest state.";
const AFTER_EXECUTION_RUNNING = "The afterExecution tasks are still running. Try again once they have ended.";
const FILES_PRUNED = "The execution's files were deleted, so it cannot be re-run. Replay it instead.";

/** One sentence for each coded 409 of the restart route; `null` for any other answer. */
export function restartRefusal(code: string | undefined): string | null {
  switch (code) {
    case "not_restartable":
      return "Only a failed or paused execution can be restarted. Refresh to see its latest state.";
    case "nothing_to_restart":
      return "There is no failed task to restart. Refresh to see the execution's latest state.";
    case "after_execution_running":
      return AFTER_EXECUTION_RUNNING;
    case "files_pruned":
      return FILES_PRUNED;
    default:
      return null;
  }
}

/** One sentence for each coded 409 of the status and task-run-state routes; `null` for any other answer. */
export function stateChangeRefusal(kind: "status" | "task_run", code: string | undefined): string | null {
  switch (code) {
    case "not_restartable":
      return NOT_RESTARTABLE;
    case "after_execution_running":
      return AFTER_EXECUTION_RUNNING;
    case "files_pruned":
      return FILES_PRUNED;
    case "state_unchanged":
      return kind === "status" ? "The execution is already in that state. Pick another." : "The task run is already in that state. Pick another.";
    case "inside_working_directory":
      return "This task run is inside a WorkingDirectory block. Restart the execution instead, which re-runs the block whole.";
    default:
      return null;
  }
}
