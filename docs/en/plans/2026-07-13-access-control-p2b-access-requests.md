# Access Control P2b — Access-request → approval workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A governed **request → approve** loop: a user requests to **join a workspace**; a workspace admin sees a pending queue and **approves** (granting time-boxed membership via P2a's `expires_at`) or **denies**. This shifts access from "an admin configures everything" to "self-service, governed".

**Architecture:** A new `access_requests` table + repo in `egret-workspace` (the workspace-membership seam). Requester endpoints (list joinable workspaces, create, my requests, cancel) and approver endpoints (per-workspace inbox, approve, deny) gate through the existing patterns (`Auth`/`WsContext` + `workspace:manage_members`). Approval reuses `MemberRepo` to add the member with an optional expiry (P2a), and emits an audit event (B4). No new authz mechanism.

**Tech Stack:** Rust (sqlx, axum, utoipa) in `egret-workspace`; migration `0079`; React 18 + shadcn + Vitest.

**Scope (MVP):** workspace-JOIN requests only. Object-level access requests and role-elevation requests are follow-ups. No email/push notifications (the inbox is the surface).

---

## Context (verified reuse surfaces)

- `crates/egret-workspace/src/members.rs` — `MemberRepo::add(ws, ptype, pid, role, by)` inserts a workspace grant (replace-then-insert; no expiry yet). Approval needs an `add_with_expiry(…, expires_at: Option<DateTime<Utc>>)` variant (P2a column `access_grants.expires_at` exists) — have `add` delegate with `None`. `resolve_principal`, `role_of`, `count_ws_admins` also here.
- `crates/egret-workspace/src/workspaces.rs` — `WorkspaceRepo::list_for_principal(user_id, include_all)` lists a user's workspaces (via `access_grants`, already expiry-filtered by P2a). "Joinable" = active workspaces MINUS that set.
- `crates/egret-workspace/src/routes.rs` — `WorkspaceState { workspaces, members, authz, pdp, users, sessions, audit }`; `WsContext(ctx)` gives `ctx.workspace_id` + `ctx.principal.user_id`; the `list_members` handler shows the gate pattern `require(&*st.pdp,&ctx,"workspace:manage_members")` + `st.audit.emit(AuditEvent::Mutation{…})`. `me_router` (`/api/me/workspaces`) and `ws_member_router` (`/api/ws/...`) mount points.
- `crates/egret-workspace/src/context.rs` — `WsContext` admits members/admins; a NON-member is `Forbidden` at the extractor. So approver endpoints use `WsContext` (only members reach them, then `manage_members` narrows to admins). Requester endpoints that are NOT workspace-scoped use `Auth(p)` (any authenticated user) — mount them OUTSIDE `ws_member_router` (e.g. a new `access_request_router` at `/api`).
- FE: `frontend/src/components/layout/WorkspaceSwitcher.tsx` — the requester entry point (add a "Request access…" item). `frontend/src/routes/explore/settings/WorkspaceSettings.tsx` — the B1 settings tab bar (Members · Roles & permissions · Shared objects) → add a **Requests** tab. `frontend/src/api/members.ts` (`apiJson` pattern) + `frontend/src/api/workspaceAccess.ts` for the new hooks. `frontend/src/routes/explore/settings/Members.tsx` for the FilterableTable / dialog idioms.
- Migration numbering: latest `0078`; next **`0079`**. `touch crates/egret-metastore/src/lib.rs` after adding it.

---

## Data model — migration `0079_access_requests.sql`

```sql
CREATE TABLE access_requests (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    requester_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id   uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    requested_role text NOT NULL DEFAULT 'ws_viewer',
    reason         text NOT NULL DEFAULT '',
    status         text NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending','approved','denied','cancelled')),
    decided_by     uuid REFERENCES users(id),
    decided_at     timestamptz,
    decision_note  text,
    expires_at     timestamptz,          -- approver-chosen grant expiry (P2a); NULL = no expiry
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX access_requests_ws_status_idx ON access_requests (workspace_id, status);
CREATE INDEX access_requests_requester_idx ON access_requests (requester_id);
-- At most one OPEN request per (requester, workspace).
CREATE UNIQUE INDEX access_requests_one_pending
  ON access_requests (requester_id, workspace_id) WHERE status = 'pending';
```

