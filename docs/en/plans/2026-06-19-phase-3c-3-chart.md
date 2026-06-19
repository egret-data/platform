# Phase 3c-3 — Chart (Table + Pivot Table) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users build, save, and view charts over a dataset — rendered as a Table or a Pivot Table — by turning the dataset's metrics (measures) and columns/calc-columns (dimensions) into `GROUP BY` SQL, running it through the existing query engine, and rendering the result.

**Architecture:** Extend `egret-bi` with a pure query builder (`chartquery.rs`: `QuerySpec → SQL`, incl. `ROLLUP` for pivot totals), a `charts` table + `ChartRepo`, and `chart_router`. `POST /charts/data` builds SQL and returns `{ sql, database_id }`; the frontend submits it through the existing `/queries` engine endpoint and polls — reusing SQL Lab's submit/poll hooks. Frontend adds an Explore page, a TanStack-based Table, a hand-rolled Pivot, and a Charts list.

**Tech Stack:** Rust (axum 0.8, sqlx 0.8 runtime, thiserror), Postgres; React 18 + Vite + `@tanstack/react-query` + `@tanstack/react-table` (new), shadcn/ui, Vitest.

**Spec:** `docs/en/specs/2026-06-19-phase-3c-3-chart-design.md`.

**Branch:** `feat/phase-3c-3-chart` (already created; spec committed).

**Conventions (do not violate):**
- Plain `git commit` with the configured identity — never pass `--author`.
- Runtime sqlx only (`sqlx::query`/`query_as`), never the `query!`/`query_as!` macros.
- DB-gated tests self-skip when `DATABASE_URL` is unset; engine-gated tests self-skip when `EGRET_TEST_TRINO_URL` is unset. Mirror the existing patterns in `crates/egret-bi/tests/`.
- Identifiers/expressions are dataset-sourced (trusted); only filter *values* are untrusted — they MUST pass through `sql_literal`.

---

## File Structure

**Backend (crate `egret-bi`):**
- Create `crates/egret-bi/src/chartquery.rs` — `QuerySpec`, `Filter`, `OrderBy`, `FilterOp`, `sql_literal`, `render_filter`, `build()`. Pure, no I/O.
- Create `crates/egret-bi/src/chart.rs` — `Chart` row + `ChartRepo` CRUD.
- Modify `crates/egret-bi/src/schema.rs` — add `pub(crate) fn quote_ident(engine, ident)`.
- Modify `crates/egret-bi/src/expr.rs` — make `source_ref` reusable (`pub(crate)`, take `Engine`).
- Modify `crates/egret-bi/src/model.rs` — `ChartDto`, `CreateChart`, `UpdateChart`, `ChartDataBody`, `ChartDataResponse`, `chart_dto`.
- Modify `crates/egret-bi/src/routes.rs` — `chart_router()` + handlers + ownership guard; add `charts: ChartRepo` to `BiState`.
- Modify `crates/egret-bi/src/lib.rs` — module decls + re-exports.
- Create `migrations/0014_chart.sql`.
- Modify `crates/egret-bin/src/main.rs` — construct `charts` in `BiState`.
- Modify `crates/egret-server/src/lib.rs` — merge `chart_router()` into the BI routes.
- Modify `crates/egret-bi/tests/http.rs` — add `charts` to the test `BiState`; chart CRUD + chart_data HTTP tests.
- Create `crates/egret-bi/tests/chart_engine.rs` — engine-gated Trino smoke (table + rollup).

**Frontend:**
- Create `frontend/src/api/charts.ts` — types + CRUD hooks + `useBuildChartData`.
- Create `frontend/src/components/chart/ChartTable.tsx` — TanStack table renderer.
- Create `frontend/src/components/chart/PivotTable.tsx` — `buildPivot` transform + grid.
- Create `frontend/src/routes/explore/ChartExplore.tsx` — explore page (controls + preview + save).
- Modify `frontend/src/routes/explore/Charts.tsx` — replace placeholder with the list.
- Modify the explore router (wherever `Charts`/`DatasetDetail` routes are registered) to add `charts/new` and `charts/:id`.
- Create `frontend/src/components/chart/__tests__/pivot.test.ts` and `.../ChartTable.test.tsx`.
- Modify `frontend/package.json` — add `@tanstack/react-table`.

---

## Task 1: Query builder — types, `sql_literal`, filter rendering

**Files:**
- Create: `crates/egret-bi/src/chartquery.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Modify: `crates/egret-bi/src/schema.rs`

- [ ] **Step 1: Add the per-engine identifier quoter**

In `crates/egret-bi/src/schema.rs`, add below `quote_mysql`:

```rust
/// Quote an identifier for the given engine.
pub(crate) fn quote_ident(engine: Engine, ident: &str) -> String {
    match engine {
        Engine::Trino => quote_trino(ident),
        Engine::StarRocks => quote_mysql(ident),
    }
}
```

`Engine` is already imported in `schema.rs` (`use egret_database::model::Engine;`).

- [ ] **Step 2: Write the failing test for `sql_literal`**

Create `crates/egret-bi/src/chartquery.rs` with only the types + `sql_literal` + a test module:

```rust
//! Pure chart query builder: QuerySpec → SQL. No I/O. Filter values are the only
//! untrusted input and pass through `sql_literal`; identifiers/expressions are
//! dataset-sourced and trusted.

use egret_database::model::Engine;
use serde::Deserialize;

use crate::dataset::{DatasetColumn, Metric, SourceSpec};
use crate::error::BiError;
use crate::expr::source_ref_for;
use crate::schema::quote_ident;

/// Hard ceiling on rows a chart query may request.
pub const MAX_CHART_ROWS: i64 = 10_000;

