//! View governance B1: which engine the `egret-lineage` EXPLAIN runs on, and
//! what the runner refuses before any engine is asked.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use egret_metastore::Db;
use egret_perm_compile::lineage::ExplainRunner;
use egret_query::execution::lineage::{
    lineage_engines_via, LineageTokenMinter, TrinoLineageRunner,
};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct CountingMinter(AtomicUsize);

#[async_trait::async_trait]
impl LineageTokenMinter for CountingMinter {
    async fn mint_lineage_token(&self) -> Result<String, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("eyJ.lineage.sig".into())
    }
}

struct Engine<'a> {
    name: &'a str,
    origin: &'a str,
    status: &'a str,
    mode: &'a str,
    apply_status: &'a str,
    connection: bool,
}

const GOOD: Engine<'static> = Engine {
    name: "",
    origin: "provisioned",
    status: "running",
    mode: "jwt",
    apply_status: "applied",
    connection: true,
};

async fn catalog(db: &Db) -> (String, Uuid, Uuid) {
    let pool = &db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'W') RETURNING id")
            .bind(unique("lr-ws"))
            .fetch_one(pool)
            .await
            .unwrap();
    let name = unique("lr_cat");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'tpch', '{}'::jsonb) RETURNING id",
    )
    .bind(&name)
    .fetch_one(pool)
    .await
    .unwrap();
    (name, id, ws)
}

