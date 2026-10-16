import { parse } from "yaml";
import type { CreateBackfillRequest } from "@/api/generated/model";
import { requestFromForm, isEmpty, validateForm, serverErrorInput, type FormState, type RunInput } from "@/components/workflows/run/inputModel";
import { triggerBlock } from "@/components/workflows/triggers/triggerYaml";

/**
 * The create form's pure rules (W7f spec §4.3): the local date-times in the
 * body's UTC instants, the labels' rows, what keeps the submit off, and which
 * field a server refusal belongs to. Pure so the dialog and its tests say the
 * same things. Its module is lazy with the dialog (`scripts/assert-workflows-lazy.mjs`).
 */

/** One label row as the form holds it; a row with neither key nor value is dropped from the body. */
export type LabelRow = { key: string; value: string };

/** The whole form: the window as `datetime-local` text, the labels, and the inputs. */
export type BackfillForm = { start: string; end: string; labels: LabelRow[]; inputs: FormState };

/** A `datetime-local` value (local time, seconds included) for an instant. */
export function localValue(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** The UTC instant of a local `datetime-local` value, or null when it is not a date-time. */
export function instantFromLocal(local: string): Date | null {
  if (local.trim() === "") return null;
  const d = new Date(local);
  return Number.isNaN(d.getTime()) ? null : d;
}

/** A label row's own error, in the server's words where it has them (FE-FORM-4); null for a usable or a fully empty row. */
export function labelRowError(row: LabelRow): string | null {
  const key = row.key.trim();
  const value = row.value.trim();
  if (key.startsWith("system.")) return "Keys starting with system. are reserved";
  if ((key === "") !== (value === "")) return "Fill both the key and the value";
  return null;
}

/** The labels the body carries: rows with a key, in the order given. */
export function labelsFromForm(rows: LabelRow[]): { key: string; value: string }[] {
  return rows.filter((r) => r.key.trim() !== "" || r.value.trim() !== "").map((r) => ({ key: r.key.trim(), value: r.value.trim() }));
}

/**
 * The create body (§3.4): the window's instants, the non-empty inputs in the
 * run form's wire shape, and the labels. An empty end is left out: the server
 * ends the window at its own now.
 */
export function bodyFromForm(f: BackfillForm): CreateBackfillRequest {
  const start = instantFromLocal(f.start);
  const end = instantFromLocal(f.end);
  const request = requestFromForm(f.inputs);
  const inputs = "json" in request ? request.json.inputs : {};
  return {
    start: (start ?? new Date(0)).toISOString(),
    ...(end ? { end: end.toISOString() } : {}),
    inputs,
    labels: labelsFromForm(f.labels),
  };
}

/** The sentences that keep the submit off, first the window's, then the rows', then the inputs'. */
export function submitReasons(f: BackfillForm, now: Date): string[] {
  const reasons: string[] = [];
  const start = instantFromLocal(f.start);
  const end = instantFromLocal(f.end);
  if (!start) reasons.push("Pick a start");
  if (end && end.getTime() > now.getTime()) reasons.push("The end is in the future; leave it empty to end now");
  if (start && (end ?? now).getTime() <= start.getTime()) reasons.push("The start must be before the end");
  if (f.labels.some((r) => labelRowError(r) !== null)) reasons.push("Fix the labels");
  if (Object.keys(validateForm(f.inputs)).length > 0) reasons.push("Fix the inputs");
  return reasons;
}

/** Where a server refusal belongs on the form (FE-FORM-4): a field, one input, or the form as a whole. */
export type ErrorPlace = { field: "start" | "end" | "labels" | "input" | "form"; input?: string; message: string };

/**
 * A 400 or 409 of the create route, placed by its code. The inputs' refusal
 * names its input in the message when it names one (`serverErrorInput`), else
 * it stays with the form.
 */
export function placeError(code: string | undefined, message: string, inputs: RunInput[]): ErrorPlace {
  switch (code) {
    case "backfill_start_too_early":
      return { field: "start", message };
    case "backfill_end_in_future":
    case "backfill_window_invalid":
      return { field: "end", message };
    case "backfill_labels_invalid":
      return { field: "labels", message };
    case "backfill_inputs_invalid": {
      const input = serverErrorInput(message, inputs) ?? inputs.find((i) => new RegExp(`\\b${escapeRe(i.id)}\\b`).test(message))?.id;
      return input ? { field: "input", input, message } : { field: "form", message };
    }
    default:
      return { field: "form", message };
  }
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * The schedule's own `inputs:` for this trigger, read from the flow's saved
 * YAML: a plain value prefills the form; a templated one (or none) is left to
 * the form, since a template cannot be rendered at the browser (§4.3).
 */
export function scheduleInputs(source: string, triggerId: string): Record<string, unknown> {
  const block = triggerBlock(source, triggerId);
  if (block === null) return {};
  try {
    const doc = parse(block) as { inputs?: unknown } | null;
    const raw = doc?.inputs;
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
    return Object.fromEntries(Object.entries(raw).filter(([, v]) => !(typeof v === "string" && v.includes("{{"))));
  } catch {
    return {};
  }
}

/** The inputs this form offers: every supported input but FILE, which a backfill cannot carry (§4.3). */
export function offeredInputs(inputs: RunInput[]): RunInput[] {
  return inputs.filter((i) => i.type !== "FILE");
}

/** A required FILE input with no default, which keeps the form off. */
export const requiredFile = (inputs: RunInput[]): RunInput | undefined =>
  inputs.find((i) => i.type === "FILE" && i.required && !i.hasDefault);

/** Whether a value counts as filled for the "unsaved" warning. */
export const hasEdits = (f: Pick<BackfillForm, "start" | "end" | "labels">) =>
  f.start !== "" || f.end !== "" || f.labels.some((r) => !isEmpty(r.key) || !isEmpty(r.value));
