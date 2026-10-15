//! **A tag service is whatever a profile's `tagService` names — not only a row
//! of `type = 'tag'`.** Live finding I5.
//!
//! The admin API creates profiles of type `trino` or `starrocks` only, so a tag
//! service made through it is a `trino` profile. Every "is this a tag service"
//! decision that read `type = 'tag'` silently skipped it:
//!
//! * migration 0182's trigger (`WHEN NEW.type = 'tag'`): a write to such a
//!   service's policies bumped only the service, never the profiles that
//!   download its policies as `tagPolicies`, so their plugins were told "not
//!   modified" and kept serving a DELETED tag mask (live);
//! * the unreadable-table-mask guard (`s.type = 'tag'`): a tag mask in such a
//!   service did not count, so a table-level attachment of the same tag was
//!   accepted and made the table unreadable;
//! * the overlap units (`svc.r#type == "tag"`): such a service was its own
//!   resource unit, so overlaps between its tag policies were judged as if they
//!   were path policies.
//!
//! The rule now is: a profile is a tag service when its type is `tag` OR some
//! OTHER profile names it as `tagService`. Migration 0191 and the two Rust
//! sites implement that; the `type = 'tag'` arm stays.
//!
//! DB-gated; FAILS without `DATABASE_URL` unless `EGRET_ALLOW_DB_SKIP` is set.
use egret_metastore::Db;
use egret_policy::{
    seed_tag, seed_trino, units_for_service_via, unreadable_table_mask_for, MaskOrigin,
    PolicyStore, TagWrite, UnitIds,
};
use serde_json::{json, Value};
use uuid::Uuid;

fn uniq(p: &str) -> String {
    format!("{p}_{}", Uuid::new_v4().simple())
}

struct Fixture {
    db: Db,
    store: PolicyStore,
    /// The tag service, of type `trino` — the shape the admin API produces.
    tag_svc: i64,
    tag_svc_name: String,
    /// The profile that names it as `tagService`.
    linker: i64,
    /// A profile that links nothing: it must never move.
    bystander: i64,
}

async fn fixture() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db.clone());
    seed_trino(&store).await.unwrap();
    seed_tag(&store).await.unwrap();
    let tag_svc_name = uniq("ttag");
    let tag_svc = store
        .create_service(&tag_svc_name, "trino", &json!({}))
        .await
        .unwrap()
        .id;
    let linker = store
        .create_service(
            &uniq("tlink"),
            "trino",
            &json!({ "tagService": tag_svc_name }),
        )
        .await
        .unwrap()
        .id;
    let bystander = store
        .create_service(&uniq("tby"), "trino", &json!({}))
        .await
        .unwrap()
        .id;
    Some(Fixture {
        db,
        store,
        tag_svc,
        tag_svc_name,
        linker,
        bystander,
    })
}

impl Fixture {
    async fn version(&self, id: i64) -> i64 {
        sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn versions(&self) -> (i64, i64, i64) {
        (
            self.version(self.tag_svc).await,
            self.version(self.linker).await,
            self.version(self.bystander).await,
        )
    }
}

fn tag_access(name: &str, tag: &str) -> Value {
    json!({
        "name": name, "policyType": 0, "serviceType": "tag", "isEnabled": true,
        "resources": {"tag": {"values": [tag], "isExcludes": false, "isRecursive": false}},
        "denyPolicyItems": [{"users": ["bob"], "accesses": [{"type": "select", "isAllowed": true}]}],
    })
}

fn tag_mask(name: &str, tag: &str) -> Value {
    json!({
        "name": name, "policyType": 1, "serviceType": "tag", "isEnabled": true,
        "resources": {"tag": {"values": [tag], "isExcludes": false, "isRecursive": false}},
        "dataMaskPolicyItems": [{
            "groups": ["public"], "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NULL"},
        }],
    })
}

/// Assert that a write moved the tag service AND its linker, and left the
/// bystander alone.
fn assert_reached_the_linker(what: &str, before: (i64, i64, i64), after: (i64, i64, i64)) {
    assert!(
        after.0 > before.0,
        "{what}: the tag service's own version must move: {before:?} -> {after:?}"
    );
    assert!(
        after.1 > before.1,
        "{what}: the profile naming this trino-typed tag service downloads its policies as \
         tagPolicies, so its version must move too, or its plugin is told 304 and keeps the \
         old block: {before:?} -> {after:?}"
    );
    assert_eq!(
        after.2, before.2,
        "{what}: a profile that links nothing must not move: {before:?} -> {after:?}"
    );
}

#[tokio::test]
async fn inserting_a_policy_in_a_trino_typed_tag_service_bumps_its_linker() {
    let Some(f) = fixture().await else { return };
    let before = f.versions().await;
    f.store
        .create_policy(f.tag_svc, "ins", &tag_access("ins", "vgsecret"))
        .await
        .unwrap();
    assert_reached_the_linker("insert", before, f.versions().await);
}

#[tokio::test]
async fn updating_a_policy_in_a_trino_typed_tag_service_bumps_its_linker() {
    let Some(f) = fixture().await else { return };
    let id = f
        .store
        .create_policy(f.tag_svc, "upd", &tag_access("upd", "vgsecret"))
        .await
        .unwrap();
    let before = f.versions().await;
    f.store
        .update_policy(id, "upd", &tag_access("upd", "vgkey"), true)
        .await
        .unwrap();
    assert_reached_the_linker("update", before, f.versions().await);
}

