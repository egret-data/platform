//! `Subflow` expansion (spec §3.4, §3.5): the child flow's graph spliced into
//! its parent in place of the `Subflow` node, through a loader seam
//! ([`SubflowSource`]) so the splice stays testable without a database.
//!
//! # Where a child is loaded from
//!
//! [`SubflowSource::current_draft`] answers the child's current draft by
//! `namespace` and `flowId` **in the request's workspace**, and only there:
//! a `Subflow` resolves within its own workspace at run time too
//! (`worker::subflow::resolve`), and `workflow:read` is a workspace
//! permission, so a viewer who may read the parent may read the child (no
//! `forbidden` reason exists). The route supplies [`FlowRepo`]'s
//! implementation; a trashed flow does not resolve.
//!
//! # Reasons
//!
//! An expansion that cannot happen is still a `subflow` cluster, with no
//! nodes and a `reason`, checked in this order:
//!
//! - `dynamic`: `namespace` or `flowId` is an expression (`{{ }}`/`{% %}`,
//!   Kestra's `containsOpeningBlockDelimiter`), so only an execution knows the
//!   target;
//! - `depth`: the `Subflow` sits in a flow that is already
//!   [`MAX_EXPANSION_DEPTH`] expansions deep (`GraphOptions::depth`), so the
//!   request flow's child and grandchild expand and the third level does not;
//! - `not_found`: no such flow in this workspace, or `namespace`/`flowId` is
//!   missing.
//!
//! An empty cluster's `start` and `end` are the `Subflow` node's own uid (no
//! marker is drawn for it) and the node stays where it was.
//!
//! # Uids: `{subflow_uid}/{child_uid}`
//!
//! A child's task and trigger uids are prefixed with the `Subflow` node's uid
//! and a `/` (`tasks.call/tasks.x`); its markers and clusters keep their
//! `start:`/`end:`/`cluster:` form around the prefixed owner
//! (`start:tasks.call/tasks.p`, `cluster:tasks.call/triggers`). Two special
//! cases make the child's flow the cluster's body:
//!
//! - the child's `start:flow` IS the `Subflow` node (the child's edges leave
//!   from it, as a flowable's construct leaves from its own node), and
//! - the child's `end:flow` is `end:{subflow_uid}`;
//!
//! and the cluster adds `start:{subflow_uid}`, with `start:S -> S`
//! `sequential`, exactly as a flowable is entered (module doc, "Edges"). The
//! parent's edges into `S` now enter `start:S`, and its edges out of `S` now
//! leave `end:S`. So a `Subflow` that expanded is drawn as a flowable is;
//! its node keeps `kind: executable`, since what it IS did not change.
//!
//! **Why `/` and not `.`.** Refusals are attributed (module doc, "Refusals")
//! by trimming a path back one `.`-segment at a time. With a `.` join,
//! `tasks.call.tasks.x` would read as a path UNDER the `Subflow` task
//! `tasks.call`, and trimming it would reach `tasks.call` -- a child's
//! finding would land on the parent's node. With `/`, trimming
//! `tasks.call/tasks.x.y` gives `tasks.call/tasks.x`, then `tasks.call/tasks`,
//! then `tasks`: it can never stop at the `Subflow` node, nor at any parent
//! path. The `/` also shows a reader where one flow ends and the next begins,
//! which no `.` can (an id may hold dots). Each flow's refusals are attributed
//! inside its own build, before any prefixing.
//!
//! **Collisions.** egret enforces no task-id grammar, so a parent task whose
//! id itself holds a `/` (`call/tasks.x`) could already own a prefixed uid.
//! Every prefixed name is therefore taken through the same `~2`, `~3`, ...
//! de-duplication the builder uses, against every uid and owner the graph
//! already holds, so one uid still names one node. `subflows` is matched
//! against the un-de-duplicated name (`{subflow_uid}/{child_uid}`), which is
//! the uid itself except in that collision.
//!
//! **Branch.** A child's task nodes take the `Subflow` node's branch when it
//! is not `tasks` (Kestra's `updateWithChildren(branchType)`), so an
//! expansion under `errors:` is drawn as an error branch.
//!
//! # On an execution (spec §3.4 last paragraph, §4)
//!
//! [`execution_graph`] expands through the same splice, with one more source:
//!
//! - A `Subflow` node whose ONE run (in the execution itself) started a child
//!   execution expands from that child EXECUTION's frozen `source`, read
//!   through [`ExecutionSource`], and carries the child's states under the
//!   prefixed uids. The child is the newest execution linked to the run by
//!   `trigger_execution_id` + `trigger_run_ix` (a retried `Subflow` starts one
//!   per attempt). `dynamic` does not apply: the child execution is the
//!   target the run rendered. `depth` still does, checked first.
//! - Otherwise -- the run has not started a child yet (or failed before it
//!   could), the node never ran, or its runs are folded over `Loop`
//!   iterations and so name several children -- the node expands exactly as
//!   on the flow graph: the reasons above, then the child flow's CURRENT
//!   draft, with no states. Every execution runs the saved draft (W7a), so
//!   once the child starts, the next poll draws the same shape from its
//!   frozen source, now with states.
//!
//! # The node limit (§3.5)
//!
//! [`MAX_NODES`] counts nodes after expansion. Each flow's graph is built
//! with what the running total has left as its budget (and stops placing
//! nodes the moment it passes it), its node count added to a running total,
//! and the total checked BEFORE any of its `Subflow`s is loaded: building
//! stops the moment the total crosses the limit, so a pathological expansion
//! never builds more than the limit plus one flow. A splice adds
//! `start:S` and drops the child's `start:flow`, so the running total is the
//! final count exactly.

