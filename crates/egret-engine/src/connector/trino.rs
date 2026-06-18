//! Trino connector — REST statement protocol with row-cap, timeout, cancellation.

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{ConnSpec, Connector};
use crate::error::EngineError;
use crate::model::{Column, Limits, QueryResult};

pub struct TrinoConnector {
    base_url: String,
    user: String,
    secret: Option<String>,
}

impl TrinoConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        let scheme = spec.options.http_scheme.as_deref().unwrap_or("http");
        let user = spec.options.user.clone().unwrap_or_else(|| "egret".into());
        TrinoConnector {
            base_url: format!("{scheme}://{}:{}", spec.host, spec.port),
            user,
            secret: spec.secret.clone(),
        }
    }
}

#[derive(Deserialize)]
struct TrinoResp {
    #[serde(default, rename = "nextUri")]
    next_uri: Option<String>,
    #[serde(default)]
    error: Option<TrinoError>,
    #[serde(default)]
    columns: Option<Vec<TrinoColumn>>,
    #[serde(default)]
    data: Option<Vec<Vec<serde_json::Value>>>,
    #[serde(default)]
    stats: Option<TrinoStats>,
}
#[derive(Deserialize)]
struct TrinoColumn {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}
#[derive(Deserialize)]
struct TrinoError {
    message: String,
}
#[derive(Deserialize)]
struct TrinoStats {
    #[serde(default, rename = "processedBytes")]
    processed_bytes: Option<u64>,
}

#[async_trait]
impl Connector for TrinoConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError> {
        let client = reqwest::Client::builder()
            .timeout(limits.timeout)
            .build()
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        let session = format!("query_max_run_time={}s", limits.timeout.as_secs().max(1));
        let mut req = client
            .post(format!("{}/v1/statement", self.base_url))
            .header("X-Trino-User", &self.user)
            .header("X-Trino-Session", session)
            .body(sql.to_string());
        if let Some(s) = &self.secret {
            req = req.basic_auth(&self.user, Some(s));
        }

        let cap = limits.max_rows;
        let mut columns: Vec<Column> = Vec::new();
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        let mut bytes_scanned: Option<u64> = None;

        let mut resp: TrinoResp = send(req).await?;
        loop {
            if let Some(err) = resp.error.take() {
                return Err(EngineError::Engine(err.message));
            }
            if columns.is_empty() {
                if let Some(cols) = resp.columns.take() {
                    columns = cols
                        .into_iter()
                        .map(|c| Column {
                            name: c.name,
                            ty: c.ty,
                        })
                        .collect();
                }
            }
            if let Some(st) = &resp.stats {
                if st.processed_bytes.is_some() {
                    bytes_scanned = st.processed_bytes;
                }
            }
            if let Some(data) = resp.data.take() {
                for r in data {
                    if rows.len() <= cap {
                        rows.push(r);
                    }
                    if rows.len() > cap {
                        break;
                    }
                }
            }
            let next = resp.next_uri.take();
            if cancel.is_cancelled() {
                if let Some(uri) = &next {
                    let _ = client
                        .delete(uri)
                        .header("X-Trino-User", &self.user)
                        .send()
                        .await;
                }
                return Err(EngineError::Cancelled);
            }
            if rows.len() > cap {
                if let Some(uri) = &next {
                    let _ = client
                        .delete(uri)
                        .header("X-Trino-User", &self.user)
                        .send()
                        .await;
                }
                break;
            }
            match next {
                None => break,
                Some(uri) => {
                    let mut g = client.get(&uri).header("X-Trino-User", &self.user);
                    if let Some(s) = &self.secret {
                        g = g.basic_auth(&self.user, Some(s));
                    }
                    resp = send(g).await?;
                }
            }
        }

        let truncated = rows.len() > cap;
        if truncated {
            rows.truncate(cap);
        }
        Ok(QueryResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned,
            truncated,
        })
    }
}

async fn send(req: reqwest::RequestBuilder) -> Result<TrinoResp, EngineError> {
    let resp = req
        .send()
        .await
        .map_err(|e| EngineError::Connect(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(EngineError::Engine(format!("http {}", resp.status())));
    }
    resp.json::<TrinoResp>()
        .await
        .map_err(|e| EngineError::Engine(format!("decode: {e}")))
}
