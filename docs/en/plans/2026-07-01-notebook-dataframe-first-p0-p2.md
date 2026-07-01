# Notebook dataframe-first — P0 spike + P1 backend tokens + P2 Iceberg/Trino helpers — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Bilingual: English canonical; VI reference at `docs/vi/plans/2026-07-01-notebook-dataframe-first-p0-p2.md`.

**Goal:** De-risk StarRocks-over-JWT (spike), then ship Daft-direct `iceberg()` and `trino()` notebook helpers backed by a new per-user engine-token endpoint — so notebook users get lazy Daft DataFrames authenticated as themselves.

**Architecture:** A new kernel-authenticated callback `POST /api/kernel/engine-token` mints a fresh per-user JWT (reusing F2's `EngineTokenMinter`) and returns the engine's connection coordinates (host/port/kind) resolved from the `databases` registry. The Python kernel gets two thin helpers that build lazy Daft DataFrames: `iceberg(table)` via `daft.read_iceberg` + the existing per-user catalog token, and `trino(sql)` via `daft.read_sql` + a SQLAlchemy connection factory carrying `JWTAuthentication`.

**Tech Stack:** Rust (axum, sqlx), Python 3.11 (Daft, pyiceberg, SQLAlchemy + trino dialect, pytest), Postgres.

**Spec:** `docs/en/specs/2026-07-01-notebook-dataframe-first-daft-design.md`

**Scope note (decomposition):** This plan covers spec Phase 0 + Phase 1 + the Iceberg/Trino half of Phase 2. The `starrocks()` helper (spec Phase 2, transport contingent on Phase 0), removal of the SQL cell surface (spec Phase 3), and full live verification (spec Phase 4) are deliberately deferred to a follow-up plan written once Phase 0 resolves — removing the old SQL path before the new helpers are proven live would be premature.

---

## File Structure

- `docs/en/specs/.../notebook-starrocks-jwt-spike-findings.md` — Phase 0 spike outcome (created by Task 1). *(P0)*
- `crates/egret-kernel/src/routes.rs` — add `POST /kernel/engine-token` handler + route. *(P1)*
- `crates/egret-kernel/src/engine_conn.rs` (new) — `EngineConnResolver` trait: `database_id → EngineConn { kind, host, port }`, keeping egret-kernel free of an egret-database dependency (mirrors the existing `NotebookQueryRunner` seam). *(P1)*
- `crates/egret-bin/src/main.rs` — concrete `EngineConnResolver` over `DatabaseRepo`; wire it into `KernelState`. *(P1)*
- `crates/egret-notebook/src/runtime.rs` + `src/model.rs` — add connection coordinates to `EngineOption` in the manifest. *(P1)*
- `kernel-image/egret_kernel_helpers/engines.py` (new) — `iceberg()`, `trino()`, `_engine_token()`. *(P2)*
- `kernel-image/egret_kernel_helpers/__init__.py` + `startup/00-egret.py` — export/auto-load the new helpers. *(P2)*
- `kernel-image/tests/test_engines.py` (new) — pytest for the helpers. *(P2)*

---

## Phase 0 — StarRocks-over-JWT spike

### Task 1: Prove (or disprove) Python/Daft → StarRocks with a per-user JWT

**Files:**
- Create (scratch, NOT shipped): `scratchpad/starrocks_jwt_spike.py`
- Create (committed): `docs/en/specs/2026-07-01-notebook-starrocks-jwt-spike-findings.md`

This is an exploratory spike, not TDD. The goal is a documented yes/no with a working recipe or a concrete blocker.

- [ ] **Step 1: Configure StarRocks JWT auth in the dev stack**

Confirm the dev StarRocks (`deploy/docker-compose.dev.yml`, `deploy/starrocks/`) has a JWT security integration against the local Keycloak realm `egret`. Create a JWT-mapped user, e.g. run in the StarRocks MySQL client:

```sql
CREATE SECURITY INTEGRATION egret_jwt PROPERTIES (
  "type" = "authentication_jwt",
  "jwks_url" = "http://keycloak:8080/realms/egret/protocol/openid-connect/certs",
  "principal_field" = "preferred_username",
  "required_issuer" = "http://localhost:8081/realms/egret",
  "required_audience" = "starrocks"
);
```
(Match the realm's `aud-starrocks` mapper already in `deploy/keycloak/egret-realm.json`.) Record the exact working properties in the findings doc.

- [ ] **Step 2: Mint a per-user JWT**

Get a token for `tester@egret.local` from Keycloak (password grant against client `egret`, scope including the `starrocks` audience) and save it to a file. Record the command.

- [ ] **Step 3: Try each Python transport, in order, and record what works**

In `scratchpad/starrocks_jwt_spike.py`, attempt to connect and run `SELECT 1` as the JWT user, trying these transports and recording the outcome of each:

```python
# A) Daft via SQLAlchemy connection factory (MySQL dialect) — does the driver
#    accept the OpenID/JWT handshake at all?
import daft
def conn():
    from sqlalchemy import create_engine
    # StarRocks speaks the MySQL wire protocol; JWT needs the openid client plugin.
    return create_engine("mysql+pymysql://tester@<host>:<port>/<db>",
                         connect_args={ ... }).connect()   # try plugin/token args
df = daft.read_sql("SELECT 1 AS n", conn)
print(df.collect())

# B) Raw mysql client 9.2+ with --authentication-openid-connect-client-id-token-file
#    (subprocess) — proves the server side even if the Python driver can't.

# C) trino-style: N/A for StarRocks (MySQL protocol), documents why.
```

- [ ] **Step 4: Write the findings doc**

Create `docs/en/specs/2026-07-01-notebook-starrocks-jwt-spike-findings.md` recording: which transport (if any) authenticated as the JWT user, the exact working recipe or the specific blocker (e.g. "PyMySQL has no openid_connect auth plugin; connectorx MySQL likewise"), and a **recommendation** for the `starrocks()` helper transport: **direct** (`daft.read_sql`) or **hybrid** (keep routing through `POST /kernel/query`). This decides the deferred follow-up plan.

- [ ] **Step 5: Commit the findings (not the scratch script)**

```bash
git add docs/en/specs/2026-07-01-notebook-starrocks-jwt-spike-findings.md
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "docs(notebook): StarRocks-over-JWT Python spike findings (P0)"
```

---

## Phase 1 — Backend engine-token endpoint + manifest coordinates

### Task 2: `EngineConnResolver` seam + fake

**Files:**
- Create: `crates/egret-kernel/src/engine_conn.rs`
- Modify: `crates/egret-kernel/src/lib.rs` (add `pub mod engine_conn;` + re-export)
- Test: unit test inside `engine_conn.rs`

Context: `egret-kernel` must not depend on `egret-database`; it already uses this pattern for `NotebookQueryRunner` (`src/query.rs`), with the concrete impl injected from `egret-bin`. Mirror it. `Database` (in `egret-database`) has fields `{ id, name, engine: Engine, host: String, port: i32, options, has_secret, status }`; `Engine` is an enum whose values map to `"trino"`/`"starrocks"`.

- [ ] **Step 1: Write the failing test + trait**

Create `crates/egret-kernel/src/engine_conn.rs`:

```rust
//! The engine-connection seam: resolve a database_id to the coordinates a kernel
//! needs to open a direct Daft connection. Concrete impl injected from egret-bin
//! (keeps this crate free of an egret-database dependency), mirroring
//! `NotebookQueryRunner`.

use async_trait::async_trait;
use uuid::Uuid;

/// Connection coordinates for a governed engine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineConn {
    /// "trino" | "starrocks".
    pub kind: String,
    pub host: String,
    pub port: i32,
}

#[derive(Debug)]
pub enum EngineConnError {
    NotFound(String),
    Other(String),
}

#[async_trait]
pub trait EngineConnResolver: Send + Sync {
    /// Resolve `database_id` to its connection coordinates.
    async fn resolve(&self, database_id: Uuid) -> Result<EngineConn, EngineConnError>;
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct FakeEngineConnResolver {
    pub conn: Option<EngineConn>,
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait]
impl EngineConnResolver for FakeEngineConnResolver {
    async fn resolve(&self, _database_id: Uuid) -> Result<EngineConn, EngineConnError> {
        self.conn
            .clone()
            .ok_or_else(|| EngineConnError::NotFound("no fake conn".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_resolver_returns_configured_conn() {
        let r = FakeEngineConnResolver {
            conn: Some(EngineConn { kind: "trino".into(), host: "trino".into(), port: 8080 }),
        };
        let got = r.resolve(Uuid::nil()).await.unwrap();
        assert_eq!(got, EngineConn { kind: "trino".into(), host: "trino".into(), port: 8080 });
    }
}
```

- [ ] **Step 2: Wire the module**

In `crates/egret-kernel/src/lib.rs`, add `pub mod engine_conn;` and (matching the `query` re-export style) `pub use engine_conn::{EngineConn, EngineConnError, EngineConnResolver};`.

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-kernel engine_conn -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p egret-kernel
git add crates/egret-kernel/src/engine_conn.rs crates/egret-kernel/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): EngineConnResolver seam for direct-connect coordinates"
```

### Task 3: `POST /kernel/engine-token` handler

**Files:**
- Modify: `crates/egret-kernel/src/routes.rs` (add handler + route + `KernelState` field)
- Test: `crates/egret-kernel/tests/` (extend the existing kernel route test file; grep for how `kernel_query`/`FakeQueryRunner` are tested and mirror it)

Context: `KernelState` already holds `query_runner`, an `engine_minter` (via the notebook state), and the `notebook.store` with `list_engine_options(ws)`. `kernel_query` is the reference handler for kernel-authenticated callbacks: it uses the `KernelIdentity` extractor (resolves `EGRET_KERNEL_TOKEN` → `(user_id, workspace_id)`), re-validates the engine is enabled, and mints the user JWT. Read `kernel_query` in `routes.rs` and copy its identity + enable-gate pattern.

- [ ] **Step 1: Write the failing test**

Add to the kernel routes test file a test that POSTs `/kernel/engine-token` with a valid kernel token + an enabled engine id and asserts a 200 with `{ jwt, expires_at, kind, host, port }`. Use the fakes: a `FakeEngineConnResolver` returning `{kind:"trino",host:"trino",port:8080}`, a fake `EngineTokenMinter` returning a known token, and a store seeded with one enabled engine whose id matches. (Mirror the existing `kernel_query` test's harness for building `KernelState` + issuing a valid `EGRET_KERNEL_TOKEN`.)

```rust
#[tokio::test]
async fn engine_token_returns_jwt_and_coords_for_enabled_engine() {
    // ... build KernelState with FakeEngineConnResolver + fake minter + store with
    //     one enabled engine (id = ENGINE_ID) for workspace WS, and a live kernel
    //     row so KernelIdentity resolves to (USER, WS). Reuse the kernel_query
    //     test harness helpers in this file.
    let body = serde_json::json!({ "engine": ENGINE_ID.to_string() });
    let resp = post_json(&app, "/kernel/engine-token", &token, &body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v: serde_json::Value = resp.json().await;
    assert_eq!(v["kind"], "trino");
    assert_eq!(v["host"], "trino");
    assert_eq!(v["port"], 8080);
    assert!(v["jwt"].as_str().unwrap().len() > 0);
    assert!(v["expires_at"].as_str().is_some());
}
```

- [ ] **Step 2: Run it — confirm it fails**

Run: `cargo test -p egret-kernel engine_token_returns_jwt_and_coords_for_enabled_engine`
Expected: FAIL (route/handler missing).

- [ ] **Step 3: Add the `EngineConnResolver` to `KernelState` and the handler**

In `crates/egret-kernel/src/routes.rs`: add `pub engine_conn: std::sync::Arc<dyn crate::engine_conn::EngineConnResolver>` to `KernelState`; register the route `.route("/kernel/engine-token", post(engine_token))` next to `/kernel/query`; add:

```rust
#[derive(serde::Deserialize)]
struct EngineTokenReq {
    /// Engine id (uuid string) or name from the manifest.
    engine: String,
}

#[derive(serde::Serialize)]
struct EngineTokenResp {
    jwt: String,
    expires_at: chrono::DateTime<chrono::Utc>,
    kind: String,
    host: String,
    port: i32,
}

/// `POST /kernel/engine-token` — mint a fresh per-user JWT + return the engine's
/// connection coordinates, so the Daft kernel can open a direct connection.
/// Kernel-authenticated (EGRET_KERNEL_TOKEN → KernelIdentity). Identity comes
/// only from the token, never the body.
async fn engine_token(
    State(st): State<KernelState>,
    id: KernelIdentity,
    Json(req): Json<EngineTokenReq>,
) -> Result<Json<EngineTokenResp>, KernelApiError> {
    // 1. Resolve engine (id or name) among the workspace's ENABLED notebook engines.
    let engines = st
        .notebook
        .store
        .list_engine_options(id.workspace_id)
        .await
        .map_err(|e| KernelApiError(e.0))?;
    let engine = engines
        .into_iter()
        .filter(|e| e.enabled)
        .find(|e| e.id.to_string() == req.engine || e.name == req.engine)
        .ok_or_else(|| {
            KernelApiError(egret_core::Error::NotFound(format!(
                "engine not enabled for notebooks: {}",
                req.engine
            )))
        })?;

    // 2. Resolve connection coordinates.
    let conn = st
        .engine_conn
        .resolve(engine.id)
        .await
        .map_err(|e| match e {
            crate::engine_conn::EngineConnError::NotFound(m) => {
                KernelApiError(egret_core::Error::NotFound(m))
            }
            crate::engine_conn::EngineConnError::Other(m) => {
                KernelApiError(egret_core::Error::Internal(anyhow::anyhow!(m)))
            }
        })?;

    // 3. Mint the user's short-lived JWT (F2 EngineTokenMinter).
    let tok = st
        .notebook
        .engine_minter
        .mint(id.user_id)
        .await
        .map_err(|e| KernelApiError(egret_core::Error::Internal(anyhow::anyhow!(e.to_string()))))?;

    Ok(Json(EngineTokenResp {
        jwt: tok.access_token,
        expires_at: tok.expires_at,
        kind: conn.kind,
        host: conn.host,
        port: conn.port,
    }))
}
```
Adjust field access (`st.notebook.store`, `st.notebook.engine_minter`) to the actual `KernelState`/`NotebookState` shape — read the struct and match it. `list_engine_options` returns `Result<_, NotebookApiError>` (a newtype over `egret_core::Error` — `.0`); confirm and map accordingly.

- [ ] **Step 4: Run the test — confirm it passes**

Run: `cargo test -p egret-kernel engine_token_returns_jwt_and_coords_for_enabled_engine`
Expected: PASS.

- [ ] **Step 5: Add the not-enabled test**

Add a second test asserting a request for a disabled/unknown engine id returns 404. Run the file's tests; both pass.

- [ ] **Step 6: fmt/clippy + commit**

```bash
cargo fmt -p egret-kernel
cargo clippy -p egret-kernel --all-targets -- -D warnings
git add crates/egret-kernel/src/routes.rs crates/egret-kernel/tests/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): POST /kernel/engine-token (per-user JWT + engine coords)"
```

### Task 4: Concrete `EngineConnResolver` in egret-bin + wire-up

**Files:**
- Modify: `crates/egret-bin/src/main.rs` (impl + inject into `KernelState`)

Context: `EngineQueryRunner` in `main.rs` already resolves a `database_id` via `self.databases.get(database_id)` → a `Database { engine: Engine, host, port, .. }`. Add a sibling resolver that returns coordinates. `Engine` maps to a label — reuse whatever `Database.engine` renders to for the `"trino"`/`"starrocks"` label used by `EngineOption.engine` (check how `list_engine_options` derives its `engine` string and match it exactly).

- [ ] **Step 1: Implement the resolver**

In `crates/egret-bin/src/main.rs`, near `EngineQueryRunner`:

```rust
struct EngineConnResolverImpl {
    databases: std::sync::Arc<egret_database::DatabaseRepo>, // match the type used by EngineQueryRunner
}

#[async_trait::async_trait]
impl egret_kernel::engine_conn::EngineConnResolver for EngineConnResolverImpl {
    async fn resolve(
        &self,
        database_id: uuid::Uuid,
    ) -> Result<egret_kernel::engine_conn::EngineConn, egret_kernel::engine_conn::EngineConnError> {
        use egret_kernel::engine_conn::{EngineConn, EngineConnError};
        let db = self
            .databases
            .get(database_id)
            .await
            .map_err(|e| EngineConnError::NotFound(e.to_string()))?;
        Ok(EngineConn {
            kind: db.engine.as_label().to_string(), // match EngineOption.engine's "trino"/"starrocks"
            host: db.host,
            port: db.port,
        })
    }
}
```
Replace `db.engine.as_label()` with the actual conversion the codebase uses to render `Engine` → `"trino"`/`"starrocks"` (grep how `EngineOption.engine` is populated in the store).

- [ ] **Step 2: Inject it into `KernelState`**

Where `KernelState` is assembled in `main.rs` (grep `KernelState {`), construct `EngineConnResolverImpl { databases: databases.clone() }` (reuse the same `databases` handle passed to `EngineQueryRunner`) and set `engine_conn: Arc::new(...)`.

- [ ] **Step 3: Build**

Run: `cargo build -p egret-bin`
Expected: compiles clean.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p egret-bin
git add crates/egret-bin/src/main.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(bin): wire EngineConnResolver over DatabaseRepo into KernelState"
```

### Task 5: Add engine connection coords to the runtime manifest

**Files:**
- Modify: `crates/egret-notebook/src/model.rs` (extend `EngineOption`)
- Modify: `crates/egret-notebook/src/runtime.rs` (populate coords)
- Modify: `crates/egret-notebook/src/store.rs` (`list_engine_options` SQL selects host/port)

Context: helpers need `{host, port, kind}` per enabled engine without a per-engine round-trip for coordinates (the JWT is still fetched fresh from `engine-token`). `EngineOption` currently is `{ id, name, engine, enabled }`. The connection coords live in the `databases` row that backs each engine option — extend the `list_engine_options` query to also select `host`, `port`.

- [ ] **Step 1: Extend `EngineOption`**

In `crates/egret-notebook/src/model.rs`, add to `EngineOption`:

```rust
    /// Engine host (from the backing databases row) — for direct Daft connections.
    pub host: String,
    /// Engine port.
    pub port: i32,
```

- [ ] **Step 2: Populate in the store query**

In `crates/egret-notebook/src/store.rs`, update `list_engine_options`'s SQL + row mapping to select the backing database `host` and `port` (join to the `databases` table the engine option references). Read the existing query to see the join, and add `host`, `port` to the SELECT and the constructed `EngineOption`.

- [ ] **Step 3: Run the notebook store tests**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-notebook list_engine_options`
Expected: existing tests compile against the new fields (fix any struct-literal call sites the compiler flags, e.g. in tests) and pass. If no such test exists, add one asserting `host`/`port` are populated for a seeded enabled engine.

- [ ] **Step 4: Confirm the manifest serializes the new fields**

The manifest `engines: Vec<EngineOption>` now serializes `host`/`port` automatically (serde). No handler change needed. Build the workspace:
Run: `cargo build -p egret-notebook -p egret-kernel`
Expected: clean.

- [ ] **Step 5: fmt/clippy + commit**

```bash
cargo fmt -p egret-notebook
cargo clippy -p egret-notebook --all-targets -- -D warnings
git add crates/egret-notebook/src/model.rs crates/egret-notebook/src/runtime.rs crates/egret-notebook/src/store.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): expose engine host/port in the runtime manifest"
```

---

## Phase 2 — Kernel helpers: `iceberg()` and `trino()`

### Task 6: `_engine_token()` — fetch + cache the per-user JWT/coords

**Files:**
- Create: `kernel-image/egret_kernel_helpers/engines.py`
- Test: `kernel-image/tests/test_engines.py`

Context: mirror `kernel-image/egret_kernel_helpers/query.py` — same env (`EGRET_API_URL`, `EGRET_KERNEL_TOKEN`), same `_post` style, same clear-error discipline (never silent-empty). The kernel is long-lived; the JWT is short-lived, so cache per engine and refresh within a safety margin of expiry. Daft opens the SQL connection at execution time, so the factory must call `_engine_token` each time (it returns cached-until-near-expiry).

- [ ] **Step 1: Write the failing test**

Create `kernel-image/tests/test_engines.py`:

```python
import json
from egret_kernel_helpers import engines as E


def _fake_http(responses):
    calls = []
    def http(url, body, token):
        calls.append((url, json.loads(body.decode()), token))
        return responses.pop(0)
    http.calls = calls
    return http


def test_engine_token_fetches_and_caches(monkeypatch):
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    http = _fake_http([
        {"jwt": "J1", "expires_at": "2999-01-01T00:00:00Z",
         "kind": "trino", "host": "trino", "port": 8080},
    ])
    cache = {}
    tok = E._engine_token("trino", env=env, http=http, cache=cache)
    assert tok["jwt"] == "J1" and tok["host"] == "trino" and tok["port"] == 8080
    # Cached: a second call (far from expiry) does not hit http again.
    tok2 = E._engine_token("trino", env=env, http=http, cache=cache)
    assert tok2["jwt"] == "J1"
    assert len(http.calls) == 1
    assert http.calls[0][0] == "http://egret/api/kernel/engine-token"
    assert http.calls[0][1] == {"engine": "trino"}
```

- [ ] **Step 2: Run it — confirm it fails**

Run: `cd kernel-image && python -m pytest tests/test_engines.py::test_engine_token_fetches_and_caches -q`
Expected: FAIL (module/function missing).

- [ ] **Step 3: Implement `_engine_token`**

Create `kernel-image/egret_kernel_helpers/engines.py`:

```python
"""Dataframe-first engine helpers: iceberg(), trino() — direct Daft as-you."""
import datetime as _dt
import json
import os

_REFRESH_MARGIN = _dt.timedelta(seconds=30)


def _parse_expiry(s):
    # RFC3339; tolerate trailing "Z".
    return _dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def _now():
    return _dt.datetime.now(_dt.timezone.utc)


def _post(url, body, token):
    import urllib.error
    import urllib.request
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise RuntimeError("egret: kernel token expired — restart the kernel") from None
        if e.code in (403, 404):
            raise RuntimeError("egret: engine not enabled for notebooks or not permitted") from None
        detail = e.read().decode()[:200]
        raise RuntimeError(f"egret: engine-token failed (HTTP {e.code}): {detail}") from None
    except Exception as e:  # noqa: BLE001
        raise RuntimeError(f"egret: cannot reach the engine-token endpoint ({e})") from None


# Module-level cache: {engine: {jwt, expires_at(dt), kind, host, port}}.
_CACHE: dict = {}


def _engine_token(engine, *, env=None, http=None, cache=None):
    env = os.environ if env is None else env
    cache = _CACHE if cache is None else cache
    cached = cache.get(engine)
    if cached is not None and _parse_expiry(cached["expires_at_raw"]) - _REFRESH_MARGIN > _now():
        return cached
    api = env.get("EGRET_API_URL")
    token = env.get("EGRET_KERNEL_TOKEN")
    if not api or not token:
        raise RuntimeError("egret: kernel not configured (missing EGRET_API_URL / EGRET_KERNEL_TOKEN)")
    body = json.dumps({"engine": engine}).encode()
    resp = (http or _post)(api.rstrip("/") + "/api/kernel/engine-token", body, token)
    resp["expires_at_raw"] = resp["expires_at"]
    cache[engine] = resp
    return resp
```

- [ ] **Step 4: Run it — confirm it passes**

Run: `cd kernel-image && python -m pytest tests/test_engines.py::test_engine_token_fetches_and_caches -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add kernel-image/egret_kernel_helpers/engines.py kernel-image/tests/test_engines.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): _engine_token fetch+cache for direct engine connections"
```

### Task 7: `trino(sql)` helper

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/engines.py`
- Test: `kernel-image/tests/test_engines.py`

