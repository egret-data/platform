# Notebook D1c — Kernel WS Proxy (design)

**Status:** draft · **Date:** 2026-06-30 · **Epic:** Notebooks · **Predecessors:** C1 (#94), F1 (#95), D1b (#96, all merged)

## 1. Context & scope

D1 (kernel runtime) = D1a (image+helpers) · D1b (launcher+lifecycle, DONE) · **D1c (WS proxy, this spec)**. D1b spawns one Jupyter Kernel Gateway per (user×workspace) and exposes start/status/stop. D1c connects the browser to a kernel: it resolves a **per-notebook** kernel inside that gateway and **bidirectionally proxies the Jupyter channels WebSocket** between the frontend and the gateway. It does NOT render cell output or wire the Run button — that is E1.

After D1c, the frontend can open a live WS to a kernel and exchange Jupyter messages (execute_request/iopub/etc.). Output rendering + the editor wiring are E1.

### In scope
- A `notebook_kernels` mapping table `(notebook_id, user_id) → kernel_id, last_used_at` (per-notebook kernel within the user's gateway).
- A gateway HTTP client (egret-kernel): ensure/create a kernel in a running gateway, delete a kernel.
- A `KernelProxy` trait + a `BackendWsProxy` impl (proxy-in-backend, sparklabx-style) — behind the trait so a route-table/CHP-style proxy can replace it later.
- A connect endpoint `POST /api/ws/{ws}/notebooks/{nb}/kernel/connect` (sparklabx flow: ensure gateway via D1b → resolve/create the per-notebook kernel_id → return it).
- A WS endpoint `GET /api/ws/{ws}/notebooks/{nb}/kernel/channels` that upgrades + proxies to `<gateway>/api/kernels/{kernel_id}/channels`, touching the kernel on each message.

### Out of scope (later)
- Cell execution wiring + output rendering + the Run button (E1).
- Interrupt / restart kernel endpoints (E1 or a follow-up — YAGNI for D1c core).
- The kernel image + helpers (D1a) — D1c proxies to whatever the gateway exposes; without D1a's real gateway, D1c is verified against fakes.
- A dedicated route-table/CHP proxy (JupyterHub model) — the `KernelProxy` trait leaves room for it; only `BackendWsProxy` is built now.

## 2. Decisions (from brainstorm)
- **Per-notebook kernel** (sparklabx) — each notebook gets its own kernel_id in the user×ws gateway; isolated state between notebooks. Requires the `notebook_kernels` mapping.
- **sparklabx connect flow** — a `connect` REST that ensures the gateway (D1b, may return `spawning` for the FE to poll) and resolves/creates the kernel_id, then the FE opens the WS. The slow gateway spawn is handled by D1b's POST /kernel + status polling, NOT inside the WS upgrade.
- **Proxy-in-backend** (sparklabx) for the MVP, behind a `KernelProxy` trait so a JupyterHub-style route-table/CHP proxy can swap in when egret scales to many concurrent docker/k8s kernels. (JupyterHub's `Spawner` validated D1b's `KernelLauncher`; its dedicated-proxy model is the documented scale path.)

## 3. Components

### 3.1 `notebook_kernels` mapping (migration, append-only)
```sql
CREATE TABLE notebook_kernels (
    notebook_id  uuid NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kernel_id    text NOT NULL,                 -- the gateway's kernel id
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (notebook_id, user_id)
);
```
A `NotebookKernelStore` (in egret-kernel): `get(notebook_id, user_id)`, `upsert(notebook_id, user_id, kernel_id)`, `touch`, `delete`. The kernel_id is the gateway-assigned id (regenerable — if the gateway restarts, the cached id is stale and gets re-created).

### 3.2 Gateway HTTP client (`src/gateway.rs`)
A thin reqwest client to a running gateway:
- `ensure_kernel(gateway_url: &str, existing: Option<&str>) -> Result<String>` — if `existing` is Some, `GET <gw>/api/kernels/{id}`; if 200 (alive) reuse it; else (404/dead) fall through. Create: `POST <gw>/api/kernels` (body `{}` or `{"name":"python3"}`) → parse `{"id": "..."}` → return it.
- `delete_kernel(gateway_url, kernel_id)` — `DELETE <gw>/api/kernels/{id}` (best-effort).
Errors (gateway unreachable, bad status) → a clean `KernelError`. No panic.

### 3.3 `KernelProxy` trait + `BackendWsProxy` (`src/proxy.rs`)
```rust
#[async_trait]
pub trait KernelProxy: Send + Sync {
    /// Proxy the upgraded client WebSocket to the gateway's kernel channels,
    /// pumping messages both ways until either side closes. `on_activity` is
    /// called per message so the manager can touch the kernel.
    async fn proxy_channels(
        &self,
        client: axum::extract::ws::WebSocket,
        gateway_url: &str,
        kernel_id: &str,
        on_activity: &(dyn Fn() + Send + Sync),
    ) -> Result<(), KernelError>;
}
```
- `BackendWsProxy`: builds the gateway channels URL (`ws://{host}/api/kernels/{kernel_id}/channels` from the http(s) gateway_url), connects via `tokio-tungstenite`, then runs two pump tasks (client→gateway, gateway→client) with `tokio::select!` — when either closes/errors, both stop. Binary + text frames forwarded verbatim (the Jupyter protocol is opaque to egret). `on_activity()` fired on each forwarded message.
- Behind the trait so a future `ChpProxy` (route-table) can replace it without touching the handler.

### 3.4 Connect handler
`POST /api/ws/{ws}/notebooks/{nb}/kernel/connect` — gate `notebook:read`; the notebook must belong to the caller (own or public — reuse the notebook read-scope check; a kernel is the caller's own regardless). Flow:
1. `manager.ensure_running(key, env)` (D1b) — build the env via `build_notebook_runtime` + `manifest_to_env` (F1, as D1b's start does). If phase is `Spawning`/not-Ready, return `{phase: "spawning"}` (200) so the FE polls; do NOT create a kernel against a not-ready gateway.
2. When Ready: `let existing = store.get(nb, user); let kid = gateway.ensure_kernel(&url, existing.as_deref()).await?; store.upsert(nb, user, &kid);` 
3. Return `{ phase: "ready", kernel_id: kid }`.

### 3.5 WS channels handler
`GET /api/ws/{ws}/notebooks/{nb}/kernel/channels` — an axum `WebSocketUpgrade`. Because WS upgrades carry cookies, authenticate via the **session cookie** (extract + validate the session like the normal session extractor; reject the upgrade with 401/403 if unauthenticated or not a workspace member with `notebook:read`). Then:
1. Resolve the gateway (`manager.status(key)` must be Ready; else close with a policy error — the FE should have connected first).
2. Resolve the kernel_id (`store.get(nb, user)`; if missing, `gateway.ensure_kernel` + upsert — so the WS is self-sufficient even if connect was skipped).
3. `ws.on_upgrade(move |socket| proxy.proxy_channels(socket, &url, &kid, &on_activity))` where `on_activity` does `manager.touch(key)` + `store.touch(nb, user)` (throttled — touch at most every N seconds to avoid a DB write per message).

### 3.6 Crate boundaries
All in `egret-kernel` (it already owns the kernel lifecycle). `KernelState` (D1b) gains the `NotebookKernelStore` + a `proxy: Arc<dyn KernelProxy>` + a gateway client (or the gateway client is a free fn). The connect + channels routes are added to `kernel_router()`. egret-server already mounts `kernel_router`; egret-bin constructs the proxy (`Arc::new(BackendWsProxy)`). Add the axum `ws` feature + `tokio-tungstenite` to egret-kernel's Cargo.toml.

## 4. Data flow
FE opens a notebook → `POST .../kernel/connect` (FE polls until `{phase: ready, kernel_id}`) → FE opens WS `.../kernel/channels` → backend authenticates + resolves kernel_id + gateway_url → `BackendWsProxy` connects to `<gw>/api/kernels/{kid}/channels` + pumps both ways → Jupyter messages (execute_request ↑, iopub/status/execute_result ↓) flow through → (rendering = E1). Each message touches the kernel so the reaper keeps it alive.

## 5. Auth / security
- Connect: `require_workspace(ctx, "notebook:read")`; the kernel/notebook is the caller's own (key from `ctx.principal.user_id` + workspace; the notebook_id is verified to belong to the workspace — reuse the notebook read-scope so a user can't connect a kernel to another workspace's/private notebook they can't read).
- WS upgrade: authenticate from the session cookie (no body); same `notebook:read` + own/readable-notebook gate BEFORE upgrading. A user can only proxy to **their own** kernel (kernel_id resolved from `notebook_kernels` keyed by their user_id) — no way to address another user's kernel.
- The proxy forwards opaque frames; egret does not interpret/modify Jupyter messages. The gateway is localhost-only (subprocess), not exposed.

## 6. Error handling
- Gateway not Ready on connect → `{phase:"spawning"}` (FE polls), not an error. On the WS, not-Ready → close with a clean code (FE re-runs connect).
- Gateway unreachable / kernel-create fails → clean `KernelError` → 502/503 (connect) or WS close (channels), no panic.
- Stale kernel_id (gateway restarted) → `ensure_kernel`'s GET returns non-200 → create a fresh kernel + upsert. Self-healing.
- Proxy: either side closing ends both pumps cleanly; a gateway error closes the client WS with a status.

## 7. Testing
- **All proxy/connect logic in Rust with FAKES (no Python):**
  - A **fake gateway HTTP server** (a tiny axum/tokio server) answering `GET/POST/DELETE /api/kernels` → test `ensure_kernel` (create returns an id; existing-alive reuses; existing-dead recreates) + `delete_kernel`.
  - A **fake WS echo server** (tokio-tungstenite) as the gateway channels endpoint → test `BackendWsProxy.proxy_channels` round-trips a text + a binary frame both ways (client→echo→client) and that `on_activity` fires per message; closing one side ends the proxy.
  - Connect handler (`#[sqlx::test]` + FakeLauncher gateway + the fake gateway HTTP): `{phase:"spawning"}` when not ready; `{phase:"ready", kernel_id}` when ready + upserts `notebook_kernels`; `notebook:read` gate (non-member 403); per-user kernel keying.
- **End-to-end** (real Jupyter Kernel Gateway + execute) = **D1a-dependent**, documented; not in CI.
- **Local CI** before merge; **append-only** migrations.

## 8. Build decomposition (one spec)
Migration `notebook_kernels` + store · gateway HTTP client (ensure/delete kernel) · `KernelProxy` trait + `BackendWsProxy` (+ fake-echo test) · connect handler · WS channels handler (session-cookie auth + upgrade + touch) · axum `ws` + `tokio-tungstenite` deps · wire into `KernelState`/router/egret-bin.

## 9. Open questions (resolve in planning)
1. Touch throttling on the WS: touch at most every ~15s (a clock per connection) vs every message. *Lean: throttle to ~15s to avoid a DB write per keystroke-output.*
2. WS-upgrade auth extraction: reuse the existing session extractor, or a small bespoke extractor on the upgrade request (cookies). *Lean: reuse the session validation the normal extractor uses; read how WsContext/session auth works on a non-WS request and apply the same to the upgrade.*
3. Gateway channels URL scheme: `ws://` from `http://` gateway_url (subprocess is plain http/ws). Confirm the host:port mapping (gateway_url already is `http://127.0.0.1:{port}`).
