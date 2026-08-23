//! Domain→wire conversions and write-body validation.
//!
//! The wire DTOs themselves live in the leaf `egret-automation-api`; what stays
//! here is everything that touches an `egret-automation`-local type — the
//! `*_dto` conversions and `to_new`, which is where a create/update body is
//! checked BEFORE it reaches the database.

use std::collections::HashMap;

pub use egret_automation_api::{
    CreateSchedule, DataapiMetric, OwnerDto, RecipientDto, RecipientInput, RecipientKind,
    ReportFormat, RunDto, RunState, ScheduleDetailDto, ScheduleSummaryDto, ScheduleType,
    UpdateSchedule, ValidatorType,
};
use uuid::Uuid;

use crate::error::AutomationError;
use crate::repo::{NewSchedule, Owner, Recipient, Schedule, ScheduleRun};

/// Superset's defaults, applied when a body omits the advanced knobs. They
/// match migration 0110's column defaults; kept here too because the write path
/// always sends an explicit value, so the column default would otherwise never
/// apply.
const DEFAULT_GRACE_S: i32 = 14400;
const DEFAULT_WORKING_TIMEOUT_S: i32 = 3600;
const DEFAULT_LOG_RETENTION_DAYS: i32 = 90;

/// The upper bound on `dataapi_window_s` (fix-wave finding 2) — 30 days, the
/// widest of the four presets `egret_dataapi::activity::ActivityRange`
/// offers. Mirrors migration 0127's `schedule_dataapi_needs_source` CHECK; a
/// second source of truth, not a shared constant, because this crate does
/// not depend on `egret-dataapi` (see `runner.rs`'s dispatch doc comment) and
/// a CHECK cannot see a Rust `const` either way.
const MAX_DATAAPI_WINDOW_S: i32 = 2_592_000;

/// The fields a create and an update body have in common, so validation is
/// written once. A body-shaped trait rather than a merged struct because the
/// two DTOs differ in exactly one field (`type`, absent from update).
pub struct WriteBody<'a> {
    pub ty: ScheduleType,
    pub name: &'a str,
    pub description: &'a str,
    pub active: bool,
    pub crontab: &'a str,
    pub timezone: &'a str,
    pub database_id: Option<Uuid>,
    pub sql: Option<&'a str>,
    pub validator_type: Option<ValidatorType>,
    pub validator_config: Option<&'a serde_json::Value>,
    pub chart_id: Option<Uuid>,
    pub dashboard_id: Option<Uuid>,
    pub report_format: Option<ReportFormat>,
    pub custom_width: Option<i32>,
    pub custom_height: Option<i32>,
    pub dataapi_endpoint_id: Option<Uuid>,
    pub dataapi_metric: Option<DataapiMetric>,
    pub dataapi_error_code: Option<&'a str>,
    pub dataapi_window_s: Option<i32>,
    pub grace_period_s: Option<i32>,
    pub working_timeout_s: Option<i32>,
    pub log_retention_days: Option<i32>,
    pub email_subject: Option<&'a str>,
    pub message: Option<&'a str>,
    pub message_footer: Option<&'a str>,
    pub skip_if_empty: bool,
    pub targets: &'a [egret_automation_api::TargetInput],
    pub owners: &'a [Uuid],
    pub recipients: &'a [RecipientInput],
}

impl<'a> From<&'a CreateSchedule> for WriteBody<'a> {
    fn from(b: &'a CreateSchedule) -> Self {
        WriteBody {
            ty: b.ty,
            name: &b.name,
            description: &b.description,
            active: b.active,
            crontab: &b.crontab,
            timezone: &b.timezone,
            database_id: b.database_id,
            sql: b.sql.as_deref(),
            validator_type: b.validator_type,
            validator_config: b.validator_config.as_ref(),
            chart_id: b.chart_id,
            dashboard_id: b.dashboard_id,
            report_format: b.report_format,
            custom_width: b.custom_width,
            custom_height: b.custom_height,
            dataapi_endpoint_id: b.dataapi_endpoint_id,
            dataapi_metric: b.dataapi_metric,
            dataapi_error_code: b.dataapi_error_code.as_deref(),
            dataapi_window_s: b.dataapi_window_s,
            grace_period_s: b.grace_period_s,
            working_timeout_s: b.working_timeout_s,
            log_retention_days: b.log_retention_days,
            email_subject: b.email_subject.as_deref(),
            message: b.message.as_deref(),
            message_footer: b.message_footer.as_deref(),
            skip_if_empty: b.skip_if_empty,
            targets: &b.targets,
            owners: &b.owners,
            recipients: &b.recipients,
        }
    }
}

/// Update carries no `type`; the caller supplies the stored one, which is what
/// makes the type immutable rather than merely un-sent.
pub fn update_body<'a>(b: &'a UpdateSchedule, stored_type: ScheduleType) -> WriteBody<'a> {
    WriteBody {
        ty: stored_type,
        name: &b.name,
        description: &b.description,
        active: b.active,
        crontab: &b.crontab,
        timezone: &b.timezone,
        database_id: b.database_id,
        sql: b.sql.as_deref(),
        validator_type: b.validator_type,
        validator_config: b.validator_config.as_ref(),
        chart_id: b.chart_id,
        dashboard_id: b.dashboard_id,
        report_format: b.report_format,
        custom_width: b.custom_width,
        custom_height: b.custom_height,
        dataapi_endpoint_id: b.dataapi_endpoint_id,
        dataapi_metric: b.dataapi_metric,
        dataapi_error_code: b.dataapi_error_code.as_deref(),
        dataapi_window_s: b.dataapi_window_s,
        grace_period_s: b.grace_period_s,
        working_timeout_s: b.working_timeout_s,
        log_retention_days: b.log_retention_days,
        email_subject: b.email_subject.as_deref(),
        message: b.message.as_deref(),
        message_footer: b.message_footer.as_deref(),
        skip_if_empty: b.skip_if_empty,
        targets: &b.targets,
        owners: &b.owners,
        recipients: &b.recipients,
    }
}

