# Everything still open — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Close what is genuinely left after nine PRs (#407–#415), in three units that do not mix
concerns.

**Architecture:** **PR A is accessibility** — things a keyboard user cannot see or cannot reach.
**PR B is two chart defects.** **PR C is three Metabase features egret does not have.** They are
independent; A first, because it is the only one with a defect a user hits on every menu.

**Tech stack:** React 18, Radix, ECharts 6.1, Tailwind v4 with OKLCH tokens, vitest.

---

## A note on where this list came from, and what was pruned

Three audits and nine PRs produced a running list of leftovers. **Before writing this plan, every
item was re-checked against the code, and five did not survive:**

- **scatter's Bubble size** — already implemented. `bubbleMetric` is in the codec, the builder reads
  it (`cartesian.ts:784`), the tooltip uses it (`:850`), and it has a test.
- **pie's Show total** — already implemented. `pie.ts:857` reads `cfg.showTotal ?? true`, gated on the
  donut hole, with a control at `PieDisplaySection.tsx:153-160`.
- **`resizable.tsx` "a ring colour with no ring width"** — false. `focus-visible:ring-1` is on the
  line below the colour.
- **scatter's missing Y value-axis block** — not an oversight. `CartesianDisplaySection.tsx:265-266`
  states that the metric-vs-metric scatter leaves through `scatterXY`, which builds its own two value
  axes and never reads those keys. A stated divergence, not a gap.
- **`ring-offset-2` without `ring-offset-background`** — real but a single site, not a pattern:
  `routes/control-plane/access/AccessOverview.tsx:376`. Folded into PR A as a one-liner.

**Two of those came from a live panel-parity sweep and one from a PR's own "found, not fixed" list.**
Both are normally reliable sources. The lesson is in this plan's self-review; treat every remaining
item below as re-verified, because it was.

---

# PR A — what a keyboard user cannot see or reach

## Task A1: a focused menu item is invisible

**MEASURED.** A focused item in a dropdown, select, context menu or command palette has **no focus
ring at all** — it carries `outline-none` and signals focus only by swapping its background to
`--accent`. Light `--accent` is `oklch(0.97 0 0)` against a `--popover` that is nearly white:
**1.03:1**.

Sites (verify the list, it may have grown):
- `components/ui/dropdown-menu.tsx:28, 84, 100, 124` — `outline-none focus:bg-accent`
- `components/ui/select.tsx:108` — `outline-hidden focus:bg-accent`
- check `command.tsx`, `context-menu.tsx`, `menubar.tsx`, `navigation-menu.tsx` for the same shape

**This is not the same defect #415 fixed.** #415 raised `--ring` so focus *rings* pass 3:1. These
elements have no ring to raise — they opted out with `outline-none`. So the fix is a different
decision, and it is a real one:

- [ ] **Step 1: Decide the mechanism and say why.** Options, and they are not equivalent:
  - restore a visible focus indicator (a ring, or a left bar) on the focused item;
  - darken `--accent` so the background swap itself carries 3:1;
  - both.

  **Darkening `--accent` is the wider blast radius** — it is a hover/active background across the
  whole product, not just menus, so changing it changes resting hover states everywhere. Check what
  else uses it before choosing. Metabase's own menus are worth a look for the shape, not the value.

- [ ] **Step 2: Write the failing test.** #415 added `frontend/src/__tests__/focusIndicatorContrast.test.ts`
  which computes contrast from `index.css` deterministically — extend it rather than writing a second
  contrast harness. Pin **both themes** and pin **3:1**, with a comment saying why it is 3 and not 4.5
  (non-text contrast, WCAG 1.4.11 / 2.4.11).

- [ ] **Step 3: Implement, then LOOK AT IT.** A contrast number is not a design review — this
  programme has a recorded case of a caption passing every numeric check while sitting 300px from
  what it captioned. Screenshot a focused menu item in both themes, and on a non-white surface.

- [ ] **Step 4: Gates, commit.**

## Task A2: the one missing ring offset

`routes/control-plane/access/AccessOverview.tsx:376` uses `focus-visible:ring-offset-2` without
`ring-offset-background`, so `ring-offset-color` defaults to white — a 2px white band inside the ring
in dark mode.

- [ ] One-line fix, with a test if the contrast harness can reach it; otherwise say it rests on a
  screenshot.

## Task A3: per-cell drill has no keyboard route

`ChartTable`, `pivot/PivotTable` and object detail **all** open a drill on a mouse `onClick` with no
keyboard equivalent. #412 gave the *headers* keyboard sort and resize; the cells were left.

**This is one decision, not three patches** — the same question in three places, and #413's object
branch recorded why it is not trivial: an object-detail cell can hold a `<pre>`, an `<img>` or a
nested button, so making the cell itself a button is not automatically right.

- [ ] **Step 1: Decide the pattern once.** A roving-tabindex grid (`role="grid"` semantics) is the
  conventional answer for a table whose cells are actionable, and it avoids putting hundreds of
  elements in the tab order. Say what you chose and why, and what it costs in each of the three.

- [ ] **Step 2: If the honest conclusion is that this needs its own design pass, say so and stop
  after A1 and A2.** That is an acceptable outcome — an accessibility fix that makes navigation worse
  is not progress. Do not ship a half-pattern across three components.

- [ ] **Step 3:** Failing tests driven with real key events, then implement, then mutate.

## Task A4: live check and PR A

- [ ] Tab through a menu, a select, and a table in **both themes**. Bundle hash checked if you build.
- [ ] Open PR A. **Do NOT merge.** Say plainly which parts are product-wide rather than chart.

