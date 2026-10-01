# Data Apps P1 — query gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `POST /api/apps/query`: a data-app kernel runs read-only SQL on a workspace database under the *viewer's* identity. It authenticates with an opaque per-session bearer token, and re-authorizes on every call.

**Architecture:** A new crate, `egret-apps`, owns everything:
- the session-token repository (`analytics.app_sessions`, hashed tokens);
- a pure read-only SQL checker built on `sqlparser`;
- the gateway handler.

The handler resolves the token, re-checks the user, the workspace feature, the app and the permissions, resolves the database **by name**, and rejects anything that is not read-only. It then calls the existing `QueryExecPort::execute_as` as the token's user, audits the call, and rate-limits per session. The route is mounted outside CSRF, like the kernel callback router.

P1 also creates what the gateway authorizes against:
- the `analytics.apps` identity table;
- the `app:read`, `app:write` and `app:manage` permissions;
- the `apps` workspace feature.

Drafts, versions and the app API are P2.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9 runtime queries, async-trait), Postgres migrations, Cedar PDP via `egret_core::authz`, `sqlparser` (new).

**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md` §4 (gateway, tokens, read-only), §5 (tables), §8 (permissions), D3/D4/D6.

## Changes made during implementation (2026-10-01)

- `app:manage` was **deferred to P2**: the `catalog_drift` test refuses a declared permission that nothing enforces. Wherever this plan says `app:manage`, read it as P2.
- The read-only gate uses **stock** `GenericDialect`/`MySqlDialect`, plus lexer rules. The adversarial review found four comment-lexing bypasses and closed them: nested `/* */`, `/*!` hints, `\r` ending `--`, and `--`/`-->` adjacency. `derive_dialect!` wrappers were rejected because they read the cargo registry at compile time.
- The gateway also refuses **non-active workspaces**, with the same 404 as a missing app. It audits rate-limited and invalid-body calls, and accepts the bearer scheme case-insensitively.
- The schema enforces that a session's workspace equals its app's workspace, with a composite FK.
- The rate limiter's lockout is one window, so a 429 holds the token for 2 s.

## Global Constraints

- **Branch:** `feat/data-apps-p1-gateway`, off `origin/main`, in its own worktree. Do not commit on `main`.
- **Git identity:** the repo's configured identity. Never override the author. Commit messages follow Conventional Commits and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **No transactions or macros required:**
  - sqlx is 0.9 and egret uses runtime `sqlx::query(...)` only, never `query!`.
  - Non-literal SQL needs `sqlx::AssertSqlSafe(...)`.
  - `SQLX_OFFLINE=true` is set, and no `cargo sqlx prepare` is needed.
- **Next migration numbers:**
  - `0161` (tables) and `0162` (permission seeds).
  - If `origin/main` has moved, use the next free numbers.
  - Migrations are forward-only.
- **Security rules. They are acceptance criteria, not style:**
  - A token is stored **only** as `egret_core::token::hash_token` (SHA-256 hex). The plaintext is returned once, from `mint`, and never logged.
  - Every gateway call re-evaluates everything. A token is identity, not permission.
  - Anything the read-only checker cannot parse is **rejected** (fail closed).
  - Unknown statement kinds, table functions, locking clauses, `SELECT … INTO` and nested statements are rejected.
  - The error for a database that does not exist is identical to the error for one in another workspace (404).
- **Permissions (spec §4.2 as amended):**

  | Mode | Required |
  |---|---|
  | run | `app:read` on the app |
  | edit | `app:write` on the app **and** `query:run` on the workspace |

- **Session lifetimes:**
  - Idle expiry is 30 min, sliding on use.
  - Absolute expiry is 8 h.
  - A session is revoked when its egret login session is deleted, through the FK cascade.
- **Limits:**
  - `max_rows` is `min(requested, 10000)`.
  - The timeout is 60 s.
  - The per-session rate limit is 20 requests per 2 s window.
  - All four values are configurable through `EGRET_APPS__*`.
- **Local CI before any PR.** GitHub CI is out of quota, so run all of these locally:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`, with a real `DATABASE_URL`/test DB. DB-gated tests silently skip without one, so confirm they ran.
  - `openapi.json` must be current.
- **Test-suite hygiene:**
  - Never run two workspace test suites at once.
  - Commit before long suites.
  - Use a per-worktree `CARGO_TARGET_DIR`, because a target shared with another checkout causes phantom E0432 errors.
- **Illustrative code is a guess.** The code below is written against signatures read from `origin/main` (ecdf4868). Where the real signature differs (for example `Db`'s pool accessor, `perm!` argument order, or `sqlparser` AST variant names in the pinned version), follow the real code. Record every deviation in the task report.

---

## File structure

| File | Responsibility |
|---|---|
| `migrations/0161_data_apps_gateway.sql` | `analytics.apps` (identity), `analytics.app_sessions` |
| `migrations/0162_data_apps_permissions.sql` | Seed `app:*` onto the workspace roles |
| `crates/egret-core/src/rbac.rs` (modify) | Declare `app:read`, `app:write`, `app:manage` |
| `crates/egret-workspace/src/features.rs` (modify) | The `apps` feature module, plus the hand-gated route list in the module doc |
| `crates/egret-config/src/lib.rs` (modify) | `AppsConfig` (`EGRET_APPS__*`) |
| `crates/egret-apps/Cargo.toml` | New crate |
| `crates/egret-apps/src/lib.rs` | Public surface: `AppsState`, `gateway_router()` |
| `crates/egret-apps/src/session.rs` | `AppMode`, `NewAppSession`, `AppSession`, `AppSessionRepo` (mint, resolve-and-touch, revoke) |
| `crates/egret-apps/src/readonly.rs` | `EngineDialect`, `check_read_only(sql, dialect)` |
| `crates/egret-apps/src/gateway.rs` | `QueryRequest`, the `query` handler |
| `crates/egret-apps/tests/session_repo.rs` | DB tests for the repository |
| `crates/egret-apps/tests/gateway.rs` | HTTP tests: real DB, real Cedar PDP, fake `QueryExecPort` |
| `crates/egret-server/src/lib.rs` (modify) | Nest `gateway_router` under `/api`, without `csrf_layer` |
| `crates/egret-bin/src/main.rs` (modify) | Build `AppsState` and share the `EngineState` Arc and audit sink |
| `Cargo.toml` (modify) | Workspace members and `sqlparser` in `[workspace.dependencies]` |

---

### Task 1: Tables, permissions, feature module

**Files:**
- Create: `migrations/0161_data_apps_gateway.sql`, `migrations/0162_data_apps_permissions.sql`
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-workspace/src/features.rs`
- Test: `crates/egret-workspace/tests/apps_feature.rs` (new), plus whichever existing rbac catalog test covers `PERMISSIONS`

**Interfaces:**
- Produces:
  - `analytics.apps(id, workspace_id, name, description, folder_id, owner_user_id, created_at, updated_at, deleted_at)`
  - `analytics.app_sessions(...)`, with the columns below
  - permission keys `"app:read"`, `"app:write"`, `"app:manage"`
  - feature key `"apps"`, gating the path prefix `apps`

- [ ] **Step 1: Write `migrations/0161_data_apps_gateway.sql`**

```sql
-- Data Apps P1 (docs/en/specs/2026-09-30-marimo-data-apps-design.md §4, §5).
-- `apps` is the identity row the gateway authorizes against; drafts, versions and
-- the app API arrive in P2.
CREATE TABLE analytics.apps (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    description   text NOT NULL DEFAULT '',
    -- P2 wires folders; until then every app is Uncategorized.
    folder_id     uuid NULL,
    owner_user_id uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz NULL
);
CREATE INDEX apps_workspace_id_idx ON analytics.apps (workspace_id) WHERE deleted_at IS NULL;

-- One row per marimo session the apps proxy opens (spec §4.1). Only the SHA-256
-- of the token is stored. `login_sid` cascades from egret's own login session, so
-- logout — including OIDC back-channel logout, which deletes that row — revokes
-- every app token minted under it without any code having to remember to.
CREATE TABLE analytics.app_sessions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash          text NOT NULL UNIQUE,
    user_id             uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id        uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id              uuid NOT NULL REFERENCES analytics.apps(id) ON DELETE CASCADE,
    -- NULL = the draft (edit mode). The FK to analytics.app_versions is added in P2.
    version_id          uuid NULL,
    mode                text NOT NULL CHECK (mode IN ('edit', 'run')),
    login_sid           uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    created_at          timestamptz NOT NULL DEFAULT now(),
    last_used_at        timestamptz NOT NULL DEFAULT now(),
    idle_expires_at     timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    revoked_at          timestamptz NULL,
    -- edit always runs the draft; run always runs a pinned version
    CHECK ((mode = 'edit') = (version_id IS NULL)),
    CHECK (idle_expires_at <= absolute_expires_at)
);
CREATE INDEX app_sessions_login_sid_idx ON analytics.app_sessions (login_sid);
CREATE INDEX app_sessions_app_id_idx ON analytics.app_sessions (app_id);
```

- [ ] **Step 2: Write `migrations/0162_data_apps_permissions.sql`**

Model it on `migrations/0160_workflow_use_secrets_permission.sql`:

```sql
-- Data Apps permissions (spec §8). Viewers open published apps; editors author;
-- admins also manage shares.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:read' FROM roles WHERE scope = 'workspace' AND name IN ('ws_viewer', 'ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:write' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:manage' FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
```

Before writing this, check that the role names `ws_viewer`, `ws_editor` and `ws_admin` exist on `origin/main`, with `git grep -n "ws_viewer" migrations | head`.

- [ ] **Step 3: Declare the permissions in `crates/egret-core/src/rbac.rs`**

Next to the `notebook:*` entries (~:407), in the same `perm!` form as those entries:

```rust
perm!("app:read", "Apps", "Open published data apps", Scope::Workspace),
perm!("app:write", "Apps", "Author, publish and restore data apps", Scope::Workspace),
perm!("app:manage", "Apps", "Manage data-app shares", Scope::Workspace),
```

Copy the exact macro shape from the neighbouring lines. If a test pins the catalog size or lists every key, update it.

- [ ] **Step 4: Add the `apps` feature module in `crates/egret-workspace/src/features.rs`**

- Add `"apps"` to `FEATURE_KEYS` (:51).
- Add a `Feature` to `FEATURES` (:197), modelled on the notebooks entry (:235-246): `key: "apps"`, `label: "Data Apps"`, `description: "Custom Python dashboards built with marimo, run with each viewer's permissions."`, `rules: &[prefix("apps")]`, and `default_enabled` equal to the notebooks entry's value.
- In the module doc (:20-34), list the hand-gated route: `POST /api/apps/query — no /ws/{ws} segment; egret-apps checks disabled_features itself`.

- [ ] **Step 5: Write the failing feature test**

`crates/egret-workspace/tests/apps_feature.rs`:

```rust
use egret_workspace::features::{gated_feature, FEATURES, FEATURE_KEYS};

#[test]
fn apps_is_a_feature_and_gates_its_prefix() {
    assert!(FEATURE_KEYS.contains(&"apps"));
    assert!(FEATURES.iter().any(|f| f.key == "apps"));
    assert_eq!(gated_feature("/api/ws/sales/apps/123"), Some("apps"));
}
```

Check `gated_feature`'s real signature and visibility (:424). If it is private or takes the suffix rather than the full path, test it through its existing test module at `features.rs:558`, in the same way as the `notebooks` assertion there.

- [ ] **Step 6: Run the tests to see them fail, then pass**

Run: `cargo test -p egret-workspace apps_feature` and `cargo test -p egret-core rbac`.
Expected: the tests fail before Steps 3–4 and pass after them.

- [ ] **Step 7: Verify the migrations apply, and the cascade**

Add to `crates/egret-apps/tests/session_repo.rs` in Task 2. The crate does not exist yet. For now, run `cargo test -p egret-metastore` or the repo's usual migration test to confirm 0161/0162 apply cleanly on a fresh `egret_test`.

- [ ] **Step 8: Commit**

```bash
git add migrations/0161_data_apps_gateway.sql migrations/0162_data_apps_permissions.sql crates/egret-core/src/rbac.rs crates/egret-workspace
git commit -m "feat(apps): apps identity table, app sessions, app:* permissions, apps feature"
```

---

### Task 2: `egret-apps` crate and the session-token repository

**Files:**
- Create: `crates/egret-apps/Cargo.toml`, `crates/egret-apps/src/lib.rs`, `crates/egret-apps/src/session.rs`
- Modify: root `Cargo.toml` (members and workspace dependency entry)
- Test: `crates/egret-apps/tests/session_repo.rs`

**Interfaces:**
- Consumes: Task 1's tables.
- Produces:

```rust
pub enum AppMode { Edit, Run }                       // as_str(): "edit" | "run"
pub struct NewAppSession {
    pub user_id: egret_core::UserId,
    pub workspace_id: egret_core::WorkspaceId,
    pub app_id: uuid::Uuid,
    pub version_id: Option<uuid::Uuid>,              // None ⇔ Edit
    pub mode: AppMode,
    pub login_sid: uuid::Uuid,
}
pub struct AppSession {
    pub id: uuid::Uuid,
    pub user_id: egret_core::UserId,
    pub workspace_id: egret_core::WorkspaceId,
    pub app_id: uuid::Uuid,
    pub version_id: Option<uuid::Uuid>,
    pub mode: AppMode,
}
pub struct AppSessionRepo { /* Db */ }
impl AppSessionRepo {
    pub fn new(db: egret_metastore::Db) -> Self;
    /// Returns the plaintext token exactly once.
    pub async fn mint(&self, new: &NewAppSession, idle: Duration, absolute: Duration) -> egret_core::Result<String>;
    /// Looks up by hash and, if live, slides the idle expiry in the same statement.
    pub async fn resolve(&self, token: &str, idle: Duration) -> egret_core::Result<Option<AppSession>>;
    pub async fn revoke(&self, token: &str) -> egret_core::Result<()>;
}
pub const TOKEN_PREFIX: &str = "ega_";
```

- [ ] **Step 1: Create the crate**

In `crates/egret-apps/Cargo.toml`, mirror `crates/egret-dataapi/Cargo.toml` and keep only what is needed:

```toml
[package]
name = "egret-apps"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core = { workspace = true, features = ["http", "token"] }
egret-metastore.workspace = true
egret-workspace.workspace = true
egret-query-api.workspace = true
egret-audit.workspace = true
egret-config.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
anyhow.workspace = true
tracing.workspace = true
tokio.workspace = true
async-trait.workspace = true
sha2.workspace = true
hex.workspace = true
sqlparser.workspace = true

