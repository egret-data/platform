# Data Apps P2 — storage and lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give data apps a home in egret. P2 adds:
- workspace-scoped app CRUD (create, list, get, update, trash, restore, folder, bookmark);
- one revisioned draft per app, saved with `If-Match`;
- immutable published versions, with publish and restore;
- object shares, managed with `app:manage`.

**Architecture:**
- All cookie-session routes live in a new `routes.rs` in the existing `egret-apps` crate, under `/api/ws/{workspace}/apps…`, in a CSRF-protected router group. They follow the notebook crate's conventions: `WsContext`, `require` for list and create, `require_object` on `ObjectRef{kind:"app"}` for everything with an id, and `utoipa` annotations.
- A new `store.rs` owns all SQL.
- Shares, folders and bookmarks reuse egret's generic machinery: `egret-bi` shares, `kind_perms` for folders, and `analytics.bookmarks`. P2 only registers the `app` kind with them.
- Migration `0163` adds `app_drafts` and `app_versions`. Versions are append-only through a trigger that still allows cascade deletes. The migration also adds the deferred FKs from P1: `apps.folder_id`, and a composite `app_sessions(version_id, app_id)`.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9 runtime queries, utoipa 5), Postgres, Cedar PDP via `egret_core::authz`, orval-generated frontend client (`npm run gen:api`).

