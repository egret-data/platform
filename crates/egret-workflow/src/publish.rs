//! `publish` — the ordered walk that turns a draft into a frozen version, and
//! the resolution of every reference a flow makes to another egret object.
//!
//! The order is the design (spec section 8, Task 6 brief step 4):
//! 1. load the draft, scoped to `ws` (`FlowRepo::get` -- `NotFound` for a
//!    missing or foreign flow, the same status every refusal below answers
//!    with);
//! 2. `parse(source)` -- a parse failure is `InvalidInput`;
//! 3. `validate(&flow)` -- any **error** is `InvalidInput`; warnings are
//!    carried into the snapshot but never block publish (a flow declaring
//!    `concurrency` must still publish -- it is simply not enforced yet);
//! 4. walk every task; `registry::known_type(&task.r#type)` -- `None` is
//!    `InvalidInput`, naming the type, so a typo like `egret.sqll` fails to
//!    publish instead of resolving vacuously and failing only at run time,
//!    in W3, with no author present. A `core.subflow` task never reaches this
//!    check at all -- it is a typed `TaskKind::Subflow`, not a `Leaf`, and is
//!    resolved by its own arm below instead (fix round 1);
//! 5. for each of that type's declared `RefField`s, read the leaf's own
//!    field and ask `refs` which workspace owns it, refusing **both** an
//!    absent object and one owned by a *different* workspace as the
//!    identical `NotFound` -- see [`ref_not_found`]. A `core.subflow` task's
//!    `namespace`/`flowId` pair is resolved the same way, through the same
//!    port, under `RefKind::Subflow` -- see
//!    `WorkflowRefPort`'s doc comment for the `name_or_id` encoding that
//!    pair uses;
//! 6. rewrite the resolved id into the plan in place;
//! 7. `insert_version` with a snapshot of `{ source, plan (resolved),
//!    warnings }`;
//! 8. `set_live`.
//!
//! Steps 5 and 6 are the whole point: after 6, the snapshot contains no name
//! that still has to be looked up. Migration 0128 is the recorded case of
//! skipping this -- a plain FK with no workspace predicate, and a Cedar check
//! that was told the CALLER's workspace, never the object's. A flow is a
//! machine for creating more of those unless every reference it makes is
//! checked HERE, before anything is frozen.
//!
//! # Which `FlowRepo` calls this makes are unscoped, and what makes them safe
//!
//! `FlowRepo::get` (step 1) is the only READ this function does, and it IS
//! scoped by `ws` -- `WHERE id = $1 AND workspace_id = $2`, `NotFound` for a
//! missing or foreign flow. `insert_version(flow, ...)` (step 7) and
//! `set_live(flow, version)` (step 8) both key off `flow` alone, with no
//! workspace predicate of their own (each says so in its own doc comment on
//! `FlowRepo`), so calling either with an unvalidated `flow` would write into
//! -- or make live -- a row this function never confirmed belongs to `ws`.
//! What makes both calls here safe is exactly step 1: `flow` only reaches
//! either of them after `repo.get(ws, flow)` has already returned `Ok`, which
//! is only possible when `flow` names a row in `ws`. This function calls
//! neither `FlowRepo::list_versions` nor `FlowRepo::get_version` (both also
//! unscoped, and — unlike `insert_version`/`set_live` — reads, so a caller of
//! either that skips its own `ws` check would hand back another workspace's
//! snapshots and notes with no error) -- that scoping obligation is
//! unclaimed by this task; a future caller of either must repeat step 1's
//! pattern for itself.

use std::future::Future;
use std::pin::Pin;

use egret_core::{Error, Result, UserId};
use uuid::Uuid;

use crate::models::{Flow, Task, TaskKind};
use crate::repo::{FlowRepo, FlowVersionRow};
use crate::spec::{parse, registry, validate};
use egret_workflow_api::{RefKind, WorkflowRefPort};

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

