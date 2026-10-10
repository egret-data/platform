//! The workflow webhook group must NOT carry the CSRF layer — asserted through
//! the **composed** application router.
//!
//! # Why this file exists at all
//!
//! `egret-workflow`'s own `tests/webhook_route.rs` nests `webhook_router()`
//! directly. A test written that way **cannot see a CSRF failure**, because it
//! never mounts the layer: all 22 of those tests pass identically whether or not
//! `build_router` wraps the group. That is exactly the shape `render_router`
//! shipped in (`crates/egret-automation/src/routes.rs:115-126`: *"the
//! integration tests call this router directly, so the layer they never mount
//! rejected every POST in production"*), and it is why
//! `dataapi_serving_mount.rs` next door exists for the identical group.
//!
//! It matters more here than it did there. A webhook caller is a machine that
//! types the credential into the URL; it has no cookie jar, so a double-submit
//! CSRF cookie is not merely inconvenient for it but **impossible**, and the
//! surface would be 100% broken in production while its own suite stayed green.
//!
//! `AppState::without_db()` sets `workflow: None`, so before this file no test
//! had ever built the composed router with either workflow group merged.
//! `AppState::with_workflow` was added for that reason.
//!
//! # No database, and it still bites
//!
//! The state is built with `Db::connect_lazy` and nothing here completes a
//! query. The three outcomes a POST to the webhook path can produce are already
//! distinguishable without one:
//!
//! * **403** `csrf check failed` — the middleware rejected it and the handler
//!   never ran. This is the failure the file exists to catch.
//! * **404** — nothing is mounted at that path at all, which would make a
//!   "not 403" assertion pass vacuously. This is also what a *routed* request
//!   would answer if it got as far as "no such workspace", so 404 alone is not
//!   evidence either way and the file never rests on it.
//! * **500** — the handler ran and its FIRST step, `workspaces.get_by_slug`,
//!   failed against a pool with nothing behind it. That is the evidence: it can
//!   only be produced by code downstream of where the layer would have rejected
//!   the request.
//!
//! The full POST → 200 path needs a database and is covered by
//! `egret-workflow`'s `a_fired_webhook_stores_the_trigger_record`. What only the
//! composed router can prove is the layering, and 500-from-the-handler proves
//! it. This file therefore asserts on every run, with no `DATABASE_URL` and no
//! skip.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// A DENY-everything PDP.
///
/// Every test here is about MOUNTING, and none of them needs an authorization
/// call to succeed. Deny is the stub that keeps it that way: an allow-all one
/// would let a mount test start depending on an authorization outcome without
/// anyone noticing. The webhook path never consults it at all — it has no
/// subject to consult it about.
struct DenyPdp;

#[async_trait::async_trait]
impl Pdp for DenyPdp {
    async fn authorize(
        &self,
        _s: &Subject,
        _a: &str,
        _r: &Resource,
    ) -> egret_core::Result<Decision> {
        Ok(Decision {
            allow: false,
            reasons: vec![],
        })
    }
}

/// A `WorkflowRefPort` that must never be consulted: nothing in this file
/// publishes a flow.
struct UnreachableRefs;

#[async_trait::async_trait]
impl egret_workflow::WorkflowRefPort for UnreachableRefs {
    async fn workspace_of(
        &self,
        _kind: egret_workflow::RefKind,
        _name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error> {
        unreachable!("a mount test must never resolve a reference")
    }
}

fn composed_app_with_workflow() -> axum::Router {
    // Lazy: the pool is never connected. A query against it FAILS, which is
    // precisely the signal this file reads.
    //
    // **The short `acquire_timeout` is load-bearing, not tidiness.**
    // `Db::connect_lazy` takes sqlx's 30 s default, and the webhook handler's
    // very first step is a database call -- so every test in this file would
    // spend thirty seconds waiting to be told what it already knows.
    // `dataapi_serving_bounds.rs` made the same choice for the same reason.
    // `Db { pool }` is public, so the options can be chosen here.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(250))
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .expect("lazy pool");
    let db = Db { pool };
    let state = egret_workflow::WorkflowState {
        db: db.clone(),
        repo: egret_workflow::FlowRepo::new(db.clone()),
        executions: egret_workflow::ExecutionRepo::new(db.clone()),
        logs: egret_workflow::logs::LogRepo::new(db.pool.clone()),
        refs: Arc::new(UnreachableRefs),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: Arc::new(DenyPdp),
        audit: egret_audit::AuditSink::new(16).0,
        scripts: Default::default(),
        upload_idle_timeout: egret_workflow::routes::UPLOAD_IDLE_TIMEOUT,
        upload_total_timeout: egret_workflow::routes::UPLOAD_TOTAL_TIMEOUT,
        export_total_timeout: egret_workflow::routes::EXPORT_TOTAL_TIMEOUT,
    };
    build_router(
        AppState::without_db()
            .with_workflow(state)
            .with_workflow_webhooks(),
    )
}

/// The same state with the exposure lever left at its default.
fn composed_app_with_workflow_webhooks_off() -> axum::Router {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(250))
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .expect("lazy pool");
    let db = Db { pool };
    let state = egret_workflow::WorkflowState {
        db: db.clone(),
        repo: egret_workflow::FlowRepo::new(db.clone()),
        executions: egret_workflow::ExecutionRepo::new(db.clone()),
        logs: egret_workflow::logs::LogRepo::new(db.pool.clone()),
        refs: Arc::new(UnreachableRefs),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: Arc::new(DenyPdp),
        audit: egret_audit::AuditSink::new(16).0,
        scripts: Default::default(),
        upload_idle_timeout: egret_workflow::routes::UPLOAD_IDLE_TIMEOUT,
        upload_total_timeout: egret_workflow::routes::UPLOAD_TOTAL_TIMEOUT,
        export_total_timeout: egret_workflow::routes::EXPORT_TOTAL_TIMEOUT,
    };
    build_router(AppState::without_db().with_workflow(state))
}

