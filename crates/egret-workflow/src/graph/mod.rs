//! W7c: the read-only topology graph of a flow (spec
//! `docs/en/specs/2026-10-13-workflow-w7c-topology-design.md` §3).
//!
//! [`flow_graph`] is a pure function over the PARSED model ([`Flow`],
//! [`TaskKind`]), not over the executor's plan: the plan drops disabled
//! tasks and refused constructs, and the graph must show both.
//!
//! Kestra builds the same picture in `core/.../utils/GraphUtils.java` and
//! `core/.../models/hierarchies/`; this module follows its shapes, but the
//! spec's §3.2 table and the rulings below are what bind.
//!
//! # Uids
//!
//! - A task node's uid is its validation path, spelled by
//!   [`crate::spec::path`] -- the same two functions `spec::validate` uses for
//!   findings, so a node and a finding about it name the same place
//!   (`tasks.seq.child`, `errors.alert`).
//! - A trigger's uid is `triggers.{id}`.
//! - Markers and clusters are `start:{owner}`, `end:{owner}` and
//!   `cluster:{owner}`, where the owner is `flow`, `triggers`, or a
//!   flowable's own uid. egret enforces no task-id grammar, so this cannot
//!   lean on a character ids never contain. It leans on structure instead:
//!   every task path starts with `tasks.`, `errors.`, `finally.` or
//!   `afterExecution.`, and every trigger uid with `triggers.`, so no task or
//!   trigger uid starts with `start:`, `end:` or `cluster:` -- even for a task
//!   literally named `start:flow` (its uid is `tasks.start:flow`).
//! - **Duplicate ids** (refused as `duplicate_task_id`, but a graph is still
//!   drawn) would give two nodes one uid, and the client needs one id per
//!   node. The first occurrence in walk order keeps the plain path; a later
//!   one takes the first free of `~2`, `~3`, ... Its descendants' uids are
//!   built under the suffixed uid, while their `refusal` is looked up under
//!   the plain validation path.
//!
//! # Edges (spec §3.2, refined by the controller's rulings of 2026-10-13)
//!
//! - **A flowable's own node sits inside its cluster.** `start:X -> X` is
//!   `sequential`; the construct's edges leave from the node `X`; the
//!   flowable's `errors:` leave from `start:X`. Every placed flowable is
//!   entered at `start:X` and left at `end:X`.
//! - A task list (`Sequential`, `AllowFailure`, `WorkingDirectory`,
//!   `LoopUntil`, a `Pause` with `tasks:`): `sequential` from `X`, through
//!   the list, to `end:X`.
//! - `Parallel`: `parallel` from `X` to each child and from each child to
//!   `end:X`.
//! - `If` / `Switch`: the first edge of a branch is `choice` with the label
//!   (`then`, `else`, the case key, `defaults`); later edges of that branch
//!   and its edge to `end:X` are `sequential` and carry the same label. An
//!   empty branch draws no edge. Cases are drawn in key order, then
//!   `defaults`.
//! - `Dag`: `dynamic` from `X` to each node with no `dependsOn`, one edge per
//!   `dependsOn` entry naming a node of this dag, and from each node nothing
//!   depends on to `end:X`. A node with `task: null` or a `dependsOn` naming
//!   no node draws nothing; so a node whose `dependsOn` entries ALL dangle has
//!   no incoming edge and sits as an orphan inside the cluster.
//! - `Loop`: `dynamic` on every edge from `X` through the body to `end:X`.
//! - `errors:` (of the flow or of a flowable): `error` on every edge, from
//!   the owner's start to the list and on to what follows it.
//! - The flow: `start:flow -> tasks -> finally -> afterExecution -> end:flow`.
//!   The flow's `errors:` leave `start:flow`; the last error goes to the first
//!   `finally` task, else the first `afterExecution` task, else `end:flow`. A
//!   flowable's last error goes to its cluster's `end`.
//! - The relation of an edge between the flow's lists: an `error` chain is
//!   `error` on every edge; otherwise the edge INTO the first `finally` or
//!   `afterExecution` task carries that list's relation, and every other edge
//!   carries the relation of the list it leaves (so `tasks -> end:flow` is
//!   `sequential` and `finally -> end:flow` is `finally`).
//! - Triggers, as Kestra chains them: `start:triggers -> trigger -> end:triggers`
//!   for each trigger and `end:triggers -> start:flow`, all `sequential`. **A
//!   trigger edge -- the one the client draws dashed -- is an edge with an
//!   endpoint in the `triggers` cluster** (`cluster:triggers`'s `nodes`).
//!   A flow with no triggers has no `triggers` cluster.
//! - An empty list draws no edge, at any level.
//!
//! # Clusters
//!
//! Every flowable is a cluster: `Sequential`, `Parallel`, `If`, `Switch`,
//! `Dag`, `Loop`, `LoopUntil`, `AllowFailure`, `WorkingDirectory`, and a
//! `Pause` that has `tasks:` or `errors:`. A cluster's `nodes` are the nodes
//! whose INNERMOST cluster it is (its markers, its own task node, its direct
//! leaf children); a nested cluster names its container in `parent`. Nodes
//! in no cluster (the flow's markers and its top-level leaves) belong to the
//! flow. A task node's `kind` is `flowable` exactly when it owns a cluster,
//! `executable` for a `Subflow`, and `leaf` otherwise (a `Pause` with neither
//! list included). A `Pause`'s `onPause` task is not drawn: validation gives
//! it no path. Descendants of an `errors:`, `finally:` or `afterExecution:`
//! task inherit that branch.
//!
//! # Refusals
//!
//! `validate_for_run` runs once; its rules are not re-derived here. An error
//! **belongs to the task node whose validation path is the LONGEST prefix of
//! the error's path**, where a prefix is the path itself or one followed by
//! `.` (controller ruling, Task 1 fix round 1). A node's `refusal` is the code
//! of the FIRST error, in validation order, that belongs to it. So an error at
//! the task's own path lands on it, and so does one at a property path:
//! `unsupported_pause_property` at `tasks.p.tasks` and
//! `unsupported_loop_until_property` at `tasks.l.{field}` land on `tasks.p`
//! and `tasks.l`, and `working_directory_child_not_runnable` at
//! `tasks.wd.tasks.c` lands on the `WorkingDirectory` `tasks.wd` (itself
//! refused). Only task nodes own errors; the flow is not a node, so an error
//! no task path prefixes (`namespace`, `inputs.x`, `triggers...`) belongs to
//! nothing. A de-duplicated node (`~2`) is matched by its plain validation
//! path, like its first occurrence.
//!
//! **Residual ambiguity, accepted rather than engineered around:** a child
//! whose id is literally a property name -- a task with id `tasks` under a
//! flowable -- has the path `tasks.x.tasks`, so it captures its parent's
//! error at the property path `tasks.x.tasks`.
//!
//! # `Subflow` expansion
//!
//! [`flow_graph`] stays pure and never expands. [`expanded_flow_graph`]
//! (module `expand`) builds the same graph and splices each requested
//! `Subflow`'s child flow in, loaded through [`SubflowSource`]; its module doc
//! holds the expansion's rules: the `{subflow_uid}/{child_uid}` uids, the
//! reasons, the depth limit and how the node limit is counted.
//!
//! # The execution graph (spec §4)
//!
//! [`execution_graph`] draws an execution's FROZEN source and overlays each
//! node's run state ([`ExecutionGraph`]; module `states` holds the run-to-node
//! map and [`fold_state`]). A `Subflow` expanded on an execution draws the
//! child EXECUTION's frozen source with its states when the node's run
//! started one (module `expand`, "On an execution").

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use utoipa::ToSchema;

