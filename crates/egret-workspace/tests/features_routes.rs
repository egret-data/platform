//! Tasks 8/9: the control-plane feature-entitlements API
//! (`GET`/`PUT /api/admin/workspaces/{id}/features`) plus the `features` field
//! `GET /api/me/workspaces` returns to tell the frontend which modules a
//! workspace has. DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS, mirroring the
//! sibling `workspace_db.rs`/`access_requests_routes.rs` harnesses (own
//! `connect`/`uniq`/`seed_user`/`ws_state`/`login_cookie` copies — each
//! integration-test binary in this crate is self-contained).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{admin_workspace_router, me_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    repo.insert_local(
        &format!("{}@test.local", uuid::Uuid::new_v4()),
        "U",
        &hash_password("pw").unwrap(),
    )
    .await
    .unwrap()
}

async fn build_pdp(db: &Db) -> std::sync::Arc<dyn egret_core::authz::Pdp> {
    std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn ws_state_with_audit(db: &Db, audit: egret_audit::AuditSink) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
        access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
            db.clone(),
            MemberRepo::new(db.clone()),
        ),
        authz: Authorizer::new(db.clone()),
        pdp: build_pdp(db).await,
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        audit,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
        session_secure: false,
    }
}

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

/// Harness for the feature-entitlements HTTP surface: a built app combining
/// `admin_workspace_router` and `me_router`, a logged-in caller, and a
/// workspace to exercise. Kept alongside the audit-sink receiver so emitted
/// `Mutation` events aren't dropped before a test can drain them (mirrors
/// `egret-controlplane/tests/routes_api.rs`'s `Fixture::audit_rx`).
struct Fixture {
    app: axum::Router,
    cookie: String,
    ws_id: Uuid,
    audit_rx: tokio::sync::mpsc::Receiver<egret_audit::AuditEvent>,
}

impl Fixture {
    async fn build(actor_is_platform_admin: bool) -> Option<Self> {
        let db = connect().await?;
        let actor = seed_user(&db).await;
        let (audit, audit_rx) = egret_audit::AuditSink::new(16);
        let st = ws_state_with_audit(&db, audit).await;
        if actor_is_platform_admin {
            st.authz
                .replace_platform_roles(actor, &["platform_admin".into()])
                .await
                .unwrap();
        }
        let cookie = login_cookie(actor, &st.sessions).await;
        // Owned by a separate user so the plain-user 403 case isn't accidentally
        // passing because the actor happens to own the workspace.
        let creator = seed_user(&db).await;
        let ws = st
            .workspaces
            .create(&uniq("feat-ws"), "Feature WS", creator)
            .await
            .unwrap();
        let app = axum::Router::new()
            .nest("/admin", admin_workspace_router())
            .nest("/me", me_router())
            .with_state(st);
        Some(Fixture {
            app,
            cookie,
            ws_id: ws.id.0,
            audit_rx,
        })
    }

    /// A platform_admin actor, which is what holds `workspace:read`/`workspace:update`.
    async fn new() -> Option<Self> {
        Self::build(true).await
    }

    /// A plain authenticated user with no platform role at all.
    async fn new_as_plain_user() -> Option<Self> {
        Self::build(false).await
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> axum::response::Response {
        let b = match body {
            Some(s) => Body::from(s.to_string()),
            None => Body::empty(),
        };
        let mut builder = HttpRequest::builder()
            .method(method)
            .uri(path)
            .header("cookie", &self.cookie);
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        self.app
            .clone()
            .oneshot(builder.body(b).unwrap())
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> axum::response::Response {
        self.request("GET", path, None).await
    }

    async fn put(&self, path: &str, body: &str) -> axum::response::Response {
        self.request("PUT", path, Some(body)).await
    }

    async fn post(&self, path: &str, body: &str) -> axum::response::Response {
        self.request("POST", path, Some(body)).await
    }

    async fn json(&self, resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[tokio::test]
async fn list_returns_every_feature_enabled_by_default() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let resp = f
        .get(&format!("/admin/workspaces/{}/features", f.ws_id))
        .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let json = f.json(resp).await;
    let arr = json.as_array().expect("array");
    // Against the catalog rather than a literal count: the point of this
    // assertion is "the route lists every module", which a hard-coded number
    // turns into "…and someone remembered to bump it" every time one is added.
    assert_eq!(arr.len(), egret_workspace::features::FEATURE_KEYS.len());
    // Every key is on by default EXCEPT the ones whose `default_enabled` is
    // false (`workflow_containers`, W5a) -- read from the catalog, not listed.
    for item in arr {
        let key = item["key"].as_str().unwrap();
        assert_eq!(
            item["enabled"],
            egret_workspace::features::default_enabled(key).unwrap(),
            "{key}"
        );
    }
    assert!(arr
        .iter()
        .any(|f| f["key"] == "workflow_containers" && f["enabled"] == false));
    assert!(arr
        .iter()
        .all(|f| f["label"].is_string() && f["description"].is_string()));
}

#[tokio::test]
async fn put_disables_a_feature_and_list_reflects_it() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"apps":false}}"#,
        )
        .await;
    assert_eq!(put.status(), StatusCode::OK);

    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    let nb = json
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "apps")
        .unwrap();
    assert_eq!(nb["enabled"], false);
}

