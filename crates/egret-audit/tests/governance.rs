//! `egret_audit::governance` (DG #3, spec §3.1): governance events written in
//! the write's own transaction, and the feed query that reads them back.
//!
//! DB-gated through `egret_test_db::url()`, which FAILS without a database
//! unless `EGRET_ALLOW_DB_SKIP` is set. Every test reads back only rows whose
//! `resource_id` it generated, because the binary's tests share one database.
use chrono::{DateTime, Duration, TimeZone, Utc};
use egret_audit::governance::{
    action, family_of, governance_changes_via, record_change, record_change_by, Actor, Change,
    Family, GOVERNANCE_ACTIONS,
};
use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

async fn db() -> Db {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

fn principal(impersonator: Option<Uuid>) -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(Uuid::new_v4()),
        email: "steward@x".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: impersonator.map(|id| egret_core::context::Impersonator {
            user_id: egret_core::UserId(id),
            email: "admin@x".into(),
            workspace_scope: None,
            expires_at: Utc::now(),
        }),
    }
}

#[derive(Debug, sqlx::FromRow)]
struct Stored {
    kind: String,
    actor_user_id: Option<Uuid>,
    impersonator: Option<Uuid>,
    workspace_id: Option<Uuid>,
    action: String,
    resource_type: String,
    resource_id: Option<String>,
    detail: Value,
}

async fn rows_for(db: &Db, resource_id: &str) -> Vec<Stored> {
    sqlx::query_as(
        "SELECT kind, actor_user_id, impersonator, workspace_id, action, resource_type, \
                resource_id, detail \
           FROM audit.audit_log WHERE resource_id = $1 ORDER BY id",
    )
    .bind(resource_id)
    .fetch_all(&db.pool)
    .await
    .expect("read audit rows")
}

#[tokio::test]
async fn a_rolled_back_change_leaves_nothing_and_a_committed_one_leaves_one_row() {
    let db = db().await;
    let p = principal(None);
    let rid = Uuid::new_v4().to_string();
    let summary = json!({"before": null, "after": {"catalog": "ice", "actions": ["read"]}});

    let mut tx = db.pool.begin().await.unwrap();
    record_change(
        &mut tx,
        Change {
            action: action::GRANT_CREATE,
            resource_type: "perm_grant",
            resource_id: rid.clone(),
            summary: summary.clone(),
            principal: &p,
        },
    )
    .await
    .expect("record");
    tx.rollback().await.unwrap();
    assert!(
        rows_for(&db, &rid).await.is_empty(),
        "a rollback must take the event with it"
    );

    let mut tx = db.pool.begin().await.unwrap();
    record_change(
        &mut tx,
        Change {
            action: action::GRANT_CREATE,
            resource_type: "perm_grant",
            resource_id: rid.clone(),
            summary: summary.clone(),
            principal: &p,
        },
    )
    .await
    .expect("record");
    tx.commit().await.unwrap();

    let rows = rows_for(&db, &rid).await;
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.kind, "mutation");
    assert_eq!(r.action, "perm_grant:create");
    assert_eq!(r.resource_type, "perm_grant");
    assert_eq!(r.resource_id.as_deref(), Some(rid.as_str()));
    assert_eq!(r.actor_user_id, Some(p.user_id.0));
    assert_eq!(r.impersonator, None);
    assert_eq!(r.workspace_id, None);
    assert_eq!(r.detail, summary);
}

#[tokio::test]
async fn an_impersonated_change_stamps_both_parties() {
    let db = db().await;
    let admin = Uuid::new_v4();
    let p = principal(Some(admin));
    let rid = Uuid::new_v4().to_string();
    let mut conn = db.pool.acquire().await.unwrap();
    record_change(
        &mut conn,
        Change {
            action: action::RULE_DELETE,
            resource_type: "perm_rule",
            resource_id: rid.clone(),
            summary: json!({"before": {"name": "r"}, "after": null}),
            principal: &p,
        },
    )
    .await
    .expect("record");
    let rows = rows_for(&db, &rid).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_user_id, Some(p.user_id.0));
    assert_eq!(rows[0].impersonator, Some(admin));
}

