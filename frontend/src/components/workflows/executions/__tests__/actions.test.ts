import { describe, expect, it } from "vitest";
import {
  canReplay,
  canResume,
  LOOP_WAIT_REASON,
  REPLAY_NOT_FINISHED_REASON,
  resumeFeedback,
  showsResume,
} from "@/components/workflows/executions/actions";

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

describe("resumeFeedback", () => {
  it("says Resumed only once the answer is no longer paused and did not end badly", () => {
    for (const state of ["RUNNING", "SUCCESS", "WARNING"]) {
      expect(resumeFeedback(state)).toEqual({ tone: "success", message: "Resumed the execution" });
    }
  });

  it("names a bad ending in the chip's word", () => {
    expect(resumeFeedback("FAILED")).toEqual({ tone: "warning", message: "Resumed, but the execution then ended Failed." });
    expect(resumeFeedback("KILLED").message).toBe("Resumed, but the execution then ended Killed.");
    expect(resumeFeedback("CANCELLED").tone).toBe("warning");
  });

  it("keeps a defensive note when the answer is still paused", () => {
    expect(resumeFeedback("PAUSED")).toEqual({
      tone: "warning",
      message: "Resume was sent, but the execution is still paused. Refresh to see its latest state.",
    });
  });
});

describe("canResume", () => {
  const run = (state: string, resume_with: string | null) => ({ state, resume_with }) as never;

  it("is enabled when a PAUSED run is a Pause, unbounded or bounded", () => {
    for (const rw of ["NEVER", "TERMINAL:SUCCESS", "TERMINAL:FAILED"]) {
      expect(canResume({ runs: [run("SUCCESS", null), run("PAUSED", rw)] })).toEqual({ ok: true });
    }
  });

  it("is disabled with the spec's reason when only a LoopUntil waits", () => {
    expect(canResume({ runs: [run("PAUSED", "ITERATE"), run("SUCCESS", null)] })).toEqual({ ok: false, reason: LOOP_WAIT_REASON });
    expect(LOOP_WAIT_REASON).toBe("Waiting on a loop condition, not a Pause task");
  });

  it("reads only PAUSED runs: a finished pause that kept its resume_with does not count", () => {
    // A naturally ended Pause keeps `TERMINAL:SUCCESS` on its row.
    expect(canResume({ runs: [run("SUCCESS", "TERMINAL:SUCCESS"), run("PAUSED", "ITERATE")] }).ok).toBe(false);
  });
});
