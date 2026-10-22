//! Claiming due schedules, and writing what happened.
//!
//! The claim is one statement, and it is the whole concurrency story:
//!
//! ```sql
//! UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n) RETURNING …
//! ```
//!
//! `SKIP LOCKED` is what lets N replicas tick at once with no leader election
//! and no external queue: a row another replica has already locked is invisible
//! to this one rather than a source of contention. Marking the claimed row
//! `working` in the same statement closes the other half — two claimers a
//! millisecond apart, whose locks do not overlap, still cannot both take it.
//!
//! `working_timeout_s` exists for the failure this design cannot prevent: a
//! replica that dies mid-run leaves its row `working` forever, and nothing else
//! would ever pick it up. A claim older than the timeout is taken back.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::AutomationError;
use crate::repo::Schedule;

/// The columns a claim returns — the same set `ScheduleRepo` reads, so a
/// claimed row and a fetched row are the same type.
const COLS: &str = "id, workspace_id, type, name, description, active, crontab, timezone, \
     next_run_at, database_id, sql, validator_type, validator_config, last_eval_at, last_state, \
     last_value, chart_id, dashboard_id, report_format, custom_width, custom_height, \
     dataapi_endpoint_id, dataapi_metric, dataapi_error_code, dataapi_window_s, \
     grace_period_s, working_timeout_s, log_retention_days, email_subject, message, message_footer, \
     skip_if_empty, folder_id, deleted_at, created_by, updated_at";

/// Take up to `limit` schedules that are due, marking each `working`.
///
/// Accepts any executor so a test can hold the claim open inside a transaction
/// and watch a second claimer skip it; production passes the pool and gets
/// autocommit.
pub async fn claim_due(
    exec: impl sqlx::PgExecutor<'_>,
    limit: i64,
) -> Result<Vec<Schedule>, AutomationError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE automation.schedules SET last_state = 'working', last_eval_at = now() \
         WHERE id IN ( \
           SELECT id FROM automation.schedules \
            WHERE active \
              AND deleted_at IS NULL \
              AND next_run_at IS NOT NULL \
              AND next_run_at <= now() \
              AND ( \
                last_state IS DISTINCT FROM 'working' \
                OR last_eval_at IS NULL \
                OR last_eval_at < now() - make_interval(secs => working_timeout_s) \
              ) \
            ORDER BY next_run_at \
            FOR UPDATE SKIP LOCKED \
            LIMIT $1 \
         ) \
         RETURNING {COLS}"
    )))
    .bind(limit)
    .fetch_all(exec)
    .await
    .map_err(AutomationError::from_db)
}

/// Whether this schedule fired recently enough that it must stay quiet.
///
/// The anti-spam rule from §5.1, and the reason `grace` is a state rather than
/// a silent skip: without it a 1-minute alert on a condition that stays true
/// sends 60 mails an hour, and the user's only recourse is to delete it.
///
/// Measured from the last run that REACHED SOMEONE (`notification_sent_at`),
/// not from the last run and not from the last `success`. Consequences, all
/// deliberate:
///
/// - a `noop` — ran, condition false — must not extend the quiet period, or a
///   flapping alert would silence itself;
/// - a firing whose every destination failed (no transport configured, a
///   webhook that refused) starts NO grace period (KI-033): nobody was told,
///   so the next tick must try again rather than stay quiet for an hour;
/// - a firing that reached at least one destination starts it even when
///   another destination failed, because re-firing every tick would spam the
///   recipients who did get the message.
pub async fn in_grace(db: &Db, id: Uuid, grace_period_s: i32) -> Result<bool, AutomationError> {
    if grace_period_s <= 0 {
        return Ok(false);
    }
    // `EXISTS` rather than `SELECT 1 … LIMIT 1`: it always returns exactly one
    // boolean, so the "found" and "not found" paths decode identically. The
    // earlier shape decoded fine whenever nothing matched and only failed once
    // a row came back — a bug that hides in exactly the case that matters.
    sqlx::query_scalar(
        "SELECT EXISTS( \
           SELECT 1 FROM automation.schedule_runs \
            WHERE schedule_id = $1 AND notification_sent_at IS NOT NULL \
              AND started_at > now() - make_interval(secs => $2) \
         )",
    )
    .bind(id)
    .bind(grace_period_s)
    .fetch_one(&db.pool)
    .await
    .map_err(AutomationError::from_db)
}

