# Chart parity PR 04 — colour assignment — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Assign series colours from a hash of the series name, the way Metabase does, so a column
keeps its colour across every card on a dashboard and a dashboard stops reading as monotone.

**Architecture:** One new module, `echarts/keyColors.ts`, exporting `colorsForKeys(keys, existing?)`
— a clean-room of Metabase's `ui/colors/charts.ts`. Every option builder and every settings panel
that shows a default swatch calls it. The palettes are already identical, so this PR changes
*assignment* only.

**Tech stack:** TypeScript, React, ECharts 6.1, vitest.

---

## What the harness measured, before any change

Both dashboards, identical data, read out of the DOM (`fill` on `path`/`rect`/`circle`, elements
under 3px excluded). This is the baseline the acceptance test compares against.

| chart | egret | Metabase |
|---|---|---|
| bar | `#689735` ×4 | `#f9d45c` ×4 |
| row | `#689735` ×4 | `#f9d45c` ×4 |
| line | `#689735` | `#f9d45c` |
| area | `#689735` ×2 | `#f9d45c` |
| scatter | `#689735` ×168 | `#ef8c8c` ×196 |
| pie | `#509ee3` `#88bf4d` `#a989c5` `#ef8c8c` | `#f2a86f` `#f9d45c` `#a989c5` `#88bf4d` |
| sankey (9 nodes) | `#509ee3` ×10, `#f9d45c`, `#f2a86f`, `#7172ad`, `#88bf4d` | `#509ee3`, `#227fd2`, `#88bf4d`, `#689636`, `#a989c5` |
| waterfall | `#88bf4d` ×12 | `#88bf4d` ×13 |
| box plot | `#509ee3` | `#88bf4d` |

Three things follow, and the first contradicts both the spec and two code comments:

1. **Metabase does not tint a lone series with the summarize green.** `options/cartesian.ts:591` and
   `:730` both say it does ("A lone series takes the summarize green, as everywhere else in the
   family" / "Metabase tints a lone series with its summarize green rather than accent blue"), and
   the design spec repeats it. Measured, Metabase draws those same lone-series charts in `#f9d45c`
   and `#ef8c8c` — hash-derived from the column name. `#689735` is `palm[50]`, which Metabase uses
   as its `success` colour and returns from `getPreferredColor` for names like `profit` and
   `completed`. It is not a lone-series rule. **Those two comments must be corrected, not just the
   code.**
2. **The `i % 8` wrap is observable in the harness.** egret's sankey has 9 nodes and gives two of
   them `#509ee3`. Metabase's ninth node gets `#227fd2` — `accent0-dark` — from the harmony
   extension. The sankey is therefore this PR's test case for the `> 8` path, live.
3. **The waterfall already agrees** and must stay agreeing: its green/red are semantic
   (increase/decrease), not palette slots. Same for the gauge's zone colours.

## Metabase's algorithm, as read from source

`frontend/src/metabase/ui/colors/charts.ts` — `getColorsForValues(keys, existingMapping)`:

- `keys.length <= 8` → **hash-based mapping** over the eight main accents.
- otherwise → **order-based mapping** over a harmony-interleaved list.

**Hash-based** (`getHashBasedMapping`), in three passes over `[...keys].sort()`:
1. Keys with an entry in `existingMapping` take it, and that colour is **marked used** — an explicit
   override consumes a slot.
2. Remaining keys take their preferred colour if it is not already used.
3. Remaining keys, looping with an `attempt` counter: `values[(hash(key) + attempt) % values.length]`,
   taken only if still unused. When `unusedValues` empties it is refilled with all values, so the
   loop terminates with duplicates rather than hanging.

**Order-based** (`getOrderBasedMapping`) is the same three passes over the keys in their given order,
with no hashing — the third pass just takes the first still-unused value.

**Hash** (`getHashCode`): `h = (Math.imul(31, h) + s.charCodeAt(i)) | 0`, then `Math.abs(h)`.

**Preferred colours** (`ui/colors/groups.ts`, `getPreferredColor`), lower-cased key:

| names | colour |
|---|---|
| `success` `succeeded` `pass` `passed` `valid` `complete` `completed` `accepted` `active` `profit` | `#689735` (palm[50]) |
| `cancel` `canceled` `cancelled` `error` `fail` `failed` `failure` `failures` `invalid` `rejected` `inactive` `loss` `cost` `deleted` `pending` | `#e3595e` (lobster[50]) |
| `warn` `warning` `incomplete` `unstable` | `#e8b921` (dubloon[30]) |
| `count` | `accent0` `#509EE3` |
| `sum` | `accent1` `#88BF4D` |
| `average` | `accent2` `#A989C5` |

The three hex values were converted from Metabase's `hsla()` base colours. `palm[50]` =
`hsla(89, 48%, 40%)` = `#689735`, which is byte-identical to egret's existing `SUMMARIZE_COLOR` —
that identity is the cross-check that the conversion is right, and Task 1 asserts it.

**Harmony list** (`getAccentColors({ light: keys.length > 16, harmony: true, gray: false })`): ranges
are pushed in the order main, light, dark, then `_.unzip(...).flat()`. So for 9–16 keys the list is
`[a0, a0-dark, a1, a1-dark, …]` (16 entries) and beyond 16 it is `[a0, a0-light, a0-dark, a1, …]`
(24 entries). egret already has `chartTint` / `chartShade`, which are `accentN-light` /
`accentN-dark` under the same ±12.5 lightness points Metabase uses (`CHART_TINT_SHADE_FACTOR = 0.125`).

## Deliberate divergences from the design spec

- **The spec says to add `colorsForKeys` to `echarts/palette.ts`.** It goes in a new
  `echarts/keyColors.ts` instead: `palette.ts` is already ~700 lines and holds colour *primitives*
  (parsing, mixing, contrast), while this is an *assignment policy* over them. Stated here so the
  divergence is a decision and not a drift.
- **`seriesVizSettingsDefaultKeys` is not ported.** It exists so Metabase can hash a series' original
  key after a user renames it; egret has no stored original-key concept, and inventing one is out of
  scope.
- **The `palette` parameter is not ported.** egret has one palette.

## Accepted consequences

- **Every chart that has not set an explicit colour changes appearance once.** Decided in the design
  spec; a migration writing current colours into `seriesColors` was rejected as it would inflate
  stored settings to preserve an appearance nobody chose.
- **An explicit override now consumes its colour**, so overriding one series can change another's.
  That is Metabase's behaviour (pass 1 marks the value used) and is what keeps a dashboard from
  drawing two series in the same colour after one is pinned.
- **Assignment no longer depends on series order** for ≤ 8 keys, because the hash pass walks sorted
  keys. Reordering metrics in the config keeps colours stable.

## File structure

| file | responsibility |
|---|---|
| `echarts/keyColors.ts` (new) | `colorsForKeys`, the hash, the preferred-colour table, the harmony list |
| `echarts/__tests__/keyColors.test.ts` (new) | the algorithm, against Metabase's own spec cases |
| `echarts/options/cartesian.ts` | breakout groups and metric series call it; lone-series green goes |
| `echarts/options/sankey.ts` | node names call it |
| `echarts/options/pie.ts` | ring-0 slice names call it |
| `settings/CartesianDisplaySection.tsx` | `defaultSeriesColor` delegates |
| `settings/PieDisplaySection.tsx` | default swatch delegates |
| `echarts/palette.ts` | `SUMMARIZE_COLOR` becomes the success entry's source, or is removed |

Out of scope, and each for a stated reason: the waterfall's `INCREASE`/`DECREASE` and the gauge's
`ZONE_*` are semantic, not palette slots; the box plot, map, progress and funnel take a single
user-set colour with `SERIES_PALETTE[0]`/`[1]` as the default, which is a default and not an
assignment; `SmartScalarChart`'s up/down are semantic.

---

## Task 1: `colorsForKeys` — the hash path

**Files:**
- Create: `frontend/src/components/chart/echarts/keyColors.ts`
- Create: `frontend/src/components/chart/echarts/__tests__/keyColors.test.ts`

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, expect, it } from "vitest";
import { colorsForKeys, PREFERRED_COLORS } from "../keyColors";
import { SERIES_PALETTE } from "../palette";

