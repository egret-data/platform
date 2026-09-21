# StarRocks on the Catalog Registry — SP3b Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove `catalog_apply::STARROCKS_NOT_YET`. A registry catalog can be attached to, edited on, re-applied to and detached from a StarRocks engine: a pure StarRocks renderer (`CREATE` / `ALTER … SET` / `DROP`, plus a redacted form for display and logs), a `can_read` matrix that means something, pinned JDBC drivers, an injectable DDL runner with FE introspection and drift detection, an attach that records-then-converges, a bootstrap that converges **every** attachment and writes per-attachment apply status and is reachable from the Retry route, and routes whose OpenAPI tells the truth about a `201` whose attachment is `failed`.

**Architecture:** `egret-catalog-registry` gains `src/render/starrocks.rs` — pure, engine-free: a typed `CatalogDef` becomes an ordered property map whose values are either literals or secret references, with text builders for the three statements and a redacted form; the pinned per-dialect JDBC driver table lives beside it. The three render types shared with Trino (`RenderError`, `StorageRender`, `WarehouseRender`) move up into `src/render/mod.rs`. `egret-engine-mgmt` gains `src/starrocks_ddl.rs`: a `StarRocksDdl` trait (execute / query), the real `mysql_async` runner, `FeTarget` resolution from an `Engine` row (host, port, root password, pinned FE certificate), `SHOW CATALOGS` / `SHOW CREATE CATALOG` introspection and the absent/matching/drifted comparison. `catalog_apply::apply_starrocks` becomes real and keeps `check_engine_supports` as the single pre-mint refusal point for both the registry attach and the legacy add route. `engines.rs::spawn_starrocks_catalog_wire` stops being a one-catalog wire and becomes a converge-every-attachment loop over `EngineMgmtState`, keeping its six `engine_still_current` checkpoints — now proven behaviourally with a fake runner instead of by grepping its own source.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9 / Postgres, utoipa 5, serde, mysql_async, tokio), orval via `npm run gen:api`.

**Spec:** `docs/en/specs/2026-09-21-starrocks-catalog-registry-sp3b-design.md` (D1–D8 govern), over `docs/en/specs/2026-09-19-catalog-registry-design.md` §3, §4.3, §4.4, §4.5 and the SP3b row of §6.

## Global Constraints

