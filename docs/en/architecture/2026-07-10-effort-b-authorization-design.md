# Effort B — Unified Authorization: North-Star + B1 Design

> **Status:** Design (approved to spec) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Service-ready architecture v1.0](./2026-07-08-service-ready-architecture-design.md) §6 "Interplay with Effort B". That doc prepared the seams; this doc is the authorization design B plugs into.
>
> **Prerequisites (all met):** E0 (auth gate relocated to `egret-auth`, shared token helper), E2 (Policy extracted), E3 (Catalog extracted).

---

## 1. Why

Egret has **two parallel authorizers** today, born of two different resolution strategies:

| | `egret-auth::authz` (static gate) | `egret-workspace::rbac::Authorizer` (DB gate) |
|---|---|---|
| Model | pure fn over `Principal.permissions` pre-loaded at session | queries DB per request |
| Resolution | eager (session build) → stale until re-login | lazy (per request) → fresh |
| Used by | platform surfaces: policy, warehouse, engine-mgmt, control-plane access, storage, secrets (~80 call-sites) | workspace surfaces: bi, notebook, query, kernel, workspace-members (~120 call-sites) |
| API | `require_permission(&p, key)` free fn | `authz.require_workspace(&ctx, key)` method |

Consequences: `require_platform` is **duplicated** (static over session perms *and* a DB re-query in `Authorizer`), giving two answers for the same question; platform permission changes are stale within a session; workspace-feature permissions (`dashboard:read`, `notebook:*`, …) are enforced as bare string literals and are **absent from the central catalog** (`egret-core::rbac::PERMISSIONS`), so the drift-lint can't see them.

Effort B unifies this into one decision model that also carries object-level scoping, per-user data-plane delegation, and real audit — but as **separate sub-epics** (see §5). This doc sets the north-star and details **B1** (the foundation).

## 2. North-star: layered authorization

Standard modern layering (NIST/XACML terms PDP/PEP/PAP/PIP + Zanzibar-lite relationship grants), realized in-process with **Cedar** (`cedar-policy`, AWS, Apache-2.0) as the decision engine. It is reusable by construction: **adding a feature = declare its actions in the catalog + pick a scope level; the PDP/PEP never change.**

| Layer | Role | Today | Target |
|---|---|---|---|
| **L0 — Identity (AuthN)** | OIDC + local + machine token → `Subject` (human/service) + group membership | oidc, session, groups (built) | B3 extends machine/delegation tokens |
| **L1 — Grant store (PAP)** | Source of truth "who has what where": grant tuple `(principal, role, scope)`, `scope ∈ {platform, workspace:{id}, object:{type,id}}`. Role = permission bundle. Group = expands to members. | 2 disjoint tables | **B1** unifies into one `access_grants` table (object scope reserved for B2) |
| **L2 — PDP (Decision)** | `authorize(subject, action, resource) -> Decision`: resolve grants along the resource's scope chain (object → workspace → platform), evaluate Cedar policies (permit/**forbid**), return allow/deny + contributing policies. **One chokepoint.** | 2 authorizers | **B1** builds it (Cedar behind a Port) |
| **L3 — PEP (Enforce)** | Middleware builds `AuthContext`; handlers call `require(ctx, action[, resource])`. Thin, uniform. | 2 call styles | **B1** unifies |
| **L4 — Data-plane federation** | Control-plane grants + policies compile down to Ranger policies + per-user downscoped tokens for engines/catalog. Same subject & grant model. | Ranger + machine-JWT (separate) | B3 + existing Policies epic |
| **Cross-cutting — Audit (decision log)** | Every PDP decision + every grant mutation → `audit_log`. Cedar diagnostics supply the reason. | only local `policy_audit` | **B4** |

### 2.1 Why Cedar (not Oso)

