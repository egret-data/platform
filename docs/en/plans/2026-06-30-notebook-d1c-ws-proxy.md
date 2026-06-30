# Notebook D1c — Kernel WS Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Per-notebook kernel resolution + a backend WebSocket proxy of the Jupyter channels protocol, so the browser can hold a live WS to a kernel inside the (user×workspace) gateway. (Output rendering + Run wiring = E1.)

**Architecture:** A `notebook_kernels` mapping + a gateway HTTP client (create/verify a kernel in a running gateway) + a `KernelProxy` trait with a `BackendWsProxy` impl (axum WS upgrade ↔ tokio-tungstenite to the gateway, pumped both ways, touch-throttled). A `connect` REST ensures the gateway (D1b) + kernel_id; a `channels` WS endpoint proxies. All extend the existing `egret-kernel` crate.

**Tech Stack:** Rust (axum 0.8 `ws` feature, tokio-tungstenite, reqwest, sqlx), Postgres.

**Spec:** `docs/en/specs/2026-06-30-notebook-d1c-ws-proxy-design.md`

**Decisions:** per-notebook kernel (notebook_kernels); sparklabx connect flow; proxy-in-backend behind a `KernelProxy` trait (CHP-style is the documented scale path); WS-upgrade auth reuses `WsContext`; touch throttled ~15s; **append-only** migrations; all proxy/connect logic tested with fakes (no Python).

---

## File structure (all in `crates/egret-kernel/`)
- `migrations/0042_notebook_kernels.sql` (new) + metastore re-embed.
- `src/notebook_kernels.rs` (new) — `NotebookKernelStore`.
- `src/gateway.rs` (new) — `ensure_kernel` / `delete_kernel` HTTP client.
- `src/proxy.rs` (new) — `KernelProxy` trait + `BackendWsProxy`.
- `src/routes.rs` (modify) — `KernelState` gains `notebook_kernels` + `proxy`; add `connect` + `channels` routes + handlers.
- `src/lib.rs` (modify) — module decls + re-exports.
- `Cargo.toml` (modify) — `axum` `ws` feature, `tokio-tungstenite`, `futures-util`.
- `crates/egret-bin/src/main.rs` (modify) — construct `BackendWsProxy` + `NotebookKernelStore` into `KernelState`.

---

## Task 1: `notebook_kernels` mapping + store (TDD)

**Files:** Create `migrations/0042_notebook_kernels.sql`, `crates/egret-kernel/src/notebook_kernels.rs`; modify metastore lib.rs + egret-kernel lib.rs.

- [ ] **Step 1: Migration** `migrations/0042_notebook_kernels.sql` (append-only):
```sql
CREATE TABLE notebook_kernels (
    notebook_id  uuid NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kernel_id    text NOT NULL,
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (notebook_id, user_id)
);
```
Touch `crates/egret-metastore/src/lib.rs` header to re-embed 0042. Run `sqlx migrate run --source migrations` (append-only).

