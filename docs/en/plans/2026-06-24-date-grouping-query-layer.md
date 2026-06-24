# Date Grouping — Sub-project A (Query-layer Temporal Units) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a chart query group a temporal dimension by a temporal unit (truncation minute…year, or extraction hour-of-day/day-of-week/…), emitting correct, same-result SQL for both Trino and StarRocks.

**Architecture:** Add an optional `temporal_units` map (dimension name → unit token) to `QuerySpec`. A `TemporalUnit` enum + `temporal_expr(engine, inner, unit)` helper produce the wrapped SQL. A `dim_expr` helper applies the wrap when resolving a dimension, so SELECT, GROUP BY, GROUPING SETS, and ORDER BY all use the identical expression. Filters stay raw. All in one file.

**Tech Stack:** Rust (`egret-bi` crate), `cargo test`, Postgres-gated repo tests (not needed here — these are pure-SQL unit tests), real Trino + StarRocks for the manual integration check.

**Spec:** `docs/en/specs/2026-06-24-date-grouping-query-layer-design.md`

**Branch:** `feat/date-grouping-query-layer` (already checked out; spec committed).

**File touched (only one):** `crates/egret-bi/src/chartquery.rs`. Reference: `QuerySpec` at line 17, `column_sql` at 124, `find_column` at 131, `orderby_sql` at 188, `build()` dimension loop at 228–235, GROUP BY at 300–302, `push_orderby` at 346, `#[cfg(test)] mod tests` at ~372 with helpers `col(name, tg, expr)`, `metric`, `phys`.

**Conventions (every task):** `cargo fmt --all` before commit; backend sweep `cargo test -p egret-bi` must pass; stage explicit paths; commit footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. `Engine` has exactly two variants (`Trino`, `StarRocks`) so every `match engine` is exhaustive with those two arms — no wildcard arm.

---

### Task 1: Add `temporal_units` to `QuerySpec` (backward compatible)

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (`QuerySpec` struct at 17–31; 10 `QuerySpec { … }` test literals at lines 439, 460, 485, 507, 548, 568, 589, 612, 638, 654)
- Modify: `crates/egret-bi/tests/chart_engine.rs` (2 `QuerySpec { … }` literals at lines 83, 117)

- [ ] **Step 1: Add the field**

In `QuerySpec` (after `rollup_groups`):

```rust
    #[serde(default)]
    pub rollup_groups: Vec<Vec<String>>,
    /// dimension column name -> temporal unit token (snake_case, e.g. "month", "day_of_week").
    /// Empty (default) = no truncation = legacy behavior.
    #[serde(default)]
    pub temporal_units: std::collections::HashMap<String, String>,
}
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo build -p egret-bi --tests 2>&1 | grep "missing field" | head`
Expected: errors `missing field 'temporal_units' in initializer of QuerySpec` at each of the 12 literal sites.

- [ ] **Step 3: Add the field to all 12 literals**

In each `QuerySpec { … }` literal in `chartquery.rs` (10 sites) and `tests/chart_engine.rs` (2 sites), add as the last field:

```rust
        temporal_units: std::collections::HashMap::new(),
```

- [ ] **Step 4: Add a regression test (empty map ⇒ unchanged SQL)**

In `chartquery.rs` `mod tests`, add:

```rust
    #[test]
    fn empty_temporal_units_leaves_sql_unchanged() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("GROUP BY \"region\""), "{sql}");
        assert!(!sql.contains("date_trunc"), "{sql}");
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p egret-bi 2>&1 | tail -5`
Expected: PASS (all existing tests + the new one).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/egret-bi/src/chartquery.rs crates/egret-bi/tests/chart_engine.rs
git commit -m "feat(bi): add QuerySpec.temporal_units (no behavior change yet)"
```

---

### Task 2: `TemporalUnit` enum with `parse` + `is_extract`

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (add enum near `FilterOp`, ~line 48; add tests in `mod tests`)

- [ ] **Step 1: Write failing tests**

In `mod tests`:

```rust
    #[test]
    fn temporal_unit_parse_and_classify() {
        use TemporalUnit::*;
        assert_eq!(TemporalUnit::parse("month"), Some(Month));
        assert_eq!(TemporalUnit::parse("day_of_week"), Some(DayOfWeek));
        assert_eq!(TemporalUnit::parse("week_of_year"), Some(WeekOfYear));
        assert_eq!(TemporalUnit::parse("nonsense"), None);
        assert!(!Month.is_extract());
        assert!(Year.is_extract() == false);
        assert!(DayOfWeek.is_extract());
        assert!(HourOfDay.is_extract());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-bi temporal_unit_parse 2>&1 | tail -5`
Expected: FAIL — `cannot find type 'TemporalUnit'`.

- [ ] **Step 3: Implement the enum**

Add after the `FilterOp` enum block (after its `impl`/definition, ~line 66):

```rust
/// A Metabase-style temporal grouping unit. Truncation variants bucket to a date/timestamp;
/// extraction variants (`*Of*`) return an integer field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalUnit {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
    MinuteOfHour,
    HourOfDay,
    DayOfWeek,
    DayOfMonth,
    DayOfYear,
    WeekOfYear,
    MonthOfYear,
    QuarterOfYear,
}

