# Catalog Registry — SP3a (Backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a platform-level catalog registry the source of truth for every engine catalog: typed per-kind configuration with validation, Trino rendering, a boot-time backfill of today's warehouses and engine catalogs with a migration report, registry/attachment/workspace-assignment APIs with `catalog_registry:*` permissions, Trino attach/detach and edit propagation with per-engine apply status and Retry — while today's engine-catalog endpoints keep working over the registry.

**Architecture:** A new leaf crate `egret-catalog-registry` owns the `registry` schema, the typed config (`CatalogDef`), the pure Trino renderer, the legacy-row classifier, the store and the boot backfill. `egret-engine-mgmt` stays the only place that talks to engines: `build_provision_req_with` renders catalogs from registry attachments instead of `engine_mgmt.engine_catalogs`, a new `catalog_apply` module owns attach/detach/re-apply and dispatches on engine type (the StarRocks arm is the SP3b seam), and a new `catalogs` module serves the registry routes. `egret-catalog-engine` (warehouse routes, notebook visibility), `egret-policy` (Ranger tag export), `egret-query` (workspace engine view) and `egret-bin` (Ranger reader, boot backfill) switch their catalog reads to the registry.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9 / Postgres, utoipa 5, serde, regex, tokio), orval via `npm run gen:api`.

**Spec:** `docs/en/specs/2026-09-19-catalog-registry-design.md` — §6 row SP3a: §4.1, §4.2, §4.3 (Trino), §4.4 (Trino), §4.5, §4.6, §4.7, §4.8, and the SP3a rows of §7.

## Global Constraints

- Branch: `feat/catalog-registry-sp3a`, cut from `origin/main`. Never commit on `main`; run `git branch --show-current` before every commit and stop if it does not print `feat/catalog-registry-sp3a`. Never `git stash`.
- `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` in every shell that runs cargo.
- DB tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` and run cargo test under `env -u EGRET_ALLOW_DB_SKIP`. **Never** the dev database `egret`. A skipped DB test prints `ok`: read every `finished in` — a DB test binary that finishes in well under 0.3 s did no database work.
- After adding a migration (Task 4 and Task 8), drop the template once: `psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'`. The migrations CLI reads `EGRET_DATABASE__URL`, not `DATABASE_URL`.
- Shared-store tests need `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399` (only the full workspace run in Task 11 reaches them).
- Never run two test suites at once. Per-task runs use `-p <crate>`; `cargo test --workspace` runs exactly once, in Task 11.
- Rust gate per task: `cargo fmt --all -- --check` and `cargo clippy -p <crates touched> --all-targets -- -D warnings`. (Whole-workspace clippy has two pre-existing `egret-workflow` failures; Task 11 records them rather than fixing them.)
- OpenAPI: regenerate with `rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json` (without `rtk proxy` the RTK hook filters stdout and corrupts the file); drift gate `cargo test -p egret-server --lib committed_openapi_json_is_current`.
- Frontend client: `cd frontend && npm run gen:api` — never bare `orval`. CI's frontend job runs `npm run gen:api` and **fails on any drift**, so commit exactly what `gen:api` produced (`git add -A frontend/src/api/generated`), including churn you did not cause. Before committing, confirm `encodeURIComponent` survived on every new path parameter.
- sqlx 0.9: `sqlx::query*` takes `&'static str`. Every SQL string in this plan is a literal or a `concat!` of literals; never build SQL with `format!`.
- Standing rules (SP1 Part B): egret never deletes a secret value an engine may still hold; engine delete never deletes a Ranger profile; apply before commit (attach renders the new catalog and writes the attachment only after Kubernetes accepted it; detach applies without it and deletes the attachment only after); `apply_built` refuses when the engine row is gone or `deleting`.
- Name rule (spec §4.1): `^[a-z][a-z0-9_]{0,62}$`, immutable. Normalisation (§4.8): lowercase, every other character → `_`, a name that does not start with a letter gets a `w_` prefix, truncate to 63, then `_2`, `_3`… on collision.
- Config JSON inside `registry.catalogs.config` is snake_case exactly as spec §4.2 names the fields; API DTO envelopes are camelCase.
- `catalog_registry:read|create|update|delete` are platform permissions; assign-to-workspace, re-apply and Retry use `catalog_registry:update`; attach uses `engine:update` (it restarts the engine); the legacy engine-catalog routes keep their `engine:*` gates.
- RTK rewrites shell commands. Any command whose raw stdout is piped or redirected runs as `rtk proxy <cmd>`; `rtk grep` swallows piped stdin — never verify a diff with it.
- Commits: Conventional Commits; body ends with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Never override the author.

## Spec statements checked against the code and the kind cluster, and the decisions they forced

Kind data read on 2026-09-19 (`kubectl --context kind-egret … psql`, SELECT only): 24 warehouses, all in workspace `522a9d07-…`, one engine `demo-trino` (`warehouse_id = a946808b-…` = `e2e-wh-1784523876270`) with one primary `egret-warehouse` row named `egret` whose config keys are `warehouse, credential, clientRowId, s3_endpoint`. The kind database is at migration **0143** (0144–0146 are on `main` but not deployed there).

| # | Spec statement | Verdict | Decision in this plan |
|---|---|---|---|
| 1 | §4.8.1 "all 24 are renamed (e.g. `e2e-wh-1784519491585` → `e2e_wh_1784519491585`)" | **Contradicts §4.8.2.** Exactly one warehouse is an engine primary, so the exception applies: `e2e-wh-1784523876270` becomes `egret`; the other 23 get normalised names (all distinct, e.g. `e2e-wh-1784567124897-p` → `e2e_wh_1784567124897_p`). | Backfill implements §4.8.2's exception; Task 6's single-primary test pins it. |
| 2 | §4.1 "`catalog.warehouses.workspace_id` is dropped" | **Not in SP3a.** It is `NOT NULL` input to `catalog.catalog_clients.workspace_id` (every `NewClient` passes `wh.workspace_id`), `WarehouseDto`, `create_warehouse`, the Warehouses page filter, the notebook check and five test seeds. | SP3a copies it into `catalog_workspaces` **once, when the catalog is created**, and switches the one server-side reader (notebook warehouse check). The column is dropped in SP3c with the Warehouses pages. |
| 3 | §4.1 "`engines.warehouse_id` becomes nullable" | **Premature.** `POST /engines` still requires a warehouse in SP3a, and `build_starrocks_provision_req` reads it for the StarRocks builtin storage volume. | Stays `NOT NULL`; it also defines the engine's *primary* catalog (the one detach refuses). SP3b/SP3c make it nullable. |
| 4 | §4.1 attachment columns | **Missing the credential.** Today the minted client's `client_id:client_secret` sits in plaintext in `engine_catalogs.config.credential`; the spec's attachment has only `catalog_client_id`. | Attachment gains `credential_secret_id` (a global named secret) and `minted_client` (today's `mintedClient` flag). Backfill moves each plaintext credential into a secret. |
| 5 | §4.1 `ON DELETE RESTRICT` on `catalog_id` | **Would block every workspace delete.** `workspaces → warehouses` cascades; with a catalog per warehouse, a RESTRICT on `catalog_workspaces.catalog_id` fires before the same statement removes the workspace's own assignment row. | `NO ACTION` (checked at end of statement) on both `catalog_id` FKs. The API still answers 409 before touching anything. |
| 6 | §4.1 the warehouse link lives only in `config` | Allows two catalogs per warehouse and no FK. | Mirrored `warehouse_id` (FK `ON DELETE CASCADE`, partial unique) and `storage_connection_id` (FK) columns, pinned to `config` by CHECKs; the store writes both. |
| 7 | §4.7 new permissions `catalog:read/create/update/delete` | **`catalog:read` already exists** as a workspace-scope key (migration 0024, held by `ws_admin/ws_editor/ws_viewer`, enforced by `egret-query` `registry/routes.rs` `PERM_READ`). Reusing `catalog:read` for the platform permission was first proposed and rejected by the controller. **Correction (Task 8):** the controller's stated reason — that a platform grant would satisfy the workspace-scope check in every workspace — was FALSE: non-admin platform-role templates bind `resource == Platform::"root"` (egret-auth `pdp/templates.rs`, the B1b-2 anti-leak), shown both empirically and from the source. The distinct keys stand for a different, real reason: permission keys are scope-blind strings (`is_known`, effective-permission lists, admin UI, Cedar actions), so one key with two meanings is ambiguous. | New, distinct platform keys `catalog_registry:read/create/update/delete` (spec §4.7 updated in both languages). Migration 0148 grants each platform role the `catalog_registry:*` counterpart of every `warehouse:*` it holds; the workspace `catalog:read` from 0024 is untouched. Task 8's regression test pins the B1b-2 anti-leak for the same-name case (a platform grant of `catalog:read` never satisfies the workspace `catalog:read` check). |
| 8 | §4.7 migration grants | A debug build panics at boot when a platform role holds a key missing from `PERMISSIONS` (`egret-bin/src/main.rs` ≈1053). | The grant is its own migration (0148) and ships in the same task as the `PERMISSIONS` rows and the drift-lint file list. |
| 9 | §4.8.2 saved SQL "datasets, charts, Data API endpoints, alerts, saved queries" | **Charts hold no SQL** (`analytics.charts.params` names a dataset). Real columns: `analytics.datasets.sql` and `.catalog`, `analytics.saved_queries.sql`, `automation.schedules.sql` (alerts and reports), `dataapi.endpoints.statements[*].sql` (draft), `dataapi.endpoint_versions.snapshot` `sql`/`statements[*].sql` via `endpoints.live_version_id`, and `analytics.sql_snippets.body` (not in the spec, same risk). | Task 6's scan covers exactly these; charts are reached through their dataset. |
| 10 | §4.8.2 scan only for the several-primaries case | Any legacy row whose SQL name changes breaks saved SQL the same way (normalised odd names, `_<engine>` suffixes, an extra `egret-warehouse` row whose warehouse catalog has another name). | The scan runs for every old name that changed; the several-primaries case is one instance. |
| 11 | §4.4 `POST/DELETE /engines/{id}/catalogs/{catalog_id}` | **Same path as today's legacy GET/PUT/DELETE `/engines/{id}/catalogs/{cid}`.** | The legacy `cid` becomes the registry catalog id, so the legacy DELETE **is** detach. It keeps its 400-on-rejected-apply for today's UI/tests; the new POST attach answers 502. SP3c unifies. |
| 12 | §4.2 D5 vs today's `egret-warehouse` add | Today an engine may add a second catalog with its own name on its primary warehouse. Under D5 a warehouse is one catalog with one name. | Legacy add of an already-attached warehouse → 409; legacy add of `egret-warehouse` ignores the body name and uses the warehouse catalog's name. Four route tests move to a second warehouse. |
| 13 | D5 for new engines | Today every engine's primary is `egret`. | A new engine's primary is its warehouse's catalog, named after the warehouse. Existing single-primary platforms keep `egret` through the exception. **User-visible.** |
| 14 | §4.2 `ssl verify_full {ca_secret_id?}` | The Trino provisioner has no file mounts, so a CA cannot reach the JVM. | Validation refuses `ca_secret_id` ("not supported yet"); `verify_full` uses the default trust store. |
| 15 | §4.2 JDBC `database?` | Trino's MySQL/MariaDB URLs carry no database; PostgreSQL, Oracle, SQL Server need one. | Required for postgresql/oracle/sqlserver, forbidden for mysql/mariadb, optional for clickhouse. |
| 16 | §4.2 `hudi` with `glue` | Trino's Hudi connector reads a Hive metastore only (`trino/docs/…/hudi.md`). | Refused. |
| 17 | §4.2 `tpch/tpcds scale?` | Trino has no scale property; the schema name is the scale factor. | Stored, not rendered for Trino (StarRocks `benchmark` uses it in SP3b). |
| 18 | §4.8.3 "a recognised kind whose properties parse becomes typed" | Parsing is not enough: pgjdbc/Connector/J default to `sslmode=prefer`/`PREFERRED`, which no typed `ssl` value renders. | Typed only if the typed form renders **exactly** the same Trino properties (round-trip gate); otherwise `generic`. Typed classifiers: JDBC dialects and TPC-H/TPC-DS. |
| 19 | §4.1 "never holds a plaintext secret" | Legacy rows carry plaintext passwords in `properties` (masked on read only). | Backfill and legacy add/PUT move every sensitive plaintext value into a global named secret. Sensitive = SP1's key regex, **except** boolean/numeric values (`iceberg.rest-catalog.vended-credentials-enabled=true` is not a secret). |
| 20 | §4.2 `generic` "every property lives in `advanced`" | Leaves secret-valued properties nowhere. | `config.secret_properties.{trino,starrocks}: {key: secret_id}`. |
| 21 | §4.3 `render_starrocks` | SP3b. | SP3a ships the seam, not a stub renderer: `catalog_apply::apply_catalog_change` dispatches on engine type and its StarRocks arm answers "not supported yet" (detach of a never-rendered catalog succeeds). Attach to StarRocks is refused; edits flag StarRocks attachments `failed`. |
| 22 | §4.5.4 "a failure does not revert the catalog" | Legacy `PUT /engines/{id}/catalogs/{cid}` restored the old config on failure. | Legacy PUT now has registry semantics; it answers 400 when the path engine's apply failed, without reverting. |
| 23 | §4.5 delete 409 "same shape as engine delete" | `EngineConflictBody.dependents` are per-workspace asset kinds. | New `CatalogConflictBody { error, code: "catalog_in_use", engines[], workspaces[] }`. |
| 24 | §4.6 "data browsing" | No server-side data-browsing path reads warehouse ownership; the CP Workspace detail filters warehouses client-side (SP3c). | SP3a switches the notebook warehouse check and adds `?workspaceId=` to `GET /catalogs`. |
| 25 | §4.8.4 tag export via "the attachment" | — | Joins every attached warehouse catalog, not only the primary. |
| 26 | "next free number is 0147" | — | Two migrations: 0147 (schema), 0148 (permissions, see #8). |
| 27 | §4.4 detach vs today's legacy DELETE | Today's DELETE deleted the engine's catalog row; under the registry the catalog outlives the attachment, so the old UI's "remove then add again with the same name" would hit a name clash. | DELETE detaches and keeps the registry catalog. A legacy add whose name belongs to a catalog that is attached nowhere and assigned nowhere replaces it (the same "adopt the orphan" rule engine create applies to Ranger profiles); an in-use name is 409. |
| 28 | Legacy PUT of a `generic` catalog | Re-classifying could turn a generic catalog typed (a kind change). | A generic catalog stays generic under the legacy form; a typed one must still fit its kind, else 400 pointing at `PUT /catalogs/{id}`. The kind of a catalog never changes. |
| 29 | §4.5 delete of an egret warehouse catalog | Deleting it through the registry would leave its warehouse without a catalog, and the next boot's backfill would recreate it. | `DELETE /catalogs/{id}` refuses (409) for a warehouse catalog; it goes with its warehouse (FK cascade), and the Warehouses page's delete clears its assignments and refuses while an engine attaches it. |
| 30 | D5 on StarRocks in SP3a | The FE bootstrap created a catalog literally named `egret`. | The bootstrap now receives the registry name of the engine's primary catalog (`egret` on single-primary platforms; the warehouse catalog's name for new engines). Everything else about StarRocks catalogs is SP3b. |
| 31 | Boot backfill failure | A catalog row the backfill cannot migrate (e.g. no credential, no secrets master key) would silently disappear from the engine at its next apply if rendering read only the registry. | Such a row is reported `backfill_failed` and left unmapped; `build_provision_req_with` refuses (409) to apply an engine that still has unmapped legacy rows. |

## File Structure

**New crate `crates/egret-catalog-registry`** — owns the `registry` schema and everything that does not need an engine.

| File | Responsibility |
|---|---|
| `Cargo.toml` | crate manifest |
| `src/lib.rs` | module list + re-exports |
| `src/name.rs` | identifier rule, normalisation, suffixing |
| `src/sensitive.rs` | sensitive-key / plaintext-secret predicates (moved from `engines.rs`) |
| `src/config.rs` | `CatalogKind`, `EngineType`, typed configs, `Advanced`, `CatalogDef::parse`/`validate`, `ValidationError` |
| `src/render/mod.rs`, `src/render/trino.rs` | `render_trino`, `jdbc_url`, `TrinoCatalogFile`, `RenderError` |
| `src/legacy.rs` | `legacy_props_and_secrets` (moved from `engines.rs`), `plaintext_secrets`, `classify` |
| `src/error.rs` | `RegistryError` |
| `src/report.rs` | `Severity`, `ReportEntry`, `ReportRow` |
| `src/store.rs` | `RegistryStore` (catalogs, attachments, assignments, secret refs, legacy map, report) |
| `src/scan.rs` | saved-SQL scan |
| `src/backfill.rs` | `run_backfill` |
| `tests/store.rs`, `tests/backfill_single_primary.rs`, `tests/backfill_several_primaries.rs` | DB tests (one scenario per binary: the backfill is global) |

**Migrations:** `migrations/0147_catalog_registry.sql`, `migrations/0148_catalog_permissions.sql`.

**`crates/egret-engine-mgmt`:** `src/catalog_apply.rs` (new: rendering an attachment, attach/detach/re-apply, the engine-type dispatch seam, legacy views), `src/catalogs.rs` (new: registry routes + OpenAPI doc), `src/engines.rs` (build from attachments, create/delete, StarRocks credential, legacy handlers), `src/store.rs` (`registry()` accessor), `src/http_error.rs` (`RegistryError` mapping, `CatalogInUse`), `src/lib.rs`, `Cargo.toml`, `tests/routes.rs`, `tests/dependents.rs`.

**Elsewhere:** `Cargo.toml` (workspace dep), `crates/egret-secrets/src/workspace_secrets.rs` (`used_by`), `crates/egret-core/src/rbac.rs`, `crates/egret-controlplane/tests/catalog_drift.rs`, `crates/egret-catalog-engine/src/{store/handle.rs,store/embedded.rs,warehouse_admin/mod.rs,warehouse_admin/error.rs}` + `Cargo.toml` + `tests/warehouse_admin_routes.rs`, `crates/egret-notebook/src/routes.rs`, `crates/egret-policy/src/store.rs` + `tests/download_tags_db.rs`, `crates/egret-query/src/registry/repo.rs` + `tests/registry_workspace_routes_api.rs`, `crates/egret-bin/src/main.rs` + `Cargo.toml`, `openapi.json`, `frontend/src/api/generated/**`.

---
### Task 1: `egret-catalog-registry` crate — names, kinds, typed configuration, validation

**Files:**
- Create: `crates/egret-catalog-registry/Cargo.toml`, `src/lib.rs`, `src/name.rs`, `src/sensitive.rs`, `src/config.rs`
- Modify: `Cargo.toml` (workspace dependency table, after the `egret-catalog-engine` line)

**Interfaces:**
- Consumes: nothing.
- Produces (all re-exported from the crate root):
  - `name`: `const MAX_NAME_LEN: usize = 63`, `fn is_valid_catalog_name(&str) -> bool`, `fn normalize_catalog_name(&str) -> String`, `fn with_suffix(base: &str, n: u32) -> String`, `fn join_truncated(base: &str, tail: &str) -> String`, `fn first_free_name(base: &str, taken: impl Fn(&str) -> bool) -> String`.
  - `sensitive`: `fn is_sensitive_key(&str) -> bool`, `fn is_plaintext_secret(key: &str, value: &str) -> bool`.
  - `config`: `enum EngineType { Trino, StarRocks }` (`as_str`, `parse`), `enum CatalogKind` (`as_str`, `parse`), structs/enums `IcebergConfig`, `IcebergMetastore`, `RestAuth`, `HiveFamilyConfig`, `HiveMetastore`, `JdbcConfig`, `JdbcDialect` (`as_str`), `JdbcSsl`, `ElasticsearchConfig`, `SampleConfig`, `GenericConfig`, `SecretProperties`, `Advanced`, `enum CatalogConfig` (`from_json`, `kind`, `to_json`), `struct CatalogDef { pub config: CatalogConfig, pub advanced: Advanced }` with `parse(kind: &str, config: &Value, advanced: &Value) -> Result<CatalogDef, ValidationError>`, `egret_warehouse(warehouse_id: Uuid) -> CatalogDef`, `validate(&self)`, `kind()`, `warehouse_id()`, `storage_connection_id()`, `secret_ids() -> BTreeSet<Uuid>`, `can_read(EngineType) -> bool`, `trino_connector_name() -> String`, `config_json()`, `advanced_json()`; `enum ValidationError`.

- [ ] **Step 0: Branch**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git fetch origin
git switch -c feat/catalog-registry-sp3a origin/main
git branch --show-current
```
Expected: `feat/catalog-registry-sp3a`.

- [ ] **Step 1: Manifest and workspace entry**

`crates/egret-catalog-registry/Cargo.toml`:

```toml
[package]
name = "egret-catalog-registry"
version = "0.1.0"
edition.workspace = true
license.workspace = true

# The platform catalog registry (SP3): the `registry` schema, the typed
# per-kind catalog configuration, rendering, and the boot-time backfill.
# No axum and no engine-mgmt dependency: engine-mgmt (routes, apply) and
# catalog-engine (warehouse routes) both depend on this crate.
[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
uuid.workspace = true
regex.workspace = true
```

In the root `Cargo.toml`, under `[workspace.dependencies]`, directly after `egret-catalog-engine = { path = "crates/egret-catalog-engine" }`:

```toml
egret-catalog-registry = { path = "crates/egret-catalog-registry" }
```

- [ ] **Step 2: `src/lib.rs`**

```rust
//! Platform catalog registry (SP3): a catalog is a platform resource that engines
//! attach and workspaces are assigned. This crate owns the `registry` schema, the
//! typed per-kind configuration and its validation, engine rendering and the
//! boot-time backfill. Engine-side effects (applying to Trino, StarRocks DDL) live
//! in `egret-engine-mgmt`, which depends on this crate.

pub mod config;
pub mod name;
pub mod sensitive;

pub use config::{
    Advanced, CatalogConfig, CatalogDef, CatalogKind, ElasticsearchConfig, EngineType,
    GenericConfig, HiveFamilyConfig, HiveMetastore, IcebergConfig, IcebergMetastore, JdbcConfig,
    JdbcDialect, JdbcSsl, RestAuth, SampleConfig, SecretProperties, ValidationError,
};
pub use name::{
    first_free_name, is_valid_catalog_name, join_truncated, normalize_catalog_name, with_suffix,
    MAX_NAME_LEN,
};
pub use sensitive::{is_plaintext_secret, is_sensitive_key};
```

- [ ] **Step 3: Write the failing name tests** — `src/name.rs` (tests first, functions stubbed with `todo!()` so the test binary compiles):

```rust
//! The registry name is the catalog's SQL identifier on every engine (spec D5),
//! so it follows the strictest rule both engines accept.

use std::sync::OnceLock;

/// Longest catalog name (the `registry.catalogs` CHECK allows 1 + 62).
pub const MAX_NAME_LEN: usize = 63;

/// `^[a-z][a-z0-9_]{0,62}$` — the rule `registry.catalogs.name` enforces.
pub fn is_valid_catalog_name(name: &str) -> bool {
    todo!()
}

/// Spec §4.8: lowercase, any other character becomes `_`, a name that does not
/// start with a letter gets a `w_` prefix, then truncate to [`MAX_NAME_LEN`].
pub fn normalize_catalog_name(raw: &str) -> String {
    todo!()
}

/// `base` + `_<n>`, truncating `base` so the result still fits.
pub fn with_suffix(base: &str, n: u32) -> String {
    todo!()
}

/// `base` + `_` + `tail`, truncated to [`MAX_NAME_LEN`] (the `_<engine>` suffix
/// §4.8.3 gives a same-name catalog with a different configuration).
pub fn join_truncated(base: &str, tail: &str) -> String {
    todo!()
}

/// `base` when free, else the first free `base_2`, `base_3`, ….
pub fn first_free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_cluster_warehouse_names_normalise_to_distinct_identifiers() {
        assert_eq!(normalize_catalog_name("e2e-wh-1784519491585"), "e2e_wh_1784519491585");
        assert_eq!(normalize_catalog_name("e2e-wh-1784567124897-p"), "e2e_wh_1784567124897_p");
        assert_eq!(normalize_catalog_name("e2e-wh-gov16659"), "e2e_wh_gov16659");
        assert!(is_valid_catalog_name("e2e_wh_1784519491585"));
        assert!(!is_valid_catalog_name("e2e-wh-1784519491585"));
    }

    #[test]
    fn a_name_that_does_not_start_with_a_letter_gets_a_prefix() {
        assert_eq!(normalize_catalog_name("1st"), "w_1st");
        assert_eq!(normalize_catalog_name("-x"), "w__x");
        assert_eq!(normalize_catalog_name(""), "w_");
        assert_eq!(normalize_catalog_name("Sales Data"), "sales_data");
        assert_eq!(normalize_catalog_name("Kho dữ liệu"), "kho_d__li_u");
    }

    #[test]
    fn normalised_names_are_always_valid_and_fit() {
        let long = "A".repeat(90);
        let n = normalize_catalog_name(&long);
        assert_eq!(n.len(), MAX_NAME_LEN);
        assert!(is_valid_catalog_name(&n));
        assert!(!is_valid_catalog_name(&"a".repeat(64)));
        assert!(is_valid_catalog_name(&"a".repeat(63)));
    }

    #[test]
    fn suffixes_skip_taken_names_and_keep_the_length_limit() {
        let taken = ["sales", "sales_2"];
        assert_eq!(first_free_name("sales", |n| taken.contains(&n)), "sales_3");
        assert_eq!(first_free_name("fresh", |n| taken.contains(&n)), "fresh");
        let base = "b".repeat(63);
        let s = with_suffix(&base, 12);
        assert_eq!(s.len(), MAX_NAME_LEN);
        assert!(s.ends_with("_12"));
        assert_eq!(join_truncated("pg_main", "trino_b"), "pg_main_trino_b");
        assert_eq!(join_truncated(&"c".repeat(60), "engine").len(), MAX_NAME_LEN);
    }
}
```

- [ ] **Step 4: Run to see them fail**

Run: `cargo test -p egret-catalog-registry --lib name::`
Expected: FAIL — every test panics with `not yet implemented`.

- [ ] **Step 5: Implement the name functions** (replace the four `todo!()` bodies):

```rust
pub fn is_valid_catalog_name(name: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9_]{0,62}$").expect("static name pattern"))
        .is_match(name)
}

pub fn normalize_catalog_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c.is_ascii_lowercase()) {
        s = format!("w_{s}");
    }
    // Every char is ASCII here, so a byte truncate cannot split one.
    s.truncate(MAX_NAME_LEN);
    s
}

pub fn with_suffix(base: &str, n: u32) -> String {
    join_truncated(base, &n.to_string())
}

pub fn join_truncated(base: &str, tail: &str) -> String {
    let keep = MAX_NAME_LEN.saturating_sub(tail.len() + 1);
    let mut head = base.to_string();
    head.truncate(keep);
    format!("{head}_{tail}")
}

pub fn first_free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2u32..)
        .map(|n| with_suffix(base, n))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always yields a free name")
}
```

- [ ] **Step 6: Run the name tests**

Run: `cargo test -p egret-catalog-registry --lib name::`
Expected: 4 passed.

- [ ] **Step 7: `src/sensitive.rs`** (the SP1 key pattern, moved here so the registry and `engines.rs` share it; Task 7 points `engines.rs` at it):

```rust
//! Which property keys name secret material (spec SP1 §4.9), and which
//! key/value pairs are plaintext secrets that must not be stored in a catalog.

use std::sync::OnceLock;

/// Whether a property key names secret material.
pub fn is_sensitive_key(key: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)(password|secret|token|credential|private[-_.]?key|access[-_.]?key)",
            )
            .expect("static sensitive-key pattern compiles")
        })
        .is_match(key)
}

/// A value that is a switch or a number, not a secret — so
/// `iceberg.rest-catalog.vended-credentials-enabled=true` and
/// `iceberg.rest-catalog.oauth2.token-refresh-enabled=false` stay plain.
fn is_flag_or_number(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("false")
        || value.parse::<f64>().is_ok()
}