**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md`:
- §5 (tables, draft API, publish, restore);
- §8 (permissions, shares);
- D5 (draft plus immutable versions);
- the P2 line of §9, including the P1 final-review carry-overs.

## Global Constraints

- **Branch:** `feat/data-apps-p2-lifecycle`, off `origin/main`, in its own worktree. **Migrations start at `0163`.** If main has moved, use the next free numbers.
- **Git:** use the configured identity. Write Conventional Commits that end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **sqlx 0.9:** runtime `sqlx::query` only, with `sqlx::AssertSqlSafe(...)` for non-literal SQL. `Db.pool` is a public **field**.
- **Behaviour, as acceptance criteria:**
  - A draft's `source` is at most **1 MiB** (1 048 576 bytes). The DB `CHECK` enforces it, and the handler checks first and returns **413**.
  - `GET …/draft` returns `{source, revision}` and an `ETag: "<revision>"` header.
  - `PUT …/draft` **requires** `If-Match`:
    - missing → **428**;
    - stale → **409** with `{current_revision}` in the body;
    - success → the new revision and a new `ETag`.
  - **Publish** copies the draft at its current revision into `version_no = max + 1`. If the body has `expected_revision` and the draft is no longer at that revision → **409**.
  - **Restore** publishes an old version's source as a **new** version. It never moves a pointer.
  - `app_versions` is **append-only**: UPDATE always fails, and DELETE fails unless the parent app row is gone (a cascade).
  - **Apps are trashed, never hard-deleted**, in P2.
  - **Permissions:**

    | Action | Permission |
    |---|---|
    | list | `app:read` (workspace) |
    | create | `app:write` (workspace) |
    | get, bookmark | `app:read` on the object |
    | update, trash, restore, folder, draft (read and write), publish, versions, version restore | `app:write` on the object |
    | shares | `app:manage` on the object, through the generic shares router |

    The draft is the author's working copy: viewers see published versions only, and run mode comes in P5.
  - An app outside the caller's workspace, or a missing one, is **404**, never 403. This follows notebook's `notebook_object_ref`.
  - **Audit:** create, update, trash, restore, folder, publish, version-restore and share changes. Shares are already audited by egret-bi; verify this. **Draft saves are not audited:** marimo autosaves, and auditing them would flood the bounded audit channel.
- **Local CI before the PR.** GitHub CI is out of quota. Run these, **one suite at a time**:
  - `cargo fmt --all -- --check`
  - `cargo clippy` with `-D warnings` on the touched crates (`--no-deps` if main's clippy 1.97 debt is still unfixed)
  - `cargo test --workspace` with a real test DB, plus `EGRET_TEST_SHARED_STORE_URL` and `EGRET_TEST_STARROCKS_URL` exported
  - `openapi.json` current
  - frontend `npm ci && npm run lint && npm run typecheck && npm test && npm run build`, and `gen:api` output committed
- **Environment:**
  - `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`. **Never use the dev DB `egret`.**
  - `CARGO_TARGET_DIR=<worktree>/target` and `CARGO_INCREMENTAL=0`.
  - Check `df -h` before full builds. About 40 GB is needed.
  - On a mass `VersionMissing`/`VersionMismatch`, drop **only** `egret_test_template`.
  - Restore mutations with `git checkout -- <file>`, because `cp` is interactive in this shell.
- **Illustrative code is a guess.** Signatures come from `origin/main` (f5e2e84d). Where the real code differs, follow it and record the deviation.

---

## File structure

| File | Responsibility |
|---|---|
| `migrations/0163_data_apps_lifecycle.sql` | `app_drafts`, `app_versions` (append-only trigger), `apps.folder_id` FK, composite `app_sessions` version FK |
| `migrations/0164_data_apps_manage_permission.sql` | Seed `app:manage` |
| `crates/egret-core/src/rbac.rs` (modify) | Declare `app:manage` |
| `crates/egret-bi/src/shares.rs` (modify) | `object_table("app") → "apps"`; `manage_action` becomes an explicit per-kind table that includes `"app" => "app:manage"` |
| `crates/egret-bi/src/routes.rs` (modify) | `kind_perms("apps") → ("app", "app:read", "app:write")` for folders |
| `crates/egret-auth/src/effective.rs` (modify) | `object_table("app") → "apps"` |
| `crates/egret-controlplane/tests/catalog_drift.rs` (modify) | Scan `egret-bi/src/shares.rs`; register `manage_action` as a permission-table function |
| `crates/egret-apps/src/template.rs` | The source of a new app (marimo file) |
| `crates/egret-apps/src/store.rs` | `AppStore`: app rows, drafts, versions, bookmarks, folder kind lookup |
| `crates/egret-apps/src/routes.rs` | `AppsApiState`, `apps_router()`, handlers, DTOs, `AppsApi` OpenAPI |
| `crates/egret-apps/src/lib.rs` (modify) | Export the new modules and `openapi()` |
| `crates/egret-apps/tests/lifecycle_store.rs` | DB tests: schema rules plus the store |
| `crates/egret-apps/tests/http.rs` | Route tests (cookie session, CSRF, real Cedar PDP) |
| `crates/egret-server/src/lib.rs` (modify) | Mount `apps_router` with `csrf_layer`; merge `egret_apps::openapi()` |
| `crates/egret-bin/src/main.rs` (modify) | Build `AppsApiState` |
| `openapi.json`, `frontend/src/api/generated/**` (regenerate) | API contract and client |
| `frontend/src/hooks/useWorkspaceFeatures.ts`, `frontend/src/api/folders.ts`, `frontend/src/components/sharing/ShareDialog.tsx` (modify) | Add `"apps"` / `"app"` to the key and kind lists; no UI yet |

---

### Task 1: Schema, `app:manage`, and registering the `app` kind

**Files:**
- Create: `migrations/0163_data_apps_lifecycle.sql`, `migrations/0164_data_apps_manage_permission.sql`
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-bi/src/shares.rs`, `crates/egret-bi/src/routes.rs`, `crates/egret-auth/src/effective.rs`, `crates/egret-controlplane/tests/catalog_drift.rs`
- Test: `crates/egret-apps/tests/lifecycle_store.rs` (schema section), plus the existing `crates/egret-bi/tests/http.rs` (add a test next to `data_api_endpoint_folders_are_a_known_kind`), plus `catalog_drift`

**Interfaces:**
- Produces:
  - `analytics.app_drafts(app_id PK→apps, source, revision, updated_by, updated_at)`
  - `analytics.app_versions(id, app_id, version_no, source, from_revision, published_by, published_at)`, with `UNIQUE (app_id, version_no)` and `UNIQUE (id, app_id)`
  - permission key `"app:manage"`
  - folder kind `"apps"`
  - share kind `"app"`

- [ ] **Step 1: Write `migrations/0163_data_apps_lifecycle.sql`**

```sql
-- Data Apps P2 (docs/en/specs/2026-09-30-marimo-data-apps-design.md §5): one revisioned
-- draft per app, immutable published versions, and the FKs P1 deferred.

CREATE TABLE analytics.app_drafts (
    app_id     uuid PRIMARY KEY REFERENCES analytics.apps(id) ON DELETE CASCADE,
    source     text NOT NULL CHECK (octet_length(source) <= 1048576),
    revision   bigint NOT NULL DEFAULT 1 CHECK (revision >= 1),
    updated_by uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE analytics.app_versions (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id        uuid NOT NULL REFERENCES analytics.apps(id) ON DELETE CASCADE,
    version_no    integer NOT NULL CHECK (version_no >= 1),
    source        text NOT NULL CHECK (octet_length(source) <= 1048576),
    -- the draft revision this version was published from; NULL for a restore
    from_revision bigint NULL,
    -- for a restore: the version whose source was republished
    restored_from uuid NULL REFERENCES analytics.app_versions(id),
    published_by  uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    published_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT app_versions_app_no_key UNIQUE (app_id, version_no),
    -- target of the composite FK from app_sessions: a pinned version belongs to that app
    CONSTRAINT app_versions_id_app_key UNIQUE (id, app_id)
);
CREATE INDEX app_versions_app_no_desc_idx ON analytics.app_versions (app_id, version_no DESC);

-- Append-only (spec D5). UPDATE is always refused. DELETE is refused unless the
-- parent app row is already gone, i.e. the delete is the ON DELETE CASCADE from
-- analytics.apps (itself reached from a workspace delete); without that exception a
-- workspace with published apps could never be deleted.
CREATE FUNCTION analytics.app_versions_append_only() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'analytics.app_versions is append-only (version %)', OLD.id
            USING ERRCODE = 'restrict_violation';
    END IF;
    IF EXISTS (SELECT 1 FROM analytics.apps WHERE id = OLD.app_id) THEN
        RAISE EXCEPTION 'analytics.app_versions is append-only (version %)', OLD.id
            USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER app_versions_append_only_trg
    BEFORE UPDATE OR DELETE ON analytics.app_versions
    FOR EACH ROW EXECUTE FUNCTION analytics.app_versions_append_only();

-- P1 deferred: folders (cascade-safe — a deleted folder leaves the app Uncategorized).
ALTER TABLE analytics.apps
    ADD CONSTRAINT apps_folder_fk FOREIGN KEY (folder_id)
        REFERENCES analytics.folders(id) ON DELETE SET NULL;

-- P1 deferred: a run session pins a version OF ITS OWN APP. Edit sessions have
-- version_id NULL, which MATCH SIMPLE leaves unchecked — as intended.
ALTER TABLE analytics.app_sessions
    ADD CONSTRAINT app_sessions_version_app_fk FOREIGN KEY (version_id, app_id)
        REFERENCES analytics.app_versions(id, app_id) ON DELETE CASCADE;
```

Before relying on the trigger's cascade exception, the Step 3 tests must **prove** it. Whether the parent row is invisible during an `ON DELETE CASCADE` depends on Postgres internals. If the `EXISTS` check still sees the parent, use `pg_trigger_depth() > 1` as the "this is a cascade" signal instead, and record which mechanism works.

- [ ] **Step 2: Write `migrations/0164_data_apps_manage_permission.sql`, and declare the permission**

```sql
-- app:manage (spec §8): manage an app's shares. P1 deferred it so that no declared
-- permission went unenforced; P2 adds the enforcement site (egret-bi shares).
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:manage' FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
```

In `crates/egret-core/src/rbac.rs`, next to `app:read`/`app:write`:

```rust
perm!("app:manage", "Apps", "Manage data-app shares", Scope::Workspace),
```

- [ ] **Step 3: Write the failing schema tests**

Put these in `crates/egret-apps/tests/lifecycle_store.rs`. Reuse the `Fixture` from `tests/session_repo.rs` by copying the helpers you need. The fixture needs a user, a workspace, an app and a login session.

```rust
#[tokio::test] async fn versions_cannot_be_updated() {
    // insert a version; UPDATE … SET source = 'x' → SQLSTATE 23001 (restrict_violation)
}
#[tokio::test] async fn versions_cannot_be_deleted_directly() {
    // DELETE FROM analytics.app_versions WHERE id = $1 → 23001
}
#[tokio::test] async fn deleting_the_workspace_cascades_through_versions() {
    // workspace → app → version + draft + run session pinned to that version;
    // DELETE FROM workspaces WHERE id = $1 succeeds and leaves no rows behind
}
#[tokio::test] async fn version_numbers_are_unique_per_app() { /* second (app, 1) → 23505 */ }
#[tokio::test] async fn draft_source_over_one_mib_is_rejected_by_the_schema() { /* 1_048_577 bytes → 23514 */ }
#[tokio::test] async fn a_run_session_must_pin_a_version_of_its_own_app() {
    // version of app B; mint a run session for app A with that version_id → 23503
}
#[tokio::test] async fn deleting_a_folder_uncategorizes_its_apps() { /* apps.folder_id → NULL */ }
```

Write the bodies as real SQL through `sqlx::query`, each asserting the exact SQLSTATE through `err.as_database_error().unwrap().code()`. Fresh random ids keep tests isolated.

- [ ] **Step 4: Run them**

Run: `cargo test -p egret-apps --test lifecycle_store`

Expected: before the migration, the tests fail because the relations don't exist. After it, all pass. If `deleting_the_workspace_cascades_through_versions` fails, apply the `pg_trigger_depth()` fallback from Step 1 and re-run.

- [ ] **Step 5: Register the `app` kind in egret-bi and egret-auth**

- In `crates/egret-bi/src/shares.rs`:
  - add `"app" => Some("apps")` to `object_table` (:22-33);
  - turn `manage_action` (:37-43) into an explicit table of literals, for example:

```rust
fn manage_action(kind: &str) -> Option<&'static str> {
    match kind {
        "notebook" => Some("notebook:write"),
        "dashboard" => Some("dashboard:manage"),
        // … one arm per kind object_table accepts, each copied from what format!("{kind}:manage")
        //    produced before — read object_table to list them exactly …
        "app" => Some("app:manage"),
        _ => None,
    }
}
```

  Update the three call sites to handle `None` as `NotFound("object kind")`. Keep the behaviour of every existing kind identical: list what `format!` produced for each kind, and add a unit test asserting each one.
- In `crates/egret-bi/src/routes.rs` `kind_perms` (:3380-3404): `"apps" => Some(("app", "app:read", "app:write")),`.
- In `crates/egret-auth/src/effective.rs:282-292` `object_table`: `"app" => Some("apps")`.

- [ ] **Step 6: Make `catalog_drift` see `app:manage`**

In `crates/egret-controlplane/tests/catalog_drift.rs`:
- add `"../egret-bi/src/shares.rs"` to `EXTRA_FILES` (:49-89);
- add `"fn manage_action("` to `PERM_TABLE_FNS` (:121).

Run `cargo test -p egret-controlplane --test catalog_drift`. Expected: green, with `ALLOWED_UNENFORCED` still empty. If scanning `shares.rs` surfaces any **other** key as unenforced or mis-scanned, stop and report. Do not paper over it.

- [ ] **Step 7: Kind-registration tests**

In `crates/egret-bi/tests/http.rs`, next to `data_api_endpoint_folders_are_a_known_kind` (:238):
- `app_folders_are_a_known_kind`: creating a folder of kind `apps` succeeds for a `ws_editor`.
- An `app` share round-trip: a `ws_admin` PUTs a `ws_viewer` grant on an app, GETs it, then DELETEs it. A `ws_editor` without `app:manage` gets 403 on the PUT.

Mirror the existing share tests in that file for the fixture.

- [ ] **Step 8: Mutation checks, then commit**

Do these mutations one at a time, confirm the named test fails, and restore with `git checkout --`:
1. Drop the trigger's `IF TG_OP = 'UPDATE'` block → `versions_cannot_be_updated` fails.
2. Drop the `"app"` arm in `manage_action` → the share test fails.
3. Drop `app_sessions_version_app_fk` → `a_run_session_must_pin_a_version_of_its_own_app` fails.

Then commit:

```bash
git add migrations/0163_data_apps_lifecycle.sql migrations/0164_data_apps_manage_permission.sql crates/egret-core crates/egret-bi crates/egret-auth crates/egret-controlplane crates/egret-apps/tests/lifecycle_store.rs
git commit -m "feat(apps): drafts, append-only versions, app:manage, and the app kind for shares and folders"
```

---

### Task 2: `AppStore` and the new-app template

**Files:**
- Create: `crates/egret-apps/src/store.rs`, `crates/egret-apps/src/template.rs`
- Modify: `crates/egret-apps/src/lib.rs`, `crates/egret-apps/Cargo.toml` (add `chrono`, and `serde` if needed)
- Test: `crates/egret-apps/tests/lifecycle_store.rs` (store section)

**Interfaces:**
- Produces:

```rust
pub struct AppRow { pub id: Uuid, pub workspace_id: WorkspaceId, pub name: String, pub description: String,
                    pub folder_id: Option<Uuid>, pub owner_user_id: Option<Uuid>,
                    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
                    pub deleted_at: Option<DateTime<Utc>>,
                    pub draft_revision: i64, pub latest_version_no: Option<i32>, pub bookmarked: bool }
