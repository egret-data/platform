# Chart Redesign 06 — Scalar (Big number)

**Status:** approved design, pending implementation plan
**Date:** 2026-07-30
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Parity rules:** `docs/en/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Previous units:** bar `1fe27573`, line `1ee0819e`, table `2638db51`, gauge `9a8e810b`, pie `d0d4a037`, debt `01e91ab9`
**Vietnamese reference:** `docs/vi/specs/2026-07-30-chart-redesign-06-scalar.md`

## What a scalar is for

Putting one number where it cannot be missed. It has no axes, no legend and no marks — the number
*is* the chart. So everything that would be "polish" on a bar chart is load-bearing here: how big
the number is, whether it fits, whether it can be read at a glance, and whether it is coloured to
say something. There is nothing else to look at.

That is why this chart is short on code and long on defects. `BigNumber.tsx` is 30 lines and every
one of them works; the problem is the twenty-five it does not have.

## Verified state on both sides, 2026-07-30

Read, not recalled — the program's rule after the table, gauge and pie rows were each wrong.

**egret** — `frontend/src/components/chart/BigNumber.tsx`, rendered from `ChartRenderer.tsx:353`:

```tsx
<div className="text-[2.5rem] font-bold leading-none tabular-nums text-foreground">{text}</div>
{metric && <div className="mt-2 text-sm text-muted-foreground">{metric}</div>}
```

`ScalarConfig` is `{ metric?: string }` — one field. Number formatting already works and is good:
`columnSettings[metric]` carries prefix, suffix, decimals, currency, scale, separators and a
`compact` flag through the shared display sections (#336), and `formatValue` honours all of it. The
`role="img"` + single `aria-label` is a genuinely nice touch that reads "total_revenue: 25,468" as
one utterance. **None of that changes.**

**Metabase** — `visualizations/Scalar/Scalar.tsx` and `components/ScalarValue/`. Live card 39 of the
sample dashboard ("Total orders this quarter") ships `scalar.segments` with three coloured bands,
so this is not a corner of the product nobody uses.

## The five gaps

### 1. The number is one fixed size, in every card, forever

`text-[2.5rem]`, whatever the card is. On a large card the number floats in a sea of white space; on
a small one it does not shrink to fit. Metabase sizes it to the box: `findSize` measures the text on
a canvas and steps the font down until it fits both dimensions.

```
step 0.2rem, weight 700, min 1rem, max = getMaxFontSize(...)
max: (cardColWidth / GRID_WIDTH) × (7 − 2.2) + 2.2, floor 2.2rem, ceiling 7rem
     …but 2.5rem if the card is narrower than 240px
