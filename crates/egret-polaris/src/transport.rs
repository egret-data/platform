//! Mockable HTTP boundary for the Polaris management client.
//!
//! All network I/O funnels through the [`HttpTransport`] trait so tests can
//! swap in a deterministic fake. [`ReqwestTransport`] is the production impl.

use crate::error::PolarisError;

/// Async HTTP boundary. Implementors perform the actual request and return the
/// raw `(status, body_bytes)` so the caller decides how to decode.
#[async_trait::async_trait]
pub trait HttpTransport: Send + Sync {
    /// Send a JSON (or bodyless) request. `method` is `"GET"`/`"POST"`/`"PUT"`/`"DELETE"`.
    async fn send(
        &self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        json_body: Option<serde_json::Value>,
    ) -> Result<(u16, Vec<u8>), PolarisError>;

    /// Send an `application/x-www-form-urlencoded` request. Used for the
    /// Keycloak client-credentials token endpoint, which is not JSON.
    async fn send_form(
        &self,
        method: &str,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>), PolarisError>;
}

/// Production [`HttpTransport`] backed by a [`reqwest::Client`].
pub struct ReqwestTransport {
    pub http: reqwest::Client,
}

impl ReqwestTransport {
    /// Build a transport with a fresh default client.
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_method(method: &str) -> Result<reqwest::Method, PolarisError> {
    method
        .parse::<reqwest::Method>()
        .map_err(|e| PolarisError::Http(format!("invalid method {method}: {e}")))
}

#[async_trait::async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(
        &self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        json_body: Option<serde_json::Value>,
    ) -> Result<(u16, Vec<u8>), PolarisError> {
        let mut req = self.http.request(parse_method(method)?, url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        if let Some(j) = json_body {
            req = req.json(&j);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?.to_vec();
        Ok((status, bytes))
    }

    async fn send_form(
        &self,
        method: &str,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>), PolarisError> {
        let req = self.http.request(parse_method(method)?, url).form(form);
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?.to_vec();
        Ok((status, bytes))
    }
}

/// A deterministic in-memory [`HttpTransport`] for tests.
///
/// Construct with a queue of scripted `(status, body)` responses which are
/// returned in order. Every call is recorded so tests can assert on the
/// method, url, bearer, and body. Cloning shares the same underlying state
/// (via `Arc<Mutex<..>>`), so a test can keep a handle after moving a clone
/// into the client under test.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct FakeTransport {
    inner: std::sync::Arc<std::sync::Mutex<FakeState>>,
}

#[cfg(test)]
struct FakeState {
    responses: std::collections::VecDeque<(u16, Vec<u8>)>,
    calls: Vec<RecordedCall>,
}

/// A single recorded request against a [`FakeTransport`].
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct RecordedCall {
    pub method: String,
    pub url: String,
    /// Bearer token sent (recorded for later grant/role tests; not yet asserted).
    #[allow(dead_code)]
    pub bearer: Option<String>,
    /// Request body as JSON. Form requests are recorded as a JSON object of
    /// their fields for uniform assertions.
    pub body: serde_json::Value,
}

#[cfg(test)]
impl FakeTransport {
    /// Create a fake seeded with scripted responses returned in order.
    pub fn new(responses: Vec<(u16, Vec<u8>)>) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(FakeState {
                responses: responses.into_iter().collect(),
                calls: Vec::new(),
            })),
        }
    }

    /// Number of requests recorded so far.
    pub fn calls(&self) -> usize {
        self.inner.lock().unwrap().calls.len()
    }

    /// The most recent request as `(method, url, body)`.
    pub fn last_request(&self) -> (String, String, serde_json::Value) {
        let state = self.inner.lock().unwrap();
        let c = state.calls.last().expect("no recorded calls");
        (c.method.clone(), c.url.clone(), c.body.clone())
    }

    fn record_and_respond(&self, call: RecordedCall) -> Result<(u16, Vec<u8>), PolarisError> {
        let mut state = self.inner.lock().unwrap();
        state.calls.push(call);
        state
            .responses
            .pop_front()
            .ok_or_else(|| PolarisError::Http("FakeTransport: no scripted response left".into()))
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl HttpTransport for FakeTransport {
    async fn send(
        &self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        json_body: Option<serde_json::Value>,
    ) -> Result<(u16, Vec<u8>), PolarisError> {
        self.record_and_respond(RecordedCall {
            method: method.to_string(),
            url: url.to_string(),
            bearer: bearer.map(|b| b.to_string()),
            body: json_body.unwrap_or(serde_json::Value::Null),
        })
    }

    async fn send_form(
        &self,
        method: &str,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>), PolarisError> {
        let body = serde_json::Value::Object(
            form.iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
                .collect(),
        );
        self.record_and_respond(RecordedCall {
            method: method.to_string(),
            url: url.to_string(),
            bearer: None,
            body,
        })
    }
}
