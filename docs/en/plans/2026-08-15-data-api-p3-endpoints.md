# Data API P3 — Endpoint definitions, versioning, management API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> Spec: `docs/en/specs/2026-08-14-data-api-design.md` §4.1–4.4, §5.1–5.3
> Part: P3 of 5 — see `docs/en/plans/2026-08-14-data-api-decomposition.md`
> Stacked on: P1 (#504) and P2 (#505), both **merged to `main`**. Branch off `main`.

**Goal:** a workspace member can author a parameterized SELECT, validate it, publish it as an immutable version, and roll back — with no serving surface yet.

**Architecture:** `dataapi.endpoints` holds the freely-editable draft; `dataapi.endpoint_versions` holds immutable published snapshots. Publishing freezes the whole definition, including the output schema resolved by `describe`. `egret-dataapi` reaches the query engine through a new `QueryExecPort` in `egret-query-api` rather than depending on `egret-query`, so the seam can move to its own binary later.

**Tech Stack:** Rust (axum, sqlx) · Postgres · Cedar PDP · Trino / StarRocks connectors.

## Global Constraints

- **The serving path will read only `endpoint_versions.snapshot`, never `dataapi.endpoints`.** P3 has no serving path, but it must build the store so that P4 cannot accidentally read the draft: the snapshot must be self-contained (SQL, params, output schema, database_id, cache and limits config), and there must be a test proving a published version is unaffected by later draft edits. If serving read the draft, versioning would be decoration.
- **Validation happens before publish, never at serve time.** `validate` rejects: more than one statement; a statement that is not `SELECT` (a leading `WITH … SELECT` is allowed); a `{{param}}` with no declared parameter; a declared parameter never referenced; a `cache.scope = "shared"` on a delegated connection; a method other than `GET` or `POST`.
- **Parameters are values, never identifiers.** A dynamic column is an `enum` parameter with an explicit `allowed` list. Do not add identifier interpolation.
- `egret-dataapi` must NOT gain a dependency on `egret-query`. It depends on the leaf `egret-query-api`, which carries the port trait. Adding the seam crate would defeat the split-out this port exists for.
- Follow P2's authz idiom exactly: `WsContext(ctx)` extractor + `egret_core::authz::require(&*st.pdp, &ctx, ACTION)`, workspace from `ctx.workspace_id` and **never** from a path parameter, 404 (not 403) for cross-workspace access. See `crates/egret-dataapi/src/routes.rs` as it stands after P2.
- Run scoped checks, not `just check`. **Set `DATABASE_URL` to a THROWAWAY database** — unset, DB-gated suites report `ok` in 0.00s having asserted nothing, and the dev DB `egret` is polluted to the point of not booting. Check durations are non-zero.
- Never run two workspace test suites concurrently.

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-query-api/src/port.rs` (create) | The `QueryExecPort` trait — the only thing `egret-dataapi` knows about query execution. |
| `crates/egret-query-api/src/lib.rs` (modify) | Declare the module; correct the now-false "no port trait" module doc. |
| `crates/egret-query/src/execution/port_impl.rs` (create) | `QueryExecPort` over `EngineState`, reusing the delegation logic `routes.rs` already has. |
| `crates/egret-query/src/execution/routes.rs` (modify) | Extract the delegation block (lines 118–156) into a shared fn so the port and the route cannot drift. |
| `migrations/0121_dataapi_endpoints.sql` (create) | `dataapi.endpoints` + `dataapi.endpoint_versions`. |
| `crates/egret-dataapi-api/src/lib.rs` (modify) | `ParamSpec`, `OutputSpec`, `CacheSpec`, `LimitsSpec`, `RateLimitSpec`, `EndpointSnapshot`. |
| `crates/egret-dataapi/src/endpoints.rs` (create) | Draft store: CRUD, scoped by workspace. |
| `crates/egret-dataapi/src/lifecycle.rs` (create) | `validate`, `publish`, `rollback`, `disable`. |
| `crates/egret-dataapi/src/routes.rs` (modify) | Endpoint routes; audit sink on `DataApiState`. |
| `crates/egret-core/src/rbac.rs` (modify) | Two permissions: `data_api_endpoint:read` / `:manage`. |
| `migrations/0122_dataapi_endpoint_permissions.sql` (create) | Seed them onto `ws_admin`, mirroring 0120. |

---

### Task 1: `QueryExecPort` and its implementation

**Files:**
- Create: `crates/egret-query-api/src/port.rs`, `crates/egret-query/src/execution/port_impl.rs`
- Modify: `crates/egret-query-api/src/lib.rs`, `crates/egret-query/src/execution/routes.rs`, `crates/egret-query/src/execution/mod.rs`, `crates/egret-dataapi/Cargo.toml`
- Test: `crates/egret-query/tests/port_describe.rs` (create)

**Interfaces:**
- Consumes: `Connector::describe` (`crates/egret-query/src/execution/connector/mod.rs:81`), `ConnSpec`, `Database`, `egret_core::{ColumnMeta, UserId}`.
- Produces: `pub trait egret_query_api::port::QueryExecPort` with `describe_as`, re-exported as `egret_query_api::QueryExecPort`.

**Scope note:** the spec's §4.3 port block also lists `execute_as` and `render`. This task adds **only `describe_as`**, because it is the only method P3 has a consumer for. `execute_as` lands in P4 alongside the serving path that calls it — a trait method with no caller is untested surface. Say so in the port's module doc so the next reader does not think it was forgotten.

- [ ] **Step 1: Write the port trait**

Create `crates/egret-query-api/src/port.rs`:

```rust
//! `QueryExecPort` — the only thing the Data API seam knows about query
//! execution.
//!
//! `egret-dataapi` holds an `Arc<dyn QueryExecPort>` and never depends on
//! `egret-query`, so the Data API can move to its own binary by swapping the
//! implementation. This mirrors `NotebookDocsPort` in `egret-analytics-api`.
//!
//! Deliberately minimal: it carries `describe_as` and nothing else, because
//! that is all P3 has a caller for. `execute_as` arrives in P4 with the serving
//! path that calls it — a trait method with no consumer is untested surface.

use uuid::Uuid;

use crate::ColumnMeta;

#[async_trait::async_trait]
pub trait QueryExecPort: Send + Sync {
    /// Resolve the output column schema of `sql` without fetching rows, running
    /// as `principal` so a delegated connection resolves under that identity's
    /// Ranger policy rather than the service account's.
    ///
    /// Used by validate-before-publish to freeze the output shape into the
    /// snapshot, so the published contract cannot drift when the underlying
    /// table changes.
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error>;
}
```

Add `async-trait` to `crates/egret-query-api/Cargo.toml` if absent (check first — the crate is a pure leaf and this is its first async trait).

- [ ] **Step 2: Declare it and correct the module doc**

In `crates/egret-query-api/src/lib.rs`, add near the other declarations:

```rust
pub mod port;
pub use port::QueryExecPort;
```

The module doc (around lines 10–12) currently ends with a claim that is now false. It reads, in part, "no `QueryPort` trait (the seam's runtime surface is the concrete `Connector` data-plane)". Replace that clause with:

```
//! Pure-serde DTOs plus two behavioural exceptions, both deliberate: the
//! dialect-escaping primitive both query consumers must share (`dialect`), and
//! `QueryExecPort` — the seam the Data API reaches execution through, so it
//! never depends on `egret-query` itself.
```

Leave the rest of the doc alone. A doc that contradicts the code is what this correction exists to prevent, so do not skip it.

- [ ] **Step 3: Extract the delegation block**

`crates/egret-query/src/execution/routes.rs:118-156` resolves a `Database` and derives `(session_user, bearer)` for delegation. The port needs the identical logic; two copies would drift, and this one decides *whose* Ranger policy applies.

Extract it into a function in `crates/egret-query/src/execution/mod.rs` (or a new `delegation.rs` if that file is already large — check before adding):

```rust
/// Resolve the per-user delegation credentials for `database_id`.
///
/// Returns `(database, session_user, bearer_token)`. For a delegated connection
/// in impersonate mode this is the caller's username and no JWT; in jwt mode it
/// is their username plus a freshly minted engine token; for a non-delegated
/// connection it is the caller's email and no token. A non-OIDC caller on a
/// delegated jwt connection is refused rather than silently falling back to
/// service credentials.
pub(crate) async fn delegation_for(
    st: &EngineState,
    principal: egret_core::UserId,
    principal_email: &str,
    database_id: Uuid,
) -> Result<(Database, Option<String>, Option<String>), EngineError>
```

Move the body verbatim from `routes.rs`, replacing `ctx.principal.user_id` with `principal` and `ctx.principal.email` with `principal_email`. Then call it from `submit` so the route and the port share one path. **Do not change its behaviour** — this is a pure extraction; the existing `egret-query` suites are the regression net.

- [ ] **Step 4: Write the failing test**

Create `crates/egret-query/tests/port_describe.rs`. Follow the self-skip convention of `crates/egret-query/tests/connector_trino.rs` (`EGRET_TEST_TRINO_URL`).

```rust
//! `describe_as` must resolve a real column schema through the port, not a stub.

#[tokio::test]
async fn describe_as_returns_the_select_list() {
    let Some(st) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn egret_query_api::QueryExecPort = &st;

    let cols = port
        .describe_as(st.ws, st.database_id, st.user, "SELECT 1 AS n, 'x' AS s")
        .await
        .expect("describe must succeed");

    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["n", "s"], "column names come from the SELECT list");
    assert_eq!(cols.len(), 2, "no extra columns");
}

