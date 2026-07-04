/**
 * Tiny helpers shared by the Pod / Placement / Containers & Mounts config
 * sections (split out of the former `PodSchedulingSection`).
 */

/** The sentinel a `Select` uses to represent "unset / cluster default". */
export const SELECT_NONE = "__default__";

/** Parse a text field to a number, or `undefined` when blank/invalid. */
export function numOrUndef(s: string): number | undefined {
  const t = s.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isFinite(n) ? n : undefined;
}
