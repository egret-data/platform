import { chartShade, chartTint, SERIES_PALETTE } from "./palette";

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
 *
 * Keys are DE-DUPLICATED, because the result is a Record keyed by them: two identical strings in
 * `keys` share one entry, so `Object.keys(out).length` can be less than `keys.length`. Callers must
 * look each colour up BY NAME rather than zipping the values back against the input array.
 */
export function colorsForKeys(
  keys: string[],
  existing?: Record<string, string> | null,
): Record<string, string> {
  return keys.length <= SERIES_PALETTE.length
    ? hashMapping(keys, [...SERIES_PALETTE], existing)
    : orderMapping(keys, harmonyColors(keys.length), existing);
}

/** Two spellings of one ink. `#F2A86F` and `#f2a86f` are the SAME COLOUR and must occupy one slot:
 *  SERIES_PALETTE is written upper-case, PREFERRED_COLORS lower-case, and a colour that came back
 *  from a native `<input type="color">` is lower-cased by the DOM whatever went in. Comparing those
 *  as raw strings lets a pinned series keep a colour without consuming it, and the hash pass then
 *  hands the very same ink to somebody else. */
const canon = (color: string) => color.toLowerCase();

/**
 * The used/unused bookkeeping, as one object so that BOTH mapping strategies are case-insensitive
 * by construction rather than by each remembering to be. Only the comparisons normalise — what gets
 * written into the mapping is the caller's own spelling, untouched.
 *
 * `unset` is the hash path's worklist, and is OPTIONAL because the order path has no attempt loop to
 * drive: it walks the keys once and every key it visits it assigns, so there is nothing to track.
 * Handing it a Set it would only write to and never read would be bookkeeping pretending to be a
 * shared contract.
 */
function colorLedger(values: string[], mapping: Record<string, string>, unset?: Set<string>) {
  const used = new Set<string>();
  const unused = new Set(values.map(canon));
  return {
    take(key: string, value: string) {
      mapping[key] = value;
      unset?.delete(key);
      used.add(canon(value));
      unused.delete(canon(value));
    },
    isUsed: (value: string) => used.has(canon(value)),
    isFree: (value: string) => unused.has(canon(value)),
    get spent() {
      return unused.size === 0;
    },
    /**
     * The first still-free value in `values`' OWN order — what the order path hands out, and
     * `undefined` once nothing is free (or when there were no values to begin with).
     *
     * Scans `values` rather than reading the head of `unused`, and that is the whole point: `unused`
     * holds canonicalised spellings, so taking its first element would write a lower-cased colour
     * into the mapping where the caller passed `#509EE3`. Insertion order makes the two scans pick
     * the same slot either way; only this one preserves the caller's spelling.
     */
    get nextFree(): string | undefined {
      return values.find((v) => unused.has(canon(v)));
    },
    refill: () => values.forEach((v) => unused.add(canon(v))),
  };
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
  const ledger = colorLedger(values, mapping, unset);

  sorted.forEach((key) => {
    const value = existing?.[key];
    if (value) ledger.take(key, value);
  });
  sorted.forEach((key) => {
    if (mapping[key]) return;
    const value = preferredFor(key);
    if (value && !ledger.isUsed(value)) ledger.take(key, value);
  });
  // Nothing left to hash into. Guarded because the loop below only ever exits by assigning every
  // key, and with no values to assign `% values.length` is NaN, `values[NaN]` is undefined, and it
  // would spin forever. Unreachable from either call site today — but the loop's termination should
  // not rest on an invariant held somewhere else.
  if (!values.length) return mapping;
  // `attempt` walks the key off its first choice when that one is taken. Refilling `unused` when it
  // empties is what makes this terminate for more keys than colours — with a repeat, not a hang.
  for (let attempt = 0; unset.size > 0; attempt++) {
    if (ledger.spent) ledger.refill();
    sorted.forEach((key) => {
      if (mapping[key]) return;
      const value = values[(hashes[key] + attempt) % values.length];
      if (ledger.isFree(value)) ledger.take(key, value);
    });
  }
  return mapping;
}

/**
 * The palette, widened by interleaving each accent with its variants — Metabase's
 * `getAccentColors({ harmony: true })`, which unzips [main, light, dark] rather than concatenating
 * them, so neighbouring series stay far apart in hue instead of walking through eight blues.
 *
 * The light variants join only past twice the accent count, exactly as Metabase gates them: at
 * nine keys a base/dark pair per accent is already sixteen colours, and pulling in a third
 * near-identical tint would make two series harder to tell apart, not easier.
 */
function harmonyColors(count: number): string[] {
  const ranges = count > SERIES_PALETTE.length * 2
    ? [SERIES_PALETTE, SERIES_PALETTE.map(chartTint), SERIES_PALETTE.map(chartShade)]
    : [SERIES_PALETTE, SERIES_PALETTE.map(chartShade)];
  return SERIES_PALETTE.flatMap((_, i) => ranges.map((r) => r[i]));
}

/**
 * The same three passes as `hashMapping`, in the keys' own order and with no hash: past eight keys
 * there are enough colours that position is as good a spread as a hash, and Metabase makes the same
 * switch (`getOrderBasedMapping`).
 *
 * Shares `colorLedger` with the hash path rather than keeping its own `Set<string>` pair — not for
 * brevity, but because comparing colours as raw strings is a live bug (see `canon`): a pinned
 * `#509ee3` that does not consume `#509EE3`'s slot gets handed straight back out to the next key,
 * and two series are drawn identically. Routing every comparison through the ledger is what stops
 * this path from having to rediscover that.
 */
function orderMapping(
  keys: string[],
  values: string[],
  existing?: Record<string, string> | null,
): Record<string, string> {
  const mapping: Record<string, string> = {};
  const ledger = colorLedger(values, mapping);

  keys.forEach((key) => {
    const value = existing?.[key];
    if (value) ledger.take(key, value);
  });
  keys.forEach((key) => {
    if (mapping[key]) return;
    const value = preferredFor(key);
    if (value && !ledger.isUsed(value)) ledger.take(key, value);
  });
  keys.forEach((key) => {
    if (mapping[key]) return;
    // More keys than colours: start the harmony over rather than leave the tail uncoloured. The
    // repeat is Metabase's own answer too, and unlike the hash path's loop this pass visits each key
    // exactly once, so it cannot spin — an empty `values` just leaves keys unmapped.
    if (ledger.spent) ledger.refill();
    const value = ledger.nextFree;
    if (value) ledger.take(key, value);
  });
  return mapping;
}
