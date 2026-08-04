# Per-series settings drill-in — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the two per-series line settings egret already renders reachable, and give them somewhere
to live that stays usable at ten series.

**Architecture:** One button per series row opens a popover holding that series' appearance settings —
Metabase's own shape, measured against the live instance. `SeriesLineStyleSection` stops being an
N-row grid and becomes the popover's body for one series.

**Tech stack:** React 18, shadcn/Radix Popover, ECharts 6.1, vitest.

---

## What was measured

A prior investigation drove Metabase at `localhost:3033`. The findings this plan rests on:

- Metabase's Display tab has **no chart-wide line-appearance controls at all**. Shape, style, size,
  dots and missing-value handling exist **only** in a per-series drill-in. The three chart-wide keys
  that remain (`line.interpolate`, `line.marker_enabled`, `line.missing`) are marked
  `// DEPRECATED: moved to series settings` in `lib/settings/graph.ts:297-308` and have no widget.
- Its series list row (`ChartSettingSeriesOrder.tsx`) carries **identity only**: drag grabber, colour
  swatch, name, an ellipsis button, a hide button. The ellipsis opens a right-anchored popover
  (`ChartSettingsWidgetPopover.tsx`, `miw 336px`, `mah 40rem`, `overflow-y auto`).
- The popover **self-prunes**: a series whose resolved display is `bar` shows no line rows at all.
  Measured on card 12 by reading the `hidden` attribute.
- The list truncates at ten rows (`graph.ts:186` `truncateAfter: 10`) behind an "N more series" button.

**egret is a superset chart-wide and a subset per-series.** Keeping the chart-wide defaults is a
deliberate divergence worth keeping — setting shape once for a ten-band chart is one click here and ten
in Metabase — and `cartesian.ts:1035/1041` already resolves per-series over chart-wide.

## The defect this closes

`api/charts.ts:62-67` declares four per-series fields. `cartesian.ts:1030-1075` honours all four.
`settings/SeriesLineStyleSection.tsx` offers **two**. `shape` and `markers` are implemented and
unreachable — the same shape of defect `FormatSection.tsx:152-154` names for `scale` and
`number_separators`, and the one that "cost the pie four settings".

The omission is currently *argued* rather than accidental, and the argument is sound as far as it goes:
a third and fourth Select per row makes an unbounded list worse. This plan removes the premise instead
of overriding the conclusion.

## Cost

**No new stored key.** `shape` and `markers` are already inside `seriesLine`, which is already in the
codec's `KEYS` and already round-trips (`cartesian-codec.test.ts:99-108`). No codec edit, no backend
edit, no migration, no compatibility break — an older build already renders a chart that carries them.

---

## Task 1: expose the two unreachable fields

**Files:**
- Modify: `frontend/src/components/chart/settings/SeriesLineStyleSection.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/SeriesLineStyleSection.test.tsx`

- [ ] **Step 1: Write the failing tests.** Two, one per field. Assert the control exists and that
  choosing a value emits the right patch — `onChange({ seriesLine: { <name>: { shape: "step" } } })`
  merged over the series' existing entry, not replacing it.

- [ ] **Step 2: Run them and watch them fail** for the right reason (no such control). Record the text.

- [ ] **Step 3: Widen the `field` union** at `SeriesLineStyleSection.tsx:22` from `"style" | "size"` to
  include `"shape"` and `"markers"`, and render the two Selects. Options and defaults must match what
  the builder resolves: shape `linear | smooth | step` (`cartesian.ts:1035`), markers
  `auto | on | off` (`cartesian.ts:1041`). "Auto" writes `undefined` so the chart-wide default is what
  applies — the existing handler already does this for `style`/`size`; follow it exactly.

  **Do not invent a `step-after` option.** `cartesian.ts:1036` maps `step` to ECharts' `"middle"` and
  the comment states this is egret's existing step mode, not Metabase's. Changing it is out of scope.

- [ ] **Step 4: Run the tests.** Expected: pass.

- [ ] **Step 5: Commit.**

```bash
git add frontend/src/components/chart/settings
git commit -m "fix(charts): two per-series line settings the chart already draws"
```

---

## Task 2: the drill-in

**Files:**
- Modify: `frontend/src/components/chart/settings/SeriesListSection.tsx`
- Create: `frontend/src/components/chart/settings/SeriesSettingsPopover.tsx`
- Modify: `frontend/src/components/chart/settings/SeriesLineStyleSection.tsx` (N series → one series)
- Modify: `frontend/src/components/chart/viz/cartesian.tsx` (drop the `seriesLineStyle` registration)

- [ ] **Step 1: Write the failing tests** in `SeriesListSection.test.tsx`: a settings button per row
  with `aria-label={`settings ${name}`}`; clicking it reveals that series' shape/style/size/markers
  controls; a patch emitted from inside the popover carries the right series name. Watch them fail.

- [ ] **Step 2: Add the button** to the row at `SeriesListSection.tsx:84`, between the rename input and
  the Show switch. Keep every existing row element — reorder arrows, swatch, display type, rename,
  visibility. That is already Metabase's row anatomy plus the display Select it keeps in the popover.

