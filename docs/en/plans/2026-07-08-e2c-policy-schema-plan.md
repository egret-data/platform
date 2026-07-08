# E2c — `policy` schema migration + drop `policy_audit` (as-built)

> **Status:** Implemented (stacked on E2a/E2b, branch `feat/e2a-policy-contract-crate`) · **Date:** 2026-07-08 · **Part:** E2c of [E2 decomposition](./2026-07-08-e2-policy-seam-decomposition.md) · **Precedent:** E1b migration 0062 (`SET SCHEMA engine_mgmt`).

## Goal
Give the Policy/Ranger seam **data ownership**: its tables live in a dedicated `policy` Postgres schema (design §3.2). One pool today; a real split later gets its own DB with no repo change (queries already schema-qualified).

## What changed
- **`migrations/0063_policy_schema.sql`**: `CREATE SCHEMA IF NOT EXISTS policy;` then `ALTER TABLE authz_service_defs | authz_services | authz_policies SET SCHEMA policy;` (SET SCHEMA preserves data, indexes, constraints, FKs). Plus **`DROP TABLE IF EXISTS policy_audit`** — dead since migration 0007, never read/written; Effort B brings a real `audit_log` in an `audit` schema.
- **FK annotations** (in the migration comment): the cross-seam FK `engine_mgmt.engines.authz_service_id → policy.authz_services` now points into `policy` (SET SCHEMA moved the referenced table; constraint follows) — `-- SEAM-CROSSING`, dropped only at a real split. In-seam FKs (`authz_services.type → authz_service_defs`, `authz_policies.service_id → authz_services`) moved with their tables.
- **Schema-qualified all store SQL**: every `authz_service_defs`/`authz_services`/`authz_policies` reference in `egret-policy/src/store.rs` (23 sites) → `policy.authz_*`. No unqualified access relies on `search_path` (matches E1b's explicit-qualification choice). No other crate has raw SQL on these tables (verified) — all cross-seam access goes through `AuthzStore`/`PolicyPort` methods.
- **Forced metastore re-embed**: bumped the `sqlx::migrate!` header comment in `egret-metastore/src/lib.rs` to "through 0063" (a new migration file alone doesn't recompile the embedding crate — known gotcha).

## Verification
Reset `egret_test` to a clean slate, rebuilt metastore, ran `cargo test -p egret-policy` → migration applied and the global `authz_*` tables are **physically in the `policy` schema** (confirmed via `pg_tables`), `policy_audit` gone; all suites green against the qualified SQL (store_services/store_policies/port_impl/http_ranger/default_policies). fmt + clippy clean.

## Known test-infra interaction (not a regression)
Like 0062, `0063` targets a **global** schema (`policy`), so the per-test isolated-schema harness (controlplane `routes_api.rs`) can collide (`relation … already exists in schema "policy"`) under full-suite DB contention — the same pre-existing E1b flake, now also touching `policy`. The real fix is test-isolation-scoping the SET SCHEMA migrations (tracked as an E1/E2 test-infra follow-up), not a seam-code issue.

## Next
E2d — move `controlplane/policies/` into `egret-policy` as `admin_router()`, strip `CpState.authz`, route the `engine_mgmt`/`users`/`groups` reads through Ports; FE URLs stable.
