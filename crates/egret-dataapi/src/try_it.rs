//! Data API P5 §3 — running an endpoint as a service account, from the console.
//!
//! The serving surface accepts a service-account bearer token and nothing else —
//! never a session cookie — so a browser cannot call a published endpoint
//! directly. This route is how an author runs their own endpoint AS one of the
//! workspace's service accounts and sees exactly what that account would get.
//!
//! # It drives the REAL serving router
//!
//! [`crate::serving_router`] is nested into a throwaway `Router` and called with
//! `tower::ServiceExt::oneshot`, exactly as the P4 test suites drive it. Same
//! router, same layers, same handler a consumer traverses.
//!
//! **Not a re-implemented equivalent, and not an HTTP loopback.** A loopback
//! would need the server's own URL and TLS. A re-implementation is worse than
//! either: P4.5 repeatedly showed a simulated path staying green while the real
//! one failed, and a Try-it that says OK while consumers get 403 converts the one
//! tool that could have found the problem into the reason nobody looked.
//!
//! `tests/try_it.rs` holds that to CI the only way it can be held: mutate
//! `serving::router::serve` to answer a different status and
//! `a_trial_without_a_grant_gets_the_real_403` must go red. It does — the check
//! was run, with `IM_A_TEAPOT`, before this module was believed.
//!
//! # It bypasses nothing
//!
//! Rate limit, cache, Ranger, the invoke grant and the access log all apply for
//! real. Three consequences the UI must state rather than let someone discover:
//! a trial spends rate-limit budget; a trial writes to `access_log` (hence
//! `actor_user_id`); and on a `cache.scope: shared` endpoint a trial can populate
//! the cache real consumers read.
//!
//! **Which rate-limit budget, precisely**, because the answer follows from
//! [`trial_client_id`] below rather than from anything in this sentence: the
//! endpoint-wide bucket is the consumers' own and a trial spends it, so a trial
//! can exhaust it and can be refused 429 by it. The per-client bucket is keyed on
//! the token's `sub`, which for a trial is a reserved marker rather than any
//! issued client — so a trial cannot throttle a live consumer's own allowance,
//! and repeated trials do throttle each other.
//!
//! # The token is minted here and never leaves the process
//!
//! 60 seconds of life, one in-process call, and the response echoes
//! `Bearer ••••` with a `$TOKEN` placeholder in the `curl` reproduction. No JWT
//! substring reaches the browser, the audit detail, or any log line this module
//! writes.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::http::{ApiError, ErrorBody};
use egret_core::{Error, WorkspacePath};
use egret_workspace::context::WsContext;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;

use crate::routes::{scoped_account, DataApiError, DataApiState, EP_MANAGE};

/// The trial ran, and its response was larger than [`MAX_TRIAL_BODY`].
///
/// **A `413`, and it needs its own type to be one.** `egret_core::Error` — which
/// `DataApiError` aliases — has no payload-too-large variant, so the nearest
/// reachable status was `409 Conflict`. That is the wrong semantic (nothing
/// conflicts; a response was too big) and, worse, it collided with this surface's
/// OTHER 409, `openapi_gen`'s `NamingCollision` — leaving one status meaning two
/// unrelated things on adjacent routes of the same feature.
///
/// Rendered through [`ErrorBody`], the same envelope
/// `impl IntoResponse for egret_core::Error` produces, so the shape a client
/// parses does not change with the status.
#[derive(Debug)]
pub struct TrialResultTooLarge(String);

impl IntoResponse for TrialResultTooLarge {
    fn into_response(self) -> Response {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorBody {
                error: self.0,
                code: None,
            }),
        )
            .into_response()
    }
}

/// [`try_endpoint`]'s error: the crate's canonical [`Error`] for everything it
/// shares with its siblings, plus the one failure this route alone can have.
pub type TryItError = ApiError<TrialResultTooLarge>;

