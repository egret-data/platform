# E2 — Policy/Ranger Seam Extraction (decomposition)

> **Status:** Decomposition · **Date:** 2026-07-08 · **Epic:** E2 of the [service-ready redesign](../architecture/2026-07-08-service-ready-architecture-design.md#5-execution--strangler-sequence)
> **Precedent:** E1 (engine-provisioning) — the template seam, delivered as E1a/E1b/E1c (PRs #179/#180/#181).
> **Per-seam DoD:** contract crate (DTOs derive `ToSchema`) · domain logic out of controlplane behind `admin_router()` · tables `SET SCHEMA`, cross-seam FKs annotated `-- SEAM-CROSSING` · OpenAPI/FE codegen (deferred with E0·P4) · event wiring (deferred to first consumer) · full local CI green.

## 1. What E2 extracts

The **Policy/Ranger** seam (design §2 seam #2): the engine-facing policy store + the Ranger-Admin-REST-compatible serving surface + the admin policy-editor API. Today it is spread across three places:

| Today | LOC | Role |
|---|---|---|
| `crates/egret-authz` | ~589 (856 w/ tests) | `AuthzStore` — CRUD over `authz_service_defs/services/policies`, versioning, `seed_trino`, `seed_default_trino_policies`. No HTTP. |
| `crates/egret-ranger` | ~446 (667 w/ tests) | Ranger-Admin-REST surface (`ranger_router`/`RangerState`) mounted at `/service/*`: policy **download** for stock plugins + Ranger-Admin CRUD. Wraps `AuthzStore`. `auth.rs` = the anonymous serving-surface. |
| `crates/egret-controlplane/src/policies/` | ~1,233 | Admin policy-editor router at `/api/control-plane/policies*`: `mod.rs` (handlers + `PolicySummary`) + `ranger_json.rs` (editor-form ↔ Ranger-JSON). Authenticated + RBAC-gated (`policy:read/create/update/delete/seed`, `platform_admin` bypass). |

### Schema (migrations 0007, 0058; current head 0062)
- `authz_service_defs` (id, name UNIQUE, def_jsonb, created_at)
- `authz_services` (id, name UNIQUE, type→`authz_service_defs(name)` **in-seam**, config_jsonb, policy_version, timestamps)
- `authz_policies` (id, service_id→`authz_services(id)` ON DELETE CASCADE **in-seam**, name, policy_jsonb, enabled, version, updated_at; UNIQUE(service_id,name))
- `policy_audit` (id, policy_id, action, actor→`users(id)`, at) — **DEAD: no code reads/writes it.**

**Cross-seam FKs:**
- inbound `engine_mgmt.engines.authz_service_id → authz_services(id)` ON DELETE SET NULL — engine-mgmt (E1) → policy. Already cross-schema + annotated since migration 0062.
- `policy_audit.actor → users(id)` — policy → iam. (Removed when we drop `policy_audit`.)

### Cross-seam couplings to route through the contract
- **`egret-engine-mgmt`** holds its own `AuthzStore` and, on engine-create, calls `seed_trino` + `list_services` + `set_engine_authz_service` to auto-bind engine→profile. → should depend on `PolicyPort`, not concrete `AuthzStore`.
- **`controlplane/policies/mod.rs`** reads back the engine seam (`st.engine_mgmt.list_engines()/list_engine_catalogs()` in `list_profiles`/`resource_lookup`), `st.engine_egret_url` (trino_config), and `st.users/st.groups` (principal picker). → these are the reads to route through Ports when the module moves.

### Frontend
Only `frontend/src/api/policies.ts` holds URL literals (`/control-plane/policies*`, `/control-plane/principals`); consumed by `routes/control-plane/{Policies,PolicyEditorPage,ProfileDetailPage}.tsx` + `components/policy/*`. **No FE code calls `/service/*`.** URLs must stay stable through E2.

## 2. Locked decisions

1. **Anonymous serving-surface (E2e) → split the gate + drop unused writes.** The only legitimate anonymous need is stock plugins polling **read-only** policy downloads over a trusted network (Trino `RangerAdminRESTClient` authenticates out-of-band, sends no bearer). Today an empty `ranger.auth_token` opens the **entire** `/service/*` router including the Ranger-Admin CRUD **write** routes — which have **zero callers** (egret admin CRUD is the authenticated controlplane path; engine-mgmt uses `AuthzStore` directly). Fix: keep the download/GET routes open-capable when no token is set; **remove** the write routes (`POST/PUT/DELETE /plugins/services|policies`) from the machine surface. Re-addable behind auth if external Ranger-Admin write-compat is ever needed.
2. **`policy_audit` → drop it.** Dead since 0007; Effort B introduces a real `audit_log` in a dedicated `audit` schema. Removed in the E2c migration.
3. **Crate shape → merge `egret-ranger` into `egret-authz`, rename to `egret-policy`** (mirrors E1's `egret-engine-mgmt`). Leaf contract crate is `egret-policy-api`.

## 3. Parts (strangler order within E2)

| Part | Content | Risk | Precedent |
|---|---|---|---|
| **E2a** | Leaf contract crate **`egret-policy-api`**: DTOs (`ServicePolicies` download envelope + service/def/policy rows + editor inputs) deriving `serde`+`utoipa::ToSchema`, and a **`PolicyPort`** trait. Point `egret-engine-mgmt` (auto-bind) + controlplane at the Port/DTOs where cheap. No schema/router move → clean, self-contained. | Low | E1a |
| **E2b** | Merge `egret-ranger` **into** `egret-authz`, rename → **`egret-policy`**. One seam crate = store + ranger router, implementing `PolicyPort`. `git mv` + import swaps + Cargo.toml fixups across bin/server/controlplane/engine-mgmt. | Low | (E1 crate merge) |
| **E2c** | Migration **0063**: `CREATE SCHEMA policy`; `SET SCHEMA policy` for the three live authz tables; **`DROP TABLE policy_audit`**; keep + annotate `engines.authz_service_id` SEAM-CROSSING. Force metastore re-embed. | Medium | E1b / 0062 |
| **E2d** | Move `controlplane/policies/` into `egret-policy` as `admin_router()`; strip `CpState.authz`; route `engine_mgmt`/`users`/`groups` reads through Ports; mount on the aggregator; **FE URLs stable**. | Medium | E1c |
| **E2e** | Fix the anonymous serving-surface per decision 1. | Low | — |

Each part: plan → TDD build → verify → **full local CI (BE fmt+clippy+test AND FE lint/typecheck/test/build)** → squash-merge (user runs the `--admin` merge). GitHub CI is out of quota (project practice).

## 4. Deferred (consistent with E0/E1)
- **OpenAPI emit + FE client codegen** — deferred with E0·P4 to the first seam that exposes a contract; E2a DTOs derive `ToSchema` now so the emit is mechanical later.
- **`egret-events`** wiring (`policy.changed` → engine reload / version bump for 304, `workspace.deleted` prune) — deferred to the first consuming seam per E0·P3. `policy_version` bump-on-write already exists in `AuthzStore`.
- **Merging the two authorizers / object-scoping / real audit_log** — Effort B (starts after E2 + E3).