---

## UPDATE — what PR A found when it re-checked this section

Written after the work, in the same spirit as the pruning note above: **three of the claims in PR A's
own tasks did not survive contact with the code**, and one assumption made *while implementing* did
not survive contact with the browser. The original text is left standing so the difference between
what was believed and what was true stays legible.

**A1's site list was wrong in both directions.** `command.tsx`, `context-menu.tsx`, `menubar.tsx` and
`navigation-menu.tsx` — the four files named as suspects — **do not exist in this repo**. But the
command palette named in the prose does: it is `components/notebook/CommandPalette.tsx`, hand-rolled
on the Dialog primitive with no cmdk dependency, and it carries the identical 1.03:1 defect on its
highlighted row. So the list was four files too long and one file too short. PR A fixes the four
`dropdown-menu.tsx` items, `select.tsx`'s item, and the palette row, and replaces the file list with a
**directory sweep of `components/ui`** so the next paste-in of a shadcn menu is caught without anyone
re-checking by hand.

The palette is not merely a fourth copy — it needs a different mechanism. DOM focus never leaves its
search input; the highlighted row is a *virtual* focus driven by a `selected` index, so no focus
pseudo-class can ever match it and the indicator has to ride the same conditional as the background.

**A2 is four sites, not one.** The plan pruned this to "real but a single site, not a pattern". The
sweep PR A added finds `ring-offset-<n>` with no `ring-offset-<colour>` in **four** places:
`AccessOverview.tsx:376` (the one named), `ui/resizable.tsx:42`, and `RuleEditor.tsx` twice. The last
two are not focus rings but the *selected* state of a colour swatch, where a white hairline drawn
around a colour reads as part of the colour. All four are fixed and the invariant is now swept
repo-wide.

Worth noting where that mistake came from: `resizable.tsx` appears in the pruned list above under a
*different* claim ("a ring colour with no ring width"), which really was false. Having correctly
dismissed one claim about that file, the second one on the same line went unlooked-at.

**A1's decision: add an indicator, do not touch `--accent`.** Measured before choosing. `--accent` is
read by 63 files and ~110 utility occurrences; **55 of those are `hover:bg-accent`** and 16 more are
`hover:bg-accent/40|/50|/60`, against only 5 `focus:bg-accent`. It is the product's resting hover
background. For the background swap itself to carry 3:1 against `--popover`, light `--accent` would
have to fall from L 0.97 to **0.654** and dark from 0.27 to **0.504** — a mid-grey slab under every
mouse hover in the product, with `--accent-foreground` then failing the 4.5:1 *text* bar it does have
to meet. Metabase's own shipped stylesheet was read for the shape and agrees: its controls use
`:focus-visible { outline: 2px solid var(--mb-color-focus) }` — a dedicated focus token, with
`outline-offset: -2px` or `box-shadow: inset 0 0 0 2px` where the element must not paint outside
itself. The rule was copied; the value is egret's `--ring`.

**A3 stopped, per its stop condition.** The pattern does not come out as one decision, and the
blockers are structural rather than a matter of effort:

- **ObjectDetail is not a grid.** It is a `<dl>` of field/value rows, so `role="grid"` roving-tabindex
  is not its pattern — it would need a list/listbox pattern. "Decide the pattern once" produces at
  least two patterns, not one.
