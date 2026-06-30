# Notebook D1b — Kernel Launcher + Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A new `egret-kernel` crate that spawns/tracks one Jupyter Kernel Gateway per (user×workspace), with start/status/stop APIs, an idle reaper, and F1-manifest env injection — all lifecycle logic testable in Rust via a fake launcher.

**Architecture:** A pluggable `KernelLauncher` trait (subprocess impl + fake impl), a `kernels` registry table, a `KernelManager` orchestrating them, lifecycle routes on the session plane, and a reaper. The kernel-start path reuses F1's `build_notebook_runtime` to assemble the env. No WS proxy / execution (D1c/E1).

**Tech Stack:** Rust (axum, sqlx, async-trait, tokio process), Postgres.

**Spec:** `docs/en/specs/2026-06-30-notebook-d1b-kernel-launcher-design.md`

**Decisions:** kernel per (user×workspace); Jupyter Kernel Gateway mechanism; subprocess launcher MVP; **append-only** migrations; all lifecycle tests use `FakeLauncher` (no Python).

---

## File structure
**New crate `crates/egret-kernel/`:** `Cargo.toml`, `src/lib.rs`, `src/launcher.rs` (trait + types + FakeLauncher), `src/subprocess.rs` (SubprocessLauncher), `src/registry.rs` (KernelRegistry store), `src/manager.rs` (KernelManager), `src/routes.rs` (KernelState + kernel_router + handlers), `src/reaper.rs` (idle reaper), `src/error.rs`.
**Edits:** `migrations/0041_kernels.sql` (new), `crates/egret-metastore/src/lib.rs` (re-embed), root `Cargo.toml` (member alias), `crates/egret-notebook/src/runtime.rs` (extract `build_notebook_runtime`), `crates/egret-server/{Cargo.toml,src/lib.rs}` (mount), `crates/egret-bin/src/main.rs` (construct + reaper + startup sweep).

---

## Task 1: `kernels` migration + KernelRegistry store (TDD)

**Files:** Create `migrations/0041_kernels.sql`; scaffold `crates/egret-kernel` (Cargo.toml, lib.rs, registry.rs, error.rs); modify metastore lib.rs + root Cargo.toml.

- [ ] **Step 1: Migration** `migrations/0041_kernels.sql` (append-only):

```sql
CREATE TABLE kernels (
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    gateway_url  text NOT NULL DEFAULT '',
    pid          integer,
    phase        text NOT NULL DEFAULT 'spawning',  -- spawning | ready | dead
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, workspace_id)
);
CREATE INDEX kernels_idle_idx ON kernels (phase, last_used_at);
```
Touch `crates/egret-metastore/src/lib.rs` header to re-embed `0041_kernels`. Run `sqlx migrate run --source migrations` (append-only — no recreate).

- [ ] **Step 2: Crate scaffold** — `crates/egret-kernel/Cargo.toml` (mirror egret-notebook deps: egret-core, egret-metastore, egret-auth, egret-workspace, egret-notebook, axum, sqlx, serde, serde_json, uuid, chrono, thiserror, tracing, async-trait, tokio; dev: tower, http-body-util). Add `egret-kernel = { path = "crates/egret-kernel" }` under root `[workspace.dependencies]`. `src/lib.rs`: `pub mod error; pub mod registry;` (add the rest as tasks create them). `src/error.rs`: `KernelError`/`KernelApiError` mirroring `NotebookApiError` (read egret-notebook/src/error.rs).

