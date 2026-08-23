//! Pure-serde wire DTOs for the Alerts & Reports seam (`automation.schedules`).
//!
//! Alerts and reports share one table and one wire shape, as they do in
//! Superset: they differ in which optional block is populated (`alert_*` vs
//! `report_*`), not in structure. Keeping them one type is what lets the list
//! page, the editor and the run-log drawer be one implementation each.
//!
//! The type-dependent fields are `Option` here and enforced as CHECK
//! constraints in migration 0110 plus explicit validation in
//! `egret-automation`'s routes, so an invalid combination is a 400 rather than
//! a row that can only fail at run time.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a schedule does: evaluate a query (`alert`), deliver a rendering of a
/// saved object (`report`), or watch a Data API endpoint's traffic
/// (`dataapi_alert`, migration 0128).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleType {
    Alert,
    Report,
    /// `#[serde(rename_all = "lowercase")]` would spell this "dataapialert" —
    /// no underscore — so it is overridden explicitly to match the stored
    /// vocabulary (`schedule_type_valid`, migration 0128).
    #[serde(rename = "dataapi_alert")]
    DataapiAlert,
}

impl ScheduleType {
    /// The stored spelling. Kept as an explicit method rather than relying on
    /// `Debug`, because these strings are the CHECK-constrained database
    /// vocabulary and must not drift with a formatting change.
    pub fn as_str(self) -> &'static str {
        match self {
            ScheduleType::Alert => "alert",
            ScheduleType::Report => "report",
            ScheduleType::DataapiAlert => "dataapi_alert",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "alert" => Some(ScheduleType::Alert),
            "report" => Some(ScheduleType::Report),
            "dataapi_alert" => Some(ScheduleType::DataapiAlert),
            _ => None,
        }
    }
}

/// The Data API metric a `dataapi_alert` watches — the closed vocabulary of
/// migration 0128's `schedule_dataapi_metric_valid` CHECK.
///
/// A typed enum here, unlike the raw `&str` `egret-automation::dataapi_metric`
/// matches on internally: an unknown metric then fails to DESERIALIZE — a 4xx
/// the JSON layer names before `to_new`'s validation ever runs — rather than
/// reaching Rust as an unchecked string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub enum DataapiMetric {
    #[serde(rename = "error_rate")]
    ErrorRate,
    #[serde(rename = "error_count")]
    ErrorCount,
    #[serde(rename = "p50_duration_ms")]
    P50DurationMs,
    #[serde(rename = "p95_duration_ms")]
    P95DurationMs,
    #[serde(rename = "call_count")]
    CallCount,
    #[serde(rename = "error_code_count")]
    ErrorCodeCount,
}

impl DataapiMetric {
    /// The stored spelling — also the exact string `dataapi_metric::measure`
    /// matches on, so a rename here without a matching change there silently
    /// stops an alert from ever firing.
    pub fn as_str(self) -> &'static str {
        match self {
            DataapiMetric::ErrorRate => "error_rate",
            DataapiMetric::ErrorCount => "error_count",
            DataapiMetric::P50DurationMs => "p50_duration_ms",
            DataapiMetric::P95DurationMs => "p95_duration_ms",
            DataapiMetric::CallCount => "call_count",
            DataapiMetric::ErrorCodeCount => "error_code_count",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "error_rate" => Some(DataapiMetric::ErrorRate),
            "error_count" => Some(DataapiMetric::ErrorCount),
            "p50_duration_ms" => Some(DataapiMetric::P50DurationMs),
            "p95_duration_ms" => Some(DataapiMetric::P95DurationMs),
            "call_count" => Some(DataapiMetric::CallCount),
            "error_code_count" => Some(DataapiMetric::ErrorCodeCount),
            _ => None,
        }
    }
}

/// How an alert's query result becomes a fire/don't-fire decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorType {
    /// Fire when the value is non-null and non-zero (Superset's rule).
    NotNull,
    /// Fire when `value <op> threshold`, per `validator_config`.
    Operator,
}

