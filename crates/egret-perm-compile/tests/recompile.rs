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

/// Bulk-tags `count` synthetic columns (`col_1`..`col_count`) under one
/// `schema`/`table`, all carrying `tag_key` — the same shape as the existing
/// per-rule ceiling test's bulk insert, generalised to a caller-chosen table
/// so several rules (SP4a Task 12's profile-total tests) can each get their
/// own tagged columns without colliding names.
async fn bulk_tag_columns(
    pool: &sqlx::PgPool,
    warehouse_id: Uuid,
    tag_key: &str,
    table: &str,
    count: i64,
) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         SELECT t.id, $1, 'sales', $2, 'col_' || gs::text \
         FROM tags t, generate_series(1, $3) gs WHERE t.key = $4",
    )
    .bind(warehouse_id)
    .bind(table)
    .bind(count)
    .bind(tag_key)
    .execute(pool)
    .await
    .expect("bulk-tag columns");
}

/// Creates a fresh tag key and a mask rule selecting it — a second (third,
/// fourth, ...) rule alongside the fixture's own, for SP4a Task 12's
/// profile-total tests, which need several rules each individually under
/// `DEFAULT_EXPANSION_CEILING` summing past `PROFILE_EXPANSION_CEILING`.
/// Returns `(rule_id, tag_key)` — the id so the rule can be deleted later,
/// the key so [`bulk_tag_columns`] can tag columns under it.
async fn create_tag_and_rule(perm: &PermStore, pool: &sqlx::PgPool, label: &str) -> (Uuid, String) {
    let tag_key = unique(label);
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $2)")
        .bind(&tag_key)
        .bind(label)
        .execute(pool)
        .await
        .expect("tag");
    let rule = Rule {
        id: Uuid::nil(),
        name: unique(&format!("{label}-hash")),
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
    (rule_id, tag_key)
}

/// Inserts `count` compiler-owned-LOOKING (correct name prefix) policy rows
/// directly, bypassing the compiler entirely — simulating a profile that is
/// already over `PROFILE_EXPANSION_CEILING` on disk, which is reachable only
/// because this ceiling did not exist before SP4a Task 12 (a profile that
/// compiled fine under the OLD per-rule-only guard can already be sitting
/// above the new profile total). These rows are deliberately NOT produced by
/// any real grant/rule, so a real recompile's diff sees them as no-longer-
/// desired and reaps every one — that reap IS the "shrink" this test proves
/// is never refused.
async fn seed_dummy_compiler_policies(pool: &sqlx::PgPool, service_id: i64, count: i64) {
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
         SELECT $1, $2 || gs::text, '{}'::jsonb FROM generate_series(1, $3) gs",
    )
    .bind(service_id)
    .bind(egret_policy::COMPILER_RULE_PREFIX)
    .bind(count)
    .execute(pool)
    .await
    .expect("seed dummy compiler-owned policies");
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

