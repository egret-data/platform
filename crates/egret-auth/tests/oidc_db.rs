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
        email_verified: true,
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

    async fn group_references_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        _names: &[String],
    ) -> egret_core::Result<Vec<egret_auth::groups::GroupReference>> {
        Ok(Vec::new())
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

/// A `source = 'local'` group, created the way the console's create does.
async fn local_group(groups: &GroupRepo, tag: &str) -> (String, uuid::Uuid) {
    let name = uniq(tag);
    let id = groups
        .create_local(&name, &Recorder::default())
        .await
        .unwrap();
    (name, id)
}

async fn names_of(db: &Db, user: egret_core::UserId) -> Vec<String> {
    let mut names: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id WHERE ug.user_id = $1",
    )
    .bind(user.0)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    names.sort();
    names
}

/// R1: the sync manages memberships in OIDC-sourced groups only. An IdP claim
/// that happens to name a LOCAL group must not make the user a member — the
/// path by which an IdP group name used to reach a local group's roles — and,
/// since nothing moved, must neither lock nor recompile anything.
#[tokio::test]
async fn a_claim_naming_a_local_group_joins_nothing() {
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
    let (local, _) = local_group(&groups, "local-grp").await;

    groups
        .sync_user_oidc_groups(id, std::slice::from_ref(&local), &rc)
        .await
        .unwrap();

    assert_eq!(
        names_of(&db, id).await,
        Vec::<String>::new(),
        "a claim naming a local group must not add a membership"
    );
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "nothing moved, so nothing is locked or recompiled"
    );
}

/// R1, the other half: a local membership the user already holds is not the
/// sync's to manage. Claiming it moves nothing, and a later login WITHOUT the
/// claim keeps it — the sync never deletes a local row.
#[tokio::test]
async fn a_local_membership_survives_claiming_it_and_dropping_the_claim() {
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
    let (local, local_gid) = local_group(&groups, "local-grp").await;
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(id.0)
        .bind(local_gid)
        .execute(&db.pool)
        .await
        .unwrap();

    groups
        .sync_user_oidc_groups(id, std::slice::from_ref(&local), &rc)
        .await
        .unwrap();
    assert_eq!(names_of(&db, id).await, vec![local.clone()]);
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "claiming a held local group moves nothing"
    );

    groups.sync_user_oidc_groups(id, &[], &rc).await.unwrap();
    assert_eq!(
        names_of(&db, id).await,
        vec![local.clone()],
        "dropping the claim must not remove a local membership"
    );
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "dropping it moves nothing either"
    );
}

/// R1 + R3 on a mixed claim: the OIDC name is joined and is the only thing
/// moved (so the only thing locked and recompiled); the local name is ignored.
#[tokio::test]
async fn a_mixed_claim_joins_and_moves_only_the_oidc_group() {
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
    let oidc_a = uniq("g");
    groups.upsert_oidc_group(&oidc_a).await.unwrap();
    let (local_b, _) = local_group(&groups, "local-grp").await;

    groups
        .sync_user_oidc_groups(id, &[oidc_a.clone(), local_b.clone()], &rc)
        .await
        .unwrap();

    assert_eq!(
        names_of(&db, id).await,
        vec![oidc_a.clone()],
        "only the OIDC group is joined"
    );
    assert_eq!(rc.take(), vec![Call::Lock(vec![oidc_a]), Call::Recompile]);
}

/// R2: `upsert_oidc_group` never flips a local group to `oidc`, so the login
/// path end to end leaves a claimed local group local and un-joined.
#[tokio::test]
async fn provision_and_sync_leaves_a_claimed_local_group_local_and_unjoined() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let rc = Recorder::default();
    let (local, local_gid) = local_group(&groups, "local-grp").await;

    let user_id = provision_and_sync(
        &users,
        &groups,
        &claims(&uniq("sub"), &format!("{}@idp.local", uniq("u")), &[&local]),
        &rc,
    )
    .await
    .unwrap();

    let source: String = sqlx::query_scalar("SELECT source FROM groups WHERE id = $1")
        .bind(local_gid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        source, "local",
        "the login must not flip the group's source"
    );
    assert_eq!(
        names_of(&db, user_id).await,
        Vec::<String>::new(),
        "and must not make the user a member"
    );
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "nothing moved, so nothing is locked or recompiled"
    );
}

