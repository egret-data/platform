//! W7d Task 1: the namespace listing (spec §3.1) --
//! `GET /api/ws/{workspace}/workflows/namespaces?prefix=` and
//! `GET /api/ws/{workspace}/workflows/namespaces/{namespace}`.
//!
//! A namespace is derived: from flows that are not deleted, from live
//! namespace files (a path's HIGHEST revision is not `deleted`) and from live
//! KV keys (the highest revision is neither deleted nor expired), plus every
//! parent by dots. The rows are seeded straight into the three tables -- the
//! listing reads rows, never objects, so no object store is needed here.
//! Every test seeds its own workspace. Requires DATABASE_URL.

mod common;

use axum::http::StatusCode;
use egret_core::UserId;
use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

fn flow(ns: &str, id: &str) -> String {
    format!("id: {id}\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n")
}

async fn seed_flow(db: &Db, ws: Uuid, user: UserId, ns: &str, id: &str) -> Uuid {
    common::seed_flow(db, ws, user, &flow(ns, id), "draft").await
}

async fn soft_delete_flow(db: &Db, flow: Uuid) {
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(flow)
        .execute(&db.pool)
        .await
        .expect("soft-delete flow");
}

/// One revision row of a namespace file (`path` ends with '/' for a
/// directory, whose shape the table pins to revision 1).
async fn file_row(db: &Db, ws: Uuid, ns: &str, path: &str, revision: i32, deleted: bool) {
    sqlx::query(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, deleted) \
         VALUES ($1, $2, $3, $4, right($3, 1) = '/', 0, $5)",
    )
    .bind(ws)
    .bind(ns)
    .bind(path)
    .bind(revision)
    .bind(deleted)
    .execute(&db.pool)
    .await
    .expect("seed namespace file row");
}

#[derive(Clone, Copy)]
enum Kv {
    Live,
    Deleted,
    Expired,
    /// Expires in an hour: live now.
    Expiring,
}

async fn kv_row(db: &Db, ws: Uuid, ns: &str, key: &str, revision: i32, state: Kv) {
    let (deleted, expires) = match state {
        Kv::Live => (false, None),
        Kv::Deleted => (true, None),
        Kv::Expired => (false, Some(-60)),
        Kv::Expiring => (false, Some(3600)),
    };
    sqlx::query(
        "INSERT INTO workflow.kv_entries \
           (workspace_id, namespace, key, revision, value_type, size, expires_at, deleted, deleted_at) \
         VALUES ($1, $2, $3, $4, 'STRING', 1, \
                 CASE WHEN $5::int IS NULL THEN NULL ELSE now() + make_interval(secs => $5::int) END, \
                 $6, CASE WHEN $6 THEN now() END)",
    )
    .bind(ws)
    .bind(ns)
    .bind(key)
    .bind(revision)
    .bind(expires)
    .bind(deleted)
    .execute(&db.pool)
    .await
    .expect("seed kv row");
}

async fn reader(db: &Db) -> (Uuid, UserId) {
    let (ws, user) = common::seed_workspace(db).await;
    common::grant(db, ws, user, &["workflow:read"]).await;
    (ws, user)
}

async fn list(app: &common::App, ws: Uuid, user: UserId, query: &str) -> Vec<Value> {
    let res = common::get(
        app,
        &format!("/api/ws/{ws}/workflows/namespaces{query}"),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    common::json(res)
        .await
        .as_array()
        .expect("an array")
        .clone()
}

fn row(ns: &str, flows: i64, files: i64, kv_keys: i64, own: bool) -> Value {
    json!({ "namespace": ns, "flows": flows, "files": files, "kv_keys": kv_keys, "own": own })
}

/// A flow alone, a file alone and a key alone each make their namespace
/// appear, with only that source's count.
#[tokio::test]
async fn each_source_alone_makes_a_namespace() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_flow(&db, ws, user, "fl", "f").await;
    file_row(&db, ws, "fi", "/a.txt", 1, false).await;
    kv_row(&db, ws, "kv", "k", 1, Kv::Live).await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "").await,
        vec![
            row("fi", 0, 1, 0, true),
            row("fl", 1, 0, 0, true),
            row("kv", 0, 0, 1, true),
        ]
    );
}

