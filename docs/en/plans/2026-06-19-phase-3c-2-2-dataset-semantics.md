# Phase 3c-2.2 Dataset Semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add metrics, calculated columns, and editable column metadata to datasets, and rewrite refresh to reconcile (upsert physical, preserve calculated columns + user metadata) instead of wiping the column snapshot.

**Architecture:** Extends `egret-bi` (no new crate). New `expr.rs` validates expressions by probing the engine via the existing `Connector::describe`. Refresh switches from `replace_columns` (DELETE-all) to `reconcile_physical_columns` (diff/upsert by name), mirroring Superset's `fetch_metadata`. The dataset detail page becomes tabbed (Columns / Metrics).

**Tech Stack:** Rust (axum 0.8, sqlx 0.8 runtime queries, async-trait), Postgres, React 18 + TS + Vite + react-query 5 + Vitest.

**Spec:** `docs/en/specs/2026-06-19-phase-3c-2-2-dataset-semantics-design.md` (vi mirror alongside).

**Conventions (must follow):**
- Plain `git commit` with the configured identity — never `--author`.
- Runtime sqlx only (`sqlx::query`, `query_as`, `query_scalar`) — never the `query!` macros.
- DB-gated tests self-skip when `DATABASE_URL` unset (mirror `crates/egret-bi/tests/dataset.rs`); engine-gated tests self-skip when Trino unreachable (mirror `crates/egret-engine/tests/describe.rs`, env `EGRET_TEST_TRINO_URL`).
- DB secrets never returned or logged.
- Branch `feat/phase-3c-2-2-dataset-semantics` (already created; spec is its first commit).
- Before each `cargo` verify, run `cargo fmt --all` (CI gates on `cargo fmt --check`); for frontend run `npm run lint` too (CI runs eslint — it WILL fail the build on an error).

---

## File Structure

| File | Responsibility |
|---|---|
| `migrations/0013_dataset_semantics.sql` (new) | `ALTER dataset_columns` (+expression/verbose_name/description) + `dataset_metrics` table |
| `crates/egret-bi/src/error.rs` (modify) | Add `BiError::Conflict(String)` → 409 |
| `crates/egret-bi/src/dataset.rs` (modify) | Extend `DatasetColumn`; `Metric`/`NewMetric`; `reconcile_physical_columns` (replaces `replace_columns`); metric + calc-column + metadata CRUD; map unique-violation → Conflict |
| `crates/egret-bi/src/schema.rs` (modify) | Make `quote_trino`/`quote_mysql` `pub(crate)` for reuse |
| `crates/egret-bi/src/expr.rs` (new) | `validate_expression` (single-statement guard + describe-probe) + per-engine source-ref builder |
| `crates/egret-bi/src/model.rs` (modify) | `MetricDto`; extend `DatasetColumnDto`/`DatasetDetailDto`; metric/column request bodies; `detail_dto` takes metrics |
| `crates/egret-bi/src/routes.rs` (modify) | metric + calc-column + metadata handlers; `load_detail` helper; route registration |
| `crates/egret-bi/src/lib.rs` (modify) | Export new repo types; declare `mod expr;` |
| `crates/egret-bi/tests/dataset.rs` (modify) | reconcile-preserves-calc test; metric + calc CRUD |
| `crates/egret-bi/tests/expr.rs` (new) | expression-validation unit + engine-gated |
| `crates/egret-bi/tests/http.rs` (modify) | dataset metric/column auth smoke (optional, mirror existing) |
| `frontend/src/api/datasets.ts` (modify) | metric/column types + hooks |
| `frontend/src/routes/explore/DatasetDetail.tsx` (modify) | Columns/Metrics tabs |
| `frontend/src/components/dataset/ColumnsTab.tsx` (new) | columns table + metadata edit + add calculated |
| `frontend/src/components/dataset/MetricsTab.tsx` (new) | metrics table + add/edit/delete |
| `frontend/src/components/dataset/__tests__/DatasetTabs.test.tsx` (new) | tabs render + add forms |

---

## Task 1: Migration `0013_dataset_semantics.sql`

**Files:** Create `migrations/0013_dataset_semantics.sql`

- [ ] **Step 1: Write the migration**

```sql
ALTER TABLE dataset_columns
  ADD COLUMN expression   text,
  ADD COLUMN verbose_name text NOT NULL DEFAULT '',
  ADD COLUMN description  text NOT NULL DEFAULT '';

CREATE TABLE dataset_metrics (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    metric_name  text NOT NULL,
    expression   text NOT NULL,
    data_type    text NOT NULL,
    type_generic text NOT NULL,
    verbose_name text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    ordinal      int  NOT NULL,
    UNIQUE (dataset_id, metric_name)
);
CREATE INDEX dataset_metrics_dataset_idx ON dataset_metrics (dataset_id);
```

- [ ] **Step 2: Verify it embeds/compiles**

Run: `cargo build -p egret-bin 2>&1 | tail -3`
Expected: builds clean (sqlx `migrate!` embeds the file; a syntax error would fail the build). If `DATABASE_URL` is set, `cargo test -p egret-bi --test http 2>&1 | tail` applies it via `db.migrate()`.

- [ ] **Step 3: Commit**

```bash
git add migrations/0013_dataset_semantics.sql
git commit -m "feat(dataset): migration for metrics + column expression/metadata"
```

---

## Task 2: `BiError::Conflict` → 409

**Files:** Modify `crates/egret-bi/src/error.rs`

The current `BiError` has `NotFound/Forbidden/BadRequest/Engine/Db`. Add `Conflict` so duplicate metric/column/dataset names return 409 with a clear message (also closes the 3c-2 gap where a duplicate dataset name returned a bare 500).

- [ ] **Step 1: Add the variant**

In the `pub enum BiError` block, add after `BadRequest`:

```rust
    #[error("conflict: {0}")]
    Conflict(String),
```

- [ ] **Step 2: Map it to 409 in `IntoResponse`**