pub struct Draft { pub source: String, pub revision: i64, pub updated_at: DateTime<Utc> }
pub struct VersionSummary { pub id: Uuid, pub version_no: i32, pub from_revision: Option<i64>,
                            pub restored_from: Option<Uuid>, pub published_by: Option<Uuid>, pub published_at: DateTime<Utc> }
pub struct Version { pub summary: VersionSummary, pub source: String }
pub enum SaveOutcome { Saved { revision: i64 }, Stale { current_revision: i64 } }
pub enum PublishOutcome { Published(VersionSummary), Stale { current_revision: i64 } }
pub const MAX_SOURCE_BYTES: usize = 1_048_576;

impl AppStore {
    pub fn new(db: Db) -> Self;
    pub async fn create(&self, ws: WorkspaceId, owner: UserId, name: &str, description: &str,
                        folder_id: Option<Uuid>, source: &str) -> Result<AppRow>;           // app + draft rev 1, one tx
    pub async fn list(&self, ws: WorkspaceId, user: UserId, trashed: bool) -> Result<Vec<AppRow>>;
    pub async fn get(&self, ws: WorkspaceId, user: UserId, id: Uuid) -> Result<Option<AppRow>>; // incl. trashed
    pub async fn update(&self, ws: WorkspaceId, id: Uuid, name: Option<&str>, description: Option<&str>) -> Result<bool>;
    pub async fn set_trashed(&self, ws: WorkspaceId, id: Uuid, trashed: bool) -> Result<bool>;
    pub async fn set_folder(&self, ws: WorkspaceId, id: Uuid, folder_id: Option<Uuid>) -> Result<bool>;
    pub async fn folder_kind(&self, folder_id: Uuid) -> Result<Option<(Uuid, String)>>;     // (workspace, entity_type)
    pub async fn set_bookmark(&self, user: UserId, id: Uuid, on: bool) -> Result<()>;
    pub async fn draft(&self, id: Uuid) -> Result<Option<Draft>>;
    pub async fn save_draft(&self, id: Uuid, by: UserId, expected: i64, source: &str) -> Result<SaveOutcome>;
    pub async fn publish(&self, id: Uuid, by: UserId, expected: Option<i64>) -> Result<PublishOutcome>;
    pub async fn versions(&self, id: Uuid) -> Result<Vec<VersionSummary>>;                // newest first
    pub async fn version(&self, id: Uuid, version_id: Uuid) -> Result<Option<Version>>;   // scoped to the app
    pub async fn restore_version(&self, id: Uuid, version_id: Uuid, by: UserId) -> Result<Option<VersionSummary>>;
}
// template.rs
pub fn new_app_source(database: Option<&str>) -> String;
```

- [ ] **Step 1: Write the failing store tests**

Add them to the store section of `lifecycle_store.rs`:

```rust
#[tokio::test] async fn create_makes_an_app_with_a_rev1_draft_from_the_template() { /* draft().revision == 1, source == template */ }
#[tokio::test] async fn save_draft_bumps_revision_and_refuses_stale() {
    // save(expected=1) → Saved{2}; save(expected=1) → Stale{current_revision: 2}; draft source is the first save's
}
#[tokio::test] async fn save_draft_over_limit_is_an_error_not_a_save() { /* MAX_SOURCE_BYTES + 1 → Err(InvalidInput) */ }
#[tokio::test] async fn publish_numbers_versions_and_honours_expected_revision() {
    // publish(None) → v1 from_revision 1; save → rev 2; publish(Some(1)) → Stale{2}; publish(Some(2)) → v2
}
#[tokio::test] async fn concurrent_publishes_get_distinct_numbers() {
    // tokio::join! two publish(None) on the same app → version_no {1, 2}, no error
}
#[tokio::test] async fn restore_republishes_old_source_as_a_new_version() {
    // v1 "a"; draft "b"; v2; restore(v1) → v3 with source "a", restored_from = v1.id, from_revision None
}
#[tokio::test] async fn version_lookup_is_scoped_to_its_app() { /* version(app_b, v_of_app_a) → None */ }
#[tokio::test] async fn list_separates_trashed_and_marks_bookmarks() { /* trashed=false excludes, true includes; bookmarked per user */ }
#[tokio::test] async fn rows_from_another_workspace_are_invisible() { /* get/update/set_trashed with the wrong ws → None/false */ }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p egret-apps --test lifecycle_store`
Expected: compile errors, because `store`/`template` don't exist yet.