- [ ] **Step 1: Write the failing test**

Add to `test_engines.py`:

```python
def test_trino_builds_read_sql_with_jwt(monkeypatch):
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"trino": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                       "expires_at_raw": "2999-01-01T00:00:00Z",
                       "kind": "trino", "host": "trino", "port": 8080}}
    captured = {}
    fake_daft = type("D", (), {"read_sql": staticmethod(
        lambda sql, conn, **kw: captured.update(sql=sql, conn=conn) or "DF")})
    monkeypatch.setitem(__import__("sys").modules, "daft", fake_daft)
    out = E.trino("SELECT 1", env=env, cache=cache)
    assert out == "DF"
    assert captured["sql"] == "SELECT 1"
    assert callable(captured["conn"])   # a connection factory, not a string
```

- [ ] **Step 2: Run it — confirm it fails**

Run: `cd kernel-image && python -m pytest tests/test_engines.py::test_trino_builds_read_sql_with_jwt -q`
Expected: FAIL.

- [ ] **Step 3: Implement `trino`**

Add to `engines.py`:

```python
def trino(sql, *, engine="trino", env=None, cache=None):
    """Run SQL on the workspace Trino as you, returning a lazy Daft DataFrame
    (pushdown). Authenticated with a fresh per-user JWT."""
    import daft
    tok = _engine_token(engine, env=env, cache=cache)

    def _conn():
        from sqlalchemy import create_engine
        from trino.auth import JWTAuthentication
        eng = create_engine(
            f"trino://egret@{tok['host']}:{tok['port']}",
            connect_args={"auth": JWTAuthentication(tok["jwt"]), "http_scheme": "https"},
        )
        return eng.connect()

    return daft.read_sql(sql, _conn)
```