#[tokio::test]
async fn put_rejects_an_unknown_key_and_writes_nothing() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"apps":false,"teleporter":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::BAD_REQUEST);
    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    // Every key at its catalog default (W5a: `workflow_containers` is off).
    assert!(json.as_array().unwrap().iter().all(|x| x["enabled"]
        == egret_workspace::features::default_enabled(x["key"].as_str().unwrap()).unwrap()));
}

/// `PUT` is a merge, not a full replacement, despite the verb: a body naming
/// only `apps` must leave a previously-disabled `datasets` disabled. This
/// pins the behaviour deliberately (see `SetFeatures`'s doc comment for the
/// forward-compatibility reasoning) — without this test a future refactor
/// that "fixed" `set_features` into resetting omitted keys would silently
/// flip the semantics back.
///
/// Uses `datasets`/`apps` rather than `charts` deliberately: neither
/// participates in the `dashboards`-requires-`charts` dependency, so this
/// test stays about merge semantics and not about that rule (which has its
/// own dedicated tests below).
#[tokio::test]
async fn put_merges_leaving_omitted_keys_untouched() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let disable_datasets = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"datasets":false}}"#,
        )
        .await;
    assert_eq!(disable_datasets.status(), StatusCode::OK);

    // A second PUT naming only a DIFFERENT key.
    let disable_apps = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"apps":false}}"#,
        )
        .await;
    assert_eq!(disable_apps.status(), StatusCode::OK);

    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    let arr = json.as_array().unwrap();
    let datasets = arr.iter().find(|x| x["key"] == "datasets").unwrap();
    let apps = arr.iter().find(|x| x["key"] == "apps").unwrap();
    assert_eq!(
        datasets["enabled"], false,
        "datasets must remain disabled: it was never mentioned in the second PUT"
    );
    assert_eq!(
        apps["enabled"], false,
        "apps was disabled by the second PUT"
    );
}

/// `dashboards` requires `charts`: turning both off together in one `PUT` is a
/// fine, consistent resulting state.
#[tokio::test]
async fn put_turning_off_both_dashboards_and_charts_together_is_ok() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"dashboards":false,"charts":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::OK);
    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    let arr = json.as_array().unwrap();
    assert_eq!(
        arr.iter().find(|x| x["key"] == "dashboards").unwrap()["enabled"],
        false
    );
    assert_eq!(
        arr.iter().find(|x| x["key"] == "charts").unwrap()["enabled"],
        false
    );
}

/// The straightforward violation: naming both keys in one body, with
/// `dashboards` on and `charts` off.
#[tokio::test]
async fn put_rejects_dashboards_on_with_charts_off_and_writes_nothing() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"dashboards":true,"charts":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::BAD_REQUEST);
    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    // Both are enabled by default, and default is untouched by the rejected
    // write.
    // Every key at its catalog default (W5a: `workflow_containers` is off).
    assert!(json.as_array().unwrap().iter().all(|x| x["enabled"]
        == egret_workspace::features::default_enabled(x["key"].as_str().unwrap()).unwrap()));
}

