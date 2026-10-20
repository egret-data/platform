//! Observing that an engine's pods have rolled out with its CURRENT Ranger
//! credential (spec 2026-10-20-ranger-surface-auth A10).
//!
//! egret retires an engine's previous Ranger credential only once the rollout of
//! the current one is OBSERVED complete — not when Kubernetes merely accepts the
//! apply. Until then a pod of the old template still holds the old password in
//! its mounted copy of the Secret, and deleting it would make that pod's policy
//! download fail (revocations and masks would silently stop applying). Standing
//! rule: never delete a secret value the cluster may still hold.
//!
//! **Tied to one credential.** The provisioners stamp the credential's
//! non-secret id ([`RANGER_CREDENTIAL_ANNOTATION`]) on the pod template of
//! every workload that runs the plugin, and the password is rendered into a
//! Secret of its own, `{name}-ranger-{id}` ([`ranger_secret_name`]), which
//! those pod templates mount: a pod can only hold the content of the
//! credential its template names. A probe asks about ONE id, so a rollout that
//! completed for an older apply (an older credential) never answers for a
//! newer rotation.
//!
//! **Fail closed.** Anything absent, unknown or partial is
//! [`RangerRollout::Pending`]; a read that fails is an `Err`. Either way the
//! caller keeps the previous credential. In particular a workload scaled to
//! zero replicas is never "complete".
//!
//! What counts as complete:
//!
//! * the current credential's own Secret exists (and names the id): pods of
//!   its template cannot have started before it was written;
//! * each workload (Trino: the coordinator and worker Deployments; StarRocks:
//!   the FE StatefulSet — the CN runs no plugin) has the id on its pod
//!   template, its status reflects its latest spec (`observedGeneration ==
//!   generation`), and every replica is updated and ready, with none of an
//!   older template left (Deployment: `updated == replicas == ready ==
//!   available == spec.replicas`, nothing unavailable or terminating;
//!   StatefulSet: `currentRevision == updateRevision` and `updated == replicas
//!   == ready == spec.replicas`);
//! * every non-terminal pod of the plugin-running roles carries the id, is
//!   Running and Ready, and is not terminating — and there is at least one.
//!   A pod that has finished (`Succeeded`/`Failed`, e.g. evicted) runs nothing
//!   and is ignored.

use serde_json::Value;

use crate::kube_client::KubeClient;
use crate::spec::{EngineSpec, RangerRollout};

/// The pod-template (and Secret) annotation naming the Ranger credential the
/// pods were rendered with: its non-secret id, never the password.
pub const RANGER_CREDENTIAL_ANNOTATION: &str = "egret/ranger-credential";

/// The `egret/component` label value of every per-credential Ranger Secret, so
/// an engine's teardown can list exactly its own (with `egret/engine={name}`).
pub const RANGER_SECRET_COMPONENT: &str = "ranger-credential";

/// The Secret an engine bound WITHOUT a credential is rendered into, and the
/// one every engine used before the Secret was named per credential (Task 6b).
pub fn legacy_ranger_secret_name(engine: &str) -> String {
    format!("{engine}-ranger-secret")
}

/// The Secret the plugin's security XML is rendered into: one PER CREDENTIAL,
/// `{engine}-ranger-{credential id}` (the id is not secret).
///
/// Per credential because the XML is mounted by `subPath`, which keeps the
/// content a pod saw at start: with one shared name, two interleaved applies
/// could leave pods of the NEW credential's template mounting the OLD
/// credential's content, and a later re-apply could rewrite the Secret without
/// rolling them — the probe would then say complete while every pod still held
/// the previous credential. A pod built from a template that names credential
/// X can only ever mount X's Secret, which is written once with X's content.
pub fn ranger_secret_name(engine: &str, credential_id: Option<&str>) -> String {
    match credential_id {
        Some(id) => format!("{engine}-ranger-{id}"),
        None => legacy_ranger_secret_name(engine),
    }
}

