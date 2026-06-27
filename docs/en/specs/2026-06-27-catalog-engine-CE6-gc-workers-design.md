# Catalog Engine CE-6 — GC / task-queue workers — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/catalog-engine-ce6-gc-workers` (off main; CE-1..CE-5 merged)
**Parent:** `docs/en/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`; predecessor `docs/en/specs/2026-06-27-catalog-engine-CE5-per-workspace-storage-design.md`

## 0. Goal

Spawn lakekeeper's **built-in task-queue workers** in the embedded catalog so that dropped / soft-deleted tables get garbage-collected (expire → purge their files from object storage) and the task log is pruned — instead of the current empty registry that serves the REST surface but runs no background work. Today, dropping a table never reclaims its storage and soft-deletes never finalize.

## 1. Decisions (locked)

- **Register the built-in queues into the State's registry, AND spawn their workers.** The current `build_state` puts an EMPTY `TaskQueueRegistry` into the lakekeeper `State`. That has two problems: (1) no workers run, and (2) the drop-with-purge handler enqueues a purge task **through the State's registry**, so with an empty registry that enqueue path has no queue registered. CE-6 registers the three built-in queues into the same registry that goes into `State`, and spawns that registry's workers.
- **`build_state` registers the built-ins and returns the registry**; `egret-bin` owns spawning the workers (with a cancellation token tied to shutdown). This keeps the worker lifecycle in the binary, not buried inside state construction, and lets tests ignore the registry (no background pollers in the test suite).
- **GC is always on** (no enable/disable toggle for the MVP — YAGNI; noted as a follow-on). Worker counts + poll interval come from lakekeeper's `CONFIG` defaults (2 workers each for expiration / purge / log-cleanup; 10 s poll).
- **Multi-replica safe with no leader election.** lakekeeper's Postgres task queue uses `FOR UPDATE SKIP LOCKED`, so multiple workers / replicas compete safely for tasks.
- **Scope = lakekeeper's built-in GC only:** `tabular_expiration` (soft-deleted tabulars past retention → schedule purge), `tabular_purge` (delete a dropped tabular's data/metadata files from object storage — needs the `SecretStore` for storage creds), `task_log_cleanup` (prune old task-log rows). This is **drop/soft-delete GC + log cleanup** — NOT per-live-table Iceberg maintenance (snapshot expiry, orphan-file removal, compaction), which is a separate, later concern.

## 2. Background — the mechanism (lakekeeper, verified)

`serve_inner` (lakekeeper `serve.rs`) does, and the embed currently skips:
```rust
let registry = TaskQueueRegistry::new();
registry.register_built_in_queues::<C, S, A>(catalog_state, secrets_state, authorizer, CONFIG.task_poll_interval).await;
let registered_task_queues = registry.registered_task_queues();   // → State
// ... State { ..., registered_task_queues, ... }
let runner = registry.task_queues_runner(cancellation_token).await;
if !registry.is_empty().await {
    spawn(async move { runner.run_queue_workers(true).await; });  // restart_workers = true
}
```
- `lakekeeper::CancellationToken` (re-export of `tokio_util::sync::CancellationToken`) — used for graceful shutdown.
- `TaskQueueRegistry` is `Clone`; `register_built_in_queues::<C, S, A>` is generic over the catalog / secret / authorizer types — egret instantiates it with `PostgresBackend`, `SecretsState`, `EgretAuthorizer` (the same types in `EngineState`).

## 3. Components

