//! Native Trino monitor DTOs + pure Trino-REST-JSON → DTO mappers.
//!
//! These functions map the coordinator's `/v1/query`, `/v1/query/{id}`, and
//! `/v1/node` JSON into egret DTOs serialized to the frontend. Times/sizes pass
//! through as RAW Trino strings ("1.23s", "12MB") — no Duration/DataSize
//! parsing — and `outputStage` passes through as a raw `serde_json::Value` for
//! the frontend stage visualizer.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use egret_auth::principal::Auth;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use egret_core::http::ErrorBody;

use crate::engines::monitor_password;
use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

#[derive(Serialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryDto {
    pub id: String,
    pub user: String,
    pub state: String,
    pub query: String,
    pub elapsed: String,
    pub cpu: String,
    pub memory: String,
    pub progress_percent: f64,
}

#[derive(Serialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryDetailDto {
    #[serde(flatten)]
    pub base: QueryDto,
    pub error_message: Option<String>,
    #[schema(value_type = Object)]
    pub output_stage: Option<Value>,
}

#[derive(Serialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeDto {
    pub uri: String,
    pub recent_failures: f64,
}

#[derive(Serialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDto {
    pub running: u32,
    pub queued: u32,
    pub blocked: u32,
    pub nodes: Vec<NodeDto>,
}

