import { describe, expect, it } from "vitest";
import type { TaskRunDto } from "@/api/generated/model";
import {
  axisTicks,
  buildGantt,
  markerReason,
  visibleRows,
  type GanttExecution,
} from "@/components/workflows/executions/gantt/ganttModel";

/** The execution's `created_at`: the base every `attempts[]` millisecond counts from. */
const ORIGIN = "2026-10-11T10:00:00.000001Z";
const T = (iso: string) => new Date(iso).getTime();
const AT = (ms: number) => T(ORIGIN) + ms;
const NOW = T("2026-10-11T10:05:00Z");

function run(run_ix: number, task_id: string, over: Partial<TaskRunDto> = {}): TaskRunDto {
  return {
    id: `r-${run_ix}`,
    execution_id: "e",
    run_ix,
    task_ix: run_ix,
    task_id,
    parent_ix: null,
    value: null,
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    attempts: [],
    outputs: {},
    created_seq: run_ix,
    ended_seq: run_ix + 1,
    started_at: null,
    ended_at: null,
    // Neither of these is a start or an end of anything below unless a case says so:
    // a model that read them by mistake draws a visibly different bar.
    created_at: "2026-10-11T10:00:00.500001Z",
    updated_at: "2026-10-11T10:04:00.000001Z",
    ...over,
  };
}

function execution(runs: TaskRunDto[], over: Partial<GanttExecution> = {}): GanttExecution {
  return {
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    created_at: ORIGIN,
    started_at: "2026-10-11T10:00:01.000001Z",
    ended_at: "2026-10-11T10:01:05.000001Z",
    updated_at: "2026-10-11T10:04:30.000001Z",
    runs,
    ...over,
  };
}

const rowOf = (g: ReturnType<typeof buildGantt>, ix: number) => {
  const r = g.rows.find((x) => x.runIx === ix);
  if (!r) throw new Error(`no row ${ix}`);
  return r;
};

describe("buildGantt: bars", () => {
  it("draws a run from started_at to ended_at, closed and exact", () => {
    const g = buildGantt(
      execution([run(0, "extract", { started_at: "2026-10-11T10:00:02Z", ended_at: "2026-10-11T10:00:09Z" })]),
      NOW,
    );
    const r = rowOf(g, 0);
    expect(r.bar).toEqual({ start: T("2026-10-11T10:00:02Z"), end: T("2026-10-11T10:00:09Z"), open: false });
    expect(r.estimated).toBe(false);
    expect(r.marker).toBe(false);
  });

  it("opens a run that has not ended to now, also while paused", () => {
    const g = buildGantt(
      execution(
        [
          run(0, "work", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_seq: null, started_at: "2026-10-11T10:00:02Z" }),
          run(1, "gate", { state: "PAUSED", history: ["CREATED", "RUNNING", "PAUSED"], ended_seq: null, started_at: "2026-10-11T10:00:03Z" }),
        ],
        { state: "RUNNING", ended_at: null },
      ),
      NOW,
    );
    expect(rowOf(g, 0).bar).toEqual({ start: T("2026-10-11T10:00:02Z"), end: NOW, open: true });
    expect(rowOf(g, 1).bar).toEqual({ start: T("2026-10-11T10:00:03Z"), end: NOW, open: true });
    expect(rowOf(g, 0).estimated).toBe(false);
    expect(g.live).toBe(true);
  });
});

