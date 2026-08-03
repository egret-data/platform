# Chart parity PR 05b — the funnel and the pivot — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the funnel readable on a narrow card, and find out why the pivot shows two rows on a
dashboard when it shows fourteen in the editor.

**Architecture:** Two unrelated React components. The funnel is a typography port — Metabase shrinks
its own text on a small card and egret hard-codes it. The pivot is **not** a layout problem at all,
and this plan does not assume what it is.

**Tech stack:** TypeScript, React, Tailwind, vitest.

---

## Correction to the design spec

Workstream 5 says the pivot *"renders correctly in the editor but collapses to header plus
grand-totals row on a dashboard card. Needs to scroll within its card rather than shed its body."*

**Measured, that is wrong on both counts.** It is not shedding a body it has, and making it scroll
would fix nothing.

| | editor | dashboard card |
|---|---|---|
| card | — | 182×232 |
| table size | **846×406** | **269×58** |
| rows | **14** (header + 12 months + grand total) | **2** |
| header row | `order_month \| Doohickey \| Gadget \| Gizmo \| Widget \| Total` | `order_month \| ⟨blank⟩ \| Total` |
| first body row | `2026-01-01 \| 1,939.99 \| 2,717.2 \| 3,439.68 \| 2,997.83 \| 11,094.7` | `Grand totals \| 7,763.7 \| 7,763.7` |
| scroller | `max-h-[60vh] overflow-auto` | `min-h-0 flex-1 overflow-auto px-2 pb-2` |
| scrollHeight / clientHeight | 406 / 406 | **192 / 192** |

Three things follow:

1. **Nothing is clipped.** `scrollHeight === clientHeight` on the dashboard, and the table is 58px
   inside a 192px scroller. There is no hidden body to scroll to.
2. **The column dimension is gone.** The dashboard's header has a blank cell where the editor has
   four category columns.
3. **The numbers differ.** `7,763.7` against an editor grand total over twelve months of roughly
   140,000. This is not the same result set.

So the dashboard path is feeding the pivot **different data**, and the visible "collapse" is what
`PivotTable` correctly renders for it. The defect is upstream of the table.

## What the funnel measurement established

egret's funnel card is 184×234. Leaf boxes, all with `overflow:hidden; text-overflow:ellipsis`:

| element | box |
|---|---|
| `Visited` (step 1 name) | 104×33 |
| `131,320` (step 1 value) | 93×28 |
| `sessions` (metric name) | 93×19 |
| `Added to cart` | **19×33** |
| `42.86 %` | **4×20** |
| `56,280` | **4×20** |

The first step takes 104px of a 152px content box and the other three share what is left.

**egret's structure already matches Metabase's.** `FunnelNormal.styled.tsx` gives its first step
`min-width: unset; width: unset` and every other step `width: 100%; min-width: 20px` — which is
exactly egret's `shrink-0` and `min-w-[20px] flex-1`. The first column being unbounded is not the bug.

**What egret lacks is Metabase's responsive typography.** `FunnelNormal.tsx:106-108`:

```ts
const isNarrow = Boolean(gridSize && gridSize.width < 7);
const isShort  = Boolean(gridSize && gridSize.height <= 5);
const isSmall  = isShort || isNarrow;
```

and the styled components consume them:

| element | normal | narrow |
|---|---|---|
| `Head` (step name) | `1em` | `0.85em` |
| `FunnelStart > Title` (the big number) | `1.72em` | `1.72em × 0.75` = **1.29em** |
| `FunnelStart > Subtitle` (metric name) | `1.72em × 0.6875` = 1.18em | `1.72em × 0.5` = **0.86em** |
| `Info` (share + value per step) | `1.15em` | `0.85em` |
| `Info > Subtitle` | `0.6875em` | `0.875em` |
| root padding (`isSmall`) | `1rem` | `0.5rem` |

egret hard-codes the left column of that table: `p-4`, `text-[1.72em]`, `text-[0.6875em]`,
`text-[1.15em]`.

**Metabase's thresholds are in dashboard GRID units, not pixels**, and egret's funnel receives no size
at all today. Translating the threshold is Task 2's job and must be measured, not guessed.

## Non-goals

