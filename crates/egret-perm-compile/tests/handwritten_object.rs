//! DG #4b route A (spec §4.2, Q2): which hand-written Ranger policies reach a
//! Data object, read by `handwritten::handwritten_for_object_via`.
//!
//! Every fixture is scoped to its own catalog, profiles and engines, so the
//! tests share one database without seeing each other's rows.
//!
//! DB-gated; FAILS without a database:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test handwritten_object

use egret_metastore::Db;
use egret_perm_compile::handwritten::{
    handwritten_for_object_via, HandWrittenError, HandWrittenPolicy, ListedState, MatchWord,
    NotDeterminedReason, ObjectHandWritten, ObjectTarget, TagOn,
};
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

pub struct Env {
    pub pool: PgPool,
    pub policy: PolicyStore,
}

pub async fn env() -> Env {
    let dsn = egret_test_db::url()
        .await
        .expect("a database is required (DATABASE_URL)");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("trino def");
    egret_policy::seed_starrocks(&policy).await.expect("sr def");
    egret_policy::seed_tag(&policy).await.expect("tag def");
    Env {
        pool: db.pool.clone(),
        policy,
    }
}

/// A catalog with a warehouse, a Trino profile (with a tag service) and a
/// StarRocks profile, each bound to a provisioned engine attached to it.
struct Fx {
    catalog: Uuid,
    catalog_name: String,
    warehouse: Uuid,
    trino: i64,
    trino_name: String,
    tag: i64,
    tag_name: String,
    sr: i64,
    sr_name: String,
}

async fn catalog(e: &Env) -> (Uuid, String, Uuid) {
    let p = &e.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$1) RETURNING id")
            .bind(unique("hwo-ws-"))
            .fetch_one(p)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("hwo-sc-"))
    .fetch_one(p)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("hwo_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(p)
    .await
    .unwrap();
    let name = unique("cl_");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&name)
    .bind(wh)
    .fetch_one(p)
    .await
    .unwrap();
    (id, name, wh)
}

async fn engine(e: &Env, ty: &str, profile: Option<i64>, catalog: Uuid, origin: &str) -> Uuid {
    let external = origin == "external";
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id, origin, \
                                          external_host, external_port, ranger_plugin_version) \
         VALUES ($1, $2, $5, '{}'::jsonb, $3, $4, $6, $7, $8) RETURNING id",
    )
    .bind(unique("hwo-eng-"))
    .bind(ty)
    .bind(profile)
    .bind(origin)
    .bind(if external { "" } else { "egret" })
    .bind(external.then_some("trino.example"))
    .bind(external.then_some(8080i32))
    .bind(external.then_some("2.8.0"))
    .fetch_one(&e.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(id)
    .bind(catalog)
    .execute(&e.pool)
    .await
    .unwrap();
    id
}

