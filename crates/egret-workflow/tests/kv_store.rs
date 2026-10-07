//! W9c Task 2: the read side of `workflow.kv_entries` (migration 0175) and
//! the objects behind it. Requires DATABASE_URL.
//!
//! Rows are inserted with plain SQL (the writers arrive in Task 3). Every
//! test makes its own workspaces, so the tests cannot see each other's rows.
//! The only object stores touched are `MemStore`s: nothing here deletes from
//! a shared bucket.
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
                read_value(&fs, &g, &e).await.unwrap();
            }
        }
        let _ = resolve_first_live(&mut c, &[gc.clone(), g.clone()], &key(k)).await;
        let _ = revisions(&mut c, &g, &key(k)).await;
    }
    for e in revisions(&mut c, &g, &key("old")).await.unwrap() {
        // An expired revision's value, read directly: still no write.
        read_value(&fs, &g, &e).await.unwrap();
    }
    list_live(&mut c, &g, None, 10).await.unwrap();
    list_live(&mut c, &gc, Some("o"), 10).await.unwrap();
    list_inherited(&mut c, child, std::slice::from_ref(&g), 10)
        .await
        .unwrap();
    rows_page(&mut c, &g, None, 10).await.unwrap();
    namespaces_with_kv(&mut c, ws).await.unwrap();
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
struct CountStatements(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountStatements {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
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
                    Some(read_value(&fs, g, &e).await.unwrap().to_json())
                }
                None => None,
            },
            Some(ns) => {
                let g = grant(ws, ns);
                match resolve(&mut c, &g, &key(k), Want::Latest).await {
                    Ok(e) => Some(read_value(&fs, &g, &e).await.unwrap().to_json()),
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
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = {
        use tracing_subscriber::layer::SubscriberExt as _;
        tracing_subscriber::registry().with(CountStatements(count.clone()))
    };
    let _guard = tracing::subscriber::set_default(subscriber);
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
    assert_eq!(namespaces_with_kv(&mut c, ws).await.unwrap(), [NS]);
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
    assert!(namespaces_with_kv(&mut c, empty).await.unwrap().is_empty());
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
        let err = read_value(&fs, &wrong, &e).await.unwrap_err();
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
    let err = read_value(&fs, &g, &e).await.unwrap_err();
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
    assert_eq!(read_value(&fs, &g, &e1).await.unwrap().to_json(), "one");
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
    let err = read_value(&fs, &g, &e).await.unwrap_err();
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
    let err = read_value(&fs, &g, &e).await.unwrap_err();
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
    assert_eq!(read_value(&fs, &g, &e).await.unwrap().to_json(), "s");
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
    match read_value(&fs, &g, &e).await.unwrap_err() {
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
    let read = read_value(&fs, &g, &e).await.unwrap();
    let took = started.elapsed();
    let ticked = ticks.load(Ordering::Relaxed) - before;
    ticker.abort();
    eprintln!(
        "read_value of {} bytes took {took:?}; the ticker ticked {ticked} times",
        bytes.len()
    );
    let KvData::List(items) = read else {
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
