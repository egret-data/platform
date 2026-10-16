//! SP4c **Task 2d**: a classification's CLEARANCE on StarRocks, which has no tag
//! service — so it expands to one ACCESS deny **per tagged column**, reusing the
//! same expansion SP4a's tag masks already use on this engine.
//!
//! Every constraint asserted here was MEASURED in D0b
//! (`docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md`), not chosen:
//!
//! - **exactly N denies for N tagged columns, no dedup** — a row filter dedups
//!   per table because it carries no column resource; an access deny does, so
//!   collapsing two columns would un-deny one of them;
//! - **`select` alone** — `checkColumnAction` has one caller and it passes
//!   `PrivilegeType.SELECT`, so the `insert`/`update`/`delete` a Trino clearance
//!   withholds can never match at column granularity. The test asserts the
//!   write accesses are ABSENT, which is the assertion that documents the
//!   asymmetry: a user without clearance can still overwrite a StarRocks column
//!   they cannot read;
//! - **priority 0 / not override** — with no tag-first short-circuit,
//!   deny-beats-allow comes from Ranger sorting `hasDeny()` evaluators first,
//!   which holds only at EQUAL priority;
//! - **refuse past the ceiling, never truncate** — a truncated clearance
//!   withholds some of the columns it claims to.
//!
//! Plus the invisible failure SP4c Task 2 built its own test around, re-asserted
//! on this path because the code that reads the username is shared but the code
//! that BUILDS the body is not: the exceptions must carry the **stored**
//! `users.username`, collision suffix included. A holder whose username equals
//! their email slug passes whether the fix exists or not, so the holder here is
//! the `-2`.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test clearance_starrocks
//!
//! **Scoping.** A clearance is compiled for every gated tag in the database, so
//! a concurrently running test's gated tag also compiles into this test's
//! profile. Every assertion is therefore keyed to the `(service_id, name)` pairs
//! this test's own tag produces, never to a count of everything in the table.

use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{recompile_service, DEFAULT_EXPANSION_CEILING};
use egret_policy::{PolicyStore, COMPILER_CLEARANCE_PREFIX};
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
    engine_id: Uuid,
    catalog_name: String,
}

/// A StarRocks profile with one engine bound to it and one registry catalog
/// attached to that engine — the chain `tagged_columns_for_service_via` walks to
/// decide which tagged columns this profile's plugin can be served a policy for.
/// No `perm.rules` row and no grant: a clearance comes from
/// `classification_policies`, not from the perm model, and a fixture carrying a
/// mask rule would let a mask's policy count be mistaken for a clearance's.
async fn seed_starrocks(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");

    let service = policy
        .create_service(&unique("sr_clr"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("clr-ws"))
            .bind("Clearance WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-clr-test','AKIATEST') RETURNING id",
    )
    .bind(unique("clr-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("clr_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret',$3,$2) RETURNING id",
    )
    .bind(unique("clr-eng"))
    .bind(service.id)
    // Pinned to egret's default image: the clearance MASK (view governance A2)
    // is folded through the evaluator at the bound engine's plugin version,
    // and an engine whose image says nothing about its plugin refuses.
    .bind(serde_json::json!({
        "starrocksConfig": {"feImage": egret_perm::DEFAULT_ENGINE_IMAGES.starrocks_fe_image}
    }))
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("clr_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog_name)
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
        perm,
        policy,
        service_id: service.id,
        warehouse_id,
        catalog_id,
        engine_id,
        catalog_name,
    }
}

/// A tag, returning `(id, key)`.
async fn new_tag(db: &Db) -> (Uuid, String) {
    let key = unique("clr-sr-tag");
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(&key)
        .bind(unique("SrClearanceTag"))
        .fetch_one(&db.pool)
        .await
        .expect("insert tag");
    (id, key)
}

async fn tag_column(db: &Db, tag_id: Uuid, warehouse_id: Uuid, table: &str, column: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales',$3,$4)",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .bind(table)
    .bind(column)
    .execute(&db.pool)
    .await
    .expect("tag column");
}

/// Tag `count` synthetic columns of one table, for the ceiling case.
async fn bulk_tag_columns(db: &Db, tag_id: Uuid, warehouse_id: Uuid, table: &str, count: i64) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) SELECT $1, $2, 'sales', $3, 'col_' || gs::text \
         FROM generate_series(1, $4) gs",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .bind(table)
    .bind(count)
    .execute(&db.pool)
    .await
    .expect("bulk-tag columns");
}

async fn new_role(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert role")
}

async fn new_user(db: &Db, email: &str, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,$2,$3) RETURNING id",
    )
    .bind(email)
    .bind("Test User")
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn grant_role(db: &Db, user_id: Uuid, role_id: Uuid) {
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(user_id)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .expect("role grant");
    assert_eq!(res.rows_affected(), 1, "the role grant row must exist");
}

async fn gate(db: &Db, tag_id: Uuid, roles: &[&str]) {
    let roles: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, ARRAY[]::uuid[]) \
         ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles",
    )
    .bind(tag_id)
    .bind(&roles)
    .execute(&db.pool)
    .await
    .expect("classification policy");
}