describe("buildGantt: attempt segments", () => {
  it("places each attempt at created_at + ms, and a retried attempt keeps its own end state", () => {
    const g = buildGantt(
      execution([
        run(0, "flaky", {
          state: "SUCCESS",
          history: ["CREATED", "RUNNING", "FAILED", "RETRYING", "RUNNING", "SUCCESS"],
          started_at: "2026-10-11T10:00:01.000001Z",
          ended_at: "2026-10-11T10:00:07.000001Z",
          attempts: [
            { started_at: 1000, ended_at: 3000, history: ["RUNNING", "FAILED"] },
            { started_at: 5000, ended_at: 7000, history: ["RUNNING", "SUCCESS"] },
          ],
        }),
      ]),
      NOW,
    );
    expect(rowOf(g, 0).attempts).toEqual([
      { start: AT(1000), end: AT(3000), state: "FAILED" },
      { start: AT(5000), end: AT(7000), state: "SUCCESS" },
    ]);
  });

  it("leaves the open attempt of a running run without an end", () => {
    const g = buildGantt(
      execution(
        [
          run(0, "work", {
            state: "RUNNING",
            history: ["CREATED", "RUNNING", "FAILED", "RETRYING", "RUNNING"],
            ended_seq: null,
            started_at: "2026-10-11T10:00:01.000001Z",
            attempts: [
              { started_at: 1000, ended_at: 2000, history: ["RUNNING", "FAILED"] },
              { started_at: 4000, history: ["RUNNING"] },
            ],
          }),
        ],
        { state: "RUNNING", ended_at: null },
      ),
      NOW,
    );
    expect(rowOf(g, 0).attempts).toEqual([
      { start: AT(1000), end: AT(2000), state: "FAILED" },
      { start: AT(4000), state: "RUNNING" },
    ]);
  });

  it("ends an attempt recorded without an end at the next attempt's start, the last at the run's end", () => {
    const g = buildGantt(
      execution([
        run(0, "legacy", {
          history: ["CREATED", "RUNNING", "FAILED", "RETRYING", "RUNNING", "SUCCESS"],
          started_at: "2026-10-11T10:00:01.000001Z",
          ended_at: "2026-10-11T10:00:09.000001Z",
          attempts: [
            { started_at: 1000, history: ["RUNNING", "FAILED"] },
            { started_at: 6000, history: ["RUNNING", "SUCCESS"] },
          ],
        }),
      ]),
      NOW,
    );
    expect(rowOf(g, 0).attempts).toEqual([
      { start: AT(1000), end: AT(6000), state: "FAILED" },
      { start: AT(6000), end: AT(9000), state: "SUCCESS" },
    ]);
  });
});

describe("buildGantt: never-started markers", () => {
  it("marks a run that never ran instead of drawing a bar: CREATED, SKIPPED, cancelled at CREATED", () => {
    const g = buildGantt(
      execution([
        run(0, "later", { state: "CREATED", history: ["CREATED"], ended_seq: null }),
        run(1, "skipped", { state: "SKIPPED", history: ["CREATED", "SKIPPED"], ended_at: "2026-10-11T10:00:04Z" }),
        // An SLA cancel at CREATED: an end was recorded, and nothing ever ran.
        run(2, "cancelled", { state: "CANCELLED", history: ["CREATED", "CANCELLED"], ended_at: "2026-10-11T10:00:06Z" }),
      ]),
      NOW,
    );
    for (const ix of [0, 1, 2]) {
      const r = rowOf(g, ix);
      expect(r.marker).toBe(true);
      expect(r.bar).toBeUndefined();
      // Never an estimated bar for a run that never ran.
      expect(r.estimated).toBe(false);
    }
    // Where it was decided: the recorded end, or the creation while nothing has happened.
    expect(rowOf(g, 0).markerAt).toBe(T("2026-10-11T10:00:00.500001Z"));
    expect(rowOf(g, 1).markerAt).toBe(T("2026-10-11T10:00:04Z"));
    expect(rowOf(g, 2).markerAt).toBe(T("2026-10-11T10:00:06Z"));
  });

  it("says why there is no bar", () => {
    expect(markerReason("SKIPPED")).toMatch(/skipped/i);
    expect(markerReason("CANCELLED")).toMatch(/cancelled before it started/i);
    expect(markerReason("KILLED")).toBe("Killed before it started.");
    expect(markerReason("CREATED")).toMatch(/not started/i);
  });
});

