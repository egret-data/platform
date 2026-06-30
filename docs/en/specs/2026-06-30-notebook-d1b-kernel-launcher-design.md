# Notebook D1b — Kernel Launcher + Lifecycle (design)

**Status:** draft · **Date:** 2026-06-30 · **Epic:** Notebooks · **Predecessors:** C1 (PR #94), F1 (PR #95, both merged)

## 1. Context & scope

D1 (kernel runtime) is decomposed into three sub-specs: **D1a** kernel image + helpers (Python/Docker), **D1b** launcher + lifecycle (Rust, *this spec*), **D1c** WS proxy (Rust). E1 (execution + output UI) follows.

**D1b builds the kernel lifecycle in Rust:** a new `egret-kernel` crate that spawns and tracks one **Jupyter Kernel Gateway** process per (user × workspace), exposes start/status/stop APIs, reaps idle kernels, and injects the F1 `/notebook-runtime` manifest into the kernel's environment at spawn. It does NOT proxy the kernel protocol or execute cells — that is D1c/E1. After D1b you can start/stop a kernel and see its status; you can't run code yet.

### In scope
- `egret-kernel` crate: `KernelLauncher` trait + **subprocess** impl + a **fake** impl (for tests).
- A `kernels` registry table (one row per user×workspace).
- A `KernelManager` orchestrating launcher + registry (ensure/status/stop/touch).
- Lifecycle REST API on the workspace session plane.
- An idle reaper background task.
- A small F1 refactor: extract `build_notebook_runtime(ws, user)` so both the HTTP handler and the launcher reuse it; serialize it into the kernel env.

### Out of scope (later)
- WS proxy of the Jupyter protocol (D1c); cell execution + output rendering (E1).
- The kernel image + helpers (D1a) — D1b's subprocess impl spawns `jupyter kernelgateway` expecting it on the host PATH; the production image is D1a.
- docker_per_user / k8s_per_user launchers (the trait is designed for them; only subprocess is built now).
- Per-user MinIO IAM / resource presets.

## 2. Decisions (from brainstorm)
- First slice = **D1b** (Rust lifecycle, fully testable with a fake launcher). Mechanism = **Jupyter Kernel Gateway** (sparklabx-style HTTP/WS boundary, proxied in D1c).
- Kernel scope = **per (user × workspace)** (single Python kernel — Daft/PySpark are just imports).

## 3. Components

### 3.1 `KernelLauncher` trait
A pluggable launcher (subprocess → docker → k8s later), mirroring sparklabx's `KernelGateway`:
```rust
#[async_trait]
pub trait KernelLauncher: Send + Sync {
    /// Spawn (or no-op if already live) the user's kernel gateway with `env`;
    /// returns its base URL + phase. May block while the process becomes ready
    /// (callers pass a timeout). `env` carries the F1 runtime manifest.
    async fn ensure(&self, key: KernelKey, env: &KernelEnv) -> Result<KernelHandle, KernelError>;
    async fn status(&self, key: KernelKey) -> Result<KernelPhase, KernelError>;
    async fn stop(&self, key: KernelKey) -> Result<(), KernelError>;
    fn mode(&self) -> &'static str; // "subprocess" | "fake" | …
}
```
- `KernelKey { user_id, workspace_id }`; `KernelHandle { url, phase }`; `KernelPhase { Spawning, Ready, Dead }`.
- **Subprocess impl** (`SubprocessLauncher`): pick a free localhost port; spawn `jupyter kernelgateway --KernelGatewayApp.ip=127.0.0.1 --KernelGatewayApp.port=<port> …` with `env` (the manifest + EGRET_API_URL/EGRET_KERNEL_TOKEN) and a per-kernel working dir; poll `GET <url>/api` until ready (timeout ~90s — first Daft/Spark import is slow); track the child PID. `stop` kills the PID. Holds an in-process map `KernelKey → (Child, url)` (the OS process is the source of truth; the DB registry mirrors it for status/reaper).
- **Fake impl** (`FakeLauncher`, test-only): records `ensure/stop` calls, returns a configurable phase + a dummy url, no process. Lets every lifecycle/registry/reaper/API test run in Rust with no Python.

### 3.2 Registry table (`kernels`)
Migration (append-only):
```sql
CREATE TABLE kernels (
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    gateway_url  text NOT NULL DEFAULT '',
    pid          integer,
    phase        text NOT NULL DEFAULT 'spawning',  -- spawning|ready|dead
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, workspace_id)
);
CREATE INDEX kernels_idle_idx ON kernels (phase, last_used_at);
```
`KernelRegistry` store: upsert on ensure, update phase, `touch(last_used_at=now())`, delete on stop, `list_idle(before)` for the reaper. (The OS process is authoritative; the table is for status display + reaper + surviving a backend restart — sparklabx's pattern.)

### 3.3 `KernelManager`
Orchestrates launcher + registry, holds the idle timeout config:
- `ensure_running(key, env) -> KernelStatus` — if the registry has a Ready row whose process is still alive, touch + return it; else `launcher.ensure(key, env)`, upsert the registry, return status.
- `status(key)`, `stop(key)` (launcher.stop + registry delete), `touch(key)`.

### 3.4 Lifecycle API (session plane, gate `notebook:read`)
- `POST   /ws/{ws}/kernel` → ensure/start; body optional; returns `{ phase, url? }`. Builds the manifest (3.6) and passes it as env.
- `GET    /ws/{ws}/kernel` → current `{ phase, url? }` (status; does not spawn).
- `DELETE /ws/{ws}/kernel` → stop.
The kernel is the caller's own (keyed by `ctx.principal.user_id` + `ctx.workspace_id`) — a user can only start/stop **their own** kernel; no cross-user access.

### 3.5 Idle reaper
A background tokio task (started in egret-bin) on an interval (e.g. every 60s): `registry.list_idle(now - idle_timeout)` → `launcher.stop` + delete each. Idle timeout from config (default 30 min, matching sparklabx). Activity (touch) is bumped by the lifecycle endpoints now; D1c/E1 will also touch on each WS message.

### 3.6 Manifest injection (F1 → kernel)
Refactor F1: extract `build_notebook_runtime(state, ws, user_id) -> Result<NotebookRuntime>` from the existing `/notebook-runtime` HTTP handler so both the handler and the kernel-start path call it. At spawn the start-handler builds the `NotebookRuntime`, serializes it to JSON, and injects it as env:
- `EGRET_NOTEBOOK_RUNTIME` = the manifest JSON (username, catalog{url,warehouse,namespace,token}, engines, catalogs).
- `EGRET_API_URL` + a short-lived `EGRET_KERNEL_TOKEN` (so the kernel can call back for fresh per-query credentials later — sparklabx pattern; the callback endpoints are D1c/E1, but the env is wired now).
The kernel image (D1a) reads these; D1b only produces + injects them.

### 3.7 Crate boundaries
`egret-kernel` depends on `egret-notebook` (for `build_notebook_runtime`), `egret-auth`, `egret-workspace`, `egret-metastore`. The launcher itself is pure (takes a `KernelEnv` map) — it does NOT depend on the catalog; the handler assembles the env. `egret-kernel` exposes `kernel_router()` + `KernelState`, mounted in egret-server like the other routers; the reaper + `KernelManager` (with the chosen launcher) are constructed in egret-bin.

## 4. Lifecycle states
`(none) → POST → Spawning → (poll) → Ready → … → idle/DELETE → Dead/(removed)`. A spawn that fails to become ready within the timeout → `Dead` + a clean error (the row is removed or marked dead so the next POST retries). A dead process detected by the manager (PID gone) → treat as `(none)` and respawn.

## 5. Error handling
- Launcher spawn failure (binary missing, port bind, timeout) → `KernelError` mapped to a clean API error (503/500), not a panic; the registry row is not left in a false `Ready`.
- Backend restart: the in-process child map is lost; the DB rows remain `Ready` but the processes are gone. On `status`/`ensure`, the manager verifies liveness (best-effort: the subprocess impl can't re-adopt a child it didn't spawn → treat orphaned rows as dead and respawn; the reaper also cleans stale rows). Document this.

## 6. Testing
- **All lifecycle logic in Rust with the FakeLauncher** (`#[sqlx::test]` + the fake): registry upsert/touch/delete; `ensure_running` returns existing-Ready vs spawns-new; `status`/`stop`; the reaper kills only rows idle past the timeout; the API endpoints (own-kernel only, `notebook:read` gate, start→status→stop). No Python needed.
- **Manifest injection test:** the start-handler builds the manifest (reusing F1) and the env contains `EGRET_NOTEBOOK_RUNTIME` with the right username/namespace/token + enabled engines.
- **Subprocess impl:** unit-test the free-port pick + the spawn argv assembly + the readiness-poll against a tiny fake HTTP server (spawn a trivial process that serves `/api`), OR mark the real-`jupyter kernelgateway` spawn as an ignored/manual test. The full real-gateway run is **infra (needs Python + jupyter_kernel_gateway)** — documented, exercised once D1a's image exists.
- **Local CI** before merge; **append-only** migrations.

## 7. Build decomposition (one spec)
Migration `kernels` · `KernelLauncher` trait + types + FakeLauncher · SubprocessLauncher · `KernelRegistry` store · `KernelManager` · lifecycle routes · idle reaper · F1 `build_notebook_runtime` refactor + env injection · wire `egret-kernel` into egret-server/egret-bin.

## 8. Open questions (resolve in planning)
1. Free-port allocation: OS-assigned (bind :0, read back) vs a configured range. *Lean: OS-assigned for the subprocess MVP.*
2. `EGRET_KERNEL_TOKEN`: reuse the F1 1-grant catalog token, or a separate short-lived session token for backend callbacks. *Lean: a separate signed token (the catalog token is for the Iceberg REST, not egret callbacks) — but the callback endpoints are D1c/E1, so for D1b inject a placeholder/the api url and finalize the token in D1c.*
3. Liveness re-check after backend restart — best-effort respawn vs a startup sweep that marks all rows dead. *Lean: startup sweep marks orphaned rows dead.*
