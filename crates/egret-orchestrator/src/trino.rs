//! Trino (Stackable operator) CR templating + provisioner. Filled in later tasks.

use crate::spec::EngineSpec;
use serde_json::{json, Value};

/// Name of the `Secret` holding the Catalog Client credential for this engine's
/// engine (so two engines wiring the same warehouse never collide on the Secret).
fn creds_secret_name(spec: &EngineSpec) -> String {
    format!("{}-creds", spec.name)
}

/// Kubernetes `Secret` holding the Catalog Client credential (`client_id:client_secret`).
pub fn trino_secret(spec: &EngineSpec) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": creds_secret_name(spec),
            "namespace": spec.namespace,
            "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name }
        },
        "stringData": {
            "credential": format!("{}:{}", spec.wiring.client_id, spec.wiring.client_secret)
        }
    })
}

/// `TrinoCatalog` — generic `iceberg` connector wired to egret's REST catalog.
pub fn trino_catalog_cr(spec: &EngineSpec) -> Value {
    let w = &spec.wiring;
    let mut props = serde_json::Map::new();
    let mut v = |k: &str, val: String| {
        props.insert(k.into(), json!({ "value": val }));
    };
    v("iceberg.catalog.type", "rest".into());
    v("iceberg.rest-catalog.uri", w.catalog_uri.clone());
    v("iceberg.rest-catalog.warehouse", w.warehouse.clone());
    v("iceberg.rest-catalog.security", "OAUTH2".into());
    v(
        "iceberg.rest-catalog.oauth2.server-uri",
        w.token_uri.clone(),
    );
    v(
        "iceberg.rest-catalog.vended-credentials-enabled",
        "true".into(),
    );
    v("fs.native-s3.enabled", "true".into());
    v("s3.endpoint", w.s3_endpoint.clone());
    v("s3.region", "us-east-1".into());
    v("s3.path-style-access", "true".into());
    // OAuth credential from the Secret (never inline in the CR).
    props.insert(
        "iceberg.rest-catalog.oauth2.credential".into(),
        json!({ "valueFromSecret": { "name": creds_secret_name(spec), "key": "credential" } }),
    );
    for p in &spec.extra_props {
        props.insert(p.key.clone(), json!({ "value": p.value }));
    }
    json!({
        "apiVersion": "trino.stackable.tech/v1alpha1",
        "kind": "TrinoCatalog",
        "metadata": {
            "name": format!("{}-egret", spec.name),
            "namespace": spec.namespace,
            "labels": { "trino": spec.name, "app.kubernetes.io/managed-by": "egret" }
        },
        "spec": { "connector": { "generic": { "connectorName": "iceberg", "properties": props } } }
    })
}

/// `TrinoCluster` — coordinator + workers, resources, and the catalog label selector.
pub fn trino_cluster_cr(spec: &EngineSpec) -> Value {
    let res = json!({
        "cpu": { "min": spec.resources.cpu_min, "max": spec.resources.cpu_max },
        "memory": { "limit": spec.resources.memory }
    });
    json!({
        "apiVersion": "trino.stackable.tech/v1alpha1",
        "kind": "TrinoCluster",
        "metadata": {
            "name": spec.name,
            "namespace": spec.namespace,
            "labels": { "app.kubernetes.io/managed-by": "egret" }
        },
        "spec": {
            "image": { "productVersion": spec.image_version },
            "clusterConfig": { "catalogLabelSelector": { "matchLabels": { "trino": spec.name } } },
            "coordinators": { "roleGroups": { "default": {
                "replicas": spec.coordinator_replicas,
                "config": { "resources": res.clone() } } } },
            "workers": { "roleGroups": { "default": {
                "replicas": spec.worker_replicas,
                "config": { "resources": res } } } }
        }
    })
}

use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{EngineStatus, PodStatus};
use std::sync::Arc;

/// Provisions a Trino engine via the Stackable operator: applies a `Secret`, a
/// `TrinoCatalog`, and a `TrinoCluster` (with rollback on partial failure).
pub struct TrinoProvisioner {
    kube: Arc<dyn KubeClient>,
}

impl TrinoProvisioner {
    pub fn new(kube: Arc<dyn KubeClient>) -> Self {
        Self { kube }
    }
}

