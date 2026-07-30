# Chart Redesign 05 — Pie Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a pie divide by up to three dimensions as nested rings, and let a person curate its
slices — the gaps the comparison's pie row claimed did not exist.

**Architecture:** `pie.dimensions: string[]` replaces `pie.dimension`, so the ring count is the
array length. Each ring is its own ECharts pie series at a different radius, built by the existing
`buildSlices` applied per level. Outer rings inherit the parent slice's colour, lightened in OKLCH.
Slice names, order and hiding apply to the first dimension. Migration 0103 lifts the old key.

**Tech Stack:** React 18 + TypeScript, ECharts, Vitest, Postgres (sqlx).

**Spec:** `docs/en/specs/2026-07-29-chart-redesign-05-pie.md`

---

## Facts established by reading the code — verify, don't re-derive

Checked against `9a8e810b`.

1. `echarts/options/pie.ts` has `buildSlices(rows, di, mi, cfg)` doing real work: drops zeros,
   handles all-negative (abs) versus mixed-sign (drop negatives), sorts by value desc unless
   `sortRows === false`, then folds sub-threshold slices into one muted `"Other"` **only when it
   would merge 2+ slices**. All of that must keep working, per ring.
2. `dimensionUnit` is not stored — `ChartRenderer` derives it as
   `params.spec.temporal_units?.[pie.dimension ?? ""]`. With several dimensions this becomes a
   per-ring lookup and needs no new setting.
3. Pie is a `fieldDescriptor` in `viz/simple.tsx` with `fields: [dimension, metric]` and
   `buildSpec: (c,s,h) => h.dimsAndMetrics([c.dimension],[c.metric],s)`.
4. `viz/fields.tsx` `FieldSpec.kind` is `"dimension" | "metric" | "number" | "boolean"`. There is no
   multi-select, and **`display` controls cannot help** — dimensions are the query, so they belong to
   the Data section, which only `fields` renders.
5. `palette.ts` has **no** lightening helper, and `zrender` is not a direct dependency (only
   `echarts` is), so `zrender/lib/tool/color`'s `lift` is off-limits — that is reaching into a
   transitive package's internals.
6. Migrations live in `./migrations`; the latest is `0102_gauge_segments.sql`.
7. The SVG renderer does not resolve CSS `var()` — chart 04 established this. Ring colours must be
   concrete strings by the time they reach the option.

---

### Task 1: The settings shape

**Files:** `frontend/src/api/charts.ts`, `frontend/src/components/chart/echarts/options/pie.ts`,
`frontend/src/components/chart/viz/simple.tsx`, `frontend/src/components/chart/ChartRenderer.tsx`

- [ ] **Step 1: Write the failing test** — a `PieConfig` carrying `dimensions` round-trips through
  the descriptor. `fromParams` reads every key under the prefix, so no codec change is needed;
  confirm that rather than assuming.

- [ ] **Step 2: Change the types**

```ts
export interface PieConfig {
  /** Up to three, drawn as nested rings from the centre out. */
  dimensions?: string[];
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  sliceThreshold?: number;
  sortRows?: boolean;
  showTotal?: boolean;
  percentVisibility?: "off" | "inside" | "both";
  sliceColors?: Record<string, string>;
  /** Rename a slice of the FIRST dimension. */
  sliceNames?: Record<string, string>;
  /** Slices of the first dimension left out entirely. */
  hiddenSlices?: string[];
  /** Display order for the first dimension's slices. */
  sliceOrder?: string[];
  /** Decimal places on percentages. */
  decimals?: number;
}
```

`dimension` is **removed**. Mirror in `PieBuildConfig`. Update `buildSpec` to
`h.dimsAndMetrics(c.dimensions ?? [], [c.metric], s)` and `legacyFromSpec` to
`{ dimensions: spec.dimensions, metric: spec.metrics?.[0] }`.

- [ ] **Step 3: Keep it rendering** — the builder still draws ONE ring, from `dimensions[0]`. Tasks
  4-6 add the rest. Say what shim you left. `ChartRenderer`'s `dimensionUnit` lookup must use
  `dimensions[0]` for now.

