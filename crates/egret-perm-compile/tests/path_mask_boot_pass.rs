//! fix/ranger-table-mask-column: the boot pass that recompiles every profile
//! still serving a compiled path mask with a missing level — the body the old
//! compiler stored for a whole-table, schema or catalog mask, which masks
//! nothing on Ranger — so it is replaced without waiting for an unrelated
//! write, and that reports loudly when the recompile is refused.
//!
//! Its own binary: the pass is installation-wide over the private database
//! `egret-test-db` hands this process.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test path_mask_boot_pass

use egret_metastore::Db;
use egret_perm::{
    PermStore, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind, Scope, Selector,
};
use egret_perm_compile::{
    recompile_path_mask_profiles, recompile_service, PATH_MASK_REFUSAL_PREFIX,
};
use egret_policy::{PolicyStore, COMPILER_HIDDEN_PREFIX, COMPILER_RULE_PREFIX};
use serde_json::Value;
use uuid::Uuid;

/// The pass is installation-wide: tests that assert on what it did run alone.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// A StarRocks profile bound to one engine with one registry catalog attached.
/// Returns `(service_id, catalog_id)`.
async fn starrocks_profile(db: &Db, policy: &PolicyStore) -> (i64, Uuid) {
    egret_policy::seed_starrocks(policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_pathmask"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");
    let catalog = bind(db, service.id, "starrocks").await;
    (service.id, catalog)
}

/// A Trino profile with a tag service, bound to one engine with one registry
/// catalog attached. Returns `(service_id, catalog_id)`.
async fn trino_profile(db: &Db, policy: &PolicyStore) -> (i64, Uuid) {
    egret_policy::seed_trino(policy).await.expect("seed trino");
    egret_policy::seed_tag(policy).await.expect("seed tag");
    let tag_service = policy
        .create_service(&unique("tag_pathmask"), "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let service = policy
        .create_service(
            &unique("trino_pathmask"),
            "trino",
            &serde_json::json!({ "tagService": tag_service.name }),
        )
        .await
        .expect("create service");
    let catalog = bind(db, service.id, "trino").await;
    (service.id, catalog)
}

async fn bind(db: &Db, service_id: i64, engine_type: &str) -> Uuid {
    let pool = &db.pool;
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name) VALUES ($1,'Path mask') RETURNING id",
    )
    .bind(unique("pathmask-ws"))
    .fetch_one(pool)
    .await
    .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-pathmask-test','AKIATEST') RETURNING id",
    )
    .bind(unique("pathmask-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("pathmask_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,$3,'egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("pathmask-eng"))
    .bind(service_id)
    .bind(engine_type)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("pathmask_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
    catalog_id
}

/// A path mask rule, stored with no recompile.
async fn path_mask(
    perm: &PermStore,
    catalog_id: Uuid,
    table: Option<&str>,
    column: Option<&str>,
    mask_type: &str,
    exempt: &[&str],
) -> (Uuid, String) {
    let name = unique("pathmask");
    let id = perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: name.clone(),
            kind: RuleKind::Mask {
                mask_type: mask_type.to_string(),
                value_expr: None,
            },
            selector: Selector::Path(Scope {
                catalog_id,
                schema: Some("sales".to_string()),
                table: table.map(str::to_string),
                column: column.map(str::to_string),
            }),
            exempt: PrincipalSet {
                include: exempt
                    .iter()
                    .map(|u| Principal {
                        kind: PrincipalKind::User,
                        r#ref: (*u).to_string(),
                    })
                    .collect(),
                exclude: Vec::new(),
            },
        })
        .await
        .expect("rule");
    (id, name)
}

fn compiled_name(rule_id: Uuid) -> String {
    format!("{COMPILER_RULE_PREFIX}{rule_id}")
}

async fn body(db: &Db, service_id: i64, name: &str) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(name)
    .fetch_optional(&db.pool)
    .await
    .expect("policy read")
}

/// Rewrite a stored compiled mask to the shape the OLD compiler stored for a
/// table- or schema-scoped mask: no `column`, and for a schema scope no
/// `table` either.
async fn age_to_old_shape(db: &Db, service_id: i64, name: &str) {
    sqlx::query(
        "UPDATE policy.authz_policies \
            SET policy_jsonb = CASE \
                WHEN policy_jsonb->'resources'->'table'->'values' = '[\"*\"]'::jsonb \
                THEN policy_jsonb #- '{resources,column}' #- '{resources,table}' \
                ELSE policy_jsonb #- '{resources,column}' END \
          WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(name)
    .execute(&db.pool)
    .await
    .expect("age the stored body");
}

async fn policy_version(db: &Db, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(&db.pool)
        .await
        .expect("policy_version")
}

async fn compile_run(db: &Db, service_id: i64) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT outcome, error FROM perm.compile_runs WHERE service_id = $1")
        .bind(service_id)
        .fetch_optional(&db.pool)
        .await
        .expect("compile run")
}

