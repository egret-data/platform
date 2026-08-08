# The chart-parity harness

A dashboard named **ZZ Chart Parity** carrying 19 chart types — and, since `#437`, three broken-out
cards — on data identical to Metabase's, so any visual difference between the two products is
attributable to the chart renderer alone. It is what PRs 01–07 of the parity programme measured
against, and what thirty-one PRs since have been compared with.

`scripts/harness/` rebuilds egret's side of it.

## The current baseline

Anything that is not in this list is a drift, and a comparison against an older report should be
checked against this table before it is trusted.

| | |
|---|---|
| egret dashboard | `8a196343-ad93-4968-8040-67598d29c3cb` in `demo`, `width: "full"` |
| cards | **22** (19 viz types + 3 breakout cards), all `4×5` on a 12-column grid |
| card `options` | **`vizOverride` count 0** — every card renders its chart's own saved settings |
| `updated_at` | **2026-08-08T01:03:49.111686Z** (was `2026-08-04T08:48:00.790962Z` at 19 cards) |
| Metabase dashboard | id `2`, 22 dashcards, all `8×6` on a 24-column grid |
| Metabase cards | 40–58 are the 19; **59 `MB · Bar Breakout`, 60 `MB · Area Breakout`, 61 `MB · Bar Breakout Desc`** |

**Do not save or mutate these charts or either dashboard.** They are what every future comparison is
measured against. The three breakout cards were ADDED and no existing card was touched: 18 of the 19
egret charts' stored `params` came back byte-identical after the re-seed, all 19 Metabase dashcards
kept their geometry and settings. The nineteenth is recorded under "Known drift" below.

## The breakout cards, and the blind spot they close

For thirty-one PRs every card left `cartesian.series` unset, so **there was no broken-out chart
anywhere in the instrument**. That is why `#436`'s defect — the same card drawing
`Doohickey / Gadget / Gizmo / Widget` on one page load and `Gizmo / Widget / Gadget / Doohickey` on
the next — survived thirty PRs of side-by-side comparison. Nobody looked and missed it; there was
nothing on either dashboard that could show it.

All three break out **`category` off `MB Month x Category`**: four bands, the same four on both sides
(egret's `VALUES` literal was materialised from Metabase's own result for the query `MB · Pivot`
already runs), and four short ASCII strings sort identically in Trino, in H2 and in JavaScript, so a
difference on screen is the renderer and never the collation. `state` (40+ values) and `vendor` were
rejected for legibility; `order_month` is already the x axis.

| card | what only it covers |
|---|---|
| `MB · Bar Breakout` | grouped bars, no authored sort — the exact shape `#436` was measured on |
| `MB · Area Breakout` | **stacked**, so band order is vertical position and not merely legend order |
| `MB · Bar Breakout Desc` | `ORDER BY category DESC`, the one condition `preserveSeriesOrder` accepts |

**The third is not a duplicate of the first, and the reason is the whole design.** Metabase does not
sort its bands: `getBreakoutDistinctValues` is `Array.from(new Set(rows.map(...)))` — pure arrival
order (`echarts/cartesian/model/series.ts:123`), made deterministic by the query rather than by the
renderer. So on an ascending card, a renderer that ALWAYS sorted would agree with Metabase by
coincidence, and the harness could not tell "egret sorts" apart from "egret honours the query". Only
a card whose engine order is not the sorted order can, and that is what the Desc card is for. Both
are worth having; either alone is half an instrument.

**Left out on purpose.** A COMBO with a breakout: egret's per-series default under a breakout is
positional (`seriesDisplayFor`: `index === 0 ? "line" : "bar"`) and Metabase's is not, so an unpinned
combo card would compare two different defaults and teach a false lesson — and pinning the display
per band on both sides would make the card blind to band order, which was its only reason to exist.
SPLIT PANELS (`#435`): its layout keys on the band index, so it is a real reader of this order, but
four panels inside a `4×5` card give each band about a fifth of ~250px and the comparison would be
about legibility rather than order — and the card cannot be resized, because the uniform `4×5`
geometry is what every PR 01–06 measurement was taken at.

