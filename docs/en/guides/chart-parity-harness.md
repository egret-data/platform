# The chart-parity harness

A dashboard named **ZZ Chart Parity** carrying 19 chart types on data identical to Metabase's, so any
visual difference between the two products is attributable to the chart renderer alone. It is what
PRs 01–07 of the parity programme measured against.

`scripts/harness/` rebuilds egret's side of it.

## Running it

```bash
cd frontend
npx vite-node --config vitest.config.ts ../scripts/harness/seed.ts -- --workspace=<slug>
```

Flags: `--workspace` (required), `--database`, `--recreate` (delete and recreate rather than upsert;
**off by default**), `--base`, `--email`, `--password`.

It is idempotent. Datasets are reused, charts are updated in place, and the dashboard PUT is a full
replace, so running it twice does not produce 38 charts and does not churn chart IDs. It **refuses**
rather than guessing when a dataset's SQL has drifted or a chart has moved to a different dataset.

The last thing it does is re-run a known number against the live instance — `2026-01 → 205 orders /
11,094.7` — and fail if it does not match.

## What a fresh workspace needs first

Two steps the script does not do for you, both of which will otherwise make it fail in a confusing way:

1. **Enable the features.** A new workspace is created with `features: []`; it needs `datasets`,
   `charts` and `dashboards`.
2. **Attach the engine.** `PUT /control-plane/engines/{id}/workspaces` is a **full-set replace** — send
   the existing workspaces alongside the new one, or the omitted ones silently lose their engine.

Note also that `DELETE /api/admin/workspaces/{id}` is a **soft delete** (`status = 'archived'`). There
is no purge endpoint; an archived row stays in the admin list.

## Why params are generated, not written

The harness lied **five times** while it was maintained by hand, and each one cost a PR real work:

| # | how | found in |
|---|---|---|
| 1 | wrong settings keys — `pie.dimension` singular, `waterfall.dimension`, box plot given a metric | building it |
| 2 | `temporal_units: {}` on every chart, making a whole PR task inert | PR 03 |
| 3 | `params.spec.rollup_groups: []` on the pivot — the editor rebuilt the spec from settings and rendered right, the dashboard read the stored spec and rendered one wrong cell | PR 05b |
| 4 | v2 params with `settings: {}` and a populated `spec` — three of four table/object charts opened on "Set up your Table" | PR 06 |
| 5 | `MB · Scalar` / `Gauge` / `Progress` storing `dimensions: ["category"], row_limit: 1000` where the descriptor builds `oneRow` — so they read `rows[0]` of an **unordered** four-row result and displayed a **different category between page loads** | PR 07 |

All five are the same mistake: **the harness wrote chart params by hand; the app writes them through
its viz descriptors.** `scripts/harness/params.ts` closes that by calling the app's own
`getDescriptor()` / `buildSpec` / `toSettings` / `mergeVizSettings`, so `harnessParams(viz, config)` is
byte-identical to what the editor's `paramsForSave()` would store. Traps 1, 3, 4 and 5 become
unreachable rather than documented; the generator **throws** if a config produces no `<prefix>.*`
settings, which is trap 4 as an executable invariant.

Trap 2 is the one that resisted: the monthly default for a temporal dimension lived in
`ChartExplore`'s `useEffect`, not in any descriptor. It is now shared as
`withDefaultTemporalUnits()` in `frontend/src/components/chart/temporal-units.ts` and called from both.

## What the script does NOT cover

**Metabase's side.** Its 19 cards and its H2 Sample Database are a separate store that a reset of
egret's Postgres does not touch. Rebuilding them needs Metabase's own card API and dashboard model.

What *is* covered is the **data**: the nine datasets are Trino `VALUES` literals materialised from
query results pulled through Metabase's `/api/dataset`, committed in `datasets.ts` as data, so the
script needs no live Metabase.

Traps on the Metabase side, recorded because they cost time:

- **H2 upper-cases unquoted identifiers.** A card's `visualization_settings` naming lowercase columns
  fails silently — that is why `MB · Funnel` showed "Which fields do you want to use?" until it was
  fixed with `{"funnel.dimension": "STEP", "funnel.metric": "SESSIONS"}`.
- **H2 wants the whole select expression in `GROUP BY`**, not the inner function call.
- A gauge needs `gauge.segments` as well as `gauge.range`; the range alone leaves auto-derived segment
  labels on the arc.
- Send `result_metadata: null` on a card PUT that changes the SQL, or Metabase keeps the stale column
  metadata.

## API-level traps the script encodes

No descriptor knows about these, so they are enforced in the script itself:

- Chart metrics must be **declared dataset metrics** (`POST .../datasets/{id}/metrics` with an
  expression such as `SUM(revenue)`), not raw column names, or the chart returns `unknown metric`.
  `charts.ts` derives each chart's metric list from the same `buildSpec` that writes the saved spec, so
  the invariant cannot check a stale hand-written list.
- Dataset creation takes `source: {kind: "virtual", sql}` — `kind: "sql"` is rejected 422.
- The dashboard PUT is a full replace and needs `name` alongside `cards`, whose geometry fields are
  `grid_col` / `grid_row` / `size_x` / `size_y`.
- egret login is rate-limited **5 per 5 minutes**. The script logs in once.

## Known, and not fixed here

**Bar and row charts render their categories in whatever order the engine returned.** Every grouped
chart's spec carries `orderby: []`, and no descriptor can express one — `SharedQueryState` is
`{filters, rowLimit, temporalUnits}`, and every `buildSpec` hardcodes `orderby: []`. Nine of the twelve
grouped charts are protected by a client-side sort they apply themselves (temporal axes, pie, sankey,
funnel, pivot); **bar and row are not**, because `cartesian.ts` deliberately leaves a nominal axis in
first-seen order.

Measured on the endpoint the dashboard uses, ten calls: seven returned
`Gizmo, Widget, Gadget, Doohickey` and three returned `Widget, Gadget, Doohickey, Gizmo`. Three
consecutive page loads all showed the 70% ordering, which is why a two-load comparison called it
deterministic and it survived PRs 02–06.

The fix belongs in `sortCategories`, not in this script — sorting a nominal axis by descending series
value, which is what `pie.ts` and `sankey.ts` already do and what Metabase's own `MB By Category`
question does. Fixing it in the seed would make two charts on one dashboard deterministic and leave
every user's bar chart flipping.

## Environment notes

The stack is the **kind** cluster, not docker-compose: the backend is at `127.0.0.1:30080` and
`demo`'s database is `demo-trino-trino.egret-engines.svc:8443`.

If you serve a built frontend on `:5174` to compare in a browser, beware that **Vite's `--strictPort`
does not protect you**: it binds IPv6 `::1` while a Python static server holds IPv4 `127.0.0.1`, so
both start and `127.0.0.1:5174` silently keeps reaching the other one. Check the loaded bundle hash
against `ls -t frontend/dist/assets/index-*.js` before trusting any measurement — the browser caches
`index.html`, and a stale bundle has produced a wrong conclusion more than once.