#[async_trait::async_trait]
impl EngineProvisioner for TrinoProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Order: Secret -> Catalog -> Cluster. On any failure, best-effort delete
        // what we applied so a partial cluster never lingers.
        let secret = trino_secret(spec);
        let catalog = trino_catalog_cr(spec);
        let cluster = trino_cluster_cr(spec);
        if let Err(e) = async {
            self.kube.apply(&secret).await?;
            self.kube.apply(&catalog).await?;
            self.kube.apply(&cluster).await?;
            anyhow::Ok(())
        }
        .await
        {
            let _ = self.delete(&spec.namespace, &spec.name).await;
            return Err(e);
        }
        Ok(())
    }

    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
        let pods = self.kube.list_pods(namespace, name).await?;
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

    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Server-side apply is idempotent: re-applying is the update path.
        self.kube.apply(&trino_secret(spec)).await?;
        self.kube.apply(&trino_catalog_cr(spec)).await?;
        self.kube.apply(&trino_cluster_cr(spec)).await?;
        Ok(())
    }

    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        let v = "trino.stackable.tech/v1alpha1";
        let _ = self.kube.delete(v, "TrinoCluster", namespace, name).await;
        let _ = self
            .kube
            .delete(v, "TrinoCatalog", namespace, &format!("{name}-egret"))
            .await;
        let _ = self
            .kube
            .delete("v1", "Secret", namespace, &format!("{name}-creds"))
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;

    fn sample_spec() -> EngineSpec {
        EngineSpec {
            name: "demo".into(),
            namespace: "egret-engines".into(),
            image_version: "481".into(),
            coordinator_replicas: 1,
            worker_replicas: 2,
            resources: Resources {
                cpu_min: "500m".into(),
                cpu_max: "2".into(),
                memory: "4Gi".into(),
            },
            wiring: CatalogWiring {
                catalog_uri: "http://egret:8080/catalog".into(),
                token_uri: "http://egret:8080/oauth/token".into(),
                warehouse: "demo-catalog".into(),
                s3_endpoint: "http://minio:9000".into(),
                client_id: "egret_abc".into(),
                client_secret: "sekret".into(),
            },
            extra_props: vec![],
        }
    }

    #[test]
    fn catalog_cr_uses_generic_iceberg_rest() {
        let s = sample_spec();
        let cr = trino_catalog_cr(&s);
        assert_eq!(cr["apiVersion"], "trino.stackable.tech/v1alpha1");
        assert_eq!(cr["kind"], "TrinoCatalog");
        assert_eq!(cr["metadata"]["labels"]["trino"], "demo");
        let props = &cr["spec"]["connector"]["generic"]["properties"];
        assert_eq!(
            cr["spec"]["connector"]["generic"]["connectorName"],
            "iceberg"
        );
        assert_eq!(props["iceberg.catalog.type"]["value"], "rest");
        assert_eq!(
            props["iceberg.rest-catalog.uri"]["value"],
            "http://egret:8080/catalog"
        );
        assert_eq!(
            props["iceberg.rest-catalog.warehouse"]["value"],
            "demo-catalog"
        );
        // credential is injected from the Secret, not inline
        assert_eq!(
            props["iceberg.rest-catalog.oauth2.credential"]["valueFromSecret"]["name"],
            "demo-creds"
        );
    }

    #[test]
    fn secret_holds_client_credential() {
        let cr = trino_secret(&sample_spec());
        assert_eq!(cr["kind"], "Secret");
        assert_eq!(cr["metadata"]["name"], "demo-creds");
        assert_eq!(cr["stringData"]["credential"], "egret_abc:sekret");
    }

    #[test]
    fn cluster_cr_sets_replicas_and_label_selector() {
        let cr = trino_cluster_cr(&sample_spec());
        assert_eq!(cr["kind"], "TrinoCluster");
        assert_eq!(
            cr["spec"]["clusterConfig"]["catalogLabelSelector"]["matchLabels"]["trino"],
            "demo"
        );
        assert_eq!(
            cr["spec"]["coordinators"]["roleGroups"]["default"]["replicas"],
            1
        );
        assert_eq!(
            cr["spec"]["workers"]["roleGroups"]["default"]["replicas"],
            2
        );
        assert_eq!(cr["spec"]["image"]["productVersion"], "481");
    }

    #[tokio::test]
    async fn provision_applies_secret_catalog_cluster() {
        use crate::kube_client::FakeKube;
        use std::sync::Arc;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        p.provision(&sample_spec()).await.unwrap();
        let applied = kube.applied.lock().unwrap();
        assert!(applied.contains_key("Secret/demo-creds"));
        assert!(applied.contains_key("TrinoCatalog/demo-egret"));
        assert!(applied.contains_key("TrinoCluster/demo"));
    }

    #[tokio::test]
    async fn status_maps_pods_to_phase() {
        use crate::kube_client::FakeKube;
        use std::sync::Arc;
        let p = TrinoProvisioner::new(Arc::new(FakeKube::default()));
        let st = p.status("egret-engines", "demo").await.unwrap();
        assert_eq!(st.phase, "running"); // fake returns a ready pod
        assert_eq!(st.pods.len(), 1);
    }
}
