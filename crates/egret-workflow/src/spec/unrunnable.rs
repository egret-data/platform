//! `unrunnable`: the tasks of a flow this deployment has no handler for
//! (no-silent-stall spec §4.2). Pure: the deployment comes in as a
//! [`Served`] argument, so `validate_for_run` and `build_plan` stay
//! deployment-free.

use crate::models::Flow;
use crate::runtime::Served;
use crate::spec::Finding;

/// One `task_type_not_served` error per task `build_plan` would make a
/// runnable whose type `served` lacks, at the task's validation path.
///
/// **Which tasks it judges.** Every `models::TaskKind::Leaf` in `tasks`,
/// `errors`, `finally` and `afterExecution`, through `children()` and
/// `error_children()` (`plugins::leaves`, the walk `validate_plugin_tasks`
/// and its paths), whose type is KNOWN: a `spec::registry::TASK_TYPES`
/// spelling (`known_type`), or a `kestra.<FQCN>` of the embedded catalog
/// (`plugins::lookup`). Every other `Leaf` is already an error of
/// `validate_for_run` by its type -- `unknown_task_type`,
/// `unsupported_flow_task`/`unsupported_foreach` (spelled like control
/// flow), or `validate_plugin_tasks`' type refusal (a bare catalog FQCN, a
/// `kestra.` type outside the catalog) -- so it is not reported twice. The
/// typed flowables of §3 (`AllowFailure`, `WorkingDirectory`, ...) are not
/// `Leaf`s and are not judged here.
pub fn unrunnable(flow: &Flow, served: &Served) -> Vec<Finding> {
    crate::plugins::leaves(flow)
        .into_iter()
        .filter(|(_, task, _)| is_known(&task.r#type) && !served.serves(&task.r#type))
        .map(|(path, task, _)| Finding {
            code: "task_type_not_served",
            message: message(&task.id, &task.r#type),
            path,
        })
        .collect()
}

/// A type `validate_for_run` lets through, so one the plan makes runnable.
fn is_known(type_id: &str) -> bool {
    crate::spec::registry::known_type(type_id).is_some()
        || crate::plugins::lookup(type_id).is_some()
}

/// Spec §4.2's two classes: a script or `kestra.` type needs a container
/// runner; any other type needs a handler this deployment does not wire.
/// Neither blames the flow.
fn message(task_id: &str, type_id: &str) -> String {
    let why = if crate::scripts::kind_of(type_id).is_some()
        || crate::plugins::lookup(type_id).is_some()
    {
        format!(
            "this egret deployment has no container runner (`workflow.container_runner`), so \
             it cannot run `{type_id}`"
        )
    } else {
        format!("this egret deployment has no handler for `{type_id}`, so it cannot run it")
    };
    format!(
        "task '{task_id}': {why}. The flow cannot be started here; this is not an error in \
         the flow"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::parse;

    fn served_none() -> Served {
        crate::runtime::served_for(&egret_config::Config::default().workflow)
    }

    fn findings(src: &str) -> Vec<Finding> {
        unrunnable(&parse(src).expect("parses"), &served_none())
    }

    fn paths(fs: &[Finding]) -> Vec<(&'static str, &str)> {
        fs.iter().map(|f| (f.code, f.path.as_str())).collect()
    }

    #[test]
    fn an_unserved_known_type_is_task_type_not_served_at_its_path() {
        let fs = findings("id: f\nnamespace: a.b\ntasks:\n  - id: q\n    type: egret.sql\n");
        assert_eq!(paths(&fs), vec![("task_type_not_served", "tasks.q")]);
    }

    #[test]
    fn nested_in_errors_finally_and_a_dag_is_found_at_each_path() {
        let src = "\
id: f
namespace: a.b
tasks:
  - id: seq
    type: io.kestra.plugin.core.flow.Sequential
    tasks:
      - id: ok
        type: io.kestra.plugin.core.log.Log
        message: hi
    errors:
      - id: h1
        type: core.http
  - id: dag
    type: io.kestra.plugin.core.flow.Dag
    tasks:
      - task:
          id: h3
          type: core.http
finally:
  - id: h2
    type: core.http
";
        let mut got: Vec<(&str, String)> = findings(src)
            .into_iter()
            .map(|f| (f.code, f.path))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("task_type_not_served", "finally.h2".to_string()),
                ("task_type_not_served", "tasks.dag.h3".to_string()),
                ("task_type_not_served", "tasks.seq.h1".to_string()),
            ]
        );
    }

    #[test]
    fn a_served_type_passes() {
        let fs = findings(
            "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        );
        assert!(fs.is_empty(), "{fs:?}");
    }

    /// §3 owns an unknown type (`unknown_task_type`), so it is reported once.
    /// So do a bare catalog FQCN and a `kestra.` type outside the catalog
    /// (`validate_plugin_tasks`), and a type spelled like Kestra control flow
    /// (`unsupported_flow_task`). Each premise is checked: `validate_for_run`
    /// does refuse each of them at the same path, under the code named.
    #[test]
    fn a_type_validate_refuses_by_type_gives_nothing() {
        for (ty, code) in [
            ("egret.sqll", "unknown_task_type"),
            (
                "io.kestra.plugin.jdbc.trino.Query",
                crate::plugins::CODE_TYPE_UNPREFIXED,
            ),
            ("kestra.io.example.Nope", crate::plugins::CODE_TYPE_UNKNOWN),
            ("core.somenewgate", "unsupported_flow_task"),
        ] {
            let flow = parse(&format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: {ty}\n"
            ))
            .expect("parses");
            let refused = crate::spec::validate_for_run(&flow);
            assert!(
                refused
                    .errors
                    .iter()
                    .any(|e| e.code == code && e.path.starts_with("tasks.t")),
                "{ty}: {refused:?}"
            );
            let fs = unrunnable(&flow, &served_none());
            assert!(fs.is_empty(), "{ty}: {fs:?}");
        }
    }

    #[test]
    fn a_script_or_plugin_type_names_the_runner_and_any_other_names_the_handler() {
        let src = "\
id: f
namespace: a.b
tasks:
  - id: sh
    type: io.kestra.plugin.scripts.shell.Commands
    commands: [echo hi]
  - id: alias
    type: egret.container
    commands: [echo hi]
  - id: pg
    type: kestra.io.kestra.plugin.jdbc.postgresql.Query
    sql: select 1
  - id: q
    type: egret.sql
";
        let fs = findings(src);
        assert_eq!(fs.len(), 4, "{fs:?}");
        for f in &fs[..3] {
            assert_eq!(f.code, "task_type_not_served");
            assert!(
                f.message
                    .contains("no container runner (`workflow.container_runner`)"),
                "{f:?}"
            );
        }
        let sql = &fs[3];
        assert_eq!(sql.path, "tasks.q");
        assert!(
            sql.message.contains("has no handler for `egret.sql`"),
            "{sql:?}"
        );
        for f in &fs {
            assert!(f.message.contains("cannot be started here"), "{f:?}");
        }
    }
}
