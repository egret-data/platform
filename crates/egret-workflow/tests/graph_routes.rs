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
use egret_workflow::graph::{flow_graph, GraphOptions, MAX_NODES};
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
    serde_json::to_value(flow_graph(&flow, &GraphOptions::default()).expect("graph")).unwrap()
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
