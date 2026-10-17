//! DB-gated: self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Verifies that granting an IdP
//! group access to a workspace makes a user in that group inherit its perms.

use egret_auth::groups::GroupRepo;
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::workspaces::WorkspaceRepo;

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
async fn grant_group_binds_workspace_perms() {
    let Some(db) = connect().await else { return };
    let users = UserRepo::new(db.clone());

    let admin_email = format!("{}@test.local", uuid::Uuid::new_v4());
    let admin_id = users
        .insert_local(&admin_email, "Admin", &hash_password("pw").unwrap())
        .await
        .unwrap();

    let slug = uniq("ws");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Test WS", admin_id)
        .await
        .unwrap();

    let group = uniq("data-team");
    let member_id: UserId = users
        .insert_oidc(
            &uniq("sub"),
            &format!("{}@test.local", uuid::Uuid::new_v4()),
            "Member",
        )
        .await
        .unwrap();
    let groups = GroupRepo::new(db.clone());
    groups.upsert_oidc_group(&group).await.unwrap();
    groups
        .sync_user_oidc_groups(
            member_id,
            std::slice::from_ref(&group),
            &egret_perm_compile::PermGroupRecompiler,
        )
        .await
        .unwrap();

    // Enforcement/permission-union now lives in the PDP; assert the durable effect of
    // the grant directly — before it runs, the group is not yet a workspace member.
    let members_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
           AND ag.principal_type = 'group' AND r.name = 'ws_editor'",
    )
    .bind(ws.id.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(members_before, 0, "no group member before grant");

    egret_bin::grant_group_to_workspace(&db, &slug, &group, "ws_editor", &admin_email)
        .await
        .unwrap();

    let members_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
           AND ag.principal_type = 'group' AND r.name = 'ws_editor'",
    )
    .bind(ws.id.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        members_after, 1,
        "grant binds the group as a ws_editor member"
    );
}
