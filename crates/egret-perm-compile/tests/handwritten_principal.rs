//! DG #4b route B (spec §4.3, Q1): hand-written Ranger policies that name a
//! principal, read by `handwritten::handwritten_for_principal_via`.
//!
//! A user's own username is `direct`; a group or role of the user named by an
//! item is listed with `onEngines: false` (engines resolve neither, Task 1 M3);
//! `public` and `{USER}` are `everyone` only. Fixtures use unique names.
//!
//! DB-gated; FAILS without a database:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test handwritten_principal

use egret_metastore::Db;
use egret_perm_compile::handwritten::{
    handwritten_for_principal_via, Bucket, HandWrittenError, NamingHow, PrincipalHandWritten,
    PrincipalPolicy, PrincipalState,
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

/// A user with `username`, a member of a new group and holder of a new role.
struct People {
    user: Uuid,
    username: String,
    group: String,
    role: String,
}

async fn people(e: &Env, with_username: bool) -> People {
    let username = unique("u");
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, 'Hw P', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("hwp")))
    .bind(with_username.then_some(&username))
    .fetch_one(&e.pool)
    .await
    .unwrap();
    let group = unique("g");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(&e.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user)
        .bind(gid)
        .execute(&e.pool)
        .await
        .unwrap();
    let role = unique("r");
    let rid: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role)
            .fetch_one(&e.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(user)
    .bind(rid)
    .execute(&e.pool)
    .await
    .unwrap();
    People {
        user,
        username,
        group,
        role,
    }
}

async fn profile(e: &Env) -> (i64, String) {
    let name = unique("hwp-tr-");
    let id = e
        .policy
        .create_service(&name, "trino", &json!({}))
        .await
        .unwrap()
        .id;
    (id, name)
}

async fn insert(
    e: &Env,
    service: i64,
    name: &str,
    item: Value,
    bucket: &str,
    enabled: bool,
    by: &str,
) {
    let body = json!({
        "name": name, "service": "x", "policyType": 0, "isEnabled": true,
        "resources": { "catalog": { "values": ["c"] }, "schema": { "values": ["*"] },
                       "table": { "values": ["*"] }, "column": { "values": ["*"] } },
        bucket: [item]
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(service)
    .bind(name)
    .bind(body)
    .bind(enabled)
    .bind(by)
    .execute(&e.pool)
    .await
    .unwrap();
}

fn item(users: &[&str], groups: &[&str], roles: &[&str]) -> Value {
    json!({ "users": users, "groups": groups, "roles": roles,
            "accesses": [{ "type": "select", "isAllowed": true }] })
}

async fn read(e: &Env, kind: &str, id: &str) -> Result<PrincipalHandWritten, HandWrittenError> {
    let mut conn = e.pool.acquire().await.unwrap();
    handwritten_for_principal_via(&mut conn, kind, id).await
}

fn row<'a>(r: &'a PrincipalHandWritten, name: &str) -> Option<&'a PrincipalPolicy> {
    r.policies.iter().find(|p| p.name == name)
}

