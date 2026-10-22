//! KI-185: SSO pre-registration. A workspace admin stores a membership for an
//! email nobody has signed in with; the first sign-in redeems it through the
//! `PendingMembershipRedeemer` port the OIDC callback calls. DB-gated, same
//! harness as `members_ax1.rs`.

use egret_auth::password::hash_password;
use egret_auth::preregistration::PendingMembershipRedeemer;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
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

async fn user(db: &Db, email: &str) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(email, "U", &hash_password("pw").unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_pre_registration_becomes_a_membership_at_first_sign_in() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = user(&db, &format!("{}@t.local", uniq("admin"))).await;
    let ws = WorkspaceRepo::new(db.clone())
        .create(&uniq("ws"), "W", admin)
        .await
        .unwrap();
    let members = MemberRepo::new(db.clone());
    let email = format!("{}@t.local", uniq("newcomer"));

    // Case differs from what the IdP will send: the match is case-insensitive.
    members
        .invite(ws.id, &email.to_uppercase(), "ws_editor", admin)
        .await
        .unwrap();
    let listed = members.list_invites(ws.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].role, "ws_editor");

    // First sign-in: the OIDC provisioning created the user.
    let newcomer = user(&db, &email).await;
    let redeemed = members.redeem(newcomer, &email).await.unwrap();

    assert_eq!(redeemed.len(), 1);
    assert_eq!(redeemed[0].workspace_id, ws.id.0);
    assert_eq!(redeemed[0].invited_by, admin.0);
    assert_eq!(
        members
            .role_of(ws.id, "user", newcomer.0)
            .await
            .unwrap()
            .as_deref(),
        Some("ws_editor")
    );
    assert!(
        members.list_invites(ws.id).await.unwrap().is_empty(),
        "redeemed once"
    );
    assert!(members.redeem(newcomer, &email).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_pre_registration_cannot_carry_a_platform_role() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = user(&db, &format!("{}@t.local", uniq("admin"))).await;
    let ws = WorkspaceRepo::new(db.clone())
        .create(&uniq("ws"), "W", admin)
        .await
        .unwrap();
    let members = MemberRepo::new(db.clone());

    let refused = members
        .invite(ws.id, "boss@t.local", "platform_admin", admin)
        .await;
    assert!(refused.is_err(), "the handler refuses a platform role");

    // And the table refuses it even when written directly.
    let direct = sqlx::query(
        "INSERT INTO workspace_invites (workspace_id, email, role_id, invited_by) \
         SELECT $1, 'boss@t.local', r.id, $2 FROM roles r \
          WHERE r.scope = 'platform' AND r.name = 'platform_admin'",
    )
    .bind(ws.id.0)
    .bind(admin.0)
    .execute(&db.pool)
    .await;
    assert!(
        direct.is_err(),
        "the foreign key to roles (id, scope) refuses it"
    );
}

#[tokio::test]
async fn an_existing_membership_is_kept_and_withdrawn_invites_grant_nothing() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = user(&db, &format!("{}@t.local", uniq("admin"))).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let ws1 = wrepo.create(&uniq("ws"), "W1", admin).await.unwrap();
    let ws2 = wrepo.create(&uniq("ws"), "W2", admin).await.unwrap();
    let members = MemberRepo::new(db.clone());
    let email = format!("{}@t.local", uniq("someone"));

    members
        .invite(ws1.id, &email, "ws_admin", admin)
        .await
        .unwrap();
    members
        .invite(ws2.id, &email, "ws_viewer", admin)
        .await
        .unwrap();
    let someone = user(&db, &email).await;
    // Added by hand to ws1 in between, as a viewer.
    members
        .add(ws1.id, "user", someone.0, "ws_viewer", admin)
        .await
        .unwrap();
    // ws2's invite withdrawn before the sign-in.
    let id = members.list_invites(ws2.id).await.unwrap()[0].id;
    assert!(
        !members.delete_invite(ws1.id, id).await.unwrap(),
        "scoped to its workspace"
    );
    assert!(members.delete_invite(ws2.id, id).await.unwrap());

    let redeemed = members.redeem(someone, &email).await.unwrap();

    assert!(redeemed.is_empty());
    assert_eq!(
        members
            .role_of(ws1.id, "user", someone.0)
            .await
            .unwrap()
            .as_deref(),
        Some("ws_viewer"),
        "the role set by hand is not replaced"
    );
    assert_eq!(
        members.role_of(ws2.id, "user", someone.0).await.unwrap(),
        None
    );
    assert!(members.list_invites(ws1.id).await.unwrap().is_empty());
}