- [ ] **Step 3: Failing test** in `src/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;
    async fn seed(db: &Db) -> (uuid::Uuid, uuid::Uuid) { /* insert a user + workspace; mirror egret-notebook store tests */ unimplemented!() }

    #[sqlx::test(migrations = "../../migrations")]
    async fn upsert_touch_delete_and_list_idle(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let (ws, user) = seed(&db).await;
        let key = KernelKey { user_id: user, workspace_id: ws };
        reg.upsert(key, "http://127.0.0.1:9999", Some(1234), "ready").await.unwrap();
        let row = reg.get(key).await.unwrap().unwrap();
        assert_eq!(row.phase, "ready");
        assert_eq!(row.gateway_url, "http://127.0.0.1:9999");
        // touch bumps last_used_at; list_idle(now) should then NOT include it
        reg.touch(key).await.unwrap();
        let idle_now = reg.list_idle(chrono::Utc::now() - chrono::Duration::minutes(30)).await.unwrap();
        assert!(idle_now.iter().all(|k| *k != key));
        reg.delete(key).await.unwrap();
        assert!(reg.get(key).await.unwrap().is_none());
    }
}
```

- [ ] **Step 4: Run — fails** `cargo test -p egret-kernel` → FAIL.

- [ ] **Step 5: Implement** `src/registry.rs`: `KernelKey { user_id, workspace_id }` (Copy, Eq), `KernelRow { gateway_url, pid: Option<i32>, phase: String, last_used_at }`, `KernelRegistry { db: Db }` with `upsert(key, url, pid, phase)` (INSERT … ON CONFLICT (user_id,workspace_id) DO UPDATE SET gateway_url, pid, phase, last_used_at=now()), `get(key)`, `set_phase(key, phase)`, `touch(key)`, `delete(key)`, `list_idle(before) -> Vec<KernelKey>` (`SELECT user_id, workspace_id FROM kernels WHERE last_used_at < $1`). Parameterized. Implement `seed()` by inserting a user + workspace (read egret-notebook store tests for the exact INSERTs).

- [ ] **Step 6: Run — passes**; `cargo clippy -p egret-kernel --all-targets -- -D warnings`; `cargo fmt -p egret-kernel`.

- [ ] **Step 7: Commit** `git add crates/egret-kernel migrations/0041_kernels.sql crates/egret-metastore/src/lib.rs Cargo.toml && git commit -m "feat(kernel): kernels registry table + store"`

---

## Task 2: `KernelLauncher` trait + types + FakeLauncher

**Files:** Create `crates/egret-kernel/src/launcher.rs`; modify lib.rs.

- [ ] **Step 1: Implement** `src/launcher.rs`:

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use crate::error::KernelError;
use crate::registry::KernelKey;

pub type KernelEnv = HashMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelPhase { Spawning, Ready, Dead }
impl KernelPhase {
    pub fn as_str(self) -> &'static str { match self { Self::Spawning=>"spawning", Self::Ready=>"ready", Self::Dead=>"dead" } }
}

#[derive(Debug, Clone)]
pub struct KernelHandle { pub url: String, pub phase: KernelPhase, pub pid: Option<i32> }

#[async_trait]
pub trait KernelLauncher: Send + Sync {
    /// Spawn (or no-op if already live) the kernel for `key` with `env`; block
    /// until ready or `timeout`. Returns the gateway URL + phase.
    async fn ensure(&self, key: KernelKey, env: &KernelEnv, timeout: std::time::Duration) -> Result<KernelHandle, KernelError>;
    async fn status(&self, key: KernelKey) -> Result<KernelPhase, KernelError>;
    async fn stop(&self, key: KernelKey) -> Result<(), KernelError>;
    fn mode(&self) -> &'static str;
}

/// Test-only launcher: records calls, returns a configured phase + dummy url, no process.
#[derive(Default)]
pub struct FakeLauncher {
    pub inner: std::sync::Mutex<FakeState>,
}
#[derive(Default)]
pub struct FakeState { pub ensured: Vec<(KernelKey, KernelEnv)>, pub stopped: Vec<KernelKey>, pub phase: Option<KernelPhase> }