/// Validate a write body and convert it to the repo's input.
///
/// Everything here is ALSO enforced by a CHECK constraint in migration 0110.
/// The duplication is deliberate: the constraint is the guarantee (nothing can
/// write a bad row, not even a future caller that forgets to validate), while
/// this is the error message — it names the field the user has to fix, which
/// `violates schedule_alert_needs_query` does not.
///
/// `creator` becomes the owner when the body names none: a schedule with no
/// owner has no identity to run its query as, so it must never be storable.
pub fn to_new(body: &WriteBody<'_>, creator: Option<Uuid>) -> Result<NewSchedule, AutomationError> {
    let bad = |m: &str| AutomationError::BadRequest(m.to_string());

    if body.name.trim().is_empty() {
        return Err(bad("name is required"));
    }
    // Rejects an unparseable crontab AND an unknown IANA zone.
    crate::cron::validate(body.crontab, body.timezone)?;
    // A subject placeholder is checked HERE, while its author is on the form.
    // Deferring it to send time would turn a typo into a lost notification.
    if let Some(subject) = body.email_subject {
        crate::subject::validate(subject).map_err(|m| bad(&m))?;
    }

    // Set only by the `DataapiAlert` arm, from a TRIMMED `dataapi_error_code`
    // — the same value the arm's own validation below checked, not a second,
    // untrimmed read of `body.dataapi_error_code` at construction time. A
    // whitespace-only `dataapi_error_code` on a `call_count` alert used to
    // pass this validation (trimmed-to-empty reads as "no error code") but
    // then persist the ORIGINAL untrimmed string, which is non-NULL and so
    // still trips `schedule_dataapi_error_code_needs_metric` at the database
    // — a 400 naming the CONSTRAINT, not the field, exactly what this
    // function exists to avoid.
    let mut dataapi_error_code: Option<String> = None;

    match body.ty {
        ScheduleType::Alert => {
            if body.database_id.is_none() {
                return Err(bad("an alert needs a database_id"));
            }
            if body.sql.map(str::trim).unwrap_or("").is_empty() {
                return Err(bad("an alert needs sql"));
            }
            if body.validator_type.is_none() {
                return Err(bad("an alert needs a validator_type"));
            }
            if body.validator_type == Some(ValidatorType::Operator) {
                validate_operator_config(body.validator_config).map_err(|m| bad(&m))?;
            }
        }
        ScheduleType::Report => {
            if body.report_format.is_none() {
                return Err(bad("a report needs a report_format"));
            }
            match (body.chart_id, body.dashboard_id) {
                (None, None) => return Err(bad("a report needs a chart_id or a dashboard_id")),
                (Some(_), Some(_)) => {
                    return Err(bad("a report takes a chart_id or a dashboard_id, not both"))
                }
                _ => {}
            }
        }
        // Mirrors migration 0127's `schedule_dataapi_needs_source`: a
        // `dataapi_alert` measures `dataapi.access_log` instead of running a
        // query, so it takes none of an engine alert's or a report's fields —
        // rejected by name here rather than silently dropped, so a client that
        // sent `sql` by mistake is told, not ignored.
        ScheduleType::DataapiAlert => {
            if body
                .sql
                .map(str::trim)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
            {
                return Err(bad("a dataapi_alert cannot carry sql"));
            }
            if body.database_id.is_some() {
                return Err(bad("a dataapi_alert cannot carry a database_id"));
            }
            if body.chart_id.is_some() || body.dashboard_id.is_some() {
                return Err(bad(
                    "a dataapi_alert cannot carry a chart_id or dashboard_id",
                ));
            }
            if !body.targets.is_empty() {
                return Err(bad("a dataapi_alert cannot carry targets"));
            }
            let Some(metric) = body.dataapi_metric else {
                return Err(bad("a dataapi_alert needs a dataapi_metric"));
            };
            let Some(window) = body.dataapi_window_s else {
                return Err(bad("a dataapi_alert needs a dataapi_window_s"));
            };
            if window <= 0 {
                return Err(bad("dataapi_window_s must be positive"));
            }
            if window > MAX_DATAAPI_WINDOW_S {
                return Err(bad(&format!(
                    "dataapi_window_s must be at most {MAX_DATAAPI_WINDOW_S} (30 days)"
                )));
            }
            // `schedule_dataapi_needs_source` pins `validator_type = 'operator'`
            // for every `dataapi_alert` row — required explicitly here, rather
            // than forced silently, so a client that sent `not_null` (or
            // nothing) is told which value is expected instead of having its
            // choice overridden without a trace.
            if body.validator_type != Some(ValidatorType::Operator) {
                return Err(bad("a dataapi_alert needs validator_type to be operator"));
            }
            validate_operator_config(body.validator_config).map_err(|m| bad(&m))?;
            // `schedule_dataapi_error_code_needs_metric` /
            // `schedule_dataapi_metric_needs_error_code`: the two
            // one-directional CHECKs, mirrored as one named field each way.
            // Trimmed ONCE, here — `dataapi_error_code` (the outer binding)
            // is what actually gets persisted below, so a whitespace-only
            // value cannot pass this check as "absent" and then reach the
            // database as a non-NULL string.
            dataapi_error_code = body
                .dataapi_error_code
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if metric == DataapiMetric::ErrorCodeCount && dataapi_error_code.is_none() {
                return Err(bad("error_code_count needs a dataapi_error_code"));
            }
            if metric != DataapiMetric::ErrorCodeCount && dataapi_error_code.is_some() {
                return Err(bad("dataapi_error_code is only valid for error_code_count"));
            }
        }
    }

    // A target is optional on an alert, but "both" is meaningless on either
    // type — the renderer has to know which object it is capturing.
    if body.ty == ScheduleType::Alert && body.chart_id.is_some() && body.dashboard_id.is_some() {
        return Err(bad("an alert takes a chart_id or a dashboard_id, not both"));
    }

    for r in body.recipients {
        validate_recipient(r).map_err(|m| bad(&m))?;
    }

    let grace = body.grace_period_s.unwrap_or(DEFAULT_GRACE_S);
    let working = body.working_timeout_s.unwrap_or(DEFAULT_WORKING_TIMEOUT_S);
    let retention = body
        .log_retention_days
        .unwrap_or(DEFAULT_LOG_RETENTION_DAYS);
    if grace < 0 {
        return Err(bad("grace_period_s cannot be negative"));
    }
    if working <= 0 {
        return Err(bad("working_timeout_s must be positive"));
    }
    if retention <= 0 {
        return Err(bad("log_retention_days must be positive"));
    }

    // An empty owner list means "whoever is creating this".
    let mut owners: Vec<Uuid> = body.owners.to_vec();
    if owners.is_empty() {
        owners.extend(creator);
    }
    if owners.is_empty() {
        return Err(bad("a schedule needs at least one owner"));
    }

    // Only the type's own block is persisted. Without this, editing an alert
    // that once had a chart_id would keep writing that chart_id back and trip
    // the report CHECK from the alert path.
    //
    // Two flags, not one `is_alert` — a `dataapi_alert` is not an `Alert`
    // (`body.ty == ScheduleType::Alert` is `false` for it), so a single
    // boolean here would null out `validator_type` for a `dataapi_alert` too,
    // even though `schedule_dataapi_needs_source` requires it to be
    // `'operator'`. `database_id`/`sql` are correctly `None` for a
    // `dataapi_alert` under `is_engine_alert` alone — it takes neither field,
    // same as a report — so only `validator_type` needed the second flag.
    let is_engine_alert = body.ty == ScheduleType::Alert;
    let is_dataapi_alert = body.ty == ScheduleType::DataapiAlert;
    Ok(NewSchedule {
        r#type: body.ty.as_str().to_string(),
        name: body.name.trim().to_string(),
        description: body.description.to_string(),
        active: body.active,
        crontab: body.crontab.to_string(),
        timezone: body.timezone.to_string(),
        database_id: is_engine_alert.then_some(body.database_id).flatten(),
        sql: is_engine_alert
            .then(|| body.sql.map(str::to_string))
            .flatten(),
        validator_type: (is_engine_alert || is_dataapi_alert)
            .then(|| body.validator_type.map(|v| v.as_str().to_string()))
            .flatten(),
        validator_config: body
            .validator_config
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        // An alert MAY now carry a target: when it fires, the mail attaches a
        // rendering of that chart or dashboard alongside the triggering rows,
        // which is what Superset does behind `ALERTS_ATTACH_REPORTS`. Reports
        // still require exactly one; for an alert it is optional decoration.
        // A `dataapi_alert` takes neither — validated above, so `body.chart_id`/
        // `body.dashboard_id` are already `None` by the time this runs.
        chart_id: body.chart_id,
        dashboard_id: body.dashboard_id,
        // Kept on an alert too — it is the format of the optional attachment.
        // Only meaningful alongside a target, which `render_alert_attachment`
        // checks before it renders anything.
        report_format: body.report_format.map(|f| f.as_str().to_string()),
        custom_width: body.custom_width,
        custom_height: body.custom_height,
        // `schedule_alert_no_dataapi_columns`'s converse: `None` for an
        // `alert` or a `report`, whatever the body sent — the same
        // silently-dropped-not-rejected treatment `chart_id`/`dashboard_id`
        // get on a `report` body above, since a stray `dataapi_metric` on an
        // ordinary alert is inert rather than a shape the caller is asking
        // for.
        dataapi_endpoint_id: is_dataapi_alert
            .then_some(body.dataapi_endpoint_id)
            .flatten(),
        dataapi_metric: is_dataapi_alert
            .then(|| body.dataapi_metric.map(|m| m.as_str().to_string()))
            .flatten(),
        // The TRIMMED value computed (and validated) in the `DataapiAlert`
        // arm above — `None` untouched for `Alert`/`Report`, since that arm
        // is the only place this outer binding is ever assigned.
        dataapi_error_code,
        dataapi_window_s: is_dataapi_alert.then_some(body.dataapi_window_s).flatten(),
        grace_period_s: grace,
        working_timeout_s: working,
        log_retention_days: retention,
        email_subject: body.email_subject.map(str::to_string),
        message: body.message.map(str::to_string),
        message_footer: body.message_footer.map(str::to_string),
        skip_if_empty: body.skip_if_empty,
        targets: body
            .targets
            .iter()
            .map(|t| (t.kind.clone(), t.object_id))
            .collect(),
        owners,
        recipients: body
            .recipients
            .iter()
            .map(|r| (r.kind.as_str().to_string(), r.config.clone()))
            .collect(),
    })
}

