//! Username takeover (spec `docs/en/specs/2026-10-23-username-takeover-design.md`
//! §4.1, decision D1): a username is never held by two people over time.
//!
//! Every test is written from the side of the NEXT person whose email slugs to
//! a name someone else held, or that a stale ref still names: they must be
//! given another name. Migration 0208's ledger enforces it.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

#[path = "support/recorder.rs"]
mod recorder;

use std::time::Duration;

use egret_auth::users::{ensure_username_via, UserRepo};
use egret_core::UserId;
use egret_metastore::Db;
use recorder::Recorder;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 8).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// A short lowercase token: unique per test, and its own slug.
fn t() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

async fn user(db: &Db, email: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(email)
    .fetch_one(&db.pool)
    .await
    .expect("insert user");
    UserId(id)
}

async fn mint(db: &Db, email: &str) -> String {
    let uid = user(db, email).await;
    UserRepo::new(db.clone())
        .ensure_username(uid)
        .await
        .expect("mint")
}

async fn backfill(db: &Db) {
    sqlx::query("SELECT username_ledger_backfill()")
        .execute(&db.pool)
        .await
        .expect("backfill");
}

fn sqlstate(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code())
        .map(|c| c.into_owned())
}

#[tokio::test]
async fn the_next_person_never_gets_a_deleted_users_name() {
    let Some(db) = connect().await else { return };
    let repo = UserRepo::new(db.clone());
    let tag = t();
    let first = user(&db, &format!("bob_{tag}@a.test")).await;
    assert_eq!(
        repo.ensure_username(first).await.unwrap(),
        format!("bob_{tag}")
    );
    repo.hard_delete(first, &Recorder::default())
        .await
        .expect("delete");

    let next = mint(&db, &format!("bob_{tag}@b.test")).await;
    assert_eq!(
        next,
        format!("bob_{tag}-2"),
        "the freed name was handed out"
    );

    // The ledger keeps who held it.
    #[derive(sqlx::FromRow)]
    struct Row {
        user_id: Option<Uuid>,
        minted_for: Option<Uuid>,
        minted_at: Option<chrono::DateTime<chrono::Utc>>,
        retired_at: Option<chrono::DateTime<chrono::Utc>>,
        source: String,
    }
    let row: Row = sqlx::query_as(
        "SELECT user_id, minted_for, minted_at, retired_at, source FROM username_ledger \
          WHERE username = $1",
    )
    .bind(format!("bob_{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.user_id, None);
    assert_eq!(row.minted_for, Some(first.0));
    assert!(row.minted_at.is_some());
    assert!(row.retired_at.is_some());
    assert_eq!(row.source, "mint");
}

#[tokio::test]
async fn a_direct_write_of_a_retired_name_is_refused() {
    let Some(db) = connect().await else { return };
    let repo = UserRepo::new(db.clone());
    let tag = t();
    let first = user(&db, &format!("bob_{tag}@a.test")).await;
    repo.ensure_username(first).await.unwrap();
    repo.hard_delete(first, &Recorder::default()).await.unwrap();

    let next = user(&db, &format!("other_{tag}@b.test")).await;
    let err = sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(next.0)
        .bind(format!("bob_{tag}"))
        .execute(&db.pool)
        .await
        .expect_err("a retired name was written");
    assert_eq!(sqlstate(&err).as_deref(), Some("23505"), "{err}");
    // Case does not help: usernames are citext.
    let err = sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(next.0)
        .bind(format!("BOB_{tag}"))
        .execute(&db.pool)
        .await
        .expect_err("a retired name was written in upper case");
    assert_eq!(sqlstate(&err).as_deref(), Some("23505"), "{err}");
}

#[tokio::test]
async fn a_renamed_users_old_name_is_retired() {
    let Some(db) = connect().await else { return };
    let tag = t();
    let a = user(&db, &format!("ren_{tag}@a.test")).await;
    sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(a.0)
        .bind(format!("ren_{tag}"))
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(a.0)
        .bind(format!("ren2_{tag}"))
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        mint(&db, &format!("ren_{tag}@b.test")).await,
        format!("ren_{tag}-2")
    );
}

/// Inserts a grant naming `user:<name>` on a throwaway catalog.
async fn grant_naming(db: &Db, name: &str) {
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'iceberg', '{}') \
         RETURNING id",
    )
    .bind(format!("ledger_cat_{}", t()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let g: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id) VALUES ('allow', ARRAY['read'], $1) \
         RETURNING id",
    )
    .bind(cat)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
         VALUES ($1, 'user', $2, 'include')",
    )
    .bind(g)
    .bind(name)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_name_named_only_by_a_stale_ref_is_never_minted() {
    let Some(db) = connect().await else { return };
    let tag = t();
    grant_naming(&db, &format!("carol_{tag}")).await;
    backfill(&db).await;
    assert_eq!(
        mint(&db, &format!("carol_{tag}@x.test")).await,
        format!("carol_{tag}-2")
    );
}

