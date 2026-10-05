# Data Apps Storage: Move Draft/Version Content to S3/MinIO Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Data Apps' app source (`.py`) and grid-layout JSON out of `analytics.app_drafts`/`analytics.app_versions` text columns and into the shared S3/MinIO object store, proactively, before real usage makes the Postgres columns a measured problem — while keeping optimistic-concurrency semantics (the `revision` compare-and-swap) and the append-only guarantee on `app_versions` exactly as they are today.

**Architecture:** Reuse the object-store abstraction and config that already exist and are already shared by the catalog and the workflow engine — `egret_storage::ObjectStore` (one `S3Store`, one `[storage]` bucket/credential) and the `egret_workflow::storage::FileStore` pattern (a small struct holding the store handle, bucket, and a key prefix; keys computed deterministically from IDs already in Postgres, never stored as a separate pointer column). `AppStore` gains the same shape. Postgres keeps `revision`/`version_no`/timestamps/audit columns — the things concurrency control and listing actually need — and stops holding the bytes.

**Tech Stack:** Rust (`egret-apps`, `egret-config`, `egret-storage`, `egret-bin`), Postgres (`sqlx`), S3-compatible object storage (MinIO in dev, via `egret_storage::S3Store`), existing `docker-compose.dev.yml` MinIO service for local verification.

## Global Constraints

