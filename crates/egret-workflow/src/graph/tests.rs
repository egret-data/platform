//! `flow_graph`, one test per construct of spec §3.2's table and per shape
//! the W7c plan names (Task 1 Step 2). Each pins the exact node uids, the
//! exact edge set and the cluster.

use std::collections::BTreeSet;

use super::*;
use crate::spec::parse;

const LOG: &str = "io.kestra.plugin.core.log.Log";
const FLOW: &str = "io.kestra.plugin.core.flow";

type E = (String, String, Relation, Option<String>);

fn leaf(id: &str) -> String {
    format!("{{id: \"{id}\", type: {LOG}, message: m}}")
}

fn graph(body: &str) -> FlowGraph {
    let yaml = format!("id: f\nnamespace: company.team\n{body}");
    let flow = parse(&yaml).unwrap_or_else(|e| panic!("parse: {e:?}\n{yaml}"));
    flow_graph(&flow, &GraphOptions::default()).expect("graph")
}

fn uids(g: &FlowGraph) -> Vec<&str> {
    g.nodes.iter().map(|n| n.uid.as_str()).collect()
}

fn edges(g: &FlowGraph) -> BTreeSet<E> {
    let set: BTreeSet<E> = g
        .edges
        .iter()
        .map(|e| {
            (
                e.source.clone(),
                e.target.clone(),
                e.relation,
                e.label.clone(),
            )
        })
        .collect();
    assert_eq!(set.len(), g.edges.len(), "duplicate edges: {:?}", g.edges);
    set
}

fn e(source: &str, target: &str, relation: Relation, label: Option<&str>) -> E {
    (
        source.to_string(),
        target.to_string(),
        relation,
        label.map(str::to_string),
    )
}

fn set(v: Vec<E>) -> BTreeSet<E> {
    v.into_iter().collect()
}

fn cluster<'g>(g: &'g FlowGraph, uid: &str) -> &'g Cluster {
    g.clusters
        .iter()
        .find(|c| c.uid == uid)
        .unwrap_or_else(|| panic!("no cluster {uid}: {:?}", g.clusters))
}

fn assert_cluster(
    g: &FlowGraph,
    uid: &str,
    kind: ClusterKind,
    nodes: &[&str],
    parent: Option<&str>,
    start: &str,
    end: &str,
) {
    let c = cluster(g, uid);
    assert_eq!(c.kind, kind, "{uid}");
    assert_eq!(c.nodes, nodes, "{uid}");
    assert_eq!(c.parent.as_deref(), parent, "{uid}");
    assert_eq!(c.start, start, "{uid}");
    assert_eq!(c.end, end, "{uid}");
    assert_eq!(c.reason, None, "{uid}");
}

/// The task node's fields, by uid.
struct T<'g> {
    task_id: &'g str,
    canonical_type: &'g str,
    kind: TaskNodeKind,
    branch: Branch,
    disabled: bool,
    refusal: Option<&'g str>,
}

fn task<'g>(g: &'g FlowGraph, uid: &str) -> T<'g> {
    let node = g
        .nodes
        .iter()
        .find(|n| n.uid == uid)
        .unwrap_or_else(|| panic!("no node {uid}"));
    match &node.kind {
        NodeKind::Task {
            task_id,
            canonical_type,
            kind,
            branch,
            disabled,
            refusal,
            ..
        } => T {
            task_id,
            canonical_type,
            kind: *kind,
            branch: *branch,
            disabled: *disabled,
            refusal: refusal.as_deref(),
        },
        other => panic!("{uid} is not a task: {other:?}"),
    }
}

use Relation::{AfterExecution, Choice, Dynamic, Error, Finally, Parallel, Sequential};

// ---- task lists ---------------------------------------------------------

