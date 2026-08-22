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
    /// Where a webhook's HMAC signing secret actually lives. The recipient row
    /// stores only a `secret_id` reference — see [`ScheduleRepo::vault_secrets`].
    secrets: egret_secrets::SecretStore,
}

/// `secrets.kind` for a webhook signing secret, so a vault row can be traced
/// back to what issued it.
pub const WEBHOOK_SECRET_KIND: &str = "automation_webhook_secret";

/// `secrets.kind` for a Slack bot token.
pub const SLACK_TOKEN_KIND: &str = "automation_slack_token";

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

    // Data API alert only (migration 0127). Which of `database_id`/`sql` vs.
    // these four is populated is pinned by `schedule_dataapi_needs_source` /
    // `schedule_alert_no_dataapi_columns`.
    pub dataapi_endpoint_id: Option<Uuid>,
    pub dataapi_metric: Option<String>,
    pub dataapi_error_code: Option<String>,
    pub dataapi_window_s: Option<i32>,

    pub grace_period_s: i32,
    pub working_timeout_s: i32,
    pub log_retention_days: i32,
    pub email_subject: Option<String>,

    pub message: Option<String>,
    /// The closing note, under the result. `message` is the opening one.
    pub message_footer: Option<String>,
    pub skip_if_empty: bool,

    pub folder_id: Option<Uuid>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,

    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// `list`'s row: a schedule plus the viewer's bookmark flag, flattened so one
/// query answers both.
#[derive(sqlx::FromRow)]
struct ScheduleWithBookmark {
    #[sqlx(flatten)]
    schedule: Schedule,
    bookmarked: bool,
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

/// One object a report delivers. A report may have several; an alert has at
/// most one, and keeps it in the schedule's own columns.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Target {
    pub kind: String,
    pub object_id: Uuid,
    pub ordinal: i32,
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

    // Data API alert only (migration 0127). `None` throughout for an `alert`
    // or a `report` — `schedule_alert_no_dataapi_columns` forbids any of
    // these on a non-`dataapi_alert` row.
    pub dataapi_endpoint_id: Option<Uuid>,
    pub dataapi_metric: Option<String>,
    pub dataapi_error_code: Option<String>,
    pub dataapi_window_s: Option<i32>,

    pub grace_period_s: i32,
    pub working_timeout_s: i32,
    pub log_retention_days: i32,
    pub email_subject: Option<String>,

    pub message: Option<String>,
    /// The closing note, under the result. `message` is the opening one.
    pub message_footer: Option<String>,
    pub skip_if_empty: bool,

    pub owners: Vec<Uuid>,
    pub recipients: Vec<(String, serde_json::Value)>,
    /// Extra delivery targets, in page order. Empty means "use the single
    /// target in `chart_id`/`dashboard_id`", which is every pre-0115 row.
    pub targets: Vec<(String, Uuid)>,
}

const COLS: &str = "id, workspace_id, type, name, description, active, crontab, timezone, \
     next_run_at, database_id, sql, validator_type, validator_config, last_eval_at, last_state, \
     last_value, chart_id, dashboard_id, report_format, custom_width, custom_height, \
     dataapi_endpoint_id, dataapi_metric, dataapi_error_code, dataapi_window_s, \
     grace_period_s, working_timeout_s, log_retention_days, email_subject, message, message_footer, \
     skip_if_empty, folder_id, deleted_at, created_by, updated_at";

impl ScheduleRepo {
    pub fn new(db: Db, secrets: egret_secrets::SecretStore) -> Self {
        Self { db, secrets }
    }

