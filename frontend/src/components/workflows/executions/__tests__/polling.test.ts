import { describe, expect, it } from "vitest";
import { AFTER_RUN_FOLLOW_MS } from "@/components/workflows/executionState";
import { executionFinished, nextExecutionPollMs, nextPollMs } from "@/components/workflows/executions/polling";

function answer(state: string, runs: string[] = [], ended_at: string | null = "2026-10-10T10:00:00Z") {
  return { status: 200, data: { state, ended_at, updated_at: ended_at ?? "2026-10-10T10:00:00Z", runs: runs.map((r) => ({ state: r })) }, headers: new Headers() } as never;
}
const ENDED = Date.parse("2026-10-10T10:00:00Z");

describe("nextPollMs", () => {
  // W7a's case (ExecutionMinimal.test.tsx), moved with the function.
  it("backs off while reads keep failing: 2 s, then the time since the last good answer, capped at 30 s", () => {
    const running = answer("RUNNING");
    const done = answer("SUCCESS");
    const at = 1_000_000;
    expect(nextExecutionPollMs({ data: running, status: "success", dataUpdatedAt: at }, at + 10)).toBe(2000);
    expect(nextExecutionPollMs({ data: running, status: "error", dataUpdatedAt: at }, at + 2000)).toBe(2000);
    expect(nextExecutionPollMs({ data: running, status: "error", dataUpdatedAt: at }, at + 8000)).toBe(8000);
    expect(nextExecutionPollMs({ data: running, status: "error", dataUpdatedAt: at }, at + 600_000)).toBe(30_000);
    expect(nextExecutionPollMs({ data: done, status: "error", dataUpdatedAt: at }, at + 2000)).toBe(false);
    // Nothing ever loaded: the page's full error and Retry take over.
    expect(nextExecutionPollMs({ data: undefined, status: "error", dataUpdatedAt: 0 }, at)).toBe(false);
    // Still loading the first answer: keep the cadence.
    expect(nextExecutionPollMs({ data: undefined, status: "pending", dataUpdatedAt: 0 }, at)).toBe(2000);
  });

  it("stops for any query once its own `finished` says the answer is final, and only then", () => {
    const at = 5_000;
    const finished = (d: { live: number }) => d.live === 0;
    expect(nextPollMs({ data: { live: 2 }, status: "success", dataUpdatedAt: at }, at, finished)).toBe(2000);
    expect(nextPollMs({ data: { live: 0 }, status: "success", dataUpdatedAt: at }, at, finished)).toBe(false);
    expect(nextPollMs({ data: { live: 1 }, status: "error", dataUpdatedAt: at }, at + 12_000, finished)).toBe(12_000);
  });

  it("calls an execution final only on a 200 with a terminal state", () => {
    expect(executionFinished(answer("SUCCESS"))).toBe(true);
    expect(executionFinished(answer("PAUSED"))).toBe(false);
    expect(executionFinished({ status: 404, data: { error: "x" }, headers: new Headers() } as never)).toBe(false);
  });

  it("is not final while a terminal execution still has a live run (an afterExecution run), and is once every run is terminal", () => {
    const now = ENDED + 60_000;
    expect(executionFinished(answer("FAILED", ["RUNNING"]), now)).toBe(false);
    expect(executionFinished(answer("FAILED", ["SUCCESS", "KILLING"]), now)).toBe(false);
    expect(executionFinished(answer("FAILED", ["SUCCESS"]), now)).toBe(true);
    expect(executionFinished(answer("RUNNING", ["SUCCESS"]), now)).toBe(false);
  });

  it("keeps polling a terminal execution with a live run, then gives up once it has been terminal longer than the window", () => {
    const state = (now: number) => nextExecutionPollMs({ data: answer("SUCCESS", ["RUNNING"]), status: "success", dataUpdatedAt: now }, now);
    expect(state(ENDED + AFTER_RUN_FOLLOW_MS - 1)).toBe(2000);
    expect(state(ENDED + AFTER_RUN_FOLLOW_MS)).toBe(false);
  });
});
