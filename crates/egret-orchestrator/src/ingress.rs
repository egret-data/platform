//! Shared ingress helpers reused by the Trino and StarRocks provisioners.
use egret_engine_mgmt_api::Ingress;
use serde_json::{json, Value};

/// Project a resolved ingress TLS cert/key into a real k8s
/// `type: kubernetes.io/tls` Secret named `{name}-ingress-tls`. Returns `None`
/// unless the ingress is enabled and carries resolved cert+key PEMs (populated
/// at provision from a typed `kubernetes.io/tls` secret ref).
pub fn ingress_tls_secret(name: &str, namespace: &str, ing: &Ingress) -> Option<Value> {
    if !ing.enabled {
        return None;
    }
    let (crt, key) = (ing.tls_cert_pem.as_ref()?, ing.tls_key_pem.as_ref()?);
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
        "metadata": { "name": format!("{name}-ingress-tls"), "namespace": namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": name } },
        "stringData": { "tls.crt": crt, "tls.key": key }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_typed_tls_secret_with_pems() {
        let ing = Ingress {
            enabled: true,
            class_name: Some("nginx".into()),
            host: "h".into(),
            tls_secret: Some("e-ingress-tls".into()),
            tls_cert_secret_ref: None,
            tls_cert_pem: Some("CERT".into()),
            tls_key_pem: Some("KEY".into()),
            annotations: Default::default(),
        };
        let s = ingress_tls_secret("e", "ns", &ing).unwrap();
        assert_eq!(s["type"], "kubernetes.io/tls");
        assert_eq!(s["metadata"]["name"], "e-ingress-tls");
        assert_eq!(s["stringData"]["tls.crt"], "CERT");
        let mut off = ing.clone();
        off.tls_cert_pem = None;
        assert!(ingress_tls_secret("e", "ns", &off).is_none());
    }
}
