//! W7c Task 2: the two flow-graph routes and `Subflow` expansion (spec §3.4,
//! §3.5, §5), driven over real HTTP against a real `CedarPdp` and the
//! branch's Postgres. DB-gated; self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is
//! set.
//!
//! ```text
//! GET  /api/ws/{workspace}/workflows/flows/{id}/graph?version=&subflows=   workflow:read
//! POST /api/ws/{workspace}/workflows/flows/graph?subflows=                 workflow:manage
//! ```

mod common;

use axum::http::StatusCode;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::graph::{flow_graph, MAX_NODES};
use serde_json::{json, Value};
use uuid::Uuid;

const LOG: &str = "io.kestra.plugin.core.log.Log";
const FLOW: &str = "io.kestra.plugin.core.flow";

fn leaf(id: &str) -> String {
    format!("{{id: \"{id}\", type: {LOG}, message: m}}")
}

fn subflow(id: &str, namespace: &str, flow_id: &str) -> String {
    format!("{{id: {id}, type: {FLOW}.Subflow, namespace: \"{namespace}\", flowId: \"{flow_id}\"}}")
}

fn flow_yaml(namespace: &str, id: &str, tasks: &[String]) -> String {
    format!(
        "id: {id}\nnamespace: {namespace}\ntasks: [{}]\n",
        tasks.join(", ")
    )
}

/// What `flow_graph` answers for `source`, as the route serialises it.
fn expected(source: &str) -> Value {
    let flow = egret_workflow::spec::parse(source).expect("parse");
    // The deployment the routes judge against: the one this binary installs.
    common::install_served_default();
    let served = egret_workflow::runtime::served();
    serde_json::to_value(flow_graph(&flow, Some(served)).expect("graph")).unwrap()
}

/// `?subflows=a&subflows=b`, each value percent-encoded (`/` as `%2F`).
fn subflows_query(uids: &[&str]) -> String {
    uids.iter()
        .map(|u| format!("subflows={}", u.replace('/', "%2F")))
        .collect::<Vec<_>>()
        .join("&")
}

async fn reader(db: &Db) -> (Uuid, UserId) {
    let (ws, user) = common::seed_workspace(db).await;
    common::grant(db, ws, user, &["workflow:read", "workflow:manage"]).await;
    (ws, user)
}

async fn get_graph(
    app: &common::App,
    ws: Uuid,
    id: Uuid,
    query: &str,
    user: UserId,
) -> axum::response::Response {
    common::get(
        app,
        &format!("/api/ws/{ws}/workflows/flows/{id}/graph?{query}"),
        user,
    )
    .await
}

async fn post_graph(
    app: &common::App,
    ws: Uuid,
    query: &str,
    source: &str,
    user: UserId,
) -> axum::response::Response {
    common::post(
        app,
        &format!("/api/ws/{ws}/workflows/flows/graph?{query}"),
        user,
        json!({ "source": source }),
    )
    .await
}

fn uids(g: &Value) -> Vec<String> {
    g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["uid"].as_str().unwrap().to_string())
        .collect()
}

fn node<'g>(g: &'g Value, uid: &str) -> &'g Value {
    g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["uid"] == uid)
        .unwrap_or_else(|| panic!("no node {uid}: {}", uids(g).join(", ")))
}

fn cluster<'g>(g: &'g Value, uid: &str) -> &'g Value {
    g["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["uid"] == uid)
        .unwrap_or_else(|| panic!("no cluster {uid}: {}", g["clusters"]))
}

fn edge_set(g: &Value) -> std::collections::BTreeSet<(String, String, String)> {
    g["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["source"].as_str().unwrap().to_string(),
                e["target"].as_str().unwrap().to_string(),
                e["relation"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn es(v: &[(&str, &str, &str)]) -> std::collections::BTreeSet<(String, String, String)> {
    v.iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect()
}

// ---- 1. GET the draft graph, and a version's -----------------------------

#[tokio::test]
async fn get_answers_the_graph_of_the_stored_draft() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let source = format!(
        "id: f\nnamespace: company.team\ntasks:\n  - {{id: s, type: {FLOW}.Sequential, tasks: [{}, {}]}}\nerrors: [{}]\ntriggers:\n  - {{id: daily, type: io.kestra.plugin.core.trigger.Schedule, cron: \"0 0 * * *\"}}\n",
        leaf("a"),
        leaf("b"),
        leaf("alert"),
    );
    let id = common::seed_flow(&db, ws, user, &source, "draft").await;
    let app = common::app(&db).await;

    let res = get_graph(&app, ws, id, "", user).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::json(res).await, expected(&source));
}

#[tokio::test]
async fn get_with_a_version_answers_that_versions_stored_source() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let draft = flow_yaml("company.team", "f", &[leaf("now")]);
    let published = flow_yaml("company.team", "f", &[leaf("then"), leaf("before")]);
    let id = common::seed_flow(&db, ws, user, &draft, "draft").await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let v = repo
        .insert_version(
            id,
            user,
            &json!({ "source": published, "plan": {}, "warnings": [] }),
            "",
        )
        .await
        .expect("insert version");
    let app = common::app(&db).await;

    let res = get_graph(&app, ws, id, &format!("version={}", v.version), user).await;
    assert_eq!(res.status(), StatusCode::OK);
    let got = common::json(res).await;
    assert_eq!(got, expected(&published));
    assert_ne!(got, expected(&draft), "the version, not the draft");

    let res = get_graph(&app, ws, id, "", user).await;
    assert_eq!(common::json(res).await, expected(&draft));
}