/// `operator` validators carry their comparison in `validator_config`; an
/// unknown operator would otherwise be stored happily and only surface at run
/// time, as an alert that never fires.
fn validate_operator_config(config: Option<&serde_json::Value>) -> Result<(), String> {
    const OPS: &[&str] = &["<", "<=", ">", ">=", "==", "!=", "between", "not between"];
    let cfg = config.ok_or("an operator validator needs a validator_config")?;
    let op = cfg
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or("validator_config needs an \"op\"")?;
    if !OPS.contains(&op) {
        return Err(format!("unknown operator: {op}"));
    }
    if op == "between" || op == "not between" {
        let lo = cfg.get("low").and_then(serde_json::Value::as_f64);
        let hi = cfg.get("high").and_then(serde_json::Value::as_f64);
        match (lo, hi) {
            (Some(lo), Some(hi)) if lo <= hi => Ok(()),
            (Some(_), Some(_)) => Err("validator_config low must be <= high".into()),
            _ => Err(format!("{op} needs numeric \"low\" and \"high\"")),
        }
    } else if cfg
        .get("threshold")
        .and_then(serde_json::Value::as_f64)
        .is_none()
    {
        Err(format!("{op} needs a numeric \"threshold\""))
    } else {
        Ok(())
    }
}

/// A recipient's `config` shape depends on its `kind`. Checked here rather than
/// at delivery time so a typo surfaces in the editor, not as a silent
/// non-delivery weeks later.
fn validate_recipient(r: &RecipientInput) -> Result<(), String> {
    match r.kind {
        RecipientKind::Email => {
            let addrs = r
                .config
                .get("addresses")
                .and_then(|v| v.as_array())
                .ok_or("an email recipient needs \"addresses\"")?;
            if addrs.is_empty() {
                return Err("an email recipient needs at least one address".into());
            }
            for a in addrs {
                let s = a.as_str().ok_or("addresses must be strings")?;
                // Deliberately shallow: full RFC 5322 validation belongs to the
                // SMTP server, and rejecting an address it would have accepted
                // is worse than passing one on.
                if !s.contains('@') || s.starts_with('@') || s.ends_with('@') {
                    return Err(format!("not an email address: {s}"));
                }
            }
            Ok(())
        }
        RecipientKind::Slack => {
            let channel = r
                .config
                .get("channel")
                .and_then(|v| v.as_str())
                .ok_or("a slack recipient needs a \"channel\"")?;
            if channel.trim().is_empty() {
                return Err("a slack channel cannot be empty".into());
            }
            Ok(())
        }
        RecipientKind::Webhook => {
            let url = r
                .config
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or("a webhook recipient needs a \"url\"")?;
            // Scheme only. The SSRF check that matters runs AFTER DNS
            // resolution, at delivery time (P2, §5.4) — a name that resolves
            // into a private range cannot be caught here, and pretending
            // otherwise is how a check gets trusted for something it never did.
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("webhook url must be http(s)".into());
            }
            Ok(())
        }
    }
}