use crate::models::{Flow, Task, TaskKind};
use crate::spec::path::{root_lists, task_path};
use crate::spec::validate_for_run;

/// The most nodes a graph may hold, counted after expansion (spec §3.5).
pub const MAX_NODES: usize = 5_000;

/// The owner of the flow's own markers (`start:flow`, `end:flow`).
pub const FLOW_OWNER: &str = "flow";
/// The owner of the triggers cluster and its markers.
pub const TRIGGERS_OWNER: &str = "triggers";

/// `start:{owner}`.
pub fn start_uid(owner: &str) -> String {
    format!("start:{owner}")
}

/// `end:{owner}`.
pub fn end_uid(owner: &str) -> String {
    format!("end:{owner}")
}

/// `cluster:{owner}`.
pub fn cluster_uid(owner: &str) -> String {
    format!("cluster:{owner}")
}

/// What to build. `subflows` (the `Subflow` node uids to expand, in the
/// final, prefixed uid space) and `depth` (how many expansions deep the flow
/// being built sits: 0 for the flow the request names) are read by
/// [`expanded_flow_graph`] only; [`flow_graph`] performs no expansion and
/// ignores both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphOptions {
    pub subflows: BTreeSet<String>,
    pub depth: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FlowGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub clusters: Vec<Cluster>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Node {
    pub uid: String,
    #[serde(flatten)]
    pub kind: NodeKind,
}

