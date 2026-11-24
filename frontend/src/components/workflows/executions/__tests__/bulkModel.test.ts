import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import type { ExecutionListItemDto } from "@/api/generated/model";
import { bulkErrorOf, bulkEligibility, queryFilterParams } from "@/components/workflows/executions/bulkModel";
import { readFilters, type ExecutionFilters } from "@/routes/workflows/executionsListModel";

function row(over: Partial<ExecutionListItemDto> = {}): ExecutionListItemDto {
  return {
    id: "e1",
    flow_id: "f1",
    namespace: "ns",
    flow_key: "flow",
    state: "SUCCESS",
    history: [],
    inputs: {},
    seq: 1,
    attempt_number: 1,
    origin_offset_ms: 0,
    created_at: "2026-10-11T10:00:00Z",
    updated_at: "2026-10-11T10:00:00Z",
    ...over,
  };
}

describe("bulkEligibility", () => {
  it("offers every action only when every selected row qualifies", () => {
    const failed = row({ state: "FAILED" });
    const running = row({ id: "e2", state: "RUNNING" });
    const e = bulkEligibility([failed, running]);
    expect(e.restart.ok).toBe(false);
    expect(e.kill.ok).toBe(false);
    expect(e.delete.ok).toBe(false);
    expect(bulkEligibility([failed]).restart.ok).toBe(true);
    expect(bulkEligibility([running]).kill.ok).toBe(true);
    expect(bulkEligibility([running]).pause.ok).toBe(true);
  });

  it("offers unqueue and force run only when every row is QUEUED", () => {
    const queued = row({ state: "QUEUED" });
    expect(bulkEligibility([queued]).unqueue.ok).toBe(true);
    expect(bulkEligibility([queued]).forceRun.ok).toBe(true);
    const mixed = bulkEligibility([queued, row({ id: "e2", state: "RUNNING" })]);
    expect(mixed.unqueue.ok).toBe(false);
    expect(mixed.forceRun.ok).toBe(false);
  });

  it("says why an action is unavailable", () => {
    const e = bulkEligibility([row({ state: "SUCCESS" })]);
    expect(e.kill).toEqual({ ok: false, reason: expect.stringMatching(/not ended|finished|running/i) });
    expect(e.resume.ok).toBe(false);
    expect(e.delete.ok).toBe(true);
    expect(e.replay.ok).toBe(true);
    expect(e.labels.ok).toBe(true);
    expect(e.status.ok).toBe(true);
  });

  it("refuses a loop iteration for restart, pause and status, a child for delete, a deleted flow for replay", () => {
    const iteration = row({ state: "FAILED", parent_execution_id: "p", loop_index: 0 });
    expect(bulkEligibility([iteration]).restart.ok).toBe(false);
    expect(bulkEligibility([iteration]).delete.ok).toBe(false);
    expect(bulkEligibility([iteration]).status.ok).toBe(false);
    expect(bulkEligibility([row({ flow_id: null })]).replay.ok).toBe(false);
    expect(bulkEligibility([row({ state: "PAUSED" })]).resume.ok).toBe(true);
  });

  it("KILLED cannot change status", () => {
    expect(bulkEligibility([row({ state: "KILLED" })]).status.ok).toBe(false);
  });
});

describe("queryFilterParams", () => {
  const f: ExecutionFilters = { ...readFilters(new URLSearchParams(), { pinned: false, pinnedNamespace: false }), states: ["FAILED"], namespace: "a", showChildren: false };
  it("sends the filter, never the paging", () => {
    const q = queryFilterParams(f, Date.now(), {});
    expect(q.state).toBe("FAILED");
    expect(q.namespace).toBe("a");
    expect(q.root_only).toBe(true);
    expect("limit" in q).toBe(false);
    expect("offset" in q).toBe(false);
  });
});

describe("bulkErrorOf", () => {
  it("reads a 400 BulkErrorResponse", () => {
    const e = new ApiError(400, "x", undefined, { message: "no", invalids: [{ id: "a", reason: "running" }] });
    expect(bulkErrorOf(e)).toEqual({ message: "no", invalids: [{ id: "a", reason: "running" }] });
  });
  it("is null for anything else", () => {
    expect(bulkErrorOf(new ApiError(500, "boom"))).toBeNull();
    expect(bulkErrorOf(new Error("x"))).toBeNull();
    expect(bulkErrorOf(new ApiError(400, "x", undefined, { error: "e" }))).toBeNull();
  });
});
