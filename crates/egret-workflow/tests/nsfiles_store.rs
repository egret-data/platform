//! W9b Task 2: the read side of `workflow.namespace_files` (migration 0174)
//! and the objects behind it; W9b Task 3: the write side (`put`,
//! `create_directory`, `soft_delete`, `move_path`, `purge_revisions`) and its
//! per-path advisory lock. Requires DATABASE_URL.
//!
//! Task 2's read tests insert rows with plain SQL. Every test makes its own
//! workspaces, so the tests cannot see each other's rows. The only object
//! stores touched are `MemStore`s (`install_mem_for_tests`'s, or a test's own
//! fresh one): every delete, purge and move here runs in memory.
//!
//! # The concurrency tests are ordered, not timed
//!
//! A writer's body is uploaded to a staging object BEFORE it takes any lock
//! (Task 8 fix round 1, the user's ruling of 2026-10-06); under the lock it
//! copies that object onto the revision's key. So a writer's lock is held
//! across its [`Hooked`] `copy`, which can be gated: it says it started, then
//! waits to be released. "Writer A holds the lock mid-write" is therefore an
//! event the test waits for, and so is "writer B is blocked": either B's copy
//! says it started (no lock stopped it) or `pg_locks` shows B waiting on the
//! path's advisory key ([`blocked_or_started`]). Nothing is asserted about
//! time; the poll of `pg_locks` only waits for a state. A [`Gated`] BODY,
//! which stalls mid-upload, now holds nothing a writer needs
//! (`a_stalled_body_holds_no_connection_and_no_lock`).

mod common;

use egret_metastore::Db;
use egret_workflow::storage;
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{
    create_directory, file_rows_page, list_live, move_path, namespaces_with_files, open,
    purge_revisions, purge_rows, put, resolve, revisions, search, soft_delete,
    soft_delete_if_empty, Conflict, NsEntry, Want, Written,
};
use egret_workflow::storage::nsfiles::{CreatedBy, NsError};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

const NS: &str = "w9b.t2";

/// One row as egret would write it: `path` in `NsPath` form, a directory's
/// ending with '/' (revision 1, size 0).
async fn row(db: &Db, ws: Uuid, ns: &str, path: &str, revision: i32, size: i64, deleted: bool) {
    let is_dir = path.ends_with('/');
    sqlx::query(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ws)
    .bind(ns)
    .bind(path)
    .bind(revision)
    .bind(is_dir)
    .bind(size)
    .bind(deleted)
    .execute(&db.pool)
    .await
    .unwrap_or_else(|e| panic!("insert {path} r{revision}: {e}"));
}

fn read_grant(ws: Uuid) -> Grant {
    authorize(ws, Actor::Api, NS, Access::Read).unwrap()
}

fn file(p: &str) -> NsPath {
    NsPath::parse_file(p).unwrap()
}

fn dir(p: &str) -> NsPath {
    NsPath::parse_dir(p).unwrap()
}

fn paths(v: &[NsEntry]) -> Vec<&str> {
    v.iter().map(|e| e.path.as_str()).collect()
}

async fn conn(db: &Db) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    db.pool.acquire().await.unwrap()
}

#[tokio::test]
async fn latest_is_the_highest_live_revision() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for (rev, size) in [(1, 10), (3, 30), (2, 20)] {
        row(&db, ws, NS, "/a/b.txt", rev, size, false).await;
    }
    let mut c = conn(&db).await;
    let g = read_grant(ws);
    let e = resolve(&mut c, &g, &file("/a/b.txt"), Want::Latest)
        .await
        .unwrap();
    assert_eq!(
        (e.revision, e.size, e.deleted, e.is_directory),
        (3, 30, false, false)
    );
    let e = resolve(&mut c, &g, &file("a/b.txt"), Want::Revision(2))
        .await
        .unwrap();
    assert_eq!((e.revision, e.size), (2, 20));
    let err = resolve(&mut c, &g, &file("/a/b.txt"), Want::Revision(4))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::NotFound(m) if m.contains("revision 4") && m.contains("/a/b.txt")),
        "{err:?}"
    );
    let err = resolve(&mut c, &g, &file("/nope"), Want::Latest)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::NotFound(m) if m.contains("'/nope'")),
        "{err:?}"
    );
}

/// R1 (w9b `layout`): revisions 1..3 written, 3 soft-deleted, then 4 written.
#[tokio::test]
async fn a_soft_deleted_path_hides_its_latest_and_serves_each_revision() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    row(&db, ws, NS, "/a/b.txt", 1, 3, false).await;
    row(&db, ws, NS, "/a/b.txt", 2, 4, false).await;
    row(&db, ws, NS, "/a/b.txt", 3, 5, true).await;
    let mut c = conn(&db).await;
    let (g, p) = (read_grant(ws), file("/a/b.txt"));
    // A latest read is not-found -- never revision 2, the older live one.
    let err = resolve(&mut c, &g, &p, Want::Latest).await.unwrap_err();
    assert!(
        matches!(&err, NsError::NotFound(m) if m.contains("/a/b.txt")),
        "{err:?}"
    );
    // Each revision is served, the deleted one included.
    let r1 = resolve(&mut c, &g, &p, Want::Revision(1)).await.unwrap();
    assert_eq!((r1.revision, r1.size, r1.deleted), (1, 3, false));
    let r3 = resolve(&mut c, &g, &p, Want::Revision(3)).await.unwrap();
    assert_eq!((r3.revision, r3.size, r3.deleted), (3, 5, true));
    // A listing does not show it either.
    assert!(list_live(&mut c, &g, None, true, 10)
        .await
        .unwrap()
        .is_empty());
    assert!(search(&mut c, &g, "b.txt", 10).await.unwrap().is_empty());
    // The rewrite: revision 4, live; revision 3 still served
    // (`layout.read_by_revision_after_rewrite.3`).
    row(&db, ws, NS, "/a/b.txt", 4, 4, false).await;
    let latest = resolve(&mut c, &g, &p, Want::Latest).await.unwrap();
    assert_eq!((latest.revision, latest.deleted), (4, false));
    let r3 = resolve(&mut c, &g, &p, Want::Revision(3)).await.unwrap();
    assert_eq!((r3.revision, r3.deleted), (3, true));
    let err = resolve(&mut c, &g, &p, Want::Revision(5))
        .await
        .unwrap_err();
    assert!(matches!(err, NsError::NotFound(_)), "{err:?}");
}

/// w9b `layout.steps[delete].revisions_api` and `[rewrite].revisions_api`.
#[tokio::test]
async fn revisions_lists_every_revision_of_a_soft_deleted_path_flagged() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let mut c = conn(&db).await;
    let (g, p) = (read_grant(ws), file("/a/b.txt"));
    // `before_any_write`: no row is not-found.
    let err = revisions(&mut c, &g, &p).await.unwrap_err();
    assert!(
        matches!(&err, NsError::NotFound(m) if m.contains("/a/b.txt")),
        "{err:?}"
    );
    row(&db, ws, NS, "/a/b.txt", 3, 5, true).await;
    row(&db, ws, NS, "/a/b.txt", 1, 3, false).await;
    row(&db, ws, NS, "/a/b.txt", 2, 4, false).await;
    let flagged = |v: Vec<NsEntry>| -> (Vec<u32>, Vec<u32>) {
        (
            v.iter().map(|e| e.revision).collect(),
            v.iter().filter(|e| e.deleted).map(|e| e.revision).collect(),
        )
    };
    assert_eq!(
        flagged(revisions(&mut c, &g, &p).await.unwrap()),
        (vec![1, 2, 3], vec![3])
    );
    row(&db, ws, NS, "/a/b.txt", 4, 4, false).await;
    assert_eq!(
        flagged(revisions(&mut c, &g, &p).await.unwrap()),
        (vec![1, 2, 3, 4], vec![3])
    );
}

#[tokio::test]
async fn the_same_path_in_two_workspaces_resolves_to_the_grants_row_only() {
    let Some(db) = common::db().await else { return };
    let (a, _) = common::seed_workspace(&db).await;
    let (b, _) = common::seed_workspace(&db).await;
    let (c_ws, _) = common::seed_workspace(&db).await;
    row(&db, a, NS, "/d/", 1, 0, false).await;
    row(&db, a, NS, "/d/x.txt", 1, 1, false).await;
    row(&db, b, NS, "/d/", 1, 0, false).await;
    row(&db, b, NS, "/d/x.txt", 1, 2, false).await;
    row(&db, b, NS, "/d/x.txt", 2, 2, false).await;
    let mut c = conn(&db).await;
    let p = file("/d/x.txt");
    let ga = read_grant(a);
    let e = resolve(&mut c, &ga, &p, Want::Latest).await.unwrap();
    assert_eq!((e.revision, e.size), (1, 1));
    assert_eq!(revisions(&mut c, &ga, &p).await.unwrap().len(), 1);
    assert_eq!(
        paths(&list_live(&mut c, &ga, None, true, 10).await.unwrap()),
        ["/d/", "/d/x.txt"]
    );
    let found = search(&mut c, &ga, "x", 10).await.unwrap();
    assert_eq!(found.iter().map(|e| e.size).collect::<Vec<_>>(), [1]);
    // Workspace C holds nothing: every read is empty or not-found, never B's.
    let gc = read_grant(c_ws);
    for want in [Want::Latest, Want::Revision(1), Want::Revision(2)] {
        let err = resolve(&mut c, &gc, &p, want).await.unwrap_err();
        assert!(matches!(err, NsError::NotFound(_)), "{want:?}: {err:?}");
    }
    assert!(matches!(
        revisions(&mut c, &gc, &p).await,
        Err(NsError::NotFound(_))
    ));
    assert!(list_live(&mut c, &gc, None, true, 10)
        .await
        .unwrap()
        .is_empty());
    assert!(search(&mut c, &gc, "x", 10).await.unwrap().is_empty());
    assert!(namespaces_with_files(&mut c, c_ws, "w9b", 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        namespaces_with_files(&mut c, a, "w9b", 10).await.unwrap(),
        [NS]
    );
}

#[tokio::test]
async fn list_live_refuses_one_past_the_cap_naming_it() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for p in ["/a", "/b", "/c"] {
        row(&db, ws, NS, p, 1, 1, false).await;
    }
    // A deleted path and older revisions are not counted.
    row(&db, ws, NS, "/gone", 1, 1, true).await;
    row(&db, ws, NS, "/a", 2, 1, false).await;
    let mut c = conn(&db).await;
    let g = read_grant(ws);
    let three = list_live(&mut c, &g, None, true, 3).await.unwrap();
    assert_eq!(paths(&three), ["/a", "/b", "/c"]);
    assert_eq!(three[0].revision, 2);
    row(&db, ws, NS, "/d", 1, 1, false).await;
    let err = list_live(&mut c, &g, None, true, 3).await.unwrap_err();
    assert!(
        matches!(&err, NsError::TooLarge(m)
            if m.contains("workflow.storage.max_namespace_files") && m.contains("more than 3")),
        "{err:?}"
    );
    assert_eq!(list_live(&mut c, &g, None, true, 4).await.unwrap().len(), 4);
    // Search is capped the same way.
    assert!(matches!(
        search(&mut c, &g, "/", 3).await,
        Err(NsError::TooLarge(_))
    ));
    assert_eq!(search(&mut c, &g, "/", 4).await.unwrap().len(), 4);
}