describe("buildGantt: the estimated fallback (spec §6.3)", () => {
  it("uses created_at + attempts[0].started_at and updated_at once ended_seq is set, hatched", () => {
    const g = buildGantt(
      execution(
        [
          run(0, "old", {
            ended_seq: 3,
            attempts: [{ started_at: 4000, history: ["RUNNING", "SUCCESS"] }],
            updated_at: "2026-10-11T10:00:20.000001Z",
          }),
        ],
        { started_at: null, ended_at: null },
      ),
      NOW,
    );
    const r = rowOf(g, 0);
    expect(r.bar).toEqual({ start: AT(4000), end: T("2026-10-11T10:00:20.000001Z"), open: false });
    expect(r.estimated).toBe(true);
    expect(g.estimated).toBe(true);
  });

  it("starts a run with no attempts (a flowable) at its own created_at", () => {
    const g = buildGantt(execution([run(0, "seq", { type_id: null, attempts: [] })], { started_at: null, ended_at: null }), NOW);
    expect(rowOf(g, 0).bar?.start).toBe(T("2026-10-11T10:00:00.500001Z"));
    expect(rowOf(g, 0).estimated).toBe(true);
  });

  it("judges each field on its own: a run in flight at deploy keeps its real end", () => {
    const g = buildGantt(
      execution([
        run(0, "straddle", {
          started_at: null,
          ended_at: "2026-10-11T10:00:12.000001Z",
          attempts: [{ started_at: 2000, history: ["RUNNING", "SUCCESS"] }],
        }),
      ]),
      NOW,
    );
    const r = rowOf(g, 0);
    expect(r.bar).toEqual({ start: AT(2000), end: T("2026-10-11T10:00:12.000001Z"), open: false });
    expect(r.estimated).toBe(true);
  });

  it("keeps a legacy run with no ended_seq open to now, estimated", () => {
    const g = buildGantt(
      execution(
        [run(0, "going", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_seq: null, attempts: [{ started_at: 3000, history: ["RUNNING"] }] })],
        { state: "RUNNING", ended_at: null, started_at: null },
      ),
      NOW,
    );
    expect(rowOf(g, 0).bar).toEqual({ start: AT(3000), end: NOW, open: true });
    expect(rowOf(g, 0).estimated).toBe(true);
  });

  it("closes a terminal run with no ended_seq at updated_at rather than leaving it open (a departure from gantt_rows)", () => {
    const g = buildGantt(
      execution(
        [run(0, "done", { state: "FAILED", history: ["CREATED", "RUNNING", "FAILED"], ended_seq: null, started_at: "2026-10-11T10:00:02Z", updated_at: "2026-10-11T10:00:30Z" })],
        { state: "RUNNING", ended_at: null },
      ),
      NOW,
    );
    expect(rowOf(g, 0).bar).toEqual({ start: T("2026-10-11T10:00:02Z"), end: T("2026-10-11T10:00:30Z"), open: false });
    expect(rowOf(g, 0).estimated).toBe(true);
  });

  it("caps a run that never ended inside an execution that has, and keeps the axis to the execution", () => {
    // Viewed three days later: the page's clock must not become the run's end, or the axis's.
    const later = T("2026-10-14T10:00:00Z");
    const g = buildGantt(
      execution(
        [
          run(0, "stuck", {
            state: "RUNNING",
            history: ["CREATED", "RUNNING"],
            ended_seq: null,
            started_at: "2026-10-11T10:00:02Z",
            attempts: [{ started_at: 2000, history: ["RUNNING"] }],
          }),
        ],
        { state: "FAILED", ended_at: "2026-10-11T10:01:40.000001Z" },
      ),
      later,
    );
    const r = rowOf(g, 0);
    expect(r.bar).toEqual({ start: T("2026-10-11T10:00:02Z"), end: T("2026-10-11T10:01:40.000001Z"), open: false });
    expect(r.estimated).toBe(true);
    expect(r.attempts).toEqual([{ start: AT(2000), end: T("2026-10-11T10:01:40.000001Z"), state: "RUNNING" }]);
    expect(g.axis.end).toBe(T("2026-10-11T10:01:40.000001Z"));
  });

  it("is not estimated when both times are recorded, and the chart says so", () => {
    const g = buildGantt(
      execution([run(0, "a", { started_at: "2026-10-11T10:00:02Z", ended_at: "2026-10-11T10:00:03Z" })]),
      NOW,
    );
    expect(g.estimated).toBe(false);
  });
});

describe("buildGantt: nesting", () => {
  // Unsorted on purpose: the wire order is run_ix, but nothing here may depend on it.
  const runs = [
    run(4, "inner-b", { parent_ix: 1 }),
    run(2, "after", {}),
    run(0, "first", {}),
    run(5, "deep", { parent_ix: 4, value: "eu-west" }),
    run(1, "parallel", { parent_ix: null }),
    run(3, "inner-a", { parent_ix: 1, value: "us-east" }),
    // A parent that is not in the list (a pruned or unread run) puts the row at the top level.
    run(6, "orphan", { parent_ix: 99 }),
    run(7, "last", {}),
  ];

  it("orders rows depth-first, siblings by run_ix, with depth and parent from parent_ix", () => {
    const g = buildGantt(execution(runs), NOW);
    expect(g.rows.map((r) => [r.runIx, r.depth, r.parentIx])).toEqual([
      [0, 0, null],
      [1, 0, null],
      [3, 1, 1],
      [4, 1, 1],
      [5, 2, 4],
      [2, 0, null],
      // Among the roots by run_ix, not appended after them.
      [6, 0, null],
      [7, 0, null],
    ]);
    expect(rowOf(g, 1).hasChildren).toBe(true);
    expect(rowOf(g, 2).hasChildren).toBe(false);
  });

  it("labels a run by task id and value", () => {
    const g = buildGantt(execution(runs), NOW);
    expect(rowOf(g, 3)).toMatchObject({ taskId: "inner-a", value: "us-east" });
    expect(rowOf(g, 0)).toMatchObject({ taskId: "first", value: null });
  });

  it("hides every descendant of a collapsed row, and nothing else", () => {
    const g = buildGantt(execution(runs), NOW);
    expect(visibleRows(g.rows, new Set([1])).map((r) => r.runIx)).toEqual([0, 1, 2, 6, 7]);
    expect(visibleRows(g.rows, new Set([4])).map((r) => r.runIx)).toEqual([0, 1, 3, 4, 2, 6, 7]);
    expect(visibleRows(g.rows, new Set()).map((r) => r.runIx)).toEqual([0, 1, 3, 4, 5, 2, 6, 7]);
  });

  it("survives a parent_ix cycle without looping", () => {
    const g = buildGantt(execution([run(1, "a", { parent_ix: 2 }), run(2, "b", { parent_ix: 1 })]), NOW);
    expect(g.rows.map((r) => r.runIx).sort()).toEqual([1, 2]);
  });
});

