# Access Control P0 — Control-Plane Overview + Who-can-access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add the two P0 Control-Plane surfaces from the access-control roadmap: an **Overview** landing page (KPIs, warnings, recent access changes, quick links) and a **Who-can-access** reverse view (given an object, every principal who can access it and why — the inverse of Access Explorer).

**Architecture:** Overview is **FE-only**, aggregating the existing `listUsers`/`listGroups`/`listRoles` endpoints (RoleDto already carries `assignee_count`) plus the existing `GET /control-plane/audit` (gated on platform_admin, panel hidden otherwise). Who-can-access adds **one** new resolver method + endpoint that reuses the existing `AccessResolver` (`crates/egret-auth/src/effective.rs`) and `CpState`'s resolver instance (the one `get_effective_access` already uses) — resource→principals is the mirror of the existing principal→resource `grant_rows`.

**Tech Stack:** Rust (axum, sqlx, utoipa) in `crates/egret-auth` + `crates/egret-controlplane`; React 18 + TanStack Query + shadcn/Tailwind; orval-generated client; Vitest + `cargo test`.

---

## Context & reuse surfaces (read these; do not rebuild)

- `crates/egret-auth/src/effective.rs` — `AccessResolver { db }` with `resolve(ptype,pid)` (principal→access), `grant_rows`, `object_table(kind)->'static table`, `resolve_ident`. The reverse method mirrors `grant_rows` keyed by resource.
- `crates/egret-controlplane/src/access/mod.rs` — the CP access router (`/access/effective` → `get_effective_access`) and `CpState`. **Confirm the resolver field name on `CpState` by reading `get_effective_access`** and reuse it for the new handler. Handlers gate via `egret_core::authz::require(&*st.pdp, &p, "…")` with `Auth(p)` and return `Result<Json<…>, CpError>`.
- `crates/egret-controlplane/src/audit.rs` — `GET /control-plane/audit` → `AuditPage { entries: AuditEntryDto[], next_cursor }`; `AuditEntryDto { id, ts, kind, actorUserId, workspaceId, action, resourceType, resourceId, decision, reasons, detail }` (camelCase). Gated `audit:read` (platform_admin only).
- `frontend/src/api/access.ts` — `listUsers()→AccessUser[]`, `listGroups()→AccessGroup[]`, `listRoles()→AccessRole[]` (RoleDto has `permission_count`, `assignee_count`, `scope`, `parent_role_id`), `getEffectiveAccess(type,id)`.
- `frontend/src/routes/control-plane/access/AccessExplorer.tsx` — the sibling report page: `Section`, `SourceSuffix`, `EffectBadge`, `allowStyle`/`denyStyle` (`color-mix` OKLCH tokens), the `PrincipalPicker` (search + deep-link via `useSearchParams`). Who-can-access mirrors this structure (an **object** picker instead of a principal picker).
- `frontend/src/routes/explore/settings/SharedObjects.tsx` (B1) — its rows are the primary deep-link source into Who-can-access; add a "Who can access?" row action there.
- `frontend/src/components/layout/Sidebar.tsx` — `CONTROL_PLANE_GROUPS`, the "Access control" group. Add **Overview** (first) and **Who-can-access** items. `perm` gates each item (`user:read`/`role:read`).
- `frontend/src/App.tsx` — control-plane routes; add the two pages.
- `frontend/src/api/policies.ts` — `listPrincipals(q,type)` (used by pickers). `frontend/src/api/generated/admin-workspaces` — `adminWorkspacesListWs` (workspace list for the object picker).

---

## Who-can-access — backend contract

### `GET /api/control-plane/access/who-can-access?kind={kind}&id={uuid}`

Every principal who can access the given object, with provenance. Gated `role:read` (same as `/access/effective`).

**Resolution (mirror of `grant_rows`, keyed by the resource):**
1. Resolve `(kind,id)` → the object's `workspace_id` + display `name` via `AccessResolver::object_table(kind)` (returns `None` → 404 for an unknown kind; object not found → 404).
2. **Direct object grants**: `access_grants` where `scope_type='object' AND scope_id=$id`, joined to `roles` + principal names (users.email / groups.name). Each → source `direct`, its `effect`.
3. **Workspace-role holders** (cascade to the object per B2-1a): `access_grants` where `scope_type='workspace' AND scope_id=$workspace_id`, joined to roles + principal names. Each → source `workspace:{slug}`, `effect` from the grant, `role` = the ws role.
4. **Platform admins** (superusers — access everything): users holding a `platform`-scope `platform_admin` allow grant. Each → source `platform`, role `platform_admin`, effect `allow`.
5. Groups are listed as principals (type `group`) — NOT expanded to members (parity with Access Explorer / members views).

