//! Notebook HTTP route tests — ownership + IDOR enforcement.
//! DB-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the `/ws/{workspace}/notebooks` router (`notebook_router`):
//!   - create → 201 with exactly one seeded `code` cell; GET → 200, `is_owner`
//!     true for the owner,
//!   - a non-owner member is Forbidden (403) on a *private* notebook, then 200
//!     (with `is_owner=false`) once the owner makes it public,
//!   - a **platform_admin** gets 403 on another user's PRIVATE notebook (read)
//!     and 403 on PATCH/DELETE — there is deliberately NO admin override,
//!   - a notebook created under workspace A is 404 through workspace B's path
//!     (cross-tenant IDOR closed),
//!   - PATCH/DELETE by a non-owner member → 403,
//!   - cell add/update/delete/reorder happy path for the owner; a cell of
//!     notebook X addressed under notebook Y → 404 (nested IDOR closed),
//!   - unknown `cell_type` on add → 400.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-notebook --test http -- --test-threads=1

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_notebook::{notebook_router, NotebookState, NotebookStore};
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    members: MemberRepo,
    sessions: SessionService,
    authz: Authorizer,
    slug_a: String,
    slug_b: String,
    ws_a: WorkspaceId,
    ws_b: WorkspaceId,
    owner: UserId,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::new_v4().simple().to_string();
    let owner = users
        .insert_local(&format!("nbowner-{suffix}@egret.test"), "NB Owner", "x")
        .await
        .expect("seed owner");

    let workspaces = WorkspaceRepo::new(db.clone());
    let slug_a = format!("nba{suffix}");
    let slug_b = format!("nbb{suffix}");
    let ws_a = workspaces
        .create(&slug_a, "NB A", owner)
        .await
        .expect("seed ws A");
    let ws_b = workspaces
        .create(&slug_b, "NB B", owner)
        .await
        .expect("seed ws B");

    let app = notebook_router().with_state(NotebookState {
        store: NotebookStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces,
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        users,
        members: MemberRepo::new(db.clone()),
        sessions,
        authz: Authorizer::new(db.clone()),
        slug_a,
        slug_b,
        ws_a: ws_a.id,
        ws_b: ws_b.id,
        owner,
    })
}

