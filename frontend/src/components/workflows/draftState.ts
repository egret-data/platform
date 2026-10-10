import type { FlowVersionDto } from "@/api/generated/model";
import type { ServerValidation } from "@/components/workflows/editor/useServerValidation";

/**
 * The flow header's state rules (spec §5.3): the draft indicator, and when
 * Save, Publish and Run are enabled, each with the reason shown on the control
 * when it is not (FE-ACTION-4).
 */

/** An action is allowed, or refused with the sentence the control shows. */
export type Enablement = { ok: true } | { ok: false; reason: string };

/**
 * Does the saved draft differ from the live version? A byte comparison of the
 * two YAML texts (FE-STATE-4): it goes out again when the draft is edited back
 * to the live text and saved. Never a timestamp: `updated_at` moves on every
 * save and on every publish, whether or not the text changed.
 */
export function draftDiffersFromLive(draftSource: string, liveSource: string): boolean {
  return draftSource !== liveSource;
}

/**
 * The live version's YAML, as published: `snapshot.source` (`publish.rs`,
 * step 8, freezes the draft's source there byte for byte). `undefined` when
 * the snapshot carries no string source, which the header reports as unknown
 * rather than as "no difference".
 */
export function liveSourceOf(version: FlowVersionDto | undefined): string | undefined {
  const source = version?.snapshot?.source;
  return typeof source === "string" ? source : undefined;
}

/** Spec §5.3: Save stays enabled with validation errors; a draft may be invalid. */
export function canSave({ dirty, saving }: { dirty: boolean; saving: boolean }): Enablement {
  if (saving) return { ok: false, reason: "Saving…" };
  if (!dirty) return { ok: false, reason: "No unsaved changes" };
  return { ok: true };
}

/**
 * Publish creates the version executions run, from the SAVED draft. So it
 * waits for the editor to be saved, and for a validation answer with no
 * errors. A failed validation request is not "no errors" (spec §10).
 * `errors` counts every error the Problems tab lists, schema and server alike.
 */
export function canPublish({
  dirty,
  errors,
  validationStatus,
}: {
  dirty: boolean;
  errors: number;
  validationStatus: ServerValidation["status"];
}): Enablement {
  if (dirty) return { ok: false, reason: "Save first" };
  if (validationStatus === "error") return { ok: false, reason: "Validation unavailable" };
  if (errors > 0) {
    return { ok: false, reason: errors === 1 ? "Fix 1 error to publish" : `Fix ${errors} errors to publish` };
  }
  if (validationStatus === "validating") return { ok: false, reason: "Validating the flow…" };
  return { ok: true };
}

/** Run starts the live version (W1), so it needs one. */
export function canRun({ liveVersionId, manage }: { liveVersionId: string | null | undefined; manage: boolean }): Enablement {
  if (!manage) return { ok: false, reason: "Running a flow needs the workflow:manage permission" };
  if (!liveVersionId) return { ok: false, reason: "Publish the flow before running it" };
  return { ok: true };
}
