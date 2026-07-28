# Chart Platform C — Params Validation — Design Spec

**Status:** approved design, pending implementation
**Date:** 2026-07-28
**Sub-project:** C · Backend params validation (layer 5 of the chart platform)
**Parent spec:** `docs/en/specs/2026-07-27-chart-studio-design.md`
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-params-validation-design.md`

## Context

A and B are finished, and the frontend now speaks exactly one params shape (v2:
`{version, spec, settings}`). The backend does not know that shape exists.

`ch_create` and `ch_update` take `params` as an opaque `serde_json::Value` and
write it to `analytics.charts.params` unread. Any JSON is accepted: `{}`, a v1
body, `{"version": 99}`, a `spec` with a string where an array belongs. Nothing
notices until a browser opens the chart, and then the failure lands on whoever
opens it rather than on whoever wrote it.

The *run* path is a different story and is already careful: `chartquery.rs`
resolves every referenced column against the dataset, type-checks filter
operands, rejects an empty `IN`, and clamps `row_limit` to `MAX_CHART_ROWS`
(10 000) with a 30-second timeout. So this sub-project is about the write path,
not the query pipeline.

## Goal

A chart that is stored is a chart the frontend can open. Malformed params are
rejected at the API with a 400 that says what is wrong, instead of being stored
and failing later in someone's browser.

## Scope

**In scope**
- Validate `params` on `POST /charts` and `PUT /charts/{id}`: shape only.
- A typed error that names the offending field.

**Out of scope — deliberately**
- **Column and metric existence.** A dataset legitimately changes after a chart
  is saved; Metabase lets a card go stale rather than block the save. The run
  path already reports unknown columns, which is where that failure belongs.
- **A whitelist of settings keys.** A2 chose flat settings partly so unknown keys
  survive a round-trip through an older client; enforcing a vocabulary here would
  break that on purpose.
- Backfilling or repairing rows already stored (migration `0099` did that).
- **The duplicate path.** `ch_duplicate` copies the row in SQL and never takes params
  from the client, so it cannot introduce a bad one. Validating there would only
  refuse to duplicate a chart that predates the backfill — a usability loss for no
  safety gain.
- The query pipeline: it is already validated, limited and timed out.

## Design

### What "valid" means

```rust
pub struct ChartParams {
    pub version: u32,           // must be 2
    pub spec: QuerySpec,        // must deserialize
    pub settings: serde_json::Map<String, serde_json::Value>,  // must be an object
}
```

Validation is exactly "does this deserialize", plus:

- `version == 2` — a future version is rejected by *this* build rather than
  silently stored for a client that cannot read it.
- `viz_type` is one of the known types. The column is a free-text `text` today,
  so `"bar "` or `"Bar"` is storable and then renders as the default viz.
- `row_limit >= 1`. The run path clamps, but storing `-5` means the builder shows
  a number the query will never use.

Everything else in `settings` is opaque by design: keys are strings, values are
whatever the viz's descriptor put there.

### Where it lives

`egret-bi/src/chartparams.rs`, next to `chartquery.rs`, exporting
`validate(viz_type: &str, params: &serde_json::Value) -> Result<(), BiError>`.
The handlers call it before touching the store; nothing else changes.

Errors are `BiError::BadRequest` with the field named:
`"params.spec.row_limit must be at least 1"`, `"params.version must be 2, got 1"`,
`"unknown viz_type \"Bar\""`.

### Why not a typed body

`CreateChart.params` could be typed as the struct instead of `Value`, letting
serde reject the body. It is left as `Value` because the error serde produces
("missing field `spec` at line 1 column 273") is worse than the one this
validator produces, and because the DTO is shared with the update path and the
OpenAPI schema, where `Value` keeps the generated TypeScript honest about
`settings` being open-ended.

## Testing

- Unit: each rejection (bad version, missing spec, non-object settings, unknown
  viz type, `row_limit < 1`) and the acceptance of a real v2 payload, including
  one carrying settings keys this build does not know.
- Route: `POST /charts` with a v1 body returns 400 and stores nothing.
- Regression: a chart saved by the current frontend still saves.

## Success criteria

- No handler writes `params` it has not validated.
- A v1 body — the shape the app itself wrote three weeks ago — is refused with a
  message naming `version`.
- The frontend needs no change: it already sends v2.

## Risks

- **An API client that still sends v1 breaks.** That is the point, but it is a
  breaking change for anything outside this repo's frontend. Nothing else is
  known to write charts; the OpenAPI surface is generated for this frontend only.
- **`viz_type` is validated against a list that lives in the backend**, so adding
  a chart type now touches two places. The list is small and the alternative —
  accepting anything — is what allows `"Bar"` to be stored today.
