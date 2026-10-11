/**
 * The outputs' pure model (W7b spec §8): where the files are in a value, and
 * how a path in a task run's outputs is written as an expression.
 */

/** A `kestra:///` URI names a file in the execution's internal storage (spec §8). */
export const FILE_URI_PREFIX = "kestra:///";

export const isFileUri = (v: unknown): v is string => typeof v === "string" && v.startsWith(FILE_URI_PREFIX);

/** A path into a value: object keys and array indexes, from the top. */
export type OutputPath = readonly (string | number)[];

export interface FoundFile {
  path: OutputPath;
  uri: string;
}

/**
 * Every `kestra:///` string in `value`, at any depth of objects and arrays,
 * with its path, in document order (object keys as given, array items by
 * index). A string that only contains the prefix past its start is not a file.
 */
export function findFileUris(value: unknown, path: OutputPath = []): FoundFile[] {
  if (isFileUri(value)) return [{ path, uri: value }];
  if (Array.isArray(value)) return value.flatMap((v, i) => findFileUris(v, [...path, i]));
  if (value !== null && typeof value === "object") {
    return Object.entries(value).flatMap(([k, v]) => findFileUris(v, [...path, k]));
  }
  return [];
}

/** A key Pebble reads after a dot: Kestra's `isValidVariable` (`ExecutionVariableExplorer.vue`). */
const DOTTED = /^[a-zA-Z][a-zA-Z0-9_]*$/;

/** One step of a path as Pebble writes it: `.key`, `["odd-key"]`, or `[0]`. Kestra's `formatStep`. */
function step(key: string | number): string {
  if (typeof key === "number") return `[${key}]`;
  return DOTTED.test(key) ? `.${key}` : `[${JSON.stringify(key)}]`;
}

/**
 * The expression that reads `path` in a task run's outputs, braces included:
 * `{{ outputs.t1.key }}`. A run that carries a `value` (a run inside a
 * looping flowable) is reached through it, in Kestra's `outputs.<taskId>[value]`
 * form: `{{ outputs.t1["a"].key }}`.
 *
 * No egret run carries a `value` today (no `CreateRun` sets one; a `Loop`'s
 * iterations are child executions, W7b Task 4), so the loop form is written
 * only when the DTO has one.
 */
export function outputExpression(taskId: string, value: string | null | undefined, path: OutputPath): string {
  return `{{ ${outputPath(taskId, value, path)} }}`;
}

/** `outputExpression` without the braces: `outputs.t1.key`. */
export function outputPath(taskId: string, value: string | null | undefined, path: OutputPath): string {
  const loop = value === null || value === undefined ? "" : `[${JSON.stringify(value)}]`;
  return `outputs${step(taskId)}${loop}${pathSteps(path)}`;
}

/** A path's steps alone, as Pebble writes them after a name: `.key[0]["odd key"]`. */
export const pathSteps = (path: OutputPath) => path.map(step).join("");

/**
 * The note shown above a loop run's outputs (spec §8: the loop form "is noted
 * as such"). Also this module's marker in `scripts/assert-workflows-lazy.mjs`.
 */
export const LOOP_EXPRESSION_NOTE = "A run inside a loop is reached through its value";

/** A value as the tree previews it on one line. */
export function previewValue(value: unknown): string {
  if (value === null) return "null";
  if (typeof value === "string") return JSON.stringify(value);
  if (Array.isArray(value)) return value.length === 1 ? "1 item" : `${value.length} items`;
  if (typeof value === "object") {
    const n = Object.keys(value).length;
    return n === 1 ? "1 key" : `${n} keys`;
  }
  return String(value);
}

/** A value as "Copy value" writes it: a string as itself, anything else as indented JSON. */
export function copyText(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}