---

## Endpoint contracts

Requester side (mount at `/api`, `Auth(p)` = any authenticated user):
- `GET /api/access-requests/joinable` → `Vec<JoinableWorkspaceDto{ id, slug, name }>` — active workspaces the caller is NOT a member of. (Visibility note: exposes workspace name/slug to authenticated users for request purposes — acceptable for a request-to-join model.)
- `POST /api/access-requests` body `{ workspace_id: Uuid, role?: string (default ws_viewer), reason?: string }` → `201 AccessRequestDto`. Reject (409) if caller is already a member of that workspace or already has a pending request (the partial unique index).
- `GET /api/access-requests/mine` → `Vec<AccessRequestDto>` (caller's, all statuses, newest first).
- `POST /api/access-requests/{id}/cancel` → `200` — only the requester, only while `pending`.

Approver side (mount under `ws_member_router` at `/api/ws/{workspace}`, `WsContext` + `workspace:manage_members`):
- `GET /api/ws/{workspace}/access-requests?status=pending` → `Vec<AccessRequestDto>` (default pending; enriched with requester email/name).
- `POST /api/ws/{workspace}/access-requests/{id}/approve` body `{ role?: string, expires_at?: DateTime<Utc> }` → `200` — verify the request belongs to `{workspace}` and is `pending`; add the member via `add_with_expiry(role ?? requested_role, expires_at)`; set `status='approved', decided_by, decided_at`. Audit `access_request:approve`.
- `POST /api/ws/{workspace}/access-requests/{id}/deny` body `{ note?: string }` → `200` — verify + set `status='denied', decided_by, decided_at, decision_note`. Audit `access_request:deny`.

`AccessRequestDto { id, workspace_id, workspace_name, requester_id, requester_name, requested_role, reason, status, decided_by, decided_at, decision_note, expires_at, created_at }` (serde default snake_case; timestamps RFC3339 strings).

---

## Non-Goals

- Object-level or role-elevation requests (workspace-join only this MVP).
- Notifications/email; multi-step or multi-approver flows; SLA/auto-expiry of requests.
- Requester picking a specific approver.

---

## File Structure

**Backend**
- `migrations/0079_access_requests.sql`; touch `crates/egret-metastore/src/lib.rs`.
- Create `crates/egret-workspace/src/access_requests.rs` — `AccessRequestRepo` (create, list_for_workspace, list_for_requester, get, approve, deny, cancel, joinable) + DTOs; `mod access_requests;` in `lib.rs`.
- Modify `crates/egret-workspace/src/members.rs` — `add_with_expiry`.
- Modify `crates/egret-workspace/src/routes.rs` — two routers (requester `/api/access-requests`, approver under `ws_member_router`) + handlers + `WorkspaceState` already has what's needed; register OpenAPI. Wire the new router into the app (`crates/egret-bin`/server where `ws_member_router`/`me_router` mount).

**Frontend**
- Create `frontend/src/api/accessRequests.ts` — hooks (`useJoinable`, `useCreateRequest`, `useMyRequests`, `useCancelRequest`, `useWorkspaceRequests`, `useApproveRequest`, `useDenyRequest`) via `apiJson`.
- Create `frontend/src/components/access/RequestAccessDialog.tsx` (requester) + wire into `WorkspaceSwitcher.tsx`.
- Create `frontend/src/routes/explore/settings/AccessRequests.tsx` (approver inbox tab) + add the tab to `WorkspaceSettings.tsx` + route in `App.tsx`.
- Tests alongside.

---

## Tasks

### Task 1: BE — migration + AccessRequestRepo + member add-with-expiry

**Files:** `migrations/0079_access_requests.sql`; `egret-metastore/src/lib.rs` (touch); `egret-workspace/src/access_requests.rs` (new); `members.rs` (`add_with_expiry`); `lib.rs`. Test: `crates/egret-workspace/tests/access_requests_db.rs`.

- [ ] **Step 1** — write the migration (above) + touch metastore.
- [ ] **Step 2 — Failing test** (mirror `access_views_db.rs` harness): `AccessRequestRepo::create` inserts a pending request; a second create for the same (requester, workspace) while pending → errors (unique). `list_for_requester` + `list_for_workspace('pending')` return it. `approve` sets status/decided_by/decided_at AND the member exists with the chosen expiry (assert the `access_grants` row's `expires_at`). `deny` sets denied+note. `cancel` sets cancelled. `joinable(user)` excludes workspaces the user is already in. `MemberRepo::add_with_expiry` persists `expires_at`.
- [ ] **Step 3 — Run, confirm fail.** `DATABASE_URL=… cargo test -p egret-workspace --test access_requests_db`.
- [ ] **Step 4 — Implement.** `AccessRequestRepo { db }` methods (parameterized SQL). `create` → INSERT (map the unique violation to `Error::Conflict`). `approve(id, ws, role, expires_at, by)` in a tx: verify row is pending + belongs to ws, call `members.add_with_expiry`, UPDATE status. `deny`/`cancel` UPDATE with status guards. `joinable(user_id)` = `SELECT id,slug,name FROM workspaces WHERE status='active' AND id NOT IN (list_for_principal set)` — reuse/mirror the `list_for_principal` grant subquery (expiry-filtered). Add `MemberRepo::add_with_expiry` (copy `add`, add the `expires_at` column to the INSERT); `add` delegates with `None`.
- [ ] **Step 5 — Run, confirm pass.** Also `cargo test -p egret-workspace` (no regression — `add` behavior unchanged).
- [ ] **Step 6 — fmt + clippy + commit.** `feat(access): access_requests table + repo (mig 0079); member add-with-expiry`.

### Task 2: BE — requester endpoints (joinable, create, mine, cancel)

**Files:** `routes.rs` (+ wire new `access_request_router` into the server). Test: extend `access_requests` route tests (or `crates/egret-controlplane`/`egret-workspace` route harness).

- [ ] **Step 1 — Failing test.** Auth'd non-member GETs `/api/access-requests/joinable` → sees a workspace they're not in; POST creates a request (201); second POST → 409; GET `/mine` shows it; POST `/{id}/cancel` → cancelled; cancel by a different user → 403/404.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement** the four handlers with `Auth(p)` (any authenticated user), DTOs, `access_request_router()` mounted at `/api`, OpenAPI fragment. `cancel` checks `requester_id == p.user_id`.
- [ ] **Step 4 — Run, confirm pass.**
- [ ] **Step 5 — Commit.** `feat(access): requester endpoints — joinable / create / mine / cancel`.

### Task 3: BE — approver endpoints (inbox, approve, deny)

**Files:** `routes.rs` (under `ws_member_router`). Test: extend.

- [ ] **Step 1 — Failing test.** A ws_admin GETs `/api/ws/{ws}/access-requests` → the pending request (enriched requester name); a ws_viewer → 403. Approve with `{role:"ws_editor", expires_at:+7d}` → request approved AND the requester is now a member with ws_editor + that expiry (assert via members list / access_grants). Approving a request for a DIFFERENT workspace via this ws path → 404 (cross-workspace guard). Deny sets denied+note. Audit events emitted.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement** three handlers with `WsContext` + `require(…,"workspace:manage_members")`; verify `request.workspace_id == ctx.workspace_id` (else 404 — IDOR guard); approve calls the repo (which adds the member) + `st.audit.emit`. Register OpenAPI.
- [ ] **Step 4 — Run, confirm pass** (incl. the cross-workspace 404 + the member-created-with-expiry assertion).
- [ ] **Step 5 — Commit.** `feat(access): approver endpoints — inbox / approve / deny (+ audit)`.

### Task 4: FE — api hooks + Request-access dialog (requester)

**Files:** create `api/accessRequests.ts`; `components/access/RequestAccessDialog.tsx` + test; modify `WorkspaceSwitcher.tsx`.

- [ ] **Step 1 — Failing test.** `RequestAccessDialog` (mock the hooks): shows joinable workspaces in a select + role select + reason; submit calls create with the chosen values; renders "my requests" with their status (pending/approved/denied) and a Cancel action on pending ones.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement.** `api/accessRequests.ts` (`apiJson`, hooks + types matching the DTOs). `RequestAccessDialog`: a Dialog with joinable `<Select>` + role `<Select>` (ws_viewer/ws_editor) + reason `<Textarea>` + submit; below, a list of `useMyRequests()` with status badges + Cancel. Add a "Request access…" item to `WorkspaceSwitcher` opening the dialog (place near the workspace list; visible to any signed-in user).
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint.
- [ ] **Step 5 — Commit.** `feat(fe): request-access dialog + workspace-switcher entry`.

### Task 5: FE — approver Requests tab

**Files:** create `routes/explore/settings/AccessRequests.tsx` + test; modify `WorkspaceSettings.tsx` (tab) + `App.tsx` (route).

- [ ] **Step 1 — Failing test.** `AccessRequests` (mock hooks): renders pending requests (requester, requested role, reason, created); Approve opens a small form (role + optional expiry date) → calls approve; Deny (optional note) → calls deny; 403 → "Only workspace admins…" alert; empty → "No pending requests." Also assert the settings tab bar now includes a "Requests" link.
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement.** `AccessRequests.tsx`: `useWorkspaceRequests(ws,'pending')` → `FilterableTable`/list with Approve/Deny per row (Approve = a popover/dialog with a role select + optional `<input type="date">` expiry, reusing the P2a end-of-day→RFC3339 conversion; Deny = optional note). ListSkeleton/403/empty per the B1 idioms. Add the `requests` tab to `WorkspaceSettings.tsx` (after Shared objects) + the `requests` child route in `App.tsx`.
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint + `npm run build`.
- [ ] **Step 5 — Commit.** `feat(fe): workspace access-requests inbox (approve/deny)`.

### Task 6: Full verification

- [ ] Backend: `cargo fmt --all --check`; `cargo clippy -p egret-workspace --all-targets`; `DATABASE_URL=… cargo test -p egret-workspace` (report counts).
- [ ] Frontend: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] Security sanity: a non-member cannot approve/deny (WsContext+manage_members); a requester can only cancel their own; the approve path can't grant access to a request from another workspace (cross-workspace 404); the created grant is time-boxed when an expiry was chosen (enforced by P2a).

---

## Self-Review Checklist

1. **Spec coverage:** table+repo (T1), requester CRUD (T2), approver inbox+approve+deny (T3), requester UI (T4), approver UI (T5).
2. **Security:** approver endpoints `WsContext`+`manage_members`; request.workspace_id == ctx guard (IDOR); cancel restricted to requester; joinable excludes already-member; approval grant honors P2a expiry (enforced automatically).
3. **Reuse, not reinvent:** approval adds the member via `MemberRepo` (+ expiry); no second grant path. Audit via `st.audit`.
4. **Type consistency:** `AccessRequestDto` (BE) ↔ FE type field-for-field; `expires_at` RFC3339 ↔ FE date input conversion (reuse P2a's).

## Follow-ups (P2b-2 / later)

- Object-level + role-elevation requests (reuse the same table with a target shape mirroring `access_grants`).
- A pending-count badge on the Requests tab / a platform-admin global inbox.
- Notifications (email/in-app) on request + decision; request auto-expiry/SLA.
- Requester "my requests" as its own page; re-request after denial cooldown.
