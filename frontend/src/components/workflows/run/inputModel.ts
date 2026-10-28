import { parse, stringify } from "yaml";
import type { ValidateInputsDto } from "@/api/generated/model";

/**
 * THE RUN FORM'S MODEL (W7a spec §6.1): a flow's `inputs:` turned into form
 * state, checked, and turned into the body of `POST …/flows/{id}/executions`.
 *
 * **Read from the YAML, not from the stored plan.** egret's parsed `Input`
 * (`crates/egret-workflow/src/models/input.rs`) is not served with the flow,
 * so the source is where the form finds `values`, bounds and `validator`.
 *
 * **The server decides** (workflow inputs #3, D1 and D14). Every start checks
 * the inputs (`crates/egret-workflow/src/inputs_check.rs`): `required`, the
 * type, `validator`, `min`/`max`, `after`/`before`, SELECT/MULTISELECT
 * `values`, literal `defaults` and `dependsOn`, and refuses the first bad one
 * with a 400 that names it in backticks. `POST …/executions/validate` runs the
 * same check without starting anything and lists every input's errors; the
 * form calls it on Run, before the start. The checks here are a quick first
 * pass that answers without a round trip; the server's answer is the one that
 * counts. A `validator` is only shown, never run here: its dialect is Java's,
 * which the browser's `RegExp` is not.
 *
 * **What the body carries.** The JSON body's `inputs` is an object; a
 * **string** posted for INT, FLOAT, BOOL, JSON, ARRAY or MULTISELECT is parsed
 * as JSON. An input left out gets the flow's `defaults:`. A FILE value must be
 * uploaded: the start becomes `multipart/form-data`, one part per input id,
 * where a part with a file name is the file and every other part is a text
 * value. The validate endpoint uploads nothing and takes the file's name.
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
  /** INT, FLOAT and DURATION bounds as written (inclusive). */
  min?: string;
  max?: string;
  /** DATETIME, DATE and TIME bounds as written (inclusive). */
  after?: string;
  before?: string;
  /** A Java regex the server matches the whole value against; shown, never run here. */
  validator?: string;
  /** SELECT/MULTISELECT: a value outside `values` is accepted. */
  allowCustomValue: boolean;
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
/** A bound as text: YAML reads `min: 10` as a number and `after: 2026-01-01` as a string. */
const optBound = (v: unknown) =>
  typeof v === "number" ? String(v) : v instanceof Date ? v.toISOString() : optString(v);

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
      min: optBound(item.min),
      max: optBound(item.max),
      after: optBound(item.after),
      before: optBound(item.before),
      validator: optString(item.validator),
      allowCustomValue: item.allowCustomValue === true,
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
/** An address with one `@` and a dotted domain; what a person means by an email, not RFC 5322 in full. */
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
/**
 * Java's `Duration.parse` grammar, which Kestra's DURATION input reads with:
 * days, hours, minutes and seconds, each optionally signed, case-insensitive.
 * No years, months or weeks: `Duration` refuses them. The lookaheads require a
 * component after `P`, and one after `T`.
 */
const DURATION = /^[-+]?P(?=\d|[-+]|T)(?:[-+]?\d+D)?(?:T(?=[-+]?\d)(?:[-+]?\d+H)?(?:[-+]?\d+M)?(?:[-+]?\d+(?:[.,]\d{0,9})?S)?)?$/i;

/** A URI with a scheme, as Kestra's URI input requires (`URI.create` of an absolute URI). */
function isUri(text: string): boolean {
  try {
    new URL(text);
    return true;
  } catch {
    return false;
  }
}
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

const setIf = (errors: Record<string, string>, id: string, message: string | null) => {
  if (message) errors[id] = message;
};

/** A parsed value or bound; `NaN` or `null` is one the form cannot read. */
const known = (n: number | null): n is number => n !== null && !Number.isNaN(n);

/** `min`/`max`, inclusive as the server's (`check_int_bounds` and its siblings). */
function rangeError(
  input: RunInput,
  value: number | null,
  read: (bound: string) => number | null,
  what: (bound: string) => string,
): string | null {
  if (!known(value)) return null;
  const min = input.min === undefined ? null : read(input.min);
  const max = input.max === undefined ? null : read(input.max);
  if (known(min) && value < min) return `Enter ${what(`at least ${input.min}`)}.`;
  if (known(max) && value > max) return `Enter ${what(`at most ${input.max}`)}.`;
  return null;
}

/** `after`/`before`, inclusive as the server's (`check_after_before`). */
function orderError(input: RunInput, value: number | null, read: (bound: string) => number | null, what: string): string | null {
  if (!known(value)) return null;
  const after = input.after === undefined ? null : read(input.after);
  const before = input.before === undefined ? null : read(input.before);
  if (known(after) && value < after) return `Enter ${what} on or after ${input.after}.`;
  if (known(before) && value > before) return `Enter ${what} on or before ${input.before}.`;
  return null;
}

