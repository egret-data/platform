import { describe, expect, it } from "vitest";
import {
  canReplay,
  canResume,
  REPLAY_NOT_FINISHED_REASON,
  showsResume,
  UNBOUNDED_PAUSE_REASON,
} from "@/components/workflows/executions/actions";

const run = (task_id: string, state: string) => ({ task_id, state }) as never;

/** A flow with one `Pause` called `gate`, inside a `Parallel` so it is found below the top level. */
function flow(gateProps: string, extra = "") {
  return [
    "id: f",
    "namespace: a.b",
    "inputs:",
    // An input with the pause's id and a `type`: it must not be mistaken for the task.
    "  - id: gate",
    "    type: STRING",
    "tasks:",
    "  - id: first",
    "    type: io.kestra.plugin.core.log.Log",
    "    message: hi",
    "  - id: fan",
    "    type: io.kestra.plugin.core.flow.Parallel",
    "    tasks:",
    "      - id: gate",
    "        type: io.kestra.plugin.core.flow.Pause",
    ...gateProps.split("\n").filter(Boolean).map((l) => `        ${l}`),
    extra,
  ].join("\n");
}

describe("canResume", () => {
  it("is disabled, with the spec's reason, when the paused run's Pause has neither pauseDuration nor timeout", () => {
    expect(canResume({ source: flow(""), runs: [run("first", "SUCCESS"), run("fan", "RUNNING"), run("gate", "PAUSED")] })).toEqual({
      ok: false,
      reason: UNBOUNDED_PAUSE_REASON,
    });
    expect(UNBOUNDED_PAUSE_REASON).toBe("This pause has no duration or timeout, so it cannot be resumed from here");
  });

  it("is enabled when the Pause has a pauseDuration, or a timeout", () => {
    const runs = [run("gate", "PAUSED")];
    expect(canResume({ source: flow("pauseDuration: PT1M"), runs })).toEqual({ ok: true });
    expect(canResume({ source: flow("timeout: PT5M"), runs })).toEqual({ ok: true });
  });

  it("reads the short spelling of Pause too", () => {
    const source = "id: f\nnamespace: a.b\ntasks:\n  - id: gate\n    type: core.pause\n";
    expect(canResume({ source, runs: [run("gate", "PAUSED")] }).ok).toBe(false);
  });

  it("judges the PAUSED run's task, not another run's", () => {
    // `first` is a Log with no duration; only `gate`, bounded, is paused.
    expect(canResume({ source: flow("pauseDuration: PT1M"), runs: [run("first", "SUCCESS"), run("gate", "PAUSED")] }).ok).toBe(true);
  });

  it("stays enabled when a paused run is not a Pause (a LoopUntil waiting for its next iteration)", () => {
    const source = "id: f\nnamespace: a.b\ntasks:\n  - id: wait\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: x\n";
    expect(canResume({ source, runs: [run("wait", "PAUSED")] }).ok).toBe(true);
  });

  it("stays enabled while one paused run could still be released", () => {
    const source = [
      "id: f",
      "namespace: a.b",
      "tasks:",
      "  - id: both",
      "    type: io.kestra.plugin.core.flow.Parallel",
      "    tasks:",
      "      - id: open",
      "        type: io.kestra.plugin.core.flow.Pause",
      "      - id: timed",
      "        type: io.kestra.plugin.core.flow.Pause",
      "        pauseDuration: PT1S",
    ].join("\n");
    expect(canResume({ source, runs: [run("open", "PAUSED"), run("timed", "PAUSED")] }).ok).toBe(true);
    expect(canResume({ source, runs: [run("open", "PAUSED"), run("timed", "SUCCESS")] }).ok).toBe(false);
  });

  it("leaves the decision to the server when the source does not parse", () => {
    expect(canResume({ source: "tasks: [\n  - id: gate", runs: [run("gate", "PAUSED")] }).ok).toBe(true);
  });
});

describe("showsResume and canReplay", () => {
  it("shows Resume only while PAUSED", () => {
    expect(showsResume({ state: "PAUSED" })).toBe(true);
    for (const state of ["RUNNING", "CREATED", "SUCCESS", "FAILED"]) expect(showsResume({ state })).toBe(false);
  });

  it("enables Replay on every terminal state and refuses the rest with the spec's reason", () => {
    for (const state of ["SUCCESS", "FAILED", "WARNING", "KILLED", "CANCELLED", "RETRIED", "SKIPPED", "RESUBMITTED"]) {
      expect(canReplay({ state })).toEqual({ ok: true });
    }
    for (const state of ["CREATED", "RUNNING", "PAUSED", "RETRYING", "QUEUED"]) {
      expect(canReplay({ state })).toEqual({ ok: false, reason: REPLAY_NOT_FINISHED_REASON });
    }
    expect(REPLAY_NOT_FINISHED_REASON).toBe("Replay is available when the execution has finished");
  });
});