// ---- 2. POST unsaved YAML ------------------------------------------------

#[tokio::test]
async fn post_answers_the_graph_of_unsaved_yaml() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let source = format!(
        "id: unsaved\nnamespace: company.team\ntasks:\n  - {{id: p, type: {FLOW}.Parallel, tasks: [{}, {}]}}\n",
        leaf("x"),
        leaf("y")
    );
    let app = common::app(&db).await;
    let res = post_graph(&app, ws, "", &source, user).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::json(res).await, expected(&source));
}

#[tokio::test]
async fn post_yaml_that_does_not_parse_is_400_with_its_line_and_column() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let app = common::app(&db).await;
    let res = post_graph(
        &app,
        ws,
        "",
        "id: f\nnamespace: company.team\ntasks:\n  - id: [unclosed\n",
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::json(res).await;
    let msg = body["error"].as_str().unwrap();
    assert!(
        msg.contains("(line ") && msg.contains(", column "),
        "the message carries the line and column: {msg}"
    );
}

#[tokio::test]
async fn post_yaml_that_fails_validation_still_gets_its_graph_with_refusals() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let source = format!(
        "id: f\nnamespace: company.team\ntasks:\n  - {}\n  - {{id: bad, type: no.such.Type}}\n",
        leaf("a")
    );
    let app = common::app(&db).await;
    let res = post_graph(&app, ws, "", &source, user).await;
    assert_eq!(res.status(), StatusCode::OK);
    let g = common::json(res).await;
    assert_eq!(g, expected(&source));
    assert_eq!(node(&g, "tasks.bad")["refusal"], "unknown_task_type");
    assert_eq!(node(&g, "tasks.a")["refusal"], Value::Null);
}

/// The final review's Important: the graph marks what THIS deployment will
/// not run, as the validate routes, the draft DTO and publish do. This binary
/// serves the runner set (`common::install_served_default`), which still has
/// no handler for `egret.report.send` (deferred, spec D10; the probe was
/// `egret.sql` until package #4 Task C5 served it); a `Subflow` child
/// expanded in place is judged against the same set.
#[tokio::test]
async fn a_task_type_this_deployment_does_not_serve_is_refused_here_and_in_an_expansion() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    assert!(
        !egret_workflow::runtime::served().serves("egret.report.send"),
        "the fixture needs a type this binary does not serve"
    );
    let unserved = "{id: q, type: egret.report.send, chart: c}";
    common::seed_flow(
        &db,
        ws,
        user,
        &flow_yaml("company.child", "c", &[unserved.to_string(), leaf("ok")]),
        "draft",
    )
    .await;
    let source = format!(
        "id: f\nnamespace: company.team\ntasks:\n  - {unserved}\n  - {}\n  - {}\n",
        leaf("a"),
        subflow("call", "company.child", "c")
    );
    let app = common::app(&db).await;
    let res = post_graph(&app, ws, &subflows_query(&["tasks.call"]), &source, user).await;
    assert_eq!(res.status(), StatusCode::OK);
    let g = common::json(res).await;
    assert_eq!(node(&g, "tasks.q")["refusal"], "task_type_not_served");
    assert_eq!(node(&g, "tasks.a")["refusal"], Value::Null);
    assert_eq!(
        node(&g, "tasks.call/tasks.q")["refusal"],
        "task_type_not_served"
    );
    assert_eq!(node(&g, "tasks.call/tasks.ok")["refusal"], Value::Null);
    assert_eq!(node(&g, "tasks.call")["refusal"], Value::Null);
}

// ---- 3. Permissions -----------------------------------------------------

#[tokio::test]
async fn get_needs_workflow_read_and_post_needs_workflow_manage() {
    let Some(db) = common::db().await else { return };
    let (ws, owner) = common::seed_workspace(&db).await;
    let source = flow_yaml("company.team", "f", &[leaf("a")]);
    let id = common::seed_flow(&db, ws, owner, &source, "draft").await;

    let (_, read_only) = common::seed_workspace(&db).await;
    common::grant(&db, ws, read_only, &["workflow:read"]).await;
    let (_, manage_only) = common::seed_workspace(&db).await;
    common::grant(&db, ws, manage_only, &["workflow:manage"]).await;
    let (_, nothing) = common::seed_workspace(&db).await;
    common::grant(&db, ws, nothing, &["workflow:use_secrets"]).await;

    let app = common::app(&db).await;
    assert_eq!(get_graph(&app, ws, id, "", read_only).await.status(), 200);
    assert_eq!(get_graph(&app, ws, id, "", manage_only).await.status(), 403);
    assert_eq!(get_graph(&app, ws, id, "", nothing).await.status(), 403);

    assert_eq!(
        post_graph(&app, ws, "", &source, manage_only)
            .await
            .status(),
        200
    );
    assert_eq!(
        post_graph(&app, ws, "", &source, read_only).await.status(),
        403
    );
    assert_eq!(
        post_graph(&app, ws, "", &source, nothing).await.status(),
        403
    );

    let anon = common::anon_get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}/graph")).await;
    assert_eq!(anon.status(), 401);
}

