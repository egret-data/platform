//! View governance R5, Task 7: what the compiler gives `egret-lineage`, asked
//! of the evaluator over the download the plugins receive (`artifact_via`).
//!
//! The fixture is one Trino profile with a tag service, one egret catalog
//! attached to an engine bound to the profile, and on `sales.orders.ssn`: a
//! clearance nobody holds (a tag deny for everyone), a `MASK_NULL` mask, a
//! `1 = 0` row filter on the table, and a deny grant to `public`. A control
//! user `alice`, granted `read` on the catalog, is denied, masked and filtered
//! by the same download; `egret-lineage` must be allowed, unmasked and
//! unfiltered, and must never be allowed a write.
//!
//! Plus the lineage store and `view_tag_set` against the same kind of fixture.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use std::collections::BTreeSet;

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Rule,
    RuleKind, Scope, Selector,
};
use egret_policy::PolicyStore;
use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, EngineConfig,
    MaskDecision, PluginVersion, RowFilterDecision,
};
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;
use uuid::Uuid;

use crate::lineage::{self, LineageTable, ViewRef, LINEAGE_PRINCIPAL};
use crate::recompile_service;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    perm: PermStore,
    policy: PolicyStore,
    profile_id: i64,
    catalog_id: Uuid,
    catalog_name: String,
    warehouse_id: Uuid,
}

async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_lin");
    let tag_service_name = format!("{profile_name}_tag");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("profile");
    policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");

    let ws_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name) VALUES ($1,'Lineage WS') RETURNING id",
    )
    .bind(unique("lin-ws"))
    .fetch_one(pool)
    .await
    .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-lin-test','AKIATEST') RETURNING id",
    )
    .bind(unique("lin-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("lin_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("lin-eng"))
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("lin_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog_name)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");

    Fixture {
        perm,
        policy,
        profile_id: profile.id,
        catalog_id,
        catalog_name,
        warehouse_id,
    }
}

async fn new_tag(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("lin-tag"))
        .bind(unique("LinTag"))
        .fetch_one(&db.pool)
        .await
        .expect("tag")
}

async fn assign(db: &Db, tag: Uuid, warehouse_id: Uuid, table: &str, column: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales',$3,$4)",
    )
    .bind(tag)
    .bind(warehouse_id)
    .bind(table)
    .bind(column)
    .execute(&db.pool)
    .await
    .expect("assign");
}

async fn gate(db: &Db, tag: Uuid, roles: &[String]) {
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(tag)
    .bind(roles)
    .execute(&db.pool)
    .await
    .expect("gate");
}

fn user(r: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: r.to_string(),
    }
}

fn scope(f: &Fixture, table: Option<&str>, column: Option<&str>) -> Scope {
    Scope {
        catalog_id: f.catalog_id,
        schema: Some("sales".into()),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    }
}

/// Seed the classified, masked, filtered and denied `sales.orders.ssn`, then
/// recompile.
async fn govern_ssn(db: &Db, f: &Fixture) {
    let tag = new_tag(db).await;
    assign(db, tag, f.warehouse_id, "orders", "ssn").await;
    // A role nobody holds: the clearance deny stands for everyone.
    gate(db, tag, &[unique("lin-nobody-role")]).await;

    f.perm
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Allow,
            actions: vec![Action::Read],
            scope: Scope {
                catalog_id: f.catalog_id,
                schema: None,
                table: None,
                column: None,
            },
            principals: PrincipalSet {
                include: vec![user("alice")],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("allow grant");
    f.perm
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Deny,
            actions: vec![Action::Read, Action::Write],
            scope: scope(f, Some("orders"), Some("ssn")),
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::Group,
                    r#ref: "public".into(),
                }],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("deny grant");
    f.perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("lin-mask"),
            kind: RuleKind::Mask {
                mask_type: "MASK_NULL".into(),
                value_expr: None,
            },
            selector: Selector::Path(scope(f, Some("orders"), Some("ssn"))),
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("mask rule");
    f.perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("lin-filter"),
            kind: RuleKind::RowFilter {
                filter_expr: "1 = 0".into(),
            },
            selector: Selector::Path(scope(f, Some("orders"), None)),
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("filter rule");

    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");
}

struct Verdicts {
    select: Decision,
    use_catalog: Decision,
    writes: Vec<(String, Decision)>,
    mask: MaskDecision,
    filter: RowFilterDecision,
}