- **Oso's open-source library is sunset** (pivoted to Oso Cloud) → maintenance risk for a long-lived in-process dependency.
- **Cedar is Rust-native and actively developed by AWS** (foundation of AWS Verified Permissions); evaluates fully **in-process** → preserves egret's one-binary / deployment-unchanged principle.
- **`forbid` overrides `permit`** (deny wins) → explicit-deny and Ranger-parity (B2) come for free.
- **Entity hierarchy** (`in` operator) maps directly onto the scope chain object → workspace → platform.
- **Schema + validation** and **policy-as-data** (store in DB, hot-reload via the planned `policy.changed` event).

### 2.2 Cedar model for egret

- **Entities (data, built per-request from the grant store):**
  - Principals: `User::"<uuid>"`, `Service::"<id>"` (catalog clients, kernels). `User in [Group::"…", Role::"…"]`.
  - Resources: `Platform::"root"`, `Workspace::"<id>"`, and (B2) typed objects `Dashboard::"<id>"`, `Warehouse::"<id>"`, … with `parent` hierarchy `Object in Workspace in Platform`.
  - `Role` entities carry a `permissions: Set<String>` attribute (from `role_permissions`) and are scoped via `Role in [Workspace | Platform]`.
- **Actions** = the unified permission catalog (control-plane actions from `egret-core::rbac` **plus** the workspace-feature actions folded in). Schema declares `appliesTo` principal/resource types.
- **Static policy-set** (few, versioned; grants are entity *data*, not per-user policies — Zanzibar-lite):
  - `permit(principal in Role::"platform_admin", action, resource);` — admin bypass.
  - platform: `permit` when the principal holds the action via a platform-scoped role and `resource == Platform::"root"`.
  - workspace: `permit` when the principal holds the action via a role scoped to the request's workspace and `resource in Workspace::"…"`.
- **Decision** returns `Allow/Deny` + contributing policy ids → the audit reason (B4).

## 3. B1 — Unify the authorizer on Cedar (behavior-preserving)

**Goal:** one Cedar PDP replaces both authorizers with **no behavior change** (every existing gate yields the same allow/deny); **no** object scoping (B2), **no** audit content (B4) — but the `resource` parameter and decision diagnostics are wired in from day one. Per the approved scope, B1 **also unifies the grant tables** (L1) so the data model is clean before object scope arrives.

### 3.1 L1 — unified grant table

New table (target schema `iam`), replacing `platform_role_assignments` and `workspace_members`:

```sql
CREATE TABLE access_grants (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_type text NOT NULL CHECK (principal_type IN ('user','group','service')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform','workspace','object')),
    scope_id       uuid,          -- NULL for platform; workspace_id for workspace; object id for B2
    object_type    text,          -- NULL in B1; reserved for B2
    granted_by     uuid,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)
);
```

- `platform_role_assignments (principal_type, principal_id, role_id)` → `scope_type='platform', scope_id=NULL`.
- `workspace_members (workspace_id, principal_type, principal_id, role)` → `scope_type='workspace', scope_id=workspace_id, role_id=lookup(roles WHERE scope='workspace' AND name=role)`.
- `roles`, `role_permissions`, `user_groups`, `groups` unchanged.
- **Migration** back-fills `access_grants` from both tables, then drops them. Workspace-member listing becomes `SELECT … WHERE scope_type='workspace' AND scope_id=$ws`. A single "primary role" for display is derived (highest-precedence role name) to preserve `WorkspaceContext.workspace_role` semantics.

### 3.2 L2 — PDP behind a Port

- New trait in `egret-core` (identity-only crate stays engine-agnostic):
  ```rust
  pub struct Decision { pub allow: bool, pub reasons: Vec<String> }
  #[async_trait] pub trait Pdp {
      async fn authorize(&self, subject: &Subject, action: &str, resource: &Resource) -> Decision;
  }
  ```
  `Resource` is `Platform | Workspace(WorkspaceId)` in B1; the enum gains object variants in B2 without touching call-sites.
- Cedar impl lives in `egret-auth` (`authz/` module): owns the `.cedarschema`, the static `.cedar` policy-set, the `Authorizer`, and the entity provider. **Handlers and PEP never reference Cedar** — only the `Pdp` port.

