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

### Task 1 (S1-1): `[apps.storage]` config

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

### Task 2 (S1-2): `AppStore` gains the object store and key helpers

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

### Task 3 (S1-3): `draft()`/`save_draft()` read and write S3, additively

Postgres's `source`/`layout_json` columns are written too, in the same transaction, so a rollback mid-rollout (reverting to the pre-S1 binary) still works.

**Reads prefer S3, with a Postgres fallback — not S3-only.** (Correction after Task 2+3's task review caught a self-contradiction in an earlier draft of this plan: a literal "reads exclusively from S3" would 500 on every app created before this part ships, until Part 8/S2's backfill runs — directly violating this Part's own opening claim, two paragraphs up, that it is "safe to ship on its own." The human ruling, recorded in this plan's SDD ledger, is: add the fallback.) `draft()` tries the object store first; on `StorageError::NotFound` for the *source* key specifically, fall back to the row's `source` column (which Task 3's dual-write keeps populated for every row, old and new, so it is always a valid answer). The *layout* key is allowed to be `NotFound` on its own merits already (it means "no layout saved") — for a PRE-S1 row this is indistinguishable from "backfill hasn't run yet," so fall back to the row's `layout_json` column whenever the row itself predates this code (in practice: just always read the Postgres `layout_json` as the fallback value for a layout-key miss, the same as the source fallback, rather than trying to distinguish the two cases). This fallback is temporary scaffolding, deliberately: Task 10 (S3-2) removes it once Task 8 (S2)'s backfill is confirmed to have run everywhere, at which point `NotFound` for either key is restored to meaning exactly what it already means for versions — a real error for source, "no layout" for layout. (Task 10 also closes finding I3, since removing this draft fallback is only safe once draft content is keyed by revision — see Task 10's own description.)

Tests must prove the content now actually round-trips through S3 for an app this code created (not through Postgres), AND that a Postgres-only row (simulating pre-S1 data) still reads correctly through the fallback.

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

#[tokio::test]
async fn a_pre_s1_row_with_no_object_store_content_falls_back_to_postgres() {
    let fx = Fixture::new().await;
    // Simulate data from before this code shipped: insert directly via SQL
    // (NOT through `AppStore::create`, which writes the object store too as
    // of this task) so the row has real Postgres content and nothing in the
    // object store for it at all.
    let app_id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.apps (workspace_id, name, description) VALUES ($1, 'legacy', '') RETURNING id",
    )
    .bind(fx.ws().0)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO analytics.app_drafts (app_id, source) VALUES ($1, 'legacy')")
        .bind(app_id)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    let d = fx.store().draft(fx.ws(), app_id).await.unwrap().unwrap();
    assert_eq!(d.source, "legacy");
}