- [ ] **Step 4: Run it — confirm it passes**

Run: `cd kernel-image && python -m pytest tests/test_engines.py -q`
Expected: PASS (all tests).

- [ ] **Step 5: Commit**

```bash
git add kernel-image/egret_kernel_helpers/engines.py kernel-image/tests/test_engines.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): trino(sql) -> lazy Daft DataFrame via per-user JWT"
```

### Task 8: `iceberg(table)` helper

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/engines.py`
- Test: `kernel-image/tests/test_engines.py`

Context: reuse the existing `catalog()` helper (`kernel-image/egret_kernel_helpers/catalog.py`) which returns a pyiceberg `RestCatalog` bound to the workspace warehouse with the user's token. `iceberg(table)` loads the table and hands it to `daft.read_iceberg`.

- [ ] **Step 1: Write the failing test**

Add to `test_engines.py`:

```python
def test_iceberg_reads_via_catalog(monkeypatch):
    loaded = {}
    class FakeCat:
        def load_table(self, name):
            loaded["name"] = name
            return f"TBL:{name}"
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    captured = {}
    fake_daft = type("D", (), {"read_iceberg": staticmethod(
        lambda t, **kw: captured.update(tbl=t) or "DF")})
    monkeypatch.setitem(__import__("sys").modules, "daft", fake_daft)
    out = E.iceberg("sales.orders")
    assert out == "DF"
    assert loaded["name"] == "sales.orders"
    assert captured["tbl"] == "TBL:sales.orders"
