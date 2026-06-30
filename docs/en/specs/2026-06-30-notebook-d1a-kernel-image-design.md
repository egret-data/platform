# Notebook D1a — Kernel Image + Helpers + Query Callback (design)

**Status:** draft · **Date:** 2026-06-30 · **Epic:** Notebooks · **Predecessors:** F1 (#95), D1b (#96), D1c (#97) — all merged

## 1. Context & scope

D1 (kernel runtime) = D1a (image+helpers+query-callback, **this spec**) · D1b (launcher+lifecycle, DONE) · D1c (WS proxy, DONE). D1b spawns a Jupyter Kernel Gateway per (user×workspace) and injects the F1 runtime manifest as env (`EGRET_NOTEBOOK_RUNTIME` / `EGRET_API_URL` / `EGRET_KERNEL_TOKEN`). D1c proxies the channels WS. **D1a makes the kernel actually useful**: it builds the kernel image (Daft preloaded), the Python helper package (`query()`/`catalog()`/`display()`), and the egret-side query callback + real kernel token that `query()` depends on.

After D1a, a notebook cell can read governed data via `query()` (Trino/StarRocks + Ranger, as the user) and read/write its private Iceberg namespace via `catalog()` (Daft + pyiceberg). **E1** later wires the FE cell types + output rendering to *use* these helpers — D1a is the capability layer, not the UX.

### In scope
- **Kernel image** (`kernel-image/`): Docker image = Jupyter Kernel Gateway + Python 3.11 + Daft + pyiceberg + the helper package; `entrypoint.sh` starts the gateway on :8888.
- **Helper package** (`egret_kernel_helpers`, pip-installable): `query()`, `catalog()`, `display()`, `runtime`, `me` — auto-loaded into every kernel via an IPython startup shim (no import needed).
- **Query callback** (Rust): `POST /api/kernel/query` — token-authenticated, runs SQL on an enabled workspace engine with per-user Ranger impersonation (`X-Trino-User`), returns JSON rows.
- **`EGRET_KERNEL_TOKEN`** (Rust): a real short-lived bearer minted on the spawn path, hashed onto the `kernels` row (append-only migration), injected into the kernel env, revoked on stop/reap. Replaces the D1b placeholder `""`.

### Out of scope (later)
- FE cell execution wiring + output rendering + the Run button — that is **E1** ("4 cell types + output rendering"). D1a provides the helpers/endpoint E1 calls.
- PySpark / Spark Connect in the image — Daft only for D1a (single Python kernel; PySpark a later opt-in).
- docker_per_user / k8s_per_user launchers (D1b designed the trait; only `SubprocessLauncher` exists). The image is the canonical environment those modes will run; the host SubprocessLauncher pip-installs the same helper package.
- Arrow result transport — JSON (columns + typed rows) for the MVP, reusing the engine's existing type coercion. Arrow is a later optimization.

## 2. Decisions (from brainstorm)
- **`query()` = backend callback, server-side execution** (chosen over kernel-direct-JDBC and over deferring to E1). The kernel POSTs `{engine, sql}` to egret; egret runs it on Trino/StarRocks with `X-Trino-User` Ranger impersonation and returns rows. The kernel holds **no** engine credentials and needs **no** drivers — egret is the single governance choke-point. The kernel token is the cost paid (a real signed/hashed bearer instead of the D1b placeholder).
- **Daft + pyiceberg only** in the image (no PySpark).
- **Verify with pytest (helpers) + CI `docker build` + a docker-run smoke** (start the container, confirm the gateway is alive, run `1+1` through it). Real governed-read end-to-end (Trino+Ranger) stays manual + documented.
- **Two build plans behind the query contract:** P1 = Rust (kernel token + query callback), P2 = Docker/Python (image + helpers). Independently testable.

## 3. Components

### 3.1 `EGRET_KERNEL_TOKEN` (egret-kernel, Rust)
A short-lived bearer the kernel uses to call egret back. **Minted only on the spawn path** inside `KernelManager::ensure_running` (NOT on the reuse path — reuse keeps the kernel's existing token). Stored **hashed (SHA-256)** on the `kernels` row via append-only migration `0043` (add `token_hash text`, `token_expires_at timestamptz`), mirroring the `catalog_tokens`/session hashed-token pattern. The plaintext is generated once, inserted into the kernel env map by `ensure_running` (so `manifest_to_env` no longer sets the placeholder), and never persisted in plaintext. Revoked implicitly when the `kernels` row is deleted on stop/reap. A `KernelToken` validator hashes a presented bearer, matches a live non-expired row, and resolves `(user_id, workspace_id)`; the username comes from `UserRepo::ensure_username` (F1).

### 3.2 Query callback (Rust)
`POST /api/kernel/query` — **token plane** (bearer, not the session cookie). Mounted alongside the kernel routes. Body:
```json
{ "engine": "<engine id or name>", "sql": "SELECT ...", "max_rows": 10000 }
```
Flow: (1) validate `EGRET_KERNEL_TOKEN` → `(user_id, workspace_id)`; (2) resolve `engine` against the workspace's **enabled notebook engines** (the F1 `engines` list / `notebook_engine_access`) — unknown/disabled → 403; (3) execute the SQL via egret-engine's connector for that database, setting `X-Trino-User=<username>` (the F1 impersonation path the BI/sqllab features already use), capped at `max_rows`; (4) return `{ "columns": [...], "rows": [[...], ...] }` JSON, reusing the engine layer's existing per-column type coercion. Errors: 401 (invalid/expired token), 403 (engine not enabled / cross-ws), 400 (SQL/engine error, message included). The endpoint never runs SQL on an engine the workspace has not enabled for notebooks.

### 3.3 Kernel image (`kernel-image/`, Docker)
`FROM python:3.11-slim`; `pip install jupyter_kernel_gateway daft pyiceberg requests` + the helper package (`pip install .`); `COPY entrypoint.sh`; `EXPOSE 8888`; entrypoint runs `jupyter kernelgateway --ip=0.0.0.0 --port=8888 --KernelGatewayApp.auth_token='' --JupyterWebsocketPersonality.list_kernels=True` (matching what D1b spawns and D1c proxies). The helper package ships an IPython startup file (installed into the profile's `startup/` dir, or registered via the package) that imports the helpers into the user namespace so `query`/`catalog`/`display` are available without an import. The image is multi-purpose: docker/k8s launcher modes run it directly; the host `SubprocessLauncher` (D1b MVP) pip-installs the same package + Daft so the helpers match.

### 3.4 Python helpers (`egret_kernel_helpers`)
Loaded once on kernel start; read `EGRET_NOTEBOOK_RUNTIME` (parsed into a `runtime` object), `EGRET_API_URL`, `EGRET_KERNEL_TOKEN`.
- **`query(engine, sql, max_rows=10000) -> daft.DataFrame`** — resolve `engine` to an id by matching `runtime.engines` (by id or name); `POST {EGRET_API_URL}/api/kernel/query` with `Authorization: Bearer {EGRET_KERNEL_TOKEN}` and `{engine, sql, max_rows}`; parse `{columns, rows}` → `daft.from_pydict`. Raises clear errors on 401/403/timeout — never returns a silent empty frame.
- **`catalog(id=None)`** — **fully self-contained** (no callback): build a pyiceberg `RestCatalog` from `runtime.catalog` (`uri=url`, `warehouse`, `token`); default namespace = private `notebook_{username}`. Helpers to list/read a table as Daft (`daft.read_iceberg`) and to write a Daft DataFrame back into the private namespace. `id`/name selects an enabled workspace catalog when given; default = the private namespace.
- **`display(obj, n=50)`** — Daft DataFrames already expose `_repr_html_`; `display` applies a row cap and renders, falling through to IPython's `display` for non-DataFrames (so existing code isn't broken).
- **`runtime`** (the parsed manifest: `username`, `catalog`, `engines`, `catalogs`) and **`me`** (= `runtime.username`).

## 4. Data flow
Two planes:
- **Governed read:** cell `query("trino-dev", sql)` → `POST /api/kernel/query` (Bearer kernel token) → egret resolves user+ws, runs SQL on Trino/StarRocks with `X-Trino-User=<username>` (Ranger enforces) → JSON `{columns, rows}` → `daft.from_pydict` → DataFrame.
- **Private read/write:** cell `catalog().load_table("notebook_alice.t")` → pyiceberg `RestCatalog` (1-grant token) → Iceberg REST `notebook_{user}` namespace → Daft read/write directly (egret not in the data path).

## 5. Auth / security
- The kernel token is scoped to one `(user_id, workspace_id)`, hashed at rest, short-lived (= kernel lifetime), and revoked when the kernel stops/reaps. A leaked token only grants what the user can already do, and only until the kernel dies.
- The query callback runs SQL **only** on engines the workspace has enabled for notebooks, **always** as the end user via `X-Trino-User` (never as the service principal) — Ranger is the policy gate, exactly as for the rest of egret's governed reads.
- The catalog plane stays as F1 designed it: a 1-grant token scoped to the user's private namespace.
- The kernel container holds no long-lived credentials beyond the short-lived kernel token + the 1-grant catalog token already injected by F1/D1b.

## 6. Error handling
- `query()`: 401 → "kernel token expired (restart the kernel)"; 403 → "engine not enabled for notebooks / not permitted"; engine/SQL error → 400 with the message; timeout/unreachable → clear RuntimeError. Never a silent empty frame.
- `catalog()`: missing warehouse/token in the manifest → clear error pointing at workspace catalog config.
- Token: expired/revoked/unknown → callback 401 (kernel re-runs `connect` → new kernel → new token).
- Image: a missing env (`EGRET_NOTEBOOK_RUNTIME`) → helpers load in a degraded mode that raises a clear message when called (so a bare `jupyter` run doesn't crash on import).

## 7. Testing
- **pytest the helper package** (no kernel, no network): mock the env + the callback HTTP (`responses`/`respx`) → `query()` builds the correct request and turns `{columns, rows}` into the right Daft DataFrame; engine name→id resolution; `catalog()` builds the expected `RestCatalog` kwargs (pyiceberg mocked); `display()` renders a DataFrame to HTML and falls through for non-DataFrames; 401/403/timeout raise clear errors.
- **Rust callback** (`#[sqlx::test]` + a fake engine connector double, no real Trino): token validate (valid → user resolved; invalid/expired → 401); engine-enabled gate (enabled → runs; disabled/cross-ws → 403); `X-Trino-User` set to the resolved username; `{columns, rows}` JSON shape; token mint-on-spawn / not-on-reuse / revoke-on-stop.
- **CI:** `docker build` the image; **docker-run smoke** — start the container, `GET /api/kernels` (gateway alive), start a python3 kernel + execute `1+1` via the gateway REST and assert `2`. (No Trino/Ranger in CI.)
- **Manual (documented):** full governed-read end-to-end against the real stack (Trino + Ranger + a provisioned warehouse), per the existing manual-Trino integration test.
- **Local CI before merge; migrations append-only.**

## 8. Build decomposition (two plans behind the query contract)
- **P1 — Rust:** migration 0043 (`kernels.token_hash`/`token_expires_at`) · kernel-token mint (spawn path) / validate / revoke · `require_kernel_token` extractor · `POST /api/kernel/query` (engine-enabled gate + `X-Trino-User` exec + JSON) · wire into egret-kernel/egret-server/egret-bin.
- **P2 — Docker/Python:** `kernel-image/` Dockerfile + entrypoint · `egret_kernel_helpers` package (`query`/`catalog`/`display`/`runtime`/`me`) + IPython startup shim · pytest · CI `docker build` + docker-run smoke.

## 9. Open questions (resolve in planning)
1. Crate for the query callback: extend `egret-kernel` with an `egret-engine` dependency, or host the endpoint in `egret-server` where engine wiring already lives. *Lean: whichever avoids a dependency cycle — likely a thin handler in egret-kernel taking an engine-runner trait, with the concrete runner injected from egret-bin (same pattern as the launcher/proxy).* 
2. Engine result coercion reuse: confirm the exact reusable function/shape from the BI/sqllab query path so the callback returns an identical `{columns, rows}` contract. *Lean: reuse the existing engine result type; do not invent a new coercion.*
3. Helper auto-load mechanism: IPython `startup/` file copied by the image vs a package entry-point/`.pth`. *Lean: a startup file installed by the package (testable + works under the gateway's IPython profile).*