async fn policy_naming(db: &Db, managed_by: &str, policy_name: &str, user: &str) {
    // A service type of its own: the row only has to exist.
    let svc_type = format!("ledger_type_{}", t());
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) \
         VALUES (abs(hashtext($1))::bigint + 1000000, $1, '{}')",
    )
    .bind(&svc_type)
    .execute(&db.pool)
    .await
    .unwrap();
    let svc: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("ledger_svc_{}", t()))
    .bind(&svc_type)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let body = serde_json::json!({
        "name": policy_name,
        "policyItems": [{ "users": [user, "{USER}"], "groups": [], "roles": [] }],
        "denyPolicyItems": [],
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(svc)
    .bind(policy_name)
    .bind(body)
    .bind(managed_by)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_name_in_a_hand_written_policy_is_never_minted() {
    let Some(db) = connect().await else { return };
    let tag = t();
    policy_naming(&db, "operator", "hand", &format!("opu_{tag}")).await;
    backfill(&db).await;
    assert_eq!(
        mint(&db, &format!("opu_{tag}@x.test")).await,
        format!("opu_{tag}-2")
    );
}

#[tokio::test]
async fn a_name_in_an_egret_admin_body_is_never_minted() {
    let Some(db) = connect().await else { return };
    let tag = t();
    policy_naming(
        &db,
        "infra",
        "egret-admin - catalog, schema, table",
        &format!("adm_{tag}"),
    )
    .await;
    backfill(&db).await;
    assert_eq!(
        mint(&db, &format!("adm_{tag}@x.test")).await,
        format!("adm_{tag}-2")
    );
    // A macro is not a name.
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM username_ledger WHERE username = '{USER}'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn a_deleted_service_accounts_base_name_is_never_minted() {
    let Some(db) = connect().await else { return };
    let tag = t();
    sqlx::query(
        "INSERT INTO audit.audit_log (kind, action, resource_type, detail) \
         VALUES ('mutation', 'service_account:delete', 'service_account', $1)",
    )
    .bind(serde_json::json!({
        "name": "rep",
        "email": format!("svc.rep_{tag}@ws.service.egret.local"),
    }))
    .execute(&db.pool)
    .await
    .unwrap();
    backfill(&db).await;
    assert_eq!(
        mint(&db, &format!("svc.rep_{tag}@other.service.egret.local")).await,
        format!("svc-rep_{tag}-2")
    );
}

#[tokio::test]
async fn a_name_only_a_governance_event_remembers_is_never_minted() {
    let Some(db) = connect().await else { return };
    let tag = t();
    sqlx::query(
        "INSERT INTO audit.audit_log (kind, action, resource_type, detail) \
         VALUES ('mutation', 'perm_grant:delete', 'perm_grant', $1), \
                ('mutation', 'engine_grant:delete', 'engine_grant', $2)",
    )
    .bind(serde_json::json!({
        "before": { "include": [format!("user:ev_{tag}")], "exclude": [] },
        "after": null,
    }))
    .bind(serde_json::json!({
        "before": { "principals": [{ "kind": "user", "ref": format!("eg_{tag}") }] },
        "after": null,
    }))
    .execute(&db.pool)
    .await
    .unwrap();
    backfill(&db).await;
    assert_eq!(
        mint(&db, &format!("ev_{tag}@x.test")).await,
        format!("ev_{tag}-2")
    );
    assert_eq!(
        mint(&db, &format!("eg_{tag}@x.test")).await,
        format!("eg_{tag}-2")
    );
}

#[tokio::test]
async fn the_delegator_name_is_reserved() {
    let Some(db) = connect().await else { return };
    let got = mint(&db, &format!("egret_delegator@{}.test", t())).await;
    assert_ne!(got, "egret_delegator");
    assert!(got.starts_with("egret_delegator-"), "{got}");
}

#[tokio::test]
async fn the_ledger_is_append_only() {
    let Some(db) = connect().await else { return };
    let tag = t();
    let name = mint(&db, &format!("ao_{tag}@x.test")).await;
    let other = user(&db, &format!("ao2_{tag}@x.test")).await;
    for (what, sql) in [
        ("delete", "DELETE FROM username_ledger WHERE username = $1"),
        (
            "rename",
            "UPDATE username_ledger SET username = username || '-x' WHERE username = $1",
        ),
        (
            "minted_for",
            "UPDATE username_ledger SET minted_for = gen_random_uuid() WHERE username = $1",
        ),
        (
            "source",
            "UPDATE username_ledger SET source = 'reserved' WHERE username = $1",
        ),
    ] {
        let err = sqlx::query(sql)
            .bind(&name)
            .execute(&db.pool)
            .await
            .expect_err(what);
        assert!(err.to_string().contains("append-only"), "{what}: {err}");
    }
    // Handing the row to someone else would hand them the name.
    let err = sqlx::query("UPDATE username_ledger SET user_id = $2 WHERE username = $1")
        .bind(&name)
        .bind(other.0)
        .execute(&db.pool)
        .await
        .expect_err("user_id reassigned");
    assert!(err.to_string().contains("append-only"), "{err}");
    let err = sqlx::query("TRUNCATE username_ledger")
        .execute(&db.pool)
        .await
        .expect_err("truncate");
    assert!(err.to_string().contains("append-only"), "{err}");
}

#[tokio::test]
async fn a_holder_resetting_their_own_name_is_allowed() {
    let Some(db) = connect().await else { return };
    let name = mint(&db, &format!("same_{}@x.test", t())).await;
    sqlx::query("UPDATE users SET username = username WHERE username = $1")
        .bind(&name)
        .execute(&db.pool)
        .await
        .expect("a holder re-setting their name");
}

#[tokio::test]
async fn a_mint_racing_a_delete_cannot_take_the_name() {
    let Some(db) = connect().await else { return };
    let tag = t();
    let dan = user(&db, &format!("dan_{tag}@a.test")).await;
    UserRepo::new(db.clone())
        .ensure_username(dan)
        .await
        .unwrap();
    let next = user(&db, &format!("dan_{tag}@b.test")).await;

    // A: the delete's row DELETE, not yet committed.
    let mut a = db.pool.begin().await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(dan.0)
        .execute(&mut *a)
        .await
        .unwrap();

    // B: a mint for the next person; it waits on the `users` unique index.
    let pool = db.pool.clone();
    let b = tokio::spawn(async move {
        let mut conn = pool.acquire().await.unwrap();
        ensure_username_via(&mut conn, next).await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!b.is_finished(), "the mint did not wait for the delete");

    a.commit().await.unwrap();
    let got = b.await.unwrap().expect("mint");
    assert_eq!(got, format!("dan_{tag}-2"));
}

/// The ledger's SQL slug (seed step 4a) agrees with `slugify_localpart`.
#[tokio::test]
async fn sql_slug_matches_slugify_localpart() {
    let Some(db) = connect().await else { return };
    for local in [
        "A.B",
        "a..b",
        "-a-",
        "ü",
        "a_b",
        "",
        "a+tag",
        "x--y",
        "Mixed.Case_9",
        "__",
        "!!!",
    ] {
        let email = format!("{local}@x.test");
        let sql: String = sqlx::query_scalar("SELECT username_slug($1)")
            .bind(&email)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            sql,
            egret_auth::users::slugify_localpart_for_tests(&email),
            "{email:?}"
        );
    }
}
