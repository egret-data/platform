# Access Control P3.1 — Classification (tag) model + UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax. Design context: `docs/en/plans/2026-07-13-access-control-p3-abac-design-spike.md`.

**Goal:** The foundation for ABAC — a first-class **classification (tag) model**: a governed global tag catalog (`pii`, `confidential`, `finance`, …) plus the ability to **assign** tags to BI objects, shown as chips. **Metadata only — no enforcement** (data-plane masking = P3.2, Cedar tag policies = P3.3). Independently useful as a "what is sensitive, and where" map.

**Architecture:** A `tags` catalog (global, `tag:manage`-gated) + polymorphic `tag_assignments` keyed like `access_grants` object grants (`object_type` + `object_id`), assigned per-object (`tag:assign` / object-manage-gated). `TagRepo` + both routers live in `egret-bi` (it already owns the object model + `object_ref`/`require_object`/`manage_action` resolution). P3.2 will share this store with `egret-policy` for the data plane.

**Tech Stack:** Rust (sqlx, axum, utoipa) in `egret-bi` + a perm entry in `egret-core`; migration `0080`; React 18 + shadcn + Vitest.

**Scope (MVP):** tag CRUD + assignment to **BI objects** (chart/dashboard/dataset/notebook/saved_query/folder), with the dataset "Tags" affordance as the first assignment surface. **Catalog table/column tagging is P3.2** (needs column identity + the catalog-browse UI — the polymorphic model supports it, the UI is deferred).

---

## Context (verified reuse surfaces)

- `crates/egret-core/src/rbac.rs` — the permission catalog: a `perm!(key, group, desc, scope)` macro list with a `policy:*` "Data Governance" group (`Scope::Platform`) and `role:*`/`group:*` etc. `is_known` validates keys; `is_admin_graph` marks non-delegatable actions. Add `tag:*` (NOT admin-graph — governance perms are delegatable to `data_steward`).
- Role→permission seeding: find how `data_steward` is seeded with `policy:*` (a migration inserting into `role_permissions`) and MIRROR it for `tag:read`/`tag:manage`. (`platform_admin` bypasses everything.)
- `crates/egret-bi/src/shares.rs` — `object_ref(db, kind, id)`, `resolve(st, ctx, kind, id)`, `manage_action(kind)`, `require_object(...)`, `valid_principal_type`/`valid_role` idioms; the `access_grants` object-grant shape (`object_type` + `scope_id`) that `tag_assignments` mirrors. `shares_router()` mounting pattern; `BiState`.
- `crates/egret-bi/src/routes.rs` — `BiState`, `WsContext`, `Auth`, the ws-object routers + how they mount; the object kinds `dashboard|chart|dataset|notebook|saved_query|folder`.
- `frontend/src/components/layout/Sidebar.tsx` — `CONTROL_PLANE_GROUPS` "Data Governance" group (Policies, Audit log) → add "Classifications" (`perm: "tag:read"`).
- `frontend/src/api/access.ts` / `workspaceAccess.ts` — the `apiJson`/generated-client patterns; `frontend/src/routes/control-plane/access/` pages + `FilterableTable`/`ConfirmDeleteDialog`/`ListSkeleton` for the admin page; `frontend/src/components/sharing/ShareDialog.tsx` for the per-object editor pattern (multi-select + chips); `frontend/src/routes/explore/Datasets*.tsx` for where the dataset "Tags" affordance goes.
- Migration numbering: latest `0079`; next **`0080`**. `touch crates/egret-metastore/src/lib.rs` after adding it.

---

## Data model — migration `0080_tags.sql`