/// Every clearance policy for `tag_id` under `service_id`, as
/// `(name, policy_jsonb)` ordered by name. Scoped to this test's own tag by the
/// name prefix the compiler builds: `<prefix><tag_id>-<catalog>.<schema>.…`.
///
/// The DENIES only (`policyType 0`): since view governance A2 each tagged column
/// also gets a clearance MASK under the same prefix — see [`clearance_masks`].
async fn clearance_policies(db: &Db, service_id: i64, tag_id: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 AND name LIKE $2 || '%' \
           AND (policy_jsonb->>'policyType')::int = 0 ORDER BY name",
    )
    .bind(service_id)
    .bind(format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}-"))
    .fetch_all(&db.pool)
    .await
    .expect("policy read")
}

fn strings_of(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_str().expect("string").to_string())
        .collect()
}

fn users_of(item: &Value) -> Vec<String> {
    strings_of(&item["users"])
}

fn access_types(item: &Value) -> Vec<String> {
    item["accesses"]
        .as_array()
        .expect("accesses")
        .iter()
        .map(|a| a["type"].as_str().expect("type").to_string())
        .collect()
}

/// **The headline of Task 2d.** A clearance on a tag covering THREE columns
/// compiles to exactly THREE per-column denies on StarRocks, each addressing its
/// own column, each withholding `select` and ONLY `select`, each at priority 0 —
/// and each excepting the cleared holder by the username actually stored, which
/// here carries a collision suffix.
#[tokio::test]
async fn a_clearance_expands_to_exactly_one_deny_per_tagged_column() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;

    // THREE columns, two of them in the SAME table — the case a row filter
    // dedups and an access deny must not.
    tag_column(&db, tag_id, f.warehouse_id, "orders", "email").await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    tag_column(&db, tag_id, f.warehouse_id, "people", "dob").await;

    // The cleared holder is the `-2`: a re-derived slug would name the OTHER
    // person, clearing the wrong human while compiling and deploying cleanly.
    let local = unique("srdupe");
    let _base = new_user(&db, &format!("{local}@egret.local"), &local).await;
    let suffixed = new_user(&db, &format!("{local}@other.local"), &format!("{local}-2")).await;
    let role_name = unique("sr-clearance");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, suffixed, role_id).await;

    gate(&db, tag_id, &[&role_name]).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let policies = clearance_policies(&db, f.service_id, tag_id).await;
    assert_eq!(
        policies.len(),
        3,
        "three tagged columns must produce exactly THREE denies — no dedup, because an access \
         deny carries a column resource; got {:?}",
        policies.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );

    // The set of columns addressed, read out of the bodies themselves.
    let mut addressed: Vec<String> = policies
        .iter()
        .map(|(_, body)| {
            let r = &body["resources"];
            format!(
                "{}.{}.{}.{}",
                strings_of(&r["catalog"]["values"]).join("+"),
                strings_of(&r["database"]["values"]).join("+"),
                strings_of(&r["table"]["values"]).join("+"),
                strings_of(&r["column"]["values"]).join("+"),
            )
        })
        .collect();
    addressed.sort();
    assert_eq!(
        addressed,
        vec![
            format!("{}.sales.orders.email", f.catalog_name),
            format!("{}.sales.orders.ssn", f.catalog_name),
            format!("{}.sales.people.dob", f.catalog_name),
        ],
        "each deny must address its OWN column, by the catalog/schema/table/column NAMES the \
         engine matches on"
    );

    for (name, body) in &policies {
        assert_eq!(
            body["serviceType"], "starrocks",
            "a StarRocks-targeted body rendered with the renderer's hardcoded \"trino\" deploys \
             cleanly and enforces nothing: {name}"
        );
        assert_eq!(body["policyType"], 0, "an ACCESS policy: {name}");
        assert_eq!(
            body["policyPriority"], 0,
            "priority 0, NEVER override: with no tag service, deny-beats-allow comes from Ranger \
             sorting deny-bearing evaluators first, and priority DOMINATES that comparator — an \
             override breaks the ordering it looks like it strengthens: {name}"
        );
        assert_eq!(
            body["isDenyAllElse"], false,
            "isDenyAllElse blanks BOTH denyPolicyItems and denyExceptions: {name}"
        );
        assert!(
            body["policyItems"]
                .as_array()
                .expect("policyItems")
                .is_empty(),
            "a clearance is a DENY only: {name}"
        );

        let deny = body["denyPolicyItems"].as_array().expect("denyPolicyItems");
        assert_eq!(deny.len(), 1, "one deny item: {name}");
        assert_eq!(
            strings_of(&deny[0]["groups"]),
            vec!["public".to_string()],
            "the deny half names {{public}} — the only 'everybody' Ranger resolves with no group \
             provider: {name}"
        );
        assert!(
            deny[0]["roles"].as_array().expect("roles").is_empty(),
            "naming the clearing ROLE matches nobody on StarRocks either — `setUserRoles` is \
             never called anywhere in `fe/` (D0b): {name}"
        );

        // **The select-only ruling, asserted as an ABSENCE.** This is the
        // assertion that records the asymmetry with Trino: a user without
        // clearance can still overwrite a column they cannot read.
        assert_eq!(
            access_types(&deny[0]),
            vec!["select".to_string()],
            "`select` ALONE: StarRocks' column check has one caller and it passes SELECT, so an \
             emitted insert/update/delete could never match — decoration that reads as \
             protection: {name}"
        );

        let exc = body["denyExceptions"].as_array().expect("denyExceptions");
        assert_eq!(exc.len(), 1, "one exception item: {name}");
        assert_eq!(
            users_of(&exc[0]),
            vec![format!("{local}-2")],
            "the STORED username must be read verbatim — a re-derivation would except {local:?}, \
             clearing the wrong person and refusing the right one: {name}"
        );
        assert_eq!(
            access_types(&exc[0]),
            access_types(&deny[0]),
            "an exception not covering the denied access type excepts nothing: {name}"
        );
        assert!(
            exc[0]["roles"].as_array().expect("roles").is_empty(),
            "an unresolvable role in denyExceptions excepts NOBODY and the deny stands for \
             everyone (D0b re-derived this for StarRocks): {name}"
        );
    }
}