#[derive(Debug, Clone, Deserialize)]
pub struct QuerySpec {
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub orderby: Vec<OrderBy>,
    pub row_limit: i64,
    #[serde(default)]
    pub rollup: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    pub col: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderBy {
    pub field: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Like,
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

/// Encode a filter value as a SQL literal, validated by the target column's generic type.
/// Numerics are validated and emitted unquoted; booleans normalized; everything else is
/// single-quoted with `'` doubled. This is the security-critical primitive.
pub(crate) fn sql_literal(v: &serde_json::Value, type_generic: &str) -> Result<String, BiError> {
    match type_generic {
        "numeric" => {
            if let Some(n) = v.as_f64() {
                Ok(n.to_string())
            } else if let Some(s) = v.as_str() {
                s.parse::<f64>()
                    .map(|n| n.to_string())
                    .map_err(|_| BiError::BadRequest(format!("not a number: {s}")))
            } else {
                Err(BiError::BadRequest("expected a numeric filter value".into()))
            }
        }
        "boolean" => {
            let b = match v {
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::String(s) => match s.to_ascii_lowercase().as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => return Err(BiError::BadRequest(format!("not a boolean: {s}"))),
                },
                _ => return Err(BiError::BadRequest("expected a boolean filter value".into())),
            };
            Ok(if b { "TRUE".into() } else { "FALSE".into() })
        }
        _ => {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => {
                    return Err(BiError::BadRequest("filter value is null".into()))
                }
                other => other.to_string(),
            };
            Ok(format!("'{}'", s.replace('\'', "''")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sql_literal_encodes_by_type() {
        assert_eq!(sql_literal(&json!(42), "numeric").unwrap(), "42");
        assert_eq!(sql_literal(&json!("42"), "numeric").unwrap(), "42");
        assert!(sql_literal(&json!("nope"), "numeric").is_err());
        assert_eq!(sql_literal(&json!(true), "boolean").unwrap(), "TRUE");
        assert_eq!(sql_literal(&json!("hi"), "string").unwrap(), "'hi'");
        // injection attempt stays safely quoted
        assert_eq!(
            sql_literal(&json!("a' OR '1'='1"), "string").unwrap(),
            "'a'' OR ''1''=''1'"
        );
    }
}
```

Add to `crates/egret-bi/src/lib.rs`: `mod chartquery;` (after `mod chart;` is added in Task 3, but adding it now alone is fine) and leave re-exports for Task 4. For now add just:

```rust
mod chartquery;
```

Note: `source_ref_for` is created in Step 4 below; until then the `use crate::expr::source_ref_for;` line will not compile. Comment it out for this step, OR do Step 4 first. To keep the test compiling now, temporarily remove the two `use crate::expr::...` / `use crate::schema::quote_ident;` lines and the unused imports — re-add them in Task 2. Simplest: keep this task focused on `sql_literal`, so at the top of the file keep only:

```rust
use serde::Deserialize;
use crate::error::BiError;
```

and add the other `use` lines in Task 2 when `build()` needs them.

- [ ] **Step 3: Run the test to verify it fails to compile / fails**

Run: `cargo test -p egret-bi chartquery::tests::sql_literal_encodes_by_type`
Expected: FAIL (module/test newly added; if you wrote `sql_literal` already it should PASS — that's fine, this is a pure function). If it fails to compile due to unused imports, trim imports as noted.

- [ ] **Step 4: Make `source_ref` reusable for the builder**

In `crates/egret-bi/src/expr.rs`, refactor the private `source_ref(spec, source)` into a `pub(crate)` function keyed on `Engine` so `chartquery` can reuse it, and keep the existing caller working. Replace the existing `fn source_ref` with:

```rust
/// Build the `FROM <source>` reference for `engine`.
pub(crate) fn source_ref_for(engine: Engine, source: &SourceSpec) -> String {
    match source {
        SourceSpec::Physical {
            catalog,
            schema,
            table,
        } => match engine {
            Engine::Trino => match catalog.as_deref() {
                Some(cat) if !cat.is_empty() => format!(
                    "{}.{}.{}",
                    quote_trino(cat),
                    quote_trino(schema),
                    quote_trino(table)
                ),
                _ => format!("{}.{}", quote_trino(schema), quote_trino(table)),
            },
            Engine::StarRocks => format!("{}.{}", quote_mysql(schema), quote_mysql(table)),
        },
        SourceSpec::Virtual { sql } => format!("(\n{sql}\n) AS _egret_src"),
    }
}
```

Then update `validate_expression` to call it: replace `source_ref(spec, source)` with `source_ref_for(spec.engine, source)`. Remove the now-unused old `fn source_ref`.

- [ ] **Step 5: Run the existing expr tests to confirm no regression**

Run: `cargo test -p egret-bi expr::`
Expected: PASS (the guard tests are unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-bi/src/chartquery.rs crates/egret-bi/src/schema.rs crates/egret-bi/src/expr.rs crates/egret-bi/src/lib.rs
git commit -m "feat(bi): chart query builder scaffolding — sql_literal + reusable source_ref"
```

---

## Task 2: Query builder — `build()` (aggregate / raw / rollup) + filters

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs`

- [ ] **Step 1: Write the failing tests for `build()`**

Append to the `tests` module in `chartquery.rs`:

```rust
fn col(name: &str, tg: &str, expr: Option<&str>) -> DatasetColumn {
    DatasetColumn {
        column_name: name.into(),
        data_type: "x".into(),
        type_generic: tg.into(),
        is_temporal: false,
        ordinal: 0,
        expression: expr.map(|e| e.into()),
        verbose_name: String::new(),
        description: String::new(),
    }
}
fn metric(name: &str, expr: &str) -> Metric {
    Metric {
        id: uuid::Uuid::nil(),
        metric_name: name.into(),
        expression: expr.into(),
        data_type: "double".into(),
        type_generic: "numeric".into(),
        verbose_name: String::new(),
        description: String::new(),
        ordinal: 0,
    }
}
fn phys() -> SourceSpec {
    SourceSpec::Physical {
        catalog: Some("c".into()),
        schema: "s".into(),
        table: "t".into(),
    }
}

#[test]
fn build_aggregate_groups_by_dimension_expression() {
    let cols = vec![col("region", "string", None), col("priced", "numeric", Some("price/100"))];
    let metrics = vec![metric("revenue", "SUM(price)")];
    let spec = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["region".into(), "priced".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 100,
        rollup: false,
    };
    let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
    assert!(sql.contains("\"region\" AS \"region\""), "{sql}");
    assert!(sql.contains("price/100 AS \"priced\""), "{sql}");
    assert!(sql.contains("SUM(price) AS \"revenue\""), "{sql}");
    assert!(sql.contains("GROUP BY \"region\", price/100"), "{sql}");
    assert!(sql.contains("LIMIT 100"), "{sql}");
}

#[test]
fn build_rollup_adds_grouping_markers() {
    let cols = vec![col("region", "string", None), col("seg", "string", None)];
    let metrics = vec![metric("revenue", "SUM(price)")];
    let spec = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["region".into(), "seg".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 100,
        rollup: true,
    };
    let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
    assert!(sql.contains("GROUP BY ROLLUP(\"region\", \"seg\")"), "{sql}");
    assert!(sql.contains("GROUPING(\"region\") AS __grp_0"), "{sql}");
    assert!(sql.contains("GROUPING(\"seg\") AS __grp_1"), "{sql}");
}

#[test]
fn build_raw_records_selects_columns_no_group_by() {
    let cols = vec![col("id", "numeric", None), col("name", "string", None)];
    let spec = QuerySpec {
        metrics: vec![],
        dimensions: vec![],
        columns: vec!["id".into(), "name".into()],
        filters: vec![],
        orderby: vec![OrderBy { field: "id".into(), desc: true }],
        row_limit: 50,
        rollup: false,
    };
    let sql = build(Engine::Trino, &phys(), &cols, &[], &spec).unwrap();
    assert!(sql.contains("\"id\" AS \"id\""), "{sql}");
    assert!(!sql.contains("GROUP BY"), "{sql}");
    assert!(sql.contains("ORDER BY \"id\" DESC"), "{sql}");
}

#[test]
fn build_filters_use_where_and_escape() {
    let cols = vec![col("region", "string", None), col("qty", "numeric", None)];
    let metrics = vec![metric("revenue", "SUM(price)")];
    let spec = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["region".into()],
        columns: vec![],
        filters: vec![
            Filter { col: "region".into(), op: FilterOp::Eq, value: serde_json::json!("EU") },
            Filter { col: "qty".into(), op: FilterOp::Gt, value: serde_json::json!(10) },
            Filter { col: "region".into(), op: FilterOp::In, value: serde_json::json!(["A", "B"]) },
            Filter { col: "qty".into(), op: FilterOp::IsNotNull, value: serde_json::Value::Null },
        ],
        orderby: vec![],
        row_limit: 100,
        rollup: false,
    };
    let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
    assert!(sql.contains("\"region\" = 'EU'"), "{sql}");
    assert!(sql.contains("\"qty\" > 10"), "{sql}");
    assert!(sql.contains("\"region\" IN ('A', 'B')"), "{sql}");
    assert!(sql.contains("\"qty\" IS NOT NULL"), "{sql}");
}

#[test]
fn build_rejects_unknown_names() {
    let cols = vec![col("region", "string", None)];
    let spec = QuerySpec {
        metrics: vec!["nope".into()],
        dimensions: vec![],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 10,
        rollup: false,
    };
    assert!(build(Engine::Trino, &phys(), &cols, &[], &spec).is_err());
}
```

Add `uuid` to `[dev-dependencies]` of `crates/egret-bi/Cargo.toml` if not already present (it is used at runtime, so it is already a normal dependency — confirm with `grep uuid crates/egret-bi/Cargo.toml`; no change needed if present).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-bi chartquery::tests::build_`
Expected: FAIL to compile — `build`, `render_filter`, and the `use` lines don't exist yet.

- [ ] **Step 3: Implement `build()` + helpers**

At the top of `chartquery.rs`, ensure these imports are present:

```rust
use egret_database::model::Engine;
use serde::Deserialize;

use crate::dataset::{DatasetColumn, Metric, SourceSpec};
use crate::error::BiError;
use crate::expr::source_ref_for;
use crate::schema::quote_ident;
```

Add the implementation (below `sql_literal`):

```rust
/// Resolve a dimension/column name to its SQL expression: a calculated column uses its
/// stored expression; a physical column uses its quoted identifier.
fn column_sql(engine: Engine, c: &DatasetColumn) -> String {
    match &c.expression {
        Some(expr) => expr.clone(),
        None => quote_ident(engine, &c.column_name),
    }
}

fn find_column<'a>(columns: &'a [DatasetColumn], name: &str) -> Option<&'a DatasetColumn> {
    columns.iter().find(|c| c.column_name == name)
}
fn find_metric<'a>(metrics: &'a [Metric], name: &str) -> Option<&'a Metric> {
    metrics.iter().find(|m| m.metric_name == name)
}

/// Render one column filter into a WHERE predicate. Metric filters are rejected (→ HAVING,
/// deferred to 3c-3.2). The operator comes from an enum, so no operator injection is possible.
fn render_filter(
    engine: Engine,
    columns: &[DatasetColumn],
    f: &Filter,
) -> Result<String, BiError> {
    let c = find_column(columns, &f.col)
        .ok_or_else(|| BiError::BadRequest(format!("unknown filter column: {}", f.col)))?;
    let lhs = column_sql(engine, c);
    let tg = c.type_generic.as_str();
    let pred = match f.op {
        FilterOp::IsNull => format!("{lhs} IS NULL"),
        FilterOp::IsNotNull => format!("{lhs} IS NOT NULL"),
        FilterOp::In | FilterOp::NotIn => {
            let arr = f
                .value
                .as_array()
                .ok_or_else(|| BiError::BadRequest("IN filter needs an array value".into()))?;
            if arr.is_empty() {
                return Err(BiError::BadRequest("IN filter value is empty".into()));
            }
            let items = arr
                .iter()
                .map(|v| sql_literal(v, tg))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            let kw = if matches!(f.op, FilterOp::In) { "IN" } else { "NOT IN" };
            format!("{lhs} {kw} ({items})")
        }
        _ => {
            let lit = sql_literal(&f.value, tg)?;
            let op = match f.op {
                FilterOp::Eq => "=",
                FilterOp::Ne => "<>",
                FilterOp::Gt => ">",
                FilterOp::Ge => ">=",
                FilterOp::Lt => "<",
                FilterOp::Le => "<=",
                FilterOp::Like => "LIKE",
                _ => unreachable!("handled above"),
            };
            format!("{lhs} {op} {lit}")
        }
    };
    Ok(pred)
}

/// Resolve an ORDER BY field (a metric or a column name) to its SQL expression.
fn orderby_sql(
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    field: &str,
) -> Result<String, BiError> {
    if let Some(m) = find_metric(metrics, field) {
        return Ok(m.expression.clone());
    }
    if let Some(c) = find_column(columns, field) {
        return Ok(column_sql(engine, c));
    }
    Err(BiError::BadRequest(format!("unknown order-by field: {field}")))
}

/// Build the SQL for a chart query. Aggregate mode when `metrics` is non-empty; otherwise
/// raw-records mode over `columns`. GROUP BY / SELECT use resolved expressions, never aliases.
pub fn build(
    engine: Engine,
    source: &SourceSpec,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    spec: &QuerySpec,
) -> Result<String, BiError> {
    let from = source_ref_for(engine, source);
    let aggregate = !spec.metrics.is_empty();

    let mut select: Vec<String> = Vec::new();

    if aggregate {
        // Dimensions (resolved expressions, aliased to their name).
        let mut dim_exprs: Vec<String> = Vec::new();
        for name in &spec.dimensions {
            let c = find_column(columns, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown dimension: {name}")))?;
            let e = column_sql(engine, c);
            select.push(format!("{e} AS {}", quote_ident(engine, name)));
            dim_exprs.push(e);
        }
        // Metrics.
        for name in &spec.metrics {
            let m = find_metric(metrics, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown metric: {name}")))?;
            select.push(format!("{} AS {}", m.expression, quote_ident(engine, name)));
        }
        // ROLLUP grouping markers.
        if spec.rollup {
            for (i, e) in dim_exprs.iter().enumerate() {
                select.push(format!("GROUPING({e}) AS __grp_{i}"));
            }
        }
        let mut sql = format!("SELECT {}\nFROM {from}", select.join(", "));
        push_where(&mut sql, engine, columns, &spec.filters)?;
        if !dim_exprs.is_empty() {
            if spec.rollup {
                sql.push_str(&format!("\nGROUP BY ROLLUP({})", dim_exprs.join(", ")));
            } else {
                sql.push_str(&format!("\nGROUP BY {}", dim_exprs.join(", ")));
            }
        }
        push_orderby(&mut sql, engine, columns, metrics, &spec.orderby)?;
        push_limit(&mut sql, spec.row_limit);
        Ok(sql)
    } else {
        if spec.columns.is_empty() {
            return Err(BiError::BadRequest(
                "a table needs at least one metric or column".into(),
            ));
        }
        for name in &spec.columns {
            let c = find_column(columns, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown column: {name}")))?;
            select.push(format!("{} AS {}", column_sql(engine, c), quote_ident(engine, name)));
        }
        let mut sql = format!("SELECT {}\nFROM {from}", select.join(", "));
        push_where(&mut sql, engine, columns, &spec.filters)?;
        push_orderby(&mut sql, engine, columns, metrics, &spec.orderby)?;
        push_limit(&mut sql, spec.row_limit);
        Ok(sql)
    }
}

fn push_where(
    sql: &mut String,
    engine: Engine,
    columns: &[DatasetColumn],
    filters: &[Filter],
) -> Result<(), BiError> {
    if filters.is_empty() {
        return Ok(());
    }
    let preds = filters
        .iter()
        .map(|f| render_filter(engine, columns, f))
        .collect::<Result<Vec<_>, _>>()?;
    sql.push_str(&format!("\nWHERE {}", preds.join(" AND ")));
    Ok(())
}

fn push_orderby(
    sql: &mut String,
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    orderby: &[OrderBy],
) -> Result<(), BiError> {
    if orderby.is_empty() {
        return Ok(());
    }
    let parts = orderby
        .iter()
        .map(|o| {
            let e = orderby_sql(engine, columns, metrics, &o.field)?;
            Ok(format!("{e} {}", if o.desc { "DESC" } else { "ASC" }))
        })
        .collect::<Result<Vec<_>, BiError>>()?;
    sql.push_str(&format!("\nORDER BY {}", parts.join(", ")));
    Ok(())
}

fn push_limit(sql: &mut String, row_limit: i64) {
    let n = row_limit.clamp(1, MAX_CHART_ROWS);
    sql.push_str(&format!("\nLIMIT {n}"));
}
```

- [ ] **Step 4: Run the builder tests**

Run: `cargo test -p egret-bi chartquery::`
Expected: PASS (all `sql_literal_*` and `build_*` tests).

- [ ] **Step 5: Format + clippy**

Run: `cargo fmt -p egret-bi && cargo clippy -p egret-bi --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): chart query builder — aggregate/raw/rollup SQL + WHERE filters"
```

---

## Task 3: Migration + `ChartRepo`

**Files:**
- Create: `migrations/0014_chart.sql`
- Create: `crates/egret-bi/src/chart.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Test: `crates/egret-bi/tests/chart_repo.rs`

- [ ] **Step 1: Write the migration**

Create `migrations/0014_chart.sql`:

```sql
CREATE TABLE charts (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    dataset_id   uuid NOT NULL REFERENCES datasets(id)   ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    viz_type     text NOT NULL,
    params       jsonb NOT NULL,
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT chart_name_uniq UNIQUE (workspace_id, name)
);
CREATE INDEX charts_workspace_idx ON charts (workspace_id);
CREATE INDEX charts_dataset_idx   ON charts (dataset_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

- [ ] **Step 2: Write the failing `ChartRepo` test**

Create `crates/egret-bi/tests/chart_repo.rs`. Mirror the DB-gated setup used by `crates/egret-bi/tests/` (read an existing test in that dir first to copy the exact `Db` construction + `DATABASE_URL` skip + workspace/dataset seeding helper). The test body:

```rust
// DB-gated: self-skips when DATABASE_URL is unset (mirror sibling tests in this dir).
#[tokio::test]
async fn chart_repo_crud_and_unique_conflict() {
    let Some(db) = common::db().await else { return };
    let repo = egret_bi::ChartRepo::new(db.clone());
    // common::seed_workspace_with_dataset returns (workspace_id, dataset_id, user_id).
    let (ws, ds, user) = common::seed_workspace_with_dataset(&db).await;

    let params = serde_json::json!({ "metrics": ["revenue"], "row_limit": 100 });
    let c = repo
        .create(ws, ds, "Sales", "", "table", &params, Some(user))
        .await
        .unwrap();
    assert_eq!(c.name, "Sales");

    let got = repo.get(c.id).await.unwrap().unwrap();
    assert_eq!(got.viz_type, "table");

    let listed = repo.list_by_workspace(ws).await.unwrap();
    assert_eq!(listed.len(), 1);

    repo.update(c.id, "Sales v2", "desc", "pivot_table", &params)
        .await
        .unwrap();
    assert_eq!(repo.get(c.id).await.unwrap().unwrap().name, "Sales v2");
    assert_eq!(repo.chart_workspace(c.id).await.unwrap(), Some(ws));

    // duplicate name → Conflict
    let dup = repo.create(ws, ds, "Sales v2", "", "table", &params, Some(user)).await;
    assert!(matches!(dup, Err(egret_bi::BiError::Conflict(_))));

    repo.delete(c.id).await.unwrap();
    assert!(repo.get(c.id).await.unwrap().is_none());
}
```

If the sibling tests don't already expose a `common` module with `db()` + a dataset seeder, add the minimal helpers there following their existing pattern (do not invent a new harness). If seeding a full dataset is heavy, seed a `datasets` row directly with a raw `sqlx::query` insert in the helper.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p egret-bi --test chart_repo`
Expected: FAIL to compile — `ChartRepo` doesn't exist.

- [ ] **Step 4: Implement `ChartRepo`**

Create `crates/egret-bi/src/chart.rs`:

```rust
//! Chart repository: chart rows (viz_type + JSONB params) scoped to a workspace + dataset.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct ChartRepo {
    db: Db,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Chart {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub dataset_id: Uuid,
    pub name: String,
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str =
    "id, workspace_id, dataset_id, name, description, viz_type, params, created_by, updated_at";

/// Map a unique-violation on (workspace_id, name) to a Conflict; pass everything else through.
fn map_create_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a chart with that name already exists".into());
        }
    }
    BiError::Db(e)
}

impl ChartRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        workspace_id: Uuid,
        dataset_id: Uuid,
        name: &str,
        description: &str,
        viz_type: &str,
        params: &serde_json::Value,
        created_by: Option<Uuid>,
    ) -> Result<Chart, BiError> {
        let sql = format!(
            "INSERT INTO charts (workspace_id, dataset_id, name, description, viz_type, params, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING {COLS}"
        );
        sqlx::query_as::<_, Chart>(&sql)
            .bind(workspace_id)
            .bind(dataset_id)
            .bind(name)
            .bind(description)
            .bind(viz_type)
            .bind(params)
            .bind(created_by)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_create_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Chart>, BiError> {
        let sql = format!("SELECT {COLS} FROM charts WHERE id = $1");
        Ok(sqlx::query_as::<_, Chart>(&sql)
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    pub async fn list_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<Chart>, BiError> {
        let sql = format!("SELECT {COLS} FROM charts WHERE workspace_id = $1 ORDER BY updated_at DESC");
        Ok(sqlx::query_as::<_, Chart>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        viz_type: &str,
        params: &serde_json::Value,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE charts SET name=$2, description=$3, viz_type=$4, params=$5, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(viz_type)
        .bind(params)
        .execute(&self.db.pool)
        .await
        .map_err(map_create_err)?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM charts WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// The workspace a chart belongs to (for the nested-ownership guard). None if absent.
    pub async fn chart_workspace(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM charts WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    /// The owner (created_by) of a chart, for owner-or-admin checks.
    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar::<_, Option<Uuid>>("SELECT created_by FROM charts WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten(),
        )
    }
}
```

Add to `crates/egret-bi/src/lib.rs`:

```rust
mod chart;
```
and extend the re-export line (or add one):
```rust
pub use chart::{Chart, ChartRepo};
```

- [ ] **Step 5: Run the repo test**

Run (with a DB): `DATABASE_URL=postgres://... cargo test -p egret-bi --test chart_repo`
Expected: PASS. Without `DATABASE_URL`: the test self-skips (returns early).

Run the migration against your dev DB first if your harness doesn't auto-migrate:
`sqlx migrate run` (from the repo root, with `DATABASE_URL` set).

- [ ] **Step 6: Commit**

```bash
git add migrations/0014_chart.sql crates/egret-bi/src/chart.rs crates/egret-bi/src/lib.rs crates/egret-bi/tests/chart_repo.rs
git commit -m "feat(bi): charts table + ChartRepo CRUD"
```

---

## Task 4: DTOs, `chart_router`, `charts/data`, server wiring

**Files:**
- Modify: `crates/egret-bi/src/model.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Modify: `crates/egret-bin/src/main.rs`
- Modify: `crates/egret-server/src/lib.rs`
- Test: `crates/egret-bi/tests/http.rs`

- [ ] **Step 1: Add DTOs**

In `crates/egret-bi/src/model.rs`, add (match the serde style of the existing DTOs in that file):

```rust
use crate::chart::Chart;
use crate::chartquery::QuerySpec;

#[derive(serde::Serialize)]
pub struct ChartDto {
    pub id: String,
    pub dataset_id: String,
    pub name: String,
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub fn chart_dto(c: Chart) -> ChartDto {
    ChartDto {
        id: c.id.to_string(),
        dataset_id: c.dataset_id.to_string(),
        name: c.name,
        description: c.description,
        viz_type: c.viz_type,
        params: c.params,
        created_by: c.created_by.map(|u| u.to_string()),
        updated_at: c.updated_at,
    }
}

#[derive(serde::Deserialize)]
pub struct CreateChart {
    pub dataset_id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

#[derive(serde::Deserialize)]
pub struct UpdateChart {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

/// Build-only data request: produce SQL for a dataset + QuerySpec; the client submits it
/// to the existing /queries endpoint and polls.
#[derive(serde::Deserialize)]
pub struct ChartDataBody {
    pub dataset_id: uuid::Uuid,
    pub query_spec: QuerySpec,
}

#[derive(serde::Serialize)]
pub struct ChartDataResponse {
    pub sql: String,
    pub database_id: String,
}
```

- [ ] **Step 2: Add `charts` to `BiState` + the router + handlers**

In `crates/egret-bi/src/routes.rs`:

Add the permission consts near the existing ones:
```rust
const CH_READ: &str = "chart:read";
const CH_MANAGE: &str = "chart:manage";
```

Add the field to `BiState`:
```rust
    pub charts: crate::chart::ChartRepo,
```

Extend the imports from `crate::model` to include the new DTOs:
```rust
use crate::model::{
    /* existing... */ ChartDataBody, ChartDataResponse, CreateChart, UpdateChart,
};
use crate::chartquery::{self, QuerySpec};
```

Add the router + handlers (place after the dataset routes):

```rust
pub fn chart_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/charts", post(ch_create).get(ch_list))
        .route(
            "/ws/{workspace}/charts/{id}",
            get(ch_get).put(ch_update).delete(ch_remove),
        )
        .route("/ws/{workspace}/charts/data", post(ch_data))
}

async fn require_chart_in_ws(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    match st.charts.chart_workspace(id).await.map_err(BiApiError::from)? {
        Some(w) if w == ctx.workspace_id.0 => Ok(()),
        _ => Err(Error::NotFound("chart".into()).into()),
    }
}

async fn require_chart_owner(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let owner = st.charts.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) {
        Ok(())
    } else {
        Err(Error::Forbidden.into())
    }
}

async fn ch_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<crate::model::ChartDto>>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_READ).await.map_err(BiApiError::from)?;
    let rows = st.charts.list_by_workspace(ctx.workspace_id.0).await.map_err(BiApiError::from)?;
    Ok(Json(rows.into_iter().map(crate::model::chart_dto).collect()))
}

async fn ch_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateChart>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_MANAGE).await.map_err(BiApiError::from)?;
    // Ensure the dataset belongs to this workspace before binding a chart to it.
    let ds = st.datasets.get(b.dataset_id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("dataset".into()).into());
    }
    let c = st.charts.create(
        ctx.workspace_id.0, b.dataset_id, &b.name, &b.description, &b.viz_type, &b.params,
        Some(ctx.principal.user_id.0),
    ).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::chart_dto(c)))
}

