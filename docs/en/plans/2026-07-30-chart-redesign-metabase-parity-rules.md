# Copying Metabase into egret — rules for charts 06–19

> Written after chart 05 (pie/sunburst) took 54 commits, most of them avoidable. Every rule below
> exists because breaking it cost real rework on that chart. Read this before designing the next one.

**Goal:** each remaining chart reaches Metabase's quality without repeating chart 05's detours.

---

## 0. The one-sentence lesson

**Matching Metabase's constants is not matching Metabase.** On chart 05 every constant in
`pie/constants.ts` was verified identical — radius ratios, border proportions, font sizes — and the
result still looked visibly worse, because the constants were right and everything around them was
wrong: the container, the series type, the tint derivation, the typeface. Constants are the last 10%.

---

## 1. Run both apps. Build the same chart in each. Measure.

Non-negotiable, and the step that found nearly every real defect on chart 05.

```bash
docker run -d --name metabase-ref -p 3033:3000 metabase/metabase:latest
# wait for /api/health, then:
TOKEN=$(curl -s localhost:3033/api/session/properties | python3 -c 'import sys,json;print(json.load(sys.stdin)["setup-token"])')
curl -s -X POST localhost:3033/api/setup -H 'Content-Type: application/json' -d "{\"token\":\"$TOKEN\",
  \"user\":{\"first_name\":\"Ref\",\"last_name\":\"User\",\"email\":\"ref@example.com\",
  \"password\":\"Refuser12345!\",\"site_name\":\"Ref\"},\"prefs\":{\"site_name\":\"Ref\",\"allow_tracking\":false}}"
```

The **E-commerce Insights** dashboard and its saved questions cover most chart types against the
sample database. `GET /api/dashboard/1` lists the cards with their `visualization_settings` — read
those first: they show which settings Metabase actually ships a chart with.

Then build the equivalent chart in egret on comparable data, at the **same viewport**, and compare by
measurement. Screenshots alone miss contrast and geometry; measurements alone miss layout — use both.

### What to measure, every time

| Dimension | How |
|---|---|
| Geometry | element bounding boxes; for round charts take the centre of the drawn sectors' bounding box |
| Colour | fills per series/level, and their saturation |
| Contrast | label ink vs its own background, WCAG ratio, count below 4.5:1 |
| Typeface | `getComputedStyle(text).fontFamily` — **check it is the product's font, not `sans-serif`** |
| Interaction | snapshot fills before/after hover; count how many elements respond |
| Truncation | how many labels contain `…` |

### Measurement traps that produced five false findings on chart 05

- `getComputedStyle` returns **`oklch(...)`** for egret tokens. A naive `rgb` regex reads its three
  numbers as RGB and reports nonsense contrast. Paint the colour to a 1×1 canvas and read it back.
- `getBoundingClientRect()` on a **rotated** label is its axis-aligned box, far larger than the glyphs.
  It reported 23 label collisions on a chart that was visually clean.
- **"Take the tallest column" finds the wrong centre of a circle** — many columns near the centre are
  within a pixel of each other. This invented a 22px horizontal offset that did not exist.
- Legend swatches share the chart's `<svg>` and inflate any bounding-box union.
- **`getBBox()` ignores the transform**, and ECharts scales a scatter symbol with one. Every point
  measured ~2px however large it was drawn, so a `width > 3` filter reported an empty chart — a
  chart that was in fact drawing correctly, and had been all along. Chart 10 spent an hour on that
  phantom. Use `getBoundingClientRect()` whenever the question is "how big does this LOOK".
- **A page held open across a redeploy keeps the old bundle**, even with a cache-busting query
  parameter, because its module graph is already loaded. Check the hashed filename in `script[src]`
  and open a **new** page. This produced a phantom "regression" that was dispatched for repair.

**Before reporting a defect, verify the measurement method.** Expect subagents to push back with
better evidence — on chart 05 they were right twice and I was wrong.

---

## 2. Copy the rule, not the value

Three categories. Decide which each thing is before copying.

