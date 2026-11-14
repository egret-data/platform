import { describe, expect, it } from "vitest";
import {
  canChangeStatus,
  canChangeTaskRunState,
  canRestart,
  changeTargets,
  isLoopIteration,
  restartRefusal,
  showsChangeStatus,
  showsChangeTaskRunState,
  showsRestart,
  stateChangeRefusal,
} from "@/components/workflows/executions/actions";

const ALL_STATES = [
  "CREATED", "SUBMITTED", "QUEUED", "RESTARTED", "RESUBMITTED", "RUNNING", "RETRYING", "KILLING", "SUCCESS", "WARNING",
  "PAUSED", "BREAKPOINT", "FAILED", "KILLED", "CANCELLED", "SKIPPED", "RETRIED",
];
const ROOT = "5e5e5e5e-0000-4000-8000-0000000000cc";
const iteration = { parent_execution_id: ROOT, loop_index: 2 };

describe("restart, change status and change task-run state: what is offered", () => {
  it("offers Restart on FAILED and PAUSED only", () => {
    expect(ALL_STATES.filter((state) => showsRestart({ state }))).toEqual(["PAUSED", "FAILED"]);
  });

  it("offers Change status and Change state on the terminal states except KILLED", () => {
    const expected = ["RESUBMITTED", "SUCCESS", "WARNING", "FAILED", "CANCELLED", "SKIPPED", "RETRIED"];
    expect(ALL_STATES.filter((state) => showsChangeStatus({ state }))).toEqual(expected);
    expect(ALL_STATES.filter((state) => showsChangeTaskRunState({ state }))).toEqual(expected);
  });

  it("enables all three for a root execution", () => {
    const root = { parent_execution_id: null, loop_index: null };
    expect(canRestart(root)).toEqual({ ok: true });
    expect(canChangeStatus(root)).toEqual({ ok: true });
    expect(canChangeTaskRunState(root)).toEqual({ ok: true });
  });

  it("does not take a subflow child, which has a parent but no loop index, for an iteration", () => {
    const child = { parent_execution_id: ROOT, loop_index: null };
    expect(isLoopIteration(child)).toBe(false);
    expect(canRestart(child)).toEqual({ ok: true });
  });

  it("disables all three for a loop iteration and names the loop's execution", () => {
    for (const verdict of [canRestart(iteration), canChangeStatus(iteration), canChangeTaskRunState(iteration)]) {
      expect(verdict.ok).toBe(false);
      if (!verdict.ok) expect(verdict.reason).toContain(ROOT);
    }
  });
});

describe("changeTargets", () => {
  it("offers the four states minus the current one", () => {
    expect(changeTargets("SUCCESS")).toEqual(["WARNING", "FAILED", "CANCELLED"]);
    expect(changeTargets("FAILED")).toEqual(["SUCCESS", "WARNING", "CANCELLED"]);
  });

  it("offers all four when the current state is not one of them", () => {
    expect(changeTargets("SKIPPED")).toEqual(["SUCCESS", "WARNING", "FAILED", "CANCELLED"]);
  });
});

describe("the refusal sentences", () => {
  it("maps each restart code to its own sentence", () => {
    const codes = ["not_restartable", "nothing_to_restart", "after_execution_running", "files_pruned"];
    const sentences = codes.map((c) => restartRefusal(c));
    expect(sentences.every((s) => typeof s === "string" && s.length > 0)).toBe(true);
    expect(new Set(sentences).size).toBe(codes.length);
    expect(restartRefusal("something_else")).toBeNull();
    expect(restartRefusal(undefined)).toBeNull();
  });

  it("maps each state-change code, and tells to restart for a working-directory block", () => {
    for (const kind of ["status", "task_run"] as const) {
      for (const c of ["not_restartable", "after_execution_running", "files_pruned", "state_unchanged"]) {
        expect(stateChangeRefusal(kind, c)).toEqual(expect.any(String));
      }
      expect(stateChangeRefusal(kind, "nope")).toBeNull();
    }
    expect(stateChangeRefusal("task_run", "inside_working_directory")).toMatch(/restart the execution/i);
  });
});