async fn ch_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_READ).await.map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    let c = st.charts.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(c)))
}

async fn ch_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateChart>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_MANAGE).await.map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    require_chart_owner(&st, &ctx, id).await?;
    st.charts.update(id, &b.name, &b.description, &b.viz_type, &b.params)
        .await.map_err(BiApiError::from)?;
    let c = st.charts.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(c)))
}

async fn ch_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_MANAGE).await.map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    require_chart_owner(&st, &ctx, id).await?;
    st.charts.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ch_data(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<ChartDataBody>,
) -> Result<Json<ChartDataResponse>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_READ).await.map_err(BiApiError::from)?;
    let ds = st.datasets.get(b.dataset_id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("dataset".into()).into());
    }
    // We need the engine to know how to quote/qualify — derive it from the granted database.
    let database = st.databases.get(ds.database_id).await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    let cols = st.datasets.columns(b.dataset_id).await.map_err(BiApiError::from)?;
    let metrics = st.datasets.metrics(b.dataset_id).await.map_err(BiApiError::from)?;
    let source = source_of(&ds);
    let sql = chartquery::build(database.engine, &source, &cols, &metrics, &b.query_spec)
        .map_err(BiApiError::from)?;
    Ok(Json(ChartDataResponse { sql, database_id: ds.database_id.to_string() }))
}
```

Note `database.engine` — confirm `Database` exposes `engine: Engine` (it does; `ConnSpec::from_database` reads `db.engine`). `QuerySpec` is imported but used via `ChartDataBody`; the `use crate::chartquery::{self, QuerySpec};` may warn `QuerySpec` unused — drop `QuerySpec` from the import if so, keeping `self`.

Add the re-export in `crates/egret-bi/src/lib.rs`:
```rust
pub use routes::{chart_router, dataset_router, sqllab_router, BiState};
```

- [ ] **Step 3: Wire the router into the server + construct the repo**

In `crates/egret-server/src/lib.rs`, change the BI merge line:
```rust
        let bi_routes = egret_bi::sqllab_router()
            .merge(egret_bi::dataset_router())
            .merge(egret_bi::chart_router());
