export interface ConfigChange {
  path: string;
  kind: "added" | "removed" | "changed";
  from?: unknown;
  to?: unknown;
}

function isEmptyArray(v: unknown): boolean {
  return Array.isArray(v) && v.length === 0;
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * `JSON.stringify` with object keys sorted recursively so two values that differ
 * only in object-key order compare equal. Array element order is preserved —
 * element order IS semantically meaningful in config (e.g. an ordered list of
 * tolerations/containers); only object-key order is canonicalized, since key
 * order in a JSON object never carries meaning.
 */
function stableStringify(v: unknown): string {
  if (Array.isArray(v)) {
    return `[${v.map(stableStringify).join(",")}]`;
  }
  if (isPlainObject(v)) {
    const body = Object.keys(v)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${stableStringify(v[k])}`)
      .join(",");
    return `{${body}}`;
  }
  return JSON.stringify(v) ?? "null";
}

/**
 * Recursive leaf-path diff of two JSON-like values. Plain objects recurse over
 * the union of their keys; arrays and primitives are leaves compared whole by
 * `JSON.stringify` equality. Paths are dot-joined (`coordinator.replicas`).
 * A key missing in `after` → `removed`, missing in `before` → `added`, present
 * in both but differing → `changed`. A missing key and an empty array are the
 * same config (the server reads an absent list as empty), so that pair is no
 * change: the review must list only what the save changes (KI-198).
 */
export function diffConfig(before: unknown, after: unknown): ConfigChange[] {
  const changes: ConfigChange[] = [];

  function walk(a: unknown, b: unknown, path: string) {
    if (isPlainObject(a) && isPlainObject(b)) {
      const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
      for (const key of keys) {
        walk(a[key], b[key], path ? `${path}.${key}` : key);
      }
      return;
    }

    const aMissing = a === undefined;
    const bMissing = b === undefined;
    if (aMissing && bMissing) return;
    if ((aMissing && isEmptyArray(b)) || (bMissing && isEmptyArray(a))) return;
    if (aMissing) {
      changes.push({ path, kind: "added", to: b });
      return;
    }
    if (bMissing) {
      changes.push({ path, kind: "removed", from: a });
      return;
    }
    if (stableStringify(a) !== stableStringify(b)) {
      changes.push({ path, kind: "changed", from: a, to: b });
    }
  }

  walk(before, after, "");
  return changes;
}