- Branch: `feat/starrocks-catalog-registry-sp3b`, cut from `origin/main` at or after `30a5e541` (SP3a, #587). The plan document itself lands on `docs/sp3b-starrocks-catalogs`; **do not implement on that branch.** Never commit on `main`; run `git branch --show-current` before every commit and stop if it does not print `feat/starrocks-catalog-registry-sp3b`. Never `git stash`.
- `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` in every shell that runs cargo.
- DB tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` and run cargo test under `env -u EGRET_ALLOW_DB_SKIP`. **Never** the dev database `egret`. A skipped DB test prints `ok`: read every `finished in` — a DB test binary that finishes in well under 0.3 s did no database work.
- SP3b adds exactly one migration, `0149_attachment_apply_pending.sql` (Task 3). After adding it, drop the test template once: `psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'` (the migrations CLI reads `EGRET_DATABASE__URL`, not `DATABASE_URL`).
- Never run two test suites at once. Per-task runs use `-p <crate>`; `cargo test --workspace` runs exactly once, in Task 7.
- Rust gate per task: `cargo fmt --all -- --check` and `cargo clippy -p <crates touched> --all-targets -- -D warnings`. (Whole-workspace clippy has two pre-existing `egret-workflow` findings; Task 7 records them rather than fixing them.)
- OpenAPI: regenerate with `rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json` (without `rtk proxy` the RTK hook filters stdout and corrupts the file); drift gate `cargo test -p egret-server --lib committed_openapi_json_is_current`.
- Frontend client: `cd frontend && npm run gen:api` — never bare `orval`. CI's frontend job runs `npm run gen:api` and **fails on any drift**, so commit exactly what `gen:api` produced (`git add -A frontend/src/api/generated`), including churn you did not cause.
- sqlx 0.9: `sqlx::query*` takes `&'static str`. Every SQL string in this plan is a literal or a `concat!` of literals; never build SQL with `format!`. **This rule is about the metastore.** StarRocks DDL is *not* sqlx: it is built with `format!` and run over `mysql_async`, which is exactly why every interpolated value goes through `escape_dq` / `escape_ident` and every test asserts the escaping.
- **Standing rules (SP1 Part B) carried forward:** egret never deletes a secret value an engine may still hold; engine delete never deletes a Ranger profile; `apply_built` refuses when the engine row is gone or `deleting`; the engine's **primary** catalog can never be detached.
- **D6 — the DDL text is secret.** A StarRocks catalog's credentials are inline in the statement. Never log a rendered statement (not at `debug`, not in a test that prints), never store one in `apply_error`, never return one in an API response. Only `StarRocksCatalogDdl::redacted()` may be logged. Every FE error string is passed through `scrub_ddl(err, &ddl)` before it reaches a log, a column or a response.
- **D1 — StarRocks attach records, then converges** (the opposite order to Trino's apply-then-write), because a fresh FE is unreachable for 5–10 minutes. Detach still applies first. Nothing about the Trino path changes.
- `check_engine_supports` stays the **single** pre-mint refusal point for both the registry attach route and the legacy add route (`engines.rs::add_catalog`), which calls it *before* `create_catalog_replacing_orphan` destroys a same-named orphan. Do not add a second refusal point anywhere after a mint or a destroy.
- Engine-gated live tests use `EGRET_TEST_STARROCKS_URL=mysql://localhost:9030` against `just up`'s `starrocks/allin1-ubuntu:latest`, the convention `egret-query`, `egret-dataapi` and `egret-workflow` already use. They self-skip when it is unset, so they are never part of `just check`.
- RTK rewrites shell commands. Any command whose raw stdout is piped or redirected runs as `rtk proxy <cmd>`; `rtk grep` swallows piped stdin — never verify a diff with it.
- Commits: Conventional Commits; body ends with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Never override the author.

## Design statements checked against the tree, and the decisions they forced

Read on 2026-09-21 against `main` at `30a5e541`. Where the design and the tree disagree, the row says which one this plan follows.

| # | Statement | Verdict | Decision in this plan |
|---|---|---|---|
| 1 | D5 "a dialect with no pinned URL and no override is **refused at validation time**" | **Not reachable as written.** `CatalogDef::validate` (`config.rs:631`) is a pure function in a crate with no deployment configuration and no engine: its StarRocks probe must use a fixed probe input, so it cannot see an operator's driver table. | The driver table is compiled in (`DEFAULT_SR_JDBC_DRIVERS`, one pinned entry per dialect, so validation refuses no dialect that exists today) and an operator's `egret.toml` may only *override* entries. Validation refuses what it really can see: an incomplete per-catalog override (`advanced.starrocks.driver_url` without `driver_class`, or either one empty). A deployment that blanks a driver is refused at render time with `RenderError::NoJdbcDriver(dialect)`, which surfaces as the attachment's `apply_error`. Task 2. |
| 2 | D5 "`advanced.starrocks.driver_url` overrides for one catalog" vs §4.3 "a key that collides with a typed key is refused at save time" | **Contradiction.** The typed JDBC render emits `driver_url` and `driver_class`, so an override of either would be refused as a collision before it could override anything. | `driver_url` and `driver_class` are the two **overridable** typed keys: `render_starrocks` consumes them from `advanced.starrocks` *before* rendering the typed block, emits the operator's value instead of the pinned one, and does not re-append them — so no collision is raised for exactly these two keys, and for nothing else. The whitelist is one `const SR_OVERRIDABLE: [&str; 2]` and its test names the key that is still refused. Task 1, Task 2. |
| 3 | D4's `can_read` matrix | **Denies nothing that exists today.** The listed StarRocks kinds (iceberg, hive, deltalake, hudi, elasticsearch, tpch/tpcds, jdbc × six dialects) are *every* typed kind and *every* `JdbcDialect` variant in `config.rs`. Today's blanket `_ => true` (`config.rs:739`) therefore returns the same answers. | The value of the change is structural, and the plan says so rather than implying a behaviour change: `can_read` becomes an **exhaustive match with no wildcard**, per engine and per JDBC dialect, so a new kind or dialect cannot be added without a decision, and the matrix is pinned by a table-driven test that lists every (kind, dialect, engine) triple. The only refusal that changes behaviour today stays `generic`'s `engines` set. Task 2. |
| 4 | "Tests to flip: `attaching_to_a_starrocks_engine_is_refused`, `legacy_add_refused_by_engine_type_leaves_the_orphan_byte_identical`" | **There are three.** `nothing_is_attached_to_a_starrocks_engine_yet` (`tests/routes.rs:6599`) asserts a 400 whose body contains `"StarRocks"` on the *registry* attach route. | All three are rewritten in Task 4, plus the two `catalog_apply` doc comments and `STARROCKS_NOT_YET` itself. A grep for `STARROCKS_NOT_YET`, `"not supported yet"` and `not_yet` must come back empty at the end of Task 4. |
| 5 | D1 "attach … returns 201 with that status rather than 502" | **Also changes the legacy route's contract.** `engines.rs::add_catalog` funnels into the same `attach_to_engine`, and today returns 400 when the apply is rejected (`CatalogOpError::into_legacy`). Under D1 a StarRocks attach no longer *fails*, so the legacy add returns 201 with an attachment whose `apply_status` may be `failed`. | Accepted and documented: `attach_to_engine` returns `Ok(())` for StarRocks whenever the row was written, whatever the FE said; the DTO both routes return carries `applyStatus` / `applyError` so the caller can see it. The Trino path is untouched (still 502/400 with nothing persisted). Task 4, Task 6. |
| 6 | D6 "the catalog DDL also starts using the engine's pinned FE certificate" | **True, and it is worse than stated.** The root-login probe at `engines.rs:4162` (`SELECT 1`) also passes `tls_cert_pem = None`, and it presents the FE **root password**. The cert itself is only minted when delegation is configured *and* `tlsEnabled` (`engines.rs:3083`), and is stored as `engines.coordinator_cert`. | `FeTarget::for_engine` resolves `{host, port, root_password, tls_cert_pem}` once from the `Engine` row (`coordinator_host()`, `coordinator_port()`, `root_secret_id`, `coordinator_cert` gated on the stored `starrocksConfig.delegation.tlsEnabled`), and **every** statement the bootstrap and the apply path run — probe, catalog DDL, delegation DDL — goes through it. No call site passes `None` by hand any more. Task 3. |
| 7 | D8 "the DDL runner becomes injectable … SP3b replaces that with real tests" | The grep test is `spawn_starrocks_catalog_wire_keeps_all_six_guard_checkpoints` (`engines.rs:8664`). The behavioural test above it (`bootstrap_stops_before_any_provisioner_call_when_engine_already_gone`, `engines.rs:8611`) drives checkpoint 1 through `CountingStatusProvisioner` and is still correct. | The grep test is **deleted**; the first-checkpoint test stays and is joined by five more, each driving the spawned task with a `FakeStarRocksDdl` and a provisioner whose `status` reports a ready FE immediately (so the 10 s polls never sleep). Task 5. |
| 8 | D2 "writing `apply_status` per attachment" | `RegistryStore` can only mark an attachment **failed** per pair (`set_apply_failed`); the success path is engine-wide and Trino-shaped (`mark_engine_applied(engine_id, read_at)` compares `catalogs.updated_at`). | One new store method, `set_apply_applied(engine_id, catalog_id)`, in Task 3. `mark_engine_applied` is left alone — Trino still uses it. |
| 9 | D2 "loops over the engine's attachments in name order" | `list_engine_attachments` orders primary-first, then `created_at`, then name — deliberately, because Trino's env-var numbering depends on it. | The StarRocks converge re-sorts the returned vector by `catalog.name`; it does not change the store's order and so cannot disturb Trino rendering. |
| 10 | "`spawn_starrocks_catalog_wire` … becomes reachable from the Retry route" | The function takes twelve positional arguments including the warehouse name and one catalog's OAuth2 credential, and is spawned from `create_starrocks_engine` (`engines.rs:3709`) and from `update_starrocks_engine` only when the delegation block changed (`engines.rs:4797`). | It takes `EngineMgmtState` (which is `Clone` and `'static`) plus the engine id/name/namespace and the optional delegation bundle, and reads its catalogs from the registry itself. `sr_primary_catalog_credential` (`engines.rs:4844`) then has no caller and is **deleted** — converge resolves each attachment's credential itself. |
| 11 | "the catalog DDL" is `starrocks_external_catalog_sql` (`engines.rs:3767`) | Its knowledge is load-bearing and live-proven: StarRocks ignores the `iceberg.catalog.oauth2.*` dot-form, so the egret warehouse catalog must use the Iceberg-standard `iceberg.catalog.credential` + `iceberg.catalog.oauth2-server-uri` names (verified live on 3.3, StarRocks issue #57766). | The function is deleted and its behaviour — with that comment verbatim — becomes the `IcebergMetastore::Egret` arm of `render_starrocks`. Its two unit tests (`engines.rs:9613`, `engines.rs:9655`) move to `render/starrocks.rs` and keep asserting the same strings, so the migration is falsifiable. `sr_escape_dq` / `sr_escape_ident` move into the registry crate as `pub fn escape_dq` / `escape_ident` and are imported back by `engines.rs`'s delegation DDL, which keeps its own escaping tests. |
| 12 | D3 "present and matching what the registry would render → treat as applied" | `SHOW CREATE CATALOG` returns the statement text with **credential-bearing properties masked**, so a rendered secret can never be compared against the FE. | The comparison is over the non-secret properties only: `Drift::Matching` means every non-secret property egret would set is present with the same value; a masked or secret-valued key is skipped and named in the outcome. A rotated password is therefore invisible to drift detection — which is exactly why re-apply always runs `ALTER CATALOG … SET` instead of trusting a match (D3's own second half). Task 3, Task 4. |
| 13 | "the `egret` catalog today's Warehouse UI tells operators to create by hand" | Real: `CREATE EXTERNAL CATALOG IF NOT EXISTS` (`engines.rs:3846` call at `4199`) silently adopts whatever is there. | `IF NOT EXISTS` is dropped. Create is preceded by `SHOW CATALOGS`; `Drift::Different` refuses with the differing keys named and leaves the FE untouched, and the attachment carries that refusal as `apply_error`. Task 3, Task 4. |
| 14 | §4.5 "`in_place` for StarRocks" in the impact route | **Already correct.** `catalogs.rs:455-481` returns `in_place` for `engine_type == "starrocks"` and `restart` otherwise. | Task 6 changes nothing there and says so; only the attach/Retry response descriptions move. |
| 15 | D1 "the attachment row … and returns 201 with that status" — what status does a row have between being written and being converged? | **The column cannot say it.** `apply_status` is `CHECK (apply_status IN ('applied','failed'))` (`0147_catalog_registry.sql:54`) with DEFAULT `'applied'`, so a row written before the DDL runs reads **`applied`** — and already does today: `create_starrocks_engine` writes the primary attachment with `applied_now: false` (`engines.rs:3608`), which leaves `applied_at` NULL but `apply_status = 'applied'` while the FE has not heard of the catalog for the next 5–10 minutes. | Migration `0149` widens the CHECK to `('applied','failed','pending')`, `NewAttachment` gains an explicit `apply_status` (the existing `applied_now` keeps owning `applied_at`, so no Trino or backfill row changes), and `pending` is what a StarRocks attach and a StarRocks engine create write. `pending` means "egret has recorded this and has not yet been able to tell the FE" — the state D1 and D2 both need and neither could express. Task 3. |



## File Structure

**`crates/egret-catalog-registry`** — pure, engine-free.

| File | Change |
|---|---|
| `src/render/mod.rs` | `pub mod starrocks;` plus the three types now shared by both renderers: `RenderError`, `StorageRender`, `WarehouseRender` (moved out of `render/trino.rs`) |
| `src/render/starrocks.rs` | **new** — `render_starrocks`, `StarRocksCatalogDdl`, `PropValue`, `escape_dq`, `escape_ident`, `JdbcDriver`, `JdbcDrivers`, `DEFAULT_SR_JDBC_DRIVERS`, `SR_OVERRIDABLE`, `scrub_ddl` |
| `src/render/trino.rs` | imports the three moved types from `super`; otherwise untouched |
| `src/config.rs` | `can_read` becomes an exhaustive per-engine, per-dialect matrix; `validate` gains the StarRocks probe and the incomplete-override refusal; `starrocks_catalog_type()` next to `trino_connector_name()` |
| `src/lib.rs` | re-exports for everything above |
| `src/store.rs` | `RegistryStore::set_apply_applied(engine_id, catalog_id)`; `NewAttachment::apply_status`; `apply_status::PENDING` |

**`crates/egret-engine-mgmt`**

| File | Change |
|---|---|
| `src/starrocks_ddl.rs` | **new**, and `pub` (the engine-gated integration test is an external crate) — `StarRocksDdl`, `MysqlDdlRunner`, `FeTarget`, `show_catalogs`, `show_create_catalog`, `parse_show_create_properties`, `Drift`, `compare_to_fe`, `FE_UNCOMPARABLE_KEYS`; `fe_target` stays `pub(crate)` (it takes `EngineMgmtState`) |
| `src/state.rs` | `EngineMgmtState` gains `starrocks_ddl: Arc<dyn StarRocksDdl>` and `sr_jdbc_drivers: JdbcDrivers` (a plain `Clone` value, like `engine_egret_url`) |
| `src/catalog_apply.rs` | `apply_starrocks` implemented; `STARROCKS_NOT_YET` deleted; `check_engine_supports` loses its StarRocks arm; `render_starrocks_attached`; attach's compensating drop (D7) |
| `src/engines.rs` | `spawn_starrocks_catalog_wire` → converge-every-attachment over `EngineMgmtState`; `converge_starrocks_catalogs`; `starrocks_external_catalog_sql`, `sr_escape_dq`, `sr_escape_ident`, `sr_primary_catalog_credential` and the grep test deleted; `run_starrocks_ddl` moves behind the trait; both call sites updated |
| `src/catalogs.rs` | attach/Retry OpenAPI wording, impact `in_place` |
| `src/lib.rs`, `Cargo.toml` | module wiring |
| `tests/routes.rs` | three tests flipped, new StarRocks attach/detach/re-apply/drift coverage, `FakeStarRocksDdl` |
| `tests/starrocks_ddl_live.rs` | **new**, engine-gated (`EGRET_TEST_STARROCKS_URL`) |

**Migration:** `migrations/0149_attachment_apply_pending.sql`.

**Elsewhere:** `crates/egret-bin/src/main.rs` + its config module (the `[catalogs.starrocks_jdbc_drivers]` override table and the two new `EngineMgmtState` fields), `justfile` (`test-engine-mgmt-starrocks`), `openapi.json`, `frontend/src/api/generated/**`.



---
### Task 1: StarRocks rendering

**Files:**
- Create: `crates/egret-catalog-registry/src/render/starrocks.rs`
- Modify: `crates/egret-catalog-registry/src/render/mod.rs`, `src/render/trino.rs` (the three shared types move out; two error messages stop naming Trino), `src/config.rs` (`starrocks_catalog_type`, the StarRocks probe in `validate`), `src/lib.rs`

**Interfaces:**
- Consumes: `CatalogDef`, `CatalogConfig`, `EngineType`, `IcebergConfig`, `IcebergMetastore`, `RestAuth`, `HiveFamilyConfig`, `HiveMetastore`, `JdbcConfig`, `JdbcDialect`, `JdbcSsl`, `ElasticsearchConfig`, `SampleConfig`, `GenericConfig`, `jdbc_url`, `is_sensitive_key`.
- Produces (re-exported from the crate root):
  - `render::{RenderError, StorageRender, WarehouseRender}` (moved; `RenderError` gains `NoJdbcDriver(JdbcDialect)`, `ReservedName(String)`, `MissingSecret(Uuid)` and loses the word "Trino" from two messages).
  - `render::starrocks::{render_starrocks, StarRocksRenderInput, StarRocksCatalogDdl, PropValue, escape_dq, escape_ident, is_reserved_starrocks_name, scrub_secrets, starrocks_jdbc_uri, JdbcDriver, JdbcDrivers, DEFAULT_SR_JDBC_DRIVERS, SR_OVERRIDABLE}`.
  - `render_starrocks(name: &str, def: &CatalogDef, input: &StarRocksRenderInput<'_>) -> Result<StarRocksCatalogDdl, RenderError>`.
  - `StarRocksCatalogDdl { pub name: String, pub properties: BTreeMap<String, PropValue> }` with `create_sql(&self, secrets: &BTreeMap<Uuid, String>) -> Result<String, RenderError>`, `alter_sql(..)`, `drop_sql(&self) -> String`, `redacted(&self) -> String`, `secret_ids(&self) -> BTreeSet<Uuid>`, `comparable(&self) -> BTreeMap<String, String>`.
  - `CatalogDef::starrocks_catalog_type(&self) -> Option<String>`.

**The property names are not guesses.** Every key below was read out of the vendored StarRocks FE on 2026-09-21; the citations belong in the module doc so the next reader can re-check them:

| What | Key(s) | Source |
|---|---|---|
| Catalog type | `type` ∈ `iceberg, hive, deltalake, hudi, jdbc, es, benchmark, …` | `fe/fe-core/.../connector/ConnectorType.java:39-67` |
| Iceberg sub-type | `iceberg.catalog.type` ∈ `rest, hive, glue, hadoop, jdbc` | `IcebergCatalogProperties.java:27`, `IcebergCatalogType.java:18-36` |
| Iceberg REST pass-through | every `iceberg.catalog.<k>` is stripped to `<k>` and handed to the Iceberg `RESTSessionCatalog` | `IcebergRESTCatalog.java:107-113` |
| Hive-family metastore | `hive.metastore.type` (`hive`\|`glue`\|`dlf`, default `hive`), `hive.metastore.uris` (**plural**, unlike Trino's `hive.metastore.uri`) | `HiveConnector.java:31-34`, `HiveConnectorInternalMgr.java:42,88-97`, `DeltaLakeConnector.java:34`, `HudiConnector.java:33-34` |
| Glue | `aws.glue.region`, `aws.glue.use_instance_profile`, `aws.glue.access_key`, `aws.glue.secret_key` | `CloudConfigurationConstants.java:66-79`. The `aws.hive.metastore.glue.*` keys in `CREATE_EXTERNAL_CATALOG.md` are stale — they appear nowhere in the FE as credentials. |
| S3 | `aws.s3.endpoint`, `aws.s3.region`, `aws.s3.access_key`, `aws.s3.secret_key`, `aws.s3.enable_path_style_access`, `aws.s3.enable_ssl`, `aws.s3.use_instance_profile` | `CloudConfigurationConstants.java:29-64`, parsed in `AwsCloudConfigurationProvider.java:77-109` |
| JDBC | `jdbc_uri`, `user`, `password`, `driver_url`, `driver_class`, `schema_resolver` — **never `checksum`**, which the FE computes itself from `driver_url` | `JDBCResource.java:49-57`, `JDBCConnector.java:46-56,74-100` |
| Elasticsearch | `hosts` (comma-separated URLs), `user`, `password`, `es.net.ssl` | `EsConfig.java:30-58`, `EsTable.java:69-86` |
| Benchmark | `scale` only (a double > 0, default `1`); **tpch/tpcds/ssb is the database you `USE`, not a property** | `BenchmarkConfig.java:21-24`, `BenchmarkCatalogConfig.java:31-47`, `BenchmarkSuiteFactory.java:27-44` |
| Grammar | keys *and* values are quoted strings; `PROPERTIES(...)` is mandatory on `CREATE`; `ALTER CATALOG <n> SET (...)` — **not** `SET PROPERTIES` | `StarRocks.g4:866-883,1140-1142,3135-3152` |
| `ALTER` | only `type` is refused, by key and regardless of value; properties merge and cannot be unset; an unknown catalog is a **silent no-op** | `CatalogAnalyzer.java:39-41,113-126`, `CatalogMgr.java:180-240` |
| Names | `^[a-zA-Z]\w{0,63}$` (64 max); `default_catalog` and the `resource_mapping_inside_catalog_` prefix are reserved | `FeNameFormat.java:41,177-195`, `CatalogAnalyzer.java:61-65,89-91` |

Egret's own name rule (`^[a-z][a-z0-9_]{0,62}$`) is a strict subset of the FE's, so no registry name can be rejected for its shape — only `default_catalog`, which egret's rule allows and StarRocks refuses.

- [ ] **Step 0: Branch**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git fetch origin
git switch -c feat/starrocks-catalog-registry-sp3b origin/main
git branch --show-current
```
Expected: `feat/starrocks-catalog-registry-sp3b`.

- [ ] **Step 1: Move the shared render types** — `src/render/mod.rs` becomes:

```rust
//! Per-engine rendering (spec §4.3). SP3a shipped Trino; SP3b adds StarRocks
//! next to it, and SP5 egret's own connection. The input types and the error
//! are shared: both renderers read the same resolved warehouse and storage.

use uuid::Uuid;

pub mod starrocks;
pub mod trino;

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("catalog {0} cannot be read by this engine")]
    NotReadable(String),
    #[error("missing render input: {0}")]
    MissingInput(&'static str),
    #[error("advanced key {0:?} collides with a property the catalog form sets")]
    Collision(String),
    #[error("property {0:?} has a value with a line break")]
    LineBreak(String),
    /// StarRocks only: no pinned driver for this dialect and no override.
    #[error("no JDBC driver is configured for {0:?}")]
    NoJdbcDriver(crate::config::JdbcDialect),
    /// StarRocks only: a name the FE reserves.
    #[error("{0} is a name StarRocks reserves")]
    ReservedName(String),
    /// StarRocks only: the DDL needs the plaintext and the caller did not resolve it.
    #[error("secret {0} was not resolved")]
    MissingSecret(Uuid),
}
```

Delete those three items from `render/trino.rs` and add `use super::{RenderError, StorageRender, WarehouseRender};` there. In `src/lib.rs` replace the render re-export block with:

```rust
pub use render::starrocks::{
    escape_dq, escape_ident, is_reserved_starrocks_name, render_starrocks, scrub_secrets,
    starrocks_jdbc_uri, JdbcDriver, JdbcDrivers, PropValue, StarRocksCatalogDdl,
    StarRocksRenderInput, DEFAULT_SR_JDBC_DRIVERS,
};
pub use render::trino::{jdbc_url, render_trino, TrinoCatalogFile, TrinoRenderInput};
pub use render::{RenderError, StorageRender, WarehouseRender};
```

`cargo check -p egret-catalog-registry` must pass before Step 2. Two assertions in `render/trino.rs`'s `refusals` test compare `RenderError` **variants**, not their text, so the message change does not touch them — if one does compare text, update it to the new wording rather than restoring the old.

- [ ] **Step 2: Write the failing render tests** — create `src/render/starrocks.rs` with the types, `render_starrocks`/`starrocks_jdbc_uri` bodies `todo!()`, and this test module. Every expected string below is the real DDL, not a sketch:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CatalogDef, ValidationError};
    use serde_json::json;

    const PW: &str = "11111111-1111-1111-1111-111111111111";
    const SC: &str = "22222222-2222-2222-2222-222222222222";
    const WH: &str = "33333333-3333-3333-3333-333333333333";
    const S3: &str = "44444444-4444-4444-4444-444444444444";
    const CRED: &str = "55555555-5555-5555-5555-555555555555";

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn input() -> StarRocksRenderInput<'static> {
        StarRocksRenderInput {
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
            drivers: &DEFAULT_SR_JDBC_DRIVERS,
        }
    }

    fn def(kind: &str, config: serde_json::Value, advanced: serde_json::Value) -> CatalogDef {
        CatalogDef::parse(kind, &config, &advanced).unwrap()
    }

    fn plain(pairs: &[(&str, &str)]) -> BTreeMap<String, PropValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), PropValue::Plain(v.to_string())))
            .collect()
    }

    fn secrets() -> BTreeMap<Uuid, String> {
        [
            (id(CRED), "cid:csecret".to_string()),
            (id(PW), "p4ssw0rd".to_string()),
            (id(S3), "s3kr3t".to_string()),
        ]
        .into_iter()
        .collect()
    }

    /// The egret warehouse catalog must render EXACTLY what
    /// `engines.rs::starrocks_external_catalog_sql` produced before SP3b, so an
    /// upgraded engine's FE sees no drift. OAuth2 uses the Iceberg-STANDARD
    /// property names (`credential`, `oauth2-server-uri`) reached through the
    /// `iceberg.catalog.` pass-through, NOT StarRocks' own
    /// `iceberg.catalog.oauth2.*` form: verified live on 3.3, where the dot-form
    /// is silently ignored and catalog reads fail with "missing bearer token"
    /// (StarRocks issue #57766).
    #[test]
    fn an_egret_warehouse_renders_the_ddl_the_bootstrap_used_to_build() {
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        assert_eq!(
            ddl.properties,
            [
                ("type".to_string(), PropValue::Plain("iceberg".into())),
                ("iceberg.catalog.type".to_string(), PropValue::Plain("rest".into())),
                ("iceberg.catalog.uri".to_string(), PropValue::Plain("http://egret:8080/catalog".into())),
                ("iceberg.catalog.warehouse".to_string(), PropValue::Plain("wh-a".into())),
                ("iceberg.catalog.credential".to_string(), PropValue::Secret(id(CRED))),
                ("iceberg.catalog.oauth2-server-uri".to_string(), PropValue::Plain("http://egret:8080/oauth/token".into())),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        );
        let sql = ddl.create_sql(&secrets()).unwrap();
        assert!(sql.starts_with("CREATE EXTERNAL CATALOG `egret` PROPERTIES ("), "sql={sql}");
        assert!(sql.contains(r#""iceberg.catalog.credential" = "cid:csecret""#), "sql={sql}");
        assert!(sql.contains(r#""iceberg.catalog.oauth2-server-uri" = "http://egret:8080/oauth/token""#), "sql={sql}");
        assert!(!sql.contains("IF NOT EXISTS"), "D3: create is preceded by a read, never adopted blindly");
        assert!(!sql.contains("iceberg.catalog.token"), "no static bearer");
        // D6: the redacted form is the only one that may be logged.
        let red = ddl.redacted();
        assert!(red.contains(r#""iceberg.catalog.credential" = "******""#), "red={red}");
        assert!(!red.contains("csecret"), "red={red}");
    }

    #[test]
    fn alter_omits_type_and_drop_is_if_exists() {
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        let alter = ddl.alter_sql(&secrets()).unwrap();
        assert!(alter.starts_with("ALTER CATALOG `egret` SET ("), "alter={alter}");
        // CatalogAnalyzer refuses `type` by key, even unchanged.
        assert!(!alter.contains(r#""type" ="#), "alter={alter}");
        assert!(alter.contains(r#""iceberg.catalog.credential" = "cid:csecret""#), "alter={alter}");
        assert_eq!(ddl.drop_sql(), "DROP CATALOG IF EXISTS `egret`");
    }

    #[test]
    fn iceberg_rest_hive_and_glue_with_s3() {
        let d = def("iceberg", json!({
            "metastore": { "type": "rest", "uri": "https://cat/api", "warehouse": "lake",
                           "auth": { "type": "oauth2", "credential_secret_id": PW, "scope": "ALL", "token_uri": "https://idp/token" } },
            "storage_connection_id": SC
        }), json!(null));
        let p = render_starrocks("lake", &d, &input()).unwrap().properties;
        assert_eq!(p["type"], PropValue::Plain("iceberg".into()));
        assert_eq!(p["iceberg.catalog.type"], PropValue::Plain("rest".into()));
        assert_eq!(p["iceberg.catalog.uri"], PropValue::Plain("https://cat/api".into()));
        assert_eq!(p["iceberg.catalog.warehouse"], PropValue::Plain("lake".into()));
        assert_eq!(p["iceberg.catalog.credential"], PropValue::Secret(id(PW)));
        assert_eq!(p["iceberg.catalog.scope"], PropValue::Plain("ALL".into()));
        assert_eq!(p["iceberg.catalog.oauth2-server-uri"], PropValue::Plain("https://idp/token".into()));
        // The S3 block every non-egret lakehouse kind carries.
        assert_eq!(p["aws.s3.endpoint"], PropValue::Plain("http://minio:9000".into()));
        assert_eq!(p["aws.s3.region"], PropValue::Plain("eu-west-1".into()));
        assert_eq!(p["aws.s3.enable_path_style_access"], PropValue::Plain("true".into()));
        assert_eq!(p["aws.s3.use_instance_profile"], PropValue::Plain("false".into()));
        assert_eq!(p["aws.s3.access_key"], PropValue::Plain("AKIA".into()));
        assert_eq!(p["aws.s3.secret_key"], PropValue::Secret(id(S3)));
        // http endpoint ⇒ TLS off; the FE has no default for this key.
        assert_eq!(p["aws.s3.enable_ssl"], PropValue::Plain("false".into()));

        let d = def("iceberg", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }), json!(null));
        let p = render_starrocks("h", &d, &input()).unwrap().properties;
        assert_eq!(p["iceberg.catalog.type"], PropValue::Plain("hive".into()));
        assert_eq!(p["hive.metastore.uris"], PropValue::Plain("thrift://hms:9083".into()));

        let d = def("iceberg", json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKG", "secret_key_secret_id": PW }, "storage_connection_id": SC }), json!(null));
        let p = render_starrocks("g", &d, &input()).unwrap().properties;
        assert_eq!(p["iceberg.catalog.type"], PropValue::Plain("glue".into()));
        assert_eq!(p["aws.glue.region"], PropValue::Plain("eu-west-1".into()));
        assert_eq!(p["aws.glue.use_instance_profile"], PropValue::Plain("false".into()));
        assert_eq!(p["aws.glue.access_key"], PropValue::Plain("AKG".into()));
        assert_eq!(p["aws.glue.secret_key"], PropValue::Secret(id(PW)));
    }

    /// StarRocks spells Delta Lake `deltalake`, with no underscore.
    #[test]
    fn hive_family_type_names() {
        for (kind, ty) in [("hive", "hive"), ("delta_lake", "deltalake"), ("hudi", "hudi")] {
            let d = def(kind, json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }), json!(null));
            let p = render_starrocks("x", &d, &input()).unwrap().properties;
            assert_eq!(p["type"], PropValue::Plain(ty.into()), "{kind}");
            assert_eq!(p["hive.metastore.type"], PropValue::Plain("hive".into()), "{kind}");
            assert_eq!(p["hive.metastore.uris"], PropValue::Plain("thrift://hms:9083".into()), "{kind}");
        }
        let d = def("hive", json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC }), json!(null));
        let p = render_starrocks("x", &d, &input()).unwrap().properties;
        assert_eq!(p["hive.metastore.type"], PropValue::Plain("glue".into()));
        assert_eq!(p["aws.glue.use_instance_profile"], PropValue::Plain("true".into()), "no key ⇒ instance profile");
    }

    /// One `jdbc` type for every dialect, with the pinned driver — and the URI
    /// in the vocabulary the driver the FE actually loads understands.
    #[test]
    fn jdbc_per_dialect_with_the_pinned_driver() {
        let cases = [
            ("postgresql", Some("sales"), "org.postgresql.Driver", "postgresql",
             "jdbc:postgresql://db:5432/sales?sslmode=require"),
            // The FE rewrites `jdbc:mysql` → `jdbc:mariadb` and loads
            // org.mariadb.jdbc.Driver whatever we send (GPL avoidance,
            // JDBCMetadata.java:121-128,187-194) — so egret sends the MariaDB
            // form itself, whose sslMode vocabulary is `disable|trust|verify-full`,
            // not MySQL's `DISABLED|REQUIRED|VERIFY_IDENTITY`.
            ("mysql", None, "org.mariadb.jdbc.Driver", "mysql",
             "jdbc:mariadb://db:5432?sslMode=trust"),
            ("mariadb", None, "org.mariadb.jdbc.Driver", "mysql",
             "jdbc:mariadb://db:5432?sslMode=trust"),
            ("oracle", Some("ORCL"), "oracle.jdbc.driver.OracleDriver", "oracle",
             "jdbc:oracle:thin:@tcps://db:5432/ORCL"),
            ("sqlserver", Some("sales"), "com.microsoft.sqlserver.jdbc.SQLServerDriver", "sqlserver",
             "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=true;trustServerCertificate=true"),
            ("clickhouse", Some("events"), "com.clickhouse.jdbc.ClickHouseDriver", "clickhouse",
             "jdbc:clickhouse://db:5432/events?ssl=true&sslmode=none"),
        ];
        for (dialect, database, class, resolver, uri) in cases {
            let mut cfg = json!({ "dialect": dialect, "host": "db", "port": 5432, "user": "u",
                                  "password_secret_id": PW, "ssl": { "mode": "require" } });
            if let Some(d) = database {
                cfg["database"] = json!(d);
            }
            let d = def("jdbc", cfg, json!(null));
            let p = render_starrocks("j", &d, &input()).unwrap().properties;
            assert_eq!(p["type"], PropValue::Plain("jdbc".into()), "{dialect}");
            assert_eq!(p["jdbc_uri"], PropValue::Plain(uri.into()), "{dialect}");
            assert_eq!(p["user"], PropValue::Plain("u".into()), "{dialect}");
            assert_eq!(p["password"], PropValue::Secret(id(PW)), "{dialect}");
            assert_eq!(p["driver_class"], PropValue::Plain(class.into()), "{dialect}");
            assert_eq!(p["schema_resolver"], PropValue::Plain(resolver.into()), "{dialect}");
            assert_eq!(
                p["driver_url"],
                PropValue::Plain(DEFAULT_SR_JDBC_DRIVERS.get(d.jdbc_dialect().unwrap()).unwrap().driver_url.clone()),
                "{dialect}"
            );
            // The FE computes `checksum` itself from `driver_url`; sending one
            // would show up as permanent drift (JDBCConnector.java:52-56).
            assert!(!p.contains_key("checksum"), "{dialect}");
        }
    }

    /// D5: the two overridable keys, and the one that is still a collision.
    #[test]
    fn the_driver_can_be_overridden_per_catalog_but_nothing_else_can() {
        let d = def("jdbc", json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
                    json!({ "starrocks": { "driver_url": "file:///opt/pg.jar", "driver_class": "org.postgresql.Driver" } }));
        let p = render_starrocks("j", &d, &input()).unwrap().properties;
        assert_eq!(p["driver_url"], PropValue::Plain("file:///opt/pg.jar".into()));
        assert_eq!(p["driver_class"], PropValue::Plain("org.postgresql.Driver".into()));

        // Anything else that the form sets is still refused — at parse time.
        let err = CatalogDef::parse(
            "jdbc",
            &json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                     "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            &json!({ "starrocks": { "jdbc_uri": "jdbc:postgresql://evil/x" } }),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ValidationError::Collision { engine: "starrocks".into(), key: "jdbc_uri".into(), kind: "jdbc".into() }
        );
    }

    #[test]
    fn elasticsearch_and_benchmark() {
        let d = def("elasticsearch", json!({ "hosts": ["es-a", "es-b"], "port": 9200, "tls": true,
                                             "user": "u", "password_secret_id": PW }), json!(null));
        let p = render_starrocks("e", &d, &input()).unwrap().properties;
        assert_eq!(p["type"], PropValue::Plain("es".into()));
        assert_eq!(p["hosts"], PropValue::Plain("https://es-a:9200,https://es-b:9200".into()));
        assert_eq!(p["es.net.ssl"], PropValue::Plain("true".into()));
        assert_eq!(p["user"], PropValue::Plain("u".into()));
        assert_eq!(p["password"], PropValue::Secret(id(PW)));

        // tpch and tpcds are both the one `benchmark` connector; the suite is
        // the DATABASE you query (BenchmarkSuiteFactory.java:27-44).
        for kind in ["tpch", "tpcds"] {
            let d = def(kind, json!({ "scale": 10 }), json!(null));
            let p = render_starrocks("b", &d, &input()).unwrap().properties;
            assert_eq!(p["type"], PropValue::Plain("benchmark".into()), "{kind}");
            assert_eq!(p["scale"], PropValue::Plain("10".into()), "{kind}");
        }
        let d = def("tpch", json!({}), json!(null));
        assert!(!render_starrocks("b", &d, &input()).unwrap().properties.contains_key("scale"));
    }

    #[test]
    fn generic_is_advanced_plus_its_secret_properties() {
        let d = def(
            "generic",
            json!({ "engines": ["starrocks"], "secret_properties": { "starrocks": { "paimon.password": PW } } }),
            json!({ "starrocks": { "type": "paimon", "paimon.catalog.type": "filesystem" } }),
        );
        let p = render_starrocks("pm", &d, &input()).unwrap().properties;
        assert_eq!(p["type"], PropValue::Plain("paimon".into()));
        assert_eq!(p["paimon.catalog.type"], PropValue::Plain("filesystem".into()));
        assert_eq!(p["paimon.password"], PropValue::Secret(id(PW)));

        // A Trino-only generic is not renderable here.
        let d = def("generic", json!({ "engines": ["trino"] }), json!({ "trino": { "connector.name": "memory" } }));
        assert_eq!(
            render_starrocks("m", &d, &input()).unwrap_err(),
            RenderError::NotReadable("m".into())
        );
    }

    /// The escaping tests that guarded `starrocks_external_catalog_sql`, moved
    /// verbatim: this DDL runs as `root` on the FE and its inputs (warehouse
    /// name, registry name) are not charset-validated everywhere upstream.
    #[test]
    fn values_and_identifiers_cannot_break_out() {
        let evil = r#"wh" ) ; DROP DATABASE x ; -- "#;
        assert_eq!(escape_dq(evil), r#"wh\" ) ; DROP DATABASE x ; -- "#);
        assert_eq!(escape_dq(r#"a\b"#), r#"a\\b"#);
        assert_eq!(escape_ident("cat`evil"), "cat``evil");
        let ddl = StarRocksCatalogDdl {
            name: "cat`evil".into(),
            properties: plain(&[("type", "iceberg"), ("iceberg.catalog.warehouse", evil)]),
        };
        let sql = ddl.create_sql(&secrets()).unwrap();
        assert!(sql.contains("`cat``evil`"), "sql={sql}");
        assert!(!sql.contains(r#"wh" ) ; DROP DATABASE x"#), "sql={sql}");
        assert_eq!(ddl.drop_sql(), "DROP CATALOG IF EXISTS `cat``evil`");
    }

    #[test]
    fn a_reserved_name_is_refused_and_a_missing_secret_is_an_error_not_an_empty_value() {
        let d = CatalogDef::egret_warehouse(id(WH));
        assert_eq!(
            render_starrocks("default_catalog", &d, &input()).unwrap_err(),
            RenderError::ReservedName("default_catalog".into())
        );
        assert!(is_reserved_starrocks_name("resource_mapping_inside_catalog_x"));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        assert_eq!(
            ddl.create_sql(&BTreeMap::new()).unwrap_err(),
            RenderError::MissingSecret(id(CRED))
        );
    }

    /// D6: `scrub_secrets` is what stands between an FE error and `apply_error`.
    #[test]
    fn an_fe_error_quoting_the_statement_is_scrubbed() {
        let err = "Getting analyzing error. Detail message: \
                   CREATE EXTERNAL CATALOG `x` PROPERTIES (\"password\" = \"p4ssw0rd\") is invalid";
        let out = scrub_secrets(err, &secrets());
        assert!(!out.contains("p4ssw0rd"), "out={out}");
        assert!(out.contains("******"), "out={out}");
        assert!(out.contains("Getting analyzing error"), "the diagnosis survives; out={out}");
    }
}
```

Run `cargo test -p egret-catalog-registry --lib render::starrocks` and confirm every test fails on `todo!()` before writing the implementation. A test that passes now is a test that asserts nothing.

- [ ] **Step 3: Implement `render_starrocks`** in the same file, above the tests:

```rust
//! StarRocks rendering: a catalog definition + resolved inputs → the property
//! map of a `CREATE EXTERNAL CATALOG`, plus the `ALTER`/`DROP` forms and a
//! redacted form for display and logs.
//!
//! Unlike Trino's, this output is NOT a file egret hands to Kubernetes: it is a
//! statement egret runs as `root` on a live FE, with secrets INLINE (StarRocks
//! has no `${ENV:…}`). Two consequences shape this module:
//!   - the rendered map keeps secrets as ids (`PropValue::Secret`) so the
//!     plaintext exists only inside `create_sql`/`alter_sql`, and
//!     [`StarRocksCatalogDdl::redacted`] is the only form that may be logged;
//!   - every interpolated value goes through [`escape_dq`] and every identifier
//!     through [`escape_ident`].
//!
//! Property names verified against the vendored FE on 2026-09-21 — see the
//! per-kind citations inline.
```

The body, arm by arm (all keys exactly as tested above):

1. `if is_reserved_starrocks_name(name) { return Err(RenderError::ReservedName(name.into())) }`, then `if !def.can_read(EngineType::StarRocks) { return Err(RenderError::NotReadable(name.into())) }`.
2. An `Out` accumulator mirroring Trino's, with `set(k, v)` and `secret(k, id)` writing `PropValue::Plain` / `PropValue::Secret`, plus `s3(input)` and `glue(region, access_key, secret)` helpers:

```rust
fn s3(&mut self, input: &StarRocksRenderInput<'_>) -> Result<(), RenderError> {
    let s = input.storage.ok_or(RenderError::MissingInput("storage connection"))?;
    if let Some(ep) = s.endpoint {
        self.set("aws.s3.endpoint", ep);
        // The FE has no default for this key; MinIO over http needs it off.
        self.set("aws.s3.enable_ssl", if ep.starts_with("https://") { "true" } else { "false" });
    }
    self.set("aws.s3.region", s.region);
    self.set("aws.s3.enable_path_style_access", bool_str(s.path_style));
    self.set("aws.s3.use_instance_profile", "false");
    self.set("aws.s3.access_key", s.access_key);
    self.secret("aws.s3.secret_key", s.secret_key_secret_id.ok_or(RenderError::MissingInput("storage secret"))?);
    Ok(())
}
```
3. `CatalogConfig::Iceberg` → `type=iceberg` then the metastore arm (`Egret` is the bootstrap's exact five keys, with the warehouse's name and the attachment credential, and **no** S3 block — the REST catalog vends credentials; `Rest`/`Hive`/`Glue` add `self.s3(input)?`).
4. `Hive | DeltaLake | Hudi` → `type = def.starrocks_catalog_type()`, `hive.metastore.type`, `hive.metastore.uris` or the glue block, then `s3`.
5. `Jdbc(c)` → `type=jdbc`, `jdbc_uri = starrocks_jdbc_uri(c)`, `user`, `secret("password", …)`, then the driver: take `driver_url`/`driver_class` from `def.advanced.starrocks` when present, else `input.drivers.get(c.dialect).ok_or(RenderError::NoJdbcDriver(c.dialect))?`; then `schema_resolver` = `c.dialect.starrocks_schema_resolver()` (`mariadb` → `"mysql"`; the FE has no `mariadb` resolver, `JDBCMetadata.java:168-185`).
6. `Elasticsearch(c)` → `type=es`, `hosts` joined, `es.net.ssl`, optional `user` + secret `password`.
7. `Tpch|Tpcds(s)` → `type=benchmark`, `scale` when `Some`.
8. `Generic(g)` → `advanced.starrocks` verbatim (it carries `type`), then `secret_properties.starrocks`.
9. Append `advanced.starrocks` for every non-generic kind, **skipping the two keys already consumed** (`SR_OVERRIDABLE`) and refusing any other key already present with `RenderError::Collision`.
10. Refuse a line break in any plain value (`RenderError::LineBreak`) — the statement is one line.

```rust
pub const SR_OVERRIDABLE: [&str; 2] = ["driver_url", "driver_class"];

/// StarRocks reserves `default_catalog` (`CatalogAnalyzer.java:61-65`) and the
/// resource-mapping prefix (`CatalogMgr.java:493`).
pub fn is_reserved_starrocks_name(name: &str) -> bool {
    name == "default_catalog" || name.starts_with("resource_mapping_inside_catalog_")
}

pub fn escape_dq(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn escape_ident(s: &str) -> String {
    s.replace('`', "``")
}
```

And the statement builders — note the `" = "` separator, which is what `SHOW CREATE CATALOG` prints (`PrintableMap`, `ShowExecutor.java:3079-3081`), so a statement egret builds and one the FE echoes read the same:

```rust
impl StarRocksCatalogDdl {
    fn body(&self, secrets: &BTreeMap<Uuid, String>, skip_type: bool) -> Result<String, RenderError> {
        let mut parts = Vec::new();
        for (k, v) in &self.properties {
            if skip_type && k == "type" {
                continue;
            }
            let value = match v {
                PropValue::Plain(s) => s.clone(),
                PropValue::Secret(id) => secrets
                    .get(id)
                    .cloned()
                    .ok_or(RenderError::MissingSecret(*id))?,
            };
            parts.push(format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(&value)));
        }
        Ok(parts.join(", "))
    }

    /// `CREATE` carries no `IF NOT EXISTS`: D3 decides absent/matching/drifted
    /// from `SHOW CATALOGS` first, and `IF NOT EXISTS` would silently adopt a
    /// catalog egret did not create.
    pub fn create_sql(&self, secrets: &BTreeMap<Uuid, String>) -> Result<String, RenderError> {
        Ok(format!(
            "CREATE EXTERNAL CATALOG `{}` PROPERTIES ({})",
            escape_ident(&self.name),
            self.body(secrets, false)?
        ))
    }

    /// `ALTER CATALOG … SET (…)` — not `SET PROPERTIES`, which is not in the
    /// grammar (StarRocks.g4:1140-1142). `type` is dropped: the analyzer
    /// refuses it by key even when the value is unchanged
    /// (`CatalogAnalyzer.java:39-41`). Properties merge; a key this render no
    /// longer emits stays on the FE, which is why a kind change is impossible
    /// by design (the registry forbids it) rather than by ALTER.
    pub fn alter_sql(&self, secrets: &BTreeMap<Uuid, String>) -> Result<String, RenderError> {
        Ok(format!(
            "ALTER CATALOG `{}` SET ({})",
            escape_ident(&self.name),
            self.body(secrets, true)?
        ))
    }

    /// `IF EXISTS` is in the grammar (StarRocks.g4:874-875), so a detach of a
    /// catalog the FE never had succeeds without parsing an error string (D7).
    pub fn drop_sql(&self) -> String {
        format!("DROP CATALOG IF EXISTS `{}`", escape_ident(&self.name))
    }

    /// The ONLY form that may be logged or shown (D6). Secret-valued
    /// properties become `******`, and so does any plain value under a
    /// sensitive key — `advanced` cannot hold one (validation refuses it), but
    /// a redaction that depends on that invariant holding elsewhere is a
    /// redaction waiting to fail.
    pub fn redacted(&self) -> String {
        let props: Vec<String> = self
            .properties
            .iter()
            .map(|(k, v)| {
                let shown = match v {
                    PropValue::Secret(_) => "******",
                    PropValue::Plain(s) if crate::sensitive::is_sensitive_key(k) => "******",
                    PropValue::Plain(s) => s.as_str(),
                };
                format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(shown))
            })
            .collect();
        format!(
            "CREATE EXTERNAL CATALOG `{}` PROPERTIES ({})",
            escape_ident(&self.name),
            props.join(", ")
        )
    }

    pub fn secret_ids(&self) -> BTreeSet<Uuid> {
        self.properties
            .values()
            .filter_map(|v| match v {
                PropValue::Secret(id) => Some(*id),
                PropValue::Plain(_) => None,
            })
            .collect()
    }

    /// The properties a drift comparison may use (Task 3): the plain ones only.
    /// A secret value can never be compared — `SHOW CREATE CATALOG` masks some
    /// of them and DELETES `password` outright (`CredentialUtil.java:88-91`).
    pub fn comparable(&self) -> BTreeMap<String, String> {
        self.properties
            .iter()
            .filter_map(|(k, v)| match v {
                PropValue::Plain(s) => Some((k.clone(), s.clone())),
                PropValue::Secret(_) => None,
            })
            .collect()
    }
}
```

- [ ] **Step 4: `starrocks_jdbc_uri`** — everything but `mysql` delegates to the existing `jdbc_url`; `mysql` renders the MariaDB form, because the FE rewrites the scheme and loads the MariaDB driver regardless of what egret sends:

```rust
/// The `jdbc_uri` a StarRocks JDBC catalog gets. Identical to Trino's URL for
/// every dialect EXCEPT MySQL: `JDBCMetadata.getDriverName`/`getJdbcUrl`
/// (JDBCMetadata.java:121-128, 187-194) force `org.mariadb.jdbc.Driver` and
/// rewrite `jdbc:mysql` → `jdbc:mariadb` "because of gpl protocol", so a URL
/// carrying Connector/J's `sslMode=VERIFY_IDENTITY` would reach a driver whose
/// vocabulary is `disable|trust|verify-full`. egret therefore sends the MariaDB
/// form for both dialects, and pins the MariaDB driver for both.
pub fn starrocks_jdbc_uri(c: &JdbcConfig) -> String {
    match c.dialect {
        JdbcDialect::Mysql => {
            let mut c = c.clone();
            c.dialect = JdbcDialect::Mariadb;
            jdbc_url(&c)
        }
        _ => jdbc_url(c),
    }
}
```

- [ ] **Step 5: Wire the StarRocks probe into `CatalogDef::validate`** (`src/config.rs`), keeping the existing Trino probe and its `_ => Ok(())` catch-all (so a `NotReadable` on either engine is not a validation error):

```rust
        use crate::render::starrocks::{render_starrocks, StarRocksRenderInput};
        use crate::render::RenderError;
        for (engine, result) in [
            ("trino", render_trino("probe", self, &TrinoRenderInput::probe()).map(|_| ())),
            ("starrocks", render_starrocks("probe", self, &StarRocksRenderInput::probe()).map(|_| ())),
        ] {
            match result {
                Err(RenderError::Collision(key)) => {
                    return Err(ValidationError::Collision {
                        engine: engine.into(),
                        key,
                        kind: self.kind().as_str().into(),
                    })
                }
                Err(RenderError::LineBreak(key)) => {
                    return Err(ValidationError::Config {
                        kind: self.kind().as_str().into(),
                        msg: format!("{key} contains a line break"),
                    })
                }
                _ => {}
            }
        }
        Ok(())
```

`StarRocksRenderInput::probe()` mirrors `TrinoRenderInput::probe()` and passes `drivers: &DEFAULT_SR_JDBC_DRIVERS`, so the probe never refuses a dialect for a missing driver (plan table #1).

- [ ] **Step 6: Add `CatalogDef::starrocks_catalog_type` and `jdbc_dialect`** next to `trino_connector_name` (`src/config.rs`):

```rust
    /// The StarRocks `"type"` property, or `None` for a catalog StarRocks
    /// cannot read. Note the spellings differ from Trino's connector names:
    /// `deltalake` (no underscore), `es`, `benchmark` for both TPC suites, and
    /// one `jdbc` for every dialect (spec §3).
    pub fn starrocks_catalog_type(&self) -> Option<String> {
        Some(match &self.config {
            CatalogConfig::Iceberg(_) => "iceberg".into(),
            CatalogConfig::Hive(_) => "hive".into(),
            CatalogConfig::DeltaLake(_) => "deltalake".into(),
            CatalogConfig::Hudi(_) => "hudi".into(),
            CatalogConfig::Jdbc(_) => "jdbc".into(),
            CatalogConfig::Elasticsearch(_) => "es".into(),
            CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) => "benchmark".into(),
            CatalogConfig::Generic(_) => self.advanced.starrocks.get("type")?.clone(),
        })
    }

    pub fn jdbc_dialect(&self) -> Option<JdbcDialect> {
        match &self.config {
            CatalogConfig::Jdbc(j) => Some(j.dialect),
            _ => None,
        }
    }
```

- [ ] **Step 7: Gate and commit**

```bash
export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target
cargo test -p egret-catalog-registry --lib
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry --all-targets -- -D warnings
git add -A && git commit -m "feat(catalog-registry): render StarRocks catalog DDL"
```
Expected: the whole `egret-catalog-registry` lib suite green, including the Trino tests untouched by the type move.

---
### Task 2: `can_read` as a real matrix, and the pinned JDBC drivers

**Files:**
- Modify: `crates/egret-catalog-registry/src/config.rs` (`can_read`, the override validation), `src/render/starrocks.rs` (`JdbcDriver`, `JdbcDrivers`, `DEFAULT_SR_JDBC_DRIVERS`), `crates/egret-config/src/lib.rs` (`[starrocks]` section), `crates/egret-engine-mgmt/src/state.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-engine-mgmt/tests/routes.rs` (`build_app`), `deploy/helm/egret/templates/configmap-env.yaml`

**Interfaces:**
- Consumes: Task 1's renderer.
- Produces: `JdbcDrivers` (a struct with one `JdbcDriver { driver_url: String, driver_class: String }` per dialect and `fn get(&self, JdbcDialect) -> Option<&JdbcDriver>`, `fn with_overrides(&self, &StarRocksSettings) -> JdbcDrivers`); `egret_config::StarRocksSettings { jdbc_drivers: JdbcDriverOverrides }` reachable as `EGRET_STARROCKS__JDBC_DRIVERS__<DIALECT>__DRIVER_URL`; `EngineMgmtState::sr_jdbc_drivers: JdbcDrivers`.

- [ ] **Step 1: The pinned table** — in `src/render/starrocks.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JdbcDriver {
    pub driver_url: String,
    pub driver_class: String,
}

/// One pinned driver per dialect. StarRocks makes the FE DOWNLOAD the jar named
/// by `driver_url` (`JDBCConnector.java:74-100`, which also MD5s it into
/// `checksum`), so the URL must be reachable from the FE pods — which is why
/// this table is configuration (`[starrocks.jdbc_drivers]`), not a constant: an
/// air-gapped platform repoints it at an internal mirror or a `file:///` path
/// baked into the image (D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JdbcDrivers {
    pub postgresql: JdbcDriver,
    pub mysql: JdbcDriver,
    pub mariadb: JdbcDriver,
    pub oracle: JdbcDriver,
    pub sqlserver: JdbcDriver,
    pub clickhouse: JdbcDriver,
}
```

with `impl Default for JdbcDrivers` returning the pinned table, `pub static DEFAULT_SR_JDBC_DRIVERS: LazyLock<JdbcDrivers> = LazyLock::new(JdbcDrivers::default)` (so `&DEFAULT_SR_JDBC_DRIVERS` derefs to `&JdbcDrivers` wherever an input wants one), and `get()` returning `None` when the entry's `driver_url` is empty (an operator's way of saying "not available here" — plan table #1). MySQL and MariaDB share the MariaDB Connector/J entry, at the version the FE itself bundles (`fe/pom.xml:1241-1246`: `mariadb-java-client 3.3.2`), because the FE loads `org.mariadb.jdbc.Driver` for both.

**Verify every URL before committing it** — a 404 is a catalog that fails to create on a live FE with a message about downloading the driver:

```bash
for u in \
  https://repo1.maven.org/maven2/org/postgresql/postgresql/42.7.3/postgresql-42.7.3.jar \
  https://repo1.maven.org/maven2/org/mariadb/jdbc/mariadb-java-client/3.3.2/mariadb-java-client-3.3.2.jar \
  https://repo1.maven.org/maven2/com/oracle/database/jdbc/ojdbc11/23.4.0.24.05/ojdbc11-23.4.0.24.05.jar \
  https://repo1.maven.org/maven2/com/microsoft/sqlserver/mssql-jdbc/12.6.1.jre11/mssql-jdbc-12.6.1.jre11.jar \
  https://repo1.maven.org/maven2/com/clickhouse/clickhouse-jdbc/0.6.0/clickhouse-jdbc-0.6.0-all.jar ; do
  printf '%s %s\n' "$(curl -sI -o /dev/null -w '%{http_code}' "$u")" "$u"
done
```
Expected: `200` for each. Record the output in the commit message. Any `404` is replaced with the nearest version that exists on Maven Central **and the replacement is recorded** — do not ship a URL this step did not see answer 200. Driver classes are fixed by the FE's own detection (`JDBCMetadata.java:144-160`): `org.postgresql.Driver`, `org.mariadb.jdbc.Driver` (×2), `oracle.jdbc.driver.OracleDriver`, `com.microsoft.sqlserver.jdbc.SQLServerDriver`, `com.clickhouse.jdbc.ClickHouseDriver`.

- [ ] **Step 2: `can_read` becomes exhaustive** (`src/config.rs`), replacing the `_ => true` wildcard:

```rust
    /// Whether an engine type can read this catalog (spec §3, design D4).
    ///
    /// Exhaustive on purpose: no `_` arm, so a new kind or a new JDBC dialect
    /// cannot be added without someone deciding, for both engines, whether it
    /// is readable. Today every typed kind is readable by both — StarRocks 4.x
    /// is the floor egret provisions (`DEFAULT_SR_FE_IMAGE`), and it reads all
    /// of them. An engine pinned to an older FE can still refuse one; that
    /// refusal arrives as the attachment's `apply_error`, not as a validation
    /// error, because egret cannot know an overridden image's version.
    pub fn can_read(&self, engine: EngineType) -> bool {
        match (&self.config, engine) {
            (CatalogConfig::Generic(g), e) => g.engines.contains(&e),
            (CatalogConfig::Iceberg(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Hive(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::DeltaLake(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Hudi(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Elasticsearch(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Jdbc(_), EngineType::Trino) => true,
            (CatalogConfig::Jdbc(j), EngineType::StarRocks) => match j.dialect {
                // v3.0; Oracle/SQL Server v3.2.9/3.3.1; ClickHouse v3.3.0
                // (docs/en/data_source/catalog/jdbc_catalog.md:5,15,21). All
                // below the 4.x floor.
                JdbcDialect::Postgresql
                | JdbcDialect::Mysql
                | JdbcDialect::Mariadb
                | JdbcDialect::Oracle
                | JdbcDialect::Sqlserver
                | JdbcDialect::Clickhouse => true,
            },
        }
    }
```

and a table-driven test that names every triple, so the matrix is a document rather than an implication:

```rust
    #[test]
    fn every_kind_and_dialect_has_a_stated_answer_for_both_engines() {
        let cases: &[(&str, serde_json::Value, bool, bool)] = &[
            ("iceberg", json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }), true, true),
            ("hive", json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }), true, true),
            ("delta_lake", json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }), true, true),
            ("hudi", json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }), true, true),
            ("elasticsearch", json!({ "hosts": ["e"], "port": 9200, "tls": false }), true, true),
            ("tpch", json!({}), true, true),
            ("tpcds", json!({}), true, true),
        ];
        for (kind, cfg, trino, sr) in cases {
            let d = parse(kind, cfg.clone()).unwrap();
            assert_eq!(d.can_read(EngineType::Trino), *trino, "{kind}/trino");
            assert_eq!(d.can_read(EngineType::StarRocks), *sr, "{kind}/starrocks");
        }
        for dialect in ["postgresql", "mysql", "mariadb", "oracle", "sqlserver", "clickhouse"] {
            let db = matches!(dialect, "postgresql" | "oracle" | "sqlserver").then_some("d");
            let d = parse("jdbc", jdbc(dialect, db)).unwrap();
            assert!(d.can_read(EngineType::Trino), "{dialect}/trino");
            assert!(d.can_read(EngineType::StarRocks), "{dialect}/starrocks");
        }
        // The only refusal that exists today.
        let d = parse_with_advanced("generic", json!({ "engines": ["trino"] }), json!({ "trino": { "connector.name": "memory" } })).unwrap();
        assert!(d.can_read(EngineType::Trino));
        assert!(!d.can_read(EngineType::StarRocks));
    }
```

- [ ] **Step 3: Refuse an incomplete per-catalog driver override** — in `validate_advanced` (`src/config.rs`), after the existing per-key checks:

```rust
    // D5: `driver_url`/`driver_class` are the only advanced keys that may
    // override what the typed JDBC form sets (plan table #2), so a half-written
    // override is refused here rather than silently mixing a pinned class with
    // an operator's jar.
    if matches!(def.config, CatalogConfig::Jdbc(_)) {
        let url = def.advanced.starrocks.get("driver_url");
        let class = def.advanced.starrocks.get("driver_class");
        if url.is_some() != class.is_some()
            || url.is_some_and(|v| v.trim().is_empty())
            || class.is_some_and(|v| v.trim().is_empty())
        {
            return Err(ValidationError::Config {
                kind: "jdbc".into(),
                msg: "advanced.starrocks.driver_url and driver_class go together, and neither may be empty".into(),
            });
        }
    }
```
with a test for each of the four shapes (both, neither, one, empty).

- [ ] **Step 4: The `[starrocks]` config section** — `crates/egret-config/src/lib.rs`, following the `[workflow]` pattern exactly (a `default_*` per field, a `default_starrocks()` for the section, a field on `Config`, an arm in `impl Default for Config`). It holds **overrides only**, so a deployment's partial table cannot blank the compiled-in defaults:

```rust
/// `[starrocks]` — deployment-level overrides for how egret talks to StarRocks
/// FEs. Today: the JDBC driver jars a `jdbc` catalog makes the FE download
/// (design D5). Every field is optional: `egret-catalog-registry` compiles in a
/// pinned driver per dialect, and this section only REPOINTS one — at an
/// internal mirror, or at a `file:///` path baked into the FE image — so a
/// partially-written table can never leave a dialect without a driver.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct StarRocksSettings {
    #[serde(default)]
    pub jdbc_drivers: JdbcDriverOverrides,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct JdbcDriverOverrides {
    #[serde(default)]
    pub postgresql: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub mysql: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub mariadb: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub oracle: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub sqlserver: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub clickhouse: Option<JdbcDriverOverride>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct JdbcDriverOverride {
    #[serde(default)]
    pub driver_url: Option<String>,
    #[serde(default)]
    pub driver_class: Option<String>,
}
```

`egret-config` does **not** depend on `egret-catalog-registry`: the mapping lives in `egret-bin` (`JdbcDrivers::default().with_overrides(&config.starrocks)`), which is also where every other config→state mapping lives. Add a `figment::Jail` test in the style of `the_replica_count_arrives_under_the_name_the_chart_writes`:

```rust
    #[test]
    fn a_driver_override_arrives_under_the_nested_env_name() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_STARROCKS__JDBC_DRIVERS__MYSQL__DRIVER_URL", "file:///opt/mariadb.jar");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.starrocks.jdbc_drivers.mysql.unwrap().driver_url.as_deref(),
                Some("file:///opt/mariadb.jar")
            );
            Ok(())
        });
    }
```
(`Env::split("__")` is recursive, so three levels work; this test is what proves it rather than assuming it.)

- [ ] **Step 5: Thread it through** — `EngineMgmtState` gains

```rust
    /// The JDBC drivers a StarRocks `jdbc` catalog makes the FE download
    /// (design D5): the compiled-in pinned table with this deployment's
    /// `[starrocks.jdbc_drivers]` overrides applied. Populated from config in
    /// egret-bin; tests use `JdbcDrivers::default()`.
    pub sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers,
```
Set it in the **two** construction sites (`crates/egret-bin/src/main.rs:1957`, `crates/egret-engine-mgmt/tests/routes.rs:57`) and in `engines.rs`'s `test_state()`. Add the chart line to `deploy/helm/egret/templates/configmap-env.yaml` only if the chart is meant to set one — it is not today, so this step adds **no** chart line and says so.

- [ ] **Step 6: Gate and commit**

```bash
cargo test -p egret-catalog-registry -p egret-config --lib
cargo fmt --all -- --check
cargo clippy -p egret-catalog-registry -p egret-config -p egret-engine-mgmt --all-targets -- -D warnings
git add -A && git commit -m "feat(catalog-registry): StarRocks can_read matrix and pinned JDBC drivers"
```



---
### Task 3: The injectable DDL runner, FE introspection, drift, and `pending`

**Files:**
- Create: `crates/egret-engine-mgmt/src/starrocks_ddl.rs`, `crates/egret-engine-mgmt/tests/starrocks_ddl_live.rs`, `migrations/0149_attachment_apply_pending.sql`
- Modify: `crates/egret-engine-mgmt/src/lib.rs`, `src/state.rs`, `src/engines.rs` (`run_starrocks_ddl` moves behind the trait; `sr_escape_dq`/`sr_escape_ident` come from the registry crate), `crates/egret-bin/src/main.rs`, `crates/egret-engine-mgmt/tests/routes.rs` (`build_app`), `crates/egret-catalog-registry/src/store.rs`, and the ten `NewAttachment { … }` sites, `justfile`

**Interfaces:**
- Consumes: Task 1's `StarRocksCatalogDdl`, `scrub_secrets`.
- Produces:
  - `starrocks_ddl::{StarRocksDdl, MysqlDdlRunner, FeTarget, fe_target, show_catalogs, show_create_catalog, parse_show_create_properties, compare_to_fe, Drift, FE_UNCOMPARABLE_KEYS}`.
  - `EngineMgmtState::starrocks_ddl: Arc<dyn StarRocksDdl>`.
  - `RegistryStore::set_apply_applied(&self, engine_id: Uuid, catalog_id: Uuid) -> Result<(), RegistryError>`; `NewAttachment::apply_status: &'static str`; `apply_status::PENDING`.

- [ ] **Step 1: Migration `0149_attachment_apply_pending.sql`**

```sql
-- SP3b D1/D2: a StarRocks attachment exists in egret before the FE has been
-- told about it — an attach records first and converges after (the FE of a
-- freshly provisioned engine is unreachable for 5-10 minutes), and engine
-- create has always written the primary attachment the same way. The CHECK
-- from 0147 allowed only 'applied' and 'failed', so that row read 'applied'
-- while nothing had been applied. 'pending' is that state, said out loud.
ALTER TABLE registry.engine_catalog_attachments
    DROP CONSTRAINT engine_catalog_attachments_apply_status_check;
ALTER TABLE registry.engine_catalog_attachments
    ADD CONSTRAINT engine_catalog_attachments_apply_status_check
    CHECK (apply_status IN ('applied', 'failed', 'pending'));
```

```bash
psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'
```

- [ ] **Step 2: `pending` in the store** — `crates/egret-catalog-registry/src/store.rs`:

```rust
pub mod apply_status {
    pub const APPLIED: &str = "applied";
    pub const FAILED: &str = "failed";
    /// Recorded by egret, not yet accepted by the engine (SP3b D1). Only a
    /// StarRocks attachment is ever written this way: a Trino apply happens
    /// before the row exists at all.
    pub const PENDING: &str = "pending";
}
```

`NewAttachment` gains `pub apply_status: &'static str` (the existing `applied_now` keeps owning `applied_at`, so no other row's behaviour changes), and `insert_attachment_row`'s statement becomes:

```rust
        "INSERT INTO registry.engine_catalog_attachments \
           (engine_id, catalog_id, catalog_client_id, credential_secret_id, minted_client, apply_status, applied_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7 THEN now() END, COALESCE($8, now())) RETURNING ",
```

Update all ten construction sites (`catalog_apply.rs:589`, `engines.rs:2879`, `engines.rs:3608`, `backfill.rs:608`, `backfill.rs:690`, `tests/routes.rs:3611,6063,6152`, `tests/store.rs:281,293,306,421`) to pass `apply_status: apply_status::APPLIED` — **every one of them**, including `engines.rs:3608`, which Task 5 is the one to switch to `PENDING`. Keeping this step behaviour-neutral is the point: if a test moves here, the change was not neutral.

Add next to `set_apply_failed`:

```rust
    /// One attachment converged (SP3b). `mark_engine_applied` stays as it is:
    /// it is Trino's whole-engine apply, which settles every attachment at once
    /// and compares `catalogs.updated_at`; a StarRocks apply is per catalog.
    pub async fn set_apply_applied(
        &self,
        engine_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<(), RegistryError> {
        sqlx::query(
            "UPDATE registry.engine_catalog_attachments \
                SET apply_status = 'applied', apply_error = NULL, applied_at = now() \
              WHERE engine_id = $1 AND catalog_id = $2",
        )
        .bind(engine_id)
        .bind(catalog_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
```

A `tests/store.rs` case writes `PENDING`, reads it back, calls `set_apply_applied` and asserts `applied`/`applied_at IS NOT NULL`/`apply_error IS NULL`.

- [ ] **Step 3: Write the failing introspection tests** — create `src/starrocks_ddl.rs` with the types and `todo!()` bodies plus:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `SHOW CREATE CATALOG` really returns: the statement text with
    /// `"k" = "v"` pairs, `,\n` between them, `\"` escaped inside values, and
    /// NO stable order (the FE prints a HashMap — `PrintableMap`,
    /// `ShowExecutor.java:3079-3081`). So egret parses the pairs; it never
    /// diffs the text.
    const SHOW_CREATE: &str = "CREATE EXTERNAL CATALOG `sales`\nPROPERTIES (\"type\" = \"jdbc\",\n\"user\" = \"u\",\n\"jdbc_uri\" = \"jdbc:postgresql://db:5432/sales?sslmode=require\",\n\"driver_class\" = \"org.postgresql.Driver\",\n\"checksum\" = \"2b1c…\",\n\"aws.s3.access_key\" = \"AK******LE\"\n)";

    #[test]
    fn properties_are_parsed_out_of_the_statement_the_fe_prints() {
        let p = parse_show_create_properties(SHOW_CREATE);
        assert_eq!(p["type"], "jdbc");
        assert_eq!(p["user"], "u");
        assert_eq!(p["jdbc_uri"], "jdbc:postgresql://db:5432/sales?sslmode=require");
        assert_eq!(p["checksum"], "2b1c…");
        assert_eq!(p.len(), 6);
        // An escaped quote inside a value does not end it.
        let p = parse_show_create_properties("PROPERTIES (\"a\" = \"x\\\"y\", \"b\" = \"z\")");
        assert_eq!(p["a"], "x\"y");
        assert_eq!(p["b"], "z");
    }

    #[test]
    fn drift_is_decided_on_the_comparable_properties_only() {
        let rendered = jdbc_ddl(); // the same catalog the constant above describes
        let fe = parse_show_create_properties(SHOW_CREATE);
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);

        // `checksum` is the FE's own (`JDBCConnector.java:74-100`) and must not
        // count as drift; a masked credential must not either.
        let mut fe2 = fe.clone();
        fe2.insert("checksum".into(), "different".into());
        fe2.insert("aws.s3.access_key".into(), "ZZ******QQ".into());
        assert_eq!(compare_to_fe(&rendered, &fe2), Drift::Matching);

        // A real difference names the key, and only that key.
        let mut fe3 = fe.clone();
        fe3.insert("user".into(), "someone_else".into());
        assert_eq!(compare_to_fe(&rendered, &fe3), Drift::Different(vec!["user".into()]));

        // A key egret sets that the FE does not have is drift too.
        let mut fe4 = fe.clone();
        fe4.remove("jdbc_uri");
        assert_eq!(compare_to_fe(&rendered, &fe4), Drift::Different(vec!["jdbc_uri".into()]));
    }

    /// `password` is not masked — it is DELETED from the output
    /// (`CredentialUtil.java:88,91`), and so are our own secret-valued keys by
    /// construction. A rotated credential is therefore INVISIBLE to drift
    /// detection, which is exactly why re-apply always ALTERs (D3).
    #[test]
    fn a_secret_valued_key_is_never_compared() {
        let rendered = jdbc_ddl();
        let mut fe = parse_show_create_properties(SHOW_CREATE);
        assert!(!fe.contains_key("password"));
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);
        fe.insert("password".into(), "whatever".into());
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);
    }
}
```

- [ ] **Step 4: Implement the module**

```rust
//! The channel egret's control plane uses to reach a StarRocks FE: one trait so
//! the whole apply path can be driven in a test without a MySQL port (design
//! D8), the real `mysql_async` runner, the FE coordinates for an engine row,
//! and the `SHOW CATALOGS` / `SHOW CREATE CATALOG` reads that decide whether a
//! catalog is absent, matching or drifted (D3).
//!
//! D6: no statement passed through here is ever logged, and no error returned
//! from here reaches a column or a response without `scrub_secrets`.

#[derive(Clone, Debug)]
pub struct FeTarget {
    pub host: String,
    pub port: u16,
    pub root_password: Option<String>,
    /// The engine's pinned FE certificate. `Some` ⇒ the connection runs over
    /// TLS with this cert as its only trust root and domain validation off
    /// (the cert's CN cannot match the in-cluster Service DNS egret dials) —
    /// the same posture `StarRocksConnector::new` and the delegation bootstrap
    /// already use, now applied to EVERY statement (D6).
    pub tls_cert_pem: Option<String>,
}

#[async_trait::async_trait]
pub trait StarRocksDdl: Send + Sync {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), String>;
    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, String>;
}

pub struct MysqlDdlRunner;
```

`MysqlDdlRunner::execute` is today's `run_starrocks_ddl` body verbatim (one statement per connection, `starrocks_root_conn_opts`, `prefer_socket(false)`); `query` is the same connection with

```rust
        let rows: Vec<mysql_async::Row> = conn.query(sql).await.map_err(|e| e.to_string())?;
        let out = rows
            .into_iter()
            .map(|row| {
                (0..row.columns_ref().len())
                    .map(|i| {
                        row.as_ref(i)
                            .cloned()
                            .and_then(|v| mysql_async::from_value_opt::<Option<String>>(v).ok())
                            .flatten()
                    })
                    .collect()
            })
            .collect();
```

`fe_target` resolves the four fields from the `Engine` row — and this is the whole of D6's cert fix:

```rust
/// Where and how to reach this engine's FE. The cert is the engine's own
/// (`engines.coordinator_cert`, minted by `ensure_tls_cert`), used whenever the
/// stored spec has `starrocksConfig.delegation.tlsEnabled` — the same condition
/// under which the FE actually serves TLS. Before SP3b the catalog DDL and the
/// root-login probe both passed `None` while the delegation DDL passed the
/// cert, so an OAuth2 credential AND the FE root password crossed the wire in
/// cleartext on a TLS-capable engine (D6).
pub(crate) async fn fe_target(st: &CpState, engine: &Engine) -> Result<FeTarget, CpError> {
    let root_password = match engine.root_secret_id {
        Some(sid) => Some(st.secrets.resolve_plaintext(None, sid).await.map_err(|e| {
            CpError::BadRequest(format!("resolve root password secret: {e}"))
        })?),
        None => None,
    };
    let tls = sr_delegation_block(&engine.spec)
        .get("tlsEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(FeTarget {
        host: engine.coordinator_host(),
        port: engine.coordinator_port() as u16,
        root_password,
        tls_cert_pem: tls.then(|| engine.coordinator_cert.clone()).flatten(),
    })
}
```

Introspection and drift:

```rust
/// `SHOW CATALOGS` → column 0 (`Catalog`; the three columns are
/// `Catalog, Type, Comment`, `ShowResultMetaFactory.java:1305-1311`).
pub async fn show_catalogs(ddl: &dyn StarRocksDdl, fe: &FeTarget) -> Result<BTreeSet<String>, String> {
    Ok(ddl
        .query(fe, "SHOW CATALOGS")
        .await?
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

/// `SHOW CREATE CATALOG <name>` → column 1 (`Create Catalog`), parsed into its
/// properties. `None` when the FE says the catalog is unknown
/// (`ERR_BAD_CATALOG_ERROR` 5078, text `Unknown catalog '<name>'`) — that is a
/// SemanticException, not an empty result, so it arrives as an Err string and
/// is translated here rather than at every call site.
pub async fn show_create_catalog(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
    name: &str,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let sql = format!("SHOW CREATE CATALOG `{}`", escape_ident(name));
    match ddl.query(fe, &sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().nth(1).flatten())
            .map(|text| parse_show_create_properties(&text))),
        Err(e) if e.contains("Unknown catalog") => Ok(None),
        Err(e) => Err(e),
    }
}

/// The properties out of that statement. The FE prints a `HashMap`, so the
/// ORDER is unstable and the text must never be diffed — only the pairs.
pub fn parse_show_create_properties(text: &str) -> BTreeMap<String, String> {
    static PAIR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PAIR.get_or_init(|| {
        // "key" = "value", with \" escapes inside either side.
        regex::Regex::new(r#""((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)""#)
            .expect("static property pattern compiles")
    });
    let body = match (text.find("PROPERTIES ("), text.rfind(')')) {
        (Some(start), Some(end)) if end > start => &text[start..=end],
        _ => return BTreeMap::new(),
    };
    re.captures_iter(body)
        .map(|c| (unescape_dq(&c[1]), unescape_dq(&c[2])))
        .collect()
}

/// The inverse of [`escape_dq`], for values read back off the FE.
fn unescape_dq(s: &str) -> String {
    s.replace("\\\"", "\"").replace("\\\\", "\\")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Drift {
    Absent,
    Matching,
    /// The keys egret would set that the FE disagrees about, sorted.
    Different(Vec<String>),
}

/// Keys that can never be compared, because the FE does not return what it
/// holds (`CredentialUtil.java:38-91`): credentials come back as
/// `xx******yy` (or `******` when ≤ 4 chars), `password` is deleted outright,
/// and `checksum` is the FE's own MD5 of `driver_url`.
pub const FE_UNCOMPARABLE_KEYS: [&str; 3] = ["password", "checksum", "azure_path_key"];

pub fn compare_to_fe(
    rendered: &StarRocksCatalogDdl,
    fe: &BTreeMap<String, String>,
) -> Drift {
    let mut differing: Vec<String> = rendered
        .comparable()          // Plain values only: secrets are never comparable
        .into_iter()
        .filter(|(k, _)| !FE_UNCOMPARABLE_KEYS.contains(&k.as_str()))
        .filter(|(k, v)| match fe.get(k) {
            None => true,
            Some(actual) => actual != v && !actual.contains("******"),
        })
        .map(|(k, _)| k)
        .collect();
    differing.sort();
    if differing.is_empty() { Drift::Matching } else { Drift::Different(differing) }
}
```

`Drift::Absent` is produced by the caller from `show_catalogs`, not here — a name the FE does not list is absent whatever `SHOW CREATE CATALOG` would say.

- [ ] **Step 5: Replace `run_starrocks_ddl`'s call sites** — `engines.rs` keeps `starrocks_root_conn_opts` (now used by `MysqlDdlRunner`) and its unit tests, deletes `run_starrocks_ddl`, and deletes `sr_escape_dq`/`sr_escape_ident` in favour of `egret_catalog_registry::{escape_dq, escape_ident}` (the JWT/security-integration statements keep their own escaping tests, which now exercise the imported functions). Every bootstrap statement goes through `st.starrocks_ddl.execute(&fe, …)` with the `FeTarget` from `fe_target` — including the `SELECT 1` root-login probe, which carries the root password.

- [ ] **Step 6: Wire the port into state** — `EngineMgmtState`:

```rust
    /// Runs statements on a StarRocks FE. A real `MysqlDdlRunner` in egret-bin;
    /// a fake in tests, which is what makes the bootstrap and the apply path
    /// testable at all (design D8).
    pub starrocks_ddl: Arc<dyn crate::starrocks_ddl::StarRocksDdl>,
```
`egret-bin` passes `Arc::new(MysqlDdlRunner)`. Tests pass the fake defined here — it is the instrument Tasks 4 and 5 both run on, so it is written once, now, in `crates/egret-engine-mgmt/src/starrocks_ddl.rs` behind `#[cfg(any(test, feature = "test-support"))]`… except that `tests/routes.rs` is an external crate, so it lives in `src/starrocks_ddl.rs` as a plain `pub struct` with a doc comment saying why:

```rust
/// A StarRocks FE that lives in memory. Public because the route tests are an
/// external crate; it is the whole of design D8 — without it, every statement
/// egret builds could only be exercised against a real MySQL port, which is
/// why five of the bootstrap's six guard checkpoints used to be pinned by a
/// test that grepped the function's own source.
///
/// It imitates the FE behaviours the apply path actually depends on, and no
/// others: a second `CREATE` errors with `already exists`, `ALTER` MERGES and
/// cannot unset, `DROP … IF EXISTS` on an unknown catalog succeeds, and
/// `SHOW CREATE CATALOG` masks credentials and DELETES `password`, exactly as
/// `CredentialUtil` does. Imitating the masking is the point: a fake that
/// echoed back what was written would make the drift comparison look correct
/// while it silently reported permanent drift against a real FE.
#[derive(Clone, Default)]
pub struct FakeStarRocksDdl {
    executed: Arc<Mutex<Vec<String>>>,
    catalogs: Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>,
    /// When set, every `execute`/`query` fails with this message.
    fail: Arc<Mutex<Option<String>>>,
    /// `(needle, message)`: fail only the statements containing `needle`.
    selective_fail: Arc<Mutex<Vec<(String, String)>>>,
    /// The `FeTarget` of the most recent call, so a test can assert the cert.
    last_target: Arc<Mutex<Option<FeTarget>>>,
    #[allow(clippy::type_complexity)]
    on_create: Arc<Mutex<Option<Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, ()> + Send + Sync>>>>,
}

impl FakeStarRocksDdl {
    pub fn executed(&self) -> Vec<String> { self.executed.lock().unwrap().clone() }
    pub fn clear_executed(&self) { self.executed.lock().unwrap().clear() }
    /// Catalog names in the order `CREATE EXTERNAL CATALOG` ran for them.
    pub fn created(&self) -> Vec<String> {
        self.executed()
            .iter()
            .filter_map(|s| statement_target("CREATE EXTERNAL CATALOG", s))
            .collect()
    }
    /// Every call fails with this message until it is cleared.
    pub fn set_fail(&self, msg: Option<&str>) { *self.fail.lock().unwrap() = msg.map(str::to_string); }
    /// Fail only the statements naming this catalog — one bad catalog among
    /// several, which is what the bootstrap loop has to survive.
    pub fn fail_catalog(&self, name: &str, msg: &str) {
        self.fail_matching(&format!("`{}`", escape_ident(name)), msg)
    }
    /// Fail only the statements containing this substring (the `already
    /// exists` branch of the delegation bootstrap).
    pub fn fail_matching(&self, needle: &str, msg: &str) {
        self.selective_fail
            .lock()
            .unwrap()
            .push((needle.to_string(), msg.to_string()));
    }
    /// The `FeTarget` of the most recent call — how a test proves the pinned
    /// certificate really reaches every statement (D6).
    pub fn last_target(&self) -> FeTarget {
        self.last_target.lock().unwrap().clone().expect("a call was made")
    }
    /// Run this when a `CREATE EXTERNAL CATALOG` is executed, before it is
    /// applied — the only way to make something happen BETWEEN two iterations
    /// of the bootstrap's loop (deleting the engine, say).
    pub fn on_create(
        &self,
        hook: impl Fn(String) -> futures::future::BoxFuture<'static, ()> + Send + Sync + 'static,
    ) {
        *self.on_create.lock().unwrap() = Some(Arc::new(hook));
    }
    pub fn catalog(&self, name: &str) -> Option<BTreeMap<String, String>> {
        self.catalogs.lock().unwrap().get(name).cloned()
    }
    /// Seed a catalog egret did not create — a hand-made `egret`, or one left
    /// by a pre-SP3a engine. This is how the drift test builds its premise.
    pub fn seed(&self, name: &str, props: &[(&str, &str)]) {
        self.catalogs.lock().unwrap().insert(
            name.to_string(),
            props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        );
    }

    /// Remove a catalog WITHOUT recording a statement — an FE that lost it to
    /// someone else's `DROP`, which is what detach must tolerate (D7).
    pub fn forget(&self, name: &str) {
        self.catalogs.lock().unwrap().remove(name);
    }
}

#[async_trait::async_trait]
impl StarRocksDdl for FakeStarRocksDdl {
    async fn execute(&self, _fe: &FeTarget, sql: &str) -> Result<(), String> {
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(msg);
        }
        self.executed.lock().unwrap().push(sql.to_string());
        let mut cats = self.catalogs.lock().unwrap();
        if let Some(name) = statement_target("CREATE EXTERNAL CATALOG", sql) {
            if cats.contains_key(&name) {
                return Err(format!("Catalog '{name}' already exists"));
            }
            cats.insert(name, parse_show_create_properties(sql));
        } else if let Some(name) = statement_target("ALTER CATALOG", sql) {
            // Merge, never replace — CatalogMgr.java:212-219. An unknown
            // catalog is a SILENT no-op (CatalogMgr.java:184-187).
            if let Some(existing) = cats.get_mut(&name) {
                existing.extend(parse_show_create_properties(sql));
            }
        } else if let Some(name) = statement_target("DROP CATALOG IF EXISTS", sql) {
            cats.remove(&name);
        }
        Ok(())
    }

    async fn query(&self, _fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(msg);
        }
        if sql == "SELECT 1" {
            return Ok(vec![vec![Some("1".into())]]);
        }
        if sql == "SHOW CATALOGS" {
            return Ok(self
                .catalogs
                .lock()
                .unwrap()
                .keys()
                .map(|n| vec![Some(n.clone()), Some("External".into()), None])
                .collect());
        }
        if let Some(name) = statement_target("SHOW CREATE CATALOG", sql) {
            let Some(props) = self.catalogs.lock().unwrap().get(&name).cloned() else {
                return Err(format!("Unknown catalog '{name}'"));
            };
            return Ok(vec![vec![Some(name.clone()), Some(show_create_text(&name, &props))]]);
        }
        Ok(vec![])
    }
}

/// The backtick-quoted name out of a statement that starts with `prefix`,
/// with doubled backticks undone — the inverse of what [`escape_ident`] wrote.
fn statement_target(prefix: &str, sql: &str) -> Option<String> {
    let rest = sql.strip_prefix(prefix)?.trim_start();
    let rest = rest.strip_prefix('`')?;
    let mut name = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '`' if chars.clone().next() == Some('`') => {
                chars.next();
                name.push('`');
            }
            '`' => return Some(name),
            c => name.push(c),
        }
    }
    None
}

/// What the FE would print back: the same `"k" = "v"` shape, with
/// `CredentialUtil`'s masking applied — `password` and `azure_path_key`
/// DELETED (`CredentialUtil.java:88,91`), the listed credential keys masked as
/// first-two + `******` + last-two, or `******` when the value is ≤ 4 chars
/// (`CredentialUtil.java:86-103`).
fn show_create_text(name: &str, props: &BTreeMap<String, String>) -> String {
    const MASKED: [&str; 6] = [
        "aws.s3.access_key",
        "aws.s3.secret_key",
        "aws.glue.access_key",
        "aws.glue.secret_key",
        "iceberg.catalog.oauth2.credential",
        "iceberg.catalog.jdbc.password",
    ];
    let body: Vec<String> = props
        .iter()
        .filter(|(k, _)| k.as_str() != "password" && k.as_str() != "azure_path_key")
        .map(|(k, v)| {
            let shown = if MASKED.contains(&k.as_str()) {
                if v.chars().count() <= 4 {
                    "******".to_string()
                } else {
                    let c: Vec<char> = v.chars().collect();
                    format!("{}{}{}", c[0], c[1], "******")
                        + &c[c.len() - 2..].iter().collect::<String>()
                }
            } else {
                v.clone()
            };
            format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(&shown))
        })
        .collect();
    format!(
        "CREATE EXTERNAL CATALOG `{}`\nPROPERTIES ({}\n)",
        escape_ident(name),
        body.join(",\n")
    )
}
```

The route tests also gain one helper, because five of them need the same eight lines:

```rust
/// A provisioned StarRocks engine, through the real create route (which mints
/// the root password, the client and the primary attachment). Returns its id.
async fn create_starrocks_via_api(f: &Fixture, token: &str) -> String {
    let wh_id = seed_provisioned_warehouse_with_secret(f).await;
    let (status, j) = call(f, "POST", "/engines", token, Some(serde_json::json!({
        "name": format!("sr-{}", Uuid::new_v4().simple()), "warehouseId": wh_id, "kind": "starrocks",
        "coordinatorReplicas": 1, "workerReplicas": 1, "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
    }))).await;
    assert_eq!(status, StatusCode::CREATED, "create starrocks; body={j}");
    j["id"].as_str().unwrap().to_string()
}
```

`tests/routes.rs::build_app` gains a `starrocks_ddl: Arc<dyn StarRocksDdl>` parameter defaulting to a fresh `FakeStarRocksDdl`, and `setup_full` gains a sibling `setup_with_fe(provisioner, kube, fake)` so a test can keep a clone and inspect it — the same shape `FakeEnginePort` already has. `engines.rs::test_state()` passes a fresh fake.

- [ ] **Step 7: The live suite** — `crates/egret-engine-mgmt/tests/starrocks_ddl_live.rs`, gated exactly like `egret-query`'s connector suites. It uses a `benchmark` catalog, so it needs **no** external database, object store or metastore — only the dev FE:

```rust
//! Engine-gated: the real DDL runner against a real StarRocks FE. Self-skips
//! unless EGRET_TEST_STARROCKS_URL is set, so it reports `ok` while asserting
//! nothing — the test's DURATION is what distinguishes a pass from a skip.
//!
//! This is the only instrument that judges the STATEMENTS this branch builds.
//! Every property name in `render/starrocks.rs` was read out of the FE source;
//! this is where "read correctly" is tested rather than argued.

fn fe() -> Option<FeTarget> {
    let base = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some(FeTarget {
        host: u.host_str()?.to_string(),
        port: u.port().unwrap_or(9030),
        root_password: None,   // the dev allin1 container's root has none
        tls_cert_pem: None,
    })
}

#[tokio::test]
async fn create_show_alter_and_drop_a_real_catalog() {
    let Some(fe) = fe() else { eprintln!("skip"); return; };
    let runner = MysqlDdlRunner;
    let name = format!("egret_sp3b_{}", &Uuid::new_v4().simple().to_string()[..8]);
    let def = CatalogDef::parse("tpch", &json!({ "scale": 1 }), &json!(null)).unwrap();
    let ddl = render_starrocks(&name, &def, &StarRocksRenderInput::probe()).unwrap();

    runner.execute(&fe, &ddl.create_sql(&BTreeMap::new()).unwrap()).await.unwrap();
    assert!(show_catalogs(&runner, &fe).await.unwrap().contains(&name));
    let props = show_create_catalog(&runner, &fe, &name).await.unwrap().expect("present");
    assert_eq!(props["type"], "benchmark", "props={props:?}");
    assert_eq!(compare_to_fe(&ddl, &props), Drift::Matching, "props={props:?}");

    // A second CREATE is an error, not a no-op — which is why D3 reads first.
    let err = runner.execute(&fe, &ddl.create_sql(&BTreeMap::new()).unwrap()).await.unwrap_err();
    assert!(err.to_lowercase().contains("already exists"), "err={err}");

    // ALTER really changes a property, and `type` is not re-sent.
    let def2 = CatalogDef::parse("tpch", &json!({ "scale": 5 }), &json!(null)).unwrap();
    let ddl2 = render_starrocks(&name, &def2, &StarRocksRenderInput::probe()).unwrap();
    assert!(matches!(compare_to_fe(&ddl2, &props), Drift::Different(_)));
    runner.execute(&fe, &ddl2.alter_sql(&BTreeMap::new()).unwrap()).await.unwrap();
    let props2 = show_create_catalog(&runner, &fe, &name).await.unwrap().expect("present");
    assert_eq!(props2["scale"], "5.0".to_string().trim_end_matches(".0").to_string(),
        "the FE may normalise the number; record what it actually returns: {props2:?}");
    assert_eq!(compare_to_fe(&ddl2, &props2), Drift::Matching, "props2={props2:?}");

    // DROP IF EXISTS twice: the second is a success, which is what lets a
    // detach clean up a row whose catalog the FE never had (D7).
    runner.execute(&fe, &ddl.drop_sql()).await.unwrap();
    runner.execute(&fe, &ddl.drop_sql()).await.unwrap();
    assert!(!show_catalogs(&runner, &fe).await.unwrap().contains(&name));
    assert!(show_create_catalog(&runner, &fe, &name).await.unwrap().is_none());
}
```

**If `scale` comes back as `5.0` and the renderer sent `5`, `compare_to_fe` reports permanent drift.** That is precisely the class of mistake this suite exists to catch: fix it in the renderer (send what the FE echoes) and record the observed value in a comment — do not weaken the assertion.

```bash
just up   # StarRocks takes 30-60s to become healthy
EGRET_TEST_STARROCKS_URL=mysql://localhost:9030 cargo test -p egret-engine-mgmt --test starrocks_ddl_live -- --nocapture
```
Add the `justfile` recipe next to `test-dataapi-starrocks`:

```make
# The StarRocks catalog DDL this branch builds, against a real FE — the only
# instrument that judges the property names in `render/starrocks.rs`.
# Opt-in behind `EGRET_TEST_STARROCKS_URL`; CI has no engine.
# Prereq: `just up`.
test-engine-mgmt-starrocks:
    {{ no_db_skip }} DATABASE_URL={{ test_database_url }} EGRET_TEST_STARROCKS_URL=mysql://localhost:9030 \
      cargo test -p egret-engine-mgmt --test starrocks_ddl_live -- --nocapture
```

- [ ] **Step 8: Gate and commit**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --lib
just test-engine-mgmt-starrocks
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt -p egret-catalog-registry --all-targets -- -D warnings
git add -A && git commit -m "feat(engine-mgmt): injectable StarRocks DDL runner, FE introspection, pending attachments"
```
Expected: green, and the live suite's `finished in` is **seconds** — a millisecond run means it self-skipped and proved nothing.

---
### Task 4: `apply_starrocks` — attach, re-apply, detach

**Files:**
- Modify: `crates/egret-engine-mgmt/src/catalog_apply.rs`, `src/engines.rs` (`add_catalog`'s comment about the refusal), `tests/routes.rs`

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces: `catalog_apply::{apply_starrocks, render_starrocks_attached, converge_attachment}`; `STARROCKS_NOT_YET` deleted; `check_engine_supports` keeps its signature and loses its StarRocks arm.

**The shape, stated once so the code reads back to it:**

| Change | StarRocks | Why |
|---|---|---|
| Attach | write the row `pending`, then converge; a failed converge leaves `failed` + `apply_error` and the route answers **201** | D1: a fresh FE is unreachable for 5–10 minutes, which is exactly when the create-engine wizard attaches |
| Attach, row unwritable | the catalog was created on the FE and egret cannot remember it → **compensating DROP** | D7: nothing prunes a stale StarRocks catalog, unlike Trino |
| Re-apply | `SHOW CATALOGS`: absent → `CREATE`; present → `ALTER … SET` | D3: `IF NOT EXISTS` makes a credential rotation a silent no-op |
| Create | always preceded by a read; a present-but-different catalog is **refused**, FE untouched | D3: today's `IF NOT EXISTS` silently adopts a hand-made `egret` catalog |
| Detach | `DROP CATALOG IF EXISTS` first, then delete the row; the primary is still refused | unchanged from SP3a's ordering; `IF EXISTS` makes an unknown catalog a success (D7) |

- [ ] **Step 1: Write the failing route tests** — rewrite the three refusal tests and add the new coverage in `tests/routes.rs`. First the fake, next to `FakeEnginePort`:

```rust
/// Records every statement egret sends to an FE and answers `SHOW` reads from
/// an in-memory catalog map — so the whole StarRocks apply path runs in a unit
/// test with no MySQL port (design D8). `fail_next` makes the next `execute`
/// fail, standing in for an unreachable FE.
#[derive(Clone, Default)]
struct FakeStarRocksDdl {
    executed: Arc<std::sync::Mutex<Vec<String>>>,
    /// catalog name → its properties, as `SHOW CREATE CATALOG` would print them.
    catalogs: Arc<std::sync::Mutex<std::collections::BTreeMap<String, BTreeMap<String, String>>>>,
    fail: Arc<std::sync::atomic::AtomicBool>,
}
```
Its `execute` parses the three statement shapes it is given (`CREATE EXTERNAL CATALOG`, `ALTER CATALOG … SET`, `DROP CATALOG IF EXISTS`) into that map — including the FE's real behaviours: a second `CREATE` errors with `Catalog 'x' already exists`, `ALTER` merges and never unsets, `DROP IF EXISTS` on an unknown name succeeds. Its `query` answers `SHOW CATALOGS` and `SHOW CREATE CATALOG`, the latter masking `aws.s3.access_key` and dropping `password`, so the drift comparison is exercised against the FE's real output shape rather than an idealised one.

Then the tests:

```rust
/// D1: an attach to a StarRocks engine whose FE is reachable creates the
/// catalog and the attachment reads `applied`.
#[tokio::test]
async fn attaching_to_a_starrocks_engine_creates_the_catalog_on_the_fe() {
    let fake = FakeStarRocksDdl::default();
    let Some(f) = setup_with_fe(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        fake.clone(),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "sr_attach_ok", true).await;
    let engine_id = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("bench");
    let (_, j) = call(&f, "POST", "/catalogs", &token,
        Some(serde_json::json!({ "name": name, "kind": "tpch", "config": {} }))).await;
    let cid = j["id"].as_str().unwrap().to_string();

    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    assert_eq!(j["applyStatus"], "applied", "{j}");
    assert_eq!(fake.created(), vec![name.clone()]);
    assert_eq!(fake.catalog(&name).unwrap()["type"], "benchmark");
    let row = registry(&f)
        .get_attachment(Uuid::parse_str(&engine_id).unwrap(), Uuid::parse_str(&cid).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.apply_status, "applied");
    assert!(row.applied_at.is_some());
}

/// D1: an attach to an UNREACHABLE FE still answers 201, records the row
/// `failed` with the error, creates nothing — and the Retry route converges it.
#[tokio::test]
async fn an_attach_against_an_unreachable_fe_is_recorded_and_converged_by_retry() {
    // … fake.fail = true
    assert_eq!(status, StatusCode::CREATED, "D1: the row is the point; {j}");
    assert_eq!(j["applyStatus"], "failed");
    assert!(j["applyError"].as_str().unwrap().contains("fe unreachable"));
    assert!(fake.executed().iter().all(|s| !s.starts_with("CREATE EXTERNAL CATALOG")));
    // Retry converges.
    fake.set_fail(false);
    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}/apply"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{j}");
    assert_eq!(reg.get_attachment(e, c).await.unwrap().unwrap().apply_status, "applied");
}

/// D3: a catalog of the same name the FE already holds with DIFFERENT
/// properties is refused, the FE is untouched, and the error names the keys.
/// This is the `egret` catalog today's Warehouse UI tells operators to create
/// by hand, and the one a pre-SP3a engine leaves behind.
#[tokio::test]
async fn a_drifted_catalog_is_refused_and_the_fe_is_left_alone() {
    let fake = FakeStarRocksDdl::default();
    let Some(f) = setup_with_fe(…, fake.clone()).await else { eprintln!("skip"); return; };
    let token = session_for(&f, "sr_drift", true).await;
    let engine_id = create_starrocks_via_api(&f, &token).await;
    let name = unique_catalog_name("bench");
    // Somebody else's catalog, under the name egret is about to use.
    fake.seed(&name, &[("type", "benchmark"), ("scale", "99")]);
    let before = fake.catalog(&name).unwrap();

    let (_, j) = call(&f, "POST", "/catalogs", &token,
        Some(serde_json::json!({ "name": name, "kind": "tpch", "config": { "scale": 1 } }))).await;
    let cid = j["id"].as_str().unwrap().to_string();
    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;

    assert_eq!(status, StatusCode::CREATED, "D1: the row is still written; {j}");
    assert_eq!(j["applyStatus"], "failed", "{j}");
    let err = j["applyError"].as_str().unwrap();
    assert!(err.contains("scale"), "the differing key is named: {err}");
    assert_eq!(fake.catalog(&name).unwrap(), before, "the FE was left exactly as it was");
    assert!(fake.executed().iter().all(|s| !s.starts_with("CREATE EXTERNAL CATALOG")
        && !s.starts_with("ALTER CATALOG")), "reads only: {:?}", fake.executed());
}

/// D3: re-applying an edited catalog ALTERs — it never issues a second CREATE,
/// which `IF NOT EXISTS` would have turned into a silent no-op.
#[tokio::test]
async fn a_credential_rotation_reaches_the_fe_through_alter() {
    // … edit the catalog, POST /catalogs/{id}/reapply
    let sent = fake.executed();
    assert!(sent.iter().any(|s| s.starts_with("ALTER CATALOG")), "sent={sent:?}");
    assert_eq!(sent.iter().filter(|s| s.starts_with("CREATE EXTERNAL CATALOG")).count(), 1,
        "exactly the original create; sent={sent:?}");
}

/// D7: detach drops first; a catalog the FE never had is still a success, and
/// the primary catalog is still refused.
#[tokio::test]
async fn detach_drops_first_tolerates_an_unknown_catalog_and_still_refuses_the_primary() {
    // … attach `bench`, then:
    let (status, _) = call(&f, "DELETE", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(fake.catalog(&name).is_none(), "dropped on the FE");
    assert!(registry(&f).get_attachment(e, c).await.unwrap().is_none(), "row gone AFTER the drop");

    // A second attach, then a DROP the FE no longer knows about: still a
    // success, so a row can always be cleaned up.
    call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    fake.forget(&name);
    let (status, j) = call(&f, "DELETE", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "an unknown catalog is not an error; {j}");
    assert!(registry(&f).get_attachment(e, c).await.unwrap().is_none());

    // The primary is still refused, and nothing was sent to the FE for it.
    let primary = warehouse_catalog(&f, wh_id).await;
    let (status, j) = call(&f, "DELETE", &format!("/engines/{engine_id}/catalogs/{}", primary.id), &token, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{j}");
    assert!(fake.catalog(&primary.name).is_some(), "the primary is still on the FE");
}

/// D7: the DDL succeeded and the attachment row could not be written — egret
/// drops the catalog again rather than leaving one nothing will ever prune.
/// SP3a's comment said "the next apply of this engine removes it again"; that
/// is true of a Trino ConfigMap and false of a StarRocks FE, where nothing
/// prunes anything.
#[tokio::test]
async fn a_catalog_egret_cannot_remember_is_dropped_again() {
    // Drive `attach_to_engine` directly (the route cannot make the INSERT
    // fail): delete the catalog row out from under it after the DDL, by
    // attaching a catalog whose id has been removed from `registry.catalogs`
    // — the FK on `catalog_id` then refuses the attachment insert.
    let err = catalog_apply_attach_with_missing_catalog_row(&f, &engine, &catalog).await;
    assert!(matches!(err, CatalogOpError::Http(_)));
    assert!(fake.executed().iter().any(|s| s.starts_with("CREATE EXTERNAL CATALOG")));
    assert!(fake.executed().iter().any(|s| s.starts_with("DROP CATALOG IF EXISTS")),
        "the compensating drop ran: {:?}", fake.executed());
    assert!(fake.catalog(&catalog.name).is_none(), "nothing left on the FE");
}

/// D6: no statement text, and no FE error carrying one, reaches `apply_error`.
#[tokio::test]
async fn an_fe_error_stored_on_the_attachment_carries_no_ddl_and_no_secret() {
    // A `jdbc` catalog, so the statement really carries a password, and an FE
    // that fails by quoting the statement back — which is what StarRocks does
    // ("Getting analyzing error. Detail message: <statement> …").
    let secret_plaintext = format!("pw-{}", Uuid::new_v4().simple());
    let secret_id = seed_secret(&f, &secret_plaintext).await;
    // … create a jdbc catalog referencing `secret_id`
    fake.set_fail(Some("Getting analyzing error. Detail message: \
        CREATE EXTERNAL CATALOG `x` PROPERTIES (\"password\" = \"<the password>\") failed"));
    let (status, j) = call(&f, "POST", &format!("/engines/{engine_id}/catalogs/{cid}"), &token, None).await;
    assert_eq!(status, StatusCode::CREATED, "{j}");
    let err = registry(&f).get_attachment(e, c).await.unwrap().unwrap().apply_error.unwrap();
    assert!(!err.contains(&secret_plaintext), "the secret leaked into apply_error: {err}");
    assert!(!j["applyError"].as_str().unwrap().contains(&secret_plaintext), "…and into the response: {j}");
    assert!(err.contains("Getting analyzing error"), "the diagnosis survived: {err}");
}
```

And the three flipped tests:
- `attaching_to_a_starrocks_engine_is_refused` → **`attaching_an_egret_warehouse_to_a_starrocks_engine_mints_a_client_and_creates_the_catalog`**: same setup, but now asserting `CREATED`, one minted client, an attachment row, and a `CREATE EXTERNAL CATALOG` carrying `iceberg.catalog.credential`.
- `nothing_is_attached_to_a_starrocks_engine_yet` → **`a_benchmark_catalog_attaches_to_a_starrocks_engine`**: `tpch` attaches, and the statement's `type` is `benchmark`.
- `legacy_add_refused_by_engine_type_leaves_the_orphan_byte_identical` keeps its name and its whole point (F-2: a refusal decided **before** `create_catalog_replacing_orphan` destroys the orphan) but re-targets at a refusal that still exists: a **Trino-only `generic`** catalog added to a StarRocks engine, which `check_engine_supports` refuses through `can_read`. The `to_jsonb` before/after assertion is unchanged.

Also add, in the same file, a test that the *other* pre-mint refusal is still pre-mint: attaching a catalog named `default_catalog` to a StarRocks engine is a 400 that mints no client.

- [ ] **Step 2: Implement** — `catalog_apply.rs`. Delete `STARROCKS_NOT_YET` and the StarRocks arm of `check_engine_supports`, replacing it with the reserved-name refusal:

```rust
    if engine_type == EngineType::StarRocks
        && egret_catalog_registry::is_reserved_starrocks_name(catalog_name)
    {
        return Err(CatalogOpError::Http(CpError::BadRequest(format!(
            "StarRocks reserves the catalog name {catalog_name}"
        ))));
    }
```

`render_starrocks_attached(st, a, engine)` mirrors `render_attached` (warehouse, storage, credential) but builds a `StarRocksRenderInput` with `drivers: &st.sr_jdbc_drivers`, and `resolve_ddl_secrets(st, &ddl)` resolves `ddl.secret_ids()` into the `BTreeMap<Uuid, String>` the statement builders need.

`converge_attachment` is the one place the FE is written to:

```rust
/// Bring one catalog on the FE in line with the registry (D3): absent →
/// CREATE; present and matching → nothing; present and DIFFERENT → refuse,
/// naming the keys, FE untouched. `already_ours` (an attachment that has been
/// applied before) turns a match into an ALTER instead of a no-op, because a
/// rotated secret is invisible to the comparison — `SHOW CREATE CATALOG`
/// deletes `password` and masks the rest — and `CREATE … IF NOT EXISTS`, which
/// this deliberately does not use, would make that rotation a silent no-op.
pub(crate) async fn converge_attachment(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    a: &AttachedCatalog,
    already_ours: bool,
) -> Result<(), ApplyError> {
    let ddl = render_starrocks_attached(st, a, engine)
        .await
        .map_err(ApplyError::Build)?;
    let secrets = resolve_ddl_secrets(st, &ddl).await.map_err(ApplyError::Build)?;
    let runner = st.starrocks_ddl.as_ref();
    let present = show_catalogs(runner, fe)
        .await
        .map_err(|e| ApplyError::Rejected(scrub_secrets(&e, &secrets)))?
        .contains(&a.catalog.name);

    let sql = if present {
        let fe_props = show_create_catalog(runner, fe, &a.catalog.name)
            .await
            .map_err(|e| ApplyError::Rejected(scrub_secrets(&e, &secrets)))?
            .unwrap_or_default();
        match compare_to_fe(&ddl, &fe_props) {
            Drift::Different(keys) if !already_ours => {
                // D3: a catalog of this name egret did not create — the `egret`
                // catalog the Warehouse UI told operators to make by hand, or
                // one left by a pre-SP3a engine. Key NAMES only: a value could
                // be anything the FE holds.
                return Err(ApplyError::Rejected(format!(
                    "a catalog named {} already exists on this engine with different properties ({}); \
                     detach it or drop it on the engine before attaching",
                    a.catalog.name,
                    keys.join(", ")
                )));
            }
            Drift::Matching if !already_ours => return Ok(()),
            _ => ddl.alter_sql(&secrets).map_err(ApplyError::Build_render)?,
        }
    } else {
        ddl.create_sql(&secrets).map_err(ApplyError::Build_render)?
    };

    match runner.execute(fe, &sql).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // D6: what the FE said, with every secret this statement carries
            // removed — and never the statement itself.
            tracing::warn!(engine = %engine.name, catalog = %a.catalog.name,
                statement = %ddl.redacted(), "starrocks catalog DDL failed");
            Err(ApplyError::Rejected(scrub_secrets(&e, &secrets)))
        }
    }
}
```
(`ApplyError::Build_render` is shorthand here for mapping a `RenderError` into `ApplyError::Build(CpError::BadRequest(...))`; write it as a small `fn render_err(e: RenderError) -> ApplyError` rather than an inherent method, so the message is built in one place.)

`apply_starrocks` then:

```rust
async fn apply_starrocks(st, engine, change) -> Result<(), ApplyError> {
    let fe = fe_target(st, engine).await.map_err(ApplyError::Build)?;
    match change {
        CatalogChange::Attach(p) => converge one catalog (p.catalog_id, with p's pending credential),
        CatalogChange::Reapply => converge every attachment of this engine, in name order,
        CatalogChange::Detach(id) => execute the DROP for that catalog's name,
    }
}
```
`CatalogChange::Reapply` carries no catalog id, because Trino re-renders the whole engine whatever changed — but a StarRocks re-apply of one attachment must touch one catalog, or a Retry on catalog A would rewrite catalog B's `apply_status` too. So the enum gains a variant, with the asymmetry written down where it will be read:

```rust
pub(crate) enum CatalogChange {
    Attach(PendingAttachment),
    Detach(Uuid),
    /// Re-apply everything this engine has. Trino's whole-engine render; for
    /// StarRocks, every attachment in name order.
    Reapply,
    /// Re-apply ONE attachment. Trino cannot express that — `apply_trino`
    /// treats it exactly as `Reapply`, because a Trino apply is the whole
    /// ConfigMap either way — but a StarRocks apply is one statement about one
    /// catalog, and a Retry on catalog A must not restate catalog B.
    ReapplyOne(Uuid),
}
```

`retry_apply` (and therefore `propagate`, and therefore `POST /catalogs/{id}/reapply` and `POST …/catalogs/{cid}/apply`) sends `ReapplyOne(catalog_id)`; the bootstrap's convergence does not go through `CatalogChange` at all — it calls `converge_starrocks_catalogs` directly.

`attach_to_engine` branches on engine type for the **ordering only**:

```rust
    // Trino: apply, then write (SP1 Part B). StarRocks: write, then converge
    // (D1) — a fresh FE is unreachable for 5-10 minutes, and a row egret has
    // forgotten is worse than one it retries.
    if engine_type == EngineType::StarRocks {
        let row = registry.insert_attachment(&NewAttachment { …, apply_status: apply_status::PENDING, applied_now: false, … })
            .await;
        let row = match row {
            Ok(r) => r,
            // The same rollback the Trino arm runs when its apply fails:
            // revoke the client FIRST, delete the credential secret only once
            // the revoke succeeded, and on a failed revoke keep the secret and
            // log loudly (catalog_apply.rs:612-637).
            Err(e) => { rollback_minted(st, engine, minted).await; return Err(e.into()); }
        };
        match apply_catalog_change(st, engine, CatalogChange::Attach(pending)).await {
            Ok(()) => registry.set_apply_applied(engine.id, catalog.id).await?,
            Err(e) => registry.set_apply_failed(engine.id, catalog.id, &e.message()).await?,
        }
        return Ok(());
    }
```
— and D7's compensating drop lives on the other side: when the DDL succeeded and `set_apply_applied` (or, in the create-engine path, the row insert) fails, `apply_starrocks` issues `ddl.drop_sql()` and logs the redacted form. Write it as one helper, `compensate_created_catalog(st, engine, &ddl)`, so both callers use the same words.

- [ ] **Step 3: Two paths that deliberately do NOT change.**
  - **Engine delete** issues no `DROP CATALOG`: the delete destroys the FE, and a statement against a cluster that is going away can only fail or hang. `retire_minted_credential` already revokes the minted Catalog Clients and their secrets, and that is the part that outlives the cluster. Record this as a decision in the commit message; a reviewer will ask.
  - **`propagate`** (spec §4.5.3, `catalog_apply.rs:663`) needs no change: it already walks every attached engine in name order and calls `retry_apply`, which for a StarRocks engine now converges that attachment. The `POST /catalogs/{id}/reapply` route therefore reaches StarRocks with no new code — which is what `a_credential_rotation_reaches_the_fe_through_alter` above actually exercises.

- [ ] **Step 4: Gate and commit**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt
rtk proxy grep -rn "STARROCKS_NOT_YET\|not supported yet\|not_yet" crates/ | grep -v target
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
git add -A && git commit -m "feat(engine-mgmt): apply registry catalogs to StarRocks engines"
```
Expected: the grep prints **nothing** in `crates/egret-engine-mgmt` and `crates/egret-catalog-registry` (an unrelated hit elsewhere is fine and is recorded).

---
### Task 5: The bootstrap converges every attachment (D2, D8)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (`spawn_starrocks_catalog_wire`, `create_starrocks_engine`, `update_starrocks_engine`, the deleted helpers and the deleted grep test), `src/catalog_apply.rs` (`converge_starrocks_catalogs` is shared with Task 4's apply path)

**Interfaces:**
- Consumes: Tasks 3 and 4.
- Produces: `spawn_starrocks_catalog_wire(st: CpState, engine_id: Uuid, engine_name: String, namespace: String, delegation: Option<StarRocksDelegationBootstrap>) -> JoinHandle<()>`; `catalog_apply::converge_starrocks_catalogs(st: &CpState, engine: &Engine) -> ()` (never fails: it writes each attachment's outcome).
- Deleted: `starrocks_external_catalog_sql`, `sr_primary_catalog_credential`, `sr_escape_dq`, `sr_escape_ident`, `run_starrocks_ddl`, `spawn_starrocks_catalog_wire_keeps_all_six_guard_checkpoints`.

- [ ] **Step 1: Write the failing bootstrap tests** — in `engines.rs`'s test module, next to the surviving first-checkpoint test. These are what D8 buys: five checkpoints that could only be grepped before are now driven.

Four helpers first, declared in `engines.rs`'s test module because that is where these tests live:

```rust
/// A `CpState` whose `starrocks_ddl` is a fake, returning both so a test can
/// inspect what the "FE" was told. Everything else is `test_state()`.
async fn test_state_with_fake_fe() -> Option<(CpState, FakeStarRocksDdl)>;

/// The status a ready StarRocks FE reports: one pod whose name contains
/// `-fe-`, which is what the bootstrap's poll actually looks for.
fn ready_fe_status(engine_name: &str) -> egret_engine_mgmt_api::EngineStatus;

/// N registry catalogs (`kind: tpch`) attached to this engine as `pending`,
/// returning name → catalog id.
async fn seed_pending_attachments(st: &CpState, engine: &Engine, names: &[&str])
    -> BTreeMap<String, Uuid>;
```

and the poll budget has to be reachable from a test, or each of these cases costs five minutes of real sleeping:

```rust
/// 30 polls × 10 s ≈ 5 minutes, twice (FE readiness, then root login). A test
/// drives the unhappy paths — an FE that never reports ready, a root login
/// that never succeeds — so the budget is a constant the test build shrinks
/// rather than a literal in the loop.
#[cfg(not(test))]
const MAX_POLLS: u32 = 30;
#[cfg(test)]
const MAX_POLLS: u32 = 1;
```
(Shrinking it in the test build means the happy-path tests must make the FIRST poll succeed — which `ready_fe_status` does — and it means `MAX_POLLS` itself is no longer under test. That is the right trade: what these tests are for is the checkpoints and the loop, not arithmetic on a constant.)

A ready-FE provisioner is needed too, because the existing fake will not do: `FakeEnginePort::status` reports one ready pod named `coordinator-0`, and the bootstrap waits for a pod whose name **contains `-fe-`** (`engines.rs:4118`). Use `FakeEnginePort::set_status` with a pod named `{engine}-fe-0`, or the poll loop burns its 30 × 10 s budget and the test hangs.

```rust
/// D2: the bootstrap converges EVERY attachment, in name order, and records
/// each one — not just the primary, and not just in the log.
#[tokio::test]
async fn the_bootstrap_converges_every_attachment_in_name_order() {
    // seed: a StarRocks engine with its primary plus two more attachments
    // (`a_cat`, `z_cat`), all `pending`
    handle.await.expect("bootstrap task did not panic");
    let names: Vec<String> = fake_ddl.created();   // in the order CREATE ran
    assert_eq!(names, vec!["a_cat", "egret", "z_cat"], "name order");
    for c in [primary, a, z] {
        let row = reg.get_attachment(engine.id, c).await.unwrap().unwrap();
        assert_eq!(row.apply_status, "applied", "catalog {c}");
        assert!(row.applied_at.is_some());
    }
}

/// D2 + D8 (checkpoint 3): an engine deleted while the loop runs stops it, and
/// the attachments it never reached stay `pending` — not `applied`, and not
/// `failed`, because nothing was attempted. Before SP3b this checkpoint could
/// only be asserted by counting `engine_still_current(` in the function's own
/// source text.
#[tokio::test]
async fn deleting_the_engine_mid_loop_stops_the_bootstrap_and_leaves_the_rest_pending() {
    let Some(st) = test_state_with_fake_fe().await else { eprintln!("skip"); return; };
    let (wh, _s) = seed_sr_warehouse(&st, &db).await;
    let engine = seed_sr_engine(&st, wh.id).await;
    // Three pending attachments: a_cat, m_cat, z_cat.
    let ids = seed_pending_attachments(&st, &engine, &["a_cat", "m_cat", "z_cat"]).await;
    // The fake deletes the engine row the moment the FIRST catalog is created,
    // so the loop's next checkpoint is the one under test.
    let store = st.engine_mgmt.clone();
    let engine_id = engine.id;
    fake.on_create(move |_name| {
        let store = store.clone();
        Box::pin(async move { store.delete_engine(engine_id).await.unwrap(); })
    });

    spawn_starrocks_catalog_wire(st.clone(), engine.id, engine.name.clone(), engine.namespace.clone(), None)
        .await
        .expect("bootstrap task did not panic");

    assert_eq!(fake.created(), vec!["a_cat"], "the loop stopped at the next catalog");
    // The rows survive the engine delete only if the cascade has not run yet;
    // assert on what the fake saw, which is the checkpoint's observable effect.
    assert!(fake.executed().iter().filter(|s| s.starts_with("CREATE EXTERNAL CATALOG")).count() == 1);
}

/// One catalog's DDL failing does not abandon the others, and what lands in
/// `apply_error` carries neither the statement nor the secret (D6).
#[tokio::test]
async fn a_failing_catalog_is_flagged_and_the_loop_goes_on() {
    // … three pending attachments; the fake refuses `m_cat` only.
    fake.fail_catalog("m_cat", "Getting analyzing error. Detail message: unsupported type");
    spawn_starrocks_catalog_wire(st.clone(), engine.id, engine.name.clone(), engine.namespace.clone(), None)
        .await
        .unwrap();
    let reg = st.engine_mgmt.registry();
    assert_eq!(reg.get_attachment(engine.id, ids["a_cat"]).await.unwrap().unwrap().apply_status, "applied");
    let m = reg.get_attachment(engine.id, ids["m_cat"]).await.unwrap().unwrap();
    assert_eq!(m.apply_status, "failed");
    assert!(m.apply_error.unwrap().contains("unsupported type"));
    assert_eq!(reg.get_attachment(engine.id, ids["z_cat"]).await.unwrap().unwrap().apply_status, "applied",
        "a failure before it must not cost the last catalog its apply");
}

/// Checkpoints 1 and 2: an FE that never reports ready, and a root login that
/// never succeeds, each end the task having executed no catalog DDL. Both
/// loops are bounded at 30 × 10 s, so this test pins `MAX_POLLS` down to 1 via
/// the same `#[cfg(test)]` constant the poll loop reads — otherwise it would
/// take five minutes per case, which is a test nobody runs.
#[tokio::test]
async fn an_fe_that_never_becomes_ready_writes_nothing() {
    // (a) status reports a pod that is not an FE.
    let prov = FakeEnginePort::default();
    prov.set_status(Some(egret_engine_mgmt_api::EngineStatus {
        phase: "provisioning".into(),
        message: None,
        pods: vec![egret_engine_mgmt_api::EnginePod { name: "sr-cn-0".into(), phase: "Running".into(), ready: true }],
    }));
    spawn_starrocks_catalog_wire(st.clone(), engine.id, engine.name.clone(), engine.namespace.clone(), None)
        .await
        .unwrap();
    assert!(fake.executed().is_empty(), "no statement before the FE is ready: {:?}", fake.executed());
    assert_eq!(
        reg.get_attachment(engine.id, ids["a_cat"]).await.unwrap().unwrap().apply_status,
        "pending",
        "an unreachable FE leaves `pending`, not `failed`: nothing was attempted"
    );

    // (b) FE ready, but root login never succeeds.
    fake.set_fail(Some("Access denied for user 'root' (using password: YES)"));
    prov.set_status(Some(ready_fe_status(&engine.name)));
    spawn_starrocks_catalog_wire(st.clone(), engine.id, engine.name.clone(), engine.namespace.clone(), None)
        .await
        .unwrap();
    assert!(fake.created().is_empty(), "no catalog before root can log in");
}

/// Checkpoints 4-6: the delegation half still runs, and `already exists` still
/// repairs — driven through the fake runner instead of a real MySQL port,
/// which is what `spawn_starrocks_catalog_wire_keeps_all_six_guard_checkpoints`
/// existed to approximate.
#[tokio::test]
async fn the_delegation_bootstrap_still_creates_then_repairs() {
    let delegation = StarRocksDelegationBootstrap {
        jwks_url: "https://idp/jwks".into(),
        issuer: "https://idp/realms/egret".into(),
        audience: "engine".into(),
        tls_cert_pem: Some("CERTPEM".into()),
        service_user: "egret_delegator".into(),
        service_password: "pw".into(),
    };
    // First run: the integration is created, the user and grant follow.
    spawn_starrocks_catalog_wire(st.clone(), engine.id, engine.name.clone(), engine.namespace.clone(), Some(delegation.clone()))
        .await
        .unwrap();
    let sent = fake.executed();
    assert!(sent.iter().any(|s| s.contains("CREATE SECURITY INTEGRATION egret_oidc")), "{sent:?}");
    assert!(sent.iter().any(|s| s.contains("CREATE USER IF NOT EXISTS")), "{sent:?}");

    // Second run against an FE that already has it: the repair ALTER runs, so
    // a changed issuer takes effect instead of being lost to `already exists`.
    fake.fail_matching("CREATE SECURITY INTEGRATION", "security integration 'egret_oidc' already exists");
    fake.clear_executed();
    spawn_starrocks_catalog_wire(st.clone(), engine.id, engine.name.clone(), engine.namespace.clone(), Some(delegation))
        .await
        .unwrap();
    assert!(fake.executed().iter().any(|s| s.contains("ALTER SECURITY INTEGRATION egret_oidc")), "{:?}", fake.executed());

    // D6: the delegation statements now share the catalog path's FeTarget, so
    // the cert is pinned for all of them — and for the root-login probe, which
    // carries the root password.
    assert_eq!(fake.last_target().tls_cert_pem.as_deref(), Some("CERTPEM"));
}
```

Delete `spawn_starrocks_catalog_wire_keeps_all_six_guard_checkpoints` **in this step**, together with the paragraph of its doc comment that explains why a behavioural seam "wasn't attempted here" — that paragraph is now false, and a stale justification is worse than none. Keep `bootstrap_stops_before_any_provisioner_call_when_engine_already_gone` and trim its doc comment to what is still true.

- [ ] **Step 2: Rewrite `spawn_starrocks_catalog_wire`**

```rust
/// Best-effort, non-blocking post-provision convergence: poll until an FE pod
/// is ready, wait for a real `root` login (the `initpwd` Job races the FE), then
/// bring EVERY attachment of this engine onto the FE and record the outcome of
/// each one (design D2) — where before it created exactly one catalog and only
/// logged what happened. When `delegation` is `Some`, the JWT integration and
/// impersonation service user follow, as before.
///
/// It takes the whole `EngineMgmtState` rather than twelve positional
/// arguments because it now reads the registry, the secrets and the storage
/// registry itself; `CpState` is `Clone` and `'static`, so the spawned task
/// owns a clone.
///
/// Every `engine_still_current` checkpoint from before is still here. The third
/// one — which used to guard a single catalog statement — now runs once per
/// attachment, so a delete part-way through a ten-catalog engine stops at the
/// next catalog instead of finishing the batch.
#[allow(clippy::too_many_arguments)]
fn spawn_starrocks_catalog_wire(
    st: CpState,
    engine_id: Uuid,
    engine_name: String,
    namespace: String,
    delegation: Option<StarRocksDelegationBootstrap>,
) -> tokio::task::JoinHandle<()>
```

The polling structure is unchanged (`MAX_POLLS = 30`, `POLL_INTERVAL = 10s`, `consecutive_read_errors` threaded through every checkpoint). What changes:
- the provisioner comes from `st.engine_provisioner` (a `None` there ends the task with a warning, as every other path treats it);
- the root-login probe and every statement go through `st.starrocks_ddl` with the `FeTarget` from `fe_target(&st, &engine)` — the probe included, so the root password no longer crosses a TLS-capable wire in cleartext (D6);
- the single `starrocks_external_catalog_sql` call is replaced by `catalog_apply::converge_starrocks_catalogs(&st, &engine).await`.

```rust
/// Bring every attachment of a StarRocks engine onto its FE, in catalog-name
/// order, recording `applied` / `failed` per attachment (D2). Never returns an
/// error: each attachment's outcome is its own row, and one failure must not
/// hide the others. Re-entrant — this is what the bootstrap runs once the FE
/// is ready, and a Retry runs the same `converge_attachment` for one row.
pub(crate) async fn converge_starrocks_catalogs(st: &CpState, engine: &Engine) {
    let fe = match fe_target(st, engine).await {
        Ok(fe) => fe,
        Err(e) => {
            tracing::warn!(engine = %engine.name, error = %e,
                "starrocks convergence: could not resolve the FE connection; nothing applied");
            return;
        }
    };
    let registry = st.engine_mgmt.registry();
    let mut attachments = match registry
        .list_engine_attachments(engine.id, Some(engine.warehouse_id))
        .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(engine = %engine.name, error = %e,
                "starrocks convergence: could not read the engine's attachments");
            return;
        }
    };
    // Name order (D2). The store's own order is primary-first, then attachment
    // time — Trino's env-var numbering depends on it, so it is re-sorted here
    // rather than changed there.
    attachments.sort_by(|a, b| a.catalog.name.cmp(&b.catalog.name));

    // `engine_still_current` and `MAX_CONSECUTIVE_ENGINE_READ_ERRORS` become
    // `pub(crate)` in `engines.rs` for this loop; nothing about them changes.
    let mut consecutive_read_errors = 0u32;
    for a in &attachments {
        // The checkpoint that used to guard one statement now guards each
        // catalog: a delete part-way through stops at the next one.
        if !engine_still_current(&st.engine_mgmt, engine.id, &engine.name, &mut consecutive_read_errors).await {
            return;
        }
        let already_ours = a.attachment.apply_status == apply_status::APPLIED;
        let outcome = converge_attachment(st, engine, &fe, a, already_ours).await;
        let write = match &outcome {
            Ok(()) => registry.set_apply_applied(engine.id, a.catalog.id).await,
            Err(e) => registry.set_apply_failed(engine.id, a.catalog.id, &e.message()).await,
        };
        if let Err(e) = write {
            tracing::warn!(engine = %engine.name, catalog = %a.catalog.name, error = %e,
                "starrocks convergence: the outcome could not be recorded");
        }
    }
}
```

- [ ] **Step 3: The two call sites**

`create_starrocks_engine` (`engines.rs:3709`) passes `st.clone()` and drops the warehouse/catalog/credential arguments; it also writes the primary attachment with `apply_status: apply_status::PENDING` (`engines.rs:3608`) — the one line of Task 3's mechanical sweep that was deliberately left for here, because this is the task that makes the status true.

`update_starrocks_engine` (`engines.rs:4797`) keeps its `deleg_changed` gate and loses the `sr_primary_catalog_credential` lookup entirely, along with the `None =>` warning arm that skipped the re-bootstrap when no primary credential was found — convergence reads its own credentials now, so that arm has nothing left to protect:

```rust
    if deleg_changed {
        if let Some(boot) = bootstrap {
            spawn_starrocks_catalog_wire(
                st.clone(),
                engine.id,
                engine.name.clone(),
                engine.namespace.clone(),
                Some(boot),
            );
        }
    }
```

- [ ] **Step 4: Retry converges what the bootstrap converges.** No new route: `POST /engines/{id}/catalogs/{cid}/apply` (SP3a) already reaches `retry_apply`, which for a StarRocks engine now runs the same `converge_attachment` the bootstrap runs, for that attachment — including the engine's **primary** catalog, which is an attachment like any other. An engine whose bootstrap failed is therefore converged one Retry per catalog, where before it could only be fixed by re-creating the engine (D2). The delegation half of the bootstrap stays reachable only through a delegation-changing `PUT`, exactly as today; SP3b does not change that and does not claim to.

- [ ] **Step 5: Gate and commit**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
rtk proxy grep -rn "starrocks_external_catalog_sql\|sr_primary_catalog_credential\|keeps_all_six_guard_checkpoints" crates/
git add -A && git commit -m "feat(engine-mgmt): converge every StarRocks attachment from the bootstrap"
```
Expected: the grep prints nothing; the bootstrap tests run in **milliseconds of wall clock** (the fake FE is ready on the first poll, so no `POLL_INTERVAL` sleep is ever reached) — a test binary that takes minutes means a poll loop is really sleeping and the fake is not wired.

---
### Task 6: Routes, OpenAPI and the generated client

**Files:**
- Modify: `crates/egret-engine-mgmt/src/catalogs.rs` (two response descriptions, the DTO doc), `openapi.json`, `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Tasks 4 and 5.
- Produces: no new route, no new DTO field — only descriptions that tell the truth, and the regenerated client.

- [ ] **Step 1: `attach_catalog`'s responses** (`catalogs.rs:576-610`). The 201 now covers two outcomes and must say so; the 400 loses StarRocks and gains the reserved name; the 502 stays Trino's:

```rust
        (status = 201, description = "Attached. Trino: the catalog was applied to the cluster first, so `applyStatus` is `applied`. StarRocks: the attachment is recorded first and the catalog is created on the FE immediately afterwards, so `applyStatus` is `applied` on success and `failed` (with `applyError`) when the FE could not be reached or refused — the attachment still exists and is converged by `POST …/catalogs/{cid}/apply` or by the engine's next bootstrap", body = CatalogAttachmentDto),
        (status = 400, description = "The engine cannot read this catalog, the name is one StarRocks reserves, or Kubernetes is not configured", body = ErrorBody),
        …
        (status = 502, description = "Trino only: Kubernetes rejected the apply (`code` = `engine_cluster_apply_failed`); nothing was attached. A StarRocks FE that rejects the DDL is reported as a `failed` attachment in the 201 body, not as a 502", body = ErrorBody),
```

- [ ] **Step 2: `retry_catalog_apply`'s responses** (`catalogs.rs:612-652`):

```rust
        (status = 200, description = "Re-applied; the attachment is `applied`. StarRocks: an absent catalog is created and an existing one is updated in place with `ALTER CATALOG … SET`", body = ApplyOutcomeDto),
        (status = 400, description = "Kubernetes is not configured (Trino), or the catalog cannot be rendered for this engine", body = ErrorBody),
        (status = 502, description = "The engine rejected the apply — Kubernetes for Trino, the FE for StarRocks (including a catalog of this name the FE already holds with different properties); the attachment stays `failed` with the new error", body = ErrorBody),
```

- [ ] **Step 3: `CatalogAttachmentDto::apply_status`'s doc comment** gains the third value, since `pending` is now reachable by any client:

```rust
    /// `applied` | `failed` | `pending`. `pending` means egret has recorded the
    /// attachment and has not yet been able to tell the engine — a StarRocks
    /// engine whose FE is still coming up (5-10 minutes after create), which
    /// the bootstrap or a Retry converges.
```

- [ ] **Step 4: The impact route is already right.** `catalog_impact` (`catalogs.rs:455-481`) returns `in_place` for `engine_type == "starrocks"` and `restart` otherwise, which is what spec §4.5.1 asks for. **Change nothing there**; this step exists to record that it was checked, not to edit it.

- [ ] **Step 5: Regenerate**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && git status --short src/api/generated && cd ..
```
Commit exactly what `gen:api` produced, including churn this branch did not cause; confirm `encodeURIComponent` survived on every path parameter of the two touched routes.

```bash
git add -A && git commit -m "docs(api): describe StarRocks attach and retry semantics; regenerate the client"
```

---
### Task 7: Full gate, then a live check (stop before the PR)

**Files:** none new — this task verifies, and fixes what it finds in the task that introduced it.

- [ ] **Step 1: Branch and tree**

```bash
git branch --show-current
git status --short
git log --oneline origin/main..HEAD
```
Expected: `feat/starrocks-catalog-registry-sp3b`, a clean tree, six commits (Tasks 1–6).

- [ ] **Step 2: Rust gates**

```bash
export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: fmt clean; clippy clean except the two pre-existing `egret-workflow` findings — record their text. Anything else is this branch's.

- [ ] **Step 3: Template, then the whole workspace once**

```bash
psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test --workspace --no-fail-fast
```
Expected: no failures. Read the `finished in` of `egret-engine-mgmt`'s `routes` and `lib` binaries and `egret-catalog-registry`'s `store`: seconds, not milliseconds. A failure outside this branch's crates is called "pre-existing" only after running it on `origin/main` in a separate worktree and seeing it fail there too.

- [ ] **Step 4: The engine-gated suite against a real FE**

```bash
just up          # wait for the StarRocks healthcheck (30-60s)
just test-engine-mgmt-starrocks
```
Expected: green, in seconds of real work. This is the only step in the whole plan that judges the property names against StarRocks itself; a skip here means the branch ships DDL no machine has ever accepted.

- [ ] **Step 5: API description and client**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > /tmp/planSP3b-openapi.json
cmp /tmp/planSP3b-openapi.json openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && git status --short src/api/generated && npm run lint && npm run typecheck && npm test && npm run build && cd ..
```
Expected: `cmp` silent; `git status` after `gen:api` shows nothing. (Use the session's scratchpad instead of `/tmp`, prefixed `planSP3b-`.)

- [ ] **Step 6: Kind cluster — manual, reported as manual.** Deploy with `just deploy-kind` (or the OIDC variant the cluster runs), then provision a StarRocks engine through the Control Plane and, with `command kubectl --context kind-egret exec -n egret-system deploy/postgres -- psql -U egret -d egret -Atc "…"` plus the FE's own MySQL port:
  - Immediately after create: `SELECT apply_status FROM registry.engine_catalog_attachments WHERE engine_id = '<id>'` → `pending`. After the FE is ready (5–10 min): `applied`, with `applied_at` set. **This is D2's whole point — a bootstrap outcome that is now in the database instead of only in a log line.**
  - `kubectl --context kind-egret exec -n egret-engines <fe-pod> -- mysql -uroot -p<pw> -P9030 -h127.0.0.1 -e 'SHOW CATALOGS'` → the engine's primary catalog is there.
  - Attach an `iceberg` (a second warehouse's) catalog and a `jdbc` PostgreSQL catalog (point it at the in-cluster `postgres`), then query each: `SELECT count(*) FROM <name>.<schema>.<table>`.
  - `SHOW CREATE CATALOG <jdbc name>` → record **verbatim** whether `password` is absent and whether `aws.s3.access_key` is masked, and whether the iceberg catalog's `iceberg.catalog.credential` comes back in clear (it is not in `CredentialUtil`'s mask list, unlike the `iceberg.catalog.oauth2.credential` spelling egret does not use — see the self-review below).
  - Rotate the JDBC password secret, `POST /api/control-plane/catalogs/{id}/reapply`, query again → the new password is in effect and the FE log shows one `ALTER`, no second `CREATE`.
  - Detach both, `SHOW CATALOGS` again → gone. Detach the primary → 400.
  Report every result as "manual, on kind", never as a test.

- [ ] **Step 7: Stop.** Do not push and do not open a PR. Report: the six commit hashes, the gate outputs (with the two recorded `egret-workflow` clippy findings), the live-FE suite result, the kind results verbatim, and anything left open.

---

## Self-review (against the design)

**Coverage of D1–D8.** D1 (records-then-converges, 201 with a `failed` status, detach still applies first) — Task 4 Steps 1–2, documented in Task 6 Step 1, with `pending` from Task 3. D2 (converge every attachment in name order, per-attachment status, checkpoints kept, reachable from Retry) — Task 5. D3 (read before create; absent/matching/drifted; `ALTER` never a second `CREATE`) — Task 3 Steps 3–4 (the comparison) and Task 4 Step 2 (the decision), proven against a real FE in Task 3 Step 7. D4 (StarRocks 4.x floor, the `can_read` matrix) — Task 2 Step 2. D5 (pinned drivers in configuration, per-catalog override, refusal) — Task 1 Step 3 and Task 2 Steps 1, 3, 4, with the deviation stated in plan table rows 1 and 2. D6 (secrets resolved at build time, never logged, errors scrubbed, the FE certificate on the DDL channel) — Task 1 (`PropValue`, `redacted`, `scrub_secrets`), Task 3 (`fe_target`, every statement through one channel), Task 4 (the `apply_error` test). D7 (compensating drop, unknown catalog on detach is success, the primary still refused) — Task 4. D8 (injectable runner, the grep test replaced by behavioural tests) — Task 3 Step 4 and Task 5 Step 1.

**Design §4 (testing).** Rendering per kind and dialect against hand-written DDL including `benchmark`, `deltalake` and every pinned driver — Task 1 Step 2. The faked runner driving attach-against-ready-FE, attach-against-unreachable-FE then Retry, drift refusal, rotation via `ALTER`, detach of an unknown catalog, and the compensating drop — Task 4 Step 1, all six named. Checkpoints as behavioural tests — Task 5 Step 1. Live on kind, reported as manual — Task 7 Step 6.

**Out of scope, deliberately.** The Catalogs UI, the Warehouses redirect and the removal of `engine_catalogs` / `engines.warehouse_id` / `warehouse:*` (SP3c). Per-catalog Ranger policy generation and StarRocks tag handling (SP4; upstream starrocks#67458). Making `engines.warehouse_id` nullable — SP3b keeps the primary-catalog rule exactly as SP3a left it.

**Three things a reviewer should push on.**

1. **egret's Iceberg-REST OAuth2 property names are not the ones StarRocks masks.** The bootstrap has always sent `iceberg.catalog.credential` and `iceberg.catalog.oauth2-server-uri` — the Iceberg-standard names, which reach the Iceberg library through StarRocks' `iceberg.catalog.` pass-through (`IcebergRESTCatalog.java:107-113`), and which were verified live on 3.3 when the `iceberg.catalog.oauth2.*` dot-form was silently ignored. Task 1 keeps them, because changing a live-proven credential path on 4.1 without evidence is worse than the alternative. But `CredentialUtil` masks the **dot-form** (`CredentialUtil.java:68-69`) and not the one egret sends, so `SHOW CREATE CATALOG` on an egret warehouse catalog may return the OAuth2 credential in clear to any FE admin. D6 already concedes that an FE admin can read credentials back, so this is not a new exposure class — but it is a real difference from what the design assumed, it is why Task 7 Step 6 asks for that output verbatim, and the follow-up (prove the dot-form works on 4.1, then switch and gain masking) belongs in SP4's ticket, not in a silent change here.

2. **The `mysql` dialect renders a MariaDB URL.** `JDBCMetadata` rewrites `jdbc:mysql` → `jdbc:mariadb` and loads `org.mariadb.jdbc.Driver` "because of gpl protocol" (`JDBCMetadata.java:121-128, 187-194`), so Trino's `sslMode=VERIFY_IDENTITY` would arrive at a driver that has never heard of it. Task 1 Step 4 therefore sends the MariaDB form for both dialects and pins the MariaDB driver for both. The consequence worth stating plainly: **a `jdbc`/`mysql` catalog renders different text for Trino and StarRocks** — the first plan in this programme where one registry row does not produce the same connection string everywhere. It still produces the same *catalog name* on both engines, which is what D5 of the SP3 design actually requires.

3. **The `can_read` matrix refuses nothing new.** Row 3 of the plan table says so explicitly rather than letting Task 2 imply a behaviour change. If a reviewer wants D4 to bite — e.g. refusing `clickhouse` on an engine pinned to a 3.2 image — that needs the FE version, which egret does not read anywhere today; the honest place for it is a version probe in SP3c, and this plan does not pretend to have one.
