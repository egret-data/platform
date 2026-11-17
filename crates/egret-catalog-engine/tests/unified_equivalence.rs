//! SP5b Task 5 (DB-gated): the unified decision path matches the legacy one.
//!
//! For a corpus of folded catalog grants, `unified_decide::covers` over a
//! `perm` snapshot returns the SAME boolean as `catalog_grant_covers_by` over
//! the role's `catalog.catalog_grants` — catalog/namespace/table × read/write, a
//! sibling that is not covered, a nested namespace, a deny (an uncovered probe),
//! and namespace-listing reach. This is the equivalence corpus Task 6 reuses.
//!
//! It also pins the switch reader's default (`catalog_rbac` with no row) and the
//! `unified` value, and the service-account principal resolution.

use egret_catalog_engine::grant_match::{catalog_grant_covers_by, grant_sees_namespace};
use egret_catalog_engine::store::CatalogStore;
use egret_catalog_engine::unified_decide::{
    catalog_grant_source, covers, service_account_username, unified_listing_grants,
    unified_name_matching_is_exact, Source,
};
use egret_perm_compile::catalog_rbac_migrate::fold_catalog;
use egret_perm_compile::opa::build::build_catalog_snapshot_via;
use sqlx::PgPool;
use uuid::Uuid;

/// One seeded catalog with three roles, each bound to its own service-account
/// client: `reader` (catalog read), `tablewriter` (write on `hr.salaries`) and
/// `nsmanager` (manage on `sales.eu`).
struct Fx {
    pool: PgPool,
    catalog_id: Uuid,
    warehouse_name: String,
    roles: Vec<Role>,
}

struct Role {
    name: String,
    client_id: String,
    username: String,
}

async fn user(pool: &PgPool, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, $1, $2) RETURNING id",
    )
    .bind(format!("{tag}@service.invalid"))
    .bind(tag)
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn role(pool: &PgPool, wh: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
         VALUES ($1, $2, 'external') RETURNING id",
    )
    .bind(wh)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("role")
}

async fn grant(
    pool: &PgPool,
    role: Uuid,
    sec: &str,
    ns: Option<Vec<&str>>,
    table: Option<&str>,
    privilege: &str,
) {
    sqlx::query(
        "INSERT INTO catalog.catalog_grants \
           (catalog_role_id, securable_type, namespace, table_name, privilege) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(role)
    .bind(sec)
    .bind(ns.map(|v| v.into_iter().map(String::from).collect::<Vec<_>>()))
    .bind(table)
    .bind(privilege)
    .execute(pool)
    .await
    .expect("grant");
}

async fn client(pool: &PgPool, wh: Uuid, role: Uuid, sa: Uuid, ws: Uuid, client_id: &str) {
    sqlx::query(
        "INSERT INTO catalog.catalog_clients \
           (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, \
            client_secret_hash, service_account_id) \
         VALUES ($1, 'external', $2, $3, $4, $5, 'h', $6)",
    )
    .bind(format!("c-{}", Uuid::new_v4()))
    .bind(ws)
    .bind(wh)
    .bind(role)
    .bind(client_id)
    .bind(sa)
    .execute(pool)
    .await
    .expect("client");
}

async fn fixture() -> Option<Fx> {
    let dsn = egret_test_db::url().await?;
    let db = egret_metastore::Db::connect(&dsn, 4)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();

    let wh = egret_test_db::seed_warehouse(&pool).await;
    let (ws, sc, warehouse_name): (Uuid, Uuid, String) = sqlx::query_as(
        "SELECT workspace_id, storage_conn_id, name FROM catalog.warehouses WHERE id = $1",
    )
    .bind(wh)
    .fetch_one(&pool)
    .await
    .expect("warehouse row");

    let tag = Uuid::new_v4().simple().to_string();
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id, storage_connection_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
            jsonb_build_object('warehouse_id', $2::text), \
            'storage_connection_id', $3::text), $2, $3) RETURNING id",
    )
    .bind(format!("c{}", &tag[..12]))
    .bind(wh)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .expect("registry catalog");

    let mut roles = Vec::new();
    // (securable_type, namespace, table, privilege) for one catalog grant.
    type GrantSpec = (
        &'static str,
        Option<&'static [&'static str]>,
        Option<&'static str>,
        &'static str,
    );
    let specs: [(&str, &[GrantSpec]); 3] = [
        ("reader", &[("catalog", None, None, "read")]),
        (
            "tablewriter",
            &[("table", Some(&["hr"]), Some("salaries"), "write")],
        ),
        (
            "nsmanager",
            &[("namespace", Some(&["sales", "eu"]), None, "manage")],
        ),
    ];
    for (rname, grants) in specs {
        let rid = role(&pool, wh, rname).await;
        for (sec, ns, table, priv_) in grants {
            grant(&pool, rid, sec, ns.map(|n| n.to_vec()), *table, priv_).await;
        }
        let uname = format!("{rname}-{}", &tag[..8]);
        let sa = user(&pool, &uname).await;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, scope, workspace_id, name) \
             VALUES ($1,'workspace',$2,$3)",
        )
        .bind(sa)
        .bind(ws)
        .bind(format!("n-{sa}"))
        .execute(&pool)
        .await
        .expect("sa");
        let client_id = format!("cid-{rname}-{}", &tag[..8]);
        client(&pool, wh, rid, sa, ws, &client_id).await;
        roles.push(Role {
            name: rname.to_string(),
            client_id,
            username: uname,
        });
    }

    // Fold the catalog RBAC into perm.grants (Task 2).
    let mut c = pool.acquire().await.unwrap();
    fold_catalog(&mut c, catalog_id, "task5-test")
        .await
        .expect("fold");

    Some(Fx {
        pool,
        catalog_id,
        warehouse_name,
        roles,
    })
}

