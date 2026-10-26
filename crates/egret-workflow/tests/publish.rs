//! Publish, and the reference-resolution seam. Requires DATABASE_URL.

mod common;

use egret_workflow::{RefKind, WorkflowRefPort};
use uuid::Uuid;

/// The task types this binary serves: the runner set, plus `egret.sql`.
///
/// No deployment serves `egret.sql` today, so since no-silent-stall Task 3
/// publish refuses it at step 3 (`task_type_not_served`), before the
/// reference resolution these tests are about. `egret.sql` is the one task
/// type with a `RefField` (`database`), so the tests keep it and run as a
/// deployment that serves it would: the resolution of steps 4-6 is the same
/// code either way. The refusal itself is `tests/not_served.rs`'s.
///
/// Every test here calls [`db`] first, so this is the set installed before
/// any `common` helper installs its default.
fn served_with_sql() -> &'static egret_workflow::runtime::Served {
    let mut cfg = egret_config::Config::default().workflow;
    cfg.container_runner = egret_config::ContainerRunner::Docker;
    let runner = egret_workflow::runtime::served_for(&cfg);
    egret_workflow::runtime::install_served_for_tests(
        egret_workflow::runtime::Served::from_type_ids(
            runner
                .type_ids()
                .map(str::to_string)
                .chain(["egret.sql".to_string()]),
        ),
    )
}

async fn db() -> Option<egret_metastore::Db> {
    served_with_sql();
    common::db().await
}

/// A fake that owns objects in a NAMED workspace, so a test can put an object in
/// a different workspace than the caller's and watch publish refuse.
#[derive(Default)]
struct FakeRefs {
    objects: std::collections::HashMap<(RefKind, String), (Uuid, Uuid)>,
}

#[async_trait::async_trait]
impl WorkflowRefPort for FakeRefs {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error> {
        Ok(self.objects.get(&(kind, name.to_owned())).copied())
    }
}

/// THE test of this task. Migration 0128 shipped a plain FK with no workspace
/// predicate and a Cedar check that was told the CALLER's workspace, never the
/// object's; five columns had that shape. A flow is a machine for creating more
/// of them, so a foreign reference must make the flow UNPUBLISHABLE rather than
/// something a handler must remember to refuse later.
#[tokio::test]
async fn a_flow_referencing_another_workspace_cannot_be_published() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let other_ws = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Database, "trino_prod".into()),
        (Uuid::new_v4(), other_ws), // owned by SOMEONE ELSE
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect_err("publishing a foreign reference must fail");

    // NotFound, not Forbidden: absence and denial must be indistinguishable, or
    // publish becomes a probe for which objects exist in other workspaces.
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

/// The same refusal for an id that never existed anywhere. If these two produced
/// different errors, the difference would be the probe.
#[tokio::test]
async fn an_unknown_reference_fails_the_same_way_as_a_foreign_one() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    database: nope\n    sql: SELECT 1\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("must fail");
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn a_resolvable_flow_publishes_and_freezes_its_references() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let db_id = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects
        .insert((RefKind::Database, "trino_prod".into()), (db_id, ws));

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n",
    )
    .await;

    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "first")
        .await
        .expect("publish");
    assert_eq!(v.version, 1);

    // The snapshot must be self-contained: the RESOLVED id, not the name.
    let s = v.snapshot.to_string();
    assert!(
        s.contains(&db_id.to_string()),
        "snapshot must carry the resolved id: {s}"
    );

    let live = repo.get(ws, f.id).await.expect("get");
    assert_eq!(live.live_version_id, Some(v.id));
    assert_eq!(live.state, "live");
}

/// An unknown task type contributes no references, so it would resolve
/// vacuously. Publish must refuse it instead -- otherwise a typo'd type
/// publishes cleanly and fails only at run time, in W3, with no author present.
#[tokio::test]
async fn an_unknown_task_type_cannot_be_published() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sqll\n    sql: SELECT 1\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("an unknown type must not publish");
    assert!(matches!(err, egret_core::Error::InvalidInput(_)), "{err:?}");
}

/// Validation errors block publish; warnings do not. A flow declaring
/// `concurrency` must publish -- it just is not enforced yet.
#[tokio::test]
async fn warnings_do_not_block_publish_but_errors_do() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let warns = common::draft(
        &repo, ws, user,
        "id: w\nnamespace: a.b\nconcurrency:\n  limit: 1\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, warns.id, "")
        .await
        .expect("a warning must not block publish");

    let bad = common::draft(
        &repo,
        ws,
        user,
        "id: e\nnamespace: BAD\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    assert!(
        egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
            .await
            .is_err()
    );
}

