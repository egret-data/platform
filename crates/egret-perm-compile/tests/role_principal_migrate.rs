//! SP3c-1: role principals -> managed `role:<name> holders` groups.

use egret_metastore::Db;
use uuid::Uuid;

#[tokio::test]
async fn a_managed_group_source_is_accepted() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let id = Uuid::new_v4();
    let r = sqlx::query("INSERT INTO public.groups (id, name, source) VALUES ($1,$2,'managed')")
        .bind(id)
        .bind(format!("role:data_steward holders {id}"))
        .execute(&db.pool)
        .await;
    assert!(r.is_ok(), "managed source must be accepted: {r:?}");
}