### 3.1 `build_state` (`egret-catalog-engine/src/mount.rs`)
- Change the return type to `anyhow::Result<(ApiContext<EngineState>, TaskQueueRegistry)>`.
- Replace the empty-registry block (currently `let registered_task_queues = TaskQueueRegistry::new().registered_task_queues();`) with:
  ```rust
  let registry = TaskQueueRegistry::new();
  registry
      .register_built_in_queues::<PostgresBackend, SecretsState, EgretAuthorizer>(
          catalog.clone(), secrets.clone(), authz.clone(), GC_POLL_INTERVAL)
      .await;
  let registered_task_queues = registry.registered_task_queues();
  ```
  where `GC_POLL_INTERVAL` is a `Duration` (use `lakekeeper::CONFIG.task_poll_interval` if reachable, else a literal `Duration::from_secs(10)` matching lakekeeper's default). Build the `State` with this `registered_task_queues` (so the drop→enqueue path + task-mgmt API work), and return `(ApiContext { v1_state: state }, registry)`.
- The doc comment at the top of `mount.rs` (which currently says "we spawn no workers / that is serve_inner's job, which we deliberately skip") must be updated to reflect that CE-6 now registers the built-ins and the binary spawns the workers.

### 3.2 `spawn_gc_workers` (new, `egret-catalog-engine`)
A small public fn (e.g. in `mount.rs` or a new `gc.rs`):
```rust
pub fn spawn_gc_workers(registry: TaskQueueRegistry, cancel: lakekeeper::CancellationToken) {
    tokio::spawn(async move {
        let runner = registry.task_queues_runner(cancel).await;
        runner.run_queue_workers(true).await;
    });
}
```
(`run_queue_workers(true)` restarts any worker that exits. It runs until the cancellation token fires.)

### 3.3 `egret-bin` `serve()` (`crates/egret-bin/src/main.rs`)
- Destructure the new return: `let (engine_state, gc_registry) = build_state(dsn, &config.catalog.system_user).await?;`.
- Create a `lakekeeper::CancellationToken`; after the engine state is built (and bootstrap/provisioning are set up), call `egret_catalog_engine::spawn_gc_workers(gc_registry, cancel.clone())`.
- Wire graceful stop: spawn a small task that awaits `tokio::signal::ctrl_c()` and calls `cancel.cancel()` so the workers stop cleanly on SIGINT. (Keep `axum::serve(listener, app).await` as-is for the MVP; optionally pass the same token to `axum::serve(...).with_graceful_shutdown(...)` — nice-to-have, not required.)
- The other branch (when the embedded catalog is disabled / `build_state` not called) spawns no workers — unchanged.

### 3.4 Test call sites
Every `build_state(...)` caller in the engine test suite (`tests/{bootstrap,browse_api,front_auth,provision_ws}.rs` — ~8 sites) becomes `let (engine_state, _registry) = build_state(...)` (tests don't spawn workers). Mechanical sweep.

## 4. Testing

- **Registry wiring (unit/integration, light):** `let (_state, registry) = build_state(dsn, system_user).await?;` then assert `registry.registered_task_queues()` is **non-empty** (the three built-in queues registered) — proving CE-6 wired them vs the previous empty registry. (`is_empty().await == false`, or assert the count / queue names.)
- **End-to-end GC (DB + MinIO):** the real proof of the worker loop —
  1. Bootstrap + provision a warehouse; create a namespace + a table (real Iceberg create, files in MinIO).
  2. Drop the table **with purge** (the Iceberg `DELETE …/tables/{t}?purgeRequested=true`, or the lakekeeper drop-with-purge path the engine exposes) → a `tabular_purge` (and/or expiration) task is enqueued in lakekeeper's task table.
  3. Build the registry with a **short poll interval** (≈1 s — register the built-ins with `Duration::from_secs(1)` for the test) and `spawn_gc_workers`.
  4. Poll (generous timeout, ≈15–20 s) until the purge task reaches a terminal/success state (the task row leaves `pending`/`running`, or the table's metadata/data prefix is gone from MinIO). Assert it completed.
  - This test does live MinIO I/O and timing; use a generous timeout and tolerate one transient MinIO flake (retry), per [[egret-run-full-ci-local-before-merge]]. If asserting on MinIO file-absence proves flaky, assert on the **task status transition to success** instead (less dependent on exact storage semantics).

## 5. File map
- Modify: `crates/egret-catalog-engine/src/mount.rs` (build_state return + register built-ins + `spawn_gc_workers`; update the module doc); `crates/egret-catalog-engine/src/lib.rs` (export `spawn_gc_workers`); `crates/egret-bin/src/main.rs` (destructure + spawn + ctrl_c cancel); the ~8 engine test call sites; a new test for the e2e GC path.
- No migration (lakekeeper's task tables already exist in the `lakekeeper` schema).

## 6. Risks
- **Worker lifecycle / shutdown:** workers run detached on the tokio runtime; the cancellation token must actually reach them (pass the SAME token into `task_queues_runner`). On SIGINT, cancel → `run_queue_workers` returns. If the token is never fired, workers stop only at process exit (acceptable MVP).
- **Background pollers in tests:** avoided by having tests ignore the returned registry (they never call `spawn_gc_workers`). Confirm no test path spawns workers (which would poll the test DB every interval and could interfere with other tests / leave tasks running).
- **`build_state` signature ripple:** ~8 test call sites + the bin must be updated in lockstep (compile-enforced).
- **Purge needs storage creds:** `tabular_purge_worker` reads the warehouse's storage credential via the `SecretStore` to delete files. For per-workspace storage (CE-5) the credential lakekeeper stored at warehouse-create time is used — confirm purge works against a MinIO warehouse in the e2e test.
- **e2e test flakiness:** live MinIO + timing; mitigate with short poll interval + generous timeout + task-status assertion. Single `iceberg` version must hold.

## 7. Out of scope (later)
- Per-live-table Iceberg maintenance: snapshot expiry, orphan-file cleanup, compaction (a separate maintenance subsystem).
- A GC enable/disable toggle + per-queue worker-count config in egret's own `Config` (MVP uses lakekeeper CONFIG defaults).
- Full graceful-shutdown for the axum server (only the GC workers get the cancellation token here).
- CE-5 follow-ons (GCS/Azure, full `SecretStore` bridge).