fn ns(levels: &[&str]) -> Vec<String> {
    levels.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn the_switch_defaults_to_catalog_rbac_and_reads_unified() {
    let Some(f) = fixture().await else { return };
    let mut c = f.pool.acquire().await.unwrap();
    assert_eq!(
        catalog_grant_source(&mut c, f.catalog_id).await.unwrap(),
        Source::CatalogRbac,
        "no row means the legacy default"
    );
    sqlx::query(
        "INSERT INTO perm.catalog_grant_switch (catalog_id, source, switched_at, switched_by) \
         VALUES ($1, 'unified', now(), 'task5-test')",
    )
    .bind(f.catalog_id)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        catalog_grant_source(&mut c, f.catalog_id).await.unwrap(),
        Source::Unified
    );
}

#[tokio::test]
async fn service_account_username_resolves_the_catalog_client() {
    let Some(f) = fixture().await else { return };
    let mut c = f.pool.acquire().await.unwrap();
    for r in &f.roles {
        let got = service_account_username(&mut c, &r.client_id)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some(r.username.as_str()));
    }
    assert_eq!(
        service_account_username(&mut c, "no-such-client")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn covers_matches_catalog_grant_covers_by_on_a_corpus() {
    let Some(f) = fixture().await else { return };
    let store = CatalogStore::new(egret_metastore::Db {
        pool: f.pool.clone(),
    });
    let mut c = f.pool.acquire().await.unwrap();
    let snapshot = build_catalog_snapshot_via(&mut c, f.catalog_id)
        .await
        .expect("snapshot")
        .expect("catalog exists");
    let catalog_name: String =
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();

    // (namespace, table, is_write, expected) per role — the corpus.
    type Probe = (&'static [&'static str], Option<&'static str>, bool, bool);
    let corpus: [(&str, &[Probe]); 3] = [
        (
            "reader",
            &[
                (&["hr"], Some("anything"), false, true), // catalog read covers all reads
                (&["hr"], Some("anything"), true, false), // read grant never writes
                (&["x", "y"], None, false, true),         // catalog reaches any namespace (listing)
                (&["x"], None, true, false),              // no catalog write
            ],
        ),
        (
            "tablewriter",
            &[
                (&["hr"], Some("salaries"), false, true), // the granted table, read
                (&["hr"], Some("salaries"), true, true),  // and write
                (&["hr"], Some("other"), false, false),   // a sibling table — not covered (deny)
                (&["hr"], None, false, true),             // reach: the enclosing namespace
                (&["finance"], None, false, false),       // a sibling namespace — not reached
                (&["hr"], None, true, false), // a table grant confers no namespace write
            ],
        ),
        (
            "nsmanager",
            &[
                (&["sales", "eu"], Some("orders"), true, true), // manage covers table write
                (&["sales", "eu"], None, true, true),           // and namespace write
                (&["sales", "eu"], None, false, true),          // and listing
                (&["sales"], Some("x"), false, false), // the parent namespace is a different schema
                (&["sales", "eu", "deep"], Some("x"), false, false), // a nested namespace is not the granted one
            ],
        ),
    ];

    for (rname, probes) in corpus {
        let r = f.roles.iter().find(|r| r.name == rname).unwrap();
        // The legacy grant set for this role, as the guard/authorizer read it.
        let legacy = store
            .grants_for_role(&f.warehouse_name, rname)
            .await
            .expect("grants_for_role");
        for (levels, table, is_write, expected) in probes {
            let namespace = ns(levels);
            let legacy_ok = legacy.iter().any(|g| {
                catalog_grant_covers_by(g, Some(&namespace), *table, *is_write, &|a, b| a == b)
            });
            let unified_ok = covers(
                &snapshot,
                &r.username,
                &catalog_name,
                &namespace,
                *table,
                *is_write,
            );
            assert_eq!(
                legacy_ok, *expected,
                "legacy oracle wrong for {rname} {levels:?} table={table:?} write={is_write}"
            );
            assert_eq!(
                unified_ok, legacy_ok,
                "unified != legacy for {rname} {levels:?} table={table:?} write={is_write}"
            );
        }
    }
}

#[tokio::test]
async fn unified_listing_grants_reproduce_legacy_grant_sees_namespace() {
    let Some(f) = fixture().await else { return };
    let store = CatalogStore::new(egret_metastore::Db {
        pool: f.pool.clone(),
    });
    let mut c = f.pool.acquire().await.unwrap();
    // nsmanager holds a namespace grant on sales.eu. grant_sees_namespace must
    // see it on/above/under and NOT a sibling — identically on the reconstructed
    // unified grants and the legacy grants_for_role.
    let r = f.roles.iter().find(|r| r.name == "nsmanager").unwrap();
    let legacy = store
        .grants_for_role(&f.warehouse_name, "nsmanager")
        .await
        .unwrap();
    let unified = unified_listing_grants(&mut c, f.catalog_id, &r.username)
        .await
        .unwrap();
    let probes: [(&[&str], bool); 4] = [
        (&["sales"], true),               // above
        (&["sales", "eu"], true),         // on
        (&["sales", "eu", "deep"], true), // under
        (&["hr"], false),                 // sibling
    ];
    for (levels, expected) in probes {
        let nsv = ns(levels);
        let legacy_sees = legacy
            .iter()
            .any(|g| grant_sees_namespace(g, &nsv, &|a, b| a == b));
        let unified_sees = unified
            .iter()
            .any(|g| grant_sees_namespace(g, &nsv, &|a, b| a == b));
        assert_eq!(legacy_sees, expected, "legacy oracle for {levels:?}");
        assert_eq!(
            unified_sees, legacy_sees,
            "unified listing visibility != legacy for {levels:?}"
        );
    }
}

#[tokio::test]
async fn name_matching_exactness_flags_non_ascii_grant_names() {
    let Some(f) = fixture().await else { return };
    let mut c = f.pool.acquire().await.unwrap();
    assert!(
        unified_name_matching_is_exact(&mut c, f.catalog_id)
            .await
            .unwrap(),
        "ascii folded names: unified matches lakekeeper's collation exactly"
    );
    sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name) \
         VALUES ('allow', ARRAY['read'], $1, $2)",
    )
    .bind(f.catalog_id)
    .bind("caf\u{e9}")
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(
        !unified_name_matching_is_exact(&mut c, f.catalog_id)
            .await
            .unwrap(),
        "a non-ASCII folded name is NOT exact (Task 6 finalize must refuse it)"
    );
}