- **Porting `funnel.type: "bar"` behaviour.** `StepBars` already exists and is out of scope.
- **Making `PivotTable` itself smaller or denser.** Until the pivot diagnosis lands, no change to the
  table's own rendering is justified.

---

## Task 1: get a Metabase reference for the funnel

**Files:** none in the repo. This configures the harness.

egret's harness has no reference rendering for the funnel: Metabase's card reports *"Which fields do
you want to use?"*, i.e. its visualization settings were never set. Without it, Task 2 has nothing to
compare against and would be porting from source alone.

- [ ] **Step 1: Configure the card**

Metabase card for `MB · Funnel` on dashboard 2. Set `visualization_settings` to name the step
dimension and the measure — `funnel.dimension` and `funnel.metric`, matching the columns its native
SQL returns. Drive it from the already-authenticated browser session
(`PUT /api/card/:id`), as the harness memory describes; do not log in from a script.

- [ ] **Step 2: Measure it at two widths**

Reload `http://localhost:3033/dashboard/2`, wait ~16s, and read the funnel card's leaf boxes with
`getBoundingClientRect()` — the same query that produced egret's table above.

Then **find where Metabase's `isNarrow` flips**: resize the card (or the viewport) until the font
sizes change, and record the card's pixel width on each side of the flip. That number, not
`gridSize.width < 7`, is what egret can act on.

- [ ] **Step 3: Report both measurements before writing any code.**

No commit — this task changes the harness, not the repo.

---

## Task 2: the funnel shrinks its text on a small card

**Files:**
- Modify: `frontend/src/components/chart/FunnelChart.tsx`
- Create: `frontend/src/components/chart/__tests__/FunnelChart.test.tsx` (or extend, if one exists)

- [ ] **Step 1: Write the failing test**

Render `FunnelChart` at the measured card size and assert on **what the DOM does**, not on which class
name was applied — a class assertion passes for a class that does nothing:

```tsx
it("does not starve the later steps at a 184px card", () => {
  const { container } = renderAt(184, 234, <FunnelChart {...funnelProps} />);
  const names = [...container.querySelectorAll("[data-funnel-step-name]")];
  // Measured before the fix: step 1 took 104px and steps 2-4 got 19px each.
  names.slice(1).forEach((n) => expect(n.getBoundingClientRect().width).toBeGreaterThan(40));
});
```

**jsdom does not lay out**, so `getBoundingClientRect()` returns zeros there. Decide how you will
measure and say so: either drive this through the real browser in the Task 5 comparison and assert
something weaker but honest here (e.g. that the font-size actually applied to the value element
differs between a narrow and a wide render), or use the same SSR/real-layout route the ECharts specs
use. **Do not write an assertion that passes under jsdom for the wrong reason** — PR 05's gauge task
found exactly that in `sizeAware.test.ts`, where a case passed only because jsdom's measurer returns 0.

- [ ] **Step 2: Run it and watch it fail.** Report the exact output.

- [ ] **Step 3: Give the component its size, then the typography**

`FunnelChart` currently receives no size. Give it one the way the ECharts builders got theirs in
PR 01 — a `ResizeObserver` on its own root is the natural fit for a plain React component; check how
`EChartsRenderer` obtains its size before inventing a second mechanism.

Then apply the six-row table above. Use the measured pixel threshold from Task 1, and put the number
in one named constant with the measurement recorded next to it.

- [ ] **Step 4: Run the suite**

