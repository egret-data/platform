//! A child role's Cedar template must include its parent's permissions.
//! Requires DATABASE_URL.
use egret_auth::pdp::CedarPdp;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn child_role_inherits_parent_permissions() {
    let db = db().await;
    // parent has dashboard:read; child inherits parent, adds nothing of its own.
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    let pname = format!("b2_3_pdp_parent_{}", &parent.to_string()[..8]);
    let cname = format!("b2_3_pdp_child_{}", &child.to_string()[..8]);
    sqlx::query("INSERT INTO roles (id, scope, name, description) VALUES ($1,'workspace',$2,'')")
        .bind(parent)
        .bind(&pname)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1,'dashboard:read')")
        .bind(parent)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO roles (id, scope, name, description, parent_role_id) VALUES ($1,'workspace',$2,'',$3)")
        .bind(child)
        .bind(&cname)
        .bind(parent)
        .execute(&db.pool)
        .await
        .unwrap();

    // Build the PDP AFTER seeding (raw SQL seeding emits no RolesChanged).
    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();

    // Grant a user the CHILD role at workspace scope, then request the
    // PARENT-only action dashboard:read on that workspace.
    let user = UserId::new();
    let ws = WorkspaceId::new();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user',$1,$2,'workspace',$3)",
    )
    .bind(user.0)
    .bind(child)
    .bind(ws.0)
    .execute(&db.pool)
    .await
    .unwrap();

    let subject = Subject {
        user_id: user,
        group_ids: vec![],
    };
    let d = pdp
        .authorize(&subject, "dashboard:read", &Resource::Workspace(ws))
        .await
        .unwrap();
    assert!(d.allow, "child role must inherit parent's dashboard:read");

    // cleanup
    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1")
        .bind(user.0)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM roles WHERE id IN ($1,$2)")
        .bind(child)
        .bind(parent)
        .execute(&db.pool)
        .await
        .unwrap();
}
