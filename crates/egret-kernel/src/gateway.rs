//! Thin HTTP client to a running Jupyter Kernel Gateway: ensure/create + delete a kernel.

use crate::error::KernelError;

/// Ensure a kernel exists in the gateway, returning its id.
///
/// If `existing` is `Some` and that kernel is still alive (`GET /api/kernels/{id}`
/// returns 2xx), reuse it. Otherwise create a fresh kernel via `POST /api/kernels`
/// and return its id.
pub async fn ensure_kernel(
    gateway_url: &str,
    existing: Option<&str>,
) -> Result<String, KernelError> {
    let client = reqwest::Client::new();
    if let Some(id) = existing {
        if let Ok(resp) = client
            .get(format!("{gateway_url}/api/kernels/{id}"))
            .send()
            .await
        {
            if resp.status().is_success() {
                return Ok(id.to_string());
            }
        }
    }
    let resp = client
        .post(format!("{gateway_url}/api/kernels"))
        .json(&serde_json::json!({ "name": "python3" }))
        .send()
        .await
        .map_err(|e| KernelError::Gateway(format!("create kernel request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(KernelError::Gateway(format!(
            "create kernel: gateway returned HTTP {}",
            resp.status()
        )));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KernelError::Gateway(format!("create kernel: bad response body: {e}")))?;
    v.get("id")
        .and_then(|i| i.as_str())
        .map(str::to_string)
        .ok_or_else(|| KernelError::Gateway("create kernel: response missing \"id\"".into()))
}

/// Best-effort delete of a kernel (errors ignored — used on cleanup paths).
pub async fn delete_kernel(gateway_url: &str, kernel_id: &str) {
    let _ = reqwest::Client::new()
        .delete(format!("{gateway_url}/api/kernels/{kernel_id}"))
        .send()
        .await;
}

/// Interrupt a running kernel via the Jupyter Kernel Gateway REST API.
pub async fn interrupt_kernel(gateway_url: &str, kernel_id: &str) -> Result<(), KernelError> {
    let resp = reqwest::Client::new()
        .post(format!("{gateway_url}/api/kernels/{kernel_id}/interrupt"))
        .send()
        .await
        .map_err(|e| KernelError::Gateway(format!("interrupt request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(KernelError::Gateway(format!(
            "interrupt: gateway returned HTTP {}",
            resp.status()
        )));
    }
    Ok(())
}

/// Restart a running kernel; returns the kernel id (unchanged for an in-place restart).
pub async fn restart_kernel(gateway_url: &str, kernel_id: &str) -> Result<String, KernelError> {
    let resp = reqwest::Client::new()
        .post(format!("{gateway_url}/api/kernels/{kernel_id}/restart"))
        .send()
        .await
        .map_err(|e| KernelError::Gateway(format!("restart request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(KernelError::Gateway(format!(
            "restart: gateway returned HTTP {}",
            resp.status()
        )));
    }
    Ok(kernel_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    /// Spawn a fake Jupyter Kernel Gateway on a random localhost port.
    /// Returns its base url (e.g. "http://127.0.0.1:54321").
    async fn spawn_fake_gateway() -> String {
        async fn create() -> Json<serde_json::Value> {
            Json(serde_json::json!({ "id": "k1" }))
        }
        async fn get_one(
            axum::extract::Path(id): axum::extract::Path<String>,
        ) -> axum::http::StatusCode {
            if id == "k1" {
                axum::http::StatusCode::OK
            } else {
                axum::http::StatusCode::NOT_FOUND
            }
        }
        async fn del_one(
            axum::extract::Path(_id): axum::extract::Path<String>,
        ) -> axum::http::StatusCode {
            axum::http::StatusCode::NO_CONTENT
        }
        async fn interrupt_one(
            axum::extract::Path(_id): axum::extract::Path<String>,
        ) -> axum::http::StatusCode {
            axum::http::StatusCode::NO_CONTENT
        }
        async fn restart_one(
            axum::extract::Path(id): axum::extract::Path<String>,
        ) -> Json<serde_json::Value> {
            Json(serde_json::json!({ "id": id }))
        }
        let app = Router::new()
            .route("/api/kernels", post(create))
            .route("/api/kernels/{id}", get(get_one).delete(del_one))
            .route("/api/kernels/{id}/interrupt", post(interrupt_one))
            .route("/api/kernels/{id}/restart", post(restart_one));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn ensure_kernel_creates_reuses_recreates() {
        let base = spawn_fake_gateway().await;
        // no existing → create
        assert_eq!(ensure_kernel(&base, None).await.unwrap(), "k1");
        // existing alive (k1) → reuse
        assert_eq!(ensure_kernel(&base, Some("k1")).await.unwrap(), "k1");
        // existing dead (404) → recreate
        assert_eq!(ensure_kernel(&base, Some("dead")).await.unwrap(), "k1");
    }

    #[tokio::test]
    async fn interrupt_and_restart_hit_the_gateway() {
        let url = spawn_fake_gateway().await;
        interrupt_kernel(&url, "k1").await.unwrap();
        let id = restart_kernel(&url, "k1").await.unwrap();
        assert_eq!(id, "k1");
    }

    #[tokio::test]
    async fn delete_kernel_is_best_effort() {
        let base = spawn_fake_gateway().await;
        // must not panic / error even though we ignore the result
        delete_kernel(&base, "k1").await;
    }
}
