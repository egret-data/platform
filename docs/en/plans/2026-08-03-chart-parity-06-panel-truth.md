# Chart parity PR 06 — the panel tells the truth — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the settings panel from lying. A saved table chart that opens empty, a hide toggle that
does nothing, a number field that eats valid input, and a slice list in an order the chart does not
draw — each one makes a user distrust every other control on the page.

**Architecture:** No new subsystem. Four independent defects in `viz/table.tsx`,
`echarts/options/pie.ts`, `settings/PieDisplaySection.tsx`, plus the panel/chart ordering seam.

**Tech stack:** TypeScript, React, vitest.

---

## Scope: this is the CORRECTNESS half of workstream 6

The design spec's workstream 6 lists six items. **A live comparison found three of them already done**
and the list stale, while turning up four defects the spec never mentioned — including one that makes
saved charts unopenable. So this plan is not the spec's list.

**PR 06 (this plan) — the panel is WRONG.** Controls that lie, break, or lose a user's work.

**PR 06b — the panel is INCOMPLETE.** Deferred, and sketched at the end with its evidence: the Axes
tab's missing X/Y grouping, `getHidden` for inapplicable settings, the table row count defaulting off,
per-series line settings, and Metabase's Power scale.

The split is deliberate: everything here is something a user would call a bug; everything in 06b is
something they would call a missing feature. Mixing them would let the cosmetics dilute the review of
the correctness fixes.

## Verdicts on the spec's six items, measured live

| # | spec item | verdict |
|---|---|---|
| 1 | Group Axes tab under X-axis / Y-axis | **still open** → PR 06b |
| 2 | Expose pie's `showTotal` | **done** — control at `PieDisplaySection.tsx:121-129`; a default divergence remains, below |
| 3 | Pie percentages Off / In legend / On the chart / Both | **done** — Auto / Off / In legend / In chart / Both; a default divergence remains |
| 4 | Hide settings that do not apply (`getHidden`) | **still open** → PR 06b |
| 5 | The `2,5` numeric input | **partly — and now worse**, see Task 3 |
| 6 | Row count in the table footer | **partly** — `ChartTable.tsx:415` renders it, but gated behind pagination which defaults off → PR 06b |

The spec's parenthetical on item 1 is misleading: `settings/ChartAxesSection.tsx` does have X/Y
headings, but it is used **only by the waterfall and box plot** (`viz/simple.tsx:187,218`), and its own
docstring says it is deliberately not cartesian's. Live, MB · Bar's Axes tab is flat and interleaved.

---

## Task 1: a saved table chart must not open empty — DIAGNOSE FIRST

**This is the worst defect found and it is on nobody's list.**

`viz/table.tsx:77-88`:

```ts
fromParams: (p) => {
  if (isParamsV2(p)) {
    return {
      columns: (p.settings["table.columns"] as string[]) ?? [],
      metrics: (p.settings["table.metrics"] as string[]) ?? [],
    };
  }
  // v1 stored no table config at all: the columns were re-derived from the spec.
  return { metrics: p.spec?.metrics ?? [], columns: [...(p.spec?.dimensions ?? []), ...(p.spec?.columns ?? [])] };
},
```

The v1 branch recovers columns from `spec`; the v2 branch does not. A chart stored as
`{version: 2, settings: {}, spec: {columns: [...]}}` therefore opens on **"Set up your Table / Add: at
least one column or metric"** — the chart looks lost.

Measured: **three of the four** table/object charts in the `demo` workspace are in that state —
`MB · Table`, `Customer survey responses`, `Most recent subscription`. `MB · Object` is fine because it
has `settings["object.columns"]`.

### Step 1 — establish whether this is a defect or a seeding artefact. REPORT BEFORE FIXING.

PR 05b's pivot looked exactly like this and turned out to be **harness seeding**: charts inserted
straight into the database carried a hand-written `spec` and empty `settings`, while anything saved
through the editor carries both. Find out which this is:

- Do the three affected charts have empty `settings` because they were seeded, or because a save path
  wrote them that way? Check whether `Customer survey responses` and `Most recent subscription` are
  demo-seed rows or user-created.
- Can a chart reach `{version: 2, settings: {}, spec: {columns}}` through `paramsForSave()`? Read
  `frontend/src/routes/explore/ChartExplore.tsx` around `paramsForSave` and `toSettings`.

