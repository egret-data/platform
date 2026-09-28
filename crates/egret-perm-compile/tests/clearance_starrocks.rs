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
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("clr-eng"))
    .bind(service.id)
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
async fn clearance_policies(db: &Db, service_id: i64, tag_id: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 AND name LIKE $2 || '%' ORDER BY name",
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