/// The merge trap: `dashboards` is currently ON (the default). A `PUT` naming
/// only `charts` — never mentioning `dashboards` at all — must still be
/// rejected, because the *resulting* state (dashboards on, charts off) is the
/// broken one. This is the case a body-only validation would miss.
#[tokio::test]
async fn put_rejects_charts_off_alone_when_dashboards_is_currently_on() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"charts":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::BAD_REQUEST);
    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    // Nothing was written: charts remains enabled (the prior state), not the
    // rejected `false`.
    assert_eq!(
        json.as_array()
            .unwrap()
            .iter()
            .find(|x| x["key"] == "charts")
            .unwrap()["enabled"],
        true,
        "the rejected write must not have landed"
    );
}

/// Same workspace, same starting state (`dashboards` on) as the merge-trap
/// test above, but this time both keys are named together, turning both off:
/// the resulting state is consistent, so it must succeed.
#[tokio::test]
async fn put_turning_off_charts_and_dashboards_together_succeeds_from_dashboards_on_state() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"charts":false,"dashboards":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::OK);
    let json = f
        .json(
            f.get(&format!("/admin/workspaces/{}/features", f.ws_id))
                .await,
        )
        .await;
    let arr = json.as_array().unwrap();
    assert_eq!(
        arr.iter().find(|x| x["key"] == "charts").unwrap()["enabled"],
        false
    );
    assert_eq!(
        arr.iter().find(|x| x["key"] == "dashboards").unwrap()["enabled"],
        false
    );
}

#[tokio::test]
async fn a_user_without_workspace_update_cannot_toggle() {
    let Some(f) = Fixture::new_as_plain_user().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"apps":false}}"#,
        )
        .await;

    assert_eq!(put.status(), StatusCode::FORBIDDEN);
}

/// The write is audited: a `Mutation` event naming the actor lands in the
/// audit channel, carrying before/after detail. The audit sink's `emit` is a
/// synchronous `try_send`, so with the receiver held open (unlike the
/// `workspace_db.rs`/other fixtures here, which drop it) the event is
/// available immediately after the request returns — same technique as
/// `egret-controlplane/tests/routes_api.rs`'s `audit_rx.try_recv()`.
#[tokio::test]
async fn put_features_emits_an_audit_event_naming_the_actor() {
    let Some(mut f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    let put = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"apps":false}}"#,
        )
        .await;
    assert_eq!(put.status(), StatusCode::OK);

    let ev = f
        .audit_rx
        .try_recv()
        .expect("a Mutation event must be queued after toggling a feature");
    match ev {
        egret_audit::AuditEvent::Mutation {
            actor,
            impersonator: _,
            workspace_id,
            action,
            resource_type,
            resource_id,
            detail,
        } => {
            assert!(actor.is_some(), "actor must be recorded");
            assert_eq!(workspace_id, Some(f.ws_id), "workspace_id");
            assert_eq!(action, "workspace.features.update", "action");
            assert_eq!(resource_type, "workspace", "resource_type");
            assert_eq!(
                resource_id.as_deref(),
                Some(f.ws_id.to_string().as_str()),
                "resource_id is the workspace id"
            );
            let after: Vec<String> = detail["after"]
                .as_array()
                .expect("detail.after array")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert!(
                after.contains(&"apps".to_string()),
                "detail.after must record apps as disabled; detail={detail}"
            );
        }
        other => panic!("expected a Mutation event, got {other:?}"),
    }
}

#[tokio::test]
async fn my_workspaces_reports_enabled_features() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };
    f.put(
        &format!("/admin/workspaces/{}/features", f.ws_id),
        r#"{"features":{"apps":false}}"#,
    )
    .await;

    let json = f.json(f.get("/me/workspaces").await).await;
    let ws = json
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == f.ws_id.to_string())
        .unwrap();

    let features: Vec<&str> = ws["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(features.contains(&"charts"));
    assert!(!features.contains(&"apps"));
    // W5a: the off-by-default key is absent from a workspace with no row.
    assert!(!features.contains(&"workflow_containers"), "{features:?}");
    assert!(features.contains(&"workflows"), "{features:?}");
}

