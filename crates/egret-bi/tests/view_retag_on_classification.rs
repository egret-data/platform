//! View governance B3 (Task 9): a view's lineage-derived column tags follow
//! its base tables' classifications, in the transaction of the base write and
//! in ONE tag-version bump per affected service.
//!
//! The lineage is written through the Task 7 API (as the hold and the sweep
//! write it); the base write goes through `CatalogTagRepo` and
//! `ClassificationPolicyRepo`, the paths an admin uses.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-bi --test view_retag_on_classification

use std::collections::BTreeSet;

use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::ClassificationPolicyRepo;
use egret_metastore::Db;
use egret_perm_compile::lineage::{self, ViewRef};
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// `(tag, column, from_lineage)`.
type Row = (Uuid, String, bool);

struct Fx {
    db: Db,
    service: i64,
    /// The base's warehouse, and a second one attached to the same engine.
    w1: Uuid,
    w2: Uuid,
    c1: String,
    c2: String,
    /// A clearance-bearing tag.
    pii: Uuid,
    repo: CatalogTagRepo,
}

async fn warehouse(pool: &sqlx::PgPool) -> Uuid {
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'VR') RETURNING id")
            .bind(unique("vr-ws"))
            .fetch_one(pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, 'us-east-1', 'egret-vr-test', 'AKIATEST') RETURNING id",
    )
    .bind(unique("vr-sc"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(unique("vr_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A registry catalog over `warehouse`, attached to `engine`; its name.
async fn catalog(pool: &sqlx::PgPool, warehouse: Uuid, engine: Uuid) -> String {
    let name = unique("vr_cat");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&name)
    .bind(warehouse)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine)
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    name
}

async fn new_tag(pool: &sqlx::PgPool, gated: bool) -> Uuid {
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("vr-tag"))
        .bind(unique("VrTag"))
        .fetch_one(pool)
        .await
        .unwrap();
    if gated {
        sqlx::query(
            "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
             VALUES ($1, $2, '{}')",
        )
        .bind(tag)
        .bind(vec![unique("vr-role")])
        .execute(pool)
        .await
        .unwrap();
    }
    tag
}

async fn setup() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = &db.pool;
    // Every test in this binary seeds this def at once, against one database.
    // `ON CONFLICT (id)` makes only the primary key the arbiter, so a racer that
    // misses the other's uncommitted `id` still collides on the `name` UNIQUE
    // index and raises `authz_service_defs_name_key` instead of doing nothing.
    // With no conflict target every unique index is an arbiter.
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1, $2, '{}'::jsonb) \
         ON CONFLICT DO NOTHING",
    )
    .bind(950951_i64)
    .bind("trino_vr")
    .execute(pool)
    .await
    .unwrap();
    let service: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'trino_vr', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("trino_vr"))
    .fetch_one(pool)
    .await
    .unwrap();
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(unique("vr-eng"))
    .bind(service)
    .fetch_one(pool)
    .await
    .unwrap();
    let w1 = warehouse(pool).await;
    let w2 = warehouse(pool).await;
    let c1 = catalog(pool, w1, engine).await;
    let c2 = catalog(pool, w2, engine).await;
    let pii = new_tag(pool, true).await;
    Some(Fx {
        repo: CatalogTagRepo::new(db.clone()),
        db,
        service,
        w1,
        w2,
        c1,
        c2,
        pii,
    })
}

impl Fx {
    fn view(&self, catalog: &str, name: &str) -> ViewRef {
        ViewRef {
            catalog: catalog.to_string(),
            schema: "s".into(),
            name: name.into(),
        }
    }

    /// Record `view`'s lineage over `tables` (in `s`) and its columns, as the
    /// hold and the sweep record them.
    async fn lineage(&self, view: &ViewRef, tables: &[(&str, &str)], columns: &[&str]) {
        let tables: BTreeSet<_> = tables
            .iter()
            .map(|(c, t)| (c.to_string(), "s".to_string(), t.to_string()))
            .collect();
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        let mut conn = self.db.pool.acquire().await.unwrap();
        lineage::write_lineage(&mut conn, view, &tables)
            .await
            .unwrap();
        lineage::set_view_columns(&mut conn, view, Some(&columns))
            .await
            .unwrap();
    }

    async fn rows(&self, warehouse: Uuid, table: &str) -> BTreeSet<Row> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT tag_id, column_name, from_lineage FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 AND schema_name = 's' AND table_name = $2",
        )
        .bind(warehouse)
        .bind(table)
        .fetch_all(&self.db.pool)
        .await
        .unwrap();
        rows.into_iter().collect()
    }

    async fn tag_version(&self) -> i64 {
        sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(self.service)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn classify_base(&self, tag: Uuid) {
        self.repo
            .assign_catalog(tag, self.w1, "s", "pii", "ssn", None)
            .await
            .expect("classify the base");
    }

    async fn declassify_base(&self, tag: Uuid) {
        self.repo
            .unassign_catalog(tag, self.w1, "s", "pii", "ssn")
            .await
            .expect("declassify the base");
    }
}