- [ ] **Step 3: Implement `template.rs`**

```rust
//! The source a new data app starts from (spec D13: the template pre-creates the
//! connection cell so the egret engine is listed and selected from the first run).

pub fn new_app_source(database: Option<&str>) -> String {
    let connect = match database {
        Some(db) => format!(
            "\n\n@app.cell\ndef _(egret):\n    # Queries run with each viewer's own permissions.\n    db = egret.connect({db:?})\n    return (db,)\n"
        ),
        None => String::new(),
    };
    format!(
        r#"import marimo

__generated_with = "0.25.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import egret_marimo as egret
    return egret, mo
{connect}

@app.cell
def _(mo):
    mo.md("# New data app")
    return


if __name__ == "__main__":
    app.run()
"#
    )
}
```

A database name containing `"` or `\` would break the file. The handler (Task 3) resolves the name against `query.databases` first, so only real names reach the template. Rust's `{:?}` escapes it anyway. Add a unit test that `new_app_source(Some("a\"b"))` contains `egret.connect("a\"b")`.

- [ ] **Step 4: Implement `store.rs`**

Rules:
- Every query that takes an app id also takes the workspace, **or** is only called after the handler has resolved the app in the workspace. Document which, per function.
- **`create`:** one transaction, `INSERT INTO analytics.apps … RETURNING …` then `INSERT INTO analytics.app_drafts (app_id, source, updated_by)`.
- **`save_draft`:**

```sql
UPDATE analytics.app_drafts SET source = $3, revision = revision + 1, updated_by = $2, updated_at = now()
 WHERE app_id = $1 AND revision = $4
