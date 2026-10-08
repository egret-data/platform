//! W9c Tasks 2 and 3: the read side of `workflow.kv_entries` (migration
//! 0175) and the objects behind it, and the writes under each key's lock
//! (`set`, `put_merge`, `delete`, `delete_many`, `purge_key`). Requires
//! DATABASE_URL.
//!
//! Task 2's rows are inserted with plain SQL; Task 3's go through the
//! writers. Every test makes its own workspaces, so the tests cannot see
//! each other's rows. The only object stores touched are `MemStore`s:
//! nothing here deletes from a shared bucket. The expiry sweep, which is
//! global, has its own binary (`tests/kv_sweep.rs`).
//!
//! The fixture is `oracle-exec/w9c-kv.json`; a test asserts egret's answer,
//! the top-level value of a cell, never its `kestra`.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::{MemStore, ObjectStore, StorageError};
use egret_workflow::storage;
use egret_workflow::storage::kv::key::{check_key, KvKey};
use egret_workflow::storage::kv::store::{
    ancestors, list_inherited, list_live, namespaces_with_kv, read_value, resolve,
    resolve_first_live, revisions, rows_page, KvEntry,
};
use egret_workflow::storage::kv::value::{encode, KvData, KvType, KvValue};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use egret_workflow::storage::nsfiles::store::Want;
use egret_workflow::storage::nsfiles::NsError;
use serde_json::Value;
use uuid::Uuid;

const NS: &str = "w9c.t2";

fn fixture() -> Value {
    serde_json::from_str(include_str!("../oracle-exec/w9c-kv.json")).expect("the fixture")
}

fn grant(ws: Uuid, ns: &str) -> Grant {
    authorize(ws, Actor::Api, ns, Resource::Kv, Access::Read).unwrap()
}

fn key(k: &str) -> KvKey {
    KvKey::parse(k).unwrap()
}

fn keys(v: &[KvEntry]) -> Vec<&str> {
    v.iter().map(|e| e.key.as_str()).collect()
}

async fn conn(db: &Db) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    db.pool.acquire().await.unwrap()
}

/// One revision row. `expires_in` is seconds from the database's `now()`
/// (negative: already expired); `None` never expires.
#[derive(Clone, Copy)]
struct R<'a> {
    ns: &'a str,
    key: &'a str,
    rev: i32,
    deleted: bool,
    expires_in: Option<f64>,
    size: i64,
}

fn r<'a>(ns: &'a str, key: &'a str, rev: i32) -> R<'a> {
    R {
        ns,
        key,
        rev,
        deleted: false,
        expires_in: None,
        size: 1,
    }
}

impl R<'_> {
    fn deleted(mut self) -> Self {
        self.deleted = true;
        self
    }
    fn expires_in(mut self, s: f64) -> Self {
        self.expires_in = Some(s);
        self
    }
    fn size(mut self, n: i64) -> Self {
        self.size = n;
        self
    }
}

async fn insert(db: &Db, ws: Uuid, row: R<'_>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflow.kv_entries \
           (workspace_id, namespace, key, revision, value_type, size, expires_at, deleted, \
            deleted_at) \
         VALUES ($1, $2, $3, $4, 'STRING', $5, \
                 CASE WHEN $6::float8 IS NULL THEN NULL \
                      ELSE now() + make_interval(secs => $6::float8) END, \
                 $7, CASE WHEN $7 THEN now() END) \
         RETURNING id",
    )
    .bind(ws)
    .bind(row.ns)
    .bind(row.key)
    .bind(row.rev)
    .bind(row.size)
    .bind(row.expires_in)
    .bind(row.deleted)
    .fetch_one(&db.pool)
    .await
    .unwrap_or_else(|e| panic!("insert {}:{} r{}: {e}", row.ns, row.key, row.rev))
}

fn mem_fs() -> (storage::FileStore, Arc<MemStore>) {
    let mem = Arc::new(MemStore::new());
    let fs = storage::FileStore {
        store: mem.clone(),
        bucket: "b".into(),
        prefix: "workflow/".into(),
        caps: storage::Caps::default(),
        retention_days: 30,
    };
    (fs, mem)
}

/// A row and its object: a STRING value `text`.
async fn insert_value(db: &Db, fs: &storage::FileStore, ws: Uuid, row: R<'_>, text: &str) -> i64 {
    let bytes = encode(&KvValue {
        kind: KvType::String,
        data: KvData::String(text.into()),
    })
    .unwrap();
    let id = insert(db, ws, row.size(bytes.len() as i64)).await;
    let object = fs.namespace_kv_uri(&grant(ws, row.ns), &key(row.key), row.rev as u32);
    fs.store.put(&object, bytes).await.unwrap();
    id
}

async fn soft_delete_latest(db: &Db, ws: Uuid, ns: &str, k: &str) {
    sqlx::query(
        "UPDATE workflow.kv_entries SET deleted = true, deleted_at = now() \
         WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND revision = \
           (SELECT max(revision) FROM workflow.kv_entries \
             WHERE workspace_id = $1 AND namespace = $2 AND key = $3)",
    )
    .bind(ws)
    .bind(ns)
    .bind(k)
    .execute(&db.pool)
    .await
    .unwrap();
}

fn not_found_message(err: &NsError) -> &str {
    match err {
        NsError::NotFound(m) => m,
        other => panic!("not NotFound: {other:?}"),
    }
}

#[tokio::test]
async fn latest_is_the_highest_live_revision() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for (rev, size) in [(1, 10), (3, 30), (2, 20)] {
        insert(&db, ws, r(NS, "k", rev).size(size)).await;
    }
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    let e = resolve(&mut c, &g, &key("k"), Want::Latest).await.unwrap();
    assert_eq!(
        (e.revision, e.size, e.deleted, e.expired, e.value_type),
        (3, 30, false, false, KvType::String)
    );
    assert_eq!((e.workspace, e.namespace.as_str()), (ws, NS));
    let e = resolve(&mut c, &g, &key("k"), Want::Revision(2))
        .await
        .unwrap();
    assert_eq!((e.revision, e.size), (2, 20));
    let err = resolve(&mut c, &g, &key("k"), Want::Revision(4))
        .await
        .unwrap_err();
    assert_eq!(
        not_found_message(&err),
        format!("revision 4 of key 'k' does not exist in namespace '{NS}'")
    );
    let err = resolve(&mut c, &g, &key("nope"), Want::Latest)
        .await
        .unwrap_err();
    assert_eq!(
        not_found_message(&err),
        format!("key 'nope' does not exist in namespace '{NS}'")
    );
}

/// R1: revisions 1..3 written, 3 soft-deleted, then 4 written.
#[tokio::test]
async fn a_deleted_latest_is_not_found_and_each_revision_is_served() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for rev in 1..=3 {
        insert(&db, ws, r(NS, "k", rev).size(rev.into())).await;
    }
    soft_delete_latest(&db, ws, NS, "k").await;
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    let err = resolve(&mut c, &g, &key("k"), Want::Latest)
        .await
        .unwrap_err();
    let never = resolve(&mut c, &g, &key("never"), Want::Latest)
        .await
        .unwrap_err();
    assert_eq!(
        not_found_message(&err),
        not_found_message(&never).replace("'never'", "'k'"),
        "a deleted key reads as a key never written"
    );
    for rev in 1..=3u32 {
        let e = resolve(&mut c, &g, &key("k"), Want::Revision(rev))
            .await
            .unwrap();
        assert_eq!((e.revision, e.deleted), (rev, rev == 3), "{rev}");
    }
    let all = revisions(&mut c, &g, &key("k")).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|e| (e.revision, e.deleted))
            .collect::<Vec<_>>(),
        [(1, false), (2, false), (3, true)]
    );
    assert!(list_live(&mut c, &g, None, 10).await.unwrap().is_empty());
    // The rewrite after the delete: revision 4, live.
    insert(&db, ws, r(NS, "k", 4)).await;
    let e = resolve(&mut c, &g, &key("k"), Want::Latest).await.unwrap();
    assert_eq!((e.revision, e.deleted), (4, false));
}

#[tokio::test]
async fn an_expired_latest_reads_exactly_as_a_key_never_written() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    insert(&db, ws, r(NS, "k", 1)).await;
    insert(&db, ws, r(NS, "k", 2).expires_in(-1.0)).await;
    insert(&db, ws, r(NS, "later", 1).expires_in(3600.0)).await;
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    let err = resolve(&mut c, &g, &key("k"), Want::Latest)
        .await
        .unwrap_err();
    let never = resolve(&mut c, &g, &key("never"), Want::Latest)
        .await
        .unwrap_err();
    assert_eq!(
        not_found_message(&err),
        not_found_message(&never).replace("'never'", "'k'")
    );
    // The expired revision is missing; the older one (no expiry) is served.
    let err = resolve(&mut c, &g, &key("k"), Want::Revision(2))
        .await
        .unwrap_err();
    let never = resolve(&mut c, &g, &key("k"), Want::Revision(9))
        .await
        .unwrap_err();
    assert_eq!(
        not_found_message(&err),
        not_found_message(&never).replace("revision 9", "revision 2")
    );
    assert_eq!(
        resolve(&mut c, &g, &key("k"), Want::Revision(1))
            .await
            .unwrap()
            .revision,
        1
    );
    // Not resurrected by the walk either.
    assert!(
        resolve_first_live(&mut c, std::slice::from_ref(&g), &key("k"))
            .await
            .unwrap()
            .is_none()
    );
    // A future expiry is live.
    let e = resolve(&mut c, &g, &key("later"), Want::Latest)
        .await
        .unwrap();
    assert!(e.live() && e.expires_at.is_some());
    assert_eq!(
        keys(&list_live(&mut c, &g, None, 10).await.unwrap()),
        ["later"]
    );
    // revisions flags it.
    let all = revisions(&mut c, &g, &key("k")).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|e| (e.revision, e.expired))
            .collect::<Vec<_>>(),
        [(1, false), (2, true)]
    );
}

/// Expired is judged by the DATABASE's clock (§4.5): `now()` of the reading
/// statement's transaction. Inside one transaction `now()` is frozen at its
/// start, so a row whose `expires_at` is 300 ms after that start is live to
/// every read of the transaction -- even 600 ms later, when this process's
/// clock (`Utc::now()`) has passed it. A reader that judged expiry by its
/// own clock would call it expired.
#[tokio::test]
async fn expiry_is_judged_by_the_databases_clock() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let g = grant(ws, NS);
    // The brief's form: now() + 200 ms, read at once. Live by both clocks
    // unless they disagree by 200 ms: it cannot tell the two apart.
    sqlx::query(
        "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
         size, expires_at) VALUES ($1, $2, 'soon', 1, 'STRING', 1, now() + interval '200 ms')",
    )
    .bind(ws)
    .bind(NS)
    .execute(&db.pool)
    .await
    .unwrap();
    let mut c = conn(&db).await;
    assert!(resolve(&mut c, &g, &key("soon"), Want::Latest)
        .await
        .is_ok());
    drop(c);

    // The instrument that separates them.
    let mut tx = db.pool.begin().await.unwrap();
    let expires: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
         size, expires_at) VALUES ($1, $2, 'k', 1, 'STRING', 1, now() + interval '300 ms') \
         RETURNING expires_at",
    )
    .bind(ws)
    .bind(NS)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        chrono::Utc::now() > expires,
        "this process's clock has passed expires_at"
    );
    let e = resolve(&mut tx, &g, &key("k"), Want::Latest)
        .await
        .expect("live by the transaction's now()");
    assert!(!e.expired);
    assert_eq!(
        keys(&list_live(&mut tx, &g, None, 10).await.unwrap()),
        ["soon", "k"]
    );
    tx.rollback().await.unwrap();
}

