//! Env-var serialization helpers: convert a [`NotebookRuntime`] manifest into
//! the [`KernelEnv`] map that is forwarded to the kernel gateway process.

use egret_analytics_api::NotebookRuntime;

use crate::launcher::KernelEnv;

/// Serialize `m` into a [`KernelEnv`] map understood by the Daft kernel.
///
/// Keys injected:
/// - `EGRET_NOTEBOOK_RUNTIME` — the full manifest as a JSON string so the
///   kernel can self-configure without an extra round-trip.
/// - `EGRET_API_URL` — base URL of the egret HTTP API (e.g. `http://localhost:3000`).
/// - `EGRET_KERNEL_TOKEN` — bearer token the kernel uses for callback requests
///   to egret. **D1b placeholder:** pass `""` here; the real callback endpoints
///   and token issuance are implemented in D1c/E1. The field is wired now so
///   T6 (lifecycle routes) can pass it through without API changes later.
pub fn manifest_to_env(m: &NotebookRuntime, api_url: &str, kernel_token: &str) -> KernelEnv {
    let mut env = KernelEnv::new();
    env.insert(
        "EGRET_NOTEBOOK_RUNTIME".into(),
        serde_json::to_string(m).unwrap_or_default(),
    );
    env.insert("EGRET_API_URL".into(), api_url.to_string());
    env.insert("EGRET_KERNEL_TOKEN".into(), kernel_token.to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_analytics_api::{EngineOption, NotebookRuntime};
    use uuid::Uuid;

    fn make_runtime() -> NotebookRuntime {
        NotebookRuntime {
            username: "alice".to_string(),
            engines: vec![EngineOption {
                id: Uuid::nil(),
                name: "trino-dev".to_string(),
                engine: "trino".to_string(),
                enabled: true,
            }],
            engine_jwt: Some("engine.jwt.token".to_string()),
            engine_jwt_expires_at: Some(
                chrono::DateTime::<chrono::Utc>::from_timestamp(4102444800, 0).unwrap(),
            ),
        }
    }

    #[test]
    fn manifest_to_env_serializes_runtime() {
        let m = make_runtime();
        let env = manifest_to_env(&m, "http://x", "");

        // EGRET_API_URL
        assert_eq!(
            env.get("EGRET_API_URL").map(String::as_str),
            Some("http://x")
        );

        // EGRET_KERNEL_TOKEN (placeholder empty)
        assert_eq!(env.get("EGRET_KERNEL_TOKEN").map(String::as_str), Some(""));

        // EGRET_NOTEBOOK_RUNTIME: must be valid JSON with username
        let raw = env
            .get("EGRET_NOTEBOOK_RUNTIME")
            .expect("EGRET_NOTEBOOK_RUNTIME key must be present");

        let v: serde_json::Value =
            serde_json::from_str(raw).expect("EGRET_NOTEBOOK_RUNTIME must be valid JSON");

        assert_eq!(
            v["username"].as_str(),
            Some("alice"),
            "username must round-trip"
        );
    }
}
