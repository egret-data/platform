//! The dependencies of a namespace (W7d spec §3.4): a flow-to-flow graph
//! built from the workspace's saved DRAFTS -- what every execution runs (W7a
//! ruling). There is no stored index; every request reads and parses.
//!
//! # The rules, each chosen and tested in `tests/namespaces_routes.rs`
//!
//! - **What is read.** Every flow of the workspace that is not deleted, in
//!   one query ([`crate::repo::FlowRepo::dependency_drafts`]), each draft
//!   parsed once with `spec::parse`. An edge into the namespace can come from
//!   any flow, so the whole workspace is the candidate set.
//! - **The limit** ([`MAX_FLOWS`]) is on that read: a workspace with more
//!   flows (not deleted) answers 422 for every namespace, even an empty one
//!   (spec §10: "A workspace with more flows gets the 422").
//! - **`subflow`**: a `Subflow` task anywhere in the tree
//!   ([`crate::spec::path::every_task`]: the four root lists, `children()`
//!   and `error_children()`), whose `namespace` and `flowId` are both
//!   present, neither an expression ([`crate::graph::target_is_dynamic`],
//!   W7c's `dynamic` reason), and name a flow that was read (parsed or not).
//! - **`flow_trigger`**: a `Flow` trigger's `dependsOn` entry naming BOTH
//!   `namespace` and `flowId` of such a flow draws `upstream -> listener`.
//!   An entry missing either draws nothing: `trigger::depends_on::
//!   dependency_matches` fails closed on a missing field, so that entry is
//!   met by no execution. A `Flow` trigger with no `dependsOn` (the
//!   window-free arm) names no upstream -- it hears every execution of the
//!   workspace, filtered by states and `when` -- and draws nothing. Disabled
//!   triggers still draw: the graph is the declared structure, and the
//!   listener's node carries `disabled` when the whole flow is off. The
//!   conditions come from the draft, the same parse `scheduler::sync` builds
//!   `workflow.flow_listeners` from, so the table is not read.
//! - **Dynamic targets**: a `Subflow` whose target is an expression draws no
//!   edge and is counted in its flow's `dynamic_subflows`.
//! - **Nodes**: the flows of the namespace and its children (dot-anchored,
//!   `in_namespace: true`), plus every flow elsewhere with a direct edge to
//!   or from one of them (`in_namespace: false`). Edges between two outside
//!   flows are dropped. Duplicate edges (two `Subflow` tasks to one target)
//!   are one edge.
//! - **`disabled`**: either door that stops a flow -- the `state` column
//!   (`disabled`) or the YAML's `disabled: true`.
//! - **A draft that does not parse** stays a node (`unparsable: true`, its
//!   `disabled` from the state column alone) and an edge target, but draws
//!   no edge of its own, since its tasks and triggers cannot be read. It is
//!   counted in the top-level `unparsable`, over every draft read (the
//!   workspace's): one outside the namespace can hide an edge into it.
//!   (Fix round 1, controller ruling; it was dropped entirely before.)
//! - **Off the async workers**: the handler calls [`build_blocking`], which
//!   runs [`build`] on tokio's blocking pool.

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;

use crate::models::{TaskKind, TriggerKind};

/// The most flows (not deleted) a workspace may have for the dependencies
/// route to answer (W7d spec §3.4, §10).
pub const MAX_FLOWS: usize = 2_000;