/// Every column of every row of `ws` (with `xmin` and `ctid`, which any
/// UPDATE changes even to the same values), hashed, and the row count.
async fn table_state(db: &Db, ws: Uuid) -> (i64, String) {
    sqlx::query_as(
        "SELECT count(*), coalesce(md5(string_agg(t::text || '|' || t.xmin::text || '|' || \
           t.ctid::text, ',' ORDER BY t.id)), '') \
         FROM workflow.kv_entries t WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_read_never_writes() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, mem) = mem_fs();
    let child = "w9c.t2.c";
    insert_value(&db, &fs, ws, r(NS, "live", 1), "l").await;
    insert_value(&db, &fs, ws, r(NS, "gone", 1), "g").await;
    soft_delete_latest(&db, ws, NS, "gone").await;
    insert_value(&db, &fs, ws, r(NS, "old", 1), "o1").await;
    insert_value(&db, &fs, ws, r(NS, "old", 2).expires_in(-5.0), "o2").await;
    insert_value(&db, &fs, ws, r(child, "old", 1).expires_in(-5.0), "c").await;
    let before = (table_state(&db, ws).await, {
        let mut k = mem.keys();
        k.sort();
        k
    });
    assert_eq!(before.0 .0, 5);

    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    let gc = grant(ws, child);
    for k in ["live", "gone", "old"] {
        let _ = resolve(&mut c, &g, &key(k), Want::Latest).await;
        for n in 1..=2 {
            if let Ok(e) = resolve(&mut c, &g, &key(k), Want::Revision(n)).await {
                read_value(&db.pool, &fs, &g, &e).await.unwrap();
            }
        }
        let _ = resolve_first_live(&mut c, &[gc.clone(), g.clone()], &key(k)).await;
        let _ = revisions(&mut c, &g, &key(k)).await;
    }
    for e in revisions(&mut c, &g, &key("old")).await.unwrap() {
        // An expired revision's value, read directly: still no write.
        read_value(&db.pool, &fs, &g, &e).await.unwrap();
    }
    list_live(&mut c, &g, None, 10).await.unwrap();
    list_live(&mut c, &gc, Some("o"), 10).await.unwrap();
    list_inherited(&mut c, child, std::slice::from_ref(&g), 10)
        .await
        .unwrap();
    rows_page(&mut c, &g, None, 10).await.unwrap();
    namespaces_with_kv(&mut c, ws, "w9c").await.unwrap();
    drop(c);

    let after = (table_state(&db, ws).await, {
        let mut k = mem.keys();
        k.sort();
        k
    });
    assert_eq!(before, after);
}

/// Round trips through the database, counted from sqlx's own statement
/// events (target `sqlx::query`, one per executed statement).
/// Counts `sqlx::query` events on the threads that registered a counter
/// (W9c Task 3 fix round 1). It is the binary's GLOBAL subscriber, installed
/// once: a thread-scoped `set_default` subscriber lost events whenever
/// another test's thread had hit sqlx's callsites first (their cached
/// interest ignored it), which the writes' longer tests made frequent.
struct CountStatements;

thread_local! {
    static COUNTER: std::cell::RefCell<Option<Arc<AtomicUsize>>> =
        const { std::cell::RefCell::new(None) };
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountStatements {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            COUNTER.with(|c| {
                if let Some(n) = c.borrow().as_ref() {
                    n.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    }
}

/// A counter of this thread's statements, the global subscriber installed.
fn count_statements() -> Arc<AtomicUsize> {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        use tracing_subscriber::layer::SubscriberExt as _;
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(CountStatements),
        )
        .expect("no other global subscriber in this binary");
    });
    let count = Arc::new(AtomicUsize::new(0));
    COUNTER.with(|c| *c.borrow_mut() = Some(count.clone()));
    count
}

const INH_OWN: &str = "w9c.inh.a.b";

/// The fixture's `inheritance._setup`, with each value `{key}@{label}`
/// (`label` is the namespace without `w9c.`, or `w9c` itself), as the cells'
/// outputs name them.
async fn seed_inheritance(db: &Db, fs: &storage::FileStore, ws: Uuid) {
    let label = |ns: &str| -> String {
        match ns.strip_prefix("w9c.") {
            Some(rest) => rest.strip_prefix("inh.").unwrap_or(rest).to_string(),
            None => ns.to_string(),
        }
    };
    const DEEP: [&str; 6] = ["deep1", "deep2", "deep3", "deep4", "deep5", "deep6"];
    let mut live = vec![
        ("w9c.inh", "top"),
        ("w9c.inh.a", "mid"),
        ("w9c.inh", "mid"),
        ("w9c.inh.a.b", "own"),
        ("w9c.inh", "own"),
        ("w9c.inh.a.b.c", "child"),
        ("w9c", "root"),
        ("w9c.inh", "delmid"),
        ("w9c.inh", "midexp"),
    ];
    live.extend(DEEP.map(|k| ("w9c.inh", k)));
    for (ns, k) in live {
        let text = format!("{k}@{}", label(ns));
        insert_value(db, fs, ws, r(ns, k, 1), &text).await;
    }
    insert_value(db, fs, ws, r("w9c.inh.a", "delmid", 1), "delmid@a").await;
    soft_delete_latest(db, ws, "w9c.inh.a", "delmid").await;
    insert_value(
        db,
        fs,
        ws,
        r("w9c.inh.a", "midexp", 1).expires_in(-1.0),
        "midexp@a",
    )
    .await;
    for k in DEEP {
        let text = format!("{k}@a.b");
        insert_value(db, fs, ws, r(INH_OWN, k, 1).expires_in(-1.0), &text).await;
    }
}

#[tokio::test]
async fn the_walk_answers_the_deepest_live_level_and_skips_an_expired_one() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, _) = mem_fs();
    seed_inheritance(&db, &fs, ws).await;
    let f = fixture();
    let cells = &f["egret_expected"]["inheritance"];
    // (cell, key, None = walk from the flow's own namespace, Some(ns) = that
    // namespace only). `get_top_ns_own` walks: its namespace IS the flow's.
    let table: &[(&str, &str, Option<&str>)] = &[
        ("kv_top", "top", None),
        ("kv_mid", "mid", None),
        ("kv_own", "own", None),
        ("kv_root", "root", None),
        ("kv_child", "child", None),
        ("kv_none", "none", None),
        ("kv_none_eom_false", "none", None),
        ("kv_top_explicit_own", "top", Some(INH_OWN)),
        ("kv_top_explicit_parent", "top", Some("w9c.inh")),
        ("kv_mid_explicit_mid", "mid", Some("w9c.inh.a")),
        ("kv_delmid", "delmid", None),
        ("kv_none_explicit_other", "none", Some("w9c.inh.a")),
        ("get_top", "top", None),
        ("get_top_ns_own", "top", None),
        ("get_top_ns_parent_N1", "top", Some("w9c.inh.a")),
        ("get_top_ns_parent_N0", "top", Some("w9c.inh")),
        ("get_mid", "mid", None),
        ("get_none_eom_true", "none", None),
        ("get_none_eom_true_ns_parent", "none", Some("w9c.inh.a")),
        ("get_delmid", "delmid", None),
        ("kv_deep_default", "deep1", None),
        ("kv_deep_eom_false", "deep2", None),
        ("get_deep", "deep3", None),
        ("get_deep_eom_true", "deep4", None),
        ("kv_midexp", "midexp", None),
    ];
    let walk: Vec<Grant> = ancestors(INH_OWN)
        .into_iter()
        .map(|ns| {
            authorize(
                ws,
                Actor::Flow { namespace: INH_OWN },
                ns,
                Resource::Kv,
                Access::Read,
            )
            .unwrap()
        })
        .collect();
    let mut c = conn(&db).await;
    for (cell, k, explicit) in table {
        let expected = cells[*cell]["outputs"]["value"].as_str();
        let got = match explicit {
            None => match resolve_first_live(&mut c, &walk, &key(k)).await.unwrap() {
                Some(e) => {
                    let g = walk.iter().find(|g| g.namespace() == e.namespace).unwrap();
                    Some(read_value(&db.pool, &fs, g, &e).await.unwrap().to_json())
                }
                None => None,
            },
            Some(ns) => {
                let g = grant(ws, ns);
                match resolve(&mut c, &g, &key(k), Want::Latest).await {
                    Ok(e) => Some(read_value(&db.pool, &fs, &g, &e).await.unwrap().to_json()),
                    Err(NsError::NotFound(_)) => None,
                    Err(other) => panic!("{cell}: {other:?}"),
                }
            }
        };
        let got = got.as_ref().map(|v| v.as_str().unwrap().to_string());
        // `kv_none_eom_false`'s "" is the missing value printed: missing.
        let expected = expected.filter(|v| !v.is_empty());
        assert_eq!(got.as_deref(), expected, "{cell}");
    }
    assert_eq!(table.len(), 25);

    // One statement per walk, whatever the depth.
    let count = count_statements();
    resolve_first_live(&mut c, &walk, &key("root"))
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1, "a 4-level walk");
    // The control: the counter counts (two reads, two statements).
    resolve(&mut c, &walk[0], &key("own"), Want::Latest)
        .await
        .unwrap();
    resolve(&mut c, &walk[0], &key("own"), Want::Latest)
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn list_inherited_is_deepest_first_and_the_first_occurrence_wins() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, _) = mem_fs();
    seed_inheritance(&db, &fs, ws).await;
    let f = fixture();
    let cells = f["egret_expected"]["inheritance"]["api_inheritance"]
        .as_object()
        .unwrap();
    let mut c = conn(&db).await;
    let mut examined = 0;
    for (ns, cell) in cells {
        if ns.starts_with('_') {
            continue;
        }
        examined += 1;
        let grants: Vec<Grant> = ancestors(ns)[1..].iter().map(|a| grant(ws, a)).collect();
        let got = list_inherited(&mut c, ns, &grants, 100).await.unwrap();
        // Grouped by namespace in the order met: deepest first.
        let mut order: Vec<String> = vec![];
        let mut by_ns: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in &got {
            if order.last() != Some(&e.namespace) {
                assert!(!order.contains(&e.namespace), "{ns}: {:?}", keys(&got));
                order.push(e.namespace.clone());
            }
            by_ns
                .entry(e.namespace.clone())
                .or_default()
                .push(e.key.to_string());
        }
        let want_order: Vec<String> = cell["namespaces_in_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(order, want_order, "{ns}");
        for (level, want) in cell["entries"].as_object().unwrap() {
            let mut want: Vec<String> = want
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let mut have = by_ns.remove(level).unwrap_or_default();
            want.sort();
            have.sort();
            assert_eq!(have, want, "{ns} / {level}: a set");
        }
        assert!(by_ns.is_empty(), "{ns}: {by_ns:?}");
    }
    assert_eq!(examined, 5);
    // The cap counts the union.
    let grants: Vec<Grant> = ancestors("w9c.inh.a.b.c")[1..]
        .iter()
        .map(|a| grant(ws, a))
        .collect();
    let err = list_inherited(&mut c, "w9c.inh.a.b.c", &grants, 3)
        .await
        .unwrap_err();
    // Review M5: the refusal names the namespace asked about, not its
    // nearest parent.
    assert!(
        matches!(&err, NsError::TooLarge(m) if m.contains("max_namespace_files")
            && m.contains("3")
            && m.contains("the parents of namespace 'w9c.inh.a.b.c'")),
        "{err:?}"
    );
    assert!(list_inherited(&mut c, "w9c", &[], 3)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_same_key_in_two_workspaces_resolves_to_the_grants_row_only() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (other, _) = common::seed_workspace(&db).await;
    let (empty, _) = common::seed_workspace(&db).await;
    insert(&db, ws, r(NS, "k", 1).size(11)).await;
    for rev in 1..=3 {
        insert(&db, other, r(NS, "k", rev).size(99)).await;
    }
    insert(&db, other, r(NS, "theirs", 1)).await;
    insert(&db, other, r("w9c", "k", 1).size(98)).await;
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    let e = resolve(&mut c, &g, &key("k"), Want::Latest).await.unwrap();
    assert_eq!((e.workspace, e.revision, e.size), (ws, 1, 11));
    assert!(resolve(&mut c, &g, &key("k"), Want::Revision(3))
        .await
        .is_err());
    assert!(resolve(&mut c, &g, &key("theirs"), Want::Latest)
        .await
        .is_err());
    assert_eq!(revisions(&mut c, &g, &key("k")).await.unwrap().len(), 1);
    assert_eq!(keys(&list_live(&mut c, &g, None, 10).await.unwrap()), ["k"]);
    assert_eq!(rows_page(&mut c, &g, None, 10).await.unwrap().len(), 1);
    let walk = [g.clone(), grant(ws, "w9c")];
    let e = resolve_first_live(&mut c, &walk, &key("k"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((e.workspace, e.size), (ws, 11));
    assert!(list_inherited(&mut c, NS, &[grant(ws, "w9c")], 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(namespaces_with_kv(&mut c, ws, NS).await.unwrap(), [NS]);
    // A namespace of another workspace answers as an empty one.
    let ge = grant(empty, NS);
    let err = resolve(&mut c, &ge, &key("k"), Want::Latest)
        .await
        .unwrap_err();
    assert_eq!(
        not_found_message(&err),
        format!("key 'k' does not exist in namespace '{NS}'")
    );
    assert!(matches!(
        revisions(&mut c, &ge, &key("k")).await,
        Err(NsError::NotFound(_))
    ));
    assert!(list_live(&mut c, &ge, None, 10).await.unwrap().is_empty());
    assert!(rows_page(&mut c, &ge, None, 10).await.unwrap().is_empty());
    assert!(
        resolve_first_live(&mut c, std::slice::from_ref(&ge), &key("k"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(namespaces_with_kv(&mut c, empty, NS)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_grant_of_the_other_resource_or_a_mixed_walk_is_refused() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (other, _) = common::seed_workspace(&db).await;
    insert(&db, ws, r(NS, "k", 1)).await;
    let mut c = conn(&db).await;
    let files = authorize(ws, Actor::Api, NS, Resource::NamespaceFiles, Access::Read).unwrap();
    let err = resolve(&mut c, &files, &key("k"), Want::Latest)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NsError::Author(m) if m.contains("egret bug")),
        "{err:?}"
    );
    assert!(list_live(&mut c, &files, None, 10).await.is_err());
    assert!(revisions(&mut c, &files, &key("k")).await.is_err());
    assert!(rows_page(&mut c, &files, None, 10).await.is_err());
    assert!(
        resolve_first_live(&mut c, std::slice::from_ref(&files), &key("k"))
            .await
            .is_err()
    );
    assert!(list_inherited(&mut c, NS, std::slice::from_ref(&files), 10)
        .await
        .is_err());
    // Review M4: a mixed walk is egret's bug, failed once (Author), never an
    // outage a handler re-claims for ever (Db).
    let mixed = [grant(ws, NS), grant(other, "w9c")];
    let bug = |err: NsError| {
        assert!(
            matches!(&err, NsError::Author(m) if m.contains("egret bug")
                && m.contains("two workspaces")),
            "{err:?}"
        );
    };
    bug(resolve_first_live(&mut c, &mixed, &key("k"))
        .await
        .unwrap_err());
    bug(list_inherited(&mut c, "w9c.t2.c", &mixed, 10)
        .await
        .unwrap_err());
    // ... and so is an entry read under another namespace's grant.
    let e = resolve(&mut c, &grant(ws, NS), &key("k"), Want::Latest)
        .await
        .unwrap();
    let (fs, _) = mem_fs();
    for wrong in [grant(other, NS), grant(ws, "w9c")] {
        let err = read_value(&db.pool, &fs, &wrong, &e).await.unwrap_err();
        assert!(
            matches!(&err, NsError::Author(m) if m.contains("egret bug")
                && m.contains("holding a grant")),
            "{err:?}"
        );
    }
    // The control.
    assert!(resolve(&mut c, &grant(ws, NS), &key("k"), Want::Latest)
        .await
        .is_ok());
}

#[tokio::test]
async fn list_live_refuses_one_past_the_cap_naming_it() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    // Dead keys first (lowest ids), then three live ones: the cap counts
    // live keys, so 3 fit a cap of 3.
    insert(&db, ws, r(NS, "d1", 1).deleted()).await;
    insert(&db, ws, r(NS, "x1", 1).expires_in(-1.0)).await;
    insert(&db, ws, r(NS, "d2", 1)).await;
    insert(&db, ws, r(NS, "d2", 2).deleted()).await;
    for k in ["a", "b", "c"] {
        insert(&db, ws, r(NS, k, 1)).await;
    }
    let got = list_live(&mut c, &g, None, 3).await.unwrap();
    assert_eq!(keys(&got), ["a", "b", "c"]);
    insert(&db, ws, r(NS, "e", 1)).await;
    let err = list_live(&mut c, &g, None, 3).await.unwrap_err();
    match &err {
        NsError::TooLarge(m) => assert_eq!(
            m,
            &format!(
                "namespace '{NS}' holds more than 3 keys, the most one answer may hold \
                 (workflow.storage.max_namespace_files); 4 were seen"
            )
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(list_live(&mut c, &g, None, 4).await.unwrap().len(), 4);
}

#[tokio::test]
async fn list_live_prefix_is_a_plain_starts_with() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    for k in ["a_b", "ab", "a.b", "aXb", "a_", "A_b", "b_a"] {
        insert(&db, ws, r(NS, k, 1)).await;
    }
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    for (prefix, want) in [
        ("a_", vec!["a_b", "a_"]),
        ("a.", vec!["a.b"]),
        ("A", vec!["A_b"]),
        ("a_b", vec!["a_b"]),
        ("%", vec![]),
        ("", vec!["a_b", "ab", "a.b", "aXb", "a_", "A_b", "b_a"]),
    ] {
        let got = list_live(&mut c, &g, Some(prefix), 100).await.unwrap();
        assert_eq!(keys(&got), want, "{prefix:?}");
    }
}

#[tokio::test]
async fn get_keys_order_matches_the_artifact() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    const GK: &str = "w9c.gk";
    for k in [
        "b", "a_b", "A", "a.1", "0", "a", "B", "aa", "a1", "a-b", "a.b", "Z9", "a0",
    ] {
        let row = if k == "B" {
            r(GK, k, 1).expires_in(-1.0)
        } else {
            r(GK, k, 1)
        };
        insert(&db, ws, row).await;
    }
    soft_delete_latest(&db, ws, GK, "a_b").await;
    let f = fixture();
    let cells = &f["egret_expected"]["get_keys"];
    let want = |cell: &Value| -> Vec<String> {
        cell["outputs"]["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let mut c = conn(&db).await;
    let mut examined = 0;
    for name in [
        "all",
        "prefix_a",
        "prefix_A",
        "prefix_a.",
        "prefix_none",
        "prefix_empty",
        "other_ns_default_empty",
        "prefix_B_expired",
    ] {
        let cell = &cells[name];
        let ns = cell["props"]["namespace"].as_str().unwrap_or(GK);
        let prefix = cell["props"]["prefix"].as_str();
        let got = list_live(&mut c, &grant(ws, ns), prefix, 100)
            .await
            .unwrap();
        assert_eq!(keys(&got), want(cell), "{name}");
        examined += 1;
    }
    // after_rewrites: a rewrite moves a key to the end.
    let steps = cells["after_rewrites"]["steps"].as_array().unwrap();
    insert(&db, ws, r(GK, "b", 2)).await;
    insert(&db, ws, r(GK, "0", 2)).await;
    let got = list_live(&mut c, &grant(ws, GK), None, 100).await.unwrap();
    assert_eq!(
        keys(&got),
        want(&serde_json::json!({"outputs": {"keys": steps[0]["keys"]}}))
    );
    insert(&db, ws, r(GK, "a_b", 2)).await;
    let got = list_live(&mut c, &grant(ws, GK), None, 100).await.unwrap();
    assert_eq!(
        keys(&got),
        want(&serde_json::json!({"outputs": {"keys": steps[1]["keys"]}}))
    );
    assert_eq!(examined + steps.len(), 10);
}

#[tokio::test]
async fn a_row_whose_object_is_missing_is_an_author_error_naming_key_and_revision() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, _) = mem_fs();
    insert_value(&db, &fs, ws, r(NS, "k", 1), "one").await;
    insert(&db, ws, r(NS, "k", 2)).await; // no object
    let mut c = conn(&db).await;
    let g = grant(ws, NS);
    let e = resolve(&mut c, &g, &key("k"), Want::Latest).await.unwrap();
    drop(c);
    let err = read_value(&db.pool, &fs, &g, &e).await.unwrap_err();
    match &err {
        NsError::Author(m) => {
            assert_eq!(
                m,
                &format!(
                    "revision 2 of key 'k' in namespace '{NS}' is recorded but its value is \
                     gone from internal storage"
                )
            );
            assert!(!m.contains("s3://") && !m.contains(&ws.to_string()), "{m}");
        }
        other => panic!("{other:?}"),
    }
    // No fallback to revision 1, which is there.
    let mut c = conn(&db).await;
    let e1 = resolve(&mut c, &g, &key("k"), Want::Revision(1))
        .await
        .unwrap();
    assert_eq!(
        read_value(&db.pool, &fs, &g, &e1).await.unwrap().to_json(),
        "one"
    );
}

/// A MemStore that counts streamed reads, and can fail them.
struct Counting {
    mem: MemStore,
    streams: AtomicUsize,
    fail: bool,
}

#[async_trait::async_trait]
impl ObjectStore for Counting {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.mem.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.mem.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.mem.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.mem.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.mem.list(prefix, delimiter).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        self.streams.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(StorageError::S3(format!(
                "connection reset reading {uri} (secret-detail)"
            )));
        }
        self.mem.get_stream(uri).await
    }
}

fn counting_fs(cap: u64, fail: bool) -> (storage::FileStore, Arc<Counting>) {
    let store = Arc::new(Counting {
        mem: MemStore::new(),
        streams: AtomicUsize::new(0),
        fail,
    });
    let fs = storage::FileStore {
        store: store.clone(),
        bucket: "b".into(),
        prefix: "workflow/".into(),
        caps: storage::Caps {
            max_read_bytes: cap,
            ..storage::Caps::default()
        },
        retention_days: 30,
    };
    (fs, store)
}

#[tokio::test]
async fn a_stored_value_over_max_read_bytes_is_too_large_naming_the_cap_without_a_download() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, store) = counting_fs(4096, false);
    let g = grant(ws, NS);
    let big = "x".repeat(5000);
    insert_value(&db, &fs, ws, r(NS, "big", 1), &big).await;
    let mut c = conn(&db).await;
    let e = resolve(&mut c, &g, &key("big"), Want::Latest)
        .await
        .unwrap();
    drop(c);
    assert_eq!(e.size, 5002);
    let err = read_value(&db.pool, &fs, &g, &e).await.unwrap_err();
    match &err {
        NsError::TooLarge(m) => assert_eq!(
            m,
            &format!(
                "revision 1 of key 'big' in namespace '{NS}' is 5002 bytes, more than \
                 workflow.storage.max_read_bytes (4096 bytes) lets egret read"
            )
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(store.streams.load(Ordering::SeqCst), 0, "no download");
    // A row whose size understates its object: the read stops at the cap.
    insert(&db, ws, r(NS, "liar", 1).size(10)).await;
    let object = fs.namespace_kv_uri(&g, &key("liar"), 1);
    fs.store
        .put(&object, format!("\"{big}\"").into_bytes())
        .await
        .unwrap();
    let mut c = conn(&db).await;
    let e = resolve(&mut c, &g, &key("liar"), Want::Latest)
        .await
        .unwrap();
    drop(c);
    let err = read_value(&db.pool, &fs, &g, &e).await.unwrap_err();
    assert!(
        matches!(&err, NsError::TooLarge(m) if m.contains("4097 bytes") && m.contains("max_read_bytes")),
        "{err:?}"
    );
    assert_eq!(store.streams.load(Ordering::SeqCst), 1);
    // The control: a value under the cap reads.
    insert_value(&db, &fs, ws, r(NS, "small", 1), "s").await;
    let mut c = conn(&db).await;
    let e = resolve(&mut c, &g, &key("small"), Want::Latest)
        .await
        .unwrap();
    assert_eq!(
        read_value(&db.pool, &fs, &g, &e).await.unwrap().to_json(),
        "s"
    );
}

#[tokio::test]
async fn a_store_outage_on_read_is_a_store_error_that_names_no_storage_key() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, _) = counting_fs(4096, true);
    let g = grant(ws, NS);
    insert_value(&db, &fs, ws, r(NS, "k", 1), "v").await;
    let mut c = conn(&db).await;
    let e = resolve(&mut c, &g, &key("k"), Want::Latest).await.unwrap();
    drop(c);
    match read_value(&db.pool, &fs, &g, &e).await.unwrap_err() {
        NsError::Store(m) => {
            assert!(m.contains("'k'") && m.contains(NS), "{m}");
            for leak in [
                "s3://",
                "secret-detail",
                "connection reset",
                &ws.to_string(),
            ] {
                assert!(!m.contains(leak), "{m:?} leaks {leak:?}");
            }
        }
        other => panic!("{other:?}"),
    }
}

/// The SQLSTATE and constraint of a refused INSERT.
async fn refused(db: &Db, sql: &str, ws: Uuid, k: &str) -> (String, Option<String>) {
    let err = sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
        .bind(ws)
        .bind(k)
        .execute(&db.pool)
        .await
        .expect_err(sql);
    let db_err = err.as_database_error().expect("a database error");
    (
        db_err.code().unwrap_or_default().to_string(),
        db_err.constraint().map(str::to_string),
    )
}

const PLAIN: &str = "INSERT INTO workflow.kv_entries \
     (workspace_id, namespace, key, revision, value_type, size) \
     VALUES ($1, 'w9c.t2', $2, 1, 'STRING', 1)";

#[tokio::test]
async fn the_key_check_refuses_what_check_key_refuses() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let k351 = "k".repeat(351);
    for bad in [k351.as_str(), "é", "_a", "", ".a", "-a", "a/b", "a b", "aé"] {
        assert!(check_key(bad).is_err(), "{bad:?}");
        let (code, _) = refused(&db, PLAIN, ws, bad).await;
        assert_eq!(code, "23514", "{bad:?}: a CHECK violation");
    }
    let k350 = "k".repeat(350);
    for ok in [k350.as_str(), "A-1._x", "0", "Z9", "a_"] {
        assert!(check_key(ok).is_ok(), "{ok}");
        sqlx::query(PLAIN)
            .bind(ws)
            .bind(ok)
            .execute(&db.pool)
            .await
            .unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
}

#[tokio::test]
async fn the_one_writer_and_deleted_shape_checks_hold() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let insert_sql = |cols: &str, vals: &str| {
        format!(
            "INSERT INTO workflow.kv_entries \
               (workspace_id, namespace, key, revision, value_type, size{cols}) \
             VALUES ($1, 'w9c.t2', $2, 1, 'STRING', 1{vals})"
        )
    };
    let u = "gen_random_uuid()";
    let cases: Vec<(String, &str)> = vec![
        (
            insert_sql(
                ", created_by_user, created_by_execution",
                &format!(", {u}, {u}"),
            ),
            "workflow_kv_entries_one_writer",
        ),
        (
            insert_sql(", created_by_task_run", &format!(", {u}")),
            "workflow_kv_entries_task_run_of_an_execution",
        ),
        (
            insert_sql(
                ", created_by_user, created_by_task_run",
                &format!(", {u}, {u}"),
            ),
            "workflow_kv_entries_task_run_of_an_execution",
        ),
        (
            insert_sql(", deleted", ", true"),
            "workflow_kv_entries_deleted_shape",
        ),
        (
            insert_sql(", deleted_at", ", now()"),
            "workflow_kv_entries_deleted_shape",
        ),
        (
            insert_sql(", deleted_by_user", &format!(", {u}")),
            "workflow_kv_entries_deleted_shape",
        ),
        (
            insert_sql(", deleted_by_execution", &format!(", {u}")),
            "workflow_kv_entries_deleted_shape",
        ),
        (
            insert_sql(
                ", deleted, deleted_at, deleted_by_user, deleted_by_execution",
                &format!(", true, now(), {u}, {u}"),
            ),
            "workflow_kv_entries_one_deleter",
        ),
        (
            insert_sql(
                ", deleted, deleted_at, deleted_by_task_run",
                &format!(", true, now(), {u}"),
            ),
            "workflow_kv_entries_one_deleter",
        ),
    ];
    let mut n = 0;
    for (sql, constraint) in &cases {
        n += 1;
        let (code, got) = refused(&db, sql, ws, &format!("k{n}")).await;
        assert_eq!(
            (code.as_str(), got.as_deref()),
            ("23514", Some(*constraint)),
            "{sql}"
        );
    }
    // The column checks.
    for (sql, what) in [
        (
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
             size) VALUES ($1, 'w9c.t2', $2, 0, 'STRING', 1)",
            "revision 0",
        ),
        (
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
             size) VALUES ($1, 'w9c.t2', $2, 1, 'INTEGER', 1)",
            "value_type INTEGER",
        ),
        (
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
             size) VALUES ($1, 'w9c.t2', $2, 1, 'string', 1)",
            "value_type string",
        ),
        (
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
             size) VALUES ($1, 'w9c.t2', $2, 1, 'STRING', -1)",
            "size -1",
        ),
        (
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
             size) VALUES ($1, '', $2, 1, 'STRING', 1)",
            "namespace ''",
        ),
    ] {
        let (code, _) = refused(&db, sql, ws, "c").await;
        assert_eq!(code, "23514", "{what}");
    }
    // The controls: each legal shape inserts.
    let ok = [
        insert_sql(", created_by_user", &format!(", {u}")),
        insert_sql(
            ", created_by_execution, created_by_task_run",
            &format!(", {u}, {u}"),
        ),
        insert_sql(", deleted, deleted_at", ", true, now()"),
        insert_sql(
            ", deleted, deleted_at, deleted_by_execution, deleted_by_task_run",
            &format!(", true, now(), {u}, {u}"),
        ),
        insert_sql(
            ", deleted, deleted_at, deleted_by_user",
            &format!(", true, now(), {u}"),
        ),
    ];
    for (i, sql) in ok.iter().enumerate() {
        sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
            .bind(ws)
            .bind(format!("ok{i}"))
            .execute(&db.pool)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    for t in KvType::ALL {
        sqlx::query(
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, \
             value_type, size) VALUES ($1, 'w9c.t2', $2, 1, $3, 1)",
        )
        .bind(ws)
        .bind(format!("t{}", t.as_str()))
        .bind(t.as_str())
        .execute(&db.pool)
        .await
        .unwrap_or_else(|e| panic!("{t}: {e}"));
    }
    // The unique revision.
    let (code, constraint) = refused(
        &db,
        "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
         size) VALUES ($1, 'w9c.t2', $2, 1, 'STRING', 1)",
        ws,
        "ok0",
    )
    .await;
    assert_eq!(
        (code.as_str(), constraint.as_deref()),
        ("23505", Some("workflow_kv_entries_revision_key"))
    );
}

