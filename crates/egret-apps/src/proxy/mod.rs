//! The apps-origin proxy (spec §6.2): everything between the browser's iframe and a
//! marimo pod. Pure decisions live in the small modules below and are tested without a
//! network; [`relay`] is the part that moves bytes.
//!
//! The proxy is mounted on its OWN listener, serving only `/_ticket` and the relay. egret's
//! API router is never reachable through it and the relay is never reachable through
//! egret's listener, so a bug in one cannot expose the other (D17).

pub mod headers;
pub mod pathguard;
pub mod pod_ref;
pub mod policy;
mod relay;
mod tokens;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::routing::get;
use axum::Router;
use egret_core::authz::Pdp;
use egret_core::{Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use pod_ref::edit_pod_ref;

use crate::grant::GrantRepo;
use crate::session::AppSessionRepo;

/// Where a pod listens and the secret that gets the proxy past marimo's own token check.
#[derive(Clone, Debug)]
pub struct PodEndpoint {
    pub base: url::Url,
    /// `marimo --token-password`; sent as `Authorization: Bearer`, never shown to a user.
    pub marimo_token: String,
}

/// Finds (and, for an edit pod, starts) the pod a grant is entitled to.
#[async_trait]
pub trait PodResolver: Send + Sync {
    /// The URL segment of an author's edit pod (`/e/{ref}/…`). Pure and stable: the redirect
    /// after a ticket, the cookie's `Path` and the check on every request all use it.
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String;
    /// The author's own edit pod, started if need be.
    async fn edit_pod(&self, user: UserId, ws: WorkspaceId) -> Result<PodEndpoint>;
    /// The pod serving one published version. `NotFound` until P5 launches run pods.
    async fn run_pod(&self, app: Uuid, version: Uuid) -> Result<PodEndpoint>;
}

/// The resolver while no launcher is wired in: the proxy and its checks are live, every pod
/// is unavailable (502). `main` uses it until the edit-pod launcher replaces it.
pub struct UnavailablePods;

#[async_trait]
impl PodResolver for UnavailablePods {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String {
        edit_pod_ref(user, ws)
    }
    async fn edit_pod(&self, _: UserId, _: WorkspaceId) -> Result<PodEndpoint> {
        Err(anyhow::anyhow!("no app pod launcher is configured").into())
    }
    async fn run_pod(&self, _: Uuid, _: Uuid) -> Result<PodEndpoint> {
        Err(egret_core::Error::NotFound("run pod".into()))
    }
}

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// egret's own origin: the only one allowed to frame this host (`frame-ancestors`).
    pub egret_origin: String,
    /// The apps origin is https: the cookie is `Secure` (and `__Secure-` prefixed).
    pub secure: bool,
    pub session_idle: Duration,
    pub session_absolute: Duration,
    pub cookie_ttl: Duration,
}

pub(crate) struct Inner {
    pub(crate) db: Db,
    pub(crate) pdp: Arc<dyn Pdp>,
    pub(crate) grants: GrantRepo,
    pub(crate) sessions: AppSessionRepo,
    pub(crate) pods: Arc<dyn PodResolver>,
    pub(crate) cfg: ProxyConfig,
    pub(crate) tokens: tokens::TokenBook,
    pub(crate) http: reqwest::Client,
}

#[derive(Clone)]
pub struct ProxyState {
    pub(crate) inner: Arc<Inner>,
}

impl ProxyState {
    pub fn new(
        db: Db,
        pdp: Arc<dyn Pdp>,
        grants: GrantRepo,
        sessions: AppSessionRepo,
        pods: Arc<dyn PodResolver>,
        cfg: ProxyConfig,
    ) -> Self {
        // No redirects (the pod's `Location` is the browser's business, rewritten by nobody),
        // no ambient proxy, and no overall timeout: a response may be a long-lived stream.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client");
        Self {
            inner: Arc::new(Inner {
                db,
                pdp,
                grants,
                sessions,
                pods,
                cfg,
                tokens: tokens::TokenBook::default(),
                http,
            }),
        }
    }
}

/// The router for the apps listener.
pub fn proxy_router(state: ProxyState) -> Router {
    Router::new()
        .route("/_ticket", get(relay::redeem))
        .fallback(relay::proxy)
        .with_state(state)
}
