//! Trino's Iceberg metadata tables (`orders$files`, `orders$partitions`, …)
//! escape a mask, a row filter and a tag mask on `orders`: Trino asks Ranger
//! about `orders$files` by that name, which a `table: *` grant matches and a
//! policy on `orders` does not (measured live on Trino 483: a masked user read
//! the masked column's bounds from `orders$files`, a filtered user the bounds
//! and partition values of the rows filtered away). The compiler adds a deny
//! on `orders$*` for the people each restricting policy restricts.
//!
//! Through `recompile_service`, the driver every write surface funnels into.
//! DB-gated:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-perm-compile --test metadata_tables

use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Scope, Selector};
use egret_perm_compile::recompile_service;
use egret_policy::PolicyStore;
use serde_json::{json, Value};
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

struct Fixture {
    perm: PermStore,
    policy: PolicyStore,
    service_id: i64,
    service_name: String,
    tag_service_id: i64,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    catalog_name: String,
}

/// A Trino profile with a real tag service, one Trino engine bound to it and
/// one registry catalog attached to that engine.
async fn trino_fixture(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let service_name = unique("tr_meta");
    let tag_name = format!("{service_name}_tag");
    let tag_service = policy
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .expect("tag service");
    let service = policy
        .create_service(&service_name, "trino", &json!({ "tagService": tag_name }))
        .await
        .expect("trino profile");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("meta-ws"))
            .bind("Meta WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-meta-test','AKIATEST') RETURNING id",
    )
    .bind(unique("meta-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("meta_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("meta-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("meta_cat");
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
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    Fixture {
        perm,
        policy,
        service_id: service.id,
        service_name,
        tag_service_id: tag_service.id,
        warehouse_id,
        catalog_id,
        catalog_name,
    }
}

async fn policy(pool: &sqlx::PgPool, service_id: i64, name: &str) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(name)
    .fetch_optional(pool)
    .await
    .expect("read policy")
}

fn path_rule(f: &Fixture, kind: RuleKind, column: Option<&str>) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: unique("meta-rule"),
        kind,
        selector: Selector::Path(Scope {
            catalog_id: f.catalog_id,
            schema: Some("sales".to_string()),
            table: Some("orders".to_string()),
            column: column.map(str::to_string),
        }),
        exempt: PrincipalSet::default(),
    }
}

/// The deny on `orders$*`: every column, `select`, `public` denied (the rule
/// exempts nobody), filed under the profile.
fn assert_metadata_deny(f: &Fixture, deny: &Value, table: &str) {
    assert_eq!(deny["service"], json!(f.service_name));
    assert_eq!(deny["serviceType"], "trino");
    assert_eq!(deny["policyType"], 0);
    assert_eq!(
        deny["resources"]["catalog"]["values"],
        json!([f.catalog_name])
    );
    assert_eq!(deny["resources"]["schema"]["values"], json!(["sales"]));
    assert_eq!(deny["resources"]["table"]["values"], json!([table]));
    assert_eq!(deny["resources"]["column"]["values"], json!(["*"]));
    assert_eq!(deny["denyPolicyItems"][0]["groups"], json!(["public"]));
    assert_eq!(
        deny["denyPolicyItems"][0]["accesses"],
        json!([{"type": "select", "isAllowed": true}])
    );
    assert_eq!(deny["policyItems"], json!([]));
}

#[tokio::test]
async fn a_path_mask_and_row_filter_deny_the_tables_metadata_tables() {
    let Some(db) = test_db().await else { return };
    let f = trino_fixture(&db).await;
    let mask = f
        .perm
        .create_rule(&path_rule(
            &f,
            RuleKind::Mask {
                mask_type: "MASK_NULL".to_string(),
                value_expr: None,
            },
            Some("email"),
        ))
        .await
        .expect("mask rule");
    let filter = f
        .perm
        .create_rule(&path_rule(
            &f,
            RuleKind::RowFilter {
                filter_expr: "region = 'us'".to_string(),
            },
            None,
        ))
        .await
        .expect("filter rule");

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    for rule in [mask, filter] {
        let deny = policy(
            &db.pool,
            f.service_id,
            &format!("perm-hidden-meta-perm-rule-{rule}"),
        )
        .await
        .unwrap_or_else(|| panic!("no metadata-table deny for rule {rule}"));
        assert_metadata_deny(&f, &deny, "orders$*");
    }

    // Deleting the rule reaps its deny on the next recompile.
    f.perm.delete_rule(mask).await.expect("delete mask rule");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile after delete");
    assert!(policy(
        &db.pool,
        f.service_id,
        &format!("perm-hidden-meta-perm-rule-{mask}")
    )
    .await
    .is_none());
}

#[tokio::test]
async fn a_tag_mask_denies_the_metadata_tables_of_each_tagged_table() {
    let Some(db) = test_db().await else { return };
    let f = trino_fixture(&db).await;
    let tag_key = unique("pii");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&tag_key)
        .execute(&db.pool)
        .await
        .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         SELECT id, $2, 'sales', 'people', 'ssn' FROM tags WHERE key = $1",
    )
    .bind(&tag_key)
    .bind(f.warehouse_id)
    .execute(&db.pool)
    .await
    .expect("tag column");
    let rule = f
        .perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("pii-null"),
            kind: RuleKind::Mask {
                mask_type: "MASK_NULL".to_string(),
                value_expr: None,
            },
            selector: Selector::Tag {
                key: tag_key.clone(),
            },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("tag rule");

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    // The tag mask itself stays in the tag service ...
    assert!(
        policy(&db.pool, f.tag_service_id, &format!("perm-rule-{rule}"))
            .await
            .is_some()
    );
    // ... and the deny on the tagged table's metadata tables is in the profile.
    let name = format!(
        "perm-hidden-meta-perm-rule-{rule}-{}.sales.people",
        f.catalog_name
    );
    let deny = policy(&db.pool, f.service_id, &name)
        .await
        .unwrap_or_else(|| panic!("no metadata-table deny {name}"));
    assert_metadata_deny(&f, &deny, "people$*");
}