RETURNING revision
```

  If no row comes back: `SELECT revision FROM analytics.app_drafts WHERE app_id = $1` gives `Stale{current_revision}`. Check `source.len() > MAX_SOURCE_BYTES` before the query and return `Error::InvalidInput("source exceeds 1 MiB")`.
- **`publish`:** one transaction.
  1. `SELECT source, revision FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE`. Locking the draft row serialises publishes of one app.
  2. If `expected` is set and differs → `Stale`.
  3. `INSERT INTO analytics.app_versions (app_id, version_no, source, from_revision, published_by) SELECT $1, COALESCE(MAX(version_no), 0) + 1, $2, $3, $4 FROM analytics.app_versions WHERE app_id = $1 RETURNING …`.
- **`restore_version`:** one transaction. Lock the draft row `FOR UPDATE` (the same serialisation point), read the old version scoped to the app, and insert `version_no = max + 1` with `restored_from` set and `from_revision` NULL.
- **`list` / `get`:**
  - join `app_drafts` for `draft_revision`;
  - use `(SELECT max(version_no) FROM analytics.app_versions v WHERE v.app_id = a.id)` for `latest_version_no`;
  - compute `bookmarked` with `EXISTS (SELECT 1 FROM analytics.bookmarks b WHERE b.user_id = $u AND b.entity_type = 'app' AND b.entity_id = a.id)`, copied from notebook `store.rs:155-182`;
  - order by `updated_at DESC`.
- **Bookmarks** use `entity_type = 'app'`, with the insert and delete copied from notebook `store.rs:313-334`.

- [ ] **Step 5: Run the tests until they pass**

Run: `cargo test -p egret-apps --test lifecycle_store` (schema and store sections). All pass, with real durations.

- [ ] **Step 6: Mutation checks, then commit**

Do these one at a time, confirm the named test fails, and restore:
1. Drop `AND revision = $4` from `save_draft` → `save_draft_bumps_revision_and_refuses_stale` fails.
2. Drop `FOR UPDATE` from `publish` → `concurrent_publishes_get_distinct_numbers` fails, at least some of the time. If it does not fail reliably, loop the join 20 times inside the test until it does, and record the result.
3. Drop the app scoping in `version` → `version_lookup_is_scoped_to_its_app` fails.

Then commit:

```bash
git add crates/egret-apps
git commit -m "feat(apps): AppStore — drafts with optimistic revisions, serialized publish, restore"
```

---

### Task 3: The cookie-session API (`/api/ws/{workspace}/apps…`)

**Files:**
- Create: `crates/egret-apps/src/routes.rs`
- Modify: `crates/egret-apps/src/lib.rs`, `crates/egret-apps/Cargo.toml` (add `egret-auth` as a normal dependency, `utoipa`, `chrono`)
- Test: `crates/egret-apps/tests/http.rs`

**Interfaces:**
- Consumes: `AppStore` and `template::new_app_source` from Task 2; `egret_workspace::WsContext`; `egret_core::authz::{require, require_object, ObjectRef}`; `egret_audit::{AuditSink, mutation_for}`.
- Produces:

```rust
#[derive(Clone)]
pub struct AppsApiState { pub store: AppStore, pub db: Db, pub sessions: SessionService,
                          pub workspaces: WorkspaceRepo, pub authz: Authorizer,
                          pub pdp: Arc<dyn Pdp>, pub audit: AuditSink }