/// `a.b.c` adds `a.b` and `a` as parents (`own: false`, zero counts); a
/// parent that is a source itself keeps `own: true` and ONLY its own counts
/// (children are not rolled up).
#[tokio::test]
async fn parents_by_dots_with_their_own_counts() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_flow(&db, ws, user, "a.b.c", "f").await;
    kv_row(&db, ws, "a", "k", 1, Kv::Live).await;
    file_row(&db, ws, "x.y", "/f", 1, false).await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "").await,
        vec![
            row("a", 0, 0, 1, true),
            row("a.b", 0, 0, 0, false),
            row("a.b.c", 1, 0, 0, true),
            row("x", 0, 0, 0, false),
            row("x.y", 0, 1, 0, true),
        ]
    );
}

/// A deleted flow, a file whose latest revision is deleted, a deleted key
/// and an expired key count for nothing and make no namespace -- not even
/// when an OLDER revision of the file or key is live.
#[tokio::test]
async fn dead_sources_count_for_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let gone = seed_flow(&db, ws, user, "dead.flow", "f").await;
    soft_delete_flow(&db, gone).await;
    file_row(&db, ws, "dead.file", "/a.txt", 1, false).await;
    file_row(&db, ws, "dead.file", "/a.txt", 2, true).await;
    kv_row(&db, ws, "dead.kv", "k", 1, Kv::Live).await;
    kv_row(&db, ws, "dead.kv", "k", 2, Kv::Deleted).await;
    kv_row(&db, ws, "dead.kv", "e", 1, Kv::Expired).await;
    // Mixed: one live of each beside the dead ones, in one namespace.
    let gone2 = seed_flow(&db, ws, user, "mixed", "gone").await;
    soft_delete_flow(&db, gone2).await;
    seed_flow(&db, ws, user, "mixed", "live").await;
    file_row(&db, ws, "mixed", "/gone", 1, true).await;
    file_row(&db, ws, "mixed", "/live", 1, false).await;
    kv_row(&db, ws, "mixed", "gone", 1, Kv::Deleted).await;
    kv_row(&db, ws, "mixed", "old", 1, Kv::Expired).await;
    kv_row(&db, ws, "mixed", "soon", 1, Kv::Expiring).await;
    // A deleted latest revision over a live older one, and the other way.
    kv_row(&db, ws, "mixed", "back", 1, Kv::Deleted).await;
    kv_row(&db, ws, "mixed", "back", 2, Kv::Live).await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "").await,
        vec![row("mixed", 1, 1, 2, true)]
    );
}

/// Exact counts: several revisions of one path are one file; several
/// revisions of one key are one key; a directory is not a file.
#[tokio::test]
async fn exact_counts() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    for id in ["f1", "f2", "f3"] {
        seed_flow(&db, ws, user, "n", id).await;
    }
    for rev in 1..=3 {
        file_row(&db, ws, "n", "/a.txt", rev, false).await;
    }
    file_row(&db, ws, "n", "/dir/", 1, false).await;
    file_row(&db, ws, "n", "/dir/b.txt", 1, false).await;
    for rev in 1..=4 {
        kv_row(&db, ws, "n", "k1", rev, Kv::Live).await;
    }
    kv_row(&db, ws, "n", "k2", 1, Kv::Live).await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "").await,
        vec![row("n", 3, 2, 2, true)]
    );
}