/// Publish `flow`'s current draft: parse, validate, resolve every reference,
/// freeze a new immutable version, and mark it live. See the module doc for
/// the ordered steps and where each refusal happens.
pub async fn publish(
    repo: &FlowRepo,
    refs: &dyn WorkflowRefPort,
    ws: Uuid,
    by: UserId,
    flow: Uuid,
    note: &str,
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
    // below untouched.
    let report = validate(&parsed);
    if !report.errors.is_empty() {
        let msg = report
            .errors
            .iter()
            .map(|f| format!("{} ({})", f.message, f.path))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::InvalidInput(msg));
    }

    // Steps 4-6: walk every task in all four root lists (mirrors
    // `spec::validate`'s own walk over `tasks`/`errors`/`finally`/
    // `afterExecution`), refusing an unknown type and resolving + rewriting
    // every declared reference in place.
    for list in [
        &mut parsed.tasks,
        &mut parsed.errors,
        &mut parsed.finally_,
        &mut parsed.after_execution,
    ] {
        resolve_all(list, ws, refs).await?;
    }

    // Step 7: self-contained on purpose -- W3's executor (and a reader of a
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

    // Step 8.
    repo.set_live(flow, version.id).await?;

    Ok(version)
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Resolve every task in `tasks`, in order. A plain `for` loop over an
/// `async fn` would do, except each task recurses back into this same
/// function through [`resolve_task`] for its own children, and Rust has no
/// native support for recursive `async fn`s -- hence the explicit boxing
/// both functions share.
fn resolve_all<'a>(
    tasks: &'a mut [Task],
    ws: Uuid,
    refs: &'a dyn WorkflowRefPort,
) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        for task in tasks.iter_mut() {
            resolve_task(task, ws, refs).await?;
        }
        Ok(())
    })
}

/// Resolve one task's own references (if it is a `Leaf`, refusing an unknown
/// type first -- step 4) and recurse into its children exactly as
/// `Task::children()` walks them, but mutably: this is what lets step 6
/// ("rewrite the resolved fields into the plan") reach every nesting depth,
/// not just a flow's top-level lists.
fn resolve_task<'a>(
    task: &'a mut Task,
    ws: Uuid,
    refs: &'a dyn WorkflowRefPort,
) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        match &mut task.kind {
            TaskKind::Sequential(s) => resolve_all(&mut s.tasks, ws, refs).await,
            TaskKind::Parallel(p) => resolve_all(&mut p.tasks, ws, refs).await,
            TaskKind::If(i) => {
                resolve_all(&mut i.then, ws, refs).await?;
                resolve_all(&mut i.r#else, ws, refs).await
            }
            TaskKind::Switch(s) => {
                for case_tasks in s.cases.values_mut() {
                    resolve_all(case_tasks, ws, refs).await?;
                }
                resolve_all(&mut s.defaults, ws, refs).await
            }
            TaskKind::Dag(d) => {
                for node in d.tasks.iter_mut() {
                    if let Some(t) = node.task.as_mut() {
                        resolve_task(t, ws, refs).await?;
                    }
                }
                Ok(())
            }
            TaskKind::Loop(l) => resolve_all(&mut l.tasks, ws, refs).await,
            TaskKind::LoopUntil(l) => resolve_all(&mut l.tasks, ws, refs).await,
            TaskKind::AllowFailure(a) => resolve_all(&mut a.tasks, ws, refs).await,
            TaskKind::WorkingDirectory(w) => resolve_all(&mut w.tasks, ws, refs).await,
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
            TaskKind::Subflow(s) => {
                let (Some(namespace), Some(flow_id)) = (&s.namespace, &s.flow_id) else {
                    return Ok(());
                };
                let name = subflow_ref_name(namespace, flow_id);
                let resolved = refs.workspace_of(RefKind::Subflow, &name).await?;
                let id = match resolved {
                    // Identical rule to the `Leaf` arm below: `None` and a
                    // foreign `owner` both refuse the same way.
                    Some((id, owner)) if owner == ws => id,
                    _ => return Err(ref_not_found()),
                };
                // `namespace`/`flow_id` are kept, unlike a `Leaf`'s single
                // reference field -- see `Subflow::flow_ref`'s own doc
                // comment in `models/task.rs` for why this is a third field
                // rather than an overwrite of either existing one.
                s.flow_ref = Some(id);
                Ok(())
            }
            TaskKind::Leaf { spec } => {
                let type_spec = registry::known_type(&task.r#type).ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "task '{}' has unknown type '{}'",
                        task.id, task.r#type
                    ))
                })?;
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

                    let resolved = refs.workspace_of(rf.kind, &value).await?;
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
    })
}

/// The `name_or_id` a `core.subflow` task's `(namespace, flowId)` pair
/// encodes into, before it is handed to
/// `WorkflowRefPort::workspace_of(RefKind::Subflow, ..)`. See that trait's
/// own doc comment for the full argument; kept here as one function so the
/// encoding has exactly one writer, matching the one-reader contract stated
/// there.
fn subflow_ref_name(namespace: &str, flow_id: &str) -> String {
    format!("{namespace}/{flow_id}")
}
