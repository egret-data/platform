# Catalog Engine CE-6 — GC / task-queue workers — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Spawn lakekeeper's built-in task-queue workers in the embedded catalog so dropped/soft-deleted tables get garbage-collected (expire → purge files) and the task log is pruned. Today the registry is empty (REST only, no workers).

**Architecture:** `build_state` registers the three built-in queues (`tabular_expiration`, `tabular_purge`, `task_log_cleanup`) into the same `TaskQueueRegistry` it puts into the lakekeeper `State`, and returns that registry. `egret-bin` spawns the registry's workers via `spawn_gc_workers(registry, cancel_token)` and cancels them on SIGINT. The Postgres task queue is multi-worker/replica safe (`SKIP LOCKED`).

**Tech Stack:** Rust, lakekeeper (embedded, sqlx 0.9), tokio, `tokio_util::sync::CancellationToken` (re-exported as `lakekeeper::CancellationToken`).

**Branch:** `feat/catalog-engine-ce6-gc-workers` (off main). DB: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO up.

**Spec:** `docs/en/specs/2026-06-27-catalog-engine-CE6-gc-workers-design.md`.

**Verified seams:**
- `crates/egret-catalog-engine/src/mount.rs`: `build_state(dsn: &str, system_user: &str) -> anyhow::Result<ApiContext<EngineState>>` — builds `catalog: CatalogState`, `secrets: SecretsState`, `authz: EgretAuthorizer`, `server_id`; currently `let registered_task_queues = TaskQueueRegistry::new().registered_task_queues();` (~line 129) then `State { authz, catalog, secrets, ..., registered_task_queues, ... }`. `EngineState = State<EgretAuthorizer, PostgresBackend, SecretsState>`.
- lakekeeper (all on `TaskQueueRegistry`, which is `Clone`): `TaskQueueRegistry::new()`; `async register_built_in_queues::<C: CatalogStore, S: SecretStore, A: Authorizer>(&self, catalog_state: C::State, secret_store: S, authorizer: A, poll_interval: Duration) -> &Self`; `registered_task_queues(&self) -> RegisteredTaskQueues`; `async task_queues_runner(&self, cancel: CancellationToken) -> TaskQueuesRunner`; `async is_empty(&self) -> bool`. `TaskQueuesRunner::run_queue_workers(self, restart_workers: bool)`. Import `lakekeeper::CancellationToken`.
- egret-bin `serve()` (`crates/egret-bin/src/main.rs`): `let engine_state = egret_catalog_engine::build_state(dsn, &config.catalog.system_user).await?;` (~line 286); ends with `axum::serve(listener, app).await` (~line 475). No cancellation token today.
- `build_state` test call sites (~8): `tests/bootstrap.rs:31`, `tests/browse_api.rs:103`, `tests/front_auth.rs:{120,319,604,714}`, `tests/provision_ws.rs:65` (all `let engine_state = build_state(&dsn, SYSTEM_USER).await...`).

---

## Task 1: `build_state` registers built-ins + returns registry; `spawn_gc_workers`

**Files:** `crates/egret-catalog-engine/src/mount.rs`; `crates/egret-catalog-engine/src/lib.rs`; all `build_state` call sites (tests + `egret-bin/src/main.rs`); test `crates/egret-catalog-engine/tests/gc_workers.rs` (new).

- [ ] **Step 1 (failing test)** new `tests/gc_workers.rs` (copy the DB setup from `tests/bootstrap.rs` — migrate + `build_state`):
```rust
#[tokio::test]
async fn build_state_registers_builtin_gc_queues() {
    // set up test DB (run_lakekeeper_migrations like bootstrap.rs), then:
    let (_state, registry) = egret_catalog_engine::build_state(&dsn, SYSTEM_USER).await.unwrap();
    assert!(!registry.is_empty().await, "built-in GC queues must be registered (was empty)");
    // optional: assert there are 3 queues
}
```
Run `cargo test -p egret-catalog-engine --test gc_workers` → FAIL (build_state returns ApiContext, not a tuple).