/// **Refuse, never truncate.** Past the per-clearance ceiling the recompile
/// FAILS — which rolls the write that caused it back — rather than writing some
/// of the denies and leaving the rest of the classified columns readable.
#[tokio::test]
async fn a_clearance_over_the_ceiling_refuses_rather_than_truncating() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;

    bulk_tag_columns(
        &db,
        tag_id,
        f.warehouse_id,
        "wide",
        DEFAULT_EXPANSION_CEILING as i64 + 1,
    )
    .await;

    let role_name = unique("sr-clearance-big");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("big")),
        &unique("bigholder"),
    )
    .await;
    grant_role(&db, holder, role_id).await;
    gate(&db, tag_id, &[&role_name]).await;

    let err = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err("a clearance over the per-clearance ceiling must REFUSE");
    match err {
        egret_perm::CompileError::ClearanceExpansionCeiling {
            count, ceiling, ..
        } => {
            assert_eq!(count, DEFAULT_EXPANSION_CEILING + 1);
            assert_eq!(ceiling, DEFAULT_EXPANSION_CEILING);
        }
        other => panic!("wrong refusal — the operator must be told this is a CLEARANCE over its ceiling, not a rule: {other:?}"),
    }

    assert!(
        clearance_policies(&db, f.service_id, tag_id)
            .await
            .is_empty(),
        "a refused clearance must write NOTHING — a partial expansion leaves the dropped columns \
         readable by exactly the people the classification exists to keep out"
    );
}

/// A clearance on a tag no column of this profile carries compiles to nothing,
/// and that is NOT a skip: there is no column to withhold. The discriminating
/// half is the second assertion — the SAME gate, once a column is tagged,
/// compiles. Without it this test would pass against a compiler that ignored
/// StarRocks clearances entirely.
#[tokio::test]
async fn a_gate_with_no_tagged_column_compiles_to_nothing_but_still_compiles_once_tagged() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;

    let role_name = unique("sr-clearance-empty");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("empty")),
        &unique("emptyholder"),
    )
    .await;
    grant_role(&db, holder, role_id).await;
    gate(&db, tag_id, &[&role_name]).await;

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
    assert!(
        clearance_policies(&db, f.service_id, tag_id)
            .await
            .is_empty(),
        "no tagged column means no column to withhold"
    );

    tag_column(&db, tag_id, f.warehouse_id, "orders", "email").await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile after tagging");
    assert_eq!(
        clearance_policies(&db, f.service_id, tag_id).await.len(),
        1,
        "the same gate must compile once a column carries the tag — otherwise this file would \
         pass against a compiler that ignored StarRocks clearances altogether"
    );
}

// ── view governance A2: the clearance MASK, folded through the evaluator ──

/// The clearance MASK for `tag_id` on `{catalog}.sales.{table}.{column}`, by
/// the name the compiler builds:
/// `<prefix><tag_id>-mask-<catalog>.<database>.<table>.<column>`.
async fn clearance_mask(
    db: &Db,
    service_id: i64,
    tag_id: Uuid,
    catalog: &str,
    table: &str,
    column: &str,
) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(format!(
        "{COMPILER_CLEARANCE_PREFIX}{tag_id}-mask-{catalog}.sales.{table}.{column}"
    ))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read")
}

/// Every clearance MASK for `tag_id` under `service_id`, ordered by name.
async fn clearance_masks(db: &Db, service_id: i64, tag_id: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 AND name LIKE $2 || '%' ORDER BY name",
    )
    .bind(service_id)
    .bind(format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}-mask-"))
    .fetch_all(&db.pool)
    .await
    .expect("policy read")
}

/// `(users, groups, dataMaskType)` of each item, in order.
fn mask_items(body: &Value) -> Vec<(Vec<String>, Vec<String>, String)> {
    body["dataMaskPolicyItems"]
        .as_array()
        .expect("dataMaskPolicyItems")
        .iter()
        .map(|i| {
            (
                strings_of(&i["users"]),
                strings_of(&i["groups"]),
                i["dataMaskInfo"]["dataMaskType"]
                    .as_str()
                    .expect("dataMaskType")
                    .to_string(),
            )
        })
        .collect()
}

/// A path mask rule on `{catalog}.sales.orders.{column}` — `*` is every column
/// of the table — exempting `exempt` (bare usernames).
fn mask_rule(f: &Fixture, column: &str, mask_type: &str, exempt: &[&str]) -> egret_perm::Rule {
    egret_perm::Rule {
        id: Uuid::nil(),
        name: unique("viewgov-mask"),
        kind: egret_perm::RuleKind::Mask {
            mask_type: mask_type.to_string(),
            value_expr: None,
        },
        selector: egret_perm::Selector::Path(egret_perm::Scope {
            catalog_id: f.catalog_id,
            schema: Some("sales".to_string()),
            table: Some("orders".to_string()),
            column: Some(column.to_string()),
        }),
        exempt: egret_perm::PrincipalSet {
            include: exempt
                .iter()
                .map(|u| egret_perm::Principal {
                    kind: egret_perm::PrincipalKind::User,
                    r#ref: u.to_string(),
                })
                .collect(),
            exclude: Vec::new(),
        },
    }
}

