import { parse, stringify } from "yaml";

/**
 * THE RUN FORM'S MODEL (W7a spec §6.1): a flow's `inputs:` turned into form
 * state, checked, and turned into the body of `POST …/flows/{id}/executions`.
 *
 * **Read from the YAML, not from the stored plan.** egret's parsed `Input`
 * (`crates/egret-workflow/src/models/input.rs`) has no `values` field, so the
 * plan has no SELECT or MULTISELECT values to offer; the source still does.
 *
 * **What the backend does with each value** (`start_flow_execution` and
 * `with_declared_defaults` in `crates/egret-workflow/src/worker/render.rs`):
 * the JSON body's `inputs` is an object, kept as sent. A **string** posted for
 * INT, FLOAT, BOOL, JSON, ARRAY or MULTISELECT is parsed as JSON; every other
 * type keeps its string. An input left out gets the flow's `defaults:`. No
 * type, `required` or SELECT membership is checked server side, so this form is
 * the only place those are checked. A FILE value must be uploaded: the request
 * becomes `multipart/form-data`, one part per input id, where a part with a
 * file name is the file and every other part is a text value.
 */

/** Every type Kestra's `Input.java` registers (`InputType` in `models/input.rs`). */
const SUPPORTED = new Set([
  "STRING",
  "INT",
  "FLOAT",
  "BOOL",
  "DATETIME",
  "DATE",
  "TIME",
  "DURATION",
  "FILE",
  "JSON",
  "YAML",
  "URI",
  "SECRET",
  "SELECT",
  "MULTISELECT",
  "ARRAY",
  "EMAIL",
]);

/** One value, one control: the types §6.1 lets a dialog hold. */
const SCALAR = new Set([
  "STRING",
  "EMAIL",
  "URI",
  "SECRET",
  "DURATION",
  "INT",
  "FLOAT",
  "BOOL",
  "DATETIME",
  "DATE",
  "TIME",
  "SELECT",
]);

export interface RunInput {
  id: string;
  /** As written in the YAML; unknown spellings are kept and reported unsupported. */
  type: string;
  displayName?: string;
  description?: string;
  /** Kestra's default is `true`. */
  required: boolean;
  /** `prefill` when the flow has one, else `defaults`: what the control starts with. */
  initial?: unknown;
  /** The flow declares `defaults:`, which the backend applies when the input is left out. */
  hasDefault: boolean;
  /** A `defaults:` that is an expression; it is not filled in, and the control names it. */
  expressionDefault?: string;
  /** SELECT and MULTISELECT values, as text. */
  values: string[];
  /** The SELECT's `values:` is an expression, so there is no list to pick from. */
  valuesExpression?: string;
  itemType?: string;
  /** Kestra's `dependsOn`: not evaluated here (spec §6.2); the control carries a note. */
  dependsOn: boolean;
  allowedFileExtensions?: string[];
}

/** A control's value: text, a switch, a list (MULTISELECT, ARRAY rows), or a chosen file. */
export type FieldValue = string | boolean | string[] | File | null;

export interface FormState {
  inputs: RunInput[];
  values: Record<string, FieldValue>;
}

export type RunRequest = { json: { inputs: Record<string, unknown> } } | { multipart: FormData };

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isExpression = (v: unknown): v is string => typeof v === "string" && v.includes("{{");
const optString = (v: unknown) => (typeof v === "string" && v !== "" ? v : undefined);