**Copy verbatim** — geometry ratios, thresholds, layout constants, algorithms:
- radius ratios, border widths, size caps, padding
- fold thresholds (`SLICE_THRESHOLD = 0.025`), minimum drawn sizes (`OTHER_SLICE_MIN_PERCENTAGE = 0.005`)
- label-fit arithmetic, tint/shade derivation
- adaptive rules such as computing decimals from the data rather than fixing them

**Copy the rule, substitute egret's value** — anything tied to brand:
- **Typeface.** Metabase's charts use Lato because the product is Lato. egret's must use egret's font.
  On chart 05 egret's charts used *neither* — they fell back to the browser's `sans-serif` while the
  UI was Inter. Set `textStyle.fontFamily` at the option root. A `graphic` element does **not**
  inherit it and needs the family named directly.
- **Surface colours.** Metabase's tooltip is a hard-coded dark slate. egret needs a token, or the
  tooltip disappears in dark mode.

**Do not copy** — where Metabase is measurably worse:
- **Label contrast.** Metabase's own sunburst fails WCAG AA on **27 of 53** labels, worst 2.70:1. Its
  `getTextColorForBackground` biases 3× toward white; ported faithfully onto egret's palette it chose
  white at 2.86:1 where dark gave 6.30:1. **That port shipped and had to be reverted.** egret's rule —
  higher raw contrast wins, white preferred only when both candidates already clear 4.5:1 — stays.
- **Highlighting by name.** Metabase dispatches `{type:"highlight", name}`, which resolves through a
  linear depth-first scan. `"Other"` occurs at every level, so it lights the wrong node. Use `dataIndex`.

When a copy makes something measurably worse, say so and keep egret's version.

---

## 3. Read how the constant is *used*, never just its name or comment

`borderProportion: 360, // 1 degree` was read as "a one-degree gap" and implemented as
`padAngle: 1`. Its actual use is a **stroke width**: `(π × innerSideLength) / 360`. A stroke straddles
a boundary and neighbours share it — a hairline. An angular pad removes real area and **grows with
radius**: 4px voids at the outer ring, splitting it into 36 white wedges. That single misreading was
the chart's most-complained-about defect.

Also: **Metabase has dead constants.** `twoRingBorderWidth` and `threeRingBorderWidth` sit in its
constants file and `getBorderWidth` never reads them. Chart 05 copied both. Grep for each constant's
call site before adopting it.

---

## 4. Check the container before the chart

Chart 05's rings were 32% thinner than Metabase's, and the ratios were already identical. The card was
hard-coded `h-[60vh]` and threw away 231px of the 783px its parent offered — for every chart type.

Before tuning any chart, measure the **host box** in both apps. If egret's is smaller, that is the
bug.

Related: **an option builder must never guess a pixel size.** Chart 05 invented a 300px reference
container while the real one was ~530px, so every label was measured against 43% less room and all 51
truncated to stubs. `EChartsRenderer` already had a `ResizeObserver`; its `option` prop now accepts
`(size) => option`. Use it.

---

## 5. Prefer the series type Metabase uses

Chart 05 drew a multi-dimension pie as **N stacked independent `pie` series**; Metabase uses **one
`sunburst`**. Independent series cannot express parent/child, so:
- `emphasis: { focus: "ancestor" }` was impossible — hovering changed **5%** of wedges against
  Metabase's **96%**, and that hover is what makes a hierarchical chart readable;
- a pile of machinery had to be hand-written that a real `children` tree gives free: per-slice angles
  from a cumulative share, reordering by parent, parent-relative percentages.

Migrating fixed all of it — but it was a rewrite of a branch already 40 commits deep. **Check the
series type in Metabase's option builder before writing egret's.**

Migration traps found the hard way:
- ECharts' **`legend` component does not work with `sunburst`**. Metabase renders its legend as React
  markup outside the SVG. Move the legend out *first* or it silently vanishes.
- `percent` and `percentPrecision` are **`PieSeriesModel`-only**. A sunburst datum carries only
  `treePathInfo`. Compute percentages yourself and put them on the datum.