In `impl IntoResponse for BiApiError`, the `BiApiError::Domain(e)` match has arms per variant. Add:

```rust
                BiError::Conflict(_) => (StatusCode::CONFLICT, e.to_string()),
```

(Place it beside the `BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string())` arm. The match must stay exhaustive.)

- [ ] **Step 3: Verify**

Run: `cargo build -p egret-bi 2>&1 | tail -3 && cargo clippy -p egret-bi --all-targets -- -D warnings 2>&1 | tail -3`
Expected: clean (the non-exhaustive-match error if you missed the arm will show here).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-bi/src/error.rs
git commit -m "feat(bi): BiError::Conflict mapped to HTTP 409"
```

---

## Task 3: Extend column model + reconcile (replace `replace_columns`)

**Files:**
- Modify: `crates/egret-bi/src/dataset.rs`
- Modify: `crates/egret-bi/src/routes.rs` (swap `replace_columns` → `reconcile_physical_columns` in `ds_refresh`)
- Test: `crates/egret-bi/tests/dataset.rs`

### Step 1: Extend `DatasetColumn` + `columns()` query

In `dataset.rs`, change the `DatasetColumn` struct to:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DatasetColumn {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
    pub expression: Option<String>,
    pub verbose_name: String,
    pub description: String,
}
```

And update `columns()` to select the new fields:

```rust
    pub async fn columns(&self, dataset_id: Uuid) -> Result<Vec<DatasetColumn>, BiError> {
        Ok(sqlx::query_as::<_, DatasetColumn>(
            "SELECT column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description \
             FROM dataset_columns WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }
```

`insert_columns` (used by `create`) inserts physical columns and now must set the new columns to defaults — update its INSERT to include `expression, verbose_name, description` as `NULL, '', ''`:

```rust
async fn insert_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dataset_id: Uuid,
    columns: &[NewColumn],
) -> Result<(), BiError> {
    for (i, c) in columns.iter().enumerate() {
        sqlx::query(
            "INSERT INTO dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
             VALUES ($1,$2,$3,$4,$5,$6, NULL, '', '')",
        )
        .bind(dataset_id)
        .bind(&c.column_name)
        .bind(&c.data_type)
        .bind(&c.type_generic)
        .bind(c.is_temporal)
        .bind(i as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
```

### Step 2: Replace `replace_columns` with `reconcile_physical_columns`

Delete the `replace_columns` method and add:

```rust
    /// Reconcile the *physical* column snapshot against a fresh introspection, mirroring
    /// Superset's fetch_metadata: upsert physical columns by name (preserving user-edited
    /// verbose_name/description), delete physical columns gone from the source, and NEVER
    /// touch calculated columns (expression IS NOT NULL) or metrics. Runs in one tx.
    pub async fn reconcile_physical_columns(
        &self,
        id: Uuid,
        fresh: &[NewColumn],
    ) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;

        // Upsert each fresh physical column. The DO UPDATE is guarded to physical rows so a
        // name collision with a calculated column leaves the calculated column untouched.
        for (i, c) in fresh.iter().enumerate() {
            sqlx::query(
                "INSERT INTO dataset_columns \
                   (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
                 VALUES ($1,$2,$3,$4,$5,$6, NULL, '', '') \
                 ON CONFLICT (dataset_id, column_name) DO UPDATE SET \
                   data_type = EXCLUDED.data_type, \
                   type_generic = EXCLUDED.type_generic, \
                   is_temporal = EXCLUDED.is_temporal, \
                   ordinal = EXCLUDED.ordinal \
                 WHERE dataset_columns.expression IS NULL",
            )
            .bind(id)
            .bind(&c.column_name)
            .bind(&c.data_type)
            .bind(&c.type_generic)
            .bind(c.is_temporal)
            .bind(i as i32)
            .execute(&mut *tx)
            .await?;
        }

        // Delete physical columns no longer present in the source.
        let fresh_names: Vec<String> = fresh.iter().map(|c| c.column_name.clone()).collect();
        sqlx::query(
            "DELETE FROM dataset_columns \
             WHERE dataset_id = $1 AND expression IS NULL AND column_name <> ALL($2)",
        )
        .bind(id)
        .bind(&fresh_names)
        .execute(&mut *tx)
        .await?;

        // Re-ordinal calculated columns to follow the physical columns.
        sqlx::query(
            "UPDATE dataset_columns c SET ordinal = sub.rn + phys.n \
             FROM (SELECT id, (row_number() OVER (ORDER BY ordinal)) - 1 AS rn \
                   FROM dataset_columns WHERE dataset_id = $1 AND expression IS NOT NULL) sub, \
                  (SELECT count(*) AS n FROM dataset_columns WHERE dataset_id = $1 AND expression IS NULL) phys \
             WHERE c.id = sub.id",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE datasets SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
```

### Step 3: Swap the call in `ds_refresh`

In `routes.rs::ds_refresh`, change:
```rust
    st.datasets
        .replace_columns(id, &new_columns)
```
to:
```rust
    st.datasets
        .reconcile_physical_columns(id, &new_columns)
```

### Step 4: Write the reconcile regression test

In `crates/egret-bi/tests/dataset.rs`, add a test (reuse the existing `setup()`/seed harness in that file). It must prove the 3c-2 clobber is fixed:

```rust
#[tokio::test]
async fn reconcile_preserves_calculated_columns_and_metadata() {
    let Some(ctx) = setup().await else { eprintln!("skip"); return };
    let repo = egret_bi::DatasetRepo::new(ctx.db.clone());

    // physical dataset with 2 columns
    let phys = vec![
        egret_bi::NewColumn { column_name: "id".into(), data_type: "bigint".into(), type_generic: "numeric".into(), is_temporal: false },
        egret_bi::NewColumn { column_name: "ts".into(), data_type: "timestamp".into(), type_generic: "temporal".into(), is_temporal: true },
    ];
    let ds = repo.create(ctx.ws, ctx.db_id, "sales", "", &egret_bi::SourceSpec::Physical {
        catalog: Some("tpch".into()), schema: "tiny".into(), table: "orders".into() }, &phys, Some(ctx.user)).await.unwrap();

    // user edits metadata on a physical column + adds a calculated column
    repo.update_column(ds.id, "id", Some("Order ID"), Some("the id"), None).await.unwrap(); // verbose_name/description
    repo.add_calc_column(ds.id, "id_x2", "id * 2", "bigint", "numeric", false, "", "").await.unwrap();

    // refresh with a changed source: drop ts, retype id, add region
    let fresh = vec![
        egret_bi::NewColumn { column_name: "id".into(), data_type: "varchar".into(), type_generic: "string".into(), is_temporal: false },
        egret_bi::NewColumn { column_name: "region".into(), data_type: "varchar".into(), type_generic: "string".into(), is_temporal: false },
    ];
    repo.reconcile_physical_columns(ds.id, &fresh).await.unwrap();

    let cols = repo.columns(ds.id).await.unwrap();
    let by = |n: &str| cols.iter().find(|c| c.column_name == n).cloned();
    // physical reconciled
    assert!(by("ts").is_none(), "dropped physical column removed");
    assert_eq!(by("region").unwrap().type_generic, "string", "new physical added");
    let id = by("id").unwrap();
    assert_eq!(id.data_type, "varchar", "physical retyped");
    assert_eq!(id.verbose_name, "Order ID", "user metadata preserved across refresh");
    // calculated column survives untouched
    let calc = by("id_x2").expect("calculated column preserved");
    assert_eq!(calc.expression.as_deref(), Some("id * 2"));
    repo.delete(ds.id).await.unwrap();
}
```

> This test references `repo.update_column` and `repo.add_calc_column` which are implemented
> in Task 4. To keep TDD order, you MAY write Task 4's repo methods first, or stub this test
> body and fill it after Task 4. Either way both tasks share `tests/dataset.rs` and must
> compile together before the suite passes.

### Step 5: Verify + commit

Run: `cargo fmt --all && cargo test -p egret-bi 2>&1 | tail && cargo clippy -p egret-bi --all-targets -- -D warnings 2>&1 | tail -3`
Expected: PASS (or `skip` without DB); clippy clean. (If Task 4 methods aren't in yet, this task's test won't compile — implement Tasks 3 and 4 together, commit once both compile.)

```bash
git add crates/egret-bi/src/dataset.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/dataset.rs
git commit -m "feat(dataset): reconcile physical columns on refresh (preserve calc cols + metadata)"
```

---

## Task 4: Metric + calculated-column + metadata repo methods

**Files:**
- Modify: `crates/egret-bi/src/dataset.rs`
- Test: `crates/egret-bi/tests/dataset.rs`

### Step 1: Add `Metric` + `NewMetric` types

In `dataset.rs`:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Metric {
    pub id: Uuid,
    pub metric_name: String,
    pub expression: String,
    pub data_type: String,
    pub type_generic: String,
    pub verbose_name: String,
    pub description: String,
    pub ordinal: i32,
}
```

### Step 2: Add the repo methods

Add to `impl DatasetRepo`. All map a unique violation to `BiError::Conflict` (mirrors the
repo-wide `is_unique_violation()` pattern, e.g. `crates/egret-database/src/repo.rs:88`):

```rust
    // ── metrics ──────────────────────────────────────────────────────────────
    pub async fn metrics(&self, dataset_id: Uuid) -> Result<Vec<Metric>, BiError> {
        Ok(sqlx::query_as::<_, Metric>(
            "SELECT id, metric_name, expression, data_type, type_generic, verbose_name, description, ordinal \
             FROM dataset_metrics WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_metric(
        &self, dataset_id: Uuid, name: &str, expression: &str,
        data_type: &str, type_generic: &str, verbose_name: &str, description: &str,
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "INSERT INTO dataset_metrics \
               (dataset_id, metric_name, expression, data_type, type_generic, verbose_name, description, ordinal) \
             VALUES ($1,$2,$3,$4,$5,$6,$7, \
               (SELECT COALESCE(MAX(ordinal),-1)+1 FROM dataset_metrics WHERE dataset_id=$1))",
        )
        .bind(dataset_id).bind(name).bind(expression)
        .bind(data_type).bind(type_generic).bind(verbose_name).bind(description)
        .execute(&self.db.pool).await;
        conflict_on_unique(r, "metric name already exists").map(|_| ())
    }

    pub async fn update_metric(
        &self, metric_id: Uuid, name: &str, expression: &str,
        data_type: &str, type_generic: &str, verbose_name: &str, description: &str,
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "UPDATE dataset_metrics SET metric_name=$2, expression=$3, data_type=$4, \
               type_generic=$5, verbose_name=$6, description=$7 WHERE id=$1",
        )
        .bind(metric_id).bind(name).bind(expression)
        .bind(data_type).bind(type_generic).bind(verbose_name).bind(description)
        .execute(&self.db.pool).await;
        conflict_on_unique(r, "metric name already exists").map(|_| ())
    }

    pub async fn delete_metric(&self, metric_id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM dataset_metrics WHERE id=$1")
            .bind(metric_id).execute(&self.db.pool).await?;
        Ok(())
    }

    /// dataset_id that a metric belongs to (for scoping the ownership/grant check).
    pub async fn metric_dataset(&self, metric_id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(sqlx::query_scalar("SELECT dataset_id FROM dataset_metrics WHERE id=$1")
            .bind(metric_id).fetch_optional(&self.db.pool).await?)
    }

    // ── calculated columns + metadata ────────────────────────────────────────
    #[allow(clippy::too_many_arguments)]
    pub async fn add_calc_column(
        &self, dataset_id: Uuid, name: &str, expression: &str,
        data_type: &str, type_generic: &str, is_temporal: bool, verbose_name: &str, description: &str,
    ) -> Result<(), BiError> {
        let r = sqlx::query(
            "INSERT INTO dataset_columns \
               (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression, verbose_name, description) \
             VALUES ($1,$2,$3,$4,$5, \
               (SELECT COALESCE(MAX(ordinal),-1)+1 FROM dataset_columns WHERE dataset_id=$1), $6,$7,$8)",
        )
        .bind(dataset_id).bind(name).bind(data_type).bind(type_generic).bind(is_temporal)
        .bind(expression).bind(verbose_name).bind(description)
        .execute(&self.db.pool).await;
        conflict_on_unique(r, "column name already exists").map(|_| ())
    }

    /// Edit a column's metadata; for calculated columns also its expression (+ inferred types).
    /// Pass `None` for a field to leave it unchanged. `expr_update` is `Some((expr, data_type,
    /// type_generic, is_temporal))` only when editing a calculated column's expression.
    pub async fn update_column(
        &self, dataset_id: Uuid, column_name: &str,
        verbose_name: Option<&str>, description: Option<&str>,
        expr_update: Option<(&str, &str, &str, bool)>,
    ) -> Result<(), BiError> {
        // metadata (verbose_name/description) — applies to any column
        sqlx::query(
            "UPDATE dataset_columns SET \
               verbose_name = COALESCE($3, verbose_name), \
               description  = COALESCE($4, description) \
             WHERE dataset_id=$1 AND column_name=$2",
        )
        .bind(dataset_id).bind(column_name).bind(verbose_name).bind(description)
        .execute(&self.db.pool).await?;

        // expression — only for an existing calculated column (expression IS NOT NULL)
        if let Some((expr, dt, tg, temporal)) = expr_update {
            sqlx::query(
                "UPDATE dataset_columns SET expression=$3, data_type=$4, type_generic=$5, is_temporal=$6 \
                 WHERE dataset_id=$1 AND column_name=$2 AND expression IS NOT NULL",
            )
            .bind(dataset_id).bind(column_name).bind(expr).bind(dt).bind(tg).bind(temporal)
            .execute(&self.db.pool).await?;
        }
        Ok(())
    }

    /// Delete a calculated column only. Returns Conflict-style BadRequest if physical/missing.
    pub async fn delete_calc_column(&self, dataset_id: Uuid, column_name: &str) -> Result<(), BiError> {
        let n = sqlx::query(
            "DELETE FROM dataset_columns WHERE dataset_id=$1 AND column_name=$2 AND expression IS NOT NULL",
        )
        .bind(dataset_id).bind(column_name).execute(&self.db.pool).await?.rows_affected();
        if n == 0 {
            return Err(BiError::BadRequest("only calculated columns can be deleted".into()));
        }
        Ok(())
    }

    /// Whether a column on this dataset is calculated (expression IS NOT NULL). None if missing.
    pub async fn column_is_calculated(&self, dataset_id: Uuid, column_name: &str) -> Result<Option<bool>, BiError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT (expression IS NOT NULL) FROM dataset_columns WHERE dataset_id=$1 AND column_name=$2",
        )
        .bind(dataset_id).bind(column_name).fetch_optional(&self.db.pool).await?)
    }