impl ValidatorType {
    pub fn as_str(self) -> &'static str {
        match self {
            ValidatorType::NotNull => "not_null",
            ValidatorType::Operator => "operator",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "not_null" => Some(ValidatorType::NotNull),
            "operator" => Some(ValidatorType::Operator),
            _ => None,
        }
    }
}

/// The rendering a report delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    Png,
    Pdf,
    Csv,
    Text,
}

impl ReportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportFormat::Png => "png",
            ReportFormat::Pdf => "pdf",
            ReportFormat::Csv => "csv",
            ReportFormat::Text => "text",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "png" => Some(ReportFormat::Png),
            "pdf" => Some(ReportFormat::Pdf),
            "csv" => Some(ReportFormat::Csv),
            "text" => Some(ReportFormat::Text),
            _ => None,
        }
    }
}

/// Superset's five run states, verbatim. They are the vocabulary that makes the
/// list page legible — in particular `noop` ("ran, condition false") and `grace`
/// ("would have fired, suppressed") are the two answers to "why didn't I get an
/// email?", and collapsing either into `success` loses that answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Success,
    Working,
    Error,
    Noop,
    Grace,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Success => "success",
            RunState::Working => "working",
            RunState::Error => "error",
            RunState::Noop => "noop",
            RunState::Grace => "grace",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "success" => Some(RunState::Success),
            "working" => Some(RunState::Working),
            "error" => Some(RunState::Error),
            "noop" => Some(RunState::Noop),
            "grace" => Some(RunState::Grace),
            _ => None,
        }
    }
}

/// Where a firing schedule delivers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecipientKind {
    Email,
    Webhook,
    Slack,
}

impl RecipientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RecipientKind::Email => "email",
            RecipientKind::Webhook => "webhook",
            RecipientKind::Slack => "slack",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "email" => Some(RecipientKind::Email),
            "webhook" => Some(RecipientKind::Webhook),
            "slack" => Some(RecipientKind::Slack),
            _ => None,
        }
    }
}

/// One object a report delivers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TargetInput {
    /// `chart` or `dashboard`.
    pub kind: String,
    pub object_id: uuid::Uuid,
}

/// A delivery destination. `config` is kind-dependent — `{"addresses": [...]}`
/// for email, `{"url": "...", "secret_id": "..."}` for webhook — and is
/// validated against `kind` on the way in.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RecipientDto {
    pub id: String,
    pub kind: RecipientKind,
    pub config: serde_json::Value,
}

/// A recipient as submitted; the id is assigned by the server.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct RecipientInput {
    pub kind: RecipientKind,
    pub config: serde_json::Value,
}

/// One row of the list page.
///
/// `last_state` is the column the page exists for, so it is on the summary
/// rather than the detail — a list that needs a second request per row to say
/// whether anything is broken would not be opened.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ScheduleSummaryDto {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: ScheduleType,
    pub name: String,
    pub description: String,
    pub active: bool,
    pub crontab: String,
    pub timezone: String,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_state: Option<RunState>,
    pub last_eval_at: Option<chrono::DateTime<chrono::Utc>>,
    pub owners: Vec<OwnerDto>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Whether the caller may edit/delete schedules in this workspace.
    ///
    /// One workspace-level answer repeated on every row, not a per-object
    /// decision — `alert:manage` is workspace-scoped, so the answer is the same
    /// for the whole list. It exists because the client CANNOT derive it: a
    /// session's `permissions` carries PLATFORM grants only, so a UI that
    /// guessed would show a reader destructive controls that 403 on click.
    pub can_manage: bool,
    /// Whether the CALLER has starred this one — per-user, unlike `can_manage`.
    pub bookmarked: bool,
    /// The folder this row is grouped under, or `None` for "Uncategorized".
    pub folder_id: Option<String>,
    /// Set only in the trash view; `None` for a live schedule.
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A schedule owner. The owner is a security-relevant field, not decoration:
/// the alert's query runs with this user's permissions at run time (§5.2).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct OwnerDto {
    pub user_id: String,
    pub email: String,
}

