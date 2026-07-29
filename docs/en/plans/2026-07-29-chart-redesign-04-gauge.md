# Chart Redesign 04 — Gauge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a gauge's coloured ranges something the user defines, so a gauge measuring a
less-is-better metric stops telling the reader the opposite of the truth.

**Architecture:** `gauge.segments` — an array of `{min, max, color, label?}` — replaces
`gauge.min`/`gauge.max`. The scale derives from the segments. An unconfigured gauge derives today's
three equal zones at render time, so it looks unchanged. Migration 0102 converts charts that stored
an explicit min or max. The editor is a `display` control on the existing `fieldDescriptor`.

**Tech Stack:** React 18 + TypeScript, ECharts, Vitest, Postgres (sqlx).

**Spec:** `docs/en/specs/2026-07-29-chart-redesign-04-gauge.md`

---

## Facts established by reading the code — verify, don't re-derive

Checked against the tree at `2638db51` plus this branch's docs commits.

1. `echarts/options/gauge.ts` draws the zones as ECharts axisLine colour stops, where each stop is
   `[fraction, color]` and the fraction is a position along `[min, max]`:
   ```ts
   axisLine: { lineStyle: { width: 18, color: [[1 / 3, ZONE_LOW], [2 / 3, ZONE_MID], [1, ZONE_HIGH]] } },
   ```
   `ZONE_LOW/MID/HIGH` are `SERIES_PALETTE[3] / [4] / [1]` — red, yellow, green.
2. Today's range: `min = cfg.min ?? 0`, `max = cfg.max ?? (value > min ? value * 2 : min + 100)`.
3. Gauge is declared by `fieldDescriptor` in `viz/simple.tsx` with three `data` fields (metric, min,
   max). **`FieldDescriptorSpec` already supports `display?: VizControl<Cfg>[]`** and pie already
   uses it — do NOT add a new field kind to `viz/fields.tsx`.
4. `fromParams` in `fields.tsx` reads **every key under the viz prefix**, not only declared fields,
   so `gauge.segments` round-trips with no codec change.
5. The colour-input idiom is pie's per-slice picker in `settings/PieDisplaySection.tsx` — read it
   and follow it rather than inventing one.
6. Migrations live in `./migrations`; latest is `0101_line_shape_settings.sql`, so yours is `0102`.

---

### Task 1: The settings shape

**Files:**
- Modify: `frontend/src/api/charts.ts`
- Modify: `frontend/src/components/chart/echarts/options/gauge.ts`
- Modify: `frontend/src/components/chart/viz/simple.tsx`
- Test: `frontend/src/components/chart/echarts/__tests__/gauge.test.ts` (find or create)

- [ ] **Step 1: Write the failing test**

That a `GaugeConfig` carrying `segments` survives a params round-trip, and that `min`/`max` are gone
from the type. Follow the neighbouring viz tests; if there is no gauge test file, create one and say
so.

- [ ] **Step 2: Change the types**

In `frontend/src/api/charts.ts`:

```ts
export interface GaugeSegment {
  min: number;
  max: number;
  color: string;
  label?: string;
}
export interface GaugeConfig {
  metric?: string;
  /** Coloured ranges, in any order. The gauge's scale is derived from them. */
  segments?: GaugeSegment[];
}
```

**Remove `min` and `max`.** Mirror the same removal in `GaugeBuildConfig` in `gauge.ts`, importing
`GaugeSegment` rather than redeclaring it.

In `viz/simple.tsx`, drop the `min` and `max` entries from gauge's `fields` array, leaving `metric`.

- [ ] **Step 3: Keep it compiling without changing behaviour yet**

`tsc` will name the reader in `gauge.ts`. Task 3 rewrites it properly; for now make it compile while
rendering exactly as today, and say in your report what you left as a shim.

- [ ] **Step 4: Verify and commit**

```
npm test -- src/components/chart && npx tsc --noEmit && npm run lint
```

```bash
git add -A && git commit -m "feat(charts): a gauge's ranges become its model"
```

---

### Task 2: Migration 0102

**Files:**
- Create: `migrations/0102_gauge_segments.sql`