- Drill by `p.seriesName` breaks. Use `treePathInfo`, whose **first entry is ECharts' synthetic root**,
  so the ring index is `length - 2`.
- A sunburst assigns palette colours **by name**, skipping nodes that already have one — one override
  slides every later colour up a slot. Pin the anchor ring's colours explicitly.
- `sort` must be **present and `undefined`**, or ECharts merges its `'desc'` default and undoes any
  explicit ordering.

---

## 6. Interaction is most of the perceived quality

On chart 05 the rest state was already equal or better, and the chart still read as worse. What was
missing was all interaction:

| | Metabase | egret before |
|---|---|---|
| hover response | 96% of elements | 5% |
| the hole/centre | reads out the hovered slice | static |
| legend | highlights the hovered series, dims the rest | inert |
| tooltip | header, all siblings with value + %, focused row, `Total` footer | one line |
| click | drill menu | **nothing** |

That last one was not a chart bug: `PreviewPane` never passed `onValueClick` to `ChartRenderer`, so
**no ECharts chart had ever been clickable in the chart editor** — it worked only on dashboards. Check
this kind of plumbing at the seam, not in the chart.

**Audit interaction explicitly for every chart.** It is invisible to unit tests and to screenshots.

---

## 7. Tests that cannot fail, and the jsdom blind spot

Chart 05 shipped several tests that passed for the wrong reason. Standing requirement: **after writing
a test, break the thing it guards, re-run, confirm it fails, restore.** Report the mutation. Ask
subagents for the same. This caught a bug where a label's colour was set to its own fill — invisible
labels — that an `expect(a).not.toBe(b)` assertion happily passed either way.

Two structural blind spots:
- **jsdom runs no layout.** Anything about size, overlap or fit needs a browser.
- **Mocking `echarts/core` means the test can only assert what the component asks for, never what
  ECharts does.** Both hypotheses in one bad bug report lived exactly there. `EChartsRenderer.echarts.test.tsx`
  runs the **real SVG renderer in jsdom** and reads the drawn output — extend that harness rather than
  asserting option shape, whenever the claim is about what appears.

Beware self-referential tests: one that recomputes the formula it is testing proves nothing. Assert
independently known values (`calcChordLength(10, π) === 20` is the diameter).

---

## 8. Per-chart checklist

- [ ] Metabase running; the same chart built in both; viewport matched
- [ ] Its card's `visualization_settings` read from `/api/dashboard/1`
- [ ] Metabase's option builder read in full — **series type**, and each constant's call site
- [ ] Host box measured in both **before** touching the chart
- [ ] Geometry, colour, contrast, typeface, truncation measured in both
- [ ] Interaction audited: hover, legend, tooltip, click/drill
- [ ] Every constant classified: copy / substitute / reject — rejections justified with a number
- [ ] Contrast sweep over the palette × all levels, both themes; anything below 4.5:1 reported, not hidden
- [ ] Charts draw in egret's typeface, verified in a browser
- [ ] Every new test mutation-checked
- [ ] Live check on kind in a **new page**, bundle hash confirmed
- [ ] Anything deliberately left un-copied recorded in the PR, with its measurement

---

## 9. Reference measurements from chart 05

Useful as a sanity baseline for round charts.

| | Metabase | egret (final) |
|---|---|---|
| host box | 735×735 | 830×661 |
| outer diameter | 550 (capped at `maxSideLength`) | 542 |
| hole ratio, 3 rings | 0.244 | 0.242 |
| ring thickness | 69px | 69px |
| slice border | 1px uniform, no angular gap | same |
| label font | 12px w700, product font | same |
| labels truncated | 24/53 | 0/51 |
| labels below 4.5:1 | **27/53, worst 2.70** | 1/51, worst 4.32 |
| hover response | 96% | 96% |

egret's one sub-AA label is the deliberate cost of adopting Metabase's darker inner ring; it is
recorded rather than hidden. `#7172AD` (the 8th accent) cannot clear 4.5:1 against either ink in
either theme — a palette limit affecting any chart with 8+ series.