/// The shared shape of every task-list construct: `start:X -> X -> a -> b ->
/// end:X`, all `sequential`, inside one cluster of `kind`.
fn assert_task_list(r#type: &str, extra: &str, kind: ClusterKind) {
    let g = graph(&format!(
        "tasks:\n  - {{id: s, type: {type}, {extra} tasks: [{a}, {b}]}}\n",
        type = r#type,
        a = leaf("a"),
        b = leaf("b"),
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.s",
            "tasks.s",
            "tasks.s.a",
            "tasks.s.b",
            "end:tasks.s",
            "end:flow"
        ],
        "{type}"
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.s", Sequential, None),
            e("start:tasks.s", "tasks.s", Sequential, None),
            e("tasks.s", "tasks.s.a", Sequential, None),
            e("tasks.s.a", "tasks.s.b", Sequential, None),
            e("tasks.s.b", "end:tasks.s", Sequential, None),
            e("end:tasks.s", "end:flow", Sequential, None),
        ]),
        "{type}"
    );
    assert_eq!(g.clusters.len(), 1, "no triggers -> no triggers cluster");
    assert_cluster(
        &g,
        "cluster:tasks.s",
        kind,
        &[
            "start:tasks.s",
            "tasks.s",
            "tasks.s.a",
            "tasks.s.b",
            "end:tasks.s",
        ],
        None,
        "start:tasks.s",
        "end:tasks.s",
    );
    let s = task(&g, "tasks.s");
    assert_eq!(s.kind, TaskNodeKind::Flowable);
    assert_eq!(s.task_id, "s");
    let a = task(&g, "tasks.s.a");
    assert_eq!(a.kind, TaskNodeKind::Leaf);
    assert_eq!(a.branch, Branch::Tasks);
}

#[test]
fn a_sequential_is_a_sequential_chain_in_its_cluster() {
    assert_task_list(&format!("{FLOW}.Sequential"), "", ClusterKind::Sequential);
}

#[test]
fn an_allow_failure_is_a_sequential_chain_in_its_cluster() {
    assert_task_list(
        &format!("{FLOW}.AllowFailure"),
        "",
        ClusterKind::AllowFailure,
    );
}

#[test]
fn a_working_directory_is_a_sequential_chain_in_its_cluster() {
    assert_task_list(
        &format!("{FLOW}.WorkingDirectory"),
        "",
        ClusterKind::WorkingDirectory,
    );
}

#[test]
fn a_loop_until_is_a_sequential_chain_in_its_cluster() {
    assert_task_list(
        &format!("{FLOW}.LoopUntil"),
        "condition: \"{{ true }}\",",
        ClusterKind::LoopUntil,
    );
}

#[test]
fn a_pause_with_tasks_is_a_sequential_chain_in_its_cluster() {
    assert_task_list(&format!("{FLOW}.Pause"), "", ClusterKind::Pause);
}

#[test]
fn a_pause_without_tasks_is_a_leaf() {
    let g = graph(&format!(
        "tasks:\n  - {{id: p, type: {FLOW}.Pause, pauseDuration: PT1S}}\n"
    ));
    assert_eq!(uids(&g), ["start:flow", "tasks.p", "end:flow"]);
    assert!(g.clusters.is_empty());
    assert_eq!(task(&g, "tasks.p").kind, TaskNodeKind::Leaf);
}

// ---- parallel, if, switch, dag, loop ------------------------------------

#[test]
fn a_parallel_fans_out_and_in() {
    let g = graph(&format!(
        "tasks:\n  - {{id: p, type: {FLOW}.Parallel, tasks: [{}, {}]}}\n",
        leaf("a"),
        leaf("b")
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.p",
            "tasks.p",
            "tasks.p.a",
            "tasks.p.b",
            "end:tasks.p",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.p", Sequential, None),
            e("start:tasks.p", "tasks.p", Sequential, None),
            e("tasks.p", "tasks.p.a", Parallel, None),
            e("tasks.p", "tasks.p.b", Parallel, None),
            e("tasks.p.a", "end:tasks.p", Parallel, None),
            e("tasks.p.b", "end:tasks.p", Parallel, None),
            e("end:tasks.p", "end:flow", Sequential, None),
        ])
    );
    assert_cluster(
        &g,
        "cluster:tasks.p",
        ClusterKind::Parallel,
        &[
            "start:tasks.p",
            "tasks.p",
            "tasks.p.a",
            "tasks.p.b",
            "end:tasks.p",
        ],
        None,
        "start:tasks.p",
        "end:tasks.p",
    );
}

