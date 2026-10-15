//! Every failure mode of `verify` must be indistinguishable from outside.
//!
//! DB-gated integration tests. Skip ONLY when `EGRET_ALLOW_DB_SKIP` is set;
//! without it a run with no DATABASE_URL FAILS rather than going green having
//! asserted nothing. CI / `just up` provides Postgres. Fixture shape copied
//! from `crates/egret-query/tests/registry_repo.rs`.
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
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");

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
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
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
    let past = chrono::Utc::now() - chrono::Duration::hours(1);
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
    // Positive direction on `expires_at`: every other test either passes
    // `expires_at: None` or a PAST instant. Nothing there would catch a bug that
    // shifts a future expiry into the past — that regression would still make
    // `an_expired_client_does_not_verify` pass, since "wrongly expired" and
    // "expired" look identical from `verify`.
    let f = skip_without_db!();
    let future = chrono::Utc::now() + chrono::Duration::hours(1);
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

/// Spec §6.1 tells an operator that the way to stop a compromised client inside
/// the 600s token window is to disable the PRINCIPAL, since a stateless token
/// cannot be recalled. That procedure is only real if `verify` reads
/// `users.status` — otherwise the operator disables the account, believes the
/// credential is dead, and it keeps minting `data-api` tokens indefinitely.
///
/// Asserted as a before/after on the SAME client so the test cannot pass by the
/// client having been broken all along.
#[tokio::test]
async fn a_disabled_service_account_does_not_verify() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert!(
        f.clients
            .verify(&c.client_id, &c.client_secret)
            .await
            .unwrap()
            .is_some(),
        "must verify while the principal is active"
    );

    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(f.sa)
        .execute(&f.db.pool)
        .await
        .unwrap();

    assert!(
        f.clients
            .verify(&c.client_id, &c.client_secret)
            .await
            .unwrap()
            .is_none(),
        "disabling the principal must stop its clients authenticating"
    );
}

/// Archiving a workspace must stop its service accounts authenticating.
///
/// This is the worse half of the `users.status` story, not a variation on it.
/// Every management route reaches the Data API through `WsContext`, which 404s
/// on a workspace that is not active — so archiving REMOVES the only routes that
/// could revoke a client or disable its principal. If `verify` did not read
/// `workspaces.status`, the operator's natural decommission action would leave
/// every credential in that workspace live AND unrevokable, which is the exact
/// opposite of what archiving looks like it does.
///
/// Before/after on the SAME client, so this cannot pass against a `verify` that
/// rejects everything.
#[tokio::test]
async fn a_client_in_an_archived_workspace_does_not_verify() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert!(
        f.clients
            .verify(&c.client_id, &c.client_secret)
            .await
            .unwrap()
            .is_some(),
        "must verify while the workspace is active"
    );

    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(f.ws)
        .execute(&f.db.pool)
        .await
        .unwrap();

    assert!(
        f.clients
            .verify(&c.client_id, &c.client_secret)
            .await
            .unwrap()
            .is_none(),
        "archiving the workspace must stop its clients authenticating — \
         nothing else can, because archiving also removes the revoke route"
    );
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

// ---- `current`: the same refusals as `verify`, without the secret ----------
//
// Each refusal test is a before/after on the SAME client so it cannot pass
// against a `current` that returns `None` for everything.

async fn assert_current_live(f: &Fixture, client_id: &str) {
    assert!(
        f.clients.current(client_id).await.unwrap().is_some(),
        "must have a current state before the refusal is applied"
    );
}

#[tokio::test]
async fn current_of_a_live_client_matches_verify_and_carries_the_stored_hash() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();

    let vc = f
        .clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .expect("must verify");
    let st = f
        .clients
        .current(&c.client_id)
        .await
        .unwrap()
        .expect("a live client has a current state");
    assert_eq!(st.verified, vc);

    let stored: String =
        sqlx::query_scalar("SELECT client_secret_hash FROM dataapi.clients WHERE id = $1")
            .bind(c.id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(st.secret_hash, stored);
}

#[tokio::test]
async fn current_of_a_client_with_a_future_expiry_is_some() {
    let f = skip_without_db!();
    let future = chrono::Utc::now() + chrono::Duration::hours(1);
    let c = f
        .clients
        .create(f.sa, "prod", Some(future), f.admin)
        .await
        .unwrap();
    assert!(f.clients.current(&c.client_id).await.unwrap().is_some());
}

#[tokio::test]
async fn current_of_an_unknown_client_is_none() {
    let f = skip_without_db!();
    assert!(f.clients.current("nope").await.unwrap().is_none());
}

#[tokio::test]
async fn current_of_a_revoked_client_is_none() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert_current_live(&f, &c.client_id).await;
    f.clients.revoke(c.id).await.unwrap();
    assert!(f.clients.current(&c.client_id).await.unwrap().is_none());
}

#[tokio::test]
async fn current_of_an_expired_client_is_none() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert_current_live(&f, &c.client_id).await;
    sqlx::query("UPDATE dataapi.clients SET expires_at = now() - interval '1 hour' WHERE id = $1")
        .bind(c.id)
        .execute(&f.db.pool)
        .await
        .unwrap();
    assert!(f.clients.current(&c.client_id).await.unwrap().is_none());
}

#[tokio::test]
async fn current_of_a_client_whose_service_account_is_disabled_is_none() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert_current_live(&f, &c.client_id).await;
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(f.sa)
        .execute(&f.db.pool)
        .await
        .unwrap();
    assert!(f.clients.current(&c.client_id).await.unwrap().is_none());
}

#[tokio::test]
async fn current_of_a_client_in_an_archived_workspace_is_none() {
    let f = skip_without_db!();
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert_current_live(&f, &c.client_id).await;
    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(f.ws)
        .execute(&f.db.pool)
        .await
        .unwrap();
    assert!(f.clients.current(&c.client_id).await.unwrap().is_none());
}