#[tokio::test]
async fn list_live_does_not_match_a_sibling_prefix() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for p in [
        "/a/", "/a/x", "/a/sub/", "/a/sub/y", "/ab/", "/ab/x", "/a_/", "/a_/x", "/a_b",
    ] {
        row(&db, ws, NS, p, 1, 0, false).await;
    }
    // A soft-deleted child, with an older live revision: not listed.
    row(&db, ws, NS, "/a/old", 1, 1, false).await;
    row(&db, ws, NS, "/a/old", 2, 1, true).await;
    // Another namespace's `/a/` children are not this one's.
    row(&db, ws, "w9b.other", "/a/", 1, 0, false).await;
    row(&db, ws, "w9b.other", "/a/z", 1, 0, false).await;
    let mut c = conn(&db).await;
    let g = read_grant(ws);
    let a = dir("/a/");
    assert_eq!(
        paths(&list_live(&mut c, &g, Some(&a), false, 10).await.unwrap()),
        ["/a/sub/", "/a/x"]
    );
    assert_eq!(
        paths(&list_live(&mut c, &g, Some(&a), true, 10).await.unwrap()),
        ["/a/sub/", "/a/sub/y", "/a/x"]
    );
    // `_` is a literal character, not a wildcard.
    assert_eq!(
        paths(
            &list_live(&mut c, &g, Some(&dir("/a_/")), true, 10)
                .await
                .unwrap()
        ),
        ["/a_/x"]
    );
    assert_eq!(
        paths(&list_live(&mut c, &g, None, false, 10).await.unwrap()),
        ["/a/", "/a_/", "/a_b", "/ab/"]
    );
    // Search: case-insensitive, files only, '' finds nothing (R3).
    assert_eq!(
        paths(&search(&mut c, &g, "X", 10).await.unwrap()),
        ["/a/x", "/a_/x", "/ab/x"]
    );
    assert_eq!(
        paths(&search(&mut c, &g, "SUB", 10).await.unwrap()),
        ["/a/sub/y"]
    );
    assert!(search(&mut c, &g, "", 10).await.unwrap().is_empty());
    assert!(search(&mut c, &g, "old", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_row_whose_object_is_missing_is_an_author_error_naming_path_and_revision() {
    let Some(db) = common::db().await else { return };
    let fs = storage::install_mem_for_tests();
    let (ws, _) = common::seed_workspace(&db).await;
    row(&db, ws, NS, "/r/x.txt", 1, 3, false).await;
    row(&db, ws, NS, "/r/x.txt", 2, 3, false).await;
    let mut c = conn(&db).await;
    let g = read_grant(ws);
    let p = file("/r/x.txt");
    // The control: revision 1's object is there, at `…/_files/r/x.txt`.
    let key1 = fs.namespace_object_uri(&g, &p, 1);
    assert_eq!(
        key1,
        format!("s3://{}/{}{ws}/w9b/t2/_files/r/x.txt", fs.bucket, fs.prefix)
    );
    fs.store.put(&key1, b"one".to_vec()).await.unwrap();
    let r1 = resolve(&mut c, &g, &p, Want::Revision(1)).await.unwrap();
    let mut got = String::new();
    open(&fs, &g, &r1)
        .await
        .unwrap()
        .read_to_string(&mut got)
        .await
        .unwrap();
    assert_eq!(got, "one");
    // Revision 2 (`…x.txt.v2`) has a row and no object: no fallback to 1.
    let r2 = resolve(&mut c, &g, &p, Want::Latest).await.unwrap();
    assert_eq!(r2.revision, 2);
    let err = match open(&fs, &g, &r2).await {
        Ok(_) => panic!("revision 2 has no object"),
        Err(e) => e,
    };
    match &err {
        NsError::Author(m) => {
            assert!(m.contains("/r/x.txt") && m.contains("revision 2"), "{m}");
            assert!(!m.contains("s3://") && !m.contains(&ws.to_string()), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_directory_shape_check_refuses_a_directory_with_revision_two() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let insert = |path: &'static str, rev: i32, is_dir: bool, size: i64| {
        sqlx::query(
            "INSERT INTO workflow.namespace_files \
               (workspace_id, namespace, path, revision, is_directory, size) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(ws)
        .bind(NS)
        .bind(path)
        .bind(rev)
        .bind(is_dir)
        .bind(size)
        .execute(&db.pool)
    };
    // The controls.
    insert("/d/", 1, true, 0).await.unwrap();
    insert("/f", 2, false, 9).await.unwrap();
    for (path, rev, is_dir, size) in [
        ("/d2/", 2, true, 0),  // a directory has one revision
        ("/d3/", 1, true, 5),  // and no size
        ("/f2/", 1, false, 0), // a file does not end with '/'
        ("/d4", 1, true, 0),   // a directory does
    ] {
        let err = insert(path, rev, is_dir, size).await.unwrap_err();
        let constraint = err
            .as_database_error()
            .and_then(|d| d.constraint())
            .map(str::to_string);
        assert_eq!(
            constraint.as_deref(),
            Some("workflow_namespace_files_directory_shape"),
            "{path}: {err}"
        );
    }
}

#[tokio::test]
async fn deleting_a_workspace_cascades_its_namespace_files() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (other, _) = common::seed_workspace(&db).await;
    row(&db, ws, NS, "/a", 1, 1, false).await;
    row(&db, ws, NS, "/a", 2, 1, true).await;
    row(&db, other, NS, "/a", 1, 1, false).await;
    let count = |w: Uuid| {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.namespace_files WHERE workspace_id = $1",
        )
        .bind(w)
        .fetch_one(&db.pool)
    };
    assert_eq!(count(ws).await.unwrap(), 2, "the control");
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(ws)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(count(ws).await.unwrap(), 0);
    assert_eq!(count(other).await.unwrap(), 1);
}

// -- Fix round 1: M1, M2, M7.

/// M1: a FILE as `list_live`'s directory is egret's bug, refused in every
/// build -- before, `/a` matched the sibling `/ab/x` under `left()`.
#[tokio::test]
async fn list_live_refuses_a_file_as_its_directory() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for p in ["/a/", "/a/x", "/ab/", "/ab/x"] {
        row(&db, ws, NS, p, 1, 0, false).await;
    }
    let mut c = conn(&db).await;
    let g = read_grant(ws);
    for recursive in [true, false] {
        let err = list_live(&mut c, &g, Some(&file("/a")), recursive, 10)
            .await
            .unwrap_err();
        match &err {
            NsError::Db(egret_core::Error::Internal(e)) => {
                assert!(e.to_string().contains("'/a', which is a file"), "{e}")
            }
            other => panic!("{other:?}"),
        }
    }
    // The control: the directory `/a/` lists its own child only.
    assert_eq!(
        paths(
            &list_live(&mut c, &g, Some(&dir("/a")), true, 10)
                .await
                .unwrap()
        ),
        ["/a/x"]
    );
}

/// M2: an entry resolved under one grant cannot be opened under another
/// namespace's or another workspace's grant -- which would otherwise stream
/// THAT namespace's object at the same path and revision.
#[tokio::test]
async fn open_refuses_an_entry_of_another_grant() {
    let Some(db) = common::db().await else { return };
    let fs = storage::install_mem_for_tests();
    let (a, _) = common::seed_workspace(&db).await;
    let (b, _) = common::seed_workspace(&db).await;
    row(&db, a, NS, "/s.txt", 1, 1, false).await;
    let mut c = conn(&db).await;
    let ga = read_grant(a);
    let p = file("/s.txt");
    let e = resolve(&mut c, &ga, &p, Want::Latest).await.unwrap();
    assert_eq!((e.workspace, e.namespace.as_str()), (a, NS));
    let other_ns = authorize(a, Actor::Api, "w9b.other", Access::Read).unwrap();
    let other_ws = read_grant(b);
    // Each wrong grant's object exists, so only the check can refuse it.
    for (g, body) in [
        (&ga, "mine"),
        (&other_ns, "other-ns"),
        (&other_ws, "other-ws"),
    ] {
        fs.store
            .put(&fs.namespace_object_uri(g, &p, 1), body.as_bytes().to_vec())
            .await
            .unwrap();
    }
    for g in [&other_ns, &other_ws] {
        match open(&fs, g, &e).await {
            Ok(_) => panic!("{:?} opened another grant's entry", g.namespace()),
            Err(NsError::Db(egret_core::Error::Internal(m))) => {
                assert!(m.to_string().contains("/s.txt"), "{m}")
            }
            Err(other) => panic!("{other:?}"),
        }
    }
    // The control: its own grant streams its own object.
    let mut got = String::new();
    open(&fs, &ga, &e)
        .await
        .unwrap()
        .read_to_string(&mut got)
        .await
        .unwrap();
    assert_eq!(got, "mine");
}

/// M7: revision N >= 2 lives at `….v{N}`, and `open` reads it there.
#[tokio::test]
async fn a_later_revision_is_read_from_its_v_suffixed_key() {
    let Some(db) = common::db().await else { return };
    let fs = storage::install_mem_for_tests();
    let (ws, _) = common::seed_workspace(&db).await;
    for rev in [1, 2, 10] {
        row(&db, ws, NS, "/r/x.txt", rev, 3, false).await;
    }
    let mut c = conn(&db).await;
    let (g, p) = (read_grant(ws), file("/r/x.txt"));
    let base = format!("s3://{}/{}{ws}/w9b/t2/_files/r/x.txt", fs.bucket, fs.prefix);
    assert_eq!(fs.namespace_object_uri(&g, &p, 1), base);
    assert_eq!(fs.namespace_object_uri(&g, &p, 2), format!("{base}.v2"));
    assert_eq!(fs.namespace_object_uri(&g, &p, 10), format!("{base}.v10"));
    for (rev, body) in [(1, "one"), (2, "two"), (10, "ten")] {
        let key = if rev == 1 {
            base.clone()
        } else {
            format!("{base}.v{rev}")
        };
        fs.store.put(&key, body.as_bytes().to_vec()).await.unwrap();
    }
    for (want, body) in [
        (Want::Revision(1), "one"),
        (Want::Revision(2), "two"),
        (Want::Latest, "ten"),
    ] {
        let e = resolve(&mut c, &g, &p, want).await.unwrap();
        let mut got = String::new();
        open(&fs, &g, &e)
            .await
            .unwrap()
            .read_to_string(&mut got)
            .await
            .unwrap();
        assert_eq!(got, body, "{want:?}");
    }
}

/// A MemStore whose streamed read fails with something other than NotFound.
struct FailingReads(egret_storage::MemStore);

#[async_trait::async_trait]
impl egret_storage::ObjectStore for FailingReads {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), egret_storage::StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, egret_storage::StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), egret_storage::StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), egret_storage::StorageError> {
        self.0.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, egret_storage::StorageError> {
        self.0.list(prefix, delimiter).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3(format!(
            "connection reset reading {uri} (secret-detail)"
        )))
    }
}