impl TemporalUnit {
    pub fn parse(token: &str) -> Option<Self> {
        use TemporalUnit::*;
        Some(match token {
            "minute" => Minute,
            "hour" => Hour,
            "day" => Day,
            "week" => Week,
            "month" => Month,
            "quarter" => Quarter,
            "year" => Year,
            "minute_of_hour" => MinuteOfHour,
            "hour_of_day" => HourOfDay,
            "day_of_week" => DayOfWeek,
            "day_of_month" => DayOfMonth,
            "day_of_year" => DayOfYear,
            "week_of_year" => WeekOfYear,
            "month_of_year" => MonthOfYear,
            "quarter_of_year" => QuarterOfYear,
            _ => return None,
        })
    }

    pub fn is_extract(self) -> bool {
        use TemporalUnit::*;
        matches!(
            self,
            MinuteOfHour
                | HourOfDay
                | DayOfWeek
                | DayOfMonth
                | DayOfYear
                | WeekOfYear
                | MonthOfYear
                | QuarterOfYear
        )
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p egret-bi temporal_unit_parse 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): TemporalUnit enum with parse + is_extract"
```

---

### Task 3: `temporal_expr` — truncation units (both engines, same syntax)

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (add `temporal_expr` fn; tests)

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn temporal_expr_truncation_both_engines() {
        use TemporalUnit::*;
        assert_eq!(
            temporal_expr(Engine::Trino, "\"created_at\"", Month),
            "date_trunc('month', \"created_at\")"
        );
        assert_eq!(
            temporal_expr(Engine::StarRocks, "`created_at`", Quarter),
            "date_trunc('quarter', `created_at`)"
        );
        assert_eq!(
            temporal_expr(Engine::Trino, "x", Year),
            "date_trunc('year', x)"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-bi temporal_expr_truncation 2>&1 | tail -5`
Expected: FAIL — `cannot find function 'temporal_expr'`.

- [ ] **Step 3: Implement truncation (extraction added in Task 4)**

Add this function (the `_ =>` arm is a temporary `todo!`-free stub returning `inner` for the
not-yet-implemented extraction variants; Task 4 replaces it):

```rust
/// Wrap an already-resolved column expression in its temporal grouping. `inner` is trusted
/// (dataset-sourced); the unit string is a fixed literal, never user text.
fn temporal_expr(engine: Engine, inner: &str, unit: TemporalUnit) -> String {
    use TemporalUnit::*;
    let trunc = |u: &str| format!("date_trunc('{u}', {inner})");
    match unit {
        Minute => trunc("minute"),
        Hour => trunc("hour"),
        Day => trunc("day"),
        Week => trunc("week"),
        Month => trunc("month"),
        Quarter => trunc("quarter"),
        Year => trunc("year"),
        // extraction variants — implemented in Task 4
        _ => extract_expr(engine, inner, unit),
    }
}
```

And a placeholder `extract_expr` so it compiles (Task 4 fills the real bodies):

```rust
fn extract_expr(_engine: Engine, inner: &str, _unit: TemporalUnit) -> String {
    inner.to_string()
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p egret-bi temporal_expr_truncation 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): temporal_expr truncation (date_trunc) for both engines"
```

---

### Task 4: `extract_expr` — extraction units (per-engine, normalized)

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (replace `extract_expr` body; tests)

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn temporal_expr_extraction_normalized() {
        use TemporalUnit::*;
        // identical function on both engines
        assert_eq!(temporal_expr(Engine::Trino, "x", MonthOfYear), "month(x)");
        assert_eq!(temporal_expr(Engine::StarRocks, "x", MonthOfYear), "month(x)");
        assert_eq!(temporal_expr(Engine::Trino, "x", HourOfDay), "hour(x)");
        // day_of_week: Trino native vs StarRocks normalized to Mon=1
        assert_eq!(temporal_expr(Engine::Trino, "x", DayOfWeek), "day_of_week(x)");
        assert_eq!(temporal_expr(Engine::StarRocks, "x", DayOfWeek), "(weekday(x) + 1)");
        // week_of_year: Trino native vs StarRocks ISO mode 3
        assert_eq!(temporal_expr(Engine::Trino, "x", WeekOfYear), "week_of_year(x)");
        assert_eq!(temporal_expr(Engine::StarRocks, "x", WeekOfYear), "week(x, 3)");
        // day_of_month / day_of_year naming differs
        assert_eq!(temporal_expr(Engine::Trino, "x", DayOfMonth), "day_of_month(x)");
        assert_eq!(temporal_expr(Engine::StarRocks, "x", DayOfMonth), "dayofmonth(x)");
        assert_eq!(temporal_expr(Engine::Trino, "x", DayOfYear), "day_of_year(x)");
        assert_eq!(temporal_expr(Engine::StarRocks, "x", DayOfYear), "dayofyear(x)");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-bi temporal_expr_extraction 2>&1 | tail -8`
Expected: FAIL — assertions mismatch (placeholder returns `inner`).

- [ ] **Step 3: Implement `extract_expr`**

Replace the placeholder with:

```rust
/// Extraction units differ by engine; results are normalized so both engines return the
/// same integers (ISO week, Monday=1 weekday), matching Trino and Metabase.
fn extract_expr(engine: Engine, inner: &str, unit: TemporalUnit) -> String {
    use Engine::*;
    use TemporalUnit::*;
    match (engine, unit) {
        (_, MinuteOfHour) => format!("minute({inner})"),
        (_, HourOfDay) => format!("hour({inner})"),
        (_, MonthOfYear) => format!("month({inner})"),
        (_, QuarterOfYear) => format!("quarter({inner})"),

        (Trino, DayOfWeek) => format!("day_of_week({inner})"),
        (StarRocks, DayOfWeek) => format!("(weekday({inner}) + 1)"),

        (Trino, DayOfMonth) => format!("day_of_month({inner})"),
        (StarRocks, DayOfMonth) => format!("dayofmonth({inner})"),

        (Trino, DayOfYear) => format!("day_of_year({inner})"),
        (StarRocks, DayOfYear) => format!("dayofyear({inner})"),

        (Trino, WeekOfYear) => format!("week_of_year({inner})"),
        (StarRocks, WeekOfYear) => format!("week({inner}, 3)"),

        // truncation variants never reach here (handled by temporal_expr); identity is safe.
        (_, Minute | Hour | Day | Week | Month | Quarter | Year) => inner.to_string(),
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p egret-bi temporal_expr 2>&1 | tail -6`
Expected: PASS (both truncation and extraction tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): extract_expr for extraction units, normalized across engines"
```

---

### Task 5: `dim_expr` + wire the dimension loop (SELECT + GROUP BY)

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (add `dim_expr`; edit `build()` dimension loop at 228–235; tests)

- [ ] **Step 1: Write failing tests**

```rust
    fn tcol(name: &str) -> DatasetColumn {
        DatasetColumn {
            column_name: name.into(),
            data_type: "timestamp".into(),
            type_generic: "temporal".into(),
            is_temporal: true,
            ordinal: 0,
            expression: None,
            verbose_name: String::new(),
            description: String::new(),
        }
    }

    fn spec_with_unit(dim: &str, unit: &str) -> QuerySpec {
        let mut tu = std::collections::HashMap::new();
        tu.insert(dim.to_string(), unit.to_string());
        QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec![dim.into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: tu,
        }
    }

    #[test]
    fn dimension_truncation_select_and_group_match() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = spec_with_unit("created_at", "month");
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(
            sql.contains("date_trunc('month', \"created_at\") AS \"created_at\""),
            "{sql}"
        );
        assert!(sql.contains("GROUP BY date_trunc('month', \"created_at\")"), "{sql}");
    }

    #[test]
    fn dimension_unit_on_non_temporal_is_rejected() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = spec_with_unit("region", "month");
        let err = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap_err();
        assert!(matches!(err, BiError::BadRequest(_)));
    }

    #[test]
    fn dimension_unknown_unit_is_rejected() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = spec_with_unit("created_at", "fortnight");
        let err = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap_err();
        assert!(matches!(err, BiError::BadRequest(_)));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-bi dimension_ 2>&1 | tail -8`
Expected: FAIL — `cannot find function 'dim_expr'` is not yet referenced, so these fail on the truncation assertion (SELECT still shows raw `"created_at"`), and the rejection tests fail (build succeeds today).

- [ ] **Step 3: Add `dim_expr` helper**

Add near `column_sql` (after line 129):

```rust
/// Resolve a dimension name to SQL, applying its temporal unit (from `spec.temporal_units`) if set.
/// A unit on a non-temporal column, or an unknown unit token, is a BadRequest.
fn dim_expr(
    engine: Engine,
    columns: &[DatasetColumn],
    spec: &QuerySpec,
    name: &str,
) -> Result<String, BiError> {
    let c = find_column(columns, name)
        .ok_or_else(|| BiError::BadRequest(format!("unknown dimension: {name}")))?;
    let base = column_sql(engine, c);
    match spec.temporal_units.get(name) {
        None => Ok(base),
        Some(tok) => {
            let unit = TemporalUnit::parse(tok)
                .ok_or_else(|| BiError::BadRequest(format!("unknown temporal unit: {tok}")))?;
            if !c.is_temporal {
                return Err(BiError::BadRequest(format!(
                    "temporal unit on non-temporal column: {name}"
                )));
            }
            Ok(temporal_expr(engine, &base, unit))
        }
    }
}
```

- [ ] **Step 4: Wire it into the dimension loop**

In `build()`, replace the body of the dimension loop (lines 228–235):

```rust
        for (i, name) in spec.dimensions.iter().enumerate() {
            let e = dim_expr(engine, columns, spec, name)?;
            select.push(format!("{e} AS {}", quote_ident(engine, name)));
            dim_index.insert(name.as_str(), i);
            dim_exprs.push(e);
        }
```

(`find_column` is now called inside `dim_expr`; the loop no longer calls it directly. `dim_exprs`
feeds GROUP BY / GROUPING SETS unchanged, so those become correct automatically.)

- [ ] **Step 5: Run tests**

Run: `cargo test -p egret-bi 2>&1 | tail -6`
Expected: PASS (new dimension tests + all existing tests, since empty `temporal_units` keeps `dim_expr` returning `base`).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): apply temporal units to dimensions (SELECT + GROUP BY)"
```

---

### Task 6: ORDER BY uses the same wrapped expression

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (`push_orderby` 346–364, `orderby_sql` 188–203, call site at 303 & 323; tests)

- [ ] **Step 1: Write failing test**

```rust
    #[test]
    fn orderby_on_grouped_temporal_dim_wraps_identically() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let mut spec = spec_with_unit("created_at", "month");
        spec.orderby = vec![OrderBy { field: "created_at".into(), desc: false }];
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("GROUP BY date_trunc('month', \"created_at\")"), "{sql}");
        assert!(
            sql.contains("ORDER BY date_trunc('month', \"created_at\") ASC"),
            "{sql}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-bi orderby_on_grouped 2>&1 | tail -6`
Expected: FAIL — ORDER BY shows raw `"created_at"`, not the `date_trunc` wrap.

- [ ] **Step 3: Thread `spec` into order-by resolution**

Change `push_orderby` to take the full columns + spec and resolve via `dim_expr` when the field is a
dimension. Replace `orderby_sql` (188–203) and `push_orderby` (346–364) with:

```rust
/// Resolve an ORDER BY field to SQL: a metric expression, a grouped dimension (with its temporal
/// unit), or a bare column. Order matters — a dimension with a unit must match its GROUP BY expr.
fn orderby_sql(
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    spec: &QuerySpec,
    field: &str,
) -> Result<String, BiError> {
    if let Some(m) = find_metric(metrics, field) {
        return Ok(m.expression.clone());
    }
    if spec.dimensions.iter().any(|d| d == field) {
        return dim_expr(engine, columns, spec, field);
    }
    if let Some(c) = find_column(columns, field) {
        return Ok(column_sql(engine, c));
    }
    Err(BiError::BadRequest(format!("unknown order-by field: {field}")))
}
```

```rust
fn push_orderby(
    sql: &mut String,
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    spec: &QuerySpec,
) -> Result<(), BiError> {
    if spec.orderby.is_empty() {
        return Ok(());
    }
    let parts = spec
        .orderby
        .iter()
        .map(|o| {
            let e = orderby_sql(engine, columns, metrics, spec, &o.field)?;
            Ok(format!("{e} {}", if o.desc { "DESC" } else { "ASC" }))
        })
        .collect::<Result<Vec<_>, BiError>>()?;
    sql.push_str(&format!("\nORDER BY {}", parts.join(", ")));
    Ok(())
}
```

- [ ] **Step 4: Update the two call sites**

In `build()` at line 303 (aggregate) and 323 (raw-records), change:

```rust
        push_orderby(&mut sql, engine, columns, metrics, &spec.orderby)?;
```

to:

```rust
        push_orderby(&mut sql, engine, columns, metrics, spec)?;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p egret-bi 2>&1 | tail -6`
Expected: PASS (new order-by test + all existing order-by tests still green — a bare column field still resolves via the `find_column` branch).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): ORDER BY reuses a dimension's temporal-unit expression"
```

---

### Task 7: Full sweep + manual Trino/StarRocks verification

**Files:** none (verification + final checks)

- [ ] **Step 1: Format check**

Run: `cargo fmt --all --check`
Expected: clean (no diff).

- [ ] **Step 2: Full crate test**

Run: `cargo test -p egret-bi 2>&1 | tail -8`
Expected: all PASS.

- [ ] **Step 3: Manual integration check on both engines**

Using the running dev stack (Trino + StarRocks), build a `QuerySpec` over a seeded temporal column
with `temporal_units {"<ts_col>":"month"}` and separately `{"<ts_col>":"day_of_week"}`, run the
generated SQL on both engines, and confirm:
- `month` returns one row per month with a date/timestamp bucket label, identical bucket set on both engines.
- `day_of_week` returns integer 1–7 with Monday=1 on both engines (Trino `day_of_week`, StarRocks `(weekday()+1)` agree).

Record the two generated SQL strings and the row counts in the PR description. (This is the
`egret-manual-trino-integration-test` step — not run in CI.)

- [ ] **Step 4: Confirm StarRocks result coercion returns integers for extraction**

In the same manual run, verify the `day_of_week` column comes back as JSON numbers, not the original
timestamp type. If it returns the wrong type, the result coercion keys off the dataset column type
instead of the result-set type — fix that (coerce by result-set type) and note it in the PR. Expected:
already correct (StarRocks reports `MYSQL_TYPE_LONG` for the extracted column).

- [ ] **Step 5: Open the PR**

```bash
git push -u origin feat/date-grouping-query-layer
gh pr create --base main --title "feat(bi): query-layer temporal units (Date Grouping A)" --body "<summary + the two manual SQL strings + parity result>"
```

---

## Self-Review

**Spec coverage:** QuerySpec field (Task 1) ✓; TemporalUnit full set + parse + is_extract (Task 2) ✓; truncation both engines (Task 3) ✓; extraction normalized incl. day_of_week/week_of_year (Task 4) ✓; dim_expr + SELECT/GROUP BY/GROUPING SETS + non-temporal & unknown-unit errors (Task 5) ✓; ORDER BY parity (Task 6) ✓; filters untouched (no task changes `render_filter`/`push_where` — correct by omission) ✓; manual two-engine + coercion verification (Task 7) ✓.

**Placeholder scan:** none — every code step shows full code; the Task 3 `extract_expr` stub is an explicit, compiling identity function replaced in Task 4 (stated).

**Type consistency:** `temporal_expr(engine, inner: &str, unit) -> String`, `extract_expr(engine, inner, unit) -> String`, `dim_expr(engine, columns, spec, name) -> Result<String, BiError>`, `orderby_sql(engine, columns, metrics, spec, field)`, `push_orderby(sql, engine, columns, metrics, spec)` — names and signatures match across tasks. `TemporalUnit` variants identical in Tasks 2/3/4. `tcol`/`spec_with_unit` helpers defined in Task 5 and reused in Task 6 (Task 6 follows Task 5).
