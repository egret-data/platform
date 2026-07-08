# E1a — Collapse the engine-spec model into `egret-engine-mgmt-api` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the triplicated engine-spec model — control-plane's ~28 kube-free `*Req` mirror structs, `egret-bin`'s ~400-line `spec_from` translator, and their orchestrator twins — by moving ONE canonical copy into a new dependency-free leaf crate `egret-engine-mgmt-api` that both `egret-controlplane` and `egret-orchestrator` import.

**Architecture:** This is the first slice of the E1 (Engine-provisioning) seam extraction — the contract-crate seed (DTOs only; the `Port` trait comes in a later E1 slice). A [de-risk investigation](../architecture/2026-07-08-project-review-audit.md) proved this is a **clean relocate**: the ~28 struct pairs are field-identical; `spec_from` + its ~20 helpers + the reverse status mapping are pure 1:1 copies with zero transforms; and **neither model derives serde** (the FE wire contract lives entirely in hand-written `parse_*` functions in `engines.rs`, which are orthogonal to which struct they populate and are NOT touched here). Canonical types get the merged derive set `Clone, Debug, Default` (nested) / `Clone, Debug` (top-level).

**Tech Stack:** Rust 2021 workspace. `egret-engine-mgmt-api` is a **leaf crate with no dependencies** (the DTOs need only std derives). Design basis: [service-ready architecture §3.1/§4.1](../architecture/2026-07-08-service-ready-architecture-design.md), E1 row of §5.