[dev-dependencies]
egret-test-db.workspace = true
egret-auth.workspace = true
tower.workspace = true
http-body-util.workspace = true
```

Then, in the root `Cargo.toml`:
- Add `crates/egret-apps` to `members`, and `egret-apps = { path = "crates/egret-apps" }` to `[workspace.dependencies]`, following how the others are declared.
- Add `sqlparser` with a pinned version and a justification comment, in the house style. Run `cargo search sqlparser --limit 1`, pin that release, and enable the `visitor` feature:

```toml
# SQL parser for the Data Apps read-only gate (egret-apps). Pinned: the gate matches
# on AST variants, so an upgrade must re-run egret-apps' readonly tests.
sqlparser = { version = "=<latest>", features = ["visitor"] }
```

Add the dev-dependencies (`tower`, `http-body-util`, `egret-test-db`) the same way `egret-dataapi` does.

- [ ] **Step 2: Write the failing repository tests**

`crates/egret-apps/tests/session_repo.rs`. Use `egret_test_db::url().await` plus `Db::connect(&url, 5)`, as `crates/egret-dataapi/tests/routes_authz.rs` does. The fixture inserts a user, a workspace, an egret login session (`sessions` row with a random `token_hash`) and an `analytics.apps` row. Copy the user and workspace inserts from `routes_authz.rs:99-138`.

```rust
use std::time::Duration;
use egret_apps::session::{AppMode, AppSessionRepo, NewAppSession};

