//! Username takeover, spec `2026-10-23-username-takeover-design.md` §4.5
//! (decision D3): the evidence of PAST takeovers, reported and never acted on.
//!
//! Before migration 0208 a deleted user's username could be minted for the
//! next person whose email slugged to it, who then inherited every ref and
//! policy still naming it. The ledger stops that from 0208 on; this reads what
//! is left of the time before:
//!
//! A holder whose name was minted after 0208 (`minted_at` set) is never a
//! finding: 0208 seeded every name a ref, body or event knew, so a minted name
//! is one nobody held before and nothing was written for someone else.
//!
//! - `ref_predates_holder`: a `user` ref older than the person who now holds
//!   the name. `definite` when a governance event named the ref before the
//!   holder existed, or (an engine grant) the row was last changed before;
//!   otherwise `possible`, because grants and rules have no `updated_at` and a
//!   later PATCH may have added the ref for the holder;
//! - `policy_predates_holder`: a policy body naming a held username, last
//!   written before its holder existed, live or in the 0208 baseline.
//!   `definite` for `compiler`/`infra` bodies, `possible` for a hand-written
//!   one (it may be a deliberate pre-provisioning);
//! - `admin_policy_named_non_admin`: an `egret-admin` body naming someone who
//!   is not an active platform admin, in the 0208 baseline or live. `definite`
//!   when the body was last written before they existed; `stale` (live only)
//!   when they hold a `platform_admin` grant (a disabled admin) or were minted
//!   after 0208: no reuse, the sync has not caught up; otherwise `possible` (a
//!   demoted admin, or someone given a deleted admin's name after their
//!   `created_at`). A baseline row for a disabled admin, or naming nobody, is
//!   no finding;
//! - `holderless_ref`: a `user` ref, or a name in a hand-written body, that
//!   nobody holds. `dead`: it is retired and matches nobody, forever; the row
//!   says whether its position fails open.
//!
//! What it cannot see is listed in the spec (§4.5, "What the report cannot
//! see"): events past retention, refs already removed, mint times before
//! 0208, free text, engine-side ownership and actual use.
//!
//! The caller runs [`findings_via`] in one `REPEATABLE READ, READ ONLY`
//! transaction, so every rule reads one state.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use egret_policy::infra_names::is_admin_all_access_policy_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReuseKind {
    RefPredatesHolder,
    PolicyPredatesHolder,
    AdminPolicyNamedNonAdmin,
    HolderlessRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The evidence proves the name was written for someone else.
    Definite,
    /// It may have been; the timestamps that remain cannot tell.
    Possible,
    /// Nobody holds the name and nobody ever can again.
    Dead,
    /// No reuse: a live `egret-admin` body still names someone who is no
    /// longer an active platform admin, because the sync has not run since.
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaceType {
    Grant,
    Rule,
    EngineGrant,
    Policy,
}

/// Where the name is written. A grant, rule or engine grant carries `id`
/// (and, for a grant or rule, `position`; for a grant, `effect`; for an engine
/// grant, `engineId`); a policy carries `policyId`, `name`, `service` and
/// `managedBy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    #[serde(rename = "type")]
    pub place_type: PlaceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    /// The engine an engine grant belongs to: its page is where it is fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<Uuid>,
    /// `include` | `exclude`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// `allow` | `deny`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The profile (Ranger service) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
}

/// Who holds the name now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub id: Uuid,
    pub email: String,
    pub created_at: DateTime<Utc>,
    /// When the name was minted for them; `null` when they held it before
    /// migration 0208 (then `createdAt` is a lower bound).
    pub minted_at: Option<DateTime<Utc>>,
}

