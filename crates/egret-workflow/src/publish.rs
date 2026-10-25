//! `publish` — the ordered walk that turns a draft into a frozen version, and
//! the resolution of every reference a flow makes to another egret object.
//!
//! The order is the design (spec section 8, Task 6 brief step 4):
//! 1. load the draft, scoped to `ws` (`FlowRepo::get` -- `NotFound` for a
//!    missing or foreign flow, the same status every refusal below answers
//!    with);
//! 2. `parse(source)` -- a parse failure is `InvalidInput`;
//! 3. `validate(&flow)`, plus `spec::unrunnable` against this deployment's
//!    `runtime::served()` set (`task_type_not_served`) -- any **error** is
//!    `InvalidInput`; warnings are carried into the snapshot but never block
//!    publish (a flow declaring `concurrency` must still publish -- it is
//!    simply not enforced yet);
//!    3b. (W5a) every script task passes the ONE governance gate,
//!    `scripts::gate::check` -- the workspace's `workflow_containers`
//!    feature, the runner's `allowed-images` -- or publish is `InvalidInput`
//!    naming the refusal. See [`ScriptGate`] and [`check_script_tasks`];
//!    3c. (W5b) every Kestra plugin task (`type: kestra.<FQCN>`) passes ITS
//!    one gate, `plugins::gate::check` -- `workflow_kestra_plugins` AND
//!    `workflow_containers`, a configured runner, the `kestra` entry's
//!    `allowed-types`. See [`check_plugin_tasks`];
//! 4. walk every task; `registry::known_type(&task.r#type)` -- `None` is
//!    `InvalidInput`, naming the type, so a typo like `egret.sqll` fails to
//!    publish instead of resolving vacuously and failing only at run time,
//!    in W3, with no author present. A `core.subflow` task never reaches this
//!    check at all -- it is a typed `TaskKind::Subflow`, not a `Leaf`, and is
//!    resolved by its own arm below instead (fix round 1). (W5b) A type
//!    `plugins::lookup` resolves (a `kestra.<FQCN>` in the catalog) is
//!    accepted too; a plugin task has no `RefField`s;
//! 5. for each of that type's declared `RefField`s, read the leaf's own
//!    field and ask `refs` which workspace owns it, refusing **both** an
//!    absent object and one owned by a *different* workspace as the
//!    identical `NotFound` -- see [`ref_not_found`]. A `core.subflow` task's
//!    `namespace`/`flowId` pair is NOT resolved through `refs`: since debt
//!    spec §5.1 (2026-10-19) it is looked up in the caller's workspace only,
//!    `FlowRepo::find_in_workspace`. Found, its id is frozen; absent -- held
//!    by no workspace, or only by another one, the two answering alike so
//!    publish probes nothing -- the flow still publishes, as Kestra's does,
//!    with no frozen id and Kestra's warning, `The subflow '<flowId>' not
//!    found in namespace '<namespace>'.`, in the snapshot's `warnings`. The
//!    run fails later, at `worker::subflow::resolve`;
//! 6. rewrite the resolved id into the plan in place;
//! 7. walk `flow.triggers`: a `core.trigger.flow`'s `dependsOn` list names
//!    OTHER flows by the identical `(namespace, flowId)` pair a
//!    `core.subflow` task does, and is resolved through the exact same
//!    scoped lookup, same frozen-id shape -- [`resolve_trigger_dependencies`].
//!    An absent dependency publishes with no frozen id and no warning
//!    (Kestra warns about none). Whole-branch review finding 2: this
//!    step did not exist before that review -- `publish` walked
//!    `tasks`/`errors`/`finally`/`afterExecution` and never `flow.triggers`
//!    at all, so a trigger's `dependsOn` froze into the snapshot unresolved.
//!    Same defect Task 6 already fixed once for `TaskKind`s; `TriggerKind`
//!    was outside that sweep;
//! 8. `insert_version` with a snapshot of `{ source, plan (resolved),
//!    warnings }`;
//! 9. `set_live`.
//!
//! Steps 5, 6 and 7 are the whole point: after them, the snapshot contains
//! no name that still has to be looked up -- save a `Subflow` or
//! `dependsOn` target the caller's workspace does not hold (debt §5.1),
//! which carries no id and is looked up by name, in the execution's own
//! workspace, at run time, as every `Subflow` target already is
//! (`worker::subflow::resolve` never reads the frozen id). Migration 0128 is the recorded
//! case of skipping this -- a plain FK with no workspace predicate, and a
//! Cedar check that was told the CALLER's workspace, never the object's. A
//! flow is a machine for creating more of those unless every reference it
//! makes is checked HERE, before anything is frozen -- "every reference"
//! meaning every place ANY model type names another flow, not just the ones
//! reachable from `Task::children()`.
//!
//! # Which `FlowRepo` calls this makes are unscoped, and what makes them safe
//!
//! `FlowRepo::get` (step 1) and `FlowRepo::find_in_workspace` (steps 5 and
//! 7, debt §5.1) are the only READs this function does, and both ARE scoped
//! by `ws`. `get` is `WHERE id = $1 AND workspace_id = $2`, `NotFound` for a
//! missing or foreign flow. `insert_version(flow, ...)` (step 8) and
//! `set_live(flow, version)` (step 9) both key off `flow` alone, with no
//! workspace predicate of their own (each says so in its own doc comment on
//! `FlowRepo`), so calling either with an unvalidated `flow` would write into
//! -- or make live -- a row this function never confirmed belongs to `ws`.
//! What makes both calls here safe is exactly step 1: `flow` only reaches
//! either of them after `repo.get(ws, flow)` has already returned `Ok`, which
//! is only possible when `flow` names a row in `ws`.
//!
//! `FlowRepo::list_versions` and `FlowRepo::get_version` are not called by
//! this function at all; as of Task 6 fix round 2, that no longer leaves an
//! obligation for a future caller to repeat step 1's pattern for -- both now
//! take `ws` and check it themselves (see their own doc comments on
//! `FlowRepo`).

