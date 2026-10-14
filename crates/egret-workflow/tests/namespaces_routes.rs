//! W7d Tasks 1 and 3: the namespace listing (spec §3.1) and dependencies (§3.4) --
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

/// A live directory, even an empty one, makes its namespace a source
/// (`own: true`) while `files` counts files only; a namespace whose only
/// directory is deleted is not listed (controller ruling, Task 1 fix round).
#[tokio::test]
async fn a_live_folder_alone_makes_a_namespace() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    file_row(&db, ws, "folder.only", "/empty/", 1, false).await;
    file_row(&db, ws, "folder.gone", "/empty/", 1, true).await;
    let app = common::app(&db).await;

    assert_eq!(
        list(&app, ws, user, "").await,
        vec![
            row("folder", 0, 0, 0, false),
            row("folder.only", 0, 0, 0, true),
        ]
    );
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/namespaces/folder.only"),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::json(res).await, row("folder.only", 0, 0, 0, true));
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/namespaces/folder.gone"),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// W7d Task 3: `GET namespaces/{namespace}/dependencies` (spec §3.4)
// ---------------------------------------------------------------------------

/// A flow `ns/id` whose `tasks:` are `body` (already indented by two) and
/// whose top level ends with `extra` (e.g. a `triggers:` block).
fn flow_with(ns: &str, id: &str, body: &str, extra: &str) -> String {
    format!("id: {id}\nnamespace: {ns}\ntasks:\n{body}{extra}")
}

/// A root `Subflow` task calling `ns/id` (both written verbatim).
fn subflow_task(task: &str, ns: &str, id: &str) -> String {
    format!(
        "  - id: {task}\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: \"{ns}\"\n    flowId: \"{id}\"\n"
    )
}

const LOG: &str = "  - id: t\n    type: core.log\n";

/// A `Flow` trigger whose `dependsOn` lists `deps` (`(namespace, flowId)`,
/// either omitted when `None`).
fn flow_trigger(deps: &[(Option<&str>, Option<&str>)]) -> String {
    let mut s = String::from(
        "triggers:\n  - id: on_upstream\n    type: io.kestra.plugin.core.trigger.Flow\n    dependsOn:\n",
    );
    for (ns, id) in deps {
        let mut first = true;
        for (key, v) in [("namespace", ns), ("flowId", id)] {
            if let Some(v) = v {
                s.push_str(if first { "      - " } else { "        " });
                s.push_str(&format!("{key}: {v}\n"));
                first = false;
            }
        }
    }
    s
}

async fn seed_src(db: &Db, ws: Uuid, user: UserId, source: &str) -> Uuid {
    common::seed_flow(db, ws, user, source, "draft").await
}

async fn deps_res(app: &common::App, ws: Uuid, user: UserId, ns: &str) -> axum::response::Response {
    common::get(
        app,
        &format!("/api/ws/{ws}/workflows/namespaces/{ns}/dependencies"),
        user,
    )
    .await
}

async fn deps(app: &common::App, ws: Uuid, user: UserId, ns: &str) -> Value {
    let res = deps_res(app, ws, user, ns).await;
    assert_eq!(res.status(), StatusCode::OK);
    common::json(res).await
}

/// `{ "ns/id": node }`, keyed by the flow's name rather than its uuid.
fn nodes_by_name(body: &Value) -> std::collections::BTreeMap<String, Value> {
    body["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| {
            (
                format!(
                    "{}/{}",
                    n["namespace"].as_str().unwrap(),
                    n["flow_id"].as_str().unwrap()
                ),
                n.clone(),
            )
        })
        .collect()
}