// + FromRef impls for SessionService, WorkspaceRepo, Authorizer (as NotebookState)
pub fn apps_router() -> Router<AppsApiState>;
pub fn openapi() -> utoipa::openapi::OpenApi;   // in lib.rs, returns routes::AppsApi::openapi()
```

| Method & path (under `/ws/{workspace}`) | Permission | Response |
|---|---|---|
| `GET /apps?trashed=` | `app:read` (ws) | `[AppSummary]` |
| `POST /apps` `{name, description?, folder_id?, database?}` | `app:write` (ws) | 201 `AppDetail` |
| `GET /apps/{id}` | `app:read` (obj) | `AppDetail` |
| `PATCH /apps/{id}` `{name?, description?}` | `app:write` (obj) | `AppDetail` |
| `POST /apps/{id}/trash`, `POST /apps/{id}/restore` | `app:write` (obj) | 204 |
| `PATCH /apps/{id}/folder` `{folder_id}` | `app:write` (obj) | 204 |
| `POST /apps/{id}/bookmark`, `DELETE /apps/{id}/bookmark` | `app:read` (obj) | 204 |
| `GET /apps/{id}/draft` | `app:write` (obj) | `{source, revision}` + `ETag: "<rev>"` |
| `PUT /apps/{id}/draft` `{source}` + `If-Match` | `app:write` (obj) | `{revision}` + `ETag`; 428 / 409 `{current_revision}` / 413 |
| `POST /apps/{id}/publish` `{expected_revision?}` | `app:write` (obj) | 201 `VersionSummary`; 409 `{current_revision}` |
| `GET /apps/{id}/versions` | `app:write` (obj) | `[VersionSummary]` newest first |
| `GET /apps/{id}/versions/{version_id}` | `app:write` (obj) | `{…summary, source}` |
| `POST /apps/{id}/versions/{version_id}/restore` | `app:write` (obj) | 201 `VersionSummary` |

`AppSummary` and `AppDetail` carry `id, name, description, folder_id, owner_user_id, is_owner, updated_at, deleted_at, bookmarked, draft_revision, latest_version_no`. Name the OpenAPI component schemas with an `App` prefix (for example `AppSetFolder`), because component names are global after the merge.

- [ ] **Step 1: Write the failing route tests**

`crates/egret-apps/tests/http.rs` follows `crates/egret-notebook/tests/http.rs`:
- `#[sqlx::test(migrations = "../../migrations")]` is fine here and is the notebook precedent; export `DATABASE_URL` as required;
- real `CedarPdp::build`;
- members added with `MemberRepo::add`;
- sessions created with `SessionService::create`.

Mount it as production does: `Router::new().nest("/api", apps_router()).layer(from_fn(csrf_layer)).with_state(st)`. Mutating requests send `Cookie: egret_session=…; egret_csrf=c1` with `x-egret-csrf: c1`.

Write one test per row:

| # | Case | Expect |
|---|---|---|
| 1 | viewer: list, get | 200; create → 403; update → 403; draft GET → 403 |
| 2 | editor: create with `database` = an existing `query.databases` name | 201; the draft source contains `egret.connect("<name>")`; `draft_revision` 1 |
| 3 | create with a `database` that does not exist in the workspace | 404, and no app row |
| 4 | create with a `folder_id` of another kind, or of another workspace | 404 "folder" |
| 5 | get / update an app from another workspace (by id) | 404, same body as a random id |
| 6 | draft GET | `ETag: "1"` and `{revision:1}` |
| 7 | draft PUT without `If-Match` | 428 |
| 8 | draft PUT `If-Match: "1"` | 200 `{revision:2}`, `ETag: "2"`; again with `"1"` → 409 `{current_revision:2}` |
| 9 | draft PUT with source of 1 048 577 bytes | 413, and the revision is unchanged |
| 10 | publish | 201 v1 `from_revision:1`; publish `{expected_revision:1}` after a save to rev 2 → 409 `{current_revision:2}` |
| 11 | versions list newest first; version GET returns source; version of another app → 404 | as stated |
| 12 | restore v1 | 201 v(n+1) with v1's source, `restored_from` = v1 |
| 13 | trash → list (default) hides it; `?trashed=true` shows it; restore brings it back; get on a trashed app → 200 with `deleted_at` (as notebooks) | as stated |
| 14 | bookmark as viewer → list shows `bookmarked:true` for that viewer only | as stated |
| 15 | object share: a `ws_viewer` grant on one app to a user with **no** workspace role → that user can GET that app (200) but not list (403) or GET another app (404/403 — record which, matching notebooks) | proves the share flows into `require_object` |
| 16 | a mutating request without the CSRF header | 403 |
| 17 | audit | create / publish / restore each emit exactly one `Mutation` with `action` `app:create` / `app:publish` / `app:version_restore`; a draft save emits none |
| 18 | `apps` feature disabled for the workspace | every route → 403 FeatureDisabled (`WsContext` handles it) |

Case 15 needs the share route. Either mount the egret-bi `shares_router` in the same test router, or insert the `access_grants` row directly (the SQL `shares.rs` uses) and rebuild the PDP snapshot, as `routes_authz.rs:208-256` does.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p egret-apps --test http`
Expected: a compile failure, because `apps_router` does not exist yet.

- [ ] **Step 3: Implement `routes.rs`**

Follow `crates/egret-notebook/src/routes.rs`:
- an `app_object_ref(&st, &ctx, id) -> Result<ObjectRef>` modelled on `notebook_object_ref` (:129-148): 404 when the app is missing or in another workspace;
- `require` for list and create, and `require_object` with `const APP_READ / APP_WRITE` for everything else. These consts make `catalog_drift` see them; add `routes.rs` to `EXTRA_FILES`.

Endpoint details:
- **`If-Match`:** parse a quoted or bare integer. Anything else → 400.
- **428:** a small `IntoResponse` type, modelled on `TrialResultTooLarge` (`crates/egret-dataapi/src/try_it.rs:78-90`), with `StatusCode::PRECONDITION_REQUIRED`.
- **413:** the same pattern with `PAYLOAD_TOO_LARGE`. Check `source.len()` before calling the store.
- **409:** a JSON body `{error, current_revision}`, through a custom response type, so the client can reload. Do not use plain `Error::Conflict`, which has no field for the revision.
- **`ETag`:** set it on the GET and PUT responses.
- **Request body limit:** the draft PUT carries up to 1 MiB of source inside JSON. Axum's default 2 MB `Json` limit is enough. Verify the default applies in egret, add an explicit `DefaultBodyLimit::max(2 * 1024 * 1024)` on `apps_router` so it does not depend on defaults, and record it.
- **Audit:** use `egret_audit::mutation_for(&ctx.principal, Some(ws), "app:create", "app", Some(id), json!{…})` with this action set: `app:create`, `app:update`, `app:trash`, `app:restore`, `app:folder`, `app:publish`, `app:version_restore`. Put `version_no` and `from_revision` in `detail`. Never put source.
- **Create with `database`:** resolve `SELECT 1 FROM query.databases WHERE workspace_id = $1 AND name = $2` before calling the store, and return 404 if it is absent.
- **OpenAPI:** annotate every handler with `#[utoipa::path(…, tag = "apps")]`, collect them in `#[derive(OpenApi)] struct AppsApi`, and add `pub fn openapi()` in `lib.rs`.

- [ ] **Step 4: Run the tests until they pass**

Run: `cargo test -p egret-apps --test http`, then all of `cargo test -p egret-apps`, then `cargo test -p egret-controlplane --test catalog_drift`. All pass, with real durations.

- [ ] **Step 5: Mutation checks, then commit**

Do these one at a time, confirm the named case fails, and restore:
1. Change the draft GET permission to `APP_READ` → case 1 (viewer draft GET 403) fails.
2. Skip the `If-Match` presence check → case 7 fails.
3. Skip the workspace check in `app_object_ref` → case 5 fails.
4. Audit draft saves → case 17 fails.

Then commit:

```bash
git add crates/egret-apps crates/egret-controlplane/tests/catalog_drift.rs
git commit -m "feat(apps): workspace API — CRUD, revisioned drafts, publish, versions, restore"
```

---

### Task 4: Wiring, contract regeneration, frontend keys, and full local CI

**Files:**
- Modify: `crates/egret-server/src/lib.rs`, `crates/egret-server/Cargo.toml` (if needed), `crates/egret-bin/src/main.rs`, `openapi.json`, `frontend/src/api/generated/**`, `frontend/src/hooks/useWorkspaceFeatures.ts`, `frontend/src/api/folders.ts`, `frontend/src/components/sharing/ShareDialog.tsx`
- Test: `crates/egret-server/tests/apps_api_mounted.rs` (new), plus the existing `feature_route_coverage.rs`, plus frontend tests

- [ ] **Step 1: Mount the router with CSRF and merge OpenAPI**

- Nest `egret_apps::apps_router().with_state(apps_api)` under `/api` **with** `.layer(from_fn(egret_auth::csrf::csrf_layer))`, as the notebook group does (`lib.rs:360-366`).
- Keep the P1 gateway group as it is: separate and without CSRF.
- Add `doc.merge(egret_apps::openapi());` to `openapi_doc()` (:209-242) in the explicit list.

Write `apps_api_mounted.rs`:
- `GET /api/ws/x/apps` without a cookie → 401 (mounted, and the session is required);
- `POST` with a cookie but no CSRF header → 403 (CSRF applies).

Model it on `apps_gateway_mounted.rs`.

- [ ] **Step 2: Build `AppsApiState` in egret-bin**

Do it next to the P1 `AppsState`. Reuse the shared `SessionService`, `WorkspaceRepo`, `Authorizer`, PDP, audit sink and Db; take them from where `NotebookState` gets them.

- [ ] **Step 3: Regenerate the contract and client**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cd frontend && npm ci && npm run gen:api && cd ..
```

Expected: a new `frontend/src/api/generated/apps/apps.ts` plus model files. `cargo test -p egret-server committed_openapi_json_is_current` and `feature_route_coverage` pass.

- [ ] **Step 4: Frontend key lists (no UI)**

- `useWorkspaceFeatures.ts:19`: append `"apps"` to `ALL_FEATURE_KEYS`.
- `folders.ts:29-35`: add `"apps"` to `EntityKind`, plus the entries `listKeyOf`/`setFolderOf` need, using the generated client. If that needs real UI wiring, keep `EntityKind` only, record why, and leave the records to P4.
- `ShareDialog.tsx:33`: add `"app"` to `ShareKind`.

Update or extend any test that pins those lists.

- [ ] **Step 5: Full local CI, one suite at a time, after committing**

```bash
git add -A && git commit -m "feat(apps): mount the app API, regenerate the contract and client"
cargo fmt --all -- --check
cargo clippy -p egret-apps -p egret-bi -p egret-auth -p egret-core -p egret-server -p egret-bin --all-targets --no-deps -- -D warnings
cargo test --workspace --no-fail-fast        # with DATABASE_URL, EGRET_TEST_SHARED_STORE_URL, EGRET_TEST_STARROCKS_URL
cd frontend && npm run lint && npm run typecheck && npm test && npm run build && cd ..
git diff --exit-code frontend/src/api/generated openapi.json
```

Expected: everything green. Record the totals before and after (test counts) for both Rust and frontend. If the workspace clippy debt on main has been fixed by then, run `cargo clippy --workspace --all-targets -- -D warnings` instead. A failure is "pre-existing" only if you prove it; otherwise report it verbatim as unexplained.

- [ ] **Step 6: Commit any fixes**

```bash
git add -A && git commit -m "chore(apps): fmt, clippy and test fixes for the app API"
```

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| §5 `app_drafts` (≤ 1 MiB, revision), `app_versions` (append-only, unique version_no) | 1 |
| §5 draft API: ETag; PUT with If-Match → 409 | 2 (store), 3 (HTTP, plus 428 / 413) |
| §5 publish copies draft at revision; "Publish draft rev N" → `expected_revision` | 2, 3 |
| §5 restore = publish old source as a new version | 2, 3 |
| §5 soft delete; folders, trash and bookmarks reused | 2, 3 (trash and restore, folder guard, bookmarks) |
| §8 `app:manage`, shares via object-shares with kind `app` | 1 (kind, manage table, seed), 3 (case 15) |
| P1 carry-over: composite `version_id` FK | 1 |
| P1 carry-over: version exists and is published | 1, via the FK. Versions are never deleted except by cascade, and every version is published by definition. |
| P1 carry-over: `folder_id` FK | 1 |
| P1 carry-over: frontend `ALL_FEATURE_KEYS` | 4 |

Out of P2, by design:
- the Apps list and Edit UI (P4);
- run-mode resolution of "latest version" for viewers (P5);
- the pod-facing draft access used by `EgretStorage` (P4: the proxy and pod token decide how a pod authenticates to the draft API);
- hard delete;
- the P4 carry-overs: revoke by id or login session, sweeper, impersonator column, lockout-only audit.