/// How long a minted trial token lives.
///
/// Long enough for one in-process call, short enough that a leak through a crash
/// dump is spent before it can be read. Deliberately far below
/// [`crate::DATA_API_TTL_SECS`] (600s), which is a real client's budget for a
/// round trip over the internet; this one never leaves the process.
const TRIAL_TOKEN_TTL_SECS: i64 = 60;

/// The `sub` a trial token carries, as a prefix.
///
/// See [`trial_client_id`] for why a trial does not borrow a real client id.
const TRIAL_CLIENT_PREFIX: &str = "trial:";

/// The largest response body a trial will read back from the serving router.
///
/// A published `limits.max_rows` may be up to a million rows, and the console
/// cannot display that however it is delivered. Over the cap the trial fails
/// with a message naming the cap, rather than reading an arbitrary amount of
/// memory into a management response. See [`TrialResultTooLarge`] for the status
/// that failure carries and why it is not a 409.
const MAX_TRIAL_BODY: usize = 4 * 1024 * 1024;

/// A trial is bounded by [`crate::serving::SERVING_REQUEST_DEADLINE`], the
/// consumer's number, and it does **not** get one of its own.
///
/// # Because Try it answers "can a consumer call this endpoint?"
///
/// That is the question the feature exists for and the one its copy promises.
/// A trial that survived where a consumer is cut off would answer a different
/// question and answer it reassuringly — the exact class of lie this surface
/// keeps removing, and worse than no Try it at all, because it is the one tool
/// an author would have used to find the problem.
///
/// So there is deliberately no `TRIAL_REQUEST_DEADLINE` constant. A second
/// number that must equal the first is a drift waiting to happen; passing the
/// consumer's constant makes the parity a property of the code rather than of
/// two comments agreeing.
///
/// # The argument for a longer trial deadline, and why it fails
///
/// It is real as far as it goes: the deadline's derivation is the edge a
/// consumer crosses (ingress-nginx gives `/data-api` 60 s), and the trial call
/// itself crosses no edge — [`try_endpoint`] drives the router with
/// `tower::ServiceExt::oneshot`, in process, with no socket between the halves.
/// A trial deadline of 120 s was tried on that reasoning.
///
/// Two things kill it. First, the trial **request** — `POST …/try` from the
/// console — is an ordinary browser call, and when `ingress.enabled` it crosses
/// the `egret` Ingress object's `proxy-read-timeout: "60"`, which covers every
/// path but `/catalog` (that one has its own object, at 120 s)
/// (`deploy/helm/egret/templates/ingress.yaml`). So behind an ingress a 120 s
/// trial deadline is unreachable: nginx answers the author with a generic
/// gateway error at 60 s instead of egret answering with its own envelope and a
/// code. Second, and independently of any deployment: a trial that completes at
/// 90 s tells the author their endpoint works, and a consumer calling it gets
/// cut off. The bound being *shared* is what makes the answer true.
///
/// # What that means for a legal-but-slow endpoint, and where it is said
///
/// `limits.timeout_seconds` defaults to 30 and `MAX_STATEMENTS` allows 16, so
/// the canonical several-statement endpoint — a `rows` query and the `total`
/// that paginates it (spec §4.4) — has a 60 s worst case at the defaults, which
/// is past this deadline and past the ingress. That is legal, and publishing it
/// is not refused. It is a fact about the **configuration**, so it is surfaced
/// where the author sets the configuration — the Config surface warns as soon as
/// `statements × timeout_seconds` exceeds the serving deadline — rather than
/// arriving as an unexplained `504` at the moment a trial dies.
/// How many trials may be in flight at once, **process-wide**.
///
/// # Why this is a constant and a `static`, not a `permits` argument
///
/// [`try_endpoint`] builds its serving router per trial. A bulkhead built with
/// that router would hold `TRIAL_MAX_CONCURRENCY` permits *per request*, so the
/// cap could never be reached by any number of concurrent trials — a limit that
/// reads like a wall in review and is not one. The semaphore therefore lives in
/// [`trial_bulkhead`], is created once, and is passed to
/// [`crate::serving::router::serving_router_sharing_bulkhead`].
///
/// # Why a quarter of the serving surface's 64
///
/// The two bulkheads are separate — a burst of trials must not spend the machine
/// surface's admission slots — but they are **not** independent underneath: both
/// draw on the one metastore pool, whose default is 10 connections
/// (`egret_config`'s `database.max_connections`), and a trial is strictly the
/// more expensive of the two: it resolves the workspace, mints an RSA-signed
/// token and emits an audit event around the same handler.
///
/// So this is derived the same way `SERVING_MAX_CONCURRENCY` is, and then given
/// the smaller share. 16 is above the pool, which keeps the pool — not this
/// layer — the thing that paces ordinary trials, exactly as the serving
/// bulkhead intends; and it is a quarter of 64, so with both bulkheads full the
/// machine surface still holds the clear majority of the process. A trial is
/// human-initiated from an authenticated console session holding [`EP_MANAGE`];
/// there is no consumer traffic pattern behind it, and the console must not be
/// able to starve the surface it exists to test.
pub const TRIAL_MAX_CONCURRENCY: usize = crate::serving::SERVING_MAX_CONCURRENCY / 4;