use std::future::Future;
use std::pin::Pin;

use egret_core::{Error, Result, UserId};
use uuid::Uuid;

use crate::models::{Flow, Task, TaskKind, Trigger, TriggerKind};
use crate::plugins;
use crate::repo::{FlowRepo, FlowVersionRow};
use crate::scripts::{self, RunnerKind};
use crate::spec::path::task_path;
use crate::spec::{parse, registry, validate, Finding};
use egret_config::PluginConfigurations;
use egret_workflow_api::WorkflowRefPort;

/// The single, shared answer to "this reference does not resolve in the
/// caller's workspace" -- whether `refs` never heard of the name at all, or
/// the object it names belongs to someone else. Both cases return exactly
/// this value: same variant, same message. See
/// `a_flow_referencing_another_workspace_cannot_be_published` and
/// `an_unknown_reference_fails_the_same_way_as_a_foreign_one` -- a
/// distinguishable response here would turn publish into a probe for what
/// exists in other workspaces.
fn ref_not_found() -> Error {
    Error::NotFound("referenced object".into())
}

/// What publish's script-task gate (step 3b) decides with: the caller's
/// workspace's `workflow_containers` state and the platform's
/// `plugin_configurations`, plus the runner a script task with no
/// `taskRunner:` runs on.
#[derive(Debug, Clone, Copy)]
pub struct ScriptGate<'a> {
    /// `workflow_containers` is ON for the workspace being published into.
    pub feature_enabled: bool,
    pub plugin_configurations: &'a PluginConfigurations,
    /// The configured default runner (W5a spec §2: "absent: the configured
    /// `container_runner`"). `None` when none is configured, in which case a
    /// script task with no `taskRunner:` is refused -- nothing would run it.
    pub default_runner: Option<RunnerKind>,
    /// (W5b) `workflow_kestra_plugins` is ON for the workspace. A plugin task
    /// needs this AND [`ScriptGate::feature_enabled`] (`workflow_containers`)
    /// AND a [`ScriptGate::default_runner`] -- see [`check_plugin_tasks`].
    pub kestra_plugins_enabled: bool,
}

