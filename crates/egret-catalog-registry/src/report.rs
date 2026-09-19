//! `registry.migration_report` rows: what the backfill did, kept until an admin
//! acknowledges them on the Catalogs page.

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReportEntry {
    pub run_id: Uuid,
    pub severity: Severity,
    pub code: &'static str,
    pub catalog_id: Option<Uuid>,
    pub engine_id: Option<Uuid>,
    pub warehouse_id: Option<Uuid>,
    pub subject: String,
    pub message: String,
    pub detail: Value,
}

impl ReportEntry {
    fn new(
        run_id: Uuid,
        severity: Severity,
        code: &'static str,
        subject: String,
        message: String,
    ) -> Self {
        ReportEntry {
            run_id,
            severity,
            code,
            catalog_id: None,
            engine_id: None,
            warehouse_id: None,
            subject,
            message,
            detail: Value::Object(Default::default()),
        }
    }
    pub fn info(
        run_id: Uuid,
        code: &'static str,
        subject: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(run_id, Severity::Info, code, subject.into(), message.into())
    }
    pub fn warning(
        run_id: Uuid,
        code: &'static str,
        subject: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            run_id,
            Severity::Warning,
            code,
            subject.into(),
            message.into(),
        )
    }
    pub fn catalog(mut self, id: Uuid) -> Self {
        self.catalog_id = Some(id);
        self
    }
    pub fn engine(mut self, id: Uuid) -> Self {
        self.engine_id = Some(id);
        self
    }
    pub fn warehouse(mut self, id: Uuid) -> Self {
        self.warehouse_id = Some(id);
        self
    }
    pub fn detail(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ReportRow {
    pub id: i64,
    pub run_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub severity: String,
    pub code: String,
    pub catalog_id: Option<Uuid>,
    pub engine_id: Option<Uuid>,
    pub warehouse_id: Option<Uuid>,
    pub subject: String,
    pub message: String,
    pub detail: Value,
    pub acknowledged_at: Option<DateTime<Utc>>,
}