const IDLE: Duration = Duration::from_secs(1800);
const ABS: Duration = Duration::from_secs(8 * 3600);

#[tokio::test]
async fn mint_then_resolve_returns_the_session_and_stores_only_the_hash() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let token = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    assert!(token.starts_with("ega_"));
    let s = repo.resolve(&token, IDLE).await.unwrap().expect("live");
    assert_eq!(s.user_id, fx.user);
    assert_eq!(s.app_id, fx.app);
    assert!(matches!(s.mode, AppMode::Run));
    let stored: String = fx.scalar("SELECT token_hash FROM analytics.app_sessions").await;
    assert_ne!(stored, token);
    assert_eq!(stored, egret_core::token::hash_token(&token));
}

#[tokio::test]
async fn unknown_revoked_idle_and_absolute_expired_tokens_do_not_resolve() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    assert!(repo.resolve("ega_nope", IDLE).await.unwrap().is_none());

    let revoked = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    repo.revoke(&revoked).await.unwrap();
    assert!(repo.resolve(&revoked, IDLE).await.unwrap().is_none());

    let idle = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    fx.exec_with_hash("UPDATE analytics.app_sessions SET idle_expires_at = now() - interval '1 second' WHERE token_hash = $1", &idle).await;
    assert!(repo.resolve(&idle, IDLE).await.unwrap().is_none());

    let abs = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    fx.exec_with_hash("UPDATE analytics.app_sessions SET idle_expires_at = now() - interval '2 seconds', absolute_expires_at = now() - interval '1 second' WHERE token_hash = $1", &abs).await;
    assert!(repo.resolve(&abs, IDLE).await.unwrap().is_none());
}

#[tokio::test]
async fn resolve_slides_idle_expiry_but_never_past_absolute() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let token = repo.mint(&fx.new_run(), Duration::from_secs(60), Duration::from_secs(90)).await.unwrap();
    let before: chrono::DateTime<chrono::Utc> = fx.scalar("SELECT idle_expires_at FROM analytics.app_sessions").await;
    tokio::time::sleep(Duration::from_millis(1100)).await; // a gap, so equal clocks cannot pass
    repo.resolve(&token, Duration::from_secs(3600)).await.unwrap().expect("live");
    let (idle, abs): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        fx.row2("SELECT idle_expires_at, absolute_expires_at FROM analytics.app_sessions").await;
    assert!(idle > before);
    assert_eq!(idle, abs); // clamped to absolute
}

#[tokio::test]
async fn deleting_the_login_session_revokes_by_cascade() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let token = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    fx.exec("DELETE FROM sessions WHERE id = $1", fx.login_sid).await;
    assert!(repo.resolve(&token, IDLE).await.unwrap().is_none());
}

#[tokio::test]
async fn edit_mode_with_a_version_is_rejected_by_the_schema() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let mut bad = fx.new_run();
    bad.mode = AppMode::Edit; // version_id is Some
    assert!(repo.mint(&bad, IDLE, ABS).await.is_err());
}
```

`Fixture` is a small helper in the same file:
- `db`, `user`, `ws`, `app`, `login_sid`;
- `new_run()`, which returns `NewAppSession { mode: Run, version_id: Some(Uuid::new_v4()), .. }`;
- `scalar`, `row2`, `exec`, `exec_with_hash`, thin wrappers over `sqlx::query`.

Each test uses fresh random ids, so tests do not collide.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p egret-apps --test session_repo`
Expected: a compile failure, because `egret_apps::session` does not exist yet.

- [ ] **Step 4: Implement `crates/egret-apps/src/session.rs`**

