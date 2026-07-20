# Local-User Management Gaps + STS Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill the four remaining local-user admin gaps (password reset, hard delete, immediate lockout) and retire the dead Keycloak refresh-token path left after #304.

**Architecture:** Most local-user CRUD/UI already exists (platform-RBAC #174). `SessionService::revoke_all_for_user` and `CpState.sessions` already exist — Task 1 just wires them in. New: a `set_password` + `hard_delete` on `UserRepo`, a `user:delete` permission, two control-plane routes + `UsersPage` actions, the STS cleanup, and a live disable→lockout e2e.

**Tech Stack:** Rust (axum, sqlx), `egret-controlplane`/`egret-auth`/`egret-core`; React/TS control-plane console; Playwright e2e on kind.

**Spec:** `docs/en/specs/2026-07-21-egret-local-user-mgmt-design.md`

**DB for tests:** `.env` has `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` — `export` it (or `set -a; source .env; set +a`) for `#[sqlx::test]` / DB tests.

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `crates/egret-controlplane/src/access/mod.rs` | user/group/role routes | Wire revoke into disable + last-admin guard (T1); add password-reset route (T2); add delete route + guards (T4) |
| `crates/egret-auth/src/users.rs` | `UserRepo` | Add `set_password` (T2), `hard_delete` (T4) |
| `crates/egret-core/src/rbac.rs` | permission catalog | Add `user:delete` (T4) |
| `crates/egret-auth/src/oidc_routes.rs` | OIDC callback | Remove refresh-token write + `refresh_repo` field (T3) |
| `crates/egret-auth/src/oidc.rs` | OIDC exchange | Drop `offline_access` scope + refresh capture (T3) |
| `crates/egret-auth/src/oidc_refresh.rs` | dead repo | Delete (T3) |
| `crates/egret-bin/src/main.rs` | wiring | Remove `OidcRefreshRepo` construction (T3) |
| `crates/egret-metastore/migrations/00XX_drop_oidc_refresh_tokens.sql` | migration | `DROP TABLE oidc_refresh_tokens` (T3) |
| `frontend/src/lib/api*` + `frontend/src/routes/control-plane/access/UsersPage.tsx` | console | `resetPassword` + `deleteUser` API fns + row actions (T2, T4) |
| `frontend/tests/e2e/local-user-mgmt-kind.spec.ts` + `justfile` | e2e | disable→lockout proof (T5) |

---

## Task 1: Immediate lockout — wire the existing revoke into disable

**Files:**
- Modify: `crates/egret-controlplane/src/access/mod.rs` (`update_user`)
- Test: `crates/egret-controlplane/tests/access_users.rs` (or the file's existing test module — check `crates/egret-controlplane/tests/`)

Context: `SessionService::revoke_all_for_user(user_id) -> Result<()>` already exists (`crates/egret-auth/src/session.rs:124`); `CpState.sessions: SessionService` already exists (`crates/egret-controlplane/src/lib.rs:52`); `Principal { user_id: UserId }` (`crates/egret-core/src/context.rs:11`); `RoleRepo::platform_admin_user_ids()` exists (`crates/egret-auth/src/roles.rs`, from #300).

- [ ] **Step 1: Write the failing test — disabling a user revokes their sessions + last-admin guard**

Add to a DB test file for the access router (mirror an existing `egret-controlplane` DB test; if none exists for users, create `crates/egret-controlplane/tests/access_users.rs` copying the harness of a neighboring `tests/*.rs` — how it builds `CpState`, seeds a user, and calls the handler or the HTTP route):

```rust
#[sqlx::test]
async fn disabling_a_user_revokes_their_sessions(pool: sqlx::PgPool) {
    let st = test_cpstate(pool.clone()).await; // build CpState (copy neighbor test's builder)
    let uid = seed_local_user(&pool, "u@egret.local").await;
    let token = st.sessions.create(uid, "", "").await.unwrap();
    assert!(st.sessions.validate(&token).await.unwrap().is_some());
    // PATCH status=disabled via the handler/route as a platform_admin caller
    update_user_as_admin(&st, uid, /*status*/ Some("disabled")).await.unwrap();
    assert!(st.sessions.validate(&token).await.unwrap().is_none(), "session revoked on disable");
}

#[sqlx::test]
async fn cannot_disable_the_last_platform_admin(pool: sqlx::PgPool) {
    let st = test_cpstate(pool.clone()).await;
    let admin = seed_platform_admin(&pool, "admin@egret.local").await; // the only platform_admin
    let err = update_user_as_admin(&st, admin, Some("disabled")).await.unwrap_err();
    assert!(matches!(err, CpError::BadRequest(_)), "last admin disable rejected");
}
```

(Use the file's real helpers for seeding a user / platform_admin and invoking the handler. If the existing tests call the HTTP route via a test server, follow that; if they call the handler fn directly, follow that.)

- [ ] **Step 2: Run to verify they fail**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret; cargo test -p egret-controlplane disabling_a_user_revokes cannot_disable_the_last`
Expected: FAIL (session still valid after disable; last-admin not guarded).

- [ ] **Step 3: Implement — revoke on disable + last-admin guard**

In `crates/egret-controlplane/src/access/mod.rs` `update_user`, replace the `status == "disabled"` block:

```rust
        if status == "disabled" {
            // Disabling is a stronger action than a plain profile update.
            egret_core::authz::require(&*st.pdp, &p, "user:disable").await?;
            st.users.set_status(user_id, status).await?;
        } else {
            st.users.set_status(user_id, status).await?;
        }
```

with:

```rust
        if status == "disabled" {
            // Disabling is a stronger action than a plain profile update.
            egret_core::authz::require(&*st.pdp, &p, "user:disable").await?;
            // Guard: don't lock the platform out of itself by disabling its last admin.
            guard_not_last_admin(&st, &p, user_id).await?;
            st.users.set_status(user_id, status).await?;
            // Immediate lockout: kill the user's live sessions now (SP1's validate
            // would reject on the next request; this makes it instantaneous).
            st.sessions.revoke_all_for_user(user_id).await?;
        } else {
            st.users.set_status(user_id, status).await?;
        }
```

Add the shared guard helper near the top of the handlers section:

```rust
/// Reject an action that would remove the last active platform admin, or that a
/// caller aims at their own account. Reused by disable + delete.
async fn guard_not_last_admin(
    st: &CpState,
    caller: &egret_core::context::Principal,
    target: UserId,
) -> Result<(), CpError> {
    if caller.user_id == target {
        return Err(CpError::BadRequest(
            "you cannot disable or delete your own account".into(),
        ));
    }
    let admins = st.roles.platform_admin_user_ids().await?;
    if admins.contains(&target) && admins.iter().all(|a| *a == target) {
        return Err(CpError::BadRequest(
            "cannot remove the last platform admin".into(),
        ));
    }
    Ok(())
}
```

(Adjust `caller.user_id`/`Principal` import path to the real one — grep `pub struct Principal`. `platform_admin_user_ids` returns `Vec<UserId>` — confirm the element type and `contains` accordingly.)

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p egret-controlplane disabling_a_user_revokes cannot_disable_the_last`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-controlplane/src/access/mod.rs crates/egret-controlplane/tests/
git commit -m "feat(users): immediate lockout on disable + last-admin guard

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Password reset for local users

**Files:**
- Modify: `crates/egret-auth/src/users.rs` (add `set_password`)
- Modify: `crates/egret-controlplane/src/access/mod.rs` (route + handler)
- Modify: `frontend/src/routes/control-plane/access/UsersPage.tsx` + the console API module
- Test: `crates/egret-auth/tests/*` (set_password) + `crates/egret-controlplane/tests/*` (route)

- [ ] **Step 1: Write the failing test for `UserRepo::set_password`**

In the egret-auth DB tests (mirror `crates/egret-auth/tests/groups_db.rs` / a users test):

```rust
#[tokio::test]
async fn set_password_updates_hash() {
    let Some(db) = connect().await else { return; };
    let users = UserRepo::new(db.clone());
    let uid = insert_local(&users, "u@egret.local", "hash-1").await;
    users.set_password(uid, "hash-2").await.unwrap();
    let u = users.find_by_id(uid).await.unwrap().unwrap();
    assert_eq!(u.password_hash.as_deref(), Some("hash-2"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-auth set_password_updates_hash`
Expected: FAIL — `no method named set_password`.

- [ ] **Step 3: Implement `set_password`**

In `crates/egret-auth/src/users.rs` (mirror `set_status`, users.rs:132):

```rust
    /// Set a local user's password hash (admin reset). Caller hashes the plaintext.
    pub async fn set_password(&self, id: UserId, password_hash: &str) -> Result<()> {
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(id.0)
            .bind(password_hash)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p egret-auth set_password_updates_hash`
Expected: PASS.

- [ ] **Step 5: Write the failing test for the route (local-only + revokes sessions)**

In the egret-controlplane user tests:

```rust
#[sqlx::test]
async fn reset_password_rehashes_and_revokes_sessions(pool: sqlx::PgPool) {
    let st = test_cpstate(pool.clone()).await;
    let uid = seed_local_user(&pool, "u@egret.local").await;
    let token = st.sessions.create(uid, "", "").await.unwrap();
    reset_password_as_admin(&st, uid, "NewPass123!").await.unwrap();
    assert!(st.sessions.validate(&token).await.unwrap().is_none(), "old session revoked");
    let u = st.users.find_by_id(uid).await.unwrap().unwrap();
    assert!(egret_auth::password::verify_password("NewPass123!", u.password_hash.as_deref().unwrap()).unwrap());
}

#[sqlx::test]
async fn reset_password_rejects_sso_user(pool: sqlx::PgPool) {
    let st = test_cpstate(pool.clone()).await;
    let uid = seed_oidc_user(&pool, "sso-sub", "sso@egret.local").await; // provider='oidc'
    let err = reset_password_as_admin(&st, uid, "x").await.unwrap_err();
    assert!(matches!(err, CpError::BadRequest(_)));
}
```

- [ ] **Step 6: Run to verify they fail**

Run: `cargo test -p egret-controlplane reset_password_rehashes reset_password_rejects_sso`
Expected: FAIL (no route/handler).

- [ ] **Step 7: Implement the route + handler**

In `crates/egret-controlplane/src/access/mod.rs`, add the route to the users router (next to `/users/{id}`):

```rust
        .route("/users/{id}/password", post(reset_password))
```

And the handler + input:

```rust
#[derive(serde::Deserialize, utoipa::ToSchema)]
struct ResetPasswordInput {
    password: String,
}

/// `POST /users/{id}/password` — admin reset of a LOCAL user's password. Rehashes,
/// then revokes the user's sessions so the old password's sessions can't linger.
async fn reset_password(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetPasswordInput>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    let user_id = UserId(id);
    let user = st.users.find_by_id(user_id).await?.ok_or(CpError::NotFound)?;
    if user.provider != "local" {
        return Err(CpError::BadRequest(
            "cannot set a password on a non-local (SSO) account".into(),
        ));
    }
    if body.password.is_empty() {
        return Err(CpError::BadRequest("password must not be empty".into()));
    }
    let hash = egret_auth::password::hash_password(&body.password)?;
    st.users.set_password(user_id, &hash).await?;
    st.sessions.revoke_all_for_user(user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

(Confirm `User.provider` is a `String` field — grep the `User` struct in users.rs. Add `reset_password` to the OpenAPI `paths(...)` list if that file enumerates them.)

- [ ] **Step 8: Run to verify they pass**

Run: `cargo test -p egret-controlplane reset_password_rehashes reset_password_rejects_sso`
Expected: PASS.

- [ ] **Step 9: Frontend — API fn + UI action**

In the console API module (grep `createUser` / `updateUser` — likely `frontend/src/lib/controlPlaneApi.ts` or similar), add:

```ts
export async function resetUserPassword(id: string, password: string): Promise<void> {
  const res = await apiFetch(`/api/control-plane/users/${id}/password`, {
    method: "POST",
    body: JSON.stringify({ password }),
  });
  if (!res.ok) throw new Error(`reset password failed: ${res.status}`);
}
```

In `UsersPage.tsx`, add a "Reset password" row action (only for `u.provider === "local"`) opening a dialog with a password input that calls `resetUserPassword(u.id, pw)` and toasts success. Mirror the existing create/edit dialog pattern in that file.

- [ ] **Step 10: Frontend verify + commit**

Run: `cd frontend && npx tsc --noEmit && npm run lint`
Expected: clean.

```bash
git add crates/egret-auth/src/users.rs crates/egret-controlplane/src/access/mod.rs crates/egret-auth/tests/ crates/egret-controlplane/tests/ frontend/src
git commit -m "feat(users): password reset for local users (rehash + revoke sessions) + console UI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: STS cleanup — retire the dead Keycloak refresh path

**Files:**
- Modify: `crates/egret-auth/src/oidc_routes.rs`, `crates/egret-auth/src/oidc.rs`, `crates/egret-bin/src/main.rs`
- Delete: `crates/egret-auth/src/oidc_refresh.rs`
- Create: `crates/egret-metastore/migrations/00XX_drop_oidc_refresh_tokens.sql`

- [ ] **Step 1: Write/adjust the test — OIDC login works without offline_access + writes no refresh row**

Find the OIDC callback test (`crates/egret-auth/tests/oidc_db.rs` or `oidc_keycloak.rs`). Add/adjust:

```rust
#[tokio::test]
async fn oidc_login_creates_session_without_refresh_token() {
    let Some(db) = connect().await else { return; };
    // provision_and_sync / callback with claims that have NO refresh token → succeeds,
    // and no oidc_refresh_tokens row exists for the user.
    let uid = provision_and_sync(&UserRepo::new(db.clone()), &GroupRepo::new(db.clone()),
                                 &claims("sub", "u@egret.local", &[])).await.unwrap();
    let cnt: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_refresh_tokens WHERE user_id = $1")
        .bind(uid.0).fetch_one(&db.pool).await.unwrap_or(0);
    assert_eq!(cnt, 0);
}
```

(This asserts the write path is gone. NOTE: after Step 4 drops the table, this exact query would error — so instead assert at the callback level that login succeeds and returns a session; drop the table-count assertion once the table is gone. Simplest: keep the test asserting the callback returns Ok + a session, and delete any OLD test that asserted a refresh token WAS stored.)

- [ ] **Step 2: Remove the refresh-token write + `refresh_repo`**

In `crates/egret-auth/src/oidc_routes.rs`: delete the block that stores the refresh token (~lines 209-237, the `if let Some(rt) = ... refresh_repo.set(...)`), and remove the `pub refresh_repo: crate::oidc_refresh::OidcRefreshRepo` field (~line 31) from `OidcRoutesState`.

- [ ] **Step 3: Drop `offline_access` + the refresh capture**

In `crates/egret-auth/src/oidc.rs`: remove `offline_access` from the OIDC auth-request scope string, and drop the `refresh_token` field capture from the token-exchange result struct (the `OidcLogin` refresh_token field + its extraction). Keep `openid email profile`.

- [ ] **Step 4: Delete the repo + drop the table**

```bash
git rm crates/egret-auth/src/oidc_refresh.rs
```
Remove `pub mod oidc_refresh;` from `crates/egret-auth/src/lib.rs`. Remove the `OidcRefreshRepo::new(db.clone())` construction at `crates/egret-bin/src/main.rs:~1044` (and the `refresh_repo:` field it fed).

Create `crates/egret-metastore/migrations/00XX_drop_oidc_refresh_tokens.sql` (next sequential number — check the highest existing):
```sql
DROP TABLE IF EXISTS oidc_refresh_tokens;
```
Then `touch crates/egret-metastore/src/lib.rs` so the embedded `sqlx::migrate!` re-embeds (known staleness gotcha).

- [ ] **Step 5: Build + test + verify no dangling refs**

Run: `grep -rn "oidc_refresh\|refresh_repo\|offline_access\|OidcRefreshRepo" crates/` → only migration/comment history, no live refs.
Run: `cargo build --workspace 2>&1 | tail -5` → builds.
Run: `cargo test -p egret-auth oidc` (export DATABASE_URL) → passes.
Run: `cargo clippy -p egret-auth -p egret-bin --all-targets -- -D warnings` → clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore(sts): retire dead Keycloak refresh path (drop oidc_refresh_tokens + offline_access)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Hard-delete user

**Files:**
- Modify: `crates/egret-core/src/rbac.rs` (add `user:delete`)
- Modify: `crates/egret-auth/src/users.rs` (add `hard_delete`)
- Modify: `crates/egret-controlplane/src/access/mod.rs` (route + handler)
- Modify: `frontend/.../UsersPage.tsx` + API module
- Test: egret-auth + egret-controlplane DB tests

- [ ] **Step 1: Add the `user:delete` permission**

In `crates/egret-core/src/rbac.rs`, next to `perm!("user:disable", "Users", "Disable users", Scope::Platform)` (~line 233):

```rust
    perm!("user:delete", "Users", "Delete users", Scope::Platform),
```

- [ ] **Step 2: Write the failing test for `UserRepo::hard_delete`**

```rust
#[tokio::test]
async fn hard_delete_removes_user_and_dependents() {
    let Some(db) = connect().await else { return; };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let uid = insert_local(&users, "u@egret.local", "h").await;
    let gid = groups.create_local("g").await.unwrap();
    groups.add_member(gid, uid).await.unwrap();
    users.hard_delete(uid).await.unwrap();
    assert!(users.find_by_id(uid).await.unwrap().is_none());
    assert!(groups.groups_of_user(uid).await.unwrap().is_empty());
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p egret-auth hard_delete_removes_user`
Expected: FAIL — `no method named hard_delete`.

- [ ] **Step 4: Implement `hard_delete` (one transaction)**

In `crates/egret-auth/src/users.rs`. Note: `oidc_refresh_tokens` is already dropped in Task 3, so do NOT reference it here. Delete dependents then the row:

```rust
    /// Hard-delete a user and all dependent rows in one transaction: sessions,
    /// group memberships, and platform role/permission grants, then the user row.
    /// (Audit rows are intentionally NOT deleted — see the migration note.)
    pub async fn hard_delete(&self, id: UserId) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM user_groups WHERE user_id = $1").bind(id.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM access_grants WHERE principal_type = 'user' AND principal_id = $1").bind(id.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM users WHERE id = $1").bind(id.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
```

IMPLEMENTATION NOTE: before finalizing, run `grep -rn "REFERENCES users" crates/egret-metastore/migrations/` to enumerate every table with a `user_id`/actor FK to `users`. If any table OTHER than the four above has a NOT-NULL FK to `users(id)` without `ON DELETE`, the final `DELETE FROM users` will fail — either add its cleanup to this tx, or (for audit/history tables) add an `ON DELETE SET NULL` migration in Task 3's migration file so history survives a removed user. Do NOT cascade-delete audit history.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p egret-auth hard_delete_removes_user`
Expected: PASS.

- [ ] **Step 6: Write the failing test for the delete route (guards)**

```rust
#[sqlx::test]
async fn delete_user_route_enforces_guards(pool: sqlx::PgPool) {
    let st = test_cpstate(pool.clone()).await;
    let admin = seed_platform_admin(&pool, "admin@egret.local").await;
    let victim = seed_local_user(&pool, "v@egret.local").await;
    // self-delete rejected
    assert!(matches!(delete_user_as(&st, admin, admin).await.unwrap_err(), CpError::BadRequest(_)));
    // last-admin delete rejected (admin is the only platform_admin)
    // (victim is not admin, so deleting victim as admin succeeds)
    delete_user_as(&st, admin, victim).await.unwrap();
    assert!(st.users.find_by_id(victim).await.unwrap().is_none());
}
```

- [ ] **Step 7: Implement the delete route + handler**

Route (next to `/users/{id}`):
```rust
        .route("/users/{id}", patch(update_user).delete(delete_user))
```
Handler (reuses `guard_not_last_admin` from Task 1):
```rust
/// `DELETE /users/{id}` — hard-delete a user. Guards: not yourself, not the last
/// platform admin. Cleans up sessions/memberships/grants + the row in one tx.
async fn delete_user(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:delete").await?;
    let user_id = UserId(id);
    st.users.find_by_id(user_id).await?.ok_or(CpError::NotFound)?;
    guard_not_last_admin(&st, &p, user_id).await?;
    st.users.hard_delete(user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```
(Add `delete_user` to the OpenAPI `paths(...)` list. Confirm the router builder pattern for chaining `.delete()` onto the existing `/users/{id}` route.)

- [ ] **Step 8: Run to verify it passes**

Run: `cargo test -p egret-controlplane delete_user_route_enforces_guards`
Expected: PASS.

- [ ] **Step 9: Frontend — API fn + destructive UI action**

Add to the console API module:
```ts
export async function deleteUser(id: string): Promise<void> {
  const res = await apiFetch(`/api/control-plane/users/${id}`, { method: "DELETE" });
  if (!res.ok) throw new Error(`delete user failed: ${res.status}`);
}
```
In `UsersPage.tsx`, add a destructive "Delete" row action with a confirmation dialog (mirror any existing destructive-confirm pattern in the console) calling `deleteUser(u.id)` then refreshing the list.

- [ ] **Step 10: Verify + commit**

Run: `cargo test -p egret-auth -p egret-controlplane` (export DATABASE_URL) + `cd frontend && npx tsc --noEmit && npm run lint`
Expected: clean.

```bash
git add -A
git commit -m "feat(users): hard-delete user (user:delete perm + self/last-admin guards + tx cleanup) + console UI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Live e2e — disable → immediate lockout

**Files:**
- Create: `frontend/tests/e2e/local-user-mgmt-kind.spec.ts`
- Modify: `justfile`

- [ ] **Step 1: Write the e2e spec**

```ts
// Proves SP2: an admin disabling a local user locks them out immediately (SP1's
// session-disabled check + SP2's active revoke); password-reset invalidates the
// old session; delete removes the account. Opt-in (EGRET_E2E_KIND=1).
import { test, expect, request as pwRequest } from "@playwright/test";
import { loginLocalAdmin, csrf } from "./helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(300_000);

test("disable locks the user out immediately", async ({ browser }) => {
  const adminCtx = await browser.newContext();
  const admin = await adminCtx.newPage();
  await loginLocalAdmin(admin); // admin@egret.local

  // create a local user via the console API
  const email = `lu-${Date.now()}@egret.local`;
  const h = { "X-Egret-CSRF": await csrf(admin), "Content-Type": "application/json" };
  const created = await admin.request.post("/api/control-plane/users", {
    headers: h, data: { email, name: "LU", password: "Passw0rd!" },
  });
  const userId = (await created.json()).id;

  // that user logs in (fresh context) → gets a working session
  const userCtx = await browser.newContext();
  const upage = await userCtx.newPage();
  // local login POST /api/auth/login {email,password}
  const login = await upage.request.post("/api/auth/login", { data: { email, password: "Passw0rd!" } });
  expect(login.ok()).toBeTruthy();
  const me1 = await upage.request.get("/api/auth/me");
  expect(me1.ok()).toBeTruthy();

  // admin disables them
  await admin.request.patch(`/api/control-plane/users/${userId}`, { headers: h, data: { status: "disabled" } });

  // the user's next authenticated request is rejected immediately
  const me2 = await upage.request.get("/api/auth/me");
  expect(me2.status()).toBe(401);
});
```

Reuse `loginLocalAdmin`/`csrf` from `frontend/tests/e2e/helpers.ts` (both exist). Confirm the local-login endpoint path (`/api/auth/login`) + the "who am I" endpoint (`/api/auth/me`) by grepping existing specs.

- [ ] **Step 2: Add the recipe**

In `justfile` (mirror `e2e-kind-sts`):
```make
# Opt-in local-user-mgmt e2e: admin disable → immediate lockout.
e2e-kind-local-user: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test local-user-mgmt-kind
```

- [ ] **Step 3: Verify discoverable (do not run live here — coordinator runs it)**

Run: `cd frontend && npx tsc --noEmit && npx playwright test local-user-mgmt-kind --list`
Expected: tsc clean, test listed.

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/local-user-mgmt-kind.spec.ts justfile
git commit -m "test(users): live e2e — disable locks a local user out immediately

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (before finishing the branch)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` (DATABASE_URL set)
- [ ] `cd frontend && npm run lint && npx tsc --noEmit && npm run build`
- [ ] Live: rebuild backend + redeploy + `just e2e-kind-local-user` green (disable→401)
- [ ] `grep -rn "oidc_refresh\|offline_access" crates/` shows no live refs
