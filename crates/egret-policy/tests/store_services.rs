//! DB-gated PolicyStore service tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
use egret_metastore::Db;
use egret_policy::{seed_tag, seed_trino, PolicyStore};
use serde_json::json;

async fn store() -> Option<PolicyStore> {
    Some(store_and_db().await?.0)
}

/// Same connection, plus the raw `Db` — the second-direction test needs to issue
/// `UPDATE`s the store has no method for, because that is exactly the point:
/// there IS no such method anywhere in `src/`.
async fn store_and_db() -> Option<(PolicyStore, Db)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some((PolicyStore::new(db.clone()), db))
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
    //
    // `create_service(.., "trino", ..)` has an FK onto `authz_service_defs`, and this
    // test never seeded that row — it passed only because some OTHER test binary had
    // left a `trino` def behind in the shared database. A private database per process
    // turned that into `authz_services_type_fkey` violated, which is the dependency
    // being made explicit here rather than a new requirement.
    seed_trino(&s).await.unwrap();
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

/// SP4d debt Task 2, **Step 5 — the other direction, and every write path that
/// can reach the state.**
///
/// A `tagService` on a non-Trino profile makes every tag policy in that
/// profile's `tagPolicies` block silently inert: `ServiceTags::from_assignments`
/// writes the resource levels `catalog`/`schema`/`table`/`column` for every
/// profile type, `starrocks.json` has no `schema` resource (its middle level is
/// `database`), and Ranger's `RangerTagEnricher.processServiceTags` drops every
/// attachment whose key set no hierarchy in the resource def contains.
///
/// `PolicyStore::create_service` refuses the CREATE direction (unit-tested
/// DB-free in `store.rs`, and over HTTP in `admin_routes.rs`). This test exists
/// because a guard on create alone leaves the hole open from the other side, and
/// **there is no Rust writer to hang a second refusal on**: no `UPDATE` of
/// `type` or of `config_jsonb` exists in any `src/` in this workspace. So the
/// other direction is held by migration 0165's CHECK, and the only way to
/// exercise it is to issue the raw `UPDATE`s here — a test that only drove the
/// route would be a green from a population that cannot reach this behaviour.
///
/// Three writes, one per shape that can produce the state:
///   1. flip an existing Trino profile's `type` to `starrocks` while it carries
///      a `tagService`;
///   2. write a `tagService` onto an existing StarRocks profile;
///   3. INSERT the pair directly, bypassing `create_service` entirely.
#[tokio::test]
async fn the_check_constraint_closes_the_update_directions_too() {
    let Some((s, db)) = store_and_db().await else {
        eprintln!("skip");
        return;
    };
    seed_trino(&s).await.unwrap();
    seed_tag(&s).await.unwrap();
    egret_policy::seed_starrocks(&s).await.unwrap();

    let tag_name = uniq("dir_tag");
    let tag_svc = s
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap();
    let trino_name = uniq("dir_trino");
    let trino_svc = s
        .create_service(&trino_name, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap();
    let sr_name = uniq("dir_sr");
    let sr_svc = s
        .create_service(&sr_name, "starrocks", &json!({}))
        .await
        .unwrap();

    // 1. The type flip. This is the hole the obvious create-side test cannot see.
    let e = sqlx::query("UPDATE policy.authz_services SET type = 'starrocks' WHERE id = $1")
        .bind(trino_svc.id)
        .execute(&db.pool)
        .await
        .expect_err("flipping a tagService-carrying profile to starrocks must be refused");
    assert_constraint_violation(&e, "type flip to starrocks");

    // 2. A `tagService` written onto an existing StarRocks profile.
    let e = sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(sr_svc.id)
        .bind(json!({ "tagService": tag_name }))
        .execute(&db.pool)
        .await
        .expect_err("binding a tag service to a starrocks profile must be refused");
    assert_constraint_violation(&e, "tagService written onto starrocks");

    // 3. A raw INSERT, bypassing `create_service` and its Rust refusal entirely.
    let e = sqlx::query(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) VALUES ($1,'starrocks',$2)",
    )
    .bind(uniq("dir_raw"))
    .bind(json!({ "tagService": tag_name }))
    .execute(&db.pool)
    .await
    .expect_err("a raw INSERT of the pair must be refused");
    assert_constraint_violation(&e, "raw INSERT");

    // Not an over-refusal: the three legal shapes still write.
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(sr_svc.id)
        .bind(json!({ "tagService": serde_json::Value::Null }))
        .execute(&db.pool)
        .await
        .expect("a JSON-null tagService is what `->>` answers NULL for, and must be allowed");
    sqlx::query("UPDATE policy.authz_services SET type = 'starrocks' WHERE id = $1")
        .bind(sr_svc.id)
        .execute(&db.pool)
        .await
        .expect("a starrocks profile with no usable tagService still updates");
    sqlx::query(
        "UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = $1",
    )
    .bind(trino_svc.id)
    .execute(&db.pool)
    .await
    .expect("a trino profile with a tagService still bumps");

    s.delete_service(trino_svc.id).await.unwrap();
    s.delete_service(sr_svc.id).await.unwrap();
    s.delete_service(tag_svc.id).await.unwrap();
}

/// The CHECK must be what refused, named — a bare `is_err()` would pass just as
/// well for a typo in the column name or a dropped connection.
fn assert_constraint_violation(e: &sqlx::Error, what: &str) {
    const NAME: &str = "authz_services_tag_service_needs_schema_level";
    let named = match e {
        sqlx::Error::Database(d) => d.constraint() == Some(NAME),
        _ => false,
    };
    assert!(
        named,
        "{what}: expected migration 0165's CHECK {NAME:?} to refuse, got: {e}"
    );
}