/// M7: a store error other than NotFound is `NsError::Store` -- an outage,
/// not the author's -- and its message names neither the key nor the
/// store's own words.
#[tokio::test]
async fn a_store_outage_on_open_is_a_store_error_that_names_no_key() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    row(&db, ws, NS, "/o.txt", 1, 1, false).await;
    row(&db, ws, NS, "/o.txt", 2, 1, false).await;
    let mem = egret_storage::MemStore::new();
    let fs = storage::FileStore {
        store: std::sync::Arc::new(FailingReads(mem)),
        bucket: "b".into(),
        prefix: "workflow/".into(),
        caps: storage::Caps::default(),
        retention_days: 30,
    };
    let mut c = conn(&db).await;
    let (g, p) = (read_grant(ws), file("/o.txt"));
    // The object is there: only the read fails.
    let key = fs.namespace_object_uri(&g, &p, 2);
    fs.store.put(&key, b"x".to_vec()).await.unwrap();
    let e = resolve(&mut c, &g, &p, Want::Latest).await.unwrap();
    let err = match open(&fs, &g, &e).await {
        Ok(_) => panic!("the store fails every streamed read"),
        Err(err) => err,
    };
    match &err {
        NsError::Store(m) => {
            assert!(m.contains("/o.txt") && m.contains("revision 2"), "{m}");
            for leak in [
                "s3://",
                "secret-detail",
                "connection reset",
                &ws.to_string(),
            ] {
                assert!(!m.contains(leak), "{m:?} leaks {leak:?}");
            }
        }
        other => panic!("{other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// W9b Task 3: the write side
// ═══════════════════════════════════════════════════════════════════════════

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use egret_storage::{MemStore, ObjectStore, StorageError};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::oneshot;

const W: &str = "w9b.t3";

fn grant(ws: Uuid, access: Access) -> Grant {
    authorize(ws, Actor::Api, W, access).unwrap()
}

fn by() -> CreatedBy {
    CreatedBy::Execution(Uuid::nil())
}

/// A store hooked for the tests below; with nothing armed it is a `MemStore`.
#[derive(Default)]
struct Hooked {
    mem: MemStore,
    /// `put_stream` number N (1-based) fails; 0 = none.
    fail_put_stream_at: AtomicUsize,
    put_streams: AtomicUsize,
    /// Every `delete`, in order.
    deletes: Mutex<Vec<String>>,
    fail_deletes: AtomicBool,
    /// The next `delete` says it started and waits to be released.
    gate_delete: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// The next `put_stream` says it started and waits to be released
    /// (fix round 1: a move paused mid-copy).
    gate_put_stream: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// Each `copy` takes the first gate queued, says it started and waits to
    /// be released (Task 8 fix round 1: a `put` paused under its lock, mid
    /// staging -> revision copy).
    gate_copy: Mutex<std::collections::VecDeque<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// Every `copy` fails.
    fail_copies: AtomicBool,
}

impl Hooked {
    fn bytes(&self, key: &str) -> Option<String> {
        let keys = self.mem.keys();
        keys.contains(&key.to_string()).then(|| {
            let mem = self.mem.clone();
            let key = key.to_string();
            String::from_utf8(futures_lite_block(
                async move { mem.get(&key).await.unwrap() },
            ))
            .unwrap()
        })
    }

    /// Every key under this workspace's namespace-file prefix, with its bytes.
    fn objects(&self, fs: &storage::FileStore, ws: Uuid) -> Vec<(String, String)> {
        let prefix = format!("s3://{}/{}{ws}/", fs.bucket, fs.prefix);
        self.mem
            .keys()
            .into_iter()
            .filter(|k| k.starts_with(&prefix))
            .map(|k| {
                let b = self.bytes(&k).unwrap();
                (k[prefix.len()..].to_string(), b)
            })
            .collect()
    }

    /// Arms the `put_stream` gate: answers (started, release).
    fn arm_put_stream_gate(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.gate_put_stream.lock().unwrap() = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    /// Queues a copy gate: answers (started, release).
    fn arm_copy_gate(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        self.gate_copy
            .lock()
            .unwrap()
            .push_back((started_tx, release_rx));
        (started_rx, release_tx)
    }

    /// Drops every copy gate no copy took.
    fn clear_copy_gates(&self) {
        self.gate_copy.lock().unwrap().clear();
    }

    /// Arms the delete gate: answers (started, release).
    fn arm_delete_gate(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.gate_delete.lock().unwrap() = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }
}

/// `MemStore::get` never awaits anything: poll it once.
fn futures_lite_block<T>(f: impl std::future::Future<Output = T>) -> T {
    let mut f = std::pin::pin!(f);
    let waker = std::task::Waker::noop();
    match f.as_mut().poll(&mut Context::from_waker(waker)) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("MemStore awaited"),
    }
}

#[async_trait::async_trait]
impl ObjectStore for Hooked {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.mem.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.mem.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.deletes.lock().unwrap().push(uri.to_string());
        let gate = self.gate_delete.lock().unwrap().take();
        if let Some((started, release)) = gate {
            let _ = started.send(());
            let _ = release.await;
        }
        if self.fail_deletes.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!("{uri}: the store is down")));
        }
        self.mem.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        let gate = self.gate_copy.lock().unwrap().pop_front();
        if let Some((started, release)) = gate {
            let _ = started.send(());
            let _ = release.await;
        }
        if self.fail_copies.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!("{dst}: the copy failed")));
        }
        self.mem.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.mem.list(prefix, delimiter).await
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        let n = self.put_streams.fetch_add(1, Ordering::SeqCst) + 1;
        let gate = self.gate_put_stream.lock().unwrap().take();
        if let Some((started, release)) = gate {
            let _ = started.send(());
            let _ = release.await;
        }
        if n == self.fail_put_stream_at.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!("{uri}: put number {n} failed")));
        }
        self.mem.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        self.mem.get_stream(uri).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.mem.head(uri).await
    }
}

fn hooked_fs(max_file_bytes: Option<u64>) -> (Arc<Hooked>, storage::FileStore) {
    let store = Arc::new(Hooked::default());
    let mut caps = storage::Caps::default();
    if let Some(cap) = max_file_bytes {
        caps.max_file_bytes = cap;
    }
    let fs = storage::FileStore {
        store: store.clone(),
        bucket: "egret-test".into(),
        prefix: "workflow/".into(),
        caps,
        retention_days: 30,
    };
    (store, fs)
}

async fn write(
    db: &Db,
    fs: &storage::FileStore,
    g: &Grant,
    p: &str,
    bytes: &str,
    conflict: Conflict,
) -> Result<Written, NsError> {
    let mut body: &[u8] = bytes.as_bytes();
    put(&db.pool, fs, g, &file(p), &mut body, conflict, by()).await
}

async fn written(db: &Db, fs: &storage::FileStore, g: &Grant, p: &str, bytes: &str) -> NsEntry {
    match write(db, fs, g, p, bytes, Conflict::Overwrite).await {
        Ok(Written::Written(e)) => e,
        other => panic!("{p}: {other:?}"),
    }
}

/// `(revision, deleted)` of every row of `p` (stored form), ascending.
async fn rows_of(db: &Db, ws: Uuid, p: &str) -> Vec<(i32, bool)> {
    sqlx::query_as(
        "SELECT revision, deleted FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND path = $3 ORDER BY revision",
    )
    .bind(ws)
    .bind(W)
    .bind(p)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// Every row of the namespace: (path, revision, deleted, size), by path.
async fn all_rows(db: &Db, ws: Uuid) -> Vec<(String, i32, bool, i64)> {
    sqlx::query_as(
        "SELECT path, revision, deleted, size FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 ORDER BY path COLLATE \"C\", revision",
    )
    .bind(ws)
    .bind(W)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

async fn read_rev(fs: &storage::FileStore, db: &Db, g: &Grant, p: &str, want: Want) -> String {
    let mut c = conn(db).await;
    let e = resolve(&mut c, g, &file(p), want).await.unwrap();
    let mut s = String::new();
    open(fs, g, &e)
        .await
        .unwrap()
        .read_to_string(&mut s)
        .await
        .unwrap();
    s
}

/// A body that hands over `first`, says so on `started`, waits for `release`,
/// then hands over `rest`.
struct Gated {
    first: Option<Vec<u8>>,
    rest: Vec<u8>,
    started: Option<oneshot::Sender<()>>,
    release: Option<oneshot::Receiver<()>>,
}

impl AsyncRead for Gated {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(first) = self.first.take() {
            buf.put_slice(&first);
            if let Some(tx) = self.started.take() {
                let _ = tx.send(());
            }
            return Poll::Ready(Ok(()));
        }
        if let Some(rx) = self.release.as_mut() {
            match Pin::new(rx).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(_) => self.release = None,
            }
        }
        let rest = std::mem::take(&mut self.rest);
        buf.put_slice(&rest);
        Poll::Ready(Ok(()))
    }
}

use std::future::Future as _;

/// A gated body and its two handles: (body, started, release).
fn gated(first: &str, rest: &str) -> (Gated, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    (
        Gated {
            first: Some(first.as_bytes().to_vec()),
            rest: rest.as_bytes().to_vec(),
            started: Some(started_tx),
            release: Some(release_rx),
        },
        started_rx,
        release_tx,
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Seen {
    /// `pg_locks` shows a session waiting on the key.
    Blocked,
    /// The writer's body was read: nothing stopped it.
    Started,
}

/// Waits until a session waits on the advisory key of `lock_path` (the FIRST
/// key the second writer takes: its outermost parent, or itself at the root),
/// or until `started` fires -- whichever happens. Polling `pg_locks` waits for a
/// state; nothing here depends on how long anything takes.
async fn blocked_or_started(
    db: &Db,
    ws: Uuid,
    lock_path: &str,
    started: &mut oneshot::Receiver<()>,
) -> Seen {
    let key: i64 = sqlx::query_scalar("SELECT hashtextextended($1, 0)")
        .bind(format!("w9b:{ws}:{W}:{lock_path}"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let waiting = async {
        loop {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
                   AND ((classid::bigint << 32) | objid::bigint) = $1",
            )
            .bind(key)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            if n > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    };
    // A guard against a hang, not a measurement: either event comes at once
    // when the code is right, and a wrong key here would otherwise wait for ever.
    let either = async {
        tokio::select! {
            _ = started => Seen::Started,
            () = waiting => Seen::Blocked,
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(60), either)
        .await
        .unwrap_or_else(|_| panic!("neither a wait on {lock_path:?} nor the body's start was seen"))
}

fn spawn_put<B: AsyncRead + Send + Unpin + 'static>(
    db: &Db,
    fs: &Arc<storage::FileStore>,
    g: &Grant,
    p: &str,
    mut body: B,
) -> tokio::task::JoinHandle<Result<Written, NsError>> {
    let (pool, fs, g, p) = (db.pool.clone(), fs.clone(), g.clone(), file(p));
    tokio::spawn(async move { put(&pool, &fs, &g, &p, &mut body, Conflict::Overwrite, by()).await })
}

/// A body handed over at once.
fn bytes(b: &'static str) -> &'static [u8] {
    b.as_bytes()
}

/// THE lock test (§4.2). Writer A holds the path's lock mid-write (its
/// staging -> revision copy) when writer B starts; B must wait, then take
/// the next revision.
#[tokio::test(flavor = "multi_thread")]
async fn two_concurrent_writers_of_one_path_get_distinct_revisions_and_keys() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    let mut blocked = 0;
    for round in 0..20 {
        // At the root: the path is its only lock key, the one B waits on.
        let p = format!("/race-{round}.txt");
        written(&db, &fs, &g, &p, "initial").await;
        let (started_a, release_a) = store.arm_copy_gate();
        let a = spawn_put(&db, &fs, &g, &p, bytes("A-a"));
        started_a
            .await
            .expect("A copies under its lock: A holds it");
        let (mut started_b, release_b) = store.arm_copy_gate();
        let b = spawn_put(&db, &fs, &g, &p, bytes("B-b"));
        let seen = blocked_or_started(&db, ws, &p, &mut started_b).await;
        release_a.send(()).unwrap();
        let _ = release_b.send(());
        let (ra, rb) = (a.await.unwrap(), b.await.unwrap());
        let (ea, eb) = match (ra, rb) {
            (Ok(Written::Written(ea)), Ok(Written::Written(eb))) => (ea, eb),
            other => panic!("round {round}: both writers must succeed: {other:?}"),
        };
        assert_eq!(
            (ea.revision, eb.revision),
            (2, 3),
            "round {round}: A took 2 while holding the lock, B the next"
        );
        let (ka, kb) = (
            fs.namespace_object_uri(&g, &file(&p), ea.revision),
            fs.namespace_object_uri(&g, &file(&p), eb.revision),
        );
        assert_ne!(ka, kb, "round {round}");
        assert_eq!(store.bytes(&ka).as_deref(), Some("A-a"), "round {round}");
        assert_eq!(store.bytes(&kb).as_deref(), Some("B-b"), "round {round}");
        assert_eq!(
            rows_of(&db, ws, &p).await,
            [(1, false), (2, false), (3, false)]
        );
        assert_eq!(
            seen,
            Seen::Blocked,
            "round {round}: B was not stopped by the lock"
        );
        blocked += 1;
    }
    assert_eq!(blocked, 20);
}

/// `/a/x` locks `/a` as a parent and the file `/a` locks `/a`: the same key
/// (`lock_path`), so one waits and then sees the other's row. Both orders.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_of_a_child_and_a_writer_of_a_file_named_like_its_parent_never_both_win() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    for round in 0..20 {
        let parent = format!("/p{round}");
        let child = format!("{parent}/x");
        let child_first = round % 2 == 0;
        let (first, second) = if child_first {
            (&child, &parent)
        } else {
            (&parent, &child)
        };
        let (started_1, release_1) = store.arm_copy_gate();
        let h1 = spawn_put(&db, &fs, &g, first, bytes("1"));
        started_1.await.unwrap();
        let (mut started_2, release_2) = store.arm_copy_gate();
        let h2 = spawn_put(&db, &fs, &g, second, bytes("2"));
        let seen = blocked_or_started(&db, ws, &parent, &mut started_2).await;
        release_1.send(()).unwrap();
        let _ = release_2.send(());
        let (r1, r2) = (h1.await.unwrap(), h2.await.unwrap());
        // The loser never reaches its copy: its gate is left unused.
        store.clear_copy_gates();
        let won = [&r1, &r2]
            .iter()
            .filter(|r| matches!(r, Ok(Written::Written(_))))
            .count();
        assert_eq!(won, 1, "round {round}: exactly one wins: {r1:?} / {r2:?}");
        // The one that waited lost, naming its path.
        match &r2 {
            Err(NsError::Conflict(m)) => {
                assert!(m.contains(&format!("'{second}'")), "round {round}: {m}");
                let why = if child_first {
                    "a directory already exists"
                } else {
                    "is a file"
                };
                assert!(m.contains(why), "round {round}: {m}");
            }
            other => panic!("round {round}: {other:?}"),
        }
        assert_eq!(seen, Seen::Blocked, "round {round}");
    }
}

/// w9b `layout`: revision 1 at the path, then `.v2`, `.v3`; the parent
/// directory has a row and no object.
#[tokio::test]
async fn revisions_number_one_then_v2_v3_as_the_artifact_lays_them_out() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    for (want, body) in [(1, "one"), (2, "two!"), (3, "three")] {
        let e = written(&db, &fs, &g, "/a/b.txt", body).await;
        assert_eq!(
            (e.revision, e.size, e.deleted),
            (want, body.len() as u64, false)
        );
        assert_eq!((e.workspace, e.namespace.as_str()), (ws, W));
    }
    let base = "w9b/t3/_files/a/b.txt";
    assert_eq!(
        store.objects(&fs, ws),
        [
            (base.to_string(), "one".to_string()),
            (format!("{base}.v2"), "two!".to_string()),
            (format!("{base}.v3"), "three".to_string()),
        ]
    );
    assert_eq!(
        all_rows(&db, ws).await,
        [
            ("/a/".into(), 1, false, 0),
            ("/a/b.txt".into(), 1, false, 3),
            ("/a/b.txt".into(), 2, false, 4),
            ("/a/b.txt".into(), 3, false, 5),
        ]
    );
    assert_eq!(
        read_rev(&fs, &db, &g, "/a/b.txt", Want::Latest).await,
        "three"
    );
    assert_eq!(
        read_rev(&fs, &db, &g, "/a/b.txt", Want::Revision(2)).await,
        "two!"
    );
}

#[tokio::test]
async fn conflict_error_and_skip() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/up/m.txt", "first").await;
    let err = write(&db, &fs, &g, "/up/m.txt", "second", Conflict::Error)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("'/up/m.txt'") && m.contains("ERROR") && m.contains(W)),
        "{err:?}"
    );
    let skipped = write(&db, &fs, &g, "/up/m.txt", "third", Conflict::Skip)
        .await
        .unwrap();
    assert_eq!(skipped, Written::Skipped(file("/up/m.txt")));
    // Nothing written by either: one row, one object, its bytes.
    assert_eq!(rows_of(&db, ws, "/up/m.txt").await, [(1, false)]);
    assert_eq!(
        store.objects(&fs, ws),
        [("w9b/t3/_files/up/m.txt".to_string(), "first".to_string())]
    );
    // The controls: a new path is written whatever the rule, and so is a
    // soft-deleted one (it does not exist to a reader).
    for (p, c) in [
        ("/up/n.txt", Conflict::Error),
        ("/up/o.txt", Conflict::Skip),
    ] {
        assert!(matches!(
            write(&db, &fs, &g, p, "x", c).await,
            Ok(Written::Written(_))
        ));
    }
    soft_delete(&db.pool, &g, &file("/up/m.txt")).await.unwrap();
    let e = match write(&db, &fs, &g, "/up/m.txt", "again", Conflict::Error).await {
        Ok(Written::Written(e)) => e,
        other => panic!("{other:?}"),
    };
    assert_eq!(e.revision, 2);
}

