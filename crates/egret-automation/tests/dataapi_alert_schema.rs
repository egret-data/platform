//! Migration 0127's CHECK constraints, proven against a real database.
//!
//! Every assertion here inserts through raw SQL and checks what Postgres
//! itself says — never what `ScheduleRepo`/`NewSchedule` would have refused,
//! because a Rust-side guard passes even with the CHECK deleted. `NewSchedule`
//! does not yet carry the four `dataapi_*` fields (that is a later task in
//! this decomposition), so the only way to exercise the constraints is
//! `sqlx::query` directly against `automation.schedules`.
//!
//! DB-gated, self-skipping without `DATABASE_URL`, following
//! `crates/egret-automation/tests/schedule_repo.rs`'s `setup()` -> `Option`
//! idiom.

use uuid::Uuid;

struct Fixture {
    pool: sqlx::PgPool,
    ws: Uuid,
}

async fn setup() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");

    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Some(Fixture { pool: db.pool, ws })
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

/// One attempted row. Everything not named here is NULL/default, so each
/// test only sets the columns its case is actually about.
#[derive(Default)]
struct Row<'a> {
    r#type: &'a str,
    database_id: Option<Uuid>,
    sql: Option<&'a str>,
    validator_type: Option<&'a str>,
    dataapi_metric: Option<&'a str>,
    dataapi_error_code: Option<&'a str>,
    dataapi_window_s: Option<i32>,
}

/// A well-formed `dataapi_alert` row — the baseline every rejection test
/// perturbs exactly one field away from.
fn well_formed() -> Row<'static> {
    Row {
        r#type: "dataapi_alert",
        validator_type: Some("operator"),
        dataapi_metric: Some("call_count"),
        dataapi_window_s: Some(300),
        ..Default::default()
    }
}

async fn try_insert(f: &Fixture, name: &str, r: Row<'_>) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO automation.schedules \
         (workspace_id, type, name, crontab, database_id, sql, validator_type, \
          dataapi_metric, dataapi_error_code, dataapi_window_s) \
         VALUES ($1,$2,$3,'0 9 * * *',$4,$5,$6,$7,$8,$9) RETURNING id",
    )
    .bind(f.ws)
    .bind(r.r#type)
    .bind(name)
    .bind(r.database_id)
    .bind(r.sql)
    .bind(r.validator_type)
    .bind(r.dataapi_metric)
    .bind(r.dataapi_error_code)
    .bind(r.dataapi_window_s)
    .fetch_one(&f.pool)
    .await
}

/// Assert the insert was rejected by the NAMED constraint — not merely that
/// it failed, which would also be true of a typo in the test itself.
fn assert_rejected_by(err: sqlx::Error, constraint: &str) {
    let db_err = err.as_database_error().expect("a database error");
    assert_eq!(
        db_err.constraint(),
        Some(constraint),
        "expected rejection by {constraint}, got: {db_err}"
    );
}

#[tokio::test]
async fn dataapi_alert_carrying_sql_is_rejected() {
    let f = fixture!();
    let mut r = well_formed();
    r.sql = Some("SELECT 1");
    let err = try_insert(&f, "carries sql", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_needs_source");
}

async fn a_database(f: &Fixture) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(f.ws)
    .fetch_one(&f.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn dataapi_alert_carrying_database_id_is_rejected() {
    let f = fixture!();
    let db_id = a_database(&f).await;

    let mut r = well_formed();
    r.database_id = Some(db_id);
    let err = try_insert(&f, "carries database_id", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_needs_source");
}

/// Fix-wave finding 2: `dataapi_window_s` was bounded below (`> 0`) but not
/// above, so nothing in the database stopped a window that silently measures
/// whatever survived `access_log` retention rather than reading the whole
/// window an operator thinks they configured. `well_formed()`'s own `300` is
/// far under the bound, so this perturbs only the field under test — the
/// same one-field-away shape every other case here follows. Model-side
/// (`model.rs::a_dataapi_alert_window_over_thirty_days_names_the_field`)
/// proves the same bound is enforced before the database ever sees it; this
/// proves the CHECK itself still holds if that Rust-side guard is ever
/// deleted, per this file's own module doc.
#[tokio::test]
async fn dataapi_alert_with_a_window_over_thirty_days_is_rejected() {
    let f = fixture!();
    let mut r = well_formed();
    r.dataapi_window_s = Some(2_592_001);
    let err = try_insert(&f, "window over 30 days", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_needs_source");
}

#[tokio::test]
async fn dataapi_alert_with_null_metric_is_rejected() {
    let f = fixture!();
    let mut r = well_formed();
    r.dataapi_metric = None;
    let err = try_insert(&f, "null metric", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_needs_source");
}

#[tokio::test]
async fn dataapi_alert_with_unknown_metric_is_rejected() {
    let f = fixture!();
    let mut r = well_formed();
    r.dataapi_metric = Some("no_such_metric");
    let err = try_insert(&f, "unknown metric", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_metric_valid");
}

#[tokio::test]
async fn error_code_without_error_code_count_metric_is_rejected() {
    let f = fixture!();
    let mut r = well_formed();
    r.dataapi_metric = Some("call_count");
    r.dataapi_error_code = Some("QUERY_TIMEOUT");
    let err = try_insert(&f, "stray error_code", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_error_code_needs_metric");
}

#[tokio::test]
async fn error_code_count_metric_without_error_code_is_rejected() {
    let f = fixture!();
    let mut r = well_formed();
    r.dataapi_metric = Some("error_code_count");
    r.dataapi_error_code = None;
    let err = try_insert(&f, "missing error_code", r).await.unwrap_err();
    assert_rejected_by(err, "schedule_dataapi_metric_needs_error_code");
}

#[tokio::test]
async fn plain_alert_carrying_dataapi_metric_is_rejected() {
    let f = fixture!();
    // Otherwise-well-formed alert (schedule_alert_needs_query's own
    // requirements met) so the failure proves schedule_alert_no_dataapi_columns
    // fired, not the pre-existing constraint.
    let db_id = a_database(&f).await;
    let r = Row {
        r#type: "alert",
        database_id: Some(db_id),
        sql: Some("SELECT 1"),
        validator_type: Some("not_null"),
        dataapi_metric: Some("call_count"),
        ..Default::default()
    };
    let err = try_insert(&f, "alert with dataapi_metric", r)
        .await
        .unwrap_err();
    assert_rejected_by(err, "schedule_alert_no_dataapi_columns");
}

#[tokio::test]
async fn a_well_formed_dataapi_alert_is_accepted() {
    let f = fixture!();
    let id = try_insert(&f, "well formed", well_formed())
        .await
        .expect("a well-formed dataapi_alert row must be accepted");

    let (ty, metric, window): (String, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT type, dataapi_metric, dataapi_window_s FROM automation.schedules WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(ty, "dataapi_alert");
    assert_eq!(metric.as_deref(), Some("call_count"));
    assert_eq!(window, Some(300));
}