```sql
CREATE TABLE tags (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    key         text NOT NULL UNIQUE,            -- machine name, e.g. 'pii' (lowercase/':'/'-'/digits)
    name        text NOT NULL,                   -- display name, e.g. 'PII'
    description text NOT NULL DEFAULT '',
    color       text NOT NULL DEFAULT '',        -- optional UI hint (semantic token name or hex)
    created_by  uuid REFERENCES users(id),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tag_assignments (
    tag_id      uuid NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    object_type text NOT NULL,                   -- chart|dashboard|dataset|notebook|saved_query|folder (P3.2: +catalog_table/column)
    object_id   uuid NOT NULL,
    assigned_by uuid REFERENCES users(id),
    assigned_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tag_id, object_type, object_id)
);
CREATE INDEX tag_assignments_object_idx ON tag_assignments (object_type, object_id);

-- Seed: data_steward gets tag:read + tag:manage (mirror the policy:* seed).
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, p.perm FROM roles r
  CROSS JOIN (VALUES ('tag:read'), ('tag:manage')) AS p(perm)
 WHERE r.scope = 'platform' AND r.name = 'data_steward'
ON CONFLICT DO NOTHING;
```
(Confirm the exact `role_permissions` columns + the data_steward seed style from the existing `policy:*` seed before finalizing the INSERT.)

---

## Endpoint contracts

Tag catalog (control-plane, `tag:manage` for writes / `tag:read` for the list):
- `GET /api/control-plane/tags` → `Vec<TagDto>` (with an `assignment_count`). Gate `tag:read`.
- `POST /api/control-plane/tags` `{ key, name, description?, color? }` → `201 TagDto`. Gate `tag:manage`. 409 on duplicate key; 400 on invalid key.
- `PATCH /api/control-plane/tags/{id}` `{ name?, description?, color? }` → `TagDto`. Gate `tag:manage`.
- `DELETE /api/control-plane/tags/{id}` → `204` (cascades assignments). Gate `tag:manage`.

