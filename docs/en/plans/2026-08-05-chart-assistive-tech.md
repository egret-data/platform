# What the charts tell assistive tech — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Stop two chart surfaces excluding keyboard users, and stop three aria descriptions saying
things that are not true.

**Architecture:** Two independent halves. **Tasks 1–3 are reachability** — controls that advertise
themselves and cannot be operated. **Tasks 4–6 are truthfulness** — descriptions that name data the
chart is not drawing. Task 7 is a design token and is deliberately last, because its blast radius is
the whole product rather than the charts.

**Tech stack:** React 18, tanstack table, ECharts 6.1, Tailwind v4 with OKLCH tokens, vitest.

---

## Where these came from

A three-way audit at `1d9b0d30`. Findings marked MEASURED were driven or executed; the rest were read.
The audit also established the counter-evidence, which matters for scoping: **198 of 199 interactive
elements across 24 rendered components already have an accessible name**, both in-scope `@dnd-kit`
contexts already have a `KeyboardSensor`, and every echarts family already emits a description. This
is a short list of specific holes, not a general accessibility problem.

---

# Part one — reachability

## Task 1: `ChartTable`'s column resize is mouse-only

**MEASURED.** `ChartTable.tsx:358-368` renders `role="separator"` + `aria-label="Resize column"` with
only `onMouseDown` / `onTouchStart`. Measured `tabindex: null`, no key handler, and no numeric width
control anywhere in the panel. **It advertises itself to assistive tech as a control and cannot be
operated.**

**The fix already exists in this repo.** `pivot/PivotTable.tsx:12-51` is a complete `ResizeHandle`
component: `tabIndex={0}`, ArrowLeft / ArrowRight against `KEY_RESIZE_STEP`, clamped to
`MIN_COL_WIDTH`, `focus-visible:bg-primary/60`, and a `getWidth` callback because a stored width may
be absent when the column is auto-sized. `ChartTable` was simply left behind.

- [ ] **Step 1: Write the failing test** — focus the handle, press ArrowRight, assert the emitted
  width grew by one step. Watch it fail and record the text.

- [ ] **Step 2: Extract and share `ResizeHandle`.** Do NOT copy it — two implementations of one
  control is how this programme ended up with four per-series settings across three parallel lists.
  Move it somewhere both tables can import from and say where.

- [ ] **Step 3: Name each handle for its column.** `"Resize column"` is the same string on every
  handle, so a screen-reader user hears N identical controls. The pivot takes a `label` prop already;
  pass the column's name.

- [ ] **Step 4: Check the pointer semantics still hold.** `ChartTable`'s current handle carries
  `onPointerDown`/`onClick` stopPropagation because **a header click cycles the sort and a drag must
  not** — the pivot's version documents the same concern at its `onPointerDown`. Whichever handler
  set survives, that guarantee must survive with it. Assert it.

- [ ] **Step 5: Gates, commit.**

## Task 2: the table header says "sortable" and Enter starts a drag

**MEASURED.** `ChartTable.tsx:338-357`, header component at `:23-47`. The `<th>` has `tabindex="0"`
and `aria-roledescription="sortable"`, and `aria-sort` is absent. Measured: Enter leaves the text
unchanged and sets `opacity: 0.6` — a dnd-kit lift. A mouse click sorts. **Sorting has no keyboard
route at all**; `SortRows.tsx` edits the query's `orderby` and exists only in the builder.

`ChartTable.tsx:285-289` documents the drag/sort collision as intentional. It does **not** record that
sorting becomes unreachable, which is the actual cost.

- [ ] **Step 1: Write the failing test** — focus the header, press Enter, assert the sort state
  changed. Watch it fail.

- [ ] **Step 2: Give sorting its own control.** A real `<button>` inside the `<th>` is the
  conventional answer: the header keeps the drag, the button takes Enter/Space. **Add `aria-sort` on
  the `<th>`** — currently the direction is carried only by a `" ▲"`/`" ▼"` glyph, which is invisible
  to a screen reader.