describe("buildGantt: the axis", () => {
  it("spans the execution's recorded start and end once it has finished", () => {
    const g = buildGantt(
      execution([run(0, "a", { started_at: "2026-10-11T10:00:02Z", ended_at: "2026-10-11T10:00:09Z" })]),
      NOW,
    );
    expect(g.axis).toEqual({ start: T("2026-10-11T10:00:01.000001Z"), end: T("2026-10-11T10:01:05.000001Z") });
    expect(g.live).toBe(false);
  });

  it("extends to now while the execution runs", () => {
    const g = buildGantt(
      execution([run(0, "a", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_seq: null, started_at: "2026-10-11T10:00:02Z" })], {
        state: "RUNNING",
        ended_at: null,
      }),
      NOW,
    );
    expect(g.axis).toEqual({ start: T("2026-10-11T10:00:01.000001Z"), end: NOW });
  });

  it("extends to now while it runs even when every run so far has ended", () => {
    // Between two tasks: the first ended, the next is not created yet; no bar reaches now.
    const g = buildGantt(
      execution([run(0, "a", { started_at: "2026-10-11T10:00:02Z", ended_at: "2026-10-11T10:00:09Z" })], {
        state: "RUNNING",
        ended_at: null,
      }),
      NOW,
    );
    expect(g.axis.end).toBe(NOW);
  });

  it("falls back to created_at and updated_at for an execution recorded before its times were kept", () => {
    const g = buildGantt(execution([], { started_at: null, ended_at: null }), NOW);
    expect(g.axis).toEqual({ start: T(ORIGIN), end: T("2026-10-11T10:04:30.000001Z") });
  });

  it("widens to hold a bar that falls outside the execution's own span", () => {
    const g = buildGantt(
      execution([run(0, "a", { started_at: "2026-10-11T10:00:00.200001Z", ended_at: "2026-10-11T10:01:30Z" })]),
      NOW,
    );
    expect(g.axis).toEqual({ start: T("2026-10-11T10:00:00.200001Z"), end: T("2026-10-11T10:01:30Z") });
  });
});

describe("axisTicks", () => {
  it("picks a round step whose labels stay at least 90 px apart", () => {
    const axis = { start: 1_000_000, end: 1_000_000 + 65_000 };
    const ticks = axisTicks(axis, 450);
    expect(ticks.map((t) => t.label)).toEqual(["0 s", "15 s", "30 s", "45 s", "1 min"]);
    expect(ticks.map((t) => t.at - axis.start)).toEqual([0, 15_000, 30_000, 45_000, 60_000]);
    for (let i = 1; i < ticks.length; i++) {
      expect(((ticks[i].at - ticks[i - 1].at) / 65_000) * 450).toBeGreaterThanOrEqual(90);
    }
  });

  it("goes below a second for a short execution, and to hours for a long one", () => {
    expect(axisTicks({ start: 0, end: 1_200 }, 600).map((t) => t.label)).toEqual(["0 s", "0.2 s", "0.4 s", "0.6 s", "0.8 s", "1 s", "1.2 s"]);
    expect(axisTicks({ start: 0, end: 3 * 3_600_000 }, 400).map((t) => t.label)).toEqual(["0 s", "1 h", "2 h", "3 h"]);
    expect(axisTicks({ start: 0, end: 150_000 }, 450).map((t) => t.label)).toEqual([
      "0 s",
      "30 s",
      "1 min",
      "1 min 30 s",
      "2 min",
      "2 min 30 s",
    ]);
  });

  it("keeps at least the origin on a very narrow chart", () => {
    expect(axisTicks({ start: 0, end: 10_000 }, 40).map((t) => t.label)).toEqual(["0 s"]);
  });
});