// ---- 4. 404 for another workspace ----------------------------------------

#[tokio::test]
async fn a_flow_or_version_of_another_workspace_is_404() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let (other, other_user) = common::seed_workspace(&db).await;
    let source = flow_yaml("company.team", "f", &[leaf("a")]);
    let foreign = common::seed_flow(&db, other, other_user, &source, "draft").await;
    let mine = common::seed_flow(&db, ws, user, &source, "draft").await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let snapshot = json!({ "source": source, "plan": {}, "warnings": [] });
    repo.insert_version(foreign, other_user, &snapshot, "")
        .await
        .unwrap();
    repo.insert_version(mine, user, &snapshot, "")
        .await
        .unwrap();
    let app = common::app(&db).await;

    // The foreign flow, through my workspace's path.
    assert_eq!(get_graph(&app, ws, foreign, "", user).await.status(), 404);
    assert_eq!(
        get_graph(&app, ws, foreign, "version=1", user)
            .await
            .status(),
        404
    );
    // Control: my own flow's version 1 is there, and version 2 is not.
    assert_eq!(
        get_graph(&app, ws, mine, "version=1", user).await.status(),
        200
    );
    assert_eq!(
        get_graph(&app, ws, mine, "version=2", user).await.status(),
        404
    );
    assert_eq!(
        get_graph(&app, ws, Uuid::new_v4(), "", user).await.status(),
        404
    );
}

// ---- 5. The node limit --------------------------------------------------

#[tokio::test]
async fn a_graph_over_the_node_limit_is_422_naming_the_count() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    // Two markers plus N leaves.
    let tasks: Vec<String> = (0..MAX_NODES - 1).map(|i| leaf(&format!("t{i}"))).collect();
    let source = flow_yaml("company.team", "big", &tasks);
    let app = common::app(&db).await;
    let res = post_graph(&app, ws, "", &source, user).await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = common::json(res).await;
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains(&(MAX_NODES + 1).to_string()), "{msg}");

    // Exactly the limit is answered.
    let tasks: Vec<String> = (0..MAX_NODES - 2).map(|i| leaf(&format!("t{i}"))).collect();
    let res = post_graph(
        &app,
        ws,
        "",
        &flow_yaml("company.team", "big", &tasks),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_node_limit_counts_nodes_after_expansion() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    // 3,002 nodes each: under the limit alone, over it twice.
    let child_tasks: Vec<String> = (0..3_000).map(|i| leaf(&format!("t{i}"))).collect();
    common::seed_flow(
        &db,
        ws,
        user,
        &flow_yaml("company.child", "big", &child_tasks),
        "draft",
    )
    .await;
    let parent = flow_yaml(
        "company.team",
        "p",
        &[
            subflow("one", "company.child", "big"),
            subflow("two", "company.child", "big"),
        ],
    );
    let app = common::app(&db).await;

    let one = post_graph(&app, ws, &subflows_query(&["tasks.one"]), &parent, user).await;
    assert_eq!(one.status(), StatusCode::OK);
    let g = common::json(one).await;
    // The parent's 4 nodes, the child's 3,002 (its `start:flow` is the
    // Subflow node itself, its `end:flow` becomes `end:tasks.one`), and the
    // expansion's own `start:tasks.one`.
    assert_eq!(g["nodes"].as_array().unwrap().len(), 4 + 3_002);

    let both = post_graph(
        &app,
        ws,
        &subflows_query(&["tasks.one", "tasks.two"]),
        &parent,
        user,
    )
    .await;
    assert_eq!(both.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = common::json(both).await;
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("5000"), "names the limit: {msg}");
}

// ---- 6. Subflow expansion -----------------------------------------------

