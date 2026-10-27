import { describe, expect, it } from "vitest";
import { AFTER_RUN_FOLLOW_MS } from "@/components/workflows/executionState";
import {
  AFTER_RUN_KILL_FEEDBACK,
  canKill,
  KILL_IN_PROGRESS_REASON,
  killFeedback,
  showsKill,
} from "@/components/workflows/executions/actions";

/**
 * Written out, not imported from `executionState.ts`: a state moved into or out
 * of the terminal set there fails here instead of moving the expectation with it.
 */
const LIVE = ["CREATED", "SUBMITTED", "QUEUED", "RESTARTED", "RUNNING", "RETRYING", "PAUSED", "BREAKPOINT"];
const TERMINAL = ["SUCCESS", "WARNING", "FAILED", "KILLED", "CANCELLED", "SKIPPED", "RETRIED", "RESUBMITTED"];

const ENDED = "2026-10-10T10:00:00Z";
const NOW = Date.parse(ENDED) + 60_000;
/** A finished execution's other fields; its runs are all terminal unless a case says so. */
const E = { ended_at: ENDED, updated_at: ENDED, runs: [{ state: "SUCCESS" }] };
const withRuns = (state: string, ...runs: string[]) => ({ ...E, state, runs: runs.map((r) => ({ state: r })) });

describe("showsKill", () => {
  it("shows Kill on a terminal execution while an afterExecution run is live, until the follow window passes", () => {
    for (const state of TERMINAL) expect(showsKill(withRuns(state, "SUCCESS", "RUNNING"), NOW)).toBe(true);
    expect(showsKill(withRuns("SUCCESS", "SUCCESS", "RUNNING"), Date.parse(ENDED) + AFTER_RUN_FOLLOW_MS)).toBe(false);
  });

  it("shows Kill on every state that is not terminal, KILLING included", () => {
    for (const state of [...LIVE, "KILLING"]) expect(showsKill({ ...E, state }, NOW)).toBe(true);
  });

  it("hides Kill once the execution has ended", () => {
    for (const state of TERMINAL) expect(showsKill({ ...E, state }, NOW)).toBe(false);
  });
});

describe("canKill", () => {
  it("is enabled for a terminal execution with a live run, and disabled, saying why, once that run is being killed", () => {
    expect(canKill(withRuns("SUCCESS", "RUNNING"))).toEqual({ ok: true });
    expect(canKill(withRuns("SUCCESS", "KILLING"))).toEqual({ ok: false, reason: KILL_IN_PROGRESS_REASON });
  });

  it("is enabled for every live state, PAUSED included", () => {
    for (const state of LIVE) expect(canKill({ ...E, state })).toEqual({ ok: true });
  });

  it("is disabled while the kill is in progress, saying so", () => {
    expect(canKill({ ...E, state: "KILLING" })).toEqual({ ok: false, reason: KILL_IN_PROGRESS_REASON });
    expect(KILL_IN_PROGRESS_REASON).toBe("The kill was sent; a worker is stopping the running task");
  });
});

describe("killFeedback", () => {
  it("names the phase when the kill stopped the afterExecution tasks of an ended execution, whatever state the route answered", () => {
    for (const answered of ["SUCCESS", "FAILED", "KILLED"]) expect(killFeedback(answered, true)).toBe(AFTER_RUN_KILL_FEEDBACK);
    expect(AFTER_RUN_KILL_FEEDBACK).toBe("Stopping the afterExecution tasks");
  });

  it("says Killed when the route answered KILLED", () => {
    expect(killFeedback("KILLED")).toBe("Killed the execution");
  });

  it("says the kill is under way when the route answered KILLING", () => {
    expect(killFeedback("KILLING")).toBe("Killing the execution: the running task is being stopped");
  });
});
