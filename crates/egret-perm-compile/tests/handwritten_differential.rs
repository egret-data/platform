//! DG #4b Task 4: route A's reach agrees with `POST /perm/explain`.
//!
//! A Trino profile (with a tag service) and a StarRocks profile hold ONLY
//! hand-written policies: an allow on a wildcard schema, a deny on one column,
//! an allow on every table but one (exclude), a mask on one column, and (Trino)
//! a tag access deny via a table tag. For every (object, user) of a grid:
//!
//! (a) the hand-written policy explain names as deciding the access (or the
//!     mask), when there is one, is in route A's `applies` set for that object
//!     and profile;
//! (b) every enabled `applies` access policy with an allow item naming the
//!     user directly is explain's decider, or explain names another decider
//!     (a deny, or a policy evaluated first) — never "no policy matched".
//!
//! Task 1 M5 (`docs/en/plans/2026-10-19-dg-classification-live.md`) measured
//! explain naming a hand-written decider by name and id on both engines.
//! Fixtures carry no unmodelled construct (conditions, validity schedules,
//! zones), so no explain answer here is Undecidable for that reason; an
//! Undecidable answer (a tag resolution explain declines) is skipped and
//! counted, never read as agreement.
//!
//! DB-gated; FAILS without a database.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::handwritten::{handwritten_for_object_via, MatchWord, ObjectTarget};
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

fn lv(values: &[&str]) -> Value {
    json!({ "values": values })
}

async fn insert(pool: &sqlx::PgPool, service: i64, body: Value) {
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, $2, $3, 'operator')",
    )
    .bind(service)
    .bind(body["name"].as_str().unwrap())
    .bind(&body)
    .execute(pool)
    .await
    .unwrap();
}

fn access(name: &str, svc: &str, res: Value, allow: &[&str], deny: &[&str]) -> Value {
    let items = |users: &[&str]| -> Value {
        if users.is_empty() {
            json!([])
        } else {
            json!([{ "users": users, "accesses": [{ "type": "select", "isAllowed": true }] }])
        }
    };
    json!({ "name": name, "service": svc, "policyType": 0, "isEnabled": true, "resources": res,
            "policyItems": items(allow), "denyPolicyItems": items(deny) })
}