**Key design decision (deviates from the de-risk report's suggestion):** the canonical types live in a **new leaf crate `egret-engine-mgmt-api`**, NOT in `egret-orchestrator`. If control-plane imported them from `egret-orchestrator`, it would transitively pull `kube`/`k8s-openapi` — defeating the very reason the kube-free `*Req` mirrors exist. A zero-dep leaf keeps both sides clean and seeds the E1 contract crate.

**Scope — IN:** unify the DTO model; delete the mirrors + `spec_from`. **OUT (do NOT do here):** touching the `parse_*`/`build_provision_req`/`resolve_*` logic (only the *target type name* they construct changes); switching `parse_*` to `#[derive(Deserialize)]` (several have non-derivable semantics — caller-supplied defaults, present-but-wrong-shape→None, null-filtering, the `build_trino_config` pre-1.5.1 back-compat branch); unifying the `EngineProvisioner` traits (a later E1 slice); any schema/storage change.

---

## File structure

- `crates/egret-engine-mgmt-api/` — **new leaf crate**. `Cargo.toml` (no deps) + `src/lib.rs` holding the canonical spec DTOs (moved verbatim from `egret-orchestrator/src/spec.rs`, which already carries the superset derive set).
- `Cargo.toml` (workspace root) — register `egret-engine-mgmt-api` in `[workspace.dependencies]`.
- `crates/egret-orchestrator/{Cargo.toml, src/spec.rs, src/lib.rs}` — depend on the api crate; `spec.rs` becomes a re-export of the api types (or is deleted and callers import from the api crate).
- `crates/egret-controlplane/{Cargo.toml, src/engines.rs, src/lib.rs}` — depend on the api crate; delete the ~28 `*Req` structs + envelope types (`EngineProvisionReq`/`EnginePod`/`EngineLiveStatus`); re-point `parse_*`/`build_provision_req`/`resolve_*`/`inject_monitor_user` and the `lib.rs` re-exports at the api types.
- `crates/egret-bin/src/main.rs` — delete `spec_from` + its ~20 `*_from` helpers + the `status()` reverse-mapping block; the `EngineProvisioner` impl passes the canonical type straight through to `self.inner`.

---

## Task 1: Create the `egret-engine-mgmt-api` leaf crate

**Files:**
- Create: `crates/egret-engine-mgmt-api/Cargo.toml`, `crates/egret-engine-mgmt-api/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create the crate manifest**

`crates/egret-engine-mgmt-api/Cargo.toml`:
```toml
[package]
name = "egret-engine-mgmt-api"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
```
(No dependencies — the DTOs use only `Clone`/`Debug`/`Default` std derives.)

- [ ] **Step 2: Move the canonical DTOs into `src/lib.rs`**

Copy the ENTIRE type-definition body of `crates/egret-orchestrator/src/spec.rs` into `crates/egret-engine-mgmt-api/src/lib.rs` (all the structs: `Toleration`, `TopologySpreadConstraint`, `NodeSelectorRequirement`, `Affinity`, `ContainerPort`, `SecurityContext`, `VolumeMount`, `MiniContainer`, `EnvFromSource`, `ObjectMount`, `DeploymentStrategy`, `Probe`, `Probes`, `HeapConfig`, `RoleResources`, `RoleConfig`, `WorkerRoleConfig`, `ResourceSelector`, `SessionRule`, `ResourceGroup`, `ResourceGroups`, `FaultTolerant`, `GracefulShutdown`, `PasswordEntry`, `Jwt`, `Auth`, `Ingress`, the KEDA/HPA/autoscaling types, `TrinoConfig`, `CatalogSpec`, `EngineSpec`, `PodStatus`, `EngineStatus`). Keep their existing derives exactly (`Clone, Debug, Default` on the carriers; `Clone, Debug` on `CatalogSpec`/`EngineSpec`). Prepend a module doc:
```rust
//! Canonical, dependency-free engine-provisioning spec DTOs (the E1 contract-crate
//! seed). Both `egret-controlplane` (HTTP + secret resolution) and
//! `egret-orchestrator` (k8s rendering) depend on these — neither owns the model.
//! Kube-free by construction: this crate has no dependencies.
```

- [ ] **Step 3: Register in the workspace**

In the root `Cargo.toml` `[workspace.dependencies]`, add (keep alphabetical-ish with the others):
```toml
egret-engine-mgmt-api = { path = "crates/egret-engine-mgmt-api" }
```

- [ ] **Step 4: Build the new crate**

Run: `cargo build -p egret-engine-mgmt-api`
Expected: clean (a leaf of pure types, no deps).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt-api Cargo.toml
git commit -m "feat(engine-mgmt-api): new leaf crate with the canonical engine-spec DTOs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Point `egret-orchestrator` at the api crate

**Files:**
- Modify: `crates/egret-orchestrator/Cargo.toml`, `crates/egret-orchestrator/src/spec.rs`, `crates/egret-orchestrator/src/lib.rs`

- [ ] **Step 1: Add the dependency**

In `crates/egret-orchestrator/Cargo.toml` `[dependencies]`, add:
```toml
egret-engine-mgmt-api = { workspace = true }
```

- [ ] **Step 2: Replace `spec.rs`'s definitions with a re-export**

Overwrite `crates/egret-orchestrator/src/spec.rs` with a single re-export so every existing `crate::spec::X` / `super::spec::X` path in orchestrator keeps resolving unchanged:
```rust
//! Engine-provisioning spec types. The definitions now live in the dependency-free
//! `egret-engine-mgmt-api` contract crate; re-exported here so orchestrator's
//! manifest builders keep referring to `crate::spec::*`.

pub use egret_engine_mgmt_api::*;
```

- [ ] **Step 3: Build + test orchestrator**

Run: `cargo test -p egret-orchestrator`
Expected: PASS — `trino.rs`'s manifest builders and the 40 unit tests compile against the re-exported types unchanged (they are byte-identical). If any test references a type by a path other than `crate::spec::*` (e.g. a direct `egret_orchestrator::spec::X`), it still resolves via the glob re-export.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-orchestrator/Cargo.toml crates/egret-orchestrator/src/spec.rs
git commit -m "refactor(orchestrator): source engine-spec DTOs from egret-engine-mgmt-api

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Replace control-plane's `*Req` mirrors with the api types

This deletes the ~28 kube-free mirror structs (the bulk of the duplication) and re-points the JSON parsers at the canonical types. **Do not change any `parse_*` logic** — only the type each constructs.

**Files:**
- Modify: `crates/egret-controlplane/Cargo.toml`, `crates/egret-controlplane/src/engines.rs`, `crates/egret-controlplane/src/lib.rs`

- [ ] **Step 1: Add the dependency**

In `crates/egret-controlplane/Cargo.toml` `[dependencies]`, add:
```toml
egret-engine-mgmt-api = { workspace = true }
```

- [ ] **Step 2: Delete the mirror structs and import the canonical types**

In `crates/egret-controlplane/src/engines.rs`:
- Delete the ~28 `*Req` struct definitions (roughly lines 48–421: `CatalogReq`, `HeapReq`, `RoleResourcesReq`, `RoleConfigReq`, `WorkerRoleConfigReq`, `ProbeReq`, `ProbesReq`, `ResourceGroupReq`, `ResourceSelectorReq`, `ResourceGroupsReq`, `SessionRuleReq`, `FaultTolerantReq`, `GracefulShutdownReq`, `PasswordEntryReq`, `JwtReq`, `AuthReq`, `IngressReq`, `TolerationReq`, `TopologySpreadConstraintReq`, `NodeSelectorRequirementReq`, `AffinityReq`, `ContainerPortReq`, `SecurityContextReq`, `VolumeMountReq`, `MiniContainerReq`, `EnvFromSourceReq`, `ObjectMountReq`, `DeploymentStrategyReq`, the KEDA/HPA/autoscaling `*Req` types, `TrinoConfigReq`) AND the envelope types `EngineProvisionReq`, `EnginePod`, `EngineLiveStatus`.
- Add an import bringing the canonical types into scope under the OLD names, so the `parse_*`/`build_provision_req` bodies (which construct e.g. `TrinoConfigReq { .. }`) need only minimal edits. Prefer aliasing to the old names to minimise churn:
```rust
use egret_engine_mgmt_api::{
    Affinity as AffinityReq, Auth as AuthReq, /* … */,
    CatalogSpec as CatalogReq, EngineSpec as EngineProvisionReq,
    HeapConfig as HeapReq, /* … one alias per old *Req name … */,
    PodStatus as EnginePod, EngineStatus as EngineLiveStatus,
    TrinoConfig as TrinoConfigReq,
};
```
> Aliasing keeps every `SomethingReq { .. }` constructor and every `-> SomethingReq` signature in `engines.rs` compiling with no body edits. (The field names are identical, so struct-literal construction is unaffected.) If the implementer prefers, rename the constructors to the canonical names instead — either is acceptable; aliasing is the lower-churn path.

- [ ] **Step 3: Fix the `lib.rs` re-exports**

In `crates/egret-controlplane/src/lib.rs`, the re-export block (around L23–32) currently re-exports the deleted `*Req`/envelope types from `engines`. Re-point it to re-export from the api crate (or drop the now-defunct ones). Ensure `EngineProvisioner` (the trait, still defined in `engines.rs`) is still exported. Check `cargo build -p egret-controlplane` output and fix exactly the names the compiler flags — do not guess.

- [ ] **Step 4: Build control-plane**

Run: `cargo build -p egret-controlplane`
Expected: clean. Fix any residual references (the aliases in Step 2 should cover the bodies; the compiler will name any straggler).

- [ ] **Step 5: Check the test mocks**

Run: `grep -n "EngineProvisionReq\|EngineProvisioner" crates/egret-controlplane/tests/routes_api.rs`
The two mock `EngineProvisioner` impls reference `EngineProvisionReq` in the trait signature. Confirm they reference it opaquely (by type in the signature) and don't destructure its fields. If they only name the type, they compile via the trait's now-canonical signature; if they destructure fields, apply the same type-name swap. Report which case held.

- [ ] **Step 6: Test control-plane**

Run: `cargo test -p egret-controlplane`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-controlplane/Cargo.toml crates/egret-controlplane/src/engines.rs crates/egret-controlplane/src/lib.rs
git commit -m "refactor(controlplane): drop *Req mirrors, use egret-engine-mgmt-api DTOs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Delete the `spec_from` bridge in `egret-bin`

With one shared type, the A→B translation is gone: control-plane's `EngineProvisioner::provision(&self, req: &EngineProvisionReq)` and orchestrator's `TrinoProvisioner::provision(&self, spec: &EngineSpec)` now take the SAME type, so the impl passes it straight through.

**Files:**
- Modify: `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Delete the translator + helpers**

In `crates/egret-bin/src/main.rs`, delete `spec_from` and all ~20 `*_from` helper fns (roughly lines 405–811) and the `status()` reverse-mapping block (`EngineStatus`→`EngineLiveStatus`, `PodStatus`→`EnginePod`, ~lines 826–842). In the `impl egret_controlplane::EngineProvisioner for EgretEngineProvisioner` block, change each delegating call from `self.inner.provision(&Self::spec_from(req))` (and update/delete/status) to pass the argument through directly: `self.inner.provision(req)` etc. For `status()`, return the orchestrator status directly (now the same type) instead of reverse-mapping.

- [ ] **Step 2: Build egret-bin**

Run: `cargo build -p egret-bin`
Expected: clean. The compiler will flag any lingering `Self::x_from` reference or type mismatch — resolve by passing the shared type through. If `provision`/`update` needed `&EngineSpec` but control-plane hands `&EngineProvisionReq`, they are now the same aliased type, so it matches; if a borrow/ownership mismatch appears, adjust the reference (`&req` vs `req`) minimally.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-bin/src/main.rs
git commit -m "refactor(bin): delete spec_from bridge — one shared engine-spec type

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Workspace gate + dedup proof

**Files:** none (verification only).

- [ ] **Step 1: Format + clippy**

Run: `cargo fmt --all --check` (fmt + `style:` commit if needed).
Run: `cargo clippy -p egret-engine-mgmt-api -p egret-orchestrator -p egret-controlplane -p egret-bin --all-targets -- -D warnings`
Expected: clean (ignore the pre-existing vendored `mysql_async` lint).

- [ ] **Step 2: Workspace build + tests for the affected crates**

Run: `cargo build --workspace`
Expected: clean.
Run: `cargo test -p egret-orchestrator -p egret-controlplane`
Expected: PASS.

- [ ] **Step 3: Dedup proof**

Run: `grep -rn "struct .*Req" crates/egret-controlplane/src/engines.rs | wc -l`
Expected: 0 spec-mirror `*Req` structs remain (only genuine wire-DTO structs like `CreateEngineBody`/`AddCatalogBody` may remain — those are NOT part of the spec model; confirm by eye that no `TrinoConfigReq`/`HeapReq`/etc. survive).
Run: `grep -n "fn spec_from\|_from(" crates/egret-bin/src/main.rs | wc -l`
Expected: 0 (the `spec_from` family is gone).
Report both counts.

- [ ] **Step 4: Frontend unaffected**

No frontend change is expected — the FE wire contract is the camelCase string literals in `engines.rs`'s `parse_*` functions, which were not touched. Confirm `git diff --stat main..HEAD` shows NO `frontend/` files changed.

- [ ] **Step 5: Final `style:` commit if fmt changed anything**

```bash
git add -A && git commit -m "style: cargo fmt after engine-spec-model collapse" || echo "nothing to format"
```

---

## Self-review notes

- **Spec coverage:** creates the `egret-engine-mgmt-api` leaf (Task 1); orchestrator sources from it (Task 2); control-plane drops the ~28 mirrors + re-points parsers (Task 3); `spec_from` deleted (Task 4); gate + dedup proof (Task 5). Matches design §3.1/§4.1 for the E1 contract-crate seed, DTO-only.
- **Placeholder scan:** no `todo!()`. The one non-verbatim spot is the Step-2/3 alias `use` list ("one alias per old `*Req` name") — the implementer fills the full list mechanically from the deleted struct names; this is a rename map, not undefined logic, and the compiler enforces completeness.
- **Risk containment:** the de-risk report's four flagged risks are all handled — (1) `parse_*`→derive is explicitly OUT of scope; (2) field-order deltas are cosmetic (no serde); (3) derive-set upgrade is already satisfied by taking orchestrator's types as canonical; (4) the test-mock check is Task 3 Step 5.
- **Type consistency:** one canonical set (`egret_engine_mgmt_api::*`), aliased to the old control-plane names to keep bodies compiling; orchestrator re-exports the same via `crate::spec::*`. No name resolves to two definitions after the deletes.