/// The 2026-10-11 ruling: a `Pause` declaring `behavior`, `onPause`, `tasks`
/// or `errors` is a validation error, so it cannot be versioned. Before the
/// ruling it published with a warning and every execution of it stalled in
/// `RUNNING`. The control is the same flow with a plain bounded `Pause`,
/// which publishes -- without it a publish that refused every `Pause` would
/// pass.
#[tokio::test]
async fn a_pause_egret_does_not_run_cannot_be_published() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let pause = |id: &str, extra: &str| {
        format!(
            "id: {id}\nnamespace: a.b\ntasks:\n  - id: gate\n    \
             type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n{extra}"
        )
    };

    let bad = common::draft(&repo, ws, user, &pause("pb", "    behavior: RESUME\n")).await;
    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
        .await
        .expect_err("a Pause with `behavior:` must not publish");
    match &err {
        egret_core::Error::InvalidInput(m) => {
            assert!(
                m.contains("'behavior'") && m.contains("(tasks.gate.behavior)"),
                "{m}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        repo.get(ws, bad.id).await.expect("get").live_version_id,
        None
    );

    let ok = common::draft(&repo, ws, user, &pause("pok", "")).await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, ok.id, "")
        .await
        .expect("a plain Pause publishes");
}

/// The 2026-10-12 ruling: `ForEach` is the validation error
/// `unsupported_foreach`, so publish refuses it naming `Loop`. It was
/// already refused here before the ruling, but only as an unknown task type
/// at step 4, while a saved draft could start and then stalled on it with
/// `NoHandler` (Task 7b); the assertion
/// is on the ruling's own message. The control is the same flow with `Loop`.
#[tokio::test]
async fn a_foreach_cannot_be_published_and_the_error_names_loop() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let bad = common::draft(&repo, ws, user, "id: fe\nnamespace: a.b\ntasks:\n  - id: fan\n    type: io.kestra.plugin.core.flow.ForEach\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n").await;
    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
        .await
        .expect_err("a ForEach must not publish");
    match &err {
        egret_core::Error::InvalidInput(m) => assert!(
            m.contains("io.kestra.plugin.core.flow.Loop") && m.contains("(tasks.fan)"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        repo.get(ws, bad.id).await.expect("get").live_version_id,
        None
    );

    let ok = common::draft(&repo, ws, user, "id: lp\nnamespace: a.b\ntasks:\n  - id: fan\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n").await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, ok.id, "")
        .await
        .expect("a Loop publishes");
}

// -- Fix round 1: `subflow:` was the sixth row in spec section 8's own
// reference table (`workflow.flows`, "this crate") and had no resolution at
// all -- no `RefField`, no arm in `resolve_task`.
//
// Debt §5.1 (2026-10-19): a `Subflow`'s `(namespace, flowId)` pair resolves
// in the CALLER's workspace only, through `FlowRepo::find_in_workspace`, not
// the unscoped `RefKind::Subflow` port (both now removed). Found: the id is frozen. Absent from
// the caller's workspace -- whether another workspace holds the name or none
// does -- the flow publishes with Kestra's warning and no frozen id. The two
// security tests that used to assert `NotFound` for "foreign" and "absent"
// are now the one test below: the answer depends on no other workspace, so
// it still cannot be a probe.

/// The JSON a snapshot carries for its first task.
fn first_task(v: &egret_workflow::FlowVersionRow) -> &serde_json::Value {
    &v.snapshot["plan"]["tasks"][0]
}

/// The message Kestra's `FlowService.warnings` gives a missing `Subflow`
/// target (`FlowService.java:642`).
fn subflow_warning(namespace: &str, flow_id: &str) -> String {
    format!("The subflow '{flow_id}' not found in namespace '{namespace}'.")
}

