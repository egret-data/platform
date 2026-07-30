# Chart Redesign 06 — Scalar: implementation plan

**Spec:** `docs/en/specs/2026-07-30-chart-redesign-06-scalar.md`
**Parity rules:** `docs/en/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Date:** 2026-07-30
**Vietnamese reference:** `docs/vi/plans/2026-07-30-chart-redesign-06-scalar.md`

Nine tasks. Every file path and every claim about existing code below was read on 2026-07-30, not
recalled — the program's standing correction after the plan for chart 01 was wrong on 6 of 9 tasks.

**Backend: not in scope, verified.** `crates/egret-bi/src/chartparams.rs` validates only shape
(version, `viz_type` against `VIZ_TYPES`, the spec), and its own header says *"Settings keys are
equally deliberately open — A2 chose flat settings so keys an older build doesn't know still survive
a round-trip."* The three new keys need no server change and there is no migration, so there is no
deploy-order constraint either.

---

## Task 0 — Baseline measurement, before any code

Rule 4: measure the host box in both apps *first*. Chart 05's rings were 32% thin with correct
ratios because nobody did this.

- Metabase card 39 ("Total orders this quarter") at 1440×920; egret's equivalent scalar on the Demo
  Sales dataset, same viewport, both as a dashboard card and in the studio.
- Record: host box, rendered font size in px, value colour, `getComputedStyle(...).fontFamily`.
- **Paint colours to a 1×1 canvas and read them back** — egret's tokens compute to `oklch(...)` and
  a naive rgb regex reports nonsense.

Output: a table in the PR. If egret's host box is smaller, that is the first bug, and it is not a
scalar bug — chart 05 found `h-[60vh]` throwing away 231px for *every* chart type. Check whether
that is still the case before tuning a font size against a container that is already wrong.

## Task 1 — Types and the renderer seam

`frontend/src/api/charts.ts`:

```ts
export interface ScalarSegment { min: number | null; max: number | null; color: string; label?: string }
export interface ScalarConfig {
  metric?: string;
  segments?: ScalarSegment[];
  label?: string;
  sublabel?: string;
}
```

`ChartRenderer.tsx:353` — the scalar branch hand-lists `metric`. **Spread it**, following the
comment already sitting above the five branches fixed in `01e91ab9`, with `columnSettings` and
`metricTypes` after the spread so stored settings cannot shadow the shell's.

This task is where the whole unit can silently fail: the pie shipped four settings that were
implemented, panel-exposed and unreachable, because tests called the builder directly and bypassed
this line. **The seam test goes in this task, not at the end** — render through `ChartRenderer`
with `vizType="scalar"` and assert a stored `segments` colour reaches the DOM.

## Task 2 — Text measurement

New `frontend/src/components/chart/measure-text.ts`, ported from Metabase's
`frontend/src/metabase/utils/measure-text.ts`:

- Module-level `<canvas>`, **appended to `document.body`**, `display:none`, with
  `canvas.style.fontSize` set from `getComputedStyle(document.documentElement).fontSize`. This is
  not incidental — `context.font` in `rem` resolves against the element's own font-size, so a
  detached canvas measures against 16px and every fit calculation is wrong wherever the user has
  changed their base size.
- Height is `actualBoundingBoxAscent + actualBoundingBoxDescent` — glyph height, not line height.
- `findSize({ text, targetHeight, targetWidth, unit, fontFamily, fontWeight, step, min, max })`,
  stepping down from `max` while it overflows either dimension. Copied verbatim.

**jsdom returns zeroes from `measureText`.** A test asserting a *chosen size* under jsdom proves
nothing — it asserts where the loop bottoms out. Unit-test `findSize` against an **injected**
measurer with known metrics (`calcChordLength(10, π) === 20`-style: independently known values, not
the formula recomputed), and leave every real-metrics claim to Task 9's browser check.

## Task 3 — Auto-sizing in `BigNumber`

- `useElementSize` (already exists, `components/chart/useElementSize.ts`) on the value's host.
- Max font from Metabase's rule, constants copied verbatim:
  `(cardColWidth / GRID_COLS) × (MAX_SIZE_LARGE − MAX_SIZE_SMALL) + MAX_SIZE_SMALL`,
  `MAX_SIZE_SMALL = 2.2`, `MAX_SIZE_LARGE = 7`, and `NARROW_CARD_MAX_SIZE = 2.5` when the card is
  under `NARROW_CARD_WIDTH = 240`. egret's `GRID_COLS` is 24, identical to Metabase's `GRID_WIDTH`,
  so the ratio ports unchanged.
- Min 1rem, step 0.2rem, weight 700.
- **The font family passed to the measurer must be the one the DOM actually renders.** Chart 05
  drew every chart in `sans-serif` while the UI was Inter. Read it from the element rather than
  naming a constant, and assert it in Task 9.
- Padding: Metabase subtracts `PADDING = 32` twice from height and once from width. Copy that, and
  note in the PR that it is why the number never touches the card edge.

Keep `tabular-nums`, `leading-none`, `role="img"` and the single `aria-label` exactly as they are.

## Task 4 — Width-driven compaction and the full-value tooltip

New `compactifyValue(value, width, format)` beside `format.ts`, copied:
`COMPACT_MIN_LENGTH = 6`, `COMPACT_MAX_WIDTH = 250`, `COMPACT_WIDTH_PER_DIGIT = 25`;
compact when `len > 6 && (width < 250 || width < len × 25)`.

- An explicit `format.compact === true` still wins — this decides only the undecided case.
- Return both values; show the full one in a tooltip **only when it differs** from what is drawn.
- The tooltip must be reachable by keyboard, not hover-only. The exact figure being one hover away
  is what makes compaction safe; if it is mouse-only, it is not.

Order matters: format → compact → measure → size. Sizing against the uncompacted string picks a
font for text that is not on screen.

## Task 5 — The shared segment editor

Extract `frontend/src/components/chart/settings/SegmentRangesSection.tsx` from
`GaugeSegmentsSection.tsx`, parameterised by the four differences the spec tabulates:
nullable bounds, contiguity push on/off, minimum row count (1 vs 0), and the fallback rows when
nothing is stored.

`GaugeSegmentsSection` becomes a thin wrapper passing gauge's four values. **Gauge's rendered output
and its existing tests must not change** — that is the acceptance condition for this task, and the
reason for a wrapper rather than a rewrite of a chart that is already live.

Scalar's wrapper: no contiguity push, no sort, `minRows: 0`, no fallback rows, and empty bound
inputs producing `null` rather than `0`. `Number("")` is `0` — a band edited to empty must not
silently become "from zero".

Wire it as a `display` control on the scalar descriptor in `viz/simple.tsx`, beside `pieDisplay`
and `gaugeDisplay`. `fromParams` forwards every key under the prefix, so no codec change.

## Task 6 — Segment colour and the legend tooltip

`getSegmentColor(value, segments)` — copied semantics: `parseFloat`, `find` (**first match wins**,
so authored order is meaning and nothing sorts), missing `min` is −∞, missing `max` is +∞, no match
or no segments falls back to the foreground token. A band with both bounds null is invalid and
dropped, matching `segmentIsValid(s, { allowOpenEnded: true })`.

Legend tooltip listing each band as swatch / range / label, ranges formatted `min - max`, `≥ min`,
`≤ max` — copied from `formatSegmentRange`.

**Not copied:** Metabase's hard-coded dark tooltip surface. egret uses a token or it vanishes in
dark mode (rule 2).

## Task 7 — Click to drill, and the hover affordance

- Pass `onValueClick` into the scalar branch — it already reaches `ChartRenderer` and already goes
  to pie.
- A scalar has no dimensions, so the drill context is empty and `drillFilters` yields no filters:
  "show me the records behind this number".
- Brand colour on hover **only when clickable**, and a real `<button>` semantics-wise so it is
  keyboard-reachable — Metabase uses a `Stack` with `onClick`, which is a mouse-only affordance and
  is not worth copying.
- Check the seam, not the chart: chart 05 found `PreviewPane` never passed `onValueClick`, so no
  ECharts chart had ever been clickable in the editor. Verify scalar is clickable in **both** the
  studio and a dashboard.

## Task 8 — Label, sublabel, and removing the always-on metric name

- Delete `{metric && <div …>{metric}</div>}`.
- Render `label` (14px) and `sublabel` (12px, muted) when set, both centred under the value, and
  both counted against the height the auto-sizer is given — otherwise a labelled number overflows
  its card.
- Text controls on the scalar descriptor.

Recorded cost, to appear in the PR: an existing saved scalar loses the printed column name. Nothing
is lost on a dashboard, where `DashboardCardView.tsx:55` already renders the header.

## Task 9 — Contrast sweep, live check, evidence

**Contrast sweep**, both themes, at the **minimum** font size (1rem bold — under the 24px WCAG
large-text boundary, so the bar is 4.5:1, not 3:1): every `SERIES_PALETTE` entry as segment ink
against the card background. Report the numbers, including failures; chart 05's palette limit was
recorded rather than hidden and that is the standard. Where a chosen colour cannot clear the bar the
editor says so — it must not silently render an unreadable headline number.

**Live check on kind**, in a **new page** with the bundle hash confirmed from `script[src]` — a page
held open across a redeploy keeps the old module graph and produces phantom results. Walk the
spec's fifteen numbered cases.

Confirm the export menu on a scalar offers CSV and **not** PNG, and that this is because the
container holds no `<svg>` — memory records a PNG export that grabbed a 16px UI icon, so verify the
absence rather than assuming it.

**Every new test mutation-checked**: break what it guards, re-run, confirm failure, restore, report
the mutation in the PR. Chart 05 shipped a regression test that survived its own mutation because
its fixture agreed at both code paths by accident.

---

## Order and stacking

1 → 2 → 3 → 4 in sequence (each consumes the previous). 5 → 6 depend only on 1. 7 and 8 depend only
on 1. 9 last.

One PR, per the program. Branch from `main` at `01e91ab9`.

## Verification gate

Frontend lint, typecheck, unit tests, build; deploy to kind; live check; before/after screenshots at
both card sizes and in both themes. Run the full suite locally — GitHub CI is out of quota. No
migration, so the backend gate is out of scope.

Open the PR and **stop there** for review; do not merge.
