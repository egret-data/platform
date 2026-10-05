//! Data Apps P2 storage (migration 0163): drafts, append-only versions, and the
//! foreign keys P1 deferred. Schema section: every guarantee here is the
//! database's, asserted by the exact SQLSTATE a violating statement gets.

use egret_apps::store::{AppStore, PublishOutcome, SaveOutcome, MAX_SOURCE_BYTES};
use egret_apps::template::new_app_source;
use egret_core::{Error, UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::{MemStore, ObjectStore, StorageError};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

struct Fixture {
    db: Db,
    user: Uuid,
    ws: Uuid,
    app: Uuid,
    login_sid: Uuid,
    // S1-6 will move every `Fixture` in this crate to a `MemStore`-backed
    // `AppStore` properly; this one is minimal, just enough for S1-3's own
    // test (`draft_content_round_trips_through_the_object_store_not_postgres`)
    // and for the pre-existing `draft`/`save_draft` call sites to compile.
    mem: Arc<MemStore>,
    bucket: String,
    prefix: String,
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 5).await.expect("connect");
        let tag = Uuid::new_v4().simple().to_string();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status, password_hash) \
             VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
        )
        .bind(format!("apps_lc_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .expect("insert user");
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Apps Lifecycle Test', $2, 'active') RETURNING id",
        )
        .bind(format!("apps-lc-{tag}"))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");
        let login_sid: Uuid = sqlx::query_scalar(
            "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
             VALUES ($1, $2, now() + interval '8 hours', now() + interval '30 minutes') RETURNING id",
        )
        .bind(format!("{tag}{}", Uuid::new_v4().simple()))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("insert login session");
        let mut fx = Self {
            db,
            user,
            ws,
            app: Uuid::nil(),
            login_sid,
            mem: Arc::new(MemStore::new()),
            bucket: "test-bucket".to_string(),
            prefix: "apps/".to_string(),
        };
        fx.app = fx.new_app().await;
        fx
    }

    async fn new_app(&self) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'app', $2) RETURNING id",
        )
        .bind(self.ws)
        .bind(self.user)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert app")
    }

    async fn try_version(&self, app: Uuid, no: i32) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no, source, from_revision, published_by) \
             VALUES ($1, $2, 'import marimo', 1, $3) RETURNING id",
        )
        .bind(app)
        .bind(no)
        .bind(self.user)
        .fetch_one(&self.db.pool)
        .await
    }

    async fn version(&self, app: Uuid, no: i32) -> Uuid {
        self.try_version(app, no).await.expect("insert version")
    }

    /// Insert a run session for `app` pinned to `version`.
    async fn try_run_session(&self, app: Uuid, version: Uuid) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO analytics.app_sessions \
             (token_hash, user_id, workspace_id, app_id, version_id, mode, login_sid, \
              idle_expires_at, absolute_expires_at) \
             VALUES ($1, $2, $3, $4, $5, 'run', $6, \
                     now() + interval '30 minutes', now() + interval '8 hours') RETURNING id",
        )
        .bind(Uuid::new_v4().simple().to_string())
        .bind(self.user)
        .bind(self.ws)
        .bind(app)
        .bind(version)
        .bind(self.login_sid)
        .fetch_one(&self.db.pool)
        .await
    }

    async fn count(&self, sql: &'static str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("count")
    }
}

/// SQLSTATE of a failed statement.
fn code<T: std::fmt::Debug>(r: Result<T, sqlx::Error>) -> String {
    let e = r.expect_err("statement must fail");
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
        .unwrap_or_else(|| panic!("expected a database error, got {e:?}"))
}

/// Name of the constraint a failed statement violated.
fn constraint<T: std::fmt::Debug>(r: Result<T, sqlx::Error>) -> String {
    let e = r.expect_err("statement must fail");
    e.as_database_error()
        .and_then(|d| d.constraint().map(str::to_string))
        .unwrap_or_else(|| panic!("expected a constraint violation, got {e:?}"))
}

#[tokio::test]
async fn versions_cannot_be_updated() {
    let fx = Fixture::new().await;
    let v = fx.version(fx.app, 1).await;
    let r = sqlx::query("UPDATE analytics.app_versions SET source = 'x' WHERE id = $1")
        .bind(v)
        .execute(&fx.db.pool)
        .await;
    assert_eq!(code(r), "23001");
    let source: String =
        sqlx::query_scalar("SELECT source FROM analytics.app_versions WHERE id = $1")
            .bind(v)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(source, "import marimo");
}