/** The `inputs:` of a flow's YAML. A source that does not parse is an error, never "no inputs". */
export function inputsFromSource(source: string): { inputs: RunInput[] } | { error: string } {
  let doc: unknown;
  try {
    doc = parse(source);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
  if (!isRecord(doc)) return { error: "The flow's source is not a YAML mapping." };
  const raw = doc.inputs;
  if (raw == null) return { inputs: [] };
  if (!Array.isArray(raw)) return { error: "The flow's `inputs` is not a list." };
  const inputs: RunInput[] = [];
  for (const item of raw) {
    if (!isRecord(item) || typeof item.id !== "string" || typeof item.type !== "string") continue;
    const type = item.type;
    const listed = item.values ?? (type === "MULTISELECT" ? item.options : undefined);
    const defaults = item.defaults;
    const initial = item.prefill ?? defaults;
    inputs.push({
      id: item.id,
      type,
      displayName: optString(item.displayName),
      description: optString(item.description),
      required: item.required !== false,
      initial: isExpression(initial) ? undefined : initial,
      hasDefault: defaults != null,
      expressionDefault: isExpression(defaults) ? defaults : undefined,
      values: Array.isArray(listed) ? listed.map(String) : [],
      valuesExpression: typeof listed === "string" ? listed : undefined,
      itemType: optString(item.itemType),
      dependsOn: item.dependsOn != null,
      allowedFileExtensions: Array.isArray(item.allowedFileExtensions)
        ? item.allowedFileExtensions.map(String)
        : undefined,
    });
  }
  return { inputs };
}

/** The inputs this form cannot enter (spec §6.1): FORM, REUSABLE_INPUTS, ION, and any type it does not know. */
export function unsupportedInputs(inputs: RunInput[]): { id: string; type: string }[] {
  return inputs.filter((i) => !SUPPORTED.has(i.type)).map(({ id, type }) => ({ id, type }));
}

/** Spec §6.1: a confirm for no inputs, a dialog for at most three scalar ones, a drawer otherwise. */
export function containerFor(inputs: RunInput[]): "confirm" | "dialog" | "drawer" {
  if (inputs.length === 0) return "confirm";
  if (inputs.length <= 3 && inputs.every((i) => SCALAR.has(i.type))) return "dialog";
  return "drawer";
}

const pad = (n: number) => String(n).padStart(2, "0");

/** An instant as a `datetime-local` control holds it: local wall time, to the second. */
function toLocalDateTime(v: unknown): string {
  if (typeof v !== "string" && !(v instanceof Date)) return "";
  const d = new Date(v);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** `yaml` reads an unquoted `2026-03-04` as a string by default; a Date is handled too. */
function toDate(v: unknown): string {
  if (v instanceof Date) return v.toISOString().slice(0, 10);
  return typeof v === "string" && /^\d{4}-\d{2}-\d{2}/.test(v) ? v.slice(0, 10) : "";
}

const asText = (v: unknown): string => (v == null ? "" : typeof v === "string" ? v : JSON.stringify(v));

/** A list default, or a JSON text of one. */
function asList(v: unknown): string[] {
  let list = v;
  if (typeof v === "string") {
    try {
      list = JSON.parse(v);
    } catch {
      return [v];
    }
  }
  return Array.isArray(list) ? list.map(asText) : [];
}

/** What a control starts with: the flow's `prefill` or `defaults`, in the control's own shape. */
function initialValue(input: RunInput): FieldValue {
  const d = input.initial;
  switch (input.type) {
    case "BOOL":
      return d === true || d === "true";
    case "DATETIME":
      return toLocalDateTime(d);
    case "DATE":
      return toDate(d);
    case "TIME":
      return typeof d === "string" && /^\d{2}:\d{2}(:\d{2})?$/.test(d) ? d : "";
    case "MULTISELECT":
    case "ARRAY":
      return asList(d);
    case "JSON":
      return d == null ? "" : typeof d === "string" ? d : JSON.stringify(d, null, 2);
    case "YAML":
      return d == null ? "" : typeof d === "string" ? d : stringify(d);
    case "FILE":
      // A FILE default is a `kestra://` URI the backend applies when the input is left out.
      return null;
    default:
      return SUPPORTED.has(input.type) ? asText(d) : null;
  }
}

export function formFromInputs(inputs: RunInput[]): FormState {
  return { inputs, values: Object.fromEntries(inputs.map((i) => [i.id, initialValue(i)])) };
}

/** No value to send: the input is left out of the request. A switch always has one. */
export function isEmpty(value: FieldValue): boolean {
  if (value === null || value === "") return true;
  if (Array.isArray(value)) return value.every((v) => v.trim() === "");
  return false;
}

const INTEGER = /^[+-]?\d+$/;
const isNumber = (s: string) => s.trim() !== "" && Number.isFinite(Number(s));

function itemError(itemType: string | undefined, item: string): string | null {
  if (itemType === "INT" && !INTEGER.test(item.trim())) return "enter a whole number.";
  if (itemType === "FLOAT" && !isNumber(item)) return "enter a number.";
  if (itemType === "BOOL" && item !== "true" && item !== "false") return "enter true or false.";
  if (itemType === "JSON") {
    try {
      JSON.parse(item);
    } catch {
      return "this is not valid JSON.";
    }
  }
  return null;
}

const REQUIRED_MESSAGE: Record<string, string> = {
  SELECT: "Choose a value.",
  MULTISELECT: "Choose at least one value.",
  ARRAY: "Add at least one item.",
  FILE: "Choose a file.",
};

/**
 * The form's own checks, by input id. The backend checks none of these (see
 * the module comment), so a wrong value would otherwise start an execution
 * that fails later, or not at all.
 */
export function validateForm(form: FormState): Record<string, string> {
  const errors: Record<string, string> = {};
  for (const input of form.inputs) {
    if (!SUPPORTED.has(input.type)) continue;
    const value = form.values[input.id] ?? null;
    if (isEmpty(value)) {
      // Left out, the flow's own default applies (`with_declared_defaults`).
      if (input.required && !input.hasDefault) errors[input.id] = REQUIRED_MESSAGE[input.type] ?? "Enter a value for this input.";
      continue;
    }
    const text = typeof value === "string" ? value : "";
    switch (input.type) {
      case "INT":
        if (!INTEGER.test(text.trim())) errors[input.id] = "Enter a whole number.";
        break;
      case "FLOAT":
        if (!isNumber(text)) errors[input.id] = "Enter a number.";
        break;
      case "DATETIME":
        if (Number.isNaN(new Date(text).getTime())) errors[input.id] = "Enter a date and time.";
        break;
      case "JSON":
        try {
          JSON.parse(text);
        } catch (e) {
          errors[input.id] = `This is not valid JSON: ${e instanceof Error ? e.message : String(e)}`;
        }
        break;
      case "ARRAY": {
        const rows = (value as string[]).map((r, i) => [r, i] as const).filter(([r]) => r.trim() !== "");
        for (const [row, i] of rows) {
          const why = itemError(input.itemType, row);
          if (why) {
            errors[input.id] = `Item ${i + 1}: ${why}`;
            break;
          }
        }
        break;
      }
    }
  }
  return errors;
}

function arrayItem(itemType: string | undefined, item: string): unknown {
  if (itemType === "INT" || itemType === "FLOAT") return Number(item);
  if (itemType === "BOOL") return item === "true";
  if (itemType === "JSON") return JSON.parse(item);
  return item;
}

/** The value as the JSON body carries it. Called on a checked, non-empty value only. */
function wireValue(input: RunInput, value: Exclude<FieldValue, null | File>): unknown {
  switch (input.type) {
    case "INT":
    case "FLOAT":
      return Number(value);
    case "BOOL":
      return value === true;
    case "DATETIME":
      return new Date(value as string).toISOString();
    case "TIME":
      return (value as string).length === 5 ? `${value}:00` : value;
    case "JSON":
      return JSON.parse(value as string);
    case "MULTISELECT":
      return value;
    case "ARRAY":
      return (value as string[]).filter((r) => r.trim() !== "").map((r) => arrayItem(input.itemType, r));
    default:
      return value;
  }
}

/**
 * The body of `POST …/flows/{id}/executions`. JSON, unless a file was chosen:
 * then multipart, the file under its input id, every other value a text part
 * (the backend parses a structured type's text as JSON and keeps any other as
 * it is). Empty values are left out, so the flow's defaults apply.
 */
export function requestFromForm(form: FormState): RunRequest {
  const entries: [string, unknown][] = [];
  let file = false;
  for (const input of form.inputs) {
    if (!SUPPORTED.has(input.type)) continue;
    const value = form.values[input.id] ?? null;
    if (isEmpty(value)) continue;
    if (value instanceof File) {
      file = true;
      entries.push([input.id, value]);
    } else if (input.type !== "FILE") {
      entries.push([input.id, wireValue(input, value as Exclude<FieldValue, null | File>)]);
    }
  }
  if (!file) return { json: { inputs: Object.fromEntries(entries) } };
  const body = new FormData();
  for (const [id, v] of entries) {
    if (v instanceof File) body.append(id, v, v.name);
    else if (typeof v === "string") body.append(id, v);
    else body.append(id, JSON.stringify(v));
  }
  return { multipart: body };
}

/**
 * The declared input a server refusal names. The backend names one in
 * backticks ("Invalid value for input `data`: …", `storage/inputs.rs` and
 * `read_text_part` in `routes.rs`); a message naming no declared input stays
 * a form-level error.
 */
export function serverErrorInput(message: string, inputs: RunInput[]): string | undefined {
  for (const m of message.matchAll(/input `([^`]+)`/g)) {
    if (inputs.some((i) => i.id === m[1])) return m[1];
  }
  return undefined;
}