#[async_trait]
impl KernelLauncher for FakeLauncher {
    async fn ensure(&self, key: KernelKey, env: &KernelEnv, _t: std::time::Duration) -> Result<KernelHandle, KernelError> {
        let mut g = self.inner.lock().unwrap();
        g.ensured.push((key, env.clone()));
        let phase = g.phase.unwrap_or(KernelPhase::Ready);
        Ok(KernelHandle { url: format!("http://fake/{}", key.user_id), phase, pid: Some(1) })
    }
    async fn status(&self, _key: KernelKey) -> Result<KernelPhase, KernelError> { Ok(self.inner.lock().unwrap().phase.unwrap_or(KernelPhase::Ready)) }
    async fn stop(&self, key: KernelKey) -> Result<(), KernelError> { self.inner.lock().unwrap().stopped.push(key); Ok(()) }
    fn mode(&self) -> &'static str { "fake" }
}
```
Add `pub mod launcher;` to lib.rs. `cargo build -p egret-kernel` → PASS. (No test step here — exercised via the manager/route tests in Tasks 4/6.)

- [ ] **Step 2: Commit** `git add crates/egret-kernel/src && git commit -m "feat(kernel): KernelLauncher trait + types + FakeLauncher"`

---

## Task 3: SubprocessLauncher (TDD the pure parts)

**Files:** Create `crates/egret-kernel/src/subprocess.rs`; modify lib.rs.

- [ ] **Step 1: Failing test** for the pure helpers (port + argv), in subprocess.rs:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn free_port_is_bindable() { let p = pick_free_port().unwrap(); assert!(p > 0); /* can bind again */ assert!(std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()); }
    #[test]
    fn argv_targets_localhost_and_port() {
        let argv = gateway_argv(54321);
        assert!(argv.iter().any(|a| a.contains("kernelgateway")));
        assert!(argv.iter().any(|a| a == "--KernelGatewayApp.ip=127.0.0.1"));
        assert!(argv.iter().any(|a| a == "--KernelGatewayApp.port=54321"));
    }
}
```

- [ ] **Step 2: Run — fails** → FAIL.

- [ ] **Step 3: Implement** `src/subprocess.rs`:
  - `pub fn pick_free_port() -> Result<u16, KernelError>` — bind `TcpListener::bind(("127.0.0.1", 0))`, read `.local_addr()?.port()`, drop the listener, return the port.
  - `pub fn gateway_argv(port: u16) -> Vec<String>` — `["jupyter","kernelgateway", "--KernelGatewayApp.ip=127.0.0.1", format!("--KernelGatewayApp.port={port}"), "--KernelGatewayApp.allow_origin=*"]`.
  - `struct SubprocessLauncher { children: tokio::sync::Mutex<HashMap<KernelKey,(tokio::process::Child,String)>> }`.
  - `ensure`: if a live child exists for key → return its url/Ready; else pick port, build url `http://127.0.0.1:{port}`, `tokio::process::Command::new(argv[0]).args(&argv[1..]).envs(env).spawn()?`, store the child, then poll `GET {url}/api` (reqwest) every 300ms until 200 or `timeout` → Ready; on timeout kill + Err. Map spawn errors (e.g. binary not found) to `KernelError::Spawn`.
  - `status`: child present + not exited → Ready, else Dead.
  - `stop`: `child.start_kill()` + remove from map.
  - `mode() = "subprocess"`.
  > The real `jupyter kernelgateway` spawn is NOT unit-tested (needs Python). Add `#[ignore]` on any test that would spawn it; the pure helpers (port/argv) ARE tested. Document that end-to-end is verified once D1a's image exists.

- [ ] **Step 4: Run — passes** `cargo test -p egret-kernel`; clippy clean (add `reqwest` to Cargo.toml deps).

- [ ] **Step 5: Commit** `git add crates/egret-kernel/src/subprocess.rs crates/egret-kernel/src/lib.rs crates/egret-kernel/Cargo.toml && git commit -m "feat(kernel): subprocess launcher (jupyter kernelgateway)"`

---

## Task 4: KernelManager (TDD with FakeLauncher)

**Files:** Create `crates/egret-kernel/src/manager.rs`; modify lib.rs.