- [ ] **Step 4: Verify and commit**

```
npm test -- src/components/chart && npx tsc --noEmit && npm run lint
```
```bash
git add -A && git commit -m "feat(charts): a pie can name more than one dimension"
```

---

### Task 2: A `dimensions` field kind

**Files:** `frontend/src/components/chart/viz/fields.tsx`, `viz/simple.tsx`, plus a test

`fields` gains an ordered multi-select. Keep it narrow — this is a list of column names, not a
structured record.

- [ ] **Step 1: Write the failing tests** — with `kind: "dimensions"` and `max: 3`: it renders one
  picker per chosen dimension plus an "Add" row; choosing a column appends it; removing one keeps the
  rest in order; the "Add" row disappears at `max`; a column already chosen is not offered again.

- [ ] **Step 2: Extend `FieldSpec`**

```ts
  kind: "dimension" | "metric" | "number" | "boolean" | "dimensions";
  /** `dimensions` only: how many may be picked. */
  max?: number;
```

and handle it in `FieldControl` alongside the existing kinds, reusing the same option list the
single `dimension` kind builds.

Declare it on pie:

```tsx
      { id: "dimensions", label: "Dimensions", kind: "dimensions", max: 3, required: "a dimension", temporal: true },
```

Check what `temporal: true` does for a multi-value field — the shared date-grouping row is rendered
per dimension today. Decide whether it renders per picked dimension or only for the first, implement
it, and say which and why.

- [ ] **Step 3: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): declare an ordered list of dimensions"
```

---

### Task 3: Migration 0103

**Files:** `migrations/0103_pie_dimensions.sql`

`pie.dimension` (a string) becomes `pie.dimensions` (an array of that one string); the old key goes.
Both storage locations, with `vizOverride` written only where it carried the old key.

**Read `0102_gauge_segments.sql` first.** It carries three things you must reuse, two of them found
by review rather than written first time:

1. extract the sub-object in a `LATERAL` before using `-` (because `-` binds tighter than `->`);
2. normalise a non-object to `'{}'::jsonb` inside that `LATERAL`, not in a `WHERE` guard;
3. **gate on `jsonb_typeof(...) = 'string'`, not on key existence** — a key present with JSON `null`
   is not a value, and 0102's first draft silently wrote garbage because of exactly this.

There is no cast here, so 0102's other lesson (an unguarded `::numeric` aborting the whole statement)
does not apply — but say in your report that you checked whether it does.

- [ ] **Prove it on a throwaway database.** Cases: a string dimension; a `null` dimension; no
  dimension key; a non-string dimension (number); a card override with the key; a card override
  without it; settings that are a string / array / JSON null; and a second run for idempotency.
  Report before/after JSON for each, then drop the scratch database and say you did.

- [ ] `touch crates/egret-metastore/src/lib.rs` and `cargo check -p egret-metastore`, or the embedded
  migrator will not see the file.

```bash
git add migrations/0103_pie_dimensions.sql && git commit -m "feat(charts): migrate a pie's single dimension to a list"
```

---

### Task 4: Nested rings

**Files:** `frontend/src/components/chart/echarts/options/pie.ts` + tests

One ECharts pie series per dimension, at increasing radii. The inner ring aggregates by
`dimensions[0]`; ring *n* aggregates by `dimensions[0..n]` and its slices must sit inside their
parent.

- [ ] **Step 1: Write the failing tests**

1. One dimension renders exactly one series, identical to today — assert the slice names and values.
2. Two dimensions render two series at different radii.
3. The outer ring's slices are ordered so each parent's children are contiguous and follow the
   parent's own order.
4. `buildSlices`' existing behaviour survives per ring: zeros dropped, all-negative absolutised,
   mixed-sign negatives dropped, sort honoured.
5. The `"Other"` fold applies **per ring**, and still only when it would merge 2+ slices.
6. A row whose outer value is null does not break its parent's slice.
7. `donut` and `showTotal` still work with one ring, and the centre stays clear with several.

- [ ] **Step 2: Implement** — aggregate per level, then map each level to a series with its own
  `radius: [inner, outer]`. Keep `buildSlices` as the per-level function rather than duplicating its
  rules; if its signature has to change, change it once and say so.

- [ ] **Step 3: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): a pie divides by more than one dimension"
```