/// SP4a **D3**: an implicit workspace grant whose workspace has NO members
/// expands to no engine principal, and compiles to NO policy — recorded as a
/// skip, never as a Ranger item naming nobody.
///
/// This is the same assertion the pre-D3 version of this test made (the grant
/// contributes nothing and the rest of the service still recompiles); what
/// changed is the REASON. Before D3 the compiler refused the principal kind
/// outright; now it expands it, and this workspace's expansion is empty. The
/// member-bearing half — a workspace WITH members compiling to a policy that
/// names each one — is `tests/workspace_members.rs`.
#[tokio::test]
async fn an_implicit_grant_for_an_empty_workspace_compiles_to_nothing() {
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
        skip.reason.contains("no engine principal"),
        "the skip reason must say WHY nothing was emitted — that the grant names nobody the \
         engine could match — not merely that something was skipped: {:?}",
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

/// Fix round 1 CRITICAL: `perm.compiled.grant_id`/`rule_id` are `ON DELETE
/// CASCADE` (`migrations/0154_perm_model.sql:121-122`), so deleting a grant
/// erases its PROVENANCE row while the `policy.authz_policies` row it
/// produced survives untouched. Before the fix, `apply_diff`'s "current
/// state" read joined THROUGH `perm.compiled`, so a deleted grant's policy
/// was invisible to that join the instant the grant was deleted — it never
/// reached the delete sweep, and no later recompile could ever reap it. This
/// is the branch that would have caught it: create an explicit grant,
/// recompile (it compiles to a real access policy), delete the grant,
/// recompile again, and assert the policy is GONE from `policy.authz_policies`
/// — not merely that the count is right, the exact row by id.
#[tokio::test]
async fn deleting_a_grant_reaps_its_policy_on_the_next_recompile() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let grant = Grant {
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
                kind: PrincipalKind::User,
                r#ref: "alice".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let grant_id = f.perm.create_grant(&grant).await.expect("create grant");

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with the new explicit grant");
    assert_eq!(
        summary.created, 1,
        "the explicit grant must compile to exactly one new access policy"
    );

    let grant_policy_name = format!("perm-grant-{grant_id}");
    let grant_policy_id: i64 = sqlx::query_scalar(
        "SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(&grant_policy_name)
    .fetch_one(pool)
    .await
    .expect("the grant's policy must exist by its deterministic name");

    // Before the fix, this delete cascades the `perm.compiled` provenance
    // row away, leaving the `policy.authz_policies` row behind, untouched,
    // and — because the old sweep read through `perm.compiled` — permanently
    // invisible to every future recompile.
    let deleted = f.perm.delete_grant(grant_id).await.expect("delete grant");
    assert!(deleted);

    let summary2 = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile after the grant is deleted");
    assert_eq!(
        summary2.deleted, 1,
        "the deleted grant's policy must be reaped by the next recompile"
    );
    assert_eq!(summary2.created, 0);
    assert_eq!(summary2.updated, 0);

    let still_there: Option<i64> =
        sqlx::query_scalar("SELECT id FROM policy.authz_policies WHERE id = $1")
            .bind(grant_policy_id)
            .fetch_optional(pool)
            .await
            .expect("query");
    assert!(
        still_there.is_none(),
        "the deleted grant's policy must be GONE from policy.authz_policies — a revocation \
         that leaves the engine enforcing it is the worst shape in a permission system"
    );

    // The unrelated, still-live rule-based policy from the fixture must be
    // untouched by this — proves the sweep reaps the RIGHT policy, not just
    // any policy.
    let remaining = policies_for(pool, f.service_id).await;
    assert_eq!(
        remaining.len(),
        1,
        "only the pre-existing rule-based policy should remain"
    );
    assert_ne!(remaining[0].0, grant_policy_id);
}

/// Fix round 1 Important: the `updated` branch — the one `perm.compiled`
/// exists for — had no test at all. The only thing that can change a
/// TRINO tag-selector rule's compiled body without changing the rule's own
/// id is the underlying tag's `name` (D6: `tags.name` is mutable, unlike
/// `tags.key`, which the rule selects by) — a Trino tag policy embeds the
/// resolved NAME at `resources.tag.values[0]` (`tagify`,
/// `egret-policy/src/admin/ranger_json.rs`). Renaming the tag must recompile
/// the SAME policy id with a DIFFERENT body (the old name gone, the new name
/// present), not create a new policy or leave the old name behind.
///
/// **Not scoped by `CompileSummary`'s whole-service counts**, deliberately:
/// a Trino tag-selector rule has NO catalog/warehouse scope in the model at
/// all (D5 — Trino's plugin resolves the tag itself, so the rule applies
/// wherever the tag turns up), so EVERY Trino service in this shared
/// test-process database compiles EVERY tag rule any concurrently-running
/// test has created, including this file's other fixtures. `created`/
/// `updated` on the whole-service `CompileSummary` are therefore not this
/// test's own count to make — only `policy.authz_policies`'s OWN `id` and
/// `version` columns for the ONE row this test created are (the same
/// "scope every count assertion to the fixture it owns" rule
/// `crates/egret-perm/tests/assignment.rs`'s `grants_for` documents).
#[tokio::test]
async fn renaming_a_tag_updates_the_trino_policy_in_place() {
    let Some(db) = test_db().await else { return };
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());

    egret_policy::seed_trino(&policy)
        .await
        .expect("seed trino service-def");
    egret_policy::seed_tag(&policy)
        .await
        .expect("seed tag service-def");
    // SP4a Task 11, defect 2: a Trino tag policy is stored under the TAG
    // service, so this fixture needs a real one. Round 1's version named a
    // `tagService` that had no `policy.authz_services` row at all and still
    // found the policy under the trino profile — which is precisely the bug
    // Task 10 measured live (the tag plugin's own download served 0 policies).
    let (service, tag_service_id) = trino_profile_with_tag_service(&policy, "trino_update").await;

    let tag_key = unique("rename-me");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, 'Original Name')")
        .bind(&tag_key)
        .execute(pool)
        .await
        .expect("insert tag");

    let rule_id = perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("rename-rule"),
            kind: RuleKind::Mask {
                mask_type: "MASK_HASH".to_string(),
                value_expr: None,
            },
            selector: Selector::Tag {
                key: tag_key.clone(),
            },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("create rule");

    recompile_service(&perm, &policy, service.id)
        .await
        .expect("initial recompile");

    let policy_name = format!("perm-rule-{rule_id}");
    let (policy_id, row_version_before, body_before): (i64, i64, Value) = sqlx::query_as(
        "SELECT id, version, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(tag_service_id)
    .bind(&policy_name)
    .fetch_one(pool)
    .await
    .expect("the rule's policy must exist");
    assert_eq!(
        body_before["resources"]["tag"]["values"][0],
        "Original Name"
    );

    let svc_version_before: i64 =
        sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
            .bind(service.id)
            .fetch_one(pool)
            .await
            .expect("policy_version");

    sqlx::query("UPDATE tags SET name = 'Renamed PII' WHERE key = $1")
        .bind(&tag_key)
        .execute(pool)
        .await
        .expect("rename tag");

    recompile_service(&perm, &policy, service.id)
        .await
        .expect("recompile after tag rename");

    let (policy_id_after, row_version_after, body_after): (i64, i64, Value) = sqlx::query_as(
        "SELECT id, version, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(tag_service_id)
    .bind(&policy_name)
    .fetch_one(pool)
    .await
    .expect("the rule's policy must still exist under the same name");
    assert_eq!(policy_id_after, policy_id, "the policy id must not churn");
    assert_eq!(
        row_version_after,
        row_version_before + 1,
        "an UPDATE increments this row's own version; a delete-then-reinsert would have reset \
         it to 1 instead"
    );
    assert_eq!(
        body_after["resources"]["tag"]["values"][0], "Renamed PII",
        "the new name must be in the compiled body"
    );
    assert_ne!(
        body_after["resources"]["tag"]["values"][0], body_before["resources"]["tag"]["values"][0],
        "the old name must be GONE, not merely joined by the new one"
    );

    // The SERVICE-level `policy_version` bump is safe to assert exactly
    // (unlike the row-scoped counts above) because it increments ONCE PER
    // RECOMPILE CALL THAT WROTE ANYTHING, not once per row — a second
    // recompile call that also happens to touch a contaminating foreign
    // rule's policy still bumps it by exactly 1, the same as if it had
    // touched only this test's own row.
    let svc_version_after: i64 =
        sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
            .bind(service.id)
            .fetch_one(pool)
            .await
            .expect("policy_version");
    assert_eq!(
        svc_version_after,
        svc_version_before + 1,
        "a real update must still bump the version exactly once"
    );
}

/// Fix round 1 Minor: `DEFAULT_EXPANSION_CEILING` is honestly labelled
/// unmeasured, but nothing had ever driven the refusal through
/// `recompile_service` end to end — only `compile_rule` directly
/// (`egret-perm-compile/tests/compile_rules.rs`). Tags 500 MORE columns onto
/// the fixture's existing tag key (501 total, one over the 500 ceiling) and
/// asserts the recompile refuses, names the rule and the count, and writes
/// NOTHING — a partial compile that silently masked 500 of 501 columns would
/// look identical to success from every screen egret has.
#[tokio::test]
async fn the_expansion_ceiling_refuses_through_recompile_service_not_only_compile_rule() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let before = policies_for(pool, f.service_id).await;
    let v_before: i64 =
        sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
            .bind(f.service_id)
            .fetch_one(pool)
            .await
            .expect("policy_version");

    let tag_key: String = sqlx::query_scalar(
        "SELECT t.key FROM tags t JOIN catalog_tag_assignments a ON a.tag_id = t.id \
         WHERE a.warehouse_id = $1 LIMIT 1",
    )
    .bind(f.warehouse_id)
    .fetch_one(pool)
    .await
    .expect("tag key");
    let rule_name: String = sqlx::query_scalar("SELECT name FROM perm.rules WHERE id = $1")
        .bind(f.rule_id)
        .fetch_one(pool)
        .await
        .expect("rule name");

    // 500 more columns on top of the fixture's one pre-tagged "email" = 501,
    // one over `DEFAULT_EXPANSION_CEILING` (500).
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         SELECT t.id, $1, 'sales', 'orders', 'col_' || gs::text \
         FROM tags t, generate_series(1, 500) gs WHERE t.key = $2",
    )
    .bind(f.warehouse_id)
    .bind(&tag_key)
    .execute(pool)
    .await
    .expect("bulk-tag 500 more columns");

    let err = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err("501 tagged columns must refuse, not truncate to 500");
    match err {
        egret_perm::CompileError::ExpansionCeiling {
            rule,
            count,
            ceiling,
        } => {
            assert_eq!(rule, rule_name);
            assert_eq!(count, 501);
            assert_eq!(ceiling, 500);
        }
        other => panic!("expected ExpansionCeiling, got {other:?}"),
    }

    // The refusal must write NOTHING — no partial 500-of-501 compile.
    assert_eq!(
        policies_for(pool, f.service_id).await,
        before,
        "a refused recompile must leave every existing policy exactly as it was"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT policy_version FROM policy.authz_services WHERE id = $1"
        )
        .bind(f.service_id)
        .fetch_one(pool)
        .await
        .expect("policy_version"),
        v_before,
        "a refused recompile must not bump the version either"
    );
}

