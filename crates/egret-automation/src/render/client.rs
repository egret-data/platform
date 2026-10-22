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
    /// The single-object form. Kept as the primary field so every existing
    /// caller and the renderer's own one-URL fast path are unchanged.
    pub url: String,
    /// The multi-object form. When non-empty the renderer captures each page
    /// and returns ONE document; `url` is then only used for logging.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
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

/// How long one render may take, end to end, for a request covering
/// `objects` print pages (KI-167).
///
/// The renderer waits up to 20 s (`renderer/server.js`'s `READY_TIMEOUT_MS`)
/// for EACH page to signal readiness, and captures the pages of a multi-object
/// report ONE AFTER ANOTHER, so a fixed client timeout is wrong for every
/// report but one size: it used to be 30 s, and two medium dashboards (about
/// 20 s each) were cut off while the renderer was still legitimately working,
/// as a bare transport error that did not even say "timeout".
///
/// Derived from the object count rather than made a config key: the number
/// that matters is known on every call, and an operator would have to tune a
/// key to the largest report anyone writes. 30 s a page covers the 20 s ready
/// wait plus navigation and capture; the extra page covers waiting behind one
/// other render for one of the renderer's two capture slots
/// (`RENDER_MAX_CONCURRENT`), which concurrent ticks make likely. The design
/// rule still holds per page — the renderer gives up on a page first and
/// answers with a clean `504 {"error"}` this client surfaces verbatim.
pub fn render_budget(objects: usize) -> Duration {
    PER_PAGE * (objects.max(1) as u32 + 1)
}

const PER_PAGE: Duration = Duration::from_secs(30);

impl HttpRenderer {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            // No client-wide timeout: each request sets its own from
            // `render_budget`, because the right value depends on how many
            // pages that request covers.
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl Renderer for HttpRenderer {
    async fn render(&self, req: RenderRequest) -> Result<Vec<u8>, String> {
        let url = format!("{}/render", self.base_url.trim_end_matches('/'));
        let objects = req.urls.len().max(1);
        let budget = render_budget(objects);
        let resp = self
            .client
            .post(&url)
            .timeout(budget)
            .json(&req)
            .send()
            .await
            .map_err(|e| transport_error(&e, budget, objects))?;

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
            .map_err(|e| transport_error(&e, budget, objects))
    }
}

/// A transport failure in words a reader of the run log can act on. A timeout
/// says so, with the budget and page count — the bare reqwest text ("error
/// sending request for url") read as "the renderer is down" when it was only
/// slow.
fn transport_error(e: &reqwest::Error, budget: Duration, objects: usize) -> String {
    if e.is_timeout() {
        format!(
            "the renderer did not finish within {}s for {objects} page(s) (timeout)",
            budget.as_secs()
        )
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_grows_with_the_number_of_pages() {
        // KI-167: two dashboards of about 20 s each did not fit in the old
        // fixed 30 s. Each page may legitimately take the renderer's full
        // 20 s ready wait, one after another.
        const READY: Duration = Duration::from_secs(20);
        for objects in 1..=20 {
            assert!(
                render_budget(objects) > READY * objects as u32,
                "{objects} page(s) must fit {objects} full ready waits: {:?}",
                render_budget(objects)
            );
        }
        assert!(render_budget(2) > render_budget(1));
        assert_eq!(
            render_budget(0),
            render_budget(1),
            "a request is at least one page"
        );
    }

    #[tokio::test]
    async fn a_slow_renderer_is_reported_as_a_timeout() {
        // A listener that accepts and never answers: the request must end as
        // a timeout that says so, not as "error sending request".
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = reqwest::Client::new();
        let budget = Duration::from_millis(200);
        let err = client
            .post(format!("http://{addr}/render"))
            .timeout(budget)
            .send()
            .await
            .unwrap_err();
        let msg = transport_error(&err, budget, 2);
        assert!(msg.contains("timeout"), "{msg}");
        assert!(msg.contains("2 page(s)"), "{msg}");
        hold.abort();
    }
}