async fn verdicts(db: &Db, f: &Fixture, who: &str, plugin: PluginVersion) -> Verdicts {
    let mut conn = db.pool.acquire().await.expect("conn");
    let svc = egret_policy::get_service_by_id_via(&mut conn, f.profile_id)
        .await
        .expect("service");
    let artifact = crate::routes::artifact_via(&mut conn, &svc)
        .await
        .expect("artifact");
    let def = ServiceDef::parse(&egret_policy::trino_service_def().expect("def"));
    let config = EngineConfig {
        engine: Some(egret_ranger_eval::eval::EngineType::Trino),
        plugin_version: Some(plugin),
        ..EngineConfig::without_super_users()
    };
    let column = scope(f, Some("orders"), Some("ssn"));
    let tags = crate::routes::resolved_tags_for_target(&mut *conn, &column)
        .await
        .expect("tag lookup")
        .expect("tags resolve");
    let col_resource = RequestResource::new()
        .with("catalog", f.catalog_name.clone())
        .with("schema", "sales")
        .with("table", "orders")
        .with("column", "ssn");
    let table_resource = RequestResource::new()
        .with("catalog", f.catalog_name.clone())
        .with("schema", "sales")
        .with("table", "orders");
    let req = |access: &str, resource: RequestResource, with_tags: bool| AccessRequest {
        user: who.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: access.to_string(),
        resource,
        owner: None,
        resolved_tags: Some(if with_tags {
            tags.clone()
        } else {
            egret_ranger_eval::eval::UnenrichedTags::new(Vec::new())
        }),
    };
    let catalog_resource = RequestResource::new().with("catalog", f.catalog_name.clone());
    Verdicts {
        select: evaluate(
            &artifact,
            &req("select", col_resource.clone(), true),
            &def,
            &config,
        ),
        use_catalog: evaluate(
            &artifact,
            &req("use", catalog_resource, false),
            &def,
            &config,
        ),
        writes: ["insert", "delete", "create", "drop", "alter", "all"]
            .into_iter()
            .map(|a| {
                (
                    a.to_string(),
                    evaluate(
                        &artifact,
                        &req(a, col_resource.clone(), true),
                        &def,
                        &config,
                    ),
                )
            })
            .collect(),
        mask: evaluate_data_mask(&artifact, &req("select", col_resource, true), &def, &config),
        filter: evaluate_row_filter(
            &artifact,
            &req("select", table_resource, false),
            &def,
            &config,
        ),
    }
}

fn is_unmasked(m: &MaskDecision) -> bool {
    match m {
        MaskDecision::NoMask => true,
        MaskDecision::Masked(r) => r.mask_type.as_deref() == Some("MASK_NONE"),
        MaskDecision::Undecidable { .. } => false,
    }
}

fn is_unfiltered(r: &RowFilterDecision) -> bool {
    match r {
        RowFilterDecision::NoFilter => true,
        RowFilterDecision::Filtered(f) => f.filter_expr.trim().is_empty(),
        RowFilterDecision::Undecidable { .. } => false,
    }
}

#[tokio::test]
async fn the_lineage_principal_reads_a_classified_masked_filtered_column_unchanged() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    govern_ssn(&db, &f).await;

    for plugin in [PluginVersion::Ranger2_5_0, PluginVersion::Ranger2_8_0] {
        // The control: the same download denies, masks and filters a user the
        // compiler does not except, so the fixture bites.
        let alice = verdicts(&db, &f, "alice", plugin).await;
        assert!(
            matches!(alice.select, Decision::Denied { .. }),
            "{plugin:?}: alice must be denied the classified column: {:?}",
            alice.select
        );
        assert!(
            matches!(&alice.mask, MaskDecision::Masked(m) if m.mask_type.as_deref() == Some("MASK_NULL")),
            "{plugin:?}: {:?}",
            alice.mask
        );
        assert!(
            matches!(&alice.filter, RowFilterDecision::Filtered(r) if r.filter_expr == "1 = 0"),
            "{plugin:?}: {:?}",
            alice.filter
        );

        let lineage = verdicts(&db, &f, LINEAGE_PRINCIPAL, plugin).await;
        assert!(
            matches!(lineage.select, Decision::Allowed { .. }),
            "{plugin:?}: egret-lineage must be allowed `select`: {:?}",
            lineage.select
        );
        assert!(
            matches!(lineage.use_catalog, Decision::Allowed { .. }),
            "{plugin:?}: egret-lineage must be allowed `use` on the catalog: {:?}",
            lineage.use_catalog
        );
        assert!(is_unmasked(&lineage.mask), "{plugin:?}: {:?}", lineage.mask);
        assert!(
            is_unfiltered(&lineage.filter),
            "{plugin:?}: {:?}",
            lineage.filter
        );
        for (access, d) in &lineage.writes {
            assert!(
                !matches!(d, Decision::Allowed { .. }),
                "{plugin:?}: egret-lineage must never be allowed `{access}`: {d:?}"
            );
        }
    }
}

