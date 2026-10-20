//! StarRocks shared-data templating + provisioner. Emits the kube-starrocks
//! operator's `starrocks.com/v1 StarRocksCluster` CR plus its `v1/ConfigMap`s;
//! unlike Trino (plain Deployments), the operator owns the StatefulSets/pods
//! for the FE (metadata) and CN (stateless compute) roles. Shared-data mode
//! keeps table data in the object store, configured via `fe.conf`'s builtin
//! storage-volume block — there is no BE role.

use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{
    EngineSpec, EngineStatus, PodStatus, RangerRollout, RoleResources, StarRocksPod,
    DEFAULT_FE_META_STORAGE_SIZE,
};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Render `RoleResources` as k8s `limits`/`requests`, omitting empty sides —
/// mirrors `trino.rs`'s `trino_deployment` resource rendering, but the
/// operator's CR takes `limits`/`requests` directly on the role spec (no
/// nested `resources` object).
fn resources_json(r: &RoleResources) -> (Value, Value) {
    let mut limits = serde_json::Map::new();
    let mut requests = serde_json::Map::new();
    if let Some(v) = &r.cpu_limit {
        limits.insert("cpu".into(), json!(v));
    }
    if let Some(v) = &r.mem_limit {
        limits.insert("memory".into(), json!(v));
    }
    if let Some(v) = &r.cpu_request {
        requests.insert("cpu".into(), json!(v));
    }
    if let Some(v) = &r.mem_request {
        requests.insert("memory".into(), json!(v));
    }
    (Value::Object(limits), Value::Object(requests))
}

/// Merge admin `fe.conf` overrides into the egret-generated base. Each base line
/// of the form `key = value` (or `key=value`) whose key appears in `overrides`
/// is REPLACED in place (preserving position); overrides whose key isn't in the
/// base are appended. Keys in `protected` are SKIPPED (egret's value wins) —
/// belt-and-suspenders behind the API-layer 400 guard. Comment/blank lines are
/// preserved verbatim.
fn merge_fe_conf(
    base: &str,
    overrides: &std::collections::BTreeMap<String, String>,
    protected: &[&str],
) -> String {
    let mut consumed: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    // Preserve a trailing newline: `split('\n')` on "a\n" yields ["a", ""], so
    // the empty final element is re-joined with the same '\n' separator.
    let mut out_lines: Vec<String> = Vec::new();
    for line in base.split('\n') {
        // Only `key = value` lines are candidates; comments/blanks pass through.
        let is_kv =
            !line.trim_start().starts_with('#') && line.contains('=') && !line.trim().is_empty();
        if is_kv {
            let (raw_key, _) = line.split_once('=').expect("contains('=') checked above");
            let key = raw_key.trim();
            if !protected.contains(&key) {
                if let Some((k, v)) = overrides.get_key_value(key) {
                    out_lines.push(format!("{k} = {v}"));
                    consumed.insert(k.as_str());
                    continue;
                }
            }
        }
        out_lines.push(line.to_string());
    }
    let mut out = out_lines.join("\n");
    // Append any override key not present in the base (and not protected), in
    // sorted BTreeMap order for determinism. Ensure the appended block starts on
    // its own line.
    for (k, v) in overrides {
        if consumed.contains(k.as_str()) || protected.contains(&k.as_str()) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{k} = {v}\n"));
    }
    out
}

/// The FE role's `fe.conf` ConfigMap (`{name}-fe-cm`). Templates the base
/// ports/log block, the `run_mode = shared_data` object-store volume config
/// sourced from `spec.starrocks`, and — only when a Ranger binding is present
/// — `access_control = ranger`.
pub fn starrocks_fe_configmap(spec: &EngineSpec) -> Value {
    let sr = spec
        .starrocks
        .as_ref()
        .expect("starrocks_fe_configmap requires spec.starrocks");

    let mut conf = String::new();
    conf.push_str("LOG_DIR = ${STARROCKS_HOME}/log\n");
    conf.push_str("DATE = \"$(date +%Y%m%d-%H%M%S)\"\n");
    conf.push_str(
        "JAVA_OPTS=\"-Dlog4j2.formatMsgNoLookups=true -Xmx8192m -XX:+UseG1GC -Xlog:gc*:${LOG_DIR}/fe.gc.log.$DATE:time\"\n",
    );
    conf.push_str("http_port = 8030\n");
    conf.push_str("rpc_port = 9020\n");
    conf.push_str("query_port = 9030\n");
    conf.push_str("edit_log_port = 9010\n");
    conf.push_str("mysql_service_nio_enabled = true\n");
    conf.push_str("sys_log_level = INFO\n");
    conf.push_str("# config for shared-data mode\n");
    conf.push_str("run_mode = shared_data\n");
    conf.push_str("cloud_native_meta_port = 6090\n");
    conf.push_str("cloud_native_storage_type = S3\n");
    conf.push_str(&format!("aws_s3_endpoint = {}\n", sr.storage_endpoint));
    conf.push_str(&format!("aws_s3_path = {}\n", sr.storage_path));
    conf.push_str(&format!("aws_s3_access_key = {}\n", sr.storage_access_key));
    conf.push_str(&format!("aws_s3_secret_key = {}\n", sr.storage_secret_key));
    conf.push_str("aws_s3_use_aws_sdk_default_behavior = false\n");
    conf.push_str("aws_s3_enable_path_style_access = true\n");
    conf.push_str("enable_load_volume_from_conf = true\n");
    if spec.ranger.is_some() {
        conf.push_str(crate::ranger_config::STARROCKS_ACCESS_CONTROL_LINE);
    }
    // FE TLS on :9030 (Access delegation parity, slice 3): the exact `ssl_*`
    // keys are LIVE-ONLY (the T4 smoke tunes them against the `fe-ubuntu`
    // image); this documented set is the starting point. `ssl_force_secure_transport
    // = false` allows plaintext to keep working alongside TLS in dev — the
    // connector (S3-T1) opts into TLS per-connection via `options.tls_cert`.
    if let Some(d) = sr.delegation.as_ref().filter(|d| d.tls_enabled) {
        let _ = d;
        conf.push_str("ssl_keystore_location = /etc/starrocks/ssl/keystore\n");
        conf.push_str("ssl_keystore_password = changeit\n");
        conf.push_str("ssl_key_password = changeit\n");
        conf.push_str("ssl_force_secure_transport = false\n");
    }

    // Merge admin `fe.conf` overrides over the egret-generated base. Protected
    // keys are guarded here too (defense in depth behind the API-layer 400).
    let merged = merge_fe_conf(&conf, &sr.fe_config, crate::spec::SR_PROTECTED_FE_KEYS);

    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-fe-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { "fe.conf": merged }
    })
}

/// The CN role's `cn.conf` ConfigMap (`{name}-cn-cm`). Unlike the FE, there is
/// NO egret-mandated CN base — `cn.conf` is JUST the admin `cn_config`
/// overrides, so this returns `None` when `cn_config` is empty (CN then uses
/// the operator defaults, unchanged from today). Lines are rendered `key =
/// value`, sorted by the `BTreeMap`'s key order for determinism.
pub fn starrocks_cn_configmap(spec: &EngineSpec) -> Option<Value> {
    let sr = spec
        .starrocks
        .as_ref()
        .expect("starrocks_cn_configmap requires spec.starrocks");
    if sr.cn_config.is_empty() {
        return None;
    }
    let conf: String = sr
        .cn_config
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect();
    Some(json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-cn-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { "cn.conf": conf }
    }))
}

/// The directory the StarRocks FE image ships its configuration in. The
/// Ranger XML files are mounted as INDIVIDUAL files inside it (see
/// [`starrocks_cluster_cr`]) — never as a directory mount, which would
/// shadow `fe.conf` and stop the FE booting.
const FE_CONF_DIR: &str = "/opt/starrocks/fe/conf";
/// Key (and file name) of the Ranger plugin security config in `{name}-ranger-secret`.
const RANGER_SECURITY_XML_KEY: &str = crate::ranger_config::STARROCKS_SECURITY_XML;
/// Key (and file name) of the Ranger plugin audit config in `{name}-ranger-cm`.
const RANGER_AUDIT_XML_KEY: &str = crate::ranger_config::STARROCKS_AUDIT_XML;

/// The Ranger plugin's files for `spec`, `None` unless `spec.ranger` is bound.
fn ranger_files(spec: &EngineSpec) -> Option<crate::ranger_config::RangerPluginFiles> {
    let r = spec.ranger.as_ref()?;
    Some(crate::ranger_config::ranger_plugin_files(
        egret_engine_mgmt_api::EngineKind::StarRocks,
        &r.service_name,
        &r.egret_url,
        r.poll_interval_ms.into(),
        r.credential_ref(),
    ))
}

/// The Ranger plugin's audit ConfigMap (`{name}-ranger-cm`). `None` unless
/// `spec.ranger` is bound. The security XML is NOT here: it carries the
/// plugin's credential, so it is in [`starrocks_ranger_secret`].
///
/// CRITICAL: no XML comment here may contain a `--` (double-hyphen) sequence
/// — that crashes the StarRocks FE boot (proven live in an earlier slice).
/// This renderer emits no comments at all to stay safe by construction.
pub fn starrocks_ranger_configmap(spec: &EngineSpec) -> Option<Value> {
    let rendered = ranger_files(spec)?;
    let audit_xml = rendered
        .get(RANGER_AUDIT_XML_KEY)
        .expect("the renderer emits it");
    Some(json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-ranger-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": {
            RANGER_AUDIT_XML_KEY: audit_xml,
        }
    }))
}

/// The `{name}-ranger-secret` Secret: the Ranger plugin's security XML, which
/// carries the engine's `/ranger` credential (spec 2026-10-20-ranger-surface-auth
/// A9). `None` unless `spec.ranger` is bound. Mounted into the FE by `subPath`
/// through the CR's `secrets` field, so [`fe_config_checksum`] hashes it.
pub fn starrocks_ranger_secret(spec: &EngineSpec) -> Option<Value> {
    let rendered = ranger_files(spec)?;
    let security_xml = rendered
        .get(RANGER_SECURITY_XML_KEY)
        .expect("the renderer emits it");
    let mut secret = json!({
        "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
        "metadata": { "name": format!("{}-ranger-secret", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { RANGER_SECURITY_XML_KEY: security_xml }
    });
    // Which credential this Secret now holds (its non-secret id), so a rollout
    // probe can tell a later write of an OLDER credential (`crate::rollout`).
    if let Some(id) = crate::rollout::ranger_credential_id(spec) {
        secret["metadata"]["annotations"] =
            json!({ crate::rollout::RANGER_CREDENTIAL_ANNOTATION: id });
    }
    Some(secret)
}

/// The FE TLS cert ConfigMap (`{name}-ssl-cm`). `None` unless the engine's
/// delegation config has `tls_enabled`. Carries ONLY the public cert
/// (`fe.crt` = `tls_cert_pem`) — informational/debugging. The cert+KEY bundle
/// used to build the keystore lives in [`starrocks_ssl_secret`], NOT here: a
/// private key must never sit in a plaintext ConfigMap.
pub fn starrocks_ssl_configmap(spec: &EngineSpec) -> Option<Value> {
    let d = spec
        .starrocks
        .as_ref()
        .and_then(|sr| sr.delegation.as_ref())
        .filter(|d| d.tls_enabled)?;
    Some(json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-ssl-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { "fe.crt": d.tls_cert_pem }
    }))
}

/// The FE TLS cert+KEY Secret (`{name}-ssl-secret`). `None` unless the
/// engine's delegation config has `tls_enabled`. Carries the cert+key PEM
/// bundle (`fe.pem` — `StarRocksDelegation::tls_keystore_pem`, i.e.
/// `ensure_tls_cert`'s `keystore_pem`) the T2c init-container (see
/// `starrocks_cluster_cr`) reads to build the Java keystore
/// `ssl_keystore_location` points at. This is a `v1/Secret` (NOT a ConfigMap)
/// because `fe.pem` contains the FE's private key. Mounted read-only via the
/// FE spec's `secrets` field at the SOURCE path (`/etc/starrocks/ssl-src`),
/// NOT `/etc/starrocks/ssl` — that path is the init-container-built keystore
/// volume.
pub fn starrocks_ssl_secret(spec: &EngineSpec) -> Option<Value> {
    let d = spec
        .starrocks
        .as_ref()
        .and_then(|sr| sr.delegation.as_ref())
        .filter(|d| d.tls_enabled)?;
    Some(json!({
        "apiVersion": "v1", "kind": "Secret",
        "type": "Opaque",
        "metadata": { "name": format!("{}-ssl-secret", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "fe.pem": d.tls_keystore_pem }
    }))
}

/// In-pod path the initpwd script is mounted at and invoked from (matches the
/// kube-starrocks operator's `init-pwd` Job).
const INITPWD_SCRIPT_PATH: &str = "/opt/starrocks/fe_initpwd.sh";
/// ConfigMap data key / mount subPath for the initpwd script.
const INITPWD_SCRIPT_KEY: &str = "fe_initpwd.sh";
/// FE MySQL-protocol query port the initpwd script connects to.
const INITPWD_FE_QUERY_PORT: &str = "9030";

/// The FE `root` password, or `None` when the engine is legacy/passwordless
/// (no `root_password`, or an explicitly-empty one). Central gate for all three
/// initpwd builders.
fn root_password(spec: &EngineSpec) -> Option<&str> {
    spec.starrocks
        .as_ref()
        .and_then(|s| s.root_password.as_deref())
        .filter(|p| !p.is_empty())
}

/// `{name}-initpwd-secret` Opaque Secret carrying the root password under
/// `stringData.password`. `None` when the engine has no root password
/// configured (legacy/passwordless) — nothing is templated.
pub fn starrocks_root_password_secret(spec: &EngineSpec) -> Option<Value> {
    let pw = root_password(spec)?;
    Some(json!({
        "apiVersion": "v1", "kind": "Secret",
        "type": "Opaque",
        "metadata": { "name": format!("{}-initpwd-secret", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "password": pw }
    }))
}

/// `{name}-initpwd-shell` ConfigMap carrying `fe_initpwd.sh` — the operator's
/// idempotent SET-PASSWORD script (copied VERBATIM from kube-starrocks's
/// `init-pwd/configmap.yaml`; error codes `1045`/`1064` are treated as
/// "already changed" and exit 0). `None` when no root password.
pub fn starrocks_initpwd_configmap(spec: &EngineSpec) -> Option<Value> {
    root_password(spec)?;
    // VERBATIM from kube-starrocks helm-charts
    // charts/kube-starrocks/charts/starrocks/templates/init-pwd/configmap.yaml.
    let script = r#"#!/bin/bash

# ensure `mysql` command can be ended with 30 seconds
# Change the root password during initial installation
init_pwd_sql="SET PASSWORD = PASSWORD('$INIT_PWD');"
success=`mysql --connect-timeout 30 -h $1 -P $2 -u root --skip-column-names --batch -e "$init_pwd_sql" 2>&1`
if [ $? -ne 0 ] ; then
  echo $success
  errcode=`echo $success | awk -F " " '{print $2}'`
  echo "error code: $errcode"

  # Password error, believed to have been changed, exiting normally
  if [[ $errcode = '1045' || $errcode = '1064' ]] ; then
    echo "Password error, believed to have been changed, exiting normally"
    exit 0
  fi

  # Other unsuccessful errors, abnormal exit
  echo "Other unsuccessful errors, abnormal exit"
  exit 1

fi

echo "Successfully modified password"
exit 0
"#;
    Some(json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-initpwd-shell", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { INITPWD_SCRIPT_KEY: script }
    }))
}