static NO_PLUGIN_CONFIGURATIONS: PluginConfigurations = PluginConfigurations(Vec::new());

impl ScriptGate<'static> {
    /// Fail-closed: the feature off, no allowlist, no default runner -- every
    /// script task is refused. What [`publish`] (the no-gate entry point)
    /// uses, so a caller that never heard of W5a cannot publish a container.
    pub fn closed() -> Self {
        ScriptGate {
            feature_enabled: false,
            plugin_configurations: &NO_PLUGIN_CONFIGURATIONS,
            default_runner: None,
            kestra_plugins_enabled: false,
        }
    }
}

/// Step 3b: every script task in `flow` through the ONE gate,
/// `scripts::gate::check` -- or, for an image written as a `{{ }}`
/// expression, `gate::check_unrendered` (the same rules minus the image
/// match, which runs after rendering, W5a Task 5). The first refusal is
/// `InvalidInput`, naming the task and the reason.
///
/// Runs after `validate`, so every script task here already parses; a parse
/// error is still answered (not unwrapped) in case a caller skipped step 3.
pub fn check_script_tasks(flow: &Flow, gate: &ScriptGate<'_>) -> Result<()> {
    for (_path, task, _kind, spec) in scripts::script_tasks(flow) {
        let parsed = scripts::parse(&task.r#type, spec).map_err(|refusals| {
            Error::InvalidInput(
                refusals
                    .iter()
                    .map(|r| format!("task '{}': {}", task.id, r.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        let refuse = |why: scripts::gate::GateRefusal| {
            Error::InvalidInput(format!("task '{}': {why}", task.id))
        };
        // The feature first, so a workspace with it off is told that --
        // whatever else about the task would also be refused.
        if !gate.feature_enabled {
            return Err(refuse(scripts::gate::GateRefusal::FeatureDisabled));
        }
        let runner = match (&parsed.task_runner, gate.default_runner) {
            (Some(r), _) => r.kind,
            (None, Some(default)) => default,
            (None, None) => {
                return Err(Error::InvalidInput(format!(
                    "task '{}': no 'taskRunner' and no default container runner is \
                     configured, so nothing would run it",
                    task.id
                )))
            }
        };
        let image = parsed.image();
        let verdict = if scripts::gate::is_unrendered(image) {
            scripts::gate::check_unrendered(
                gate.feature_enabled,
                runner,
                gate.plugin_configurations,
            )
        } else {
            scripts::gate::check(&scripts::gate::GateInput {
                feature_enabled: gate.feature_enabled,
                image,
                runner,
                plugin_configurations: gate.plugin_configurations,
            })
        };
        verdict.map_err(refuse)?;
        // An explicit `taskRunner` must be the runner this deployment HAS
        // (W5a Task 5 fix round 1). Without this a flow naming the other
        // runner published and failed every run -- and with
        // `container_runner = none` it published a task nothing is
        // registered for, which `WorkerError::NoHandler` re-claimed for ever
        // (since no-silent-stall Task 4 it fails the run instead).
        match gate.default_runner {
            None => {
                return Err(Error::InvalidInput(format!(
                    "task '{}': no container runner is configured on this egret \
                     (workflow.container_runner is none), so nothing would run it",
                    task.id
                )))
            }
            Some(configured) if configured != runner => {
                return Err(Error::InvalidInput(format!(
                    "task '{}': {}",
                    task.id,
                    scripts::runner_unavailable(runner, configured)
                )))
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Step 3c (W5b): every Kestra plugin task in `flow` (`plugins::plugin_tasks`,
/// which walks `errors:` too) through the ONE plugin gate,
/// `plugins::gate::check`. The first refusal is `InvalidInput`, naming the
/// task and the reason.
///
/// Runs after `validate`, so every plugin task here already parses; a parse
/// error is still answered (not unwrapped) in case a caller skipped step 3.
pub fn check_plugin_tasks(flow: &Flow, gate: &ScriptGate<'_>) -> Result<()> {
    for (_path, task, spec) in plugins::plugin_tasks(flow) {
        let parsed = plugins::parse(&task.r#type, spec).map_err(|refusals| {
            Error::InvalidInput(
                refusals
                    .iter()
                    .map(|r| format!("task '{}': {}", task.id, r.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        plugins::gate::check(&plugins::gate::GateInput {
            feature_enabled: gate.kestra_plugins_enabled,
            containers_enabled: gate.feature_enabled,
            plugin_configurations: gate.plugin_configurations,
            runner: gate.default_runner,
            fqcn: parsed.fqcn,
        })
        .map_err(|why| Error::InvalidInput(format!("task '{}': {why}", task.id)))?;
    }
    Ok(())
}

/// Publish `flow`'s current draft: parse, validate, resolve every reference,
/// freeze a new immutable version, and mark it live. See the module doc for
/// the ordered steps and where each refusal happens.
///
/// Script tasks are gated with [`ScriptGate::closed`], i.e. refused: this
/// entry point knows neither the workspace's `workflow_containers` state nor
/// the platform allowlist. The route calls [`publish_gated`].
pub async fn publish(
    repo: &FlowRepo,
    refs: &dyn WorkflowRefPort,
    ws: Uuid,
    by: UserId,
    flow: Uuid,
    note: &str,
) -> Result<FlowVersionRow> {
    publish_gated(repo, refs, ws, by, flow, note, &ScriptGate::closed()).await
}

/// [`publish`], with the script-task gate's inputs (step 3b).
pub async fn publish_gated(
    repo: &FlowRepo,
    refs: &dyn WorkflowRefPort,
    ws: Uuid,
    by: UserId,
    flow: Uuid,
    note: &str,
    gate: &ScriptGate<'_>,
) -> Result<FlowVersionRow> {
    // Step 1: `FlowRepo::get` is scoped by `ws` -- a missing OR foreign flow
    // both answer `NotFound` already, the module doc's module-level
    // convention (`repo.rs`'s own comment on the same point).
    let draft = repo.get(ws, flow).await?;

    // Step 2.
    let mut parsed: Flow = parse(&draft.source).map_err(|e| {
        Error::InvalidInput(match (e.line, e.column) {
            (Some(l), Some(c)) => format!("{} (line {l}, column {c})", e.message),
            _ => e.message,
        })
    })?;

    // Step 3: errors block, warnings are carried through to the snapshot
    // below untouched. A task type this deployment does not serve is one of
    // the errors (no-silent-stall §4.3 point 3), found here, before 3b/3c:
    // with no runner, a script or plugin task is refused once, as
    // `task_type_not_served`, and the gates' runner refusals stay for a
    // deployment that has a runner.
    let mut report = validate(&parsed);
    report
        .errors
        .extend(crate::spec::unrunnable(&parsed, crate::runtime::served()));
    report
        .warnings
        .extend(crate::spec::property_warnings(&draft.source));
    if !report.errors.is_empty() {
        return Err(Error::InvalidInput(crate::spec::describe(&report.errors)));
    }

    // Step 3b (W5a).
    check_script_tasks(&parsed, gate)?;
    // Step 3c (W5b).
    check_plugin_tasks(&parsed, gate)?;

    // Steps 4-6: walk every task in all four root lists (mirrors
    // `spec::validate`'s own walk over `tasks`/`errors`/`finally`/
    // `afterExecution`), refusing an unknown type and resolving + rewriting
    // every declared reference in place.
    let cx = Cx { ws, repo, refs };
    for (name, list) in [
        ("tasks", &mut parsed.tasks),
        ("errors", &mut parsed.errors),
        ("finally", &mut parsed.finally_),
        ("afterExecution", &mut parsed.after_execution),
    ] {
        resolve_all(list, name.to_string(), cx, &mut report.warnings).await?;
    }

    // Step 7: see the module doc and `resolve_trigger_dependencies`'s own
    // comment.
    resolve_trigger_dependencies(&mut parsed.triggers, ws, repo).await?;

    // Step 8: self-contained on purpose -- W3's executor (and a reader of a
    // published version) must never need to re-read the draft or re-resolve
    // a name. `warnings` rides along so a declared-but-unimplemented feature
    // stays visible on the version that shipped it, not just on the draft
    // that happened to be live when someone last looked.
    let snapshot = serde_json::json!({
        "source": draft.source,
        "plan": parsed,
        "warnings": report.warnings,
    });
    let version = repo.insert_version(flow, by, &snapshot, note).await?;

    // Step 9.
    repo.set_live(flow, version.id).await?;

    Ok(version)
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Resolve every task in `tasks`, in order. A plain `for` loop over an
/// `async fn` would do, except each task recurses back into this same
/// function through [`resolve_task`] for its own children, and Rust has no
/// native support for recursive `async fn`s -- hence the explicit boxing
/// both functions share.
///
/// `parent` is the path `tasks` hang under -- a root list's name or the
/// owning task's own path -- so a warning names the task the way
/// `spec::validate`'s findings do (`spec::path`).
fn resolve_all<'a>(
    tasks: &'a mut [Task],
    parent: String,
    cx: Cx<'a>,
    warnings: &'a mut Vec<Finding>,
) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        for task in tasks.iter_mut() {
            let path = task_path(&parent, &task.id);
            resolve_task(task, path, cx, warnings).await?;
        }
        Ok(())
    })
}

/// What every step of the task walk resolves with: the caller's workspace,
/// the `FlowRepo` a `Subflow` pair is looked up in (scoped to `ws`), and the
/// port every `Leaf` `RefField` is resolved through.
#[derive(Clone, Copy)]
struct Cx<'a> {
    ws: Uuid,
    repo: &'a FlowRepo,
    refs: &'a dyn WorkflowRefPort,
}

/// Resolve one task's own references (if it is a `Leaf`, refusing an unknown
/// type first -- step 4) and recurse into its children exactly as
/// `Task::children()` walks them, but mutably: this is what lets step 6
/// ("rewrite the resolved fields into the plan") reach every nesting depth,
/// not just a flow's top-level lists.
fn resolve_task<'a>(
    task: &'a mut Task,
    path: String,
    cx: Cx<'a>,
    warnings: &'a mut Vec<Finding>,
) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        let ws = cx.ws;
        let parent = path.clone();
        let own: Result<()> = async {
            match &mut task.kind {
                // Control flow names no egret object of its own; its children
                // are walked below, through `Task::children_mut`.
                TaskKind::Sequential(_)
                | TaskKind::Parallel(_)
                | TaskKind::If(_)
                | TaskKind::Switch(_)
                | TaskKind::Dag(_)
                | TaskKind::Loop(_)
                | TaskKind::LoopUntil(_)
                | TaskKind::AllowFailure(_)
                | TaskKind::WorkingDirectory(_)
                | TaskKind::Pause(_) => Ok(()),
                // Fix round 1: a subflow spawns a DIFFERENT flow's execution
                // rather than owning local tasks -- `Task::children()` returns
                // none for it either, and `registry::known_type` never sees its
                // type string (it dispatches to this typed variant before ever
                // reaching a `Leaf`), so it is not subject to step 4's
                // unknown-type refusal. That is also WHY resolution lives here,
                // in this typed arm, rather than as a `RefField`/`TASK_TYPES`
                // entry: `RefField` is read out of a `Leaf`'s opaque `spec` map
                // by `registry::known_type`, a path a `Subflow` structurally
                // never reaches (`kind_for` in `task.rs` dispatches
                // `core.subflow`/its FQCN straight to this variant, before any
                // `Leaf` match arm exists to consult the registry) -- an entry
                // in `TASK_TYPES` for `core.subflow` would be dead: no code path
                // would ever look it up. `spec::validate`'s own Subflow checks
                // (`subflow_namespace_required`/`subflow_flow_id_required`/
                // `self_subflow`) already made this same choice, in its own
                // typed `TaskKind::Subflow` arm rather than a table.
                //
                // `namespace`/`flow_id` are `Option<String>` (Task 4's bean-
                // validation-not-parsing debt), but `validate` (step 3, above)
                // already refused this flow with `subflow_namespace_required`/
                // `subflow_flow_id_required` if either were `None` -- so by the
                // time this arm runs, both are guaranteed `Some` for any
                // `Subflow` that reaches it. The `let...else` is defensive, not
                // load-bearing: it cannot actually fire.
                //
                // Debt §5.1 (2026-10-19): the pair is looked up in the caller's
                // workspace ONLY (`FlowRepo::find_in_workspace`) -- the
                // unscoped `RefKind::Subflow` port it used is gone -- and an absent target is
                // not refused. Kestra publishes it with a warning
                // (`FlowService.java:619-648`), and so does egret, with the same
                // text. A flow only another workspace holds is "absent" here,
                // byte for byte, so the answer reveals nothing about any other
                // workspace -- the property `ref_not_found` gave the `Leaf` arm,
                // kept without the refusal. The run fails later, at
                // `worker::subflow::resolve`, which looks the target up in the
                // execution's workspace whatever was frozen.
                TaskKind::Subflow(s) => {
                    let (Some(namespace), Some(flow_id)) = (&s.namespace, &s.flow_id) else {
                        return Ok(());
                    };
                    let found = cx.repo.find_in_workspace(ws, namespace, flow_id).await?;
                    if found.is_none() {
                        warnings.push(subflow_not_found(namespace, flow_id, path));
                    }
                    // `namespace`/`flow_id` are kept, unlike a `Leaf`'s single
                    // reference field -- see `Subflow::flow_ref`'s own doc
                    // comment in `models/task.rs` for why this is a third field
                    // rather than an overwrite of either existing one. Written
                    // even when `None`: `flowRef` deserialises from a draft, and
                    // an id the AUTHOR wrote must not reach the snapshot.
                    s.flow_ref = found;
                    Ok(())
                }
                TaskKind::Leaf { spec } => {
                    let Some(type_spec) = registry::known_type(&task.r#type) else {
                        // W5b: a catalog plugin type is known too. It names no
                        // egret object, so there is nothing to resolve; step 3c
                        // already gated it.
                        if plugins::lookup(&task.r#type).is_some() {
                            return Ok(());
                        }
                        return Err(Error::InvalidInput(format!(
                            "task '{}' has unknown type '{}'",
                            task.id, task.r#type
                        )));
                    };
                    for rf in type_spec.references {
                        let value = spec
                            .get(rf.field)
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                Error::InvalidInput(format!(
                                    "task '{}': field '{}' is required to resolve its reference",
                                    task.id, rf.field
                                ))
                            })?
                            .to_string();

                        let resolved = cx.refs.workspace_of(rf.kind, &value).await?;
                        let id = match resolved {
                            // Steps 5's whole point: `None` (never heard of it)
                            // and `Some((_, other_ws))` (heard of it, but it is
                            // someone else's) take the SAME branch below.
                            Some((id, owner)) if owner == ws => id,
                            _ => return Err(ref_not_found()),
                        };

                        if let Some(obj) = spec.as_object_mut() {
                            obj.insert(
                                rf.field.to_string(),
                                serde_json::Value::String(id.to_string()),
                            );
                        }
                    }
                    Ok(())
                }
            }
        }
        .await;
        own?;
        // The control-flow children, exactly `Task::children()`'s lists in
        // its order: `children_mut` is its mutable twin, checked against it
        // by `children_mut_walks_exactly_what_children_walks`. This walk used
        // its own per-kind arms until PR-2's final review (M1), and they had
        // drifted: `AllowFailure.finally` was never walked, so a `Subflow`
        // there kept an authored `flowRef` and got no warning.
        for child in task.children_mut() {
            let child_path = task_path(&parent, &child.id);
            resolve_task(child, child_path, cx, warnings).await?;
        }
        // Task 2.5 fix round 1 (review I1): every kind's own `errors:`
        // lists too -- `Task::error_children()`, which `children()`
        // deliberately leaves out. Without this, a `Subflow` there kept an
        // authored `flowRef` and got no warning, and an `egret.sql`
        // `database:` there reached the snapshot unchecked. After the
        // control-flow children, in `error_children()`'s order.
        for child in task.error_children_mut() {
            let child_path = task_path(&parent, &child.id);
            resolve_task(child, child_path, cx, warnings).await?;
        }
        Ok(())
    })
}

/// Kestra's warning for a `Subflow` whose target the publishing workspace
/// does not hold: `FlowService.warnings`, `FlowService.java:642`, verbatim.
/// `code` and `path` are egret's, the shape every `spec::validate` finding
/// has.
fn subflow_not_found(namespace: &str, flow_id: &str, path: String) -> Finding {
    Finding {
        code: "subflow_not_found",
        message: format!("The subflow '{flow_id}' not found in namespace '{namespace}'."),
        path,
    }
}

/// Resolve every `core.trigger.flow`'s `dependsOn` list -- Kestra's `Flow`
/// trigger names OTHER flows by the identical `(namespace, flowId)` pair a
/// `core.subflow` task does (`models::trigger::Dependency`, structurally the
/// same shape as `models::task::Subflow`), and is resolved through the exact
/// same scoped lookup, `FlowRepo::find_in_workspace`, and frozen-id shape as
/// [`resolve_task`]'s `TaskKind::Subflow` arm above -- see that arm's own
/// comment for why `namespace`/`flow_id` are kept rather than overwritten,
/// and for `Dependency::flow_ref`'s identical third-field reasoning.
///
/// Debt §5.1 (2026-10-19): a dependency the caller's workspace does not hold
/// publishes with `flow_ref = None` and, unlike a `Subflow`, **no** warning:
/// Kestra's `FlowService.warnings` says nothing about a `dependsOn`
/// (`FlowService.java:556-568`), where a dependency on a flow not created
/// yet is legal.
///
/// Whole-branch review finding 2: this function is what closes the gap --
/// before it, `publish` never read `flow.triggers` at all, so a trigger's
/// `dependsOn` pair reached [`FlowRepo::insert_version`] verbatim, unresolved
/// and unchecked against the caller's workspace. The exact defect Task 6
/// closed for `TaskKind`s/`RefKind`s, reopened one type over because
/// `TriggerKind` was never in that sweep's scope.
///
/// A `Dependency` with `namespace`/`flow_id` still `None` is skipped rather
/// than refused here -- `spec::validate`'s `dependency_namespace_required`/
/// `dependency_flow_id_required` (added in the same fix round) already
/// refused this flow at step 3, above, if either were absent on ANY
/// dependency, so by the time this function runs every `Dependency` it sees
/// is guaranteed to have both. The `let...else` is defensive, not
/// load-bearing, same as `TaskKind::Subflow`'s identical guard.
async fn resolve_trigger_dependencies(
    triggers: &mut [Trigger],
    ws: Uuid,
    repo: &FlowRepo,
) -> Result<()> {
    for trigger in triggers.iter_mut() {
        let TriggerKind::Flow(ft) = &mut trigger.kind else {
            continue;
        };
        for dep in ft.depends_on.iter_mut() {
            let (Some(namespace), Some(flow_id)) = (&dep.namespace, &dep.flow_id) else {
                continue;
            };
            // Scoped to `ws`, so a foreign flow and an absent one are the
            // same `None`; written even when `None`, as the `Subflow` arm.
            dep.flow_ref = repo.find_in_workspace(ws, namespace, flow_id).await?;
        }
    }
    Ok(())
}