fn derived(tag: Uuid, columns: &[&str]) -> BTreeSet<Row> {
    columns.iter().map(|c| (tag, c.to_string(), true)).collect()
}

/// Classifying a base column re-tags every dependent view, in the base's
/// warehouse and in another one, on every column, in ONE tag version: the
/// engine's one service moves by exactly one.
#[tokio::test]
async fn classifying_a_base_column_retags_every_dependent_view_in_one_bump() {
    let Some(fx) = setup().await else { return };
    let same = fx.view(&fx.c1.clone(), &unique("v_same"));
    let other = fx.view(&fx.c2.clone(), &unique("v_other"));
    let unrelated = fx.view(&fx.c1.clone(), &unique("v_unrel"));
    fx.lineage(&same, &[(&fx.c1, "pii")], &["id", "ssn"]).await;
    fx.lineage(&other, &[(&fx.c1, "pii"), (&fx.c2, "x")], &["a"])
        .await;
    fx.lineage(&unrelated, &[(&fx.c1, "orders")], &["id"]).await;
    let before = fx.tag_version().await;

    fx.classify_base(fx.pii).await;

    assert_eq!(
        fx.rows(fx.w1, &same.name).await,
        derived(fx.pii, &["id", "ssn"]),
        "every column of the view in the base's warehouse"
    );
    assert_eq!(
        fx.rows(fx.w2, &other.name).await,
        derived(fx.pii, &["a"]),
        "a view in another warehouse that reads the base"
    );
    assert!(fx.rows(fx.w1, &unrelated.name).await.is_empty());
    assert_eq!(
        fx.tag_version().await,
        before + 1,
        "the base write and its dependent views reach the plugins in one tag version"
    );
}

/// Declassifying removes the views' derived tags: the lineage is computed, so
/// this is not removal on uncertainty. An admin's manual tag on a view column
/// stays, including one on a coordinate the lineage had derived.
#[tokio::test]
async fn declassifying_removes_the_derived_view_tags_and_keeps_manual_ones() {
    let Some(fx) = setup().await else { return };
    let same = fx.view(&fx.c1.clone(), &unique("v_same"));
    let other = fx.view(&fx.c2.clone(), &unique("v_other"));
    fx.lineage(&same, &[(&fx.c1, "pii")], &["id", "ssn"]).await;
    fx.lineage(&other, &[(&fx.c1, "pii")], &["a"]).await;
    fx.classify_base(fx.pii).await;
    assert_eq!(fx.rows(fx.w2, &other.name).await, derived(fx.pii, &["a"]));
    // By hand: another tag on `id`, and the derived `pii` on `ssn` again.
    let manual = new_tag(&fx.db.pool, false).await;
    fx.repo
        .assign_catalog(manual, fx.w1, "s", &same.name, "id", None)
        .await
        .unwrap();
    fx.repo
        .assign_catalog(fx.pii, fx.w1, "s", &same.name, "ssn", None)
        .await
        .unwrap();
    let before = fx.tag_version().await;

    fx.declassify_base(fx.pii).await;

    assert_eq!(
        fx.rows(fx.w1, &same.name).await,
        BTreeSet::from([
            (manual, "id".to_string(), false),
            (fx.pii, "ssn".to_string(), false)
        ]),
        "the derived pii on `id` goes; the two manual assignments stay"
    );
    assert!(fx.rows(fx.w2, &other.name).await.is_empty());
    assert_eq!(fx.tag_version().await, before + 1);
}

/// A view whose lineage is `failed` keeps its tags when its base is
/// declassified: tags are never removed on uncertainty.
#[tokio::test]
async fn a_view_whose_lineage_failed_keeps_its_old_tags() {
    let Some(fx) = setup().await else { return };
    let v = fx.view(&fx.c1.clone(), &unique("v_failed"));
    fx.lineage(&v, &[(&fx.c1, "pii")], &["id", "ssn"]).await;
    fx.classify_base(fx.pii).await;
    assert_eq!(
        fx.rows(fx.w1, &v.name).await,
        derived(fx.pii, &["id", "ssn"])
    );
    {
        let mut conn = fx.db.pool.acquire().await.unwrap();
        lineage::record_lineage_failure(&mut conn, &v, "engine trino-1: unreachable")
            .await
            .unwrap();
        assert!(matches!(
            lineage::recorded_lineage(&mut conn, &v).await.unwrap(),
            lineage::RecordedLineage::Failed { .. }
        ));
        // The retag itself leaves it, and says why.
        let mut tx = fx.db.pool.begin().await.unwrap();
        let got = egret_perm_compile::retag_view_in_tx(&mut tx, &v)
            .await
            .unwrap();
        assert!(
            matches!(&got, egret_perm_compile::ViewRetag::Kept { reason } if reason.contains("unreachable")),
            "{got:?}"
        );
        tx.rollback().await.unwrap();
    }

    fx.declassify_base(fx.pii).await;

    assert_eq!(
        fx.rows(fx.w1, &v.name).await,
        derived(fx.pii, &["id", "ssn"]),
        "a failed lineage keeps the view's tags"
    );
}