```

In `crates/egret-bin/src/main.rs`, add to the `BiState { ... }` literal:
```rust
        charts: egret_bi::ChartRepo::new(db.clone()),
```

- [ ] **Step 4: Add the failing HTTP test**

In `crates/egret-bi/tests/http.rs`: add `charts: egret_bi::ChartRepo::new(db.clone())` to the test `build_state` `BiState { ... }`. Then add a DB-gated test that creates a dataset (reuse the file's existing dataset-creation helper/flow), creates a chart, lists it, posts `charts/data`, and asserts the returned `sql` contains the expected fragments. Follow the request-helper pattern already in that file (tower `oneshot` or the harness it uses). Example assertions for the `charts/data` response:

```rust
// after creating a dataset `ds_id` with a numeric column "price" and a metric "revenue":
let body = serde_json::json!({
    "dataset_id": ds_id,
    "query_spec": { "metrics": ["revenue"], "dimensions": [], "row_limit": 10 }
});
// POST /api/ws/{ws}/charts/data → 200, body.sql contains "AS \"revenue\"" and "LIMIT 10"
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p egret-bi`
Expected: PASS (DB-gated tests skip without `DATABASE_URL`; the crate compiles and unit tests pass).
Run: `cargo build -p egret-bin -p egret-server`
Expected: PASS (wiring compiles).

- [ ] **Step 6: Format, clippy, commit**

```bash
cargo fmt --all && cargo clippy -p egret-bi -p egret-server -p egret-bin --all-targets -- -D warnings
git add crates/egret-bi crates/egret-server crates/egret-bin
git commit -m "feat(bi): chart CRUD routes + charts/data SQL builder endpoint + wiring"
```

---

## Task 5: Engine-gated Trino smoke (table + rollup)

**Files:**
- Create: `crates/egret-bi/tests/chart_engine.rs`

- [ ] **Step 1: Write the gated test**

Mirror the existing engine-gated dataset test in `crates/egret-bi/tests/` (read it first for the exact `EGRET_TEST_TRINO_URL` parsing + `ConnSpec`/connector setup). The test builds SQL with `egret_bi`'s builder against a known Trino table (e.g. `tpch.tiny.orders`) and executes it via a connector directly:

```rust
// Engine-gated: self-skips when EGRET_TEST_TRINO_URL is unset.
#[tokio::test]
async fn chart_rollup_runs_on_trino() {
    let Some(spec) = common_engine::trino_spec() else { return };
    use egret_bi::chartquery::{build, QuerySpec};
    use egret_bi::SourceSpec;

    let source = SourceSpec::Physical {
        catalog: Some("tpch".into()), schema: "tiny".into(), table: "orders".into(),
    };
    // columns/metrics as the dataset would store them:
    let cols = vec![/* orderpriority: string dimension */];
    let metrics = vec![/* revenue = SUM(totalprice) */];
    let qs = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["orderpriority".into()],
        columns: vec![], filters: vec![], orderby: vec![],
        row_limit: 100, rollup: true,
    };
    let sql = build(spec.engine, &source, &cols, &metrics, &qs).unwrap();
    let r = egret_engine::connector::build_connector(&spec)
        .execute(&sql, &egret_engine::model::Limits { max_rows: 100, timeout: std::time::Duration::from_secs(30) },
                 tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    // ROLLUP adds a grand-total row → more than the number of distinct priorities.
    assert!(!r.rows.is_empty());
    // __grp_0 = 1 marks the grand total; assert at least one such row exists.
}
```

Fill in `cols`/`metrics` using the `DatasetColumn`/`Metric` constructors (the `tests` helpers from Task 2 can be copied locally, or build the structs inline). Keep `chartquery` and `SourceSpec` public (Task 6 re-export step makes `chartquery` public — ensure `pub mod chartquery;` in `lib.rs`).

- [ ] **Step 2: Make `chartquery` public**

In `crates/egret-bi/src/lib.rs` change `mod chartquery;` to `pub mod chartquery;` and ensure `pub use dataset::SourceSpec;` already exists (it does).

- [ ] **Step 3: Run (gated)**

Run: `EGRET_TEST_TRINO_URL=... cargo test -p egret-bi --test chart_engine`
Expected: PASS against live Trino; without the env var, self-skips.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-bi/tests/chart_engine.rs crates/egret-bi/src/lib.rs
git commit -m "test(bi): engine-gated Trino smoke for chart table + rollup SQL"
```