/// SP4a Task 12: the per-rule ceiling alone does not bound the profile
/// TOTAL. Four rules each individually AT or under `DEFAULT_EXPANSION_CEILING`
/// (300 < 500) still sum past `PROFILE_EXPANSION_CEILING` (1000) — the live
/// gate's own finding, reproduced here at 300/rule so the assertion lands on
/// the PROFILE ceiling specifically and not incidentally on the per-rule one
/// too. Rule 1 is the fixture's own (grown from 1 to 300 tagged columns);
/// three more rules bring the total to 1200.
#[tokio::test]
async fn the_profile_total_ceiling_refuses_and_names_the_profile_and_the_total() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let service_name: String =
        sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
            .bind(f.service_id)
            .fetch_one(pool)
            .await
            .expect("service name");

    let before = policies_for(pool, f.service_id).await;
    let v_before = policy_version(pool, f.service_id).await;

    let tag_key: String = sqlx::query_scalar(
        "SELECT t.key FROM tags t JOIN catalog_tag_assignments a ON a.tag_id = t.id \
         WHERE a.warehouse_id = $1 LIMIT 1",
    )
    .bind(f.warehouse_id)
    .fetch_one(pool)
    .await
    .expect("tag key");

    // Rule 1 (the fixture's own): 1 -> 300 tagged columns, still well under
    // `DEFAULT_EXPANSION_CEILING` (500).
    bulk_tag_columns(pool, f.warehouse_id, &tag_key, "orders", 299).await;

    // Three more rules, 300 tagged columns each — every rule individually
    // under its own per-rule cap.
    for i in 0..3 {
        let label = unique(&format!("bulk{i}"));
        let (_rule_id, rule_tag) = create_tag_and_rule(&f.perm, pool, &label).await;
        bulk_tag_columns(pool, f.warehouse_id, &rule_tag, &label, 300).await;
    }

    let err = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err(
            "4 rules x 300 columns = 1200, over the profile-total ceiling of 1000, must refuse \
             even though every rule sits under its own per-rule cap",
        );
    match err {
        egret_perm::CompileError::ProfileExpansionCeiling {
            profile,
            total,
            ceiling,
        } => {
            assert_eq!(profile, service_name, "the refusal must name the PROFILE");
            assert_eq!(total, 1200, "the refusal must name the running TOTAL");
            assert_eq!(ceiling, 1000);
        }
        other => panic!("expected ProfileExpansionCeiling, got {other:?}"),
    }

    // The refusal must write NOTHING — same invariant as the per-rule
    // ceiling's own refusal.
    assert_eq!(
        policies_for(pool, f.service_id).await,
        before,
        "a refused profile-total recompile must leave every existing policy exactly as it was"
    );
    assert_eq!(
        policy_version(pool, f.service_id).await,
        v_before,
        "a refused recompile must not bump the version either"
    );
}

