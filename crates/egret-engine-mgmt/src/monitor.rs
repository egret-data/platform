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

use crate::engines::monitor_password_for;
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
/// has an auth block (`trinoConfig.auth`), authenticate as the `egret-monitor`
/// file-password user over HTTP Basic (plus `X-Forwarded-Proto: https` so Trino
/// accepts Basic on the proxied hop), with the password the engine's pods were
/// last given ([`monitor_password_for`]); otherwise pass the trino user header
/// directly (no-auth engines).
async fn monitor_headers(st: &CpState, engine: &Engine) -> Result<Vec<(String, String)>, CpError> {
    if engine.spec["trinoConfig"]["auth"].is_object() {
        let password = monitor_password_for(st, engine).await?;
        let creds = format!("egret-monitor:{password}");
        let b64 = base64::engine::general_purpose::STANDARD.encode(creds);
        Ok(vec![
            ("Authorization".into(), format!("Basic {b64}")),
            ("X-Forwarded-Proto".into(), "https".into()),
        ])
    } else {
        Ok(vec![("X-Trino-User".into(), "egret-monitor".into())])
    }
}

async fn load_engine(st: &CpState, id: Uuid) -> Result<Engine, CpError> {
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // Every monitor route loads its engine here, so this is the one place an
    // external engine is turned away: egret never calls its coordinator.
    crate::external::refuse_if_external(&engine, "monitor it or cancel its queries")?;
    Ok(engine)
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
    let mut cb = reqwest::Client::builder().timeout(std::time::Duration::from_secs(4));
    // Verify the ingress cert by default; only accept an invalid/self-signed cert
    // when the operator sets the explicit `EGRET_ENGINE_TLS_INSECURE` opt-out (the
    // same escape hatch the query connectors honor). This is a health probe (no
    // per-user credentials), so on a verify failure `send` errors and the caller
    // falls back to the internal seam — a self-signed dev ingress just degrades to
    // the fallback instead of being silently trusted.
    if egret_query::execution::connector::tls_insecure_optout() {
        cb = cb.danger_accept_invalid_certs(true);
    }
    let client = cb.build().ok()?;
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
    // The chokepoint under every coordinator request, including the catalog
    // tests, whatever route reached it.
    crate::external::refuse_if_external(engine, "send requests to its coordinator")?;
    let headers = monitor_headers(st, engine).await?;
    if let Some(resp) = try_ingress(engine, method, path, &headers, body.clone()).await {
        return Ok(resp);
    }
    let service = format!("{}-trino", engine.name);
    dispatch_coordinator(
        st.kube.as_deref(),
        st.coordinator_transport.as_deref(),
        &engine.namespace,
        &service,
        method,
        path,
        &headers,
        body,
    )
    .await
}

