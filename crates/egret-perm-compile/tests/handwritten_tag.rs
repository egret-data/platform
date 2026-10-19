//! DG #4b route D (spec §4.5, Q4, CL10): the hand-written tag policies of a
//! classification, read by `handwritten::handwritten_for_tag_via`. A tag
//! policy names a tag NAME, matched through the tag service-def's matcher
//! (`wildCard: true`, `ignoreCase: false`; Task 1 M7 measured the case on
//! Trino).
//!
//! Every tag service in the database is read, so assertions name only this
//! test's own rows. DB-gated; FAILS without a database:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test handwritten_tag

use egret_metastore::Db;
use egret_perm_compile::handwritten::{
    handwritten_for_tag_via, HandWrittenError, TagHandWritten, TagPolicyState,
};
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    pool: PgPool,
    policy: PolicyStore,
}

async fn env() -> Env {
    let dsn = egret_test_db::url()
        .await
        .expect("a database is required (DATABASE_URL)");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.unwrap();
    egret_policy::seed_starrocks(&policy).await.unwrap();
    egret_policy::seed_tag(&policy).await.unwrap();
    Env {
        pool: db.pool.clone(),
        policy,
    }
}

/// A Trino profile with its own tag service and one bound engine.
async fn profile(e: &Env) -> (i64, String, i64, String, Uuid) {
    let tag_name = unique("hwt-tag-");
    let tag = e
        .policy
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap()
        .id;
    let name = unique("hwt-tr-");
    let id = e
        .policy
        .create_service(&name, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap()
        .id;
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(unique("hwt-eng-"))
    .bind(id)
    .fetch_one(&e.pool)
    .await
    .unwrap();
    (id, name, tag, tag_name, engine)
}

async fn tag(e: &Env, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("hwt-k-"))
        .bind(name)
        .fetch_one(&e.pool)
        .await
        .unwrap()
}

async fn insert(e: &Env, service: i64, name: &str, tag_value: &str, by: &str) {
    let body: Value = json!({
        "name": name, "service": "x", "serviceType": "tag", "policyType": 1, "isEnabled": true,
        "resources": { "tag": { "values": [tag_value] } },
        "dataMaskPolicyItems": [{ "users": ["ua"], "groups": ["g1"], "accesses": [{ "type": "select" }],
                                  "dataMaskInfo": { "dataMaskType": "MASK_HASH" } }]
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(service)
    .bind(name)
    .bind(body)
    .bind(by)
    .execute(&e.pool)
    .await
    .unwrap();
}

async fn read(e: &Env, id: Uuid) -> Result<TagHandWritten, HandWrittenError> {
    let mut conn = e.pool.acquire().await.unwrap();
    handwritten_for_tag_via(&mut conn, id).await
}

fn names(r: &TagHandWritten) -> Vec<&str> {
    r.policies.iter().map(|p| p.name.as_str()).collect()
}

#[tokio::test]
async fn tag_policies_match_by_name_through_the_tag_matcher() {
    let e = env().await;
    let (_, pname, tsvc, tname, engine) = profile(&e).await;
    let n = unique("PII");
    let id = tag(&e, &n).await;
    let exact = unique("exact-");
    let wild = unique("wild-");
    let lower = unique("lower-");
    let compiled = unique("perm-clearance-");
    let seeded = unique("seeded-");
    insert(&e, tsvc, &exact, &n, "operator").await;
    insert(&e, tsvc, &wild, &format!("{}*", &n[..4]), "operator").await;
    insert(&e, tsvc, &lower, &n.to_lowercase(), "operator").await;
    insert(&e, tsvc, &compiled, &n, "compiler").await;
    insert(&e, tsvc, &seeded, &n, "infra").await;

    let r = read(&e, id).await.unwrap();
    assert_eq!(r.state, TagPolicyState::Listed);
    let got = names(&r);
    assert!(got.contains(&exact.as_str()), "{got:?}");
    assert!(got.contains(&wild.as_str()), "{got:?}");
    assert!(!got.contains(&lower.as_str()), "no case folding: {got:?}");
    assert!(!got.contains(&compiled.as_str()));
    assert!(!got.contains(&seeded.as_str()));

    let row = r.policies.iter().find(|p| p.name == exact).unwrap();
    assert_eq!(row.service, tname);
    assert!(!row.through_pattern);
    assert_eq!(row.policy_type, "masking");
    assert!(row.enabled);
    assert_eq!(row.profiles.len(), 1);
    assert_eq!(row.profiles[0].name, pname);
    assert_eq!(row.engines.len(), 1);
    assert_eq!(row.engines[0].id, engine);
    assert_eq!(row.items[0].mask_type.as_deref(), Some("MASK_HASH"));
    assert!(!row.items[0].groups[0].on_engines);
    assert!(
        r.policies
            .iter()
            .find(|p| p.name == wild)
            .unwrap()
            .through_pattern
    );
    assert!(r.same_name.is_empty());
}

#[tokio::test]
async fn another_tag_with_the_same_name_is_named() {
    let e = env().await;
    profile(&e).await;
    let n = unique("DUP");
    let a = tag(&e, &n).await;
    let b = tag(&e, &n).await;
    let r = read(&e, a).await.unwrap();
    assert_eq!(r.same_name.len(), 1);
    assert_eq!(r.same_name[0].id, b);
    assert_eq!(r.tag.id, a);
}

#[tokio::test]
async fn unknown_tag_is_not_found() {
    let e = env().await;
    assert!(matches!(
        read(&e, Uuid::new_v4()).await,
        Err(HandWrittenError::NotFound)
    ));
}

/// With no Trino profile naming a tag service, nothing downloads tag
/// policies. Measured inside a transaction that unlinks every tag service and
/// is rolled back, so other tests never see it.
#[tokio::test]
async fn no_tag_service_anywhere_is_its_own_state() {
    let e = env().await;
    profile(&e).await;
    let id = tag(&e, &unique("NTS")).await;
    let mut tx = e.pool.begin().await.unwrap();
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = config_jsonb - 'tagService'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let r = handwritten_for_tag_via(&mut tx, id).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.state, TagPolicyState::NoTagService);
    assert!(r.policies.is_empty());
}