#[tokio::test]
async fn versions_cannot_be_deleted_directly() {
    let fx = Fixture::new().await;
    let v = fx.version(fx.app, 1).await;
    let r = sqlx::query("DELETE FROM analytics.app_versions WHERE id = $1")
        .bind(v)
        .execute(&fx.db.pool)
        .await;
    assert_eq!(code(r), "23001");
    assert_eq!(
        fx.count(
            "SELECT count(*) FROM analytics.app_versions WHERE id = $1",
            v
        )
        .await,
        1
    );
}

/// The trigger's one exception: a delete that is the ON DELETE CASCADE from the
/// app (itself reached from the workspace). Includes a restore (a version whose
/// `restored_from` points at another version of the same app), a draft, and a
/// run session pinned to a version, so every edge the cascade crosses is present.
#[tokio::test]
async fn deleting_the_workspace_cascades_through_versions() {
    let fx = Fixture::new().await;
    let v1 = fx.version(fx.app, 1).await;
    let v2: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.app_versions (app_id, version_no, source, restored_from) \
         SELECT app_id, 2, source, id FROM analytics.app_versions WHERE id = $1 RETURNING id",
    )
    .bind(v1)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert restore");
    sqlx::query(
        "INSERT INTO analytics.app_drafts (app_id, source, updated_by) VALUES ($1, 'draft', $2)",
    )
    .bind(fx.app)
    .bind(fx.user)
    .execute(&fx.db.pool)
    .await
    .expect("insert draft");
    fx.try_run_session(fx.app, v1)
        .await
        .expect("run session on v1");
    fx.try_run_session(fx.app, v2)
        .await
        .expect("run session on v2");

    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(fx.ws)
        .execute(&fx.db.pool)
        .await
        .expect("deleting the workspace must cascade through the append-only versions");

    for sql in [
        "SELECT count(*) FROM analytics.apps WHERE id = $1",
        "SELECT count(*) FROM analytics.app_versions WHERE app_id = $1",
        "SELECT count(*) FROM analytics.app_drafts WHERE app_id = $1",
        "SELECT count(*) FROM analytics.app_sessions WHERE app_id = $1",
    ] {
        assert_eq!(fx.count(sql, fx.app).await, 0, "{sql}");
    }
}

#[tokio::test]
async fn version_numbers_are_unique_per_app() {
    let fx = Fixture::new().await;
    fx.version(fx.app, 1).await;
    assert_eq!(code(fx.try_version(fx.app, 1).await), "23505");
    assert_eq!(
        constraint(fx.try_version(fx.app, 1).await),
        "app_versions_app_no_key"
    );
    // The same number on a DIFFERENT app is fine.
    let other = fx.new_app().await;
    fx.version(other, 1).await;
}

#[tokio::test]
async fn draft_source_over_one_mib_is_rejected_by_the_schema() {
    let fx = Fixture::new().await;
    let insert = |source: String| {
        sqlx::query("INSERT INTO analytics.app_drafts (app_id, source) VALUES ($1, $2)")
            .bind(fx.app)
            .bind(source)
            .execute(&fx.db.pool)
    };
    assert_eq!(code(insert("a".repeat(1_048_577)).await), "23514");
    // Exactly 1 MiB is the limit, not past it.
    insert("a".repeat(1_048_576))
        .await
        .expect("a 1 MiB draft is accepted");
}

#[tokio::test]
async fn a_run_session_must_pin_a_version_of_its_own_app() {
    let fx = Fixture::new().await;
    let app_b = fx.new_app().await;
    let v_b = fx.version(app_b, 1).await;
    let v_a = fx.version(fx.app, 1).await;

    assert_eq!(code(fx.try_run_session(fx.app, v_b).await), "23503");
    assert_eq!(
        constraint(fx.try_run_session(fx.app, v_b).await),
        "app_sessions_version_app_fk"
    );
    // A version that does not exist at all is refused the same way.
    assert_eq!(
        code(fx.try_run_session(fx.app, Uuid::new_v4()).await),
        "23503"
    );
    // Control: its own app's version is accepted.
    fx.try_run_session(fx.app, v_a)
        .await
        .expect("a run session pinned to its own app's version");
}