/// **Debt §5.1: a `Subflow` naming a flow only ANOTHER workspace has publishes
/// exactly as one naming a flow nobody has** -- with Kestra's warning, no
/// frozen id -- so publish still answers nothing about other workspaces.
///
/// Through the real `WorkflowRefs`. Its `RefKind::Subflow` arm used to be the
/// unscoped `FlowRepo::find_by_name`, which found the other workspace's flow:
/// a publish that asked it refused the foreign case (`NotFound`) and not the
/// absent one -- the difference that would be the probe. Both are removed
/// (fix round 1); the mutation that restores an unscoped lookup is in the
/// task report.
#[tokio::test]
async fn a_subflow_absent_from_the_callers_workspace_publishes_with_the_warning_whoever_has_it() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    // Another workspace HAS `company.analytics/nightly_rollup`.
    common::draft(
        &repo, other_ws, other_user,
        "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n",
    )
    .await;
    let src = |flow: &str, target: &str| {
        format!(
            "id: {flow}\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: {target}\n"
        )
    };
    let foreign = common::draft(&repo, ws, user, &src("foreign", "nightly_rollup")).await;
    let absent = common::draft(&repo, ws, user, &src("absent", "nope")).await;
    let refs = common::workflow_refs();

    let a = egret_workflow::publish(&repo, &refs, ws, user, foreign.id, "")
        .await
        .expect("a subflow naming another workspace's flow publishes");
    let b = egret_workflow::publish(&repo, &refs, ws, user, absent.id, "")
        .await
        .expect("a subflow naming a flow nobody has publishes");

    // Same shape: no frozen id, and the warning naming the pair.
    for (v, id) in [(&a, "nightly_rollup"), (&b, "nope")] {
        let warnings = v.snapshot["warnings"].to_string();
        assert!(
            warnings.contains(&subflow_warning("company.analytics", id)),
            "{warnings}"
        );
        // The path is real (so `flowRef` being absent is not vacuous) ...
        assert_eq!(first_task(v)["flowId"], id, "{}", v.snapshot);
        // ... and nothing was frozen.
        assert!(first_task(v)["flowRef"].is_null(), "{}", v.snapshot);
    }
    // Byte-identical apart from the name: the warning list has one entry of
    // the same code and path either way.
    assert_eq!(
        a.snapshot["warnings"],
        serde_json::json!([{
            "code": "subflow_not_found",
            "message": subflow_warning("company.analytics", "nightly_rollup"),
            "path": "tasks.t",
        }]),
    );
}

/// A `flowRef` the AUTHOR wrote into the draft is not a frozen id: the field
/// deserialises (it is how a snapshot reads back), and no Kestra `Subflow` has
/// it. Since package #1 (spec D1, D17) the save-time key census refuses it as
/// an `unknown_property` error, so the author's uuid never reaches a published
/// plan, and nothing is published.
#[tokio::test]
async fn an_authored_flow_ref_is_refused_at_publish() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let theirs = common::draft(
        &repo, other_ws, other_user,
        "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n",
    )
    .await;
    let f = common::draft(
        &repo,
        ws,
        user,
        &format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n    flowRef: {}\n",
            theirs.id
        ),
    )
    .await;
    // Control: the draft really carries the forged id.
    assert!(f.source.contains(&theirs.id.to_string()));

    let err = egret_workflow::publish(&repo, &common::workflow_refs(), ws, user, f.id, "")
        .await
        .expect_err("an authored flowRef must not publish");
    match err {
        egret_core::Error::InvalidInput(m) => assert!(
            m.contains("`flowRef` is not a property of io.kestra.plugin.core.flow.Subflow")
                && m.contains("tasks.t.flowRef"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
}

/// Found in the caller's workspace: the id is frozen, and no warning. The
/// same name in ANOTHER workspace too -- the removed `FlowRepo::find_by_name`'s recorded
/// over-refusal (its doc: "can make an otherwise-publishable reference
/// resolve to the WRONG workspace's flow and be refused as foreign") -- does
/// not change which id is frozen.
#[tokio::test]
async fn a_subflow_naming_a_flow_in_the_callers_own_workspace_publishes_and_freezes_its_id() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let target_src = "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n";
    let target = common::draft(&repo, ws, user, target_src).await;
    common::draft(&repo, other_ws, other_user, target_src).await;
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n",
    )
    .await;

    let v = egret_workflow::publish(&repo, &common::workflow_refs(), ws, user, f.id, "")
        .await
        .expect("publish");

    assert_eq!(
        first_task(&v)["flowRef"],
        serde_json::json!(target.id),
        "snapshot must carry the resolved subflow id: {}",
        v.snapshot
    );
    assert_eq!(
        v.snapshot["warnings"],
        serde_json::json!([]),
        "{}",
        v.snapshot
    );
}

// -- Fix round 2, Finding 1: `namespace: "a/b"`, `flowId: "c"` used to encode
// to "a/b/c" and decode, via split_once('/'), as ("a", "b/c") -- a different
// pair than declared. `spec::validate` now refuses this at publish's step 3,
// before the colliding string is ever built.

/// The collision itself, end to end through `publish`: a namespace carrying
/// its own `/` must not publish.
#[tokio::test]
async fn a_subflow_namespace_containing_a_slash_cannot_be_published() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: \"a/b\"\n    flowId: c\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("a subflow namespace containing '/' must not publish");
    assert!(matches!(err, egret_core::Error::InvalidInput(_)), "{err:?}");
}

