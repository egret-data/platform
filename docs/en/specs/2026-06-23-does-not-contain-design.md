# Text filter "does-not-contain" (`not_like`) — design

**Status:** approved (brainstorm) · **Date:** 2026-06-23 · **Branch:** `feat/dashboards-does-not-contain` (off main)
**Part of:** the dashboard Metabase-parity program ([audit](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Add the text-filter **does-not-contain** operator, the one filter operator deferred across Tiers 2d/3b because it needs a backend op. It is the negation of `contains`: `contains` maps to `like '%v%'`, so `does-not-contain` maps to a new `not_like '%v%'`. This is the first dashboard-filter feature touching the backend SQL builder.

## Scope

**In:** (1) a `NotLike` variant on the backend `FilterOp` enum + its SQL rendering (`NOT LIKE`); (2) `"not_like"` on the frontend `FilterOp` union; (3) `paramToFilters` text `does-not-contain` → `not_like '%v%'`; (4) `OPERATORS.text` gains `does-not-contain`.

**Out / not applicable:** per-engine branching (NOT LIKE is standard SQL, identical on Trino + StarRocks; `engine` only affects identifier quoting). The chart-builder has **no per-op text-filter picker** (verified — filters there go through the field zone), so there is nothing to extend there; `does-not-contain` is purely a dashboard text-parameter operator. No new migration.

## Existing code (verified)

- Backend `crates/egret-bi/src/chartquery.rs`: `FilterOp` enum (`Eq/Ne/Gt/Ge/Lt/Le/Like/In/NotIn/IsNull/IsNotNull`, `#[serde(rename_all = "snake_case")]`). `render_filter`'s scalar arm builds `format!("{lhs} {op} {lit}")` where `op` comes from a `match f.op` of the scalar ops (`Eq => "="`, …, `Like => "LIKE"`, `_ => unreachable!`). `In/NotIn`, `IsNull/IsNotNull` are handled in earlier match arms. `sql_literal` single-quotes/escapes string values (the security-critical primitive). The op comes from an enum, so no operator injection.
- Frontend `src/api/charts.ts`: `FilterOp = "eq"|…|"like"|"in"|"not_in"|"is_null"|"is_not_null"`; `ValueFilterOp = Exclude<FilterOp, NullFilterOp>` (so adding `not_like` makes it a `ValueFilterOp` automatically). `ChartFilter` value variant `{col, op: ValueFilterOp, value}`.
- Frontend `src/components/chart/dashboard/params.ts`: `paramToFilters` text case maps `is→eq`, `is-not→ne`, `contains→like '%v%'`, `starts-with→like 'v%'`, `ends-with→like '%v'`. `mkf(op, v) => ({col, op, value:v})`.
- Frontend `src/components/chart/dashboard/ParameterEditor.tsx`: `OPERATORS.text = ["is","is-not","contains","starts-with","ends-with"]`.

## Backend change

In `chartquery.rs`:
1. Add `NotLike,` to the `FilterOp` enum (after `Like`). With `#[serde(rename_all = "snake_case")]` it deserializes from `"not_like"`.
2. In `render_filter`'s scalar op `match f.op`, add `FilterOp::NotLike => "NOT LIKE",` (alongside `Like => "LIKE"`).

Result: a `{col, op:"not_like", value:"%v%"}` filter renders `col NOT LIKE '%v%'` for both engines. The value is escaped by `sql_literal` exactly like the `Like` path.

## Frontend change

1. `api/charts.ts`: add `"not_like"` to the `FilterOp` union (no other change — `ValueFilterOp`/`ChartFilter` follow).
2. `params.ts` `paramToFilters` text case: add `case "does-not-contain": return [mkf("not_like", \`%${v}%\`)];` (symmetric to `contains`).
3. `ParameterEditor.tsx`: `OPERATORS.text` += `"does-not-contain"`.
4. `api/dashboards.ts` `ParamOperator`: ensure `"does-not-contain"` is a member of the text-operator group (add it to the union if absent).

## Data flow

```
dashboard text param, operator "does-not-contain", value "foo"
  → paramToFilters → { col, op: "not_like", value: "%foo%" }
  → applyParamsToChart injects into the card's QuerySpec.filters
  → backend render_filter → `col NOT LIKE '%foo%'`
```

## Error handling

- `not_like` with a blank value → `paramToFilters` returns `[]` (the existing `isBlank(value)` guard at the top of the text case covers it).
- A `not_like` value that isn't a string → `sql_literal` stringifies/escapes it like any other (same as `Like` today); no new failure mode.
- Backend rejects unknown ops at deserialize time (enum), so a stale frontend can't inject SQL.

## Testing

- **Backend** (`crates/egret-bi`, the chartquery tests): a test that a `not_like` filter renders `... NOT LIKE '%v%'` (mirror the existing `Like` rendering test; assert the value is quoted/escaped). Run with the engine(s) the existing filter tests use.
- **Frontend** (Vitest): `paramToFilters` `does-not-contain` → `{op:"not_like", value:"%v%"}`; blank → `[]`; `OPERATORS.text` includes `does-not-contain`.
- **Live** (Playwright MCP, side-by-side Metabase, 0 console errors): a dashboard text filter set to does-not-contain narrows a card (rows containing the term disappear). Throwaway dashboard only; do not touch "a1".
- **Full sweep** before each commit — backend: `cargo test -p egret-bi && cargo fmt --all --check`; frontend (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Conventions

Metabase as the standard · clean-room · TDD · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual (en canonical + vi reference) · squash-merge · branch `feat/dashboards-does-not-contain`.