---

## Task 6: Frontend API client (`charts.ts`)

**Files:**
- Modify: `frontend/package.json`
- Create: `frontend/src/api/charts.ts`
- Test: `frontend/src/api/__tests__/charts.test.ts`

- [ ] **Step 1: Add the table dependency**

Run: `cd frontend && npm install @tanstack/react-table`
Expected: `package.json` + lockfile updated; `@tanstack/react-table` added.

- [ ] **Step 2: Write `charts.ts`**

Create `frontend/src/api/charts.ts`:

```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type VizType = "table" | "pivot_table";
export type FilterOp =
  | "eq" | "ne" | "gt" | "ge" | "lt" | "le" | "like" | "in" | "not_in" | "is_null" | "is_not_null";

export interface ChartFilter { col: string; op: FilterOp; value?: unknown }
export interface ChartOrderBy { field: string; desc: boolean }

export interface QuerySpec {
  metrics: string[];
  dimensions: string[];
  columns: string[];
  filters: ChartFilter[];
  orderby: ChartOrderBy[];
  row_limit: number;
  rollup: boolean;
}

export interface Chart {
  id: string;
  dataset_id: string;
  name: string;
  description: string;
  viz_type: VizType;
  // params holds the QuerySpec plus a pivot layout: { spec: QuerySpec, rows: string[], cols: string[], metric: string }
  params: ChartParams;
  created_by: string | null;
  updated_at: string;
}

export interface ChartParams {
  spec: QuerySpec;
  // pivot-only layout (ignored for table viz)
  rows?: string[];
  cols?: string[];
  metric?: string;
}

export const chartKeys = {
  list: (ws: string) => ["charts", ws] as const,
  one: (ws: string, id: string) => ["charts", ws, id] as const,
};

export function useCharts(ws: string) {
  return useQuery<Chart[]>({
    queryKey: chartKeys.list(ws),
    queryFn: () => apiJson<Chart[]>(`/ws/${ws}/charts`),
  });
}

export function useChart(ws: string, id: string | null) {
  return useQuery<Chart>({
    queryKey: id ? chartKeys.one(ws, id) : ["charts", ws, "none"],
    queryFn: () => apiJson<Chart>(`/ws/${ws}/charts/${id}`),
    enabled: !!id,
  });
}

export function useChartMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: chartKeys.list(ws) });
  const create = useMutation({
    mutationFn: (b: { dataset_id: string; name: string; description?: string; viz_type: VizType; params: ChartParams }) =>
      apiJson<Chart>(`/ws/${ws}/charts`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: invalidate,
  });
  const update = useMutation({
    mutationFn: ({ id, ...b }: { id: string; name: string; description?: string; viz_type: VizType; params: ChartParams }) =>
      apiJson<Chart>(`/ws/${ws}/charts/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: invalidate,
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/charts/${id}`, { method: "DELETE" }),
    onSuccess: invalidate,
  });
  return { create, update, remove };
}

/// Build-only: returns the SQL + database_id; the caller submits via useSubmitQuery and polls.
export function useBuildChartData(ws: string) {
  return useMutation({
    mutationFn: (b: { dataset_id: string; query_spec: QuerySpec }) =>
      apiJson<{ sql: string; database_id: string }>(`/ws/${ws}/charts/data`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b),
      }),
  });
}
```

