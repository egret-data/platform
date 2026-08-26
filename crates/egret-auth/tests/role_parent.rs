//! Role hierarchy: parent set/validate on RoleRepo. Requires DATABASE_URL.
use egret_auth::roles::RoleRepo;
use egret_events::EventBus;
use egret_metastore::Db;

async fn db() -> Db {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn set_parent_same_scope_ok_and_publishes() {
    let db = db().await;
    let bus = EventBus::new();
    let repo = RoleRepo::new(db.clone(), bus.clone());
    let mut rx = bus.subscribe::<egret_auth::events::RolesChanged>();
    let child = repo
        .create("workspace", "b2_3_child", "", None)
        .await
        .unwrap();
    let parent = repo
        .create("workspace", "b2_3_parent", "", None)
        .await
        .unwrap();
    repo.set_parent(child, Some(parent)).await.unwrap();
    // A RolesChanged fires on set_parent (drain: create also fired earlier).
    assert!(
        rx.try_recv().is_ok(),
        "set_parent must publish RolesChanged"
    );
    let row = repo.get(child).await.unwrap().unwrap();
    assert_eq!(row.parent_role_id, Some(parent));
    // cleanup
    repo.delete(child).await.unwrap();
    repo.delete(parent).await.unwrap();
}

#[tokio::test]
async fn set_parent_cross_scope_rejected() {
    let db = db().await;
    let repo = RoleRepo::new(db.clone(), EventBus::new());
    let child = repo
        .create("workspace", "b2_3_xs_child", "", None)
        .await
        .unwrap();
    let platform_parent = repo
        .create("platform", "b2_3_xs_parent", "", None)
        .await
        .unwrap();
    let err = repo.set_parent(child, Some(platform_parent)).await;
    assert!(err.is_err(), "cross-scope parent must be rejected");
    repo.delete(child).await.unwrap();
    repo.delete(platform_parent).await.unwrap();
}

#[tokio::test]
async fn set_parent_cycle_rejected() {
    let db = db().await;
    let repo = RoleRepo::new(db.clone(), EventBus::new());
    let a = repo
        .create("workspace", "b2_3_cyc_a", "", None)
        .await
        .unwrap();
    let b = repo
        .create("workspace", "b2_3_cyc_b", "", None)
        .await
        .unwrap();
    repo.set_parent(b, Some(a)).await.unwrap(); // b -> a
    let err = repo.set_parent(a, Some(b)).await; // a -> b would cycle
    assert!(err.is_err(), "cycle must be rejected");
    repo.delete(b).await.unwrap();
    repo.delete(a).await.unwrap();
}
