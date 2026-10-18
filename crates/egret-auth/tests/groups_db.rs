//! DB-gated `GroupRepo`/`UserRepo` tests. Self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS; CI / `just up` provides Postgres.

use egret_auth::groups::GroupRepo;
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

#[tokio::test]
async fn group_local_crud_and_membership() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());
    let rc = Recorder::default();

    // Create a local group.
    let name = uniq("t_group");
    let group_id = groups
        .create_local(&name, &Recorder::default())
        .await
        .unwrap();

    // Duplicate name errors (Conflict).
    assert!(groups
        .create_local(&name, &Recorder::default())
        .await
        .is_err());

    // Add two members.
    let u1 = users
        .insert_local(&format!("{}@test.local", uniq("u1")), "U1", "x")
        .await
        .unwrap();
    let u2 = users
        .insert_local(&format!("{}@test.local", uniq("u2")), "U2", "x")
        .await
        .unwrap();
    groups.add_member(group_id, u1, &rc).await.unwrap();
    groups.add_member(group_id, u2, &rc).await.unwrap();
    // Idempotent: re-adding does not error or duplicate.
    groups.add_member(group_id, u1, &rc).await.unwrap();

    let mut members = groups.members_of(group_id).await.unwrap();
    members.sort();
    let mut expected = vec![u1.0, u2.0];
    expected.sort();
    assert_eq!(members, expected);

    // list_with_meta shows a member count of 2 for our group.
    let with_meta = groups.list_with_meta().await.unwrap();
    let (g, count) = with_meta
        .iter()
        .find(|(g, _)| g.id == group_id)
        .expect("group present");
    assert_eq!(g.name, name);
    assert_eq!(g.source, "local");
    assert_eq!(*count, 2);

    // Remove one member -> count drops to 1.
    groups.remove_member(group_id, u1, &rc).await.unwrap();
    assert_eq!(groups.members_of(group_id).await.unwrap(), vec![u2.0]);
    let with_meta = groups.list_with_meta().await.unwrap();
    let (_, count) = with_meta
        .iter()
        .find(|(g, _)| g.id == group_id)
        .expect("group present");
    assert_eq!(*count, 1);

    // Rename the local group.
    let new_name = uniq("t_group_renamed");
    let rows = groups
        .rename(
            group_id,
            &new_name,
            &rc,
            egret_auth::groups::ReferenceDetail::Entries,
        )
        .await
        .unwrap();
    assert_eq!(rows, 1);

    // Delete the local group.
    let rows = groups
        .delete_local(group_id, &rc, egret_auth::groups::ReferenceDetail::Entries)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    assert!(groups
        .list_with_meta()
        .await
        .unwrap()
        .iter()
        .all(|(g, _)| g.id != group_id));

    // set_status on a user.
    let user_email = format!("{}@test.local", uniq("statususer"));
    let user_id = users
        .insert_local(&user_email, "StatusUser", "x")
        .await
        .unwrap();
    users.set_status(user_id, "disabled").await.unwrap();
    let fetched = users.find_by_id(user_id).await.unwrap().expect("exists");
    assert_eq!(fetched.status, "disabled");
}

#[tokio::test]
async fn rename_and_delete_are_noop_on_oidc_group() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let rc = Recorder::default();

    let name = uniq("t_oidc_group");
    let group_id = groups.upsert_oidc_group(&name).await.unwrap();

    // Neither rename nor delete affects an oidc-sourced group.
    assert_eq!(
        groups
            .rename(
                group_id,
                "renamed",
                &rc,
                egret_auth::groups::ReferenceDetail::Entries
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        groups
            .delete_local(group_id, &rc, egret_auth::groups::ReferenceDetail::Entries)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "a write that changes nothing locks and compiles nothing"
    );
}

#[tokio::test]
async fn groups_of_user_returns_membership() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());

    let name = uniq("t_groups_of_user");
    let group_id = groups
        .create_local(&name, &Recorder::default())
        .await
        .unwrap();

    let user_id = users
        .insert_local(&format!("{}@test.local", uniq("gn_user")), "GnUser", "x")
        .await
        .unwrap();
    groups
        .add_member(group_id, user_id, &Recorder::default())
        .await
        .unwrap();

    let names = groups.groups_of_user(user_id).await.unwrap();
    assert_eq!(names, vec![name]);
}

