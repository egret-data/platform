//! `automation.schedules` persistence.
//!
//! Every read and write is filtered by `workspace_id`, so a schedule id from
//! another workspace is invisible rather than merely un-writable — the same
//! no-cross-tenant-IDOR rule the rest of the content repos follow.

use std::collections::HashMap;

use chrono::Utc;
use egret_core::UserId;
use egret_metastore::Db;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::AutomationError;

#[derive(Clone)]
pub struct ScheduleRepo {
    db: Db,
}

/// A schedule row, as stored. The type-dependent columns are `Option` because
/// one table holds both alerts and reports; which ones must be present is
/// pinned by migration 0110's CHECK constraints.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Schedule {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub r#type: String,
    pub name: String,
    pub description: String,
    pub active: bool,
    pub crontab: String,
    pub timezone: String,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,

    pub database_id: Option<Uuid>,
    pub sql: Option<String>,
    pub validator_type: Option<String>,
    pub validator_config: serde_json::Value,
    pub last_eval_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_state: Option<String>,
    pub last_value: Option<f64>,

    pub chart_id: Option<Uuid>,
    pub dashboard_id: Option<Uuid>,
    pub report_format: Option<String>,
    pub custom_width: Option<i32>,
    pub custom_height: Option<i32>,

    pub grace_period_s: i32,
    pub working_timeout_s: i32,
    pub log_retention_days: i32,
    pub email_subject: Option<String>,

    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A schedule owner, joined to the user's email for display. The owner is a
