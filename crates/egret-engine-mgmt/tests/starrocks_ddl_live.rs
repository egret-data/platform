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
//! (then `starrocks/allin1-ubuntu:latest`, `@@version_comment` = `4.1.4-4a9848e`;
//! the compose now pins `4.1.6`)
//! answers `CREATE EXTERNAL CATALOG ... "type"="benchmark"` with `ERROR 1064
//! (HY000): Getting analyzing error. Detail message: [type : benchmark] is
//! not supported.'`. The vendored FE source this branch's property names were
//! read against (`data-portal/starrocks`, checked out at `main`) added
//! `BenchmarkConnector` on 2026-01-22 (commit `c019a1481e`), but that commit
//! is NOT an ancestor of the running build's revision (`4a9848edf03`,
//! 2026-08-05) and vice versa — divergent history, i.e. the `benchmark`
//! connector lives on `main` only and has never shipped in any released 4.1.x
//! branch build (`can_read` now refuses `tpch`/`tpcds` on StarRocks; see
//! `render/starrocks.rs`'s `tpch_and_tpcds_are_refused_on_starrocks_…` test).
//! `jdbc` is the next-best infra-free target: no extra service beyond what
//! `docker-compose.dev.yml` already runs, and it is fully supported by 4.1.x.
//!
//! Fix round 1 (review F-13): the whole body runs behind `run()`, which never
//! panics (every assertion is a returned `Err(String)`), so the `#[tokio::test]`
//! wrapper can run a `DROP CATALOG IF EXISTS` cleanup unconditionally — a
//! failing run no longer leaves `egret_sp3b_*` behind on the FE for the next
//! one to trip over. Errors are also scrubbed against this run's own secrets
//! map before they're formatted into a panic message (review F-3/F-13): this
//! is the pattern a real caller (Task 4/5, which DOES hold the secrets map at
//! the point it calls `execute`/`query`) is expected to follow before an
//! `FeError::scrubbed` reaches `apply_error` or an API response.

use std::collections::BTreeMap;

use egret_catalog_registry::{escape_ident, render_starrocks, CatalogDef, StarRocksRenderInput};
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

