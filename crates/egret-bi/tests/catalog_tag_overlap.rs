//! SP4d-4 Task 3: a tag ASSIGNMENT that makes two mask (or row-filter)
//! policies decide one column differently is refused with 409 naming both and
//! the column — between compiled rules (refused by the recompile the write
//! runs), hand-written tag policies (user ruling), or one of each (refused by
//! the write's own before/after check over the stored unit).
//!
//! Driven through `CatalogTagRepo::assign_catalog`, the method the HTTP
//! handler calls, and its error rendered through `IntoResponse`, so the status
//! and body asserted are what a client receives.
//!
//! Tag rules are catalog-unscoped (every Trino profile compiles every one), so
//! the tests in this binary run one at a time ([`SERIAL`]); the binary has a
//! private database.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-bi --test catalog_tag_overlap

use axum::http::StatusCode;
use axum::response::IntoResponse;
use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::TagRepo;
use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Scope, Selector};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    tags: TagRepo,
    repo: CatalogTagRepo,
    by: Uuid,
}

struct Unit {
    tag_service_id: i64,
    tag_service: String,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    catalog: String,
}

async fn env() -> Option<Env> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    let by: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("ovl_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("user");
    Some(Env {
        perm: PermStore::new(db.pool.clone()),
        tags: TagRepo::new(db.clone()),
        repo: CatalogTagRepo::new(db.clone()),
        policy,
        by,
        db,
    })
}

