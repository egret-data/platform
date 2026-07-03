//! Kubernetes access seam. Kept behind a trait so tests can inject a fake and
//! kube-rs stays out of the templating + provisioning logic.

use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Minimal k8s surface egret needs. `apply` is server-side-apply of one resource
/// identified by (apiVersion, kind, namespace, name); `delete` removes it; the
/// pod read backs status. Keeps kube-rs out of the templating + tests.
#[async_trait::async_trait]
pub trait KubeClient: Send + Sync {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()>;
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
}

/// In-memory fake for unit tests: records applied/deleted objects.
#[derive(Default)]
pub struct FakeKube {
    pub applied: Mutex<BTreeMap<String, Value>>, // key = "kind/name"
    pub deleted: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl KubeClient for FakeKube {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()> {
        let key = format!(
            "{}/{}",
            obj["kind"].as_str().unwrap_or(""),
            obj["metadata"]["name"].as_str().unwrap_or("")
        );
        self.applied.lock().unwrap().insert(key, obj.clone());
        Ok(())
    }
    async fn delete(&self, _av: &str, kind: &str, _ns: &str, name: &str) -> anyhow::Result<()> {
        self.deleted.lock().unwrap().push(format!("{kind}/{name}"));
        Ok(())
    }
    async fn list_pods(
        &self,
        _ns: &str,
        _instance: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>> {
        Ok(vec![("demo-coordinator-0".into(), "Running".into(), true)])
    }
}

/// Real kube-rs backed client. Applies/deletes CRs as `DynamicObject`s (server-side
/// apply) and reads pods via the typed `Pod` API. Not unit-tested (needs a cluster);
/// the `FakeKube` tests are the coverage.
pub struct LiveKube {
    client: kube::Client,
}

impl LiveKube {
    /// Build from the in-cluster service-account (prod: egret running inside k8s).
    pub async fn in_cluster() -> anyhow::Result<Self> {
        let config = kube::Config::incluster()?;
        let client = kube::Client::try_from(config)?;
        Ok(Self { client })
    }

    /// Build from the local kubeconfig / inferred config (dev).
    pub async fn from_kubeconfig() -> anyhow::Result<Self> {
        let config = kube::Config::infer().await?;
        let client = kube::Client::try_from(config)?;
        Ok(Self { client })
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
        api.delete(name, &DeleteParams::default()).await?;
        Ok(())
    }

    async fn list_pods(
        &self,
        namespace: &str,
        instance: &str,
    ) -> anyhow::Result<Vec<(String, String, bool)>> {
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::{Api, ListParams};

        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let lp = ListParams::default().labels(&format!("app.kubernetes.io/instance={instance}"));
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
}
