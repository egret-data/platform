//! The relay: ticket redemption, then every request of an iframe, HTTP and WebSocket.
//!
//! The order of the checks is the design. Each one fails closed, and the pod hears nothing
//! of a request until all of them have passed:
//!
//! 1. the raw path is unambiguous ([`super::pathguard`]) — 400 otherwise;
//! 2. it is `/e/{ref}/…` or `/r/{ref}/…` — 404 otherwise;
//! 3. the apps cookie names a live grant (login session unexpired, user active) — 401;
//! 4. the grant is of this surface (an edit grant on `/e`, a run grant on `/r`) — 401;
//! 5. `{ref}` is the one that grant is entitled to: the caller's own edit pod, or the version
//!    the run grant pins — 403;
//! 6. the route is on the allow-list for the surface ([`super::policy`]) — 403;
//! 7. edit only: a request that NAMES a notebook file (`?file=`) names an app the caller may
//!    write right now — 400 / 403 / 404.
//!
//! Only then is a pod found and a token minted, the path forwarded **as received**, and the
//! response filtered.

use axum::body::Body;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message as AxMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, REFERRER_POLICY, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use egret_core::Error;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as TgMessage;
use uuid::Uuid;

use crate::grant::Grant;
use crate::podapi::require_app_write;
use crate::proxy::headers::{
    app_id_of_file, named_file, response_headers, upstream_headers, Inject,
};
use crate::proxy::pathguard::check_raw_path;
use crate::proxy::policy::{classify, Surface, Verdict};
use crate::proxy::{PodEndpoint, ProxyState};
use crate::session::AppMode;

/// The apps-origin cookie. A `__Host-` name is impossible here (it forbids a `Path`, and the
/// path is what keeps an edit tab and a run tab from overwriting each other); `__Secure-` is
/// the strongest prefix that allows one.
fn cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Secure-egret_apps_grant"
    } else {
        "egret_apps_grant"
    }
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    (status, msg).into_response()
}

fn from_error(e: Error) -> Response {
    match e {
        Error::Unauthorized => plain(StatusCode::UNAUTHORIZED, "sign in again"),
        Error::Forbidden => plain(StatusCode::FORBIDDEN, "not allowed"),
        Error::NotFound(_) => plain(StatusCode::NOT_FOUND, "not found"),
        Error::FeatureDisabled(_) => plain(StatusCode::FORBIDDEN, "not allowed"),
        _ => plain(StatusCode::BAD_GATEWAY, "the app is unavailable"),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct TicketQuery {
    t: Option<String>,
}

/// `GET /_ticket?t=…`: spend the ticket, set the cookie, go to the app.
pub(super) async fn redeem(State(st): State<ProxyState>, Query(q): Query<TicketQuery>) -> Response {
    let Some(ticket) = q.t else {
        return plain(StatusCode::BAD_REQUEST, "no ticket");
    };
    let redeemed = match st
        .inner
        .grants
        .redeem(&ticket, st.inner.cfg.cookie_ttl)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return plain(
                StatusCode::FORBIDDEN,
                "this link has expired; reopen the app",
            )
        }
        Err(e) => return from_error(e),
    };
    let (grant, cookie_value) = redeemed;
    let (scope, location) = match grant.mode {
        AppMode::Edit => {
            let r = st.inner.pods.edit_ref(grant.user_id, grant.workspace_id);
            (
                format!("/e/{r}/"),
                format!("/e/{r}/?file={}.py", grant.app_id),
            )
        }
        AppMode::Run => {
            let v = grant.version_id.map(|v| v.hyphenated().to_string());
            let scope = format!("/r/{}/", v.unwrap_or_default());
            (scope.clone(), scope)
        }
    };
    let cookie = Cookie::build((cookie_name(st.inner.cfg.secure), cookie_value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(st.inner.cfg.secure)
        .path(scope)
        .max_age(time::Duration::seconds(
            st.inner.cfg.cookie_ttl.as_secs() as i64
        ))
        .build();
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::FOUND;
    let h = resp.headers_mut();
    h.insert(
        LOCATION,
        HeaderValue::from_str(&location).expect("ascii location"),
    );
    h.insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie.to_string()).expect("ascii cookie"),
    );
    // The ticket is in the URL: keep it out of caches and out of any Referer.
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    resp
}