- [ ] **Step 3: Write a small test for the params round-trip shape**

Create `frontend/src/api/__tests__/charts.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import type { ChartParams } from "../charts";

describe("ChartParams", () => {
  it("carries spec + pivot layout", () => {
    const p: ChartParams = {
      spec: { metrics: ["revenue"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup: true },
      rows: ["region"], cols: ["segment"], metric: "revenue",
    };
    expect(p.spec.rollup).toBe(true);
    expect(p.rows).toEqual(["region"]);
  });
});
```

- [ ] **Step 4: Run**

Run: `cd frontend && npm test -- charts`
Expected: PASS. Run `npm run typecheck` → clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/package.json frontend/package-lock.json frontend/src/api/charts.ts frontend/src/api/__tests__/charts.test.ts
git commit -m "feat(web): charts API client + @tanstack/react-table dep"
```

---

## Task 7: Pivot transform + renderer

**Files:**
- Create: `frontend/src/components/chart/PivotTable.tsx`
- Test: `frontend/src/components/chart/__tests__/pivot.test.ts`

- [ ] **Step 1: Write the failing `buildPivot` test**

Create `frontend/src/components/chart/__tests__/pivot.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { buildPivot, type FlatRow } from "../PivotTable";

// columns returned by the engine for: rows=[region], cols=[segment], metric=revenue, rollup
const columns = ["region", "segment", "revenue", "__grp_0", "__grp_1"];
const rows: FlatRow[] = [
  ["EU", "SMB", 10, 0, 0],
  ["EU", "ENT", 20, 0, 0],
  ["EU", null, 30, 0, 1], // subtotal for EU
  ["US", "SMB", 5, 0, 0],
  ["US", null, 5, 0, 1],  // subtotal for US
  [null, null, 35, 1, 1], // grand total
];

