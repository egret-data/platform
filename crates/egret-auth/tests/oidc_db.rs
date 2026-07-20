//! Tier-0 OIDC DB tests (no IdP). Self-skip without DATABASE_URL.

use egret_auth::groups::GroupRepo;
use egret_auth::oidc::{provision_and_sync, OidcClaims};
use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

fn claims(sub: &str, email: &str, groups: &[&str]) -> OidcClaims {
    OidcClaims {
        subject: sub.into(),
        email: email.into(),
        name: "T".into(),
        groups: groups.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn insert_and_find_oidc_user_by_subject() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let sub = uniq("sub");
    let email = format!("{}@idp.local", uniq("u"));
    let id = users.insert_oidc(&sub, &email, "Tester").await.unwrap();
    let found = users
        .find_by_external_subject(&sub)
        .await
        .unwrap()
        .expect("found");
    assert_eq!(found.id, id);
    assert_eq!(found.provider, "oidc");
}

#[tokio::test]
async fn group_sync_replaces_oidc_keeps_local() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let id = users
        .insert_oidc(&uniq("sub"), &format!("{}@idp.local", uniq("u")), "T")
        .await
        .unwrap();

    // a manual (local) group membership that must survive sync
    let local_gid: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(uniq("local-grp"))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(id.0)
        .bind(local_gid)
        .execute(&db.pool)
        .await
        .unwrap();

    // first sync: [a, b]
    let a = uniq("g");
    let b = uniq("g");
    groups.upsert_oidc_group(&a).await.unwrap();
    groups.upsert_oidc_group(&b).await.unwrap();
    groups
        .sync_user_oidc_groups(id, &[a.clone(), b.clone()])
        .await
        .unwrap();

    // second sync: [a] only → b removed, local kept
    groups
        .sync_user_oidc_groups(id, std::slice::from_ref(&a))
        .await
        .unwrap();

    let names: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id WHERE ug.user_id = $1",
    )
    .bind(id.0)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(names.contains(&a));
    assert!(!names.contains(&b), "oidc group b should be revoked");
    assert_eq!(
        names.iter().filter(|n| n.starts_with("local-grp")).count(),
        1,
        "local membership kept"
    );
}

#[tokio::test]
async fn provision_creates_then_updates_same_user() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let sub = uniq("sub");
    let g = uniq("g");
    let e1 = format!("{}@idp.local", uniq("u"));

    let id1 = provision_and_sync(&users, &groups, &claims(&sub, &e1, &[&g]))
        .await
        .unwrap();
    // second login, same sub, new email → same id, profile updated
    let e2 = format!("{}@idp.local", uniq("u"));
    let id2 = provision_and_sync(&users, &groups, &claims(&sub, &e2, &[&g]))
        .await
        .unwrap();
    assert_eq!(id1, id2);
    let u = users.find_by_id(id1).await.unwrap().unwrap();
    assert_eq!(u.email.to_lowercase(), e2.to_lowercase());
}

/// Regression (Task 5, STS token broker): the egret-minted engine token's `groups`
/// claim is sourced from `GroupRepo::groups_of_user`, which reads `user_groups`.
/// This proves the OIDC login callback path (`provision_and_sync`) JIT-writes the
/// IdP's `groups` claim into `user_groups` — including creating brand-new group
/// rows on first sight — so a freshly-logged-in SSO user's engine token carries
/// their current IdP groups.
#[tokio::test]
async fn provision_and_sync_writes_through_to_user_groups() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());

    let sub = uniq("sub");
    let email = format!("{}@idp.local", uniq("u"));
    let g1 = uniq("data-team");
    let g2 = uniq("analysts");

    let user_id = provision_and_sync(&users, &groups, &claims(&sub, &email, &[&g1, &g2]))
        .await
        .unwrap();

    let mut names = groups.groups_of_user(user_id).await.unwrap();
    names.sort();
    let mut expected = vec![g1.clone(), g2.clone()];
    expected.sort();
    assert_eq!(
        names, expected,
        "user_groups (read via groups_of_user, the same source the engine-token \
         minter uses) must contain exactly the OIDC groups claim"
    );
}

#[tokio::test]
async fn provision_email_collision_with_other_account_conflicts() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let email = format!("{}@idp.local", uniq("u"));
    // a pre-existing local user owns this email; use raw SQL to avoid hash validation
    sqlx::query("INSERT INTO users (provider, email, name) VALUES ('local', $1, 'L')")
        .bind(&email)
        .execute(&db.pool)
        .await
        .unwrap();
    let err = provision_and_sync(&users, &groups, &claims(&uniq("sub"), &email, &[])).await;
    assert!(matches!(err, Err(egret_core::Error::Conflict(_))));
}
