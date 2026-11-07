//! Boot repair (spec `docs/en/specs/2026-10-31-local-user-username.md`,
//! decision 4): every human user still without a username is allocated one
//! through the steward's allocation core — the ledger mint and the stale-ref
//! guard — and a service account is left as it is.
//!
//! Its own test binary, so `egret-test-db` gives it a private database: the
//! repair reads every user, and a concurrent test elsewhere that seeds a
//! user with no username on purpose must not see it minted.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_metastore::Db;
use egret_perm_compile::{mint_missing_usernames, UsernameRepair};
use uuid::Uuid;

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

async fn insert_user(pool: &sqlx::PgPool, provider: &str, email: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ($1, $2, 'Test') RETURNING id",
    )
    .bind(provider)
    .bind(email)
    .fetch_one(pool)
    .await
    .expect("insert user")
}

async fn username(pool: &sqlx::PgPool, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read username")
}

/// A grant whose `user` include names `name`, written behind the write path's
/// back (the write path refuses a ref no user carries): the stale ref the
/// guard exists for.
async fn seed_stale_grant(pool: &sqlx::PgPool, name: &str) {
    let t = tag();
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(format!("br-ws-{t}"))
            .bind("BR WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-br-test','AKIATEST') RETURNING id",
    )
    .bind(format!("br-sc-{t}"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("br_wh_{t}"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(format!("br_cat_{t}"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    let grant: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id) \
         VALUES ('allow'::perm.effect, ARRAY['read'], $1) RETURNING id",
    )
    .bind(catalog_id)
    .fetch_one(pool)
    .await
    .expect("grant");
    sqlx::query(
        "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
         VALUES ($1, 'user'::perm.principal_kind, $2, 'include'::perm.principal_mode)",
    )
    .bind(grant)
    .bind(name)
    .execute(pool)
    .await
    .expect("grant principal");
}

#[tokio::test]
async fn names_every_human_user_without_one_and_leaves_service_accounts_and_guarded_names() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = &db.pool;
    // Converge whatever the template database holds, so the counts below are
    // this test's own (plus any user it already had to refuse).
    let base = mint_missing_usernames(pool).await.expect("first repair");

    let t = tag();
    let local = insert_user(pool, "local", &format!("br_local_{t}@egret.local")).await;
    let sso = insert_user(pool, "oidc", &format!("br_sso_{t}@idp.local")).await;
    let service = insert_user(pool, "service", &format!("br_svc_{t}@svc.local")).await;
    let guarded = insert_user(pool, "local", &format!("br_guarded_{t}@egret.local")).await;
    seed_stale_grant(pool, &format!("br_guarded_{t}")).await;

    let repair = mint_missing_usernames(pool).await.expect("repair");
    assert_eq!(
        repair,
        UsernameRepair {
            minted: 2,
            refused: base.refused + 1
        }
    );
    assert_eq!(username(pool, local).await, Some(format!("br_local_{t}")));
    assert_eq!(username(pool, sso).await, Some(format!("br_sso_{t}")));
    assert_eq!(
        username(pool, service).await,
        None,
        "service accounts unchanged"
    );
    assert_eq!(
        username(pool, guarded).await,
        None,
        "a name a stored ref already names is not allocated"
    );
    // Through the ledger, as every mint is.
    let ledgered: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM username_ledger WHERE username = $1::citext")
            .bind(format!("br_local_{t}"))
            .fetch_one(pool)
            .await
            .expect("ledger row");
    assert_eq!(ledgered, Some(local));

    // Idempotent: the next boot finds only the guarded user, and refuses again.
    let again = mint_missing_usernames(pool).await.expect("second repair");
    assert_eq!(
        again,
        UsernameRepair {
            minted: 0,
            refused: base.refused + 1
        }
    );
}
