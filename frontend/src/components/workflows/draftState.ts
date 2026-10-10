import { parse } from "yaml";
import type { FlowVersionDto } from "@/api/generated/model";
import type { ServerValidation } from "@/components/workflows/editor/useServerValidation";

/**
 * The flow header's state rules (spec §5.3): the changes indicator, and when
 * Save, Create version and Run are enabled, each with the reason shown on the
 * control when it is not (FE-ACTION-4).
 *
 * **Every execution runs the saved draft** (the user's ruling, 2026-10-10):
 * API starts, schedules, webhooks and subflows all read `workflow.flows.source`
 * and never a version. A version (`POST …/publish`) is a numbered snapshot to
 * compare and restore; it changes nothing that runs.
 */

/** An action is allowed, or refused with the sentence the control shows. */
export type Enablement = { ok: true } | { ok: false; reason: string };

/**
 * Does the saved draft differ from a version? A byte comparison of the two
 * YAML texts (FE-STATE-4): it goes out again when the draft is edited back to
 * the version's text and saved. Never a timestamp: `updated_at` moves on every
 * save and on every version created, whether or not the text changed.
 */
export function changedSinceVersion(draftSource: string, versionSource: string): boolean {
  return draftSource !== versionSource;
}

/**
 * A version's YAML: `snapshot.source` (`publish.rs`, step 8, freezes the
 * draft's source there byte for byte). `undefined` when the snapshot carries
 * no string source, which the header reports as unknown rather than as "no
 * changes".
 */
export function versionSourceOf(version: FlowVersionDto | undefined): string | undefined {
  const source = version?.snapshot?.source;
  return typeof source === "string" ? source : undefined;
}

/** The highest-numbered version, which "Changes since v{n}" compares against. */
export function latestVersion(versions: FlowVersionDto[]): FlowVersionDto | undefined {
  return versions.reduce<FlowVersionDto | undefined>((top, v) => (!top || v.version > top.version ? v : top), undefined);
}

/**
 * Is the flow switched off? Either door the backend's `startable_flow`
 * closes: the `state` column, or the YAML's own `disabled: true`. A source
 * that does not parse is not known to be disabled; validation reports it.
 */
export function isFlowDisabled(state: string, source: string): boolean {
  if (state === "disabled") return true;
  try {
    const doc: unknown = parse(source);
    return typeof doc === "object" && doc !== null && (doc as { disabled?: unknown }).disabled === true;
  } catch {
    return false;
  }
}

/** Spec §5.3: Save stays enabled with validation errors; a draft may be invalid. */
export function canSave({ dirty, saving }: { dirty: boolean; saving: boolean }): Enablement {
  if (saving) return { ok: false, reason: "Saving…" };
  if (!dirty) return { ok: false, reason: "No unsaved changes" };
  return { ok: true };
}

const errorCount = (n: number) => (n === 1 ? "1 error" : `${n} errors`);

/**
 * The validation half of Create version and Run: a validation answer with no
 * errors. A failed validation request is not "no errors" (spec §10).
 * `errors` counts every error the Problems tab lists, schema and server alike.
 */
function validated(errors: number, validationStatus: ServerValidation["status"], action: string): Enablement {
  if (validationStatus === "error") return { ok: false, reason: "Validation unavailable" };
  if (errors > 0) return { ok: false, reason: `Fix ${errorCount(errors)} to ${action}` };
  if (validationStatus === "validating") return { ok: false, reason: "Validating the flow…" };
  return { ok: true };
}

/** Create version snapshots the SAVED draft, so it waits for a save and a clean validation. */
export function canCreateVersion({
  dirty,
  errors,
  validationStatus,
}: {
  dirty: boolean;
  errors: number;
  validationStatus: ServerValidation["status"];
}): Enablement {
  if (dirty) return { ok: false, reason: "Save first" };
  return validated(errors, validationStatus, "create a version");
}

/**
 * Run starts the SAVED draft, and the run form reads that draft's inputs, so
 * it waits for a save, a clean validation, and a flow that is not disabled.
 * It needs no version.
 *
 * `stale`: the server's saved draft is not the text this editor last loaded
 * or saved (someone else's save, read back by a refetch), while the editor
 * has no edits of its own. Run would start THAT text, which the editor does
 * not show and validation did not check, so it waits for a reload.
 */
export function canRun({
  manage,
  dirty,
  errors,
  validationStatus,
  disabled,
  stale = false,
}: {
  manage: boolean;
  dirty: boolean;
  errors: number;
  validationStatus: ServerValidation["status"];
  disabled: boolean;
  stale?: boolean;
}): Enablement {
  if (!manage) return { ok: false, reason: "Running a flow needs the workflow:manage permission" };
  if (disabled) return { ok: false, reason: "The flow is disabled" };
  if (dirty) return { ok: false, reason: "Save first: Run uses the saved draft" };
  if (stale) return { ok: false, reason: "The flow changed on the server; reload to run it" };
  return validated(errors, validationStatus, "run");
}
