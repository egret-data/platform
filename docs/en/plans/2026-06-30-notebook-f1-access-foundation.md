# Notebook F1 — Access Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Per-user Trino impersonation (opt-in per connection, `X-Trino-User`=stable username) + idempotent provisioning of a per-user `notebook_{username}` namespace + 1-grant catalog token + a `GET /notebook-runtime` manifest endpoint (the F1→D1 bridge).

**Architecture:** F1a threads a stable per-user `username` from the engine job into `TrinoConnector`, which sets `X-Trino-User` only when the connection's new `impersonate` flag is on (basic-auth stays the service principal). F1b adds a provisioning routine + runtime endpoint in `egret-notebook` (gains an `egret-catalog` dep) that ensures the namespace, mints-and-revokes a 1-grant token, and returns the kernel manifest including the workspace's notebook-enabled engines/catalogs (RS-2).

**Tech Stack:** Rust (axum, sqlx, async-trait), Postgres; React + TanStack Query (DatabasesPage toggle).

**Spec:** `docs/en/specs/2026-06-30-notebook-f1-access-foundation-design.md`

**Decisions:** impersonation = per-connection flag, default OFF; `X-Trino-User` = stable `username` (email local-part, collision-suffixed); migrations are **append-only** (lesson from C1).

---

## File structure

**F1a (impersonation):**
- `migrations/0040_user_username.sql` — add `users.username citext UNIQUE` + backfill (new).
- `crates/egret-auth/src/users.rs` — username derivation + `ensure_username(user_id)` lookup-or-derive (modify).
- `crates/egret-database/src/model.rs` — `ConnOptions.impersonate: Option<bool>` (modify).
- `crates/egret-engine/src/connector/mod.rs` — `ConnSpec.session_user` + `ConnectorFactory::build(.., session_user)` (modify).
- `crates/egret-engine/src/connector/trino.rs` — `X-Trino-User` impersonation logic (modify).
- `crates/egret-engine/src/job.rs` — thread `session_user` submit→run→execute (modify).
- `crates/egret-engine/src/routes.rs` — resolve username from principal, pass to submit (modify).
- `frontend/src/api/databases.ts` + `frontend/src/routes/explore/DatabasesPage.tsx` — `impersonate` toggle (modify).

**F1b (per-user catalog access):**
- `crates/egret-notebook/Cargo.toml` — add `egret-catalog` dep (modify).
- `crates/egret-notebook/src/provisioning.rs` — ensure namespace + mint/revoke 1-grant token (new).
- `crates/egret-notebook/src/runtime.rs` — `GET /notebook-runtime` handler + manifest DTO (new).
- `crates/egret-notebook/src/routes.rs` + `lib.rs` — mount the runtime route, extend `NotebookState` (modify).
- `crates/egret-bin/src/main.rs` — wire the catalog handle into `NotebookState` (modify).

---

## Task 1: Stable per-user username (migration + derivation)

**Files:**
- Create: `migrations/0040_user_username.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (re-embed comment), `crates/egret-auth/src/users.rs`

- [ ] **Step 1: Migration** — `migrations/0040_user_username.sql` (append-only; nullable so future OIDC users derive lazily):

```sql
ALTER TABLE users ADD COLUMN username citext UNIQUE;

