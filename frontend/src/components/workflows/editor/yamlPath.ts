import { isMap, isScalar, isSeq, LineCounter, parseDocument, type Pair, type YAMLMap } from "yaml";

/** A range in the source, 1-based, the end column exclusive (Monaco's convention). */
export type YamlRange = { startLine: number; startCol: number; endLine: number; endCol: number };

type Parsed = { contents: unknown; at: (start: number, end: number) => YamlRange };

/** The document's tree, or null when it does not parse: a broken document maps nothing. */
function parse(source: string): Parsed | null {
  const lineCounter = new LineCounter();
  const doc = parseDocument(source, { lineCounter });
  if (doc.errors.length > 0) return null;
  return {
    contents: doc.contents,
    at: (start, end) => {
      const s = lineCounter.linePos(start);
      const e = lineCounter.linePos(end);
      return { startLine: s.line, startCol: s.col, endLine: e.line, endCol: e.col };
    },
  };
}

type Ranged = { range?: [number, number, number] | null };
const rangeOf = (n: unknown) => (n as Ranged | null)?.range ?? null;

/** The scalar value under `key` in `map`, or undefined. */
function scalarOf(map: YAMLMap, key: string): unknown {
  const v = map.get(key, true);
  return isScalar(v) ? v.value : undefined;
}

function pairOf(map: YAMLMap, key: string): Pair | undefined {
  return map.items.find((p) => isScalar(p.key) && String(p.key.value) === key);
}

/**
 * The item of a list that a server path names by id: a task, input or
 * trigger (`id: name`), or a `core.dag` node (`task: { id: name }`).
 */
function itemById(items: readonly unknown[], name: string): unknown {
  return items.find((i) => {
    if (!isMap(i)) return false;
    if (scalarOf(i, "id") === name) return true;
    const task = i.get("task", true);
    return isMap(task) && scalarOf(task, "id") === name;
  });
}

/**
 * A flowable's child named by id. egret names a child task by its id right
 * after its parent's (`tasks.parent.child`), whichever list holds it (`tasks`,
 * `then`, `else`, a `cases` entry, …), so every list of the parent is searched,
 * one mapping level deep for `cases`.
 */
function childTask(task: YAMLMap, name: string): unknown {
  for (const pair of task.items) {
    const lists = isSeq(pair.value)
      ? [pair.value]
      : isMap(pair.value)
        ? pair.value.items.map((p) => p.value).filter(isSeq)
        : [];
    for (const list of lists) {
      const hit = itemById(list.items, name);
      if (hit) return hit;
    }
  }
  return undefined;
}

/** A pair's range: its key through its value when the value is a scalar, else the key. */
function pairEnd(pair: Pair): number | null {
  const key = rangeOf(pair.key);
  if (!key) return null;
  const value = rangeOf(pair.value);
  return isScalar(pair.value) && value && value[1] > value[0] ? value[1] : key[1];
}

/**
 * Map a finding's `path` to its range in `source`.
 *
 * Two spellings are accepted, and may mix:
 * - by index, `tasks[2].retry.maxAttempt`;
 * - by id, as egret's validator writes them (`crates/egret-workflow/src/spec/validate.rs`):
 *   a list of tasks, inputs or triggers is read by id (`tasks.extract.retry`),
 *   and a flowable's child is named right after its parent (`tasks.parallel.child`).
 *
 * A key maps to the key and, when its value is a scalar, the value; a list
 * item maps to its first entry, so a whole task is not painted. Null when the
 * path names nothing in the document or the document does not parse.
 */
export function rangeOfPath(source: string, path: string): YamlRange | null {
  if (!path) return null;
  const parsed = parse(source);
  if (!parsed) return null;

  let node: unknown = parsed.contents;
  let pair: Pair | null = null;
  for (const segment of path.split(".")) {
    const m = /^([^[\]]*)((?:\[\d+\])*)$/.exec(segment);
    if (!m || (!m[1] && !m[2])) return null;
    const [, name, indexes] = m;
    if (name) {
      pair = null;
      if (isMap(node)) {
        const p = pairOf(node, name);
        if (p) {
          pair = p;
          node = p.value;
        } else {
          node = scalarOf(node, "type") !== undefined ? childTask(node, name) : undefined;
        }
      } else if (isSeq(node)) {
        node = itemById(node.items, name);
      } else {
        return null;
      }
      if (node === undefined) return null;
    }
    for (const [, i] of indexes.matchAll(/\[(\d+)\]/g)) {
      if (!isSeq(node)) return null;
      node = node.items[Number(i)];
      pair = null;
      if (node === undefined) return null;
    }
  }

  if (pair) {
    const start = rangeOf(pair.key)?.[0];
    const end = pairEnd(pair);
    return start === undefined || end === null ? null : parsed.at(start, end);
  }
  const r = rangeOf(node);
  if (!r) return null;
  if (isMap(node) && node.items.length > 0) {
    const end = pairEnd(node.items[0]);
    return end === null ? null : parsed.at(r[0], end);
  }
  return parsed.at(r[0], r[1]);
}

/**
 * The `type:` of the innermost task or trigger enclosing `offset`, or null at
 * flow level. A task or trigger is a mapping with an `id` and a string `type`,
 * as Kestra's editor decides (`isTaskLike` in `yamlLanguageConfigurator.ts`),
 * so a retry policy's own `type: constant` is not taken for a task.
 */
export function typeAt(source: string, offset: number): string | null {
  const parsed = parse(source);
  if (!parsed) return null;
  let found: string | null = null;
  const visit = (node: unknown) => {
    const r = rangeOf(node);
    if (!r || offset < r[0] || offset > r[1]) return;
    if (isMap(node)) {
      const type = scalarOf(node, "type");
      if (typeof type === "string" && scalarOf(node, "id") != null) found = type;
      for (const p of node.items) visit(p.value);
    } else if (isSeq(node)) {
      for (const item of node.items) visit(item);
    }
  };
  visit(parsed.contents);
  return found;
}