#[tokio::test]
async fn deleting_a_folder_uncategorizes_its_apps() {
    let fx = Fixture::new().await;
    let folder: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.folders (workspace_id, entity_type, name, created_by) \
         VALUES ($1, 'app', $2, $3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(format!("f-{}", Uuid::new_v4().simple()))
    .bind(fx.user)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert folder");
    sqlx::query("UPDATE analytics.apps SET folder_id = $2 WHERE id = $1")
        .bind(fx.app)
        .bind(folder)
        .execute(&fx.db.pool)
        .await
        .expect("file the app");
    // The FK exists: a folder id that is not a folder is refused.
    let r = sqlx::query("UPDATE analytics.apps SET folder_id = $2 WHERE id = $1")
        .bind(fx.app)
        .bind(Uuid::new_v4())
        .execute(&fx.db.pool)
        .await;
    assert_eq!(code(r), "23503");

    sqlx::query("DELETE FROM analytics.folders WHERE id = $1")
        .bind(folder)
        .execute(&fx.db.pool)
        .await
        .expect("delete folder");
    let folder_id: Option<Uuid> =
        sqlx::query_scalar("SELECT folder_id FROM analytics.apps WHERE id = $1")
            .bind(fx.app)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(folder_id, None);
}

// ---------------------------------------------------------------------------
// Store section: AppStore over the real schema.
// ---------------------------------------------------------------------------

impl Fixture {
    fn store(&self) -> AppStore {
        AppStore::new(
            self.db.clone(),
            self.mem.clone(),
            self.bucket.clone(),
            self.prefix.clone(),
        )
    }
    fn ws(&self) -> WorkspaceId {
        WorkspaceId(self.ws)
    }
    fn uid(&self) -> UserId {
        UserId(self.user)
    }
    fn bucket(&self) -> &str {
        &self.bucket
    }
    fn mem_store(&self) -> Arc<MemStore> {
        self.mem.clone()
    }
    /// An app made through the store (so it has a rev-1 draft).
    async fn store_app(&self, source: &str) -> Uuid {
        self.store()
            .create(self.ws(), self.uid(), "app", "", None, source)
            .await
            .expect("create")
            .id
    }
}

fn published(o: PublishOutcome) -> egret_apps::store::VersionSummary {
    match o {
        PublishOutcome::Published(v) => v,
        PublishOutcome::Stale { current_revision } => panic!("stale at {current_revision}"),
    }
}

#[tokio::test]
async fn create_makes_an_app_with_a_rev1_draft_from_the_template() {
    let fx = Fixture::new().await;
    let src = new_app_source(Some("lake"));
    let app = fx
        .store()
        .create(fx.ws(), fx.uid(), "My app", "d", None, &src)
        .await
        .unwrap();
    assert_eq!(app.name, "My app");
    assert_eq!(app.draft_revision, 1);
    assert_eq!(app.latest_version_no, None);
    assert!(!app.bookmarked);
    let draft = fx.store().draft(fx.ws(), app.id).await.unwrap().unwrap();
    assert_eq!(draft.revision, 1);
    assert_eq!(draft.source, src);
}

#[tokio::test]
async fn save_draft_bumps_revision_and_refuses_stale() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    assert_eq!(
        st.save_draft(fx.ws(), app, fx.uid(), 1, "first", None)
            .await
            .unwrap(),
        SaveOutcome::Saved { revision: 2 }
    );
    assert_eq!(
        st.save_draft(fx.ws(), app, fx.uid(), 1, "second", None)
            .await
            .unwrap(),
        SaveOutcome::Stale {
            current_revision: 2
        }
    );
    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!((d.source.as_str(), d.revision), ("first", 2));
}