/// A StarRocks table mask and a Trino SCHEMA mask the old compiler stored
/// without their missing levels are rewritten by the boot pass, with no other
/// write: every level is present, `policy_version` moves, and on Trino the
/// schema mask's metadata-table deny (`perm-hidden-meta-…`, which a body with
/// no `table` level never produced) appears. A profile whose only mask names
/// its column is outside the pass. A second boot recompiles nothing.
#[tokio::test]
async fn the_boot_pass_replaces_masks_the_old_compiler_stored_without_their_levels() {
    let Some(db) = test_db().await else { return };
    let _serial = SERIAL.lock().await;
    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());

    let (sr, sr_catalog) = starrocks_profile(&db, &policy).await;
    let (sr_mask, _) = path_mask(&perm, sr_catalog, Some("orders"), None, "MASK_HASH", &[]).await;
    let (trino, trino_catalog) = trino_profile(&db, &policy).await;
    let (trino_mask, _) = path_mask(&perm, trino_catalog, None, None, "MASK_HASH", &[]).await;
    let (control, control_catalog) = starrocks_profile(&db, &policy).await;
    let (column_mask, _) = path_mask(
        &perm,
        control_catalog,
        Some("orders"),
        Some("email"),
        "MASK_HASH",
        &[],
    )
    .await;
    for s in [sr, trino, control] {
        recompile_service(&perm, &policy, s)
            .await
            .expect("recompile");
    }
    let meta_deny = format!("{COMPILER_HIDDEN_PREFIX}meta-{}", compiled_name(trino_mask));
    assert!(
        body(&db, trino, &meta_deny).await.is_some(),
        "today's compiler gives a Trino schema mask a metadata-table deny"
    );

    // The bodies the old compiler stored.
    age_to_old_shape(&db, sr, &compiled_name(sr_mask)).await;
    age_to_old_shape(&db, trino, &compiled_name(trino_mask)).await;
    sqlx::query("DELETE FROM policy.authz_policies WHERE service_id = $1 AND name = $2")
        .bind(trino)
        .bind(&meta_deny)
        .execute(&db.pool)
        .await
        .expect("drop the metadata deny the old body never produced");
    let aged = body(&db, trino, &compiled_name(trino_mask)).await.unwrap();
    assert!(aged["resources"].get("column").is_none() && aged["resources"].get("table").is_none());
    // A control the pass must NOT reach: a stale mask type on a column mask.
    sqlx::query(
        "UPDATE policy.authz_policies SET policy_jsonb = jsonb_set(policy_jsonb, \
         '{dataMaskPolicyItems,0,dataMaskInfo,dataMaskType}', '\"MASK_NULL\"'::jsonb) \
          WHERE service_id = $1 AND name = $2",
    )
    .bind(control)
    .bind(compiled_name(column_mask))
    .execute(&db.pool)
    .await
    .expect("age the control");
    let (sr_before, trino_before, control_before) = (
        policy_version(&db, sr).await,
        policy_version(&db, trino).await,
        policy_version(&db, control).await,
    );

    let run = recompile_path_mask_profiles(&db.pool)
        .await
        .expect("boot pass");
    assert!(
        run.compiled.contains(&sr) && run.compiled.contains(&trino),
        "both stale profiles are recompiled: {run:?}"
    );
    assert!(
        !run.compiled.contains(&control),
        "a profile whose only mask names a column is not reached: {run:?}"
    );

    let sr_body = body(&db, sr, &compiled_name(sr_mask)).await.unwrap();
    assert_eq!(
        sr_body["resources"]["column"]["values"],
        serde_json::json!(["*"])
    );
    let trino_body = body(&db, trino, &compiled_name(trino_mask)).await.unwrap();
    assert_eq!(
        trino_body["resources"]["table"]["values"],
        serde_json::json!(["*"])
    );
    assert_eq!(
        trino_body["resources"]["column"]["values"],
        serde_json::json!(["*"])
    );
    assert!(
        body(&db, trino, &meta_deny).await.is_some(),
        "the repaired Trino schema mask brings its metadata-table deny"
    );
    let (sr_after, trino_after) = (
        policy_version(&db, sr).await,
        policy_version(&db, trino).await,
    );
    assert!(sr_after > sr_before, "StarRocks policy_version moves");
    assert!(trino_after > trino_before, "Trino policy_version moves");
    assert_eq!(policy_version(&db, control).await, control_before);
    assert_eq!(
        body(&db, control, &compiled_name(column_mask))
            .await
            .unwrap()["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"],
        "MASK_NULL",
        "the control's stored body is untouched"
    );

    // The pass selects on the stored artifact, so once applied it retires.
    let second = recompile_path_mask_profiles(&db.pool)
        .await
        .expect("second boot pass");
    assert!(
        second.compiled.is_empty(),
        "a second boot recompiles nothing: {second:?}"
    );
    assert_eq!(policy_version(&db, sr).await, sr_after);
    assert_eq!(policy_version(&db, trino).await, trino_after);
}