async fn rule_policy(db: &Db, service_id: i64, rule_id: Uuid) -> (i64, Value) {
    sqlx::query_as(
        "SELECT id, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(format!("{}{rule_id}", egret_policy::COMPILER_RULE_PREFIX))
    .fetch_one(&db.pool)
    .await
    .expect("rule policy")
}

/// Two cleared users (`ann`, `bob`) and the classification gating `ssn`.
async fn cleared_pair(db: &Db, tag_id: Uuid) -> (String, String) {
    let ann = unique("ann");
    let bob = unique("bob");
    let ann_id = new_user(db, &format!("{ann}@egret.local"), &ann).await;
    let bob_id = new_user(db, &format!("{bob}@egret.local"), &bob).await;
    let role_name = unique("viewgov-clear");
    let role_id = new_role(db, &role_name).await;
    grant_role(db, ann_id, role_id).await;
    grant_role(db, bob_id, role_id).await;
    gate(db, tag_id, &[&role_name]).await;
    (ann, bob)
}

/// **The headline of A2.** A StarRocks classified column beside a WILDCARD rule
/// mask (`MASK` on every column of the table, exempting ann) compiles to a
/// priority-1 clearance mask whose cleared items are what that rule gives each
/// cleared user — ann `MASK_NONE`, bob `MASK` — then `{public}` `MASK_NULL`.
/// The rule's own policy is untouched, and the stored-overlap report finds no
/// pair on that column: different priorities are ordered, not ambiguous.
#[tokio::test]
async fn a_classified_column_gets_a_priority_one_mask_folded_from_the_rule_masks() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;

    let ann = unique("ann");
    let rule_id = f
        .perm
        .create_rule(&mask_rule(&f, "*", "MASK", &[&ann]))
        .await
        .expect("create rule");
    // The rule policy BEFORE any clearance exists.
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile before the gate");
    let (rule_policy_id, rule_body_before) = rule_policy(&db, f.service_id, rule_id).await;
    assert!(
        clearance_masks(&db, f.service_id, tag_id).await.is_empty(),
        "no gate, no clearance mask"
    );

    let bob = unique("bob");
    let ann_id = new_user(&db, &format!("{ann}@egret.local"), &ann).await;
    let bob_id = new_user(&db, &format!("{bob}@egret.local"), &bob).await;
    let role_name = unique("viewgov-clear");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, ann_id, role_id).await;
    grant_role(&db, bob_id, role_id).await;
    gate(&db, tag_id, &[&role_name]).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with the gate");

    let mask = clearance_mask(&db, f.service_id, tag_id, &f.catalog_name, "orders", "ssn")
        .await
        .expect("the classified column must get a clearance mask");
    assert_eq!(
        mask_items(&mask),
        vec![
            (vec![ann.clone()], vec![], "MASK_NONE".to_string()),
            (vec![bob.clone()], vec![], "MASK".to_string()),
            (vec![], vec!["public".to_string()], "MASK_NULL".to_string()),
        ],
        "cleared items reproduce the rule mask per user, then {{public}} MASK_NULL"
    );
    assert_eq!(mask["policyPriority"], serde_json::json!(1));
    assert_eq!(mask["policyType"], serde_json::json!(1));
    assert_eq!(mask["serviceType"], serde_json::json!("starrocks"));
    let r = &mask["resources"];
    assert_eq!(
        strings_of(&r["catalog"]["values"]),
        vec![f.catalog_name.clone()]
    );
    assert_eq!(strings_of(&r["database"]["values"]), vec!["sales"]);
    assert_eq!(strings_of(&r["table"]["values"]), vec!["orders"]);
    assert_eq!(strings_of(&r["column"]["values"]), vec!["ssn"]);
    assert_eq!(
        clearance_masks(&db, f.service_id, tag_id).await.len(),
        1,
        "one mask per classified column"
    );
    // The deny for direct reads is still there (A1).
    assert_eq!(clearance_policies(&db, f.service_id, tag_id).await.len(), 1);

    // The rule policy is unchanged: same row, same body.
    let (rule_policy_id_after, rule_body_after) = rule_policy(&db, f.service_id, rule_id).await;
    assert_eq!(rule_policy_id_after, rule_policy_id);
    assert_eq!(rule_body_after, rule_body_before);

    // `GET /perm/overlaps`' store function reports no pair in this profile.
    let mut conn = db.pool.acquire().await.expect("conn");
    let overlaps: Vec<_> = egret_perm_compile::overlap::stored_overlaps_via(&mut conn)
        .await
        .expect("overlaps")
        .into_iter()
        .filter(|o| o.service_id == f.service_id)
        .collect();
    assert!(
        overlaps.is_empty(),
        "a priority-1 clearance mask beside a priority-0 rule mask is ordered, not ambiguous: \
         {overlaps:?}"
    );
}