- [ ] **Step 1: Failing test** (`#[sqlx::test]`, FakeLauncher):

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn ensure_running_spawns_then_reuses(pool: sqlx::PgPool) {
    let db = Db { pool };
    let reg = KernelRegistry::new(db.clone());
    let launcher = std::sync::Arc::new(FakeLauncher::default());
    let mgr = KernelManager::new(reg, launcher.clone(), std::time::Duration::from_secs(1800));
    let (ws, user) = seed_kernel(&db).await; // user + workspace
    let key = KernelKey { user_id: user, workspace_id: ws };
    let env = KernelEnv::new();
    let s1 = mgr.ensure_running(key, &env).await.unwrap();
    assert_eq!(s1.phase, KernelPhase::Ready);
    let s2 = mgr.ensure_running(key, &env).await.unwrap();
    // second call reused the registry row (launcher.ensure called once more is acceptable,
    // but the registry has exactly one row and phase Ready)
    assert_eq!(s2.phase, KernelPhase::Ready);
    mgr.stop(key).await.unwrap();
    assert!(launcher.inner.lock().unwrap().stopped.contains(&key));
}
```

- [ ] **Step 2: Run — fails** → FAIL.

- [ ] **Step 3: Implement** `src/manager.rs`: `KernelManager { registry: KernelRegistry, launcher: Arc<dyn KernelLauncher>, idle_timeout: Duration }` with:
  - `ensure_running(key, env) -> Result<KernelStatus>`: `if let Some(row) = registry.get(key)` and `row.phase=="ready"` and `launcher.status(key)==Ready` → `registry.touch(key)`; return `{phase: Ready, url: row.gateway_url}`. Else `let h = launcher.ensure(key, env, /* a fixed 90s */).await?; registry.upsert(key, &h.url, h.pid, h.phase.as_str()); Ok({phase:h.phase, url:h.url})`.
  - `status(key) -> Option<KernelStatus>` (from registry, no spawn).
  - `stop(key)`: `launcher.stop(key)?; registry.delete(key)`.
  - `touch(key)`: `registry.touch(key)`.
  - `KernelStatus { phase: KernelPhase, url: String }` serializes for the API.

- [ ] **Step 4: Run — passes**; clippy clean.

- [ ] **Step 5: Commit** `git add crates/egret-kernel/src/manager.rs crates/egret-kernel/src/lib.rs && git commit -m "feat(kernel): KernelManager (ensure/status/stop/touch)"`

---

## Task 5: Extract F1 `build_notebook_runtime` + env serialization

**Files:** Modify `crates/egret-notebook/src/runtime.rs`; create `crates/egret-kernel/src/env.rs` (or a fn in routes.rs).

- [ ] **Step 1: Refactor F1** — in runtime.rs, extract steps 2–6 of the `runtime` handler into:

```rust
pub async fn build_notebook_runtime(
    st: &NotebookState, ws: uuid::Uuid, user_id: egret_core::UserId,
) -> Result<NotebookRuntime, NotebookApiError> { /* the existing steps 2..6 body */ }
```
and make the HTTP `runtime` handler call `let manifest = build_notebook_runtime(&st, ctx.workspace_id.0, ctx.principal.user_id).await?;` after its `require_workspace` check. Re-run `cargo test -p egret-notebook` → the existing runtime tests still pass (behavior unchanged).

- [ ] **Step 2: Env serialization** — in egret-kernel, a fn `manifest_to_env(m: &NotebookRuntime, api_url: &str, kernel_token: &str) -> KernelEnv`:

```rust
let mut env = KernelEnv::new();
env.insert("EGRET_NOTEBOOK_RUNTIME".into(), serde_json::to_string(m).unwrap());
env.insert("EGRET_API_URL".into(), api_url.to_string());
env.insert("EGRET_KERNEL_TOKEN".into(), kernel_token.to_string());
env
```
(`NotebookRuntime` already derives Serialize.) For D1b, `kernel_token` is a placeholder/empty (the callback endpoints are D1c/E1) — pass `""` and note it. Test: `manifest_to_env` produces a map whose `EGRET_NOTEBOOK_RUNTIME` parses back to a JSON object with the username + namespace.

- [ ] **Step 3: Run — passes** `cargo test -p egret-notebook -p egret-kernel`; clippy clean.

- [ ] **Step 4: Commit** `git add crates/egret-notebook/src/runtime.rs crates/egret-kernel/src && git commit -m "feat(kernel): reuse F1 build_notebook_runtime + serialize manifest to kernel env"`

---

## Task 6: Lifecycle routes + KernelState (TDD http with FakeLauncher)

**Files:** Create `crates/egret-kernel/src/routes.rs`; modify lib.rs; create `crates/egret-kernel/tests/http.rs`.

- [ ] **Step 1: Failing http test** (`#[sqlx::test]`, authenticated like egret-notebook tests/http.rs; build `KernelState` with a `FakeLauncher` + a `NotebookState`-like handle to build the manifest). Cover: `POST /ws/{ws}/kernel` → 200 `{phase:"ready", url}`; the FakeLauncher received an env containing `EGRET_NOTEBOOK_RUNTIME`; `GET` → 200 status; `DELETE` → 204 and the registry row is gone; a NON-member → 403/404; user A cannot see user B's kernel (each POST/GET is keyed by the caller's principal).

