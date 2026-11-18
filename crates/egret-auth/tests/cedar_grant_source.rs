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

/// Restores the shared DB even if an assertion panics: switch back to Legacy,
/// rename the mirror back if it was renamed, and delete seeded rows.
struct Cleanup {
    ids: Vec<uuid::Uuid>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let ids = self.ids.clone();
        let h = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    // Own pool on this runtime: the test runtime is blocked in Drop.
                    let url = egret_test_db::url().await.expect("DATABASE_URL");
                    let pool = sqlx::PgPool::connect(&url).await.unwrap();
                    let _ = sqlx::query(
                        "DO $$ BEGIN IF to_regclass('perm.cedar_grants') IS NULL AND to_regclass('perm.cedar_grants_tmp') IS NOT NULL THEN ALTER TABLE perm.cedar_grants_tmp RENAME TO cedar_grants; END IF; END $$",
                    )
                    .execute(&pool)
                    .await;
                    let _ = sqlx::query("UPDATE perm.cedar_grant_source SET source = 'legacy'")
                        .execute(&pool)
                        .await;
                    for id in ids {
                        let _ = sqlx::query("DELETE FROM perm.cedar_grants WHERE id = $1")
                            .bind(id)
                            .execute(&pool)
                            .await;
                        let _ = sqlx::query("DELETE FROM access_grants WHERE id = $1")
                            .bind(id)
                            .execute(&pool)
                            .await;
                    }
                });
        });
        let _ = h.join();
    }
}

async fn seed_role(db: &Db) -> uuid::Uuid {
    sqlx::query_scalar("SELECT id FROM roles LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .expect("a seeded role")
}

async fn seed_legacy_grant(db: &Db, user: uuid::Uuid, role: uuid::Uuid) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, effect) \
         VALUES ('user', $1, $2, 'platform', NULL, 'allow') RETURNING id",
    )
    .bind(user)
    .bind(role)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn report_empty_when_mirror_is_faithful_and_catches_divergence() {
    use egret_auth::grants::GrantDivergenceKind::{OnlyLegacy, OnlyUnified};
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    let store = egret_auth::grants::AccessGrantStore::new(db.clone());
    store.fold().await.unwrap();
    assert!(
        store.report().await.unwrap().is_empty(),
        "faithful mirror => empty report"
    );

    let role = seed_role(&db).await;
    let ghost = uuid::Uuid::new_v4();
    let user = uuid::Uuid::new_v4();
    // (a) legacy-only row: insert then drop its mirror copy.
    let legacy_only = seed_legacy_grant(&db, user, role).await;
    // (b) same id, different effect: insert then flip the mirror's effect.
    let differing = seed_legacy_grant(&db, uuid::Uuid::new_v4(), role).await;
    let _c = Cleanup {
        ids: vec![ghost, legacy_only, differing],
    };
    sqlx::query("DELETE FROM perm.cedar_grants WHERE id = $1")
        .bind(legacy_only)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE perm.cedar_grants SET effect = 'deny' WHERE id = $1")
        .bind(differing)
        .execute(&db.pool)
        .await
        .unwrap();
    // (c) mirror-only ghost row.
    sqlx::query(
        "INSERT INTO perm.cedar_grants (id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at) \
         VALUES ($1,'user',$1,$1,'platform',NULL,NULL,'allow',NULL)",
    )
    .bind(ghost)
    .execute(&db.pool)
    .await
    .unwrap();

    let r = store.report().await.unwrap();
    let has = |id, k| r.iter().any(|d| d.id == id && d.kind == k);
    assert!(has(ghost, OnlyUnified), "mirror-only row");
    assert!(has(legacy_only, OnlyLegacy), "legacy-only row");
    assert!(
        has(differing, OnlyLegacy) && has(differing, OnlyUnified),
        "same id with differing effect shows under both kinds"
    );
}

#[tokio::test]
async fn grants_for_reads_the_source_the_switch_names() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    let store = egret_auth::grants::AccessGrantStore::new(db.clone());
    let user = uuid::Uuid::new_v4();
    let uid = egret_core::ids::UserId(user);
    let role_id = seed_role(&db).await;
    let gid = seed_legacy_grant(&db, user, role_id).await;
    let ghost = uuid::Uuid::new_v4();
    let _c = Cleanup {
        ids: vec![gid, ghost],
    };
    store.fold().await.unwrap();

    let key = |g: &egret_auth::grants::Grant| (g.role_id, g.scope_type.clone(), g.effect.clone());
    let sorted = |v: Vec<egret_auth::grants::Grant>| {
        let mut k: Vec<_> = v.iter().map(key).collect();
        k.sort();
        k
    };

    write_source(&db, GrantSource::Legacy, "t").await.unwrap();
    let legacy = sorted(store.grants_for(uid, None, &[]).await.unwrap());
    write_source(&db, GrantSource::Unified, "t").await.unwrap();
    let unified = sorted(store.grants_for(uid, None, &[]).await.unwrap());
    assert_eq!(legacy.len(), 1);
    assert_eq!(
        legacy, unified,
        "faithful mirror => same set under both sources"
    );

    // Mirror-only ghost for the same subject: visible ONLY under unified.
    sqlx::query(
        "INSERT INTO perm.cedar_grants (id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at) \
         VALUES ($1,'user',$2,$3,'platform',NULL,NULL,'deny',NULL)",
    )
    .bind(ghost)
    .bind(user)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let unified_ghost = store.grants_for(uid, None, &[]).await.unwrap();
    write_source(&db, GrantSource::Legacy, "t").await.unwrap();
    let legacy_ghost = store.grants_for(uid, None, &[]).await.unwrap();
    assert_eq!(unified_ghost.len(), 2, "unified reads perm.cedar_grants");
    assert_eq!(legacy_ghost.len(), 1, "legacy ignores the mirror-only row");
}

#[tokio::test]
async fn unified_read_error_fails_safe_to_access_grants() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    let store = egret_auth::grants::AccessGrantStore::new(db.clone());
    let user = uuid::Uuid::new_v4();
    let role = seed_role(&db).await;
    let gid = seed_legacy_grant(&db, user, role).await;
    let _c = Cleanup { ids: vec![gid] };
    write_source(&db, GrantSource::Unified, "t").await.unwrap();
    // Force the unified read to error: the mirror table "does not exist".
    sqlx::query("ALTER TABLE perm.cedar_grants RENAME TO cedar_grants_tmp")
        .execute(&db.pool)
        .await
        .unwrap();
    let got = store
        .grants_for(egret_core::ids::UserId(user), None, &[])
        .await;
    // Restore before asserting (Cleanup also restores on panic).
    sqlx::query("ALTER TABLE perm.cedar_grants_tmp RENAME TO cedar_grants")
        .execute(&db.pool)
        .await
        .unwrap();
    let got = got.expect("fail-safe: no error under Unified");
    assert_eq!(got.len(), 1, "falls back to access_grants rows");
    assert_eq!(got[0].role_id, role);
}