/// A non-empty, non-flag value under a sensitive key: it must live in a named
/// secret, never in `registry.catalogs`.
pub fn is_plaintext_secret(key: &str, value: &str) -> bool {
    is_sensitive_key(key) && !value.is_empty() && !is_flag_or_number(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_secrets_and_switches_are_not() {
        assert!(is_plaintext_secret("connection-password", "hunter2"));
        assert!(is_plaintext_secret("s3.aws-secret-key", "abc"));
        assert!(!is_plaintext_secret("connection-password", ""));
        assert!(!is_plaintext_secret("iceberg.rest-catalog.vended-credentials-enabled", "true"));
        assert!(!is_plaintext_secret("iceberg.rest-catalog.oauth2.token-refresh-enabled", "FALSE"));
        assert!(!is_plaintext_secret("connection-user", "reader"));
        assert!(is_sensitive_key("PRIVATE_KEY"));
    }
}
```

- [ ] **Step 8: Write the failing config tests** — create `src/config.rs` with the types below, `CatalogDef::parse`/`validate` bodies as `todo!()`, and this test module at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PW: &str = "11111111-1111-1111-1111-111111111111";
    const SC: &str = "22222222-2222-2222-2222-222222222222";
    const WH: &str = "33333333-3333-3333-3333-333333333333";

    fn parse(kind: &str, config: serde_json::Value) -> Result<CatalogDef, ValidationError> {
        CatalogDef::parse(kind, &config, &serde_json::Value::Null)
    }

    fn jdbc(dialect: &str, database: Option<&str>) -> serde_json::Value {
        let mut v = json!({
            "dialect": dialect, "host": "db.internal", "port": 5432,
            "user": "reader", "password_secret_id": PW, "ssl": { "mode": "require" }
        });
        if let Some(d) = database {
            v["database"] = json!(d);
        }
        v
    }

    #[test]
    fn every_kind_accepts_its_documented_shape() {
        parse("iceberg", json!({ "metastore": { "type": "egret", "warehouse_id": WH } })).unwrap();
        parse("iceberg", json!({
            "metastore": { "type": "rest", "uri": "https://cat.example/api", "warehouse": "lake",
                           "auth": { "type": "oauth2", "credential_secret_id": PW, "scope": "PRINCIPAL_ROLE:ALL" } },
            "storage_connection_id": SC
        })).unwrap();
        parse("iceberg", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC })).unwrap();
        parse("iceberg", json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC })).unwrap();
        parse("hive", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC })).unwrap();
        parse("delta_lake", json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKIA", "secret_key_secret_id": PW }, "storage_connection_id": SC })).unwrap();
        parse("hudi", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC })).unwrap();
        parse("jdbc", jdbc("postgresql", Some("sales"))).unwrap();
        parse("jdbc", jdbc("mysql", None)).unwrap();
        parse("jdbc", jdbc("clickhouse", None)).unwrap();
        parse("elasticsearch", json!({ "hosts": ["es1", "es2"], "port": 9200, "tls": true, "user": "elastic", "password_secret_id": PW })).unwrap();
        parse("tpch", json!({})).unwrap();
        parse("tpcds", json!({ "scale": 10 })).unwrap();
        CatalogDef::parse(
            "generic",
            &json!({ "engines": ["trino"], "secret_properties": { "trino": { "bigquery.credentials-key": PW } } }),
            &json!({ "trino": { "connector.name": "bigquery", "bigquery.project-id": "p" } }),
        )
        .unwrap();
    }

    #[test]
    fn unknown_kind_and_unknown_fields_are_refused() {
        assert_eq!(parse("mongo", json!({})), Err(ValidationError::UnknownKind("mongo".into())));
        let mut v = jdbc("postgresql", Some("sales"));
        v["bogus"] = json!(1);
        assert!(matches!(parse("jdbc", v), Err(ValidationError::Config { .. })));
        let err = parse("iceberg", json!({
            "metastore": { "type": "hive", "uri": "thrift://hms:9083", "extra": true },
            "storage_connection_id": SC
        }));
        assert!(matches!(err, Err(ValidationError::Config { .. })), "{err:?}");
    }

    #[test]
    fn missing_required_fields_are_refused() {
        let mut v = jdbc("postgresql", Some("sales"));
        v.as_object_mut().unwrap().remove("password_secret_id");
        assert!(parse("jdbc", v).is_err());
        assert!(parse("jdbc", jdbc("postgresql", None)).is_err(), "postgresql needs a database");
        assert!(parse("iceberg", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" } })).is_err(), "storage required");
        assert!(parse("hive", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" } })).is_err());
        assert!(parse("elasticsearch", json!({ "hosts": [], "port": 9200, "tls": false })).is_err());
    }

    #[test]
    fn a_plaintext_secret_cannot_be_stored() {
        let mut v = jdbc("postgresql", Some("sales"));
        v["password_secret_id"] = json!("hunter2");
        assert!(matches!(parse("jdbc", v), Err(ValidationError::Config { .. })));
        let err = CatalogDef::parse("tpch", &json!({}), &json!({ "trino": { "some.password": "hunter2" } }));
        assert_eq!(err, Err(ValidationError::PlaintextSecret { engine: "trino".into(), key: "some.password".into() }));
        CatalogDef::parse("tpch", &json!({}), &json!({ "trino": { "x.token-refresh-enabled": "true" } })).unwrap();
    }

    #[test]
    fn per_kind_rules() {
        assert!(parse("jdbc", jdbc("mysql", Some("shop"))).is_err(), "mysql exposes databases as schemas");
        let mut v = jdbc("postgresql", Some("sales"));
        v["ssl"] = json!({ "mode": "verify_full", "ca_secret_id": PW });
        assert!(parse("jdbc", v).is_err(), "custom CA not supported yet");
        let mut v = jdbc("postgresql", Some("sales"));
        v["host"] = json!("db.internal/x?y=1");
        assert!(parse("jdbc", v).is_err(), "host cannot smuggle URL parts");
        assert!(parse("hudi", json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC })).is_err());
        assert!(parse("iceberg", json!({ "metastore": { "type": "egret", "warehouse_id": WH }, "storage_connection_id": SC })).is_err());
        assert!(parse("tpch", json!({ "scale": 0 })).is_err());
        assert!(parse("elasticsearch", json!({ "hosts": ["es1"], "port": 9200, "tls": false, "password_secret_id": PW })).is_err(), "password needs a user");
        assert!(parse("iceberg", json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKIA" }, "storage_connection_id": SC })).is_err(), "access key without secret");
    }

    #[test]
    fn generic_rules() {
        let adv = json!({ "trino": { "connector.name": "bigquery" } });
        assert!(CatalogDef::parse("generic", &json!({ "engines": [] }), &adv).is_err());
        assert!(CatalogDef::parse("generic", &json!({ "engines": ["trino"] }), &json!({})).is_err(), "connector.name required");
        assert!(CatalogDef::parse("generic", &json!({ "engines": ["trino"] }), &json!({ "trino": { "connector.name": "x" }, "starrocks": { "type": "y" } })).is_err(), "starrocks keys without starrocks");
        let err = CatalogDef::parse(
            "generic",
            &json!({ "engines": ["trino"], "secret_properties": { "trino": { "bigquery.project-id": PW } } }),
            &json!({ "trino": { "connector.name": "bigquery", "bigquery.project-id": "p" } }),
        );
        assert!(matches!(err, Err(ValidationError::Collision { .. })), "{err:?}");
    }

    #[test]
    fn a_line_break_cannot_reach_a_properties_file() {
        let err = CatalogDef::parse("tpch", &json!({}), &json!({ "trino": { "a.b": "x\ny=z" } }));
        assert!(matches!(err, Err(ValidationError::Advanced(_))), "{err:?}");
    }

    #[test]
    fn derived_facts() {
        let d = parse("iceberg", json!({ "metastore": { "type": "egret", "warehouse_id": WH } })).unwrap();
        assert_eq!(d.warehouse_id(), Some(Uuid::parse_str(WH).unwrap()));
        assert_eq!(d, CatalogDef::egret_warehouse(Uuid::parse_str(WH).unwrap()));
        assert_eq!(d.trino_connector_name(), "iceberg");
        let d = parse("jdbc", jdbc("postgresql", Some("sales"))).unwrap();
        assert_eq!(d.secret_ids(), [Uuid::parse_str(PW).unwrap()].into_iter().collect());
        assert_eq!(d.trino_connector_name(), "postgresql");
        assert!(d.can_read(EngineType::Trino) && d.can_read(EngineType::StarRocks));
        let g = CatalogDef::parse("generic", &json!({ "engines": ["trino"] }), &json!({ "trino": { "connector.name": "bigquery" } })).unwrap();
        assert!(g.can_read(EngineType::Trino));
        assert!(!g.can_read(EngineType::StarRocks));
        assert_eq!(g.trino_connector_name(), "bigquery");
        // Round trip through the stored JSON.
        let again = CatalogDef::parse(g.kind().as_str(), &g.config_json(), &g.advanced_json()).unwrap();
        assert_eq!(again, g);
    }
}
```

- [ ] **Step 9: Run to see them fail**

Run: `cargo test -p egret-catalog-registry --lib config::`
Expected: FAIL (`not yet implemented`).

- [ ] **Step 10: Implement `src/config.rs`** (everything above the test module):

```rust
//! Typed catalog configuration (spec §4.2). `registry.catalogs` stores `kind`,
//! `config` and `advanced` as separate columns; [`CatalogDef::parse`] is the only
//! way in, and it validates. No field of any kind can hold a plaintext secret:
//! secrets are always `*_secret_id` UUIDs, and `advanced` refuses sensitive keys.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::sensitive::is_plaintext_secret;

/// The engines a catalog can be attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EngineType {
    #[serde(rename = "trino")]
    Trino,
    #[serde(rename = "starrocks")]
    StarRocks,
}

impl EngineType {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineType::Trino => "trino",
            EngineType::StarRocks => "starrocks",
        }
    }
    /// The `engine_mgmt.engines.type` value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "trino" => Some(EngineType::Trino),
            "starrocks" => Some(EngineType::StarRocks),
            _ => None,
        }
    }
}

/// `registry.catalogs.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogKind {
    Iceberg,
    Hive,
    DeltaLake,
    Hudi,
    Jdbc,
    Elasticsearch,
    Tpch,
    Tpcds,
    Generic,
}

impl CatalogKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CatalogKind::Iceberg => "iceberg",
            CatalogKind::Hive => "hive",
            CatalogKind::DeltaLake => "delta_lake",
            CatalogKind::Hudi => "hudi",
            CatalogKind::Jdbc => "jdbc",
            CatalogKind::Elasticsearch => "elasticsearch",
            CatalogKind::Tpch => "tpch",
            CatalogKind::Tpcds => "tpcds",
            CatalogKind::Generic => "generic",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "iceberg" => CatalogKind::Iceberg,
            "hive" => CatalogKind::Hive,
            "delta_lake" => CatalogKind::DeltaLake,
            "hudi" => CatalogKind::Hudi,
            "jdbc" => CatalogKind::Jdbc,
            "elasticsearch" => CatalogKind::Elasticsearch,
            "tpch" => CatalogKind::Tpch,
            "tpcds" => CatalogKind::Tpcds,
            "generic" => CatalogKind::Generic,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IcebergConfig {
    pub metastore: IcebergMetastore,
    /// Required unless `metastore` is `egret`, which uses its warehouse's storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_connection_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IcebergMetastore {
    /// egret's own warehouse; the catalog IS the warehouse.
    Egret { warehouse_id: Uuid },
    Rest {
        uri: String,
        warehouse: String,
        auth: RestAuth,
    },
    Hive {
        uri: String,
    },
    Glue {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_key_secret_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestAuth {
    #[serde(rename = "none")]
    NoAuth,
    Oauth2 {
        credential_secret_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_uri: Option<String>,
    },
}

/// `hive`, `delta_lake`, `hudi`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiveFamilyConfig {
    pub metastore: HiveMetastore,
    pub storage_connection_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HiveMetastore {
    Hive {
        uri: String,
    },
    Glue {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_key_secret_id: Option<Uuid>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JdbcDialect {
    Postgresql,
    Mysql,
    Mariadb,
    Oracle,
    Sqlserver,
    Clickhouse,
}

impl JdbcDialect {
    /// Also the Trino connector name.
    pub fn as_str(self) -> &'static str {
        match self {
            JdbcDialect::Postgresql => "postgresql",
            JdbcDialect::Mysql => "mysql",
            JdbcDialect::Mariadb => "mariadb",
            JdbcDialect::Oracle => "oracle",
            JdbcDialect::Sqlserver => "sqlserver",
            JdbcDialect::Clickhouse => "clickhouse",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcConfig {
    pub dialect: JdbcDialect,
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub user: String,
    pub password_secret_id: Uuid,
    pub ssl: JdbcSsl,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum JdbcSsl {
    Disable,
    Require,
    VerifyFull {
        /// Refused by validation until engines can mount a CA (see plan table #14).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_secret_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticsearchConfig {
    pub hosts: Vec<String>,
    pub port: u16,
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_secret_id: Option<Uuid>,
}

/// `tpch`, `tpcds`. Trino ignores `scale` (schema names carry it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
}

/// Secret-valued properties of a `generic` catalog, per engine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretProperties {
    #[serde(default)]
    pub trino: BTreeMap<String, Uuid>,
    #[serde(default)]
    pub starrocks: BTreeMap<String, Uuid>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericConfig {
    pub engines: BTreeSet<EngineType>,
    #[serde(default)]
    pub secret_properties: SecretProperties,
}

/// Free-form properties per engine, rendered after the typed keys (§4.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Advanced {
    #[serde(default)]
    pub trino: BTreeMap<String, String>,
    #[serde(default)]
    pub starrocks: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CatalogConfig {
    Iceberg(IcebergConfig),
    Hive(HiveFamilyConfig),
    DeltaLake(HiveFamilyConfig),
    Hudi(HiveFamilyConfig),
    Jdbc(JdbcConfig),
    Elasticsearch(ElasticsearchConfig),
    Tpch(SampleConfig),
    Tpcds(SampleConfig),
    Generic(GenericConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid catalog name {0:?}: use ^[a-z][a-z0-9_]{{0,62}}$")]
    Name(String),
    #[error("unknown catalog kind {0:?}")]
    UnknownKind(String),
    #[error("invalid {kind} configuration: {msg}")]
    Config { kind: String, msg: String },
    #[error("invalid advanced properties: {0}")]
    Advanced(String),
    #[error("advanced.{engine}.{key} looks like a secret; store it as a secret and reference its id")]
    PlaintextSecret { engine: String, key: String },
    #[error("advanced.{engine}.{key} collides with a property the {kind} form sets")]
    Collision {
        engine: String,
        key: String,
        kind: String,
    },
}

fn de<T: DeserializeOwned>(kind: CatalogKind, v: &Value) -> Result<T, ValidationError> {
    serde_json::from_value(v.clone()).map_err(|e| ValidationError::Config {
        kind: kind.as_str().to_string(),
        msg: e.to_string(),
    })
}

impl CatalogConfig {
    pub fn from_json(kind: CatalogKind, v: &Value) -> Result<Self, ValidationError> {
        Ok(match kind {
            CatalogKind::Iceberg => CatalogConfig::Iceberg(de(kind, v)?),
            CatalogKind::Hive => CatalogConfig::Hive(de(kind, v)?),
            CatalogKind::DeltaLake => CatalogConfig::DeltaLake(de(kind, v)?),
            CatalogKind::Hudi => CatalogConfig::Hudi(de(kind, v)?),
            CatalogKind::Jdbc => CatalogConfig::Jdbc(de(kind, v)?),
            CatalogKind::Elasticsearch => CatalogConfig::Elasticsearch(de(kind, v)?),
            CatalogKind::Tpch => CatalogConfig::Tpch(de(kind, v)?),
            CatalogKind::Tpcds => CatalogConfig::Tpcds(de(kind, v)?),
            CatalogKind::Generic => CatalogConfig::Generic(de(kind, v)?),
        })
    }

    pub fn kind(&self) -> CatalogKind {
        match self {
            CatalogConfig::Iceberg(_) => CatalogKind::Iceberg,
            CatalogConfig::Hive(_) => CatalogKind::Hive,
            CatalogConfig::DeltaLake(_) => CatalogKind::DeltaLake,
            CatalogConfig::Hudi(_) => CatalogKind::Hudi,
            CatalogConfig::Jdbc(_) => CatalogKind::Jdbc,
            CatalogConfig::Elasticsearch(_) => CatalogKind::Elasticsearch,
            CatalogConfig::Tpch(_) => CatalogKind::Tpch,
            CatalogConfig::Tpcds(_) => CatalogKind::Tpcds,
            CatalogConfig::Generic(_) => CatalogKind::Generic,
        }
    }

    pub fn to_json(&self) -> Value {
        let v = match self {
            CatalogConfig::Iceberg(c) => serde_json::to_value(c),
            CatalogConfig::Hive(c) | CatalogConfig::DeltaLake(c) | CatalogConfig::Hudi(c) => {
                serde_json::to_value(c)
            }
            CatalogConfig::Jdbc(c) => serde_json::to_value(c),
            CatalogConfig::Elasticsearch(c) => serde_json::to_value(c),
            CatalogConfig::Tpch(c) | CatalogConfig::Tpcds(c) => serde_json::to_value(c),
            CatalogConfig::Generic(c) => serde_json::to_value(c),
        };
        v.expect("typed catalog config always serializes")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogDef {
    pub config: CatalogConfig,
    pub advanced: Advanced,
}

fn regex(cell: &'static OnceLock<regex::Regex>, pattern: &str) -> &'static regex::Regex {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("static pattern compiles"))
}

fn text(field: &str, v: &str) -> Result<(), String> {
    if v.trim().is_empty() {
        return Err(format!("{field} is required"));
    }
    if v.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(())
}

fn http_uri(field: &str, v: &str) -> Result<(), String> {
    text(field, v)?;
    if !(v.starts_with("http://") || v.starts_with("https://")) || v.contains(char::is_whitespace)
    {
        return Err(format!("{field} must be an http(s) URL"));
    }
    Ok(())
}

fn thrift_uri(field: &str, v: &str) -> Result<(), String> {
    text(field, v)?;
    if v.split(',').any(|u| !u.trim().starts_with("thrift://")) || v.contains(char::is_whitespace)
    {
        return Err(format!("{field} must be thrift://host:port (comma-separated)"));
    }
    Ok(())
}

fn host(field: &str, v: &str) -> Result<(), String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if !regex(&RE, r"^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$").is_match(v) {
        return Err(format!("{field} must be a host name or address"));
    }
    Ok(())
}

fn database_name(v: &str) -> Result<(), String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if !regex(&RE, r"^[A-Za-z0-9_$.-]+$").is_match(v) {
        return Err("database may contain letters, digits, _ $ . - only".into());
    }
    Ok(())
}

fn property_key(v: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    regex(&RE, r"^[A-Za-z0-9][A-Za-z0-9._-]*$").is_match(v)
}

fn glue(region: &str, access_key: &Option<String>, secret: &Option<Uuid>) -> Result<(), String> {
    text("region", region)?;
    if let Some(a) = access_key {
        text("access_key", a)?;
    }
    if access_key.is_some() != secret.is_some() {
        return Err("access_key and secret_key_secret_id go together".into());
    }
    Ok(())
}

fn validate_config(c: &CatalogConfig) -> Result<(), ValidationError> {
    let kind = c.kind();
    let bad = |msg: String| ValidationError::Config {
        kind: kind.as_str().to_string(),
        msg,
    };
    match c {
        CatalogConfig::Iceberg(i) => {
            let egret = matches!(i.metastore, IcebergMetastore::Egret { .. });
            if egret && i.storage_connection_id.is_some() {
                return Err(bad(
                    "an egret warehouse catalog uses its warehouse's storage; remove storage_connection_id".into(),
                ));
            }
            if !egret && i.storage_connection_id.is_none() {
                return Err(bad("storage_connection_id is required".into()));
            }
            match &i.metastore {
                IcebergMetastore::Egret { .. } => {}
                IcebergMetastore::Rest {
                    uri,
                    warehouse,
                    auth,
                } => {
                    http_uri("uri", uri).map_err(bad)?;
                    text("warehouse", warehouse).map_err(bad)?;
                    if let RestAuth::Oauth2 {
                        scope, token_uri, ..
                    } = auth
                    {
                        if let Some(s) = scope {
                            text("scope", s).map_err(bad)?;
                        }
                        if let Some(t) = token_uri {
                            http_uri("token_uri", t).map_err(bad)?;
                        }
                    }
                }
                IcebergMetastore::Hive { uri } => thrift_uri("uri", uri).map_err(bad)?,
                IcebergMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => glue(region, access_key, secret_key_secret_id).map_err(bad)?,
            }
        }
        CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
            match &h.metastore {
                HiveMetastore::Hive { uri } => thrift_uri("uri", uri).map_err(bad)?,
                HiveMetastore::Glue { .. } if kind == CatalogKind::Hudi => {
                    return Err(bad("Hudi reads a Hive metastore only".into()));
                }
                HiveMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => glue(region, access_key, secret_key_secret_id).map_err(bad)?,
            }
        }
        CatalogConfig::Jdbc(j) => {
            host("host", &j.host).map_err(bad)?;
            if j.port == 0 {
                return Err(bad("port must be 1-65535".into()));
            }
            text("user", &j.user).map_err(bad)?;
            match (j.dialect, &j.database) {
                (JdbcDialect::Postgresql | JdbcDialect::Oracle | JdbcDialect::Sqlserver, None) => {
                    return Err(bad(format!("database is required for {}", j.dialect.as_str())));
                }
                (JdbcDialect::Mysql | JdbcDialect::Mariadb, Some(_)) => {
                    return Err(bad(format!(
                        "{} catalogs expose every database as a schema; leave database empty",
                        j.dialect.as_str()
                    )));
                }
                (_, Some(d)) => database_name(d).map_err(bad)?,
                (_, None) => {}
            }
            if let JdbcSsl::VerifyFull {
                ca_secret_id: Some(_),
            } = j.ssl
            {
                return Err(bad(
                    "a custom CA certificate is not supported yet; verify_full uses the engine's default trust store".into(),
                ));
            }
        }
        CatalogConfig::Elasticsearch(e) => {
            if e.hosts.is_empty() {
                return Err(bad("hosts must list at least one host".into()));
            }
            for h in &e.hosts {
                host("hosts[]", h).map_err(bad)?;
            }
            if e.port == 0 {
                return Err(bad("port must be 1-65535".into()));
            }
            if let Some(u) = &e.user {
                text("user", u).map_err(bad)?;
            }
            if e.user.is_some() != e.password_secret_id.is_some() {
                return Err(bad("user and password_secret_id go together".into()));
            }
        }
        CatalogConfig::Tpch(s) | CatalogConfig::Tpcds(s) => {
            if s.scale == Some(0) {
                return Err(bad("scale must be positive".into()));
            }
        }
        CatalogConfig::Generic(g) => {
            if g.engines.is_empty() {
                return Err(bad("engines must name trino, starrocks or both".into()));
            }
        }
    }
    Ok(())
}

fn validate_advanced(def: &CatalogDef) -> Result<(), ValidationError> {
    for (engine, map) in [
        (EngineType::Trino, &def.advanced.trino),
        (EngineType::StarRocks, &def.advanced.starrocks),
    ] {
        for (k, v) in map {
            if !property_key(k) {
                return Err(ValidationError::Advanced(format!(
                    "{}: invalid property key {k:?}",
                    engine.as_str()
                )));
            }
            if v.chars().any(char::is_control) {
                return Err(ValidationError::Advanced(format!(
                    "{}.{k}: value contains a control character",
                    engine.as_str()
                )));
            }
            if is_plaintext_secret(k, v) {
                return Err(ValidationError::PlaintextSecret {
                    engine: engine.as_str().to_string(),
                    key: k.clone(),
                });
            }
        }
    }
    if let CatalogConfig::Generic(g) = &def.config {
        for (engine, required, adv, secrets) in [
            (EngineType::Trino, "connector.name", &def.advanced.trino, &g.secret_properties.trino),
            (EngineType::StarRocks, "type", &def.advanced.starrocks, &g.secret_properties.starrocks),
        ] {
            let e = engine.as_str();
            if g.engines.contains(&engine) {
                if adv.get(required).is_none_or(|v| v.trim().is_empty()) {
                    return Err(ValidationError::Config {
                        kind: "generic".into(),
                        msg: format!("advanced.{e}.{required} is required"),
                    });
                }
            } else if !adv.is_empty() || !secrets.is_empty() {
                return Err(ValidationError::Config {
                    kind: "generic".into(),
                    msg: format!("advanced.{e} or secret_properties.{e} is set but engines does not include {e}"),
                });
            }
            for k in secrets.keys() {
                if !property_key(k) {
                    return Err(ValidationError::Advanced(format!("{e}: invalid secret property key {k:?}")));
                }
                if adv.contains_key(k) {
                    return Err(ValidationError::Collision {
                        engine: e.to_string(),
                        key: k.clone(),
                        kind: "generic".into(),
                    });
                }
            }
        }
    }
    Ok(())
}

impl CatalogDef {
    /// Parse and validate the three stored columns.
    pub fn parse(kind: &str, config: &Value, advanced: &Value) -> Result<Self, ValidationError> {
        let kind = CatalogKind::parse(kind)
            .ok_or_else(|| ValidationError::UnknownKind(kind.to_string()))?;
        let config = CatalogConfig::from_json(kind, config)?;
        let advanced = if advanced.is_null() {
            Advanced::default()
        } else {
            serde_json::from_value(advanced.clone())
                .map_err(|e| ValidationError::Advanced(e.to_string()))?
        };
        let def = CatalogDef { config, advanced };
        def.validate()?;
        Ok(def)
    }

    /// The catalog of an egret warehouse.
    pub fn egret_warehouse(warehouse_id: Uuid) -> Self {
        CatalogDef {
            config: CatalogConfig::Iceberg(IcebergConfig {
                metastore: IcebergMetastore::Egret { warehouse_id },
                storage_connection_id: None,
            }),
            advanced: Advanced::default(),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_config(&self.config)?;
        validate_advanced(self)?;
        Ok(())
    }

    pub fn kind(&self) -> CatalogKind {
        self.config.kind()
    }

    pub fn config_json(&self) -> Value {
        self.config.to_json()
    }

    pub fn advanced_json(&self) -> Value {
        serde_json::to_value(&self.advanced).expect("advanced always serializes")
    }

    /// The warehouse an egret catalog is (mirrored into `registry.catalogs.warehouse_id`).
    pub fn warehouse_id(&self) -> Option<Uuid> {
        match &self.config {
            CatalogConfig::Iceberg(IcebergConfig {
                metastore: IcebergMetastore::Egret { warehouse_id },
                ..
            }) => Some(*warehouse_id),
            _ => None,
        }
    }

    /// Mirrored into `registry.catalogs.storage_connection_id`.
    pub fn storage_connection_id(&self) -> Option<Uuid> {
        match &self.config {
            CatalogConfig::Iceberg(i) => i.storage_connection_id,
            CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
                Some(h.storage_connection_id)
            }
            _ => None,
        }
    }

    /// Every secret the configuration references (mirrored into
    /// `registry.catalog_secret_refs`, which blocks deleting them).
    pub fn secret_ids(&self) -> BTreeSet<Uuid> {
        let mut out = BTreeSet::new();
        let glue_secret = |out: &mut BTreeSet<Uuid>, s: &Option<Uuid>| {
            if let Some(id) = s {
                out.insert(*id);
            }
        };
        match &self.config {
            CatalogConfig::Iceberg(i) => match &i.metastore {
                IcebergMetastore::Rest {
                    auth: RestAuth::Oauth2 { credential_secret_id, .. },
                    ..
                } => {
                    out.insert(*credential_secret_id);
                }
                IcebergMetastore::Glue { secret_key_secret_id, .. } => {
                    glue_secret(&mut out, secret_key_secret_id)
                }
                _ => {}
            },
            CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
                if let HiveMetastore::Glue { secret_key_secret_id, .. } = &h.metastore {
                    glue_secret(&mut out, secret_key_secret_id)
                }
            }
            CatalogConfig::Jdbc(j) => {
                out.insert(j.password_secret_id);
            }
            CatalogConfig::Elasticsearch(e) => glue_secret(&mut out, &e.password_secret_id),
            CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) => {}
            CatalogConfig::Generic(g) => {
                out.extend(g.secret_properties.trino.values().copied());
                out.extend(g.secret_properties.starrocks.values().copied());
            }
        }
        out
    }

    /// Whether an engine type can read this catalog (spec §3: every typed kind is
    /// readable by both engines; `generic` only by the engines it names).
    pub fn can_read(&self, engine: EngineType) -> bool {
        match &self.config {
            CatalogConfig::Generic(g) => g.engines.contains(&engine),
            _ => true,
        }
    }

    /// The Trino connector this catalog renders to — also the `kind` the legacy
    /// engine-catalog endpoints report for a non-warehouse catalog.
    pub fn trino_connector_name(&self) -> String {
        match &self.config {
            CatalogConfig::Iceberg(_) => "iceberg".into(),
            CatalogConfig::Hive(_) => "hive".into(),
            CatalogConfig::DeltaLake(_) => "delta_lake".into(),
            CatalogConfig::Hudi(_) => "hudi".into(),
            CatalogConfig::Jdbc(j) => j.dialect.as_str().into(),
            CatalogConfig::Elasticsearch(_) => "elasticsearch".into(),
            CatalogConfig::Tpch(_) => "tpch".into(),
            CatalogConfig::Tpcds(_) => "tpcds".into(),
            CatalogConfig::Generic(_) => self
                .advanced
                .trino
                .get("connector.name")
                .cloned()
                .unwrap_or_else(|| "generic".into()),
        }
    }
}
```

`Option::is_none_or` is stable since Rust 1.82; the workspace pins `rust-version = "1.85"` and `rust-toolchain.toml` tracks `stable`, so it is available.

- [ ] **Step 11: Run the crate tests**

Run: `cargo test -p egret-catalog-registry --lib`
Expected: all tests in `name`, `sensitive`, `config` pass. If `unknown_kind_and_unknown_fields_are_refused` passes for the top-level JDBC field but not the nested `metastore` one, serde is not applying `deny_unknown_fields` to the internally tagged enum: stop and report — do not weaken the test.

- [ ] **Step 12: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry --all-targets -- -D warnings
git branch --show-current
git add Cargo.toml Cargo.lock crates/egret-catalog-registry
git commit -m "feat(catalog-registry): typed catalog configuration and validation

New egret-catalog-registry crate: the SQL-identifier name rule and the
normalisation the migration uses, per-kind typed configuration for the
four v1 families plus generic, and validation that refuses unknown
fields, missing fields, plaintext secrets and control characters.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Trino rendering

**Files:**
- Create: `crates/egret-catalog-registry/src/render/mod.rs`, `src/render/trino.rs`
- Modify: `crates/egret-catalog-registry/src/lib.rs`, `src/config.rs` (`CatalogDef::validate` gains the collision probe)

**Interfaces:**
- Consumes: Task 1 `CatalogDef`, `CatalogConfig`, `EngineType`, `JdbcConfig`, `JdbcSsl`, `ValidationError`.
- Produces: `render::trino::{render_trino, jdbc_url, TrinoRenderInput, WarehouseRender, StorageRender, TrinoCatalogFile, RenderError}`, re-exported from the crate root. `render_trino(name: &str, def: &CatalogDef, input: &TrinoRenderInput<'_>) -> Result<TrinoCatalogFile, RenderError>`; `TrinoCatalogFile { name: String, properties: BTreeMap<String, String>, env: BTreeMap<String, Uuid> }` with `split_secret_properties(&self) -> (BTreeMap<String, String>, BTreeMap<String, Uuid>)`; `TrinoRenderInput::probe() -> TrinoRenderInput<'static>`.

Secret placeholders are `${ENV:CAT_<prefix>_<i>}` with `i` counting from 0 in render order; the egret-warehouse credential keeps today's `${ENV:EGRET_<prefix>}` so an upgraded engine renders a byte-identical catalog file (no pod roll just for the migration).

- [ ] **Step 1: Wire the module** — `src/render/mod.rs`:

```rust
//! Per-engine rendering (spec §4.3). SP3a ships Trino; SP3b adds StarRocks next
//! to it, and SP5 egret's own connection.

pub mod trino;
```

In `src/lib.rs` add `pub mod render;` to the module list and:

```rust
pub use render::trino::{
    jdbc_url, render_trino, RenderError, StorageRender, TrinoCatalogFile, TrinoRenderInput,
    WarehouseRender,
};
```

- [ ] **Step 2: Write the failing render tests** — `src/render/trino.rs` with the types, `render_trino`/`jdbc_url` bodies `todo!()`, and:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CatalogDef;
    use serde_json::json;

    const PW: &str = "11111111-1111-1111-1111-111111111111";
    const SC: &str = "22222222-2222-2222-2222-222222222222";
    const WH: &str = "33333333-3333-3333-3333-333333333333";
    const S3: &str = "44444444-4444-4444-4444-444444444444";
    const CRED: &str = "55555555-5555-5555-5555-555555555555";

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn input() -> TrinoRenderInput<'static> {
        TrinoRenderInput {
            env_prefix: "C0",
            egret_url: "http://egret:8080",
            warehouse: Some(WarehouseRender { name: "wh-a", s3_endpoint: "http://minio:9000" }),
            storage: Some(StorageRender {
                endpoint: Some("http://minio:9000"),
                region: "eu-west-1",
                path_style: true,
                access_key: "AKIA",
                secret_key_secret_id: Some(id(S3)),
            }),
            credential_secret_id: Some(id(CRED)),
        }
    }

    fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn def(kind: &str, config: serde_json::Value, advanced: serde_json::Value) -> CatalogDef {
        CatalogDef::parse(kind, &config, &advanced).unwrap()
    }

    const S3_PROPS: [(&str, &str); 6] = [
        ("fs.native-s3.enabled", "true"),
        ("s3.endpoint", "http://minio:9000"),
        ("s3.region", "eu-west-1"),
        ("s3.path-style-access", "true"),
        ("s3.aws-access-key", "AKIA"),
        ("s3.aws-secret-key", "${ENV:CAT_C0_0}"),
    ];

    /// Parity with the `egret-warehouse` arm `build_catalog_req` had before SP3a.
    #[test]
    fn an_egret_warehouse_renders_exactly_as_before_the_registry() {
        let f = render_trino("egret", &CatalogDef::egret_warehouse(id(WH)), &input()).unwrap();
        assert_eq!(f.name, "egret");
        assert_eq!(f.properties, props(&[
            ("connector.name", "iceberg"),
            ("iceberg.catalog.type", "rest"),
            ("iceberg.rest-catalog.uri", "http://egret:8080/catalog"),
            ("iceberg.rest-catalog.warehouse", "wh-a"),
            ("iceberg.rest-catalog.security", "OAUTH2"),
            ("iceberg.rest-catalog.oauth2.server-uri", "http://egret:8080/oauth/token"),
            ("iceberg.rest-catalog.oauth2.credential", "${ENV:EGRET_C0}"),
            ("iceberg.rest-catalog.vended-credentials-enabled", "true"),
            ("fs.native-s3.enabled", "true"),
            ("s3.endpoint", "http://minio:9000"),
            ("s3.region", "us-east-1"),
            ("s3.path-style-access", "true"),
        ]));
        assert_eq!(f.env, [("EGRET_C0".to_string(), id(CRED))].into_iter().collect());
    }

    #[test]
    fn iceberg_rest_with_oauth2_and_s3() {
        let d = def("iceberg", json!({
            "metastore": { "type": "rest", "uri": "https://cat/api", "warehouse": "lake",
                           "auth": { "type": "oauth2", "credential_secret_id": PW, "scope": "ALL", "token_uri": "https://idp/token" } },
            "storage_connection_id": SC
        }), json!(null));
        let f = render_trino("lake", &d, &input()).unwrap();
        let mut want = props(&[
            ("connector.name", "iceberg"),
            ("iceberg.catalog.type", "rest"),
            ("iceberg.rest-catalog.uri", "https://cat/api"),
            ("iceberg.rest-catalog.warehouse", "lake"),
            ("iceberg.rest-catalog.security", "OAUTH2"),
            ("iceberg.rest-catalog.oauth2.credential", "${ENV:CAT_C0_0}"),
            ("iceberg.rest-catalog.oauth2.scope", "ALL"),
            ("iceberg.rest-catalog.oauth2.server-uri", "https://idp/token"),
        ]);
        want.extend(props(&S3_PROPS[..5]));
        want.insert("s3.aws-secret-key".into(), "${ENV:CAT_C0_1}".into());
        assert_eq!(f.properties, want);
        assert_eq!(f.env, [("CAT_C0_0".to_string(), id(PW)), ("CAT_C0_1".to_string(), id(S3))].into_iter().collect());
    }

    #[test]
    fn iceberg_hive_and_glue_metastores() {
        let d = def("iceberg", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }), json!(null));
        let mut want = props(&[("connector.name", "iceberg"), ("iceberg.catalog.type", "hive_metastore"), ("hive.metastore.uri", "thrift://hms:9083")]);
        want.extend(props(&S3_PROPS));
        assert_eq!(render_trino("h", &d, &input()).unwrap().properties, want);

        let d = def("iceberg", json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKG", "secret_key_secret_id": PW }, "storage_connection_id": SC }), json!(null));
        let f = render_trino("g", &d, &input()).unwrap();
        assert_eq!(f.properties["iceberg.catalog.type"], "glue");
        assert_eq!(f.properties["hive.metastore.glue.region"], "eu-west-1");
        assert_eq!(f.properties["hive.metastore.glue.aws-access-key"], "AKG");
        assert_eq!(f.properties["hive.metastore.glue.aws-secret-key"], "${ENV:CAT_C0_0}");
        assert_eq!(f.properties["s3.aws-secret-key"], "${ENV:CAT_C0_1}");
    }

    #[test]
    fn hive_family() {
        let d = def("hive", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }), json!(null));
        let mut want = props(&[("connector.name", "hive"), ("hive.metastore.uri", "thrift://hms:9083")]);
        want.extend(props(&S3_PROPS));
        assert_eq!(render_trino("h", &d, &input()).unwrap().properties, want);

        let d = def("delta_lake", json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC }), json!(null));
        let f = render_trino("d", &d, &input()).unwrap();
        assert_eq!(f.properties["connector.name"], "delta_lake");
        assert_eq!(f.properties["hive.metastore"], "glue");
        assert_eq!(f.properties["hive.metastore.glue.region"], "eu-west-1");

        let d = def("hudi", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }), json!(null));
        assert_eq!(render_trino("u", &d, &input()).unwrap().properties["connector.name"], "hudi");
    }

    #[test]
    fn jdbc_urls_for_every_dialect_and_ssl_mode() {
        let cases = [
            ("postgresql", Some("sales"), "disable", "jdbc:postgresql://db:5432/sales?sslmode=disable"),
            ("postgresql", Some("sales"), "require", "jdbc:postgresql://db:5432/sales?sslmode=require"),
            ("postgresql", Some("sales"), "verify_full", "jdbc:postgresql://db:5432/sales?sslmode=verify-full"),
            ("mysql", None, "disable", "jdbc:mysql://db:5432?sslMode=DISABLED"),
            ("mysql", None, "require", "jdbc:mysql://db:5432?sslMode=REQUIRED"),
            ("mysql", None, "verify_full", "jdbc:mysql://db:5432?sslMode=VERIFY_IDENTITY"),
            ("mariadb", None, "disable", "jdbc:mariadb://db:5432?sslMode=disable"),
            ("mariadb", None, "require", "jdbc:mariadb://db:5432?sslMode=trust"),
            ("mariadb", None, "verify_full", "jdbc:mariadb://db:5432?sslMode=verify-full"),
            ("oracle", Some("ORCL"), "disable", "jdbc:oracle:thin:@//db:5432/ORCL"),
            ("oracle", Some("ORCL"), "require", "jdbc:oracle:thin:@tcps://db:5432/ORCL"),
            ("oracle", Some("ORCL"), "verify_full", "jdbc:oracle:thin:@tcps://db:5432/ORCL?oracle.net.ssl_server_dn_match=true"),
            ("sqlserver", Some("sales"), "disable", "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=false"),
            ("sqlserver", Some("sales"), "require", "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=true;trustServerCertificate=true"),
            ("sqlserver", Some("sales"), "verify_full", "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=true;trustServerCertificate=false"),
            ("clickhouse", Some("events"), "disable", "jdbc:clickhouse://db:5432/events?ssl=false"),
            ("clickhouse", None, "require", "jdbc:clickhouse://db:5432/?ssl=true&sslmode=none"),
            ("clickhouse", Some("events"), "verify_full", "jdbc:clickhouse://db:5432/events?ssl=true&sslmode=strict"),
        ];
        for (dialect, database, mode, url) in cases {
            let mut cfg = json!({ "dialect": dialect, "host": "db", "port": 5432, "user": "u", "password_secret_id": PW, "ssl": { "mode": mode } });
            if let Some(d) = database {
                cfg["database"] = json!(d);
            }
            let d = def("jdbc", cfg, json!(null));
            let f = render_trino("j", &d, &input()).unwrap();
            assert_eq!(f.properties, props(&[
                ("connector.name", dialect),
                ("connection-url", url),
                ("connection-user", "u"),
                ("connection-password", "${ENV:CAT_C0_0}"),
            ]), "{dialect}/{mode}");
            assert_eq!(f.env, [("CAT_C0_0".to_string(), id(PW))].into_iter().collect());
        }
    }

    #[test]
    fn elasticsearch_with_and_without_a_password() {
        let d = def("elasticsearch", json!({ "hosts": ["es1", "es2"], "port": 9200, "tls": true, "user": "elastic", "password_secret_id": PW }), json!(null));
        assert_eq!(render_trino("es", &d, &input()).unwrap().properties, props(&[
            ("connector.name", "elasticsearch"),
            ("elasticsearch.host", "es1,es2"),
            ("elasticsearch.port", "9200"),
            ("elasticsearch.tls.enabled", "true"),
            ("elasticsearch.security", "PASSWORD"),
            ("elasticsearch.auth.user", "elastic"),
            ("elasticsearch.auth.password", "${ENV:CAT_C0_0}"),
        ]));
        let d = def("elasticsearch", json!({ "hosts": ["es1"], "port": 9200, "tls": false }), json!(null));
        assert_eq!(render_trino("es", &d, &input()).unwrap().properties, props(&[
            ("connector.name", "elasticsearch"),
            ("elasticsearch.host", "es1"),
            ("elasticsearch.port", "9200"),
            ("elasticsearch.tls.enabled", "false"),
        ]));
    }

    #[test]
    fn samples_generic_and_advanced_merge() {
        let d = def("tpch", json!({ "scale": 10 }), json!({ "trino": { "tpch.column-naming": "STANDARD" } }));
        assert_eq!(render_trino("t", &d, &input()).unwrap().properties, props(&[("connector.name", "tpch"), ("tpch.column-naming", "STANDARD")]));
        let d = def("tpcds", json!({}), json!(null));
        assert_eq!(render_trino("t", &d, &input()).unwrap().properties, props(&[("connector.name", "tpcds")]));

        let d = def("generic",
            json!({ "engines": ["trino"], "secret_properties": { "trino": { "bigquery.credentials-key": PW } } }),
            json!({ "trino": { "connector.name": "bigquery", "bigquery.project-id": "p" } }));
        let f = render_trino("bq", &d, &input()).unwrap();
        assert_eq!(f.properties, props(&[
            ("connector.name", "bigquery"),
            ("bigquery.project-id", "p"),
            ("bigquery.credentials-key", "${ENV:CAT_C0_0}"),
        ]));
        let (plain, secret) = f.split_secret_properties();
        assert_eq!(plain, props(&[("connector.name", "bigquery"), ("bigquery.project-id", "p")]));
        assert_eq!(secret, [("bigquery.credentials-key".to_string(), id(PW))].into_iter().collect());
    }

    #[test]
    fn refusals() {
        // An advanced key the typed form sets is refused at save time.
        let err = CatalogDef::parse("tpch", &json!({}), &json!({ "trino": { "connector.name": "memory" } }));
        assert!(matches!(err, Err(crate::ValidationError::Collision { .. })), "{err:?}");
        let err = CatalogDef::parse("jdbc",
            &json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "d", "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            &json!({ "trino": { "connection-url": "jdbc:postgresql://evil/x" } }));
        assert!(matches!(err, Err(crate::ValidationError::Collision { .. })), "{err:?}");
        // StarRocks-only generic cannot render for Trino.
        let d = def("generic", json!({ "engines": ["starrocks"] }), json!({ "starrocks": { "type": "paimon" } }));
        assert_eq!(render_trino("p", &d, &input()), Err(RenderError::NotReadable("p".into())));
        // Missing inputs.
        let mut i = input();
        i.storage = None;
        let d = def("hive", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }), json!(null));
        assert_eq!(render_trino("h", &d, &i), Err(RenderError::MissingInput("storage connection")));
        let mut i = input();
        i.credential_secret_id = None;
        assert_eq!(render_trino("e", &CatalogDef::egret_warehouse(id(WH)), &i), Err(RenderError::MissingInput("catalog client credential")));
    }
}
```

- [ ] **Step 3: Run to see them fail**

Run: `cargo test -p egret-catalog-registry --lib render::`
Expected: FAIL (`not yet implemented`).

- [ ] **Step 4: Implement `src/render/trino.rs`** (above the tests):

```rust
//! Trino rendering: a catalog definition + resolved inputs → one
//! `<name>.properties` file and the env entries its secret placeholders read.
//! Pure: secrets stay ids; the caller resolves them to plaintext at apply time.

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::config::{
    CatalogConfig, CatalogDef, EngineType, HiveMetastore, IcebergMetastore, JdbcConfig,
    JdbcDialect, JdbcSsl, RestAuth,
};

/// The warehouse behind an egret catalog.
#[derive(Clone, Copy, Debug)]
pub struct WarehouseRender<'a> {
    pub name: &'a str,
    pub s3_endpoint: &'a str,
}

/// A resolved storage connection (secret still an id).
#[derive(Clone, Copy, Debug)]
pub struct StorageRender<'a> {
    pub endpoint: Option<&'a str>,
    pub region: &'a str,
    pub path_style: bool,
    pub access_key: &'a str,
    pub secret_key_secret_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug)]
pub struct TrinoRenderInput<'a> {
    /// Namespaces this catalog's env vars (`C0`, `C1`, … by attachment order).
    pub env_prefix: &'a str,
    /// In-cluster egret URL (REST catalog + token endpoint).
    pub egret_url: &'a str,
    pub warehouse: Option<WarehouseRender<'a>>,
    pub storage: Option<StorageRender<'a>>,
    /// The attachment's minted Catalog Client credential (egret catalogs only).
    pub credential_secret_id: Option<Uuid>,
}

impl TrinoRenderInput<'static> {
    /// Inputs that satisfy every kind: rendering with these tells which keys the
    /// typed form sets, which is what the save-time collision check needs.
    pub fn probe() -> Self {
        TrinoRenderInput {
            env_prefix: "P",
            egret_url: "http://probe",
            warehouse: Some(WarehouseRender {
                name: "probe",
                s3_endpoint: "http://probe",
            }),
            storage: Some(StorageRender {
                endpoint: Some("http://probe"),
                region: "probe",
                path_style: true,
                access_key: "probe",
                secret_key_secret_id: Some(Uuid::nil()),
            }),
            credential_secret_id: Some(Uuid::nil()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrinoCatalogFile {
    pub name: String,
    pub properties: BTreeMap<String, String>,
    /// env var → secret id, for every `${ENV:…}` placeholder in `properties`.
    pub env: BTreeMap<String, Uuid>,
}

impl TrinoCatalogFile {
    /// (plaintext properties, property → secret id).
    pub fn split_secret_properties(&self) -> (BTreeMap<String, String>, BTreeMap<String, Uuid>) {
        let mut plain = BTreeMap::new();
        let mut secret = BTreeMap::new();
        for (k, v) in &self.properties {
            let var = v.strip_prefix("${ENV:").and_then(|r| r.strip_suffix('}'));
            match var.and_then(|var| self.env.get(var)) {
                Some(id) => {
                    secret.insert(k.clone(), *id);
                }
                None => {
                    plain.insert(k.clone(), v.clone());
                }
            }
        }
        (plain, secret)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("catalog {0} cannot be read by Trino")]
    NotReadable(String),
    #[error("missing render input: {0}")]
    MissingInput(&'static str),
    #[error("advanced.trino key {0:?} collides with a property the catalog form sets")]
    Collision(String),
    #[error("property {0:?} has a value with a line break")]
    LineBreak(String),
}

struct Out<'a> {
    props: BTreeMap<String, String>,
    env: BTreeMap<String, Uuid>,
    prefix: &'a str,
    next: usize,
}

impl Out<'_> {
    fn set(&mut self, k: &str, v: impl Into<String>) {
        self.props.insert(k.to_string(), v.into());
    }

    fn secret(&mut self, k: &str, id: Uuid) {
        let var = format!("CAT_{}_{}", self.prefix, self.next);
        self.next += 1;
        self.props.insert(k.to_string(), format!("${{ENV:{var}}}"));
        self.env.insert(var, id);
    }

    fn s3(&mut self, input: &TrinoRenderInput<'_>) -> Result<(), RenderError> {
        let s = input
            .storage
            .as_ref()
            .ok_or(RenderError::MissingInput("storage connection"))?;
        self.set("fs.native-s3.enabled", "true");
        if let Some(ep) = s.endpoint.filter(|e| !e.is_empty()) {
            self.set("s3.endpoint", ep);
        }
        self.set("s3.region", s.region);
        self.set("s3.path-style-access", if s.path_style { "true" } else { "false" });
        self.set("s3.aws-access-key", s.access_key);
        if let Some(id) = s.secret_key_secret_id {
            self.secret("s3.aws-secret-key", id);
        }
        Ok(())
    }

    fn glue(&mut self, region: &str, access_key: Option<&str>, secret: Option<Uuid>) {
        self.set("hive.metastore.glue.region", region);
        if let Some(a) = access_key {
            self.set("hive.metastore.glue.aws-access-key", a);
        }
        if let Some(id) = secret {
            self.secret("hive.metastore.glue.aws-secret-key", id);
        }
    }
}

/// The JDBC URL Trino's connector for `c.dialect` gets. Every form states its TLS
/// mode explicitly — driver defaults (`prefer`, `PREFERRED`) are never implied.
pub fn jdbc_url(c: &JdbcConfig) -> String {
    let (h, p) = (&c.host, c.port);
    let db = c.database.as_deref().unwrap_or("");
    let pick = |off: &'static str, req: &'static str, full: &'static str| match c.ssl {
        JdbcSsl::Disable => off,
        JdbcSsl::Require => req,
        JdbcSsl::VerifyFull { .. } => full,
    };
    match c.dialect {
        JdbcDialect::Postgresql => format!(
            "jdbc:postgresql://{h}:{p}/{db}?sslmode={}",
            pick("disable", "require", "verify-full")
        ),
        JdbcDialect::Mysql => format!(
            "jdbc:mysql://{h}:{p}?sslMode={}",
            pick("DISABLED", "REQUIRED", "VERIFY_IDENTITY")
        ),
        JdbcDialect::Mariadb => format!(
            "jdbc:mariadb://{h}:{p}?sslMode={}",
            pick("disable", "trust", "verify-full")
        ),
        JdbcDialect::Oracle => match c.ssl {
            JdbcSsl::Disable => format!("jdbc:oracle:thin:@//{h}:{p}/{db}"),
            JdbcSsl::Require => format!("jdbc:oracle:thin:@tcps://{h}:{p}/{db}"),
            JdbcSsl::VerifyFull { .. } => format!(
                "jdbc:oracle:thin:@tcps://{h}:{p}/{db}?oracle.net.ssl_server_dn_match=true"
            ),
        },
        JdbcDialect::Sqlserver => format!(
            "jdbc:sqlserver://{h}:{p};databaseName={db};{}",
            pick(
                "encrypt=false",
                "encrypt=true;trustServerCertificate=true",
                "encrypt=true;trustServerCertificate=false"
            )
        ),
        JdbcDialect::Clickhouse => format!(
            "jdbc:clickhouse://{h}:{p}/{db}?{}",
            pick("ssl=false", "ssl=true&sslmode=none", "ssl=true&sslmode=strict")
        ),
    }
}

pub fn render_trino(
    name: &str,
    def: &CatalogDef,
    input: &TrinoRenderInput<'_>,
) -> Result<TrinoCatalogFile, RenderError> {
    if !def.can_read(EngineType::Trino) {
        return Err(RenderError::NotReadable(name.to_string()));
    }
    let mut out = Out {
        props: BTreeMap::new(),
        env: BTreeMap::new(),
        prefix: input.env_prefix,
        next: 0,
    };
    match &def.config {
        CatalogConfig::Iceberg(c) => {
            out.set("connector.name", "iceberg");
            match &c.metastore {
                IcebergMetastore::Egret { .. } => {
                    let wh = input
                        .warehouse
                        .as_ref()
                        .ok_or(RenderError::MissingInput("warehouse"))?;
                    let credential = input
                        .credential_secret_id
                        .ok_or(RenderError::MissingInput("catalog client credential"))?;
                    let var = format!("EGRET_{}", input.env_prefix);
                    out.set("iceberg.catalog.type", "rest");
                    out.set("iceberg.rest-catalog.uri", format!("{}/catalog", input.egret_url));
                    out.set("iceberg.rest-catalog.warehouse", wh.name);
                    out.set("iceberg.rest-catalog.security", "OAUTH2");
                    out.set(
                        "iceberg.rest-catalog.oauth2.server-uri",
                        format!("{}/oauth/token", input.egret_url),
                    );
                    out.set("iceberg.rest-catalog.oauth2.credential", format!("${{ENV:{var}}}"));
                    out.env.insert(var, credential);
                    out.set("iceberg.rest-catalog.vended-credentials-enabled", "true");
                    out.set("fs.native-s3.enabled", "true");
                    out.set("s3.endpoint", wh.s3_endpoint);
                    out.set("s3.region", "us-east-1");
                    out.set("s3.path-style-access", "true");
                }
                IcebergMetastore::Rest {
                    uri,
                    warehouse,
                    auth,
                } => {
                    out.set("iceberg.catalog.type", "rest");
                    out.set("iceberg.rest-catalog.uri", uri.as_str());
                    out.set("iceberg.rest-catalog.warehouse", warehouse.as_str());
                    if let RestAuth::Oauth2 {
                        credential_secret_id,
                        scope,
                        token_uri,
                    } = auth
                    {
                        out.set("iceberg.rest-catalog.security", "OAUTH2");
                        out.secret("iceberg.rest-catalog.oauth2.credential", *credential_secret_id);
                        if let Some(s) = scope {
                            out.set("iceberg.rest-catalog.oauth2.scope", s.as_str());
                        }
                        if let Some(t) = token_uri {
                            out.set("iceberg.rest-catalog.oauth2.server-uri", t.as_str());
                        }
                    }
                    out.s3(input)?;
                }
                IcebergMetastore::Hive { uri } => {
                    out.set("iceberg.catalog.type", "hive_metastore");
                    out.set("hive.metastore.uri", uri.as_str());
                    out.s3(input)?;
                }
                IcebergMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => {
                    out.set("iceberg.catalog.type", "glue");
                    out.glue(region, access_key.as_deref(), *secret_key_secret_id);
                    out.s3(input)?;
                }
            }
        }
        CatalogConfig::Hive(c) | CatalogConfig::DeltaLake(c) | CatalogConfig::Hudi(c) => {
            out.set("connector.name", def.trino_connector_name());
            match &c.metastore {
                HiveMetastore::Hive { uri } => out.set("hive.metastore.uri", uri.as_str()),
                HiveMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => {
                    out.set("hive.metastore", "glue");
                    out.glue(region, access_key.as_deref(), *secret_key_secret_id);
                }
            }
            out.s3(input)?;
        }
        CatalogConfig::Jdbc(c) => {
            out.set("connector.name", c.dialect.as_str());
            out.set("connection-url", jdbc_url(c));
            out.set("connection-user", c.user.as_str());
            out.secret("connection-password", c.password_secret_id);
        }
        CatalogConfig::Elasticsearch(c) => {
            out.set("connector.name", "elasticsearch");
            out.set("elasticsearch.host", c.hosts.join(","));
            out.set("elasticsearch.port", c.port.to_string());
            out.set("elasticsearch.tls.enabled", if c.tls { "true" } else { "false" });
            if let Some(user) = &c.user {
                out.set("elasticsearch.security", "PASSWORD");
                out.set("elasticsearch.auth.user", user.as_str());
                if let Some(id) = c.password_secret_id {
                    out.secret("elasticsearch.auth.password", id);
                }
            }
        }
        CatalogConfig::Tpch(_) => out.set("connector.name", "tpch"),
        CatalogConfig::Tpcds(_) => out.set("connector.name", "tpcds"),
        CatalogConfig::Generic(g) => {
            for (k, v) in &def.advanced.trino {
                out.set(k, v.as_str());
            }
            for (k, id) in &g.secret_properties.trino {
                out.secret(k, *id);
            }
        }
    }
    if !matches!(def.config, CatalogConfig::Generic(_)) {
        for (k, v) in &def.advanced.trino {
            if out.props.contains_key(k) {
                return Err(RenderError::Collision(k.clone()));
            }
            out.set(k, v.as_str());
        }
    }
    if let Some((k, _)) = out.props.iter().find(|(_, v)| v.contains(['\n', '\r'])) {
        return Err(RenderError::LineBreak(k.clone()));
    }
    Ok(TrinoCatalogFile {
        name: name.to_string(),
        properties: out.props,
        env: out.env,
    })
}
```

- [ ] **Step 5: Hook the collision probe into validation** — in `src/config.rs`, replace `CatalogDef::validate` with:

```rust
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_config(&self.config)?;
        validate_advanced(self)?;
        // What the typed form sets is exactly what rendering produces; probing
        // with inputs that satisfy every kind finds any advanced key that would
        // silently override the form (§4.3). SP3b adds the StarRocks probe.
        use crate::render::trino::{render_trino, RenderError, TrinoRenderInput};
        match render_trino("probe", self, &TrinoRenderInput::probe()) {
            Err(RenderError::Collision(key)) => Err(ValidationError::Collision {
                engine: "trino".into(),
                key,
                kind: self.kind().as_str().into(),
            }),
            Err(RenderError::LineBreak(key)) => Err(ValidationError::Config {
                kind: self.kind().as_str().into(),
                msg: format!("{key} contains a line break"),
            }),
            _ => Ok(()),
        }
    }
```

- [ ] **Step 6: Run the crate tests**

Run: `cargo test -p egret-catalog-registry --lib`
Expected: every `render::` and `config::` test passes.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry --all-targets -- -D warnings
git branch --show-current
git add crates/egret-catalog-registry
git commit -m "feat(catalog-registry): render catalogs to Trino properties

One pure function per target (spec §4.3). An egret warehouse renders
byte-for-byte what engines carry today, every JDBC dialect states its
TLS mode, secrets become \${ENV:…} placeholders resolved at apply time,
and an advanced key that collides with a typed key is refused at save.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Legacy-row classifier

**Files:**
- Create: `crates/egret-catalog-registry/src/legacy.rs`
- Modify: `crates/egret-catalog-registry/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1–2 (`CatalogDef`, `render_trino`, `TrinoRenderInput`, `is_plaintext_secret`).
- Produces: `struct LegacyCatalog<'a> { pub name: &'a str, pub kind: &'a str, pub config: &'a Value, pub secret_ref: Option<Uuid> }`; `fn legacy_props_and_secrets(kind: &str, config: &Value, secret_ref: Option<Uuid>) -> (String, Vec<(String, String)>, Vec<(String, Uuid)>)` (moved verbatim from `engines.rs::catalog_props_and_secrets`); `fn plaintext_secrets(&LegacyCatalog) -> Vec<(String, String)>`; `enum ClassifyOutcome { Typed, Generic { reason: String } }`; `struct Classified { pub def: CatalogDef, pub outcome: ClassifyOutcome }`; `fn classify(&LegacyCatalog, extracted: &BTreeMap<String, Uuid>) -> Result<Classified, ValidationError>`; `fn classify_generic(&LegacyCatalog, extracted: &BTreeMap<String, Uuid>) -> Result<CatalogDef, ValidationError>`.

The classifier serves both the backfill and the legacy `POST/PUT /engines/{id}/catalogs` handlers. Callers first create a named secret for each `plaintext_secrets` entry, then pass the ids as `extracted`. A typed result is accepted only when it renders exactly the legacy properties.

- [ ] **Step 1: Write the failing tests** — `src/legacy.rs` with the public items stubbed `todo!()` and:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CatalogConfig, JdbcSsl};
    use serde_json::json;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn legacy<'a>(kind: &'a str, config: &'a Value, secret_ref: Option<Uuid>) -> LegacyCatalog<'a> {
        LegacyCatalog { name: "cat", kind, config, secret_ref }
    }

    #[test]
    fn a_canonical_postgres_row_becomes_typed_jdbc() {
        let cfg = json!({
            "properties": { "connection-url": "jdbc:postgresql://db:5432/sales?sslmode=require", "connection-user": "reader" },
            "secretProperties": { "connection-password": id(7).to_string() }
        });
        let c = classify(&legacy("postgresql", &cfg, None), &BTreeMap::new()).unwrap();
        assert_eq!(c.outcome, ClassifyOutcome::Typed);
        let CatalogConfig::Jdbc(j) = &c.def.config else { panic!("{:?}", c.def) };
        assert_eq!((j.host.as_str(), j.port, j.database.as_deref()), ("db", 5432, Some("sales")));
        assert_eq!(j.ssl, JdbcSsl::Require);
        assert_eq!(j.password_secret_id, id(7));
    }

    #[test]
    fn a_url_without_an_explicit_ssl_mode_stays_generic() {
        let cfg = json!({
            "properties": { "connection-url": "jdbc:postgresql://db:5432/x", "connection-user": "u", "connection-password": "hunter2" }
        });
        let l = legacy("postgresql", &cfg, None);
        assert_eq!(plaintext_secrets(&l), vec![("connection-password".to_string(), "hunter2".to_string())]);
        let extracted: BTreeMap<String, Uuid> = [("connection-password".to_string(), id(9))].into_iter().collect();
        let c = classify(&l, &extracted).unwrap();
        assert!(matches!(c.outcome, ClassifyOutcome::Generic { .. }), "{:?}", c.outcome);
        let CatalogConfig::Generic(g) = &c.def.config else { panic!() };
        assert_eq!(g.secret_properties.trino["connection-password"], id(9));
        assert_eq!(c.def.advanced.trino["connector.name"], "postgresql");
        assert_eq!(c.def.advanced.trino["connection-url"], "jdbc:postgresql://db:5432/x");
        assert!(!c.def.advanced.trino.contains_key("connection-password"), "plaintext never stored");
    }

    #[test]
    fn tpch_rows_become_typed_with_their_extra_properties_in_advanced() {
        let c = classify(&legacy("tpch", &json!({}), None), &BTreeMap::new()).unwrap();
        assert_eq!(c.outcome, ClassifyOutcome::Typed);
        assert!(matches!(c.def.config, CatalogConfig::Tpch(_)));
        let cfg = json!({ "properties": { "tpch.column-naming": "STANDARD" } });
        let c = classify(&legacy("tpch", &cfg, None), &BTreeMap::new()).unwrap();
        assert_eq!(c.outcome, ClassifyOutcome::Typed);
        assert_eq!(c.def.advanced.trino["tpch.column-naming"], "STANDARD");
    }

    /// The pre-v2 postgres shape (`host/port/database/user` + `secret_ref`) and
    /// the pre-v2 generic shape (`connectorName` + `secretProperty`) still map.
    #[test]
    fn v1_rows_keep_their_rendered_properties() {
        let cfg = json!({ "host": "pg", "port": 5433, "database": "d", "user": "u" });
        let c = classify(&legacy("postgresql", &cfg, Some(id(3))), &BTreeMap::new()).unwrap();
        let f = crate::render_trino("cat", &c.def, &probe_free()).unwrap();
        let (plain, secret) = f.split_secret_properties();
        assert_eq!(plain["connection-url"], "jdbc:postgresql://pg:5433/d");
        assert_eq!(plain["connection-user"], "u");
        assert_eq!(secret["connection-password"], id(3));

        let cfg = json!({ "connectorName": "mysql", "properties": { "connection-url": "jdbc:mysql://m:3306" }, "secretProperty": "connection-password" });
        let c = classify(&legacy("generic", &cfg, Some(id(4))), &BTreeMap::new()).unwrap();
        assert_eq!(c.def.trino_connector_name(), "mysql");
        let f = crate::render_trino("cat", &c.def, &probe_free()).unwrap();
        assert_eq!(f.split_secret_properties().1["connection-password"], id(4));
    }

    #[test]
    fn unknown_connectors_are_generic_and_flags_are_not_secrets() {
        let cfg = json!({ "properties": {
            "iceberg.catalog.type": "rest",
            "iceberg.rest-catalog.vended-credentials-enabled": "true"
        } });
        let l = legacy("iceberg", &cfg, None);
        assert!(plaintext_secrets(&l).is_empty());
        let c = classify(&l, &BTreeMap::new()).unwrap();
        assert!(matches!(c.outcome, ClassifyOutcome::Generic { .. }));
        assert_eq!(c.def.advanced.trino["iceberg.rest-catalog.vended-credentials-enabled"], "true");
    }

    #[test]
    fn two_secret_properties_survive() {
        let cfg = json!({ "properties": { "a.user": "x" }, "secretProperties": { "a.password": id(1).to_string(), "b.token": id(2).to_string() } });
        let c = classify(&legacy("bigquery", &cfg, None), &BTreeMap::new()).unwrap();
        let CatalogConfig::Generic(g) = &c.def.config else { panic!() };
        assert_eq!(g.secret_properties.trino.len(), 2);
    }

    fn probe_free() -> crate::TrinoRenderInput<'static> {
        crate::TrinoRenderInput { env_prefix: "T", egret_url: "", warehouse: None, storage: None, credential_secret_id: None }
    }
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p egret-catalog-registry --lib legacy::`
Expected: FAIL (`not yet implemented`).

- [ ] **Step 3: Implement `src/legacy.rs`**. Copy the body of `catalog_props_and_secrets` from `crates/egret-engine-mgmt/src/engines.rs` (lines 531–608 on `main`) into `legacy_props_and_secrets`, replacing `cat.kind` with `kind`, `cat.config` with `config` and `cat.secret_ref` with `secret_ref`; everything else stays verbatim. Then add:

```rust
//! Classification of an `engine_mgmt.engine_catalogs` row (or a legacy
//! `POST/PUT /engines/{id}/catalogs` body) into a registry definition.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;
use uuid::Uuid;

use crate::config::{
    Advanced, CatalogConfig, CatalogDef, EngineType, GenericConfig, JdbcConfig, JdbcDialect,
    JdbcSsl, SampleConfig, SecretProperties, ValidationError,
};
use crate::render::trino::{render_trino, TrinoRenderInput};
use crate::sensitive::is_plaintext_secret;

/// A legacy catalog as stored in `engine_mgmt.engine_catalogs` or posted to the
/// legacy engine-catalog endpoints.
#[derive(Clone, Copy, Debug)]
pub struct LegacyCatalog<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub config: &'a Value,
    pub secret_ref: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassifyOutcome {
    Typed,
    Generic { reason: String },
}

#[derive(Clone, Debug)]
pub struct Classified {
    pub def: CatalogDef,
    pub outcome: ClassifyOutcome,
}

/// Plaintext secrets inside `properties`: each must become a named secret
/// before [`classify`] runs (pass the new ids as `extracted`).
pub fn plaintext_secrets(legacy: &LegacyCatalog<'_>) -> Vec<(String, String)> {
    let (_, plain, _) = legacy_props_and_secrets(legacy.kind, legacy.config, legacy.secret_ref);
    plain
        .into_iter()
        .filter(|(k, v)| is_plaintext_secret(k, v))
        .collect()
}

pub fn classify(
    legacy: &LegacyCatalog<'_>,
    extracted: &BTreeMap<String, Uuid>,
) -> Result<Classified, ValidationError> {
    let (want_plain, want_secret) = legacy_target(legacy, extracted);

    let reason = match typed_candidate(&want_plain, &want_secret) {
        Ok(def) if def.validate().is_ok() && round_trips(legacy.name, &def, &want_plain, &want_secret) => {
            return Ok(Classified { def, outcome: ClassifyOutcome::Typed });
        }
        Ok(_) => "the typed form would not render the same Trino properties".to_string(),
        Err(reason) => reason,
    };

    Ok(Classified {
        def: generic_def(want_plain, want_secret)?,
        outcome: ClassifyOutcome::Generic { reason },
    })
}

/// The legacy catalog as a `generic` Trino catalog, without trying a typed
/// form — what a legacy `PUT` of a catalog that is already generic keeps.
pub fn classify_generic(
    legacy: &LegacyCatalog<'_>,
    extracted: &BTreeMap<String, Uuid>,
) -> Result<CatalogDef, ValidationError> {
    let (want_plain, want_secret) = legacy_target(legacy, extracted);
    generic_def(want_plain, want_secret)
}

/// What the engine renders today: `connector.name` first, then the plain
/// properties (which may override it, exactly as the old renderer did), and
/// every secret-backed property.
fn legacy_target(
    legacy: &LegacyCatalog<'_>,
    extracted: &BTreeMap<String, Uuid>,
) -> (BTreeMap<String, String>, BTreeMap<String, Uuid>) {
    let (connector, plain, secrets) =
        legacy_props_and_secrets(legacy.kind, legacy.config, legacy.secret_ref);
    let mut want_plain: BTreeMap<String, String> = BTreeMap::new();
    want_plain.insert("connector.name".into(), connector);
    for (k, v) in plain {
        if !extracted.contains_key(&k) {
            want_plain.insert(k, v);
        }
    }
    let mut want_secret: BTreeMap<String, Uuid> = secrets.into_iter().collect();
    want_secret.extend(extracted.iter().map(|(k, v)| (k.clone(), *v)));
    (want_plain, want_secret)
}

fn generic_def(
    want_plain: BTreeMap<String, String>,
    want_secret: BTreeMap<String, Uuid>,
) -> Result<CatalogDef, ValidationError> {
    let def = CatalogDef {
        config: CatalogConfig::Generic(GenericConfig {
            engines: [EngineType::Trino].into_iter().collect(),
            secret_properties: SecretProperties {
                trino: want_secret,
                starrocks: BTreeMap::new(),
            },
        }),
        advanced: Advanced {
            trino: want_plain,
            starrocks: BTreeMap::new(),
        },
    };
    def.validate()?;
    Ok(def)
}

fn round_trips(
    name: &str,
    def: &CatalogDef,
    want_plain: &BTreeMap<String, String>,
    want_secret: &BTreeMap<String, Uuid>,
) -> bool {
    let input = TrinoRenderInput {
        env_prefix: "R",
        egret_url: "",
        warehouse: None,
        storage: None,
        credential_secret_id: None,
    };
    match render_trino(name, def, &input) {
        Ok(file) => {
            let (plain, secret) = file.split_secret_properties();
            &plain == want_plain && &secret == want_secret
        }
        Err(_) => false,
    }
}

fn dialect_of(connector: &str) -> Option<JdbcDialect> {
    Some(match connector {
        "postgresql" => JdbcDialect::Postgresql,
        "mysql" => JdbcDialect::Mysql,
        "mariadb" => JdbcDialect::Mariadb,
        "oracle" => JdbcDialect::Oracle,
        "sqlserver" => JdbcDialect::Sqlserver,
        "clickhouse" => JdbcDialect::Clickhouse,
        _ => return None,
    })
}

fn typed_candidate(
    plain: &BTreeMap<String, String>,
    secret: &BTreeMap<String, Uuid>,
) -> Result<CatalogDef, String> {
    let connector = plain.get("connector.name").map(String::as_str).unwrap_or("");
    let rest = |skip: &[&str]| -> BTreeMap<String, String> {
        plain
            .iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    if connector == "tpch" || connector == "tpcds" {
        if !secret.is_empty() {
            return Err("carries secret properties the typed form has no field for".into());
        }
        let config = if connector == "tpch" {
            CatalogConfig::Tpch(SampleConfig::default())
        } else {
            CatalogConfig::Tpcds(SampleConfig::default())
        };
        let advanced = Advanced { trino: rest(&["connector.name"]), starrocks: BTreeMap::new() };
        return Ok(CatalogDef { config, advanced });
    }
    let Some(dialect) = dialect_of(connector) else {
        return Err(format!("no typed form for connector {connector:?}"));
    };
    let url = plain.get("connection-url").ok_or("no connection-url")?;
    let user = plain.get("connection-user").ok_or("no connection-user")?;
    if secret.len() != 1 || !secret.contains_key("connection-password") {
        return Err("the typed form needs exactly one secret, connection-password".into());
    }
    let (host, port, database, ssl) = parse_jdbc_url(dialect, url)
        .ok_or("connection-url is not in the canonical form egret renders")?;
    let advanced = Advanced {
        trino: rest(&["connector.name", "connection-url", "connection-user"]),
        starrocks: BTreeMap::new(),
    };
    Ok(CatalogDef {
        config: CatalogConfig::Jdbc(JdbcConfig {
            dialect,
            host,
            port,
            database,
            user: user.clone(),
            password_secret_id: secret["connection-password"],
            ssl,
        }),
        advanced,
    })
}

fn caps<'u>(cell: &'static OnceLock<regex::Regex>, pattern: &str, url: &'u str) -> Option<regex::Captures<'u>> {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("static jdbc pattern"))
        .captures(url)
}

/// Inverse of [`crate::jdbc_url`] for the canonical forms only; anything else is
/// `None` and the row stays generic. The round-trip check in [`classify`] is the
/// real gate, so a too-lenient pattern here cannot change what Trino sees.
fn parse_jdbc_url(dialect: JdbcDialect, url: &str) -> Option<(String, u16, Option<String>, JdbcSsl)> {
    static PG: OnceLock<regex::Regex> = OnceLock::new();
    static MY: OnceLock<regex::Regex> = OnceLock::new();
    static MA: OnceLock<regex::Regex> = OnceLock::new();
    static OR: OnceLock<regex::Regex> = OnceLock::new();
    static SS: OnceLock<regex::Regex> = OnceLock::new();
    static CH: OnceLock<regex::Regex> = OnceLock::new();
    let verify = JdbcSsl::VerifyFull { ca_secret_id: None };
    let (c, ssl) = match dialect {
        JdbcDialect::Postgresql => {
            let c = caps(&PG, r"^jdbc:postgresql://(?P<h>[^:/?]+):(?P<p>\d+)/(?P<d>[^?]+)\?sslmode=(?P<s>disable|require|verify-full)$", url)?;
            let ssl = match &c["s"] { "disable" => JdbcSsl::Disable, "require" => JdbcSsl::Require, _ => verify };
            (c, ssl)
        }
        JdbcDialect::Mysql => {
            let c = caps(&MY, r"^jdbc:mysql://(?P<h>[^:/?]+):(?P<p>\d+)\?sslMode=(?P<s>DISABLED|REQUIRED|VERIFY_IDENTITY)$", url)?;
            let ssl = match &c["s"] { "DISABLED" => JdbcSsl::Disable, "REQUIRED" => JdbcSsl::Require, _ => verify };
            (c, ssl)
        }
        JdbcDialect::Mariadb => {
            let c = caps(&MA, r"^jdbc:mariadb://(?P<h>[^:/?]+):(?P<p>\d+)\?sslMode=(?P<s>disable|trust|verify-full)$", url)?;
            let ssl = match &c["s"] { "disable" => JdbcSsl::Disable, "trust" => JdbcSsl::Require, _ => verify };
            (c, ssl)
        }
        JdbcDialect::Oracle => {
            let c = caps(&OR, r"^jdbc:oracle:thin:@(?P<s>//|tcps://)(?P<h>[^:/?]+):(?P<p>\d+)/(?P<d>[^?]+)(?P<dn>\?oracle\.net\.ssl_server_dn_match=true)?$", url)?;
            let ssl = match (&c["s"], c.name("dn").is_some()) {
                ("//", _) => JdbcSsl::Disable,
                (_, false) => JdbcSsl::Require,
                (_, true) => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Sqlserver => {
            let c = caps(&SS, r"^jdbc:sqlserver://(?P<h>[^:;]+):(?P<p>\d+);databaseName=(?P<d>[^;]+);(?P<s>encrypt=false|encrypt=true;trustServerCertificate=true|encrypt=true;trustServerCertificate=false)$", url)?;
            let ssl = match &c["s"] {
                "encrypt=false" => JdbcSsl::Disable,
                "encrypt=true;trustServerCertificate=true" => JdbcSsl::Require,
                _ => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Clickhouse => {
            let c = caps(&CH, r"^jdbc:clickhouse://(?P<h>[^:/?]+):(?P<p>\d+)/(?P<d>[^?]*)\?(?P<s>ssl=false|ssl=true&sslmode=none|ssl=true&sslmode=strict)$", url)?;
            let ssl = match &c["s"] { "ssl=false" => JdbcSsl::Disable, "ssl=true&sslmode=none" => JdbcSsl::Require, _ => verify };
            (c, ssl)
        }
    };
    let host = c.name("h")?.as_str().to_string();
    let port = c.name("p")?.as_str().parse().ok()?;
    let database = c.name("d").map(|m| m.as_str().to_string()).filter(|d| !d.is_empty());
    Some((host, port, database, ssl))
}
```


In `src/lib.rs` add `pub mod legacy;` and:

```rust
pub use legacy::{
    classify, classify_generic, legacy_props_and_secrets, plaintext_secrets, Classified,
    ClassifyOutcome, LegacyCatalog,
};
```

- [ ] **Step 4: Run the crate tests**

Run: `cargo test -p egret-catalog-registry --lib`
Expected: all pass, including the six `legacy::` tests.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry --all-targets -- -D warnings
git branch --show-current
git add crates/egret-catalog-registry
git commit -m "feat(catalog-registry): classify legacy engine catalogs

A legacy engine_catalogs row (or legacy endpoint body) becomes a typed
catalog only when the typed form renders exactly the Trino properties
the engine has today; everything else becomes generic with its
properties preserved. Plaintext passwords are surfaced for extraction
into named secrets instead of being stored.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 4: Migration 0147 — the `registry` schema

**Files:**
- Create: `migrations/0147_catalog_registry.sql`
- Modify: `crates/egret-engine-mgmt/tests/dependents.rs` (`every_fk_child_of_engines_cascades`)

**Interfaces:**
- Produces the tables every later task reads: `registry.catalogs`, `registry.engine_catalog_attachments`, `registry.catalog_workspaces`, `registry.catalog_secret_refs`, `registry.legacy_engine_catalogs`, `registry.migration_report`. Constraint names later tasks match on: `catalogs_name_key`, `catalogs_warehouse_id_key`, `engine_catalog_attachments_pkey`.

- [ ] **Step 1: Write the failing FK-inventory expectation** — in `crates/egret-engine-mgmt/tests/dependents.rs`, `every_fk_child_of_engines_cascades`, change the expected set to:

```rust
    let expected: BTreeSet<FkRow> = [
        ("engine_mgmt.engine_catalogs", "engine_id", "c", true),
        ("engine_mgmt.engine_workspaces", "engine_id", "c", true),
        ("query.databases", "engine_id", "c", false),
        ("registry.engine_catalog_attachments", "engine_id", "c", true),
    ]
```

and its doc comment to `/// Deleting the engine row cascades these four; nothing else may reference it.`

- [ ] **Step 2: Run it to see it fail**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test dependents every_fk_child_of_engines_cascades
```
Expected: FAIL — the live set lacks `registry.engine_catalog_attachments`.

- [ ] **Step 3: Write `migrations/0147_catalog_registry.sql`**

```sql
-- 0147_catalog_registry.sql
-- SP3: a catalog is a platform resource (spec docs/en/specs/2026-09-19-catalog-registry-design.md).
-- `registry.catalogs` holds one connection definition per catalog; engines attach
-- catalogs, workspaces are assigned catalogs. Data is filled by the Rust backfill
-- at boot (egret-catalog-registry::backfill), not here: naming, dedupe, typed
-- parsing and secret extraction need the typed parser and the secret vault.
CREATE SCHEMA IF NOT EXISTS registry;

CREATE TABLE registry.catalogs (
    id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The catalog's SQL name on every engine (D5). Immutable.
    name                  text NOT NULL UNIQUE CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    kind                  text NOT NULL CHECK (kind IN
                              ('iceberg', 'hive', 'delta_lake', 'hudi', 'jdbc',
                               'elasticsearch', 'tpch', 'tpcds', 'generic')),
    -- Typed per kind, validated by egret_catalog_registry::CatalogDef on every
    -- write. Never a plaintext secret: only secret ids and storage ids.
    config                jsonb NOT NULL,
    -- {"trino": {k: v}, "starrocks": {k: v}}, rendered after the typed keys.
    advanced              jsonb NOT NULL DEFAULT '{}'::jsonb,
    description           text,
    -- Mirrors config.metastore.warehouse_id of an egret catalog: one catalog per
    -- warehouse, and the catalog goes when its warehouse goes.
    warehouse_id          uuid REFERENCES catalog.warehouses(id) ON DELETE CASCADE,
    -- Mirrors config.storage_connection_id.
    storage_connection_id uuid REFERENCES storage.storage_connections(id),
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT catalogs_warehouse_mirrors_config CHECK (
        (warehouse_id IS NULL AND NOT (kind = 'iceberg' AND config->'metastore'->>'type' = 'egret'))
        OR (warehouse_id IS NOT NULL AND kind = 'iceberg'
            AND config->'metastore'->>'type' = 'egret'
            AND config->'metastore'->>'warehouse_id' = warehouse_id::text)),
    CONSTRAINT catalogs_storage_mirrors_config CHECK (
        storage_connection_id IS NOT DISTINCT FROM (config->>'storage_connection_id')::uuid)
);
CREATE UNIQUE INDEX catalogs_warehouse_id_key ON registry.catalogs (warehouse_id)
    WHERE warehouse_id IS NOT NULL;
CREATE INDEX catalogs_storage_connection_idx ON registry.catalogs (storage_connection_id);

-- NO ACTION (end of statement), not RESTRICT, on both catalog_id FKs: deleting a
-- workspace cascades to its warehouses and their catalogs in the same statement
-- that removes the workspace's own assignment rows; RESTRICT would fire first.
-- The API still refuses a catalog delete with 409 while anything references it.
CREATE TABLE registry.engine_catalog_attachments (
    engine_id            uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    catalog_id           uuid NOT NULL REFERENCES registry.catalogs(id),
    -- egret catalogs only: the Catalog Client egret minted for this engine…
    catalog_client_id    uuid,
    -- …and the named global secret holding its "client_id:client_secret".
    credential_secret_id uuid,
    -- false: the client belongs to someone else and is never revoked by egret.
    minted_client        boolean NOT NULL DEFAULT true,
    apply_status         text NOT NULL DEFAULT 'applied' CHECK (apply_status IN ('applied', 'failed')),
    apply_error          text,
    applied_at           timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (engine_id, catalog_id),
    CONSTRAINT attachments_client_has_credential
        CHECK (catalog_client_id IS NULL OR credential_secret_id IS NOT NULL)
);
CREATE INDEX engine_catalog_attachments_catalog_idx
    ON registry.engine_catalog_attachments (catalog_id);
CREATE INDEX engine_catalog_attachments_credential_idx
    ON registry.engine_catalog_attachments (credential_secret_id);

CREATE TABLE registry.catalog_workspaces (
    catalog_id   uuid NOT NULL REFERENCES registry.catalogs(id),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (catalog_id, workspace_id)
);
CREATE INDEX catalog_workspaces_workspace_idx ON registry.catalog_workspaces (workspace_id);

-- Every secret a catalog's config references (written with the catalog). Counted
-- by WorkspaceSecretStore::used_by, so a referenced secret cannot be deleted.
CREATE TABLE registry.catalog_secret_refs (
    catalog_id uuid NOT NULL REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    secret_id  uuid NOT NULL,
    PRIMARY KEY (catalog_id, secret_id)
);
CREATE INDEX catalog_secret_refs_secret_idx ON registry.catalog_secret_refs (secret_id);

-- Which engine_mgmt.engine_catalogs rows the backfill has migrated. A row here is
-- never migrated again, so a detach or catalog delete an admin makes later is not
-- undone at the next boot. No FK to engine_catalogs: SP3c drops that table.
CREATE TABLE registry.legacy_engine_catalogs (
    engine_catalog_id uuid PRIMARY KEY,
    engine_id         uuid NOT NULL,
    catalog_id        uuid REFERENCES registry.catalogs(id) ON DELETE SET NULL,
    migrated_at       timestamptz NOT NULL DEFAULT now()
);

-- What the backfill did, for the log and for the Catalogs page (shown until
-- acknowledged). Codes: warehouse_catalog_created, primary_named_egret,
-- engine_catalog_typed, engine_catalog_generic, engine_catalog_shared,
-- engine_catalog_suffixed, engine_catalog_renamed, duplicate_attachment_skipped,
-- secret_extracted, saved_sql_references_catalog, backfill_failed.
CREATE TABLE registry.migration_report (
    id              bigserial PRIMARY KEY,
    run_id          uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    severity        text NOT NULL CHECK (severity IN ('info', 'warning')),
    code            text NOT NULL,
    catalog_id      uuid REFERENCES registry.catalogs(id) ON DELETE SET NULL,
    engine_id       uuid,
    warehouse_id    uuid,
    subject         text NOT NULL,
    message         text NOT NULL,
    detail          jsonb NOT NULL DEFAULT '{}'::jsonb,
    acknowledged_at timestamptz
);
CREATE INDEX migration_report_unacknowledged_idx
    ON registry.migration_report (id) WHERE acknowledged_at IS NULL;
```

- [ ] **Step 4: Refresh the test template and re-run**

```bash
psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test dependents
```
Expected: all `dependents` tests pass; the run takes seconds (the template is rebuilt), not milliseconds.

- [ ] **Step 5: Check the migration is embedded** — `crates/egret-metastore/build.rs` re-embeds on a new file; confirm with `cargo build -p egret-metastore` producing no error and the previous step having created `registry.*` (the FK test saw the new table).

- [ ] **Step 6: Commit**

```bash
git branch --show-current
git add migrations/0147_catalog_registry.sql crates/egret-engine-mgmt/tests/dependents.rs
git commit -m "feat(catalog-registry): registry schema

Catalogs, engine attachments with apply status, workspace assignments,
secret references (so a referenced secret cannot be deleted), the
legacy-row map that keeps the boot backfill idempotent, and the
migration report.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `RegistryStore`, secret reference counting, store accessors

**Files:**
- Create: `crates/egret-catalog-registry/src/error.rs`, `src/report.rs`, `src/store.rs`, `tests/store.rs`
- Modify: `crates/egret-catalog-registry/Cargo.toml`, `src/lib.rs`; `crates/egret-secrets/src/workspace_secrets.rs` (`list`, `get_meta`); `crates/egret-engine-mgmt/Cargo.toml`, `src/store.rs`, `src/http_error.rs`; `crates/egret-catalog-engine/Cargo.toml`, `src/store/handle.rs`

**Interfaces:**
- Consumes: Task 1–3 types; Task 4 schema.
- Produces:
  - `RegistryError { NotFound(String), AlreadyExists(String), Conflict(String), Invalid(ValidationError), Secrets(egret_secrets::SecretsError), Db(sqlx::Error) }`.
  - `report`: `enum Severity { Info, Warning }`, `struct ReportEntry { run_id, severity, code: &'static str, catalog_id, engine_id, warehouse_id: Option<Uuid>, subject: String, message: String, detail: Value }` with `info(run_id, code, subject, message)`, `warning(…)`, builders `.catalog(id)`, `.engine(id)`, `.warehouse(id)`, `.detail(v)`; `struct ReportRow` (sqlx row of `registry.migration_report`).
  - `store`: `mod apply_status { APPLIED, FAILED }`; `CatalogRow` (+ `def()`), `AttachmentRow` (+ `pending(engine_id, catalog_id, client, secret)`), `AttachedCatalog { catalog, attachment }`, `CatalogEngineRow { engine_id, engine_name, engine_type, apply_status, apply_error, applied_at }`, `CatalogWorkspaceRow { workspace_id, name, slug, assigned_at }`, `NewAttachment { engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, applied_now: bool, created_at: Option<DateTime<Utc>> }`, `WarehouseIdentity<'a> { id, name, workspace_id }`, `CatalogFilter { workspace_id, secret_id }`, and `RegistryStore` methods listed in Step 5.
  - `EngineMgmtStore::registry(&self) -> RegistryStore`, `CatalogStore::registry(&self) -> RegistryStore`, `impl From<RegistryError> for EngineHttpError`.

- [ ] **Step 1: Dependencies** — `crates/egret-catalog-registry/Cargo.toml` `[dependencies]` gains:

```toml
egret-metastore.workspace = true
egret-secrets.workspace = true
sqlx.workspace = true
chrono.workspace = true
tracing.workspace = true

[dev-dependencies]
egret-test-db.workspace = true
tokio.workspace = true
```

`crates/egret-engine-mgmt/Cargo.toml` and `crates/egret-catalog-engine/Cargo.toml` `[dependencies]` each gain `egret-catalog-registry.workspace = true` (engine-mgmt: after `egret-catalog-api.workspace = true`; catalog-engine: after `egret-catalog-api = { workspace = true }`, same style).

- [ ] **Step 2: `src/error.rs` and `src/report.rs`**

```rust
//! Registry store errors.

use crate::config::ValidationError;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error(transparent)]
    Secrets(#[from] egret_secrets::SecretsError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
```

```rust
//! `registry.migration_report` rows: what the backfill did, kept until an admin
//! acknowledges them on the Catalogs page.

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReportEntry {
    pub run_id: Uuid,
    pub severity: Severity,
    pub code: &'static str,
    pub catalog_id: Option<Uuid>,
    pub engine_id: Option<Uuid>,
    pub warehouse_id: Option<Uuid>,
    pub subject: String,
    pub message: String,
    pub detail: Value,
}

impl ReportEntry {
    fn new(run_id: Uuid, severity: Severity, code: &'static str, subject: String, message: String) -> Self {
        ReportEntry { run_id, severity, code, catalog_id: None, engine_id: None, warehouse_id: None, subject, message, detail: Value::Object(Default::default()) }
    }
    pub fn info(run_id: Uuid, code: &'static str, subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(run_id, Severity::Info, code, subject.into(), message.into())
    }
    pub fn warning(run_id: Uuid, code: &'static str, subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(run_id, Severity::Warning, code, subject.into(), message.into())
    }
    pub fn catalog(mut self, id: Uuid) -> Self {
        self.catalog_id = Some(id);
        self
    }
    pub fn engine(mut self, id: Uuid) -> Self {
        self.engine_id = Some(id);
        self
    }
    pub fn warehouse(mut self, id: Uuid) -> Self {
        self.warehouse_id = Some(id);
        self
    }
    pub fn detail(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ReportRow {
    pub id: i64,
    pub run_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub severity: String,
    pub code: String,
    pub catalog_id: Option<Uuid>,
    pub engine_id: Option<Uuid>,
    pub warehouse_id: Option<Uuid>,
    pub subject: String,
    pub message: String,
    pub detail: Value,
    pub acknowledged_at: Option<DateTime<Utc>>,
}
```

- [ ] **Step 3: Write the failing store tests** — `crates/egret-catalog-registry/tests/store.rs`:

```rust
//! RegistryStore against a real database. DB-gated; self-skips only under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry --test store

use egret_catalog_registry::store::{apply_status, CatalogFilter, NewAttachment, WarehouseIdentity};
use egret_catalog_registry::report::ReportEntry;
use egret_catalog_registry::{CatalogDef, RegistryError, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

async fn db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn secrets(db: &Db) -> egret_secrets::WorkspaceSecretStore {
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto");
    egret_secrets::WorkspaceSecretStore::new(db.clone(), egret_secrets::SecretStore::new(db.clone(), Some(crypto)))
}

async fn workspace(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO workspaces (slug, name, status) VALUES ($1, 'Reg', 'active') RETURNING id")
        .bind(format!("reg-{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool).await.expect("workspace")
}

async fn warehouse(db: &Db, name: &str, ws: Uuid) -> Uuid {
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'b', 'k', true) RETURNING id")
        .bind(format!("reg-sc-{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool).await.expect("storage");
    sqlx::query_scalar("INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id")
        .bind(name).bind(ws).bind(sc)
        .fetch_one(&db.pool).await.expect("warehouse")
}

async fn engine(db: &Db, wh: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id")
        .bind(format!("reg-eng-{}", Uuid::new_v4().simple())).bind(wh)
        .fetch_one(&db.pool).await.expect("engine")
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

#[tokio::test]
async fn catalog_crud_keeps_secret_refs_and_blocks_secret_deletes() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let vault = secrets(&db);
    let pw = vault.create(None, &unique("pw"), None, "hunter2").await.unwrap();
    let pw2 = vault.create(None, &unique("pw"), None, "hunter3").await.unwrap();
    let def = |id: Uuid| CatalogDef::parse("jdbc", &json!({
        "dialect": "postgresql", "host": "db", "port": 5432, "database": "d",
        "user": "u", "password_secret_id": id, "ssl": { "mode": "require" }
    }), &json!(null)).unwrap();

    let name = unique("pg");
    let row = store.create_catalog(&name, &def(pw), Some("sales db")).await.unwrap();
    assert_eq!(row.kind, "jdbc");
    assert_eq!(row.def().unwrap(), def(pw));
    assert!(matches!(store.create_catalog(&name, &def(pw), None).await, Err(RegistryError::AlreadyExists(_))));
    assert!(matches!(store.create_catalog("Bad-Name", &def(pw), None).await, Err(RegistryError::Invalid(_))));

    // A referenced secret cannot be deleted.
    assert!(matches!(vault.delete(None, pw).await, Err(egret_secrets::SecretsError::InUse(1))));
    let listed = store.list_catalogs(&CatalogFilter { secret_id: Some(pw), ..Default::default() }).await.unwrap();
    assert_eq!(listed.iter().map(|c| c.id).collect::<Vec<_>>(), vec![row.id]);

    // Re-pointing the catalog releases the old secret.
    let updated = store.update_catalog(row.id, &def(pw2), None).await.unwrap().unwrap();
    assert!(updated.updated_at >= row.updated_at);
    vault.delete(None, pw).await.expect("old secret no longer referenced");
    assert!(matches!(vault.delete(None, pw2).await, Err(egret_secrets::SecretsError::InUse(1))));

    assert!(store.delete_catalog(row.id).await.unwrap());
    vault.delete(None, pw2).await.expect("catalog gone, secret free");
}

#[tokio::test]
async fn warehouse_catalogs_are_created_once_with_their_workspace() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let base = unique("Sales-Data");
    let wh1 = warehouse(&db, &base, ws).await;
    let wh2 = warehouse(&db, &base.to_uppercase(), workspace(&db).await).await;

    let (c1, created) = store.ensure_warehouse_catalog(&WarehouseIdentity { id: wh1, name: &base, workspace_id: ws }, None).await.unwrap();
    assert!(created);
    assert_eq!(c1.name, egret_catalog_registry::normalize_catalog_name(&base));
    assert_eq!(c1.warehouse_id, Some(wh1));
    assert_eq!(store.list_catalog_workspaces(c1.id).await.unwrap().iter().map(|w| w.workspace_id).collect::<Vec<_>>(), vec![ws]);

    let (again, created) = store.ensure_warehouse_catalog(&WarehouseIdentity { id: wh1, name: &base, workspace_id: ws }, None).await.unwrap();
    assert!(!created);
    assert_eq!(again.id, c1.id);

    let (c2, _) = store.ensure_warehouse_catalog(&WarehouseIdentity { id: wh2, name: &base, workspace_id: ws }, None).await.unwrap();
    assert_eq!(c2.name, format!("{}_2", c1.name), "same normalised name gets a suffix");

    // Removing the assignment is not undone by ensuring again.
    store.set_catalog_workspaces(c1.id, &[]).await.unwrap();
    store.ensure_warehouse_catalog(&WarehouseIdentity { id: wh1, name: &base, workspace_id: ws }, None).await.unwrap();
    assert!(store.list_catalog_workspaces(c1.id).await.unwrap().is_empty());
    assert!(!store.warehouse_visible_to_workspace(wh1, ws).await.unwrap());
    store.set_catalog_workspaces(c1.id, &[ws]).await.unwrap();
    assert!(store.warehouse_visible_to_workspace(wh1, ws).await.unwrap());
}

#[tokio::test]
async fn attachments_order_status_and_block_deletes() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let wh = warehouse(&db, &unique("wh"), ws).await;
    let eng = engine(&db, wh).await;
    let (primary, _) = store.ensure_warehouse_catalog(&WarehouseIdentity { id: wh, name: &unique("wh"), workspace_id: ws }, None).await.unwrap();
    let tpch = store.create_catalog(&unique("bench"), &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(), None).await.unwrap();

    store.insert_attachment(&NewAttachment { engine_id: eng, catalog_id: tpch.id, catalog_client_id: None, credential_secret_id: None, minted_client: false, applied_now: true, created_at: None }).await.unwrap();
    store.insert_attachment(&NewAttachment { engine_id: eng, catalog_id: primary.id, catalog_client_id: None, credential_secret_id: None, minted_client: false, applied_now: false, created_at: None }).await.unwrap();
    assert!(matches!(
        store.insert_attachment(&NewAttachment { engine_id: eng, catalog_id: tpch.id, catalog_client_id: None, credential_secret_id: None, minted_client: false, applied_now: true, created_at: None }).await,
        Err(RegistryError::AlreadyExists(_))));

    let listed = store.list_engine_attachments(eng, Some(wh)).await.unwrap();
    assert_eq!(listed.iter().map(|a| a.catalog.id).collect::<Vec<_>>(), vec![primary.id, tpch.id], "primary first");
    assert_eq!(store.attached_catalog_names(eng).await.unwrap().len(), 2);

    store.set_apply_failed(eng, tpch.id, "boom").await.unwrap();
    let a = store.get_attachment(eng, tpch.id).await.unwrap().unwrap();
    assert_eq!((a.apply_status.as_str(), a.apply_error.as_deref()), (apply_status::FAILED, Some("boom")));

    // A successful apply that read catalogs BEFORE this edit must not mark it applied.
    let read_at = store.db_now().await.unwrap();
    store.update_catalog(tpch.id, &CatalogDef::parse("tpch", &json!({ "scale": 2 }), &json!(null)).unwrap(), None).await.unwrap();
    store.mark_engine_applied(eng, read_at).await.unwrap();
    assert_eq!(store.get_attachment(eng, tpch.id).await.unwrap().unwrap().apply_status, apply_status::FAILED);
    store.mark_engine_applied(eng, store.db_now().await.unwrap()).await.unwrap();
    let a = store.get_attachment(eng, tpch.id).await.unwrap().unwrap();
    assert_eq!(a.apply_status, apply_status::APPLIED);
    assert!(a.apply_error.is_none() && a.applied_at.is_some());

    assert_eq!(store.list_catalog_attachments(tpch.id).await.unwrap()[0].engine_id, eng);
    assert!(matches!(store.delete_catalog(tpch.id).await, Err(RegistryError::Conflict(_))));
    assert!(store.delete_attachment(eng, tpch.id).await.unwrap());
    assert!(store.delete_catalog(tpch.id).await.unwrap());
    assert_eq!(store.unmigrated_legacy_catalog_count(eng).await.unwrap(), 0);
}

#[tokio::test]
async fn report_rows_are_listed_until_acknowledged() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let run = Uuid::new_v4();
    store.insert_report(&[
        ReportEntry::info(run, "warehouse_catalog_created", "warehouse a", "a is now catalog a"),
        ReportEntry::warning(run, "backfill_failed", "engine x catalog y", "not migrated").detail(json!({ "why": "test" })),
    ]).await.unwrap();
    let open = store.list_report(false).await.unwrap();
    assert!(open.iter().filter(|r| r.run_id == run).count() == 2);
    assert!(store.acknowledge_report().await.unwrap() >= 2);
    assert!(store.list_report(false).await.unwrap().iter().all(|r| r.run_id != run));
    assert_eq!(store.list_report(true).await.unwrap().iter().filter(|r| r.run_id == run).count(), 2);
}
```

(Add `#[allow(clippy::…)]` nowhere; if a line is too long, `cargo fmt` reflows it.)

- [ ] **Step 4: Run to see them fail**

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry --test store`
Expected: compile error — `store`, `report`, `RegistryStore`, `RegistryError` do not exist.

- [ ] **Step 5: Implement `src/store.rs`**

```rust
//! `registry` schema store (SP3). Runtime sqlx queries, schema-qualified.
//! Reads `engine_mgmt.engines` / `engine_mgmt.engine_catalogs`,
//! `catalog.warehouses` and `workspaces` by name for joins; writes only
//! `registry.*`.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::config::{CatalogDef, ValidationError};
use crate::error::RegistryError;
use crate::name::{first_free_name, is_valid_catalog_name, normalize_catalog_name};
use crate::report::{ReportEntry, ReportRow};

macro_rules! catalog_cols {
    () => {
        "id, name, kind, config, advanced, description, warehouse_id, storage_connection_id, created_at, updated_at"
    };
}
macro_rules! attachment_cols {
    () => {
        "engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, apply_status, apply_error, applied_at, created_at"
    };
}

pub mod apply_status {
    pub const APPLIED: &str = "applied";
    pub const FAILED: &str = "failed";
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub advanced: serde_json::Value,
    pub description: Option<String>,
    pub warehouse_id: Option<Uuid>,
    pub storage_connection_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CatalogRow {
    pub fn def(&self) -> Result<CatalogDef, ValidationError> {
        CatalogDef::parse(&self.kind, &self.config, &self.advanced)
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AttachmentRow {
    pub engine_id: Uuid,
    pub catalog_id: Uuid,
    pub catalog_client_id: Option<Uuid>,
    pub credential_secret_id: Option<Uuid>,
    pub minted_client: bool,
    pub apply_status: String,
    pub apply_error: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl AttachmentRow {
    /// An attachment not yet written (attach renders it before committing it).
    pub fn pending(engine_id: Uuid, catalog_id: Uuid, catalog_client_id: Option<Uuid>, credential_secret_id: Option<Uuid>) -> Self {
        AttachmentRow {
            engine_id,
            catalog_id,
            catalog_client_id,
            credential_secret_id,
            minted_client: catalog_client_id.is_some(),
            apply_status: apply_status::APPLIED.into(),
            apply_error: None,
            applied_at: None,
            created_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AttachedCatalog {
    pub catalog: CatalogRow,
    pub attachment: AttachmentRow,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogEngineRow {
    pub engine_id: Uuid,
    pub engine_name: String,
    pub engine_type: String,
    pub apply_status: String,
    pub apply_error: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogWorkspaceRow {
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    pub assigned_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewAttachment {
    pub engine_id: Uuid,
    pub catalog_id: Uuid,
    pub catalog_client_id: Option<Uuid>,
    pub credential_secret_id: Option<Uuid>,
    pub minted_client: bool,
    /// Set `applied_at = now()` (the engine already accepted this attachment).
    pub applied_now: bool,
    /// Keep a legacy row's creation time, so attachment order — and with it the
    /// rendered env-var names — is unchanged after the migration.
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
pub struct WarehouseIdentity<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub workspace_id: Uuid,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogFilter {
    pub workspace_id: Option<Uuid>,
    pub secret_id: Option<Uuid>,
}

fn unique_constraint(e: &sqlx::Error) -> Option<String> {
    let d = e.as_database_error()?;
    (d.code().as_deref() == Some("23505")).then(|| d.constraint().map(str::to_owned))?
}

fn is_fk_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23503")
}

fn map_insert_error(e: sqlx::Error, name: &str) -> RegistryError {
    match unique_constraint(&e).as_deref() {
        Some("catalogs_name_key") => RegistryError::AlreadyExists(format!("catalog {name}")),
        Some("catalogs_warehouse_id_key") => RegistryError::AlreadyExists("this warehouse already has a catalog".into()),
        _ => RegistryError::Db(e),
    }
}

pub(crate) async fn insert_catalog(conn: &mut PgConnection, name: &str, def: &CatalogDef, description: Option<&str>) -> Result<CatalogRow, sqlx::Error> {
    let row = sqlx::query_as::<_, CatalogRow>(concat!(
        "INSERT INTO registry.catalogs (name, kind, config, advanced, description, warehouse_id, storage_connection_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING ",
        catalog_cols!()
    ))
    .bind(name)
    .bind(def.kind().as_str())
    .bind(def.config_json())
    .bind(def.advanced_json())
    .bind(description)
    .bind(def.warehouse_id())
    .bind(def.storage_connection_id())
    .fetch_one(&mut *conn)
    .await?;
    replace_secret_refs(conn, row.id, &def.secret_ids()).await?;
    Ok(row)
}

async fn replace_secret_refs(conn: &mut PgConnection, catalog_id: Uuid, ids: &BTreeSet<Uuid>) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM registry.catalog_secret_refs WHERE catalog_id = $1")
        .bind(catalog_id)
        .execute(&mut *conn)
        .await?;
    let ids: Vec<Uuid> = ids.iter().copied().collect();
    sqlx::query("INSERT INTO registry.catalog_secret_refs (catalog_id, secret_id) SELECT $1, unnest($2::uuid[])")
        .bind(catalog_id)
        .bind(ids)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(crate) async fn insert_attachment_row(conn: &mut PgConnection, a: &NewAttachment) -> Result<AttachmentRow, sqlx::Error> {
    sqlx::query_as::<_, AttachmentRow>(concat!(
        "INSERT INTO registry.engine_catalog_attachments \
           (engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, applied_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END, COALESCE($7, now())) RETURNING ",
        attachment_cols!()
    ))
    .bind(a.engine_id)
    .bind(a.catalog_id)
    .bind(a.catalog_client_id)
    .bind(a.credential_secret_id)
    .bind(a.minted_client)
    .bind(a.applied_now)
    .bind(a.created_at)
    .fetch_one(&mut *conn)
    .await
}

pub(crate) async fn map_legacy_row_on(conn: &mut PgConnection, engine_catalog_id: Uuid, engine_id: Uuid, catalog_id: Option<Uuid>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO registry.legacy_engine_catalogs (engine_catalog_id, engine_id, catalog_id) \
         VALUES ($1, $2, $3) ON CONFLICT (engine_catalog_id) DO NOTHING",
    )
    .bind(engine_catalog_id)
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(Clone)]
pub struct RegistryStore {
    db: Db,
}

impl RegistryStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub(crate) fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    /// The database clock (what `updated_at` is written with).
    pub async fn db_now(&self) -> Result<DateTime<Utc>, RegistryError> {
        Ok(sqlx::query_scalar("SELECT now()").fetch_one(self.pool()).await?)
    }

    // catalogs -----------------------------------------------------------------

    pub async fn create_catalog(&self, name: &str, def: &CatalogDef, description: Option<&str>) -> Result<CatalogRow, RegistryError> {
        if !is_valid_catalog_name(name) {
            return Err(ValidationError::Name(name.to_string()).into());
        }
        let mut tx = self.pool().begin().await?;
        let row = insert_catalog(&mut tx, name, def, description).await.map_err(|e| map_insert_error(e, name))?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get_catalog(&self, id: Uuid) -> Result<Option<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(concat!("SELECT ", catalog_cols!(), " FROM registry.catalogs WHERE id = $1"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?)
    }

    pub async fn get_catalog_by_name(&self, name: &str) -> Result<Option<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(concat!("SELECT ", catalog_cols!(), " FROM registry.catalogs WHERE name = $1"))
            .bind(name)
            .fetch_optional(self.pool())
            .await?)
    }

    pub async fn get_catalog_for_warehouse(&self, warehouse_id: Uuid) -> Result<Option<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(concat!("SELECT ", catalog_cols!(), " FROM registry.catalogs WHERE warehouse_id = $1"))
            .bind(warehouse_id)
            .fetch_optional(self.pool())
            .await?)
    }

    /// Catalogs, optionally only those assigned to a workspace and/or referencing
    /// a secret (config reference or an attachment's minted credential).
    pub async fn list_catalogs(&self, filter: &CatalogFilter) -> Result<Vec<CatalogRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogRow>(
            "SELECT c.id, c.name, c.kind, c.config, c.advanced, c.description, c.warehouse_id, \
                    c.storage_connection_id, c.created_at, c.updated_at \
               FROM registry.catalogs c \
              WHERE ($1::uuid IS NULL OR EXISTS (SELECT 1 FROM registry.catalog_workspaces w \
                                                  WHERE w.catalog_id = c.id AND w.workspace_id = $1)) \
                AND ($2::uuid IS NULL \
                     OR EXISTS (SELECT 1 FROM registry.catalog_secret_refs r WHERE r.catalog_id = c.id AND r.secret_id = $2) \
                     OR EXISTS (SELECT 1 FROM registry.engine_catalog_attachments a \
                                 WHERE a.catalog_id = c.id AND a.credential_secret_id = $2)) \
              ORDER BY c.name",
        )
        .bind(filter.workspace_id)
        .bind(filter.secret_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Replace config/advanced/description (kind and name never change).
    pub async fn update_catalog(&self, id: Uuid, def: &CatalogDef, description: Option<&str>) -> Result<Option<CatalogRow>, RegistryError> {
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query_as::<_, CatalogRow>(concat!(
            "UPDATE registry.catalogs SET config = $2, advanced = $3, description = $4, \
                    warehouse_id = $5, storage_connection_id = $6, updated_at = now() \
              WHERE id = $1 AND kind = $7 RETURNING ",
            catalog_cols!()
        ))
        .bind(id)
        .bind(def.config_json())
        .bind(def.advanced_json())
        .bind(description)
        .bind(def.warehouse_id())
        .bind(def.storage_connection_id())
        .bind(def.kind().as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(r) = &row {
            replace_secret_refs(&mut tx, r.id, &def.secret_ids()).await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    /// `Conflict` while an attachment or assignment still references it.
    pub async fn delete_catalog(&self, id: Uuid) -> Result<bool, RegistryError> {
        match sqlx::query("DELETE FROM registry.catalogs WHERE id = $1").bind(id).execute(self.pool()).await {
            Ok(r) => Ok(r.rows_affected() == 1),
            Err(e) if is_fk_violation(&e) => Err(RegistryError::Conflict("the catalog is still attached or assigned".into())),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) async fn names_starting_with(&self, prefix: &str) -> Result<HashSet<String>, RegistryError> {
        // left(), not LIKE: `_` is a LIKE wildcard and catalog names are full of it.
        let names: Vec<String> = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE left(name, length($1)) = $1")
            .bind(prefix)
            .fetch_all(self.pool())
            .await?;
        Ok(names.into_iter().collect())
    }

    /// The catalog of a warehouse, created (normalised name, assigned to the
    /// warehouse's workspace) the first time. The assignment is written only with
    /// the catalog, so an admin's later un-assignment is never undone here.
    /// `preferred_name` is used verbatim when free (the backfill's `egret`).
    pub async fn ensure_warehouse_catalog(&self, wh: &WarehouseIdentity<'_>, preferred_name: Option<&str>) -> Result<(CatalogRow, bool), RegistryError> {
        if let Some(c) = self.get_catalog_for_warehouse(wh.id).await? {
            return Ok((c, false));
        }
        let def = CatalogDef::egret_warehouse(wh.id);
        let base = preferred_name.map(str::to_string).unwrap_or_else(|| normalize_catalog_name(wh.name));
        for _ in 0..32 {
            let taken = self.names_starting_with(&base).await?;
            let name = first_free_name(&base, |n| taken.contains(n));
            let mut tx = self.pool().begin().await?;
            match insert_catalog(&mut tx, &name, &def, None).await {
                Ok(row) => {
                    sqlx::query(
                        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2) \
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(row.id)
                    .bind(wh.workspace_id)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok((row, true));
                }
                Err(e) => {
                    tx.rollback().await?;
                    match unique_constraint(&e).as_deref() {
                        Some("catalogs_name_key") => continue,
                        Some("catalogs_warehouse_id_key") => {
                            let c = self.get_catalog_for_warehouse(wh.id).await?.ok_or_else(|| RegistryError::NotFound(format!("catalog of warehouse {}", wh.id)))?;
                            return Ok((c, false));
                        }
                        _ => return Err(e.into()),
                    }
                }
            }
        }
        Err(RegistryError::Conflict(format!("no free catalog name for warehouse {}", wh.name)))
    }

    /// Clear a warehouse catalog's workspace assignments so the warehouse (and,
    /// by cascade, its catalog) can be deleted from the legacy Warehouses page.
    pub async fn release_warehouse_catalog(&self, warehouse_id: Uuid) -> Result<(), RegistryError> {
        sqlx::query(
            "DELETE FROM registry.catalog_workspaces \
              WHERE catalog_id IN (SELECT id FROM registry.catalogs WHERE warehouse_id = $1)",
        )
        .bind(warehouse_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    // attachments --------------------------------------------------------------

    pub async fn insert_attachment(&self, a: &NewAttachment) -> Result<AttachmentRow, RegistryError> {
        let mut conn = self.pool().acquire().await?;
        insert_attachment_row(&mut conn, a).await.map_err(|e| match unique_constraint(&e).as_deref() {
            Some("engine_catalog_attachments_pkey") => RegistryError::AlreadyExists("the catalog is already attached to this engine".into()),
            _ => RegistryError::Db(e),
        })
    }

    pub async fn get_attachment(&self, engine_id: Uuid, catalog_id: Uuid) -> Result<Option<AttachmentRow>, RegistryError> {
        Ok(sqlx::query_as::<_, AttachmentRow>(concat!(
            "SELECT ", attachment_cols!(), " FROM registry.engine_catalog_attachments WHERE engine_id = $1 AND catalog_id = $2"
        ))
        .bind(engine_id)
        .bind(catalog_id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_attachment(&self, engine_id: Uuid, catalog_id: Uuid) -> Result<bool, RegistryError> {
        let r = sqlx::query("DELETE FROM registry.engine_catalog_attachments WHERE engine_id = $1 AND catalog_id = $2")
            .bind(engine_id)
            .bind(catalog_id)
            .execute(self.pool())
            .await?;
        Ok(r.rows_affected() == 1)
    }

    /// An engine's catalogs in render order: its primary (the catalog of
    /// `primary_warehouse`) first, then by attachment time, then name — the
    /// order `engine_catalogs` rendered in, so env-var names stay put.
    pub async fn list_engine_attachments(&self, engine_id: Uuid, primary_warehouse: Option<Uuid>) -> Result<Vec<AttachedCatalog>, RegistryError> {
        let attachments = sqlx::query_as::<_, AttachmentRow>(
            "SELECT a.engine_id, a.catalog_id, a.catalog_client_id, a.credential_secret_id, a.minted_client, \
                    a.apply_status, a.apply_error, a.applied_at, a.created_at \
               FROM registry.engine_catalog_attachments a JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = $1 \
              ORDER BY (c.warehouse_id IS NOT DISTINCT FROM $2) DESC, a.created_at, c.name",
        )
        .bind(engine_id)
        .bind(primary_warehouse)
        .fetch_all(self.pool())
        .await?;
        let ids: Vec<Uuid> = attachments.iter().map(|a| a.catalog_id).collect();
        let catalogs = sqlx::query_as::<_, CatalogRow>(concat!("SELECT ", catalog_cols!(), " FROM registry.catalogs WHERE id = ANY($1)"))
            .bind(ids)
            .fetch_all(self.pool())
            .await?;
        attachments
            .into_iter()
            .map(|attachment| {
                let catalog = catalogs
                    .iter()
                    .find(|c| c.id == attachment.catalog_id)
                    .cloned()
                    .ok_or_else(|| RegistryError::NotFound(format!("catalog {}", attachment.catalog_id)))?;
                Ok(AttachedCatalog { catalog, attachment })
            })
            .collect()
    }

    /// The engines a catalog is attached to, with their apply status.
    pub async fn list_catalog_attachments(&self, catalog_id: Uuid) -> Result<Vec<CatalogEngineRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogEngineRow>(
            "SELECT a.engine_id, e.name AS engine_name, e.type AS engine_type, a.apply_status, a.apply_error, a.applied_at \
               FROM registry.engine_catalog_attachments a JOIN engine_mgmt.engines e ON e.id = a.engine_id \
              WHERE a.catalog_id = $1 ORDER BY e.name",
        )
        .bind(catalog_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn attached_catalog_names(&self, engine_id: Uuid) -> Result<Vec<String>, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT c.name FROM registry.engine_catalog_attachments a JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = $1 ORDER BY c.name",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// After a successful full apply: every attachment whose catalog the apply
    /// rendered (unchanged since `read_at`) is on the engine now.
    pub async fn mark_engine_applied(&self, engine_id: Uuid, read_at: DateTime<Utc>) -> Result<(), RegistryError> {
        sqlx::query(
            "UPDATE registry.engine_catalog_attachments a \
                SET apply_status = 'applied', apply_error = NULL, applied_at = now() \
               FROM registry.catalogs c \
              WHERE c.id = a.catalog_id AND a.engine_id = $1 AND c.updated_at <= $2",
        )
        .bind(engine_id)
        .bind(read_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn set_apply_failed(&self, engine_id: Uuid, catalog_id: Uuid, error: &str) -> Result<(), RegistryError> {
        sqlx::query(
            "UPDATE registry.engine_catalog_attachments SET apply_status = 'failed', apply_error = $3 \
              WHERE engine_id = $1 AND catalog_id = $2",
        )
        .bind(engine_id)
        .bind(catalog_id)
        .bind(error)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    // workspaces ---------------------------------------------------------------

    pub async fn list_catalog_workspaces(&self, catalog_id: Uuid) -> Result<Vec<CatalogWorkspaceRow>, RegistryError> {
        Ok(sqlx::query_as::<_, CatalogWorkspaceRow>(
            "SELECT cw.workspace_id, w.name, w.slug, cw.assigned_at \
               FROM registry.catalog_workspaces cw JOIN workspaces w ON w.id = cw.workspace_id \
              WHERE cw.catalog_id = $1 ORDER BY w.name",
        )
        .bind(catalog_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Replace the full assignment set.
    pub async fn set_catalog_workspaces(&self, catalog_id: Uuid, workspace_ids: &[Uuid]) -> Result<(), RegistryError> {
        let ids = workspace_ids.to_vec();
        let mut tx = self.pool().begin().await?;
        sqlx::query("DELETE FROM registry.catalog_workspaces WHERE catalog_id = $1 AND NOT (workspace_id = ANY($2))")
            .bind(catalog_id)
            .bind(ids.clone())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) \
             SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
        )
        .bind(catalog_id)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Whether a workspace may use a warehouse (notebook warehouse selection,
    /// §4.6): its catalog is assigned there — or, for a warehouse the backfill
    /// has not reached yet, the warehouse's own workspace.
    pub async fn warehouse_visible_to_workspace(&self, warehouse_id: Uuid, workspace_id: Uuid) -> Result<bool, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM registry.catalogs c JOIN registry.catalog_workspaces cw ON cw.catalog_id = c.id \
                             WHERE c.warehouse_id = $1 AND cw.workspace_id = $2) \
                 OR (NOT EXISTS (SELECT 1 FROM registry.catalogs c WHERE c.warehouse_id = $1) \
                     AND EXISTS (SELECT 1 FROM catalog.warehouses w WHERE w.id = $1 AND w.workspace_id = $2))",
        )
        .bind(warehouse_id)
        .bind(workspace_id)
        .fetch_one(self.pool())
        .await?)
    }

    // legacy map + report ------------------------------------------------------

    /// `engine_mgmt.engine_catalogs` rows of this engine the backfill has not
    /// migrated. Non-zero means rendering from the registry would drop catalogs
    /// the engine runs today, so applies refuse (engines.rs).
    pub async fn unmigrated_legacy_catalog_count(&self, engine_id: Uuid) -> Result<i64, RegistryError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engine_catalogs c \
              WHERE c.engine_id = $1 \
                AND NOT EXISTS (SELECT 1 FROM registry.legacy_engine_catalogs m WHERE m.engine_catalog_id = c.id)",
        )
        .bind(engine_id)
        .fetch_one(self.pool())
        .await?)
    }

    pub async fn insert_report(&self, entries: &[ReportEntry]) -> Result<(), RegistryError> {
        let mut tx = self.pool().begin().await?;
        for e in entries {
            sqlx::query(
                "INSERT INTO registry.migration_report \
                   (run_id, severity, code, catalog_id, engine_id, warehouse_id, subject, message, detail) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(e.run_id)
            .bind(e.severity.as_str())
            .bind(e.code)
            .bind(e.catalog_id)
            .bind(e.engine_id)
            .bind(e.warehouse_id)
            .bind(&e.subject)
            .bind(&e.message)
            .bind(&e.detail)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_report(&self, include_acknowledged: bool) -> Result<Vec<ReportRow>, RegistryError> {
        Ok(sqlx::query_as::<_, ReportRow>(
            "SELECT id, run_id, created_at, severity, code, catalog_id, engine_id, warehouse_id, subject, message, \
                    detail, acknowledged_at \
               FROM registry.migration_report \
              WHERE $1 OR acknowledged_at IS NULL ORDER BY id",
        )
        .bind(include_acknowledged)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn acknowledge_report(&self) -> Result<u64, RegistryError> {
        Ok(sqlx::query("UPDATE registry.migration_report SET acknowledged_at = now() WHERE acknowledged_at IS NULL")
            .execute(self.pool())
            .await?
            .rows_affected())
    }
}
```

`sqlx::Transaction` derefs to `PgConnection`, so `insert_catalog(&mut tx, …)` compiles through auto-deref; if the compiler asks for it, write `&mut *tx`.

In `src/lib.rs` add `pub mod error; pub mod report; pub mod store;` and:

```rust
pub use error::RegistryError;
pub use report::{ReportEntry, ReportRow, Severity};
pub use store::{
    AttachedCatalog, AttachmentRow, CatalogEngineRow, CatalogFilter, CatalogRow,
    CatalogWorkspaceRow, NewAttachment, RegistryStore, WarehouseIdentity,
};
```

- [ ] **Step 6: Count registry references in `used_by`** — in `crates/egret-secrets/src/workspace_secrets.rs`, in **both** `list` and `get_meta`, replace

```rust
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) AS used_by, \
```

with

```rust
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) \
             + (SELECT count(*) FROM registry.catalog_secret_refs WHERE secret_id = ws.id) \
             + (SELECT count(*) FROM registry.engine_catalog_attachments WHERE credential_secret_id = ws.id) AS used_by, \
```

and the `used_by` field doc to `/// References to this secret: storage and database connections, registry catalogs, and engine catalog credentials.`

- [ ] **Step 7: Accessors** — `crates/egret-engine-mgmt/src/store.rs`, inside `impl EngineMgmtStore` after `pool()`:

```rust
    /// The catalog registry on the same database (SP3).
    pub fn registry(&self) -> egret_catalog_registry::RegistryStore {
        egret_catalog_registry::RegistryStore::new(self.db.clone())
    }
```

`crates/egret-catalog-engine/src/store/handle.rs`, inside `impl CatalogStore` after `pool()`:

```rust
    /// The catalog registry on the same database (SP3).
    pub fn registry(&self) -> egret_catalog_registry::RegistryStore {
        egret_catalog_registry::RegistryStore::new(self.db.clone())
    }
```

`crates/egret-engine-mgmt/src/http_error.rs`, after `impl From<crate::EngineMgmtError> for EngineHttpError`:

```rust
impl From<egret_catalog_registry::RegistryError> for EngineHttpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        use egret_catalog_registry::RegistryError as E;
        match e {
            E::NotFound(_) => EngineHttpError::NotFound,
            E::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            E::Conflict(s) => EngineHttpError::Conflict(s),
            E::Invalid(v) => EngineHttpError::BadRequest(v.to_string()),
            E::Secrets(s) => s.into(),
            E::Db(e) => db_error(e),
        }
    }
}
```

- [ ] **Step 8: Run the store tests**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry --test store
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-secrets
```
Expected: 4 store tests pass (seconds, not milliseconds); `egret-secrets` still green.

- [ ] **Step 9: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry -p egret-secrets -p egret-engine-mgmt -p egret-catalog-engine --all-targets -- -D warnings
git branch --show-current
git add Cargo.lock crates/egret-catalog-registry crates/egret-secrets/src/workspace_secrets.rs \
  crates/egret-engine-mgmt/Cargo.toml crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/src/http_error.rs \
  crates/egret-catalog-engine/Cargo.toml crates/egret-catalog-engine/src/store/handle.rs
git commit -m "feat(catalog-registry): registry store

Catalog CRUD with secret references, warehouse catalogs created once
with their workspace, attachments in render order with apply status,
workspace assignment, the legacy-row map and the migration report. A
secret a catalog or an engine credential references can no longer be
deleted.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 6: Boot backfill, saved-SQL scan, migration report

**Files:**
- Create: `crates/egret-catalog-registry/src/scan.rs`, `src/backfill.rs`, `tests/backfill_single_primary.rs`, `tests/backfill_several_primaries.rs`
- Modify: `crates/egret-catalog-registry/src/lib.rs`, `src/store.rs` (two backfill transactions); `crates/egret-bin/Cargo.toml`, `crates/egret-bin/src/main.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: `scan::{scan_saved_sql, SavedSqlHit}` — `scan_saved_sql(pool: &PgPool, catalog_name: &str) -> Result<Vec<SavedSqlHit>, sqlx::Error>`, `SavedSqlHit { object_type: &'static str, object_id: Uuid, workspace_id: Uuid, name: String }`; `backfill::{run_backfill, BackfillSummary, BACKFILL_LOCK_KEY}` — `run_backfill(store: &RegistryStore, secrets: &WorkspaceSecretStore) -> Result<BackfillSummary, RegistryError>`, `BackfillSummary { run_id: Uuid, report_rows: usize, failed_rows: usize }`.

What the backfill does (spec §4.8), in order, every boot, under `pg_advisory_lock(BACKFILL_LOCK_KEY)`:

1. Every warehouse without a registry catalog gets one (`ensure_warehouse_catalog`). If exactly one warehouse is an engine primary (`engine_catalogs` rows `is_primary AND kind = 'egret-warehouse'`, joined to `engines.warehouse_id`) and no catalog is called `egret` yet, that warehouse's catalog is named `egret`.
2. Every `engine_mgmt.engine_catalogs` row without a `registry.legacy_engine_catalogs` entry, ordered `is_primary DESC, created_at, id`:
   - `egret-warehouse`: resolve the warehouse (primary → `engines.warehouse_id`; otherwise the minted client's `catalog_clients.warehouse_id`, then the unique warehouse named `config.warehouse`); if the engine already has that catalog attached, map the row and report `duplicate_attachment_skipped`; else move `config.credential` into the global secret `catalog-client-<clientRowId>` and write attachment + map in one transaction.
   - any other kind: same normalised name + identical `(kind, config, secret_ref)` seen earlier in this run → attach the shared catalog; name taken → `<name>_<engine>` (then `_2`…); move plaintext secrets into `catalog-<name>-<property>` secrets; `classify`; write catalog + attachment + map in one transaction.
   - a row whose migration fails is reported `backfill_failed` and left unmapped; engine applies refuse until a later boot migrates it (Task 7).
3. For every legacy name whose SQL name changed, scan saved SQL and report each object.
4. Write the report rows (and log each one).

The advisory lock serialises replicas. Secrets use deterministic names and are reused on `AlreadyExists`, so a crash between a secret write and its transaction is harmless on the next boot.

- [ ] **Step 1: Backfill transactions in the store** — append to `impl RegistryStore` in `src/store.rs`:

```rust
    /// Backfill: attach + mark the legacy row migrated, atomically.
    pub(crate) async fn insert_attachment_and_map(&self, a: &NewAttachment, engine_catalog_id: Uuid) -> Result<(), RegistryError> {
        let mut tx = self.pool().begin().await?;
        insert_attachment_row(&mut tx, a).await?;
        map_legacy_row_on(&mut tx, engine_catalog_id, a.engine_id, Some(a.catalog_id)).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Backfill: mark a legacy row migrated without writing an attachment.
    pub(crate) async fn map_legacy_row(&self, engine_catalog_id: Uuid, engine_id: Uuid, catalog_id: Option<Uuid>) -> Result<(), RegistryError> {
        let mut conn = self.pool().acquire().await?;
        map_legacy_row_on(&mut conn, engine_catalog_id, engine_id, catalog_id).await?;
        Ok(())
    }

    /// Backfill: catalog + attachment + map, atomically.
    pub(crate) async fn create_catalog_for_legacy_row(
        &self,
        name: &str,
        def: &CatalogDef,
        engine_id: Uuid,
        engine_catalog_id: Uuid,
        created_at: DateTime<Utc>,
    ) -> Result<CatalogRow, RegistryError> {
        let mut tx = self.pool().begin().await?;
        let row = insert_catalog(&mut tx, name, def, None).await.map_err(|e| map_insert_error(e, name))?;
        insert_attachment_row(&mut tx, &NewAttachment {
            engine_id,
            catalog_id: row.id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: false,
            created_at: Some(created_at),
        })
        .await?;
        map_legacy_row_on(&mut tx, engine_catalog_id, engine_id, Some(row.id)).await?;
        tx.commit().await?;
        Ok(row)
    }
```

- [ ] **Step 2: `src/scan.rs`**

```rust
//! Saved SQL that names a catalog (spec §4.8.2). Charts hold no SQL — they are
//! reached through their dataset. Matching is textual: `<name>.` or `"<name>".`
//! not preceded by an identifier character, case-insensitive.

use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
struct Hit {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedSqlHit {
    pub object_type: &'static str,
    pub object_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
}

pub async fn scan_saved_sql(pool: &sqlx::PgPool, catalog_name: &str) -> Result<Vec<SavedSqlHit>, sqlx::Error> {
    let pattern = format!(r#"(^|[^a-z0-9_"])"?{}"?\s*\."#, regex::escape(catalog_name));
    let mut out = Vec::new();
    let mut push = |object_type: &'static str, hits: Vec<Hit>| {
        out.extend(hits.into_iter().map(|h| SavedSqlHit { object_type, object_id: h.id, workspace_id: h.workspace_id, name: h.name }));
    };
    push("dataset", sqlx::query_as(
        "SELECT id, workspace_id, name FROM analytics.datasets \
          WHERE deleted_at IS NULL AND (sql ~* $1 OR lower(catalog) = lower($2)) ORDER BY workspace_id, name")
        .bind(&pattern).bind(catalog_name).fetch_all(pool).await?);
    push("saved_query", sqlx::query_as(
        "SELECT id, workspace_id, name FROM analytics.saved_queries WHERE sql ~* $1 ORDER BY workspace_id, name")
        .bind(&pattern).fetch_all(pool).await?);
    push("sql_snippet", sqlx::query_as(
        "SELECT id, workspace_id, name FROM analytics.sql_snippets WHERE body ~* $1 ORDER BY workspace_id, name")
        .bind(&pattern).fetch_all(pool).await?);
    push("alert_or_report", sqlx::query_as(
        "SELECT id, workspace_id, name FROM automation.schedules \
          WHERE deleted_at IS NULL AND sql ~* $1 ORDER BY workspace_id, name")
        .bind(&pattern).fetch_all(pool).await?);
    push("data_api_endpoint_draft", sqlx::query_as(
        "SELECT e.id, e.workspace_id, e.name FROM dataapi.endpoints e \
          WHERE e.deleted_at IS NULL AND EXISTS ( \
                SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(e.statements) = 'array' \
                                                        THEN e.statements ELSE '[]'::jsonb END) s \
                 WHERE s->>'sql' ~* $1) \
          ORDER BY e.workspace_id, e.name")
        .bind(&pattern).fetch_all(pool).await?);
    push("data_api_endpoint_live", sqlx::query_as(
        "SELECT e.id, e.workspace_id, e.name FROM dataapi.endpoints e \
           JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id \
          WHERE e.deleted_at IS NULL AND ((v.snapshot->>'sql') ~* $1 OR EXISTS ( \
                SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(v.snapshot->'statements') = 'array' \
                                                        THEN v.snapshot->'statements' ELSE '[]'::jsonb END) s \
                 WHERE s->>'sql' ~* $1)) \
          ORDER BY e.workspace_id, e.name")
        .bind(&pattern).fetch_all(pool).await?);
    Ok(out)
}
```

- [ ] **Step 3: `src/backfill.rs`**

```rust
//! Boot-time backfill of the catalog registry (spec §4.8). Runs on every boot,
//! under an advisory lock, and only ever adds: a warehouse without a catalog gets
//! one; an `engine_mgmt.engine_catalogs` row without a
//! `registry.legacy_engine_catalogs` entry is migrated once. Anything an admin
//! removes later is therefore never recreated.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use serde_json::json;
use uuid::Uuid;

use crate::legacy::{classify, plaintext_secrets, ClassifyOutcome, LegacyCatalog};
use crate::name::{first_free_name, join_truncated, normalize_catalog_name};
use crate::report::ReportEntry;
use crate::store::{NewAttachment, RegistryStore, WarehouseIdentity};
use crate::RegistryError;

/// `pg_advisory_lock` key ("SP3A") serialising replicas booting together.
pub const BACKFILL_LOCK_KEY: i64 = 0x5350_3341;
const EGRET: &str = "egret";
const EXTRACTED: &str = "Moved out of an engine catalog's configuration by the SP3 registry migration";

#[derive(Debug)]
pub struct BackfillSummary {
    pub run_id: Uuid,
    pub report_rows: usize,
    pub failed_rows: usize,
}

#[derive(sqlx::FromRow)]
struct WarehouseRow {
    id: Uuid,
    name: String,
    workspace_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct LegacyRow {
    id: Uuid,
    engine_id: Uuid,
    engine_name: String,
    engine_warehouse_id: Uuid,
    name: String,
    kind: String,
    config: serde_json::Value,
    secret_ref: Option<Uuid>,
    is_primary: bool,
    created_at: DateTime<Utc>,
}

pub async fn run_backfill(store: &RegistryStore, secrets: &WorkspaceSecretStore) -> Result<BackfillSummary, RegistryError> {
    let mut lock = store.pool().acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)").bind(BACKFILL_LOCK_KEY).execute(&mut *lock).await?;
    let result = run_locked(store, secrets).await;
    if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)").bind(BACKFILL_LOCK_KEY).execute(&mut *lock).await {
        tracing::warn!(error = %e, "catalog registry backfill: advisory unlock failed");
    }
    result
}

async fn run_locked(store: &RegistryStore, secrets: &WorkspaceSecretStore) -> Result<BackfillSummary, RegistryError> {
    let run_id = Uuid::new_v4();
    let mut report: Vec<ReportEntry> = Vec::new();
    let mut failed_rows = 0usize;

    // 1. Warehouses.
    let primaries: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.warehouse_id FROM engine_mgmt.engines e \
           JOIN engine_mgmt.engine_catalogs c ON c.engine_id = e.id \
          WHERE c.is_primary AND c.kind = 'egret-warehouse'",
    )
    .fetch_all(store.pool())
    .await?;
    let egret_warehouse = match primaries.as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    let warehouses: Vec<WarehouseRow> = sqlx::query_as(
        "SELECT w.id, w.name, w.workspace_id FROM catalog.warehouses w \
          WHERE NOT EXISTS (SELECT 1 FROM registry.catalogs c WHERE c.warehouse_id = w.id) \
          ORDER BY w.created_at, w.id",
    )
    .fetch_all(store.pool())
    .await?;
    for wh in &warehouses {
        let preferred = if Some(wh.id) == egret_warehouse && store.get_catalog_by_name(EGRET).await?.is_none() {
            Some(EGRET)
        } else {
            None
        };
        let ident = WarehouseIdentity { id: wh.id, name: &wh.name, workspace_id: wh.workspace_id };
        let (cat, created) = store.ensure_warehouse_catalog(&ident, preferred).await?;
        if !created {
            continue;
        }
        report.push(
            ReportEntry::info(run_id, "warehouse_catalog_created", format!("warehouse {}", wh.name),
                format!("warehouse {} is catalog {} and is assigned to its workspace", wh.name, cat.name))
                .catalog(cat.id)
                .warehouse(wh.id)
                .detail(json!({ "warehouseName": wh.name, "catalogName": cat.name })),
        );
        if preferred.is_some() {
            report.push(
                ReportEntry::info(run_id, "primary_named_egret", format!("warehouse {}", wh.name),
                    format!("warehouse {} is the only engine primary, so its catalog keeps the name egret that saved SQL uses", wh.name))
                    .catalog(cat.id)
                    .warehouse(wh.id),
            );
        }
    }

    // 2. Legacy engine catalogs.
    let rows: Vec<LegacyRow> = sqlx::query_as(
        "SELECT c.id, c.engine_id, e.name AS engine_name, e.warehouse_id AS engine_warehouse_id, c.name, c.kind, \
                c.config, c.secret_ref, c.is_primary, c.created_at \
           FROM engine_mgmt.engine_catalogs c JOIN engine_mgmt.engines e ON e.id = c.engine_id \
          WHERE NOT EXISTS (SELECT 1 FROM registry.legacy_engine_catalogs m WHERE m.engine_catalog_id = c.id) \
          ORDER BY c.is_primary DESC, c.created_at, c.id",
    )
    .fetch_all(store.pool())
    .await?;
    let mut shared: HashMap<String, (String, Uuid)> = HashMap::new();
    let mut renames: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in &rows {
        let result = if row.kind == "egret-warehouse" {
            migrate_warehouse_row(store, secrets, run_id, row, &mut report).await
        } else {
            migrate_connector_row(store, secrets, run_id, row, &mut shared, &mut report).await
        };
        match result {
            Ok((final_name, catalog_id)) => {
                if final_name != row.name {
                    report.push(
                        ReportEntry::warning(run_id, "engine_catalog_renamed",
                            format!("engine {} catalog {}", row.engine_name, row.name),
                            format!("engine {} now reaches catalog {} as {}; queries that name {} fail from its next apply",
                                row.engine_name, row.name, final_name, row.name))
                            .engine(row.engine_id)
                            .catalog(catalog_id)
                            .detail(json!({ "oldName": row.name, "newName": final_name, "primary": row.is_primary })),
                    );
                    renames.entry(row.name.clone()).or_default().insert(final_name);
                }
            }
            Err(e) => {
                failed_rows += 1;
                report.push(
                    ReportEntry::warning(run_id, "backfill_failed",
                        format!("engine {} catalog {}", row.engine_name, row.name),
                        format!("not migrated: {e}. Engine {} refuses applies until a later boot migrates it", row.engine_name))
                        .engine(row.engine_id)
                        .detail(json!({ "engineCatalogId": row.id })),
                );
            }
        }
    }

    // 3. Saved SQL that still names a catalog by its old name.
    for (old, new_names) in &renames {
        for hit in crate::scan::scan_saved_sql(store.pool(), old).await? {
            report.push(
                ReportEntry::warning(run_id, "saved_sql_references_catalog",
                    format!("{} {}", hit.object_type, hit.name),
                    format!("{} {} names catalog {old}, which is now {}", hit.object_type, hit.name,
                        new_names.iter().cloned().collect::<Vec<_>>().join(" / ")))
                    .detail(json!({
                        "objectType": hit.object_type, "objectId": hit.object_id,
                        "workspaceId": hit.workspace_id, "oldName": old, "newNames": new_names,
                    })),
            );
        }
    }

    // 4. Report.
    for e in &report {
        match e.severity {
            crate::Severity::Info => tracing::info!(code = e.code, subject = %e.subject, "catalog registry backfill: {}", e.message),
            crate::Severity::Warning => tracing::warn!(code = e.code, subject = %e.subject, "catalog registry backfill: {}", e.message),
        }
    }
    store.insert_report(&report).await?;
    Ok(BackfillSummary { run_id, report_rows: report.len(), failed_rows })
}

async fn create_or_reuse_secret(secrets: &WorkspaceSecretStore, name: &str, value: &str) -> Result<Uuid, RegistryError> {
    match secrets.create(None, name, Some(EXTRACTED), value).await {
        Ok(id) => Ok(id),
        Err(SecretsError::AlreadyExists(_)) => secrets
            .list(None)
            .await?
            .into_iter()
            .find(|m| m.name == name)
            .map(|m| m.id)
            .ok_or_else(|| RegistryError::Conflict(format!("secret {name} exists but cannot be listed"))),
        Err(e) => Err(e.into()),
    }
}

fn client_row_id(config: &serde_json::Value) -> Option<Uuid> {
    config.get("clientRowId").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok())
}

async fn row_warehouse(store: &RegistryStore, row: &LegacyRow) -> Result<Uuid, RegistryError> {
    if row.is_primary {
        return Ok(row.engine_warehouse_id);
    }
    if let Some(cid) = client_row_id(&row.config) {
        let wh: Option<Uuid> = sqlx::query_scalar("SELECT warehouse_id FROM catalog.catalog_clients WHERE id = $1")
            .bind(cid)
            .fetch_optional(store.pool())
            .await?;
        if let Some(wh) = wh {
            return Ok(wh);
        }
    }
    let name = row.config.get("warehouse").and_then(|v| v.as_str())
        .ok_or_else(|| RegistryError::Conflict("the row names no warehouse".into()))?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM catalog.warehouses WHERE name = $1")
        .bind(name)
        .fetch_all(store.pool())
        .await?;
    match ids.as_slice() {
        [only] => Ok(*only),
        [] => Err(RegistryError::NotFound(format!("warehouse {name}"))),
        _ => Err(RegistryError::Conflict(format!("{} warehouses are named {name}", ids.len()))),
    }
}

/// Returns (the name the engine reaches the catalog by now, catalog id).
async fn migrate_warehouse_row(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    row: &LegacyRow,
    report: &mut Vec<ReportEntry>,
) -> Result<(String, Uuid), RegistryError> {
    let warehouse_id = row_warehouse(store, row).await?;
    let wh: WarehouseRow = sqlx::query_as("SELECT id, name, workspace_id FROM catalog.warehouses WHERE id = $1")
        .bind(warehouse_id)
        .fetch_optional(store.pool())
        .await?
        .ok_or_else(|| RegistryError::NotFound(format!("warehouse {warehouse_id}")))?;
    let (cat, _) = store
        .ensure_warehouse_catalog(&WarehouseIdentity { id: wh.id, name: &wh.name, workspace_id: wh.workspace_id }, None)
        .await?;
    let client = client_row_id(&row.config);
    if store.get_attachment(row.engine_id, cat.id).await?.is_some() {
        store.map_legacy_row(row.id, row.engine_id, Some(cat.id)).await?;
        report.push(
            ReportEntry::warning(run_id, "duplicate_attachment_skipped",
                format!("engine {} catalog {}", row.engine_name, row.name),
                format!("engine {} already reaches warehouse {} as catalog {}; its extra catalog {} is dropped at the engine's next apply. \
                         Its Catalog Client was left in place: revoke it on the warehouse's Clients tab afterwards",
                    row.engine_name, wh.name, cat.name, row.name))
                .engine(row.engine_id)
                .catalog(cat.id)
                .warehouse(wh.id)
                .detail(json!({ "catalogClientId": client })),
        );
        return Ok((cat.name, cat.id));
    }
    let credential = row.config.get("credential").and_then(|v| v.as_str()).filter(|s| s.contains(':'))
        .ok_or_else(|| RegistryError::Conflict("the row carries no catalog client credential".into()))?;
    let secret_name = match client {
        Some(id) => format!("catalog-client-{id}"),
        None => format!("catalog-client-legacy-{}", row.id),
    };
    let secret_id = create_or_reuse_secret(secrets, &secret_name, credential).await?;
    store
        .insert_attachment_and_map(
            &NewAttachment {
                engine_id: row.engine_id,
                catalog_id: cat.id,
                catalog_client_id: client,
                credential_secret_id: Some(secret_id),
                minted_client: row.config.get("mintedClient").and_then(|v| v.as_bool()) != Some(false),
                applied_now: false,
                created_at: Some(row.created_at),
            },
            row.id,
        )
        .await?;
    report.push(
        ReportEntry::info(run_id, "secret_extracted", format!("engine {} catalog {}", row.engine_name, row.name),
            format!("the catalog client credential moved into secret {secret_name}"))
            .engine(row.engine_id)
            .catalog(cat.id),
    );
    Ok((cat.name, cat.id))
}

async fn migrate_connector_row(
    store: &RegistryStore,
    secrets: &WorkspaceSecretStore,
    run_id: Uuid,
    row: &LegacyRow,
    shared: &mut HashMap<String, (String, Uuid)>,
    report: &mut Vec<ReportEntry>,
) -> Result<(String, Uuid), RegistryError> {
    let legacy = LegacyCatalog { name: &row.name, kind: &row.kind, config: &row.config, secret_ref: row.secret_ref };
    let key = json!({ "kind": row.kind, "config": row.config, "secretRef": row.secret_ref }).to_string();
    let base = normalize_catalog_name(&row.name);
    let subject = format!("engine {} catalog {}", row.engine_name, row.name);

    if let Some((seen_key, catalog_id)) = shared.get(&base) {
        if *seen_key == key {
            let catalog_id = *catalog_id;
            if store.get_attachment(row.engine_id, catalog_id).await?.is_none() {
                store
                    .insert_attachment_and_map(
                        &NewAttachment {
                            engine_id: row.engine_id,
                            catalog_id,
                            catalog_client_id: None,
                            credential_secret_id: None,
                            minted_client: false,
                            applied_now: false,
                            created_at: Some(row.created_at),
                        },
                        row.id,
                    )
                    .await?;
            } else {
                store.map_legacy_row(row.id, row.engine_id, Some(catalog_id)).await?;
            }
            report.push(
                ReportEntry::info(run_id, "engine_catalog_shared", subject,
                    format!("identical to catalog {base} on another engine; both engines now attach one catalog"))
                    .engine(row.engine_id)
                    .catalog(catalog_id),
            );
            return Ok((base, catalog_id));
        }
    }

    let name = if store.get_catalog_by_name(&base).await?.is_none() {
        base.clone()
    } else {
        let with_engine = join_truncated(&base, &normalize_catalog_name(&row.engine_name));
        let taken = store.names_starting_with(&with_engine).await?;
        let name = first_free_name(&with_engine, |n| taken.contains(n));
        report.push(
            ReportEntry::warning(run_id, "engine_catalog_suffixed", subject.clone(),
                format!("another catalog is already called {base} with a different configuration; this one is {name}"))
                .engine(row.engine_id),
        );
        name
    };

    let mut extracted = BTreeMap::new();
    for (property, value) in plaintext_secrets(&legacy) {
        let secret_name = format!("catalog-{name}-{property}");
        let id = create_or_reuse_secret(secrets, &secret_name, &value).await?;
        extracted.insert(property.clone(), id);
        report.push(
            ReportEntry::info(run_id, "secret_extracted", subject.clone(),
                format!("plaintext {property} moved into secret {secret_name}"))
                .engine(row.engine_id),
        );
    }
    let classified = classify(&legacy, &extracted)?;
    let cat = store
        .create_catalog_for_legacy_row(&name, &classified.def, row.engine_id, row.id, row.created_at)
        .await?;
    report.push(match &classified.outcome {
        ClassifyOutcome::Typed => ReportEntry::info(run_id, "engine_catalog_typed", subject,
            format!("catalog {name} is a typed {} catalog", classified.def.kind().as_str())),
        ClassifyOutcome::Generic { reason } => ReportEntry::info(run_id, "engine_catalog_generic", subject,
            format!("catalog {name} keeps its properties as a generic catalog: {reason}"))
            .detail(json!({ "reason": reason })),
    }
    .engine(row.engine_id)
    .catalog(cat.id));
    shared.insert(base, (key, cat.id));
    Ok((name, cat.id))
}
```

In `src/lib.rs` add `pub mod backfill; pub mod scan;` and `pub use backfill::{run_backfill, BackfillSummary, BACKFILL_LOCK_KEY};`.

`crates/egret-catalog-registry/Cargo.toml`: nothing new (regex, tracing, egret-secrets already there).

- [ ] **Step 4: Write the single-primary test** — `crates/egret-catalog-registry/tests/backfill_single_primary.rs`. One test per binary: the backfill reads the whole database.

```rust
//! Backfill: one engine primary (the kind-cluster shape), name normalisation and
//! suffixing, typed vs generic, shared vs suffixed same-name rows, secret
//! extraction, idempotency. DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, CatalogConfig, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

struct Seed {
    db: Db,
    ws: Uuid,
    sc: Uuid,
}

impl Seed {
    async fn warehouse(&self, name: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id")
            .bind(name).bind(self.ws).bind(self.sc).fetch_one(&self.db.pool).await.unwrap()
    }
    async fn engine(&self, name: &str, wh: Uuid) -> Uuid {
        sqlx::query_scalar("INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id")
            .bind(name).bind(wh).fetch_one(&self.db.pool).await.unwrap()
    }
    async fn legacy(&self, engine: Uuid, name: &str, kind: &str, config: serde_json::Value, primary: bool) -> Uuid {
        sqlx::query_scalar("INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) VALUES ($1, $2, $3, $4, $5) RETURNING id")
            .bind(engine).bind(name).bind(kind).bind(config).bind(primary).fetch_one(&self.db.pool).await.unwrap()
    }
}

async fn catalog_names(db: &Db) -> BTreeSet<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM registry.catalogs").fetch_all(&db.pool).await.unwrap().into_iter().collect()
}

async fn codes(db: &Db, run: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT code FROM registry.migration_report WHERE run_id = $1 ORDER BY id").bind(run).fetch_all(&db.pool).await.unwrap()
}

#[tokio::test]
async fn single_primary_platform_migrates_like_the_kind_cluster() {
    let Some(dsn) = egret_test_db::url().await else { return };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let ws: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug, name, status) VALUES ('bf-one', 'BF', 'active') RETURNING id").fetch_one(&db.pool).await.unwrap();
    let sc: Uuid = sqlx::query_scalar("INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) VALUES ('bf-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id").fetch_one(&db.pool).await.unwrap();
    let s = Seed { db: db.clone(), ws, sc };
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(db.clone(), egret_secrets::SecretStore::new(db.clone(), Some(crypto)));
    let store = RegistryStore::new(db.clone());

    let primary_wh = s.warehouse("e2e-wh-1784523876270").await;
    s.warehouse("e2e-wh-1784519491585").await;
    s.warehouse("Sales Data").await;
    s.warehouse("sales_data").await;
    s.warehouse("1st").await;
    let demo = s.engine("demo-trino", primary_wh).await;
    let other = s.engine("trino-b", primary_wh).await;
    let client = Uuid::new_v4();
    s.legacy(demo, "egret", "egret-warehouse", json!({
        "warehouse": "e2e-wh-1784523876270", "s3_endpoint": "http://minio:9000",
        "credential": "cid:csecret", "clientRowId": client
    }), true).await;
    s.legacy(demo, "bench", "tpch", json!({}), false).await;
    s.legacy(other, "bench", "tpch", json!({}), false).await;
    s.legacy(demo, "pg_main", "postgresql", json!({ "properties": {
        "connection-url": "jdbc:postgresql://db:5432/x", "connection-user": "u", "connection-password": "hunter2" } }), false).await;
    s.legacy(other, "pg_main", "postgresql", json!({ "properties": {
        "connection-url": "jdbc:postgresql://other:5432/y", "connection-user": "u" } }), false).await;
    let pw = vault.create(None, "bf-pg-typed", None, "s3cr3t").await.unwrap();
    s.legacy(demo, "Bad-Name", "postgresql", json!({
        "properties": { "connection-url": "jdbc:postgresql://db:5432/z?sslmode=require", "connection-user": "u" },
        "secretProperties": { "connection-password": pw.to_string() } }), false).await;

    let first = run_backfill(&store, &vault).await.expect("backfill");
    assert_eq!(first.failed_rows, 0);

    // Warehouses: the single primary is `egret`; the rest normalise, collisions suffix.
    assert_eq!(catalog_names(&db).await, [
        "egret", "e2e_wh_1784519491585", "sales_data", "sales_data_2", "w_1st",
        "bench", "pg_main", "pg_main_trino_b", "bad_name",
    ].into_iter().map(String::from).collect());
    let egret = store.get_catalog_by_name("egret").await.unwrap().unwrap();
    assert_eq!(egret.warehouse_id, Some(primary_wh));
    assert_eq!(store.list_catalog_workspaces(egret.id).await.unwrap()[0].workspace_id, ws);

    // The primary attachment carries its client; the credential is a secret now.
    let a = store.get_attachment(demo, egret.id).await.unwrap().unwrap();
    assert_eq!(a.catalog_client_id, Some(client));
    assert!(a.minted_client);
    assert_eq!(vault.resolve_plaintext(None, a.credential_secret_id.unwrap()).await.unwrap(), "cid:csecret");
    // trino-b's primary row was never there (it has no legacy primary), so it has no egret attachment.
    assert!(store.get_attachment(other, egret.id).await.unwrap().is_none());

    // Identical `bench` rows share one typed catalog; different `pg_main` rows do not.
    let bench = store.get_catalog_by_name("bench").await.unwrap().unwrap();
    assert!(matches!(bench.def().unwrap().config, CatalogConfig::Tpch(_)));
    assert!(store.get_attachment(demo, bench.id).await.unwrap().is_some());
    assert!(store.get_attachment(other, bench.id).await.unwrap().is_some());
    let pg = store.get_catalog_by_name("pg_main").await.unwrap().unwrap();
    assert_eq!(pg.kind, "generic");
    assert!(!pg.advanced.to_string().contains("hunter2"), "plaintext left the catalog");
    let typed = store.get_catalog_by_name("bad_name").await.unwrap().unwrap();
    assert_eq!(typed.kind, "jdbc");

    let got = codes(&db, first.run_id).await;
    for code in ["warehouse_catalog_created", "primary_named_egret", "engine_catalog_typed", "engine_catalog_generic",
                 "engine_catalog_shared", "engine_catalog_suffixed", "engine_catalog_renamed", "secret_extracted"] {
        assert!(got.iter().any(|c| c == code), "missing {code} in {got:?}");
    }
    assert!(!got.iter().any(|c| c == "saved_sql_references_catalog"), "nothing references bad-name");

    // Every legacy row is mapped; nothing blocks either engine.
    assert_eq!(store.unmigrated_legacy_catalog_count(demo).await.unwrap(), 0);
    assert_eq!(store.unmigrated_legacy_catalog_count(other).await.unwrap(), 0);

    // Idempotent, and an admin's removal is not undone.
    store.set_catalog_workspaces(egret.id, &[]).await.unwrap();
    store.delete_attachment(other, bench.id).await.unwrap();
    let second = run_backfill(&store, &vault).await.expect("second backfill");
    assert_eq!((second.report_rows, second.failed_rows), (0, 0));
    assert!(store.list_catalog_workspaces(egret.id).await.unwrap().is_empty());
    assert!(store.get_attachment(other, bench.id).await.unwrap().is_none());
    assert_eq!(catalog_names(&db).await.len(), 9);
}
```

- [ ] **Step 5: Write the several-primaries test** — `crates/egret-catalog-registry/tests/backfill_several_primaries.rs`:

```rust
//! Backfill: two engines with primaries on different warehouses keep normalised
//! names, and every saved object that names `egret.` is reported. A row that
//! cannot be migrated is reported and blocks only its engine.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn several_primaries_keep_normalised_names_and_report_saved_sql() {
    let Some(dsn) = egret_test_db::url().await else { return };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = &db.pool;
    let ws: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug, name, status) VALUES ('bf-many', 'BF', 'active') RETURNING id").fetch_one(pool).await.unwrap();
    let sc: Uuid = sqlx::query_scalar("INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) VALUES ('bf-sc2', 's3', 'us-east-1', 'b', 'k', true) RETURNING id").fetch_one(pool).await.unwrap();
    let mut engines = Vec::new();
    for (wh_name, eng) in [("lake-a", "trino-a"), ("lake-b", "trino-b"), ("lake-c", "trino-c")] {
        let wh: Uuid = sqlx::query_scalar("INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id")
            .bind(wh_name).bind(ws).bind(sc).fetch_one(pool).await.unwrap();
        let e: Uuid = sqlx::query_scalar("INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id")
            .bind(eng).bind(wh).fetch_one(pool).await.unwrap();
        engines.push((e, wh_name));
    }
    for (e, wh_name) in &engines[..2] {
        sqlx::query("INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) VALUES ($1, 'egret', 'egret-warehouse', $2, true)")
            .bind(e).bind(json!({ "warehouse": wh_name, "credential": "id:secret", "clientRowId": Uuid::new_v4() }))
            .execute(pool).await.unwrap();
    }
    // trino-c's primary has no credential: it cannot be migrated.
    sqlx::query("INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) VALUES ($1, 'egret', 'egret-warehouse', '{\"warehouse\":\"lake-c\"}'::jsonb, true)")
        .bind(engines[2].0).execute(pool).await.unwrap();

    // Saved SQL: four objects name egret., one does not.
    let db_id: Uuid = sqlx::query_scalar("INSERT INTO query.databases (workspace_id, name, engine, host, port) VALUES ($1, 'bf-conn', 'trino', 'h', 8080) RETURNING id")
        .bind(ws).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO analytics.saved_queries (workspace_id, name, sql) VALUES ($1, 'orders', 'SELECT * FROM egret.sales.orders'), ($1, 'quoted', 'select 1 from \"EGRET\" . s.t'), ($1, 'unrelated', 'SELECT * FROM my_egret.s.t')")
        .bind(ws).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO analytics.datasets (workspace_id, database_id, name, catalog, schema_name, table_name) VALUES ($1, $2, 'orders-ds', 'egret', 'sales', 'orders')")
        .bind(ws).bind(db_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO analytics.sql_snippets (workspace_id, name, body) VALUES ($1, 'snip', 'JOIN egret.dim.customers c')")
        .bind(ws).execute(pool).await.unwrap();

    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(db.clone(), egret_secrets::SecretStore::new(db.clone(), Some(crypto)));
    let store = RegistryStore::new(db.clone());
    let run = run_backfill(&store, &vault).await.expect("backfill");

    assert!(store.get_catalog_by_name("egret").await.unwrap().is_none(), "no exception with several primaries");
    for name in ["lake_a", "lake_b", "lake_c"] {
        assert!(store.get_catalog_by_name(name).await.unwrap().is_some(), "{name}");
    }
    assert_eq!(run.failed_rows, 1);
    assert_eq!(store.unmigrated_legacy_catalog_count(engines[2].0).await.unwrap(), 1, "trino-c blocked");
    assert_eq!(store.unmigrated_legacy_catalog_count(engines[0].0).await.unwrap(), 0);

    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT code, detail FROM registry.migration_report WHERE run_id = $1 ORDER BY id")
        .bind(run.run_id).fetch_all(pool).await.unwrap();
    let renamed = rows.iter().filter(|(c, _)| c == "engine_catalog_renamed").count();
    assert_eq!(renamed, 2, "{rows:?}");
    assert_eq!(rows.iter().filter(|(c, _)| c == "backfill_failed").count(), 1);
    let mut hits: Vec<String> = rows.iter()
        .filter(|(c, _)| c == "saved_sql_references_catalog")
        .map(|(_, d)| format!("{}", d["objectType"].as_str().unwrap()))
        .collect();
    hits.sort();
    assert_eq!(hits, vec!["dataset", "saved_query", "saved_query", "sql_snippet"], "{rows:?}");
}
```

- [ ] **Step 6: Run the backfill tests**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry --test backfill_single_primary --test backfill_several_primaries
```
Expected: both pass; each binary takes seconds. If `quoted` does not match, the pattern's `\s*` before the dot is missing — fix the pattern, not the test.

- [ ] **Step 7: Run it at boot** — `crates/egret-bin/Cargo.toml` `[dependencies]` gains `egret-catalog-registry.workspace = true` (after `egret-catalog-engine.workspace = true`). In `crates/egret-bin/src/main.rs`, directly after

```rust
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
```

insert:

```rust

    // Catalog registry backfill (SP3 §4.8): idempotent, advisory-locked, on every
    // boot. Not fatal: an engine whose catalogs could not be migrated refuses
    // applies until a later boot migrates them, and the report says why.
    match egret_catalog_registry::run_backfill(
        &egret_catalog_registry::RegistryStore::new(db.clone()),
        &workspace_secrets,
    )
    .await
    {
        Ok(s) => tracing::info!(run_id = %s.run_id, report_rows = s.report_rows,
            failed_rows = s.failed_rows, "catalog registry backfill finished"),
        Err(e) => tracing::error!(error = %e,
            "catalog registry backfill failed; engines with unmigrated catalogs refuse applies"),
    }
```

- [ ] **Step 8: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry -p egret-bin --all-targets -- -D warnings
git branch --show-current
git add Cargo.lock crates/egret-catalog-registry crates/egret-bin/Cargo.toml crates/egret-bin/src/main.rs
git commit -m "feat(catalog-registry): migrate warehouses and engine catalogs at boot

Every warehouse becomes an iceberg catalog assigned to its workspace
(the single engine primary keeps the name egret), every engine catalog
row becomes an attachment — shared when identical, suffixed when not,
typed when the typed form renders the same properties — and credentials
and inline passwords move into named secrets. Renamed catalogs are
looked up in saved SQL, and everything lands in the migration report.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 7: Engines run on the registry — rendering, create, delete, legacy endpoints

This task is large on purpose: the engine-catalog seam changes its source of truth, and no smaller step keeps `egret-engine-mgmt`'s suite green (rendering from attachments while the legacy endpoints still write `engine_catalogs`, or the reverse, breaks the catalog route tests either way). Review it as one change: "every engine catalog is a registry attachment, and today's routes behave as before".

**Files:**
- Create: `crates/egret-engine-mgmt/src/catalog_apply.rs`
- Modify: `crates/egret-engine-mgmt/src/lib.rs`, `src/engines.rs`, `tests/routes.rs`

**Interfaces:**
- Consumes: Task 5 `RegistryStore` (via `st.engine_mgmt.registry()`), `AttachedCatalog`, `AttachmentRow::pending`, `NewAttachment`, `WarehouseIdentity`; Task 2 `render_trino`, `TrinoRenderInput`, `WarehouseRender`, `StorageRender`, `TrinoCatalogFile`; Task 3 `LegacyCatalog`, `plaintext_secrets`, `classify`.
- Produces (crate-internal, used by Tasks 8–9):
  - `engines.rs` (now `pub(crate)`): `BuildOpts { exclude_catalog, pending_attachment, rotate_internal_secrets }`, `PendingAttachment { catalog_id, catalog_client_id, credential_secret_id }`, `build_provision_req_with`, `apply_built`, `ApplyOp`, `ApplyBuiltError`, `apply_conflict`, `ensure_engine_role`, `EngineCatalogDto`, `legacy_kind(&CatalogDef) -> String`.
  - `catalog_apply.rs`: `const STARROCKS_NOT_YET: &str`; `enum ApplyError { Conflict, Rejected(String), NotSupported(&'static str), Build(CpError) }` with `message()`; `enum CatalogChange { Attach(PendingAttachment), Detach(Uuid), Reapply }`; `enum CatalogOpError { Http(CpError), Apply(ApplyError) }` with `into_legacy()` and `into_registry()`; `struct ApplyOutcome { engine_id, engine_name, applied: bool, error: Option<String> }`; functions `apply_catalog_change`, `trino_catalog_req`, `legacy_config_view`, `store_client_credential`, `extract_plaintext_secrets`, `validate_references`, `attach_to_engine`, `detach_from_engine`, `propagate`, `retry_apply`.

- [ ] **Step 1: Write the failing route-test changes** in `crates/egret-engine-mgmt/tests/routes.rs` (they pin the new behaviour before the code exists):

1. Add a registry helper after `secret_store`:

```rust
fn registry(f: &Fixture) -> egret_catalog_registry::RegistryStore {
    egret_catalog_registry::RegistryStore::new(f.db.clone())
}

/// The registry catalog of a warehouse (created with the engine that uses it).
async fn warehouse_catalog(f: &Fixture, wh_id: Uuid) -> egret_catalog_registry::CatalogRow {
    registry(f)
        .get_catalog_for_warehouse(wh_id)
        .await
        .expect("read catalog")
        .expect("warehouse has a catalog")
}
```

(`egret-catalog-registry` is already a normal dependency of `egret-engine-mgmt` since Task 5, so the integration tests can name it without a dev-dependency.)

2. `create_engine_starrocks_persists_starrocks_type` (≈line 1001): replace
```rust
    assert_eq!(cats[0]["name"], "egret", "catalog name; body={j}");
```
with
```rust
    // D5: an engine reaches its warehouse under the warehouse catalog's name.
    let wh_cat = warehouse_catalog(&f, wh_id).await;
    assert_eq!(cats[0]["name"], serde_json::json!(wh_cat.name), "catalog name; body={j}");
    assert_eq!(cats[0]["id"], serde_json::json!(wh_cat.id), "legacy id is the registry id; body={j}");
```
and update the comment above the `client_count` query to "The client's credential lives in a named secret on the attachment and is revoked when the engine is deleted, same as Trino's; confirm it was minted."

3. `test_catalog_reports_ok_error_and_non_2xx` (≈line 699): the "different engine" check must use a catalog that engine does not have — both engines share the warehouse catalog now. Replace `"warehouseId": wh_id,` in the `other_name` create body with `"warehouseId": other_wh,` and add before it:
```rust
    let other_wh = seed_provisioned_warehouse(&f).await;
```

4. `add_catalog_with_client_id_inherits_role_and_mints_new_client`: a warehouse is one catalog per engine (D5), so attach the governed catalog from a **second** warehouse. After `let wh_id = seed_provisioned_warehouse(&f).await;` add `let governed_wh = seed_provisioned_warehouse(&f).await;`; create the role and `existing_client` on `governed_wh` (both `create_role(governed_wh, …)` and `NewClient { warehouse_id: governed_wh, workspace_id: <governed warehouse>.workspace_id, … }` — fetch it with `catalog_store.get_warehouse(governed_wh)`); post `"config": { "warehouseId": governed_wh, "clientId": existing_client.id }`; replace the `engine_mgmt.list_engine_catalogs(…)` lookup with

```rust
    let stored = registry(&f)
        .get_attachment(Uuid::parse_str(&engine_id).unwrap(), Uuid::parse_str(&cat_id).unwrap())
        .await
        .expect("read attachment")
        .expect("attachment row");
    let minted_client_row_id = stored.catalog_client_id.expect("minted client id");
```

and use `governed_wh` in both `list_clients` calls. Delete the now-unused `engine_mgmt` binding.

5. `add_catalog_with_foreign_client_id_400`: post both bodies with a second warehouse (`let other_wh = seed_provisioned_warehouse(&f).await;` and `"warehouseId": other_wh`) so the check under test (a client that is not the target warehouse's) is reached instead of "already attached".

6. `catalog_detail_masks_inline_secrets_and_a_resave_keeps_them`: the inline password is now moved into a named secret at add time. Replace everything from `assert_eq!(status, StatusCode::OK, "catalog detail; body={detail}");` to the end of the test with:

```rust
    assert_eq!(status, StatusCode::OK, "catalog detail; body={detail}");
    let secret_id = detail["config"]["secretProperties"]["connection-password"]
        .as_str()
        .expect("password is a secret reference now")
        .to_string();
    assert!(detail["config"]["properties"].get("connection-password").is_none());
    assert_eq!(detail["config"]["properties"]["connection-user"], "u");
    assert!(!detail.to_string().contains("hunter2"), "no plaintext in GET; body={detail}");
    assert_eq!(
        secret_store(&f).resolve_plaintext(None, Uuid::parse_str(&secret_id).unwrap()).await.unwrap(),
        "hunter2"
    );

    // Re-save exactly what the form received.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/catalogs/{cat_id}"),
            &token,
            Some(serde_json::json!({ "config": detail["config"].clone() })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "re-save catalog; body={j}");

    let stored = registry(&f).get_catalog(Uuid::parse_str(&cat_id).unwrap()).await.unwrap().unwrap();
    assert!(!stored.advanced.to_string().contains("hunter2"), "plaintext never stored");
    let rendered = port
        .last_updated()
        .catalogs
        .into_iter()
        .find(|c| c.name == "pg")
        .expect("pg catalog rendered");
    let placeholder = &rendered.properties["connection-password"];
    let var = placeholder.strip_prefix("${ENV:").and_then(|v| v.strip_suffix('}')).expect("env placeholder");
    assert_eq!(rendered.env[var], "hunter2");
```

7. `remove_catalog_applies_before_deleting_and_revoking`: the extra warehouse catalog comes from a second warehouse. After `let wh_id = …` add `let extra_wh = seed_provisioned_warehouse(&f).await;`, post `"config": { "warehouseId": extra_wh }`, and replace the store lookups:

```rust
    let cat_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let cat_name = j["name"].as_str().unwrap().to_string();
    let engine_uuid = Uuid::parse_str(&engine_id).unwrap();
    let reg = registry(&f);
    let client_id = reg
        .get_attachment(engine_uuid, cat_id)
        .await
        .unwrap()
        .unwrap()
        .catalog_client_id
        .expect("minted client id");
```

then `.all(|c| c.name != cat_name)`, `reg.get_attachment(engine_uuid, cat_id).await.unwrap().is_some()` ("row kept") / `.is_none()` ("row deleted"), and `list_clients(extra_wh)` in both client checks. Delete the `store` binding.

8. `delete_engine_removes_cluster_row_secrets_and_minted_clients_only`: replace the block from `let primary = store` down to the closing `.unwrap();` of `create_engine_catalog(…)` with:

```rust
    let reg = registry(&f);
    let primary = reg
        .get_attachment(engine_id, warehouse_catalog(&f, wh_id).await.id)
        .await
        .unwrap()
        .expect("primary attachment");
    let minted = primary.catalog_client_id.expect("minted client");
    engine_secrets.push(primary.credential_secret_id.expect("credential secret"));

    // A second warehouse's catalog, attached with a client the engine did NOT mint.
    let foreign_wh = seed_provisioned_warehouse(&f).await;
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store.get_warehouse(foreign_wh).await.unwrap().unwrap();
    let role = catalog_store
        .create_role(foreign_wh, "readonly_analyst", "external", Some("read-only role"))
        .await
        .unwrap();
    let foreign = catalog_store
        .create_client(&egret_catalog_engine::NewClient {
            name: "not-minted-by-the-engine".into(),
            consumer_type: "external".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: foreign_wh,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    let foreign_cred = secrets
        .create(None, &format!("foreign-cred-{}", Uuid::new_v4().simple()), None, "x:y")
        .await
        .unwrap();
    engine_secrets.push(foreign_cred);
    let (foreign_cat, _) = reg
        .ensure_warehouse_catalog(
            &egret_catalog_registry::WarehouseIdentity { id: foreign_wh, name: &wh.name, workspace_id: wh.workspace_id },
            None,
        )
        .await
        .unwrap();
    reg.insert_attachment(&egret_catalog_registry::NewAttachment {
        engine_id,
        catalog_id: foreign_cat.id,
        catalog_client_id: Some(foreign.id),
        credential_secret_id: Some(foreign_cred),
        minted_client: false,
        applied_now: true,
        created_at: None,
    })
    .await
    .unwrap();
```

Remove the `let wh = catalog_store.get_warehouse(wh_id)` / `let role` / `let foreign` block that followed (now above) and the `catalog_store` re-declaration. The existing assertions stand: the minted client is revoked, the foreign client survives, and every id in `engine_secrets` — now including both credential secrets, which egret stored and the deleted cluster no longer holds — is gone.

- [ ] **Step 2: Run to see them fail**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test routes -- catalog engine_catalog create_engine_starrocks delete_engine_removes
```
Expected: compile succeeds, the edited tests FAIL (legacy ids are `engine_catalogs` ids, no attachments exist, the primary is named `egret`).

- [ ] **Step 3: Create `src/catalog_apply.rs`**

```rust
//! Registry catalogs ↔ engines (SP3 §4.4–4.5): render an attachment for Trino,
//! attach, detach, re-apply, and the per-engine-type dispatch. The StarRocks arm
//! is the SP3b seam: today it refuses everything but detaching a catalog that
//! was never created on the FE.

use std::collections::BTreeMap;

use egret_catalog_api::{CreatedClient, NewClient};
use egret_catalog_registry::{
    classify, plaintext_secrets, render_trino, AttachedCatalog, CatalogDef, CatalogRow,
    EngineType, LegacyCatalog, NewAttachment, StorageRender, TrinoCatalogFile, TrinoRenderInput,
    WarehouseRender,
};
use uuid::Uuid;

use crate::engines::{
    apply_built, apply_conflict, build_provision_req_with, ensure_engine_role, ApplyBuiltError,
    ApplyOp, BuildOpts, CatalogReq, PendingAttachment,
};
use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

pub(crate) const STARROCKS_NOT_YET: &str =
    "applying registry catalogs to StarRocks engines is not supported yet";

#[derive(Debug)]
pub(crate) enum ApplyError {
    /// The engine row is gone or `deleting` (a concurrent delete).
    Conflict,
    /// Kubernetes rejected (or never answered) the apply.
    Rejected(String),
    NotSupported(&'static str),
    /// The request could not be assembled (missing warehouse, secret, …).
    Build(CpError),
}

impl ApplyError {
    pub(crate) fn message(&self) -> String {
        match self {
            ApplyError::Conflict => "engine changed while the update was being applied; retry".into(),
            ApplyError::Rejected(m) => m.clone(),
            ApplyError::NotSupported(m) => (*m).to_string(),
            ApplyError::Build(e) => e.to_string(),
        }
    }
}

pub(crate) enum CatalogChange {
    Attach(PendingAttachment),
    Detach(Uuid),
    Reapply,
}

#[derive(Debug)]
pub(crate) enum CatalogOpError {
    Http(CpError),
    Apply(ApplyError),
}

impl From<CpError> for CatalogOpError {
    fn from(e: CpError) -> Self {
        CatalogOpError::Http(e)
    }
}

impl From<egret_catalog_registry::RegistryError> for CatalogOpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        CatalogOpError::Http(e.into())
    }
}

impl From<egret_catalog_api::CatalogApiError> for CatalogOpError {
    fn from(e: egret_catalog_api::CatalogApiError) -> Self {
        CatalogOpError::Http(e.into())
    }
}

impl CatalogOpError {
    /// Today's engine-catalog routes: a rejected apply is a 400.
    pub(crate) fn into_legacy(self) -> CpError {
        match self {
            CatalogOpError::Http(e) | CatalogOpError::Apply(ApplyError::Build(e)) => e,
            CatalogOpError::Apply(ApplyError::Conflict) => apply_conflict(),
            CatalogOpError::Apply(ApplyError::Rejected(m)) => CpError::BadRequest(format!("engine update failed: {m}")),
            CatalogOpError::Apply(ApplyError::NotSupported(m)) => CpError::BadRequest(m.to_string()),
        }
    }

    /// Registry routes: a rejected apply is a 502 (spec §4.4).
    pub(crate) fn into_registry(self) -> CpError {
        match self {
            CatalogOpError::Apply(ApplyError::Rejected(m)) => CpError::ClusterApplyFailed(m),
            other => other.into_legacy(),
        }
    }
}

/// One engine's result of applying a catalog change.
#[derive(Clone, Debug)]
pub(crate) struct ApplyOutcome {
    pub(crate) engine_id: Uuid,
    pub(crate) engine_name: String,
    pub(crate) applied: bool,
    pub(crate) error: Option<String>,
}

/// Apply a catalog change to one engine, by engine type.
pub(crate) async fn apply_catalog_change(st: &CpState, engine: &Engine, change: CatalogChange) -> Result<(), ApplyError> {
    match EngineType::parse(&engine.r#type) {
        Some(EngineType::Trino) => apply_trino(st, engine, change).await,
        Some(EngineType::StarRocks) => apply_starrocks(st, engine, change).await,
        None => Err(ApplyError::NotSupported("unknown engine type")),
    }
}

async fn apply_trino(st: &CpState, engine: &Engine, change: CatalogChange) -> Result<(), ApplyError> {
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(ApplyError::Build(CpError::BadRequest("kubernetes not configured".into())));
    };
    let opts = match change {
        CatalogChange::Attach(p) => BuildOpts { pending_attachment: Some(p), ..BuildOpts::default() },
        CatalogChange::Detach(id) => BuildOpts { exclude_catalog: Some(id), ..BuildOpts::default() },
        CatalogChange::Reapply => BuildOpts::default(),
    };
    let built = build_provision_req_with(st, engine, opts).await.map_err(ApplyError::Build)?;
    apply_built(st, prov.as_ref(), engine, &built, ApplyOp::Update)
        .await
        .map_err(|e| match e {
            ApplyBuiltError::Conflict => ApplyError::Conflict,
            ApplyBuiltError::Failed(m) => ApplyError::Rejected(m),
        })
}

/// SP3b replaces this with CREATE / ALTER / DROP CATALOG on the FE. In SP3a the
/// FE bootstrap creates only the engine's primary catalog, which cannot be
/// detached, so any other attachment was never on the FE and detaching it has
/// nothing to drop.
async fn apply_starrocks(_st: &CpState, _engine: &Engine, change: CatalogChange) -> Result<(), ApplyError> {
    match change {
        CatalogChange::Detach(_) => Ok(()),
        CatalogChange::Attach(_) | CatalogChange::Reapply => Err(ApplyError::NotSupported(STARROCKS_NOT_YET)),
    }
}

/// Render an attached catalog with its inputs resolved (secrets still ids).
pub(crate) async fn render_attached(st: &CpState, a: &AttachedCatalog, env_prefix: &str) -> Result<TrinoCatalogFile, CpError> {
    let def = a.catalog.def().map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", a.catalog.name)))?;
    let warehouse = match def.warehouse_id() {
        Some(id) => {
            let wh = st.catalog.get_warehouse(id).await?.ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
            let sc = st.storage_registry.get_storage_conn(wh.storage_conn_id).await?
                .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
            Some((wh.name, sc.endpoint.unwrap_or_default()))
        }
        None => None,
    };
    let storage = match def.storage_connection_id() {
        Some(id) => Some(st.storage_registry.get_storage_conn(id).await?
            .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?),
        None => None,
    };
    let input = TrinoRenderInput {
        env_prefix,
        egret_url: &st.engine_egret_url,
        warehouse: warehouse.as_ref().map(|(name, ep)| WarehouseRender { name, s3_endpoint: ep }),
        storage: storage.as_ref().map(|s| StorageRender {
            endpoint: s.endpoint.as_deref(),
            region: &s.region,
            path_style: s.path_style,
            access_key: &s.access_key,
            secret_key_secret_id: s.secret_ref,
        }),
        credential_secret_id: a.attachment.credential_secret_id,
    };
    render_trino(&a.catalog.name, &def, &input)
        .map_err(|e| CpError::BadRequest(format!("render catalog {}: {e}", a.catalog.name)))
}

/// The provisioner's catalog entry: rendered properties + resolved env values.
pub(crate) async fn trino_catalog_req(st: &CpState, a: &AttachedCatalog, env_prefix: &str) -> Result<CatalogReq, CpError> {
    let file = render_attached(st, a, env_prefix).await?;
    let mut env = BTreeMap::new();
    for (var, secret_id) in &file.env {
        let value = st
            .secrets
            .resolve_plaintext(None, *secret_id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
        env.insert(var.clone(), value);
    }
    Ok(CatalogReq { name: file.name, properties: file.properties, env })
}

/// What `GET /engines/{id}/catalogs/{cid}` has always returned: an egret
/// warehouse's summary, or `{properties, secretProperties}` for the edit form.
pub(crate) async fn legacy_config_view(st: &CpState, a: &AttachedCatalog) -> Result<serde_json::Value, CpError> {
    let def = a.catalog.def().map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", a.catalog.name)))?;
    if let Some(wh_id) = def.warehouse_id() {
        let wh = st.catalog.get_warehouse(wh_id).await?.ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        let endpoint = st.storage_registry.get_storage_conn(wh.storage_conn_id).await?.and_then(|s| s.endpoint).unwrap_or_default();
        return Ok(serde_json::json!({
            "warehouseId": wh.id,
            "warehouse": wh.name,
            "s3_endpoint": endpoint,
            "mintedClient": a.attachment.minted_client,
        }));
    }
    let file = render_attached(st, a, "L").await?;
    let (mut plain, secret) = file.split_secret_properties();
    plain.remove("connector.name");
    let secret: serde_json::Map<String, serde_json::Value> =
        secret.into_iter().map(|(k, id)| (k, serde_json::Value::String(id.to_string()))).collect();
    Ok(serde_json::json!({ "properties": plain, "secretProperties": secret }))
}

/// Store a freshly minted client's credential as a named global secret.
pub(crate) async fn store_client_credential(st: &CpState, client: &CreatedClient) -> Result<Uuid, CpError> {
    Ok(st
        .secrets
        .create(
            None,
            &format!("catalog-client-{}", client.id),
            Some("Catalog Client credential an engine presents to egret's catalog"),
            &format!("{}:{}", client.client_id, client.client_secret),
        )
        .await?)
}

/// Move plaintext secrets out of a legacy body into named global secrets.
pub(crate) async fn extract_plaintext_secrets(st: &CpState, catalog_name: &str, legacy: &LegacyCatalog<'_>) -> Result<BTreeMap<String, Uuid>, CpError> {
    let mut out = BTreeMap::new();
    for (property, value) in plaintext_secrets(legacy) {
        let short = Uuid::new_v4().simple().to_string();
        let id = st
            .secrets
            .create(
                None,
                &format!("catalog-{catalog_name}-{property}-{}", &short[..8]),
                Some("Inline property of an engine catalog, moved into a secret"),
                &value,
            )
            .await?;
        out.insert(property, id);
    }
    Ok(out)
}

/// Every id a definition references must exist (global secrets, storage,
/// warehouse) — the typed parser cannot see the database.
pub(crate) async fn validate_references(st: &CpState, def: &CatalogDef) -> Result<(), CpError> {
    for id in def.secret_ids() {
        match st.secrets.get_meta(None, id).await {
            Ok(_) => {}
            Err(egret_secrets::SecretsError::NotFound) => return Err(CpError::BadRequest(format!("unknown secret {id}"))),
            Err(e) => return Err(e.into()),
        }
    }
    if let Some(id) = def.storage_connection_id() {
        st.storage_registry.get_storage_conn(id).await?
            .ok_or_else(|| CpError::BadRequest(format!("unknown storage connection {id}")))?;
    }
    if let Some(id) = def.warehouse_id() {
        st.catalog.get_warehouse(id).await?.ok_or_else(|| CpError::BadRequest(format!("unknown warehouse {id}")))?;
    }
    Ok(())
}

fn is_primary(engine: &Engine, catalog: &CatalogRow) -> bool {
    catalog.warehouse_id == Some(engine.warehouse_id)
}

/// Attach a registry catalog to an engine: apply first, then write the
/// attachment (SP1 Part B ordering). An egret warehouse catalog mints a Catalog
/// Client for this engine — under `role_from_client`'s role when given (today's
/// `config.clientId`), else the shared engine role.
pub(crate) async fn attach_to_engine(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
    role_from_client: Option<Uuid>,
) -> Result<(), CatalogOpError> {
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Http(apply_conflict()));
    }
    let registry = st.engine_mgmt.registry();
    if registry.get_attachment(engine.id, catalog.id).await?.is_some() {
        return Err(CatalogOpError::Http(CpError::Conflict(format!(
            "catalog {} is already attached to engine {}",
            catalog.name, engine.name
        ))));
    }
    let def = catalog.def().map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", catalog.name)))?;
    let engine_type = EngineType::parse(&engine.r#type)
        .ok_or_else(|| CpError::BadRequest(format!("unknown engine type {}", engine.r#type)))?;
    if !def.can_read(engine_type) {
        return Err(CatalogOpError::Http(CpError::BadRequest(format!(
            "a {} engine cannot read catalog {}",
            engine_type.as_str(),
            catalog.name
        ))));
    }
    if engine_type == EngineType::StarRocks {
        return Err(CatalogOpError::Apply(ApplyError::NotSupported(STARROCKS_NOT_YET)));
    }

    let mut minted: Option<(Uuid, Uuid)> = None;
    if let Some(wh_id) = def.warehouse_id() {
        let wh = st.catalog.get_warehouse(wh_id).await?.ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        if wh.lakekeeper_warehouse_id.is_none() {
            return Err(CatalogOpError::Http(CpError::BadRequest("warehouse is not provisioned yet".into())));
        }
        let role_id = match role_from_client {
            Some(cid) => {
                st.catalog
                    .list_clients(wh.id)
                    .await?
                    .into_iter()
                    .find(|c| c.id == cid)
                    .ok_or_else(|| CpError::BadRequest("config.clientId does not reference a client of this warehouse".into()))?
                    .catalog_role_id
            }
            None => ensure_engine_role(st, wh.id).await?,
        };
        let client = st
            .catalog
            .create_client(NewClient {
                name: format!("engine-{}-{}", engine.name, catalog.name),
                consumer_type: "engine".into(),
                workspace_id: wh.workspace_id,
                warehouse_id: wh.id,
                catalog_role_id: role_id,
            })
            .await?;
        match store_client_credential(st, &client).await {
            Ok(secret_id) => minted = Some((client.id, secret_id)),
            Err(e) => {
                let _ = st.catalog.revoke_client(client.id).await;
                return Err(e.into());
            }
        }
    }
    let pending = PendingAttachment {
        catalog_id: catalog.id,
        catalog_client_id: minted.map(|m| m.0),
        credential_secret_id: minted.map(|m| m.1),
    };

    if let Err(e) = apply_catalog_change(st, engine, CatalogChange::Attach(pending)).await {
        // Revoking makes the credential worthless even if the failed apply got as
        // far as the engine's env Secret, so its egret copy can go too.
        if let Some((client_id, secret_id)) = minted {
            if let Err(err) = st.catalog.revoke_client(client_id).await {
                tracing::warn!(engine = %engine.name, %client_id, error = ?err, "attach rollback: revoke failed");
            }
            if let Err(err) = st.secrets.delete(None, secret_id).await {
                tracing::warn!(engine = %engine.name, %secret_id, error = %err, "attach rollback: secret delete failed");
            }
        }
        return Err(CatalogOpError::Apply(e));
    }

    registry
        .insert_attachment(&NewAttachment {
            engine_id: engine.id,
            catalog_id: catalog.id,
            catalog_client_id: pending.catalog_client_id,
            credential_secret_id: pending.credential_secret_id,
            minted_client: minted.is_some(),
            applied_now: true,
            created_at: None,
        })
        .await
        .map_err(|e| {
            tracing::error!(engine = %engine.name, catalog = %catalog.name, error = %e,
                "catalog applied to the engine but the attachment could not be written; \
                 the next apply of this engine removes it again");
            CatalogOpError::from(e)
        })?;
    Ok(())
}

/// Detach: apply the engine without the catalog, then delete the attachment and
/// retire what egret minted for it. The engine's primary catalog cannot go.
pub(crate) async fn detach_from_engine(st: &CpState, engine: &Engine, catalog: &CatalogRow) -> Result<(), CatalogOpError> {
    let registry = st.engine_mgmt.registry();
    let attachment = registry
        .get_attachment(engine.id, catalog.id)
        .await?
        .ok_or(CatalogOpError::Http(CpError::NotFound))?;
    if is_primary(engine, catalog) {
        return Err(CatalogOpError::Http(CpError::BadRequest("cannot delete the primary catalog".into())));
    }
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Http(apply_conflict()));
    }
    apply_catalog_change(st, engine, CatalogChange::Detach(catalog.id))
        .await
        .map_err(CatalogOpError::Apply)?;
    registry.delete_attachment(engine.id, catalog.id).await?;
    // The cluster no longer uses these; failures are logged, the detach happened.
    if attachment.minted_client {
        if let Some(client_id) = attachment.catalog_client_id {
            if let Err(e) = st.catalog.revoke_client(client_id).await {
                tracing::warn!(engine = %engine.name, %client_id, error = ?e,
                    "catalog detached but its minted client could not be revoked");
            }
        }
    }
    if let Some(secret_id) = attachment.credential_secret_id {
        if let Err(e) = st.secrets.delete(None, secret_id).await {
            tracing::warn!(engine = %engine.name, %secret_id, error = %e,
                "catalog detached but its credential secret could not be deleted");
        }
    }
    Ok(())
}

/// Re-apply one engine after a catalog changed; flags the attachment on failure.
pub(crate) async fn retry_apply(st: &CpState, engine: &Engine, catalog_id: Uuid) -> Result<(), CatalogOpError> {
    let result = if engine.status == crate::engine_status::DELETING {
        Err(ApplyError::Conflict)
    } else {
        apply_catalog_change(st, engine, CatalogChange::Reapply).await
    };
    if let Err(e) = result {
        st.engine_mgmt.registry().set_apply_failed(engine.id, catalog_id, &e.message()).await?;
        return Err(CatalogOpError::Apply(e));
    }
    Ok(())
}

/// Spec §4.5.3: apply a catalog's stored configuration to every engine that
/// attaches it, in engine-name order. A failure flags that attachment and moves
/// on — the other engines already hold the new configuration, so nothing reverts.
pub(crate) async fn propagate(st: &CpState, catalog: &CatalogRow) -> Result<Vec<ApplyOutcome>, CpError> {
    let mut out = Vec::new();
    for row in st.engine_mgmt.registry().list_catalog_attachments(catalog.id).await? {
        let Some(engine) = st.engine_mgmt.get_engine(row.engine_id).await? else {
            continue;
        };
        let outcome = match retry_apply(st, &engine, catalog.id).await {
            Ok(()) => ApplyOutcome { engine_id: engine.id, engine_name: engine.name.clone(), applied: true, error: None },
            Err(CatalogOpError::Apply(e)) => ApplyOutcome {
                engine_id: engine.id,
                engine_name: engine.name.clone(),
                applied: false,
                error: Some(e.message()),
            },
            Err(CatalogOpError::Http(e)) => return Err(e),
        };
        out.push(outcome);
    }
    Ok(out)
}
```

`src/lib.rs`: add `pub(crate) mod catalog_apply;` to the module list.

- [ ] **Step 4: Point `engines.rs` at the registry** — edits in `crates/egret-engine-mgmt/src/engines.rs`, in file order:

(a) Imports: replace `use crate::{Engine, EngineCatalog};` with `use crate::Engine;` and add
```rust
use egret_catalog_registry::{AttachedCatalog, CatalogDef, LegacyCatalog, WarehouseIdentity};
use crate::catalog_apply::CatalogOpError;
```

(b) `EngineCatalogDto` becomes `pub(crate)` (fields too) and loses its `impl From<EngineCatalog>`; add after it:

```rust
/// The legacy `kind` of a registry catalog: `egret-warehouse` for a warehouse,
/// otherwise the Trino connector name (what `engine_catalogs.kind` held).
pub(crate) fn legacy_kind(def: &CatalogDef) -> String {
    if def.warehouse_id().is_some() {
        "egret-warehouse".into()
    } else {
        def.trino_connector_name()
    }
}

fn legacy_catalog_dto(engine: &Engine, a: &AttachedCatalog) -> Result<EngineCatalogDto, CpError> {
    let def = a.catalog.def().map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", a.catalog.name)))?;
    Ok(EngineCatalogDto {
        id: a.catalog.id,
        name: a.catalog.name.clone(),
        kind: legacy_kind(&def),
        is_primary: a.catalog.warehouse_id == Some(engine.warehouse_id),
        secret_ref: def.secret_ids().into_iter().next(),
    })
}

/// The registry catalog `cid` as attached to `engine` (404 when not attached).
async fn find_attached(st: &CpState, engine: &Engine, cid: Uuid) -> Result<AttachedCatalog, CpError> {
    st.engine_mgmt
        .registry()
        .list_engine_attachments(engine.id, Some(engine.warehouse_id))
        .await?
        .into_iter()
        .find(|a| a.catalog.id == cid)
        .ok_or(CpError::NotFound)
}
```

(c) `is_sensitive_config_key`: replace its body with `egret_catalog_registry::is_sensitive_key(key)` (keep the fn and its doc; the redaction helpers and their tests keep using it).

(d) Delete `build_catalog_req`, the `CatalogPropsAndSecrets` alias and `catalog_props_and_secrets` (lines 437–608 on `main`: from `// ── translation: stored catalog rows → provisioning request` through the end of `catalog_props_and_secrets`). Their logic lives in `egret_catalog_registry::{legacy_props_and_secrets, render_trino}` now.

(e) Replace `BuildOpts` and `BuiltReq` (lines 610–625) with:

```rust
/// Knobs for one spec assembly.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuildOpts {
    /// Render WITHOUT this registry catalog: detach applies first and deletes the
    /// attachment only after Kubernetes dropped the catalog.
    pub(crate) exclude_catalog: Option<Uuid>,
    /// Render as if this attachment existed: attach applies first and writes the
    /// attachment only after Kubernetes accepted the catalog.
    pub(crate) pending_attachment: Option<PendingAttachment>,
    /// Mint new internal secrets even when the engine already has some.
    pub(crate) rotate_internal_secrets: bool,
}

/// An attachment rendered before it is written (see [`BuildOpts`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingAttachment {
    pub(crate) catalog_id: Uuid,
    pub(crate) catalog_client_id: Option<Uuid>,
    pub(crate) credential_secret_id: Option<Uuid>,
}

/// An assembled provisioning request plus the internal secrets it rendered
/// (Trino only). Hand it to [`apply_built`], which settles those secrets.
pub(crate) struct BuiltReq {
    req: EngineProvisionReq,
    internal: Option<EngineInternalSecrets>,
    /// Database time just before the catalogs were read (Trino only). A
    /// successful apply marks applied only attachments whose catalog has not
    /// changed since.
    catalogs_read_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

and make `build_provision_req_with` `pub(crate) async fn`.

(f) In `build_provision_req_with`, the StarRocks early return becomes `return Ok(BuiltReq { req, internal: None, catalogs_read_at: None });`, and replace

```rust
    let cats: Vec<EngineCatalog> = st
        .engine_mgmt
        .list_engine_catalogs(engine.id)
        .await?
        .into_iter()
        .filter(|c| Some(c.id) != opts.exclude_catalog)
        .collect();
    let mut catalogs = Vec::new();
    for (i, c) in cats.iter().enumerate() {
        catalogs.push(build_catalog_req(st, engine, c, &format!("C{i}")).await?);
    }
```

with

```rust
    // Catalogs come from the registry (SP3). An engine whose legacy
    // `engine_catalogs` rows the boot backfill has not migrated must not be
    // applied: rendering only the registry would drop catalogs it runs today.
    let registry = st.engine_mgmt.registry();
    let unmigrated = registry.unmigrated_legacy_catalog_count(engine.id).await?;
    if unmigrated > 0 {
        return Err(CpError::Conflict(format!(
            "engine {} has {unmigrated} catalog(s) the registry migration has not moved yet; \
             see the catalog migration report",
            engine.name
        )));
    }
    let catalogs_read_at = registry.db_now().await?;
    let mut attached = registry
        .list_engine_attachments(engine.id, Some(engine.warehouse_id))
        .await?;
    attached.retain(|a| Some(a.catalog.id) != opts.exclude_catalog);
    if let Some(p) = opts.pending_attachment {
        let catalog = registry.get_catalog(p.catalog_id).await?.ok_or(CpError::NotFound)?;
        attached.push(AttachedCatalog {
            catalog,
            attachment: egret_catalog_registry::AttachmentRow::pending(
                engine.id,
                p.catalog_id,
                p.catalog_client_id,
                p.credential_secret_id,
            ),
        });
    }
    let mut catalogs = Vec::with_capacity(attached.len());
    for (i, a) in attached.iter().enumerate() {
        catalogs.push(crate::catalog_apply::trino_catalog_req(st, a, &format!("C{i}")).await?);
    }
```

and the final `Ok(BuiltReq { … internal: Some(internal), })` gains `catalogs_read_at: Some(catalogs_read_at),`.

(g) `enum ApplyOp`, `enum ApplyBuiltError`, `fn apply_built`, `fn apply_conflict` become `pub(crate)`. In `apply_built`, after `let res = match op { … };` insert:

```rust
    // Every attachment this request rendered is now on the engine (Trino renders
    // them all on every apply), so their failed flags — Retry's subject — clear.
    if res.is_ok() && !engine.is_starrocks() {
        if let Some(read_at) = built.catalogs_read_at {
            if let Err(e) = st.engine_mgmt.registry().mark_engine_applied(engine.id, read_at).await {
                tracing::warn!(engine = %engine.name, error = %e,
                    "applied, but the catalog apply status could not be recorded");
            }
        }
    }
```

(h) `ensure_engine_role` becomes `pub(crate)`. Delete `client_row_id` and `is_minted_client` (the attachment carries both facts now).

(i) `CreateUndo` gains, after `client_id`:
```rust
    /// The named secret holding that client's credential.
    credential_secret_id: Option<Uuid>,
```
In `compensate_failed_create`, declare `let mut row_kept = false;` before `if let Some(engine_id) = undo.engine_id {`, set `row_kept = true;` as the first statement of the `Err(e) => {` arm of `match st.engine_mgmt.delete_engine(engine_id).await`, and after the engine-row block (before the profiles loop) add:

```rust
    // The credential of the auto-wired client. Kept only while the row is kept
    // (its attachment still names it); otherwise the cluster is confirmed gone or
    // was never touched, exactly like the owned secrets above.
    if !row_kept {
        if let Some(secret_id) = undo.credential_secret_id {
            if let Err(e) = st.secrets.delete(None, secret_id).await {
                tracing::error!(%secret_id, error = %e, "create compensation: credential secret delete failed");
            }
        }
    }
```

(j) `create_engine`, step 5–7. Replace from `// 5. Auto-wire: mint a fresh Catalog Client` through the end of step 7 (the `create_engine_catalog(… "egret", "egret-warehouse" …)` call and its error arm) with:

```rust
    // 5. Auto-wire: the warehouse's registry catalog (created with the warehouse;
    //    ensured here for one that predates the registry) and a fresh Catalog
    //    Client for this engine, whose credential lives in a named secret. Minted
    //    only AFTER delegation resolves (step 3) and the spec validates (step 4);
    //    from here on every failure runs `compensate_failed_create`.
    let (wh_catalog, _) = st
        .engine_mgmt
        .registry()
        .ensure_warehouse_catalog(
            &WarehouseIdentity { id: wh.id, name: &wh.name, workspace_id: wh.workspace_id },
            None,
        )
        .await?;
    let role_id = ensure_engine_role(&st, wh.id).await?;
    let client = st
        .catalog
        .create_client(NewClient {
            name: format!("engine-{}", b.name),
            consumer_type: "engine".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh.id,
            catalog_role_id: role_id,
        })
        .await?;
    let mut undo = CreateUndo {
        client_id: Some(client.id),
        ..CreateUndo::default()
    };
    match crate::catalog_apply::store_client_credential(&st, &client).await {
        Ok(id) => undo.credential_secret_id = Some(id),
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    }

    // 6. Record the intent row (status `provisioning`).
    let engine = match st
        .engine_mgmt
        .create_engine(
            &b.name,
            "trino",
            &namespace,
            wh.id,
            &spec,
            b.tls_cert_secret_ref,
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };
    undo.engine_id = Some(engine.id);

    // 7. Attach the warehouse's catalog: the engine's PRIMARY catalog. Marked
    //    applied by the provision below.
    if let Err(e) = st
        .engine_mgmt
        .registry()
        .insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: engine.id,
            catalog_id: wh_catalog.id,
            catalog_client_id: Some(client.id),
            credential_secret_id: undo.credential_secret_id,
            minted_client: true,
            applied_now: false,
            created_at: None,
        })
        .await
    {
        compensate_failed_create(&st, undo).await;
        return Err(e.into());
    }
```

Step 2's `s3_endpoint` is no longer used (the renderer resolves the endpoint on every apply), but its existence check is still the create's early 400. Replace step 2 (`// 2. Resolve the storage endpoint for the Trino S3 connector.` through `let s3_endpoint = sc.endpoint.unwrap_or_default();`) with:

```rust
    // 2. The warehouse's storage connection must exist (the renderer resolves
    //    its endpoint on every apply).
    st.storage_registry
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
```

(k) `create_starrocks_engine`: the same three changes as (j).
   - Directly before `let role_id = ensure_engine_role(&st, wh.id).await?;` insert:

```rust
    let (wh_catalog, _) = st
        .engine_mgmt
        .registry()
        .ensure_warehouse_catalog(
            &WarehouseIdentity { id: wh.id, name: &wh.name, workspace_id: wh.workspace_id },
            None,
        )
        .await?;
```

   - Directly after its `let mut undo = CreateUndo { client_id: Some(client.id), ..CreateUndo::default() };` insert:

```rust
    match crate::catalog_apply::store_client_credential(&st, &client).await {
        Ok(id) => undo.credential_secret_id = Some(id),
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    }
```

   - Replace its step 2 (the `// Step 2: record the PRIMARY catalog row` comment, the `cat_config` json and the `create_engine_catalog(… "egret", "egret-warehouse" …)` call with its error arm) with:

```rust
    // Step 2: attach the warehouse's catalog (the engine's PRIMARY catalog); its
    // credential secret is what the background FE wire presents.
    if let Err(e) = st
        .engine_mgmt
        .registry()
        .insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: engine.id,
            catalog_id: wh_catalog.id,
            catalog_client_id: Some(client.id),
            credential_secret_id: undo.credential_secret_id,
            minted_client: true,
            applied_now: false,
            created_at: None,
        })
        .await
    {
        compensate_failed_create(&st, undo).await;
        return Err(e.into());
    }
```

   - In its final `spawn_starrocks_catalog_wire(…)` call add `wh_catalog.name.clone(),` directly after `wh.name.clone(),`.

(l) `spawn_starrocks_catalog_wire`: add a parameter `catalog_name: String,` after `warehouse_name: String,`, and in its body replace `starrocks_external_catalog_sql("egret", &egret_url, &warehouse_name, &credential)` with `starrocks_external_catalog_sql(&catalog_name, &egret_url, &warehouse_name, &credential)`. In the unit test `bootstrap_stops_before_any_provisioner_call_when_engine_already_gone` add `"egret".into(),` after `wh.name.clone(),`.

(m) Replace `sr_primary_catalog_credential` with:

```rust
/// The StarRocks primary catalog's name and OAuth2 `(client_id, client_secret)`
/// (the attachment's credential secret holds `"<client_id>:<client_secret>"`).
/// `None` when there is no primary attachment or credential — the re-bootstrap
/// then skips the catalog-wire step (best-effort, like create).
async fn sr_primary_catalog_credential(st: &CpState, engine: &Engine) -> Option<(String, String, String)> {
    let attached = st
        .engine_mgmt
        .registry()
        .list_engine_attachments(engine.id, Some(engine.warehouse_id))
        .await
        .ok()?;
    let primary = attached
        .into_iter()
        .find(|a| a.catalog.warehouse_id == Some(engine.warehouse_id))?;
    let credential = st
        .secrets
        .resolve_plaintext(None, primary.attachment.credential_secret_id?)
        .await
        .ok()?;
    let (client_id, client_secret) = credential.split_once(':')?;
    Some((primary.catalog.name, client_id.to_string(), client_secret.to_string()))
}
```

and at its call site in `update_starrocks_engine`: `match sr_primary_catalog_credential(&st, &engine).await { Some((catalog_name, client_id, client_secret)) => { …spawn_starrocks_catalog_wire(…, wh.name.clone(), catalog_name, client_id, client_secret, root_password, Some(boot)); } … }`, and update the comment above it ("Needs the primary catalog's OAuth2 credential (its attachment's credential secret)").

(n) `delete_engine`: replace
```rust
    // Catalog client ids, read before the catalogs cascade away with the row.
    let cats = st.engine_mgmt.list_engine_catalogs(engine.id).await?;
```
with
```rust
    // Attachments (minted clients, credential secrets), read before they cascade
    // away with the row.
    let attachments = st
        .engine_mgmt
        .registry()
        .list_engine_attachments(engine.id, Some(engine.warehouse_id))
        .await?;
```
the cleanup call with `cleanup_deleted_engine(&st, &engine, &attachments).await;`, and in `cleanup_deleted_engine` change the signature to `(st: &CpState, engine: &Engine, attachments: &[AttachedCatalog])` and its first loop to:

```rust
    // Only clients we minted; `minted_client = false` belongs to someone else.
    // Credential secrets are egret's copies; the cluster that used them is gone.
    for a in attachments {
        if a.attachment.minted_client {
            if let Some(cid) = a.attachment.catalog_client_id {
                if let Err(e) = st.catalog.revoke_client(cid).await {
                    tracing::warn!(engine = %engine.name, client = %cid, error = %e, "revoke engine catalog client failed");
                }
            }
        }
        if let Some(sid) = a.attachment.credential_secret_id {
            match st.secrets.delete(None, sid).await {
                Ok(()) | Err(egret_secrets::SecretsError::NotFound) => {}
                Err(e) => tracing::warn!(engine = %engine.name, secret = %sid, error = %e, "delete catalog credential secret failed"),
            }
        }
    }
```

(o) The legacy handlers. `list_catalogs` body after the gate:

```rust
    let engine = st.engine_mgmt.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let attached = st
        .engine_mgmt
        .registry()
        .list_engine_attachments(engine.id, Some(engine.warehouse_id))
        .await?;
    Ok(Json(
        attached
            .iter()
            .map(|a| legacy_catalog_dto(&engine, a))
            .collect::<Result<_, _>>()?,
    ))
```

`add_catalog`: keep the gate, the engine lookup, the `DELETING` 409 and the blank-kind 400 exactly as they are; replace everything from `// For an egret-warehouse catalog, resolve the warehouse + endpoint and mint a` to the end of the function with:

```rust
    // Registry semantics (SP3): an egret warehouse attaches its warehouse's
    // catalog (under that catalog's name — D5); any other kind becomes a registry
    // catalog of its own, classified typed when that renders the same properties.
    let registry = st.engine_mgmt.registry();
    let catalog = if b.kind == "egret-warehouse" {
        let wh_id = b.config["warehouseId"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                CpError::BadRequest("egret-warehouse catalog requires config.warehouseId".into())
            })?;
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        if wh.lakekeeper_warehouse_id.is_none() {
            return Err(CpError::BadRequest("warehouse is not provisioned yet".into()));
        }
        // `config.clientId` (optional): mint this engine's client under the role
        // of an EXISTING client of this warehouse instead of the engine role.
        let client_id_param = match b.config.get("clientId") {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| CpError::BadRequest("config.clientId must be a UUID string".into()))?,
            ),
        };
        let (catalog, _) = registry
            .ensure_warehouse_catalog(
                &WarehouseIdentity { id: wh.id, name: &wh.name, workspace_id: wh.workspace_id },
                None,
            )
            .await?;
        crate::catalog_apply::attach_to_engine(&st, &engine, &catalog, client_id_param)
            .await
            .map_err(CatalogOpError::into_legacy)?;
        catalog
    } else {
        if !egret_catalog_registry::is_valid_catalog_name(&b.name) {
            return Err(CpError::BadRequest(format!(
                "catalog name {:?} must match ^[a-z][a-z0-9_]{{0,62}}$",
                b.name
            )));
        }
        if let Some(existing) = registry.get_catalog_by_name(&b.name).await? {
            let in_use = !registry.list_catalog_attachments(existing.id).await?.is_empty()
                || !registry.list_catalog_workspaces(existing.id).await?.is_empty();
            if in_use || existing.warehouse_id.is_some() {
                return Err(CpError::AlreadyExists(format!(
                    "a registry catalog named {} already exists and is in use",
                    b.name
                )));
            }
            // Unattached and unassigned — what an earlier remove through this
            // form leaves behind. Re-adding it works as it did before the
            // registry: the new body replaces it.
            registry.delete_catalog(existing.id).await?;
        }
        let legacy = LegacyCatalog { name: &b.name, kind: &b.kind, config: &b.config, secret_ref: b.secret_ref };
        let extracted = crate::catalog_apply::extract_plaintext_secrets(&st, &b.name, &legacy).await?;
        let classified = egret_catalog_registry::classify(&legacy, &extracted)
            .map_err(|e| CpError::BadRequest(e.to_string()))?;
        crate::catalog_apply::validate_references(&st, &classified.def).await?;
        let catalog = registry.create_catalog(&b.name, &classified.def, None).await?;
        if let Err(e) = crate::catalog_apply::attach_to_engine(&st, &engine, &catalog, None).await {
            // Nothing attached it; the extracted secrets stay (see plan table #19).
            if let Err(del) = registry.delete_catalog(catalog.id).await {
                tracing::warn!(catalog = %catalog.name, error = %del, "add rollback: catalog delete failed");
            }
            return Err(e.into_legacy());
        }
        catalog
    };
    let attached = find_attached(&st, &engine, catalog.id).await?;
    Ok((StatusCode::CREATED, Json(legacy_catalog_dto(&engine, &attached)?)))
```

`get_catalog` body after the gate:

```rust
    let engine = st.engine_mgmt.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let attached = find_attached(&st, &engine, cid).await?;
    let dto = legacy_catalog_dto(&engine, &attached)?;
    let config = crate::catalog_apply::legacy_config_view(&st, &attached).await?;
    Ok(Json(EngineCatalogDetailDto {
        id: dto.id,
        name: dto.name,
        kind: dto.kind,
        is_primary: dto.is_primary,
        secret_ref: dto.secret_ref,
        config: redact_catalog_config(config),
    }))
```

`test_catalog`: replace the `let cat = … get_engine_catalog(cid) …` lookup and its `engine_id` guard with `let cat = find_attached(&st, &engine, cid).await?.catalog;` (the SQL uses `cat.name` as before).

`remove_catalog` body after the gate:

```rust
    let engine = st.engine_mgmt.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let attached = find_attached(&st, &engine, cid).await?;
    crate::catalog_apply::detach_from_engine(&st, &engine, &attached.catalog)
        .await
        .map_err(CatalogOpError::into_legacy)?;
    Ok(StatusCode::OK)
```

In the `#[utoipa::path]` annotations of `get_catalog`, `test_catalog`, `remove_catalog` and `update_catalog`, change `("cid" = Uuid, Path, description = "Catalog id")` to `("cid" = Uuid, Path, description = "Registry catalog id")`; in `remove_catalog`'s, change the 200 description to `"Catalog removed from the cluster, then detached (the registry catalog is kept) and its minted client revoked"`; in `update_catalog`'s, change the 200 description to `"Catalog config replaced in the registry and applied to every engine that attaches it"` and the 400 description to `"Primary or managed egret-warehouse catalogs are not editable, the properties do not fit the catalog's kind, or this engine's update failed (the catalog is not reverted)"`.

`update_catalog` body after the gate:

```rust
    let engine = st.engine_mgmt.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let attached = find_attached(&st, &engine, cid).await?;
    let def = attached
        .catalog
        .def()
        .map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", attached.catalog.name)))?;
    if attached.catalog.warehouse_id == Some(engine.warehouse_id) {
        return Err(CpError::BadRequest("cannot edit the primary catalog".into()));
    }
    if def.warehouse_id().is_some() {
        return Err(CpError::BadRequest(
            "egret-warehouse catalogs are managed and not editable".into(),
        ));
    }
    if engine.status == crate::engine_status::DELETING {
        return Err(apply_conflict());
    }
    // The form echoes what GET returned; keep stored values the admin left masked.
    let stored = crate::catalog_apply::legacy_config_view(&st, &attached).await?;
    let mut config = b.config.clone();
    restore_redacted_properties(&mut config, &stored);
    let kind = legacy_kind(&def);
    let legacy = LegacyCatalog { name: &attached.catalog.name, kind: &kind, config: &config, secret_ref: b.secret_ref };
    let extracted = crate::catalog_apply::extract_plaintext_secrets(&st, &attached.catalog.name, &legacy).await?;
    // A generic catalog stays generic; a typed one must still be expressible as
    // its kind (the kind of a catalog never changes).
    let new_def = if def.kind() == egret_catalog_registry::CatalogKind::Generic {
        egret_catalog_registry::classify_generic(&legacy, &extracted)
            .map_err(|e| CpError::BadRequest(e.to_string()))?
    } else {
        let classified = egret_catalog_registry::classify(&legacy, &extracted)
            .map_err(|e| CpError::BadRequest(e.to_string()))?;
        if classified.def.kind() != def.kind() {
            return Err(CpError::BadRequest(format!(
                "these properties cannot be expressed as a {} catalog; \
                 edit catalog {} through PUT /api/control-plane/catalogs/{{id}} instead",
                def.kind().as_str(),
                attached.catalog.name
            )));
        }
        classified.def
    };
    crate::catalog_apply::validate_references(&st, &new_def).await?;
    // Registry semantics (spec §4.5): store, then apply to every engine that
    // attaches it. Nothing reverts; a failed engine is flagged. This route
    // answers for its own engine.
    let updated = st
        .engine_mgmt
        .registry()
        .update_catalog(cid, &new_def, attached.catalog.description.as_deref())
        .await?
        .ok_or(CpError::NotFound)?;
    let outcomes = crate::catalog_apply::propagate(&st, &updated).await?;
    if let Some(failed) = outcomes.iter().find(|o| o.engine_id == engine.id && !o.applied) {
        return Err(CpError::BadRequest(format!(
            "engine update failed: {}",
            failed.error.clone().unwrap_or_default()
        )));
    }
    let attached = find_attached(&st, &engine, cid).await?;
    Ok(Json(legacy_catalog_dto(&engine, &attached)?))
```

(A typed catalog re-saved unchanged through the legacy form re-classifies to the same kind, because its view is exactly what the typed form renders.)

(p) Delete the unit tests that called the deleted `build_catalog_req` — `generic_catalog_without_secret_ref_is_verbatim`, `generic_catalog_with_secret_ref_injects_env_var`, `v2_connector_single_secret_property`, `v2_connector_two_secret_properties`, `v2_connector_no_secrets_is_verbatim`, `legacy_generic_row_still_resolves`, `legacy_postgres_row_still_resolves` — and the helpers `generic_catalog` and `catalog_of`. Their cases are covered by `egret-catalog-registry`'s `legacy::` and `render::` tests (Tasks 2–3). Keep `dummy_engine` (other tests use it).

- [ ] **Step 5: Build**

Run: `cargo check -p egret-engine-mgmt --all-targets`
Expected: clean. Fix any leftover reference to `EngineCatalog`, `client_row_id`, `is_minted_client` or `build_catalog_req` inside `engines.rs` (the store and `lib.rs` keep `EngineCatalog` until SP3c drops the table).

- [ ] **Step 6: Run the crate suite**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt
```
Expected: all pass, including the eight edited route tests and `rotate_and_add_catalog_reject_a_deleting_engine` (its `DELETING` check still runs first). Read the `finished in` of `tests/routes.rs`: it must be seconds.

- [ ] **Step 7: Regenerate the API description and client** (annotation texts changed)

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && cd ..
git status --short openapi.json frontend/src/api/generated
```
Expected: the drift test passes; `git status` lists `openapi.json` and whatever `gen:api` rewrote (descriptions only). Commit all of it — CI reruns `gen:api` and fails on any difference.

- [ ] **Step 8: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
git branch --show-current
git add crates/egret-engine-mgmt openapi.json
git add -A frontend/src/api/generated
git commit -m "feat(engine-mgmt): render engine catalogs from the registry

Engines now render every catalog from registry attachments: create
attaches the warehouse's catalog with its client credential in a named
secret, delete revokes minted clients and deletes those secrets, and a
successful apply clears apply failures. An engine with unmigrated
legacy catalogs refuses applies. The existing engine-catalog endpoints
keep their shape over the registry: the catalog id is the registry id,
DELETE is detach, and inline passwords move into secrets.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 8: `catalog_registry:*` permissions and the registry catalog API

**Files:**
- Create: `migrations/0148_catalog_permissions.sql`, `crates/egret-engine-mgmt/src/catalogs.rs`
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-controlplane/tests/catalog_drift.rs`, `crates/egret-engine-mgmt/src/{lib.rs,engines.rs,http_error.rs}`, `crates/egret-engine-mgmt/tests/routes.rs`, `crates/egret-query/tests/registry_workspace_routes_api.rs` (regression guard), `openapi.json`, `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Task 7 `catalog_apply::{propagate, validate_references, ApplyOutcome}`; Task 5 `RegistryStore`, `CatalogRow`, `CatalogEngineRow`, `CatalogFilter`.
- Produces:
  - Routes (all under `/api/control-plane`, tag `control-plane-catalog-registry`): `GET/POST /catalogs` (`?workspaceId=&secretId=`), `GET/PUT/DELETE /catalogs/{id}`, `GET /catalogs/{id}/impact`, `POST /catalogs/{id}/reapply`.
  - DTOs (pub(crate) unless noted): `RegistryCatalogDto`, `CatalogAttachmentDto`, `CreateRegistryCatalogBody`, `UpdateRegistryCatalogBody`, `ApplyOutcomeDto { engine_id, engine_name, status, error }`, `CatalogUpdateResultDto { catalog, outcomes }`, `CatalogImpactDto`, `CatalogImpactEngineDto`; `pub` `CatalogConflictBody`, `CatalogConflictEngine`, `CatalogConflictWorkspace`.
  - `EngineHttpError::CatalogInUse { engines, workspaces }` → 409 `{ error, code: "catalog_in_use", engines, workspaces }`.
  - `catalogs::routes() -> Router<CpState>`, merged by `engines::router()`; `catalogs::CatalogRegistryApi` merged by `lib.rs::openapi()`.
  - Test fixture: `FakeEnginePort::set_fail_update_for(&self, engine_name: &str, fail: bool)`.

- [ ] **Step 1: Per-engine failure in the fake** — `tests/routes.rs`, `FakeEnginePort`: add the field

```rust
    /// Engine names whose `update` fails (the others succeed) — for propagation.
    fail_update_for: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
```

the method

```rust
    fn set_fail_update_for(&self, engine_name: &str, fail: bool) {
        let mut names = self.fail_update_for.lock().unwrap();
        if fail {
            names.insert(engine_name.to_string());
        } else {
            names.remove(engine_name);
        }
    }
```

and in `update` replace the `if self.fail_update.load(…)` condition with `if self.fail_update.load(std::sync::atomic::Ordering::SeqCst) || self.fail_update_for.lock().unwrap().contains(&req.name)`.

- [ ] **Step 2: Write the failing route tests** — append to `tests/routes.rs`:

```rust
// ── Catalog registry (SP3a) ─────────────────────────────────────────────────────

fn unique_catalog_name(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn call(f: &Fixture, method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
    into_json(f.app.clone().oneshot(req(method, uri, token, body)).await.unwrap()).await
}

#[tokio::test]
async fn registry_catalog_crud_validation_and_delete_refusal() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_crud_admin", true).await;
    let pw = seed_secret(&f, "hunter2").await;
    let name = unique_catalog_name("pg");
    let jdbc = serde_json::json!({
        "dialect": "postgresql", "host": "db.internal", "port": 5432, "database": "sales",
        "user": "reader", "password_secret_id": pw, "ssl": { "mode": "require" }
    });

    let (status, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({
        "name": name, "kind": "jdbc", "config": jdbc, "description": "sales db"
    }))).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!((j["name"].as_str(), j["kind"].as_str()), (Some(name.as_str()), Some("jdbc")));
    assert_eq!(j["engines"], serde_json::json!([]));
    let id = j["id"].as_str().unwrap().to_string();

    let (status, j) = call(&f, "GET", &format!("/catalogs?secretId={pw}"), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(j.as_array().unwrap().iter().any(|c| c["id"] == serde_json::json!(id)), "{j}");

    let (status, _) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({ "name": name, "kind": "jdbc", "config": jdbc }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate name");
    for (why, body) in [
        ("bad name", serde_json::json!({ "name": "Bad-Name", "kind": "tpch", "config": {} })),
        ("unknown kind", serde_json::json!({ "name": unique_catalog_name("x"), "kind": "mongo", "config": {} })),
        ("unknown secret", serde_json::json!({ "name": unique_catalog_name("x"), "kind": "jdbc", "config": {
            "dialect": "postgresql", "host": "h", "port": 5432, "database": "d", "user": "u",
            "password_secret_id": Uuid::new_v4(), "ssl": { "mode": "require" } } })),
        ("plaintext advanced", serde_json::json!({ "name": unique_catalog_name("x"), "kind": "tpch", "config": {},
            "advanced": { "trino": { "a.password": "hunter2" } } })),
        ("egret warehouse", serde_json::json!({ "name": unique_catalog_name("x"), "kind": "iceberg",
            "config": { "metastore": { "type": "egret", "warehouse_id": Uuid::new_v4() } } })),
    ] {
        let (status, j) = call(&f, "POST", "/catalogs", &token, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {j}");
    }

    // Refused while attached; the 409 names the engine.
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let catalog_id = Uuid::parse_str(&id).unwrap();
    let reg = registry(&f);
    reg.insert_attachment(&egret_catalog_registry::NewAttachment {
        engine_id, catalog_id, catalog_client_id: None, credential_secret_id: None,
        minted_client: false, applied_now: true, created_at: None,
    }).await.unwrap();
    let (status, j) = call(&f, "DELETE", &format!("/catalogs/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert_eq!(j["code"], "catalog_in_use");
    assert_eq!(j["engines"][0]["name"], serde_json::json!(engine_name));
    reg.delete_attachment(engine_id, catalog_id).await.unwrap();
    let (status, _) = call(&f, "DELETE", &format!("/catalogs/{id}"), &token, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A warehouse catalog goes with its warehouse, never through this route.
    let lone_wh = seed_provisioned_warehouse(&f).await;
    let wh = egret_catalog_engine::CatalogStore::new(f.db.clone()).get_warehouse(lone_wh).await.unwrap().unwrap();
    let (lone, _) = reg.ensure_warehouse_catalog(
        &egret_catalog_registry::WarehouseIdentity { id: lone_wh, name: &wh.name, workspace_id: wh.workspace_id }, None,
    ).await.unwrap();
    reg.set_catalog_workspaces(lone.id, &[]).await.unwrap();
    let (status, j) = call(&f, "DELETE", &format!("/catalogs/{}", lone.id), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert!(j["error"].as_str().unwrap().contains("warehouse"), "{j}");
}

#[tokio::test]
async fn registry_update_propagates_and_flags_the_failed_engine() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_prop_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name_a = format!("trino-a-{}", Uuid::new_v4().simple());
    let name_b = format!("trino-b-{}", Uuid::new_v4().simple());
    let mut engines = Vec::new();
    for n in [&name_a, &name_b] {
        let (status, j) = create_engine_via_api(&f, &token, n, wh_id, serde_json::json!({})).await;
        assert_eq!(status, StatusCode::CREATED, "{j}");
        engines.push(Uuid::parse_str(j["id"].as_str().unwrap()).unwrap());
    }
    let cat_name = unique_catalog_name("bench");
    let (status, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} }))).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let catalog_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let reg = registry(&f);
    for engine_id in &engines {
        reg.insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: *engine_id, catalog_id, catalog_client_id: None, credential_secret_id: None,
            minted_client: false, applied_now: true, created_at: None,
        }).await.unwrap();
    }

    let (status, j) = call(&f, "GET", &format!("/catalogs/{catalog_id}/impact"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["engines"].as_array().unwrap().len(), 2);
    assert!(j["engines"].as_array().unwrap().iter().all(|e| e["action"] == "restart"), "{j}");

    port.set_fail_update_for(&name_b, true);
    let (status, j) = call(&f, "PUT", &format!("/catalogs/{catalog_id}"), &token, Some(serde_json::json!({
        "config": {}, "advanced": { "trino": { "tpch.column-naming": "STANDARD" } }
    }))).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    let outcome = |n: &str| j["outcomes"].as_array().unwrap().iter().find(|o| o["engineName"] == n).cloned().unwrap();
    assert_eq!(outcome(&name_a)["status"], "applied");
    assert_eq!(outcome(&name_b)["status"], "failed");
    assert!(outcome(&name_b)["error"].as_str().unwrap().contains("injected update failure"));

    // Engine A holds the new configuration; nothing was reverted.
    let spec_a = port.updated.lock().unwrap().iter().rev().find(|s| s.name == name_a).cloned().unwrap();
    let rendered = spec_a.catalogs.iter().find(|c| c.name == cat_name).expect("catalog rendered on A");
    assert_eq!(rendered.properties["tpch.column-naming"], "STANDARD");
    let stored = reg.get_catalog(catalog_id).await.unwrap().unwrap();
    assert_eq!(stored.advanced["trino"]["tpch.column-naming"], "STANDARD");
    let a = reg.get_attachment(engines[0], catalog_id).await.unwrap().unwrap();
    let b = reg.get_attachment(engines[1], catalog_id).await.unwrap().unwrap();
    assert_eq!(a.apply_status, "applied");
    assert_eq!(b.apply_status, "failed");
    assert!(b.apply_error.unwrap().contains("injected update failure"));

    // Re-apply once the engine is healthy again.
    port.set_fail_update_for(&name_b, false);
    let (status, j) = call(&f, "POST", &format!("/catalogs/{catalog_id}/reapply"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert!(j.as_array().unwrap().iter().all(|o| o["status"] == "applied"), "{j}");
    assert_eq!(reg.get_attachment(engines[1], catalog_id).await.unwrap().unwrap().apply_status, "applied");
}

#[tokio::test]
async fn catalog_permissions_with_the_real_pdp() {
    let Some(mut f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let reader = session_for_with_permission(&f, "cat_reader", "catalog_registry:read").await;
    let engine_reader = session_for_with_permission(&f, "cat_engine_reader", "engine:read").await;
    attach_real_pdp(&mut f).await;
    let body = serde_json::json!({ "name": unique_catalog_name("x"), "kind": "tpch", "config": {} });
    assert_eq!(call(&f, "GET", "/catalogs", &reader, None).await.0, StatusCode::OK);
    assert_eq!(call(&f, "POST", "/catalogs", &reader, Some(body)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&f, "GET", "/catalogs", &engine_reader, None).await.0, StatusCode::FORBIDDEN);
    // 0148: every platform role holding warehouse:read now holds catalog_registry:read.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
          WHERE r.scope = 'platform' AND r.name = 'data_steward' AND rp.permission = 'catalog_registry:read'",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}
```

- [ ] **Step 2b: Regression guard — the platform key must not open workspace views.** Append to `crates/egret-query/tests/registry_workspace_routes_api.rs`:

```rust
/// SP3 permission split: the platform key `catalog_registry:read` must NOT
/// satisfy the workspace-scope `catalog:read` check (Cedar `Workspace in
/// Platform` would make a same-named platform grant pass it in EVERY
/// workspace). The HTTP 403 below is also produced by WsContext's membership
/// gate, so the direct PDP assertions are what actually discriminate: they fail
/// if the platform permission is ever renamed back to `catalog:read`.
#[tokio::test]
async fn platform_catalog_registry_read_does_not_grant_workspace_catalog_read() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let email = format!("regreader_{}@egret.local", Uuid::new_v4().simple());
    let uid = f.users.insert_local(&email, "Test", "x").await.expect("insert user");
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') RETURNING id",
    )
    .bind(format!("test_role_{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("insert role");
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'catalog_registry:read')")
        .bind(role_id)
        .execute(&f.db.pool)
        .await
        .expect("grant permission");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(uid.0)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("assign role");
    let token = f.sessions.create(uid, "test", "127.0.0.1").await.expect("session").token;

    // Not a member of ws_a: the workspace engine view is 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/ws/{}/engines", f.slug_a), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The PDP itself: the role was seeded after `setup()` built its PDP, so build
    // a fresh one that sees it.
    let pdp = egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
        .await
        .expect("build CedarPdp");
    use egret_core::authz::{Pdp, Resource, Subject};
    let subject = Subject { user_id: uid, group_ids: vec![] };
    let platform = pdp
        .authorize(&subject, "catalog_registry:read", &Resource::Platform)
        .await
        .expect("authorize");
    assert!(platform.allow, "positive control: the platform grant works");
    let workspace = pdp
        .authorize(&subject, "catalog:read", &Resource::Workspace(f.ws_a))
        .await
        .expect("authorize");
    assert!(
        !workspace.allow,
        "catalog_registry:read must not satisfy the workspace-scope catalog:read"
    );
}
```

(`f.ws_a` is the fixture's `WorkspaceId`; `insert_local` returns a `UserId`, which is what `Subject.user_id` holds.)

- [ ] **Step 3: Run to see them fail**

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test routes -- registry_ catalog_permissions`
Expected: FAIL — `/catalogs` is 404.

- [ ] **Step 4: Permissions** — `crates/egret-core/src/rbac.rs`, directly after the `warehouse:delete` entry:

```rust
    // Registry catalogs (SP3). Deliberately NOT `catalog:*`: `catalog:read` is
    // the workspace-scope key every workspace role holds (0024) for
    // egret-query's workspace engine view, and a platform grant of the same key
    // would satisfy that check in every workspace.
    perm!(
        "catalog_registry:read",
        "Catalogs",
        "View registry catalogs and their engine attachments",
        Scope::Platform
    ),
    perm!(
        "catalog_registry:create",
        "Catalogs",
        "Create registry catalogs",
        Scope::Platform
    ),
    perm!(
        "catalog_registry:update",
        "Catalogs",
        "Edit registry catalogs, re-apply them to engines, and assign them to workspaces",
        Scope::Platform
    ),
    perm!(
        "catalog_registry:delete",
        "Catalogs",
        "Delete registry catalogs",
        Scope::Platform
    ),
```

and add to the `tests` module:

```rust
    #[test]
    fn catalog_registry_perms_are_platform_scoped_and_distinct() {
        for k in ["catalog_registry:read", "catalog_registry:create", "catalog_registry:update", "catalog_registry:delete"] {
            assert!(PERMISSIONS.iter().any(|p| p.key == k && p.scope == Scope::Platform), "{k}");
        }
        assert!(groups().contains(&"Catalogs"));
        // The workspace-scope `catalog:read` (0024) is NOT a platform key and
        // must never become one: a platform grant of it would pass the
        // workspace check in every workspace.
        assert!(!is_known("catalog:read"));
    }
```

`migrations/0148_catalog_permissions.sql`:

```sql
-- 0148_catalog_permissions.sql
-- Registry catalogs (SP3 §4.7) get their own platform permissions,
-- catalog_registry:*. Every platform role receives the catalog_registry:*
-- counterpart of each warehouse:* it holds, so data_steward gains
-- catalog_registry:read. The workspace-scope catalog:read from 0024 (egret-query's
-- workspace engine view) is a different key and is not touched: reusing it here
-- would let a platform grant pass that check in every workspace.
-- warehouse:* stays until SP3c removes the Warehouses pages.
INSERT INTO role_permissions (role_id, permission)
SELECT rp.role_id, 'catalog_registry:' || split_part(rp.permission, ':', 2)
  FROM role_permissions rp
  JOIN roles r ON r.id = rp.role_id
 WHERE r.scope = 'platform'
   AND rp.permission IN ('warehouse:read', 'warehouse:create', 'warehouse:update', 'warehouse:delete')
ON CONFLICT (role_id, permission) DO NOTHING;
```

`crates/egret-controlplane/tests/catalog_drift.rs`, `EXTRA_FILES`: after `"../egret-engine-mgmt/src/monitor.rs",` add

```rust
    // `catalog_registry:*` (SP3) are gated in the engine seam's registry routes.
    "../egret-engine-mgmt/src/catalogs.rs",
```

- [ ] **Step 5: The 409 body** — `crates/egret-engine-mgmt/src/http_error.rs`: add the variant after `ClusterApplyFailed`:

```rust
    /// A registry catalog is still attached to engines or assigned to
    /// workspaces; nothing was changed. 409 listing both.
    #[error("catalog is still in use")]
    CatalogInUse {
        engines: Vec<crate::catalogs::CatalogConflictEngine>,
        workspaces: Vec<crate::catalogs::CatalogConflictWorkspace>,
    },
```

and in `into_response`, before the `EngineHttpError::Forbidden` arm:

```rust
            EngineHttpError::CatalogInUse { engines, workspaces } => {
                let body = crate::catalogs::CatalogConflictBody {
                    error: format!(
                        "{msg}: attached to {} engine(s) and assigned to {} workspace(s); \
                         detach and unassign it first",
                        engines.len(),
                        workspaces.len()
                    ),
                    code: "catalog_in_use".into(),
                    engines,
                    workspaces,
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
```

- [ ] **Step 6: `src/catalogs.rs`**

```rust
//! Catalog registry routes (SP3 §4.4–4.7): platform catalogs, the engines they
//! are attached to, their workspace assignment and the migration report. Merged
//! into the engines router, so mounted under `/api/control-plane`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_catalog_registry::{CatalogDef, CatalogEngineRow, CatalogFilter, CatalogRow};
use egret_core::http::ErrorBody;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::catalog_apply::{propagate, validate_references, ApplyOutcome};
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

pub(crate) fn routes() -> Router<CpState> {
    Router::new()
        .route("/catalogs", get(list_registry_catalogs).post(create_registry_catalog))
        .route(
            "/catalogs/{id}",
            get(get_registry_catalog)
                .put(update_registry_catalog)
                .delete(delete_registry_catalog),
        )
        .route("/catalogs/{id}/impact", get(catalog_impact))
        .route("/catalogs/{id}/reapply", post(reapply_registry_catalog))
}

/// One engine a catalog is attached to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogAttachmentDto {
    engine_id: Uuid,
    engine_name: String,
    engine_type: String,
    /// `applied` | `failed`.
    apply_status: String,
    apply_error: Option<String>,
    applied_at: Option<DateTime<Utc>>,
}

impl From<CatalogEngineRow> for CatalogAttachmentDto {
    fn from(r: CatalogEngineRow) -> Self {
        CatalogAttachmentDto {
            engine_id: r.engine_id,
            engine_name: r.engine_name,
            engine_type: r.engine_type,
            apply_status: r.apply_status,
            apply_error: r.apply_error,
            applied_at: r.applied_at,
        }
    }
}

/// A registry catalog. `config` is the typed per-kind configuration (snake_case
/// fields, spec §4.2); it never holds a plaintext secret.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegistryCatalogDto {
    id: Uuid,
    name: String,
    kind: String,
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[schema(value_type = Object)]
    advanced: serde_json::Value,
    description: Option<String>,
    /// The warehouse an egret catalog is.
    warehouse_id: Option<Uuid>,
    engines: Vec<CatalogAttachmentDto>,
    workspace_ids: Vec<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateRegistryCatalogBody {
    /// SQL name on every engine: `^[a-z][a-z0-9_]{0,62}$`, immutable.
    name: String,
    /// iceberg | hive | delta_lake | hudi | jdbc | elasticsearch | tpch | tpcds | generic
    kind: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Object)]
    advanced: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
}

/// Replaces config/advanced/description; name and kind never change.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateRegistryCatalogBody {
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Object)]
    advanced: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
}

/// One engine's result of applying a catalog.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyOutcomeDto {
    engine_id: Uuid,
    engine_name: String,
    /// `applied` | `failed`.
    status: String,
    error: Option<String>,
}

impl From<ApplyOutcome> for ApplyOutcomeDto {
    fn from(o: ApplyOutcome) -> Self {
        ApplyOutcomeDto {
            engine_id: o.engine_id,
            engine_name: o.engine_name,
            status: if o.applied { "applied" } else { "failed" }.into(),
            error: o.error,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogUpdateResultDto {
    catalog: RegistryCatalogDto,
    outcomes: Vec<ApplyOutcomeDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogImpactEngineDto {
    engine_id: Uuid,
    engine_name: String,
    engine_type: String,
    /// `restart` (Trino rolls its pods) | `in_place` (StarRocks rebuilds the connector).
    action: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogImpactDto {
    engines: Vec<CatalogImpactEngineDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogConflictEngine {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogConflictWorkspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

/// 409 body of a refused catalog delete (`code` = `catalog_in_use`).
#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogConflictBody {
    pub error: String,
    pub code: String,
    pub engines: Vec<CatalogConflictEngine>,
    pub workspaces: Vec<CatalogConflictWorkspace>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListCatalogsQuery {
    /// Only catalogs assigned to this workspace.
    workspace_id: Option<Uuid>,
    /// Only catalogs referencing this secret (config or engine credential).
    secret_id: Option<Uuid>,
}

async fn to_dto(st: &CpState, row: CatalogRow) -> Result<RegistryCatalogDto, CpError> {
    let registry = st.engine_mgmt.registry();
    let engines = registry
        .list_catalog_attachments(row.id)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    let workspace_ids = registry
        .list_catalog_workspaces(row.id)
        .await?
        .into_iter()
        .map(|w| w.workspace_id)
        .collect();
    Ok(RegistryCatalogDto {
        id: row.id,
        name: row.name,
        kind: row.kind,
        config: row.config,
        advanced: row.advanced,
        description: row.description,
        warehouse_id: row.warehouse_id,
        engines,
        workspace_ids,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn load(st: &CpState, id: Uuid) -> Result<CatalogRow, CpError> {
    st.engine_mgmt.registry().get_catalog(id).await?.ok_or(CpError::NotFound)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs",
    tag = "control-plane-catalog-registry",
    params(ListCatalogsQuery),
    responses(
        (status = 200, description = "Registry catalogs with their engines and workspaces", body = Vec<RegistryCatalogDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_registry_catalogs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<ListCatalogsQuery>,
) -> Result<Json<Vec<RegistryCatalogDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let rows = st
        .engine_mgmt
        .registry()
        .list_catalogs(&CatalogFilter { workspace_id: q.workspace_id, secret_id: q.secret_id })
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(to_dto(&st, row).await?);
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs",
    tag = "control-plane-catalog-registry",
    request_body = CreateRegistryCatalogBody,
    responses(
        (status = 201, description = "Catalog created (not attached to any engine)", body = RegistryCatalogDto),
        (status = 400, description = "Invalid name, kind or configuration, an unknown secret or storage connection, or an egret warehouse catalog (those are created with their warehouse)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:create permission", body = ErrorBody),
        (status = 409, description = "A catalog with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateRegistryCatalogBody>,
) -> Result<(StatusCode, Json<RegistryCatalogDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:create").await?;
    let def = CatalogDef::parse(&b.kind, &b.config, &b.advanced)
        .map_err(|e| CpError::BadRequest(e.to_string()))?;
    if def.warehouse_id().is_some() {
        return Err(CpError::BadRequest(
            "an egret warehouse catalog is created with its warehouse".into(),
        ));
    }
    validate_references(&st, &def).await?;
    let row = st
        .engine_mgmt
        .registry()
        .create_catalog(&b.name, &def, b.description.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(to_dto(&st, row).await?)))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/{id}",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "The catalog", body = RegistryCatalogDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<RegistryCatalogDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let row = load(&st, id).await?;
    Ok(Json(to_dto(&st, row).await?))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/catalogs/{id}",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    request_body = UpdateRegistryCatalogBody,
    responses(
        (status = 200, description = "Stored, then applied to every attached engine; `outcomes` lists each engine (a failed engine is flagged, nothing is reverted)", body = CatalogUpdateResultDto),
        (status = 400, description = "Invalid configuration, an unknown secret or storage connection, or a changed warehouse", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateRegistryCatalogBody>,
) -> Result<Json<CatalogUpdateResultDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let row = load(&st, id).await?;
    let def = CatalogDef::parse(&row.kind, &b.config, &b.advanced)
        .map_err(|e| CpError::BadRequest(e.to_string()))?;
    if def.warehouse_id() != row.warehouse_id {
        return Err(CpError::BadRequest(
            "the warehouse of an egret catalog cannot change".into(),
        ));
    }
    validate_references(&st, &def).await?;
    let updated = st
        .engine_mgmt
        .registry()
        .update_catalog(id, &def, b.description.as_deref())
        .await?
        .ok_or(CpError::NotFound)?;
    let outcomes = propagate(&st, &updated).await?;
    Ok(Json(CatalogUpdateResultDto {
        catalog: to_dto(&st, updated).await?,
        outcomes: outcomes.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/catalogs/{id}",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 204, description = "Catalog deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:delete permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 409, description = "Still attached or assigned (`code` = `catalog_in_use`, with `engines` and `workspaces`), or an egret warehouse catalog (delete the warehouse instead)", body = CatalogConflictBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:delete").await?;
    let row = load(&st, id).await?;
    let registry = st.engine_mgmt.registry();
    let engines = registry.list_catalog_attachments(id).await?;
    let workspaces = registry.list_catalog_workspaces(id).await?;
    if !engines.is_empty() || !workspaces.is_empty() {
        return Err(CpError::CatalogInUse {
            engines: engines
                .into_iter()
                .map(|e| CatalogConflictEngine { id: e.engine_id, name: e.engine_name })
                .collect(),
            workspaces: workspaces
                .into_iter()
                .map(|w| CatalogConflictWorkspace { id: w.workspace_id, name: w.name, slug: w.slug })
                .collect(),
        });
    }
    if row.warehouse_id.is_some() {
        return Err(CpError::Conflict(
            "this catalog is an egret warehouse; delete the warehouse instead".into(),
        ));
    }
    registry.delete_catalog(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/{id}/impact",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "What saving this catalog does to each attached engine", body = CatalogImpactDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn catalog_impact(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<CatalogImpactDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    load(&st, id).await?;
    let engines = st
        .engine_mgmt
        .registry()
        .list_catalog_attachments(id)
        .await?
        .into_iter()
        .map(|e| CatalogImpactEngineDto {
            action: if e.engine_type == "starrocks" { "in_place" } else { "restart" }.into(),
            engine_id: e.engine_id,
            engine_name: e.engine_name,
            engine_type: e.engine_type,
        })
        .collect();
    Ok(Json(CatalogImpactDto { engines }))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs/{id}/reapply",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "The unchanged configuration re-applied to every attached engine (e.g. after rotating a secret)", body = Vec<ApplyOutcomeDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn reapply_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ApplyOutcomeDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let row = load(&st, id).await?;
    Ok(Json(propagate(&st, &row).await?.into_iter().map(Into::into).collect()))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_registry_catalogs,
        create_registry_catalog,
        get_registry_catalog,
        update_registry_catalog,
        delete_registry_catalog,
        catalog_impact,
        reapply_registry_catalog,
    ),
    components(schemas(
        RegistryCatalogDto,
        CatalogAttachmentDto,
        CreateRegistryCatalogBody,
        UpdateRegistryCatalogBody,
        ApplyOutcomeDto,
        CatalogUpdateResultDto,
        CatalogImpactDto,
        CatalogImpactEngineDto,
        CatalogConflictBody,
        CatalogConflictEngine,
        CatalogConflictWorkspace,
        ErrorBody,
    )),
    tags((name = "control-plane-catalog-registry", description = "Catalog registry: platform catalogs, their engine attachments and workspace assignment")),
)]
pub struct CatalogRegistryApi;
```

`src/lib.rs`: add `pub mod catalogs;` and in `openapi()` after `doc.merge(monitor::MonitorApi::openapi());` add `doc.merge(catalogs::CatalogRegistryApi::openapi());`. `src/engines.rs` `router()`: after `.merge(crate::monitor::routes())` add `.merge(crate::catalogs::routes())`.

- [ ] **Step 7: Refresh the template (new migration), run the tests**

```bash
psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt
cargo test -p egret-core --lib rbac
cargo test -p egret-controlplane --test catalog_drift
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-query --test registry_workspace_routes_api platform_catalog_registry_read
```
Expected: all green; the three new route tests pass; `catalog_drift` finds `catalog_registry:read|create|update|delete` enforced in `catalogs.rs`; the regression guard passes (and would fail on its PDP assertion if the platform key were `catalog:read`).

- [ ] **Step 8: Regenerate the API description and client**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api
ls src/api/generated/control-plane-catalog-registry
grep -n 'catalogs/\${' src/api/generated/control-plane-catalog-registry/control-plane-catalog-registry.ts
npm run typecheck
cd ..
```
Expected: a new `control-plane-catalog-registry` module; every `catalogs/${…}` path segment in it is wrapped in `encodeURIComponent(String(…))` (if one is not, `gen:api` was bypassed — rerun `npm run gen:api`, never edit generated files); typecheck passes.

- [ ] **Step 9: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt -p egret-core -p egret-controlplane -p egret-query --all-targets -- -D warnings
git branch --show-current
git add migrations/0148_catalog_permissions.sql crates/egret-core/src/rbac.rs crates/egret-controlplane/tests/catalog_drift.rs \
  crates/egret-engine-mgmt crates/egret-query/tests/registry_workspace_routes_api.rs openapi.json
git add -A frontend/src/api/generated
git commit -m "feat(catalog-registry): catalog API and catalog permissions

GET/POST /catalogs, GET/PUT/DELETE /catalogs/{id}, impact and re-apply.
An edit is stored first and then applied to every attached engine in
turn; a failed engine is flagged on its attachment and nothing reverts.
Delete is refused with 409 while engines or workspaces use the catalog.
New catalog_registry:* platform permissions (distinct from the workspace
catalog:read, which a platform grant would otherwise satisfy everywhere);
every platform role gets the counterpart of the warehouse permissions it
holds.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Attach, Retry, workspace assignment, migration report routes

**Files:**
- Modify: `crates/egret-engine-mgmt/src/catalogs.rs`, `src/engines.rs` (route line), `tests/routes.rs`, `openapi.json`, `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Task 7 `attach_to_engine`, `retry_apply`, `CatalogOpError::into_registry`; Task 8 DTOs.
- Produces routes: `POST /engines/{id}/catalogs/{cid}` (attach, `engine:update`, 201 `CatalogAttachmentDto`, 502 on a rejected apply), `POST /engines/{id}/catalogs/{cid}/apply` (Retry, `catalog_registry:update`, 200 `ApplyOutcomeDto`, 502 on failure), `GET/PUT /catalogs/{id}/workspaces` (`catalog_registry:read` / `catalog_registry:update`, `Vec<CatalogWorkspaceDto>`), `GET /catalogs/migration-report?all=` (`catalog_registry:read`), `POST /catalogs/migration-report/acknowledge` (`catalog_registry:update`, 204). DTOs `SetCatalogWorkspacesBody { workspace_ids }`, `CatalogWorkspaceDto { workspace_id, name, slug, assigned_at }`, `MigrationReportRowDto`.

- [ ] **Step 1: Write the failing tests** — append to `tests/routes.rs`:

```rust
#[tokio::test]
async fn attach_detach_and_retry_on_a_trino_engine() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_attach_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &engine_name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let cat_name = unique_catalog_name("bench");
    let (_, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({ "name": cat_name, "kind": "tpch", "config": {} }))).await;
    let cid = j["id"].as_str().unwrap().to_string();
    let reg = registry(&f);
    let (e, c) = (Uuid::parse_str(&engine_id).unwrap(), Uuid::parse_str(&cid).unwrap());

    // A rejected apply is a 502 and writes nothing.
    port.set_fail_update(true);
    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{j}");
    assert_eq!(j["code"], "engine_cluster_apply_failed");
    assert!(reg.get_attachment(e, c).await.unwrap().is_none());
    port.set_fail_update(false);

    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["applyStatus"], "applied");
    assert!(port.last_updated().catalogs.iter().any(|c| c.name == cat_name));
    let (status, _) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "already attached");

    // A StarRocks-only generic catalog cannot be attached to Trino.
    let (_, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({
        "name": unique_catalog_name("paimon"), "kind": "generic",
        "config": { "engines": ["starrocks"] }, "advanced": { "starrocks": { "type": "paimon" } }
    }))).await;
    let sr_only = j["id"].as_str().unwrap().to_string();
    let (status, _) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{sr_only}"), &token, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Retry: a failed attachment is re-applied once the engine is healthy.
    reg.set_apply_failed(e, c, "earlier failure").await.unwrap();
    port.set_fail_update(true);
    let (status, _) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}/apply"), &token, None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(reg.get_attachment(e, c).await.unwrap().unwrap().apply_error.unwrap().contains("injected update failure"));
    port.set_fail_update(false);
    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}/apply"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j["status"], "applied");
    let a = reg.get_attachment(e, c).await.unwrap().unwrap();
    assert_eq!((a.apply_status.as_str(), a.apply_error), ("applied", None));

    // Detach (the shared DELETE) applies first, then removes the attachment.
    let (status, _) = call(&f, "DELETE", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(reg.get_attachment(e, c).await.unwrap().is_none());
    assert!(port.last_updated().catalogs.iter().all(|c| c.name != cat_name));
}

#[tokio::test]
async fn nothing_is_attached_to_a_starrocks_engine_yet() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_sr_admin", true).await;
    let wh_id = seed_provisioned_warehouse_with_secret(&f).await;
    let (status, j) = call(&f, "POST", "/engines", &token, Some(serde_json::json!({
        "name": format!("sr-{}", Uuid::new_v4().simple()), "kind": "starrocks", "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1, "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
    }))).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let engine_id = j["id"].as_str().unwrap().to_string();
    let (_, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({ "name": unique_catalog_name("b"), "kind": "tpch", "config": {} }))).await;
    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{}", j["id"].as_str().unwrap()), &token, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert!(j["error"].as_str().unwrap().contains("StarRocks"), "{j}");
}

#[tokio::test]
async fn workspace_assignment_and_the_migration_report() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cat_ws_admin", true).await;
    let (_, j) = call(&f, "POST", "/catalogs", &token, Some(serde_json::json!({ "name": unique_catalog_name("b"), "kind": "tpch", "config": {} }))).await;
    let cid = j["id"].as_str().unwrap().to_string();
    let ws1 = seed_workspace(&f).await;
    let ws2 = seed_workspace(&f).await;

    let (status, j) = call(&f, "PUT", &format!("/catalogs/{cid}/workspaces"), &token, Some(serde_json::json!({ "workspaceIds": [ws1, ws2] }))).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(j.as_array().unwrap().len(), 2);
    let (status, j) = call(&f, "PUT", &format!("/catalogs/{cid}/workspaces"), &token, Some(serde_json::json!({ "workspaceIds": [ws2] }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(j[0]["workspaceId"], serde_json::json!(ws2));
    let (_, j) = call(&f, "GET", &format!("/catalogs?workspaceId={ws2}"), &token, None).await;
    assert!(j.as_array().unwrap().iter().any(|c| c["id"] == serde_json::json!(cid)));
    let (status, _) = call(&f, "PUT", &format!("/catalogs/{cid}/workspaces"), &token, Some(serde_json::json!({ "workspaceIds": [Uuid::new_v4()] }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown workspace");
    let (status, j) = call(&f, "DELETE", &format!("/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(j["workspaces"][0]["id"], serde_json::json!(ws2));

    let run = Uuid::new_v4();
    registry(&f).insert_report(&[egret_catalog_registry::ReportEntry::info(run, "warehouse_catalog_created", "warehouse w", "w is catalog w")]).await.unwrap();
    let (status, j) = call(&f, "GET", "/catalogs/migration-report", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(j.as_array().unwrap().iter().any(|r| r["runId"] == serde_json::json!(run)), "{j}");
    let (status, _) = call(&f, "POST", "/catalogs/migration-report/acknowledge", &token, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, j) = call(&f, "GET", "/catalogs/migration-report", &token, None).await;
    assert!(j.as_array().unwrap().iter().all(|r| r["runId"] != serde_json::json!(run)));
    let (_, j) = call(&f, "GET", "/catalogs/migration-report?all=true", &token, None).await;
    assert!(j.as_array().unwrap().iter().any(|r| r["runId"] == serde_json::json!(run)));
}
```

(`seed_workspace` already exists in `routes.rs` (≈line 2925) and returns the workspace id.)

- [ ] **Step 2: Run to see them fail**

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test routes -- attach_detach nothing_is_attached workspace_assignment`
Expected: FAIL — POST on `/engines/{id}/catalogs/{cid}` is 405, the other routes 404.

- [ ] **Step 3: Implement** — in `src/engines.rs` `router()`, change the catalog route to

```rust
        .route(
            "/engines/{id}/catalogs/{cid}",
            get(get_catalog)
                .put(update_catalog)
                .post(crate::catalogs::attach_catalog)
                .delete(remove_catalog),
        )
```

In `src/catalogs.rs`, extend `routes()`:

```rust
        .route("/catalogs/migration-report", get(get_migration_report))
        .route("/catalogs/migration-report/acknowledge", post(acknowledge_migration_report))
        .route(
            "/catalogs/{id}/workspaces",
            get(list_catalog_workspaces).put(set_catalog_workspaces),
        )
        .route("/engines/{id}/catalogs/{cid}/apply", post(retry_catalog_apply))
```

and add (with the imports `use crate::catalog_apply::{attach_to_engine, retry_apply, CatalogOpError};`):

```rust
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetCatalogWorkspacesBody {
    workspace_ids: Vec<Uuid>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogWorkspaceDto {
    workspace_id: Uuid,
    name: String,
    slug: String,
    assigned_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationReportRowDto {
    id: i64,
    run_id: Uuid,
    created_at: DateTime<Utc>,
    /// `info` | `warning`.
    severity: String,
    code: String,
    catalog_id: Option<Uuid>,
    engine_id: Option<Uuid>,
    warehouse_id: Option<Uuid>,
    subject: String,
    message: String,
    #[schema(value_type = Object)]
    detail: serde_json::Value,
    acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct MigrationReportQuery {
    /// Include rows already acknowledged.
    #[serde(default)]
    all: bool,
}

async fn engine_or_404(st: &CpState, id: Uuid) -> Result<crate::Engine, CpError> {
    st.engine_mgmt.get_engine(id).await?.ok_or(CpError::NotFound)
}

async fn attachment_dto(st: &CpState, engine_id: Uuid, catalog_id: Uuid) -> Result<CatalogAttachmentDto, CpError> {
    st.engine_mgmt
        .registry()
        .list_catalog_attachments(catalog_id)
        .await?
        .into_iter()
        .find(|r| r.engine_id == engine_id)
        .map(Into::into)
        .ok_or(CpError::NotFound)
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Registry catalog id"),
    ),
    responses(
        (status = 201, description = "Applied to the engine, then attached (an egret warehouse catalog mints a Catalog Client for the engine)", body = CatalogAttachmentDto),
        (status = 400, description = "The engine cannot read this catalog, StarRocks attachment is not supported yet, or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog", body = ErrorBody),
        (status = 409, description = "Already attached, or the engine is being deleted", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply (`code` = `engine_cluster_apply_failed`); nothing was attached", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(crate) async fn attach_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<CatalogAttachmentDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = engine_or_404(&st, id).await?;
    let catalog = load(&st, cid).await?;
    attach_to_engine(&st, &engine, &catalog, None)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok((StatusCode::CREATED, Json(attachment_dto(&st, engine.id, cid).await?)))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}/apply",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Registry catalog id"),
    ),
    responses(
        (status = 200, description = "Re-applied; the attachment is `applied`", body = ApplyOutcomeDto),
        (status = 400, description = "StarRocks re-apply is not supported yet, or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such engine, or the catalog is not attached to it", body = ErrorBody),
        (status = 409, description = "The engine is being deleted", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply; the attachment stays `failed` with the new error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn retry_catalog_apply(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApplyOutcomeDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let engine = engine_or_404(&st, id).await?;
    st.engine_mgmt
        .registry()
        .get_attachment(engine.id, cid)
        .await?
        .ok_or(CpError::NotFound)?;
    retry_apply(&st, &engine, cid)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok(Json(ApplyOutcomeDto {
        engine_id: engine.id,
        engine_name: engine.name,
        status: "applied".into(),
        error: None,
    }))
}

async fn workspace_dtos(st: &CpState, id: Uuid) -> Result<Vec<CatalogWorkspaceDto>, CpError> {
    Ok(st
        .engine_mgmt
        .registry()
        .list_catalog_workspaces(id)
        .await?
        .into_iter()
        .map(|w| CatalogWorkspaceDto {
            workspace_id: w.workspace_id,
            name: w.name,
            slug: w.slug,
            assigned_at: w.assigned_at,
        })
        .collect())
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/{id}/workspaces",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "Workspaces the catalog is assigned to", body = Vec<CatalogWorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_catalog_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<CatalogWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    load(&st, id).await?;
    Ok(Json(workspace_dtos(&st, id).await?))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/catalogs/{id}/workspaces",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    request_body = SetCatalogWorkspacesBody,
    responses(
        (status = 200, description = "The full assignment set was replaced. In SP3 it decides which egret warehouse catalogs a workspace can pick for notebooks; engines are not restricted by it yet", body = Vec<CatalogWorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog or workspace (nothing changed)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_catalog_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<SetCatalogWorkspacesBody>,
) -> Result<Json<Vec<CatalogWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    load(&st, id).await?;
    let mut ids = b.workspace_ids;
    ids.sort();
    ids.dedup();
    if !st.engine_mgmt.missing_workspace_ids(&ids).await?.is_empty() {
        return Err(CpError::NotFound);
    }
    st.engine_mgmt.registry().set_catalog_workspaces(id, &ids).await?;
    Ok(Json(workspace_dtos(&st, id).await?))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/migration-report",
    tag = "control-plane-catalog-registry",
    params(MigrationReportQuery),
    responses(
        (status = 200, description = "What the registry migration did, oldest first (unacknowledged rows unless `all`)", body = Vec<MigrationReportRowDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_migration_report(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<MigrationReportQuery>,
) -> Result<Json<Vec<MigrationReportRowDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let rows = st.engine_mgmt.registry().list_report(q.all).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| MigrationReportRowDto {
                id: r.id,
                run_id: r.run_id,
                created_at: r.created_at,
                severity: r.severity,
                code: r.code,
                catalog_id: r.catalog_id,
                engine_id: r.engine_id,
                warehouse_id: r.warehouse_id,
                subject: r.subject,
                message: r.message,
                detail: r.detail,
                acknowledged_at: r.acknowledged_at,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs/migration-report/acknowledge",
    tag = "control-plane-catalog-registry",
    responses(
        (status = 204, description = "Every open report row acknowledged"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn acknowledge_migration_report(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    st.engine_mgmt.registry().acknowledge_report().await?;
    Ok(StatusCode::NO_CONTENT)
}
```

Add the six handlers to `CatalogRegistryApi`'s `paths(…)` (`attach_catalog, retry_catalog_apply, list_catalog_workspaces, set_catalog_workspaces, get_migration_report, acknowledge_migration_report`) and `SetCatalogWorkspacesBody, CatalogWorkspaceDto, MigrationReportRowDto` to its schemas. The attach path shares `/api/control-plane/engines/{id}/catalogs/{cid}` with the legacy GET/PUT/DELETE in `EngineMgmtApi`; utoipa's `Paths::merge` merges the operations of one path item, so both appear.

- [ ] **Step 4: Run the crate suite**

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt`
Expected: all green, including the three new tests.

- [ ] **Step 5: Regenerate**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && npm run typecheck && cd ..
git status --short openapi.json frontend/src/api/generated
```
Then confirm `post` sits next to `get`, `put` and `delete` under `"/api/control-plane/engines/{id}/catalogs/{cid}"` in `openapi.json` (`rtk proxy grep -n '"/api/control-plane/engines/{id}/catalogs/{cid}"' openapi.json`, then read that block), and that the generated attach/apply/workspaces functions wrap every path parameter in `encodeURIComponent(String(…))`.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
git branch --show-current
git add crates/egret-engine-mgmt openapi.json
git add -A frontend/src/api/generated
git commit -m "feat(catalog-registry): attach, retry, workspace assignment, report

POST /engines/{id}/catalogs/{cid} applies the catalog to a Trino engine
and only then records the attachment (502 and nothing written when
Kubernetes refuses); Retry re-applies a failed attachment; catalogs are
assigned to workspaces; the migration report is listed until
acknowledged. StarRocks attachment is refused until SP3b.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 10: Everything else that read `engine_catalogs` or warehouse ownership

**Files:**
- Modify: `crates/egret-policy/src/store.rs` (`tag_assignments_for_service`), `crates/egret-policy/tests/download_tags_db.rs`; `crates/egret-bin/src/main.rs` (`EngineBindingReaderImpl::engine_catalog_names`); `crates/egret-query/src/registry/repo.rs` (`list_engine_catalogs`), `crates/egret-query/tests/registry_workspace_routes_api.rs`; `crates/egret-catalog-engine/src/{store/embedded.rs,warehouse_admin/mod.rs,warehouse_admin/error.rs}`, `crates/egret-catalog-engine/tests/warehouse_admin_routes.rs`; `crates/egret-notebook/src/routes.rs`; `openapi.json`, `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Task 5 `RegistryStore::{attached_catalog_names, ensure_warehouse_catalog, get_catalog_for_warehouse, list_catalog_attachments, release_warehouse_catalog, warehouse_visible_to_workspace}`, `CatalogStore::registry()`.
- Produces: `CatalogStore::warehouse_visible_to_workspace(&self, warehouse_id: Uuid, workspace_id: Uuid) -> Result<bool, CatalogError>`; `impl From<RegistryError> for CatalogHttpError`.

- [ ] **Step 1: Ranger tag export — failing test first.** In `crates/egret-policy/tests/download_tags_db.rs`, `seed_bound_warehouse` returns `(warehouse_id, catalog_name)` and seeds the registry instead of `engine_catalogs`. Replace the `engine_catalogs` insert (and the comment above it) with:

```rust
    // The engine reaches the warehouse through its registry catalog; the tag
    // export must emit THAT catalog's name (the SQL name Trino sees).
    let catalog_name = format!("tagcat_{}", Uuid::new_v4().simple());
    let (catalog_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&catalog_name)
    .bind(wh_id)
    .fetch_one(pool)
    .await
    .expect("insert registry catalog");
    sqlx::query("INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)")
        .bind(eng_id)
        .bind(catalog_id)
        .execute(pool)
        .await
        .expect("attach catalog");
    (wh_id, catalog_name)
```

Change the doc comment to "Returns (warehouse_id, the registry catalog name the engine reaches it by)", the two call sites to `let (wh_id, catalog_name) = seed_bound_warehouse(…)`, and the two assertions `== "egret"` to `== serde_json::json!(catalog_name)` (keep the comments' point: the catalog name, not the warehouse name). Remove the now-unused `wh_name` binding if clippy says so.

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test download_tags_db`
Expected: FAIL — the query still joins `engine_catalogs`.

- [ ] **Step 2: Ranger tag export — implementation.** In `crates/egret-policy/src/store.rs` `tag_assignments_for_service`, replace the two final joins and keep everything else:

```rust
               JOIN registry.catalogs c
                    ON c.warehouse_id = a.warehouse_id
               JOIN registry.engine_catalog_attachments ea
                    ON ea.catalog_id = c.id
               JOIN engine_mgmt.engines e
                    ON e.id = ea.engine_id
                   AND e.authz_service_id = COALESCE(tp.id, s.id)
```

(replacing the `JOIN engine_mgmt.engines e ON e.warehouse_id = a.warehouse_id AND …` and `JOIN engine_mgmt.engine_catalogs c ON c.engine_id = e.id AND c.is_primary AND c.kind = 'egret-warehouse'` lines). Rewrite the paragraph of the doc comment that explains "Join the engine's primary `engine_catalogs` row" to: "Every warehouse an engine attaches is served under its registry catalog's name — the SQL name on every engine (SP3 D5) — not only the engine's primary, and not an assumed `egret`."

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy`
Expected: green.

- [ ] **Step 3: Policy resource autocomplete** — `crates/egret-bin/src/main.rs`, `EngineBindingReaderImpl::engine_catalog_names`, replace its body with:

```rust
        self.store
            .registry()
            .attached_catalog_names(engine_id)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))
```

- [ ] **Step 4: Workspace engine view — failing test first.** In `crates/egret-query/tests/registry_workspace_routes_api.rs`, `seed_assigned_engine_with_catalog` returns a fifth element, the catalog name. Replace its `engine_catalogs` insert with:

```rust
    // A registry catalog (the engine's warehouse) referencing the secret,
    // attached to the engine.
    let catalog_name = format!("lakehouse_{}", Uuid::new_v4().simple());
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&catalog_name)
    .bind(wh_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed registry catalog");
    sqlx::query("INSERT INTO registry.catalog_secret_refs (catalog_id, secret_id) VALUES ($1, $2)")
        .bind(catalog_id)
        .bind(secret_ref)
        .execute(&db.pool)
        .await
        .expect("seed secret ref");
    sqlx::query("INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)")
        .bind(engine_id)
        .bind(catalog_id)
        .execute(&db.pool)
        .await
        .expect("seed attachment");
```

return `(engine_id, engine_name, store_name, secret_name, catalog_name)`, update the doc comment, the return type `(Uuid, String, String, String, String)`, both call sites (`let (engine_id, engine_name, storage_name, secret_name, catalog_name) = …` and `let (_engine_id, _name, _s, _sec, _cat) = …`), and the assertion `serde_json::json!("lakehouse")` → `serde_json::json!(catalog_name)`. The `kind` assertion (`"egret-warehouse"`) and the `secret_name` assertion stay.

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-query --test registry_workspace_routes_api ws_engine_view`
Expected: FAIL (the repo still reads `engine_catalogs`).

- [ ] **Step 5: Workspace engine view — implementation.** In `crates/egret-query/src/registry/repo.rs`, `list_engine_catalogs`, replace the SQL with:

```rust
            "SELECT c.name, \
                    CASE WHEN c.warehouse_id IS NOT NULL THEN 'egret-warehouse' \
                         WHEN c.kind = 'jdbc' THEN c.config->>'dialect' \
                         WHEN c.kind = 'generic' THEN COALESCE(c.advanced->'trino'->>'connector.name', 'generic') \
                         ELSE c.kind END AS kind, \
                    (SELECT min(ws.name) FROM registry.catalog_secret_refs r \
                       JOIN workspace_secrets ws ON ws.id = r.secret_id \
                      WHERE r.catalog_id = c.id) AS secret_name \
               FROM registry.engine_catalog_attachments a \
               JOIN registry.catalogs c ON c.id = a.catalog_id \
              WHERE a.engine_id = $1 \
              ORDER BY c.name",
```

and its doc comment to: "Catalogs attached to an engine (SP3 registry), with the legacy `kind` label the workspace view shows (`egret-warehouse` or the Trino connector name) and the NAME of one secret the catalog references (`None` when none; the value is never read). Ordered by name."

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-query --test registry_workspace_routes_api`
Expected: green.

- [ ] **Step 6: Warehouse routes keep the registry in step — failing test first.** Append to `crates/egret-catalog-engine/tests/warehouse_admin_routes.rs`:

```rust
/// SP3: creating a warehouse creates its registry catalog assigned to the
/// warehouse's workspace; deleting it is refused while an engine attaches the
/// catalog, and otherwise removes the catalog with the warehouse.
#[tokio::test]
async fn warehouse_create_and_delete_keep_the_registry_in_step() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "wh_registry_admin", true).await;
    let ws: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'Reg WS') RETURNING id")
        .bind(format!("reg-ws-{}", Uuid::new_v4().simple()))
        .fetch_one(&f.db.pool).await.unwrap();
    let other_ws: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'Other WS') RETURNING id")
        .bind(format!("reg-ws-{}", Uuid::new_v4().simple()))
        .fetch_one(&f.db.pool).await.unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) VALUES ($1, 'us-east-1', 'b', 'k') RETURNING id")
        .bind(format!("reg-sc-{}", Uuid::new_v4().simple()))
        .fetch_one(&f.db.pool).await.unwrap();
    let wh_name = format!("Reg-WH-{}", Uuid::new_v4().simple());

    let resp = f.app.clone().oneshot(req("POST", "/warehouses", &token, Some(serde_json::json!({
        "name": wh_name, "workspaceId": ws, "storageConnectionId": sc
    })))).await.unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let wh_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let reg = store.registry();
    let cat = reg.get_catalog_for_warehouse(wh_id).await.unwrap().expect("catalog created with the warehouse");
    assert_eq!(cat.name, egret_catalog_registry::normalize_catalog_name(&wh_name));
    assert!(store.warehouse_visible_to_workspace(wh_id, ws).await.unwrap());
    assert!(!store.warehouse_visible_to_workspace(wh_id, other_ws).await.unwrap());
    reg.set_catalog_workspaces(cat.id, &[other_ws]).await.unwrap();
    assert!(store.warehouse_visible_to_workspace(wh_id, other_ws).await.unwrap());

    // An engine on ANOTHER warehouse attaches this warehouse's catalog.
    let engine_wh = store.create_warehouse(&format!("eng-wh-{}", Uuid::new_v4().simple()), ws, sc).await.unwrap();
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id")
        .bind(format!("reg-eng-{}", Uuid::new_v4().simple())).bind(engine_wh.id)
        .fetch_one(&f.db.pool).await.unwrap();
    sqlx::query("INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)")
        .bind(engine_id).bind(cat.id).execute(&f.db.pool).await.unwrap();

    let resp = f.app.clone().oneshot(req("DELETE", &format!("/warehouses/{wh_id}"), &token, None)).await.unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "{j}");
    assert!(reg.get_catalog_for_warehouse(wh_id).await.unwrap().is_some());

    reg.delete_attachment(engine_id, cat.id).await.unwrap();
    let resp = f.app.clone().oneshot(req("DELETE", &format!("/warehouses/{wh_id}"), &token, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(reg.get_catalog(cat.id).await.unwrap().is_none(), "catalog went with its warehouse");
}
```

`egret-catalog-registry` is a normal dependency of `egret-catalog-engine` (Task 5), so the test can name it.

Run: `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-engine --test warehouse_admin_routes warehouse_create_and_delete`
Expected: FAIL — no catalog is created, `warehouse_visible_to_workspace` does not exist.

- [ ] **Step 7: Warehouse routes — implementation.**

`crates/egret-catalog-engine/src/warehouse_admin/error.rs`, after the `CatalogError` impl:

```rust
impl From<egret_catalog_registry::RegistryError> for CatalogHttpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        use egret_catalog_registry::RegistryError as E;
        match e {
            E::NotFound(_) => CatalogHttpError::NotFound,
            E::AlreadyExists(s) => CatalogHttpError::AlreadyExists(s),
            E::Conflict(s) => CatalogHttpError::Conflict(s),
            E::Invalid(v) => CatalogHttpError::BadRequest(v.to_string()),
            E::Secrets(s) => s.into(),
            E::Db(_) => CatalogHttpError::Internal,
        }
    }
}
```

`crates/egret-catalog-engine/src/store/embedded.rs`, in the warehouses section of `impl CatalogStore` after `get_warehouse_by_name`:

```rust
    /// Whether a workspace may use a warehouse (SP3 §4.6): its registry catalog
    /// is assigned there, or — for a warehouse the boot backfill has not reached
    /// — the warehouse is the workspace's own.
    pub async fn warehouse_visible_to_workspace(
        &self,
        warehouse_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<bool, CatalogError> {
        self.registry()
            .warehouse_visible_to_workspace(warehouse_id, workspace_id)
            .await
            .map_err(|e| match e {
                egret_catalog_registry::RegistryError::Db(e) => CatalogError::Db(e),
                other => CatalogError::BadRequest(other.to_string()),
            })
    }
```

`crates/egret-catalog-engine/src/warehouse_admin/mod.rs`:

- `create_warehouse`: after the final `let w = st.store.get_warehouse(w.id)…;` and before `Ok((StatusCode::CREATED, …))`, add

```rust
    // SP3: the warehouse is a registry catalog, assigned to this workspace. A
    // failure is not fatal — engine create and the boot backfill ensure it too.
    if let Err(e) = st
        .store
        .registry()
        .ensure_warehouse_catalog(
            &egret_catalog_registry::WarehouseIdentity {
                id: w.id,
                name: &w.name,
                workspace_id: w.workspace_id,
            },
            None,
        )
        .await
    {
        tracing::warn!(warehouse = %w.name, error = %e, "warehouse created but its registry catalog was not");
    }
```

(check `tracing` is a dependency of egret-catalog-engine: `grep -n '^tracing' crates/egret-catalog-engine/Cargo.toml`; it is used elsewhere in the crate) and change the 201 description to `"Warehouse created (egret row + lakekeeper warehouse when a provisioner is wired) with its registry catalog, assigned to the workspace"`.

- `delete_warehouse`: replace `st.store.delete_warehouse(id).await?;` with

```rust
    // SP3: the warehouse's registry catalog goes with it (FK cascade) unless an
    // engine attaches it; its workspace assignments are cleared first, as the
    // warehouse's workspace link never blocked a delete before.
    let registry = st.store.registry();
    if let Some(cat) = registry.get_catalog_for_warehouse(id).await? {
        let engines = registry.list_catalog_attachments(cat.id).await?;
        if !engines.is_empty() {
            let names: Vec<String> = engines.into_iter().map(|e| e.engine_name).collect();
            return Err(CatalogHttpError::Conflict(format!(
                "the warehouse's catalog {} is attached to engine(s) {}; detach it first",
                cat.name,
                names.join(", ")
            )));
        }
        registry.release_warehouse_catalog(id).await?;
    }
    st.store.delete_warehouse(id).await?;
```

and add `(status = 409, description = "The warehouse's catalog is attached to an engine", body = ErrorBody),` to its responses.

- [ ] **Step 8: Notebook warehouse selection** — `crates/egret-notebook/src/routes.rs`, `set_notebook_warehouse`: replace

```rust
    if wh.workspace_id != ctx.workspace_id.0 {
```

with

```rust
    if !st
        .catalog
        .warehouse_visible_to_workspace(wh.id, ctx.workspace_id.0)
        .await
        .map_err(cat_err)?
    {
```

(the body — the 400 "warehouse not in this workspace" — stays), update the comment above to "The warehouse must exist AND its catalog be assigned to this workspace (SP3 §4.6; cross-tenant → 400, no partial mutation).", and the 400 response description to `"Warehouse not found or not assigned to this workspace"`.

- [ ] **Step 9: Run the touched suites**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-engine --test warehouse_admin_routes
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-notebook
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-query --test registry_workspace_routes_api
cargo check -p egret-bin
```
Expected: all green. The notebook tests seed warehouses without registry catalogs, so they exercise the "not backfilled yet" branch; `set_notebook_warehouse_rejects_foreign_warehouse` still gets its 400.

- [ ] **Step 10: Regenerate** (three response descriptions changed)

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && npm run typecheck && cd ..
git status --short openapi.json frontend/src/api/generated
```
Expected: drift test passes; commit whatever `gen:api` rewrote.

- [ ] **Step 11: Gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-policy -p egret-query -p egret-catalog-engine -p egret-notebook -p egret-bin --all-targets -- -D warnings
git branch --show-current
git add crates/egret-policy crates/egret-query crates/egret-catalog-engine crates/egret-notebook crates/egret-bin/src/main.rs openapi.json
git add -A frontend/src/api/generated
git commit -m "feat(catalog-registry): read catalogs from the registry everywhere

Ranger tag export serves every attached warehouse under its registry
catalog name instead of assuming egret, policy autocomplete and the
workspace engine view list attached catalogs, warehouse create/delete
keep the warehouse's catalog in step, and notebook warehouse selection
follows catalog workspace assignment.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Full gate (stop before the PR)

**Files:** none new — this task only verifies, and fixes what it finds in the task that introduced it.

- [ ] **Step 1: Branch and tree**

```bash
git branch --show-current
git status --short
git log --oneline origin/main..HEAD
```
Expected: `feat/catalog-registry-sp3a`, a clean tree, ten commits (Tasks 1–10).

- [ ] **Step 2: Rust gates**

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: fmt clean; clippy clean except the two pre-existing `egret-workflow` findings — record their text; any other finding is this branch's and is fixed.

- [ ] **Step 3: Template, then the whole workspace once**

```bash
psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test --workspace --no-fail-fast
cargo test -p egret-bin -p egret-workflow --lib --test oracle --no-fail-fast
```
Expected: no failures. Read the `finished in` of the new test binaries (`egret-catalog-registry` `store`, `backfill_single_primary`, `backfill_several_primaries`; `egret-engine-mgmt` `routes`): each is seconds, not milliseconds. A failure outside this branch's crates is called "pre-existing" only after running it on `origin/main` in a separate worktree and seeing it fail there too.

- [ ] **Step 4: API description and client**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > /tmp/planSP3a-openapi.json
cmp /tmp/planSP3a-openapi.json openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend
npm run gen:api
git status --short src/api/generated
npm run lint
npm run typecheck
npm test
npm run build
cd ..
```
Expected: `cmp` silent; drift test passes; `git status` after `gen:api` shows **nothing** (CI's frontend job would fail otherwise — commit the difference into the task that caused it); lint, typecheck, tests and build pass. (Use the scratchpad path instead of `/tmp` if the executing session has one; prefix `planSP3a-`.)

- [ ] **Step 5: Kind cluster — manual, reported as manual.** Deploy with `just deploy-kind` (or the OIDC variant the kind cluster runs), then with `command kubectl --context kind-egret exec -n egret-system deploy/postgres -- psql -U egret -d egret -Atc "…"`:
  - `SELECT count(*) FROM registry.catalogs WHERE warehouse_id IS NOT NULL` → 24.
  - `SELECT name FROM registry.catalogs WHERE warehouse_id = 'a946808b-c005-424e-9a44-21b4793261b5'` → `egret`.
  - `SELECT code, count(*) FROM registry.migration_report GROUP BY 1` → `warehouse_catalog_created` 24, `primary_named_egret` 1, `secret_extracted` 1, no `backfill_failed`.
  - `GET /api/control-plane/engines/{demo-trino id}/catalogs` in the browser session → one primary catalog `egret`.
  - Attach a PostgreSQL catalog (`POST /api/control-plane/catalogs` kind `jdbc`, then `POST …/engines/{id}/catalogs/{cid}`), run `SHOW SCHEMAS FROM <name>` via `POST …/engines/{id}/catalogs/{cid}/test`, rotate the password secret, `POST /api/control-plane/catalogs/{cid}/reapply`, and test again.
  Report each result as "manual, on kind", never as a test.

- [ ] **Step 6: Stop.** Do not push and do not open a PR. Report: the ten commit hashes, the gate outputs (with the two recorded `egret-workflow` clippy findings), the kind results, and anything left open.

---

## Self-review (against the spec)

**Coverage (§6 SP3a):** schema — Task 4; typed config + validation — Task 1 (+ collision in Task 2); Trino rendering — Task 2; migration + backfill + report — Tasks 4 and 6 (report API Task 9); registry API — Task 8; attachment API — Task 9 (+ legacy DELETE as detach, Task 7); Trino attach/detach — Tasks 7 and 9; edit propagation with per-engine apply status and Retry — Tasks 7 (`propagate`, `mark_engine_applied`), 8 (PUT, re-apply), 9 (Retry); workspace assignment (§4.6) — Tasks 5, 9, 10; permissions (§4.7) — Task 8; regenerated client — Tasks 7–10; legacy endpoints over the registry — Task 7; tag export (§4.8.4) — Task 10; secrets page "which catalogs reference this secret" — `GET /catalogs?secretId=` (Task 8) and `used_by` (Task 5). §7 tests: rendering per kind and dialect (Task 2), validation per kind (Task 1), migration scenarios with asserted report rows (Task 6), routes incl. one failing engine, Retry, delete refusals, real-PDP gates (Tasks 8–9), live kind check (Task 11, manual).

**Deliberately not here:** StarRocks rendering/DDL, attach, propagation and the generalised bootstrap (SP3b; the seam is `catalog_apply::apply_starrocks` and `CatalogDef::can_read`); the Catalogs UI, the redirect of Warehouses, dropping `engine_catalogs`, `engines.warehouse_id`, `catalog.warehouses.workspace_id` and `warehouse:*` (SP3c); the catalog "Test" TCP fallback (§5, SP3c).