/// The seam fallback of [`coordinator_call`]: the kube client when there is
/// one (unchanged), else the plain-HTTP transport (Docker), else the old
/// "not configured" error.
#[allow(clippy::too_many_arguments)]
async fn dispatch_coordinator(
    kube: Option<&dyn egret_provisioner::KubeClient>,
    transport: Option<&dyn egret_engine_mgmt_api::CoordinatorTransport>,
    namespace: &str,
    service: &str,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
) -> Result<(u16, Vec<u8>), CpError> {
    if let Some(kube) = kube {
        return kube
            .coordinator_request(namespace, service, method, path, headers, body)
            .await
            .map_err(|_| CpError::BadGateway);
    }
    if let Some(transport) = transport {
        return transport
            .request(namespace, service, method, path, headers, body)
            .await
            .map_err(|_| CpError::BadGateway);
    }
    Err(CpError::BadRequest("kubernetes not configured".into()))
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not monitor it", body = ErrorBody),
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not monitor it", body = ErrorBody),
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not cancel queries on it", body = ErrorBody),
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not monitor it", body = ErrorBody),
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

    #[derive(Default)]
    struct FakeTransport {
        calls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::CoordinatorTransport for FakeTransport {
        async fn request(
            &self,
            namespace: &str,
            service: &str,
            method: &str,
            path: &str,
            headers: &[(String, String)],
            body: Option<Vec<u8>>,
        ) -> Result<(u16, Vec<u8>), String> {
            self.calls.lock().unwrap().push(format!(
                "{namespace}|{service}|{method}|{path}|{headers:?}|{body:?}"
            ));
            Ok((207, b"from-transport".to_vec()))
        }
    }

    fn hdrs() -> Vec<(String, String)> {
        vec![("X-Trino-User".into(), "monitor".into())]
    }

    #[tokio::test]
    async fn dispatch_uses_the_transport_when_there_is_no_kube() {
        let t = FakeTransport::default();
        let got = dispatch_coordinator(
            None,
            Some(&t),
            "ns",
            "e-trino",
            "GET",
            "/v1/query",
            &hdrs(),
            Some(vec![1]),
        )
        .await
        .unwrap();
        assert_eq!(got, (207, b"from-transport".to_vec()));
        assert_eq!(
            t.calls.lock().unwrap().as_slice(),
            ["ns|e-trino|GET|/v1/query|[(\"X-Trino-User\", \"monitor\")]|Some([1])"]
        );
    }

    #[tokio::test]
    async fn dispatch_prefers_kube_over_the_transport() {
        let kube = egret_provisioner::kube_client::FakeKube::default();
        kube.set_coordinator_response(200, b"from-kube".to_vec());
        let t = FakeTransport::default();
        let got = dispatch_coordinator(
            Some(&kube),
            Some(&t),
            "ns",
            "e-trino",
            "GET",
            "/v1/query",
            &hdrs(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(got, (200, b"from-kube".to_vec()));
        assert!(t.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_with_neither_keeps_the_old_error() {
        let err = dispatch_coordinator(None, None, "ns", "s", "GET", "/", &[], None)
            .await
            .unwrap_err();
        assert!(matches!(err, CpError::BadRequest(m) if m == "kubernetes not configured"));
    }

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
            spec,
            status: "running".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            created_at: chrono::Utc::now(),
            origin: "provisioned".into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: None,
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

    /// Allow-all `Pdp`, only so `CpState` can be constructed — `monitor_headers`
    /// never touches `st.pdp`.
    struct AllowAllPdp;
    #[async_trait::async_trait]
    impl egret_core::authz::Pdp for AllowAllPdp {
        async fn authorize(
            &self,
            _s: &egret_core::authz::Subject,
            _a: &str,
            _r: &egret_core::authz::Resource,
        ) -> egret_core::Result<egret_core::authz::Decision> {
            Ok(egret_core::authz::Decision {
                allow: true,
                reasons: vec![],
            })
        }
    }

    /// Build a real `CpState` against the local dev Postgres, mirroring
    /// `engines::tests::test_state`. `None` when `DATABASE_URL` isn't set so this
    /// suite doesn't fail in environments without a database.
    async fn test_state() -> Option<CpState> {
        let dsn = egret_test_db::url().await?;
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
        let workspace_secrets =
            egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
        let databases =
            egret_query::DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());
        Some(CpState {
            engine_mgmt: crate::EngineMgmtStore::new(db.clone()),
            assignment: crate::EngineAssignmentService::new(
                crate::EngineMgmtStore::new(db.clone()),
                databases,
            ),
            catalog: std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
            storage_registry: egret_storage::StorageRegistry::new(db.clone()),
            authz: std::sync::Arc::new(egret_policy::PolicyStore::new(db.clone())),
            secrets: workspace_secrets,
            sessions: egret_auth::session::SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            users: egret_auth::users::UserRepo::new(db.clone()),
            roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
            pdp: std::sync::Arc::new(AllowAllPdp),
            engine_provisioner: None,
            kube: None,
            coordinator_transport: None,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
            engine_oidc_issuer: "https://idp/realms/egret".into(),
            ranger_enabled: false,
            ranger_bearer_configured: false,
            sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers::default(),
            starrocks_ddl: std::sync::Arc::new(crate::starrocks_ddl::FakeStarRocksDdl::default()),
            perm_compiler: None,
            catalog_assignment_compiler: None,
        })
    }

    /// Decode a `headers` list's `Authorization: Basic <b64>` entry back to
    /// `user:pass`.
    fn decoded_basic_credential(headers: &[(String, String)]) -> String {
        let (_, v) = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .expect("Authorization header present");
        let b64 = v.strip_prefix("Basic ").expect("Basic scheme");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("valid base64");
        String::from_utf8(raw).expect("valid utf8")
    }

    /// I3 (SP1 Part B final review): `monitor_headers` is the only consumer of
    /// the rotated monitor password — nothing pinned the credential it actually
    /// builds, so reverting it to always use `legacy_monitor_password(engine.id)`
    /// (the original P1 bug, ignoring any rotated/stored pair) was a green
    /// build. Pins both cases: an engine with a stored `monitor_secret_id` must
    /// present THAT secret's plaintext, and one with none (legacy/NULL column)
    /// must present `legacy_monitor_password(engine.id)`.
    #[tokio::test]
    async fn monitor_headers_uses_the_stored_secret_or_falls_back_to_legacy() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping monitor_headers_uses_the_stored_secret_or_falls_back_to_legacy: DATABASE_URL not set"
            );
            return;
        };

        // (a) Stored pair: `monitor_secret_id` resolves to a known plaintext —
        // the header must carry THAT password, not the legacy formula.
        let secret_name = format!("egret-monitor-test-{}", Uuid::new_v4());
        let secret_id = st
            .secrets
            .create(None, &secret_name, None, "s3cr3t-rotated-pw")
            .await
            .expect("create test secret");
        let mut stored = engine_with_spec(json!({ "trinoConfig": { "auth": {} } }));
        stored.monitor_secret_id = Some(secret_id);
        let headers = monitor_headers(&st, &stored).await.expect("headers");
        assert_eq!(
            decoded_basic_credential(&headers),
            "egret-monitor:s3cr3t-rotated-pw"
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "X-Forwarded-Proto" && v == "https"),
            "Basic auth over the proxied hop needs X-Forwarded-Proto: https"
        );

        // (b) Legacy: no stored secret (NULL `monitor_secret_id`, e.g. an
        // engine that predates rotation) falls back to the derived password.
        let legacy = engine_with_spec(json!({ "trinoConfig": { "auth": {} } }));
        assert_eq!(legacy.monitor_secret_id, None);
        let headers = monitor_headers(&st, &legacy).await.expect("headers");
        assert_eq!(
            decoded_basic_credential(&headers),
            format!(
                "egret-monitor:{}",
                crate::engines::legacy_monitor_password(legacy.id)
            )
        );
    }
}
