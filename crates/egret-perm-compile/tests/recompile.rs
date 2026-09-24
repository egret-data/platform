//! `recompile_service` (SP4a Task 7): recompiling a service's profile stays
//! true to the CURRENT `perm` model and the CURRENT tag assignments
//! underneath it. D5: on StarRocks a tag-selected rule is enumerated per
//! column at compile time (no tag service — upstream `starrocks#67458`), and
//! that enumeration goes stale the moment a new column gets the tag.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test recompile
//!
//! Every assertion below is scoped to the fixture's OWN `service_id` — this
//! workspace's test-db design shares one process-private database across
//! every concurrently-running `#[tokio::test]` in a binary (see
//! `crates/egret-perm/tests/assignment.rs`'s `grants_for` for the same
//! rationale), and asserts WHICH policy exists/changed, never only a bare
//! count — a count-only assertion is exactly what let two Criticals through
//! in Task 5 ("right count, wrong value").

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Rule,
    RuleKind, Scope, Selector,
};
use egret_perm_compile::{recompile_service, SourceKind};
use egret_policy::PolicyStore;
use serde_json::Value;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    perm: PermStore,
    policy: PolicyStore,
    service_id: i64,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    rule_id: Uuid,
}

/// A StarRocks profile with one engine bound to it, one registry catalog
/// attached to that engine, one `perm.rules` masking rule selecting tag
/// `"pii"`, and one column ("sales.orders.email") already carrying that tag.
/// Recompiles once during setup so the fixture starts from the state a real
/// tag-write pipeline would have left it in — `recompile_service` itself is
/// exercised here, not bypassed.
async fn seed_starrocks_engine_with_tag_rule(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");

    let service = policy
        .create_service(&unique("sr_recompile"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("recompile-ws"))
            .bind("Recompile WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-recompile-test','AKIATEST') RETURNING id",
    )
    .bind(unique("recompile-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("recompile_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("recompile-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("recompile_cat"))
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

    let tag_key = unique("pii");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, 'PII')")
        .bind(&tag_key)
        .execute(pool)
        .await
        .expect("tag");
    tag_column(pool, warehouse_id, "sales", "orders", "email", &tag_key).await;

    let rule = Rule {
        id: Uuid::nil(),
        name: unique("pii-hash"),
        kind: RuleKind::Mask {
            mask_type: "MASK_HASH".to_string(),
            value_expr: None,
        },
        selector: Selector::Tag {
            key: tag_key.clone(),
        },
        exempt: PrincipalSet::default(),
    };
    let rule_id = perm.create_rule(&rule).await.expect("create rule");

    let summary = recompile_service(&perm, &policy, service.id)
        .await
        .expect("initial recompile");
    assert_eq!(
        summary.created, 1,
        "the fixture's one pre-tagged column must compile to exactly one StarRocks masking policy"
    );

    Fixture {
        perm,
        policy,
        service_id: service.id,
        warehouse_id,
        catalog_id,
        rule_id,
    }
}

async fn tag_column(
    pool: &sqlx::PgPool,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
    column: &str,
    tag_key: &str,
) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         SELECT id, $2, $3, $4, $5 FROM tags WHERE key = $1",
    )
    .bind(tag_key)
    .bind(warehouse_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .execute(pool)
    .await
    .expect("tag column");
}

/// Policies for `service_id`, scoped — never a bare `policy.authz_policies`
/// total, which every concurrently-running test in this binary shares.
async fn policies_for(pool: &sqlx::PgPool, service_id: i64) -> Vec<(i64, String, Value)> {
    sqlx::query_as(
        "SELECT id, name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY id",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .expect("list policies")
}

async fn policy_version(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("policy_version")
}

#[tokio::test]
async fn tagging_a_new_column_recompiles_the_starrocks_profile() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let before = policies_for(pool, f.service_id).await;
    assert_eq!(before.len(), 1, "fixture seeded exactly one tagged column");
    let (email_id, email_name, email_body) = before[0].clone();

    let tag_key: String = sqlx::query_scalar(
        "SELECT t.key FROM tags t JOIN catalog_tag_assignments a ON a.tag_id = t.id \
         WHERE a.warehouse_id = $1 LIMIT 1",
    )
    .bind(f.warehouse_id)
    .fetch_one(pool)
    .await
    .expect("tag key");
    tag_column(pool, f.warehouse_id, "sales", "orders", "email_2", &tag_key).await;

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile after tagging a new column");
    assert_eq!(
        summary.created, 1,
        "exactly one NEW masking policy for the newly tagged column"
    );
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.deleted, 0);

    let after = policies_for(pool, f.service_id).await;
    assert_eq!(
        after.len(),
        before.len() + 1,
        "a newly tagged column must produce a new masking policy on StarRocks"
    );

    // The pre-existing "email" policy must be untouched: same id, same name,
    // byte-identical body — "writes only the difference" means the old
    // column's policy is not touched by a compile that only ADDS a column.
    let still_email = after
        .iter()
        .find(|(id, ..)| *id == email_id)
        .expect("the email policy must still exist, same id");
    assert_eq!(still_email.1, email_name);
    assert_eq!(
        still_email.2, email_body,
        "unrelated policy must be byte-identical, not just present"
    );

    // The NEW policy must actually name the new column, on the StarRocks
    // engine, as a masking policy — value-level, not just "a row appeared".
    let new_policy = after
        .iter()
        .find(|(id, ..)| *id != email_id)
        .expect("a new policy row must exist");
    assert_eq!(new_policy.2["policyType"], 1, "masking policy");
    assert_eq!(
        new_policy.2["serviceType"], "starrocks",
        "the StarRocks serviceType stamp must apply to this path too"
    );
    assert_eq!(
        new_policy.2["resources"]["column"]["values"][0], "email_2",
        "the new policy must be scoped to the newly tagged column"
    );
    assert_eq!(new_policy.2["resources"]["table"]["values"][0], "orders");
    assert_eq!(new_policy.2["resources"]["database"]["values"][0], "sales");
}

#[tokio::test]
async fn recompiling_an_unchanged_model_writes_nothing_and_keeps_policy_ids() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let before = policies_for(pool, f.service_id).await;
    let ids_before: Vec<i64> = before.iter().map(|(id, ..)| *id).collect();
    let bodies_before: Vec<Value> = before.iter().map(|(_, _, b)| b.clone()).collect();
    let v_before = policy_version(pool, f.service_id).await;

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile of an unchanged model");

    assert_eq!(summary.created, 0);
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.deleted, 0);
    assert_eq!(summary.unchanged, 1);
    assert!(!summary.wrote_anything());

    let after = policies_for(pool, f.service_id).await;
    let ids_after: Vec<i64> = after.iter().map(|(id, ..)| *id).collect();
    let bodies_after: Vec<Value> = after.iter().map(|(_, _, b)| b.clone()).collect();
    assert_eq!(
        ids_after, ids_before,
        "a no-op recompile must not churn policy ids — an engine caches them"
    );
    assert_eq!(
        bodies_after, bodies_before,
        "a no-op recompile must not rewrite an unchanged body either"
    );
    assert_eq!(
        policy_version(pool, f.service_id).await,
        v_before,
        "and must not bump the version, or every plugin re-downloads for nothing"
    );
}