/// Companion, through `publish` too: a legitimate, dotted namespace must
/// still publish -- the fix must not reject every subflow with a `.` in it.
#[tokio::test]
async fn a_subflow_with_a_legitimate_dotted_namespace_still_publishes() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let target = common::draft(
        &repo, ws, user,
        "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n",
    )
    .await;
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n",
    )
    .await;

    let v = egret_workflow::publish(&repo, &common::workflow_refs(), ws, user, f.id, "")
        .await
        .expect("a legitimate dotted namespace must publish");
    assert_eq!(
        first_task(&v)["flowRef"],
        serde_json::json!(target.id),
        "snapshot must carry the resolved id: {}",
        v.snapshot
    );
}

// ---------- Whole-branch review, IMPORTANT finding 2 ----------
//
// A `core.trigger.flow`'s `dependsOn` list names another flow the same way
// `core.subflow` does -- `(namespace, flowId)` -- and, before this fix,
// `publish` never read `flow.triggers` at all: it walked `tasks`/`errors`/
// `finally`/`afterExecution` and nothing else, so a trigger's `dependsOn`
// reached `insert_version` verbatim, unresolved and unchecked against the
// caller's workspace. Spec §8's invariant ("the published artifact contains
// no unresolved id") was false for this one shape. These tests are
// the trigger twins of the `TaskKind::Subflow` tests just above (since debt
// §5.1: found in the caller's workspace freezes the id; absent from it
// publishes with no id and, unlike a `Subflow`, no warning).

fn flow_with_trigger_dependency(namespace: &str, flow_id: &str) -> String {
    format!(
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
         triggers:\n  - id: chain\n    type: core.flow\n    dependsOn:\n      \
         - namespace: {namespace}\n        flowId: {flow_id}\n"
    )
}

/// The `dependsOn` entry a snapshot carries for the first trigger.
fn first_dependency(v: &egret_workflow::FlowVersionRow) -> &serde_json::Value {
    &v.snapshot["plan"]["triggers"][0]["dependsOn"][0]
}

/// The trigger twin of
/// `a_subflow_naming_a_flow_in_the_callers_own_workspace_publishes_and_freezes_its_id`.
#[tokio::test]
async fn a_trigger_dependency_naming_a_flow_in_the_callers_own_workspace_publishes_and_freezes_its_id(
) {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let target_src = "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n";
    let target = common::draft(&repo, ws, user, target_src).await;
    common::draft(&repo, other_ws, other_user, target_src).await;
    let f = common::draft(
        &repo,
        ws,
        user,
        &flow_with_trigger_dependency("company.analytics", "nightly_rollup"),
    )
    .await;

    let v = egret_workflow::publish(&repo, &common::workflow_refs(), ws, user, f.id, "")
        .await
        .expect("publish");
    assert_eq!(
        first_dependency(&v)["flowRef"],
        serde_json::json!(target.id),
        "snapshot must carry the resolved trigger dependency id, not the bare name: {}",
        v.snapshot
    );
}

/// **Debt §5.1, the trigger twin of
/// `a_subflow_absent_from_the_callers_workspace_publishes_with_the_warning_whoever_has_it`**:
/// a `dependsOn` naming a flow only another workspace has publishes exactly as
/// one naming a flow nobody has -- no frozen id, and **no** warning, because
/// Kestra's `FlowService.warnings` says nothing about a `dependsOn`
/// (`FlowService.java:556-568`): a dependency on a flow not created yet is
/// legal there.
#[tokio::test]
async fn a_trigger_dependency_absent_from_the_callers_workspace_publishes_whoever_has_it() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    common::draft(
        &repo, other_ws, other_user,
        "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n",
    )
    .await;
    let src = |flow: &str, target: &str| {
        flow_with_trigger_dependency("company.analytics", target).replacen(
            "id: f\n",
            &format!("id: {flow}\n"),
            1,
        )
    };
    let foreign = common::draft(&repo, ws, user, &src("foreign", "nightly_rollup")).await;
    let absent = common::draft(&repo, ws, user, &src("absent", "nope")).await;
    let refs = common::workflow_refs();

    let a = egret_workflow::publish(&repo, &refs, ws, user, foreign.id, "")
        .await
        .expect("a dependency on another workspace's flow publishes");
    let b = egret_workflow::publish(&repo, &refs, ws, user, absent.id, "")
        .await
        .expect("a dependency on a flow nobody has publishes");

    for (v, id) in [(&a, "nightly_rollup"), (&b, "nope")] {
        assert_eq!(first_dependency(v)["flowId"], id, "{}", v.snapshot);
        assert!(first_dependency(v)["flowRef"].is_null(), "{}", v.snapshot);
        assert_eq!(
            v.snapshot["warnings"],
            serde_json::json!([]),
            "{}",
            v.snapshot
        );
    }
}

