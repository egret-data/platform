//! Rejected Ranger credentials are counted per engine (spec
//! 2026-10-20-ranger-surface-auth, A12), only for engines that exist, and a
//! later successful download clears them.
//!
//! DB-gated: self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`,
//! and otherwise FAIL (`egret_test_db::url`). Every test uses its own engine
//! ids, so the process-wide coalescing of the middleware never joins two tests.
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use egret_engine_mgmt_api::credential::{
    plugin_username, EngineCredentialVerifier, EngineIdentity,
};
use egret_metastore::Db;
use egret_policy::ranger::auth::AuthMode;
use egret_policy::{ranger_surface, seed_tag, seed_trino, AuthFailure, PolicyStore, RangerState};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

const PW: &str = "the-engine-password";

async fn setup() -> Option<(PolicyStore, sqlx::PgPool)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino");
    seed_tag(&store).await.expect("seed tag");
    Some((store, pool))
}

/// A trino profile and a provisioned engine bound to it: `(engine id, profile name)`.
async fn engine(store: &PolicyStore, pool: &sqlx::PgPool) -> (Uuid, String) {
    let name = format!("trino_{}", Uuid::new_v4().simple());
    let sid = store
        .create_service(&name, "trino", &json!({}))
        .await
        .expect("profile")
        .id;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{\"imageVersion\":\"483\"}'::jsonb, 'running', $2) \
         RETURNING id",
    )
    .bind(format!("{name}-eng"))
    .bind(sid)
    .fetch_one(pool)
    .await
    .expect("engine");
    (id, name)
}

async fn row(store: &PolicyStore, id: Uuid) -> Option<AuthFailure> {
    store
        .auth_failures()
        .await
        .expect("auth_failures")
        .into_iter()
        .find(|f| f.engine_id == id)
}

async fn raw_count(pool: &sqlx::PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM policy.ranger_auth_failures WHERE engine_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Wait up to 5 s for `cond` (the middleware records off the request path).
async fn eventually<F, Fut>(what: &str, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("not within 5 s: {what}");
}

struct Stub {
    good: Uuid,
    bound_to: Option<i64>,
}

#[async_trait::async_trait]
impl EngineCredentialVerifier for Stub {
    async fn verify(&self, id: Uuid, pw: &str) -> Result<Option<EngineIdentity>, String> {
        Ok((id == self.good && pw == PW).then_some(EngineIdentity {
            engine_id: id,
            authz_service_id: self.bound_to,
        }))
    }
}

fn basic(id: Uuid, pw: &str) -> String {
    use base64::Engine as _;
    let raw = format!("{}:{pw}", plugin_username(id));
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

fn download(svc: &str, auth: &str) -> Request<Body> {
    Request::builder()
        .uri(format!(
            "/ranger/service/plugins/secure/policies/download/{svc}"
        ))
        .header(header::AUTHORIZATION, auth)
        .body(Body::empty())
        .unwrap()
}

async fn sid_of(store: &PolicyStore, svc: &str) -> i64 {
    store.get_service(svc).await.unwrap().id
}

#[tokio::test]
async fn a_failure_upserts_and_counts() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (id, _) = engine(&store, &pool).await;
    assert!(store.record_auth_failure(id).await.unwrap());
    let first = row(&store, id).await.expect("a row after one failure");
    assert_eq!(first.count, 1);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(store.record_auth_failure(id).await.unwrap());
    let second = row(&store, id).await.unwrap();
    assert_eq!(second.count, 2);
    assert!(second.last_at > first.last_at, "{first:?} then {second:?}");
    assert_eq!(second.ok_at, None);
}

#[tokio::test]
async fn an_unknown_engine_id_is_not_counted() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip");
        return;
    };
    // Anyone can send `engine-<random uuid>`: it must not grow the table.
    let forged = Uuid::new_v4();
    assert!(!store.record_auth_failure(forged).await.unwrap());
    assert_eq!(raw_count(&pool, forged).await, 0);
}

#[tokio::test]
async fn a_download_after_the_failure_clears_it_and_a_later_failure_raises_it_again() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (id, _) = engine(&store, &pool).await;
    // A download with nothing outstanding writes no row.
    store.record_engine_download(id).await.unwrap();
    assert_eq!(raw_count(&pool, id).await, 0);

    store.record_auth_failure(id).await.unwrap();
    assert!(row(&store, id).await.unwrap().is_outstanding());
    tokio::time::sleep(Duration::from_millis(20)).await;
    store.record_engine_download(id).await.unwrap();
    let cleared = row(&store, id).await.unwrap();
    assert!(!cleared.is_outstanding(), "{cleared:?}");
    // The row is kept: the count is history, not state.
    assert_eq!(cleared.count, 1);

    tokio::time::sleep(Duration::from_millis(20)).await;
    store.record_auth_failure(id).await.unwrap();
    let again = row(&store, id).await.unwrap();
    assert!(again.is_outstanding(), "{again:?}");
    assert_eq!(again.count, 2);
}

#[tokio::test]
async fn deleting_the_engine_deletes_its_row() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (id, _) = engine(&store, &pool).await;
    store.record_auth_failure(id).await.unwrap();
    assert_eq!(raw_count(&pool, id).await, 1);
    sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(raw_count(&pool, id).await, 0);
}

#[tokio::test]
async fn the_middleware_counts_a_rejected_engine_credential_once_per_window() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (id, svc) = engine(&store, &pool).await;
    let bound = sid_of(&store, &svc).await;
    let app = ranger_surface(RangerState {
        store: store.clone(),
        auth_token: Arc::from("operator"),
        mode: AuthMode::Required,
        verifier: Some(Arc::new(Stub {
            good: id,
            bound_to: Some(bound),
        })),
    });
    // A flood of wrong passwords: every one is still a 401.
    for _ in 0..25 {
        let r = app
            .clone()
            .oneshot(download(&svc, &basic(id, "wrong")))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    eventually("the rejection is recorded", || async {
        raw_count(&pool, id).await == 1
    })
    .await;
    // Give any stray write time to land: the flood cost ONE upsert.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let f = row(&store, id).await.unwrap();
    assert_eq!(f.count, 1, "coalesced within the window: {f:?}");
    assert!(f.is_outstanding());

    // A forged engine id is answered the same and stores nothing.
    let forged = Uuid::new_v4();
    let r = app
        .clone()
        .oneshot(download(&svc, &basic(forged, PW)))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // The engine's next successful download clears the finding.
    let r = app.oneshot(download(&svc, &basic(id, PW))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    eventually("the download clears the rejection", || async {
        !row(&store, id).await.unwrap().is_outstanding()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(raw_count(&pool, forged).await, 0);
}
