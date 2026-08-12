//! HTTP client for `egret-renderer` (P4 §6).
//!
//! `Renderer` is a trait, not a bare function, for the same reason
//! `evaluator::QueryRunner` is: the report branch of the tick must be testable
//! without a real Chromium process, exactly the way `tick_end_to_end.rs`
//! fakes the query engine. `HttpRenderer` is the one production impl.

use std::time::Duration;

use serde::Serialize;

/// What `egret-renderer`'s `POST /render` expects — field names are the wire
/// contract (`renderer/server.js`'s `{url, width, height, format, scale}`),
/// spelled out here rather than derived from an internal type that might be
/// refactored out from under them.
#[derive(Debug, Clone, Serialize)]
pub struct RenderRequest {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub scale: u32,
}

/// Execution seam for the renderer call, mirroring `evaluator::QueryRunner`.
#[async_trait::async_trait]
pub trait Renderer: Send + Sync {
    /// Bytes on success. The renderer's own contract (`renderer/server.js`)
    /// never returns bytes on a non-2xx — a half-drawn chart in someone's
    /// inbox is worse than a missing one — so `Err` here always means nothing
    /// was captured, never a partial image.
    async fn render(&self, req: RenderRequest) -> Result<Vec<u8>, String>;
}

/// The production `Renderer`, calling the `egret-renderer` service over HTTP.
pub struct HttpRenderer {
    base_url: String,
    client: reqwest::Client,
}

/// The renderer's own default readiness timeout
/// (`renderer/server.js`'s `READY_TIMEOUT_MS`) is 20s. This client's timeout
/// is set ABOVE that: the design (`docs/en/specs/2026-08-12-report-renderer-design.md`
/// §6) is "a hard per-render timeout, below the CALLER's, so egret sees a
/// clean error rather than a hanging socket" — i.e. the renderer is meant to
/// give up first and answer with a clean `504 {"error": "..."}, which this
/// client can then surface verbatim. A client timeout shorter than the
/// renderer's own would instead abort the connection while the renderer is
/// still legitimately working, turning a slow-but-successful render into a
/// generic transport error.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

impl HttpRenderer {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::builder()
                .timeout(CLIENT_TIMEOUT)
                .build()
                .expect("reqwest client with a fixed timeout always builds"),
        }
    }
}

#[async_trait::async_trait]
impl Renderer for HttpRenderer {
    async fn render(&self, req: RenderRequest) -> Result<Vec<u8>, String> {
        let url = format!("{}/render", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            let status = resp.status();
            // The renderer's own contract: a non-2xx body is `{"error": "..."}`
            // and NEVER carries image bytes. Fall back to the bare status only
            // if the body itself is unreadable/not JSON — never guess at bytes.
            return Err(match resp.json::<serde_json::Value>().await {
                Ok(v) => v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("renderer returned {status}")),
                Err(_) => format!("renderer returned {status}"),
            });
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| e.to_string())
    }
}