-- Backfill existing rows: slug of the email local-part ([a-z0-9_-], lowercased),
-- de-duplicated by appending "-N" to collisions (deterministic by created_at).
WITH base AS (
  SELECT id,
         NULLIF(regexp_replace(lower(split_part(email::text,'@',1)), '[^a-z0-9_-]+', '-', 'g'), '') AS slug,
         created_at
  FROM users
), ranked AS (
  SELECT id, COALESCE(slug, 'user') AS slug,
         row_number() OVER (PARTITION BY COALESCE(slug,'user') ORDER BY created_at, id) AS rn
  FROM base
)
UPDATE users u
SET username = CASE WHEN r.rn = 1 THEN r.slug ELSE r.slug || '-' || r.rn END
FROM ranked r WHERE u.id = r.id;
```

- [ ] **Step 2: Re-embed** — edit the header comment in `crates/egret-metastore/src/lib.rs` to mention `0040_user_username`.

- [ ] **Step 3: Write failing test** in `crates/egret-auth/src/users.rs` test module (mirror existing `#[sqlx::test]` there; if none, use `#[sqlx::test(migrations = "../../migrations")]`):

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn ensure_username_derives_and_is_stable(pool: sqlx::PgPool) {
    let repo = UserRepo::new(/* however UserRepo is constructed from a pool — read this file */);
    let a = repo.insert_local("Alice@Corp.com", "Alice", "x").await.unwrap();
    let u1 = repo.ensure_username(a).await.unwrap();
    assert_eq!(u1, "alice");                       // slug of local-part
    let u2 = repo.ensure_username(a).await.unwrap();
    assert_eq!(u2, "alice");                       // stable (no re-derive)
    let b = repo.insert_local("alice@other.com", "Alice2", "x").await.unwrap();
    let ub = repo.ensure_username(b).await.unwrap();
    assert_eq!(ub, "alice-2");                      // collision suffix
}
```

- [ ] **Step 4: Run — fails** `cargo test -p egret-auth ensure_username` → FAIL (no method).

- [ ] **Step 5: Implement** `ensure_username` in `users.rs`. READ the existing `UserRepo` first for the pool field + error type. Logic: if `username` already set for the user, return it; else derive `slug(local-part(email))`, then loop appending `-N` until an INSERT/UPDATE with the unique constraint succeeds (catch unique-violation, bump N). Pseudocode:

```rust
pub async fn ensure_username(&self, user_id: Uuid) -> Result<String> {
    // 1. return existing if set
    if let Some(u) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT username FROM users WHERE id=$1").bind(user_id)
        .fetch_one(&self.pool).await? { return Ok(u); }
    // 2. derive base slug from email local-part
    let email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id=$1")
        .bind(user_id).fetch_one(&self.pool).await?;
    let base = slugify_localpart(&email); // [a-z0-9_-], lowercased, "user" if empty
    // 3. claim base, then base-2, base-3 … until the UNIQUE update sticks
    for n in 1..1000 {
        let cand = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let res = sqlx::query("UPDATE users SET username=$2 WHERE id=$1 AND username IS NULL")
            .bind(user_id).bind(&cand).execute(&self.pool).await;
        match res {
            Ok(r) if r.rows_affected() == 1 => return Ok(cand),
            Ok(_) => { // someone set it concurrently → re-read
                if let Some(u) = sqlx::query_scalar::<_,Option<String>>(
                    "SELECT username FROM users WHERE id=$1").bind(user_id)
                    .fetch_one(&self.pool).await? { return Ok(u); }
            }
            Err(sqlx::Error::Database(d)) if d.is_unique_violation() => continue, // taken
            Err(e) => return Err(e.into()),
        }
    }
    Err(/* domain error: could not allocate username */)
}
```

Add a small `fn slugify_localpart(email: &str) -> String` (lowercase, `regexp`-equiv via char filter to `[a-z0-9_-]`, collapse repeats, fallback `"user"`).

- [ ] **Step 6: Run — passes** `cargo test -p egret-auth ensure_username` → PASS. Recreate the dev DB is NOT needed (append-only migration); just `sqlx migrate run`.

- [ ] **Step 7: Commit**

```bash
git add migrations/0040_user_username.sql crates/egret-metastore/src/lib.rs crates/egret-auth/src/users.rs
git commit -m "feat(auth): stable per-user username (email local-part, collision-suffixed)"
```

---

## Task 2: `impersonate` flag on engine connection (backend model + frontend)

**Files:**
- Modify: `crates/egret-database/src/model.rs` (ConnOptions), `frontend/src/api/databases.ts`, `frontend/src/routes/explore/DatabasesPage.tsx` + its test.

- [ ] **Step 1: Backend** — add to `ConnOptions` (model.rs:33, after `database`):

```rust
    /// Trino only: run each query as the logged-in user (X-Trino-User).
    /// Requires Trino impersonation + Ranger. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impersonate: Option<bool>,
