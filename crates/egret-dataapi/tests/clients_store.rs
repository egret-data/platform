//! Every failure mode of `verify` must be indistinguishable from outside.
//!
//! DB-gated integration tests. Skip (pass) when DATABASE_URL is unset so the
//! suite stays green offline; CI / `just up` provides Postgres. Fixture shape
//! copied from `crates/egret-query/tests/registry_repo.rs`.
use egret_core::UserId;
use egret_dataapi::ClientStore;
use egret_machine_idp::ClientVerifier;
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    clients: ClientStore,
    /// The service account's `users.id` — also `dataapi.service_accounts.user_id`.
    sa: Uuid,
    /// The workspace the service account (and its clients) belong to.
    ws: Uuid,
    /// A human admin user, recorded as `created_by` on the client.
    admin: UserId,
}

async fn fixture() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let admin_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1,'admin','local','active','x') RETURNING id",
    )
    .bind(format!("admin_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("admin user");
    let admin = UserId(admin_id);

    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(admin_id)
    .fetch_one(&db.pool)
    .await
    .expect("workspace");

    // A service account is a real `users` row (provider = 'service', no
    // password) with a `dataapi.service_accounts` row binding it to a workspace.
    let sa: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status) \
         VALUES ($1,'svc','service','active') RETURNING id",
    )
    .bind(format!("svc_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("service account user");
    sqlx::query(
        "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, created_by) \
         VALUES ($1, $2, 'svc', $3)",
    )
    .bind(sa)
    .bind(ws)
    .bind(admin_id)
    .execute(&db.pool)
    .await
    .expect("service_accounts row");

    let clients = ClientStore::new(db.clone());
    Some(Fixture {
        db,
        clients,
        sa,
        ws,
        admin,
    })
}

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

#[tokio::test]
async fn a_new_client_verifies_and_carries_its_workspace() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();

    let vc = f
        .clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .expect("must verify");

    assert_eq!(vc.audience, "data-api");
    assert_eq!(vc.ttl_secs, 600, "short TTL is the revocation mitigation");
    assert_eq!(vc.extra_claims["ws"], f.ws.to_string());
    assert_eq!(vc.extra_claims["egret_user_id"], f.sa.to_string());
    assert!(
        vc.extra_claims.get("warehouse").is_none(),
        "no catalog claims on a data-api token"
    );
}

#[tokio::test]
async fn a_revoked_client_does_not_verify() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    f.clients.revoke(c.id).await.unwrap();
    assert!(f
        .clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn an_expired_client_does_not_verify() {
    let f = skip_without_db!();
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let c = f
        .clients
        .create(f.sa, "prod", Some(past), f.admin)
        .await
        .unwrap();
    assert!(f
        .clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_client_with_a_future_expiry_verifies() {
    // Positive-direction pin on `offset_to_chrono`: every other test either
    // passes `expires_at: None` or a PAST instant. Nothing here would catch a
    // conversion bug that shifts a future expiry into the past (wrong epoch
    // unit, a sign flip, `from_timestamp_nanos` vs `from_timestamp`) — that
    // regression would still make `an_expired_client_does_not_verify` pass,
    // since "wrongly expired" and "expired" look identical from `verify`.
    let f = skip_without_db!();
    let future = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    let c = f
        .clients
        .create(f.sa, "prod", Some(future), f.admin)
        .await
        .unwrap();

    let vc = f
        .clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .expect("a client with a future expires_at must still verify");

    assert_eq!(vc.audience, "data-api");
    assert_eq!(vc.ttl_secs, 600);
    assert_eq!(vc.extra_claims["ws"], f.ws.to_string());
    assert_eq!(vc.extra_claims["egret_user_id"], f.sa.to_string());
}

#[tokio::test]
async fn a_wrong_secret_does_not_verify() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert!(f
        .clients
        .verify(&c.client_id, "not-the-secret")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn an_unknown_client_id_does_not_verify() {
    let f = skip_without_db!();
    assert!(f.clients.verify("nope", "s").await.unwrap().is_none());
}

#[tokio::test]
async fn deleting_the_service_account_removes_its_clients() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(f.sa)
        .execute(&f.db.pool)
        .await
        .unwrap();
    // ON DELETE CASCADE reaches dataapi.clients through service_accounts.
    assert!(f
        .clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .is_none());
}
