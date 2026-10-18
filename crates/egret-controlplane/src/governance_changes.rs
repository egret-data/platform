//! `GET /governance/changes` (DG #3, spec §3.4): recent governance changes,
//! each family only to a reader holding its permission (`Policy` →
//! `policy:read`, `Tag` → `tag:read`). The events are recorded by
//! `egret_audit::governance::record_change` in their write's transaction,
//! plus the legacy role and catalog-tag events the best-effort sink writes
//! after commit (spec O4).
//!
//! Labels are resolved at read time, so a renamed object shows its current
//! name and a deleted one falls back to what the event recorded.
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_audit::governance::{action, governance_changes_via, ChangeRow, Family};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::governance_overview::Ref;
use crate::{CpError, CpState};

const DEFAULT_LIMIT: i64 = 25;
const MAX_LIMIT: i64 = 100;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChangesQuery {
    /// Page size, default 25, at most 100.
    pub limit: Option<i64>,
    /// The previous page's `nextCursor` (`<ts_rfc3339>,<id>`).
    pub cursor: Option<String>,
}

/// Who acted. `label` is the user's name or email, `null` when the user no
/// longer exists (the id is kept).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Party {
    pub user_id: Uuid,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor: Option<Party>,
    /// Set when an administrator acted as `actor`.
    pub impersonator: Option<Party>,
    pub action: String,
    /// One sentence, built here per action.
    pub sentence: String,
    /// What the change was made to, when there is a page for it.
    pub target: Option<Ref>,
    /// Whether that object still exists (a deleted grant's event names it and
    /// links nowhere).
    pub target_exists: bool,
    /// The recorded summary (`before`/`after`, …) as stored.
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangesPage {
    pub entries: Vec<ChangeEntry>,
    pub next_cursor: Option<String>,
    /// Days after which events are pruned; `null` when they are never pruned.
    pub retention_days: Option<i64>,
}

pub(crate) fn router() -> Router<CpState> {
    Router::new().route("/governance/changes", get(changes))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(changes), components(schemas(ChangesPage, ChangeEntry, Party)))]
pub(crate) struct ControlPlaneGovernanceChangesApi;

fn parse_cursor(s: &str) -> Option<(DateTime<Utc>, i64)> {
    let (ts, id) = s.split_once(',')?;
    Some((ts.parse().ok()?, id.parse().ok()?))
}

fn db(what: &'static str) -> impl FnOnce(sqlx::Error) -> CpError {
    move |e| {
        tracing::error!(error = %e, "governance changes: {what}");
        CpError::Internal
    }
}

/// Recent governance changes, newest first.
#[utoipa::path(
    get,
    path = "/api/control-plane/governance/changes",
    tag = "control-plane-governance",
    params(ChangesQuery),
    description = "Governance events, newest first: grant, rule, hand-written policy, \
                   migration and compile changes and role assignments for a `policy:read` \
                   reader; clearance changes and catalog-column tags for a `tag:read` reader. \
                   Recorded from the version that introduced this feed on; role assignments and \
                   catalog tags are best-effort (recorded after commit).",
    responses(
        (status = 200, description = "One page of governance changes", body = ChangesPage),
        (status = 400, description = "A malformed cursor", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Holds neither policy:read nor tag:read", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn changes(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<ChangesQuery>,
) -> Result<Json<ChangesPage>, CpError> {
    crate::governance_browse::require_governance_read(&st, &p).await?;
    let mut families = Vec::new();
    for (perm, family) in [("policy:read", Family::Policy), ("tag:read", Family::Tag)] {
        match egret_core::authz::require(&*st.pdp, &p, perm).await {
            Ok(()) => families.push(family),
            Err(egret_core::Error::Forbidden) => {}
            Err(e) => return Err(e.into()),
        }
    }
    let before = match q.cursor.as_deref() {
        None => None,
        Some(c) => Some(
            parse_cursor(c)
                .ok_or_else(|| CpError::BadRequest(format!("malformed cursor: {c:?}")))?,
        ),
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut conn = st.db.pool.acquire().await.map_err(db("acquire"))?;
    let mut rows = governance_changes_via(&mut conn, &families, before, limit + 1)
        .await
        .map_err(db("read the feed"))?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = more
        .then(|| {
            rows.last()
                .map(|r| format!("{},{}", r.ts.to_rfc3339(), r.id))
        })
        .flatten();
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(entry(&mut conn, row).await.map_err(db("label an event"))?);
    }
    Ok(Json(ChangesPage {
        entries,
        next_cursor,
        retention_days: st.audit_retention_days,
    }))
}

// ── labels ───────────────────────────────────────────────────────────────

async fn user_label(c: &mut PgConnection, id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(NULLIF(btrim(name), ''), email) FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *c)
        .await
}

async fn party(c: &mut PgConnection, id: Option<Uuid>) -> Result<Option<Party>, sqlx::Error> {
    Ok(match id {
        None => None,
        Some(user_id) => Some(Party {
            user_id,
            label: user_label(c, user_id).await?,
        }),
    })
}

