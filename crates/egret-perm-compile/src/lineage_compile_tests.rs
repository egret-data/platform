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

use std::collections::{BTreeMap, BTreeSet};

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

use crate::lineage::{
    self, LineageError, LineageTable, RecordedLineage, ViewRef, LINEAGE_PRINCIPAL,
};
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
    engine_id: Uuid,
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
         VALUES ($1,'trino','egret', \
                 '{\"trinoConfig\": {\"delegationMode\": \"jwt\"}}'::jsonb,$2) RETURNING id",
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
        engine_id,
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

fn scope(f: &Fixture, table: Option<&str>, column: Option<&str>) -> Scope {
    Scope {
        catalog_id: f.catalog_id,
        schema: Some("sales".into()),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    }
}

fn group(r: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Group,
        r#ref: r.to_string(),
    }
}

async fn tag_key_of(db: &Db, tag: Uuid) -> String {
    sqlx::query_scalar("SELECT key FROM tags WHERE id = $1")
        .bind(tag)
        .fetch_one(&db.pool)
        .await
        .expect("tag key")
}

/// A hand-written (operator) policy stored by raw SQL, as a profile the
/// compiler later claimed can still hold.
async fn hand_written(db: &Db, f: &Fixture, name: &str, body: serde_json::Value) {
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator')",
    )
    .bind(f.profile_id)
    .bind(name)
    .bind(body)
    .execute(&db.pool)
    .await
    .expect("hand-written policy");
}

fn lvl(v: &str) -> serde_json::Value {
    serde_json::json!({"values": [v], "isExcludes": false, "isRecursive": false})
}

