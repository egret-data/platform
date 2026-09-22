//! SP3c-1 review F10: "an engine with catalogs and no primary is observable"
//! is a promise about a log line, and a promise about a log line is prose until
//! a subscriber reads it back.
//!
//! # The property
//!
//! "Exactly one primary per engine" has no anchor row for a CHECK.
//! `engine_catalog_attachments_primary_key` is a PARTIAL unique index, so it
//! gives *at most* one and ZERO stays representable — an engine SP3a's backfill
//! skipped, or a table written by hand. Nothing refuses that state, and its
//! consequence is silent: whichever catalog sorts first by attachment time
//! becomes Trino's `C0`. `list_engine_attachments` is the one place every render
//! path goes through, so it is where the anomaly is said out loud.
//!
//! The control matters as much as the assertion: a `list_engine_attachments`
//! that warned on EVERY call would satisfy "the anomaly is warned about"
//! perfectly, so the healthy engine in the same test must stay silent.
//!
//! # Why this is a whole binary for one test
//!
//! Same reason as `egret-dataapi/tests/serving_guard_logs.rs`: `tracing` caches
//! a callsite's `Interest` process-wide, and `set_default` is thread-local. In a
//! multi-test binary another test's subscriber can be the one that first
//! evaluates this `warn!` callsite; if it is not interested, the callsite is
//! cached disabled and this test's subscriber never sees the event. A binary
//! with one test has one subscriber, so the cache cannot be poisoned.
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::store::{apply_status, NewAttachment};
use egret_catalog_registry::{CatalogDef, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Collects `tracing` output so the test can assert on what was written.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn warehouse(db: &Db, ws: Uuid) -> Uuid {
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .bind(unique("f10-sc"))
    .fetch_one(&db.pool)
    .await
    .expect("storage");
    sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(unique("f10wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(&db.pool)
    .await
    .expect("warehouse")
}

async fn engine(db: &Db, wh: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id",
    )
    .bind(unique("f10-eng"))
    .bind(wh)
    .fetch_one(&db.pool)
    .await
    .expect("engine")
}

#[tokio::test]
async fn an_engine_with_catalogs_and_no_primary_is_warned_about() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = RegistryStore::new(db.clone());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'F10', 'active') RETURNING id",
    )
    .bind(unique("f10"))
    .fetch_one(&db.pool)
    .await
    .expect("workspace");

    // Two catalogs on a BROKEN engine: attachments, no primary. This is exactly
    // what an engine SP3a's backfill skipped looks like after migration 0150 —
    // the UPDATE matched nothing for it, and the partial unique index is happy.
    let broken_wh = warehouse(&db, ws).await;
    let broken = engine(&db, broken_wh).await;
    for _ in 0..2 {
        let c = store
            .create_catalog(
                &unique("bench"),
                &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
                None,
            )
            .await
            .unwrap();
        store
            .insert_attachment(&NewAttachment {
                engine_id: broken,
                catalog_id: c.id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: true,
                is_primary: false,
                created_at: None,
                apply_status: apply_status::APPLIED,
            })
            .await
            .unwrap();
    }

    // A HEALTHY engine, listed through the same call in the same capture.
    let healthy_wh = warehouse(&db, ws).await;
    let healthy = engine(&db, healthy_wh).await;
    let lead = store
        .create_catalog(
            &unique("bench"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();
    store
        .insert_attachment(&NewAttachment {
            engine_id: healthy,
            catalog_id: lead.id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: true,
            is_primary: true,
            created_at: None,
            apply_status: apply_status::APPLIED,
        })
        .await
        .unwrap();

    // An engine with NO attachments at all: not an anomaly, just an engine
    // mid-create. It must not be warned about either.
    let empty_wh = warehouse(&db, ws).await;
    let empty = engine(&db, empty_wh).await;

    let cap = LogCapture::default();
    let sub = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(cap.clone())
        .without_time()
        // The capture is a pipe, not a tty, but `fmt` colours by default here
        // and the escape codes land BETWEEN a field's name and its `=`.
        .with_ansi(false)
        .finish();
    let (broken_rows, healthy_rows, empty_rows) = {
        let _guard = tracing::subscriber::set_default(sub);
        (
            store.list_engine_attachments(broken).await.unwrap(),
            store.list_engine_attachments(healthy).await.unwrap(),
            store.list_engine_attachments(empty).await.unwrap(),
        )
    };
    assert_eq!(broken_rows.len(), 2);
    assert_eq!(healthy_rows.len(), 1);
    assert!(empty_rows.is_empty());

    let log = String::from_utf8(cap.0.lock().expect("log lock").clone()).expect("utf-8 log");
    assert!(
        log.contains(&broken.to_string()),
        "the broken engine is named in the warning; log={log}"
    );
    assert!(
        log.contains("no primary attachment"),
        "the warning says what is wrong; log={log}"
    );
    assert!(
        log.contains("attachments=2"),
        "and how many catalogs are affected; log={log}"
    );
    assert!(
        !log.contains(&healthy.to_string()),
        "an engine WITH a primary must stay silent — a call that warned every \
         time would pass the assertion above for the wrong reason; log={log}"
    );
    assert!(
        !log.contains(&empty.to_string()),
        "an engine with no attachments is not an anomaly; log={log}"
    );
}