**Response `200 body = WhoCanAccess`:**

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct WhoCanAccess {
    pub object: ObjectRef,            // { kind, id, name, workspace_slug }
    pub entries: Vec<AccessEntry>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct ObjectRef {
    pub kind: String, pub id: String,
    pub name: Option<String>, pub workspace_slug: Option<String>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct AccessEntry {
    pub principal_type: String,       // user | group
    pub principal_id: String,
    pub principal_name: Option<String>,
    pub access: String,               // role name (e.g. ws_viewer, platform_admin)
    pub effect: String,               // allow | deny
    pub source: String,               // "direct" | "workspace:{slug}" | "platform"
}
```

Place the struct + resolver method in `effective.rs` (next to `AccessResolver`), the DTO + handler in `access/mod.rs`, route `.route("/access/who-can-access", get(who_can_access))`, register in the `control-plane-access` OpenAPI fragment.

---

## Overview — data sources (FE-only)

`GET`-only aggregation, no new endpoint:
- **KPIs**: `listUsers()` → total + active count + `platform_admin` count (a user whose `roles` includes `platform_admin`); `listGroups()` → total (+ oidc-synced count); `listRoles()` → total (+ platform/workspace split).
- **Warnings** (each links to the relevant page): platform-admin count (highlight if > a small threshold, e.g. `≥ 3` → "review superusers"); roles with `assignee_count === 0` ("unused roles"); OIDC groups with no roles (optional).
- **Recent changes**: only if the caller is `platform_admin` (`me.platform_roles.includes("platform_admin")`) — fetch `GET /control-plane/audit?limit=8`, render a compact timeline (action, resourceType, actor, relative ts) with a "View audit log" link. If not platform_admin, hide the panel entirely (no 403 noise).
- **Quick links**: cards/links to Users, Groups, Roles, Permissions, Access explorer, Who-can-access.

---

## Non-Goals

- No global cross-workspace object **search** picker for Who-can-access in this increment. Entry is (a) deep-link `?ws=&kind=&id=` from the B1 Shared-objects roll-up + Access Explorer object rows, and (b) a bounded picker: **workspace `<Select>` → shared-object `<Select>`** sourced from the B1 `GET /ws/{ws}/shares` roll-up (who-can-access is most meaningful for shared objects; unshared objects are accessed uniformly by ws members). Picking arbitrary unshared objects = follow-up.
- No expansion of groups to individual members (parity with existing views).
- No new authz mechanism; no data-model change. Overview adds no endpoint.
- No summary/count endpoint (client-side aggregation is acceptable for an admin overview); a dedicated `/access/summary` is a possible follow-up if the list calls prove heavy.

---

## File Structure

**Backend**
- Modify `crates/egret-auth/src/effective.rs` — add `WhoCanAccess`/`ObjectRef`/`AccessEntry` + `AccessResolver::who_can_access(kind,&id)`.
- Modify `crates/egret-controlplane/src/access/mod.rs` — DTO passthrough (or reuse the `effective.rs` structs directly if they already derive `ToSchema`), handler `who_can_access`, route, OpenAPI registration.
- Test: `crates/egret-auth/tests/who_can_access_db.rs` (mirror `effective_db.rs`).

**Frontend**
- Modify `frontend/src/api/access.ts` — `getWhoCanAccess(kind,id)` helper (via the regenerated client, matching the `getEffectiveAccess` adapter).
- Create `frontend/src/routes/control-plane/access/WhoCanAccess.tsx` + test.
- Create `frontend/src/routes/control-plane/access/AccessOverview.tsx` + test.
- Modify `frontend/src/components/layout/Sidebar.tsx` (nav items) + `frontend/src/App.tsx` (routes).
- Modify `frontend/src/routes/explore/settings/SharedObjects.tsx` — add a "Who can access?" row action deep-linking to `/control-plane/access/who-can-access?ws={ws}&kind={kind}&id={object_id}`.

---

## Tasks

> TDD throughout. Backend DB tests run against the container Postgres `postgres://egret:egret@localhost:5432/egret_test` (self-skip when unset, like `effective_db.rs`). FE tests use Vitest + `vi.mock`.

### Task 1: BE — `AccessResolver::who_can_access` + reverse resolution

**Files:** `crates/egret-auth/src/effective.rs`; test `crates/egret-auth/tests/who_can_access_db.rs`.

- [ ] **Step 1 — Failing test.** In a fresh workspace W with a chart C: grant group G `ws_viewer` directly on C (allow); grant user U2 `ws_editor` on the WORKSPACE W (cascades to C); make user U3 a `platform_admin`. Assert `who_can_access("chart", C)` returns the object ref (kind, name, workspace_slug) and entries containing: G (direct, ws_viewer, allow), U2 (workspace:{slug}, ws_editor, allow), U3 (platform, platform_admin, allow); and that a chart in a DIFFERENT workspace's grants are absent.
- [ ] **Step 2 — Run, confirm fail.** `DATABASE_URL=… cargo test -p egret-auth --test who_can_access_db`.
- [ ] **Step 3 — Implement.** Add the three structs, then `who_can_access`: resolve object via `object_table` (404-shaped `Err(NotFound)`/`Ok`-with-empty as appropriate — return `egret_core::Error::NotFound` for unknown kind / missing object so the handler maps to 404), then three parameterized queries (direct object grants; workspace grants on the object's workspace; platform_admin users) folded into `Vec<AccessEntry>`. Reuse `db_err`. Order entries by source then principal name.
- [ ] **Step 4 — Run, confirm pass** (incl. the cross-workspace absence assertion).
- [ ] **Step 5 — fmt + clippy + commit.** `cargo fmt --all`; `cargo clippy -p egret-auth --all-targets`. Commit `feat(auth): AccessResolver::who_can_access (reverse resource→principals)`.

### Task 2: BE — handler + route + OpenAPI

**Files:** `crates/egret-controlplane/src/access/mod.rs`; test `crates/egret-controlplane/tests/routes_api.rs` (extend).

- [ ] **Step 1 — Failing test.** A `role:read` caller GETs `/api/control-plane/access/who-can-access?kind=chart&id={C}` → 200 with entries; an unknown kind → 404; a caller lacking `role:read` → 403.
- [ ] **Step 2 — Run, confirm fail** (route missing → 404 for the happy path too).
- [ ] **Step 3 — Implement** `who_can_access` handler mirroring `get_effective_access`: `Auth(p)`, `require(&*st.pdp,&p,"role:read")`, read `kind`+`id` from `Query`, call the resolver field on `CpState` (confirm its name), map `Error::NotFound`→`CpError::NotFound`. Add the route + register in the `control-plane-access` `#[openapi(...)]` (path + `WhoCanAccess`/`ObjectRef`/`AccessEntry` schemas).
- [ ] **Step 4 — Run, confirm pass** (200 + 404 + 403).
- [ ] **Step 5 — fmt + clippy + commit.** `feat(access): GET /access/who-can-access endpoint`.

### Task 3: FE — regenerate client + api helper

**Files:** `frontend/src/api/generated/**` (orval), `frontend/src/api/access.ts`.

- [ ] **Step 1** — regen the client (the repo's orval/openapi script) so the `who-can-access` op appears; add `getWhoCanAccess(kind,id)` to `access.ts` mirroring the `getEffectiveAccess` adapter (unwrap `{data}`), and export the `WhoCanAccess` type.
- [ ] **Step 2** — `cd frontend && npm run typecheck` clean.
- [ ] **Step 3 — Commit.** `chore(api): regenerate client for who-can-access`.

### Task 4: FE — Who-can-access page

**Files:** create `frontend/src/routes/control-plane/access/WhoCanAccess.tsx` + `__tests__/WhoCanAccess.test.tsx`.

- [ ] **Step 1 — Failing test.** `vi.mock("@/api/access")` (getWhoCanAccess) + `vi.mock` the workspace list + shares hooks used by the picker. Cases: (a) with `?ws=demo&kind=chart&id=abc`, renders the object header + an entry table with principal, access (role), an allow/deny badge, and a source suffix (direct / workspace:{slug} / platform); (b) no params → an empty prompt + the picker (workspace select, then object select); (c) 403 → the `role:read` alert.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement.** Mirror `AccessExplorer.tsx`: `useSearchParams` for `ws`/`kind`/`id`; a `Card` "Object" picker = a workspace `<Select>` (from `adminWorkspacesListWs`) + an object `<Select>` populated from the B1 `useWorkspaceShares(ws)` roll-up (distinct objects; label `kind · name`), writing `{ws,kind,id}` to the URL on pick. Below, a `Report` `Card`: object header (kind badge + name + `/{slug}`), then a `FilterableTable` of `entries` (columns: Principal type+name, Access role badge, Effect badge via copied `allowStyle`/`denyStyle`, Source suffix). Reuse `Section`/`SourceSuffix`/`EffectBadge` styling from AccessExplorer (extract shared helpers if convenient, else copy the two `CSSProperties` + the tiny components). Loading/403/empty states like AccessExplorer.
- [ ] **Step 4 — Run, confirm pass**; `npm run typecheck` + `npm run lint` clean.
- [ ] **Step 5 — Commit.** `feat(fe): Who-can-access reverse view`.

### Task 5: FE — Access Overview page

**Files:** create `frontend/src/routes/control-plane/access/AccessOverview.tsx` + `__tests__/AccessOverview.test.tsx`.

- [ ] **Step 1 — Failing test.** `vi.mock("@/api/access")` (listUsers/listGroups/listRoles) + `vi.mock` the audit hook + `useAuth`. Cases: (a) renders KPI tiles (users total/active, groups, roles, platform-admins) from the mocked lists; (b) a warning row for a role with `assignee_count===0`; (c) when `me.platform_roles` includes `platform_admin`, the recent-changes panel renders mocked audit entries; when it does NOT, the panel is absent; (d) quick-link to Who-can-access + Users present.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement.** Header; a KPI row (reuse a stat-tile pattern — check `frontend/src/routes/control-plane` for an existing stat-card/`SummaryStrip`; if none fits, a simple `Card` grid with big number + label, semantic tokens, tabular-nums). Warnings `Card` (superuser count with `--highlight` token when `≥3`; unused roles list — each links to `/control-plane/access/roles`). Recent-changes `Card` gated on `platform_admin`, listing `useQuery` audit `limit=8` with relative timestamps (`lib/relativeTime`) + a "View audit log" link to the audit page. Quick-links `Card` (NavLinks to the sibling access pages). Loading → skeletons; errors → inline muted text (Overview must degrade gracefully — a failed sub-query shows "—", never blanks the page).
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint clean.
- [ ] **Step 5 — Commit.** `feat(fe): Access-control Overview page`.

### Task 6: FE — nav + routes + Shared-objects deep-link

**Files:** `frontend/src/components/layout/Sidebar.tsx`, `frontend/src/App.tsx`, `frontend/src/routes/explore/settings/SharedObjects.tsx`.

- [ ] **Step 1 — Failing test.** Extend/create a Sidebar test asserting the "Access control" group now leads with **Overview** and includes **Who-can-access**; a SharedObjects test asserting the new "Who can access?" row action navigates to `/control-plane/access/who-can-access?ws=…&kind=…&id=…`.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement.** In `CONTROL_PLANE_GROUPS` "Access control": prepend `{ label: "Overview", icon: <pick a Lucide e.g. LayoutDashboard/Gauge>, perm: "user:read", to: "access-overview" }` and add `{ label: "Who can access", icon: <e.g. ScanSearch>, perm: "role:read", to: "who-can-access" }` after Access explorer. Add routes in `App.tsx` (`access-overview` → `AccessOverview`, `who-can-access` → `WhoCanAccess`). In `SharedObjects.tsx`, add a dropdown item "Who can access?" using `useNavigate` to the deep-link. Consider making Overview the Control-Plane access landing (optional: index redirect).
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint + `npm run build`.
- [ ] **Step 5 — Commit.** `feat(fe): access nav (Overview + Who-can-access) + shared-objects deep-link`.

### Task 7: Full verification

- [ ] Backend: `cargo fmt --all --check`; `cargo clippy -p egret-auth -p egret-controlplane --all-targets`; `DATABASE_URL=… cargo test -p egret-auth -p egret-controlplane` (report counts; do NOT run two `--workspace` test runs at once).
- [ ] Frontend: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] Confirm Overview degrades gracefully for a non-platform_admin `role:read` viewer (no audit panel, no 403 noise) and Who-can-access 404s cleanly for an unknown object.

---

## Self-Review Checklist

1. **Spec coverage:** Overview KPIs/warnings/recent-changes/quick-links (Task 5); Who-can-access resolver (1) + endpoint (2) + page (4); nav + deep-link (6). ✓
2. **Placeholder scan:** the resolver field name on `CpState` (Task 2) and the stat-tile pattern (Task 5) are read-to-confirm steps, not code placeholders.
3. **Type consistency:** `WhoCanAccess`/`ObjectRef`/`AccessEntry` (BE) ↔ FE `WhoCanAccess` type match; `AccessEntry.effect`/`source` strings consumed by the badge/suffix exactly as produced.
4. **Security:** Who-can-access gated `role:read`; audit panel gated `platform_admin`; resolver cross-workspace correct (object resolved to its own workspace before the workspace-grant join).

## Follow-ups (not this plan)

- Global object **search** picker for Who-can-access (arbitrary unshared objects) + entry from Access Explorer object rows.
- Optional `/access/summary` count endpoint if the Overview list calls prove heavy.
- P1: surface standardization + skeletons; impact-preview on role edit/delete.