use std::collections::{BTreeMap, HashMap, HashSet};

use futures_util::future::BoxFuture;
use uuid::Uuid;

use super::states::{level_states, ExecutionGraph, GraphRun, LevelStates, NodeState};
use super::{
    build, cluster_uid, end_uid, start_uid, Branch, Cluster, ClusterKind, Edge, ExpansionReason,
    FlowGraph, GraphError, GraphOptions, Node, NodeKind, Relation, SubflowRef, FLOW_OWNER,
    MAX_NODES,
};
use crate::models::Flow;
use crate::repo::{ExecutionRepo, FlowRepo};
use crate::spec::parse;

/// How many expansions deep a graph goes: the request flow's child (1) and
/// grandchild (2) expand; a `Subflow` asked for below that gets `depth`.
pub const MAX_EXPANSION_DEPTH: u8 = 2;

/// Joins a `Subflow` node's uid to a child uid (module doc, "Uids").
pub const SUBFLOW_SEPARATOR: char = '/';

/// Where an expansion loads a child flow from.
#[async_trait::async_trait]
pub trait SubflowSource: Send + Sync {
    /// The YAML source of the current draft of the flow `(namespace,
    /// flow_id)` in workspace `ws`, or `None` when that workspace has no
    /// such (untrashed) flow.
    async fn current_draft(
        &self,
        ws: Uuid,
        namespace: &str,
        flow_id: &str,
    ) -> egret_core::Result<Option<String>>;
}

#[async_trait::async_trait]
impl SubflowSource for FlowRepo {
    async fn current_draft(
        &self,
        ws: Uuid,
        namespace: &str,
        flow_id: &str,
    ) -> egret_core::Result<Option<String>> {
        self.draft_source_by_name(ws, namespace, flow_id).await
    }
}

/// An execution's frozen source and the runs its graph is drawn with.
#[derive(Debug, Clone)]
pub struct FrozenExecution {
    pub source: String,
    /// Its runs and its `Loop` iterations' runs (at any depth), iterations
    /// in loop order.
    pub runs: Vec<GraphRun>,
}

