//! `managed_by` (the column, which enforces) and the name predicates (which
//! `attribution` reads, and which the migration's backfill mirrors) must not
//! drift apart.
//!
//! This test runs the backfill `UPDATE` statements that are IN
//! `migrations/0180_policy_managed_by.sql` (loaded with `include_str!`, never
//! re-typed here) over a fixture whose rows start with the column default, and
//! then compares the result with the expected owner and with the Rust
//! predicates. A broken backfill (say `LIKE 'all%'` without the ` - `
//! separator, or the `all - ` arm without its `type = 'trino'` join) therefore
//! fails here.
//!
//! Scoped to this fixture's own two `service_id`s (one Trino, one StarRocks):
//! other tests in this binary write into the same per-process database, so an
//! unscoped `SELECT` or `UPDATE` would touch their rows. The fixture therefore
//! carries one row of EVERY shape on each type — all three owners plus the
//! near-misses — because shapes, not row counts, are what the predicates can
//! disagree about. The whole-table check belongs against a real cluster (plan
//! Task 10 Step 6).
//!
//! The extractor strips only WHOLE-LINE `--` comments (0180 says so too): a
//! trailing comment on a SQL line would comment out the scoping appended here.

use egret_metastore::Db;
use egret_policy::infra_names::is_egret_infrastructure_policy;
use egret_policy::is_compiler_owned_policy_name;

const MIGRATION: &str = include_str!("../../../migrations/0180_policy_managed_by.sql");

/// The migration's `UPDATE policy.authz_policies ... ;` statements, each with
/// its `WHERE` predicate wrapped and restricted to `service_id = ANY($1)`.
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
            format!("{head}WHERE ({predicate}) AND service_id = ANY($1)")
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
    let store = egret_policy::PolicyStore::new(db.clone());
    egret_policy::seed_trino(&store)
        .await
        .expect("seed trino service-def");
    egret_policy::seed_starrocks(&store)
        .await
        .expect("seed starrocks service-def");

    // A parsing change must not be able to make this test run nothing.
    let backfill = scoped_backfill_statements();
    assert_eq!(
        backfill.len(),
        2,
        "expected exactly the compiler and infra backfill UPDATEs: {backfill:#?}"
    );

    let mut service_ids = std::collections::BTreeMap::new();
    for ty in ["trino", "starrocks"] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO policy.authz_services (name, type, config_jsonb) \
             VALUES ($1, $2, '{}'::jsonb) RETURNING id",
        )
        .bind(format!("t-agree-{ty}-{}", uuid::Uuid::new_v4().simple()))
        .bind(ty)
        .fetch_one(&pool)
        .await
        .expect("service");
        service_ids.insert(ty, id);
    }
    let ids: Vec<i64> = service_ids.values().copied().collect();

    let trino_fixture = [
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
    // SP4d-3 final review I-1 (USER RULING, narrowed): on StarRocks only the
    // `all - ` shape changes owner. The default set is Trino-shaped, so a
    // StarRocks `all - …` row is an operator's (deletable); the per-admin set
    // and the delegator's names stay egret's on every type.
    let starrocks_fixture = [
        ("all - catalog, database, table", "operator"),
        ("egret-admin - catalog, database, table", "infra"),
        ("egret-query-timeout", "infra"),
        (
            "perm-grant-44444444-4444-4444-4444-444444444444",
            "compiler",
        ),
        ("sales can read orders", "operator"),
    ];
    let fixture: Vec<(&str, &str, &str)> = trino_fixture
        .iter()
        .map(|(n, e)| ("trino", *n, *e))
        .chain(starrocks_fixture.iter().map(|(n, e)| ("starrocks", *n, *e)))
        .collect();
    // Rows take the column DEFAULT; only the backfill may move them off it.
    for (ty, name, _) in &fixture {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
             VALUES ($1, $2, '{}'::jsonb)",
        )
        .bind(service_ids[ty])
        .bind(name)
        .execute(&pool)
        .await
        .expect("insert");
    }

    for stmt in &backfill {
        // Audited: the text is the repo's own migration, read at compile time;
        // the only runtime value (`service_id`) is a bind parameter.
        sqlx::query(sqlx::AssertSqlSafe(stmt.as_str()))
            .bind(&ids)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("backfill statement failed: {e}\n{stmt}"));
    }

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT s.type, p.name, p.managed_by FROM policy.authz_policies p \
         JOIN policy.authz_services s ON s.id = p.service_id \
         WHERE p.service_id = ANY($1) ORDER BY p.id",
    )
    .bind(&ids)
    .fetch_all(&pool)
    .await
    .expect("read back");

    // The control that makes a zero meaningful: if the fixture had not landed,
    // or the WHERE were wrong, this would be 0 and the loop below would assert
    // nothing at all while still reporting `ok`.
    assert_eq!(rows.len(), 15, "fixture did not land");

    let mut disagreements = Vec::new();
    for ((ty, name, managed_by), (fixture_ty, fixture_name, expected)) in
        rows.iter().zip(fixture.iter().copied())
    {
        assert_eq!(
            (ty.as_str(), name.as_str()),
            (fixture_ty, fixture_name),
            "row order changed"
        );
        let from_names = if is_compiler_owned_policy_name(name) {
            "compiler"
        } else if is_egret_infrastructure_policy(ty, name) {
            "infra"
        } else {
            "operator"
        };
        if managed_by != expected {
            disagreements.push(format!(
                "{ty}/{name:?}: backfilled column={managed_by} expected={expected}"
            ));
        }
        if managed_by != from_names {
            disagreements.push(format!(
                "{ty}/{name:?}: backfilled column={managed_by} predicates={from_names}"
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