#[tokio::test]
async fn draft_content_round_trips_through_the_object_store_not_postgres() {
    let fx = Fixture::new().await;
    let app = fx.store_app("hello").await;
    let st = fx.store();
    st.save_draft(fx.ws(), app, fx.uid(), 1, "x = 2", None)
        .await
        .unwrap();

    // Postgres's source column is still written too (the Task 9 rollback/fallback
    // safety net), but the object store is preferred when present -- prove this
    // content came from there by reading the key directly.
    let key = format!(
        "s3://{}/apps/{}/{}/draft.py",
        fx.bucket(),
        fx.ws().0.hyphenated(),
        app.hyphenated()
    );
    let bytes = fx.mem_store().get(&key).await.unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "x = 2");

    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!(d.source, "x = 2");
}

/// Part S1 must stay safe to ship on its own, with existing Postgres-only rows
/// still readable, until Part S2's backfill runs -- so `draft()` falls back to
/// the row's own column on an object-store miss. Inserted directly via SQL
/// (NOT through `store_app`/`create`, which writes the object store too as of
/// this task) to simulate data that genuinely predates this code.
#[tokio::test]
async fn a_pre_s1_row_with_no_object_store_content_falls_back_to_postgres() {
    let fx = Fixture::new().await;
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

/// Regression test for a race a task review caught: an unlocked "check revision,
/// then write S3, then UPDATE" lets two ordinary concurrent saves both pass the
/// check, both write to S3, and leave the CAS loser's content visible under the
/// winner's revision. The fix locks the draft row (`SELECT ... FOR UPDATE`, the
/// same pattern `publish` already uses on this table) across the object-store
/// writes, so a second saver blocks for the first's full duration and emerges
/// to see the new revision -- this is real Postgres row-level locking, not a
/// timing-dependent race, so it does not need an artificial delay in the object
/// store to prove: both transactions' `FOR UPDATE` requests reach a REAL
/// Postgres instance over real connections, and Postgres itself serializes
/// them regardless of how fast `MemStore`'s `put` returns. Repeated (as the
/// existing `concurrent_publishes_get_distinct_numbers` is) to build confidence
/// the lock is actually taking effect, not accidentally not-racing.
#[tokio::test]
async fn a_losing_concurrent_save_never_becomes_the_visible_draft() {
    let fx = Fixture::new().await;
    let st = fx.store();
    for i in 0..20 {
        let app = fx.store_app("v1").await;
        let (a, b) = tokio::join!(
            st.save_draft(fx.ws(), app, fx.uid(), 1, "a", None),
            st.save_draft(fx.ws(), app, fx.uid(), 1, "b", None),
        );
        let winner = match (a, b) {
            (Ok(SaveOutcome::Saved { .. }), Ok(SaveOutcome::Stale { .. })) => "a",
            (Ok(SaveOutcome::Stale { .. }), Ok(SaveOutcome::Saved { .. })) => "b",
            other => {
                panic!("iteration {i}: expected exactly one Saved and one Stale, got {other:?}")
            }
        };
        let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
        assert_eq!(
            d.source, winner,
            "iteration {i}: the CAS loser's content must never become the visible draft"
        );
    }
}

#[tokio::test]
async fn save_draft_over_limit_is_an_error_not_a_save() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let big = "a".repeat(MAX_SOURCE_BYTES + 1);
    let err = fx
        .store()
        .save_draft(fx.ws(), app, fx.uid(), 1, &big, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
    let d = fx.store().draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!((d.source.as_str(), d.revision), ("v1", 1));
    // Exactly the limit is accepted.
    let ok = "a".repeat(MAX_SOURCE_BYTES);
    assert_eq!(
        fx.store()
            .save_draft(fx.ws(), app, fx.uid(), 1, &ok, None)
            .await
            .unwrap(),
        SaveOutcome::Saved { revision: 2 }
    );
}

#[tokio::test]
async fn publish_numbers_versions_and_honours_expected_revision() {
    let fx = Fixture::new().await;
    let app = fx.store_app("a").await;
    let st = fx.store();
    let v1 = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
    assert_eq!((v1.version_no, v1.from_revision), (1, Some(1)));
    assert_eq!(v1.published_by, Some(fx.user));
    st.save_draft(fx.ws(), app, fx.uid(), 1, "b", None)
        .await
        .unwrap();
    match st.publish(fx.ws(), app, fx.uid(), Some(1)).await.unwrap() {
        PublishOutcome::Stale { current_revision } => assert_eq!(current_revision, 2),
        other => panic!("expected stale, got {other:?}"),
    }
    let v2 = published(st.publish(fx.ws(), app, fx.uid(), Some(2)).await.unwrap());
    assert_eq!((v2.version_no, v2.from_revision), (2, Some(2)));
    let nos: Vec<i32> = st
        .versions(app)
        .await
        .unwrap()
        .iter()
        .map(|v| v.version_no)
        .collect();
    assert_eq!(nos, [2, 1], "newest first");
}

/// Two publishes on separate pool connections. Without the draft-row lock both read
/// max=0 and one dies on `app_versions_app_no_key`; the race is narrow, so repeat it.
#[tokio::test]
async fn concurrent_publishes_get_distinct_numbers() {
    let fx = Fixture::new().await;
    let st = fx.store();
    for i in 0..20 {
        let app = fx.store_app("a").await;
        let (a, b) = tokio::join!(
            st.publish(fx.ws(), app, fx.uid(), None),
            st.publish(fx.ws(), app, fx.uid(), None)
        );
        let mut nos = vec![
            published(a.unwrap_or_else(|e| panic!("iteration {i}: {e:?}"))).version_no,
            published(b.unwrap_or_else(|e| panic!("iteration {i}: {e:?}"))).version_no,
        ];
        nos.sort();
        assert_eq!(nos, [1, 2], "iteration {i}");
    }
}

/// A restore and a publish of the same app both append a version; restore must take
/// the same draft-row lock, or both read the same max and one dies on
/// `app_versions_app_no_key`. Narrow race, so repeat it.
#[tokio::test]
async fn concurrent_publish_and_restore_get_distinct_numbers() {
    let fx = Fixture::new().await;
    let st = fx.store();
    for i in 0..20 {
        let app = fx.store_app("a").await;
        let v1 = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
        let (a, b) = tokio::join!(
            st.publish(fx.ws(), app, fx.uid(), None),
            st.restore_version(fx.ws(), app, v1.id, fx.uid())
        );
        let mut nos = vec![
            published(a.unwrap_or_else(|e| panic!("iteration {i}: publish {e:?}"))).version_no,
            b.unwrap_or_else(|e| panic!("iteration {i}: restore {e:?}"))
                .expect("v1 belongs to the app")
                .version_no,
        ];
        nos.sort();
        assert_eq!(nos, [2, 3], "iteration {i}");
    }
}

/// No draft row means no lock to take: restore refuses (as publish does) rather
/// than append a version unserialised.
#[tokio::test]
async fn restore_without_a_draft_row_is_not_found() {
    let fx = Fixture::new().await;
    // `fx.app` was inserted raw, so it has no draft row.
    let v = fx.version(fx.app, 1).await;
    let err = fx
        .store()
        .restore_version(fx.ws(), fx.app, v, fx.uid())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn restore_republishes_old_source_as_a_new_version() {
    let fx = Fixture::new().await;
    let app = fx.store_app("a").await;
    let st = fx.store();
    let v1 = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
    st.save_draft(fx.ws(), app, fx.uid(), 1, "b", None)
        .await
        .unwrap();
    published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
    let v3 = st
        .restore_version(fx.ws(), app, v1.id, fx.uid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v3.version_no, 3);
    assert_eq!(v3.restored_from, Some(v1.id));
    assert_eq!(v3.from_revision, None);
    assert_eq!(
        st.version(fx.ws(), app, v3.id)
            .await
            .unwrap()
            .unwrap()
            .source,
        "a"
    );
    // The draft is untouched by a restore.
    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!((d.source.as_str(), d.revision), ("b", 2));
    // A version of another app is not restorable here.
    let other = fx.store_app("x").await;
    let ov = published(st.publish(fx.ws(), other, fx.uid(), None).await.unwrap());
    assert!(st
        .restore_version(fx.ws(), app, ov.id, fx.uid())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn version_lookup_is_scoped_to_its_app() {
    let fx = Fixture::new().await;
    let app_a = fx.store_app("a").await;
    let app_b = fx.store_app("b").await;
    let st = fx.store();
    let va = published(st.publish(fx.ws(), app_a, fx.uid(), None).await.unwrap());
    assert!(st.version(fx.ws(), app_b, va.id).await.unwrap().is_none());
    assert_eq!(
        st.version(fx.ws(), app_a, va.id)
            .await
            .unwrap()
            .unwrap()
            .source,
        "a"
    );
}

#[tokio::test]
async fn publishing_writes_the_versions_content_to_the_object_store() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let v = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());

    let key = format!(
        "s3://{}/apps/{}/{}/versions/{}/source.py",
        fx.bucket(),
        fx.ws().0.hyphenated(),
        app.hyphenated(),
        v.version_no
    );
    assert_eq!(fx.mem_store().get(&key).await.unwrap(), b"v1");
}

#[tokio::test]
async fn restoring_a_version_copies_its_object_server_side_without_reading_it_into_the_app() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let v1 = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
    st.save_draft(fx.ws(), app, fx.uid(), 1, "v2", None)
        .await
        .unwrap();
    published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());

    let v3 = st
        .restore_version(fx.ws(), app, v1.id, fx.uid())
        .await
        .unwrap()
        .unwrap();

    let key = |no: i32| {
        format!(
            "s3://{}/apps/{}/{}/versions/{}/source.py",
            fx.bucket(),
            fx.ws().0.hyphenated(),
            app.hyphenated(),
            no
        )
    };
    assert_eq!(
        fx.mem_store().get(&key(v3.version_no)).await.unwrap(),
        b"v1"
    );
    // This test proves the CONTENT is right. It does not prove restore_version
    // used `copy()` rather than `get()`+`put()` -- `egret_storage::MemStore`
    // (checked during this plan's research: crates/egret-storage/src/mem.rs)
    // has no call-tracking today, and adding one is a separate, wider-blast-
    // radius change to a shared test-util crate this task should not casually
    // make. Confirm the `copy()` call site by reading the diff in review
    // instead of asserting it here.
}

