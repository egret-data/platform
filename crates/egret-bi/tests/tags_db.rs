//! Classification tag model DB harness (Access P3.1). DB-gated; self-skips
//! without DATABASE_URL. Covers the `TagRepo` CRUD + assignment round-trips
//! against migration 0080's `tags` / `tag_assignments` tables.
//!
//! METADATA ONLY: no authorization decision reads these tables in this phase.

use egret_bi::tags::TagRepo;
use egret_bi::BiError;
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
async fn create_dup_conflict_count_assign_cascade() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let repo = TagRepo::new(db.clone());
    let by = seed_user(&db).await;
    let key = format!("pii-{}", Uuid::new_v4().simple());

    // create inserts a tag.
    let tag = repo
        .create(&key, "PII", "Personal data", "#ff0000", Some(by))
        .await
        .expect("create tag");
    assert_eq!(tag.key, key);
    assert_eq!(tag.name, "PII");
    assert_eq!(tag.description, "Personal data");
    assert_eq!(tag.color, "#ff0000");
    assert_eq!(tag.assignment_count, 0);

    // a second create with the SAME key -> Conflict.
    let dup = repo
        .create(&key, "PII 2", "", "", Some(by))
        .await
        .expect_err("dup key must conflict");
    assert!(
        matches!(dup, BiError::Conflict(_)),
        "expected Conflict, got {dup:?}"
    );

    // an invalid key -> BadRequest (not a DB error).
    let bad = repo
        .create("Has Spaces!", "x", "", "", Some(by))
        .await
        .expect_err("invalid key must be rejected");
    assert!(
        matches!(bad, BiError::BadRequest(_)),
        "expected BadRequest, got {bad:?}"
    );

    // get(id) round-trips.
    let got = repo.get(tag.id).await.expect("get").expect("some");
    assert_eq!(got.id, tag.id);
    assert_eq!(got.key, key);

    // list() shows the tag with assignment_count = 0.
    let listed = repo.list().await.expect("list");
    let row = listed.iter().find(|t| t.id == tag.id).expect("tag in list");
    assert_eq!(row.assignment_count, 0);

    // assign to two objects; assignment_count becomes 2.
    let obj_a = Uuid::new_v4();
    let obj_b = Uuid::new_v4();
    repo.assign(tag.id, "dataset", obj_a, Some(by))
        .await
        .expect("assign a");
    // idempotent: assigning the same (tag, type, object) again is fine.
    repo.assign(tag.id, "dataset", obj_a, Some(by))
        .await
        .expect("assign a again (idempotent)");
    repo.assign(tag.id, "dataset", obj_b, Some(by))
        .await
        .expect("assign b");

    let listed = repo.list().await.expect("list");
    let row = listed.iter().find(|t| t.id == tag.id).expect("tag in list");
    assert_eq!(row.assignment_count, 2, "two distinct objects assigned");

    // tags_for_object round-trip: obj_a carries our tag.
    let for_a = repo
        .tags_for_object("dataset", obj_a)
        .await
        .expect("tags_for_object");
    assert!(for_a.iter().any(|t| t.id == tag.id), "obj_a has the tag");

    // unassign obj_b -> count back to 1.
    repo.unassign(tag.id, "dataset", obj_b)
        .await
        .expect("unassign b");
    let for_b = repo
        .tags_for_object("dataset", obj_b)
        .await
        .expect("tags_for_object b");
    assert!(!for_b.iter().any(|t| t.id == tag.id), "obj_b lost the tag");

    // update: change name/desc/color (COALESCE the optionals).
    let updated = repo
        .update(tag.id, Some("PII v2"), Some("Sensitive"), None)
        .await
        .expect("update")
        .expect("some");
    assert_eq!(updated.name, "PII v2");
    assert_eq!(updated.description, "Sensitive");
    assert_eq!(updated.color, "#ff0000", "color unchanged when None");

    // delete cascades the remaining assignment on obj_a.
    repo.delete(tag.id).await.expect("delete");
    assert!(repo.get(tag.id).await.expect("get after delete").is_none());
    let for_a = repo
        .tags_for_object("dataset", obj_a)
        .await
        .expect("tags_for_object after delete");
    assert!(
        !for_a.iter().any(|t| t.id == tag.id),
        "delete cascades assignments"
    );
}
