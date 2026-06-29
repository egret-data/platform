//! Workspace-admin catalog-token HTTP route tests (Workspace Admin track,
//! ADR-19 layer 2). DB-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the list/mint/revoke routes added to the browse router
//! (`egret_catalog_engine::browse_router`):
//!   - a `ws_admin` member (holds `workspace:manage_members`) can mint, list,
//!     and revoke tokens,
//!   - the raw secret / hash is never echoed back in the list response,
//!   - a non-admin member (`ws_viewer`) is Forbidden (403),
//!   - revoking a token that belongs to ANOTHER workspace via this workspace's
//!     path is a 404 (cross-tenant IDOR closed),
//!   - malformed permission requests are 400.
//!
//! Tokens are now scoped to a specific warehouse, so the routes live under
//! `/ws/{ws}/catalog/warehouses/{wid}/tokens`. The harness seeds a warehouse ROW
//! directly via the store (`insert_warehouse`, no lakekeeper/MinIO needed) — the
//! token routes themselves never touch lakekeeper — then drives the browse router
//! with an authenticated cookie.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test catalog_tokens_api

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    browse_router, build_state, ensure_bootstrapped, run_lakekeeper_migrations, BrowseState,
};
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

struct Fixture {
    app: axum::Router,
    store: CatalogStore,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    db: Db,
    slug: String,
    workspace_id: WorkspaceId,
    owner: UserId,
    /// The egret warehouse-row id the token routes are scoped under (`{wid}`).
    wid: Uuid,
}

async fn into_json(resp: Response) -> (StatusCode, Vec<u8>, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, bytes.to_vec(), json)
}

async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // lakekeeper schema + default project, so build_state succeeds.
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap");

    let store = CatalogStore::new(db.clone());
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("tokowner-{suffix}@egret.test"), "Tok Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("tokws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Token Admin Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    // Seed a warehouse ROW (the token routes are scoped under `{wid}`). These
    // routes never touch lakekeeper, so the lakekeeper UUID is arbitrary.
    let wid = store
        .insert_warehouse(workspace_id.0, Uuid::now_v7(), "default", None)
        .await
        .expect("seed warehouse row")
        .id;

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(db.clone(), secret_store),
        system_user: SYSTEM_USER.into(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        store,
        sessions,
        users,
        members: MemberRepo::new(db.clone()),
        db,
        slug,
        workspace_id,
        owner,
        wid,
    })
}

