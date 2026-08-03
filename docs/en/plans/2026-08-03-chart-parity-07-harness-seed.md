# Chart parity PR 07 — the harness seed script — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Commit a script that rebuilds the nine datasets and the 19-chart **ZZ Chart Parity**
dashboard, so the comparison survives a database reset and anyone can reproduce it.

**Architecture:** A TypeScript script run with `vite-node`, generating chart params **through the
app's own viz descriptors** rather than hand-writing them. That is the whole design, and the reason
is below.

**Tech stack:** TypeScript, `vite-node` (already present via vitest — no new dependency), the egret
REST API.

---

## The spec says "encode the traps". Make most of them unreachable instead.

Workstream 7 lists four traps to encode. Since it was written, the harness has lied **four more
times**, and every one has the same root cause: **the harness writes chart params by hand, and the app
writes them through its descriptors.**

| # | how the harness lied | found in |
|---|---|---|
| 1 | wrong settings keys — `pie.dimension` singular, `waterfall.dimension`, box plot given a metric | building it |
| 2 | `temporal_units: {}` on every chart, making a whole PR inert for a task | PR 03 |
| 3 | `params.spec.rollup_groups: []` on the pivot — editor rebuilt the spec from settings and rendered right, the dashboard read the stored spec and rendered one wrong cell | PR 05b |
| 4 | v2 params with `settings: {}` and a populated `spec` — three of four table/object charts opened on "Set up your Table" | PR 06 |

Numbers 1–4 are all the same mistake wearing different clothes. Encoding each as a rule in a Python
script leaves the *next* variant free to appear.

**Verified feasible before writing this plan.** `frontend/src/components/chart/viz/registry.ts`
exports `getDescriptor(viz)`; `buildSpec`, `toSettings` and `mergeVizSettings` are plain functions with
no React dependency at the call site. `vite-node` is already on disk via vitest. A probe:

```ts
import { getDescriptor } from "@/components/chart/viz/registry";
const d = getDescriptor("pie")!;
const cfg = { ...d.defaultConfig, dimensions: ["category"], metric: "revenue" };
d.toSettings(cfg);
//  → {"pie.dimensions":["category"],"pie.metric":"revenue"}
d.buildSpec(cfg, { filters: [], rowLimit: undefined, temporalUnits: {} });
//  → {"metrics":["revenue"],"dimensions":["category"],"columns":[],"filters":[],
//     "orderby":[],"rollup_groups":[],"temporal_units":{}}
```

`pie.dimensions` — the plural key that trap 1 is about — comes out correct without anyone knowing it
was a trap. So does `rollup_groups`, which is trap 3.

**Traps that stay API-level and DO need encoding**, because no descriptor knows about them:

- Chart metrics must be **declared dataset metrics** (`POST .../datasets/{id}/metrics` with an
  expression such as `SUM(revenue)`), not raw column names, or the chart returns `unknown metric`.
- Dataset creation takes `source: {kind: "virtual", sql}` — `kind: "sql"` is rejected 422.
- The dashboard PUT is a **full replace** and needs `name` alongside `cards`, whose geometry fields
  are `grid_col` / `grid_row` / `size_x` / `size_y`.
- egret login is rate-limited **5 per 5 minutes**.

## Scope

**egret's side only.** Metabase's own 19 cards and its H2 Sample Database are a separate store that a
reset of egret's Postgres does not touch, and rebuilding them needs Metabase's card API and its own
dashboard model. Out of scope, stated in the script's header.

What *is* in scope from the Metabase side: the **data**. The nine datasets are Trino `VALUES` literals
materialised from query results pulled through Metabase's `/api/dataset`. Those literals go in the
repo as data, so the script needs no live Metabase.

**One Metabase-side trap to record in the docs even though it is out of scope**: Metabase's H2
upper-cases unquoted identifiers, so a card's `visualization_settings` naming lowercase columns
silently fails to render — that is why `MB · Funnel` showed "Which fields do you want to use?" until
PR 05b fixed it.

---

## Task 1: params generated the way the editor generates them