#[tokio::test]
async fn describe_as_rejects_invalid_sql() {
    let Some(st) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn egret_query_api::QueryExecPort = &st;
    assert!(
        port.describe_as(st.ws, st.database_id, st.user, "SELECT FROM WHERE")
            .await
            .is_err(),
        "a syntax error must surface as an error, not an empty column list"
    );
}
```

Run: `cargo test -p egret-query --test port_describe`

Expected: compile error — `QueryExecPort` is not implemented for the state type. That is the red state.

- [ ] **Step 5: Implement the port**

Create `crates/egret-query/src/execution/port_impl.rs`:

```rust
//! `QueryExecPort` over the concrete `EngineState`. The Data API seam holds this
//! as `Arc<dyn QueryExecPort>` and never names `egret-query`.

use std::time::Duration;
use uuid::Uuid;

use egret_core::{ColumnMeta, Error, UserId};

use crate::execution::connector::{build_connector, ConnSpec};
use crate::execution::delegation_for;
use crate::execution::EngineState;

/// `describe` is called directly, with no JobManager wrapping it, so it carries
/// its own timeout — the same reason `Connector::describe` does.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for EngineState {
    async fn describe_as(
        &self,
        _ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        let email = self
            .users
            .find_by_id(principal)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("resolve principal: {e}")))?
            .ok_or_else(|| Error::Internal(anyhow::anyhow!("principal not found")))?
            .email;
        let (db, session_user, bearer) = delegation_for(self, principal, &email, database_id)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("delegation: {e}")))?;
        let secret = self
            .databases
            .open_secret(database_id)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("open secret: {e}")))?;
        let mut spec = ConnSpec::from_database(&db, secret);
        spec.session_user = session_user;
        spec.bearer_token = bearer;
        build_connector(&spec)
            .describe(sql, DESCRIBE_TIMEOUT)
            .await
            .map_err(|e| Error::InvalidInput(e.to_string()))
    }
}
```

Accessor names verified against the tree, not guessed: `EngineState` (`crates/egret-query/src/execution/routes.rs:30-44`) carries `databases: DatabaseRepo`, `users: UserRepo` and `engine_minter`; the secret comes from `databases.open_secret(database_id)` (the shape `crates/egret-query/src/execution/job.rs:216` uses); the delegated username comes from `users.ensure_username(principal)`, which `delegation_for` already calls internally.

If `ConnSpec::from_database_as` (`connector/mod.rs:43`) already accepts the session user, prefer it over mutating the two fields — check its signature first.

Note the error mapping: a bad SQL string becomes `InvalidInput`, not `Internal`, because validate surfaces it to the author as a 400. `Error::Internal` would render as a 500 and tell them nothing.

Declare the module in `crates/egret-query/src/execution/mod.rs`:

```rust
mod port_impl;
```

- [ ] **Step 6: Add the dependency and run the tests**

Add `egret-query-api.workspace = true` to `crates/egret-dataapi/Cargo.toml` (it will be used from Task 4 onward; adding it here keeps the seam declaration with the seam).

Run with a live Trino:
```bash
EGRET_TEST_TRINO_URL=http://localhost:8090 \
  cargo test -p egret-query --test port_describe -- --nocapture
