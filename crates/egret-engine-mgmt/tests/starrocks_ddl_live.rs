//! Engine-gated: the real DDL runner against a real StarRocks FE. Self-skips
//! unless EGRET_TEST_STARROCKS_URL is set, so it reports `ok` while asserting
//! nothing — the test's DURATION is what distinguishes a pass from a skip.
//!
//! This is the only instrument that judges the STATEMENTS this branch builds.
//! Every property name in `render/starrocks.rs` was read out of the FE source;
//! this is where "read correctly" is tested rather than argued.
//!
//! Uses a `jdbc` catalog against the dev-compose's own Postgres (reachable
//! from the FE container as `postgres:5432`), NOT `tpch`/`benchmark` as the
//! SP3b plan's illustrative brief has it. Measured live: the deployed dev FE
//! (`starrocks/allin1-ubuntu:latest`, `@@version_comment` = `4.1.4-4a9848e`)
//! answers `CREATE EXTERNAL CATALOG ... "type"="benchmark"` with `ERROR 1064
//! (HY000): Getting analyzing error. Detail message: [type : benchmark] is
//! not supported.'`. The vendored FE source this branch's property names were
//! read against (`data-portal/starrocks`, checked out at `main`) added
//! `BenchmarkConnector` on 2026-01-22 (commit `c019a1481e`), but that commit
//! is NOT an ancestor of the running build's revision (`4a9848edf03`,
//! 2026-08-05) and vice versa — divergent history, i.e. the `benchmark`
//! connector lives on `main` only and has never shipped in any released 4.1.x
//! branch build. So no `4.1-latest`-pinned engine egret actually provisions
//! (design D4) can take a `tpch`/`tpcds` catalog today either; that is a
//! rendering-layer (Task 1/2) / D4 concern, out of this task's scope to fix.
//! `jdbc` is the next-best infra-free target: no extra service beyond what
//! `docker-compose.dev.yml` already runs, and it is fully supported by 4.1.4.

use std::collections::BTreeMap;

use egret_catalog_registry::{render_starrocks, CatalogDef, StarRocksRenderInput};
use egret_engine_mgmt::{compare_to_fe, show_catalogs, show_create_catalog};
use egret_engine_mgmt::{Drift, FeTarget, MysqlDdlRunner, StarRocksDdl};
use serde_json::json;
use uuid::Uuid;

fn fe() -> Option<FeTarget> {
    let base = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some(FeTarget {
        host: u.host_str()?.to_string(),
        port: u.port().unwrap_or(9030),
        root_password: None, // the dev allin1 container's root has none
        tls_cert_pem: None,
    })
}

/// A `jdbc`/postgresql `CatalogDef` pointed at the dev-compose's own Postgres
/// (`postgres:5432`, database `egret`), reachable in-cluster from the FE
/// container on the same docker network — no catalog data is ever read, only
/// DDL is exercised, so the target database need not hold anything egret
/// cares about.
fn jdbc_def(database: &str) -> (CatalogDef, Uuid) {
    let password_secret_id = Uuid::new_v4();
    let def = CatalogDef::parse(
        "jdbc",
        &json!({
            "dialect": "postgresql",
            "host": "postgres",
            "port": 5432,
            "database": database,
            "user": "egret",
            "password_secret_id": password_secret_id,
            "ssl": { "mode": "disable" },
        }),
        &json!(null),
    )
    .unwrap();
    (def, password_secret_id)
}

#[tokio::test]
async fn create_show_alter_and_drop_a_real_catalog() {
    let Some(fe) = fe() else {
        eprintln!("skip");
        return;
    };
    let runner = MysqlDdlRunner;
    let name = format!("egret_sp3b_{}", &Uuid::new_v4().simple().to_string()[..8]);
    let (def, pw_id) = jdbc_def("egret");
    let ddl = render_starrocks(&name, &def, &StarRocksRenderInput::probe()).unwrap();
    let mut secrets = BTreeMap::new();
    secrets.insert(pw_id, "egret".to_string());

    runner
        .execute(&fe, &ddl.create_sql(&secrets).unwrap())
        .await
        .unwrap();
    assert!(show_catalogs(&runner, &fe).await.unwrap().contains(&name));
    let props = show_create_catalog(&runner, &fe, &name)
        .await
        .unwrap()
        .expect("present");
    assert_eq!(props["type"], "jdbc", "props={props:?}");
    // `password` never comes back — CredentialUtil deletes it outright — so
    // the create above must not have failed to actually run.
    assert!(!props.contains_key("password"), "props={props:?}");
    assert_eq!(
        compare_to_fe(&ddl, &props),
        Drift::Matching,
        "props={props:?}"
    );

    // A second CREATE is an error, not a no-op — which is why D3 reads first.
    let err = runner
        .execute(&fe, &ddl.create_sql(&secrets).unwrap())
        .await
        .unwrap_err();
    assert!(err.to_lowercase().contains("already exists"), "err={err}");

    // ALTER really changes a property (here `jdbc_uri`, via `database`), and
    // `type` is not re-sent.
    let (def2, pw_id2) = jdbc_def("postgres");
    let ddl2 = render_starrocks(&name, &def2, &StarRocksRenderInput::probe()).unwrap();
    let mut secrets2 = BTreeMap::new();
    secrets2.insert(pw_id2, "egret".to_string());
    assert!(matches!(compare_to_fe(&ddl2, &props), Drift::Different(_)));
    runner
        .execute(&fe, &ddl2.alter_sql(&secrets2).unwrap())
        .await
        .unwrap();
    let props2 = show_create_catalog(&runner, &fe, &name)
        .await
        .unwrap()
        .expect("present");
    assert!(
        props2["jdbc_uri"].contains("/postgres?") || props2["jdbc_uri"].ends_with("/postgres"),
        "ALTER must have really changed jdbc_uri to the new database: props2={props2:?}"
    );
    assert_eq!(
        compare_to_fe(&ddl2, &props2),
        Drift::Matching,
        "props2={props2:?}"
    );

    // DROP IF EXISTS twice: the second is a success, which is what lets a
    // detach clean up a row whose catalog the FE never had (D7).
    runner.execute(&fe, &ddl.drop_sql()).await.unwrap();
    runner.execute(&fe, &ddl.drop_sql()).await.unwrap();
    assert!(!show_catalogs(&runner, &fe).await.unwrap().contains(&name));
    assert!(show_create_catalog(&runner, &fe, &name)
        .await
        .unwrap()
        .is_none());
}