#[tokio::test]
async fn a_summary_over_16_kib_is_stored_as_its_size() {
    let db = db().await;
    let p = principal(None);
    let rid = Uuid::new_v4().to_string();
    let big = json!({"before": null, "after": {"blob": "x".repeat(20 * 1024)}});
    let bytes = serde_json::to_vec(&big).unwrap().len();
    let mut conn = db.pool.acquire().await.unwrap();
    record_change(
        &mut conn,
        Change {
            action: action::RANGER_POLICY_CREATE,
            resource_type: "ranger_policy",
            resource_id: rid.clone(),
            summary: big,
            principal: &p,
        },
    )
    .await
    .expect("record");
    let rows = rows_for(&db, &rid).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail, json!({"truncated": true, "bytes": bytes}));
}

/// Decision O3 (user, 2026-10-18): `audit.enabled = false` turns off the
/// best-effort sink and the PDP decorator, never governance events.
#[tokio::test]
async fn an_event_is_recorded_when_audit_is_disabled() {
    egret_audit::governance::configure(false);
    let db = db().await;
    let p = principal(None);
    let rid = Uuid::new_v4().to_string();
    let mut conn = db.pool.acquire().await.unwrap();
    record_change(
        &mut conn,
        Change {
            action: action::CLEARANCE_CLEAR,
            resource_type: "tag",
            resource_id: rid.clone(),
            summary: json!({"before": {"tag_key": "pii"}, "after": null}),
            principal: &p,
        },
    )
    .await
    .expect("record");
    assert_eq!(rows_for(&db, &rid).await.len(), 1);
}

/// Every constant with the resource type its write records under. Listed by
/// hand: a constant added to `action` but not here (or not to
/// `GOVERNANCE_ACTIONS`) fails `governance_actions_lists_every_constant_once`.
const EXPECTED: &[(&str, &str, Family)] = &[
    (action::GRANT_CREATE, "perm_grant", Family::Policy),
    (action::GRANT_UPDATE, "perm_grant", Family::Policy),
    (action::GRANT_DELETE, "perm_grant", Family::Policy),
    (action::RULE_CREATE, "perm_rule", Family::Policy),
    (action::RULE_UPDATE, "perm_rule", Family::Policy),
    (action::RULE_DELETE, "perm_rule", Family::Policy),
    (action::CLEARANCE_SET, "tag", Family::Tag),
    (action::CLEARANCE_CLEAR, "tag", Family::Tag),
    (
        action::RANGER_POLICY_CREATE,
        "ranger_policy",
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_UPDATE,
        "ranger_policy",
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_DELETE,
        "ranger_policy",
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_ENABLE,
        "ranger_policy",
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_DISABLE,
        "ranger_policy",
        Family::Policy,
    ),
    (action::MIGRATION_APPLY, "policy_profile", Family::Policy),
    (action::MIGRATION_UNDO, "policy_profile", Family::Policy),
    (action::PROFILE_COMPILE, "policy_profile", Family::Policy),
    (action::ENGINE_GRANT_CREATE, "engine_grant", Family::Policy),
    (action::ENGINE_GRANT_UPDATE, "engine_grant", Family::Policy),
    (action::ENGINE_GRANT_DELETE, "engine_grant", Family::Policy),
    (action::USER_DELETE, "user", Family::User),
    (action::CATALOG_TAG_MOVE, "catalog", Family::Tag),
    (action::CATALOG_TAG_PURGE, "catalog", Family::Tag),
    (action::GRANT_MOVE, "perm_grant", Family::Policy),
    (action::GRANT_COPY, "perm_grant", Family::Policy),
    (action::RULE_COPY, "perm_rule", Family::Policy),
    (action::GRANT_LEFT, "perm_grant", Family::Policy),
    (action::RANGER_POLICY_LEFT, "ranger_policy", Family::Policy),
    (action::CATALOG_GRANT_MOVE, "catalog_grant", Family::Catalog),
    (
        action::CATALOG_GRANT_DELETE,
        "catalog_grant",
        Family::Catalog,
    ),
];

#[test]
fn every_action_maps_to_its_family() {
    for (a, rt, fam) in EXPECTED {
        assert_eq!(family_of(a, rt), Some(*fam), "{a} on {rt}");
    }
    assert_eq!(family_of("tag:assign", "catalog"), Some(Family::Tag));
    assert_eq!(family_of("tag:unassign", "catalog"), Some(Family::Tag));
    assert_eq!(
        family_of("tag:assign", "object"),
        None,
        "a BI object tag is not governance"
    );
    assert_eq!(family_of("role:assign", "grant"), Some(Family::Policy));
    assert_eq!(family_of("role:unassign", "grant"), Some(Family::Policy));
    assert_eq!(family_of("dashboard:certify", "dashboard"), None);
    assert_eq!(family_of("dashboard:certify", ""), None);
}