/// The id of the credential `spec` renders, if it renders one.
pub(crate) fn ranger_credential_id(spec: &EngineSpec) -> Option<&str> {
    spec.ranger
        .as_ref()
        .and_then(|r| r.credential.as_ref())
        .map(|c| c.id())
}

/// [`ranger_secret_name`] of what `spec` renders.
pub(crate) fn spec_ranger_secret_name(spec: &EngineSpec) -> String {
    ranger_secret_name(&spec.name, ranger_credential_id(spec))
}

/// Every Ranger Secret of engine `engine` in `namespace`: the per-credential
/// ones, listed by their labels (`egret/engine={engine}` exactly, never a name
/// prefix another engine's name could share), and the legacy name.
pub(crate) async fn all_ranger_secret_names(
    kube: &dyn KubeClient,
    namespace: &str,
    engine: &str,
) -> anyhow::Result<Vec<String>> {
    let mut names = kube
        .list_object_names(
            "v1",
            "Secret",
            namespace,
            &format!("egret/engine={engine},egret/component={RANGER_SECRET_COMPONENT}"),
        )
        .await?;
    let legacy = legacy_ranger_secret_name(engine);
    if !names.contains(&legacy) {
        names.push(legacy);
    }
    Ok(names)
}

/// Unbind: delete every Ranger Secret of `engine`, best-effort (logged).
pub(crate) async fn delete_all_ranger_secrets_best_effort(
    kube: &dyn KubeClient,
    namespace: &str,
    engine: &str,
) {
    let names = all_ranger_secret_names(kube, namespace, engine)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(engine, error = %e, "could not list the engine's ranger Secrets");
            vec![legacy_ranger_secret_name(engine)]
        });
    for name in names {
        let _ = kube.delete("v1", "Secret", namespace, &name).await;
    }
}

/// Engine delete: `objects` plus every Ranger Secret of `engine`. All are
/// attempted; a failure to list them is an error too, after the rest.
pub(crate) async fn delete_with_ranger_secrets(
    kube: &dyn KubeClient,
    namespace: &str,
    engine: &str,
    objects: &[(&str, &str, String)],
) -> anyhow::Result<()> {
    let mut all = objects.to_vec();
    let listed = all_ranger_secret_names(kube, namespace, engine).await;
    if let Ok(names) = &listed {
        for name in names {
            if !all.iter().any(|(_, k, n)| *k == "Secret" && n == name) {
                all.push(("v1", "Secret", name.clone()));
            }
        }
    }
    let deleted = crate::kube_client::delete_objects(kube, namespace, &all).await;
    match listed {
        Ok(_) => deleted,
        Err(e) => Err(match deleted {
            Ok(()) => e.context("could not list the engine's ranger Secrets"),
            Err(d) => d.context(format!("could not list the engine's ranger Secrets: {e:#}")),
        }),
    }
}

/// Delete the Secret of a RETIRED credential, `credential_id`, by its exact
/// name — and the legacy-named Secret, which no pod mounts once a rollout of a
/// per-credential Secret has been observed complete. Called only after that
/// observation (`EngineProvisioner::delete_ranger_secret`).
pub(crate) async fn delete_retired_ranger_secret(
    kube: &dyn KubeClient,
    namespace: &str,
    engine: &str,
    credential_id: &str,
) -> anyhow::Result<()> {
    crate::kube_client::delete_objects(
        kube,
        namespace,
        &[
            (
                "v1",
                "Secret",
                ranger_secret_name(engine, Some(credential_id)),
            ),
            ("v1", "Secret", legacy_ranger_secret_name(engine)),
        ],
    )
    .await
}

fn name_of(obj: &Value) -> &str {
    obj["metadata"]["name"].as_str().unwrap_or("<unnamed>")
}

/// The credential id an annotation map names, if any.
fn annotated(annotations: &Value) -> Option<&str> {
    annotations[RANGER_CREDENTIAL_ANNOTATION].as_str()
}