/// `{name}-initpwd` batch/v1 Job that runs `fe_initpwd.sh` against the FE
/// service in-cluster to set root's password — the egret equivalent of the
/// operator's `init-pwd` Job. Retries (`backoffLimit: 10`, `OnFailure`) until
/// the FE is up. `None` when no root password.
///
/// The script is mounted read-only from the `{name}-initpwd-shell` ConfigMap
/// via a `subPath` single-file projection (as the operator does), and the
/// password is injected as `INIT_PWD` from the `{name}-initpwd-secret` Secret.
/// Unlike the operator (which targets `{name}-fe-0.{name}-fe-search`), egret
/// targets the FE Service DNS name `{name}-fe-service.{namespace}` so the Job
/// resolves the FE regardless of pod ordinal.
pub fn starrocks_initpwd_job(spec: &EngineSpec) -> Option<Value> {
    root_password(spec)?;
    let sr = spec.starrocks.as_ref()?;
    let secret_name = format!("{}-initpwd-secret", spec.name);
    let cm_name = format!("{}-initpwd-shell", spec.name);
    let fe_host = format!("{}-fe-service.{}", spec.name, spec.namespace);
    Some(json!({
        "apiVersion": "batch/v1", "kind": "Job",
        "metadata": { "name": format!("{}-initpwd", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": {
            "backoffLimit": 10,
            "template": {
                "spec": {
                    "restartPolicy": "OnFailure",
                    "containers": [{
                        "name": "initpwd",
                        "image": sr.fe_image,
                        "command": ["/bin/bash"],
                        "args": [INITPWD_SCRIPT_PATH, fe_host, INITPWD_FE_QUERY_PORT],
                        "env": [{
                            "name": "INIT_PWD",
                            "valueFrom": { "secretKeyRef": { "name": secret_name, "key": "password" } },
                        }],
                        "volumeMounts": [{
                            "name": "initpwd-script",
                            "mountPath": INITPWD_SCRIPT_PATH,
                            "subPath": INITPWD_SCRIPT_KEY,
                        }],
                    }],
                    "volumes": [{
                        "name": "initpwd-script",
                        "configMap": { "name": cm_name, "defaultMode": 0o755 },
                    }],
                }
            }
        }
    }))
}

/// Name of the writable `storageVolumes` entry (see `starrocks_cluster_cr`)
/// the build-keystore init-container writes `keystore` into and the FE main
/// container reads `ssl_keystore_location` from.
const SSL_KEYSTORE_VOLUME_NAME: &str = "ssl-keystore";
const SSL_KEYSTORE_MOUNT_PATH: &str = "/etc/starrocks/ssl";

/// FE metadata volume — the BDBJE journal + image directory that holds EVERY
/// piece of FE state that is not in the object store: the catalogs, the
/// StarRocks-side users/roles/privileges, and the schemas.
///
/// `mountPath` is the operator's OWN default meta path, and that is the point:
/// kube-starrocks only injects its `emptyDir` default for a path that no
/// `storageVolumes` entry already mounts (`fe-meta` at this exact path), so
/// matching it here is what SUPPRESSES the emptyDir rather than adding a
/// second volume beside it. The name matches the operator's default too, so
/// the generated StatefulSet claim keeps its familiar `fe-meta-<sts>-<ord>`
/// shape.
const FE_META_VOLUME_NAME: &str = "fe-meta";
const FE_META_MOUNT_PATH: &str = "/opt/starrocks/fe/meta";

/// Pod-template annotation key stamped onto BOTH the FE and CN roles to force a
/// rolling restart. The kube-starrocks operator copies each role spec's
/// `annotations` map straight onto that role's pod-template `ObjectMeta`
/// (confirmed against the operator source: `pkg/subcontrollers/fe/fe_pod.go` and
/// `cn/cn_pod.go` both do `annotations := pod.Annotations(spec)` →
/// `PodTemplateSpec.ObjectMeta.Annotations`, and `pod.Annotations` just returns
/// `spec.GetAnnotations()` verbatim). Changing this annotation changes the pod
/// template, so the operator reconciles the StatefulSet and rolls its pods —
/// exactly the `kubectl rollout restart` effect, but through the CR (patching
/// the operator-owned StatefulSet directly would be reverted by the operator).
const RESTART_STAMP_KEY: &str = "egret.io/restarted-at";

/// Pod-template annotation carrying a hash of every rendered config the FE
/// **mounts**: `{name}-fe-cm` (`fe.conf`, via `configMapInfo`),
/// `{name}-ranger-cm` (the Ranger audit XML, via `configMaps` + `subPath`) and
/// `{name}-ranger-secret` (the security XML and its credential, via `secrets` +
/// `subPath`).
///
/// **This is what makes a config change reach the FE.** Neither mount is
/// self-refreshing: `configMapInfo` is resolved at boot, and a `subPath`
/// projection is copied when the container starts and never updated. So a
/// ConfigMap whose CONTENT changed while its NAME stayed the same produces a
/// byte-identical CR, the operator sees no pod-template change, no pod is
/// replaced, and the FE goes on enforcing the old file indefinitely.
///
/// The Ranger case is the dangerous one, and a genuine fail-open: the binding
/// reaches this renderer only as `spec.ranger.is_some()`, and the profile NAME
/// lives solely inside `{name}-ranger-cm`, so re-pointing an engine from a
/// permissive profile to a restrictive one changed nothing the operator could
/// act on. The API and the database said the new profile; the FE enforced the
/// old one, indefinitely.
///
/// Hashing the rendered config into a pod annotation is exactly how the Trino
/// path already avoids this (`crate::trino::config_checksum`, stamped as
/// `egret/config-checksum` on the Deployment's pod template) — the same
/// mechanism, not a second one. `starRocksFeSpec.annotations` is the
/// equivalent surface here: the operator copies that map straight onto the
/// role's pod-template `ObjectMeta` (see [`RESTART_STAMP_KEY`] for the
/// operator-source citation), so changing it rolls the StatefulSet.
///
/// FE-only by design: every hashed object is an FE mount. The CN's own
/// `{name}-cn-cm` is a separate surface this does not claim to cover.
const CONFIG_CHECKSUM_KEY: &str = "egret/config-checksum";

/// The [`CONFIG_CHECKSUM_KEY`] value for `spec`. Pure and deterministic: the
/// same spec always hashes to the same string, and any change to either
/// mounted ConfigMap's rendered content changes it.
fn fe_config_checksum(spec: &EngineSpec) -> String {
    let mut h = DefaultHasher::new();
    starrocks_fe_configmap(spec).to_string().hash(&mut h);
    // `None` (unbound) hashes differently from any bound profile: no bound
    // profile renders an empty string here.
    starrocks_ranger_configmap(spec)
        .map(|cm| cm.to_string())
        .unwrap_or_default()
        .hash(&mut h);
    // The security XML, credential included: a rotation changes only this
    // Secret, and its `subPath` mount is never refreshed in a running FE.
    starrocks_ranger_secret(spec)
        .map(|s| s.to_string())
        .unwrap_or_default()
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

/// JSON merge-patch (RFC 7386) body that stamps `stamp` onto the FE + CN pod
/// annotations of an existing `StarRocksCluster` CR. Merge semantics mean this
/// only sets the one annotation key on each role — every other CR field, and any
/// other annotations, are left untouched. Pure fn so the restart wiring is
/// unit-testable without a live cluster (see `restart_stamps_fe_and_cn`).
pub fn starrocks_restart_patch(stamp: &str) -> Value {
    json!({
        "spec": {
            "starRocksFeSpec": { "annotations": { RESTART_STAMP_KEY: stamp } },
            "starRocksCnSpec": { "annotations": { RESTART_STAMP_KEY: stamp } },
        }
    })
}

/// Apply the admin pod/scheduling fields onto a role spec (FE or CN). Simple
/// scalars/maps overwrite; `init_containers`/`sidecars` are APPENDED to any
/// existing array (so the FE SSL keystore init-container is preserved).
/// `env_key` is the role-specific env field name ("feEnvVars" | "cnEnvVars").
///
/// Operator CR keys (kube-starrocks `StarRocksLoadSpec`/`StarRocksComponentSpec`,
/// `pkg/apis/starrocks/v1/{load_type,component_type,starrockscluster_types}.go`):
/// `serviceAccount`, `nodeSelector`, `annotations`, `podLabels`, `feEnvVars`/
/// `cnEnvVars`, `tolerations`, `affinity`, `imagePullSecrets`
/// (`[]LocalObjectReference` → `[{name}]`), `initContainers`, `sidecars`.
///
/// NOTE: `pod.security_context` is intentionally NOT rendered. The operator's
/// FE/CN role spec has no role-level `securityContext` field, and the
/// StarRocksCluster CRD is a structural schema (no
/// `x-kubernetes-preserve-unknown-fields`), so an emitted `securityContext`
/// key would be silently PRUNED by the API server — a misleading no-op. Pod
/// security lives per-container inside `initContainers`/`sidecars` instead.
fn apply_pod_fields(spec: &mut Value, pod: &StarRocksPod, env_key: &str) {
    if let Some(sa) = &pod.service_account {
        spec["serviceAccount"] = json!(sa);
    }
    if !pod.node_selector.is_empty() {
        spec["nodeSelector"] = json!(pod.node_selector);
    }
    if !pod.annotations.is_empty() {
        // MERGE into any existing `annotations` map — never clobber (there are
        // none here today, but the restart flow stamps this field separately).
        let entry = spec
            .as_object_mut()
            .expect("role spec is a JSON object")
            .entry("annotations")
            .or_insert_with(|| json!({}));
        if let Some(map) = entry.as_object_mut() {
            for (k, v) in &pod.annotations {
                map.insert(k.clone(), json!(v));
            }
        }
    }
    if !pod.pod_labels.is_empty() {
        spec["podLabels"] = json!(pod.pod_labels);
    }
    if !pod.env.is_empty() {
        spec[env_key] = json!(pod.env);
    }
    if let Some(t) = &pod.tolerations {
        spec["tolerations"] = t.clone();
    }
    if let Some(a) = &pod.affinity {
        spec["affinity"] = a.clone();
    }
    // `pod.security_context` is intentionally NOT rendered — see the doc comment.
    if !pod.image_pull_secrets.is_empty() {
        // k8s `LocalObjectReference`: a list of `{name: "..."}` objects.
        spec["imagePullSecrets"] = json!(pod
            .image_pull_secrets
            .iter()
            .map(|n| json!({ "name": n }))
            .collect::<Vec<_>>());
    }
    // APPEND (never overwrite): the FE SSL block may already have populated
    // `initContainers` with the build-keystore container.
    append_array(spec, "initContainers", pod.init_containers.as_ref());
    append_array(spec, "sidecars", pod.sidecars.as_ref());
}

/// Append the elements of `add` (expected a JSON array) onto `spec[key]`,
/// creating the array if absent — preserving any existing entries (e.g. the
/// FE SSL keystore init-container already on `initContainers`).
fn append_array(spec: &mut Value, key: &str, add: Option<&Value>) {
    // A present-but-non-array passthrough is malformed (the field is typed
    // `Option<Value>` so we can't rely on serde to reject it) — drop it, but
    // surface the drop so it isn't silently invisible.
    if let Some(v) = add {
        if !v.is_null() && !v.is_array() {
            tracing::warn!(field = key, "ignoring non-array StarRocks pod field");
        }
    }
    let Some(add_items) = add.and_then(Value::as_array).filter(|a| !a.is_empty()) else {
        return;
    };
    let obj = spec.as_object_mut().expect("role spec is a JSON object");
    match obj.get_mut(key).and_then(Value::as_array_mut) {
        Some(existing) => existing.extend(add_items.iter().cloned()),
        None => {
            obj.insert(key.to_string(), Value::Array(add_items.clone()));
        }
    }
}

/// The `starrocks.com/v1 StarRocksCluster` CR: FE + CN roles only (shared-data
/// mode has no BE). FE mounts `fe.conf` via `configMapInfo`, plus, when a
/// binding is present, the Ranger audit XML ConfigMap under `configMaps` and the
/// security XML Secret under `secrets`. When
/// delegation TLS is enabled it also gets the TLS cert+key source Secret (via
/// `secrets`), a build-keystore `initContainers` entry, and a writable
/// `storageVolumes` keystore volume (see the T2c comment block below for the
/// CRD constraints that shape this).
pub fn starrocks_cluster_cr(spec: &EngineSpec) -> Value {
    let sr = spec
        .starrocks
        .as_ref()
        .expect("starrocks_cluster_cr requires spec.starrocks");

    let mut fe_spec = json!({
        "image": sr.fe_image,
        "replicas": sr.fe_replicas,
        "configMapInfo": {
            "configMapName": format!("{}-fe-cm", spec.name),
            "resolveKey": "fe.conf",
        }
    });
    if let Some(r) = &sr.fe_resources {
        let (limits, requests) = resources_json(r);
        if limits.as_object().is_some_and(|m| !m.is_empty()) {
            fe_spec["limits"] = limits;
        }
        if requests.as_object().is_some_and(|m| !m.is_empty()) {
            fe_spec["requests"] = requests;
        }
    }
    // Extra ConfigMaps the FE mounts: the Ranger XML (when bound). The TLS
    // cert+key SOURCE is a Secret (see `starrocks_ssl_secret`), mounted via
    // the FE spec's `secrets` field below — NOT here, since it carries the
    // private key. The cert-only `{name}-ssl-cm` ConfigMap is still applied
    // (public-cert reference) but is not needed for the keystore build, so it
    // is not mounted into the FE.
    //
    // Each XML gets its OWN mount with a `subPath`, so only that one file
    // lands in `/opt/starrocks/fe/conf`. Mounting the ConfigMap at the bare
    // conf directory replaces the WHOLE directory with the ConfigMap's two
    // keys — `fe.conf` included — and the FE cannot boot (observed live as
    // `ln: ...: Read-only file system`, then an FE that never started and
    // lost its metadata).
    //
    // A `subPath` mount does NOT receive ConfigMap updates the way a
    // directory mount does: the file is copied at container start and never
    // refreshed. So new content only reaches the plugin when the POD is
    // replaced — which is why the FE role carries a `CONFIG_CHECKSUM_KEY`
    // annotation over this ConfigMap's content (see `fe_config_checksum`).
    //
    // An earlier version of this comment claimed "a rebind rolls the FE
    // anyway". That was FALSE, and it is the bug `fe_config_checksum` fixes:
    // two different profiles render a byte-identical CR here (the binding is
    // consumed only as `is_some()`, and the service name lives solely inside
    // `{name}-ranger-cm`), so nothing in the pod template changed and the
    // operator had no reason to roll anything.
    //
    // The audit XML comes from `{name}-ranger-cm`; the security XML carries the
    // plugin's credential, so it comes from the `{name}-ranger-secret` Secret
    // through the CR's `secrets` field (the same name+mountPath+subPath shape;
    // the operator names the pod volume itself, which no one here references).
    let mut extra_config_maps: Vec<Value> = Vec::new();
    let mut extra_secrets: Vec<Value> = Vec::new();
    if spec.ranger.is_some() {
        extra_config_maps.push(json!({
            "name": format!("{}-ranger-cm", spec.name),
            "mountPath": format!("{FE_CONF_DIR}/{RANGER_AUDIT_XML_KEY}"),
            "subPath": RANGER_AUDIT_XML_KEY,
        }));
        extra_secrets.push(json!({
            "name": format!("{}-ranger-secret", spec.name),
            "mountPath": format!("{FE_CONF_DIR}/{RANGER_SECURITY_XML_KEY}"),
            "subPath": RANGER_SECURITY_XML_KEY,
        }));
    }
    if !extra_config_maps.is_empty() {
        fe_spec["configMaps"] = json!(extra_config_maps);
    }
    if !extra_secrets.is_empty() {
        fe_spec["secrets"] = json!(extra_secrets);
    }
    // T2c: build the Java keystore `ssl_keystore_location` needs, from the
    // cert+key PEM the `{name}-ssl-secret` Secret carries (`fe.pem`).
    //
    // CRD CONSTRAINT (checked against `deploy/starrocks-operator/crd.yaml`):
    // `starRocksFeSpec` has NO generic top-level `volumes` field — only
    // `configMaps`/`secrets` (name+mountPath+subPath, always read-only
    // ConfigMap/Secret projections), `storageVolumes` (hostPath OR PVC,
    // name+mountPath — the only WRITABLE volume type the CRD exposes), and
    // `initContainers` (a full container spec, including its own
    // `volumeMounts`). So the writable share between the init-container and
    // the main FE container has to be a `storageVolumes` entry (hostPath
    // here, as the closest available substitute for `emptyDir`), not a plain
    // `volumes: [{emptyDir: {}}]` — that field doesn't exist on this CRD. The
    // cert+key SOURCE is projected read-only via the `secrets` field (the
    // CRD's RO Secret-projection mount) so the private key never touches a
    // ConfigMap.
    //
    // ASSUMPTION (UNVERIFIED, live-only): the init-container's `volumeMounts`
    // reference pod-level volume NAMES. For `storageVolumes` the name is
    // exactly what we set below (`SSL_KEYSTORE_VOLUME_NAME`), so that half is
    // solid. For the `secrets` entry there is no separate "volume name" field
    // in the CRD schema — only the Secret's own `name` — so this assumes the
    // operator names the generated pod volume identically to the Secret
    // object's name (`{ssl_secret_name}`) and that this volume is reachable
    // from `initContainers[].volumeMounts` (not just the main container). If
    // the live smoke finds the operator uses a different internal volume
    // name, or doesn't expose Secret-derived volumes to init containers, this
    // mount needs a live-tuned fix (candidate: an initContainer image that
    // reads the cert+key via a k8s API/downward mechanism instead of a shared
    // volume).
    // FE METADATA — a PVC, but ONLY for an engine created after this shipped.
    //
    // `fe_meta_persistent` is a creation-time marker stamped by the create path
    // (`build_starrocks_spec` with no existing config), so it is `false` for
    // every engine that already exists. That gate is not caution, it is a
    // measured constraint: `volumeClaimTemplates` is IMMUTABLE on a live
    // StatefulSet. Adding the claim to an engine whose FE StatefulSet already
    // exists does not migrate it — the operator's merge patch is rejected by
    // the API server (`updates to statefulset spec for fields other than
    // 'replicas', 'ordinals', 'template', ... are forbidden`), the FE keeps the
    // `emptyDir` it already had, AND the `StarRocksCluster` goes to
    // `phase=failed` and stops converging, so NO later change of any kind (a
    // Ranger rebind, an image bump) applies either. Measured on kind
    // 2026-09-26; §7 of `docs/en/plans/2026-09-26-fe-meta-live-result.md` has
    // the verbatim error. An unmarked engine therefore renders EXACTLY what it
    // rendered before the PVC existed, so upgrading egret is a no-op for it and
    // its cluster keeps reconciling. Migrating one is a deliberate operator
    // action (`kubectl delete sts <engine>-fe --cascade=orphan`, which destroys
    // that FE's metadata and requires accepting that), never a side effect.
    //
    // Without an entry at the meta path the operator injects its own default
    // `fe-meta {"emptyDir": {}}`, so the FE's metadata lives and dies with the
    // POD: any replacement (a config checksum change, a node drain, a manual
    // `kubectl delete pod`, an image bump) brings the FE back with
    // `first start fe with meta not exist` — an empty catalog list, `root`
    // back to no password, and every StarRocks-side user and role gone. That
    // was observed live on PR #597's gate.
    //
    // NOT a `hostPath`, deliberately, and NOT following the `ssl-keystore`
    // precedent a few lines below: the keystore is REBUILT from a Secret by
    // the init-container on every boot, so losing it costs nothing. Metadata
    // is the exact opposite — it is the only copy. A `hostPath` happens to
    // survive a restart on the same node and is silently destroyed the moment
    // the pod is rescheduled elsewhere, which would turn today's loud,
    // reproducible failure into a rare one that looks like metadata
    // corruption. A PVC follows the pod.
    //
    // SIBLINGS, enumerated deliberately (see the live inventory in
    // `docs/en/plans/2026-09-26-fe-meta-live-result.md`): the operator
    // defaults `fe-log` (/opt/starrocks/fe/log) and `cn-log`
    // (/opt/starrocks/cn/log) to `emptyDir` as well, and in shared-data mode
    // the CN keeps NO data volume at all (its local block cache is rebuilt
    // from the object store). Those three are left on ephemeral storage ON
    // PURPOSE: logs are already scraped out of the pod, and a cache is
    // rebuildable by definition. `fe-meta` is the only volume in this CR whose
    // loss destroys state that exists nowhere else.
    let mut storage_volumes: Vec<Value> = Vec::new();
    if sr.fe_meta_persistent {
        let mut fe_meta = json!({
            "name": FE_META_VOLUME_NAME,
            "mountPath": FE_META_MOUNT_PATH,
            "storageSize": sr
                .fe_meta_storage_size
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_FE_META_STORAGE_SIZE),
        });
        // Omitted (not `""`) when unset, so the claim binds the cluster's
        // DEFAULT StorageClass. An empty-string `storageClassName` in k8s means
        // "no class, bind a pre-provisioned PV only" — the opposite of the
        // intent — so a blank config value must render as ABSENT, never as an
        // empty string.
        if let Some(class) = sr
            .fe_meta_storage_class
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            fe_meta["storageClassName"] = json!(class);
        }
        storage_volumes.push(fe_meta);
    }

    let tls_enabled = starrocks_ssl_secret(spec).is_some();
    if tls_enabled {
        let ssl_secret_name = format!("{}-ssl-secret", spec.name);
        // Writable volume the init-container builds `keystore` into and the FE
        // main container reads via `ssl_keystore_location`. `storageVolumes`
        // names are deterministic (the operator does NOT hash them), so the
        // init-container can reference `ssl-keystore` reliably.
        //
        // PUSHED onto the vec — never assigned over `storageVolumes` — so the
        // FE metadata entry above survives. Assigning here is exactly the
        // careless-overwrite bug `keystore_volume_keeps_fe_meta_volume` pins.
        storage_volumes.push(json!({
            "name": SSL_KEYSTORE_VOLUME_NAME,
            "mountPath": SSL_KEYSTORE_MOUNT_PATH,
            "hostPath": {
                "path": format!("/var/lib/egret/starrocks/{}/ssl-keystore", spec.name),
                "type": "DirectoryOrCreate",
            },
        }));
        // The cert+key SOURCE is delivered to the init-container via a
        // `secretKeyRef` ENV var (referencing the `{name}-ssl-secret` Secret by
        // OBJECT NAME) rather than a mounted volume — the kube-starrocks
        // operator renames `secrets`/`configMaps` pod volumes to
        // `<name>-<fnv32a(mountInfo)[:4]>` (`getVolumeName`, VolumeNameWithHash),
        // a nondeterministic suffix an `initContainers[].volumeMounts` entry
        // can't reference. `secretKeyRef` sidesteps the volume-name hashing
        // entirely. The FE main container doesn't need the PEM (it reads the
        // built keystore), so no `secrets` mount is templated.
        //
        // LIVE-ONLY (flagged per T2c scope): the keystore FORMAT (PKCS12 here,
        // built by `openssl pkcs12 -export` straight from the PEM cert+key) and
        // whether StarRocks' `ssl_keystore_location` accepts PKCS12 vs requiring
        // JKS (`keytool`) is verified/tuned at the live smoke.
        fe_spec["initContainers"] = json!([{
            "name": "build-keystore",
            "image": "alpine/openssl:latest",
            "command": ["sh", "-c"],
            "args": [format!(
                "printf '%s' \"$FE_PEM\" > /tmp/fe.pem && \
                 openssl pkcs12 -export -in /tmp/fe.pem -inkey /tmp/fe.pem -out {dst}/keystore -passout pass:changeit -name fe && \
                 echo keystore built",
                dst = SSL_KEYSTORE_MOUNT_PATH,
            )],
            "env": [{
                "name": "FE_PEM",
                "valueFrom": { "secretKeyRef": { "name": ssl_secret_name, "key": "fe.pem" } },
            }],
            "volumeMounts": [
                { "name": SSL_KEYSTORE_VOLUME_NAME, "mountPath": SSL_KEYSTORE_MOUNT_PATH },
            ],
        }]);
    }
    // ONLY when something is in it. An unmarked, TLS-less engine must render no
    // `storageVolumes` KEY AT ALL — an empty array is a different document and
    // would be a live change for every existing engine, which is the whole
    // thing this gate exists to prevent.
    if !storage_volumes.is_empty() {
        fe_spec["storageVolumes"] = json!(storage_volumes);
    }

    let mut cn_spec = json!({
        "image": sr.cn_image,
        "replicas": sr.cn_replicas,
    });
    // Mount the admin `cn.conf` ConfigMap (only emitted when `cn_config` is
    // non-empty — see `starrocks_cn_configmap`). When empty, CN keeps the
    // operator defaults and gets no `configMapInfo` (unchanged from today).
    if !sr.cn_config.is_empty() {
        cn_spec["configMapInfo"] = json!({
            "configMapName": format!("{}-cn-cm", spec.name),
            "resolveKey": "cn.conf",
        });
    }
    if let Some(r) = &sr.cn_resources {
        let (limits, requests) = resources_json(r);
        if limits.as_object().is_some_and(|m| !m.is_empty()) {
            cn_spec["limits"] = limits;
        }
        if requests.as_object().is_some_and(|m| !m.is_empty()) {
            cn_spec["requests"] = requests;
        }
    }
    // CN horizontal autoscaling → the operator's `autoScalingPolicy` on the CN
    // spec (it provisions an HPA that scales the CN StatefulSet). `version: "v2"`
    // is the operator's `AutoScalerV2` enum value (see
    // `pkg/apis/starrocks/v1/auto_scale.go`: `AutoScalerV2 = "v2"`); `metrics` is
    // the standard k8s `autoscaling/v2` Resource-CPU-Utilization MetricSpec,
    // matching the operator's `HPAPolicy.Metrics []autoscalingv2beta2.MetricSpec`.
    //
    // PRECEDENCE: `cn_spec.replicas` (set from `cn_replicas` above) stays in the
    // CR, but once `autoScalingPolicy` is present the operator hands replica count
    // to the HPA and stops reconciling `replicas` — so emitting both is
    // intentional and harmless (the static value is just the pre-HPA baseline).
    //
    // DISABLED SENTINEL: `max_replicas == 0` means "autoscaling disabled" → we
    // emit NO `autoScalingPolicy` (CN falls back to the static `cn_replicas`).
    // This is the FE's post-enable OFF switch: the omit=preserve config store
    // can't clear a `Some(cn_autoscaling)`, so the FE sends `{maxReplicas: 0}` to
    // turn autoscaling off. `None` (never enabled) likewise renders no policy.
    if let Some(a) = &sr.cn_autoscaling {
        if a.max_replicas > 0 {
            let mut pol = json!({ "version": "v2", "maxReplicas": a.max_replicas });
            if let Some(m) = a.min_replicas {
                pol["minReplicas"] = json!(m);
            }
            if let Some(cpu) = a.target_cpu_percent {
                pol["hpaPolicy"] = json!({ "metrics": [{
                    "type": "Resource",
                    "resource": {
                        "name": "cpu",
                        "target": { "type": "Utilization", "averageUtilization": cpu }
                    }
                }]});
            }
            cn_spec["autoScalingPolicy"] = pol;
        }
    }

    // Admin pod/scheduling fields on BOTH roles, applied LAST so the FE SSL
    // block's `initContainers` (build-keystore) is already in place and gets
    // APPENDED to — not clobbered — by any admin `init_containers`.
    if let Some(pod) = &sr.pod {
        apply_pod_fields(&mut fe_spec, pod, "feEnvVars");
        apply_pod_fields(&mut cn_spec, pod, "cnEnvVars");
    }

    // Stamped LAST, and merged into whatever `annotations` map is already
    // there, so an operator-supplied `annotations` entry can neither clobber
    // the map nor shadow this key — which would silently disable the only
    // thing that makes a Ranger rebind reach the FE.
    let checksum = fe_config_checksum(spec);
    fe_spec
        .as_object_mut()
        .expect("fe role spec is a JSON object")
        .entry("annotations")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("annotations is a JSON object")
        .insert(CONFIG_CHECKSUM_KEY.to_string(), json!(checksum));
    // Only the FE runs the Ranger plugin: name the credential its pods are
    // rendered with (its non-secret id), so the rollout of THAT credential can
    // be observed before the previous one is retired (`crate::rollout`).
    if let Some(id) = crate::rollout::ranger_credential_id(spec) {
        fe_spec["annotations"][crate::rollout::RANGER_CREDENTIAL_ANNOTATION] = json!(id);
    }

    json!({
        "apiVersion": "starrocks.com/v1", "kind": "StarRocksCluster",
        "metadata": { "name": spec.name, "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": {
            "starRocksFeSpec": fe_spec,
            "starRocksCnSpec": cn_spec,
        }
    })
}

/// Render a `networking.k8s.io/v1` Ingress exposing the StarRocks FE HTTP
/// endpoint (`{name}-fe-service` port 8030). Returns `None` unless the ingress
/// is enabled. TLS terminates at the ingress; the FE stays HTTP internally.
/// (The MySQL query port 9030 is raw TCP and is NOT exposed here.)
pub fn starrocks_ingress(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled {
        return None;
    }
    let mut meta = json!({
        "name": format!("{}-fe", spec.name), "namespace": spec.namespace,
        "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name }
    });
    if !ing.annotations.is_empty() {
        meta["annotations"] = Value::Object(
            ing.annotations
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
        );
    }
    let mut spec_obj = json!({
        "rules": [{ "host": ing.host, "http": { "paths": [{
            "path": "/", "pathType": "Prefix",
            "backend": { "service": { "name": format!("{}-fe-service", spec.name), "port": { "number": 8030 } } }
        }]}}]
    });
    if let Some(cn) = &ing.class_name {
        spec_obj["ingressClassName"] = json!(cn);
    }
    if let Some(ts) = &ing.tls_secret {
        spec_obj["tls"] = json!([{ "hosts": [ing.host], "secretName": ts }]);
    }
    Some(json!({
        "apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
        "metadata": meta, "spec": spec_obj
    }))
}

/// Provisions a StarRocks engine (shared-data mode) via the kube-starrocks
/// operator: an `fe.conf` ConfigMap, the optional Ranger audit ConfigMap and
/// credential Secret, and the `StarRocksCluster` CR. The operator owns the FE/CN StatefulSets and pods.
pub struct StarRocksProvisioner {
    kube: Arc<dyn KubeClient>,
}
impl StarRocksProvisioner {
    pub fn new(kube: Arc<dyn KubeClient>) -> Self {
        Self { kube }
    }

    /// Apply the fe.conf ConfigMap, the Ranger XML ConfigMap (when bound), the
    /// TLS cert ConfigMap + cert+key Secret (when delegation TLS is enabled),
    /// and the CR in order. On the happy path these are the objects a
    /// StarRocks engine needs; `provision` best-effort rolls them back on
    /// error.
    async fn apply_all(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        self.kube.apply(&starrocks_fe_configmap(spec)).await?;
        if let Some(cm) = starrocks_cn_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        if let Some(cm) = starrocks_ranger_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        // Before the CR that mounts it.
        if let Some(secret) = starrocks_ranger_secret(spec) {
            self.kube.apply(&secret).await?;
        }
        if let Some(cm) = starrocks_ssl_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        if let Some(secret) = starrocks_ssl_secret(spec) {
            self.kube.apply(&secret).await?;
        }
        // Root-password initpwd resources (present only when a root password is
        // configured). The Secret + ConfigMap are mutable and freely
        // re-appliable; the Job's `spec.template` is IMMUTABLE, so on a second
        // provision/update a plain apply would error on immutable fields. Delete
        // any existing `{name}-initpwd` Job first (best-effort, ignore
        // not-found) so every provision/update gets a fresh Job running the
        // idempotent SET-PASSWORD script.
        if let Some(secret) = starrocks_root_password_secret(spec) {
            self.kube.apply(&secret).await?;
            if let Some(cm) = starrocks_initpwd_configmap(spec) {
                self.kube.apply(&cm).await?;
            }
            if let Some(job) = starrocks_initpwd_job(spec) {
                let _ = self
                    .kube
                    .delete(
                        "batch/v1",
                        "Job",
                        &spec.namespace,
                        &format!("{}-initpwd", spec.name),
                    )
                    .await;
                self.kube.apply(&job).await?;
            }
        }
        self.kube.apply(&starrocks_cluster_cr(spec)).await?;
        if let Some(ing) = spec.config.ingress.as_ref() {
            if let Some(sec) = crate::ingress::ingress_tls_secret(&spec.name, &spec.namespace, ing)
            {
                self.kube.apply(&sec).await?;
            }
        }
        if let Some(ing) = starrocks_ingress(spec) {
            self.kube.apply(&ing).await?;
        }
        Ok(())
    }

    /// Roll the FE + CN pods by JSON-merge-patching a fresh restart-stamp
    /// annotation onto the existing `StarRocksCluster` CR's fe/cn specs; the
    /// operator propagates the annotation to each role's pod template, which
    /// changes the template and rolls the StatefulSet pods (see
    /// [`starrocks_restart_patch`] / [`RESTART_STAMP_KEY`]).
    ///
    /// A targeted merge-patch is used rather than a full CR re-apply: restart
    /// only carries `namespace`+`name` (not the full `EngineSpec`), and a merge
    /// patch can't wipe operator-managed fields the way a partial server-side
    /// apply could. The `stamp` is generated by the caller (egret-bin, which has
    /// `std::time`) so this stays a pure, deterministic operation.
    pub async fn restart(&self, namespace: &str, name: &str, stamp: &str) -> anyhow::Result<()> {
        self.kube
            .merge_patch(
                "starrocks.com/v1",
                "StarRocksCluster",
                namespace,
                name,
                &starrocks_restart_patch(stamp),
            )
            .await
    }
}

#[async_trait::async_trait]
impl EngineProvisioner for StarRocksProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Mirror `TrinoProvisioner::provision`: if any apply fails partway, the
        // earlier objects (e.g. the ConfigMaps) would be orphaned, so best-effort
        // delete everything before returning the error.
        if let Err(e) = self.apply_all(spec).await {
            let _ = self.delete(&spec.namespace, &spec.name).await;
            return Err(e);
        }
        Ok(())
    }
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Apply WITHOUT `provision`'s delete-on-failure: that cleanup is right for a
        // brand-new engine, but on an update it would tear down a running cluster
        // because one manifest was rejected. The caller restores the stored spec.
        self.apply_all(spec).await?;
        // `apply_all` only applies the Ranger ConfigMap and Secret when bound, so
        // toggling a RangerBinding OFF would otherwise leave `{name}-ranger-cm` and
        // `{name}-ranger-secret` orphaned (the CR's mounts are dropped, but the
        // objects linger).
        // Best-effort delete it when Ranger is absent.
        if spec.ranger.is_none() {
            let _ = self
                .kube
                .delete(
                    "v1",
                    "ConfigMap",
                    &spec.namespace,
                    &format!("{}-ranger-cm", spec.name),
                )
                .await;
            let _ = self
                .kube
                .delete(
                    "v1",
                    "Secret",
                    &spec.namespace,
                    &format!("{}-ranger-secret", spec.name),
                )
                .await;
        }
        // `apply_all` only applies the CN ConfigMap when `cn_config` is
        // non-empty, so clearing all CN overrides would otherwise orphan
        // `{name}-cn-cm` (the CR's `configMapInfo` is dropped, but the
        // ConfigMap lingers). Best-effort delete it when there are no overrides.
        if spec
            .starrocks
            .as_ref()
            .is_none_or(|sr| sr.cn_config.is_empty())
        {
            let _ = self
                .kube
                .delete(
                    "v1",
                    "ConfigMap",
                    &spec.namespace,
                    &format!("{}-cn-cm", spec.name),
                )
                .await;
        }
        // Mirror the same orphan-cleanup for the TLS ConfigMap AND the cert+key
        // Secret when delegation TLS is toggled off.
        if starrocks_ssl_secret(spec).is_none() {
            let _ = self
                .kube
                .delete(
                    "v1",
                    "ConfigMap",
                    &spec.namespace,
                    &format!("{}-ssl-cm", spec.name),
                )
                .await;
            let _ = self
                .kube
                .delete(
                    "v1",
                    "Secret",
                    &spec.namespace,
                    &format!("{}-ssl-secret", spec.name),
                )
                .await;
        }
        Ok(())
    }
    async fn ranger_rollout(
        &self,
        namespace: &str,
        name: &str,
        credential_id: &str,
    ) -> anyhow::Result<RangerRollout> {
        // The operator names the FE StatefulSet `{cluster}-fe` and labels its
        // pods `app.starrocks.ownerreference/name={cluster}-fe` (see `status`).
        // The CN runs no Ranger plugin, so it is not observed.
        use crate::rollout::Workload;
        crate::rollout::observe(
            &*self.kube,
            namespace,
            &format!("{name}-ranger-secret"),
            &[Workload::StatefulSet(format!("{name}-fe"))],
            &format!("app.starrocks.ownerreference/name={name}-fe"),
            credential_id,
        )
        .await
    }
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
        // The kube-starrocks operator owns the FE/CN pods and stamps
        // `app.starrocks.ownerreference/name=<cluster>-fe` / `-cn` (NOT the
        // `app.kubernetes.io/instance=<name>` the Trino path sets on its own
        // Deployment pods — verified at the T8 live smoke). Query both owner
        // labels and merge so the control-plane health UI sees FE + CN pods.
        let mut pods = self
            .kube
            .list_pods_selector(
                namespace,
                &format!("app.starrocks.ownerreference/name={name}-fe"),
            )
            .await?;
        pods.extend(
            self.kube
                .list_pods_selector(
                    namespace,
                    &format!("app.starrocks.ownerreference/name={name}-cn"),
                )
                .await?,
        );
        let pod_status: Vec<PodStatus> = pods
            .into_iter()
            .map(|(name, phase, ready)| PodStatus { name, phase, ready })
            .collect();
        let phase = if pod_status.is_empty() {
            "provisioning"
        } else if pod_status.iter().all(|p| p.ready) {
            "running"
        } else if pod_status.iter().any(|p| p.phase == "Failed") {
            "failed"
        } else {
            "provisioning"
        };
        Ok(EngineStatus {
            phase: phase.into(),
            message: None,
            pods: pod_status,
        })
    }
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        // The CR first (the operator tears down FE/CN), then the objects egret
        // applies beside it. A 404 is success; any other failure is returned,
        // after the remaining objects have still been attempted.
        let objects = [
            ("starrocks.com/v1", "StarRocksCluster", name.to_string()),
            ("v1", "ConfigMap", format!("{name}-fe-cm")),
            ("v1", "ConfigMap", format!("{name}-cn-cm")),
            ("v1", "ConfigMap", format!("{name}-ranger-cm")),
            ("v1", "Secret", format!("{name}-ranger-secret")),
            ("v1", "ConfigMap", format!("{name}-ssl-cm")),
            ("v1", "Secret", format!("{name}-ssl-secret")),
            ("batch/v1", "Job", format!("{name}-initpwd")),
            ("v1", "ConfigMap", format!("{name}-initpwd-shell")),
            ("v1", "Secret", format!("{name}-initpwd-secret")),
            ("networking.k8s.io/v1", "Ingress", format!("{name}-fe")),
            ("v1", "Secret", format!("{name}-ingress-tls")),
        ];
        crate::kube_client::delete_objects(&*self.kube, namespace, &objects).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;
    use std::collections::BTreeMap;

    fn sample_sr_spec() -> EngineSpec {
        EngineSpec {
            name: "sr1".into(),
            namespace: "egret-engines".into(),
            image_version: "3.3-latest".into(),
            config: TrinoConfig::default(),
            catalogs: vec![],
            ranger: None,
            kind: EngineKind::StarRocks,
            starrocks: Some(StarRocksConfig {
                fe_image: "starrocks/fe-ubuntu:3.3-latest".into(),
                cn_image: "starrocks/cn-ubuntu:3.3-latest".into(),
                fe_replicas: 1,
                cn_replicas: 1,
                storage_endpoint: "http://host.docker.internal:9000".into(),
                storage_path: "warehouse/starrocks".into(),
                storage_access_key: "egret".into(),
                storage_secret_key: "egretsecret".into(),
                fe_resources: None,
                cn_resources: None,
                delegation: None,
                fe_config: Default::default(),
                cn_config: Default::default(),
                cn_autoscaling: None,
                pod: None,
                root_password: None,
                fe_meta_storage_class: None,
                fe_meta_storage_size: None,
                // The default fixture is an engine created AFTER the FE-meta
                // PVC shipped, i.e. the shape every new engine has from now on.
                // `sample_sr_spec_unmarked` below is the legacy counterpart.
                fe_meta_persistent: true,
            }),
        }
    }

    /// `sample_sr_spec` as an engine that ALREADY EXISTED when the FE metadata
    /// PVC shipped: no creation-time marker, so egret must render exactly what
    /// it rendered before — nothing at the meta path, leaving the operator's
    /// `emptyDir` default in place. See `fe_meta_*_unmarked_*` below for why
    /// that is mandatory and not merely conservative.
    fn sample_sr_spec_unmarked() -> EngineSpec {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().fe_meta_persistent = false;
        spec
    }

    /// `sample_sr_spec_with_tls` with the creation-time marker cleared.
    fn sample_sr_spec_with_tls_unmarked() -> EngineSpec {
        let mut spec = sample_sr_spec_with_tls();
        spec.starrocks.as_mut().unwrap().fe_meta_persistent = false;
        spec
    }

    /// `sample_sr_spec` plus a delegation block with TLS enabled — the fixture
    /// for the ssl-config / ssl-cm tests below.
    fn sample_sr_spec_with_tls() -> EngineSpec {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().delegation = Some(StarRocksDelegation {
            jwks_url: "https://kc/realms/egret/protocol/openid-connect/certs".into(),
            issuer: "https://kc/realms/egret".into(),
            audience: "egret".into(),
            tls_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into(),
            tls_keystore_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n\
                                -----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n"
                .into(),
            tls_enabled: true,
        });
        spec
    }

    #[test]
    fn starrocks_ingress_targets_fe_http_with_tls() {
        let mut spec = sample_sr_spec();
        spec.name = "sr1".into();
        spec.namespace = "egret-engines".into();
        spec.config.ingress = Some(egret_engine_mgmt_api::Ingress {
            enabled: true,
            class_name: Some("nginx".into()),
            host: "sr1.localtest.me".into(),
            tls_secret: Some("sr1-ingress-tls".into()),
            tls_cert_secret_ref: None,
            tls_cert_pem: None,
            tls_key_pem: None,
            annotations: Default::default(),
        });
        let ing = starrocks_ingress(&spec).unwrap();
        assert_eq!(ing["kind"], "Ingress");
        assert_eq!(ing["metadata"]["name"], "sr1-fe");
        let b = &ing["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"];
        assert_eq!(b["name"], "sr1-fe-service");
        assert_eq!(b["port"]["number"], 8030);
        assert_eq!(ing["spec"]["tls"][0]["secretName"], "sr1-ingress-tls");
        spec.config.ingress.as_mut().unwrap().enabled = false;
        assert!(starrocks_ingress(&spec).is_none());
    }

    #[test]
    fn cr_has_fe_and_cn_shared_data() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert_eq!(cr["apiVersion"], "starrocks.com/v1");
        assert_eq!(cr["kind"], "StarRocksCluster");
        let spec = &cr["spec"];
        assert_eq!(
            spec["starRocksFeSpec"]["image"],
            "starrocks/fe-ubuntu:3.3-latest"
        );
        assert_eq!(
            spec["starRocksFeSpec"]["configMapInfo"]["resolveKey"],
            "fe.conf"
        );
        assert_eq!(
            spec["starRocksCnSpec"]["image"],
            "starrocks/cn-ubuntu:3.3-latest"
        );
        assert!(spec.get("starRocksBeSpec").is_none()); // no BE in shared-data
    }

    fn sr_spec_with_autoscaling(a: CnAutoscaling) -> EngineSpec {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().cn_autoscaling = Some(a);
        spec
    }

    #[test]
    fn cr_renders_cn_autoscaling() {
        let spec = sr_spec_with_autoscaling(CnAutoscaling {
            min_replicas: Some(1),
            max_replicas: 3,
            target_cpu_percent: Some(70),
        });
        let cr = starrocks_cluster_cr(&spec);
        let pol = &cr["spec"]["starRocksCnSpec"]["autoScalingPolicy"];
        assert_eq!(pol["version"], "v2");
        assert_eq!(pol["minReplicas"], 1);
        assert_eq!(pol["maxReplicas"], 3);
        let metric = &pol["hpaPolicy"]["metrics"][0];
        assert_eq!(metric["type"], "Resource");
        assert_eq!(metric["resource"]["name"], "cpu");
        assert_eq!(metric["resource"]["target"]["type"], "Utilization");
        assert_eq!(metric["resource"]["target"]["averageUtilization"], 70);
    }

    #[test]
    fn cr_autoscaling_no_min_no_cpu() {
        let spec = sr_spec_with_autoscaling(CnAutoscaling {
            min_replicas: None,
            max_replicas: 2,
            target_cpu_percent: None,
        });
        let cr = starrocks_cluster_cr(&spec);
        let pol = &cr["spec"]["starRocksCnSpec"]["autoScalingPolicy"];
        assert_eq!(pol["maxReplicas"], 2);
        assert!(pol.get("minReplicas").is_none());
        assert!(pol.get("hpaPolicy").is_none());
    }

    #[test]
    fn cr_autoscaling_disabled_sentinel() {
        let spec = sr_spec_with_autoscaling(CnAutoscaling {
            min_replicas: None,
            max_replicas: 0,
            target_cpu_percent: None,
        });
        let cr = starrocks_cluster_cr(&spec);
        assert!(cr["spec"]["starRocksCnSpec"]
            .get("autoScalingPolicy")
            .is_none());
    }

    #[test]
    fn cr_no_autoscaling_when_none() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert!(cr["spec"]["starRocksCnSpec"]
            .get("autoScalingPolicy")
            .is_none());
    }

    #[test]
    fn fe_configmap_has_shared_data_storage_and_ports() {
        let cm = starrocks_fe_configmap(&sample_sr_spec());
        let conf = cm["data"]["fe.conf"].as_str().unwrap();
        assert!(conf.contains("run_mode = shared_data"));
        assert!(conf.contains("query_port = 9030"));
        assert!(conf.contains("cloud_native_storage_type = S3"));
        assert!(conf.contains("aws_s3_endpoint = http://host.docker.internal:9000"));
        assert!(conf.contains("aws_s3_enable_path_style_access = true"));
        assert!(conf.contains("enable_load_volume_from_conf = true"));
        assert!(!conf.contains("access_control = ranger")); // not unless bound
        assert!(!conf.contains("ssl_")); // not unless delegation.tls_enabled
    }

    #[test]
    fn the_provisioned_ranger_files_equal_the_renderer_and_the_pre_refactor_golden() {
        use crate::ranger_config::{ranger_plugin_files, STARROCKS_FE_CONF_LINE};
        let mut spec = sample_sr_spec();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: None,
        });
        let rendered = ranger_plugin_files(
            egret_engine_mgmt_api::EngineKind::StarRocks,
            "sr1",
            "http://host.docker.internal:8080",
            egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS.into(),
            None,
        );
        let cm = starrocks_ranger_configmap(&spec).unwrap();
        let secret = starrocks_ranger_secret(&spec).unwrap();
        assert_eq!(
            cm["data"][RANGER_AUDIT_XML_KEY].as_str().unwrap(),
            rendered.get(RANGER_AUDIT_XML_KEY).unwrap()
        );
        assert_eq!(
            secret["stringData"][RANGER_SECURITY_XML_KEY]
                .as_str()
                .unwrap(),
            rendered.get(RANGER_SECURITY_XML_KEY).unwrap()
        );
        assert_eq!(
            secret["stringData"][RANGER_SECURITY_XML_KEY]
                .as_str()
                .unwrap(),
            include_str!(
                "../../egret-engine-mgmt-api/tests/golden/starrocks.ranger-starrocks-security.xml"
            )
        );
        assert_eq!(
            cm["data"][RANGER_AUDIT_XML_KEY].as_str().unwrap(),
            include_str!(
                "../../egret-engine-mgmt-api/tests/golden/starrocks.ranger-starrocks-audit.xml"
            )
        );
        // The whole fe.conf, byte for byte, and the admin-facing line inside it.
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            conf,
            include_str!("../../egret-engine-mgmt-api/tests/golden/starrocks.fe.conf")
        );
        assert!(conf.contains(rendered.get(STARROCKS_FE_CONF_LINE).unwrap()));
    }

    #[test]
    fn ranger_binding_adds_access_control_and_xml_configmap() {
        let mut spec = sample_sr_spec();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: None,
        });
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(conf.contains("access_control = ranger"));
        let xml_cm = starrocks_ranger_configmap(&spec).unwrap();
        let secret = starrocks_ranger_secret(&spec).unwrap();
        let xml = secret["stringData"]["ranger-starrocks-security.xml"]
            .as_str()
            .unwrap();
        assert!(xml.contains("<name>ranger.plugin.starrocks.service.name</name>"));
        assert!(xml.contains("<value>sr1</value>"));
        assert!(xml.contains("http://host.docker.internal:8080/ranger"));
        let cr = starrocks_cluster_cr(&spec);
        let mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap();
        assert!(mounts
            .iter()
            .any(|m| m["name"] == xml_cm["metadata"]["name"]));
    }

    /// The Ranger XML ConfigMap is mounted as INDIVIDUAL FILES, never at the
    /// bare `/opt/starrocks/fe/conf` directory.
    ///
    /// The discriminating half is the negative: a directory mount replaces
    /// everything the image ships in that directory, `fe.conf` included, so
    /// the FE cannot boot. Asserting only "the XML is mounted somewhere"
    /// passes against that bug.
    #[test]
    fn ranger_xml_mounts_per_file_never_the_bare_conf_dir() {
        let mut spec = sample_sr_spec();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: None,
        });
        let cr = starrocks_cluster_cr(&spec);
        let mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap()
            .clone();

        // Negative: nothing may target the conf directory itself.
        for m in &mounts {
            assert_ne!(
                m["mountPath"].as_str().unwrap(),
                "/opt/starrocks/fe/conf",
                "a bare conf-directory mount shadows fe.conf and stops the FE booting: {m}"
            );
        }
        // And every mount carries a non-empty `subPath`, which is what keeps
        // a ConfigMap projection from replacing the directory.
        for m in &mounts {
            assert!(
                m["subPath"].as_str().is_some_and(|p| !p.is_empty()),
                "every conf-dir mount must be a subPath file mount: {m}"
            );
        }

        // Positive: the audit key is mounted from the ConfigMap and the
        // security key — it carries the credential — from the Secret, each at
        // its own full path, and the key mounted is the key the object carries.
        let secret_mounts = cr["spec"]["starRocksFeSpec"]["secrets"]
            .as_array()
            .expect("the security XML is mounted from a Secret")
            .clone();
        for m in &secret_mounts {
            assert_ne!(m["mountPath"].as_str().unwrap(), "/opt/starrocks/fe/conf");
            assert!(m["subPath"].as_str().is_some_and(|p| !p.is_empty()));
        }
        let cm = starrocks_ranger_configmap(&spec).unwrap();
        let secret = starrocks_ranger_secret(&spec).unwrap();
        for (key, object, mounted, data) in [
            (
                "ranger-starrocks-audit.xml",
                "sr1-ranger-cm",
                &mounts,
                &cm["data"],
            ),
            (
                "ranger-starrocks-security.xml",
                "sr1-ranger-secret",
                &secret_mounts,
                &secret["stringData"],
            ),
        ] {
            assert!(data.get(key).is_some(), "{object} is missing key {key}");
            let m = mounted
                .iter()
                .find(|m| m["subPath"] == key)
                .unwrap_or_else(|| panic!("no subPath mount for {key}"));
            assert_eq!(m["name"], object);
            assert_eq!(
                m["mountPath"].as_str().unwrap(),
                format!("/opt/starrocks/fe/conf/{key}")
            );
        }
        assert_eq!(mounts.len(), 1, "the ConfigMap mounts the audit key only");
        assert_eq!(
            secret_mounts.len(),
            1,
            "the Secret mounts the security key only"
        );
        assert!(
            cm["data"].get("ranger-starrocks-security.xml").is_none(),
            "the security XML left the ConfigMap"
        );
    }

    #[test]
    fn no_ranger_binding_omits_xml_configmap() {
        assert!(starrocks_ranger_configmap(&sample_sr_spec()).is_none());
        assert!(starrocks_ranger_secret(&sample_sr_spec()).is_none());
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert!(cr["spec"]["starRocksFeSpec"].get("secrets").is_none());
    }

    const TEST_CREDENTIAL_ID: &str = "0b5ad7e2-0000-4000-8000-00000000c0de";

    fn bound_sr(credential: Option<(&str, &str)>) -> EngineSpec {
        let mut spec = sample_sr_spec();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: credential.map(|(u, p)| {
                crate::ranger_config::RenderableCredential::new(TEST_CREDENTIAL_ID, u, p)
                    .expect("renderable")
            }),
        });
        spec
    }

    /// A9: the credential is in `{name}-ranger-secret` and in no ConfigMap, CR
    /// or Debug output.
    #[test]
    fn the_credential_is_rendered_only_into_the_ranger_secret() {
        let spec = bound_sr(Some(("engine-1", "Pw_only-in-the-secret")));
        let secret = starrocks_ranger_secret(&spec).unwrap();
        assert_eq!(secret["kind"], "Secret");
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["metadata"]["name"], "sr1-ranger-secret");
        assert_eq!(secret["metadata"]["labels"]["egret/engine"], "sr1");
        assert!(secret["stringData"]["ranger-starrocks-security.xml"]
            .as_str()
            .unwrap()
            .contains(
                "<name>ranger.plugin.starrocks.policy.rest.client.password</name><value>Pw_only-in-the-secret</value>"
            ));
        assert_eq!(
            secret["stringData"].as_object().unwrap().len(),
            1,
            "the Secret holds the security XML only"
        );
        for obj in [
            Some(starrocks_fe_configmap(&spec)),
            starrocks_cn_configmap(&spec),
            starrocks_ranger_configmap(&spec),
            Some(starrocks_cluster_cr(&spec)),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !obj.to_string().contains("Pw_only-in-the-secret"),
                "{} {} carries the password",
                obj["kind"],
                obj["metadata"]["name"]
            );
        }
        assert!(!format!("{spec:?}").contains("Pw_only-in-the-secret"));
    }

    /// A rotation changes only the Secret; the FE mounts it by `subPath`,
    /// which is copied at container start, so the checksum must move.
    #[test]
    fn a_new_ranger_password_rolls_the_fe_and_the_same_one_does_not() {
        let ck = |pw: &str| {
            starrocks_cluster_cr(&bound_sr(Some(("engine-1", pw))))["spec"]["starRocksFeSpec"]
                ["annotations"]["egret/config-checksum"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(ck("pw-one"), ck("pw-one"), "deterministic");
        assert_ne!(ck("pw-one"), ck("pw-two"), "the FE would not roll");
    }

    #[tokio::test]
    async fn the_ranger_secret_is_applied_before_the_cluster() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        *kube.fail_apply_kind.lock().unwrap() = Some("StarRocksCluster".into());
        let p = StarRocksProvisioner::new(kube.clone());
        p.update(&bound_sr(Some(("engine-1", "pw"))))
            .await
            .expect_err("the CR apply fails");
        let applied = kube.applied.lock().unwrap();
        assert!(applied.contains_key("Secret/sr1-ranger-secret"));
        assert!(!applied.contains_key("StarRocksCluster/sr1"));
    }

    #[tokio::test]
    async fn unbinding_removes_the_ranger_secret_on_update() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        let p = StarRocksProvisioner::new(kube.clone());
        p.update(&sample_sr_spec()).await.unwrap();
        assert!(kube
            .deleted
            .lock()
            .unwrap()
            .contains(&"Secret/sr1-ranger-secret".to_string()));
    }

    /// TDD (T2 step 2): `ssl_*` keys appear ONLY when `delegation.tls_enabled`.
    #[test]
    fn delegation_tls_enabled_adds_ssl_config_and_configmap() {
        let spec = sample_sr_spec_with_tls();
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(conf.contains("ssl_keystore_location = /etc/starrocks/ssl/keystore"));
        assert!(conf.contains("ssl_keystore_password = changeit"));
        assert!(conf.contains("ssl_key_password = changeit"));
        assert!(conf.contains("ssl_force_secure_transport = false"));

        // The ConfigMap carries ONLY the public cert — never the private key.
        let ssl_cm = starrocks_ssl_configmap(&spec).expect("ssl-cm present when tls_enabled");
        assert_eq!(ssl_cm["metadata"]["name"], "sr1-ssl-cm");
        assert!(ssl_cm["data"]["fe.crt"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(
            ssl_cm["data"].get("fe.pem").is_none(),
            "cert+key bundle must NOT be in the ConfigMap"
        );
        let cm_json = ssl_cm.to_string();
        assert!(
            !cm_json.contains("PRIVATE KEY"),
            "no private key may appear anywhere in the ConfigMap"
        );

        // The cert+KEY bundle lives in a Secret, keyed `fe.pem`.
        let ssl_secret = starrocks_ssl_secret(&spec).expect("ssl-secret present when tls_enabled");
        assert_eq!(ssl_secret["kind"], "Secret");
        assert_eq!(ssl_secret["type"], "Opaque");
        assert_eq!(ssl_secret["metadata"]["name"], "sr1-ssl-secret");
        let fe_pem = ssl_secret["stringData"]["fe.pem"].as_str().unwrap();
        assert!(fe_pem.contains("BEGIN CERTIFICATE"));
        assert!(fe_pem.contains("BEGIN PRIVATE KEY"));

        let cr = starrocks_cluster_cr(&spec);
        // The cert+key Secret is delivered to the init-container via a
        // `secretKeyRef` ENV (by object name), NOT a `secrets` volume mount —
        // the operator hashes secret/configmap volume names, which an
        // init-container can't reference. So no `secrets` field is templated.
        assert!(cr["spec"]["starRocksFeSpec"].get("secrets").is_none());
        let init = &cr["spec"]["starRocksFeSpec"]["initContainers"][0];
        let env = init["env"].as_array().unwrap();
        assert!(env.iter().any(|e| e["name"] == "FE_PEM"
            && e["valueFrom"]["secretKeyRef"]["name"] == ssl_secret["metadata"]["name"]
            && e["valueFrom"]["secretKeyRef"]["key"] == "fe.pem"));
        // And the cert-only ConfigMap is NOT mounted into the FE (it's not
        // needed for the keystore build), so no `configMaps` entry references
        // it when Ranger is absent.
        assert!(cr["spec"]["starRocksFeSpec"].get("configMaps").is_none());
    }

    /// T2c TDD: when delegation TLS is enabled, the FE spec gets a
    /// build-keystore `initContainers` entry (mounting the cert+key Secret
    /// source + the writable keystore volume) and a `storageVolumes` entry
    /// providing that writable keystore volume at `/etc/starrocks/ssl` —
    /// matching `ssl_keystore_location = /etc/starrocks/ssl/keystore`. The
    /// keystore source comes from the SECRET, never a ConfigMap.
    #[test]
    fn keystore_volume_and_init_container_present() {
        let spec = sample_sr_spec_with_tls();
        let cr = starrocks_cluster_cr(&spec);
        let fe_spec = &cr["spec"]["starRocksFeSpec"];

        let volumes = fe_spec["storageVolumes"].as_array().unwrap();
        // The FE metadata PVC is always first; the keystore is APPENDED.
        assert_eq!(volumes.len(), 2);
        let ks = volumes
            .iter()
            .find(|v| v["name"] == "ssl-keystore")
            .expect("keystore volume present");
        assert_eq!(ks["mountPath"], "/etc/starrocks/ssl");
        assert!(ks["hostPath"]["path"].is_string());

        let init_containers = fe_spec["initContainers"].as_array().unwrap();
        assert_eq!(init_containers.len(), 1);
        let init = &init_containers[0];
        assert_eq!(init["name"], "build-keystore");
        assert!(init["command"].is_array());
        let args = init["args"].as_array().unwrap();
        assert!(args[0].as_str().unwrap().contains("openssl"));
        assert!(args[0].as_str().unwrap().contains("/tmp/fe.pem"));
        assert!(args[0]
            .as_str()
            .unwrap()
            .contains("/etc/starrocks/ssl/keystore"));

        // The init-container reads the cert+key from the Secret via a
        // `secretKeyRef` env (FE_PEM) — NOT a mounted volume (the operator
        // hashes secret volume names). It mounts ONLY the writable keystore vol.
        let env = init["env"].as_array().unwrap();
        assert!(env.iter().any(|e| e["name"] == "FE_PEM"
            && e["valueFrom"]["secretKeyRef"]["name"] == "sr1-ssl-secret"
            && e["valueFrom"]["secretKeyRef"]["key"] == "fe.pem"));
        let init_mounts = init["volumeMounts"].as_array().unwrap();
        assert!(init_mounts
            .iter()
            .any(|m| m["name"] == "ssl-keystore" && m["mountPath"] == "/etc/starrocks/ssl"));
        assert!(
            !init_mounts
                .iter()
                .any(|m| m["name"] == "sr1-ssl-secret" || m["name"] == "sr1-ssl-cm"),
            "init-container sources the key via secretKeyRef env, not a volume"
        );
    }

    /// The FE metadata volume is a PVC at the operator's own meta path, for
    /// EVERY engine (TLS or not). A `hostPath` here would be a regression: it
    /// survives a same-node restart and is destroyed silently on reschedule.
    #[test]
    fn fe_meta_volume_is_a_pvc_not_a_hostpath() {
        for spec in [sample_sr_spec(), sample_sr_spec_with_tls()] {
            let cr = starrocks_cluster_cr(&spec);
            let volumes = cr["spec"]["starRocksFeSpec"]["storageVolumes"]
                .as_array()
                .expect("FE spec carries storageVolumes");
            let meta = volumes
                .iter()
                .find(|v| v["mountPath"] == "/opt/starrocks/fe/meta")
                .expect("a storageVolumes entry mounts the FE meta path");
            assert_eq!(meta["name"], "fe-meta");
            // A PVC: a storage SIZE and no hostPath at all.
            assert_eq!(meta["storageSize"], "20Gi");
            assert!(
                meta.get("hostPath").is_none(),
                "FE metadata must never be a hostPath: {meta}"
            );
            // Unset class ⇒ the key is ABSENT so the default StorageClass
            // binds. An empty string would mean "pre-provisioned PV only".
            assert!(meta.get("storageClassName").is_none());
        }
    }

    /// The class and size come from the StarRocks config surface.
    #[test]
    fn fe_meta_volume_honours_configured_class_and_size() {
        let mut spec = sample_sr_spec();
        {
            let sr = spec.starrocks.as_mut().unwrap();
            sr.fe_meta_storage_class = Some("fast-ssd".into());
            sr.fe_meta_storage_size = Some("50Gi".into());
        }
        let cr = starrocks_cluster_cr(&spec);
        let meta = &cr["spec"]["starRocksFeSpec"]["storageVolumes"][0];
        assert_eq!(meta["storageClassName"], "fast-ssd");
        assert_eq!(meta["storageSize"], "50Gi");

        // Blank strings fall back to the defaults rather than rendering `""`.
        {
            let sr = spec.starrocks.as_mut().unwrap();
            sr.fe_meta_storage_class = Some("   ".into());
            sr.fe_meta_storage_size = Some("".into());
        }
        let cr = starrocks_cluster_cr(&spec);
        let meta = &cr["spec"]["starRocksFeSpec"]["storageVolumes"][0];
        assert!(meta.get("storageClassName").is_none());
        assert_eq!(meta["storageSize"], "20Gi");
    }

    /// The careless-overwrite guard: the TLS branch must APPEND the keystore to
    /// `storageVolumes`, so enabling TLS cannot drop the metadata PVC (and the
    /// init-container still finds its keystore volume alongside it).
    #[test]
    fn keystore_volume_keeps_fe_meta_volume() {
        let cr = starrocks_cluster_cr(&sample_sr_spec_with_tls());
        let fe_spec = &cr["spec"]["starRocksFeSpec"];
        let volumes = fe_spec["storageVolumes"].as_array().unwrap();
        let names: Vec<&str> = volumes
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"fe-meta") && names.contains(&"ssl-keystore"),
            "both volumes must render together, got {names:?}"
        );
        // And the meta entry is still the PVC, not collapsed into a hostPath.
        let meta = volumes.iter().find(|v| v["name"] == "fe-meta").unwrap();
        assert!(meta["storageSize"].is_string());
        assert!(meta.get("hostPath").is_none());
    }

    /// An engine WITHOUT the creation-time marker renders NO meta volume entry
    /// and — with TLS off — no `storageVolumes` KEY AT ALL, which is byte-for-byte
    /// what egret rendered before the PVC existed.
    ///
    /// This is the whole correction. `volumeClaimTemplates` is IMMUTABLE on a
    /// live StatefulSet (measured on kind 2026-09-26): rendering the claim for
    /// an engine that already has one does not migrate it, it makes every
    /// operator reconcile rejected by the API server and parks the
    /// `StarRocksCluster` at `phase=failed` forever, so nothing else ever
    /// applies to it either. An empty ARRAY would fail this test too, on
    /// purpose: `{"storageVolumes": []}` is a different document from no key,
    /// and shipping it would be a live change for every existing engine.
    #[test]
    fn fe_meta_volume_absent_for_an_unmarked_engine() {
        let cr = starrocks_cluster_cr(&sample_sr_spec_unmarked());
        let fe_spec = &cr["spec"]["starRocksFeSpec"];
        assert!(
            fe_spec.get("storageVolumes").is_none(),
            "an unmarked, TLS-less engine must render no storageVolumes key at \
             all (an empty array is already a change): {}",
            fe_spec["storageVolumes"]
        );

        // With TLS the key exists — it always did — but it holds the keystore
        // and NOTHING else, exactly the pre-PVC shape.
        let cr = starrocks_cluster_cr(&sample_sr_spec_with_tls_unmarked());
        let volumes = cr["spec"]["starRocksFeSpec"]["storageVolumes"]
            .as_array()
            .expect("the TLS branch still renders storageVolumes");
        assert_eq!(
            volumes.len(),
            1,
            "pre-PVC shape is the keystore alone: {volumes:?}"
        );
        assert_eq!(volumes[0]["name"], "ssl-keystore");
        assert_eq!(volumes[0]["mountPath"], "/etc/starrocks/ssl");
        assert!(volumes[0]["hostPath"]["path"].is_string());
        assert!(
            !volumes
                .iter()
                .any(|v| v["mountPath"] == "/opt/starrocks/fe/meta"),
            "nothing may mount the meta path for an unmarked engine: {volumes:?}"
        );
    }

    /// An engine WITH the marker renders the PVC — and when TLS is on, the
    /// keystore entry is still there ALONGSIDE it rather than replacing it or
    /// being replaced by it.
    #[test]
    fn fe_meta_volume_rendered_for_a_marked_engine() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        let volumes = cr["spec"]["starRocksFeSpec"]["storageVolumes"]
            .as_array()
            .expect("a marked engine renders storageVolumes");
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0]["name"], "fe-meta");
        assert_eq!(volumes[0]["mountPath"], "/opt/starrocks/fe/meta");
        assert_eq!(volumes[0]["storageSize"], "20Gi");
        assert!(volumes[0].get("hostPath").is_none());

        let cr = starrocks_cluster_cr(&sample_sr_spec_with_tls());
        let volumes = cr["spec"]["starRocksFeSpec"]["storageVolumes"]
            .as_array()
            .expect("a marked TLS engine renders storageVolumes");
        let by_name = |n: &str| volumes.iter().find(|v| v["name"] == n);
        let meta = by_name("fe-meta").expect("the meta PVC survives the TLS branch");
        assert_eq!(meta["mountPath"], "/opt/starrocks/fe/meta");
        assert_eq!(meta["storageSize"], "20Gi");
        assert!(meta.get("hostPath").is_none());
        let ks = by_name("ssl-keystore").expect("the keystore is ALONGSIDE the PVC, not replaced");
        assert_eq!(ks["mountPath"], "/etc/starrocks/ssl");
        assert!(ks["hostPath"]["path"].is_string());
        assert_eq!(volumes.len(), 2, "exactly the two entries: {volumes:?}");
    }

    /// No delegation TLS → no init-container/storageVolumes/secrets noise on
    /// the FE spec.
    #[test]
    fn no_delegation_tls_omits_keystore_init_container() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        let fe_spec = &cr["spec"]["starRocksFeSpec"];
        assert!(fe_spec.get("initContainers").is_none());
        assert!(fe_spec.get("secrets").is_none());
        // `storageVolumes` is NOT absent any more: the FE metadata PVC is
        // unconditional. Without TLS it is the ONLY entry.
        let volumes = fe_spec["storageVolumes"].as_array().unwrap();
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0]["name"], "fe-meta");
    }

    /// The private key must NEVER appear in any ConfigMap egret templates for
    /// a TLS-enabled StarRocks engine (fe-cm, ranger-cm, or ssl-cm).
    #[test]
    fn no_private_key_in_any_configmap() {
        let mut spec = sample_sr_spec_with_tls();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: None,
        });
        for cm in [
            Some(starrocks_fe_configmap(&spec)),
            starrocks_ranger_configmap(&spec),
            starrocks_ssl_configmap(&spec),
        ]
        .into_iter()
        .flatten()
        {
            assert_eq!(cm["kind"], "ConfigMap");
            assert!(
                !cm.to_string().contains("PRIVATE KEY"),
                "private key leaked into a ConfigMap: {}",
                cm["metadata"]["name"]
            );
        }
        // The key IS present — but only in the Secret.
        let secret = starrocks_ssl_secret(&spec).unwrap();
        assert!(secret.to_string().contains("PRIVATE KEY"));
    }

    /// No delegation / TLS-off → no ssl Secret (mirrors the ConfigMap gating).
    #[test]
    fn no_delegation_omits_ssl_secret() {
        assert!(starrocks_ssl_secret(&sample_sr_spec()).is_none());
        let mut spec = sample_sr_spec_with_tls();
        spec.starrocks
            .as_mut()
            .unwrap()
            .delegation
            .as_mut()
            .unwrap()
            .tls_enabled = false;
        assert!(starrocks_ssl_secret(&spec).is_none());
    }

    /// No delegation block at all → no ssl config/configmap (slice-2 default).
    #[test]
    fn no_delegation_omits_ssl_config_and_configmap() {
        assert!(starrocks_ssl_configmap(&sample_sr_spec()).is_none());
    }

    /// Delegation present but `tls_enabled = false` → also no ssl config/configmap.
    #[test]
    fn delegation_without_tls_enabled_omits_ssl_config_and_configmap() {
        let mut spec = sample_sr_spec_with_tls();
        spec.starrocks
            .as_mut()
            .unwrap()
            .delegation
            .as_mut()
            .unwrap()
            .tls_enabled = false;
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!conf.contains("ssl_"));
        assert!(starrocks_ssl_configmap(&spec).is_none());
        let cr = starrocks_cluster_cr(&spec);
        assert!(cr["spec"]["starRocksFeSpec"].get("configMaps").is_none());
    }

    /// Both Ranger AND TLS bound: the FE mounts the Ranger XML via
    /// `configMaps`, and the cert+key source reaches the keystore init-container
    /// via a `secretKeyRef` env (no `secrets` volume mount — the operator hashes
    /// those names). So `configMaps` carries only the Ranger entry, and no
    /// `secrets` field is templated.
    #[test]
    fn ranger_and_tls_mount_on_their_respective_fields() {
        let mut spec = sample_sr_spec_with_tls();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: None,
        });
        let cr = starrocks_cluster_cr(&spec);
        let cm_mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap();
        // The audit XML (a `subPath` file mount), nothing else.
        assert_eq!(cm_mounts.len(), 1);
        assert!(cm_mounts.iter().all(|m| m["name"] == "sr1-ranger-cm"));
        assert!(
            !cm_mounts.iter().any(|m| m["name"] == "sr1-ssl-cm"),
            "cert-only ssl-cm is not mounted into the FE"
        );

        // The only `secrets` mount is the Ranger security XML; the TLS key
        // reaches the init-container via env.
        let secrets = cr["spec"]["starRocksFeSpec"]["secrets"].as_array().unwrap();
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0]["name"], "sr1-ranger-secret");
        let init = &cr["spec"]["starRocksFeSpec"]["initContainers"][0];
        assert!(init["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "FE_PEM"
                && e["valueFrom"]["secretKeyRef"]["name"] == "sr1-ssl-secret"));
    }

    /// Task 2 TDD: an existing base key whose name is in the overrides map is
    /// REPLACED in place, exactly once, dropping the old value line.
    #[test]
    fn merge_fe_conf_replaces_existing_key() {
        let base = "# header\nsys_log_level = INFO\nquery_port = 9030\n";
        let overrides = BTreeMap::from([("sys_log_level".to_string(), "WARN".to_string())]);
        let out = merge_fe_conf(base, &overrides, &[]);
        assert!(out.contains("sys_log_level = WARN"));
        assert!(!out.contains("sys_log_level = INFO"));
        assert_eq!(out.matches("sys_log_level =").count(), 1);
        // Unrelated lines are preserved verbatim.
        assert!(out.contains("# header"));
        assert!(out.contains("query_port = 9030"));
    }

    /// An override key absent from the base is appended as `key = value`.
    #[test]
    fn merge_fe_conf_appends_new_key() {
        let base = "sys_log_level = INFO\n";
        let overrides = BTreeMap::from([("query_timeout".to_string(), "600".to_string())]);
        let out = merge_fe_conf(base, &overrides, &[]);
        assert!(out.contains("sys_log_level = INFO"));
        assert!(out.contains("query_timeout = 600"));
    }

    /// A protected override key is SKIPPED — egret's base line wins, the admin
    /// value never appears (belt-and-suspenders behind the API 400 guard).
    #[test]
    fn merge_fe_conf_skips_protected() {
        let base = "run_mode = shared_data\n";
        let overrides = BTreeMap::from([("run_mode".to_string(), "shared_nothing".to_string())]);
        let out = merge_fe_conf(base, &overrides, &["run_mode"]);
        assert!(out.contains("run_mode = shared_data"));
        assert!(!out.contains("shared_nothing"));
    }

    /// Empty overrides → output is byte-identical to the base.
    #[test]
    fn merge_fe_conf_empty_is_identity() {
        let base = "# header\nsys_log_level = INFO\nrun_mode = shared_data\n";
        let overrides = BTreeMap::new();
        assert_eq!(merge_fe_conf(base, &overrides, SR_PROTECTED_FE_KEYS), base);
    }

    /// Integration through `starrocks_fe_configmap`: an admin `feConfig`
    /// override for a non-protected key lands in the rendered `fe.conf` once.
    #[test]
    fn fe_configmap_applies_override() {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().fe_config =
            BTreeMap::from([("sys_log_level".to_string(), "WARN".to_string())]);
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(conf.contains("sys_log_level = WARN"));
        assert!(!conf.contains("sys_log_level = INFO"));
        assert_eq!(conf.matches("sys_log_level =").count(), 1);
    }

    /// Task 3 TDD: no CN `cn.conf` ConfigMap is emitted when `cn_config` is
    /// empty — CN uses the operator defaults (behavior-preserving).
    #[test]
    fn cn_configmap_none_when_empty() {
        assert!(starrocks_cn_configmap(&sample_sr_spec()).is_none());
    }

    /// A non-empty `cn_config` renders a `{name}-cn-cm` ConfigMap whose
    /// `cn.conf` carries the admin override lines.
    #[test]
    fn cn_configmap_some_when_nonempty() {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().cn_config =
            BTreeMap::from([("sys_log_level".to_string(), "WARN".to_string())]);
        let cm = starrocks_cn_configmap(&spec).expect("cn-cm present when cn_config non-empty");
        assert_eq!(cm["kind"], "ConfigMap");
        assert_eq!(cm["metadata"]["name"], "sr1-cn-cm");
        let conf = cm["data"]["cn.conf"].as_str().unwrap();
        assert!(conf.contains("sys_log_level = WARN"));
    }

    /// A non-empty `cn_config` wires `starRocksCnSpec.configMapInfo` pointing at
    /// the `{name}-cn-cm` ConfigMap with `resolveKey: cn.conf`.
    #[test]
    fn cr_wires_cn_configmapinfo_when_nonempty() {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().cn_config =
            BTreeMap::from([("sys_log_level".to_string(), "WARN".to_string())]);
        let cr = starrocks_cluster_cr(&spec);
        let info = &cr["spec"]["starRocksCnSpec"]["configMapInfo"];
        assert_eq!(info["configMapName"], "sr1-cn-cm");
        assert_eq!(info["resolveKey"], "cn.conf");
    }

    /// Empty `cn_config` → CN spec has no `configMapInfo` (unchanged from today).
    #[test]
    fn cr_no_cn_configmapinfo_when_empty() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert!(cr["spec"]["starRocksCnSpec"].get("configMapInfo").is_none());
    }

    /// The provisioner applies the CN ConfigMap (alongside the FE one) when
    /// `cn_config` is non-empty, and omits it when empty.
    #[tokio::test]
    async fn provisioner_applies_cn_configmap() {
        use crate::kube_client::FakeKube;

        // Non-empty → CN ConfigMap is among the applied objects.
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().cn_config =
            BTreeMap::from([("sys_log_level".to_string(), "WARN".to_string())]);
        let kube = Arc::new(FakeKube::default());
        StarRocksProvisioner::new(kube.clone())
            .provision(&spec)
            .await
            .unwrap();
        {
            let applied = kube.applied.lock().unwrap();
            let cn = applied
                .get("ConfigMap/sr1-cn-cm")
                .expect("CN ConfigMap applied");
            assert!(cn["data"]["cn.conf"]
                .as_str()
                .unwrap()
                .contains("sys_log_level = WARN"));
        }

        // Empty → no CN ConfigMap applied (behavior-preserving).
        let kube = Arc::new(FakeKube::default());
        StarRocksProvisioner::new(kube.clone())
            .provision(&sample_sr_spec())
            .await
            .unwrap();
        assert!(kube
            .applied
            .lock()
            .unwrap()
            .get("ConfigMap/sr1-cn-cm")
            .is_none());
    }

    /// The provisioner applies the initpwd Secret + ConfigMap + Job when the
    /// engine has a root password, and applies none of them when it doesn't
    /// (behavior-preserving for legacy/passwordless engines). Because a Job's
    /// `spec.template` is immutable, the Job is deleted-before-applied so a
    /// re-provision gets a fresh, idempotent run.
    #[tokio::test]
    async fn provisioner_applies_initpwd_when_root_password_set() {
        use crate::kube_client::FakeKube;

        // With a root password → Secret + ConfigMap + Job are all applied, and
        // the Job was deleted (delete-before-apply) first.
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().root_password = Some("s3cret".to_string());
        let kube = Arc::new(FakeKube::default());
        StarRocksProvisioner::new(kube.clone())
            .provision(&spec)
            .await
            .unwrap();
        {
            let applied = kube.applied.lock().unwrap();
            let secret = applied
                .get("Secret/sr1-initpwd-secret")
                .expect("initpwd Secret applied");
            assert_eq!(secret["stringData"]["password"], "s3cret");
            applied
                .get("ConfigMap/sr1-initpwd-shell")
                .expect("initpwd ConfigMap applied");
            applied.get("Job/sr1-initpwd").expect("initpwd Job applied");
        }
        assert!(
            kube.deleted
                .lock()
                .unwrap()
                .contains(&"Job/sr1-initpwd".to_string()),
            "initpwd Job is deleted-before-applied (immutable spec.template)"
        );

        // Without a root password → none of the initpwd objects are applied.
        let kube = Arc::new(FakeKube::default());
        StarRocksProvisioner::new(kube.clone())
            .provision(&sample_sr_spec())
            .await
            .unwrap();
        {
            let applied = kube.applied.lock().unwrap();
            assert!(applied.get("Secret/sr1-initpwd-secret").is_none());
            assert!(applied.get("ConfigMap/sr1-initpwd-shell").is_none());
            assert!(applied.get("Job/sr1-initpwd").is_none());
        }
    }

    /// `delete` best-effort cleans up the initpwd Job, ConfigMap, and Secret
    /// alongside the CR + other orphans.
    #[tokio::test]
    async fn provisioner_delete_cleans_up_initpwd() {
        use crate::kube_client::FakeKube;

        let kube = Arc::new(FakeKube::default());
        StarRocksProvisioner::new(kube.clone())
            .delete("egret-engines", "sr1")
            .await
            .unwrap();
        let deleted = kube.deleted.lock().unwrap();
        assert!(deleted.contains(&"Job/sr1-initpwd".to_string()));
        assert!(deleted.contains(&"ConfigMap/sr1-initpwd-shell".to_string()));
        assert!(deleted.contains(&"Secret/sr1-initpwd-secret".to_string()));
        assert!(
            deleted.contains(&"Secret/sr1-ranger-secret".to_string()),
            "engine teardown removes the Ranger credential Secret"
        );
    }

    /// D5/D6: a failed CR delete surfaces, and no longer aborts the cleanup of the
    /// ConfigMaps/Secrets/Job/Ingress egret owns.
    #[tokio::test]
    async fn provisioner_delete_surfaces_a_failed_cluster_delete() {
        use crate::kube_client::FakeKube;

        let kube = Arc::new(FakeKube::default());
        kube.fail_delete("StarRocksCluster", "sr1", "forbidden");
        let err = StarRocksProvisioner::new(kube.clone())
            .delete("egret-engines", "sr1")
            .await
            .expect_err("the CR delete failure must surface");
        assert!(format!("{err:#}").contains("StarRocksCluster/sr1: forbidden"));
        let deleted = kube.deleted.lock().unwrap();
        assert_eq!(deleted.len(), 11, "{deleted:?}");
        assert!(
            deleted.contains(&"ConfigMap/sr1-fe-cm".to_string()),
            "{deleted:?}"
        );
    }

    /// T4 TDD: the restart merge-patch body stamps the restart annotation on
    /// BOTH the FE and CN pod-annotation field (`annotations`, which the
    /// operator propagates to each role's pod template), and the base CR carries
    /// no such annotation (restart is opt-in, never on a plain provision).
    #[test]
    fn restart_stamps_fe_and_cn() {
        let patch = starrocks_restart_patch("1720000000000");
        assert_eq!(
            patch["spec"]["starRocksFeSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );
        assert_eq!(
            patch["spec"]["starRocksCnSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );

        // A plain provision CR must NOT carry the restart stamp on either role
        // (restart is opt-in). Asserted by KEY, not by the absence of the whole
        // `annotations` map: the FE always carries `egret/config-checksum`
        // there, which is what makes a config change roll the pod at all.
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert!(cr["spec"]["starRocksFeSpec"]["annotations"]
            .get(RESTART_STAMP_KEY)
            .is_none());
        assert!(cr["spec"]["starRocksCnSpec"].get("annotations").is_none());
    }

    /// C1: re-pointing the engine at a DIFFERENT Ranger profile must change the
    /// FE pod template, so the operator replaces the pod and the plugin reads
    /// the new XML.
    ///
    /// Only the ranger CONTENT differs between the two specs here — both are
    /// bound, so the mount list, `fe.conf`'s `access_control = ranger` line and
    /// every other CR field are identical. That is the whole point: before the
    /// fix the two CRs were BYTE-IDENTICAL, nothing in the pod template moved,
    /// no pod was replaced, and the FE went on enforcing the OLD profile while
    /// the API and the database said the new one. A test that also changed a
    /// mount would roll the pod for an unrelated reason and pass against the
    /// bug — which is exactly how this hid from the live gate.
    #[test]
    fn rebinding_to_another_profile_changes_the_fe_pod_template() {
        let bound_to = |svc: &str| {
            let mut spec = sample_sr_spec();
            spec.ranger = Some(RangerBinding {
                service_name: svc.into(),
                egret_url: "http://host.docker.internal:8080".into(),
                poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
                credential: None,
            });
            starrocks_cluster_cr(&spec)
        };
        let a = bound_to("permissive_profile");
        let b = bound_to("restrictive_profile");

        // The pod template must differ.
        assert_ne!(
            a["spec"]["starRocksFeSpec"], b["spec"]["starRocksFeSpec"],
            "two profiles rendered an identical FE spec: nothing would roll the pod"
        );
        let ck_a = a["spec"]["starRocksFeSpec"]["annotations"]["egret/config-checksum"]
            .as_str()
            .expect("FE carries a config checksum");
        let ck_b = b["spec"]["starRocksFeSpec"]["annotations"]["egret/config-checksum"]
            .as_str()
            .expect("FE carries a config checksum");
        assert_ne!(ck_a, ck_b, "the checksum must move with the ranger content");

        // And the checksum is the ONLY thing carrying the difference — strip it
        // and the two CRs collapse into each other. This is the assertion that
        // states the defect: without the annotation there is nothing else for
        // the operator to notice.
        let strip = |mut cr: serde_json::Value| {
            cr["spec"]["starRocksFeSpec"]["annotations"]
                .as_object_mut()
                .unwrap()
                .remove("egret/config-checksum");
            cr
        };
        assert_eq!(
            strip(a.clone()),
            strip(b.clone()),
            "if these ever differ for another reason, this test is no longer \
             measuring the checksum"
        );

        // Same profile, same checksum: the stamp is content-derived, never a
        // timestamp, so an unchanged apply must NOT roll the pod.
        assert_eq!(
            bound_to("permissive_profile")["spec"]["starRocksFeSpec"]["annotations"]
                ["egret/config-checksum"],
            ck_a,
            "an unchanged spec must hash to the same value"
        );
    }

    /// The checksum also moves when the ranger binding is ADDED or REMOVED, and
    /// when `fe.conf` itself changes — both are FE mounts that are resolved at
    /// boot and never refreshed.
    #[test]
    fn fe_config_checksum_moves_on_binding_and_on_fe_conf_changes() {
        let unbound = sample_sr_spec();
        let mut bound = sample_sr_spec();
        bound.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: None,
        });
        let ck = |spec: &EngineSpec| {
            starrocks_cluster_cr(spec)["spec"]["starRocksFeSpec"]["annotations"]
                ["egret/config-checksum"]
                .as_str()
                .expect("checksum")
                .to_string()
        };
        assert_ne!(ck(&unbound), ck(&bound), "binding must move the checksum");

        let mut fe_conf_changed = sample_sr_spec();
        fe_conf_changed
            .starrocks
            .as_mut()
            .unwrap()
            .fe_config
            .insert("sys_log_level".into(), "DEBUG".into());
        assert_ne!(
            ck(&unbound),
            ck(&fe_conf_changed),
            "an fe.conf change must move the checksum too"
        );
    }

    /// Task 5 TDD: the admin pod/scheduling fields render onto BOTH the FE and
    /// CN role specs, using each role's operator keys.
    #[test]
    fn pod_fields_on_both_roles() {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().pod = Some(StarRocksPod {
            service_account: Some("sa".into()),
            node_selector: BTreeMap::from([("disktype".to_string(), "ssd".to_string())]),
            tolerations: Some(json!([{ "key": "dedicated", "operator": "Exists" }])),
            ..Default::default()
        });
        let cr = starrocks_cluster_cr(&spec);
        for role in ["starRocksFeSpec", "starRocksCnSpec"] {
            let rs = &cr["spec"][role];
            assert_eq!(rs["serviceAccount"], "sa", "{role}");
            assert_eq!(rs["nodeSelector"]["disktype"], "ssd", "{role}");
            assert_eq!(rs["tolerations"][0]["key"], "dedicated", "{role}");
        }
    }

    /// Env vars use the role-SPECIFIC operator key (`feEnvVars` on FE,
    /// `cnEnvVars` on CN) — never a generic `env`.
    #[test]
    fn pod_env_uses_role_specific_key() {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().pod = Some(StarRocksPod {
            env: vec![json!({ "name": "FOO", "value": "bar" })],
            ..Default::default()
        });
        let cr = starrocks_cluster_cr(&spec);
        let fe = &cr["spec"]["starRocksFeSpec"];
        assert_eq!(fe["feEnvVars"][0]["name"], "FOO");
        assert_eq!(fe["feEnvVars"][0]["value"], "bar");
        assert!(fe.get("env").is_none());
        assert!(fe.get("cnEnvVars").is_none());
        let cn = &cr["spec"]["starRocksCnSpec"];
        assert_eq!(cn["cnEnvVars"][0]["name"], "FOO");
        assert!(cn.get("env").is_none());
        assert!(cn.get("feEnvVars").is_none());
    }

    /// Admin `init_containers` are APPENDED to the FE spec — the SSL
    /// build-keystore init-container the TLS block already set MUST survive.
    #[test]
    fn pod_initcontainers_preserve_ssl() {
        let mut spec = sample_sr_spec_with_tls();
        spec.starrocks.as_mut().unwrap().pod = Some(StarRocksPod {
            init_containers: Some(json!([{ "name": "admin-init", "image": "busybox" }])),
            ..Default::default()
        });
        let cr = starrocks_cluster_cr(&spec);
        let inits = cr["spec"]["starRocksFeSpec"]["initContainers"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = inits.iter().filter_map(|c| c["name"].as_str()).collect();
        assert!(
            names.contains(&"build-keystore"),
            "SSL keystore init survives: {names:?}"
        );
        assert!(
            names.contains(&"admin-init"),
            "admin init appended: {names:?}"
        );
        assert_eq!(inits.len(), 2);
    }

    /// `pod = None` is a no-op: no pod/scheduling keys appear on either role,
    /// and a TLS engine's SSL init-container is still present and alone.
    #[test]
    fn pod_none_is_noop() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        for role in ["starRocksFeSpec", "starRocksCnSpec"] {
            let rs = &cr["spec"][role];
            for k in [
                "serviceAccount",
                "nodeSelector",
                "podLabels",
                "tolerations",
                "affinity",
                "securityContext",
                "imagePullSecrets",
                "sidecars",
                "feEnvVars",
                "cnEnvVars",
            ] {
                assert!(rs.get(k).is_none(), "{role} unexpectedly has {k}");
            }
        }
        // `annotations` is the one exception, and only on the FE: it always
        // carries `egret/config-checksum` (that is what rolls the pod when a
        // mounted ConfigMap's content changes). `pod = None` must add NOTHING
        // beyond it, and must not give the CN an annotations map at all.
        assert_eq!(
            cr["spec"]["starRocksFeSpec"]["annotations"]
                .as_object()
                .expect("FE annotations")
                .keys()
                .collect::<Vec<_>>(),
            vec![CONFIG_CHECKSUM_KEY],
        );
        assert!(cr["spec"]["starRocksCnSpec"].get("annotations").is_none());
        // TLS engine, pod = None: the SSL init-container is present and alone.
        let cr = starrocks_cluster_cr(&sample_sr_spec_with_tls());
        let inits = cr["spec"]["starRocksFeSpec"]["initContainers"]
            .as_array()
            .unwrap();
        assert_eq!(inits.len(), 1);
        assert_eq!(inits[0]["name"], "build-keystore");
    }

    /// `StarRocksProvisioner::restart` JSON-merge-patches the existing
    /// `StarRocksCluster` CR (by name) with the stamp on both roles — it never
    /// re-applies the full CR (which would need the full spec and could wipe
    /// operator-managed fields).
    #[tokio::test]
    async fn provisioner_restart_merge_patches_cr() {
        use crate::kube_client::FakeKube;
        let kube = Arc::new(FakeKube::default());
        let prov = StarRocksProvisioner::new(kube.clone());
        prov.restart("egret-engines", "sr1", "1720000000000")
            .await
            .unwrap();
        let patched = kube.patched.lock().unwrap();
        assert_eq!(patched.len(), 1, "exactly one merge-patch");
        let (key, patch) = &patched[0];
        assert_eq!(key, "StarRocksCluster/sr1");
        assert_eq!(
            patch["spec"]["starRocksFeSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );
        assert_eq!(
            patch["spec"]["starRocksCnSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );
        // Restart must NOT re-apply the CR (no full-spec apply on this path).
        assert!(
            kube.applied.lock().unwrap().is_empty(),
            "restart uses merge-patch, not full apply"
        );
    }

    /// `sample_sr_spec` with the FE `root` password set — the fixture for the
    /// initpwd Secret/ConfigMap/Job builders.
    fn sample_sr_spec_with_root_pw() -> EngineSpec {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().root_password = Some("s3cr3t-root".into());
        spec
    }

    /// RP-T1 TDD: no root password (legacy passwordless) → all three initpwd
    /// builders return `None` (nothing new is templated for existing engines).
    #[test]
    fn initpwd_none_when_no_root_password() {
        let spec = sample_sr_spec();
        assert!(starrocks_root_password_secret(&spec).is_none());
        assert!(starrocks_initpwd_configmap(&spec).is_none());
        assert!(starrocks_initpwd_job(&spec).is_none());

        // An explicitly-empty password is also treated as "no password".
        let mut empty = sample_sr_spec();
        empty.starrocks.as_mut().unwrap().root_password = Some(String::new());
        assert!(starrocks_root_password_secret(&empty).is_none());
        assert!(starrocks_initpwd_configmap(&empty).is_none());
        assert!(starrocks_initpwd_job(&empty).is_none());
    }

    /// The Secret is an Opaque `{name}-initpwd-secret` carrying the root
    /// password under `stringData.password`, with egret's managed-by labels.
    #[test]
    fn initpwd_secret_shape() {
        let spec = sample_sr_spec_with_root_pw();
        let secret = starrocks_root_password_secret(&spec).expect("secret present when pw set");
        assert_eq!(secret["kind"], "Secret");
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["metadata"]["name"], "sr1-initpwd-secret");
        assert_eq!(secret["metadata"]["namespace"], "egret-engines");
        assert_eq!(
            secret["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "egret"
        );
        assert_eq!(secret["stringData"]["password"], "s3cr3t-root");
    }

    /// The ConfigMap is `{name}-initpwd-shell` and carries the operator's
    /// verbatim idempotent `fe_initpwd.sh` (SET PASSWORD + 1045/1064 branch).
    #[test]
    fn initpwd_configmap_carries_script() {
        let spec = sample_sr_spec_with_root_pw();
        let cm = starrocks_initpwd_configmap(&spec).expect("configmap present when pw set");
        assert_eq!(cm["kind"], "ConfigMap");
        assert_eq!(cm["metadata"]["name"], "sr1-initpwd-shell");
        let script = cm["data"]["fe_initpwd.sh"].as_str().unwrap();
        assert!(script.contains("SET PASSWORD = PASSWORD('$INIT_PWD');"));
        assert!(script.contains("--connect-timeout 30 -h $1 -P $2 -u root"));
        // Idempotency: already-changed password (1045) or syntax (1064) exits 0.
        assert!(script.contains("1045"));
        assert!(script.contains("1064"));
    }

    /// The Job is a batch/v1 `{name}-initpwd` running the script against the FE
    /// service in-cluster, with INIT_PWD from the initpwd Secret and the script
    /// mounted from the initpwd ConfigMap.
    #[test]
    fn initpwd_job_shape() {
        let spec = sample_sr_spec_with_root_pw();
        let job = starrocks_initpwd_job(&spec).expect("job present when pw set");
        assert_eq!(job["apiVersion"], "batch/v1");
        assert_eq!(job["kind"], "Job");
        assert_eq!(job["metadata"]["name"], "sr1-initpwd");
        assert_eq!(job["metadata"]["namespace"], "egret-engines");

        let jspec = &job["spec"];
        assert_eq!(jspec["backoffLimit"], 10);
        let pod = &jspec["template"]["spec"];
        assert_eq!(pod["restartPolicy"], "OnFailure");

        let c = &pod["containers"][0];
        assert_eq!(c["name"], "initpwd");
        assert_eq!(c["image"], "starrocks/fe-ubuntu:3.3-latest");
        assert_eq!(c["command"][0], "/bin/bash");
        let args = c["args"].as_array().unwrap();
        assert_eq!(args[0], "/opt/starrocks/fe_initpwd.sh");
        assert_eq!(args[1], "sr1-fe-service.egret-engines");
        assert_eq!(args[2], "9030");

        // INIT_PWD sourced from the initpwd Secret.
        let env = c["env"].as_array().unwrap();
        assert!(env.iter().any(|e| e["name"] == "INIT_PWD"
            && e["valueFrom"]["secretKeyRef"]["name"] == "sr1-initpwd-secret"
            && e["valueFrom"]["secretKeyRef"]["key"] == "password"));

        // The script is mounted (subPath) from the initpwd ConfigMap.
        let mounts = c["volumeMounts"].as_array().unwrap();
        assert!(mounts
            .iter()
            .any(|m| m["mountPath"] == "/opt/starrocks/fe_initpwd.sh"
                && m["subPath"] == "fe_initpwd.sh"));
        let vols = pod["volumes"].as_array().unwrap();
        assert!(vols
            .iter()
            .any(|v| v["configMap"]["name"] == "sr1-initpwd-shell"));
    }

    /// An update whose CR apply fails must leave the running cluster alone:
    /// only `provision` (a brand-new engine) may clean up after a failed apply.
    /// `update` tears down 11 objects total across its various toggle-off
    /// cleanups (Ranger/CN/TLS ConfigMaps, etc.) — a bare "the CR specifically
    /// wasn't deleted" check would miss a regression that reaches one of THOSE
    /// deletes despite the early `?` return on a failed apply, so this asserts
    /// `delete` was never called at all (M1).
    #[tokio::test]
    async fn update_failure_keeps_the_running_cluster() {
        use crate::kube_client::FakeKube;

        let spec = sample_sr_spec();
        let kube = Arc::new(FakeKube::default());
        *kube.fail_apply_kind.lock().unwrap() = Some("StarRocksCluster".to_string());
        let res = StarRocksProvisioner::new(kube.clone()).update(&spec).await;
        assert!(res.is_err(), "the injected CR apply failure must surface");
        let deleted = kube.deleted.lock().unwrap();
        assert!(
            deleted.is_empty(),
            "a failed update must not delete anything at all; deleted: {deleted:?}"
        );
    }

    // ── Task 6b: the rollout of ONE Ranger credential is observable ─────────

    /// The FE (which runs the plugin) and the Secret name the credential they
    /// were rendered with; the CN does not.
    #[test]
    fn the_fe_and_the_secret_name_the_rendered_credential() {
        use crate::rollout::RANGER_CREDENTIAL_ANNOTATION as ANN;
        let spec = bound_sr(Some(("engine-1", "Pw_secret")));
        let cr = starrocks_cluster_cr(&spec);
        assert_eq!(
            cr["spec"]["starRocksFeSpec"]["annotations"][ANN],
            TEST_CREDENTIAL_ID
        );
        assert!(cr["spec"]["starRocksCnSpec"]["annotations"]
            .get(ANN)
            .is_none());
        let secret = starrocks_ranger_secret(&spec).unwrap();
        assert_eq!(secret["metadata"]["annotations"][ANN], TEST_CREDENTIAL_ID);
        let unbound = starrocks_cluster_cr(&sample_sr_spec());
        assert!(unbound["spec"]["starRocksFeSpec"]["annotations"]
            .get(ANN)
            .is_none());
    }

    fn fe_labels() -> Value {
        json!({ "app.starrocks.ownerreference/name": "sr1-fe" })
    }

    /// The live cluster of a fully rolled StarRocks engine `sr1` on `cred`; its
    /// CN pod still carries an OLD annotation, which must not matter.
    fn rolled_sr(kube: &crate::kube_client::FakeKube, cred: &str) {
        use crate::rollout::tests::{ready_pod, rolled_statefulset, secret};
        kube.set_live(secret("sr1-ranger-secret", cred));
        kube.set_live(rolled_statefulset("sr1-fe", cred));
        *kube.live_pods.lock().unwrap() = vec![
            ready_pod("sr1-fe-0", fe_labels(), cred),
            ready_pod(
                "sr1-cn-0",
                json!({ "app.starrocks.ownerreference/name": "sr1-cn" }),
                "an-old-one",
            ),
        ];
    }

    #[tokio::test]
    async fn a_starrocks_rollout_is_observed_on_the_fe_only() {
        use crate::kube_client::FakeKube;
        let kube = Arc::new(FakeKube::default());
        let p = StarRocksProvisioner::new(kube.clone());
        rolled_sr(&kube, "c2");
        assert_eq!(
            p.ranger_rollout("egret-engines", "sr1", "c2")
                .await
                .unwrap(),
            RangerRollout::Complete
        );
        // An older rollout never answers for a newer credential.
        assert!(matches!(
            p.ranger_rollout("egret-engines", "sr1", "c3")
                .await
                .unwrap(),
            RangerRollout::Pending(_)
        ));
    }

    #[tokio::test]
    async fn a_stalled_fe_rollout_is_pending_and_a_failed_read_is_an_error() {
        use crate::kube_client::FakeKube;
        use crate::rollout::tests::ready_pod;
        let kube = Arc::new(FakeKube::default());
        let p = StarRocksProvisioner::new(kube.clone());
        rolled_sr(&kube, "c2");
        // The operator has updated the StatefulSet, but the old FE pod is
        // still the one running: two revisions.
        let mut sts = crate::rollout::tests::rolled_statefulset("sr1-fe", "c2");
        sts["status"]["currentRevision"] = json!("fe-old");
        sts["status"]["updatedReplicas"] = json!(0);
        kube.set_live(sts);
        *kube.live_pods.lock().unwrap() = vec![ready_pod("sr1-fe-0", fe_labels(), "c1")];
        assert!(matches!(
            p.ranger_rollout("egret-engines", "sr1", "c2")
                .await
                .unwrap(),
            RangerRollout::Pending(_)
        ));
        *kube.fail_reads.lock().unwrap() = Some("forbidden".into());
        assert!(p
            .ranger_rollout("egret-engines", "sr1", "c2")
            .await
            .is_err());
    }
}
