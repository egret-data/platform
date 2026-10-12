import type { ResourceLevel, ServiceDefResource } from "@/api/policies";

/**
 * What the policy editor's resource levels will mean once saved (SP4d-4 B1).
 *
 * The server saves an EMPTY non-root level as `*`, refuses an empty root, and
 * refuses a named level whose parent level is absent. These pure helpers
 * mirror those rules over the service-def hierarchy so the form can say so
 * before the save instead of after it.
 */

/**
 * Whether a level names anything. Mirrors the server's `level_is_empty`: a
 * level whose values are all blank or whitespace is EMPTY (`[""]`, `[" "]`),
 * so it is filled or refused exactly like `[]`.
 */
export const levelHasValue = (lvl: ResourceLevel | undefined): boolean =>
  (lvl?.values ?? []).some((v) => typeof v === "string" && v.trim() !== "");
const hasValues = levelHasValue;

const parentOf = (defs: ServiceDefResource[], name: string): string | undefined =>
  defs.find((d) => d.name === name)?.parent ?? undefined;

/**
 * The levels the server will save as `*`: a non-root level that is present in
 * the map with no value, under a parent level that is present. Root first.
 */
export function levelsToFill(
  defs: ServiceDefResource[],
  value: Record<string, ResourceLevel>,
): string[] {
  return Object.keys(value)
    .filter((name) => {
      const parent = parentOf(defs, name);
      return !!parent && !hasValues(value[name]) && parent in value;
    })
    .sort((a, b) => depth(defs, a) - depth(defs, b));
}

/**
 * Named levels whose parent level is absent: the server refuses these. Each
 * entry is the named level and the parent that is missing.
 */
export function levelGaps(
  defs: ServiceDefResource[],
  value: Record<string, ResourceLevel>,
): { level: string; missing: string }[] {
  return Object.keys(value).flatMap((name) => {
    const parent = parentOf(defs, name);
    return parent && hasValues(value[name]) && !(parent in value)
      ? [{ level: name, missing: parent }]
      : [];
  });
}

/**
 * The deepest level a chain stops at when it stops above `column` on the
 * catalog/schema/table path (so the policy grants no column read), or `null`
 * when it reaches column, is empty, or is not on that path (role, user…).
 */
export function stopsAboveColumn(
  defs: ServiceDefResource[],
  value: Record<string, ResourceLevel>,
): string | null {
  if (!defs.some((d) => d.name === "column")) return null;
  const above: string[] = [];
  for (let p = parentOf(defs, "column"); p; p = parentOf(defs, p)) above.push(p);
  const keys = Object.keys(value);
  if (keys.length === 0 || keys.includes("column")) return null;
  if (!keys.every((k) => above.includes(k))) return null;
  return [...keys].sort((a, b) => depth(defs, b) - depth(defs, a))[0];
}

/** Whether these access types include a column read (`select`, or `all`, which implies it). */
export const grantsSelect = (types: readonly string[] | undefined): boolean =>
  !!types && (types.includes("select") || types.includes("all"));

function depth(defs: ServiceDefResource[], name: string): number {
  let n = 0;
  for (let p = parentOf(defs, name); p; p = parentOf(defs, p)) n += 1;
  return n;
}
