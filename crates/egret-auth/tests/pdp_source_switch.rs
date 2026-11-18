//! EO3 switch-store tests. DB-gated via egret_test_db; requires DATABASE_URL.
use egret_auth::pdp::source_switch::{read_source, read_state, write_source, PolicySource};
use egret_auth::pdp::CedarPdp;
use egret_metastore::Db;

/// Serialises the tests that mutate the global switch row.
static SWITCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db() -> Db {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

async fn reset(db: &Db) {
    sqlx::query("DELETE FROM perm.pdp_policy_source")
        .execute(&db.pool)
        .await
        .expect("reset");
}

// One test body: the switch is a global singleton, so parallel tests would race.
#[tokio::test]
async fn switch_store_default_roundtrip_and_singleton() {
    let _guard = SWITCH_LOCK.lock().await;
    let db = db().await;
    reset(&db).await;

    // Absent row reads as legacy.
    assert_eq!(read_source(&db).await, PolicySource::Legacy);
    let st = read_state(&db).await.expect("state");
    assert_eq!(st.source, PolicySource::Legacy);
    assert!(st.switched_at.is_none() && st.switched_by.is_none());

    write_source(&db, PolicySource::Shadow, "op@egret.local")
        .await
        .expect("write");
    assert_eq!(read_source(&db).await, PolicySource::Shadow);
    let st = read_state(&db).await.expect("state");
    assert_eq!(st.source, PolicySource::Shadow);
    assert_eq!(st.switched_by.as_deref(), Some("op@egret.local"));
    assert!(st.switched_at.is_some());

    // Singleton: a second write updates the SAME row.
    write_source(&db, PolicySource::Compiled, "op2@egret.local")
        .await
        .expect("write2");
    assert_eq!(read_source(&db).await, PolicySource::Compiled);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.pdp_policy_source")
        .fetch_one(&db.pool)
        .await
        .expect("count");
    assert_eq!(n, 1, "singleton row");

    reset(&db).await;
}

#[test]
fn db_text_roundtrip() {
    for s in [
        PolicySource::Legacy,
        PolicySource::Shadow,
        PolicySource::Compiled,
    ] {
        assert_eq!(PolicySource::from_db(s.as_db()), Some(s));
    }
    assert_eq!(PolicySource::from_db("bogus"), None);
}

#[tokio::test]
async fn report_is_empty_on_a_normal_db() {
    let db = db().await;
    let divergences = CedarPdp::report(&db).await.expect("report");
    assert!(
        divergences.is_empty(),
        "expected equivalence, got {divergences:?}"
    );
}

// One body: the switch is a global singleton shared by every test in this file.
#[tokio::test]
async fn each_state_serves_the_expected_assembler_and_fails_safe() {
    let _guard = SWITCH_LOCK.lock().await;
    let db = db().await;
    reset(&db).await;
    let legacy_ids = CedarPdp::legacy_ids_for_test(&db).await.expect("legacy");
    let compiled_ids = CedarPdp::compiled_ids_for_test(&db)
        .await
        .expect("compiled");

    // Absent row => legacy.
    assert_eq!(
        CedarPdp::served_policy_ids_for_test(&db).await.unwrap(),
        legacy_ids
    );
    write_source(&db, PolicySource::Legacy, "t").await.unwrap();
    assert_eq!(
        CedarPdp::served_policy_ids_for_test(&db).await.unwrap(),
        legacy_ids
    );
    write_source(&db, PolicySource::Shadow, "t").await.unwrap();
    assert_eq!(
        CedarPdp::served_policy_ids_for_test(&db).await.unwrap(),
        legacy_ids,
        "shadow serves legacy"
    );
    write_source(&db, PolicySource::Compiled, "t")
        .await
        .unwrap();
    assert_eq!(
        CedarPdp::served_policy_ids_for_test(&db).await.unwrap(),
        compiled_ids,
        "compiled serves compiled"
    );

    // Fail-safe: compiled build errors => legacy served, no error.
    // Shadow with a compiled error also serves legacy.
    for src in [PolicySource::Compiled, PolicySource::Shadow] {
        assert_eq!(
            CedarPdp::select_served_for_test(&db, src, true)
                .await
                .expect("never errors"),
            legacy_ids,
            "{src:?}: compiled error must serve legacy"
        );
    }
    reset(&db).await;
}
