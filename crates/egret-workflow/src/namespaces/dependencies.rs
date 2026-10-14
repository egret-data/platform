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
//!   W7c's `dynamic` reason), and name a flow that was read and parsed.
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
//! - **A draft that does not parse** is skipped -- no node, no edge either
//!   way -- and counted in `unparsable`, over every draft read (the
//!   workspace's): one outside the namespace can hide an edge into it.

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
    let mut unparsable = 0;
    // (index into `drafts`, parsed flow), parsed ones only.
    let mut parsed = Vec::with_capacity(drafts.len());
    for (i, d) in drafts.iter().enumerate() {
        match crate::spec::parse(&d.source) {
            Ok(flow) => parsed.push((i, flow)),
            Err(_) => unparsable += 1,
        }
    }
    let index: HashMap<(&str, &str), usize> = parsed
        .iter()
        .map(|(i, _)| {
            (
                (drafts[*i].namespace.as_str(), drafts[*i].flow_id.as_str()),
                *i,
            )
        })
        .collect();
    let lookup = |namespace: &Option<String>, flow_id: &Option<String>| match (namespace, flow_id) {
        (Some(n), Some(f)) => index.get(&(n.as_str(), f.as_str())).copied(),
        _ => None,
    };

    let mut dynamic = vec![0i64; drafts.len()];
    let mut disabled = vec![false; drafts.len()];
    let mut edges: BTreeSet<(usize, usize, EdgeKind)> = BTreeSet::new();
    for (i, flow) in &parsed {
        let i = *i;
        disabled[i] = drafts[i].state == "disabled" || flow.disabled;
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
    let mut keep: Vec<bool> = parsed
        .iter()
        .fold(vec![false; drafts.len()], |mut k, (i, _)| {
            k[*i] = inside[*i];
            k
        });
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
        unparsable,
    }
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
}