/// The node's kind, serialised as the `node` tag beside its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum NodeKind {
    Task {
        task_id: String,
        /// The `type:` as written.
        r#type: String,
        /// The FQCN for a flowable or `Subflow` (whichever spelling was
        /// written) and for a registry type (its canonical row); otherwise
        /// the type as written.
        canonical_type: String,
        kind: TaskNodeKind,
        branch: Branch,
        disabled: bool,
        refusal: Option<String>,
    },
    Trigger {
        trigger_id: String,
        r#type: String,
        disabled: bool,
    },
    Start,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskNodeKind {
    Leaf,
    Executable,
    Flowable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Branch {
    Tasks,
    Errors,
    Finally,
    AfterExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub relation: Relation,
    pub label: Option<String>,
}

/// Kestra's seven `RelationType`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Sequential,
    Choice,
    Parallel,
    Dynamic,
    Error,
    Finally,
    AfterExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Cluster {
    pub uid: String,
    /// The flowable's task id; `None` for the triggers cluster.
    pub task_id: Option<String>,
    pub kind: ClusterKind,
    pub nodes: Vec<String>,
    pub parent: Option<String>,
    pub start: String,
    pub end: String,
    /// Why a `subflow` cluster is empty (spec §3.4); `None` on every other
    /// cluster and on a `subflow` cluster that expanded.
    pub reason: Option<ExpansionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterKind {
    Sequential,
    Parallel,
    If,
    Switch,
    Dag,
    Loop,
    LoopUntil,
    AllowFailure,
    WorkingDirectory,
    Pause,
    Triggers,
    /// An expanded `Subflow`, or one that could not be (`reason`); see
    /// [`expand`].
    Subflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpansionReason {
    NotFound,
    Dynamic,
    Depth,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// During expansion, `nodes` is the count when the limit was crossed:
    /// building stops there, so a larger expansion is never built whole.
    #[error("the graph has {nodes} nodes, over the limit of {MAX_NODES}")]
    TooLarge { nodes: usize },
    /// A `Subflow`'s child could not be loaded, or its stored draft no longer
    /// parses (a stored draft always parsed when it was saved).
    #[error("expanding a subflow: {0}")]
    Source(String),
    /// A stored run's state is not a `RunState` wire name (execution graph
    /// only; a run is written through `RunState::wire`, so this is corrupt
    /// data).
    #[error("{0}")]
    State(String),
}

/// Build the topology of `flow` (spec §3). See the module doc for every
/// rule. Pure: it performs no `Subflow` expansion and does not read `opts`;
/// [`expanded_flow_graph`] does both.
pub fn flow_graph(flow: &Flow, opts: &GraphOptions) -> Result<FlowGraph, GraphError> {
    let _ = opts;
    build(flow).map(|(graph, _)| graph)
}

/// A `Subflow` task node as placed, with the target it names -- what
/// [`expand`] needs and the node does not carry.
#[derive(Debug, Clone)]
pub(crate) struct SubflowRef {
    pub uid: String,
    pub task_id: String,
    pub branch: Branch,
    pub namespace: Option<String>,
    pub flow_id: Option<String>,
}

/// [`flow_graph`], plus every `Subflow` node it placed, in walk order.
pub(crate) fn build(flow: &Flow) -> Result<(FlowGraph, Vec<SubflowRef>), GraphError> {
    let mut b = Builder {
        graph: FlowGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            clusters: Vec::new(),
        },
        used: HashSet::new(),
        task_paths: Vec::new(),
        subflows: Vec::new(),
    };
    b.flow(flow);
    b.attribute_refusals(flow);
    let nodes = b.graph.nodes.len();
    if nodes > MAX_NODES {
        return Err(GraphError::TooLarge { nodes });
    }
    Ok((b.graph, b.subflows))
}

/// Where a list (or a task) placed in the graph is entered and left.
#[derive(Debug, Clone)]
struct Span {
    entry: String,
    exit: String,
}

/// The position the next task is placed at.
#[derive(Debug, Clone)]
struct Ctx {
    /// The validation path prefix, for `refusal`.
    path: String,
    /// The uid prefix (the path, unless an ancestor was de-duplicated).
    uid: String,
    branch: Branch,
    /// Index into `graph.clusters` of the innermost cluster.
    cluster: Option<usize>,
}

impl Ctx {
    fn root(list: &str, branch: Branch) -> Ctx {
        Ctx {
            path: list.to_string(),
            uid: list.to_string(),
            branch,
            cluster: None,
        }
    }
}

struct Builder {
    graph: FlowGraph,
    used: HashSet<String>,
    /// Every task node, as (index into `graph.nodes`, validation path).
    task_paths: Vec<(usize, String)>,
    /// Every `Subflow` node, in walk order.
    subflows: Vec<SubflowRef>,
}

/// The longest prefix of `path` -- `path` itself, or a part of it ending
/// just before a `.` -- that is in `paths`.
fn owner_of<'p>(path: &'p str, paths: &HashSet<&str>) -> Option<&'p str> {
    let mut candidate = path;
    loop {
        if paths.contains(candidate) {
            return Some(candidate);
        }
        candidate = &candidate[..candidate.rfind('.')?];
    }
}

