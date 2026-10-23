//! Catalog resource-tag control-plane endpoints (Access P3.2). DB-gated;
//! self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Builds a REAL `CedarPdp` after `migrate()`.
//!
//! Proves: a platform_admin (holds `catalog_registry:update`/`catalog_registry:read`
//! via the PDP bypass) PUTs a table-level and a column-level tag → 200; GET
//! `?schema=&table=` lists BOTH (distinguishing table vs column); DELETE removes
//! one → 200 (GET then omits it). A plain user (no catalog_registry perm) PUT → 403.
//! A warehouse or classification id that names nothing → a coded 404.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService, TagRepo,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        saved: SavedQueryRepo::new(db.clone()),
        schema: SchemaService::new(),
        databases: egret_query::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        charts: ChartRepo::new(db.clone()),
        datasets: DatasetRepo::new(db.clone()),
        dashboards: DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

/// Insert a user; when `admin` grant it platform_admin (PDP bypass).
async fn seed_user(db: &Db, admin: bool) -> Uuid {
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    if admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid)
        .execute(&db.pool)
        .await
        .expect("grant platform_admin");
    }
    uid
}

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn resource_tag_assign_list_unassign_and_gating() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let admin = seed_user(&db, true).await;
    let plain = seed_user(&db, false).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    // Two tags: one for the table, one for a column.
    let tags = TagRepo::new(db.clone());
    let tbl_tag = tags
        .create(
            &format!("pii-{}", Uuid::new_v4().simple()),
            "PII",
            "Personal data",
            "#ff0000",
            Some(admin),
        )
        .await
        .expect("create table tag");
    let col_tag = tags
        .create(
            &format!("secret-{}", Uuid::new_v4().simple()),
            "Secret",
            "Secret column",
            "#00ff00",
            Some(admin),
        )
        .await
        .expect("create column tag");

    let wh = egret_test_db::seed_warehouse(&db.pool).await;
    let admin_tok = session_for(&sessions, admin).await;
    let plain_tok = session_for(&sessions, plain).await;

    let app = egret_bi::resource_tags_router().with_state(st);
    let uri = format!("/control-plane/warehouses/{wh}/resource-tags");

    // 1. platform_admin assigns a TABLE-level tag (no column) → 200.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&uri)
                .header("Cookie", format!("egret_session={admin_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "analytics", "table": "orders", "tag_id": tbl_tag.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "platform_admin must assign a table tag; got {s}: {}",
        body_string(resp).await
    );

    // 2. platform_admin assigns a COLUMN-level tag → 200.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&uri)
                .header("Cookie", format!("egret_session={admin_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "analytics", "table": "orders",
                        "column": "email", "tag_id": col_tag.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "assign column tag");

    // 3. GET lists BOTH: the table tag (empty column) and the column tag.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{uri}?schema=analytics&table=orders"))
                .header("Cookie", format!("egret_session={admin_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_string(resp).await;
    assert!(
        listed.contains(&tbl_tag.id.to_string()) && listed.contains(&col_tag.id.to_string()),
        "GET must list both the table and column tags; got {listed}"
    );
    assert!(
        listed.contains("\"column\":\"email\"") && listed.contains("\"column\":\"\""),
        "GET must distinguish the column tag (email) from the table tag (empty column); got {listed}"
    );

    // 3a. BOUNDARY: the wire shape MUST nest the tag under `tag` (the FE reads
    // `a.tag.id` — a flat DTO would crash the browse UI). Deserialize and assert
    // the nested structure by field, not by substring.
    let parsed: serde_json::Value = serde_json::from_str(&listed).expect("resource-tags is JSON");
    let arr = parsed.as_array().expect("resource-tags is a JSON array");
    let col_entry = arr
        .iter()
        .find(|e| e["column"] == "email")
        .expect("an entry for the email column");
    assert_eq!(
        col_entry["tag"]["id"],
        serde_json::json!(col_tag.id.to_string()),
        "the tag must be nested under `tag` with the column tag's id; got {col_entry}"
    );
    assert_eq!(
        col_entry["tag"]["name"], "Secret",
        "nested `tag.name` must be the column tag's name; got {col_entry}"
    );
    assert_eq!(
        col_entry["column"], "email",
        "the entry's column coordinate must be `email`; got {col_entry}"
    );
    assert!(
        col_entry.get("tag_id").is_none() && col_entry["tag"].get("key").is_some(),
        "the DTO must be nested (no flat `tag_id`; `tag` carries the full tag shape); got {col_entry}"
    );

    // 4. A plain user (no warehouse perm) PUT → 403.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&uri)
                .header("Cookie", format!("egret_session={plain_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "analytics", "table": "orders", "tag_id": tbl_tag.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a user without catalog_registry:update must NOT tag a catalog resource"
    );

    // 5. DELETE the column tag → 200, then GET no longer lists it (table tag stays).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&uri)
                .header("Cookie", format!("egret_session={admin_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "analytics", "table": "orders",
                        "column": "email", "tag_id": col_tag.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "unassign column tag");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{uri}?schema=analytics&table=orders"))
                .header("Cookie", format!("egret_session={admin_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_string(resp).await;
    assert!(
        !listed.contains(&col_tag.id.to_string()) && listed.contains(&tbl_tag.id.to_string()),
        "after unassign, GET drops the column tag but keeps the table tag; got {listed}"
    );
}

/// **SP4d debt Task 5, at the route, in the direction the obvious
/// implementation misses: the ATTACHMENT is created second.**
///
/// A guard that only refused "a tag-addressed MASK whose tag is already on a
/// whole table" leaves the hole wide open — the steward authors the mask first
/// (legal at that moment) and attaches the tag to the table afterwards, and in
/// this programme "two guards that must agree, written to different shapes" has
/// produced a defect four times. So this is the `PUT …/resource-tags` half, and
/// it is run against BOTH surfaces that can author the mask: a `perm.rules` row
/// and a hand-authored tag masking policy. The second is not the hypothetical
/// one — when SP4d-2b's live gate swept this development host, every live
/// instance of the pair it found came from the policy editor and none from
/// `perm.rules`. The count is not restated: that sweep's database was dropped
/// between sessions and a re-measurement on 2026-10-10 finds 0 pairs across the
/// 9 databases carrying both schemas, so the figure cannot be reproduced
/// (`egret_policy::unreadable_table_mask`'s module doc records both the old
/// reading and the new one). The editor half earns its coverage from being
/// reachable, not from a population count.
///
/// Four assertions per surface, and the last two are the ones a DB-level test
/// cannot make: the refusal is a 400, the body carries the remedies verbatim
/// (`CatalogTagEditor` renders `assign.error.message` straight into the page),
/// nothing was stored, and a COLUMN-level assignment of the very same tag still
/// succeeds — the remedy the message names, through the same route.
#[tokio::test]
async fn a_table_level_assignment_is_refused_when_a_tag_mask_names_the_tag() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let admin = seed_user(&db, true).await;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let token = session_for(&sessions, admin).await;
    let app = egret_bi::resource_tags_router().with_state(st);
    let wh = egret_test_db::seed_warehouse(&db.pool).await;
    let uri = format!("/control-plane/warehouses/{wh}/resource-tags");

    let put = |column: Option<&str>, tag_id: Uuid, table: &str| {
        let mut body = serde_json::json!({ "schema": "sales", "table": table, "tag_id": tag_id });
        if let Some(c) = column {
            body["column"] = serde_json::json!(c);
        }
        Request::builder()
            .method("PUT")
            .uri(&uri)
            .header("Cookie", format!("egret_session={token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    let tags = TagRepo::new(db.clone());
    for surface in ["rule", "policy"] {
        // A tag of its own per surface. Its `name` differs from its `key` on
        // purpose: `perm.rules.tag_key` references the KEY and a tag policy
        // addresses the NAME, so a fixture where they coincide cannot tell a
        // key/name confusion in the guard from a correct answer.
        let suffix = Uuid::new_v4().simple().to_string();
        let tag = tags
            .create(
                &format!("t5-k-{surface}-{suffix}"),
                &format!("T5-N-{surface}-{suffix}"),
                "",
                "",
                Some(admin),
            )
            .await
            .expect("create tag");

        // The MASK half, written the way that surface writes it.
        let mask_name = format!("t5-{surface}-{suffix}");
        match surface {
            "rule" => {
                sqlx::query(
                    "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, tag_key) \
                     VALUES ($1, 'mask', 'MASK_HASH', 'tag', $2)",
                )
                .bind(&mask_name)
                .bind(&tag.key)
                .execute(&db.pool)
                .await
                .expect("tag mask rule");
            }
            _ => {
                // `policy.authz_services.type` FK-references `authz_service_defs`,
                // so the tag def has to exist before a tag service can.
                egret_policy::seed_tag(&egret_policy::PolicyStore::new(db.clone()))
                    .await
                    .expect("seed tag service-def");
                let svc: i64 = sqlx::query_scalar(
                    "INSERT INTO policy.authz_services (name, type, config_jsonb) \
                     VALUES ($1, 'tag', '{}'::jsonb) RETURNING id",
                )
                .bind(format!("t5-tagsvc-{suffix}"))
                .fetch_one(&db.pool)
                .await
                .expect("tag service");
                sqlx::query(
                    "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
                     VALUES ($1, $2, jsonb_build_object( \
                         'policyType', 1, 'serviceType', 'tag', \
                         'resources', jsonb_build_object('tag', \
                             jsonb_build_object('values', jsonb_build_array($3::text)))))",
                )
                .bind(svc)
                .bind(&mask_name)
                .bind(&tag.name)
                .execute(&db.pool)
                .await
                .expect("tag masking policy");
            }
        }

        // The TABLE-level half, through the route → 400.
        let resp = app
            .clone()
            .oneshot(put(None, tag.id, "orders"))
            .await
            .unwrap();
        let status = resp.status();
        let body = body_string(resp).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the {surface} surface's mask must make a table-level assignment a 400; got {body}"
        );
        let json: serde_json::Value = serde_json::from_str(&body).expect("json body");
        assert_eq!(json["code"], "table_tag_mask_conflict", "{surface}: {json}");
        assert_eq!(
            json["maskOrigin"],
            if surface == "rule" { "rule" } else { "policy" }
        );
        assert_eq!(json["tagId"], tag.id.to_string());
        assert!(
            json["warehouseId"].is_null(),
            "the attachment in flight is the table-level one"
        );
        assert!(
            body.contains("sales.orders"),
            "the refusal must name the table that would become unreadable; got {body}"
        );
        assert!(
            body.contains(&mask_name),
            "the refusal must name the {surface} the steward has to edit; got {body}"
        );
        assert!(
            body.contains("to the columns that need masking instead of to the table")
                && body.contains("by catalog/schema/table/column path instead of by tag"),
            // Task 16 added a THIRD remedy to the shared sentence; this still asserts
            // the two an ATTACHMENT writer can act on.
            "the refusal must carry both attachment-side remedies; got {body}"
        );

        // Nothing was stored — the assertion a status-only test cannot make.
        let stored: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog_tag_assignments WHERE tag_id = $1 AND column_name = ''",
        )
        .bind(tag.id)
        .fetch_one(&db.pool)
        .await
        .expect("count");
        assert_eq!(stored, 0, "a refused assignment must not be stored");

        // And the remedy works through the same route: attach the tag to the
        // COLUMN instead. ⚠ If this ever goes red the guard has started
        // refusing the sentinel's legitimate sibling — the configuration every
        // live attachment on this host actually uses.
        let resp = app
            .clone()
            .oneshot(put(Some("email"), tag.id, "orders"))
            .await
            .unwrap();
        let status = resp.status();
        let body = body_string(resp).await;
        assert!(
            status.is_success(),
            "the column-level remedy must be accepted; got {status}: {body}"
        );
    }
}

/// Spec §11.1: `catalog_tag_assignments.warehouse_id` has no foreign key, so a
/// PUT naming a warehouse that does not exist answered 200 and stored a row no
/// catalog serves; a `tag_id` naming no classification hit the tag foreign key
/// and answered 500. Both are a coded 404 now, on PUT and DELETE alike, and
/// nothing is stored.
#[tokio::test]
async fn an_unknown_warehouse_or_classification_is_a_coded_404() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let admin = seed_user(&db, true).await;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let token = session_for(&st.sessions, admin).await;
    let app = egret_bi::resource_tags_router().with_state(st);
    let suffix = Uuid::new_v4().simple().to_string();
    let tag = TagRepo::new(db.clone())
        .create(
            &format!("nf-{suffix}"),
            &format!("NF {suffix}"),
            "",
            "",
            Some(admin),
        )
        .await
        .expect("create tag");
    let real_wh = egret_test_db::seed_warehouse(&db.pool).await;
    let (no_wh, no_tag) = (Uuid::new_v4(), Uuid::new_v4());

    for (method, wh, tag_id, code, field, id) in [
        (
            "PUT",
            no_wh,
            tag.id,
            "warehouse_not_found",
            "warehouseId",
            no_wh,
        ),
        (
            "DELETE",
            no_wh,
            tag.id,
            "warehouse_not_found",
            "warehouseId",
            no_wh,
        ),
        ("PUT", real_wh, no_tag, "tag_not_found", "tagId", no_tag),
        ("DELETE", real_wh, no_tag, "tag_not_found", "tagId", no_tag),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/control-plane/warehouses/{wh}/resource-tags"))
                    .header("Cookie", format!("egret_session={token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "schema": "sales", "table": "orders", "tag_id": tag_id
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body: serde_json::Value =
            serde_json::from_str(&body_string(resp).await).expect("JSON body");
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {code}: {body}");
        assert_eq!(body["code"], code, "{method}: {body}");
        assert_eq!(body[field], id.to_string(), "{method}: {body}");
        assert!(body["error"].is_string(), "{method}: {body}");
    }

    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_tag_assignments WHERE warehouse_id = $1 OR tag_id = $2",
    )
    .bind(no_wh)
    .bind(no_tag)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(stored, 0, "a refused write stores nothing");
}