/// One flow as the dependencies graph reads it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DraftRow {
    pub id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    pub source: String,
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeKind {
    Subflow,
    FlowTrigger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    pub disabled: bool,
    pub in_namespace: bool,
    pub dynamic_subflows: i64,
    /// The draft does not parse: the node stands, edges INTO it still draw,
    /// but it has none of its own (its tasks and triggers cannot be read).
    pub unparsable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependencies {
    /// In the order of `drafts` (the repository's: namespace, then flow id).
    pub nodes: Vec<Node>,
    /// By source, then target (each in node order), then kind.
    pub edges: Vec<Edge>,
    pub unparsable: i64,
}

/// `namespace` is `ns` or beneath it, dot-anchored (`a.b` holds `a.b.c`,
/// never `a.bc`), as `repo::NAMESPACES_SQL`'s prefix.
fn within(namespace: &str, ns: &str) -> bool {
    namespace == ns
        || namespace
            .strip_prefix(ns)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// The graph of `ns` over `drafts` (every flow of the workspace that is not
/// deleted). Pure: see the module doc for the rules.
pub fn build(ns: &str, drafts: &[DraftRow]) -> Dependencies {
    // (index into `drafts`, parsed flow), parsed ones only.
    let mut parsed = Vec::with_capacity(drafts.len());
    let mut broken = vec![false; drafts.len()];
    for (i, d) in drafts.iter().enumerate() {
        match crate::spec::parse(&d.source) {
            Ok(flow) => parsed.push((i, flow)),
            Err(_) => broken[i] = true,
        }
    }
    // Every flow read is a target, parsed or not: a broken draft is still a
    // flow that exists, so an edge into it draws.
    let index: HashMap<(&str, &str), usize> = drafts
        .iter()
        .enumerate()
        .map(|(i, d)| ((d.namespace.as_str(), d.flow_id.as_str()), i))
        .collect();
    let lookup = |namespace: &Option<String>, flow_id: &Option<String>| match (namespace, flow_id) {
        (Some(n), Some(f)) => index.get(&(n.as_str(), f.as_str())).copied(),
        _ => None,
    };

    let mut dynamic = vec![0i64; drafts.len()];
    // The state column is one door for every flow; the YAML's `disabled:`
    // is the other, readable only on a parsed draft.
    let mut disabled: Vec<bool> = drafts.iter().map(|d| d.state == "disabled").collect();
    let mut edges: BTreeSet<(usize, usize, EdgeKind)> = BTreeSet::new();
    for (i, flow) in &parsed {
        let i = *i;
        disabled[i] |= flow.disabled;
        for (_, task) in crate::spec::path::every_task(flow) {
            let TaskKind::Subflow(sub) = &task.kind else {
                continue;
            };
            if crate::graph::target_is_dynamic(&sub.namespace, &sub.flow_id) {
                dynamic[i] += 1;
            } else if let Some(t) = lookup(&sub.namespace, &sub.flow_id) {
                edges.insert((i, t, EdgeKind::Subflow));
            }
        }
        for trigger in &flow.triggers {
            let TriggerKind::Flow(ft) = &trigger.kind else {
                continue;
            };
            for dep in &ft.depends_on {
                if let Some(u) = lookup(&dep.namespace, &dep.flow_id) {
                    edges.insert((u, i, EdgeKind::FlowTrigger));
                }
            }
        }
    }

    let inside: Vec<bool> = drafts.iter().map(|d| within(&d.namespace, ns)).collect();
    let edges: Vec<_> = edges
        .into_iter()
        .filter(|(s, t, _)| inside[*s] || inside[*t])
        .collect();
    let mut keep = inside.clone();
    for (s, t, _) in &edges {
        keep[*s] = true;
        keep[*t] = true;
    }

    Dependencies {
        nodes: drafts
            .iter()
            .enumerate()
            .filter(|(i, _)| keep[*i])
            .map(|(i, d)| Node {
                id: d.id,
                namespace: d.namespace.clone(),
                flow_id: d.flow_id.clone(),
                disabled: disabled[i],
                in_namespace: inside[i],
                dynamic_subflows: dynamic[i],
                unparsable: broken[i],
            })
            .collect(),
        edges: edges
            .into_iter()
            .map(|(s, t, kind)| Edge {
                source: drafts[s].id,
                target: drafts[t].id,
                kind,
            })
            .collect(),
        unparsable: broken.iter().filter(|b| **b).count() as i64,
    }
}

/// [`build`] off the async workers: it parses up to [`MAX_FLOWS`] drafts
/// (YAML, twice each), seconds of CPU at the limit, which must not pin a
/// runtime worker that also runs the scheduler, the reaper and lease
/// renewal. A join failure is a 500.
pub async fn build_blocking(ns: String, drafts: Vec<DraftRow>) -> egret_core::Result<Dependencies> {
    off_runtime(move || build(&ns, &drafts)).await
}

/// Runs `f` on tokio's blocking pool, never on the calling worker.
async fn off_runtime<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> egret_core::Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|join| {
        egret_core::Error::Internal(anyhow::anyhow!(
            "building namespace dependencies stopped: {join}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_is_dot_anchored() {
        assert!(within("a.b", "a.b"));
        assert!(within("a.b.c", "a.b"));
        assert!(!within("a.bc", "a.b"));
        assert!(!within("a", "a.b"));
    }

    /// The closure `build_blocking` hands over runs on another thread than
    /// the (single-threaded) runtime awaiting it: inline execution would
    /// report the same thread.
    #[tokio::test(flavor = "current_thread")]
    async fn the_build_runs_off_the_awaiting_worker() {
        let here = std::thread::current().id();
        let there = off_runtime(|| std::thread::current().id()).await.unwrap();
        assert_ne!(here, there);
    }

    /// A broken draft stays a node, flagged, with the edge into it.
    #[test]
    fn a_broken_draft_is_a_flagged_target() {
        let row = |ns: &str, id: &str, source: &str| DraftRow {
            id: Uuid::new_v4(),
            namespace: ns.into(),
            flow_id: id.into(),
            source: source.into(),
            state: "draft".into(),
        };
        let drafts = vec![
            row("n", "broken", "tasks: [unclosed"),
            row(
                "n",
                "caller",
                "id: caller\nnamespace: n\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: n\n    flowId: broken\n",
            ),
        ];
        let d = build("n", &drafts);
        assert_eq!(d.unparsable, 1);
        assert_eq!(d.nodes.len(), 2);
        assert!(d.nodes[0].unparsable && !d.nodes[1].unparsable);
        assert_eq!(
            d.edges,
            vec![Edge {
                source: drafts[1].id,
                target: drafts[0].id,
                kind: EdgeKind::Subflow
            }]
        );
    }
}