/// Where an execution graph loads an execution from.
#[async_trait::async_trait]
pub trait ExecutionSource: Send + Sync {
    /// Execution `id` of workspace `ws`, or `None` when that workspace has no
    /// such execution.
    async fn frozen(&self, ws: Uuid, id: Uuid) -> egret_core::Result<Option<FrozenExecution>>;
}

#[async_trait::async_trait]
impl ExecutionSource for ExecutionRepo {
    async fn frozen(&self, ws: Uuid, id: Uuid) -> egret_core::Result<Option<FrozenExecution>> {
        self.graph_source(ws, id).await
    }
}

/// The sources one expansion reads: drafts always; executions only for an
/// execution graph.
#[derive(Clone, Copy)]
struct Sources<'a> {
    drafts: &'a dyn SubflowSource,
    executions: Option<&'a dyn ExecutionSource>,
}

/// [`super::flow_graph`], with every `Subflow` node whose uid is in
/// `opts.subflows` expanded from `source` within workspace `ws` (module doc).
pub async fn expanded_flow_graph(
    flow: &Flow,
    opts: &GraphOptions,
    ws: Uuid,
    source: &dyn SubflowSource,
) -> Result<FlowGraph, GraphError> {
    let mut total = 0;
    let sources = Sources {
        drafts: source,
        executions: None,
    };
    let (graph, _) = graph_of(flow, opts, String::new(), ws, sources, None, &mut total).await?;
    Ok(graph)
}

/// The graph of an execution whose frozen source parsed to `flow` and whose
/// runs are `runs`, with each node's state (spec §4) and every requested
/// `Subflow` expanded (module doc, "On an execution").
pub async fn execution_graph(
    flow: &Flow,
    runs: &[GraphRun],
    opts: &GraphOptions,
    ws: Uuid,
    drafts: &dyn SubflowSource,
    executions: &dyn ExecutionSource,
) -> Result<ExecutionGraph, GraphError> {
    let mut total = 0;
    let sources = Sources {
        drafts,
        executions: Some(executions),
    };
    let (graph, states) = graph_of(
        flow,
        opts,
        String::new(),
        ws,
        sources,
        Some(runs),
        &mut total,
    )
    .await?;
    Ok(ExecutionGraph { graph, states })
}

/// One flow's graph, with its expansions spliced in, and its states.
type Drawn = (FlowGraph, BTreeMap<String, NodeState>);

/// A child's graph and states, or why it has none.
type Child = Result<Drawn, ExpansionReason>;

