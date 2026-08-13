//! Domain→wire conversions and write-body validation.
//!
//! The wire DTOs themselves live in the leaf `egret-automation-api`; what stays
//! here is everything that touches an `egret-automation`-local type — the
//! `*_dto` conversions and `to_new`, which is where a create/update body is
//! checked BEFORE it reaches the database.

use std::collections::HashMap;

pub use egret_automation_api::{
    CreateSchedule, OwnerDto, RecipientDto, RecipientInput, RecipientKind, ReportFormat, RunDto,
    RunState, ScheduleDetailDto, ScheduleSummaryDto, ScheduleType, UpdateSchedule, ValidatorType,
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
    pub grace_period_s: Option<i32>,
    pub working_timeout_s: Option<i32>,
    pub log_retention_days: Option<i32>,
    pub email_subject: Option<&'a str>,
    pub message: Option<&'a str>,
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
            grace_period_s: b.grace_period_s,
            working_timeout_s: b.working_timeout_s,
            log_retention_days: b.log_retention_days,
            email_subject: b.email_subject.as_deref(),
            message: b.message.as_deref(),
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
        grace_period_s: b.grace_period_s,
        working_timeout_s: b.working_timeout_s,
        log_retention_days: b.log_retention_days,
        email_subject: b.email_subject.as_deref(),
        message: b.message.as_deref(),
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
    let is_alert = body.ty == ScheduleType::Alert;
    Ok(NewSchedule {
        r#type: body.ty.as_str().to_string(),
        name: body.name.trim().to_string(),
        description: body.description.to_string(),
        active: body.active,
        crontab: body.crontab.to_string(),
        timezone: body.timezone.to_string(),
        database_id: is_alert.then_some(body.database_id).flatten(),
        sql: is_alert.then(|| body.sql.map(str::to_string)).flatten(),
        validator_type: is_alert
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
        chart_id: body.chart_id,
        dashboard_id: body.dashboard_id,
        // Kept on an alert too — it is the format of the optional attachment.
        // Only meaningful alongside a target, which `render_alert_attachment`
        // checks before it renders anything.
        report_format: body.report_format.map(|f| f.as_str().to_string()),
        custom_width: body.custom_width,
        custom_height: body.custom_height,
        grace_period_s: grace,
        working_timeout_s: working,
        log_retention_days: retention,
        email_subject: body.email_subject.map(str::to_string),
        message: body.message.map(str::to_string),
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
    if kind != "webhook" {
        return config.clone();
    }
    let mut out = serde_json::Map::new();
    out.insert(
        "url".into(),
        config
            .get("url")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    out.insert(
        "has_secret".into(),
        serde_json::Value::Bool(
            config.get("secret_id").is_some() || config.get("secret").is_some(),
        ),
    );
    serde_json::Value::Object(out)
}

fn owner_dto(o: &Owner) -> OwnerDto {
    OwnerDto {
        user_id: o.user_id.to_string(),
        email: o.email.clone(),
    }
}

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
        grace_period_s: s.grace_period_s,
        working_timeout_s: s.working_timeout_s,
        log_retention_days: s.log_retention_days,
        email_subject: s.email_subject.clone(),
        message: s.message.clone(),
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
        // `schedule_report_needs_target` is guarded by `type <> 'report'`.
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
}