### Verified, and how

Egret at **1600×1000**, the fresh `dist` bundle served through a proxy at `127.0.0.1:5175` (the kind
deployment's own bundle is stale — see "Environment notes"), **six page loads** of the dashboard:
6/6 `Doohickey / Gadget / Gizmo / Widget` on the bar and the area, 6/6
`Widget / Gizmo / Gadget / Doohickey` on the Desc card. Metabase's three question views agree with
each, band for band.

That the card would have CAUGHT `#436` was shown rather than asserted, in two halves:

- **The data half, live.** The pre-`#436` spec for this card (`ORDER BY order_month` alone, because
  `withImplicitBreakoutOrder` took only the x axis) returned **three different band arrival orders in
  ten runs** against Trino. The seeded spec, with both breakouts, returned one order 10/10.
- **The render half, live, on the real pre-`#436` code.** The kind deployment still serves a
  pre-`#433` bundle, and the STUDIO rebuilds a chart's spec from its config rather than reading the
  stored one — so opening `MB · Bar Breakout` there is the pre-`#436` experience exactly. Six loads
  drew `Doohickey / Gizmo / Widget / Gadget` three times and `Gadget / Gizmo / Doohickey / Widget`
  three times. Neither is the sorted order.

`scripts/harness/charts.breakout.test.ts` keeps that proof re-runnable without a live stack: it feeds
the card's own config two different arrival orders and asserts they draw the same today and
differently under `preserveSeriesOrder: true`, which is not an approximation of the old builder —
`sortBands` begins `if (preserve) return bands`, the pre-`#436` expression verbatim.

### Settings keys these cards depend on

Both sides, since a card that stores a subtly different key teaches a false lesson:

| | egret | Metabase |
|---|---|---|
| breakout | `cartesian.series: "category"` | `graph.dimensions: ["ORDER_MONTH", "CATEGORY"]` (second entry) |
| metric | `cartesian.yMetrics: ["revenue"]` | `graph.metrics: ["REVENUE"]` |
| stacking | `cartesian.stacked: true` | `stackable.stack_type: "stacked"` |
| band order | the spec's `orderby` | the card's own SQL `ORDER BY` |

**UPPERCASE on the Metabase side**, because H2 upper-cases unquoted identifiers and the result's
columns really are `ORDER_MONTH` / `CATEGORY` / `REVENUE`. This is the trap that left `MB · Funnel`
showing "Which fields do you want to use?".

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

**Metabase's side.** Its 22 cards and its H2 Sample Database are a separate store that a reset of
egret's Postgres does not touch. Rebuilding them needs Metabase's own card API and dashboard model.

The three breakout cards were therefore **hand-constructed** against that API, the same way the 19
were, and the shape is worth recording because nothing in this repo encodes it:

- `POST /api/card` with `dataset_query` in the MBQL5 shape the existing cards use —
  `{"lib/type": "mbql/query", "database": 1, "stages": [{"lib/type": "mbql.stage/native", "native": "<SQL>"}]}`.
- `PUT /api/dashboard/2` with the FULL `dashcards` list. Existing entries are echoed back with their
  own `id`, geometry and `visualization_settings` untouched; a new one carries a **negative `id`**,
  which is how Metabase is told to create it. The 24-column grid takes `8×6` cards, three per row.
- The dashboard's `updated_at` does NOT move when only its dashcards change, so it cannot be used to
  tell whether a card was added. Count the dashcards.

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

## Resolved since this guide was written

**Category order — fixed, and fixed differently from what this guide proposed.** It used to say bar and
row render in engine order (measured: seven of ten calls one way, three the other), that no descriptor
could express an `orderby`, and that the fix belonged in `sortCategories` as a **descending-series-value**
sort.

All three are now out of date:

- `#394` sorts a nominal axis **ascending by the dimension**, and derives an `orderby` in `buildSpec`
  so the *data* is ordered too — the order Metabase's query processor adds implicitly for every
  breakout column (`add_implicit_clauses.clj:71`), not descending by value.
- `#395` added a sort control to the studio, so `SharedQueryState` **does** carry an `orderby` now.
- `#399` closed the waterfall, the fourth family.

Probed after the last re-seed, ten calls each: **10/10 `Doohickey, Gadget, Gizmo, Widget`** on both bar
and row. Do not go looking for this.

**The scalar family — fixed by running this script.** `MB · Scalar`, `Gauge` and `Progress` used to
store `dimensions: ["category"], row_limit: 1000` and read `rows[0]` of an unordered `GROUP BY`. The
live symptom was two cards on one page load disagreeing: Scalar showing Widget's 423.7K while Gauge
showed Gizmo's 383,061.95. They now read the whole-dataset aggregate **1,510,621.69** against a
2,000,000 bound, 15 of 15 identical calls.

**If you are re-deriving a baseline table, note the viewport.** The `166×184` svg figure quoted in the
parity PRs was taken at a viewport nobody recorded; at 1600×1000 the same cards measure `193×214`.
Fix your viewport, measure before and after in the same one, and say which you used. (Re-confirmed at
`193×214` on the new breakout cards at 1600×1000.)

## Known drift, found and not fixed

- **`MB · Map` gained `map.type: "region"`** on the 2026-08-08 re-seed. Not a change anyone made to
  the card: `#434` added a pin/region discriminator to the map descriptor whose `fromParams` defaults
  an absent key to `"region"`, so the seed now WRITES the default that the renderer was already
  assuming. Behaviourally neutral, and unavoidable short of skipping the card — the harness's whole
  design is that it writes what the app writes. Any re-seed since `#434` would have done it.
- **Metabase draws no series legend on an `8×6` dashcard.** All three breakout cards render their
  bands on the dashboard with no legend at all, while egret draws one inside a `4×5` card. A band
  ORDER comparison must therefore be made on the QUESTION view (`/question/59`), or on the dashboard
  by colour and stack position. Left alone: resizing the Metabase cards would break the geometry the
  earlier measurements were taken at.
- **`MB · Pivot` has no Metabase rendering at all, and never had one.** Card 41 draws
  *"Pivot tables are only supported for questions built in the query builder"* — on the dashboard as
  well as on `/question/41` — because the harness's cards are native SQL. Uppercasing its settings
  (below) did not change this and could not have: the card never reaches the point of reading them.
  This was already recorded in `2026-08-02-chart-parity-05-label-room.md`; it is repeated here
  because it is easy to mistake for the case bug and it is not one. **The pivot's target still has to
  be argued on its own terms.** Its *data* is still trustworthy — egret's `VALUES` literal was
  materialised from the result this very card runs, and the query runs fine; it is only the *pivot
  rendering* Metabase declines.

## The inert-settings bug, and what it cost (2026-08-08)

This guide used to say the original Metabase cards' lowercase column names were harmless — *"They
render correctly … the settings happen to coincide with the default for these result shapes."*
**That was wrong for four of them**, and the four were not visible from the settings alone. They are
fixed now; this section is the record, because the same trap has now bitten this harness twice (the
funnel, then these).

### The mechanism, exactly

H2 upper-cases unquoted identifiers, so every result column really is `CATEGORY` / `ORDER_MONTH` /
`REVENUE`. Metabase resolves a column-name setting with `columnsAreValid`
(`visualizations/lib/utils.ts:35`), which is a **case-sensitive object lookup** —
`colsByName[name]`, plus a strict `col.name === columnName` at render time
(`lib/graph/columns.ts:16`). A name that does not resolve fails the setting's `isValid`, and
`getComputedSetting` (`lib/settings.ts:118`) then falls through to `getDefault`. **The card renders
from Metabase's default and the stored setting is never read.** It does not warn, and it does not
render blank — which is exactly why this survived.

Two consequences worth knowing:

- `graph.dimensions` and `graph.metrics` are validated **together** by
  `getAreDimensionsAndMetricsValid`, so one bad name discards both.
- `pivot_table.column_split` is the exception: it uses `getValue`, not `isValid`/`getDefault`, so its
  stored value is always processed and case-mismatched names are dropped from their partition rather
  than the whole setting falling back.

### The real count: 12, not 19

"Nineteen" counted the original cards wholesale. Card by card, only **12 carried a lowercase column
reference at all**, and of those only **4 rendered differently from what their settings say**:

| | cards | |
|---|---|---|
| lowercase **and visibly wrong** | **46, 47, 56, 58** | scatter, combo, sankey, map |
| lowercase but the default coincided | 42, 43, 44, 45, 48, 53, 55 | bar, row, line, area, pie, waterfall, box plot — all `[dimension, metric]` result shapes, where the default *is* the stored intent |
| lowercase, but inert for a different reason | 41 | pivot — never renders (see "Known drift") |
| no column-naming settings to be inert | 40, 49, 54, 57 | `visualization_settings: {}` |
| settings that name no column | 50, 51 | `progress.goal`, `gauge.range`/`segments` — numbers, always effective |
| already uppercase | 52, 59, 60, 61 | funnel (fixed earlier) + the three breakout cards |

### What the four were actually drawing

Measured at 1600×1000 on `/dashboard/2` and the question views, before and after:

| card | stored settings say | was drawing | now |
|---|---|---|---|
| 47 `MB · Combo` | 2 metrics; `REVENUE` a **line** on the right axis | **3** series (`ORDERS`, `REVENUE`, `AVG_TOTAL`) with `REVENUE` a **bar** — `getDefaultMetrics` takes every numeric column | 2 series, `REVENUE` a line |
| 58 `MB · Map` | shade by `REVENUE` | shaded by **`ORDERS`** — `map.metric`'s default is `cols.find(isMetric)`, the *first* numeric. Legend read `21 – 206 … 1,342 +`, which is ORDERS' exact min/max; REVENUE's range is `1,358.69 – 108,466.6` | legend `1.4k – 12.4k … 108.5k +` |
| 46 `MB · Scatter` | bubbles sized by `ORDERS` | **uniform** bubbles — `getDefaultScatterColumns` returns `bubble: null` | bubbles sized |
| 56 `MB · Sankey` | `SOURCE → CATEGORY` | **`CATEGORY → SOURCE`**, i.e. reversed — `findSensibleSankeyColumns` picked its own | `SOURCE → CATEGORY` |

The other eight cards are byte-identical on screen before and after; only `visualization_settings`
changed on any card, and dashboard 2 still has its 22 dashcards at `8×6` with no dashcard-level
overrides.

### Did this move an egret-side finding? No — and that is the point

**No write-up anywhere in `docs/`, `scripts/harness/` or commits #407–#438 ever called egret
divergent on combo series, scatter bubbles, sankey direction or the map metric.** The harness's only
chart-by-chart comparison table (`2026-08-02-chart-parity-04-colour-assignment.md`) does not cover
combo or map at all, and covers scatter and sankey only for colour and label geometry. So no
recorded conclusion has to be withdrawn.

That is a lucky escape rather than a good outcome. In all four cases **egret was already storing what
Metabase's settings intended** (`scripts/harness/charts.ts:99-122`), so anyone who *had* compared
them would have found egret "wrong" and been comparing against an unconfigured card.

One older measurement is worth flagging as unexplained. `2026-08-02-chart-parity-05-label-room.md:45`
records Metabase's longest right-hand sankey label as **`Doohickey`** — a CATEGORY value, i.e.
`SOURCE → CATEGORY`, the orientation the settings intend and the opposite of what card 56 was drawing
on 2026-08-08. Either the default flipped under it (a stale `result_metadata` refresh would do it) or
the label was mis-attributed. It is not worth chasing now, because the uppercase fix removes the
dependence on Metabase's default-detection entirely — **which is the real argument for fixing an
inert setting even where the default happens to agree: a default can drift, a stored setting cannot.**

### Reproducing it

There is still no script for Metabase's side. The corrected settings are listed here so they can be
re-applied by hand or by any future tool, each via `PUT /api/card/{id}` with just
`{"visualization_settings": …}`:

```json
41 {"pivot_table.column_split": {"rows": ["ORDER_MONTH"], "columns": ["CATEGORY"], "values": ["REVENUE"]}}
42 {"graph.dimensions": ["CATEGORY"],    "graph.metrics": ["REVENUE"]}
43 {"graph.dimensions": ["CATEGORY"],    "graph.metrics": ["REVENUE"]}
44 {"graph.dimensions": ["ORDER_MONTH"], "graph.metrics": ["REVENUE"]}
45 {"graph.dimensions": ["ORDER_MONTH"], "graph.metrics": ["REVENUE"]}
46 {"graph.dimensions": ["PRICE"],       "graph.metrics": ["RATING"], "scatter.bubble": "ORDERS"}
47 {"graph.dimensions": ["ORDER_MONTH"], "graph.metrics": ["ORDERS", "REVENUE"],
    "series_settings": {"REVENUE": {"axis": "right", "display": "line"}}}
48 {"pie.dimension": "CATEGORY", "pie.metric": "REVENUE"}
53 {"graph.dimensions": ["ORDER_MONTH"], "graph.metrics": ["REVENUE"]}
55 {"graph.dimensions": ["CATEGORY"],    "graph.metrics": ["PRICE"]}
56 {"sankey.source": "SOURCE", "sankey.target": "CATEGORY", "sankey.value": "ORDERS"}
58 {"map.dimension": "STATE", "map.metric": "REVENUE", "map.region": "us_states", "map.type": "region"}
```

`series_settings` is keyed by the **metric column name** for a single-card chart
(`getSeriesVizSettingsKey`), so it needs uppercasing too — a lowercase key silently matches nothing
and the series keeps its default colour and display.

**How to tell whether a setting is live, without reading Metabase's source.** Do not check that the
card "looks right" — that is precisely the check that failed here for a year of PRs. Change the
setting to something *wrong* and reload: if the picture does not change, the setting is inert.

## Environment notes

The stack is the **kind** cluster, not docker-compose: the backend is at `127.0.0.1:30080` and
`demo`'s database is `demo-trino-trino.egret-engines.svc:8443`.

If you serve a built frontend on `:5174` to compare in a browser, beware that **Vite's `--strictPort`
does not protect you**: it binds IPv6 `::1` while a Python static server holds IPv4 `127.0.0.1`, so
both start and `127.0.0.1:5174` silently keeps reaching the other one. Check the loaded bundle hash
against `ls -t frontend/dist/assets/index-*.js` before trusting any measurement — the browser caches
`index.html`, and a stale bundle has produced a wrong conclusion more than once.

**The bundle kind serves is not the bundle you just built, and it is much older than it looks.**
Measured 2026-08-08: `:30080` served `index-BWjx3fdn.js`, which carries none of `unpinFromZero`,
`tooltipMetrics` or `splitPanels` — i.e. it predates `#433`, eleven merged PRs behind `main`. Any
"live check" done by opening `:30080` is a check of that build. Date it by grepping the served JS for
`cartesian-codec.ts`'s `KEYS` entries, which are STRING LITERALS and survive minification (a property
name like `preserveSeriesOrder` does too, but a settings key like `cartesian.yMetrics` does NOT — the
codec builds it by concatenation, so grepping for it always returns zero and proves nothing).

A plain static server cannot stand in, because the app is same-origin with its API and the dev
server's proxy points at the docker stack's `:8080` rather than kind's `:30080`. What works is ~60
lines: serve `frontend/dist` and reverse-proxy `/api` to `127.0.0.1:30080` on a port of your own,
bound explicitly to `127.0.0.1`.

Two more things that cost time in the browser:

- The **egret session cookie is not scoped by port**, so one login at `127.0.0.1:5175` is also a
  login at `127.0.0.1:30080`. Useful, given the 5-attempts-per-5-minutes limit.
- Opening a chart in the STUDIO fires a `beforeunload` prompt on the next navigation, because the
  monthly-grouping effect marks it dirty on open. It blocks Playwright's snapshot and evaluate tools
  and does not always clear; opening a **new tab** per load is more reliable than reloading, and
  nothing is saved either way.
