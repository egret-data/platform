//! DB-gated Ranger `download_tags` tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
//!
//! Exercises the real tag-download path: 404 unknown service, 304 on matching
//! `tagVersion`, and 200 serving a seeded `catalog_tag_assignments` row for the
//! service's warehouse (resolved via the engine binding — see
//! `PolicyStore::tag_assignments_for_service`).
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

fn app(store: PolicyStore) -> axum::Router {
    ranger_router(RangerState {
        store,
        auth_token: Arc::from(TOKEN),
        mode: egret_policy::ranger::AuthMode::Required,
        verifier: None,
    })
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

/// A bare workspace + storage connection + warehouse, no engine attached.
/// Returns the warehouse id.
async fn seed_warehouse(pool: &sqlx::PgPool) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("tagws-{}", Uuid::new_v4().simple()))
            .bind("Tag Test WS")
            .fetch_one(pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("tagsc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-tag-test")
    .bind("AKIATEST")
    .fetch_one(pool)
    .await
    .expect("insert storage connection");
    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("tagwh_{}", Uuid::new_v4().simple()))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    wh_id
}

/// A Trino engine bound to `service_id`. Its warehouse is not a column any
/// more (SP3c-2 Task 6) — a caller that wants this engine tied to a warehouse
/// must attach that warehouse's registry catalog itself, via `attach_catalog`.
async fn seed_engine(pool: &sqlx::PgPool, service_id: i64) -> Uuid {
    let (eng_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(format!("tageng-{}", Uuid::new_v4().simple()))
    .bind(service_id)
    .fetch_one(pool)
    .await
    .expect("insert engine");
    eng_id
}

/// A registry catalog for `warehouse_id`, named `{prefix}_<uuid>`. Returns
/// `(catalog_id, catalog_name)`.
async fn seed_registry_catalog(
    pool: &sqlx::PgPool,
    warehouse_id: Uuid,
    prefix: &str,
) -> (Uuid, String) {
    let catalog_name = format!("{prefix}_{}", Uuid::new_v4().simple());
    let (catalog_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) RETURNING id",
    )
    .bind(&catalog_name)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("insert registry catalog");
    (catalog_id, catalog_name)
}

/// Attach `catalog_id` to `engine_id` (`registry.engine_catalog_attachments`
/// — the registry branch's key).
async fn attach_catalog(pool: &sqlx::PgPool, engine_id: Uuid, catalog_id: Uuid) {
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
}

/// One PII tag, assigned to `sales.orders.email` on `warehouse_id`.
async fn seed_tag_assignment(pool: &sqlx::PgPool, warehouse_id: Uuid) {
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("pii-{}", Uuid::new_v4().simple()))
            .bind("PII")
            .fetch_one(pool)
            .await
            .expect("insert tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .execute(pool)
    .await
    .expect("insert assignment");
}

/// Seed a workspace + storage connection + warehouse + a Trino engine bound to
/// `service_id`, with a registry catalog attached (the modern, fully-migrated
/// shape). Returns (warehouse_id, the registry catalog name the engine
/// reaches it by).
async fn seed_bound_warehouse(pool: &sqlx::PgPool, service_id: i64) -> (Uuid, String) {
    let wh_id = seed_warehouse(pool).await;
    let eng_id = seed_engine(pool, service_id).await;
    // The engine reaches the warehouse through its registry catalog; the tag
    // export must emit THAT catalog's name (the SQL name Trino sees).
    let (catalog_id, catalog_name) = seed_registry_catalog(pool, wh_id, "tagcat").await;
    attach_catalog(pool, eng_id, catalog_id).await;
    (wh_id, catalog_name)
}

