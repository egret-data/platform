//! Migration 0195 reserves the `perm-engine-` prefix (spec E19): it renames
//! every policy under that prefix the compiler does not own, so the sweep that
//! reaps uncompiled `perm-engine-*` rows can never reap a hand-written one.
//!
//! The statement run here is the one IN `migrations/0195_perm_engine_grants.sql`
//! (`include_str!`, never re-typed), with its `WHERE` restricted to this
//! fixture's own service ids: other tests in this binary share the database.
//! Like `managed_by_agreement.rs`, the extractor strips only WHOLE-LINE `--`
//! comments.
//!
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grant_prefix_rename

use egret_metastore::Db;

const MIGRATION: &str = include_str!("../../../migrations/0195_perm_engine_grants.sql");

/// Every statement of the migration that renames policies, each with the
/// renaming `UPDATE`'s predicate wrapped and scoped to `service_id = ANY($1)`.
fn scoped_rename_statements() -> Vec<String> {
    let without_comments: String = MIGRATION
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    without_comments
        .split(';')
        .map(str::trim)
        .filter(|s| s.contains("UPDATE policy.authz_policies"))
        .map(|stmt| {
            let update = stmt
                .find("UPDATE policy.authz_policies")
                .expect("located above");
            let where_at = update + stmt[update..].find("WHERE").expect("rename has a WHERE");
            let returning_at = where_at
                + stmt[where_at..]
                    .find("RETURNING")
                    .expect("rename returns its service ids");
            let predicate = stmt[where_at..returning_at]
                .trim_start_matches("WHERE")
                .trim();
            format!(
                "{}WHERE ({predicate}) AND service_id = ANY($1)\n    {}",
                &stmt[..where_at],
                &stmt[returning_at..]
            )
        })
        .collect()
}

#[tokio::test]
async fn the_rename_moves_only_hand_written_perm_engine_policies() {
    let Some(url) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = egret_policy::PolicyStore::new(db.clone());
    egret_policy::seed_trino(&store)
        .await
        .expect("seed trino service-def");

    // A parsing change must not be able to make this test run nothing.
    let renames = scoped_rename_statements();
    assert_eq!(
        renames.len(),
        1,
        "expected exactly one renaming UPDATE: {renames:#?}"
    );

    let service_id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'trino', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("t-0195-{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("service");
    // A second profile with a hand-written `perm-engine-` row that the scoping
    // keeps out of this run: proves the scoping, and that the bump follows the
    // renamed rows' services only.
    let other_service: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'trino', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("t-0195-other-{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("other service");

    for (sid, name, managed_by) in [
        (service_id, "perm-engine-x", "operator"),
        (service_id, "perm-engine-y", "compiler"),
        (service_id, "perm-grant-z", "compiler"),
        (other_service, "perm-engine-x", "operator"),
    ] {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
             VALUES ($1, $2, jsonb_build_object('name', $2::text, 'isEnabled', true), $3)",
        )
        .bind(sid)
        .bind(name)
        .bind(managed_by)
        .execute(&pool)
        .await
        .expect("insert");
    }

    type Row = (String, serde_json::Value, i64, String);
    async fn rows(pool: &sqlx::PgPool, sid: i64) -> Vec<Row> {
        sqlx::query_as(
            "SELECT name, policy_jsonb, version, managed_by FROM policy.authz_policies \
             WHERE service_id = $1 ORDER BY id",
        )
        .bind(sid)
        .fetch_all(pool)
        .await
        .expect("read")
    }
    async fn policy_version(pool: &sqlx::PgPool, sid: i64) -> i64 {
        sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
            .bind(sid)
            .fetch_one(pool)
            .await
            .expect("policy_version")
    }
    let before = rows(&pool, service_id).await;
    let other_before = rows(&pool, other_service).await;
    let version_before = policy_version(&pool, service_id).await;
    let other_version_before = policy_version(&pool, other_service).await;
    assert_eq!(before.len(), 3, "fixture did not land");

    for stmt in &renames {
        // Audited: the text is the repo's own migration, read at compile time;
        // the only runtime value (the service ids) is a bind parameter.
        sqlx::query(sqlx::AssertSqlSafe(stmt.as_str()))
            .bind(vec![service_id])
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("rename statement failed: {e}\n{stmt}"));
    }

    let after = rows(&pool, service_id).await;
    assert_eq!(
        after[0].0, "renamed by 0195: perm-engine-x",
        "column renamed"
    );
    assert_eq!(
        after[0].1.get("name").and_then(|v| v.as_str()),
        Some("renamed by 0195: perm-engine-x"),
        "body renamed"
    );
    assert_eq!(after[0].2, before[0].2 + 1, "row version bumped once");
    assert_eq!(after[0].3, "operator", "owner unchanged");
    assert_eq!(
        after[1], before[1],
        "a compiler-owned perm-engine- row is untouched"
    );
    assert_eq!(after[2], before[2], "another prefix is untouched");
    assert_eq!(
        policy_version(&pool, service_id).await,
        version_before + 1,
        "the renamed row's profile is bumped once"
    );
    assert_eq!(rows(&pool, other_service).await, other_before, "scoped out");
    assert_eq!(
        policy_version(&pool, other_service).await,
        other_version_before,
        "a profile with no renamed row is not bumped"
    );
}