- **One object store, one `[storage]` config.** Do not add a second MinIO connection or a new top-level config section for Data Apps. `egret-workflow`'s own wiring comment says it plainly: "the SAME object store... one client, not a second one." Apps gets an `[apps.storage]` *prefix* section, exactly like `[workflow.storage]`, nothing else.
- **No new storage-abstraction crate or library (no OpenDAL).** `egret_storage::ObjectStore` already exists, is already shared by two subsystems, and already has everything this needs (`put`, `get`, `delete`, `head`). Introducing a second abstraction for the same job is explicitly out of scope — if a future need (a backend `egret_storage` cannot express) shows up, that is a separate decision, made then.
- **Keys are computed, never stored.** Follow `FileStore::object_uri`/`execution_prefix` exactly: a draft's or version's S3 key is a pure function of `workspace_id`, `app_id`, and (for versions) `version_no` — all already columns on existing rows. No new "S3 key" column, anywhere.
- **`MAX_SOURCE_BYTES` (1 MiB) does not change in this plan.** This plan is about *where* the bytes live, not about raising the cap now that S3 removes the practical reason for a small one. That is a separate, later decision.
- **Append-only stays append-only.** `analytics.app_versions`' `app_versions_append_only_trg` trigger is untouched; nothing in this plan updates or deletes a version row other than the one existing `published_by → NULL` exception.
- **Cutover order matters.** The read/write path must never switch to S3-only before every existing row's content has been copied there — a half-done cutover would make an existing app's draft silently appear empty. Part S2 (backfill) always ships and runs before Part S3 (cutover).
- **The column drop (Part S4) is irreversible** and gets its own migration, its own PR, and a plainly-stated warning in that PR's description — same discipline as every other irreversible migration in this codebase (see `migrations/0169_remove_notebook.sql`'s PR). It does not ship in the same PR as the cutover, so there is a real window to notice a problem before the old data is gone for good.
- **Tests use `egret_storage::MemStore`** (the `test-util` feature, already used by `egret-workflow`'s own tests via `install_mem_for_tests`), not a real MinIO, except for one live-verification pass against the dev stack's MinIO before each part ships.

---

# Part S1 — Wire the object store into `egret-apps` (additive, no behavior change)

Branch `feat/data-apps-storage-s1-wiring` off `main`. Nothing user-visible changes: `AppStore` gains the ability to read and write S3, but every existing code path still reads and writes the `source`/`layout_json` Postgres columns exactly as today. This part is pure plumbing, safe to ship on its own.

### Task S1-1: `[apps.storage]` config

**Files:**
- Modify: `crates/egret-config/src/lib.rs`
  - Add `AppsStorageConfig` (new struct), a `storage: AppsStorageConfig` field on `AppsConfig` (the existing struct at line ~206), a `default_apps_storage_prefix()` function, and a `validate()` method on `AppsStorageConfig` — all modeled directly on `WorkflowStorageConfig` (same file, ~line 801) and its `validate()`.
  - Wire the new validation into `Config::validate()` (existing method, ~line 1592 area): when `self.apps.pod.launcher != "none"`, require `[storage]` to be configured (same shape as the existing check "`catalog.enabled requires storage ...`" and "`workflow.container_runner other than none requires [storage] bucket`" — grep `requires [storage]` in that file to find the exact pattern and add a third arm).

**Interfaces:**
- Produces: `egret_config::AppsStorageConfig { prefix: String }` with `prefix` defaulting to `"apps/"`, and `AppsStorageConfig::validate(&self) -> Result<(), String>` checking the same two rules `WorkflowStorageConfig::validate` does (non-empty, ends with `/`, does not start with `/`).
- Consumes: nothing new; reuses the existing `[storage]` section (`Config.storage: StorageConfig`, already defined).

- [ ] **Step 1:** Add the struct and default function:

```rust
/// `[apps.storage]`: Data Apps' draft/version source and grid-layout JSON. The
/// bucket and its credentials are `[storage]`'s; this is the key prefix inside
/// it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppsStorageConfig {
    /// The key prefix: a draft's key is `{prefix}{workspace_id}/{app_id}/draft.py`
    /// (and `draft.layout.json`); a version's is
    /// `{prefix}{workspace_id}/{app_id}/versions/{version_no}/source.py`.
    /// Default `apps/`.
    #[serde(default = "default_apps_storage_prefix")]
    pub prefix: String,
}

fn default_apps_storage_prefix() -> String {
    "apps/".to_string()
}

impl Default for AppsStorageConfig {
    fn default() -> Self {
        Self {
            prefix: default_apps_storage_prefix(),
        }
    }
}

impl AppsStorageConfig {
    /// Same two rules as `WorkflowStorageConfig::validate`: `prefix` is a
    /// non-empty key prefix ending with `/` and not starting with one.
    pub fn validate(&self) -> Result<(), String> {
        let p = &self.prefix;
        if p.is_empty() || !p.ends_with('/') || p.starts_with('/') {
            return Err(format!(
                "apps.storage.prefix {p:?} must be non-empty, end with '/' and not start \
                 with '/' (e.g. \"apps/\"; EGRET_APPS__STORAGE__PREFIX)"
            ));
        }
        Ok(())
    }
}
```

- [ ] **Step 2:** Add `#[serde(default)] pub storage: AppsStorageConfig,` to `AppsConfig` (next to the existing `pod: AppPodConfig` field), and add `apps.storage.validate()?` next to wherever `AppsConfig`'s other fields are validated (if `AppsConfig` has no `validate()` of its own yet, call `self.apps.storage.validate().map_err(...)?` from `Config::validate()` directly, same place the `workflow.storage.validate()` call lives).
- [ ] **Step 3:** Add the "`[storage]` required" check. Find the existing workflow one:

```rust
grep -n "container_runner other than none requires" crates/egret-config/src/lib.rs
```

  and add a matching arm for apps, e.g.:

```rust
if self.apps.pod.launcher != "none"
    && (self.storage.bucket.is_empty()
        || self.storage.access_key.is_empty()
        || self.storage.secret_key.is_empty())
{
    return Err(
        "apps.pod.launcher other than \"none\" requires [storage] bucket, access_key, secret_key".into(),
    );
}
```

- [ ] **Step 4:** Write the test (mirror whichever existing test proves the workflow boot check, e.g. the one asserting `cfg.storage.bucket` emptiness fails validation when `container_runner != none` — same file, test module near line 1969):

```rust
#[test]
fn apps_pod_launcher_other_than_none_requires_storage() {
    let mut cfg = Config::default();
    cfg.apps.pod.launcher = "subprocess".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("[storage]"), "{err}");
}

#[test]
fn apps_storage_prefix_must_end_with_a_slash() {
    let cfg = AppsStorageConfig {
        prefix: "apps".into(),
    };
    assert!(cfg.validate().is_err());
}
```

- [ ] **Step 5:** Run: `cargo test -p egret-config apps_storage -- --nocapture` and `cargo test -p egret-config apps_pod_launcher`. Expected: both pass.
- [ ] **Step 6:** Commit `feat(config): add [apps.storage] prefix and its boot-time [storage] requirement`.

### Task S1-2: `AppStore` gains the object store and key helpers

**Files:**
- Modify: `crates/egret-apps/src/store.rs`

**Interfaces:**
- Consumes: `egret_storage::ObjectStore` trait (`put`, `get`, `delete`), `egret_config::AppsStorageConfig` (read by the caller, not by this crate — `egret-apps` does not depend on `egret-config`; `egret-bin` passes the already-resolved `bucket: String` and `prefix: String` strings in, exactly as it does for `egret_workflow::storage::FileStore`).
- Produces: `AppStore::new(db: Db, store: Arc<dyn ObjectStore>, bucket: String, prefix: String) -> Self` (signature change — every caller updates in Task S1-5/S1-6); `AppStore::draft_key(workspace: WorkspaceId, app: Uuid) -> String` and `AppStore::draft_layout_key(...)`, `AppStore::version_key(...)`, `AppStore::version_layout_key(...)` (private or `pub(crate)`, whichever `routes.rs`/`podapi.rs` need — they do not, since they only ever go through `AppStore`'s own methods).

- [ ] **Step 1:** Add the fields and key helpers. Mirror `FileStore::object_uri` precisely — same `s3://{bucket}/{prefix}{workspace}/...` shape:

```rust
use std::sync::Arc;

#[derive(Clone)]
pub struct AppStore {
    db: Db,
    store: Arc<dyn egret_storage::ObjectStore>,
    bucket: String,
    prefix: String,
}

impl AppStore {
    pub fn new(db: Db, store: Arc<dyn egret_storage::ObjectStore>, bucket: String, prefix: String) -> Self {
        Self { db, store, bucket, prefix }
    }

    fn draft_key(&self, ws: WorkspaceId, app: Uuid) -> String {
        format!("s3://{}/{}{}/{}/draft.py", self.bucket, self.prefix, ws.0.hyphenated(), app.hyphenated())
    }

    fn draft_layout_key(&self, ws: WorkspaceId, app: Uuid) -> String {
        format!("s3://{}/{}{}/{}/draft.layout.json", self.bucket, self.prefix, ws.0.hyphenated(), app.hyphenated())
    }

    fn version_key(&self, ws: WorkspaceId, app: Uuid, version_no: i32) -> String {
        format!(
            "s3://{}/{}{}/{}/versions/{}/source.py",
            self.bucket, self.prefix, ws.0.hyphenated(), app.hyphenated(), version_no
        )
    }

    fn version_layout_key(&self, ws: WorkspaceId, app: Uuid, version_no: i32) -> String {
        format!(
            "s3://{}/{}{}/{}/versions/{}/layout.json",
            self.bucket, self.prefix, ws.0.hyphenated(), app.hyphenated(), version_no
        )
    }
}
```

  Note `draft_key`/`draft_layout_key` take `WorkspaceId` — every current `AppStore` method that touches a draft (`draft`, `save_draft`) is `app_id`-only today and relies on the caller having already resolved the workspace (per the module's own "scoping rule" doc comment). **This is a real signature change these methods need**: they must gain a `ws: WorkspaceId` parameter to compute the key. Carry that through every call site in Task S1-3/S1-4 and update every caller in `routes.rs`/`podapi.rs` (both already have `ctx.workspace_id` or an equivalent in scope at every call site — confirm this while editing, it is expected to be a mechanical addition, not a new lookup).

- [ ] **Step 2:** Commit nothing yet — this task has no behavior change on its own and is easiest reviewed together with S1-3. Leave uncommitted (or commit as `wip` and squash later); proceed directly to S1-3.

### Task S1-3: `draft()`/`save_draft()` read and write S3, additively

Postgres's `source`/`layout_json` columns are written too, in the same transaction, so a rollback mid-rollout (reverting to the pre-S1 binary) still works. They stop being *read* here, which is the real behavior change this task makes, done carefully: tests must prove the content now actually round-trips through S3, not through Postgres.

**Files:**
- Modify: `crates/egret-apps/src/store.rs`
- Modify: `crates/egret-apps/src/routes.rs`, `crates/egret-apps/src/podapi.rs` (both call `draft`/`save_draft` — add the now-required `ws` argument)

**Interfaces:**
- Consumes: Task S1-2's key helpers.
- Produces: `AppStore::draft(&self, ws: WorkspaceId, id: Uuid) -> Result<Option<Draft>>` and `AppStore::save_draft(&self, ws: WorkspaceId, id: Uuid, by: UserId, expected: i64, source: &str, layout: Option<&str>) -> Result<SaveOutcome>` (both gain `ws: WorkspaceId` as the second parameter).

- [ ] **Step 1:** Write the failing test first (`crates/egret-apps/tests/lifecycle_store.rs`, a new test near `save_draft_bumps_revision_and_refuses_stale`):

```rust
#[tokio::test]
async fn draft_content_round_trips_through_the_object_store_not_postgres() {
    let fx = Fixture::new().await; // Task S1-6 gives Fixture a MemStore-backed AppStore
    let app = fx.store_app("hello").await;
    let st = fx.store();
    st.save_draft(fx.ws(), app, fx.uid(), 1, "x = 2", None).await.unwrap();

    // The content is NOT in the row's source column (it's unused going forward) --
    // prove it came from the object store by reading the key directly.
    let key = format!("s3://{}/apps/{}/{}/draft.py", fx.bucket(), fx.ws().0.hyphenated(), app.hyphenated());
    let bytes = fx.mem_store().get(&key).await.unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "x = 2");

    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!(d.source, "x = 2");
}
```

- [ ] **Step 2:** Run it: `EGRET_ALLOW_DB_SKIP=1 cargo test -p egret-apps --test lifecycle_store draft_content_round_trips -- --nocapture` (fails: `Fixture`/`AppStore` do not have the new signature/helpers yet).
- [ ] **Step 3:** Implement. `draft()` becomes: one `SELECT revision, updated_at FROM analytics.app_drafts WHERE app_id = $1` (no `source`/`layout_json` in the select list), then `self.store.get(&self.draft_key(ws, id))` for the source and `self.store.get(&self.draft_layout_key(ws, id))` for the layout (treat `StorageError::NotFound`-shaped errors, or whatever `S3Store`/`MemStore` return for a missing key — check `egret_storage::StorageError`'s variants — as "no layout", matching today's `Option<String>` semantics; a missing *draft* key with an existing `app_drafts` row is a genuine error, not "no draft", and should propagate). `save_draft()` becomes: write `source` to S3 first (fail fast, nothing written to Postgres, if the PUT fails), write `layout` to S3 (or skip/delete the key if `layout` is `None` — reuse `ObjectStore::delete`, treating "key already absent" as success), *then* run the existing `UPDATE ... SET revision = revision + 1 ... RETURNING revision` (now with `source`/`layout_json` still bound too, kept for the rollback safety net this task's intro describes).
- [ ] **Step 4:** Update `routes.rs`'s `draft_get`/`draft_put` and `podapi.rs`'s `draft_get`/`draft_put` to pass `ctx.workspace_id` (routes.rs) / the pod principal's `p.workspace_id` (podapi.rs) as the new `ws` argument.
- [ ] **Step 5:** Run the test from Step 1 again. Expected: passes.
- [ ] **Step 6:** Run the full existing suite to confirm nothing else broke: `EGRET_ALLOW_DB_SKIP=1 cargo test -p egret-apps --lib` and (against a real database per this session's established recipe) `cargo test -p egret-apps`.
- [ ] **Step 7:** Commit `feat(apps): read/write draft source and layout through the object store`.

### Task S1-4: `publish()`/`restore_version()`/`version()` do the same for `app_versions`

**Files:**
- Modify: `crates/egret-apps/src/store.rs`
- Modify: `crates/egret-apps/src/podapi.rs` (`version_get`)

**Interfaces:**
- Produces: `AppStore::publish(&self, ws: WorkspaceId, id: Uuid, by: UserId, expected: Option<i64>) -> Result<PublishOutcome>`, `AppStore::version(&self, ws: WorkspaceId, id: Uuid, version_id: Uuid) -> Result<Option<Version>>`, `AppStore::restore_version(&self, ws: WorkspaceId, id: Uuid, version_id: Uuid, by: UserId) -> Result<Option<VersionSummary>>` (all gain `ws: WorkspaceId`); internal `insert_version` gains a `store`/`bucket`/`prefix` (or just takes the already-computed keys) so it can write to S3 before the `INSERT`.

- [ ] **Step 1:** Failing test (`lifecycle_store.rs`):

```rust
#[tokio::test]
async fn publishing_writes_the_versions_content_to_the_object_store() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let v = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());

    let key = format!(
        "s3://{}/apps/{}/{}/versions/{}/source.py",
        fx.bucket(), fx.ws().0.hyphenated(), app.hyphenated(), v.summary.version_no
    );
    assert_eq!(fx.mem_store().get(&key).await.unwrap(), b"v1");
}

#[tokio::test]
async fn restoring_a_version_copies_its_object_server_side_without_reading_it_into_the_app() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let v1 = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
    st.save_draft(fx.ws(), app, fx.uid(), 1, "v2", None).await.unwrap();
    published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());

    let v3 = st
        .restore_version(fx.ws(), app, v1.id, fx.uid())
        .await
        .unwrap()
        .unwrap();

    let key = |no: i32| {
        format!(
            "s3://{}/apps/{}/{}/versions/{}/source.py",
            fx.bucket(), fx.ws().0.hyphenated(), app.hyphenated(), no
        )
    };
    assert_eq!(fx.mem_store().get(&key(v3.version_no)).await.unwrap(), b"v1");
    // This test proves the CONTENT is right. It does not prove restore_version
    // used `copy()` rather than `get()`+`put()` -- `egret_storage::MemStore`
    // (checked during this plan's research: crates/egret-storage/src/mem.rs)
    // has no call-tracking today, and adding one is a separate, wider-blast-
    // radius change to a shared test-util crate this task should not casually
    // make. Confirm the `copy()` call site by reading the diff in review
    // instead of asserting it here.
}
```

- [ ] **Step 2:** Run it (fails — same reason as S1-3).
- [ ] **Step 3:** Implement: `publish()` reads the draft's *current* S3 content (not a Postgres `source` column — by this point in the rollout `draft()`'s S3 read is already the source of truth from S1-3) and writes it to the version's key before `insert_version`'s `INSERT`.

  `restore_version()` does **not** read the old version's bytes into the application and write them back out — that would pull potentially-large content through `egret-bin` for no reason. Versions are immutable and each already has its own key, so "restoring" is a pure copy: use `ObjectStore::copy(&self, src, dst)` (already on the trait, already used by the catalog's own file-manager move/rename per its doc comment) to copy `versions/{old_version_no}/source.py` → `versions/{new_version_no}/source.py` server-side, and the same for `layout.json` *only if the old version has one* (check with `head()` first, or let a missing-source `copy` error be treated as "no layout to copy" the same way a missing layout key is treated elsewhere in this plan — pick whichever `MemStore`/`S3Store` actually do for a `copy` of a nonexistent source and match it, confirmed in Task S1-4's own test).

  `version()`'s `SELECT` drops `source`/`layout_json` from the column list and reads both from S3 after.
- [ ] **Step 4:** Update `podapi.rs`'s `version_get` to pass `p.workspace_id`.
- [ ] **Step 5:** Run the Step 1 test (passes) and the full suite again.
- [ ] **Step 6:** Commit `feat(apps): publish/restore_version read and write the object store too`.

### Task S1-5: Wire `egret-bin`

**Files:**
- Modify: `crates/egret-bin/src/main.rs` (the two `AppStore::new` call sites, ~line 2287 and ~2304)

**Interfaces:**
- Consumes: `engine_state.storage` (the ALREADY-CONSTRUCTED `Arc<dyn ObjectStore>` — the same `S3Store` workflow reuses; do not build a second one), `config.storage.bucket`, `config.apps.storage.prefix`.

- [ ] **Step 1:** Change both `AppStore::new(db.clone())` calls to `AppStore::new(db.clone(), engine_state.storage.clone(), config.storage.bucket.clone(), config.apps.storage.prefix.clone())` — same pattern as the workflow `FileStore` construction a few lines below in the same function, which already demonstrates reusing `engine_state.storage.clone()` with a one-line comment explaining why ("the SAME object store... one client, not a second one"); copy that comment's spirit onto this call site too.
- [ ] **Step 2:** `cargo check -p egret-bin`. Expected: compiles (this task is the one that makes S1-2 through S1-4's signature changes finally compile end to end).
- [ ] **Step 3:** Commit `feat(apps): wire AppStore to the shared object store in egret-bin`.

### Task S1-6: Test fixtures move to `MemStore`

**Files:**
- Modify: `crates/egret-apps/tests/{lifecycle_store,http,podapi,proxy}.rs` (every `Fixture` that constructs `AppStore::new`, per the grep done during this plan's research: `lifecycle_store.rs:318`, `proxy.rs:1159`, `podapi.rs:81`, `http.rs:70,448,1376`)

**Interfaces:**
- Consumes: `egret_storage::MemStore::new()` (already exported under the `test-util` feature, already a dev-dependency pattern egret-workflow's tests use — confirm `egret-apps/Cargo.toml` has `egret-storage` as a dev-dependency with `features = ["test-util"]`, add it if not).

- [ ] **Step 1:** Add (if missing) to `crates/egret-apps/Cargo.toml`:

```toml
[dev-dependencies]
egret-storage = { workspace = true, features = ["test-util"] }
```

- [ ] **Step 2:** In each `Fixture`, replace `AppStore::new(db.clone())` with `AppStore::new(db.clone(), Arc::new(egret_storage::MemStore::new()), "test-bucket".to_string(), "apps/".to_string())`, and add `fn mem_store(&self) -> Arc<MemStore>` / `fn bucket(&self) -> &str` / `fn ws(&self) -> WorkspaceId` accessors to whichever `Fixture`s the new S1-3/S1-4 tests need them on (`lifecycle_store.rs` at minimum — check `MemStore::get`'s exact signature first; it may need to be the `ObjectStore` trait object, in which case store `Arc<dyn ObjectStore>` on the fixture and downcast, or simplest: keep a second, concretely-typed `Arc<MemStore>` handle on the fixture alongside the `Arc<dyn ObjectStore>` passed into `AppStore::new`, since the test needs to call `MemStore`'s own inspection methods, not just the trait).
- [ ] **Step 3:** `cargo test -p egret-apps --no-run`. Expected: compiles.
- [ ] **Step 4:** Run the full suite (`EGRET_ALLOW_DB_SKIP=1 cargo test -p egret-apps --lib` plus the DB-backed suite per this session's established recipe). Expected: all green, including S1-3/S1-4's new tests.
- [ ] **Step 5:** Commit `test(apps): point every AppStore fixture at MemStore instead of a real object store`.

### Task S1-7: One live verification against the dev stack's real MinIO

Not a code change — a manual check before the PR, same discipline as every other "verify against the real thing" step this session has used.

- [ ] **Step 1:** `just up` (starts the dev stack's MinIO at `egret-dev-minio-1`, already confirmed present in `deploy/docker-compose.dev.yml`).
- [ ] **Step 2:** Point a local `egret.toml`/env at it (`[storage]` section matching the dev MinIO's credentials — check `deploy/docker-compose.dev.yml`'s MinIO service env for the exact access/secret key and bucket name already used by the catalog/workflow dev setup, reuse them) and `apps.pod.launcher = "subprocess"` (or whatever makes `[storage]` required per Task S1-1's new check).
- [ ] **Step 3:** Through the running `egret serve`, create an app, save a draft, publish it. Use `mc` (MinIO client) or the AWS CLI against the MinIO endpoint to confirm the objects exist at the expected keys.
- [ ] **Step 4:** Record the result (pass/fail, what was checked) in the PR description — same honesty standard as every `*-live-result.md` doc this project has produced for Data Apps so far.

### Self-review for Part S1

- Every `AppStore` constructor call site updated: `egret-bin` (2), every test `Fixture` (≥4 files). ✓ covered (S1-5, S1-6).
- `routes.rs`/`podapi.rs` callers of the now-`ws`-taking methods updated. ✓ covered (S1-3 step 4, S1-4 step 4).
- No new "S3 key" column anywhere — keys are computed. ✓ (Task S1-2).
- Postgres `source`/`layout_json` still written (not yet dropped) so this part alone is safely revertible. ✓ (Task S1-3 step 3's explicit design).

---

# Part S2 — Backfill existing rows into the object store

Branch `feat/data-apps-storage-s2-backfill` off `main` after S1 merges. A new one-off `egret` subcommand, following the existing `Command::{Serve,Migrate,Init,Workspace}` pattern in `egret-bin`. Idempotent (safe to run more than once) — it always overwrites the S3 key with the row's current Postgres content rather than skip-if-exists, so a rerun after an interrupted first run cannot leave a stale object behind.

### Task S2-1: `egret backfill-apps-storage`

**Files:**
- Modify: `crates/egret-bin/src/main.rs` (add `Command::BackfillAppsStorage` and its handler, next to `Command::Migrate`'s handler)

**Interfaces:**
- Consumes: the same `AppStore`/`S3Store` construction as `serve` already does (S1-5); a raw `sqlx` query over `analytics.app_drafts` and `analytics.app_versions` (joined to `analytics.apps` for `workspace_id`, since neither table carries it directly — confirm this against the actual schema before writing the query; `analytics.apps` is the parent row both reference via `app_id`).

- [ ] **Step 1:** Add the subcommand:

```rust
Command::BackfillAppsStorage => backfill_apps_storage(config).await,
```

  and, mirroring `migrate()`'s shape:

```rust
async fn backfill_apps_storage(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections).await?;
    let storage_cfg = egret_storage::StorageConfig { /* same four fields as the `serve` block */ };
    let store: Arc<dyn egret_storage::ObjectStore> = Arc::new(egret_storage::S3Store::new(&storage_cfg).await?);
    let app_store = egret_apps::store::AppStore::new(db.clone(), store, config.storage.bucket.clone(), config.apps.storage.prefix.clone());

    let drafts = sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>)>(
        "SELECT a.workspace_id, d.app_id, d.source, d.layout_json \
           FROM analytics.app_drafts d JOIN analytics.apps a ON a.id = d.app_id",
    )
    .fetch_all(&db.pool)
    .await?;
    let mut n = 0u64;
    for (ws, app_id, source, layout) in drafts {
        app_store.backfill_draft(WorkspaceId(ws), app_id, &source, layout.as_deref()).await?;
        n += 1;
    }
    tracing::info!(count = n, "drafts backfilled");

    let versions = sqlx::query_as::<_, (Uuid, Uuid, i32, String, Option<String>)>(
        "SELECT a.workspace_id, v.app_id, v.version_no, v.source, v.layout_json \
           FROM analytics.app_versions v JOIN analytics.apps a ON a.id = v.app_id",
    )
    .fetch_all(&db.pool)
    .await?;
    let mut m = 0u64;
    for (ws, app_id, version_no, source, layout) in versions {
        app_store.backfill_version(WorkspaceId(ws), app_id, version_no, &source, layout.as_deref()).await?;
        m += 1;
    }
    tracing::info!(count = m, "versions backfilled");
    Ok(())
}
```

  (`backfill_draft`/`backfill_version` are two new, small `pub` methods on `AppStore` that do exactly the S3 writes `save_draft`/`insert_version` already do, factored out so both the live-save path and the backfill tool call the same code — do not duplicate the key-format logic.)

- [ ] **Step 2:** Add `AppStore::backfill_draft`/`backfill_version` (pure S3 writes, no Postgres touch — the row's Postgres state is already correct, only S3 is catching up).
- [ ] **Step 3:** Test (`crates/egret-apps/tests/lifecycle_store.rs` or a new `backfill.rs`): seed a draft and a published version directly via SQL (bypassing `AppStore`, simulating "data from before S1 shipped"), call `backfill_draft`/`backfill_version`, assert the S3 keys now hold the right bytes.
- [ ] **Step 4:** Run it. Expected: passes.
- [ ] **Step 5:** Manual run against the dev stack (same MinIO as S1-7): seed a couple of apps/versions, run `cargo run -p egret-bin -- backfill-apps-storage`, confirm via `mc`/AWS CLI that objects now exist for rows that predate this plan.
- [ ] **Step 6:** Commit `feat(bin): add the one-off apps-storage backfill command`.

---

# Part S3 — Cut over: stop writing (and reading) the Postgres columns

Branch `feat/data-apps-storage-s3-cutover` off `main` after S2 has actually been run against every environment that has real Data Apps content (there is currently believed to be none in production — confirm this is still true before merging, the same diligence this session already applied before P6e). This part removes the "write both" safety net Task S1-3 deliberately kept: `save_draft`/`publish`/`restore_version` now write ONLY to S3.

### Task S3-1: Stop writing `source`/`layout_json`

**Files:**
- Modify: `crates/egret-apps/src/store.rs` (`save_draft`, `insert_version`)

- [ ] **Step 1:** Remove `source`/`layout_json` from the `UPDATE`/`INSERT` statements in `save_draft` and `insert_version` (they still exist as columns — Part S4 drops them — but nothing writes to them from here on).
- [ ] **Step 2:** Run the full suite (unit + DB-backed). Expected: unaffected — nothing was reading these columns since Part S1.
- [ ] **Step 3:** Commit `feat(apps): stop writing the now-unused source/layout_json columns`.

### Self-review for Part S3

- Confirmed Part S2's backfill has actually run against every real environment before this merges — this is a process check, not a code one; state it plainly in the PR description, same as every other irreversible-adjacent step this session has handled.

---

# Part S4 — Drop the columns

Branch `feat/data-apps-storage-s4-drop-columns` off `main`, **separately, after a confidence period following S3** (this session has no fixed number of days to recommend — that is a product/ops call, not an engineering one; note it as an open question in the PR rather than guessing).

### Task S4-1: The migration

**Files:**
- Create: `migrations/0171_drop_app_draft_source_columns.sql` (confirm 0171 is still free — `ls migrations/ | tail -3` — before writing; this plan was written when 0170 was the latest)

- [ ] **Step 1:** Write the migration:

```sql
-- Data Apps: `source` and `layout_json` moved to the shared object store
-- (`[storage]`, `apps.storage.prefix`) in Parts S1-S3 of
-- docs/en/plans/2026-10-05-data-apps-storage-s3.md. This is IRREVERSIBLE: any
-- row whose content was never backfilled (Part S2's `egret
-- backfill-apps-storage`) loses that content for good. Confirmed before this
-- migration was written to run: every environment with real Data Apps
-- content has had the backfill run against it.
ALTER TABLE analytics.app_drafts
  DROP COLUMN source,
  DROP COLUMN layout_json;

ALTER TABLE analytics.app_versions
  DROP COLUMN source,
  DROP COLUMN layout_json;
```

- [ ] **Step 2:** Remove the now-dead `Draft`/`Version`/`VersionSummary` `FromRow` mappings' reliance on these columns if anything still references them defensively (by Part S3 nothing should).
- [ ] **Step 3:** Verify against a disposable test database — same recipe this session used for `migrations/0169` and `migrations/0170`: create a scratch DB, run every migration including this one, confirm the columns are gone and every other `egret-apps` test still passes against it.
- [ ] **Step 4:** Commit `feat(db): drop the now-unused app_drafts/app_versions source columns`.

### Task S4-2: PR

- [ ] State plainly in the PR description: this is the irreversible step; confirm (again, at merge time, not just at write time) that every real environment has been backfilled.

---

## Self-review (whole plan)

**Spec coverage** — the three questions from the brainstorming discussion this plan follows up on:
1. *An S3 option for heavy content* — Parts S1-S4 deliver exactly this, with the old Postgres storage fully retired at the end rather than kept as a permanent dual path (simpler than a toggle, and nothing in the discussion asked for both to coexist long-term).
2. *Shared vs. per-service MinIO* — resolved by construction: Task S1-1/S1-5 reuse `[storage]` and `engine_state.storage` exactly as `egret-workflow` already does; no new MinIO config exists anywhere in this plan.
3. *A common interface (OpenDAL?)* — resolved by construction: every task reuses `egret_storage::ObjectStore`; no new abstraction is introduced.

**Placeholder scan** — no "TBD"/"add error handling" left in; the one genuinely unresolved item (how long a confidence period before Part S4) is called out explicitly as a non-engineering decision rather than hidden in a vague step.

**Type consistency** — `AppStore::new`'s signature (`db, store, bucket, prefix`) is the same across every task that constructs it (S1-2's definition, S1-5's `egret-bin` call sites, S1-6's test fixtures, S2-1's backfill command). Every method that gained `ws: WorkspaceId` (`draft`, `save_draft`, `publish`, `version`, `restore_version`) is listed consistently in S1-3's and S1-4's Interfaces blocks and used that way in every Step that calls them.

---

**Plan complete and saved to `docs/en/plans/2026-10-05-data-apps-storage-s3.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