/// Same fallback as Task 3's `draft()` -- a version published before this code
/// shipped has real content in `analytics.app_versions.source` and nothing in
/// the object store.
#[tokio::test]
async fn a_pre_s1_version_with_no_object_store_content_falls_back_to_postgres() {
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

    let v = fx
        .store()
        .version(fx.ws(), app, version_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v.source, "legacy v1");
}

#[tokio::test]
async fn list_separates_trashed_and_marks_bookmarks() {
    let fx = Fixture::new().await;
    let st = fx.store();
    let live = fx.store_app("a").await;
    let gone = fx.store_app("b").await;
    assert!(st.set_trashed(fx.ws(), gone, true).await.unwrap());
    st.set_bookmark(fx.uid(), live, true).await.unwrap();
    st.set_bookmark(fx.uid(), live, true).await.unwrap(); // idempotent

    // The fixture's own `fx.app` (no draft, unbookmarked) is also live in this workspace.
    let l = st.list(fx.ws(), fx.uid(), false).await.unwrap();
    assert_eq!(
        l.iter().map(|a| a.id).collect::<HashSet<_>>(),
        HashSet::from([live, fx.app])
    );
    assert!(l.iter().find(|a| a.id == live).unwrap().bookmarked);
    assert!(!l.iter().find(|a| a.id == fx.app).unwrap().bookmarked);
    let t = st.list(fx.ws(), fx.uid(), true).await.unwrap();
    assert_eq!(t.iter().map(|a| a.id).collect::<Vec<_>>(), [gone]);
    assert!(t[0].deleted_at.is_some() && !t[0].bookmarked);
    // Bookmarks are per user.
    let other = UserId(Uuid::new_v4());
    let any_bookmarked = |l: Vec<egret_apps::store::AppRow>| l.iter().any(|a| a.bookmarked);
    assert!(!any_bookmarked(
        st.list(fx.ws(), other, false).await.unwrap()
    ));
    st.set_bookmark(fx.uid(), live, false).await.unwrap();
    assert!(!any_bookmarked(
        st.list(fx.ws(), fx.uid(), false).await.unwrap()
    ));
    // get returns trashed apps too, with the latest version number once published.
    assert!(st.get(fx.ws(), fx.uid(), gone).await.unwrap().is_some());
    published(st.publish(fx.ws(), live, fx.uid(), None).await.unwrap());
    assert_eq!(
        st.get(fx.ws(), fx.uid(), live)
            .await
            .unwrap()
            .unwrap()
            .latest_version_no,
        Some(1)
    );
}