```

Expected: PASS with no "skip" in the output. A self-skipped test is not a passing test.

- [ ] **Step 7: Verify the extraction did not change behaviour**

```bash
cargo test -p egret-query
```

Expected: the pre-existing suites stay green. They are the regression net for Step 3's extraction — if any of them changes behaviour, the extraction was not pure.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-query-api crates/egret-query crates/egret-dataapi/Cargo.toml
git commit -m "feat(query): QueryExecPort so the Data API seam can describe SQL

egret-dataapi reaches execution through a leaf-crate trait rather than
depending on egret-query, so the seam can move to its own binary later.

Only describe_as: it is the only method P3 has a caller for. execute_as
lands in P4 with the serving path that calls it.

The delegation block routes.rs used is extracted so the route and the port
cannot drift — that logic decides whose Ranger policy applies."
```

---

### Task 2: Schema and the config DTOs

**Files:**
- Create: `migrations/0121_dataapi_endpoints.sql`
- Modify: `crates/egret-dataapi-api/src/lib.rs`
- Test: `crates/egret-dataapi/tests/endpoints_schema.rs` (create)

**Interfaces:**
- Produces: `ParamSpec`, `ParamType`, `OutputSpec`, `CacheSpec`, `CacheScope`, `LimitsSpec`, `RateLimitSpec`, `EndpointSnapshot` — all pure-serde in `egret-dataapi-api`.

- [ ] **Step 1: Write the migration**

Create `migrations/0121_dataapi_endpoints.sql`, in the house style (explain *why*):

