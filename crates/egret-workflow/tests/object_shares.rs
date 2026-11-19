//! EO6 Task 4: a `workflow:*` object grant reaches exactly that flow through the
//! per-id routes, with no workspace role. DB-gated like the rest of the suite.

mod common;

use axum::http::StatusCode;
use egret_core::UserId;
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

fn yaml(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: company.team\ntasks:\n  - {{id: a, type: io.kestra.plugin.core.log.Log, message: m}}\n"
    )
}

/// A fresh user with NO workspace role, holding a role of exactly `perms` on
/// the single flow `flow` -- the row the share API writes (object scope).
async fn guest_with(db: &Db, owner: UserId, flow: Uuid, perms: &[&str]) -> UserId {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 'g', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("g_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("guest");
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 't') RETURNING id",
    )
    .bind(format!("test_share_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("role");
    for p in perms {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&db.pool)
            .await
            .expect("perm");
    }
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by) \
         VALUES ('user', $1, $2, 'object', 'workflow', $3, 'allow', $4)",
    )
    .bind(uid)
    .bind(role)
    .bind(flow)
    .bind(owner.0)
    .execute(&db.pool)
    .await
    .expect("share");
    UserId(uid)
}

#[tokio::test]
async fn a_read_share_reaches_that_flow_only() {
    let Some(db) = common::db().await else { return };
    let (ws, owner) = common::seed_workspace(&db).await;
    let x = common::seed_flow(&db, ws, owner, &yaml("x"), "draft").await;
    let y = common::seed_flow(&db, ws, owner, &yaml("y"), "draft").await;
    let guest = guest_with(&db, owner, x, &["workflow:read"]).await;
    let app = common::app(&db).await;

    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{x}"), guest).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = common::json(res).await;
    assert_eq!(body["can_manage"], json!(false));
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{y}"), guest).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // read does not write
    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{x}"),
        guest,
        json!({ "source": yaml("x") }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_manage_share_updates_that_flow_only() {
    let Some(db) = common::db().await else { return };
    let (ws, owner) = common::seed_workspace(&db).await;
    let x = common::seed_flow(&db, ws, owner, &yaml("x"), "draft").await;
    let y = common::seed_flow(&db, ws, owner, &yaml("y"), "draft").await;
    let guest = guest_with(&db, owner, x, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;

    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{x}"),
        guest,
        json!({ "source": yaml("x") }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{y}"),
        guest,
        json!({ "source": yaml("y") }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{x}/versions"),
        guest,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}