```

- [ ] **Step 2: Run it — confirm it fails**

Run: `cd kernel-image && python -m pytest tests/test_engines.py::test_iceberg_reads_via_catalog -q`
Expected: FAIL.

- [ ] **Step 3: Implement `iceberg`**

Add to `engines.py` (import `catalog` at module top: `from .catalog import catalog`):

```python
def iceberg(table, *, catalog_id=None):
    """Read an Iceberg table as a lazy Daft DataFrame (native, pushdown). `table`
    is "namespace.name" resolved in the workspace warehouse / your private
    `notebook_{username}` namespace."""
    import daft
    tbl = catalog(catalog_id).load_table(table)
    return daft.read_iceberg(tbl)
```
(Confirm `catalog()`'s parameter name; the current signature is `catalog(id=None, *, env=None)` — call it positionally as `catalog(catalog_id)`.)

- [ ] **Step 4: Run it — confirm it passes**

Run: `cd kernel-image && python -m pytest tests/test_engines.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add kernel-image/egret_kernel_helpers/engines.py kernel-image/tests/test_engines.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): iceberg(table) -> lazy Daft DataFrame (native)"
```

### Task 9: Auto-load the helpers

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/__init__.py`
- Modify: `kernel-image/startup/00-egret.py`
- Test: `kernel-image/tests/test_init.py`

