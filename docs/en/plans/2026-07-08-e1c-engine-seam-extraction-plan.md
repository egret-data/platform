# E1c — Engine domain leaves control-plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `engines.rs` (3.2k) + `monitor.rs` (389) out of `egret-controlplane` into the `egret-engine-mgmt` seam crate, behind a kube-free `EngineProvisioningPort` in `egret-engine-mgmt-api`, preserving all behavior and FE URLs. After this, the engine seam is fully extracted and control-plane no longer references engines.

**Architecture:** One atomic relocation (the workspace only compiles at the end). The 9-method `EngineProvisioner` trait splits: 5 kube-free methods (`provision/status/update/delete/restart`) become `EngineProvisioningPort` (in `-api`); the 4 kube-shaped ones (`coordinator_request/pod_logs/pod_logs_stream/exec`) are served by calling `egret_orchestrator::KubeClient` directly from the seam state. The seam gets its own `EngineMgmtState` + HTTP error, and its router nests at `/api/control-plane` so URLs are unchanged. Design: [E1c design](../specs/2026-07-08-e1c-engine-seam-extraction-design.md).

**Tech Stack:** Rust 2021, axum (ws), sqlx, kube (confined to orchestrator + seam crate, never `-api`).

**Ground truth:** file:line anchors below are from the current `main` (post-E1b). Use the compiler + `git mv` to relocate; do not re-type the 3.6k LOC by hand.

**Scope OUT:** any change to provisioning/parsing/auto-wiring *logic* (verbatim move); abstracting the cross-seam catalog/authz calls behind Ports (E3); any authz-model change (Effort B); frontend.

---

## Task 1: `EngineProvisioningPort` in `egret-engine-mgmt-api`

**Files:** `crates/egret-engine-mgmt-api/Cargo.toml`, `crates/egret-engine-mgmt-api/src/lib.rs`

- [ ] **Step 1: Add async-trait dep**

`crates/egret-engine-mgmt-api/Cargo.toml` `[dependencies]`, add `async-trait.workspace = true` (currently only `serde_json`).

- [ ] **Step 2: Define the Port**

Append to `crates/egret-engine-mgmt-api/src/lib.rs`:
```rust
use async_trait::async_trait;

/// Provisions/updates/deletes a query engine on Kubernetes. Kube-free by
/// construction (this crate has no kube dep). Implemented in the composition
/// root (egret-bin) over `egret-orchestrator::TrinoProvisioner`.
#[async_trait]
pub trait EngineProvisioningPort: Send + Sync {
    async fn provision(&self, req: &EngineSpec) -> Result<(), String>;
    async fn status(&self, namespace: &str, name: &str) -> Result<EngineStatus, String>;
    async fn update(&self, req: &EngineSpec) -> Result<(), String>;
    async fn delete(&self, namespace: &str, name: &str) -> Result<(), String>;
    async fn restart(&self, namespace: &str, name: &str) -> Result<(), String>;
}
```
`EngineSpec`/`EngineStatus` already exist in this crate. Confirm no `kube` reference.

- [ ] **Step 3: Build + commit**

`cargo build -p egret-engine-mgmt-api` → clean.
```bash
git add crates/egret-engine-mgmt-api
git commit -m "feat(engine-mgmt-api): kube-free EngineProvisioningPort" # + Co-Authored-By trailer
```

---

## Task 2: `egret-engine-mgmt` deps + seam HTTP error

**Files:** `crates/egret-engine-mgmt/Cargo.toml`, `crates/egret-engine-mgmt/src/error.rs`, `src/lib.rs`

- [ ] **Step 1: Add the real dependency set**

To `crates/egret-engine-mgmt/Cargo.toml` `[dependencies]` add (promote `egret-catalog` from dev to real): `egret-engine-mgmt-api.workspace = true`, `egret-auth.workspace = true`, `egret-catalog.workspace = true`, `egret-secrets.workspace = true`, `egret-authz.workspace = true`, `egret-core.workspace = true`, `egret-orchestrator.workspace = true`, `axum = { workspace = true, features = ["ws"] }`, `async-trait.workspace = true`, `bytes.workspace = true`, `futures-util.workspace = true`, `tokio.workspace = true`, `serde.workspace = true`, `base64.workspace = true`, `reqwest.workspace = true`, `bcrypt = "0.15"`, `tracing.workspace = true`, `kube.workspace = true`. (Remove `egret-catalog` from `[dev-dependencies]` if it duplicates.)