/// Important 2: the unified path is NO BROADER than the legacy name match.
/// lakekeeper's collation (`lakekeeper.case_insensitive`, ICU `und-u-ks-level2`)
/// is case-/width-/compatibility-/canonical-insensitive but accent-SENSITIVE, and
/// is never case-sensitive — so the hypothesised "grant sales, request Sales"
/// over-grant cannot happen (both fold case). perm-eval matches via
/// `str::to_lowercase`, which identifies only case variants — a subset of the
/// collation's folds — so `to_lowercase`-equal ⟹ collation-equal, i.e. unified
/// NEVER over-grants. This pins both halves: a case variant is allowed (folds, as
/// the collation does), and an accent variant is NOT allowed (stays distinct, as
/// the collation does). (The legacy `CatalogNameEq` path needs the `lakekeeper`
/// schema, which the egret test DB does not create, so this exercises the unified
/// side directly against those invariants.)
#[tokio::test]
async fn unified_name_matching_is_no_broader_than_the_collation() {
    let Some(f) = fixture().await else { return };
    let mut c = f.pool.acquire().await.unwrap();
    let snapshot = build_catalog_snapshot_via(&mut c, f.catalog_id)
        .await
        .unwrap()
        .unwrap();
    let catalog_name: String =
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    // tablewriter: write on hr.salaries.
    let t = f.roles.iter().find(|r| r.name == "tablewriter").unwrap();
    // Exact spelling, and a case variant: both allowed (to_lowercase folds case,
    // as the case-insensitive collation does — no under-grant).
    assert!(covers(
        &snapshot,
        &t.username,
        &catalog_name,
        &ns(&["hr"]),
        Some("salaries"),
        false
    ));
    assert!(covers(
        &snapshot,
        &t.username,
        &catalog_name,
        &ns(&["HR"]),
        Some("SALARIES"),
        false
    ));
    // An accent variant is NOT allowed: to_lowercase keeps accents distinct, as
    // the accent-sensitive collation does — so unified cannot over-fold accents.
    assert!(!covers(
        &snapshot,
        &t.username,
        &catalog_name,
        &ns(&["hr"]),
        Some("salarie\u{df}"),
        false
    ));
    assert!(!covers(
        &snapshot,
        &t.username,
        &catalog_name,
        &ns(&["h\u{e4}"]),
        Some("salaries"),
        false
    ));
    // The invariant that makes this hold: to_lowercase identifies only case
    // variants (a subset of the level-2 collation's folds).
    assert_eq!("SALARIES".to_lowercase(), "salaries");
    assert_ne!("cafe", "caf\u{e9}".to_lowercase());
}