impl Builder {
    /// Set every task node's `refusal` (module doc, "Refusals").
    fn attribute_refusals(&mut self, flow: &Flow) {
        let paths: HashSet<&str> = self.task_paths.iter().map(|(_, p)| p.as_str()).collect();
        let mut owned: HashMap<&str, &'static str> = HashMap::new();
        let errors = validate_for_run(flow).errors;
        for finding in &errors {
            if let Some(owner) = owner_of(&finding.path, &paths) {
                owned.entry(owner).or_insert(finding.code);
            }
        }
        for (ix, path) in &self.task_paths {
            if let NodeKind::Task { refusal, .. } = &mut self.graph.nodes[*ix].kind {
                *refusal = owned.get(path.as_str()).map(|c| c.to_string());
            }
        }
    }

    fn flow(&mut self, flow: &Flow) {
        let start = start_uid(FLOW_OWNER);
        let end = end_uid(FLOW_OWNER);
        self.add_node(start.clone(), NodeKind::Start, None);
        self.triggers(flow, &start);

        let [(tasks_name, tasks), (errors_name, errors), (finally_name, finally), (after_name, after)] =
            root_lists(flow);
        let main = self.list(
            tasks,
            &Ctx::root(tasks_name, Branch::Tasks),
            Relation::Sequential,
            None,
        );
        let errs = self.list(
            errors,
            &Ctx::root(errors_name, Branch::Errors),
            Relation::Error,
            None,
        );
        let fin = self.list(
            finally,
            &Ctx::root(finally_name, Branch::Finally),
            Relation::Finally,
            None,
        );
        let after = self.list(
            after,
            &Ctx::root(after_name, Branch::AfterExecution),
            Relation::AfterExecution,
            None,
        );

        // What follows the main list (and the errors list): the first
        // `finally` task, else the first `afterExecution` task, else the end.
        let (next, next_relation) = match (&fin, &after) {
            (Some(f), _) => (f.entry.clone(), Some(Relation::Finally)),
            (None, Some(a)) => (a.entry.clone(), Some(Relation::AfterExecution)),
            (None, None) => (end.clone(), None),
        };
        match &main {
            Some(m) => {
                self.edge(&start, &m.entry, Relation::Sequential, None);
                let relation = next_relation.unwrap_or(Relation::Sequential);
                self.edge(&m.exit, &next, relation, None);
            }
            None => {
                if let Some(relation) = next_relation {
                    self.edge(&start, &next, relation, None);
                }
            }
        }
        if let Some(e) = &errs {
            self.edge(&start, &e.entry, Relation::Error, None);
            self.edge(&e.exit, &next, Relation::Error, None);
        }
        if let Some(f) = &fin {
            match &after {
                Some(a) => self.edge(&f.exit, &a.entry, Relation::AfterExecution, None),
                None => self.edge(&f.exit, &end, Relation::Finally, None),
            }
        }
        if let Some(a) = &after {
            self.edge(&a.exit, &end, Relation::AfterExecution, None);
        }
        self.add_node(end, NodeKind::End, None);
    }

