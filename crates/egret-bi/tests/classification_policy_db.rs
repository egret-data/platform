//! Classification access-policy model DB harness (Access P3.3). DB-gated;
//! self-skips without DATABASE_URL. Covers the `ClassificationPolicyRepo`
//! set/get/clear/list round-trips + the `ON DELETE CASCADE` from `tags`
//! against migration 0082's `classification_policies` table.
//!
//! MODEL + CRUD ONLY: no authorization decision reads this table in this task
//! (the Cedar PDP wiring is a later task). Empty `required_roles` = NO gate.

use egret_bi::tags::{ClassificationPolicyRepo, TagRepo};
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

#[tokio::test]
async fn set_get_update_clear_and_cascade() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());
    let by = seed_user(&db).await;
    let key = format!("secret-{}", Uuid::new_v4().simple());

    // A tag with no policy yet -> get returns None.
    let tag = tags
        .create(&key, "Secret", "Restricted", "#000000", Some(by))
        .await
        .expect("create tag");
    assert_eq!(pol.get_policy(tag.id).await.expect("get"), None);

    // set_policy inserts the required-role list; get round-trips it.
    let roles = vec!["clearance_high".to_string(), "auditor".to_string()];
    pol.set_policy(tag.id, &roles, Some(by))
        .await
        .expect("set policy");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some(roles.clone())
    );

    // set_policy again UPSERTS (updates) the role list for the same tag.
    let roles2 = vec!["clearance_top".to_string()];
    pol.set_policy(tag.id, &roles2, Some(by))
        .await
        .expect("update policy");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some(roles2.clone())
    );

    // list_policies surfaces the (tag_id, roles) pair for the PDP loader / UI.
    let listed = pol.list_policies().await.expect("list");
    assert!(
        listed.iter().any(|(id, r)| *id == tag.id && *r == roles2),
        "list_policies must contain the set policy, got {listed:?}"
    );

    // an empty role list is a legal (informational, NO gate) policy row.
    pol.set_policy(tag.id, &[], Some(by))
        .await
        .expect("set empty");
    assert_eq!(pol.get_policy(tag.id).await.expect("get"), Some(vec![]));

    // clear_policy removes the row.
    pol.clear_policy(tag.id).await.expect("clear");
    assert_eq!(pol.get_policy(tag.id).await.expect("get"), None);

    // deleting the tag CASCADEs any policy row away.
    pol.set_policy(tag.id, &roles, Some(by))
        .await
        .expect("re-set policy");
    assert!(pol.get_policy(tag.id).await.expect("get").is_some());
    tags.delete(tag.id).await.expect("delete tag");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get after tag delete"),
        None,
        "deleting the tag must CASCADE-delete its classification policy"
    );
}