/// A Trino profile with a tag service, one engine bound to it, one registry
/// catalog attached.
async fn trino_unit(e: &Env) -> Unit {
    let pool = &e.db.pool;
    let tag_service = unique("ovlb_tag");
    let tag = e
        .policy
        .create_service(&tag_service, "tag", &json!({}))
        .await
        .expect("tag service");
    let svc = e
        .policy
        .create_service(
            &unique("ovlb_trino"),
            "trino",
            &json!({ "tagService": tag_service }),
        )
        .await
        .expect("profile");
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'Ovl') RETURNING id")
            .bind(unique("ovlb-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-ovl','AKIATEST') RETURNING id",
    )
    .bind(unique("ovlb-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("ovlb_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("ovlb-eng"))
    .bind(svc.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("ovlb_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
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
    Unit {
        tag_service_id: tag.id,
        tag_service,
        warehouse_id,
        catalog_id,
        catalog,
    }
}

/// A tag; `key` and `name` are the same string. Returns `(id, name)`.
async fn new_tag(e: &Env, label: &str) -> (Uuid, String) {
    let key = format!("{label}-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let tag = e
        .tags
        .create(&key, &key, "", "#ff0000", Some(e.by))
        .await
        .expect("tag");
    (tag.id, key)
}

async fn tag_rule(e: &Env, name: &str, key: &str, mask: &str) -> Uuid {
    e.perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: name.to_string(),
            kind: RuleKind::Mask {
                mask_type: mask.to_string(),
                value_expr: None,
            },
            selector: Selector::Tag {
                key: key.to_string(),
            },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("tag rule")
}

/// A hand-written tag mask stored under `u`'s tag service (a `manual` service
/// until a compile claims it).
async fn hand_tag_mask(e: &Env, u: &Unit, name: &str, tag: &str, mask: &str) {
    let body = json!({
        "name": name, "service": u.tag_service, "serviceType": "tag",
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
        .create_policy(u.tag_service_id, name, &body)
        .await
        .expect("hand-written tag mask");
}

async fn assign(e: &Env, u: &Unit, tag: Uuid, column: &str) -> Result<(), (StatusCode, Value)> {
    match e
        .repo
        .assign_catalog(tag, u.warehouse_id, "sales", "orders", column, Some(e.by))
        .await
    {
        Ok(()) => Ok(()),
        Err(err) => {
            let resp = err.into_response();
            let status = resp.status();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            Err((
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            ))
        }
    }
}

async fn assignments_on(e: &Env, u: &Unit, column: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM catalog_tag_assignments \
          WHERE warehouse_id = $1 AND table_name = 'orders' AND column_name = $2",
    )
    .bind(u.warehouse_id)
    .bind(column)
    .fetch_one(&e.db.pool)
    .await
    .expect("count")
}

fn assert_overlap(got: Result<(), (StatusCode, Value)>, labels: &[String], column: &str) -> Value {
    let (status, body) = got.expect_err("the assignment must be refused");
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    assert_eq!(body["rules"].as_array().map(Vec::len), Some(2), "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    for l in labels {
        assert!(error.contains(l.as_str()), "names {l}: {error}");
    }
    assert!(error.contains(column), "names the column {column}: {error}");
    body
}

/// Test 3 + test 4 (compiled rules): two masked tags on one column are refused
/// at the assignment that puts the second there; on two different columns
/// both assignments succeed.
#[tokio::test]
async fn a_tag_assignment_that_puts_two_masked_tags_on_one_column_is_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let u = trino_unit(&e).await;
    let (pii, pii_key) = new_tag(&e, "pii").await;
    let (gdpr, gdpr_key) = new_tag(&e, "gdpr").await;
    let (pii_rule, gdpr_rule) = (unique("mask pii"), unique("hash gdpr"));
    tag_rule(&e, &pii_rule, &pii_key, "MASK").await;
    tag_rule(&e, &gdpr_rule, &gdpr_key, "MASK_HASH").await;

    // Test 4, the compile-time-only case: no shared column yet, both admitted.
    assign(&e, &u, pii, "email").await.expect("pii on email");
    assign(&e, &u, gdpr, "phone").await.expect("gdpr on phone");

    // Test 3: gdpr onto email as well completes the pair.
    let column = format!("{}.sales.orders.email", u.catalog);
    let body = assert_overlap(
        assign(&e, &u, gdpr, "email").await,
        &[format!("rule {pii_rule:?}"), format!("rule {gdpr_rule:?}")],
        &column,
    );
    assert!(
        body["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"].is_string()),
        "both members are compiled rules, named by id: {body}"
    );
    // The sentence describes the assignment, not a recompile nobody asked for.
    let error = body["error"].as_str().unwrap();
    assert!(
        error.starts_with("assigning this tag to sales.orders.email would create"),
        "{error}"
    );
    assert!(!error.contains("recompiling profile"), "{error}");
    assert_eq!(
        assignments_on(&e, &u, "email").await,
        1,
        "only pii stays on email"
    );
}

/// Test 5, assignment-after-rule: a tag rule and a path rule, then the
/// assignment that puts the tag on the path rule's column.
#[tokio::test]
async fn an_assignment_under_a_path_rule_completes_a_tag_and_path_pair() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let u = trino_unit(&e).await;
    let (pii, pii_key) = new_tag(&e, "pii").await;
    let tag_name = unique("mask pii");
    tag_rule(&e, &tag_name, &pii_key, "MASK").await;
    let path_name = unique("null email");
    e.perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: path_name.clone(),
            kind: RuleKind::Mask {
                mask_type: "MASK_NULL".into(),
                value_expr: None,
            },
            selector: Selector::Path(Scope {
                catalog_id: u.catalog_id,
                schema: Some("sales".into()),
                table: Some("orders".into()),
                column: Some("email".into()),
            }),
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("path rule");
    // Accept-control: the tag on another column.
    assign(&e, &u, pii, "sku").await.expect("pii on sku");
    assert_overlap(
        assign(&e, &u, pii, "email").await,
        &[format!("rule {tag_name:?}"), format!("rule {path_name:?}")],
        &format!("{}.sales.orders.email", u.catalog),
    );
    assert_eq!(assignments_on(&e, &u, "email").await, 0);
}

/// User ruling: HAND-WRITTEN tag masks on a manual profile, refused at the
/// assignment that puts both tags on one column — no compile sees them.
#[tokio::test]
async fn an_assignment_that_pairs_two_hand_written_tag_masks_is_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let u = trino_unit(&e).await;
    let (pii, pii_name) = new_tag(&e, "hpii").await;
    let (gdpr, gdpr_name) = new_tag(&e, "hgdpr").await;
    let (a, b) = (unique("hand pii"), unique("hand gdpr"));
    hand_tag_mask(&e, &u, &a, &pii_name, "MASK").await;
    hand_tag_mask(&e, &u, &b, &gdpr_name, "MASK_NULL").await;

    assign(&e, &u, pii, "email").await.expect("pii on email");
    assign(&e, &u, gdpr, "phone").await.expect("gdpr on phone");
    let body = assert_overlap(
        assign(&e, &u, gdpr, "email").await,
        &[format!("policy {a:?}"), format!("policy {b:?}")],
        &format!("{}.sales.orders.email", u.catalog),
    );
    assert!(
        body["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"].is_null()),
        "hand-written members have no rule id: {body}"
    );
    assert_eq!(assignments_on(&e, &u, "email").await, 1);
    // Unassigning is never refused: it can only remove pairs.
    e.repo
        .unassign_catalog(pii, u.warehouse_id, "sales", "orders", "email")
        .await
        .expect("unassign");
}