/// `prefix`: the final uid of the `Subflow` node this flow expands (empty
/// for the request flow), so a node here is selected by its final uid.
/// `runs`: this flow's runs when it is an execution's, `None` when it is a
/// draft (no states).
fn graph_of<'a>(
    flow: &'a Flow,
    opts: &'a GraphOptions,
    prefix: String,
    ws: Uuid,
    sources: Sources<'a>,
    runs: Option<&'a [GraphRun]>,
    total: &'a mut usize,
) -> BoxFuture<'a, Result<Drawn, GraphError>> {
    Box::pin(async move {
        // This level may place what the running total has left, so the build
        // itself stops as the limit is crossed (`build`).
        let budget = MAX_NODES.saturating_sub(*total);
        let (mut graph, subflows) = build(flow, budget).map_err(|e| match e {
            GraphError::TooLarge { nodes } => GraphError::TooLarge {
                nodes: *total + nodes,
            },
            other => other,
        })?;
        *total += graph.nodes.len();
        if *total > MAX_NODES {
            return Err(GraphError::TooLarge { nodes: *total });
        }
        // Mapped before any splice: this flow's runs name this flow's tasks.
        let LevelStates {
            mut states,
            children,
        } = match runs {
            Some(runs) => level_states(&graph, runs)?,
            None => LevelStates::default(),
        };
        for sub in subflows {
            let full = if prefix.is_empty() {
                sub.uid.clone()
            } else {
                format!("{prefix}{SUBFLOW_SEPARATOR}{}", sub.uid)
            };
            if !opts.subflows.contains(&full) {
                continue;
            }
            let child_opts = GraphOptions {
                subflows: opts.subflows.clone(),
                depth: opts.depth + 1,
            };
            let started = match (sources.executions, children.get(&sub.uid)) {
                (Some(executions), Some(child)) => {
                    if opts.depth >= MAX_EXPANSION_DEPTH {
                        Some(Err(ExpansionReason::Depth))
                    } else {
                        executions
                            .frozen(ws, *child)
                            .await
                            .map_err(|e| GraphError::Source(e.to_string()))?
                            .map(Ok)
                    }
                }
                _ => None,
            };
            let child: Child = match started {
                Some(Err(reason)) => Err(reason),
                Some(Ok(frozen)) => {
                    let child_flow = parse(&frozen.source).map_err(|e| {
                        GraphError::Source(format!(
                            "the frozen source of execution of '{}'.'{}' does not parse: {}",
                            sub.namespace.as_deref().unwrap_or_default(),
                            sub.flow_id.as_deref().unwrap_or_default(),
                            e.message
                        ))
                    })?;
                    Ok(graph_of(
                        &child_flow,
                        &child_opts,
                        full,
                        ws,
                        sources,
                        Some(&frozen.runs),
                        total,
                    )
                    .await?)
                }
                None => match target(&sub, opts.depth) {
                    Err(reason) => Err(reason),
                    Ok((namespace, flow_id)) => {
                        match sources
                            .drafts
                            .current_draft(ws, namespace, flow_id)
                            .await
                            .map_err(|e| GraphError::Source(e.to_string()))?
                        {
                            None => Err(ExpansionReason::NotFound),
                            Some(yaml) => {
                                let child_flow = parse(&yaml).map_err(|e| {
                                    GraphError::Source(format!(
                                        "the draft of '{namespace}'.'{flow_id}' does not parse: {}",
                                        e.message
                                    ))
                                })?;
                                Ok(graph_of(
                                    &child_flow,
                                    &child_opts,
                                    full,
                                    ws,
                                    sources,
                                    None,
                                    total,
                                )
                                .await?)
                            }
                        }
                    }
                },
            };
            splice(&mut graph, &mut states, &sub, child);
        }
        Ok((graph, states))
    })
}

/// The `(namespace, flowId)` to load, or why there is none to load. The
/// order is the module doc's ("Reasons").
fn target(sub: &SubflowRef, depth: u8) -> Result<(&str, &str), ExpansionReason> {
    let dynamic = |v: &Option<String>| {
        v.as_deref()
            .is_some_and(crate::scripts::gate::is_unrendered)
    };
    if dynamic(&sub.namespace) || dynamic(&sub.flow_id) {
        return Err(ExpansionReason::Dynamic);
    }
    if depth >= MAX_EXPANSION_DEPTH {
        return Err(ExpansionReason::Depth);
    }
    match (&sub.namespace, &sub.flow_id) {
        (Some(ns), Some(fid)) => Ok((ns, fid)),
        _ => Err(ExpansionReason::NotFound),
    }
}

/// The marker or cluster owner a uid names (`start:x`, `end:x`, `cluster:x`).
fn owner_of_marker(uid: &str) -> Option<&str> {
    uid.strip_prefix("start:")
        .or_else(|| uid.strip_prefix("end:"))
        .or_else(|| uid.strip_prefix("cluster:"))
}

