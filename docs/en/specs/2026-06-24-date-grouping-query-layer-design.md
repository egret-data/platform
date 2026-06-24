# Date Grouping — Sub-project A: Query-layer Temporal Units (Design)

**Status:** Approved approach (parallel-map), pending spec review
**Part of:** Date Grouping program (A → B → C). This is **A**, the backend foundation.
**Canonical:** this file (English). Vietnamese reference: `docs/vi/specs/2026-06-24-date-grouping-query-layer-design.md`.

## Goal

Let a chart query group/break out a **temporal dimension** by a **temporal unit** — either by
*truncation* (minute…year, e.g. "by month") or by *extraction* (hour-of-day, day-of-week,
month-of-year, …) — emitting correct, identical-result SQL for **both Trino and StarRocks**.

This sub-project is **backend only**. It does not touch the frontend (sub-project B) or add a
dashboard parameter (sub-project C). When it lands, a caller (today: a hand-built request; later:
B's builder UI) can send a temporal unit per dimension and get correctly bucketed rows back.

## Non-goals

- No builder UI (sub-project B).
- No dashboard-level Date-Grouping parameter (sub-project C).
- No new persisted column-type metadata. Extraction changes a dimension's *result* type to integer;
  the dataset column's stored type is unchanged. Display formatting of extracted integers is B's job.

## Background (current behavior)

`QuerySpec` (`crates/egret-bi/src/chartquery.rs:17`) carries `dimensions: Vec<String>` — bare column
names. `build()` resolves each dimension via `column_sql(engine, c)` (`chartquery.rs:124`) to a quoted
identifier or the column's stored expression, then:

- pushes `"{e} AS {name}"` into the SELECT (line 232),
- collects the same `e` into `dim_exprs`, used verbatim for `GROUP BY` and `GROUP BY GROUPING SETS`
  (lines 243–302),
- `ORDER BY` resolves fields independently via `orderby_sql` (`chartquery.rs:188`),
- filters resolve the raw column via `render_filter` (`chartquery.rs:140`).

There is **no truncation today**: a `timestamp` dimension groups by the exact instant, so "by month"
is impossible. Columns already carry an `is_temporal` flag (`typegen.rs`, `GenericType::Temporal`).

## Wire / API change (backward compatible)

Add one optional field to `QuerySpec`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct QuerySpec {
    // ... existing fields ...
    /// dimension column name -> temporal unit token (snake_case, e.g. "month", "day_of_week").
    /// Empty (default) = no truncation = legacy behavior. Only applies to names that resolve as
    /// dimensions; a unit for an unknown/non-temporal column is a BadRequest at build time.
    #[serde(default)]
    pub temporal_units: std::collections::HashMap<String, String>,
}
```

`#[serde(default)]` means existing saved chart `params.spec` and `ChartDataBody.query_spec` payloads
deserialize unchanged (empty map). The chart `params` JSON flows through serde, so no `model.rs`
change is needed.

## TemporalUnit enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalUnit {
    // truncation → result is a temporal value (date_trunc)
    Minute, Hour, Day, Week, Month, Quarter, Year,
    // extraction → result is an integer
    MinuteOfHour, HourOfDay, DayOfWeek, DayOfMonth, DayOfYear, WeekOfYear, MonthOfYear, QuarterOfYear,
}
```

- `TemporalUnit::parse(token: &str) -> Option<Self>` maps snake_case tokens
  (`"minute"`, `"hour"`, `"day"`, `"week"`, `"month"`, `"quarter"`, `"year"`, `"minute_of_hour"`,
  `"hour_of_day"`, `"day_of_week"`, `"day_of_month"`, `"day_of_year"`, `"week_of_year"`,
  `"month_of_year"`, `"quarter_of_year"`). Unknown token → `None` → `BadRequest`.
- `fn is_extract(self) -> bool` — true for the `*Of*` variants.

This is the exact Metabase temporal-unit set.

## SQL generation: `temporal_expr(engine, inner: &str, unit: TemporalUnit) -> String`

`inner` is the already-resolved column SQL (output of `column_sql`). The helper wraps it.

### Truncation — identical syntax on both engines

Both Trino and StarRocks accept `date_trunc('<unit>', expr)` for minute, hour, day, week, month,
quarter, year:

| unit    | Trino & StarRocks         |
|---------|---------------------------|
| minute  | `date_trunc('minute', e)` |
| hour    | `date_trunc('hour', e)`   |
| day     | `date_trunc('day', e)`    |
| week    | `date_trunc('week', e)`   |
| month   | `date_trunc('month', e)`  |
| quarter | `date_trunc('quarter', e)`|
| year    | `date_trunc('year', e)`   |

Both define `week` as Monday-start, so results match.

### Extraction — per-engine functions, normalized to identical numbering

Extraction function names differ; some also differ in numbering. The helper branches on `engine`
and uses the expressions below so **both engines return the same integers** (ISO, matching Trino &
Metabase):

| unit            | Trino               | StarRocks            | range |
|-----------------|---------------------|----------------------|-------|
| minute_of_hour  | `minute(e)`         | `minute(e)`          | 0–59  |
| hour_of_day     | `hour(e)`           | `hour(e)`            | 0–23  |
| day_of_week     | `day_of_week(e)`    | `(weekday(e) + 1)`   | 1–7 (Mon=1) |
| day_of_month    | `day_of_month(e)`   | `dayofmonth(e)`      | 1–31  |
| day_of_year     | `day_of_year(e)`    | `dayofyear(e)`       | 1–366 |
| week_of_year    | `week_of_year(e)`   | `week(e, 3)`         | 1–53 (ISO) |
| month_of_year   | `month(e)`          | `month(e)`           | 1–12  |
| quarter_of_year | `quarter(e)`        | `quarter(e)`         | 1–4   |

Normalization notes (the only two that need care):
- **day_of_week:** StarRocks `dayofweek()` returns 1=Sun…7=Sat; `weekday()` returns 0=Mon…6=Sun, so
  `weekday(e) + 1` gives 1=Mon…7=Sun, matching Trino's `day_of_week`.
- **week_of_year:** StarRocks `week(e, 3)` selects ISO week mode (Mon-start, week 1 has ≥4 days),
  matching Trino's `week_of_year`.

`Engine` is the existing `egret_database::model::Engine`. A `match engine { Trino => …, StarRocks => … }`
covers both current engines; any other engine variant falls back to the Trino branch (closest to ANSI).

## Application points in `build()`

Introduce one helper and use it everywhere a dimension becomes SQL, so SELECT / GROUP BY / ORDER BY
stay identical (a temporal dimension MUST be grouped by the same wrapped expression it selects, or the
engine rejects the query):

```rust
/// Resolve a dimension name to SQL, applying its temporal unit if one is set.
fn dim_expr(engine: Engine, columns: &[DatasetColumn], spec: &QuerySpec, name: &str)
    -> Result<String, BiError> {
    let c = find_column(columns, name)
        .ok_or_else(|| BiError::BadRequest(format!("unknown dimension: {name}")))?;
    let base = column_sql(engine, c);
    match spec.temporal_units.get(name) {
        None => Ok(base),
        Some(tok) => {
            let unit = TemporalUnit::parse(tok)
                .ok_or_else(|| BiError::BadRequest(format!("unknown temporal unit: {tok}")))?;
            if !c.is_temporal { // is_temporal sourced from column metadata
                return Err(BiError::BadRequest(format!("temporal unit on non-temporal column: {name}")));
            }
            Ok(temporal_expr(engine, &base, unit))
        }
    }
}
```

Wiring:
1. **Dimension loop** (`chartquery.rs:228–235`): replace `column_sql(engine, c)` with
   `dim_expr(engine, columns, spec, name)`. `dim_exprs` then holds wrapped expressions, so **GROUP BY
   and GROUPING SETS are automatically correct** with no further change.
2. **ORDER BY** (`push_orderby` / `orderby_sql`, lines 346–364, 188): pass `spec` (or
   `&spec.temporal_units`) through. When an order-by `field` matches a dimension that has a temporal
   unit, resolve it through the same `dim_expr` so `ORDER BY` matches `GROUP BY`. A field that is a
   metric or an ungrouped column is unchanged.
3. **Filters** (`render_filter`, `push_where`): **unchanged.** Filtering a temporal column uses the
   raw value (a date range is compared against the real timestamp, not the truncated bucket).
4. **Raw-records mode** (non-aggregate, `columns`): unchanged. Temporal units apply only to
   aggregate-mode dimensions; `temporal_units` keyed to a non-dimension is ignored in raw mode.

### `c.is_temporal` availability

`dim_expr` needs the column's `is_temporal`. `DatasetColumn` is the type passed into `build()`; confirm
it exposes `is_temporal` (the value `routes.rs:381` reads as `g.is_temporal()` / `model.rs:75`). If
`DatasetColumn` does not already carry it, derive it via `generic_type(engine, &c.data_type).is_temporal()`
inside `dim_expr` (the same `typegen` path used elsewhere). Either way, no schema change.

## Result type and value coercion

- **Truncation** → temporal value; result column reads back as a date/timestamp, formatted as today.
- **Extraction** → integer; the engine's result-set column type is `bigint`/`int`, so it coerces to a
  JSON number. This relies on result coercion using the **result-set's** reported column type, not the
  dataset's stored type. StarRocks returns `MYSQL_TYPE_*` result-type names (see
  `egret-mysql-text-protocol-coercion` / `egret-starrocks-vs-trino-sql`); Trino returns typed JSON.
  **Verification point:** a manual run of an extraction unit on StarRocks must return integers, not the
  original timestamp type. If coercion is found to key off the dataset column type, that is a bug to fix
  here (coerce by result-set type).

## Testing

**Unit tests** (`chartquery.rs` `#[cfg(test)]`, pure `build()` assertions — no DB):
1. Truncation, Trino: `temporal_units {"created_at":"month"}` → SELECT and GROUP BY both contain
   `date_trunc('month', "created_at")`.