/// Append a run-log row.
///
/// Called on EVERY terminal path — success, error, noop, grace — because the
/// run log is the only answer to "why didn't I get an email?", and a path that
/// writes nothing is indistinguishable from a scheduler that never ran.
pub async fn record_run(
    db: &Db,
    id: Uuid,
    state: &str,
    value: Option<f64>,
    error: Option<&str>,
    notified: bool,
) -> Result<Uuid, AutomationError> {
    sqlx::query_scalar(
        "INSERT INTO automation.schedule_runs \
           (schedule_id, state, ended_at, value, error, notification_sent_at) \
         VALUES ($1, $2, now(), $3, $4, CASE WHEN $5 THEN now() ELSE NULL END) \
         RETURNING id",
    )
    .bind(id)
    .bind(state)
    .bind(value)
    .bind(error)
    .bind(notified)
    .fetch_one(&db.pool)
    .await
    .map_err(AutomationError::from_db)
}

/// Release a claimed schedule: record its outcome and schedule the next run.
///
/// `next_run_at` is recomputed from the crontab here rather than left alone,
/// because the claim query keys on it: a row released without a new next run
/// would be re-claimed on the very next tick, forever.
pub async fn release(
    db: &Db,
    id: Uuid,
    state: &str,
    value: Option<f64>,
    next_run_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "UPDATE automation.schedules \
         SET last_state = $2, last_value = $3, next_run_at = $4, updated_at = updated_at \
         WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(value)
    .bind(next_run_at)
    .execute(&db.pool)
    .await
    .map_err(AutomationError::from_db)?;
    Ok(())
}

/// Write a MANUAL run's outcome onto the schedule (KI-032), so the list's
/// **Last run** and the detail badge show what "Run now" just did.
///
/// Unlike [`release`] it leaves `next_run_at` alone: a manual run must not move
/// the schedule's own cadence. And it skips a row a tick has claimed
/// (`working`): overwriting that marker would make the row claimable again
/// while the tick is still running it, and the tick's own `release` writes the
/// outcome a moment later anyway.
pub async fn record_manual_outcome(
    db: &Db,
    id: Uuid,
    state: &str,
    value: Option<f64>,
) -> Result<(), AutomationError> {
    sqlx::query(
        "UPDATE automation.schedules \
         SET last_state = $2, last_value = $3, last_eval_at = now(), updated_at = updated_at \
         WHERE id = $1 AND last_state IS DISTINCT FROM 'working'",
    )
    .bind(id)
    .bind(state)
    .bind(value)
    .execute(&db.pool)
    .await
    .map_err(AutomationError::from_db)?;
    Ok(())
}