**Files:**
- Create: `scripts/harness/params.ts`
- Create: `scripts/harness/__tests__/params.test.ts`

- [ ] **Step 1: Write the failing test**

The contract is byte-identity with `paramsForSave()` (`frontend/src/routes/explore/ChartExplore.tsx:269-284`):

```ts
{
  version: 2,
  spec: descriptor.buildSpec(config, { filters, rowLimit, temporalUnits }),
  settings: {
    ...mergeVizSettings(settings, descriptor, config),
    column_settings: columnSettings,
    conditional_formatting: conditionalFormatting,
  },
}
```

Assert that for a pie, a pivot and a table the generator's output equals what those three calls
produce — **and that `settings` is never `{}`**, which is trap 4 stated as a property.

**`temporalUnits` is the trap that is NOT in the descriptor.** `ChartExplore` defaults a temporal
dimension to `"month"` in a `useEffect` (around line 162), i.e. in the component, not in
`buildSpec`. A headless caller gets `{}` unless it applies that rule itself. **Write a test that a
chart with a temporal dimension comes out with `temporal_units: {"<dim>": "month"}`** — that is trap 2,
and it is the one place where "encode the rule" is still the answer. Say in a comment that the rule is
duplicated from `ChartExplore` and why it could not be shared.

- [ ] **Step 2: Run it and watch it fail.** `cd frontend && npx vitest run ../scripts/harness` (adjust
  the path to wherever the test lands — check `vitest.config` includes it, and say what you changed if
  it did not).

- [ ] **Step 3: Implement.** Import `getDescriptor` and `mergeVizSettings`; do not reimplement either.

- [ ] **Step 4: Commit**

```bash
git add scripts/harness frontend
git commit -m "feat(harness): chart params come from the app's own descriptors"
```

---

## Task 2: the datasets and their metrics

**Files:**
- Create: `scripts/harness/datasets.ts` — the nine `VALUES` literals and the metric declarations
- Create: `scripts/harness/api.ts` — a thin API client

- [ ] **Step 1:** Write `api.ts`: login (respecting the 5-per-5-minutes limit — log in **once** and
  reuse the session), and typed helpers for dataset create, metric create, chart create, dashboard PUT.
  Read the existing `scripts/seed_ecommerce_clone.py` for the endpoints and the auth shape; it is the
  house precedent even though its params approach is the one this PR replaces.

- [ ] **Step 2:** Move the nine datasets' SQL into `datasets.ts`. They are Trino `VALUES` literals built
  from Metabase's own query results: `MB Monthly`, `MB By Category`, `MB Month x Category`,
  `MB Scatter`, `MB By State`, `MB Sankey`, `MB Raw Orders`, `MB Product Prices`, `MB Funnel`.

  **They must be recovered from the running instance**, not retyped — read them back from the API
  (`GET .../datasets`) and commit exactly what is there, so the seeded data is byte-identical to the
  data every measurement in PRs 01–06 was taken against. Verify by re-running one chart and comparing
  a known number: `2026-01 → 205 orders / 11,094.7`.

- [ ] **Step 3:** Declare the metrics. Every metric a chart uses must exist as a dataset metric with an
  expression (`SUM(revenue)`), or the chart returns `unknown metric`. Encode that as a checked
  invariant, not a comment: the script should fail loudly if a chart names a metric no dataset declares.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(harness): the nine datasets and the metrics the charts need"