/// The edges as `(source name, target name, kind)`, sorted.
fn edge_names(body: &Value) -> Vec<(String, String, String)> {
    let by_id: std::collections::HashMap<String, String> = nodes_by_name(body)
        .into_iter()
        .map(|(name, n)| (n["id"].as_str().unwrap().to_string(), name))
        .collect();
    let mut out: Vec<_> = body["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|e| {
            let name = |k: &str| {
                by_id
                    .get(e[k].as_str().unwrap())
                    .unwrap_or_else(|| panic!("edge {k} names no node: {e}"))
                    .clone()
            };
            (
                name("source"),
                name("target"),
                e["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn edge(s: &str, t: &str, kind: &str) -> (String, String, String) {
    (s.to_string(), t.to_string(), kind.to_string())
}

/// A literal `Subflow` draws an edge, at the root and under a flowable's
/// `errors:` (and `afterExecution:`); every node carries the spec's six
/// fields, `disabled` from either door (the state column, the YAML key).
#[tokio::test]
async fn literal_subflows_draw_edges_including_under_errors() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let caller = seed_src(
        &db,
        ws,
        user,
        &flow_with(
            "dep",
            "caller",
            &format!(
                "{}  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: inner\n        type: core.log\n    errors:\n      - id: on_err\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: dep\n        flowId: handler\n",
                subflow_task("call", "dep", "callee")
            ),
            &format!("afterExecution:\n{}", subflow_task("after", "dep", "after")),
        ),
    )
    .await;
    common::seed_flow(&db, ws, user, &flow("dep", "callee"), "disabled").await;
    seed_src(
        &db,
        ws,
        user,
        &format!("{}disabled: true\n", flow("dep", "handler")),
    )
    .await;
    seed_flow(&db, ws, user, "dep", "after").await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "dep").await;
    assert_eq!(
        edge_names(&body),
        vec![
            edge("dep/caller", "dep/after", "subflow"),
            edge("dep/caller", "dep/callee", "subflow"),
            edge("dep/caller", "dep/handler", "subflow"),
        ]
    );
    let nodes = nodes_by_name(&body);
    assert_eq!(
        nodes["dep/caller"],
        json!({
            "id": caller, "namespace": "dep", "flow_id": "caller",
            "disabled": false, "in_namespace": true, "dynamic_subflows": 0,
            "unparsable": false,
        })
    );
    assert_eq!(nodes["dep/callee"]["disabled"], json!(true), "state column");
    assert_eq!(nodes["dep/handler"]["disabled"], json!(true), "YAML key");
    assert_eq!(nodes["dep/after"]["disabled"], json!(false));
    assert_eq!(body["unparsable"], json!(0));
}

/// A `Subflow` whose `namespace` or `flowId` is an expression (`{{ }}` or
/// `{% %}`) draws no edge and is counted on its flow's node; a literal one
/// naming no flow draws nothing and is not counted.
#[tokio::test]
async fn a_dynamic_subflow_draws_no_edge_and_is_counted() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with(
            "dyn",
            "caller",
            &format!(
                "{}{}{}",
                subflow_task("a", "{{ inputs.ns }}", "target"),
                subflow_task("b", "dyn", "{% if true %}target{% endif %}"),
                subflow_task("c", "dyn", "missing"),
            ),
            "",
        ),
    )
    .await;
    seed_flow(&db, ws, user, "dyn", "target").await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "dyn").await;
    assert_eq!(edge_names(&body), vec![]);
    let nodes = nodes_by_name(&body);
    assert_eq!(nodes["dyn/caller"]["dynamic_subflows"], json!(2));
    assert_eq!(nodes["dyn/target"]["dynamic_subflows"], json!(0));
}

/// A `Flow` trigger's `dependsOn` entry naming `namespace` AND `flowId` of
/// an existing flow draws `upstream -> listener`. An entry with only a
/// namespace draws nothing: egret's `dependency_matches` fails closed on a
/// missing field, so such a listener is never fired by any flow.
#[tokio::test]
async fn a_flow_trigger_draws_an_edge_from_the_flow_it_names() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_flow(&db, ws, user, "ft", "up").await;
    seed_flow(&db, ws, user, "ft", "other").await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with(
            "ft",
            "listener",
            LOG,
            &flow_trigger(&[(Some("ft"), Some("up")), (Some("ft"), None)]),
        ),
    )
    .await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "ft").await;
    assert_eq!(
        edge_names(&body),
        vec![edge("ft/up", "ft/listener", "flow_trigger")]
    );
    assert_eq!(
        nodes_by_name(&body).len(),
        3,
        "ft/other is a node, edge-less"
    );
}