#[tokio::test]
async fn deleting_a_workspace_cascades_its_kv_entries() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (other, _) = common::seed_workspace(&db).await;
    for k in ["a", "b"] {
        insert(&db, ws, r(NS, k, 1)).await;
        insert(&db, other, r(NS, k, 1)).await;
    }
    let count = |w: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1",
            )
            .bind(w)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!((count(ws).await, count(other).await), (2, 2));
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(ws)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!((count(ws).await, count(other).await), (0, 2));
}

#[tokio::test]
async fn the_latest_row_query_uses_the_unique_index() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    // 10 000 rows: 1 000 keys of 10 revisions.
    sqlx::query(
        "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
         size) \
         SELECT $1, 'w9c.big', 'k' || (i / 10), (i % 10) + 1, 'STRING', 1 \
         FROM generate_series(0, 9999) i",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE workflow.kv_entries")
        .execute(&db.pool)
        .await
        .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN SELECT id FROM workflow.kv_entries \
         WHERE workspace_id = '{ws}' AND namespace = 'w9c.big' AND key = 'k500' \
         ORDER BY revision DESC LIMIT 1"
    )))
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let plan = plan.join("\n");
    println!("{plan}");
    assert!(
        plan.contains("Index Scan Backward using workflow_kv_entries_revision_key"),
        "{plan}"
    );
    let mut c = conn(&db).await;
    let e = resolve(&mut c, &grant(ws, "w9c.big"), &key("k500"), Want::Latest)
        .await
        .unwrap();
    assert_eq!(e.revision, 10);
}

#[tokio::test]
async fn the_kv_object_key_is_kestras_layout() {
    let f = fixture();
    let layout = &f["egret_expected"]["layout"];
    let (ns, k) = (
        layout["namespace"].as_str().unwrap(),
        layout["key"].as_str().unwrap(),
    );
    let (fs, _) = mem_fs();
    let ws = Uuid::new_v4();
    let g = grant(ws, ns);
    let base = format!("s3://b/workflow/{ws}/w9c/lay/_kv/");
    let name = |rev: u32| {
        let uri = fs.namespace_kv_uri(&g, &key(k), rev);
        uri.strip_prefix(&base)
            .unwrap_or_else(|| panic!("{uri} is not under {base}"))
            .to_string()
    };
    // Every object any step lists is the name of some revision.
    let mut examined = 0;
    for step in layout["steps"].as_array().unwrap() {
        for obj in step["objects"].as_array().unwrap() {
            let obj = obj.as_str().unwrap();
            let rev: u32 = match obj.strip_prefix("k.ion") {
                Some("") => 1,
                Some(v) => v.strip_prefix(".v").unwrap().parse().unwrap(),
                None => panic!("{obj}"),
            };
            assert_eq!(name(rev), obj);
            examined += 1;
        }
    }
    assert!(examined > 10, "{examined}");
    assert_eq!(name(1), "k.ion");
    assert_eq!(name(2), "k.ion.v2");
    assert_eq!(name(12), "k.ion.v12");
    // The workspace and namespace are the grant's.
    let other = grant(Uuid::new_v4(), "a.b.c");
    assert!(fs
        .namespace_kv_uri(&other, &key("x"), 1)
        .ends_with(&format!("{}/a/b/c/_kv/x.ion", other.workspace())));
}