/// What the checks in the module doc establish.
struct Admitted {
    grant: Grant,
    surface: Surface,
    verdict: Verdict,
    /// The `{ref}` segment, as the client wrote it.
    pod_ref: String,
    /// The notebook file the request named and was authorized for.
    file: Option<String>,
}

pub(super) async fn proxy(
    State(st): State<ProxyState>,
    // Not an `Option`: axum's optional extraction is not implemented for `WebSocketUpgrade`.
    // A request that is not an upgrade is an ordinary HTTP request, hence `.ok()`.
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    req: Request,
) -> Response {
    match handle(&st, ws.ok(), req).await {
        Ok(r) | Err(r) => r,
    }
}

async fn handle(
    st: &ProxyState,
    ws: Option<WebSocketUpgrade>,
    req: Request,
) -> Result<Response, Response> {
    let raw_path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    let method = if ws.is_some() {
        "WEBSOCKET".to_string()
    } else {
        req.method().as_str().to_string()
    };

    let admitted = admit(st, req.headers(), &raw_path, query.as_deref(), &method).await?;

    // The edit page without a file would show marimo's file picker; go to the grant's draft.
    if admitted.surface == Surface::Edit
        && admitted.file.is_none()
        && method == "GET"
        && admitted.verdict == Verdict::Allow
        && is_edit_root(&raw_path)
    {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::FOUND;
        resp.headers_mut().insert(
            LOCATION,
            HeaderValue::from_str(&format!(
                "/e/{}/?file={}.py",
                admitted.pod_ref, admitted.grant.app_id
            ))
            .map_err(|_| plain(StatusCode::BAD_REQUEST, "bad pod reference"))?,
        );
        return Ok(resp);
    }

    let pod = match admitted.surface {
        Surface::Edit => {
            st.inner
                .pods
                .edit_pod(admitted.grant.user_id, admitted.grant.workspace_id)
                .await
        }
        Surface::Run => match admitted.grant.version_id {
            Some(v) => st.inner.pods.run_pod(admitted.grant.app_id, v).await,
            None => Err(Error::NotFound("version".into())),
        },
    }
    .map_err(from_error)?;

    // A static file needs no identity, so it costs no token.
    let token = if admitted.verdict == Verdict::Allow {
        Some(
            st.inner
                .tokens
                .token_for(
                    &admitted.grant,
                    &st.inner.sessions,
                    st.inner.cfg.session_idle,
                    st.inner.cfg.session_absolute,
                )
                .await
                .map_err(from_error)?,
        )
    } else {
        None
    };
    let headers = upstream_headers(
        req.headers(),
        &Inject {
            app_token: token.as_deref(),
            marimo_token: &pod.marimo_token,
            pinned_notebook: admitted.file.as_deref(),
        },
    );
    // The path and query exactly as the client sent them: the thing that was checked.
    let path_and_query = req
        .uri()
        .path_and_query()
        .map_or(raw_path.as_str(), |p| p.as_str())
        .to_string();

    match ws {
        Some(upgrade) => relay_ws(st, upgrade, &pod, &path_and_query, headers, &admitted).await,
        None => relay_http(st, req, &pod, &path_and_query, headers).await,
    }
}

fn is_edit_root(raw_path: &str) -> bool {
    // `/e/{ref}` or `/e/{ref}/`, nothing after.
    let mut segs = raw_path.split('/').skip(1);
    let _ = segs.next();
    let _ = segs.next();
    matches!(segs.next(), None | Some("")) && segs.next().is_none()
}

