export interface ConfigChange {
  path: string;
  kind: "added" | "removed" | "changed";
  from?: unknown;
  to?: unknown;
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * Recursive leaf-path diff of two JSON-like values. Plain objects recurse over
 * the union of their keys; arrays and primitives are leaves compared whole by
 * `JSON.stringify` equality. Paths are dot-joined (`coordinator.replicas`).
 * A key missing in `after` → `removed`, missing in `before` → `added`, present
 * in both but differing → `changed`.
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
    if (aMissing) {
      changes.push({ path, kind: "added", to: b });
      return;
    }
    if (bMissing) {
      changes.push({ path, kind: "removed", from: a });
      return;
    }
    if (JSON.stringify(a) !== JSON.stringify(b)) {
      changes.push({ path, kind: "changed", from: a, to: b });
    }
  }

  walk(before, after, "");
  return changes;
}
