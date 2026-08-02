# Chart parity PR 05 — label room — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop four charts from drawing labels that do not fit — a sankey whose right-hand labels run
off the canvas, a row chart whose value ticks pile into one smear, a gauge that puts eleven numbers
round a small arc, and a pie whose slice labels overlap.

**Architecture:** Each is a size decision, so each attaches to the size-aware builder seam PR 01
created. Three of the four reuse machinery that already exists (`axisLabelFit`, `measureText`,
`sliceLabel`); the sankey needs new room-reservation in its own builder.

**Tech stack:** TypeScript, ECharts 6.1, vitest.

---

## Scope: this plan covers FOUR of workstream 5's six items

The design spec's workstream 5 lists six defects. Two of them are not ECharts label fitting and are
**deferred to PR 05b**:

- **The funnel** is a React component (`FunnelChart.tsx`), not an ECharts builder — a flex layout
  problem, measured below.
- **The pivot** is a table that collapses instead of scrolling. Also React, also not a builder.

They share no code with the four here and would double the PR's blast radius for no shared benefit.
Both are measured in this document so PR 05b starts from evidence rather than from the spec's prose.

## What the harness measured, before any change

egret cards are svg **166×184**; Metabase's are **311×223**. Read out of the DOM.

### 1. Sankey — the right-hand labels leave the canvas

| | egret | Metabase |
|---|---|---|
| right-column label x | **158** on a 166px svg | 273 on a 343px svg |
| label widths | 39–63px | 34–56px |
| right edge reached | **197–221px** | 307–329px |

egret's labels start 8px from the right edge and run 30–55px past it. The user sees a sliver — which
is what the design spec described as "truncate to one or two characters"; they are not truncated,
they are **drawn outside the canvas**. Metabase reserves room so its longest right-hand label
(`Doohickey`, 56px) ends 14px inside the edge.

`options/sankey.ts` already carries the diagnosis at lines ~129–142: `label.position` defaults to
`"right"` and is never overridden, so even the last column draws rightward into a `right: "8%"`
gutter; no `label.width` or `label.overflow` is set anywhere. That comment names the seam and says
the builder "has never been able to see" the size — PR 01 changed that.

### 2. Row chart — the value axis is never fitted

egret's bottom axis, all four ticks on one row at `y=136`:

| tick | x | width | span |
|---|---|---|---|
| `100K` | 101 | 31 | 101–132 |
| `200K` | 109 | 32 | 109–141 |
| `300K` | 117 | 33 | 117–150 |
| `400K` | 125 | 33 | 125–158 |

An 8px pitch for 31px labels: each overlaps its neighbour by 23px. Metabase's row chart at 311px
draws **one** tick, `0`.

**Cause, found in the source:** `options/cartesian.ts:1027` is

```ts
const ordinalX = !horizontal && !usesXScale(cfg);
```

and the fit ladder at `:1076` runs only `if (… && ordinalX && …)`. A row chart sets `horizontal`, so
`ordinalX` is false and **PR 02's ladder never runs for it**. Its bottom axis is a *value* axis
anyway, which wants a different remedy: fewer ticks, not rotated ones.

Second defect on the same chart: the rotated axis title `category` sits at `x=66, y=46, h=54`, while
the `Gadget` label occupies `x=56–100, y=51`. They overlap. Metabase puts `CATEGORY` at `x=1`,
outside its labels.

### 3. Gauge — eleven numbers on a small arc

egret draws `0`, `76,612.39`, `153,224.78` … `766,123.9` — **eleven labels**, several overlapping:
`229,837.17` spans x 65–122 and `306,449.56` spans x 73–135, both at `y=69`.

`options/gauge.ts:97-98` already states the cause and the fix in a comment: the builder sets no
`splitNumber`, so ECharts' default of 10 intervals puts eleven numbers round the arc at every size.

### 4. Pie — slice labels overlap

| label | x span | y span |
|---|---|---|
| `Doohickey` | 34–106 | 38–55 |
| `Widget` | 76–126 | 42–59 |

