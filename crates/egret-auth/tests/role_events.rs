//! DB-gated test that `RoleRepo` publishes `RolesChanged` on exactly the catalog-mutating
//! methods (create / delete / set_permissions) and on none of the others. Self-skips
//! without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS;
//! CI / `just up` provides Postgres.

use egret_auth::events::RolesChanged;
use egret_auth::roles::RoleRepo;
use egret_events::EventBus;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn role_mutations_publish_roles_changed() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let bus = EventBus::new();
    let mut rx = bus.subscribe::<RolesChanged>();
    let repo = RoleRepo::new(db.clone(), bus.clone());

    // create → exactly one event.
    let id = repo
        .create("workspace", &uniq("evt"), "", None)
        .await
        .unwrap();
    assert!(rx.try_recv().is_ok(), "create should publish RolesChanged");
    assert!(rx.try_recv().is_err(), "create should publish exactly one");

    // set_permissions → exactly one event.
    repo.set_permissions(id, &["dashboard:read".into()])
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "set_permissions should publish RolesChanged"
    );
    assert!(
        rx.try_recv().is_err(),
        "set_permissions should publish exactly one"
    );

    // update_description → no event.
    repo.update_description(id, "x").await.unwrap();
    assert!(
        rx.try_recv().is_err(),
        "update_description should NOT publish"
    );

    // delete → exactly one event.
    repo.delete(id).await.unwrap();
    assert!(rx.try_recv().is_ok(), "delete should publish RolesChanged");
    assert!(rx.try_recv().is_err(), "delete should publish exactly one");
}