#[test]
fn an_if_draws_then_and_else_as_labelled_choices() {
    let g = graph(&format!(
        "tasks:\n  - {{id: i, type: {FLOW}.If, condition: \"{{{{ true }}}}\", then: [{}, {}], else: [{}]}}\n",
        leaf("a"),
        leaf("b"),
        leaf("c")
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.i",
            "tasks.i",
            "tasks.i.a",
            "tasks.i.b",
            "tasks.i.c",
            "end:tasks.i",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.i", Sequential, None),
            e("start:tasks.i", "tasks.i", Sequential, None),
            e("tasks.i", "tasks.i.a", Choice, Some("then")),
            e("tasks.i.a", "tasks.i.b", Sequential, Some("then")),
            e("tasks.i.b", "end:tasks.i", Sequential, Some("then")),
            e("tasks.i", "tasks.i.c", Choice, Some("else")),
            e("tasks.i.c", "end:tasks.i", Sequential, Some("else")),
            e("end:tasks.i", "end:flow", Sequential, None),
        ])
    );
    assert_cluster(
        &g,
        "cluster:tasks.i",
        ClusterKind::If,
        &[
            "start:tasks.i",
            "tasks.i",
            "tasks.i.a",
            "tasks.i.b",
            "tasks.i.c",
            "end:tasks.i",
        ],
        None,
        "start:tasks.i",
        "end:tasks.i",
    );
}

#[test]
fn an_if_with_an_empty_else_draws_no_else_edge() {
    let g = graph(&format!(
        "tasks:\n  - {{id: i, type: {FLOW}.If, condition: \"{{{{ true }}}}\", then: [{}], else: []}}\n",
        leaf("a")
    ));
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.i", Sequential, None),
            e("start:tasks.i", "tasks.i", Sequential, None),
            e("tasks.i", "tasks.i.a", Choice, Some("then")),
            e("tasks.i.a", "end:tasks.i", Sequential, Some("then")),
            e("end:tasks.i", "end:flow", Sequential, None),
        ])
    );
}

#[test]
fn a_switch_labels_each_case_and_its_defaults() {
    let g = graph(&format!(
        "tasks:\n  - {{id: w, type: {FLOW}.Switch, value: x, cases: {{y: [{}, {}], x: [{}]}}, defaults: [{}]}}\n",
        leaf("b"),
        leaf("c"),
        leaf("a"),
        leaf("d")
    ));
    // Cases in key order, then `defaults`.
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.w",
            "tasks.w",
            "tasks.w.a",
            "tasks.w.b",
            "tasks.w.c",
            "tasks.w.d",
            "end:tasks.w",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.w", Sequential, None),
            e("start:tasks.w", "tasks.w", Sequential, None),
            e("tasks.w", "tasks.w.a", Choice, Some("x")),
            e("tasks.w.a", "end:tasks.w", Sequential, Some("x")),
            e("tasks.w", "tasks.w.b", Choice, Some("y")),
            e("tasks.w.b", "tasks.w.c", Sequential, Some("y")),
            e("tasks.w.c", "end:tasks.w", Sequential, Some("y")),
            e("tasks.w", "tasks.w.d", Choice, Some("defaults")),
            e("tasks.w.d", "end:tasks.w", Sequential, Some("defaults")),
            e("end:tasks.w", "end:flow", Sequential, None),
        ])
    );
    assert_cluster(
        &g,
        "cluster:tasks.w",
        ClusterKind::Switch,
        &[
            "start:tasks.w",
            "tasks.w",
            "tasks.w.a",
            "tasks.w.b",
            "tasks.w.c",
            "tasks.w.d",
            "end:tasks.w",
        ],
        None,
        "start:tasks.w",
        "end:tasks.w",
    );
}

