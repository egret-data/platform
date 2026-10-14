import { isMap, isScalar, isSeq, LineCounter, parseDocument, type Pair, type YAMLMap } from "yaml";

/** A range in the source, 1-based, the end column exclusive (Monaco's convention). */
export type YamlRange = { startLine: number; startCol: number; endLine: number; endCol: number };

/**
 * One parse of one source. `yaml`'s tree is error-tolerant: a document with a
 * half-typed key (`    m`) or an open quote still has a tree with ranges, and
 * `ok` says whether it had errors. Parsing is the expensive part (≈10 ms for a
 * 500-line flow), so it happens once per source (`useFlowEditor`) and every
 * lookup reads this.
 */
export type YamlDoc = { source: string; contents: unknown; ok: boolean; lineCounter: LineCounter };

export function parseYaml(source: string): YamlDoc {
  const lineCounter = new LineCounter();
  const doc = parseDocument(source, { lineCounter });
  return { source, contents: doc.contents, ok: doc.errors.length === 0, lineCounter };
}

/**
 * The top-level scalar `key` of a parsed flow, as text (a flow's `namespace`), or null when the
 * document is not a map or has no such scalar.
 */
export function topLevelScalar(doc: YamlDoc, key: string): string | null {
  if (!isMap(doc.contents)) return null;
  const v = scalarOf(doc.contents, key);
  return v === undefined || v === null ? null : String(v);
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
 * The items of a list that a server path names by id: a task, input or
 * trigger (`id: name`), or a `core.dag` node (`task: { id: name }`). All of
 * them, in order: ids may repeat (the validator reports that), and the rest of
 * the path decides which one is meant.
 */
function itemsById(items: readonly unknown[], name: string): unknown[] {
  return items.filter((i) => {
    if (!isMap(i)) return false;
    if (scalarOf(i, "id") === name) return true;
    const task = i.get("task", true);
    return isMap(task) && scalarOf(task, "id") === name;
  });
}

/**
 * A flowable's children named by id. egret names a child task by its id right
 * after its parent's (`tasks.parent.child`), whichever list holds it (`tasks`,
 * `then`, `else`, a `cases` entry, …), so every list of the parent is searched,
 * one mapping level deep for `cases`.
 */
function childTasks(task: YAMLMap, name: string): unknown[] {
  return task.items.flatMap((pair) => {
    const lists = isSeq(pair.value)
      ? [pair.value]
      : isMap(pair.value)
        ? pair.value.items.map((p) => p.value).filter(isSeq)
        : [];
    return lists.flatMap((list) => itemsById(list.items, name));
  });
}

type Segment = { name: string; indexes: number[] };
type Found = { node: unknown; pair: Pair | null };

function segmentsOf(path: string): Segment[] | null {
  const out: Segment[] = [];
  for (const raw of path.split(".")) {
    const m = /^([^[\]]*)((?:\[\d+\])*)$/.exec(raw);
    if (!m || (!m[1] && !m[2])) return null;
    out.push({ name: m[1], indexes: [...m[2].matchAll(/\[(\d+)\]/g)].map((x) => Number(x[1])) });
  }
  return out;
}

/**
 * Walk `segments[i…]` from `node`, trying every reading of an ambiguous step
 * and keeping the first that resolves the whole path:
 * - in a mapping, the key first, then (in a task) a child task with that id,
 *   so a child called `condition` under an `If` is still found;
 * - in a list, the item with that id; an id may contain dots (Kestra allows
 *   them in input ids), so the longest dotted run is tried first;
 * - ids that repeat are tried in order.
 */
function walk(node: unknown, segs: Segment[], i: number, pair: Pair | null): Found | null {
  if (i === segs.length) return { node, pair };
  const index = (from: unknown, p: Pair | null, indexes: number[], next: number): Found | null => {
    let cur = from;
    let pp = p;
    for (const k of indexes) {
      if (!isSeq(cur)) return null;
      cur = cur.items[k];
      pp = null;
      if (cur === undefined) return null;
    }
    return walk(cur, segs, next, pp);
  };
  const { name, indexes } = segs[i];
  if (!name) return index(node, pair, indexes, i + 1);
  if (isMap(node)) {
    const p = pairOf(node, name);
    const viaKey = p ? index(p.value, p, indexes, i + 1) : null;
    if (viaKey) return viaKey;
    if (scalarOf(node, "type") === undefined) return null;
    for (const child of childTasks(node, name)) {
      const found = index(child, null, indexes, i + 1);
      if (found) return found;
    }
    return null;
  }
  if (isSeq(node)) {
    for (let j = segs.length; j > i; j--) {
      // Only the last segment of a dotted id may carry indexes.
      if (segs.slice(i, j - 1).some((s) => s.indexes.length > 0 || !s.name)) continue;
      const id = segs.slice(i, j).map((s) => s.name).join(".");
      for (const item of itemsById(node.items, id)) {
        const found = index(item, null, segs[j - 1].indexes, j);
        if (found) return found;
      }
    }
  }
  return null;
}

/** A pair's end: its value's when the value is a scalar, else its key's. */
function pairEnd(pair: Pair): number | null {
  const key = rangeOf(pair.key);
  if (!key) return null;
  const value = rangeOf(pair.value);
  return isScalar(pair.value) && value && value[1] > value[0] ? value[1] : key[1];
}

/** The offsets a path names in one document, or null. */
function offsetsOfPath(doc: YamlDoc, path: string): [number, number] | null {
  const segs = path ? segmentsOf(path) : null;
  if (!segs) return null;
  const found = walk(doc.contents, segs, 0, null);
  if (!found) return null;
  if (found.pair) {
    const start = rangeOf(found.pair.key)?.[0];
    const end = pairEnd(found.pair);
    return start === undefined || end === null ? null : [start, end];
  }
  const r = rangeOf(found.node);
  if (!r) return null;
  if (isMap(found.node) && found.node.items.length > 0) {
    const end = pairEnd(found.node.items[0]);
    return end === null ? null : [r[0], end];
  }
  return [r[0], r[1]];
}

/**
 * Is this mapping a task or a trigger? It has a string `type`, and either an
 * `id` or it is an item of a list. That skips a retry policy's own
 * `type: constant` (a mapping under a key, with no id), as Kestra's
 * `isTaskLike` does, and still finds a task whose `type:` was typed before its
 * `id:`.
 */
function typeOfTask(map: YAMLMap, isItem: boolean): string | null {
  const type = scalarOf(map, "type");
  if (typeof type !== "string") return null;
  return isItem || scalarOf(map, "id") != null ? type : null;
}

function typeIn(doc: YamlDoc, offset: number): string | null {
  let found: string | null = null;
  const visit = (node: unknown, isItem: boolean) => {
    const r = rangeOf(node);
    if (!r || offset < r[0] || offset > r[1]) return;
    if (isMap(node)) {
      found = typeOfTask(node, isItem) ?? found;
      for (const p of node.items) visit(p.value, false);
    } else if (isSeq(node)) {
      for (const item of node.items) visit(item, true);
    }
  };
  visit(doc.contents, false);
  return found;
}

/**
 * Where `current` differs from `before`: one region, found by the common
 * prefix and suffix. An edit while typing is one region, so an offset outside
 * it moves by the length difference and an offset inside it has no
 * counterpart.
 */
function editRegion(before: string, current: string) {
  const max = Math.min(before.length, current.length);
  let p = 0;
  while (p < max && before[p] === current[p]) p++;
  let s = 0;
  while (s < max - p && before[before.length - 1 - s] === current[current.length - 1 - s]) s++;
  const delta = current.length - before.length;
  return {
    toCurrent: (o: number) => (o < p ? o : o >= before.length - s ? o + delta : null),
    toBefore: (o: number) => (o < p ? o : o >= current.length - s ? o - delta : null),
  };
}

/** Lookups in one source, falling back to the last source that parsed. */
export type YamlView = {
  doc: YamlDoc;
  rangeOfPath: (path: string) => YamlRange | null;
  typeAt: (offset: number) => string | null;
};

/**
 * The lookups for `doc`. While `doc` has errors (normal while typing: a new
 * key is an error until its `:`), a lookup outside the edited region reads
 * `lastGood`, the last source that parsed, translated to the current text: an
 * open quote can swallow the rest of the document into one string, and that
 * must not move every finding to line 1. Inside the edited region the
 * error-tolerant tree of `doc` itself is read.
 */
export function createYamlView(doc: YamlDoc, lastGood: YamlDoc | null): YamlView {
  const at = (start: number, end: number): YamlRange => {
    const s = doc.lineCounter.linePos(start);
    const e = doc.lineCounter.linePos(end);
    return { startLine: s.line, startCol: s.col, endLine: e.line, endCol: e.col };
  };
  const fallback = !doc.ok && lastGood && lastGood.source !== doc.source ? lastGood : null;
  const region = fallback ? editRegion(fallback.source, doc.source) : null;
  return {
    doc,
    rangeOfPath(path) {
      if (fallback && region) {
        const old = offsetsOfPath(fallback, path);
        const start = old && region.toCurrent(old[0]);
        const end = old && region.toCurrent(old[1]);
        if (start != null && end != null) return at(start, end);
      }
      const own = offsetsOfPath(doc, path);
      return own ? at(own[0], own[1]) : null;
    },
    typeAt(offset) {
      if (fallback && region) {
        const old = region.toBefore(offset);
        if (old !== null) return typeIn(fallback, old);
      }
      return typeIn(doc, offset);
    },
  };
}

/**
 * Map a finding's `path` to its range in `source`. Parses `source`; a caller
 * with many paths uses {@link createYamlView} on one {@link parseYaml}.
 *
 * Two spellings are accepted, and may mix:
 * - by index, `tasks[2].retry.maxAttempt`;
 * - by id, as egret's validator writes them (`crates/egret-workflow/src/spec/validate.rs`):
 *   a list of tasks, inputs or triggers is read by id (`tasks.extract.retry`),
 *   and a flowable's child is named right after its parent (`tasks.parallel.child`).
 *
 * A key maps to the key and, when its value is a scalar, the value; a list
 * item maps to its first entry, so a whole task is not painted. Null when the
 * path names nothing in the document.
 */
export function rangeOfPath(source: string, path: string): YamlRange | null {
  return createYamlView(parseYaml(source), null).rangeOfPath(path);
}

/**
 * The `type:` of the innermost task or trigger enclosing `offset`, or null at
 * flow level. A task or trigger is a mapping with a string `type` that has an
 * `id` or is a list item (see `typeOfTask`).
 */
export function typeAt(source: string, offset: number): string | null {
  return createYamlView(parseYaml(source), null).typeAt(offset);
}
