//! Governance events for clearances (DG #3, spec §3.2):
//! `ClassificationPolicyRepo::set_policy`/`clear_policy` record
//! `tag_clearance:set`/`:clear` on their own transaction, with group NAMES
//! resolved at write time; a set refused by an overlap records nothing.
//!
//! Tag rules are catalog-unscoped, so the tests here run one at a time
//! ([`SERIAL`]) as `catalog_tag_overlap.rs` does. DB-gated; FAILS without a
//! database unless `EGRET_ALLOW_DB_SKIP` is set.

use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::tags::{ClassificationPolicyRepo, TagRepo};
use egret_events::EventBus;
use egret_metastore::Db;
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    policy: PolicyStore,
    tags: TagRepo,
    repo: CatalogTagRepo,
    policies: ClassificationPolicyRepo,
    actor: egret_core::Principal,
}

async fn env() -> Option<Env> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("clr_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("user");
    Some(Env {
        tags: TagRepo::new(db.clone()),
        repo: CatalogTagRepo::new(db.clone()),
        policies: ClassificationPolicyRepo::new(db.clone(), EventBus::new()),
        policy,
        actor: egret_core::Principal {
            user_id: egret_core::UserId(uid),
            email: "steward@x".into(),
            platform_roles: vec![],
            permissions: vec![],
            group_ids: vec![],
            impersonator: None,
        },
        db,
    })
}

async fn new_tag(e: &Env, label: &str) -> (Uuid, String) {
    let key = format!("{label}-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let tag = e
        .tags
        .create(&key, &key, "", "#ff0000", Some(e.actor.user_id.0))
        .await
        .expect("tag");
    (tag.id, key)
}

#[derive(Debug, sqlx::FromRow)]
struct Event {
    kind: String,
    actor_user_id: Option<Uuid>,
    action: String,
    resource_type: String,
    detail: Value,
}

async fn events(e: &Env, tag: Uuid) -> Vec<Event> {
    sqlx::query_as(
        "SELECT kind, actor_user_id, action, resource_type, detail FROM audit.audit_log \
          WHERE resource_id = $1 ORDER BY id",
    )
    .bind(tag.to_string())
    .fetch_all(&e.db.pool)
    .await
    .expect("events")
}

#[tokio::test]
async fn set_and_clear_record_one_event_each_with_group_names() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let (tag, key) = new_tag(&e, "clr").await;
    let group = unique("clr-analysts");
    let gid: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&group)
        .fetch_one(&e.db.pool)
        .await
        .expect("group");
    let role = unique("clr-role");

    e.policies
        .set_policy(tag, std::slice::from_ref(&role), &[gid], &e.actor)
        .await
        .expect("set");
    let first = json!({
        "tag_key": key, "required_roles": [role], "required_groups": [group]
    });
    let evs = events(&e, tag).await;
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(evs[0].kind, "mutation");
    assert_eq!(evs[0].action, "tag_clearance:set");
    assert_eq!(evs[0].resource_type, "tag");
    assert_eq!(evs[0].actor_user_id, Some(e.actor.user_id.0));
    assert_eq!(evs[0].detail, json!({ "before": null, "after": first }));
    let created_by: Option<Uuid> =
        sqlx::query_scalar("SELECT created_by FROM classification_policies WHERE tag_id = $1")
            .bind(tag)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(
        created_by,
        Some(e.actor.user_id.0),
        "the author is stored as before"
    );

    e.policies
        .set_policy(tag, &[], &[gid], &e.actor)
        .await
        .expect("set again");
    let second = json!({ "tag_key": key, "required_roles": [], "required_groups": [group] });
    let evs = events(&e, tag).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].detail, json!({ "before": first, "after": second }));

    e.policies.clear_policy(tag, &e.actor).await.expect("clear");
    let evs = events(&e, tag).await;
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert_eq!(evs[2].action, "tag_clearance:clear");
    assert_eq!(evs[2].resource_type, "tag");
    assert_eq!(evs[2].detail, json!({ "before": second, "after": null }));

    // Clearing a tag with no gate changes nothing and records nothing.
    e.policies
        .clear_policy(tag, &e.actor)
        .await
        .expect("clear again");
    assert_eq!(events(&e, tag).await.len(), 3);
}

/// A Trino profile with a tag service and one catalog attached; returns the
/// tag service's id and name, the warehouse and the catalog name.
async fn trino_unit(e: &Env) -> (i64, String, Uuid, String) {
    let pool = &e.db.pool;
    let tag_service = unique("clr_tag");
    let tsvc = e
        .policy
        .create_service(&tag_service, "tag", &json!({}))
        .await
        .expect("tag service");
    let svc = e
        .policy
        .create_service(
            &unique("clr_trino"),
            "trino",
            &json!({ "tagService": tag_service }),
        )
        .await
        .expect("profile");
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'Clr') RETURNING id")
            .bind(unique("clr-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-clr','AKIATEST') RETURNING id",
    )
    .bind(unique("clr-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("clr_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("clr-eng"))
    .bind(svc.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("clr_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(wh)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    (tsvc.id, tag_service, wh, catalog)
}

async fn hand_tag_mask(e: &Env, tag_service_id: i64, tag_service: &str, tag: &str, mask: &str) {
    let name = unique("clr mask");
    let body = json!({
        "name": name, "service": tag_service, "serviceType": "tag",
        "policyType": 1, "isEnabled": true, "policyPriority": 0,
        "resources": {"tag": {"values": [tag], "isExcludes": false, "isRecursive": false}},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": mask}
        }],
        "policyItems": [], "denyPolicyItems": []
    });
    e.policy
        .create_policy(tag_service_id, &name, &body)
        .await
        .expect("hand-written tag mask");
}

/// The fixture of `catalog_tag_overlap.rs`'s
/// `a_clearance_that_completes_an_overlap_on_a_view_column_is_refused`.
#[tokio::test]
async fn a_set_refused_by_an_overlap_records_nothing() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let (tsid, tsname, wh, catalog) = trino_unit(&e).await;
    let (a, a_name) = new_tag(&e, "a").await;
    let (b, b_name) = new_tag(&e, "b").await;
    hand_tag_mask(&e, tsid, &tsname, &a_name, "MASK").await;
    hand_tag_mask(&e, tsid, &tsname, &b_name, "MASK_HASH").await;
    let view = egret_perm_compile::lineage::ViewRef {
        catalog: catalog.clone(),
        schema: "sales".into(),
        name: unique("v_clr"),
    };
    {
        let mut conn = e.db.pool.acquire().await.unwrap();
        let tables = std::collections::BTreeSet::from([(
            catalog.clone(),
            "sales".to_string(),
            "orders".to_string(),
        )]);
        egret_perm_compile::lineage::write_lineage(&mut conn, &view, &tables)
            .await
            .unwrap();
        egret_perm_compile::lineage::set_view_columns(
            &mut conn,
            &view,
            Some(&["email".to_string()]),
        )
        .await
        .unwrap();
    }
    e.repo
        .assign_catalog(a, wh, "sales", &view.name, "email", Some(e.actor.user_id.0))
        .await
        .expect("a on the view");
    e.repo
        .assign_catalog(b, wh, "sales", "orders", "email", Some(e.actor.user_id.0))
        .await
        .expect("b on the base");

    let got = e
        .policies
        .set_policy(b, &[unique("ovl-role")], &[], &e.actor)
        .await;
    assert!(
        matches!(got, Err(egret_bi::BiError::Overlap { .. })),
        "the clearance must be refused: {got:?}"
    );
    assert!(
        events(&e, b).await.is_empty(),
        "a refused set records nothing"
    );
}