Object assignment (workspace, gated by object-manage — reuse `require_object(manage_action(kind))` like `grant_put`):
- `GET /api/ws/{workspace}/objects/{kind}/{id}/tags` → `Vec<TagDto>` (the object's tags). Gate: object read.
- `PUT /api/ws/{workspace}/objects/{kind}/{id}/tags/{tag_id}` → `200` (idempotent add). Gate: object manage.
- `DELETE /api/ws/{workspace}/objects/{kind}/{id}/tags/{tag_id}` → `200`. Gate: object manage.

`TagDto { id, key, name, description, color, assignment_count: i64, created_at }` (serde default snake_case).

---

## Non-Goals

- No enforcement (masking/row-filter/allow-deny by tag) — P3.2 (data plane) / P3.3 (Cedar BI).
- No catalog table/column tagging UI (model supports it; UI deferred to P3.2).
- No key=value tags, no tag inheritance/propagation, no bulk-tagging — later.
- No Cedar schema change.

---

## File Structure

**Backend**
- `migrations/0080_tags.sql`; `touch crates/egret-metastore/src/lib.rs`.
- `crates/egret-core/src/rbac.rs` — three `tag:*` perms.
- Create `crates/egret-bi/src/tags.rs` — `TagRepo` (catalog CRUD + assignments) + DTOs + `tag_catalog_router()` (control-plane) + `object_tag_router()` (ws) or add the object-tag routes to the existing object router. `mod tags;` in `egret-bi/src/lib.rs`; wire routers where `shares_router()`/BI routers mount (server). Add `tags: TagRepo` to `BiState` if the handlers need it (or construct per-call from `st.db`).

**Frontend**
- Create `frontend/src/api/tags.ts` — hooks (`useTags`, `useCreateTag`, `useUpdateTag`, `useDeleteTag`, `useObjectTags`, `useAssignTag`, `useUnassignTag`).
- Create `frontend/src/routes/control-plane/governance/Classifications.tsx` (tag catalog admin) + test; nav item + route.
- Create `frontend/src/components/access/TagEditor.tsx` (multi-select + chips for an object) + a `TagChips` display; wire into the datasets surface + test.

---

## Tasks

### Task 1: BE — migration + rbac perms + TagRepo

**Files:** `migrations/0080_tags.sql`; `egret-metastore/src/lib.rs`; `egret-core/src/rbac.rs`; `egret-bi/src/tags.rs` (new); `egret-bi/src/lib.rs`. Test: `crates/egret-bi/tests/tags_db.rs`.

- [ ] **Step 1** — migration (above; confirm the `role_permissions` seed shape against the existing `policy:*` seed) + touch metastore.
- [ ] **Step 2 — rbac perms.** Add to `rbac.rs`: `perm!("tag:read","Classifications","View classification tags",Scope::Platform)`, `perm!("tag:manage","Classifications","Create, edit, and delete classification tags",Scope::Platform)`, `perm!("tag:assign","Classifications","Assign or remove tags on objects",Scope::Workspace)`. NOT admin-graph. Confirm the perm-uniqueness test still passes.
- [ ] **Step 3 — Failing test** (`tags_db.rs`, mirror an egret-bi DB test): `TagRepo::create` inserts a tag; duplicate `key` → Conflict; `list` returns it with `assignment_count`; `update`/`delete`; `assign(tag, object_type, object_id, by)` + `unassign` + `tags_for_object(object_type, object_id)` round-trip; `delete` cascades assignments. Run `DATABASE_URL=… cargo test -p egret-bi --test tags_db` → FAIL.
- [ ] **Step 4 — Implement** `TagRepo { db }`: `create`/`list`(LEFT JOIN count)/`get`/`update`/`delete`; `assign`(INSERT ON CONFLICT DO NOTHING)/`unassign`(DELETE)/`tags_for_object`(JOIN). Validate `key` (lowercase alnum/`:`/`-`). Parameterized SQL. `mod tags;` in lib.rs.
- [ ] **Step 5 — Run, confirm pass**; `cargo test -p egret-bi` (no regression). fmt + clippy.
- [ ] **Step 6 — Commit.** `feat(access): classification tag model — tags + tag_assignments (mig 0080) + tag:* perms`.

### Task 2: BE — tag catalog endpoints (control-plane)

**Files:** `egret-bi/src/tags.rs` (+ `routes.rs`/server wiring). Test: `crates/egret-bi/tests/tags_routes.rs`.

- [ ] **Step 1 — Failing test.** A `tag:manage` caller POSTs a tag (201), lists it (`tag:read`), PATCHes it, DELETEs it (204); a caller lacking `tag:read` → 403; duplicate key → 409. (Mirror an egret-bi route-test harness that authenticates a principal with given perms.)
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement** `TagDto` + `tag_catalog_router()` (`GET/POST/PATCH/DELETE /control-plane/tags`) gated `tag:read`/`tag:manage` via `require`; mount it where control-plane routers mount (grep the server for the control-plane router `.nest`/`.merge`); OpenAPI fragment.
- [ ] **Step 4 — Run, confirm pass.**
- [ ] **Step 5 — Commit.** `feat(access): tag catalog endpoints (list/create/update/delete)`.

### Task 3: BE — object tag assignment endpoints

**Files:** `egret-bi/src/tags.rs` (+ router mount). Test: extend `tags_routes.rs`.

- [ ] **Step 1 — Failing test.** For a dataset in a workspace: an object-manager PUTs a tag on `/api/ws/{ws}/objects/dataset/{id}/tags/{tag_id}` (200), GET lists it, DELETE removes it; a non-manager (object read only) PUT → 403; GET as a reader → 200. Cross-workspace object → 404 (via `resolve`).
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement** the three handlers reusing `resolve(st,ctx,kind,id)` + `require_object(manage_action(kind))` for writes and object-read for GET (mirror `grant_put`/`grant_list` gating). Route + OpenAPI.
- [ ] **Step 4 — Run, confirm pass** (incl. the non-manager 403 + cross-workspace 404).
- [ ] **Step 5 — Commit.** `feat(access): object tag assignment endpoints (list/assign/unassign)`.

### Task 4: FE — Classifications admin page

**Files:** create `api/tags.ts`; `routes/control-plane/governance/Classifications.tsx` + test; `Sidebar.tsx` (+ `App.tsx` route).

- [ ] **Step 1 — Failing test.** `Classifications` (mock `@/api/tags`): renders tags in a `FilterableTable` (name, key mono, description, assignment_count); "New tag" opens a create dialog calling `useCreateTag`; edit + delete (via `ConfirmDeleteDialog`) wired; 403 → alert. Also assert the sidebar Data-Governance group includes a "Classifications" item.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement** `api/tags.ts` (all hooks, `apiJson`) + `Classifications.tsx` (mirror `RolesPage`/`GroupsPage` idioms: header, `FilterableTable`, create/edit dialogs, `ConfirmDeleteDialog`, `ListSkeleton`, `errorStatus` from `@/lib/httpStatus`). A tag row shows a color chip. Add `{ label: "Classifications", icon: <Tag/Tags Lucide>, perm: "tag:read", to: "classifications" }` to the Data-Governance group in `Sidebar.tsx` + the route in `App.tsx`.
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint.
- [ ] **Step 5 — Commit.** `feat(fe): Classifications admin page (tag catalog)`.

### Task 5: FE — TagEditor + chips on datasets

**Files:** create `components/access/TagEditor.tsx` (+ `TagChips`) + test; wire into the datasets surface.

- [ ] **Step 1 — Failing test.** `TagEditor` (mock `useTags` + `useObjectTags` + assign/unassign): shows the object's current tags as removable chips + a picker to add from the catalog; adding calls `useAssignTag`, removing calls `useUnassignTag`. `TagChips` renders a read-only chip list.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement** `TagEditor({ ws, kind, id })` — `useObjectTags(ws,kind,id)` chips (each removable via `useUnassignTag`) + an add `<Select>`/combobox from `useTags()` (excluding already-assigned) calling `useAssignTag`. `TagChips({ tags })` for read-only display (color from the tag). Wire a "Tags" affordance on datasets — a dialog/popover opened from the dataset row/detail (mirror where "Share" lives, e.g. the dataset actions menu); show `TagChips` where datasets are listed. Keep it reusable for charts/dashboards (note it, don't build those surfaces now).
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint + `npm run build`.
- [ ] **Step 5 — Commit.** `feat(fe): object tag editor + chips (datasets)`.