#[test]
fn a_dag_diamond_draws_one_edge_per_dependency() {
    let g = graph(&format!(
        "tasks:\n  - id: g\n    type: {FLOW}.Dag\n    tasks:\n      - task: {}\n      - {{task: {}, dependsOn: [a]}}\n      - {{task: {}, dependsOn: [a]}}\n      - {{task: {}, dependsOn: [b, c]}}\n",
        leaf("a"),
        leaf("b"),
        leaf("c"),
        leaf("d")
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.g",
            "tasks.g",
            "tasks.g.a",
            "tasks.g.b",
            "tasks.g.c",
            "tasks.g.d",
            "end:tasks.g",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.g", Sequential, None),
            e("start:tasks.g", "tasks.g", Sequential, None),
            e("tasks.g", "tasks.g.a", Dynamic, None),
            e("tasks.g.a", "tasks.g.b", Dynamic, None),
            e("tasks.g.a", "tasks.g.c", Dynamic, None),
            e("tasks.g.b", "tasks.g.d", Dynamic, None),
            e("tasks.g.c", "tasks.g.d", Dynamic, None),
            e("tasks.g.d", "end:tasks.g", Dynamic, None),
            e("end:tasks.g", "end:flow", Sequential, None),
        ])
    );
    assert_cluster(
        &g,
        "cluster:tasks.g",
        ClusterKind::Dag,
        &[
            "start:tasks.g",
            "tasks.g",
            "tasks.g.a",
            "tasks.g.b",
            "tasks.g.c",
            "tasks.g.d",
            "end:tasks.g",
        ],
        None,
        "start:tasks.g",
        "end:tasks.g",
    );
}

#[test]
fn a_dag_node_without_a_task_or_a_dangling_dependency_draws_nothing() {
    let g = graph(&format!(
        "tasks:\n  - id: g\n    type: {FLOW}.Dag\n    tasks:\n      - task: {}\n      - {{dependsOn: [a]}}\n      - {{task: {}, dependsOn: [nope]}}\n",
        leaf("a"),
        leaf("b"),
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.g",
            "tasks.g",
            "tasks.g.a",
            "tasks.g.b",
            "end:tasks.g",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.g", Sequential, None),
            e("start:tasks.g", "tasks.g", Sequential, None),
            e("tasks.g", "tasks.g.a", Dynamic, None),
            // Only the task-less node depends on `a`, and it is not drawn, so
            // nothing drawn depends on `a`.
            e("tasks.g.a", "end:tasks.g", Dynamic, None),
            e("tasks.g.b", "end:tasks.g", Dynamic, None),
            e("end:tasks.g", "end:flow", Sequential, None),
        ])
    );
}

#[test]
fn a_loop_is_dynamic_from_start_through_the_body_to_end() {
    let g = graph(&format!(
        "tasks:\n  - {{id: l, type: {FLOW}.Loop, values: [1, 2], tasks: [{}, {}]}}\n",
        leaf("a"),
        leaf("b")
    ));
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.l", Sequential, None),
            e("start:tasks.l", "tasks.l", Sequential, None),
            e("tasks.l", "tasks.l.a", Dynamic, None),
            e("tasks.l.a", "tasks.l.b", Dynamic, None),
            e("tasks.l.b", "end:tasks.l", Dynamic, None),
            e("end:tasks.l", "end:flow", Sequential, None),
        ])
    );
    assert_cluster(
        &g,
        "cluster:tasks.l",
        ClusterKind::Loop,
        &[
            "start:tasks.l",
            "tasks.l",
            "tasks.l.a",
            "tasks.l.b",
            "end:tasks.l",
        ],
        None,
        "start:tasks.l",
        "end:tasks.l",
    );
}

#[test]
fn a_subflow_is_an_executable_leaf() {
    let g = graph(&format!(
        "tasks:\n  - {{id: c, type: {FLOW}.Subflow, namespace: company.team, flowId: child}}\n"
    ));
    assert_eq!(uids(&g), ["start:flow", "tasks.c", "end:flow"]);
    assert!(g.clusters.is_empty());
    let c = task(&g, "tasks.c");
    assert_eq!(c.kind, TaskNodeKind::Executable);
    assert_eq!(c.canonical_type, "io.kestra.plugin.core.flow.Subflow");
}