### 3.3 L2 — entity provider

Replaces `session::permissions` + `rbac::Authorizer`. From `access_grants` + `role_permissions` + `user_groups` it builds the Cedar entity-set for `(subject, resource-scope-chain)`, memoized per request. One query shape covers platform and workspace (filter by scope), removing the eager/lazy split.

### 3.4 L3 — unified PEP

- One helper `require(ctx, action[, resource])` replaces **both** `require_permission(&p, key)` (~80 sites) and `authz.require_workspace/require_platform(&ctx, key)` (~120 sites).
- `WorkspaceContext` becomes/extends `AuthContext` carrying the request's subject + scope. Middleware builds it (platform routes → `Platform` scope; workspace routes → `Workspace(id)` scope, as today).

### 3.5 Unified catalog

Fold the workspace-feature actions (`dataset:*`, `chart:*`, `dashboard:*`, `notebook:*`, `query:*`, `kernel:*`, …) into `egret-core::rbac::PERMISSIONS`, each tagged with its scope level. This closes the catalog gap and lets the drift-lint and the Cedar schema see every action. The Cedar schema is generated/validated against this catalog.

### 3.6 Behavior preservation

- Keep the `catalog_drift` lint; extend it to the workspace-feature call-sites.
- **Regression matrix test:** for a representative set of (role, action, scope) tuples, assert the Cedar PDP returns the same allow/deny as the pre-B1 logic (golden table).
- Full-workspace `fmt + clippy + test` **and** frontend `lint + typecheck + test + build` green locally before merge (GitHub CI is out of quota — project practice).

### 3.7 B1 scope boundaries (YAGNI)

- **In:** Cedar PDP + Port, unified `access_grants` table + migration, unified PEP + catalog, behavior-preserving regression.
- **Out:** object-level grants / role hierarchy (B2), `audit_log` content (B4), per-user catalog/engine delegation and Ranger federation (B3/L4). The `Resource` enum, `scope_id`/`object_type` columns, and decision `reasons` are the seams those plug into.

## 4. Interfaces & isolation

- `egret-core`: `Subject`, `Resource`, `Decision`, `Pdp` trait, unified `PERMISSIONS` catalog. No engine dependency.
- `egret-auth`: the only crate that knows Cedar exists — schema, policies, entity provider, `Pdp` impl, `AuthContext` builder middleware, `require()` helper.
- Every other seam depends only on `egret-core` (`require`, `Pdp`) — unchanged dependency direction; seams still gate without pulling in controlplane or Cedar.

## 5. Sub-epic sequence

| Sub-epic | Content | Depends on |
|---|---|---|
| **B1 — Unify authorizer** (this doc §3) | Cedar PDP + Port, unified `access_grants`, unified PEP + catalog, behavior-preserving | E0/E2/E3 (met) |
| **B2 — Object-level scoping** | object grants + `Resource` object variants + hierarchy inheritance + explicit deny; unify object-share UIs (Metabase-style collections) | B1 |
| **B3 — Per-user data-plane delegation** | replace machine-JWT with per-user downscoped tokens (catalog + Trino + StarRocks); federate control-plane grants into Ranger | B1, E0 token helper, E3 catalog contract |
| **B4 — Real `audit_log`** | middleware writes every decision + grant mutation from Cedar diagnostics; `audit` schema | B1 (chokepoint) |

## 6. Open questions (for the B1 plan)

- Cedar entity/schema versioning + hot-reload wiring to the (planned) `policy.changed` event, or static-embed for B1 and defer reload to B2.
- Precise "primary role" precedence rule to preserve `WorkspaceContext.workspace_role` display.
- Whether `Service` principals (catalog clients, kernels) route through the same PDP in B1 or stay on their current token checks until B3.

---

*Design v1.0 (2026-07-10). North-star for Effort B (authorization); B1 is the first increment. Companion to the Service-ready architecture v1.0.*
