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
    /// The bound engine's name; its coordinator pod polls as
    /// `trino@{engine}-coordinator-<hash>-<suffix>-{service}`.
    engine: String,
}

fn pod_plugin(engine: &str, service: &str) -> String {
    format!("trino@{engine}-coordinator-7d9f8c6b5-x2k4p-{service}")
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
    let engine_name = format!("e-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let engine: Uuid = if origin == "external" {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, origin, \
             external_host, external_port, ranger_plugin_version, authz_service_id) \
             VALUES ($1,'trino','','{}'::jsonb,'running','external','h',8443,'2.8.0',$2) \
             RETURNING id",
        )
        .bind(&engine_name)
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
        .bind(&engine_name)
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
        engine: engine_name,
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
    let plugin = pod_plugin(&f.engine, &f.name);

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
        json!(version(&f.pool, f.id).await),
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

/// Task 12 (live finding I2): egret's client sets the system session
/// property `query_max_run_time` on every statement, so the lineage EXPLAIN
/// needs `alter` on it. The served transform allows it, and only it, without
/// relying on the stored `egret-query-timeout` seed (which this fixture does
/// not write).
#[tokio::test]
async fn the_served_transform_allows_query_max_run_time_and_no_other_system_property() {
    use egret_ranger_eval::eval::{
        evaluate, AccessRequest, Decision, EngineConfig, EngineType, PluginVersion,
    };
    use egret_ranger_eval::model::ServicePolicySet;
    use egret_ranger_eval::resource::RequestResource;
    use egret_ranger_eval::servicedef::ServiceDef;

    let Some(db) = db().await else { return };
    let f = setup(db, "provisioned", "jwt").await;
    let (status, body) = download(
        &f.store,
        &uri(&f.name, Some(&pod_plugin(&f.engine, &f.name))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let set: ServicePolicySet = serde_json::from_value(body.clone()).unwrap();
    let def = ServiceDef::parse(&body["serviceDef"]);
    let ask = |user: &str, property: &str| {
        evaluate(
            &set,
            &AccessRequest {
                user: user.into(),
                groups: Default::default(),
                roles: Default::default(),
                access_type: "alter".into(),
                resource: RequestResource::new().with("systemproperty", property),
                owner: None,
                resolved_tags: None,
            },
            &def,
            &EngineConfig {
                engine: Some(EngineType::Trino),
                plugin_version: Some(PluginVersion::Ranger2_8_0),
                ..EngineConfig::without_super_users()
            },
        )
    };
    let d = ask("egret-lineage", "query_max_run_time");
    assert!(matches!(d, Decision::Allowed { .. }), "{d:?}");
    for other in ["query_max_memory", "join_distribution_type"] {
        let d = ask("egret-lineage", other);
        assert!(!matches!(d, Decision::Allowed { .. }), "{other}: {d:?}");
    }
    // The allow names the principal only: nobody else gains the property.
    let d = ask("alice", "query_max_run_time");
    assert!(!matches!(d, Decision::Allowed { .. }), "{d:?}");
}

#[tokio::test]
async fn every_other_poller_gets_the_stored_policies() {
    let Some(db) = db().await else { return };
    // A pod of an external engine and of an impersonate-mode engine, each bound.
    for (origin, mode) in [("external", ""), ("provisioned", "impersonate")] {
        let f = setup(db.clone(), origin, mode).await;
        let (status, body) = download(
            &f.store,
            &uri(&f.name, Some(&pod_plugin(&f.engine, &f.name))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!names_lineage(&body), "{origin}/{mode}: {body}");
        // A provisioned engine whose spec names no delegation mode at all is
        // ineligible too, not an error.
        if origin == "provisioned" {
            sqlx::query("UPDATE engine_mgmt.engines SET spec = '{}'::jsonb WHERE name = $1")
                .bind(&f.engine)
                .execute(&f.pool)
                .await
                .unwrap();
            let (status, body) = download(
                &f.store,
                &uri(&f.name, Some(&pod_plugin(&f.engine, &f.name))),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(!names_lineage(&body), "no mode: {body}");
        }
    }
    // On an eligible profile (fix round 2, N1): no pluginId, one naming another
    // service, a well-formed id with an unknown host (the shape Task 5 measured
    // from a container), and a pod of a provisioned JWT engine NOT bound here.
    let f = setup(db.clone(), "provisioned", "jwt").await;
    let other = setup(db, "provisioned", "jwt").await;
    for plugin in [
        None,
        Some(pod_plugin(&f.engine, "another-service")),
        Some(format!("trino@e00d4f5be6a4-{}", f.name)),
        Some(pod_plugin(&other.engine, &f.name)),
    ] {
        let (_, body) = download(&f.store, &uri(&f.name, plugin.as_deref())).await;
        assert!(!names_lineage(&body), "{plugin:?}: {body}");
    }
    // The control: the bound engine's own worker pod is served the transform.
    let worker = format!("trino@{}-worker-5c4b8d9f7-ab12c-{}", f.engine, f.name);
    let (_, body) = download(&f.store, &uri(&f.name, Some(&worker))).await;
    assert!(names_lineage(&body));
}

async fn version(pool: &sqlx::PgPool, id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Fix round 2, N1(ii): a bind, an unbind and a delegation-mode change of a
/// bound engine each bump the profile, so no cached transform outlives a
/// change of eligibility. An unrelated engine edit bumps nothing.
#[tokio::test]
async fn binding_changes_bump_the_profiles_version() {
    let Some(db) = db().await else { return };
    let f = setup(db.clone(), "provisioned", "jwt").await;
    let other = setup(db, "provisioned", "jwt").await;
    let id = |name: &str| {
        let pool = f.pool.clone();
        let name = name.to_string();
        async move {
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM engine_mgmt.engines WHERE name = $1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let e = id(&f.engine).await;

    let v0 = version(&f.pool, f.id).await;
    let w0 = version(&f.pool, other.id).await;
    // Unbind.
    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = NULL WHERE id = $1")
        .bind(e)
        .execute(&f.pool)
        .await
        .unwrap();
    let v1 = version(&f.pool, f.id).await;
    assert!(v1 > v0, "unbind: {v0} -> {v1}");
    // Bind to another profile: that profile moves, this one does not.
    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = $2 WHERE id = $1")
        .bind(e)
        .bind(other.id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(version(&f.pool, other.id).await > w0, "bind");
    assert_eq!(version(&f.pool, f.id).await, v1);
    // Delegation mode of a bound engine.
    let w1 = version(&f.pool, other.id).await;
    sqlx::query(
        "UPDATE engine_mgmt.engines SET spec = jsonb_set(spec, '{trinoConfig,delegationMode}', '\"impersonate\"') \
          WHERE id = $1",
    )
    .bind(e)
    .execute(&f.pool)
    .await
    .unwrap();
    let w2 = version(&f.pool, other.id).await;
    assert!(w2 > w1, "mode change: {w1} -> {w2}");
    // An unrelated edit (status) bumps nothing.
    sqlx::query("UPDATE engine_mgmt.engines SET status = 'failed' WHERE id = $1")
        .bind(e)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(version(&f.pool, other.id).await, w2);
}
