//! SA2 task 2: the Catalog `ClientVerifier` honours the service-account kill
//! switch on BOTH `verify` and `current` (the IdP revalidates a cache hit via
//! `current`). Every refusal is `Ok(None)` — no distinguishing error.
mod common;

use egret_catalog_engine::{CatalogStore, NewClient};
use egret_machine_idp::ClientVerifier;
use uuid::Uuid;

struct Seeded {
    store: CatalogStore,
    sa: Uuid,
    client_id: String,
    secret: String,
}

async fn seed() -> Option<Seeded> {
    let store = common::store().await?;
    let ws = common::seed_workspace(&store).await;
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("cv-store-{}", Uuid::new_v4().simple()))
    .fetch_one(store.pool())
    .await
    .unwrap();
    let wh = store
        .create_warehouse(&format!("cv-wh-{}", Uuid::new_v4().simple()), ws, sc_id)
        .await
        .unwrap();
    let role = store
        .create_role(wh.id, "engine", "engine", None)
        .await
        .unwrap();
    let sa = common::seed_service_account(&store, ws).await;
    let created = store
        .create_client(&NewClient {
            service_account_id: sa,
            name: "cv".into(),
            consumer_type: "engine".into(),
            workspace_id: ws,
            warehouse_id: wh.id,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    Some(Seeded {
        store,
        sa,
        client_id: created.client_id,
        secret: created.client_secret,
    })
}

async fn assert_refused(s: &Seeded, why: &str) {
    let v = s.store.verify(&s.client_id, &s.secret).await.unwrap();
    assert!(v.is_none(), "verify must refuse: {why}");
    let c = s.store.current(&s.client_id).await.unwrap();
    assert!(c.is_none(), "current must refuse: {why}");
}

async fn exec(s: &Seeded, sql: &'static str) {
    sqlx::query(sql)
        .bind(&s.client_id)
        .bind(s.sa)
        .execute(s.store.pool())
        .await
        .unwrap();
}

macro_rules! seeded {
    () => {
        match seed().await {
            Some(s) => s,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

#[tokio::test]
async fn active_owned_client_verifies_and_is_current() {
    let s = seeded!();
    assert!(s
        .store
        .verify(&s.client_id, &s.secret)
        .await
        .unwrap()
        .is_some());
    assert!(s.store.current(&s.client_id).await.unwrap().is_some());
}

#[tokio::test]
async fn disabled_owner_is_refused_by_verify_and_current() {
    let s = seeded!();
    exec(
        &s,
        "UPDATE users SET status = 'disabled' WHERE id = $2 AND $1::text IS NOT NULL",
    )
    .await;
    assert_refused(&s, "owner disabled").await;
}

#[tokio::test]
async fn revoked_client_is_refused() {
    let s = seeded!();
    exec(
        &s,
        "UPDATE catalog.catalog_clients SET revoked_at = now() WHERE client_id = $1 AND $2::uuid IS NOT NULL",
    )
    .await;
    assert_refused(&s, "revoked").await;
}

#[tokio::test]
async fn expired_client_is_refused() {
    let s = seeded!();
    exec(
        &s,
        "UPDATE catalog.catalog_clients SET expires_at = now() - interval '1 hour' \
         WHERE client_id = $1 AND $2::uuid IS NOT NULL",
    )
    .await;
    assert_refused(&s, "expired").await;
}

#[tokio::test]
async fn unowned_client_is_refused_fail_closed() {
    let s = seeded!();
    exec(
        &s,
        "UPDATE catalog.catalog_clients SET service_account_id = NULL \
         WHERE client_id = $1 AND $2::uuid IS NOT NULL",
    )
    .await;
    assert_refused(&s, "no owning service account").await;
}