#[tokio::test]
async fn a_losing_concurrent_save_never_becomes_the_visible_draft() {
    // Regression test for the race the task review caught: two savers both
    // read revision 1, both attempt to save: whichever loses the Postgres
    // compare-and-swap must NOT have its content end up in the object store
    // under the winner's revision. Use a store wrapper that delays `put` so
    // the race window is wide enough to hit reliably (an instant MemStore
    // will not reproduce it) -- see the task brief/report for the wrapper
    // shape chosen.
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let (a, b) = tokio::join!(
        st.save_draft(fx.ws(), app, fx.uid(), 1, "a", None),
        st.save_draft(fx.ws(), app, fx.uid(), 1, "b", None),
    );
    let winner = match (a, b) {
        (Ok(SaveOutcome::Saved { .. }), Err(_)) | (Ok(SaveOutcome::Saved { .. }), Ok(SaveOutcome::Stale { .. })) => "a",
        (Err(_), Ok(SaveOutcome::Saved { .. })) | (Ok(SaveOutcome::Stale { .. }), Ok(SaveOutcome::Saved { .. })) => "b",
        other => panic!("expected exactly one Saved and one Stale, got {other:?}"),
    };
    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!(d.source, winner, "the CAS loser's content must never become the visible draft");
}
```

- [ ] **Step 2:** Run the three tests: `EGRET_ALLOW_DB_SKIP=1 cargo test -p egret-apps --test lifecycle_store draft_content_round_trips -- --nocapture`, `... a_pre_s1_row_with_no_object_store_content_falls_back_to_postgres ...`, `... a_losing_concurrent_save_never_becomes_the_visible_draft ...` (all three fail: `Fixture`/`AppStore` do not have the new signature/helpers/fallback/locking yet).
- [ ] **Step 3:** Implement `draft()`: one `SELECT source, layout_json, revision, updated_at FROM analytics.app_drafts WHERE app_id = $1` (the Postgres columns are STILL selected — they are the fallback, not dead weight, until Task 9 removes this), then try `self.store.get(&self.draft_key(ws, id))` for the source — on `Ok`, use it; on `Err(StorageError::NotFound(_))`, use the row's `source` column instead (the fallback); any other `Err` propagates. Same shape for the layout: try `self.store.get(&self.draft_layout_key(ws, id))` — `Ok` wins, `NotFound` falls back to the row's `layout_json` column (which is already `Option<String>`, so "fell back and it's `None`" and "no layout, full stop" collapse into the same value, correctly), any other `Err` propagates.

  Implement `save_draft()` **inside one transaction, holding the draft row locked across the object-store writes** — mirror `publish()`'s existing `SELECT ... FOR UPDATE` on `analytics.app_drafts` (same table, read the current code before writing this) rather than the brief's originally-described "check, then write S3, then separately UPDATE": `SELECT revision FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE`, compare to `expected` and return `SaveOutcome::Stale` (after releasing the transaction) if it doesn't match, otherwise — still holding the lock — write `source` to S3, write-or-delete the layout key (`None` deletes it; `ObjectStore::delete` on an absent key is already a no-op success in both `MemStore` and `S3Store`, confirmed during this plan's task review), then run the `UPDATE ... SET revision = revision + 1, source = $3, layout_json = $5, ... RETURNING revision` against the SAME row (it cannot have changed underneath the lock, so this UPDATE is now a formality, not a second compare-and-swap) and commit. A second saver blocks on the row lock for the lock-holder's full duration (including its S3 round-trip), then sees the new revision and returns `Stale` without ever touching the object store — this is what closes the race, not a pre-check read outside a lock.
- [ ] **Step 4:** Update `routes.rs`'s `draft_get`/`draft_put` and `podapi.rs`'s `draft_get`/`draft_put` to pass `ctx.workspace_id` (routes.rs) / the pod principal's `p.workspace_id` (podapi.rs) as the new `ws` argument.
- [ ] **Step 5:** Run all three tests from Step 1 again. Expected: all pass.
- [ ] **Step 6:** Run the full existing suite to confirm nothing else broke: `EGRET_ALLOW_DB_SKIP=1 cargo test -p egret-apps --lib` and (against a real database per this session's established recipe) `cargo test -p egret-apps`.
- [ ] **Step 7:** Commit `feat(apps): read/write draft source and layout through the object store, with a Postgres fallback and a locked compare-and-swap`.

### Task 4 (S1-4): `publish()`/`restore_version()`/`version()` do the same for `app_versions`

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

#[tokio::test]
async fn a_pre_s1_version_with_no_object_store_content_falls_back_to_postgres() {
    // Same fallback as Task 3's draft() -- a version published before this
    // code shipped has real content in `analytics.app_versions.source` and
    // nothing in the object store.
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let (version_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO analytics.app_versions (app_id, version_no, source) \
         VALUES ($1, 1, 'legacy v1') RETURNING id",
    )
    .bind(app)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();

    let v = fx.store().version(fx.ws(), app, version_id).await.unwrap().unwrap();
    assert_eq!(v.source, "legacy v1");
}
```