**Both answers matter and they lead to different work.** If only seeded rows are affected, the fix is
still worth making — a chart that cannot be opened is a bad failure mode whatever wrote it — but the
PR must say so plainly rather than implying users are losing charts.

### Step 2 — write the failing test

`fromParams` on a v2 params object with empty settings and a populated `spec` must recover the
columns. Assert on the returned config, not on what the UI renders.

### Step 3 — run it and watch it fail. Report the exact output.

### Step 4 — fix

The obvious fix is to fall back to `spec` in the v2 branch as v1 does. **Check the object descriptor
too** (`settings["object.columns"]`) and any other v2 descriptor with the same shape — a fix that
repairs the table and leaves a sibling broken is half a fix. Say which descriptors you checked.

Consider whether a v2 chart with empty settings should be *migrated* on save so it stops depending on
the fallback, and state your decision either way. Do not build a database migration.

### Step 5 — run the suite, commit

```bash
cd frontend && npx vitest run src/components/chart && npx tsc --noEmit
git add frontend/src/components/chart
git commit -m "fix(charts): a saved table chart opens with the columns it was saved with"
```

---

## Task 2: the pie's hide, rename and colour must land on the slice the user clicked

`echarts/options/pie.ts:900-902` filters `hiddenSlices` against the **formatted** slice name, and
`sliceOrder`, `sliceNames` and `sliceColors` key the same way — while the settings panel holds **raw**
dimension values. PR 04 recorded this seam; PR 06 fixes it.

**Reproduced in the UI**, pie over `order_month` with Date style `January 2024`:

- The chart draws `December 2026`, `November 2026`, … ; the panel lists `2026-12-01`, `2026-11-01`, … .
  Two vocabularies side by side.
- Toggling **"Show 2026-12-01"** off does **nothing** — the chart still renders all 12 slices.
- **Control**: set Date style to `2024-01-31`, which formats back to the raw ISO string, and the same
  toggle works — 11 slices, December gone.

So the failure is exactly the raw/formatted mismatch, demonstrated by making the two coincide.

- [ ] **Step 1: Write the failing test.** A pie with a `columnSettings` format on its dimension, a
  `hiddenSlices` entry naming the **raw** value, must hide that slice. Add the sibling cases for
  `sliceColors` and `sliceNames` — the reproduction only proves `hiddenSlices`, and a fix that
  repairs one keying and not the others would pass.

- [ ] **Step 2: Run it and watch it fail.** Report the exact output.

- [ ] **Step 3: Decide WHERE the seam closes, and say why.** Two directions:
  1. **Give the panel the builder's `labelOf`**, so both sides speak formatted labels.
  2. **Key the builder on raw values** and format only for display.

  Read how Metabase does it before choosing — its pie keys on the raw dimension value and formats at
  render, which argues for (2), but egret's `buildPieRings` aggregates on the formatted label, so (2)
  may change which rows merge. **If the choice changes aggregation behaviour, stop and report** rather
  than deciding it inside a settings PR.

- [ ] **Step 4:** run the suite, commit.

```bash
git commit -m "fix(charts): a pie's hide and rename find the slice the panel named"
```

---

## Task 3: "Group slices below" must accept the numbers a user types

`settings/PieDisplaySection.tsx:184-187`:

```tsx
<Input id="pz-threshold" type="number" min={0} max={100} step={0.5}
  value={config.sliceThreshold ?? 2.5}
  onChange={(e) => { const n = Number(e.target.value); onChange({ sliceThreshold: Number.isNaN(n) ? 0 : n }); }} />
```

The spec's `2,5` comma is gone — the DOM value is `"2.5"`. **The input is now worse than the defect it
replaced**, measured with real key presses, not `fill()`:

- **Clearing the box stores `0`**, which is a *different setting* (never fold) and the field then reads
  `"0"` with no way back to the 2.5 default.
- Typing `4` then `.` collapses the field to **`0`** — `Number("4.")` is fine but the intermediate
  empty/partial states are not handled.

Metabase uses a plain text input for "Minimum slice percentage" and has neither problem.

- [ ] **Step 1: Write the failing test.** Drive it with `userEvent.type` and `userEvent.clear`, not
  `fireEvent.change` with a final value — the defect lives in the intermediate states and a test that
  sets the final value passes against the broken code. Assert what is stored after each keystroke.

- [ ] **Step 2: Run it and watch it fail.** Report the exact output.