- [ ] **Step 2: Failing test** in `notebook_kernels.rs` (`#[sqlx::test(migrations="../../migrations")]`, seed a user + workspace + a notebook — read how egret-notebook store tests insert a notebook, or insert one directly: `INSERT INTO notebooks (workspace_id, owner_user_id, name) VALUES ($1,$2,'nb') RETURNING id`):
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn upsert_get_touch_delete(pool: sqlx::PgPool) {
    let db = Db { pool };
    let store = NotebookKernelStore::new(db.clone());
    let (nb, user) = seed_nb(&db).await;
    assert!(store.get(nb, user).await.unwrap().is_none());
    store.upsert(nb, user, "kid-1").await.unwrap();
    assert_eq!(store.get(nb, user).await.unwrap().unwrap(), "kid-1");
    store.upsert(nb, user, "kid-2").await.unwrap();   // re-bind
    assert_eq!(store.get(nb, user).await.unwrap().unwrap(), "kid-2");
    store.touch(nb, user).await.unwrap();
    store.delete(nb, user).await.unwrap();
    assert!(store.get(nb, user).await.unwrap().is_none());
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-kernel notebook_kernels` → FAIL.

- [ ] **Step 4: Implement** `NotebookKernelStore { db: Db }`: `get(notebook_id, user_id) -> Result<Option<String>, KernelError>` (`SELECT kernel_id`), `upsert(notebook_id, user_id, kernel_id) -> Result<()>` (`INSERT … ON CONFLICT (notebook_id,user_id) DO UPDATE SET kernel_id=$3, last_used_at=now()`), `touch(notebook_id, user_id)` (`UPDATE … SET last_used_at=now()`), `delete(notebook_id, user_id)`. Derive `Clone`. Parameterized. Add `pub mod notebook_kernels; pub use ...` to lib.rs.

- [ ] **Step 5: Run — passes**; clippy clean; fmt.

- [ ] **Step 6: Commit** `git add migrations/0042_notebook_kernels.sql crates/egret-metastore/src/lib.rs crates/egret-kernel/src/notebook_kernels.rs crates/egret-kernel/src/lib.rs && git commit -m "feat(kernel): notebook_kernels mapping (per-notebook kernel id)"`

---

## Task 2: Gateway HTTP client (TDD with a fake gateway)

**Files:** Create `crates/egret-kernel/src/gateway.rs`; modify lib.rs.

- [ ] **Step 1: Failing test** — spin a tiny axum fake gateway in-test serving the Jupyter Kernel Gateway kernels API, then test the client:
```rust
#[tokio::test]
async fn ensure_kernel_creates_then_reuses() {
    // fake gateway: POST /api/kernels -> {"id":"k1"} ; GET /api/kernels/k1 -> 200 ; GET /api/kernels/dead -> 404
    let (base, _shutdown) = spawn_fake_gateway().await;       // returns "http://127.0.0.1:{port}"
    // no existing -> create
    let k = ensure_kernel(&base, None).await.unwrap();
    assert_eq!(k, "k1");
    // existing alive -> reuse
    let k2 = ensure_kernel(&base, Some("k1")).await.unwrap();
    assert_eq!(k2, "k1");
    // existing dead -> recreate
    let k3 = ensure_kernel(&base, Some("dead")).await.unwrap();
    assert_eq!(k3, "k1");
}
```
`spawn_fake_gateway` = an axum Router on `TcpListener::bind("127.0.0.1:0")` with the three routes, spawned with `tokio::spawn(axum::serve(...))`, returning the base url + a drop guard. (Write it in the test module.)

- [ ] **Step 2: Run — fails** → FAIL.

- [ ] **Step 3: Implement** `gateway.rs`:
```rust
pub async fn ensure_kernel(gateway_url: &str, existing: Option<&str>) -> Result<String, KernelError> {
    let client = reqwest::Client::new();
    if let Some(id) = existing {
        let r = client.get(format!("{gateway_url}/api/kernels/{id}")).send().await;
        if matches!(r, Ok(ref resp) if resp.status().is_success()) { return Ok(id.to_string()); }
    }
    let resp = client.post(format!("{gateway_url}/api/kernels"))
        .json(&serde_json::json!({ "name": "python3" })).send().await
        .map_err(|e| KernelError::Gateway(format!("create kernel: {e}")))?;
    if !resp.status().is_success() {
        return Err(KernelError::Gateway(format!("create kernel: HTTP {}", resp.status())));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| KernelError::Gateway(e.to_string()))?;
    v.get("id").and_then(|i| i.as_str()).map(str::to_string)
        .ok_or_else(|| KernelError::Gateway("kernel create response missing id".into()))
}
pub async fn delete_kernel(gateway_url: &str, kernel_id: &str) {
    let _ = reqwest::Client::new().delete(format!("{gateway_url}/api/kernels/{kernel_id}")).send().await;
}
```
Add a `Gateway(String)` variant to `KernelError` (error.rs) mapping to a 502/Internal API error. Add `pub mod gateway;` to lib.rs.

- [ ] **Step 4: Run — passes**; clippy clean.

- [ ] **Step 5: Commit** `git add crates/egret-kernel/src/gateway.rs crates/egret-kernel/src/error.rs crates/egret-kernel/src/lib.rs && git commit -m "feat(kernel): gateway HTTP client (ensure/delete kernel)"`

---

## Task 3: `KernelProxy` trait + `BackendWsProxy` (TDD with a fake echo WS)

**Files:** Create `crates/egret-kernel/src/proxy.rs`; modify lib.rs + Cargo.toml.

- [ ] **Step 1: Deps** — in `crates/egret-kernel/Cargo.toml`: `axum = { workspace = true, features = ["ws"] }` (or add the feature to the existing axum dep line — check how axum is declared), `tokio-tungstenite.workspace = true` (add to root `[workspace.dependencies]` if absent — a common version is `"0.24"`/`"0.26"`; match what compiles with the workspace tokio), `futures-util.workspace = true`.

- [ ] **Step 2: Failing test** — a fake echo WS server (tokio-tungstenite) standing in for the gateway channels endpoint; drive the proxy with an in-memory client pair. Because axum's `WebSocket` is hard to construct in a unit test, structure the proxy so the PUMP CORE is a standalone async fn over two generic sinks/streams that you CAN test, and `proxy_channels` adapts the axum socket to it. Test the pump core:
```rust
#[tokio::test]
async fn pump_forwards_both_ways_and_counts_activity() {
    // two duplex tungstenite-style channels via tokio::sync::mpsc emulating
    // client<->core<->gateway; send a Text + Binary each way; assert they arrive
    // on the other end and the activity counter incremented per message.
}
```
(If adapting axum's WebSocket to a generic pump is awkward, instead: spin a real fake echo WS server with tokio-tungstenite, connect to it from `BackendWsProxy` as the "gateway", and feed the "client" side via a tokio-tungstenite client too — then assert a round-trip. Pick whichever is cleaner; the REQUIREMENT is a real bidirectional-forward test + the activity callback firing, with no Python.)

- [ ] **Step 3: Run — fails** → FAIL.

- [ ] **Step 4: Implement** `proxy.rs`:
```rust
#[async_trait::async_trait]
pub trait KernelProxy: Send + Sync {
    async fn proxy_channels(
        &self,
        client: axum::extract::ws::WebSocket,
        gateway_url: &str,
        kernel_id: &str,
        on_activity: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), KernelError>;
}

pub struct BackendWsProxy;

#[async_trait::async_trait]
impl KernelProxy for BackendWsProxy {
    async fn proxy_channels(&self, client: axum::extract::ws::WebSocket, gateway_url: &str, kernel_id: &str, on_activity: std::sync::Arc<dyn Fn()+Send+Sync>) -> Result<(), KernelError> {
        use futures_util::{SinkExt, StreamExt};
        // http(s)://host -> ws(s)://host
        let ws_url = format!("{}/api/kernels/{}/channels",
            gateway_url.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1), kernel_id);
        let (gw, _) = tokio_tungstenite::connect_async(&ws_url).await
            .map_err(|e| KernelError::Gateway(format!("connect channels: {e}")))?;
        let (mut gw_tx, mut gw_rx) = gw.split();
        let (mut cl_tx, mut cl_rx) = client.split();
        let act1 = on_activity.clone();
        // client -> gateway
        let c2g = async {
            while let Some(Ok(msg)) = cl_rx.next().await {
                act1();
                let out = axum_to_tungstenite(msg);            // map Text/Binary/Close/Ping/Pong
                if let Some(out) = out { if gw_tx.send(out).await.is_err() { break; } }
            }
        };
        // gateway -> client
        let g2c = async {
            while let Some(Ok(msg)) = gw_rx.next().await {
                on_activity();
                let out = tungstenite_to_axum(msg);
                if let Some(out) = out { if cl_tx.send(out).await.is_err() { break; } }
            }
        };
        tokio::select! { _ = c2g => {}, _ = g2c => {} }
        Ok(())
    }
}
```
Write `axum_to_tungstenite` / `tungstenite_to_axum` mapping Text↔Text, Binary↔Binary, Ping/Pong↔Ping/Pong, Close↔Close (drop unknown/`Frame` variants → None). Factor the two pump loops into a testable `pub(crate) async fn pump(...)` if that makes the test cleaner. Add `pub mod proxy;` to lib.rs.

- [ ] **Step 5: Run — passes**; clippy clean; fmt.

- [ ] **Step 6: Commit** `git add crates/egret-kernel/src/proxy.rs crates/egret-kernel/src/lib.rs crates/egret-kernel/Cargo.toml Cargo.toml && git commit -m "feat(kernel): KernelProxy trait + BackendWsProxy (Jupyter channels WS proxy)"`

---

## Task 4: Connect handler (TDD http)

**Files:** Modify `crates/egret-kernel/src/routes.rs` (KernelState fields + route + handler); `tests/http.rs`.

- [ ] **Step 1: KernelState fields** — add `pub notebook_kernels: NotebookKernelStore` and `pub proxy: Arc<dyn KernelProxy>` to `KernelState`. Add the route: in `kernel_router()`, `.route("/ws/{workspace}/notebooks/{notebook}/kernel/connect", post(connect))`.

- [ ] **Step 2: Failing http test** (`#[sqlx::test]`, authenticated; build KernelState with a FakeLauncher-backed manager + a FAKE-gateway base url as the manager's launcher url + a fake gateway HTTP server + a real NotebookKernelStore; seed ws+user(member)+warehouse+notebook). Cover:
  - When the gateway is Ready (FakeLauncher returns Ready with the fake-gateway url), `POST .../notebooks/{nb}/kernel/connect` → 200 `{phase:"ready", kernel_id}` and `notebook_kernels` now has a row.
  - When FakeLauncher.phase = Spawning → 200 `{phase:"spawning"}` and NO kernel created (gateway HTTP not hit / no notebook_kernels row).
  - Non-member → 403.
  (To make ensure_running return the fake-gateway url, the FakeLauncher's `ensure` returns a `KernelHandle{url}` — set its url to the fake gateway base. Check FakeLauncher returns `http://fake/{user}`; you may need a configurable-url fake or point ensure_kernel at a fake server keyed off that. Simplest: extend FakeLauncher to return a settable url, OR run the fake gateway and have the test's manager use a launcher that returns that url. Adapt cleanly.)