/// SP4a Task 12, the subtlety this ceiling exists to get right: a profile
/// that is ALREADY over `PROFILE_EXPANSION_CEILING` must still be able to
/// recompile when the change SHRINKS the total (or leaves it unchanged) —
/// even while the shrunk total is STILL over the ceiling. A naive
/// "total > ceiling ⇒ refuse" bricks exactly this profile: every recompile
/// that would accompany an operator's delete-your-way-back-under-the-cap
/// refuses, and the delete rolls back with it — the same shape as Task 9's
/// I-1 ("an uncompilable grant poisons every later recompile"), one level up.
#[tokio::test]
async fn a_profile_already_over_the_cap_can_still_shrink_by_deleting_a_rule() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let tag_key: String = sqlx::query_scalar(
        "SELECT t.key FROM tags t JOIN catalog_tag_assignments a ON a.tag_id = t.id \
         WHERE a.warehouse_id = $1 LIMIT 1",
    )
    .bind(f.warehouse_id)
    .fetch_one(pool)
    .await
    .expect("tag key");

    // Rule 1 (the fixture's own) grows from 1 to 400 tagged columns.
    bulk_tag_columns(pool, f.warehouse_id, &tag_key, "orders", 399).await;

    // Three more rules, 400 tagged columns each — 1600 desired in total,
    // every rule individually well under `DEFAULT_EXPANSION_CEILING` (500).
    let mut extra_rule_ids = Vec::new();
    for i in 0..3 {
        let label = unique(&format!("bulk{i}"));
        let (rule_id, rule_tag) = create_tag_and_rule(&f.perm, pool, &label).await;
        bulk_tag_columns(pool, f.warehouse_id, &rule_tag, &label, 400).await;
        extra_rule_ids.push(rule_id);
    }

    // Simulate a profile that is ALREADY over the cap from before this
    // ceiling existed: seed dummy compiler-owned rows so the STORED total (1
    // real "email" policy + these dummies) comfortably exceeds the 1600 this
    // recompile is about to desire.
    seed_dummy_compiler_policies(pool, f.service_id, 1649).await;

    let stored_before_first: i64 =
        sqlx::query_scalar("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
            .bind(f.service_id)
            .fetch_one(pool)
            .await
            .expect("count");
    assert_eq!(
        stored_before_first, 1650,
        "sanity: 1 real + 1649 dummy, already over PROFILE_EXPANSION_CEILING (1000)"
    );

    // First recompile: desired 1600 is OVER the ceiling, but the STORED
    // total (1650) is even higher — the total is shrinking, so this must be
    // ALLOWED despite 1600 > 1000.
    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect(
            "a recompile whose new total (1600) is <= the currently stored total (1650) must \
             never be refused, even though both are over the profile-total ceiling",
        );
    assert_eq!(summary.deleted, 1649, "every dummy row must be reaped");

    let after_first = policies_for(pool, f.service_id).await;
    assert_eq!(
        after_first.len(),
        1600,
        "1 unchanged (email) + 1599 newly created = 1600 real compiler-owned policies"
    );

    // Now delete ONE of the three extra rules: desired drops from 1600 to
    // 1200 — STILL over the profile-total ceiling (1000), but less than the
    // 1600 now actually stored. A naive "total > ceiling ⇒ refuse" gets this
    // wrong; the correct answer is ALLOWED, because the total shrank.
    let deleted_rule = extra_rule_ids[0];
    f.perm
        .delete_rule(deleted_rule)
        .await
        .expect("delete one of the three extra rules");

    let summary2 = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect(
            "deleting a rule from an over-cap profile must be allowed to recompile — the total \
             shrank from 1600 to 1200, even though 1200 is still over the ceiling",
        );
    assert_eq!(
        summary2.deleted, 400,
        "the deleted rule's 400 policies must be reaped"
    );

    let after_second = policies_for(pool, f.service_id).await;
    assert_eq!(
        after_second.len(),
        1200,
        "1200 real compiler-owned policies remain after the rule delete"
    );
}