describe("buildPivot", () => {
  const pivot = buildPivot(columns, rows, ["region"], ["segment"], "revenue");

  it("collects the distinct column keys", () => {
    expect(pivot.colKeys).toEqual([["SMB"], ["ENT"]]);
  });
  it("places leaf cells by (rowKey, colKey)", () => {
    const eu = pivot.rows.find((r) => r.key.join("|") === "EU" && !r.isSubtotal)!;
    expect(eu.cells["SMB"]).toBe(10);
    expect(eu.cells["ENT"]).toBe(20);
  });
  it("marks subtotal + grand-total rows from grouping markers", () => {
    expect(pivot.rows.some((r) => r.isSubtotal && r.key[0] === "EU")).toBe(true);
    expect(pivot.grandTotal).not.toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- pivot`
Expected: FAIL — `buildPivot` not defined.

- [ ] **Step 3: Implement `PivotTable.tsx`**

Create `frontend/src/components/chart/PivotTable.tsx`:

```tsx
export type FlatRow = unknown[];

export interface PivotRow {
  key: string[];           // row-dimension values (strings)
  cells: Record<string, number | string | null>; // colKey.join("|") → metric value
  isSubtotal: boolean;
}
export interface Pivot {
  colKeys: string[][];     // distinct column-dimension tuples (data only)
  rows: PivotRow[];        // leaf + subtotal rows, in source order
  grandTotal: Record<string, number | string | null> | null;
}

const key = (parts: (string | null)[]) => parts.map((p) => (p == null ? "∅" : String(p))).join("|");

/// Cross-tab flat ROLLUP rows. `rowDims`/`colDims` are column names; `metric` is the value column.
/// `__grp_<i>` markers (1 = rolled up) identify subtotal/grand-total rows.
export function buildPivot(
  columns: string[],
  rows: FlatRow[],
  rowDims: string[],
  colDims: string[],
  metric: string,
): Pivot {
  const idx = (name: string) => columns.indexOf(name);
  const rowDimIdx = rowDims.map(idx);
  const colDimIdx = colDims.map(idx);
  const metricIdx = idx(metric);
  const grpIdx = columns.map((c, i) => (c.startsWith("__grp_") ? i : -1)).filter((i) => i >= 0);
  // grouping markers align with the GROUP BY ROLLUP order = rowDims then colDims.
  const nRow = rowDims.length;

  const colKeySet = new Map<string, string[]>();
  const out: PivotRow[] = [];
  let grand: Record<string, number | string | null> | null = null;
  const byRowKey = new Map<string, PivotRow>();

  for (const r of rows) {
    const markers = grpIdx.map((i) => Number(r[i]) === 1);
    const rowRolled = markers.slice(0, nRow).some(Boolean);
    const colRolled = markers.slice(nRow).some(Boolean);
    const value = r[metricIdx] as number | string | null;

    if (rowRolled && colRolled) {
      grand = grand ?? {};
      grand["__total"] = value;
      continue;
    }
    const rk = rowDimIdx.map((i) => r[i] as string | null);
    const ck = colDimIdx.map((i) => r[i] as string | null);

    if (colRolled) {
      // subtotal row for this rowKey
      out.push({ key: rk.map((v) => String(v ?? "")), cells: { __total: value }, isSubtotal: true });
      continue;
    }
    // leaf cell
    const ckStr = key(ck);
    if (!colKeySet.has(ckStr)) colKeySet.set(ckStr, ck.map((v) => String(v ?? "")));
    const rkStr = key(rk);
    let pr = byRowKey.get(rkStr);
    if (!pr) {
      pr = { key: rk.map((v) => String(v ?? "")), cells: {}, isSubtotal: false };
      byRowKey.set(rkStr, pr);
      out.push(pr);
    }
    pr.cells[ckStr] = value;
  }

  return { colKeys: [...colKeySet.values()], rows: out, grandTotal: grand };
}

export function PivotTable(props: {
  columns: string[];
  rows: FlatRow[];
  rowDims: string[];
  colDims: string[];
  metric: string;
}) {
  const p = buildPivot(props.columns, props.rows, props.rowDims, props.colDims, props.metric);
  const colKeyStr = (k: string[]) => k.join("|");
  return (
    <table className="w-full border-collapse text-sm">
      <thead>
        <tr className="border-b text-left text-muted-foreground">
          {props.rowDims.map((d) => <th key={d} className="py-1 pr-3">{d}</th>)}
          {p.colKeys.map((k) => <th key={colKeyStr(k)} className="px-3 text-right">{k.join(" / ")}</th>)}
          <th className="px-3 text-right font-semibold">Total</th>
        </tr>
      </thead>
      <tbody>
        {p.rows.map((r, i) => (
          <tr key={i} className={r.isSubtotal ? "border-t bg-muted/40 font-medium" : "border-t"}>
            {r.key.map((v, j) => <td key={j} className="py-1 pr-3">{v}</td>)}
            {p.colKeys.map((k) => (
              <td key={colKeyStr(k)} className="px-3 text-right tabular-nums">
                {r.isSubtotal ? "" : fmt(r.cells[colKeyStr(k)])}
              </td>
            ))}
            <td className="px-3 text-right font-semibold tabular-nums">{fmt(r.cells["__total"])}</td>
          </tr>
        ))}
        {p.grandTotal && (
          <tr className="border-t-2 bg-muted font-semibold">
            <td className="py-1 pr-3" colSpan={props.rowDims.length}>Grand total</td>
            {p.colKeys.map((k) => <td key={colKeyStr(k)} />)}
            <td className="px-3 text-right tabular-nums">{fmt(p.grandTotal["__total"])}</td>
          </tr>
        )}
      </tbody>
    </table>
  );
}

function fmt(v: number | string | null | undefined): string {
  if (v == null) return "";
  return typeof v === "number" ? v.toLocaleString() : String(v);
}
```

- [ ] **Step 4: Run the test**

Run: `cd frontend && npm test -- pivot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/PivotTable.tsx frontend/src/components/chart/__tests__/pivot.test.ts
git commit -m "feat(web): pivot cross-tab transform + renderer (ROLLUP-aware)"
```

---

## Task 8: Table renderer (TanStack)

**Files:**
- Create: `frontend/src/components/chart/ChartTable.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartTable.test.tsx`

- [ ] **Step 1: Write the failing render test**

Create `frontend/src/components/chart/__tests__/ChartTable.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ChartTable } from "../ChartTable";

describe("ChartTable", () => {
  it("renders headers and formatted numeric cells", () => {
    render(
      <ChartTable
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1234.5], ["US", 9876]]}
      />,
    );
    expect(screen.getByText("region")).toBeInTheDocument();
    expect(screen.getByText("revenue")).toBeInTheDocument();
    // 1234.5 rendered with locale grouping
    expect(screen.getByText("1,234.5")).toBeInTheDocument();
  });
});
```

(If the repo's Vitest setup lacks `@testing-library/jest-dom`, drop `.toBeInTheDocument()` for `expect(screen.queryByText(...)).not.toBeNull()` to match the existing test style — check a sibling `*.test.tsx` first.)

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- ChartTable`
Expected: FAIL — `ChartTable` not defined.

- [ ] **Step 3: Implement `ChartTable.tsx`**

Create `frontend/src/components/chart/ChartTable.tsx`:

```tsx
import { useMemo, useState } from "react";
import {
  flexRender, getCoreRowModel, getSortedRowModel, useReactTable,
  type ColumnDef, type SortingState,
} from "@tanstack/react-table";

export interface ResultColumn { name: string; type: string }

const NUMERIC = /int|double|decimal|real|numeric|float|bigint/i;

function formatCell(value: unknown, type: string): string {
  if (value == null) return "";
  if (NUMERIC.test(type) && typeof value === "number") return value.toLocaleString();
  return String(value);
}

export function ChartTable({ columns, rows }: { columns: ResultColumn[]; rows: unknown[][] }) {
  const [sorting, setSorting] = useState<SortingState>([]);

  const cols = useMemo<ColumnDef<unknown[]>[]>(
    () =>
      columns.map((c, i) => ({
        id: c.name,
        accessorFn: (row) => row[i],
        header: c.name,
        cell: (ctx) => formatCell(ctx.getValue(), c.type),
        enableSorting: true,
      })),
    [columns],
  );

  const table = useReactTable({
    data: rows,
    columns: cols,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  return (
    <div className="overflow-auto">
      <table className="w-full border-collapse text-sm">
        <thead>
          {table.getHeaderGroups().map((hg) => (
            <tr key={hg.id} className="border-b text-left text-muted-foreground">
              {hg.headers.map((h) => {
                const isNum = NUMERIC.test(columns.find((c) => c.name === h.column.id)?.type ?? "");
                return (
                  <th
                    key={h.id}
                    onClick={h.column.getToggleSortingHandler()}
                    className={`cursor-pointer select-none py-1 px-3 ${isNum ? "text-right" : "text-left"}`}
                  >
                    {flexRender(h.column.columnDef.header, h.getContext())}
                    {{ asc: " ▲", desc: " ▼" }[h.column.getIsSorted() as string] ?? ""}
                  </th>
                );
              })}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((r) => (
            <tr key={r.id} className="border-t">
              {r.getVisibleCells().map((cell) => {
                const isNum = NUMERIC.test(columns.find((c) => c.name === cell.column.id)?.type ?? "");
                return (
                  <td key={cell.id} className={`py-1 px-3 ${isNum ? "text-right tabular-nums" : ""}`}>
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
```

- [ ] **Step 4: Run the test**

Run: `cd frontend && npm test -- ChartTable`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/ChartTable.tsx frontend/src/components/chart/__tests__/ChartTable.test.tsx
git commit -m "feat(web): TanStack-based chart table renderer (sort + number formatting)"
```

---

## Task 9: Explore page + Charts list + routing

**Files:**
- Create: `frontend/src/routes/explore/ChartExplore.tsx`
- Modify: `frontend/src/routes/explore/Charts.tsx`
- Modify: the explore route registration (find it: `grep -rn "DatasetDetail\|Datasets\b" frontend/src --include=*.tsx` → the router file that lists `datasets`/`datasets/:id`).

- [ ] **Step 1: Build the Explore page**

Create `frontend/src/routes/explore/ChartExplore.tsx`. It composes: a dataset picker, the dataset's metric/column lists (from `useDataset` in `frontend/src/api/datasets.ts` — confirm the hook name there), a Table/Pivot toggle, a controls panel that assembles a `QuerySpec`, a **Run** button that calls `useBuildChartData` then `useSubmitQuery` + polls with `useQueryStatus`/`useResults`, the preview (`ChartTable` or `PivotTable`), and a Save dialog (`useChartMutations().create`/`update`). Use this skeleton and fill the control inputs with the existing shadcn primitives (`Button`, `Input`, `Select`, `Dialog`) already used in `DatasetDetail.tsx`:

```tsx
import { useState } from "react";
import { useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDataset, useDatasets } from "@/api/datasets";
import {
  useBuildChartData, useChartMutations, type QuerySpec, type VizType, type ChartParams,
} from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/PivotTable";
import { Button } from "@/components/ui/button";