/// R1, w9b `layout`: three revisions, a delete, a rewrite.
#[tokio::test]
async fn rewriting_a_soft_deleted_path_creates_the_next_revision_live() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (_store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    for body in ["one", "two!", "three"] {
        written(&db, &fs, &g, "/a/b.txt", body).await;
    }
    let marked = soft_delete(&db.pool, &g, &file("/a/b.txt")).await.unwrap();
    assert_eq!(
        marked
            .iter()
            .map(|e| (e.path.as_str(), e.revision, e.deleted))
            .collect::<Vec<_>>(),
        [("/a/b.txt", 3, true)]
    );
    let mut c = conn(&db).await;
    assert!(matches!(
        resolve(&mut c, &g, &file("/a/b.txt"), Want::Latest).await,
        Err(NsError::NotFound(_))
    ));
    let e = written(&db, &fs, &g, "/a/b.txt", "four").await;
    assert_eq!((e.revision, e.deleted), (4, false));
    let revs = revisions(&mut c, &g, &file("/a/b.txt")).await.unwrap();
    assert_eq!(
        revs.iter()
            .map(|e| (e.revision, e.deleted))
            .collect::<Vec<_>>(),
        [(1, false), (2, false), (3, true), (4, false)]
    );
    // `layout.read_by_revision_after_rewrite`.
    for (n, body) in [(1, "one"), (2, "two!"), (3, "three"), (4, "four")] {
        assert_eq!(
            read_rev(&fs, &db, &g, "/a/b.txt", Want::Revision(n)).await,
            body
        );
    }
    assert_eq!(
        read_rev(&fs, &db, &g, "/a/b.txt", Want::Latest).await,
        "four"
    );
}

/// §4.2 step 4: a live directory where a file is written is a conflict
/// naming the path; a soft-deleted directory with nothing under it has no
/// history (no object, no revisions), so its row is removed and the file is
/// written.
#[tokio::test]
async fn a_live_directory_where_a_file_is_written_is_a_conflict_and_a_lone_deleted_one_is_removed()
{
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (_store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);

    create_directory(&db.pool, &g, &dir("/d"), by())
        .await
        .unwrap();
    let err = write(&db, &fs, &g, "/d", "x", Conflict::Overwrite)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("'/d'") && m.contains("a directory already exists")),
        "{err:?}"
    );
    assert_eq!(rows_of(&db, ws, "/d").await, []);
    soft_delete(&db.pool, &g, &dir("/d")).await.unwrap();
    assert_eq!(written(&db, &fs, &g, "/d", "file").await.revision, 1);
    assert_eq!(
        rows_of(&db, ws, "/d/").await,
        [],
        "the lone deleted directory row was removed"
    );
}

/// The user's ruling (2026-10-05), a directory over a file: a live file is a
/// conflict; a soft-deleted file still has its revisions, so creating the
/// directory is refused naming it, and nothing is purged. `PurgeFiles`
/// (`purge_revisions`) first, then the directory can be created.
#[tokio::test]
async fn a_directory_over_a_soft_deleted_file_is_refused_until_it_is_purged() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/f", "f1").await;
    written(&db, &fs, &g, "/f", "f2").await;
    let err = create_directory(&db.pool, &g, &dir("/f"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("'/f'") && m.contains("a file already exists")),
        "{err:?}"
    );

    soft_delete(&db.pool, &g, &file("/f")).await.unwrap();
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    let err = create_directory(&db.pool, &g, &dir("/f"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("'/f'") && m.contains("deleted file") && m.contains("purge it first")),
        "{err:?}"
    );
    assert_eq!(
        (all_rows(&db, ws).await, store.objects(&fs, ws)),
        before,
        "nothing was purged"
    );
    for (n, body) in [(1, "f1"), (2, "f2")] {
        assert_eq!(read_rev(&fs, &db, &g, "/f", Want::Revision(n)).await, body);
    }

    // To reuse the path, the author purges first.
    let p = grant(ws, Access::Purge);
    assert_eq!(
        purge_revisions(&db.pool, &fs, &p, &file("/f"), &[1, 2])
            .await
            .unwrap(),
        2
    );
    let d = create_directory(&db.pool, &g, &dir("/f"), by())
        .await
        .unwrap();
    assert_eq!(
        (d.path.as_str(), d.is_directory, d.deleted),
        ("/f/", true, false)
    );
}

/// The user's ruling (2026-10-05), a parent directory over a file: a file
/// under `/p`, or a directory under it, needs the directory `/p/`. A live
/// file `/p` is a conflict; a soft-deleted one keeps its history, so both
/// writes are refused naming it and nothing is purged. After `PurgeFiles`
/// the write goes through and makes `/p/`.
#[tokio::test]
async fn a_write_under_a_soft_deleted_file_is_refused_until_it_is_purged() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/p", "p").await;
    let err = write(&db, &fs, &g, "/p/x", "x", Conflict::Overwrite)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("'/p/x'") && m.contains("its parent '/p' is a file")),
        "{err:?}"
    );

    soft_delete(&db.pool, &g, &file("/p")).await.unwrap();
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    let err = write(&db, &fs, &g, "/p/x", "x", Conflict::Overwrite)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("'/p/x'") && m.contains("its parent '/p' is a deleted file")
                && m.contains("purge it first")),
        "{err:?}"
    );
    let err = create_directory(&db.pool, &g, &dir("/p/g"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("'/p/g'") && m.contains("its parent '/p' is a deleted file")),
        "{err:?}"
    );
    assert_eq!(
        (all_rows(&db, ws).await, store.objects(&fs, ws)),
        before,
        "nothing was purged"
    );
    assert_eq!(read_rev(&fs, &db, &g, "/p", Want::Revision(1)).await, "p");

    let p = grant(ws, Access::Purge);
    assert_eq!(
        purge_revisions(&db.pool, &fs, &p, &file("/p"), &[1])
            .await
            .unwrap(),
        1
    );
    written(&db, &fs, &g, "/p/x", "x").await;
    assert_eq!(rows_of(&db, ws, "/p").await, []);
    assert_eq!(rows_of(&db, ws, "/p/").await, [(1, false)]);
}

#[tokio::test]
async fn parents_are_created_and_an_existing_parent_is_left_alone() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    let u = CreatedBy::User(user.0);
    let execution = Uuid::new_v4();
    let who = |p: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<
                _,
                (
                    Option<Uuid>,
                    Option<Uuid>,
                    chrono::DateTime<chrono::Utc>,
                    bool,
                ),
            >(
                "SELECT created_by_user, created_by_execution, created_at, deleted \
                   FROM workflow.namespace_files \
                  WHERE workspace_id = $1 AND namespace = $2 AND path = $3",
            )
            .bind(ws)
            .bind(W)
            .bind(p)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // `layout.create_directory`: `/d1/d2` makes both rows, no object.
    let d2 = create_directory(&db.pool, &g, &dir("d1/d2"), u)
        .await
        .unwrap();
    assert_eq!(d2.path.as_str(), "/d1/d2/");
    assert_eq!(
        all_rows(&db, ws).await,
        [
            ("/d1/".into(), 1, false, 0),
            ("/d1/d2/".into(), 1, false, 0)
        ]
    );
    assert!(store.objects(&fs, ws).is_empty());
    // Idempotent.
    let again = create_directory(&db.pool, &g, &dir("/d1/d2/"), by())
        .await
        .unwrap();
    assert_eq!(again, d2);

    let before = who("/d1/").await;
    let mut body: &[u8] = b"z";
    put(
        &db.pool,
        &fs,
        &g,
        &file("/d1/e/z.txt"),
        &mut body,
        Conflict::Overwrite,
        CreatedBy::Execution(execution),
    )
    .await
    .unwrap();
    assert_eq!(
        who("/d1/").await,
        before,
        "an existing parent is left alone"
    );
    let (u_e, x_e, _, _) = who("/d1/e/").await;
    assert_eq!(
        (u_e, x_e),
        (None, Some(execution)),
        "the new parent is the writer's"
    );
    assert_eq!(before.0, Some(user.0));

    // A soft-deleted parent is live again once something is written under it.
    soft_delete(&db.pool, &g, &dir("/d1")).await.unwrap();
    written(&db, &fs, &g, "/d1/e/w.txt", "w").await;
    let mut c = conn(&db).await;
    let live = list_live(&mut c, &g, None, true, 100).await.unwrap();
    assert_eq!(paths(&live), ["/d1/", "/d1/e/", "/d1/e/w.txt"]);
    assert!(who("/d1/d2/").await.3, "a sibling stays deleted");
    assert_eq!(rows_of(&db, ws, "/d1/e/z.txt").await, [(1, true)]);
}