```rust
//! Per-marimo-session bearer tokens for the query gateway (spec §4.1).

use std::time::Duration;

use egret_core::{token, UserId, WorkspaceId};
use egret_metastore::Db;
use sqlx::Row;
use uuid::Uuid;

pub const TOKEN_PREFIX: &str = "ega_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode { Edit, Run }

impl AppMode {
    pub fn as_str(self) -> &'static str { match self { Self::Edit => "edit", Self::Run => "run" } }
    fn parse(s: &str) -> egret_core::Result<Self> {
        match s { "edit" => Ok(Self::Edit), "run" => Ok(Self::Run),
                  other => Err(anyhow::anyhow!("unknown app session mode {other}").into()) }
    }
}

#[derive(Debug, Clone)]
pub struct NewAppSession {
    pub user_id: UserId, pub workspace_id: WorkspaceId, pub app_id: Uuid,
    pub version_id: Option<Uuid>, pub mode: AppMode, pub login_sid: Uuid,
}

#[derive(Debug, Clone)]
pub struct AppSession {
    pub id: Uuid, pub user_id: UserId, pub workspace_id: WorkspaceId, pub app_id: Uuid,
    pub version_id: Option<Uuid>, pub mode: AppMode,
}

#[derive(Clone)]
pub struct AppSessionRepo { db: Db }

impl AppSessionRepo {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn mint(&self, new: &NewAppSession, idle: Duration, absolute: Duration) -> egret_core::Result<String> {
        let plaintext = format!("{TOKEN_PREFIX}{}", token::random_token_b64());
        sqlx::query(
            "INSERT INTO analytics.app_sessions
               (token_hash, user_id, workspace_id, app_id, version_id, mode, login_sid,
                idle_expires_at, absolute_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7,
                     now() + make_interval(secs => $8), now() + make_interval(secs => $9))",
        )
        .bind(token::hash_token(&plaintext))
        .bind(new.user_id.0).bind(new.workspace_id.0).bind(new.app_id)
        .bind(new.version_id).bind(new.mode.as_str()).bind(new.login_sid)
        .bind(idle.as_secs_f64()).bind(absolute.as_secs_f64())
        .execute(self.db.pool())
        .await
        .map_err(anyhow::Error::from)?;
        Ok(plaintext)
    }

    pub async fn resolve(&self, token_plain: &str, idle: Duration) -> egret_core::Result<Option<AppSession>> {
        if !token_plain.starts_with(TOKEN_PREFIX) { return Ok(None); }
        let row = sqlx::query(
            "UPDATE analytics.app_sessions
                SET last_used_at = now(),
                    idle_expires_at = LEAST(now() + make_interval(secs => $2), absolute_expires_at)
              WHERE token_hash = $1
                AND revoked_at IS NULL
                AND idle_expires_at > now()
                AND absolute_expires_at > now()
          RETURNING id, user_id, workspace_id, app_id, version_id, mode",
        )
        .bind(token::hash_token(token_plain))
        .bind(idle.as_secs_f64())
        .fetch_optional(self.db.pool())
        .await
        .map_err(anyhow::Error::from)?;
        row.map(|r| Ok(AppSession {
            id: r.get("id"),
            user_id: UserId(r.get("user_id")),
            workspace_id: WorkspaceId(r.get("workspace_id")),
            app_id: r.get("app_id"),
            version_id: r.get("version_id"),
            mode: AppMode::parse(r.get::<&str, _>("mode"))?,
        })).transpose()
    }

    pub async fn revoke(&self, token_plain: &str) -> egret_core::Result<()> {
        sqlx::query("UPDATE analytics.app_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL")
            .bind(token::hash_token(token_plain))
            .execute(self.db.pool())
            .await
            .map_err(anyhow::Error::from)?;
        Ok(())
    }
}
```

`crates/egret-apps/src/lib.rs` for now:

```rust
//! Data Apps: the query gateway and its session tokens (spec docs/en/specs/2026-09-30-marimo-data-apps-design.md).
pub mod session;
```

Check two things against the real code:
- `Db`'s pool accessor, `pool()` versus a public field, in `crates/egret-metastore/src/lib.rs:45`;
- whether `WorkspaceId`/`UserId` are re-exported at `egret_core::` or live under `egret_core::ids`.

Adjust the imports to match.

- [ ] **Step 5: Run the tests until they pass**

Run: `cargo test -p egret-apps --test session_repo`
Expected: 5 passed. Confirm they really ran against the DB, not skipped: the output must show 5 tests executed, with nonzero durations.

- [ ] **Step 6: Mutation check**

Temporarily delete `AND revoked_at IS NULL` from `resolve`, and confirm `unknown_revoked_idle_and_absolute_expired_tokens_do_not_resolve` fails. Restore the line, and confirm with `git diff` that the file is unchanged. Record the check in the report.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/egret-apps
git commit -m "feat(apps): egret-apps crate and hashed per-session gateway tokens"
```

---

### Task 3: The read-only SQL gate

**Files:**
- Create: `crates/egret-apps/src/readonly.rs`
- Modify: `crates/egret-apps/src/lib.rs` (`pub mod readonly;`)
- Test: unit tests inside `readonly.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Copy)] pub enum EngineDialect { Trino, StarRocks }
impl EngineDialect { pub fn from_engine(engine: &str) -> Option<Self>; }  // "trino" | "starrocks"
#[derive(Debug, PartialEq)] pub enum ReadOnlyError { Unparseable(String), NotExactlyOne(usize), NotReadOnly(&'static str) }
impl std::fmt::Display for ReadOnlyError;   // user-facing message
pub fn check_read_only(sql: &str, dialect: EngineDialect) -> Result<(), ReadOnlyError>;
```

- [ ] **Step 1: Write the failing tests**

At the bottom of `readonly.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use EngineDialect::*;

    fn ok(sql: &str) { for d in [Trino, StarRocks] { assert_eq!(check_read_only(sql, d), Ok(()), "{d:?}: {sql}"); } }
    fn no(sql: &str) { for d in [Trino, StarRocks] { assert!(check_read_only(sql, d).is_err(), "{d:?} accepted: {sql}"); } }

    #[test] fn plain_reads_pass() {
        ok("SELECT 1");
        ok("SELECT 1;");
        ok("select a, count(*) from t where b > 1 group by a order by 2 desc limit 10");
        ok("WITH x AS (SELECT * FROM t) SELECT * FROM x JOIN u ON x.id = u.id");
        ok("SELECT * FROM (SELECT 1 AS a) s UNION ALL SELECT 2");
        ok("-- a comment\nSELECT /* inline */ 1");
        ok("SELECT * FROM cat.sch.t");
    }
    #[test] fn metadata_reads_pass() {
        ok("EXPLAIN SELECT * FROM t");
        ok("DESCRIBE t");
        ok("SHOW TABLES");
        ok("SHOW COLUMNS FROM t");
    }
    #[test] fn writes_and_ddl_are_rejected() {
        no("INSERT INTO t VALUES (1)");
        no("UPDATE t SET a = 1");
        no("DELETE FROM t");
        no("MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE");
        no("CREATE TABLE t2 AS SELECT * FROM t");
        no("DROP TABLE t");
        no("TRUNCATE TABLE t");
        no("GRANT SELECT ON t TO u");
        no("CALL system.flush_metadata_cache()");
        no("SET SESSION query_max_run_time = '1h'");
        no("USE other_schema");
    }
    #[test] fn smuggling_is_rejected() {
        no("SELECT 1; DROP TABLE t");
        no("/* harmless */ DELETE FROM t");
        no("EXPLAIN DELETE FROM t");
        no("SELECT * INTO t2 FROM t");
        no("SELECT * FROM t FOR UPDATE");
        no("WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x");
    }
    #[test] fn table_functions_are_rejected() {
        // Trino pass-through: runs arbitrary SQL on the underlying database.
        no("SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t'))");
        no("SELECT * FROM my_table_function(1)");
    }
    #[test] fn unparseable_and_empty_are_rejected() {
        no("");
        no("   ");
        no("SELEC 1");
        no("this is not sql");
    }
    #[test] fn error_kinds() {
        assert!(matches!(check_read_only("SELECT 1; SELECT 2", Trino), Err(ReadOnlyError::NotExactlyOne(2))));
        assert!(matches!(check_read_only("SELEC 1", Trino), Err(ReadOnlyError::Unparseable(_))));
        assert!(matches!(check_read_only("DELETE FROM t", Trino), Err(ReadOnlyError::NotReadOnly(_))));
    }
    #[test] fn dialect_from_engine() {
        assert!(matches!(EngineDialect::from_engine("trino"), Some(Trino)));
        assert!(matches!(EngineDialect::from_engine("starrocks"), Some(StarRocks)));
        assert!(EngineDialect::from_engine("postgres").is_none());
    }
}
```

Before relying on `from_engine`, check the engine strings stored in `query.databases.engine`: `git grep -n "\"trino\"\|\"starrocks\"" crates/egret-query-api/src`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p egret-apps readonly`
Expected: a compile failure, because `check_read_only` does not exist yet.