    /// Move any cleartext webhook secret into the vault, returning the configs
    /// to store and the vault rows the write leaves behind.
    ///
    /// A webhook's signing secret is a credential, so it follows the same rule
    /// as the SMTP password (P3): the row holds a `secret_id` reference and the
    /// value never appears in a column, a response, or a log line. Three-way,
    /// like SMTP's, because the editor is never sent the secret and so cannot
    /// render it:
    ///
    /// - `secret` absent → keep whatever is stored for that URL;
    /// - `secret: ""` → remove it;
    /// - `secret: "…"` → replace it.
    ///
    /// The stored config is REBUILT from `url` + `secret_id` rather than
    /// patched, so a client cannot smuggle extra keys — including a cleartext
    /// `secret` — into the row by sending them alongside.
    async fn vault_secrets(
        &self,
        existing: Option<Uuid>,
        recipients: &[(String, serde_json::Value)],
    ) -> Result<(Vec<(String, serde_json::Value)>, Vec<Uuid>), AutomationError> {
        // url -> stored secret id, the lookup behind "absent means keep".
        let mut stored: HashMap<String, Uuid> = HashMap::new();
        if let Some(id) = existing {
            for r in self.recipients_for(id).await? {
                // Keyed by the destination's own identity — a webhook by URL,
                // a slack recipient by channel — so "absent means keep" can
                // find the credential that belongs to it.
                let (key, secret) = match r.kind.as_str() {
                    "webhook" => (r.config.get("url"), r.config.get("secret_id")),
                    "slack" => (r.config.get("channel"), r.config.get("token_id")),
                    _ => continue,
                };
                let key = key.and_then(|v| v.as_str());
                let sid = secret
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());
                if let (Some(key), Some(sid)) = (key, sid) {
                    stored.insert(key.to_string(), sid);
                }
            }
        }

        let mut out = Vec::with_capacity(recipients.len());
        let mut kept: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for (kind, config) in recipients {
            // Slack's bot token is a credential exactly like a webhook signing
            // secret: the row keeps a reference, never the value. Keyed by
            // channel, so "absent means keep" works the same way.
            if kind == "slack" {
                let channel = config
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let token_id = match config.get("token").and_then(|v| v.as_str()) {
                    Some("") => None,
                    Some(t) => Some(
                        self.secrets
                            .put(SLACK_TOKEN_KIND, t)
                            .await
                            .map_err(|e| AutomationError::BadRequest(e.to_string()))?,
                    ),
                    None => stored.get(&channel).copied(),
                };
                if let Some(id) = token_id {
                    kept.insert(id);
                }
                let mut cfg = serde_json::Map::new();
                cfg.insert("channel".into(), serde_json::Value::String(channel));
                if let Some(id) = token_id {
                    cfg.insert("token_id".into(), serde_json::Value::String(id.to_string()));
                }
                out.push((kind.clone(), serde_json::Value::Object(cfg)));
                continue;
            }
            if kind != "webhook" {
                out.push((kind.clone(), config.clone()));
                continue;
            }
            let url = config
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let secret_id = match config.get("secret").and_then(|v| v.as_str()) {
                Some("") => None,
                Some(s) => Some(
                    self.secrets
                        .put(WEBHOOK_SECRET_KIND, s)
                        .await
                        .map_err(|e| AutomationError::BadRequest(e.to_string()))?,
                ),
                None => stored.get(&url).copied(),
            };
            if let Some(id) = secret_id {
                kept.insert(id);
            }
            let mut cfg = serde_json::Map::new();
            cfg.insert("url".into(), serde_json::Value::String(url));
            if let Some(id) = secret_id {
                cfg.insert(
                    "secret_id".into(),
                    serde_json::Value::String(id.to_string()),
                );
            }
            out.push((kind.clone(), serde_json::Value::Object(cfg)));
        }