/// Two priority-0 rule masks that both reach a cleared user on the classified
/// column and disagree: the evaluator is `Undecidable` for that user, and the
/// recompile refuses, naming the column and the user — nothing is guessed.
#[tokio::test]
async fn an_undecidable_cleared_user_refuses_the_recompile() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    let (ann, bob) = cleared_pair(&db, tag_id).await;
    let (first, _) = if ann < bob { (ann, bob) } else { (bob, ann) };

    f.perm
        .create_rule(&mask_rule(&f, "ssn", "MASK", &[]))
        .await
        .expect("create rule");
    f.perm
        .create_rule(&mask_rule(&f, "*", "MASK_NULL", &[]))
        .await
        .expect("create rule");

    let err = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err("an undecidable fold must refuse");
    assert_eq!(
        err,
        egret_perm::CompileError::ClearanceMaskUndecidable {
            column: format!("{}.sales.orders.ssn", f.catalog_name),
            user: first,
        }
    );
    assert!(clearance_masks(&db, f.service_id, tag_id).await.is_empty());
}

/// The fold is an evaluation at the bound engine's plugin version. An engine
/// whose image names no plugin egret has measured does NOT refuse the recompile
/// (user ruling 2026-10-13, Task 1 fix round 1): the per-column deny is still
/// compiled, no clearance mask is emitted, and the recompile reports a skip that
/// names the engine and the remedy.
#[tokio::test]
async fn an_unmodelled_bound_engine_skips_the_mask_and_keeps_the_deny() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    cleared_pair(&db, tag_id).await;
    let engine_name: String = sqlx::query_scalar(
        "UPDATE engine_mgmt.engines SET spec = '{}'::jsonb WHERE id = $1 RETURNING name",
    )
    .bind(f.engine_id)
    .fetch_one(&db.pool)
    .await
    .expect("unpin the engine");

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("an unmodelled engine must not refuse the recompile");

    assert_eq!(
        clearance_policies(&db, f.service_id, tag_id).await.len(),
        1,
        "the direct-read deny is compiled exactly as before"
    );
    assert!(
        clearance_masks(&db, f.service_id, tag_id).await.is_empty(),
        "no mask can be folded at an unmodelled plugin"
    );
    let skips: Vec<_> = summary.skipped.iter().filter(|s| s.id == tag_id).collect();
    assert_eq!(skips.len(), 1, "{:?}", summary.skipped);
    assert_eq!(
        skips[0].source_kind,
        egret_perm_compile::SourceKind::Clearance
    );
    let reason = &skips[0].reason;
    assert!(
        reason.starts_with("view protection (clearance mask) not active for "),
        "{reason}"
    );
    assert!(
        reason.contains(&format!(
            "engine {engine_name} runs an image egret has not modelled"
        )),
        "{reason}"
    );
    assert!(
        reason.contains("pin it to a known image (see docs/en/guides/engine-versions.md)"),
        "{reason}"
    );
}

/// A hand-written mask naming a group other than `public` can reach the
/// classified column. The fold evaluates a cleared user with no groups, so it
/// cannot know what an engine resolving `contractors` would serve them: the
/// mask is skipped with a reason naming the policy and the group, and the deny
/// is kept — never a silent evaluation.
#[tokio::test]
async fn a_mask_naming_a_group_skips_the_clearance_mask_and_keeps_the_deny() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    cleared_pair(&db, tag_id).await;

    let lvl =
        |v: &str| serde_json::json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let name = unique("hand-contractors");
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator')",
    )
    .bind(f.service_id)
    .bind(&name)
    .bind(serde_json::json!({
        "name": name, "service": "x", "serviceType": "starrocks",
        "policyType": 1, "isEnabled": true,
        "resources": {"catalog": lvl(&f.catalog_name), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("ssn")},
        "dataMaskPolicyItems": [
            {"users": [], "groups": ["contractors"], "roles": [],
             "accesses": [{"type": "select", "isAllowed": true}],
             "dataMaskInfo": {"dataMaskType": "MASK_NULL"}},
            {"users": [], "groups": ["public"], "roles": [],
             "accesses": [{"type": "select", "isAllowed": true}],
             "dataMaskInfo": {"dataMaskType": "MASK_NONE"}}
        ],
        "policyItems": [], "denyPolicyItems": []
    }))
    .execute(&db.pool)
    .await
    .expect("hand-written mask");

    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("a group gap must not refuse the recompile");
    assert_eq!(clearance_policies(&db, f.service_id, tag_id).await.len(), 1);
    assert!(clearance_masks(&db, f.service_id, tag_id).await.is_empty());
    let skips: Vec<_> = summary.skipped.iter().filter(|s| s.id == tag_id).collect();
    assert_eq!(skips.len(), 1, "{:?}", summary.skipped);
    assert!(skips[0].reason.contains(&name), "{}", skips[0].reason);
    assert!(
        skips[0].reason.contains("group \"contractors\""),
        "{}",
        skips[0].reason
    );
}

/// A hand-written (operator) mask on the classified column is served beside the
/// compiled set and reaches a cleared user exactly as a rule mask does, so the
/// fold reads it too. Left out, the cleared user would be folded `MASK_NONE`
/// where they are served `MASK` today.
#[tokio::test]
async fn a_hand_written_mask_is_folded_like_a_rule_mask() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    let (ann, bob) = cleared_pair(&db, tag_id).await;

    let lvl =
        |v: &str| serde_json::json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let name = unique("hand-mask");
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator')",
    )
    .bind(f.service_id)
    .bind(&name)
    .bind(serde_json::json!({
        "name": name, "service": "x", "serviceType": "starrocks",
        "policyType": 1, "isEnabled": true,
        "resources": {"catalog": lvl(&f.catalog_name), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("ssn")},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK"}
        }],
        "policyItems": [], "denyPolicyItems": []
    }))
    .execute(&db.pool)
    .await
    .expect("hand-written mask");

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
    let mask = clearance_mask(&db, f.service_id, tag_id, &f.catalog_name, "orders", "ssn")
        .await
        .expect("clearance mask");
    let mut both = vec![ann, bob];
    both.sort();
    assert_eq!(
        mask_items(&mask),
        vec![
            (both, vec![], "MASK".to_string()),
            (vec![], vec!["public".to_string()], "MASK_NULL".to_string()),
        ]
    );
}