fn s(v: &Value, p: &str) -> String {
    v.get(p).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub fn query_dto(v: &Value) -> QueryDto {
    let qs = &v["queryStats"];
    QueryDto {
        id: s(v, "queryId"),
        user: v["session"]["user"].as_str().unwrap_or("").to_string(),
        state: s(v, "state"),
        query: s(v, "query"),
        elapsed: qs["elapsedTime"].as_str().unwrap_or("").to_string(),
        cpu: qs["totalCpuTime"].as_str().unwrap_or("").to_string(),
        memory: qs["userMemoryReservation"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        progress_percent: qs["progressPercentage"].as_f64().unwrap_or(0.0),
    }
}

pub fn query_detail_dto(v: &Value) -> QueryDetailDto {
    QueryDetailDto {
        base: query_dto(v),
        error_message: v["failureInfo"]["message"].as_str().map(|s| s.to_string()),
        output_stage: v.get("outputStage").filter(|s| !s.is_null()).cloned(),
    }
}

pub fn cluster_dto(queries: &Value, nodes: &Value) -> ClusterDto {
    let count = |st: &str| {
        queries
            .as_array()
            .map(|a| a.iter().filter(|q| q["state"] == st).count() as u32)
            .unwrap_or(0)
    };
    ClusterDto {
        running: count("RUNNING"),
        queued: count("QUEUED"),
        blocked: count("BLOCKED"),
        nodes: nodes
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| NodeDto {
                        uri: n["uri"].as_str().unwrap_or("").to_string(),
                        recent_failures: n["recentFailures"].as_f64().unwrap_or(0.0),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ── routes ─────────────────────────────────────────────────────────────────────
//
// The native Trino monitor: each route resolves the engine, builds the monitor
// auth headers, and issues a raw HTTP request to the coordinator via
// `coordinator_call`, which PREFERS the engine's public Ingress URL when it has
// one and falls back to the kube-free `EngineProvisioner::coordinator_request`
// seam (`{name}-trino` Service) otherwise or on an ingress transport error. A
// coordinator that is unreachable, non-2xx, or returns a non-JSON error page
// maps to `CpError::BadGateway` (502) so the UI can surface it distinctly from a
// 4xx/5xx egret error.

/// Monitor routes mounted onto the engines router.
pub fn routes() -> Router<CpState> {
    Router::new()
        .route("/engines/{id}/queries", get(list_queries))
        .route(
            "/engines/{id}/queries/{qid}",
            get(get_query).delete(kill_query),
        )
        .route("/engines/{id}/cluster", get(get_cluster))
}

/// Monitor auth headers for a coordinator request. When the engine's stored spec
/// has an auth block (`trinoConfig.auth`), authenticate as the derived
/// `egret-monitor` file-password user over HTTP Basic (plus `X-Forwarded-Proto:
/// https` so Trino accepts Basic on the proxied hop); otherwise pass the trino
/// user header directly (no-auth engines).
fn monitor_headers(engine: &Engine) -> Vec<(String, String)> {
    if engine.spec["trinoConfig"]["auth"].is_object() {
        let creds = format!("egret-monitor:{}", monitor_password(engine.id));
        let b64 = base64::engine::general_purpose::STANDARD.encode(creds);
        vec![
            ("Authorization".into(), format!("Basic {b64}")),
            ("X-Forwarded-Proto".into(), "https".into()),
        ]
    } else {
        vec![("X-Trino-User".into(), "egret-monitor".into())]
    }
}

async fn load_engine(st: &CpState, id: Uuid) -> Result<Engine, CpError> {
    st.engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)
}

/// The engine's public Ingress base URL (`https://{host}`) when
/// `trinoConfig.ingress` is enabled with a non-empty host; otherwise `None`.
fn ingress_base(engine: &Engine) -> Option<String> {
    let ing = &engine.spec["trinoConfig"]["ingress"];
    if ing["enabled"].as_bool() != Some(true) {
        return None;
    }
    let host = ing["host"].as_str().filter(|h| !h.is_empty())?;
    Some(format!("https://{host}"))
}

/// Attempt the coordinator request via the engine's public Ingress. Returns
/// `Some((status, body))` on ANY HTTP response, or `None` when there is no
/// usable ingress or the request fails at the transport level (bad method,
/// client build, connect/timeout) — so the caller falls back to the internal
/// seam. The dev ingress is self-signed (`danger_accept_invalid_certs`) and a
/// short timeout lets a dead ingress fall back fast.
async fn try_ingress(
    engine: &Engine,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
) -> Option<(u16, Vec<u8>)> {
    let url = format!("{}{path}", ingress_base(engine)?);
    let m = method.parse::<reqwest::Method>().ok()?;
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(4))
        .build()
        .ok()?;
    let mut rb = client.request(m, &url);
    for (k, v) in headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some(b) = body {
        rb = rb.body(b);
    }
    let resp = rb.send().await.ok()?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.ok()?.to_vec();
    Some((status, bytes))
}

/// Issue one coordinator request, preferring the engine's public Ingress URL
/// when it has one and falling back to the internal kube-free seam otherwise or
/// on a transport error. Transport errors map to a 502.
///
/// `pub(crate)` so [`crate::engines::test_catalog`] can drive the raw Trino
/// `/v1/statement` submit-then-poll protocol with the SAME service-name +
/// header + ingress-preference behavior as the native monitor, instead of
/// duplicating it.
pub(crate) async fn coordinator_call(
    st: &CpState,
    engine: &Engine,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
) -> Result<(u16, Vec<u8>), CpError> {
    let headers = monitor_headers(engine);
    if let Some(resp) = try_ingress(engine, method, path, &headers, body.clone()).await {
        return Ok(resp);
    }
    let kube = st
        .kube
        .as_ref()
        .ok_or_else(|| CpError::BadRequest("kubernetes not configured".into()))?;
    let service = format!("{}-trino", engine.name);
    kube.coordinator_request(&engine.namespace, &service, method, path, &headers, body)
        .await
        .map_err(|_| CpError::BadGateway)
}

/// Call the coordinator and parse a 2xx JSON body; any non-2xx status or
/// non-JSON body (e.g. a Trino HTML error page) is a 502.
async fn coordinator_json(
    st: &CpState,
    engine: &Engine,
    method: &str,
    path: &str,
) -> Result<Value, CpError> {
    let (code, body) = coordinator_call(st, engine, method, path, None).await?;
    if !(200..300).contains(&code) {
        return Err(CpError::BadGateway);
    }
    serde_json::from_slice(&body).map_err(|_| CpError::BadGateway)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/queries",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "Live queries on the engine's Trino coordinator", body = Vec<QueryDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 502, description = "Trino coordinator unreachable or returned a non-JSON error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_queries(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<QueryDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = load_engine(&st, id).await?;
    let v = coordinator_json(&st, &engine, "GET", "/v1/query").await?;
    let list = v
        .as_array()
        .map(|a| a.iter().map(query_dto).collect())
        .unwrap_or_default();
    Ok(Json(list))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/queries/{qid}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("qid" = String, Path, description = "Trino query id"),
    ),
    responses(
        (status = 200, description = "Query detail including error message and output-stage plan", body = QueryDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 502, description = "Trino coordinator unreachable or returned a non-JSON error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_query(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, qid)): Path<(Uuid, String)>,
) -> Result<Json<QueryDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = load_engine(&st, id).await?;
    let v = coordinator_json(&st, &engine, "GET", &format!("/v1/query/{qid}")).await?;
    Ok(Json(query_detail_dto(&v)))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/engines/{id}/queries/{qid}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("qid" = String, Path, description = "Trino query id"),
    ),
    responses(
        (status = 200, description = "Query cancelled on the coordinator"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:kill permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 502, description = "Trino coordinator unreachable or rejected the cancel", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn kill_query(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, qid)): Path<(Uuid, String)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:kill").await?;
    let engine = load_engine(&st, id).await?;
    let (code, _) =
        coordinator_call(&st, &engine, "DELETE", &format!("/v1/query/{qid}"), None).await?;
    if !(200..300).contains(&code) {
        return Err(CpError::BadGateway);
    }
    Ok(StatusCode::OK)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/cluster",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "Cluster summary: running/queued/blocked query tallies + node list", body = ClusterDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 502, description = "Trino coordinator unreachable or returned a non-JSON error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_cluster(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<ClusterDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = load_engine(&st, id).await?;
    let nodes = coordinator_json(&st, &engine, "GET", "/v1/node").await?;
    let queries = coordinator_json(&st, &engine, "GET", "/v1/query").await?;
    Ok(Json(cluster_dto(&queries, &nodes)))
}

/// OpenAPI fragment for the native-Trino monitor routes merged onto the engines
/// router (query list/detail/kill + cluster summary). Shares the
/// `control-plane-engines` tag with [`crate::engines::EngineMgmtApi`] and is
/// unioned into `egret_engine_mgmt::openapi()`. Errors use the shared
/// [`egret_core::http::ErrorBody`].
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_queries, get_query, kill_query, get_cluster),
    components(schemas(QueryDto, QueryDetailDto, NodeDto, ClusterDto, ErrorBody))
)]
pub struct MonitorApi;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn map_query_list_item() {
        let v = json!({"queryId":"20260704_x","state":"RUNNING","query":"SELECT 1",
            "session":{"user":"bob"},
            "queryStats":{"elapsedTime":"1.23s","totalCpuTime":"0.50s","userMemoryReservation":"12MB","progressPercentage":42.0}});
        let d = query_dto(&v);
        assert_eq!(d.id, "20260704_x");
        assert_eq!(d.user, "bob");
        assert_eq!(d.state, "RUNNING");
        assert_eq!(d.query, "SELECT 1");
        assert_eq!(d.elapsed, "1.23s");
        assert_eq!(d.memory, "12MB");
        assert_eq!(d.progress_percent, 42.0);
    }

    #[test]
    fn map_query_detail_passes_output_stage_and_error() {
        let v = json!({"queryId":"q","state":"FAILED","query":"SELECT 1","session":{"user":"u"},
            "queryStats":{"elapsedTime":"1s","totalCpuTime":"1s","userMemoryReservation":"0B","progressPercentage":100.0},
            "failureInfo":{"message":"boom"},
            "outputStage":{"stageId":"0","state":"FINISHED","subStages":[]}});
        let d = query_detail_dto(&v);
        assert_eq!(d.base.state, "FAILED");
        assert_eq!(d.error_message.as_deref(), Some("boom"));
        assert_eq!(d.output_stage.as_ref().unwrap()["stageId"], "0");
    }

    fn engine_with_spec(spec: Value) -> Engine {
        Engine {
            id: Uuid::nil(),
            name: "t".into(),
            r#type: "trino".into(),
            namespace: "ns".into(),
            warehouse_id: Uuid::nil(),
            spec,
            status: "running".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn ingress_base_none_when_disabled_or_missing() {
        assert_eq!(ingress_base(&engine_with_spec(json!({}))), None);
        assert_eq!(
            ingress_base(&engine_with_spec(
                json!({"trinoConfig":{"ingress":{"enabled":false,"host":"h.local"}}})
            )),
            None
        );
        // enabled but empty/absent host → no usable ingress.
        assert_eq!(
            ingress_base(&engine_with_spec(
                json!({"trinoConfig":{"ingress":{"enabled":true,"host":""}}})
            )),
            None
        );
        assert_eq!(
            ingress_base(&engine_with_spec(
                json!({"trinoConfig":{"ingress":{"enabled":true}}})
            )),
            None
        );
    }

    #[test]
    fn ingress_base_uses_host_when_enabled() {
        let e = engine_with_spec(
            json!({"trinoConfig":{"ingress":{"enabled":true,"host":"trino.dev.local"}}}),
        );
        assert_eq!(ingress_base(&e).as_deref(), Some("https://trino.dev.local"));
    }

    #[test]
    fn cluster_tally_counts_states() {
        let queries = json!([{"state":"RUNNING"},{"state":"RUNNING"},{"state":"QUEUED"},{"state":"FINISHED"}]);
        let nodes = json!([{"uri":"http://w1:8080","recentFailures":0.0}]);
        let c = cluster_dto(&queries, &nodes);
        assert_eq!(c.running, 2);
        assert_eq!(c.queued, 1);
        assert_eq!(c.nodes.len(), 1);
    }
}