/// Add a new user as a workspace member with `role`, returning a session token.
async fn add_member(f: &Fixture, email_tag: &str, role: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(f.workspace_id, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Seed a second workspace Y and mint a token in it directly via the store.
/// Returns the token id (to be attacked across the tenant boundary).
async fn seed_other_workspace_token(f: &Fixture) -> Uuid {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("toky-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("toky{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Token Other", owner)
        .await
        .expect("seed other workspace");
    let y_wid = f
        .store
        .insert_warehouse(ws.id.0, Uuid::now_v7(), "default", None)
        .await
        .expect("seed Y warehouse")
        .id;
    let (token, _raw) = f
        .store
        .create_token(
            ws.id.0,
            y_wid,
            "y-token",
            &["catalog:read".into()],
            &[],
            &[],
        )
        .await
        .expect("mint Y token");
    token.id
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

#[tokio::test]
async fn admin_can_mint_list_and_revoke_tokens() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    // MINT
    let body = serde_json::json!({
        "label": "ci",
        "permissions": ["read", "write"],
        "namespace_scope": ["marts"],
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &token, Some(body)))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "mint; body={json}");
    assert!(
        json["raw"].as_str().is_some_and(|s| !s.is_empty()),
        "mint returns a non-empty raw token; body={json}"
    );
    assert_eq!(
        json["token"]["permissions"],
        serde_json::json!(["catalog:read", "catalog:write"]),
        "perms mapped to catalog:* ; body={json}"
    );
    assert_eq!(
        json["token"]["namespace_scope"],
        serde_json::json!(["marts"]),
        "namespace_scope preserved; body={json}"
    );
    let minted_id = json["token"]["id"].as_str().unwrap().to_string();
    let raw_secret = json["raw"].as_str().unwrap().to_string();

    // LIST — token present, NO secret/hash leaked.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list; body={json}");
    let listed = json
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"].as_str() == Some(minted_id.as_str()));
    assert!(listed, "minted token listed; body={json}");
    let raw_str = String::from_utf8_lossy(&bytes);
    assert!(!raw_str.contains("hash"), "no `hash` field in list JSON");
    assert!(
        !raw_str.contains("token_hash"),
        "no `token_hash` field in list JSON"
    );
    assert!(
        !raw_str.contains(&raw_secret),
        "raw secret must never be echoed in list"
    );

    // REVOKE
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{base}/{minted_id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "revoke → 204");

    // LIST again — gone.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let still = json
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"].as_str() == Some(minted_id.as_str()));
    assert!(!still, "revoked token no longer listed; body={json}");
}

#[tokio::test]
async fn revoke_of_foreign_workspace_token_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let foreign_id = seed_other_workspace_token(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!(
                "/ws/{}/catalog/warehouses/{}/tokens/{}",
                f.slug, f.wid, foreign_id
            ),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "revoking another workspace's token must 404 (IDOR closed)"
    );
}

/// A token minted FOR warehouse B (same workspace) must NOT be revocable via
/// warehouse A's `{wid}` path — the store-layer `warehouse_id` guard scopes the
/// revoke to the warehouse, not just the workspace.
#[tokio::test]
async fn revoke_of_other_warehouse_token_is_404_and_token_survives() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;

    // A SECOND warehouse (B) in the SAME workspace, and a token minted for B.
    let wid_b = f
        .store
        .insert_warehouse(f.workspace_id.0, Uuid::now_v7(), "warehouse-b", None)
        .await
        .expect("seed warehouse B")
        .id;
    let (token_b, _raw) = f
        .store
        .create_token(
            f.workspace_id.0,
            wid_b,
            "b-token",
            &["catalog:read".into()],
            &[],
            &[],
        )
        .await
        .expect("mint token for warehouse B");

    // Attempt to revoke B's token via warehouse A's path (`f.wid`) → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!(
                "/ws/{}/catalog/warehouses/{}/tokens/{}",
                f.slug, f.wid, token_b.id
            ),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a token from warehouse B must not be revocable via warehouse A's path"
    );

    // And B's token is STILL active (listed under warehouse B).
    let still = f
        .store
        .list_tokens_for_warehouse(f.workspace_id.0, wid_b)
        .await
        .expect("list warehouse B tokens")
        .iter()
        .any(|t| t.id == token_b.id);
    assert!(
        still,
        "B's token must remain active after the failed revoke"
    );
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    // GET (list) → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not list catalog tokens"
    );

    // POST (mint) → 403. The admin gate runs before body validation, so any
    // body must still be rejected with 403 (not 400).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "x",
                "permissions": ["read"],
                "namespace_scope": [],
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not mint catalog tokens"
    );

    // DELETE (revoke) → 403, before any store lookup.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{base}/{}", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not revoke catalog tokens"
    );
}

#[tokio::test]
async fn mint_rejects_bad_permissions() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    // Unknown permission → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "x",
                "permissions": ["bogus"],
                "namespace_scope": [],
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown permission → 400"
    );

    // Empty permissions → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "x",
                "permissions": [],
                "namespace_scope": [],
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty permissions → 400"
    );
}

#[tokio::test]
async fn mint_dedups_duplicate_permissions() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "dup",
                "permissions": ["read", "read"],
                "namespace_scope": [],
            })),
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "mint; body={json}");
    assert_eq!(
        json["token"]["permissions"],
        serde_json::json!(["catalog:read"]),
        "duplicate input perms collapse to a single mapped perm; body={json}"
    );
}