`cd frontend && npx vitest run src/components/chart` and `npx tsc --noEmit`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart
git commit -m "feat(charts): a funnel shrinks its own text before it starves its steps"
```

---

## Task 3: why does the pivot get different data on a dashboard? — DIAGNOSE ONLY

**Files:** none yet. **Report before changing anything.**

The evidence is in the table at the top of this plan. Establish which of these is true — and it may be
none of them:

1. The dashboard card issues a **different query** from the editor (a row limit, a different
   parameter set, a cached older result).
2. The dashboard passes the pivot **different params** — e.g. the column dimension is absent from the
   dashboard card's stored settings, which would explain the blank header cell exactly.
3. The pivot **engine** receives the full result but folds it differently when some setting is missing.

Useful discriminators:

- Compare the network responses for the same chart id on the two routes — row count and column list.
  Read them from the browser's own network log rather than guessing endpoint names.
- `7,763.7` is the number to explain. Find what subset of the data sums to it; that identifies the
  result set the dashboard actually received.
- The blank header cell means the column dimension resolved to one empty value, not to four
  categories. That is a strong pointer at params rather than at layout.

**Beware the harness.** Three of egret's chart cards were once blank on this dashboard because I had
authored their settings keys wrongly, and PR 03 was inert for a whole task because the harness stored
`temporal_units: {}`. **Check the dashboard card's stored params against the chart's own before
concluding egret has a defect** — if the card carries different settings from the chart, that is a
harness bug and this task ends with fixing the harness and re-measuring, not with a code change.

- [ ] **Step 1: Establish the cause. Report it with the evidence. Do not proceed without checking in.**

---

## Task 4: fix what Task 3 found

**Files:** unknown until Task 3 reports.

- [ ] **Step 1: Write a failing test that asserts the observed defect** — a dashboard-sized pivot shows
  its body rows and its column dimension — **not the mechanism**, so it survives a change of cause.
- [ ] **Step 2: Fix it.**
- [ ] **Step 3:** `cd frontend && npx vitest run src/components/chart` and `npx tsc --noEmit`.
- [ ] **Step 4: Commit.**

If Task 3 concludes the defect is in the harness rather than in egret, **this task becomes: fix the
harness, re-measure, and record in the PR that the spec's pivot item was a harness artefact.** That is
a legitimate outcome and must be reported plainly rather than dressed up as a code fix.

---

## Task 5: the live comparison, and the PR

**This is the acceptance test.** Every PR in this programme has shipped a defect that passed its unit
suite and was caught only here.

- [ ] **Step 1: Gates.** `cd frontend && npx tsc --noEmit && npm run lint && npx vitest run`. Report the
  exact test count.

- [ ] **Step 2: Build and serve.** `npm run build`; a static server on :5174 serves `frontend/dist` and
  proxies `/api`, `/catalog`, `/oauth` to `127.0.0.1:30080` (`serve.py` in the session scratchpad).
  egret's session expires — log in with `admin@egret.local` / `Admin12345!` when redirected.

- [ ] **Step 3: Measure both sides.** egret
  `http://127.0.0.1:5174/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`, Metabase
  `http://localhost:3033/dashboard/2`. Wait ~16s.

Acceptance criteria, each against a number in this document:

1. **Every funnel step's name box is wider than 40px** at the 184px card. Today steps 2–4 get 19px.
2. **Every funnel value and percentage box is wider than 40px.** Today they get 4px.
3. **The funnel's first step still shows its value and metric name** — shrinking must not truncate
   them into ellipses.
4. **The pivot shows its column dimension and its body rows** at the 182×232 card, or — if Task 3
   found a harness artefact — the pivot renders identically to the editor once the harness is fixed.
5. **Nothing else on the dashboard changed.** Re-check the four charts PR 05 fixed (sankey, row,
   gauge, pie) against the tables in that PR.

- [ ] **Step 4: Open the PR.** The body carries the before/after measurements, the correction to the
  spec's pivot item, the Metabase funnel reference obtained in Task 1, and whatever Task 3 found.
  Push and open. **Do not merge.** Stop and report.

---

## Self-review

- **Spec coverage.** Workstream 5's two remaining items: the funnel (Tasks 1–2) and the pivot
  (Tasks 3–4).
- **The spec's pivot description is wrong** and this plan corrects it with measurements rather than
  implementing it. Nothing is clipped, so "make it scroll" would change nothing on screen.
- **Task 3 is deliberately diagnosis-only**, for the same reason PR 05's pie task was: two mechanisms
  that should have prevented the pie's overlap were already present, and guessing which had failed
  would have produced a fix for the wrong thing. Here the numbers say the data differs, which no
  layout change can address.
- **Known soft spot.** Task 2's test cannot use `getBoundingClientRect()` under jsdom, and the plan
  does not settle how to measure. That is stated rather than papered over; an implementer who cannot
  find an honest unit-level assertion should say so and let the Task 5 browser check carry the
  criterion, rather than writing a test that passes for the wrong reason.
