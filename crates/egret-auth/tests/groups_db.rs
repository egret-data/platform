//! DB-gated `GroupRepo`/`UserRepo` tests. Self-skip (pass) without
//! `DATABASE_URL` so the suite stays green offline; CI / `just up` provides
//! Postgres.

use egret_auth::groups::GroupRepo;
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

#[tokio::test]
async fn group_local_crud_and_membership() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());

    // Create a local group.
    let name = uniq("t_group");
    let group_id = groups.create_local(&name).await.unwrap();

    // Duplicate name errors (Conflict).
    assert!(groups.create_local(&name).await.is_err());

    // Add two members.
    let u1 = users
        .insert_local(&format!("{}@test.local", uniq("u1")), "U1", "x")
        .await
        .unwrap();
    let u2 = users
        .insert_local(&format!("{}@test.local", uniq("u2")), "U2", "x")
        .await
        .unwrap();
    groups.add_member(group_id, u1).await.unwrap();
    groups.add_member(group_id, u2).await.unwrap();
    // Idempotent: re-adding does not error or duplicate.
    groups.add_member(group_id, u1).await.unwrap();

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
    groups.remove_member(group_id, u1).await.unwrap();
    assert_eq!(groups.members_of(group_id).await.unwrap(), vec![u2.0]);
    let with_meta = groups.list_with_meta().await.unwrap();
    let (_, count) = with_meta
        .iter()
        .find(|(g, _)| g.id == group_id)
        .expect("group present");
    assert_eq!(*count, 1);

    // Rename the local group.
    let new_name = uniq("t_group_renamed");
    let rows = groups.rename(group_id, &new_name).await.unwrap();
    assert_eq!(rows, 1);

    // Delete the local group.
    let rows = groups.delete_local(group_id).await.unwrap();
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
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let groups = GroupRepo::new(db.clone());

    let name = uniq("t_oidc_group");
    let group_id = groups.upsert_oidc_group(&name).await.unwrap();

    // Neither rename nor delete affects an oidc-sourced group.
    assert_eq!(groups.rename(group_id, "renamed").await.unwrap(), 0);
    assert_eq!(groups.delete_local(group_id).await.unwrap(), 0);
}

#[tokio::test]
async fn groups_of_user_returns_membership() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());

    let name = uniq("t_groups_of_user");
    let group_id = groups.create_local(&name).await.unwrap();

    let user_id = users
        .insert_local(&format!("{}@test.local", uniq("gn_user")), "GnUser", "x")
        .await
        .unwrap();
    groups.add_member(group_id, user_id).await.unwrap();

    let names = groups.groups_of_user(user_id).await.unwrap();
    assert_eq!(names, vec![name]);
}