#[tokio::test]
async fn rows_from_another_workspace_are_invisible() {
    let fx = Fixture::new().await;
    let st = fx.store();
    let app = fx.store_app("a").await;
    let other_ws = Fixture::new().await.ws();
    assert!(st.get(other_ws, fx.uid(), app).await.unwrap().is_none());
    assert!(!st.update(other_ws, app, Some("x"), None).await.unwrap());
    assert!(!st.set_trashed(other_ws, app, true).await.unwrap());
    assert!(!st.set_folder(other_ws, app, None).await.unwrap());
    let listed = st.list(other_ws, fx.uid(), false).await.unwrap();
    assert!(listed
        .iter()
        .all(|a| a.id != app && a.workspace_id == other_ws));
    // Nothing changed in the owning workspace.
    let a = st.get(fx.ws(), fx.uid(), app).await.unwrap().unwrap();
    assert_eq!((a.name.as_str(), a.deleted_at.is_none()), ("app", true));
}

/// `published_by ... ON DELETE SET NULL` is an UPDATE on an append-only table: the
/// trigger must let exactly that UPDATE through, or no publisher could ever be deleted.
/// Driven through the real `UserRepo::hard_delete`, not a hand-written DELETE.
#[tokio::test]
async fn deleting_a_publisher_keeps_their_versions_with_publisher_null() {
    let fx = Fixture::new().await;
    let tag = Uuid::new_v4().simple().to_string();
    let publisher: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'publisher', 'active', 'x') RETURNING id",
    )
    .bind(format!("apps_lc_pub_{tag}@egret.local"))
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert publisher");
    let app = fx.store_app("by the publisher").await;
    let st = fx.store();
    let v = published(
        st.publish(fx.ws(), app, UserId(publisher), None)
            .await
            .unwrap(),
    );
    assert_eq!(v.published_by, Some(publisher));

    let deleted = egret_auth::users::UserRepo::new(fx.db.clone())
        .hard_delete(UserId(publisher))
        .await
        .expect("a publisher can be deleted");
    assert!(deleted.is_some(), "the user row was deleted");

    let (by, source, no): (Option<Uuid>, String, i32) = sqlx::query_as(
        "SELECT published_by, source, version_no FROM analytics.app_versions WHERE id = $1",
    )
    .bind(v.id)
    .fetch_one(&fx.db.pool)
    .await
    .expect("the version row survives");
    assert_eq!(by, None);
    assert_eq!(source, "by the publisher");
    assert_eq!(no, v.version_no);

    // The allowance is exact: nulling `published_by` while changing anything else
    // is still an UPDATE of a published version.
    let v2 = published(st.publish(fx.ws(), app, fx.uid(), None).await.unwrap());
    let r = sqlx::query(
        "UPDATE analytics.app_versions SET published_by = NULL, source = 'tampered' WHERE id = $1",
    )
    .bind(v2.id)
    .execute(&fx.db.pool)
    .await;
    assert_eq!(code(r), "23001");
}

