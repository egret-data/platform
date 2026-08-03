# A sort control in the chart studio — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user say what order a chart's rows come back in. Today nothing can, which is why every
query carried `orderby: []` and three chart types drew their categories in engine order.

**Architecture:** `orderby` joins `filters` and `rowLimit` in `SharedQueryState` — sort is a query
concern, which is how Metabase models it too (a notebook step, not a viz setting). An authored sort
overrides the ascending default PR #394 derives, following Metabase's own precedence rule.

**Tech stack:** TypeScript, React, vitest.

---

## Why this exists

PR #394 stopped bar, row and box plot charts drawing categories in whatever order the engine returned.
It fixed the symptom in two layers — `buildSpec` derives an ascending `orderby`, and `sortCategories`
sorts a nominal axis for charts already saved. **It could not fix the cause**: `SharedQueryState` is
`{filters, rowLimit, temporalUnits}`, so no descriptor can express a sort and nothing in the studio can
ask for one.

The visible consequence, measured on the parity dashboard:

| chart | egret | Metabase |
|---|---|---|
| bar | `Doohickey, Gadget, Gizmo, Widget` | `Widget, Gadget, Gizmo, Doohickey` |

Metabase's differs because its question was **written** with `ORDER BY revenue DESC`. egret matches
Metabase's *implicit* default and has no way to express the explicit one.

## The precedence rule — port Metabase's, do not invent one

`../metabase/src/metabase/query_processor/middleware/add_implicit_clauses.clj:71`:

> Fields specified in `breakout` should add an implicit ascending `order-by` subclause **unless that
> Field is already explicitly referenced in `order-by`**.

So: **an explicit sort wins; otherwise ascending by the breakout.** PR #394 already implements the
second half in `viz/cartesian.tsx`'s `buildSpec`. This PR adds the first half and must not break the
second — a chart with no authored sort keeps behaving exactly as it does today.

## THE TRAP THIS PLAN EXISTS TO AVOID

**`sortCategories` will silently override an authored sort.** PR #394's layer 2 sorts a nominal axis
client-side *whatever the query asked for*, precisely so that already-saved charts with `orderby: []`
get a stable order. A user who asks for descending-by-revenue would get their query honoured, their
rows returned in that order, and then re-sorted alphabetically before drawing — the control would
appear to do nothing.

The seam already exists. `cartesian.ts:59` has `preserveCategoryOrder`, added by #394 for the funnel's
bar mode, and its comment states the rule this PR completes:

> Everything that reaches this from a stored `orderby: []` spec must NOT set it — see `sortCategories`.

The inverse is what you wire: **a chart whose stored spec carries a NON-EMPTY `orderby` must set
`preserveCategoryOrder`**, because the engine has already put the rows in the order the user asked
for. `options/boxplot.ts` shares `sortCategories` and needs the same treatment.

**This is the acceptance test's centre of gravity.** A sort control that passes its unit tests and does
nothing on screen is the exact failure this programme has shipped twice.

## Scope

**In:** multiple sorts, ordered, each a field plus a direction; persistence (free — `spec.orderby` is
already stored and already round-trips through `harnessParams`); the `preserveCategoryOrder` wiring.

**Out, and say so in the PR:**
- **Drag-to-reorder the sort list.** `@dnd-kit` is already a dependency and `SeriesListSection` uses it,
  so it is cheap later, but arrow buttons are enough to ship and keep the diff reviewable.
- **A filter builder.** The studio has none either — filters arrive only from drilling
  (`ChartExplore.tsx:254`). Out of scope, but worth noting in the PR that sort is the first *authored*
  query clause the studio gets, so it sets the pattern.
- **Per-series or per-column sorts.** Metabase has one sort list per query stage; so does this.

## What fields are sortable

`crates/egret-bi/src/chartquery.rs:364` (`orderby_sql`) accepts, in order: a **metric** (resolved to its
expression), a **dimension** in the spec, or a **raw column**. Anything else is
`unknown order-by field: {field}` — a 400.

So the picker's options are exactly: the spec's `dimensions` + its `metrics` + (for a raw-records viz
such as the table or box plot) its `columns`. **Derive them from the spec the descriptor builds**, not
from the dataset's full column list, or the user can pick a field the query will reject.

---

## Task 1: `orderby` becomes shared query state

**Files:**
- Modify: `frontend/src/components/chart/viz/types.ts` (`SharedQueryState`)
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (state, `specForRun`, load, reset)
- Modify: every `buildSpec` that hardcodes `orderby: []` — `viz/fields.tsx:73,79,84`,
  `viz/cartesian.tsx`, `viz/pivot.tsx:186`, `viz/map.tsx:96`, `viz/table.tsx:108,139`