/// J-F3 Task 1b: every `GroupRepo` writer of `user_groups` calls the recompile
/// port — locking the affected group names BEFORE its write, recompiling
/// after — and a write that changes no row recompiles nothing. A REMOVE that
/// skipped the port would leave the ex-member named in every policy the group
/// reaches; the compiled effect is asserted in
/// `egret-perm-compile/tests/group_grants.rs`, this pins the call.
#[tokio::test]
async fn every_membership_writer_calls_the_recompile_port() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());
    let rc = Recorder::default();

    let name = uniq("t_port_group");
    let gid = groups
        .create_local(&name, &Recorder::default())
        .await
        .unwrap();
    let uid = users
        .insert_local(&format!("{}@test.local", uniq("port")), "P", "x")
        .await
        .unwrap();
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "creating a group moves no member"
    );

    groups.add_member(gid, uid, &rc).await.unwrap();
    assert_eq!(
        rc.take(),
        vec![Call::Lock(vec![name.clone()]), Call::Recompile]
    );
    groups.add_member(gid, uid, &rc).await.unwrap();
    assert_eq!(
        rc.take(),
        vec![Call::Lock(vec![name.clone()])],
        "an idempotent re-add writes no row, so nothing is recompiled"
    );

    groups.remove_member(gid, uid, &rc).await.unwrap();
    assert_eq!(
        rc.take(),
        vec![Call::Lock(vec![name.clone()]), Call::Recompile]
    );

    groups.add_member(gid, uid, &rc).await.unwrap();
    rc.take();
    let renamed = uniq("t_port_renamed");
    assert_eq!(
        groups
            .rename(
                gid,
                &renamed,
                &rc,
                egret_auth::groups::ReferenceDetail::Entries
            )
            .await
            .unwrap(),
        1
    );
    let mut both = vec![name.clone(), renamed.clone()];
    both.sort();
    assert_eq!(
        rc.take(),
        vec![Call::Lock(both), Call::Recompile],
        "a rename moves the members off the old name AND onto the new one"
    );

    users.hard_delete(uid, &rc).await.unwrap();
    assert_eq!(
        rc.take(),
        vec![Call::Lock(vec![renamed.clone()]), Call::Recompile],
        "deleting a user removes them from every group they were in"
    );

    assert_eq!(
        groups
            .delete_local(gid, &rc, egret_auth::groups::ReferenceDetail::Entries)
            .await
            .unwrap(),
        1
    );
    assert_eq!(rc.take(), vec![Call::Lock(vec![renamed]), Call::Recompile]);
}

/// Design R4: an OIDC group's membership is owned by the identity provider.
/// An admin ADD is refused with `Conflict` before any profile lock or
/// recompile and writes no row — it would be undone at the next login of an
/// OIDC user, and never undone for a local user, whom no login reaches. A
/// REMOVE stays allowed: it fails closed and serves as an emergency revoke
/// until the user's next login, so it still recompiles. An add to a LOCAL
/// group is unchanged.
#[tokio::test]
async fn an_admin_add_to_an_oidc_group_is_refused_and_a_remove_is_not() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());
    let rc = Recorder::default();
    let uid = users
        .insert_local(&format!("{}@test.local", uniq("r4")), "R", "x")
        .await
        .unwrap();
    let oidc = uniq("t_oidc_group");
    let oidc_gid = groups.upsert_oidc_group(&oidc).await.unwrap();
    let is_member = |gid: uuid::Uuid| {
        let groups = &groups;
        async move { groups.members_of(gid).await.unwrap().contains(&uid.0) }
    };

    let err = groups.add_member(oidc_gid, uid, &rc).await.unwrap_err();
    match err {
        egret_core::Error::Conflict(msg) => assert_eq!(
            msg,
            "Cannot add members to an OIDC-synced group; its membership is owned by the identity provider"
        ),
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert!(!is_member(oidc_gid).await, "a refused add writes no row");
    assert_eq!(
        rc.take(),
        Vec::<Call>::new(),
        "a refused add locks no profile and recompiles nothing"
    );

    // A row the login sync wrote: the admin can still remove it.
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(uid.0)
        .bind(oidc_gid)
        .execute(&db.pool)
        .await
        .unwrap();
    groups.remove_member(oidc_gid, uid, &rc).await.unwrap();
    assert!(!is_member(oidc_gid).await, "the remove deleted the row");
    assert_eq!(
        rc.take(),
        vec![Call::Lock(vec![oidc.clone()]), Call::Recompile],
        "the remove recompiles what the group reaches"
    );

    let local = uniq("t_local_group");
    let local_gid = groups
        .create_local(&local, &Recorder::default())
        .await
        .unwrap();
    groups.add_member(local_gid, uid, &rc).await.unwrap();
    assert!(
        is_member(local_gid).await,
        "an add to a local group still works"
    );
    assert_eq!(rc.take(), vec![Call::Lock(vec![local]), Call::Recompile]);
}

