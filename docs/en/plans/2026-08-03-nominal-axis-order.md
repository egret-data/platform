# A nominal axis draws in a stable order — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop bar and row charts from drawing their categories in whatever order the engine happened
to return.

**Architecture:** Derive an `orderby` inside cartesian's `buildSpec`, the way smartscalar already does,
so the *data* is deterministic; and sort a nominal axis in `sortCategories` so charts already saved
with `orderby: []` are fixed too. Both layers, each for a stated reason.

**Tech stack:** TypeScript, ECharts, vitest.

---

## The defect

Measured by PR 07 on `POST /charts/{id}/data`, ten calls against the same chart:

```
7 × ['Gizmo', 'Widget', 'Gadget', 'Doohickey']
3 × ['Widget', 'Gadget', 'Doohickey', 'Gizmo']
```

**Three consecutive page loads all showed the 70% ordering**, which is why a two-load comparison called
it deterministic and it survived PRs 02–06. Any measurement of category *position* on a bar or row
chart — and this programme took several — was measuring a value that changes between page loads.

Every grouped chart's spec carries `orderby: []`. Nine of the twelve grouped charts are saved by a
client-side sort they apply themselves:

| protection | charts |
|---|---|
| temporal / numeric axis sort (`cartesian.ts:527-528`, `waterfall.ts:116-118`) | line, area, combo, waterfall, scatter |
| value-descending (`pie.ts:320`, `sankey.ts:298`) | pie, sankey |
| explicit step rank (`funnel.ts:57`) | funnel |
| `sortTree` (`pivot/engine.ts:195`) | pivot |
| `orderby` derived in `buildSpec` (`simple.tsx:335`) | smartscalar |
| **none** | **bar, row** |
| order varies, no visual consequence | map |

`cartesian.ts:526` is deliberate and its comment is accurate:

> Order the x categories by axis type — temporal chronologically, numeric ascending — while leaving a
> nominal (string) axis in first-seen order. (Metabase model/dataset.ts.)

## Metabase really does leave it alone — so the fix is not a client-side sort alone

`../metabase/frontend/src/metabase/visualizations/echarts/cartesian/model/dataset.ts:849`:

```ts
export const sortDataset = (dataset, xAxisScale, showWarning) => {
  if (xAxisScale === "ordinal") {
    return dataset;          // ← nominal axis: unchanged
  }
  if (xAxisScale === "timeseries") { … }
  return sortByDimension(dataset, …numeric…);
};
```

Metabase does not need to sort, because **its queries carry the order**: an MBQL question's sort step,
or the `ORDER BY` a native question's author wrote. The harness's own `MB By Category` question uses
`ORDER BY revenue DESC`.

egret's `buildSpec` hardcodes `orderby: []` everywhere except smartscalar. **That is the real gap**, and
it is why the renderer was left holding a problem the query should have solved.

## Two layers, two reasons

**1. Derive an `orderby` in cartesian's `buildSpec`.** This is the faithful fix: it makes the *data*
deterministic, so tooltips, drills, exports and any row limit all agree, not just the drawn axis.
`simple.tsx:335` is the in-codebase precedent — smartscalar derives its own from its config, with a
stated reason, and needs no new `SharedQueryState` field:

```ts
orderby: typeof c.dimension === "string" && c.dimension ? [{ field: c.dimension, desc: false }] : [],
```

**2. Sort a nominal axis in `sortCategories` as well.** Layer 1 only helps charts saved *after* it: the
dashboard path deserialises the **stored** spec verbatim (PR 05b established this), so every already
saved chart keeps its `orderby: []` until someone re-saves it. Without layer 2 the fix reaches almost
nobody.

`pivot/engine.ts:191-193` is the precedent, and its reasoning applies here word for word:

> Non-deterministic output that changes between runs is worse than any particular order, and Metabase
> sorts both axes too.

**Layer 2 is a deliberate divergence from Metabase**, and the PR must say so: Metabase can leave it
alone because its queries are ordered; egret's stored ones are not.

## The one open decision: which order?

**Ascending by the dimension value** is what egret has chosen twice — `sortTree` (`cmpValues`
ascending) and smartscalar (`desc: false`). It is neutral: it encodes no guess about what the user
wants ranked.

**Descending by the metric** is what the harness's Metabase question uses, what `pie.ts` and
`sankey.ts` do, and what reads best on a bar chart.

**Establish what Metabase's query builder emits by default** for a "summarise by category" question
before choosing — that is the decisive evidence neither the plan nor PR 07 has. Look at the MBQL a
fresh summarise-by produces, and whether it carries an `order-by` at all.

- If Metabase's default carries no order, take **ascending by dimension**: it matches egret's own two
  precedents and is the least surprising thing to do with no user intent.
- If it orders by the aggregation descending, take that.

**Say which you chose and what evidence decided it.** Do not pick by preference.

## Non-goals

- **A sort control in the studio.** The reason the query has no `orderby` is that nothing lets a user
  ask for one. That is a real gap and a Chart Studio feature; it is not this PR.
- **Changing the other eleven grouped charts.** Their client-side sorts work. Adding an `orderby` to
  every `buildSpec` would be more correct and is worth considering later, but it changes which rows
  survive a row limit on eleven chart types at once and needs its own comparison.

---

## Task 1: the query carries the order