/// Insert a user, make it a member of `workspace` with `role`, and return
/// `(user_id, session_token)`.
async fn add_member(
    f: &Fixture,
    workspace: WorkspaceId,
    tag: &str,
    role: &str,
) -> (UserId, String) {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(workspace, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    let token = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    (uid, token)
}

/// Insert a platform_admin user (no workspace membership needed — `WsContext`
/// bypasses membership for platform_admin). Returns `(user_id, token)`.
async fn add_platform_admin(f: &Fixture, tag: &str) -> (UserId, String) {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Admin", "x")
        .await
        .expect("insert admin");
    f.authz
        .replace_platform_roles(uid, &["platform_admin".to_string()])
        .await
        .expect("grant platform_admin");
    let token = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    (uid, token)
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

/// Create a notebook (as `token`) under workspace `slug`; returns its id.
async fn create_nb(f: &Fixture, slug: &str, token: &str) -> String {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{slug}/notebooks"),
            token,
            Some(serde_json::json!({ "name": "nb" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create notebook; body={json}");
    json["id"].as_str().expect("id in body").to_string()
}

#[tokio::test]
async fn create_and_get_owner_sees_seeded_cell() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_uid, token) = add_member(&f, f.ws_a, "owner", "ws_editor").await;

    // CREATE → 201 with exactly one seeded code cell.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks", f.slug_a),
            &token,
            Some(serde_json::json!({ "name": "first" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={json}");
    let cells = json["cells"].as_array().expect("cells array");
    assert_eq!(cells.len(), 1, "create seeds exactly one cell; body={json}");
    assert_eq!(cells[0]["cell_type"], "code", "seeded cell is a code cell");
    let id = json["id"].as_str().unwrap().to_string();

    // GET → 200, is_owner true for the owner.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebooks/{id}", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "owner GET; body={json}");
    assert_eq!(json["is_owner"], serde_json::json!(true));
}

#[tokio::test]
async fn non_owner_private_403_then_public_200() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_other_uid, other_tok) = add_member(&f, f.ws_a, "other", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // Non-owner GET on a PRIVATE notebook → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &other_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-owner must not view a private notebook"
    );

    // Owner makes it public.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &owner_tok,
            Some(serde_json::json!({ "is_public": true })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "owner can publish");

    // Now non-owner GET → 200 with is_owner=false.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &other_tok, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "non-owner can view a public nb");
    assert_eq!(
        json["is_owner"],
        serde_json::json!(false),
        "non-owner viewer => is_owner false"
    );
}

#[tokio::test]
async fn platform_admin_has_no_override() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_admin_uid, admin_tok) = add_platform_admin(&f, "admin").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // GET another user's PRIVATE notebook → still 403 (no override).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &admin_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "platform_admin must NOT read another user's private notebook"
    );

    // PATCH → 403 (no override).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &admin_tok,
            Some(serde_json::json!({ "name": "hijacked" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "platform_admin must NOT patch another user's notebook"
    );

    // DELETE → 403 (no override).
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &admin_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "platform_admin must NOT delete another user's notebook"
    );
}

#[tokio::test]
async fn cross_workspace_get_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Owner is a member of both A and B (so WsContext passes for the B path).
    let (owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    f.members
        .add(f.ws_b, "user", owner_uid.0, "ws_editor", f.owner)
        .await
        .expect("member of B too");
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;

    // Same notebook id under workspace B's path → 404 (cross-tenant IDOR closed).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebooks/{id}", f.slug_b),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a notebook of ws A must 404 through ws B's path"
    );
}

#[tokio::test]
async fn non_owner_patch_and_delete_are_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_other_uid, other_tok) = add_member(&f, f.ws_a, "other", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &other_tok,
            Some(serde_json::json!({ "name": "nope" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-owner PATCH must be forbidden"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &other_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-owner DELETE must be forbidden"
    );
}

#[tokio::test]
async fn cell_crud_happy_path_and_nested_idor() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let nb_x = create_nb(&f, &f.slug_a, &owner_tok).await;
    let nb_y = create_nb(&f, &f.slug_a, &owner_tok).await;
    let cells = format!("/ws/{}/notebooks/{nb_x}/cells", f.slug_a);

    // ADD cell → 201, notebook now has 2 cells.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &cells,
            &owner_tok,
            Some(serde_json::json!({ "cell_type": "markdown", "source": "# hi", "cell_order": 1 })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add cell; body={json}");
    let cell_list = json["cells"].as_array().unwrap();
    assert_eq!(cell_list.len(), 2, "notebook X now has 2 cells");
    let seeded_cell = cell_list
        .iter()
        .find(|c| c["cell_type"] == "code")
        .expect("seeded code cell");
    let cid_x = seeded_cell["id"].as_str().unwrap().to_string();
    let new_cid = cell_list
        .iter()
        .find(|c| c["cell_type"] == "markdown")
        .expect("new markdown cell")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // UPDATE cell → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("{cells}/{cid_x}"),
            &owner_tok,
            Some(serde_json::json!({ "source": "print(1)" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "update cell");

    // REORDER → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("{cells}/order"),
            &owner_tok,
            Some(serde_json::json!({ "order": [new_cid, cid_x] })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "reorder cells");

    // Verify reorder took effect: markdown cell is now first.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebooks/{nb_x}", f.slug_a),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    let (_status, json) = into_json(resp).await;
    assert_eq!(
        json["cells"][0]["id"].as_str(),
        Some(new_cid.as_str()),
        "markdown cell is now first after reorder"
    );

    // Nested IDOR: cell of notebook X addressed under notebook Y → 404.
    // (write_scoped passes for Y since owner owns Y, but the cell row is scoped
    //  by notebook_id so the parent mismatch matches no rows → NotFound.)
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/ws/{}/notebooks/{nb_y}/cells/{cid_x}", f.slug_a),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-notebook DELETE must 404 (foreign cell not in this notebook)"
    );

    // Same for PATCH: updating notebook X's cell under notebook Y → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/ws/{}/notebooks/{nb_y}/cells/{cid_x}", f.slug_a),
            &owner_tok,
            Some(serde_json::json!({ "source": "hijacked" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-notebook PATCH must 404 (foreign cell not in this notebook)"
    );

    // Prove notebook X still has both cells (the cross-notebook ops were rejected,
    // not silently applied) — the foreign cell is provably untouched.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebooks/{nb_x}", f.slug_a),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    let (_status, json) = into_json(resp).await;
    assert_eq!(
        json["cells"].as_array().unwrap().len(),
        2,
        "cross-notebook ops must NOT touch notebook X's cells (nested IDOR closed)"
    );
    let still_unchanged = json["cells"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"].as_str() == Some(cid_x.as_str()) && c["source"] == "print(1)");
    assert!(
        still_unchanged,
        "the foreign cell's source must remain the owner's value, not 'hijacked'"
    );

    // DELETE the cell under its real parent → cell count drops to 1.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{cells}/{cid_x}"), &owner_tok, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete cell");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebooks/{nb_x}", f.slug_a),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    let (_status, json) = into_json(resp).await;
    assert_eq!(json["cells"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_cell_type_is_400() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;

    // Unknown cell_type on add_cell → 400.
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{id}/cells", f.slug_a),
            &owner_tok,
            Some(serde_json::json!({ "cell_type": "bogus", "source": "" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown cell_type must be rejected"
    );
}

/// List notebooks (optionally the trash bin) as `token` under workspace `slug`.
async fn list_nbs(f: &Fixture, slug: &str, token: &str, trashed: bool) -> serde_json::Value {
    let uri = if trashed {
        format!("/ws/{slug}/notebooks?trashed=true")
    } else {
        format!("/ws/{slug}/notebooks")
    };
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list notebooks; body={json}");
    json
}

#[tokio::test]
async fn trash_hides_and_restore_reappears() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;

    // Trash → 204; live list hides it, trash bin shows it.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{id}/trash", f.slug_a),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "trash → 204");

    let live = list_nbs(&f, &f.slug_a, &owner_tok, false).await;
    assert!(
        !live.as_array().unwrap().iter().any(|n| n["id"] == id),
        "trashed notebook hidden from default list"
    );
    let trash = list_nbs(&f, &f.slug_a, &owner_tok, true).await;
    let trashed = trash
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .expect("trashed notebook in trash bin");
    assert!(
        trashed["deleted_at"].is_string(),
        "trashed notebook carries deleted_at"
    );

    // Restore → 204; reappears in live list.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{id}/restore", f.slug_a),
            &owner_tok,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "restore → 204");
    let live = list_nbs(&f, &f.slug_a, &owner_tok, false).await;
    assert!(
        live.as_array().unwrap().iter().any(|n| n["id"] == id),
        "restored notebook back in live list"
    );
}

#[tokio::test]
async fn non_owner_trash_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_other_uid, other_tok) = add_member(&f, f.ws_a, "other", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{id}/trash", f.slug_a),
            &other_tok,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-owner trash must be forbidden"
    );
}

#[tokio::test]
async fn bookmark_is_per_user_and_allowed_on_public() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_other_uid, other_tok) = add_member(&f, f.ws_a, "other", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // Owner bookmarks own notebook → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("{uri}/bookmark"), &owner_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "owner bookmark → 204"
    );

    // Owner's list shows bookmarked=true for that notebook.
    let owner_list = list_nbs(&f, &f.slug_a, &owner_tok, false).await;
    let mine = owner_list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .expect("owner sees own notebook");
    assert_eq!(
        mine["bookmarked"],
        serde_json::json!(true),
        "owner sees bookmarked=true"
    );

    // Make it public so the other member can read + bookmark it.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &owner_tok,
            Some(serde_json::json!({ "is_public": true })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "publish");

    // Other user's list shows bookmarked=false (per-user flag).
    let other_list = list_nbs(&f, &f.slug_a, &other_tok, false).await;
    let theirs = other_list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .expect("other sees the public notebook");
    assert_eq!(
        theirs["bookmarked"],
        serde_json::json!(false),
        "other user's view: not bookmarked"
    );

    // Non-owner CAN bookmark a PUBLIC notebook (read_scoped, not owner-only) → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("{uri}/bookmark"), &other_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "non-owner may bookmark a public notebook"
    );
    let other_list = list_nbs(&f, &f.slug_a, &other_tok, false).await;
    let theirs = other_list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap();
    assert_eq!(
        theirs["bookmarked"],
        serde_json::json!(true),
        "other user now sees their own bookmark"
    );

    // DELETE bookmark → 204, flag clears for that user.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{uri}/bookmark"), &other_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "remove bookmark → 204"
    );
    let other_list = list_nbs(&f, &f.slug_a, &other_tok, false).await;
    let theirs = other_list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap();
    assert_eq!(theirs["bookmarked"], serde_json::json!(false));
}

/// Create a folder of `entity_type` under workspace `ws_id` directly in the DB.
async fn create_folder(db: &Db, ws_id: WorkspaceId, entity_type: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO folders (workspace_id, entity_type, name) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(ws_id.0)
    .bind(entity_type)
    .bind(format!("F-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert folder")
}

#[tokio::test]
async fn set_folder_happy_path_and_guards() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // We need a Db handle to seed folders; rebuild from DATABASE_URL.
    let db = Db::connect(&std::env::var("DATABASE_URL").unwrap(), 5)
        .await
        .unwrap();

    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}/folder", f.slug_a);

    // Happy path: a notebook folder in this workspace → 204, folder_id set.
    let good = create_folder(&db, f.ws_a, "notebook").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &owner_tok,
            Some(serde_json::json!({ "folder_id": good })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "set_folder → 204");

    let list = list_nbs(&f, &f.slug_a, &owner_tok, false).await;
    let mine = list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap();
    assert_eq!(
        mine["folder_id"].as_str(),
        Some(good.to_string().as_str()),
        "folder_id reflects the assignment"
    );

    // Wrong entity_type (a dashboard folder in the same ws) → 404.
    let wrong_type = create_folder(&db, f.ws_a, "dashboard").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &owner_tok,
            Some(serde_json::json!({ "folder_id": wrong_type })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "folder of a different entity_type must be rejected"
    );

    // Folder belonging to a different workspace → 404.
    let other_ws = create_folder(&db, f.ws_b, "notebook").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &owner_tok,
            Some(serde_json::json!({ "folder_id": other_ws })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "folder of a different workspace must be rejected"
    );

    // Clear the folder (null) → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &owner_tok,
            Some(serde_json::json!({ "folder_id": null })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "clearing the folder → 204"
    );
}