```

And add the helper at the bottom of the file (next to `insert_columns`):

```rust
/// Map a sqlx unique-violation to BiError::Conflict; pass other errors through.
fn conflict_on_unique(
    r: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
    msg: &str,
) -> Result<sqlx::postgres::PgQueryResult, BiError> {
    match r {
        Ok(v) => Ok(v),
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => Err(BiError::Conflict(msg.into())),
        Err(e) => Err(BiError::Db(e)),
    }
}
```

### Step 3: Also map dataset `create` name conflicts (closes the 3c-2 500 nit)

Wrap the dataset INSERT in `create()` so a duplicate `(workspace_id, name)` returns Conflict.
Change the `fetch_one(&mut *tx).await?` on the datasets INSERT to capture and map:

```rust
        let ds = match sqlx::query_as::<_, Dataset>(&format!(
            "INSERT INTO datasets (...) VALUES (...) RETURNING {COLS}"   // unchanged SQL
        ))
        .bind(/* ... unchanged binds ... */)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(d) => d,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(BiError::Conflict("a dataset with this name already exists".into()));
            }
            Err(e) => return Err(BiError::Db(e)),
        };
```

### Step 4: Tests (extend `tests/dataset.rs`)

Add metric + calc-column CRUD tests (DB-gated, reuse `setup()`):

```rust
#[tokio::test]
async fn metric_and_calc_column_crud() {
    let Some(ctx) = setup().await else { eprintln!("skip"); return };
    let repo = egret_bi::DatasetRepo::new(ctx.db.clone());
    let ds = repo.create(ctx.ws, ctx.db_id, "ds", "", &egret_bi::SourceSpec::Virtual { sql: "SELECT 1".into() },
        &[egret_bi::NewColumn { column_name: "n".into(), data_type: "bigint".into(), type_generic: "numeric".into(), is_temporal: false }],
        Some(ctx.user)).await.unwrap();

    repo.add_metric(ds.id, "total", "SUM(n)", "bigint", "numeric", "Total", "").await.unwrap();
    assert!(repo.add_metric(ds.id, "total", "COUNT(*)", "bigint", "numeric", "", "").await.is_err()); // dup → Conflict
    let ms = repo.metrics(ds.id).await.unwrap();
    assert_eq!(ms.len(), 1);
    repo.update_metric(ms[0].id, "total2", "COUNT(*)", "bigint", "numeric", "", "").await.unwrap();
    assert_eq!(repo.metric_dataset(ms[0].id).await.unwrap(), Some(ds.id));
    repo.delete_metric(ms[0].id).await.unwrap();
    assert!(repo.metrics(ds.id).await.unwrap().is_empty());

    repo.add_calc_column(ds.id, "n2", "n*2", "bigint", "numeric", false, "", "").await.unwrap();
    assert_eq!(repo.column_is_calculated(ds.id, "n2").await.unwrap(), Some(true));
    assert_eq!(repo.column_is_calculated(ds.id, "n").await.unwrap(), Some(false));
    assert!(repo.delete_calc_column(ds.id, "n").await.is_err()); // physical → rejected
    repo.delete_calc_column(ds.id, "n2").await.unwrap();
    repo.delete(ds.id).await.unwrap();
}
```

### Step 5: Verify + commit

Run: `cargo fmt --all && cargo test -p egret-bi 2>&1 | tail && cargo clippy -p egret-bi --all-targets -- -D warnings 2>&1 | tail -3`
Expected: PASS (or skip); clippy clean.

```bash
git add crates/egret-bi/src/dataset.rs crates/egret-bi/tests/dataset.rs
git commit -m "feat(dataset): metric + calculated-column + metadata repo methods"
```

---

## Task 5: `expr.rs` — expression validation

**Files:**
- Modify: `crates/egret-bi/src/schema.rs` (make quoting `pub(crate)`)
- Create: `crates/egret-bi/src/expr.rs`
- Modify: `crates/egret-bi/src/lib.rs` (`mod expr;`)
- Test: `crates/egret-bi/tests/expr.rs`

### Step 1: Expose the identifier quoting

In `crates/egret-bi/src/schema.rs`, change the two private free functions to crate-visible:

```rust
pub(crate) fn quote_trino(ident: &str) -> String { /* unchanged body */ }
pub(crate) fn quote_mysql(ident: &str) -> String { /* unchanged body */ }
```

### Step 2: Declare the module

In `crates/egret-bi/src/lib.rs`, add `mod expr;` (next to `mod schema;`) and to the `pub use` block:

```rust
pub use expr::{validate_expression, DescribedExpr};
```

### Step 3: Write `crates/egret-bi/src/expr.rs`

```rust
//! Validate a metric/calculated-column expression by probing the engine, and infer its type.

