//! DB-gated PolicyStore service tests. Self-skip without DATABASE_URL.
use egret_metastore::Db;
use egret_policy::{seed_tag, PolicyStore};
use serde_json::json;

async fn store() -> Option<PolicyStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(PolicyStore::new(db))
}
fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn seed_def_then_create_service() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino");
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []}))
        .await
        .unwrap();
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []}))
        .await
        .unwrap(); // idempotent
    assert_eq!(s.get_service_def(&defname).await.unwrap().name, defname);

    let svcname = uniq("trino_prod");
    let svc = s
        .create_service(&svcname, &defname, &json!({}))
        .await
        .unwrap();
    assert_eq!(svc.policy_version, 1);
    assert!(matches!(
        s.create_service(&svcname, &defname, &json!({}))
            .await
            .unwrap_err(),
        egret_policy::PolicyStoreError::AlreadyExists(_)
    ));
    s.delete_service(svc.id).await.unwrap();
    assert!(s.get_service(&svcname).await.is_err());
}

/// Part A (Access P3.2) — the DB-testable slice of tag-service provisioning:
/// the tag service-def seeds, a tag service instance exists with type "tag",
/// and a trino profile's config carries the `tagService` link. (The Ranger
/// runtime correctness of the def + the download wiring is LIVE-ONLY.)
#[tokio::test]
async fn tag_def_seeds_and_tag_service_provisions() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    // 1. Tag service-def seeds (idempotent) and reads back.
    seed_tag(&s).await.unwrap();
    seed_tag(&s).await.unwrap(); // idempotent
    let def = s.get_service_def("tag").await.unwrap();
    assert_eq!(def.name, "tag");
    assert_eq!(def.def_jsonb["resources"][0]["name"], "tag");

    // 2. A tag service instance provisions with type "tag".
    let tag_name = uniq("eng_tag");
    let tag_svc = s
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap();
    assert_eq!(tag_svc.r#type, "tag");
    assert_eq!(s.get_service(&tag_name).await.unwrap().r#type, "tag");

    // 3. The bound trino profile's config carries `tagService = <tag service>`.
    let trino_name = uniq("eng");
    let trino_svc = s
        .create_service(&trino_name, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap();
    assert_eq!(
        s.get_service(&trino_name).await.unwrap().config_jsonb["tagService"],
        json!(tag_name)
    );

    s.delete_service(trino_svc.id).await.unwrap();
    s.delete_service(tag_svc.id).await.unwrap();
}