#[tokio::test]
async fn an_expanded_subflow_is_a_prefixed_cluster_under_the_subflow_node() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let child = format!(
        "id: child\nnamespace: company.child\ntasks:\n  - {}\n  - {{id: p, type: {FLOW}.Parallel, tasks: [{}]}}\n  - {{id: bad, type: no.such.Type}}\n",
        leaf("x"),
        leaf("y"),
    );
    common::seed_flow(&db, ws, user, &child, "draft").await;
    let parent = format!(
        "id: p\nnamespace: company.team\ntasks:\n  - {}\n  - {{id: seq, type: {FLOW}.Sequential, tasks: [{}]}}\n  - {}\n",
        leaf("a"),
        subflow("call", "company.child", "child"),
        leaf("b"),
    );
    let id = common::seed_flow(&db, ws, user, &parent, "draft").await;
    let app = common::app(&db).await;

    let res = get_graph(&app, ws, id, &subflows_query(&["tasks.seq.call"]), user).await;
    assert_eq!(res.status(), StatusCode::OK);
    let g = common::json(res).await;
    let s = "tasks.seq.call";

    let mut got = uids(&g);
    got.sort();
    let mut want: Vec<String> = [
        "start:flow",
        "end:flow",
        "tasks.a",
        "tasks.b",
        "start:tasks.seq",
        "tasks.seq",
        "end:tasks.seq",
        // The expansion.
        "start:tasks.seq.call",
        "tasks.seq.call",
        "end:tasks.seq.call",
        "tasks.seq.call/tasks.x",
        "start:tasks.seq.call/tasks.p",
        "tasks.seq.call/tasks.p",
        "tasks.seq.call/tasks.p.y",
        "end:tasks.seq.call/tasks.p",
        "tasks.seq.call/tasks.bad",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    want.sort();
    assert_eq!(got, want);

    let c = cluster(&g, &format!("cluster:{s}"));
    assert_eq!(c["kind"], "subflow");
    assert_eq!(c["task_id"], "call");
    assert_eq!(c["parent"], "cluster:tasks.seq");
    assert_eq!(c["reason"], Value::Null);
    assert_eq!(c["start"], format!("start:{s}"));
    assert_eq!(c["end"], format!("end:{s}"));
    assert_eq!(
        c["nodes"],
        json!([
            format!("start:{s}"),
            s,
            format!("{s}/tasks.x"),
            format!("{s}/tasks.bad"),
            format!("end:{s}"),
        ])
    );
    // The Subflow node moved from the Sequential's cluster into its own.
    assert_eq!(
        cluster(&g, "cluster:tasks.seq")["nodes"],
        json!(["start:tasks.seq", "tasks.seq", "end:tasks.seq"])
    );
    let p = cluster(&g, &format!("cluster:{s}/tasks.p"));
    assert_eq!(p["kind"], "parallel");
    assert_eq!(p["parent"], format!("cluster:{s}"));
    assert_eq!(
        p["nodes"],
        json!([
            format!("start:{s}/tasks.p"),
            format!("{s}/tasks.p"),
            format!("{s}/tasks.p.y"),
            format!("end:{s}/tasks.p"),
        ])
    );

    // The child's refusal comes along, at its prefixed uid.
    assert_eq!(
        node(&g, &format!("{s}/tasks.bad"))["refusal"],
        "unknown_task_type"
    );
    assert_eq!(node(&g, s)["kind"], "executable");

    let edges = edge_set(&g);
    let want = es(&[
        ("start:flow", "tasks.a", "sequential"),
        ("tasks.a", "start:tasks.seq", "sequential"),
        ("start:tasks.seq", "tasks.seq", "sequential"),
        // Into the expansion where the Subflow node was entered ...
        ("tasks.seq", "start:tasks.seq.call", "sequential"),
        ("start:tasks.seq.call", "tasks.seq.call", "sequential"),
        // ... the child's flow, leaving from the Subflow node ...
        ("tasks.seq.call", "tasks.seq.call/tasks.x", "sequential"),
        (
            "tasks.seq.call/tasks.x",
            "start:tasks.seq.call/tasks.p",
            "sequential",
        ),
        (
            "start:tasks.seq.call/tasks.p",
            "tasks.seq.call/tasks.p",
            "sequential",
        ),
        (
            "tasks.seq.call/tasks.p",
            "tasks.seq.call/tasks.p.y",
            "parallel",
        ),
        (
            "tasks.seq.call/tasks.p.y",
            "end:tasks.seq.call/tasks.p",
            "parallel",
        ),
        (
            "end:tasks.seq.call/tasks.p",
            "tasks.seq.call/tasks.bad",
            "sequential",
        ),
        (
            "tasks.seq.call/tasks.bad",
            "end:tasks.seq.call",
            "sequential",
        ),
        // ... and out where it was left.
        ("end:tasks.seq.call", "end:tasks.seq", "sequential"),
        ("end:tasks.seq", "tasks.b", "sequential"),
        ("tasks.b", "end:flow", "sequential"),
    ]);
    assert_eq!(edges, want);
    assert_eq!(edges.len(), g["edges"].as_array().unwrap().len());
}

#[tokio::test]
async fn a_subflow_in_an_errors_branch_gives_its_expansion_that_branch() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    common::seed_flow(
        &db,
        ws,
        user,
        &flow_yaml("company.child", "child", &[leaf("x")]),
        "draft",
    )
    .await;
    let parent = format!(
        "id: p\nnamespace: company.team\ntasks: [{}]\nerrors: [{}]\n",
        leaf("a"),
        subflow("alert", "company.child", "child")
    );
    let app = common::app(&db).await;
    let g =
        common::json(post_graph(&app, ws, &subflows_query(&["errors.alert"]), &parent, user).await)
            .await;
    assert_eq!(node(&g, "errors.alert/tasks.x")["branch"], "errors");
}