use std::time::Duration;

use egret_database::model::Engine;
use egret_engine::connector::{build_connector, ConnSpec};

use crate::dataset::SourceSpec;
use crate::error::BiError;
use crate::schema::{quote_mysql, quote_trino};
use crate::typegen::generic_type;

/// The inferred result type of a validated expression.
#[derive(Debug, Clone)]
pub struct DescribedExpr {
    pub data_type: String,
    pub type_generic: String,
}

/// Reject empty or multi-statement expressions. A `;` outside string literals would let the
/// expression smuggle a second statement into the probe; one statement only.
fn guard_single_statement(expr: &str) -> Result<(), BiError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(BiError::BadRequest("expression is empty".into()));
    }
    let mut in_str = false;
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                if in_str && chars.peek() == Some(&'\'') {
                    chars.next(); // escaped '' inside a string literal
                } else {
                    in_str = !in_str;
                }
            }
            ';' if !in_str => {
                return Err(BiError::BadRequest("expression must be a single statement".into()));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Build the `FROM <source>` reference for the probe.
fn source_ref(spec: &ConnSpec, source: &SourceSpec) -> String {
    match source {
        SourceSpec::Physical { catalog, schema, table } => match spec.engine {
            Engine::Trino => {
                let cat = catalog.as_deref().unwrap_or("");
                format!("{}.{}.{}", quote_trino(cat), quote_trino(schema), quote_trino(table))
            }
            Engine::StarRocks => format!("{}.{}", quote_mysql(schema), quote_mysql(table)),
        },
        SourceSpec::Virtual { sql } => format!("(\n{sql}\n) AS _egret_src"),
    }
}

/// Validate `expr` against `source` on `spec`'s engine and return its inferred type.
/// Reuses `Connector::describe`, which wraps the probe in `SELECT * FROM (...) LIMIT 0`.
pub async fn validate_expression(
    spec: &ConnSpec,
    source: &SourceSpec,
    expr: &str,
) -> Result<DescribedExpr, BiError> {
    guard_single_statement(expr)?;
    let probe = format!("SELECT {expr} AS _egret_expr FROM {}", source_ref(spec, source));
    let cols = build_connector(spec)
        .describe(&probe, Duration::from_secs(15))
        .await
        .map_err(BiError::from)?;
    let data_type = cols
        .into_iter()
        .next()
        .map(|c| c.ty)
        .ok_or_else(|| BiError::Engine("expression produced no column".into()))?;
    let g = generic_type(spec.engine, &data_type);
    Ok(DescribedExpr { data_type, type_generic: g.as_str().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_accepts_normal_expressions() {
        for e in ["SUM(x)", "price * qty", "EXTRACT(year FROM ts)", "concat(a, ';', b)"] {
            assert!(guard_single_statement(e).is_ok(), "{e}");
        }
    }

    #[test]
    fn guard_rejects_multi_statement_and_empty() {
        assert!(guard_single_statement("").is_err());
        assert!(guard_single_statement("   ").is_err());
        assert!(guard_single_statement("1; DROP TABLE t").is_err());
        // a `;` inside a string literal is allowed
        assert!(guard_single_statement("concat(a, ';', b)").is_ok());
    }
}
```

> `EngineError → BiError` already has a `From` impl (`error.rs`), so `.map_err(BiError::from)`
> works for the describe call.

### Step 4: Engine-gated test `crates/egret-bi/tests/expr.rs`

Mirror `crates/egret-engine/tests/describe.rs` for the Trino self-skip + `ConnSpec`
construction (env `EGRET_TEST_TRINO_URL`). Then:

```rust
// Build a ConnSpec to local Trino exactly like crates/egret-engine/tests/describe.rs does;
// self-skip if the env var / probe is absent.

#[tokio::test]
async fn valid_expression_infers_type() {
    let Some(spec) = trino_spec() else { eprintln!("skip: no Trino"); return };
    let src = egret_bi::SourceSpec::Physical { catalog: Some("tpch".into()), schema: "tiny".into(), table: "orders".into() };
    let d = egret_bi::validate_expression(&spec, &src, "sum(totalprice)").await.expect("valid");
    assert_eq!(d.type_generic, "numeric");
}

#[tokio::test]
async fn invalid_expression_errors() {
    let Some(spec) = trino_spec() else { eprintln!("skip: no Trino"); return };
    let src = egret_bi::SourceSpec::Physical { catalog: Some("tpch".into()), schema: "tiny".into(), table: "orders".into() };
    assert!(egret_bi::validate_expression(&spec, &src, "no_such_fn(totalprice)").await.is_err());
}
```

> `validate_expression`, `DescribedExpr`, and `SourceSpec` must be re-exported from `lib.rs`
> for the integration test (SourceSpec already is; add validate_expression/DescribedExpr in
> Step 2).

### Step 5: Verify + commit

Run: `cargo fmt --all && cargo test -p egret-bi expr 2>&1 | tail && cargo clippy -p egret-bi --all-targets -- -D warnings 2>&1 | tail -3`
Expected: unit tests PASS; engine-gated tests PASS or `skip`; clippy clean.

```bash
git add crates/egret-bi/src/expr.rs crates/egret-bi/src/schema.rs crates/egret-bi/src/lib.rs crates/egret-bi/tests/expr.rs
git commit -m "feat(dataset): expression validation via describe-probe + single-statement guard"
```

---

## Task 6: DTOs + routes wiring

**Files:**
- Modify: `crates/egret-bi/src/model.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Modify: `crates/egret-bi/src/lib.rs` (export `Metric` if needed by tests — already covered)

### Step 1: DTOs + request bodies in `model.rs`

Extend `DatasetColumnDto` and `DatasetDetailDto`, add `MetricDto`, and the request bodies.
Also import `Metric` from the dataset module.

```rust
use crate::dataset::{Dataset, DatasetColumn, Metric};   // extend the existing `use crate::dataset::...`

#[derive(Debug, Serialize)]
pub struct DatasetColumnDto {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
    pub expression: Option<String>,
    pub verbose_name: String,
    pub description: String,
    pub is_calculated: bool,
}

#[derive(Debug, Serialize)]
pub struct MetricDto {
    pub id: String,
    pub metric_name: String,
    pub expression: String,
    pub data_type: String,
    pub type_generic: String,
    pub verbose_name: String,
    pub description: String,
    pub ordinal: i32,
}

#[derive(Debug, Deserialize)]
pub struct MetricBody {
    pub metric_name: String,
    pub expression: String,
    #[serde(default)] pub verbose_name: String,
    #[serde(default)] pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct AddColumnBody {
    pub column_name: String,
    pub expression: String,
    #[serde(default)] pub verbose_name: String,
    #[serde(default)] pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateColumnBody {
    #[serde(default)] pub verbose_name: Option<String>,
    #[serde(default)] pub description: Option<String>,
    #[serde(default)] pub expression: Option<String>,
}

pub fn metric_dto(m: Metric) -> MetricDto {
    MetricDto {
        id: m.id.to_string(),
        metric_name: m.metric_name,
        expression: m.expression,
        data_type: m.data_type,
        type_generic: m.type_generic,
        verbose_name: m.verbose_name,
        description: m.description,
        ordinal: m.ordinal,
    }
}
```

Add `metrics` to `DatasetDetailDto` and change `detail_dto` to take metrics:

```rust
#[derive(Debug, Serialize)]
pub struct DatasetDetailDto {
    // ...existing fields...
    pub columns: Vec<DatasetColumnDto>,
    pub metrics: Vec<MetricDto>,
}

pub fn detail_dto(ds: Dataset, columns: Vec<DatasetColumn>, metrics: Vec<Metric>) -> DatasetDetailDto {
    DatasetDetailDto {
        // ...existing field maps...
        columns: columns.into_iter().map(|c| DatasetColumnDto {
            is_calculated: c.expression.is_some(),
            column_name: c.column_name,
            data_type: c.data_type,
            type_generic: c.type_generic,
            is_temporal: c.is_temporal,
            ordinal: c.ordinal,
            expression: c.expression,
            verbose_name: c.verbose_name,
            description: c.description,
        }).collect(),
        metrics: metrics.into_iter().map(metric_dto).collect(),
    }
}
```

### Step 2: `load_detail` helper + update existing handlers in `routes.rs`

Add a helper that fetches ds + columns + metrics and builds the DTO, then use it in
`ds_create`/`ds_get`/`ds_update`/`ds_refresh` (replacing their `detail_dto(ds, cols)` tails):

```rust
async fn load_detail(st: &BiState, id: Uuid) -> Result<DatasetDetailDto, BiApiError> {
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    let metrics = st.datasets.metrics(id).await.map_err(BiApiError::from)?;
    Ok(crate::model::detail_dto(ds, cols, metrics))
}
```

Replace each handler's final two lines (`let cols = ...; Ok(Json(detail_dto(ds, cols)))`) with
`Ok(Json(load_detail(&st, id).await?))`. For `ds_create`, after `create(...)` returns `ds`,
use `load_detail(&st, ds.id).await?`.

### Step 3: Add the new routes + handlers

Extend `dataset_router()`:

```rust
        .route("/ws/{workspace}/datasets/{id}/metrics", post(ds_add_metric))
        .route("/ws/{workspace}/datasets/{id}/metrics/{metric_id}", put(ds_update_metric).delete(ds_delete_metric))
        .route("/ws/{workspace}/datasets/{id}/columns", post(ds_add_column))
        .route("/ws/{workspace}/datasets/{id}/columns/{column_name}", put(ds_update_column).delete(ds_delete_column))
```

(Add `put` to the `axum::routing` import if not present.)

Handlers — each authorizes `DS_MANAGE` + owner, resolves the spec via `conn_spec_perm`,
validates the expression, persists, and returns the refreshed detail. Add imports:
`use crate::expr::validate_expression;` and the new model bodies.

```rust
async fn ds_add_metric(
    State(st): State<BiState>, WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>, Json(b): Json<crate::model::MetricBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?.ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
    let d = validate_expression(&spec, &source_of(&ds), &b.expression).await.map_err(BiApiError::from)?;
    st.datasets.add_metric(id, &b.metric_name, &b.expression, &d.data_type, &d.type_generic, &b.verbose_name, &b.description)
        .await.map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_update_metric(
    State(st): State<BiState>, WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>, Json(b): Json<crate::model::MetricBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?.ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
    let d = validate_expression(&spec, &source_of(&ds), &b.expression).await.map_err(BiApiError::from)?;
    st.datasets.update_metric(metric_id, &b.metric_name, &b.expression, &d.data_type, &d.type_generic, &b.verbose_name, &b.description)
        .await.map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_delete_metric(
    State(st): State<BiState>, WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    st.datasets.delete_metric(metric_id).await.map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_add_column(
    State(st): State<BiState>, WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>, Json(b): Json<crate::model::AddColumnBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?.ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
    let d = validate_expression(&spec, &source_of(&ds), &b.expression).await.map_err(BiApiError::from)?;
    st.datasets.add_calc_column(id, &b.column_name, &b.expression, &d.data_type, &d.type_generic, d.type_generic == "temporal", &b.verbose_name, &b.description)
        .await.map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_update_column(
    State(st): State<BiState>, WsContext(ctx): WsContext,
    Path((_w, id, column_name)): Path<(String, Uuid, String)>, Json(b): Json<crate::model::UpdateColumnBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    // editing an expression is only valid for a calculated column
    let expr_update = if let Some(expr) = b.expression.as_deref() {
        match st.datasets.column_is_calculated(id, &column_name).await.map_err(BiApiError::from)? {
            Some(true) => {
                let ds = st.datasets.get(id).await.map_err(BiApiError::from)?.ok_or(Error::NotFound("dataset".into()))?;
                let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
                let d = validate_expression(&spec, &source_of(&ds), expr).await.map_err(BiApiError::from)?;
                Some((expr.to_string(), d.data_type, d.type_generic.clone(), d.type_generic == "temporal"))
            }
            Some(false) => return Err(crate::BiError::BadRequest("cannot set an expression on a physical column".into()).into()),
            None => return Err(Error::NotFound("column".into()).into()),
        }
    } else {
        // metadata-only edit still needs DS_MANAGE on the workspace
        st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
        None
    };
    let eu = expr_update.as_ref().map(|(e, dt, tg, t)| (e.as_str(), dt.as_str(), tg.as_str(), *t));
    st.datasets.update_column(id, &column_name, b.verbose_name.as_deref(), b.description.as_deref(), eu)
        .await.map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_delete_column(
    State(st): State<BiState>, WsContext(ctx): WsContext,
    Path((_w, id, column_name)): Path<(String, Uuid, String)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    st.datasets.delete_calc_column(id, &column_name).await.map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}
```

> Note: `require_dataset_owner` only checks ownership, not the workspace permission. The
> metric/add-column handlers call `conn_spec_perm(..., DS_MANAGE)` which enforces DS_MANAGE;
> the metadata-only and delete paths add an explicit `require_workspace(DS_MANAGE)`. Keep
> both so every mutation is gated. `ds_update_metric`/`ds_delete_metric` use `metric_id`
> from the path but scope via `require_dataset_owner(id)` — acceptable since the route nests
> the metric under its dataset id; optionally assert `metric_dataset(metric_id) == Some(id)`
> for defense-in-depth (do it if trivial).

### Step 4: Verify + commit

Run: `cargo fmt --all && cargo build --workspace 2>&1 | tail && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | tail`
Expected: builds, clippy clean, tests pass/skip. Fix the `detail_dto` arity change at every call site (the compiler lists them).

```bash
git add crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs
git commit -m "feat(dataset): metric + calculated-column REST routes with expression validation"
```

---

## Task 7: Frontend API hooks

**Files:** Modify `frontend/src/api/datasets.ts`

- [ ] **Step 1: Extend types + add hooks**

Extend `DatasetColumn`, add `DatasetMetric`, extend `DatasetDetail`, and add CRUD hooks.
Match the existing POST/PUT/DELETE convention in the file.

```ts
export interface DatasetColumn {
  column_name: string; data_type: string; type_generic: string; is_temporal: boolean; ordinal: number;
  expression: string | null; verbose_name: string; description: string; is_calculated: boolean;
}
export interface DatasetMetric {
  id: string; metric_name: string; expression: string; data_type: string; type_generic: string;
  verbose_name: string; description: string; ordinal: number;
}
export interface DatasetDetail {
  id: string; name: string; description: string; database_id: string; kind: DatasetKind;
  catalog: string | null; schema: string | null; table: string | null; sql: string | null;
  updated_at: string; columns: DatasetColumn[]; metrics: DatasetMetric[];
}

function mutate(ws: string, id: string) {
  return (path: string, method: string, body?: unknown) =>
    apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}${path}`, {
      method,
      headers: body ? { "content-type": "application/json" } : undefined,
      body: body ? JSON.stringify(body) : undefined,
    });
}

export function useDatasetMutations(ws: string, id: string) {
  const qc = useQueryClient();
  const inval = () => qc.invalidateQueries({ queryKey: dsKeys.detail(ws, id) });
  const m = mutate(ws, id);
  return {
    addMetric: useMutation({ mutationFn: (b: { metric_name: string; expression: string; verbose_name?: string; description?: string }) => m(`/metrics`, "POST", b), onSuccess: inval }),
    updateMetric: useMutation({ mutationFn: (a: { metricId: string; body: { metric_name: string; expression: string; verbose_name?: string; description?: string } }) => m(`/metrics/${a.metricId}`, "PUT", a.body), onSuccess: inval }),
    deleteMetric: useMutation({ mutationFn: (metricId: string) => m(`/metrics/${metricId}`, "DELETE"), onSuccess: inval }),
    addColumn: useMutation({ mutationFn: (b: { column_name: string; expression: string; verbose_name?: string; description?: string }) => m(`/columns`, "POST", b), onSuccess: inval }),
    updateColumn: useMutation({ mutationFn: (a: { name: string; body: { verbose_name?: string; description?: string; expression?: string } }) => m(`/columns/${encodeURIComponent(a.name)}`, "PUT", a.body), onSuccess: inval }),
    deleteColumn: useMutation({ mutationFn: (name: string) => m(`/columns/${encodeURIComponent(name)}`, "DELETE"), onSuccess: inval }),
  };
}
```

> Confirm `apiJson` accepts `headers: undefined` (the existing `useRefreshDataset` POSTs with
> no body — match that). If it doesn't, always send the content-type header.

- [ ] **Step 2: Verify + commit**

Run: `cd frontend && npm run lint 2>&1 | tail -3 && npm run typecheck 2>&1 | tail -2`
Expected: clean.

```bash
git add frontend/src/api/datasets.ts
git commit -m "feat(dataset): frontend hooks for metrics + calculated columns"
```

---

## Task 8: Frontend — Columns / Metrics tabs

**Files:**
- Create: `frontend/src/components/dataset/ColumnsTab.tsx`
- Create: `frontend/src/components/dataset/MetricsTab.tsx`
- Modify: `frontend/src/routes/explore/DatasetDetail.tsx`
- Create: `frontend/src/components/dataset/__tests__/DatasetTabs.test.tsx`

Read `DatasetDetail.tsx` (it already renders the columns `<table>`), and reuse the shadcn
`Tabs`, `Input`, `Textarea`, `Button`, `AlertDialog` primitives already imported elsewhere
in the dataset components.

### Step 1: `ColumnsTab.tsx`

A table of `detail.columns`: column_name, data_type, generic-type badge, temporal flag, a
**calculated** badge when `is_calculated`, verbose_name (inline edit), description (inline
edit). A **+ Calculated column** button opens a small form (`column_name`, `expression`,
optional verbose_name/description) → `mutations.addColumn`. Calculated rows get a delete
button → `mutations.deleteColumn`. Editing verbose_name/description (any column) calls
`mutations.updateColumn({ name, body: { verbose_name, description } })`; editing a calculated
column's expression sends `{ expression }`. Surface mutation errors with `toast.error` (the
engine message on a bad expression comes back here).

Props: `{ ws: string; datasetId: string; columns: DatasetColumn[] }`. Build mutations via
`useDatasetMutations(ws, datasetId)`.

### Step 2: `MetricsTab.tsx`

A table of `detail.metrics`: metric_name, expression (mono), generic-type badge, verbose_name.
**+ Metric** / edit / delete forms wired to `addMetric`/`updateMetric`/`deleteMetric`. Same
toast-on-error.

Props: `{ ws: string; datasetId: string; metrics: DatasetMetric[] }`.

### Step 3: Wire tabs into `DatasetDetail.tsx`

Below the existing header block (name/kind/source + Refresh/Edit/Delete), replace the inline
columns `<table>` with a `<Tabs>`:

```tsx
<Tabs defaultValue="columns" className="mt-4">
  <TabsList>
    <TabsTrigger value="columns">Columns ({ds.columns.length})</TabsTrigger>
    <TabsTrigger value="metrics">Metrics ({ds.metrics.length})</TabsTrigger>
  </TabsList>
  <TabsContent value="columns"><ColumnsTab ws={ws} datasetId={id} columns={ds.columns} /></TabsContent>
  <TabsContent value="metrics"><MetricsTab ws={ws} datasetId={id} metrics={ds.metrics} /></TabsContent>
</Tabs>
```

Import `Tabs, TabsContent, TabsList, TabsTrigger` from `@/components/ui/tabs` and the two new
components. The existing column-rendering markup moves into `ColumnsTab`.

### Step 4: Vitest

`frontend/src/components/dataset/__tests__/DatasetTabs.test.tsx` — copy the provider harness
(QueryClientProvider + MemoryRouter) from an existing dataset test. Render `ColumnsTab` and
`MetricsTab` with a mocked `useDatasetMutations` (or a real QueryClient) and a small fixture;
assert: a calculated column shows the **calculated** badge; the metrics table renders a metric
row; the **+ Calculated column** and **+ Metric** buttons render.

### Step 5: Verify + commit

Run: `cd frontend && npm run lint 2>&1 | tail -3 && npm run typecheck 2>&1 | tail -2 && npm test -- --run 2>&1 | tail -6 && npm run build 2>&1 | tail -3`
Expected: lint clean, tsc clean, tests pass, build succeeds.

```bash
git add frontend/src/routes/explore/DatasetDetail.tsx frontend/src/components/dataset/
git commit -m "feat(dataset): tabbed detail — Columns + Metrics editors"
```

---

## Final verification (after all tasks)

- [ ] **Whole-workspace + frontend**

Run:
```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd frontend && npm run lint && npm run typecheck && npm test -- --run && npm run build
```
Expected: all green (DB-/engine-gated tests self-skip when infra absent).

- [ ] **Manual smoke (if the dev stack is up)**

On a physical dataset over `tpch.tiny.orders`: add a metric `SUM(totalprice)` (→ numeric),
add a calculated column `totalprice / 100` (→ numeric), edit a column's verbose_name, then
**Refresh** and confirm the metric, the calculated column, and the edited verbose_name all
survive. Add a bad expression (`nope(x)`) and confirm a toast with the engine error and that
nothing persisted.

---

## Notes for the executor

- The reconcile `ON CONFLICT ... DO UPDATE ... WHERE dataset_columns.expression IS NULL` is
  the crux: it makes a physical-vs-calculated name collision leave the calculated column
  intact. Don't drop the `WHERE`.
- `type_generic == "temporal"` is the rule for a calculated column's `is_temporal` — keep it
  consistent with `typegen`.
- Don't return or log DB secrets; expression validation passes the `ConnSpec` to the
  connector but never serializes it.
- Every dataset mutation returns the full refreshed `DatasetDetail` so the SPA re-renders
  from one response — keep `load_detail` as the single tail for all of them.
