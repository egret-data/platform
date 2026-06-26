# CE-1 Spike — Findings (input to CE-2..8)

**Date:** 2026-06-26
**Branch:** `feat/catalog-engine-lakekeeper`
**Result: ✅ THIN-EMBED PROVEN VIABLE END-TO-END.** The embedded lakekeeper Iceberg REST router runs in-process and round-trips a real catalog op (config → create namespace → create table → load) through lakekeeper's store, persisting to its Postgres schema, with the S3 warehouse validating against MinIO. Commits: `5c110e3` (deps) · `71945cb` (migrator) · `9cdd90a` (router) · `abf4f74` (round-trip).

---

## 1. Feasibility (the headline)
- **Single `iceberg` version:** egret and lakekeeper both pin fork rev `bb70173` → one `iceberg` 0.9.0 in the tree. No type conflict. This is the enabler.
- **Builds + round-trips:** `egret-catalog-engine` compiles with `lakekeeper` + `lakekeeper-storage-postgres` + `lakekeeper-io`; the round-trip test passes (idempotent).

## 2. Build couplings (record; resolve before merge)
- **rustflags global:** `.cargo/config.toml` sets `--cfg tracing_unstable --cfg tokio_unstable` (lakekeeper uses these). Applies to the WHOLE egret build; committed, so CI inherits it. Coupling, not a bug.
- **sqlx dual-version:** egret workspace = sqlx **0.8**; lakekeeper = **0.9**. `egret-catalog-engine` depends on sqlx 0.9 directly for lakekeeper's pool types. Two `Pool<Postgres>` types → **egret cannot share its pool with lakekeeper**; the engine keeps its own. Both versions compile (size/time cost). Goes away if egret upgrades to sqlx 0.9 workspace-wide.
- **SQLX_OFFLINE:** `.cargo/config.toml` sets `SQLX_OFFLINE=true` + `SQLX_OFFLINE_DIR=../lakekeeper/.sqlx` (lakekeeper uses compile-time `sqlx::query!` macros). Safe because egret's own crates use ZERO compile-time sqlx macros. **Fragile:** sibling-relative path; breaks if egret is checked out without the lakekeeper sibling.
- **Path deps → git pin (before merge):** deps are `path = "../../../lakekeeper/crates/..."` to the sibling checkout — not reproducible. **CE-2 must switch to a git-pinned rev** (and re-point `SQLX_OFFLINE_DIR`, or vendor the `.sqlx` cache).
- **`test-utils` feature in `[dependencies]`:** needed for the bootstrap principal (see §6); pulls `lakekeeper-io/storage-in-memory` into the artifact. **CE-2: move to `[dev-dependencies]` and replace the bootstrap principal.**

