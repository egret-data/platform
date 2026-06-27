# Catalog Engine CE-6 — GC / task-queue workers — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-27-catalog-engine-CE6-gc-workers-design.md`.
> Bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-27 · **Branch:** `feat/catalog-engine-ce6-gc-workers` (off main; CE-1..CE-5 đã merge).

## Mục tiêu
Spawn các **built-in task-queue worker** của lakekeeper trong catalog embed để bảng đã drop/soft-delete được GC (expire → purge file khỏi object store) + prune task log — thay vì registry rỗng chỉ serve REST. Hiện drop bảng không bao giờ giải phóng storage, soft-delete không finalize.

## Quyết định (chốt)
- **Register built-in queue vào registry của State, VÀ spawn worker.** `build_state` hiện nhét registry RỖNG vào `State` → (1) không worker chạy, (2) handler drop-with-purge enqueue purge task **qua registry của State** nên registry rỗng = không enqueue được. CE-6 register 3 queue vào đúng registry đi vào State, rồi spawn worker của registry đó.
- **`build_state` register built-ins + trả registry**; `egret-bin` lo spawn worker (với cancellation token gắn shutdown). Giữ vòng đời worker ở binary, test bỏ qua registry (không có poller nền trong test).
- **GC luôn BẬT** (không toggle MVP — YAGNI, ghi follow-on). Worker count + poll interval lấy default CONFIG lakekeeper (2 worker mỗi loại; poll 10s).
- **Multi-replica an toàn, không leader election** (PG queue dùng `FOR UPDATE SKIP LOCKED`).
- **Phạm vi = built-in GC của lakekeeper:** `tabular_expiration` (tabular soft-deleted hết retention → lên lịch purge), `tabular_purge` (xoá file data/metadata của tabular đã drop khỏi object store — cần `SecretStore` lấy storage creds), `task_log_cleanup` (prune task-log). Đây là **GC drop/soft-delete + log cleanup** — KHÔNG phải Iceberg maintenance per-live-table (snapshot expiry/orphan/compaction — lớp khác, để sau).

## Cơ chế (lakekeeper, đã verify)
`serve_inner` làm (embed đang bỏ): tạo `TaskQueueRegistry::new()` → `register_built_in_queues::<C,S,A>(catalog, secrets, authz, poll)` → `registered_task_queues()` vào State → `task_queues_runner(cancel)` → nếu `!is_empty()` thì `spawn(runner.run_queue_workers(true))`. `lakekeeper::CancellationToken` = re-export `tokio_util::sync::CancellationToken`. `TaskQueueRegistry` Clone; generic theo `PostgresBackend`/`SecretsState`/`EgretAuthorizer` (đúng type trong `EngineState`).

## Thành phần
1. **`build_state`** (mount.rs): đổi return `Result<(ApiContext<EngineState>, TaskQueueRegistry)>`. Thay block registry rỗng bằng tạo registry + `register_built_in_queues::<PostgresBackend, SecretsState, EgretAuthorizer>(catalog.clone(), secrets.clone(), authz.clone(), GC_POLL_INTERVAL).await` (GC_POLL_INTERVAL = `CONFIG.task_poll_interval` nếu reachable, else `Duration::from_secs(10)`); `registered_task_queues()` vào State; trả `(ApiContext, registry)`. Sửa doc comment đầu mount.rs (đang nói "spawn no workers… skip").
2. **`spawn_gc_workers(registry, cancel)`** (hàm mới): `tokio::spawn(async move { registry.task_queues_runner(cancel).await.run_queue_workers(true).await })`.
3. **egret-bin `serve()`**: `let (engine_state, gc_registry) = build_state(...).await?;` tạo `CancellationToken`, gọi `spawn_gc_workers(gc_registry, cancel.clone())`; spawn task nhỏ `tokio::signal::ctrl_c()` → `cancel.cancel()`. axum serve giữ nguyên (optional `.with_graceful_shutdown` cùng token).
4. **Test call sites**: ~8 chỗ `build_state(...)` trong `tests/{bootstrap,browse_api,front_auth,provision_ws}.rs` đổi `let (engine_state, _registry) = build_state(...)` (test không spawn worker).

## Test
- Wiring (nhẹ): `let (_state, registry) = build_state(...)` → assert `registered_task_queues()` non-empty (3 queue) — chứng minh wiring vs rỗng cũ.
- E2E GC (DB+MinIO): bootstrap+provision warehouse → tạo namespace+table (file thật MinIO) → drop table **with purge** (enqueue `tabular_purge`) → build registry poll interval ngắn (~1s) + `spawn_gc_workers` → poll (timeout ~15-20s) tới khi purge task terminal/success (hoặc prefix bảng biến mất khỏi MinIO). MinIO flake → retry (xem [[egret-run-full-ci-local-before-merge]]); nếu assert file-absence flaky thì assert task status → success.

## Rủi ro
Vòng đời/shutdown worker (token phải tới đúng runner); poller nền trong test (tránh bằng test bỏ qua registry); ripple chữ ký build_state (~8 test + bin, compile bắt); purge cần storage creds (CE-5 per-workspace cred — verify purge chạy với warehouse MinIO); e2e flaky (poll ngắn + timeout rộng + assert task status). Giữ 1 iceberg.

## Ngoài phạm vi
Iceberg maintenance per-live-table (snapshot expiry/orphan/compaction); toggle bật/tắt GC + cấu hình worker-count trong Config egret; full graceful-shutdown axum; CE-5 follow-ons.
