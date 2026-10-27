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
    // F11 (engine-scoped grants, `perm.engine_grants`).
    pub const ENGINE_GRANT_CREATE: &str = "engine_grant:create";
    pub const ENGINE_GRANT_UPDATE: &str = "engine_grant:update";
    pub const ENGINE_GRANT_DELETE: &str = "engine_grant:delete";
    // Username takeover (spec 2026-10-23 §4.4): who was deleted, with the
    // username retired for good.
    pub const USER_DELETE: &str = "user:delete";
    // Classifications follow a rename (spec 2026-10-26 §5.7): a table or
    // column's rows moved to its new name, or a dropped namespace's removed.
    pub const CATALOG_TAG_MOVE: &str = "tag:move";
    pub const CATALOG_TAG_PURGE: &str = "tag:purge";
    // Grants follow a rename (spec 2026-10-27 §5.7, D9): an allow re-pointed
    // to the renamed table, a restriction copied to it (the original stays).
    pub const GRANT_MOVE: &str = "perm_grant:move";
    pub const GRANT_COPY: &str = "perm_grant:copy";
    pub const RULE_COPY: &str = "perm_rule:copy";
}

/// The `resource_type` each governance action is recorded under.
pub mod resource {
    pub const PERM_GRANT: &str = "perm_grant";
    pub const PERM_RULE: &str = "perm_rule";
    pub const TAG: &str = "tag";
    pub const RANGER_POLICY: &str = "ranger_policy";
    pub const POLICY_PROFILE: &str = "policy_profile";
    pub const ENGINE_GRANT: &str = "engine_grant";
    pub const USER: &str = "user";
    /// A catalog table or column (`<warehouse_id>:<schema>.<table>`), as the
    /// legacy `tag:assign` sink events record it.
    pub const CATALOG: &str = "catalog";
}

/// Which permission reads an event in the feed: `Policy` needs `policy:read`,
/// `Tag` needs `tag:read`, `User` needs `user:read`. `User` is its own family
/// because its events carry a person's email (username-takeover spec, D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Policy,
    Tag,
    User,
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
    (
        action::ENGINE_GRANT_CREATE,
        resource::ENGINE_GRANT,
        Family::Policy,
    ),
    (
        action::ENGINE_GRANT_UPDATE,
        resource::ENGINE_GRANT,
        Family::Policy,
    ),
    (
        action::ENGINE_GRANT_DELETE,
        resource::ENGINE_GRANT,
        Family::Policy,
    ),
    (action::USER_DELETE, resource::USER, Family::User),
    (action::CATALOG_TAG_MOVE, resource::CATALOG, Family::Tag),
    (action::CATALOG_TAG_PURGE, resource::CATALOG, Family::Tag),
    (action::GRANT_MOVE, resource::PERM_GRANT, Family::Policy),
    (action::GRANT_COPY, resource::PERM_GRANT, Family::Policy),
    (action::RULE_COPY, resource::PERM_RULE, Family::Policy),
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
    action::ENGINE_GRANT_CREATE,
    action::ENGINE_GRANT_UPDATE,
    action::ENGINE_GRANT_DELETE,
    action::USER_DELETE,
    action::CATALOG_TAG_MOVE,
    action::CATALOG_TAG_PURGE,
    action::GRANT_MOVE,
    action::GRANT_COPY,
    action::RULE_COPY,
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

/// Who made a governance change.
pub enum Actor<'a> {
    /// A signed-in user (or an administrator acting as one).
    User(&'a egret_core::Principal),
    /// A catalog client (`catalog.catalog_clients`) whose token reached the
    /// Iceberg catalog: it is no user, so it is recorded in `detail.actor`
    /// with `actor_user_id` NULL (rename spec C6a). A client deleted since
    /// keeps its `client_id`, with `id`, `name` and `consumer_type` unknown.
    CatalogClient {
        id: Option<Uuid>,
        client_id: String,
        name: Option<String>,
        consumer_type: Option<String>,
    },
    /// egret itself (no caller is known: a request with no catalog guard).
    Egret,
}

/// INSERT one `kind = 'mutation'` row on the caller's connection (its
/// transaction). Stamps `actor_user_id` and `impersonator` exactly as
/// [`crate::mutation_for`] does. A summary over [`MAX_SUMMARY_BYTES`] is
/// stored as `{"truncated": true, "bytes": n}`.
pub async fn record_change(conn: &mut PgConnection, c: Change<'_>) -> Result<(), sqlx::Error> {
    record_change_by(
        conn,
        Actor::User(c.principal),
        c.action,
        c.resource_type,
        c.resource_id,
        c.summary,
    )
    .await
}

/// [`record_change`] by any [`Actor`]: a user is stamped as `record_change`
/// does; a catalog client leaves `actor_user_id` NULL and is added to the
/// stored detail as `actor: {kind: "catalog_client", id, client_id, name,
/// consumer_type}`; egret sets neither.
pub async fn record_change_by(
    conn: &mut PgConnection,
    actor: Actor<'_>,
    action: &'static str,
    resource_type: &'static str,
    resource_id: String,
    summary: serde_json::Value,
) -> Result<(), sqlx::Error> {
    debug_assert!(
        family_of(action, resource_type).is_some(),
        "{action} on {resource_type} is not a governance pair; add it to FAMILIES"
    );
    if !RECORDING.load(Ordering::Relaxed) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&summary)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    let mut detail = if bytes > MAX_SUMMARY_BYTES {
        serde_json::json!({ "truncated": true, "bytes": bytes })
    } else {
        summary
    };
    let (user, impersonator) = match &actor {
        Actor::User(p) => (
            Some(p.user_id.0),
            p.impersonator.as_ref().map(|i| i.user_id.0),
        ),
        Actor::CatalogClient {
            id,
            client_id,
            name,
            consumer_type,
        } => {
            if let Some(obj) = detail.as_object_mut() {
                obj.insert(
                    "actor".to_string(),
                    serde_json::json!({
                        "kind": "catalog_client", "id": id, "client_id": client_id,
                        "name": name, "consumer_type": consumer_type,
                    }),
                );
            }
            (None, None)
        }
        Actor::Egret => (None, None),
    };
    sqlx::query(
        "INSERT INTO audit.audit_log \
             (kind, actor_user_id, impersonator, workspace_id, action, resource_type, \
              resource_id, detail) \
         VALUES ('mutation', $1, $2, NULL, $3, $4, $5, $6)",
    )
    .bind(user)
    .bind(impersonator)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
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
/// by `audit_log_mutation_ts_idx` (migration 0199).
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