#[tokio::test]
async fn an_unresolvable_subflow_is_an_empty_cluster_with_not_found() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    let (other, other_user) = common::seed_workspace(&db).await;
    // A flow of the very name, in ANOTHER workspace.
    common::seed_flow(
        &db,
        other,
        other_user,
        &flow_yaml("company.child", "elsewhere", &[leaf("x")]),
        "draft",
    )
    .await;
    let parent = flow_yaml(
        "company.team",
        "p",
        &[
            subflow("missing", "company.child", "nowhere"),
            subflow("foreign", "company.child", "elsewhere"),
        ],
    );
    let id = common::seed_flow(&db, ws, user, &parent, "draft").await;
    let app = common::app(&db).await;

    let res = get_graph(
        &app,
        ws,
        id,
        &subflows_query(&["tasks.missing", "tasks.foreign"]),
        user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let g = common::json(res).await;
    for s in ["tasks.missing", "tasks.foreign"] {
        let c = cluster(&g, &format!("cluster:{s}"));
        assert_eq!(c["reason"], "not_found", "{s}");
        assert_eq!(c["nodes"], json!([]), "{s}");
        assert_eq!(c["kind"], "subflow");
        assert_eq!(c["parent"], Value::Null);
    }
    // Nothing of the foreign flow leaked in, and the graph is otherwise the
    // unexpanded one.
    assert!(!uids(&g).iter().any(|u| u.contains('/')), "{:?}", uids(&g));
    let plain = expected(&parent);
    assert_eq!(g["nodes"], plain["nodes"]);
    assert_eq!(g["edges"], plain["edges"]);
}

#[tokio::test]
async fn a_subflow_named_by_an_expression_is_dynamic() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    common::seed_flow(
        &db,
        ws,
        user,
        &flow_yaml("company.child", "child", &[leaf("x")]),
        "draft",
    )
    .await;
    let parent = flow_yaml(
        "company.team",
        "p",
        &[
            subflow("ns", "{{ inputs.ns }}", "child"),
            subflow("fid", "company.child", "{{ inputs.f }}"),
        ],
    );
    let app = common::app(&db).await;
    let g = common::json(
        post_graph(
            &app,
            ws,
            &subflows_query(&["tasks.ns", "tasks.fid"]),
            &parent,
            user,
        )
        .await,
    )
    .await;
    for s in ["tasks.ns", "tasks.fid"] {
        let c = cluster(&g, &format!("cluster:{s}"));
        assert_eq!(c["reason"], "dynamic", "{s}");
        assert_eq!(c["nodes"], json!([]), "{s}");
    }
}

#[tokio::test]
async fn expansion_stops_at_two_levels_and_the_third_is_depth() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    // l1 -> l2 -> l3 -> l4, each calling the next as `call`.
    for (n, next) in [(1, Some(2)), (2, Some(3)), (3, Some(4)), (4, None)] {
        let mut tasks = vec![leaf(&format!("own{n}"))];
        if let Some(next) = next {
            tasks.push(subflow("call", "company.chain", &format!("l{next}")));
        }
        common::seed_flow(
            &db,
            ws,
            user,
            &flow_yaml("company.chain", &format!("l{n}"), &tasks),
            "draft",
        )
        .await;
    }
    let parent = flow_yaml(
        "company.team",
        "p",
        &[subflow("call", "company.chain", "l1")],
    );
    let app = common::app(&db).await;
    let g = common::json(
        post_graph(
            &app,
            ws,
            &subflows_query(&[
                "tasks.call",
                "tasks.call/tasks.call",
                "tasks.call/tasks.call/tasks.call",
            ]),
            &parent,
            user,
        )
        .await,
    )
    .await;

    // Level 1 and level 2 expand.
    node(&g, "tasks.call/tasks.own1");
    node(&g, "tasks.call/tasks.call/tasks.own2");
    assert_eq!(cluster(&g, "cluster:tasks.call")["reason"], Value::Null);
    let l2 = cluster(&g, "cluster:tasks.call/tasks.call");
    assert_eq!(l2["reason"], Value::Null);
    assert_eq!(l2["parent"], "cluster:tasks.call");
    // The third does not.
    let l3 = cluster(&g, "cluster:tasks.call/tasks.call/tasks.call");
    assert_eq!(l3["reason"], "depth");
    assert_eq!(l3["nodes"], json!([]));
    assert_eq!(l3["parent"], "cluster:tasks.call/tasks.call");
    assert!(
        !uids(&g).iter().any(|u| u.contains("own3")),
        "nothing of level 3 is drawn: {:?}",
        uids(&g)
    );
}

#[tokio::test]
async fn several_subflows_expand_at_once_and_an_unknown_uid_is_ignored() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = reader(&db).await;
    common::seed_flow(
        &db,
        ws,
        user,
        &flow_yaml("company.child", "one", &[leaf("x")]),
        "draft",
    )
    .await;
    common::seed_flow(
        &db,
        ws,
        user,
        &flow_yaml("company.child", "two", &[leaf("y")]),
        "draft",
    )
    .await;
    let parent = flow_yaml(
        "company.team",
        "p",
        &[
            leaf("a"),
            subflow("first", "company.child", "one"),
            subflow("second", "company.child", "two"),
        ],
    );
    let app = common::app(&db).await;
    let g = common::json(
        post_graph(
            &app,
            ws,
            &subflows_query(&["tasks.first", "tasks.second", "tasks.nope", "tasks.a"]),
            &parent,
            user,
        )
        .await,
    )
    .await;
    node(&g, "tasks.first/tasks.x");
    node(&g, "tasks.second/tasks.y");
    let subflow_clusters: Vec<&str> = g["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["uid"].as_str().unwrap())
        .collect();
    assert_eq!(
        subflow_clusters,
        ["cluster:tasks.first", "cluster:tasks.second"],
        "an unknown uid and a uid that is not a Subflow expand nothing"
    );

    // Only unknown uids: the plain graph, exactly.
    let g = common::json(
        post_graph(
            &app,
            ws,
            &subflows_query(&["tasks.nope", "tasks.a", "nothing/at.all"]),
            &parent,
            user,
        )
        .await,
    )
    .await;
    assert_eq!(g, expected(&parent));
}