They overlap in both axes and both are drawn. This is the surprising one: `options/pie.ts` already
runs `fitSliceLabel` (`sliceLabel.ts`, a port of Metabase's own chord-length fit) **and** sets
`labelLayout: { hideOverlap: true }` as a backstop at `:1544`. Neither dropped these two.
**Establish why before changing anything** — the fix depends on whether the fit measured the wrong
room, or `hideOverlap` is not reached for a sunburst, or the labels are drawn by a path this file
does not fit at all.

Metabase at 208×214 draws no slice labels at all — only `1.5M` / `Total` in the centre — so "hide
them" is a legitimate answer at this size, not a cop-out.

### Deferred to PR 05b, measured here

**Funnel** (card 184×234). Leaf boxes, with `overflow:hidden; text-overflow:ellipsis`:

| element | box |
|---|---|
| `Visited` | 104×33 |
| `131,320` | 93×28 |
| `Added to cart` | **19×33** |
| `42.86 %` | **4×20** |
| `56,280` | **4×20** |

The first step takes 104px and the other three get 19px for a name and 4px for a number. Metabase's
funnel card on this dashboard says *"Which fields do you want to use?"* — it is unconfigured, so
**there is no reference rendering**. PR 05b must configure it first or argue the target from source.

**Pivot** (card 184×234). The table holds **2 rows**: a header (`order_month` / `Total`) and
`Grand totals 7,763.7 7,763.7`. `scrollableDescendants: 0` — nothing in the card scrolls. Metabase's
pivot card says *"Pivot tables are only supported for questions built in the query builder"*, and the
harness's cards are native SQL, so **no Metabase reference is obtainable for the pivot at all**. Its
target has to be argued on its own terms: scroll within the card rather than shed the body.

## Non-goals

- **Plot-area share.** egret's bar and box plot give 29% of card height to the plot where the line
  chart gives 58%, because 90°-rotated category labels reserve their full length. Metabase's are 59%
  on a taller card. This is worth its own investigation and is not in this PR — changing it means
  revisiting PR 02's rotate ladder, which is merged and measured.
- **Metabase's axis-title placement generally.** Only the row chart's overlap is fixed here.

---

## Task 1: the sankey reserves room for its labels

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/sankey.ts`
- Modify: `frontend/src/components/chart/echarts/__tests__/sankey.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
it("keeps the last column's labels inside the canvas", () => {
  // Nine nodes, the harness's own set. At 166px the right-hand labels ran 30-55px past the edge.
  const opt = buildSankeyOption(cols, rows, cfg, { width: 166, height: 184 });
  const s = opt.series[0];
  // The right gutter must be at least the widest right-column label, not a flat 8%.
  expect(gutterPx(s.right, 166)).toBeGreaterThanOrEqual(measuredWidth("Doohickey"));
});

it("draws the last column's labels to the LEFT of their nodes", () => {
  const opt = buildSankeyOption(cols, rows, cfg, { width: 166, height: 184 });
  expect(opt.series[0].label.position).toBeDefined();
});
```

Read the existing `sankey.test.ts` for its harness shape and the `measureText` stub convention used
by `sliceLabel`'s tests — do not invent a second measurer.

- [ ] **Step 2: Run it and watch it fail**

`cd frontend && npx vitest run src/components/chart/echarts/__tests__/sankey.test.ts`
Expected: FAIL — `right` is the string `"8%"` (13px of a 166px canvas) against a 63px label.

- [ ] **Step 3: Decide the mechanism, then implement it**

Two candidates, and the choice must be made against Metabase's behaviour, not guessed:

1. **Reserve the gutter**: measure the widest last-column label and set `right` to it. Matches what
   Metabase's layout achieves (its longest right label ends 14px inside the edge).
2. **Turn the last column's labels inward** (`label.position: "left"` for the final column). ECharts
   supports per-node label overrides, which this builder already uses for `itemStyle`.

Read Metabase's sankey layout (`../metabase/frontend/src/metabase/visualizations/echarts/graph/sankey/`)
and say which it does before choosing. If both are needed at a narrow card, say so.

Whatever is chosen must use the `size` argument PR 01 threaded through — the comment at
`sankey.ts:129-142` describes exactly this seam and should be updated to describe what now happens.

- [ ] **Step 4: Run the test, then the chart suite**

`cd frontend && npx vitest run src/components/chart`

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart
git commit -m "feat(charts): a sankey reserves room for the labels it draws"
```

---

## Task 2: the row chart's value axis is fitted

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts` (~1027, ~1076)
- Create: `frontend/src/components/chart/echarts/__tests__/rowAxisLabels.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
it("does not draw four value ticks in 57px", () => {
  // Measured: 100K/200K/300K/400K at an 8px pitch, each 31px wide — a 23px overlap per pair.
  const opt = buildCartesianOption(cols, rows, { ...rowCfg, kind: "row" }, { width: 166, height: 184 });
  const xa = Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis;
  expect(ticksDrawn(xa, 166)).toBeLessThanOrEqual(2);
});

it("keeps the rotated axis title clear of the category labels", () => {
  const opt = buildCartesianOption(cols, rows, { ...rowCfg, kind: "row" }, { width: 166, height: 184 });
  expect(titleGapPx(opt)).toBeGreaterThanOrEqual(0);
});
```

`ticksDrawn` must derive from what the option asks ECharts to draw — `splitNumber`, `interval`, or an
explicit tick array — not from a re-run of the fitter. If the only honest way to count is to render
the option through real ECharts in SSR, do that: `__tests__/valueAxisGutter.echarts.test.ts` already
does exactly this and is the precedent.

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL — four ticks in 57px.

- [ ] **Step 3: Fit the value axis**

The gate at `cartesian.ts:1027` (`const ordinalX = !horizontal && !usesXScale(cfg)`) is why the row
chart is never fitted. **Do not simply widen that gate** — a value axis is not a category axis and
must not be handed to the rotate ladder. A value axis has one remedy: fewer ticks. Derive a
`splitNumber` from the available width and the widest tick label, the way `estimatePlotWidth` and
`widestTickLabel` in `axisLabelFit.ts` already reason about value-axis ticks. Reuse them; do not
write a second copy of that arithmetic.

For the axis title: `cartesian.ts:1116` already honours an explicit rotation "WHEREVER the category
axis sits, `ordinalX` or not". Read that block before touching the title — the row chart's category
axis is the left one, and the title's `nameGap` is what puts it on top of the labels.

- [ ] **Step 4: Run the suite**

`cd frontend && npx vitest run src/components/chart`

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart
git commit -m "fix(charts): a row chart's value axis draws ticks that fit"
```

---

## Task 3: the gauge's graduations come from its radius

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/gauge.ts` (~163)
- Modify: `frontend/src/components/chart/echarts/__tests__/gauge.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
it("does not put eleven numbers round a small arc", () => {
  // Measured at 166x184: eleven labels, 229,837.17 and 306,449.56 overlapping at y=69.
  const opt = buildGaugeOption(cols, rows, cfg, { width: 166, height: 184 });
  expect(opt.series[0].splitNumber).toBeLessThanOrEqual(4);
});

it("gives a large gauge more graduations than a small one", () => {
  const small = buildGaugeOption(cols, rows, cfg, { width: 166, height: 184 });
  const large = buildGaugeOption(cols, rows, cfg, { width: 600, height: 600 });
  expect(large.series[0].splitNumber).toBeGreaterThan(small.series[0].splitNumber);
});
```

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL — `splitNumber` is `undefined`, and ECharts' default of 10 gives eleven labels.

- [ ] **Step 3: Derive the count from the measured radius**

The comment at `gauge.ts:97-98` already names `splitNumber` as the lever. The arc's length is what
the labels have to share, so the count follows from the radius and the widest formatted label — the
same measure-then-fit shape as `axisLabelFit`, and it should use the same `measureText`.

**The labels here are long** (`766,123.9` measured 51px). A count that fits `766,123.9` at 166px may
be 2. That is the right answer if it is what fits; do not pick a prettier number.

- [ ] **Step 4: Run the suite and commit**

```bash
cd frontend && npx vitest run src/components/chart
git add frontend/src/components/chart
git commit -m "fix(charts): a gauge's graduations fit the arc that carries them"
```

---

## Task 4: the pie's overlapping labels — diagnose first

**Files:**
- Investigate: `frontend/src/components/chart/echarts/options/pie.ts` (~1544, ~1693-1710),
  `frontend/src/components/chart/echarts/sliceLabel.ts`
- Modify: whichever the diagnosis names
- Modify: `frontend/src/components/chart/echarts/__tests__/pie.test.ts`

**This task starts with a question, not a change.** `fitSliceLabel` and
`labelLayout: { hideOverlap: true }` are both already in place and neither dropped `Doohickey` and
`Widget`, which overlap by 30px horizontally and 13px vertically at a 166×129 card.

- [ ] **Step 1: Establish the cause and report it before writing any fix**

Candidate explanations, each of which implies a different fix:
1. `fitSliceLabel` measured a chord the label does fit, but the label is drawn *outside* that chord
   (e.g. `position: "outside"` for a small slice), so the fit measured the wrong room.
2. `hideOverlap` is not applied to the series that draws these labels — the pie is a `sunburst`,
   and `labelLayout` may not be honoured there.
3. These labels come from a path that `fitSliceLabel` is not on at all.

Answer it by building the option for the measured config and reading what it asks for, then — if the
option looks correct — rendering it through real ECharts in SSR and reading the labels back. The
`.echarts.test.ts` files are the precedent for the second half.

**Report the cause before implementing.** If it is (2), the honest fix may be to hide slice labels
below a size threshold, which is what Metabase does at this card size — it draws none, only
`1.5M / Total` in the centre.

- [ ] **Step 2: Write the failing test, once the cause is known**

It must assert on the observed defect — two labels overlapping at 166×129 — not on the mechanism.

- [ ] **Step 3: Fix, run the suite, commit**

```bash
cd frontend && npx vitest run src/components/chart
git add frontend/src/components/chart
git commit -m "fix(charts): a pie draws no label it cannot place"
```

---

## Task 5: the live comparison, and the PR

**This is the acceptance test.** Every PR in this programme has shipped a defect that passed its unit
suite and was caught only here. A simulation is not evidence.

- [ ] **Step 1: Gates**

```bash
cd frontend && npx tsc --noEmit && npm run lint && npx vitest run
```
Report the exact test count.

- [ ] **Step 2: Build and measure**

```bash
cd frontend && npm run build
```

A static server on :5174 serves `frontend/dist` and proxies `/api`, `/catalog`, `/oauth` to
`127.0.0.1:30080`; the script is in the session scratchpad as `serve.py`. egret's session expires —
log in through the UI with `admin@egret.local` / `Admin12345!` if redirected.

egret `http://127.0.0.1:5174/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`,
Metabase `http://localhost:3033/dashboard/2`. Wait ~16s. Read positions with
`getBoundingClientRect()`.

Acceptance criteria, each against a number in this document:

1. **No sankey label extends past its svg's right edge.** Today they reach 197–221px on a 166px svg.
2. **No two row-chart value ticks overlap.** Today all four do, by 23px each.
3. **The row chart's axis title does not overlap a category label.** Today `category` at x 66–81,
   y 46–100 crosses `Gadget` at x 56–100, y 51–66.
4. **The gauge draws at most four graduations at 166×184**, none overlapping. Today eleven, with at
   least two overlapping.
5. **No two pie slice labels overlap.** Today `Doohickey` and `Widget` do.
6. **Nothing that fitted before stops fitting** — check the bar, line, area, combo, scatter,
   waterfall and box plot axes against the tables in PR 02 and PR 03.

- [ ] **Step 3: Open the PR**

Body carries the before/after table, the scope decision that funnel and pivot are PR 05b with the
measurements above, the pie diagnosis from Task 4, and any divergence found. Push and open.

**Do not merge.** Stop and report.

---

## Self-review

- **Spec coverage.** Workstream 5's six items: sankey (Task 1), row chart (Task 2), gauge (Task 3),
  pie (Task 4), funnel and pivot deferred to PR 05b with measurements recorded here.
- **The spec's description of the sankey is wrong** and this plan corrects it: the labels are not
  truncated to one or two characters, they are drawn outside the canvas. The visible result is the
  same; the cause, and therefore the fix, is not.
- **Task 4 is deliberately open.** It is the only one whose cause is not established, because two
  mechanisms that should have prevented it are already present. Writing a fix into the plan without
  knowing which one failed would be guessing, and this programme has been burned by that.
- **Known soft spot.** Task 2 asks for a `splitNumber` derived from width without specifying the
  formula, because `axisLabelFit.ts` already contains two functions that reason about value-axis
  ticks and the right answer is to reuse them. An implementer who finds they do not fit the row
  chart's geometry should stop and report rather than writing a third copy.