#[test]
fn governance_actions_lists_every_constant_once() {
    let mut listed: Vec<&str> = GOVERNANCE_ACTIONS.to_vec();
    listed.sort_unstable();
    let before = listed.len();
    listed.dedup();
    assert_eq!(listed.len(), before, "a constant is listed twice");
    let mut expected: Vec<&str> = EXPECTED.iter().map(|(a, _, _)| *a).collect();
    expected.sort_unstable();
    assert_eq!(listed, expected);
}

/// Insert a row as-is (a decision, a legacy sink event, or a governance event
/// at a chosen `ts`), returning its id.
async fn raw(
    db: &Db,
    kind: &str,
    action: &str,
    resource_type: &str,
    resource_id: &str,
    ts: DateTime<Utc>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO audit.audit_log (ts, kind, action, resource_type, resource_id, decision) \
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $2 = 'decision' THEN 'allow' END) RETURNING id",
    )
    .bind(ts)
    .bind(kind)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .fetch_one(&db.pool)
    .await
    .expect("insert raw audit row")
}

/// A random instant in 2001, so no other test's `now()` rows sit between a
/// test's own rows and its cursor.
fn private_instant() -> DateTime<Utc> {
    let secs = (Uuid::new_v4().as_u128() % (300 * 24 * 3600)) as i64;
    Utc.with_ymd_and_hms(2001, 1, 1, 0, 0, 0).unwrap() + Duration::seconds(secs)
}

#[tokio::test]
async fn the_feed_returns_only_governance_pairs_the_families_allow() {
    let db = db().await;
    let t = private_instant();
    let rid = Uuid::new_v4().to_string();
    let grant = raw(&db, "mutation", action::GRANT_CREATE, "perm_grant", &rid, t).await;
    let clear = raw(&db, "mutation", action::CLEARANCE_SET, "tag", &rid, t).await;
    let catalog_tag = raw(&db, "mutation", "tag:assign", "catalog", &rid, t).await;
    let role = raw(&db, "mutation", "role:assign", "grant", &rid, t).await;
    let _object_tag = raw(&db, "mutation", "tag:assign", "object", &rid, t).await;
    let _decision = raw(&db, "decision", action::GRANT_CREATE, "perm_grant", &rid, t).await;
    let _other = raw(&db, "mutation", "dashboard:certify", "dashboard", &rid, t).await;
    let cursor = Some((t + Duration::seconds(1), i64::MAX));

    let mine = |rows: Vec<egret_audit::governance::ChangeRow>| -> Vec<i64> {
        let mut ids: Vec<i64> = rows
            .into_iter()
            .filter(|r| r.resource_id.as_deref() == Some(rid.as_str()))
            .map(|r| r.id)
            .collect();
        ids.sort_unstable();
        ids
    };
    let mut conn = db.pool.acquire().await.unwrap();

    let both = governance_changes_via(&mut conn, &[Family::Policy, Family::Tag], cursor, 100)
        .await
        .unwrap();
    let mut want = vec![grant, clear, catalog_tag, role];
    want.sort_unstable();
    assert_eq!(mine(both), want);

    let tag_only = governance_changes_via(&mut conn, &[Family::Tag], cursor, 100)
        .await
        .unwrap();
    assert!(tag_only
        .iter()
        .all(|r| !r.action.starts_with("perm_grant:")));
    let mut want = vec![clear, catalog_tag];
    want.sort_unstable();
    assert_eq!(mine(tag_only), want);

    let policy_only = governance_changes_via(&mut conn, &[Family::Policy], cursor, 100)
        .await
        .unwrap();
    let mut want = vec![grant, role];
    want.sort_unstable();
    assert_eq!(mine(policy_only), want);

    let none = governance_changes_via(&mut conn, &[], cursor, 100)
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn the_feed_pages_newest_first_with_no_overlap_and_no_gap() {
    let db = db().await;
    let t = private_instant();
    let rid = Uuid::new_v4().to_string();
    // Three rows share `ts`; two are older.
    let a = raw(&db, "mutation", action::GRANT_CREATE, "perm_grant", &rid, t).await;
    let b = raw(&db, "mutation", action::GRANT_UPDATE, "perm_grant", &rid, t).await;
    let c = raw(&db, "mutation", action::GRANT_DELETE, "perm_grant", &rid, t).await;
    let d = raw(
        &db,
        "mutation",
        action::RULE_CREATE,
        "perm_rule",
        &rid,
        t - Duration::seconds(1),
    )
    .await;
    let e = raw(
        &db,
        "mutation",
        action::RULE_DELETE,
        "perm_rule",
        &rid,
        t - Duration::seconds(2),
    )
    .await;
    let mut conn = db.pool.acquire().await.unwrap();

    let page1 = governance_changes_via(
        &mut conn,
        &[Family::Policy],
        Some((t + Duration::microseconds(1), i64::MAX)),
        2,
    )
    .await
    .unwrap();
    assert_eq!(page1.iter().map(|r| r.id).collect::<Vec<_>>(), vec![c, b]);
    let last = page1.last().unwrap();
    let page2 = governance_changes_via(&mut conn, &[Family::Policy], Some((last.ts, last.id)), 2)
        .await
        .unwrap();
    assert_eq!(page2.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a, d]);
    let last = page2.last().unwrap();
    let page3 = governance_changes_via(&mut conn, &[Family::Policy], Some((last.ts, last.id)), 2)
        .await
        .unwrap();
    assert_eq!(page3.first().map(|r| r.id), Some(e));
    assert!(page3.iter().all(|r| r.ts < t - Duration::seconds(1)));
}

