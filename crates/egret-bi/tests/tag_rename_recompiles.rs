//! SP4a whole-branch review, Important 3 — D6's "a rename recompiles instead
//! of breaking", which spec §6 requires a test for and which nothing
//! implemented.
//!
//! Two renames, opposite failure directions:
//!
//! - **A TAG rename was fail-OPEN.** A compiled Trino tag policy carries
//!   `tags.name` as resolved at compile time (D6), while `download_tags`
//!   serves `t.name` live. Renaming the tag left the two agreeing only until
//!   the next plugin restart or `lastKnownVersion=-1` poll, at which point the
//!   tag map carried the NEW name and the policy still named the OLD one — the
//!   mask stopped applying and the column returned real values.
//! - **A CATALOG rename would be fail-CLOSED** (every compiled grant naming a
//!   catalog the engine no longer has), and is closed by construction rather
//!   than by a trigger: `registry.catalogs.name` has no writer. That is the
//!   half of §6's requirement this file pins, because a trigger for an event
//!   that cannot happen would be untestable and untrue.
//!
//! # Why its own test BINARY
//!
//! `egret-test-db` gives each test PROCESS a private database, and a tag rule
//! is installation-wide by design (D5): the rename recompiles EVERY
//! compiler-targeted profile in the database. Sharing a binary with another
//! fixture's profile would mean recompiling that fixture's profile from
//! underneath it — the same reason `egret-perm-compile/tests/tag_service_routing.rs`
//! stands alone.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_bi::TagRepo;
use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Selector};
use egret_policy::PolicyStore;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    profile_id: i64,
    tag_service_id: i64,
    warehouse_id: Uuid,
    catalog_id: Uuid,
}

/// A Trino profile + its tag service + engine + registry catalog + warehouse.
async fn seed_trino_fixture(db: &Db) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_rename");
    let tag_service_name = format!("{profile_name}_tag");
    let tag_service = policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("create trino profile");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("rename-ws"))
            .bind("Rename WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-rename-test','AKIATEST') RETURNING id",
    )
    .bind(unique("rename-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("rename_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("rename-eng"))
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("rename_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    Fixture {
        profile_id: profile.id,
        tag_service_id: tag_service.id,
        warehouse_id,
        catalog_id,
    }
}

/// The `tag` resource value of the one compiled tag policy under `service_id`.
/// This is the datum the Ranger tag plugin matches on, and the one a rename
/// used to leave stale.
async fn compiled_tag_name(pool: &sqlx::PgPool, service_id: i64, policy_name: &str) -> String {
    let body: serde_json::Value = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(policy_name)
    .fetch_one(pool)
    .await
    .expect("the compiled tag policy must exist under the tag service");
    body["resources"]["tag"]["values"][0]
        .as_str()
        .expect("a tag policy names a tag")
        .to_string()
}

async fn tag_version(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("tag_version")
}

#[tokio::test]
async fn renaming_a_tag_recompiles_the_policies_that_carry_its_name() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());
    let tags = TagRepo::new(db.clone());

    let f = seed_trino_fixture(&db).await;

    // `tags.key` allows only lowercase ascii alphanumerics, ':' or '-'.
    let tag_key = format!("rename-pii-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let tag = tags
        .create(&tag_key, "Old Name", "Personal data", "#ff0000", None)
        .await
        .expect("create tag");
    // One catalog resource carries the tag, so the rename has a warehouse
    // whose bound services must stop 304-ing the tag download.
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag.id)
    .bind(f.warehouse_id)
    .execute(&pool)
    .await
    .expect("assign tag to a catalog resource");

    let rule_id = perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("rename-rule"),
            kind: RuleKind::Mask {
                mask_type: "MASK".to_string(),
                value_expr: None,
            },
            selector: Selector::Tag {
                key: tag_key.clone(),
            },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("create rule");
    egret_perm_compile::recompile_service(&perm, &policy, f.profile_id)
        .await
        .expect("first compile");

    let policy_name = format!("perm-rule-{rule_id}");
    assert_eq!(
        compiled_tag_name(&pool, f.tag_service_id, &policy_name).await,
        "Old Name",
        "D6 resolves key → NAME at compile time; that is what makes a rename break"
    );
    let profile_tv = tag_version(&pool, f.profile_id).await;
    let tag_service_tv = tag_version(&pool, f.tag_service_id).await;

    // The rename, through the real repo method the HTTP handler calls.
    tags.update(tag.id, Some("New Name"), None, None)
        .await
        .expect("rename")
        .expect("the tag exists");

    assert_eq!(
        compiled_tag_name(&pool, f.tag_service_id, &policy_name).await,
        "New Name",
        "a rename must recompile: the policy still naming the old tag stops applying the \
         moment a plugin restart re-reads the tag map, and the column returns real values"
    );
    assert_eq!(
        tag_version(&pool, f.profile_id).await,
        profile_tv + 1,
        "the tag MAP changed too — without this bump the plugin is answered 304 and matches \
         the recompiled policy against the old map"
    );
    assert_eq!(
        tag_version(&pool, f.tag_service_id).await,
        tag_service_tv + 1,
        "the tag service the plugin actually polls must bump as well"
    );

    // A non-rename update must not be mistaken for one — and must still work.
    let before = tag_version(&pool, f.profile_id).await;
    tags.update(tag.id, None, Some("Still personal data"), None)
        .await
        .expect("describe")
        .expect("the tag exists");
    assert_eq!(
        tag_version(&pool, f.profile_id).await,
        before,
        "editing a description changes no compiled name and must not churn every plugin"
    );
}

/// The catalog half of §6's requirement. A compiled access policy carries the
/// catalog NAME (`catalogs_for_service_via` → `compile_grant`'s
/// `catalog_name`), so a rename would leave every grant naming a catalog the
/// engine no longer has — a silent, total, fail-closed outage. There is no
/// recompile trigger for it because there is no rename: `registry.catalogs.name`
/// is written exactly once, by the insert.
///
/// Asserted through the store's own update method rather than by grepping for
/// an `UPDATE`, because the claim that matters is behavioural — and if a
/// rename is ever added, this test is what makes adding the trigger
/// unavoidable rather than optional.
#[tokio::test]
async fn a_registry_catalog_cannot_be_renamed_so_no_grant_can_be_orphaned() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let f = seed_trino_fixture(&db).await;

    let original: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(f.catalog_id)
        .fetch_one(&pool)
        .await
        .expect("catalog name");

    let registry = egret_catalog_registry::RegistryStore::new(db.clone());
    let row = registry
        .get_catalog(f.catalog_id)
        .await
        .expect("get catalog")
        .expect("the fixture's catalog");
    let def = egret_catalog_registry::CatalogDef::parse(&row.kind, &row.config, &row.advanced)
        .expect("the fixture's catalog def must parse");
    registry
        .update_catalog(f.catalog_id, &def, Some("edited through the update path"))
        .await
        .expect("update catalog");

    let after: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(f.catalog_id)
        .fetch_one(&pool)
        .await
        .expect("catalog name");
    assert_eq!(
        after, original,
        "the one write path to registry.catalogs must leave the name alone — a rename would \
         orphan every compiled grant naming it, with no trigger to repair them"
    );
}