- [ ] **Step 3: Fix.** Hold the in-progress text in component state and commit a number only when it
  parses; an empty field must mean "unset" (fall back to the default), not `0`. Check whether other
  numeric inputs in the settings tree share the pattern — `Y min`, `Y max`, `Decimal places`,
  `Freeze first columns` — and say which you checked and whether they have the same defect. **If more
  than two do, stop and report**; a shared numeric-input component is a bigger change than this task.

- [ ] **Step 4:** run the suite, commit.

```bash
git commit -m "fix(charts): the slice threshold keeps the number being typed into it"
```

---

## Task 4: the slice list is in the order the chart draws

The panel lists slices in **raw distinct order** — `Gizmo, Widget, Gadget, Doohickey` — while the chart
draws **value-descending** — `Widget, Gadget, Gizmo, Doohickey`. Metabase's list is value-descending
and matches its chart. On a twelve-slice monthly pie egret's list is thoroughly scrambled, and a user
matching a legend swatch to a panel row has to hunt.

Related, from PR 04 and **reproduced**: with "Group slices below" set high enough to fold two slices
into "Other", the panel still lists the folded slices with colour, rename and hide controls **that now
do nothing**, and has no row for "Other". Pinning a folded slice's colour then makes the panel show
`Widget = #98d9d9` while the chart draws `Widget = #F2A86F`, because `colorsForKeys` spent a slot on a
key the chart never sees.

- [ ] **Step 1: Write the failing tests** — the panel's row order equals the chart's draw order; a
  folded slice does not appear as an actionable row.

- [ ] **Step 2: Run and watch them fail.** Report the exact output.

- [ ] **Step 3: Fix.** The panel needs the same ordering and folding the builder applies, which means
  it needs the **measure**, not just the distinct dimension values — PR 04 recorded that
  `PieDisplayControl` (`viz/simple.tsx`) hands over names only. Thread what the builder uses.

  **This is the task most likely to grow.** If threading the measure into the panel turns into
  reshaping how descriptors receive data, **stop and report** — that is a Chart Studio change, not a
  settings fix.

- [ ] **Step 4:** run the suite, commit.

```bash
git commit -m "fix(charts): the slice list reads in the order the pie draws"
```

---

## Task 5: two defaults that disagree with Metabase

Both are one-line changes with a visible effect, and both were measured on the parity pie.

- **Percentages.** `resolvePercentVisibility(undefined, 1)` returns `"off"`; Metabase's
  `getDefaultPercentVisibility()` is `"legend"` unconditionally. Live, Metabase's pie shows
  28.0 / 26.9 / 25.4 / 19.7 % in its legend and egret's shows none. It cascades: egret hides
  "Decimal places" whenever the resolved visibility is `off`, so that control is missing on the
  parity pie where Metabase shows it.
- **Show total.** Metabase's `getDefaultShowTotal` is `true` unconditionally and the control is always
  offered; egret gates the control on `hasHole`, so a plain pie can never be given the centre total
  Metabase draws by default.

- [ ] **Step 1: Write the failing tests** — a single-ring pie with no stored settings resolves
  percentages to `legend`; the `Show total` control is present on a pie without a hole.
- [ ] **Step 2: Run and watch them fail.**
- [ ] **Step 3: Fix.** Both are documented deliberate choices in egret — **read the reasoning before
  overriding it**, and if the existing justification is sound, say so and leave it rather than
  changing behaviour to match a table in this plan. Parity is the goal, but a documented decision
  deserves an argument, not a silent reversal.
- [ ] **Step 4:** run the suite, commit.

---

## Task 6: the live comparison, and the PR

**This is the acceptance test.** Every PR in this programme has shipped a defect that passed its unit
suite and was caught only here.

- [ ] **Step 1: Gates.** `cd frontend && npx tsc --noEmit && npm run lint && npx vitest run`. Report the
  exact test count.

- [ ] **Step 2: Build, and CHECK THE BUNDLE HASH.** `npm run build`, then after loading the page compare
  `[...document.querySelectorAll("script[src]")].map(s => s.src)` against
  `ls -t frontend/dist/assets/index-*.js`. PR 05b's first live measurement was wrong because the
  browser had cached `index.html` and was running the previous bundle.

- [ ] **Step 3: Measure.** egret `http://127.0.0.1:5174`, log in via `/login` → "Admin sign-in
  (break-glass)" → `admin@egret.local` / `Admin12345!` (rate-limited 5 per 5 minutes — log in once).
  Metabase `http://localhost:3033`.

