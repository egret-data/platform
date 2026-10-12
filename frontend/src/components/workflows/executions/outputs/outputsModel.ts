/**
 * The outputs' pure model (W7b spec §8): where the files are in a value, and
 * how a path in a task run's outputs is written as an expression.
 */

/**
 * A well-formed `kestra:///` URI: the prefix, then a non-empty path with no
 * whitespace that does not end in `/`. A log-like string that merely starts
 * with a URI ("kestra:///x.csv written") or the bare prefix is not a file, so
 * it is not offered as a file row that would read "no longer available".
 */
const FILE_URI = /^kestra:\/\/\/\S*[^\s/]$/;

export const isFileUri = (v: unknown): v is string => typeof v === "string" && FILE_URI.test(v);

/** A path into a value: object keys and array indexes, from the top. */
export type OutputPath = readonly (string | number)[];

export interface FoundFile {
  path: OutputPath;
  uri: string;
}

/**
 * Every `kestra:///` URI in `value`, at any depth of objects and arrays, with
 * its path, in document order (object keys as given, array items by index).
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

/** A path's steps alone, as Pebble writes them after a name: `.key[0]["odd key"]`. */
export const pathSteps = (path: OutputPath) => path.map(step).join("");

/**
 * The expression that reads `path` in a task run's outputs, braces included:
 * `{{ outputs.t1.key }}`.
 *
 * Always flat by task id: egret's renderer puts each run's outputs into the
 * context under `outputs.<taskId>` (`worker/render.rs`), never keyed by a
 * run's `value`, so Kestra's `outputs.<taskId>[value]` form would not resolve
 * here (W7b Task 6 review; ruled 2026-10-12).
 */
export function outputExpression(taskId: string, path: OutputPath): string {
  return `{{ ${outputPath(taskId, path)} }}`;
}

/** `outputExpression` without the braces: `outputs.t1.key`. */
export function outputPath(taskId: string, path: OutputPath): string {
  return `outputs${step(taskId)}${pathSteps(path)}`;
}

/**
 * The end of the note shown on a loop iteration's outputs: a `Loop`'s
 * iterations are child executions, whose context is the loop ancestors'
 * outputs plus their own, so the expressions resolve inside the iteration and
 * nowhere else. Also this module's marker in `scripts/assert-workflows-lazy.mjs`.
 */
export const LOOP_ITERATION_NOTE = "The parent execution cannot reach these outputs.";

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