// ---- errors, finally, afterExecution ------------------------------------

#[test]
fn a_parallel_in_a_sequentials_errors_hangs_off_the_owners_start() {
    let g = graph(&format!(
        "tasks:\n  - {{id: s, type: {FLOW}.Sequential, tasks: [{}], errors: [{{id: p, type: {FLOW}.Parallel, tasks: [{}, {}]}}]}}\n",
        leaf("a"),
        leaf("x"),
        leaf("y")
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:tasks.s",
            "tasks.s",
            "tasks.s.a",
            "start:tasks.s.p",
            "tasks.s.p",
            "tasks.s.p.x",
            "tasks.s.p.y",
            "end:tasks.s.p",
            "end:tasks.s",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "start:tasks.s", Sequential, None),
            e("start:tasks.s", "tasks.s", Sequential, None),
            e("tasks.s", "tasks.s.a", Sequential, None),
            e("tasks.s.a", "end:tasks.s", Sequential, None),
            e("start:tasks.s", "start:tasks.s.p", Error, None),
            e("start:tasks.s.p", "tasks.s.p", Sequential, None),
            e("tasks.s.p", "tasks.s.p.x", Parallel, None),
            e("tasks.s.p", "tasks.s.p.y", Parallel, None),
            e("tasks.s.p.x", "end:tasks.s.p", Parallel, None),
            e("tasks.s.p.y", "end:tasks.s.p", Parallel, None),
            e("end:tasks.s.p", "end:tasks.s", Error, None),
            e("end:tasks.s", "end:flow", Sequential, None),
        ])
    );
    assert_cluster(
        &g,
        "cluster:tasks.s",
        ClusterKind::Sequential,
        &["start:tasks.s", "tasks.s", "tasks.s.a", "end:tasks.s"],
        None,
        "start:tasks.s",
        "end:tasks.s",
    );
    assert_cluster(
        &g,
        "cluster:tasks.s.p",
        ClusterKind::Parallel,
        &[
            "start:tasks.s.p",
            "tasks.s.p",
            "tasks.s.p.x",
            "tasks.s.p.y",
            "end:tasks.s.p",
        ],
        Some("cluster:tasks.s"),
        "start:tasks.s.p",
        "end:tasks.s.p",
    );
    assert_eq!(task(&g, "tasks.s").branch, Branch::Tasks);
    assert_eq!(task(&g, "tasks.s.a").branch, Branch::Tasks);
    // Descendants of an `errors:` task inherit its branch.
    for uid in ["tasks.s.p", "tasks.s.p.x", "tasks.s.p.y"] {
        assert_eq!(task(&g, uid).branch, Branch::Errors, "{uid}");
    }
}

#[test]
fn the_flows_errors_finally_and_after_execution() {
    let g = graph(&format!(
        "tasks: [{}]\nerrors: [{}, {}]\nfinally: [{}]\nafterExecution: [{}]\n",
        leaf("a"),
        leaf("e1"),
        leaf("e2"),
        leaf("f"),
        leaf("x")
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "tasks.a",
            "errors.e1",
            "errors.e2",
            "finally.f",
            "afterExecution.x",
            "end:flow"
        ]
    );
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "tasks.a", Sequential, None),
            e("tasks.a", "finally.f", Finally, None),
            e("start:flow", "errors.e1", Error, None),
            e("errors.e1", "errors.e2", Error, None),
            e("errors.e2", "finally.f", Error, None),
            e("finally.f", "afterExecution.x", AfterExecution, None),
            e("afterExecution.x", "end:flow", AfterExecution, None),
        ])
    );
    assert!(g.clusters.is_empty());
    assert_eq!(task(&g, "tasks.a").branch, Branch::Tasks);
    assert_eq!(task(&g, "errors.e1").branch, Branch::Errors);
    assert_eq!(task(&g, "finally.f").branch, Branch::Finally);
    assert_eq!(task(&g, "afterExecution.x").branch, Branch::AfterExecution);
}

