//! Access-request → approval workflow (P2b) data-layer integration tests.
//! Covers create + dup-pending conflict, list-for-requester / list-for-workspace,
//! approve (creates the workspace member with the chosen role + expiry), deny,
//! cancel guards, joinable (active workspaces the user is NOT in), and
//! `MemberRepo::add_with_expiry`. DB-gated; self-skips without DATABASE_URL —
//! mirroring the sibling `access_views_db.rs` / `members_ax1.rs` harnesses.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{Error, UserId, WorkspaceId};
use egret_metastore::Db;
use egret_workspace::access_requests::AccessRequestRepo;
use egret_workspace::members::MemberRepo;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    let email = format!("u-{}@t.local", uuid::Uuid::new_v4().simple());
    UserRepo::new(db.clone())
        .insert_local(&email, "U", &hash_password("pw").unwrap())
        .await
        .unwrap()
}

fn repos(db: &Db) -> (AccessRequestRepo, WorkspaceRepo, MemberRepo) {
    let members = MemberRepo::new(db.clone());
    (
        AccessRequestRepo::new(db.clone(), members.clone()),
        WorkspaceRepo::new(db.clone()),
        members,
    )
}

/// The workspace-grant row (role name + expiry) for a principal, if any.
async fn grant_of(
    db: &Db,
    ws: WorkspaceId,
    uid: UserId,
) -> Option<(String, Option<chrono::DateTime<chrono::Utc>>)> {
    sqlx::query_as::<_, (String, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT r.name, ag.expires_at FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
           AND ag.principal_type = 'user' AND ag.principal_id = $2",
    )
    .bind(ws.0)
    .bind(uid.0)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn status_of(db: &Db, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM access_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

// ── create + dup-pending conflict ─────────────────────────────────────────────

#[tokio::test]
async fn create_inserts_pending_and_rejects_dup_pending() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (reqs, wrepo, _) = repos(&db);
    let requester = seed_user(&db).await;
    let admin = seed_user(&db).await;
    let ws = wrepo.create(&uniq("ws"), "W", admin).await.unwrap();

    let ar = reqs
        .create(requester, ws.id, "ws_viewer", "please let me in")
        .await
        .unwrap();
    assert_eq!(ar.status, "pending");
    assert_eq!(ar.requester_id, requester.0);
    assert_eq!(ar.workspace_id, ws.id.0);
    assert_eq!(ar.requested_role, "ws_viewer");
    assert_eq!(ar.reason, "please let me in");

    // A second pending request for the same (requester, workspace) → Conflict.
    let dup = reqs.create(requester, ws.id, "ws_viewer", "again").await;
    assert!(
        matches!(dup, Err(Error::Conflict(_))),
        "duplicate pending request must be a Conflict, got {dup:?}"
    );
}

// ── list_for_requester / list_for_workspace ───────────────────────────────────

#[tokio::test]
async fn lists_surface_the_pending_request() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (reqs, wrepo, _) = repos(&db);
    let requester = seed_user(&db).await;
    let admin = seed_user(&db).await;
    let ws = wrepo.create(&uniq("ws"), "W", admin).await.unwrap();
    let ar = reqs
        .create(requester, ws.id, "ws_viewer", "hi")
        .await
        .unwrap();

    let mine = reqs.list_for_requester(requester).await.unwrap();
    assert!(
        mine.iter().any(|r| r.id == ar.id),
        "requester sees own request"
    );

    let inbox = reqs.list_for_workspace(ws.id, "pending").await.unwrap();
    let found = inbox
        .iter()
        .find(|r| r.id == ar.id)
        .expect("workspace pending inbox lists the request");
    assert!(
        found.requester_name.is_some(),
        "list_for_workspace enriches requester_name"
    );

    // A different status filter must not return the pending row.
    let approved = reqs.list_for_workspace(ws.id, "approved").await.unwrap();
    assert!(!approved.iter().any(|r| r.id == ar.id));
}

// ── approve → creates member with role + expiry ───────────────────────────────