/// Minting with a `table_scope` persists it and lists it back on the DTO.
#[tokio::test]
async fn mint_with_table_scope_persists_and_lists() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "scoped",
                "permissions": ["read"],
                "namespace_scope": ["sale"],
                "table_scope": ["sale.orders"],
            })),
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "mint; body={json}");
    assert_eq!(
        json["token"]["table_scope"],
        serde_json::json!(["sale.orders"]),
        "table_scope persisted on mint; body={json}"
    );
    let id = json["token"]["id"].as_str().unwrap().to_string();

    // LIST reflects the table_scope on the row.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list; body={json}");
    let row = json
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(id.as_str()))
        .expect("minted row listed");
    assert_eq!(
        row["table_scope"],
        serde_json::json!(["sale.orders"]),
        "table_scope listed; body={json}"
    );
}

/// A single-segment table scope entry (no namespace) is rejected with 400.
#[tokio::test]
async fn mint_rejects_malformed_table_scope() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "bad",
                "permissions": ["read"],
                "table_scope": ["orders"],
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a table_scope entry without a namespace must 400"
    );
}

/// PUT edits label + permissions + namespace_scope + table_scope in place; GET
/// reflects the change and the ORIGINAL raw secret still authenticates with the
/// new scope (the hash is untouched).
#[tokio::test]
async fn edit_token_updates_scope() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    // MINT (read-only, ns=sale, table=sale.orders).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "before",
                "permissions": ["read"],
                "namespace_scope": ["sale"],
                "table_scope": ["sale.orders"],
            })),
        ))
        .await
        .unwrap();
    let (_s, _b, json) = into_json(resp).await;
    let id = json["token"]["id"].as_str().unwrap().to_string();
    let raw = json["raw"].as_str().unwrap().to_string();

    // PUT — widen perms, change ns + table scope, rename.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("{base}/{id}"),
            &token,
            Some(serde_json::json!({
                "label": "after",
                "permissions": ["read", "write"],
                "namespace_scope": ["marts"],
                "table_scope": ["marts.daily", "marts.weekly"],
            })),
        ))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "edit; body={json}");
    assert_eq!(json["label"], "after", "label updated; body={json}");
    assert_eq!(
        json["permissions"],
        serde_json::json!(["catalog:read", "catalog:write"]),
        "perms updated; body={json}"
    );
    assert_eq!(
        json["namespace_scope"],
        serde_json::json!(["marts"]),
        "ns scope updated; body={json}"
    );
    assert_eq!(
        json["table_scope"],
        serde_json::json!(["marts.daily", "marts.weekly"]),
        "table scope updated; body={json}"
    );

    // The ORIGINAL raw still authenticates and carries the NEW scope.
    let auth = f
        .store
        .authenticate_token(&raw)
        .await
        .expect("original secret still valid after edit");
    assert_eq!(auth.permissions, vec!["catalog:read", "catalog:write"]);
    assert_eq!(auth.namespace_scope, vec!["marts"]);
    assert_eq!(auth.table_scope, vec!["marts.daily", "marts.weekly"]);
}

