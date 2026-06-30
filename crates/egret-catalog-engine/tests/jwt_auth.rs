//! JWT catalog-auth mapping tests (F2-P2 Task 3).
//!
//! Verification is faked (no IdP / network); only egret's own Postgres tables
//! are touched (no MinIO / lakekeeper). The DB is seeded through the egret-side
//! (sqlx-0.8) repos so the test never mixes sqlx majors against `Db`'s pool —
//! the crate itself links sqlx 0.9 for lakekeeper, so `#[sqlx::test]` (which
//! would hand back a 0.9 pool) can't build the 0.8 `Db`. We therefore follow the
//! crate's DB-gated pattern: connect to `DATABASE_URL`, run egret's migrations,
//! seed with a unique suffix so the test is re-runnable against a persistent DB,
//! and self-skip when `DATABASE_URL` is unset.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-catalog-engine --test jwt_auth

use std::sync::Arc;

use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::jwt_auth::{AccessTokenVerifier, CatalogJwtAuth, JwtAuthError};
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

/// Fake verifier: returns a fixed subject regardless of the token bytes.
struct FakeVerifier(String);

#[async_trait::async_trait]
impl AccessTokenVerifier for FakeVerifier {
    async fn verify(&self, _t: &str) -> Result<String, JwtAuthError> {
        Ok(self.0.clone())
    }
}

/// Connect + apply egret migrations, or `None` when `DATABASE_URL` is unset.
async fn db_or_skip(what: &str) -> Option<Db> {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping {what}");
        return None;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");
    Some(db)
}

#[tokio::test]
async fn jwt_member_gets_notebook_rw_grant() {
    let Some(db) = db_or_skip("jwt_member_gets_notebook_rw_grant").await else {
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();
    let sub = format!("sub-{suffix}");

    // OIDC user + its stable username.
    let user_id = UserRepo::new(db.clone())
        .insert_oidc(&sub, &format!("alice-{suffix}@egret.test"), "Alice")
        .await
        .expect("seed oidc user");
    let username = UserRepo::new(db.clone())
        .ensure_username(user_id)
        .await
        .expect("ensure username");

    // Workspace + membership (owner is a separate local user).
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("owner-{suffix}@egret.test"), "Owner", "x")
        .await
        .expect("seed owner");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&format!("jwtws{suffix}"), "JWT WS", owner)
        .await
        .expect("seed workspace");
    MemberRepo::new(db.clone())
        .add(ws.id, "user", user_id.0, "ws_editor", owner)
        .await
        .expect("seed membership");

    // A warehouse whose lakekeeper UUID we authenticate against.
    let lakekeeper_uuid = Uuid::new_v4();
    let store = Arc::new(CatalogStore::new(db.clone()));
    let wh = store
        .insert_warehouse(ws.id.0, lakekeeper_uuid, &format!("wh-{suffix}"), None)
        .await
        .expect("insert warehouse");

    let auth = CatalogJwtAuth::new(Arc::new(FakeVerifier(sub.clone())), store, db);
    let ta = auth.authenticate("a.b.c", lakekeeper_uuid).await.unwrap();

    assert_eq!(ta.workspace_id, ws.id.0);
    assert_eq!(ta.warehouse_id, wh.id);
    assert_eq!(ta.grants.len(), 1);
    assert_eq!(ta.grants[0].namespace, format!("notebook_{username}"));
    assert_eq!(ta.grants[0].table, None);
    assert!(ta.grants[0]
        .permissions
        .iter()
        .any(|p| p == "catalog:write"));
    assert!(ta.grants[0].permissions.iter().any(|p| p == "catalog:read"));
}