/// A body that hands over `n` bytes, then fails.
struct Breaks(usize);

impl AsyncRead for Breaks {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.0 == 0 {
            return Poll::Ready(Err(std::io::Error::other("the body broke (detail-xyz)")));
        }
        let n = self.0.min(buf.remaining());
        buf.put_slice(&b"abc"[..n]);
        self.0 -= n;
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_failed_upload_rolls_back_and_the_next_write_reuses_the_revision() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/f.txt", "one").await;
    // An orphan at revision 2's key, as a failed multipart upload might leave.
    let v2 = fs.namespace_object_uri(&g, &file("/f.txt"), 2);
    store.mem.put(&v2, b"orphan".to_vec()).await.unwrap();
    let mut body = Breaks(3);
    let err = put(
        &db.pool,
        &fs,
        &g,
        &file("/f.txt"),
        &mut body,
        Conflict::Overwrite,
        by(),
    )
    .await
    .unwrap_err();
    match &err {
        NsError::Store(m) => {
            assert!(m.contains("'/f.txt'"), "{m}");
            for leak in ["s3://", "detail-xyz"] {
                assert!(!m.contains(leak), "{m} leaks {leak}");
            }
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rows_of(&db, ws, "/f.txt").await, [(1, false)], "no row");
    let e = written(&db, &fs, &g, "/f.txt", "two").await;
    assert_eq!(e.revision, 2, "the same revision again");
    assert_eq!(
        store.bytes(&v2).as_deref(),
        Some("two"),
        "the orphan is overwritten"
    );
}

#[tokio::test]
async fn a_file_over_max_file_bytes_is_refused_naming_the_cap_and_leaves_no_row() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(Some(4));
    let g = grant(ws, Access::Write);
    let err = write(&db, &fs, &g, "/big/x.bin", "12345", Conflict::Overwrite)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::TooLarge(m)
            if m.contains("workflow.storage.max_file_bytes") && m.contains(" 4 bytes") && m.contains("'/big/x.bin'")),
        "{err:?}"
    );
    assert_eq!(all_rows(&db, ws).await, [], "no row, not even the parent's");
    assert!(store.objects(&fs, ws).is_empty());
    // The control: at the cap.
    assert_eq!(written(&db, &fs, &g, "/big/x.bin", "1234").await.size, 4);
}

#[tokio::test]
async fn soft_delete_marks_the_path_and_every_child_in_one_statement() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/d/a", "a1").await;
    written(&db, &fs, &g, "/d/a", "a2").await;
    written(&db, &fs, &g, "/d/b/c", "c").await;
    written(&db, &fs, &g, "/e", "e").await;
    let objects = store.objects(&fs, ws);
    let marked = soft_delete(&db.pool, &g, &dir("/d")).await.unwrap();
    assert_eq!(
        marked
            .iter()
            .map(|e| (e.path.as_str(), e.revision, e.deleted))
            .collect::<Vec<_>>(),
        [
            ("/d/", 1, true),
            ("/d/a", 2, true),
            ("/d/b/", 1, true),
            ("/d/b/c", 1, true)
        ]
    );
    // Only each path's latest row is marked; objects are kept.
    assert_eq!(rows_of(&db, ws, "/d/a").await, [(1, false), (2, true)]);
    assert_eq!(rows_of(&db, ws, "/e").await, [(1, false)]);
    assert_eq!(store.objects(&fs, ws), objects);
    let mut c = conn(&db).await;
    assert_eq!(
        paths(&list_live(&mut c, &g, None, true, 10).await.unwrap()),
        ["/e"]
    );
    // Idempotent, and a missing path is not an error (§9).
    assert!(soft_delete(&db.pool, &g, &dir("/d"))
        .await
        .unwrap()
        .is_empty());
    assert!(soft_delete(&db.pool, &g, &file("/nope"))
        .await
        .unwrap()
        .is_empty());
    // A file path takes nothing under a same-named directory.
    written(&db, &fs, &g, "/k/x", "x").await;
    assert!(soft_delete(&db.pool, &g, &file("/k"))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(rows_of(&db, ws, "/k/x").await, [(1, false)]);
}

/// `left(path, length($3)) = $3`, never `LIKE`: `_` is a wildcard there.
#[tokio::test]
async fn soft_delete_does_not_touch_a_sibling_with_a_shared_prefix() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (_store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    for p in ["/a/x", "/ab/x", "/a_/x", "/a_b", "/aXb"] {
        written(&db, &fs, &g, p, "x").await;
    }
    let marked = soft_delete(&db.pool, &g, &dir("/a_")).await.unwrap();
    assert_eq!(paths(&marked), ["/a_/", "/a_/x"]);
    let marked = soft_delete(&db.pool, &g, &dir("/a")).await.unwrap();
    assert_eq!(paths(&marked), ["/a/", "/a/x"]);
    let marked = soft_delete(&db.pool, &g, &file("/a_b")).await.unwrap();
    assert_eq!(paths(&marked), ["/a_b"]);
    let mut c = conn(&db).await;
    assert_eq!(
        paths(&list_live(&mut c, &g, None, true, 10).await.unwrap()),
        ["/aXb", "/ab/", "/ab/x"]
    );
}

#[tokio::test]
async fn move_refuses_a_live_target() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/m/a", "a").await;
    written(&db, &fs, &g, "/m/b", "b").await;
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    let err = move_path(&db.pool, &fs, &g, &file("/m/a"), &file("/m/b"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("'/m/b' already exists") && m.contains("'/m/a'")),
        "{err:?}"
    );
    // A live directory of the other kind at the target, and a live file
    // where the target's parent goes.
    create_directory(&db.pool, &g, &dir("/m/c"), by())
        .await
        .unwrap();
    let err = move_path(&db.pool, &fs, &g, &file("/m/a"), &file("/m/c"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("'/m/c/' already exists")),
        "{err:?}"
    );
    let err = move_path(&db.pool, &fs, &g, &file("/m/a"), &file("/m/b/z"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("its parent '/m/b' is a file")),
        "{err:?}"
    );
    // A missing source: 404 naming it (application of R3), nothing written.
    let err = move_path(
        &db.pool,
        &fs,
        &g,
        &file("/nope.txt"),
        &file("/nope2.txt"),
        by(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, NsError::NotFound(m) if m.contains("'/nope.txt'")),
        "{err:?}"
    );
    // Under itself.
    let err = move_path(&db.pool, &fs, &g, &dir("/m"), &dir("/m/inner"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Author(m) if m.contains("under itself")),
        "{err:?}"
    );
    let mut rows = before.0.clone();
    rows.push(("/m/c/".into(), 1, false, 0));
    rows.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()).then(a.1.cmp(&b.1)));
    assert_eq!(
        (all_rows(&db, ws).await, store.objects(&fs, ws)),
        (rows, before.1)
    );
    // The control: a fresh target, the move goes.
    let moved = move_path(&db.pool, &fs, &g, &file("/m/a"), &file("/m/fresh"), by())
        .await
        .unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(rows_of(&db, ws, "/m/fresh").await, [(1, false)]);
    assert_eq!(read_rev(&fs, &db, &g, "/m/fresh", Want::Latest).await, "a");
}

#[tokio::test]
async fn move_copies_every_revision_and_child_then_purges_the_source() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    for body in ["f1", "f2", "f3"] {
        written(&db, &fs, &g, "/src/f", body).await;
    }
    written(&db, &fs, &g, "/src/sub/g", "g1").await;
    written(&db, &fs, &g, "/src/gone", "x").await;
    soft_delete(&db.pool, &g, &file("/src/gone")).await.unwrap();
    written(&db, &fs, &g, "/srcx", "neighbour").await;
    let pairs = move_path(&db.pool, &fs, &g, &dir("/src"), &dir("/dst/in"), by())
        .await
        .unwrap();
    assert_eq!(
        pairs
            .iter()
            .map(|(s, d)| (
                s.path.as_str(),
                d.path.as_str(),
                s.revision,
                d.revision,
                d.deleted
            ))
            .collect::<Vec<_>>(),
        [
            ("/src/", "/dst/in/", 1, 1, false),
            ("/src/f", "/dst/in/f", 1, 1, false),
            ("/src/f", "/dst/in/f", 2, 2, false),
            ("/src/f", "/dst/in/f", 3, 3, false),
            ("/src/gone", "/dst/in/gone", 1, 1, true),
            ("/src/sub/", "/dst/in/sub/", 1, 1, false),
            ("/src/sub/g", "/dst/in/sub/g", 1, 1, false),
        ]
    );
    assert_eq!(
        all_rows(&db, ws).await,
        [
            ("/dst/".into(), 1, false, 0),
            ("/dst/in/".into(), 1, false, 0),
            ("/dst/in/f".into(), 1, false, 2),
            ("/dst/in/f".into(), 2, false, 2),
            ("/dst/in/f".into(), 3, false, 2),
            ("/dst/in/gone".into(), 1, true, 1),
            ("/dst/in/sub/".into(), 1, false, 0),
            ("/dst/in/sub/g".into(), 1, false, 2),
            ("/srcx".into(), 1, false, 9),
        ]
    );
    let base = "w9b/t3/_files";
    assert_eq!(
        store.objects(&fs, ws),
        [
            (format!("{base}/dst/in/f"), "f1".into()),
            (format!("{base}/dst/in/f.v2"), "f2".into()),
            (format!("{base}/dst/in/f.v3"), "f3".into()),
            (format!("{base}/dst/in/gone"), "x".into()),
            (format!("{base}/dst/in/sub/g"), "g1".into()),
            (format!("{base}/srcx"), "neighbour".into()),
        ]
    );
    for (n, body) in [(1, "f1"), (2, "f2"), (3, "f3")] {
        assert_eq!(
            read_rev(&fs, &db, &g, "/dst/in/f", Want::Revision(n)).await,
            body
        );
    }
    // A file with its revisions, into a new directory.
    written(&db, &fs, &g, "/one.txt", "1").await;
    written(&db, &fs, &g, "/one.txt", "22").await;
    let pairs = move_path(
        &db.pool,
        &fs,
        &g,
        &file("/one.txt"),
        &file("/deep/two.txt"),
        by(),
    )
    .await
    .unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(rows_of(&db, ws, "/deep/").await, [(1, false)]);
    assert_eq!(
        rows_of(&db, ws, "/deep/two.txt").await,
        [(1, false), (2, false)]
    );
    assert_eq!(rows_of(&db, ws, "/one.txt").await, []);
    assert_eq!(
        read_rev(&fs, &db, &g, "/deep/two.txt", Want::Latest).await,
        "22"
    );
}

