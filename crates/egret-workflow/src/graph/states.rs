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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Iterations {
    /// How many iterations this node ran in.
    pub total: i32,
    /// Iterations by their folded state's wire name.
    pub by_state: BTreeMap<String, i32>,
}

/// One task run, as the graph needs it: no outputs, no history.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GraphRun {
    pub execution_id: Uuid,
    /// The run is in a `Loop` iteration (a loop descendant of the execution
    /// the graph is drawn for), not in that execution itself.
    pub iteration: bool,
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
            // Each iteration's runs, folded to one state.
            let mut per_iteration: Vec<(Uuid, Vec<RunState>)> = Vec::new();
            for (r, s) in rs.iter().zip(&states).filter(|(r, _)| r.iteration) {
                match per_iteration.iter_mut().find(|(e, _)| *e == r.execution_id) {
                    Some((_, v)) => v.push(*s),
                    None => per_iteration.push((r.execution_id, vec![*s])),
                }
            }
            let mut by_state: BTreeMap<String, i32> = BTreeMap::new();
            for (_, v) in &per_iteration {
                *by_state
                    .entry(fold_state(v.iter().copied()).wire().to_string())
                    .or_default() += 1;
            }
            Some(Iterations {
                total: per_iteration.len() as i32,
                by_state,
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
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use RunState::*;

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