- [ ] **Step 3: Implement `readonly.rs`**

The AST variant names below are from recent `sqlparser` releases. Match them to the pinned version, and record any renames.

```rust
//! Read-only gate for the Data Apps query gateway (spec §4.3, D6).
//!
//! Fail closed: anything this module cannot parse, or any construct it does not
//! positively recognise as a read, is rejected. It is defence in addition to
//! Ranger, not instead of it. Table functions are rejected because Trino's
//! `TABLE(<catalog>.system.query(...))` passes arbitrary SQL — including writes —
//! to the underlying database.

use std::ops::ControlFlow;

use sqlparser::ast::{Query, SetExpr, Statement, TableFactor, Visit, Visitor};
use sqlparser::dialect::{Dialect, GenericDialect, MySqlDialect};
use sqlparser::parser::Parser;

#[derive(Debug, Clone, Copy)]
pub enum EngineDialect { Trino, StarRocks }

impl EngineDialect {
    pub fn from_engine(engine: &str) -> Option<Self> {
        match engine { "trino" => Some(Self::Trino), "starrocks" => Some(Self::StarRocks), _ => None }
    }
    fn dialect(self) -> Box<dyn Dialect> {
        match self { Self::Trino => Box::new(GenericDialect {}), Self::StarRocks => Box::new(MySqlDialect {}) }
    }
}

#[derive(Debug, PartialEq)]
pub enum ReadOnlyError { Unparseable(String), NotExactlyOne(usize), NotReadOnly(&'static str) }

impl std::fmt::Display for ReadOnlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unparseable(e) => write!(f, "the SQL could not be parsed, so it cannot be shown to be read-only ({e}); rewrite it in standard SQL"),
            Self::NotExactlyOne(n) => write!(f, "exactly one statement is allowed per query, got {n}"),
            Self::NotReadOnly(what) => write!(f, "data apps may only read data: {what} is not allowed"),
        }
    }
}

pub fn check_read_only(sql: &str, dialect: EngineDialect) -> Result<(), ReadOnlyError> {
    let stmts = Parser::parse_sql(&*dialect.dialect(), sql).map_err(|e| ReadOnlyError::Unparseable(e.to_string()))?;
    if stmts.len() != 1 { return Err(ReadOnlyError::NotExactlyOne(stmts.len())); }
    check_statement(&stmts[0])
}

fn check_statement(stmt: &Statement) -> Result<(), ReadOnlyError> {
    match stmt {
        Statement::Query(q) => check_query(q),
        Statement::Explain { statement, .. } => check_statement(statement),
        Statement::ExplainTable { .. } => Ok(()),
        Statement::ShowTables { .. } | Statement::ShowColumns { .. } | Statement::ShowCreate { .. }
        | Statement::ShowSchemas { .. } | Statement::ShowDatabases { .. } => Ok(()),
        _ => Err(ReadOnlyError::NotReadOnly("this statement kind")),
    }
}

fn check_query(q: &Query) -> Result<(), ReadOnlyError> {
    struct Guard;
    impl Visitor for Guard {
        type Break = ReadOnlyError;
        fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<Self::Break> {
            if !q.locks.is_empty() { return ControlFlow::Break(ReadOnlyError::NotReadOnly("a locking clause")); }
            match &*q.body {
                SetExpr::Select(s) if s.into.is_some() => ControlFlow::Break(ReadOnlyError::NotReadOnly("SELECT … INTO")),
                SetExpr::Select(_) | SetExpr::Query(_) | SetExpr::SetOperation { .. }
                | SetExpr::Values(_) | SetExpr::Table(_) => ControlFlow::Continue(()),
                _ => ControlFlow::Break(ReadOnlyError::NotReadOnly("a data-modifying query body")),
            }
        }
        fn pre_visit_table_factor(&mut self, t: &TableFactor) -> ControlFlow<Self::Break> {
            match t {
                TableFactor::Table { args: None, .. } | TableFactor::Derived { .. }
                | TableFactor::NestedJoin { .. } | TableFactor::UNNEST { .. } => ControlFlow::Continue(()),
                _ => ControlFlow::Break(ReadOnlyError::NotReadOnly("a table function")),
            }
        }
        fn pre_visit_statement(&mut self, _: &Statement) -> ControlFlow<Self::Break> {
            ControlFlow::Break(ReadOnlyError::NotReadOnly("a statement nested in a query"))
        }
    }
    match q.visit(&mut Guard) { ControlFlow::Break(e) => Err(e), ControlFlow::Continue(()) => Ok(()) }
}
```

Adjustments you may need:
- **`SHOW` variants.** The show-statement variant names (for example `ShowSchemas` / `ShowDatabases` / `ShowCatalogs`) differ between versions. Include those that exist and that the metadata test strings parse into. If a Trino or StarRocks metadata form parses to an unlisted variant, leave it rejected and record it. Fail closed.
- **Parse failures in the dialect.** If `WITH x AS (DELETE …)` or `SELECT * FROM t FOR UPDATE` fails to parse in a dialect, it is still rejected (as `Unparseable`), and the test passes. That is intended.

- [ ] **Step 4: Run the tests until they pass**

Run: `cargo test -p egret-apps readonly`
Expected: all pass. If a `ok(...)` case fails because the dialect can't parse it, do **not** weaken the gate. Replace the case with a form both dialects parse, and record the removed form as a known limitation.

- [ ] **Step 5: Mutation checks**