/// Design R5: rows whose provenance is unknown are REPORTED, never changed.
/// (a) a local group with members whose `users.provider = 'oidc'` — a login
/// before R1 may have added them — with the count of such members; (b) each
/// membership of a non-OIDC user (`provider` `'local'` or `'service'`) in an
/// OIDC group — the R4 class, which no login ever removes. A local user in a local group and an OIDC user
/// in an OIDC group are the expected shapes and appear in neither list.
/// Assertions are scoped to the seeded ids: the shared test DB holds others.
#[tokio::test]
async fn the_provenance_report_lists_only_the_two_unknown_shapes() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());
    let seed = |uid: egret_core::UserId, gid: uuid::Uuid| {
        let db = db.clone();
        async move {
            sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
                .bind(uid.0)
                .bind(gid)
                .execute(&db.pool)
                .await
                .unwrap();
        }
    };
    let oidc_user = |tag: &'static str| {
        let users = &users;
        async move {
            users
                .insert_oidc(&uniq("sub"), &format!("{}@idp.local", uniq(tag)), "O")
                .await
                .unwrap()
        }
    };
    let local_user = |tag: &'static str| {
        let users = &users;
        async move {
            users
                .insert_local(&format!("{}@test.local", uniq(tag)), "L", "x")
                .await
                .unwrap()
        }
    };

    let (o1, o2, o3) = (
        oidc_user("o1").await,
        oidc_user("o2").await,
        oidc_user("o3").await,
    );
    let (l1, l2) = (local_user("l1").await, local_user("l2").await);
    // A service account (`provider = 'service'`) is no more reached by a login
    // than a local user is, so it belongs in (b) too (final review I1).
    let svc = users
        .insert_service(&format!("{}@svc.local", uniq("svc")), "S")
        .await
        .unwrap();
    // (a): two OIDC users and one local user in a local group → count 2.
    let mixed_local = groups
        .create_local(&uniq("t_prov_mixed"), &Recorder::default())
        .await
        .unwrap();
    seed(o1, mixed_local).await;
    seed(o2, mixed_local).await;
    seed(l1, mixed_local).await;
    // Neither: a local group with only local members.
    let pure_local = groups
        .create_local(&uniq("t_prov_local"), &Recorder::default())
        .await
        .unwrap();
    seed(l2, pure_local).await;
    // (b): a local user in an OIDC group; the OIDC member beside them is not.
    let oidc_name = uniq("t_prov_oidc");
    let oidc_gid = groups.upsert_oidc_group(&oidc_name).await.unwrap();
    seed(l1, oidc_gid).await;
    seed(o3, oidc_gid).await;
    seed(svc, oidc_gid).await;

    let report = egret_auth::groups::membership_provenance_report(&db.pool)
        .await
        .unwrap();

    let a: Vec<_> = report
        .local_groups_with_oidc_members
        .iter()
        .filter(|r| [mixed_local, pure_local, oidc_gid].contains(&r.group_id))
        .map(|r| (r.group_id, r.oidc_members))
        .collect();
    assert_eq!(
        a,
        vec![(mixed_local, 2)],
        "(a) counts OIDC members of local groups only"
    );
    let mut b: Vec<_> = report
        .non_oidc_users_in_oidc_groups
        .iter()
        .filter(|r| [o1.0, o2.0, o3.0, l1.0, l2.0, svc.0].contains(&r.user_id))
        .map(|r| (r.user_id, r.group_id, r.group_name.clone()))
        .collect();
    b.sort();
    let mut want = vec![
        (l1.0, oidc_gid, oidc_name.clone()),
        (svc.0, oidc_gid, oidc_name),
    ];
    want.sort();
    assert_eq!(
        b, want,
        "(b) lists non-OIDC users (local and service) in OIDC groups only"
    );

    // The logging wrapper reports the same rows and changes none.
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_groups WHERE group_id = ANY($1)")
            .bind(vec![mixed_local, pure_local, oidc_gid])
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let logged = egret_auth::groups::warn_unknown_membership_provenance(&db.pool)
        .await
        .unwrap();
    assert!(logged
        .local_groups_with_oidc_members
        .iter()
        .any(|r| r.group_id == mixed_local && r.oidc_members == 2));
    assert!(logged
        .non_oidc_users_in_oidc_groups
        .iter()
        .any(|r| r.user_id == l1.0 && r.group_id == oidc_gid));
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_groups WHERE group_id = ANY($1)")
            .bind(vec![mixed_local, pure_local, oidc_gid])
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!((before, after), (7, 7), "the report changes no row");
}