/// Fix round 1 Minor: `compile_grant`'s output had no value-level test for
/// the StarRocks `serviceType` stamp (`grants.rs`'s `stamp_service_type`
/// reuse, fixed alongside the recompile driver itself) — only the RULE path
/// had one (`tagging_a_new_column_recompiles_the_starrocks_profile`). An
/// explicit grant on the same StarRocks fixture must render with
/// `serviceType: "starrocks"`, not the renderer's hardcoded `"trino"`
/// default.
#[tokio::test]
async fn an_explicit_grant_on_starrocks_gets_the_starrocks_service_type_stamp() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let pool = &db.pool;

    let grant = Grant {
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
                kind: PrincipalKind::User,
                r#ref: "bob".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let grant_id = f.perm.create_grant(&grant).await.expect("create grant");

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with the explicit grant");

    let body: Value = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(format!("perm-grant-{grant_id}"))
    .fetch_one(pool)
    .await
    .expect("the grant's policy must exist");
    assert_eq!(
        body["serviceType"], "starrocks",
        "an access policy compiled for a StarRocks profile must not carry the renderer's \
         hardcoded Trino serviceType"
    );
    assert_eq!(body["policyType"], 0, "an access policy, not a masking one");
}

/// A StarRocks profile with one engine bound to it and one registry catalog
/// attached — no grant or rule yet, and NO recompile during setup (unlike
/// `seed_starrocks_engine_with_tag_rule`, which bakes one in). That gap is
/// the point: it lets a test observe `policy.authz_services.origin` both
/// BEFORE and AFTER the service's first real compile (SP4a Task 8, D4).
struct EmptyFixture {
    perm: PermStore,
    policy: PolicyStore,
    service_id: i64,
    catalog_id: Uuid,
}