```sql
-- Data API P3: endpoint definitions and their immutable published versions.
--
-- TWO tables, and the split is the whole feature. `endpoints` is the draft an
-- author edits freely. `endpoint_versions` holds frozen snapshots, and the
-- serving path (P4) reads ONLY those. If serving read the draft, editing a
-- query would change behaviour under every client already calling it, and
-- "versioning" would be decoration.
CREATE TABLE dataapi.endpoints (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- Cross-schema FK, as for dataapi.service_accounts -> users: `folders` is
  -- already a shared table serving chart/dashboard/dataset, and duplicating it
  -- per schema would be worse than the ownership bend.
  folder_id     uuid REFERENCES folders(id) ON DELETE SET NULL,
  name          text NOT NULL,
  description   text NOT NULL DEFAULT '',
  -- The URL path is FREE-FORM and user-typed. It is deliberately NOT derived
  -- from folder_id: folders are renamed and re-parented freely, and a URL is a
  -- published contract with deployed clients.
  path          text NOT NULL,
  method        text NOT NULL DEFAULT 'GET' CHECK (method IN ('GET','POST')),
  database_id   uuid NOT NULL,
  sql           text NOT NULL,
  params        jsonb NOT NULL DEFAULT '[]',
  output        jsonb NOT NULL DEFAULT '{}',
  cache         jsonb NOT NULL DEFAULT '{}',
  limits        jsonb NOT NULL DEFAULT '{}',
  rate_limit    jsonb NOT NULL DEFAULT '{}',
  source_kind   text NOT NULL DEFAULT 'raw' CHECK (source_kind IN ('raw','saved_query','dataset')),
  source_ref    uuid,
  live_version_id uuid,
  state         text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft','live','disabled')),
  created_by    uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, method, path)
);
CREATE INDEX dataapi_endpoints_ws_idx ON dataapi.endpoints (workspace_id);

CREATE TABLE dataapi.endpoint_versions (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  endpoint_id uuid NOT NULL REFERENCES dataapi.endpoints(id) ON DELETE CASCADE,
  version     integer NOT NULL,
  -- The FULL definition, self-contained: sql, params, resolved output schema,
  -- database_id, cache and limits. Serving must never need to consult the
  -- draft row to answer a request.
  snapshot    jsonb NOT NULL,
  note        text NOT NULL DEFAULT '',
  created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (endpoint_id, version)
);

-- Added after both tables exist, so the circular reference resolves.
ALTER TABLE dataapi.endpoints
  ADD CONSTRAINT endpoints_live_version_fk
  FOREIGN KEY (live_version_id) REFERENCES dataapi.endpoint_versions(id) ON DELETE SET NULL;
```

Note the deliberate absence of an explicit index on any `UNIQUE` column — `UNIQUE` already creates one. (P2's 0119 shipped a redundant one and it had to be removed.)

- [ ] **Step 2: Write the DTOs**

In `crates/egret-dataapi-api/src/lib.rs`, add (pure serde — no sqlx, no axum):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamType { String, Number, Boolean, Date, Timestamp, Enum }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation { Query, Body }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub location: ParamLocation,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Required when `ty` is `Enum`. The ONLY safe form for anything that
    /// reaches an identifier position — see spec §3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<String>>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope { PerPrincipal, Shared }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct CacheSpec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u32,
    /// `Shared` is refused at validate time on a delegated connection: Ranger
    /// filters per principal, so a shared cache would leak across identities.
    #[serde(default = "default_scope")]
    pub scope: CacheScope,
}
fn default_ttl() -> u32 { 60 }
fn default_scope() -> CacheScope { CacheScope::PerPrincipal }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct LimitsSpec {
    #[serde(default = "default_max_rows")]
    pub max_rows: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}
fn default_max_rows() -> u32 { 10_000 }
fn default_timeout() -> u32 { 30 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct RateLimitSpec {
    #[serde(default)] pub per_key_qps: u32,
    #[serde(default)] pub burst: u32,
    #[serde(default)] pub per_endpoint_qps: u32,
}

/// What a published version freezes. Self-contained on purpose: P4's serving
/// path must be able to answer a request from this alone, without reading the
/// draft row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct EndpointSnapshot {
    pub sql: String,
    pub method: String,
    pub path: String,
    pub database_id: uuid::Uuid,
    pub params: Vec<ParamSpec>,
    pub output: OutputSpec,
    pub cache: CacheSpec,
    pub limits: LimitsSpec,
    pub rate_limit: RateLimitSpec,
    /// Resolved by `describe_as` at publish time and frozen here, so the
    /// published contract does not drift when the underlying table changes.
    pub columns: Vec<ColumnSpec>,
}

/// The frozen form of a `ColumnMeta`. Deliberately its own type rather than
/// re-using `egret_core::ColumnMeta`: this one is persisted inside a published
/// snapshot, so it must stay stable even if the engine-facing type gains or
/// renames a field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ColumnSpec { pub name: String, pub ty: String }