/// R2's input: the login warns once per claimed LOCAL group and says whether
/// the user already holds it. OIDC names and names with no group are not
/// reported.
#[tokio::test]
async fn claimed_local_groups_reports_local_names_with_membership() {
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
    let (held, held_gid) = local_group(&groups, "local-a").await;
    let (not_held, _) = local_group(&groups, "local-b").await;
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(id.0)
        .bind(held_gid)
        .execute(&db.pool)
        .await
        .unwrap();
    let oidc = uniq("g");
    groups.upsert_oidc_group(&oidc).await.unwrap();

    let got = groups
        .claimed_local_groups(
            id,
            &[oidc, held.clone(), not_held.clone(), uniq("no-such-group")],
        )
        .await
        .unwrap();

    assert_eq!(got, vec![(held, true), (not_held, false)]);
}

/// 2026-10-18 design R2, this crate's half: a claimed name with no `groups`
/// row is created and joined only when the port says nothing in the model
/// names it. The one the port reports is refused — no row, no membership —
/// and the login still succeeds; the other claimed name is created and
/// joined as before. A referenced name that already HAS a row is never asked
/// about, so it is joined as before too. (The real port, and the compiled
/// policies, are in `egret-perm-compile/tests/group_grants.rs`.)
#[tokio::test]
async fn a_login_does_not_create_a_group_the_model_still_names() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let dangling = uniq("dangling");
    let fresh = uniq("fresh");
    let existing = uniq("existing");
    groups.upsert_oidc_group(&existing).await.unwrap();
    let reference = |name: &str| egret_auth::groups::GroupReference {
        group_name: name.to_string(),
        source: "grant".into(),
        id: uuid::Uuid::new_v4(),
        rule_name: None,
        effect: Some("allow".into()),
        position: "include".into(),
    };
    let rc = Recorder::with_references(vec![reference(&dangling), reference(&existing)]);

    let refused = groups
        .upsert_claimed_oidc_groups(&[dangling.clone(), fresh.clone(), existing.clone()], &rc)
        .await
        .unwrap();
    assert_eq!(refused, vec![dangling.clone()]);

    let user_id = provision_and_sync(
        &users,
        &groups,
        &claims(
            &uniq("sub"),
            &format!("{}@idp.local", uniq("u")),
            &[&dangling, &fresh, &existing],
        ),
        &rc,
    )
    .await
    .expect("a refused name does not refuse the login");

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM groups WHERE name = $1")
        .bind(&dangling)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "no `groups` row for a name the model still names");
    let mut expected = vec![existing, fresh];
    expected.sort();
    assert_eq!(names_of(&db, user_id).await, expected);
}

/// a-i: a member who has not logged in for the window loses their OIDC group
/// memberships, the drop recompiles what it moved, and a local membership and a
/// recent member are untouched. The IdP change is only seen at login, so this is the
/// only way a removed or renamed IdP group stops reaching a member who never returns.
#[tokio::test]
async fn a_member_who_has_not_logged_in_for_the_window_loses_only_their_oidc_groups() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let rc = Recorder::default();
    let oidc_group = uniq("idp-grp");

    let stale = provision_and_sync(
        &users,
        &groups,
        &claims(
            &uniq("sub"),
            &format!("{}@idp.local", uniq("s")),
            &[&oidc_group],
        ),
        &rc,
    )
    .await
    .unwrap();
    let recent = provision_and_sync(
        &users,
        &groups,
        &claims(
            &uniq("sub"),
            &format!("{}@idp.local", uniq("r")),
            &[&oidc_group],
        ),
        &rc,
    )
    .await
    .unwrap();
    let (local, local_gid) = local_group(&groups, "local-grp").await;
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(stale.0)
        .bind(local_gid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET last_login_at = now() - interval '8 days' WHERE id = $1")
        .bind(stale.0)
        .execute(&db.pool)
        .await
        .unwrap();
    rc.take();

    let dropped = groups
        .drop_stale_oidc_memberships(std::time::Duration::from_secs(7 * 24 * 3600), &rc)
        .await
        .unwrap();

    assert!(dropped >= 1, "the stale member is dropped");
    assert_eq!(
        names_of(&db, stale).await,
        vec![local.clone()],
        "the stale member keeps only the local membership"
    );
    assert_eq!(
        names_of(&db, recent).await,
        vec![oidc_group.clone()],
        "a member who logged in recently keeps the OIDC group"
    );
    assert!(
        rc.take()
            .iter()
            .any(|c| matches!(c, Call::Lock(names) if names.contains(&oidc_group))),
        "dropping the stale member's OIDC group recompiles what names it"
    );
}