/// A tag that loses its clearance no longer travels to the views (only
/// clearance-bearing tags do, `lineage::view_tag_set`); one that gains it does.
/// Each change bumps the view's service once.
#[tokio::test]
async fn a_tag_gaining_or_losing_its_clearance_retags_the_views() {
    let Some(fx) = setup().await else { return };
    let v = fx.view(&fx.c1.clone(), &unique("v_gate"));
    fx.lineage(&v, &[(&fx.c1, "pii")], &["id"]).await;
    fx.classify_base(fx.pii).await;
    assert_eq!(fx.rows(fx.w1, &v.name).await, derived(fx.pii, &["id"]));
    let policies = ClassificationPolicyRepo::new(fx.db.clone(), egret_events::EventBus::new());

    let before = fx.tag_version().await;
    policies
        .clear_policy(fx.pii, &test_actor(&fx.db.pool).await)
        .await
        .expect("clear");
    assert!(fx.rows(fx.w1, &v.name).await.is_empty());
    assert_eq!(fx.tag_version().await, before + 1);

    policies
        .set_policy(
            fx.pii,
            &[unique("vr-role")],
            &[],
            &test_actor(&fx.db.pool).await,
        )
        .await
        .expect("set");
    assert_eq!(fx.rows(fx.w1, &v.name).await, derived(fx.pii, &["id"]));
    assert_eq!(fx.tag_version().await, before + 2);
}

/// Task 9 fix round 1, M4: the base and a dependent view are served by two
/// profiles, each with its own tag service (the Trino profile / tag service
/// split). A classification moves each of the four services by exactly one.
#[tokio::test]
async fn one_classification_moves_each_affected_service_by_exactly_one() {
    let Some(fx) = setup().await else { return };
    let pool = &fx.db.pool;
    let policy = egret_policy::PolicyStore::new(fx.db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    // Two profiles, each with a tag service; one engine each.
    let mut services = Vec::new();
    let mut engines = Vec::new();
    for _ in 0..2 {
        let tag_service = unique("vr_tag");
        let tag_id = policy
            .create_service(&tag_service, "tag", &serde_json::json!({}))
            .await
            .expect("tag service")
            .id;
        let profile = policy
            .create_service(
                &unique("trino_vr"),
                "trino",
                &serde_json::json!({ "tagService": tag_service }),
            )
            .await
            .expect("profile")
            .id;
        let engine: Uuid = sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
             VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
        )
        .bind(unique("vr-eng"))
        .bind(profile)
        .fetch_one(pool)
        .await
        .unwrap();
        services.extend([profile, tag_id]);
        engines.push(engine);
    }
    // The base's warehouse on engine 0, the view's on engine 1.
    let wb = warehouse(pool).await;
    let wv = warehouse(pool).await;
    let cb = catalog(pool, wb, engines[0]).await;
    let cv = catalog(pool, wv, engines[1]).await;
    let v = fx.view(&cv, &unique("v_two"));
    fx.lineage(&v, &[(&cb, "pii")], &["id"]).await;
    let version = |id: i64| async move {
        sqlx::query_scalar::<_, i64>("SELECT tag_version FROM policy.authz_services WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    };
    let mut before = Vec::new();
    for &s in &services {
        before.push(version(s).await);
    }

    fx.repo
        .assign_catalog(fx.pii, wb, "s", "pii", "ssn", None)
        .await
        .expect("classify the base");

    assert_eq!(fx.rows(wv, &v.name).await, derived(fx.pii, &["id"]));
    for (i, &s) in services.iter().enumerate() {
        assert_eq!(
            version(s).await,
            before[i] + 1,
            "service {s} (index {i}) moves by exactly one"
        );
    }
}

/// DG #3: the acting principal of a clearance write, for a user that exists.
fn principal_of(id: uuid::Uuid) -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(id),
        email: "actor@egret.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}

/// DG #3: a clearance write takes its acting principal, stored as
/// `created_by` (which references `users(id)`), so this is a real user.
async fn test_actor(pool: &sqlx::PgPool) -> egret_core::Principal {
    let id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!(
                "actor_{}@egret.local",
                uuid::Uuid::new_v4().simple()
            ))
            .fetch_one(pool)
            .await
            .expect("insert actor");
    principal_of(id)
}