async fn seed_starrocks_engine_no_grants_yet(db: &Db) -> EmptyFixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");

    let service = policy
        .create_service(&unique("sr_origin"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("origin-ws"))
            .bind("Origin WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-origin-test','AKIATEST') RETURNING id",
    )
    .bind(unique("origin-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("origin_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("origin-eng"))
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
    .bind(unique("origin_cat"))
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

    EmptyFixture {
        perm,
        policy,
        service_id: service.id,
        catalog_id,
    }
}

async fn origin_of(pool: &sqlx::PgPool, service_id: i64) -> String {
    sqlx::query_scalar("SELECT origin FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("origin")
}

/// SP4a Task 8 (D4): `origin` starts `'manual'` (migration 0155's default,
/// via `PolicyStore::create_service`) and is stamped `'generated'` by the
/// compiler's OWN write path (`egret-perm-compile::trigger::apply_diff_in_tx`)
/// the moment it first produces a policy for the service — NOT by a
/// recompile that has nothing to compile, and NOT by a hand-written `UPDATE`.
/// This is the proof the editor guard in `egret-policy`
/// (`crates/egret-policy/tests/generated_profile_guard.rs`) depends on:
/// without a real write path that sets `'generated'`, that guard would be
/// dead code, since nothing would ever produce the state it checks for.
#[tokio::test]
async fn a_real_recompile_stamps_the_profile_generated_only_once_it_compiles_something() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_no_grants_yet(&db).await;
    let pool = &db.pool;

    assert_eq!(
        origin_of(pool, f.service_id).await,
        "manual",
        "a freshly created profile must start manual (migration 0155's default)"
    );

    // A recompile with nothing to compile (no grant/rule reaches this
    // service yet) must NOT flip the profile to generated — a profile only
    // becomes machine-owned once the compiler actually claims a policy in
    // it, per this module's own doc comment.
    let empty_summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with nothing to compile");
    assert_eq!(empty_summary.created, 0);
    assert_eq!(
        origin_of(pool, f.service_id).await,
        "manual",
        "a no-op recompile (no grant/rule reaches this service) must leave origin untouched"
    );

    let grant = Grant {
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
                kind: PrincipalKind::User,
                r#ref: "alice".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    f.perm.create_grant(&grant).await.expect("create grant");

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with a grant now reaching this service");
    assert_eq!(
        summary.created, 1,
        "the new grant must compile to one policy"
    );

    assert_eq!(
        origin_of(pool, f.service_id).await,
        "generated",
        "the FIRST real compile that produces a policy for this service must stamp \
         origin = 'generated' — through recompile_service, not a hand-written UPDATE"
    );
}

/// SP4a Task 8 fix round 3: `egret-policy::store::create_policy`/`update_policy`
/// now refuse any human write whose NAME carries the compiler's own reserved
/// prefix (`COMPILER_GRANT_PREFIX`/`COMPILER_RULE_PREFIX`), in any profile.
/// The compiler itself must be completely unaffected — `apply_diff_in_tx`
/// writes `policy.authz_policies` with its own raw `sqlx::query` calls
/// directly against its own transaction, never through `PolicyStore`'s
/// guarded methods, so the new reservation cannot reach it. Proven here with
/// a REAL `recompile_service` call (not a hand INSERT) that still produces
/// (and can update) a `perm-grant-…`-named policy after the round-3 change.
#[tokio::test]
async fn the_compiler_itself_still_writes_reserved_prefix_policies_after_the_reservation() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_no_grants_yet(&db).await;
    let pool = &db.pool;

    let grant = Grant {
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
                kind: PrincipalKind::User,
                r#ref: "carol".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let grant_id = f.perm.create_grant(&grant).await.expect("create grant");

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("the compiler's own write path must not be blocked by the reservation it owns");
    assert_eq!(summary.created, 1, "the grant must compile to one policy");

    let expected_name = format!("perm-grant-{grant_id}");
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(&expected_name)
    .fetch_optional(pool)
    .await
    .expect("query");
    assert!(
        row.is_some(),
        "the compiler must still be able to write a policy named with its own reserved \
         prefix ({expected_name:?}); row={row:?}"
    );

    // And it can still be UPDATED by a second recompile (the grant's
    // principal changes, so the compiled body changes too) — proving the
    // reservation blocks only the human write paths, not the compiler's
    // create-or-update-in-place sweep.
    f.perm
        .delete_grant(grant_id)
        .await
        .expect("delete original grant");
    let grant2 = Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read, Action::Write],
        scope: Scope {
            catalog_id: f.catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::User,
                r#ref: "carol".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let grant2_id = f.perm.create_grant(&grant2).await.expect("create grant 2");
    let summary2 = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile after replacing the grant");
    assert_eq!(summary2.deleted, 1, "the old grant's policy must be reaped");
    assert_eq!(
        summary2.created, 1,
        "the new grant must compile to a new policy"
    );

    let expected_name2 = format!("perm-grant-{grant2_id}");
    let exists2: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM policy.authz_policies WHERE service_id = $1 AND name = $2)",
    )
    .bind(f.service_id)
    .bind(&expected_name2)
    .fetch_one(pool)
    .await
    .expect("query");
    assert!(
        exists2,
        "the compiler must still be able to write a SECOND reserved-prefix policy on the \
         next recompile ({expected_name2:?})"
    );
}