- [ ] **Step 2: HTTP-facing seam error**

`egret-engine-mgmt` already has `EngineMgmtError` (domain, E1b). Add an HTTP error the handlers return. Create `crates/egret-engine-mgmt/src/http_error.rs` by copying `egret-controlplane/src/error.rs`'s `CpError` enum + its `IntoResponse` + the `From` impls the engine code needs — keep exactly the variants `engines.rs`/`monitor.rs` use: `Forbidden`, `NotFound`, `AlreadyExists(String)`, `BadRequest(String)`, `Conflict(String)`, `InUse(i64)`, `BadGateway`, `Internal`. Rename the type to `EngineHttpError`. Keep the `From<egret_catalog::CatalogError>`, `From<egret_secrets::SecretsError>`, `From<egret_authz::AuthzError>`, `From<egret_core::Error>` impls (copy verbatim from CpError), and ADD `From<crate::EngineMgmtError>` (AlreadyExists→AlreadyExists, Db→Internal). Wire `pub mod http_error; pub use http_error::EngineHttpError;` in `lib.rs`.
> This mirrors control-plane's mapping exactly, so behavior (status codes + bodies) is unchanged. `require_permission` returns `egret_core::Error::Forbidden` (from `egret-auth`, post-P5) → `EngineHttpError::Forbidden` via the copied `From<egret_core::Error>`.

- [ ] **Step 3: Build + commit**

`cargo build -p egret-engine-mgmt` → clean.
```bash
git add crates/egret-engine-mgmt
git commit -m "feat(engine-mgmt): seam deps + EngineHttpError (mirrors CpError mapping)" # +trailer
```

---

## Task 3: Move `engines.rs` + define `EngineMgmtState`

**Files:** `git mv crates/egret-controlplane/src/engines.rs crates/egret-engine-mgmt/src/engines.rs`; `crates/egret-engine-mgmt/src/state.rs` (new), `src/lib.rs`.

- [ ] **Step 1: Relocate the file**

`git mv crates/egret-controlplane/src/engines.rs crates/egret-engine-mgmt/src/engines.rs`. Add `pub mod engines;` to the seam `lib.rs`.

- [ ] **Step 2: `EngineMgmtState`**

`crates/egret-engine-mgmt/src/state.rs`:
```rust
use std::sync::Arc;
use axum::extract::FromRef;
use egret_auth::session::SessionService;
use egret_engine_mgmt_api::EngineProvisioningPort;
use crate::EngineMgmtStore;

#[derive(Clone)]
pub struct EngineMgmtState {
    pub engine_mgmt: EngineMgmtStore,
    pub storage: egret_catalog::CatalogStore,        // cross-seam: auto-wiring
    pub authz: egret_authz::AuthzStore,              // cross-seam: Ranger auto-bind
    pub secrets: egret_secrets::WorkspaceSecretStore,// secret resolution
    pub sessions: SessionService,                    // Auth extractor
    pub engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    pub kube: Option<Arc<dyn egret_orchestrator::KubeClient>>,
    pub engine_egret_url: String,
    pub engine_jwks_uri: String,
}

impl FromRef<EngineMgmtState> for SessionService {
    fn from_ref(s: &EngineMgmtState) -> Self { s.sessions.clone() }
}
```
Wire `pub mod state; pub use state::EngineMgmtState;` in `lib.rs`.

- [ ] **Step 3: Adapt `engines.rs`**