/// Edges into the namespace and out of it bring their outside end along with
/// `in_namespace: false`; an outside flow with no edge to the namespace, and
/// an edge between two outside flows, are absent.
#[tokio::test]
async fn edges_across_the_namespace_boundary_bring_the_outside_flow() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("in", "mid", &subflow_task("out", "outer", "callee"), ""),
    )
    .await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("outer", "caller", &subflow_task("in", "in", "mid"), ""),
    )
    .await;
    seed_flow(&db, ws, user, "outer", "callee").await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("outer", "lonely", &subflow_task("x", "outer", "callee"), ""),
    )
    .await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with(
            "outer",
            "watcher",
            LOG,
            &flow_trigger(&[(Some("in"), Some("mid"))]),
        ),
    )
    .await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "in").await;
    assert_eq!(
        edge_names(&body),
        vec![
            edge("in/mid", "outer/callee", "subflow"),
            edge("in/mid", "outer/watcher", "flow_trigger"),
            edge("outer/caller", "in/mid", "subflow"),
        ]
    );
    let nodes = nodes_by_name(&body);
    assert_eq!(
        nodes.keys().cloned().collect::<Vec<_>>(),
        vec!["in/mid", "outer/callee", "outer/caller", "outer/watcher"]
    );
    assert_eq!(nodes["in/mid"]["in_namespace"], json!(true));
    for n in ["outer/callee", "outer/caller", "outer/watcher"] {
        assert_eq!(nodes[n]["in_namespace"], json!(false), "{n}");
    }
}

/// The namespace's children (dot-anchored) are inside it; a sibling sharing
/// its prefix (`kidsx`) and its parent are not.
#[tokio::test]
async fn child_namespaces_are_inside() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_flow(&db, ws, user, "kids", "top").await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("kids.a.b", "deep", &subflow_task("s", "kids", "top"), ""),
    )
    .await;
    seed_flow(&db, ws, user, "kidsx", "sibling").await;
    seed_flow(&db, ws, user, "parent", "p").await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "kids").await;
    let nodes = nodes_by_name(&body);
    assert_eq!(
        nodes.keys().cloned().collect::<Vec<_>>(),
        vec!["kids.a.b/deep", "kids/top"]
    );
    assert!(nodes.values().all(|n| n["in_namespace"] == json!(true)));
    assert_eq!(
        edge_names(&body),
        vec![edge("kids.a.b/deep", "kids/top", "subflow")]
    );
}

/// A deleted flow is no node, and neither its edges nor edges naming it are
/// drawn.
#[tokio::test]
async fn a_deleted_flow_is_absent() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("del", "caller", &subflow_task("s", "del", "gone"), ""),
    )
    .await;
    let gone = seed_flow(&db, ws, user, "del", "gone").await;
    let gone_caller = seed_src(
        &db,
        ws,
        user,
        &flow_with(
            "del",
            "gone_caller",
            &subflow_task("s", "del", "caller"),
            "",
        ),
    )
    .await;
    soft_delete_flow(&db, gone).await;
    soft_delete_flow(&db, gone_caller).await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "del").await;
    assert_eq!(
        nodes_by_name(&body).keys().cloned().collect::<Vec<_>>(),
        vec!["del/caller"]
    );
    assert_eq!(edge_names(&body), vec![]);
}

/// Another workspace's flows are neither nodes nor edge ends, even under the
/// same names.
#[tokio::test]
async fn another_workspace_is_invisible_to_dependencies() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let (other, other_user) = common::seed_workspace(&db).await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("mine", "caller", &subflow_task("s", "theirs", "t"), ""),
    )
    .await;
    seed_flow(&db, other, other_user, "theirs", "t").await;
    seed_flow(&db, other, other_user, "mine", "intruder").await;
    seed_src(
        &db,
        other,
        other_user,
        &flow_with("theirs", "c", &subflow_task("s", "mine", "caller"), ""),
    )
    .await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "mine").await;
    assert_eq!(
        nodes_by_name(&body).keys().cloned().collect::<Vec<_>>(),
        vec!["mine/caller"]
    );
    assert_eq!(edge_names(&body), vec![]);
}