Acceptance criteria, each against a finding above:

1. **`MB · Table` opens with its ten columns**, not "Set up your Table". Same for
   `Customer survey responses` and `Most recent subscription`.
2. **On a pie over `order_month` with Date style `January 2024`, toggling a slice off hides it** —
   today it does nothing.
3. **Typing `4` then `.` into "Group slices below" leaves `4.` in the field**, and clearing it restores
   the default rather than storing 0.
4. **The pie panel's row order equals the drawn order**, and a folded slice has no actionable row.
5. **A single-ring pie shows legend percentages and offers Show total** — or, if Task 5 argued to keep
   egret's defaults, the PR says why.
6. **Nothing that worked before broke** — re-check the charts PRs 01–05b fixed.

- [ ] **Step 4: Open the PR.** Body carries the six-item verdict table, the before/after for each task,
  the Task 1 diagnosis (defect or seeding artefact), and the PR 06b list below. Push and open.
  **Do not merge.** Stop and report.

---

## PR 06b — the panel is incomplete (deferred, with evidence)

Not in this PR. Recorded so it starts from measurement:

- **Axes tab grouping.** MB · Bar renders flat: `Horizontal · X axis scale · X label · Y label · Log
  scale · X axis lines and ticks · Y axis lines and ticks · Y tick marks · Y min · Y max`. Metabase
  splits into **X-axis** (Show label, Label, Show lines and tick marks, Scale) and **Y-axis** (Show
  label, Label, Auto y-axis range, Scale, Show lines and tick marks, Number of tick marks). Note
  `StudioPanel.tsx` deliberately flattens the Axes tab, so the fix belongs in the cartesian controls.
- **`getHidden`.** Egret's single-series bar shows `Stacking = Don't stack`; Metabase hides it —
  `lib/settings/graph.ts:353-365`, `return stackableDisplays.length <= 1`. The row chart shows it too.
- **Row count.** `ChartTable.tsx:415` renders `Showing {start+1}–{…} of {totalRows}` but it sits inside
  `{paginationEnabled && …}` and `paginationEnabled` defaults false. Metabase shows "Showing 60 rows"
  unconditionally.
- **Per-series line settings.** Egret's Line shape, Missing values and Markers are global; Metabase's
  are per series, and it adds Line style, Line size (S/M/L), Y-axis position, trend line and values
  per series. On a multi-series line egret cannot give one series its own shape.
- **Y scale.** Egret offers a binary "Log scale" switch; Metabase offers Linear / Power / Log.
- **Goal line.** Egret buries Goal value / Goal label / Trend line behind "More options (2)"; Metabase
  has "Goal line" as a top-level Display toggle.
- **Table column lists.** Egret has two — Data tab "Columns" (what to query) and Display tab "Visible
  columns" (what to show) — where Metabase has one "Columns" tab. Egret's Show all / Hide all, numeric
  Freeze first columns and Paginate are improvements; Metabase's "Freeze rows" is missing.
- **A picker showing a format the chart is not using.** On a pie over `order_month` with nothing stored,
  "Date style" reads `January 31, 2024` while the chart draws raw `2026-12-01`, and there is no Auto
  option to get back to unformatted. Same class as the seam Task 2 fixes; likely resolved by it —
  **re-check after Task 2 before planning it separately.**
- Lower confidence: a `beforeunload` prompt fires after only expanding a disclosure and switching tabs,
  with no config edit.

---

## Self-review

- **Spec coverage.** Workstream 6's six items are each given a verdict; three are done, two move to
  PR 06b, one (the numeric input) is Task 3 and turned out to be worse than the spec described.
- **The spec's list is stale and this plan says so**, rather than implementing items already shipped.
- **Task 1 is diagnosis-first** because PR 05b's pivot presented identically and was a harness artefact.
  The fix is worth making either way; the PR's wording is what depends on the answer.
- **Two tasks carry an explicit stop condition** — Task 2 if the keying change alters aggregation,
  Task 4 if threading the measure reshapes descriptors. Both are the kind of scope growth that has
  turned other tasks in this programme into two PRs' worth of work.
- **Known soft spot.** Task 5 asks an implementer to argue with a documented decision. That is
  deliberate: egret's defaults were chosen on purpose and recorded, and "Metabase does X" is not by
  itself a reason to reverse a reasoned choice. If the argument fails, keeping egret's default and
  saying why in the PR is the correct outcome.