// =========================================================================
// W7c Task 3: `GET executions/{id}/graph` (spec §4, §5).
//
// Every test here drives the REAL worker on a private database
// (`#[sqlx::test]`): `worker::tick` is global and would take a neighbour's
// runs on the shared one, and what is under test is what the production
// writer leaves behind, not what a fixture chose to insert.
// =========================================================================

use chrono::{DateTime, Utc};
use egret_workflow::worker::{self, Outcome, Registry, TaskInput};

/// Renders a leaf's `message` and answers by it: `boom` fails every attempt,
/// `once` fails the first attempt only, anything else succeeds.
struct ByMessage;

#[async_trait::async_trait]
impl Registry for ByMessage {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let raw = task.spec["message"].as_str().unwrap_or_default();
        let msg = egret_workflow::render_str(raw, &task.render).unwrap_or_else(|_| raw.into());
        Ok(match (msg.as_str(), task.attempt) {
            ("boom", _) | ("once", 1) => Outcome::failed_with(format!("{msg} fails")),
            _ => Outcome::success(),
        })
    }
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

async fn seeded(pool: &sqlx::PgPool) -> (Db, Uuid, UserId) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    (db, ws, user)
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, flow: Option<Uuid>, source: &str) -> Uuid {
    worker::start_execution(pool, ws, flow, source, json!({}))
        .await
        .expect("start execution")
}

/// Tick until the worker claims nothing.
async fn drain(pool: &sqlx::PgPool) {
    for _ in 0..64 {
        if worker::tick(pool, 16, &ByMessage).await.expect("tick") == 0 {
            return;
        }
    }
    panic!("the worker never stopped claiming");
}

async fn execution_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution state")
}

#[derive(Debug, sqlx::FromRow)]
struct RunRow {
    id: Uuid,
    task_id: String,
    state: String,
    attempts: Value,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
}

async fn run_rows(pool: &sqlx::PgPool, execution: Uuid) -> Vec<RunRow> {
    sqlx::query_as(
        "SELECT id, task_id, state, attempts, started_at, ended_at \
           FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(execution)
    .fetch_all(pool)
    .await
    .expect("runs")
}

async fn exec_graph(
    app: &common::App,
    ws: Uuid,
    id: Uuid,
    query: &str,
    user: UserId,
) -> axum::response::Response {
    common::get(
        app,
        &format!("/api/ws/{ws}/workflows/executions/{id}/graph?{query}"),
        user,
    )
    .await
}

async fn exec_graph_ok(app: &common::App, ws: Uuid, id: Uuid, query: &str, user: UserId) -> Value {
    let res = exec_graph(app, ws, id, query, user).await;
    assert_eq!(res.status(), StatusCode::OK);
    common::json(res).await
}

fn ts(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().map(|s| s.parse().expect("an RFC 3339 instant"))
}

fn state_keys(g: &Value) -> Vec<String> {
    g["states"].as_object().unwrap().keys().cloned().collect()
}

/// Every run's state, attempt count, id and timestamps against its row.
fn assert_matches_row(g: &Value, uid: &str, row: &RunRow) {
    let s = &g["states"][uid];
    assert_eq!(s["state"], row.state, "{uid}: {s}");
    assert_eq!(
        s["attempts"],
        row.attempts.as_array().unwrap().len(),
        "{uid}: {s}"
    );
    assert_eq!(s["run_id"], row.id.to_string(), "{uid}: {s}");
    assert_eq!(ts(&s["started_at"]), row.started_at, "{uid}: {s}");
    assert_eq!(ts(&s["ended_at"]), row.ended_at, "{uid}: {s}");
    assert_eq!(s["iterations"], Value::Null, "{uid}: {s}");
}

// ---- 1. A finished execution ---------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn each_run_of_a_finished_execution_sits_on_its_node(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    let source = format!(
        "id: f\nnamespace: t\ntasks:\n  - {{id: s, type: {FLOW}.Sequential, tasks: [{}, {}]}}\n  - {}\n",
        leaf("a"),
        leaf("b"),
        leaf("c"),
    );
    let ex = start(&pool, ws, None, &source).await;
    drain(&pool).await;
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, "", user).await;

    assert_eq!(g["graph"], expected(&source));
    assert_eq!(
        state_keys(&g),
        ["tasks.c", "tasks.s", "tasks.s.a", "tasks.s.b"]
    );
    let rows = run_rows(&pool, ex).await;
    assert_eq!(rows.len(), 4, "{rows:?}");
    for row in &rows {
        let uid = match row.task_id.as_str() {
            "s" => "tasks.s",
            "a" => "tasks.s.a",
            "b" => "tasks.s.b",
            "c" => "tasks.c",
            other => panic!("unexpected run {other}"),
        };
        assert!(row.ended_at.is_some(), "a finished run has an end: {row:?}");
        assert_matches_row(&g, uid, row);
    }
}

