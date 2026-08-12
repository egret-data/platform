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
use crate::deliver::email::{self, Attachment, EmailMessage};
use crate::deliver::smtp_config::SmtpConfigRepo;
use crate::deliver::webhook::{self, WebhookPayload};
use crate::evaluator::{evaluate, owner_identity, EvalError, QueryRunner};
use crate::model::report_target;
use crate::render::client::{RenderRequest, Renderer};
use crate::render::token::RenderTokenRepo;
use crate::repo::{Schedule, ScheduleRepo};
use crate::scheduler::{claim_due, in_grace, prune_runs, record_run, release};
use egret_automation_api::{RecipientKind, ReportFormat, ValidatorType};

/// How many schedules one tick will take. A cap, not a target: it bounds how
/// much work a single replica commits to before the next tick can rebalance.
const BATCH: i64 = 20;

/// How long a render token lives (design §5: "about a minute"). Long enough
/// for the renderer to receive the request and the print page to fetch the
/// payload; short enough that a leaked token is useless within the same
/// coffee break. Every mint uses a FRESH token — `RenderTokenRepo::redeem` is
/// destructive on read, so reusing one across a retry would 404 the second
/// attempt.
const RENDER_TOKEN_TTL: Duration = Duration::from_secs(60);

/// Sizing when a schedule sets no `custom_width`/`custom_height` (design §6).
fn default_size(kind: &str) -> (u32, u32) {
    if kind == "dashboard" {
        (1200, 800)
    } else {
        (800, 600)
    }
}

