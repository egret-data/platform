# Chart Redesign 02 — Line Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give line charts an honest set of shape, missing-value and marker settings — each a real multi-state choice rather than a boolean — with per-series overrides, and migrate existing charts without changing how any of them look.

**Architecture:** Three chart-wide settings (`lineShape`, `missing`, `markers`) replace four booleans in the shared `cartesian.*` namespace, plus a `seriesLine` map of per-series overrides following the existing `seriesColors`/`seriesLabels` pattern. Migration 0101 rewrites both places settings are stored. Rendering stays in `decorateSeries`; only `zero` needs a data transform, because ECharts has no null-to-zero mode.

**Tech Stack:** React 18 + TypeScript, ECharts (SVG renderer), Vitest, Postgres (sqlx migrations).

**Spec:** `docs/en/specs/2026-07-28-chart-redesign-02-line.md`

---

## Facts established by reading the code — do not re-derive, and do not contradict

These were checked against the tree at `1fe27573`. If a task's snippet disagrees with what you
find, STOP and report rather than forcing it.

1. `decorateSeries` in `frontend/src/components/chart/echarts/options/cartesian.ts` already sets,
   for every line series:
   ```ts
   out.lineStyle = { width: CHART_THEME.lineWidth, type: lineType(colorIdx) };
   out.symbol = symbolShape(colorIdx);
   out.showSymbol = cfg.showMarkers !== false;
   out.connectNulls = cfg.connectNulls ?? true;
   if (cfg.step) out.step = "middle";
   out.smooth = !!cfg.smooth;
   ```
   **`lineType(colorIdx)` already cycles solid/dashed/dotted per series on purpose** — it is an
   accessibility feature so colour-blind users can tell lines apart. A per-series `style` is
   therefore an *override of an existing automatic cycle*, not a new capability. Same for
   `symbolShape(colorIdx)`.
2. egret's step mode is `"middle"`. Metabase's is `step-after`. **Keep `"middle"`** — changing it
   would restyle every existing stepped chart, which this unit exists to avoid.
3. Migrations live in `./migrations` at the repo root; the latest is `0100_dashboard_viz_override_v2.sql`.
4. Settings live in **two** places: `analytics.charts.params -> 'settings'` and
   `analytics.dashboard_cards.options -> 'vizOverride'`. Both are flat objects keyed
   `cartesian.<field>`. Migration 0101 must handle both.
5. `CartesianConfig` is in `frontend/src/api/charts.ts`; `KEYS` in
   `frontend/src/components/chart/viz/cartesian-codec.ts` is
   `as const satisfies readonly (keyof CartesianConfig)[]`, so type and KEYS must change together.
6. `ChartRenderer` now spreads the whole cartesian config into the builder, so no field-forwarding
   list needs touching. Do not reintroduce one.

---

## File structure

- `migrations/0101_line_shape_settings.sql` — new; rewrites both settings locations.
- `frontend/src/api/charts.ts` — `CartesianConfig`: add three fields + `seriesLine`, remove four.
- `frontend/src/components/chart/viz/cartesian-codec.ts` — `KEYS` follows.
- `frontend/src/components/chart/echarts/options/cartesian.ts` — `CartesianBuildConfig` follows;
  `decorateSeries` reads the new settings; a new `applyMissing` helper for the `zero` transform.
- `frontend/src/components/chart/settings/CartesianDisplaySection.tsx` — chart-wide controls in
  `SeriesOptions`; per-series overrides in the existing per-series rows.
- `frontend/src/components/chart/echarts/__tests__/cartesian.line.test.ts` — new.
- `frontend/src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx` — extended.

---

### Task 1: The settings shape

**Files:**
- Modify: `frontend/src/api/charts.ts`
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Modify: `frontend/src/components/chart/viz/cartesian-codec.ts`
- Test: `frontend/src/components/chart/viz/__tests__/cartesian-codec.test.ts`

- [ ] **Step 1: Write the failing test**

