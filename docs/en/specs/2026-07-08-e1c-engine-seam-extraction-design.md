# E1c — Engine domain leaves control-plane (seam extraction) (design)

> **Status:** Design (approved to plan) · **Date:** 2026-07-08 · **Epic:** E1 (Engine-provisioning seam), final slice.
> **Follows:** E1a (spec-model collapse, #179), E1b (storage split, #180).
> **One-line goal:** Move the engine domain — the HTTP router (`engines.rs` + `monitor.rs`) and the provisioning contract — out of the `egret-controlplane` god-crate into the `egret-engine-mgmt` seam crate, behind a kube-free `EngineProvisioningPort` in `egret-engine-mgmt-api`. After this, the engine seam is fully extracted (owns its DTOs, data, HTTP, and contract) and control-plane no longer knows about engines.

---

## 1. Why & the resolved decisions

E1a/E1b gave the seam its DTOs and data. E1c gives it its **HTTP surface + provisioning contract**, completing the extraction and realizing the "control-plane becomes a thin aggregator" goal (§4.1) for the engine domain. Built on the three resolved E1 decisions:
1. **Secret-resolution** (`build_provision_req`/`resolve_fault_tolerant`/`resolve_auth`/`inject_monitor_user`/`parse_*`) is HTTP-layer policy → it **moves with the router** into `egret-engine-mgmt` (which gains an `egret-secrets` dep). The Port receives a fully-resolved `EngineSpec`.
2. **exec / pod-logs / coordinator-request** stay **out of the kube-free Port** → served via `orchestrator::KubeClient` held directly in the seam's state.
3. **Storage** is already the seam's (`EngineMgmtStore`, E1b); auto-wiring calls the catalog/policy seams cross-seam.

## 2. Components

### 2.1 `EngineProvisioningPort` (new, in `egret-engine-mgmt-api`)
The kube-free provisioning contract — 5 methods over the canonical DTOs (all already in this crate): `provision`, `status`, `update`, `delete`, `restart` (signatures taken verbatim from the relevant subset of today's `EngineProvisioner` trait, retyped over `EngineSpec`/`EngineStatus`). No `kube` types appear. This retires the 8-method `EngineProvisioner` trait currently in `egret-controlplane/src/engines.rs`; its `coordinator_request`/`pod_logs`/`pod_logs_stream`/`exec` methods do **not** join the Port (decision 2).

### 2.2 `egret-engine-mgmt` grows the HTTP surface
`engines.rs` (3.2k) + `monitor.rs` (389) move into the seam crate, becoming `engine_mgmt_router()` + `EngineMgmtState`. The crate's new dependency footprint (its real, honest deps — better here than in the god-crate): `egret-engine-mgmt-api` (Port + DTOs), `egret-catalog` (auto-wiring: `CatalogStore`, `GrantInput`, `NewClient` — cross-seam), `egret-authz` (Ranger auto-bind: `AuthzStore`, `seed_trino` — cross-seam), `egret-secrets` (resolution), `egret-orchestrator` (the `KubeClient` trait for exec/log/coordinator), `egret-auth` (the `Auth` extractor + `require_platform_admin` gate, now in `egret-auth` after P5), `egret-core`, `egret-metastore`, `axum`.

### 2.3 `EngineMgmtState`
The seam router's state:
```
EngineMgmtState {
    engine_mgmt: EngineMgmtStore,               // its own data (E1b)
    storage: egret_catalog::CatalogStore,        // cross-seam: auto-wiring
    authz: egret_authz::AuthzStore,              // cross-seam: Ranger auto-bind
    secrets: WorkspaceSecretStore,               // secret resolution
    provisioner: Option<Arc<dyn EngineProvisioningPort>>,  // kube-free Port (None → "k8s not configured")
    kube: Option<Arc<dyn egret_orchestrator::KubeClient>>, // exec/log/coordinator (decision 2)
    sessions: SessionService,                    // backs the Auth extractor
    engine_egret_url: String,
    engine_jwks_uri: String,
}
```
`FromRef<EngineMgmtState> for SessionService` so the `Auth` extractor resolves (mirrors `CpState`).

### 2.4 exec / logs / coordinator via `KubeClient`
The WS exec + log-follow bridges and `monitor.rs`'s coordinator query call `state.kube` directly (not the Port). The **pod-ownership check** (a requested pod must belong to the engine's live pods) is preserved exactly — it is a security boundary, not a functional detail. `monitor.rs`'s `coordinator_request` calls switch from `engine_provisioner.coordinator_request` to `kube.coordinator_request`.

## 3. Mounting — FE URLs unchanged

`egret-server`'s `AppState` gains `pub engine_mgmt: Option<egret_engine_mgmt::EngineMgmtState>`. `build_router` adds a block mirroring the control-plane one, nesting the seam router at the **same** prefix so URLs are byte-identical:
```rust
if let Some(em) = state.engine_mgmt.clone() {
    router = router.merge(
        Router::new()
            .nest("/api/control-plane", egret_engine_mgmt::engine_mgmt_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(em),
    );
}
```
`engine_mgmt_router()` defines `/engines`, `/engines/{id}`, `/engines/{id}/catalogs`, `/engines/{id}/pods/{pod}/logs|exec`, `/engines/{id}/restart`, and the monitor routes → nested at `/api/control-plane` yields the existing `/api/control-plane/engines/*` URLs. **No frontend change** (the FE clients in `controlPlane.ts`/`EngineDetail.tsx`/`PodLogsDialog.tsx`/`PodTerminalDialog.tsx` are untouched). Merging two routers at the same nest prefix is the pattern egret-server already uses (database/engine/bi all nest at `/api` and merge).

## 4. Control-plane shrinks (the aggregator payoff)

- `egret-controlplane/src/engines.rs` and `monitor.rs` are **deleted**; `control_plane_router()` drops the `engines::router()` merge; the `pub use engines::{…}` re-export block (already just the Port + it was DTOs, now Port only) is removed.
- `CpState` **loses** `engine_provisioner`, `engine_egret_url`, `engine_jwks_uri`, and the `engine_mgmt` field (added in E1b) — all move to `EngineMgmtState`.
- `egret-controlplane` loses its dependencies on `egret-orchestrator` (it never should have needed kube) and on the engine-provisioning surface; it keeps storage/secrets/warehouses/policies/access. This is ~3.6k LOC leaving the god-crate — the single biggest de-bloat of E1.

## 5. Wiring (`egret-bin`)

`egret-bin` builds `EngineMgmtState`: constructs `EgretEngineProvisioner` (over `orchestrator::TrinoProvisioner` + `KubeClient`) as `Arc<dyn EngineProvisioningPort>`, passes the same `Arc<dyn KubeClient>` into `state.kube`, wires `EngineMgmtStore`/`CatalogStore`/`AuthzStore`/secrets/sessions/urls. It sets `AppState.engine_mgmt` and removes the engine fields from the `CpState` construction. `EgretEngineProvisioner` implements the new 5-method Port (its `exec`/`pod_logs`/`coordinator_request` pass-throughs are dropped from the Port impl — the seam calls `KubeClient` for those directly).

## 6. Scope boundaries

- **IN:** `EngineProvisioningPort` in `-api`; move `engines.rs`+`monitor.rs` → `egret-engine-mgmt` as `engine_mgmt_router()`+`EngineMgmtState`; exec/log/coordinator via `KubeClient`; `AppState.engine_mgmt` + mount; shrink `CpState`/control-plane; egret-bin rewire. **Behavior + URLs unchanged.**
- **OUT:** any change to the provisioning/parsing/auto-wiring *logic* (a verbatim move, only the home + the provisioner-interface split change); the cross-seam catalog/authz calls remain direct (not yet abstracted behind their own Ports — that is E3's concern); any authz-model change (Effort B); frontend.

## 7. Verification

- **URL parity:** the control-plane integration tests (`egret-controlplane/tests/routes_api.rs`) that exercise `/api/control-plane/engines/*` move to `egret-engine-mgmt`'s tests and must pass against the same URLs; a route-list assertion confirms the paths are byte-identical.
- **Behavior:** engine CRUD + the 8-step `create_engine` auto-wiring + monitor + the WS exec/log bridges (with pod-ownership checks) behave identically.
- **De-bloat proof:** `egret-controlplane` no longer references `engines`/`monitor`/`EngineProvisioner`/`orchestrator` (grep); `egret-engine-mgmt-api` stays kube-free (no `kube` dep).
- **Gate:** fmt, clippy (the touched crates), `cargo build --workspace`, `cargo test` across `egret-engine-mgmt`, `egret-controlplane`, `egret-server`, `egret-bin`. No `frontend/` change.

## 8. Decisions locked

1. `EngineProvisioningPort` = **5 kube-free methods** in `egret-engine-mgmt-api`; exec/log/coordinator via `KubeClient` (not the Port).
2. `engines.rs`+`monitor.rs` **move whole** into `egret-engine-mgmt` (secret-resolution comes with them); the crate takes the honest deps (secrets, catalog, authz, orchestrator, auth).
3. Seam router **nests at `/api/control-plane`** via a new `AppState.engine_mgmt` → **FE URLs unchanged**.
4. `CpState`/control-plane **shrink** (lose the engine fields + orchestrator dep); the move is behavior- and URL-preserving.

---

## 9. Suggested implementation decomposition (for the plan)

E1c is large (~3.6k LOC relocating). A sensible task order, each independently buildable: **(1)** define `EngineProvisioningPort` in `-api`; **(2)** create `EngineMgmtState` + move `engines.rs` into `egret-engine-mgmt` (re-point store calls, split exec/log onto `KubeClient`); **(3)** move `monitor.rs` (coordinator via `KubeClient`); **(4)** `engine_mgmt_router()` + `AppState.engine_mgmt` + `build_router` mount; **(5)** egret-bin rewire + shrink `CpState`; **(6)** delete the control-plane engine code + drop its `orchestrator` dep; **(7)** move the integration tests + gate. If it proves too large for one plan, split at task 4 (Port + move) / (mount + rewire + delete).

---

*Design for E1c. Next: a bite-sized implementation plan (likely decomposed per §9), then subagent-driven execution.*