async fn admit(
    st: &ProxyState,
    headers: &HeaderMap,
    raw_path: &str,
    query: Option<&str>,
    method: &str,
) -> Result<Admitted, Response> {
    // 1–2
    let decoded = check_raw_path(raw_path)
        .map_err(|_| plain(StatusCode::BAD_REQUEST, "ambiguous path rejected"))?;
    let mut segs = decoded.split('/').skip(1);
    let surface = match segs.next() {
        Some("e") => Surface::Edit,
        Some("r") => Surface::Run,
        _ => return Err(plain(StatusCode::NOT_FOUND, "not found")),
    };
    let pod_ref_decoded = match segs.next() {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => return Err(plain(StatusCode::NOT_FOUND, "not found")),
    };
    let rest: Vec<&str> = segs.collect();
    let rel = if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", rest.join("/"))
    };
    // The reference as written (not lower-cased): same segment of the raw path.
    let pod_ref = raw_path.split('/').nth(2).unwrap_or_default().to_string();

    // 3
    let jar = CookieJar::from_headers(headers);
    let cookie = jar
        .get(cookie_name(st.inner.cfg.secure))
        .map(|c| c.value().to_string())
        .ok_or_else(|| plain(StatusCode::UNAUTHORIZED, "open the app from egret"))?;
    let grant = st
        .inner
        .grants
        .resolve_cookie(&cookie)
        .await
        .map_err(from_error)?
        .ok_or_else(|| plain(StatusCode::UNAUTHORIZED, "open the app from egret"))?;

    // 4
    let grant_surface = match grant.mode {
        AppMode::Edit => Surface::Edit,
        AppMode::Run => Surface::Run,
    };
    if grant_surface != surface {
        return Err(plain(StatusCode::UNAUTHORIZED, "open the app from egret"));
    }

    // 5
    let entitled = match surface {
        Surface::Edit => st.inner.pods.edit_ref(grant.user_id, grant.workspace_id),
        Surface::Run => grant
            .version_id
            .map(|v| v.hyphenated().to_string())
            .unwrap_or_default(),
    };
    if pod_ref_decoded != entitled.to_ascii_lowercase() {
        return Err(plain(StatusCode::FORBIDDEN, "not your app"));
    }

    // 6
    let verdict = classify(surface, method, &rel);
    if verdict == Verdict::Deny {
        return Err(plain(StatusCode::FORBIDDEN, "blocked by egret"));
    }

    // 7
    let mut file = None;
    if surface == Surface::Edit {
        let named = named_file(query)
            .map_err(|()| plain(StatusCode::BAD_REQUEST, "one notebook file at a time"))?;
        if let Some(f) = named {
            let app = app_id_of_file(&f)
                .ok_or_else(|| plain(StatusCode::BAD_REQUEST, "not an app file"))?;
            require_app_write(
                &st.inner.db,
                &*st.inner.pdp,
                grant.user_id,
                grant.workspace_id,
                app,
            )
            .await
            .map_err(from_error)?;
            file = Some(f);
        }
    }
    Ok(Admitted {
        grant,
        surface,
        verdict,
        pod_ref,
        file,
    })
}

fn upstream_url(pod: &PodEndpoint, path_and_query: &str) -> Option<url::Url> {
    let base = pod.base.as_str().trim_end_matches('/');
    url::Url::parse(&format!("{base}{path_and_query}")).ok()
}

fn unavailable() -> Response {
    plain(StatusCode::BAD_GATEWAY, "the app is unavailable")
}

async fn relay_http(
    st: &ProxyState,
    req: Request,
    pod: &PodEndpoint,
    path_and_query: &str,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let url = upstream_url(pod, path_and_query).ok_or_else(unavailable)?;
    let method = req.method().clone();
    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());
    let upstream = st
        .inner
        .http
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "apps proxy: pod request failed");
            plain(StatusCode::BAD_GATEWAY, "the app is unavailable")
        })?;
    let status = upstream.status();
    let out_headers = response_headers(upstream.headers(), &st.inner.cfg.egret_origin);
    let mut resp = Response::new(Body::from_stream(upstream.bytes_stream()));
    *resp.status_mut() = status;
    *resp.headers_mut() = out_headers;
    Ok(resp)
}