/// User ruling, "one of each": a compiled tag rule and a hand-written tag mask
/// (stored before the compile claimed the tag service) meeting on a column.
#[tokio::test]
async fn an_assignment_that_pairs_a_rule_with_a_hand_written_tag_mask_is_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let u = trino_unit(&e).await;
    let (pii, pii_key) = new_tag(&e, "mpii").await;
    let (gdpr, gdpr_name) = new_tag(&e, "mgdpr").await;
    let hand = unique("hand gdpr");
    hand_tag_mask(&e, &u, &hand, &gdpr_name, "MASK_NULL").await;
    let rule = unique("mask pii");
    tag_rule(&e, &rule, &pii_key, "MASK").await;

    assign(&e, &u, pii, "email").await.expect("pii on email");
    assert_overlap(
        assign(&e, &u, gdpr, "email").await,
        &[format!("rule {rule:?}"), format!("policy {hand:?}")],
        &format!("{}.sales.orders.email", u.catalog),
    );
    assert_eq!(assignments_on(&e, &u, "email").await, 1);
}

/// SP4d-4 Task 3 fix round 3 (re-review M-2): a tag RENAME into the name a
/// hand-written tag mask addresses completes a pair with the mask already
/// deciding that column. The rename's recompile reads the new name on both of
/// its sides and cannot see it; the rename's own before-snapshot refuses it,
/// and nothing is renamed. Accept-control: renaming into an unaddressed name.
#[tokio::test]
async fn a_rename_into_a_hand_written_masks_tag_name_completing_a_pair_is_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let u = trino_unit(&e).await;
    let (pii, pii_name) = new_tag(&e, "rpii").await;
    let (other, other_name) = new_tag(&e, "rother").await;
    let target = format!("rgdpr-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let (a, b) = (unique("hand pii"), unique("hand target"));
    hand_tag_mask(&e, &u, &a, &pii_name, "MASK").await;
    hand_tag_mask(&e, &u, &b, &target, "MASK_NULL").await;
    assign(&e, &u, pii, "email").await.expect("pii on email");
    assign(&e, &u, other, "email")
        .await
        .expect("other on email");

    let refused = match e.tags.update(other, Some(&target), None, None).await {
        Ok(t) => panic!("the rename must be refused, got {t:?}"),
        Err(err) => {
            let resp = err.into_response();
            let status = resp.status();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let body = assert_overlap(
        Err(refused),
        &[format!("policy {a:?}"), format!("policy {b:?}")],
        &format!("{}.sales.orders.email", u.catalog),
    );
    assert_eq!(body["kind"], "mask", "{body}");
    let name: String = sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(other)
        .fetch_one(&e.db.pool)
        .await
        .expect("name");
    assert_eq!(name, other_name, "nothing renamed");

    let free = format!("rfree-{}", &Uuid::new_v4().simple().to_string()[..12]);
    e.tags
        .update(other, Some(&free), None, None)
        .await
        .expect("a rename into an unaddressed name passes");
}
