//! The execution graph's state overlay (spec §4): which run, or which runs,
//! each node of an execution's graph stands for.
//!
//! # Runs to nodes
//!
//! A run names its task by `task_id`; a node by its uid (a validation path).
//! Within ONE flow -- the execution's frozen source, or one expanded child's --
//! a run maps to the FIRST task node, in walk order, whose `task_id` it
//! carries. Task ids are unique within a flow (`duplicate_task_id` is a
//! `validate_for_run` error, so no execution starts from a source that has
//! two), so in practice the map is one-to-one. Should a stored execution ever
//! hold a duplicate, its runs land on the first occurrence (the plain path)
//! and the `~2` node gets no entry: there is nothing in a run that says which
//! occurrence it was.
//!
//! Each flow is mapped inside its own build, BEFORE its graph is spliced into
//! a parent's, so a child's `x` never lands on a parent's `x`; the splice then
//! renames the child's keys with its nodes (`{subflow_uid}/{child_uid}`).
//!
//! # One run, or a fold
//!
//! - A node with exactly ONE run in the execution itself takes that run's
//!   state, attempt count, id and timestamps.
//! - A node whose runs live in `Loop` iterations (child executions linked by
//!   `parent_execution_id` with a `loop_index`, at any depth of nested
//!   loops) is FOLDED: `state` is [`fold_state`] over every run, `run_id` is
//!   null, `attempts` is the most attempts any one run took, `started_at` the
//!   earliest start, `ended_at` the latest end once every run has ended (null
//!   while any has not), and `iterations` counts the iterations the node ran
//!   in, each iteration's runs folded to one state.
//! - A node with several runs in the execution itself (no flowable egret runs
//!   today creates that) is folded the same way, with no `iterations`.
//! - A node with no run has no entry.
//!
//! # Kestra's priority
//!
//! [`fold_state`] picks the state that ranks first in [`FOLD_PRIORITY`].
//! Kestra's list (`ui/packages/topology/src/nodes/TaskNode.vue`, `SORT_STATUS`)
//! is FAILED, KILLED, WARNING, SKIPPED, KILLING, RUNNING, SUCCESS, RESTARTED,
//! CREATED; egret has no KILLING, and five of its states are not on the list
//! at all. They are placed by meaning (see [`FOLD_PRIORITY`]).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{FlowGraph, GraphError, NodeKind};
use crate::executor::RunState;

/// `GET …/executions/{id}/graph`'s answer: the graph of the execution's frozen
/// source, and the state of every node that has a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ExecutionGraph {
    pub graph: FlowGraph,
    /// Node uid -> its state. A node with no run has no entry.
    pub states: BTreeMap<String, NodeState>,
}

/// One node's state (module doc, "One run, or a fold").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NodeState {
    /// A `RunState` wire name (`RUNNING`, `SUCCESS`, ...).
    pub state: String,
    /// The run's attempt count; for a fold, the most any one run took.
    pub attempts: i32,
    /// The run's id; null for a fold.
    pub run_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Set when the runs live in `Loop` iterations.
    pub iterations: Option<Iterations>,
    /// For a `Subflow` node's single run: the newest child execution it
    /// started. Absent for any other node, and for a fold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_execution_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Iterations {
    /// How many iterations this node ran in: `runs.len()`.
    pub total: i32,
    /// Iterations by their folded state's wire name: `runs` counted by `state`.
    pub by_state: BTreeMap<String, i32>,
    /// The node's own run in each iteration it ran in, by `loop_index`. An
    /// iteration where the node never ran has no entry.
    pub runs: Vec<IterationRun>,
}

/// One iteration of the `Loop` the execution ran itself, as one node of its
/// body saw it (W7c Task 6, fix round 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct IterationRun {
    /// The iteration's index in its `Loop`.
    pub loop_index: i32,
    /// THIS NODE's state in the iteration: its runs there, folded by
    /// [`FOLD_PRIORITY`] -- not the iteration's own state, which another body
    /// task can decide.
    pub state: String,
    /// The iteration: the child execution of the graph's execution that ran it.
    pub child_execution_id: Uuid,
    /// The node's run, when the iteration ran it exactly once; null when a
    /// nested `Loop` ran it in several deeper iterations.
    pub run_id: Option<Uuid>,
}