    fn triggers(&mut self, flow: &Flow, flow_start: &str) {
        if flow.triggers.is_empty() {
            return;
        }
        let start = start_uid(TRIGGERS_OWNER);
        let end = end_uid(TRIGGERS_OWNER);
        let ix = self.graph.clusters.len();
        self.graph.clusters.push(Cluster {
            uid: cluster_uid(TRIGGERS_OWNER),
            task_id: None,
            kind: ClusterKind::Triggers,
            nodes: Vec::new(),
            parent: None,
            start: start.clone(),
            end: end.clone(),
            reason: None,
        });
        self.add_node(start.clone(), NodeKind::Start, Some(ix));
        for trigger in &flow.triggers {
            let uid = self.unique(task_path(TRIGGERS_OWNER, &trigger.id));
            self.add_node(
                uid.clone(),
                NodeKind::Trigger {
                    trigger_id: trigger.id.clone(),
                    r#type: trigger.r#type.clone(),
                    disabled: trigger.disabled,
                },
                Some(ix),
            );
            self.edge(&start, &uid, Relation::Sequential, None);
            self.edge(&uid, &end, Relation::Sequential, None);
        }
        self.add_node(end.clone(), NodeKind::End, Some(ix));
        self.edge(&end, flow_start, Relation::Sequential, None);
    }