```

`cargo build -p egret-database` → PASS (it's stored in the `options` JSONB; no migration).

- [ ] **Step 2: Frontend types** — in `frontend/src/api/databases.ts`, add `impersonate?: boolean` to the database `options` type used by the form (read the file to find the exact `options` interface).

- [ ] **Step 3: Frontend toggle test** — in `DatabasesPage.test.tsx`, add a test: when engine = trino, an "Run as logged-in user" switch is present; toggling it + submitting includes `options.impersonate: true` in the create/update payload. (Mirror the existing field tests.)

- [ ] **Step 4: Run — fails** `cd frontend && npm run test -- DatabasesPage` → FAIL.

- [ ] **Step 5: Implement** — in `DatabasesPage.tsx` Add/Edit dialog, render a shadcn `Switch` (label "Run queries as the logged-in user", help "Requires Trino impersonation + Ranger") **only when engine === "trino"**, bound to `options.impersonate`; include it in the submit payload. Reuse the Switch pattern from the notebook settings / public-toggle.

- [ ] **Step 6: Run — passes** `npm run test -- DatabasesPage`; `npm run typecheck` → PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-database/src/model.rs frontend/src/api/databases.ts frontend/src/routes/explore/DatabasesPage.tsx frontend/src/routes/explore/__tests__/DatabasesPage.test.tsx
git commit -m "feat(engine): per-connection impersonate flag (Trino, default off)"
```

---

## Task 3: TrinoConnector `X-Trino-User` impersonation (TDD)

**Files:**
- Modify: `crates/egret-engine/src/connector/mod.rs` (ConnSpec + factory), `crates/egret-engine/src/connector/trino.rs`

- [ ] **Step 1: Extend ConnSpec + factory** — in `connector/mod.rs`: add `pub session_user: Option<String>` to `ConnSpec`; `from_database` sets it `None`; add a builder `ConnSpec::from_database_as(db, secret, session_user)` that sets it. Extend the `ConnectorFactory::build` trait signature to `build(&self, db: &Database, secret: Option<String>, session_user: Option<String>) -> Result<Box<dyn Connector>, EngineError>` and update `DefaultConnectorFactory` to call `ConnSpec::from_database_as(db, secret, session_user)`. Update `build_connector` callers.

- [ ] **Step 2: Failing test** — in `connector/trino.rs` test module:

```rust
#[test]
fn xtrino_user_is_session_user_only_when_impersonating() {
    let mut opts = ConnOptions::default();
    opts.user = Some("svc".into());
    // impersonate off → X-Trino-User = service user
    let s0 = ConnSpec { engine: Engine::Trino, host: "h".into(), port: 8080, options: opts.clone(), secret: None, session_user: Some("alice".into()) };
    assert_eq!(TrinoConnector::new(&s0).x_trino_user(), "svc");
    // impersonate on + session_user → X-Trino-User = alice; basic-auth stays svc
    opts.impersonate = Some(true);
    let s1 = ConnSpec { engine: Engine::Trino, host: "h".into(), port: 8080, options: opts, secret: None, session_user: Some("alice".into()) };
    let c = TrinoConnector::new(&s1);
    assert_eq!(c.x_trino_user(), "alice");
    assert_eq!(c.auth_user(), "svc");
}
```

- [ ] **Step 2b: Run — fails** `cargo test -p egret-engine xtrino_user` → FAIL.