/// Governance sub-project 1, Task 4: `ir_load::load_ep` reads the enforcement
/// point exactly as `compile_desired` uses it — the profile's engine, its
/// classification channel (a `tagService` that names a real service), its
/// catalog scope and its sweep set (the profile plus its tag service).
#[cfg(feature = "harness")]
#[tokio::test]
async fn the_loader_reads_the_ep_as_the_compiler_does() {
    let Some(db) = test_db().await else { return };
    let f = trino_fixture(&db).await;
    let mut conn = db.pool.acquire().await.unwrap();
    let l = egret_perm_compile::harness_support::load_ep_for_test(&mut conn, f.service_id)
        .await
        .unwrap()
        .expect("a trino profile");
    assert_eq!(l.ep.kind.engine, egret_perm::EngineType::Trino);
    assert!(l.ep.classification_channel);
    assert!(l.ep.catalogs.contains_key(&f.catalog_id));
    assert_eq!(l.sweep.len(), 2, "profile plus tag service");
}

/// Governance SP2b Task 5: `load_ep` reads the profile's mechanism (migration
/// 0213) into `EpInput.kind.mechanism`, rather than naming Ranger itself.
#[cfg(feature = "harness")]
#[tokio::test]
async fn the_loader_reads_the_profiles_mechanism() {
    use egret_perm_ir::ep::Mechanism;
    let Some(db) = test_db().await else { return };
    let f = trino_fixture(&db).await;
    let mut conn = db.pool.acquire().await.unwrap();
    assert_eq!(
        loaded_mechanism(&mut conn, f.service_id).await,
        Mechanism::Ranger,
        "the default"
    );
    sqlx::query("UPDATE policy.authz_services SET mechanism = 'opa' WHERE id = $1")
        .bind(f.service_id)
        .execute(&mut *conn)
        .await
        .expect("switch to opa");
    assert_eq!(
        loaded_mechanism(&mut conn, f.service_id).await,
        Mechanism::Opa,
        "an opa profile loads as Mechanism::Opa"
    );
}

#[cfg(feature = "harness")]
async fn loaded_mechanism(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> egret_perm_ir::ep::Mechanism {
    egret_perm_compile::harness_support::load_ep_for_test(conn, service_id)
        .await
        .unwrap()
        .expect("a trino profile")
        .ep
        .kind
        .mechanism
}

/// SP2b Task 26 follow-up: the loader reads only the model rows the profile
/// can lower — the grants and path rules on the catalogs it reaches, and every
/// tag rule — not every grant and rule in the installation. Two profiles, each
/// with its own catalog; each one's read holds its own catalog's grant and
/// path rule, not the other's, and both hold the tag rule.
#[cfg(feature = "harness")]
#[tokio::test]
async fn the_loader_reads_only_the_grants_and_rules_its_catalogs_reach() {
    use egret_perm::{Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind};
    let Some(db) = test_db().await else { return };
    let a = trino_fixture(&db).await;
    let b = trino_fixture(&db).await;
    let public = PrincipalSet {
        include: vec![Principal {
            kind: PrincipalKind::Group,
            r#ref: "public".to_string(),
        }],
        exclude: vec![],
    };
    let grant_on = |f: &Fixture| Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: f.catalog_id,
            schema: Some("sales".to_string()),
            table: None,
            column: None,
        },
        principals: public.clone(),
        origin: GrantOrigin::Explicit,
    };
    let filter = RuleKind::RowFilter {
        filter_expr: "region = 'us'".to_string(),
    };
    let grant_a = a.perm.create_grant(&grant_on(&a)).await.expect("grant a");
    let grant_b = b.perm.create_grant(&grant_on(&b)).await.expect("grant b");
    let rule_a = a
        .perm
        .create_rule(&path_rule(&a, filter.clone(), None))
        .await
        .expect("rule a");
    let rule_b = b
        .perm
        .create_rule(&path_rule(&b, filter, None))
        .await
        .expect("rule b");
    let tag_key = unique("scope_tag");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&tag_key)
        .execute(&db.pool)
        .await
        .expect("tag");
    let tag_rule = a
        .perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("scope-tag-rule"),
            kind: RuleKind::Mask {
                mask_type: "MASK_NULL".to_string(),
                value_expr: None,
            },
            selector: Selector::Tag { key: tag_key },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("tag rule");

    let mut conn = db.pool.acquire().await.unwrap();
    for (f, own, other) in [
        (&a, (grant_a, rule_a), (grant_b, rule_b)),
        (&b, (grant_b, rule_b), (grant_a, rule_a)),
    ] {
        let (grants, rules) = egret_perm_compile::harness_support::load_ep_model_ids_for_test(
            &mut conn,
            f.service_id,
        )
        .await
        .unwrap()
        .expect("a trino profile");
        assert!(grants.contains(&own.0), "its own catalog's grant");
        assert!(rules.contains(&own.1), "its own catalog's path rule");
        assert!(rules.contains(&tag_rule), "every tag rule");
        assert!(!grants.contains(&other.0), "not the other catalog's grant");
        assert!(
            !rules.contains(&other.1),
            "not the other catalog's path rule"
        );
    }
}
