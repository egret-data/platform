//! Classification tag model DB harness (Access P3.1). DB-gated; self-skips without
//! `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Covers the
//! `TagRepo` CRUD + assignment round-trips against migration 0080's `tags` /
//! `tag_assignments` tables.
//!
//! METADATA ONLY: no authorization decision reads these tables in this phase.

use egret_bi::tags::TagRepo;
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
    let Some(url) = egret_test_db::url().await else {
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

    // a second create with the SAME key -> tag_key_taken, naming the holder.
    let dup = repo
        .create(&key, "PII 2", "", "", Some(by))
        .await
        .expect_err("dup key must conflict");
    assert_eq!(dup.code(), Some("tag_key_taken"), "got {dup:?}");
    let egret_bi::BiError::Coded { status, body } = &dup else {
        panic!("{dup:?}")
    };
    assert_eq!(*status, axum::http::StatusCode::CONFLICT);
    assert_eq!(body["key"], key);
    assert_eq!(body["existing"]["id"], tag.id.to_string());
    assert_eq!(body["existing"]["name"], "PII");

    // an invalid key -> tag_key_invalid (not a DB error), `_` included.
    for bad_key in ["Has Spaces!", "a_b"] {
        let bad = repo
            .create(bad_key, "x", "", "", Some(by))
            .await
            .expect_err("invalid key must be rejected");
        assert_eq!(bad.code(), Some("tag_key_invalid"), "{bad_key}: {bad:?}");
    }

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

/// SP4a Task 7 fix round 1: the reviewer flagged a THIRD staleness path — a
/// tag deleted out from under a StarRocks masking rule cascades away its
/// `catalog_tag_assignments` with no `tag_version` bump and no recompile,
/// leaving a compiled policy that still masks columns nobody has tagged
/// PII any more. Investigated rather than assumed: `perm.rules.tag_key`
/// (migration 0154) references `tags(key)` with no `ON DELETE` clause — the
/// Postgres default is RESTRICT — so deleting a tag a live rule still
/// SELECTS is refused outright, not silently allowed to cascade. This test
/// pins that refusal (mapped to `BiError::Conflict`, not a bare 500) and,
/// separately, that a tag with NO rule referencing it — the only shape that
/// CAN be deleted — never had a compiled policy to begin with, so its
/// cascade is not a staleness case at all.
#[tokio::test]
async fn deleting_a_tag_a_live_rule_selects_is_refused_not_silently_cascaded() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let repo = TagRepo::new(db.clone());
    let by = seed_user(&db).await;
    let key = format!("fk-guard-{}", Uuid::new_v4().simple());
    let tag = repo
        .create(&key, &key, "", "", Some(by))
        .await
        .expect("create tag");

    let rule_id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, tag_key) \
         VALUES ($1, 'mask', 'MASK_HASH', 'tag', $2) RETURNING id",
    )
    .bind(format!("fk-guard-rule-{}", Uuid::new_v4().simple()))
    .bind(&key)
    .fetch_one(&db.pool)
    .await
    .expect("insert perm rule referencing the tag");

    let err = repo
        .delete(tag.id)
        .await
        .expect_err("a tag a live perm.rules row selects must not delete");
    assert_eq!(err.code(), Some("tag_in_use"), "got {err:?}");
    let egret_bi::BiError::Coded { body, .. } = &err else {
        panic!("{err:?}")
    };
    let ids: Vec<&str> = body["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![rule_id.to_string().as_str()]);
    assert!(
        !body["error"].as_str().unwrap().contains("perm.rules"),
        "no internal table names: {body}"
    );
    assert!(
        repo.get(tag.id).await.expect("get").is_some(),
        "the tag must still exist — the delete must not have partially applied"
    );
}

#[tokio::test]
async fn name_taken_is_case_insensitive_and_skips_the_tag_itself() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let repo = TagRepo::new(db.clone());
    let by = seed_user(&db).await;
    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("Public {suffix}");
    let t = repo
        .create(&format!("pub-{suffix}"), &name, "", "", Some(by))
        .await
        .expect("create");

    let hit = repo
        .name_taken(&name.to_uppercase(), None)
        .await
        .expect("query")
        .expect("an upper-case spelling is the same name");
    assert_eq!((hit.id, hit.key.as_str()), (t.id, t.key.as_str()));
    assert!(
        repo.name_taken(&format!("  {name}  "), None)
            .await
            .unwrap()
            .is_some(),
        "trimmed"
    );
    assert!(
        repo.name_taken(&name.to_lowercase(), Some(t.id))
            .await
            .unwrap()
            .is_none(),
        "renaming a classification to its own name in another case is not a duplicate"
    );
    assert!(repo
        .name_taken(&format!("Other {suffix}"), None)
        .await
        .unwrap()
        .is_none());
}

/// Migration 0207's unique index on `lower(tags.name)` is the backstop for the
/// handlers' `name_taken` check, which two concurrent writes can both pass. The
/// repo is called directly here, past that check, so the write that loses the
/// race reaches the index: both a create and a rename must answer the same coded
/// `tag_name_taken` the check gives, not a 500.
#[tokio::test]
async fn a_name_the_index_refuses_is_tag_name_taken() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let repo = TagRepo::new(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("Race {suffix}");
    let holder = repo
        .create(&format!("race-a-{suffix}"), &name, "", "", None)
        .await
        .expect("create the holder");

    let created = repo
        .create(
            &format!("race-b-{suffix}"),
            &name.to_uppercase(),
            "",
            "",
            None,
        )
        .await
        .expect_err("a create into a taken name must be refused");
    let other = repo
        .create(
            &format!("race-c-{suffix}"),
            &format!("Other {suffix}"),
            "",
            "",
            None,
        )
        .await
        .expect("create a second classification");
    let renamed = repo
        .update(other.id, Some(&name.to_lowercase()), None, None)
        .await
        .expect_err("a rename into a taken name must be refused");

    for (what, err) in [("create", created), ("rename", renamed)] {
        let egret_bi::BiError::Coded { status, body } = &err else {
            panic!("{what}: {err:?}")
        };
        assert_eq!(*status, axum::http::StatusCode::CONFLICT, "{what}: {body}");
        assert_eq!(body["code"], "tag_name_taken", "{what}: {body}");
        assert_eq!(
            body["existing"]["id"],
            holder.id.to_string(),
            "{what}: {body}"
        );
    }
    assert_eq!(
        repo.get(other.id).await.unwrap().expect("still there").name,
        format!("Other {suffix}"),
        "the refused rename wrote nothing"
    );
}