- [ ] **Step 3: Implement** — in `TrinoConnector`: store `auth_user` (= `spec.options.user` or "egret") and `session_user` (= if `spec.options.impersonate == Some(true)` then `spec.session_user.clone().unwrap_or(auth_user)` else `auth_user`). Add small accessors `x_trino_user(&self) -> &str` (returns session_user) and `auth_user(&self) -> &str`. Replace every `.header("X-Trino-User", &self.user)` with `self.x_trino_user()`, and every `basic_auth(&self.user, ...)` with `self.auth_user()`. (There are 4 X-Trino-User sites + 2 basic_auth sites — see trino.rs:74,78,122,132,141,143.)

- [ ] **Step 4: Run — passes** `cargo test -p egret-engine` → PASS; `cargo clippy -p egret-engine --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine/src/connector
git commit -m "feat(engine): TrinoConnector sends X-Trino-User=session user when impersonating"
```

---

## Task 4: Thread `session_user` from the job + routes (TDD)

**Files:**
- Modify: `crates/egret-engine/src/job.rs` (submit/run/execute), `crates/egret-engine/src/routes.rs` (resolve username), `crates/egret-engine/src/lib.rs`/state if a UserRepo handle is needed.

- [ ] **Step 1: Thread through JobManager** — READ job.rs. `submit(ws, database_id, user: Option<Uuid>, sql, limit)` → add a `session_user: Option<String>` param carried into the spawned `run` → `execute`, where `self.factory.build(&db, secret, session_user)` is called (currently `build(&db, secret)` at job.rs:126). Keep the stored query unchanged.

- [ ] **Step 2: Resolve username in the submit route** — READ routes.rs `submit`. It has `WsContext(ctx)`; `ctx.principal.user_id`. Before calling `jobs.submit(...)`, resolve the username: `let session_user = Some(state.users.ensure_username(ctx.principal.user_id.0).await?)` (add a `UserRepo`/`users` handle to `EngineState` if not present — mirror how other repos are held; construct in egret-bin). Pass `session_user` to `jobs.submit`.

- [ ] **Step 3: Test (fake factory captures session_user)** — in egret-engine's job tests (read the existing fake factory used by JobManager tests), assert that submitting with a resolved username causes `factory.build` to receive `session_user = Some("alice")`. If the existing test factory ignores the new arg, extend it to record the last `session_user` and assert it. (No real Trino needed — this proves the threading.)

- [ ] **Step 4: Run — passes** `cargo test -p egret-engine` → PASS; `cargo build -p egret-bin` (EngineState change compiles) → PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine crates/egret-bin
git commit -m "feat(engine): thread end-user identity into the connector for impersonation"
```

---

## Task 5: Provisioning — namespace + 1-grant token (TDD)

**Files:**
- Modify: `crates/egret-notebook/Cargo.toml` (add `egret-catalog.workspace = true`)
- Create: `crates/egret-notebook/src/provisioning.rs`; Modify: `crates/egret-notebook/src/lib.rs`

- [ ] **Step 1: Add dep** — `egret-catalog.workspace = true` to egret-notebook `[dependencies]`.

- [ ] **Step 2: Failing test** — `crates/egret-notebook/src/provisioning.rs` test module (`#[sqlx::test(migrations = "../../migrations")]`). Seed a workspace + user + a `workspace_warehouses` row (reuse the `seed_warehouse_http` helper pattern from tests/http.rs). Assert:

```rust
// provision twice → idempotent; the second mint returns a DIFFERENT raw token,
// and the previous token is revoked (only one active per user×ws); the grant is
// exactly { namespace: "notebook_<username>", table: None, perms: [read,write] }.
let p1 = provision_notebook_access(&store, ws, user, warehouse_id, "alice").await.unwrap();
let p2 = provision_notebook_access(&store, ws, user, warehouse_id, "alice").await.unwrap();
assert_eq!(p1.namespace, "notebook_alice");
assert_ne!(p1.token, p2.token);
// p1.token no longer authenticates; p2.token does, scoped to notebook_alice rw
```

- [ ] **Step 3: Run — fails** → FAIL (fn missing).