/// The A3 `create materialized view` deny of this test's profile, if any. One
/// per StarRocks profile (revised 2026-10-14): StarRocks creates a materialized
/// view only in `default_catalog` and checks the privilege on the target
/// database, so the deny cannot be tied to the database a column lives in.
async fn mv_deny(db: &Db, f: &Fixture) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(format!("{COMPILER_CLEARANCE_PREFIX}mv"))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read")
}

/// A second classification in another database narrows the exceptions of the
/// ONE engine-wide deny to the users cleared for BOTH; untagging every column
/// on the profile deletes the policy on the next recompile.
#[tokio::test]
async fn the_mv_deny_narrows_with_a_second_classification_and_goes_with_the_last_column() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_x, _) = new_tag(&db).await;
    tag_column(&db, tag_x, f.warehouse_id, "orders", "ssn").await;
    let (ann, bob) = cleared_pair(&db, tag_x).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = mv_deny(&db, &f)
        .await
        .expect("a database with a classified column");
    assert_eq!(body["serviceType"], serde_json::json!("starrocks"));
    assert_eq!(body["policyType"], serde_json::json!(0));
    assert_eq!(
        strings_of(&body["resources"]["catalog"]["values"]),
        vec!["default_catalog"]
    );
    assert_eq!(
        strings_of(&body["resources"]["database"]["values"]),
        vec!["*"]
    );
    assert!(body["resources"].get("table").is_none());
    let deny = &body["denyPolicyItems"][0];
    assert_eq!(strings_of(&deny["groups"]), vec!["public"]);
    assert_eq!(access_types(deny), vec!["create materialized view"]);
    let mut both = vec![ann.clone(), bob.clone()];
    both.sort();
    assert_eq!(users_of(&body["denyExceptions"][0]), both);

    // A second classification in ANOTHER database, cleared for bob only: the
    // deny is still one policy, and its exceptions narrow.
    let (tag_y, _) = new_tag(&db).await;
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'hr','staff','salary')",
    )
    .bind(tag_y)
    .bind(f.warehouse_id)
    .execute(&db.pool)
    .await
    .expect("tag a column in hr");
    let bob_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(&bob)
        .fetch_one(&db.pool)
        .await
        .expect("bob");
    let role_y = unique("viewgov-clear-y");
    let role_y_id = new_role(&db, &role_y).await;
    grant_role(&db, bob_id, role_y_id).await;
    gate(&db, tag_y, &[&role_y]).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with the second classification");
    let body = mv_deny(&db, &f).await.expect("still one policy");
    assert_eq!(users_of(&body["denyExceptions"][0]), vec![bob.clone()]);
    let mv_policies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name LIKE $2 || '%'",
    )
    .bind(f.service_id)
    .bind(format!("{COMPILER_CLEARANCE_PREFIX}mv"))
    .fetch_one(&db.pool)
    .await
    .expect("count");
    assert_eq!(mv_policies, 1, "two databases, still one engine-wide deny");

    // Untag every column: the database holds no classified column any more.
    sqlx::query("DELETE FROM catalog_tag_assignments WHERE tag_id = ANY($1)")
        .bind(vec![tag_x, tag_y])
        .execute(&db.pool)
        .await
        .expect("untag");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile after untagging");
    assert!(
        mv_deny(&db, &f).await.is_none(),
        "a database with no classified column keeps no deny"
    );
}

/// Engine goes from modelled to unmodelled: the clearance mask is deleted, the
/// per-column deny stays, and a skip is reported.
#[tokio::test]
async fn binding_an_unmodelled_engine_after_a_modelled_one_removes_the_mask_only() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    cleared_pair(&db, tag_id).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with a modelled engine");
    assert_eq!(clearance_masks(&db, f.service_id, tag_id).await.len(), 1);

    sqlx::query("UPDATE engine_mgmt.engines SET spec = '{}'::jsonb WHERE id = $1")
        .bind(f.engine_id)
        .execute(&db.pool)
        .await
        .expect("unpin the engine");
    let summary = recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile with an unmodelled engine");

    assert!(
        clearance_masks(&db, f.service_id, tag_id).await.is_empty(),
        "the previously compiled mask must be deleted"
    );
    assert_eq!(clearance_policies(&db, f.service_id, tag_id).await.len(), 1);
    assert!(
        mv_deny(&db, &f).await.is_some(),
        "the MV deny is engine-independent"
    );
    assert!(
        summary
            .skipped
            .iter()
            .any(|s| s.id == tag_id && s.source_kind == egret_perm_compile::SourceKind::Clearance),
        "{:?}",
        summary.skipped
    );
}