Do three mutations, one at a time. For each, confirm the named test fails, then restore the code and confirm `git diff` is clean. Record all three in the report.
1. Make `pre_visit_table_factor` always `Continue`. `table_functions_are_rejected` must fail.
2. Make the `stmts.len() != 1` check `> 2`. `smuggling_is_rejected` or `error_kinds` must fail.
3. Replace the `_ =>` arm in `check_statement` with `Ok(())`. `writes_and_ddl_are_rejected` must fail.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-apps
git commit -m "feat(apps): fail-closed read-only SQL gate with table-function rejection"
```

---

### Task 4: Config, state, and the gateway handler

**Files:**
- Create: `crates/egret-apps/src/gateway.rs`
- Modify: `crates/egret-apps/src/lib.rs`, `crates/egret-config/src/lib.rs`
- Test: `crates/egret-apps/tests/gateway.rs`

**Interfaces:**
- Consumes:
  - `AppSessionRepo` and `AppMode` (Task 2);
  - `check_read_only` and `EngineDialect` (Task 3);
  - `egret_query_api::{QueryExecPort, Limits, Param}`;
  - `egret_core::authz::{Pdp, Subject, Resource, ObjectRef}`;
  - `egret_workspace::…::WorkspaceRepo::disabled_features`;
  - `egret_audit::{AuditSink, AuditEvent}`;
  - `egret_core::ratelimit::RateLimiter`.
- Produces:

```rust
// egret-config
pub struct AppsConfig { pub max_rows: usize, pub timeout_secs: u64, pub session_idle_secs: u64,
                        pub session_absolute_secs: u64, pub rate_max: u32, pub rate_window_secs: u64 }
// defaults: 10000, 60, 1800, 28800, 20, 2; env prefix EGRET_APPS__

// egret-apps
#[derive(Clone)]
pub struct AppsState {
    pub db: egret_metastore::Db,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub query: std::sync::Arc<dyn egret_query_api::QueryExecPort>,
    pub audit: egret_audit::AuditSink,
    pub sessions: session::AppSessionRepo,
    pub limiter: egret_core::ratelimit::RateLimiter,
    pub cfg: egret_config::AppsConfig,
}
pub fn gateway_router() -> axum::Router<AppsState>;      // POST /apps/query
#[derive(serde::Deserialize)]
pub struct QueryRequest { pub database: String, pub sql: String,
                          #[serde(default)] pub params: Vec<serde_json::Value>,
                          pub max_rows: Option<usize> }