/// The ledger's record of a name nobody holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Retired {
    pub retired_at: Option<DateTime<Utc>>,
    /// The user it was minted for, when the ledger knows (no FK: they may be
    /// deleted).
    pub minted_for: Option<Uuid>,
    /// Whether the position fails OPEN now that the name matches nobody: a
    /// deny include, an allow exclude, a rule exemption's exclude, or a
    /// hand-written deny item or allow exception.
    pub fails_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReuseFinding {
    pub kind: ReuseKind,
    pub verdict: Verdict,
    pub username: String,
    pub place: Place,
    pub holder: Option<Holder>,
    /// Set for `holderless_ref`.
    pub retired: Option<Retired>,
    /// The time the evidence is from.
    pub evidence_at: Option<DateTime<Utc>>,
    /// The evidence, in words.
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsernameReuseReport {
    pub generated_at: DateTime<Utc>,
    pub findings: Vec<ReuseFinding>,
    /// When migration 0208 took the baseline; `null` when it kept no row.
    pub baseline_taken_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct RefRow {
    place_type: String,
    place_id: Uuid,
    engine_id: Option<Uuid>,
    username: String,
    position: String,
    effect: Option<String>,
    parent_created: DateTime<Utc>,
    parent_updated: Option<DateTime<Utc>>,
    holder_id: Option<Uuid>,
    holder_email: Option<String>,
    holder_created: Option<DateTime<Utc>>,
    minted_at: Option<DateTime<Utc>>,
    retired_at: Option<DateTime<Utc>>,
    minted_for: Option<Uuid>,
    event_at: Option<DateTime<Utc>>,
    event_action: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PolicyRow {
    policy_id: i64,
    policy_name: String,
    service: String,
    managed_by: String,
    updated_at: DateTime<Utc>,
    username: String,
    body: Option<serde_json::Value>,
    holder_id: Option<Uuid>,
    holder_email: Option<String>,
    holder_created: Option<DateTime<Utc>>,
    holder_is_active_admin: Option<bool>,
    /// Whether the holder holds `platform_admin` at all, whatever their status.
    holder_holds_admin: Option<bool>,
    minted_at: Option<DateTime<Utc>>,
    retired_at: Option<DateTime<Utc>>,
    minted_for: Option<Uuid>,
    ledger_source: Option<String>,
    from_baseline: bool,
}

/// Every `user` ref, its holder and ledger row, and the earliest governance
/// event still in retention that names it on its own grant, rule or engine
/// grant. The events are read once, every name each one carries extracted
/// (`events`), and joined: a per-ref scan of `audit.audit_log` would be a
/// nested loop with no index on `(resource_type, resource_id)`. The two
/// shapes are 0208 step 4b's: `"user:<name>"` strings and `{kind: "user",
/// ref}` objects.
const REFS: &str = "\
WITH refs AS ( \
    SELECT 'grant' AS place_type, g.id AS place_id, NULL::uuid AS engine_id, p.ref, \
           p.mode::text AS position, g.effect::text AS effect, g.created_at AS parent_created, \
           NULL::timestamptz AS parent_updated \
      FROM perm.grant_principals p JOIN perm.grants g ON g.id = p.grant_id \
     WHERE p.kind = 'user' \
    UNION ALL \
    SELECT 'rule', r.id, NULL, e.ref, e.mode::text, NULL, r.created_at, NULL \
      FROM perm.rule_exemptions e JOIN perm.rules r ON r.id = e.rule_id \
     WHERE e.kind = 'user' \
    UNION ALL \
    SELECT 'engine_grant', g.id, g.engine_id, p.ref, 'include', 'allow', g.created_at, \
           g.updated_at \
      FROM perm.engine_grant_principals p JOIN perm.engine_grants g ON g.id = p.engine_grant_id \
     WHERE p.kind = 'user' \
), named AS ( \
    SELECT a.id, a.ts, a.action, a.resource_type, a.resource_id, substr(s #>> '{}', 6) AS ref \
      FROM audit.audit_log a, \
           LATERAL jsonb_path_query(a.detail, \
               'strict $.** ? (@.type() == \"string\" && @ starts with \"user:\")') AS s \
     WHERE a.kind = 'mutation' AND a.resource_type IN ('perm_grant', 'perm_rule', 'engine_grant') \
    UNION ALL \
    SELECT a.id, a.ts, a.action, a.resource_type, a.resource_id, s #>> '{}' \
      FROM audit.audit_log a, \
           LATERAL jsonb_path_query(a.detail, \
               'strict $.** ? (@.type() == \"object\" && @.kind == \"user\").ref') AS s \
     WHERE a.kind = 'mutation' AND a.resource_type IN ('perm_grant', 'perm_rule', 'engine_grant') \
), events AS ( \
    SELECT DISTINCT ON (resource_type, resource_id, ref) resource_type, resource_id, ref, ts, action \
      FROM named ORDER BY resource_type, resource_id, ref, ts, id \
) \
SELECT r.place_type, r.place_id, r.engine_id, r.ref AS username, r.position, r.effect, r.parent_created, \
       r.parent_updated, u.id AS holder_id, u.email::text AS holder_email, \
       u.created_at AS holder_created, l.minted_at, l.retired_at, l.minted_for, \
       ev.ts AS event_at, ev.action AS event_action \
  FROM refs r \
  LEFT JOIN users u ON u.username = r.ref::citext \
  LEFT JOIN username_ledger l ON l.username = r.ref::citext \
  LEFT JOIN events ev \
         ON ev.resource_type = CASE r.place_type WHEN 'grant' THEN 'perm_grant' \
                                                 WHEN 'rule' THEN 'perm_rule' \
                                                 ELSE 'engine_grant' END \
        AND ev.resource_id = r.place_id::text AND ev.ref = r.ref \
 ORDER BY r.ref, r.place_type, r.place_id";

/// Every name in every policy body now, and every row of the 0208 baseline,
/// with the holder now, whether they hold `platform_admin` (at all, and as an
/// active user), and the ledger. A baseline holder deleted since is no holder.
const POLICIES: &str = "\
WITH admins AS ( \
    SELECT u.id, u.status FROM users u \
     WHERE (EXISTS (SELECT 1 FROM access_grants g JOIN roles r ON r.id = g.role_id \
                     WHERE r.name = 'platform_admin' AND g.scope_type = 'platform' \
                       AND g.principal_type = 'user' AND g.principal_id = u.id) \
         OR EXISTS (SELECT 1 FROM access_grants g JOIN roles r ON r.id = g.role_id \
                      JOIN user_groups ug ON ug.group_id = g.principal_id \
                     WHERE r.name = 'platform_admin' AND g.scope_type = 'platform' \
                       AND g.principal_type = 'group' AND ug.user_id = u.id)) \
) \
SELECT p.id AS policy_id, p.name AS policy_name, s.name AS service, p.managed_by, \
       p.updated_at, n.name AS username, p.policy_jsonb AS body, u.id AS holder_id, \
       u.email::text AS holder_email, u.created_at AS holder_created, \
       (u.id IN (SELECT id FROM admins WHERE status = 'active')) AS holder_is_active_admin, \
       (u.id IN (SELECT id FROM admins)) AS holder_holds_admin, \
       l.minted_at, l.retired_at, l.minted_for, l.source AS ledger_source, false AS from_baseline \
  FROM policy.authz_policies p \
  JOIN policy.authz_services s ON s.id = p.service_id \
 CROSS JOIN LATERAL policy_body_usernames(p.policy_jsonb) AS n(name) \
  LEFT JOIN users u ON u.username = n.name::citext \
  LEFT JOIN username_ledger l ON l.username = n.name::citext \
UNION ALL \
SELECT b.policy_id, b.policy_name, COALESCE(s.name, b.service_id::text), b.managed_by, \
       b.policy_updated_at, b.username::text, NULL::jsonb, u.id, u.email::text, \
       u.created_at, (u.id IN (SELECT id FROM admins WHERE status = 'active')), \
       (u.id IN (SELECT id FROM admins)), l.minted_at, l.retired_at, l.minted_for, l.source, true \
  FROM username_reuse_baseline b \
  LEFT JOIN policy.authz_services s ON s.id = b.service_id \
  LEFT JOIN users u ON u.id = b.holder_id \
  LEFT JOIN username_ledger l ON l.username = b.username \
 ORDER BY 6, 1";

fn holder(
    id: Option<Uuid>,
    email: Option<String>,
    created: Option<DateTime<Utc>>,
    minted_at: Option<DateTime<Utc>>,
) -> Option<Holder> {
    match (id, created) {
        (Some(id), Some(created_at)) => Some(Holder {
            id,
            email: email.unwrap_or_default(),
            created_at,
            minted_at,
        }),
        _ => None,
    }
}

fn ref_place(r: &RefRow) -> Place {
    let place_type = match r.place_type.as_str() {
        "grant" => PlaceType::Grant,
        "rule" => PlaceType::Rule,
        _ => PlaceType::EngineGrant,
    };
    Place {
        place_type,
        id: Some(r.place_id),
        engine_id: r.engine_id,
        position: (place_type != PlaceType::EngineGrant).then(|| r.position.clone()),
        effect: (place_type == PlaceType::Grant)
            .then(|| r.effect.clone())
            .flatten(),
        policy_id: None,
        name: None,
        service: None,
        managed_by: None,
    }
}

fn noun(place_type: PlaceType) -> &'static str {
    match place_type {
        PlaceType::Grant => "grant",
        PlaceType::Rule => "rule",
        PlaceType::EngineGrant => "engine grant",
        PlaceType::Policy => "policy",
    }
}

fn classify_ref(r: RefRow) -> Option<ReuseFinding> {
    let place = ref_place(&r);
    match holder(
        r.holder_id,
        r.holder_email.clone(),
        r.holder_created,
        r.minted_at,
    ) {
        None => {
            let fails_open = matches!(
                (place.place_type, r.position.as_str(), r.effect.as_deref()),
                (PlaceType::Grant, "include", Some("deny"))
                    | (PlaceType::Grant, "exclude", Some("allow"))
                    | (PlaceType::Rule, "exclude", _)
            );
            Some(ReuseFinding {
                kind: ReuseKind::HolderlessRef,
                verdict: Verdict::Dead,
                username: r.username,
                evidence_at: r.retired_at,
                evidence: format!(
                    "no user holds this name; the {} names nobody",
                    noun(place.place_type)
                ),
                retired: Some(Retired {
                    retired_at: r.retired_at,
                    minted_for: r.minted_for,
                    fails_open,
                }),
                place,
                holder: None,
            })
        }
        // Minted after 0208: nobody held the name before.
        Some(h) if h.minted_at.is_some() => None,
        Some(h) => {
            let (verdict, at, evidence) =
                if let Some(at) = r.event_at.filter(|at| *at < h.created_at) {
                    (
                        Verdict::Definite,
                        at,
                        format!(
                            "governance event {} named user:{} before its holder existed",
                            r.event_action.as_deref().unwrap_or("?"),
                            r.username
                        ),
                    )
                } else if let Some(at) = r.parent_updated.filter(|at| *at < h.created_at) {
                    (
                        Verdict::Definite,
                        at,
                        "the engine grant was last changed before its holder existed".to_string(),
                    )
                } else if r.parent_created < h.created_at {
                    (
                        Verdict::Possible,
                        r.parent_created,
                        format!(
                        "the {} was created before its holder existed; it may have been edited \
                         to name them later",
                        noun(place.place_type)
                    ),
                    )
                } else {
                    return None;
                };
            Some(ReuseFinding {
                kind: ReuseKind::RefPredatesHolder,
                verdict,
                username: r.username,
                place,
                holder: Some(h),
                retired: None,
                evidence_at: Some(at),
                evidence,
            })
        }
    }
}

/// Whether `username` is in a hand-written body's deny items or allow
/// exceptions, the two lists that fail open when the name matches nobody.
fn fails_open_in_body(body: &serde_json::Value, username: &str) -> bool {
    ["denyPolicyItems", "allowExceptions"].iter().any(|list| {
        body[list].as_array().into_iter().flatten().any(|item| {
            item["users"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|u| u.as_str() == Some(username))
        })
    })
}

fn classify_policy(p: PolicyRow) -> Option<ReuseFinding> {
    let place = Place {
        place_type: PlaceType::Policy,
        id: None,
        engine_id: None,
        position: None,
        effect: None,
        policy_id: Some(p.policy_id),
        name: Some(p.policy_name.clone()),
        service: Some(p.service.clone()),
        managed_by: Some(p.managed_by.clone()),
    };
    let when = if p.from_baseline {
        " (at migration 0208)"
    } else {
        ""
    };
    let h = holder(p.holder_id, p.holder_email, p.holder_created, p.minted_at);
    let is_admin_policy =
        p.managed_by == "infra" && is_admin_all_access_policy_name(&p.policy_name);
    if is_admin_policy && !p.holder_is_active_admin.unwrap_or(false) {
        // Naming nobody: the name is retired and matches nobody.
        let h = h?;
        let (verdict, evidence) = if h.minted_at.is_some() || p.holder_holds_admin.unwrap_or(false)
        {
            // The same person, disabled or demoted after the body named them
            // as an admin. The live sync converges the body; a baseline row
            // records what that sync has since repaired.
            if p.from_baseline {
                return None;
            }
            (
                Verdict::Stale,
                "the egret-admin body still gives engine ALL access to someone who is no \
                 longer an active platform admin; the sync has not run since"
                    .to_string(),
            )
        } else if p.updated_at < h.created_at {
            (
                Verdict::Definite,
                format!(
                    "the egret-admin body{when} was last written before its holder existed, \
                     and they are not a platform admin"
                ),
            )
        } else {
            (
                Verdict::Possible,
                format!(
                    "the egret-admin body{when} names someone who holds no platform_admin \
                     grant: a demoted admin, or someone given a deleted admin's name after \
                     they joined"
                ),
            )
        };
        return Some(ReuseFinding {
            kind: ReuseKind::AdminPolicyNamedNonAdmin,
            verdict,
            username: p.username,
            place,
            holder: Some(h),
            retired: None,
            evidence_at: Some(p.updated_at),
            evidence,
        });
    }
    match h {
        // Minted after 0208: nobody held the name before.
        Some(h) if h.minted_at.is_some() => None,
        Some(h) if p.updated_at < h.created_at => Some(ReuseFinding {
            kind: ReuseKind::PolicyPredatesHolder,
            verdict: if p.managed_by == "operator" {
                Verdict::Possible
            } else {
                Verdict::Definite
            },
            username: p.username,
            place,
            holder: Some(h),
            retired: None,
            evidence_at: Some(p.updated_at),
            evidence: format!("the policy body{when} was last written before its holder existed"),
        }),
        Some(_) => None,
        // A name the ledger does not hold is free: the next person whose
        // email slugs to it is given it (a pre-provisioned name).
        None if p.ledger_source.is_none() => None,
        None if !p.from_baseline && p.managed_by == "operator" => Some(ReuseFinding {
            kind: ReuseKind::HolderlessRef,
            verdict: Verdict::Dead,
            evidence: holderless_policy_evidence(p.minted_for, p.ledger_source.as_deref()),
            retired: Some(Retired {
                retired_at: p.retired_at,
                minted_for: p.minted_for,
                fails_open: p
                    .body
                    .as_ref()
                    .is_some_and(|b| fails_open_in_body(b, &p.username)),
            }),
            evidence_at: p.retired_at,
            username: p.username,
            place,
            holder: None,
        }),
        None => None,
    }
}

/// The evidence of a hand-written body naming a retired name. A name 0208
/// retired with no record of a holder (it found the name in a body, ref or
/// event) may have been written for someone who had not signed in yet: they
/// were given another name, and the body never applies to them.
fn holderless_policy_evidence(minted_for: Option<Uuid>, source: Option<&str>) -> String {
    let base = "no user holds this name; the hand-written policy names nobody";
    match (minted_for, source) {
        (None, Some(source)) if source.starts_with("backfill_") => format!(
            "{base}. It was retired, with no record of who held it, when username reuse \
             protection was installed: if it was written for someone who had not signed in \
             yet, rewrite it to the name they were given"
        ),
        _ => base.to_string(),
    }
}

/// Every finding, ordered by username then place. Read-only; run it in one
/// `REPEATABLE READ, READ ONLY` transaction.
pub async fn findings_via(conn: &mut PgConnection) -> Result<UsernameReuseReport, sqlx::Error> {
    let refs: Vec<RefRow> = sqlx::query_as(REFS).fetch_all(&mut *conn).await?;
    let policies: Vec<PolicyRow> = sqlx::query_as(POLICIES).fetch_all(&mut *conn).await?;
    let baseline_taken_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT min(taken_at) FROM username_reuse_baseline")
            .fetch_one(&mut *conn)
            .await?;
    let mut findings: Vec<ReuseFinding> = refs
        .into_iter()
        .filter_map(classify_ref)
        .chain(policies.into_iter().filter_map(classify_policy))
        .collect();
    findings.sort_by(|a, b| {
        (a.username.as_str(), noun(a.place.place_type))
            .cmp(&(b.username.as_str(), noun(b.place.place_type)))
    });
    findings.dedup();
    Ok(UsernameReuseReport {
        generated_at: Utc::now(),
        findings,
        baseline_taken_at,
    })
}