/// One task run, as the graph needs it: no outputs, no history.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GraphRun {
    pub execution_id: Uuid,
    /// The run is in a `Loop` iteration (a loop descendant of the execution
    /// the graph is drawn for), not in that execution itself.
    pub iteration: bool,
    /// For a run in an iteration: the iteration of the graph's execution it
    /// lies under (its depth-1 ancestor, itself at depth 1), and that
    /// iteration's `loop_index`. Null for a run of the execution itself.
    pub iteration_execution_id: Option<Uuid>,
    pub iteration_index: Option<i32>,
    pub run_id: Uuid,
    pub task_id: String,
    /// The wire name, as stored.
    pub state: String,
    pub attempts: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// The newest execution this run started as a `Subflow` (linked by
    /// `trigger_execution_id` + `trigger_run_ix`), if any.
    pub child_execution_id: Option<Uuid>,
}

/// The fold's order, first wins. Kestra's `SORT_STATUS`, with egret's states
/// that it does not list placed by meaning:
///
/// | rank | state | why here |
/// |---|---|---|
/// | 1 | FAILED | Kestra |
/// | 2 | KILLED | Kestra |
/// | 3 | CANCELLED | not in Kestra's list; a terminal that stopped the run, as KILLED did |
/// | 4 | WARNING | Kestra |
/// | 5 | SKIPPED | Kestra |
/// | 6 | RETRIED | not listed; a terminal left by a `CREATE_NEW_EXECUTION` retry, ranked with the terminals above the live states, as Kestra ranks WARNING and SKIPPED |
/// | 7 | PAUSED | not listed; live and waiting on a person, so it shows over a sibling that is merely running (Kestra's KILLING slot, live, sits here too) |
/// | 8 | RETRYING | not listed; live, between attempts |
/// | 9 | RUNNING | Kestra |
/// | 10 | SUBMITTED | not listed; live, handed to a worker but not yet running |
/// | 11 | SUCCESS | Kestra |
/// | 12 | RESTARTED | Kestra |
/// | 13 | CREATED | Kestra |
pub const FOLD_PRIORITY: [RunState; 13] = [
    RunState::Failed,
    RunState::Killed,
    RunState::Cancelled,
    RunState::Warning,
    RunState::Skipped,
    RunState::Retried,
    RunState::Paused,
    RunState::Retrying,
    RunState::Running,
    RunState::Submitted,
    RunState::Success,
    RunState::Restarted,
    RunState::Created,
];

/// `s`'s position in [`FOLD_PRIORITY`]. Exhaustive, so a new `RunState` does
/// not compile until it is placed.
fn rank(s: RunState) -> u8 {
    match s {
        RunState::Failed => 0,
        RunState::Killed => 1,
        RunState::Cancelled => 2,
        RunState::Warning => 3,
        RunState::Skipped => 4,
        RunState::Retried => 5,
        RunState::Paused => 6,
        RunState::Retrying => 7,
        RunState::Running => 8,
        RunState::Submitted => 9,
        RunState::Success => 10,
        RunState::Restarted => 11,
        RunState::Created => 12,
    }
}

/// The state that ranks first in [`FOLD_PRIORITY`] among `states`; `CREATED`
/// (the last) for none.
pub fn fold_state(states: impl Iterator<Item = RunState>) -> RunState {
    states.min_by_key(|s| rank(*s)).unwrap_or(RunState::Created)
}

/// One flow's states, and the child execution each `Subflow` node started.
#[derive(Debug, Default)]
pub(crate) struct LevelStates {
    pub states: BTreeMap<String, NodeState>,
    /// `Subflow` node uid -> the child execution its single run started.
    pub children: HashMap<String, Uuid>,
}

fn parse_state(wire: &str) -> Result<RunState, GraphError> {
    crate::worker::from_wire(wire)
        .ok_or_else(|| GraphError::State(format!("a run's state {wire:?} is not a RunState")))
}

/// One iteration of the graph's execution, its `loop_index`, and one node's
/// runs (with their parsed states) under it.
type IterationRuns<'a> = (Uuid, i32, Vec<(&'a GraphRun, RunState)>);