```

---

## Task 3: the 19 charts and the dashboard

**Files:**
- Create: `scripts/harness/charts.ts` — one entry per chart: name, dataset, viz, config, grid position
- Create: `scripts/harness/seed.ts` — the entry point

- [ ] **Step 1:** Each chart entry carries a **descriptor config**, not settings and not a spec —
  the shapes `defaultConfig` describes. Task 1's generator turns them into params.

  Recover the 19 charts' current configs from the running instance rather than retyping them, for the
  same reason as the datasets: the measurements in PRs 01–06 were taken against these.

  **Watch for the ones the harness got wrong and that later PRs repaired** — the pivot's
  `rollup_groups`, `MB · Trend`'s and the pivot's `temporal_units`, and the three table/object charts'
  empty `settings`. Read the *current* state, which PR 05b and PR 06 fixed, not what was originally
  seeded. If any chart still carries a shape the descriptor would not produce, **report it** — that is
  a fifth harness lie still in place.

- [ ] **Step 2:** The dashboard. The PUT is a **full replace**: it needs `name` alongside `cards`, and
  each card carries `grid_col` / `grid_row` / `size_x` / `size_y`. Recover the current geometry so the
  card sizes every measurement was taken at are preserved — the 166×184 svg that PRs 02–05 measured is
  a function of this geometry.

- [ ] **Step 3:** Make it idempotent. Running it twice must not produce 38 charts. Decide how — delete
  and recreate, or upsert by name — and say which and why. **Deleting a dashboard someone is looking at
  is destructive**, so if you choose delete-and-recreate, gate it behind an explicit flag and make the
  default safe.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(harness): seed the 19-chart parity dashboard"
```

---

## Task 4: prove it by rebuilding, not by reading

**This is the acceptance test.** A seed script that has never been run against an empty workspace is a
document, not a script.

- [ ] **Step 1:** Run it into a **fresh workspace** — not `demo`, so the existing harness survives if
  the script is wrong. Report the exact command and its output.

- [ ] **Step 2:** Compare the two side by side in the browser. For each of the 19 charts, read the DOM
  and compare against the existing `demo` dashboard: the same labels, the same colours, the same card
  sizes. **Check the bundle hash** against `ls -t frontend/dist/assets/index-*.js` first — the browser
  caches `index.html` and that produced a wrong conclusion in PR 05b.

  Specifically re-check the four things earlier PRs had to repair, because they are what a hand-written
  seed gets wrong: the pivot renders 14 rows with four category columns; `MB · Table` opens with its ten
  columns; the line/area/combo/waterfall x-axes read `January 2026`; the sankey's nine nodes have nine
  distinct colours.

- [ ] **Step 3:** Report any difference. **A difference is a finding, not a failure** — it means the
  hand-built harness and the app disagree somewhere, which is exactly what this PR exists to expose.

- [ ] **Step 4:** Delete the scratch workspace, or say why you kept it.

---

## Task 5: the docs, and the PR

- [ ] **Step 1:** Write `docs/en/` + `docs/vi/` pages: how to run it, what it builds, what it does not
  (Metabase's side), and the API-level traps it encodes. Record the four harness lies and how the
  descriptor route makes 1, 3 and 4 unreachable while 2 is encoded explicitly.

- [ ] **Step 2: Gates.** `cd frontend && npx tsc --noEmit && npm run lint && npx vitest run`. Report the
  exact test count. If the script lives outside `frontend/`, say how it is type-checked and linted, and
  make sure it is — an unchecked script rots.

- [ ] **Step 3: Open the PR.** Body carries: the design argument (generate, don't hand-write), the four
  harness lies with where each was found, the Task 4 rebuild comparison, and anything Task 3 Step 1
  found still wrong in the current harness. Push and open. **Do not merge.** Stop and report.

---

## Self-review

- **Spec coverage.** Workstream 7's four traps: metrics (Task 2 Step 3, as a checked invariant),
  settings keys (unreachable via Task 1), virtual source (Task 2 Step 1), dashboard PUT (Task 3 Step 2).
- **The plan deliberately diverges from the spec's approach.** The spec says encode the traps; four
  more traps have appeared since, all the same mistake. Generating params through the descriptors makes
  three of the original four and three of the four new ones unreachable rather than documented.
- **One trap resists the design and the plan says so.** The temporal-month default lives in
  `ChartExplore`'s `useEffect`, not in any descriptor, so a headless caller must duplicate it. That
  duplication is the plan's weakest point; an implementer who finds a way to share it instead should
  take it and say so.
- **Known soft spot.** Task 3 Step 1 asks the implementer to recover 19 chart configs from a running
  instance and notice any that a descriptor would not produce. If more than two are wrong, that is a
  finding worth its own report before the script is finished — say so rather than quietly normalising
  them, because a normalised harness silently changes what future measurements compare against.