/// The editor's view: everything on the summary plus the type-specific blocks,
/// recipients, and the advanced knobs.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ScheduleDetailDto {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: ScheduleType,
    pub name: String,
    pub description: String,
    pub active: bool,
    pub crontab: String,
    pub timezone: String,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,

    // Alert block.
    pub database_id: Option<String>,
    pub sql: Option<String>,
    pub validator_type: Option<ValidatorType>,
    pub validator_config: serde_json::Value,
    pub last_state: Option<RunState>,
    pub last_eval_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_value: Option<f64>,

    // Report block.
    pub chart_id: Option<String>,
    pub dashboard_id: Option<String>,
    pub report_format: Option<ReportFormat>,
    pub custom_width: Option<i32>,
    pub custom_height: Option<i32>,

    // Data API alert block (migration 0128). `dataapi_endpoint_id: None` is
    // workspace-scoped — it watches every endpoint, not "no endpoint".
    pub dataapi_endpoint_id: Option<String>,
    pub dataapi_metric: Option<DataapiMetric>,
    pub dataapi_error_code: Option<String>,
    pub dataapi_window_s: Option<i32>,

    // Advanced.
    pub grace_period_s: i32,
    pub working_timeout_s: i32,
    pub log_retention_days: i32,
    pub email_subject: Option<String>,
    /// A user-authored note carried into the notification body. Distinct from
    /// `description`, which is the line the LIST shows.
    pub message: Option<String>,
    /// The closing note, rendered UNDER the result — `message` opens the
    /// notification, this one closes it, and an inline image sits between them.
    /// Defaulted, so a client written before 0116 keeps working unchanged.
    #[serde(default)]
    pub message_footer: Option<String>,
    /// Metabase's `skip_if_empty`: send nothing when the run produced no rows.
    #[serde(default)]
    pub skip_if_empty: bool,
    /// Objects a REPORT delivers, in page order. Empty falls back to the
    /// single `chart_id`/`dashboard_id`, which is every pre-0115 row.
    #[serde(default)]
    pub targets: Vec<TargetInput>,

    pub owners: Vec<OwnerDto>,
    pub recipients: Vec<RecipientDto>,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Create body. `type` decides which optional block is required, checked in the
/// route before the database's CHECK constraints see it, so the client gets a
/// message naming the missing field rather than a constraint name.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateSchedule {
    #[serde(rename = "type")]
    pub ty: ScheduleType,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub active: bool,
    pub crontab: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,

    #[serde(default)]
    pub database_id: Option<Uuid>,
    #[serde(default)]
    pub sql: Option<String>,
    #[serde(default)]
    pub validator_type: Option<ValidatorType>,
    #[serde(default)]
    pub validator_config: Option<serde_json::Value>,

    #[serde(default)]
    pub chart_id: Option<Uuid>,
    #[serde(default)]
    pub dashboard_id: Option<Uuid>,
    #[serde(default)]
    pub report_format: Option<ReportFormat>,
    #[serde(default)]
    pub custom_width: Option<i32>,
    #[serde(default)]
    pub custom_height: Option<i32>,

    /// `None` is workspace-scoped: watch every endpoint rather than one.
    #[serde(default)]
    pub dataapi_endpoint_id: Option<Uuid>,
    #[serde(default)]
    pub dataapi_metric: Option<DataapiMetric>,
    /// Required, and only meaningful, when `dataapi_metric` is
    /// `error_code_count` — `schedule_dataapi_error_code_needs_metric` /
    /// `schedule_dataapi_metric_needs_error_code` (migration 0128).
    #[serde(default)]
    pub dataapi_error_code: Option<String>,
    #[serde(default)]
    pub dataapi_window_s: Option<i32>,

    #[serde(default)]
    pub grace_period_s: Option<i32>,
    #[serde(default)]
    pub working_timeout_s: Option<i32>,
    #[serde(default)]
    pub log_retention_days: Option<i32>,
    #[serde(default)]
    pub email_subject: Option<String>,
    /// A user-authored note carried into the notification body. Distinct from
    /// `description`, which is the line the LIST shows.
    pub message: Option<String>,
    /// The closing note, rendered UNDER the result — `message` opens the
    /// notification, this one closes it, and an inline image sits between them.
    /// Defaulted, so a client written before 0116 keeps working unchanged.
    #[serde(default)]
    pub message_footer: Option<String>,
    /// Metabase's `skip_if_empty`: send nothing when the run produced no rows.
    #[serde(default)]
    pub skip_if_empty: bool,
    /// Objects a REPORT delivers, in page order. Empty falls back to the
    /// single `chart_id`/`dashboard_id`, which is every pre-0115 row.
    #[serde(default)]
    pub targets: Vec<TargetInput>,

    /// Owner user ids. Empty means "the creator", resolved server-side — a
    /// schedule with no owner has no identity to run as (§5.2).
    #[serde(default)]
    pub owners: Vec<Uuid>,
    #[serde(default)]
    pub recipients: Vec<RecipientInput>,
}

