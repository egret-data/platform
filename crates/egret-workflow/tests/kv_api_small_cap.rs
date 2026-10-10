//! W9c Task 6 fix round 1 (review M12): `GET …/kv/{key}` of a stored value
//! over `max_read_bytes` answers 413 naming the cap, at the route. Its own
//! binary: the store is installed once per process, and this one installs a
//! `MemStore` with a small cap ([`CAP`]). The value is seeded through
//! `store::set` with a second `FileStore` over the SAME `MemStore` whose cap
//! is larger, as a value written before the cap was lowered.
//! Requires DATABASE_URL.

mod common;

use std::sync::Arc;

use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::kv::store::{self as kv_store, KvWritten, NewValue};
use egret_workflow::storage::kv::value::{KvData, KvType, KvValue};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Resource};
use egret_workflow::storage::nsfiles::CreatedBy;
use egret_workflow::storage::{self, FileStore};
use serde_json::{json, Value};

/// This binary's `max_read_bytes`.
const CAP: u64 = 1024;

#[tokio::test]
async fn get_of_a_stored_value_over_max_read_bytes_is_413_naming_the_cap() {
    let Some(db) = common::db().await else { return };
    let memory: Arc<dyn egret_storage::ObjectStore> = Arc::new(egret_storage::MemStore::new());
    let file_store = |max_read_bytes: u64| FileStore {
        store: memory.clone(),
        bucket: "egret-test".to_string(),
        prefix: "workflow/".to_string(),
        caps: storage::Caps {
            max_read_bytes,
            ..Default::default()
        },
        retention_days: 30,
    };
    storage::install(Arc::new(file_store(CAP))).expect("this binary installs the only store");
    egret_workflow::storage::nsfiles::install_db_for_tests(&db.pool);
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let ns = "w9c.cap";
    // Seeded under a cap of 1 MiB: a STRING of 2,000 bytes, 2,002 as Ion.
    let seeder = file_store(1 << 20);
    let grant = authorize(ws, Actor::Api, ns, Resource::Kv, Access::Write).unwrap();
    for (key, text) in [("big", "b".repeat(2000)), ("small", "s".to_string())] {
        let new = NewValue {
            value: KvValue {
                kind: KvType::String,
                data: KvData::String(text),
            },
            description: None,
            ttl: None,
            overwrite: true,
        };
        let written = kv_store::set(
            &db.pool,
            &seeder,
            &grant,
            &KvKey::parse(key).unwrap(),
            new,
            CreatedBy::User(user.0),
        )
        .await
        .unwrap();
        assert!(matches!(written, KvWritten::Written(_)), "{key}");
    }
    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/namespaces/{ns}/kv");
    let answer = |res: axum::response::Response| async move {
        let status = res.status().as_u16();
        let v: Value = serde_json::from_str(&common::body_text(res).await).unwrap();
        (status, v)
    };
    // The latest and a given revision: 413 naming the size and the cap.
    for rest in ["/big", "/big?revision=1"] {
        let (status, v) = answer(common::get(&app, &format!("{base}{rest}"), user).await).await;
        assert_eq!(status, 413, "{rest}: {v}");
        assert_eq!(
            v["error"],
            json!(format!(
                "revision 1 of key 'big' in namespace '{ns}' is 2002 bytes, more than \
                 workflow.storage.max_read_bytes ({CAP} bytes) lets egret read"
            )),
            "{rest}"
        );
    }
    // The control: a value under the cap is read; the big key is listed.
    let (status, v) = answer(common::get(&app, &format!("{base}/small"), user).await).await;
    assert_eq!((status, v["value"].clone()), (200, json!("s")));
    let (status, v) = answer(common::get(&app, &base, user).await).await;
    assert_eq!(status, 200);
    assert_eq!(v.as_array().map(Vec::len), Some(2), "{v}");
}