    /// Place `tasks` in order with `relation` (and `label`) between
    /// consecutive tasks. The caller draws the edges into the first and out
    /// of the last. `None` for an empty list.
    fn list<'t>(
        &mut self,
        tasks: impl IntoIterator<Item = &'t Task>,
        ctx: &Ctx,
        relation: Relation,
        label: Option<&str>,
    ) -> Option<Span> {
        let mut span: Option<Span> = None;
        for task in tasks {
            let placed = self.task(task, ctx);
            span = Some(match span {
                None => placed,
                Some(s) => {
                    self.edge(&s.exit, &placed.entry, relation, label);
                    Span {
                        entry: s.entry,
                        exit: placed.exit,
                    }
                }
            });
        }
        span
    }

    /// `from -> list -> to`: the first edge `first`, the rest `rest`.
    #[allow(clippy::too_many_arguments)]
    fn chain<'t>(
        &mut self,
        from: &str,
        tasks: impl IntoIterator<Item = &'t Task>,
        ctx: &Ctx,
        first: Relation,
        rest: Relation,
        label: Option<&str>,
        to: &str,
    ) {
        if let Some(s) = self.list(tasks, ctx, rest, label) {
            self.edge(from, &s.entry, first, label);
            self.edge(&s.exit, to, rest, label);
        }
    }

    fn task(&mut self, task: &Task, ctx: &Ctx) -> Span {
        let path = task_path(&ctx.path, &task.id);
        let uid = self.unique(task_path(&ctx.uid, &task.id));
        let cluster_kind = cluster_kind(task);
        let node_kind = match (&task.kind, cluster_kind) {
            (_, Some(_)) => TaskNodeKind::Flowable,
            (TaskKind::Subflow(_), None) => TaskNodeKind::Executable,
            _ => TaskNodeKind::Leaf,
        };
        let node = NodeKind::Task {
            task_id: task.id.clone(),
            r#type: task.r#type.clone(),
            canonical_type: canonical_type(task),
            kind: node_kind,
            branch: ctx.branch,
            disabled: task.disabled,
            // Set by `attribute_refusals` once every task path is known.
            refusal: None,
        };

        let Some(kind) = cluster_kind else {
            if let TaskKind::Subflow(s) = &task.kind {
                self.subflows.push(SubflowRef {
                    uid: uid.clone(),
                    task_id: task.id.clone(),
                    branch: ctx.branch,
                    namespace: s.namespace.clone(),
                    flow_id: s.flow_id.clone(),
                });
            }
            self.add_node(uid.clone(), node, ctx.cluster);
            self.task_paths.push((self.graph.nodes.len() - 1, path));
            return Span {
                entry: uid.clone(),
                exit: uid,
            };
        };

        let start = start_uid(&uid);
        let end = end_uid(&uid);
        let ix = self.graph.clusters.len();
        let parent = ctx.cluster.map(|p| self.graph.clusters[p].uid.clone());
        self.graph.clusters.push(Cluster {
            uid: cluster_uid(&uid),
            task_id: Some(task.id.clone()),
            kind,
            nodes: Vec::new(),
            parent,
            start: start.clone(),
            end: end.clone(),
            reason: None,
        });
        self.add_node(start.clone(), NodeKind::Start, Some(ix));
        self.add_node(uid.clone(), node, Some(ix));
        self.task_paths
            .push((self.graph.nodes.len() - 1, path.clone()));
        self.edge(&start, &uid, Relation::Sequential, None);

        let inner = Ctx {
            path: path.clone(),
            uid: uid.clone(),
            branch: ctx.branch,
            cluster: Some(ix),
        };
        use Relation::*;
        match &task.kind {
            TaskKind::Sequential(s) => {
                self.chain(&uid, &s.tasks, &inner, Sequential, Sequential, None, &end)
            }
            TaskKind::AllowFailure(a) => {
                self.chain(&uid, &a.tasks, &inner, Sequential, Sequential, None, &end)
            }
            TaskKind::WorkingDirectory(w) => {
                self.chain(&uid, &w.tasks, &inner, Sequential, Sequential, None, &end)
            }
            TaskKind::LoopUntil(l) => {
                self.chain(&uid, &l.tasks, &inner, Sequential, Sequential, None, &end)
            }
            TaskKind::Pause(p) => {
                self.chain(&uid, &p.tasks, &inner, Sequential, Sequential, None, &end)
            }
            TaskKind::Loop(l) => self.chain(&uid, &l.tasks, &inner, Dynamic, Dynamic, None, &end),
            TaskKind::Parallel(p) => {
                for child in &p.tasks {
                    let s = self.task(child, &inner);
                    self.edge(&uid, &s.entry, Parallel, None);
                    self.edge(&s.exit, &end, Parallel, None);
                }
            }
            TaskKind::If(i) => {
                self.chain(
                    &uid,
                    &i.then,
                    &inner,
                    Choice,
                    Sequential,
                    Some("then"),
                    &end,
                );
                self.chain(
                    &uid,
                    &i.r#else,
                    &inner,
                    Choice,
                    Sequential,
                    Some("else"),
                    &end,
                );
            }
            TaskKind::Switch(s) => {
                for (key, tasks) in &s.cases {
                    self.chain(&uid, tasks, &inner, Choice, Sequential, Some(key), &end);
                }
                self.chain(
                    &uid,
                    &s.defaults,
                    &inner,
                    Choice,
                    Sequential,
                    Some("defaults"),
                    &end,
                );
            }
            TaskKind::Dag(d) => {
                let placed: Vec<(&Task, &[String], Span)> = d
                    .tasks
                    .iter()
                    .filter_map(|n| n.task.as_ref().map(|t| (t, n.depends_on.as_slice())))
                    .map(|(t, deps)| (t, deps, self.task(t, &inner)))
                    .collect();
                let mut by_id: HashMap<&str, &Span> = HashMap::new();
                for (t, _, s) in &placed {
                    by_id.entry(t.id.as_str()).or_insert(s);
                }
                let depended: HashSet<&str> = placed
                    .iter()
                    .flat_map(|(_, deps, _)| deps.iter().map(String::as_str))
                    .collect();
                let mut edges = Vec::new();
                for (t, deps, s) in &placed {
                    if deps.is_empty() {
                        edges.push((uid.clone(), s.entry.clone()));
                    }
                    for dep in deps.iter() {
                        if let Some(d) = by_id.get(dep.as_str()) {
                            edges.push((d.exit.clone(), s.entry.clone()));
                        }
                    }
                    if !depended.contains(t.id.as_str()) {
                        edges.push((s.exit.clone(), end.clone()));
                    }
                }
                for (from, to) in edges {
                    self.edge(&from, &to, Dynamic, None);
                }
            }
            TaskKind::Subflow(_) | TaskKind::Leaf { .. } => {
                unreachable!("cluster_kind gives no cluster to a Subflow or a leaf")
            }
        }

        let errors = Ctx {
            branch: Branch::Errors,
            ..inner
        };
        self.chain(
            &start,
            task.error_children(),
            &errors,
            Error,
            Error,
            None,
            &end,
        );
        self.add_node(end.clone(), NodeKind::End, Some(ix));
        Span {
            entry: start,
            exit: end,
        }
    }

    /// `path`, or the first of `path~2`, `path~3`, ... not yet used.
    fn unique(&mut self, path: String) -> String {
        let mut uid = path.clone();
        let mut n = 1;
        while self.used.contains(&uid) {
            n += 1;
            uid = format!("{path}~{n}");
        }
        self.used.insert(uid.clone());
        uid
    }

    fn add_node(&mut self, uid: String, kind: NodeKind, cluster: Option<usize>) {
        if let Some(ix) = cluster {
            self.graph.clusters[ix].nodes.push(uid.clone());
        }
        self.graph.nodes.push(Node { uid, kind });
    }

    fn edge(&mut self, source: &str, target: &str, relation: Relation, label: Option<&str>) {
        self.graph.edges.push(Edge {
            source: source.to_string(),
            target: target.to_string(),
            relation,
            label: label.map(str::to_string),
        });
    }
}