/// POST rotate mints a NEW secret for the same row: the old raw stops
/// authenticating, the new raw authenticates with the same id + label + scopes.
#[tokio::test]
async fn rotate_token_invalidates_old_secret() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses/{}/tokens", f.slug, f.wid);

    // MINT.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "label": "rot",
                "permissions": ["read"],
                "namespace_scope": ["sale"],
                "table_scope": ["sale.orders"],
            })),
        ))
        .await
        .unwrap();
    let (_s, _b, json) = into_json(resp).await;
    let id = json["token"]["id"].as_str().unwrap().to_string();
    let raw1 = json["raw"].as_str().unwrap().to_string();

    // raw1 authenticates before rotation.
    assert!(
        f.store.authenticate_token(&raw1).await.is_ok(),
        "raw1 valid before rotate"
    );

    // ROTATE.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("{base}/{id}/rotate"), &token, None))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "rotate; body={json}");
    let raw2 = json["raw"].as_str().unwrap().to_string();
    assert!(!raw2.is_empty(), "rotate returns a new raw; body={json}");
    assert_ne!(raw1, raw2, "rotated secret differs from the original");
    assert_eq!(
        json["token"]["id"].as_str(),
        Some(id.as_str()),
        "same row id after rotate; body={json}"
    );
    assert_eq!(
        json["token"]["label"], "rot",
        "label preserved; body={json}"
    );
    assert_eq!(
        json["token"]["table_scope"],
        serde_json::json!(["sale.orders"]),
        "scope preserved across rotate; body={json}"
    );

    // raw1 no longer authenticates; raw2 does, with the SAME scope.
    assert!(
        f.store.authenticate_token(&raw1).await.is_err(),
        "old secret invalidated after rotate"
    );
    let auth = f
        .store
        .authenticate_token(&raw2)
        .await
        .expect("new secret authenticates");
    assert_eq!(auth.namespace_scope, vec!["sale"]);
    assert_eq!(auth.table_scope, vec!["sale.orders"]);
}

/// Editing or rotating a token that belongs to ANOTHER warehouse (same
/// workspace) via this warehouse's path is a 404 — the store-layer warehouse
/// guard scopes both mutations, closing the cross-warehouse IDOR.
#[tokio::test]
async fn edit_and_rotate_cross_warehouse_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;

    // Second warehouse B + a token minted for it.
    let wid_b = f
        .store
        .insert_warehouse(f.workspace_id.0, Uuid::now_v7(), "warehouse-b", None)
        .await
        .expect("seed warehouse B")
        .id;
    let (token_b, _raw) = f
        .store
        .create_token(
            f.workspace_id.0,
            wid_b,
            "b-token",
            &["catalog:read".into()],
            &[],
            &[],
        )
        .await
        .expect("mint token for warehouse B");

    // PUT via warehouse A's path → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!(
                "/ws/{}/catalog/warehouses/{}/tokens/{}",
                f.slug, f.wid, token_b.id
            ),
            &token,
            Some(serde_json::json!({
                "label": "hijack",
                "permissions": ["read", "write"],
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "editing warehouse B's token via warehouse A must 404"
    );

    // POST rotate via warehouse A's path → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!(
                "/ws/{}/catalog/warehouses/{}/tokens/{}/rotate",
                f.slug, f.wid, token_b.id
            ),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "rotating warehouse B's token via warehouse A must 404"
    );
}

/// Focused table-scope coverage: a table-scoped token round-trips its scope
/// through `authenticate_token` into `TokenAuth.table_scope`, which is exactly
/// what `front_auth` puts into the `"egret"` claim's `tables` field. The
/// authorizer's table-FQN allow/deny decision over that claim is unit-tested in
/// `authorizer.rs` (`gate_table_*` / `table_scope_*`). The FULL live REST path
/// (create two iceberg tables, load the allowed one + deny the sibling + filter
/// the list) requires lakekeeper + MinIO table creation and is deferred to the
/// manual real-Trino integration test.
#[tokio::test]
async fn table_scope_round_trips_into_token_auth() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (_token, raw) = f
        .store
        .create_token(
            f.workspace_id.0,
            f.wid,
            "tbl",
            &["catalog:read".into()],
            &["sale".into()],
            &["sale.orders".into()],
        )
        .await
        .expect("mint table-scoped token");

    let auth = f
        .store
        .authenticate_token(&raw)
        .await
        .expect("authenticate table-scoped token");
    assert_eq!(
        auth.table_scope,
        vec!["sale.orders".to_string()],
        "table_scope flows into TokenAuth (→ the `tables` front-auth claim)"
    );
    assert_eq!(auth.namespace_scope, vec!["sale".to_string()]);
}
