//! `managed_by` (the column, which enforces) and the name predicates (which
//! `attribution` reads, and which the migration's backfill mirrors) must not
//! drift apart.
//!
//! This test runs the backfill `UPDATE` statements that are IN
//! `migrations/0180_policy_managed_by.sql` (loaded with `include_str!`, never
//! re-typed here) over a fixture whose rows start with the column default, and
//! then compares the result with the expected owner and with the Rust
//! predicates. A broken backfill (say `LIKE 'all%'` without the ` - `
//! separator) therefore fails here.
//!
//! Scoped to this fixture's own `service_id`: the test database is shared with
//! every other concurrently-running test in this binary, so an unscoped
//! `SELECT` or `UPDATE` would touch their rows. The fixture therefore carries
//! one row of EVERY shape — all three owners plus the near-misses — because
//! shapes, not row counts, are what the predicates can disagree about. The
//! whole-table check belongs against a real cluster (plan Task 10 Step 6).

use egret_metastore::Db;
use egret_policy::infra_names::is_egret_infrastructure_policy_name;
use egret_policy::is_compiler_owned_policy_name;

const MIGRATION: &str = include_str!("../../../migrations/0180_policy_managed_by.sql");

/// The migration's `UPDATE policy.authz_policies ... ;` statements, each with
/// its `WHERE` predicate wrapped and restricted to `service_id = $1`.
fn scoped_backfill_statements() -> Vec<String> {
    let without_comments: String = MIGRATION
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    without_comments
        .split(';')
        .map(str::trim)
        .filter(|s| s.starts_with("UPDATE policy.authz_policies"))
        .map(|stmt| {
            let at = stmt.find("WHERE").expect("backfill UPDATE has a WHERE");
            let (head, predicate) = stmt.split_at(at);
            let predicate = predicate.trim_start_matches("WHERE").trim();
            format!("{head}WHERE ({predicate}) AND service_id = $1")
        })
        .collect()
}

#[tokio::test]
async fn the_column_and_the_name_predicates_agree_on_every_shape() {
    let Some(url) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    // `authz_services.type` is a foreign key to the service-def table.
    egret_policy::seed_trino(&egret_policy::PolicyStore::new(db.clone()))
        .await
        .expect("seed trino service-def");

    // A parsing change must not be able to make this test run nothing.
    let backfill = scoped_backfill_statements();
    assert_eq!(
        backfill.len(),
        2,
        "expected exactly the compiler and infra backfill UPDATEs: {backfill:#?}"
    );

    let service_id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'trino', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("t-agree-{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("service");

    let fixture = [
        (
            "perm-grant-11111111-1111-1111-1111-111111111111",
            "compiler",
        ),
        ("perm-rule-22222222-2222-2222-2222-222222222222", "compiler"),
        (
            "perm-clearance-33333333-3333-3333-3333-333333333333",
            "compiler",
        ),
        ("egret-admin - catalog, schema, table", "infra"),
        ("all - catalog, schema, table", "infra"),
        ("egret-delegator-impersonate", "infra"),
        ("egret-query-timeout", "infra"),
        ("all-hands-reporting", "operator"),
        ("egret-admin-notes", "operator"),
        ("sales can read orders", "operator"),
    ];
    // Rows take the column DEFAULT; only the backfill may move them off it.
    for (name, _) in fixture {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
             VALUES ($1, $2, '{}'::jsonb)",
        )
        .bind(service_id)
        .bind(name)
        .execute(&pool)
        .await
        .expect("insert");
    }

    for stmt in &backfill {
        // Audited: the text is the repo's own migration, read at compile time;
        // the only runtime value (`service_id`) is a bind parameter.
        sqlx::query(sqlx::AssertSqlSafe(stmt.as_str()))
            .bind(service_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("backfill statement failed: {e}\n{stmt}"));
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, managed_by FROM policy.authz_policies WHERE service_id = $1 ORDER BY id",
    )
    .bind(service_id)
    .fetch_all(&pool)
    .await
    .expect("read back");

    // The control that makes a zero meaningful: if the fixture had not landed,
    // or the WHERE were wrong, this would be 0 and the loop below would assert
    // nothing at all while still reporting `ok`.
    assert_eq!(rows.len(), 10, "fixture did not land");

    let mut disagreements = Vec::new();
    for ((name, managed_by), (fixture_name, expected)) in rows.iter().zip(fixture) {
        assert_eq!(name, fixture_name, "row order changed");
        let from_names = if is_compiler_owned_policy_name(name) {
            "compiler"
        } else if is_egret_infrastructure_policy_name(name) {
            "infra"
        } else {
            "operator"
        };
        if managed_by != expected {
            disagreements.push(format!(
                "{name:?}: backfilled column={managed_by} expected={expected}"
            ));
        }
        if managed_by != from_names {
            disagreements.push(format!(
                "{name:?}: backfilled column={managed_by} predicates={from_names}"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "the backfilled column disagrees on {} check(s):\n  {}",
        disagreements.len(),
        disagreements.join("\n  ")
    );
}
