//! Kubernetes access seam. Kept behind a trait so tests can inject a fake and
//! kube-rs stays out of the templating + provisioning logic.

use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Mutex;

/// A boxed stream of raw log-byte chunks, as read off a following (`follow:
/// true`) pod-log request. Returned by [`KubeClient::pod_logs_stream`] and
/// pumped onto a browser WebSocket by the control-plane's log-follow route.
pub type PodLogStream = Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>>;

/// Minimal k8s surface egret needs. `apply` is server-side-apply of one resource
/// identified by (apiVersion, kind, namespace, name); `delete` removes it; the
/// pod read backs status. Keeps kube-rs out of the templating + tests.
#[async_trait::async_trait]
pub trait KubeClient: Send + Sync {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()>;
    /// JSON merge-patch (RFC 7386) an existing object identified by
    /// (apiVersion, kind, namespace, name). Only the fields present in `patch`
    /// are changed; unlike [`apply`](Self::apply) this needs no full object and
    /// won't clear omitted fields. Used to stamp a restart annotation onto a CR
    /// the operator owns (e.g. rolling StarRocks FE/CN pods) without re-sending
    /// — and risking clobbering — the whole spec.
    async fn merge_patch(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
        patch: &Value,
    ) -> anyhow::Result<()>;
    /// Delete one object. An object that is already gone (HTTP 404) is `Ok`:
    /// deletes are retried after partial failures, and "already deleted" is the
    /// outcome the caller asked for. Every other failure is an `Err`.
    async fn delete(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<()>;
    /// Pods labeled `app.kubernetes.io/instance=<instance>` in the namespace.
    /// Returns `(name, phase, ready)` per pod.
    async fn list_pods(
        &self,
        namespace: &str,
        instance: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>>;

    /// Pods matching a raw label selector in the namespace. Returns
    /// `(name, phase, ready)` per pod. Used for engines (e.g. StarRocks) whose
    /// operator stamps its own pod labels rather than `app.kubernetes.io/instance`.
    async fn list_pods_selector(
        &self,
        namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>>;

    /// Read one whole object (metadata, spec and status) as JSON; `Ok(None)`
    /// when it does not exist (HTTP 404). Used to observe a rollout from a
    /// Deployment's or StatefulSet's status.
    async fn get_object(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<Option<Value>>;

    /// Read only an object's metadata, as `{"metadata": {...}}`; `Ok(None)`
    /// when it does not exist. For a Secret this never transfers its data.
    async fn get_object_metadata(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<Option<Value>>;

    /// The names of the objects of one kind matching a label selector in the
    /// namespace (metadata only: listing Secrets never transfers their data).
    async fn list_object_names(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<String>>;

    /// The whole Pod objects matching a label selector in the namespace — the
    /// pod-level half of a rollout observation (annotations, deletion
    /// timestamp, phase and conditions).
    async fn list_pod_objects(
        &self,
        namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<Value>>;

    /// Issue a raw HTTP request to a Trino coordinator `Service` (port 8080) and
    /// return `(status, body)`. In-cluster this hits the Service DNS directly; in
    /// dev it goes through the kube API-server proxy subresource. Non-2xx statuses
    /// are returned as-is (the caller decides how to map them).
    async fn coordinator_request(
        &self,
        namespace: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)>;

    /// Tail a pod's (optionally previous-container's) logs, bounded to
    /// `tail_lines`. `container` selects a specific container when the pod has
    /// more than one; `None` picks the pod's sole container.
    async fn pod_logs(
        &self,
        namespace: &str,
        pod: &str,
        container: Option<&str>,
        tail_lines: i64,
        previous: bool,
    ) -> anyhow::Result<String>;

    /// Open a live-following stream (`follow: true`) of a pod's log bytes —
    /// the `kubectl logs -f` equivalent of [`pod_logs`](Self::pod_logs)'s
    /// one-shot tail. `tail_lines` still bounds the backlog replayed before
    /// new lines start arriving. The caller (the control-plane's
    /// `pod_logs_follow` WS route) pumps chunks onto a browser WebSocket as
    /// they arrive; this is a ONE-directional stream (pod -> browser).
    async fn pod_logs_stream(
        &self,
        namespace: &str,
        pod: &str,
        tail_lines: i64,
    ) -> anyhow::Result<PodLogStream>;

    /// Force a rollout of a Deployment WITHOUT changing its rendered config:
    /// patches the pod-template's `egret/restarted-at` annotation to now, which
    /// changes the pod template and triggers a roll (same effect as
    /// `kubectl rollout restart deployment/<name>`).
    async fn restart_deployment(&self, namespace: &str, name: &str) -> anyhow::Result<()>;

    /// Exec `command` inside `pod` (kube's "exec" subresource — a WebSocket
    /// protocol, hence the `ws` kube feature) and return the live
    /// `AttachedProcess`: stdin/stdout streams plus a terminal-resize sender,
    /// for the caller to bridge onto e.g. a browser WebSocket. Requested with
    /// `tty(true)` + `stderr(false)` (stderr must be off when tty is on — the
    /// pty merges stderr into stdout). Not implemented by `FakeKube`: exec is
    /// smoke-tested against a real cluster, not unit-tested.
    async fn exec(
        &self,
        namespace: &str,
        pod: &str,
        command: &[String],
    ) -> anyhow::Result<kube::api::AttachedProcess>;
}

/// In-memory fake for unit tests: records applied/deleted objects.
pub struct FakeKube {
    pub applied: Mutex<BTreeMap<String, Value>>, // key = "kind/name"
    pub deleted: Mutex<Vec<String>>,
    /// `(method, path)` per `coordinator_request` call.
    pub coordinator_calls: Mutex<Vec<(String, String)>>,
    /// Canned `(status, body)` returned by `coordinator_request`.
    pub coordinator_response: Mutex<(u16, Vec<u8>)>,
    /// `(namespace, name)` per `restart_deployment` call.
    pub restarted: Mutex<Vec<(String, String)>>,
    /// `("kind/name", patch)` per `merge_patch` call.
    pub patched: Mutex<Vec<(String, Value)>>,
    /// `"kind/name"` → error message. A `delete` of a listed object fails with
    /// that message and is NOT recorded in `deleted`. Set with [`FakeKube::fail_delete`].
    pub delete_failures: Mutex<BTreeMap<String, String>>,
    /// When `Some(kind)`, `apply` of an object of that `kind` fails (and is not
    /// recorded) — stands in for the API server rejecting a manifest.
    pub fail_apply_kind: Mutex<Option<String>>,
    /// The cluster's LIVE objects as `get_object` reads them, keyed
    /// `"kind/name"` (status included). Separate from `applied`: what egret
    /// sent is not what the controllers have made of it. Set with
    /// [`FakeKube::set_live`].
    pub live: Mutex<BTreeMap<String, Value>>,
    /// The Pod objects `list_pod_objects` filters by label selector.
    pub live_pods: Mutex<Vec<Value>>,
    /// When `Some`, every `get_object*` / `list_pod_objects` read fails with it.
    pub fail_reads: Mutex<Option<String>>,
}

impl Default for FakeKube {
    fn default() -> Self {
        Self {
            applied: Mutex::new(BTreeMap::new()),
            deleted: Mutex::new(Vec::new()),
            coordinator_calls: Mutex::new(Vec::new()),
            coordinator_response: Mutex::new((200, Vec::new())),
            restarted: Mutex::new(Vec::new()),
            patched: Mutex::new(Vec::new()),
            delete_failures: Mutex::new(BTreeMap::new()),
            fail_apply_kind: Mutex::new(None),
            live: Mutex::new(BTreeMap::new()),
            live_pods: Mutex::new(Vec::new()),
            fail_reads: Mutex::new(None),
        }
    }
}

impl FakeKube {
    /// Set the canned response returned by `coordinator_request`.
    pub fn set_coordinator_response(&self, status: u16, body: Vec<u8>) {
        *self.coordinator_response.lock().unwrap() = (status, body);
    }

    /// Put `obj` into the live cluster state `get_object` reads.
    pub fn set_live(&self, obj: Value) {
        let key = format!(
            "{}/{}",
            obj["kind"].as_str().unwrap_or(""),
            obj["metadata"]["name"].as_str().unwrap_or("")
        );
        self.live.lock().unwrap().insert(key, obj);
    }

    fn read_failure(&self) -> anyhow::Result<()> {
        match self.fail_reads.lock().unwrap().as_deref() {
            Some(message) => anyhow::bail!("{message}"),
            None => Ok(()),
        }
    }

    /// Make every later `delete` of `kind/name` fail with `message`.
    pub fn fail_delete(&self, kind: &str, name: &str, message: &str) {
        self.delete_failures
            .lock()
            .unwrap()
            .insert(format!("{kind}/{name}"), message.to_string());
    }
}

#[async_trait::async_trait]
impl KubeClient for FakeKube {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()> {
        if let Some(kind) = self.fail_apply_kind.lock().unwrap().as_deref() {
            if obj["kind"].as_str() == Some(kind) {
                anyhow::bail!("injected apply failure for {kind}");
            }
        }
        let key = format!(
            "{}/{}",
            obj["kind"].as_str().unwrap_or(""),
            obj["metadata"]["name"].as_str().unwrap_or("")
        );
        self.applied.lock().unwrap().insert(key, obj.clone());
        Ok(())
    }
    async fn merge_patch(
        &self,
        _av: &str,
        kind: &str,
        _ns: &str,
        name: &str,
        patch: &Value,
    ) -> anyhow::Result<()> {
        self.patched
            .lock()
            .unwrap()
            .push((format!("{kind}/{name}"), patch.clone()));
        Ok(())
    }
    async fn delete(&self, _av: &str, kind: &str, _ns: &str, name: &str) -> anyhow::Result<()> {
        let key = format!("{kind}/{name}");
        if let Some(message) = self.delete_failures.lock().unwrap().get(&key) {
            anyhow::bail!("{message}");
        }
        self.deleted.lock().unwrap().push(key);
        Ok(())
    }
    async fn list_pods(
        &self,
        _ns: &str,
        _instance: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>> {
        Ok(vec![("demo-coordinator-0".into(), "Running".into(), true)])
    }
    async fn list_pods_selector(
        &self,
        _ns: &str,
        _label_selector: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>> {
        Ok(vec![("demo-fe-0".into(), "Running".into(), true)])
    }

    async fn get_object(
        &self,
        _api_version: &str,
        kind: &str,
        _namespace: &str,
        name: &str,
    ) -> anyhow::Result<Option<Value>> {
        self.read_failure()?;
        Ok(self
            .live
            .lock()
            .unwrap()
            .get(&format!("{kind}/{name}"))
            .cloned())
    }

    async fn get_object_metadata(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<Option<Value>> {
        Ok(self
            .get_object(api_version, kind, namespace, name)
            .await?
            .map(|o| serde_json::json!({ "metadata": o["metadata"] })))
    }

    async fn list_object_names(
        &self,
        _api_version: &str,
        kind: &str,
        _namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<String>> {
        self.read_failure()?;
        let wanted = parse_selector(label_selector);
        let deleted = self.deleted.lock().unwrap().clone();
        Ok(self
            .applied
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, o)| {
                o["kind"].as_str() == Some(kind)
                    && !deleted.contains(key)
                    && matches_selector(o, &wanted)
            })
            .map(|(_, o)| o["metadata"]["name"].as_str().unwrap_or("").to_string())
            .collect())
    }

    async fn list_pod_objects(
        &self,
        _namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<Value>> {
        self.read_failure()?;
        let wanted = parse_selector(label_selector);
        Ok(self
            .live_pods
            .lock()
            .unwrap()
            .iter()
            .filter(|p| matches_selector(p, &wanted))
            .cloned()
            .collect())
    }

    async fn coordinator_request(
        &self,
        _namespace: &str,
        _service: &str,
        method: &str,
        path: &str,
        _headers: &[(String, String)],
        _body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        self.coordinator_calls
            .lock()
            .unwrap()
            .push((method.to_string(), path.to_string()));
        Ok(self.coordinator_response.lock().unwrap().clone())
    }

    async fn pod_logs(
        &self,
        _namespace: &str,
        pod: &str,
        _container: Option<&str>,
        _tail_lines: i64,
        _previous: bool,
    ) -> anyhow::Result<String> {
        Ok(format!("logs for {pod} (fake)\n"))
    }

    async fn pod_logs_stream(
        &self,
        _namespace: &str,
        _pod: &str,
        _tail_lines: i64,
    ) -> anyhow::Result<PodLogStream> {
        Ok(Box::pin(futures_util::stream::once(async {
            Ok(Bytes::from_static(b"fake log line\n"))
        })))
    }

    async fn restart_deployment(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        self.restarted
            .lock()
            .unwrap()
            .push((namespace.to_string(), name.to_string()));
        Ok(())
    }

    async fn exec(
        &self,
        _namespace: &str,
        _pod: &str,
        _command: &[String],
    ) -> anyhow::Result<kube::api::AttachedProcess> {
        anyhow::bail!("exec not supported in fake kube")
    }
}

/// Equality selectors only (`k=v[,k=v]`), which is all egret uses.
fn parse_selector(selector: &str) -> Vec<(&str, &str)> {
    selector
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .collect()
}

fn matches_selector(obj: &Value, wanted: &[(&str, &str)]) -> bool {
    wanted
        .iter()
        .all(|(k, v)| obj["metadata"]["labels"][*k].as_str() == Some(*v))
}

/// True for the API server's "not found" answer. A missing object and a resource
/// type the cluster does not serve (e.g. KEDA's `ScaledObject` with no KEDA
/// installed) both come back as 404, and both mean there is nothing to delete.
pub fn is_not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(resp) if resp.code == 404)
}

/// Map a kube delete result the way `LiveKube::delete` treats it: success or
/// "already gone" (`is_not_found`) both count as deleted; anything else
/// propagates. Pulled out as a pure function so this tolerance is pinned
/// directly — `LiveKube::delete` itself needs a real cluster and isn't
/// unit-tested (see its doc comment).
fn tolerate_not_found(result: Result<(), kube::Error>) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Delete every `(apiVersion, kind, name)` in `namespace`. All of them are
/// attempted even after a failure, so a retry has as little left as possible;
/// the error names every object that failed and why.
pub async fn delete_objects(
    kube: &dyn KubeClient,
    namespace: &str,
    objects: &[(&str, &str, String)],
) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    for (api_version, kind, name) in objects {
        if let Err(e) = kube.delete(api_version, kind, namespace, name).await {
            failures.push(format!("{kind}/{name}: {e:#}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("failed to delete {}", failures.join("; "))
    }
}

/// Real kube-rs backed client. Applies/deletes CRs as `DynamicObject`s (server-side
/// apply) and reads pods via the typed `Pod` API. Not unit-tested (needs a cluster);
/// the `FakeKube` tests are the coverage.
pub struct LiveKube {
    client: kube::Client,
    /// True when egret runs inside the cluster (reach coordinators via Service DNS);
    /// false in dev (reach them via the kube API-server proxy subresource).
    in_cluster: bool,
}

impl LiveKube {
    /// Build from the in-cluster service-account (prod: egret running inside k8s).
    pub async fn in_cluster() -> anyhow::Result<Self> {
        let config = kube::Config::incluster()?;
        let client = kube::Client::try_from(config)?;
        Ok(Self {
            client,
            in_cluster: true,
        })
    }

    /// Build from the local kubeconfig / inferred config (dev).
    pub async fn from_kubeconfig() -> anyhow::Result<Self> {
        let config = kube::Config::infer().await?;
        let client = kube::Client::try_from(config)?;
        Ok(Self {
            client,
            in_cluster: false,
        })
    }

    /// Derive an `ApiResource` from an object's `apiVersion` + `kind`.
    /// `apiVersion` is `group/version` (e.g. `trino.stackable.tech/v1alpha1`) or a
    /// bare core version (e.g. `v1`, group empty).
    fn api_resource(api_version: &str, kind: &str) -> kube::core::ApiResource {
        let (group, version) = match api_version.split_once('/') {
            Some((g, v)) => (g.to_string(), v.to_string()),
            None => (String::new(), api_version.to_string()),
        };
        let gvk = kube::core::GroupVersionKind::gvk(&group, &version, kind);
        kube::core::ApiResource::from_gvk(&gvk)
    }
}

#[async_trait::async_trait]
impl KubeClient for LiveKube {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()> {
        use kube::api::{Api, Patch, PatchParams};
        use kube::core::DynamicObject;

        let api_version = obj["apiVersion"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("object missing apiVersion"))?;
        let kind = obj["kind"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("object missing kind"))?;
        let namespace = obj["metadata"]["namespace"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("object missing metadata.namespace"))?;
        let name = obj["metadata"]["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("object missing metadata.name"))?;

        let ar = Self::api_resource(api_version, kind);
        let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &ar);
        let pp = PatchParams::apply("egret").force();
        api.patch(name, &pp, &Patch::Apply(obj)).await?;
        Ok(())
    }

    async fn merge_patch(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
        patch: &Value,
    ) -> anyhow::Result<()> {
        use kube::api::{Api, Patch, PatchParams};
        use kube::core::DynamicObject;

        let ar = Self::api_resource(api_version, kind);
        let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &ar);
        // RFC-7386 merge patch: sets only the keys present in `patch`, leaving
        // every other (operator-managed) CR field intact — no `force`/apply
        // field-ownership dance needed.
        api.patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await?;
        Ok(())
    }

    async fn delete(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        use kube::api::{Api, DeleteParams};
        use kube::core::DynamicObject;

        let ar = Self::api_resource(api_version, kind);
        let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &ar);
        tolerate_not_found(api.delete(name, &DeleteParams::default()).await.map(|_| ()))
    }

    async fn list_pods(
        &self,
        namespace: &str,
        instance: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>> {
        self.list_pods_selector(namespace, &format!("app.kubernetes.io/instance={instance}"))
            .await
    }

    async fn list_pods_selector(
        &self,
        namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>> {
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::{Api, ListParams};

        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let lp = ListParams::default().labels(label_selector);
        let pods = api.list(&lp).await?;
        let out = pods
            .into_iter()
            .map(|p| {
                let name = p.metadata.name.clone().unwrap_or_default();
                let phase = p
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_default();
                let ready = p
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .map(|conds| {
                        conds
                            .iter()
                            .any(|c| c.type_ == "Ready" && c.status == "True")
                    })
                    .unwrap_or(false);
                (name, phase, ready)
            })
            .collect();
        Ok(out)
    }

    async fn get_object(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<Option<Value>> {
        use kube::api::Api;
        use kube::core::DynamicObject;

        let ar = Self::api_resource(api_version, kind);
        let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &ar);
        match api.get_opt(name).await? {
            Some(obj) => Ok(Some(serde_json::to_value(obj)?)),
            None => Ok(None),
        }
    }

    async fn get_object_metadata(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> anyhow::Result<Option<Value>> {
        use kube::api::Api;
        use kube::core::DynamicObject;

        let ar = Self::api_resource(api_version, kind);
        let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &ar);
        match api.get_metadata_opt(name).await? {
            Some(meta) => Ok(Some(serde_json::json!({ "metadata": meta.metadata }))),
            None => Ok(None),
        }
    }

    async fn list_object_names(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<String>> {
        use kube::api::{Api, ListParams};
        use kube::core::DynamicObject;

        let ar = Self::api_resource(api_version, kind);
        let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &ar);
        let list = api
            .list_metadata(&ListParams::default().labels(label_selector))
            .await?;
        Ok(list
            .items
            .into_iter()
            .filter_map(|o| o.metadata.name)
            .collect())
    }

    async fn list_pod_objects(
        &self,
        namespace: &str,
        label_selector: &str,
    ) -> anyhow::Result<Vec<Value>> {
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::{Api, ListParams};

        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let pods = api
            .list(&ListParams::default().labels(label_selector))
            .await?;
        pods.into_iter()
            .map(|p| serde_json::to_value(p).map_err(anyhow::Error::from))
            .collect()
    }

    async fn coordinator_request(
        &self,
        namespace: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        if self.in_cluster {
            // Prod: reach the coordinator Service directly over cluster DNS.
            let url = format!("http://{service}.{namespace}.svc:8080{path}");
            let mut rb = reqwest::Client::new().request(method.parse()?, &url);
            for (k, v) in headers {
                rb = rb.header(k, v);
            }
            if let Some(b) = body {
                rb = rb.body(b);
            }
            let resp = rb.send().await?;
            Ok((resp.status().as_u16(), resp.bytes().await?.to_vec()))
        } else {
            // Dev: egret on the host proxies through the kube API server.
            use kube::client::Body;
            let uri = format!("/api/v1/namespaces/{namespace}/services/{service}:8080/proxy{path}");
            let mut req = http::Request::builder().method(method).uri(uri);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let req = req.body(Body::from(body.unwrap_or_default()))?;
            let resp = self.client.send(req).await?;
            let status = resp.status().as_u16();
            let bytes = resp.into_body().collect_bytes().await?.to_vec();
            Ok((status, bytes))
        }
    }

    async fn pod_logs(
        &self,
        namespace: &str,
        pod: &str,
        container: Option<&str>,
        tail_lines: i64,
        previous: bool,
    ) -> anyhow::Result<String> {
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::{Api, LogParams};

        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let lp = LogParams {
            container: container.map(str::to_string),
            tail_lines: Some(tail_lines),
            previous,
            ..Default::default()
        };
        let logs = api.logs(pod, &lp).await?;
        Ok(logs)
    }

    async fn pod_logs_stream(
        &self,
        namespace: &str,
        pod: &str,
        tail_lines: i64,
    ) -> anyhow::Result<PodLogStream> {
        use futures_util::StreamExt;
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::{Api, LogParams};
        use tokio_util::compat::FuturesAsyncReadCompatExt;
        use tokio_util::io::ReaderStream;

        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let lp = LogParams {
            follow: true,
            tail_lines: Some(tail_lines),
            ..Default::default()
        };
        // `log_stream` returns `impl futures::AsyncBufRead` (the `futures`
        // crate's trait, not tokio's); `.compat()` bridges it to
        // `tokio::io::AsyncRead` so `ReaderStream` can turn it into a
        // `Stream<Item = io::Result<Bytes>>`.
        let reader = api.log_stream(pod, &lp).await?.compat();
        let stream = ReaderStream::new(reader).map(|r| r.map_err(anyhow::Error::from));
        Ok(Box::pin(stream))
    }

    async fn restart_deployment(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        use k8s_openapi::api::apps::v1::Deployment;
        use kube::api::{Api, Patch, PatchParams};

        let api: Api<Deployment> = Api::namespaced(self.client.clone(), namespace);
        let ts = chrono::Utc::now().to_rfc3339();
        // A JSON merge patch of just the pod-template annotation is exactly what
        // `kubectl rollout restart` does: it changes the pod template (without
        // touching config) so the Deployment controller rolls the pods.
        let patch = serde_json::json!({
            "spec": {
                "template": {
                    "metadata": {
                        "annotations": { "egret/restarted-at": ts }
                    }
                }
            }
        });
        api.patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?;
        Ok(())
    }

    async fn exec(
        &self,
        namespace: &str,
        pod: &str,
        command: &[String],
    ) -> anyhow::Result<kube::api::AttachedProcess> {
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::{Api, AttachParams};

        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        // stderr must be off when tty is on (kube-rs would otherwise reject the
        // request): the pty already merges stderr into stdout.
        let ap = AttachParams::default()
            .stdin(true)
            .stdout(true)
            .stderr(false)
            .tty(true);
        let attached = api.exec(pod, command.to_vec(), &ap).await?;
        Ok(attached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn fake_kube_pod_logs_stream_yields_canned_line() {
        let k = FakeKube::default();
        let mut stream = k
            .pod_logs_stream("egret-engines", "demo-coordinator-0", 1000)
            .await
            .unwrap();
        let chunk = stream.next().await.unwrap().unwrap();
        assert_eq!(chunk, Bytes::from_static(b"fake log line\n"));
        assert!(stream.next().await.is_none(), "fake stream is single-shot");
    }

    #[tokio::test]
    async fn fake_kube_coordinator_request_records() {
        let k = FakeKube::default();
        k.set_coordinator_response(200, br#"[{"queryId":"x"}]"#.to_vec());
        let (code, body) = k
            .coordinator_request(
                "egret-engines",
                "demo-trino",
                "GET",
                "/v1/query",
                &[("X-Trino-User".into(), "egret-monitor".into())],
                None,
            )
            .await
            .unwrap();
        assert_eq!(code, 200);
        assert!(String::from_utf8_lossy(&body).contains("queryId"));
        assert_eq!(
            k.coordinator_calls.lock().unwrap()[0],
            ("GET".to_string(), "/v1/query".to_string())
        );
    }

    /// A restart rolls BOTH the `{name}-coordinator` and `{name}-worker`
    /// Deployments (the naming convention `TrinoProvisioner`/`trino.rs` use),
    /// each recorded as its own `restart_deployment` call.
    #[tokio::test]
    async fn fake_kube_restart_deployment_records_coordinator_and_worker() {
        let k = FakeKube::default();
        k.restart_deployment("egret-engines", "demo-coordinator")
            .await
            .unwrap();
        k.restart_deployment("egret-engines", "demo-worker")
            .await
            .unwrap();
        let calls = k.restarted.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                ("egret-engines".to_string(), "demo-coordinator".to_string()),
                ("egret-engines".to_string(), "demo-worker".to_string()),
            ]
        );
    }

    fn api_error(code: u16, reason: &str) -> kube::Error {
        kube::Error::Api(kube::core::ErrorResponse {
            status: "Failure".into(),
            message: format!("{reason} (test)"),
            reason: reason.into(),
            code,
        })
    }

    /// A missing object and a resource type the cluster does not serve both come
    /// back as 404; nothing else may be read as "already deleted".
    #[test]
    fn only_a_404_api_error_is_not_found() {
        assert!(is_not_found(&api_error(404, "NotFound")));
        assert!(!is_not_found(&api_error(403, "Forbidden")));
        assert!(!is_not_found(&api_error(500, "InternalError")));
    }

    /// Pins `LiveKube::delete`'s actual tolerance policy (via the pure helper
    /// it now calls): a 404 is swallowed as already-deleted, everything else
    /// propagates. Before this helper existed, deleting the whole
    /// `Err(e) if is_not_found(&e) => Ok(())` arm in `LiveKube::delete` kept
    /// the suite green because nothing called that code path directly.
    #[test]
    fn tolerate_not_found_swallows_404_and_propagates_other_errors() {
        assert!(tolerate_not_found(Ok(())).is_ok());
        assert!(tolerate_not_found(Err(api_error(404, "NotFound"))).is_ok());
        assert!(tolerate_not_found(Err(api_error(403, "Forbidden"))).is_err());
        assert!(tolerate_not_found(Err(api_error(500, "InternalError"))).is_err());
    }

    #[tokio::test]
    async fn delete_objects_attempts_every_object_and_names_each_failure() {
        let k = FakeKube::default();
        k.fail_delete("Deployment", "demo-coordinator", "forbidden by RBAC");
        k.fail_delete("Service", "demo-trino", "connection refused");
        let objects = [
            ("apps/v1", "Deployment", "demo-coordinator".to_string()),
            ("apps/v1", "Deployment", "demo-worker".to_string()),
            ("v1", "Service", "demo-trino".to_string()),
            ("v1", "ConfigMap", "demo-config".to_string()),
        ];
        let err = delete_objects(&k, "egret-engines", &objects)
            .await
            .expect_err("two deletes failed");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Deployment/demo-coordinator: forbidden by RBAC"),
            "{msg}"
        );
        assert!(
            msg.contains("Service/demo-trino: connection refused"),
            "{msg}"
        );
        assert_eq!(
            *k.deleted.lock().unwrap(),
            vec![
                "Deployment/demo-worker".to_string(),
                "ConfigMap/demo-config".to_string()
            ],
            "the objects after a failure are still attempted"
        );
    }
}
