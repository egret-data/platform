//! View governance R5 (Task 7 fix round 1): the `egret-lineage` material is
//! added at download time, only for a plugin of a Trino engine egret
//! provisions in JWT mode, and never stored.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_policy::{ranger_router, seed_tag, seed_trino, PolicyStore, RangerState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "devtoken";

async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn download(store: &PolicyStore, uri: &str) -> (StatusCode, Value) {
    let resp = ranger_router(RangerState {
        store: store.clone(),
        auth_token: Arc::from(TOKEN),
    })
    .oneshot(
        Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

struct Fx {
    store: PolicyStore,
    pool: sqlx::PgPool,
    name: String,
    id: i64,
    catalog: String,
}

/// A trino profile bound to one engine of `origin`/`mode`, with one attached
/// catalog and one stored hand-written mask (`managed_by` operator).
async fn setup(db: Db, origin: &str, mode: &str) -> Fx {
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.unwrap();
    seed_tag(&store).await.unwrap();
    let name = format!("trino_{}", Uuid::new_v4().simple());
    let row = store
        .create_service(&name, "trino", &json!({}))
        .await
        .unwrap();
    let engine: Uuid = if origin == "external" {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, origin, \
             external_host, external_port, ranger_plugin_version, authz_service_id) \
             VALUES ($1,'trino','','{}'::jsonb,'running','external','h',8443,'2.5.0',$2) \
             RETURNING id",
        )
        .bind(format!("e-{}", Uuid::new_v4().simple()))
        .bind(row.id)
        .fetch_one(&pool)
        .await
        .unwrap()
    } else {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, authz_service_id) \
             VALUES ($1,'trino','egret',jsonb_build_object('trinoConfig', \
                     jsonb_build_object('delegationMode', $2::text)),'running',$3) RETURNING id",
        )
        .bind(format!("e-{}", Uuid::new_v4().simple()))
        .bind(mode)
        .bind(row.id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let catalog = format!("c_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1,'tpch','{}'::jsonb) RETURNING id",
    )
    .bind(&catalog)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, apply_status) \
         VALUES ($1,$2,'applied')",
    )
    .bind(engine)
    .bind(cat)
    .execute(&pool)
    .await
    .unwrap();
    let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, 'hand mask', $2, true, 'operator')",
    )
    .bind(row.id)
    .bind(json!({
        "name": "hand mask", "policyType": 1, "isEnabled": true,
        "resources": {"catalog": lvl(&catalog), "schema": lvl("s"), "table": lvl("t"), "column": lvl("c")},
        "dataMaskPolicyItems": [{"users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NULL"}}],
        "policyItems": [], "denyPolicyItems": []
    }))
    .execute(&pool)
    .await
    .unwrap();
    Fx {
        store,
        pool,
        name,
        id: row.id,
        catalog,
    }
}

fn names_lineage(v: &Value) -> bool {
    v.to_string().contains("egret-lineage")
}

fn uri(svc: &str, plugin: Option<&str>) -> String {
    match plugin {
        Some(p) => format!("/plugins/policies/download/{svc}?pluginId={p}&lastKnownVersion=-1"),
        None => format!("/plugins/policies/download/{svc}?lastKnownVersion=-1"),
    }
}

#[tokio::test]
async fn a_provisioned_jwt_trino_plugin_is_served_the_transform_and_nothing_is_stored() {
    let Some(db) = db().await else { return };
    let f = setup(db, "provisioned", "jwt").await;
    let plugin = format!("trino@pod-0-{}", f.name);

    let (status, body) = download(&f.store, &uri(&f.name, Some(&plugin))).await;
    assert_eq!(status, StatusCode::OK);
    let policies = body["policies"].as_array().unwrap();
    let hand = policies.iter().find(|p| p["name"] == "hand mask").unwrap();
    assert_eq!(
        hand["dataMaskPolicyItems"][0]["users"],
        json!(["egret-lineage"]),
        "the hand-written mask is served with the lineage item first"
    );
    assert_eq!(
        hand["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"],
        "MASK_NONE"
    );
    let read = policies
        .iter()
        .find(|p| p["name"] == format!("egret-lineage-read-{}", f.catalog))
        .expect("the per-catalog read policy");
    assert_eq!(read["resources"]["catalog"]["values"], json!([f.catalog]));
    assert!(policies
        .iter()
        .any(|p| p["name"] == "egret-lineage-deny-catalog-schema-table-column"));
    assert_eq!(
        body["policyVersion"],
        json!(1),
        "the served version is the stored one"
    );

    // Deterministic: the same version serves the same bytes.
    let (_, again) = download(&f.store, &uri(&f.name, Some(&plugin))).await;
    assert_eq!(body.to_string(), again.to_string());

    // Nothing about the principal is stored.
    let stored: Vec<Value> =
        sqlx::query_scalar("SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1")
            .bind(f.id)
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert!(!stored.iter().any(names_lineage), "{stored:?}");
}

#[tokio::test]
async fn every_other_plugin_gets_the_stored_policies() {
    let Some(db) = db().await else { return };
    // An external engine, an impersonate-mode engine, and on the eligible
    // engine a poll with no pluginId or one naming another service.
    for (origin, mode) in [("external", ""), ("provisioned", "impersonate")] {
        let f = setup(db.clone(), origin, mode).await;
        let plugin = format!("trino@pod-0-{}", f.name);
        let (status, body) = download(&f.store, &uri(&f.name, Some(&plugin))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!names_lineage(&body), "{origin}/{mode}: {body}");
    }
    let f = setup(db, "provisioned", "jwt").await;
    for plugin in [None, Some("trino@pod-0-another-service".to_string())] {
        let (_, body) = download(&f.store, &uri(&f.name, plugin.as_deref())).await;
        assert!(!names_lineage(&body), "{plugin:?}: {body}");
    }
}