The rule: **only charts that stored an explicit `gauge.min` or `gauge.max` are converted**, into the
three thirds they are drawn at today with today's colours. Charts that stored neither get nothing
and fall through to the render-time default — writing segments for them would freeze an auto-range
that currently follows the data.

- [ ] **Step 1: Read `0101_line_shape_settings.sql` first**

It solves the same shape of problem and carries two hard-won details you must reuse: extract the
sub-object in a `LATERAL` before using `-` on it (Postgres parses `params->'settings' - 'k'` as
`params -> ('settings' - 'k')`, because `-` binds tighter than `->`), and normalise a non-object to
`'{}'::jsonb` **inside** that `LATERAL` rather than guarding with `jsonb_typeof(...) AND EXISTS(...)`
— Postgres does not promise to evaluate `AND` left to right.

- [ ] **Step 2: Write it**

Both storage locations again: `analytics.charts.params -> 'settings'` and
`analytics.dashboard_cards.options -> 'vizOverride'`. Remember from 0101 that `vizOverride` is a
**partial** override — write `gauge.segments` there only when that override itself carried a
`gauge.min` or `gauge.max`, or you will shadow what the base chart draws.

The three segments, from stored `mn = gauge.min` (default 0) and `mx = gauge.max`:

```
[mn,               mn + (mx-mn)/3]  → red
[mn + (mx-mn)/3,   mn + 2*(mx-mn)/3] → yellow
[mn + 2*(mx-mn)/3, mx]              → green
```

Use the literal hex values `SERIES_PALETTE[3]`, `[4]` and `[1]` resolve to — **read them from
`echarts/palette.ts` and put the actual strings in the SQL**, with a comment saying where they came
from. A migration cannot import a palette, and a colour that drifts later must not silently restyle
migrated charts.

A chart with `gauge.min` but no `gauge.max` has no upper bound to divide — decide what to do,
implement it, and say what you chose and why. (Consider: today such a chart renders with an
auto-range from the value, so freezing any max would change it.)

Remove `gauge.min` and `gauge.max`.

- [ ] **Step 3: Prove it on a real database**

Port-forward and use a **throwaway** database, never the dev one:

```bash
kubectl port-forward -n egret-system deploy/postgres 5433:5432
```

Cases: min+max set; max only; min only; neither; a dashboard card override with min/max; an override
without them; settings that are a string / array / JSON null; and a re-run for idempotency. Report
before/after JSON for each, then drop the scratch database and say you did.

- [ ] **Step 4: `touch crates/egret-metastore/src/lib.rs`**

A new migration file alone does not recompile that crate, so `serve` would run a stale embedded
migrator and silently skip yours. Then `cargo check -p egret-metastore`.

- [ ] **Step 5: Commit**

```bash
git add migrations/0102_gauge_segments.sql && git commit -m "feat(charts): migrate gauge min/max to coloured ranges"
```

---

### Task 3: Rendering from segments

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/gauge.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/gauge.test.ts`

- [ ] **Step 1: Write the failing tests**

1. **No segments renders exactly as today** — three stops at `1/3`, `2/3`, `1`, red→yellow→green,
   over `[0, value*2]`. Assert the actual numbers; this is the test that proves an unconfigured
   gauge is untouched.
2. A zero or negative value with no segments still produces a usable range (today's
   `min + 100` fallback).
3. Given segments, the scale is derived: `min` is the smallest segment min, `max` the largest max.
4. Given segments, the colour stops are at the right fractions — e.g. segments
   `[0,50] green`, `[50,100] red` produce stops `[[0.5, green], [1, red]]`.
5. **The less-is-better case renders**: red at the TOP of the arc, green at the bottom. This is the
   defect the unit exists to fix, so pin it explicitly.
6. Segments given out of order are sorted before becoming stops.
7. One segment fills the whole arc.
8. A segment `label` reaches the option somewhere a reader can see it — decide where (ECharts gauge
   has no per-zone label, so this likely means the tooltip or the aria description) and say what you
   chose.

- [ ] **Step 2: Run them, watch them fail**

- [ ] **Step 3: Implement**

```ts
/** Today's three zones, derived rather than stored so an unconfigured gauge is unchanged. */
function defaultSegments(value: number): GaugeSegment[] {
  const max = value > 0 ? value * 2 : 100;
  const third = max / 3;
  return [
    { min: 0, max: third, color: ZONE_LOW },
    { min: third, max: 2 * third, color: ZONE_MID },
    { min: 2 * third, max, color: ZONE_HIGH },
  ];
}
```

Then derive the range and the stops from whichever segment list applies. ECharts wants stops in
ascending fraction order, so sort by `max` first, and guard the degenerate `max === min` case rather
than dividing by zero.

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): a gauge's scale follows its ranges"
```