// ---------------------------------------------------------------------------
// SP4a Task 11, defect 2 — the fixture half.
// ---------------------------------------------------------------------------
//
// A Trino tag policy is stored under the profile's TAG service, not the
// profile, so any Trino fixture carrying a tag rule needs a REAL tag-service
// row. The routing and reaping assertions themselves live in their OWN test
// binary, `tests/tag_service_routing.rs` — see that file for why they cannot
// share this one's database.

/// A Trino profile whose `tagService` names a REAL tag service row, with both
/// service-defs seeded. Returns the profile row and the tag service's id.
async fn trino_profile_with_tag_service(
    policy: &PolicyStore,
    prefix: &str,
) -> (egret_policy::ServiceRow, i64) {
    let profile_name = unique(prefix);
    let tag_name = format!("{profile_name}_tag");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_name }),
        )
        .await
        .expect("create trino profile");
    let tag_service = policy
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    (profile, tag_service.id)
}

// ---------------------------------------------------------------------------
// SP4d debt Task 6, item 1 — the compiler must not EMIT an order-ambiguous
// mask/row-filter pair.
// ---------------------------------------------------------------------------
//
// SP4d-1's finding F2, reproduced from the MODEL rather than from a hand-built
// policy set: two `perm.rules` rows whose selectors land on the same column
// compile, independently, to two `policyType 1` policies at `policyPriority 0`
// selecting one resource. Which one the engine applies falls through to a
// tie-break the artifact does not express (see
// `egret_policy::ambiguous_arbitration`), so the whole recompile is refused.
//
// These run through `recompile_service`, the real driver every write surface
// funnels through — `POST`/`PATCH /perm/rules`, `POST /perm/preview`, and the
// tag/role/membership triggers — so the guard is exercised where it actually
// sits and not through a unit call that could not fail for route reasons.

/// Creates a tag and a MASK rule on it with a caller-chosen `mask_type`, so a
/// test can make two rules that AGREE or DIFFER on the decisive value.
/// Returns `(rule_id, tag_key)`.
async fn create_tag_and_mask_rule(
    perm: &PermStore,
    pool: &sqlx::PgPool,
    label: &str,
    mask_type: &str,
) -> (Uuid, String) {
    let tag_key = unique(label);
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $2)")
        .bind(&tag_key)
        .bind(label)
        .execute(pool)
        .await
        .expect("tag");
    let rule = Rule {
        id: Uuid::nil(),
        name: unique(&format!("{label}-rule")),
        kind: RuleKind::Mask {
            mask_type: mask_type.to_string(),
            value_expr: None,
        },
        selector: Selector::Tag {
            key: tag_key.clone(),
        },
        exempt: PrincipalSet::default(),
    };
    let rule_id = perm.create_rule(&rule).await.expect("create rule");
    (rule_id, tag_key)
}