- [ ] **Step 2:** Run it (fails — same reason as Task 3).
- [ ] **Step 3:** Implement: `publish()` reads the draft's *current* content via `draft()` (which already does S3-with-Postgres-fallback as of Task 3 — `publish()` does not need its own fallback logic for the SOURCE it is publishing, only `version()`'s own read of an EXISTING version needs one, see below) and writes it to the version's key before `insert_version`'s `INSERT`.

  `restore_version()` does **not** read the old version's bytes into the application and write them back out — that would pull potentially-large content through `egret-bin` for no reason. Versions are immutable and each already has its own key, so "restoring" is a pure copy: use `ObjectStore::copy(&self, src, dst)` (already on the trait, already used by the catalog's own file-manager move/rename per its doc comment) to copy `versions/{old_version_no}/source.py` → `versions/{new_version_no}/source.py` server-side, and the same for `layout.json` *only if the old version has one* (check with `head()` first, or let a missing-source `copy` error be treated as "no layout to copy" the same way a missing layout key is treated elsewhere in this plan — pick whichever `MemStore`/`S3Store` actually do for a `copy` of a nonexistent source and match it, confirmed in Task 4's own test). **If the OLD version predates this code (no object-store key at all — see the fallback test above), `copy()` will fail with `NotFound`: in that one case, fall back to reading the old version's `source`/`layout_json` straight from its Postgres row and `put()`-ing them to the new version's key instead of copying** (this is the one place a GET+PUT through the app is unavoidable, and only for the one-time migration of a version that predates this rollout).

  `version()`'s `SELECT` keeps `source`/`layout_json` in the column list (same reasoning as Task 3's `draft()`: they are the fallback, not dead weight, until Task 9 removes this) and tries the object store first; `StorageError::NotFound` on either key falls back to the row's column, exactly mirroring Task 3's `draft()`.
- [ ] **Step 4:** Update `podapi.rs`'s `version_get` to pass `p.workspace_id`.
- [ ] **Step 5:** Run all tests from Step 1 again (passes) and the full suite.
- [ ] **Step 6:** Commit `feat(apps): publish/restore_version read and write the object store too, with the same Postgres fallback as drafts`.

### Task 5 (S1-5): Wire `egret-bin`

**Files:**
- Modify: `crates/egret-bin/src/main.rs` (the two `AppStore::new` call sites, ~line 2287 and ~2304)

**Interfaces:**
- Consumes: `engine_state.storage` (the ALREADY-CONSTRUCTED `Arc<dyn ObjectStore>` — the same `S3Store` workflow reuses; do not build a second one), `config.storage.bucket`, `config.apps.storage.prefix`.

