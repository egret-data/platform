# Service accounts SA1 — Principal backbone — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Promote the service account to a control-plane-managed first-class principal with platform/workspace scope, group/role membership, and disable→immediate-revoke (Data-API audience). Backend only.

**Architecture:** A new platform-level `service_accounts` table is the SA principal of record (`user_id → users`, scope, workspace, name, description). The existing `dataapi.service_accounts` becomes a backward-compatible **view** over it (scope='workspace') with an INSTEAD OF INSERT trigger, so all existing Data-API code is unchanged; `dataapi.clients`'s FK repoints to the new table. A `ServiceAccountRepo` in `egret-auth` and a `/api/control-plane/service-accounts` surface in `egret-controlplane` manage SAs and their group/role membership, reusing the user-id principal plumbing that already exists.

**Tech Stack:** Rust, axum, sqlx (Postgres), utoipa (OpenAPI), the existing `UserRepo`/`GroupRepo`/`RoleRepo` in `egret-auth`.

## Global Constraints

- Branch `feat/svc-accounts-sa1` (worktree `../egret-sa1`, off `ce713769e`). `CARGO_TARGET_DIR=target-sa1` on EVERY cargo command. Throwaway Postgres `DATABASE_URL` required — the DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (`target-sa1/` is not gitignored — never `git add -A`). Co-author line `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- Migration numbers are a shared namespace — the migration number is provisional; renumber above main AND open PRs at merge.
- Run before each commit: `CARGO_TARGET_DIR=target-sa1 cargo fmt` and `CARGO_TARGET_DIR=target-sa1 cargo clippy -p <crate> --all-targets -- -D warnings`.
- SA1 does **not** touch the frontend (SA3), does **not** re-own `catalog_clients` or the Catalog kill switch (SA2), and does **not** mint credentials (creation of clients stays on the Data-API routes).

---

### Task 1: Migration — `service_accounts` table, backfill, Data-API compatibility view

**Files:**
- Create: `migrations/0229_service_account_principals.sql` (number provisional)
- Test: `crates/egret-dataapi/tests/service_account_view_db.rs` (new, DB-gated)

**Interfaces:**
- Produces: a base table `public.service_accounts(user_id PK, scope, workspace_id, name, description, created_by, created_at)`; a view `dataapi.service_accounts(user_id, workspace_id, name, description, created_by, created_at)` that is SELECT-able and INSERT-able exactly as the old table was; `dataapi.clients.service_account_id` now FKs `service_accounts(user_id)`.

- [ ] **Step 1: Write the migration.**

```sql
-- 0229_service_account_principals.sql
-- SA1: the service account becomes a first-class principal of record. The
-- platform-level identity (scope + name) moves here from dataapi.service_accounts,
-- which becomes a backward-compatible view so existing Data-API code is unchanged.
CREATE TABLE service_accounts (
  user_id      uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  scope        text NOT NULL CHECK (scope IN ('platform','workspace')),
  workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT service_account_scope_ws CHECK (
    (scope = 'platform'  AND workspace_id IS NULL) OR
    (scope = 'workspace' AND workspace_id IS NOT NULL)
  )
);
CREATE UNIQUE INDEX service_accounts_ws_name
  ON service_accounts (workspace_id, name) WHERE scope = 'workspace';
CREATE UNIQUE INDEX service_accounts_platform_name
  ON service_accounts (name) WHERE scope = 'platform';
CREATE INDEX service_accounts_ws_idx ON service_accounts (workspace_id);

-- Backfill: every existing dataapi.service_accounts row is workspace-scoped.
INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by, created_at)
  SELECT user_id, 'workspace', workspace_id, name, description, created_by, created_at
    FROM dataapi.service_accounts;

-- Repoint dataapi.clients' FK from the old table to the new base table.
-- Find the real constraint name first (it is auto-named); see Step 2 note.
ALTER TABLE dataapi.clients DROP CONSTRAINT clients_service_account_id_fkey;
ALTER TABLE dataapi.clients
  ADD CONSTRAINT clients_service_account_id_fkey
  FOREIGN KEY (service_account_id) REFERENCES service_accounts(user_id) ON DELETE CASCADE;