/// security-relevant field: P2 runs the alert's query with this user's
/// permissions.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Owner {
    pub schedule_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Recipient {
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub kind: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduleRun {
    pub id: Uuid,
    pub state: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub value: Option<f64>,
    pub error: Option<String>,
    pub notification_sent_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The write shape shared by create and update: everything a caller may set,
/// already validated. `next_run_at` is computed by the repo from
/// `crontab`/`timezone` rather than accepted, so no caller can persist a row
/// whose claim key disagrees with its own expression.
#[derive(Debug, Clone)]
pub struct NewSchedule {
    pub r#type: String,
    pub name: String,
    pub description: String,
    pub active: bool,
    pub crontab: String,
    pub timezone: String,

    pub database_id: Option<Uuid>,
    pub sql: Option<String>,
    pub validator_type: Option<String>,
    pub validator_config: serde_json::Value,

    pub chart_id: Option<Uuid>,
    pub dashboard_id: Option<Uuid>,
    pub report_format: Option<String>,
    pub custom_width: Option<i32>,
    pub custom_height: Option<i32>,

    pub grace_period_s: i32,
    pub working_timeout_s: i32,
    pub log_retention_days: i32,
    pub email_subject: Option<String>,

    pub owners: Vec<Uuid>,
    pub recipients: Vec<(String, serde_json::Value)>,
}

const COLS: &str = "id, workspace_id, type, name, description, active, crontab, timezone, \
     next_run_at, database_id, sql, validator_type, validator_config, last_eval_at, last_state, \
     last_value, chart_id, dashboard_id, report_format, custom_width, custom_height, \
     grace_period_s, working_timeout_s, log_retention_days, email_subject, created_by, updated_at";

impl ScheduleRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert a schedule with its owners and recipients in ONE transaction.
    ///
    /// The three tables are one user-visible object: a schedule that exists
    /// with no owner has no identity to run its query as (§5.2), so a partial
    /// write is worse than no write at all.
    pub async fn create(
        &self,
        ws: Uuid,
        new: &NewSchedule,
        actor: Option<UserId>,
    ) -> Result<Schedule, AutomationError> {
        let next_run_at = crate::cron::next_run_after(&new.crontab, &new.timezone, Utc::now())?;

        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(AutomationError::from_db)?;
        let row: Schedule = sqlx::query_as(&format!(
            "INSERT INTO automation.schedules \
             (workspace_id, type, name, description, active, crontab, timezone, next_run_at, \
              database_id, sql, validator_type, validator_config, \
              chart_id, dashboard_id, report_format, custom_width, custom_height, \
              grace_period_s, working_timeout_s, log_retention_days, email_subject, \
              created_by, updated_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$22) \
             RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(&new.r#type)
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.active)
        .bind(&new.crontab)
        .bind(&new.timezone)
        .bind(next_run_at)
        .bind(new.database_id)
        .bind(&new.sql)
        .bind(&new.validator_type)
        .bind(&new.validator_config)
        .bind(new.chart_id)
        .bind(new.dashboard_id)
        .bind(&new.report_format)
        .bind(new.custom_width)
        .bind(new.custom_height)
        .bind(new.grace_period_s)
        .bind(new.working_timeout_s)
        .bind(new.log_retention_days)
        .bind(&new.email_subject)
        .bind(actor.map(|u| u.0))
        .fetch_one(&mut *tx)
        .await
        .map_err(AutomationError::from_db)?;

        write_children(&mut tx, row.id, new).await?;
        tx.commit().await.map_err(AutomationError::from_db)?;
        Ok(row)
    }

    /// Schedules in `ws`, optionally narrowed to one `type` (the two list tabs).
    pub async fn list(&self, ws: Uuid, ty: Option<&str>) -> Result<Vec<Schedule>, AutomationError> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM automation.schedules \
             WHERE workspace_id = $1 AND ($2::text IS NULL OR type = $2) \
             ORDER BY updated_at DESC"
        ))
        .bind(ws)
        .bind(ty)
        .fetch_all(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    pub async fn get(&self, ws: Uuid, id: Uuid) -> Result<Option<Schedule>, AutomationError> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM automation.schedules WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    /// Replace a schedule's mutable fields, owners and recipients.
    ///
    /// `type` is deliberately not updatable — see `UpdateSchedule`. Owners and
    /// recipients are REPLACED, not merged: the editor submits the full list,
    /// and merging would leave a removed recipient still receiving mail.
    pub async fn update(
        &self,
        ws: Uuid,
        id: Uuid,
        new: &NewSchedule,
        actor: Option<UserId>,
    ) -> Result<Option<Schedule>, AutomationError> {
        let next_run_at = crate::cron::next_run_after(&new.crontab, &new.timezone, Utc::now())?;

        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(AutomationError::from_db)?;
        let row: Option<Schedule> = sqlx::query_as(&format!(
            "UPDATE automation.schedules SET \
               name=$3, description=$4, active=$5, crontab=$6, timezone=$7, next_run_at=$8, \
               database_id=$9, sql=$10, validator_type=$11, validator_config=$12, \
               chart_id=$13, dashboard_id=$14, report_format=$15, custom_width=$16, \
               custom_height=$17, grace_period_s=$18, working_timeout_s=$19, \
               log_retention_days=$20, email_subject=$21, updated_by=$22, updated_at=now() \
             WHERE id=$1 AND workspace_id=$2 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(ws)
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.active)
        .bind(&new.crontab)
        .bind(&new.timezone)
        .bind(next_run_at)
        .bind(new.database_id)
        .bind(&new.sql)
        .bind(&new.validator_type)
        .bind(&new.validator_config)
        .bind(new.chart_id)
        .bind(new.dashboard_id)
        .bind(&new.report_format)
        .bind(new.custom_width)
        .bind(new.custom_height)
        .bind(new.grace_period_s)
        .bind(new.working_timeout_s)
        .bind(new.log_retention_days)
        .bind(&new.email_subject)
        .bind(actor.map(|u| u.0))
        .fetch_optional(&mut *tx)
        .await
        .map_err(AutomationError::from_db)?;

        // No row means the id belongs to another workspace (or nothing). Commit
        // an empty transaction rather than writing children for a schedule this
        // caller cannot see.
        let Some(row) = row else {
            tx.rollback().await.map_err(AutomationError::from_db)?;
            return Ok(None);
        };

        sqlx::query("DELETE FROM automation.schedule_owners WHERE schedule_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AutomationError::from_db)?;
        sqlx::query("DELETE FROM automation.schedule_recipients WHERE schedule_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AutomationError::from_db)?;
        write_children(&mut tx, id, new).await?;
        tx.commit().await.map_err(AutomationError::from_db)?;
        Ok(Some(row))
    }

    /// Flip only `active`, nothing else (P5's list-row toggle).
    ///
    /// A dedicated statement rather than a read-modify-write built on
    /// `update`: `update` REPLACES every field from a full body, so building
    /// a toggle out of it would mean reading the current row and writing it
    /// straight back with one field changed — a window in which a concurrent
    /// edit (crontab, recipients, SQL, …) could be silently discarded. This
    /// touches exactly the one column the toggle means to change.
    ///
    /// `next_run_at` is deliberately left untouched, in both directions:
    /// pausing does not need to change it (`claim_due` already requires
    /// `active`), and resuming does not recompute it either, so a schedule
    /// paused past its last computed slot becomes due again as soon as it is
    /// reactivated — the same "catch up once" behaviour a user gets from
    /// editing and re-saving it.
    pub async fn set_active(
        &self,
        ws: Uuid,
        id: Uuid,
        active: bool,
    ) -> Result<Option<Schedule>, AutomationError> {
        sqlx::query_as(&format!(
            "UPDATE automation.schedules SET active = $3, updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(ws)
        .bind(active)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    /// Returns whether a row was actually deleted, so the route can 404 on an
    /// id belonging to another workspace instead of reporting a false success.
    pub async fn delete(&self, ws: Uuid, id: Uuid) -> Result<bool, AutomationError> {
        let done = sqlx::query("DELETE FROM automation.schedules WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(ws)
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        Ok(done.rows_affected() > 0)
    }

    /// Owners for several schedules in one query — the list page shows an owner
    /// column, and a query per row is what this signature exists to prevent.
    pub async fn owners_for(
        &self,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<Owner>>, AutomationError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<Owner> = sqlx::query_as(
            "SELECT o.schedule_id, o.user_id, u.email \
             FROM automation.schedule_owners o JOIN users u ON u.id = o.user_id \
             WHERE o.schedule_id = ANY($1) ORDER BY u.email",
        )
        .bind(ids)
        .fetch_all(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;

        let mut out: HashMap<Uuid, Vec<Owner>> = HashMap::new();
        for row in rows {
            out.entry(row.schedule_id).or_default().push(row);
        }
        Ok(out)
    }

    pub async fn recipients_for(&self, id: Uuid) -> Result<Vec<Recipient>, AutomationError> {
        sqlx::query_as(
            "SELECT id, schedule_id, kind, config FROM automation.schedule_recipients \
             WHERE schedule_id = $1 ORDER BY kind, id",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    /// The run log for one schedule, newest first.
    ///
    /// Joined back to `schedules` and filtered by workspace: the drawer is
    /// reached through a schedule, so leaving the child unscoped would undo the
    /// 404 the parent gives a caller from another workspace.
    pub async fn runs(
        &self,
        ws: Uuid,
        id: Uuid,
        limit: i64,
    ) -> Result<Vec<ScheduleRun>, AutomationError> {
        sqlx::query_as(
            "SELECT r.id, r.state, r.started_at, r.ended_at, r.value, r.error, \
                    r.notification_sent_at \
             FROM automation.schedule_runs r \
             JOIN automation.schedules s ON s.id = r.schedule_id \
             WHERE r.schedule_id = $1 AND s.workspace_id = $2 \
             ORDER BY r.started_at DESC LIMIT $3",
        )
        .bind(id)
        .bind(ws)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }
}

/// Insert a schedule's owners and recipients inside the caller's transaction.
/// Shared by create and update so the two paths cannot drift in what they
/// consider a complete schedule.
async fn write_children(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
    new: &NewSchedule,
) -> Result<(), AutomationError> {
    for owner in &new.owners {
        sqlx::query(
            "INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(schedule_id)
        .bind(owner)
        .execute(&mut **tx)
        .await
        .map_err(AutomationError::from_db)?;
    }
    for (kind, config) in &new.recipients {
        sqlx::query(
            "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
             VALUES ($1,$2,$3)",
        )
        .bind(schedule_id)
        .bind(kind)
        .bind(config)
        .execute(&mut **tx)
        .await
        .map_err(AutomationError::from_db)?;
    }
    Ok(())
}
