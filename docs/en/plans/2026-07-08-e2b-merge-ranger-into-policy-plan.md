# E2b — Merge `egret-ranger` into the seam crate, rename to `egret-policy` (as-built)

> **Status:** Implemented (stacked on E2a, branch `feat/e2a-policy-contract-crate`) · **Date:** 2026-07-08 · **Part:** E2b of [E2 decomposition](./2026-07-08-e2-policy-seam-decomposition.md) · **Precedent:** the E1 crate consolidation.

## Goal
Collapse the two-crate split (`egret-authz` = store, `egret-ranger` = HTTP serving surface over that store) into **one seam crate `egret-policy`**. The two always moved together and `egret-ranger` depended on `egret-authz`; keeping them separate served no boundary. Pure structural move — **zero behavior change**, no schema/route change.

## What changed
- **`git mv crates/egret-authz → crates/egret-policy`** (preserves history).
- **Folded the Ranger surface in under a `ranger` submodule**: `git mv` of `routes.rs`, `auth.rs`, `dto.rs`, `error.rs` (`RangerError`), and `api/` from the old `egret-ranger/src/` into `crates/egret-policy/src/ranger/`, plus a new `ranger/mod.rs` (the old `egret-ranger` lib root). Internal paths rewritten: `crate::{api,auth,routes,dto,error}` → `super::…`/`crate::ranger::…`; `egret_authz::{AuthzStore,AuthzError}` → `crate::…`.
- **`egret-policy/src/lib.rs`** now also `pub mod ranger;` and re-exports `ranger::{ranger_router, RangerState}` at the crate root (so callers write `egret_policy::ranger_router`, same shape as the old `egret_ranger::ranger_router`).
- **Tests + fixture moved** into `egret-policy/tests/` (`http_ranger.rs`, `ranger_engine_gate.rs`, `fixtures/trino-servicedef.json`); the `dto.rs` fixture `include_str!` path fixed for its new depth.
- **`egret-ranger` crate deleted**; workspace manifest drops it and renames the `egret-authz` path entry → `egret-policy`.
- **Merged `Cargo.toml`**: union of both crates' deps; added `axum` (serving surface); dev-deps gained `tower` + `http-body-util`. **Dropped the vestigial `egret-config` dep** (unused in ranger src/tests). `http` not re-added (no bare `http::` use).
- **Dependents rewired** (`egret_authz`/`egret_ranger` → `egret_policy` crate paths, one workspace-wide pass): `egret-bin` (collapsed its two deps into one), `egret-server`, `egret-controlplane` (src + tests), `egret-engine-mgmt` (dev-dep + test state construction).

## Deliberately NOT renamed (churn control)
Type names stay: `AuthzStore`, `AuthzError`, `RangerState`, `ranger_router`, `seed_trino`, `PolicyRow`. Only the **crate** was renamed. Renaming `AuthzStore → PolicyStore` etc. would touch every call site again for cosmetics; it can ride along with E2d (when the admin router moves in and the crate's public surface is revisited) or be skipped. Noted so the `authz`-named types inside `egret-policy` are a known, intentional interim.

## Verification
`cargo check -p egret-policy --tests` clean; full `cargo check --workspace --all-targets`; then fmt + clippy + the affected test suites (`egret-policy` incl. the folded `http_ranger`/`ranger_engine_gate`, plus downstream). No frontend change (the `/service/*` machine surface and `/api/control-plane/policies*` URLs are untouched).

## Next
E2c — migration 0063 `SET SCHEMA policy` for the three live authz tables + `DROP policy_audit`, cross-seam FK annotations, metastore re-embed.