/// Another workspace's namespaces never appear, and its rows in a namespace
/// of the SAME name never add to this workspace's counts.
#[tokio::test]
async fn another_workspace_is_absent() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let (other, other_user) = common::seed_workspace(&db).await;
    seed_flow(&db, ws, user, "shared", "f").await;
    seed_flow(&db, other, other_user, "shared", "f").await;
    file_row(&db, other, "shared", "/a", 1, false).await;
    kv_row(&db, other, "shared", "k", 1, Kv::Live).await;
    seed_flow(&db, other, other_user, "theirs.flow", "f").await;
    file_row(&db, other, "theirs.file", "/a", 1, false).await;
    kv_row(&db, other, "theirs.kv", "k", 1, Kv::Live).await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "").await,
        vec![row("shared", 1, 0, 0, true)]
    );
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/namespaces/theirs.kv"),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// `prefix=company` is dot-anchored: `company` and `company.*`, never
/// `companyx`, and never a parent above the prefix.
#[tokio::test]
async fn prefix_is_dot_anchored() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_flow(&db, ws, user, "acme.company.team", "f").await;
    seed_flow(&db, ws, user, "company.team.etl", "f").await;
    kv_row(&db, ws, "company", "k", 1, Kv::Live).await;
    file_row(&db, ws, "companyx", "/a", 1, false).await;
    seed_flow(&db, ws, user, "companyx.sub", "f").await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "?prefix=company").await,
        vec![
            row("company", 0, 0, 1, true),
            row("company.team", 0, 0, 0, false),
            row("company.team.etl", 1, 0, 0, true),
        ]
    );
    assert_eq!(
        list(&app, ws, user, "?prefix=company.team").await,
        vec![
            row("company.team", 0, 0, 0, false),
            row("company.team.etl", 1, 0, 0, true),
        ]
    );
    // `_` and `%` are characters, not wildcards.
    assert_eq!(
        list(&app, ws, user, "?prefix=compan_").await,
        Vec::<Value>::new()
    );
    assert_eq!(
        list(&app, ws, user, "?prefix=comp%25").await,
        Vec::<Value>::new()
    );
}

/// The detail: 200 for a source and for a parent, 404 for anything else
/// (a dead namespace, a sibling-by-text, a child of a source), 400 for a
/// malformed name.
#[tokio::test]
async fn one_namespace() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_flow(&db, ws, user, "a.b", "f").await;
    kv_row(&db, ws, "a.b", "k", 1, Kv::Live).await;
    let gone = seed_flow(&db, ws, user, "dead", "f").await;
    soft_delete_flow(&db, gone).await;
    let app = common::app(&db).await;
    let one = |ns: &str| {
        let path = format!("/api/ws/{ws}/workflows/namespaces/{ns}");
        let app = &app;
        async move { common::get(app, &path, user).await }
    };

    let res = one("a.b").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::json(res).await, row("a.b", 1, 0, 1, true));
    let res = one("a").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::json(res).await, row("a", 0, 0, 0, false));
    for absent in ["dead", "ab", "a.b.c", "b", "a.bc"] {
        assert_eq!(
            one(absent).await.status(),
            StatusCode::NOT_FOUND,
            "{absent}"
        );
    }
    for bad in ["A.B", "-a", ".a", "a%20b"] {
        assert_eq!(one(bad).await.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
}

/// Both routes need `workflow:read`: without it 403, without a session 401.
#[tokio::test]
async fn permissions() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    // No grant yet: a member of nothing in this workspace's workflows.
    seed_flow(&db, ws, user, "a", "f").await;
    let app = common::app(&db).await;
    for path in [
        format!("/api/ws/{ws}/workflows/namespaces"),
        format!("/api/ws/{ws}/workflows/namespaces/a"),
    ] {
        assert_eq!(
            common::get(&app, &path, user).await.status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
        assert_eq!(
            common::anon_get(&app, &path).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    common::grant(&db, ws, user, &["workflow:read"]).await;
    // A fresh app: the first one's session holds the permissions it saw.
    let app = common::app(&db).await;
    for path in [
        format!("/api/ws/{ws}/workflows/namespaces"),
        format!("/api/ws/{ws}/workflows/namespaces/a"),
    ] {
        assert_eq!(
            common::get(&app, &path, user).await.status(),
            StatusCode::OK,
            "{path}"
        );
    }
}

/// The bare `namespaces/{namespace}` route neither shadows nor is shadowed
/// by W9b's `/files` and W9c's `/kv` routes beneath it.
#[tokio::test]
async fn the_detail_route_does_not_clash_with_files_and_kv() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    kv_row(&db, ws, "a", "k", 1, Kv::Live).await;
    let app = common::app(&db).await;

    // W9c's KV listing answers beneath it: this binary installs no store,
    // so it answers its own 503 -- not this task's namespace row.
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/namespaces/a/kv"),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = common::json(res).await;
    assert!(
        body["error"].as_str().unwrap().contains("internal storage"),
        "{body}"
    );
    // W9b's directory listing (rows only): an array, not a namespace row.
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/namespaces/a/files/directory"),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(common::json(res).await.is_array());
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/namespaces/a"), user).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::json(res).await, row("a", 0, 0, 1, true));
}