// ── domain → wire ─────────────────────────────────────────────────────────

/// What a client is allowed to see of a recipient's `config`.
///
/// A webhook's signing secret lives in the vault and its `secret_id` is a
/// reference the editor has no use for — handing one out only invites a fetch
/// nobody intended. Both are replaced by a boolean, the same shape the SMTP
/// config uses for its password (`has_password`).
///
/// This is an ALLOW-LIST, not a redaction: the two keys a webhook may show are
/// named, and anything else in the row — including a cleartext `secret` left by
/// a row written before this existed — is dropped rather than passed through.
fn redact_recipient_config(kind: &str, config: &serde_json::Value) -> serde_json::Value {
    // An ALLOW-LIST per kind, not "webhook gets special treatment". The webhook
    // branch was the only one, so a slack recipient went out verbatim — which
    // meant the response carried `token_id`, the vault reference the webhook
    // branch deliberately hides behind `has_secret`, and never carried
    // `has_token`, which the editor reads to decide whether to say "a token is
    // stored". The two credential paths now report themselves the same way.
    let get = |k: &str| config.get(k).cloned().unwrap_or(serde_json::Value::Null);
    let mut out = serde_json::Map::new();
    match kind {
        "webhook" => {
            out.insert("url".into(), get("url"));
            out.insert(
                "has_secret".into(),
                serde_json::Value::Bool(
                    config.get("secret_id").is_some() || config.get("secret").is_some(),
                ),
            );
        }
        "slack" => {
            out.insert("channel".into(), get("channel"));
            out.insert(
                "has_token".into(),
                serde_json::Value::Bool(
                    config.get("token_id").is_some() || config.get("token").is_some(),
                ),
            );
        }
        "email" => {
            out.insert("addresses".into(), get("addresses"));
        }
        // A kind this build does not know: emit nothing rather than pass an
        // unreviewed object through. A future kind is one that has not been
        // read for credentials yet, and defaulting to disclosure is how the
        // slack case happened.
        _ => {}
    }
    serde_json::Value::Object(out)
}

fn owner_dto(o: &Owner) -> OwnerDto {
    OwnerDto {
        user_id: o.user_id.to_string(),
        email: o.email.clone(),
    }
}

