import { ApiError } from "@/api/client";
import type { BulkErrorResponse, ExecutionListItemDto } from "@/api/generated/model";
import type { Enablement } from "@/components/workflows/draftState";
import { isTerminalState } from "@/components/workflows/executionState";
import { isLoopIteration, showsChangeStatus } from "@/components/workflows/executions/actions";
import { toQueryParams, type ExecutionFilters } from "@/routes/workflows/executionsListModel";

/**
 * THE BULK TOOLBAR'S MODEL (package 7f). The ten actions, and when the selection allows each.
 *
 * A list row carries the execution's state, its loop and parent fields and its flow, but not the
 * `idle` flag or the task runs the single-op predicates in `actions.ts` also read. So these rules
 * are those predicates' list-row subset (the same state tests, the same loop-iteration and
 * deleted-flow refusals); what only the server can see (a task still waiting, an afterExecution
 * run) comes back as a 400 listing the refused ids, and nothing is applied (all or none).
 */
export type BulkAction = "restart" | "kill" | "delete" | "status" | "replay" | "resume" | "pause" | "labels" | "unqueue" | "forceRun";

export const BULK_ACTIONS: { action: BulkAction; label: string }[] = [
  { action: "restart", label: "Restart" },
  { action: "kill", label: "Kill" },
  { action: "delete", label: "Delete" },
  { action: "status", label: "Change status" },
  { action: "replay", label: "Replay" },
  { action: "resume", label: "Resume" },
  { action: "pause", label: "Pause" },
  { action: "labels", label: "Edit labels" },
  { action: "unqueue", label: "Unqueue" },
  { action: "forceRun", label: "Force run" },
];

type Row = ExecutionListItemDto;

const no = (reason: string): Enablement => ({ ok: false, reason });
const YES: Enablement = { ok: true };

/** One row's verdict per action, mirroring `showsX` + `canX` of `actions.ts`. */
function rowVerdict(r: Row): Record<BulkAction, Enablement> {
  const terminal = isTerminalState(r.state);
  const iteration = isLoopIteration(r);
  const iterationReason = (verb: string) => `${verb}: a loop iteration is handled through its loop's execution`;
  return {
    restart:
      r.state !== "FAILED" && r.state !== "PAUSED"
        ? no("Only failed or paused executions can be restarted")
        : iteration
          ? no(iterationReason("Restart"))
          : YES,
    kill: terminal ? no("Only executions that have not ended can be killed") : r.state === "KILLING" ? no("An execution is already being killed") : YES,
    delete: r.parent_execution_id != null ? no("Delete the run's first execution, not a child or retry") : terminal ? YES : no("Only ended executions can be deleted; kill running ones first"),
    status: !showsChangeStatus(r) ? no("Only ended executions, killed ones excepted, can change status") : iteration ? no(iterationReason("Change status")) : YES,
    replay: !terminal ? no("Only ended executions can be replayed") : r.flow_id == null ? no("An execution whose flow was deleted permanently cannot be replayed") : YES,
    resume: r.state === "PAUSED" ? YES : no("Only paused executions can be resumed"),
    pause:
      r.state === "KILLING"
        ? no("An execution being killed cannot be paused")
        : r.state !== "RUNNING" && r.state !== "RESTARTED"
          ? no("Only running executions can be paused")
          : iteration
            ? no(iterationReason("Pause"))
            : YES,
    unqueue: r.state === "QUEUED" ? YES : no("Only queued executions can be unqueued"),
    forceRun: r.state === "QUEUED" ? YES : no("Only queued executions can be force-run"),
    labels: terminal ? YES : no("Labels can be edited once executions have ended"),
  };
}

/** Each action is enabled only when EVERY selected row qualifies; otherwise it says how many do not and why. */
export function bulkEligibility(rows: Row[]): Record<BulkAction, Enablement> {
  const out = {} as Record<BulkAction, Enablement>;
  for (const { action } of BULK_ACTIONS) {
    let bad = 0;
    let reason = "";
    for (const r of rows) {
      const v = rowVerdict(r)[action];
      if (!v.ok) {
        bad += 1;
        reason ||= v.reason;
      }
    }
    out[action] = bad === 0 ? YES : no(`${bad} of ${rows.length} selected cannot do this. ${reason}`);
  }
  return out;
}

/** The list's filter as the by-query routes take it: no paging, the time preset resolved at `now`. */
export function queryFilterParams(
  filters: ExecutionFilters,
  now: number,
  pins: { pinnedFlowId?: string; pinnedNamespace?: string },
) {
  const { limit: _limit, offset: _offset, ...rest } = toQueryParams(filters, now, pins);
  return rest;
}

/** A `400 BulkErrorResponse` out of a thrown `ApiError`; `null` for anything else. */
export function bulkErrorOf(e: unknown): BulkErrorResponse | null {
  if (!(e instanceof ApiError) || e.status !== 400) return null;
  const b = e.body as Partial<BulkErrorResponse> | undefined;
  if (!b || typeof b.message !== "string" || !Array.isArray(b.invalids)) return null;
  return { message: b.message, invalids: b.invalids };
}