Add to the codec test file, in its existing style:

```ts
  it("round-trips the line settings", () => {
    const cfg = {
      yMetrics: ["rev"],
      lineShape: "step" as const,
      missing: "zero" as const,
      markers: "off" as const,
      seriesLine: { rev: { style: "dashed" as const, size: "L" as const } },
    };
    expect(cartesianFromSettings(cartesianToSettings(cfg))).toEqual(cfg);
  });

  it("no longer carries the booleans the line settings replaced", () => {
    const settings = cartesianToSettings({ yMetrics: [] } as never);
    for (const dead of ["cartesian.smooth", "cartesian.step", "cartesian.connectNulls", "cartesian.showMarkers"]) {
      expect(Object.keys(settings)).not.toContain(dead);
    }
  });
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd frontend && npm test -- src/components/chart/viz/__tests__/cartesian-codec.test.ts`
Expected: FAIL — the new fields are not in `KEYS`, so they are dropped by the round-trip.

- [ ] **Step 3: Change the types**

In `frontend/src/api/charts.ts`, in `CartesianConfig`, **remove** `smooth`, `step`, `connectNulls`
and `showMarkers`, and add:

```ts
  /** Line shape. One choice, because a line cannot be both smoothed and stepped. */
  lineShape?: "linear" | "smooth" | "step";
  /** What a null becomes: bridged, dropped to zero, or left as a break in the line. */
  missing?: "interpolate" | "zero" | "none";
  /** Point markers. "auto" shows them only when there are few enough points to read. */
  markers?: "auto" | "on" | "off";
  /** Per-series overrides of the chart-wide line appearance, keyed by series name. */
  seriesLine?: Record<string, LineSeriesStyle>;
```

and add the shared type next to it:

```ts
export interface LineSeriesStyle {
  shape?: "linear" | "smooth" | "step";
  style?: "solid" | "dashed" | "dotted";
  size?: "S" | "M" | "L";
  markers?: "auto" | "on" | "off";
}
```

Mirror all of it in `CartesianBuildConfig` in `echarts/options/cartesian.ts` (importing
`LineSeriesStyle` rather than redeclaring it), removing the same four fields there.

In `cartesian-codec.ts`, drop the four dead names from `KEYS` and add
`"lineShape", "missing", "markers", "seriesLine"`.

- [ ] **Step 4: Fix the fallout, and report it**

Removing four fields will break every existing reader. `npx tsc --noEmit` will name them. Do NOT
fix them by re-adding the fields. The two you should expect are `decorateSeries` (Task 2 rewrites
it — for now make it compile by reading the new fields with the old behaviour) and
`CartesianDisplaySection` (Task 6 rewrites it — for now delete the four toggles rather than
rewiring them, and say so in your report so the next task knows they are gone).

- [ ] **Step 5: Run the tests, then commit**

Run: `cd frontend && npm test -- src/components/chart && npx tsc --noEmit && npm run lint`

```bash
git add -A && git commit -m "feat(charts): line shape, missing and marker settings replace four booleans"
```

---

### Task 2: Migration 0101

**Files:**
- Create: `migrations/0101_line_shape_settings.sql`

The rule, stated once so the SQL can be checked against it: **for every cartesian chart and every
dashboard card override, write all three new keys computed so the chart looks exactly as it does
today, then remove the four old keys.**

- `lineShape`: `step` wins over `smooth` (both can be true today and ECharts draws a step), else
  `smooth`, else `linear`.