/// Update body. Same shape as create minus `type`, which is immutable: an alert
/// and a report have disjoint required fields, so flipping the type of an
/// existing row can only produce a row that violates its own constraints.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateSchedule {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub active: bool,
    pub crontab: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,

    #[serde(default)]
    pub database_id: Option<Uuid>,
    #[serde(default)]
    pub sql: Option<String>,
    #[serde(default)]
    pub validator_type: Option<ValidatorType>,
    #[serde(default)]
    pub validator_config: Option<serde_json::Value>,

    #[serde(default)]
    pub chart_id: Option<Uuid>,
    #[serde(default)]
    pub dashboard_id: Option<Uuid>,
    #[serde(default)]
    pub report_format: Option<ReportFormat>,
    #[serde(default)]
    pub custom_width: Option<i32>,
    #[serde(default)]
    pub custom_height: Option<i32>,

    /// `None` is workspace-scoped: watch every endpoint rather than one.
    #[serde(default)]
    pub dataapi_endpoint_id: Option<Uuid>,
    #[serde(default)]
    pub dataapi_metric: Option<DataapiMetric>,
    /// Required, and only meaningful, when `dataapi_metric` is
    /// `error_code_count` — `schedule_dataapi_error_code_needs_metric` /
    /// `schedule_dataapi_metric_needs_error_code` (migration 0128).
    #[serde(default)]
    pub dataapi_error_code: Option<String>,
    #[serde(default)]
    pub dataapi_window_s: Option<i32>,

    #[serde(default)]
    pub grace_period_s: Option<i32>,
    #[serde(default)]
    pub working_timeout_s: Option<i32>,
    #[serde(default)]
    pub log_retention_days: Option<i32>,
    #[serde(default)]
    pub email_subject: Option<String>,
    /// A user-authored note carried into the notification body. Distinct from
    /// `description`, which is the line the LIST shows.
    pub message: Option<String>,
    /// The closing note, rendered UNDER the result — `message` opens the
    /// notification, this one closes it, and an inline image sits between them.
    /// Defaulted, so a client written before 0116 keeps working unchanged.
    #[serde(default)]
    pub message_footer: Option<String>,
    /// Metabase's `skip_if_empty`: send nothing when the run produced no rows.
    #[serde(default)]
    pub skip_if_empty: bool,
    /// Objects a REPORT delivers, in page order. Empty falls back to the
    /// single `chart_id`/`dashboard_id`, which is every pre-0115 row.
    #[serde(default)]
    pub targets: Vec<TargetInput>,

    #[serde(default)]
    pub owners: Vec<Uuid>,
    #[serde(default)]
    pub recipients: Vec<RecipientInput>,
}

/// One entry in the run-log drawer.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RunDto {
    pub id: String,
    pub state: RunState,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub value: Option<f64>,
    pub error: Option<String>,
    pub notification_sent_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_true() -> bool {
    true
}