/// A status count. The API server omits a count that is zero, so absent is 0.
fn count(obj: &Value, field: &str) -> i64 {
    obj["status"][field].as_i64().unwrap_or(0)
}

/// The template names `expected`, and the status reflects the latest spec.
fn check_template_and_generation<'a>(
    kind: &str,
    obj: Option<&'a Value>,
    expected: &str,
) -> Result<(&'a Value, i64), String> {
    let obj = obj.ok_or_else(|| format!("{kind} not found"))?;
    let name = name_of(obj);
    if annotated(&obj["spec"]["template"]["metadata"]["annotations"]) != Some(expected) {
        return Err(format!(
            "{kind} {name}: its pod template is not rendered with the current credential"
        ));
    }
    let generation = obj["metadata"]["generation"].as_i64();
    let observed = obj["status"]["observedGeneration"].as_i64();
    if generation.is_none() || observed != generation {
        return Err(format!(
            "{kind} {name}: its controller has not observed the latest spec yet"
        ));
    }
    let want = obj["spec"]["replicas"]
        .as_i64()
        .ok_or_else(|| format!("{kind} {name}: no spec.replicas"))?;
    if want <= 0 {
        return Err(format!(
            "{kind} {name}: scaled to zero replicas, so no rollout can be observed"
        ));
    }
    Ok((obj, want))
}

/// The Secret's check: it was last written with credential `expected`.
pub(crate) fn check_secret(obj: Option<&Value>, expected: &str) -> Result<(), String> {
    let obj = obj.ok_or("the current credential's ranger Secret not found")?;
    if annotated(&obj["metadata"]["annotations"]) != Some(expected) {
        return Err(format!(
            "Secret {}: it does not hold the current credential",
            name_of(obj)
        ));
    }
    Ok(())
}

/// A Deployment's check (see the module doc).
pub(crate) fn check_deployment(obj: Option<&Value>, expected: &str) -> Result<(), String> {
    let (obj, want) = check_template_and_generation("Deployment", obj, expected)?;
    let (updated, replicas, ready, available) = (
        count(obj, "updatedReplicas"),
        count(obj, "replicas"),
        count(obj, "readyReplicas"),
        count(obj, "availableReplicas"),
    );
    let (unavailable, terminating) = (
        count(obj, "unavailableReplicas"),
        count(obj, "terminatingReplicas"),
    );
    if [updated, replicas, ready, available] != [want; 4] || unavailable != 0 || terminating != 0 {
        return Err(format!(
            "Deployment {}: rollout in progress ({updated} of {want} updated, {replicas} \
             replicas, {ready} ready, {available} available, {unavailable} unavailable, \
             {terminating} terminating)",
            name_of(obj)
        ));
    }
    Ok(())
}

/// A StatefulSet's check (see the module doc).
pub(crate) fn check_statefulset(obj: Option<&Value>, expected: &str) -> Result<(), String> {
    let (obj, want) = check_template_and_generation("StatefulSet", obj, expected)?;
    let name = name_of(obj);
    let current = obj["status"]["currentRevision"].as_str();
    let update = obj["status"]["updateRevision"].as_str();
    if current.is_none() || current != update {
        return Err(format!(
            "StatefulSet {name}: its pods are not all on the latest revision"
        ));
    }
    let (updated, replicas, ready) = (
        count(obj, "updatedReplicas"),
        count(obj, "replicas"),
        count(obj, "readyReplicas"),
    );
    if [updated, replicas, ready] != [want; 3] {
        return Err(format!(
            "StatefulSet {name}: rollout in progress ({updated} of {want} updated, \
             {replicas} replicas, {ready} ready)"
        ));
    }
    Ok(())
}

