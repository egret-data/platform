import { describe, expect, it } from "vitest";
import type { FlowVersionDto } from "@/api/generated/model";
import {
  canCreateVersion,
  canRun,
  canSave,
  changedSinceVersion,
  isFlowDisabled,
  latestVersion,
  versionSourceOf,
} from "@/components/workflows/draftState";

const LIVE = "id: a\nnamespace: n\ntasks:\n  - id: t\n    type: core.log\n";

describe("changedSinceVersion", () => {
  it("compares bytes, so a change in whitespace alone is a difference", () => {
    expect(changedSinceVersion(LIVE, LIVE)).toBe(false);
    expect(changedSinceVersion(`${LIVE} `, LIVE)).toBe(true);
    expect(changedSinceVersion(LIVE.replace("core.log", "core.Log"), LIVE)).toBe(true);
  });

  it("is a content comparison: an identical text built separately does not differ", () => {
    const rebuilt = ["id: a", "namespace: n", "tasks:", "  - id: t", "    type: core.log", ""].join("\n");
    expect(changedSinceVersion(rebuilt, LIVE)).toBe(false);
  });
});

describe("versionSourceOf", () => {
  const version = (snapshot: Record<string, unknown>): FlowVersionDto => ({
    id: "v", flow_id: "f", version: 3, snapshot, note: "", created_at: "2026-10-10T00:00:00Z",
  });

  it("reads the snapshot's frozen source", () => {
    expect(versionSourceOf(version({ source: LIVE, plan: {} }))).toBe(LIVE);
  });

  it("is undefined, not empty, when the snapshot has no string source", () => {
    expect(versionSourceOf(version({ plan: {} }))).toBeUndefined();
    expect(versionSourceOf(version({ source: 42 }))).toBeUndefined();
    expect(versionSourceOf(undefined)).toBeUndefined();
  });
});

describe("canSave", () => {
  it("needs unsaved changes and no save in flight, and says why otherwise", () => {
    expect(canSave({ dirty: true, saving: false })).toEqual({ ok: true });
    expect(canSave({ dirty: false, saving: false })).toEqual({ ok: false, reason: "No unsaved changes" });
    expect(canSave({ dirty: true, saving: true })).toEqual({ ok: false, reason: "Saving…" });
  });
});

describe("canCreateVersion", () => {
  it("is allowed with a saved draft and a clean validation", () => {
    expect(canCreateVersion({ dirty: false, errors: 0, validationStatus: "ok" })).toEqual({ ok: true });
  });

  it("asks for a save first while there are unsaved edits, whatever else holds", () => {
    expect(canCreateVersion({ dirty: true, errors: 0, validationStatus: "ok" })).toEqual({ ok: false, reason: "Save first" });
    expect(canCreateVersion({ dirty: true, errors: 3, validationStatus: "error" })).toEqual({ ok: false, reason: "Save first" });
  });

  it("counts the errors, with the page's own plural", () => {
    expect(canCreateVersion({ dirty: false, errors: 1, validationStatus: "ok" })).toEqual({ ok: false, reason: "Fix 1 error to create a version" });
    expect(canCreateVersion({ dirty: false, errors: 4, validationStatus: "ok" })).toEqual({ ok: false, reason: "Fix 4 errors to create a version" });
  });

  it("never reads a failed validation as no errors", () => {
    expect(canCreateVersion({ dirty: false, errors: 0, validationStatus: "error" })).toEqual({ ok: false, reason: "Validation unavailable" });
  });

  it("waits for a validation in flight", () => {
    expect(canCreateVersion({ dirty: false, errors: 0, validationStatus: "validating" })).toEqual({ ok: false, reason: "Validating the flow…" });
  });

  it("is refused while the page is stale, after unsaved edits and before validation", () => {
    const reason = "The flow changed on the server; reload to create a version";
    expect(canCreateVersion({ dirty: false, errors: 0, validationStatus: "ok", stale: true })).toEqual({ ok: false, reason });
    // Stale outranks validation: the text validated is not the server's draft.
    expect(canCreateVersion({ dirty: false, errors: 2, validationStatus: "error", stale: true })).toEqual({ ok: false, reason });
    // An edit is the first thing to fix, as for Run.
    expect(canCreateVersion({ dirty: true, errors: 0, validationStatus: "ok", stale: true })).toEqual({ ok: false, reason: "Save first" });
  });
});

describe("latestVersion", () => {
  const v = (version: number): FlowVersionDto => ({
    id: `v${version}`, flow_id: "f", version, snapshot: {}, note: "", created_at: "2026-10-10T00:00:00Z",
  });

  it("is the highest number, whatever order the list comes in", () => {
    expect(latestVersion([v(2), v(7), v(3)])?.version).toBe(7);
    expect(latestVersion([])).toBeUndefined();
  });
});

describe("isFlowDisabled", () => {
  it("is either door: the state column, or the YAML's own disabled: true", () => {
    expect(isFlowDisabled("disabled", LIVE)).toBe(true);
    expect(isFlowDisabled("draft", `${LIVE}disabled: true\n`)).toBe(true);
    expect(isFlowDisabled("live", LIVE)).toBe(false);
    expect(isFlowDisabled("draft", `${LIVE}disabled: false\n`)).toBe(false);
    // Only the boolean: a string is not the YAML switching the flow off.
    expect(isFlowDisabled("draft", `${LIVE}disabled: "true"\n`)).toBe(false);
    expect(isFlowDisabled("draft", "id: [unclosed\n")).toBe(false);
  });
});

describe("canRun", () => {
  const ready = { manage: true, dirty: false, errors: 0, validationStatus: "ok" as const, disabled: false };

  it("runs the saved draft: it needs no version", () => {
    expect(canRun(ready)).toEqual({ ok: true });
  });

  it("needs workflow:manage", () => {
    expect(canRun({ ...ready, manage: false }).ok).toBe(false);
  });

  it("asks for a save first, because Run uses the saved draft", () => {
    expect(canRun({ ...ready, dirty: true })).toEqual({ ok: false, reason: "Save first: Run uses the saved draft" });
  });

  it("counts the errors, with the page's own plural", () => {
    expect(canRun({ ...ready, errors: 1 })).toEqual({ ok: false, reason: "Fix 1 error to run" });
    expect(canRun({ ...ready, errors: 3 })).toEqual({ ok: false, reason: "Fix 3 errors to run" });
  });

  it("never reads a failed validation as no errors, and waits for one in flight", () => {
    expect(canRun({ ...ready, validationStatus: "error" })).toEqual({ ok: false, reason: "Validation unavailable" });
    expect(canRun({ ...ready, validationStatus: "validating" })).toEqual({ ok: false, reason: "Validating the flow…" });
  });

  it("waits for a reload when the server's saved draft is not the one this editor holds", () => {
    expect(canRun({ ...ready, stale: true })).toEqual({ ok: false, reason: "The flow changed on the server; reload to run it" });
    // Unsaved edits are named first: saving is what the user is about to do, and the 409 handles the rest.
    expect(canRun({ ...ready, stale: true, dirty: true })).toEqual({ ok: false, reason: "Save first: Run uses the saved draft" });
  });

  it("says the flow is disabled before anything a save could fix", () => {
    expect(canRun({ ...ready, disabled: true })).toEqual({ ok: false, reason: "The flow is disabled" });
    expect(canRun({ ...ready, disabled: true, dirty: true, errors: 2 })).toEqual({ ok: false, reason: "The flow is disabled" });
  });
});