/// No compiled body ever names `egret-lineage` beside an access type other
/// than `select` or `use`.
#[tokio::test]
async fn no_compiled_policy_gives_the_lineage_principal_a_non_read_access() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    govern_ssn(&db, &f).await;

    let bodies: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT p.policy_jsonb FROM policy.authz_policies p \
          WHERE p.service_id IN (SELECT id FROM policy.authz_services WHERE name LIKE $1 || '%')",
    )
    .bind(
        egret_policy::get_service_by_id_via(&mut db.pool.acquire().await.unwrap(), f.profile_id)
            .await
            .unwrap()
            .name,
    )
    .fetch_all(&db.pool)
    .await
    .expect("bodies");
    let mut seen = 0;
    for body in &bodies {
        for bucket in [
            "policyItems",
            "denyPolicyItems",
            "allowExceptions",
            "denyExceptions",
        ] {
            for item in body[bucket].as_array().into_iter().flatten() {
                let names_it = item["users"]
                    .as_array()
                    .is_some_and(|u| u.iter().any(|u| u == LINEAGE_PRINCIPAL));
                if !names_it {
                    continue;
                }
                seen += 1;
                for a in item["accesses"].as_array().expect("accesses") {
                    let t = a["type"].as_str().expect("type");
                    assert!(
                        lineage::LINEAGE_ACCESSES.contains(&t),
                        "`{t}` for egret-lineage in {bucket} of {}",
                        body["name"]
                    );
                }
            }
        }
    }
    // The allow, the clearance exception and the deny-grant exception.
    assert!(seen >= 3, "expected the lineage items, saw {seen}");
}

#[tokio::test]
async fn view_tag_set_is_the_union_of_clearance_bearing_tags_over_the_lineage() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pii = new_tag(&db).await;
    let fin = new_tag(&db).await;
    let table_level = new_tag(&db).await;
    let informational = new_tag(&db).await;
    let elsewhere = new_tag(&db).await;
    assign(&db, pii, f.warehouse_id, "pii", "ssn").await;
    assign(&db, fin, f.warehouse_id, "orders", "amount").await;
    assign(&db, table_level, f.warehouse_id, "orders", "").await;
    assign(&db, informational, f.warehouse_id, "pii", "name").await;
    assign(&db, elsewhere, f.warehouse_id, "unread", "x").await;
    for t in [pii, fin, table_level, elsewhere] {
        gate(&db, t, &["lin-role".to_string()]).await;
    }
    // A row with both lists empty is informational: no clearance.
    gate(&db, informational, &[]).await;

    let tables: BTreeSet<LineageTable> = [
        (f.catalog_name.clone(), "sales".into(), "pii".into()),
        // Case-folded like the tag resolver.
        (f.catalog_name.clone(), "SALES".into(), "Orders".into()),
    ]
    .into();
    let mut conn = db.pool.acquire().await.unwrap();
    let got = lineage::view_tag_set(&mut conn, &tables)
        .await
        .expect("tags");
    assert_eq!(got, BTreeSet::from([pii, fin, table_level]));

    // A lineage that names a catalog egret does not know carries no tag.
    let unknown: BTreeSet<LineageTable> =
        [("system".into(), "runtime".into(), "nodes".into())].into();
    assert!(lineage::view_tag_set(&mut conn, &unknown)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_lineage_store_replaces_records_failure_and_deletes() {
    let Some(db) = test_db().await else { return };
    let mut conn = db.pool.acquire().await.unwrap();
    let view = ViewRef {
        catalog: unique("lin_vcat"),
        schema: "s".into(),
        name: "v".into(),
    };
    let t = |n: &str| -> LineageTable { ("ice".into(), "s".into(), n.into()) };

    lineage::write_lineage(&mut conn, &view, &BTreeSet::from([t("pii"), t("orders")]))
        .await
        .unwrap();
    lineage::write_lineage(&mut conn, &view, &BTreeSet::from([t("orders")]))
        .await
        .unwrap();
    assert_eq!(
        lineage::recorded_lineage(&mut conn, &view).await.unwrap(),
        BTreeSet::from([t("orders")])
    );
    assert_eq!(
        state(&mut conn, &view).await,
        Some(("computed".to_string(), None))
    );

    lineage::record_lineage_failure(&mut conn, &view, "boom")
        .await
        .unwrap();
    assert!(lineage::recorded_lineage(&mut conn, &view)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        state(&mut conn, &view).await,
        Some(("failed".to_string(), Some("boom".to_string())))
    );

    lineage::delete_lineage(&mut conn, &view).await.unwrap();
    assert_eq!(state(&mut conn, &view).await, None);
}

async fn state(conn: &mut sqlx::PgConnection, v: &ViewRef) -> Option<(String, Option<String>)> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, detail FROM view_lineage_state \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&v.catalog)
    .bind(&v.schema)
    .bind(&v.name)
    .fetch_optional(conn)
    .await
    .unwrap()
}
