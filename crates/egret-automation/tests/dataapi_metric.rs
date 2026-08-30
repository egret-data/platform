//! `dataapi_metric::measure` against a real `dataapi.access_log`.
//!
//! DB-gated, self-skipping without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP` and
//! otherwise FAILING, following `crates/egret-automation/tests/dataapi_alert_schema.rs`'s
//! `setup()` -> `Option` idiom.
//!
//! Every test binds its own `seed_time` from a single `Utc::now()` read and
//! derives `now`/`since` from it arithmetically — never a second clock read —
//! for the same reason `dataapi_metric.rs` takes `now` as a parameter: two
//! readings that can coincide is a flake this repo has already recorded.

use chrono::{Duration, Utc};
use egret_automation::dataapi_metric::measure;
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    ws: Uuid,
}

async fn setup() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let ws = new_workspace(&db).await;
    Some(Fixture { db, ws })
}

/// A second, independent workspace in the same fixture's database — used by
/// the cross-workspace isolation test. Not part of `setup()` itself: most
/// tests need exactly one workspace, and creating a second unconditionally
/// would just be extra rows nobody reads.
async fn new_workspace(db: &Db) -> Uuid {
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();

    sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

macro_rules! fixture {
    () => {
        match setup().await {
            Some(f) => f,
            None => {
                eprintln!("skip: DATABASE_URL unset");
                return;
            }
        }
    };
}

/// One `access_log` row to seed. Everything not named defaults to a healthy,
/// non-trial, in-workspace 200 with a small duration — each test only sets
/// the fields its case is actually about.
struct Row {
    ts: chrono::DateTime<Utc>,
    workspace_id: Option<Uuid>,
    endpoint_id: Option<Uuid>,
    status_code: i32,
    duration_ms: i32,
    actor_user_id: Option<Uuid>,
    error_code: Option<String>,
}

impl Row {
    fn at(ts: chrono::DateTime<Utc>) -> Self {
        Row {
            ts,
            workspace_id: None,
            endpoint_id: None,
            status_code: 200,
            duration_ms: 10,
            actor_user_id: None,
            error_code: None,
        }
    }
}

async fn seed(f: &Fixture, r: Row) {
    sqlx::query(
        "INSERT INTO dataapi.access_log \
         (ts, workspace_id, endpoint_id, status_code, duration_ms, actor_user_id, \
          error_code, request_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(r.ts)
    .bind(r.workspace_id.unwrap_or(f.ws))
    .bind(r.endpoint_id)
    .bind(r.status_code)
    .bind(r.duration_ms)
    .bind(r.actor_user_id)
    .bind(r.error_code)
    .bind(Uuid::new_v4().to_string())
    .execute(&f.db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn error_rate_over_a_mixed_window() {
    let f = fixture!();
    let seed_time = Utc::now();
    seed(&f, Row::at(seed_time)).await;
    seed(&f, Row::at(seed_time)).await;
    seed(
        &f,
        Row {
            status_code: 500,
            ..Row::at(seed_time)
        },
    )
    .await;

    let now = seed_time + Duration::seconds(1);
    let v = measure(&f.db, f.ws, None, "error_rate", None, 3600, now)
        .await
        .unwrap()
        .expect("3 rows is not an empty window");
    assert!((v - 1.0 / 3.0).abs() < 1e-9, "got {v}");
}

#[tokio::test]
async fn error_rate_over_an_empty_window_is_none() {
    let f = fixture!();
    // Nothing seeded for this workspace: the window is empty by construction.
    let now = Utc::now();
    let v = measure(&f.db, f.ws, None, "error_rate", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(v, None, "a rate over zero rows is undefined, not zero");
}

/// Separate case from the one above on purpose: `error_rate`'s `None` and
/// `call_count`'s `Some(0.0)` are two different rules over the same empty
/// window, and one test passing says nothing about the other.
#[tokio::test]
async fn call_count_over_an_empty_window_is_some_zero() {
    let f = fixture!();
    let now = Utc::now();
    let v = measure(&f.db, f.ws, None, "call_count", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(v, Some(0.0), "zero calls IS a measurement, not an absence");
}

#[tokio::test]
async fn error_count_over_an_empty_window_is_some_zero() {
    let f = fixture!();
    let now = Utc::now();
    let v = measure(&f.db, f.ws, None, "error_count", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(v, Some(0.0));
}

#[tokio::test]
async fn error_code_count_over_an_empty_window_is_some_zero() {
    let f = fixture!();
    let now = Utc::now();
    let v = measure(
        &f.db,
        f.ws,
        None,
        "error_code_count",
        Some("QUERY_TIMEOUT"),
        3600,
        now,
    )
    .await
    .unwrap();
    assert_eq!(v, Some(0.0));
}

#[tokio::test]
async fn p50_and_p95_over_an_empty_window_are_none() {
    let f = fixture!();
    let now = Utc::now();
    let p50 = measure(&f.db, f.ws, None, "p50_duration_ms", None, 3600, now)
        .await
        .unwrap();
    let p95 = measure(&f.db, f.ws, None, "p95_duration_ms", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(p50, None);
    assert_eq!(p95, None);
}

/// The test that dies if someone drops `FILTER (WHERE status_code BETWEEN
/// 200 AND 299)` from the percentile query: a burst of fast 401s must not
/// drag p95 down at exactly the moment the system is failing.
#[tokio::test]
async fn p95_ignores_a_burst_of_401s() {
    let f = fixture!();
    let seed_time = Utc::now();
    for _ in 0..10 {
        seed(
            &f,
            Row {
                status_code: 401,
                duration_ms: 1,
                ..Row::at(seed_time)
            },
        )
        .await;
    }
    seed(
        &f,
        Row {
            status_code: 200,
            duration_ms: 900,
            ..Row::at(seed_time)
        },
    )
    .await;

    let now = seed_time + Duration::seconds(1);
    let v = measure(&f.db, f.ws, None, "p95_duration_ms", None, 3600, now)
        .await
        .unwrap()
        .expect("one successful call is a non-empty population");
    assert!(
        (v - 900.0).abs() < 1e-6,
        "p95 should reflect the single 200, got {v}"
    );
}

/// The test that dies if someone drops `actor_user_id IS NULL`: a Try-it run
/// is not consumer traffic.
#[tokio::test]
async fn trials_are_excluded() {
    let f = fixture!();
    let seed_time = Utc::now();
    seed(
        &f,
        Row {
            actor_user_id: Some(Uuid::new_v4()),
            ..Row::at(seed_time)
        },
    )
    .await;
    seed(&f, Row::at(seed_time)).await;

    let now = seed_time + Duration::seconds(1);
    let v = measure(&f.db, f.ws, None, "call_count", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(v, Some(1.0), "only the non-trial row should be counted");
}

#[tokio::test]
async fn error_code_count_counts_only_the_named_code() {
    let f = fixture!();
    let seed_time = Utc::now();
    seed(
        &f,
        Row {
            error_code: Some("QUERY_TIMEOUT".into()),
            status_code: 504,
            ..Row::at(seed_time)
        },
    )
    .await;
    seed(
        &f,
        Row {
            error_code: Some("QUERY_TIMEOUT".into()),
            status_code: 504,
            ..Row::at(seed_time)
        },
    )
    .await;
    seed(
        &f,
        Row {
            error_code: Some("FORBIDDEN".into()),
            status_code: 403,
            ..Row::at(seed_time)
        },
    )
    .await;

    let now = seed_time + Duration::seconds(1);
    let v = measure(
        &f.db,
        f.ws,
        None,
        "error_code_count",
        Some("QUERY_TIMEOUT"),
        3600,
        now,
    )
    .await
    .unwrap();
    assert_eq!(v, Some(2.0));
}

#[tokio::test]
async fn endpoint_id_none_aggregates_and_some_narrows() {
    let f = fixture!();
    let seed_time = Utc::now();
    let endpoint_a = Uuid::new_v4();
    let endpoint_b = Uuid::new_v4();
    seed(
        &f,
        Row {
            endpoint_id: Some(endpoint_a),
            ..Row::at(seed_time)
        },
    )
    .await;
    seed(
        &f,
        Row {
            endpoint_id: Some(endpoint_b),
            ..Row::at(seed_time)
        },
    )
    .await;
    seed(&f, Row::at(seed_time)).await; // endpoint_id NULL: unmatched request

    let now = seed_time + Duration::seconds(1);

    let whole_ws = measure(&f.db, f.ws, None, "call_count", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(whole_ws, Some(3.0));

    let narrowed = measure(&f.db, f.ws, Some(endpoint_a), "call_count", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(
        narrowed,
        Some(1.0),
        "endpoint b's and the unmatched row must not leak into endpoint a's count"
    );
}

#[tokio::test]
async fn a_row_from_another_workspace_never_counts() {
    let f = fixture!();
    let other_ws = new_workspace(&f.db).await;
    let seed_time = Utc::now();
    seed(
        &f,
        Row {
            workspace_id: Some(other_ws),
            ..Row::at(seed_time)
        },
    )
    .await;

    let now = seed_time + Duration::seconds(1);
    let v = measure(&f.db, f.ws, None, "call_count", None, 3600, now)
        .await
        .unwrap();
    assert_eq!(
        v,
        Some(0.0),
        "the other workspace's row must not be visible"
    );
}

/// The test that dies if `measure` ignores its `now` parameter and reads
/// `Utc::now()` internally instead. Every other test in this file calls
/// `measure` with a `now` within about a second of the real wall clock, so a
/// mutant that swapped the parameter for `Utc::now()` internally would pass
/// every one of them — the injected clock would be decoration nobody
/// actually exercises. This test displaces `now` ten days from the seeded
/// row's real timestamp and gives the window only five seconds of width, so
/// the row lands inside the window if and only if `measure` used the PASSED
/// `now` — the real wall clock, ten days later, would put the row nowhere
/// near a 5-second window ending "now".
#[tokio::test]
async fn now_is_the_parameter_not_the_wall_clock() {
    let f = fixture!();
    let seed_time = Utc::now() - Duration::days(10);
    seed(&f, Row::at(seed_time)).await;

    let now = seed_time + Duration::seconds(1);
    let v = measure(&f.db, f.ws, None, "call_count", None, 5, now)
        .await
        .unwrap();
    assert_eq!(
        v,
        Some(1.0),
        "the row is inside a 5s window anchored at the PASSED `now`, ten \
         days from the real clock; a `measure` reading Utc::now() internally \
         would see an empty window and report Some(0.0) here"
    );
}
