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
