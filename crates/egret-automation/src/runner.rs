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

use crate::dataapi_metric;
use crate::deliver::destination::{Resolver, SsrfPolicy};
use crate::deliver::email::{self, Attachment, EmailMessage};
use crate::deliver::slack;
use crate::deliver::smtp_config::SmtpConfigRepo;
use crate::deliver::webhook::{self, WebhookPayload};
use crate::evaluator::{evaluate, owner_identity, EvalError, Evaluation, QueryRunner};
use crate::model::report_target;
use crate::render::client::{RenderRequest, Renderer};
use crate::render::token::RenderTokenRepo;
use crate::repo::{Schedule, ScheduleRepo};
use crate::scheduler::{claim_due, in_grace, prune_runs, record_run, release};
use crate::validator::should_fire;
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
    /// The single decision point (Task 4, Z2) — needed because a `dataapi_alert`
    /// re-checks its own access at run time, the way every OTHER schedule kind
    /// already does but by a different mechanism. An engine `alert` re-verifies
    /// the owner's access on every run because the CONNECTOR enforces it
    /// (`evaluator.rs`'s module doc: "authorization is settled by the engine at
    /// RUN time, not remembered from save time"); a `dataapi_alert` runs no
    /// query at all — it measures `dataapi.access_log` directly — so nothing
    /// downstream of this struct would otherwise notice that the owner lost
    /// `data_api_endpoint:read` after the schedule was saved.
    pub pdp: Arc<dyn egret_core::authz::Pdp>,
    /// Fix-round-3: the same `DataApiEndpointPort` seam
    /// `AutomationState.dataapi_endpoints` uses (routes.rs) — resolves an
    /// endpoint id's REAL workspace before this run-time re-check ever asks
    /// the PDP about it. Without this, `owner_can_read_dataapi_endpoint`'s
    /// `ObjectRef.workspace_id` below is the SCHEDULE's workspace, never the
    /// endpoint's, so a workspace-cascaded owner grant would authorize ANY
    /// uuid — structurally the same gap fix-round-2 closed on the routes.
    /// Confirmed a DEAD branch as of this fix: both write paths now refuse a
    /// foreign `dataapi_endpoint_id` before any row is ever written, so no
    /// live schedule can carry one today. Fixed anyway, for symmetry and
    /// because the seam already exists — see the report for the full
    /// reachability argument.
    pub dataapi_endpoints: Arc<dyn egret_dataapi_api::DataApiEndpointPort>,
    pub resolver: Arc<dyn Resolver>,
    pub policy: SsrfPolicy,
    pub smtp: SmtpConfigRepo,
    /// The WORKSPACE's Slack app. A recipient names a channel; the credential
    /// belongs to the workspace, like its SMTP relay.
    pub slack_config: crate::deliver::slack_config::SlackConfigRepo,
    /// `csv`/`text` reports execute the chart's own saved spec — never the
    /// renderer — through the same `compile_chart_sql` seam the render-payload
    /// endpoint uses (`render::payload`), so a report can never diverge from
    /// what the interactive chart would show.
    pub charts: egret_bi::ChartRepo,
    /// `render_image_report`/`render_alert_attachment` need this too — a
    /// dashboard target must be confirmed to belong to THIS schedule's
    /// workspace before a token is minted for it, the same way
    /// `build_chart_report` already confirms a chart (SEC-3).
    pub dashboards: egret_bi::DashboardRepo,
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
    /// Base URL for links a PERSON follows. `public_url` is the renderer's
    /// in-cluster address; a mail linking there is a dead end in the inbox.
    pub browser_url: String,
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
        // An explicit three-way branch — not `if report { .. } else { /*
        // alert */ }` — because that shape is exactly what let a
        // `dataapi_alert` fall through into the SQL-alert branch by default
        // and hit `NotRunnable("alert is missing its query")` below: a
        // `dataapi_alert` carries neither `database_id` nor `sql`
        // (`schedule_dataapi_needs_source`, migration 0128), so it can never
        // satisfy that branch's requirements. An unrecognised type is now its
        // own error rather than silently taking the alert path, so a future
        // fourth type fails loudly here instead of repeating this bug.
        match s.r#type.as_str() {
            "report" => {
                // Reports have no condition to evaluate — a report that
                // renders and delivers always "fires" — so they branch off
                // before any of the alert's query/validator machinery below.
                return self.render_and_deliver(s).await;
            }
            "dataapi_alert" => return self.evaluate_and_deliver_dataapi_alert(s).await,
            "alert" => {}
            _ => return Err(EvalError::NotRunnable("schedule has an unrecognised type")),
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
            s.workspace_id,
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

    /// The `dataapi_alert` branch (Z2): the condition is MEASURED, from a
    /// window of `dataapi.access_log` via `dataapi_metric::measure`, rather
    /// than run as SQL — there is no engine, no owner identity, and no
    /// `evaluate` here. Everything from `should_fire` onward is the exact
    /// same machinery `evaluate_and_deliver`'s alert branch uses, so a
    /// `dataapi_alert` and an engine `alert` behave identically once a value
    /// is in hand: the same validator, the same grace suppression (§233's
    /// `in_grace`, called below just like the alert path calls it), the same
    /// `deliver`.
    ///
    /// `metric`/`window_s` are asserted `Some` rather than defaulted:
    /// migration 0128's `schedule_dataapi_needs_source` CHECK already
    /// requires both non-null (and `window_s > 0`) for a persisted
    /// `dataapi_alert` row, so a `None` here means the row is corrupt, not
    /// that the caller should guess a default — the same "unreachable but
    /// guarded, not `unwrap`ped" stance `evaluate_and_deliver`'s own
    /// `NotRunnable` fail-safe takes for the SQL-alert branch just above.
    /// `validator_type` is trusted to be `"operator"` for the same reason:
    /// the same CHECK pins it, so `ValidatorType::Operator` is used directly
    /// rather than parsed from the row.
    ///
    /// NOT handled here: a `dataapi_endpoint_id` whose endpoint has been
    /// trashed or disabled. §5.6 of the design wants that case to read as
    /// `noop` with the reason in the run log, not `error` — moving an
    /// endpoint to the trash is deliberate, and an error every five minutes
    /// because of it is noise nobody asked for. But that state lives on
    /// `dataapi.endpoints`, owned by `egret-dataapi`, and this crate does not
    /// depend on it (see `dataapi_metric.rs`'s own module doc: the six
    /// queries are duplicated rather than importing that crate). Reaching
    /// into `dataapi.endpoints` with a raw query here would be exactly the
    /// workaround the brief for this task says not to invent.
    ///
    /// Task 4 stands up the first contract seam this crate shares with
    /// `egret-dataapi` (`egret-dataapi-api`, a pure leaf — no sqlx, no axum),
    /// but deliberately does not extend it to endpoint STATE. Visibility of a
    /// trashed/disabled row is a different question from "may the owner READ
    /// this endpoint" — a trashed endpoint an owner can still read is exactly
    /// the case this section is about, and folding both into one task risks
    /// under-building the smaller one to close the box. Its natural shape,
    /// for whoever picks it up: an `EndpointState { trashed: bool, enabled:
    /// bool }` DTO in `egret-dataapi-api` plus a small port trait
    /// (`AutomationRunner` already holds several — `QueryRunner`, `Renderer`,
    /// `EngineTokenMinter` — this would be one more `Arc<dyn _>`), implemented
    /// by a thin `egret-dataapi`-backed adapter and wired the same way
    /// `pdp`/`query`/`renderer` are below. Left for that task.
    async fn evaluate_and_deliver_dataapi_alert(&self, s: &Schedule) -> Result<Outcome, EvalError> {
        let (Some(metric), Some(window_s)) = (s.dataapi_metric.as_deref(), s.dataapi_window_s)
        else {
            return Err(EvalError::NotRunnable(
                "dataapi_alert is missing its metric or window",
            ));
        };

        // Run-time re-check (Task 4, Z2): the OWNER — the identity everything
        // downstream answers to, same as an engine alert's `resolve_owner` —
        // may have lost `data_api_endpoint:read` on the watched endpoint (or,
        // unscoped, on the workspace) since the schedule was saved. An engine
        // alert gets this for free because the CONNECTOR enforces the
        // owner's access on every run (`evaluator.rs`'s module doc); a
        // `dataapi_alert` runs no query at all, so nothing else on this path
        // would notice. Checked BEFORE `measure` — a caller who has lost
        // access to the endpoint must not have its access_log rows counted
        // for them, even into an aggregate.
        let owner = self.resolve_owner(s.id).await?;
        if !owner_can_read_dataapi_endpoint(
            &*self.pdp,
            &*self.dataapi_endpoints,
            owner,
            s.workspace_id,
            s.dataapi_endpoint_id,
        )
        .await?
        {
            return Err(EvalError::EndpointAccessRevoked);
        }

        let value = dataapi_metric::measure(
            &self.db,
            s.workspace_id,
            s.dataapi_endpoint_id,
            metric,
            s.dataapi_error_code.as_deref(),
            window_s,
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| EvalError::Query(e.to_string()))?;

        // No extra guard needed for a `None` measurement (an empty window,
        // e.g. `error_rate` with zero calls): `should_fire`'s `Operator` arm
        // already answers `false` for every operator when `value` is `None`
        // (validator.rs's `a_missing_value_never_fires_whatever_the_operator`
        // — the null trap the module's own doc comment calls out), so it
        // falls straight into the `!fired` branch below, recorded as `noop`
        // like any other non-firing evaluation.
        let fired = should_fire(ValidatorType::Operator, &s.validator_config, value)?;
        if !fired {
            return Ok(Outcome::NotFired { value });
        }
        // Fired — but recently enough that the user has already been told.
        // Same call, same column, same semantics as the SQL-alert branch's
        // grace check above: a dataapi_alert must not bypass anti-spam just
        // because it reached `Outcome::Fired` a different way.
        if in_grace(&self.db, s.id, s.grace_period_s)
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?
        {
            return Ok(Outcome::Suppressed { value });
        }

        // A dataapi_alert has no query result set to attach — the value IS
        // the whole answer. `rows`/`columns` are empty rather than omitted so
        // `deliver` can stay the ONE delivery path both alert kinds share:
        // `rows_to_csv` over `Value::Null` produces a header-only CSV, and
        // the webhook payload's `rows` is `null` — an honest description of
        // "there were no rows", not a fabricated one.
        let eval = Evaluation {
            fired: true,
            value,
            rows: serde_json::Value::Null,
            columns: Vec::new(),
        };
        let delivered = self.deliver(s, &eval).await;
        Ok(Outcome::Fired { value, delivered })
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

        // Every target must belong to THIS schedule's workspace, confirmed
        // BEFORE a token is minted or the renderer is ever asked to fetch it.
        // `schedule_targets.object_id` has no foreign key and no workspace
        // column (it can't — `kind` is polymorphic), and save-time validation
        // (SEC-2) only guards writes going forward: a row already in the
        // table — written before that shipped, or by anything that bypassed
        // the API — must fail HERE, as a reported "error" run with nothing
        // sent, rather than mint a token for an object that will only 404 at
        // `/render/payload` and let the renderer hand back a screenshot of
        // whatever the print page shows for that.
        for (kind, object_id) in &targets {
            let found = match *kind {
                "chart" => self
                    .charts
                    .get(*object_id, s.workspace_id)
                    .await
                    .map_err(|e| EvalError::Query(e.to_string()))?
                    .is_some(),
                _ => self
                    .dashboards
                    .get(*object_id, s.workspace_id)
                    .await
                    .map_err(|e| EvalError::Query(e.to_string()))?
                    .is_some(),
            };
            if !found {
                return Err(EvalError::NotRunnable("report's target no longer exists"));
            }
        }

        // One token PER object: a render token is bound to a single object by
        // design, so N pages need N of them.
        let mut urls = Vec::with_capacity(targets.len());
        for (kind, object_id) in &targets {
            let token = self
                .render_tokens
                .mint(kind, *object_id, s.workspace_id, owner, RENDER_TOKEN_TTL)
                .await
                .map_err(|e| EvalError::Query(e.to_string()))?;
            urls.push(format!(
                "{}/embed/print/{kind}/{object_id}?t={token}",
                self.public_url.trim_end_matches('/')
            ));
        }
        // `kind` still drives the default capture size; the id is already in
        // the URL list built above.
        let (kind, _) = targets[0];
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
            body: match format {
                // A PNG is shown IN the body, so "attached" would be a lie in
                // the plain-text alternative a text-only client reads.
                ReportFormat::Png => format!("The report \"{}\" is below.\n", s.name),
                _ => format!("The report \"{}\" is attached.\n", s.name),
            },
            attachment: Some(Attachment {
                filename: format!("{}.{ext}", slugify(&s.name)),
                content_type: content_type.into(),
                bytes,
                inline_cid: match format {
                    ReportFormat::Png => Some(REPORT_IMAGE_CID.to_string()),
                    // A multi-page PDF has no `cid:` rendering — every client
                    // would show a broken image where the pages should be.
                    _ => None,
                },
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
            .get(chart_id, s.workspace_id)
            .await
            .map_err(|e| EvalError::Query(e.to_string()))?
            .ok_or(EvalError::NotRunnable("report's chart no longer exists"))?;
        let spec: egret_bi::chartquery::QuerySpec =
            serde_json::from_value(chart.params.get("spec").cloned().ok_or_else(|| {
                EvalError::Query(format!("chart {} has no runnable query", chart.id))
            })?)
            .map_err(|_| EvalError::Query(format!("chart {} spec invalid", chart.id)))?;

        // The SCHEDULE's own workspace, not the chart's — they are the same
        // value by construction once `validate_targets_in_workspace` refuses a
        // foreign chart at save time, but passing `chart.workspace_id` here
        // was the exact self-referential idiom (`ds.workspace_id != workspace_id`
        // compared against itself) that let the original cross-tenant
        // disclosure through: the one scoping guard on this path could never
        // fire because both sides were sourced from the same foreign object.
        // `s.workspace_id` makes the guard live defence-in-depth again.
        let (sql, database_id) = egret_bi::compile_chart_sql(
            &self.datasets,
            &self.databases,
            s.workspace_id,
            chart.dataset_id,
            &spec,
        )
        .await
        .map_err(bi_compile_err)?;

        let identity = owner_identity(
            owner,
            s.workspace_id,
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
                        inline_cid: None,
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
        for r in &recipients {
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
        if let Some(e) = self.deliver_slack(s, &content.body, &recipients).await {
            first_error.get_or_insert(e);
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
                html: Some(email::notification_html(&email::NotificationBody {
                    title: &s.name,
                    header: s.message.as_deref(),
                    footer: s.message_footer.as_deref(),
                    summary: &content.body,
                    image_cid: content
                        .attachment
                        .as_ref()
                        .and_then(|a| a.inline_cid.as_deref()),
                    url: &self.object_url(s, &self.workspace_slug(s.workspace_id).await),
                })),
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
        for r in &recipients {
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
        let value = eval
            .value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(no value)".into());
        if let Some(e) = self
            .deliver_slack(s, &format!("Observed value: {value}"), &recipients)
            .await
        {
            first_error.get_or_insert(e);
        }
        if !email_to.is_empty() {
            match self.send_email(s, eval, &email_to).await {
                Ok(None) => {}
                // Sent, but with a note for the run log (fix round 1) — e.g.
                // an alert's attachment was skipped because its target
                // belongs to another workspace. `state` must stay whatever
                // the alert's own condition produced; only `error` explains.
                Ok(Some(note)) => {
                    first_error.get_or_insert(note);
                }
                Err(e) => {
                    tracing::warn!(schedule = %s.id, error = %e, "automation: email failed");
                    first_error.get_or_insert(e);
                }
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Post to every Slack recipient. One message per channel, best-effort in
    /// the same sense as the rest of delivery: one refusing channel must not
    /// stop the others or lose the run.
    async fn deliver_slack(
        &self,
        s: &Schedule,
        summary: &str,
        recipients: &[crate::repo::Recipient],
    ) -> Option<String> {
        let mut first_error: Option<String> = None;
        let url = self.object_url(s, &self.workspace_slug(s.workspace_id).await);
        for r in recipients {
            if RecipientKind::parse(&r.kind) != Some(RecipientKind::Slack) {
                continue;
            }
            let Some(channel) = r.config.get("channel").and_then(|v| v.as_str()) else {
                continue;
            };
            // The recipient's OWN token first, then the workspace's.
            //
            // Not merely a preference order — it is what makes migration 0118
            // safe. Every slack recipient used to carry a token, and the
            // backfill could only adopt ONE of them per workspace; reading the
            // recipient's first means a destination whose token differed keeps
            // delivering with exactly the credential it always used. New
            // recipients carry none and fall through to the shared one.
            let token = match self.schedules.resolve_slack_token(&r.config).await {
                Some(t) => Some(t),
                None => self.slack_config.resolve_token(s.workspace_id).await,
            };
            let Some(token) = token else {
                first_error.get_or_insert_with(|| slack::SlackError::NoToken.to_string());
                continue;
            };
            let text = slack::message(&s.name, s.message.as_deref(), summary, &url);
            if let Err(e) = slack::deliver(&token, channel, &text).await {
                tracing::warn!(schedule = %s.id, error = %e, "automation: slack delivery failed");
                first_error.get_or_insert(e.to_string());
            }
        }
        first_error
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
        let base = self.browser_url.trim_end_matches('/');
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
    ///
    /// Returns `Err` in exactly ONE case: the target exists but does not
    /// belong to this schedule's workspace. Every other "no picture" reason
    /// (no target set, wrong format, owner/mint/render trouble) stays
    /// `Ok(None)` — silent, as before fix round 1 — because those are
    /// operational, not a tenancy refusal a workspace-1 operator needs
    /// explained. `send_email` threads the `Err` into `deliver`'s
    /// `first_error` so it reaches `schedule_runs.error` without flipping
    /// `state` away from whatever the alert's own condition produced.
    async fn render_alert_attachment(
        &self,
        s: &Schedule,
    ) -> Result<Option<email::Attachment>, String> {
        let Some((kind, object_id)) = report_target(s) else {
            return Ok(None);
        };
        // Only the image formats make sense here; the rows already ride along
        // as CSV, so `csv`/`text` on an alert would attach them twice.
        let format = match s.report_format.as_deref().and_then(ReportFormat::parse) {
            Some(f @ (ReportFormat::Png | ReportFormat::Pdf)) => f,
            _ => return Ok(None),
        };
        // `model.rs`'s alert branch checks only ARITY — "not both a chart and
        // a dashboard" — never ownership, so `chart_id`/`dashboard_id` can be
        // a foreign object on a row written before SEC-2's save-time check
        // existed. Confirmed here, BEFORE a token is minted: an alert's own
        // `sql`/`value` never touched this object, so losing the render must
        // not lose the alert (the philosophy above), but it must also never
        // mint a token — let alone attach a picture — for something that
        // isn't this workspace's.
        let scoped = match kind {
            "chart" => match self.charts.get(object_id, s.workspace_id).await {
                Ok(v) => v.is_some(),
                // A DB error fetching the chart is operational, not a
                // tenancy fact — stays silent, matching every other trouble
                // path below.
                Err(_) => return Ok(None),
            },
            _ => match self.dashboards.get(object_id, s.workspace_id).await {
                Ok(v) => v.is_some(),
                Err(_) => return Ok(None),
            },
        };
        if !scoped {
            let note = format!(
                "alert attachment skipped: {kind} {object_id} does not belong to this workspace"
            );
            tracing::warn!(
                schedule = %s.id, %kind, %object_id,
                "automation: alert attachment target does not belong to this workspace"
            );
            return Err(note);
        }
        let Ok(owner) = self.resolve_owner(s.id).await else {
            return Ok(None);
        };
        let Ok(token) = self
            .render_tokens
            .mint(kind, object_id, s.workspace_id, owner, RENDER_TOKEN_TTL)
            .await
        else {
            return Ok(None);
        };
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
                return Ok(None);
            }
        };
        let (ext, content_type) = match format {
            ReportFormat::Png => ("png", "image/png"),
            _ => ("pdf", "application/pdf"),
        };
        Ok(Some(email::Attachment {
            filename: format!("{}.{ext}", slugify(&s.name)),
            content_type: content_type.into(),
            bytes,
            inline_cid: matches!(format, ReportFormat::Png).then(|| REPORT_IMAGE_CID.to_string()),
        }))
    }

    /// One mail to all of this schedule's email recipients.
    ///
    /// A workspace with no SMTP configured is a REPORTED failure, not a silent
    /// skip: the user added an email recipient, so "nothing happened and
    /// nothing said why" is the outcome to avoid.
    /// `Ok(None)`: sent cleanly. `Ok(Some(note))`: sent, but with something
    /// the run log's `error` column should say (fix round 1 — currently only
    /// `render_alert_attachment`'s tenancy refusal). `Err`: the send itself
    /// failed and nothing went out.
    async fn send_email(
        &self,
        s: &Schedule,
        eval: &crate::evaluator::Evaluation,
        to: &[String],
    ) -> Result<Option<String>, String> {
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

        let subject = render_subject(s, &format!("[egret] {} triggered", s.name));
        let value = eval
            .value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(no value)".into());
        let body = alert_email_body(&s.r#type, &s.name, &value);

        // A `dataapi_alert` measures `dataapi.access_log` directly — there is
        // no query result set (`evaluate_and_deliver_dataapi_alert` always
        // builds `Evaluation { rows: Value::Null, columns: Vec::new() }`, its
        // own doc comment). Building a CSV from that would not describe "zero
        // rows" — the positional fallback below reads the empty `columns` as
        // "the engine returned none" and fabricates a `column_1` header, so
        // the mail would claim rows are attached and enclose a 12-byte lie.
        // The alerts programme already found and fixed this exact artifact
        // once on a live check; a `dataapi_alert` must never reach this
        // branch at all. See `alert_email_body`'s test for the prior wording.
        let mut attachments: Vec<email::Attachment> = Vec::new();
        if s.r#type != "dataapi_alert" {
            // The query's own column names. Falls back to positional headers
            // only if the engine returned none — a header of `column_1` is a
            // last resort, not the normal case, because an alert email is
            // read by someone who cannot see the SQL.
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
            attachments.push(email::Attachment {
                filename: format!("{}.csv", slugify(&s.name)),
                content_type: "text/csv".into(),
                bytes: csv.into_bytes(),
                inline_cid: None,
            });
        }
        // An alert may also name a chart or dashboard. When it does, the mail
        // carries a rendering of it beside the numbers — the recipient sees
        // WHAT moved, not only that something did. A render failure must not
        // lose the alert itself, so it is logged and the mail goes without.
        let mut note = None;
        match self.render_alert_attachment(s).await {
            Ok(Some(extra)) => attachments.push(extra),
            Ok(None) => {}
            Err(reason) => note = Some(reason),
        }

        email::send(
            &config,
            password,
            &EmailMessage {
                to,
                subject: &subject,
                html: Some(email::notification_html(&email::NotificationBody {
                    title: &s.name,
                    header: s.message.as_deref(),
                    footer: s.message_footer.as_deref(),
                    summary: &body,
                    image_cid: attachments.iter().find_map(|a| a.inline_cid.as_deref()),
                    url: &self.object_url(s, &self.workspace_slug(s.workspace_id).await),
                })),
                body,
                attachments,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(note)
    }
}

/// Whether `owner` may still read `endpoint_id` (or, when `None`, holds
/// workspace-level Data API read) — the run-time re-check Task 4 adds for a
/// `dataapi_alert` (§`evaluate_and_deliver_dataapi_alert`'s own doc comment).
///
/// A free function, not a method, because it needs only the PDP: unlike
/// `egret-dataapi::invoke::can_invoke` (the sibling this mirrors), it is
/// called from a crate that does not — and, per this task's own decision
/// above, should not — depend on `egret-dataapi` for anything beyond the two
/// constants in `egret-dataapi-api`.
///
/// `group_ids: Vec::new()` — no group lookup for the owner here, following
/// `can_invoke`'s own precedent for an owner-based re-check made outside a
/// live session: a group-scoped grant is a possible false REFUSAL (the owner
/// is treated as having fewer permissions than they actually do), never a
/// false allow, so it cannot turn this into a security hole — only, in the
/// rare case a workspace grants Data API read solely through a group, into an
/// alert that stops firing until it is re-saved. The same conservative gap
/// `can_invoke` already accepts.
///
/// Fix-round-3: `endpoint_id`'s `Some` branch first resolves the endpoint's
/// REAL workspace through `dataapi_endpoints` and returns `Ok(false)` on a
/// mismatch (or a nonexistent id) — folded into the same boolean the PDP
/// path returns, not a new `EvalError` variant, because the caller already
/// treats "not allowed" uniformly as `EndpointAccessRevoked` regardless of
/// WHY: a schedule watching an endpoint outside its own workspace is exactly
/// as unable to read it as one whose owner lost the grant.
async fn owner_can_read_dataapi_endpoint(
    pdp: &dyn egret_core::authz::Pdp,
    dataapi_endpoints: &dyn egret_dataapi_api::DataApiEndpointPort,
    owner: uuid::Uuid,
    workspace_id: uuid::Uuid,
    endpoint_id: Option<uuid::Uuid>,
) -> Result<bool, EvalError> {
    let subject = egret_core::authz::Subject {
        user_id: egret_core::UserId(owner),
        group_ids: Vec::new(),
    };
    let resource = match endpoint_id {
        Some(id) => {
            let owner_ws = dataapi_endpoints
                .workspace_of(id)
                .await
                .map_err(|e| EvalError::Query(e.to_string()))?;
            if owner_ws != Some(workspace_id) {
                return Ok(false);
            }
            egret_core::authz::Resource::Object(egret_core::authz::ObjectRef {
                kind: egret_dataapi_api::OBJECT_KIND.to_string(),
                id,
                folder_id: None,
                workspace_id: egret_core::WorkspaceId(workspace_id),
            })
        }
        None => egret_core::authz::Resource::Workspace(egret_core::WorkspaceId(workspace_id)),
    };
    pdp.authorize(&subject, egret_dataapi_api::EP_READ, &resource)
        .await
        .map(|d| d.allow)
        .map_err(|e| EvalError::Query(e.to_string()))
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

/// The alert email's own summary line (fix-wave finding 1).
///
/// An engine `alert` always attaches its triggering rows as a CSV, so its
/// body says so. A `dataapi_alert` has no query result set — the measured
/// value IS the whole answer, the same fact `ScheduleEditor.tsx`'s "There are
/// no query rows to attach" note states to the author — so its body must not
/// claim rows are attached, the way `send_email`'s hardcoded line used to for
/// every schedule type unconditionally.
fn alert_email_body(schedule_type: &str, name: &str, value: &str) -> String {
    if schedule_type == "dataapi_alert" {
        format!("The alert \"{name}\" fired.\n\nObserved value: {value}\n")
    } else {
        format!(
            "The alert \"{name}\" fired.\n\nObserved value: {value}\n\n\
             The triggering rows are attached.\n"
        )
    }
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
    render_subject(s, &format!("[egret] {} report", s.name))
}

/// `Content-ID` for the picture shown inside the notification body. One value
/// because a notification carries at most one rendered image; the body's
/// `cid:` reference and the part's header must agree, so they read it from
/// here rather than each spelling it out.
const REPORT_IMAGE_CID: &str = "egret-render";

/// The subject a run actually sends: the user's template with its `{{date}}`
/// style placeholders filled in, or `default` when they set none.
///
/// Rendered in the schedule's own timezone — see [`crate::subject`]. Unknown
/// placeholders cannot reach here: `to_new` rejects them on save.
fn render_subject(s: &Schedule, default: &str) -> String {
    let template = s.email_subject.as_deref().unwrap_or(default);
    crate::subject::render(template, &s.name, chrono::Utc::now(), &s.timezone)
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
/// the workflow runtime's tickers (`egret_workflow::runtime::spawn_with_logs`)
/// — one periodic task per replica, no leader election, with `SKIP LOCKED` in
/// the claim making that safe.
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

// ── SEC-3: render_alert_attachment's scoping check, direct and in-process ──
//
// This one entry point cannot be reached from an integration test in
// `tests/`: `send_email` (its only caller) needs a REAL SMTP relay
// (`crate::deliver::email::send` builds a `lettre` transport over the
// network), which this crate's test suite never stands up. So this calls the
// private method directly, in-process, against a real database — the same
// shape `evaluator.rs`'s own `#[cfg(test)] mod tests` already uses for
// `owner_identity`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::RunIdentity;
    use crate::render::client::RenderRequest;
    use egret_bi::{ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SourceSpec};
    use std::sync::Mutex;
    use uuid::Uuid;

    /// Records every request; answers with fixed PNG bytes. If
    /// `render_alert_attachment` calls this for a foreign chart, the test
    /// that asserts it never does will see a non-empty `seen`.
    struct FakeRenderer {
        seen: Mutex<Vec<RenderRequest>>,
    }
    #[async_trait::async_trait]
    impl Renderer for FakeRenderer {
        async fn render(&self, req: RenderRequest) -> Result<Vec<u8>, String> {
            self.seen.lock().unwrap().push(req);
            Ok(vec![0x89, b'P', b'N', b'G'])
        }
    }

    struct NoMinter;
    #[async_trait::async_trait]
    impl EngineTokenMinter for NoMinter {
        async fn mint(
            &self,
            _user: Uuid,
        ) -> Result<
            egret_analytics_api::engine_token::EngineToken,
            egret_analytics_api::engine_token::BrokerError,
        > {
            Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
        }
    }

    /// `render_alert_attachment` must never reach the query engine — it only
    /// mints a render token and asks the renderer for a picture.
    struct NoQuery;
    #[async_trait::async_trait]
    impl QueryRunner for NoQuery {
        async fn run(
            &self,
            _db: Uuid,
            _sql: &str,
            _identity: &RunIdentity,
            _limit: Option<usize>,
        ) -> Result<egret_core::TabularResult, String> {
            Err("render_alert_attachment must never query an engine".into())
        }
    }

    /// None of this module's tests exercise a `dataapi_alert` — they all
    /// build engine `alert`s via `mk_alert` — so the run-time
    /// `data_api_endpoint:read` re-check (`owner_can_read_dataapi_endpoint`)
    /// must never be reached. A panic here would surface that as a loud
    /// test failure rather than a silently-wrong `allow`/`deny`.
    struct NeverPdp;
    #[async_trait::async_trait]
    impl egret_core::authz::Pdp for NeverPdp {
        async fn authorize(
            &self,
            _subject: &egret_core::authz::Subject,
            _action: &str,
            _resource: &egret_core::authz::Resource,
        ) -> egret_core::Result<egret_core::authz::Decision> {
            panic!("this module's tests only build engine alerts; the PDP must never be consulted");
        }
    }

    struct PublicDns;
    #[async_trait::async_trait]
    impl Resolver for PublicDns {
        async fn resolve(
            &self,
            _host: &str,
            port: u16,
        ) -> Result<Vec<std::net::SocketAddr>, std::io::Error> {
            Ok(vec![std::net::SocketAddr::new(
                "93.184.216.34".parse().unwrap(),
                port,
            )])
        }
    }

    async fn mk_user(db: &Db, tag: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("{tag}_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    async fn mk_workspace(db: &Db, owner: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
        )
        .bind(format!("w-{}", Uuid::new_v4().simple()))
        .bind(owner)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    /// A chart that exists, in `ws`, with a real (if unused here — this path
    /// never compiles its spec) dataset behind it.
    async fn mk_chart(db: &Db, ws: Uuid, owner: Uuid) -> Uuid {
        let database: Uuid = sqlx::query_scalar(
            "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
             VALUES ($1,'trino','localhost',8090,$2,false) RETURNING id",
        )
        .bind(format!("db-{}", Uuid::new_v4().simple()))
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let datasets = DatasetRepo::new(db.clone());
        let dataset = datasets
            .create(
                ws,
                database,
                "orders",
                "",
                &SourceSpec::Physical {
                    catalog: Some("tpch".into()),
                    schema: "tiny".into(),
                    table: "orders".into(),
                },
                &[NewColumn {
                    column_name: "orderpriority".into(),
                    data_type: "varchar".into(),
                    type_generic: "string".into(),
                    is_temporal: false,
                }],
                Some(egret_core::UserId(owner)),
            )
            .await
            .unwrap();
        let charts = ChartRepo::new(db.clone());
        let chart = charts
            .create(
                ws,
                dataset.id,
                "chart",
                "",
                "table",
                &serde_json::json!({"spec": {}}),
                Some(owner),
            )
            .await
            .unwrap();
        chart.id
    }

    fn test_runner(db: &Db, renderer: Arc<FakeRenderer>) -> AutomationRunner {
        test_runner_with_query(db, renderer, Arc::new(NoQuery))
    }

    fn test_runner_with_query(
        db: &Db,
        renderer: Arc<FakeRenderer>,
        query: Arc<dyn QueryRunner>,
    ) -> AutomationRunner {
        let secrets = egret_secrets::SecretStore::new(db.clone(), None);
        let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
        AutomationRunner {
            db: db.clone(),
            schedules: ScheduleRepo::new(
                db.clone(),
                egret_secrets::SecretStore::new(
                    db.clone(),
                    Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
                ),
            ),
            databases: DatabaseRepo::new(db.clone(), secrets, ws_secrets),
            users: UserRepo::new(db.clone()),
            minter: Arc::new(NoMinter),
            query,
            pdp: Arc::new(NeverPdp),
            dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
            resolver: Arc::new(PublicDns),
            policy: SsrfPolicy::default(),
            smtp: SmtpConfigRepo::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
            slack_config: crate::deliver::slack_config::SlackConfigRepo::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
            charts: ChartRepo::new(db.clone()),
            dashboards: DashboardRepo::new(db.clone()),
            datasets: DatasetRepo::new(db.clone()),
            render_tokens: RenderTokenRepo::new(db.clone()),
            renderer,
            public_url: "https://egret.internal".into(),
            browser_url: "https://egret.internal".into(),
        }
    }

    /// Insert an alert directly — no route, matching the probe's method for
    /// rows that "got in anyway" (written before SEC-2, or by anything that
    /// bypassed the API). `chart_id` may belong to a different workspace than
    /// `ws`; that is the whole point of the two tests below.
    async fn mk_alert(db: &Db, ws: Uuid, owner: Uuid, chart_id: Uuid) -> Schedule {
        // `schedule_alert_needs_query` requires database_id/sql/validator_type
        // regardless of what this test exercises — a own-workspace database,
        // since the CHECK does not (and cannot) also require it to match the
        // schedule's workspace.
        let database: Uuid = sqlx::query_scalar(
            "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
             VALUES ($1,'trino','localhost',8090,$2,false) RETURNING id",
        )
        .bind(format!("db-{}", Uuid::new_v4().simple()))
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO automation.schedules \
               (workspace_id, type, name, crontab, database_id, sql, validator_type, \
                chart_id, report_format) \
             VALUES ($1,'alert','t','* * * * *',$2,'SELECT 1','not_null',$3,'png') \
             RETURNING id",
        )
        .bind(ws)
        .bind(database)
        .bind(chart_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
            .bind(id)
            .bind(owner)
            .execute(&db.pool)
            .await
            .unwrap();
        ScheduleRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        )
        .get(ws, id)
        .await
        .unwrap()
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn render_alert_attachment_renders_a_same_workspace_chart(pool: sqlx::PgPool) {
        let db = Db { pool };
        let owner = mk_user(&db, "own").await;
        let ws = mk_workspace(&db, owner).await;
        let chart = mk_chart(&db, ws, owner).await;
        let s = mk_alert(&db, ws, owner, chart).await;

        let renderer = Arc::new(FakeRenderer {
            seen: Mutex::new(vec![]),
        });
        let runner = test_runner(&db, renderer.clone());

        let attachment = runner.render_alert_attachment(&s).await.unwrap();
        assert!(attachment.is_some(), "a same-workspace chart still renders");
        assert_eq!(
            renderer.seen.lock().unwrap().len(),
            1,
            "the renderer was asked exactly once"
        );
        // Positive control for fix round 1's item 2: a legitimate render DOES
        // mint a token, so the foreign case's "0 tokens" below is a real
        // contrast rather than a fixture that never mints one at all.
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM automation.render_tokens")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(tokens, 1, "a same-workspace render mints exactly one token");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn render_alert_attachment_refuses_a_foreign_chart_without_minting_a_token(
        pool: sqlx::PgPool,
    ) {
        // §5's alert case: `model.rs`'s alert branch checks only that a
        // chart_id and a dashboard_id are not BOTH set — never that the one
        // set belongs to the alert's own workspace. This is a row that "got
        // in anyway" (mirrors the probe: written before SEC-2, or by
        // anything that bypassed the route), inserted directly by SQL.
        let db = Db { pool };
        let owner = mk_user(&db, "own").await;
        let ws1 = mk_workspace(&db, owner).await;
        let ws2 = mk_workspace(&db, owner).await;
        let foreign_chart = mk_chart(&db, ws2, owner).await;
        let s = mk_alert(&db, ws1, owner, foreign_chart).await;

        let renderer = Arc::new(FakeRenderer {
            seen: Mutex::new(vec![]),
        });
        let runner = test_runner(&db, renderer.clone());

        let result = runner.render_alert_attachment(&s).await;
        let note = result.expect_err("a foreign chart must be refused, not silently skipped");
        assert!(note.contains("does not belong to this workspace"), "{note}");
        assert!(
            renderer.seen.lock().unwrap().is_empty(),
            "the renderer must never be asked to render a foreign object — a \
             mint-then-403 or mint-then-404 still leaks the fact that a \
             render was ATTEMPTED, and the fix is to refuse before minting"
        );
        // Fix round 1's item 2: the test name claims no token was minted —
        // prove it against the table, not just the renderer's own seam.
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM automation.render_tokens")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(tokens, 0, "no render token may exist for a foreign object");
    }

    // ── fix round 1, item 1: the skip must reach schedule_runs.error ───────

    /// Returns one truthy row for whatever alert SQL is handed to it — enough
    /// for a `not_null` validator to fire. `render_alert_attachment` never
    /// calls this (it only mints a token and asks the renderer), so this is
    /// safe to share with a fixture that also exercises the attachment path.
    struct FiresOnce;
    #[async_trait::async_trait]
    impl QueryRunner for FiresOnce {
        async fn run(
            &self,
            _db: Uuid,
            _sql: &str,
            _identity: &RunIdentity,
            _limit: Option<usize>,
        ) -> Result<egret_core::TabularResult, String> {
            Ok(egret_core::TabularResult {
                columns: vec![egret_core::tabular::ColumnMeta {
                    name: "v".into(),
                    ty: "bigint".into(),
                }],
                rows: vec![vec![serde_json::json!(1)]],
                row_count: 1,
                bytes_scanned: None,
                truncated: false,
            })
        }
    }

    /// A minimal plaintext SMTP server — enough of RFC 5321 for `lettre`'s
    /// `AsyncSmtpTransport` (`TlsMode::None`, no auth) to consider a send
    /// successful, not a general-purpose mail server. Accepts exactly one
    /// connection, then exits.
    async fn fake_smtp() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (r, mut w) = stream.into_split();
            let mut lines = tokio::io::BufReader::new(r).lines();
            let _ = w.write_all(b"220 fake.local ESMTP\r\n").await;
            let mut in_data = false;
            while let Ok(Some(line)) = lines.next_line().await {
                if in_data {
                    if line == "." {
                        in_data = false;
                        let _ = w.write_all(b"250 2.0.0 OK: queued\r\n").await;
                    }
                    continue;
                }
                let upper = line.to_ascii_uppercase();
                let reply: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                    b"250-fake.local\r\n250 OK\r\n"
                } else if upper.starts_with("MAIL FROM") {
                    b"250 2.1.0 OK\r\n"
                } else if upper.starts_with("RCPT TO") {
                    b"250 2.1.5 OK\r\n"
                } else if upper == "DATA" {
                    in_data = true;
                    b"354 End data with <CR><LF>.<CR><LF>\r\n"
                } else if upper.starts_with("QUIT") {
                    let _ = w.write_all(b"221 2.0.0 Bye\r\n").await;
                    break;
                } else {
                    b"250 OK\r\n"
                };
                let _ = w.write_all(reply).await;
            }
        });
        addr
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_skipped_alert_attachment_lands_in_the_error_column_with_state_unchanged(
        pool: sqlx::PgPool,
    ) {
        // Fix round 1, item 1: a foreign attachment target used to be a
        // `tracing::warn!` only — invisible to a workspace-1 operator reading
        // `schedule_runs`, the one place this codebase treats as the answer
        // to "why didn't I get an email". This drives a REAL tick, through a
        // REAL (if minimal) SMTP send, and reads the row back.
        let db = Db { pool };
        let owner = mk_user(&db, "note").await;
        let ws1 = mk_workspace(&db, owner).await;
        let ws2 = mk_workspace(&db, owner).await;
        let foreign_chart = mk_chart(&db, ws2, owner).await;

        let database: Uuid = sqlx::query_scalar(
            "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
             VALUES ($1,'trino','localhost',8090,$2,false) RETURNING id",
        )
        .bind(format!("db-{}", Uuid::new_v4().simple()))
        .bind(ws1)
        .fetch_one(&db.pool)
        .await
        .unwrap();

        let schedule_id: Uuid = sqlx::query_scalar(
            "INSERT INTO automation.schedules \
               (workspace_id, type, name, crontab, next_run_at, database_id, sql, \
                validator_type, chart_id, report_format, grace_period_s) \
             VALUES ($1,'alert','note test','* * * * *', now() - interval '1 minute', \
                     $2,'SELECT 1','not_null',$3,'png',0) RETURNING id",
        )
        .bind(ws1)
        .bind(database)
        .bind(foreign_chart)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
            .bind(schedule_id)
            .bind(owner)
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
             VALUES ($1,'email',$2)",
        )
        .bind(schedule_id)
        .bind(serde_json::json!({"addresses": ["victim@example.com"]}))
        .execute(&db.pool)
        .await
        .unwrap();

        let smtp_addr = fake_smtp().await;
        sqlx::query(
            "INSERT INTO automation.smtp_config \
               (workspace_id, host, port, from_address, tls_mode) \
             VALUES ($1,$2,$3,'alerts@example.com','none')",
        )
        .bind(ws1)
        .bind(smtp_addr.ip().to_string())
        .bind(smtp_addr.port() as i32)
        .execute(&db.pool)
        .await
        .unwrap();

        let renderer = Arc::new(FakeRenderer {
            seen: Mutex::new(vec![]),
        });
        let runner = test_runner_with_query(&db, renderer.clone(), Arc::new(FiresOnce));

        assert_eq!(runner.tick_once().await, 1);

        let (state, error): (String, Option<String>) = sqlx::query_as(
            "SELECT state, error FROM automation.schedule_runs WHERE schedule_id = $1",
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            state, "success",
            "the alert itself must not be lost over its attachment"
        );
        let error = error.unwrap_or_default();
        assert!(
            error.contains("does not belong to this workspace"),
            "the run log's error column must explain the skip: {error}"
        );
        // And the email genuinely sent — the attachment being skipped must
        // not be confused with the mail failing to go out at all.
        assert!(
            renderer.seen.lock().unwrap().is_empty(),
            "the foreign chart must never reach the renderer"
        );
    }

    // ── fix-wave finding 1: dataapi_alert's email body claims no attached rows ──

    /// Fix-wave finding 1: `send_email` used to hardcode "The triggering rows
    /// are attached" for EVERY schedule type, including `dataapi_alert` —
    /// which never has rows, only a measured value — and pair it with a
    /// fabricated `column_1` CSV built from the query-result fallback path.
    /// `dataapi_alert_tick.rs`'s tick fixtures configure no SMTP, so
    /// `send_email` never got far enough to build that body; this proves the
    /// TEXT decision directly, without a live SMTP relay.
    #[test]
    fn a_dataapi_alert_body_says_nothing_about_attached_rows() {
        let body = alert_email_body("dataapi_alert", "Error spike", "0.72");
        assert!(
            !body.to_lowercase().contains("attached"),
            "a dataapi_alert has no rows to attach: {body:?}"
        );
        assert!(body.contains("Error spike"));
        assert!(body.contains("0.72"));
    }

    /// The unchanged half: an engine `alert` DOES attach a CSV of its
    /// triggering rows (`send_email`'s own `if s.r#type != "dataapi_alert"`
    /// guard), so its body must keep saying so.
    #[test]
    fn an_engine_alert_body_still_says_rows_are_attached() {
        let body = alert_email_body("alert", "Orders dropped", "3");
        assert!(body.contains("The triggering rows are attached."));
    }
}
