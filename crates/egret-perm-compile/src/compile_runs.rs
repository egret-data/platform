//! `perm.compile_runs` (DG #3, spec §3.3, migration 0198): the last compile
//! of each profile, read by the Governance Overview's `compile_failed`.
//!
//! - **Success** is upserted by the compile funnel
//!   (`trigger::recompile_locked_service_in_tx`), on the compile's own
//!   transaction, so a compile whose caller rolls back leaves no trace. Every
//!   compile path reaches that one function.
//! - **Failure** is upserted only by entry points no write waits on
//!   ([`crate::recompile_service`] and the manual compile route), after their
//!   transaction rolled back. A compile that fails inside a grant, rule, tag or
//!   policy write is NOT recorded: that write is refused with the error, its
//!   caller sees it, and the engine keeps the previous compile.
use chrono::{DateTime, Utc};
use egret_perm::CompileError;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

use crate::CompileSummary;

/// The longest error text stored, in characters.
pub const MAX_ERROR_CHARS: usize = 2000;

/// `error`, cut to [`MAX_ERROR_CHARS`] characters.
pub fn capped(error: &str) -> String {
    error.chars().take(MAX_ERROR_CHARS).collect()
}

fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Upsert `outcome = 'ok'` with `s`'s counts on the compile's transaction.
///
/// Both upserts stamp `clock_timestamp()` (when the compile ran, not when its
/// transaction began: a transaction can begin before a failure is recorded and
/// compile after it) and keep whichever attempt is newer, so the row is always
/// the last attempt, whichever of the two was written last.
pub(crate) async fn record_compile_ok_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    s: &CompileSummary,
) -> Result<(), CompileError> {
    sqlx::query(
        "INSERT INTO perm.compile_runs \
             (service_id, outcome, attempted_at, error, last_ok_at, \
              created, updated, deleted, unchanged, skipped) \
         SELECT $1, 'ok', c.t, NULL, c.t, $2, $3, $4, $5, $6 \
           FROM (SELECT clock_timestamp() AS t) c \
         ON CONFLICT (service_id) DO UPDATE SET \
             outcome = 'ok', attempted_at = EXCLUDED.attempted_at, error = NULL, \
             last_ok_at = EXCLUDED.last_ok_at, created = EXCLUDED.created, \
             updated = EXCLUDED.updated, deleted = EXCLUDED.deleted, \
             unchanged = EXCLUDED.unchanged, skipped = EXCLUDED.skipped \
         WHERE perm.compile_runs.attempted_at <= EXCLUDED.attempted_at",
    )
    .bind(service_id)
    .bind(count(s.created))
    .bind(count(s.updated))
    .bind(count(s.deleted))
    .bind(count(s.unchanged))
    .bind(count(s.skipped.len()))
    .execute(&mut **tx)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;
    Ok(())
}

/// Upsert `outcome = 'error'` on its own connection, after the failed
/// compile's transaction rolled back. Keeps `last_ok_at` and the counts of the
/// last success. Never overwrites a newer attempt, and records nothing for a
/// profile that does not exist.
pub async fn record_compile_failure(
    pool: &PgPool,
    service_id: i64,
    error: &str,
) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    record_compile_failure_via(&mut conn, service_id, error).await
}

/// [`record_compile_failure`] on a caller's connection — the manual compile
/// route writes it in one transaction with its `profile:compile` event.
pub async fn record_compile_failure_via(
    conn: &mut PgConnection,
    service_id: i64,
    error: &str,
) -> Result<(), sqlx::Error> {
    let error = capped(error);
    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, error) \
         SELECT s.id, 'error', clock_timestamp(), $2 \
           FROM policy.authz_services s WHERE s.id = $1 \
         ON CONFLICT (service_id) DO UPDATE SET \
             outcome = 'error', attempted_at = EXCLUDED.attempted_at, error = EXCLUDED.error \
         WHERE perm.compile_runs.attempted_at <= EXCLUDED.attempted_at",
    )
    .bind(service_id)
    .bind(error)
    .execute(conn)
    .await?;
    Ok(())
}

/// One profile's last compile, with the profile's name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CompileRun {
    pub service_id: i64,
    pub service_name: String,
    pub outcome: String,
    pub attempted_at: DateTime<Utc>,
    pub error: Option<String>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub created: i32,
    pub updated: i32,
    pub deleted: i32,
    pub unchanged: i32,
    pub skipped: i32,
}

/// Every profile whose last compile failed, by profile name.
pub async fn failed_compile_runs_via(
    conn: &mut PgConnection,
) -> Result<Vec<CompileRun>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.service_id, s.name AS service_name, r.outcome, r.attempted_at, r.error, \
                r.last_ok_at, r.created, r.updated, r.deleted, r.unchanged, r.skipped \
           FROM perm.compile_runs r JOIN policy.authz_services s ON s.id = r.service_id \
          WHERE r.outcome = 'error' \
          ORDER BY s.name",
    )
    .fetch_all(conn)
    .await
}