// ---- 2. A retried task ---------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_retried_task_has_two_attempts(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    let source = format!(
        "id: r\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: once\n    \
         retry:\n      type: constant\n      interval: PT1S\n      maxAttempts: 2\n"
    );
    let ex = start(&pool, ws, None, &source).await;
    drain(&pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    worker::wake(&pool, 16).await.expect("wake");
    drain(&pool).await;
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, "", user).await;

    assert_eq!(g["states"]["tasks.a"]["attempts"], 2, "{}", g["states"]);
    assert_eq!(g["states"]["tasks.a"]["state"], "SUCCESS");
    assert_matches_row(&g, "tasks.a", &run_rows(&pool, ex).await[0]);
}

// ---- 3. A waiting Pause --------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_waiting_pause_is_paused(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    let source = format!(
        "id: p\nnamespace: t\ntasks:\n  - id: wait\n    type: {FLOW}.Pause\n  - {}\n",
        leaf("after")
    );
    let ex = start(&pool, ws, None, &source).await;
    drain(&pool).await;
    assert_eq!(execution_state(&pool, ex).await, "PAUSED");
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, "", user).await;

    assert_eq!(g["states"]["tasks.wait"]["state"], "PAUSED");
    assert_eq!(g["states"]["tasks.wait"]["ended_at"], Value::Null);
    assert_matches_row(&g, "tasks.wait", &run_rows(&pool, ex).await[0]);
    assert_eq!(state_keys(&g), ["tasks.wait"], "`after` has not run yet");
}

// ---- 4. A task that never ran --------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_task_that_never_ran_has_no_entry(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    let source = format!(
        "id: i\nnamespace: t\ntasks:\n  - id: choose\n    type: {FLOW}.If\n    condition: \"{{{{ true }}}}\"\n    then: [{}]\n    else: [{}]\n",
        leaf("yes"),
        leaf("no")
    );
    let ex = start(&pool, ws, None, &source).await;
    drain(&pool).await;
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, "", user).await;

    node(&g["graph"], "tasks.choose.no");
    assert_eq!(state_keys(&g), ["tasks.choose", "tasks.choose.yes"]);
}

// ---- 5. A Loop with one failing iteration --------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_body_folds_over_its_iterations(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    // `boom` is the MIDDLE value: a "last run wins" fold would read `ok2`.
    // `concurrencyLimit: 0` starts every iteration at once (the default, 1,
    // stops at the first failure and never runs `ok2`).
    let source = format!(
        "id: l\nnamespace: t\ntasks:\n  - id: each\n    type: {FLOW}.Loop\n    \
         concurrencyLimit: 0\n    values: '[\"ok1\", \"boom\", \"ok2\"]'\n    tasks:\n      - {}\n      \
         - {{id: b, type: {LOG}, message: \"{{{{ item.value }}}}\"}}\n",
        leaf("a")
    );
    let ex = start(&pool, ws, None, &source).await;
    drain(&pool).await;
    assert_eq!(execution_state(&pool, ex).await, "FAILED");
    let iterations: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL ORDER BY loop_index",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    let states: Vec<&str> = iterations.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(states, ["SUCCESS", "FAILED", "SUCCESS"], "the fixture");
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, "", user).await;

    // Each node's own run in each iteration (fix round 1), by `loop_index`.
    let own_runs = |task: &'static str| {
        let pool = pool.clone();
        let iterations = iterations.clone();
        async move {
            let mut out = Vec::new();
            for (ix, (it, _)) in iterations.iter().enumerate() {
                let r = run_rows(&pool, *it)
                    .await
                    .into_iter()
                    .find(|r| r.task_id == task)
                    .expect("the task ran in every iteration");
                out.push(json!({
                    "loop_index": ix,
                    "state": r.state,
                    "child_execution_id": it.to_string(),
                    "run_id": r.id.to_string(),
                }));
            }
            out
        }
    };
    let b_runs_json = own_runs("b").await;
    assert_eq!(
        g["states"]["tasks.each.b"],
        json!({
            "state": "FAILED",
            "attempts": 1,
            "run_id": null,
            "started_at": g["states"]["tasks.each.b"]["started_at"],
            "ended_at": g["states"]["tasks.each.b"]["ended_at"],
            "iterations": {
                "total": 3,
                "by_state": { "SUCCESS": 2, "FAILED": 1 },
                "runs": b_runs_json,
            },
        })
    );
    // `a` succeeded in every iteration, the failed one (#1) included: its own
    // state there is SUCCESS, not the iteration's FAILED.
    let a_runs_json = own_runs("a").await;
    assert_eq!(
        g["states"]["tasks.each.a"]["iterations"],
        json!({ "total": 3, "by_state": { "SUCCESS": 3 }, "runs": a_runs_json })
    );
    assert_eq!(
        g["states"]["tasks.each.a"]["iterations"]["runs"][1]["state"],
        "SUCCESS"
    );
    assert_eq!(iterations[1].1, "FAILED", "the iteration itself failed");
    assert_eq!(g["states"]["tasks.each.a"]["state"], "SUCCESS");
    assert_eq!(g["states"]["tasks.each.a"]["run_id"], Value::Null);
    // The span covers every iteration's run of `b`.
    let mut b_runs = Vec::new();
    for (it, _) in &iterations {
        b_runs.extend(
            run_rows(&pool, *it)
                .await
                .into_iter()
                .filter(|r| r.task_id == "b"),
        );
    }
    assert_eq!(b_runs.len(), 3);
    let b = &g["states"]["tasks.each.b"];
    assert_eq!(
        ts(&b["started_at"]),
        b_runs.iter().filter_map(|r| r.started_at).min()
    );
    assert_eq!(
        ts(&b["ended_at"]),
        b_runs.iter().filter_map(|r| r.ended_at).max()
    );
    // The Loop itself is one run in the execution.
    let each = run_rows(&pool, ex)
        .await
        .into_iter()
        .find(|r| r.task_id == "each")
        .unwrap();
    assert_matches_row(&g, "tasks.each", &each);
    assert_eq!(each.state, "FAILED");
}

