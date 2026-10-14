//! The ONE spelling of a task's path -- the `path` a validation finding
//! carries and the `uid` a graph node carries (`graph`, W7c spec §3.1).
//!
//! A root task is `{list}.{id}` for the four root lists, a nested task is
//! `{parent}.{id}`, whichever of the parent's lists (`tasks`, `then`, a
//! `Switch` case, a `Dag` node, its own `errors:`) holds it. `spec::validate`,
//! `plugins::leaves`, `scripts::script_tasks` and `graph` all build paths
//! through these two functions, so a finding and a node name the same place.
//!
//! [`every_task`] is the one walk over the whole tree; `plugins::leaves`,
//! `scripts::script_tasks` and the namespace dependencies (W7d) filter it.

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

/// Every task in `flow` with its path, in all four root lists and at every
/// depth -- through [`Task::children`] **and** [`Task::error_children`], so a
/// task inside a flowable's `errors:` is visited. Pre-order: a parent before
/// its children, `children()` before `error_children()`.
pub(crate) fn every_task(flow: &Flow) -> Vec<(String, &Task)> {
    fn walk<'a>(task: &'a Task, path: String, out: &mut Vec<(String, &'a Task)>) {
        let children: Vec<&Task> = task
            .children()
            .into_iter()
            .chain(task.error_children())
            .collect();
        out.push((path.clone(), task));
        for child in children {
            walk(child, task_path(&path, &child.id), out);
        }
    }
    let mut out = Vec::new();
    for (name, list) in root_lists(flow) {
        for task in list {
            walk(task, task_path(name, &task.id), &mut out);
        }
    }
    out
}
