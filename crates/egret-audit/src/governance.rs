//! Governance events, recorded **in the write's own transaction** (DG #3,
//! spec `docs/en/specs/2026-10-18-dg-overview-design.md` §3.1, decision O3).
//!
//! The best-effort [`crate::AuditSink`] runs after commit, drops on overflow
//! and is not drained at all when `audit.enabled = false`. The governance feed
//! is the evidence of who changed governance, so these events are one INSERT
//! on the caller's connection instead: an event exists exactly when its write
//! committed, and a refused, rolled-back, previewed or dry-run write leaves
//! none.
//!
//! The rows share `audit.audit_log` with every other event, as
//! `kind = 'mutation'`. Which `(action, resource_type)` pairs are governance,
//! and which permission reads each, is [`FAMILIES`]: the one table both
//! [`family_of`] and the feed query ([`governance_changes_via`]) read. A
//! package adding governance actions (F11's `engine_grant:*`) adds them there.
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

/// The action strings. The feed returns exactly these, plus the two legacy
/// sink families in [`FAMILIES`].
pub mod action {
    pub const GRANT_CREATE: &str = "perm_grant:create";
    pub const GRANT_UPDATE: &str = "perm_grant:update";
    pub const GRANT_DELETE: &str = "perm_grant:delete";
    pub const RULE_CREATE: &str = "perm_rule:create";
    pub const RULE_UPDATE: &str = "perm_rule:update";
    pub const RULE_DELETE: &str = "perm_rule:delete";
    pub const CLEARANCE_SET: &str = "tag_clearance:set";
    pub const CLEARANCE_CLEAR: &str = "tag_clearance:clear";
    pub const RANGER_POLICY_CREATE: &str = "ranger_policy:create";
    pub const RANGER_POLICY_UPDATE: &str = "ranger_policy:update";
    pub const RANGER_POLICY_DELETE: &str = "ranger_policy:delete";
    pub const RANGER_POLICY_ENABLE: &str = "ranger_policy:enable";
    pub const RANGER_POLICY_DISABLE: &str = "ranger_policy:disable";
    pub const MIGRATION_APPLY: &str = "policy_migration:apply";
    pub const MIGRATION_UNDO: &str = "policy_migration:undo";
    pub const PROFILE_COMPILE: &str = "profile:compile";
    // F11 (engine-scoped grants) adds, in its own package:
    // ENGINE_GRANT_CREATE/UPDATE/DELETE = "engine_grant:create|update|delete", family Policy.
}

/// The `resource_type` each governance action is recorded under.
pub mod resource {
    pub const PERM_GRANT: &str = "perm_grant";
    pub const PERM_RULE: &str = "perm_rule";
    pub const TAG: &str = "tag";
    pub const RANGER_POLICY: &str = "ranger_policy";
    pub const POLICY_PROFILE: &str = "policy_profile";
}

/// Which permission reads an event in the feed: `Policy` needs `policy:read`,
/// `Tag` needs `tag:read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Policy,
    Tag,
}

/// Every governance `(action, resource_type)` pair and its family. The last
/// four are the legacy sink events (spec O4): role holders on `grant`, and
/// catalog-column tags on `catalog` only; `tag:assign` on `object` is a
/// workspace BI tag and is not governance.
pub const FAMILIES: &[(&str, &str, Family)] = &[
    (action::GRANT_CREATE, resource::PERM_GRANT, Family::Policy),
    (action::GRANT_UPDATE, resource::PERM_GRANT, Family::Policy),
    (action::GRANT_DELETE, resource::PERM_GRANT, Family::Policy),
    (action::RULE_CREATE, resource::PERM_RULE, Family::Policy),
    (action::RULE_UPDATE, resource::PERM_RULE, Family::Policy),
    (action::RULE_DELETE, resource::PERM_RULE, Family::Policy),
    (action::CLEARANCE_SET, resource::TAG, Family::Tag),
    (action::CLEARANCE_CLEAR, resource::TAG, Family::Tag),
    (
        action::RANGER_POLICY_CREATE,
        resource::RANGER_POLICY,
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_UPDATE,
        resource::RANGER_POLICY,
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_DELETE,
        resource::RANGER_POLICY,
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_ENABLE,
        resource::RANGER_POLICY,
        Family::Policy,
    ),
    (
        action::RANGER_POLICY_DISABLE,
        resource::RANGER_POLICY,
        Family::Policy,
    ),
    (
        action::MIGRATION_APPLY,
        resource::POLICY_PROFILE,
        Family::Policy,
    ),
    (
        action::MIGRATION_UNDO,
        resource::POLICY_PROFILE,
        Family::Policy,
    ),
    (
        action::PROFILE_COMPILE,
        resource::POLICY_PROFILE,
        Family::Policy,
    ),
    ("role:assign", "grant", Family::Policy),
    ("role:unassign", "grant", Family::Policy),
    ("tag:assign", "catalog", Family::Tag),
    ("tag:unassign", "catalog", Family::Tag),
];