**Files:**
- Modify: `frontend/src/components/chart/viz/cartesian.tsx` (`buildSpec`, around line 290)
- Modify/extend: the cartesian codec tests under `frontend/src/components/chart/viz/__tests__/`

- [ ] **Step 1: Settle the order question above and report the evidence before writing code.**

- [ ] **Step 2: Write the failing test.** A bar chart config over a string dimension produces a
  non-empty `orderby`; a temporal or numeric x-axis keeps whatever it has today (say which, and why
  changing it is out of scope); a chart with no dimension is unaffected.

  **Note the harness contract**: `scripts/harness/params.ts` generates params through this same
  `buildSpec`, and `scripts/harness/charts.test.ts` asserts the 19 configs round-trip. Your change will
  move what the seed emits — **that is correct**, and PR 07's docs predicted it. Update those
  expectations and say so.

- [ ] **Step 3: Run it and watch it fail.** Report the exact output.

- [ ] **Step 4: Implement**, following `simple.tsx:335`'s shape.

- [ ] **Step 5:** `cd frontend && npx vitest run && npx tsc --noEmit && npm run lint && npm run lint:harness`.

- [ ] **Step 6: Commit**

```bash
git add frontend scripts
git commit -m "fix(charts): a cartesian query asks for the order it draws in"
```

---

## Task 2: charts already saved are fixed too

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts` (`sortCategories`, line 526)
- Modify: the cartesian option tests

- [ ] **Step 1: Write the failing test.** Build a bar option from rows whose categories arrive in a
  scrambled order, with a spec carrying `orderby: []`, and assert the drawn categories come out in the
  chosen order. **Use the real scrambling**: PR 07 measured `['Widget','Gadget','Doohickey','Gizmo']`
  arriving where `['Gizmo','Widget','Gadget','Doohickey']` is the 70% case — a fixture already in the
  target order cannot detect this at all.

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement.** Rewrite `sortCategories`' nominal branch and **correct its comment** — it
  currently states the behaviour being removed, and it cites Metabase for it. The new comment must say
  what egret now does, that Metabase does not, and why the difference is deliberate (Metabase's queries
  carry the order; egret's stored ones do not).

- [ ] **Step 4:** run the full suite. **Bar, row and box plot all go through this file**, and the box
  plot's category axis is nominal too — check whether it is affected and say so either way.

- [ ] **Step 5: Commit**

```bash
git commit -m "fix(charts): a nominal axis draws in a stable order, whatever the query returned"
```

---

## Task 3: the live comparison, and the PR

**This is the acceptance test.** Every PR in this programme has shipped a defect that passed its unit
suite and was caught only here — and this defect in particular survived six PRs of review because it
looked stable across two page loads.

- [ ] **Step 1: Gates.** `npx tsc --noEmit`, `npm run lint`, `npm run lint:harness`, `npx vitest run`.
  Report the exact test count.

- [ ] **Step 2: Build, and CHECK THE BUNDLE HASH** against `ls -t frontend/dist/assets/index-*.js`
  after loading the page. A stale `index.html` produced a wrong conclusion in PR 05b, and PR 07 found
  `dist` genuinely stale.

- [ ] **Step 3: Prove determinism the way PR 07 disproved it.** Two page loads is not enough — that is
  exactly what missed this for six PRs.

  - **Ten** calls to `POST /charts/{id}/data` for `MB · Bar`: the category order must be identical
    every time. Report all ten.
  - **Six** page loads of the dashboard, reading the drawn category order out of the DOM: identical
    every time. Report all six.
  - Do the same for `MB · Row` and the box plot.

- [ ] **Step 4: Compare against Metabase** at a matched card size and report the ordering on both
  sides. They will differ if you chose an order Metabase's harness question does not use — **that is a
  documented divergence, not a failure**, and the PR body must state it with the reason.

- [ ] **Step 5: Nothing else moved.** Re-check the charts PRs 02–06 fixed: the sankey's nine colours,
  the row chart's `0 / 500K` ticks, the gauge's single label, the pie's absent slice labels, the
  pivot's 14 rows, line/area/combo/waterfall reading `January 2026`.

- [ ] **Step 6: Open the PR.** Body carries: the ten-call and six-load evidence, the order chosen and
  what decided it, the deliberate divergence from Metabase in layer 2, the note that stored charts are
  fixed by layer 2 while layer 1 only helps on re-save, and the studio sort control as the follow-up.
  Push and open. **Do not merge.** Stop and report.

---

## Self-review

- **The plan's own premise was checked, not assumed.** PR 07 recommended a client-side sort by value
  descending. Reading Metabase's `sortDataset` shows it does not sort a nominal axis at all, which
  makes a client-side sort a divergence — so this plan adds the query-level fix as the faithful layer
  and keeps the client-side one only for already-stored charts, with that reason stated.
- **The order is left open on purpose**, with the evidence that should settle it named. egret has
  chosen ascending-by-dimension twice; the harness's Metabase question uses descending-by-metric. An
  implementer who picks by preference rather than by what Metabase's query builder emits has not done
  the task.
- **Known soft spot.** Layer 1 changes what `scripts/harness/params.ts` emits, so PR 07's seed will
  produce different specs than the charts currently stored in `demo`. That is correct and expected, but
  it means the harness and `demo` diverge until the seed is re-run — which should be said in the PR
  rather than discovered later.
