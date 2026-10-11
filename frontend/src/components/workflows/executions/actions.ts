import { parse } from "yaml";
import type { ExecutionDto } from "@/api/generated/model";
import type { Enablement } from "@/components/workflows/draftState";
import { isTerminalState } from "@/components/workflows/executionState";

/**
 * WHEN RESUME AND REPLAY ARE OFFERED (W7b spec §5.2). Both need
 * `workflow:manage`, which the page reads from `ExecutionDto.can_manage` and
 * which hides them both; these rules decide, for a user who has it, whether
 * each is enabled, and say why not (FE-ACTION-4).
 */

/** Spec §5.2, verbatim. */
export const UNBOUNDED_PAUSE_REASON = "This pause has no duration or timeout, so it cannot be resumed from here";
/** Spec §5.2, verbatim. */
export const REPLAY_NOT_FINISHED_REASON = "Replay is available when the execution has finished";

/** Resume is shown only while the execution waits (spec §5.2). */
export function showsResume(execution: Pick<ExecutionDto, "state">): boolean {
  return execution.state === "PAUSED";
}

/** The spellings of Kestra's `Pause` the engine accepts (`spec/schema/mod.rs`, the flowable aliases). */
const PAUSE_TYPES = new Set(["io.kestra.plugin.core.flow.Pause", "core.pause"]);

/**
 * The top-level keys of a flow that hold tasks. `inputs` and `triggers` are
 * left out on purpose: their entries also carry `id` and `type`, and an input
 * named like a task must not be mistaken for it.
 */
const TASK_ROOTS = ["tasks", "errors", "finally", "afterExecution"];

type Node = Record<string, unknown>;
const isNode = (v: unknown): v is Node => typeof v === "object" && v !== null && !Array.isArray(v);

/** The task whose `id` is `taskId`, anywhere under the flow's task lists; task ids are unique in a flow. */
function findTask(flow: Node, taskId: string): Node | undefined {
  const walk = (v: unknown): Node | undefined => {
    if (Array.isArray(v)) {
      for (const item of v) {
        const hit = walk(item);
        if (hit) return hit;
      }
      return undefined;
    }
    if (!isNode(v)) return undefined;
    if (v.id === taskId && typeof v.type === "string") return v;
    for (const child of Object.values(v)) {
      const hit = walk(child);
      if (hit) return hit;
    }
    return undefined;
  };
  for (const key of TASK_ROOTS) {
    const hit = walk(flow[key]);
    if (hit) return hit;
  }
  return undefined;
}

/**
 * Is this the `Pause` the resume route cannot release: one with neither
 * `pauseDuration` nor `timeout` (`executor::Resume::Never`;
 * `routes.rs` `resume_execution`)? `false` when the task is not found or is
 * not a `Pause` (a paused `LoopUntil` waits for its next iteration).
 */
function isUnboundedPause(flow: Node, taskId: string): boolean {
  const task = findTask(flow, taskId);
  if (!task || !PAUSE_TYPES.has(task.type as string)) return false;
  const bound = (key: string) => task[key] !== undefined && task[key] !== null;
  return !bound("pauseDuration") && !bound("timeout");
}

/**
 * Resume, for an execution that `showsResume`. The pause's properties are read
 * from the execution's FROZEN `source`, the text that is running, never the
 * flow's draft. It is disabled only when every paused run is an unbounded
 * `Pause`: a resume then moves nothing. A source that does not parse leaves it
 * enabled, and the server decides.
 */
export function canResume(execution: Pick<ExecutionDto, "source" | "runs">): Enablement {
  const paused = execution.runs.filter((r) => r.state === "PAUSED");
  if (paused.length === 0) return { ok: true };
  let flow: unknown;
  try {
    flow = parse(execution.source);
  } catch {
    return { ok: true };
  }
  if (!isNode(flow)) return { ok: true };
  const root = flow;
  return paused.every((r) => isUnboundedPause(root, r.task_id)) ? { ok: false, reason: UNBOUNDED_PAUSE_REASON } : { ok: true };
}

/**
 * Replay: only a terminal execution (spec §5.2). The server's other refusals
 * (a loop iteration, a disabled or trashed flow) are its own to state; the
 * dialog shows them in its words.
 */
export function canReplay(execution: Pick<ExecutionDto, "state">): Enablement {
  return isTerminalState(execution.state) ? { ok: true } : { ok: false, reason: REPLAY_NOT_FINISHED_REASON };
}