#[tokio::test]
async fn route_a_reach_agrees_with_explain() {
    let dsn = egret_test_db::url()
        .await
        .expect("a database is required (DATABASE_URL)");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.unwrap();
    egret_policy::seed_starrocks(&policy).await.unwrap();
    egret_policy::seed_tag(&policy).await.unwrap();

    // Catalog with a warehouse.
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$1) RETURNING id")
            .bind(unique("hwd-ws-"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("hwd-sc-"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("hwd_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let cat_name = unique("hwd_");
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&cat_name)
    .bind(wh)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Profiles and engines at egret's default images (a known plugin version).
    let tag_svc = unique("hwd-tag-");
    let tag_id = policy
        .create_service(&tag_svc, "tag", &json!({}))
        .await
        .unwrap()
        .id;
    let tr = unique("hwd-tr-");
    let tr_id = policy
        .create_service(&tr, "trino", &json!({ "tagService": tag_svc }))
        .await
        .unwrap()
        .id;
    let sr = unique("hwd-sr-");
    let sr_id = policy
        .create_service(&sr, "starrocks", &json!({}))
        .await
        .unwrap()
        .id;
    let d = egret_perm::DEFAULT_ENGINE_IMAGES;
    for (ty, sid, spec) in [
        (
            "trino",
            tr_id,
            json!({ "imageVersion": d.trino_image_version }),
        ),
        (
            "starrocks",
            sr_id,
            json!({ "starrocksConfig": { "feImage": d.starrocks_fe_image } }),
        ),
    ] {
        let eng: Uuid = sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
             VALUES ($1, $2, 'egret', $3, $4) RETURNING id",
        )
        .bind(unique("hwd-eng-"))
        .bind(ty)
        .bind(spec)
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
        )
        .bind(eng)
        .bind(cat)
        .execute(&pool)
        .await
        .unwrap();
    }

    // The hand-written policies.
    let c = cat_name.as_str();
    for (svc, sid, mid) in [(&tr, tr_id, "schema"), (&sr, sr_id, "database")] {
        let res = |s: &str, t: Value, col: &str| json!({ "catalog": lv(&[c]), mid: lv(&[s]), "table": t, "column": lv(&[col]) });
        insert(
            &pool,
            sid,
            access(
                &format!("{svc}-wild"),
                svc,
                res("*", lv(&["*"]), "*"),
                &["ua"],
                &[],
            ),
        )
        .await;
        insert(
            &pool,
            sid,
            access(
                &format!("{svc}-deny-email"),
                svc,
                res("s", lv(&["t"]), "email"),
                &[],
                &["ua", "ub"],
            ),
        )
        .await;
        insert(
            &pool,
            sid,
            access(
                &format!("{svc}-not-u"),
                svc,
                res("s", json!({ "values": ["u"], "isExcludes": true }), "*"),
                &["ub"],
                &[],
            ),
        )
        .await;
        insert(&pool, sid, json!({
            "name": format!("{svc}-mask-country"), "service": svc, "policyType": 1, "isEnabled": true,
            "resources": { "catalog": lv(&[c]), mid: lv(&["s"]), "table": lv(&["t"]), "column": lv(&["country"]) },
            "dataMaskPolicyItems": [{ "users": ["ua"], "accesses": [{ "type": "select", "isAllowed": true }],
                                      "dataMaskInfo": { "dataMaskType": "MASK_NULL" } }]
        })).await;
    }
    let tagname = unique("HWDT");
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("hwd-k-"))
        .bind(&tagname)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', 'u', '')",
    )
    .bind(tag)
    .bind(wh)
    .execute(&pool)
    .await
    .unwrap();
    insert(&pool, tag_id, json!({
        "name": format!("{tag_svc}-deny-ub"), "service": tag_svc, "serviceType": "tag", "policyType": 0,
        "isEnabled": true, "resources": { "tag": lv(&[&tagname]) },
        "denyPolicyItems": [{ "users": ["ub"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    })).await;

    // The explain route, as a platform admin.
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'hwd') RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("hwd")))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    let token = sessions
        .create(egret_core::UserId(admin), "test", "127.0.0.1")
        .await
        .unwrap()
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .unwrap(),
    );
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(pool.clone()),
        policy: policy.clone(),
        sessions,
        pdp,
    });

    let objects: Vec<(Option<&str>, Option<&str>, Option<&str>)> = vec![
        (None, None, None),
        (Some("s"), None, None),
        (Some("s"), Some("t"), None),
        (Some("s"), Some("u"), None),
        (Some("s"), Some("t"), Some("id")),
        (Some("s"), Some("t"), Some("email")),
        (Some("s"), Some("t"), Some("country")),
        (Some("s"), Some("u"), Some("id")),
    ];
    let mut checked_a = 0;
    let mut checked_b = 0;
    let mut undecidable = 0;
    let mut skipped_mask = 0;
    for (s, t, col) in &objects {
        let target = ObjectTarget {
            catalog_id: cat,
            schema: s.map(Into::into),
            table: t.map(Into::into),
            column: col.map(Into::into),
        };
        let mut conn = pool.acquire().await.unwrap();
        let reach = handwritten_for_object_via(&mut conn, &target)
            .await
            .unwrap();
        drop(conn);
        for user in ["ua", "ub"] {
            let req = HttpRequest::builder()
                .method("POST")
                .uri("/perm/explain")
                .header("Cookie", format!("egret_session={token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({ "subject": { "user": user }, "action": "read",
                            "target": { "catalog_id": cat, "schema": s, "table": t, "column": col } })
                    .to_string(),
                ))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let ex: Value =
                serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            for prof in ex["profiles"].as_array().unwrap() {
                let svc = prof["service_name"].as_str().unwrap();
                let services: BTreeSet<&str> = if svc == tr {
                    [tr.as_str(), tag_svc.as_str()].into()
                } else {
                    [svc].into()
                };
                let applies: BTreeSet<&str> = reach
                    .policies
                    .iter()
                    .filter(|p| {
                        p.r#match == MatchWord::Applies && services.contains(p.service.as_str())
                    })
                    .map(|p| p.name.as_str())
                    .collect();
                if let Some(m) = prof["mask"]["decision"]["result"]["policy_name"].as_str() {
                    assert!(
                        applies.contains(m),
                        "(a) mask decider {m} not in applies for {user} in {svc}"
                    );
                    checked_a += 1;
                }
                // StarRocks reads a MASK_NULL column without checking `select`
                // (explain's `masked_read_skips_access`): no access verdict.
                if prof["masked_read_skips_access"] == json!(true) {
                    skipped_mask += 1;
                    continue;
                }
                let decision = &prof["accesses"][0]["decision"];
                let where_ = format!("{user} on {s:?}.{t:?}.{col:?} in {svc}: {decision}");
                if decision["kind"] == "undecidable" {
                    undecidable += 1;
                    continue;
                }
                let decider = decision["policy_name"].as_str().unwrap_or("");
                // (a)
                if !decider.is_empty() {
                    assert!(
                        applies.contains(decider),
                        "(a) decider not in applies: {where_}; applies {applies:?}"
                    );
                    checked_a += 1;
                }
                // (b)
                for p in reach.policies.iter().filter(|p| {
                    p.r#match == MatchWord::Applies
                        && services.contains(p.service.as_str())
                        && p.enabled
                        && p.policy_type == "access"
                        && p.items.iter().any(|i| {
                            i.bucket == egret_perm_compile::handwritten::Bucket::Allow
                                && i.users.iter().any(|u| u == user)
                        })
                }) {
                    assert!(
                        !decider.is_empty(),
                        "(b) {} applies with an allow for {user} but explain names no decider: {where_}",
                        p.name
                    );
                    checked_b += 1;
                }
            }
        }
    }
    eprintln!(
        "differential: (a) {checked_a} deciders, (b) {checked_b} allows, {undecidable} \
         undecidable and {skipped_mask} masked-read verdicts skipped"
    );
    assert!(checked_a >= 10, "too few deciders compared: {checked_a}");
    assert!(checked_b >= 10, "too few allows compared: {checked_b}");
}