```

Those are geometry constants — **copied verbatim** (rule 2). egret's `GRID_COLS` is 24, the same as
Metabase's `GRID_WIDTH`, so even the ratio ports unchanged. At 2.5rem today, egret's number is
below Metabase's *floor* for any card wider than a quarter of the grid.

Two traps come with `findSize`, both from reading its use rather than its name:

- `measureText` appends its canvas to `document.body` and sets `canvas.style.fontSize` from the
  document root, **because `context.font` in `rem` resolves against the element's own font-size**.
  A detached canvas silently measures against 16px and the fit maths is wrong wherever the user has
  changed their base size. Copy the whole trick, not the loop.
- Its `height` is `actualBoundingBoxAscent + actualBoundingBoxDescent` — glyph height, not line
  height. Fitting against line height leaves a visible gap at the bottom of every card.

And the measured width must be plumbed, never guessed — chart 05's invented 300px container
truncated all 51 of its labels. `useElementSize` already exists for exactly this.

### 2. A long number has nowhere to go

egret has no width-driven compaction. `ColumnFormat.compact` exists but is a manual, always-on
switch, so a user picks between "3.4M forever" and "3,428,911 clipped on a small card".

Metabase decides per render, from the measured width:

```ts
COMPACT_MIN_LENGTH = 6, COMPACT_MAX_WIDTH = 250, COMPACT_WIDTH_PER_DIGIT = 25
compact when  len > 6  &&  (width < 250 || width < len × 25)
```

and — the part that makes it safe — keeps the full value and shows it in a tooltip whenever the
displayed value differs. The number stays readable and the exact figure stays one hover away. An
explicit `compact: true` in column settings still wins; this only decides the *undecided* case.

### 3. Clicking the number does nothing

`ChartRenderer` receives `onValueClick` and hands it to the pie branch. The scalar branch does not
take it. Rule 6 exists because this is most of the perceived difference between the two products:
on chart 05 the rest state was already equal and the chart still read as worse.

A scalar's drill is the simplest one in the program — one value, no dimensions, so `drillFilters`
of an empty context — but "see the records behind this number" is the single most obvious thing to
want from a number that large. Metabase also turns the value brand-coloured on hover when it is
clickable, which is the affordance that tells you it *is* clickable.

### 4. The number cannot say whether it is good

`scalar.segments` — `{ min, max, color, label? }[]` — colours the value by the band it falls in, and
a hover tooltip lists the bands as a legend.

This is the gauge's model (chart 04) applied to a number instead of an arc. **An earlier draft of
this spec said the gauge's type and editor could be reused as they stand. Reading
`GaugeSegmentsSection.tsx` and `echarts/options/gauge.ts` in full before writing the plan showed
that is wrong on four counts** — and every one of them is gauge behaviour that exists to satisfy a
constraint a scalar does not have:

| | Gauge | Scalar | Why |
|---|---|---|---|
| bounds | `min: number; max: number`, both required | `min: number \| null; max: number \| null` | an arc needs two ends to have a length; a number does not. `segmentIsValid(s, { allowOpenEnded: true })` — "≥ 1701 is High" is the natural top band |
| editing `max` | pushes the next band's `min` to match | independent | ECharts' `axisLine` stops are **cumulative**, so a gauge gap silently swallows a zone. A scalar just tests `min <= v && v <= max` |
| order | sorted by `max` in the builder | **authored order preserved** | `segments.find` — first match wins, so order is meaning. Sorting would change which band a value lands in |
| removing the last row | refused (no arc without one) | allowed | Metabase passes `canRemoveAll: true` for exactly this. Zero bands is the *default* state |
| nothing stored | three zones derived from the value | **no colour** | chart 04's lesson: inventing bands with a guessed direction states something false. Here the analogue is saying nothing |

So `ScalarSegment` is its own type, and the shared part — the row (swatch, min, max, name, delete),
the add button cycling `SERIES_PALETTE`, and the rule that rows are **never re-sorted while being
edited**, because a row jumping out from under the cursor mid-keystroke is the reason that rule
exists — is lifted into a `SegmentRangesSection` that both charts drive with explicit props.
`GaugeSegmentsSection` becomes a thin wrapper supplying gauge's four behaviours, so its rendered
output and its tests are unchanged. A second hand-copied editor would drift; `01e91ab9` was spent
cleaning up precisely that kind of duplication.

**The band's meaning must not live in the colour alone — this is where egret departs from
Metabase.** `/ui-ux-pro-max --domain ux` flags `color-not-only` at **High** severity ("don't convey
information by color alone"), and its gauge/bullet guidance says the same in the KPI context:
*"Always show numerical value + % of target as text beside chart. Never rely on color position
alone."* Metabase puts the band labels in a **hover tooltip only**, so a red number and a green
number are indistinguishable to a colourblind reader, to anyone on a monochrome display, and to
anyone who never hovers.

So when a value falls in a band **that has a label**, egret renders that label as visible text under
the number — a small caption, not a tooltip. Colour stays as the fast signal; the word carries the
meaning. Bands without a label stay colour-only, because there is nothing to say. This costs one
line of layout and is the difference between an accessible KPI and a decorative one; it is recorded
in the PR as a deliberate non-copy, with the rule cited.

**Contrast is an obligation here, not a nicety.** The colour lands on *ink*, not on a fill, so a
user-chosen band colour can put the headline number below AA against the card. The number is bold
and ≥1rem, which straddles the WCAG large-text boundary (3:1 at ≥24px bold, 4.5:1 below it), so the
sweep must be run at the **minimum** size, in **both themes** — chart 05 shipped a light-mode-only
sweep and put 51/51 dark-mode labels under AA. Findings get reported with numbers, not hidden, and
where a chosen colour cannot clear the bar the editor says so rather than silently rendering it.

### 5. The metric name is printed twice

`{metric && <div …>{metric}</div>}` prints the machine column name under the value, always.
Dashboard cards already render a header from `titleOverride || chart.name`
(`DashboardCardView.tsx:55`), so on a dashboard the name appears twice — once as prose, once as
`total_revenue`. Metabase does not print it at all; the card title carries it, and its `scalar.label`
/ `scalar.sublabel` are hidden settings used only by the metrics viewer.

egret follows Metabase, with the label kept as an **explicit optional setting, default off**. The
recorded cost: an existing saved scalar loses the printed column name. On a dashboard it loses
nothing, because the header already said it; in the studio the chart name is on screen too. The
gain is real — the label is up to 20% of the card's height, and the auto-sizer spends all of it on
the number.

## The model

```
ScalarConfig {
  metric?: string;                 // unchanged
  segments?: ScalarSegment[];      // new
  label?: string;                  // new — optional, replaces the always-on metric name
  sublabel?: string;               // new — optional second line
}

