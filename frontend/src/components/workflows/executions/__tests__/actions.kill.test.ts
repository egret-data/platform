import { describe, expect, it } from "vitest";
import {
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

describe("showsKill", () => {
  it("shows Kill on every state that is not terminal, KILLING included", () => {
    for (const state of [...LIVE, "KILLING"]) expect(showsKill({ state })).toBe(true);
  });

  it("hides Kill once the execution has ended", () => {
    for (const state of TERMINAL) expect(showsKill({ state })).toBe(false);
  });
});

describe("canKill", () => {
  it("is enabled for every live state, PAUSED included", () => {
    for (const state of LIVE) expect(canKill({ state })).toEqual({ ok: true });
  });

  it("is disabled while the kill is in progress, saying so", () => {
    expect(canKill({ state: "KILLING" })).toEqual({ ok: false, reason: KILL_IN_PROGRESS_REASON });
    expect(KILL_IN_PROGRESS_REASON).toBe("The kill was sent; a worker is stopping the running task");
  });
});

describe("killFeedback", () => {
  it("says Killed when the route answered KILLED", () => {
    expect(killFeedback("KILLED")).toBe("Killed the execution");
  });

  it("says the kill is under way when the route answered KILLING", () => {
    expect(killFeedback("KILLING")).toBe("Killing the execution: the running task is being stopped");
  });
});