- [ ] **Step 4: Implement** `provision_notebook_access(store, ws, user_id, warehouse_id, username) -> Result<Provisioned>` where `Provisioned { namespace: String, token: String }`:
  1. `let ns = format!("notebook_{username}");`
  2. Ensure the namespace exists — call the catalog engine's namespace-create. READ `crates/egret-catalog-engine/src/browse.rs:883` (`create_namespace_route`) + the `NamespaceService::create_namespace` it delegates to; pick the cleanest in-process call (a direct service call, treating "already exists" as success). If a direct call entangles crates badly, fall back to an internal HTTP POST to `/api/ws/{ws}/catalog/warehouses/{wid}/namespaces` with an admin/static token — note the choice.
  3. Revoke prior tokens for this (user×ws) notebook namespace, then mint a fresh one: use `CatalogStore::revoke_token_for_workspace` to drop the previous (track it by a deterministic `label = format!("notebook-runtime:{user_id}")`, revoking any existing token with that label for the ws), then `CatalogStore::create_token(ws, warehouse_id, &label, &[Grant { namespace: ns.clone(), table: None, permissions: vec!["catalog:read".into(), "catalog:write".into()] }])`. Return `(ns, raw)`.

  > READ `crates/egret-catalog/src/service/tokens.rs` for the exact `Grant` struct fields (namespace/table/permissions), `create_token` signature (confirmed: `(workspace_id, warehouse_id, label, scope: &[Grant]) -> (CatalogToken, String)`), and the revoke-by-workspace fn + whether it can target by label (if not, add a small `revoke_tokens_by_label(ws, label)` helper).

- [ ] **Step 5: Run — passes** `cargo test -p egret-notebook provision` → PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-notebook/Cargo.toml crates/egret-notebook/src/provisioning.rs crates/egret-notebook/src/lib.rs
git commit -m "feat(notebook): provision per-user notebook namespace + 1-grant token (idempotent mint+revoke)"
```

---

## Task 6: Runtime endpoint `GET /notebook-runtime` (TDD)

**Files:**
- Create: `crates/egret-notebook/src/runtime.rs`; Modify: `crates/egret-notebook/src/routes.rs` (mount), `lib.rs`, `NotebookState` (catalog handles + warehouse lookup).

- [ ] **Step 1: Manifest DTO** in runtime.rs:

```rust
#[derive(serde::Serialize)]
pub struct NotebookRuntime {
    pub username: String,
    pub catalog: RuntimeCatalog,
    pub engines: Vec<crate::model::EngineOption>,   // reuse settings DTOs
    pub catalogs: Vec<crate::model::CatalogOption>,
}
#[derive(serde::Serialize)]
pub struct RuntimeCatalog { pub url: String, pub warehouse: String, pub namespace: String, pub token: String }
```

- [ ] **Step 2: Failing http test** — in tests/http.rs (`#[sqlx::test]`, authenticated like the others): seed ws + user (member, notebook:read) + a warehouse + enabled engines via the settings PUT. `GET /ws/{ws}/notebook-runtime` → 200; assert `username` present, `catalog.namespace == "notebook_<username>"`, `catalog.token` non-empty, and `engines` contains only the workspace's notebook-enabled engines (from RS-2). A non-member → 403/404.

- [ ] **Step 3: Run — fails** → FAIL (route missing).