ScalarSegment { min: number | null; max: number | null; color: string; label?: string }
```

`min` and `max` are **nullable, not optional** — copied from Metabase's type. The difference
matters: a band edited down to an empty input must round-trip as an explicit "no bound" through
JSONB, and an optional key that disappears is indistinguishable from one that was never set. A
band with both ends null matches everything and is dropped as invalid, matching `segmentIsValid`.

**No migration.** Every new key is additive and every old key keeps its meaning, so there is no
0104. This is the first chart in the program without one, and it is worth saying out loud: the
always-on label disappearing is a *render-time* behaviour change, not a stored-data change, so
there is nothing to convert and nothing that can half-convert. Chart 02's deploy-order rule does
not apply.

`fromParams` forwards every key under the `scalar.` prefix, so `segments`, `label` and `sublabel`
round-trip with no codec change — the same property that let the gauge's segments through. The
editor is a `display` control on the scalar descriptor in `viz/simple.tsx`, next to where pie and
gauge put theirs; the Data tab keeps asking only for the metric.

And per `01e91ab9`: the scalar branch of `ChartRenderer` **spreads** its config. It currently
hand-lists `metric`, which is the exact whitelist that cost pie four settings and cartesian four
before it — and this unit adds three fields to that side.

## Deliberately not done

- **Multi-series scalar → bar chart.** Metabase's `scalarToBarTransform` turns several scalar cards
  into one bar chart. egret has no multi-series card, so there is nothing to transform.
- **PNG export stays unavailable.** Metabase sets `canSavePng = false` on Scalar. egret arrives at
  the same place by accident and correctly: `ChartExportMenu` offers PNG only when the container
  holds an `<svg>`, and a big number is plain DOM. The live check confirms the item is absent
  rather than present-and-broken — memory records a PNG export that grabbed a 16px UI icon.
- **`scalar.field` auto-defaulting.** Metabase defaults to `cols[0]` and hides the picker when there
  is only one column. egret's descriptor requires the metric explicitly, which is the program's
  established pattern across all nine simple descriptors; changing it here alone would be an
  inconsistency, not a parity win.
- **`scalar.switch_positive_negative` and `scalar.compact_primary_number`** belong to SmartScalar,
  chart 11. They are typed as scalar settings in Metabase but its Scalar never reads them.
- **The number formatting.** It already matches. Do not touch `formatValue`.

## What the live measurement changed

Measured in a browser at 1440×920 against a harness rendering the real component at five card
sizes in both themes. Three things were wrong that no unit test could have shown.

**1. The font ceiling was 7rem where Metabase's is 4rem.** Metabase reads
`max: gridSize ? getMaxFontSize(…) : 4` — the no-grid fallback is a flat `4`, not `MAX_SIZE_LARGE`.
Misreading it put a two-digit number at **112px on a 300×160 card**, filling it corner to corner.
Fixed, and the card's grid width is now plumbed (`DashboardCardView` → `ChartRenderer` →
`BigNumber`) so a dashboard card scales its ceiling by its share of the grid, as Metabase does.

**2. `SERIES_PALETTE` is unusable as band ink, and the whole palette failed.** A band colours the
numeral itself, which is a different problem from filling an area. Measured as ink on the light
card, **all 8 series colours fail 4.5:1 and 6 of 8 fail even 3:1** (worst `#F9D45C` at **1.44**).
Dark mode was fine (7/8 over 4.5), which is exactly how a light-mode-only sweep would have missed
it — chart 05's lesson, repeated.

There is also a hard limit worth stating, because it bounds what any palette can promise here:
**no single colour clears 4.5:1 against both themes.** Light demands relative luminance ≤ 0.183,
dark demands ≥ 0.274; the ranges do not overlap. Checked against 27 candidates — all 27 fail one
side. Since the numeral is bold and never renders below 24px in a real card, the bar that applies
is the large-text **3:1**, and a purpose-built `SCALAR_BAND_PALETTE` clears it in both themes:

| | light | dark |
|---|---|---|
| before (`SERIES_PALETTE`) | **8/8 fail 4.5, 6/8 fail 3.0**, worst 1.44 | 1/8 fail 4.5, worst 3.97 |
| after (`SCALAR_BAND_PALETTE`) | 8/8 pass 3.0, worst **3.73** | 8/8 pass 3.0, worst **3.60** |

The editor also warns, naming the failing theme and the number, when a user picks a colour of their
own that misses the bar — silence would mean shipping an unreadable headline number.

**Metabase's own segment colours are not copied.** Its sample card 39 ships `#51528D`, which
measures **2.48** against egret's dark card.

**3. A float-accumulation bug in the ported `findSize`.** Metabase's bare `size - step` drifts:
stepping 7 down to what should be 2.4 yields `2.399999999999996`, which reaches the DOM as a
font-size with sixteen decimals and never lands on the step grid its own constant describes. Each
step is rounded now. Caught by this module's own tests, not by reading.