/// **The refusal path (review I1).** Two legacy masks at one priority on one
/// table that disagree — R1 on the whole table (stored by the old compiler with
/// no `column`, so it masks nothing) and R2 on `email` — were stored before the
/// overlap guard. Rendering R1 as `column: *` changes the pair's coordinate, so
/// the compile reads it as a pair it CREATES and is refused. The pass must:
/// return the refusal, record it in `perm.compile_runs` with the explanation
/// on its first line and both rule names, leave the old artifact untouched,
/// and still compile another profile in the same run. `GET /perm/overlaps`
/// (`stored_overlaps_via`) lists the pair, so a steward can find it.
#[tokio::test]
async fn a_refused_boot_recompile_is_recorded_reported_and_leaves_the_old_artifact() {
    let Some(db) = test_db().await else { return };
    let _serial = SERIAL.lock().await;
    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());

    let (svc, catalog) = starrocks_profile(&db, &policy).await;
    let (r2, r2_name) = path_mask(
        &perm,
        catalog,
        Some("orders"),
        Some("email"),
        "MASK_HASH",
        &[],
    )
    .await;
    recompile_service(&perm, &policy, svc)
        .await
        .expect("recompile R2 alone");
    // R1 is stored in the model, and its LEGACY compiled body (R2's body with
    // the column dropped and another mask type) is put beside R2's directly,
    // as it would be on an installation that predates the overlap guard.
    let (r1, r1_name) = path_mask(&perm, catalog, Some("orders"), None, "MASK_NULL", &[]).await;
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         SELECT service_id, $3, \
                jsonb_set(jsonb_set(policy_jsonb #- '{resources,column}', '{name}', to_jsonb($3::text)), \
                          '{dataMaskPolicyItems,0,dataMaskInfo,dataMaskType}', '\"MASK_NULL\"'::jsonb), \
                'compiler' \
           FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(svc)
    .bind(compiled_name(r2))
    .bind(compiled_name(r1))
    .execute(&db.pool)
    .await
    .expect("store R1's legacy body");
    let legacy = body(&db, svc, &compiled_name(r1)).await.unwrap();
    assert!(legacy["resources"].get("column").is_none());

    // A second stale profile with no pair: the same run must still compile it.
    let (other, other_catalog) = starrocks_profile(&db, &policy).await;
    let (other_mask, _) =
        path_mask(&perm, other_catalog, Some("orders"), None, "MASK_HASH", &[]).await;
    recompile_service(&perm, &policy, other)
        .await
        .expect("recompile other");
    age_to_old_shape(&db, other, &compiled_name(other_mask)).await;

    // The pair is listed by the read-only overlaps report BEFORE the boot.
    let listed = {
        let mut conn = db.pool.acquire().await.expect("conn");
        egret_perm_compile::overlap::stored_overlaps_via(&mut conn)
            .await
            .expect("stored overlaps")
    };
    assert!(
        listed.iter().any(|o| o.service_id == svc
            && o.rules.iter().any(|m| m.id == Some(r1))
            && o.rules.iter().any(|m| m.id == Some(r2))),
        "GET /perm/overlaps lists the legacy pair: {listed:?}"
    );
    let version_before = policy_version(&db, svc).await;

    let run = recompile_path_mask_profiles(&db.pool)
        .await
        .expect("boot pass");

    let refused: Vec<i64> = run.refused.iter().map(|(id, _)| *id).collect();
    assert_eq!(refused, vec![svc], "the pass returns the refusal: {run:?}");
    assert!(
        run.failed.is_empty(),
        "an overlap is a refusal, not a failure: {run:?}"
    );
    assert!(
        run.compiled.contains(&other) && !run.compiled.contains(&svc),
        "one transaction per profile: the other profile still compiles: {run:?}"
    );
    let (outcome, error) = compile_run(&db, svc).await.expect("a compile run");
    let error = error.expect("an error text");
    assert_eq!(outcome, "error");
    eprintln!("recorded refusal: {error}");
    assert!(
        error.contains("order-ambiguous pair"),
        "the refusal is the overlap guard's: {error}"
    );
    assert!(
        PATH_MASK_REFUSAL_PREFIX.chars().count() < 200,
        "the explanation fits the Overview's 200-character detail line"
    );
    assert!(
        error.starts_with(PATH_MASK_REFUSAL_PREFIX),
        "the Overview's first line explains the consequence: {error}"
    );
    assert!(
        error.contains(&format!("\"{r1_name}\"")) && error.contains(&format!("\"{r2_name}\"")),
        "the recorded refusal names both rules (the Overview links them): {error}"
    );
    assert_eq!(
        body(&db, svc, &compiled_name(r1)).await.unwrap(),
        legacy,
        "the old artifact is untouched"
    );
    assert_eq!(policy_version(&db, svc).await, version_before);

    // Still stale, so a later boot retries — and reports — it again.
    let again = recompile_path_mask_profiles(&db.pool)
        .await
        .expect("second boot pass");
    assert_eq!(
        again.refused.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![svc]
    );
    assert!(
        again.compiled.is_empty(),
        "nothing else is stale: {again:?}"
    );
}