/// Every constant of [`action`], once.
pub const GOVERNANCE_ACTIONS: &[&str] = &[
    action::GRANT_CREATE,
    action::GRANT_UPDATE,
    action::GRANT_DELETE,
    action::RULE_CREATE,
    action::RULE_UPDATE,
    action::RULE_DELETE,
    action::CLEARANCE_SET,
    action::CLEARANCE_CLEAR,
    action::RANGER_POLICY_CREATE,
    action::RANGER_POLICY_UPDATE,
    action::RANGER_POLICY_DELETE,
    action::RANGER_POLICY_ENABLE,
    action::RANGER_POLICY_DISABLE,
    action::MIGRATION_APPLY,
    action::MIGRATION_UNDO,
    action::PROFILE_COMPILE,
];

/// The family of a stored event, or `None` when it is not governance.
pub fn family_of(action: &str, resource_type: &str) -> Option<Family> {
    FAMILIES
        .iter()
        .find(|(a, r, _)| *a == action && *r == resource_type)
        .map(|(_, _, f)| *f)
}

/// One governance change. `summary` is `{ "before": …|null, "after": …|null,
/// …extra }`, small and stable (spec §3.1), never a whole request body.
pub struct Change<'a> {
    /// One of [`action`].
    pub action: &'static str,
    /// One of [`resource`].
    pub resource_type: &'static str,
    /// Grant/rule uuid, tag uuid, policy id or profile id.
    pub resource_id: String,
    pub summary: serde_json::Value,
    pub principal: &'a egret_core::Principal,
}

/// Summaries larger than this, serialised, are replaced by their size.
pub const MAX_SUMMARY_BYTES: usize = 16 * 1024;

/// Whether governance events are recorded at all — set once at startup from
/// `audit.enabled` by [`configure`]. Defaults to recording.
static RECORDING: AtomicBool = AtomicBool::new(true);

/// **Decision O3 (user, 2026-10-18), and the one place that decides it:**
/// governance events are recorded whether or not `audit.enabled` is set. The
/// flag keeps governing the PDP decision rows and the best-effort sink. To
/// make governance recording follow the flag instead, change this function
/// and nothing else.
pub fn records_governance_changes(audit_enabled: bool) -> bool {
    let _ = audit_enabled;
    true
}

/// Apply [`records_governance_changes`] to the process's `audit.enabled`.
/// Called once by `egret-bin` at startup.
pub fn configure(audit_enabled: bool) {
    RECORDING.store(records_governance_changes(audit_enabled), Ordering::Relaxed);
}

/// INSERT one `kind = 'mutation'` row on the caller's connection (its
/// transaction). Stamps `actor_user_id` and `impersonator` exactly as
/// [`crate::mutation_for`] does. A summary over [`MAX_SUMMARY_BYTES`] is
/// stored as `{"truncated": true, "bytes": n}`.
pub async fn record_change(conn: &mut PgConnection, c: Change<'_>) -> Result<(), sqlx::Error> {
    debug_assert!(
        family_of(c.action, c.resource_type).is_some(),
        "{} on {} is not a governance pair; add it to FAMILIES",
        c.action,
        c.resource_type
    );
    if !RECORDING.load(Ordering::Relaxed) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&c.summary)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    let detail = if bytes > MAX_SUMMARY_BYTES {
        serde_json::json!({ "truncated": true, "bytes": bytes })
    } else {
        c.summary
    };
    sqlx::query(
        "INSERT INTO audit.audit_log \
             (kind, actor_user_id, impersonator, workspace_id, action, resource_type, \
              resource_id, detail) \
         VALUES ('mutation', $1, $2, NULL, $3, $4, $5, $6)",
    )
    .bind(c.principal.user_id.0)
    .bind(c.principal.impersonator.as_ref().map(|i| i.user_id.0))
    .bind(c.action)
    .bind(c.resource_type)
    .bind(c.resource_id)
    .bind(detail)
    .execute(conn)
    .await?;
    Ok(())
}

/// A governance event as the feed reads it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChangeRow {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub actor_user_id: Option<Uuid>,
    pub impersonator: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub detail: serde_json::Value,
}

/// Newest first, `kind = 'mutation'`, `(action, resource_type)` in the pairs
/// `families` allow, strictly before `before = (ts, id)` when given. Served
/// by `audit_log_mutation_ts_idx` (migration 0197).
pub async fn governance_changes_via(
    conn: &mut PgConnection,
    families: &[Family],
    before: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> Result<Vec<ChangeRow>, sqlx::Error> {
    let (actions, resource_types): (Vec<&str>, Vec<&str>) = FAMILIES
        .iter()
        .filter(|(_, _, f)| families.contains(f))
        .map(|(a, r, _)| (*a, *r))
        .unzip();
    if actions.is_empty() {
        return Ok(Vec::new());
    }
    let (before_ts, before_id) = match before {
        Some((ts, id)) => (Some(ts), Some(id)),
        None => (None, None),
    };
    sqlx::query_as::<_, ChangeRow>(
        "SELECT id, ts, actor_user_id, impersonator, action, resource_type, resource_id, detail \
           FROM audit.audit_log \
          WHERE kind = 'mutation' \
            AND (action, resource_type) IN \
                (SELECT * FROM unnest($1::text[], $2::text[])) \
            AND ($3::timestamptz IS NULL OR (ts, id) < ($3, $4)) \
          ORDER BY ts DESC, id DESC \
          LIMIT $5",
    )
    .bind(actions)
    .bind(resource_types)
    .bind(before_ts)
    .bind(before_id)
    .bind(limit.max(1))
    .fetch_all(conn)
    .await
}
