import { describe, expect, it } from "vitest";
import type { FlowVersionDto } from "@/api/generated/model";
import {
  canPublish,
  canRun,
  canSave,
  draftDiffersFromLive,
  liveSourceOf,
} from "@/components/workflows/draftState";

const LIVE = "id: a\nnamespace: n\ntasks:\n  - id: t\n    type: core.log\n";

describe("draftDiffersFromLive", () => {
  it("compares bytes, so a change in whitespace alone is a difference", () => {
    expect(draftDiffersFromLive(LIVE, LIVE)).toBe(false);
    expect(draftDiffersFromLive(`${LIVE} `, LIVE)).toBe(true);
    expect(draftDiffersFromLive(LIVE.replace("core.log", "core.Log"), LIVE)).toBe(true);
  });

  it("is a content comparison: an identical text built separately does not differ", () => {
    const rebuilt = ["id: a", "namespace: n", "tasks:", "  - id: t", "    type: core.log", ""].join("\n");
    expect(draftDiffersFromLive(rebuilt, LIVE)).toBe(false);
  });
});

describe("liveSourceOf", () => {
  const version = (snapshot: Record<string, unknown>): FlowVersionDto => ({
    id: "v", flow_id: "f", version: 3, snapshot, note: "", created_at: "2026-10-10T00:00:00Z",
  });

  it("reads the snapshot's frozen source", () => {
    expect(liveSourceOf(version({ source: LIVE, plan: {} }))).toBe(LIVE);
  });

  it("is undefined, not empty, when the snapshot has no string source", () => {
    expect(liveSourceOf(version({ plan: {} }))).toBeUndefined();
    expect(liveSourceOf(version({ source: 42 }))).toBeUndefined();
    expect(liveSourceOf(undefined)).toBeUndefined();
  });
});

describe("canSave", () => {
  it("needs unsaved changes and no save in flight, and says why otherwise", () => {
    expect(canSave({ dirty: true, saving: false })).toEqual({ ok: true });
    expect(canSave({ dirty: false, saving: false })).toEqual({ ok: false, reason: "No unsaved changes" });
    expect(canSave({ dirty: true, saving: true })).toEqual({ ok: false, reason: "Saving…" });
  });
});

describe("canPublish", () => {
  it("is allowed with a saved draft and a clean validation", () => {
    expect(canPublish({ dirty: false, errors: 0, validationStatus: "ok" })).toEqual({ ok: true });
  });

  it("asks for a save first while there are unsaved edits, whatever else holds", () => {
    expect(canPublish({ dirty: true, errors: 0, validationStatus: "ok" })).toEqual({ ok: false, reason: "Save first" });
    expect(canPublish({ dirty: true, errors: 3, validationStatus: "error" })).toEqual({ ok: false, reason: "Save first" });
  });

  it("counts the errors, with the page's own plural", () => {
    expect(canPublish({ dirty: false, errors: 1, validationStatus: "ok" })).toEqual({ ok: false, reason: "Fix 1 error to publish" });
    expect(canPublish({ dirty: false, errors: 4, validationStatus: "ok" })).toEqual({ ok: false, reason: "Fix 4 errors to publish" });
  });

  it("never reads a failed validation as no errors", () => {
    expect(canPublish({ dirty: false, errors: 0, validationStatus: "error" })).toEqual({ ok: false, reason: "Validation unavailable" });
  });

  it("waits for a validation in flight", () => {
    expect(canPublish({ dirty: false, errors: 0, validationStatus: "validating" })).toEqual({ ok: false, reason: "Validating the flow…" });
  });
});

describe("canRun", () => {
  it("needs a live version", () => {
    expect(canRun({ liveVersionId: "v1", manage: true })).toEqual({ ok: true });
    expect(canRun({ liveVersionId: null, manage: true })).toEqual({ ok: false, reason: "Publish the flow before running it" });
    expect(canRun({ liveVersionId: undefined, manage: true })).toEqual({ ok: false, reason: "Publish the flow before running it" });
  });

  it("needs workflow:manage", () => {
    expect(canRun({ liveVersionId: "v1", manage: false }).ok).toBe(false);
  });
});