- [ ] **Step 1:** Change both `AppStore::new(db.clone())` calls to `AppStore::new(db.clone(), engine_state.storage.clone(), config.storage.bucket.clone(), config.apps.storage.prefix.clone())` — same pattern as the workflow `FileStore` construction a few lines below in the same function, which already demonstrates reusing `engine_state.storage.clone()` with a one-line comment explaining why ("the SAME object store... one client, not a second one"); copy that comment's spirit onto this call site too.
- [ ] **Step 2:** `cargo check -p egret-bin`. Expected: compiles (this task is the one that makes S1-2 through S1-4's signature changes finally compile end to end).
- [ ] **Step 3:** Commit `feat(apps): wire AppStore to the shared object store in egret-bin`.

### Task 6 (S1-6): Test fixtures move to `MemStore`

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

### Task 7 (S1-7): One live verification against the dev stack's real MinIO

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

### Known limitation carried into Part S2/S3 (not fixed in S1)

The final whole-branch review (`.superpowers/sdd/2026-10-05-data-apps-storage-s3/final-review-report.md`, finding I3) found: `save_draft`'s row lock (Task S1-3) stops a CAS *loser* from ever writing to S3, but it does not stop the CAS *winner*'s own S3 `put` from landing before its Postgres `UPDATE`/commit. If the commit fails, or the request's handler future is dropped between the `put` and the commit (e.g. the client disconnects mid-request), S3 can hold content that Postgres never committed — `draft()` then serves that content under the *old* revision, breaking the invariant that one revision identifies one piece of content. The same shape of gap exists in `publish`/`insert_version` (an S3 write lands before the matching Postgres row commits).

This is accepted as a known limitation for Part S1 (the window is narrow — it requires a commit failure or a dropped future between the object-store write and the commit — and Postgres remains the system of record until Part S3's cutover, so a stale S3 object is shadowed by the correct Postgres column in every read path today). **It must be resolved before Part S3 removes the Postgres fallback reads**, because after cutover there is no column left to disagree with a stale S3 object. The reviewer's suggested fix: key draft content by revision (e.g. `draft/{revision}.py` instead of the fixed `draft.py`) — still a computed key, since `revision` is already a Postgres column — so a reader only ever sees the committed revision's object, and the orphaned pre-commit object can be deleted best-effort afterwards. Part S2's backfill command and Part S3's cutover task should address this (or explicitly re-affirm acceptance of the window) before Part S3 ships.

The final whole-branch review of Part S2 (finding I5) separately found that `backfill_apps_storage`'s drafts loop originally re-wrote every draft's S3 objects from an unlocked, up-front Postgres read, so a `save_draft` landing mid-run could have its new content clobbered by the backfill's stale snapshot under the save's own new revision number — fixed by `AppStore::backfill_draft_locked`, which takes `save_draft`'s row lock and re-reads the row from inside it. That fix means a live backfill run now repairs any stale draft object it encounters using the row's content *at the moment it takes the lock*, rather than a pre-read snapshot. It does **not**, however, close the underlying race described above for saves made *after* the backfill run completes: the known limitation remains an unresolved, hard prerequisite for Part S3 to close before the Postgres fallback reads are removed.

---

# Part S2 — Backfill existing rows into the object store

Branch `feat/data-apps-storage-s2-backfill` off `main` after S1 merges. A new one-off `egret` subcommand, following the existing `Command::{Serve,Migrate,Init,Workspace}` pattern in `egret-bin`. Idempotent (safe to run more than once) — it always overwrites the S3 key with the row's current Postgres content rather than skip-if-exists, so a rerun after an interrupted first run cannot leave a stale object behind.

### Task 8 (S2-1): `egret backfill-apps-storage`

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

### Task 9 (S3-1): Versions — stop writing `source`/`layout_json`, remove the Task 4 fallback reads

Versions have no I3-equivalent exposure (`app_versions` is append-only/immutable, so re-reading and rewriting an already-committed version's content is always safe) — this task is exactly the original S3-1 scope, narrowed to versions only. Drafts' cutover (which DOES have an I3 exposure) is Task 10.

**Files:**
- Modify: `crates/egret-apps/src/store.rs` (`version`, `insert_version`, `restore_version`)

- [ ] **Step 1:** Remove `source`/`layout_json` from the `INSERT` statements in `insert_version` and `restore_version` (they still exist as columns — Part S4 drops them — but nothing writes to them from here on).
- [ ] **Step 2:** Remove the Postgres-fallback branch Task 4 added: `version()`'s `SELECT` drops `source`/`layout_json` from the column list entirely (there is nothing left to fall back to that could be trusted — Task 8/S2's backfill is why this is now safe). On the SOURCE key, `StorageError::NotFound` becomes a real, propagated error again (not a fallback). On the LAYOUT key, `StorageError::NotFound` becomes `Ok(None)` — a layout-key miss is now unambiguous (no backfill-not-run-yet case left to be ambiguous with), so it just means "no layout," not an error. `restore_version()`'s `copy()`-then-fallback-to-GET+PUT also loses its fallback branch: every version from here on is guaranteed to have an object-store key.
- [ ] **Step 3:** Delete the fallback test Task 4 added (`a_pre_s1_version_with_no_object_store_content_falls_back_to_postgres`) — it asserts behavior this step deliberately removes. Add one test proving the opposite: a version row with NO object-store key at its source now surfaces as an error from `version()`, not a silent fallback. Add a second test proving a version with no layout object reads back `layout_json: None` (not an error) — the now-unambiguous case Step 2 describes.
- [ ] **Step 4:** Run the full suite (unit + DB-backed). Expected: green, with the fallback test gone and the two new tests passing.
- [ ] **Step 5:** Commit `feat(apps): stop writing version source/layout_json and remove the backfill-era Postgres fallback for versions`.

### Task 10 (S3-2): Drafts — close I3 by keying draft content by revision, then cut over

This is the one place in the whole plan with a real correctness gap: `save_draft`'s row lock (Part S1) stops a CAS *loser* from writing to S3, but a *fixed* draft key (`draft.py`, one object per app, overwritten on every save) means the CAS *winner*'s own S3 `put` can land before its Postgres commit. If the commit fails, or the request's handler future is dropped (client disconnect) between the `put` and the commit, S3 holds content Postgres never committed, and `draft()` — once this task removes the Postgres fallback — has no column left to disagree with it. This was tracked as finding I3 since Part S1's final review, with the fix the reviewer proposed then: key draft content by revision, the same way `app_versions` already keys by `version_no` — immutable, one object per revision, so a reader only ever sees the committed revision's object. `version_no`/`versions/{version_no}/...` already proves this pattern works; this task applies it to drafts.

Because Parts S1/S2 already wrote real content at the OLD fixed key (`draft.py`/`draft.layout.json`) in every environment already exercised (dev, test, the live-MinIO verifications for S1 and S2), switching `draft()`/`save_draft()` to the new revision-keyed format requires a one-time migration of that existing content to the new key shape BEFORE the old fixed-key path stops being written — otherwise every existing app's draft 404s the moment this ships. This task's `egret backfill-apps-storage` step does that migration (human-confirmed before this task began: production currently has no real Data Apps content, so the migration's correctness matters more than its urgency here, but it ships as a real, idempotent, defense-in-depth step regardless, not skipped on that basis).

**Files:**
- Modify: `crates/egret-apps/src/store.rs` (`draft_key`, `draft_layout_key`, `create`, `draft`, `save_draft`, `backfill_draft`, `backfill_draft_locked`)
- Modify: `crates/egret-bin/src/main.rs` (`backfill_apps_storage`'s drafts section — add the key-format migration)
- Modify: every test in `crates/egret-apps/tests/{lifecycle_store,http,podapi,proxy,s3_minio}.rs` that hand-builds a draft's S3 key string for an assertion (`grep -n 'draft.py\|draft.layout.json\|draft_key' crates/egret-apps/tests/*.rs` to find every site)

**Interfaces:**
- `draft_key`/`draft_layout_key` gain a `revision: i64` parameter: `fn draft_key(&self, ws: WorkspaceId, app: Uuid, revision: i64) -> String` producing `s3://{bucket}/{prefix}{ws}/{app}/draft/{revision}.py` (mirroring `version_key`'s `.../versions/{version_no}/source.py` shape exactly), and `draft_layout_key(..., revision: i64) -> String` producing `.../draft/{revision}.layout.json`.
- `backfill_draft` gains a `revision: i64` parameter (it now writes to the revision-keyed location, like `backfill_version` already does for `version_no`).

- [ ] **Step 1:** Change `draft_key`/`draft_layout_key` to take `revision: i64` and produce the new key shape above.
- [ ] **Step 2:** `create()`: a brand-new draft's row is inserted with `app_drafts.revision`'s schema default (`DEFAULT 1`) — pass the literal `1` to `draft_key(ws, id, 1)` at its existing write site (no Postgres read needed; the default is a known constant).
- [ ] **Step 3:** `save_draft()`: after confirming `revision == expected` under the row lock (unchanged), write the NEW content to `draft_key(ws, id, expected + 1)` (source, always) and `draft_layout_key(ws, id, expected + 1)` (layout, only if `Some` — there is no "delete" case anymore *within* a save, since each revision's key is distinct and never shared with another revision; a `None` layout for the new revision simply means nothing is ever written at that revision's layout key). After `tx.commit()` succeeds, best-effort-delete the OLD revision's objects (`draft_key(ws, id, expected)`, `draft_layout_key(ws, id, expected)`) to bound storage growth — this is cleanup, not correctness: an undeleted old-revision object is simply never read again, since nothing points at `expected` once the row moves to `expected + 1`. Ignore the delete's own errors (log at most); a failed cleanup must never fail the save that already committed.
- [ ] **Step 4:** `draft()`: keep `revision` in the `SELECT` (drop `source`/`layout_json`, per the original Task 9 intent). Compute `draft_key(ws, id, revision)` / `draft_layout_key(ws, id, revision)` using the just-read revision. On the SOURCE key, `StorageError::NotFound` becomes a real, propagated error (every draft is guaranteed a revision-keyed object once Step 6's migration has run and this code is live). On the LAYOUT key, `StorageError::NotFound` becomes `Ok(None)` — same unambiguous-miss reasoning as Task 9's Step 2 for versions.
- [ ] **Step 5:** `backfill_draft`: add a `revision: i64` parameter, write to `draft_key`/`draft_layout_key` with it instead of the old fixed-key call. `backfill_draft_locked`: extend its existing `SELECT source, layout_json FROM analytics.app_drafts WHERE app_id = $1 FOR UPDATE` to also select `revision`, and pass it through to `backfill_draft`.
- [ ] **Step 6 (the key-format migration):** In `egret-bin/src/main.rs`'s `backfill_apps_storage`, after the existing drafts loop (which already ensures Postgres's content is in S3 at the OLD fixed key for any pre-S1 row), add a second pass over the same `(workspace_id, app_id, revision)` rows: for each, `ObjectStore::copy()` from the OLD fixed key (`draft.py`) to the NEW revision-keyed key (`draft_key(ws, app, revision)`). A `StorageError::NotFound` on the copy's source is a legitimate outcome (a row created by code that already writes the new format directly has nothing at the old key to migrate) — not an error, skip it. Do the same for the layout key, where `NotFound` is additionally the ordinary "no layout" case. Any other error propagates and fails the command (same fail-fast-on-first-error posture the rest of this command already has). This reuses `copy()`'s existing server-side, no-bytes-through-this-process behavior (same as `restore_version`), and is safe to run more than once: a rerun after the old key was already migrated and cleaned up just sees `NotFound` again, a no-op.
- [ ] **Step 7:** Delete the fallback test Task 3 added (`a_pre_s1_row_with_no_object_store_content_falls_back_to_postgres`) — it asserts behavior this task removes. Add tests: (a) a draft with no object-store content at its current revision's key now surfaces as an error from `draft()`; (b) a draft whose layout is absent at its current revision's key reads back `layout_json: None`, not an error; (c) two successive `save_draft` calls produce two distinct, independently-readable objects (one per revision) — proving the key format is actually revision-scoped, not just renamed; (d) the key-format migration (Step 6) copies an old-fixed-key draft's content to the new revision-keyed location and `draft()` can read it afterward, using `MemStore` seeded directly (bypassing `AppStore`, simulating a pre-this-task row) the same way Part S2's own backfill test does.
- [ ] **Step 8:** Update every test in `crates/egret-apps/tests/*.rs` that hand-builds a draft S3 key string (several already exist, e.g. in `lifecycle_store.rs`, `s3_minio.rs`) to include the revision — `grep` for `draft.py`/`draft.layout.json`/`draft_key` across that directory first, don't rely on memory of which files have one.
- [ ] **Step 9:** Run the full suite (unit + DB-backed). Expected: green.
- [ ] **Step 10:** Manual live-verification against the dev stack's real MinIO (same discipline as S1-7 and S2-1's own live passes, and the highest-stakes one in this plan — this is the only task that changes a live read/write path's actual object KEY SHAPE, not just a fallback's presence): create an app pre-this-task-shaped (seed via the OLD fixed key directly, e.g. by temporarily running the pre-this-commit binary, or by `put`-ing directly at the old key via `mc`), run the migration (Step 6), confirm via `draft()`/the HTTP API that its content still reads correctly, then save a new draft and confirm the NEW revision-keyed object appears and the old-fixed-key object is untouched (nothing should write there anymore).
- [ ] **Step 11:** Commit `feat(apps): key draft content by revision (closes I3) and migrate existing drafts to the new key shape`.

### Self-review for Part S3

- Confirmed Part S2's backfill has actually run against every real environment before this merges, AND Task 10's key-format migration has too — this is a process check, not a code one; state it plainly in the PR description, same as every other irreversible-adjacent step this session has handled.
- I3 (tracked since Part S1's final review, reaffirmed as a hard Part S3 prerequisite in Part S2's own review) is resolved by Task 10, not deferred further.
- **Deploy order matters for this PR specifically, not just the usual rolling-deploy caveats.** Tasks 9 and 10 each relax a `NOT NULL` constraint (`app_versions.source`, `app_drafts.source`) and make new code write `NULL` into it. An OLD pod (pre-this-PR binary) still assumes that column is never `NULL` and will **panic** decoding a `NULL` value for any row the NEW code creates or saves (e.g. a brand-new app, or a save on an existing one) — this is not just "stale reads," it is a crash. Deploy this PR with no old/new pod overlap (a `Recreate` strategy, or scale old pods to zero before the new ones start serving) rather than an ordinary rolling deploy. Run `egret backfill-apps-storage` once before the deploy and once again after it finishes, exactly as Part S2 already required — the second run catches anything an old pod wrote in the brief window before it was fully replaced. State this explicitly in the PR description; it is a real behavior change for this one release, not a generic caveat.

---

# Part S4 — Drop the columns

Branch `feat/data-apps-storage-s4-drop-columns` off `main`, **separately, after a confidence period following S3** (this session has no fixed number of days to recommend — that is a product/ops call, not an engineering one; note it as an open question in the PR rather than guessing).

### Task 11 (S4-1): The migration

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

### Task 12 (S4-2): PR

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