#[tokio::test]
async fn move_that_fails_mid_copy_purges_its_copies_in_reverse_and_leaves_the_source() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/s/a", "a1").await;
    written(&db, &fs, &g, "/s/a", "a2").await;
    written(&db, &fs, &g, "/s/b", "b1").await;
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    // The copies' put_streams are 4, 5, 6: the third copy fails.
    assert_eq!(store.put_streams.load(Ordering::SeqCst), 3);
    store.fail_put_stream_at.store(6, Ordering::SeqCst);
    store.deletes.lock().unwrap().clear();
    let err = move_path(&db.pool, &fs, &g, &dir("/s"), &dir("/n/t"), by())
        .await
        .unwrap_err();
    match &err {
        NsError::Store(m) => {
            assert!(
                m.contains("'/s/'") && m.contains("'/n/t/'") && m.contains("nothing was moved"),
                "{m}"
            );
            assert!(!m.contains("s3://"), "{m}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        (all_rows(&db, ws).await, store.objects(&fs, ws)),
        before,
        "the source is untouched and no copy is left"
    );
    // The copies made (`/n/t/a` revisions 1 and 2) were removed last first.
    let prefix = format!("s3://egret-test/workflow/{ws}/w9b/t3/_files");
    assert_eq!(
        *store.deletes.lock().unwrap(),
        [format!("{prefix}/n/t/a.v2"), format!("{prefix}/n/t/a")]
    );
    // M5: the parent `/n/` is created only after every copy succeeded.
    assert_eq!(
        rows_of(&db, ws, "/n/").await,
        [],
        "no parent row is left behind"
    );
}

/// §8.4 with a writer in the window: the purge has committed its row deletes
/// and is about to delete the object when a writer of the same path starts.
/// The writer must wait for the purge's SESSION lock; a transaction lock
/// (released by that commit) would let it write revision 1 to the very key
/// the purge then deletes.
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_and_a_writer_never_leave_a_row_without_an_object() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    let p = "/purged.txt";
    written(&db, &fs, &g, p, "old").await;
    let (delete_started, release_delete) = store.arm_delete_gate();
    let purge = {
        let (pool, fs, g) = (db.pool.clone(), fs.clone(), g.clone());
        tokio::spawn(async move { purge_revisions(&pool, &fs, &g, &file(p), &[1]).await })
    };
    delete_started.await.unwrap();
    assert_eq!(
        rows_of(&db, ws, p).await,
        [],
        "the rows went first, committed"
    );
    let (mut started, release_writer) = store.arm_copy_gate();
    let writer = spawn_put(&db, &fs, &g, p, bytes("new"));
    let seen = blocked_or_started(&db, ws, p, &mut started).await;
    if seen == Seen::Started {
        // Nothing stopped the writer: let it finish before the purge goes on.
        release_writer.send(()).unwrap();
        writer.await.unwrap().unwrap();
        release_delete.send(()).unwrap();
        assert_eq!(purge.await.unwrap().unwrap(), 1);
    } else {
        release_delete.send(()).unwrap();
        assert_eq!(purge.await.unwrap().unwrap(), 1);
        let _ = release_writer.send(());
        writer.await.unwrap().unwrap();
    }
    let rows = rows_of(&db, ws, p).await;
    assert!(!rows.is_empty());
    for (rev, _) in rows {
        let key = fs.namespace_object_uri(&g, &file(p), rev as u32);
        assert!(
            fs.store.head(&key).await.unwrap().is_some(),
            "revision {rev}'s row names no object"
        );
    }
    assert_eq!(read_rev(&fs, &db, &g, p, Want::Latest).await, "new");
    assert_eq!(seen, Seen::Blocked);
}

/// §8.4's order, observed: when the object delete runs, the row delete is
/// already committed (another connection sees no row).
#[tokio::test]
async fn a_purge_commits_its_row_deletes_before_it_deletes_an_object() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Purge);
    let w = grant(ws, Access::Write);
    written(&db, &fs, &w, "/o.txt", "1").await;
    written(&db, &fs, &w, "/o.txt", "2").await;
    // The writes' own deletes (their staging objects, Task 8 fix round 1).
    store.deletes.lock().unwrap().clear();
    let (delete_started, release_delete) = store.arm_delete_gate();
    let purge = {
        let (pool, fs, g) = (db.pool.clone(), fs.clone(), g.clone());
        tokio::spawn(async move { purge_revisions(&pool, &fs, &g, &file("/o.txt"), &[1, 7]).await })
    };
    delete_started.await.unwrap();
    assert_eq!(rows_of(&db, ws, "/o.txt").await, [(2, false)]);
    let v1 = fs.namespace_object_uri(&w, &file("/o.txt"), 1);
    assert!(store.bytes(&v1).is_some(), "the object is deleted after");
    release_delete.send(()).unwrap();
    assert_eq!(
        purge.await.unwrap().unwrap(),
        1,
        "revision 7 has no row: not counted"
    );
    assert!(store.bytes(&v1).is_none());
    assert_eq!(*store.deletes.lock().unwrap(), [v1]);
}

/// §11: the row is gone, so the file is gone for every reader; a failed
/// object delete is an orphan, not an error.
#[tokio::test]
async fn an_object_delete_that_fails_after_the_commit_is_an_orphan_not_an_error() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/x", "x").await;
    store.fail_deletes.store(true, Ordering::SeqCst);
    assert_eq!(
        purge_revisions(&db.pool, &fs, &g, &file("/x"), &[1])
            .await
            .unwrap(),
        1
    );
    assert_eq!(rows_of(&db, ws, "/x").await, []);
    assert_eq!(store.objects(&fs, ws).len(), 1, "the orphan");
}

/// The PostgreSQL fact the purge relies on, measured on this database: a
/// session advisory lock and a transaction advisory lock on one key conflict,
/// both ways.
#[tokio::test]
async fn a_session_lock_blocks_a_transaction_lock_on_the_same_key() {
    let Some(db) = common::db().await else { return };
    let key: i64 = sqlx::query_scalar("SELECT hashtextextended($1, 0)")
        .bind(format!("w9b:{}:t3:fact", Uuid::new_v4()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let mut session = conn(&db).await;
    let mut other = conn(&db).await;
    let (try_xact, try_session, unlock) = (
        "SELECT pg_try_advisory_xact_lock($1)",
        "SELECT pg_try_advisory_lock($1)",
        "SELECT pg_advisory_unlock($1)",
    );
    assert!(ask(&mut session, try_session, key).await);
    let mut tx = sqlx::Connection::begin(&mut *other).await.unwrap();
    assert!(
        !ask(&mut tx, try_xact, key).await,
        "a session lock blocks a transaction lock"
    );
    tx.rollback().await.unwrap();
    assert!(ask(&mut session, unlock, key).await);
    // The control: free, it is taken.
    let mut tx = sqlx::Connection::begin(&mut *other).await.unwrap();
    assert!(ask(&mut tx, try_xact, key).await);
    // And the other way: a transaction lock blocks a session lock.
    assert!(
        !ask(&mut session, try_session, key).await,
        "a transaction lock blocks a session lock"
    );
    tx.commit().await.unwrap();
    assert!(
        ask(&mut session, try_session, key).await,
        "released by the commit"
    );
    assert!(ask(&mut session, unlock, key).await);
}

async fn ask(c: &mut sqlx::PgConnection, sql: &'static str, key: i64) -> bool {
    sqlx::query_scalar(sql)
        .bind(key)
        .fetch_one(c)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_write_with_a_read_grant_is_an_internal_error() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let w = grant(ws, Access::Write);
    written(&db, &fs, &w, "/k", "k").await;
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    let r = grant(ws, Access::Read);
    let internal = |e: NsError| matches!(e, NsError::Db(egret_core::Error::Internal(_)));
    assert!(internal(
        write(&db, &fs, &r, "/n", "n", Conflict::Overwrite)
            .await
            .unwrap_err()
    ));
    assert!(internal(
        create_directory(&db.pool, &r, &dir("/n"), by())
            .await
            .unwrap_err()
    ));
    assert!(internal(
        soft_delete(&db.pool, &r, &file("/k")).await.unwrap_err()
    ));
    assert!(internal(
        move_path(&db.pool, &fs, &r, &file("/k"), &file("/n"), by())
            .await
            .unwrap_err()
    ));
    assert!(internal(
        purge_revisions(&db.pool, &fs, &r, &file("/k"), &[1])
            .await
            .unwrap_err()
    ));
    // A Purge grant writes nothing either, and purges.
    let p = grant(ws, Access::Purge);
    assert!(internal(
        write(&db, &fs, &p, "/n", "n", Conflict::Overwrite)
            .await
            .unwrap_err()
    ));
    assert_eq!((all_rows(&db, ws).await, store.objects(&fs, ws)), before);
    assert_eq!(
        purge_revisions(&db.pool, &fs, &p, &file("/k"), &[1])
            .await
            .unwrap(),
        1
    );
    // The control: a Write grant purges too (move does).
    written(&db, &fs, &w, "/k2", "k").await;
    assert_eq!(
        purge_revisions(&db.pool, &fs, &w, &file("/k2"), &[1])
            .await
            .unwrap(),
        1
    );
}

// ─── Fix round 1 ────────────────────────────────────────────────────────────

/// Spawns `work` and answers its handle and a signal that fires when it
/// returns (the "nothing stopped it" event of [`blocked_or_started`]).
fn spawn_signalled<T: Send + 'static>(
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> (tokio::task::JoinHandle<T>, oneshot::Receiver<()>) {
    let (done_tx, done_rx) = oneshot::channel();
    let h = tokio::spawn(async move {
        let r = work.await;
        let _ = done_tx.send(());
        r
    });
    (h, done_rx)
}

type Moved = Result<Vec<(NsEntry, NsEntry)>, NsError>;

fn spawn_move(
    db: &Db,
    fs: &Arc<storage::FileStore>,
    g: &Grant,
    from: NsPath,
    to: NsPath,
) -> tokio::task::JoinHandle<Moved> {
    let (pool, fs, g) = (db.pool.clone(), fs.clone(), g.clone());
    tokio::spawn(async move { move_path(&pool, &fs, &g, &from, &to, by()).await })
}

/// I1: a move of `/m/` holds `/m`'s key; a soft delete of `/m/x` locks its
/// chain (`/m`, `/m/x`), so it waits for the move and then finds nothing at
/// `/m/x` -- the serial order "move, then delete". Before the fix it
/// answered at once that `/m/x` was deleted while the move made `/t/x` live.
#[tokio::test(flavor = "multi_thread")]
async fn a_soft_delete_of_a_child_waits_for_a_move_of_its_parent() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/m/x", "x").await;
    let (copying, release_copy) = store.arm_put_stream_gate();
    let mover = spawn_move(&db, &fs, &g, dir("/m"), dir("/t"));
    copying.await.unwrap();
    let (pool, g2) = (db.pool.clone(), g.clone());
    let (deleter, mut returned) =
        spawn_signalled(async move { soft_delete(&pool, &g2, &file("/m/x")).await });
    let seen = blocked_or_started(&db, ws, "/m", &mut returned).await;
    release_copy.send(()).unwrap();
    let moved = mover.await.unwrap().unwrap();
    let deleted = deleter.await.unwrap().unwrap();
    assert_eq!(moved.len(), 2);
    assert!(
        deleted.is_empty(),
        "move, then delete: nothing at /m/x: {deleted:?}"
    );
    assert_eq!(
        all_rows(&db, ws).await,
        [("/t/".into(), 1, false, 0), ("/t/x".into(), 1, false, 1)]
    );
    assert_eq!(seen, Seen::Blocked);
}

/// I1, the purge variant: a purge of `/m/x` waits for the move of `/m/`, then
/// finds no row (0 purged); the moved revision and its object are intact.
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_of_a_child_waits_for_a_move_of_its_parent() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/m/x", "x").await;
    let (copying, release_copy) = store.arm_put_stream_gate();
    let mover = spawn_move(&db, &fs, &g, dir("/m"), dir("/t"));
    copying.await.unwrap();
    let (pool, fs2, p) = (db.pool.clone(), fs.clone(), grant(ws, Access::Purge));
    let (purger, mut returned) =
        spawn_signalled(async move { purge_revisions(&pool, &fs2, &p, &file("/m/x"), &[1]).await });
    let seen = blocked_or_started(&db, ws, "/m", &mut returned).await;
    release_copy.send(()).unwrap();
    assert_eq!(mover.await.unwrap().unwrap().len(), 2);
    assert_eq!(
        purger.await.unwrap().unwrap(),
        0,
        "move, then purge: no row at /m/x"
    );
    assert_eq!(rows_of(&db, ws, "/t/x").await, [(1, false)]);
    assert_eq!(read_rev(&fs, &db, &g, "/t/x", Want::Latest).await, "x");
    assert_eq!(seen, Seen::Blocked);
}

