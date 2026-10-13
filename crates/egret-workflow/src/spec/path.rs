//! The ONE spelling of a task's path -- the `path` a validation finding
//! carries and the `uid` a graph node carries (`graph`, W7c spec §3.1).
//!
//! A root task is `{list}.{id}` for the four root lists, a nested task is
//! `{parent}.{id}`, whichever of the parent's lists (`tasks`, `then`, a
//! `Switch` case, a `Dag` node, its own `errors:`) holds it. `spec::validate`,
//! `plugins::leaves`, `scripts::script_tasks` and `graph` all build paths
//! through these two functions, so a finding and a node name the same place.

use crate::models::{Flow, Task};

/// The flow's four root lists, in Kestra's `Flow.allTasks()` order, each
/// with the name its paths start with.
pub(crate) fn root_lists(flow: &Flow) -> [(&'static str, &[Task]); 4] {
    [
        ("tasks", &flow.tasks),
        ("errors", &flow.errors),
        ("finally", &flow.finally_),
        ("afterExecution", &flow.after_execution),
    ]
}

/// The path of the task `id` under `prefix` -- a root list's name or the
/// parent task's own path.
pub(crate) fn task_path(prefix: &str, id: &str) -> String {
    format!("{prefix}.{id}")
}