/// The first draft filed one deny per `(catalog, database)`, named
/// `…mv-{catalog}.{database}`. Such a stored policy matches no real `CREATE
/// MATERIALIZED VIEW` (StarRocks creates one only in `default_catalog`), so a
/// recompile must sweep it by the compiler prefix and file the engine-wide one.
#[tokio::test]
async fn a_stored_per_database_mv_deny_from_the_first_draft_is_swept_on_recompile() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    cleared_pair(&db, tag_id).await;
    let old_name = format!("{COMPILER_CLEARANCE_PREFIX}mv-{}.sales", f.catalog_name);
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'compiler')",
    )
    .bind(f.service_id)
    .bind(&old_name)
    .bind(serde_json::json!({
        "name": old_name, "service": "x", "serviceType": "starrocks",
        "policyType": 0, "isEnabled": true,
        "resources": {"catalog": {"values": [f.catalog_name], "isExcludes": false, "isRecursive": false},
                      "database": {"values": ["sales"], "isExcludes": false, "isRecursive": false}},
        "denyPolicyItems": [{"groups": ["public"],
                             "accesses": [{"type": "create materialized view", "isAllowed": true}]}],
    }))
    .execute(&db.pool)
    .await
    .expect("store the first-draft deny");

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let left: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(&old_name)
    .fetch_optional(&db.pool)
    .await
    .expect("read");
    assert!(
        left.is_none(),
        "the first-draft per-database deny must be swept"
    );
    assert!(
        mv_deny(&db, &f).await.is_some(),
        "the engine-wide deny replaces it"
    );
}

/// No engine bindings: the editor routes these tests drive never read them.
struct NoEngines;
#[async_trait::async_trait]
impl egret_policy::EngineBindingReader for NoEngines {
    async fn list_engine_bindings(
        &self,
    ) -> Result<Vec<egret_policy::EngineBinding>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn engine_catalog_names(
        &self,
        _e: Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_schemas(
        &self,
        _w: Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_tables(
        &self,
        _w: Uuid,
        _s: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_columns(
        &self,
        _w: Uuid,
        _s: &str,
        _t: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
}

/// The policy editor's router as egret-bin mounts it (with the real
/// recompile seam), and a platform admin's session token.
async fn editor(db: &Db, policy: &PolicyStore) -> (axum::Router, String) {
    use std::sync::Arc;
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let uid = egret_auth::users::UserRepo::new(db.clone())
        .insert_local(&format!("{}@egret.local", unique("refold")), "T", "x")
        .await
        .expect("insert user")
        .0;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let app = egret_policy::policy_admin_router().with_state(egret_policy::PolicyAdminState {
        store: policy.clone(),
        engines: Arc::new(NoEngines),
        users: egret_auth::users::UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions,
        engine_egret_url: "http://egret:8080".into(),
        pdp,
        recompiler: Arc::new(egret_perm_compile::PermRecompiler),
    });
    (app, token)
}

async fn call(
    app: &axum::Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (axum::http::StatusCode, Value) {
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    let req = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("content-type", "application/json")
        .body(match body {
            Some(b) => axum::body::Body::from(b.to_string()),
            None => axum::body::Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Final review I-3: the clearance mask folds, for each cleared user, what the
/// profile's OTHER masks give them, hand-written ones included. Every editor
/// write of a hand-written data mask on the profile (enable, edit, disable,
/// delete) refolds it in the same transaction. Before the fix only an
/// unrelated grant, rule or tag write did, so tightening a hand-written mask
/// left the cleared users on the old, looser fold (fail-open). (The editor's
/// create is refused on a compiled profile, which a recompile stamps
/// `generated`, so the hand-written mask is seeded as an operator's from
/// before; it starts disabled.)
#[tokio::test]
async fn every_editor_write_of_a_hand_written_mask_refolds_the_clearance_mask() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    tag_column(&db, tag_id, f.warehouse_id, "orders", "ssn").await;
    let (ann, bob) = cleared_pair(&db, tag_id).await;
    let name = unique("hand-mask");
    let lvl =
        |v: &str| serde_json::json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, false, 'operator') RETURNING id",
    )
    .bind(f.service_id)
    .bind(&name)
    .bind(serde_json::json!({
        "name": name, "service": "x", "serviceType": "starrocks",
        "policyType": 1, "isEnabled": false,
        "resources": {"catalog": lvl(&f.catalog_name), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("ssn")},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK"}
        }],
        "policyItems": [], "denyPolicyItems": []
    }))
    .fetch_one(&db.pool)
    .await
    .expect("hand-written mask");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
    let mut both = vec![ann, bob];
    both.sort();
    let folded = |cleared: &str| {
        vec![
            (both.clone(), vec![], cleared.to_string()),
            (vec![], vec!["public".to_string()], "MASK_NULL".to_string()),
        ]
    };
    let fold = || async {
        mask_items(
            &clearance_mask(&db, f.service_id, tag_id, &f.catalog_name, "orders", "ssn")
                .await
                .expect("clearance mask"),
        )
    };
    assert_eq!(fold().await, folded("MASK_NONE"), "the hand mask is off");

    let (app, token) = editor(&db, &f.policy).await;
    let profile: String =
        sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
            .bind(f.service_id)
            .fetch_one(&db.pool)
            .await
            .expect("profile name");
    let form = |mask: &str| {
        serde_json::json!({
            "kind": "masking", "name": name, "service": profile, "isEnabled": true,
            "resources": {"catalog": {"values": [f.catalog_name]}, "database": {"values": ["sales"]},
                          "table": {"values": ["orders"]}, "column": {"values": ["ssn"]}},
            "items": [{"groups": ["public"], "maskType": mask}]
        })
    };

    // Enable: tightens the cleared users' outcome.
    let enabled = |on: bool| serde_json::json!({ "enabled": on });
    let uri = format!("/policies/{id}/enabled");
    let (status, body) = call(&app, &token, "PUT", &uri, Some(enabled(true))).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(fold().await, folded("MASK"), "enable refolds");

    // Edit.
    let (status, body) = call(
        &app,
        &token,
        "PUT",
        &format!("/policies/{id}"),
        Some(form("MASK_HASH")),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(fold().await, folded("MASK_HASH"), "edit refolds");

    // Disable, then enable.
    let (status, body) = call(&app, &token, "PUT", &uri, Some(enabled(false))).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(fold().await, folded("MASK_NONE"), "disable refolds");
    let (status, body) = call(&app, &token, "PUT", &uri, Some(enabled(true))).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(fold().await, folded("MASK_HASH"), "enable refolds");

    // Delete.
    let (status, body) = call(&app, &token, "DELETE", &format!("/policies/{id}"), None).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(fold().await, folded("MASK_NONE"), "delete refolds");
}

/// Final re-review N-2: the refold is scoped to a profile whose compile folds a
/// clearance mask. On a profile with NO compiled clearance mask whose recompile
/// is refused for an unrelated reason (here: a clearance over the expansion
/// ceiling, seeded as the ceiling test above does), a steward must still be
/// able to enable, disable and delete a hand-written mask, the way out of such
/// a state. The write has nothing to refold, so the refused recompile must not
/// refuse it.
#[tokio::test]
async fn a_profile_with_no_compiled_clearance_mask_does_not_refold_on_a_mask_write() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    bulk_tag_columns(
        &db,
        tag_id,
        f.warehouse_id,
        "wide",
        DEFAULT_EXPANSION_CEILING as i64 + 1,
    )
    .await;
    let role_name = unique("sr-clearance-refused");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("refused")),
        &unique("refusedholder"),
    )
    .await;
    grant_role(&db, holder, role_id).await;
    gate(&db, tag_id, &[&role_name]).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err("the seeded recompile must be refused");
    assert!(
        clearance_masks(&db, f.service_id, tag_id).await.is_empty(),
        "the profile must hold no compiled clearance mask"
    );

    let name = unique("hand-mask-refused");
    let lvl =
        |v: &str| serde_json::json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, false, 'operator') RETURNING id",
    )
    .bind(f.service_id)
    .bind(&name)
    .bind(serde_json::json!({
        "name": name, "service": "x", "serviceType": "starrocks",
        "policyType": 1, "isEnabled": false,
        "resources": {"catalog": lvl(&f.catalog_name), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("ssn")},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK"}
        }],
        "policyItems": [], "denyPolicyItems": []
    }))
    .fetch_one(&db.pool)
    .await
    .expect("hand-written mask");

    let (app, token) = editor(&db, &f.policy).await;
    let uri = format!("/policies/{id}/enabled");
    for on in [true, false] {
        let (status, body) = call(
            &app,
            &token,
            "PUT",
            &uri,
            Some(serde_json::json!({ "enabled": on })),
        )
        .await;
        assert!(
            status.is_success(),
            "enabled={on} must not be refused by a recompile with nothing to refold: {status} {body}"
        );
    }
    let (status, body) = call(&app, &token, "DELETE", &format!("/policies/{id}"), None).await;
    assert!(status.is_success(), "delete: {status} {body}");
}

