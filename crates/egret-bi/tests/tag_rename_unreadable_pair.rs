//! **SP4d debt Task 15: a tag RENAME is the third surface that can author the
//! unreadable-table pair, and Task 5's two guards do not sit on it.**
//!
//! Task 5 refused "a tag-addressed MASK plus a TABLE-level attachment of the
//! same tag" at three write paths (`POST/PATCH /perm/rules`,
//! `POST /policies` with `kind: "tag-masking"`, `PUT …/resource-tags`), because
//! the engine type-checks one tag mask against EVERY column of the scanned
//! table and the table becomes unreadable rather than masked — measured live on
//! Trino 470 (`egret_policy::unreadable_table_mask`'s module doc).
//!
//! The sequence this file drives creates that pair without going through either
//! guarded half:
//!
//! 1. `POST /policies`, `kind: "tag-masking"`, addressing tag NAME `SENSITIVE`
//!    — accepted, because an editor tag policy carries the name as a plain
//!    string (`tag_mask_addresses` reads `/resources/tag/values`) and no tag is
//!    called that yet.
//! 2. `PUT …/resource-tags` with no `column` for tag `PII` — accepted, because
//!    no mask names `PII`.
//! 3. rename `PII` → `SENSITIVE` through `TagRepo::update`, the repo method
//!    `PATCH /api/control-plane/tags/{id}` calls.
//!
//! **This file first ran in the opposite direction**, asserting the pair EXISTS
//! after step 3, to confirm the hole rather than assume it — see
//! `the_pair_the_rename_would_create_is_the_shape_the_engine_downloads` below,
//! which keeps that measurement as a comment with its verbatim output. The
//! assertions then changed direction, deliberately, once the rename took the
//! same guard.
//!
//! # Why this binary
//!
//! `egret-test-db` gives each test PROCESS a private database and a tag
//! rename recompiles every compiler-targeted profile in the installation (D5),
//! so a rename fixture that shared a binary with `tag_rename_recompiles.rs`
//! would churn that file's `tag_version` assertions from underneath it — the
//! same reason that file stands alone.
//!
//! It lives in `egret-bi` because that is where the renaming `UPDATE tags SET
//! name = …` is, and the crate reaches BOTH other surfaces as real
//! dependencies: `egret_policy::policy_admin_router` (the editor mask half) and
//! its own `resource_tags_router` (the attachment half), plus
//! `egret_policy::ranger_router`, which is how this file can show what the
//! engine's plugin actually downloads rather than only what egret stores.
//!
//! DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-bi --test tag_rename_unreadable_pair

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService, TagRepo,
};
use egret_metastore::Db;
use egret_policy::{
    policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
    PolicyStore, RangerState,
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const RANGER_TOKEN: &str = "devtoken";

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

// ---------- fixtures ----------

struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

/// `PolicyAdminState` needs the engine-binding seam; the editor mask half does
/// not read it (`resolve_tag_service_and_body` only touches `authz_services`),
/// so an empty reader is the honest stand-in rather than a convenience.
struct NoEngines;
#[async_trait::async_trait]
impl EngineBindingReader for NoEngines {
    async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError> {
        Ok(vec![])
    }
    async fn engine_catalog_names(&self, _e: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_schemas(&self, _w: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_tables(&self, _w: Uuid, _s: &str) -> Result<Vec<String>, EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_columns(
        &self,
        _w: Uuid,
        _s: &str,
        _t: &str,
    ) -> Result<Vec<String>, EngineReadError> {
        Ok(vec![])
    }
}

fn bi_state(db: &Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
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
        folders: egret_bi::folder::FolderRepo::new(db.clone()),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

/// A platform-admin user (PDP bypass) and a session token for it.
async fn admin_session(db: &Db, sessions: &egret_auth::session::SessionService) -> String {
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

struct Fixture {
    db: Db,
    /// `/policies` (the editor mask half) and `/control-plane/warehouses/…`
    /// (the attachment half), each the real router with real state.
    policies: axum::Router,
    resource_tags: axum::Router,
    /// The Ranger plugin's two download endpoints.
    ranger: axum::Router,
    tags: TagRepo,
    token: String,
    trino_name: String,
    tag_service_name: String,
    warehouse_id: Uuid,
}

/// Trino profile + its tag service + engine + registry catalog + warehouse —
/// everything `tag_assignments_for_service` needs to actually export a tag map,
/// so a download assertion cannot pass by exporting nothing.
async fn setup() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&store).await.expect("seed trino");
    egret_policy::seed_tag(&store).await.expect("seed tag");

    let trino_name = unique("t15trino");
    let tag_service_name = format!("{trino_name}_tag");
    store
        .create_service(
            &trino_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("trino profile");
    let tag_svc = store
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("t15-ws"))
            .bind("T15 WS")
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-t15','AKIATEST') RETURNING id",
    )
    .bind(unique("t15-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("t15_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("t15-eng"))
    .bind(
        store
            .get_service(&trino_name)
            .await
            .expect("get trino profile")
            .id,
    )
    .fetch_one(&pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("t15_cat"))
    .bind(warehouse_id)
    .fetch_one(&pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect("attach catalog");
    let _ = tag_svc;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = bi_state(&db, pdp.clone());
    let sessions = st.sessions.clone();
    let token = admin_session(&db, &sessions).await;
    let policies = policy_admin_router().with_state(PolicyAdminState {
        store: store.clone(),
        engines: Arc::new(NoEngines),
        users: egret_auth::users::UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions: sessions.clone(),
        engine_egret_url: "http://egret:8080".into(),
        pdp,
    });
    let resource_tags = egret_bi::resource_tags_router().with_state(st);
    let ranger = egret_policy::ranger_router(RangerState {
        store,
        auth_token: Arc::from(RANGER_TOKEN),
    });

    Some(Fixture {
        tags: TagRepo::new(db.clone()),
        db,
        policies,
        resource_tags,
        ranger,
        token,
        trino_name,
        tag_service_name,
        warehouse_id,
    })
}

impl Fixture {
    /// Step 1 — the editor mask half, through the real route.
    async fn create_tag_mask_policy(&self, name: &str, tag_name: &str) -> (StatusCode, String) {
        let body = serde_json::json!({
            "kind": "tag-masking", "name": name, "service": self.trino_name,
            "tag": tag_name, "isEnabled": true,
            "items": [{"groups": ["analysts"], "maskType": "MASK_HASH"}],
        });
        let resp = self
            .policies
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/policies")
                    .header("Cookie", format!("egret_session={}", self.token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        (status, body_string(resp).await)
    }

    /// Step 2 — the attachment half, through the real route. `column: None` is
    /// the TABLE-level sentinel (`column_name = ''`, migration 0081).
    async fn put_resource_tag(
        &self,
        tag_id: Uuid,
        schema: &str,
        table: &str,
        column: Option<&str>,
    ) -> (StatusCode, String) {
        let mut body =
            serde_json::json!({ "schema": schema, "table": table, "tag_id": tag_id.to_string() });
        if let Some(c) = column {
            body["column"] = serde_json::json!(c);
        }
        let resp = self
            .resource_tags
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/control-plane/warehouses/{}/resource-tags",
                        self.warehouse_id
                    ))
                    .header("Cookie", format!("egret_session={}", self.token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        (status, body_string(resp).await)
    }

    /// What the Ranger plugin downloads: the tag MAP for the polled tag
    /// service, and the trino profile's policies with the tag service's
    /// policies embedded as `tagPolicies` (which is what turns tag masking on
    /// in the plugin at all).
    async fn downloaded_tag_map(&self) -> serde_json::Value {
        self.ranger_get(&format!("/tags/download/{}", self.tag_service_name))
            .await
    }
    async fn downloaded_policies(&self) -> serde_json::Value {
        self.ranger_get(&format!("/plugins/policies/download/{}", self.trino_name))
            .await
    }
    async fn ranger_get(&self, uri: &str) -> serde_json::Value {
        let resp = self
            .ranger
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("authorization", format!("Bearer {RANGER_TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET {uri}");
        serde_json::from_str(&body_string(resp).await).expect("ranger json")
    }

    async fn tag_name(&self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("tag name")
    }
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Does the downloaded tag map attach `tag_name` to the WHOLE table
/// `schema.table` (no column)? This is the plugin's own view of the attachment
/// half — `ServiceTags` keys tags by id and resources by index, so the lookup
/// walks the envelope rather than trusting a flat shape.
fn table_level_tag_in_map(map: &serde_json::Value, schema: &str, table: &str) -> Vec<String> {
    let tags = &map["tags"];
    let mut found = vec![];
    for res in map["serviceResources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let elems = &res["resourceElements"];
        let s = elems["schema"]["values"][0].as_str().unwrap_or_default();
        let t = elems["table"]["values"][0].as_str().unwrap_or_default();
        let has_column = elems.get("column").is_some();
        if s != schema || t != table || has_column {
            continue;
        }
        // `resourceToTagIds` maps resource wire-id → [tag wire-id]; each tag
        // carries its own `type`, which is the tag NAME the engine matches tag
        // policies on (D6).
        let id = res["id"].as_i64().unwrap_or(-1);
        if let Some(tag_ids) = map["resourceToTagIds"][id.to_string()].as_array() {
            for ti in tag_ids {
                let key = ti.as_i64().unwrap_or(-1).to_string();
                if let Some(t) = tags[&key]["type"].as_str() {
                    found.push(t.to_string());
                }
            }
        }
    }
    found
}

/// The tag NAMES that the downloaded `tagPolicies` block masks (`policyType`
/// 1) — the mask half as the plugin receives it.
fn masked_tags_in_policies(policies: &serde_json::Value) -> Vec<String> {
    let mut out = vec![];
    for p in policies["tagPolicies"]["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        if p["policyType"].as_i64() != Some(1) {
            continue;
        }
        for v in p["resources"]["tag"]["values"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            if let Some(s) = v.as_str() {
                out.push(s.to_string());
            }
        }
    }
    out
}

/// **Step 1 of the brief: confirm the hole end to end before fixing it.**
#[tokio::test]
async fn the_pair_the_rename_would_create_is_the_shape_the_engine_downloads() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let suffix = Uuid::new_v4().simple().to_string();
    let old_name = format!("PII-{suffix}");
    let new_name = format!("SENSITIVE-{suffix}");
    let mask_name = format!("t15-mask-{suffix}");

    // 1. The editor mask half, addressing a tag NAME nothing is called yet.
    let (status, body) = f.create_tag_mask_policy(&mask_name, &new_name).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a tag masking policy may name a tag that does not exist — the body carries the NAME as \
         a plain string and nothing resolves it at authoring time; body={body}"
    );

    // 2. The attachment half, for a DIFFERENT tag, attached to the whole table.
    // `validate_key` allows lowercase ascii alphanumerics, ':' and '-' only —
    // no underscore, which is why this does not use `unique`.
    let tag = f
        .tags
        .create(
            &format!("t15-pii-{}", &suffix[..12]),
            &old_name,
            "",
            "",
            None,
        )
        .await
        .expect("create tag");
    let (status, body) = f.put_resource_tag(tag.id, "sales", "orders", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a table-level attachment of a tag no mask names is the ordinary configuration; \
         body={body}"
    );

    // 3. The rename, through the repo method `PATCH /tags/{id}` calls.
    //
    // ⚠ **THIS ASSERTION CHANGED DIRECTION.** It first read
    // `.expect("rename").expect("the tag exists")`, and that is how the hole
    // was confirmed rather than assumed: the rename succeeded and the plugin's
    // own two downloads then carried BOTH halves of the pair under the same
    // name. Verbatim, from that run (`scratchpad/t15_confirm_evidence.txt`,
    // `test result: ok. 1 passed`, suffix elided):
    //
    //   CONFIRM: the rename returned Ok and neither Task 5 guard ran
    //     renamed PII-4a2e… -> SENSITIVE-4a2e…
    //     downloaded tag map, table-level tags on sales.orders: ["SENSITIVE-4a2e…"]
    //     downloaded tagPolicies masks:                         ["SENSITIVE-4a2e…"]
    //     the guard's own verdict on the resulting state:
    //       Some(UnreadableTableMask { mask_origin: Policy, mask: "t15-mask-4a2e…",
    //            tag: "SENSITIVE-4a2e…", schema: "sales", table: "orders" })
    //
    // That last line is the whole finding in one datum: Task 5's own predicate
    // calls the resulting state the refused pair, and it was authored by a write
    // neither of its guards sat on. Keeping the assertion in that direction
    // would have pinned the defect as desired behaviour — a mistake this
    // programme has shipped before — so it is now the refusal, with the
    // measurement kept here as the record.
    let err = f
        .tags
        .update(tag.id, Some(&new_name), None, None)
        .await
        .expect_err("renaming INTO a masked name must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("sales.orders") && msg.contains(&mask_name) && msg.contains(&new_name),
        "the refusal must name the table, the policy to edit and the tag; got {msg}"
    );
    assert!(
        msg.contains("to the columns that need masking instead of to the table")
            && msg.contains("by catalog/schema/table/column path instead of by tag"),
        // Task 16 added a THIRD remedy to the shared sentence; the two asserted
        // here are the ones a RENAME writer can act on.
        "the refusal must carry both tag-side remedies; got {msg}"
    );

    // Nothing was written: the rename is refused on its own transaction before
    // the `UPDATE`, so the tag keeps its name.
    assert_eq!(
        f.tag_name(tag.id).await,
        old_name,
        "a refused rename must not have been committed"
    );

    // And the pair is not in what the engine downloads — the assertion the
    // repo-level one cannot make, because "refused" and "stored but harmless"
    // look identical from the `tags` table alone.
    let map = f.downloaded_tag_map().await;
    let masks = masked_tags_in_policies(&f.downloaded_policies().await);
    let table_tags = table_level_tag_in_map(&map, "sales", "orders");
    assert!(
        table_tags.contains(&old_name),
        "the attachment half is still there under the OLD name (it is legal alone); got \
         {table_tags:?}"
    );
    assert!(
        masks.contains(&new_name),
        "the mask half is still there (it is legal alone); got {masks:?}"
    );
    assert!(
        !table_tags.contains(&new_name),
        "no table-level attachment may carry the masked name — that is the pair the engine \
         type-checks against every column; got {table_tags:?}"
    );
}