---

### Task 5: Ring colours

**Files:** `frontend/src/components/chart/echarts/palette.ts` + test, `options/pie.ts`

A three-ring pie with unrelated colours per slice is unreadable. An outer slice inherits its
parent's colour, lightened per ring, so the family reads and depth reads.

`palette.ts` has no lightening helper and `zrender` is not a direct dependency, so write one. Do it
in **OKLCH** — adjust L, keep C and H — because that is the space this codebase's tokens already use
and the one where equal L steps look equal.

- [ ] **Step 1: Write the failing tests** for the helper: lightening a hex raises its perceived
  lightness without shifting hue; lightening by 0 returns the input; the result is a valid concrete
  colour string (the SVG renderer will not resolve `var()`); an unparseable input returns the input
  unchanged rather than throwing.

- [ ] **Step 2: Implement** `lightenOklch(hex, amount)` — sRGB → linear → OKLab → raise L → back.
  Roughly 35 lines of colour maths; keep it in one function with the formulae cited in a comment so
  a reader can check them.

- [ ] **Step 3: Wire it** — ring *n* uses its parent slice's colour lightened by *n* steps. A
  per-slice `sliceColors` override still wins. Test that an overridden parent's children lighten
  from the override, not from the palette.

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): outer rings lighten their parent's colour"
```

---

### Task 6: Slice curation and decimals

**Files:** `options/pie.ts`, `settings/PieDisplaySection.tsx` + tests

Rename, hide and reorder the **first dimension's** slices, and set percent decimals. Say in the panel
that these apply to the first dimension.

- [ ] **Step 1: Write the failing tests** — a renamed slice shows its name in the chart and the
  legend; a hidden slice is absent **and the remaining percentages re-base** (assert a percentage,
  not just absence); `sliceOrder` reorders and unlisted slices keep their natural place after the
  listed ones; `decimals` changes the label; hiding a parent hides its children.

That re-basing case is the one that matters — a hidden slice that still counts toward the total is a
chart that lies about proportions.

- [ ] **Step 2: Implement**, following how chart 01 did `seriesOrder`/`hiddenSeries` for cartesian —
  same shape of problem, and the panel row idiom is already there for per-slice colours.

- [ ] **Step 3: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): rename, hide and order a pie's slices"
```

---

### Task 7: Full verify, deploy, live check, PR

- [ ] **Both gates** — there is a migration, so the backend is in scope:

```
cd frontend && npm run lint && npm run typecheck && npm test && npm run build
cargo fmt --all -- --check && cargo clippy --workspace --all-targets && cargo test --workspace
```

Give the DB-gated workspace test a throwaway database rather than skipping it, and confirm
`_sqlx_migrations` shows `103`.

- [ ] **The migration needs a fixture that may not exist.** Check whether the kind database holds a
  pie with `pie.dimension`. Gauge's did not, and a migration verified against an empty table is not
  verified. If there is none, save one on `main` first, record how it looks, then deploy.

- [ ] **Deploy backend first**, confirm the migration ran, then the frontend. Open the app with a
  cache-busting query parameter.

- [ ] **The spec's eight live steps.** Step 1 is the guarantee everything rests on: a one-dimension
  pie unchanged. Step 3 — the outer ring recognisably a lighter member of the parent's family — is a
  judgement only a human eye can make, so look at it rather than asserting it.

- [ ] **Before/after screenshots at both sizes**, deploying `main` first for a genuine before.

- [ ] **Open the PR — do not merge.** State: that the pie row of the comparison was wrong and this is
  the third such row; what a person can now do; that a one-dimension pie is unchanged; what the
  migration preserves; and anything left undone.