/// Two workspaces with different disabled sets: `/me/workspaces` must report
/// each one's own `features` correctly, which the batched
/// `disabled_features_for` lookup keys by workspace id. This does not
/// directly observe query *count* (no query-log instrumentation is wired into
/// this fixture) — it proves correctness of the batched per-workspace
/// grouping, which is the part that would silently break if the batching were
/// done wrong (e.g. accidentally unioning all workspaces' disabled sets).
///
/// Uses `datasets` (not `charts`) as ws2's disabled feature deliberately: with
/// `dashboards` on by default, disabling `charts` alone would trip the
/// `dashboards`-requires-`charts` dependency, which is out of scope for this
/// test.
#[tokio::test]
async fn my_workspaces_reports_each_workspaces_own_disabled_set_when_batched() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip");
        return;
    };

    // Disable `apps` on the first (fixture) workspace.
    let put1 = f
        .put(
            &format!("/admin/workspaces/{}/features", f.ws_id),
            r#"{"features":{"apps":false}}"#,
        )
        .await;
    assert_eq!(put1.status(), StatusCode::OK);

    // A second workspace with a DIFFERENT feature disabled.
    let slug2 = uniq("feat-ws2");
    let created = f
        .json(
            f.post(
                "/admin/workspaces",
                &format!(r#"{{"slug":"{slug2}","name":"Feature WS 2"}}"#),
            )
            .await,
        )
        .await;
    let ws2_id = created["id"].as_str().unwrap().to_string();
    let put2 = f
        .put(
            &format!("/admin/workspaces/{ws2_id}/features"),
            r#"{"features":{"datasets":false}}"#,
        )
        .await;
    assert_eq!(put2.status(), StatusCode::OK);

    let json = f.json(f.get("/me/workspaces").await).await;
    let arr = json.as_array().unwrap();

    let ws1 = arr.iter().find(|w| w["id"] == f.ws_id.to_string()).unwrap();
    let ws1_features: Vec<&str> = ws1["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!ws1_features.contains(&"apps"));
    assert!(ws1_features.contains(&"datasets"));

    let ws2 = arr.iter().find(|w| w["id"] == ws2_id).unwrap();
    let ws2_features: Vec<&str> = ws2["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!ws2_features.contains(&"datasets"));
    assert!(ws2_features.contains(&"apps"));
}

/// KI-231: `/me/workspaces` tells the UI which page-gating permissions the
/// caller holds in each workspace, so the sidebar can leave out Endpoints,
/// Clients, Workflows and the admin-only settings sections for a viewer. The
/// answer must be the PDP's, per workspace: a viewer holds none of them, an
/// admin of another workspace holds all of them THERE and still none here.
#[tokio::test]
async fn my_workspaces_reports_the_page_permissions_the_caller_holds_in_each_workspace() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (audit, _audit_rx) = egret_audit::AuditSink::new(16);
    let st = ws_state_with_audit(&db, audit).await;
    let creator = seed_user(&db).await;
    let caller = seed_user(&db).await;
    let viewed = st
        .workspaces
        .create(&uniq("perm-view"), "Viewed", creator)
        .await
        .unwrap();
    let administered = st
        .workspaces
        .create(&uniq("perm-admin"), "Administered", creator)
        .await
        .unwrap();
    st.members
        .add(viewed.id, "user", caller.0, "ws_viewer", creator)
        .await
        .unwrap();
    st.members
        .add(administered.id, "user", caller.0, "ws_admin", creator)
        .await
        .unwrap();
    let cookie = login_cookie(caller, &st.sessions).await;
    let app = axum::Router::new().nest("/me", me_router()).with_state(st);

    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri("/me/workspaces")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let perms_of = |id: Uuid| -> Vec<String> {
        let ws = json
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id.to_string())
            .unwrap_or_else(|| panic!("workspace {id} missing from {json}"));
        let mut v: Vec<String> = ws["permissions"]
            .as_array()
            .unwrap_or_else(|| panic!("no permissions array on {ws}"))
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        v.sort();
        v
    };

    assert_eq!(perms_of(viewed.id.0), Vec::<String>::new());
    let mut every: Vec<String> = egret_workspace::routes::PAGE_PERMISSIONS
        .iter()
        .map(|p| p.to_string())
        .collect();
    every.sort();
    assert_eq!(perms_of(administered.id.0), every);
}
