//! Classification access-policy model DB harness (Access P3.3). DB-gated;
//! self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Covers the `ClassificationPolicyRepo`
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
    let Some(url) = egret_test_db::url().await else {
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
        .create(&key, &key, "Restricted", "#000000", Some(by))
        .await
        .expect("create tag");
    assert_eq!(pol.get_policy(tag.id).await.expect("get"), None);

    // set_policy inserts the required-role + required-group lists; get round-trips.
    let roles = vec!["clearance_high".to_string(), "auditor".to_string()];
    let g1 = Uuid::new_v4();
    let g2 = Uuid::new_v4();
    let groups = vec![g1, g2];
    pol.set_policy(tag.id, &roles, &groups, &principal_of(by))
        .await
        .expect("set policy");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some((roles.clone(), groups.clone()))
    );

    // set_policy again UPSERTS (updates) BOTH lists for the same tag.
    let roles2 = vec!["clearance_top".to_string()];
    let groups2 = vec![Uuid::new_v4()];
    pol.set_policy(tag.id, &roles2, &groups2, &principal_of(by))
        .await
        .expect("update policy");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some((roles2.clone(), groups2.clone()))
    );

    // A GROUP-ONLY policy (no roles) is a legal gate.
    let group_only = vec![Uuid::new_v4()];
    pol.set_policy(tag.id, &[], &group_only, &principal_of(by))
        .await
        .expect("set group-only");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some((vec![], group_only.clone()))
    );

    // list_policies surfaces the (tag_id, roles, groups) triple for the PDP / UI.
    pol.set_policy(tag.id, &roles2, &groups2, &principal_of(by))
        .await
        .expect("re-set for list");
    let listed = pol.list_policies().await.expect("list");
    assert!(
        listed
            .iter()
            .any(|(id, r, g)| *id == tag.id && *r == roles2 && *g == groups2),
        "list_policies must contain the set policy triple, got {listed:?}"
    );

    // both lists empty is a legal (informational, NO gate) policy row.
    pol.set_policy(tag.id, &[], &[], &principal_of(by))
        .await
        .expect("set empty");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some((vec![], vec![]))
    );

    // clear_policy removes the row.
    pol.clear_policy(tag.id, &principal_of(by))
        .await
        .expect("clear");
    assert_eq!(pol.get_policy(tag.id).await.expect("get"), None);

    // deleting the tag CASCADEs any policy row away.
    pol.set_policy(tag.id, &roles, &groups, &principal_of(by))
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

/// FIX 1a (security): `set_policy` validates every `required_roles` entry against
/// a safe role-name charset BEFORE the INSERT, so a `tag:manage` caller cannot
/// inject a Cedar policy-string payload (which would either break the PDP reload
/// or splice a `|| true` that disables the clearance gate). A single bad entry
/// rejects the whole call with `BadRequest` (→ HTTP 400); a valid list is Ok.
#[tokio::test]
async fn set_policy_rejects_injection_payloads() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());
    let by = seed_user(&db).await;
    let key = format!("secret-{}", Uuid::new_v4().simple());
    let tag = tags
        .create(&key, &key, "Restricted", "#000000", Some(by))
        .await
        .expect("create tag");

    // A quote-injection entry → rejected.
    let bad_quote = pol
        .set_policy(tag.id, &["bad\"role".to_string()], &[], &principal_of(by))
        .await;
    assert!(
        matches!(bad_quote, Err(egret_bi::BiError::BadRequest(_))),
        "a role name containing a quote must be rejected as BadRequest, got {bad_quote:?}"
    );

    // A `|| true` gate-disabling payload → rejected.
    let bad_inject = pol
        .set_policy(
            tag.id,
            &["x\"]) || true || context.roles.containsAny([\"y".to_string()],
            &[],
            &principal_of(by),
        )
        .await;
    assert!(
        matches!(bad_inject, Err(egret_bi::BiError::BadRequest(_))),
        "a `|| true` injection payload must be rejected, got {bad_inject:?}"
    );

    // Other unsafe chars (backslash, bracket, space, pipe, empty) → rejected;
    // and a MIXED list with one bad entry rejects the WHOLE call (no partial write).
    for bad in ["back\\slash", "brack]et", "has space", "pi|pe", "", "UPPER"] {
        let r = pol
            .set_policy(tag.id, &[bad.to_string()], &[], &principal_of(by))
            .await;
        assert!(
            matches!(r, Err(egret_bi::BiError::BadRequest(_))),
            "role name {bad:?} must be rejected, got {r:?}"
        );
    }
    let mixed = pol
        .set_policy(
            tag.id,
            &["finance_cleared".to_string(), "bad\"role".to_string()],
            &[],
            &principal_of(by),
        )
        .await;
    assert!(
        matches!(mixed, Err(egret_bi::BiError::BadRequest(_))),
        "one bad entry must reject the whole call, got {mixed:?}"
    );
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        None,
        "a rejected set_policy must not have written any row"
    );

    // A valid list (egret role names: lowercase + `_`/`:`/`-`) + groups → Ok.
    let good = vec![
        "finance_cleared".to_string(),
        "data_steward".to_string(),
        "role:sub-team".to_string(),
    ];
    let good_groups = vec![Uuid::new_v4()];
    pol.set_policy(tag.id, &good, &good_groups, &principal_of(by))
        .await
        .expect("valid clearance role names must be accepted");
    assert_eq!(
        pol.get_policy(tag.id).await.expect("get"),
        Some((good, good_groups))
    );
}

/// DG #3: the acting principal of a clearance write, for a user that exists.
fn principal_of(id: uuid::Uuid) -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(id),
        email: "actor@egret.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}