- `missing`: `connectNulls = false` → `none`, otherwise `interpolate` (the builder's `?? true`).
- `markers`: `showMarkers = false` → `off`, **otherwise `on`** — never `auto`. Today's builder
  shows markers unless explicitly disabled, so `auto` would restyle existing charts. New charts
  get `auto` from the builder default; existing ones get the explicit value they already render.

- [ ] **Step 1: Write the migration**

```sql
-- Line shape, missing-value and marker settings replace four booleans that could not express
-- what they described: `smooth` and `step` could both be true, and a two-state `connectNulls`
-- hid a render-time default that made the panel contradict the chart. See
-- docs/en/specs/2026-07-28-chart-redesign-02-line.md.
--
-- Every value written here is chosen to preserve how each chart already looks. In particular
-- `markers` becomes 'on' rather than 'auto' whenever it was not explicitly off: the builder
-- shows markers unless told otherwise, so 'auto' would restyle existing charts. New charts get
-- 'auto' from the builder default instead.

CREATE OR REPLACE FUNCTION pg_temp.line_settings_v2(s jsonb) RETURNS jsonb AS $$
  SELECT (s - 'cartesian.smooth' - 'cartesian.step'
            - 'cartesian.connectNulls' - 'cartesian.showMarkers')
    || jsonb_build_object(
         'cartesian.lineShape',
           CASE WHEN s->>'cartesian.step' = 'true' THEN 'step'
                WHEN s->>'cartesian.smooth' = 'true' THEN 'smooth'
                ELSE 'linear' END,
         'cartesian.missing',
           CASE WHEN s->>'cartesian.connectNulls' = 'false' THEN 'none'
                ELSE 'interpolate' END,
         'cartesian.markers',
           CASE WHEN s->>'cartesian.showMarkers' = 'false' THEN 'off'
                ELSE 'on' END
       )
$$ LANGUAGE sql IMMUTABLE;

UPDATE analytics.charts
SET params = jsonb_set(params, '{settings}', pg_temp.line_settings_v2(params->'settings'))
WHERE jsonb_typeof(params->'settings') = 'object'
  AND EXISTS (
    SELECT 1 FROM jsonb_object_keys(params->'settings') AS k
    WHERE k LIKE 'cartesian.%'
  );

UPDATE analytics.dashboard_cards
SET options = jsonb_set(options, '{vizOverride}', pg_temp.line_settings_v2(options->'vizOverride'))
WHERE jsonb_typeof(options->'vizOverride') = 'object'
  AND EXISTS (
    SELECT 1 FROM jsonb_object_keys(options->'vizOverride') AS k
    WHERE k LIKE 'cartesian.%'
  );
```

- [ ] **Step 2: Check it against a real database, do not assume**

`pg_temp` functions and `jsonb_object_keys` in a `WHERE EXISTS` are both places this can fail to
parse. Port-forward the kind Postgres into a throwaway database and run the file:

```bash
kubectl port-forward -n egret-system deploy/postgres 5433:5432
```

Then apply it against a scratch database with a couple of hand-inserted rows covering: a chart with
`step` and `smooth` both true; a chart with `connectNulls: false`; a chart with no marker key; a
dashboard card override; and a chart with no `cartesian.` keys at all (must be left alone). Report
the before/after JSON for each. If `pg_temp` proves awkward inside a migration, inline the
expression twice instead — correctness first, DRY second.

- [ ] **Step 3: Force the embedded migrator to notice the new file**

`touch crates/egret-metastore/src/lib.rs` — a new migration file alone does not recompile the
crate, so `serve` would run a stale embedded migrator.

- [ ] **Step 4: Commit**

```bash
git add migrations/0101_line_shape_settings.sql && git commit -m "feat(charts): migrate line booleans to shape, missing and marker settings"
```

---

### Task 3: Rendering the line shape

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.line.test.ts` (create)

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [{ name: "d", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["a", 1], ["b", 2], ["c", 3]];
const opt = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, rows, { kind: "line", xAxis: "d", yMetrics: ["rev"], ...cfg } as never) as never as {
    series: Record<string, unknown>[];
  };

describe("line shape", () => {
  it("draws a straight line by default", () => {
    const s = opt({}).series[0];
    expect(s.smooth).toBe(false);
    expect(s.step).toBeUndefined();
  });

  it("smooths when asked", () => {
    expect(opt({ lineShape: "smooth" }).series[0].smooth).toBe(true);
  });

  it("steps when asked, and does not also smooth", () => {
    const s = opt({ lineShape: "step" }).series[0];
    expect(s.step).toBe("middle");
    expect(s.smooth).toBe(false);
  });

  it("lets a series override the chart's shape", () => {
    const s = opt({ lineShape: "linear", seriesLine: { rev: { shape: "smooth" } } }).series[0];
    expect(s.smooth).toBe(true);
  });
});
```

- [ ] **Step 2: Run it, watch it fail**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/cartesian.line.test.ts`

- [ ] **Step 3: Implement**

In `decorateSeries`, replace the `out.smooth` / `out.step` lines. Resolve the per-series override
first, so every later line-appearance decision reads from one place:

```ts
  if (type === "line") {
    // A series may override any part of the chart-wide line appearance (Metabase keeps these
    // per-series; egret keeps a chart-wide default and lets a series depart from it).
    const ln = cfg.seriesLine?.[s.name] ?? {};
    const shape = ln.shape ?? cfg.lineShape ?? "linear";
    out.smooth = shape === "smooth";
    if (shape === "step") out.step = "middle";   // egret's existing step mode; not Metabase's step-after
    ...
```

Keep the rest of the block as it is for now — Tasks 4, 5 and 6 replace the remaining lines.

- [ ] **Step 4: Verify and commit**

Run: `cd frontend && npm test -- src/components/chart && npx tsc --noEmit`

```bash
git add -A && git commit -m "feat(charts): one line shape setting, overridable per series"
```

---

### Task 4: Missing values, including the zero transform

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.line.test.ts`

ECharts has `connectNulls` (bridge or break) but **no null-to-zero mode**, so `zero` is a data
transform, applied to the series data before the option is assembled.

- [ ] **Step 1: Write the failing test**

```ts
const gappy = [["a", 1], ["b", null], ["c", 3]];
const optG = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, gappy, { kind: "line", xAxis: "d", yMetrics: ["rev"], ...cfg } as never) as never as {
    series: { data: unknown[]; connectNulls?: boolean }[];
  };

describe("missing values", () => {
  it("bridges the gap by default, as it always has", () => {
    const s = optG({}).series[0];
    expect(s.connectNulls).toBe(true);
    expect(s.data).toEqual([1, null, 3]);
  });

  it("leaves a break when asked", () => {
    expect(optG({ missing: "none" }).series[0].connectNulls).toBe(false);
  });

  it("drops to zero when asked — ECharts has no such mode, so the data changes", () => {
    const s = optG({ missing: "zero" }).series[0];
    expect(s.data).toEqual([1, 0, 3]);
  });

  it("only rewrites nulls for line-ish kinds, never for a bar", () => {
    const s = buildCartesianOption(columns, gappy, {
      kind: "bar", xAxis: "d", yMetrics: ["rev"], missing: "zero",
    } as never) as never as { series: { data: unknown[] }[] };
    expect(s.series[0].data).toEqual([1, null, 3]);
  });
});
```

The last case matters: a missing bar and a zero-height bar say different things, and this setting
belongs to line.

- [ ] **Step 2: Run it, watch it fail**

- [ ] **Step 3: Implement**

In `decorateSeries`'s line branch:

```ts
    // Chart-wide only: `LineSeriesStyle` deliberately has no `missing`. How a gap is read is a
    // statement about the data, and two series in one chart disagreeing about it would make the
    // chart say two things at once.
    const missing = cfg.missing ?? "interpolate";
    out.connectNulls = missing === "interpolate";
    if (missing === "zero") {
      // ECharts can bridge a null or break at it, but it cannot treat one as zero — that has
      // to be the data, not an option.
      out.data = (s.data ?? []).map((v) => (v == null ? 0 : v));
    }
```

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): interpolate, zero or break for missing line values"
```

---

### Task 5: Markers, with an auto mode

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.line.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
const many = Array.from({ length: 60 }, (_, i) => [`d${i}`, i]);

describe("markers", () => {
  it("shows them on a short series under auto", () => {
    expect(opt({}).series[0].showSymbol).toBe(true);
  });

  it("hides them on a long series under auto — they would read as a thick line", () => {
    const s = buildCartesianOption(columns, many, {
      kind: "line", xAxis: "d", yMetrics: ["rev"],
    } as never) as never as { series: { showSymbol: boolean }[] };
    expect(s.series[0].showSymbol).toBe(false);
  });

  it("obeys an explicit choice in both directions", () => {
    expect(opt({ markers: "off" }).series[0].showSymbol).toBe(false);
    const s = buildCartesianOption(columns, many, {
      kind: "line", xAxis: "d", yMetrics: ["rev"], markers: "on",
    } as never) as never as { series: { showSymbol: boolean }[] };
    expect(s.series[0].showSymbol).toBe(true);
  });

  it("lets a series override the chart", () => {
    expect(opt({ markers: "off", seriesLine: { rev: { markers: "on" } } }).series[0].showSymbol).toBe(true);
  });
});
```

- [ ] **Step 2: Run it, watch it fail**

- [ ] **Step 3: Implement**

```ts
    // Markers stop being points and start being a thick line once they touch; ~30 is where a
    // full-width chart crosses that. PROVISIONAL — tuned against a real chart in Task 8.
    const MARKER_AUTO_MAX = 30;
    const markers = ln.markers ?? cfg.markers ?? "auto";
    out.showSymbol = markers === "on" || (markers === "auto" && (s.data?.length ?? 0) <= MARKER_AUTO_MAX);
```

Put `MARKER_AUTO_MAX` at module scope with the other constants rather than inside the function.

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): auto, on or off markers on a line"
```

---

### Task 6: Per-series line style and size

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.line.test.ts`

