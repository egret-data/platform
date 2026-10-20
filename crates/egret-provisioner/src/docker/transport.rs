//! Reaching an engine's coordinator over plain HTTP, without Kubernetes.

use crate::kube_client::http_coordinator_request;
use egret_engine_mgmt_api::CoordinatorTransport;
use std::sync::Arc;

type BaseUrl = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// `http://{service}.{namespace}.svc:8080{path}`: the URL the in-cluster
/// Kubernetes branch uses. The coordinator container carries that alias on the
/// engine network, so the same name resolves there.
pub struct HttpCoordinatorTransport {
    base: BaseUrl,
}

impl HttpCoordinatorTransport {
    pub fn new() -> Self {
        Self::with_base(|namespace, service| format!("http://{service}.{namespace}.svc:8080"))
    }

    /// A custom `(namespace, service) -> http://host:port`; for tests, where
    /// the `*.svc` name cannot resolve.
    pub fn with_base(base: impl Fn(&str, &str) -> String + Send + Sync + 'static) -> Self {
        Self {
            base: Arc::new(base),
        }
    }
}

impl Default for HttpCoordinatorTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CoordinatorTransport for HttpCoordinatorTransport {
    async fn request(
        &self,
        namespace: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>), String> {
        let base = (self.base)(namespace, service);
        http_coordinator_request(&base, method, path, headers, body)
            .await
            .map_err(|e| format!("coordinator request to {service}.{namespace} failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn sends_method_path_headers_and_body_and_returns_status_and_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut got = Vec::new();
            // Read until the body ("ping") has arrived.
            while !got.ends_with(b"ping") {
                let n = sock.read(&mut buf).await.unwrap();
                assert!(n > 0, "connection closed before the body");
                got.extend_from_slice(&buf[..n]);
            }
            sock.write_all(
                b"HTTP/1.1 202 Accepted\r\ncontent-length: 4\r\nconnection: close\r\n\r\npong",
            )
            .await
            .unwrap();
            String::from_utf8(got).unwrap()
        });

        let t = HttpCoordinatorTransport::with_base(move |ns, svc| {
            assert_eq!((ns, svc), ("ns", "e-trino"));
            format!("http://{addr}")
        });
        let (status, body) = t
            .request(
                "ns",
                "e-trino",
                "POST",
                "/v1/query",
                &[("X-Trino-User".into(), "monitor".into())],
                Some(b"ping".to_vec()),
            )
            .await
            .unwrap();
        assert_eq!((status, body), (202, b"pong".to_vec()));

        let req = server.await.unwrap().to_lowercase();
        assert!(req.starts_with("post /v1/query http/1.1"), "{req}");
        assert!(req.contains("x-trino-user: monitor"), "{req}");
    }

    #[tokio::test]
    async fn an_unreachable_coordinator_is_an_error_naming_it() {
        let t = HttpCoordinatorTransport::with_base(|_, _| "http://127.0.0.1:1".into());
        let err = t
            .request("ns", "e-trino", "GET", "/", &[], None)
            .await
            .unwrap_err();
        assert!(err.contains("e-trino.ns"), "{err}");
    }
}