Confirmed good in the same pass: the value draws in **Inter** (the product font — chart 05's
`sans-serif` defect does not reach a DOM chart), nothing overflows at any of the five sizes,
compaction fires with the full value in the tooltip (`3,428,911` → `3.4M`), the narrow-card cap
lands exactly on 2.5rem at 220px, and the band name renders as text under the number.

## Live check on kind — what running it actually found

Deployed to kind (frontend image rebuilt, `kind load`, rollout, **new page, bundle hash confirmed**)
and driven end to end against real Trino data (`Demo Sales`, `total_revenue` = 116,269), side by
side with Metabase card 39 at 1440×920.

**A fourth defect, and only the screenshot could see it.** The captions were stranded at the
**bottom of the card, ~300px under the number**: the value's box had `flex-1 basis-0` and swallowed
all the spare height. Every numeric check passed — the number fitted, nothing overflowed, contrast
was fine — because none of them measured the *gap*. The measuring element is now absolutely
positioned (out of flow) and the caption block is measured so its height is subtracted from the
number's. Confirmed after redeploy: gap 0px, group centred to within 3px.

This is the rule from §1 earning its place in the other direction: measurements alone miss layout.

**Also fixed here:** `Records where all rows` was not a sentence. A drill with no filters was
unreachable until the scalar arrived — it has no dimensions, so its context is always empty — so a
string that had always been wrong only now became visible. It reads `All records`.

**A near miss worth recording:** a rebuild run from the wrong directory failed, and the very next
command happily re-loaded the *old* image and reported a successful rollout. Verifying the bundle
hash and the Docker image ID is what stopped a check against stale code.

### Side by side with Metabase card 39

| | Metabase | egret |
|---|---|---|
| font-size | **64px** | **64px** |
| weight | 700 | 700 |
| typeface | Lato (its product font) | Inter (egret's) |
| band colour | `#E75454`, **3.62:1** on white | `#5B8C3A`, **3.99:1** |
| band name shown | **never** — colour only | as text under the number |
| drill | menu | menu (`All records` → real rows) |
| PNG export | `canSavePng = false` | absent (no `<svg>` in the container) |

The 64px match is the strongest evidence the port is right: both apps land on the same no-grid
ceiling from the same constants, on different data.

### Verified in the running app

Round-trip through the backend is clean — `scalar.segments` stores under the flat `scalar.` prefix
and **`"max": null` survives as null, not `0`**, which is the `Number("") === 0` trap the editor
guards. Reloading the saved chart renders it identically (no repeat of the pie's silent blank).
The band editor cycles the new palette, the contrast warning fires with the measured number
(`#F9D45C` → "light 1.44:1"), drill opens real records, the export menu offers CSV and not PNG
even though four 16px icons sit in the same `main`, and a dashboard card compacts to `116.3K` at
44.8px with the caption still under the number. Both themes checked.

## Live check

On the Demo Sales dataset, against Metabase's card 39 at a matched viewport:

1. Host box measured in both **before** anything is tuned (rule 4).
2. A scalar on a large card: the number fills it, and the size lands within the copied bounds.
3. The same scalar on a 2×2 card: it shrinks rather than clipping.
4. A card narrower than 240px caps at 2.5rem.
5. A 9-digit number compacts, and hovering shows the full value.
6. `compact: true` in column settings still compacts a short number.
7. Clicking the number opens the drill menu; the value is brand-coloured on hover.
8. Three bands as in card 39 — the value is red under 1500, blue to 1700, green above.
9. An open-ended top band (`≥ 1701`, no max) colours correctly.
10. Hovering shows the band legend with its colours and labels.
11. An unconfigured scalar is foreground-coloured and unchanged from `main`.
12. The metric name no longer appears under the number; the card header still names it.
13. Setting `label` prints it; setting `sublabel` prints both.
14. Export menu on a scalar offers CSV and **not** PNG.
15. Both themes, for the contrast sweep and for the band colours.

## Verification

The program's gate: lint, typecheck, tests, build, deploy to kind, live check in a **new page** with
the bundle hash confirmed, before/after screenshots at both card sizes. No migration, so the backend
gate is out of scope and there is no deploy-order constraint.

Every new test is mutation-checked and the mutation reported (rule 7). Two blind spots apply
directly to this chart and both need the real-browser harness rather than jsdom: **jsdom runs no
layout**, so every claim about fit, size and compaction is unprovable there, and `measureText`
returns zeroes — an auto-sizer tested under jsdom will "pass" at whatever size the loop bottoms out
at. Extend `EChartsRenderer.echarts.test.tsx`'s approach — read the drawn output — rather than
asserting what the component asked for.