async fn exists(c: &mut PgConnection, sql: &'static str, id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(sql).bind(id).fetch_one(&mut *c).await
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The side of a `{before, after}` summary that describes the object: `after`,
/// or `before` for a delete.
fn side(detail: &Value) -> &Value {
    match detail.get("after") {
        Some(a) if !a.is_null() => a,
        _ => detail.get("before").unwrap_or(&Value::Null),
    }
}

/// `allow read on ice.s.t to group g1`.
fn grant_phrase(g: &Value) -> String {
    let path: Vec<&str> = ["catalog", "schema", "table", "column"]
        .iter()
        .map_while(|k| g.get(*k).and_then(Value::as_str))
        .collect();
    let who: Vec<String> = strings(g, "include")
        .iter()
        .map(|r| r.replacen(':', " ", 1))
        .collect();
    let mut out = format!(
        "{} {} on {} to {}",
        s(g, "effect"),
        strings(g, "actions").join(", "),
        path.join("."),
        who.join(", ")
    );
    let except: Vec<String> = strings(g, "exclude")
        .iter()
        .map(|r| r.replacen(':', " ", 1))
        .collect();
    if !except.is_empty() {
        out.push_str(&format!(" except {}", except.join(", ")));
    }
    out
}

/// `rule pii-hash: mask MASK_HASH on ice.s.t.email`.
fn rule_phrase(r: &Value) -> String {
    let what = match s(r, "kind") {
        "row_filter" => format!("row filter {}", s(r, "function")),
        _ => format!("mask {}", s(r, "function")),
    };
    format!("rule {}: {what} on {}", s(r, "name"), s(r, "selector"))
}

fn names(list: &[String]) -> String {
    if list.is_empty() {
        "none".to_string()
    } else {
        list.join(", ")
    }
}

async fn entry(c: &mut PgConnection, row: ChangeRow) -> Result<ChangeEntry, sqlx::Error> {
    let actor = party(c, row.actor_user_id).await?;
    let impersonator = party(c, row.impersonator).await?;
    let who = |p: &Option<Party>| match p {
        Some(Party { label: Some(l), .. }) => l.clone(),
        Some(_) => "A deleted user".to_string(),
        None => "egret".to_string(),
    };
    let a = match &impersonator {
        Some(_) => format!("{} as {}", who(&impersonator), who(&actor)),
        None => who(&actor),
    };
    let d = &row.detail;
    let id = row.resource_id.clone().unwrap_or_default();
    let uuid = id.parse::<Uuid>().ok();
    let (sentence, target, target_exists) = match row.action.as_str() {
        action::GRANT_CREATE | action::GRANT_UPDATE | action::GRANT_DELETE => {
            let verb = match row.action.as_str() {
                action::GRANT_CREATE => "created",
                action::GRANT_UPDATE => "changed",
                _ => "deleted",
            };
            let live = exists(
                c,
                "SELECT EXISTS (SELECT 1 FROM perm.grants WHERE id::text = $1)",
                &id,
            )
            .await?;
            (
                format!("{a} {verb} a grant: {}", grant_phrase(side(d))),
                uuid.map(|id| Ref::Grant { id }),
                live,
            )
        }
        action::RULE_CREATE | action::RULE_UPDATE | action::RULE_DELETE => {
            let verb = match row.action.as_str() {
                action::RULE_CREATE => "created",
                action::RULE_UPDATE => "changed",
                _ => "deleted",
            };
            let live = exists(
                c,
                "SELECT EXISTS (SELECT 1 FROM perm.rules WHERE id::text = $1)",
                &id,
            )
            .await?;
            (
                format!("{a} {verb} {}", rule_phrase(side(d))),
                uuid.map(|id| Ref::Rule { id }),
                live,
            )
        }
        action::CLEARANCE_SET | action::CLEARANCE_CLEAR => {
            let current: Option<String> =
                sqlx::query_scalar("SELECT name FROM tags WHERE id::text = $1")
                    .bind(&id)
                    .fetch_optional(&mut *c)
                    .await?;
            let tag = current
                .clone()
                .unwrap_or_else(|| s(side(d), "tag_key").to_string());
            let sentence = if row.action == action::CLEARANCE_SET {
                let after = side(d);
                format!(
                    "{a} set the clearance of classification {tag}: roles {}; groups {}",
                    names(&strings(after, "required_roles")),
                    names(&strings(after, "required_groups"))
                )
            } else {
                format!("{a} removed the clearance of classification {tag}")
            };
            let target = match (uuid, current) {
                (Some(id), Some(name)) => Some(Ref::Tag { id, name }),
                _ => None,
            };
            let live = target.is_some();
            (sentence, target, live)
        }
        action::RANGER_POLICY_CREATE
        | action::RANGER_POLICY_UPDATE
        | action::RANGER_POLICY_DELETE
        | action::RANGER_POLICY_ENABLE
        | action::RANGER_POLICY_DISABLE => {
            let verb = match row.action.as_str() {
                action::RANGER_POLICY_CREATE => "created",
                action::RANGER_POLICY_UPDATE => "changed",
                action::RANGER_POLICY_DELETE => "deleted",
                action::RANGER_POLICY_ENABLE => "enabled",
                _ => "disabled",
            };
            let p = side(d);
            let live = exists(
                c,
                "SELECT EXISTS (SELECT 1 FROM policy.authz_policies WHERE id::text = $1)",
                &id,
            )
            .await?;
            (
                format!(
                    "{a} {verb} hand-written policy {} on profile {}",
                    s(p, "name"),
                    s(p, "service")
                ),
                Some(Ref::Profile {
                    name: s(p, "service").to_string(),
                }),
                live,
            )
        }
        action::MIGRATION_APPLY | action::MIGRATION_UNDO | action::PROFILE_COMPILE => {
            let current: Option<String> =
                sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id::text = $1")
                    .bind(&id)
                    .fetch_optional(&mut *c)
                    .await?;
            let profile = current
                .clone()
                .unwrap_or_else(|| s(d, "service").to_string());
            let policies = strings(d, "policies").join(", ");
            let sentence = match row.action.as_str() {
                action::MIGRATION_APPLY => {
                    format!(
                        "{a} migrated {policies} of profile {profile} into egret grants and rules"
                    )
                }
                action::MIGRATION_UNDO => {
                    format!("{a} undid the migration of {policies} of profile {profile}")
                }
                _ if s(d, "outcome") == "error" => format!(
                    "{a} compiled profile {profile}; the compile failed: {}",
                    s(d, "error").lines().next().unwrap_or("")
                ),
                _ => format!(
                    "{a} compiled profile {profile}: {} created, {} updated, {} deleted",
                    d.get("created").and_then(Value::as_i64).unwrap_or(0),
                    d.get("updated").and_then(Value::as_i64).unwrap_or(0),
                    d.get("deleted").and_then(Value::as_i64).unwrap_or(0)
                ),
            };
            let live = current.is_some();
            (sentence, current.map(|name| Ref::Profile { name }), live)
        }
        "role:assign" | "role:unassign" => {
            let role: Option<String> =
                sqlx::query_scalar("SELECT name FROM roles WHERE id::text = $1")
                    .bind(&id)
                    .fetch_optional(&mut *c)
                    .await?;
            let role_label = role.clone().unwrap_or_else(|| format!("{id} (deleted)"));
            let principal_id = s(d, "principal_id").parse::<Uuid>().ok();
            let holder = match (s(d, "principal_type"), principal_id) {
                ("group", Some(g)) => {
                    let name: Option<String> =
                        sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
                            .bind(g)
                            .fetch_optional(&mut *c)
                            .await?;
                    format!("group {}", name.unwrap_or_else(|| "a deleted group".into()))
                }
                (_, Some(u)) => user_label(c, u)
                    .await?
                    .unwrap_or_else(|| "a deleted user".to_string()),
                _ => "someone".to_string(),
            };
            let sentence = if row.action == "role:assign" {
                format!("{a} gave role {role_label} to {holder}")
            } else {
                format!("{a} removed role {role_label} from {holder}")
            };
            let live = role.is_some();
            (sentence, role.map(|name| Ref::Role { name }), live)
        }
        "tag:assign" | "tag:unassign" => {
            // `<warehouse_id>:<schema>.<table>.<column>`, the catalog named by
            // the registry catalog over that warehouse.
            let (warehouse, path) = id.split_once(':').unwrap_or(("", id.as_str()));
            let catalog: Option<String> = sqlx::query_scalar(
                "SELECT name FROM registry.catalogs WHERE warehouse_id::text = $1",
            )
            .bind(warehouse)
            .fetch_optional(&mut *c)
            .await?;
            let tag_id = s(d, "tag_id").to_string();
            let tag: Option<String> =
                sqlx::query_scalar("SELECT name FROM tags WHERE id::text = $1")
                    .bind(&tag_id)
                    .fetch_optional(&mut *c)
                    .await?;
            let tag = tag.unwrap_or_else(|| "a deleted classification".to_string());
            let at = match &catalog {
                Some(cat) => format!("{cat}.{path}"),
                None => path.to_string(),
            };
            let sentence = if row.action == "tag:assign" {
                format!("{a} tagged {at} with {tag}")
            } else {
                format!("{a} removed {tag} from {at}")
            };
            let mut parts = path.splitn(3, '.');
            let target = match (&catalog, parts.next(), parts.next()) {
                (Some(cat), Some(schema), Some(table)) => Some(Ref::Object {
                    catalog: cat.clone(),
                    schema: schema.to_string(),
                    object: table.to_string(),
                }),
                _ => None,
            };
            let live = target.is_some();
            (sentence, target, live)
        }
        other => (format!("{a} made a change: {other}"), None, false),
    };
    Ok(ChangeEntry {
        id: row.id,
        at: row.ts,
        actor,
        impersonator,
        action: row.action,
        sentence,
        target,
        target_exists,
        detail: row.detail,
    })
}