export default function ChartExplore() {
  const { ws = "" } = useParams();
  const datasets = useDatasets(ws);
  const [datasetId, setDatasetId] = useState<string | null>(null);
  const detail = useDataset(ws, datasetId); // { columns, metrics, ... }
  const [viz, setViz] = useState<VizType>("table");

  // controls → QuerySpec pieces
  const [metrics, setMetrics] = useState<string[]>([]);
  const [dimensions, setDimensions] = useState<string[]>([]);
  const [columns, setColumns] = useState<string[]>([]);
  const [rowLimit, setRowLimit] = useState(1000);
  // pivot layout
  const [rows, setRows] = useState<string[]>([]);
  const [cols, setCols] = useState<string[]>([]);
  const [pivotMetric, setPivotMetric] = useState<string>("");

  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  function specForRun(): QuerySpec {
    if (viz === "pivot_table") {
      return {
        metrics: pivotMetric ? [pivotMetric] : [],
        dimensions: [...rows, ...cols], // GROUP BY ROLLUP order = rows then cols
        columns: [], filters: [], orderby: [], row_limit: rowLimit, rollup: true,
      };
    }
    return {
      metrics, dimensions, columns: metrics.length ? [] : columns,
      filters: [], orderby: [], row_limit: rowLimit, rollup: false,
    };
  }

  async function run() {
    if (!datasetId) { toast.error("Pick a dataset"); return; }
    try {
      const { sql, database_id } = await build.mutateAsync({ dataset_id: datasetId, query_spec: specForRun() });
      const ack = await submit.mutateAsync({ database_id, sql, limit: rowLimit });
      setQueryId(ack.id);
    } catch (e) { toast.error(String(e)); }
  }

  const save = useChartMutations(ws);
  async function onSave(name: string) {
    if (!datasetId) return;
    const params: ChartParams =
      viz === "pivot_table"
        ? { spec: specForRun(), rows, cols, metric: pivotMetric }
        : { spec: specForRun() };
    await save.create.mutateAsync({ dataset_id: datasetId, name, viz_type: viz, params });
    toast.success("Saved");
  }

  return (
    <div className="flex h-full gap-4">
      {/* left rail: dataset picker + metric/column chips that push into the state arrays */}
      {/* center: viz toggle + controls + <Button onClick={run}>Run</Button> + save dialog */}
      {/* preview: */}
      <div className="flex-1 overflow-auto">
        {status.data?.status === "failed" && <p className="text-destructive">{status.data.error}</p>}
        {finished && results.data && viz === "table" && (
          <ChartTable columns={results.data.columns} rows={results.data.rows} />
        )}
        {finished && results.data && viz === "pivot_table" && (
          <PivotTable columns={results.data.columns.map((c) => c.name)} rows={results.data.rows}
            rowDims={rows} colDims={cols} metric={pivotMetric} />
        )}
      </div>
    </div>
  );
}
```

Fill the left rail + controls using the same primitives and styling as `DatasetDetail.tsx`/`Datasets.tsx`. Confirm the dataset hooks: open `frontend/src/api/datasets.ts` and use the actual exported hook names (e.g. `useDataset(ws, id)` returning `{ columns, metrics }`); adjust `detail` access accordingly. Metric/dimension/column pickers are multi-selects over `detail.data?.metrics` (names) and `detail.data?.columns` (names); a column with `is_calculated` is still a valid dimension.

- [ ] **Step 2: Replace the Charts placeholder with the list**

Overwrite `frontend/src/routes/explore/Charts.tsx`:

```tsx
import { Link, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useCharts, useChartMutations } from "@/api/charts";
import { Button } from "@/components/ui/button";

export default function Charts() {
  const { ws = "" } = useParams();
  const { data: charts, isLoading } = useCharts(ws);
  const { remove } = useChartMutations(ws);

  return (
    <div className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Charts</h1>
        <Button asChild size="sm"><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>
      </div>
      {isLoading ? (
        <p className="text-muted-foreground">Loading…</p>
      ) : !charts?.length ? (
        <p className="text-muted-foreground">No charts yet.</p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-muted-foreground">
            <tr><th className="py-1 pr-3">Name</th><th className="pr-3">Type</th><th className="pr-3">Updated</th><th /></tr>
          </thead>
          <tbody>
            {charts.map((c) => (
              <tr key={c.id} className="border-t">
                <td className="py-1 pr-3"><Link className="hover:underline" to={`/explore/${ws}/charts/${c.id}`}>{c.name}</Link></td>
                <td className="pr-3">{c.viz_type === "pivot_table" ? "Pivot" : "Table"}</td>
                <td className="pr-3 text-muted-foreground">{new Date(c.updated_at).toLocaleString()}</td>
                <td>
                  <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive"
                    onClick={async () => { try { await remove.mutateAsync(c.id); } catch (e) { toast.error(String(e)); } }}>
                    Delete
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
```

- [ ] **Step 3: Register the routes**

In the explore router file (the one that registers `datasets`/`datasets/:id`), add routes for the explore page. Match the existing route style (lazy or direct import). Example with the existing pattern:

```tsx
{ path: "charts", element: <Charts /> },
{ path: "charts/new", element: <ChartExplore /> },
{ path: "charts/:id", element: <ChartExplore /> },
```

If `ChartExplore` reads an `:id` it should load the saved chart via `useChart(ws, id)` and hydrate the controls from `params` on first load — add that effect (when `id` present: set `datasetId`, `viz`, and the control arrays from `chart.params`).

- [ ] **Step 4: Typecheck + lint + build**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build`
Expected: clean. Fix any unused-var / hook-dependency lint errors (CI runs `npm run lint`).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx frontend/src/routes/explore/Charts.tsx frontend/src/<explore-router-file>
git commit -m "feat(web): chart explore page + charts list + routes"
```

---

## Task 10: Manual smoke + full test sweep

**Files:** none (verification task).

- [ ] **Step 1: Backend test sweep**

Run: `cargo test -p egret-bi && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS / clean. (DB- and engine-gated tests self-skip without their env vars.)

- [ ] **Step 2: Frontend test sweep**

Run: `cd frontend && npm test && npm run typecheck && npm run lint && npm run build`
Expected: PASS / clean.

- [ ] **Step 3: Manual smoke on the live stack**

With the dev stack running (backend :8080, vite :5173, login `admin@egret.local` / `Admin12345!`, ws `eng`, dataset `orders` with metric `revenue` + a string dimension):
1. Explore → Charts → **+ Chart**.
2. Table: pick `orders`, add metric `revenue`, add dimension `orderpriority`, Run → rows render, numbers right-aligned, headers sort.
3. Pivot: toggle Pivot, rows=`orderpriority`, cols= a second dimension, metric=`revenue`, Run → cross-tab with subtotal rows + a grand-total row.
4. Save the chart; reload Charts list; open it → controls + preview hydrate.
5. Delete it.

Record results. If the ROLLUP path errors on the engine, capture the SQL (it is returned by `charts/data`) and verify `GROUPING(...)`/`ROLLUP(...)` against the engine's dialect.

- [ ] **Step 4: Run the manual Trino integration test (if changed engine contract)**

Per the project convention, run the manual real-Trino catalog+ranger+engine integration test before pushing engine-contract-sensitive changes. Extend it (or the `chart_engine` gated test) so it exercises a ROLLUP chart query end-to-end.

- [ ] **Step 5: Final commit (if any fixes)**

```bash
git add -A
git commit -m "test: phase 3c-3 chart full sweep + manual smoke fixes"
```

---

## Final Review

After all tasks: dispatch a final holistic code review (the IDOR-class issue in 3c-2.2 was caught only by the final cross-cutting review, not the per-task reviews — keep it). Focus areas:
- **`sql_literal` / `render_filter`**: can any filter value escape quoting? Are all ops enum-bound? Is the numeric/boolean validation tight?
- **Nested ownership**: `require_chart_in_ws` on every `/charts/{id}` handler and on `charts/data`'s dataset (a chart/dataset from another workspace must 404).
- **`row_limit`** clamped to `MAX_CHART_ROWS` on both the builder (`push_limit`) and the submit (`limit`).
- **GROUP BY uses expressions, not aliases**; calc-column dimensions group correctly.

Then use **superpowers:finishing-a-development-branch** to open the PR.