- [ ] **Step 1: Export from the package**

In `kernel-image/egret_kernel_helpers/__init__.py`, add `iceberg` and `trino` to the imports/`__all__` alongside `query`, `catalog`, `display`, `runtime`, `me`:

```python
from .engines import iceberg, trino  # noqa: F401
```

- [ ] **Step 2: Auto-load in startup**

In `kernel-image/startup/00-egret.py`, extend the first import line so `iceberg` and `trino` are pulled into the global namespace:

```python
from egret_kernel_helpers import query, catalog, display, runtime, me, iceberg, trino  # noqa: F401
```

- [ ] **Step 3: Update/extend the init test**

In `kernel-image/tests/test_init.py`, assert `iceberg` and `trino` are importable from `egret_kernel_helpers`. Run:
`cd kernel-image && python -m pytest tests/test_init.py -q`
Expected: PASS.

- [ ] **Step 4: Full kernel-helpers test run + commit**

Run: `cd kernel-image && python -m pytest -q`
Expected: all pass.

```bash
git add kernel-image/egret_kernel_helpers/__init__.py kernel-image/startup/00-egret.py kernel-image/tests/test_init.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): auto-load iceberg()/trino() into the kernel namespace"
```

---

## Task 10: Phase-close verification

**Files:** none (verification only).