#[tokio::test]
async fn two_rules_masking_one_column_differently_refuse_the_whole_recompile() {
    let Some(db) = test_db().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    // The fixture's rule masks `sales.orders.email` with MASK_HASH. A second
    // tag on the SAME column, carrying a rule with a DIFFERENT mask type, is
    // F2's differing half.
    let (second_rule, second_tag) =
        create_tag_and_mask_rule(&f.perm, &db.pool, "ambig", "MASK_NULL").await;
    tag_column(
        &db.pool,
        f.warehouse_id,
        "sales",
        "orders",
        "email",
        &second_tag,
    )
    .await;

    let err = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err("a recompile that would emit the ambiguous pair must refuse");
    let msg = err.to_string();
    assert!(
        matches!(
            err,
            egret_perm::CompileError::AmbiguousMaskOrFilterArbitration { .. }
        ),
        "the refusal must be the dedicated variant, not a generic Db error: {msg}"
    );
    // The message has to point at something the steward can edit: both policy
    // names (each carrying its rule's uuid) and the resource that collided.
    assert!(
        msg.contains(&f.rule_id.to_string()) && msg.contains(&second_rule.to_string()),
        "the refusal must name BOTH rules' compiled policies: {msg}"
    );
    assert!(
        msg.contains("column=[email]"),
        "the refusal must name the resource that collided: {msg}"
    );
    assert!(
        msg.contains("mask"),
        "the refusal must say which decision is ambiguous: {msg}"
    );

    // ⚠ The refusal is raised in `compile_desired`, BEFORE `apply_diff_in_tx`,
    // so nothing is written: the profile's previously-compiled policy set is
    // untouched and the installation keeps enforcing what it was enforcing.
    // Asserted on the stored row rather than on the summary (there is no
    // summary — the call returned an error).
    let still_there: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 \
         AND (policy_jsonb->>'policyType') = '1'",
    )
    .bind(f.service_id)
    .fetch_one(&db.pool)
    .await
    .expect("query");
    assert_eq!(
        still_there, 1,
        "a refused recompile must write nothing — the fixture's one masking policy stands"
    );
}

/// ⚠ **An ACCEPT-control, and it is the assertion that stops this guard being
/// widened into something that breaks working installations.** SP4d-1 §8's
/// other half is two masks on one column that AGREE (policies 8777 and 8801,
/// both `dataMaskType: "MASK"`, no `valueExpr`) — a real artifact, on a real
/// profile. Whichever evaluator runs first, the column is masked the same way,
/// so nothing about enforcement is undetermined and the compile must succeed.
///
/// `egret-ranger-eval` still declines to PREDICT that pair, because a
/// `MaskResult` names the policy that produced it; that is an explainability
/// limit, not an enforcement one, and turning it into a compile-time refusal
/// here would have broken the one measured artifact for no enforcement gain.
#[tokio::test]
async fn two_rules_masking_one_column_the_same_way_still_compile() {
    let Some(db) = test_db().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let (_second_rule, second_tag) =
        create_tag_and_mask_rule(&f.perm, &db.pool, "agree", "MASK_HASH").await;
    tag_column(
        &db.pool,
        f.warehouse_id,
        "sales",
        "orders",
        "email",
        &second_tag,
    )
    .await;

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("an AGREEING tie must still compile");
    assert_eq!(
        summary.created, 1,
        "the second rule's own policy is created; the first one is unchanged"
    );
    // Both policies really are present, both really are masks on the same
    // column, and they really do agree — asserted on the stored bodies, so the
    // test cannot pass by the second rule having compiled to nothing (the
    // failure mode a bare `expect` would hide).
    let masks: Vec<Value> = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 \
         AND (policy_jsonb->>'policyType') = '1' ORDER BY name",
    )
    .bind(f.service_id)
    .fetch_all(&db.pool)
    .await
    .expect("query");
    assert_eq!(masks.len(), 2, "two masking policies, not one");
    for m in &masks {
        assert_eq!(
            m["resources"]["column"]["values"][0], "email",
            "both must select the same column, or this is not the tie under test"
        );
        assert_eq!(
            m["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"], "MASK_HASH",
            "both must carry the SAME mask type, or this is not an agreeing tie"
        );
    }
    assert_eq!(
        masks[0]["dataMaskPolicyItems"], masks[1]["dataMaskPolicyItems"],
        "the decisive buckets must be equal — that is what makes the tie harmless"
    );
}