/// Put `child` (or the reason it is missing) in place of the `Subflow` node
/// `sub` (module doc, "Uids"); the child's states move to its renamed uids.
fn splice(
    graph: &mut FlowGraph,
    states: &mut BTreeMap<String, NodeState>,
    sub: &SubflowRef,
    child: Child,
) {
    let s = sub.uid.as_str();
    let home = graph
        .clusters
        .iter()
        .position(|c| c.nodes.iter().any(|n| n == s));
    let parent = home.map(|ix| graph.clusters[ix].uid.clone());

    let (child, child_states) = match child {
        Ok(child) => child,
        Err(reason) => {
            graph.clusters.push(Cluster {
                uid: cluster_uid(s),
                task_id: Some(sub.task_id.clone()),
                kind: ClusterKind::Subflow,
                nodes: Vec::new(),
                parent,
                start: s.to_string(),
                end: s.to_string(),
                reason: Some(reason),
            });
            return;
        }
    };

    // Every name the graph already holds: task and trigger uids, and the
    // owners of its markers and clusters.
    let mut used: HashSet<String> = HashSet::new();
    for n in &graph.nodes {
        used.insert(owner_of_marker(&n.uid).unwrap_or(&n.uid).to_string());
    }
    for c in &graph.clusters {
        used.insert(owner_of_marker(&c.uid).unwrap_or(&c.uid).to_string());
    }
    // Each child name, prefixed and de-duplicated, in walk order.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut take = |name: &str| {
        if name == FLOW_OWNER || names.contains_key(name) {
            return;
        }
        let base = format!("{s}{SUBFLOW_SEPARATOR}{name}");
        let mut uid = base.clone();
        let mut n = 1;
        while used.contains(&uid) {
            n += 1;
            uid = format!("{base}~{n}");
        }
        used.insert(uid.clone());
        names.insert(name.to_string(), uid);
    };
    for n in &child.nodes {
        take(owner_of_marker(&n.uid).unwrap_or(&n.uid));
    }
    for c in &child.clusters {
        take(owner_of_marker(&c.uid).unwrap_or(&c.uid));
    }

    let start = start_uid(s);
    let end = end_uid(s);
    let flow_start = start_uid(FLOW_OWNER);
    let flow_end = end_uid(FLOW_OWNER);
    let map = |uid: &str| -> String {
        if uid == flow_start {
            return s.to_string();
        }
        if uid == flow_end {
            return end.clone();
        }
        for (marker, make) in [
            ("start:", start_uid as fn(&str) -> String),
            ("end:", end_uid),
            ("cluster:", cluster_uid),
        ] {
            if let Some(owner) = uid.strip_prefix(marker) {
                return make(&names[owner]);
            }
        }
        names[uid].clone()
    };

    for (uid, state) in child_states {
        states.insert(map(&uid), state);
    }

    // The parent's edges now enter and leave the expansion's markers.
    for e in &mut graph.edges {
        if e.target == s {
            e.target = start.clone();
        }
        if e.source == s {
            e.source = end.clone();
        }
    }
    if let Some(ix) = home {
        graph.clusters[ix].nodes.retain(|n| n != s);
    }

    let clustered: HashSet<&str> = child
        .clusters
        .iter()
        .flat_map(|c| c.nodes.iter().map(String::as_str))
        .collect();
    let mut members = vec![start.clone(), s.to_string()];
    graph.nodes.push(Node {
        uid: start.clone(),
        kind: NodeKind::Start,
    });
    for n in &child.nodes {
        if n.uid == flow_start {
            continue;
        }
        let uid = map(&n.uid);
        if !clustered.contains(n.uid.as_str()) {
            members.push(uid.clone());
        }
        let mut kind = n.kind.clone();
        if let NodeKind::Task { branch, .. } = &mut kind {
            if sub.branch != Branch::Tasks {
                *branch = sub.branch;
            }
        }
        graph.nodes.push(Node { uid, kind });
    }

    graph.edges.push(Edge {
        source: start.clone(),
        target: s.to_string(),
        relation: Relation::Sequential,
        label: None,
    });
    for e in &child.edges {
        graph.edges.push(Edge {
            source: map(&e.source),
            target: map(&e.target),
            relation: e.relation,
            label: e.label.clone(),
        });
    }

    let own = cluster_uid(s);
    graph.clusters.push(Cluster {
        uid: own.clone(),
        task_id: Some(sub.task_id.clone()),
        kind: ClusterKind::Subflow,
        nodes: members,
        parent,
        start,
        end: end.clone(),
        reason: None,
    });
    for c in &child.clusters {
        graph.clusters.push(Cluster {
            uid: map(&c.uid),
            task_id: c.task_id.clone(),
            kind: c.kind,
            nodes: c.nodes.iter().map(|n| map(n)).collect(),
            parent: Some(match c.parent.as_deref() {
                Some(p) => map(p),
                None => own.clone(),
            }),
            start: map(&c.start),
            end: map(&c.end),
            reason: c.reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use super::*;

    /// An in-memory [`SubflowSource`]: `(ws, namespace, flow_id) -> source`.
    struct Memory(HashMap<(Uuid, String, String), String>);

    #[async_trait::async_trait]
    impl SubflowSource for Memory {
        async fn current_draft(
            &self,
            ws: Uuid,
            namespace: &str,
            flow_id: &str,
        ) -> egret_core::Result<Option<String>> {
            Ok(self
                .0
                .get(&(ws, namespace.to_string(), flow_id.to_string()))
                .cloned())
        }
    }

    const LOG: &str = "io.kestra.plugin.core.log.Log";

    #[tokio::test]
    async fn a_parent_uid_that_already_holds_a_prefixed_name_is_not_shared() {
        let ws = Uuid::new_v4();
        let child = format!("id: c\nnamespace: n\ntasks: [{{id: x, type: {LOG}, message: m}}]\n");
        let source = Memory(HashMap::from([(
            (ws, "n".to_string(), "c".to_string()),
            child,
        )]));
        // A sibling whose id holds the separator owns `tasks.call/tasks.x`.
        let parent = parse(&format!(
            "id: p\nnamespace: q\ntasks:\n  - {{id: \"call/tasks.x\", type: {LOG}, message: m}}\n  - {{id: call, type: io.kestra.plugin.core.flow.Subflow, namespace: n, flowId: c}}\n"
        ))
        .unwrap();
        let opts = GraphOptions {
            subflows: BTreeSet::from(["tasks.call".to_string()]),
            depth: 0,
        };
        let g = expanded_flow_graph(&parent, &opts, ws, &source)
            .await
            .unwrap();
        let uids: Vec<&str> = g.nodes.iter().map(|n| n.uid.as_str()).collect();
        let unique: HashSet<&str> = uids.iter().copied().collect();
        assert_eq!(unique.len(), uids.len(), "one uid, one node: {uids:?}");
        assert!(uids.contains(&"tasks.call/tasks.x"), "the parent's own");
        assert!(
            uids.contains(&"tasks.call/tasks.x~2"),
            "the child's: {uids:?}"
        );
        // The child's edges follow its renamed node.
        assert!(g
            .edges
            .iter()
            .any(|e| e.source == "tasks.call" && e.target == "tasks.call/tasks.x~2"));
    }

    #[tokio::test]
    async fn a_child_refusal_never_lands_on_the_subflow_node() {
        let ws = Uuid::new_v4();
        let child = "id: c\nnamespace: n\ntasks: [{id: bad, type: no.such.Type}]\n".to_string();
        let source = Memory(HashMap::from([(
            (ws, "n".to_string(), "c".to_string()),
            child,
        )]));
        let parent = parse(
            "id: p\nnamespace: q\ntasks:\n  - {id: call, type: io.kestra.plugin.core.flow.Subflow, namespace: n, flowId: c}\n",
        )
        .unwrap();
        let opts = GraphOptions {
            subflows: BTreeSet::from(["tasks.call".to_string()]),
            depth: 0,
        };
        let g = expanded_flow_graph(&parent, &opts, ws, &source)
            .await
            .unwrap();
        let refusal = |uid: &str| match &g.nodes.iter().find(|n| n.uid == uid).unwrap().kind {
            NodeKind::Task { refusal, .. } => refusal.clone(),
            _ => panic!("{uid} is not a task"),
        };
        assert_eq!(refusal("tasks.call"), None);
        assert_eq!(
            refusal("tasks.call/tasks.bad").as_deref(),
            Some("unknown_task_type")
        );
    }
}