**Read fact 1 at the top of this plan before starting.** `lineType(colorIdx)` already cycles
solid/dashed/dotted per series as an accessibility feature. A per-series `style` overrides that
cycle for one series; it does not introduce dashing.

- [ ] **Step 1: Write the failing test**

```ts
describe("per-series line appearance", () => {
  it("keeps the automatic dash cycle when nothing overrides it", () => {
    // The cycle exists so colour-blind users can tell lines apart; do not remove it.
    expect(opt({}).series[0].lineStyle).toMatchObject({ type: expect.anything() });
  });

  it("lets a series choose its own dash style", () => {
    const s = opt({ seriesLine: { rev: { style: "dotted" } } }).series[0];
    expect((s.lineStyle as { type: string }).type).toBe("dotted");
  });

  it("maps size to a width, leaving others at the theme width", () => {
    const thick = (opt({ seriesLine: { rev: { size: "L" } } }).series[0].lineStyle as { width: number }).width;
    const normal = (opt({}).series[0].lineStyle as { width: number }).width;
    expect(thick).toBeGreaterThan(normal);
  });
});
```

- [ ] **Step 2: Run it, watch it fail**

- [ ] **Step 3: Implement**

```ts
    // The dash cycle is an a11y feature (tell lines apart without colour); a per-series style
    // overrides it for that series only.
    out.lineStyle = {
      width: ln.size ? LINE_WIDTHS[ln.size] : CHART_THEME.lineWidth,
      type: ln.style ?? lineType(colorIdx),
    };
```