/// W9b: the save-time refusal of a namespace with a `_files` segment, through
/// publish (its unit twin is `spec::validate`'s
/// `publish_refuses_a_files_segment_namespace_by_name`; the run-time twin is
/// `nsfiles::access`'s `a_files_segment_namespace_is_refused_for_every_access_and_actor`).
#[tokio::test]
async fn publish_refuses_a_files_segment_namespace_by_name() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let bad = common::draft(
        &repo,
        ws,
        user,
        "id: f\nnamespace: a._files.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
        .await
        .expect_err("a `_files` namespace segment must not publish");
    match err {
        egret_core::Error::InvalidInput(m) => assert!(
            m.contains("'a._files.b'") && m.contains("'_files'") && m.contains("(namespace)"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
    // The control: the same flow one character away publishes.
    let good = common::draft(
        &repo,
        ws,
        user,
        "id: g\nnamespace: a.files.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, good.id, "")
        .await
        .expect("a.files.b publishes");
}

/// W9c: the save-time refusal of a namespace with a `_kv` segment, through
/// publish (its unit twin is `spec::validate`'s
/// `publish_refuses_a_kv_segment_namespace_by_name`; the run-time twin is
/// `nsfiles::access`'s `a_kv_segment_namespace_is_refused_for_every_resource_access_and_actor`).
#[tokio::test]
async fn publish_refuses_a_kv_segment_namespace_by_name() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let bad = common::draft(
        &repo,
        ws,
        user,
        "id: f\nnamespace: a._kv.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
        .await
        .expect_err("a `_kv` namespace segment must not publish");
    match err {
        egret_core::Error::InvalidInput(m) => assert!(
            m.contains("'a._kv.b'") && m.contains("'_kv'") && m.contains("(namespace)"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
    // The control: the same flow one character away publishes.
    let good = common::draft(
        &repo,
        ws,
        user,
        "id: g\nnamespace: a.kv.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, good.id, "")
        .await
        .expect("a.kv.b publishes");
}

/// A `WorkingDirectory` flow whose `namespaceFiles` block is `block` (YAML,
/// indented under the key).
fn working_directory(id: &str, block: &str) -> String {
    format!(
        "id: {id}\nnamespace: w9b.wd\ntasks:\n  - id: wd\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    namespaceFiles:\n{block}    tasks:\n      - id: t\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n"
    )
}

/// W9b Task 5 (spec §7, the user's ruling): a `WorkingDirectory`'s
/// `namespaceFiles` is parsed at publish by the script tasks' own parse; a
/// malformed block is refused naming the property, a valid one draws no
/// `namespaceFiles` finding. (It published until no-silent-stall Task 1, which
/// refuses every `WorkingDirectory` as `unsupported_working_directory`.)
#[tokio::test]
async fn a_working_directory_with_a_malformed_namespace_files_is_refused_by_name_and_a_valid_one_is_not(
) {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    for (id, block, property) in [
        (
            "bad-ns",
            "      namespaces: w9b.s1\n",
            "namespaceFiles.namespaces",
        ),
        (
            "bad-if",
            "      ifExists: NOPE\n",
            "namespaceFiles.ifExists",
        ),
        ("bad-key", "      unknown: 1\n", "namespaceFiles.unknown"),
    ] {
        let bad = common::draft(&repo, ws, user, &working_directory(id, block)).await;
        let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
            .await
            .expect_err("a malformed namespaceFiles must not publish");
        match err {
            egret_core::Error::InvalidInput(m) => {
                // Fix round 1, M7: the finding's whole path, task prefix
                // included, as the report closes with it.
                assert!(m.contains(&format!("(tasks.wd.{property})")), "{m}")
            }
            other => panic!("{id}: {other:?}"),
        }
    }
    // The control: a valid `namespaceFiles` publishes. Its claim runs in the
    // prepare step (spec §5.1), so the property is no longer refused by name.
    for (id, block) in [
        (
            "good",
            "      namespaces: [io.test.first, io.test.second]\n      enabled: true\n      folderPerNamespace: true\n      exclude: [/ignore/**]\n",
        ),
        (
            "templated",
            "      enabled: \"{{ true }}\"\n      ifExists: WARN\n      include: [\"{{ flow.id }}/**\"]\n",
        ),
        // Fix round 1, M4: the four properties Kestra types as `Property`
        // take an expression, checked once it renders.
        (
            "expressions",
            "      ifExists: \"{{ inputs.mode }}\"\n      namespaces: \"{{ inputs.nss }}\"\n      include: \"{{ inputs.inc }}\"\n      exclude: \"{% if true %}[]{% endif %}\"\n",
        ),
    ] {
        let good = common::draft(&repo, ws, user, &working_directory(id, block)).await;
        egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, good.id, "")
            .await
            .unwrap_or_else(|e| panic!("{id}: a valid namespaceFiles publishes: {e:?}"));
    }
}

/// Plan Task 7 (spec §5.5): an `outputFiles` name the finish claim cannot
/// collect is refused at publish, by name, at its own path. A `**` is not
/// implemented by egret; a plain name and a file-name pattern are accepted.
#[tokio::test]
async fn a_working_directory_output_file_egret_cannot_collect_is_refused_at_publish() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let bad = "id: wd-out-bad
namespace: w9b.wd
tasks:
  - id: wd
    type: io.kestra.plugin.core.flow.WorkingDirectory
    outputFiles:
      - \"**/x.txt\"
    tasks:
      - id: t
        type: io.kestra.plugin.core.log.Log
        message: hi
";
    let draft = common::draft(&repo, ws, user, bad).await;
    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, draft.id, "")
        .await
        .expect_err("a `**` output must not publish");
    match err {
        egret_core::Error::InvalidInput(m) => {
            assert!(m.contains("(tasks.wd.outputFiles)"), "{m}");
            assert!(m.contains("not implemented by egret"), "{m}");
        }
        other => panic!("{other:?}"),
    }
    let good = "id: wd-out-good
namespace: w9b.wd
tasks:\n  - id: wd\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    outputFiles:\n      - out.txt\n      - \"*.csv\"\n    tasks:\n      - id: t\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";
    let draft = common::draft(&repo, ws, user, good).await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, draft.id, "")
        .await
        .expect("plain names and file-name patterns publish");
}

/// Plan Task 7 (spec §5.4): a `cache` that does not say what it caches is
/// refused at publish, naming the property. A `patterns` list is required; a
/// `ttl` must be an ISO-8601 duration.
#[tokio::test]
async fn a_working_directory_cache_that_is_malformed_is_refused_at_publish() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    for (id, cache) in [
        ("no-patterns", "      ttl: PT1H\n"),
        (
            "bad-ttl",
            "      patterns: [\"cache/**\"]\n      ttl: soon\n",
        ),
    ] {
        let yaml = format!(
            "id: wd-cache-{id}\nnamespace: w9b.wd\ntasks:\n  - id: wd\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    cache:\n{cache}    tasks:\n      - id: t\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n"
        );
        let draft = common::draft(&repo, ws, user, &yaml).await;
        let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, draft.id, "")
            .await
            .expect_err("a malformed cache must not publish");
        match err {
            egret_core::Error::InvalidInput(m) => {
                assert!(m.contains("(tasks.wd.cache)"), "{id}: {m}");
            }
            other => panic!("{id}: {other:?}"),
        }
    }
}

/// The block is kept in the model, verbatim: serialize -> parse -> equal,
/// and the parse of the kept value is the parse of the written one.
#[tokio::test]
async fn a_working_directory_namespace_files_round_trips_through_the_model() {
    use egret_workflow::models::{Task, TaskKind};
    let yaml = "id: wd\ntype: io.kestra.plugin.core.flow.WorkingDirectory\nnamespaceFiles:\n  namespaces: [a.b, c]\n  include: ['**.txt']\n  ifExists: FAIL\n  folderPerNamespace: true\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let task: Task = serde_norway::from_str(yaml).expect("a task");
    let TaskKind::WorkingDirectory(w) = &task.kind else {
        panic!("{:?}", task.kind)
    };
    let kept = w.namespace_files.clone().expect("namespaceFiles is kept");
    assert_eq!(
        kept,
        serde_json::json!({
            "namespaces": ["a.b", "c"], "include": ["**.txt"], "ifExists": "FAIL",
            "folderPerNamespace": true,
        })
    );
    let value = serde_json::to_value(&task).expect("serialize");
    assert_eq!(value["namespaceFiles"], kept, "{value}");
    let again: Task = serde_json::from_value(value).expect("parse again");
    assert_eq!(again, task);
    let spec = egret_workflow::storage::nsfiles::stage::NamespaceFilesSpec::parse(&kept).unwrap();
    assert_eq!(
        spec.if_exists,
        egret_workflow::storage::nsfiles::stage::IfExistsProp::Value(
            egret_workflow::storage::nsfiles::stage::IfExists::Fail
        )
    );
    // Absent stays absent, and is not serialized.
    let bare: Task = serde_norway::from_str(
        "id: wd\ntype: io.kestra.plugin.core.flow.WorkingDirectory\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
    )
    .unwrap();
    let value = serde_json::to_value(&bare).unwrap();
    assert!(value.get("namespaceFiles").is_none(), "{value}");
}

/// W9c Task 4 (W9c spec §7): a `Set` or `Put` whose `value` calls `secret()`
/// is refused at publish with its own message, naming the task and the
/// property, and not with the generic `secret()` refusal for that property.
/// Its unit twin is `spec::validate`'s
/// `a_secret_in_a_set_or_put_value_is_refused_with_its_own_code_and_message`.
#[tokio::test]
async fn a_secret_in_a_kv_value_is_refused_at_publish_by_its_own_message() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    for (id, ty) in [
        ("set", "io.kestra.plugin.core.kv.Set"),
        ("put", "io.kestra.plugin.core.kv.Put"),
    ] {
        let f = common::draft(
            &repo,
            ws,
            user,
            &format!(
                "id: {id}\nnamespace: a.b\ntasks:\n  - id: s\n    type: {ty}\n    key: k\n    \
                 value:\n      token: \"{{{{ secret('API') }}}}\"\n"
            ),
        )
        .await;
        let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
            .await
            .expect_err("a secret in a KV value must not publish");
        let want = egret_workflow::plugins::secret_in_kv_value_message("s", "tasks.s.value.token");
        match err {
            egret_core::Error::InvalidInput(m) => {
                assert!(m.contains(&want), "{ty}: {m}");
                assert!(
                    !m.contains("resolves only on `kestra.*` plugin tasks"),
                    "{ty}: the generic refusal too: {m}"
                );
            }
            other => panic!("{ty}: {other:?}"),
        }
    }
}

/// W9c Task 4: the corpus's `valids/kv` (two `kv()` calls in `Return`
/// tasks) publishes. `valids/retry-with-flowable-errors` carries no
/// `secret()` either. Its `AllowFailure` block is planned now (spec §5.2 of the
/// AllowFailure flowable), but a `retry:` on the block itself is a non-goal, so
/// it is refused for that retry and for nothing else.
#[tokio::test]
async fn valids_kv_publishes_and_retry_with_flowable_errors_is_refused_only_for_its_block_retry() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let kv = include_str!("../oracle-exec/corpus/valids/kv.yaml");
    let draft = common::draft(&repo, ws, user, kv).await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, draft.id, "")
        .await
        .unwrap_or_else(|e| panic!("{e}: {kv}"));

    let af = include_str!("../oracle-exec/corpus/valids/retry-with-flowable-errors.yaml");
    let draft = common::draft(&repo, ws, user, af).await;
    match egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, draft.id, "").await {
        Err(egret_core::Error::InvalidInput(m)) => {
            assert!(
                m.contains("put `retry:` on the child task")
                    && m.contains("(tasks.retry_block.retry)"),
                "{m}"
            );
        }
        other => panic!("{other:?}"),
    }
}