/// Decrements the grant's WebSocket count when the relay ends — or when the upgrade never
/// happens and the closure holding this is dropped — and revokes its tokens at zero.
struct WsGuard {
    st: ProxyState,
    grant: Uuid,
}

impl Drop for WsGuard {
    fn drop(&mut self) {
        if self.st.inner.tokens.ws_closed(self.grant) {
            let sessions = self.st.inner.sessions.clone();
            let grant = self.grant;
            tokio::spawn(async move {
                if let Err(e) = sessions.revoke_for_grant(grant).await {
                    tracing::warn!(error = %e, "apps proxy: revoking a grant's tokens failed");
                }
            });
        }
    }
}

async fn relay_ws(
    st: &ProxyState,
    upgrade: WebSocketUpgrade,
    pod: &PodEndpoint,
    path_and_query: &str,
    headers: HeaderMap,
    admitted: &Admitted,
) -> Result<Response, Response> {
    let mut url = upstream_url(pod, path_and_query).ok_or_else(unavailable)?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|()| plain(StatusCode::BAD_GATEWAY, "the app is unavailable"))?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| plain(StatusCode::BAD_GATEWAY, "the app is unavailable"))?;
    for (k, v) in &headers {
        request.headers_mut().insert(k.clone(), v.clone());
    }
    let (upstream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "apps proxy: pod WebSocket failed");
            plain(StatusCode::BAD_GATEWAY, "the app is unavailable")
        })?;
    st.inner.tokens.ws_opened(admitted.grant.id);
    let guard = WsGuard {
        st: st.clone(),
        grant: admitted.grant.id,
    };
    Ok(upgrade.on_upgrade(move |socket| async move {
        pump(socket, upstream).await;
        drop(guard);
    }))
}

async fn pump(
    client: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (mut c_tx, mut c_rx) = client.split();
    let (mut u_tx, mut u_rx) = upstream.split();
    loop {
        tokio::select! {
            m = c_rx.next() => match m {
                Some(Ok(msg)) => {
                    if let Some(out) = ax_to_tung(msg) {
                        if u_tx.send(out).await.is_err() { break; }
                    }
                }
                _ => {
                    let _ = u_tx.send(TgMessage::Close(None)).await;
                    break;
                }
            },
            m = u_rx.next() => match m {
                Some(Ok(msg)) => {
                    if let Some(out) = tung_to_ax(msg) {
                        if c_tx.send(out).await.is_err() { break; }
                    }
                }
                _ => {
                    let _ = c_tx.send(AxMessage::Close(None)).await;
                    break;
                }
            },
        }
    }
}

fn ax_to_tung(m: AxMessage) -> Option<TgMessage> {
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    Some(match m {
        AxMessage::Text(t) => TgMessage::Text(t.as_str().to_owned().into()),
        AxMessage::Binary(b) => TgMessage::Binary(b.to_vec().into()),
        AxMessage::Ping(b) => TgMessage::Ping(b.to_vec().into()),
        AxMessage::Pong(b) => TgMessage::Pong(b.to_vec().into()),
        AxMessage::Close(Some(cf)) => TgMessage::Close(Some(CloseFrame {
            code: CloseCode::from(cf.code),
            reason: cf.reason.as_str().to_owned().into(),
        })),
        AxMessage::Close(None) => TgMessage::Close(None),
    })
}

fn tung_to_ax(m: TgMessage) -> Option<AxMessage> {
    use axum::extract::ws::CloseFrame;
    Some(match m {
        TgMessage::Text(t) => AxMessage::Text(t.as_str().to_owned().into()),
        TgMessage::Binary(b) => AxMessage::Binary(b.to_vec().into()),
        TgMessage::Ping(b) => AxMessage::Ping(b.to_vec().into()),
        TgMessage::Pong(b) => AxMessage::Pong(b.to_vec().into()),
        TgMessage::Close(Some(cf)) => AxMessage::Close(Some(CloseFrame {
            code: u16::from(cf.code),
            reason: cf.reason.as_str().to_owned().into(),
        })),
        TgMessage::Close(None) => AxMessage::Close(None),
        TgMessage::Frame(_) => return None,
    })
}