/// Final re-review N-2 (other side): `origin = 'generated'` alone scopes the
/// refold. Same seed as the test above, stamped `generated` with no clearance
/// mask row: the mask write refolds, so the refused recompile refuses it.
#[tokio::test]
async fn a_generated_profile_without_a_compiled_clearance_mask_still_refolds_on_a_mask_write() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks(&db).await;
    let (tag_id, _key) = new_tag(&db).await;
    bulk_tag_columns(
        &db,
        tag_id,
        f.warehouse_id,
        "wide",
        DEFAULT_EXPANSION_CEILING as i64 + 1,
    )
    .await;
    let role_name = unique("sr-clearance-refused");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("refused")),
        &unique("refusedholder"),
    )
    .await;
    grant_role(&db, holder, role_id).await;
    gate(&db, tag_id, &[&role_name]).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect_err("the seeded recompile must be refused");
    assert!(
        clearance_masks(&db, f.service_id, tag_id).await.is_empty(),
        "the profile must hold no compiled clearance mask"
    );

    // The compiler stamped the profile `generated` on an earlier compile whose
    // clearance mask is now skipped, so no clearance mask row exists.
    sqlx::query("UPDATE policy.authz_services SET origin = 'generated' WHERE id = $1")
        .bind(f.service_id)
        .execute(&db.pool)
        .await
        .expect("stamp generated");
    let name = unique("hand-mask-generated");
    let lvl =
        |v: &str| serde_json::json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, false, 'operator') RETURNING id",
    )
    .bind(f.service_id)
    .bind(&name)
    .bind(serde_json::json!({
        "name": name, "service": "x", "serviceType": "starrocks",
        "policyType": 1, "isEnabled": false,
        "resources": {"catalog": lvl(&f.catalog_name), "database": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("ssn")},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK"}
        }],
        "policyItems": [], "denyPolicyItems": []
    }))
    .fetch_one(&db.pool)
    .await
    .expect("hand-written mask");

    let (app, token) = editor(&db, &f.policy).await;
    let uri = format!("/policies/{id}/enabled");
    let (status, body) = call(
        &app,
        &token,
        "PUT",
        &uri,
        Some(serde_json::json!({ "enabled": true })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::CONFLICT,
        "a generated profile refolds on a mask write, so the refused recompile refuses it: {body}"
    );
}