// -- Task 2.5 fix round 1 (review I1): a nested flowable's `errors:` list.
// `resolve_task` walked `Task::children()` only, the control-flow walk, so a
// task inside a `Sequential`'s (or any of the six kinds') `errors:` never had
// its references resolved: an authored `flowRef` and an unchecked `database:`
// both reached the snapshot as written. Each test below puts the task ONLY
// inside a nested `errors:`, so nothing at the top level can satisfy it.

/// A `Sequential` whose `errors:` holds `handler`, and whose `tasks:` holds a
/// plain `Log`.
fn with_nested_errors_handler(id: &str, handler: &str) -> String {
    format!(
        "id: {id}\nnamespace: a.b\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n    errors:\n{handler}"
    )
}

/// An absent `Subflow` target inside a nested `errors:` gets what a top-level
/// one gets: Kestra's warning at its path, and no `flowRef`. (An authored
/// `flowRef`, even one naming another workspace's flow, is refused before this
/// point: `an_authored_flow_ref_is_refused_at_publish`.)
#[tokio::test]
async fn a_subflow_inside_a_nested_errors_list_is_resolved_and_warned() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let handler = "      - id: h\n        type: core.subflow\n        namespace: company.analytics\n        flowId: nightly_rollup\n";
    let f = common::draft(&repo, ws, user, &with_nested_errors_handler("f", handler)).await;

    let v = egret_workflow::publish(&repo, &common::workflow_refs(), ws, user, f.id, "")
        .await
        .expect("publishes");
    let h = &v.snapshot["plan"]["tasks"][0]["errors"][0];
    // The path is real, so `flowRef` being absent is not vacuous.
    assert_eq!(h["flowId"], "nightly_rollup", "{}", v.snapshot);
    assert!(h["flowRef"].is_null(), "{}", v.snapshot);
    assert_eq!(
        v.snapshot["warnings"],
        serde_json::json!([
            {
                "code": "subflow_not_found",
                "message": subflow_warning("company.analytics", "nightly_rollup"),
                "path": "tasks.seq.h",
            },
        ]),
    );
}