- [ ] **Step 4: Implement** the handler `runtime(State, WsContext)`:
  1. `require_workspace(ctx, "notebook:read")`.
  2. `let username = state.users.ensure_username(ctx.principal.user_id.0).await?;`
  3. Resolve the workspace's warehouse id + catalog REST base/prefix (READ how the catalog browser / warehouse list resolves the workspace warehouse — `workspace_warehouses` row → warehouse_id + prefix; reuse that).
  4. `let prov = provision_notebook_access(&state.store?, ws, user, warehouse_id, &username).await?;` (the store/catalog handle needed — add to NotebookState).
  5. Load enabled engines/catalogs via the settings store (reuse `list_engine_options`/`list_catalog_options`, filter `enabled`).
  6. Return `Json(NotebookRuntime { username, catalog: RuntimeCatalog { url, warehouse, namespace: prov.namespace, token: prov.token }, engines, catalogs })`.

  > Mount in `notebook_router`: `.route("/ws/{workspace}/notebook-runtime", get(runtime))`. Extend `NotebookState` with whatever catalog handle `provision_notebook_access` needs (a `CatalogStore` + the namespace-create entrypoint); construct it in egret-bin (Task 7).

- [ ] **Step 5: Run — passes** `cargo test -p egret-notebook` → PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-notebook/src/runtime.rs crates/egret-notebook/src/routes.rs crates/egret-notebook/src/lib.rs
git commit -m "feat(notebook): GET /notebook-runtime manifest endpoint (F1→D1 bridge)"
```

---

## Task 7: Wire NotebookState (catalog handles) in egret-bin + build

**Files:**
- Modify: `crates/egret-notebook/src/routes.rs` (NotebookState fields + FromRef), `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Extend NotebookState** with the handles the runtime needs (a `CatalogStore`, the `UserRepo`/`users`, and whatever the namespace-create call requires). Add `FromRef` impls if any handle is also used as an extractor.

- [ ] **Step 2: Construct in egret-bin** — READ `crates/egret-bin/src/main.rs` where `NotebookState` is built (~line 503) and where the catalog engine state + UserRepo already exist; pass the same handles in. `cargo build -p egret-bin` → PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-notebook/src/routes.rs crates/egret-bin/src/main.rs
git commit -m "feat(notebook): wire catalog + user handles into NotebookState for runtime"
```

---

## Task 8: Full local CI + manual Trino integration test + smoke

- [ ] **Step 1: Backend** — `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p egret-auth -p egret-engine -p egret-notebook` (and `cargo test --workspace --no-fail-fast` to confirm only the known-pre-existing failures remain). All green.

- [ ] **Step 2: Frontend** — `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → all green.

- [ ] **Step 3: Manual real-Trino integration test** (memory `egret-manual-trino-integration-test`): with the dev stack up, enable `impersonate` on the dev Trino connection; submit a query in SQL Lab as two different users; verify Trino received `X-Trino-User=<username>` and Ranger enforced per-user (one allowed, one denied on a governed table). Document the result. Then `GET /api/ws/{ws}/notebook-runtime` (authenticated) and confirm the manifest (username, `notebook_<username>` namespace, a usable token scoped to that namespace, enabled engines/catalogs).

- [ ] **Step 4: Live smoke** — DatabasesPage shows the impersonate toggle (Trino only); toggling persists.

- [ ] **Step 5:** Push branch + open PR (or hand to finishing-a-development-branch). Admin-squash-merge after green (GitHub Actions out of quota).

---

## Self-review notes (author)
- Spec coverage: F1a username (T1), impersonate flag (T2), connector X-Trino-User (T3), job/route threading (T4); F1b provisioning mint+revoke (T5), runtime manifest reading RS-2 settings (T6), wiring (T7), CI+manual-Trino+smoke (T8). ✔
- Type consistency: `ensure_username(user_id)->String`, `ConnSpec.session_user: Option<String>`, `ConnectorFactory::build(db, secret, session_user)`, `provision_notebook_access(..,username)->Provisioned{namespace,token}`, `NotebookRuntime{username,catalog{url,warehouse,namespace,token},engines,catalogs}` align across tasks. ✔
- Verify-points (read-first) called out where egret signatures must be confirmed: UserRepo shape, namespace-create call path, revoke-by-label, EngineState/NotebookState construction, DatabasesPage options interface, warehouse/prefix resolution. ✔
- Migrations append-only (0040 only adds a nullable column) — no DB recreate, no dev-data loss (the C1 lesson). ✔