/** A `YYYY-MM-DD` date as a number that orders as the date does. */
function dateKey(text: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text.trim());
  return m ? Number(m[1]) * 10000 + Number(m[2]) * 100 + Number(m[3]) : null;
}

/** `HH:MM`, `HH:MM:SS` or `HH:MM:SS.fff` as seconds since midnight. */
function timeSeconds(text: string): number | null {
  const m = /^(\d{2}):(\d{2})(?::(\d{2}(?:\.\d{1,9})?))?$/.exec(text.trim());
  return m ? Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3] ?? 0) : null;
}

/** The parts of an ISO-8601 duration that {@link DURATION} accepts. */
const DURATION_PARTS = /^([-+]?)P(?:([-+]?\d+)D)?(?:T(?:([-+]?\d+)H)?(?:([-+]?\d+)M)?(?:([-+]?\d+)(?:[.,](\d{0,9}))?S)?)?$/i;

/** A duration as seconds, as Java's `Duration.parse` reads it: a fraction takes its seconds' sign. */
function durationSeconds(text: string): number | null {
  if (!DURATION.test(text)) return null;
  const m = DURATION_PARTS.exec(text);
  if (!m) return null;
  const [, sign, d, h, min, sec, frac] = m;
  const seconds = sec === undefined ? 0 : Number(frac ? `${sec}.${frac}` : sec);
  const total = Number(d ?? 0) * 86400 + Number(h ?? 0) * 3600 + Number(min ?? 0) * 60 + seconds;
  return sign === "-" ? -total : total;
}

const REQUIRED_MESSAGE: Record<string, string> = {
  SELECT: "Choose a value.",
  MULTISELECT: "Choose at least one value.",
  ARRAY: "Add at least one item.",
  FILE: "Choose a file.",
};

/**
 * The form's own checks, by input id: the first pass before the validate
 * endpoint (see the module comment), so a value the form can already tell is
 * wrong is answered at once, in the form's words. A bound that does not parse
 * here is left to the server, which refuses such a flow at save.
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
        else setIf(errors, input.id, rangeError(input, Number(text), Number, (b) => `a number of ${b}`));
        break;
      case "FLOAT":
        if (!isNumber(text)) errors[input.id] = "Enter a number.";
        else setIf(errors, input.id, rangeError(input, Number(text), Number, (b) => `a number of ${b}`));
        break;
      case "DATETIME": {
        const at = new Date(text).getTime();
        if (Number.isNaN(at)) errors[input.id] = "Enter a date and time.";
        else setIf(errors, input.id, orderError(input, at, (b) => new Date(b).getTime(), "a date and time"));
        break;
      }
      case "DATE":
        setIf(errors, input.id, orderError(input, dateKey(text), dateKey, "a date"));
        break;
      case "TIME":
        setIf(errors, input.id, orderError(input, timeSeconds(text), timeSeconds, "a time"));
        break;
      case "SELECT":
        // Kestra checks a SELECT's `values` only when it is required (D4); its picker offers
        // only them, but a `defaults` outside them fills it in.
        if (input.required && !input.allowCustomValue && input.values.length > 0 && !input.values.includes(text)) {
          errors[input.id] = `Choose one of the listed values; ${text} is not one of them.`;
        }
        break;
      case "MULTISELECT": {
        const stray = input.allowCustomValue || input.values.length === 0 ? [] : (value as string[]).filter((v) => !input.values.includes(v));
        if (stray.length > 0) errors[input.id] = `Untick ${stray.join(", ")}: only the listed values are accepted.`;
        break;
      }
      case "EMAIL":
        if (!EMAIL.test(text.trim())) errors[input.id] = "Enter an email address, such as name@example.com.";
        break;
      case "URI":
        if (!isUri(text.trim())) errors[input.id] = "Enter a URI with its scheme, such as https://example.com.";
        break;
      case "DURATION":
        if (!DURATION.test(text.trim())) errors[input.id] = "Enter an ISO-8601 duration, such as PT1H or P1DT12H.";
        else setIf(errors, input.id, rangeError(input, durationSeconds(text.trim()), durationSeconds, (b) => `a duration of ${b}`));
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
      // The text as typed, already checked to parse: the backend parses a JSON input's string
      // exactly once (`with_declared_defaults`), so sending it parsed would parse a JSON string
      // twice and turn `"123"` into the number 123.
      return value;
    case "MULTISELECT":
      return value;
    case "ARRAY":
      return (value as string[]).filter((r) => r.trim() !== "").map((r) => arrayItem(input.itemType, r));
    default:
      return value;
  }
}

/** Each non-empty value as the request carries it; a chosen file stays a `File`. */
function requestEntries(form: FormState): [string, unknown][] {
  const entries: [string, unknown][] = [];
  for (const input of form.inputs) {
    if (!SUPPORTED.has(input.type)) continue;
    const value = form.values[input.id] ?? null;
    if (isEmpty(value)) continue;
    if (value instanceof File) entries.push([input.id, value]);
    else if (input.type !== "FILE") entries.push([input.id, wireValue(input, value as Exclude<FieldValue, null | File>)]);
  }
  return entries;
}

