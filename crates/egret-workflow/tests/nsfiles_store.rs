//! W9b Task 2: the read side of `workflow.namespace_files` (migration 0169)
//! and the objects behind it. Requires DATABASE_URL.
//!
//! Rows are inserted with plain SQL: Task 3 writes the writer. Every test
//! makes its own workspaces, so the tests cannot see each other's rows. The
//! only object store touched is the `MemStore` of `install_mem_for_tests`.

mod common;

use egret_metastore::Db;
use egret_workflow::storage;
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{
    list_live, namespaces_with_files, open, resolve, revisions, search, NsEntry, Want,
};
use egret_workflow::storage::nsfiles::NsError;
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
    assert!(namespaces_with_files(&mut c, c_ws)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(namespaces_with_files(&mut c, a).await.unwrap(), [NS]);
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