// response: egret_core::TabularResult as JSON
```

- [ ] **Step 1: Add `AppsConfig`**

Put it in `crates/egret-config/src/lib.rs`, mirroring `KernelConfig` (~:181) and its `EGRET_KERNEL__*` handling. Include its `Default` and the defaults above. Add a unit test next to the existing config tests asserting the defaults, and one asserting that `EGRET_APPS__MAX_ROWS=5` overrides. Follow how the kernel config test sets env; figment turns a bare integer env value into the right type.

- [ ] **Step 2: Write the failing gateway tests**

`crates/egret-apps/tests/gateway.rs` follows `crates/egret-dataapi/tests/routes_authz.rs`:
- real DB and a real `CedarPdp::build(db, &EventBus::new())`;
- `AuditSink::new(64)`, keeping the receiver so the audit can be asserted;
- a recording fake port modelled on `FakePort` in `crates/egret-dataapi/tests/serving_common/mod.rs:325`, recording `(ws, database_id, principal, sql, max_rows)` and able to return a queued error.

The fixture creates:
- users `viewer` (`ws_viewer`), `editor` (`ws_editor`) and `outsider` (no grant), granted the way `routes_authz.rs:208-235` does;
- workspaces `A` and `B`;
- an egret login session per user;
- one `analytics.apps` row in A;
- `query.databases` rows `trino_prod` (engine `trino`) in A and `b_only` in B. Copy the insert from an existing test with `git grep -n "INSERT INTO query.databases" -- crates/*/tests | head`, filling only the columns without defaults.

Tokens are minted with `AppSessionRepo::mint`. Requests go through `Router::new().nest("/api", egret_apps::gateway_router()).with_state(st)` using `.oneshot(...)`, with `Authorization: Bearer <token>` and a JSON body.

Write one `#[tokio::test]` per case:

| # | Case | Expect |
|---|---|---|
| 1 | no `Authorization` / `Bearer garbage` / cookie only | 401, port not called |
| 2 | revoked token; idle-expired; login session deleted | 401 |
| 3 | user `status='disabled'` after minting | 401 |
| 4 | feature `apps` disabled for A (via the repo's disable call, or an insert into the disabled-features table used by `disabled_features`) | 403 (FeatureDisabled) |
| 5 | app soft-deleted (`deleted_at = now()`) | 404 |
| 6 | `outsider` with a run token for A's app | 403 |
| 7 | `viewer` run token, `SELECT 1` on `trino_prod` | 200; port called once with `principal = viewer`, `ws = A` (viewer lacks `query:run`, so this case pins the amended rule) |
| 8 | `viewer` **edit** token | 403 (needs `app:write` + `query:run`) |
| 9 | `editor` edit token, `SELECT 1` | 200 |
| 10 | database `b_only` (exists only in B) from an A token; and a name that exists nowhere | both 404 with identical bodies; port not called |
| 11 | `DELETE FROM t`; `SELECT 1; DROP TABLE t` | 400 whose message says why; port not called |
| 12 | port returns `Error::Forbidden` | 403 |
| 13 | port returns `Error::InvalidInput("line 1: …")` | 400 carrying that message |
| 14 | `max_rows: 1_000_000` | port saw `max_rows == 10000` (config cap); `max_rows: 5` → port saw 5; absent → 10000 |
| 15 | success | body has `columns`/`rows`/`truncated`; exactly one audit `Mutation` with `action == "app:query"`, `resource_id == app_id`, `detail.outcome == "ok"`, and `detail` containing no SQL text (only `sql_sha256`) |
| 16 | a failed call (case 12) | audit `detail.outcome == "forbidden"` |
| 17 | 25 rapid calls on one token (config `rate_max=20, window=2`) | at least one 429; a second token in the same window is unaffected |
| 18 | engine not supported (a `query.databases` row with another engine) | 400 "engine not supported by data apps" |

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p egret-apps --test gateway`
Expected: a compile failure, because the gateway does not exist yet.

- [ ] **Step 4: Implement `gateway.rs` and export it from `lib.rs`**

```rust
//! POST /api/apps/query — the Data Apps query gateway (spec §4.2).

use std::time::{Duration, Instant};

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use egret_core::authz::{ObjectRef, Resource, Subject};
use egret_core::{Error, Result, TabularResult, UserId, WorkspaceId};
use egret_query_api::{Limits, Param};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::readonly::{check_read_only, EngineDialect};
use crate::session::{AppMode, AppSession};
use crate::AppsState;

#[derive(serde::Deserialize)]
pub struct QueryRequest {
    pub database: String,
    pub sql: String,
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    pub max_rows: Option<usize>,
}

pub fn gateway_router() -> Router<AppsState> {
    Router::new().route("/apps/query", post(query))
}

async fn query(State(st): State<AppsState>, headers: HeaderMap, Json(req): Json<QueryRequest>) -> Result<Json<TabularResult>> {
    let token = bearer(&headers).ok_or(Error::Unauthorized)?;
    let idle = Duration::from_secs(st.cfg.session_idle_secs);
    let sess = st.sessions.resolve(token, idle).await?.ok_or(Error::Unauthorized)?;
    if !st.limiter.check(&sess.id.to_string()) { return Err(Error::TooManyRequests); }

    let started = Instant::now();
    let outcome = run(&st, &sess, &req).await;
    audit(&st, &sess, &req, &outcome, started.elapsed());
    outcome.map(Json)
}

async fn run(st: &AppsState, sess: &AppSession, req: &QueryRequest) -> Result<TabularResult> {
    ensure_user_active(st, sess.user_id).await?;
    ensure_feature_enabled(st, sess.workspace_id).await?;
    let folder_id = app_folder(st, sess).await?; // 404 if missing/deleted/other ws
    authorize(st, sess, folder_id).await?;
    let (database_id, engine) = database_by_name(st, sess.workspace_id, &req.database).await?;
    let dialect = EngineDialect::from_engine(&engine)
        .ok_or_else(|| Error::InvalidInput("engine not supported by data apps".into()))?;
    check_read_only(&req.sql, dialect).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let limits = Limits {
        max_rows: req.max_rows.unwrap_or(st.cfg.max_rows).min(st.cfg.max_rows),
        timeout: Duration::from_secs(st.cfg.timeout_secs),
    };
    let params = to_params(&req.params)?;
    if params.is_empty() {
        st.query.execute_as(sess.workspace_id.0, database_id, sess.user_id, &req.sql, limits).await
    } else {
        st.query.execute_as_with_params(sess.workspace_id.0, database_id, sess.user_id, &req.sql, &params, limits).await
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

async fn ensure_user_active(st: &AppsState, user: UserId) -> Result<()> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(user.0).fetch_optional(st.db.pool()).await.map_err(anyhow::Error::from)?;
    if status.as_deref() == Some("active") { Ok(()) } else { Err(Error::Unauthorized) }
}

async fn ensure_feature_enabled(st: &AppsState, ws: WorkspaceId) -> Result<()> {
    let disabled = egret_workspace::WorkspaceRepo::new(st.db.clone()).disabled_features(ws).await?;
    if disabled.contains("apps") { Err(Error::FeatureDisabled("apps")) } else { Ok(()) }
}

async fn app_folder(st: &AppsState, sess: &AppSession) -> Result<Option<Uuid>> {
    let row = sqlx::query("SELECT folder_id FROM analytics.apps WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL")
        .bind(sess.app_id).bind(sess.workspace_id.0)
        .fetch_optional(st.db.pool()).await.map_err(anyhow::Error::from)?;
    row.map(|r| r.get::<Option<Uuid>, _>("folder_id")).ok_or_else(|| Error::NotFound("app".into()))
}

async fn authorize(st: &AppsState, sess: &AppSession, folder_id: Option<Uuid>) -> Result<()> {
    let group_ids: Vec<Uuid> = sqlx::query_scalar("SELECT group_id FROM user_groups WHERE user_id = $1")
        .bind(sess.user_id.0).fetch_all(st.db.pool()).await.map_err(anyhow::Error::from)?;
    let subject = Subject { user_id: sess.user_id, group_ids: group_ids.into_iter().map(Into::into).collect() };
    let app = Resource::Object(ObjectRef { kind: "app".into(), id: sess.app_id, folder_id, workspace_id: sess.workspace_id });
    let allowed = |ok: bool| if ok { Ok(()) } else { Err(Error::Forbidden) };
    match sess.mode {
        AppMode::Run => allowed(st.pdp.authorize(&subject, "app:read", &app).await?),
        AppMode::Edit => {
            allowed(st.pdp.authorize(&subject, "app:write", &app).await?)?;
            allowed(st.pdp.authorize(&subject, "query:run", &Resource::Workspace(sess.workspace_id)).await?)
        }
    }
}

async fn database_by_name(st: &AppsState, ws: WorkspaceId, name: &str) -> Result<(Uuid, String)> {
    let row = sqlx::query("SELECT id, engine FROM query.databases WHERE workspace_id = $1 AND name = $2")
        .bind(ws.0).bind(name)
        .fetch_optional(st.db.pool()).await.map_err(anyhow::Error::from)?;
    row.map(|r| (r.get("id"), r.get("engine"))).ok_or_else(|| Error::NotFound("database".into()))
}

fn to_params(values: &[serde_json::Value]) -> Result<Vec<Param>> {
    values.iter().map(|v| match v {
        serde_json::Value::Null => Ok(Param::Null),
        serde_json::Value::Bool(b) => Ok(Param::Bool(*b)),
        serde_json::Value::Number(n) => n.as_i64().map(Param::I64).or_else(|| n.as_f64().map(Param::F64))
            .ok_or_else(|| Error::InvalidInput("unsupported number".into())),
        serde_json::Value::String(s) => Ok(Param::Str(s.clone())),
        _ => Err(Error::InvalidInput("params must be null, bool, number or string".into())),
    }).collect()
}

fn audit(st: &AppsState, sess: &AppSession, req: &QueryRequest, outcome: &Result<TabularResult>, took: Duration) {
    let (label, rows, truncated) = match outcome {
        Ok(t) => ("ok", Some(t.row_count), Some(t.truncated)),
        Err(Error::Unauthorized) => ("unauthorized", None, None),
        Err(Error::Forbidden) => ("forbidden", None, None),
        Err(Error::FeatureDisabled(_)) => ("feature_disabled", None, None),
        Err(Error::NotFound(_)) => ("not_found", None, None),
        Err(Error::InvalidInput(_)) => ("invalid", None, None),
        Err(_) => ("error", None, None),
    };
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(sess.user_id.0),
        impersonator: None,
        workspace_id: Some(sess.workspace_id.0),
        action: "app:query".into(),
        resource_type: "app".into(),
        resource_id: Some(sess.app_id.to_string()),
        detail: serde_json::json!({
            "mode": sess.mode.as_str(),
            "version_id": sess.version_id,
            "database": req.database,
            "sql_sha256": hex::encode(Sha256::digest(req.sql.as_bytes())),
            "outcome": label,
            "row_count": rows,
            "truncated": truncated,
            "duration_ms": took.as_millis() as u64,
        }),
    });
}
```

Additions to `lib.rs`:

```rust
pub mod gateway;
pub mod readonly;
pub mod session;

pub use gateway::{gateway_router, QueryRequest};

#[derive(Clone)]
pub struct AppsState {
    pub db: egret_metastore::Db,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub query: std::sync::Arc<dyn egret_query_api::QueryExecPort>,
    pub audit: egret_audit::AuditSink,
    pub sessions: session::AppSessionRepo,
    pub limiter: egret_core::ratelimit::RateLimiter,
    pub cfg: egret_config::AppsConfig,
}
```

Check these against the real code:
- `Subject`'s field types (`group_ids` may be `Vec<GroupId>`);
- whether `Pdp::authorize` returns `Result<bool>` or a decision enum;
- where `WorkspaceRepo` lives and how it is constructed;
- `AuditSink::emit`'s receiver type;
- whether `RateLimiter::check` returns `true` for *allowed* or for *limited* (`crates/egret-core/src/ratelimit.rs`, and its use in `crates/egret-auth/src/routes.rs:165-168`); invert the condition if needed, and case 17 pins it;
- whether `egret_core::Error` implements `IntoResponse` under the `http` feature (the Data API handlers rely on it).

Adapt to what you find, and keep the behaviour in the table above.

The rate-limit check happens **after** token resolution. An invalid token is 401, and it does not consume anyone's quota.

- [ ] **Step 5: Run the tests until they pass**

Run: `cargo test -p egret-apps --test gateway`
Expected: 18 cases pass. Confirm they executed against the DB, not skipped.

- [ ] **Step 6: Mutation checks**

Do each mutation, confirm the named case fails, then restore and confirm `git diff` is clean:
1. Remove the `query:run` check in `Edit`. Case 8 must fail.
2. Compare `max_rows` without `.min(cap)`. Case 14 must fail.
3. Drop `workspace_id = $1` from `database_by_name`. Case 10 must fail.
4. Skip `ensure_user_active`. Case 3 must fail.

Record all four in the report.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-apps crates/egret-config
git commit -m "feat(apps): POST /api/apps/query — viewer-identity, read-only query gateway"
```

---

### Task 5: Wiring, OpenAPI decision, full local CI

**Files:**
- Modify: `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-server/Cargo.toml`, `crates/egret-bin/Cargo.toml`
- Test: `crates/egret-server/tests/apps_gateway_mounted.rs` (new)

**Interfaces:**
- Consumes: `egret_apps::{AppsState, gateway_router}`, and `egret_config::AppsConfig` inside the root config.
- Produces: `POST /api/apps/query`, reachable without the CSRF header.

- [ ] **Step 1: Mount the router without CSRF**

In `crates/egret-server/src/lib.rs`, next to the kernel callback mount (:366-369), nest `egret_apps::gateway_router().with_state(apps_state)` under `/api`, **outside** `csrf_layer`, exactly as `kernel_callback_router` is. Add a comment:

```rust
// Data Apps gateway: bearer-token (per marimo session) callers inside app pods; no
// cookie is involved, so no CSRF layer (spec §4.2). Hand-gated for the `apps` feature.
```

- [ ] **Step 2: Build `AppsState` in `crates/egret-bin/src/main.rs`**

Next to where `DataApiState` is built (:2264-2272), reuse:
- the same `Arc<EngineState>` as `query` (build one `Arc` and clone it for both states if they currently construct separate ones);
- the shared audit sink;
- the PDP;
- the DB.

Also create `AppSessionRepo::new(db.clone())`, and `RateLimiter::new(cfg.apps.rate_max, cfg.apps.rate_window_secs, 0)` (no lockout).

Check that `RateLimiter::new` accepts a 0-second lockout. If it does not, use the smallest value it accepts, and record it.

- [ ] **Step 3: Write the failing mount test**

`crates/egret-server/tests/apps_gateway_mounted.rs` builds the app router the way existing egret-server tests do (see `feature_route_coverage.rs` for how they obtain it). It asserts:
- `POST /api/apps/query` with **no** `X-Egret-CSRF` header and no cookie returns **401**. A 401 proves the route exists and that CSRF did not block it with a 403.
- The same request with a garbage bearer token also returns 401.

- [ ] **Step 4: Decide the OpenAPI treatment, and implement it**

Leave the handler **un-annotated**, as `/metrics` is: it is a pod-to-egret internal endpoint with no frontend caller, and annotating it would force `npm run gen:api` to generate an unused client.

Put a doc comment on `gateway_router` stating this. Then run `cargo test -p egret-server committed_openapi_json_is_current`, which must pass unchanged.

If the team prefers annotating it anyway, that choice needs `egret openapi > openapi.json` plus `npm run gen:api`, with both outputs committed. Record which way was taken.

- [ ] **Step 5: Run full local CI, one suite at a time, after committing**

```bash
git add -A && git commit -m "feat(apps): mount the Data Apps query gateway outside CSRF"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: everything is green.

- DB-gated tests must actually run. Confirm with the test count, and by checking that `egret-apps` tests report real durations.
- If a failure looks pre-existing, prove it on `origin/main` before labelling it so. "Pre-existing" is a hypothesis until it has been measured on main.
- If `cargo test --workspace` exceeds about 2 minutes, it will be backgrounded. Wait for it to finish, and read the whole output.

- [ ] **Step 6: Commit any fmt or clippy fixes**

```bash
git add -A && git commit -m "chore(apps): fmt and clippy"
```

---

### Task 6: Measure an engine-side read-only session (spec §4.3)

The spec asks P1 to *measure* whether the delegated connection can also be put into a read-only mode on each engine, as defence in depth. This is a measurement task. Its output is a short result document; no code is required.

**Files:**
- Create: `docs/en/plans/<date>-data-apps-p1-engine-readonly-result.md`

- [ ] **Step 1: Trino**

On the kind stack, following the manual Trino integration test recipe (the `egret-manual-trino-integration-test` memory / repo docs), check whether any session property or connector/catalog setting makes a delegated Trino session reject writes. Look in the pinned Trino version's docs for session-level and connector-level read-only or access-control options, and try each candidate. Do not assume one exists or does not exist; record what you tried.

Record the finding, with the exact statement tried and the engine's response.

- [ ] **Step 2: StarRocks**

Check whether the StarRocks delegated user connection can run `SET SESSION transaction_read_only = true` / `SET query_timeout …` equivalents, and whether a subsequent `INSERT` is rejected.

Record the statement and the response.

- [ ] **Step 3: Write the result**

Write, per engine:
- available: yes/no;
- the mechanism;
- evidence (exact statements and outputs);
- a recommendation for P3/P4: adopt it, or rely on the parser plus Ranger.

Do not implement anything here. If it is worth adopting, it becomes a follow-up task.

- [ ] **Step 4: Commit**

```bash
git add docs/en/plans/*-data-apps-p1-engine-readonly-result.md
git commit -m "docs(apps): P1 engine-side read-only session measurement"
```

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| §4.1 opaque token, hash-only storage, binding (user, ws, app, version/draft, mode, login sid) | Task 1 (table), Task 2 (repo) |
| §4.1 idle 30 min / absolute 8 h, revoke on logout / WS close | Task 2. WS-close revoke is exposed as `revoke()`; the proxy calls it in P4. Logout revocation is by FK cascade. |
| §4.1 re-authorize on every call | Task 4 (`run`) |
| §4.2 feature hand-gate, `app:read` / `app:write` + `query:run`, database by name, read-only, `execute_as`, audit, error mapping, rate limit | Task 4 |
| §4.3 fail-closed parser, one statement, no DML, no data-modifying CTE | Task 3 |
| §4.3 engine-side read-only measurement | Task 6 |
| §8 `app:*` permissions, `apps` feature | Task 1 |
| D6 read-only | Task 3 + Task 4 |

Out of P1, by design:
- minting tokens from the proxy (P4);
- `app_versions` and the FK from `app_sessions.version_id` (P2);
- the Python client `egret_marimo` (P3);
- shares for the `app` object kind (P2).