impl From<&egret_core::ColumnMeta> for ColumnSpec {
    fn from(c: &egret_core::ColumnMeta) -> Self {
        // ColumnMeta is { name: String, ty: String } — verified at
        // crates/egret-core/src/tabular.rs:11-15.
        Self { name: c.name.clone(), ty: c.ty.clone() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct OutputSpec {
    #[serde(default = "default_shape")]
    pub shape: String,   // "objects" | "arrays"
    #[serde(default = "default_naming")]
    pub naming: String,  // "as_is" | "camel" | "snake"
}
fn default_shape() -> String { "objects".into() }
fn default_naming() -> String { "as_is".into() }
```

- [ ] **Step 3: Write and run the round-trip test**

Create `crates/egret-dataapi/tests/endpoints_schema.rs` asserting that an `EndpointSnapshot` survives a jsonb round trip through Postgres unchanged (serialize → insert → select → deserialize → `assert_eq!`). This is what proves the snapshot is genuinely self-contained and stable; a field that fails to round-trip would silently change a published contract.

Run: `cargo test -p egret-dataapi --test endpoints_schema`

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add migrations/0121_dataapi_endpoints.sql crates/egret-dataapi-api crates/egret-dataapi/tests/endpoints_schema.rs
git commit -m "feat(dataapi): endpoint and endpoint_versions schema

Two tables, and the split is the feature: endpoints is the editable draft,
endpoint_versions holds frozen snapshots, and P4's serving path will read
only the latter. The snapshot is self-contained so serving never needs the
draft row.

The URL path is free-form and deliberately not derived from folder_id —
folders are re-parented freely, a URL is a published contract."
```

---

### Task 3: Draft CRUD

**Files:**
- Create: `crates/egret-dataapi/src/endpoints.rs`
- Modify: `crates/egret-dataapi/src/lib.rs`, `crates/egret-dataapi/src/routes.rs`, `crates/egret-core/src/rbac.rs`
- Create: `migrations/0122_dataapi_endpoint_permissions.sql`
- Test: `crates/egret-dataapi/tests/endpoints_routes.rs` (create)

**Interfaces:**
- Consumes: `DataApiState` (P2), the `scoped_*` helper idiom in `routes.rs`.
- Produces: `EndpointStore` with `create`, `update`, `list`, `get`, `delete`, all workspace-scoped.

Routes:
```text
GET    /api/ws/{workspace}/data-api/endpoints
POST   /api/ws/{workspace}/data-api/endpoints
GET    /api/ws/{workspace}/data-api/endpoints/{id}
PATCH  /api/ws/{workspace}/data-api/endpoints/{id}
DELETE /api/ws/{workspace}/data-api/endpoints/{id}
```

- [ ] **Step 1: Add the permissions**

In `crates/egret-core/src/rbac.rs`, beside P2's `service_account:*` entries:

```rust
    perm!(
        "data_api_endpoint:read",
        "Data API",
        "View Data API endpoints and their versions",
        Scope::Workspace
    ),
    perm!(
        "data_api_endpoint:manage",
        "Data API",
        "Create, edit, publish and roll back Data API endpoints",
        Scope::Workspace
    ),
```

Create `migrations/0122_dataapi_endpoint_permissions.sql` seeding both onto `ws_admin`, copying the shape of `0120_dataapi_service_account_permissions.sql` **including its disclosure comment** — this migration widens what every existing `ws_admin` can do the moment it runs, and 0120 established that this must be said out loud.

- [ ] **Step 2: Write the failing tests**

Create `crates/egret-dataapi/tests/endpoints_routes.rs`. Copy the fixture from P2's `crates/egret-dataapi/tests/routes_authz.rs` — it already builds a router with a real `CedarPdp`, a workspace, and `principal_with(&[...])`.

```rust
#[tokio::test]
async fn read_permission_cannot_create_an_endpoint() {
    let f = fixture().await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;
    let res = f.post_as(reader, &format!("/api/ws/{}/data-api/endpoints", f.ws_slug),
        serde_json::json!({ "name": "rev", "path": "reports/revenue",
                            "database_id": f.database_id, "sql": "SELECT 1" })).await;
    assert_eq!(res.status(), 403, "read must not imply create");
}

#[tokio::test]
async fn an_endpoint_cannot_be_read_across_workspaces() {
    let f = fixture().await;
    let ep = f.create_endpoint("rev", "reports/revenue", "SELECT 1").await;
    let other = f.create_workspace("other").await;
    let mgr = f.principal_with_in(&["data_api_endpoint:manage"], other).await;
    let res = f.get_as(mgr, &format!("/api/ws/{other}/data-api/endpoints/{ep}")).await;
    assert_eq!(res.status(), 404, "not 403 — absence and denial must look the same");
}

#[tokio::test]
async fn duplicate_method_and_path_is_a_conflict() {
    let f = fixture().await;
    f.create_endpoint("a", "reports/revenue", "SELECT 1").await;
    let mgr = f.principal_with(&["data_api_endpoint:read", "data_api_endpoint:manage"]).await;
    let res = f.post_as(mgr, &format!("/api/ws/{}/data-api/endpoints", f.ws_slug),
        serde_json::json!({ "name": "b", "path": "reports/revenue",
                            "database_id": f.database_id, "sql": "SELECT 2" })).await;
    assert_eq!(res.status(), 409, "the UNIQUE (workspace, method, path) must surface as 409");
}
```

Note the permission pair in the third test: egret's permissions are **flat**, so `:manage` does not imply `:read`. P2 learned this the hard way — grant both where a realistic operator would hold both.

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test -p egret-dataapi --test endpoints_routes`

Expected: compile error — the routes do not exist.

- [ ] **Step 4: Implement the store and routes**

Write `crates/egret-dataapi/src/endpoints.rs` (store) and extend `routes.rs`, following P2's shape exactly: `WsContext(ctx)`, `require(&*st.pdp, &ctx, ACTION)` first, `ctx.workspace_id.0` for scoping, a `scoped_endpoint` helper returning 404 on mismatch, and `Path((_ws_ignored, id)): Path<(String, Uuid)>` so the path workspace is never used.

Map the `UNIQUE (workspace_id, method, path)` violation to `Conflict` (409), and say *which* path collided in the message — the author needs to know.

- [ ] **Step 5: Run and verify they pass**

Run: `cargo test -p egret-dataapi --test endpoints_routes`

Expected: PASS, 3 tests.

- [ ] **Step 6: Verify each gate by mutation**

1. Drop the `require(..., ":manage")` gate on create → `read_permission_cannot_create_an_endpoint` FAILS.
2. Drop the `scoped_endpoint` call on get → `an_endpoint_cannot_be_read_across_workspaces` FAILS.

Revert each. A gate whose removal leaves the suite green is not a gate.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-dataapi crates/egret-core/src/rbac.rs migrations/0122_dataapi_endpoint_permissions.sql
git commit -m "feat(dataapi): endpoint draft CRUD

Workspace-scoped, gated on data_api_endpoint:read/:manage. Cross-workspace
access 404s rather than 403s, so absence and denial are indistinguishable.
Permissions are flat here, so :manage does not imply :read."
```

---

### Task 4: Validate, publish, rollback, disable

**Files:**
- Create: `crates/egret-dataapi/src/lifecycle.rs`
- Modify: `crates/egret-dataapi/src/routes.rs`, `crates/egret-dataapi/src/lib.rs`
- Test: `crates/egret-dataapi/tests/lifecycle.rs` (create)

**Interfaces:**
- Consumes: `QueryExecPort::describe_as` (Task 1), `EndpointStore` (Task 3), `EndpointSnapshot` (Task 2).
- Produces: `validate(...) -> Result<Vec<ColumnSpec>, ValidationError>`, `publish`, `rollback`, `disable`.

Routes:
```text
POST /api/ws/{workspace}/data-api/endpoints/{id}/validate
POST /api/ws/{workspace}/data-api/endpoints/{id}/publish
POST /api/ws/{workspace}/data-api/endpoints/{id}/rollback   { "version": N }
POST /api/ws/{workspace}/data-api/endpoints/{id}/disable
GET  /api/ws/{workspace}/data-api/endpoints/{id}/versions
```

- [ ] **Step 1: Write the SQL-shape validator**

In `crates/egret-dataapi/src/lifecycle.rs`:

```rust
/// Reject anything that is not exactly one read-only statement.
///
/// This is a shape check, not a parser: it strips comments and string literals,
/// then looks at what is left. A leading `WITH … SELECT` (CTE) is allowed —
/// it is the normal way to write a readable analytical query. Anything else
/// (DML, DDL, `SET`, or a second statement after `;`) is refused.
pub fn check_single_select(sql: &str) -> Result<(), ValidationError> {
    let stripped = strip_comments_and_strings(sql);
    let trimmed = stripped.trim().trim_end_matches(';').trim();
    if trimmed.contains(';') {
        return Err(ValidationError::MultipleStatements);
    }
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if head != "SELECT" && head != "WITH" {
        return Err(ValidationError::NotASelect(head));
    }
    Ok(())
}
```

`strip_comments_and_strings` must remove `--` line comments, `/* */` block comments, and single-quoted literals **before** the `;` scan — otherwise `SELECT 'a;b'` is rejected and `SELECT 1 -- ; DROP` is accepted, both wrong. Write it with the escaping rules P1 established: on StarRocks a backslash escapes inside a literal, on Trino it does not, so strip conservatively (treat `\` as escaping inside a literal on both) — over-stripping can only cause a false rejection at validate time, never a false acceptance.

- [ ] **Step 2: Write the failing tests**

```rust
    #[test]
    fn accepts_select_and_cte() {
        assert!(check_single_select("SELECT 1").is_ok());
        assert!(check_single_select("  select 1  ").is_ok());
        assert!(check_single_select("WITH t AS (SELECT 1) SELECT * FROM t").is_ok());
        assert!(check_single_select("SELECT 1;").is_ok(), "one trailing semicolon is fine");
    }

    #[test]
    fn rejects_everything_else() {
        assert!(check_single_select("INSERT INTO t VALUES (1)").is_err());
        assert!(check_single_select("DELETE FROM t").is_err());
        assert!(check_single_select("SET x = 1").is_err());
        assert!(check_single_select("SELECT 1; DROP TABLE t").is_err());
        assert!(check_single_select("").is_err());
    }

    #[test]
    fn a_semicolon_inside_a_literal_is_not_a_statement_break() {
        // The naive `contains(';')` check rejects this, which would make a
        // perfectly valid query unpublishable.
        assert!(check_single_select("SELECT 'a;b' AS s").is_ok());
    }

    #[test]
    fn a_semicolon_hidden_in_a_comment_does_not_smuggle_a_statement() {
        // ...and the mirror: stripping comments must happen before the scan, or
        // `-- ;` hides a second statement from the check.
        assert!(check_single_select("SELECT 1 -- ; DROP TABLE t\n").is_ok());
        assert!(check_single_select("SELECT 1 /* ; */ ; DROP TABLE t").is_err());
    }
```

The last two are the point of the task: the obvious implementation passes the first two tests and fails these.

- [ ] **Step 3: Run, implement, run**

Run: `cargo test -p egret-dataapi --lib lifecycle`

Expected: red, then implement `strip_comments_and_strings`, then green.

- [ ] **Step 4: Write the parameter and config checks**

```rust
/// Every `{{param}}` resolves to a declared parameter, and every declared
/// parameter is referenced. Both directions matter: an unreferenced parameter
/// is a lie in the generated OpenAPI, and an undeclared placeholder is a 500
/// waiting to happen at serve time.
pub fn check_params(sql: &str, params: &[ParamSpec]) -> Result<(), ValidationError>;

/// `Shared` cache is refused on a delegated connection: Ranger filters per
/// principal, so one principal's rows would be served to another. Refusing at
/// VALIDATE time means the unsafe configuration can never be published, rather
/// than being caught (or not) at serve time.
pub fn check_cache_scope(cache: &CacheSpec, delegated: bool) -> Result<(), ValidationError>;

/// An `Enum` parameter must carry a non-empty `allowed` list — it is the only
/// safe form for a value that may reach an identifier position.
pub fn check_enum_params(params: &[ParamSpec]) -> Result<(), ValidationError>;
```

Write a test for each, including the negative case for each.

- [ ] **Step 5: Implement publish / rollback / disable**

`publish` runs the full validation, calls `describe_as` to resolve the output columns, builds an `EndpointSnapshot`, inserts it as `version = max(version) + 1` for that endpoint, sets `endpoints.live_version_id` and `state = 'live'` — **all in one transaction**, so a failed describe cannot leave a half-published endpoint.

`rollback` sets `live_version_id` to an existing older version's id; it must refuse a version id belonging to a different endpoint. `disable` sets `state = 'disabled'` and leaves `live_version_id` alone, so re-enabling does not need a re-publish.

- [ ] **Step 6: The invariant test — this is the one that matters**

```rust
#[tokio::test]
async fn a_published_version_is_unaffected_by_later_draft_edits() {
    let f = fixture().await;
    let ep = f.create_endpoint("rev", "reports/revenue", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let v1 = f.live_snapshot(ep).await;
    assert_eq!(v1.sql, "SELECT 1 AS n");

    // Edit the draft afterwards — including the SQL and the parameter list.
    f.patch_endpoint(ep, serde_json::json!({ "sql": "SELECT 2 AS n" })).await;

    let still = f.live_snapshot(ep).await;
    assert_eq!(
        still.sql, "SELECT 1 AS n",
        "the live version must not follow the draft — this is what makes versioning real"
    );
}

#[tokio::test]
async fn rollback_restores_the_previous_snapshot() {
    let f = fixture().await;
    let ep = f.create_endpoint("rev", "reports/revenue", "SELECT 1 AS n").await;
    f.publish(ep).await;                                     // v1
    f.patch_endpoint(ep, serde_json::json!({ "sql": "SELECT 2 AS n" })).await;
    f.publish(ep).await;                                     // v2
    assert_eq!(f.live_snapshot(ep).await.sql, "SELECT 2 AS n");

    f.rollback(ep, 1).await;
    assert_eq!(f.live_snapshot(ep).await.sql, "SELECT 1 AS n");
}

#[tokio::test]
async fn rollback_refuses_a_version_from_another_endpoint() {
    let f = fixture().await;
    let a = f.create_endpoint("a", "a", "SELECT 1").await;
    let b = f.create_endpoint("b", "b", "SELECT 2").await;
    f.publish(a).await;
    f.publish(b).await;
    let res = f.rollback_raw(a, f.version_id_of(b, 1).await).await;
    assert_eq!(res.status(), 404, "a version belongs to exactly one endpoint");
}
```

- [ ] **Step 7: Mutation-verify the invariant**

Change `live_snapshot`'s read path to fall back to the draft row when convenient (for example, read `sql` from `endpoints` instead of the snapshot). Expected: `a_published_version_is_unaffected_by_later_draft_edits` FAILS. Revert.

If that test does not go red under this mutation, the store is already reading the draft somewhere and the invariant is not actually held — stop and report it rather than moving on.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-dataapi
git commit -m "feat(dataapi): validate, publish, rollback, disable

Publishing freezes a self-contained snapshot including the output schema
resolved by describe, in one transaction, so a failed describe cannot leave
a half-published endpoint.

The validator strips comments and string literals before scanning for a
statement break, so SELECT 'a;b' publishes and SELECT 1 /* ; */ ; DROP does
not — the naive check gets both backwards."
```

---

### Task 5: Audit events, and close P2's parked duplication

**Files:**
- Modify: `crates/egret-dataapi/src/routes.rs`, `crates/egret-dataapi/Cargo.toml`, `crates/egret-bin/src/main.rs`
- Modify: `crates/egret-auth/src/users.rs`
- Modify: `docs/en/specs/2026-08-14-data-api-design.md`, `docs/vi/specs/...`
- Test: `crates/egret-dataapi/tests/audit.rs` (create)

- [ ] **Step 1: Wire the audit sink**

Add `pub audit: egret_audit::AuditSink` to `DataApiState` and populate it in `crates/egret-bin/src/main.rs` from the same sink `CpState` already receives. Emit `egret_audit::mutation_for(&ctx.principal, Some(ws), action, "data_api_endpoint", Some(id.to_string()), detail)` from `publish`, `rollback`, `disable`, `delete_endpoint`, and — closing P2's gap — `issue_client` and `revoke_client` with `resource_type = "data_api_client"`.

`AuditSink::emit` is non-blocking and never fails the caller, so no handler needs to change its error type.

**Do not include the client secret in `detail`.** Record the `client_id` and the account, never the secret. Add a test asserting the emitted detail contains no `client_secret` key — a secret reaching the audit table would outlive every rotation.

- [ ] **Step 2: Correct the spec**

Spec §5.3 currently says publish/rollback are audit events and that client issue/revoke are P3. After this task all six are emitted — update both language files to say so, and remove the P3-deferral note. This is the third time this spec has had to be reconciled with the code; check both files agree before committing.

- [ ] **Step 3: Close P2's parked duplication**

P2's final review left this open: `UserRepo::insert_service` has **zero production callers** because `create_service_account` inlines an equivalent INSERT to join its transaction, so the two writes can drift and the tests pin a path production no longer takes.

Fix: make it generic over the executor so both paths use one function.

```rust
    /// Insert a service account: a user row that can never log in.
    ///
    /// Generic over the executor so a caller inside a transaction (the Data API
    /// service-account create path) and a caller on the pool use the SAME write.
    pub async fn insert_service_with<'e, E>(exec: E, email: &str, name: &str) -> Result<UserId>
    where
        E: sqlx::PgExecutor<'e>,
    { /* the existing body, with `exec` in place of `&self.db.pool` */ }
```

Keep `insert_service(&self, …)` as a thin wrapper delegating to it, so existing tests keep working. Change `create_service_account` to call `insert_service_with(&mut *tx, …)`. Verify the inlined SQL is gone.

- [ ] **Step 4: Full scoped check**

```bash
cargo fmt --all --check
cargo clippy -p egret-dataapi -p egret-dataapi-api -p egret-query -p egret-query-api -p egret-auth -p egret-core -p egret-bin --all-targets
cargo test -p egret-dataapi -p egret-dataapi-api -p egret-query -p egret-query-api -p egret-auth -p egret-core
```

Sequentially, with `DATABASE_URL` at a throwaway database, checking durations are non-zero.

- [ ] **Step 5: Commit**

```bash
git add crates/ docs/
git commit -m "feat(dataapi): audit publish/rollback/disable and client lifecycle

Closes the gap the spec described but P2 did not implement. The detail
carries the client_id and account, never the secret — a secret in the audit
table outlives every rotation.

Also folds the service-account INSERT back into one executor-generic
function; P2 left an inlined copy in the transaction path, so
UserRepo::insert_service had no production callers and the two writes could
drift."
```

---

## Done when

- `cargo test` green for all seven crates in Task 5 Step 4; fmt and clippy clean.
- Every test named in Tasks 1, 3 and 4 has been observed FAILING against its stated mutation. In particular **Task 4 Step 7** — if the draft-isolation test does not go red when the read path is pointed at the draft, the invariant is not held and this plan is not done.
- An author can create a draft, validate it, publish it, edit the draft without affecting the live version, roll back, and disable — through the HTTP API, not just the store.
- `describe_as` has been run against a live Trino with `EGRET_TEST_TRINO_URL` set, with no "skip" in the output.
- The spec's §5.3 matches what is emitted, in both languages.

## Not in this plan

- **The serving surface.** No endpoint is callable yet; `/data-api/v1` is P4, along with `execute_as`, the cache, the rate limiter, the error envelope and `dataapi.access_log`.
- **`execute_as` on the port** — added in P4 where it has a consumer.
- **"Publish as API" from a Saved Query.** `saved_queries` carries `sql`, `database_id`, `workspace_id`, so seeding a draft from one is a small addition — but it is UI-driven and belongs with P5's editor.
- **Frontend** — P5.
- **The P2 minors that remain open**: the Argon2 timing oracle, `/oauth/token` rate limiting, and `jsonwebtoken`'s missing crypto-backend feature. All recorded on #505; none is P3's to fix.