---

### Task 4: The segment editor

**Files:**
- Create: `frontend/src/components/chart/settings/GaugeSegmentsSection.tsx`
- Modify: `frontend/src/components/chart/viz/simple.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/GaugeSegmentsSection.test.tsx`

**Do not touch `viz/fields.tsx`.** Gauge gets a `display` control, the way pie already does.

- [ ] **Step 1: Write the failing tests**

- with no segments stored, the editor shows the three defaults it will render, not an empty list —
  otherwise the user cannot edit what they can see;
- editing a bound emits the whole `segments` array with that one value changed;
- editing a colour emits it;
- naming a range emits `label`; clearing the name emits `undefined`, not `""` (the lesson from
  chart 03's column title);
- "Add range" appends one; removing goes down to one and no further — a gauge with zero ranges has
  no arc;
- the rows render in the stored order, and reordering bounds does not reorder the rows underneath
  the user's cursor mid-edit.

That last one is a real trap: if the list is sorted on every keystroke, typing a new minimum makes
rows jump. Sort at RENDER time in the option builder (task 3 already does), not in the editor.

- [ ] **Step 2: Implement**

Follow `PieDisplaySection`'s colour-input idiom exactly. Each row is a min, a max, a colour and an
optional name. Keep it compact — this sits in a narrow panel and must survive a dashboard card's
width.

Register it on gauge's descriptor in `viz/simple.tsx`:

```tsx
    display: gaugeDisplay,
```

matching how `pie` does it.

- [ ] **Step 3: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): edit a gauge's ranges"
```

---

### Task 5: Full verify, deploy, live check, PR

- [ ] **Step 1: Both gates** — there is a migration, so the backend is in scope

```
cd frontend && npm run lint && npm run typecheck && npm test && npm run build
cargo fmt --all -- --check && cargo clippy --workspace --all-targets && cargo test --workspace
```

The DB-gated workspace test needs `DATABASE_URL`; give it a throwaway database rather than skipping
it, and confirm `_sqlx_migrations` shows `102` applied — that is also how you prove the `touch` in
task 2 worked.

- [ ] **Step 2: Deploy — backend first**

Rebuild and roll `deploy/egret`, confirm the migration ran against the live database, and only then
roll `deploy/egret-frontend`. Chart 02 established why: the frontend stops reading the old keys, so a
new frontend in front of an unmigrated database renders gauges wrong.

Open the app with a cache-busting query parameter; the browser caches `index.html`.

- [ ] **Step 3: The spec's seven live steps**

Step 1 is the one that matters most — an unconfigured gauge must be pixel-identical to `main`.
Step 4 is the point of the unit: red at the top, green at the bottom, for a less-is-better metric.

- [ ] **Step 4: Before/after screenshots at both sizes**

Deploy `main` first for a genuine "before". For step 1 the two screenshots should be
indistinguishable — say so if they are not.

- [ ] **Step 5: Open the PR — do not merge**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-redesign-gauge
gh pr create --base main --title "feat(charts): redesign 04 — gauge" --body-file <a file you write>
```

**This branch is stacked on `docs/chart-comparison-audit` (PR #357), not on `main`.** Check whether
#357 has merged. If it has, rebase onto `main` before pushing. If it has not, say so in the PR body
and target the base accordingly rather than silently including the audit's commits in a chart PR.

The body must state: why gauge jumped the queue, what a person can now do, that an unconfigured
gauge is unchanged, what the migration preserves, and anything left undone.
