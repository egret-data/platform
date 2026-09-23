//! The database-error backstop in `EngineHttpError`: a constraint violation is a
//! 409 naming the constraint, anything else stays a redacted 500, and both are
//! LOGGED with their SQLSTATE — "internal error with nothing in the logs" is the
//! defect (spec D3).
//!
//! One test in its own binary on purpose: `tracing` caches callsite interest per
//! process, and another test's subscriber evaluating the callsite first can make
//! this one's subscriber miss the event (see
//! `crates/egret-dataapi/tests/serving_guard_logs.rs`).
//!
//! DB-gated: FAILS without `DATABASE_URL` unless `EGRET_ALLOW_DB_SKIP` is set.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use egret_engine_mgmt::{EngineHttpError, EngineMgmtError, EngineMgmtStore};
use egret_metastore::Db;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).expect("utf-8 log")
    }
}

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Convert with a subscriber installed; the log is written inside `From`.
fn convert(e: EngineMgmtError) -> (Response, String) {
    let cap = Capture::default();
    let sub = tracing_subscriber::fmt()
        .with_writer(cap.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let resp = tracing::subscriber::with_default(sub, || EngineHttpError::from(e).into_response());
    (resp, cap.text())
}

async fn json(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn database_errors_are_logged_and_constraint_violations_are_409s() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = EngineMgmtStore::new(db.clone());

    // Seed an engine + a user so the ONLY violated FK is the workspace one.
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'Err', 'active') RETURNING id",
    )
    .bind(format!("err-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("err-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let _wh = egret_catalog_engine::CatalogStore::new(db.clone())
        .create_warehouse(&format!("err-wh-{}", Uuid::new_v4().simple()), ws, sc)
        .await
        .unwrap();
    let engine = store
        .create_engine(
            &format!("err-eng-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap()
        .id;
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("err_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // 23503: assigning to a workspace that does not exist.
    let fk = store
        .assign_workspace(engine, Uuid::new_v4(), user)
        .await
        .expect_err("unknown workspace must violate the FK");
    let (resp, log) = convert(fk);
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = json(resp).await;
    assert_eq!(body["code"], "constraint_violation", "{body}");
    assert_eq!(
        body["constraint"], "engine_workspaces_workspace_id_fkey",
        "{body}"
    );
    assert!(
        log.contains("23503") && log.contains("engine_workspaces_workspace_id_fkey"),
        "{log}"
    );

    // 23514: an alert with no query.
    let check = sqlx::query(
        "INSERT INTO automation.schedules (workspace_id, type, name, crontab) \
         VALUES ($1, 'alert', 'no-query', '0 9 * * *')",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect_err("CHECK must reject an alert without a query");
    let (resp, log) = convert(EngineMgmtError::Db(check));
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(json(resp).await["constraint"], "schedule_alert_needs_query");
    assert!(log.contains("23514"), "{log}");

    // Anything else stays a redacted 500 — but is no longer silent.
    let other = sqlx::query("SELECT 1 FROM engine_mgmt.no_such_table")
        .execute(&db.pool)
        .await
        .expect_err("undefined table");
    let (resp, log) = convert(EngineMgmtError::Db(other));
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        json(resp).await,
        serde_json::json!({ "error": "internal error" })
    );
    assert!(
        log.contains("42P01") && log.contains("no_such_table"),
        "{log}"
    );
}