#[test]
fn without_finally_the_lists_lead_to_after_execution_then_to_the_end() {
    let g = graph(&format!(
        "tasks: [{}]\nerrors: [{}]\nafterExecution: [{}]\n",
        leaf("a"),
        leaf("e"),
        leaf("x")
    ));
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "tasks.a", Sequential, None),
            e("tasks.a", "afterExecution.x", AfterExecution, None),
            e("start:flow", "errors.e", Error, None),
            e("errors.e", "afterExecution.x", Error, None),
            e("afterExecution.x", "end:flow", AfterExecution, None),
        ])
    );

    let g = graph(&format!(
        "tasks: [{}]\nerrors: [{}]\nfinally: [{}]\n",
        leaf("a"),
        leaf("e"),
        leaf("f")
    ));
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "tasks.a", Sequential, None),
            e("tasks.a", "finally.f", Finally, None),
            e("start:flow", "errors.e", Error, None),
            e("errors.e", "finally.f", Error, None),
            e("finally.f", "end:flow", Finally, None),
        ])
    );

    let g = graph(&format!(
        "tasks: [{}]\nerrors: [{}]\n",
        leaf("a"),
        leaf("e")
    ));
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "tasks.a", Sequential, None),
            e("tasks.a", "end:flow", Sequential, None),
            e("start:flow", "errors.e", Error, None),
            e("errors.e", "end:flow", Error, None),
        ])
    );
}

// ---- triggers -----------------------------------------------------------

#[test]
fn triggers_of_each_kind_form_one_cluster_chained_into_the_flows_start() {
    let g = graph(&format!(
        "tasks: [{}]\ntriggers:\n  - {{id: s, type: io.kestra.plugin.core.trigger.Schedule, cron: \"0 * * * *\"}}\n  - {{id: w, type: io.kestra.plugin.core.trigger.Webhook, key: k, disabled: true}}\n  - {{id: fl, type: io.kestra.plugin.core.trigger.Flow, dependsOn: [{{namespace: company.team, flowId: other, states: [SUCCESS]}}]}}\n  - {{id: o, type: io.kestra.plugin.other.Trigger}}\n",
        leaf("a")
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "start:triggers",
            "triggers.s",
            "triggers.w",
            "triggers.fl",
            "triggers.o",
            "end:triggers",
            "tasks.a",
            "end:flow"
        ]
    );
    let mut want = vec![
        e("end:triggers", "start:flow", Sequential, None),
        e("start:flow", "tasks.a", Sequential, None),
        e("tasks.a", "end:flow", Sequential, None),
    ];
    for t in ["triggers.s", "triggers.w", "triggers.fl", "triggers.o"] {
        want.push(e("start:triggers", t, Sequential, None));
        want.push(e(t, "end:triggers", Sequential, None));
    }
    assert_eq!(edges(&g), set(want));
    assert_cluster(
        &g,
        "cluster:triggers",
        ClusterKind::Triggers,
        &[
            "start:triggers",
            "triggers.s",
            "triggers.w",
            "triggers.fl",
            "triggers.o",
            "end:triggers",
        ],
        None,
        "start:triggers",
        "end:triggers",
    );
    assert_eq!(cluster(&g, "cluster:triggers").task_id, None);
    let trig = |uid: &str| match &g.nodes.iter().find(|n| n.uid == uid).unwrap().kind {
        NodeKind::Trigger {
            trigger_id,
            r#type,
            disabled,
        } => (trigger_id.clone(), r#type.clone(), *disabled),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        trig("triggers.w"),
        (
            "w".to_string(),
            "io.kestra.plugin.core.trigger.Webhook".to_string(),
            true
        )
    );
    assert!(!trig("triggers.s").2);
    assert!(!trig("triggers.fl").2);
    assert!(!trig("triggers.o").2);
}

// ---- disabled, refusals --------------------------------------------------

#[test]
fn a_disabled_task_is_drawn_and_keeps_its_edges() {
    let g = graph(&format!(
        "tasks:\n  - {{id: a, type: {LOG}, message: m, disabled: true}}\n  - {}\n",
        leaf("b")
    ));
    assert_eq!(uids(&g), ["start:flow", "tasks.a", "tasks.b", "end:flow"]);
    assert_eq!(
        edges(&g),
        set(vec![
            e("start:flow", "tasks.a", Sequential, None),
            e("tasks.a", "tasks.b", Sequential, None),
            e("tasks.b", "end:flow", Sequential, None),
        ])
    );
    assert!(task(&g, "tasks.a").disabled);
    assert!(!task(&g, "tasks.b").disabled);
}

#[test]
fn refusal_is_the_validation_error_at_the_tasks_own_path() {
    let g = graph(&format!(
        "tasks:\n  - {{id: af, type: {FLOW}.AllowFailure, tasks: [{}]}}\n  - {{id: typo, type: io.kestra.plugin.core.log.Logg, message: m}}\n  - {}\n",
        leaf("inner"),
        leaf("ok")
    ));
    assert_eq!(
        task(&g, "tasks.af").refusal,
        Some("unsupported_allow_failure")
    );
    assert_eq!(task(&g, "tasks.af").kind, TaskNodeKind::Flowable);
    assert_eq!(task(&g, "tasks.typo").refusal, Some("unknown_task_type"));
    assert_eq!(task(&g, "tasks.ok").refusal, None);
    assert_eq!(task(&g, "tasks.af.inner").refusal, None);
}

/// A `Pause` with `tasks:` is refused at `tasks.p.tasks` -- a PROPERTY path,
/// not the task's own -- so under the binding rule (the error AT the task's
/// path) its node carries no refusal. Pinned so a change to either side is
/// seen; see the Task 1 report's concerns.
#[test]
fn a_refusal_at_a_property_path_is_not_the_tasks_refusal() {
    let yaml = format!(
        "id: f\nnamespace: company.team\ntasks:\n  - {{id: p, type: {FLOW}.Pause, tasks: [{}]}}\n",
        leaf("a")
    );
    let flow = parse(&yaml).unwrap();
    let report = validate_for_run(&flow);
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code == "unsupported_pause_property" && f.path == "tasks.p.tasks"),
        "{report:?}"
    );
    let g = flow_graph(&flow, &GraphOptions::default()).unwrap();
    assert_eq!(task(&g, "tasks.p").refusal, None);
}

