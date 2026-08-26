//! Deny-wins: an allow on a folder + a deny on one item inside it → the item is
//! denied, siblings still allowed. Requires DATABASE_URL.
use egret_auth::pdp::CedarPdp;
use egret_core::authz::{ObjectRef, Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

async fn db() -> Db {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn deny_on_item_overrides_folder_allow() {
    let db = db().await;
    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();
    let user = UserId::new();
    let ws = WorkspaceId::new();
    let folder = Uuid::new_v4();
    let d1 = Uuid::new_v4(); // denied item
    let d2 = Uuid::new_v4(); // sibling, allowed
    let viewer: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_viewer'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let editor: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_editor'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    // Allow ws_viewer on the folder (cascades to items).
    sqlx::query("INSERT INTO access_grants (principal_type,principal_id,role_id,scope_type,object_type,scope_id,effect) VALUES ('user',$1,$2,'object','folder',$3,'allow')")
        .bind(user.0).bind(viewer).bind(folder).execute(&db.pool).await.unwrap();
    // Deny ws_editor (read+manage) on item d1.
    sqlx::query("INSERT INTO access_grants (principal_type,principal_id,role_id,scope_type,object_type,scope_id,effect) VALUES ('user',$1,$2,'object','dashboard',$3,'deny')")
        .bind(user.0).bind(editor).bind(d1).execute(&db.pool).await.unwrap();

    let subject = Subject {
        user_id: user,
        group_ids: vec![],
    };
    let obj = |id: Uuid| {
        Resource::Object(ObjectRef {
            kind: "dashboard".into(),
            id,
            folder_id: Some(folder),
            workspace_id: ws,
        })
    };
    // d1 denied (deny-wins over the folder allow).
    assert!(
        !pdp.authorize(&subject, "dashboard:read", &obj(d1))
            .await
            .unwrap()
            .allow,
        "deny on d1 must win"
    );
    // d2 still allowed via the folder grant.
    assert!(
        pdp.authorize(&subject, "dashboard:read", &obj(d2))
            .await
            .unwrap()
            .allow,
        "sibling d2 stays allowed"
    );

    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1")
        .bind(user.0)
        .execute(&db.pool)
        .await
        .unwrap();
}
