# Notebook D1b — Kernel Launcher + Lifecycle (thiết kế)

**Trạng thái:** nháp · **Ngày:** 2026-06-30 · **Epic:** Notebooks · **Trước đó:** C1 (PR #94), F1 (PR #95, đã merge)
> Bản tiếng Anh là canonical: `docs/en/specs/2026-06-30-notebook-d1b-kernel-launcher-design.md`. Bản này tham chiếu.

## 1. Bối cảnh & phạm vi
D1 (kernel runtime) tách 3 sub-spec: **D1a** image+helpers (Python/Docker), **D1b** launcher+lifecycle (Rust, *spec này*), **D1c** WS proxy (Rust). E1 (execution + output UI) sau.

**D1b xây vòng đời kernel bằng Rust:** crate mới `egret-kernel` spawn + track 1 process **Jupyter Kernel Gateway** per (user×workspace), API start/status/stop, reap idle, inject manifest F1 `/notebook-runtime` vào env kernel lúc spawn. **Không** proxy protocol / execute cell — đó là D1c/E1. Sau D1b: start/stop + xem status kernel; chưa chạy code được.

### Trong phạm vi
- Crate `egret-kernel`: trait `KernelLauncher` + impl **subprocess** + impl **fake** (test).
- Bảng registry `kernels` (1 row/user×workspace).
- `KernelManager` orchestrate launcher+registry (ensure/status/stop/touch).
- API lifecycle (session plane).
- Idle reaper (tokio task).
- Refactor nhẹ F1: tách `build_notebook_runtime(ws, user)` để cả HTTP handler lẫn launcher dùng chung; serialize thành env.

### Ngoài phạm vi (sau)
WS proxy (D1c); execute + output (E1); image+helpers (D1a — subprocess impl spawn `jupyter kernelgateway` kỳ vọng có trên PATH host, image production là D1a); launcher docker/k8s (trait thiết kế sẵn, chỉ build subprocess); MinIO IAM/resource presets.

## 2. Quyết định (brainstorm)
- Slice đầu = **D1b** (Rust, full-test bằng fake launcher). Cơ chế = **Jupyter Kernel Gateway** (kiểu sparklabx, proxy ở D1c).
- Kernel scope = **per (user×workspace)** (1 Python kernel — Daft/PySpark chỉ là import).

## 3. Components
- **3.1 trait `KernelLauncher`** (pluggable subprocess→docker→k8s): `ensure(key, env) -> KernelHandle{url, phase}`, `status`, `stop`, `mode`. `KernelKey{user_id, workspace_id}`, `KernelPhase{Spawning,Ready,Dead}`. **Subprocess impl**: chọn port localhost trống, spawn `jupyter kernelgateway --ip=127.0.0.1 --port=<port>` với env (manifest + EGRET_API_URL/EGRET_KERNEL_TOKEN), poll `GET <url>/api` tới ready (~90s, import Daft/Spark chậm), track PID; stop = kill PID; giữ map in-process `key→(Child,url)` (process OS là nguồn sự thật, DB mirror cho status/reaper). **Fake impl** (test-only): ghi nhận call, trả phase cấu hình + url giả, không spawn → mọi test lifecycle/registry/reaper/API chạy bằng Rust, không cần Python.
- **3.2 Bảng `kernels`** (migration append-only): `(user_id, workspace_id) PK, gateway_url, pid, phase[spawning|ready|dead], created_at, last_used_at` + index `(phase,last_used_at)`. `KernelRegistry`: upsert/update phase/touch/delete/list_idle.
- **3.3 `KernelManager`**: `ensure_running` (registry có Ready + process sống → touch+trả : launcher.ensure→upsert), `status`, `stop`, `touch`; giữ config idle timeout.
- **3.4 API lifecycle** (session plane, gate `notebook:read`): `POST /ws/{ws}/kernel` (ensure→{phase,url?}), `GET /ws/{ws}/kernel` (status, không spawn), `DELETE /ws/{ws}/kernel` (stop). Kernel keyed per (user×workspace) — user chỉ start/stop **kernel của chính mình**, không cross-user.
- **3.5 Idle reaper**: tokio task định kỳ (~60s) `list_idle(now-timeout)` → stop+delete. Timeout config (default 30 phút). Endpoint lifecycle bump touch; D1c/E1 touch thêm mỗi WS message.
- **3.6 Manifest injection (F1→kernel):** refactor tách `build_notebook_runtime(state, ws, user) -> NotebookRuntime` (dùng chung HTTP handler + launcher). Lúc spawn: build manifest → JSON → inject env `EGRET_NOTEBOOK_RUNTIME` (username, catalog{url,warehouse,namespace,token}, engines, catalogs) + `EGRET_API_URL` + `EGRET_KERNEL_TOKEN` ngắn hạn (kernel callback lấy credential tươi — endpoint callback là D1c/E1, env wire sẵn). Image D1a đọc các env này.
- **3.7 Ranh giới crate:** `egret-kernel` → `egret-notebook` (build_notebook_runtime) + egret-auth/workspace/metastore. Launcher thuần (nhận `KernelEnv` map), không phụ thuộc catalog; handler ráp env. `egret-kernel` expose `kernel_router()`+`KernelState`, mount ở egret-server; reaper+`KernelManager` (với launcher đã chọn) construct ở egret-bin.

## 4. Trạng thái vòng đời
`(none) → POST → Spawning → (poll) → Ready → … → idle/DELETE → Dead/(removed)`. Spawn không ready trong timeout → Dead + lỗi sạch (row xóa/đánh dead, POST sau retry). Process chết (PID mất) → coi như (none), respawn.

## 5. Xử lý lỗi
Spawn fail (thiếu binary, bind port, timeout) → `KernelError` → API error sạch (503/500), không panic; registry không để false `Ready`. Restart backend: map child in-process mất, row DB còn `Ready` nhưng process mất → manager check liveness (subprocess không re-adopt được child không phải nó spawn → coi orphaned là dead + respawn; reaper dọn row stale). Document.

## 6. Kiểm thử
- **Toàn bộ logic lifecycle bằng Rust + FakeLauncher** (`#[sqlx::test]`): registry upsert/touch/delete; `ensure_running` existing-Ready vs spawn-new; status/stop; reaper chỉ kill row quá timeout; API (chỉ own-kernel, gate notebook:read, start→status→stop). Không cần Python.
- **Manifest injection test:** start-handler build manifest (reuse F1), env có `EGRET_NOTEBOOK_RUNTIME` đúng username/namespace/token + engines enabled.
- **Subprocess impl:** unit-test pick port + ráp argv + poll-ready với HTTP server giả (spawn process tí phục vụ `/api`), HOẶC mark spawn `jupyter kernelgateway` thật là test ignored/manual. Chạy gateway thật = **infra (cần Python + jupyter_kernel_gateway)** → documented, verify khi có image D1a.
- **CI local** trước merge; migration **append-only**.

## 7. Phân rã build (1 spec)
Migration `kernels` · trait `KernelLauncher` + types + FakeLauncher · SubprocessLauncher · store `KernelRegistry` · `KernelManager` · routes lifecycle · idle reaper · refactor F1 `build_notebook_runtime` + inject env · wire `egret-kernel` vào egret-server/egret-bin.

## 8. Câu hỏi mở (chốt khi plan)
1. Cấp port: OS-assigned (bind :0) vs range cấu hình. *Lean: OS-assigned cho subprocess MVP.*
2. `EGRET_KERNEL_TOKEN`: dùng lại token 1-grant F1, hay token session ngắn hạn riêng cho callback. *Lean: token ký riêng (token catalog là cho Iceberg REST, không phải callback egret) — nhưng endpoint callback là D1c/E1, nên D1b inject placeholder/api url, chốt token ở D1c.*
3. Re-check liveness sau restart — respawn best-effort vs startup sweep đánh dead hết row. *Lean: startup sweep đánh dead orphaned.*
