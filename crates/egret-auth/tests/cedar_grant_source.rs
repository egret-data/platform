//! EO4 grant-source switch-store tests. DB-gated via egret_test_db; requires DATABASE_URL.
use egret_auth::pdp::grant_source_switch::{read_source, read_state, write_source, GrantSource};
use egret_metastore::Db;

/// Serialises the tests that mutate the global switch row.
static SWITCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db() -> Db {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn default_is_legacy_when_absent() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    sqlx::query("DELETE FROM perm.cedar_grant_source")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(read_source(&db).await, GrantSource::Legacy);
    let st = read_state(&db).await.unwrap();
    assert!(st.switched_at.is_none() && st.switched_by.is_none());
}

#[tokio::test]
async fn write_then_read_roundtrips_with_audit() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    write_source(&db, GrantSource::Unified, "op@egret.local")
        .await
        .unwrap();
    assert_eq!(read_source(&db).await, GrantSource::Unified);
    let st = read_state(&db).await.unwrap();
    assert_eq!(st.source, GrantSource::Unified);
    assert_eq!(st.switched_by.as_deref(), Some("op@egret.local"));
    assert!(st.switched_at.is_some());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.cedar_grant_source")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "singleton row");
    write_source(&db, GrantSource::Legacy, "reset")
        .await
        .unwrap();
}

#[test]
fn db_text_roundtrip() {
    for s in [GrantSource::Legacy, GrantSource::Unified] {
        assert_eq!(GrantSource::from_db(s.as_db()), Some(s));
    }
    assert_eq!(GrantSource::from_db("nope"), None);
}
