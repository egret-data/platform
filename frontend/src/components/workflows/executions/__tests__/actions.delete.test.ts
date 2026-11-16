import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import {
  DELETE_NOT_IDLE_REASON,
  LABELS_NOT_IDLE_REASON,
  canDelete,
  canEditLabels,
  deleteRefusal,
  dependentsOf,
  labelsRefusal,
} from "@/components/workflows/executions/actions";

describe("canDelete", () => {
  it("allows an idle root", () => {
    expect(canDelete({ idle: true, parent_execution_id: null })).toEqual({ ok: true });
  });
  it("says why while it runs", () => {
    expect(canDelete({ idle: false, parent_execution_id: null })).toEqual({ ok: false, reason: DELETE_NOT_IDLE_REASON });
  });
  it("names the execution an iteration or retry belongs to", () => {
    const got = canDelete({ idle: true, parent_execution_id: "root-1" });
    expect(got.ok).toBe(false);
    expect(!got.ok && got.reason).toContain("root-1");
  });
});

describe("canEditLabels", () => {
  it("follows idle", () => {
    expect(canEditLabels({ idle: true })).toEqual({ ok: true });
    expect(canEditLabels({ idle: false })).toEqual({ ok: false, reason: LABELS_NOT_IDLE_REASON });
  });
});

describe("refusals", () => {
  it("has a sentence for each delete code and none for others", () => {
    for (const code of ["not_terminated", "not_a_root", "has_dependents"]) expect(deleteRefusal(code)).toEqual(expect.any(String));
    expect(deleteRefusal("delete_logs_required")).toBeNull();
    expect(deleteRefusal(undefined)).toBeNull();
    expect(labelsRefusal("not_terminated")).toEqual(expect.any(String));
    expect(labelsRefusal("invalid_labels")).toBeNull();
  });
  it("reads has_dependents' details", () => {
    const e = new ApiError(409, "2 other execution(s)…", "has_dependents", {
      error: "…",
      code: "has_dependents",
      details: { executions: ["a", "b"], total: 3 },
    });
    expect(dependentsOf(e)).toEqual({ executions: ["a", "b"], total: 3 });
    expect(dependentsOf(new ApiError(409, "x", "not_terminated"))).toBeNull();
    expect(dependentsOf(new Error("x"))).toBeNull();
  });
});