        // Anything the schedule referenced and no longer does — a rotated
        // secret, or a webhook that was removed outright. A superseded
        // credential left in the vault is one nobody is watching.
        let orphans = stored
            .values()
            .copied()
            .filter(|id| !kept.contains(id))
            .collect();
        Ok((out, orphans))
    }

    /// The one place a webhook signing secret becomes plaintext, called at
    /// delivery time by the runner.
    pub async fn resolve_webhook_secret(&self, config: &serde_json::Value) -> Option<String> {
        self.resolve_secret(config, "secret_id").await
    }

    /// The Slack bot token for one recipient, from the vault.
    pub async fn resolve_slack_token(&self, config: &serde_json::Value) -> Option<String> {
        self.resolve_secret(config, "token_id").await
    }

    async fn resolve_secret(&self, config: &serde_json::Value, key: &str) -> Option<String> {
        let id = config
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())?;
        match self.secrets.get(id).await {
            Ok(s) => Some(s),
            Err(e) => {
                // Never fall back to sending unsigned: a receiver verifying the
                // signature would reject it anyway, and a silent downgrade is
                // the failure mode this whole change exists to prevent.
                tracing::warn!(secret = %id, error = %e, "automation: webhook secret unreadable");
                None
            }
        }
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
        // Nothing stored yet, so there is no secret to carry forward.
        let (recipients, _) = self.vault_secrets(None, &new.recipients).await?;

        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(AutomationError::from_db)?;
        let row: Schedule = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO automation.schedules \
             (workspace_id, type, name, description, active, crontab, timezone, next_run_at, \
              database_id, sql, validator_type, validator_config, \
              chart_id, dashboard_id, report_format, custom_width, custom_height, \
              dataapi_endpoint_id, dataapi_metric, dataapi_error_code, dataapi_window_s, \
              grace_period_s, working_timeout_s, log_retention_days, email_subject, \
              message, message_footer, skip_if_empty, created_by, updated_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,\
                      $22,$23,$24,$25,$26,$27,$28,$29,$29) \
             RETURNING {COLS}"
        )))
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
        .bind(new.dataapi_endpoint_id)
        .bind(&new.dataapi_metric)
        .bind(&new.dataapi_error_code)
        .bind(new.dataapi_window_s)
        .bind(new.grace_period_s)
        .bind(new.working_timeout_s)
        .bind(new.log_retention_days)
        .bind(&new.email_subject)
        .bind(&new.message)
        .bind(&new.message_footer)
        .bind(new.skip_if_empty)
        .bind(actor.map(|u| u.0))
        .fetch_one(&mut *tx)
        .await
        .map_err(AutomationError::from_db)?;

        write_children(&mut tx, row.id, &new.owners, &recipients, &new.targets).await?;
        tx.commit().await.map_err(AutomationError::from_db)?;
        Ok(row)
    }

    /// Schedules in `ws`, optionally narrowed to one `type` (the two list tabs).
    /// Schedules in `ws`, optionally narrowed to one `type` (the two list tabs).
    ///
    /// `trashed` selects the trash bin instead of the live list, and the
    /// per-user `bookmarked` flag rides along so the star column costs no extra
    /// query — the same shape `ChartRepo::list` uses.
    pub async fn list(
        &self,
        ws: Uuid,
        ty: Option<&str>,
        viewer: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Schedule, bool)>, AutomationError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        // AssertSqlSafe: `COLS` is a module const and `pred` is one of the two
        // `'static` literals chosen just above. Every value is bound.
        let rows: Vec<ScheduleWithBookmark> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS}, \
             EXISTS (SELECT 1 FROM analytics.bookmarks b \
                      WHERE b.user_id = $3 AND b.entity_type = 'alert' \
                        AND b.entity_id = schedules.id) AS bookmarked \
             FROM automation.schedules \
             WHERE workspace_id = $1 AND ($2::text IS NULL OR type = $2) AND {pred} \
             ORDER BY updated_at DESC"
        )))
        .bind(ws)
        .bind(ty)
        .bind(viewer)
        .fetch_all(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;
        Ok(rows
            .into_iter()
            .map(|r| (r.schedule, r.bookmarked))
            .collect())
    }

    /// Soft-delete (`trashed = true`) or restore. Scoped to the workspace, so a
    /// foreign id is a no-op rather than a cross-tenant write.
    pub async fn set_trashed(
        &self,
        ws: Uuid,
        id: Uuid,
        trashed: bool,
    ) -> Result<bool, AutomationError> {
        let sql = if trashed {
            "UPDATE automation.schedules SET deleted_at = now() WHERE id=$1 AND workspace_id=$2"
        } else {
            "UPDATE automation.schedules SET deleted_at = NULL WHERE id=$1 AND workspace_id=$2"
        };
        let done = sqlx::query(sql)
            .bind(id)
            .bind(ws)
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn set_folder(
        &self,
        ws: Uuid,
        id: Uuid,
        folder: Option<Uuid>,
    ) -> Result<bool, AutomationError> {
        let done = sqlx::query(
            "UPDATE automation.schedules SET folder_id=$3 WHERE id=$1 AND workspace_id=$2",
        )
        .bind(id)
        .bind(ws)
        .bind(folder)
        .execute(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;
        Ok(done.rows_affected() > 0)
    }

    /// Extra targets for one schedule, in page order.
    pub async fn targets_for(&self, id: Uuid) -> Result<Vec<Target>, AutomationError> {
        sqlx::query_as(
            "SELECT kind, object_id, ordinal FROM automation.schedule_targets \
             WHERE schedule_id = $1 ORDER BY ordinal, kind",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    pub async fn add_bookmark(&self, user: Uuid, id: Uuid) -> Result<(), AutomationError> {
        sqlx::query(
            "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) \
             VALUES ($1,'alert',$2) ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user)
        .bind(id)
        .execute(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;
        Ok(())
    }

    pub async fn remove_bookmark(&self, user: Uuid, id: Uuid) -> Result<(), AutomationError> {
        sqlx::query(
            "DELETE FROM analytics.bookmarks \
             WHERE user_id=$1 AND entity_type='alert' AND entity_id=$2",
        )
        .bind(user)
        .bind(id)
        .execute(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;
        Ok(())
    }

    pub async fn get(&self, ws: Uuid, id: Uuid) -> Result<Option<Schedule>, AutomationError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM automation.schedules WHERE id = $1 AND workspace_id = $2"
        )))
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
        let row: Option<Schedule> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE automation.schedules SET \
               name=$3, description=$4, active=$5, crontab=$6, timezone=$7, next_run_at=$8, \
               database_id=$9, sql=$10, validator_type=$11, validator_config=$12, \
               chart_id=$13, dashboard_id=$14, report_format=$15, custom_width=$16, \
               custom_height=$17, dataapi_endpoint_id=$18, dataapi_metric=$19, \
               dataapi_error_code=$20, dataapi_window_s=$21, grace_period_s=$22, \
               working_timeout_s=$23, log_retention_days=$24, email_subject=$25, message=$26, \
               message_footer=$27, skip_if_empty=$28, updated_by=$29, updated_at=now() \
             WHERE id=$1 AND workspace_id=$2 RETURNING {COLS}"
        )))
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
        .bind(new.dataapi_endpoint_id)
        .bind(&new.dataapi_metric)
        .bind(&new.dataapi_error_code)
        .bind(new.dataapi_window_s)
        .bind(new.grace_period_s)
        .bind(new.working_timeout_s)
        .bind(new.log_retention_days)
        .bind(&new.email_subject)
        .bind(&new.message)
        .bind(&new.message_footer)
        .bind(new.skip_if_empty)
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

        // After the visibility check, so a caller who cannot see this schedule
        // never causes a vault write.
        let (recipients, orphans) = self.vault_secrets(Some(id), &new.recipients).await?;

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
        sqlx::query("DELETE FROM automation.schedule_targets WHERE schedule_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AutomationError::from_db)?;
        write_children(&mut tx, id, &new.owners, &recipients, &new.targets).await?;
        tx.commit().await.map_err(AutomationError::from_db)?;

        // Only once the row committed: had the write failed, the old secret is
        // still the live one.
        for sid in orphans {
            let _ = self.secrets.delete(sid).await;
        }
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
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE automation.schedules SET active = $3, updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 RETURNING {COLS}"
        )))
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
        // Read the vault references BEFORE the cascade removes the rows that
        // name them, or the secrets outlive the schedule with nothing left
        // pointing at them.
        let secret_ids: Vec<Uuid> = self
            .recipients_for(id)
            .await?
            .iter()
            .filter_map(|r| r.config.get("secret_id").and_then(|v| v.as_str()))
            .filter_map(|s| Uuid::parse_str(s).ok())
            .collect();

        let done = sqlx::query("DELETE FROM automation.schedules WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(ws)
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        let deleted = done.rows_affected() > 0;
        if deleted {
            for sid in secret_ids {
                let _ = self.secrets.delete(sid).await;
            }
        }
        Ok(deleted)
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
/// `recipients` are the VAULTED configs from [`ScheduleRepo::vault_secrets`],
/// passed separately from `owners` so a caller cannot accidentally write the
/// raw, still-cleartext ones off `NewSchedule`.
async fn write_children(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
    owners: &[Uuid],
    recipients: &[(String, serde_json::Value)],
    targets: &[(String, Uuid)],
) -> Result<(), AutomationError> {
    for owner in owners {
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
    for (i, (kind, object_id)) in targets.iter().enumerate() {
        sqlx::query(
            "INSERT INTO automation.schedule_targets (schedule_id, kind, object_id, ordinal) \
             VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        )
        .bind(schedule_id)
        .bind(kind)
        .bind(object_id)
        .bind(i as i32)
        .execute(&mut **tx)
        .await
        .map_err(AutomationError::from_db)?;
    }
    for (i, (kind, object_id)) in targets.iter().enumerate() {
        sqlx::query(
            "INSERT INTO automation.schedule_targets (schedule_id, kind, object_id, ordinal) \
             VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        )
        .bind(schedule_id)
        .bind(kind)
        .bind(object_id)
        .bind(i as i32)
        .execute(&mut **tx)
        .await
        .map_err(AutomationError::from_db)?;
    }
    for (kind, config) in recipients {
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