- [ ] **Step 3: Run — fails** → FAIL.

- [ ] **Step 4: Implement** `connect` handler:
```rust
async fn connect(State(st): State<KernelState>, WsContext(ctx): WsContext, Path((_ws, nb)): Path<(String, Uuid)>) -> Result<Json<serde_json::Value>, KernelApiError> {
    st.authz.require_workspace(&ctx, "notebook:read").await.map_err(...)?;
    // (optional: verify nb belongs to the workspace + readable — reuse a notebook read-scope check if available; else trust notebook:read + the ws scope)
    let key = KernelKey { user_id: ctx.principal.user_id.0, workspace_id: ctx.workspace_id.0 };
    let manifest = build_notebook_runtime(&st.notebook, ctx.workspace_id.0, ctx.principal.user_id).await?;
    let env = manifest_to_env(&manifest, &st.api_url, "");
    let status = st.manager.ensure_running(key, &env).await?;
    if status.phase != KernelPhase::Ready {
        return Ok(Json(serde_json::json!({ "phase": status.phase.as_str() })));
    }
    let existing = st.notebook_kernels.get(nb, ctx.principal.user_id.0).await?;
    let kid = egret_kernel_gateway_ensure(&status.url, existing.as_deref()).await?; // crate::gateway::ensure_kernel
    st.notebook_kernels.upsert(nb, ctx.principal.user_id.0, &kid).await?;
    Ok(Json(serde_json::json!({ "phase": "ready", "kernel_id": kid })))
}
```