#[tokio::test]
async fn download_tags_unknown_service_404() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let r = app(store)
        .oneshot(get("/tags/download/nope_xyz_unknown"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_tags_matching_tag_version_304() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", Uuid::new_v4().simple());
    let row = store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");
    // Freshly-created services default to tag_version = 1.
    assert_eq!(row.tag_version, 1);
    let r = app(store)
        .oneshot(get(&format!(
            "/tags/download/{svc}?lastKnownVersion={}",
            row.tag_version
        )))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn download_tags_serves_seeded_assignment() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", Uuid::new_v4().simple());
    let svc_row = store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");

    let (wh_id, catalog_name) = seed_bound_warehouse(&pool, svc_row.id).await;

    // One classification tag, assigned to a column of a table in this warehouse.
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("pii-{}", Uuid::new_v4().simple()))
            .bind("PII")
            .fetch_one(&pool)
            .await
            .expect("insert tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(wh_id)
    .execute(&pool)
    .await
    .expect("insert assignment");

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(st["op"], "add");
    assert_eq!(st["serviceName"], svc.as_str());
    assert_eq!(st["tagVersion"], 1);
    // One tag def named PII.
    let tags = st["tags"].as_object().unwrap();
    assert_eq!(tags.len(), 1, "one tag, got {tags:?}");
    assert_eq!(st["tags"]["1"]["type"], "PII");
    // One resource. `resourceElements` sits directly on the entry (Ranger
    // `RangerServiceResource` shape), and `catalog` resolves to the engine's
    // registry catalog name, NOT the warehouse name.
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(srs.len(), 1, "one resource, got {srs:?}");
    assert_eq!(srs[0]["isEnabled"], true);
    let els = &srs[0]["resourceElements"];
    assert_eq!(els["catalog"]["values"][0], json!(catalog_name));
    assert_eq!(els["schema"]["values"][0], "sales");
    assert_eq!(els["table"]["values"][0], "orders");
    assert_eq!(els["column"]["values"][0], "email");
    // Resource → tag linkage.
    assert_eq!(st["resourceToTagIds"]["1"], json!([1]));
}

/// The Trino tag plugin polls the TAG service (`<profile>_tag`), but engines bind
/// to the TRINO profile. `download_tags` must resolve the owning trino profile
/// (via its `tagService` config) and serve ITS engine's assignments — otherwise
/// tag masking silently sees zero resources.
#[tokio::test]
async fn download_tags_via_tag_service_resolves_owning_trino_profile() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino def");
    seed_tag(&store).await.expect("seed tag def");
    let base = Uuid::new_v4().simple().to_string();
    let trino = format!("trino_{base}");
    let tag_svc = format!("{trino}_tag");
    // Trino profile carries `tagService` pointing at its tag service; engine binds
    // to the TRINO profile (not the tag service).
    store
        .create_service(&tag_svc, "tag", &json!({}))
        .await
        .expect("create tag service");
    let trino_row = store
        .create_service(&trino, "trino", &json!({ "tagService": tag_svc }))
        .await
        .expect("create trino service");
    let (wh_id, catalog_name) = seed_bound_warehouse(&pool, trino_row.id).await;
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("pii-{base}"))
            .bind("PII")
            .fetch_one(&pool)
            .await
            .expect("insert tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag_id)
    .bind(wh_id)
    .execute(&pool)
    .await
    .expect("insert assignment");

    // Poll the TAG service, as the Trino plugin does.
    let r = app(store)
        .oneshot(get(&format!("/tags/download/{tag_svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(
        srs.len(),
        1,
        "polling the tag service must resolve the owning trino profile's assignments, got {srs:?}"
    );
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(catalog_name)
    );
    assert_eq!(srs[0]["resourceElements"]["column"]["values"][0], "email");
}

/// I3: a SECONDARY attachment — a warehouse that is NOT the attaching engine's
/// own primary attachment — is exported under its registry
/// catalog name; an engine bound to the SAME service that does NOT attach
/// that warehouse's catalog sees nothing for it, even though another engine
/// (bound to the same service) does. This pins the attachment join: a mutant
/// that drops it and falls back to `e.warehouse_id = a.warehouse_id` would
/// pass neither half of this test (it would show W's tags to the
/// non-attaching engine's service via a `warehouse_id` match it doesn't have,
/// or fail to show them to the attaching one).
#[tokio::test]
async fn download_tags_secondary_attachment_scoped_to_attaching_engine() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", Uuid::new_v4().simple());
    let svc_row = store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");

    // Two engines bound to the SAME service, neither attached to W yet.
    let eng_a = seed_engine(&pool, svc_row.id).await;
    let eng_b = seed_engine(&pool, svc_row.id).await;

    // A THIRD warehouse W, tagged, whose catalog only engine B attaches — a
    // secondary attachment (W is not B's primary warehouse either).
    let wh_w = seed_warehouse(&pool).await;
    let (catalog_w, catalog_name_w) = seed_registry_catalog(&pool, wh_w, "secondary").await;
    attach_catalog(&pool, eng_b, catalog_w).await;
    seed_tag_assignment(&pool, wh_w).await;

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(
        srs.len(),
        1,
        "only the secondary attachment's tag must be exported, got {srs:?}"
    );
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(catalog_name_w)
    );
    let _ = eng_a;
}