- [ ] **Step 1: Backend tests**

Run (Postgres up):
```bash
cargo fmt --all -- --check
cargo clippy -p egret-kernel -p egret-notebook -p egret-bin --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-kernel -p egret-notebook
```
Expected: green.

- [ ] **Step 2: Kernel helper tests**

Run: `cd kernel-image && python -m pytest -q`
Expected: green.

- [ ] **Step 3: Manual smoke (optional, needs the live stack + kernel image rebuild)**

With the OIDC stack up and the kernel image rebuilt (D1a), in a notebook code cell:
```python
df = iceberg("notebook_<you>.smoke")   # or any table you can read
df.show()
df2 = trino("SELECT 1 AS n")
df2.show()
```
Expected: both render as Daft DataFrames authenticated as you. (StarRocks is out of scope for this plan — pending the Phase 0 decision.)

- [ ] **Step 4: Finish the increment**

Use `superpowers:finishing-a-development-branch`. The follow-up plan (StarRocks helper per Phase 0 + removal of the SQL cell surface + full live verification) is written next.

---

## Notes / risks

- **Phase 0 gates the follow-up, not this plan.** `iceberg()`/`trino()` do not depend on the spike; only `starrocks()` (deferred) does.
- **Trino `http_scheme`.** The helper hard-codes `https`. If the dev Trino is plaintext `http`, thread the scheme from the engine coordinates (add `scheme` to `EngineConn`/`EngineOption`) — do this if the manual smoke fails on TLS. Kept out of the initial cut to stay minimal; revisit in Step 3 smoke.
- **`daft.read_sql` connection factory** must return a live DB-API/SQLAlchemy connection each call; Daft may open it more than once (partitioning) — the factory re-reads the cached token, which is fine.
- **Removing the old SQL path is deferred.** Until the follow-up plan lands, `%%egret_sql` still exists; the new helpers are additive, so nothing breaks meanwhile.