- **ChartTable's arrow keys are already owned.** `useSensor(KeyboardSensor, { coordinateGetter:
  sortableKeyboardCoordinates })` binds arrows to moving a lifted column, and dnd-kit puts
  `tabIndex=0` on every `<th>`. A `role="grid"` roving tabindex needs arrows for cell movement and
  needs the whole table to be ONE tab stop — which means dismantling the header tab stops #412
  deliberately created (`SortButton`, `ResizeHandle`, the drag activator). That is redoing #412's
  model inside a new one, not extending it.
- **PivotTable's header is not rectangular.** Multi-level headers with `colSpan`/`rowSpan`, collapse
  toggles and sort cycling; roving-tabindex coordinates are ill-defined without a real grid model.
- **The drill callback takes viewport coordinates.** `onCellDrill(ctx, x, y)` uses the mouse position
  to place the popup. Keyboard activation has none, so all three call sites need a rect-derived
  anchor, plus focus return on close — unwritten in all three.
- **A cell can contain its own interactive content** (`<pre>`, `<img>`, a "Show more" button), which
  needs the enter-cell/escape-cell sub-mode of the grid pattern, the most error-prone part of it.

A3 therefore needs its own design pass and its own PR. `ObjectDetail.tsx:123-133` already carries the
note saying so; nothing in PR A contradicts it.

**One implementation assumption that the browser refuted.** The ring was hung on `focus-visible:`
expecting that to keep it off the mouse path, since Radix moves DOM focus on pointer move. Measured in
Chrome on the built bundle: a real mousemove over a menu item leaves it matching `:focus-visible` and
the ring paints. The control run shows the discriminator works — a nav link focused by a real mouse
*click* matches `:focus` and not `:focus-visible` — it simply cannot see through Radix's script
`focus()`. The behaviour is accepted (in a Radix menu the highlighted item is the one Enter will run,
whichever device moved it there) and the wrong claim is corrected in `dropdown-menu.tsx`'s comment
rather than left standing.

---

# PR B — two chart defects

## Task B1: the trend's number overflows a narrow card

**MEASURED (#413):** 203.75px of number in a 166px card. Pre-existing — #413 pinned that its new
button wrapper contributes 0px, so it is not that PR's doing.

- [ ] **Step 1: Find out what the scalar does**, because `BigNumber` solved this exact problem with
  canvas `measureText` + `findSize`, width-driven compaction and a full-value tooltip. **Reuse it if
  it fits.** If the trend's layout makes that unworkable, say precisely why.

- [ ] **Step 2:** jsdom runs no layout and `measureText` returns zeroes, so an auto-sizer "passes" at
  whatever the loop bottoms out at. Whatever test you write must say what it does and does not prove,
  and the real check is a screenshot at the harness's 166px card width.

## Task B2: a combo's left axis title reads a hidden series' name

**Found in #412, INFERRED — confirm it reproduces before fixing it.** After a legend click hides a
series, the left axis title still names it while the axis has switched scales. Same "stale after a
legend click" family as the two aria bugs #412 fixed, which were closed by subscribing to
`legendselectchanged` in `EChartsRenderer`.

- [ ] **Step 1: Reproduce it in a browser first.** If it does not reproduce, say so and stop — a fix
  for a defect that is not there is worse than no fix.

- [ ] **Step 2: If confirmed**, check whether the `legendselectchanged` subscription #412 added can
  carry this too, rather than adding a second mechanism.

---

## UPDATE — what PR B found when it executed this section

Written after the work, in the spirit of the pruning note and PR A's update above. **Both defects
were real; the one that was only INFERRED reproduced, and understated itself.** Two points of the
brief were wrong in detail and one hazard could not have been foreseen from the plan. The original
text is left standing.

**B2 reproduced, in Chrome, before a line was written — and it is worse than the claim.** On the
deployed "MB · Combo", clicking `orders` off leaves ECharts drawing revenue alone and dropping the
left axis's tick labels *entirely* (0/100/200/300/400/500 all gone: no series on that axis, no
extent, nothing to number). The rotated title "orders" stays painted beside the empty axis, and its
`<text>` box even moved from x=573 to x=549 as the plot expanded into the space the numbers vacated.
So the plan's "the axis has switched scales" is generous: the axis has no scale at all, and the only
thing left on it is the name of a series nobody can see. This is the sixth item in this run to be
re-checked before acting on it and **the first that survived**, which is worth recording — the
discipline is not a formality that always says "already fixed".

**B1's "reuse the scalar's sizer if it fits" — it fits, and it is reused whole.** No second sizer
exists: `compactifyValue` → `findSize` → `measureText`, `maxFontSizeRem`, `SCALAR_PADDING`,
`MIN_SIZE_REM`, `SIZE_STEP_REM` and the full-value tooltip all come across unchanged, and `gridCols`
is threaded from `ChartRenderer` exactly as the scalar's is. What did NOT transfer is only what the
trend does not have: `BigNumber`'s conditional colour bands and its `label`/`sublabel`. One
structural difference had to be absorbed rather than copied — `BigNumber` has a single caption
`<div>` to measure, while the trend has two sibling captions (the period label and the comparison
line), so they are wrapped in one measured block with the outer `gap-1` restated inside it. The
spacing on screen is unchanged; without the wrapper there is no single height to subtract.

**A hazard the plan could not have named: an ECharts merge patch keyed on `name` is matched BY
name.** The first working version of B2 patched the two y-axes as an array, and ECharts'
`mappingToExists` — which resolves a patch entry to an existing component by `id` first and by
`name` second — matched the *second* patch entry to the *first* axis through its empty name.
Measured result: `["", "rev"]`, the title on the wrong side of the chart, after nothing more exotic
than toggling a legend entry off and on again. The field being patched was the field being matched
on. Both value axes now carry explicit `id`s and the patch addresses one by id.

**Two existing tests moved, and the second one is a hole in #412's coverage rather than a cost of
this change.** `sizeAware.test.ts` pins the exact number of closures a built option carries, so a
deliberate new one takes that 4 → 5. While making that change its `probe` helper turned out to
render every function's return through `String()` — which collapses *any object* to the constant
`"[object Object]"`. `__ariaFor` (#412) and `__axisNamesFor` both return objects, so both were being
"probed" into six identical characters whatever they computed, and the size-leak check that file
exists for could not have seen a leak in either. `probe` now serialises object returns.

**Found, not fixed (1).** At the harness's 166px card the trend's *comparison line* (`▲ 9.54% / vs
previous month: 22,577.58`) wraps mid-phrase across four lines. It does not overflow — every box is
inside the card — so it is not this task, and shortening it is a Metabase-parity design question
(Metabase drops to the percentage alone on a narrow card) rather than a sizing bug. Named here so it
is a decision on the record.

**Found, not fixed (2), and deliberately scoped before being reported.** The chart editor raises its
"unsaved changes" `beforeunload` prompt after ANY click inside the chart canvas — a legend toggle, or
a click on empty plot area at (700, 300) — although neither changes anything that is saved. It was
first noticed while driving the legend for B2, so the first suspicion was that this PR's
`setOption` on `legendselectchanged` caused it. **It does not:** isolated by three runs — a chart
page visited and left with NO interaction raises nothing, a legend click raises it, and a click on
bare canvas raises it too. So it is any click, not the legend, and it predates both this PR and
#412's handler. Both charts driven here were re-fetched afterwards and their `updated_at` is
unchanged (`2026-08-04T08:48Z`), so nothing was actually saved.

**Not a correction to this plan, but a trap for the next reader:** the chart and dashboard UUIDs
quoted in the earlier plan docs of this run (#412's `be2dce76…` among them) survived, but the ones
cached in the scratchpad did not — the demo data has been reseeded since, and every id in
`charts-zzparity.json` now 404s. Re-read the list from `/explore/demo/charts` rather than from a
saved fixture.

---

# PR C — three Metabase features egret does not have

All three verified absent by grep. These are **gaps, not defects** — nothing is broken, egret simply
offers less.

## Task C1: "Unpin from zero"

`graph.y_axis.unpin_from_zero` appears on Metabase's line, scatter and box plot. egret has no
equivalent — no `unpin`, `pinToZero` or `startAtZero` anywhere.

- [ ] Read Metabase's implementation before designing. **Copy the RULE, not the value** — this
  programme's most-repeated lesson, having hit six charts. Establish what it actually does to the axis
  extent, and whether ECharts expresses it as `min: "dataMin"` or something else.

- [ ] New stored key ⇒ a `KEYS` entry in `viz/cartesian-codec.ts` (`satisfies keyof CartesianConfig`)
  or it will not persist. Backend needs nothing: `chartparams.rs` validates shape only.

## Task C2: the "Compact" x-axis label mode

Metabase's "Show lines and tick marks" is a 5-way select `{Hide, Show, Compact, Rotate 45°, Rotate
90°}`. egret's is 4-way — **Compact is missing** (`ChartAxesSection.tsx` has no `compact`).

- [ ] **Note a recorded decision before you start:** a previous unit deliberately did NOT copy
  "Compact" to the **waterfall**, with the reason that it shortens TEMPORAL labels and a waterfall's
  x-axis is a category. **That reason does not generalise** — for a cartesian chart with a temporal
  x-axis it is exactly right. Establish which charts should get it and record the reason.

## Task C3: the trend's "Comparisons"

Metabase's trend Data tab carries a Comparisons list ("Previous month" + "Add comparison"). egret's
trend has no comparison control at all.

- [ ] **This is the largest of the three and may deserve its own PR.** Establish its size first — a
  comparison is a second query shape, not just a setting. If it turns out to need backend work, stop
  and report rather than expanding scope; `chartparams.rs` is permissive but the *query* may not be.

---

---

## UPDATE — what PR C found when it executed this section

Written after the work, in the spirit of the pruning note and PR A's and PR B's updates above. **PR C
ships C1 and C2 and STOPS BEFORE C3**, and the reason is not the one this plan predicted. Two points
of the brief needed correcting, one recorded decision was overturned by measurement, and the two
test files an interrupted attempt had left on the branch were kept — but one of them had a fixture
that never reached the path it was asserting about. The original text is left standing.

**Both pre-existing test files were KEPT, and their Metabase citations are all verbatim.** Every
line number in their headers was re-checked against the sibling checkout and every one is exact:
`option/axis.ts:380` really is `scale: !!settings["graph.y_axis.unpin_from_zero"]` and is the whole
implementation; `model/axis.ts:716/778/856` really do say `compact: settings[...] === "compact"`
three times; `cartesian-chart.ts:245-260` and `:242` are verbatim; `formatting/date.tsx:714-718` is
the `compact || date_abbreviate` swap. Both files also failed for the right reason before a line was
written — 18 failures across the two, none of them an import error dressed up as an assertion,
except the five `unpinFromZeroApplies` cases which correctly reported `TypeError: … is not a
function` for a function that did not exist.

**One assertion in them was wrong, and it was wrong in the way this programme keeps being wrong: a
precondition nobody measured.** The box plot's fixture was two short category names on a 166x184
card, at which `verticalValueAxisFit` finds room for its five ticks and DECLINES — so no `min` is
written at all and `expect(pinned.min).toBe(0)` read `undefined`. That is a fixture failing to reach
the measured path, not a chart that is pinned; the file's own header warns about exactly this class
of gap on the mocking side and then fell into it on the fixture side. Corrected to four names, which
is the shape `options/boxplot.ts` measured its own defect on, and the pinned arm now asserts
`toBeDefined()` before asserting the value so the two arms cannot agree at `undefined`.

**The unpin flag does nothing but set ECharts' `scale`, and copying only that would have shipped a
control that saves and does not paint.** Metabase computes no number: `scale: false` (ECharts'
default) forces a value axis to include the origin, `true` lets it fit the data. But egret's builders
do not leave the extent to ECharts — on a measured card `verticalValueAxisFit` writes an explicit
`min`/`max`, and **an explicit `min` outranks `scale` outright**. Both extent seeds therefore move
with the flag (`axisExtent` in `options/cartesian.ts`, `axisLo`/`axisHi` in `options/boxplot.ts`).
Under jsdom `measure-text` returns zeroes by design, so an unmocked suite cannot see that half at
all — which is why both new test files mock it with zrender's own estimator, and why the mutation
"seed at the origin even when unpinned" is a mutation worth having.

**Measured in Metabase's own browser at :3033 before designing, and again after**: "MB · Line"
pinned draws `0 / 5,000 / 10,000 / 15,000 / 20,000 / 25,000`; the same card unpinned draws `9,000 /
12,000 / 15,000 / 18,000 / 21,000 / 24,000 / 27,000`. The presence matrix was driven rather than
read: **Area hidden, Bar hidden, Combo hidden** (its default second series is a bar), **Waterfall
hidden, Boxplot shown and unchecked, Scatter shown and ALREADY CHECKED** — a scatter is unpinned out
of the box and everything else is pinned. Metabase's panel puts the row immediately under "Auto
y-axis range", and egret's now does too. Nothing was saved; every change was made on an ad-hoc
(unsaved) question.

### C2's recorded decision was RIGHT ABOUT THE BOX PLOT AND WRONG ABOUT THE WATERFALL

This is the plan's own Task C2 instruction — "a previous unit deliberately did NOT copy Compact to
the waterfall … **That reason does not generalise**" — confirmed, and it is worth writing down which
half of it survived so nobody re-derives this.

**The recorded reason, verbatim**, from `settings/ChartAxesSection.tsx`'s section doc and repeated in
`CartesianDisplaySection.tsx`: Compact "shortens temporal axis labels, and these charts' x is a
category run through its column format, so the option would do nothing here". One sentence, written
once, covering the TWO charts that share that Axes tab.

**It is true of the box plot.** `options/boxplot.ts` gives its category axis **no
`axisLabel.formatter` at all** — it draws the raw group key and says so. Compact is a formatting flag;
with no formatter there is nothing for it to act through, and offering it there would be exactly the
inert control this programme keeps deleting. That chart still does not get it.

**It is false of the waterfall, and the falsifier is structural rather than a matter of degree.**
`options/waterfall.ts:234-242` carries `bucketedX = namesAPeriod(cfg.xAxisUnit)` and a `drawLabel`
whose second arm is `periodName(cfg.xAxisUnit, v)` — **the same two arms, calling the same module, as
the cartesian builder's `drawLabel`**. That is not an accident of implementation: the waterfall's own
comment says the shape is cartesian's "same three arms in the same order, and deliberately not a
second idiom". So a month-bucketed waterfall draws "January 2026" TODAY, which is a temporal label,
which is precisely the thing Compact shortens. The premise "this chart's x is a category run through
its column format" describes arm 1 and silently omits arm 2, which is the arm that matters.

The general lesson, since this is the third time in this run a single recorded reason has been
applied wider than it was measured: **the reason was written about the tab, and the tab has two
charts that differ on the fact it asserts.** Compact is therefore a PROP now (`offerCompact`), not a
fact about the tab, so the next reader cannot infer the withheld half from the offered one. The same
split runs the other way for "Unpin from zero" — the box plot has it and the waterfall does not,
because `isYAxisUnpinFromZeroValid` excludes a waterfall display by name. Two charts, two settings,
each chart getting exactly one of them.

**Cartesian's own deferral was wrong for a third and different reason.** It read: Compact is
deferred because "cartesian is the one family with a `timeseries` x-scale, which is exactly where a
temporal shortener would bite, but that axis is `scaleAxis`, whose tick labels ECharts generates".
Both clauses are true; the conclusion is not. The ORDINAL x — the default, and what every bucketed
temporal chart in the product actually draws — goes through `drawLabel`, which is where Compact acts.
The deferral was scoped to the one x-scale Compact cannot reach and then applied to all four.
`scaleAxis` remains out of reach and is now GREYED there, on the same `valueX` fact the rotations are
already greyed for, rather than withheld from every chart.

**Compact is one Metabase flag and TWO egret fields.** `formatValue`'s `compact` upstream reaches
both a date's month token and a number's magnitude suffix; egret spells those as `date_abbreviate`
and `compact` on `ColumnFormat`. Setting only `compact` would have made a compact DATE axis a silent
no-op — on the very axis the feature is named for. The mutation run pins both halves separately.

**A trap the plan could not have named: `specifiesDateRendering` must be asked about the STORED
format, never the compact-augmented one.** Compact adds `date_abbreviate`, which is one of the seven
fields that function consults — so asking it the augmented format makes it true on every bucketed
axis, handing the label to `formatDate`'s DEFAULTS and drawing "Jan 1, 2026" where "Jan 2026" was
wanted. Compact is a modifier on whatever wins; it is not itself a reason for the column format to
outrank the period name. It has its own mutation.

**A correction to this plan's own instructions.** The brief says a new stored key needs "a `KEYS`
entry in `viz/cartesian-codec.ts` … Backend needs nothing." Both halves are true and the first is
**necessary but not sufficient**: the BUILDER configs (`CartesianBuildConfig`,
`WaterfallBuildConfig`, `BoxplotBuildConfig`) are separate interfaces from the API-facing
`CartesianConfig`/`WaterfallConfig`/`BoxplotConfig`, and a key added only to the latter type-checks
in the codec and never reaches the chart. `tsc` caught it (7 errors in 3 files), which is the system
working — but a codec-only change would have persisted the setting and drawn nothing. The waterfall
and box plot need no codec entry at all: `fieldDescriptor` copies every key under a prefix
generically, so only the cartesian family has a hand-maintained list.

### C3: THE STOP CONDITION DID NOT FIRE, AND I STOPPED ANYWAY ON A DIFFERENT GROUND

**This distinction is the useful part of this section, so it is stated before the numbers.** The
plan's stop condition is: *"a comparison is a **second query shape**, not just a setting … If it
turns out to need backend work, stop and report rather than expanding scope; `chartparams.rs`
validates shape only and is permissive, but the *query* path may not be."*

**That risk does not exist.** It was a reasonable guess and it is wrong, in both halves:

- **`chartparams.rs` accepts it**, as the plan expected — it is shape-permissive on `settings` by
  explicit design, with a test locking exactly that in
  (`accepts_settings_keys_this_build_does_not_know`).
- **The query path accepts it too, because there is no second query.** `computeTrend` upstream is a
  **pure function of the rows the card already fetched**. Per type: `previousValue` walks back to the
  previous non-empty ROW; `previousPeriod` is the same scan with `dateUnitsAgo = 1`; `periodsAgo` the
  same with `dateUnitsAgo = N`; `staticNumber` reads only the setting; `anotherColumn` reads a second
  column of the SAME row. Nothing issues a request. egret's trend already fetches the whole grouped
  series — `dimsAndMetrics` sets `row_limit` (default 1000) and there is **no trend-specific LIMIT
  anywhere** — so every row a comparison needs is already in the browser.

So the plan's premise is false for four of the five types outright, and true for `anotherColumn` only
in the weak sense that it adds a COLUMN to the existing query — which is the pattern `goalColumn`
already uses at `viz/simple.tsx:283-286`. **No backend work. No query change. No new query shape.**

**I stopped anyway, on size and reviewability, and that is a judgement rather than a rule firing.**
The measured cost, so whoever picks C3 up starts from a number instead of the plan's guess:

| piece | cost |
|---|---|
| backend | **none** — `chartparams.rs` unchanged |
| query / spec | **none** — the rows are already fetched |
| new stored key | one, `smartscalar.comparisons`, an array of tagged objects; persists free via `fieldDescriptor` |
| compute | one new module porting `SmartScalar/compute.ts:341-655`. **The only real semantic risk in the whole feature** is `areDatesTheSame`'s timezone-offset guard — the same class of bug `parseInOwnZone` exists for in `temporal-period.ts` |
| caption rendering | small; the trend already prints one comparison line |
| **settings UI** | **the dominant cost.** Metabase spends six components across nine files on the comparison picker (add/remove/reorder rows, a type select, and a different second control per type) against roughly two files for all of the arithmetic |
| **total** | **5–7 frontend files**, of which the UI is the majority |

Landing that inside a PR whose other two units are axis settings would make neither reviewable, and
the C3 half would dominate the diff of a PR named for the other two.

**The cheapest honest subset**, if a follow-up wants one: the list model plus `staticNumber` +
`previousValue` + `previousPeriod` behind a plain per-row `<Select>` — about four files and one new
module, still no backend and no query change. `periodsAgo` and `anotherColumn` are the two that add
UI without adding arithmetic.

**Found while sizing C3, not fixed (1), and it is a correctness gap rather than a missing feature.**
egret's trend takes `valid[n - 1]` and `valid[n - 2]` — the previous non-empty ROW, which is
Metabase's `previousValue` — and then captions it "vs previous month", which is Metabase's
`previousPeriod`. Those are two different comparisons upstream, computed by two different functions,
and Metabase separates them precisely because a gap in the series makes the row-based one lie about
which period it compared. On a series with no gaps the two agree, which is why this has never been
seen. It belongs with C3 and is named here so it is on the record.

**Found and FIXED, after first being filed under the wrong heading — and the misfiling is the part
worth recording.** The first draft of this PR gave the scatter Metabase's unpinned-by-default in the
BUILDER and withheld the control from the PANEL, then wrote that up as a note for whoever picks up
C3. It is not a C3 note. It was **a behaviour change this PR introduced with no control to reverse
it**: every saved metric-vs-metric scatter went from pinned (`scatterXY` wrote no `scale`, so
ECharts' default of `false` applied) to unpinned, and no user could pin it back. That is
"implemented and unreachable" — the family #414 exists for — and it was one sentence away from
shipping because it had been written down as a limitation instead of recognised as a defect.

**How it happened is worth more than what it was.** The panel gate read `kind !== "scatter"`, and the
justification quoted `CartesianDisplaySection.tsx:265-266`'s standing divergence: the
metric-vs-metric scatter leaves through `scatterXY`, which reads none of this panel's Y value-axis
keys. That was TRUE, and this PR is what made it false — `scatterXY` now reads `unpinFromZero`. The
exception was carved into the builder and the panel went on quoting the rule the exception had just
broken. **A divergence note is a fact with a date on it, not an invariant**, and this is the second
time in this PR that a recorded reason was applied past the point where it stopped holding (the
waterfall's Compact exclusion was the first).

The fix is the gate plus two things the gate alone would have got wrong:

- the switch reads the **resolved** value (`unpinFromZeroOn`), not the stored key — otherwise a
  scatter with nothing stored draws the switch OFF beneath a chart that is visibly unpinned, which is
  a control that lies rather than a cosmetic slip;
- the patch is **default-aware** — "off" on a scatter must persist an explicit `false`, because
  `undefined` means "take the default" and the default there is ON, so the switch would spring back
  and the pin would be unsavable. Every other key in this panel keeps the same "store only what
  differs from the default" rule; this is that rule with a default that is not always `false`.

The divergence note still holds for `yMin`, `yMax`, `yTickCount` and `yAxisTicks`, which `scatterXY`
genuinely does ignore. Three mutations cover the fix, including re-introducing the exact defect.

**One live observation that is NOT a defect, checked rather than assumed.** On the deployed
"MB · Scatter" the toggle flips but the axis does not move: the rating column runs **0 to 5 with 24
rows at exactly 0**, so the extent already reaches the origin and `scale` has nothing to lift. The
same shape as the box plot's non-movement, reached for a different reason — there ECharts'
nice-rounding lands on 0, here the data does. Both were confirmed against the number rather than
explained away.

---

## UPDATE — C3 executed, and the caption defect it was sized alongside

Written after the work, in the spirit of the pruning note and the three updates above. **C3 ships,
in the "cheapest honest subset" its own sizing note proposed plus one type, and the correctness gap
that section filed under "found, not fixed" is fixed with it** — because it turned out not to be a
separate item at all. The original text is left standing.

### The caption that lied, and which way it was fixed

C3's sizing note filed this as *"egret's trend takes `valid[n - 1]` and `valid[n - 2]` — the previous
non-empty ROW, which is Metabase's `previousValue` — and then captions it 'vs previous month', which
is Metabase's `previousPeriod`."* That is exactly right, and it leaves open which of the two moves.
They are different products, so the brief for this unit required the answer to come from Metabase's
behaviour rather than from taste.

**THE COMPUTATION MOVES. THE CAPTION STAYS.** Three independent readings, all agreeing:

1. **Driven at :3033**, on an ad-hoc Orders/Count-by-month question filtered to `Nov 1 – Dec 31 2028
   OR Apr 1–30 2029` so that January to March are missing, carrying one `previousPeriod` and one
   `previousValue` comparison. Verbatim:

   ```text
   344
   Apr 2029
   N/A       vs. previous month: (No data)
   ↓ 37.45%  vs. Dec 2028: 550
   ```

   `previousPeriod` refuses to reach past the hole and says so. `previousValue` reaches the previous
   ROW and then names the period it landed on — "Dec 2028", never "previous month". **Neither is
   what egret drew**, which was `▼ 37.45% vs previous month: 550`.

2. **Metabase's own unit spec**, both arms: `SmartScalar/compute.unit.spec.ts:487-505` expects
   `changeType: "missing"` for a `previousPeriod` across a two-year gap, and `:250-271` expects
   `dateStr: "Sep"` for a `previousValue` across a missing October.

3. **`getDefaultComparison` (`SmartScalar/utils.ts`) settles which one egret's default should be**,
   and this is the argument that makes it a fix rather than a swap: a series **with** a date unit
   defaults to `previousPeriod`; one **without** defaults to `previousValue`. egret's trend prints
   "vs previous month" exactly when it has a unit — so the caption it already shipped is the caption
   Metabase's default produces. Making the caption honest about the row walk instead would have
   moved egret's default OFF Metabase's default and lost the more useful of the two comparisons.

So the arithmetic is now the search Metabase's caption describes, and `previousValue` is available
as a type in its own right, captioned with the period it actually reached.

**Two consequences that are behaviour changes, both driven before being adopted.** A trend over a
single period used to draw the number alone; it now draws `N/A / vs previous month: (No data)`,
which is what :3033 renders for the same query (checked separately, on a one-month filter). And a
trend with no unit used to say "vs previous"; it now names the row — "vs Jan: 100". Silence is the
weaker answer in the first case: a card that omits the comparison reads as though none was
configured, which is the ambiguity this whole unit exists to remove.

### What shipped, and the one type that did not

**Four of the five comparison types**: `previousPeriod`, `periodsAgo`, `previousValue`,
`staticNumber`. The list model, Metabase's cap of three, its validity rules and its default rule all
come across.

**`anotherColumn` is left out, and the line is sharper than the plan's.** C3's sizing note said the
type "only adds a COLUMN to the existing query — which is the pattern `goalColumn` already uses". True,
and it is still the only one of the five **that touches the query at all**: the trend's spec is
`dimsAndMetrics([c.dimension], [c.metric], s)`, exactly one metric by construction. So the boundary
drawn here is *everything that is a pure function of rows already fetched ships; the one that needs a
second column in the spec does not.*

**And Metabase agrees, which was not predicted.** Driving the type menu on a Count-by-month trend at
:3033 shows exactly `Previous month / _ months ago / Previous value / Custom value…` — no fifth
entry, because `getComparisonOptions` only offers `anotherColumn` when `getColumnsForComparison`
finds a numeric column that is not the primary field. On every trend egret can currently build,
there is none. **The four shipped here are Metabase's own list for this chart**, not a subset of it.

**`periodsAgo` was added beyond the note's proposed subset** because its cost turned out to be
marginal, not because the subset was wrong: it is `previousPeriod`'s computation with `back = N`
(upstream serves both from one function reading `value ?? 1`), and both of its caption arms were
needed anyway — the "previous month" arm by `previousPeriod` and the dated arm by `previousValue`.
The only new surface is one integer input.

**Reordering by drag is not here.** Metabase's rows carry a handle; with a cap of three, delete-and-
re-add reaches every ordering, and a dnd-kit sortable is a bigger dependency than the ordering buys.

### The sizing note's numbers, checked against what it cost

| piece | predicted | actual |
|---|---|---|
| backend | none | **none** — `chartparams.rs` untouched, confirmed by reading its `accepts_settings_keys_this_build_does_not_know` test |
| query / spec | none | **none** |
| new stored key | one, persists free via `fieldDescriptor` | **correct, and the plan's own codec warning does not apply here** — `viz/cartesian-codec.ts` exists because the cartesian descriptor keeps a hand-maintained list; `fieldDescriptor` copies every key under a prefix generically, so `smartscalar.comparisons` needed no entry and `tsc` found nothing |
| compute | one module; `areDatesTheSame`'s timezone guard the only real risk | **correct on both counts** — one module, and that guard was the one place the port needed its own reasoning |
| settings UI | dominant cost, six components across nine files upstream | **one component, ~160 lines** — upstream's count is Mantine menu plumbing (`PeriodsAgoMenuOption`, `MenuItem.styled`, two form components) that a shadcn `Select` collapses |
| total | 5–7 frontend files | **7 source files + 3 test files** |

### A hazard the plan could not have named: `dayjs` accessors are plugins

`areDatesTheSame`'s offset guard reads the unit's own number off the date — `target[dateUnit]()`
upstream. **`.week()` is not core dayjs and `.quarter()` is not either.** Upstream gets both from a
global registration; egret has none, so a week-bucketed trend would have thrown inside a render
rather than returned a wrong answer. It is a `switch` here rather than an index, so a unit with no
accessor is a compile error, and the module registers `weekOfYear` itself.

One upstream quirk is carried verbatim with its reason written out: **dayjs's `.day()` is the day of
the WEEK**, not of the month. It is sufficient — the guard only runs after `diff(…, "day") === 0`,
which already bounds the two instants to under 24 hours apart — and `.date()` would read more
obviously but would be a silent divergence from the code this is a port of.

### Mutation: 25 of 29 killed, and the four survivors are argued rather than reported

The harness journals file bytes before the first write, scores only on the child process exit code,
requires every search string to match **exactly once**, refuses to score a run that executed no tests
or far fewer than the baseline, restores on every signal path, and ends with a byte-for-byte journal
comparison plus a clean full-suite run. **Its own first run was refused by its empty-run guard** —
vitest colours its summary, so the count regex matched nothing — which is the guard working on the
harness rather than on the code.

One survivor was a **real gap**: deleting `areDatesTheSame`'s coarse `diff !== 0` check survived,
because `.month()` is 0–11 and February 2028 and February 2029 both report 1 — so with only the
unit-number guard, a February two years back is accepted as a March's "previous month". Two tests
now pin it from both directions.

**Three are equivalent mutants, with the argument rather than an apology:**

- **the early bail and the search-window bound** are both pure short-circuits. The rows arrive in
  ascending dimension order (the trend descriptor derives it and refuses `shared.orderby` so it
  cannot be overridden) and a GROUP BY makes the dimension unique, so once a candidate is older than
  the target no earlier row can match either. Removing either cannot change an answer. They are kept
  because upstream keeps them and because a linear scan of a thousand-row series inside a render is
  a real cost — but **no assertion should be written to pin them**, because an assertion that cannot
  fail is worse than none.
- **`comparableUnit`'s `namesAPeriod` gate** overlaps the seven-name list beside it, and neither is
  alone — the same shape `periodName` already records about its own pair of guards. What the gate
  buys is the DIRECTION of any future drift, which the list cannot express because it must be a
  literal tuple to give `ComparableUnit` its type.
- **`.day()` → `.date()`** is the expected survivor described above.

### A comment this PR's own diff made stale, in four places

The programme's rule that *a divergence note is a fact with a date on it* caught one here. Four
sites asserted that `SmartScalarChart` reads `valid[n - 1]` and `valid[n - 2]` — `viz/orderby.ts`,
`viz/simple.tsx`, and `viz/__tests__/orderby.test.ts` twice — as the justification for the trend
being the one descriptor that ignores `shared.orderby`. **This PR is what made that false.** The
conclusion survives and is in fact stronger: the backwards search stops at the first candidate older
than the target, which is sound *only* under ascending order, so the ordering went from an incidental
property of "the last two rows" to a stated precondition. All four now say the true thing.

`temporal-period.ts`'s "what both callers need it for" was stale in the same way — there is a third
kind of caller now, asking whether a period can be SUBTRACTED rather than rendered.

### Found, not fixed

**The narrow-card comparison line, now up to three of them.** PR B recorded that at the harness's
166px card the single comparison line already wraps across four lines without overflowing. A card
may now carry three, and the caption block is measured, so the big number shrinks to make room —
correct, but small. Metabase solves this with `getValueHeight`, which drops to showing ONE comparison
when they do not fit and moves the rest into a tooltip. That is a size-dependent rule, so **jsdom
cannot test it at all** (`measureText` returns zeroes and every candidate "fits"), and it is a
design decision of the same family as PR B's own deferred "shorten the comparison line on a narrow
card". Named so it is on the record rather than rediscovered.

**egret's zero-baseline percentage is a deliberate retained divergence.** Upstream's `computeChange`
returns ±Infinity when the value compared against is 0 and draws "∞%"; egret has always drawn the
absolute change instead. Porting it would change what every already-saved card with a zero baseline
draws, for no gain to this task.

**The `beforeunload`-on-any-canvas-click defect PR B reported is still live**, and was hit again
while driving the studio here. Not this unit's.

---

## Out of scope, deliberately

The two shadcn generations (solid `ring-2 ring-ring ring-offset-2` versus two-part `border-ring` +
`ring-ring/50` + `ring-[3px]`) are worth unifying and that is its own work with its own review.

Housekeeping the user has not yet ruled on: the `egret-pg-test` container, and the stray `.png` files
in the repo parent.

## Self-review

- **Five items were pruned from this list by re-reading the code**, two of them from a live
  panel-parity sweep and one from a PR's own "found, not fixed" section. **A finding relayed without
  re-verification is a rumour**, however careful the source. Every item above was re-checked; do the
  same to anything this plan hands you.
- **PR A Task A3 has an explicit stop condition**, because a half-implemented keyboard pattern across
  three components is worse than none.
- **PR C Task C3 has one too**, for size.
- **Known soft spot:** Task A1's choice between "add a ring" and "darken `--accent`" is a genuine
  design decision I have not made for you, and the blast radii differ by an order of magnitude.
  Establish what else reads `--accent` before touching it.