/// A `database:` owned by another workspace inside a nested `errors:` is
/// refused exactly as `a_flow_referencing_another_workspace_cannot_be_published`
/// refuses it at the top level: `NotFound`.
#[tokio::test]
async fn a_foreign_database_inside_a_nested_errors_list_cannot_be_published() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Database, "trino_prod".into()),
        (Uuid::new_v4(), Uuid::new_v4()), // owned by SOMEONE ELSE
    );
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        &with_nested_errors_handler(
            "f",
            "      - id: h\n        type: egret.sql\n        database: trino_prod\n        sql: SELECT 1\n",
        ),
    )
    .await;

    let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect_err("a foreign database inside errors: must not publish");
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

/// The same flow with the `database:` in the caller's workspace publishes and
/// freezes the id -- the control that the refusal above is the reference.
#[tokio::test]
async fn an_own_database_inside_a_nested_errors_list_publishes_and_freezes_its_id() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let db_id = Uuid::new_v4();
    let mut refs = FakeRefs::default();
    refs.objects
        .insert((RefKind::Database, "trino_prod".into()), (db_id, ws));
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        &with_nested_errors_handler(
            "f",
            "      - id: h\n        type: egret.sql\n        database: trino_prod\n        sql: SELECT 1\n",
        ),
    )
    .await;

    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect("publish");
    assert_eq!(
        v.snapshot["plan"]["tasks"][0]["errors"][0]["database"],
        serde_json::json!(db_id.to_string()),
        "{}",
        v.snapshot
    );
}