/// The live I5 shape: the deleted tag mask kept being served.
#[tokio::test]
async fn deleting_a_policy_in_a_trino_typed_tag_service_bumps_its_linker() {
    let Some(f) = fixture().await else { return };
    let id = f
        .store
        .create_policy(f.tag_svc, "del", &tag_mask("del", "vgkey"))
        .await
        .unwrap();
    let before = f.versions().await;
    f.store.delete_policy(id).await.unwrap();
    assert_reached_the_linker("delete", before, f.versions().await);
}

/// The cascade is ONE hop, exactly as the download embeds one level: a bump of
/// the linker (which is itself a `trino` profile, and could in principle be
/// named by a third) must not ripple back or onward.
#[tokio::test]
async fn a_bump_of_the_linker_does_not_move_the_tag_service() {
    let Some(f) = fixture().await else { return };
    let before = f.versions().await;
    f.store
        .create_policy(f.linker, "res", &json!({
            "name": "res", "policyType": 0, "isEnabled": true,
            "resources": {
                "catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                "table": {"values": ["t"]}, "column": {"values": ["*"]},
            },
            "policyItems": [{"users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}]}],
        }))
        .await
        .unwrap();
    let after = f.versions().await;
    assert_eq!(after.0, before.0, "{before:?} -> {after:?}");
    assert!(after.1 > before.1, "{before:?} -> {after:?}");
    assert_eq!(after.2, before.2, "{before:?} -> {after:?}");
}

/// The unreadable-table-mask guard: a tag MASK in a trino-typed tag service is
/// a tag mask, so a TABLE-level attachment of the same tag is refused.
#[tokio::test]
async fn a_table_level_attachment_is_refused_when_a_trino_typed_tag_service_masks_the_tag() {
    let Some(f) = fixture().await else { return };
    let suffix = Uuid::new_v4().simple().to_string();
    let tag_name = format!("TT-N-{suffix}");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("tt-k-{suffix}"))
            .bind(&tag_name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let mask = uniq("ttmask");
    f.store
        .create_policy(f.tag_svc, &mask, &tag_mask(&mask, &tag_name))
        .await
        .unwrap();

    let mut conn = f.db.pool.acquire().await.unwrap();
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Attachment {
            tag_id,
            schema: "sales",
            table: "orders",
            column: "",
        },
    )
    .await
    .expect("query")
    .expect(
        "a tag mask in a trino-typed tag service masks by tag on the linker's engine, so the \
         table-level attachment must be refused",
    );
    assert_eq!(pair.mask_origin, MaskOrigin::Policy);
    assert_eq!(pair.mask, mask);
}

/// The overlap units: a write to a trino-typed tag service lands in the unit
/// of every profile naming it, with the service in the TAG slot.
#[tokio::test]
async fn the_overlap_unit_of_a_trino_typed_tag_service_is_its_linker() {
    let Some(f) = fixture().await else { return };
    let mut conn = f.db.pool.acquire().await.unwrap();
    let units = units_for_service_via(&mut conn, f.tag_svc).await.unwrap();
    assert_eq!(
        units,
        vec![UnitIds {
            resource: Some(f.linker),
            tag_service: Some(f.tag_svc),
        }],
        "the tag service {} must be grouped with its linker",
        f.tag_svc_name
    );
    // And the bystander, which nobody names, is still a resource unit alone.
    let units = units_for_service_via(&mut conn, f.bystander).await.unwrap();
    assert_eq!(
        units,
        vec![UnitIds {
            resource: Some(f.bystander),
            tag_service: None,
        }]
    );
}

/// **A cycle terminates.** With 0182's `type = 'tag'` guard gone, two `trino`
/// profiles naming each other would cascade A → B → A → … until Postgres'
/// stack-depth limit aborted the writer's transaction. `pg_trigger_depth()`
/// stops the cascade after one hop: each side moves exactly once.
#[tokio::test]
async fn two_profiles_naming_each_other_bump_once_and_terminate() {
    let Some(f) = fixture().await else { return };
    let a = f.tag_svc;
    let b = f.linker;
    // Close the cycle: the tag service now names its own linker. 0176's CHECK
    // allows it (both are `trino`), and the store's create-time guards are not
    // what is under test.
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(a)
        .bind(json!({ "tagService": sqlx::query_scalar::<_, String>(
                "SELECT name FROM policy.authz_services WHERE id = $1")
            .bind(b)
            .fetch_one(&f.db.pool)
            .await
            .unwrap() }))
        .execute(&f.db.pool)
        .await
        .unwrap();
    let (a0, b0) = (f.version(a).await, f.version(b).await);
    f.store
        .create_policy(a, "cyc", &tag_access("cyc", "vgkey"))
        .await
        .expect("a write in a cycle must not abort on stack depth");
    assert_eq!(
        (f.version(a).await - a0, f.version(b).await - b0),
        (1, 1),
        "one hop each way: the writer bumps A, the trigger bumps B, and B's own firing stops"
    );
}