### Task 6: Full verification

- [ ] Backend: `cargo fmt --all --check`; `cargo clippy -p egret-core -p egret-bi --all-targets`; `DATABASE_URL=… cargo test -p egret-core -p egret-bi` (report counts).
- [ ] Frontend: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] Sanity: a non-`tag:read` user can't see Classifications; a non-object-manager can't assign tags to an object; deleting a tag removes its chips everywhere.

---

## Self-Review Checklist

1. **Spec coverage:** tags table+repo (T1), catalog endpoints (T2), assignment endpoints (T3), admin page (T4), editor+chips (T5).
2. **Metadata-only:** confirm NOTHING here reads `tag_assignments` in an authz decision (`grants_for`, Cedar, Ranger untouched) — enforcement is P3.2/P3.3.
3. **Auth:** catalog writes `tag:manage`; catalog read `tag:read`; assignment gated by object-manage (reuse `require_object`); cross-workspace via `resolve` → 404.
4. **Type consistency:** `TagDto` (BE) ↔ FE `Tag` type; `object_type`/`object_id` assignment keys mirror the `access_grants` object shape.

## Follow-ups (P3.2+)

- Catalog **table/column** tag targets + the catalog-browse tagging UI (needed by P3.2 masking).
- **P3.2:** serve real `download_tags` from `tag_assignments`; tag-based Ranger mask/row/access policies.
- **P3.3:** load the Cedar schema + `resource.tags` attribute + tag-conditioned forbid on BI objects (Who-can-access shows "blocked by tag policy").
- `tag:manage` vs `tag:assign` split enforcement; tag chips on charts/dashboards; key=value tags.