#[tokio::test]
async fn a_user_page_marks_direct_group_and_role_items() {
    let e = env().await;
    let p = people(&e, true).await;
    let (sid, sname) = profile(&e).await;
    let d = unique("direct-");
    let g = unique("by-group-");
    let r = unique("by-role-");
    let off = unique("off-");
    insert(
        &e,
        sid,
        &d,
        item(&[&p.username], &[], &[]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &g,
        item(&[], &[&p.group], &[]),
        "denyPolicyItems",
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &r,
        item(&[], &[], &[&p.role]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &off,
        item(&[&p.username], &[], &[]),
        "policyItems",
        false,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &unique("perm-grant-"),
        item(&[&p.username], &[], &[]),
        "policyItems",
        true,
        "compiler",
    )
    .await;
    insert(
        &e,
        sid,
        &unique("seed-"),
        item(&[&p.username], &[], &[]),
        "policyItems",
        true,
        "infra",
    )
    .await;

    let out = read(&e, "user", &p.user.to_string()).await.unwrap();
    assert_eq!(out.state, PrincipalState::Listed);
    assert_eq!(out.principal.username.as_deref(), Some(p.username.as_str()));
    assert_eq!(
        out.policies.len(),
        4,
        "{:?}",
        out.policies.iter().map(|p| &p.name).collect::<Vec<_>>()
    );

    let n = &row(&out, &d).unwrap().naming;
    assert_eq!(n.len(), 1);
    assert_eq!(
        (n[0].how, n[0].via.as_deref(), n[0].on_engines, n[0].bucket),
        (NamingHow::Direct, None, true, Bucket::Allow)
    );
    let n = &row(&out, &g).unwrap().naming;
    assert_eq!(
        (n[0].how, n[0].via.as_deref(), n[0].on_engines, n[0].bucket),
        (
            NamingHow::Group,
            Some(p.group.as_str()),
            false,
            Bucket::Deny
        )
    );
    let n = &row(&out, &r).unwrap().naming;
    assert_eq!(
        (n[0].how, n[0].via.as_deref(), n[0].on_engines),
        (NamingHow::Role, Some(p.role.as_str()), false)
    );
    let off_row = row(&out, &off).unwrap();
    assert!(!off_row.enabled);
    assert_eq!(off_row.service, sname);
    assert!(out.everyone.policies.iter().all(|x| x.service != sname));
}

#[tokio::test]
async fn public_and_any_user_are_everyone_only() {
    let e = env().await;
    let p = people(&e, true).await;
    let (sid, _) = profile(&e).await;
    let pubn = unique("public-");
    let anyn = unique("anyuser-");
    insert(
        &e,
        sid,
        &pubn,
        item(&[], &["public"], &[]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &anyn,
        item(&["{USER}"], &[], &[]),
        "denyPolicyItems",
        true,
        "operator",
    )
    .await;
    let out = read(&e, "user", &p.user.to_string()).await.unwrap();
    assert!(row(&out, &pubn).is_none());
    assert!(row(&out, &anyn).is_none());
    let names: Vec<&str> = out
        .everyone
        .policies
        .iter()
        .map(|x| x.name.as_str())
        .collect();
    assert!(names.contains(&pubn.as_str()) && names.contains(&anyn.as_str()));
    let any = out
        .everyone
        .policies
        .iter()
        .find(|x| x.name == anyn)
        .unwrap();
    assert_eq!(any.bucket, Bucket::Deny);
    assert!(out.everyone.count >= 2);
}

#[tokio::test]
async fn a_user_without_username_gets_only_the_everyone_count() {
    let e = env().await;
    let p = people(&e, false).await;
    let (sid, _) = profile(&e).await;
    insert(
        &e,
        sid,
        &unique("by-group-"),
        item(&[], &[&p.group], &[]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    let out = read(&e, "user", &p.user.to_string()).await.unwrap();
    assert_eq!(out.state, PrincipalState::NoUsername);
    assert!(out.policies.is_empty());
    assert_eq!(out.principal.username, None);
}

#[tokio::test]
async fn group_and_role_pages() {
    let e = env().await;
    let p = people(&e, true).await;
    let (sid, _) = profile(&e).await;
    let g = unique("by-group-");
    let r = unique("by-role-");
    insert(
        &e,
        sid,
        &g,
        item(&[], &[&p.group], &[]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &r,
        item(&[], &[], &[&p.role]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    let out = read(&e, "group", &p.group).await.unwrap();
    assert_eq!(out.principal.kind, "group");
    let n = &row(&out, &g).unwrap().naming;
    assert_eq!((n[0].how, n[0].on_engines), (NamingHow::Group, false));
    assert!(row(&out, &r).is_none());
    let out = read(&e, "role", &p.role).await.unwrap();
    let n = &row(&out, &r).unwrap().naming;
    assert_eq!((n[0].how, n[0].on_engines), (NamingHow::Role, false));
    assert!(row(&out, &g).is_none());
}

#[tokio::test]
async fn refusals() {
    let e = env().await;
    assert!(matches!(
        read(&e, "user", &Uuid::new_v4().to_string()).await,
        Err(HandWrittenError::NotFound)
    ));
    assert!(matches!(
        read(&e, "user", "not-a-uuid").await,
        Err(HandWrittenError::NotFound)
    ));
    assert!(matches!(
        read(&e, "group", &unique("nog")).await,
        Err(HandWrittenError::NotFound)
    ));
    assert!(matches!(
        read(&e, "role", &unique("nor")).await,
        Err(HandWrittenError::NotFound)
    ));
    assert!(matches!(
        read(&e, "workspace", "x").await,
        Err(HandWrittenError::BadRequest(_))
    ));
}

/// egret folds group names (#688), while a hand-written item keeps whatever
/// case its author typed. This listing is informational (engines resolve no
/// groups, Q1), so an item naming `G1` is listed on the page of group `g1` and
/// of its members; the item's own spelling is shown. Role names are NOT folded
/// (`roles` is UNIQUE(scope, name), case-sensitive): an item naming `R1` is
/// not role `r1`.
#[tokio::test]
async fn group_items_match_regardless_of_case_role_items_exactly() {
    let e = env().await;
    let p = people(&e, true).await;
    let (sid, _) = profile(&e).await;
    let g = unique("by-group-upper-");
    let r = unique("by-role-upper-");
    let group_upper = p.group.to_uppercase();
    let role_upper = p.role.to_uppercase();
    assert_ne!(group_upper, p.group, "the fixture name must have a letter");
    assert_ne!(role_upper, p.role, "the fixture name must have a letter");
    insert(
        &e,
        sid,
        &g,
        item(&[], &[&group_upper], &[]),
        "policyItems",
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        sid,
        &r,
        item(&[], &[], &[&role_upper]),
        "policyItems",
        true,
        "operator",
    )
    .await;

    let out = read(&e, "user", &p.user.to_string()).await.unwrap();
    let n = &row(&out, &g)
        .expect("group item listed on the member's page")
        .naming;
    assert_eq!(
        (n[0].how, n[0].via.as_deref(), n[0].on_engines),
        (NamingHow::Group, Some(group_upper.as_str()), false)
    );
    assert!(
        row(&out, &r).is_none(),
        "a role item of another case is another role"
    );

    let out = read(&e, "group", &p.group).await.unwrap();
    assert!(row(&out, &g).is_some(), "listed on the group's own page");
    let out = read(&e, "role", &p.role).await.unwrap();
    assert!(row(&out, &r).is_none(), "not listed on role r1's page");
}

/// A body that cannot be read may name anyone, so the page must not say "no
/// hand-written policy names this principal" as if it had read it: route B
/// lists it as unreadable, with route A's parse error (spec §4.2.6: never
/// dropped). Only `operator` rows: an unreadable seeded row is not ours to list.
#[tokio::test]
async fn an_unreadable_body_is_listed_not_dropped() {
    let e = env().await;
    let p = people(&e, true).await;
    let (sid, sname) = profile(&e).await;
    let bad = unique("bad-");
    let bad_infra = unique("bad-infra-");
    for (name, by) in [(&bad, "operator"), (&bad_infra, "infra")] {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(sid)
        .bind(name)
        .bind(json!({ "name": name, "policyType": 1, "resources": "not a map" }))
        .bind(by)
        .execute(&e.pool)
        .await
        .unwrap();
    }
    for (kind, id) in [
        ("user", p.user.to_string()),
        ("group", p.group.clone()),
        ("role", p.role.clone()),
    ] {
        let out = read(&e, kind, &id).await.unwrap();
        let u = out
            .unreadable
            .iter()
            .find(|u| u.name == bad)
            .unwrap_or_else(|| panic!("{kind} page lists the unreadable policy"));
        assert_eq!(u.service, sname);
        assert!(!u.in_tag_service);
        assert_eq!(u.policy_type, "masking");
        assert!(!u.parse_error.is_empty());
        assert!(out.unreadable.iter().all(|u| u.name != bad_infra));
        assert!(row(&out, &bad).is_none());
    }
}