/// I2 (the user's ruling, 2026-10-05): a target with soft-deleted history --
/// at it, or a deleted file where its parent goes -- is refused, naming it;
/// nothing moves and the history is intact.
#[tokio::test]
async fn a_move_onto_a_soft_deleted_file_is_refused_and_keeps_its_history() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/a", "a").await;
    for body in ["b1", "b2", "b3"] {
        written(&db, &fs, &g, "/b", body).await;
    }
    soft_delete(&db.pool, &g, &file("/b")).await.unwrap();
    written(&db, &fs, &g, "/f", "f").await;
    soft_delete(&db.pool, &g, &file("/f")).await.unwrap();
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    // Fix round 2, M-R1: a soft-deleted FILE where a parent of the target
    // goes; the move would otherwise make `/f/` beside `/f`'s history.
    let err = move_path(&db.pool, &fs, &g, &file("/a"), &file("/f/x"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("its parent '/f' is a deleted file") && m.contains("nothing was moved")),
        "{err:?}"
    );
    assert_eq!((all_rows(&db, ws).await, store.objects(&fs, ws)), before);
    let err = move_path(&db.pool, &fs, &g, &file("/a"), &file("/b"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m) if m.contains("'/b'") && m.contains("history")),
        "{err:?}"
    );
    assert_eq!((all_rows(&db, ws).await, store.objects(&fs, ws)), before);
    for (n, body) in [(1, "b1"), (2, "b2"), (3, "b3")] {
        assert_eq!(read_rev(&fs, &db, &g, "/b", Want::Revision(n)).await, body);
    }
}

/// I2 for a directory: one child target (`/t/a`) holds history and nothing
/// else does -- not even `/t/` (a lone row inserted by SQL) -- and the move
/// of `/s/` to `/t/` is refused naming it.
#[tokio::test]
async fn a_directory_move_onto_a_child_with_history_is_refused() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/s/a", "new").await;
    written(&db, &fs, &g, "/s/b", "b").await;
    let old = fs.namespace_object_uri(&g, &file("/t/a"), 1);
    store.mem.put(&old, b"old".to_vec()).await.unwrap();
    sqlx::query(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, deleted) \
         VALUES ($1, $2, '/t/a', 1, false, 3, true)",
    )
    .bind(ws)
    .bind(W)
    .execute(&db.pool)
    .await
    .unwrap();
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    let err = move_path(&db.pool, &fs, &g, &dir("/s"), &dir("/t"), by())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("'/t/a'") && m.contains("'/t/'") && m.contains("nothing was moved")),
        "{err:?}"
    );
    assert_eq!((all_rows(&db, ws).await, store.objects(&fs, ws)), before);
    assert_eq!(
        read_rev(&fs, &db, &g, "/t/a", Want::Revision(1)).await,
        "old"
    );
}

/// I3: the caller's future is dropped while the move is paused mid-copy; the
/// move still runs to completion once released.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_whose_caller_goes_away_still_runs_to_completion() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/c/a", "a1").await;
    written(&db, &fs, &g, "/c/a", "a2").await;
    written(&db, &fs, &g, "/c/b", "b1").await;
    let (copying, release_copy) = store.arm_put_stream_gate();
    let caller = spawn_move(&db, &fs, &g, dir("/c"), dir("/d"));
    copying.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let _ = release_copy.send(());
    // The move releases its session lock on `/c` when it is done: take the
    // same key as a transaction lock to wait for exactly that.
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("w9b:{ws}:{W}:/c"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        all_rows(&db, ws).await,
        [
            ("/d/".into(), 1, false, 0),
            ("/d/a".into(), 1, false, 2),
            ("/d/a".into(), 2, false, 2),
            ("/d/b".into(), 1, false, 2),
        ]
    );
    let keys: Vec<String> = store.objects(&fs, ws).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        keys,
        [
            "w9b/t3/_files/d/a",
            "w9b/t3/_files/d/a.v2",
            "w9b/t3/_files/d/b"
        ]
    );
}

/// M1: the conflict rule reads the LATEST row. Two revisions, the second
/// soft-deleted: revision 1 is still `deleted = false`, so a rule reading the
/// oldest row would answer Conflict.
#[tokio::test]
async fn the_conflict_rule_reads_the_latest_row_not_the_oldest() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (_store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/p", "1").await;
    written(&db, &fs, &g, "/p", "2").await;
    soft_delete(&db.pool, &g, &file("/p")).await.unwrap();
    assert_eq!(rows_of(&db, ws, "/p").await, [(1, false), (2, true)]);
    match write(&db, &fs, &g, "/p", "3", Conflict::Error).await {
        Ok(Written::Written(e)) => assert_eq!(e.revision, 3),
        other => panic!("{other:?}"),
    }
    soft_delete(&db.pool, &g, &file("/p")).await.unwrap();
    match write(&db, &fs, &g, "/p", "4", Conflict::Skip).await {
        Ok(Written::Written(e)) => assert_eq!(e.revision, 4),
        other => panic!("{other:?}"),
    }
}

/// M4, and the user's ruling (2026-10-05), a file over a directory: a file
/// over a soft-deleted directory whose children keep history is refused
/// naming the path; nothing is purged. Once `PurgeFiles` removed the
/// children's revisions, the directory is alone and the file is written.
#[tokio::test]
async fn a_file_over_a_deleted_directory_with_history_under_it_is_refused() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/q/x", "x").await;
    soft_delete(&db.pool, &g, &dir("/q")).await.unwrap();
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws));
    let err = write(&db, &fs, &g, "/q", "file", Conflict::Overwrite)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Conflict(m)
            if m.contains("'/q'") && m.contains("'/q/' is a deleted directory")
                && m.contains("history")),
        "{err:?}"
    );
    assert_eq!((all_rows(&db, ws).await, store.objects(&fs, ws)), before);
    assert_eq!(read_rev(&fs, &db, &g, "/q/x", Want::Revision(1)).await, "x");

    let p = grant(ws, Access::Purge);
    assert_eq!(
        purge_revisions(&db.pool, &fs, &p, &file("/q/x"), &[1])
            .await
            .unwrap(),
        1
    );
    assert_eq!(written(&db, &fs, &g, "/q", "file").await.revision, 1);
    assert_eq!(rows_of(&db, ws, "/q/").await, []);
}

/// W9b Task 7 fix round 1, I1: a purge planned from an earlier listing
/// names rows by `id`, so it never removes a newer file that took the same
/// revision NUMBER. The reviewer's interleaving, forced in order: plan (a
/// soft-deleted `/x.txt` revision 1) -> another purge removes it -> a new
/// upload writes `/x.txt` revision 1, live -> the stale purge removes 0
/// rows, and the new file stays live with its object.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_purge_never_removes_a_new_file_that_reused_its_revision_numbers() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let w = grant(ws, Access::Write);
    let purge = grant(ws, Access::Purge);
    let p = file("/x.txt");
    written(&db, &fs, &w, "/x.txt", "old").await;
    soft_delete(&db.pool, &w, &p).await.unwrap();
    // A plans: every revision of the soft-deleted path.
    let plan: Vec<(i64, u32)> = {
        let mut c = conn(&db).await;
        file_rows_page(&mut c, &purge, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, e)| (id, e.revision))
            .collect()
    };
    assert_eq!(plan.iter().map(|r| r.1).collect::<Vec<_>>(), [1]);
    // B purges it first.
    assert_eq!(
        purge_rows(&db.pool, &fs, &purge, &p, &plan).await.unwrap(),
        1
    );
    // A new file takes revision 1 again.
    let new = written(&db, &fs, &w, "/x.txt", "new").await;
    assert_eq!(new.revision, 1, "the number is reused");
    // A resumes with its stale plan: nothing of the new file goes.
    assert_eq!(
        purge_rows(&db.pool, &fs, &purge, &p, &plan).await.unwrap(),
        0
    );
    assert_eq!(rows_of(&db, ws, "/x.txt").await, [(1, false)]);
    let key = fs.namespace_object_uri(&w, &p, 1);
    assert_eq!(store.bytes(&key).as_deref(), Some("new"));
    let mut c = conn(&db).await;
    assert!(resolve(&mut c, &w, &p, Want::Latest).await.is_ok());
}

/// W9b Task 7 fix round 1, M1/M2: `soft_delete_if_empty` judges and deletes
/// in one transaction holding the directory's chain lock, which a writer
/// under it holds from its revision's choice to its commit. A writer of
/// `/p/new.txt` paused there makes the judge WAIT, then see the new live
/// file and keep `/p/`.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_into_a_parent_being_judged_empty_keeps_it() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    for round in 0..10 {
        let d = format!("/p{round}");
        // The parent holds only a soft-deleted file: empty of live files.
        let old = format!("{d}/old.txt");
        written(&db, &fs, &g, &old, "old").await;
        soft_delete(&db.pool, &g, &file(&old)).await.unwrap();
        let (started, release) = store.arm_copy_gate();
        let new = format!("{d}/new.txt");
        let writer = spawn_put(&db, &fs, &g, &new, bytes("N-new"));
        started
            .await
            .expect("the writer holds the chain lock mid-write");
        let judge = {
            let (pool, g, dir) = (db.pool.clone(), g.clone(), dir(&format!("{d}/")));
            tokio::spawn(async move { soft_delete_if_empty(&pool, &g, &dir).await })
        };
        // The judge's first key is the directory's own (`/pN`).
        let (_never, mut no_body) = oneshot::channel::<()>();
        let seen = blocked_or_started(&db, ws, &d, &mut no_body).await;
        assert_eq!(seen, Seen::Blocked, "round {round}: the judge did not wait");
        release.send(()).unwrap();
        assert!(matches!(writer.await.unwrap(), Ok(Written::Written(_))));
        let marked = judge.await.unwrap().unwrap();
        assert!(marked.is_empty(), "round {round}: {marked:?}");
        let mut c = conn(&db).await;
        assert!(resolve(&mut c, &g, &file(&new), Want::Latest).await.is_ok());
        assert!(resolve(&mut c, &g, &dir(&format!("{d}/")), Want::Latest)
            .await
            .is_ok());
    }
}

// ─── Task 8 fix round 1: staging (the user's ruling of 2026-10-06) ─────────

use egret_workflow::storage::nsfiles::staging::{self, StagingSweepConfig};

/// The advisory keys of `lock_paths` that some session holds now.
async fn held_keys(db: &Db, ws: Uuid, lock_paths: &[&str]) -> i64 {
    let texts: Vec<String> = lock_paths
        .iter()
        .map(|lp| format!("w9b:{ws}:{W}:{lp}"))
        .collect();
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks l \
          WHERE l.locktype = 'advisory' AND l.granted \
            AND ((l.classid::bigint << 32) | l.objid::bigint) IN \
                (SELECT hashtextextended(t, 0) FROM unnest($1::text[]) AS t)",
    )
    .bind(&texts)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// This workspace's staging rows.