/// The cluster a task owns, if it is a flowable.
fn cluster_kind(task: &Task) -> Option<ClusterKind> {
    Some(match &task.kind {
        TaskKind::Sequential(_) => ClusterKind::Sequential,
        TaskKind::Parallel(_) => ClusterKind::Parallel,
        TaskKind::If(_) => ClusterKind::If,
        TaskKind::Switch(_) => ClusterKind::Switch,
        TaskKind::Dag(_) => ClusterKind::Dag,
        TaskKind::Loop(_) => ClusterKind::Loop,
        TaskKind::LoopUntil(_) => ClusterKind::LoopUntil,
        TaskKind::AllowFailure(_) => ClusterKind::AllowFailure,
        TaskKind::WorkingDirectory(_) => ClusterKind::WorkingDirectory,
        TaskKind::Pause(p) if !p.tasks.is_empty() || !p.errors.is_empty() => ClusterKind::Pause,
        TaskKind::Pause(_) | TaskKind::Subflow(_) | TaskKind::Leaf { .. } => return None,
    })
}

/// The `canonical_type` of a task node: see that field on `NodeKind::Task`.
fn canonical_type(task: &Task) -> String {
    let class = match &task.kind {
        TaskKind::Sequential(_) => "Sequential",
        TaskKind::Parallel(_) => "Parallel",
        TaskKind::If(_) => "If",
        TaskKind::Switch(_) => "Switch",
        TaskKind::Dag(_) => "Dag",
        TaskKind::Loop(_) => "Loop",
        TaskKind::LoopUntil(_) => "LoopUntil",
        TaskKind::Subflow(_) => "Subflow",
        TaskKind::AllowFailure(_) => "AllowFailure",
        TaskKind::WorkingDirectory(_) => "WorkingDirectory",
        TaskKind::Pause(_) => "Pause",
        TaskKind::Leaf { .. } => {
            return crate::spec::registry::known_type(&task.r#type)
                .map(|row| row.r#type.to_string())
                .unwrap_or_else(|| task.r#type.clone());
        }
    };
    format!("io.kestra.plugin.core.flow.{class}")
}

mod expand;
mod states;
pub use expand::{
    execution_graph, expanded_flow_graph, ExecutionSource, FrozenExecution, SubflowSource,
    MAX_EXPANSION_DEPTH, SUBFLOW_SEPARATOR,
};
pub use states::{fold_state, ExecutionGraph, GraphRun, Iterations, NodeState, FOLD_PRIORITY};

#[cfg(test)]
mod tests;