-- Replace the old table with a view over the new base (workspace scope only),
-- exposing exactly the columns Data-API reads/writes.
DROP TABLE dataapi.service_accounts;
CREATE VIEW dataapi.service_accounts AS
  SELECT user_id, workspace_id, name, description, created_by, created_at
    FROM service_accounts WHERE scope = 'workspace';

-- The Data-API create path does `INSERT INTO dataapi.service_accounts (...)`;
-- route it into the base table as a workspace SA, filling created_at for RETURNING.
CREATE FUNCTION dataapi.service_accounts_insert() RETURNS trigger AS $$
BEGIN
  INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by)
    VALUES (NEW.user_id, 'workspace', NEW.workspace_id, NEW.name,
            COALESCE(NEW.description, ''), NEW.created_by)
    RETURNING created_at INTO NEW.created_at;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER service_accounts_insert_trg
  INSTEAD OF INSERT ON dataapi.service_accounts
  FOR EACH ROW EXECUTE FUNCTION dataapi.service_accounts_insert();
```

> **Constraint-name note (do, don't guess):** before committing, confirm the FK's real name against the throwaway DB: `psql "$DATABASE_URL" -c "\d dataapi.clients"` (or query `information_schema.table_constraints` for `table_name='clients'` and `constraint_type='FOREIGN KEY'`). `clients_service_account_id_fkey` is Postgres's default auto-name for a single-column FK on that column, but verify; if it differs, use the real name in the `DROP CONSTRAINT`.

- [ ] **Step 2: Write the failing DB test.**

In `crates/egret-dataapi/tests/service_account_view_db.rs` (model the harness on an existing DB-gated dataapi test — same `DATABASE_URL`/skip-guard shape; a module `static SERIAL: tokio::sync::Mutex<()>` if the suite uses one):

```rust
// Arrange: run migrations on a fresh DB (the test harness does this), seed a
// workspace + a service user via the same path the app uses.
// Act + Assert:
// 1. INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, description, created_by)
//    ... RETURNING created_at  -> succeeds through the trigger, created_at is non-null.
// 2. The row is visible BOTH via `SELECT ... FROM dataapi.service_accounts WHERE workspace_id=$1`
//    AND via `SELECT scope, workspace_id FROM service_accounts WHERE user_id=$1`
//    with scope='workspace'.
// 3. A dataapi.clients row FK-referencing that user_id inserts OK (FK now points at base).
// 4. Deleting the users row cascades the service_accounts row away (count = 0).
```

(Write the concrete sqlx statements; assert each step. Seed the service user with `INSERT INTO users (provider, email, name, username) VALUES ('service', $1, $1, $1)` as the existing `people.rs` tests do, or via `UserRepo::insert_service_with` if reachable from the test.)

- [ ] **Step 3: Run the test, verify it passes.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-dataapi --test service_account_view_db -- --nocapture`
Expected: PASS, and the test actually executes (not skipped). If it skips, fix `DATABASE_URL`.