async fn engine(db: &Db, catalog_id: Uuid, ws: Uuid, e: Engine<'_>) {
    let pool = &db.pool;
    let id: Uuid = if e.origin == "external" {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, origin, \
             external_host, external_port, ranger_plugin_version) \
             VALUES ($1,'trino','','{}'::jsonb,'running','external','h',8443,'2.8.0') RETURNING id",
        )
        .bind(e.name)
        .fetch_one(pool)
        .await
        .unwrap()
    } else {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status) \
             VALUES ($1,'trino','egret',jsonb_build_object('trinoConfig', \
                     jsonb_build_object('delegationMode', $2::text)),$3) RETURNING id",
        )
        .bind(e.name)
        .bind(e.mode)
        .bind(e.status)
        .fetch_one(pool)
        .await
        .unwrap()
    };
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, apply_status, \
         apply_error) VALUES ($1,$2,$3, CASE WHEN $3 = 'failed' THEN 'x' END)",
    )
    .bind(id)
    .bind(catalog_id)
    // An external decoy is 'applied' too, so only the origin filter excludes it.
    .bind(e.apply_status)
    .execute(pool)
    .await
    .unwrap();
    if e.connection {
        sqlx::query(
            "INSERT INTO query.databases (name, engine, host, port, options, workspace_id, \
             delegated, engine_id) VALUES ($1,'trino',$2,8443,$3,$4,true,$5)",
        )
        .bind(unique("lr-db"))
        .bind(format!("{}-trino.egret.svc", e.name))
        .bind(serde_json::json!({"http_scheme": "https"}))
        .bind(ws)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn only_running_provisioned_jwt_engines_with_the_catalog_applied_are_used() {
    let Some(db) = test_db().await else { return };
    let (cat, cat_id, ws) = catalog(&db).await;
    let b = unique("lr-b");
    let a = unique("lr-a");
    engine(&db, cat_id, ws, Engine { name: &b, ..GOOD }).await;
    engine(&db, cat_id, ws, Engine { name: &a, ..GOOD }).await;
    for decoy in [
        Engine {
            name: &unique("lr-ext"),
            origin: "external",
            ..GOOD
        },
        Engine {
            name: &unique("lr-imp"),
            mode: "impersonate",
            ..GOOD
        },
        Engine {
            name: &unique("lr-prov"),
            status: "provisioning",
            ..GOOD
        },
        Engine {
            name: &unique("lr-fail"),
            apply_status: "failed",
            ..GOOD
        },
        Engine {
            name: &unique("lr-noconn"),
            connection: false,
            ..GOOD
        },
    ] {
        engine(&db, cat_id, ws, decoy).await;
    }

    let mut conn = db.pool.acquire().await.unwrap();
    let got = lineage_engines_via(&mut conn, &cat).await.unwrap();
    let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
    let mut want = vec![a.as_str(), b.as_str()];
    want.sort();
    assert_eq!(names, want);
    assert_eq!(got[0].port, 8443);
    assert!(lineage_engines_via(&mut conn, "no_such_catalog")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_runner_refuses_before_minting_when_no_engine_or_the_sql_is_an_explain() {
    let Some(db) = test_db().await else { return };
    let minter = Arc::new(CountingMinter(AtomicUsize::new(0)));
    let runner = TrinoLineageRunner::new(db.pool.clone(), minter.clone(), Duration::from_secs(5));

    let err = runner
        .explain_io("no_such_catalog", "s", "SELECT 1")
        .await
        .expect_err("no engine");
    assert!(err.contains("no_such_catalog"), "{err}");
    let err = runner
        .explain_io("no_such_catalog", "s", "EXPLAIN ANALYZE SELECT 1")
        .await
        .expect_err("nested explain");
    assert!(err.contains("EXPLAIN"), "{err}");
    assert_eq!(
        minter.0.load(Ordering::SeqCst),
        0,
        "no token minted for a refusal"
    );
}

/// An engine that cannot be reached is skipped and named; with none left the
/// runner refuses, saying so. (`127.0.0.1:1` refuses connections.)
#[tokio::test]
async fn an_unreachable_engine_is_named_in_the_refusal() {
    let Some(db) = test_db().await else { return };
    let (cat, cat_id, ws) = catalog(&db).await;
    let name = unique("lr-down");
    engine(
        &db,
        cat_id,
        ws,
        Engine {
            name: &name,
            ..GOOD
        },
    )
    .await;
    sqlx::query("UPDATE query.databases SET host = '127.0.0.1', port = 1 WHERE name LIKE 'lr-db%' AND engine_id = (SELECT id FROM engine_mgmt.engines WHERE name = $1)")
        .bind(&name)
        .execute(&db.pool)
        .await
        .unwrap();
    let minter = Arc::new(CountingMinter(AtomicUsize::new(0)));
    let runner = TrinoLineageRunner::new(db.pool.clone(), minter.clone(), Duration::from_secs(5));
    let err = runner
        .explain_io(&cat, "s", "SELECT 1")
        .await
        .expect_err("unreachable");
    assert!(err.contains(&name) && err.contains("reachable"), "{err}");
    assert_eq!(minter.0.load(Ordering::SeqCst), 1);
}

/// DG frontend redesign #1: the catalog lister dials only the engine it was
/// asked for, and only when that engine is one the lineage EXPLAIN could use;
/// anything else is refused before a token is minted.
#[tokio::test]
async fn the_catalog_lister_refuses_an_ineligible_engine_before_minting() {
    let Some(db) = test_db().await else { return };
    let (cat, cat_id, ws) = catalog(&db).await;
    let imp = unique("cl-imp");
    let ext = unique("cl-ext");
    engine(
        &db,
        cat_id,
        ws,
        Engine {
            name: &imp,
            mode: "impersonate",
            ..GOOD
        },
    )
    .await;
    engine(
        &db,
        cat_id,
        ws,
        Engine {
            name: &ext,
            origin: "external",
            ..GOOD
        },
    )
    .await;
    let minter = Arc::new(CountingMinter(AtomicUsize::new(0)));
    let lister = egret_query::execution::catalog_listing::TrinoCatalogLister::new(
        db.pool.clone(),
        minter.clone(),
        Duration::from_secs(5),
    );

    for name in [imp.as_str(), ext.as_str(), "no-such-engine"] {
        let err = lister
            .list(name, &cat, None, None)
            .await
            .expect_err("not an engine the lister may dial");
        assert!(err.contains(name), "{err}");
    }
    let err = lister
        .list(&imp, &cat, None, Some("t"))
        .await
        .expect_err("a table needs a schema");
    assert!(err.contains("schema"), "{err}");
    assert_eq!(
        minter.0.load(Ordering::SeqCst),
        0,
        "no token minted for a refusal"
    );
}