fn default_timezone() -> String {
    "UTC".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stored spellings are a database vocabulary pinned by CHECK
    /// constraints in migration 0110. A rename here that is not also a
    /// migration produces rows the scheduler's WHERE clause silently never
    /// matches, so round-tripping every variant is worth the few lines.
    #[test]
    fn every_enum_round_trips_through_its_stored_spelling() {
        for v in [
            ScheduleType::Alert,
            ScheduleType::Report,
            ScheduleType::DataapiAlert,
        ] {
            assert_eq!(ScheduleType::parse(v.as_str()), Some(v));
        }
        for v in [ValidatorType::NotNull, ValidatorType::Operator] {
            assert_eq!(ValidatorType::parse(v.as_str()), Some(v));
        }
        for v in [
            DataapiMetric::ErrorRate,
            DataapiMetric::ErrorCount,
            DataapiMetric::P50DurationMs,
            DataapiMetric::P95DurationMs,
            DataapiMetric::CallCount,
            DataapiMetric::ErrorCodeCount,
        ] {
            assert_eq!(DataapiMetric::parse(v.as_str()), Some(v));
        }
        for v in [
            ReportFormat::Png,
            ReportFormat::Pdf,
            ReportFormat::Csv,
            ReportFormat::Text,
        ] {
            assert_eq!(ReportFormat::parse(v.as_str()), Some(v));
        }
        for v in [
            RunState::Success,
            RunState::Working,
            RunState::Error,
            RunState::Noop,
            RunState::Grace,
        ] {
            assert_eq!(RunState::parse(v.as_str()), Some(v));
        }
        for v in [
            RecipientKind::Email,
            RecipientKind::Webhook,
            RecipientKind::Slack,
        ] {
            assert_eq!(RecipientKind::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn unknown_spellings_are_rejected_rather_than_defaulted() {
        // A default here would turn a typo into a silently wrong row, which is
        // the exact failure the CHECK constraints exist to prevent.
        assert_eq!(ScheduleType::parse("digest"), None);
        assert_eq!(RunState::parse("sucess"), None);
        assert_eq!(ValidatorType::parse("notnull"), None);
        assert_eq!(ReportFormat::parse("jpeg"), None);
        // `slack` USED to be the unknown here. It became a real kind in #484,
        // and neither this line nor the round-trip list above was updated — so
        // the crate has been red on main ever since, invisible to per-crate
        // runs. The negative case now uses a spelling nothing will ever add.
        assert_eq!(RecipientKind::parse("carrier_pigeon"), None);
        assert_eq!(DataapiMetric::parse("p99_duration_ms"), None);
    }

    /// The JSON spellings must match the stored spellings, or the wire and the
    /// database disagree about what `not_null` is called.
    #[test]
    fn serde_spelling_matches_the_stored_spelling() {
        let json = serde_json::to_string(&ValidatorType::NotNull).unwrap();
        assert_eq!(json, "\"not_null\"");
        assert_eq!(serde_json::to_string(&RunState::Noop).unwrap(), "\"noop\"");
        assert_eq!(
            serde_json::to_string(&ScheduleType::Alert).unwrap(),
            "\"alert\""
        );
        assert_eq!(
            serde_json::to_string(&ReportFormat::Png).unwrap(),
            "\"png\""
        );
        // The one spelling `#[serde(rename_all = "lowercase")]` would have
        // gotten wrong (no underscore) — see the explicit `#[serde(rename)]`
        // on the variant.
        assert_eq!(
            serde_json::to_string(&ScheduleType::DataapiAlert).unwrap(),
            "\"dataapi_alert\""
        );
        // The digit makes this one worth pinning too: serde's automatic case
        // conversion is not used for `DataapiMetric` at all (every variant
        // carries an explicit `#[serde(rename)]`) precisely because
        // "P50DurationMs" is not obviously "p50_duration_ms" under any
        // mechanical rule.
        assert_eq!(
            serde_json::to_string(&DataapiMetric::P50DurationMs).unwrap(),
            "\"p50_duration_ms\""
        );
    }
}
