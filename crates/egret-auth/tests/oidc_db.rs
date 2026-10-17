//! Tier-0 OIDC DB tests (no IdP). Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.

use egret_auth::groups::GroupRepo;
use egret_auth::oidc::{provision_and_sync, OidcClaims};
use egret_auth::users::UserRepo;
use egret_metastore::Db;

#[path = "support/recorder.rs"]
mod recorder;
use recorder::{Call, Recorder};

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
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
        .sync_user_oidc_groups(id, &[a.clone(), b.clone()], &Recorder::default())
        .await
        .unwrap();

    // second sync: [a] only → b removed, local kept
    groups
        .sync_user_oidc_groups(id, std::slice::from_ref(&a), &Recorder::default())
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

    let id1 = provision_and_sync(
        &users,
        &groups,
        &claims(&sub, &e1, &[&g]),
        &Recorder::default(),
    )
    .await
    .unwrap();
    // second login, same sub, new email → same id, profile updated
    let e2 = format!("{}@idp.local", uniq("u"));
    let id2 = provision_and_sync(
        &users,
        &groups,
        &claims(&sub, &e2, &[&g]),
        &Recorder::default(),
    )
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

    let user_id = provision_and_sync(
        &users,
        &groups,
        &claims(&sub, &email, &[&g1, &g2]),
        &Recorder::default(),
    )
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
    let err = provision_and_sync(
        &users,
        &groups,
        &claims(&uniq("sub"), &email, &[]),
        &Recorder::default(),
    )
    .await;
    assert!(matches!(err, Err(egret_core::Error::Conflict(_))));
}

/// J-F3 Task 1b: the per-login sync recompiles only what the login MOVED. A
/// login whose claim is unchanged — the common case — must not touch the port
/// at all (recompiling on every login would make every login an
/// installation-wide write); one that moves a group names exactly the moved
/// groups, the dropped one included — the REMOVE that fails open if missed.
/// A claimed name with no `groups` row is not a membership, so it moves nothing.
#[tokio::test]
async fn a_login_recompiles_only_the_groups_it_moved() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let rc = Recorder::default();
    let id = users
        .insert_oidc(&uniq("sub"), &format!("{}@idp.local", uniq("u")), "T")
        .await
        .unwrap();
    let a = uniq("g");
    let b = uniq("g");
    let c = uniq("g");
    for g in [&a, &b, &c] {
        groups.upsert_oidc_group(g).await.unwrap();
    }
    let ghost = uniq("no-such-group");

    groups
        .sync_user_oidc_groups(id, &[a.clone(), b.clone(), ghost.clone()], &rc)
        .await
        .unwrap();
    let mut ab = vec![a.clone(), b.clone()];
    ab.sort();
    assert_eq!(rc.take(), vec![Call::Lock(ab), Call::Recompile]);

    groups
        .sync_user_oidc_groups(id, &[b.clone(), a.clone(), ghost.clone()], &rc)
        .await
        .unwrap();
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "an unchanged claim must not lock or recompile anything"
    );

    groups
        .sync_user_oidc_groups(id, &[a.clone(), c.clone()], &rc)
        .await
        .unwrap();
    let mut bc = vec![b.clone(), c.clone()];
    bc.sort();
    assert_eq!(
        rc.take(),
        vec![Call::Lock(bc), Call::Recompile],
        "dropping b and gaining c moves exactly those two"
    );
}

/// Creates `late` (on its own connection, committed) the moment the sync asks
/// for its locks — after the sync read which claimed names name a group, and
/// before its INSERT. Records the names it was asked to lock.
struct CreatesGroupMidSync {
    pool: sqlx::PgPool,
    late: String,
    locked: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl egret_auth::groups::GroupMembershipRecompiler for CreatesGroupMidSync {
    async fn lock_affected_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        group_names: &[String],
    ) -> egret_core::Result<Vec<i64>> {
        self.locked
            .lock()
            .unwrap()
            .extend(group_names.iter().cloned());
        sqlx::query("INSERT INTO groups (name, source) VALUES ($1, 'oidc')")
            .bind(&self.late)
            .execute(&self.pool)
            .await
            .expect("create the late group");
        Ok(Vec::new())
    }

    async fn recompile_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        _service_ids: &[i64],
    ) -> egret_core::Result<()> {
        Ok(())
    }
}

/// J-F3 final review I2: the sync must insert only the memberships it locked
/// and recompiles. A claimed name whose group is created after the sync read
/// the claimed set — so it was never locked and nothing naming it was
/// recompiled — must not become a membership; a stored deny or allow-exclude
/// naming it would otherwise keep compiling to nobody while it has a member.
/// The next login, which sees the group, adds it with a lock and a recompile.
#[tokio::test]
async fn a_group_created_mid_sync_is_not_joined_without_its_lock() {
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
    let known = uniq("g");
    groups.upsert_oidc_group(&known).await.unwrap();
    let late = uniq("late");
    let rc = CreatesGroupMidSync {
        pool: db.pool.clone(),
        late: late.clone(),
        locked: std::sync::Mutex::new(Vec::new()),
    };

    groups
        .sync_user_oidc_groups(id, &[known.clone(), late.clone()], &rc)
        .await
        .unwrap();

    let held: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
          WHERE ug.user_id = $1 ORDER BY g.name",
    )
    .bind(id.0)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let locked = rc.locked.lock().unwrap().clone();
    assert_eq!(
        locked,
        vec![known.clone()],
        "only the known group was locked"
    );
    assert_eq!(held, vec![known], "every membership inserted was locked");

    // The next login sees the group: joined, and locked first.
    let rc2 = Recorder::default();
    groups
        .sync_user_oidc_groups(id, std::slice::from_ref(&late), &rc2)
        .await
        .unwrap();
    let calls = rc2.take();
    assert!(
        matches!(calls.first(), Some(Call::Lock(names)) if names.contains(&late)),
        "{calls:?}"
    );
}