async fn body_text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn webhook_uri() -> String {
    format!(
        "/workflows/v1/{}/executions/webhook/fx.hooks/fx-flow/fx-key",
        Uuid::new_v4()
    )
}

/// The property: a POST with **no cookie at all** must reach the handler.
#[tokio::test]
async fn a_webhook_post_needs_no_csrf_cookie() {
    // Built through egret_server::build_router, NOT webhook_router() — a direct
    // call never mounts the CSRF layer and so cannot fail this.
    let app = composed_app_with_workflow();

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(webhook_uri())
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let body = body_text(res).await;

    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "the webhook group is inside the CSRF layer: a machine caller has no cookie jar, \
         so every POST to it would be rejected in production. Body: {body}"
    );
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "expected the handler's OWN first step (workspaces.get_by_slug against a pool with \
         nothing behind it). A 404 here would mean nothing is mounted at this path at all, \
         which would make the assertion above vacuous. Got {status}: {body}"
    );
}

/// `PUT` and `GET` reach the same handler through the composed router.
///
/// Kestra routes five methods to one handler; egret routes three. A router that
/// mounted only `POST` would leave the other two answering 405, and no test in
/// `egret-workflow` could tell that apart from its own direct mount.
#[tokio::test]
async fn put_and_get_reach_the_same_handler() {
    for method in ["PUT", "GET"] {
        let app = composed_app_with_workflow();
        let res = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(webhook_uri())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_text(res).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method} did not reach the handler (405 would mean the method is not routed, \
             404 that the path is not). Got {status}: {body}"
        );
    }
}

/// The anti-vacuity control for this whole file: with `workflow: None` the path
/// is **not mounted**, and the request 404s.
///
/// Without it, every assertion above would still hold for a router that had
/// mounted something else entirely at that path — and the file would be
/// asserting a property of the 404 handler.
#[tokio::test]
async fn the_path_is_not_mounted_when_the_seam_is_absent() {
    let app = build_router(AppState::without_db());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(webhook_uri())
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "with workflow: None nothing must answer at the webhook path -- this is what makes \
         the 500s above evidence that the handler ran"
    );
}

/// The **management** group, by contrast, is session-authenticated and browser
/// facing, so it must stay INSIDE the CSRF layer.
///
/// Kept in this file rather than a third one because the two groups are merged
/// by the same `if let Some(workflow)` block: a change that moved the webhook
/// group inside the layer would most likely move the management group outside
/// it, and a file asserting only one direction would not see that.
#[tokio::test]
async fn the_management_group_does_carry_csrf() {
    let app = composed_app_with_workflow();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/ws/{}/workflows/flows", Uuid::new_v4()))
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let body = body_text(res).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a cookie-less POST to the management router must be stopped by the CSRF layer \
         before it reaches anything. Got {status}: {body}"
    );
}

/// **The exposure lever, and its default.**
///
/// Fix round 2. The webhook group was mounted unconditionally, so the security
/// review's "not approved for exposure" had nothing behind it: an operator who
/// agreed with the verdict could not act on it. `workflow_webhooks_enabled` is
/// that lever, it defaults to **false**, and `egret-bin` reads
/// `[workflow] webhooks_enabled` — `EGRET_WORKFLOW__WEBHOOKS_ENABLED` — through
/// `egret_config::Config`. (It named the round-2 spelling, without the section
/// separator, until fix round 4.)
///
/// Three rows, because a default is only a default if the OTHER value is also
/// asserted and if the management surface is shown to be unaffected.
#[tokio::test]
async fn the_webhook_surface_is_off_unless_switched_on() {
    // 1. The default: the workflow seam is present and the webhook path is NOT
    //    mounted, so it 404s like any unrouted URL rather than being
    //    mounted-and-refusing.
    let app = composed_app_with_workflow_webhooks_off();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(webhook_uri())
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let off_status = res.status();

    // **Asserted against a path that has never existed, not against a literal.**
    //
    // Measured: with the lever off this answers **403**, not 404, and so does
    // `/no-such-surface/at/all` — the composed router's CSRF layer carries a
    // fallback that catches every unrouted POST. The first draft of this test
    // asserted 404 and was wrong about the mechanism. What matters is not the
    // number but that the two are **the same**: with the lever off the webhook
    // path is indistinguishable from a URL nobody ever wrote.
    let app = composed_app_with_workflow_webhooks_off();
    let unrouted = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/no-such-surface/at/all")
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        off_status,
        unrouted.status(),
        "with the lever off the webhook path must be indistinguishable from a path that has \
         never existed"
    );

    // 2. The control: the SAME state with the lever on reaches the handler.
    //    Without this row, row 1 is satisfied by a router that mounts nothing
    //    at all -- which is exactly what `the_path_is_not_mounted_when_the_seam_
    //    is_absent` already covers for a different reason.
    let app = composed_app_with_workflow();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(webhook_uri())
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_ne!(
        off_status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "the two answers must differ, or row 1 proves nothing about reachability"
    );

    // 3. The lever gates ONLY the webhook group: management is still mounted
    //    with the lever off, and still behind CSRF.
    let app = composed_app_with_workflow_webhooks_off();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/ws/{}/workflows/flows", Uuid::new_v4()))
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "turning webhooks off must not turn flow authoring off"
    );
}