pub struct AutomationRunner {
    pub db: Db,
    pub schedules: ScheduleRepo,
    pub databases: DatabaseRepo,
    pub users: UserRepo,
    pub minter: Arc<dyn EngineTokenMinter>,
    pub query: Arc<dyn QueryRunner>,
    pub resolver: Arc<dyn Resolver>,
    pub policy: SsrfPolicy,
    pub smtp: SmtpConfigRepo,
    /// `csv`/`text` reports execute the chart's own saved spec — never the
    /// renderer — through the same `compile_chart_sql` seam the render-payload
    /// endpoint uses (`render::payload`), so a report can never diverge from
    /// what the interactive chart would show.
    pub charts: egret_bi::ChartRepo,
    pub datasets: egret_bi::DatasetRepo,
    /// Per-render credentials (P4): mints the single-use token a `png`/`pdf`
    /// report's renderer call authenticates with.
    pub render_tokens: RenderTokenRepo,
    /// The `egret-renderer` client. A trait object so a test can fake it,
    /// exactly like `query: Arc<dyn QueryRunner>` above.
    pub renderer: Arc<dyn Renderer>,
    /// Base URL the RENDERER can reach to fetch `/embed/print/...` — an
    /// in-cluster Service name, not necessarily this server's user-facing
    /// hostname. See `AutomationConfig::public_url`.
    pub public_url: String,
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
        // A render token outlives its usefulness within ~60s of being minted
        // (§5) — nothing else deletes one that was minted for a render that
        // never happened (a crash between mint and redeem, say).
        if let Err(e) = self.render_tokens.prune_expired().await {
            tracing::warn!(error = %e, "automation: pruning render tokens failed");
        }
        n
    }

    /// Run one claimed schedule to a terminal state.
    async fn run_one(&self, s: Schedule) {
        let (state, value, _error) = self.run_and_log(&s).await;
        // The next run is computed even on the error path: an alert that fails
        // must keep trying on its schedule, not stop dead. Leaving it unreleased
        // would strand the row in `working` until its timeout.
        let next = crate::cron::next_run_after(&s.crontab, &s.timezone, chrono::Utc::now()).ok();
        if let Err(e) = release(&self.db, s.id, &state, value, next).await {
            tracing::error!(schedule = %s.id, error = %e, "automation: release failed");
        }
    }

    /// Evaluate+deliver `s` and append the resulting run-log row. Shared by
    /// the tick (`run_one`, above — which then also `release`s the claim and
    /// advances `next_run_at`) and manual "Run now" (`run_now`, below — which
    /// does neither). This is the ONE place that turns an
    /// `evaluate_and_deliver` outcome into a run-log row, so a manual run and
    /// a scheduled run can never record the same outcome differently.
    async fn run_and_log(&self, s: &Schedule) -> (String, Option<f64>, Option<String>) {
        let outcome = self.evaluate_and_deliver(s).await;
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
        (state.to_string(), value, error)
    }

    /// Run one schedule immediately — the "Run now" button (P5). Bypasses the
    /// scheduler queue entirely: unlike `run_one`, it never claims the row
    /// (no `last_state = 'working'`) and never calls `release`, so
    /// `next_run_at` is left exactly as it was. That is deliberate: a manual
    /// run must not cause the schedule's own cadence to skip a slot (because
    /// `release` moved `next_run_at` forward) or double-fire one (because the
    /// row was claimed and instantly re-releasable, racing a concurrent
    /// tick). The tradeoff is that a manual run can run concurrently with a
    /// tick claiming the same schedule — acceptable, because the only thing
    /// that must never happen is corrupting the schedule's own timing, and
    /// this path never touches it.
    ///
    /// Runs through the exact same `evaluate_and_deliver` the tick uses (via
    /// `run_and_log` above), so a manual run and a scheduled run can never
    /// evaluate an alert differently — including the grace-period check
    /// inside it.
    ///
    /// DELIBERATE CHOICE: a manual run does NOT bypass the grace period. If
    /// the alert already fired (and notified) within its grace window,
    /// "Run now" records `grace` and sends nothing — exactly what the tick
    /// would do right now. Two reasons this is the right default rather than
    /// a bypass flag threaded into the evaluator:
    /// (1) a bypass is a second evaluation path in disguise, which is exactly
    ///     what this function exists to avoid — "Run now" is supposed to
    ///     answer "what would happen right now", and grace suppression IS
    ///     part of that answer;
    /// (2) silently skipping the anti-spam guard because the user clicked a
    ///     button would let someone who mashes "Run now" a few times in a row
    ///     spam every recipient once per click, which is the exact failure
    ///     mode the grace period exists to prevent.
    /// The `grace` state still lands in the run log, so the UI can show the
    /// user WHY nothing was sent rather than the button silently doing
    /// nothing.
    pub async fn run_now(&self, s: &Schedule) {
        self.run_and_log(s).await;
    }

    async fn evaluate_and_deliver(&self, s: &Schedule) -> Result<Outcome, EvalError> {
        // Reports have no condition to evaluate — a report that renders and
        // delivers always "fires" — so they branch off before any of the
        // alert's query/validator machinery below.
        if s.r#type == "report" {
            return self.render_and_deliver(s).await;
        }

        let (Some(database_id), Some(sql), Some(validator)) = (
            s.database_id,
            s.sql.as_deref(),
            s.validator_type.as_deref().and_then(ValidatorType::parse),
        ) else {
            return Err(EvalError::NotRunnable("alert is missing its query"));
        };

        // The owner is the identity everything downstream runs as (§5.2).
        let owner = self.resolve_owner(s.id).await?;
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
        // Opted-in silence: the condition held but the query returned nothing
        // to show, so there is no mail worth sending. Recorded as `noop` — the
        // run log still says it ran, which is the difference between this and
        // a scheduler that quietly stopped.
        if s.skip_if_empty && eval.rows.as_array().is_none_or(|r| r.is_empty()) {
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

    /// The identity everything downstream runs as (§5.2) — shared by the
    /// alert and report branches so there is exactly one place that decides
    /// who a schedule runs as, not two that could drift.
    async fn resolve_owner(&self, schedule_id: uuid::Uuid) -> Result<uuid::Uuid, EvalError> {
        let owners = self
            .schedules
            .owners_for(&[schedule_id])
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?;
        owners
            .get(&schedule_id)
            .and_then(|o| o.first())
            .map(|o| o.user_id)
            .ok_or(EvalError::NoOwner)
    }

    // ── Reports (P4) ─────────────────────────────────────────────────────

    /// The report branch of the tick. Reports have no condition — rendering
    /// and delivering IS the outcome — so this always returns `Fired` on
    /// success; a renderer/query failure surfaces as `Err`, which `run_one`
    /// records as `error` and still reschedules (§8 of the design).
    async fn render_and_deliver(&self, s: &Schedule) -> Result<Outcome, EvalError> {
        let owner = self.resolve_owner(s.id).await?;
        let format = s
            .report_format
            .as_deref()
            .and_then(ReportFormat::parse)
            .ok_or(EvalError::NotRunnable("report is missing its format"))?;

        let content = match format {
            // Data the backend already has — never touches the renderer.
            ReportFormat::Csv | ReportFormat::Text => {
                self.build_chart_report(s, owner, format).await?
            }
            ReportFormat::Png | ReportFormat::Pdf => {
                self.render_image_report(s, owner, format).await?
            }
        };

        let delivered = self.deliver_report(s, &content).await;
        Ok(Outcome::Fired {
            value: None,
            delivered,
        })
    }

    /// `png`/`pdf`: mint a single-use token for the schedule's target, ask
    /// `egret-renderer` to capture `/embed/print/{kind}/{id}?t=<token>`, and
    /// attach the bytes it returns.
    ///
    /// A FRESH token every call — never reused across a retry. `redeem` is
    /// DELETE-on-read (see `render::token`), so a second attempt with the
    /// same token would 404 at the print page rather than re-render.
    async fn render_image_report(
        &self,
        s: &Schedule,
        owner: uuid::Uuid,
        format: ReportFormat,
    ) -> Result<ReportContent, EvalError> {
        // Several targets, in page order — or the single one in the schedule's
        // own columns, which is every report written before 0115.
        let stored = self
            .schedules
            .targets_for(s.id)
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?;
        let targets: Vec<(&'static str, uuid::Uuid)> = if stored.is_empty() {
            report_target(s)
                .map(|t| vec![t])
                .ok_or(EvalError::NotRunnable("report has no chart or dashboard"))?
        } else {
            stored
                .iter()
                .map(|t| {
                    let kind: &'static str = if t.kind == "chart" {
                        "chart"
                    } else {
                        "dashboard"
                    };
                    (kind, t.object_id)
                })
                .collect()
        };

        // One token PER object: a render token is bound to a single object by
        // design, so N pages need N of them.
        let mut urls = Vec::with_capacity(targets.len());
        for (kind, object_id) in &targets {
            let token = self
                .render_tokens
                .mint(kind, *object_id, owner, RENDER_TOKEN_TTL)
                .await
                .map_err(|e| EvalError::Query(e.to_string()))?;
            urls.push(format!(
                "{}/embed/print/{kind}/{object_id}?t={token}",
                self.public_url.trim_end_matches('/')
            ));
        }
        let (kind, object_id) = targets[0];
        let url = urls[0].clone();
        // A PNG has no second page. Rather than send one object of several and
        // let the recipient believe that is the whole report, several targets
        // force the multi-page format.
        let format = if targets.len() > 1 {
            ReportFormat::Pdf
        } else {
            format
        };
        let (width, height) = match (s.custom_width, s.custom_height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as u32, h as u32),
            _ => default_size(kind),
        };
        let bytes = self
            .renderer
            .render(RenderRequest {
                url,
                urls: if targets.len() > 1 { urls } else { Vec::new() },
                width,
                height,
                format: format.as_str().to_string(),
                scale: 2,
            })
            .await
            .map_err(EvalError::Render)?;

        let (ext, content_type) = match format {
            ReportFormat::Png => ("png", "image/png"),
            ReportFormat::Pdf => ("pdf", "application/pdf"),
            ReportFormat::Csv | ReportFormat::Text => unreachable!("handled by build_chart_report"),
        };
        Ok(ReportContent {
            subject: report_subject(s),
            body: format!("The report \"{}\" is attached.\n", s.name),
            attachment: Some(Attachment {
                filename: format!("{}.{ext}", slugify(&s.name)),
                content_type: content_type.into(),
                bytes,
            }),
        })
    }

    /// `csv`/`text`: run the chart's own SAVED spec — through the SAME
    /// `compile_chart_sql` seam `render::payload` uses for the print page's
    /// data, so a report can never diverge from what the interactive chart
    /// shows — and format the result without ever calling the renderer.
    /// A dashboard report is never `csv`/`text` (migration 0112's
    /// `schedule_report_format_matches_target` CHECK), so `s.chart_id` is
    /// always the target here.
    async fn build_chart_report(
        &self,
        s: &Schedule,
        owner: uuid::Uuid,
        format: ReportFormat,
    ) -> Result<ReportContent, EvalError> {
        let chart_id = s
            .chart_id
            .ok_or(EvalError::NotRunnable("report is missing its chart"))?;
        let chart = self
            .charts
            .get(chart_id)
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?
            .ok_or(EvalError::NotRunnable("report's chart no longer exists"))?;
        let spec: egret_bi::chartquery::QuerySpec =
            serde_json::from_value(chart.params.get("spec").cloned().ok_or_else(|| {
                EvalError::Query(format!("chart {} has no runnable query", chart.id))
            })?)
            .map_err(|_| EvalError::Query(format!("chart {} spec invalid", chart.id)))?;

        let (sql, database_id) = egret_bi::compile_chart_sql(
            &self.datasets,
            &self.databases,
            chart.workspace_id,
            chart.dataset_id,
            &spec,
        )
        .await
        .map_err(bi_compile_err)?;

        let identity = owner_identity(
            owner,
            database_id,
            &self.databases,
            &self.users,
            &*self.minter,
        )
        .await?;

        // The chart's own saved `row_limit`, not an alert's single-cell cap —
        // a report shows every row the chart was built to show.
        let result = self
            .query
            .run(
                database_id,
                &sql,
                &identity,
                Some(spec.row_limit.max(0) as usize),
            )
            .await
            .map_err(EvalError::Query)?;

        let columns: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();
        let subject = report_subject(s);

        match format {
            ReportFormat::Csv => {
                let rows_json =
                    serde_json::to_value(&result.rows).unwrap_or(serde_json::Value::Null);
                let csv = email::rows_to_csv(&columns, &rows_json);
                Ok(ReportContent {
                    subject,
                    body: format!("The report \"{}\" is attached.\n", s.name),
                    attachment: Some(Attachment {
                        filename: format!("{}.csv", slugify(&s.name)),
                        content_type: "text/csv".into(),
                        bytes: csv.into_bytes(),
                    }),
                })
            }
            // The thing you can read on a phone without opening an attachment
            // (design §7) — inline in the body, not an attachment.
            ReportFormat::Text => Ok(ReportContent {
                subject,
                body: format!(
                    "{}\n\n{}",
                    s.name,
                    rows_to_text_table(&columns, &result.rows)
                ),
                attachment: None,
            }),
            ReportFormat::Png | ReportFormat::Pdf => unreachable!("handled by render_image_report"),
        }
    }

    /// Deliver a report's rendered/exported content to every recipient —
    /// webhooks (a plain "this report ran" notice; a report carries no
    /// numeric `value`), and email where the workspace has SMTP configured.
    /// Mirrors `deliver`'s recipient-collection shape but with report-shaped
    /// content instead of an alert's `Evaluation`.
    async fn deliver_report(&self, s: &Schedule, content: &ReportContent) -> Result<(), String> {
        let recipients = self
            .schedules
            .recipients_for(s.id)
            .await
            .map_err(|e| e.to_string())?;
        let mut first_error = None;
        let mut email_to: Vec<String> = Vec::new();
        for r in recipients {
            if RecipientKind::parse(&r.kind) == Some(RecipientKind::Email) {
                if let Some(addrs) = r.config.get("addresses").and_then(|v| v.as_array()) {
                    email_to.extend(addrs.iter().filter_map(|a| a.as_str().map(str::to_string)));
                }
                continue;
            }
            if RecipientKind::parse(&r.kind) != Some(RecipientKind::Webhook) {
                continue;
            }
            let Some(url) = r.config.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            // From the vault, never the row — see `ScheduleRepo::vault_secrets`.
            let secret = self.schedules.resolve_webhook_secret(&r.config).await;
            let payload = WebhookPayload {
                schedule: &s.name,
                schedule_id: &s.id.to_string(),
                ty: &s.r#type,
                state: "success",
                value: None,
                triggered_at: chrono::Utc::now(),
                rows: serde_json::Value::Null,
            };
            if let Err(e) = webhook::deliver(
                url,
                secret.as_deref(),
                &payload,
                &self.policy,
                &*self.resolver,
            )
            .await
            {
                tracing::warn!(schedule = %s.id, error = %e, "automation: report webhook failed");
                first_error.get_or_insert(e.to_string());
            }
        }
        if !email_to.is_empty() {
            if let Err(e) = self.send_report_email(s, content, &email_to).await {
                tracing::warn!(schedule = %s.id, error = %e, "automation: report email failed");
                first_error.get_or_insert(e);
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// One mail to all of this report's email recipients, carrying whatever
    /// `content` built (an attachment for `png`/`pdf`/`csv`, inline text for
    /// `text`).
    async fn send_report_email(
        &self,
        s: &Schedule,
        content: &ReportContent,
        to: &[String],
    ) -> Result<(), String> {
        let config = self
            .smtp
            .get(s.workspace_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| crate::deliver::email::EmailError::NotConfigured.to_string())?;
        let password = self
            .smtp
            .resolve_password(&config)
            .await
            .map_err(|e| e.to_string())?;

        email::send(
            &config,
            password,
            &EmailMessage {
                to,
                subject: &content.subject,
                body: content.body.clone(),
                html: Some(email::notification_html(
                    &s.name,
                    s.message.as_deref(),
                    &content.body,
                    &self.object_url(s, &self.workspace_slug(s.workspace_id).await),
                )),
                attachments: content.attachment.clone().into_iter().collect(),
            },
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Send to every recipient — webhooks, and email where the workspace has
    /// SMTP configured.
    ///
    /// Returns the first failure, if any: the run is still `success` (the alert
    /// DID fire, and the grace period starts) but the error is recorded so a
    /// silently-undelivered notification is visible in the run log rather than
    /// only in a log file nobody is reading.
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
        let mut email_to: Vec<String> = Vec::new();
        for r in recipients {
            if RecipientKind::parse(&r.kind) == Some(RecipientKind::Email) {
                // Collected rather than sent per-recipient: one message with
                // several addressees is one connection to the relay, and one
                // thread for the people reading it.
                if let Some(addrs) = r.config.get("addresses").and_then(|v| v.as_array()) {
                    email_to.extend(addrs.iter().filter_map(|a| a.as_str().map(str::to_string)));
                }
                continue;
            }
            if RecipientKind::parse(&r.kind) != Some(RecipientKind::Webhook) {
                continue;
            }
            let Some(url) = r.config.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            // From the vault, never the row — see `ScheduleRepo::vault_secrets`.
            let secret = self.schedules.resolve_webhook_secret(&r.config).await;
            let payload = WebhookPayload {
                schedule: &s.name,
                schedule_id: &s.id.to_string(),
                ty: &s.r#type,
                state: "success",
                value: eval.value,
                triggered_at: chrono::Utc::now(),
                rows: eval.rows.clone(),
            };
            if let Err(e) = webhook::deliver(
                url,
                secret.as_deref(),
                &payload,
                &self.policy,
                &*self.resolver,
            )
            .await
            {
                tracing::warn!(schedule = %s.id, error = %e, "automation: webhook failed");
                first_error.get_or_insert(e.to_string());
            }
        }
        if !email_to.is_empty() {
            if let Err(e) = self.send_email(s, eval, &email_to).await {
                tracing::warn!(schedule = %s.id, error = %e, "automation: email failed");
                first_error.get_or_insert(e);
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The workspace slug, for building a user-facing URL. A schedule always
    /// belongs to a workspace, so a miss here means the row vanished under us.
    async fn workspace_slug(&self, ws: uuid::Uuid) -> String {
        sqlx::query_scalar::<_, String>("SELECT slug FROM workspaces WHERE id = $1")
            .bind(ws)
            .fetch_optional(&self.db.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// The URL a recipient can follow back to the thing that mailed them.
    ///
    /// A notification that says a number moved and gives no way to look is a
    /// dead end — Superset's mail ends in a call-to-action for exactly this
    /// reason. Falls back to the workspace's alert list when the schedule has
    /// no chart or dashboard of its own.
    fn object_url(&self, s: &Schedule, ws_slug: &str) -> String {
        let base = self.public_url.trim_end_matches('/');
        match report_target(s) {
            Some(("chart", id)) => format!("{base}/explore/{ws_slug}/charts/{id}"),
            Some(("dashboard", id)) => format!("{base}/explore/{ws_slug}/dashboards/{id}"),
            _ => format!("{base}/explore/{ws_slug}/alerts"),
        }
    }

    /// Render an alert's optional chart/dashboard, if it names one.
    ///
    /// Returns `None` — never an error — when the alert has no target, no
    /// image format, or the render fails. An alert exists to tell someone that
    /// a number moved; losing that message because a screenshot did not come
    /// back would be the worse failure, so the picture is best-effort and the
    /// reason lands in the log.
    async fn render_alert_attachment(&self, s: &Schedule) -> Option<email::Attachment> {
        let (kind, object_id) = report_target(s)?;
        // Only the image formats make sense here; the rows already ride along
        // as CSV, so `csv`/`text` on an alert would attach them twice.
        let format = match s.report_format.as_deref().and_then(ReportFormat::parse) {
            Some(f @ (ReportFormat::Png | ReportFormat::Pdf)) => f,
            _ => return None,
        };
        let owner = self.resolve_owner(s.id).await.ok()?;
        let token = self
            .render_tokens
            .mint(kind, object_id, owner, RENDER_TOKEN_TTL)
            .await
            .ok()?;
        let url = format!(
            "{}/embed/print/{kind}/{object_id}?t={token}",
            self.public_url.trim_end_matches('/')
        );
        let (width, height) = match (s.custom_width, s.custom_height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as u32, h as u32),
            _ => default_size(kind),
        };
        let bytes = match self
            .renderer
            .render(RenderRequest {
                url,
                urls: Vec::new(),
                width,
                height,
                format: format.as_str().to_string(),
                scale: 2,
            })
            .await
        {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(schedule = %s.id, error = %e, "automation: alert attachment render failed");
                return None;
            }
        };
        let (ext, content_type) = match format {
            ReportFormat::Png => ("png", "image/png"),
            _ => ("pdf", "application/pdf"),
        };
        Some(email::Attachment {
            filename: format!("{}.{ext}", slugify(&s.name)),
            content_type: content_type.into(),
            bytes,
        })
    }

    /// One mail to all of this schedule's email recipients.
    ///
    /// A workspace with no SMTP configured is a REPORTED failure, not a silent
    /// skip: the user added an email recipient, so "nothing happened and
    /// nothing said why" is the outcome to avoid.
    async fn send_email(
        &self,
        s: &Schedule,
        eval: &crate::evaluator::Evaluation,
        to: &[String],
    ) -> Result<(), String> {
        let config = self
            .smtp
            .get(s.workspace_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| crate::deliver::email::EmailError::NotConfigured.to_string())?;
        let password = self
            .smtp
            .resolve_password(&config)
            .await
            .map_err(|e| e.to_string())?;

        let subject = s
            .email_subject
            .clone()
            .unwrap_or_else(|| format!("[egret] {} triggered", s.name));
        let value = eval
            .value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(no value)".into());
        let body = format!(
            "The alert \"{}\" fired.\n\nObserved value: {}\n\nThe triggering rows are attached.\n",
            s.name, value
        );
        // The query's own column names. Falls back to positional headers only
        // if the engine returned none — a header of `column_1` is a last
        // resort, not the normal case, because an alert email is read by
        // someone who cannot see the SQL.
        let columns: Vec<String> = if eval.columns.is_empty() {
            let width = eval
                .rows
                .as_array()
                .and_then(|r| r.first())
                .and_then(|r| r.as_array())
                .map(Vec::len)
                .unwrap_or(1);
            (1..=width).map(|i| format!("column_{i}")).collect()
        } else {
            eval.columns.clone()
        };
        let csv = email::rows_to_csv(&columns, &eval.rows);

        let mut attachments = vec![email::Attachment {
            filename: format!("{}.csv", slugify(&s.name)),
            content_type: "text/csv".into(),
            bytes: csv.into_bytes(),
        }];
        // An alert may also name a chart or dashboard. When it does, the mail
        // carries a rendering of it beside the numbers — the recipient sees
        // WHAT moved, not only that something did. A render failure must not
        // lose the alert itself, so it is logged and the mail goes without.
        if let Some(extra) = self.render_alert_attachment(s).await {
            attachments.push(extra);
        }

        email::send(
            &config,
            password,
            &EmailMessage {
                to,
                subject: &subject,
                html: Some(email::notification_html(
                    &s.name,
                    s.message.as_deref(),
                    &body,
                    &self.object_url(s, &self.workspace_slug(s.workspace_id).await),
                )),
                body,
                attachments,
            },
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// `egret_bi::compile_chart_sql`'s error has no `Display` of its own
/// (`ApiError<D>` carries none) — mirrors `render::payload`'s `bi_api_err`,
/// but flattened to a message rather than a domain `AutomationError`, since
/// `build_chart_report` reports through `EvalError::Query` like every other
/// query failure on this path.
fn bi_compile_err(e: egret_bi::BiApiError) -> EvalError {
    EvalError::Query(match e {
        egret_bi::BiApiError::Auth(err) => err.to_string(),
        egret_bi::BiApiError::Domain(d) => d.to_string(),
    })
}

/// A filename-safe version of a schedule name.
fn slugify(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_lowercase();
    if s.is_empty() {
        "alert".into()
    } else {
        s
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

/// What a completed report run is ready to deliver: an email subject/body
/// plus, for every format except `text`, an attachment. `text` carries no
/// attachment by design (§7) — its table rides in `body` instead.
struct ReportContent {
    subject: String,
    body: String,
    attachment: Option<Attachment>,
}

/// The report's email subject — same fallback shape as an alert's, but naming
/// "report" rather than "triggered": a report always delivers, so "triggered"
/// would misdescribe every send.
fn report_subject(s: &Schedule) -> String {
    s.email_subject
        .clone()
        .unwrap_or_else(|| format!("[egret] {} report", s.name))
}

/// A small plain-text table — the thing a report's `text` format promises:
/// readable on a phone without opening an attachment (design §7). Deliberately
/// not aligned into columns: a fixed-width table only reads correctly in a
/// monospace font, which an HTML-rendering mail client will not give it.
fn rows_to_text_table(columns: &[String], rows: &[Vec<serde_json::Value>]) -> String {
    let mut out = columns.join(" | ");
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|c| match c {
                serde_json::Value::Null => String::new(),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push('\n');
    }
    out
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