2. Truncation, StarRocks: same input → `date_trunc('month', \`created_at\`)` (engine quoting).
3. Extraction, Trino vs StarRocks: `"day_of_week"` → `day_of_week(...)` for Trino,
   `(weekday(...) + 1)` for StarRocks; `"week_of_year"` → `week_of_year(...)` vs `week(..., 3)`.
4. ORDER BY on a grouped temporal dimension wraps identically to GROUP BY (string-equal exprs).
5. Unknown unit token → `BadRequest`.
6. Temporal unit on a non-temporal column → `BadRequest`.
7. **Regression:** empty `temporal_units` → byte-identical SQL to before (no truncation).

**Manual integration test** (the existing real-Trino + real-StarRocks harness,
`egret-manual-trino-integration-test`): run a `date_trunc('month', …)` group and a `day_of_week`
extraction against both engines on the seeded demo data; assert the same bucket labels and integer
values appear from both. Run before pushing (not in CI).

## Files touched

- `crates/egret-bi/src/chartquery.rs` — `QuerySpec.temporal_units`; `TemporalUnit` enum + `parse` +
  `is_extract`; `temporal_expr`; `dim_expr`; wire into the dimension loop + `push_orderby`/`orderby_sql`;
  unit tests.

No other crate changes. The frontend and dashboard parameter are sub-projects B and C.

## Conventions

- Backend sweep before commit: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi`
  and `cargo fmt --all --check`.
- Run the manual Trino + StarRocks integration test before pushing (query/SQL change on both engines).
- Configured git identity; `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` footer;
  stage explicit paths.