- [ ] **Step 3: Update the comment at `:285-289`** to say what the collision costs and how it is now
  resolved. A comment that states a checked fact which is no longer true is worse than none — #411
  found one asserting Metabase's gauge has no drill when it does.

- [ ] **Step 4: Gates, commit.**

## Task 3: the pivot's row-dimension header cannot be focused at all

**MEASURED.** `pivot/PivotTable.tsx:154-174` is a `<th onClick>` with no `tabIndex`, no `onKeyDown`
and no `aria-sort`. Measured: `th.focus()` does not move `activeElement`, and Enter fires the handler
zero times. Strictly worse than Task 2 — there is not even a wrong keyboard behaviour, there is none.

- [ ] **Step 1: Failing test, then the same treatment as Task 2**, reusing whatever Task 2 built.

- [ ] **Step 2: Gates, commit.**

---

# Part two — truthfulness

## Task 4: the pie's aria description names slices the chart is not drawing

**MEASURED, and independently re-verified by the auditor from pristine source.** The description is
built from `data = effectiveRings[0]` (`echarts/options/pie.ts:1697` → `:2063-2066`), while what is
drawn comes from `rootSlices = pieRootSlices(...)` (`:1805` → `seriesData` at `:1999`), which filters
`toggledOffSlices` (`:1393-1396`).

With `toggledOffSlices: ["US"]` the description reads *"3 values: US 30, APAC 20, EU 10"* while ring 0
draws `["APAC", "EU"]`. **Both the count and the enumeration are false.**

The saved `hiddenSlices` curation is handled correctly — the auditor's mutations on that path were
killed. It is the **ephemeral legend click** that aria never learns about.

- [ ] **Step 1: Write the failing test** — a pie with a toggled-off slice; assert the description's
  count and its enumeration. Watch it fail on both.

- [ ] **Step 2: Build the aria points from `rootSlices`.**

- [ ] **Step 3: Check the divergence-note path at `options/pie.ts:2067-2069` is undisturbed.** The
  auditor explicitly flagged that they had not verified this. Do verify it.

- [ ] **Step 4: Gates, commit.**

## Task 5: the scatter's aria reads `NaN` for every y