/// I3: an engine bound to a DIFFERENT service exports nothing for a warehouse
/// it attaches — confirming the service scope (`e.authz_service_id =
/// COALESCE(tp.id, s.id)`) still holds on both branches of the union.
#[tokio::test]
async fn download_tags_unrelated_service_exports_nothing() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");

    // The service under test binds NO engine at all.
    let empty_svc = format!("trino_{}", Uuid::new_v4().simple());
    store
        .create_service(&empty_svc, "trino", &json!({}))
        .await
        .expect("create empty service");

    // A DIFFERENT service, with a fully-migrated engine + tagged warehouse —
    // proves the fixture actually has tags to (not) leak.
    let other_svc = format!("trino_{}", Uuid::new_v4().simple());
    let other_row = store
        .create_service(&other_svc, "trino", &json!({}))
        .await
        .expect("create other service");
    let (wh_id, _catalog_name) = seed_bound_warehouse(&pool, other_row.id).await;
    seed_tag_assignment(&pool, wh_id).await;

    let r = app(store)
        .oneshot(get(&format!("/tags/download/{empty_svc}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert!(
        srs.is_empty(),
        "a service with no bound engine must export nothing, got {srs:?}"
    );
}

/// I3 leftover (SP3 catalog-registry review re-review). The fail-closed
/// direction: an engine with NO catalog attachment — e.g. right after a
/// detach with nothing re-attached — exports nothing for W.
///
/// Renamed in SP3c-2 Task 6 review: it used to be `…owning_engine…`, because
/// engine C owned W through `engines.warehouse_id` and the point was that
/// ownership alone did not export. That column is gone, so C owns nothing and
/// the old name claimed a setup the fixture no longer builds.
///
/// What still earns the test its place is the POSITIVE CONTROL, not the
/// absence: S1 exports nothing **while S2, at the same moment, exports W**.
/// Without that pairing this would be indistinguishable from an export that
/// is broken for everyone — which is how a tag export fails open unnoticed.
/// The two engines must sit on TWO DIFFERENT services: sharing one service
/// cannot isolate "this engine's own export" from "the service's export via
/// some other bound engine".
#[tokio::test]
async fn download_tags_unattached_engine_exports_nothing_while_the_attaching_service_does() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");

    // Engine C: bound to service S1, no catalog attachment at all — the state
    // right after a detach with nothing re-attached.
    let svc_c = format!("trino_{}", Uuid::new_v4().simple());
    let svc_c_row = store
        .create_service(&svc_c, "trino", &json!({}))
        .await
        .expect("create service C");
    let wh_w = seed_warehouse(&pool).await;
    let _eng_c = seed_engine(&pool, svc_c_row.id).await;
    seed_tag_assignment(&pool, wh_w).await;

    // Engine D: a DIFFERENT service S2, attaches W's registry catalog as its
    // only attachment.
    let svc_d = format!("trino_{}", Uuid::new_v4().simple());
    let svc_d_row = store
        .create_service(&svc_d, "trino", &json!({}))
        .await
        .expect("create service D");
    let eng_d = seed_engine(&pool, svc_d_row.id).await;
    let (catalog_w, catalog_name_w) = seed_registry_catalog(&pool, wh_w, "detached_owner").await;
    attach_catalog(&pool, eng_d, catalog_w).await;

    // S1 (engine C, the owning-but-detached engine) exports NOTHING for W.
    let r = app(store.clone())
        .oneshot(get(&format!("/tags/download/{svc_c}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert!(
        srs.is_empty(),
        "an engine whose primary IS the tagged warehouse, with no attachment, \
         must export nothing for it, got {srs:?}"
    );

    // S2 (engine D, the actual attaching engine) exports it correctly.
    let r = app(store)
        .oneshot(get(&format!("/tags/download/{svc_d}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let st: Value = serde_json::from_slice(&bytes).unwrap();
    let srs = st["serviceResources"].as_array().unwrap();
    assert_eq!(srs.len(), 1, "one resource, got {srs:?}");
    assert_eq!(
        srs[0]["resourceElements"]["catalog"]["values"][0],
        json!(catalog_name_w)
    );
}

// ---------------------------------------------------------------------------
// SP4d debt Task 6, item 2 — the resolver and the download writer must cover
// the same set, and now share the one spelling of the reachability leg.
// ---------------------------------------------------------------------------

/// **The set-equality measurement.** `explain` evaluates the artifact the
/// engine downloads, and the TAG set it hands the evaluator comes from
/// `resolved_tags_for_resource_via`, which scopes on `a.warehouse_id =
/// c.warehouse_id` and applies no engine predicate at all. The engine's own tag
/// map comes from `PolicyStore::tag_assignments_for_service`, which does. They
/// agreed only because `explain` narrows the catalog through
/// `catalogs_for_service_via` first, and nothing compared them — so a change to
/// either join made `explain` resolve tags the engine never received, i.e. a
/// confident over-deny.
///
/// Since Task 6 the reachability leg is spelled ONCE
/// (`catalogs_for_service_via`) and the writer calls it, so the two cannot
/// drift. This asserts the property that factoring buys, over a fixture built
/// so that it is **not** vacuous:
///
/// * catalog A — attached to an engine bound to THIS profile: served, and
///   resolvable.
/// * catalog B — its own warehouse, its own assignments, attached to **no
///   engine at all**: must appear on NEITHER side. This is the
///   `engine_catalog_attachments` leg.
/// * catalog C — its own warehouse, its own assignments, attached to an engine
///   bound to a **DIFFERENT profile**: must appear on neither side either. This
///   is the `authz_service_id` leg.
///
/// (B cannot be a second catalog NAME on A's warehouse: `registry.catalogs`
/// carries `catalogs_warehouse_id_key`, a UNIQUE on `warehouse_id`, so a
/// warehouse has at most one catalog. Measured, not assumed — the first version
/// of this fixture tried it and got `23505`.)
///
/// Compared as SETS of `(catalog, schema, table, column, tag)` rather than as
/// counts, and the set is asserted non-empty first — `assert_eq!(a, b)` over two
/// empty sets is the assertion that cannot fail.
///
/// The resolver side is filtered to `MatchType::Self_`. That is not a fudge: the
/// writer exports STORED coordinates verbatim, while the resolver also rolls a
/// table-level attachment up as an `Ancestor` of every column asked about, so an
/// `Ancestor` entry is the resolver's view of a row the writer already exported
/// at its own coordinate. Counting it would double-count one row, not reveal a
/// disagreement.
#[tokio::test]
async fn the_tag_download_and_the_explain_resolver_cover_the_same_set() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let store = PolicyStore::new(db.clone());
    seed_trino(&store).await.expect("seed");

    let profile = store
        .create_service(
            &format!("trino_{}", Uuid::new_v4().simple()),
            "trino",
            &json!({}),
        )
        .await
        .expect("create profile");
    let other_profile = store
        .create_service(
            &format!("trino_{}", Uuid::new_v4().simple()),
            "trino",
            &json!({}),
        )
        .await
        .expect("create second profile");

    // ── catalog A: reachable by `profile`.
    let wh = seed_warehouse(&pool).await;
    let eng = seed_engine(&pool, profile.id).await;
    let (cat_a, name_a) = seed_registry_catalog(&pool, wh, "tagcat_a").await;
    attach_catalog(&pool, eng, cat_a).await;
    seed_tag_assignment(&pool, wh).await;

    // ── catalog B: its own warehouse and assignments, attached to NO engine.
    let wh_b = seed_warehouse(&pool).await;
    let (_cat_b, name_b) = seed_registry_catalog(&pool, wh_b, "tagcat_b").await;
    seed_tag_assignment(&pool, wh_b).await;

    // ── catalog C: another warehouse, bound to the OTHER profile.
    let wh_c = seed_warehouse(&pool).await;
    let eng_c = seed_engine(&pool, other_profile.id).await;
    let (cat_c, name_c) = seed_registry_catalog(&pool, wh_c, "tagcat_c").await;
    attach_catalog(&pool, eng_c, cat_c).await;
    seed_tag_assignment(&pool, wh_c).await;

    type Row = (String, String, String, String, String);

    // ── what the engine downloads.
    let served: std::collections::BTreeSet<Row> = store
        .tag_assignments_for_service(profile.id)
        .await
        .expect("tag assignments")
        .into_iter()
        .map(|r| {
            (
                r.catalog,
                r.schema_name,
                r.table_name,
                r.column_name,
                r.tag_name,
            )
        })
        .collect();

    // ── what `explain` would resolve, reached exactly the way `explain`
    //    reaches it: narrow the catalogs through `catalogs_for_service_via`,
    //    then resolve each stored coordinate.
    let mut conn = pool.acquire().await.expect("conn");
    let mut resolved: std::collections::BTreeSet<Row> = std::collections::BTreeSet::new();
    for c in egret_policy::catalogs_for_service_via(&mut conn, profile.id)
        .await
        .expect("catalogs for service")
    {
        for (schema, table, column) in egret_policy::tag_assignment_coordinates_via(&mut conn, c.id)
            .await
            .expect("coordinates")
        {
            for t in egret_policy::resolved_tags_for_resource_via(
                &mut conn, c.id, &schema, &table, &column,
            )
            .await
            .expect("resolve")
            .into_iter()
            .filter(|t| t.match_type == egret_ranger_eval::resource::MatchType::Self_)
            {
                resolved.insert((
                    c.name.clone(),
                    schema.clone(),
                    table.clone(),
                    column.clone(),
                    t.name,
                ));
            }
        }
    }

    assert!(
        !served.is_empty(),
        "the fixture must actually serve something, or this comparison is vacuous"
    );
    assert_eq!(
        served, resolved,
        "the tag download and the explain resolver must cover the same set"
    );
    // And the fixture really did hold out the two unreachable catalogs, so the
    // equality above is not the equality of two over-broad sets.
    let names: Vec<&str> = served.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        names,
        vec![name_a.as_str()],
        "only the attached catalog's name may be served"
    );
    for held_out in [&name_b, &name_c] {
        assert!(
            !served.iter().any(|r| &r.0 == held_out) && !resolved.iter().any(|r| &r.0 == held_out),
            "{held_out} is not reachable by this profile and must appear on neither side"
        );
    }
}
