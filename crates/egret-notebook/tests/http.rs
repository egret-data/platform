//! Notebook HTTP route tests — role-based authorization + IDOR enforcement.
//!
//! Notebooks are ROLE-BASED (like BI content): the per-`{id}` handlers are
//! object-scoped via `require_object`, so a workspace role's `notebook:read`/
//! `notebook:write` cascades to every notebook in the workspace (owner/public no
//! longer gate access). These prove the `/ws/{workspace}/notebooks` router:
//!   - create → 201 with exactly one seeded `code` cell; GET → 200, `is_owner`
//!     true for the owner,
//!   - a non-owner workspace member READS any notebook (200, `is_owner=false`),
//!   - a **platform_admin** now HAS an override (object → Platform ancestry):
//!     reads + writes another user's notebook,
//!   - a notebook created under workspace A is 404 through workspace B's path
//!     (cross-tenant IDOR closed),
//!   - a non-owner `ws_editor` member PATCH/DELETE/trash a notebook they don't
//!     own → 2xx (role-based edit); a `ws_viewer` is denied writes (403),
//!   - cell add/update/delete/reorder happy path for the owner; a cell of
//!     notebook X addressed under notebook Y → 404 (nested IDOR closed),
//!   - unknown `cell_type` on add → 400.
//!
//! Each test receives a fresh, auto-migrated scratch database from `#[sqlx::test]`
//! and never touches the shared dev `egret` database.

use std::sync::Arc;

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

// ── Fake EngineTokenMinter ───────────────────────────────────────────────────

/// Returns a canned engine JWT (no IdP in unit tests).
struct FakeEngineMinter;

#[async_trait::async_trait]
impl egret_notebook::engine_token::EngineTokenMinter for FakeEngineMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<egret_notebook::engine_token::EngineToken, egret_notebook::engine_token::BrokerError>
    {
        Ok(egret_notebook::engine_token::EngineToken {
            access_token: "fake.engine.jwt".into(),
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(4102444800, 0).unwrap(),
        })
    }
}

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
    db: Db,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// Builds a real `CedarPdp` snapshotted against `db`'s already-migrated