**MEASURED (live, during #411).** The description comes out `"15.69 NaN, 19.87 NaN, …"`. Pre-existing
and unrelated to #411's change — that card stores no `xScale`, so `resolveXScale` short-circuits
before any of the new code.

- [ ] **Step 1: Find where the y is read.** A scatter datum is an `[x, y]` pair, and the cartesian
  aria path has been changed twice recently (`datumY` was introduced in #397 for exactly this class of
  unwrapping). Establish whether the aria builder is missing that unwrap or reading a different field.

- [ ] **Step 2: Failing test, fix, mutate.** The mutation that matters: read `v[0]` instead of `v[1]`.
  On a scatter both are numbers, so **the fixture must use x and y values that cannot be confused** —
  the same trap #397 recorded, where a 13-digit epoch made the two fail in opposite directions.

- [ ] **Step 3: Gates, commit.**

## Task 6: the cartesian aria goes stale when a legend entry is clicked

**INFERRED — the auditor could not measure this without a browser, so confirm it before fixing it.**
`options/cartesian.ts:1684-1686` sets echarts' own `legend`; `:1713-1730` bakes `seriesCount` and the
values into a **static string at build time**; `EChartsRenderer.tsx` subscribes to `click`,
`mouseover` and `mouseout` — never `legendselectchanged`. Same shape as Task 4, one family over.

- [ ] **Step 1: Confirm it in a browser first.** If it does not reproduce, say so and stop — a fix for
  a defect that isn't there is worse than no fix.

- [ ] **Step 2: If confirmed**, decide whether the honest fix is to re-derive the description on
  `legendselectchanged` or to stop enumerating values that a legend can hide. Say which and why.

- [ ] **Step 3: Gates, commit.**

---

## Task 7: the light-theme focus ring fails its contrast bar

**MEASURED by the auditor**, by converting the OKLCH tokens to sRGB and compositing alpha numerically
— deliberately avoiding the "a regex reads `oklch()`'s three numbers as RGB" trap this programme has
hit before.

| | light | dark |
|---|---|---|
| `--ring` solid (`ring-ring`, `border-ring`) | **2.58:1 FAIL** | 4.08:1 pass |
| `ring-ring/50` (Button, Input, Select) | **1.54:1 FAIL** | **1.83:1 FAIL** |
| global `* { outline-ring/50 }` fallback (`index.css:246`) | **1.54:1 FAIL** | **1.83:1 FAIL** |

**The bar here is 3:1, not 4.5** — this is a non-text focus indicator (WCAG 1.4.11 / 2.4.11). State
that explicitly rather than quietly failing the text bar. Light `--ring` is `oklch(0.71 0.01 286.09)`
at `index.css:114`; the dark override is at `:165`.

- [ ] **Step 1: Re-derive the numbers yourself before changing anything.** This is a token change that
  reaches every focusable element in the product, so it does not go in on a second-hand measurement.
  Paint to a 1×1 canvas and read it back, or convert numerically — say which.

- [ ] **Step 2: Establish the blast radius.** Grep for `ring-ring`, `border-ring`, `outline-ring` and
  the global fallback. Report how many call sites and which surfaces outside the charts are affected.
  **If this turns out to be a product-wide change rather than a chart one, say so and stop** — it may
  belong in its own PR with a different reviewer.

- [ ] **Step 3: If proceeding**, darken light `--ring` (the auditor's estimate was roughly
  `oklch(0.63 …)`, unverified) and stop halving its alpha on focus. Re-measure both themes.

- [ ] **Step 4: One thing the auditor could not settle**, which needs a browser: whether Chrome's
  `outline-style: auto` honours an author `outline-color` at all. If it does not, the global fallback
  needs a different fix and saying so is the finding.

- [ ] **Step 5: Gates, commit.**

---

## Task 8: live check and the PR

- [ ] **Step 1: Gates**, then drive it. If you build, **check the loaded bundle hash** against
  `ls -t frontend/dist/assets/index-*.js` — the scratchpad `serve.py` serves `index.html` with no cache
  headers, so the first reload after a rebuild keeps the OLD bundle.

- [ ] **Step 2: Drive every keyboard path with the keyboard**, not with `.focus()` + a synthetic
  event. Tab to it, press the key, read the result.

- [ ] **Step 3: Read the aria descriptions out of the live DOM** after toggling a legend entry, for
  Tasks 4 and 6.

- [ ] **Step 4:** the 19 "ZZ Chart Parity" charts, no regressions.

- [ ] **Step 5: Open the PR. Do NOT merge.**

---

## Out of scope, deliberately

`onPointClick` is still missing on funnel, gauge, progress, smartscalar and object, and a comment at
`ChartRenderer.tsx:319` asserts Metabase's gauge has no drill when `Gauge.tsx:186-188` shows it does.
That is a **separate PR** — each of those five needs a decision about what clicking it filters by, and
this programme's per-chart drill shapes have been individually argued (sankey edges filter both
columns; boxplot reads `dataIndex`; map filters the raw dimension, never the normalised geojson key;
waterfall refuses on its Total bar). Squeezing five such decisions into an accessibility PR would
bury them.

Also out: the dead `view_as`/`link_text`/`link_url` trio on 17 of 18 viz types; the four mutation
survivors; and the Metabase-side gaps.

## Self-review

- **Tasks 1–3 share a control and should share an implementation.** If Task 1's extraction is done
  well, Tasks 2 and 3 get cheaper; if it is done as a copy, they get more expensive and the programme
  repeats a mistake it has already made twice.