/// Delete run-log rows older than each schedule's `log_retention_days`.
/// Returns how many were removed.
pub async fn prune_runs(db: &Db) -> Result<u64, AutomationError> {
    let done = sqlx::query(
        "DELETE FROM automation.schedule_runs r \
         USING automation.schedules s \
         WHERE s.id = r.schedule_id \
           AND r.started_at < now() - make_interval(days => s.log_retention_days)",
    )
    .execute(&db.pool)
    .await
    .map_err(AutomationError::from_db)?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A due alert, ready to be claimed.
    async fn seed(db: &Db, name: &str, due: bool) -> Uuid {
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id",
        )
        .bind(format!("{name}_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
        )
        .bind(format!("w-{}", Uuid::new_v4().simple()))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let database: Uuid = sqlx::query_scalar(
            "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
             VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
        )
        .bind(format!("db-{}", Uuid::new_v4().simple()))
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap();

        sqlx::query_scalar(
            "INSERT INTO automation.schedules \
               (workspace_id, type, name, crontab, next_run_at, database_id, sql, validator_type) \
             VALUES ($1,'alert',$2,'* * * * *', \
                     CASE WHEN $3 THEN now() - interval '1 minute' \
                          ELSE now() + interval '1 hour' END, \
                     $4,'SELECT 1','not_null') RETURNING id",
        )
        .bind(ws)
        .bind(name)
        .bind(due)
        .bind(database)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_due_schedule_is_claimed_and_a_future_one_is_not(pool: sqlx::PgPool) {
        let db = Db { pool };
        let due = seed(&db, "due", true).await;
        seed(&db, "later", false).await;

        let claimed = claim_due(&db.pool, 10).await.unwrap();
        assert_eq!(claimed.len(), 1, "only the due schedule");
        assert_eq!(claimed[0].id, due);
        assert_eq!(
            claimed[0].last_state.as_deref(),
            Some("working"),
            "claiming marks the row so no other replica takes it"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_trashed_schedule_is_never_claimed(pool: sqlx::PgPool) {
        // Trash is a soft delete, so the row is still `active` with a due
        // `next_run_at` — nothing about it stops the ticker except the
        // `deleted_at IS NULL` term in the claim. Without that term a schedule
        // the user believes they deleted keeps firing and keeps emailing.
        let db = Db { pool };
        let live = seed(&db, "live", true).await;
        let binned = seed(&db, "binned", true).await;
        sqlx::query("UPDATE automation.schedules SET deleted_at = now() WHERE id = $1")
            .bind(binned)
            .execute(&db.pool)
            .await
            .unwrap();

        let claimed = claim_due(&db.pool, 10).await.unwrap();
        assert_eq!(
            claimed.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![live],
            "a trashed schedule must not be claimed"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn two_concurrent_claimers_never_take_the_same_row(pool: sqlx::PgPool) {
        // THE reason the claim uses FOR UPDATE SKIP LOCKED. The first claimer
        // holds its transaction open, so its lock is still live when the second
        // runs — exactly the overlap two replicas ticking together produce.
        let db = Db { pool };
        seed(&db, "contested", true).await;

        let mut tx = db.pool.begin().await.unwrap();
        let first = claim_due(&mut *tx, 10).await.unwrap();
        assert_eq!(first.len(), 1, "the first claimer takes the row");

        // Second claimer, on its own connection, while the first still holds
        // the lock.
        //
        // Wrapped in a timeout deliberately. Plain `FOR UPDATE` also ends up
        // returning nothing here — it BLOCKS until the first transaction
        // commits and then sees the row marked `working` — so an untimed
        // assertion passes with or without `SKIP LOCKED` and proves only that
        // the marker works. What `SKIP LOCKED` actually buys is that a busy row
        // costs this replica nothing: it must come back at once, not wait for
        // someone else's run to finish.
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            claim_due(&db.pool, 10),
        )
        .await
        .expect("a locked row must be SKIPPED immediately, not waited on")
        .unwrap();
        assert!(
            second.is_empty(),
            "a row locked by another claimer must not be double-run: {:?}",
            second.iter().map(|s| &s.name).collect::<Vec<_>>()
        );

        tx.commit().await.unwrap();

        // And once committed it stays claimed — `working` keeps it out of the
        // next tick even though no lock is held any more.
        let third = claim_due(&db.pool, 10).await.unwrap();
        assert!(third.is_empty(), "a working row is not re-claimed");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_working_row_past_its_timeout_is_taken_back(pool: sqlx::PgPool) {
        // The replica-died case: without this the row is stuck forever.
        let db = Db { pool };
        let id = seed(&db, "stuck", true).await;
        claim_due(&db.pool, 10).await.unwrap();
        assert!(claim_due(&db.pool, 10).await.unwrap().is_empty());

        // Backdate the claim past working_timeout_s (default 3600).
        sqlx::query(
            "UPDATE automation.schedules SET last_eval_at = now() - interval '2 hours' \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap();

        let retaken = claim_due(&db.pool, 10).await.unwrap();
        assert_eq!(retaken.len(), 1, "an abandoned claim must be recoverable");
        assert_eq!(retaken[0].id, id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_working_row_inside_its_timeout_is_left_alone(pool: sqlx::PgPool) {
        // The other half: the timeout must not be so eager that a slow but
        // healthy run gets a second runner.
        let db = Db { pool };
        let id = seed(&db, "slow", true).await;
        claim_due(&db.pool, 10).await.unwrap();
        sqlx::query(
            "UPDATE automation.schedules SET last_eval_at = now() - interval '5 minutes' \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap();

        assert!(
            claim_due(&db.pool, 10).await.unwrap().is_empty(),
            "a run 5 minutes into a 1 hour timeout is still running"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_inactive_schedule_is_never_claimed(pool: sqlx::PgPool) {
        let db = Db { pool };
        let id = seed(&db, "paused", true).await;
        sqlx::query("UPDATE automation.schedules SET active = false WHERE id = $1")
            .bind(id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(claim_due(&db.pool, 10).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn releasing_sets_the_next_run_so_the_row_is_not_re_claimed(pool: sqlx::PgPool) {
        let db = Db { pool };
        let id = seed(&db, "cycle", true).await;
        claim_due(&db.pool, 10).await.unwrap();

        let next = chrono::Utc::now() + chrono::Duration::hours(1);
        release(&db, id, "success", Some(3.0), Some(next))
            .await
            .unwrap();

        assert!(
            claim_due(&db.pool, 10).await.unwrap().is_empty(),
            "a released schedule with a future next run must not be claimed again"
        );
        let (state, value): (String, Option<f64>) =
            sqlx::query_as("SELECT last_state, last_value FROM automation.schedules WHERE id = $1")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(state, "success");
        assert_eq!(value, Some(3.0));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_release_with_no_next_run_would_be_re_claimed_immediately(pool: sqlx::PgPool) {
        // Documents WHY release recomputes the next run: leaving it in the past
        // is a hot loop, not a paused schedule.
        let db = Db { pool };
        let id = seed(&db, "hotloop", true).await;
        claim_due(&db.pool, 10).await.unwrap();
        release(
            &db,
            id,
            "success",
            None,
            Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
        )
        .await
        .unwrap();
        assert_eq!(
            claim_due(&db.pool, 10).await.unwrap().len(),
            1,
            "a next run left in the past means the row is due again at once"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn grace_starts_when_an_alert_fires_and_not_when_it_does_not(pool: sqlx::PgPool) {
        let db = Db { pool };
        let id = seed(&db, "graced", true).await;

        // A run that did not fire must not start a grace period.
        record_run(&db, id, "noop", None, None, false)
            .await
            .unwrap();
        assert!(
            !in_grace(&db, id, 3600).await.unwrap(),
            "a noop run must not suppress the next real trigger"
        );

        // One that fired but reached nobody must not start it either (KI-033):
        // nobody was told, so the next tick has to try again.
        record_run(&db, id, "error", Some(1.0), Some("webhook: refused"), false)
            .await
            .unwrap();
        assert!(
            !in_grace(&db, id, 3600).await.unwrap(),
            "a firing whose every destination failed must not silence the next one"
        );

        // One that reached someone does — even with another destination failing.
        let fired = record_run(&db, id, "error", Some(1.0), Some("slack: no token"), true)
            .await
            .unwrap();
        assert!(
            in_grace(&db, id, 3600).await.unwrap(),
            "a firing that reached a recipient starts the quiet period"
        );

        // ...and it expires. Backdated rather than slept on: a grace period is
        // measured against the stored timestamp, so moving the timestamp tests
        // the same arithmetic without making the suite wait.
        sqlx::query(
            "UPDATE automation.schedule_runs SET started_at = now() - interval '2 hours' \
             WHERE id = $1",
        )
        .bind(fired)
        .execute(&db.pool)
        .await
        .unwrap();
        assert!(
            !in_grace(&db, id, 3600).await.unwrap(),
            "a notification 2 hours ago is outside a 1 hour grace period"
        );
        assert!(
            in_grace(&db, id, 10800).await.unwrap(),
            "...but inside a 3 hour one"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_zero_grace_period_never_suppresses(pool: sqlx::PgPool) {
        let db = Db { pool };
        let id = seed(&db, "nograce", true).await;
        record_run(&db, id, "success", Some(1.0), None, true)
            .await
            .unwrap();
        assert!(!in_grace(&db, id, 0).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn every_terminal_state_lands_in_the_run_log(pool: sqlx::PgPool) {
        // The plan's warning: a scheduler test can pass because nothing was
        // due. Asserting on the run log — not on the absence of an error — is
        // what makes a silent no-op visible.
        let db = Db { pool };
        let id = seed(&db, "logged", true).await;
        for (state, err) in [
            ("success", None),
            ("noop", None),
            ("grace", None),
            ("error", Some("table not found")),
        ] {
            record_run(&db, id, state, None, err, false).await.unwrap();
        }
        let states: Vec<String> = sqlx::query_scalar(
            "SELECT state FROM automation.schedule_runs WHERE schedule_id = $1 \
             ORDER BY started_at, state",
        )
        .bind(id)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(states.len(), 4, "every path writes a row: {states:?}");
        for want in ["success", "noop", "grace", "error"] {
            assert!(states.iter().any(|s| s == want), "{want} missing");
        }
        let stored_error: Option<String> = sqlx::query_scalar(
            "SELECT error FROM automation.schedule_runs \
             WHERE schedule_id = $1 AND state = 'error'",
        )
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(stored_error.as_deref(), Some("table not found"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn pruning_removes_runs_past_their_retention_and_keeps_the_rest(pool: sqlx::PgPool) {
        let db = Db { pool };
        let id = seed(&db, "pruned", true).await;
        let old = record_run(&db, id, "success", None, None, false)
            .await
            .unwrap();
        let fresh = record_run(&db, id, "success", None, None, false)
            .await
            .unwrap();
        // log_retention_days defaults to 90.
        sqlx::query(
            "UPDATE automation.schedule_runs SET started_at = now() - interval '120 days' \
             WHERE id = $1",
        )
        .bind(old)
        .execute(&db.pool)
        .await
        .unwrap();

        assert_eq!(prune_runs(&db).await.unwrap(), 1);
        let left: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM automation.schedule_runs WHERE schedule_id = $1")
                .bind(id)
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(left, vec![fresh], "only the recent run survives");
    }
}