/// `ScheduleType::parse(...).unwrap_or(ScheduleType::Alert)` appears on both
/// DTOs below. The fallback is unreachable while `schedule_type_valid` holds
/// (Task 5 review question: does a third variant change what it should
/// default to? Decision: no — it is dead code either way, reachable only for
/// a corrupt row the CHECK should have already refused, and `Alert` was
/// already the existing choice for that case before this task. Changing it to
/// `DataapiAlert` would not make an unreachable branch any more correct, only
/// different.) What DOES change here: before this task, a REAL
/// `dataapi_alert` row also fell into this fallback, because `parse` did not
/// recognise the string yet — so a live `dataapi_alert` was serialised to the
/// client as `"type":"alert"`. That bug is fixed by `parse` gaining the
/// `dataapi_alert` arm above, not by touching this fallback.
pub fn summary_dto(
    s: &Schedule,
    owners: &HashMap<Uuid, Vec<Owner>>,
    can_manage: bool,
    bookmarked: bool,
) -> ScheduleSummaryDto {
    ScheduleSummaryDto {
        id: s.id.to_string(),
        ty: ScheduleType::parse(&s.r#type).unwrap_or(ScheduleType::Alert),
        name: s.name.clone(),
        description: s.description.clone(),
        active: s.active,
        crontab: s.crontab.clone(),
        timezone: s.timezone.clone(),
        next_run_at: s.next_run_at,
        last_state: s.last_state.as_deref().and_then(RunState::parse),
        last_eval_at: s.last_eval_at,
        owners: owners
            .get(&s.id)
            .map(|v| v.iter().map(owner_dto).collect())
            .unwrap_or_default(),
        updated_at: s.updated_at,
        can_manage,
        bookmarked,
        folder_id: s.folder_id.map(|v| v.to_string()),
        deleted_at: s.deleted_at,
    }
}

pub fn detail_dto(
    s: &Schedule,
    owners: &[Owner],
    recipients: &[Recipient],
    targets: &[crate::repo::Target],
) -> ScheduleDetailDto {
    ScheduleDetailDto {
        id: s.id.to_string(),
        ty: ScheduleType::parse(&s.r#type).unwrap_or(ScheduleType::Alert),
        name: s.name.clone(),
        description: s.description.clone(),
        active: s.active,
        crontab: s.crontab.clone(),
        timezone: s.timezone.clone(),
        next_run_at: s.next_run_at,
        database_id: s.database_id.map(|v| v.to_string()),
        sql: s.sql.clone(),
        validator_type: s.validator_type.as_deref().and_then(ValidatorType::parse),
        validator_config: s.validator_config.clone(),
        last_state: s.last_state.as_deref().and_then(RunState::parse),
        last_eval_at: s.last_eval_at,
        last_value: s.last_value,
        chart_id: s.chart_id.map(|v| v.to_string()),
        dashboard_id: s.dashboard_id.map(|v| v.to_string()),
        report_format: s.report_format.as_deref().and_then(ReportFormat::parse),
        custom_width: s.custom_width,
        custom_height: s.custom_height,
        dataapi_endpoint_id: s.dataapi_endpoint_id.map(|v| v.to_string()),
        dataapi_metric: s.dataapi_metric.as_deref().and_then(DataapiMetric::parse),
        dataapi_error_code: s.dataapi_error_code.clone(),
        dataapi_window_s: s.dataapi_window_s,
        grace_period_s: s.grace_period_s,
        working_timeout_s: s.working_timeout_s,
        log_retention_days: s.log_retention_days,
        email_subject: s.email_subject.clone(),
        message: s.message.clone(),
        message_footer: s.message_footer.clone(),
        skip_if_empty: s.skip_if_empty,
        targets: targets
            .iter()
            .map(|t| egret_automation_api::TargetInput {
                kind: t.kind.clone(),
                object_id: t.object_id,
            })
            .collect(),
        owners: owners.iter().map(owner_dto).collect(),
        recipients: recipients
            .iter()
            .map(|r| RecipientDto {
                id: r.id.to_string(),
                kind: RecipientKind::parse(&r.kind).unwrap_or(RecipientKind::Email),
                config: redact_recipient_config(&r.kind, &r.config),
            })
            .collect(),
        created_by: s.created_by.map(|v| v.to_string()),
        updated_at: s.updated_at,
    }
}

/// The object a report renders, and the `kind` string the render-token
/// vocabulary and print-page routes use for it (`chart`|`dashboard` — mirrors
/// migration 0112's `render_token_kind_valid` CHECK).
///
/// `None` only for a corrupt row: migration 0110's report CHECK guarantees a
/// real report schedule carries exactly one of `chart_id`/`dashboard_id`.
pub fn report_target(s: &Schedule) -> Option<(&'static str, Uuid)> {
    if let Some(id) = s.chart_id {
        Some(("chart", id))
    } else if let Some(id) = s.dashboard_id {
        Some(("dashboard", id))
    } else {
        None
    }
}

pub fn run_dto(r: &ScheduleRun) -> RunDto {
    RunDto {
        id: r.id.to_string(),
        state: RunState::parse(&r.state).unwrap_or(RunState::Error),
        started_at: r.started_at,
        ended_at: r.ended_at,
        value: r.value,
        error: r.error.clone(),
        notification_sent_at: r.notification_sent_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert_body() -> CreateSchedule {
        serde_json::from_value(serde_json::json!({
            "type": "alert",
            "name": "orders",
            "crontab": "0 9 * * *",
            "database_id": Uuid::new_v4(),
            "sql": "SELECT 1",
            "validator_type": "not_null",
        }))
        .unwrap()
    }

    fn err_of(b: &CreateSchedule) -> String {
        match to_new(&WriteBody::from(b), Some(Uuid::new_v4())) {
            Err(AutomationError::BadRequest(m)) => m,
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    /// A well-formed `dataapi_alert` body — the baseline every rejection test
    /// below perturbs exactly one field away from, matching
    /// `dataapi_alert_schema.rs`'s `well_formed()` on the database side.
    fn dataapi_alert_body() -> CreateSchedule {
        serde_json::from_value(serde_json::json!({
            "type": "dataapi_alert",
            "name": "traffic",
            "crontab": "*/5 * * * *",
            "validator_type": "operator",
            "validator_config": {"op": ">", "threshold": 0},
            "dataapi_metric": "call_count",
            "dataapi_window_s": 300,
        }))
        .unwrap()
    }

    #[test]
    fn a_valid_alert_converts() {
        let n = to_new(&WriteBody::from(&alert_body()), Some(Uuid::new_v4())).unwrap();
        assert_eq!(n.r#type, "alert");
        assert_eq!(n.owners.len(), 1, "the creator becomes the owner");
        assert_eq!(n.grace_period_s, DEFAULT_GRACE_S);
    }

    #[test]
    fn an_alert_missing_its_query_names_the_field_not_the_constraint() {
        // The CHECK would also reject this; the point of validating here is
        // that the user is told which field to fix.
        let mut b = alert_body();
        b.sql = None;
        assert_eq!(err_of(&b), "an alert needs sql");
        let mut b = alert_body();
        b.database_id = None;
        assert_eq!(err_of(&b), "an alert needs a database_id");
        let mut b = alert_body();
        b.validator_type = None;
        assert_eq!(err_of(&b), "an alert needs a validator_type");
    }

    #[test]
    fn whitespace_only_sql_is_as_empty_as_none() {
        let mut b = alert_body();
        b.sql = Some("   \n ".into());
        assert_eq!(err_of(&b), "an alert needs sql");
    }

    #[test]
    fn a_report_needs_exactly_one_target() {
        let mut b = alert_body();
        b.ty = ScheduleType::Report;
        b.report_format = Some(ReportFormat::Png);
        assert_eq!(err_of(&b), "a report needs a chart_id or a dashboard_id");

        b.chart_id = Some(Uuid::new_v4());
        b.dashboard_id = Some(Uuid::new_v4());
        assert_eq!(
            err_of(&b),
            "a report takes a chart_id or a dashboard_id, not both"
        );

        b.dashboard_id = None;
        assert!(to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).is_ok());
    }

    #[test]
    fn an_alert_may_carry_a_target_for_its_attachment() {
        // It used to be nulled out, on the belief that an alert with a
        // chart_id would trip the report CHECK. It does not:
        // `schedule_report_has_format` is guarded by `type <> 'report'`.
        // Superset allows the same thing behind ALERTS_ATTACH_REPORTS, and it
        // is what lets a fired alert mail a picture rather than only numbers.
        let mut b = alert_body();
        let chart = Uuid::new_v4();
        b.chart_id = Some(chart);
        b.report_format = Some(ReportFormat::Png);
        let n = to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).unwrap();
        assert_eq!(n.chart_id, Some(chart));
        assert_eq!(n.report_format.as_deref(), Some("png"));
    }

    #[test]
    fn an_alert_naming_both_targets_is_rejected() {
        // Optional does not mean ambiguous: the renderer has to know which
        // object it is capturing.
        let mut b = alert_body();
        b.chart_id = Some(Uuid::new_v4());
        b.dashboard_id = Some(Uuid::new_v4());
        let err = to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
    }

    #[test]
    fn an_alerts_query_block_still_wins_over_a_reports() {
        // The alert-only fields survive and the report-only requirement does
        // not apply — only the target is now shared between the two types.
        let b = alert_body();
        let n = to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).unwrap();
        assert!(n.sql.is_some() && n.database_id.is_some() && n.validator_type.is_some());
    }

    #[test]
    fn an_unparseable_crontab_is_rejected_before_the_database_sees_it() {
        let mut b = alert_body();
        b.crontab = "every tuesday".into();
        assert!(err_of(&b).contains("invalid crontab"));
    }

    #[test]
    fn an_unknown_timezone_is_rejected() {
        let mut b = alert_body();
        b.timezone = "Mars/Olympus".into();
        assert!(err_of(&b).contains("unknown timezone"));
    }

    #[test]
    fn an_operator_validator_must_carry_a_usable_comparison() {
        let mut b = alert_body();
        b.validator_type = Some(ValidatorType::Operator);
        assert_eq!(err_of(&b), "an operator validator needs a validator_config");

        b.validator_config = Some(serde_json::json!({"op": "≥", "threshold": 1}));
        assert_eq!(err_of(&b), "unknown operator: ≥");

        b.validator_config = Some(serde_json::json!({"op": ">"}));
        assert_eq!(err_of(&b), "> needs a numeric \"threshold\"");

        b.validator_config = Some(serde_json::json!({"op": "between", "low": 10, "high": 1}));
        assert_eq!(err_of(&b), "validator_config low must be <= high");

        b.validator_config = Some(serde_json::json!({"op": "between", "low": 1, "high": 10}));
        assert!(to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).is_ok());
    }

    #[test]
    fn recipient_config_is_checked_against_its_kind() {
        let mut b = alert_body();
        b.recipients = vec![serde_json::from_value(serde_json::json!({
            "kind": "email", "config": {}
        }))
        .unwrap()];
        assert_eq!(err_of(&b), "an email recipient needs \"addresses\"");

        b.recipients = vec![serde_json::from_value(serde_json::json!({
            "kind": "email", "config": {"addresses": []}
        }))
        .unwrap()];
        assert_eq!(err_of(&b), "an email recipient needs at least one address");

        b.recipients = vec![serde_json::from_value(serde_json::json!({
            "kind": "webhook", "config": {"url": "ftp://internal/x"}
        }))
        .unwrap()];
        assert_eq!(err_of(&b), "webhook url must be http(s)");
    }

    #[test]
    fn a_schedule_with_no_owner_and_no_creator_is_refused() {
        // P2 runs the query as an owner; a row with none has no identity to
        // run as, and would fail every tick forever.
        let b = alert_body();
        assert_eq!(
            match to_new(&WriteBody::from(&b), None) {
                Err(AutomationError::BadRequest(m)) => m,
                other => panic!("expected BadRequest, got {other:?}"),
            },
            "a schedule needs at least one owner"
        );
    }

    #[test]
    fn nonsensical_timers_are_refused() {
        let mut b = alert_body();
        b.working_timeout_s = Some(0);
        assert_eq!(err_of(&b), "working_timeout_s must be positive");
        let mut b = alert_body();
        b.log_retention_days = Some(0);
        assert_eq!(err_of(&b), "log_retention_days must be positive");
        let mut b = alert_body();
        b.grace_period_s = Some(-1);
        assert_eq!(err_of(&b), "grace_period_s cannot be negative");
    }

    #[test]
    fn a_blank_name_is_refused() {
        let mut b = alert_body();
        b.name = "   ".into();
        assert_eq!(err_of(&b), "name is required");
    }

    // ── dataapi_alert (Task 5) ───────────────────────────────────────────
    //
    // Each test below pairs with one of migration 0127's four CHECKs, proven
    // against a real database in `tests/dataapi_alert_schema.rs`. These prove
    // the OTHER layer: the same malformed shape gets a 400 naming the field
    // here, before a row is ever attempted.

    #[test]
    fn a_valid_dataapi_alert_converts() {
        let n = to_new(
            &WriteBody::from(&dataapi_alert_body()),
            Some(Uuid::new_v4()),
        )
        .unwrap();
        assert_eq!(n.r#type, "dataapi_alert");
        assert_eq!(n.dataapi_metric.as_deref(), Some("call_count"));
        assert_eq!(n.dataapi_window_s, Some(300));
        assert_eq!(n.validator_type.as_deref(), Some("operator"));
        assert!(
            n.database_id.is_none() && n.sql.is_none(),
            "a dataapi_alert must carry neither: {n:?}"
        );
        assert_eq!(n.owners.len(), 1, "the creator becomes the owner");
    }

    /// Mirrors `schedule_dataapi_needs_source`'s `dataapi_metric IS NOT NULL
    /// AND dataapi_window_s IS NOT NULL AND dataapi_window_s > 0` clause —
    /// proven rejected by the database in
    /// `dataapi_alert_with_null_metric_is_rejected`.
    #[test]
    fn a_dataapi_alert_missing_its_metric_or_window_names_the_field() {
        let mut b = dataapi_alert_body();
        b.dataapi_metric = None;
        assert_eq!(err_of(&b), "a dataapi_alert needs a dataapi_metric");

        let mut b = dataapi_alert_body();
        b.dataapi_window_s = None;
        assert_eq!(err_of(&b), "a dataapi_alert needs a dataapi_window_s");

        let mut b = dataapi_alert_body();
        b.dataapi_window_s = Some(0);
        assert_eq!(err_of(&b), "dataapi_window_s must be positive");

        let mut b = dataapi_alert_body();
        b.dataapi_window_s = Some(-60);
        assert_eq!(err_of(&b), "dataapi_window_s must be positive");
    }

    /// Fix-wave finding 2: `dataapi_window_s` was bounded below (`> 0`) but
    /// not above, so nothing stopped a "90-day error rate" that silently
    /// measures whatever survived `access_log` retention — a threshold
    /// decision made unattended every tick, unlike the Activity page's own
    /// range picker, which disables a preset that reaches past retention
    /// rather than let someone misread the thinning tail as a real drop in
    /// traffic. Mirrors migration 0127's `dataapi_window_s <= 2592000`.
    #[test]
    fn a_dataapi_alert_window_over_thirty_days_names_the_field() {
        let mut b = dataapi_alert_body();
        b.dataapi_window_s = Some(MAX_DATAAPI_WINDOW_S);
        assert!(
            to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).is_ok(),
            "the bound itself must still be accepted"
        );

        let mut b = dataapi_alert_body();
        b.dataapi_window_s = Some(MAX_DATAAPI_WINDOW_S + 1);
        assert_eq!(
            err_of(&b),
            "dataapi_window_s must be at most 2592000 (30 days)"
        );
    }

    /// Mirrors `schedule_dataapi_needs_source`'s `validator_type = 'operator'`
    /// clause. `not_null` is the OTHER value this same field accepts on an
    /// ordinary alert, which is exactly why a `dataapi_alert` needs its own
    /// check rather than reusing the alert branch's "needs a validator_type"
    /// (present-but-wrong is not the same failure as absent).
    #[test]
    fn a_dataapi_alert_needs_validator_type_operator() {
        let mut b = dataapi_alert_body();
        b.validator_type = Some(ValidatorType::NotNull);
        assert_eq!(
            err_of(&b),
            "a dataapi_alert needs validator_type to be operator"
        );

        let mut b = dataapi_alert_body();
        b.validator_type = None;
        assert_eq!(
            err_of(&b),
            "a dataapi_alert needs validator_type to be operator"
        );
    }

    /// The condition itself still has to be usable — same
    /// `validate_operator_config` an ordinary `operator` alert goes through.
    #[test]
    fn a_dataapi_alerts_condition_is_checked_like_an_operator_alerts() {
        let mut b = dataapi_alert_body();
        b.validator_config = None;
        assert_eq!(err_of(&b), "an operator validator needs a validator_config");
    }

    /// Mirrors `schedule_dataapi_error_code_needs_metric` /
    /// `schedule_dataapi_metric_needs_error_code` — the two one-directional
    /// CHECKs, proven rejected by the database in
    /// `error_code_without_error_code_count_metric_is_rejected` and
    /// `error_code_count_metric_without_error_code_is_rejected`.
    #[test]
    fn error_code_count_pairing_is_checked_in_rust_too() {
        let mut b = dataapi_alert_body();
        b.dataapi_metric = Some(DataapiMetric::ErrorCodeCount);
        b.dataapi_error_code = None;
        assert_eq!(err_of(&b), "error_code_count needs a dataapi_error_code");

        let mut b = dataapi_alert_body();
        b.dataapi_error_code = Some("QUERY_TIMEOUT".into());
        assert_eq!(
            err_of(&b),
            "dataapi_error_code is only valid for error_code_count"
        );

        let mut b = dataapi_alert_body();
        b.dataapi_metric = Some(DataapiMetric::ErrorCodeCount);
        b.dataapi_error_code = Some("QUERY_TIMEOUT".into());
        assert!(to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).is_ok());
    }

    /// Fix-round-1, Minor 3: a whitespace-only `dataapi_error_code` used to
    /// read as "absent" for the CHECK-mirroring validation above (trimmed to
    /// empty) but then persist the ORIGINAL, untrimmed, non-NULL string —
    /// which still violates `schedule_dataapi_error_code_needs_metric` at
    /// the database, surfacing as a 400 naming the CONSTRAINT rather than a
    /// 400 naming the FIELD, which is exactly what this whole function
    /// exists to avoid.
    #[test]
    fn a_whitespace_only_error_code_is_not_persisted_verbatim() {
        let mut b = dataapi_alert_body();
        b.dataapi_error_code = Some("   ".into());
        let n = to_new(&WriteBody::from(&b), Some(Uuid::new_v4())).unwrap();
        assert!(
            n.dataapi_error_code.is_none(),
            "a whitespace-only error_code must not reach NewSchedule verbatim: {n:?}"
        );
    }

    /// Mirrors `schedule_dataapi_needs_source`'s `database_id IS NULL AND sql
    /// IS NULL` clause — the load-bearing half (per migration 0127's own
    /// comment) that makes a row both evaluators claim as theirs impossible to
    /// write. Proven rejected by the database in
    /// `dataapi_alert_carrying_sql_is_rejected` /
    /// `dataapi_alert_carrying_database_id_is_rejected`.
    #[test]
    fn a_dataapi_alert_cannot_carry_sql_or_a_database_id() {
        let mut b = dataapi_alert_body();
        b.sql = Some("SELECT 1".into());
        assert_eq!(err_of(&b), "a dataapi_alert cannot carry sql");

        let mut b = dataapi_alert_body();
        b.database_id = Some(Uuid::new_v4());
        assert_eq!(err_of(&b), "a dataapi_alert cannot carry a database_id");
    }

    /// The rest of `schedule_dataapi_needs_source`'s `chart_id IS NULL AND
    /// dashboard_id IS NULL` clause. Not carried in Task 1's DB-test list
    /// (which only exercised `sql`/`database_id`), so this is proven ONLY
    /// here — see the report for why that DB-side pair was not duplicated.
    #[test]
    fn a_dataapi_alert_cannot_carry_a_target() {
        let mut b = dataapi_alert_body();
        b.chart_id = Some(Uuid::new_v4());
        assert_eq!(
            err_of(&b),
            "a dataapi_alert cannot carry a chart_id or dashboard_id"
        );

        let mut b = dataapi_alert_body();
        b.targets = vec![serde_json::from_value(serde_json::json!({
            "kind": "chart", "object_id": Uuid::new_v4(),
        }))
        .unwrap()];
        assert_eq!(err_of(&b), "a dataapi_alert cannot carry targets");
    }

    /// Mirrors `schedule_dataapi_metric_valid`. Enforced at the JSON boundary
    /// by `DataapiMetric` being a typed enum rather than a bare `String` — an
    /// unknown metric fails to DESERIALIZE, so `to_new` never even runs. This
    /// is still a 4xx that names the field: `serde`'s own message quotes the
    /// bad value and the field it was decoding.
    #[test]
    fn an_unknown_dataapi_metric_fails_to_deserialize_before_validation_even_runs() {
        let err = serde_json::from_value::<CreateSchedule>(serde_json::json!({
            "type": "dataapi_alert",
            "name": "traffic",
            "crontab": "*/5 * * * *",
            "validator_type": "operator",
            "dataapi_metric": "no_such_metric",
            "dataapi_window_s": 300,
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("no_such_metric"),
            "expected the bad value to be named, got: {err}"
        );
    }
}