/// Map `runs` onto `graph`'s task nodes (module doc). `graph` is ONE flow's,
/// not yet spliced into a parent.
pub(crate) fn level_states(
    graph: &FlowGraph,
    runs: &[GraphRun],
) -> Result<LevelStates, GraphError> {
    let mut uid_of: HashMap<&str, &str> = HashMap::new();
    for n in &graph.nodes {
        if let NodeKind::Task { task_id, .. } = &n.kind {
            uid_of.entry(task_id.as_str()).or_insert(n.uid.as_str());
        }
    }
    // Runs by node, in the order the query returned them.
    let mut by_node: BTreeMap<&str, Vec<&GraphRun>> = BTreeMap::new();
    for r in runs {
        if let Some(uid) = uid_of.get(r.task_id.as_str()) {
            by_node.entry(uid).or_default().push(r);
        }
    }

    let mut out = LevelStates::default();
    for (uid, rs) in by_node {
        let looped = rs.iter().any(|r| r.iteration);
        if let ([r], false) = (rs.as_slice(), looped) {
            parse_state(&r.state)?;
            out.states.insert(
                uid.to_string(),
                NodeState {
                    state: r.state.clone(),
                    attempts: r.attempts,
                    run_id: Some(r.run_id),
                    started_at: r.started_at,
                    ended_at: r.ended_at,
                    iterations: None,
                    child_execution_id: r.child_execution_id,
                },
            );
            if let Some(child) = r.child_execution_id {
                out.children.insert(uid.to_string(), child);
            }
            continue;
        }

        let states = rs
            .iter()
            .map(|r| parse_state(&r.state))
            .collect::<Result<Vec<_>, _>>()?;
        let iterations = if looped {
            // Each iteration's runs of this node, folded to one state. An
            // iteration is a child of the graph's execution: a nested Loop's
            // deeper iterations fold into the outer one they ran under.
            let mut per_iteration: Vec<IterationRuns> = Vec::new();
            // Index of each iteration's entry in `per_iteration`, so grouping
            // is linear in the runs rather than quadratic in the iterations.
            let mut slot: HashMap<Uuid, usize> = HashMap::new();
            for (r, s) in rs.iter().zip(&states).filter(|(r, _)| r.iteration) {
                let (Some(it), Some(ix)) = (r.iteration_execution_id, r.iteration_index) else {
                    return Err(GraphError::State(format!(
                        "run {} is in an iteration with no iteration of its own",
                        r.run_id
                    )));
                };
                match slot.get(&it) {
                    Some(&i) => per_iteration[i].2.push((r, *s)),
                    None => {
                        slot.insert(it, per_iteration.len());
                        per_iteration.push((it, ix, vec![(r, *s)]));
                    }
                }
            }
            per_iteration.sort_by_key(|(e, ix, _)| (*ix, *e));
            let runs: Vec<IterationRun> = per_iteration
                .iter()
                .map(|(e, ix, v)| IterationRun {
                    loop_index: *ix,
                    state: fold_state(v.iter().map(|(_, s)| *s)).wire().to_string(),
                    child_execution_id: *e,
                    run_id: match v.as_slice() {
                        [(r, _)] => Some(r.run_id),
                        _ => None,
                    },
                })
                .collect();
            let mut by_state: BTreeMap<String, i32> = BTreeMap::new();
            for r in &runs {
                *by_state.entry(r.state.clone()).or_default() += 1;
            }
            Some(Iterations {
                total: runs.len() as i32,
                by_state,
                runs,
            })
        } else {
            None
        };
        let ended_at = if rs.iter().all(|r| r.ended_at.is_some()) {
            rs.iter().filter_map(|r| r.ended_at).max()
        } else {
            None
        };
        out.states.insert(
            uid.to_string(),
            NodeState {
                state: fold_state(states.into_iter()).wire().to_string(),
                attempts: rs.iter().map(|r| r.attempts).max().unwrap_or(0),
                run_id: None,
                started_at: rs.iter().filter_map(|r| r.started_at).min(),
                ended_at,
                iterations,
                child_execution_id: None,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{flow_graph, GraphOptions};
    use RunState::*;

    const LOG: &str = "io.kestra.plugin.core.log.Log";
    const FLOW: &str = "io.kestra.plugin.core.flow";

    fn graph_of(tasks: &str) -> FlowGraph {
        let yaml = format!("id: f\nnamespace: t\ntasks:\n{tasks}");
        let flow = crate::spec::parse(&yaml).unwrap_or_else(|e| panic!("parse: {e:?}\n{yaml}"));
        flow_graph(&flow, &GraphOptions::default()).expect("graph")
    }

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// A run of `task` in execution `exec`; `top` is `(iteration, loop_index)` for a run under an iteration.
    fn run(exec: u128, top: Option<(u128, i32)>, run: u128, task: &str, state: &str) -> GraphRun {
        GraphRun {
            execution_id: id(exec),
            iteration: top.is_some(),
            iteration_execution_id: top.map(|(e, _)| id(e)),
            iteration_index: top.map(|(_, ix)| ix),
            run_id: id(run),
            task_id: task.into(),
            state: state.into(),
            attempts: 1,
            started_at: None,
            ended_at: None,
            child_execution_id: None,
        }
    }

    /// Body `[a, b, c]` over three iterations (100, 101, 102). Iteration #1 fails
    /// on `c`; iteration #2 fails on `a`, so its `b` and `c` never run. Each
    /// node's per-iteration state is ITS OWN run there, not the iteration's.
    #[test]
    fn each_iteration_lists_the_nodes_own_run_not_the_iterations_state() {
        let g = graph_of(&format!(
            "  - id: each\n    type: {FLOW}.Loop\n    values: '[1, 2, 3]'\n    tasks:\n      \
             - {{id: a, type: {LOG}, message: m}}\n      - {{id: b, type: {LOG}, message: m}}\n      \
             - {{id: c, type: {LOG}, message: m}}\n"
        ));
        // Out of loop order, as rows of several parents can come.
        let runs = vec![
            run(1, None, 1, "each", "FAILED"),
            run(102, Some((102, 2)), 20, "a", "FAILED"),
            run(100, Some((100, 0)), 10, "a", "SUCCESS"),
            run(100, Some((100, 0)), 11, "b", "SUCCESS"),
            run(100, Some((100, 0)), 12, "c", "SUCCESS"),
            run(101, Some((101, 1)), 13, "a", "SUCCESS"),
            run(101, Some((101, 1)), 14, "b", "SUCCESS"),
            run(101, Some((101, 1)), 15, "c", "FAILED"),
        ];
        let out = level_states(&g, &runs).expect("states");

        let a = out.states["tasks.each.a"]
            .iterations
            .clone()
            .expect("a is folded");
        assert_eq!(
            a.runs,
            vec![
                IterationRun {
                    loop_index: 0,
                    state: "SUCCESS".into(),
                    child_execution_id: id(100),
                    run_id: Some(id(10))
                },
                // Iteration #1 failed, on `c`: `a` itself succeeded there.
                IterationRun {
                    loop_index: 1,
                    state: "SUCCESS".into(),
                    child_execution_id: id(101),
                    run_id: Some(id(13))
                },
                IterationRun {
                    loop_index: 2,
                    state: "FAILED".into(),
                    child_execution_id: id(102),
                    run_id: Some(id(20))
                },
            ]
        );
        assert_eq!(a.total, 3);
        assert_eq!(
            a.by_state,
            BTreeMap::from([("SUCCESS".into(), 2), ("FAILED".into(), 1)])
        );

        // `c` never ran in iteration #2: two entries, and the counts agree with them.
        let c = out.states["tasks.each.c"]
            .iterations
            .clone()
            .expect("c is folded");
        assert_eq!(
            c.runs
                .iter()
                .map(|r| (r.loop_index, r.state.as_str()))
                .collect::<Vec<_>>(),
            [(0, "SUCCESS"), (1, "FAILED")]
        );
        assert_eq!(c.total, 2);
        assert_eq!(
            c.by_state,
            BTreeMap::from([("SUCCESS".into(), 1), ("FAILED".into(), 1)])
        );
        assert_eq!(out.states["tasks.each.c"].state, "FAILED");
        // The Loop itself is one run of the execution.
        assert_eq!(out.states["tasks.each"].run_id, Some(id(1)));
    }

    /// An outer Loop over an inner Loop: the inner body's runs sit in deeper
    /// iterations and are listed under the OUTER iteration they ran in, folded.
    #[test]
    fn a_nested_loops_runs_are_listed_under_the_outer_iteration() {
        let g = graph_of(&format!(
            "  - id: outer\n    type: {FLOW}.Loop\n    values: '[1, 2]'\n    tasks:\n      \
             - id: inner\n        type: {FLOW}.Loop\n        values: '[1, 2]'\n        tasks:\n          \
             - {{id: x, type: {LOG}, message: m}}\n"
        ));
        let runs = vec![
            run(1, None, 1, "outer", "FAILED"),
            run(100, Some((100, 0)), 2, "inner", "SUCCESS"),
            run(101, Some((101, 1)), 3, "inner", "FAILED"),
            run(200, Some((100, 0)), 4, "x", "SUCCESS"),
            run(201, Some((100, 0)), 5, "x", "SUCCESS"),
            run(202, Some((101, 1)), 6, "x", "SUCCESS"),
            run(203, Some((101, 1)), 7, "x", "FAILED"),
        ];
        let out = level_states(&g, &runs).expect("states");
        let x = out.states["tasks.outer.inner.x"]
            .iterations
            .clone()
            .expect("x is folded");
        assert_eq!(
            x.runs,
            vec![
                IterationRun {
                    loop_index: 0,
                    state: "SUCCESS".into(),
                    child_execution_id: id(100),
                    run_id: None
                },
                IterationRun {
                    loop_index: 1,
                    state: "FAILED".into(),
                    child_execution_id: id(101),
                    run_id: None
                },
            ]
        );
        assert_eq!(x.total, 2);
        let inner = out.states["tasks.outer.inner"]
            .iterations
            .clone()
            .expect("inner is folded");
        assert_eq!(inner.runs[1].run_id, Some(id(3)));
    }

    #[test]
    fn fold_priority_lists_every_run_state_once_in_rank_order() {
        assert_eq!(FOLD_PRIORITY.len(), RunState::ALL.len());
        for s in RunState::ALL {
            assert_eq!(FOLD_PRIORITY[rank(*s) as usize], *s, "{s:?}");
        }
    }

    #[test]
    fn fold_state_follows_kestras_priority() {
        let table: &[(&[RunState], RunState)] = &[
            // Kestra's own list, pairwise, each beating every one after it.
            (&[Success, Failed], Failed),
            (&[Failed, Killed], Failed),
            (&[Warning, Killed], Killed),
            (&[Skipped, Warning], Warning),
            (&[Running, Skipped], Skipped),
            (&[Success, Running], Running),
            (&[Running, Success], Running),
            (&[Restarted, Success], Success),
            (&[Created, Restarted], Restarted),
            (&[Success, Success, Success], Success),
            (&[Success, Failed, Success], Failed),
            // egret's states Kestra does not list.
            (&[Killed, Cancelled], Killed),
            (&[Warning, Cancelled], Cancelled),
            (&[Retried, Skipped], Skipped),
            (&[Paused, Retried], Retried),
            (&[Running, Paused], Paused),
            (&[Retrying, Paused], Paused),
            (&[Running, Retrying], Retrying),
            (&[Submitted, Running], Running),
            (&[Success, Submitted], Submitted),
            (&[Created, Success], Success),
            (&[Created], Created),
            (&[], Created),
        ];
        for (states, want) in table {
            assert_eq!(
                fold_state(states.iter().copied()),
                *want,
                "fold of {states:?}"
            );
        }
        // Every state beats every state ranked after it, in both orders.
        for (i, a) in FOLD_PRIORITY.iter().enumerate() {
            for b in &FOLD_PRIORITY[i + 1..] {
                assert_eq!(fold_state([*a, *b].into_iter()), *a);
                assert_eq!(fold_state([*b, *a].into_iter()), *a);
            }
        }
    }
}