with, at module scope:

```ts
/** Metabase's S/M/L line sizes, relative to the theme's default stroke. */
const LINE_WIDTHS = { S: 1, M: 2, L: 4 } as const;
```

Check `CHART_THEME.lineWidth`'s actual value first and pick the three so that M is the current
width — report what it is and what you chose.

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): per-series line style and size"
```

---

### Task 7: The panel

**Files:**
- Modify: `frontend/src/components/chart/settings/CartesianDisplaySection.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx`

Task 1 deleted the four old toggles. This puts back one control per setting, in the right shape.

- [ ] **Step 1: Write the failing tests**

Test, in the file's existing idiom:
- Line shape renders three options and emits `lineShape`.
- Missing values renders three and emits `missing`.
- Markers renders Auto/On/Off and emits `markers`, with Auto emitting `undefined`.
- All three are hidden for `kind: "bar"` and shown for `kind: "line"`.
- A per-series row offers style and size, and emits into `seriesLine` **without discarding the
  other series' entries** — this is the merge bug that per-series maps invite; pin it.

- [ ] **Step 2: Run them, watch them fail**

- [ ] **Step 3: Implement**

Chart-wide controls go in `SeriesOptions`, gated by the existing `isLineish` (which is
`kind === "line" || kind === "area"`). Reuse the `AutoToggle` helper already in the file for
markers — it is exactly the Auto/On/Off shape — and follow the "X axis scale" `Select` idiom for
line shape and missing values.

The per-series overrides go in the existing per-series rows next to colour and label. Merge, do
not replace:

```tsx
onChange({ seriesLine: { ...config.seriesLine, [name]: { ...config.seriesLine?.[name], style: v } } })
```

- [ ] **Step 4: Verify and commit**

Run: `cd frontend && npm test -- src/components/chart && npx tsc --noEmit && npm run lint`

```bash
git add -A && git commit -m "feat(charts): line shape, missing and marker controls"
```

---

### Task 8: Full verify, deploy, live check, PR

**Files:** none (verification only)

- [ ] **Step 1: Full frontend gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`