/// The pods' check (see the module doc).
pub(crate) fn check_pods(pods: &[Value], expected: &str) -> Result<(), String> {
    let running: Vec<&Value> = pods
        .iter()
        .filter(|p| !matches!(p["status"]["phase"].as_str(), Some("Succeeded" | "Failed")))
        .collect();
    if running.is_empty() {
        return Err("no running pod".into());
    }
    for pod in running {
        let name = name_of(pod);
        if !pod["metadata"]["deletionTimestamp"].is_null() {
            return Err(format!("pod {name}: still terminating"));
        }
        if annotated(&pod["metadata"]["annotations"]) != Some(expected) {
            return Err(format!(
                "pod {name}: still runs a template older than the current credential's"
            ));
        }
        let ready = pod["status"]["conditions"].as_array().is_some_and(|cs| {
            cs.iter()
                .any(|c| c["type"] == "Ready" && c["status"] == "True")
        });
        if pod["status"]["phase"].as_str() != Some("Running") || !ready {
            return Err(format!("pod {name}: not running and ready"));
        }
    }
    Ok(())
}

/// A workload that runs the Ranger plugin.
pub(crate) enum Workload {
    Deployment(String),
    StatefulSet(String),
}

/// Read the current credential's Secret (`{engine}-ranger-{expected}`), the
/// workloads and the pods, and judge them all against `expected`. The first check that does not hold is the `Pending` reason.
pub(crate) async fn observe(
    kube: &dyn KubeClient,
    namespace: &str,
    engine: &str,
    workloads: &[Workload],
    pod_selector: &str,
    expected: &str,
) -> anyhow::Result<RangerRollout> {
    let secret = kube
        .get_object_metadata(
            "v1",
            "Secret",
            namespace,
            &ranger_secret_name(engine, Some(expected)),
        )
        .await?;
    let mut verdict = check_secret(secret.as_ref(), expected);
    for w in workloads {
        let checked = match w {
            Workload::Deployment(name) => {
                let obj = kube
                    .get_object("apps/v1", "Deployment", namespace, name)
                    .await?;
                check_deployment(obj.as_ref(), expected)
            }
            Workload::StatefulSet(name) => {
                let obj = kube
                    .get_object("apps/v1", "StatefulSet", namespace, name)
                    .await?;
                check_statefulset(obj.as_ref(), expected)
            }
        };
        verdict = verdict.and(checked);
    }
    let pods = kube.list_pod_objects(namespace, pod_selector).await?;
    verdict = verdict.and(check_pods(&pods, expected));
    Ok(match verdict {
        Ok(()) => RangerRollout::Complete,
        Err(reason) => RangerRollout::Pending(reason),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn rolled_deployment(name: &str, cred: &str) -> Value {
        json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": { "name": name, "generation": 3 },
            "spec": { "replicas": 2, "template": { "metadata": {
                "annotations": { RANGER_CREDENTIAL_ANNOTATION: cred } } } },
            "status": { "observedGeneration": 3, "replicas": 2, "updatedReplicas": 2,
                        "readyReplicas": 2, "availableReplicas": 2 }
        })
    }

    pub(crate) fn rolled_statefulset(name: &str, cred: &str) -> Value {
        json!({
            "apiVersion": "apps/v1", "kind": "StatefulSet",
            "metadata": { "name": name, "generation": 5 },
            "spec": { "replicas": 1, "template": { "metadata": {
                "annotations": { RANGER_CREDENTIAL_ANNOTATION: cred } } } },
            "status": { "observedGeneration": 5, "replicas": 1, "updatedReplicas": 1,
                        "readyReplicas": 1, "currentReplicas": 1,
                        "currentRevision": "fe-7d9", "updateRevision": "fe-7d9" }
        })
    }

    pub(crate) fn ready_pod(name: &str, labels: Value, cred: &str) -> Value {
        json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": { "name": name, "labels": labels,
                          "annotations": { RANGER_CREDENTIAL_ANNOTATION: cred } },
            "status": { "phase": "Running",
                        "conditions": [{ "type": "Ready", "status": "True" }] }
        })
    }

    pub(crate) fn secret(name: &str, cred: &str) -> Value {
        json!({
            "apiVersion": "v1", "kind": "Secret",
            "metadata": { "name": name,
                          "annotations": { RANGER_CREDENTIAL_ANNOTATION: cred } }
        })
    }

    /// `obj` with the JSON-pointer `path` set to `v`.
    fn with(mut obj: Value, path: &str, v: Value) -> Value {
        *obj.pointer_mut(path).unwrap_or_else(|| panic!("{path}")) = v;
        obj
    }

    fn without(mut obj: Value, parent: &str, key: &str) -> Value {
        obj.pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("{parent}"))
            .remove(key);
        obj
    }

    #[test]
    fn a_deployment_is_rolled_only_when_every_replica_runs_the_credential() {
        let ok = rolled_deployment("d", "c2");
        assert_eq!(check_deployment(Some(&ok), "c2"), Ok(()));

        let not_rolled = [
            ("absent", None),
            (
                "older credential on the template",
                Some(with(
                    ok.clone(),
                    "/spec/template/metadata/annotations/egret~1ranger-credential",
                    json!("c1"),
                )),
            ),
            (
                "no annotation",
                Some(without(
                    ok.clone(),
                    "/spec/template/metadata/annotations",
                    RANGER_CREDENTIAL_ANNOTATION,
                )),
            ),
            (
                "status not caught up with the spec",
                Some(with(ok.clone(), "/status/observedGeneration", json!(2))),
            ),
            (
                "no observedGeneration",
                Some(without(ok.clone(), "/status", "observedGeneration")),
            ),
            (
                "a replica not updated",
                Some(with(ok.clone(), "/status/updatedReplicas", json!(1))),
            ),
            (
                "a surplus old replica",
                Some(with(ok.clone(), "/status/replicas", json!(3))),
            ),
            (
                "a replica not ready",
                Some(with(ok.clone(), "/status/readyReplicas", json!(1))),
            ),
            (
                "a replica not available",
                Some(with(ok.clone(), "/status/availableReplicas", json!(1))),
            ),
            (
                "an unavailable replica",
                Some(with(
                    ok.clone(),
                    "/status",
                    json!({ "observedGeneration": 3, "replicas": 2, "updatedReplicas": 2,
                            "readyReplicas": 2, "availableReplicas": 2,
                            "unavailableReplicas": 1 }),
                )),
            ),
            (
                "a terminating replica",
                Some(with(
                    ok.clone(),
                    "/status",
                    json!({ "observedGeneration": 3, "replicas": 2, "updatedReplicas": 2,
                            "readyReplicas": 2, "availableReplicas": 2,
                            "terminatingReplicas": 1 }),
                )),
            ),
            ("no status at all", Some(without(ok.clone(), "", "status"))),
            (
                "no spec.replicas",
                Some(without(ok.clone(), "/spec", "replicas")),
            ),
            (
                "scaled to zero",
                Some(with(
                    with(ok.clone(), "/spec/replicas", json!(0)),
                    "/status",
                    json!({ "observedGeneration": 3 }),
                )),
            ),
        ];
        for (why, obj) in not_rolled {
            assert!(
                check_deployment(obj.as_ref(), "c2").is_err(),
                "{why} must not count as rolled"
            );
        }
    }

    #[test]
    fn a_statefulset_is_rolled_only_on_one_revision_with_every_replica_ready() {
        let ok = rolled_statefulset("fe", "c2");
        assert_eq!(check_statefulset(Some(&ok), "c2"), Ok(()));

        let not_rolled = [
            ("absent", None),
            (
                "older credential on the template",
                Some(with(
                    ok.clone(),
                    "/spec/template/metadata/annotations/egret~1ranger-credential",
                    json!("c1"),
                )),
            ),
            (
                "two revisions",
                Some(with(ok.clone(), "/status/currentRevision", json!("fe-1a2"))),
            ),
            (
                "no update revision",
                Some(without(ok.clone(), "/status", "updateRevision")),
            ),
            (
                "status not caught up with the spec",
                Some(with(ok.clone(), "/status/observedGeneration", json!(4))),
            ),
            (
                "a replica not updated",
                Some(with(ok.clone(), "/status/updatedReplicas", json!(0))),
            ),
            (
                "a replica not ready",
                Some(with(ok.clone(), "/status/readyReplicas", json!(0))),
            ),
            (
                "a surplus replica",
                Some(with(ok.clone(), "/status/replicas", json!(2))),
            ),
            (
                "scaled to zero",
                Some(with(
                    with(ok.clone(), "/spec/replicas", json!(0)),
                    "/status",
                    json!({ "observedGeneration": 5, "currentRevision": "fe-7d9",
                            "updateRevision": "fe-7d9" }),
                )),
            ),
        ];
        for (why, obj) in not_rolled {
            assert!(
                check_statefulset(obj.as_ref(), "c2").is_err(),
                "{why} must not count as rolled"
            );
        }
    }

    #[test]
    fn every_running_pod_must_run_the_credential_and_be_ready() {
        let labels = json!({ "app": "x" });
        let a = ready_pod("a", labels.clone(), "c2");
        let b = ready_pod("b", labels.clone(), "c2");
        assert_eq!(check_pods(&[a.clone(), b.clone()], "c2"), Ok(()));

        // A finished pod (evicted: `Failed`) runs nothing, whatever it was.
        let evicted = with(
            ready_pod("e", labels.clone(), "c1"),
            "/status/phase",
            json!("Failed"),
        );
        assert_eq!(check_pods(&[a.clone(), evicted.clone()], "c2"), Ok(()));

        let not_rolled: [(&str, Vec<Value>); 6] = [
            ("no pod", vec![]),
            ("only finished pods", vec![evicted]),
            (
                "an old pod still running",
                vec![a.clone(), ready_pod("old", labels.clone(), "c1")],
            ),
            (
                "a pod with no annotation",
                vec![
                    a.clone(),
                    without(
                        ready_pod("n", labels.clone(), "c2"),
                        "/metadata",
                        "annotations",
                    ),
                ],
            ),
            (
                "a pod not ready",
                vec![
                    a.clone(),
                    with(
                        b.clone(),
                        "/status/conditions",
                        json!([{ "type": "Ready", "status": "False" }]),
                    ),
                ],
            ),
            (
                "a terminating pod",
                vec![
                    a.clone(),
                    with(
                        b.clone(),
                        "/metadata",
                        json!({ "name": "b", "deletionTimestamp": "2026-10-20T00:00:00Z",
                                "annotations": { RANGER_CREDENTIAL_ANNOTATION: "c2" } }),
                    ),
                ],
            ),
        ];
        for (why, pods) in not_rolled {
            assert!(
                check_pods(&pods, "c2").is_err(),
                "{why} must not count as rolled"
            );
        }
        let pending = with(b, "/status/phase", json!("Pending"));
        assert!(check_pods(&[a, pending], "c2").is_err(), "a Pending pod");
    }

    #[test]
    fn the_secret_must_name_the_credential() {
        assert_eq!(check_secret(Some(&secret("s", "c2")), "c2"), Ok(()));
        assert!(check_secret(Some(&secret("s", "c1")), "c2").is_err());
        assert!(check_secret(None, "c2").is_err());
        assert!(check_secret(Some(&json!({ "metadata": { "name": "s" } })), "c2").is_err());
    }

    /// The reasons are logged: none may carry anything but names and counts.
    #[test]
    fn a_pending_reason_names_the_object() {
        let stalled = with(
            rolled_deployment("demo-worker", "c2"),
            "/status/updatedReplicas",
            json!(1),
        );
        let reason = check_deployment(Some(&stalled), "c2").unwrap_err();
        assert!(reason.contains("demo-worker"), "{reason}");
    }
}
