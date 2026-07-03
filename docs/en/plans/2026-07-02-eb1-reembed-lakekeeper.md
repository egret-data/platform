# EB-1: re-embed lakekeeper (AllowAll) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md` (Component 1). **EB-1 of the embedded-catalog epic.** VI: `docs/vi/plans/2026-07-02-eb1-reembed-lakekeeper.md`.

**Goal:** Bring the embedded lakekeeper Iceberg REST catalog back into egret (in-process, mounted at `/catalog/v1/*`) with an **AllowAll authorizer** and **no legacy auth** (the JWT auth + real Authorizer come in EB-2/EB-3). Remove Polaris from the dev compose.

**Architecture:** Recover the `egret-catalog-engine` (+ `egret-iceberg-ext`) crates deleted in PR #115 from git (`26a13f4~1`), strip their legacy auth surface (front_auth opaque-tokens, browse, provisioning, mint-token), keep the lakekeeper mount + lakekeeper migrations + the pluggable `Authorizer` seam wired to an AllowAll impl, and nest `/catalog/v1` in egret-server. lakekeeper reuses the existing Postgres + MinIO.

**Tech Stack:** Rust (axum, sqlx, git-pinned lakekeeper).

---

## Background the implementer needs

- The crates + their deps/`[patch]` existed at **`26a13f4~1`** (old main before the epic). PR #115 (`26a13f4`) removed them. Recover from that ref.
- The old crate mounted lakekeeper via `mount_catalog_engine` and ran `run_lakekeeper_migrations` (into a `lakekeeper` Postgres schema); it also had `front_auth.rs` (opaque-token auth — DROP), `browse.rs` (storage/secrets browse — DROP; those live in control-plane now), `provisioning.rs` (DROP). Keep the lakekeeper server assembly + the `Authorizer` trait usage.
- The old `AppState` had `catalog_engine`/`catalog_browse` routers + a `/catalog/v1` nest (removed in P1); recover the `catalog_engine` nest only.
- Migration 0048 (merged) dropped the `lakekeeper` schema + `catalog_tokens`/`workspace_warehouses`. lakekeeper will re-create its own schema on boot via its migrations. Do NOT re-add `catalog_tokens`/`workspace_warehouses` (superseded by EB-3's tables).

---

## Task 1: Recover the crates + workspace deps

**Files:** `crates/egret-catalog-engine/`, `crates/egret-iceberg-ext/` (recover); root `Cargo.toml`

- [ ] **Step 1: Recover files**

```bash
git checkout 26a13f4~1 -- crates/egret-catalog-engine crates/egret-iceberg-ext
```
Then recover the root-`Cargo.toml` dependency + `[patch]` lines for lakekeeper/iceberg-rust that these crates need — inspect them at the old ref and re-apply to the current root `Cargo.toml`:
```bash
git show 26a13f4~1:Cargo.toml | grep -nE "lakekeeper|iceberg|catalog-engine|iceberg-ext|\[patch"
```
Re-add the `[workspace.dependencies]` entries for `egret-catalog-engine`/`egret-iceberg-ext` and any lakekeeper/iceberg git pins + `[patch."https://github.com/lakekeeper/lakekeeper"]` block.

- [ ] **Step 2: Build the crate in isolation**

`cargo build -p egret-iceberg-ext -p egret-catalog-engine` → resolve deps until it compiles. (The `.sqlx-lakekeeper/` offline cache is recovered too; keep it for lakekeeper's compile-time queries.)

- [ ] **Step 3: Commit the raw recovery**
```bash
git add crates/egret-catalog-engine crates/egret-iceberg-ext Cargo.toml Cargo.lock
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "chore(catalog): recover egret-catalog-engine + egret-iceberg-ext from pre-#115"
```
(Co-Authored-By trailer on every commit.)

---

## Task 2: Strip legacy auth; wire an AllowAll authorizer

**Files:** `crates/egret-catalog-engine/src/*`

- [ ] **Step 1: Remove the legacy surface**

`git rm` `front_auth.rs`, `browse.rs`, `provisioning.rs` (+ their tests). In `lib.rs`: remove their `mod`/`pub use`; keep the lakekeeper assembly (`build_state`/`mount_catalog_engine`/`run_lakekeeper_migrations`/`ensure_bootstrapped`/gc workers) + the `Authorizer` wiring. Remove `catalog_public_url`/response-rewrite/`warehouse_prefix` (proxy-era hacks — not needed in-process).

- [ ] **Step 2: AllowAll authorizer**

Find lakekeeper's `Authorizer` trait (the seam the old `EgretAuthorizer` implemented). Provide a minimal `AllowAllAuthorizer` (every check → allow) and use it in `mount_catalog_engine` for now. (EB-3 replaces it with the real egret Authorizer.) If lakekeeper ships its own `AllowAllAuthorizer`, use that.

- [ ] **Step 3: Build + commit**

`cargo build -p egret-catalog-engine` → clean.
```bash
git add crates/egret-catalog-engine && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "refactor(catalog): strip legacy front_auth/browse/provisioning; AllowAll authorizer"
```

---

## Task 3: Mount `/catalog/v1` in egret-server

**Files:** `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Recover the assembly + nest**

In `egret-bin/src/main.rs`: recover (from `26a13f4~1:crates/egret-bin/src/main.rs`) the catalog-engine assembly — `run_lakekeeper_migrations` + `build_state` + `ensure_bootstrapped` + `mount_catalog_engine` (with the AllowAll authorizer) + gc workers — adapted to current state; produce a `catalog_engine: Option<Router>`. Drop the `catalog_browse` router (browse is gone). In `egret-server/src/lib.rs`: add `catalog_engine: Option<Router>` back to `AppState` (+ `None` in `without_db()`) and nest it at `/catalog/v1` when `Some` (recover the old nest; **no** CSRF — machine API).

- [ ] **Step 2: Build**

`cargo build --workspace` → 0 errors. Add `egret-catalog-engine` back to `egret-bin` deps.

- [ ] **Step 3: Commit**
```bash
git add crates/egret-server crates/egret-bin && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(catalog): mount embedded lakekeeper at /catalog/v1 (AllowAll)"
```

---

## Task 4: Remove Polaris from dev compose

**Files:** `deploy/docker-compose.dev.yml`

- [ ] **Step 1:** Remove the `polaris` service (added in PR #115). lakekeeper is embedded now + uses the existing `postgres` + `minio`. Keep those.
- [ ] **Step 2: Commit**
```bash
git add deploy/docker-compose.dev.yml && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "chore(dev): remove Polaris service (lakekeeper is embedded)"
```

---

## Task 5: Verify lakekeeper serves in-process

- [ ] **Step 1:** With Postgres + MinIO up + serve running: `curl -s http://localhost:8080/catalog/v1/config` → a JSON config (lakekeeper responding in-process). Check serve logs for lakekeeper migration + bootstrap success. (AllowAll → no token needed yet.)
- [ ] **Step 2: CI:** `cargo fmt --all --check` + `cargo clippy -p egret-catalog-engine -p egret-server -p egret-bin --all-targets -- -D warnings` + `cargo build --workspace`.
- [ ] **Step 3:** commit fixups.

---

## Self-review notes (spec coverage for EB-1)

- Spec **Component 1**: embedded lakekeeper mounted at `/catalog/v1` in-process (Task 3), legacy auth stripped + AllowAll (Task 2), Polaris removed from compose (Task 4), serving verified (Task 5).
- **Out of EB-1:** mini-IdP + clients (EB-2), real Authorizer + warehouses/roles/grants + JWT authN (EB-3), control-plane mgmt + egret-polaris revert (EB-4), UI (EB-5), e2e (EB-6).