// ---------------------------------------------------------------------------
// Final-review fixes (I1/I2): stale layout keys on publish/restore, and
// fallback-path coverage for restore/draft layouts.
// ---------------------------------------------------------------------------

/// I1: `version_no` is a freshly computed `MAX+1`, so it can collide with an
/// orphaned layout object left behind by a previous failed publish attempt
/// (e.g. the handler future dropped after `insert_version`'s layout PUT but
/// before the transaction committed). Pre-seed exactly such an orphan at the
/// key version 1 will use, then publish a draft with NO layout -- `version()`
/// must report `None` (Postgres's genuinely NULL column), not the stale object.
#[tokio::test]
async fn publishing_with_no_layout_clears_any_stale_layout_object() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();

    let orphan_key = format!(
        "s3://{}/apps/{}/{}/versions/1/layout.json",
        fx.bucket(),
        fx.ws().0.hyphenated(),
        app.hyphenated()
    );
    fx.mem_store()
        .put(&orphan_key, b"orphaned layout".to_vec())
        .await
        .unwrap();

    // The draft has no layout (saved with `None`), so the resulting v1 must
    // have `layout_json: None`, not the pre-seeded orphan.
    st.save_draft(fx.ws(), app, fx.uid(), 1, "v1", None)
        .await
        .unwrap();
    let v1 = published(st.publish(fx.ws(), app, fx.uid(), Some(2)).await.unwrap());
    assert_eq!(v1.version_no, 1);

    let v = st.version(fx.ws(), app, v1.id).await.unwrap().unwrap();
    assert_eq!(v.layout_json, None);
}