// ---- uids ---------------------------------------------------------------

#[test]
fn a_task_named_like_a_marker_cannot_collide_with_one() {
    let g = graph(&format!(
        "tasks:\n  - {}\n  - {}\n  - {}\n  - {}\n  - {{id: \"start:tasks.s\", type: {FLOW}.Sequential, tasks: [{}]}}\n",
        leaf("start:flow"),
        leaf("flow"),
        leaf("end:flow"),
        leaf("cluster:flow"),
        leaf("x")
    ));
    let all = uids(&g);
    let distinct: BTreeSet<&str> = all.iter().copied().collect();
    assert_eq!(distinct.len(), all.len(), "{all:?}");
    for uid in [
        "tasks.start:flow",
        "tasks.flow",
        "tasks.end:flow",
        "tasks.cluster:flow",
        "tasks.start:tasks.s",
        "start:tasks.start:tasks.s",
    ] {
        assert!(all.contains(&uid), "{uid} in {all:?}");
    }
    assert_eq!(all[0], "start:flow");
    assert_eq!(*all.last().unwrap(), "end:flow");
    assert_eq!(g.clusters[0].uid, "cluster:tasks.start:tasks.s");
}

#[test]
fn duplicate_ids_get_a_deterministic_suffix() {
    let g = graph(&format!(
        "tasks:\n  - {}\n  - {}\n  - {}\n  - {{id: s, type: {FLOW}.Sequential, tasks: [{}]}}\n  - {{id: s, type: {FLOW}.Sequential, tasks: [{{id: x, type: nope.Typo}}]}}\n",
        leaf("a"),
        leaf("a"),
        leaf("a~2"),
        leaf("x"),
    ));
    assert_eq!(
        uids(&g),
        [
            "start:flow",
            "tasks.a",
            "tasks.a~2",
            "tasks.a~2~2",
            "start:tasks.s",
            "tasks.s",
            "tasks.s.x",
            "end:tasks.s",
            "start:tasks.s~2",
            "tasks.s~2",
            "tasks.s~2.x",
            "end:tasks.s~2",
            "end:flow"
        ]
    );
    assert!(g.clusters.iter().any(|c| c.uid == "cluster:tasks.s~2"));
    // `duplicate_task_id` is reported at the FIRST path; a later occurrence
    // looks its refusal up at the same validation path.
    assert_eq!(task(&g, "tasks.a").refusal, Some("duplicate_task_id"));
    assert_eq!(task(&g, "tasks.a~2").refusal, Some("duplicate_task_id"));
    assert_eq!(task(&g, "tasks.a~2~2").task_id, "a~2");
    // The second `x` (a typo) and the first share the path `tasks.s.x`, where
    // validation reports first `duplicate_task_id`... or the typo, whichever
    // came first: both nodes take that one code.
    assert_eq!(
        task(&g, "tasks.s.x").refusal,
        task(&g, "tasks.s~2.x").refusal
    );
    assert!(task(&g, "tasks.s.x").refusal.is_some());
}