/// SP4a Task 6 gave every workspace-assigned catalog an implicit grant whose
/// ONLY principal is `WorkspaceMembers`, which `compile_grant` refuses
/// (`CompileError::UnexpandedPrincipalKind`) rather than silently dropping.
/// This driver's decision (see `trigger`'s module doc): skip that ONE grant,
/// record why, and still recompile everything else for the service — proven
/// here by asserting BOTH that the tag rule still compiles (the StarRocks
/// masking policy from the fixture survives) AND that the skip is reported,
/// not silently swallowed.
#[tokio::test]
async fn an_unexpanded_workspace_members_grant_is_skipped_not_fatal() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("recompile-skip-ws"))
            .bind("Recompile Skip WS")
            .fetch_one(&db.pool)
            .await
            .expect("workspace");

    let implicit_grant = Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: f.catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::WorkspaceMembers,
                r#ref: ws_id.to_string(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::WorkspaceAssignment {
            workspace_id: ws_id,
        },
    };
    let grant_id = f
        .perm
        .create_grant(&implicit_grant)
        .await
        .expect("create implicit grant");

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile must not fail over a known, named limitation");

    assert_eq!(
        summary.created, 0,
        "the grant must compile to NO policy (skipped), and the rule was already compiled (unchanged)"
    );
    assert_eq!(
        summary.unchanged, 1,
        "the existing StarRocks masking policy is untouched"
    );
    assert_eq!(
        summary.skipped.len(),
        1,
        "exactly the one WorkspaceMembers grant must be recorded as skipped"
    );
    let skip = &summary.skipped[0];
    assert_eq!(skip.id, grant_id);
    assert_eq!(skip.source_kind, SourceKind::Grant);
    assert!(
        skip.reason.contains("WorkspaceMembers"),
        "the skip reason must name what was not expanded, not just that something was skipped: {:?}",
        skip.reason
    );

    // The rule this recompile HAS to fail over the grant to break is the
    // regression this test exists to prevent — assert the id from the
    // fixture's own setup is still the one and only rule referenced.
    let rows: Vec<(Option<Uuid>,)> =
        sqlx::query_as("SELECT rule_id FROM perm.compiled WHERE service_id = $1")
            .bind(f.service_id)
            .fetch_all(&db.pool)
            .await
            .expect("compiled rows");
    assert_eq!(rows, vec![(Some(f.rule_id),)]);
}
