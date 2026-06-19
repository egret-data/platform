# Phase 3c-2 Dataset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the Dataset — a named, governable physical-or-virtual source backed by a registered database, carrying a synced snapshot of typed columns — as the object charts (3c-3) will build on.

**Architecture:** Extend the `egret-bi` crate (it already owns `SchemaService`, `BiState`, RBAC + owner-gate patterns) with a `DatasetRepo`, a pure `typegen` column-type classifier, and a `dataset_router`. Add one engine capability in `egret-engine`: a `Connector::describe(sql)` that returns typed columns for an arbitrary `SELECT` (default impl for Trino + the fake; StarRocks overrides for a clean type vocabulary). Routes orchestrate engine introspection + DB persistence; the repo stays a pure DB unit.

**Tech Stack:** Rust (axum 0.8, sqlx 0.8 runtime queries, async-trait, tokio, mysql_async, reqwest), Postgres, React 18 + TS + Vite + @tanstack/react-query 5 + Vitest.

**Spec:** `docs/en/specs/2026-06-18-phase-3c-2-dataset-design.md` (vi mirror alongside).

**Conventions (must follow):**
- Plain `git commit` with the configured identity — never pass `--author`.
- Runtime sqlx only (`sqlx::query`, `sqlx::query_as`, `query_scalar`) — never the `query!`/`query_as!` compile-time macros.
- DB secrets are never returned or logged.
- DB-gated tests self-skip when Postgres is absent; engine-gated tests self-skip when Trino:8090 / StarRocks:9030 are absent (follow the existing pattern in `crates/egret-engine/tests/`).
- Work on branch `feat/phase-3c-2-dataset` (already created; the spec commit is its first commit).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-bi/src/typegen.rs` (new) | Pure `GenericType` + `generic_type(engine, raw_type)` classifier |
| `crates/egret-engine/src/connector/mod.rs` (modify) | Add `describe` (default impl) to the `Connector` trait |
| `crates/egret-engine/src/connector/starrocks.rs` (modify) | Override `describe`; `coltype_name` MySQL→canonical mapping |
| `crates/egret-engine/tests/describe.rs` (new) | Engine-gated `describe` tests (Trino + StarRocks) |
| `migrations/0012_dataset.sql` (new) | `datasets` + `dataset_columns` tables + RBAC seed |
| `crates/egret-bi/src/dataset.rs` (new) | `DatasetRepo` — CRUD, column snapshot, replace-columns |
| `crates/egret-bi/src/model.rs` (modify) | Dataset DTOs + request bodies |
| `crates/egret-bi/src/routes.rs` (modify) | `dataset_router` + handlers (introspect + persist) |
| `crates/egret-bi/src/lib.rs` (modify) | Export `dataset_router`, `DatasetRepo`; add module |
| `crates/egret-bi/tests/dataset.rs` (new) | DB-gated `DatasetRepo` tests |
| `crates/egret-server/src/lib.rs` (modify) | Merge `dataset_router` into the bi route group |
| `crates/egret-bin/src/main.rs` (modify) | Add `datasets: DatasetRepo::new(db)` to `BiState` |
| `frontend/src/api/datasets.ts` (new) | react-query hooks + types |
| `frontend/src/routes/explore/Datasets.tsx` (rewrite) | List page |
| `frontend/src/components/dataset/CreateDatasetDialog.tsx` (new) | Create flow (from-table / from-SQL) |
| `frontend/src/routes/explore/DatasetDetail.tsx` (new) | Detail page (columns, refresh, edit, delete) |
| `frontend/src/App.tsx` (modify) | Register `datasets/:id` route |
| `frontend/src/components/sqllab/SqlTab.tsx` (modify) | "Save as dataset" toolbar action |
| `frontend/src/components/dataset/__tests__/*` (new) | Vitest for list + create + bridge |

---

## Task 1: `typegen` — pure column-type classifier

**Files:**
- Create: `crates/egret-bi/src/typegen.rs`
- Modify: `crates/egret-bi/src/lib.rs`

This is the single source of truth charts (3c-3) rely on to split dimensions / measures / time. Pure, no DB, no engine — exhaustively unit-tested.

- [ ] **Step 1: Add the module declaration**

In `crates/egret-bi/src/lib.rs`, add after the existing `mod schema;` line:

```rust
mod typegen;
```

And add to the `pub use` exports block:

```rust
pub use typegen::{generic_type, GenericType};
```

- [ ] **Step 2: Write the failing tests**

Create `crates/egret-bi/src/typegen.rs`:

```rust
//! Pure classification of an engine column type into a generic class.
//! This is the single source of truth charts rely on to tell dimensions,
//! measures, and time columns apart.

use egret_database::model::Engine;

/// The generic class of a column, independent of engine-specific type spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericType {
    String,
    Numeric,
    Temporal,
    Boolean,
}

impl GenericType {
    pub fn as_str(self) -> &'static str {
        match self {
            GenericType::String => "string",
            GenericType::Numeric => "numeric",
            GenericType::Temporal => "temporal",
            GenericType::Boolean => "boolean",
        }
    }
    /// Whether this column is a time column (drives time-series chart axes).
    pub fn is_temporal(self) -> bool {
        matches!(self, GenericType::Temporal)
    }
}

/// Classify a raw engine type string (e.g. "varchar(20)", "decimal(10,2)",
/// "timestamp with time zone") into a `GenericType`. Matching is
/// case-insensitive on the leading keyword; precision/length suffixes are
/// ignored. Unknown types fall back to `String` (safe — usable as a dimension).
pub fn generic_type(engine: Engine, raw_type: &str) -> GenericType {
    let head = type_head(raw_type);
    match engine {
        Engine::Trino => classify_trino(&head),
        Engine::StarRocks => classify_starrocks(&head),
    }
}

/// Lowercased leading keyword: strip any "(...)" suffix and trailing tokens,
/// but keep multi-word temporal types (e.g. "timestamp with time zone").
fn type_head(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let no_paren = lowered.split('(').next().unwrap_or("").trim();
    no_paren.to_string()
}

fn classify_trino(head: &str) -> GenericType {
    let first = head.split_whitespace().next().unwrap_or("");
    match first {
        "tinyint" | "smallint" | "integer" | "int" | "bigint" | "real" | "double"
        | "decimal" => GenericType::Numeric,
        "date" | "time" | "timestamp" => GenericType::Temporal,
        "boolean" => GenericType::Boolean,
        _ => GenericType::String,
    }
}

fn classify_starrocks(head: &str) -> GenericType {
    let first = head.split_whitespace().next().unwrap_or("");
    match first {
        "tinyint" | "smallint" | "int" | "bigint" | "largeint" | "float" | "double"
        | "decimal" | "decimalv3" => GenericType::Numeric,
        "date" | "datetime" | "time" => GenericType::Temporal,
        "boolean" => GenericType::Boolean,
        _ => GenericType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trino_numeric_variants() {
        for t in ["bigint", "INTEGER", "double", "decimal(10,2)", "real", "tinyint"] {
            assert_eq!(generic_type(Engine::Trino, t), GenericType::Numeric, "{t}");
        }
    }

    #[test]
    fn trino_temporal_including_tz() {
        assert_eq!(generic_type(Engine::Trino, "date"), GenericType::Temporal);
        assert_eq!(
            generic_type(Engine::Trino, "timestamp(3) with time zone"),
            GenericType::Temporal
        );
        assert!(generic_type(Engine::Trino, "timestamp").is_temporal());
    }

    #[test]
    fn trino_string_and_boolean() {
        assert_eq!(generic_type(Engine::Trino, "varchar(255)"), GenericType::String);
        assert_eq!(generic_type(Engine::Trino, "json"), GenericType::String);
        assert_eq!(generic_type(Engine::Trino, "boolean"), GenericType::Boolean);
    }

    #[test]
    fn starrocks_variants() {
        assert_eq!(generic_type(Engine::StarRocks, "int"), GenericType::Numeric);
        assert_eq!(generic_type(Engine::StarRocks, "largeint"), GenericType::Numeric);
        assert_eq!(generic_type(Engine::StarRocks, "DATETIME"), GenericType::Temporal);
        assert_eq!(generic_type(Engine::StarRocks, "varchar(20)"), GenericType::String);
        assert_eq!(generic_type(Engine::StarRocks, "boolean"), GenericType::Boolean);
    }

    #[test]
    fn unknown_falls_back_to_string() {
        assert_eq!(generic_type(Engine::Trino, "hyperloglog"), GenericType::String);
        assert_eq!(generic_type(Engine::StarRocks, "bitmap"), GenericType::String);
        assert_eq!(generic_type(Engine::Trino, ""), GenericType::String);
    }

    #[test]
    fn as_str_values() {
        assert_eq!(GenericType::Numeric.as_str(), "numeric");
        assert_eq!(GenericType::Temporal.as_str(), "temporal");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p egret-bi typegen`
Expected: FAIL to compile until the module exists (it now does after Step 2) → then PASS. If you wrote Step 2 fully, run anyway to confirm green.

- [ ] **Step 4: Verify pass + lint**

Run: `cargo test -p egret-bi typegen && cargo clippy -p egret-bi --all-targets -- -D warnings`
Expected: PASS, no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bi/src/typegen.rs crates/egret-bi/src/lib.rs
git commit -m "feat(dataset): pure type_generic column classifier"
```

---

## Task 2: `Connector::describe` — typed columns for an arbitrary SELECT

**Files:**
- Modify: `crates/egret-engine/src/connector/mod.rs`
- Modify: `crates/egret-engine/src/connector/starrocks.rs`
- Test: `crates/egret-engine/tests/describe.rs` (new)

Trino's `execute` already returns clean Trino type names, so the trait's **default** `describe` (wrap as `LIMIT 0`, reuse `execute`, return columns) is correct for Trino and harmless for the test `FakeConnector`. StarRocks' `execute` returns debug `{:?}` MySQL enum strings (`MYSQL_TYPE_LONG`), which would not match the `DESCRIBE` vocabulary physical datasets use — so StarRocks **overrides** `describe` to emit canonical lowercase names.

- [ ] **Step 1: Add `describe` (with default) to the `Connector` trait**

In `crates/egret-engine/src/connector/mod.rs`, add `use std::time::Duration;` at the top, and add this method to the `trait Connector` block (after `execute`):

```rust
    /// Return the typed columns of an arbitrary `SELECT` without fetching rows.
    /// Default: wrap the query as `LIMIT 0` and reuse `execute`'s column metadata —
    /// correct for engines whose `execute` already yields clean type names (Trino).
    /// Engines whose `execute` emits protocol-level type tags override this.
    async fn describe(&self, sql: &str, timeout: Duration) -> Result<Vec<crate::model::Column>, EngineError> {
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let limits = Limits { max_rows: 0, timeout };
        let r = self.execute(&wrapped, &limits, CancellationToken::new()).await?;
        Ok(r.columns)
    }
```

(`Limits` and `CancellationToken` are already imported in this file.)

- [ ] **Step 2: Override `describe` in StarRocks + add the type-name mapping**

In `crates/egret-engine/src/connector/starrocks.rs`, add `use std::time::Duration;` at the top, then add inside `impl Connector for StarRocksConnector` (after `execute`):

```rust
    async fn describe(
        &self,
        sql: &str,
        _timeout: Duration,
    ) -> Result<Vec<Column>, EngineError> {
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let mut conn = mysql_async::Conn::new(self.opts.clone())
            .await
            .map_err(|e| EngineError::Connect(e.to_string()))?;
        let result = conn
            .query_iter(wrapped)
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;
        let columns: Vec<Column> = result
            .columns_ref()
            .iter()
            .map(|c| Column {
                name: c.name_str().to_string(),
                ty: coltype_name(c.column_type()).to_string(),
            })
            .collect();
        drop(result);
        let _ = conn.disconnect().await;
        Ok(columns)
    }
```

And add this free function at the bottom of the file:

```rust
/// Map a MySQL/StarRocks protocol column type to a canonical lowercase SQL name,
/// matching the vocabulary `DESCRIBE` returns so `typegen` classifies both the
/// physical (DESCRIBE) and virtual (describe) paths identically.
fn coltype_name(ty: mysql_async::consts::ColumnType) -> &'static str {
    use mysql_async::consts::ColumnType::*;
    match ty {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => "datetime",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING => "varchar",
        _ => "string",
    }
}
```

> Note: StarRocks `BOOLEAN` is stored as `tinyint` at the protocol level, so a boolean
> column in a *virtual* dataset classifies as `numeric` (physical datasets get `boolean`
> from `DESCRIBE`). Acceptable v1 edge — documented in the spec.

- [ ] **Step 3: Write engine-gated describe tests**

Look at an existing engine-gated test (e.g. `crates/egret-engine/tests/job.rs` or any test that probes Trino:8090 / StarRocks:9030) to copy the exact self-skip helper and `ConnSpec` construction used in this repo. Then create `crates/egret-engine/tests/describe.rs` following that pattern:

```rust
//! Engine-gated tests for Connector::describe. Self-skip when the engine is absent.

use std::time::Duration;
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};

// Reuse the repo's existing reachability helper pattern. If the existing tests use a
// TcpStream probe to host:port with a short timeout, replicate it here returning Option<()>.
fn trino_spec() -> Option<ConnSpec> {
    // Construct a ConnSpec pointing at the local Trino (host/port/options exactly as the
    // other engine-gated tests do). Return None if a TCP probe to the port fails.
    probe("127.0.0.1", 8090).then(|| ConnSpec {
        engine: Engine::Trino,
        host: "127.0.0.1".into(),
        port: 8090,
        options: ConnOptions { http_scheme: Some("http".into()), user: Some("egret".into()), ..Default::default() },
        secret: None,
    })
}

fn starrocks_spec() -> Option<ConnSpec> {
    probe("127.0.0.1", 9030).then(|| ConnSpec {
        engine: Engine::StarRocks,
        host: "127.0.0.1".into(),
        port: 9030,
        options: ConnOptions { user: Some("root".into()), ..Default::default() },
        secret: None,
    })
}

fn probe(host: &str, port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("{host}:{port}").parse().unwrap(),
        Duration::from_millis(300),
    )
    .is_ok()
}

#[tokio::test]
async fn trino_describe_types() {
    let Some(spec) = trino_spec() else { eprintln!("skip: no Trino"); return };
    let conn = build_connector(&spec);
    let cols = conn
        .describe("SELECT CAST(1 AS bigint) AS n, CAST('x' AS varchar) AS s", Duration::from_secs(15))
        .await
        .expect("describe");
    let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"n") && names.contains(&"s"));
    let n = cols.iter().find(|c| c.name == "n").unwrap();
    assert!(n.ty.to_lowercase().contains("bigint"));
}

#[tokio::test]
async fn starrocks_describe_canonical_names() {
    let Some(spec) = starrocks_spec() else { eprintln!("skip: no StarRocks"); return };
    let conn = build_connector(&spec);
    let cols = conn
        .describe("SELECT 1 AS n, 'x' AS s", Duration::from_secs(15))
        .await
        .expect("describe");
    let n = cols.iter().find(|c| c.name == "n").expect("col n");
    // canonical lowercase SQL name, NOT a MYSQL_TYPE_* debug string
    assert!(!n.ty.starts_with("MYSQL_TYPE"));
    assert!(["tinyint", "int", "bigint"].contains(&n.ty.as_str()));
}
```

> The exact `ConnOptions` fields (`http_scheme`, `user`, `database`, …) and the self-skip
> helper must match this repo — copy them from the existing engine-gated test rather than
> trusting the sketch above. The assertions are the part that matters.

- [ ] **Step 4: Run tests + lint**

Run: `cargo test -p egret-engine describe && cargo clippy -p egret-engine --all-targets -- -D warnings`
Expected: tests PASS (or print `skip:` lines if engines are down — still exit 0); no clippy warnings. Also run `cargo test -p egret-engine` to confirm `FakeConnector` still compiles against the trait (it inherits the default `describe`).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine/src/connector/mod.rs crates/egret-engine/src/connector/starrocks.rs crates/egret-engine/tests/describe.rs
git commit -m "feat(engine): Connector::describe for typed columns of arbitrary SELECT"
```

---

## Task 3: Migration `0012_dataset.sql`

**Files:**
- Create: `migrations/0012_dataset.sql`

- [ ] **Step 1: Write the migration**

Create `migrations/0012_dataset.sql`. The RBAC seed mirrors `0009_engine.sql` exactly (`role_permissions(role_id, permission)` selecting `roles.id` by `scope='workspace'` + name):

```sql
CREATE TABLE datasets (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    catalog      text,
    schema_name  text,
    table_name   text,
    sql          text,
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT dataset_one_kind CHECK ((table_name IS NOT NULL) <> (sql IS NOT NULL)),
    CONSTRAINT dataset_name_uniq UNIQUE (workspace_id, name)
);

CREATE TABLE dataset_columns (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    column_name  text NOT NULL,
    data_type    text NOT NULL,
    type_generic text NOT NULL,
    is_temporal  boolean NOT NULL DEFAULT false,
    ordinal      int NOT NULL,
    UNIQUE (dataset_id, column_name)
);
CREATE INDEX dataset_columns_dataset_idx ON dataset_columns (dataset_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

> Before writing, open `migrations/0009_engine.sql` and confirm the `roles` columns
> (`scope`, `name`) and `role_permissions` columns (`role_id`, `permission`) match this
> seed verbatim. Fix the seed to match if the repo differs.

- [ ] **Step 2: Apply the migration locally**

Run the project's migration step (the same one the server runs on boot, or `sqlx migrate run` if the repo uses the sqlx CLI — check `crates/egret-metastore` / the bin for how migrations are applied). Expected: applies cleanly, no error.

If a running dev Postgres is available, verify:
Run: `psql "$DATABASE_URL" -c "\d datasets" -c "\d dataset_columns"`
Expected: both tables exist with the columns above.

- [ ] **Step 3: Commit**

```bash
git add migrations/0012_dataset.sql
git commit -m "feat(dataset): datasets + dataset_columns tables and RBAC seed"
```

---

## Task 4: `DatasetRepo` — dataset + column snapshot persistence

**Files:**
- Create: `crates/egret-bi/src/dataset.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Test: `crates/egret-bi/tests/dataset.rs` (new)

The repo is a **pure DB unit** — it receives already-classified columns (the route does engine introspection + `typegen`). Refresh is "replace columns within a transaction" (the observable end-state is the reconciled set; no per-column metadata exists yet to preserve in v1).

- [ ] **Step 1: Add the module + exports**

In `crates/egret-bi/src/lib.rs` add `mod dataset;` and to the `pub use` block:

```rust
pub use dataset::{Dataset, DatasetColumn, DatasetRepo, NewColumn, SourceSpec};
```

- [ ] **Step 2: Write `dataset.rs`**

```rust
//! Dataset repository: dataset rows + their synced column snapshot.

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct DatasetRepo {
    db: Db,
}

/// The source side of a dataset: exactly one kind.
#[derive(Debug, Clone)]
pub enum SourceSpec {
    Physical { catalog: Option<String>, schema: String, table: String },
    Virtual { sql: String },
}

/// A classified column ready to persist (route maps raw engine type → generic).
#[derive(Debug, Clone)]
pub struct NewColumn {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dataset {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub database_id: Uuid,
    pub name: String,
    pub description: String,
    pub catalog: Option<String>,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub sql: Option<String>,
    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DatasetColumn {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
}

const COLS: &str = "id, workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by, updated_at";

impl DatasetRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Create a dataset and its column snapshot in one transaction.
    pub async fn create(
        &self,
        ws: Uuid,
        database_id: Uuid,
        name: &str,
        description: &str,
        source: &SourceSpec,
        columns: &[NewColumn],
        user: Option<UserId>,
    ) -> Result<Dataset, BiError> {
        let (catalog, schema_name, table_name, sql) = match source {
            SourceSpec::Physical { catalog, schema, table } => {
                (catalog.clone(), Some(schema.clone()), Some(table.clone()), None)
            }
            SourceSpec::Virtual { sql } => (None, None, None, Some(sql.clone())),
        };

        let mut tx = self.db.pool.begin().await?;
        let ds = sqlx::query_as::<_, Dataset>(&format!(
            "INSERT INTO datasets (workspace_id, database_id, name, description, catalog, schema_name, table_name, sql, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(database_id)
        .bind(name)
        .bind(description)
        .bind(&catalog)
        .bind(&schema_name)
        .bind(&table_name)
        .bind(&sql)
        .bind(user.map(|u| u.0))
        .fetch_one(&mut *tx)
        .await?;

        insert_columns(&mut tx, ds.id, columns).await?;
        tx.commit().await?;
        Ok(ds)
    }

    pub async fn list(&self, ws: Uuid) -> Result<Vec<(Dataset, i64)>, BiError> {
        // dataset + its column count, newest first.
        let rows = sqlx::query_as::<_, (Dataset, i64)>(&format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dataset_columns c WHERE c.dataset_id = datasets.id) AS column_count \
             FROM datasets WHERE workspace_id = $1 ORDER BY updated_at DESC"
        ))
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Dataset>, BiError> {
        Ok(sqlx::query_as::<_, Dataset>(&format!("SELECT {COLS} FROM datasets WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    pub async fn columns(&self, dataset_id: Uuid) -> Result<Vec<DatasetColumn>, BiError> {
        Ok(sqlx::query_as::<_, DatasetColumn>(
            "SELECT column_name, data_type, type_generic, is_temporal, ordinal \
             FROM dataset_columns WHERE dataset_id = $1 ORDER BY ordinal",
        )
        .bind(dataset_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Update name/description, and (for virtual datasets) sql. Does not touch columns.
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        sql: Option<&str>,
    ) -> Result<Dataset, BiError> {
        let ds = sqlx::query_as::<_, Dataset>(&format!(
            "UPDATE datasets SET name=$2, description=$3, \
             sql = CASE WHEN sql IS NULL THEN sql ELSE COALESCE($4, sql) END, \
             updated_at = now() WHERE id=$1 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(sql)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| BiError::NotFound(id.to_string()))?;
        Ok(ds)
    }

    /// Replace the column snapshot in one transaction (used by refresh).
    pub async fn replace_columns(&self, id: Uuid, columns: &[NewColumn]) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM dataset_columns WHERE dataset_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        insert_columns(&mut tx, id, columns).await?;
        sqlx::query("UPDATE datasets SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM datasets WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(sqlx::query_scalar("SELECT created_by FROM datasets WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .flatten())
    }
}

async fn insert_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dataset_id: Uuid,
    columns: &[NewColumn],
) -> Result<(), BiError> {
    for (i, c) in columns.iter().enumerate() {
        sqlx::query(
            "INSERT INTO dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal) \
             VALUES ($1,$2,$3,$4,$5,$6)",
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

> `query_as::<_, (Dataset, i64)>` relies on sqlx deriving `FromRow` for the tuple: the
> first N columns map to `Dataset`, the trailing `column_count` to `i64`. If the sqlx
> version in this repo does not support tuple-of-(FromRow, scalar), fall back to a small
> `#[derive(FromRow)] struct DatasetWithCount { #[sqlx(flatten)] ds: Dataset, column_count: i64 }`
> — check `Cargo.toml` for the sqlx version and pick whichever compiles. Keep the public
> `list` return shape as `Vec<(Dataset, i64)>` either way (map at the end).

- [ ] **Step 3: Write DB-gated repo tests**

Mirror `crates/egret-bi/tests/saved.rs` (the existing DB-gated repo harness — it shows how this repo obtains a `Db` via `DATABASE_URL` + `Db::connect`/`migrate`, seeds a workspace, and self-skips with `eprintln!("skip")` when `DATABASE_URL` is unset). Read it first. Create `crates/egret-bi/tests/dataset.rs` exercising: create physical, create virtual, list-with-count, get + columns ordered by ordinal, update name/description, `replace_columns` produces the new set (add/remove/retype), delete cascades. Obtain a workspace id and a granted database id the same way `saved.rs` seeds its fixtures.

```rust
// Sketch — adapt the harness (Db construction, ws/db seeding, self-skip) to this repo's
// existing DB-gated test pattern. The assertions below are the contract.
#[tokio::test]
async fn create_list_get_update_refresh_delete() {
    let Some(ctx) = test_ctx().await else { eprintln!("skip: no DB"); return };
    let repo = egret_bi::DatasetRepo::new(ctx.db.clone());

    let cols = vec![
        egret_bi::NewColumn { column_name: "id".into(), data_type: "bigint".into(), type_generic: "numeric".into(), is_temporal: false },
        egret_bi::NewColumn { column_name: "ts".into(), data_type: "timestamp".into(), type_generic: "temporal".into(), is_temporal: true },
    ];
    let ds = repo.create(ctx.ws, ctx.db_id, "sales", "", 
        &egret_bi::SourceSpec::Physical { catalog: Some("tpch".into()), schema: "sf1".into(), table: "orders".into() },
        &cols, Some(ctx.user)).await.unwrap();

    let listed = repo.list(ctx.ws).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].1, 2); // column_count

    let got = repo.columns(ds.id).await.unwrap();
    assert_eq!(got.iter().map(|c| c.column_name.as_str()).collect::<Vec<_>>(), ["id", "ts"]);
    assert!(got[1].is_temporal);

    repo.update(ds.id, "sales2", "desc", None).await.unwrap();

    // refresh: retype id, drop ts, add region
    let cols2 = vec![
        egret_bi::NewColumn { column_name: "id".into(), data_type: "varchar".into(), type_generic: "string".into(), is_temporal: false },
        egret_bi::NewColumn { column_name: "region".into(), data_type: "varchar".into(), type_generic: "string".into(), is_temporal: false },
    ];
    repo.replace_columns(ds.id, &cols2).await.unwrap();
    let after = repo.columns(ds.id).await.unwrap();
    assert_eq!(after.iter().map(|c| c.column_name.as_str()).collect::<Vec<_>>(), ["id", "region"]);
    assert_eq!(after[0].type_generic, "string");

    repo.delete(ds.id).await.unwrap();
    assert!(repo.get(ds.id).await.unwrap().is_none());
    assert!(repo.columns(ds.id).await.unwrap().is_empty()); // cascade
}
```

- [ ] **Step 4: Run tests + lint**

Run: `cargo test -p egret-bi dataset && cargo clippy -p egret-bi --all-targets -- -D warnings`
Expected: PASS (or `skip:` line when no DB); no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bi/src/dataset.rs crates/egret-bi/src/lib.rs crates/egret-bi/tests/dataset.rs
git commit -m "feat(dataset): DatasetRepo CRUD + column snapshot persistence"
```

---

## Task 5: Dataset DTOs, `dataset_router`, and wiring

**Files:**
- Modify: `crates/egret-bi/src/model.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Modify: `crates/egret-server/src/lib.rs`
- Modify: `crates/egret-bin/src/main.rs`

Routes orchestrate: resolve+authorize the database (reuse `conn_spec`), introspect columns (physical → `SchemaService::columns`; virtual → `build_connector(spec).describe`), classify via `typegen`, then persist. Create is introspect-first (a bad source creates nothing). Refresh re-introspects and replaces columns, keeping the old snapshot on failure.

- [ ] **Step 1: Add DTOs to `model.rs`**

Append to `crates/egret-bi/src/model.rs`:

```rust
use crate::dataset::{Dataset, DatasetColumn};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CreateSource {
    Physical { catalog: Option<String>, schema: String, table: String },
    Virtual { sql: String },
}

#[derive(Debug, Deserialize)]
pub struct CreateDataset {
    pub name: String,
    pub database_id: Uuid,
    #[serde(default)]
    pub description: String,
    pub source: CreateSource,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDataset {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sql: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DatasetColumnDto {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
}

#[derive(Debug, Serialize)]
pub struct DatasetSummaryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub database_id: String,
    pub kind: &'static str,
    pub column_count: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct DatasetDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub database_id: String,
    pub kind: &'static str,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub sql: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub columns: Vec<DatasetColumnDto>,
}

pub fn dataset_kind(ds: &Dataset) -> &'static str {
    if ds.sql.is_some() { "virtual" } else { "physical" }
}

pub fn summary_dto(ds: Dataset, column_count: i64) -> DatasetSummaryDto {
    DatasetSummaryDto {
        kind: dataset_kind(&ds),
        id: ds.id.to_string(),
        name: ds.name,
        description: ds.description,
        database_id: ds.database_id.to_string(),
        column_count,
        updated_at: ds.updated_at,
    }
}

pub fn detail_dto(ds: Dataset, columns: Vec<DatasetColumn>) -> DatasetDetailDto {
    DatasetDetailDto {
        kind: dataset_kind(&ds),
        id: ds.id.to_string(),
        name: ds.name,
        description: ds.description,
        database_id: ds.database_id.to_string(),
        catalog: ds.catalog,
        schema: ds.schema_name,
        table: ds.table_name,
        sql: ds.sql,
        updated_at: ds.updated_at,
        columns: columns
            .into_iter()
            .map(|c| DatasetColumnDto {
                column_name: c.column_name,
                data_type: c.data_type,
                type_generic: c.type_generic,
                is_temporal: c.is_temporal,
                ordinal: c.ordinal,
            })
            .collect(),
    }
}
```

- [ ] **Step 2: Add `datasets: DatasetRepo` to `BiState` and the `dataset_router` + handlers in `routes.rs`**

In `crates/egret-bi/src/routes.rs`:

1. Add to the `BiState` struct: `pub datasets: crate::dataset::DatasetRepo,`.
2. Add constants near `const PERM`: `const DS_READ: &str = "dataset:read";` and `const DS_MANAGE: &str = "dataset:manage";`.
3. Add imports: `use crate::dataset::{NewColumn, SourceSpec};`, `use crate::model::{CreateDataset, CreateSource, DatasetDetailDto, DatasetSummaryDto, UpdateDataset};`, `use crate::typegen::generic_type;`, `use egret_engine::connector::build_connector;`, `use std::time::Duration;`.
4. Add the router and handlers:

```rust
pub fn dataset_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/datasets", post(ds_create).get(ds_list))
        .route(
            "/ws/{workspace}/datasets/{id}",
            get(ds_get).put(ds_update).delete(ds_remove),
        )
        .route("/ws/{workspace}/datasets/{id}/refresh", post(ds_refresh))
}

/// Introspect a source's columns and classify them. Physical → DESCRIBE; virtual → describe().
async fn introspect(
    st: &BiState,
    spec: &ConnSpec,
    source: &SourceSpec,
) -> Result<Vec<NewColumn>, BiApiError> {
    let raw: Vec<(String, String)> = match source {
        SourceSpec::Physical { catalog, schema, table } => st
            .schema
            .columns(spec, catalog.as_deref().unwrap_or(""), schema, table)
            .await
            .map_err(BiApiError::from)?
            .into_iter()
            .map(|c| (c.name, c.ty))
            .collect(),
        SourceSpec::Virtual { sql } => build_connector(spec)
            .describe(sql, Duration::from_secs(15))
            .await
            .map_err(|e| BiApiError::from(crate::BiError::from(e)))?
            .into_iter()
            .map(|c| (c.name, c.ty))
            .collect(),
    };
    Ok(raw
        .into_iter()
        .map(|(name, ty)| {
            let g = generic_type(spec.engine, &ty);
            NewColumn {
                column_name: name,
                data_type: ty,
                type_generic: g.as_str().to_string(),
                is_temporal: g.is_temporal(),
            }
        })
        .collect())
}

async fn ds_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    let spec = conn_spec_perm(&st, &ctx, b.database_id, DS_MANAGE).await?;
    let source = match b.source {
        CreateSource::Physical { catalog, schema, table } => SourceSpec::Physical { catalog, schema, table },
        CreateSource::Virtual { sql } => SourceSpec::Virtual { sql },
    };
    // introspect-first: a bad table/SQL returns the engine error and creates nothing.
    let columns = introspect(&st, &spec, &source).await?;
    let ds = st
        .datasets
        .create(ctx.workspace_id.0, b.database_id, &b.name, &b.description, &source, &columns, Some(ctx.principal.user_id))
        .await
        .map_err(BiApiError::from)?;
    let cols = st.datasets.columns(ds.id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DatasetSummaryDto>>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_READ).await.map_err(BiApiError::from)?;
    let rows = st.datasets.list(ctx.workspace_id.0).await.map_err(BiApiError::from)?;
    Ok(Json(rows.into_iter().map(|(ds, n)| crate::model::summary_dto(ds, n)).collect()))
}

async fn ds_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_READ).await.map_err(BiApiError::from)?;
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st.datasets.update(id, &b.name, &b.description, b.sql.as_deref()).await.map_err(BiApiError::from)?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_refresh(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
    let source = source_of(&ds);
    // re-introspect; on failure return the error and keep the existing snapshot.
    let columns = introspect(&st, &spec, &source).await?;
    st.datasets.replace_columns(id, &columns).await.map_err(BiApiError::from)?;
    let ds = st.datasets.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    st.datasets.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn source_of(ds: &crate::dataset::Dataset) -> SourceSpec {
    match &ds.sql {
        Some(sql) => SourceSpec::Virtual { sql: sql.clone() },
        None => SourceSpec::Physical {
            catalog: ds.catalog.clone(),
            schema: ds.schema_name.clone().unwrap_or_default(),
            table: ds.table_name.clone().unwrap_or_default(),
        },
    }
}

async fn require_dataset_owner(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let owner = st.datasets.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) {
        Ok(())
    } else {
        Err(Error::Forbidden.into())
    }
}
```

5. Generalize the existing `conn_spec` so the permission is a parameter (datasets use `dataset:*`, not `query:run`). Rename the existing private `conn_spec` body into `conn_spec_perm(st, ctx, db, perm)` and keep `conn_spec` as a thin wrapper passing `PERM` so the SQL-Lab handlers are unchanged:

```rust
async fn conn_spec(st: &BiState, ctx: &egret_core::WorkspaceContext, db: Uuid) -> Result<ConnSpec, BiApiError> {
    conn_spec_perm(st, ctx, db, PERM).await
}
async fn conn_spec_perm(st: &BiState, ctx: &egret_core::WorkspaceContext, db: Uuid, perm: &str) -> Result<ConnSpec, BiApiError> {
    // ...existing body of conn_spec, but call require_workspace(ctx, perm) instead of PERM...
}
```

- [ ] **Step 3: Export `dataset_router` + `DatasetRepo` from the crate**

In `crates/egret-bi/src/lib.rs`, update the routes export line to include the new router:

```rust
pub use routes::{dataset_router, sqllab_router, BiState};
```

(The `DatasetRepo` etc. exports were added in Task 4.)

- [ ] **Step 4: Fix the existing route-test harness (compile-breaker) + add a dataset auth smoke**

Adding the `datasets` field to `BiState` breaks the struct literal in `crates/egret-bi/tests/http.rs::build_state`. Add the field there:

```rust
        datasets: egret_bi::DatasetRepo::new(db.clone()),
```

(place it alongside `saved:`/`schema:`). Then add a dataset unauthenticated-smoke test mirroring the existing `saved_list_requires_auth`, using `dataset_router()`:

```rust
#[tokio::test]
async fn dataset_list_requires_auth() {
    let Some(st) = state().await else { eprintln!("skip"); return };
    let app = egret_bi::dataset_router().with_state(st);
    let resp = app
        .oneshot(Request::builder().uri("/ws/eng/datasets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

> Import `dataset_router` in the test's `use egret_bi::{...}` line. Deeper RBAC-denial and
> owner-gate behavior is enforced by the shared `Authorizer`/`require_owner` paths already
> exercised in `egret-workspace` — these route handlers reuse them verbatim, matching how
> the saved-query routes are tested (auth smoke only) in this repo.

- [ ] **Step 5: Mount the router (server) + construct the repo (bin)**

In `crates/egret-server/src/lib.rs`, change the bi group to merge both routers before `with_state`:

```rust
    if let Some(bi) = state.bi.clone() {
        let bi_routes = egret_bi::sqllab_router().merge(egret_bi::dataset_router());
        let group = Router::new()
            .nest("/api", bi_routes)
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(bi);
        router = router.merge(group);
    }
```

In `crates/egret-bin/src/main.rs`, add the field to the `BiState { … }` construction:

```rust
        datasets: egret_bi::DatasetRepo::new(db.clone()),
```

- [ ] **Step 6: Build + lint the whole workspace**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: builds clean, no clippy warnings. Fix any signature mismatches before moving on. `ConnOptions` derives `Default` with `Option` fields `user`/`http_scheme`/`database`, so `ConnOptions { http_scheme: Some(..), user: Some(..), ..Default::default() }` is valid.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/src/lib.rs crates/egret-bi/tests/http.rs crates/egret-server/src/lib.rs crates/egret-bin/src/main.rs
git commit -m "feat(dataset): dataset_router (CRUD + refresh) with engine introspection"
```

---

## Task 6: Frontend API hooks (`api/datasets.ts`)

**Files:**
- Create: `frontend/src/api/datasets.ts`

Mirror the shape and conventions of `frontend/src/api/sqllab.ts` (`apiJson`/`apiFetch`, query keys, react-query 5).

- [ ] **Step 1: Write the hooks**

```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type DatasetKind = "physical" | "virtual";
export interface DatasetSummary {
  id: string; name: string; description: string; database_id: string;
  kind: DatasetKind; column_count: number; updated_at: string;
}
export interface DatasetColumn {
  column_name: string; data_type: string; type_generic: string; is_temporal: boolean; ordinal: number;
}
export interface DatasetDetail {
  id: string; name: string; description: string; database_id: string; kind: DatasetKind;
  catalog: string | null; schema: string | null; table: string | null; sql: string | null;
  updated_at: string; columns: DatasetColumn[];
}
export type CreateSource =
  | { kind: "physical"; catalog?: string | null; schema: string; table: string }
  | { kind: "virtual"; sql: string };
export interface CreateDatasetBody {
  name: string; database_id: string; description?: string; source: CreateSource;
}

export const dsKeys = {
  list: (ws: string) => ["datasets", ws, "list"] as const,
  detail: (ws: string, id: string) => ["datasets", ws, "detail", id] as const,
};

export function useDatasets(ws: string) {
  return useQuery<DatasetSummary[]>({
    queryKey: dsKeys.list(ws),
    queryFn: () => apiJson<DatasetSummary[]>(`/ws/${ws}/datasets`),
  });
}
export function useDataset(ws: string, id: string | null) {
  return useQuery<DatasetDetail>({
    queryKey: id ? dsKeys.detail(ws, id) : ["datasets", ws, "detail", "none"],
    queryFn: () => apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}`),
    enabled: !!id,
  });
}
export function useCreateDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateDatasetBody) =>
      apiJson<DatasetDetail>(`/ws/${ws}/datasets`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: dsKeys.list(ws) }),
  });
}
export function useUpdateDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; description?: string; sql?: string | null }) =>
      apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => { qc.invalidateQueries({ queryKey: dsKeys.detail(ws, id) }); qc.invalidateQueries({ queryKey: dsKeys.list(ws) }); },
  });
}
export function useRefreshDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}/refresh`, { method: "POST" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: dsKeys.detail(ws, id) }),
  });
}
export function useDeleteDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/datasets/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: dsKeys.list(ws) }),
  });
}
```

> Confirm `apiJson`/`apiFetch` signatures and the `useDatabases` import path against
> `frontend/src/api/sqllab.ts` (Task uses the same `./client`). `useDatasets` reuses the
> existing `useDatabases(ws)` from `sqllab.ts` in the UI — no need to redefine it.

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npx tsc --noEmit`
Expected: no type errors in `datasets.ts`.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/api/datasets.ts
git commit -m "feat(dataset): frontend react-query hooks"
```

---

## Task 7: Datasets list page + Create dialog

**Files:**
- Rewrite: `frontend/src/routes/explore/Datasets.tsx`
- Create: `frontend/src/components/dataset/CreateDatasetDialog.tsx`
- Create: `frontend/src/components/dataset/__tests__/CreateDatasetDialog.test.tsx`

Use the existing shadcn primitives already in the repo (`@/components/ui/*` — Button, Select, Dialog, Tabs, Input/Textarea). Match how `SqlTab.tsx` / `SaveQueryDialog.tsx` import and use them. Read `frontend/src/routes/explore/SqlLab.tsx` for how the current workspace id (`ws`) is obtained, and reuse the cascading catalog→schema→table select logic from `DataPanel.tsx`.

- [ ] **Step 1: Write the Create dialog**

`frontend/src/components/dataset/CreateDatasetDialog.tsx` — a Dialog with two tabs:
- *From table*: database Select (from `useDatabases`), then cascading catalog (Trino only) → schema → table (reuse `useCatalogs`/`useSchemas`/`useTables` from `@/api/sqllab`), plus name + description inputs.
- *From SQL*: database Select + a SQL `<textarea>` (or Monaco `SqlEditor` if trivially reusable) + name + description. Accepts optional `initialSql` / `initialDbId` props for the SQL-Lab bridge (Task 9).

On submit, call `useCreateDataset(ws)` with the tab-appropriate `source`. Show a `sonner` toast on error (engine errors surface here when the table/SQL is bad). Close + navigate to the new dataset's detail on success.

```tsx
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useDatabases, useCatalogs, useSchemas, useTables } from "@/api/sqllab";
import { useCreateDataset, type CreateSource } from "@/api/datasets";

export function CreateDatasetDialog({ ws, open, onOpenChange, initialSql, initialDbId, initialTab }: {
  ws: string; open: boolean; onOpenChange: (o: boolean) => void;
  initialSql?: string; initialDbId?: string | null; initialTab?: "table" | "sql";
}) {
  const nav = useNavigate();
  const { data: databases } = useDatabases(ws);
  const create = useCreateDataset(ws);
  const [dbId, setDbId] = useState(initialDbId ?? "");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  // from-table state
  const engine = databases?.find((d) => d.id === dbId)?.engine ?? null;
  const isTrino = engine === "trino";
  const [catalog, setCatalog] = useState("");
  const [schema, setSchema] = useState("");
  const [table, setTable] = useState("");
  const cats = useCatalogs(ws, isTrino ? (dbId || null) : null);
  const schemas = useSchemas(ws, dbId || null, isTrino ? catalog : "");
  const tables = useTables(ws, dbId || null, catalog, schema);
  // from-sql state
  const [sql, setSql] = useState(initialSql ?? "");

  async function submit(source: CreateSource) {
    if (!dbId) { toast.error("Pick a database"); return; }
    if (!name.trim()) { toast.error("Name required"); return; }
    try {
      const ds = await create.mutateAsync({ name, database_id: dbId, description, source });
      toast.success("Dataset created");
      onOpenChange(false);
      // absolute path: this dialog opens from both /explore/:ws/datasets and /explore/:ws/sql-lab
      nav(`/explore/${ws}/datasets/${ds.id}`);
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>New dataset</DialogTitle></DialogHeader>
        <Tabs defaultValue={initialTab ?? "table"}>
          <TabsList><TabsTrigger value="table">From table</TabsTrigger><TabsTrigger value="sql">From SQL</TabsTrigger></TabsList>

          <div className="space-y-2 py-2">
            <Select value={dbId} onValueChange={(v) => { setDbId(v); setCatalog(""); setSchema(""); setTable(""); }}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Database" /></SelectTrigger>
              <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
            </Select>
            <Input placeholder="Dataset name" value={name} onChange={(e) => setName(e.target.value)} />
            <Input placeholder="Description (optional)" value={description} onChange={(e) => setDescription(e.target.value)} />
          </div>

          <TabsContent value="table" className="space-y-2">
            {isTrino && (
              <Select value={catalog} onValueChange={(v) => { setCatalog(v); setSchema(""); setTable(""); }}>
                <SelectTrigger className="w-full"><SelectValue placeholder="Catalog" /></SelectTrigger>
                <SelectContent>{(cats.data ?? []).map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}</SelectContent>
              </Select>
            )}
            <Select value={schema} onValueChange={(v) => { setSchema(v); setTable(""); }} disabled={isTrino && !catalog}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Schema" /></SelectTrigger>
              <SelectContent>{(schemas.data ?? []).map((s) => <SelectItem key={s} value={s}>{s}</SelectItem>)}</SelectContent>
            </Select>
            <Select value={table} onValueChange={setTable} disabled={!schema}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Table" /></SelectTrigger>
              <SelectContent>{(tables.data ?? []).map((t) => <SelectItem key={t} value={t}>{t}</SelectItem>)}</SelectContent>
            </Select>
            <Button disabled={!table || create.isPending}
              onClick={() => submit({ kind: "physical", catalog: isTrino ? catalog : null, schema, table })}>Create</Button>
          </TabsContent>

          <TabsContent value="sql" className="space-y-2">
            <textarea className="h-40 w-full rounded border p-2 font-mono text-sm" placeholder="SELECT …"
              value={sql} onChange={(e) => setSql(e.target.value)} />
            <Button disabled={!sql.trim() || create.isPending}
              onClick={() => submit({ kind: "virtual", sql })}>Create</Button>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
```

> These primitives all exist in `frontend/src/components/ui/`: `button`, `input`,
> `dialog`, `tabs`, `select`, `textarea`. `SaveQueryDialog.tsx` is a working reference for
> the Dialog usage. You may swap the raw `<textarea>` for `@/components/ui/textarea`
> (`Textarea`) for consistency. Don't invent components.

- [ ] **Step 2: Rewrite the list page**

`frontend/src/routes/explore/Datasets.tsx`:

```tsx
import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { useDatasets } from "@/api/datasets";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";

export default function Datasets() {
  const ws = useParams().workspace!; // same source as SqlLab.tsx
  const { data, isLoading } = useDatasets(ws);
  const [open, setOpen] = useState(false);

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Datasets</h1>
        <Button onClick={() => setOpen(true)}>Create</Button>
      </div>
      {isLoading ? <p className="text-muted-foreground">Loading…</p>
        : (data && data.length > 0) ? (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr><th className="py-1">Name</th><th>Kind</th><th>Columns</th><th>Updated</th></tr>
            </thead>
            <tbody>
              {data.map((d) => (
                <tr key={d.id} className="border-t hover:bg-accent">
                  <td className="py-1"><Link className="font-medium hover:underline" to={`${d.id}`}>{d.name}</Link></td>
                  <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{d.kind}</span></td>
                  <td>{d.column_count}</td>
                  <td className="text-muted-foreground">{new Date(d.updated_at).toLocaleString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No datasets yet. A dataset is a named, reusable source for charts.</p>
            <Button className="mt-3" onClick={() => setOpen(true)}>Create your first dataset</Button>
          </div>
        )}
      <CreateDatasetDialog ws={ws} open={open} onOpenChange={setOpen} />
    </div>
  );
}
```

> `ws` comes from `useParams().workspace` — identical to `SqlLab.tsx:10`. The `datasets`
> route renders under the `/explore/:workspace/*` parent (see `ExploreRoutes` in
> `App.tsx`), so the param is in scope. The `<Link to={d.id}>` is relative → resolves to
> `datasets/:id`.

- [ ] **Step 3: Write the vitest**

`frontend/src/components/dataset/__tests__/CreateDatasetDialog.test.tsx` — render the dialog inside a `QueryClientProvider` + `MemoryRouter` (copy the test harness from an existing test under `frontend/src/components/sqllab/__tests__/`), mock `useDatabases` to return one DB, assert both tab triggers render and the name input is present. Assert the "Create" button under "From SQL" is disabled when SQL is empty.

- [ ] **Step 4: Run frontend tests + build**

Run: `cd frontend && npm test -- --run && npm run build`
Expected: tests pass, build succeeds.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/Datasets.tsx frontend/src/components/dataset/
git commit -m "feat(dataset): list page + create dialog (from-table / from-SQL)"
```

---

## Task 8: Dataset detail page (columns, refresh, edit, delete)

**Files:**
- Create: `frontend/src/routes/explore/DatasetDetail.tsx`
- Modify: `frontend/src/App.tsx`

- [ ] **Step 1: Register the nested route**

In `frontend/src/App.tsx`, inside the `ExploreRoutes()` function's inner `<Routes>` block (the one containing `<Route path="datasets" element={<Datasets />} />` at ~line 47), add a sibling route, and add the eager import beside the existing `import Datasets from "@/routes/explore/Datasets";`:

```tsx
import DatasetDetail from "@/routes/explore/DatasetDetail";
// ...inside ExploreRoutes' <Routes>:
<Route path="datasets" element={<Datasets />} />
<Route path="datasets/:id" element={<DatasetDetail />} />
```

> The file uses eager imports (`import Datasets from ...`) — follow that, not lazy loading.
> Both routes live in `ExploreRoutes`, which is mounted under `/explore/:workspace/*`, so
> `useParams()` inside `DatasetDetail` yields both `workspace` and `id`.

- [ ] **Step 2: Write the detail page**

`frontend/src/routes/explore/DatasetDetail.tsx`:

```tsx
import { useParams, useNavigate, Link } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { useDataset, useRefreshDataset, useDeleteDataset } from "@/api/datasets";

export default function DatasetDetail() {
  const { workspace: ws = "", id = "" } = useParams();
  const nav = useNavigate();
  const { data: ds, isLoading } = useDataset(ws, id || null);
  const refresh = useRefreshDataset(ws, id);
  const del = useDeleteDataset(ws);

  if (isLoading) return <div className="p-4 text-muted-foreground">Loading…</div>;
  if (!ds) return <div className="p-4">Not found. <Link className="underline" to="..">Back</Link></div>;

  const source = ds.kind === "virtual" ? ds.sql : [ds.catalog, ds.schema, ds.table].filter(Boolean).join(".");

  return (
    <div className="p-4">
      <div className="mb-4 flex items-start justify-between">
        <div>
          <Link className="text-sm text-muted-foreground hover:underline" to="..">← Datasets</Link>
          <h1 className="text-lg font-semibold">{ds.name} <span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-xs">{ds.kind}</span></h1>
          {ds.description && <p className="text-sm text-muted-foreground">{ds.description}</p>}
          <pre className="mt-1 max-w-2xl overflow-auto rounded bg-muted p-2 text-xs">{source}</pre>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" disabled={refresh.isPending}
            onClick={() => refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))}>
            {refresh.isPending ? "Refreshing…" : "Refresh"}
          </Button>
          <Button variant="destructive"
            onClick={() => { if (confirm("Delete this dataset?")) del.mutate(id, { onSuccess: () => { toast.success("Deleted"); nav(".."); } }); }}>
            Delete
          </Button>
        </div>
      </div>

      <table className="w-full text-sm">
        <thead className="text-left text-muted-foreground">
          <tr><th className="py-1">Column</th><th>Type</th><th>Class</th><th>Temporal</th></tr>
        </thead>
        <tbody>
          {ds.columns.map((c) => (
            <tr key={c.column_name} className="border-t">
              <td className="py-1 font-mono">{c.column_name}</td>
              <td className="font-mono text-muted-foreground">{c.data_type}</td>
              <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{c.type_generic}</span></td>
              <td>{c.is_temporal ? "✓" : ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
```

> Edit (rename/description, and SQL for virtual) can reuse a small inline form or be
> deferred to a follow-up — the spec's required mutations are Refresh + Delete + the
> create flow. If adding Edit now, wire `useUpdateDataset(ws, id)`; otherwise omit the Edit
> button (do not leave a dead button).

- [ ] **Step 3: Typecheck + build**

Run: `cd frontend && npx tsc --noEmit && npm run build`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/DatasetDetail.tsx frontend/src/App.tsx
git commit -m "feat(dataset): detail page with column snapshot + refresh + delete"
```

---

## Task 9: SQL Lab → "Save as dataset" bridge

**Files:**
- Modify: `frontend/src/components/sqllab/SqlTab.tsx`
- Create/Modify: `frontend/src/components/sqllab/__tests__/` (add a small assertion)

- [ ] **Step 1: Add the toolbar button + dialog**

In `frontend/src/components/sqllab/SqlTab.tsx`, import the create dialog and add local state:

```tsx
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
// ...
const [datasetOpen, setDatasetOpen] = useState(false);
```

Add a button beside the existing "Save" button in the toolbar:

```tsx
<Button size="sm" variant="outline" onClick={() => setDatasetOpen(true)}>Save as dataset</Button>
```

And render the dialog (next to where `saveDialog` is rendered), prefilled from the tab:

```tsx
<CreateDatasetDialog
  ws={ws}
  open={datasetOpen}
  onOpenChange={setDatasetOpen}
  initialSql={tab.sql}
  initialDbId={tab.dbId}
  initialTab="sql"
/>
```

Place it in both the collapsed and expanded returns (wherever `saveDialog` already appears) so it is always mounted.

- [ ] **Step 2: Update/extend the SQL Lab test**

In the existing `frontend/src/components/sqllab/__tests__/` suite, add (or extend a test) to assert the "Save as dataset" button renders in the toolbar. Keep the existing mocks; if `CreateDatasetDialog` pulls `useNavigate`, ensure the test renders within a `MemoryRouter` (it likely already does).

- [ ] **Step 3: Run frontend tests + build**

Run: `cd frontend && npm test -- --run && npm run build`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/sqllab/SqlTab.tsx frontend/src/components/sqllab/__tests__/
git commit -m "feat(dataset): Save as dataset bridge from SQL Lab"
```

---

## Final verification (after all tasks)

- [ ] **Whole-workspace tests + lint**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green (engine-/DB-gated tests self-skip when infra is absent).

> If `cargo test --workspace` fails to **link** with `ld terminated with signal 7 [Bus
> error]`, that is the known infra link-OOM, not a code defect — see the project memory
> `egret-ci-link-oom`. The durable fix (`debug = "line-tables-only"` in `[profile.dev]`)
> may be applied if it recurs.

- [ ] **Frontend tests + build**

Run: `cd frontend && npm test -- --run && npm run build`
Expected: all green.

- [ ] **Manual smoke (optional, if the dev stack is up)**

Create a physical dataset from a `tpch` table and a virtual dataset from `SELECT 1 AS n, now() AS t`; confirm columns + generic types appear, Refresh works, and "Save as dataset" from SQL Lab prefills the SQL.

---

## Notes for the executor

- The `egret-bi` crate previously declared in its doc comment "Dataset/chart/dashboard come later" — update that `lib.rs` comment to reflect datasets now exist.
- Engine introspection in routes uses a fixed 15s timeout (matches `SchemaService::limits`); do not add a config knob in this phase.
- Do not return or log DB secrets anywhere in the new code (the `ConnSpec` carries an opened secret — never serialize it).
- Keep the per-engine type vocabulary consistent: physical uses `DESCRIBE` names, virtual uses `describe()`'s canonical names; `typegen` must classify both.