// ---- canonical types, serialisation, limit ------------------------------

#[test]
fn canonical_type_folds_every_spelling() {
    let g = graph(&format!(
        "tasks:\n  - {{id: s, type: core.sequential, tasks: [{{id: l, type: core.log, message: m}}]}}\n  - {{id: w, type: {FLOW}.WaitFor, condition: \"{{{{ true }}}}\", tasks: [{}]}}\n  - {{id: u, type: some.plugin.Thing}}\n",
        leaf("a")
    ));
    assert_eq!(
        task(&g, "tasks.s").canonical_type,
        "io.kestra.plugin.core.flow.Sequential"
    );
    assert_eq!(task(&g, "tasks.s.l").canonical_type, LOG);
    assert_eq!(
        task(&g, "tasks.w").canonical_type,
        "io.kestra.plugin.core.flow.LoopUntil"
    );
    assert_eq!(cluster(&g, "cluster:tasks.w").kind, ClusterKind::LoopUntil);
    assert_eq!(task(&g, "tasks.u").canonical_type, "some.plugin.Thing");
}

#[test]
fn a_node_serialises_flat_with_its_kind_tag() {
    let g = graph(&format!("tasks: [{}]\n", leaf("a")));
    let json = serde_json::to_value(&g).unwrap();
    assert_eq!(
        json["nodes"][1],
        serde_json::json!({
            "uid": "tasks.a",
            "node": "task",
            "task_id": "a",
            "type": LOG,
            "canonical_type": LOG,
            "kind": "leaf",
            "branch": "tasks",
            "disabled": false,
            "refusal": null,
        })
    );
    assert_eq!(
        json["nodes"][0],
        serde_json::json!({"uid": "start:flow", "node": "start"})
    );
    assert_eq!(
        json["edges"][0],
        serde_json::json!({"source": "start:flow", "target": "tasks.a", "relation": "sequential", "label": null})
    );
}

#[test]
fn a_graph_over_five_thousand_nodes_is_refused() {
    // Two markers plus N leaves.
    let flow_of = |n: usize| {
        let tasks: Vec<String> = (0..n).map(|i| leaf(&format!("t{i}"))).collect();
        let yaml = format!(
            "id: f\nnamespace: company.team\ntasks: [{}]\n",
            tasks.join(", ")
        );
        parse(&yaml).unwrap()
    };
    let at_limit = flow_graph(&flow_of(MAX_NODES - 2), &GraphOptions::default())
        .expect("exactly the limit is allowed");
    assert_eq!(at_limit.nodes.len(), MAX_NODES);
    assert_eq!(
        flow_graph(&flow_of(MAX_NODES - 1), &GraphOptions::default()),
        Err(GraphError::TooLarge {
            nodes: MAX_NODES + 1
        })
    );
}