describe("colorsForKeys", () => {
  it("gives every key a distinct accent when there are no more than eight", () => {
    const keys = ["alpha", "beta", "gamma", "delta"];
    const out = colorsForKeys(keys);
    expect(Object.keys(out).sort()).toEqual([...keys].sort());
    expect(new Set(Object.values(out)).size).toBe(4);
    Object.values(out).forEach((c) => expect(SERIES_PALETTE).toContain(c));
  });

  it("keys a colour to the NAME, so order cannot change it", () => {
    const a = colorsForKeys(["revenue", "orders", "profit"]);
    const b = colorsForKeys(["profit", "revenue", "orders"]);
    expect(b).toEqual(a);
  });

  it("gives a semantic name its own colour ahead of the hash", () => {
    const out = colorsForKeys(["profit", "loss", "widgets"]);
    expect(out.profit).toBe(PREFERRED_COLORS.success);
    expect(out.loss).toBe(PREFERRED_COLORS.error);
  });

  it("matches on case, not on exact spelling", () => {
    expect(colorsForKeys(["Completed"]).Completed).toBe(PREFERRED_COLORS.success);
  });

  it("lets an explicit colour win, and does not hand it to anyone else", () => {
    const out = colorsForKeys(["a", "b", "c"], { a: SERIES_PALETTE[2] });
    expect(out.a).toBe(SERIES_PALETTE[2]);
    expect(out.b).not.toBe(SERIES_PALETTE[2]);
    expect(out.c).not.toBe(SERIES_PALETTE[2]);
  });

  it("takes the success colour from the summarize green, byte for byte", () => {
    // palm[50] = hsla(89, 48%, 40%). If this ever drifts, one of the two is wrong.
    expect(PREFERRED_COLORS.success).toBe("#689735");
  });

  it("terminates when there are more keys than colours in the hash path", () => {
    // Eight is the boundary: still the hash path, and every accent is spent.
    const keys = Array.from({ length: 8 }, (_, i) => `k${i}`);
    expect(Object.keys(colorsForKeys(keys))).toHaveLength(8);
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/keyColors.test.ts`
Expected: FAIL — `Failed to resolve import "../keyColors"`.

- [ ] **Step 3: Write the module**

```ts
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
```

- [ ] **Step 4: Stub `orderMapping` and `harmonyColors` so the file compiles**

```ts
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
```

Task 2 replaces both. They are stubs so this task's tests can run green on their own; Task 2's
tests fail against them, which is the point.

- [ ] **Step 5: Run the tests**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/keyColors.test.ts`
Expected: PASS, 7 tests.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/echarts/keyColors.ts \
        frontend/src/components/chart/echarts/__tests__/keyColors.test.ts
git commit -m "feat(charts): a series colour derives from its name, not its position"
```

---

## Task 2: the harmony path, beyond eight keys

**Files:**
- Modify: `frontend/src/components/chart/echarts/keyColors.ts`
- Modify: `frontend/src/components/chart/echarts/__tests__/keyColors.test.ts`

- [ ] **Step 1: Write the failing tests**

```ts
import { chartShade, chartTint } from "../palette";

describe("colorsForKeys beyond eight", () => {
  it("extends the harmony rather than repeating an accent", () => {
    const keys = Array.from({ length: 9 }, (_, i) => `n${i}`);
    const out = colorsForKeys(keys);
    expect(new Set(Object.values(out)).size).toBe(9);
  });

  it("interleaves base and dark, so the ninth is accent0's dark", () => {
    // The sankey case, measured live in Metabase: a0, a0-dark, a1, a1-dark, …
    const keys = Array.from({ length: 9 }, (_, i) => `n${i}`);
    const out = colorsForKeys(keys);
    expect(Object.values(out)).toContain(chartShade(SERIES_PALETTE[0]));
  });

  it("brings in the light variants only past sixteen keys", () => {
    const under = colorsForKeys(Array.from({ length: 16 }, (_, i) => `n${i}`));
    expect(Object.values(under)).not.toContain(chartTint(SERIES_PALETTE[0]));
    const over = colorsForKeys(Array.from({ length: 17 }, (_, i) => `n${i}`));
    expect(Object.values(over)).toContain(chartTint(SERIES_PALETTE[0]));
  });

  it("assigns in key order past eight, without hashing", () => {
    const keys = ["z", "y", "x", "w", "v", "u", "t", "s", "r"];
    const out = colorsForKeys(keys);
    expect(out.z).toBe(SERIES_PALETTE[0]);
    expect(out.y).toBe(chartShade(SERIES_PALETTE[0]));
  });

  it("still lets a semantic name and an explicit colour win", () => {
    const keys = ["profit", ...Array.from({ length: 9 }, (_, i) => `n${i}`)];
    const out = colorsForKeys(keys, { n0: SERIES_PALETTE[5] });
    expect(out.profit).toBe(PREFERRED_COLORS.success);
    expect(out.n0).toBe(SERIES_PALETTE[5]);
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/keyColors.test.ts`
Expected: FAIL — the stubs hash over eight accents, so `new Set(...).size` is 8, not 9.

- [ ] **Step 3: Replace the stubs**

```ts
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

/** The same three passes as `hashMapping`, in the keys' own order and with no hash: past eight
 *  keys there are enough colours that position is as good a spread as a hash, and Metabase makes
 *  the same switch. */
function orderMapping(
  keys: string[],
  values: string[],
  existing?: Record<string, string> | null,
): Record<string, string> {
  const mapping: Record<string, string> = {};
  const used = new Set<string>();
  const unused = new Set(values);

  const take = (key: string, value: string) => {
    mapping[key] = value;
    used.add(value);
    unused.delete(value);
  };

  keys.forEach((key) => {
    const value = existing?.[key];
    if (value) take(key, value);
  });
  keys.forEach((key) => {
    if (mapping[key]) return;
    const value = preferredFor(key);
    if (value && !used.has(value)) take(key, value);
  });
  keys.forEach((key) => {
    if (mapping[key]) return;
    if (!unused.size) values.forEach((v) => unused.add(v));
    const [value] = unused;
    take(key, value);
  });
  return mapping;
}
```

- [ ] **Step 4: Run the tests**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/keyColors.test.ts`
Expected: PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/keyColors.ts \
        frontend/src/components/chart/echarts/__tests__/keyColors.test.ts
git commit -m "feat(charts): past eight series the harmony widens instead of repeating"
```

---

## Task 3: cartesian — and the end of the lone-series green

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts:591`, `:730`, `:799`
- Modify: `frontend/src/components/chart/echarts/__tests__/cartesianColors.test.ts` (create)

- [ ] **Step 1: Write the failing test**

```ts
import { describe, expect, it } from "vitest";
import { colorsForKeys } from "../keyColors";
import { SUMMARIZE_GREEN_IS_GONE } from "./helpers"; // see step 3 — delete this import line

describe("cartesian series colours", () => {
  it("does not give a lone series the summarize green", () => {
    // Metabase, measured on identical data, draws this card #f9d45c. The green was egret's.
    const opt = buildCartesian({ ...baseCfg, yMetrics: ["revenue"] });
    expect(seriesColorsOf(opt)).not.toContain("#689735");
  });

  it("gives a lone series the colour its NAME hashes to", () => {
    const opt = buildCartesian({ ...baseCfg, yMetrics: ["revenue"] });
    expect(seriesColorsOf(opt)).toEqual([colorsForKeys(["revenue"]).revenue]);
  });

  it("gives the same column the same colour in two different charts", () => {
    const a = buildCartesian({ ...baseCfg, yMetrics: ["revenue", "orders"] });
    const b = buildCartesian({ ...baseCfg, yMetrics: ["orders", "revenue"] });
    const byName = (opt: unknown) => Object.fromEntries(
      seriesOf(opt).map((s) => [s.name, s.itemStyle?.color ?? s.lineStyle?.color]));
    expect(byName(a).revenue).toBe(byName(b).revenue);
  });

  it("colours a breakout by its group name", () => {
    const opt = buildCartesian({ ...baseCfg, series: "category" });
    const groups = ["Doohickey", "Gadget", "Gizmo", "Widget"];
    expect(seriesColorsOf(opt)).toEqual(groups.map((g) => colorsForKeys(groups)[g]));
  });
});
```

Write `baseCfg`, `buildCartesian`, `seriesOf` and `seriesColorsOf` following the existing
`echarts/__tests__/cartesianXLabels.test.ts`, which already builds a config and reads the option
back. Do not invent a second harness.

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/cartesianColors.test.ts`
Expected: FAIL — the first case receives `#689735`.

- [ ] **Step 3: Change the three call sites**

At `:591` (breakout groups), replace:

```ts
const color = cfg.seriesColors?.[g] ?? (groups.length === 1 ? SUMMARIZE_COLOR : seriesColor(si));
```

with a mapping computed once above the `.map`:

```ts
// Keyed by GROUP NAME so a breakout value keeps its colour across every card that breaks out by
// it — and so an explicit `seriesColors` entry consumes its colour rather than shadowing it.
const groupColors = colorsForKeys(groups, cfg.seriesColors);
```

and inside the map, `const color = groupColors[g];`.

At `:730`, delete `singleColor` entirely and its comment. Replace with a mapping over the metric
names, passed down where `colorOverride` used to be:

```ts
// The comment this replaces said Metabase tints a lone series with its summarize green. Measured
// on identical data it does not: a single-metric bar card draws #f9d45c, the colour "revenue"
// hashes to. The green is Metabase's `success`, which `colorsForKeys` still returns — for a series
// actually NAMED "profit" or "completed", which is the rule that was mistaken for a lone-series one.
const metricColors = colorsForKeys(cfg.yMetrics, cfg.seriesColors);
```

At `:799`, replace `cfg.seriesColors?.[s.name] ?? colorOverride ?? seriesColor(colorIdx)` with a
lookup in the mapping the caller passes in. `colorsForKeys` already folded `seriesColors` in, so
the `??` chain goes; leaving it would apply the override twice and re-introduce the shadowing this
task removes.

- [ ] **Step 4: Run the test and the whole chart suite**

Run: `cd frontend && npx vitest run src/components/chart`
Expected: the new file PASSES. Existing tests that assert `#689735` for a lone series WILL FAIL —
that is this task's intended change. Update each one to the hashed colour, and where a test name or
comment says "summarize green", correct it rather than leaving a renamed assertion.

- [ ] **Step 5: Correct the comments that stated the false rule**

Search the chart module for "summarize green" and "SUMMARIZE_COLOR". Every surviving comment must
either describe the preferred-colour rule accurately or go. If `SUMMARIZE_COLOR` has no callers
left, delete the export and let `PREFERRED_COLORS.success` be the one definition.

Run: `cd frontend && grep -rn "summarize green\|SUMMARIZE_COLOR" src/components/chart`
Expected: no stale claim that Metabase colours a lone series green.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart
git commit -m "fix(charts): a lone series is not green, and never was in Metabase"
```

---

## Task 4: sankey and pie

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/sankey.ts:171`
- Modify: `frontend/src/components/chart/echarts/options/pie.ts:997`, `:1121`
- Modify: `frontend/src/components/chart/echarts/__tests__/` — extend the existing sankey and pie specs

- [ ] **Step 1: Write the failing tests**

```ts
it("gives nine sankey nodes nine distinct colours", () => {
  // Nine is the harness's own node count, and the case where `i % 8` handed two nodes one colour.
  const opt = buildSankey(nineNodeCfg);
  const colors = opt.series[0].data.map((d) => d.itemStyle.color);
  expect(new Set(colors).size).toBe(9);
});

it("keys a sankey node's colour to its name", () => {
  const names = nodeNamesOf(nineNodeCfg);
  const opt = buildSankey(nineNodeCfg);
  expect(colorOf(opt, "Widget")).toBe(colorsForKeys(names).Widget);
});

it("keys a pie slice's colour to its name, and honours sliceColors", () => {
  const names = ["Doohickey", "Gadget", "Gizmo", "Widget"];
  const opt = buildPie({ ...pieCfg, sliceColors: { Gadget: SERIES_PALETTE[6] } });
  expect(ring0ColorOf(opt, "Gadget")).toBe(SERIES_PALETTE[6]);
  expect(ring0ColorOf(opt, "Widget")).toBe(
    colorsForKeys(names, { Gadget: SERIES_PALETTE[6] }).Widget);
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__`
Expected: FAIL — the sankey case reports `8` distinct colours for nine nodes.

- [ ] **Step 3: Change the call sites**

Sankey `:171`:

```ts
const nodeColors = colorsForKeys([...names]);
const data = [...names].map((name) => ({ name, itemStyle: { color: nodeColors[name] } }));
```

Keep the existing comment about ECharts interpolating a sankey palette — it explains why each node
states its own colour at all, which this change does not alter.

Pie `:997`: ring 0 pins its colours by NAME rather than by index. The long comment above it already
explains that a sunburst colours a top-level node by name and that an override must not slide later
slices; that argument is unchanged and the code now matches it more closely:

```ts
const ring0Colors = colorsForKeys((effectiveRings[0] ?? []).map((s) => s.name), cfg.sliceColors);
(effectiveRings[0] ?? []).forEach((s) => {
  s.itemStyle ??= { color: ring0Colors[s.name] };
});
```

At `:1121` the belt-and-braces `?? seriesColor(i)` becomes `?? ring0Colors[s.name]`. It stays a
fallback, not a second source of truth.

Check whether `cfg.sliceColors` is already applied elsewhere in `buildPieRings`. If it is, remove
that application — passing it to `colorsForKeys` is now the single place, and applying it twice
re-introduces the shadowing.

- [ ] **Step 4: Run the suite**

Run: `cd frontend && npx vitest run src/components/chart`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart
git commit -m "feat(charts): sankey nodes and pie slices take their colour from their name"
```

---

## Task 5: the settings panels must agree with the chart

**Files:**
- Modify: `frontend/src/components/chart/settings/CartesianDisplaySection.tsx:36`
- Modify: `frontend/src/components/chart/settings/PieDisplaySection.tsx:170`
- Modify: `frontend/src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx`

This is the trap in this PR. Both panels compute a default swatch **independently** of the renderer
— `defaultSeriesColor(hasBreakout, count, i)` and `SERIES_PALETTE[i % SERIES_PALETTE.length]`. Left
alone they would show the user a colour the chart does not draw, and the user would "fix" a colour
that was never wrong.

- [ ] **Step 1: Write the failing test**

```ts
it("shows the swatch the chart actually draws", () => {
  const names = ["revenue", "orders"];
  expect(names.map((n, i) => defaultSeriesColor(names, i)))
    .toEqual(names.map((n) => colorsForKeys(names)[n]));
});

it("shows a lone series its hashed colour, not the summarize green", () => {
  expect(defaultSeriesColor(["revenue"], 0)).not.toBe("#689735");
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx`
Expected: FAIL — `defaultSeriesColor` takes `(hasBreakout, count, i)` and returns the green.

- [ ] **Step 3: Change the signature to take the names**

```ts
/** The colour a series shows before any override — the renderer's own answer, asked the same way.
 *  This took (hasBreakout, count, index) and re-derived the rule from the index, which is how a
 *  panel comes to offer a swatch the chart does not draw. */
export function defaultSeriesColor(seriesNames: string[], i: number): string {
  return colorsForKeys(seriesNames)[seriesNames[i]];
}
```

Update every caller. The panel must pass the same names the builder passes — the metric names for a
plain cartesian, the group names for a breakout. If a caller does not have the names in scope, that
is the bug this task exists to find: thread them, do not reconstruct them.

Do the same for `PieDisplaySection.tsx:170`, replacing
`config.sliceColors?.[name] ?? SERIES_PALETTE[i % SERIES_PALETTE.length]` with a lookup in
`colorsForKeys(sliceNames, config.sliceColors)`.

- [ ] **Step 4: Run the settings suite**

Run: `cd frontend && npx vitest run src/components/chart/settings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/settings
git commit -m "fix(charts): the colour picker shows the colour the chart draws"
```

---

## Task 6: the live comparison, and the PR

**This is the acceptance test.** The three previous PRs in this programme each shipped a defect that
passed every unit test and was caught only here. A simulation is not evidence.

- [ ] **Step 1: Gates**

```bash
cd frontend && npx tsc --noEmit && npm run lint && npx vitest run
```
Expected: no type errors, lint rc 0, all tests passing. Report the exact test count.

- [ ] **Step 2: Build and serve**

```bash
cd frontend && npm run build
```

A static server on :5174 serves `frontend/dist` and proxies `/api`, `/catalog`, `/oauth` to
`127.0.0.1:30080`. If it is not running, the script pattern is in the session scratchpad as
`serve.py`.

- [ ] **Step 3: Measure both sides**

egret `http://127.0.0.1:5174/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`,
Metabase `http://localhost:3033/dashboard/2`. Wait ~14s. Read `fill` off `path`/`rect`/`circle`,
skipping anything under 3px, and tabulate per chart — the same query that produced the baseline
table at the top of this plan.

Acceptance criteria:

1. **No chart draws every series in `#689735`.** Five do today: bar, row, line, area, scatter.
2. **The sankey's nine nodes have nine distinct colours.** Today two share `#509ee3`.
3. **A column keeps its colour across cards.** `revenue` appears on several cards; its colour must
   be one value across all of them.
4. **The waterfall and the gauge are unchanged** — their colours are semantic.
5. **The colour picker's swatch equals the drawn colour** for at least one series, checked by
   opening the settings panel in the browser.

Report the measured table. Where egret and Metabase differ, say so — identical assignment is not
expected, because the hash is over egret's own column names, and Metabase's series keys carry table
and display-name decoration egret does not have. What must match is the *policy*: distinct colours,
name-stable, semantic names honoured.

- [ ] **Step 4: Open the PR**

Body must carry: the before/after measured table; the corrected claim about the summarize green,
naming the two comments that asserted it and the spec line that repeated it; the three accepted
consequences from this plan; and the divergences section. Push the branch and open the PR.

**Do not merge.** Stop and report.

---

## Self-review

- **Spec coverage.** Workstream 4's four bullets: hash of the name (Task 1), semantic names win
  (Task 1), harmony beyond eight (Task 2), remove the single-series branch (Task 3). "All option
  builders call this one function" is Tasks 3 and 4, with the out-of-scope builders and the reason
  each is out of scope listed under File structure.
- **The spec is wrong about one thing** and this plan corrects it rather than implementing it: the
  lone-series summarize green is not Metabase's behaviour. Measured, both sides, identical data.
- **Type consistency.** `colorsForKeys(keys: string[], existing?: Record<string, string> | null)`
  returns `Record<string, string>` and is called that way in every task. `defaultSeriesColor`
  changes signature in Task 5 and has no other definition.
- **Known soft spot.** Task 3's `colorOverride` parameter threading is described but not written out
  as a diff, because the surrounding function takes it through two call paths that must be read
  first. An implementer who finds the threading larger than one parameter should stop and report
  rather than restructure the series builder inside a colour PR.