- [ ] **Step 2: Backend gate** — this unit adds a migration, so unlike chart 01 the Rust side is in scope

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets && cargo test --workspace`

Do not run two `cargo test --workspace` at once. If a test fails, check it fails on `main` too
before treating it as a regression.

- [ ] **Step 3: Deploy to kind**

```bash
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

The backend also changed (new migration), so rebuild and roll `deploy/egret` too, and confirm the
migration actually ran — check the chart settings in the database afterwards, do not assume.

**Roll the backend BEFORE the frontend.** Task 1 removed the four legacy keys from the codec, so a
chart still storing `cartesian.smooth` reads back as `lineShape: undefined` and renders unsmoothed.
Migration 0101 is what closes that gap. In a squash-merged release the two are one commit and the
question never arises, but this manual two-deployment sequence can put the new frontend in front of
an unmigrated database — and the symptom (every line chart quietly loses its shape) looks exactly
like a rendering bug in task 3. Roll `deploy/egret`, confirm the migration ran, then roll the
frontend.

Read the docker build output before trusting the rollout: a failed build leaves the old image
loadable and `rollout status` still reports success.

**After redeploying, load the app with a cache-busting query parameter.** The browser caches
`index.html`, and chart 01 wasted a cycle screenshotting a stale bundle that looked identical.

- [ ] **Step 4: Live check — the spec's six steps**

1. `order_date` (day) × `total_revenue`: line shape linear / smooth / step each visibly different,
   only one active.
2. A series with a real gap: all three missing modes differ.
3. Markers on auto: present on a short series, gone on a long one. **Tune `MARKER_AUTO_MAX` here**
   against what actually looks right, and say what you changed it to.
4. Breakout by `category`; set one series dashed and another to size L; only those change.
5. Open **Revenue over time (line)**, saved before this change — it must look exactly as it did.
   This is the migration's real test.
6. The same chart at a dashboard card's width.

Capture before/after screenshots at both sizes. To get a genuine "before", deploy `main` first,
screenshot, then redeploy the branch — chart 01 did this and it was worth the two extra builds.

- [ ] **Step 5: Open the PR — do not merge**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-redesign-line
gh pr create --base main --title "feat(charts): redesign 02 — line" --body-file <a file you write>
```

State in the body: what a person sees, the three defects fixed, the migration's appearance-
preserving choices, what the live check found, and anything left undone. Then stop and wait for
approval.