// ---- 7. Expanding a Subflow on an execution ------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn an_expanded_subflow_draws_the_child_executions_frozen_source_and_states(
    pool: sqlx::PgPool,
) {
    let (db, ws, user) = seeded(&pool).await;
    let child_ran = flow_yaml("t", "child", &[leaf("x")]);
    let child = common::seed_flow(&db, ws, user, &child_ran, "draft").await;
    let parent = flow_yaml("t", "parent", &[subflow("call", "t", "child")]);
    let ex = start(&pool, ws, None, &parent).await;
    drain(&pool).await;
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    let child_ex: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    // The child's draft moves on after the run.
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(child)
        .bind(flow_yaml("t", "child", &[leaf("x"), leaf("later")]))
        .execute(&pool)
        .await
        .unwrap();
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, &subflows_query(&["tasks.call"]), user).await;

    let got = uids(&g["graph"]);
    assert!(got.contains(&"tasks.call/tasks.x".to_string()), "{got:?}");
    assert!(
        !got.iter().any(|u| u.starts_with("tasks.call/tasks.later")),
        "the child EXECUTION's frozen source, not the draft: {got:?}"
    );
    assert_eq!(
        cluster(&g["graph"], "cluster:tasks.call")["reason"],
        Value::Null
    );
    assert_eq!(state_keys(&g), ["tasks.call", "tasks.call/tasks.x"]);
    let x = run_rows(&pool, child_ex).await.into_iter().next().unwrap();
    assert_matches_row(&g, "tasks.call/tasks.x", &x);
    let call = run_rows(&pool, ex).await.into_iter().next().unwrap();
    assert_matches_row(&g, "tasks.call", &call);

    // The Subflow's run names the child execution it started (fix round 1);
    // the child's own leaf, and any other node, carries no such field.
    assert_eq!(
        g["states"]["tasks.call"]["child_execution_id"],
        child_ex.to_string()
    );
    assert!(g["states"]["tasks.call/tasks.x"]
        .get("child_execution_id")
        .is_none());

    // Unexpanded, the same execution carries only its own states, the link included.
    let g = exec_graph_ok(&app, ws, ex, "", user).await;
    assert_eq!(state_keys(&g), ["tasks.call"]);
    assert_eq!(
        g["states"]["tasks.call"]["child_execution_id"],
        child_ex.to_string()
    );
}

// ---- 8. The execution's frozen source, not the draft ---------------------

#[sqlx::test(migrations = "../../migrations")]
async fn the_execution_graph_is_drawn_from_the_frozen_source(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    let ran = flow_yaml("t", "f", &[leaf("a"), leaf("b")]);
    let flow = common::seed_flow(&db, ws, user, &ran, "draft").await;
    let ex = start(&pool, ws, Some(flow), &ran).await;
    drain(&pool).await;
    let edited = flow_yaml("t", "f", &[leaf("z")]);
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(&edited)
        .execute(&pool)
        .await
        .unwrap();
    let app = common::app(&db).await;

    let g = exec_graph_ok(&app, ws, ex, "", user).await;

    assert_eq!(g["graph"], expected(&ran));
    assert_ne!(g["graph"], expected(&edited));
    assert_eq!(state_keys(&g), ["tasks.a", "tasks.b"]);
}

// ---- 9. Another workspace's execution, and permissions -------------------

#[sqlx::test(migrations = "../../migrations")]
async fn another_workspaces_execution_is_404(pool: sqlx::PgPool) {
    let (db, ws, user) = seeded(&pool).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let source = flow_yaml("t", "f", &[leaf("a")]);
    let foreign = start(&pool, other_ws, None, &source).await;
    let mine = start(&pool, ws, None, &source).await;
    let app = common::app(&db).await;

    let res = exec_graph(&app, ws, foreign, "", user).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = exec_graph(&app, ws, Uuid::new_v4(), "", user).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    // Positive control: the same request for my own execution is served.
    let res = exec_graph(&app, ws, mine, "", user).await;
    assert_eq!(res.status(), StatusCode::OK);

    // `workflow:read` is required; `:manage` alone is not it.
    let (_, stranger) = common::seed_workspace(&db).await;
    common::grant(&db, ws, stranger, &["workflow:manage"]).await;
    let res = exec_graph(&app, ws, mine, "", stranger).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