- [ ] **Step 4: Confirm the existing Data-API SA tests still pass against the view.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-dataapi service_account 2>&1 | tail -30` (and any `clients`/`routes` SA tests). Expected: PASS — the view + trigger are a drop-in.

- [ ] **Step 5: Commit.**

```bash
git add migrations/0229_service_account_principals.sql crates/egret-dataapi/tests/service_account_view_db.rs
git commit -m "feat(sa): service_accounts principal table with a Data-API compatibility view (SA1)"
```

---

### Task 2: `ServiceAccountRepo` in `egret-auth`

**Files:**
- Create: `crates/egret-auth/src/service_accounts.rs`
- Modify: `crates/egret-auth/src/lib.rs` (add `pub mod service_accounts;` and re-export)
- Test: `crates/egret-auth/tests/service_accounts_db.rs` (new, DB-gated)

**Interfaces:**
- Consumes: `UserRepo::insert_service_with(executor, email, name) -> Result<UserId>` (already generic over the executor); the `users.status` update path (find the existing status setter used by the control-plane `status` branch — `UserRepo` method or a raw statement — and reuse it so session/token revocation stays consistent).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaScope { Platform, Workspace }

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceAccount {
    pub user_id: Uuid,
    pub scope: SaScope,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub status: String, // mirrors users.status
}

pub struct ServiceAccountRepo { pool: PgPool }

impl ServiceAccountRepo {
    pub fn new(pool: PgPool) -> Self;
    pub async fn create(&self, scope: SaScope, workspace_id: Option<Uuid>, name: &str, description: &str, created_by: Uuid) -> Result<ServiceAccount>;
    pub async fn list(&self, scope: Option<SaScope>, workspace_id: Option<Uuid>) -> Result<Vec<ServiceAccount>>;
    pub async fn get(&self, user_id: Uuid) -> Result<Option<ServiceAccount>>;
    pub async fn delete(&self, user_id: Uuid) -> Result<()>;
    pub async fn disable(&self, user_id: Uuid) -> Result<()>;
    pub async fn enable(&self, user_id: Uuid) -> Result<()>;
}
```

- [ ] **Step 1: Write failing tests.** In `service_accounts_db.rs` (DB-gated, same skip-guard as other `egret-auth` `*_db.rs` tests):
  - `create` a **workspace** SA → row in `service_accounts` (scope='workspace', workspace_id set), a `users` row with `provider='service'`, `password_hash IS NULL`, `status='active'`, username starting `svc-`.
  - `create` a **platform** SA (workspace_id=None) → scope='platform', workspace_id NULL; `users` row as above.
  - `create` with scope/workspace mismatch (Platform + Some(ws), or Workspace + None) → `Err` (the repo validates before the DB CHECK fires).
  - `list(Some(Workspace), Some(ws))` returns only that workspace's SAs; `list(Some(Platform), None)` returns only platform SAs.
  - `disable` sets `status<>'active'`; `enable` restores `'active'`; `get` reflects it.
  - `delete` removes the `users` row and (by cascade) the `service_accounts` row.