- [ ] **Step 1: Write the failing test.** With `shared.orderby` set, each descriptor's `buildSpec`
  emits it. With it empty, cartesian keeps the derived ascending default from #394 and the others keep
  `[]`. **Assert both halves** — a change that always emits the shared value would silently delete
  #394's default and reintroduce the bug it fixed.

  Smartscalar is the exception: `simple.tsx:335` derives its own for a reason ("Trend compares the last
  two periods, so the rows must come back in dimension order"). Decide whether an authored sort should
  override it and say why either way.

- [ ] **Step 2: Run it and watch it fail.** Report the exact output.

- [ ] **Step 3: Implement.** Follow how `filters` and `rowLimit` are threaded — same state shape, same
  load path (`ChartExplore.tsx:118` reads `p.spec?.filters`), same reset (`:320`).

- [ ] **Step 4:** `cd frontend && npx vitest run && npx tsc --noEmit && npm run lint && npm run lint:harness`.

  **The harness will move**: `scripts/harness/params.ts` runs through these same `buildSpec`s and
  `charts.test.ts` pins what the seed emits. Update it and say what changed.

- [ ] **Step 5: Commit**

```bash
git add frontend scripts
git commit -m "feat(charts): a chart's query can be asked for an order"
```

---

## Task 2: an authored sort is not overridden by the renderer

**Files:**
- Modify: `frontend/src/components/chart/ChartRenderer.tsx` (set `preserveCategoryOrder`)
- Modify: `frontend/src/components/chart/echarts/options/boxplot.ts` if it needs the same
- Modify: the cartesian and box-plot option tests

- [ ] **Step 1: Write the failing test.** Rows arriving in a deliberate non-alphabetical order, a spec
  carrying a non-empty `orderby`, and the drawn categories must come out **in the arrival order** — the
  engine has already sorted them. The sibling case: an empty `orderby` still sorts, which is #394's
  behaviour and must not regress.

  **Use the real orderings** PR #394 measured — `['Widget','Gadget','Doohickey','Gizmo']` and
  `['Gizmo','Widget','Gadget','Doohickey']`. A fixture already alphabetical cannot detect either
  direction of this.

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement.** `ChartRenderer` builds the option config from params, which carry
  `spec.orderby` — check it does before threading anything, and **if the spec is not reachable there,
  STOP AND REPORT** rather than plumbing a new channel through the renderer inside a settings PR.

  Update `preserveCategoryOrder`'s doc comment: it currently describes one exemption (the funnel) and
  states the `orderby: []` rule; it must now describe both callers and why.

- [ ] **Step 4:** run the suite. Bar, row, box plot and the funnel's bar mode all reach this code.

- [ ] **Step 5: Commit**

```bash
git commit -m "fix(charts): a sort the user asked for is not re-sorted before drawing"
```

---

## Task 3: the control

**Files:**
- Create: `frontend/src/components/chart/SortRows.tsx` (or extend an existing Data-tab section — read
  what is there first)
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` — render it in the Data tab
- Create: its test

- [ ] **Step 1: Write the failing test.** Rendering the control with a spec's fields offers exactly the
  sortable ones (dimensions + metrics + raw columns, per `orderby_sql`); adding a sort patches
  `orderby`; toggling direction flips `desc`; removing removes; the order of the list is the order of
  the array.

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement.** Follow the Data tab's existing shape — `Row limit` is a plain labelled
  input at `ChartExplore.tsx:459`, and `FilterChips` (`components/chart/FilterChips.tsx`) is the house
  pattern for a list of query clauses with a remove affordance. Do not invent a third idiom.

  Look at Metabase's own sort step for what to offer — a field, a direction, several of them, ordered —
  and say where you deliberately differ.

- [ ] **Step 4:** run the suite and the gates.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(charts): the studio can ask for a row order"
```

---

## Task 4: the live comparison, and the PR

**This is the acceptance test**, and for this PR it is not optional: the failure mode is a control that
works in tests and does nothing on screen.

- [ ] **Step 1: Gates.** `npx tsc --noEmit`, `npm run lint`, `npm run lint:harness`, `npx vitest run`.
  Report the exact count.

- [ ] **Step 2: Build, and CHECK THE BUNDLE HASH** against `ls -t frontend/dist/assets/index-*.js`
  after loading the page. A stale `index.html` has produced a wrong conclusion twice in this programme.

  A static server on :5174 serves `frontend/dist` and proxies `/api` to `127.0.0.1:30080`; the script
  is `serve.py` in the session scratchpad. Log in at `/login` → "Admin sign-in (break-glass)" →
  `admin@egret.local` / `Admin12345!`, rate-limited 5 per 5 minutes.

- [ ] **Step 3: Drive it in the browser.** On `MB · Bar`:

  1. Add a sort on `revenue`, descending. **The chart must redraw as
     `Widget, Gadget, Gizmo, Doohickey`** — the same order Metabase's card shows, since that is what
     its author's `ORDER BY revenue DESC` produces.
  2. Save, reload the page, and confirm it persisted.
  3. Remove the sort. It must fall back to ascending-by-dimension, not to engine order.
  4. **Load the dashboard six times** and confirm the authored order is stable across all six. Six,
     not two — three consecutive loads showed the same engine order and is what hid the original
     defect for six PRs.

- [ ] **Step 4: Nothing else moved.** Re-check the charts PRs 02–07 fixed: the sankey's truncated
  labels and nine colours, the row chart's ticks, the gauge's single graduation, the pie's absent slice
  labels, the pivot's 14 rows, the temporal axes reading `January 2026`.

- [ ] **Step 5: Open the PR.** Body carries: the precedence rule and where it came from, the
  `preserveCategoryOrder` wiring and why the control would have been inert without it, the browser
  evidence including all six loads, what moved in the harness, and the non-goals. Push and open.
  **Do not merge.** Stop and report.

---

## Self-review

- **Metabase's rule is ported, not invented** — explicit wins, otherwise ascending by breakout, from
  `add_implicit_clauses.clj:71`.
- **The trap is named as the main risk**, because it is the one that produces a green suite and a dead
  control, and this programme has shipped that shape twice: PR 05b's funnel fix looked inert until a
  stale bundle was ruled out, and PR 06's panel test reimplemented the line under test rather than
  rendering it.
- **Two stop conditions.** Task 2 if the spec is not reachable from `ChartRenderer`; Task 1 if
  smartscalar's own derived order turns out to need more than a one-line decision.
- **Known soft spot.** The sortable-field list is derived from the spec the descriptor builds, so a viz
  whose `buildSpec` puts a field somewhere unexpected could offer a field the backend rejects with a
  400. Task 3's test should cover at least one raw-records viz (table or box plot) and one aggregate
  viz, since those two take different branches of `orderby_sql`.