/// `role_permissions` (the `notebook:read`/`notebook:write` grants seeded by
/// migration `0037_notebooks.sql` for `ws_admin`/`ws_editor`/`ws_viewer`).
/// Almost every test below asserts a 403, and most of those are the
/// *ownership* checks in `read_scoped`/`write_scoped` that run only after the
/// PDP gate passes — an allow-all stub would let those still exercise
/// correctly, but a real PDP is what the migrated `require()` call site now
/// actually depends on, so we build it for real here.
async fn build_pdp(db: &Db) -> std::sync::Arc<dyn egret_core::authz::Pdp> {
    std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn setup(pool: sqlx::PgPool) -> Fixture {
    let db = Db { pool };

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

    let pdp = build_pdp(&db).await;

    let app = notebook_router().with_state(NotebookState {
        store: NotebookStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces,
        authz: Authorizer::new(db.clone()),
        pdp,
        users: UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(FakeEngineMinter),
    });

    Fixture {
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
        db,
    }
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

#[sqlx::test(migrations = "../../migrations")]
async fn create_and_get_owner_sees_seeded_cell(pool: sqlx::PgPool) {
    let f = setup(pool).await;
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

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_member_reads_any_notebook(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_other_uid, other_tok) = add_member(&f, f.ws_a, "other", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // Role-based: a non-owner member GETs a notebook they don't own (never made
    // public) → 200 with is_owner=false. Private-by-default is gone.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &other_tok, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "non-owner member reads any workspace notebook (role-based)"
    );
    assert_eq!(
        json["is_owner"],
        serde_json::json!(false),
        "non-owner viewer => is_owner false"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn platform_admin_has_override(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_admin_uid, admin_tok) = add_platform_admin(&f, "admin").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // Object-scoped now: the platform_admin's unrestricted platform grant
    // cascades to objects (Object → Folder → Workspace → Platform), so it reads
    // and writes any notebook — consistent with BI content.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &admin_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "platform_admin reads another user's notebook (override applies)"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &admin_tok,
            Some(serde_json::json!({ "name": "renamed-by-admin" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "platform_admin patches another user's notebook (override applies)"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &admin_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "platform_admin deletes another user's notebook (override applies)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn cross_workspace_get_is_404(pool: sqlx::PgPool) {
    let f = setup(pool).await;
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

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_editor_writes_but_viewer_denied(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    // A non-owner ws_editor member and a non-owner ws_viewer member.
    let (_editor_uid, editor_tok) = add_member(&f, f.ws_a, "editor", "ws_editor").await;
    let (_viewer_uid, viewer_tok) = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // ws_viewer lacks notebook:write → PATCH forbidden (403), even though it can read.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &viewer_tok,
            Some(serde_json::json!({ "name": "nope" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer must NOT patch (lacks notebook:write)"
    );

    // A non-owner ws_editor member PATCHes a notebook they don't own → 200
    // (role-based edit, owner gate removed).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &editor_tok,
            Some(serde_json::json!({ "name": "renamed-by-editor" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "non-owner ws_editor member patches any notebook (role-based)"
    );

    // …and DELETEs it → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &editor_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "non-owner ws_editor member deletes any notebook (role-based)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn cell_crud_happy_path_and_nested_idor(pool: sqlx::PgPool) {
    let f = setup(pool).await;
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

/// PDP-backed workspace authz (this seam migrated onto
/// `egret_core::authz::require`): a `ws_viewer` member — granted
/// `notebook:read` but NOT `notebook:write` by migration `0037_notebooks.sql`
/// — can GET a public notebook (200, resolved by the real `CedarPdp`), but is
/// denied (403) on a PATCH before the ownership check (`write_scoped`) even
/// runs. This exercises the migrated `require()` call site itself, not just
/// the app-level ownership rules the other tests above cover.
#[sqlx::test(migrations = "../../migrations")]
async fn pdp_backed_read_write_gate(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_viewer_uid, viewer_tok) = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;

    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}", f.slug_a);

    // Owner publishes it so the viewer's read passes the ownership check too.
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

    // ws_viewer has notebook:read → GET succeeds (200).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &viewer_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "ws_viewer has notebook:read; GET a public notebook must succeed"
    );

    // ws_viewer lacks notebook:write → PATCH is denied at the PDP gate (403),
    // before write_scoped's ownership check ever runs.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            &viewer_tok,
            Some(serde_json::json!({ "name": "hijacked" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer lacks notebook:write; PATCH must be denied by the PDP gate"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_cell_type_is_400(pool: sqlx::PgPool) {
    let f = setup(pool).await;
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

#[sqlx::test(migrations = "../../migrations")]
async fn trash_hides_and_restore_reappears(pool: sqlx::PgPool) {
    let f = setup(pool).await;
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

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_editor_can_trash_but_viewer_cannot(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let (_editor_uid, editor_tok) = add_member(&f, f.ws_a, "editor", "ws_editor").await;
    let (_viewer_uid, viewer_tok) = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let trash_uri = format!("/ws/{}/notebooks/{id}/trash", f.slug_a);

    // ws_viewer lacks notebook:write → trash forbidden (403).
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &trash_uri, &viewer_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer must NOT trash (lacks notebook:write)"
    );

    // A non-owner ws_editor member trashes a notebook they don't own → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &trash_uri, &editor_tok, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "non-owner ws_editor member trashes any notebook (role-based)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bookmark_is_per_user_and_allowed_on_public(pool: sqlx::PgPool) {
    let f = setup(pool).await;
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
        "INSERT INTO analytics.folders (workspace_id, entity_type, name) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(ws_id.0)
    .bind(entity_type)
    .bind(format!("F-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert folder")
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_folder_happy_path_and_guards(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_owner_uid, owner_tok) = add_member(&f, f.ws_a, "owner", "ws_editor").await;
    let id = create_nb(&f, &f.slug_a, &owner_tok).await;
    let uri = format!("/ws/{}/notebooks/{id}/folder", f.slug_a);

    // Happy path: a notebook folder in this workspace → 204, folder_id set.
    let good = create_folder(&f.db, f.ws_a, "notebook").await;
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
    let wrong_type = create_folder(&f.db, f.ws_a, "dashboard").await;
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
    let other_ws = create_folder(&f.db, f.ws_b, "notebook").await;
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

// ── Notebook settings HTTP tests ─────────────────────────────────────────────

async fn seed_database_http(db: &Db, ws_id: WorkspaceId) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1, 'trino', 'localhost', 8080, $2) RETURNING id",
    )
    .bind(format!("httpdb-{}", Uuid::new_v4().simple()))
    .bind(ws_id.0)
    .fetch_one(&db.pool)
    .await
    .expect("insert database for http test")
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_settings_returns_engines_with_enabled_flags(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_uid, token) = add_member(&f, f.ws_a, "viewer", "ws_viewer").await;
    let db_id = seed_database_http(&f.db, f.ws_a).await;

    // GET settings → 200, engine present with enabled=false.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "GET settings; body={json}");
    let engines = json["engines"].as_array().expect("engines array");
    let found = engines.iter().find(|e| e["id"] == db_id.to_string());
    assert!(found.is_some(), "seeded engine appears in settings");
    assert_eq!(found.unwrap()["enabled"], serde_json::json!(false));
    let _ = db_id;
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_settings_enables_and_get_reflects(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_uid, token) = add_member(&f, f.ws_a, "admin", "ws_admin").await;
    let db_id = seed_database_http(&f.db, f.ws_a).await;

    // PUT → 200 with engines[0].enabled=true.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &token,
            Some(serde_json::json!({ "engine_ids": [db_id] })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "PUT settings; body={json}");
    let engines = json["engines"].as_array().unwrap();
    let e = engines
        .iter()
        .find(|e| e["id"] == db_id.to_string())
        .unwrap();
    assert_eq!(
        e["enabled"],
        serde_json::json!(true),
        "engine enabled after PUT"
    );

    // GET also shows it enabled.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "GET after PUT; body={json}");
    let engines = json["engines"].as_array().unwrap();
    let e = engines
        .iter()
        .find(|e| e["id"] == db_id.to_string())
        .unwrap();
    assert_eq!(e["enabled"], serde_json::json!(true));
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_settings_removes_engine_on_second_put(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (_uid, token) = add_member(&f, f.ws_a, "admin2", "ws_admin").await;
    let db_id = seed_database_http(&f.db, f.ws_a).await;

    // Enable.
    f.app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &token,
            Some(serde_json::json!({ "engine_ids": [db_id] })),
        ))
        .await
        .unwrap();

    // Remove by sending empty engine_ids.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &token,
            Some(serde_json::json!({ "engine_ids": [] })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "PUT empty; body={json}");
    let engines = json["engines"].as_array().unwrap();
    if let Some(e) = engines.iter().find(|e| e["id"] == db_id.to_string()) {
        assert_eq!(e["enabled"], serde_json::json!(false), "should be disabled");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_settings_cross_workspace_engine_rejected(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let (uid, token) = add_member(&f, f.ws_a, "adminx", "ws_admin").await;
    // Also member of ws_b so WsContext works for both paths.
    f.members
        .add(f.ws_b, "user", uid.0, "ws_admin", f.owner)
        .await
        .expect("member of ws_b");
    // Create a database in ws_b.
    let db_b = seed_database_http(&f.db, f.ws_b).await;

    // Try to enable ws_b's database in ws_a → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &token,
            Some(serde_json::json!({ "engine_ids": [db_b] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "cross-workspace engine id must be rejected with 400"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_settings_requires_manage_perm(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    // ws_viewer role does NOT have workspace:manage_members.
    let (_uid, viewer_tok) = add_member(&f, f.ws_a, "viewer2", "ws_viewer").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &viewer_tok,
            Some(serde_json::json!({ "engine_ids": [] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer must not PUT notebook settings"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_settings_allowed_for_notebook_read_member(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    // ws_viewer has notebook:read (from role_permissions seed).
    let (_uid, viewer_tok) = add_member(&f, f.ws_a, "viewer3", "ws_viewer").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            &viewer_tok,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "ws_viewer has notebook:read → GET notebook-settings must be 200"
    );
}

// ── Runtime endpoint tests ───────────────────────────────────────────────────

/// Seed a database (engine) row and return its id.
async fn seed_engine_runtime(db: &Db, ws_id: WorkspaceId) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1, 'trino', 'localhost', 8080, $2) RETURNING id",
    )
    .bind(format!("rt-eng-{}", Uuid::new_v4().simple()))
    .bind(ws_id.0)
    .fetch_one(&db.pool)
    .await
    .expect("insert engine for runtime test")
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_runtime_returns_manifest_with_enabled_engines_only(pool: sqlx::PgPool) {
    let f = setup(pool).await;

    // Seed a member with notebook:read (ws_viewer has this via role_permissions seed).
    let (_uid, token) = add_member(&f, f.ws_a, "rtviewer", "ws_viewer").await;

    // Seed TWO engines, enable only ONE via the settings PUT.
    let eng_enabled = seed_engine_runtime(&f.db, f.ws_a).await;
    let eng_disabled = seed_engine_runtime(&f.db, f.ws_a).await;

    // Enable only eng_enabled.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{}/notebook-settings", f.slug_a),
            // ws_admin token needed for the PUT (manage_members perm).
            // Re-add the same user as ws_admin for the PUT call.
            &{
                let (_admin_uid, admin_tok) = add_member(&f, f.ws_a, "rt-admin", "ws_admin").await;
                admin_tok
            },
            Some(serde_json::json!({
                "engine_ids": [eng_enabled]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PUT settings to enable one engine"
    );

    // GET /ws/{ws}/notebook-runtime → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebook-runtime", f.slug_a),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "GET notebook-runtime; body={json}");

    // username must be present and non-empty.
    let username = json["username"].as_str().expect("username in response");
    assert!(!username.is_empty(), "username must be non-empty");

    // engines: must contain ONLY the enabled one (not the disabled one).
    let engines = json["engines"].as_array().expect("engines array");
    let ids: Vec<&str> = engines.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(
        ids.contains(&eng_enabled.to_string().as_str()),
        "enabled engine must appear in runtime engines; ids={ids:?}"
    );
    assert!(
        !ids.contains(&eng_disabled.to_string().as_str()),
        "disabled engine must NOT appear in runtime engines; ids={ids:?}"
    );
    assert_eq!(engines.len(), 1, "exactly one enabled engine; body={json}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_runtime_non_member_is_403_or_404(pool: sqlx::PgPool) {
    let f = setup(pool).await;

    // A user with NO membership in ws_a.
    let outsider = f
        .users
        .insert_local(
            &format!("outsider_{}@egret.test", Uuid::new_v4().simple()),
            "Outsider",
            "x",
        )
        .await
        .expect("insert outsider");
    let outsider_tok = f
        .sessions
        .create(outsider, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/notebook-runtime", f.slug_a),
            &outsider_tok,
            None,
        ))
        .await
        .unwrap();

    // WsContext returns 404 for unknown workspace membership; exact code may vary.
    let status = resp.status();
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "non-member must get 403 or 404; got {status}"
    );
}
