//! Write a profile's `egret-admin - …` rows as a test ARRANGEMENT.
//!
//! Since governance SP2a, convergence (`egret_perm_compile::infra`) is the only
//! production writer of these rows, and it derives their users from the store
//! (the active platform admins). A test that needs a body naming someone else
//! (a non-admin, a stale holder, a foreign def's hierarchies) writes it here:
//! the bodies `admin_all_access_bodies` builds, upserted `managed_by = 'infra'`,
//! with one `policy_version` bump, as the removed apply-time seeder did.

use egret_policy::default_policies::admin_all_access_bodies;

/// Upsert `service_id`'s `egret-admin - …` rows for `users`, built from
/// `service_type`'s def with `service_name` in the bodies. Returns how many
/// rows were written.
pub async fn write_admin_rows(
    pool: &sqlx::PgPool,
    service_id: i64,
    service_name: &str,
    service_type: &str,
    users: &[String],
) -> usize {
    let bodies = admin_all_access_bodies(service_type, service_name, users).expect("bodies");
    for (name, body) in &bodies {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
             VALUES ($1, $2, $3, 'infra') \
             ON CONFLICT (service_id, name) DO UPDATE \
               SET policy_jsonb = EXCLUDED.policy_jsonb, managed_by = 'infra', \
                   version = policy.authz_policies.version + 1, updated_at = now()",
        )
        .bind(service_id)
        .bind(name)
        .bind(body)
        .execute(pool)
        .await
        .expect("write egret-admin row");
    }
    sqlx::query(
        "UPDATE policy.authz_services SET policy_version = policy_version + 1, \
         updated_at = now() WHERE id = $1",
    )
    .bind(service_id)
    .execute(pool)
    .await
    .expect("bump policy_version");
    bodies.len()
}
