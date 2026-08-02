import { SERIES_PALETTE } from "./palette";

/** Names that carry meaning, so the colour should carry it too rather than fall out of a hash.
 *  Metabase's `getPreferredColor` (ui/colors/groups.ts); the hex values are its `success`,
 *  `error` and `warning` theme colours converted from hsla. `success` is byte-identical to
 *  egret's own summarize green, which is the cross-check that the conversion is right. */
export const PREFERRED_COLORS = {
  success: "#689735", // palm[50]
  error: "#e3595e", // lobster[50]
  warning: "#e8b921", // dubloon[30]
} as const;

const PREFERRED_BY_NAME: Record<string, string> = {};
const preferred = (color: string, ...names: string[]) => {
  names.forEach((n) => (PREFERRED_BY_NAME[n] = color));
};
preferred(PREFERRED_COLORS.success, "success", "succeeded", "pass", "passed", "valid",
  "complete", "completed", "accepted", "active", "profit");
preferred(PREFERRED_COLORS.error, "cancel", "canceled", "cancelled", "error", "fail", "failed",
  "failure", "failures", "invalid", "rejected", "inactive", "loss", "cost", "deleted", "pending");
preferred(PREFERRED_COLORS.warning, "warn", "warning", "incomplete", "unstable");
preferred(SERIES_PALETTE[0], "count");
preferred(SERIES_PALETTE[1], "sum");
preferred(SERIES_PALETTE[2], "average");

const preferredFor = (key: string): string | undefined => PREFERRED_BY_NAME[key.toLowerCase()];

/** Metabase's `getHashCode`. `Math.imul` keeps the multiply in int32, which is what makes the
 *  colour a given column gets reproducible across cards — and identical to Metabase's. */
function hashCode(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (Math.imul(31, h) + s.charCodeAt(i)) | 0;
  return Math.abs(h);
}

/**
 * A colour per key, stable under reordering and shared across cards, because it derives from the
 * KEY rather than from the key's position. Metabase's `getColorsForValues`.
 *
 * `existing` is the user's explicit choice — `seriesColors`, `sliceColors`. It wins, and it also
 * CONSUMES its colour: whoever else would have hashed onto it moves along. That is deliberate
 * (Metabase's first pass marks the value used) and is what stops a pinned series and an unpinned
 * one from being drawn the same.
 */
export function colorsForKeys(
  keys: string[],
  existing?: Record<string, string> | null,
): Record<string, string> {
  return keys.length <= SERIES_PALETTE.length
    ? hashMapping(keys, [...SERIES_PALETTE], existing)
    : orderMapping(keys, harmonyColors(keys.length), existing);
}

function hashMapping(
  keys: string[],
  values: string[],
  existing?: Record<string, string> | null,
): Record<string, string> {
  const mapping: Record<string, string> = {};
  const sorted = [...keys].sort();
  const hashes = Object.fromEntries(keys.map((k) => [k, hashCode(k)]));
  const unset = new Set(keys);
  const used = new Set<string>();
  const unused = new Set(values);

  const take = (key: string, value: string) => {
    mapping[key] = value;
    unset.delete(key);
    used.add(value);
    unused.delete(value);
  };

  sorted.forEach((key) => {
    const value = existing?.[key];
    if (value) take(key, value);
  });
  sorted.forEach((key) => {
    if (mapping[key]) return;
    const value = preferredFor(key);
    if (value && !used.has(value)) take(key, value);
  });
  // `attempt` walks the key off its first choice when that one is taken. Refilling `unused` when it
  // empties is what makes this terminate for more keys than colours — with a repeat, not a hang.
  for (let attempt = 0; unset.size > 0; attempt++) {
    if (!unused.size) values.forEach((v) => unused.add(v));
    sorted.forEach((key) => {
      if (mapping[key]) return;
      const value = values[(hashes[key] + attempt) % values.length];
      if (unused.has(value)) take(key, value);
    });
  }
  return mapping;
}

// --- Task 2 replaces both of these with the real harmony path. -----------------------------------
// They exist so this file compiles and the hash path can be tested on its own; nothing above eight
// keys is correct yet.

function harmonyColors(_count: number): string[] {
  return [...SERIES_PALETTE];
}

function orderMapping(
  keys: string[],
  values: string[],
  existing?: Record<string, string> | null,
): Record<string, string> {
  return hashMapping(keys, values, existing);
}