async fn fixture(e: &Env) -> Fx {
    let (catalog, catalog_name, warehouse) = catalog(e).await;
    let tag_name = unique("hwo-tag-");
    let tag = e
        .policy
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap()
        .id;
    let trino_name = unique("hwo-tr-");
    let trino = e
        .policy
        .create_service(&trino_name, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap()
        .id;
    let sr_name = unique("hwo-sr-");
    let sr = e
        .policy
        .create_service(&sr_name, "starrocks", &json!({}))
        .await
        .unwrap()
        .id;
    engine(e, "trino", Some(trino), catalog, "provisioned").await;
    engine(e, "starrocks", Some(sr), catalog, "provisioned").await;
    Fx {
        catalog,
        catalog_name,
        warehouse,
        trino,
        trino_name,
        tag,
        tag_name,
        sr,
        sr_name,
    }
}

async fn insert(e: &Env, service: i64, name: &str, body: Value, enabled: bool, by: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(service)
    .bind(name)
    .bind(body)
    .bind(enabled)
    .bind(by)
    .fetch_one(&e.pool)
    .await
    .unwrap()
}

fn lv(values: &[&str]) -> Value {
    json!({ "values": values })
}

/// An access allow for `ua` with `resources`.
fn allow(name: &str, service: &str, resources: Value) -> Value {
    json!({
        "name": name, "service": service, "policyType": 0, "isEnabled": true,
        "resources": resources,
        "policyItems": [{ "users": ["ua"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    })
}

fn trino_res(c: &str, s: &str, t: &str, col: &str) -> Value {
    json!({ "catalog": lv(&[c]), "schema": lv(&[s]), "table": lv(&[t]), "column": lv(&[col]) })
}

async fn read(e: &Env, target: ObjectTarget) -> Result<ObjectHandWritten, HandWrittenError> {
    let mut conn = e.pool.acquire().await.unwrap();
    handwritten_for_object_via(&mut conn, &target).await
}

fn t(catalog: Uuid, s: Option<&str>, tb: Option<&str>, c: Option<&str>) -> ObjectTarget {
    ObjectTarget {
        catalog_id: catalog,
        schema: s.map(Into::into),
        table: tb.map(Into::into),
        column: c.map(Into::into),
    }
}

fn row<'a>(r: &'a ObjectHandWritten, name: &str) -> Option<&'a HandWrittenPolicy> {
    r.policies.iter().find(|p| p.name == name)
}

#[tokio::test]
async fn a_wildcard_schema_applies_through_a_pattern_and_nowhere_else() {
    let e = env().await;
    let f = fixture(&e).await;
    let c = f.catalog_name.as_str();
    insert(
        &e,
        f.trino,
        "wild",
        allow("wild", &f.trino_name, trino_res(c, "s", "*", "*")),
        true,
        "operator",
    )
    .await;

    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    assert_eq!(r.state, ListedState::Listed);
    let p = row(&r, "wild").expect("listed on s.t");
    assert_eq!(p.r#match, MatchWord::Applies);
    assert!(p.through_pattern);
    assert!(p.via_tag.is_none());
    assert!(p.enabled);
    assert!(!p.in_tag_service);
    assert_eq!(p.service, f.trino_name);
    // Its full items.
    assert_eq!(
        serde_json::to_value(&p.items).unwrap(),
        json!([{ "bucket": "allow", "users": ["ua"], "groups": [], "roles": [], "everyone": false,
                 "accesses": ["select"], "maskType": null, "rowFilter": null }])
    );
    // On the schema and on a column too.
    let r = read(&e, t(f.catalog, Some("s"), None, None)).await.unwrap();
    assert_eq!(row(&r, "wild").unwrap().r#match, MatchWord::Applies);
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), Some("email")))
        .await
        .unwrap();
    assert_eq!(row(&r, "wild").unwrap().r#match, MatchWord::Applies);
    // Not on another schema.
    let r = read(&e, t(f.catalog, Some("s2"), Some("v"), None))
        .await
        .unwrap();
    assert!(row(&r, "wild").is_none());
}

#[tokio::test]
async fn an_exclude_reaches_every_table_but_the_excluded_one() {
    let e = env().await;
    let f = fixture(&e).await;
    let res = json!({ "catalog": lv(&[&f.catalog_name]), "schema": lv(&["s"]),
                      "table": { "values": ["u"], "isExcludes": true }, "column": lv(&["*"]) });
    insert(
        &e,
        f.trino,
        "not-u",
        allow("not-u", &f.trino_name, res),
        true,
        "operator",
    )
    .await;
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    let p = row(&r, "not-u").unwrap();
    assert_eq!((p.r#match, p.through_pattern), (MatchWord::Applies, true));
    let r = read(&e, t(f.catalog, Some("s"), Some("u"), None))
        .await
        .unwrap();
    assert!(row(&r, "not-u").is_none(), "the excluded table");
}

#[tokio::test]
async fn a_recursive_level_and_a_case_differing_literal() {
    let e = env().await;
    let f = fixture(&e).await;
    let rec = json!({ "catalog": lv(&[&f.catalog_name]), "schema": { "values": ["s"], "isRecursive": true },
                      "table": lv(&["t"]), "column": lv(&["*"]) });
    insert(
        &e,
        f.trino,
        "rec",
        allow("rec", &f.trino_name, rec),
        true,
        "operator",
    )
    .await;
    let upper = f.catalog_name.to_uppercase();
    insert(
        &e,
        f.trino,
        "case",
        allow("case", &f.trino_name, trino_res(&upper, "S", "T", "*")),
        true,
        "operator",
    )
    .await;
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    let p = row(&r, "rec").unwrap();
    assert_eq!((p.r#match, p.through_pattern), (MatchWord::Applies, true));
    let p = row(&r, "case").unwrap();
    assert_eq!((p.r#match, p.through_pattern), (MatchWord::Applies, false));
}

#[tokio::test]
async fn match_levels_follow_q2() {
    let e = env().await;
    let f = fixture(&e).await;
    let c = f.catalog_name.as_str();
    insert(
        &e,
        f.trino,
        "full",
        allow("full", &f.trino_name, trino_res(c, "s", "*", "*")),
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        f.trino,
        "schema-only",
        allow(
            "schema-only",
            &f.trino_name,
            json!({ "catalog": lv(&[c]), "schema": lv(&["s"]) }),
        ),
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        f.trino,
        "one-col",
        allow("one-col", &f.trino_name, trino_res(c, "s", "t", "email")),
        true,
        "operator",
    )
    .await;

    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    assert_eq!(row(&r, "full").unwrap().r#match, MatchWord::Applies);
    assert_eq!(row(&r, "schema-only").unwrap().r#match, MatchWord::Parent);
    assert_eq!(row(&r, "one-col").unwrap().r#match, MatchWord::Part);
    // Ordered applies, part, parent.
    let order: Vec<MatchWord> = r.policies.iter().map(|p| p.r#match).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);

    let r = read(&e, t(f.catalog, Some("s"), Some("t"), Some("email")))
        .await
        .unwrap();
    let p = row(&r, "one-col").unwrap();
    assert_eq!((p.r#match, p.through_pattern), (MatchWord::Applies, false));
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), Some("id")))
        .await
        .unwrap();
    assert!(row(&r, "one-col").is_none());
}

#[tokio::test]
async fn a_starrocks_profile_matches_on_its_database_level() {
    let e = env().await;
    let f = fixture(&e).await;
    let res = json!({ "catalog": lv(&[&f.catalog_name]), "database": lv(&["s"]),
                      "table": lv(&["t"]), "column": lv(&["*"]) });
    insert(
        &e,
        f.sr,
        "sr-t",
        allow("sr-t", &f.sr_name, res),
        true,
        "operator",
    )
    .await;
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    let p = row(&r, "sr-t").unwrap();
    assert_eq!(
        (p.r#match, p.service.as_str()),
        (MatchWord::Applies, f.sr_name.as_str())
    );
    assert_eq!(r.profiles.len(), 2);
    let sr = r.profiles.iter().find(|p| p.service_id == f.sr).unwrap();
    assert_eq!(sr.tag_service, None);
    let tr = r.profiles.iter().find(|p| p.service_id == f.trino).unwrap();
    assert_eq!(tr.tag_service.as_deref(), Some(f.tag_name.as_str()));
    assert_eq!(tr.engines.len(), 1);
}

async fn tag_on(e: &Env, f: &Fx, name: &str, table: &str, column: &str) {
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("hwo-k-"))
        .bind(name)
        .fetch_one(&e.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', $3, $4)",
    )
    .bind(id)
    .bind(f.warehouse)
    .bind(table)
    .bind(column)
    .execute(&e.pool)
    .await
    .unwrap();
}

fn tag_deny(name: &str, service: &str, tag: &str) -> Value {
    json!({
        "name": name, "service": service, "serviceType": "tag", "policyType": 0, "isEnabled": true,
        "resources": { "tag": lv(&[tag]) },
        "denyPolicyItems": [{ "users": ["ua"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    })
}

#[tokio::test]
async fn a_tag_policy_reaches_through_the_objects_tags() {
    let e = env().await;
    let f = fixture(&e).await;
    let table_tag = unique("TT");
    let col_tag = unique("CT");
    tag_on(&e, &f, &table_tag, "t", "").await;
    tag_on(&e, &f, &col_tag, "u", "secret").await;
    insert(
        &e,
        f.tag,
        "by-table",
        tag_deny("by-table", &f.tag_name, &table_tag),
        true,
        "operator",
    )
    .await;
    insert(
        &e,
        f.tag,
        "by-col",
        tag_deny("by-col", &f.tag_name, &col_tag),
        true,
        "operator",
    )
    .await;

    // A table tag: applies to the table and to its columns.
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    let p = row(&r, "by-table").unwrap();
    assert_eq!(p.r#match, MatchWord::Applies);
    assert!(p.in_tag_service);
    assert_eq!(p.service, f.tag_name);
    let v = p.via_tag.as_ref().unwrap();
    assert_eq!(
        (v.name.as_str(), v.on, v.column.as_deref()),
        (table_tag.as_str(), TagOn::This, None)
    );
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), Some("email")))
        .await
        .unwrap();
    let p = row(&r, "by-table").unwrap();
    assert_eq!(
        (p.r#match, p.via_tag.as_ref().unwrap().on),
        (MatchWord::Applies, TagOn::Parent)
    );
    assert!(row(&r, "by-col").is_none());

    // A column tag: part of its table, applies to the column.
    let r = read(&e, t(f.catalog, Some("s"), Some("u"), None))
        .await
        .unwrap();
    let p = row(&r, "by-col").unwrap();
    assert_eq!(p.r#match, MatchWord::Part);
    let v = p.via_tag.as_ref().unwrap();
    assert_eq!((v.on, v.column.as_deref()), (TagOn::Below, Some("secret")));
    let r = read(&e, t(f.catalog, Some("s"), Some("u"), Some("SECRET")))
        .await
        .unwrap();
    assert_eq!(row(&r, "by-col").unwrap().r#match, MatchWord::Applies);
    // The schema: both below.
    let r = read(&e, t(f.catalog, Some("s"), None, None)).await.unwrap();
    assert_eq!(row(&r, "by-table").unwrap().r#match, MatchWord::Part);
    assert_eq!(row(&r, "by-col").unwrap().r#match, MatchWord::Part);
}

#[tokio::test]
async fn seeded_rows_are_counted_and_compiled_rows_never_listed() {
    let e = env().await;
    let f = fixture(&e).await;
    let c = f.catalog_name.as_str();
    insert(
        &e,
        f.trino,
        "seed-1",
        allow("seed-1", &f.trino_name, trino_res(c, "*", "*", "*")),
        true,
        "infra",
    )
    .await;
    insert(
        &e,
        f.trino,
        "seed-2",
        allow("seed-2", &f.trino_name, trino_res(c, "s", "*", "*")),
        true,
        "infra",
    )
    .await;
    insert(
        &e,
        f.trino,
        "seed-miss",
        allow("seed-miss", &f.trino_name, trino_res(c, "x", "*", "*")),
        true,
        "infra",
    )
    .await;
    insert(
        &e,
        f.trino,
        "perm-grant-x",
        allow("perm-grant-x", &f.trino_name, trino_res(c, "*", "*", "*")),
        true,
        "compiler",
    )
    .await;
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    assert!(r.policies.is_empty(), "{:?}", r.policies);
    assert_eq!(r.seeded.len(), 1);
    assert_eq!(
        (r.seeded[0].service.as_str(), r.seeded[0].count),
        (f.trino_name.as_str(), 2)
    );
}

#[tokio::test]
async fn enabled_comes_from_the_column_and_a_bad_body_is_unknown() {
    let e = env().await;
    let f = fixture(&e).await;
    let c = f.catalog_name.as_str();
    // The body says isEnabled: true; the column says false.
    insert(
        &e,
        f.trino,
        "off",
        allow("off", &f.trino_name, trino_res(c, "s", "t", "*")),
        false,
        "operator",
    )
    .await;
    insert(
        &e,
        f.trino,
        "bad",
        json!({ "name": "bad", "resources": "not a map" }),
        true,
        "operator",
    )
    .await;
    let r = read(&e, t(f.catalog, Some("s"), Some("t"), None))
        .await
        .unwrap();
    let p = row(&r, "off").unwrap();
    assert!(!p.enabled);
    assert_eq!(p.r#match, MatchWord::Applies);
    let p = row(&r, "bad").unwrap();
    assert_eq!(p.r#match, MatchWord::Unknown);
    assert!(p.parse_error.is_some());
    assert_eq!(r.policies.last().unwrap().name, "bad", "unknown sorts last");
}

#[tokio::test]
async fn states_when_nothing_can_be_determined() {
    let e = env().await;
    // No engine.
    let (cat, _, _) = catalog(&e).await;
    let r = read(&e, t(cat, Some("s"), None, None)).await.unwrap();
    assert_eq!(
        (r.state, r.reason),
        (
            ListedState::NotDetermined,
            Some(NotDeterminedReason::NoEngineAttached)
        )
    );
    assert!(r.policies.is_empty());
    // Engines without a profile.
    let (cat, _, _) = catalog(&e).await;
    engine(&e, "trino", None, cat, "provisioned").await;
    let r = read(&e, t(cat, None, None, None)).await.unwrap();
    assert_eq!(
        (r.state, r.reason),
        (
            ListedState::NotDetermined,
            Some(NotDeterminedReason::NoProfile)
        )
    );
}

#[tokio::test]
async fn an_external_engines_profile_is_listed_from_the_database() {
    let e = env().await;
    let (cat, name, _) = catalog(&e).await;
    let tag_name = unique("hwo-xtag-");
    e.policy
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap();
    let pname = unique("hwo-x-");
    let prof = e
        .policy
        .create_service(&pname, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap()
        .id;
    let eng = engine(&e, "trino", Some(prof), cat, "external").await;
    insert(
        &e,
        prof,
        "x-all",
        allow("x-all", &pname, trino_res(&name, "*", "*", "*")),
        true,
        "operator",
    )
    .await;
    let r = read(&e, t(cat, Some("s"), Some("t"), None)).await.unwrap();
    assert_eq!(r.state, ListedState::Listed);
    assert_eq!(r.profiles[0].engines[0].id, eng);
    assert_eq!(r.profiles[0].engines[0].origin, "external");
    assert_eq!(row(&r, "x-all").unwrap().r#match, MatchWord::Applies);
}

#[tokio::test]
async fn refusals() {
    let e = env().await;
    let f = fixture(&e).await;
    for bad in [
        t(f.catalog, None, Some("t"), None),
        t(f.catalog, Some("s"), None, Some("c")),
        t(f.catalog, Some(""), None, None),
        t(f.catalog, Some("s"), Some("*"), None),
    ] {
        assert!(matches!(
            read(&e, bad).await,
            Err(HandWrittenError::BadRequest(_))
        ));
    }
    assert!(matches!(
        read(&e, t(Uuid::new_v4(), None, None, None)).await,
        Err(HandWrittenError::NotFound)
    ));
}