#[tokio::test]
async fn jwt_non_member_is_forbidden() {
    let Some(db) = db_or_skip("jwt_non_member_is_forbidden").await else {
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();
    let sub = format!("sub-{suffix}");

    let _user_id = UserRepo::new(db.clone())
        .insert_oidc(&sub, &format!("alice-{suffix}@egret.test"), "Alice")
        .await
        .expect("seed oidc user");
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("owner-{suffix}@egret.test"), "Owner", "x")
        .await
        .expect("seed owner");
    // Workspace exists, but the OIDC user is NOT added as a member.
    let ws = WorkspaceRepo::new(db.clone())
        .create(&format!("jwtws{suffix}"), "JWT WS", owner)
        .await
        .expect("seed workspace");

    let lakekeeper_uuid = Uuid::new_v4();
    let store = Arc::new(CatalogStore::new(db.clone()));
    store
        .insert_warehouse(ws.id.0, lakekeeper_uuid, &format!("wh-{suffix}"), None)
        .await
        .expect("insert warehouse");

    let auth = CatalogJwtAuth::new(Arc::new(FakeVerifier(sub)), store, db);
    let err = auth
        .authenticate("a.b.c", lakekeeper_uuid)
        .await
        .unwrap_err();
    assert!(matches!(err, JwtAuthError::Forbidden));
}

#[tokio::test]
async fn jwt_unknown_subject_is_unauthenticated() {
    let Some(db) = db_or_skip("jwt_unknown_subject_is_unauthenticated").await else {
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();

    // Seed a member user, but verify a DIFFERENT (unknown) subject.
    let user_id = UserRepo::new(db.clone())
        .insert_oidc(
            &format!("sub-{suffix}"),
            &format!("alice-{suffix}@egret.test"),
            "Alice",
        )
        .await
        .expect("seed oidc user");
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("owner-{suffix}@egret.test"), "Owner", "x")
        .await
        .expect("seed owner");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&format!("jwtws{suffix}"), "JWT WS", owner)
        .await
        .expect("seed workspace");
    MemberRepo::new(db.clone())
        .add(ws.id, "user", user_id.0, "ws_editor", owner)
        .await
        .expect("seed membership");

    let lakekeeper_uuid = Uuid::new_v4();
    let store = Arc::new(CatalogStore::new(db.clone()));
    store
        .insert_warehouse(ws.id.0, lakekeeper_uuid, &format!("wh-{suffix}"), None)
        .await
        .expect("insert warehouse");

    let auth = CatalogJwtAuth::new(
        Arc::new(FakeVerifier(format!("nobody-{suffix}"))),
        store,
        db,
    );
    let err = auth
        .authenticate("a.b.c", lakekeeper_uuid)
        .await
        .unwrap_err();
    assert!(matches!(err, JwtAuthError::Unauthenticated));
}

#[tokio::test]
async fn jwt_unknown_warehouse_is_forbidden() {
    let Some(db) = db_or_skip("jwt_unknown_warehouse_is_forbidden").await else {
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();
    let sub = format!("sub-{suffix}");

    let user_id = UserRepo::new(db.clone())
        .insert_oidc(&sub, &format!("alice-{suffix}@egret.test"), "Alice")
        .await
        .expect("seed oidc user");
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("owner-{suffix}@egret.test"), "Owner", "x")
        .await
        .expect("seed owner");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&format!("jwtws{suffix}"), "JWT WS", owner)
        .await
        .expect("seed workspace");
    MemberRepo::new(db.clone())
        .add(ws.id, "user", user_id.0, "ws_editor", owner)
        .await
        .expect("seed membership");

    let store = Arc::new(CatalogStore::new(db.clone()));
    // Seed a warehouse, but authenticate against a DIFFERENT (unknown) prefix.
    store
        .insert_warehouse(ws.id.0, Uuid::new_v4(), &format!("wh-{suffix}"), None)
        .await
        .expect("insert warehouse");

    let auth = CatalogJwtAuth::new(Arc::new(FakeVerifier(sub)), store, db);
    let err = auth
        .authenticate("a.b.c", Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, JwtAuthError::Forbidden));
}