/// The trial bulkhead, built once for the process.
///
/// `OnceLock` rather than a field on `DataApiState` because the bound is on the
/// *process* — the tokio runtime and the metastore pool that trials consume are
/// process-wide, and a per-state semaphore would multiply the cap by however
/// many states a binary happens to construct.
fn trial_bulkhead() -> Arc<Semaphore> {
    static BULKHEAD: OnceLock<Arc<Semaphore>> = OnceLock::new();
    BULKHEAD
        .get_or_init(|| crate::serving::guard::serving_bulkhead(TRIAL_MAX_CONCURRENCY))
        .clone()
}

/// The serving router a trial runs against: the real one, with the consumer's
/// deadline and body cap, and the trial path's own bulkhead.
///
/// Exposed so the bounds can be tested through the same function the handler
/// calls. A test that hand-assembled an equivalent router would be asserting
/// against its own copy of the wiring, which is the shape
/// [`try_endpoint`]'s module comment refuses for the handler and refuses here
/// for the same reason.
pub fn trial_serving_router() -> axum::Router<DataApiState> {
    crate::serving::router::serving_router_sharing_bulkhead(
        // The consumer's deadline, not a trial-shaped one. See the block above
        // the bulkhead constant for why there is no second number here.
        crate::serving::SERVING_REQUEST_DEADLINE,
        trial_bulkhead(),
        // The body cap is inherited unchanged and is **unreachable** on this
        // path: the request below is built with `Body::empty()` and no
        // `Content-Length`, so nothing a trial sends can cross it. Kept rather
        // than removed so the trial router stays the serving router with two
        // numbers changed, not a third shape.
        crate::serving::SERVING_MAX_BODY_BYTES,
    )
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TryRequest {
    pub service_account_id: Uuid,
    /// Values exactly as they would arrive on the query string. A `Vec` per key
    /// because a repeated parameter is a LIST (P4.5), and a trial that could
    /// only send one value could not exercise the case most likely to be wrong.
    #[serde(default)]
    pub params: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TryResponse {
    /// The status the SERVING path returned. The trial itself is a 200 whenever
    /// it ran — a 403 from the endpoint is a result, not a failure of the trial.
    pub status: u16,
    /// The serving path's own body, verbatim: on success the spec §4.4 envelope
    /// `{status, message, data, meta}` — `data` an array for a one-statement
    /// endpoint and an object keyed by statement name for several — or, for an
    /// endpoint published with `output.envelope: "bare"`, `data`'s value on its
    /// own. On a refusal, spec §7's `{code, message, request_id}`. Verbatim
    /// because the point of the feature is to show what the consumer gets, and a
    /// body this route summarised would be a different answer than the real
    /// one — including about which envelope the endpoint is published in.
    #[schema(value_type = Object)]
    pub body: serde_json::Value,
    pub request: TryRequestEcho,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TryRequestEcho {
    pub method: String,
    pub url: String,
    /// Always the literal `Bearer ••••`. The token is never serialised.
    pub authorization: String,
    /// A copyable reproduction using a `$TOKEN` placeholder.
    pub curl: String,
}

/// Marker inserted into the request the management handler builds, carrying the
/// human who ran the trial.
///
/// **An extension, not a header and not a field on the token.** A header could
/// be sent by any consumer, and the serving path would then be trusting the
/// caller's word about whether their own call was a trial. Only an in-process
/// handler can insert an extension, so `actor_user_id` cannot be forged from
/// outside — which is what makes filtering trials out of statistics a fact
/// rather than a convention.
#[derive(Clone, Copy)]
pub struct TrialActor(pub egret_core::ids::UserId);

/// The `sub` claim a trial token carries.
///
/// # Why not one of the account's real client ids
///
/// `sub` becomes [`crate::ServingPrincipal::client_id`], which the serving path
/// puts in `access_log.client_id` and keys the per-client rate-limit bucket on.
/// Borrowing an issued client's id would make both of those untrue in ways that
/// matter: the access log would name a credential that did not make the call
/// (and, since a trial is attributable through `actor_user_id`, would do so
/// while looking exactly like consumer traffic to anything reading `client_id`),
/// and an author clicking Try-it would spend a live consumer's own allowance —
/// throttling production from a console button.
///
/// # Why not mint a client either
///
/// Minting one would make Try-it a credential-creating action: a
/// `dataapi.clients` row whose secret is discarded but which is a real,
/// exchangeable credential for as long as it exists. A console preview must not
/// widen the credential surface.
///
/// # So: a reserved marker, and it is honest
///
/// `trial:<service account id>` says exactly what happened — no client made this
/// call — and cannot collide with an issued id, which `ClientStore::create`
/// always shapes as `egret-dataapi-<uuid>`. It is also uniform: an account with
/// no clients yet is the normal case for an endpoint being authored, and a Try-it
/// whose behaviour changed once a credential existed would be a Try-it whose
/// answer depended on something the trial is not about.
///
/// Nothing outside this process can produce this `sub`: the machine-IdP always
/// sets `sub` to the verified `client_id`, so a consumer cannot present a token
/// claiming to be a trial.
fn trial_client_id(service_account_id: Uuid) -> String {
    format!("{TRIAL_CLIENT_PREFIX}{service_account_id}")
}

/// Sign a short-lived `data-api` token for `service_account_id`.
///
/// # The claim shape is [`crate::verify_serving_token`]'s, field for field
///
/// Read from the VERIFIER, not from a test fixture's helper:
///
/// * `exp` — must be present and in the future. Absent is refused as malformed,
///   not treated as immortal.
/// * `aud` — must equal [`crate::DATA_API_AUDIENCE`]. The const, never the
///   literal, for the same reason `ClientStore::verify` uses it: mint and verify
///   cannot drift apart if there is one definition.
/// * `ws` — a uuid string; becomes the principal's workspace, which the serving
///   path then requires the URL's slug to agree with.
/// * `egret_user_id` — a uuid string; the principal every downstream
///   authorization, Ranger filter and delegated query runs as.
/// * `sub` — a non-blank string; becomes `client_id`. See [`trial_client_id`].
///
/// `iss` is NOT checked by the verifier, and is stamped anyway: every token this
/// deployment signs carries it, and a trial token that omitted it would be the
/// one shape an operator inspecting a captured JWT could not place.
///
/// Signed with `keys.active()`, the same key `verify_serving_token` verifies
/// against and the same one `POST /oauth/token` mints with.
pub(crate) async fn mint_trial_token(
    st: &DataApiState,
    workspace_id: Uuid,
    service_account_id: Uuid,
) -> Result<String, DataApiError> {
    let km = st
        .keys
        .active()
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("idp signing key: {e}")))?;
    egret_machine_idp::sign_claims(
        &km,
        &serde_json::json!({
            // The ISSUER, deliberately — a claim, not an address. It must stay
            // byte-equal to what every other token this deployment signs
            // carries; `consumer_base_url` (used for the echoed URL below) is a
            // different audience and would make this token's `iss` disagree
            // with the rest of the system the moment the two differ.
            "iss": st.issuer.trim_end_matches('/'),
            "sub": trial_client_id(service_account_id),
            "aud": crate::serving::DATA_API_AUDIENCE,
            "exp": chrono::Utc::now().timestamp() + TRIAL_TOKEN_TTL_SECS,
            "ws": workspace_id.to_string(),
            "egret_user_id": service_account_id.to_string(),
        }),
    )
    .map_err(|e| Error::Internal(anyhow::anyhow!("sign trial token: {e}")))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/try",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = TryRequest,
    responses(
        (status = 200, description = "The trial RAN. `status` and `body` are the serving path's own answer — a 403 or a 429 in there is a result, not a failure of this route", body = TryResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage — a trial runs a query as somebody else, so :read does not reach it", body = ErrorBody),
        (status = 404, description = "No such endpoint, or no such service account, in this workspace", body = ErrorBody),
        (status = 413, description = "The trial RAN, but its result is larger than a console trial will read back; lower the endpoint's row limit or call it with a client", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub async fn try_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(body): Json<TryRequest>,
) -> Result<Json<TryResponse>, TryItError> {
    // `:manage`, not `:read` — this runs a query as somebody else.
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // 404s an endpoint outside this workspace before anything is minted.
    let endpoint = st.endpoints.get(ctx.workspace_id.0, id).await?;
    // Same containment for the account named in the BODY: an id from another
    // workspace — or an id that is a user but not a service account — must not
    // become a token. Without it, one request body mints a `data-api`
    // credential for an arbitrary principal.
    let account = scoped_account(&st, ctx.workspace_id.0, body.service_account_id).await?;

    // The slug comes from `ctx.workspace_id`, which `WsContext` resolved and
    // authorized against — never from a second read of the `{workspace}` path
    // segment. Gating on one and building the URL from the other is exactly the
    // shape this module's siblings avoid (see `routes`' invariant 2).
    let slug = st
        .workspaces
        .get_by_id(ctx.workspace_id)
        .await?
        .ok_or_else(|| Error::NotFound("workspace".into()))?
        .slug;

    let token = mint_trial_token(&st, ctx.workspace_id.0, account).await?;

    // `url::form_urlencoded`, the same crate `serving::params` decodes with, so
    // what this encodes is what that decodes. Every value of a key is appended,
    // because a repeated parameter is a LIST on the serving path and the trial
    // has to be able to produce one.
    //
    // Scoped so the `Serializer` is dropped before the `await` below: it holds an
    // `Option<&dyn Fn(&str) -> Cow<[u8]>>` custom-encoder slot and is therefore
    // NOT `Send`, and a value alive across an await makes the whole handler's
    // future non-`Send` — which axum reports only as "this is not a `Handler`".
    let query = {
        let mut qs = url::form_urlencoded::Serializer::new(String::new());
        for (k, values) in &body.params {
            for v in values {
                qs.append_pair(k, v);
            }
        }
        qs.finish()
    };
    // A trial calls the LIVE version, so it calls the URL the live version is
    // served at — not the draft's path, which may be a move not yet published
    // (KI-023). An endpoint never published has no live route; its draft URL
    // then reaches the same 404 any consumer would get.
    let method = endpoint.live_method.as_deref().unwrap_or(&endpoint.method);
    let served_path = endpoint.live_path.as_deref().unwrap_or(&endpoint.path);
    // The stored path is canonical (`/probe`); the wildcard capture must not
    // start with a second slash or the route matches an empty segment.
    let path = served_path.trim_start_matches('/');
    let uri = if query.is_empty() {
        format!("/data-api/v1/{slug}/{path}")
    } else {
        format!("/data-api/v1/{slug}/{path}?{query}")
    };

    // THE REAL ROUTER. Same nest, same layers, same handler a consumer reaches —
    // with the two bounds that are the trial path's rather than the consumer
    // path's: the deadline and the body cap are the CONSUMER's, deliberately,
    // so a trial cannot succeed where a consumer would be cut off; the bulkhead
    // is the trial's own, because a router built per trial cannot own one. See
    // the two blocks above [`TRIAL_MAX_CONCURRENCY`].
    let router = axum::Router::new()
        .nest("/data-api/v1", trial_serving_router())
        .with_state(st.clone());
    let req = Request::builder()
        .method(method)
        .uri(&uri)
        .header("authorization", format!("Bearer {token}"))
        .extension(TrialActor(ctx.principal.user_id))
        .body(Body::empty())
        .map_err(|e| Error::Internal(anyhow::anyhow!("build trial request: {e}")))?;

    let res = router
        .oneshot(req)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("serving router: {e}")))?;
    let status = res.status();

    // An impersonation action, audited like one. Precedent: user impersonation
    // (#332). The token is not in the detail and never will be — `tests/audit.rs`
    // asserts the crate-wide half of that.
    //
    // **Emitted HERE — after the call returned, before the body is read — and the
    // order is the property, not a style choice.** By this line the query has
    // already run as the service account: the impersonation is a fact. Every
    // remaining step can fail (the body can exceed [`MAX_TRIAL_BODY`], which a
    // published `max_rows` of up to a million makes reachable in normal use), and
    // with the emit below them a trial that really happened would leave no audit
    // record at all. Nothing in the detail is derived from the body, so there is
    // nothing to wait for. `access_log` would still carry the row — but the two
    // trails are read by different people for different questions, and "was this
    // endpoint ever run as that account" must be answerable from the audit log
    // alone.
    //
    // Anything added below this line must therefore stay incapable of skipping
    // it: keep the emit above every `?`.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:try",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "service_account_id": account,
            "status": status.as_u16(),
            "path": served_path,
            "method": method,
        }),
    ));

    let bytes = axum::body::to_bytes(res.into_body(), MAX_TRIAL_BODY)
        .await
        .map_err(|_| {
            TryItError::Domain(TrialResultTooLarge(format!(
                "the endpoint returned more than {} MiB; lower its row limit or call it \
                 with a client",
                MAX_TRIAL_BODY / (1024 * 1024)
            )))
        })?;
    // `Null` rather than a failure: the serving surface answers JSON on every
    // path it owns, so an unparseable body is worth showing the author as-is
    // rather than replacing their result with a 500 about our own parser.
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);

    // The CONSUMER-facing origin, not the issuer: this URL and the `curl` line
    // below are shown to the AUTHOR in the browser, and an in-cluster Service
    // DNS name is a line they cannot paste into a terminal and run. (The trial
    // itself never goes over the network — it drives the serving router
    // in-process — so this string is documentation, and documentation that
    // names an unreachable host is worse than none.)
    let origin = st.consumer_base_url.trim_end_matches('/');
    Ok(Json(TryResponse {
        status: status.as_u16(),
        body: parsed,
        request: TryRequestEcho {
            method: method.to_owned(),
            url: format!("{origin}{uri}"),
            // Never the token. Not truncated, not prefixed — absent.
            authorization: "Bearer ••••".into(),
            curl: format!("curl -H 'Authorization: Bearer $TOKEN' '{origin}{uri}'"),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker cannot collide with an id `ClientStore::create` produces, and
    /// it names the account it stands for. Both halves matter: a collision would
    /// merge a trial's rate-limit bucket with a real consumer's, and an opaque
    /// marker would leave `access_log.client_id` unattributable.
    #[test]
    fn a_trial_marker_is_not_shaped_like_an_issued_client_id() {
        let sa = Uuid::new_v4();
        let marker = trial_client_id(sa);
        assert_eq!(marker, format!("trial:{sa}"));
        assert!(!marker.starts_with("egret-dataapi-"));
        // Non-blank, or `verify_serving_token` refuses the token outright.
        assert!(!marker.trim().is_empty());
    }
}