async fn staging_rows(db: &Db, ws: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM workflow.namespace_file_staging WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// This workspace's staging objects.
fn staging_objects(store: &Hooked, fs: &storage::FileStore, ws: Uuid) -> Vec<String> {
    let prefix = format!(
        "s3://{}/{}{ws}/{}/",
        fs.bucket,
        fs.prefix,
        staging::STAGING_SEGMENT
    );
    store
        .mem
        .keys()
        .into_iter()
        .filter(|k| k.starts_with(&prefix))
        .collect()
}

/// I-1: writer A's body stalls mid-upload. It holds no advisory lock and no
/// pool connection meanwhile, so writer B of the SAME path writes at once.
/// Released, A takes the next revision; neither leaves a staging object.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_body_holds_no_connection_and_no_lock() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    let p = "/stall/s.txt";
    let (body_a, started_a, release_a) = gated("s", "-a");
    let a = spawn_put(&db, &fs, &g, p, body_a);
    started_a.await.expect("A's body is being read");
    assert_eq!(
        held_keys(&db, ws, &["/stall", p]).await,
        0,
        "A holds a lock of its path while its body stalls"
    );
    // A connection this test used goes back to the pool a moment after its
    // query; one A held would never come back. A guard, not a measurement.
    let all_idle = async {
        while db.pool.size() as usize != db.pool.num_idle() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), all_idle)
        .await
        .expect("A holds a pool connection while its body stalls");
    let b = tokio::time::timeout(std::time::Duration::from_secs(30), {
        let (pool, fs, g) = (db.pool.clone(), fs.clone(), g.clone());
        async move {
            let mut body = bytes("B");
            put(
                &pool,
                &fs,
                &g,
                &file(p),
                &mut body,
                Conflict::Overwrite,
                by(),
            )
            .await
        }
    })
    .await
    .expect("B waited for A's stalled body");
    match b {
        Ok(Written::Written(e)) => assert_eq!(e.revision, 1),
        other => panic!("{other:?}"),
    }
    release_a.send(()).unwrap();
    match a.await.unwrap() {
        Ok(Written::Written(e)) => assert_eq!(e.revision, 2),
        other => panic!("{other:?}"),
    }
    assert_eq!(read_rev(&fs, &db, &g, p, Want::Revision(1)).await, "B");
    assert_eq!(read_rev(&fs, &db, &g, p, Want::Latest).await, "s-a");
    assert_eq!(staging_rows(&db, ws).await, 0);
    assert_eq!(staging_objects(&store, &fs, ws), Vec::<String>::new());
}

/// Task 6's re-claim, judged under the path's lock (fix round 1): a task
/// run's own live latest revision is `Already` under every conflict mode and
/// nothing is written; another task run writes the next revision; a deleted
/// latest is written again.
#[tokio::test]
async fn a_task_runs_own_latest_revision_is_already_written_under_every_conflict_mode() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (_store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    let execution = Uuid::new_v4();
    let me = CreatedBy::TaskRun {
        execution,
        task_run: Uuid::new_v4(),
    };
    let other = CreatedBy::TaskRun {
        execution,
        task_run: Uuid::new_v4(),
    };
    let p = file("/r.txt");
    let put_as = |by: CreatedBy, body: &'static str, conflict: Conflict| {
        let (pool, fs, g, p) = (db.pool.clone(), fs.clone(), g.clone(), p.clone());
        async move {
            let mut b = body.as_bytes();
            put(&pool, &fs, &g, &p, &mut b, conflict, by).await
        }
    };
    assert!(matches!(
        put_as(me, "one", Conflict::Overwrite).await,
        Ok(Written::Written(e)) if e.revision == 1
    ));
    for conflict in [Conflict::Overwrite, Conflict::Error, Conflict::Skip] {
        match put_as(me, "again", conflict).await {
            Ok(Written::Already(e)) => assert_eq!(e.revision, 1, "{conflict:?}"),
            other => panic!("{conflict:?}: {other:?}"),
        }
    }
    assert_eq!(rows_of(&db, ws, "/r.txt").await, [(1, false)]);
    assert!(matches!(
        put_as(other, "two", Conflict::Overwrite).await,
        Ok(Written::Written(e)) if e.revision == 2
    ));
    assert!(matches!(
        put_as(me, "three", Conflict::Overwrite).await,
        Ok(Written::Written(e)) if e.revision == 3
    ));
    soft_delete(&db.pool, &g, &p).await.unwrap();
    assert!(matches!(
        put_as(me, "four", Conflict::Overwrite).await,
        Ok(Written::Written(e)) if e.revision == 4
    ));
    assert_eq!(read_rev(&fs, &db, &g, "/r.txt", Want::Latest).await, "four");
}

/// Re-review m-1: the caller's future is dropped once the body is staged
/// (paused at the staging -> revision copy, under the path's lock). The
/// commit still runs to completion once released, and the staging object
/// and its row are deleted with no sweep run.
#[tokio::test(flavor = "multi_thread")]
async fn a_put_whose_caller_goes_away_once_staged_leaves_no_staging_object_or_row() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let fs = Arc::new(fs);
    let g = grant(ws, Access::Write);
    let (copying, release_copy) = store.arm_copy_gate();
    let caller = spawn_put(&db, &fs, &g, "/gone/x.txt", bytes("staged"));
    copying.await.expect("the body is staged and being copied");
    assert_eq!(staging_rows(&db, ws).await, 1);
    assert_eq!(staging_objects(&store, &fs, ws).len(), 1);
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let _ = release_copy.send(());
    let cleaned = async {
        while staging_rows(&db, ws).await != 0 || !staging_objects(&store, &fs, ws).is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), cleaned)
        .await
        .expect("a dropped put left its staging object or row for the sweep");
    assert_eq!(rows_of(&db, ws, "/gone/x.txt").await, [(1, false)]);
    assert_eq!(
        read_rev(&fs, &db, &g, "/gone/x.txt", Want::Latest).await,
        "staged"
    );
}

/// Every way out of `put` deletes its staging object and its staging row:
/// written, skipped, a conflict, over the cap, a body that fails, a copy
/// that fails. A staging delete that fails leaves the row for the sweep.
#[tokio::test]
async fn every_put_leaves_no_staging_object_and_no_staging_row() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(Some(4));
    let g = grant(ws, Access::Write);
    let clean = |what: &'static str| {
        let (db, store, fs) = (&db, &store, &fs);
        async move {
            assert_eq!(staging_rows(db, ws).await, 0, "{what}");
            assert_eq!(
                staging_objects(store, fs, ws),
                Vec::<String>::new(),
                "{what}"
            );
        }
    };
    written(&db, &fs, &g, "/a", "x").await;
    clean("written").await;
    assert!(matches!(
        write(&db, &fs, &g, "/a", "y", Conflict::Skip).await,
        Ok(Written::Skipped(_))
    ));
    clean("skipped").await;
    assert!(matches!(
        write(&db, &fs, &g, "/a", "y", Conflict::Error).await,
        Err(NsError::Conflict(_))
    ));
    clean("conflict").await;
    assert!(matches!(
        write(&db, &fs, &g, "/b", "12345", Conflict::Overwrite).await,
        Err(NsError::TooLarge(_))
    ));
    clean("over the cap").await;
    let mut broken = Breaks(2);
    assert!(matches!(
        put(
            &db.pool,
            &fs,
            &g,
            &file("/c"),
            &mut broken,
            Conflict::Overwrite,
            by()
        )
        .await,
        Err(NsError::Store(_))
    ));
    clean("a body that fails").await;
    store.fail_copies.store(true, Ordering::SeqCst);
    match write(&db, &fs, &g, "/d", "z", Conflict::Overwrite).await {
        Err(NsError::Store(m)) => assert!(m.contains("'/d'") && !m.contains("s3://"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        rows_of(&db, ws, "/d").await,
        [],
        "a failed copy writes no row"
    );
    clean("a copy that fails").await;
    store.fail_copies.store(false, Ordering::SeqCst);
    // The staging delete fails: the row stays, for the sweep.
    store.fail_deletes.store(true, Ordering::SeqCst);
    written(&db, &fs, &g, "/e", "e").await;
    assert_eq!(staging_rows(&db, ws).await, 1);
    assert_eq!(staging_objects(&store, &fs, ws).len(), 1);
}

/// The staging sweep (fix round 1, I-1 step 4): a staging row older than
/// 24 hours loses its object, then itself; a younger one is kept; a row that
/// names anything but its own staging key is removed WITHOUT deleting what it
/// names; a store failure ends the pass and keeps the row. Batches of one,
/// so the loop over pages is exercised.
#[tokio::test]
async fn the_staging_sweep_deletes_old_orphans_only_and_only_their_staging_keys() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = hooked_fs(None);
    let g = grant(ws, Access::Write);
    let keep = written(&db, &fs, &g, "/keep.txt", "keep").await;
    let keep_key = fs.namespace_object_uri(&g, &keep.path, 1);
    let mut staged = Vec::new();
    for body in ["old-1", "old-2", "fresh", "forged"] {
        let s = staging::record(&db.pool, &fs, ws).await.unwrap();
        fs.store
            .put(&s.uri, body.as_bytes().to_vec())
            .await
            .unwrap();
        staged.push(s);
    }
    let (old1, old2, fresh, forged) = (&staged[0], &staged[1], &staged[2], &staged[3]);
    sqlx::query("UPDATE workflow.namespace_file_staging SET object_uri = $2 WHERE id = $1")
        .bind(forged.id)
        .bind(&keep_key)
        .execute(&db.pool)
        .await
        .unwrap();
    let age = |ids: Vec<Uuid>, hours: i32| {
        let pool = db.pool.clone();
        async move {
            sqlx::query(
                "UPDATE workflow.namespace_file_staging \
                    SET created_at = now() - make_interval(hours => $2) WHERE id = ANY($1)",
            )
            .bind(&ids)
            .bind(hours)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    age(vec![old1.id, old2.id, forged.id], 25).await;
    age(vec![fresh.id], 23).await;
    let row = |id: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM workflow.namespace_file_staging WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let cfg = StagingSweepConfig {
        batch: 1,
        ..StagingSweepConfig::default()
    };
    let removed = staging::sweep(&db.pool, &fs, &cfg).await.unwrap();
    assert!(removed >= 3, "{removed}");
    for (s, what) in [(old1, "old-1"), (old2, "old-2")] {
        assert_eq!(row(s.id).await, 0, "{what}'s row");
        assert_eq!(store.bytes(&s.uri), None, "{what}'s object");
    }
    assert_eq!(row(forged.id).await, 0, "the forged row is removed");
    assert_eq!(
        store.bytes(&keep_key).as_deref(),
        Some("keep"),
        "the key a forged row names is never deleted"
    );
    assert_eq!(store.bytes(&forged.uri).as_deref(), Some("forged"));
    assert_eq!(row(fresh.id).await, 1, "a young row is kept");
    assert_eq!(store.bytes(&fresh.uri).as_deref(), Some("fresh"));
    // A store that fails: the pass ends, the row stays for the next one.
    age(vec![fresh.id], 25).await;
    store.fail_deletes.store(true, Ordering::SeqCst);
    staging::sweep(&db.pool, &fs, &cfg).await.unwrap();
    assert_eq!(row(fresh.id).await, 1);
    store.fail_deletes.store(false, Ordering::SeqCst);
    staging::sweep(&db.pool, &fs, &cfg).await.unwrap();
    assert_eq!(row(fresh.id).await, 0);
    assert_eq!(store.bytes(&fresh.uri), None);
}

/// The file cap a namespace file gets: `max_file_bytes`, or 5 GiB -- one
/// server-side copy's limit -- when that is configured above it, named.
#[test]
fn a_namespace_file_is_capped_at_one_copys_limit_naming_it() {
    use egret_workflow::storage::nsfiles::store::namespace_file_cap;
    let five = egret_storage::MAX_SINGLE_COPY_BYTES;
    assert_eq!(five, 5 << 30);
    for (configured, cap, named) in [
        (1 << 30, 1 << 30, "workflow.storage.max_file_bytes"),
        (five, five, "workflow.storage.max_file_bytes"),
        (five + 1, five, "internal-storage copy"),
        (u64::MAX, five, "internal-storage copy"),
    ] {
        let caps = storage::Caps {
            max_file_bytes: configured,
            ..Default::default()
        };
        let (got, name) = namespace_file_cap(&caps);
        assert_eq!(got, cap, "{configured}");
        assert!(name.contains(named), "{configured}: {name}");
    }
}
