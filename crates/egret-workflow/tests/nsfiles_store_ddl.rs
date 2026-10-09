//! W9b Task 3 fix round 2, M-R2: a move whose parent-directory insert fails
//! after every copy succeeded purges the copies. The failure is injected by
//! a `CREATE TRIGGER` on `workflow.namespace_files`, whose DDL takes table
//! locks that would stall `nsfiles_store`'s gated lock tests, so this test
//! has its own binary: its own process and per-process test database
//! (fix round 2, M-R2a). The object store is an in-memory `MemStore`.
//! Requires DATABASE_URL.

mod common;

use std::sync::{Arc, Mutex};

use egret_metastore::Db;
use egret_storage::{MemStore, ObjectStore, StorageError};
use egret_workflow::storage;
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{move_path, put, Conflict, NsEntry, Written};
use egret_workflow::storage::nsfiles::{CreatedBy, NsError};
use tokio::io::AsyncRead;
use uuid::Uuid;

const W: &str = "w9b.t3";

fn grant(ws: Uuid, access: Access) -> Grant {
    authorize(ws, Actor::Api, W, Resource::NamespaceFiles, access).unwrap()
}

fn by() -> CreatedBy {
    CreatedBy::Execution(Uuid::nil())
}

fn dir(p: &str) -> NsPath {
    NsPath::parse_dir(p).unwrap()
}

/// A `MemStore` that records every `delete`, in order.
#[derive(Default)]
struct Recording {
    mem: MemStore,
    deletes: Mutex<Vec<String>>,
}

impl Recording {
    /// Every key under this workspace's namespace-file prefix, with its bytes.
    async fn objects(&self, fs: &storage::FileStore, ws: Uuid) -> Vec<(String, String)> {
        let prefix = format!("s3://{}/{}{ws}/", fs.bucket, fs.prefix);
        let mut keys: Vec<String> = self
            .mem
            .keys()
            .into_iter()
            .filter(|k| k.starts_with(&prefix))
            .collect();
        keys.sort();
        let mut out = Vec::new();
        for k in keys {
            let b = String::from_utf8(self.mem.get(&k).await.unwrap()).unwrap();
            out.push((k[prefix.len()..].to_string(), b));
        }
        out
    }
}

#[async_trait::async_trait]
impl ObjectStore for Recording {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.mem.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.mem.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.deletes.lock().unwrap().push(uri.to_string());
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
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.mem.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        self.mem.get_stream(uri).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.mem.head(uri).await
    }
}

fn recording_fs() -> (Arc<Recording>, storage::FileStore) {
    let store = Arc::new(Recording::default());
    let fs = storage::FileStore {
        store: store.clone(),
        bucket: "egret-test".into(),
        prefix: "workflow/".into(),
        caps: storage::Caps::default(),
        retention_days: 30,
    };
    (store, fs)
}

async fn written(db: &Db, fs: &storage::FileStore, g: &Grant, p: &str, bytes: &str) -> NsEntry {
    let mut body: &[u8] = bytes.as_bytes();
    let path = NsPath::parse_file(p).unwrap();
    match put(&db.pool, fs, g, &path, &mut body, Conflict::Overwrite, by()).await {
        Ok(Written::Written(e)) => e,
        other => panic!("{p}: {other:?}"),
    }
}

/// Every row of the namespace: (path, revision, deleted, size), by path.
async fn all_rows(db: &Db, ws: Uuid) -> Vec<(String, i32, bool, i64)> {
    sqlx::query_as(
        "SELECT path, revision, deleted, size FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 ORDER BY path COLLATE \"C\", revision",
    )
    .bind(ws)
    .bind(W)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// M-R2: every copy has succeeded and then the insert of `to`'s missing
/// parent (`/n/`) fails at the database. The copies are purged, last first,
/// exactly as for a failed copy; the source is untouched.
///
/// The failure is injected by a `BEFORE INSERT` trigger whose `WHEN` matches
/// only this test's workspace and the path `/n/`, so no other test's insert
/// can reach it; it is dropped right after the move, before any assertion.
#[tokio::test]
async fn a_move_whose_parent_insert_fails_after_the_copies_purges_them() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let (store, fs) = recording_fs();
    let g = grant(ws, Access::Write);
    written(&db, &fs, &g, "/s/a", "a1").await;
    written(&db, &fs, &g, "/s/a", "a2").await;
    let before = (all_rows(&db, ws).await, store.objects(&fs, ws).await);

    sqlx::query(
        "CREATE OR REPLACE FUNCTION public.w9b_t3_refuse_insert() RETURNS trigger \
         LANGUAGE plpgsql AS $$ BEGIN \
           RAISE EXCEPTION 'injected by a W9b Task 3 test'; \
         END $$",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let trigger = format!("w9b_t3_refuse_{}", ws.simple());
    // The only interpolations are a `Uuid`'s hex and hyphenated forms.
    let create = format!(
        "CREATE TRIGGER {trigger} BEFORE INSERT ON workflow.namespace_files FOR EACH ROW \
         WHEN (NEW.workspace_id = '{ws}' AND NEW.path = '/n/') \
         EXECUTE FUNCTION public.w9b_t3_refuse_insert()"
    );
    sqlx::query(sqlx::AssertSqlSafe(create))
        .execute(&db.pool)
        .await
        .unwrap();
    store.deletes.lock().unwrap().clear();
    let moved = move_path(&db.pool, &fs, &g, &dir("/s"), &dir("/n/t"), by()).await;
    // Dropped before any assertion can panic.
    let drop = format!("DROP TRIGGER {trigger} ON workflow.namespace_files");
    sqlx::query(sqlx::AssertSqlSafe(drop))
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        (all_rows(&db, ws).await, store.objects(&fs, ws).await),
        before,
        "the source is untouched and no copy is left"
    );
    let prefix = format!("s3://egret-test/workflow/{ws}/w9b/t3/_files");
    assert_eq!(
        *store.deletes.lock().unwrap(),
        [format!("{prefix}/n/t/a.v2"), format!("{prefix}/n/t/a")],
        "the copies were removed last first"
    );
    let err = moved.unwrap_err();
    match &err {
        NsError::Db(egret_core::Error::Internal(m)) => {
            let m = m.to_string();
            assert!(
                m.contains("'/s/'") && m.contains("'/n/t/'") && m.contains("nothing was moved"),
                "{m}"
            )
        }
        other => panic!("{other:?}"),
    }
}