#[tokio::test]
async fn approve_creates_member_with_role_and_expiry() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (reqs, wrepo, _) = repos(&db);
    let requester = seed_user(&db).await;
    let admin = seed_user(&db).await;
    let ws = wrepo.create(&uniq("ws"), "W", admin).await.unwrap();
    let ar = reqs
        .create(requester, ws.id, "ws_viewer", "hi")
        .await
        .unwrap();

    // Not yet a member.
    assert!(grant_of(&db, ws.id, requester).await.is_none());

    let expires_at = chrono::Utc::now() + chrono::Duration::days(30);
    reqs.approve(ar.id, ws.id, Some("ws_editor"), Some(expires_at), admin)
        .await
        .unwrap();

    // Request row updated.
    assert_eq!(status_of(&db, ar.id).await, "approved");
    let (decided_by, decided_at): (Option<Uuid>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT decided_by, decided_at FROM access_requests WHERE id = $1")
            .bind(ar.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(decided_by, Some(admin.0));
    assert!(decided_at.is_some());

    // Member now exists with the chosen role + expiry.
    let (role, got_exp) = grant_of(&db, ws.id, requester)
        .await
        .expect("requester is now a workspace member");
    assert_eq!(role, "ws_editor");
    let got_exp = got_exp.expect("grant carries the chosen expiry");
    assert!(
        (got_exp - expires_at).num_milliseconds().abs() < 1000,
        "expiry persisted (got {got_exp}, want {expires_at})"
    );
}

// ── deny ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deny_marks_denied_with_note() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (reqs, wrepo, _) = repos(&db);
    let requester = seed_user(&db).await;
    let admin = seed_user(&db).await;
    let ws = wrepo.create(&uniq("ws"), "W", admin).await.unwrap();
    let ar = reqs
        .create(requester, ws.id, "ws_viewer", "hi")
        .await
        .unwrap();

    reqs.deny(ar.id, ws.id, Some("no"), admin).await.unwrap();
    assert_eq!(status_of(&db, ar.id).await, "denied");
    let note: Option<String> =
        sqlx::query_scalar("SELECT decision_note FROM access_requests WHERE id = $1")
            .bind(ar.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(note.as_deref(), Some("no"));
    // Denied ≠ member.
    assert!(grant_of(&db, ws.id, requester).await.is_none());
}

// ── cancel + guards ───────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_by_owner_only_and_pending_only() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (reqs, wrepo, _) = repos(&db);
    let requester = seed_user(&db).await;
    let stranger = seed_user(&db).await;
    let admin = seed_user(&db).await;
    let ws = wrepo.create(&uniq("ws"), "W", admin).await.unwrap();
    let ar = reqs
        .create(requester, ws.id, "ws_viewer", "hi")
        .await
        .unwrap();

    // Cancel by a different user → must NOT cancel.
    let by_stranger = reqs.cancel(ar.id, stranger).await;
    assert!(
        by_stranger.is_err(),
        "stranger cannot cancel, got {by_stranger:?}"
    );
    assert_eq!(status_of(&db, ar.id).await, "pending", "still pending");

    // Cancel by owner → cancelled.
    reqs.cancel(ar.id, requester).await.unwrap();
    assert_eq!(status_of(&db, ar.id).await, "cancelled");

    // Cancelling a non-pending request again → error / no-op.
    let again = reqs.cancel(ar.id, requester).await;
    assert!(
        again.is_err(),
        "cannot cancel a non-pending request, got {again:?}"
    );
    assert_eq!(status_of(&db, ar.id).await, "cancelled");
}

// ── joinable ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn joinable_lists_active_non_member_workspaces() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (reqs, wrepo, mrepo) = repos(&db);
    let user = seed_user(&db).await;
    let admin = seed_user(&db).await;

    // A workspace the user is NOT in.
    let open = wrepo.create(&uniq("open"), "Open", admin).await.unwrap();
    // A workspace the user IS a member of.
    let joined = wrepo
        .create(&uniq("joined"), "Joined", admin)
        .await
        .unwrap();
    mrepo
        .add(joined.id, "user", user.0, "ws_viewer", admin)
        .await
        .unwrap();

    let joinable = reqs.joinable(user).await.unwrap();
    let ids: Vec<Uuid> = joinable.iter().map(|(id, _, _)| *id).collect();
    assert!(
        ids.contains(&open.id.0),
        "non-member active workspace is joinable"
    );
    assert!(
        !ids.contains(&joined.id.0),
        "workspace the user already belongs to is excluded"
    );
}

// ── MemberRepo::add_with_expiry ───────────────────────────────────────────────

#[tokio::test]
async fn add_with_expiry_persists_and_add_leaves_null() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (_, wrepo, mrepo) = repos(&db);
    let admin = seed_user(&db).await;
    let with_exp = seed_user(&db).await;
    let without = seed_user(&db).await;
    let ws = wrepo.create(&uniq("ws"), "W", admin).await.unwrap();

    let ts = chrono::Utc::now() + chrono::Duration::days(7);
    mrepo
        .add_with_expiry(ws.id, "user", with_exp.0, "ws_viewer", admin, Some(ts))
        .await
        .unwrap();
    mrepo
        .add(ws.id, "user", without.0, "ws_viewer", admin)
        .await
        .unwrap();

    let (_, exp) = grant_of(&db, ws.id, with_exp).await.unwrap();
    let exp = exp.expect("add_with_expiry persists expires_at");
    assert!((exp - ts).num_milliseconds().abs() < 1000);

    let (_, none_exp) = grant_of(&db, ws.id, without).await.unwrap();
    assert!(none_exp.is_none(), "plain add leaves expires_at NULL");
}