- [ ] **Step 2: Run tests, verify they fail.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-auth --test service_accounts_db`
Expected: FAIL — module/methods absent.

- [ ] **Step 3: Implement `ServiceAccountRepo`.**
  - `create`: open a tx. Validate `(scope, workspace_id)` agree (return a domain error, not a DB error, on mismatch). Derive the synthetic email + rely on `insert_service_with` for the `users` row and `svc-` username:
    - **Workspace scope:** derive the email the SAME way the Data-API create path does (name + workspace slug) — read that derivation in `crates/egret-dataapi/src/routes.rs` `create_service_account` and reuse an equivalent so a workspace SA created here is indistinguishable from one created via Data-API.
    - **Platform scope:** no workspace slug exists. Derive `email = format!("svc-{}@service.egret.local", slug(name))` and let `insert_service_with` allocate the `svc-{slug}` username via the existing ledger path. (Confirm `insert_service_with`'s exact email/username contract first; if it derives the username from `name`, pass `name` so the ledger owns `svc-` allocation — see [[egret-username-takeover-package]].)
    - Then `INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by) ... RETURNING created_at`. Map a unique violation to a domain `Conflict`. Commit. Return the `ServiceAccount` (status='active').
  - `list`/`get`: `SELECT sa.*, u.status FROM service_accounts sa JOIN users u ON u.id = sa.user_id` with optional `scope`/`workspace_id` filters, `ORDER BY sa.created_at DESC`.
  - `disable`/`enable`: set `users.status` through the SAME mechanism the control-plane `status` branch uses (so downstream revocation stays correct). If that mechanism lives in `egret-controlplane`, set `users.status` here via `UserRepo` and leave session/token revocation to the route layer (Task 3) — note which, so Task 3 wires the rest.
  - `delete`: `DELETE FROM users WHERE id=$1` (cascade handles the rest).

- [ ] **Step 4: Run tests, verify they pass.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-auth --test service_accounts_db`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-auth/src/service_accounts.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/service_accounts_db.rs
git commit -m "feat(sa): ServiceAccountRepo — create/list/get/disable/delete (SA1)"
```

---

### Task 3: Control-plane `/service-accounts` surface + membership + guards

**Files:**
- Create: `crates/egret-controlplane/src/access/service_accounts.rs`
- Modify: `crates/egret-controlplane/src/access/mod.rs` (mount the routes in the router builder; wire `ServiceAccountRepo` into `CpState` if not already reachable)
- Modify: `crates/egret-controlplane/src/lib.rs` if `CpState` needs the repo
- Test: `crates/egret-controlplane/tests/service_account_principals.rs` (new, DB-gated; model the harness on `service_account_surfaces.rs` — `control_plane_router()` UNNESTED, relative routes)

**Interfaces:**
- Consumes: `ServiceAccountRepo` (Task 2); the existing group-member add/remove and `platform_role_assignments` add/remove helpers (find them in `egret-auth`/`egret-controlplane` — the group routes near `access/mod.rs:631+` and the role-assignment code in `roles.rs`); the OIDC-group predicate used by the OIDC-member reports in `groups.rs`; the `platform_admin` constant/lookup in `roles.rs`; `require_permission`/`egret_core::authz::require`.
- Produces: routes under `/service-accounts` (relative in the unnested router; `/api/control-plane/service-accounts` mounted). Reuse the people-route permission strings verbatim: `user:read` (list/get), `user:create` (create), `user:update` (disable/enable, membership), `user:delete` (delete). DTOs: `ServiceAccountDto`, `CreateServiceAccountBody { scope, workspace_id?, name, description? }`, `AddGroupBody { group_id }`, `AddRoleBody { role_id, scope_type, workspace_id? }`.

- [ ] **Step 1: Write failing tests** (`service_account_principals.rs`, DB-gated, AllowAll PDP fixture like `service_account_surfaces.rs`):
  - `POST /service-accounts {scope:"workspace", workspace_id, name}` → 200, body has user_id/scope/name; a second identical POST → 409.
  - `POST /service-accounts {scope:"platform", name}` → 200, scope platform, workspace_id null.
  - `GET /service-accounts?scope=platform` lists only platform SAs; `?workspace_id=` filters.
  - `GET /service-accounts/{id}` → the SA plus its (empty) group/role memberships.
  - `POST /service-accounts/{id}/groups {group_id}` into a LOCAL group → 200; into an **OIDC** group → 4xx with a message about IdP-owned membership.
  - `POST /service-accounts/{id}/roles {role_id=<platform_admin>, scope_type:"platform"}` → 4xx refusing platform_admin; a non-admin role → 200.
  - `POST /service-accounts/{id}/disable` → the SA's `data-api` client fails `ClientStore::verify` (or, if verify is not reachable in this crate's tests, assert `users.status<>'active'` and leave the verify assertion to Task 1/Task 4's dataapi-side test — pick the reachable assertion and note it).
  - `DELETE /service-accounts/{id}` → 204/200; `GET` afterward → 404.
  - **Human-route unchanged:** `PATCH /users/{sa_id}` with a name still 400s (`refuse_service_account_profile` stays); with `{status:"disabled"}` still 200s.

- [ ] **Step 2: Run tests, verify they fail.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-controlplane --test service_account_principals`
Expected: FAIL — routes absent.