/// An unknown task type inside a nested `errors:` is refused, as at the top
/// level (`an_unknown_task_type_cannot_be_published`).
#[tokio::test]
async fn an_unknown_task_type_inside_a_nested_errors_list_cannot_be_published() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        &with_nested_errors_handler(
            "f",
            "      - id: h\n        type: egret.sqll\n        sql: SELECT 1\n",
        ),
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("an unknown type inside errors: must not publish");
    match &err {
        egret_core::Error::InvalidInput(m) => assert!(m.contains("egret.sqll"), "{m}"),
        other => panic!("{other:?}"),
    }
}

/// **PR-2 final review M1: an `AllowFailure`'s `finally:` is walked too.**
/// `Task::children()` walks `tasks` then `finally`, and publish's walk now
/// goes through the same list (`Task::children_mut`). Before, the
/// `AllowFailure` arm walked `tasks` only: a `Subflow` inside `finally:` was
/// never resolved and got no warning. Now it gets what a top-level one gets.
/// (An authored `flowRef` there is refused as an unknown property.)
#[tokio::test]
async fn a_subflow_inside_an_allow_failure_finally_is_resolved_and_warned() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let source = "id: f\nnamespace: a.b\ntasks:\n  - id: af\n    type: io.kestra.plugin.core.flow.AllowFailure\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n    finally:\n      - id: h\n        type: core.subflow\n        namespace: company.analytics\n        flowId: nightly_rollup\n";
    let f = common::draft(&repo, ws, user, source).await;

    let v = egret_workflow::publish(&repo, &common::workflow_refs(), ws, user, f.id, "")
        .await
        .expect("publishes");
    let h = &v.snapshot["plan"]["tasks"][0]["finally"][0];
    // The path is real, so `flowRef` being absent is not vacuous.
    assert_eq!(h["flowId"], "nightly_rollup", "{}", v.snapshot);
    assert!(h["flowRef"].is_null(), "{}", v.snapshot);
    assert_eq!(
        v.snapshot["warnings"],
        serde_json::json!([
            {
                "code": "subflow_not_found",
                "message": subflow_warning("company.analytics", "nightly_rollup"),
                "path": "tasks.af.h",
            },
        ]),
    );
}
