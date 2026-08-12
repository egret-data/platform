//! The tick: claim what is due, run it, write down what happened.
//!
//! Every path through `run_one` ends in a run-log row and a release. That is
//! not tidiness — the run log is the only answer to "why didn't I get an
//! email?", and a path that writes nothing is indistinguishable from a
//! scheduler that never ran at all. A schedule left claimed (`working`) with no
//! release is worse still: it is invisible until its timeout expires.

use std::sync::Arc;
use std::time::Duration;

use egret_analytics_api::engine_token::EngineTokenMinter;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_query::DatabaseRepo;
use tokio_util::sync::CancellationToken;

use crate::deliver::destination::{Resolver, SsrfPolicy};
use crate::deliver::webhook::{self, WebhookPayload};
use crate::evaluator::{evaluate, owner_identity, EvalError, QueryRunner};
use crate::repo::{Schedule, ScheduleRepo};
use crate::scheduler::{claim_due, in_grace, prune_runs, record_run, release};
use egret_automation_api::{RecipientKind, ValidatorType};

/// How many schedules one tick will take. A cap, not a target: it bounds how
/// much work a single replica commits to before the next tick can rebalance.
const BATCH: i64 = 20;

pub struct AutomationRunner {
    pub db: Db,
    pub schedules: ScheduleRepo,
    pub databases: DatabaseRepo,
    pub users: UserRepo,
    pub minter: Arc<dyn EngineTokenMinter>,
    pub query: Arc<dyn QueryRunner>,
    pub resolver: Arc<dyn Resolver>,
    pub policy: SsrfPolicy,
}

impl AutomationRunner {
    /// Claim and run everything currently due. Returns how many were processed.
    pub async fn tick_once(&self) -> usize {
        let due = match claim_due(&self.db.pool, BATCH).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(error = %e, "automation: claim failed");
                return 0;
            }
        };
        let n = due.len();
        for s in due {
            // One schedule's failure must not abandon the rest of the batch,
            // and `run_one` already records its own outcome.
            self.run_one(s).await;
        }
        if let Err(e) = prune_runs(&self.db).await {
            tracing::warn!(error = %e, "automation: pruning run log failed");
        }
        n
    }

    /// Run one claimed schedule to a terminal state.
    async fn run_one(&self, s: Schedule) {
        let outcome = self.evaluate_and_deliver(&s).await;
        let (state, value, error) = match outcome {
            Ok(Outcome::Fired { value, delivered }) => ("success", value, delivered.err()),
            Ok(Outcome::NotFired { value }) => ("noop", value, None),
            Ok(Outcome::Suppressed { value }) => ("grace", value, None),
            Err(e) => ("error", None, Some(e.to_string())),
        };
        let notified = state == "success" && error.is_none();

        if let Err(e) = record_run(&self.db, s.id, state, value, error.as_deref(), notified).await {
            tracing::error!(schedule = %s.id, error = %e, "automation: run log write failed");
        }
        // The next run is computed even on the error path: an alert that fails
        // must keep trying on its schedule, not stop dead. Leaving it unreleased
        // would strand the row in `working` until its timeout.
        let next = crate::cron::next_run_after(&s.crontab, &s.timezone, chrono::Utc::now()).ok();
        if let Err(e) = release(&self.db, s.id, state, value, next).await {
            tracing::error!(schedule = %s.id, error = %e, "automation: release failed");
        }
    }

    async fn evaluate_and_deliver(&self, s: &Schedule) -> Result<Outcome, EvalError> {
        let (Some(database_id), Some(sql), Some(validator)) = (
            s.database_id,
            s.sql.as_deref(),
            s.validator_type.as_deref().and_then(ValidatorType::parse),
        ) else {
            return Err(EvalError::NotRunnable("alert is missing its query"));
        };

        // The owner is the identity everything downstream runs as (§5.2).
        let owners = self
            .schedules
            .owners_for(&[s.id])
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?;
        let owner = owners
            .get(&s.id)
            .and_then(|o| o.first())
            .map(|o| o.user_id)
            .ok_or(EvalError::NoOwner)?;
        let identity = owner_identity(
            owner,
            database_id,
            &self.databases,
            &self.users,
            &*self.minter,
        )
        .await?;

        let eval = evaluate(
            &*self.query,
            database_id,
            sql,
            validator,
            &s.validator_config,
            &identity,
        )
        .await?;

        if !eval.fired {
            return Ok(Outcome::NotFired { value: eval.value });
        }
        // Fired — but recently enough that the user has already been told.
        if in_grace(&self.db, s.id, s.grace_period_s)
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?
        {
            return Ok(Outcome::Suppressed { value: eval.value });
        }

        let delivered = self.deliver(s, &eval).await;
        Ok(Outcome::Fired {
            value: eval.value,
            delivered,
        })
    }

    /// Send to every webhook recipient. Email waits for P3's SMTP config.
    ///
    /// Returns the first failure, if any: the run is still `success` (the alert
    /// DID fire, and the grace period starts) but the error is recorded so a
    /// silently-unreachable webhook is visible in the run log.
    async fn deliver(
        &self,
        s: &Schedule,
        eval: &crate::evaluator::Evaluation,
    ) -> Result<(), String> {
        let recipients = self
            .schedules
            .recipients_for(s.id)
            .await
            .map_err(|e| e.to_string())?;
        let mut first_error = None;
        for r in recipients {
            if RecipientKind::parse(&r.kind) != Some(RecipientKind::Webhook) {
                continue; // email is P3
            }
            let Some(url) = r.config.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            let secret = r.config.get("secret").and_then(|v| v.as_str());
            let payload = WebhookPayload {
                schedule: &s.name,
                schedule_id: &s.id.to_string(),
                ty: &s.r#type,
                state: "success",
                value: eval.value,
                triggered_at: chrono::Utc::now(),
                rows: eval.rows.clone(),
            };
            if let Err(e) =
                webhook::deliver(url, secret, &payload, &self.policy, &*self.resolver).await
            {
                tracing::warn!(schedule = %s.id, error = %e, "automation: webhook failed");
                first_error.get_or_insert(e.to_string());
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

enum Outcome {
    Fired {
        value: Option<f64>,
        delivered: Result<(), String>,
    },
    NotFired {
        value: Option<f64>,
    },
    Suppressed {
        value: Option<f64>,
    },
}

/// Spawn the ticker; returns immediately. Cancelled via `cancel`, exactly like
/// `egret-kernel`'s reaper — one periodic task per replica, no leader election,
/// with `SKIP LOCKED` in the claim making that safe.
pub fn spawn(runner: Arc<AutomationRunner>, every: Duration, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        // A replica that pauses (or a laptop that sleeps) must not then fire a
        // burst of catch-up ticks — the schedules are still there and the next
        // tick will find them.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => { runner.tick_once().await; }
            }
        }
    });
}
