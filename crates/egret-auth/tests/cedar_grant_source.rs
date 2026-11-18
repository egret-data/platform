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

#[tokio::test]
async fn fold_backfills_and_trigger_mirrors_new_writes() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    let store = egret_auth::grants::AccessGrantStore::new(db.clone());
    // Backfill existing rows.
    store.fold().await.unwrap();
    // Replica fidelity right after fold: the two tables match row-for-row.
    let diff: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ( \
           (SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM access_grants \
            EXCEPT SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM perm.cedar_grants) \
           UNION ALL \
           (SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM perm.cedar_grants \
            EXCEPT SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM access_grants) \
         ) d",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(diff, 0, "fold makes the mirror faithful");

    // A later access_grants write must appear in the mirror via the trigger.
    let role_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM roles LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .expect("a seeded role");
    let principal = uuid::Uuid::new_v4();
    let gid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, effect) \
         VALUES ('user', $1, $2, 'workspace', $3, 'deny') RETURNING id",
    )
    .bind(principal)
    .bind(role_id)
    .bind(uuid::Uuid::new_v4())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mirrored: Option<String> =
        sqlx::query_scalar("SELECT effect FROM perm.cedar_grants WHERE id = $1")
            .bind(gid)
            .fetch_optional(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        mirrored.as_deref(),
        Some("deny"),
        "insert mirrored by trigger"
    );

    sqlx::query("UPDATE access_grants SET effect = 'allow' WHERE id = $1")
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    let updated: String = sqlx::query_scalar("SELECT effect FROM perm.cedar_grants WHERE id = $1")
        .bind(gid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(updated, "allow", "update mirrored by trigger");

    sqlx::query("DELETE FROM access_grants WHERE id = $1")
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.cedar_grants WHERE id = $1")
        .bind(gid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "delete mirrored by trigger");

    // clear_mirror empties the mirror; fold restores it.
    store.clear_mirror().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.cedar_grants")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "clear_mirror empties the mirror");
    store.fold().await.unwrap();
}