/// Inserts `n` minimal drafts `bulk/f{i}` straight into the table (the
/// repository's create would take minutes at this count).
async fn bulk_flows(db: &Db, ws: Uuid, ns: &str, n: i32) {
    sqlx::query(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan) \
         SELECT $1, $2, 'f' || i, \
                'id: f' || i || E'\\nnamespace: ' || $2 || E'\\ntasks:\\n  - id: t\\n    type: core.log\\n', \
                '{}'::jsonb \
           FROM generate_series(1, $3) AS i",
    )
    .bind(ws)
    .bind(ns)
    .bind(n)
    .execute(&db.pool)
    .await
    .expect("bulk flows");
}

/// The limit is the WORKSPACE's flow count (not deleted), since every draft
/// is read: 2,000 answer, 2,001 is a 422 with a message -- even for a
/// namespace holding none of them -- and a deleted 2,001st does not count.
#[tokio::test]
async fn more_than_two_thousand_flows_answer_422() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    bulk_flows(&db, ws, "bulk", 2_000).await;
    let extra = seed_flow(&db, ws, user, "elsewhere", "f").await;
    soft_delete_flow(&db, extra).await;
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "bulk").await;
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2_000);

    seed_flow(&db, ws, user, "elsewhere", "g").await;
    for ns in ["bulk", "elsewhere", "empty"] {
        let res = deps_res(&app, ws, user, ns).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "{ns}");
        let body = common::json(res).await;
        assert_eq!(body["code"], json!("dependencies_too_large"));
        assert!(body["error"].as_str().unwrap().contains("2000"), "{body}");
    }
}

/// A draft whose YAML does not parse stays a node, flagged `unparsable`,
/// with no edges of its own -- but a literal `Subflow` INTO it from a
/// parsable flow still draws. It is counted in the top-level `unparsable`
/// over every draft read (the workspace's, because one outside the namespace
/// can hide an edge into it); a broken draft outside with no edge to the
/// namespace is counted but is no node. The graph still answers 200.
#[tokio::test]
async fn an_unparsable_draft_stays_a_flagged_node_and_is_counted() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    seed_src(
        &db,
        ws,
        user,
        &flow_with("bad", "ok", &subflow_task("s", "bad", "broken"), ""),
    )
    .await;
    // Before it broke, `broken` called `bad/ok`: that edge is now unknown.
    let broken_in = seed_src(
        &db,
        ws,
        user,
        &flow_with("bad", "broken", &subflow_task("s", "bad", "ok"), ""),
    )
    .await;
    let broken_out = seed_flow(&db, ws, user, "elsewhere", "broken").await;
    for id in [broken_in, broken_out] {
        sqlx::query("UPDATE workflow.flows SET source = 'tasks: [unclosed' WHERE id = $1")
            .bind(id)
            .execute(&db.pool)
            .await
            .expect("break the draft");
    }
    let app = common::app(&db).await;

    let body = deps(&app, ws, user, "bad").await;
    let nodes = nodes_by_name(&body);
    assert_eq!(
        nodes.keys().cloned().collect::<Vec<_>>(),
        vec!["bad/broken", "bad/ok"]
    );
    assert_eq!(nodes["bad/broken"]["unparsable"], json!(true));
    assert_eq!(nodes["bad/broken"]["in_namespace"], json!(true));
    assert_eq!(nodes["bad/ok"]["unparsable"], json!(false));
    assert_eq!(
        edge_names(&body),
        vec![edge("bad/ok", "bad/broken", "subflow")]
    );
    assert_eq!(body["unparsable"], json!(2));
}

/// An empty namespace answers an empty graph (not 404); a name outside the
/// flow grammar is 400; the route needs `workflow:read`.
#[tokio::test]
async fn dependencies_validation_and_permission() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let app = common::app(&db).await;
    assert_eq!(
        deps_res(&app, ws, user, "a").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        common::anon_get(
            &app,
            &format!("/api/ws/{ws}/workflows/namespaces/a/dependencies")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    assert_eq!(
        deps(&app, ws, user, "nothing.here").await,
        json!({ "nodes": [], "edges": [], "unparsable": 0 })
    );
    assert_eq!(
        deps_res(&app, ws, user, "Bad_NS").await.status(),
        StatusCode::BAD_REQUEST
    );
}