/// Review I1: `read_value` decodes on the blocking pool, not on the async
/// worker. On a CURRENT-THREAD runtime (one worker for every task), a ticker
/// spawned beside `read_value` keeps ticking while a large value decodes;
/// were the decode inline, the worker would sit in the Ion reader's join and
/// the ticker could not run at all (W9b's `matching_runs_off_the_async_
/// runtime_and_a_ticker_keeps_ticking`). No database: the entry is built.
#[tokio::test(flavor = "current_thread")]
async fn a_large_decode_runs_off_the_async_runtime_and_a_ticker_keeps_ticking() {
    let (fs, _) = mem_fs();
    let ws = Uuid::new_v4();
    let g = grant(ws, NS);
    // 150 000 strings of 60 bytes: about 9.5 MB, under the 10 MiB default
    // and under the reader's value budget.
    let items: Vec<Value> = (0..150_000)
        .map(|i| Value::String(format!("{i:0>60}")))
        .collect();
    let bytes = encode(&KvValue::from_json(KvType::Json, &Value::Array(items)).unwrap()).unwrap();
    assert!(bytes.len() > 9_000_000 && (bytes.len() as u64) < fs.caps.max_read_bytes);
    let k = key("big");
    fs.store
        .put(&fs.namespace_kv_uri(&g, &k, 1), bytes.clone())
        .await
        .unwrap();
    let e = KvEntry {
        id: 1,
        workspace: ws,
        namespace: NS.into(),
        key: k,
        revision: 1,
        value_type: KvType::Json,
        size: bytes.len() as u64,
        description: None,
        expires_at: None,
        deleted: false,
        created_at: chrono::Utc::now(),
        expired: false,
        created_by_task_run: None,
        deleted_by_task_run: None,
    };
    let ticks = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ticker = {
        let ticks = ticks.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    tokio::task::yield_now().await;
    let started = std::time::Instant::now();
    let before = ticks.load(Ordering::Relaxed);
    // No database: the object is its row's size, so `read_value` never
    // asks the pool (it would, only to judge a mismatch).
    let unused = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .unwrap();
    let read = read_value(&unused, &fs, &g, &e).await.unwrap();
    let took = started.elapsed();
    let ticked = ticks.load(Ordering::Relaxed) - before;
    ticker.abort();
    eprintln!(
        "read_value of {} bytes took {took:?}; the ticker ticked {ticked} times",
        bytes.len()
    );
    let KvData::List(items) = &read else {
        panic!("{read:?}")
    };
    assert_eq!(items.len(), 150_000);
    // The decode must dominate, or the test proves nothing.
    assert!(
        took >= std::time::Duration::from_millis(200),
        "the read took only {took:?}: raise the value's size"
    );
    // A 2 ms ticker on a free runtime ticks at least once per 10 ms.
    let floor = took.as_millis() as u64 / 10;
    assert!(
        ticked >= floor,
        "{ticked} ticks in {took:?} (at least {floor} expected): the decode ran on the async \
         worker"
    );
}

// ===========================================================================
// Task 3: the writes
// ===========================================================================

mod writes {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::Mutex;
    use std::time::Duration;

    use egret_workflow::storage::kv::store::{
        delete, delete_many, purge_key, put_merge, set, unstorable, Deleted, KvWritten, NewValue,
        PurgeRow, MAX_BULK_DELETE_KEYS,
    };
    use egret_workflow::storage::kv::value::{decode, parse_ttl, KvNumber, Ttl, Unstorable};
    use egret_workflow::storage::nsfiles::CreatedBy;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use tokio::sync::Barrier;

    const NS3: &str = "w9c.t3";
    const USER: CreatedBy = CreatedBy::User(Uuid::nil());

    fn wgrant(ws: Uuid, ns: &str) -> Grant {
        authorize(
            ws,
            Actor::Flow { namespace: ns },
            ns,
            Resource::Kv,
            Access::Write,
        )
        .unwrap()
    }

    fn api_wgrant(ws: Uuid, ns: &str, wait: Duration) -> Grant {
        authorize(ws, Actor::Api, ns, Resource::Kv, Access::Write)
            .unwrap()
            .with_lock_wait(Some(wait))
    }

    fn pgrant(ws: Uuid, ns: &str) -> Grant {
        authorize(
            ws,
            Actor::Flow { namespace: ns },
            ns,
            Resource::Kv,
            Access::Purge,
        )
        .unwrap()
    }

    fn typed(v: Value) -> KvValue {
        KvValue::from_json(KvType::of(&v), &v).unwrap()
    }

    fn new(v: Value) -> NewValue {
        NewValue {
            value: typed(v),
            description: None,
            ttl: None,
            overwrite: true,
        }
    }

    fn task_run(run: Uuid) -> CreatedBy {
        CreatedBy::TaskRun {
            execution: Uuid::from_u128(7),
            task_run: run,
        }
    }

    fn lock_text(ws: Uuid, ns: &str, k: &str) -> String {
        format!("w9c:{}:{ns}:{k}", ws.hyphenated())
    }

    /// A bigger pool than `common::db`'s 5, for the concurrency tests.
    async fn pool(db: &Db, n: u32, app: &str) -> PgPool {
        let opts = (*db.pool.connect_options()).clone().application_name(app);
        PgPoolOptions::new()
            .max_connections(n)
            .connect_with(opts)
            .await
            .unwrap()
    }

    /// Two barriers: the store call waits at `entered` (the test learns it is
    /// in there), then at `release`.
    #[derive(Clone)]
    struct Hold {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl Hold {
        fn new() -> Self {
            Self {
                entered: Arc::new(Barrier::new(2)),
                release: Arc::new(Barrier::new(2)),
            }
        }
        async fn pass(&self) {
            self.entered.wait().await;
            self.release.wait().await;
        }
    }

    /// A MemStore whose calls can be held, failed, slowed and counted.
    #[derive(Default)]
    struct Hooked {
        mem: MemStore,
        /// The next `copy` is held.
        copy_hold: Mutex<Option<Hold>>,
        /// The next PUT of a staging object is held.
        staging_hold: Mutex<Option<Hold>>,
        /// The next PUT of a `_kv` object is held.
        kv_put_hold: Mutex<Option<Hold>>,
        fail_copy: AtomicBool,
        /// The next `copy` panics (fix round 2, concern 4).
        panic_copy: AtomicBool,
        /// The next PUT of a `_kv` object panics.
        panic_kv_put: AtomicBool,
        /// The next `get_stream` is held before it reads.
        get_hold: Mutex<Option<Hold>>,
        /// Milliseconds every delete of a `_kv` object sleeps first.
        delete_delay_ms: AtomicU64,
        staging_puts: AtomicUsize,
    }

    fn is_staging(uri: &str) -> bool {
        uri.contains("/_nsfiles_staging/")
    }

    #[async_trait::async_trait]
    impl ObjectStore for Hooked {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
            let hold = if is_staging(uri) {
                self.staging_puts.fetch_add(1, Ordering::SeqCst);
                self.staging_hold.lock().unwrap().take()
            } else {
                if self.panic_kv_put.swap(false, Ordering::SeqCst) {
                    panic!("an injected panic in a KV object PUT");
                }
                self.kv_put_hold.lock().unwrap().take()
            };
            if let Some(h) = hold {
                h.pass().await;
            }
            self.mem.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
            self.mem.get(uri).await
        }
        async fn delete(&self, uri: &str) -> Result<(), StorageError> {
            let ms = self.delete_delay_ms.load(Ordering::SeqCst);
            if ms > 0 && uri.contains("/_kv/") {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            self.mem.delete(uri).await
        }
        async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
            if self.panic_copy.swap(false, Ordering::SeqCst) {
                panic!("an injected panic in a KV copy");
            }
            if self.fail_copy.load(Ordering::SeqCst) {
                return Err(StorageError::S3(format!(
                    "copy {src} -> {dst} failed (secret-detail)"
                )));
            }
            let hold = self.copy_hold.lock().unwrap().take();
            if let Some(h) = hold {
                h.pass().await;
            }
            self.mem.copy(src, dst).await
        }
        async fn list(
            &self,
            prefix: &str,
            delimiter: Option<&str>,
        ) -> Result<egret_storage::Listing, StorageError> {
            self.mem.list(prefix, delimiter).await
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
            let hold = self.get_hold.lock().unwrap().take();
            if let Some(h) = hold {
                h.pass().await;
            }
            self.mem.get_stream(uri).await
        }
        async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
            self.mem.head(uri).await
        }
    }

    fn hooked_fs(cap: u64) -> (storage::FileStore, Arc<Hooked>) {
        let store = Arc::new(Hooked::default());
        let fs = storage::FileStore {
            store: store.clone(),
            bucket: "b".into(),
            prefix: "workflow/".into(),
            caps: storage::Caps {
                max_read_bytes: cap,
                ..storage::Caps::default()
            },
            retention_days: 30,
        };
        (fs, store)
    }

    const DEFAULT_CAP: u64 = 10 * 1024 * 1024;

    /// Every row of the key, ascending (none: empty).
    async fn rows_of(db: &Db, ws: Uuid, ns: &str, k: &str) -> Vec<KvEntry> {
        let mut c = conn(db).await;
        match revisions(&mut c, &grant(ws, ns), &key(k)).await {
            Ok(rows) => rows,
            Err(NsError::NotFound(_)) => vec![],
            Err(e) => panic!("{e:?}"),
        }
    }

    fn revs(rows: &[KvEntry]) -> Vec<u32> {
        rows.iter().map(|e| e.revision).collect()
    }

    /// The latest live value, as a reader sees it.
    async fn latest_value(
        db: &Db,
        fs: &storage::FileStore,
        ws: Uuid,
        ns: &str,
        k: &str,
    ) -> Option<Value> {
        let g = grant(ws, ns);
        let mut c = conn(db).await;
        let e = match resolve(&mut c, &g, &key(k), Want::Latest).await {
            Ok(e) => e,
            Err(NsError::NotFound(_)) => return None,
            Err(e) => panic!("{e:?}"),
        };
        drop(c);
        Some(read_value(&db.pool, fs, &g, &e).await.unwrap().to_json())
    }

    async fn staging_rows(db: &Db, ws: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM workflow.namespace_file_staging WHERE workspace_id = $1",
        )
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    fn staging_objects(mem: &MemStore) -> usize {
        mem.keys().iter().filter(|k| is_staging(k)).count()
    }

    /// The object names under the namespace's `_kv` prefix.
    fn kv_objects(mem: &MemStore, ws: Uuid, ns: &str) -> Vec<String> {
        let prefix = format!("s3://b/workflow/{ws}/{}/_kv/", ns.replace('.', "/"));
        let mut names: Vec<String> = mem
            .keys()
            .into_iter()
            .filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
            .collect();
        names.sort();
        names
    }

    async fn expire_latest(db: &Db, ws: Uuid, ns: &str, k: &str) {
        sqlx::query(
            "UPDATE workflow.kv_entries SET expires_at = now() - interval '1 second' \
             WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND revision = \
               (SELECT max(revision) FROM workflow.kv_entries \
                 WHERE workspace_id = $1 AND namespace = $2 AND key = $3)",
        )
        .bind(ws)
        .bind(ns)
        .bind(k)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    fn written(r: Result<KvWritten, NsError>) -> KvEntry {
        match r {
            Ok(KvWritten::Written(e)) => e,
            other => panic!("not Written: {other:?}"),
        }
    }

    fn too_large(r: Result<KvWritten, NsError>) -> String {
        match r {
            Err(NsError::TooLarge(m)) => m,
            other => panic!("not TooLarge: {other:?}"),
        }
    }

    #[test]
    fn unstorable_maps_the_variant_never_the_message() {
        assert!(matches!(
            unstorable(Unstorable::TooLarge("Invalid".into())),
            NsError::TooLarge(m) if m == "Invalid"
        ));
        assert!(matches!(
            unstorable(Unstorable::Invalid("too large".into())),
            NsError::Author(m) if m == "too large"
        ));
    }

    /// Plan item 1, mutation (a): A holds the key's lock mid-copy while B
    /// starts; exactly one is written, the other is Kestra's overwrite
    /// conflict naming the key. 20 rounds.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_concurrent_sets_with_overwrite_false_write_exactly_once() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let pool = pool(&db, 10, "w9c-t3-race").await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        for round in 0..20 {
            let k = key(&format!("race{round}"));
            let hold = Hold::new();
            *store.copy_hold.lock().unwrap() = Some(hold.clone());
            let start = |v: &str| {
                let (pool, fs, g, k) = (pool.clone(), fs.clone(), g.clone(), k.clone());
                let mut nv = new(json!(v));
                nv.overwrite = false;
                tokio::spawn(async move { set(&pool, &fs, &g, &k, nv, USER).await })
            };
            let a = start("a");
            hold.entered.wait().await; // A holds the lock, mid-copy
            let b = start("b");
            tokio::time::sleep(Duration::from_millis(50)).await;
            hold.release.wait().await;
            let results = [a.await.unwrap(), b.await.unwrap()];
            let wrote = results
                .iter()
                .filter(|r| matches!(r, Ok(KvWritten::Written(_))))
                .count();
            let conflicts: Vec<&String> = results
                .iter()
                .filter_map(|r| match r {
                    Err(NsError::Conflict(m)) => Some(m),
                    _ => None,
                })
                .collect();
            assert_eq!(
                (wrote, conflicts.len()),
                (1, 1),
                "round {round}: {results:?}"
            );
            assert_eq!(
                conflicts[0],
                &format!(
                    "Cannot set value for key '{k}'. Key already exists and `overwrite` is set \
                     to `false`."
                )
            );
            assert_eq!(
                revs(&rows_of(&db, ws, NS3, k.as_str()).await),
                [1],
                "round {round}"
            );
        }
    }

    /// Mutation (b): A's Put holds the lock mid-write while B's Put starts;
    /// B merges into A's result, so both fields survive, in two revisions.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_concurrent_puts_merging_different_fields_both_survive_in_two_revisions() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let pool = pool(&db, 10, "w9c-t3-puts").await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let k = key("merged");
        written(set(&pool, &fs, &g, &k, new(json!({"base": 0})), USER).await);
        let hold = Hold::new();
        *store.kv_put_hold.lock().unwrap() = Some(hold.clone());
        let start = |patch: Value| {
            let (pool, fs, g, k) = (pool.clone(), fs.clone(), g.clone(), k.clone());
            tokio::spawn(async move {
                put_merge(
                    &pool,
                    &fs,
                    &g,
                    &k,
                    KvData::from_json(&patch).unwrap(),
                    false,
                    USER,
                )
                .await
            })
        };
        let a = start(json!({"a": 1}));
        hold.entered.wait().await;
        let b = start(json!({"b": 2}));
        tokio::time::sleep(Duration::from_millis(50)).await;
        hold.release.wait().await;
        written(a.await.unwrap());
        written(b.await.unwrap());
        assert_eq!(revs(&rows_of(&db, ws, NS3, "merged").await), [1, 2, 3]);
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "merged").await,
            Some(json!({"base": 0, "a": 1, "b": 2}))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_writers_get_distinct_revisions_and_keys() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let pool = pool(&db, 12, "w9c-t3-writers").await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let k = key("many");
        let mut handles = Vec::new();
        for w in 0..8 {
            let (pool, fs, g, k) = (pool.clone(), fs.clone(), g.clone(), k.clone());
            handles.push(tokio::spawn(async move {
                let mut revisions = Vec::new();
                for round in 0..5 {
                    let e = written(
                        set(&pool, &fs, &g, &k, new(json!(format!("{w}-{round}"))), USER).await,
                    );
                    revisions.push(e.revision);
                }
                revisions
            }));
        }
        let mut all: Vec<u32> = Vec::new();
        for h in handles {
            all.extend(h.await.unwrap());
        }
        all.sort_unstable();
        assert_eq!(all, (1..=40).collect::<Vec<u32>>());
        assert_eq!(
            revs(&rows_of(&db, ws, NS3, "many").await),
            (1..=40).collect::<Vec<u32>>()
        );
        let objects = kv_objects(&store.mem, ws, NS3);
        assert_eq!(objects.len(), 40, "{objects:?}");
        assert_eq!(staging_objects(&store.mem), 0);
        assert_eq!(staging_rows(&db, ws).await, 0);
    }

    /// w9c `layout`: every step, driven through the writers and
    /// `purge_key`: objects and rows exact.
    #[tokio::test]
    async fn revisions_are_laid_out_as_the_artifact_lays_them_out() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let f = fixture();
        let layout = &f["egret_expected"]["layout"];
        let (ns, k) = (
            layout["namespace"].as_str().unwrap(),
            layout["key"].as_str().unwrap(),
        );
        let g = wgrant(ws, ns);
        let mut examined = 0;
        for step in layout["steps"].as_array().unwrap() {
            let name = step["step"].as_str().unwrap();
            match name {
                "before_any_write" => {}
                n if n.starts_with("write") || n.starts_with("rewrite") => {
                    written(set(&db.pool, &fs, &g, &key(k), new(json!(n)), USER).await);
                }
                "delete" => {
                    assert_eq!(
                        delete(&db.pool, &g, &key(k), USER).await.unwrap(),
                        Deleted::Deleted
                    );
                }
                "purge version keepAmount 1" => {
                    let n = purge_key(&db.pool, &fs, &pgrant(ws, ns), &key(k), 100, |rows| {
                        let keep = rows.len().saturating_sub(1);
                        rows[..keep].iter().map(|r| r.entry.id).collect()
                    })
                    .await
                    .unwrap();
                    assert_eq!(n, 3);
                }
                other => panic!("step {other}"),
            }
            let objects: Vec<String> = step["objects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o.as_str().unwrap().to_string())
                .collect();
            assert_eq!(kv_objects(&store.mem, ws, ns), objects, "{name}");
            let rows = rows_of(&db, ws, ns, k).await;
            let max = rows.iter().map(|e| e.revision).max();
            let got: Vec<Value> = rows
                .iter()
                .map(|e| {
                    json!({"key": k, "revision": e.revision, "latest": Some(e.revision) == max,
                           "deleted": e.deleted, "expires": e.expires_at.map(|_| "<ts>")})
                })
                .collect();
            assert_eq!(Value::Array(got), step["rows"], "{name}");
            let status = if latest_value(&db, &fs, ws, ns, k).await.is_some() {
                200
            } else {
                404
            };
            assert_eq!(json!(status), step["latest_status"], "{name}");
            examined += 1;
        }
        assert_eq!(examined, 8);
    }

    /// w9c `overwrite`; mutation (c).
    #[tokio::test]
    async fn overwrite_false_over_a_deleted_or_expired_key_writes() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let f = fixture();
        let cells = &f["egret_expected"]["overwrite"];
        let g = wgrant(ws, "w9c.ow");
        let no_overwrite = |v: &str| {
            let mut nv = new(json!(v));
            nv.overwrite = false;
            nv
        };
        for k in ["live", "del", "exp"] {
            let mut nv = new(json!("v1"));
            if k == "exp" {
                nv.ttl = Some(chrono::TimeDelta::seconds(60));
            }
            written(set(&db.pool, &fs, &g, &key(k), nv, USER).await);
        }
        assert_eq!(
            delete(&db.pool, &g, &key("del"), USER).await.unwrap(),
            Deleted::Deleted
        );
        expire_latest(&db, ws, "w9c.ow", "exp").await;
        // over_live
        match set(&db.pool, &fs, &g, &key("live"), no_overwrite("v2"), USER).await {
            Err(NsError::Conflict(m)) => assert_eq!(json!(m), cells["over_live"]["message"]),
            other => panic!("{other:?}"),
        }
        // over_deleted, over_expired: written, rows as the cells.
        for (k, cell) in [("del", "over_deleted"), ("exp", "over_expired")] {
            let e = written(set(&db.pool, &fs, &g, &key(k), no_overwrite("v2"), USER).await);
            assert_eq!(e.revision, 2, "{k}");
            let rows = rows_of(&db, ws, "w9c.ow", k).await;
            let got: Vec<Value> = rows
                .iter()
                .map(|e| {
                    json!({"key": k, "revision": e.revision, "latest": e.revision == 2,
                           "deleted": e.deleted, "expires": e.expires_at.map(|_| "<ts>")})
                })
                .collect();
            assert_eq!(Value::Array(got), cells[cell]["rows_after"], "{k}");
        }
        // over_missing
        written(set(&db.pool, &fs, &g, &key("never"), no_overwrite("v2"), USER).await);
        // true_over_live
        let e = written(set(&db.pool, &fs, &g, &key("live"), new(json!("v3")), USER).await);
        assert_eq!(json!(e.revision), cells["true_over_live"]["revision"]);
        for k in ["live", "del", "exp", "never"] {
            let cell = &cells["api_after"][k];
            assert_eq!(
                latest_value(&db, &fs, ws, "w9c.ow", k).await,
                Some(cell["value"].clone()),
                "{k}"
            );
            let rows = rows_of(&db, ws, "w9c.ow", k).await;
            assert_eq!(
                json!(rows.last().unwrap().revision),
                cell["revision"],
                "{k}"
            );
        }
    }

    /// Mutation (d): the cap is checked before anything is staged.
    #[tokio::test]
    async fn a_value_over_max_read_bytes_is_refused_naming_the_cap_and_nothing_is_staged() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(64);
        let g = wgrant(ws, NS3);
        // `"` + 63 x + `"`: 65 bytes as stored.
        let m = too_large(
            set(
                &db.pool,
                &fs,
                &g,
                &key("big"),
                new(json!("x".repeat(63))),
                USER,
            )
            .await,
        );
        assert!(
            m.contains("workflow.storage.max_read_bytes") && m.contains("64 bytes"),
            "{m}"
        );
        assert!(m.contains("'big'") && m.contains(NS3), "{m}");
        assert_eq!(
            store.staging_puts.load(Ordering::SeqCst),
            0,
            "a staging upload"
        );
        assert_eq!(
            (staging_rows(&db, ws).await, staging_objects(&store.mem)),
            (0, 0)
        );
        assert!(rows_of(&db, ws, NS3, "big").await.is_empty());
        // At exactly 64 bytes: written.
        let e = written(
            set(
                &db.pool,
                &fs,
                &g,
                &key("big"),
                new(json!("x".repeat(62))),
                USER,
            )
            .await,
        );
        assert_eq!(e.size, 64);
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "big").await,
            Some(json!("x".repeat(62)))
        );
    }

    #[tokio::test]
    async fn a_merged_result_over_max_read_bytes_is_refused_and_nothing_is_written() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(4096);
        let g = wgrant(ws, NS3);
        written(
            set(
                &db.pool,
                &fs,
                &g,
                &key("m"),
                new(json!({"a": "x".repeat(3000)})),
                USER,
            )
            .await,
        );
        let patch = KvData::from_json(&json!({"b": "y".repeat(2000)})).unwrap();
        let m = too_large(put_merge(&db.pool, &fs, &g, &key("m"), patch, false, USER).await);
        assert!(
            m.contains("workflow.storage.max_read_bytes") && m.contains("4096 bytes"),
            "{m}"
        );
        assert_eq!(revs(&rows_of(&db, ws, NS3, "m").await), [1]);
        assert_eq!(kv_objects(&store.mem, ws, NS3), ["m.ion"]);
        // The control: a patch that fits is written.
        let patch = KvData::from_json(&json!({"b": "y"})).unwrap();
        written(put_merge(&db.pool, &fs, &g, &key("m"), patch, false, USER).await);
    }

    /// The user's ruling (2026-10-07), mutation (l): 200 000 zeros (400 KB)
    /// under the 10 MiB default pass a byte check but not the reader's value
    /// budget (163 840 values).
    #[tokio::test]
    async fn a_dense_value_over_the_value_limit_is_refused_naming_it_and_nothing_is_staged() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let zeros = json!(vec![0; 200_000]);
        let m = too_large(set(&db.pool, &fs, &g, &key("dense"), new(zeros.clone()), USER).await);
        assert!(m.contains("more than 163840 values"), "{m}");
        let patch = KvData::from_json(&zeros).unwrap();
        let m = too_large(put_merge(&db.pool, &fs, &g, &key("dense"), patch, false, USER).await);
        assert!(m.contains("more than 163840 values"), "{m}");
        assert_eq!(store.staging_puts.load(Ordering::SeqCst), 0);
        assert_eq!(
            (staging_rows(&db, ws).await, staging_objects(&store.mem)),
            (0, 0)
        );
        assert!(rows_of(&db, ws, NS3, "dense").await.is_empty());
        assert!(kv_objects(&store.mem, ws, NS3).is_empty());
        // The control: 100 000 zeros fit.
        written(
            set(
                &db.pool,
                &fs,
                &g,
                &key("dense"),
                new(json!(vec![0; 100_000])),
                USER,
            )
            .await,
        );
    }

    /// Fix round 3 (the user's ruling, 2026-10-07), mutation (n).
    #[tokio::test]
    async fn a_number_over_1000_digits_is_413_class_and_nothing_is_staged() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let number = |digits: usize| NewValue {
            value: KvValue {
                kind: KvType::Number,
                data: KvData::Number(KvNumber::Int("7".repeat(digits))),
            },
            description: None,
            ttl: None,
            overwrite: true,
        };
        let m = too_large(set(&db.pool, &fs, &g, &key("n"), number(1001), USER).await);
        assert!(m.contains("1000 digits"), "{m}");
        assert_eq!(store.staging_puts.load(Ordering::SeqCst), 0);
        assert_eq!(
            (staging_rows(&db, ws).await, staging_objects(&store.mem)),
            (0, 0)
        );
        assert!(rows_of(&db, ws, NS3, "n").await.is_empty());
        written(set(&db.pool, &fs, &g, &key("n"), number(1000), USER).await);
        let mut c = conn(&db).await;
        let e = resolve(&mut c, &grant(ws, NS3), &key("n"), Want::Latest)
            .await
            .unwrap();
        drop(c);
        assert_eq!(
            read_value(&db.pool, &fs, &grant(ws, NS3), &e)
                .await
                .unwrap(),
            KvData::Number(KvNumber::Int("7".repeat(1000)))
        );
        // The Invalid arm: a DURATION holding a bool is the author's (400).
        let bad = NewValue {
            value: KvValue {
                kind: KvType::Duration,
                data: KvData::Bool(true),
            },
            description: None,
            ttl: None,
            overwrite: true,
        };
        match set(&db.pool, &fs, &g, &key("d"), bad, USER).await {
            Err(NsError::Author(m)) => assert!(m.contains("DURATION"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            store.staging_puts.load(Ordering::SeqCst),
            1,
            "only the 1000-digit write"
        );
    }

    /// Mutation (m): the merge never goes through JSON.
    #[tokio::test]
    async fn put_merge_into_a_stored_decimal_keeps_it_exact() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let stored = decode(KvType::Json, b"{a:123456789012345678901234567890.5}", 4096).unwrap();
        let nv = NewValue {
            value: KvValue {
                kind: KvType::Json,
                data: stored,
            },
            description: None,
            ttl: None,
            overwrite: true,
        };
        written(set(&db.pool, &fs, &g, &key("dec"), nv, USER).await);
        let patch = KvData::from_json(&json!({"b": 2})).unwrap();
        written(put_merge(&db.pool, &fs, &g, &key("dec"), patch, false, USER).await);
        let object = fs.namespace_kv_uri(&g, &key("dec"), 2);
        assert_eq!(
            String::from_utf8(store.mem.get(&object).await.unwrap()).unwrap(),
            "{a:123456789012345678901234567890.5,b:2}"
        );
    }

    #[tokio::test]
    async fn expires_at_is_now_plus_ttl_by_the_databases_clock() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let mut nv = new(json!("v"));
        nv.ttl = Some(parse_ttl("PT1M30.25S", Ttl::Task).unwrap());
        let e = written(set(&db.pool, &fs, &g, &key("ttl"), nv, USER).await);
        let expected = e.created_at + chrono::TimeDelta::milliseconds(90_250);
        let drift = (e.expires_at.unwrap() - expected)
            .num_microseconds()
            .unwrap()
            .abs();
        assert!(drift < 1000, "{drift} µs");
        // Task 3 review I1: zero is the API header's "expired at once" (w9c
        // `api.headers.h_ttl_zero`: 200, one row with an expiry); its sign
        // and zero rules are `parse_ttl`'s modes, not `set`'s.
        let f = fixture();
        let cell = &f["egret_expected"]["api"]["headers"]["h_ttl_zero"];
        let mut nv = new(json!("v"));
        nv.ttl = Some(parse_ttl(cell["ttl"].as_str().unwrap(), Ttl::Header).unwrap());
        let e = written(set(&db.pool, &fs, &g, &key("h_ttl_zero"), nv, USER).await);
        assert_eq!(e.expires_at, Some(e.created_at), "expired at once");
        let rows: Vec<Value> = rows_of(&db, ws, NS3, "h_ttl_zero")
            .await
            .iter()
            .map(|e| {
                json!({"key": e.key.as_str(), "revision": e.revision, "latest": true,
                       "deleted": e.deleted, "expires": e.expires_at.map(|_| "<ts>"),
                       "description": e.description})
            })
            .collect();
        assert_eq!(Value::Array(rows), cell["rows"]);
        assert_eq!(latest_value(&db, &fs, ws, NS3, "h_ttl_zero").await, None);
        // Task 3 review I3: past 1,000 years (a caller that skipped
        // `parse_ttl`) is the author's, never a database error.
        for bad in [
            chrono::TimeDelta::days(200_000_000),
            chrono::TimeDelta::seconds(-1),
        ] {
            let mut nv = new(json!("v"));
            nv.ttl = Some(bad);
            match set(&db.pool, &fs, &g, &key("ttl_big"), nv, USER).await {
                Err(NsError::Author(m)) => {
                    assert!(m.contains("ttl") && m.contains("365250 days"), "{m}")
                }
                other => panic!("{other:?}"),
            }
        }
        for raw in ["P200000000D"] {
            for mode in [Ttl::Task, Ttl::Header] {
                assert!(
                    parse_ttl(raw, mode).unwrap_err().contains("ttl"),
                    "{mode:?}"
                );
            }
        }
        // At the bound: written.
        let mut nv = new(json!("v"));
        nv.ttl = Some(parse_ttl("P365250D", Ttl::Task).unwrap());
        written(set(&db.pool, &fs, &g, &key("ttl_max"), nv, USER).await);
    }

    /// w9c `put_merge.keep_meta`.
    #[tokio::test]
    async fn put_keeps_the_latest_description_and_expiry() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let mut nv = new(json!({"a": 1}));
        nv.description = Some("my description".into());
        nv.ttl = Some(parse_ttl("P1D", Ttl::Task).unwrap());
        let r1 = written(set(&db.pool, &fs, &g, &key("keep_meta"), nv, USER).await);
        let patch = KvData::from_json(&json!({"b": 2})).unwrap();
        let r2 = written(put_merge(&db.pool, &fs, &g, &key("keep_meta"), patch, false, USER).await);
        assert_eq!(r2.description.as_deref(), Some("my description"));
        assert_eq!(r2.expires_at, r1.expires_at);
        assert_eq!(r2.value_type, KvType::Json);
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "keep_meta").await,
            Some(json!({"a": 1, "b": 2}))
        );
    }

    /// Item 8, w9c `put_merge.over_expired`, `over_expired_eom_true`,
    /// `over_deleted_with_description`.
    #[tokio::test]
    async fn put_over_an_expired_key_is_a_missing_key() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let f = fixture();
        let cells = &f["egret_expected"]["put_merge"];
        let ns = "w9c.put";
        let g = wgrant(ws, ns);
        for k in ["expired_meta", "expired_eom", "deleted_meta"] {
            let mut nv = new(json!({"a": 1}));
            nv.description = Some("old desc".into());
            nv.ttl = Some(chrono::TimeDelta::seconds(60));
            written(set(&db.pool, &fs, &g, &key(k), nv, USER).await);
        }
        expire_latest(&db, ws, ns, "expired_meta").await;
        expire_latest(&db, ws, ns, "expired_eom").await;
        delete(&db.pool, &g, &key("deleted_meta"), USER)
            .await
            .unwrap();
        let patch = || KvData::from_json(&json!({"b": 2})).unwrap();
        for (k, cell) in [
            ("expired_meta", "over_expired"),
            ("deleted_meta", "over_deleted_with_description"),
        ] {
            let e = written(put_merge(&db.pool, &fs, &g, &key(k), patch(), false, USER).await);
            assert_eq!(
                (e.revision, e.description.clone(), e.expires_at),
                (2, None, None),
                "{k}"
            );
            assert_eq!(
                latest_value(&db, &fs, ws, ns, k).await.as_ref(),
                Some(&cells[cell]["result"]),
                "{k}"
            );
        }
        match put_merge(&db.pool, &fs, &g, &key("expired_eom"), patch(), true, USER).await {
            Err(NsError::NotFound(m)) => {
                assert_eq!(json!(m), cells["over_expired_eom_true"]["message"])
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(revs(&rows_of(&db, ws, ns, "expired_eom").await), [1]);
    }

    /// Every `put_merge` cell but the templates (Task 5's render), through
    /// the store: seeded with `set` (its description and ttl), put in its
    /// state, then `put_merge` with its `errorOnMissing`. Outcome, result,
    /// message, revision and kept metadata as the cell.
    #[tokio::test]
    async fn every_put_merge_cell_through_the_store() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let f = fixture();
        let ns = "w9c.put";
        let g = wgrant(ws, ns);
        let mut examined = 0;
        for (name, cell) in f["egret_expected"]["put_merge"].as_object().unwrap() {
            if name.starts_with('_') || cell["patch"].to_string().contains("{{") {
                continue;
            }
            // The key: the one the cell's message or rows name, else its name.
            let k = cell["rows_after"][0]["key"]
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    cell["message"].as_str().and_then(|m| {
                        m.strip_prefix("No value found for key '")
                            .and_then(|r| r.split_once('\''))
                            .map(|(k, _)| k.to_string())
                    })
                })
                .unwrap_or_else(|| name.clone());
            let k = key(&k);
            let state = cell["existing_state"].as_str().unwrap();
            if state != "missing" {
                let mut nv = new(cell["existing"].clone());
                nv.description = cell["existing_description"].as_str().map(str::to_string);
                nv.ttl = cell["existing_ttl"].as_str().map(|t| {
                    parse_ttl(t, Ttl::Task)
                        .unwrap()
                        .max(chrono::TimeDelta::seconds(60))
                });
                written(set(&db.pool, &fs, &g, &k, nv, USER).await);
                match state {
                    "live" => {}
                    "expired" => expire_latest(&db, ws, ns, k.as_str()).await,
                    "deleted" => assert_eq!(
                        delete(&db.pool, &g, &k, USER).await.unwrap(),
                        Deleted::Deleted
                    ),
                    other => panic!("{name}: state {other}"),
                }
            }
            let before = rows_of(&db, ws, ns, k.as_str()).await;
            let patch = KvData::from_json(&cell["patch"]).unwrap();
            let eom = cell["errorOnMissing"] == json!(true);
            let got = put_merge(&db.pool, &fs, &g, &k, patch, eom, USER).await;
            match cell["outcome"].as_str().unwrap() {
                "ok" => {
                    let e = written(got);
                    if let Some(r) = cell["revision_after"].as_u64() {
                        assert_eq!(u64::from(e.revision), r, "{name}");
                    }
                    let object = fs.namespace_kv_uri(&g, &k, e.revision);
                    let bytes = store.mem.get(&object).await.unwrap();
                    if let Some(ion) = cell["ion_bytes"].as_str() {
                        assert_eq!(String::from_utf8(bytes).unwrap(), ion, "{name}");
                    }
                    let read = latest_value(&db, &fs, ws, ns, k.as_str()).await.unwrap();
                    assert_eq!(
                        serde_json::to_string(&read).unwrap(),
                        serde_json::to_string(&cell["result"]).unwrap(),
                        "{name}"
                    );
                    if cell.get("description_after").is_some() {
                        assert_eq!(json!(e.description), cell["description_after"], "{name}");
                    }
                    if cell.get("expires_after").is_some() {
                        assert_eq!(
                            json!(e.expires_at.map(|_| "<ts>")),
                            cell["expires_after"],
                            "{name}"
                        );
                    }
                }
                "error" => {
                    let m = match got {
                        Err(NsError::Author(m) | NsError::NotFound(m)) => m,
                        other => panic!("{name}: {other:?}"),
                    };
                    // Message exact (review M1): the store adds no prefix.
                    assert_eq!(json!(m), cell["message"], "{name}");
                    assert_eq!(
                        rows_of(&db, ws, ns, k.as_str()).await,
                        before,
                        "{name}: nothing written"
                    );
                }
                other => panic!("{name}: {other}"),
            }
            examined += 1;
        }
        assert_eq!(examined, 45);
    }

    /// W9b's R1.
    #[tokio::test]
    async fn delete_flags_the_latest_and_a_rewrite_makes_the_next_revision_live() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let by = CreatedBy::User(Uuid::from_u128(42));
        for v in ["one", "two"] {
            written(set(&db.pool, &fs, &g, &key("d"), new(json!(v)), USER).await);
        }
        assert_eq!(
            delete(&db.pool, &g, &key("d"), by).await.unwrap(),
            Deleted::Deleted
        );
        let (deleted_at, deleted_by): (Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>) =
            sqlx::query_as(
                "SELECT deleted_at, deleted_by_user FROM workflow.kv_entries \
                  WHERE workspace_id = $1 AND key = 'd' AND revision = 2",
            )
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert!(deleted_at.is_some());
        assert_eq!(deleted_by, Some(Uuid::from_u128(42)));
        assert_eq!(latest_value(&db, &fs, ws, NS3, "d").await, None);
        assert_eq!(
            delete(&db.pool, &g, &key("d"), by).await.unwrap(),
            Deleted::Missing
        );
        assert_eq!(
            delete(&db.pool, &g, &key("never"), by).await.unwrap(),
            Deleted::Missing
        );
        let e = written(set(&db.pool, &fs, &g, &key("d"), new(json!("three")), USER).await);
        assert_eq!(e.revision, 3);
        let rows = rows_of(&db, ws, NS3, "d").await;
        assert_eq!(
            rows.iter().map(|e| e.deleted).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "d").await,
            Some(json!("three"))
        );
        // A delete removes no object.
        assert_eq!(
            kv_objects(&store.mem, ws, NS3),
            ["d.ion", "d.ion.v2", "d.ion.v3"]
        );
        // An expired key is Missing to a delete.
        let mut nv = new(json!("x"));
        nv.ttl = Some(chrono::TimeDelta::seconds(60));
        written(set(&db.pool, &fs, &g, &key("e"), nv, USER).await);
        expire_latest(&db, ws, NS3, "e").await;
        assert_eq!(
            delete(&db.pool, &g, &key("e"), by).await.unwrap(),
            Deleted::Missing
        );
    }

    /// §11, mutation (j).
    #[tokio::test]
    async fn a_reclaimed_set_put_or_delete_writes_nothing_more() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let run = task_run(Uuid::new_v4());
        // Set twice.
        written(set(&db.pool, &fs, &g, &key("s"), new(json!("v")), run).await);
        match set(&db.pool, &fs, &g, &key("s"), new(json!("v")), run).await {
            Ok(KvWritten::Already(e)) => assert_eq!(e.revision, 1),
            other => panic!("{other:?}"),
        }
        assert_eq!(revs(&rows_of(&db, ws, NS3, "s").await), [1]);
        // overwrite: false twice: never a conflict with its own write.
        let mut nv = new(json!("v"));
        nv.overwrite = false;
        written(set(&db.pool, &fs, &g, &key("o"), nv.clone(), run).await);
        assert!(matches!(
            set(&db.pool, &fs, &g, &key("o"), nv.clone(), run).await,
            Ok(KvWritten::Already(_))
        ));
        // Another task run: the control.
        assert!(matches!(
            set(&db.pool, &fs, &g, &key("o"), nv, task_run(Uuid::new_v4())).await,
            Err(NsError::Conflict(_))
        ));
        // Put twice: one merge.
        let patch = || KvData::from_json(&json!({"n": 1})).unwrap();
        written(put_merge(&db.pool, &fs, &g, &key("p"), patch(), false, run).await);
        assert!(matches!(
            put_merge(&db.pool, &fs, &g, &key("p"), patch(), false, run).await,
            Ok(KvWritten::Already(_))
        ));
        assert_eq!(revs(&rows_of(&db, ws, NS3, "p").await), [1]);
        // Delete twice.
        written(set(&db.pool, &fs, &g, &key("x"), new(json!("v")), USER).await);
        assert_eq!(
            delete(&db.pool, &g, &key("x"), run).await.unwrap(),
            Deleted::Deleted
        );
        assert_eq!(
            delete(&db.pool, &g, &key("x"), run).await.unwrap(),
            Deleted::Already
        );
        assert_eq!(
            delete(&db.pool, &g, &key("x"), task_run(Uuid::new_v4()))
                .await
                .unwrap(),
            Deleted::Missing
        );
    }

    /// Insert `n` live single-revision keys `bulk0000..` with plain SQL.
    async fn seed_keys(db: &Db, ws: Uuid, ns: &str, n: i32) -> Vec<KvKey> {
        sqlx::query(
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, size) \
             SELECT $1, $2, 'bulk' || lpad(i::text, 4, '0'), 1, 'STRING', 1 FROM generate_series(0, $3 - 1) i",
        )
        .bind(ws)
        .bind(ns)
        .bind(n)
        .execute(&db.pool)
        .await
        .unwrap();
        (0..n).map(|i| key(&format!("bulk{i:04}"))).collect()
    }

    async fn live_count(db: &Db, ws: Uuid, ns: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1 AND namespace = $2 AND NOT deleted",
        )
        .bind(ws)
        .bind(ns)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    /// `delete_many`'s permits are one per process: the tests that call it
    /// take turns, so none of them sees another's permits.
    static BULK_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn delete_many_takes_1000_keys_and_refuses_1001_naming_the_cap() {
        let _serial = BULK_SERIAL.lock().await;
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = api_wgrant(ws, NS3, Duration::from_secs(5));
        let mut keys = seed_keys(&db, ws, NS3, 1001).await;
        match delete_many(&db.pool, &fs, &g, &keys, USER).await {
            Err(NsError::TooLarge(m)) => {
                assert!(
                    m.contains(&MAX_BULK_DELETE_KEYS.to_string()) && m.contains("1001"),
                    "{m}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(live_count(&db, ws, NS3).await, 1001, "a row changed");
        keys.truncate(1000);
        let gone = delete_many(&db.pool, &fs, &g, &keys, USER).await.unwrap();
        assert_eq!(gone, keys);
        assert_eq!(live_count(&db, ws, NS3).await, 1);
    }

    /// Mutation (i).
    #[tokio::test]
    async fn delete_many_is_all_or_nothing_and_answers_the_live_keys() {
        let _serial = BULK_SERIAL.lock().await;
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let wg = wgrant(ws, NS3);
        for k in ["a", "b", "c", "z"] {
            written(set(&db.pool, &fs, &wg, &key(k), new(json!(k)), USER).await);
        }
        delete(&db.pool, &wg, &key("c"), USER).await.unwrap();
        // Another session holds z's lock past the wait.
        let mut holder = db.pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(lock_text(ws, NS3, "z"))
            .execute(&mut *holder)
            .await
            .unwrap();
        let g = api_wgrant(ws, NS3, Duration::from_millis(200));
        let ask: Vec<KvKey> = ["z", "a", "b", "c", "a", "never"]
            .into_iter()
            .map(key)
            .collect();
        match delete_many(&db.pool, &fs, &g, &ask, USER).await {
            Err(NsError::Conflict(m)) => assert!(m.contains("'z'") && m.contains("busy"), "{m}"),
            other => panic!("{other:?}"),
        }
        for k in ["a", "b", "z"] {
            assert!(
                latest_value(&db, &fs, ws, NS3, k).await.is_some(),
                "{k} was deleted"
            );
        }
        sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(lock_text(ws, NS3, "z"))
            .execute(&mut *holder)
            .await
            .unwrap();
        let gone = delete_many(&db.pool, &fs, &g, &ask, USER).await.unwrap();
        assert_eq!(gone, [key("a"), key("b"), key("z")]);
        assert_eq!(
            delete_many(&db.pool, &fs, &g, &ask, USER).await.unwrap(),
            Vec::<KvKey>::new()
        );
    }

    /// W9b final review I-1: an API writer waits at most its lock wait, then
    /// a 409 naming the key, its connection let go.
    #[tokio::test]
    async fn an_api_grant_waits_at_most_its_lock_wait() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let writer_pool = pool(&db, 3, "w9c-t3-waiter").await;
        written(
            set(
                &db.pool,
                &fs,
                &wgrant(ws, NS3),
                &key("w"),
                new(json!({"a": 1})),
                USER,
            )
            .await,
        );
        let mut holder = db.pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(lock_text(ws, NS3, "w"))
            .execute(&mut *holder)
            .await
            .unwrap();
        let g = api_wgrant(ws, NS3, Duration::from_millis(100));
        let busy = |r: Result<(), NsError>| {
            match r {
            Err(NsError::Conflict(m)) => assert_eq!(
                m,
                format!("key 'w' in namespace '{NS3}': the key is busy (another write holds it); try again")
            ),
            other => panic!("{other:?}"),
        }
        };
        let started = std::time::Instant::now();
        busy(
            set(&writer_pool, &fs, &g, &key("w"), new(json!("v")), USER)
                .await
                .map(drop),
        );
        let patch = KvData::from_json(&json!({"b": 2})).unwrap();
        busy(
            put_merge(&writer_pool, &fs, &g, &key("w"), patch, false, USER)
                .await
                .map(drop),
        );
        busy(delete(&writer_pool, &g, &key("w"), USER).await.map(drop));
        let api_purge = authorize(ws, Actor::Api, NS3, Resource::Kv, Access::Purge)
            .unwrap()
            .with_lock_wait(Some(Duration::from_millis(100)));
        busy(
            purge_key(&writer_pool, &fs, &api_purge, &key("w"), 10, |_| vec![])
                .await
                .map(drop),
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        // Every connection of the writer is back in its pool, and the staging
        // object and row are forgotten.
        assert_eq!(writer_pool.size() as usize - writer_pool.num_idle(), 0);
        assert_eq!(
            (staging_rows(&db, ws).await, staging_objects(&store.mem)),
            (0, 0)
        );
        assert_eq!(revs(&rows_of(&db, ws, NS3, "w").await), [1]);
        // Review M7: the holder's lock is let go before it returns to the pool.
        sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(lock_text(ws, NS3, "w"))
            .execute(&mut *holder)
            .await
            .unwrap();
    }

    /// The hold audit's "nothing held" claim for `set`: while its body is
    /// uploaded to the staging object, another session can take the key's
    /// lock, and the writer's pool has no connection out.
    #[tokio::test(flavor = "multi_thread")]
    async fn set_holds_no_connection_and_no_lock_while_it_stages() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let app = format!("w9c-t3-stage-{}", Uuid::new_v4().simple());
        let writer_pool = pool(&db, 3, &app).await;
        let hold = Hold::new();
        *store.staging_hold.lock().unwrap() = Some(hold.clone());
        let task = {
            let (p, fs, g) = (writer_pool.clone(), fs.clone(), wgrant(ws, NS3));
            tokio::spawn(async move { set(&p, &fs, &g, &key("s"), new(json!("v")), USER).await })
        };
        hold.entered.wait().await;
        let mut tx = db.pool.begin().await.unwrap();
        let free: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(lock_text(ws, NS3, "s"))
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.rollback().await.unwrap();
        assert!(free, "the key's lock is held while staging");
        assert_eq!(
            writer_pool.size() as usize - writer_pool.num_idle(),
            0,
            "a connection is out"
        );
        let busy: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1 AND state <> 'idle'",
        )
        .bind(&app)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(busy, 0);
        hold.release.wait().await;
        written(task.await.unwrap());
    }

    /// A failed copy rolls back and forgets the staging object; the error
    /// names no storage key.
    #[tokio::test]
    async fn a_failed_copy_rolls_back_and_forgets_the_staging_object() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        store.fail_copy.store(true, Ordering::SeqCst);
        match set(&db.pool, &fs, &g, &key("c"), new(json!("v")), USER).await {
            Err(NsError::Store(m)) => {
                assert!(m.contains("'c'") && m.contains(NS3), "{m}");
                for leak in ["s3://", "secret-detail", &ws.to_string()] {
                    assert!(!m.contains(leak), "{m} leaks {leak}");
                }
            }
            other => panic!("{other:?}"),
        }
        assert!(rows_of(&db, ws, NS3, "c").await.is_empty());
        assert_eq!(store.staging_puts.load(Ordering::SeqCst), 1);
        assert_eq!(
            (staging_rows(&db, ws).await, staging_objects(&store.mem)),
            (0, 0)
        );
        store.fail_copy.store(false, Ordering::SeqCst);
        assert_eq!(
            written(set(&db.pool, &fs, &g, &key("c"), new(json!("v")), USER).await).revision,
            1
        );
    }

    /// The user's ruling (2026-10-07), mutation (k).
    #[tokio::test]
    async fn purge_key_hands_the_judge_each_rows_successors_created_at() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let mut created = Vec::new();
        for v in ["1", "2", "3"] {
            created.push(
                written(set(&db.pool, &fs, &g, &key("s"), new(json!(v)), USER).await).created_at,
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let seen: Arc<Mutex<Vec<PurgeRow>>> = Arc::default();
        let judge = |seen: Arc<Mutex<Vec<PurgeRow>>>| {
            move |rows: &[PurgeRow]| {
                *seen.lock().unwrap() = rows.to_vec();
                vec![]
            }
        };
        let n = purge_key(
            &db.pool,
            &fs,
            &pgrant(ws, NS3),
            &key("s"),
            10,
            judge(seen.clone()),
        )
        .await
        .unwrap();
        assert_eq!(n, 0);
        let rows = seen.lock().unwrap().clone();
        assert_eq!(
            rows.iter().map(|r| r.entry.revision).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(
            rows.iter().map(|r| r.superseded_at).collect::<Vec<_>>(),
            [Some(created[1]), Some(created[2]), None]
        );
        // A soft-deleted latest has no successor either.
        delete(&db.pool, &g, &key("s"), USER).await.unwrap();
        purge_key(
            &db.pool,
            &fs,
            &pgrant(ws, NS3),
            &key("s"),
            10,
            judge(seen.clone()),
        )
        .await
        .unwrap();
        let rows = seen.lock().unwrap().clone();
        assert!(rows[2].entry.deleted);
        assert_eq!(rows[2].superseded_at, None);
        // More rows than the cap: TooLarge naming the key and the cap.
        match purge_key(&db.pool, &fs, &pgrant(ws, NS3), &key("s"), 2, |_| vec![]).await {
            Err(NsError::TooLarge(m)) => assert!(
                m.contains("'s'") && m.contains("more than 2 revisions"),
                "{m}"
            ),
            other => panic!("{other:?}"),
        }
    }

    /// Plan item 13, mutation (e): a writer holds the key's lock mid-copy
    /// (revision 3 coming); the purge waits, then judges the rows as they
    /// are AFTER the lock: it keeps only the latest it sees, revision 3.
    #[tokio::test(flavor = "multi_thread")]
    async fn purge_key_judges_under_the_lock() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let pool = pool(&db, 6, "w9c-t3-judge").await;
        let g = wgrant(ws, NS3);
        for v in ["1", "2"] {
            written(set(&pool, &fs, &g, &key("j"), new(json!(v)), USER).await);
        }
        let hold = Hold::new();
        *store.copy_hold.lock().unwrap() = Some(hold.clone());
        let writer = {
            let (p, fs, g) = (pool.clone(), fs.clone(), g.clone());
            tokio::spawn(async move { set(&p, &fs, &g, &key("j"), new(json!("3")), USER).await })
        };
        hold.entered.wait().await;
        let purge = {
            let (p, fs) = (pool.clone(), fs.clone());
            tokio::spawn(async move {
                purge_key(&p, &fs, &pgrant(ws, NS3), &key("j"), 10, |rows| {
                    rows[..rows.len() - 1].iter().map(|r| r.entry.id).collect()
                })
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        hold.release.wait().await;
        written(writer.await.unwrap());
        assert_eq!(purge.await.unwrap().unwrap(), 2);
        assert_eq!(revs(&rows_of(&db, ws, NS3, "j").await), [3]);
        assert_eq!(kv_objects(&store.mem, ws, NS3), ["j.ion.v3"]);
    }

    /// Mutation (f): with a SESSION lock held until the objects are deleted,
    /// no writer reuses a freed revision number's object key in between.
    /// Object deletes are slowed to widen the window.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_purge_and_a_writer_never_leave_a_row_without_an_object() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        store.delete_delay_ms.store(20, Ordering::SeqCst);
        let pool = pool(&db, 8, "w9c-t3-purge-writer").await;
        let g = wgrant(ws, NS3);
        for round in 0..10 {
            let k = key(&format!("pw{round}"));
            for v in ["1", "2", "3"] {
                written(set(&pool, &fs, &g, &k, new(json!(v)), USER).await);
            }
            let purge = {
                let (p, fs, k) = (pool.clone(), fs.clone(), k.clone());
                tokio::spawn(async move {
                    purge_key(&p, &fs, &pgrant(ws, NS3), &k, 10, |rows| {
                        rows.iter().map(|r| r.entry.id).collect()
                    })
                    .await
                })
            };
            tokio::time::sleep(Duration::from_millis(2)).await;
            let writer = {
                let (p, fs, g, k) = (pool.clone(), fs.clone(), g.clone(), k.clone());
                tokio::spawn(async move { set(&p, &fs, &g, &k, new(json!("w")), USER).await })
            };
            purge.await.unwrap().unwrap();
            written(writer.await.unwrap());
            for e in rows_of(&db, ws, NS3, k.as_str()).await {
                let object = fs.namespace_kv_uri(&g, &k, e.revision);
                assert!(
                    store.mem.head(&object).await.unwrap().is_some(),
                    "round {round}: revision {} has no object",
                    e.revision
                );
            }
        }
    }

    /// w9c `purge` behaviour cells through `purge_key`, with judges as Task
    /// 5 builds them (key/expiredOnly, version keepAmount, version before,
    /// keyPattern): `size` and the surviving revisions exact. The refusal
    /// cells (`expiredOnly: false`, neither property, both) are Task 5's
    /// property checks, before any purge.
    #[tokio::test]
    async fn every_purge_behaviour_cell_through_purge_key() {
        let Some(db) = common::db().await else { return };
        let f = fixture();
        let cells = &f["egret_expected"]["purge"];
        let mut examined = 0;
        for name in [
            "key_default",
            "key_expiredOnly_true",
            "version_keep1",
            "version_keep0",
            "version_keep2",
            "version_before",
            "key_default_keyPattern",
            "version_keep1_keyPattern",
        ] {
            let cell = &cells[name];
            let (ws, _) = common::seed_workspace(&db).await;
            let (fs, _) = hooked_fs(DEFAULT_CAP);
            let ns = "w9c.pg2";
            // `_setup`: r1 at t-60s, r2 at t-50s (ke's at t-30s, expired),
            // t_mid at t-40s, r3 at t-20s; kd soft-deleted.
            let seed = |k: &'static str, rev: i32, ago: f64, expired: bool, deleted: bool| {
                let pool = db.pool.clone();
                async move {
                    sqlx::query(
                        "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, \
                           value_type, size, created_at, expires_at, deleted, deleted_at) \
                         VALUES ($1, $2, $3, $4, 'STRING', 1, now() - make_interval(secs => $5), \
                           CASE WHEN $6 THEN now() - interval '1 second' END, $7, \
                           CASE WHEN $7 THEN now() END)",
                    )
                    .bind(ws)
                    .bind(ns)
                    .bind(k)
                    .bind(rev)
                    .bind(ago)
                    .bind(expired)
                    .bind(deleted)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
            };
            for k in ["k3", "kd", "kx3"] {
                seed(k, 1, 60.0, false, false).await;
                seed(k, 2, 50.0, false, false).await;
                seed(k, 3, 20.0, false, k == "kd").await;
            }
            seed("ke", 1, 60.0, false, false).await;
            seed("ke", 2, 30.0, true, false).await;
            seed("k1", 1, 60.0, false, false).await;
            let t_mid: chrono::DateTime<chrono::Utc> =
                sqlx::query_scalar("SELECT now() - interval '40 seconds'")
                    .fetch_one(&db.pool)
                    .await
                    .unwrap();
            let props = &cell["props"];
            let behavior = &props["behavior"];
            let pattern = props["keyPattern"].as_str().map(str::to_string);
            let mut size = 0;
            for k in ["k1", "k3", "kd", "ke", "kx3"] {
                if let Some(p) = &pattern {
                    if !egret_workflow::storage::nsfiles::glob::wildcard_match(p, k) {
                        continue;
                    }
                }
                let keep = behavior["keepAmount"].as_u64();
                let before = behavior["before"].as_str().map(|_| t_mid);
                let version = behavior["type"] == "version";
                size += purge_key(
                    &db.pool,
                    &fs,
                    &pgrant(ws, ns),
                    &key(k),
                    100,
                    move |rows: &[PurgeRow]| {
                        let latest = rows.last().map(|r| &r.entry);
                        let all = || rows.iter().map(|r| r.entry.id).collect();
                        if !version {
                            return if latest.is_some_and(|e| e.expired) {
                                all()
                            } else {
                                vec![]
                            };
                        }
                        if let Some(t) = before {
                            return rows
                                .iter()
                                .filter(|r| r.superseded_at.is_some_and(|s| s <= t))
                                .map(|r| r.entry.id)
                                .collect();
                        }
                        let keep = keep.unwrap() as usize;
                        if latest.is_some_and(|e| e.deleted) {
                            return all();
                        }
                        let cut = rows.len().saturating_sub(keep);
                        rows[..cut].iter().map(|r| r.entry.id).collect()
                    },
                )
                .await
                .unwrap();
            }
            assert_eq!(json!({"size": size}), cell["outputs"], "{name}");
            let mut surviving = serde_json::Map::new();
            for k in ["k1", "k3", "kd", "ke", "kx3"] {
                let r = revs(&rows_of(&db, ws, ns, k).await);
                if !r.is_empty() {
                    surviving.insert(k.into(), json!(r));
                }
            }
            assert_eq!(
                Value::Object(surviving),
                cell["surviving_revisions"],
                "{name}"
            );
            examined += 1;
        }
        assert_eq!(examined, 8);
    }

    /// The KV lock texts are `w9c:`, W9b's `w9b:`: a KV write of key `k`
    /// waits for no namespace-file lock of a path `/k`, and touches no
    /// namespace-file row.
    #[tokio::test]
    async fn a_kv_write_never_touches_namespace_file_rows_or_their_lock_keys() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let kv_text = lock_text(ws, NS3, "k");
        let files_text = format!("w9b:{}:{NS3}:/k", ws.hyphenated());
        let (kv_hash, files_hash): (i64, i64) =
            sqlx::query_as("SELECT hashtextextended($1, 0), hashtextextended($2, 0)")
                .bind(&kv_text)
                .bind(&files_text)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_ne!(kv_hash, files_hash);
        let mut holder = db.pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(files_hash)
            .execute(&mut *holder)
            .await
            .unwrap();
        let g = api_wgrant(ws, NS3, Duration::from_millis(200));
        written(set(&db.pool, &fs, &g, &key("k"), new(json!({"a": 1})), USER).await);
        let patch = KvData::from_json(&json!({"b": 1})).unwrap();
        written(put_merge(&db.pool, &fs, &g, &key("k"), patch, false, USER).await);
        assert_eq!(
            delete(&db.pool, &g, &key("k"), USER).await.unwrap(),
            Deleted::Deleted
        );
        let files: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow.namespace_files WHERE workspace_id = $1",
        )
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(files, 0);
        // And the KV text is the one `set` takes: held, a write is busy.
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(kv_hash)
            .execute(&mut *holder)
            .await
            .unwrap();
        assert!(matches!(
            set(&db.pool, &fs, &g, &key("k"), new(json!(1)), USER).await,
            Err(NsError::Conflict(_))
        ));
        // Review M7: both locks let go before the holder returns to the pool.
        let released: (bool, bool) =
            sqlx::query_as("SELECT pg_advisory_unlock($1), pg_advisory_unlock($2)")
                .bind(files_hash)
                .bind(kv_hash)
                .fetch_one(&mut *holder)
                .await
                .unwrap();
        assert_eq!(released, (true, true));
    }

    /// Holds the key's lock on its own connection for `hold`, then lets it go.
    async fn hold_lock(db: &Db, text: String, hold: Duration) -> tokio::task::JoinHandle<()> {
        let mut c = db.pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(&text)
            .execute(&mut *c)
            .await
            .unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(hold).await;
            sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
                .bind(&text)
                .execute(&mut *c)
                .await
                .unwrap();
        })
    }

    /// Task 3 review I4: a write that waited 3 s for the lock with a 2 s ttl
    /// is live when it commits (`now()` is the transaction's start, before
    /// the wait; the clock is read under the lock).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_set_that_waited_longer_than_its_ttl_commits_live() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let holder = hold_lock(&db, lock_text(ws, NS3, "slow"), Duration::from_secs(3)).await;
        let mut nv = new(json!("v"));
        nv.ttl = Some(chrono::TimeDelta::seconds(2));
        let started = std::time::Instant::now();
        let e = written(set(&db.pool, &fs, &g, &key("slow"), nv, USER).await);
        assert!(
            started.elapsed() >= Duration::from_millis(2900),
            "it did not wait"
        );
        holder.await.unwrap();
        assert!(!e.expired, "born expired: {e:?}");
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "slow").await,
            Some(json!("v"))
        );
    }

    /// I4: `overwrite: false` over a key that expired while the writer
    /// waited for the lock writes.
    #[tokio::test(flavor = "multi_thread")]
    async fn overwrite_false_over_a_key_that_expired_during_the_wait_writes() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let mut nv = new(json!("first"));
        nv.ttl = Some(chrono::TimeDelta::seconds(2));
        written(set(&db.pool, &fs, &g, &key("exp"), nv, USER).await);
        // Live when the second writer begins, expired when it gets the lock.
        let holder = hold_lock(&db, lock_text(ws, NS3, "exp"), Duration::from_secs(3)).await;
        let mut nv = new(json!("second"));
        nv.overwrite = false;
        let e = written(set(&db.pool, &fs, &g, &key("exp"), nv, USER).await);
        holder.await.unwrap();
        assert_eq!(e.revision, 2);
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "exp").await,
            Some(json!("second"))
        );
    }

    /// I4: `created_at` rises with the revision even when a writer began its
    /// transaction before the one that took the lock first. Another session
    /// holds the key's lock and writes revision 1 as a writer does, a second
    /// after the waiting writer began; the waiter's revision 2 is later.
    #[tokio::test(flavor = "multi_thread")]
    async fn created_at_rises_with_the_revision() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let text = lock_text(ws, NS3, "mono");
        let mut first = db.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&text)
            .execute(&mut *first)
            .await
            .unwrap();
        let waiter = {
            let (p, fs, g) = (db.pool.clone(), fs.clone(), g.clone());
            tokio::spawn(async move { set(&p, &fs, &g, &key("mono"), new(json!("2")), USER).await })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        sqlx::query(
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
               size, created_at) VALUES ($1, $2, 'mono', 1, 'STRING', 3, clock_timestamp())",
        )
        .bind(ws)
        .bind(NS3)
        .execute(&mut *first)
        .await
        .unwrap();
        fs.store
            .put(&fs.namespace_kv_uri(&g, &key("mono"), 1), b"\"1\"".to_vec())
            .await
            .unwrap();
        first.commit().await.unwrap();
        let e = written(waiter.await.unwrap());
        assert_eq!(e.revision, 2);
        let rows = rows_of(&db, ws, NS3, "mono").await;
        assert!(
            rows[0].created_at < rows[1].created_at,
            "revision 1 at {}, revision 2 at {}",
            rows[0].created_at,
            rows[1].created_at
        );
    }

    /// A `Number` value, stored as `12345`.
    fn number_value() -> NewValue {
        NewValue {
            value: KvValue {
                kind: KvType::Number,
                data: KvData::Number(KvNumber::Int("12345".into())),
            },
            description: None,
            ttl: None,
            overwrite: true,
        }
    }

    /// Review M3 and re-review 1: an object that is not the size its row
    /// records is never decoded under the row's type (mutation t). Its row
    /// unchanged, the mismatch is permanent (the object altered outside
    /// egret): `Author`, never retried, on a read and under `Put`'s lock.
    #[tokio::test]
    async fn an_object_altered_under_an_unchanged_row_is_the_authors_not_retried() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        written(set(&db.pool, &fs, &g, &key("aba"), number_value(), USER).await);
        let mut c = conn(&db).await;
        let e = resolve(&mut c, &grant(ws, NS3), &key("aba"), Want::Latest)
            .await
            .unwrap();
        drop(c);
        store
            .mem
            .put(
                &fs.namespace_kv_uri(&g, &key("aba"), 1),
                b"\"a string\"".to_vec(),
            )
            .await
            .unwrap();
        for _ in 0..2 {
            match read_value(&db.pool, &fs, &grant(ws, NS3), &e).await {
                Err(NsError::Author(m)) => assert!(
                    m.contains("revision 1 of key 'aba'")
                        && m.contains("does not match its record")
                        && m.contains("5 bytes recorded, 10 read"),
                    "{m}"
                ),
                other => panic!("{other:?}"),
            }
        }
        match put_merge(&db.pool, &fs, &g, &key("aba"), KvData::Null, false, USER).await {
            Err(NsError::Author(m)) => assert!(m.contains("does not match its record"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(revs(&rows_of(&db, ws, NS3, "aba").await), [1]);
    }

    /// Re-review 1: the row was purged and the revision number reused WHILE
    /// it was read, so the object is the new value's. The row read again by
    /// id is gone: `Store`, retried, and the retry reads the new value.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_read_racing_a_purge_and_a_rewrite_is_retried_and_the_retry_reads_the_new_value() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        written(set(&db.pool, &fs, &g, &key("aba"), number_value(), USER).await);
        let mut c = conn(&db).await;
        let e = resolve(&mut c, &grant(ws, NS3), &key("aba"), Want::Latest)
            .await
            .unwrap();
        drop(c);
        let hold = Hold::new();
        *store.get_hold.lock().unwrap() = Some(hold.clone());
        let reader = {
            let (p, fs) = (db.pool.clone(), fs.clone());
            tokio::spawn(async move { read_value(&p, &fs, &grant(ws, NS3), &e).await })
        };
        hold.entered.wait().await;
        assert_eq!(
            purge_key(&db.pool, &fs, &pgrant(ws, NS3), &key("aba"), 10, |rows| {
                rows.iter().map(|r| r.entry.id).collect()
            })
            .await
            .unwrap(),
            1
        );
        let e2 = written(set(&db.pool, &fs, &g, &key("aba"), new(json!("a string")), USER).await);
        assert_eq!(e2.revision, 1, "the number is reused");
        hold.release.wait().await;
        match reader.await.unwrap() {
            Err(NsError::Store(m)) => {
                assert!(
                    m.contains("'aba'") && m.contains("changed while it was read"),
                    "{m}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            latest_value(&db, &fs, ws, NS3, "aba").await,
            Some(json!("a string"))
        );
    }

    /// C1 (re-review 1): a 50 000-segment key took 88 s and then aborted the
    /// process on a blocking-pool thread's 2 MiB stack. `put_merge` runs the
    /// merge on that pool: the key is refused in milliseconds, naming the
    /// limit, and nothing is written. A key of exactly `MAX_DEPTH` segments
    /// is written and reads back.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_put_key_deeper_than_max_depth_is_refused_at_once_and_one_at_it_is_written() {
        use egret_workflow::storage::ion::MAX_DEPTH;
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        let entry = |k: String| data_of(json!([{"key": k, "value": 1}]));
        for n in [MAX_DEPTH + 1, 50_000] {
            let started = std::time::Instant::now();
            let got = put_merge(
                &db.pool,
                &fs,
                &g,
                &key("deep"),
                entry(vec!["a"; n].join(".")),
                false,
                USER,
            )
            .await;
            let took = started.elapsed();
            match got {
                Err(NsError::Author(m)) => assert!(
                    m.contains(&format!("{n} dotted segments"))
                        && m.contains(&format!("more than the {MAX_DEPTH} levels")),
                    "{m}"
                ),
                other => panic!("{n}: {other:?}"),
            }
            assert!(took < Duration::from_secs(1), "{n} segments took {took:?}");
            assert!(rows_of(&db, ws, NS3, "deep").await.is_empty());
        }
        written(
            put_merge(
                &db.pool,
                &fs,
                &g,
                &key("deep"),
                entry(vec!["a"; MAX_DEPTH].join(".")),
                false,
                USER,
            )
            .await,
        );
        let mut got = latest_value(&db, &fs, ws, NS3, "deep").await.unwrap();
        for _ in 0..MAX_DEPTH {
            got = got["a"].take();
        }
        assert_eq!(got, json!(1));
    }

    /// C1's budget: a patch's dotted segments in all are at most the
    /// dense-value limit, `max_read_bytes / 64` (100 at a cap of 6 400
    /// bytes), refused before the lock as `TooLarge` naming it. Ten keys
    /// sharing a 9-segment prefix: at 100 segments the patch is written; at
    /// 101 it is refused, although its merged value (one map more) is
    /// storable -- a cap one segment larger writes it -- so the budget, not
    /// the value's own limit, refuses it.
    #[tokio::test]
    async fn a_put_patch_over_the_segment_budget_is_refused_and_one_at_it_is_written() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(100 * 64);
        let g = wgrant(ws, NS3);
        let patch = |last: &str| {
            data_of(Value::Array(
                (0..10)
                    .map(|i| {
                        let leaf = if i == 9 {
                            format!("k9{last}")
                        } else {
                            format!("k{i}")
                        };
                        json!({"key": format!("a.b.c.d.e.f.g.h.i.{leaf}"), "value": 1})
                    })
                    .collect(),
            ))
        };
        let over = patch(".x");
        match put_merge(&db.pool, &fs, &g, &key("budget"), over.clone(), false, USER).await {
            Err(NsError::TooLarge(m)) => assert!(
                m.contains("101 dotted segments")
                    && m.contains("more than the 100")
                    && m.contains("max_read_bytes"),
                "{m}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(rows_of(&db, ws, NS3, "budget").await.is_empty());
        let (roomy, _) = hooked_fs(101 * 64);
        written(put_merge(&db.pool, &roomy, &g, &key("roomy"), over, false, USER).await);
        written(put_merge(&db.pool, &fs, &g, &key("budget"), patch(""), false, USER).await);
        let mut got = latest_value(&db, &fs, ws, NS3, "budget").await.unwrap();
        for step in ["a", "b", "c", "d", "e", "f", "g", "h", "i"] {
            got = got[step].take();
        }
        assert_eq!(got.as_object().unwrap().len(), 10);
        assert_eq!(got["k9"], json!(1));
    }

    fn data_of(v: Value) -> KvData {
        KvData::from_json(&v).unwrap()
    }

    /// Concern 4 (re-review 1): a panic inside a KV writer's detached block
    /// is egret's bug, `Author` (fails once), never a `Db` outage re-claimed
    /// forever. `set`'s staging object is left for the staging sweeper, and
    /// the key's lock is let go.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_panic_inside_a_kv_write_is_an_egret_bug_never_retried() {
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, store) = hooked_fs(DEFAULT_CAP);
        let g = wgrant(ws, NS3);
        written(set(&db.pool, &fs, &g, &key("p"), new(json!({"a": 1})), USER).await);
        store.panic_copy.store(true, Ordering::SeqCst);
        match set(&db.pool, &fs, &g, &key("p"), new(json!({"a": 2})), USER).await {
            Err(NsError::Author(m)) => assert!(
                m.contains("internal error (an egret bug, not retried)")
                    && m.contains("a KV write stopped"),
                "{m}"
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            (staging_rows(&db, ws).await, staging_objects(&store.mem)),
            (1, 1),
            "the staging object is the sweeper's"
        );
        store.panic_kv_put.store(true, Ordering::SeqCst);
        match put_merge(
            &db.pool,
            &fs,
            &g,
            &key("p"),
            data_of(json!({"b": 1})),
            false,
            USER,
        )
        .await
        {
            Err(NsError::Author(m)) => assert!(
                m.contains("internal error (an egret bug, not retried)")
                    && m.contains("a KV put stopped"),
                "{m}"
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(revs(&rows_of(&db, ws, NS3, "p").await), [1]);
        let api = api_wgrant(ws, NS3, Duration::from_secs(2));
        assert_eq!(
            written(set(&db.pool, &fs, &api, &key("p"), new(json!({"a": 3})), USER).await).revision,
            2,
            "the lock was let go"
        );
    }

    /// How many backends wait for the advisory lock of `text`.
    async fn waiting_on(db: &Db, text: &str) -> i64 {
        sqlx::query_scalar(
            "WITH k AS (SELECT hashtextextended($1, 0) AS h) \
             SELECT count(*) FROM pg_locks, k \
              WHERE locktype = 'advisory' AND NOT granted AND objsubid = 1 \
                AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
                AND classid::bigint = ((k.h >> 32) & 4294967295) \
                AND objid::bigint = (k.h & 4294967295)",
        )
        .bind(text)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    /// Waits until `n` backends wait on `text`'s lock (at most 10 s).
    async fn until_waiting(db: &Db, text: &str, n: i64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let now = waiting_on(db, text).await;
            if now >= n {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{now} waiting, not {n}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Takes `text`'s session lock on a connection of its own.
    async fn lock_on_the_side(db: &Db, text: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
        let mut c = db.pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(text)
            .execute(&mut *c)
            .await
            .unwrap();
        c
    }

    async fn unlock_on_the_side(mut c: sqlx::pool::PoolConnection<sqlx::Postgres>, text: &str) {
        let released: bool =
            sqlx::query_scalar("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
                .bind(text)
                .fetch_one(&mut *c)
                .await
                .unwrap();
        assert!(released);
    }

    fn bulk_delete(
        p: &PgPool,
        fs: &storage::FileStore,
        g: &Grant,
        ks: &[&str],
    ) -> tokio::task::JoinHandle<Result<Vec<KvKey>, NsError>> {
        let (p, fs, g) = (p.clone(), fs.clone(), g.clone());
        let ks: Vec<KvKey> = ks.iter().map(|k| key(k)).collect();
        tokio::spawn(async move { delete_many(&p, &fs, &g, &ks, USER).await })
    }

    /// Concern 4 (re-review 1, review M2): at most `BULK_DELETES_AT_ONCE`
    /// (2) bulk deletes hold key locks at once. Three wait on one held key
    /// with a long lock wait: exactly 2 backends wait on its lock, the third
    /// on a permit, holding no connection. Released, all three finish.
    #[tokio::test(flavor = "multi_thread")]
    async fn at_most_two_bulk_deletes_wait_on_key_locks_at_once() {
        let _serial = BULK_SERIAL.lock().await;
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let p = pool(&db, 6, "w9c-t3-bulk").await;
        written(
            set(
                &db.pool,
                &fs,
                &wgrant(ws, NS3),
                &key("held"),
                new(json!(1)),
                USER,
            )
            .await,
        );
        let text = lock_text(ws, NS3, "held");
        let side = lock_on_the_side(&db, &text).await;
        let g = api_wgrant(ws, NS3, Duration::from_secs(60));
        let calls: Vec<_> = (0..3)
            .map(|_| bulk_delete(&p, &fs, &g, &["held"]))
            .collect();
        until_waiting(&db, &text, 2).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(waiting_on(&db, &text).await, 2, "waiting on the key's lock");
        unlock_on_the_side(side, &text).await;
        let mut gone = Vec::new();
        for call in calls {
            gone.push(call.await.unwrap().unwrap().len());
        }
        gone.sort_unstable();
        assert_eq!(gone, [0, 0, 1]);
    }

    /// Re-review 1: an API bulk delete waits for a permit at most its lock
    /// wait, then answers 409, while two others hold both permits.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_third_api_bulk_delete_gets_409_while_two_hold_the_permits() {
        let _serial = BULK_SERIAL.lock().await;
        let Some(db) = common::db().await else { return };
        let (ws, _) = common::seed_workspace(&db).await;
        let (fs, _) = hooked_fs(DEFAULT_CAP);
        let p = pool(&db, 6, "w9c-t3-bulk409").await;
        for k in ["held", "free"] {
            written(
                set(
                    &db.pool,
                    &fs,
                    &wgrant(ws, NS3),
                    &key(k),
                    new(json!(1)),
                    USER,
                )
                .await,
            );
        }
        let text = lock_text(ws, NS3, "held");
        let side = lock_on_the_side(&db, &text).await;
        let long = api_wgrant(ws, NS3, Duration::from_secs(60));
        let holders: Vec<_> = (0..2)
            .map(|_| bulk_delete(&p, &fs, &long, &["held"]))
            .collect();
        until_waiting(&db, &text, 2).await;
        let short = api_wgrant(ws, NS3, Duration::from_millis(300));
        let started = std::time::Instant::now();
        let third = tokio::time::timeout(
            Duration::from_secs(5),
            bulk_delete(&p, &fs, &short, &["free"]),
        )
        .await
        .expect("the third bulk delete waited past its lock wait")
        .unwrap();
        let took = started.elapsed();
        match third {
            Err(NsError::Conflict(m)) => {
                assert!(m.contains(NS3) && m.contains("bulk deletes"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(latest_value(&db, &fs, ws, NS3, "free").await.is_some());
        unlock_on_the_side(side, &text).await;
        let mut gone = Vec::new();
        for h in holders {
            gone.push(h.await.unwrap().unwrap().len());
        }
        gone.sort_unstable();
        assert_eq!(gone, [0, 1]);
    }

    /// Task 3 re-review 2, H1: a value or a patch nested 10,000 deep, built
    /// by hand (no constructor builds one), is refused by `set` and
    /// `put_merge` before anything clones or encodes it, as `Author`, with no
    /// abort, on a 2 MiB stack (the runtime's workers and blocking pool), and
    /// nothing is staged or written.
    #[test]
    fn a_value_nested_10000_deep_is_refused_by_set_and_put_merge() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .thread_stack_size(2 * 1024 * 1024)
                    .build()
                    .unwrap()
                    .block_on(async {
                        let Some(db) = common::db().await else { return };
                        let (ws, _) = common::seed_workspace(&db).await;
                        let (fs, store) = hooked_fs(DEFAULT_CAP);
                        let g = wgrant(ws, NS3);
                        let deep = || {
                            let mut d = KvData::Bool(true);
                            for _ in 0..10_000 {
                                d = KvData::Struct(vec![("d".into(), d)]);
                            }
                            d
                        };
                        let nv = NewValue {
                            value: KvValue {
                                kind: KvType::Json,
                                data: deep(),
                            },
                            description: None,
                            ttl: None,
                            overwrite: true,
                        };
                        match set(&db.pool, &fs, &g, &key("deep"), nv, USER).await {
                            Err(NsError::Author(m)) => assert!(
                                m.contains("'deep'")
                                    && m.contains("deeper than 100 levels")
                                    && m.contains("does not store"),
                                "{m}"
                            ),
                            other => panic!("{other:?}"),
                        }
                        match put_merge(&db.pool, &fs, &g, &key("deep"), deep(), false, USER).await
                        {
                            Err(NsError::Author(m)) => assert!(
                                m.contains("the patch nests containers deeper than 102 levels"),
                                "{m}"
                            ),
                            other => panic!("{other:?}"),
                        }
                        assert!(rows_of(&db, ws, NS3, "deep").await.is_empty());
                        assert_eq!(store.staging_puts.load(Ordering::SeqCst), 0);
                    })
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