/// The governed fixture, stored with NOTHING about `egret-lineage`:
///
/// - `sales.orders.ssn`: a clearance nobody holds (a tag deny for everyone), a
///   compiled `MASK_NULL` path mask, a compiled deny grant of `read` to
///   `public`;
/// - `sales.orders`: a compiled `1 = 0` path row filter;
/// - `sales.orders.email`: a compiled TAG mask (`MASK_HASH`);
/// - `sales.customers`: a compiled TAG row filter (`1 = 0`);
/// - `sales.orders.notes`: a HAND-WRITTEN `MASK_NULL` mask;
/// - `sales.items`: a HAND-WRITTEN `1 = 0` row filter;
/// - the whole catalog: a compiled ALLOW of `read` and `write` to `public`
///   (review I2: an inherited write allow), and no write deny anywhere.
async fn govern(db: &Db, f: &Fixture) {
    let clearance = new_tag(db).await;
    assign(db, clearance, f.warehouse_id, "orders", "ssn").await;
    gate(db, clearance, &[unique("lin-nobody-role")]).await;
    let mask_tag = new_tag(db).await;
    assign(db, mask_tag, f.warehouse_id, "orders", "email").await;
    let filter_tag = new_tag(db).await;
    assign(db, filter_tag, f.warehouse_id, "customers", "").await;

    f.perm
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Allow,
            actions: vec![Action::Read, Action::Write],
            scope: Scope {
                catalog_id: f.catalog_id,
                schema: None,
                table: None,
                column: None,
            },
            principals: PrincipalSet {
                include: vec![group("public")],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("public read+write allow");
    f.perm
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Deny,
            actions: vec![Action::Read],
            scope: scope(f, Some("orders"), Some("ssn")),
            principals: PrincipalSet {
                include: vec![group("public")],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("deny grant");
    let rules = [
        (
            RuleKind::Mask {
                mask_type: "MASK_NULL".into(),
                value_expr: None,
            },
            Selector::Path(scope(f, Some("orders"), Some("ssn"))),
        ),
        (
            RuleKind::RowFilter {
                filter_expr: "1 = 0".into(),
            },
            Selector::Path(scope(f, Some("orders"), None)),
        ),
        (
            RuleKind::Mask {
                mask_type: "MASK_HASH".into(),
                value_expr: None,
            },
            Selector::Tag {
                key: tag_key_of(db, mask_tag).await,
            },
        ),
        (
            RuleKind::RowFilter {
                filter_expr: "1 = 0".into(),
            },
            Selector::Tag {
                key: tag_key_of(db, filter_tag).await,
            },
        ),
    ];
    for (kind, selector) in rules {
        f.perm
            .create_rule(&Rule {
                id: Uuid::nil(),
                name: unique("lin-rule"),
                kind,
                selector,
                exempt: PrincipalSet::default(),
            })
            .await
            .expect("rule");
    }
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    hand_written(
        db,
        f,
        "hand mask notes",
        serde_json::json!({
            "name": "hand mask notes", "policyType": 1, "isEnabled": true,
            "resources": {"catalog": lvl(&f.catalog_name), "schema": lvl("sales"),
                          "table": lvl("orders"), "column": lvl("notes")},
            "dataMaskPolicyItems": [{"users": [], "groups": ["public"], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": "MASK_NULL"}}],
            "policyItems": [], "denyPolicyItems": []
        }),
    )
    .await;
    hand_written(
        db,
        f,
        "hand filter items",
        serde_json::json!({
            "name": "hand filter items", "policyType": 2, "isEnabled": true,
            "resources": {"catalog": lvl(&f.catalog_name), "schema": lvl("sales"),
                          "table": lvl("items")},
            "rowFilterPolicyItems": [{"users": [], "groups": ["public"], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
                "rowFilterInfo": {"filterExpr": "1 = 0"}}],
            "policyItems": [], "denyPolicyItems": []
        }),
    )
    .await;
}

struct Asker {
    artifact: egret_ranger_eval::model::ServicePolicySet,
    def: ServiceDef,
    config: EngineConfig,
    catalog: String,
    tags: BTreeMap<(String, String), egret_ranger_eval::eval::UnenrichedTags>,
}

impl Asker {
    async fn new(db: &Db, f: &Fixture, plugin: PluginVersion) -> Self {
        let mut conn = db.pool.acquire().await.expect("conn");
        let svc = egret_policy::get_service_by_id_via(&mut conn, f.profile_id)
            .await
            .expect("service");
        let artifact = crate::routes::served_artifact_via(&mut conn, &svc)
            .await
            .expect("artifact");
        let mut tags = BTreeMap::new();
        for (table, column) in [
            ("orders", "ssn"),
            ("orders", "email"),
            ("orders", "notes"),
            ("orders", ""),
            ("customers", ""),
            ("items", ""),
        ] {
            let target = scope(f, Some(table), (!column.is_empty()).then_some(column));
            let t = crate::routes::resolved_tags_for_target(&mut *conn, &target)
                .await
                .expect("tag lookup")
                .expect("tags resolve");
            tags.insert((table.to_string(), column.to_string()), t);
        }
        Asker {
            artifact,
            def: ServiceDef::parse(&egret_policy::trino_service_def().expect("def")),
            config: EngineConfig {
                engine: Some(egret_ranger_eval::eval::EngineType::Trino),
                plugin_version: Some(plugin),
                ..EngineConfig::without_super_users()
            },
            catalog: f.catalog_name.clone(),
            tags,
        }
    }

    fn request(
        &self,
        who: &str,
        access: &str,
        table: Option<&str>,
        column: Option<&str>,
    ) -> AccessRequest {
        let mut resource = RequestResource::new().with("catalog", self.catalog.clone());
        let mut key = (String::new(), String::new());
        if let Some(t) = table {
            resource = resource.with("schema", "sales").with("table", t);
            key.0 = t.to_string();
        }
        if let Some(c) = column {
            resource = resource.with("column", c);
            key.1 = c.to_string();
        }
        AccessRequest {
            user: who.to_string(),
            groups: BTreeSet::new(),
            roles: BTreeSet::new(),
            access_type: access.to_string(),
            resource,
            owner: None,
            resolved_tags: Some(
                self.tags
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| egret_ranger_eval::eval::UnenrichedTags::new(Vec::new())),
            ),
        }
    }

    fn access(
        &self,
        who: &str,
        access: &str,
        table: Option<&str>,
        column: Option<&str>,
    ) -> Decision {
        evaluate(
            &self.artifact,
            &self.request(who, access, table, column),
            &self.def,
            &self.config,
        )
    }

    fn mask(&self, who: &str, table: &str, column: &str) -> MaskDecision {
        evaluate_data_mask(
            &self.artifact,
            &self.request(who, "select", Some(table), Some(column)),
            &self.def,
            &self.config,
        )
    }

    fn filter(&self, who: &str, table: &str) -> RowFilterDecision {
        evaluate_row_filter(
            &self.artifact,
            &self.request(who, "select", Some(table), None),
            &self.def,
            &self.config,
        )
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

const WRITES: [&str; 8] = [
    "insert", "delete", "create", "drop", "alter", "grant", "revoke", "show",
];

const PLUGINS: [PluginVersion; 2] = [PluginVersion::Ranger2_5_0, PluginVersion::Ranger2_8_0];

/// The download a provisioned JWT-mode Trino engine receives lets
/// `egret-lineage` read every governed column unchanged: through the
/// clearance, the deny grant, compiled path and tag masks and row filters, and
/// hand-written ones (review I1). The same download denies, masks and filters
/// `alice`, so the fixture bites.
#[tokio::test]
async fn the_lineage_principal_reads_classified_masked_filtered_data_unchanged() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    govern(&db, &f).await;

    for plugin in PLUGINS {
        let a = Asker::new(&db, &f, plugin).await;
        // The control.
        assert!(
            matches!(
                a.access("alice", "select", Some("orders"), Some("ssn")),
                Decision::Denied { .. }
            ),
            "{plugin:?}"
        );
        for (table, column, want) in [
            ("orders", "ssn", "MASK_NULL"),
            ("orders", "email", "MASK_HASH"),
            ("orders", "notes", "MASK_NULL"),
        ] {
            assert!(
                matches!(&a.mask("alice", table, column), MaskDecision::Masked(m) if m.mask_type.as_deref() == Some(want)),
                "{plugin:?} alice {table}.{column}: {:?}",
                a.mask("alice", table, column)
            );
        }
        for table in ["orders", "customers", "items"] {
            assert!(
                matches!(&a.filter("alice", table), RowFilterDecision::Filtered(r) if r.filter_expr == "1 = 0"),
                "{plugin:?} alice {table}: {:?}",
                a.filter("alice", table)
            );
        }

        let l = LINEAGE_PRINCIPAL;
        for (table, column) in [("orders", "ssn"), ("orders", "email"), ("orders", "notes")] {
            let d = a.access(l, "select", Some(table), Some(column));
            assert!(
                matches!(d, Decision::Allowed { .. }),
                "{plugin:?} select {table}.{column}: {d:?}"
            );
            let m = a.mask(l, table, column);
            assert!(is_unmasked(&m), "{plugin:?} mask {table}.{column}: {m:?}");
        }
        let d = a.access(l, "use", None, None);
        assert!(
            matches!(d, Decision::Allowed { .. }),
            "{plugin:?} use: {d:?}"
        );
        for table in ["orders", "customers", "items"] {
            let r = a.filter(l, table);
            assert!(is_unfiltered(&r), "{plugin:?} filter {table}: {r:?}");
        }
    }
}

/// Review I2: `{public}` IS allowed write on the catalog and nothing denies a
/// write, so `alice` may write; `egret-lineage` may not, at table or column
/// level, for any non-read access type.
#[tokio::test]
async fn the_lineage_principal_never_writes_even_where_public_may() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    govern(&db, &f).await;

    for plugin in PLUGINS {
        let a = Asker::new(&db, &f, plugin).await;
        for (table, column) in [(Some("customers"), None), (Some("customers"), Some("id"))] {
            let d = a.access("alice", "insert", table, column);
            assert!(
                matches!(d, Decision::Allowed { .. }),
                "{plugin:?}: the control must be allowed to write {table:?}.{column:?}: {d:?}"
            );
            for w in WRITES {
                let d = a.access(LINEAGE_PRINCIPAL, w, table, column);
                assert!(
                    !matches!(d, Decision::Allowed { .. }),
                    "{plugin:?}: egret-lineage must never be allowed `{w}` on {table:?}.{column:?}: {d:?}"
                );
            }
        }
    }
}

/// Review I4: a profile bound to an engine that is not a provisioned JWT-mode
/// Trino engine is served the stored policies, so lineage fails closed there.
/// And nothing about the principal is ever stored.
#[tokio::test]
async fn an_ineligible_engine_is_served_nothing_for_the_lineage_principal() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    govern(&db, &f).await;

    let stored: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT p.policy_jsonb FROM policy.authz_policies p JOIN policy.authz_services s \
             ON s.id = p.service_id \
          WHERE p.service_id = $1 OR s.name = (SELECT config_jsonb->>'tagService' \
                FROM policy.authz_services WHERE id = $1)",
    )
    .bind(f.profile_id)
    .fetch_all(&db.pool)
    .await
    .expect("stored");
    assert!(stored.len() > 4, "{stored:?}");
    assert!(
        !stored
            .iter()
            .any(|b| b.to_string().contains(LINEAGE_PRINCIPAL)),
        "a stored policy names the principal"
    );

    sqlx::query(
        "UPDATE engine_mgmt.engines SET spec = '{\"trinoConfig\": {\"delegationMode\": \"impersonate\"}}' \
          WHERE id = $1",
    )
    .bind(f.engine_id)
    .execute(&db.pool)
    .await
    .expect("impersonate");
    let a = Asker::new(&db, &f, PluginVersion::Ranger2_8_0).await;
    let d = a.access(LINEAGE_PRINCIPAL, "select", Some("customers"), Some("id"));
    // Only the inherited `{public}` allow reaches it now, and it is masked and
    // filtered like anyone: the transform is absent.
    assert!(
        matches!(&a.filter(LINEAGE_PRINCIPAL, "customers"), RowFilterDecision::Filtered(r) if r.filter_expr == "1 = 0"),
        "{d:?}"
    );
    assert!(matches!(
        a.access(LINEAGE_PRINCIPAL, "select", Some("orders"), Some("ssn")),
        Decision::Denied { .. }
    ));
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

    // Review I3: no lineage is never "no tags".
    assert!(matches!(
        lineage::view_tag_set(&mut conn, &BTreeSet::new()).await,
        Err(LineageError::Empty)
    ));
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

    // Review I3: missing is its own answer, and refuses tagging.
    assert_eq!(
        lineage::recorded_lineage(&mut conn, &view).await.unwrap(),
        RecordedLineage::Missing
    );
    assert!(matches!(
        lineage::recorded_view_tag_set(&mut conn, &view).await,
        Err(LineageError::Missing)
    ));
    // An empty lineage is refused and writes nothing.
    assert!(matches!(
        lineage::write_lineage(&mut conn, &view, &BTreeSet::new()).await,
        Err(LineageError::Empty)
    ));
    assert_eq!(state(&mut conn, &view).await, None);

    lineage::write_lineage(&mut conn, &view, &BTreeSet::from([t("pii"), t("orders")]))
        .await
        .unwrap();
    lineage::write_lineage(&mut conn, &view, &BTreeSet::from([t("orders")]))
        .await
        .unwrap();
    assert_eq!(
        lineage::recorded_lineage(&mut conn, &view).await.unwrap(),
        RecordedLineage::Computed(BTreeSet::from([t("orders")]))
    );
    assert_eq!(
        state(&mut conn, &view).await,
        Some(("computed".to_string(), None))
    );
    // A computed lineage over tables with no tag is "no tags", the one case
    // that is.
    assert!(lineage::recorded_view_tag_set(&mut conn, &view)
        .await
        .unwrap()
        .is_empty());

    lineage::record_lineage_failure(&mut conn, &view, "boom")
        .await
        .unwrap();
    assert_eq!(
        lineage::recorded_lineage(&mut conn, &view).await.unwrap(),
        RecordedLineage::Failed {
            detail: "boom".to_string()
        }
    );
    assert!(matches!(
        lineage::recorded_view_tag_set(&mut conn, &view).await,
        Err(LineageError::Failed(d)) if d == "boom"
    ));
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