- **Task 6 is the only one starting from an inferred finding**, which is why its first step is
  confirmation and its stopping condition is explicit.
- **Task 7 may not belong in this PR at all**, and Step 2 is there to find that out early rather than
  after the diff is written.
- **Known soft spot:** Task 5 assumes the scatter aria bug is an unwrapping error. It might instead be
  that the scatter's aria path was written for a category axis and never updated — in which case the
  fix is larger than one field access. Establish which before writing it.

---

## What executing it corrected (added after the work, PR #412)

Every finding held. Five points of the plan did not, and are recorded here rather than quietly
fixed in the diff.

1. **Task 1's extraction could not be a straight move.** The plan reads as though `ChartTable` only
   had to adopt the pivot's `ResizeHandle`. It could not adopt its *drag*: tanstack's
   `getResizeHandler` is what drives `columnSizingInfo`, and that is what `ChartTable`'s
   persist-on-gesture-end effect watches. Replacing it with the pivot's pointer loop would have made
   every drag either silently unsaved or saved on every `pointermove`. The shared component takes a
   `dragHandlers` prop instead; the keyboard half — the actual defect — is shared outright.

2. **Task 1 also needed a commit path of its own.** An arrow-key resize is a gesture with no drag
   around it, so `isResizingColumn` never moves and the effect never fires. Without that, the width
   changed on screen and was lost on the next render from the parent.

3. **Task 5's known soft spot was half right, and there was a second defect underneath it.** The
   unwrap hypothesis was correct — `datumY` is the fix. But `applyXScale` also FILTERS: a datum whose
   x will not parse is dropped, so `series.data` is not index-aligned with `drawnCategories`, and the
   zip paired each survivor with the *next* label's value. That does not read as `NaN`; it reads as a
   plausible sentence, which is why it outlived the visible defect. Reachable in production —
   `Date.parse` refuses Trino's region-zone timestamp rendering (`… 00:00:00.000 Asia/Ho_Chi_Minh`).

4. **Task 6 reproduced exactly as inferred.** Measured on the deployed "MB · Combo": with `orders`
   clicked off, the chart drew revenue alone and the container still announced *"Combo chart of
   orders … 2 series; January 2026 205, …"*. Fixed by re-deriving on `legendselectchanged` rather
   than by dropping the enumeration — the enumeration is the only part of that sentence carrying
   numbers, and a multi-series chart is where a screen-reader user has least other recourse.

5. **Task 7 is product-wide and stopped at step 2, as its own stopping condition required.**
   `--ring` reaches 34 call sites across 26 files — twelve shadcn primitives plus the notebook, SQL
   Lab, the control plane and the explore routes; only 5 files are under `components/chart/`. The
   measurements were re-derived independently (numeric OKLCH→sRGB, alpha composited) and match the
   auditor's table to two decimal places. **Step 4's open question is answered: Chrome's
   `outline-style: auto` DOES honour an author `outline-color`** — measured by forcing
   `outline-color: red` on a focused element whose style stayed `auto` and screenshotting it, which
   drew a red ring. So the global `index.css:246` fallback needs no different fix; darkening `--ring`
   and dropping the `/50` would reach it. What DID land here: the two controls this PR introduces
   carry a `--primary` focus outline (3.29:1 on `--accent`, 3.58:1 on `--background`) rather than
   inheriting a 1.54:1 one.

Two further things the plan did not anticipate, both from measurement:

- **Wrapping a header's text in a labelled button leaves the `<th>` unnamed in Chrome.** Read off the
  live accessibility tree by deleting the attribute from one header: that columnheader came back with
  no name at all, beside nine siblings still named for their column. Both tables therefore give the
  `<th>` an explicit `aria-label` of the plain title. jsdom does not reproduce this.
- **A first draft clamped the column width in two places**, and the mutation deleting the clamp inside
  the shared control survived because the caller's clamp covered for it. One clamp now.