#[tokio::test]
async fn compile_runs_refuses_an_error_without_its_text_and_cascades_with_the_profile() {
    let db = db().await;
    let def = format!("audit_t_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let def_id = (Uuid::new_v4().as_u128() % 1_000_000_000) as i64 + 1_000_000_000;
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1, $2, '{}')",
    )
    .bind(def_id)
    .bind(&def)
    .execute(&db.pool)
    .await
    .unwrap();
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type) VALUES ($1, $1) RETURNING id",
    )
    .bind(&def)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let refused = sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, error) \
         VALUES ($1, 'error', now(), NULL)",
    )
    .bind(sid)
    .execute(&db.pool)
    .await;
    let err = refused.expect_err("an error row with no error text must be refused");
    assert!(
        err.to_string().contains("compile_runs_error_says_why"),
        "{err}"
    );

    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, last_ok_at, created) \
         VALUES ($1, 'ok', now(), now(), 1)",
    )
    .bind(sid)
    .execute(&db.pool)
    .await
    .expect("an ok row");
    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(sid)
        .execute(&db.pool)
        .await
        .unwrap();
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM perm.compile_runs WHERE service_id = $1")
            .bind(sid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(left, 0, "deleting the profile deletes its compile run");
}

#[tokio::test]
async fn the_mutation_feed_index_is_partial_on_mutations() {
    let db = db().await;
    let def: Option<String> = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes \
          WHERE schemaname = 'audit' AND indexname = 'audit_log_mutation_ts_idx'",
    )
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    let def = def.expect("audit_log_mutation_ts_idx exists");
    assert!(def.contains("WHERE (kind = 'mutation'::text)"), "{def}");
    assert!(def.contains("ts DESC, id DESC"), "{def}");
}

/// Rename spec C6a: a catalog client is no user. Its event leaves
/// `actor_user_id` NULL and names the client in `detail.actor`; an event by
/// egret itself names nobody.
#[tokio::test]
async fn a_catalog_client_is_recorded_in_the_detail_and_egret_as_nobody() {
    let db = db().await;
    let client = Uuid::new_v4();
    let rid = format!("{}:sales.orders", Uuid::new_v4());
    let mut conn = db.pool.acquire().await.unwrap();
    record_change_by(
        &mut conn,
        Actor::CatalogClient {
            id: Some(client),
            client_id: "trino-main-7f3a".into(),
            name: Some("trino-main".into()),
            consumer_type: Some("engine".into()),
        },
        action::CATALOG_TAG_MOVE,
        "catalog",
        rid.clone(),
        json!({"rows": 2}),
    )
    .await
    .expect("record");
    record_change_by(
        &mut conn,
        Actor::Egret,
        action::CATALOG_TAG_PURGE,
        "catalog",
        rid.clone(),
        json!({"rows": 1}),
    )
    .await
    .expect("record");

    let rows = rows_for(&db, &rid).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].actor_user_id, None);
    assert_eq!(
        rows[0].detail,
        json!({"rows": 2, "actor": {
            "kind": "catalog_client", "id": client, "client_id": "trino-main-7f3a",
            "name": "trino-main", "consumer_type": "engine"}})
    );
    assert_eq!(rows[1].actor_user_id, None);
    assert_eq!(rows[1].detail, json!({"rows": 1}));
}
