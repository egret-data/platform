//! Publish, and the reference-resolution seam. Requires DATABASE_URL.

mod common;

use egret_workflow::{RefKind, WorkflowRefPort};
use uuid::Uuid;

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
    let Some(db) = common::db().await else { return };
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
    let Some(db) = common::db().await else { return };
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
    let Some(db) = common::db().await else { return };
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
    let Some(db) = common::db().await else { return };
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
    let Some(db) = common::db().await else { return };
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

// -- Fix round 1: `subflow:` was the sixth row in spec section 8's own
// reference table (`workflow.flows`, "this crate") and had no resolution at
// all -- no `RefField`, no arm in `resolve_task`. Same three-way coverage as
// `database:` above, plus the mutation below.

/// The subflow twin of `a_flow_referencing_another_workspace_cannot_be_published`.
#[tokio::test]
async fn a_subflow_naming_another_workspaces_flow_cannot_be_published() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let other_ws = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Subflow, "company.analytics/nightly_rollup".into()),
        (Uuid::new_v4(), other_ws), // owned by SOMEONE ELSE
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect_err("publishing a foreign subflow reference must fail");
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

/// The subflow twin of `an_unknown_reference_fails_the_same_way_as_a_foreign_one`.
#[tokio::test]
async fn a_subflow_naming_a_nonexistent_flow_fails_the_same_way() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nope\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("must fail");
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err:?}");
}

/// The subflow twin of `a_resolvable_flow_publishes_and_freezes_its_references`.
#[tokio::test]
async fn a_subflow_naming_a_flow_in_the_callers_own_workspace_publishes_and_freezes_its_id() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let target_id = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Subflow, "company.analytics/nightly_rollup".into()),
        (target_id, ws),
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n",
    )
    .await;

    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect("publish");

    let s = v.snapshot.to_string();
    assert!(
        s.contains(&target_id.to_string()),
        "snapshot must carry the resolved subflow id: {s}"
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
    let Some(db) = common::db().await else { return };
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
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let target_id = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Subflow, "company.analytics/nightly_rollup".into()),
        (target_id, ws),
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: nightly_rollup\n",
    )
    .await;

    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect("a legitimate dotted namespace must publish");
    let s = v.snapshot.to_string();
    assert!(
        s.contains(&target_id.to_string()),
        "snapshot must carry the resolved id: {s}"
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
// no unresolved id") was false for this one shape. These three tests are
// the trigger twins of the `TaskKind::Subflow` tests just above, and prove
// the identical three outcomes now hold for `dependsOn` too.

fn flow_with_trigger_dependency(namespace: &str, flow_id: &str) -> String {
    format!(
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
         triggers:\n  - id: chain\n    type: core.flow\n    dependsOn:\n      \
         - namespace: {namespace}\n        flowId: {flow_id}\n"
    )
}

/// The trigger twin of
/// `a_subflow_naming_a_flow_in_the_callers_own_workspace_publishes_and_freezes_its_id`.
#[tokio::test]
async fn a_trigger_dependency_naming_a_flow_in_the_callers_own_workspace_publishes_and_freezes_its_id(
) {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let target_id = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Subflow, "company.analytics/nightly_rollup".into()),
        (target_id, ws),
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        &flow_with_trigger_dependency("company.analytics", "nightly_rollup"),
    )
    .await;

    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect("publish");
    let s = v.snapshot.to_string();
    assert!(
        s.contains(&target_id.to_string()),
        "snapshot must carry the resolved trigger dependency id, not the bare name: {s}"
    );
}

/// The trigger twin of
/// `a_subflow_naming_another_workspaces_flow_cannot_be_published`.
#[tokio::test]
async fn a_trigger_dependency_naming_another_workspaces_flow_cannot_be_published() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let other_ws = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Subflow, "company.analytics/nightly_rollup".into()),
        (Uuid::new_v4(), other_ws), // owned by SOMEONE ELSE
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        &flow_with_trigger_dependency("company.analytics", "nightly_rollup"),
    )
    .await;

    let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect_err("a foreign trigger dependency must fail publish");
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "expected NotFound, got {err:?} -- before this fix the flow published successfully \
         with the dependency frozen UNRESOLVED"
    );
}

/// The trigger twin of `a_subflow_naming_a_nonexistent_flow_fails_the_same_way`.
#[tokio::test]
async fn a_trigger_dependency_naming_a_nonexistent_flow_fails_the_same_way() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo,
        ws,
        user,
        &flow_with_trigger_dependency("company.analytics", "nope"),
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("must fail");
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err:?}");
}

/// W9b: the save-time refusal of a namespace with a `_files` segment, through
/// publish (its unit twin is `spec::validate`'s
/// `publish_refuses_a_files_segment_namespace_by_name`; the run-time twin is
/// `nsfiles::access`'s `a_files_segment_namespace_is_refused_for_every_access_and_actor`).
#[tokio::test]
async fn publish_refuses_a_files_segment_namespace_by_name() {
    let Some(db) = common::db().await else { return };
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
    let Some(db) = common::db().await else { return };
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
/// malformed block is refused naming the property, a valid one publishes.
#[tokio::test]
async fn a_working_directory_with_a_malformed_namespace_files_is_refused_by_name_and_a_valid_one_publishes(
) {
    let Some(db) = common::db().await else { return };
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
    // The control: the corpus flow's block, and a templated one, publish.
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
            .unwrap_or_else(|e| panic!("{id} publishes: {e:?}"));
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
    let Some(db) = common::db().await else { return };
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
/// `secret()` either; it stopped only at the unknown `kv.Set` type until W9c
/// Task 5 registered the six KV tasks, and it publishes now (it still needs
/// the `AllowFailure` flowable to run as Kestra ran it).
#[tokio::test]
async fn valids_kv_and_retry_with_flowable_errors_publish() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    for source in [
        include_str!("../oracle-exec/corpus/valids/kv.yaml"),
        include_str!("../oracle-exec/corpus/valids/retry-with-flowable-errors.yaml"),
    ] {
        let draft = common::draft(&repo, ws, user, source).await;
        egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, draft.id, "")
            .await
            .unwrap_or_else(|e| panic!("{e}: {source}"));
    }
}