- [ ] **Step 5: Run — passes**; clippy clean.

- [ ] **Step 6: Commit** `git add crates/egret-kernel/src/routes.rs crates/egret-kernel/tests/http.rs && git commit -m "feat(kernel): kernel connect endpoint (resolve per-notebook kernel id)"`

---

## Task 5: WS channels handler

**Files:** Modify `crates/egret-kernel/src/routes.rs`.

- [ ] **Step 1: Route + handler** — add `.route("/ws/{workspace}/notebooks/{notebook}/kernel/channels", get(channels))`. Handler uses `WsContext` (authenticates the session cookie + workspace membership on the upgrade GET) + `WebSocketUpgrade`:
```rust
async fn channels(State(st): State<KernelState>, WsContext(ctx): WsContext, Path((_ws, nb)): Path<(String, Uuid)>, upgrade: axum::extract::ws::WebSocketUpgrade) -> Result<axum::response::Response, KernelApiError> {
    st.authz.require_workspace(&ctx, "notebook:read").await.map_err(...)?;
    let key = KernelKey { user_id: ctx.principal.user_id.0, workspace_id: ctx.workspace_id.0 };
    let status = st.manager.status(key).await?.ok_or_else(/* Conflict: "kernel not started; call connect first" */)?;
    if status.phase != KernelPhase::Ready { return Err(/* Conflict */); }
    // resolve kernel_id (self-heal if connect was skipped)
    let existing = st.notebook_kernels.get(nb, ctx.principal.user_id.0).await?;
    let kid = crate::gateway::ensure_kernel(&status.url, existing.as_deref()).await?;
    st.notebook_kernels.upsert(nb, ctx.principal.user_id.0, &kid).await?;
    let st2 = st.clone(); let user = ctx.principal.user_id.0; let url = status.url.clone();
    // throttled touch: shared Instant guarded by a Mutex; only touch DB/manager every ~15s
    let last = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(60)));
    let on_activity: std::sync::Arc<dyn Fn()+Send+Sync> = std::sync::Arc::new(move || {
        let mut g = last.lock().unwrap();
        if g.elapsed() >= std::time::Duration::from_secs(15) {
            *g = std::time::Instant::now();
            let (st3, kk, nb2, u2) = (st2.clone(), key, nb, user);
            tokio::spawn(async move { let _ = st3.manager.touch(kk).await; let _ = st3.notebook_kernels.touch(nb2, u2).await; });
        }
    });
    let proxy = st.proxy.clone();
    Ok(upgrade.on_upgrade(move |socket| async move {
        let _ = proxy.proxy_channels(socket, &url, &kid, on_activity).await;
    }))
}
```
> Note: the WS upgrade is a `GET`, so the CSRF layer (which gates mutating methods) lets it through — verify `egret_auth::csrf::csrf_layer` only checks POST/PUT/PATCH/DELETE. If it blocks the GET upgrade, mount the channels route on a sub-router WITHOUT the csrf layer (WS upgrades are not CSRF-able the same way — the session cookie + same-origin check suffice; document the choice).