- [ ] **Step 2: Run — fails** → FAIL.

- [ ] **Step 3: Implement** `src/routes.rs`: `KernelState { manager: KernelManager, notebook: NotebookState, sessions, workspaces, authz, api_url: String }` (+ FromRef for the extractor types). `kernel_router()`:
```rust
Router::new()
  .route("/ws/{workspace}/kernel", post(start).get(status).delete(stop))
```
Handlers (all `require_workspace(ctx, "notebook:read")`, key = `{ctx.principal.user_id.0, ctx.workspace_id.0}`):
  - `start`: `let m = build_notebook_runtime(&st.notebook, ws, ctx.principal.user_id).await?; let env = manifest_to_env(&m, &st.api_url, ""); let s = st.manager.ensure_running(key, &env).await?; Json(s)`.
  - `status`: `st.manager.status(key)` → 200 `{phase, url?}` or a `{phase:"dead"}`/404 when none.
  - `stop`: `st.manager.stop(key)` → 204.

- [ ] **Step 4: Run — passes** `cargo test -p egret-kernel`; clippy clean.

- [ ] **Step 5: Commit** `git add crates/egret-kernel/src/routes.rs crates/egret-kernel/src/lib.rs crates/egret-kernel/tests/http.rs && git commit -m "feat(kernel): kernel lifecycle API (start/status/stop)"`

---

## Task 7: Idle reaper (TDD with FakeLauncher)

**Files:** Create `crates/egret-kernel/src/reaper.rs`; modify lib.rs.

- [ ] **Step 1: Failing test** (`#[sqlx::test]`): seed two kernels — one with `last_used_at` 40 min ago (idle), one just touched. Run ONE reaper sweep (`reap_once(&registry, &launcher, idle_timeout=30min)`). Assert: the idle one is stopped (FakeLauncher.stopped contains it) + its registry row deleted; the fresh one is untouched.

- [ ] **Step 2: Run — fails** → FAIL.