/// I2 Test A: restoring a pre-S1 version (no object-store content for either
/// source or layout) falls back to Postgres's columns -- the path
/// `S3Store::copy`'s `NotFound`-mapping fix exists to make reachable.
#[tokio::test]
async fn restoring_a_pre_s1_version_falls_back_to_postgres_content() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let (version_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO analytics.app_versions (app_id, version_no, source, layout_json) \
         VALUES ($1, 1, 'legacy source', 'legacy layout') RETURNING id",
    )
    .bind(app)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();

    let new_summary = st
        .restore_version(fx.ws(), app, version_id, fx.uid())
        .await
        .unwrap()
        .expect("version belongs to the app");
    assert_eq!(new_summary.version_no, 2);

    let new_version = st
        .version(fx.ws(), app, new_summary.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_version.source, "legacy source");
    assert_eq!(new_version.layout_json.as_deref(), Some("legacy layout"));

    // Prove the fallback `put` path actually ran: the new version's own
    // object-store keys hold the right bytes, not just that some reader
    // (which could itself fall back) found the right content.
    let source_key = format!(
        "s3://{}/apps/{}/{}/versions/2/source.py",
        fx.bucket(),
        fx.ws().0.hyphenated(),
        app.hyphenated()
    );
    let layout_key = format!(
        "s3://{}/apps/{}/{}/versions/2/layout.json",
        fx.bucket(),
        fx.ws().0.hyphenated(),
        app.hyphenated()
    );
    assert_eq!(
        fx.mem_store().get(&source_key).await.unwrap(),
        b"legacy source"
    );
    assert_eq!(
        fx.mem_store().get(&layout_key).await.unwrap(),
        b"legacy layout"
    );
}

/// I2 Test B: a draft layout put-then-clear round-trips through the object
/// store in both directions -- saving a layout writes the object, and saving
/// again with `None` actually deletes it (not just stops exposing it).
#[tokio::test]
async fn saving_with_no_layout_clears_a_previously_saved_one() {
    let fx = Fixture::new().await;
    let app = fx.store_app("v1").await;
    let st = fx.store();
    let layout_key = format!(
        "s3://{}/apps/{}/{}/draft.layout.json",
        fx.bucket(),
        fx.ws().0.hyphenated(),
        app.hyphenated()
    );

    st.save_draft(fx.ws(), app, fx.uid(), 1, "v1", Some("some layout"))
        .await
        .unwrap();
    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!(d.layout_json.as_deref(), Some("some layout"));
    assert_eq!(
        fx.mem_store().get(&layout_key).await.unwrap(),
        b"some layout"
    );

    st.save_draft(fx.ws(), app, fx.uid(), 2, "v1", None)
        .await
        .unwrap();
    let d = st.draft(fx.ws(), app).await.unwrap().unwrap();
    assert_eq!(d.layout_json, None);
    assert!(matches!(
        fx.mem_store().get(&layout_key).await,
        Err(StorageError::NotFound(_))
    ));
}

/// I2 Test C: a pre-S1 draft row's `layout_json` column also falls back
/// correctly, mirroring `a_pre_s1_row_with_no_object_store_content_falls_back_to_postgres`
/// for `source` but extended to the layout column too.
#[tokio::test]
async fn a_pre_s1_drafts_layout_also_falls_back_to_postgres() {
    let fx = Fixture::new().await;
    let app_id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.apps (workspace_id, name, description) VALUES ($1, 'legacy', '') RETURNING id",
    )
    .bind(fx.ws().0)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO analytics.app_drafts (app_id, source, layout_json) VALUES ($1, 'legacy', 'legacy layout')",
    )
    .bind(app_id)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    let d = fx.store().draft(fx.ws(), app_id).await.unwrap().unwrap();
    assert_eq!(d.source, "legacy");
    assert_eq!(d.layout_json.as_deref(), Some("legacy layout"));
}
