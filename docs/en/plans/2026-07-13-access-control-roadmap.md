# Access Control — Review & Roadmap (2026-07-13)

> Strategic reference for the next wave of access-control work. Companion to the
> per-increment plan `docs/en/plans/2026-07-13-workspace-access-tab.md` (the
> first execution step of this roadmap). For the prior epic see
> `docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`.

## Verdict

The **core RBAC/ReBAC engine is enterprise-grade and essentially complete.**
Cedar PDP + single `access_grants` store + role hierarchy + object-scoping +
per-user data-plane delegation + audit log is at or above many commercial
products *mechanically*. Access Explorer with provenance ("who — can do what —
where — **why**") is a genuine differentiator.

The remaining gap is **not** the authorization mechanism. It is (a) the
**governance-workflow / ABAC** layer (request→approve→certify, tag-based
policy, time-bound/future grants) that Immuta / SailPoint / Unity Catalog
occupy, and (b) two **UI/UX** gaps: no resource→principal ("who can access
this?") reverse view, and no Access-control **Overview** landing.

## Enterprise comparison

| Capability | Egret | Databricks UC | Snowflake | Note |
|---|:---:|:---:|:---:|---|
| RBAC + role hierarchy | ✅ | ✅ | ✅ | Par |
| Object / ReBAC inheritance | ✅ | ✅ | ✅ | Par |
| Explicit deny (deny-wins) | ✅ | ⚠️ | ⚠️ | **Egret ahead** |
| Per-user data delegation | ✅ | ✅ | ✅ | Par |
| Effective-access + provenance viewer | ✅ | ❌ | ❌ | **Egret ahead** |
| Audit log (queryable) | ✅ | ✅ | ✅ | Par |
| Resource → "who can access" reverse view | ❌ | ✅ | ✅ | **Gap — P0** |
| Access request / approval workflow | ❌ | ⚠️ | ⚠️ | Gap |
| Access certification / recertification | ❌ | ❌ | ⚠️ | Gap (governance) |
| Tag / attribute-based policy (ABAC) | ❌ | ✅ | ✅ | Gap — modern direction |
| Time-bound / expiring grants | ❌ | ⚠️ | ✅ | Gap |
| Future grants (ON FUTURE …) | ❌ | ✅ | ✅ | Gap |
| SCIM provisioning | ⚠️ (OIDC sync) | ✅ | ✅ | Partial |
| Row filter / column mask | ✅ (Ranger) | ✅ | ✅ | Par |

## Control Plane vs Workspace — the split

Split authz UI by **definition vs assignment**, not by "general vs detailed":

- **Assignment** (grant a role to a person, share an object) → **workspace**.
- **Definition** (what a role is allowed to do) → **central (Control Plane)**,
  because workspace roles are **global templates** — `WORKSPACE_ROLES =
  [ws_admin, ws_editor, ws_viewer]`, `roles` has `scope` but **no
  `workspace_id`**; editing one changes it for every workspace. Databricks and
  Snowflake both separate "who defines privilege sets" (central) from "who
  assigns them" (local).

Target information architecture:

```
CONTROL PLANE → Access control   (GLOBAL only)
├─ Overview            ← P0 (new): KPIs, recent changes, warnings
├─ Users, Groups                 (identity, SSO)
├─ Roles                         (platform roles + ws-role TEMPLATES,
│                                 labelled "applies to all workspaces")
├─ Permissions                   (catalog)
├─ Access explorer   (principal → access)
├─ Who-can-access     ← P0 (new): resource → principals
└─ Audit log

WORKSPACE → Settings → Access    (LOCAL to this workspace)
├─ Members                       ✅ exists (assign ws role, bulk)
├─ Roles & permissions           ← B1 (new): READ-ONLY view of ws roles
├─ Shared objects                ← B1 (new): object-grant roll-up
└─ Access requests               ← later
```

One-liner: **Control Plane answers "define privileges & identity"; Workspace
answers "in my space, who has what".**

- **B1** = the read-only workspace surfaces above (planned:
  `2026-07-13-workspace-access-tab.md`). Keeps definitions central, no
  data-model change. **This is the frontend split, first to ship.**
- **B2** (future) = true per-workspace custom roles (`roles.workspace_id` +
  Cedar template scoping + ws-admin role CRUD). Only then can role *definition*
  legitimately move into the workspace.

## UI/UX findings (ui-ux-pro-max lens; product = data-dense admin console)

| # | Issue | Fix |
|---|---|---|
| U1 | 5 flat items, no overview entry point | **Overview** page: KPI counts, recent access changes (from audit), warnings (superuser count, unheld roles, deny grants) |
| U2 | Access Explorer is principal→access only | **Who-can-access** reverse view for a chart/dashboard/table (effective set incl. inherited) |
| U3 | Inconsistent surface language (Card+FilterableTable vs ConfigSection grid vs bespoke lists) | Unify one layout shell (header + summary strip + table/section) |
| U3b | Plain "Loading…" text, scattered empty states | Use `ListSkeleton` consistently; empty states with a next action |
| U5 | Role tree has no impact preview on edit/delete | Show "affects N users, M groups" before confirm (transitive closure already computed) |
| U6 | Permissions page is a reference dump | Filter by role/area; "granted-to N roles" click-through |

## Roadmap (priority order)

| Priority | Item | Why | Size |
|---|---|---|---|
| **P0** | Access control **Overview** page (U1) | Highest perceived value; reuses existing data | FE-only, small |
| **P0** | **Who-can-access** reverse view (U2) | Closes the top auditor question; AccessResolver exists | FE + 1 endpoint |
| **P0/B1** | **Workspace Access tab** — read-only Roles & permissions + Shared-objects roll-up | The Control-Plane↔Workspace frontend split | FE + 2 small BE reads — *planned* |
| **P1** | Surface standardization + skeleton/empty (U3, U3b) | Consistent polish, low cost | FE-only |
| **P1** | Impact preview on role edit/delete (U5) | Operation safety | FE + read |
| **P2** | **Time-bound / expiring grants** | Clearest enterprise gap; just-in-time access | BE + FE (mig `access_grants.expires_at`) |
| **P2** | **Access request → approval** workflow | Shifts admin-config → self-service governed | Epic |
| **P3** | **ABAC / tag-based policy** | Modern direction (UC tags, Snowflake tag masking); joins Data Governance Policies | Large epic |

## Sequencing

1. **B1 workspace split** (`2026-07-13-workspace-access-tab.md`) — ship first; it
   is the "chia Workspace / Control Plane" ask and unblocks the mental model.
2. **P0 Control-Plane** — Overview + Who-can-access (each its own plan).
3. **P1 polish** — surface standardization, skeletons, impact preview.
4. **P2 governance** — time-bound grants, then access-request workflow.
5. **P3 ABAC** — tag-based policy (design spike first; ties into Ranger/Policies).

Each row above becomes its own dated plan under `docs/en/plans/` when picked up.