- [ ] **Step 3: Implement** `src/reaper.rs`:
```rust
pub async fn reap_once(reg: &KernelRegistry, launcher: &Arc<dyn KernelLauncher>, idle: Duration) -> usize {
    let before = chrono::Utc::now() - chrono::Duration::from_std(idle).unwrap();
    let stale = reg.list_idle(before).await.unwrap_or_default();
    for key in &stale { let _ = launcher.stop(*key).await; let _ = reg.delete(*key).await; }
    stale.len()
}
/// Spawn the periodic reaper; returns immediately. Cancelled via the token.
pub fn spawn_reaper(reg: KernelRegistry, launcher: Arc<dyn KernelLauncher>, idle: Duration, every: Duration, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop { tokio::select! { _ = cancel.cancelled() => break, _ = tick.tick() => { reap_once(&reg, &launcher, idle).await; } } }
    });
}
```
(Use the `CancellationToken` type already used by egret-bin's gc workers — `tokio_util::sync::CancellationToken`.)

- [ ] **Step 4: Run — passes** (test calls `reap_once` directly — deterministic, no timers); clippy clean.

- [ ] **Step 5: Commit** `git add crates/egret-kernel/src/reaper.rs crates/egret-kernel/src/lib.rs && git commit -m "feat(kernel): idle kernel reaper"`

---

## Task 8: Wire egret-kernel into server + egret-bin (subprocess + reaper + startup sweep)

**Files:** Modify `crates/egret-server/{Cargo.toml,src/lib.rs}`, `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: server dep + AppState** — add `egret-kernel.workspace = true` to egret-server Cargo.toml; add `pub kernel: Option<egret_kernel::KernelState>` to `AppState` (None in the no-db ctor); in `build_router`, after the notebook block, mirror it: `.nest("/api", egret_kernel::kernel_router()).layer(csrf_layer).with_state(kernel)`.

- [ ] **Step 2: egret-bin construction** — build `KernelManager` with `Arc::new(SubprocessLauncher::default())`, the idle timeout from config (default 30 min), the `KernelRegistry::new(db.clone())`, and a `KernelState { manager, notebook: <the NotebookState>, sessions, workspaces, authz, api_url: <config> }`. Pass into `AppState.kernel`. **Startup sweep:** on boot, mark all existing `kernels` rows dead/delete (orphaned processes from a prior run are gone) — a one-shot `reg.delete_all()` or `UPDATE kernels SET phase='dead'`; simplest: `DELETE FROM kernels` at startup (the subprocess launcher can't re-adopt them). **Reaper:** `egret_kernel::spawn_reaper(registry, launcher, idle, Duration::from_secs(60), cancel)`.

- [ ] **Step 3: Build** `cargo build -p egret-bin` → PASS; clippy clean.

- [ ] **Step 4: Commit** `git add crates/egret-server crates/egret-bin && git commit -m "feat(kernel): wire kernel lifecycle + reaper into server + composition root"`

---

## Task 9: Full local CI + smoke + PR

- [ ] **Step 1: Backend** — `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p egret-kernel -p egret-notebook` (+ `cargo test --workspace --no-fail-fast` to confirm no NEW failures vs the known set). All green.
- [ ] **Step 2: Smoke** (FakeLauncher path is the realistic one without Python): rebuild + restart backend; authenticated `POST /api/ws/eng/kernel` → if the host has `jupyter kernelgateway`, it spawns + reaches ready (note the result); if not, expect a clean spawn error (503/500), NOT a panic — verifying the error path. `GET` then `DELETE`. The real-gateway end-to-end is D1a-dependent (documented).
- [ ] **Step 3:** Push branch + open PR; admin-squash-merge after green (GitHub Actions out of quota).

---

## Self-review notes (author)
- Spec coverage: registry+migration (T1), launcher trait+fake (T2), subprocess (T3), manager (T4), F1 manifest reuse+env (T5), lifecycle API own-kernel+gate (T6), reaper (T7), wiring + startup sweep (T8), CI+smoke (T9). ✔
- Type consistency: `KernelKey{user_id,workspace_id}`, `KernelPhase`, `KernelHandle{url,phase,pid}`, `KernelEnv`, `KernelManager::ensure_running/status/stop/touch`, `KernelStatus{phase,url}`, `build_notebook_runtime(st,ws,user_id)`, `manifest_to_env`, `reap_once`/`spawn_reaper` align across tasks. ✔
- Verify-points: egret-notebook store-test seed pattern, NotebookState shape (for KernelState), egret-bin construction (sessions/workspaces/authz/notebook handles), CancellationToken type — all "read first". ✔
- Migrations append-only (0041 adds a table) — no dev-DB recreate. All lifecycle tests use FakeLauncher (no Python). The subprocess real-gateway spawn is documented as D1a-dependent. ✔
