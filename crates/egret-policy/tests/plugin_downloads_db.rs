//! DB-gated tests for the plugin download ledger (view governance B4): what
//! each Ranger plugin downloaded, and whether every plugin holds a tag version.
//! Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and
//! otherwise FAIL.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_policy::{
    plugins_hold_policy_version, plugins_hold_tag_version, ranger_router, seed_starrocks, seed_tag,
    seed_trino, wait_for_tag_version, wait_for_versions, PolicyStore, RangerState, WaitError,
    NO_PLUGIN_POLLED,
};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "devtoken";
const RECENT: Duration = Duration::from_secs(90);

async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn app(store: PolicyStore) -> axum::Router {
    ranger_router(RangerState {
        store,
        auth_token: Arc::from(TOKEN),
        mode: egret_policy::ranger::AuthMode::Required,
        verifier: None,
    })
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

/// A fresh trino service (tag_version = 1) and the store around the same pool.
async fn setup(db: Db) -> (PolicyStore, sqlx::PgPool, String, i64) {
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino");
    seed_tag(&store).await.expect("seed tag");
    let name = format!("trino_{}", Uuid::new_v4().simple());
    let row = store
        .create_service(&name, "trino", &json!({}))
        .await
        .expect("create service");
    assert_eq!(row.tag_version, 1);
    (store, pool, name, row.id)
}

async fn poll(store: &PolicyStore, uri: &str) -> StatusCode {
    app(store.clone()).oneshot(get(uri)).await.unwrap().status()
}

type Row = (Option<i64>, Option<i64>);

async fn rows(pool: &sqlx::PgPool, service_id: i64, plugin: &str, kind: &str) -> Vec<Row> {
    sqlx::query_as(
        "SELECT last_known_version, served_version FROM policy.ranger_plugin_downloads \
         WHERE service_id = $1 AND plugin_id = $2 AND kind = $3",
    )
    .bind(service_id)
    .bind(plugin)
    .bind(kind)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_download_records_a_row_and_a_second_poll_upserts_it() {
    let Some(db) = db().await else { return };
    let (store, pool, svc, id) = setup(db).await;
    let plugin = format!("trino@host-{svc}");

    let uri = format!("/tags/download/{svc}?pluginId={plugin}&lastKnownVersion=-1");
    assert_eq!(poll(&store, &uri).await, StatusCode::OK);
    assert_eq!(
        rows(&pool, id, &plugin, "tags").await,
        vec![(Some(-1), Some(1))]
    );

    let uri = format!("/tags/download/{svc}?pluginId={plugin}&lastKnownVersion=1");
    assert_eq!(poll(&store, &uri).await, StatusCode::NOT_MODIFIED);
    // One row, now reporting 1; the served version of the earlier 200 is kept.
    assert_eq!(
        rows(&pool, id, &plugin, "tags").await,
        vec![(Some(1), Some(1))]
    );

    // The policies endpoint records under its own kind.
    let uri = format!("/plugins/policies/download/{svc}?pluginId={plugin}&lastKnownVersion=-1");
    assert_eq!(poll(&store, &uri).await, StatusCode::OK);
    assert_eq!(rows(&pool, id, &plugin, "policies").await.len(), 1);
    assert_eq!(rows(&pool, id, &plugin, "tags").await.len(), 1);
}

#[tokio::test]
async fn a_served_200_is_not_held_but_the_next_poll_reporting_it_is() {
    let Some(db) = db().await else { return };
    let (store, _pool, svc, id) = setup(db).await;
    let plugin = format!("trino@host-{svc}");

    let uri = format!("/tags/download/{svc}?pluginId={plugin}&lastKnownVersion=0");
    assert_eq!(poll(&store, &uri).await, StatusCode::OK); // serves version 1
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(
        !s.confirmed,
        "a version served on a 200 must not count as held"
    );
    assert_eq!(s.waiting_on, vec![plugin.clone()]);

    let uri = format!("/tags/download/{svc}?pluginId={plugin}&lastKnownVersion=1");
    assert_eq!(poll(&store, &uri).await, StatusCode::NOT_MODIFIED);
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(s.confirmed, "{s:?}");
    assert!(s.waiting_on.is_empty());
}

#[tokio::test]
async fn two_plugins_confirm_only_when_both_hold_the_version() {
    let Some(db) = db().await else { return };
    let (store, _pool, svc, id) = setup(db).await;
    let (a, b) = (format!("trino@a-{svc}"), format!("trino@b-{svc}"));

    for (p, v) in [(&a, 1), (&b, 0)] {
        let uri = format!("/tags/download/{svc}?pluginId={p}&lastKnownVersion={v}");
        poll(&store, &uri).await;
    }
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(!s.confirmed, "{s:?}");
    assert_eq!(s.waiting_on, vec![b.clone()]);

    let uri = format!("/tags/download/{svc}?pluginId={b}&lastKnownVersion=1");
    poll(&store, &uri).await;
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(s.confirmed, "{s:?}");
}

#[tokio::test]
async fn a_plugin_not_seen_within_recent_is_ignored() {
    let Some(db) = db().await else { return };
    let (store, pool, svc, id) = setup(db).await;
    let (live, gone) = (format!("trino@live-{svc}"), format!("trino@gone-{svc}"));

    for (p, v) in [(&live, 1), (&gone, 0)] {
        poll(
            &store,
            &format!("/tags/download/{svc}?pluginId={p}&lastKnownVersion={v}"),
        )
        .await;
    }
    sqlx::query(
        "UPDATE policy.ranger_plugin_downloads SET at = now() - interval '1 hour' \
         WHERE service_id = $1 AND plugin_id = $2",
    )
    .bind(id)
    .bind(&gone)
    .execute(&pool)
    .await
    .unwrap();
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(
        s.confirmed,
        "a plugin silent for an hour is not waited on: {s:?}"
    );

    // Once every plugin is stale there is nobody to confirm: fail closed.
    sqlx::query("UPDATE policy.ranger_plugin_downloads SET at = now() - interval '1 hour' WHERE service_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(!s.confirmed);
    assert_eq!(s.waiting_on, vec![NO_PLUGIN_POLLED.to_string()]);
}

#[tokio::test]
async fn with_no_plugin_seen_the_answer_is_not_confirmed() {
    let Some(db) = db().await else { return };
    let (store, _pool, _svc, id) = setup(db).await;
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(!s.confirmed);
    assert_eq!(s.waiting_on, vec!["<no plugin has polled>".to_string()]);
}

#[tokio::test]
async fn a_policies_poll_never_confirms_a_tag_version() {
    let Some(db) = db().await else { return };
    let (store, _pool, svc, id) = setup(db).await;
    let uri =
        format!("/plugins/policies/download/{svc}?pluginId=trino@h-{svc}&lastKnownVersion=99");
    poll(&store, &uri).await;
    let s = plugins_hold_tag_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(!s.confirmed, "{s:?}");
}

#[tokio::test]
async fn an_unknown_service_records_nothing_and_stays_404() {
    let Some(db) = db().await else { return };
    let (store, pool, _svc, _id) = setup(db).await;
    let plugin = format!("trino@h-unknown-{}", Uuid::new_v4().simple());
    for path in ["/tags/download", "/plugins/policies/download"] {
        let uri = format!("{path}/no_such_service_xyz?pluginId={plugin}&lastKnownVersion=1");
        assert_eq!(poll(&store, &uri).await, StatusCode::NOT_FOUND);
    }
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM policy.ranger_plugin_downloads WHERE plugin_id = $1")
            .bind(&plugin)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn an_absent_or_empty_plugin_id_records_nothing_and_still_serves() {
    let Some(db) = db().await else { return };
    let (store, pool, svc, id) = setup(db).await;
    for uri in [
        format!("/tags/download/{svc}?lastKnownVersion=-1"),
        format!("/tags/download/{svc}?pluginId=&lastKnownVersion=-1"),
        format!("/plugins/policies/download/{svc}?lastKnownVersion=-1"),
        format!("/plugins/policies/download/{svc}?pluginId="),
    ] {
        assert_eq!(poll(&store, &uri).await, StatusCode::OK, "{uri}");
    }
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM policy.ranger_plugin_downloads WHERE service_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0, "an anonymous poll has no identity to record");
}

#[tokio::test]
async fn wait_for_tag_version_times_out_naming_the_plugin_and_succeeds_when_it_catches_up() {
    let Some(db) = db().await else { return };
    let (store, _pool, svc, id) = setup(db).await;
    let plugin = format!("trino@h-{svc}");
    poll(
        &store,
        &format!("/tags/download/{svc}?pluginId={plugin}&lastKnownVersion=0"),
    )
    .await;

    let err = wait_for_tag_version(&store, &[(id, 1)], Duration::from_millis(700))
        .await
        .unwrap_err();
    match err {
        WaitError::Timeout { waiting_on } => assert_eq!(waiting_on, vec![plugin.clone()]),
        other => panic!("expected Timeout, got {other:?}"),
    }

    let s2 = store.clone();
    let uri = format!("/tags/download/{svc}?pluginId={plugin}&lastKnownVersion=1");
    let t = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        poll(&s2, &uri).await;
    });
    wait_for_tag_version(&store, &[(id, 1)], Duration::from_secs(10))
        .await
        .expect("the plugin caught up");
    t.await.unwrap();

    // Nothing to wait on is a refusal, never a vacuous success.
    assert!(matches!(
        wait_for_tag_version(&store, &[], Duration::from_millis(100)).await,
        Err(WaitError::Timeout { .. })
    ));
}

#[tokio::test]
async fn inline_upsert_latency() {
    let Some(db) = db().await else { return };
    let (store, _pool, svc, id) = setup(db).await;
    let plugin = format!("trino@h-{svc}");
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    const N: u32 = 50;
    for i in 0..N {
        let t = Instant::now();
        store
            .record_plugin_download(
                id,
                &plugin,
                egret_policy::DownloadKind::Tags,
                Some(i64::from(i)),
                None,
            )
            .await
            .unwrap();
        let d = t.elapsed();
        worst = worst.max(d);
        total += d;
    }
    eprintln!(
        "LATENCY upsert: mean {:?}, worst {:?} over {N}",
        total / N,
        worst
    );
}

async fn set_tag_version(pool: &sqlx::PgPool, id: i64, v: i64) {
    sqlx::query("UPDATE policy.authz_services SET tag_version = $2 WHERE id = $1")
        .bind(id)
        .bind(v)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn wait_uses_each_services_own_counter() {
    let Some(db) = db().await else { return };
    let (store, pool, a, a_id) = setup(db).await;
    let b = format!("trino_{}", Uuid::new_v4().simple());
    let b_id = store
        .create_service(&b, "trino", &json!({}))
        .await
        .unwrap()
        .id;
    set_tag_version(&pool, a_id, 10).await;
    set_tag_version(&pool, b_id, 3).await;
    let (pa, pb) = (format!("trino@h-{a}"), format!("trino@h-{b}"));
    poll(
        &store,
        &format!("/tags/download/{a}?pluginId={pa}&lastKnownVersion=10"),
    )
    .await;
    poll(
        &store,
        &format!("/tags/download/{b}?pluginId={pb}&lastKnownVersion=5"),
    )
    .await;
    let short = Duration::from_millis(600);

    // B's counter moves to 6: its plugin at 5 does not hold it, A's does hold 10.
    let err = wait_for_tag_version(&store, &[(a_id, 10), (b_id, 6)], short)
        .await
        .unwrap_err();
    match err {
        WaitError::Timeout { waiting_on } => assert_eq!(waiting_on, vec![pb.clone()]),
        other => panic!("expected Timeout, got {other:?}"),
    }
    // B still at counter 3 (plugin reports 5): confirmed, though 5 < A's 10.
    wait_for_tag_version(&store, &[(a_id, 10), (b_id, 3)], short)
        .await
        .expect("each service is judged against its own counter");
}

#[tokio::test]
async fn an_overlong_plugin_id_records_nothing_and_still_serves() {
    let Some(db) = db().await else { return };
    let (store, pool, svc, id) = setup(db).await;
    let long = "x".repeat(257);
    let ok = "y".repeat(256);
    assert_eq!(
        poll(
            &store,
            &format!("/tags/download/{svc}?pluginId={long}&lastKnownVersion=-1")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        poll(
            &store,
            &format!("/tags/download/{svc}?pluginId={ok}&lastKnownVersion=-1")
        )
        .await,
        StatusCode::OK
    );
    let ids: Vec<(String,)> = sqlx::query_as(
        "SELECT plugin_id FROM policy.ranger_plugin_downloads WHERE service_id = $1",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(ids, vec![(ok,)], "257 bytes refused, 256 recorded");
}

#[tokio::test]
async fn stale_rows_are_pruned_on_upsert_and_fresh_ones_kept() {
    let Some(db) = db().await else { return };
    let (store, pool, svc, id) = setup(db).await;
    let other = format!("trino_{}", Uuid::new_v4().simple());
    let other_id = store
        .create_service(&other, "trino", &json!({}))
        .await
        .unwrap()
        .id;
    for (sid, plugin) in [(id, "old"), (id, "fresh"), (other_id, "old-elsewhere")] {
        store
            .record_plugin_download(sid, plugin, egret_policy::DownloadKind::Tags, Some(1), None)
            .await
            .unwrap();
    }
    sqlx::query(
        "UPDATE policy.ranger_plugin_downloads SET at = now() - interval '25 hours' \
         WHERE plugin_id IN ('old', 'old-elsewhere')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // The prune runs on a fixed tick of this store's upserts; 64 in a row
    // always hit it. The tick is the store's, not the process's: with one
    // counter per process, another test's upserts took the 64th tick and the
    // prune ran for ITS service, not this one.
    for i in 0..64 {
        store
            .record_plugin_download(id, "fresh", egret_policy::DownloadKind::Tags, Some(i), None)
            .await
            .unwrap();
    }
    let left: Vec<(String,)> = sqlx::query_as(
        "SELECT plugin_id FROM policy.ranger_plugin_downloads \
         WHERE service_id = ANY($1) ORDER BY plugin_id",
    )
    .bind(vec![id, other_id])
    .fetch_all(&pool)
    .await
    .unwrap();
    // Pruning is per polled service: the other service's old row is untouched.
    assert_eq!(
        left,
        vec![("fresh".to_string(),), ("old-elsewhere".to_string(),)],
        "{svc}"
    );
}

/// A fresh StarRocks profile at `policy_version` `v`, on `store`.
async fn starrocks_at(store: &PolicyStore, pool: &sqlx::PgPool, v: i64) -> (String, i64) {
    seed_starrocks(store).await.expect("seed starrocks");
    let name = format!("starrocks_{}", Uuid::new_v4().simple());
    let id = store
        .create_service(&name, "starrocks", &json!({}))
        .await
        .expect("create service")
        .id;
    sqlx::query("UPDATE policy.authz_services SET policy_version = $2 WHERE id = $1")
        .bind(id)
        .bind(v)
        .execute(pool)
        .await
        .unwrap();
    (name, id)
}

/// The poll a StarRocks plugin sends (`pluginId`, `lastKnownVersion`, E24).
fn starrocks_poll(svc: &str, v: i64) -> String {
    format!(
        "/plugins/policies/download/{svc}?pluginId=starrocks@sr-fe-0-{svc}&lastKnownVersion={v}"
    )
}

#[tokio::test]
async fn a_starrocks_plugin_that_reported_the_policy_version_confirms_it() {
    let Some(db) = db().await else { return };
    let (store, pool, _trino, _) = setup(db).await;
    let (svc, id) = starrocks_at(&store, &pool, 7).await;
    let plugin = format!("starrocks@sr-fe-0-{svc}");

    poll(&store, &starrocks_poll(&svc, 6)).await;
    let s = plugins_hold_policy_version(&store, id, 7, RECENT)
        .await
        .unwrap();
    assert!(!s.confirmed, "{s:?}");
    assert_eq!(s.waiting_on, vec![plugin]);

    assert_eq!(
        poll(&store, &starrocks_poll(&svc, 7)).await,
        StatusCode::NOT_MODIFIED
    );
    let s = plugins_hold_policy_version(&store, id, 7, RECENT)
        .await
        .unwrap();
    assert!(s.confirmed, "{s:?}");
}

#[tokio::test]
async fn a_tag_poll_does_not_confirm_a_policy_version() {
    let Some(db) = db().await else { return };
    let (store, _pool, svc, id) = setup(db).await;
    let uri = format!("/tags/download/{svc}?pluginId=trino@h-{svc}&lastKnownVersion=99");
    poll(&store, &uri).await;
    let s = plugins_hold_policy_version(&store, id, 1, RECENT)
        .await
        .unwrap();
    assert!(!s.confirmed, "{s:?}");
    assert_eq!(s.waiting_on, vec![NO_PLUGIN_POLLED.to_string()]);
}

#[tokio::test]
async fn wait_for_versions_waits_for_both_kinds() {
    let Some(db) = db().await else { return };
    let (store, pool, trino, trino_id) = setup(db).await;
    let (sr, sr_id) = starrocks_at(&store, &pool, 4).await;
    poll(
        &store,
        &format!("/tags/download/{trino}?pluginId=trino@h-{trino}&lastKnownVersion=1"),
    )
    .await;
    poll(&store, &starrocks_poll(&sr, 3)).await;

    // The StarRocks plugin never catches up: a timeout naming it alone.
    let err = wait_for_versions(
        &store,
        &[(trino_id, 1)],
        &[(sr_id, 4)],
        Duration::from_millis(700),
    )
    .await
    .unwrap_err();
    match err {
        WaitError::Timeout { waiting_on } => {
            assert_eq!(waiting_on, vec![format!("starrocks@sr-fe-0-{sr}")])
        }
        other => panic!("expected Timeout, got {other:?}"),
    }

    // It reports late: confirmed only after that poll.
    let (s2, uri) = (store.clone(), starrocks_poll(&sr, 4));
    let polled_at = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        poll(&s2, &uri).await;
        Instant::now()
    });
    wait_for_versions(
        &store,
        &[(trino_id, 1)],
        &[(sr_id, 4)],
        Duration::from_secs(10),
    )
    .await
    .expect("both kinds confirmed");
    let confirmed_at = Instant::now();
    assert!(confirmed_at >= polled_at.await.unwrap());
}

#[tokio::test]
async fn an_empty_list_is_skipped_but_an_empty_wait_is_still_a_timeout_for_the_tag_wait() {
    let Some(db) = db().await else { return };
    let (store, pool, _trino, _) = setup(db).await;
    let (sr, sr_id) = starrocks_at(&store, &pool, 2).await;
    poll(&store, &starrocks_poll(&sr, 2)).await;

    // No Trino pair: only the policies are waited on (a StarRocks-only warehouse).
    wait_for_versions(&store, &[], &[(sr_id, 2)], Duration::from_millis(700))
        .await
        .expect("an empty tag list is skipped, not a timeout");
    wait_for_versions(&store, &[], &[], Duration::from_millis(100))
        .await
        .expect("nothing bound: nothing to wait for");
    // The view hold's contract is unchanged.
    assert!(matches!(
        wait_for_tag_version(&store, &[], Duration::from_millis(100)).await,
        Err(WaitError::Timeout { .. })
    ));
}