- [ ] **Step 3: Implement the routes + handlers + guards.**
  - Each handler calls `egret_core::authz::require(&*st.pdp, &p, "<perm>")` first (strings above), then `ServiceAccountRepo`.
  - `disable`/`enable`: after setting status via the repo, also run the SAME session/token revocation the `/users/{id}` status branch runs (reuse that code path — extract a shared helper if it is inline in `update_user`, so the kill switch behaves identically).
  - **Group add guard:** resolve the group; if its membership is IdP-owned (reuse the `groups.rs` OIDC predicate), refuse with a clear message; else add the SA's `user_id` as a member via the existing helper.
  - **Role add guard:** if `role_id` resolves to `platform_admin` (reuse the `roles.rs` constant/lookup), refuse; else insert into `platform_role_assignments` (`principal_type='user'`, `principal_id=user_id`, `scope_type`, `workspace_id?`) via the existing helper.
  - utoipa-annotate each handler (`#[utoipa::path(...)]`, tag `"access"` or the people routes' tag — match them) and register the paths/DTOs in the OpenAPI doc struct the way the people routes are registered.

- [ ] **Step 4: Run tests, verify they pass.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-controlplane --test service_account_principals`
Expected: PASS. Also run the existing `service_account_surfaces` test — it must still pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-controlplane/src/access/service_accounts.rs crates/egret-controlplane/src/access/mod.rs crates/egret-controlplane/src/lib.rs crates/egret-controlplane/tests/service_account_principals.rs
git commit -m "feat(sa): control-plane service-accounts routes with membership guards (SA1)"
```

---

### Task 4: Regenerate OpenAPI + client + digest; full gate

**Files:**
- Modify: the committed OpenAPI spec + generated TS client + any digest/lockfile the repo keeps in sync (find them — SP3 PRs regenerated `openapi`/`client`/`digest`; the command is the repo's documented generator, e.g. a `just`/`cargo xtask`/`npm run gen:api` target).

- [ ] **Step 1: Find and run the generator.**

Identify the repo's OpenAPI→client generation command (grep `justfile`/`package.json`/`xtask` for `openapi`, `gen:api`, `digest`). Run it. Never hand-edit generated files.

- [ ] **Step 2: Confirm the sync/digest CI check is satisfied.**

Run the same check CI runs (the `just`/script target that diffs the generated artifacts). Expected: clean — the new `/service-accounts` paths appear in the spec and client, and the digest matches.

- [ ] **Step 3: Full backend gate.**

Run: `CARGO_TARGET_DIR=target-sa1 cargo fmt --check` · `CARGO_TARGET_DIR=target-sa1 cargo clippy -p egret-auth -p egret-controlplane -p egret-dataapi --all-targets -- -D warnings` · `CARGO_TARGET_DIR=target-sa1 cargo test -p egret-auth -p egret-controlplane -p egret-dataapi`
Expected: all green.

- [ ] **Step 4: Commit.**

```bash
git add <generated spec/client/digest paths>
git commit -m "chore(sa): regenerate OpenAPI + client for service-accounts routes (SA1)"
```

---

## Self-Review

- **Spec coverage:** identity table + backfill → Task 1; `ServiceAccountRepo` (create/list/get/disable/delete, scope) → Task 2; control-plane surface + membership + guards (no platform_admin, no OIDC group) → Task 3; OpenAPI/client sync → Task 4. Out-of-scope items (catalog kill switch, FE tab, live-check, credential minting) are excluded and named in Global Constraints.
- **Placeholder scan:** the only "find/confirm in code" notes are genuine lookups of existing names (FK constraint name, `insert_service_with` email/username contract, the status-revocation helper, group/role helpers, OIDC predicate, `platform_admin` lookup, the generator command) — each with the exact file to read. No vague "add error handling"; every test lists concrete assertions.
- **Type consistency:** `SaScope {Platform, Workspace}`, the `service_accounts` column set, the permission strings (`user:read/create/update/delete`), and the route shapes are identical across Tasks 1–4. The view exposes exactly `(user_id, workspace_id, name, description, created_by, created_at)` — the columns Task 1's audit found Data-API reads/writes.
- **Risk:** the view+trigger is the one subtle piece; Task 1 Step 4 re-runs the existing Data-API SA suite against it as the guardrail, and the FK repoint is verified against a real DB before commit.
