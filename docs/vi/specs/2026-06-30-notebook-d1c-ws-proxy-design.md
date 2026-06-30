# Notebook D1c — Kernel WS Proxy (thiết kế)

**Trạng thái:** nháp · **Ngày:** 2026-06-30 · **Epic:** Notebooks · **Trước đó:** C1 (#94), F1 (#95), D1b (#96, đã merge)
> Bản tiếng Anh là canonical: `docs/en/specs/2026-06-30-notebook-d1c-ws-proxy-design.md`. Bản này tham chiếu.

## 1. Bối cảnh & phạm vi
D1 = D1a (image+helpers) · D1b (launcher+lifecycle, XONG) · **D1c (WS proxy, spec này)**. D1b spawn 1 Jupyter Kernel Gateway per (user×workspace) + start/status/stop. D1c nối browser tới kernel: resolve 1 kernel **per-notebook** trong gateway đó + **proxy 2 chiều WS channels Jupyter** giữa FE và gateway. KHÔNG render output / nối nút Run — đó là E1.

Sau D1c: FE mở WS sống tới kernel, trao đổi message Jupyter. Render output + nối editor = E1.

### Trong phạm vi
- Bảng map `notebook_kernels` `(notebook_id, user_id) → kernel_id, last_used_at` (per-notebook kernel trong gateway của user).
- Gateway HTTP client (egret-kernel): ensure/create kernel trong gateway đang chạy, delete kernel.
- Trait `KernelProxy` + impl `BackendWsProxy` (proxy-in-backend, sparklabx) — sau trait để swap route-table/CHP sau.
- Endpoint connect `POST /api/ws/{ws}/notebooks/{nb}/kernel/connect` (flow sparklabx: ensure gateway D1b → resolve/create kernel_id → trả về).
- Endpoint WS `GET /api/ws/{ws}/notebooks/{nb}/kernel/channels` upgrade + proxy tới `<gateway>/api/kernels/{kernel_id}/channels`, touch mỗi message.

### Ngoài phạm vi (sau)
Nối cell execute + render output + nút Run (E1); interrupt/restart kernel (E1/sau, YAGNI); image+helpers (D1a — D1c proxy tới cái gateway expose, chưa có D1a thì verify bằng fake); proxy route-table/CHP (trait chừa chỗ, chỉ build BackendWsProxy).

## 2. Quyết định (brainstorm)
- **Per-notebook kernel** (sparklabx) — mỗi notebook có kernel_id riêng trong gateway user×ws; state cách ly. Cần map `notebook_kernels`.
- **Connect flow sparklabx** — REST `connect` lo ensure gateway (D1b, `spawning` thì FE poll) + resolve/create kernel_id, rồi FE mở WS. Spawn chậm xử lý ở D1b POST /kernel + poll status, KHÔNG trong WS upgrade.
- **Proxy-in-backend** (sparklabx) cho MVP, sau trait `KernelProxy` để swap CHP/route-table kiểu JupyterHub khi scale nhiều docker/k8s kernel. (JupyterHub `Spawner` xác nhận `KernelLauncher` của D1b; mô hình proxy riêng của họ là đường scale documented.)

## 3. Components
- **3.1 Migration `notebook_kernels`** (append-only): `(notebook_id, user_id) PK → kernel_id text, last_used_at`, FK cascade. Store `NotebookKernelStore`: get/upsert/touch/delete. kernel_id do gateway cấp (regenerable — gateway restart thì id cũ stale, tạo lại).
- **3.2 Gateway HTTP client** (`src/gateway.rs`, reqwest): `ensure_kernel(gateway_url, existing?) -> kernel_id` (existing Some → GET `<gw>/api/kernels/{id}`; 200 reuse, else tạo `POST <gw>/api/kernels` → parse `{"id"}`), `delete_kernel` (best-effort). Lỗi → KernelError sạch, không panic.
- **3.3 trait `KernelProxy` + `BackendWsProxy`** (`src/proxy.rs`): `proxy_channels(client_ws, gateway_url, kernel_id, on_activity)` — build URL `ws://.../api/kernels/{id}/channels`, nối tokio-tungstenite, 2 task bơm (client→gateway, gateway→client) với `tokio::select!`, forward frame text+binary nguyên văn (Jupyter opaque với egret), `on_activity()` mỗi message. Sau trait để swap `ChpProxy` sau.
- **3.4 Connect handler** `POST /api/ws/{ws}/notebooks/{nb}/kernel/connect` (gate `notebook:read`, notebook thuộc caller): `ensure_running` (env qua build_notebook_runtime+manifest_to_env như D1b); chưa Ready → `{phase:"spawning"}` (FE poll); Ready → `ensure_kernel(url, store.get(nb,user))` → upsert → `{phase:"ready", kernel_id}`.
- **3.5 WS channels handler** `GET .../kernel/channels` (axum `WebSocketUpgrade`): auth bằng **session cookie** (validate session + gate `notebook:read` + own/readable-notebook TRƯỚC upgrade, 401/403 nếu fail) → resolve gateway (status Ready) + kernel_id (store.get, thiếu thì ensure_kernel+upsert) → `ws.on_upgrade(|sock| proxy.proxy_channels(sock, url, kid, on_activity))`; `on_activity` = `manager.touch` + `store.touch` (throttle ~15s, tránh ghi DB mỗi message).
- **3.6 Ranh giới crate:** tất cả trong `egret-kernel`. `KernelState` (D1b) thêm `NotebookKernelStore` + `proxy: Arc<dyn KernelProxy>` + gateway client. Route connect+channels thêm vào `kernel_router()`. egret-bin construct `Arc::new(BackendWsProxy)`. Thêm feature `ws` cho axum + `tokio-tungstenite` vào Cargo.toml egret-kernel.

## 4. Luồng dữ liệu
FE mở notebook → `POST connect` (poll tới `{ready, kernel_id}`) → mở WS `channels` → backend auth + resolve kernel_id+gateway_url → `BackendWsProxy` nối `<gw>/api/kernels/{kid}/channels` + bơm 2 chiều → message Jupyter (execute_request ↑, iopub/result ↓) chảy qua → (render = E1). Mỗi message touch kernel để reaper giữ sống.

## 5. Auth / bảo mật
Connect: `require_workspace("notebook:read")`, kernel/notebook là của caller (key từ principal+ws; notebook_id verify thuộc ws + own/readable — tái dùng read-scope notebook để không nối kernel tới notebook private/khác-ws). WS upgrade: auth từ session cookie (không body), cùng gate TRƯỚC upgrade; user chỉ proxy tới **kernel của chính mình** (kernel_id resolve từ notebook_kernels theo user_id) — không địa chỉ được kernel người khác. Proxy forward frame opaque, egret không sửa message. Gateway localhost-only.

## 6. Xử lý lỗi
Gateway chưa Ready khi connect → `{phase:"spawning"}` (poll), không phải lỗi; trên WS chưa Ready → close code sạch (FE chạy lại connect). Gateway unreachable/create fail → KernelError sạch → 502/503 (connect) hoặc WS close, không panic. kernel_id stale (gateway restart) → GET non-200 → tạo mới + upsert (self-heal). Proxy: 1 bên đóng → cả 2 pump dừng sạch.

## 7. Kiểm thử
- **Toàn bộ proxy/connect bằng FAKE (không Python):**
  - **Fake gateway HTTP server** (axum/tokio nhỏ) trả `GET/POST/DELETE /api/kernels` → test `ensure_kernel` (create trả id; existing-alive reuse; existing-dead recreate) + `delete_kernel`.
  - **Fake WS echo server** (tokio-tungstenite) làm channels endpoint → test `BackendWsProxy.proxy_channels` round-trip frame text+binary 2 chiều + `on_activity` fires mỗi message; đóng 1 bên kết thúc proxy.
  - Connect handler (`#[sqlx::test]` + FakeLauncher + fake gateway HTTP): `{spawning}` khi chưa ready; `{ready, kernel_id}` + upsert `notebook_kernels` khi ready; gate `notebook:read` (non-member 403); keying per-user.
- **End-to-end** (gateway Jupyter thật + execute) = **phụ thuộc D1a**, documented, không trong CI.
- **CI local** trước merge; migration **append-only**.

## 8. Phân rã build (1 spec)
Migration notebook_kernels + store · gateway HTTP client · trait `KernelProxy` + `BackendWsProxy` (+ test fake-echo) · connect handler · WS channels handler (auth session-cookie + upgrade + touch) · axum `ws` + `tokio-tungstenite` · wire `KernelState`/router/egret-bin.

## 9. Câu hỏi mở (chốt khi plan)
1. Throttle touch trên WS: tối đa ~15s/lần (clock mỗi connection) vs mỗi message. *Lean: throttle ~15s.*
2. Auth WS-upgrade: tái dùng session extractor sẵn có vs extractor nhỏ riêng trên upgrade request (cookie). *Lean: tái dùng cách validate session của extractor thường, áp cho upgrade.*
3. Scheme URL gateway channels: `ws://` từ `http://` gateway_url (subprocess plain http/ws); confirm host:port (gateway_url đã là `http://127.0.0.1:{port}`).