## 3. Migrator coexistence — SOLVED (search_path)
- Build a dedicated sqlx-0.9 pool with `SET search_path = lakekeeper, public` (after_connect); `CREATE SCHEMA IF NOT EXISTS lakekeeper` first. lakekeeper's 63 migrations + its `_sqlx_migrations` land in schema `lakekeeper`; egret's 24 stay in `public`. No leak. lakekeeper documents+tests this (custom-schema migration, issues #1519/#1707). **No separate database needed.**
- API: `lakekeeper_storage_postgres::migrations::migrate_core_only(&pool)` (= `migrate(pool, vec![])`; the core call from `lakekeeper-bin/main.rs`, minus authorizer/post-hooks).

## 4. The embed surface — state + router (clean, headless)
- Router: `lakekeeper::api::iceberg::new_v1_full_router::<EngineServer, EngineState>().with_state(state)`.
  - `EngineServer = lakekeeper::server::CatalogServer<PostgresBackend, AllowAllAuthorizer, SecretsState>` (blanket-impls all 6 endpoint traits — config/namespace/tables/metrics/s3_signer/views; you never name them).
  - `EngineState = lakekeeper::service::State<AllowAllAuthorizer, PostgresBackend, SecretsState>` (blanket `ThreadSafe`).
- `State` has 8 fields, ALL constructible headless: `authz` (`AllowAllAuthorizer{server_id}`, server_id via `PostgresBackend::get_server_info`), `catalog`/`secrets` (`*::from_pools`), `contract_verifiers` (`::default()`), `events` (`EventDispatcher::new(vec![])`), `registered_task_queues` (**empty** `TaskQueueRegistry` — the router only reads `api_config()`, spawns NO workers), `license_status`/`build_info` (static defaults). → Thin REST embed needs none of the bin's background plumbing.
- **CE-6 implication:** GC/expire/soft-delete are the task workers — an empty registry serves REST but runs NO maintenance. CE-6 must register the real task queues + spawn the worker loop.

## 5. Routing shape (CE-2/CE-3)
- Routes mount at top level in the bare router; the real server nests them under `/catalog/v1` (`api/router.rs`).
- **`{prefix}` = the warehouse UUID** (`Prefix: TryFrom → WarehouseId`, template `{warehouse_id}`). The warehouse *name* is used only in `GET /config?warehouse=<name>`, whose response `defaults.prefix` returns the UUID for all subsequent paths. → egret's slug→warehouse mapping resolves to a UUID prefix.

## 6. Provisioning API (CE-2)
- Management `Service` trait methods (on `lakekeeper::api::management::v1::ApiServer<C,A,S>`): `server::Service::bootstrap`, `project::Service::create_project`, `warehouse::Service::create_warehouse`. Signature: `(request, ApiContext<State>, RequestMetadata)`. **Always needs an actor; the authorizer is consulted.**
- `bootstrap` seeds the **default project** (OSS: nil UUID) when default-project is enabled → no separate `create_project`.
- **`create_warehouse` does a LIVE S3 `validate_access`** at create time — the bucket must exist + be writable (not a pure DB insert).
- **Bootstrap needs a principal WITH a user_id** — the internal actor (`new_lakekeeper_internal`, `user_id()=None`) fails `MissingUserId` (400). Spike used `RequestMetadata::test_user(...)` (test-utils). **CE-2: bootstrap with a real egret admin principal (first-boot).**
- **Idempotency:** re-creating a warehouse with the same name+bucket+prefix returns `CreateWarehouseStorageProfileOverlap` (400, before the name-unique check), NOT a 409 name conflict. **CE-2: look up the warehouse by name first, don't sniff create-time errors.**

## 7. Principal injection = the CE-3 bridge
- The bare router has NO authn middleware → handlers' `Extension<RequestMetadata>` would fail. A thin `axum::middleware::from_fn` layer inserts a `RequestMetadata` per request. The spike used `RequestMetadata::new_lakekeeper_internal(uuid)` (production-public; `bypasses_control_plane_authz=true`, project_id None → default project).
- **CE-3:** replace the injected metadata with one built from egret's **authenticated token principal** (+ workspace→project/warehouse, + per-token scope). This `from_fn` layer is exactly where egret's `catalog_tokens` → lakekeeper principal bridge lives. It sits in front, so egret's token auth + per-token scope can deny BEFORE lakekeeper (incl. before S3 sign).

## 8. Authz (CE-4)
- Spike uses `AllowAllAuthorizer`. The `Authorizer` trait is the swap point: egret's impl = workspace RBAC (`catalog:read|write`) + per-token scope (permissions + namespace_scope). The authorizer is consulted on both data and management ops. The internal actor's `bypasses_control_plane_authz` is the model for egret's own system/provisioning ops.

## 9. Storage + secrets (CE-5)
- `StorageProfile::S3` + `S3Credential::AccessKey` via lakekeeper-io; MinIO works (`S3Flavor::S3Compat`, path-style, `sts_enabled=false`, region cosmetic). Secrets via postgres `SecretsState` for the spike; CE-5 can bridge `egret-secrets` or keep lakekeeper's postgres secrets.

## 10. Not mounted into egret-server (yet)
- Spike drives the router via `tower::oneshot` (no socket). Mounting into `egret-server` pulls the full lakekeeper dep tree into that crate's build; do it behind a config flag in CE-2/CE-7.

---

## Net effect on the CE-2..8 plans
- **CE-2 (tenancy+provisioning):** git-pin deps; real admin-principal bootstrap; warehouse-by-name idempotency; workspace slug → warehouse(UUID); move `test-utils` to dev. Mount into egret-server behind a flag.
- **CE-3 (authn+scope):** implement the `from_fn` principal layer from egret `catalog_tokens` (+ scope) → `RequestMetadata`.
- **CE-4 (authz):** implement the `Authorizer` trait = RBAC + token scope; deny before sign.
- **CE-5 (storage):** storage profiles + secrets bridge (S3 now).
- **CE-6 (GC):** register task queues + spawn workers (the empty registry serves REST but runs no maintenance).
- **CE-7/CE-8:** UI/internal-API onto lakekeeper data; retire egret's MVP catalog store/schema.