- [ ] **Step 3: Build `SeriesSettingsPopover`** on the shadcn Popover already in the codebase. It
  renders the per-series stack for ONE series. Header: the colour swatch and the series name, as
  Metabase's `ChartNestedSettingSeriesSingle` does.

- [ ] **Step 4: Prune by resolved display type.** A series whose resolved display is `bar` gets no line
  rows — measured Metabase behaviour, and it is what keeps the popover small. The resolver already
  exists: `seriesDisplayFor(cfg.kind, name, colorIdx, cfg.seriesTypes)` at `cartesian.ts:939`. Use it
  rather than re-deriving the rule; if it is not importable from a settings component, say so and
  propose where it should live rather than copying it.

- [ ] **Step 5: Repurpose `SeriesLineStyleSection`** to render one series' controls, and delete the
  `seriesLineStyle` control registration at `viz/cartesian.tsx:206-209`. The "More options" disclosure
  for it is no longer needed: one row per series with one button is bounded by construction.

- [ ] **Step 6: Run the full gates.**

```bash
cd frontend && npx tsc --noEmit && npm run lint && npm run lint:harness && npx vitest run
```

  **Two tests will fail by design and that is the signal the change landed:**
  `viz/__tests__/cartesian.controls.test.tsx` asserts `advanced === true` on `seriesLineStyle`, and
  `StudioPanel.cartesianGroups.test.tsx:72-104` asserts the controls are absent until "More options" is
  clicked. Update both and say in the PR why each moved. **Any OTHER test that moves must be
  understood and explained** — this programme has shipped a passing suite over a dead branch twice.

- [ ] **Step 7: Commit.**

---

## Task 3: truncate the list

**Files:** `frontend/src/components/chart/settings/SeriesListSection.tsx`

- [ ] **Step 1: Write the failing test:** eleven series render ten rows plus an affordance; activating
  it reveals the rest.

- [ ] **Step 2: Implement**, matching `graph.ts:186` (`truncateAfter: 10`).

  This is the piece that actually answers "unreadable at ten series", and it belongs here whether or
  not the popover lands.

- [ ] **Step 3: Gates, commit.**

---

## Task 4: restate the `missing` divergence

**Files:** `frontend/src/components/chart/echarts/options/cartesian.ts:1042-1045`

The comment argues `LineSeriesStyle` has no `missing` because "two series disagreeing would make the
chart say two things at once". **The conclusion stands; the implied premise about Metabase is false.**
Measured: setting one band of a four-band chart to Zero while the others interpolate is stored and
drawn by Metabase with no warning, no badge, and nothing in the legend or tooltip to say so.

- [ ] **Step 1: Rewrite the comment** to state the divergence as a decision on its own merits — that
  Metabase makes this per-series, that egret deliberately does not, and why. Do not imply Metabase
  forbids it.

- [ ] **Step 2: Commit.**

---

## Task 5: live check and the PR

- [ ] **Step 1: Gates**, then bring the app up. Login `admin@egret.local` / `Admin12345!` at `/login`
  → "Admin sign-in (break-glass)", rate-limited 5 per 5 minutes, so log in ONCE.

- [ ] **Step 2: Drive it.** On a multi-series line chart: open a series' popover, change shape to
  `step` and markers to `off`, confirm **only that series** changes. Then confirm the chart-wide
  control still applies to the others. Screenshots to the session scratchpad, ABSOLUTE paths.

- [ ] **Step 3: Confirm the pruning** — in a combo, a series set to `bar` offers no line rows.

- [ ] **Step 4: Nothing regressed** on the "ZZ Chart Parity" dashboard's 19 charts.

- [ ] **Step 5: Open the PR. Do NOT merge.** Body carries: the failing-assertion text from each task,
  mutation results, what was verified live and what was not and why.

---

## Out of scope, deliberately

Per-series `missing` (Task 4 keeps the divergence), per-series trend line, per-series show-values, and
per-series y-axis position. Each needs a new top-level map in the codec's `KEYS` and a mirror on
`CartesianBuildConfig` — five files each, against Bucket A's one.

**One of them is a real capability gap, not a spelling difference**, and should be recorded as
follow-up rather than forgotten: egret's left/right axis choice is per-METRIC
(`rightAxisMetrics`, `cartesian.ts:1078`). Under a breakout every band shares `cfg.yMetrics[0]`, so
egret cannot split bands across two axes and Metabase can.

## Self-review

- **Task 1 is nearly free and Task 2 is the real work.** Task 1 stands alone: if Task 2 turns out
  worse than the current grid, Task 1 still closed the unreachable-setting defect.
- **The backend validates nothing here.** `crates/egret-bi/src/chartparams.rs:83-91` checks only that
  `params.settings` is a JSON object, and a test at `:125-131` pins that permissiveness. So unvalidated
  JSON can reach the builder — which is why `cartesian.ts:1070` guards `ln.size && ln.size in
  LINE_WIDTHS`. Any control added here needs the same defensiveness on read.
- **Known soft spot:** Task 2 Step 4 assumes `seriesDisplayFor` is reachable from a settings component.
  If it is not, do not copy the rule — a second display resolver is exactly how the colour rule
  diverged before (`viz/cartesian.tsx:130-135` records that).