/// The whole exercise, panic-free: every check is a returned `Err(String)`
/// (already scrubbed against `secrets`) so the caller can run cleanup exactly
/// once, regardless of how this ends.
async fn run(fe: &FeTarget, name: &str) -> Result<(), String> {
    let runner = MysqlDdlRunner;
    let (def, pw_id) = jdbc_def("egret");
    let ddl =
        render_starrocks(name, &def, &StarRocksRenderInput::probe()).map_err(|e| e.to_string())?;
    let mut secrets = BTreeMap::new();
    secrets.insert(pw_id, "egret".to_string());
    // Task 4 made this structural: `excerpt` is private and `Display` is the
    // FE code alone, so `scrubbed` is the only way to read the diagnosis — the
    // pattern every production caller now follows by construction. The final
    // review went one step further: `scrubbed` takes the `&FeTarget` too, so
    // the credential this connection logs in with cannot be left out of the
    // map even here, where the map is hand-written.
    let scrub = |e: &egret_engine_mgmt::FeError| e.scrubbed(fe, &secrets);

    runner
        .execute(fe, &ddl.create_sql(&secrets).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| format!("CREATE failed: {}", scrub(&e)))?;
    if !show_catalogs(&runner, fe)
        .await
        .map_err(|e| scrub(&e))?
        .contains(name)
    {
        return Err(format!(
            "{name} missing from SHOW CATALOGS right after CREATE"
        ));
    }
    let props = show_create_catalog(&runner, fe, name)
        .await
        .map_err(|e| scrub(&e))?
        .ok_or_else(|| {
            format!("{name} present in SHOW CATALOGS but SHOW CREATE CATALOG says absent")
        })?;
    if props.get("type").map(String::as_str) != Some("jdbc") {
        return Err(format!("props={props:?}"));
    }
    // `password` never comes back — CredentialUtil deletes it outright — so
    // the create above must not have failed to actually run.
    if props.contains_key("password") {
        return Err(format!(
            "password leaked back from SHOW CREATE CATALOG: props={props:?}"
        ));
    }
    if compare_to_fe(&ddl, &props) != Drift::Matching {
        return Err(format!(
            "freshly-created catalog must compare Matching: props={props:?}"
        ));
    }

    // A second CREATE is an error, not a no-op — which is why D3 reads first.
    match runner
        .execute(fe, &ddl.create_sql(&secrets).map_err(|e| e.to_string())?)
        .await
    {
        // `is_already_exists`, decided inside `fe_error` from the RAW message,
        // and NOT a `contains` on the scrubbed string this used to ask: a
        // withheld or over-capped excerpt would turn a correct FE reply into a
        // named failure here (final whole-branch review, F1).
        Err(e) if e.is_already_exists() => {}
        Err(e) => {
            return Err(format!(
                "second CREATE failed for a different reason: {}",
                scrub(&e)
            ))
        }
        Ok(()) => {
            return Err(
                "second CREATE unexpectedly succeeded — D3 requires an error, not a silent no-op"
                    .into(),
            )
        }
    }

    // ALTER really changes a property (here `jdbc_uri`, via `database`), and
    // `type` is not re-sent.
    let (def2, pw_id2) = jdbc_def("postgres");
    let ddl2 =
        render_starrocks(name, &def2, &StarRocksRenderInput::probe()).map_err(|e| e.to_string())?;
    let mut secrets2 = BTreeMap::new();
    secrets2.insert(pw_id2, "egret".to_string());
    if !matches!(compare_to_fe(&ddl2, &props), Drift::Different(_)) {
        return Err(format!(
            "an edited jdbc_uri must compare Different before the ALTER: props={props:?}"
        ));
    }
    runner
        .execute(fe, &ddl2.alter_sql(&secrets2).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| format!("ALTER failed: {}", scrub(&e)))?;
    let props2 = show_create_catalog(&runner, fe, name)
        .await
        .map_err(|e| scrub(&e))?
        .ok_or_else(|| format!("{name} vanished after ALTER"))?;
    let jdbc_uri = props2.get("jdbc_uri").cloned().unwrap_or_default();
    if !(jdbc_uri.contains("/postgres?") || jdbc_uri.ends_with("/postgres")) {
        return Err(format!(
            "ALTER must have really changed jdbc_uri to the new database: props2={props2:?}"
        ));
    }
    if compare_to_fe(&ddl2, &props2) != Drift::Matching {
        return Err(format!(
            "post-ALTER catalog must compare Matching: props2={props2:?}"
        ));
    }

    // DROP IF EXISTS twice: the second is a success, which is what lets a
    // detach clean up a row whose catalog the FE never had (D7).
    runner
        .execute(fe, &ddl.drop_sql())
        .await
        .map_err(|e| format!("first DROP failed: {}", scrub(&e)))?;
    runner
        .execute(fe, &ddl.drop_sql())
        .await
        .map_err(|e| format!("second (already-gone) DROP failed: {}", scrub(&e)))?;
    if show_catalogs(&runner, fe)
        .await
        .map_err(|e| scrub(&e))?
        .contains(name)
    {
        return Err(format!("{name} still listed after DROP"));
    }
    if show_create_catalog(&runner, fe, name)
        .await
        .map_err(|e| scrub(&e))?
        .is_some()
    {
        return Err(format!("{name} still describable after DROP"));
    }
    Ok(())
}

#[tokio::test]
async fn create_show_alter_and_drop_a_real_catalog() {
    let Some(fe) = fe() else {
        eprintln!("skip");
        return;
    };
    let name = format!("egret_sp3b_{}", &Uuid::new_v4().simple().to_string()[..8]);
    let result = run(&fe, &name).await;
    // Self-healing regardless of outcome (F-13): `run` never panics, so this
    // always executes, even on a failed assertion — no leftover catalog for
    // the next run to trip over.
    let _ = MysqlDdlRunner
        .execute(
            &fe,
            &format!("DROP CATALOG IF EXISTS `{}`", escape_ident(&name)),
        )
        .await;
    result.unwrap();
}