- [ ] **Step 2: Build + a minimal test** — `cargo build -p egret-kernel`. A full WS round-trip through the real handler needs a live test server; the proxy pump is already unit-tested (T3) and connect (T4). Add at least a test that the `channels` route returns 401/403 for a non-member (the upgrade is rejected before proxying) and a 409/425 when the kernel is not started (status None). (These don't need a real WS client — assert the HTTP status of the upgrade request without the Upgrade headers, which axum rejects as 400/426, OR with WsContext failing first → 401/403. Focus on the auth/precondition gates.)

- [ ] **Step 3: Run — passes** `cargo test -p egret-kernel`; clippy clean.

- [ ] **Step 4: Commit** `git add crates/egret-kernel/src/routes.rs && git commit -m "feat(kernel): WS channels proxy endpoint (auth + touch-throttled)"`

---

## Task 6: Wire KernelState (proxy + store) in egret-bin

**Files:** Modify `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Construct** — where `KernelState` is built (Task added in D1b T8), add the two new fields: `notebook_kernels: egret_kernel::NotebookKernelStore::new(db.clone())` and `proxy: std::sync::Arc::new(egret_kernel::BackendWsProxy) as std::sync::Arc<dyn egret_kernel::KernelProxy>`. (Re-export `NotebookKernelStore`, `BackendWsProxy`, `KernelProxy` from egret-kernel lib.rs.)

- [ ] **Step 2: Build** `cargo build -p egret-bin` → PASS; clippy clean.

- [ ] **Step 3: Commit** `git add crates/egret-kernel/src/lib.rs crates/egret-bin/src/main.rs && git commit -m "feat(kernel): wire BackendWsProxy + notebook_kernels into KernelState"`

---

## Task 7: Full local CI + smoke + PR

- [ ] **Step 1: Backend** — `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p egret-kernel -p egret-notebook` (+ `cargo test --workspace --no-fail-fast` to confirm no new failures). All green.
- [ ] **Step 2: Frontend** — `cd frontend && npm run typecheck && npm run build` (no FE changes in D1c — sanity).
- [ ] **Step 3: Smoke** — rebuild + restart backend; authenticated `POST /api/ws/eng/notebooks/{nb}/kernel/connect` → with no warehouse → clean 404 (no panic); the route is mounted; the WS `…/channels` route exists (a non-Upgrade GET → 400/426, not a 404). Confirm the backend doesn't crash. (Real gateway end-to-end needs D1a — documented.)
- [ ] **Step 4:** Push branch + open PR; admin-squash-merge after green (GitHub Actions out of quota).

---

## Self-review notes (author)
- Spec coverage: notebook_kernels+store (T1), gateway client (T2), KernelProxy+BackendWsProxy (T3), connect (T4), channels WS + auth + touch-throttle (T5), wiring (T6), CI+smoke (T7). ✔
- Type consistency: `NotebookKernelStore::{get,upsert,touch,delete}`, `gateway::{ensure_kernel,delete_kernel}`, `KernelProxy::proxy_channels(client, gateway_url, kernel_id, on_activity: Arc<dyn Fn()>)`, `BackendWsProxy`, KernelState{+notebook_kernels,+proxy}, `connect`/`channels` handlers align. ✔
- Verify-points (read-first): how a notebook row is inserted in tests; how axum `ws` feature is enabled on the workspace axum dep; the tokio-tungstenite version that compiles; whether csrf_layer blocks the GET upgrade (mount channels without csrf if so); FakeLauncher url configurability for the connect test. ✔
- Migrations append-only (0042 adds a table). Proxy/connect fully testable with fakes (fake gateway HTTP + fake echo WS) — no Python. Real end-to-end = D1a-dependent (documented). ✔