In the moved `engines.rs`, apply these mechanical swaps (compiler-driven — build and fix what it flags):
- Imports: delete `use crate::auth::require_permission;` → `use egret_auth::require_permission;`. `use crate::error::CpError;` → `use crate::EngineHttpError as CpError;` (aliasing to `CpError` keeps the ~hundreds of `CpError::X` bodies unchanged — lowest-diff). `use crate::CpState;` → `use crate::EngineMgmtState as CpState;` (same aliasing trick keeps `State<CpState>` handler signatures unchanged). `egret_engine_mgmt::{Engine, EngineCatalog}` → `crate::{Engine, EngineCatalog}` (now native). Keep `egret_catalog::service::embedded::{GrantInput, NewClient}`.
- Delete the `EngineProvisioner` trait definition (engines.rs:76-146) — it moves to the Port (Task 1) / KubeClient (already exists). Import `use egret_engine_mgmt_api::EngineProvisioningPort;`.
- The `pub use egret_engine_mgmt_api::{...}` alias block (engines.rs:53-70): keep as-is (still resolves; minimal-diff).
- **exec/log/coordinator split** (the one non-mechanical part): in `pod_logs`, `pod_logs_follow`, `pod_exec` handlers, the `prov.status(...)` pod-ownership check stays on `st.engine_provisioner` (the Port); the actual `prov.pod_logs(...)`/`prov.pod_logs_stream(...)`/`prov.exec(...)` calls become `st.kube.as_ref().ok_or(CpError::BadRequest("kubernetes not configured".into()))?.<method>(...)`. `bridge_log_follow`/`bridge_exec` change their `prov: Arc<dyn EngineProvisioner>` param to `kube: Arc<dyn egret_orchestrator::KubeClient>`. Per the relocation map (§3), these handlers now read BOTH `st.engine_provisioner` (status) AND `st.kube` (the op).

- [ ] **Step 4: Build the seam crate**

