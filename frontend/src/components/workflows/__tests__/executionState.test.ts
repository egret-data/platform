import { describe, expect, it } from "vitest";
import { AFTER_RUN_FOLLOW_MS, executionLive } from "@/components/workflows/executionState";

const ENDED = "2026-10-10T10:00:00.000001Z";
const T_END = Date.parse(ENDED);
const exec = (state: string, runs: string[], over: { ended_at?: string | null; updated_at?: string } = {}) => ({
  state,
  ended_at: ENDED,
  updated_at: "2026-10-10T10:00:01Z",
  runs: runs.map((r) => ({ state: r })),
  ...over,
});

describe("executionLive (end hooks D15)", () => {
  it("is live while the execution itself is not terminal, whatever its runs", () => {
    expect(executionLive(exec("RUNNING", ["SUCCESS"]), T_END)).toBe(true);
    expect(executionLive(exec("KILLING", []), T_END)).toBe(true);
  });

  it("is live for a terminal execution that still has a non-terminal run, shortly after it ended", () => {
    expect(executionLive(exec("SUCCESS", ["SUCCESS", "RUNNING"]), T_END + 1000)).toBe(true);
    expect(executionLive(exec("FAILED", ["KILLING"]), T_END + 1000)).toBe(true);
  });

  it("is not live once every run is terminal", () => {
    expect(executionLive(exec("SUCCESS", ["SUCCESS", "FAILED"]), T_END + 1000)).toBe(false);
    expect(executionLive(exec("SUCCESS", []), T_END + 1000)).toBe(false);
  });

  it("stops following a live run once the execution has been terminal for the whole window", () => {
    const e = exec("SUCCESS", ["RUNNING"]);
    expect(AFTER_RUN_FOLLOW_MS).toBe(30 * 60 * 1000);
    expect(executionLive(e, T_END + AFTER_RUN_FOLLOW_MS - 1)).toBe(true);
    expect(executionLive(e, T_END + AFTER_RUN_FOLLOW_MS)).toBe(false);
  });

  it("measures the window from updated_at when no end is recorded", () => {
    const e = exec("SUCCESS", ["RUNNING"], { ended_at: null, updated_at: ENDED });
    expect(executionLive(e, T_END + 1000)).toBe(true);
    expect(executionLive(e, T_END + AFTER_RUN_FOLLOW_MS)).toBe(false);
  });
});
