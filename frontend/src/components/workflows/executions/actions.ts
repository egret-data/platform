import type { ExecutionDto } from "@/api/generated/model";
import type { Enablement } from "@/components/workflows/draftState";
import { isTerminalState } from "@/components/workflows/executionState";

/**
 * WHEN RESUME AND REPLAY ARE OFFERED (W7b spec §5.2). Both need
 * `workflow:manage`, which the page reads from `ExecutionDto.can_manage` and
 * which hides them both; these rules decide, for a user who has it, whether
 * each is offered, and say why not (FE-ACTION-4).
 */

/** Spec §5.2, verbatim. Also this module's marker in `scripts/assert-workflows-lazy.mjs`. */
export const REPLAY_NOT_FINISHED_REASON = "Replay is available when the execution has finished";

/**
 * Resume is shown exactly while the execution waits (spec §5.2). The route
 * releases every paused `Pause` at once, whatever its bounds (Kestra's
 * `ExecutionService.resume`; the user's ruling of 2026-10-11); see
 * `canResume` for the one wait it does not end.
 */
export function showsResume(execution: Pick<ExecutionDto, "state">): boolean {
  return execution.state === "PAUSED";
}

/** Spec §5.2 (fix round 2), verbatim. */
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
 * Replay: only a terminal execution (spec §5.2). The server's other refusals
 * (a loop iteration, a disabled or trashed flow) are its own to state; the
 * dialog shows them in its words.
 */
export function canReplay(execution: Pick<ExecutionDto, "state">): Enablement {
  return isTerminalState(execution.state) ? { ok: true } : { ok: false, reason: REPLAY_NOT_FINISHED_REASON };
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