- [ ] **Step 2 (build_state change):** in `mount.rs`, change the signature to `pub async fn build_state(dsn: &str, system_user: &str) -> anyhow::Result<(ApiContext<EngineState>, TaskQueueRegistry)>`. Replace the empty-registry line with:
```rust
let registry = TaskQueueRegistry::new();
registry
    .register_built_in_queues::<PostgresBackend, SecretsState, EgretAuthorizer>(
        catalog.clone(),
        secrets.clone(),
        authz.clone(),
        std::time::Duration::from_secs(10), // lakekeeper CONFIG default poll interval
    )
    .await;
let registered_task_queues = registry.registered_task_queues();
```
Build `State { authz, catalog, secrets, ..., registered_task_queues, ... }` as before, then `Ok((ApiContext { v1_state: state }, registry))`. (Note: `catalog`, `secrets`, `authz` are moved into `State`; clone them for `register_built_in_queues` BEFORE building `State`, or reorder so the registration happens while they're still owned — register first, then build State with the originals.) Update the `mount.rs` module doc comment that claims no workers are spawned.

- [ ] **Step 3 (`spawn_gc_workers`):** add to `mount.rs` (and `pub use` from `lib.rs`):
```rust
/// Spawn lakekeeper's built-in GC task-queue workers (tabular expiration +
/// purge + task-log cleanup). Runs until `cancel` fires; restarts any worker
/// that exits. Call once, after `build_state`, from the binary.
pub fn spawn_gc_workers(registry: TaskQueueRegistry, cancel: lakekeeper::CancellationToken) {
    tokio::spawn(async move {
        let runner = registry.task_queues_runner(cancel).await;
        runner.run_queue_workers(true).await;
    });
}
```

- [ ] **Step 4 (update all call sites to the tuple):** `grep -rn "build_state(" crates --include=*.rs`. In each test, `let engine_state = build_state(...)...` → `let (engine_state, _registry) = build_state(...)...`. In `egret-bin/src/main.rs`, `let engine_state = ...build_state(...).await?;` → `let (engine_state, _gc_registry) = ...build_state(...).await?;` (Task 2 turns `_gc_registry` into the spawned workers — for now ignore it so the bin compiles).

- [ ] **Step 5:** `cargo test -p egret-catalog-engine --test gc_workers` → PASS. `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`; single iceberg. Commit `feat(catalog-engine): register built-in GC queues + spawn_gc_workers`.

---

## Task 2: egret-bin spawns the GC workers

**Files:** `crates/egret-bin/src/main.rs`.

- [ ] **Step 1:** In `serve()`, after `build_state` (now `let (engine_state, gc_registry) = ...`), and after the engine wiring is set up, create a cancellation token and spawn the workers:
```rust
let gc_cancel = lakekeeper::CancellationToken::new();
egret_catalog_engine::spawn_gc_workers(gc_registry, gc_cancel.clone());
{
    let gc_cancel = gc_cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        gc_cancel.cancel();
    });
}
```
Place this only in the branch where the embedded catalog is enabled (where `build_state` is called). If `lakekeeper` isn't a direct dependency of `egret-bin`, reach `CancellationToken` through `egret_catalog_engine` (re-export it: add `pub use lakekeeper::CancellationToken;` to `egret-catalog-engine/src/lib.rs` and use `egret_catalog_engine::CancellationToken`) — prefer this so `egret-bin` doesn't gain a direct lakekeeper dep.

- [ ] **Step 2:** Add a startup log line: `tracing::info!("catalog GC workers started")` after spawning.

- [ ] **Step 3:** `cargo build -p egret-bin`; `cargo clippy -p egret-bin --all-targets -- -D warnings`; `cargo fmt --all`. (No new unit test here — Task 3 covers behavior.) Optional live smoke: `cargo run -p egret-bin -- serve` and confirm the "catalog GC workers started" log + no panic; Ctrl-C exits cleanly. Commit `feat(bin): start catalog GC workers on serve, stop on SIGINT`.

---

## Task 3: end-to-end GC integration test

**Files:** test `crates/egret-catalog-engine/tests/gc_e2e.rs` (new); study `tests/front_auth.rs` (mounts the engine router, provisions a warehouse, drives Iceberg ops over `oneshot`) and `tests/roundtrip.rs` (real table create) for the harness.

- [ ] **Step 1 (failing test):** new `tests/gc_e2e.rs`:
  - Set up: migrate, `build_state`, `ensure_bootstrapped`, provision a warehouse for a workspace, mount the engine router (front_auth or the raw `mount_catalog_engine` harness the other tests use).
  - Create a namespace + a table (real Iceberg create → metadata/data in MinIO), capture its storage prefix.
  - **Drop the table with purge:** issue the Iceberg REST drop with purge — `DELETE /catalog/v1/{prefix}/namespaces/{ns}/tables/{table}?purgeRequested=true` (confirm the exact query param lakekeeper expects by checking its drop handler / the OpenAPI; it is `purgeRequested=true`). Assert the drop returns success (204/200) and the table no longer loads (404).
  - **Run GC:** build a registry registered with a SHORT poll interval for the test — i.e. add a test-only variant or pass `Duration::from_secs(1)`. Simplest: register the built-ins directly in the test (mirror `build_state`'s registration block but with `Duration::from_secs(1)`), reusing the same `catalog`/`secrets`/`authz` from the engine state, then `spawn_gc_workers(registry, cancel)`. (If reaching those sub-states out of `ApiContext` is awkward, add a `#[cfg(feature = "test-utils")]` helper in the engine crate that registers + spawns with a custom interval.)
  - **Assert GC ran:** poll, with a generous timeout (≈20 s) and a short sleep between polls, until the `tabular_purge` task reaches a terminal/success state. Determine the assertion target by inspecting the lakekeeper task table in the `lakekeeper` schema (e.g. `SELECT status, ... FROM <task table> WHERE queue_name = 'tabular_purge'` — find the real table + status column/enum by querying the schema or reading lakekeeper's task store). Acceptable alternatives if the task-row assertion is awkward: assert the table's data/metadata prefix is gone from MinIO (list-objects under the captured prefix → empty). Pick the LESS flaky one and document the choice in the test.
  - Cancel the token at the end to stop the workers.
  Run → FAIL (until the harness compiles + GC works).

- [ ] **Step 2:** Make it pass. Tolerate one transient MinIO write flake (retry the create once); do NOT weaken the GC assertion to make it pass. If the worker needs the warehouse's storage credential (CE-5 path), confirm the provisioned warehouse's MinIO creds let purge delete the files.

- [ ] **Step 3:** `cargo test -p egret-catalog-engine --test gc_e2e` → PASS (may take ~10–20 s). `cargo test -p egret-catalog-engine` (full crate) green; `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`; single iceberg. Commit `test(catalog-engine): e2e GC — drop-with-purge is reclaimed by workers`.

---

## Self-review checklist
- [ ] `build_state` registers the 3 built-in queues into the State's registry (drop-with-purge can enqueue) AND returns the registry.
- [ ] `spawn_gc_workers` spawns the runner; `egret-bin` starts it on `serve` and cancels on SIGINT; tests ignore the registry (no background pollers).
- [ ] e2e: a table dropped with purge is actually reclaimed by the workers (task reaches success / files gone) within the timeout.
- [ ] All `build_state` call sites updated; suite green; clippy/fmt; single iceberg.
- [ ] `egret-bin` gains no direct lakekeeper dep (CancellationToken re-exported via egret-catalog-engine).

## Out of scope
Per-live-table Iceberg maintenance (snapshot expiry/orphan/compaction); GC enable/disable toggle + worker-count config; full axum graceful shutdown; CE-5 follow-ons.