`cargo build -p egret-engine-mgmt` → resolve compiler errors mechanically (the aliasing covers most; the exec/kube split + the `router()` return type `Router<CpState>`→`Router<EngineMgmtState>` via the alias are the substantive spots).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(engine-mgmt): move engines.rs in; EngineMgmtState + exec/log via KubeClient" # +trailer
```

---

## Task 4: Move `monitor.rs`

**Files:** `git mv crates/egret-controlplane/src/monitor.rs crates/egret-engine-mgmt/src/monitor.rs`.

- [ ] **Step 1: Relocate + wire**

`git mv` the file; add `pub mod monitor;` to the seam `lib.rs`.

- [ ] **Step 2: Adapt**

Same import swaps as Task 3 Step 3 (`crate::auth::require_permission`→`egret_auth::require_permission`, `crate::error::CpError`→`crate::EngineHttpError as CpError`, `crate::CpState`→`crate::EngineMgmtState as CpState`, `egret_engine_mgmt::Engine`→`crate::Engine`). `crate::engines::monitor_password` stays (sibling module). In `coordinator_call` (monitor.rs:216-235), change `st.engine_provisioner.as_ref()...coordinator_request(...)` → `st.kube.as_ref().ok_or(CpError::BadRequest("kubernetes not configured".into()))?.coordinator_request(...)`.

- [ ] **Step 3: Build + commit**

`cargo build -p egret-engine-mgmt` → clean.
```bash
git add -A && git commit -m "refactor(engine-mgmt): move monitor.rs in; coordinator via KubeClient" # +trailer
```

---

## Task 5: `engine_mgmt_router()`

**Files:** `crates/egret-engine-mgmt/src/lib.rs` (or keep `router()` in engines.rs, re-export).

- [ ] **Step 1: Expose the router**

The moved `engines::router()` (engines.rs:148-170, now returning `Router<EngineMgmtState>` via the alias) already merges `monitor::routes()`. Re-export it as the seam's public entry: in `lib.rs`, `pub use engines::router as engine_mgmt_router;`. Route paths are unchanged (`/engines`, `/engines/{id}`, …).

- [ ] **Step 2: Build + commit**

`cargo build -p egret-engine-mgmt` → clean.
```bash
git add crates/egret-engine-mgmt/src/lib.rs && git commit -m "feat(engine-mgmt): public engine_mgmt_router()" # +trailer
```

---

## Task 6: Mount in `egret-server`

**Files:** `crates/egret-server/Cargo.toml`, `crates/egret-server/src/lib.rs`.

- [ ] **Step 1: Dep + AppState field**

`egret-server/Cargo.toml`: add `egret-engine-mgmt.workspace = true`. In `AppState` (lib.rs:17-45) add `pub engine_mgmt: Option<egret_engine_mgmt::EngineMgmtState>,`; add `engine_mgmt: None,` to `without_db()` (lib.rs:49-65).

- [ ] **Step 2: Mount block**

In `build_router`, after the `control_plane` block (lib.rs:164-175), add:
```rust
    if let Some(em) = state.engine_mgmt.clone() {
        let group = Router::new()
            .nest("/api/control-plane", egret_engine_mgmt::engine_mgmt_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(em);
        router = router.merge(group);
    }
```

- [ ] **Step 3: Build + commit**

`cargo build -p egret-server` → clean.
```bash
git add crates/egret-server && git commit -m "feat(server): mount engine_mgmt router at /api/control-plane (URLs unchanged)" # +trailer
```

---

## Task 7: Rewire `egret-bin` + shrink `CpState`

**Files:** `crates/egret-bin/Cargo.toml`, `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Dep**

`egret-bin/Cargo.toml`: add `egret-engine-mgmt.workspace = true` and `egret-engine-mgmt-api.workspace = true` (if not present).

- [ ] **Step 2: Port impl struct**

Rename `EgretEngineProvisioner` → `EgretEngineProvisioningPort` and change its trait impl from `egret_controlplane::EngineProvisioner` to `egret_engine_mgmt_api::EngineProvisioningPort`, keeping ONLY the 5 methods (`provision`/`status`/`update`/`delete`/`restart` — bodies unchanged: provision/status/update/delete delegate to `self.inner`, restart calls `self.kube.restart_deployment` twice). DELETE the 4 methods `coordinator_request`/`pod_logs`/`pod_logs_stream`/`exec` from the impl. Signatures now use `egret_engine_mgmt_api::{EngineSpec, EngineStatus}` (the map calls them `EngineProvisionReq`/`EngineLiveStatus` — those are the same aliased types; use the canonical `EngineSpec`/`EngineStatus`).

- [ ] **Step 3: Build `EngineMgmtState`, drop engine fields from `CpState`**

At main.rs:1166-1204, split the kube resolution so both `kube: Option<Arc<dyn KubeClient>>` and `engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>` are available (per the map §7). Construct `AppState.engine_mgmt = Some(egret_engine_mgmt::EngineMgmtState { engine_mgmt: EngineMgmtStore::new(db.clone()), storage: CatalogStore::new(db.clone()), authz: AuthzStore::new(db.clone()), secrets: workspace_secrets.clone(), sessions: sessions.clone(), engine_provisioner, kube, engine_egret_url: <same config expr as today>, engine_jwks_uri: <same> })`. In the `CpState { … }` construction (main.rs:1193-1222), REMOVE `engine_mgmt`, `engine_provisioner`, `engine_egret_url`, `engine_jwks_uri`.

- [ ] **Step 4: Build**

`cargo build -p egret-bin` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bin && git commit -m "feat(bin): wire EngineMgmtState; provisioner impls the api Port" # +trailer
```

---

## Task 8: Delete engine residue from `egret-controlplane`

**Files:** `crates/egret-controlplane/src/lib.rs`, `Cargo.toml`.

- [ ] **Step 1: lib.rs**

Remove `mod engines;` (lib.rs:14), `pub mod monitor;` (lib.rs:16), the `pub use engines::{…}` block (lib.rs:23-32), and the `.merge(engines::router())` line in `control_plane_router()` (lib.rs:~102). In `CpState`, remove the `engine_mgmt`/`engine_provisioner`/`engine_egret_url`/`engine_jwks_uri` fields (they moved).

- [ ] **Step 2: Cargo.toml — drop now-unused deps**

Remove (verify each with a grep across the remaining `egret-controlplane/src` modules first — storage/secrets/warehouses/policies/access — that nothing else uses it): `kube`, `k8s-openapi` (dev), `egret-engine-mgmt`, `egret-engine-mgmt-api`, `bcrypt`, and the `ws` feature on axum + `bytes`/`futures-util` if unused. KEEP `async-trait` (WarehouseProvisioner/Browser), `egret-engine-mgmt` — WAIT: `CpState` no longer holds `engine_mgmt`, so `egret-engine-mgmt` dep can go IF no other controlplane module references it (grep). The compiler + `cargo build -p egret-controlplane` is the final arbiter — remove a dep, rebuild, restore only if it breaks.

- [ ] **Step 3: Build + commit**

`cargo build -p egret-controlplane` → clean.
```bash
git add crates/egret-controlplane && git commit -m "refactor(controlplane): drop engine domain (moved to egret-engine-mgmt seam)" # +trailer
```

---

## Task 9: Move the tests + fix the drift-lint

**Files:** `crates/egret-controlplane/tests/routes_api.rs`, `crates/egret-engine-mgmt/tests/routes.rs` (new), `crates/egret-controlplane/tests/catalog_drift.rs`.

- [ ] **Step 1: Move the engine tests**

Move the ~17 engine/monitor tests (relocation map §9 lists them by name+line) + the `FakeEngineProvisioner`/`MonitorFakeProvisioner` helpers from `egret-controlplane/tests/routes_api.rs` into a new `crates/egret-engine-mgmt/tests/routes.rs`. The fakes split: a Port fake implementing `EngineProvisioningPort` (5 methods) for provision/status/update/delete/restart, and use `egret_orchestrator::FakeKube` (already exists) for the `kube` field's exec/log/coordinator. The fixture builds `EngineMgmtState` (not `CpState`) with `storage`/`authz`/`secrets`/`sessions` wired (cross-seam, same DB as today). Leave the non-engine tests (storage/secrets/browse/policy/access/users/groups/roles) in `routes_api.rs`.

- [ ] **Step 2: Fix `catalog_drift.rs`**

In `crates/egret-controlplane/tests/catalog_drift.rs`, remove `"engines.rs"`/`"monitor.rs"` from `CONTROLPLANE_FILES` (lines 20-28) and add `"../egret-engine-mgmt/src/engines.rs"` + `"../egret-engine-mgmt/src/monitor.rs"` to `EXTRA_FILES` (line 34) — matching the existing cross-crate-file precedent.

- [ ] **Step 3: Test + commit**

`cargo test -p egret-engine-mgmt -p egret-controlplane` → PASS (needs the migrated DB).
```bash
git add -A && git commit -m "test: move engine route tests to egret-engine-mgmt; fix drift-lint paths" # +trailer
```

---

## Task 10: Workspace gate + extraction proof

**Files:** none.

- [ ] **Step 1: fmt + clippy**

`cargo fmt --all --check` (fmt + `style:` if needed). `cargo clippy -p egret-engine-mgmt-api -p egret-engine-mgmt -p egret-controlplane -p egret-server -p egret-bin --all-targets -- -D warnings` → clean (bar vendored mysql_async).

- [ ] **Step 2: Workspace build + tests**

`cargo build --workspace` → clean. `cargo test -p egret-engine-mgmt -p egret-controlplane -p egret-server` → PASS.

- [ ] **Step 3: Extraction proof**

- `grep -rn "engines\|monitor\|EngineProvisioner\|orchestrator\|kube" crates/egret-controlplane/src` → no engine/kube references (only unrelated hits, if any; report).
- `grep -n "kube" crates/egret-engine-mgmt-api/Cargo.toml` → nothing (Port crate stays kube-free).
- URL parity: the moved tests assert the same `/api/control-plane/engines/*` paths and pass.
- `git diff --stat main..HEAD -- frontend/` → empty.
Report all four.

- [ ] **Step 4: Final `style:` commit if needed**

```bash
git add -A && git commit -m "style: cargo fmt after engine seam extraction" || echo "nothing to format"
```

---

## Self-review notes

- **Spec coverage:** Port (T1) §2.1; seam deps+error (T2) §2.2; move engines+state+exec/kube (T3) §2.2-2.4; monitor (T4); router (T5) §3; mount (T6) §3; egret-bin+CpState shrink (T7) §5+§4; controlplane residue (T8) §4; tests+drift (T9) §7; gate+proof (T10) §7.
- **Behavior/URL preservation:** files move verbatim (aliasing `EngineHttpError as CpError`, `EngineMgmtState as CpState` keeps bodies unchanged); `EngineHttpError` mirrors `CpError`'s mapping; routes nest at the same prefix; the only semantic change is the exec/log/coordinator calls routing to `st.kube` while the pod-ownership `status` check stays on the Port.
- **Placeholder scan:** no `todo!()`; the compiler-driven "fix what it flags" steps are bounded mechanical import/type swaps with the aliasing making them minimal.
- **Risk spots:** (1) the exec/kube split is the only non-mechanical edit — the pod-ownership check MUST stay before the WS upgrade (security); T3 Step 3 + map §3 pin the exact sites. (2) Dep removal in T8 is compiler-verified (remove→rebuild→restore-if-broken). (3) the two-router-at-`/api/control-plane` merge is the established egret-server pattern (multiple `/api` nests merge). (4) test fakes split into a Port fake + `FakeKube` — T9 Step 1.