/// 2026-10-18 design R1, this crate's half: while the port reports a reference
/// to the group (delete), or to its old or new name (rename), the write is
/// refused with `Conflict` BEFORE any profile lock or recompile, writes
/// nothing, and the message lists each entry — for a rename, with the name it
/// refers to. A rename INTO a name another group has is `Conflict` too.
/// (The real port is in `egret-perm-compile/tests/group_grants.rs`.)
#[tokio::test]
async fn a_referenced_group_cannot_be_deleted_or_renamed() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let name = uniq("t_ref_group");
    let target = uniq("t_ref_target");
    let gid = groups
        .create_local(&name, &Recorder::default())
        .await
        .unwrap();
    let grant_id = uuid::Uuid::new_v4();
    let rule_id = uuid::Uuid::new_v4();
    let rc = Recorder::with_references(vec![
        egret_auth::groups::GroupReference {
            group_name: name.clone(),
            source: "grant".into(),
            id: grant_id,
            rule_name: None,
            effect: Some("deny".into()),
            position: "include".into(),
        },
        egret_auth::groups::GroupReference {
            group_name: target.clone(),
            source: "rule".into(),
            id: rule_id,
            rule_name: Some("mask_email".into()),
            effect: None,
            position: "exclude".into(),
        },
    ]);

    let err = groups
        .delete_local(gid, &rc, egret_auth::groups::ReferenceDetail::Entries)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, egret_core::Error::Conflict(m) if m == &format!(
            "group `{name}` is named by 1 permission entry; remove them first: \
             grant {grant_id} (deny, include)"
        )),
        "{err:?}"
    );
    assert_eq!(rc.take(), Vec::<Call>::new(), "refused before any lock");

    let err = groups
        .rename(
            gid,
            &target,
            &rc,
            egret_auth::groups::ReferenceDetail::Entries,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, egret_core::Error::Conflict(m) if m == &format!(
            "cannot rename group `{name}` to `{target}`: these names are named by 2 \
             permission entries; remove them first: grant {grant_id} (deny, include) \
             naming `{name}`, rule `mask_email` (exempt exclude) naming `{target}`"
        )),
        "{err:?}"
    );
    assert_eq!(rc.take(), Vec::<Call>::new(), "refused before any lock");

    let taken = groups
        .create_local(&uniq("t_ref_taken"), &Recorder::default())
        .await;
    let taken_name: String = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
        .bind(taken.unwrap())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let err = groups
        .rename(
            gid,
            &taken_name,
            &Recorder::default(),
            egret_auth::groups::ReferenceDetail::Entries,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, egret_core::Error::Conflict(m) if m.contains("already exists")),
        "{err:?}"
    );

    let still: String = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
        .bind(gid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(still, name, "nothing written");
}

/// Review M-3: `create_local` takes no group lock, so a group of the new name
/// can appear between the rename's "already exists" read and its UPDATE. The
/// unique index then refuses the UPDATE, and the rename answers `Conflict`
/// like `create_local` does — not an internal error. Deterministic: the
/// competing insert is held uncommitted while the rename runs, so the read
/// misses it and the UPDATE waits on it.
#[tokio::test]
async fn a_rename_racing_a_create_of_the_new_name_is_a_conflict() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let gid = groups
        .create_local(&uniq("t_race_rn"), &Recorder::default())
        .await
        .unwrap();
    let target = uniq("t_race_target");

    let mut competing = db.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO groups (name, source) VALUES ($1, 'local')")
        .bind(&target)
        .execute(&mut *competing)
        .await
        .unwrap();
    let rename = {
        let groups = groups.clone();
        let target = target.clone();
        tokio::spawn(async move {
            groups
                .rename(
                    gid,
                    &target,
                    &Recorder::default(),
                    egret_auth::groups::ReferenceDetail::Entries,
                )
                .await
        })
    };
    // Wait until the rename has passed its read and is blocked in the UPDATE
    // on the uncommitted index entry — otherwise its read would see the row
    // and answer from there, and this test would not reach the UPDATE.
    let mut blocked = false;
    for _ in 0..200 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE wait_event_type = 'Lock' AND query LIKE 'UPDATE groups SET name%'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if n > 0 {
            blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(blocked, "the rename never reached its UPDATE");
    competing.commit().await.unwrap();

    let err = rename.await.unwrap().unwrap_err();
    assert!(
        matches!(&err, egret_core::Error::Conflict(m) if m == &format!("group already exists: {target}")),
        "{err:?}"
    );
}