/**
 * The body of `POST …/flows/{id}/executions`. JSON, unless a file was chosen:
 * then multipart, the file under its input id, every other value a text part
 * (the backend parses a structured type's text as JSON and keeps any other as
 * it is). Empty values are left out, so the flow's defaults apply.
 */
export function requestFromForm(form: FormState): RunRequest {
  const entries = requestEntries(form);
  if (!entries.some(([, v]) => v instanceof File)) return { json: { inputs: Object.fromEntries(entries) } };
  const body = new FormData();
  for (const [id, v] of entries) {
    if (v instanceof File) body.append(id, v, v.name);
    else if (typeof v === "string") body.append(id, v);
    else body.append(id, JSON.stringify(v));
  }
  return { multipart: body };
}

/**
 * The body of `POST …/flows/{id}/executions/validate`: the start's JSON body,
 * with a chosen file as its name. The endpoint uploads nothing and takes a
 * FILE input's name or URI (B1), so a file's contents and extension are left
 * to the start, which stays the final word.
 */
export function validationRequestFromForm(form: FormState): { inputs: Record<string, unknown> } {
  return { inputs: Object.fromEntries(requestEntries(form).map(([id, v]) => [id, v instanceof File ? v.name : v])) };
}

/** What the form shows from a validate answer. */
export interface ValidationView {
  /** The errors of each input the form shows and the start would check, by input id. */
  errors: Record<string, string>;
  /** Inputs `dependsOn` turns off with these values: the start ignores them. */
  inactive: string[];
}

/**
 * The validate answer (`ValidateInputsDto`) for this form. An input turned off
 * by `dependsOn` is listed with its errors, but a start would not report them
 * (B1), so they are not shown; nor are errors of an input the form has no
 * control for. The input is matched by the `id` of its YAML entry.
 */
export function validationView(answer: ValidateInputsDto, inputs: RunInput[]): ValidationView {
  const shown = new Set(inputs.filter((i) => SUPPORTED.has(i.type)).map((i) => i.id));
  const view: ValidationView = { errors: {}, inactive: [] };
  // A body of another shape (no `inputs` list) tells nothing; the start then decides.
  for (const outcome of Array.isArray(answer.inputs) ? answer.inputs : []) {
    const id = outcome.input.id;
    if (typeof id !== "string" || !shown.has(id)) continue;
    if (!outcome.enabled) {
      view.inactive.push(id);
      continue;
    }
    const message = outcome.errors.map((e) => e.message).join("\n");
    if (message) view.errors[id] = message;
  }
  return view;
}

/** The declared input one line of a server refusal names, in backticks or in Kestra's `Missing required input:<id>`. */
function lineInput(line: string, declared: Set<string>): string | undefined {
  for (const m of line.matchAll(/input `([^`]+)`/g)) if (declared.has(m[1])) return m[1];
  const missing = /^Missing required input:(\S+)/.exec(line.trim());
  return missing && declared.has(missing[1]) ? missing[1] : undefined;
}

/**
 * A server refusal split by the declared inputs it names, line by line. The
 * backend names an input in backticks ("Invalid value for input `data`: …",
 * `inputs_check.rs`, and `read_text_part` in `routes.rs`) or as
 * `Missing required input:<id>`; a refusal of several values has one line per
 * value. A line naming no input continues the line before it; one before any
 * named line is `rest`, a form-level error.
 */
export function serverErrorsByInput(message: string, inputs: RunInput[]): { fields: Record<string, string>; rest: string[] } {
  const declared = new Set(inputs.map((i) => i.id));
  const fields: Record<string, string> = {};
  const rest: string[] = [];
  let last: string | undefined;
  for (const line of message.split("\n")) {
    if (line.trim() === "") continue;
    const id = lineInput(line, declared) ?? last;
    if (id === undefined) {
      rest.push(line);
      continue;
    }
    fields[id] = fields[id] ? `${fields[id]}\n${line}` : line;
    last = id;
  }
  return { fields, rest };
}

/** The first declared input a server refusal names, for a form that shows one error (the backfill dialog). */
export function serverErrorInput(message: string, inputs: RunInput[]): string | undefined {
  return Object.keys(serverErrorsByInput(message, inputs).fields)[0];
}
