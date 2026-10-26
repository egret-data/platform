//! The Governance Overview (DG #3, spec `docs/en/specs/2026-10-18-dg-overview-design.md`
//! §4): `GET /governance/attention`, what needs attention, filtered by the
//! reader's permissions.
//!
//! Every source runs in its own savepoint of ONE `REPEATABLE READ, READ ONLY`
//! transaction with a 5 s statement timeout, so the plugin ledger, the engines
//! and the profiles are judged on one state, and a failing source becomes a
//! `source_unavailable` finding instead of aborting the others (in Postgres one
//! failed statement aborts the transaction; a savepoint scopes that).
//!
//! The drift rules (`plugin_silent`, `plugin_behind`, `plugin_quiet`) follow
//! the live measurement of `docs/en/plans/2026-10-18-dg-overview-live.md`
//! §1.1:
//! - liveness reads the NEWEST `policies` poll per profile, so the rows a
//!   rescheduled pod leaves behind (they keep their last `at` for 24 h) never
//!   raise a finding;
//! - a report of `lastKnownVersion = -1` is "not yet reported": it neither
//!   confirms nor counts as behind. Trino's tag retriever reports `-1` for one
//!   tag poll after every policy download, and every plugin reports it on its
//!   first poll after a restart. The cost: a plugin that never reports a
//!   version at all is not flagged as behind (it still polls, so it is not
//!   quiet either);
//! - a kind is judged only for a profile that has a `tags` (or `policies`) row
//!   polled within 24 h: StarRocks never downloads tags, so a missing tags
//!   download is never reported for it.
use std::collections::{BTreeMap, HashSet};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use egret_perm_compile::table_columns::{table_states_via, TableKey, TableState};
use futures_util::future::BoxFuture;
use serde::Serialize;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::plugin_judgement::{
    expects_tags_via, judge_plugins, ledger_rows_via, KindState, LedgerRow, Liveness,
    ProfileVersions,
};
use crate::{CpError, CpState};

/// At most this many samples per finding.
const SAMPLE: usize = 5;
/// The permissions a source may need.
const POLICY_READ: &str = "policy:read";
const TAG_READ: &str = "tag:read";
const ENGINE_READ: &str = "engine:read";
const CATALOG_READ: &str = "catalog_registry:read";
const KNOWN_PERMISSIONS: [&str; 4] = [POLICY_READ, TAG_READ, ENGINE_READ, CATALOG_READ];
/// Not a permission: the `platform_admin` role at platform scope, held as one
/// in `held` (username-takeover spec §4.5). The username-reuse source shows
/// emails and holders across workspaces, so only an admin runs it; everyone
/// else gets it in `notChecked`.
const PLATFORM_ADMIN: &str = "platform_admin";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Warning,
    Info,
}

/// Where a finding or a sample is fixed; the client builds the URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ref {
    Grant {
        id: Uuid,
    },
    Rule {
        id: Uuid,
    },
    Unmatched,
    Profile {
        name: String,
    },
    Profiles,
    Engine {
        id: Uuid,
        name: String,
    },
    Engines,
    Catalog {
        id: Uuid,
        name: String,
    },
    Catalogs,
    Object {
        catalog: String,
        schema: String,
        object: String,
    },
    Tag {
        id: Uuid,
        name: String,
    },
    Tags,
    /// A role's People page (a role event of the changes feed).
    Role {
        name: String,
    },
    /// The Governance Data area's index.
    Data,
    DataCatalog {
        name: String,
    },
    /// Governance > People > Reused names (the username-reuse report).
    ReusedNames,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub label: String,
    pub detail: Option<String>,
    #[serde(rename = "ref")]
    pub r#ref: Option<Ref>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub kind: String,
    pub severity: Severity,
    /// The true total; `sample` holds at most five.
    pub count: i64,
    pub sentence: String,
    pub sample: Vec<Sample>,
    #[serde(rename = "ref")]
    pub r#ref: Option<Ref>,
    /// Set for `source_unavailable`: the kind that could not be checked.
    pub source: Option<String>,
}

/// A check the reader may not run, and the permissions it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotChecked {
    pub kind: String,
    /// All of these, or (for `Any`) one of them.
    pub requires: Vec<String>,
    /// `true` when holding any one of `requires` is enough.
    pub any: bool,
}

/// Counts the reader may read; `null` for one they may not (never `0`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub catalogs: Option<i64>,
    pub grants: Option<i64>,
    pub rules: Option<i64>,
    pub tags: Option<i64>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttentionResponse {
    pub generated_at: DateTime<Utc>,
    pub counts: Counts,
    /// Critical, warning, info; then by kind.
    pub findings: Vec<Finding>,
    pub not_checked: Vec<NotChecked>,
}

/// Why a source could not answer; becomes a `source_unavailable` finding.
#[derive(Debug)]
pub struct SourceError(pub String);

impl<E: std::fmt::Display> From<E> for SourceError {
    fn from(e: E) -> Self {
        SourceError(e.to_string())
    }
}

pub type SourceFn =
    for<'c> fn(&'c mut PgConnection) -> BoxFuture<'c, Result<Option<Finding>, SourceError>>;

/// Which permissions a source needs.
#[derive(Debug, Clone, Copy)]
pub enum Requires {
    All(&'static [&'static str]),
    Any(&'static [&'static str]),
}

impl Requires {
    fn held_by(&self, held: &HashSet<&str>) -> bool {
        match self {
            Requires::All(ps) => ps.iter().all(|p| held.contains(p)),
            Requires::Any(ps) => ps.iter().any(|p| held.contains(p)),
        }
    }
    fn not_checked(&self, kind: &str) -> NotChecked {
        let (ps, any) = match self {
            Requires::All(ps) => (ps, false),
            Requires::Any(ps) => (ps, true),
        };
        NotChecked {
            kind: kind.to_string(),
            requires: ps.iter().map(|p| p.to_string()).collect(),
            any,
        }
    }
}

/// One check of the Overview.
pub struct Source {
    pub kind: &'static str,
    /// The object of "Could not check …".
    pub what: &'static str,
    pub requires: Requires,
    pub run: SourceFn,
}

const POLICY: Requires = Requires::All(&[POLICY_READ]);
const DRIFT: Requires = Requires::All(&[POLICY_READ, ENGINE_READ]);
const GOVERNANCE: Requires = Requires::Any(&[POLICY_READ, TAG_READ]);

/// Every source, in no particular order (findings are sorted).
pub const SOURCES: &[Source] = &[
    Source {
        kind: "unresolved_principals_fail_open",
        what: "unresolved principal references",
        requires: POLICY,
        run: |c| Box::pin(unresolved_principals(c)),
    },
    Source {
        kind: "plugin_silent",
        what: "plugin liveness",
        requires: DRIFT,
        run: |c| Box::pin(drift(c, Drift::Silent)),
    },
    Source {
        kind: "plugin_behind",
        what: "plugin versions",
        requires: DRIFT,
        run: |c| Box::pin(drift(c, Drift::Behind)),
    },
    Source {
        kind: "plugin_quiet",
        what: "plugin liveness",
        requires: DRIFT,
        run: |c| Box::pin(drift(c, Drift::Quiet)),
    },
    Source {
        kind: "engine_auth_failing",
        what: "engine Ranger credentials",
        requires: Requires::All(&[ENGINE_READ]),
        run: |c| Box::pin(engine_auth_failing(c)),
    },
    Source {
        kind: "compile_failed",
        what: "profile compiles",
        requires: POLICY,
        run: |c| Box::pin(compile_failed(c)),
    },
    Source {
        kind: "engine_failed",
        what: "engine status",
        requires: Requires::All(&[ENGINE_READ]),
        run: |c| Box::pin(engine_failed(c)),
    },
    Source {
        kind: "rule_overlaps",
        what: "rule overlaps",
        requires: POLICY,
        run: |c| Box::pin(rule_overlaps(c)),
    },
    Source {
        kind: "lineage_failed",
        what: "view lineage",
        requires: GOVERNANCE,
        run: |c| Box::pin(lineage_failed(c)),
    },
    Source {
        kind: "catalog_apply_failed",
        what: "catalog attachments",
        requires: Requires::All(&[CATALOG_READ]),
        run: |c| Box::pin(catalog_apply_failed(c)),
    },
    Source {
        kind: "policy_warnings",
        what: "policy warnings",
        requires: POLICY,
        run: |c| Box::pin(policy_warnings(c)),
    },
    Source {
        kind: "plugin_unmodelled",
        what: "engine plugin versions",
        requires: Requires::All(&[ENGINE_READ]),
        run: |c| Box::pin(plugin_unmodelled(c)),
    },
    Source {
        kind: "tags_without_clearance",
        what: "classification clearances",
        requires: Requires::All(&[TAG_READ]),
        run: |c| Box::pin(tags_without_clearance(c)),
    },
    Source {
        kind: "username_reuse",
        what: "username reuse",
        requires: Requires::All(&[PLATFORM_ADMIN]),
        run: |c| Box::pin(username_reuse(c)),
    },
    Source {
        kind: "classifications_on_missing_tables",
        what: "classified tables",
        requires: Requires::All(&[TAG_READ]),
        run: |c| Box::pin(classifications_on_tables(c, TableState::Missing)),
    },
    Source {
        kind: "classifications_on_dropped_tables",
        what: "classified tables",
        requires: Requires::All(&[TAG_READ]),
        run: |c| Box::pin(classifications_on_tables(c, TableState::Dropped)),
    },
    Source {
        kind: "catalogs_without_engine",
        what: "catalog attachments",
        requires: GOVERNANCE,
        run: |c| Box::pin(catalogs_without_engine(c)),
    },
];

pub(crate) fn router() -> Router<CpState> {
    Router::new().route("/governance/attention", get(attention))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(attention),
    components(schemas(AttentionResponse, Counts, Finding, NotChecked, Ref, Sample, Severity))
)]
pub(crate) struct ControlPlaneGovernanceOverviewApi;

/// What needs attention in governance, per the reader's permissions.
#[utoipa::path(
    get,
    path = "/api/control-plane/governance/attention",
    tag = "control-plane-governance",
    description = "Findings computed from egret's own records, each only for a reader holding \
                   its source's permissions; the rest are listed in `notChecked`. A source that \
                   fails is reported as a `source_unavailable` finding and the others still \
                   answer. Counts the reader may not read are `null`.",
    responses(
        (status = 200, description = "Counts, findings and the checks not run for this reader", body = AttentionResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Holds neither policy:read nor tag:read", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn attention(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<AttentionResponse>, CpError> {
    crate::governance_browse::require_governance_read(&st, &p).await?;
    let mut held: HashSet<&str> = HashSet::new();
    for perm in KNOWN_PERMISSIONS {
        match egret_core::authz::require(&*st.pdp, &p, perm).await {
            Ok(()) => {
                held.insert(perm);
            }
            Err(egret_core::Error::Forbidden) => {}
            Err(e) => return Err(e.into()),
        }
    }
    if p.platform_roles.iter().any(|r| r == PLATFORM_ADMIN) {
        held.insert(PLATFORM_ADMIN);
    }
    let mut tx = begin_snapshot(&st).await?;
    let response = attention_via(&mut tx, &held, SOURCES).await;
    tx.rollback().await.map_err(db("end the snapshot"))?;
    Ok(Json(response))
}

fn db(what: &'static str) -> impl FnOnce(sqlx::Error) -> CpError {
    move |e| {
        tracing::error!(error = %e, "governance overview: {what}");
        CpError::Internal
    }
}

/// One `REPEATABLE READ, READ ONLY` transaction with a 5 s statement timeout.
pub async fn begin_snapshot(st: &CpState) -> Result<Transaction<'static, Postgres>, CpError> {
    let mut tx = st.db.pool.begin().await.map_err(db("begin"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(db("set the snapshot"))?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *tx)
        .await
        .map_err(db("set the timeout"))?;
    Ok(tx)
}

/// Run every source the reader may run, each in its own savepoint of `tx`,
/// and the counts it may read.
pub async fn attention_via(
    tx: &mut Transaction<'_, Postgres>,
    held: &HashSet<&str>,
    sources: &[Source],
) -> AttentionResponse {
    let mut findings = Vec::new();
    let mut not_checked = Vec::new();
    for s in sources {
        if !s.requires.held_by(held) {
            not_checked.push(s.requires.not_checked(s.kind));
            continue;
        }
        match savepoint(tx, s.run).await {
            Ok(Some(f)) => findings.push(f),
            Ok(None) => {}
            Err(e) => findings.push(unavailable(s, &e.0)),
        }
    }
    findings.sort_by(|a, b| (a.severity, &a.kind).cmp(&(b.severity, &b.kind)));
    let can = |p: &str| held.contains(p);
    let counts = Counts {
        catalogs: count(tx, "SELECT count(*) FROM registry.catalogs", true).await,
        grants: count(tx, "SELECT count(*) FROM perm.grants", can(POLICY_READ)).await,
        rules: count(tx, "SELECT count(*) FROM perm.rules", can(POLICY_READ)).await,
        tags: count(tx, "SELECT count(*) FROM tags", can(TAG_READ)).await,
    };
    AttentionResponse {
        generated_at: Utc::now(),
        counts,
        findings,
        not_checked,
    }
}

/// Run `run` in a savepoint of `tx`: released on success, rolled back on
/// failure so the snapshot stays usable for the next source.
async fn savepoint(
    tx: &mut Transaction<'_, Postgres>,
    run: SourceFn,
) -> Result<Option<Finding>, SourceError> {
    sqlx::query("SAVEPOINT governance_source")
        .execute(&mut **tx)
        .await?;
    match run(tx).await {
        Ok(f) => {
            sqlx::query("RELEASE SAVEPOINT governance_source")
                .execute(&mut **tx)
                .await?;
            Ok(f)
        }
        Err(e) => {
            sqlx::query("ROLLBACK TO SAVEPOINT governance_source")
                .execute(&mut **tx)
                .await?;
            Err(e)
        }
    }
}

/// A count the reader may read, or `None`. A count that fails is omitted too
/// (and logged): a label never claims a number it does not have.
async fn count(
    tx: &mut Transaction<'_, Postgres>,
    sql: &'static str,
    allowed: bool,
) -> Option<i64> {
    if !allowed {
        return None;
    }
    let run = async {
        sqlx::query("SAVEPOINT governance_count")
            .execute(&mut **tx)
            .await?;
        let n: Result<i64, sqlx::Error> = sqlx::query_scalar(sql).fetch_one(&mut **tx).await;
        let end = if n.is_ok() {
            "RELEASE SAVEPOINT governance_count"
        } else {
            "ROLLBACK TO SAVEPOINT governance_count"
        };
        sqlx::query(end).execute(&mut **tx).await?;
        n
    };
    let run: Result<i64, sqlx::Error> = run.await;
    match run {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::warn!(error = %e, sql, "governance overview: a count failed");
            None
        }
    }
}

/// A source that failed: a fixed sentence for the page, the database's own
/// error in the log (review M1: it can name schema internals and says nothing
/// a steward can act on).
fn unavailable(s: &Source, error: &str) -> Finding {
    tracing::warn!(
        source = s.kind,
        error,
        "governance overview: a source failed"
    );
    Finding {
        kind: "source_unavailable".into(),
        severity: Severity::Warning,
        count: 1,
        sentence: format!("Could not check {}.", s.what),
        sample: Vec::new(),
        r#ref: None,
        source: Some(s.kind.to_string()),
    }
}

fn finding(
    kind: &str,
    severity: Severity,
    count: usize,
    sentence: String,
    sample: Vec<Sample>,
    r#ref: Option<Ref>,
) -> Option<Finding> {
    (count > 0).then(|| Finding {
        kind: kind.into(),
        severity,
        count: count as i64,
        sentence,
        sample: sample.into_iter().take(SAMPLE).collect(),
        r#ref,
        source: None,
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The longest first line of an error a sample shows, in characters.
const DETAIL_CHARS: usize = 200;

/// The first line of `s`, cut to [`DETAIL_CHARS`] with an ellipsis.
fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > DETAIL_CHARS {
        let mut cut: String = line.chars().take(DETAIL_CHARS).collect();
        cut.push('…');
        cut
    } else {
        line.to_string()
    }
}

/// An overlap's coordinate (`catalog=[ice] column=[email] schema=[s]
/// table=[t]`, Ranger's resource map) as a path, `ice.s.t.email`, or
/// `tag pii` for a tag coordinate. Anything else is shown as it came.
fn coordinate_path(coordinate: &str) -> String {
    let mut levels: BTreeMap<&str, &str> = BTreeMap::new();
    for part in coordinate.split_whitespace() {
        if let Some((k, v)) = part.split_once("=[") {
            levels.insert(k, v.trim_end_matches(']'));
        }
    }
    if let Some(tag) = levels.get("tag") {
        return format!("tag {tag}");
    }
    let path: Vec<&str> = ["catalog", "schema", "table", "column"]
        .iter()
        .map_while(|k| levels.get(k).copied())
        .collect();
    if path.is_empty() {
        coordinate.to_string()
    } else {
        path.join(".")
    }
}

// ── sources ─────────────────────────────────────────────────────────────

/// Past username takeovers (username-takeover spec §4.5): `critical` when
/// any finding is `definite`, else `warning` for `possible` and `stale` ones.
/// Refs nobody holds (`dead`) are no finding here:
/// `unresolved_principals_fail_open` already reports the ones that fail open.
/// The sentence says, per verdict, only what the evidence shows. Samples name
/// the username and the place, never an email.
async fn username_reuse(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    use egret_perm_compile::username_reuse::{findings_via, PlaceType, Verdict};
    let rows: Vec<_> = findings_via(c)
        .await?
        .findings
        .into_iter()
        .filter(|f| f.verdict != Verdict::Dead)
        .collect();
    let severity = if rows.iter().any(|f| f.verdict == Verdict::Definite) {
        Severity::Critical
    } else {
        Severity::Warning
    };
    let sample = rows
        .iter()
        .map(|f| {
            let (detail, r#ref) = match f.place.place_type {
                PlaceType::Grant => (
                    "a grant".to_string(),
                    f.place.id.map(|id| Ref::Grant { id }),
                ),
                PlaceType::Rule => ("a rule".to_string(), f.place.id.map(|id| Ref::Rule { id })),
                PlaceType::EngineGrant => ("an engine grant".to_string(), None),
                PlaceType::Policy => {
                    let service = f.place.service.clone().unwrap_or_default();
                    (
                        format!(
                            "policy {} on profile {service}",
                            f.place.name.as_deref().unwrap_or("")
                        ),
                        Some(Ref::Profile { name: service }),
                    )
                }
            };
            Sample {
                label: f.username.clone(),
                detail: Some(detail),
                r#ref,
            }
        })
        .collect();
    let count = |v: Verdict| rows.iter().filter(|f| f.verdict == v).count();
    Ok(finding(
        "username_reuse",
        severity,
        rows.len(),
        reuse_sentence(
            count(Verdict::Definite),
            count(Verdict::Possible),
            count(Verdict::Stale),
        ),
        sample,
        Some(Ref::ReusedNames),
    ))
}

/// What the `username_reuse` finding says: `definite` findings were written
/// before the current holder of the name existed, `possible` ones may have
/// been, and `stale` ones are `egret-admin` bodies the sync has not caught up
/// with (no reuse).
fn reuse_sentence(definite: usize, possible: usize, stale: usize) -> String {
    const PLACES: (&str, &str) = ("reference or policy", "references or policies");
    const WRITTEN: &str = "written for a username before the person who holds it now existed";
    let mut parts = Vec::new();
    if definite > 0 {
        let verb = if definite == 1 { "was" } else { "were" };
        parts.push(format!(
            "{} {verb} {WRITTEN}.",
            plural(definite, PLACES.0, PLACES.1)
        ));
    }
    if possible > 0 && definite > 0 {
        parts.push(format!("{possible} more may have been."));
    } else if possible > 0 {
        parts.push(format!(
            "{} may have been {WRITTEN}.",
            plural(possible, PLACES.0, PLACES.1)
        ));
    }
    if stale > 0 {
        let verb = if stale == 1 { "gives" } else { "give" };
        parts.push(format!(
            "{} still {verb} engine ALL access to someone who is no longer an active platform \
             admin.",
            plural(stale, "egret-admin policy", "egret-admin policies")
        ));
    }
    parts.join(" ")
}

async fn unresolved_principals(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<_> = egret_perm_compile::principal_refs::unresolved_principals_via(c)
        .await?
        .into_iter()
        .filter(|r| r.fails_open)
        .collect();
    let sample = rows
        .iter()
        .map(|r| Sample {
            label: format!("{} {}", r.kind, r.r#ref),
            detail: Some(r.reason.clone()),
            r#ref: Some(if r.source == "rule" {
                Ref::Rule { id: r.id }
            } else {
                Ref::Grant { id: r.id }
            }),
        })
        .collect();
    Ok(finding(
        "unresolved_principals_fail_open",
        Severity::Critical,
        rows.len(),
        if rows.len() == 1 {
            "1 stored principal reference matches nobody, so the compiled policy allows more than \
             the model states."
                .to_string()
        } else {
            format!(
                "{} stored principal references match nobody, so the compiled policy allows more \
                 than the model states.",
                rows.len()
            )
        },
        sample,
        Some(Ref::Unmatched),
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drift {
    Silent,
    Behind,
    Quiet,
}

/// A profile bound to at least one provisioned, running engine.
struct DriftProfile {
    name: String,
    policy_version: i64,
    tag_version: i64,
    updated_at: DateTime<Utc>,
    /// `(id, name)` of its provisioned running engines, by name.
    running: Vec<(Uuid, String)>,
    binds_external: bool,
}

/// One (profile, bound engine) pair: the profile's id, name, policy and tag
/// versions and `updated_at`; the engine's id, name, origin and status.
type BoundEngineRow = (
    i64,
    String,
    i64,
    i64,
    DateTime<Utc>,
    Uuid,
    String,
    String,
    String,
);

async fn drift_profiles(c: &mut PgConnection) -> Result<BTreeMap<i64, DriftProfile>, SourceError> {
    let rows: Vec<BoundEngineRow> = sqlx::query_as(
        "SELECT s.id, s.name, s.policy_version, s.tag_version, s.updated_at, \
                    e.id, e.name, e.origin, e.status \
               FROM policy.authz_services s \
               JOIN engine_mgmt.engines e ON e.authz_service_id = s.id \
              ORDER BY s.name, e.name",
    )
    .fetch_all(&mut *c)
    .await?;
    let mut out: BTreeMap<i64, DriftProfile> = BTreeMap::new();
    for (sid, name, pv, tv, at, eid, ename, origin, status) in rows {
        let p = out.entry(sid).or_insert_with(|| DriftProfile {
            name,
            policy_version: pv,
            tag_version: tv,
            updated_at: at,
            running: Vec::new(),
            binds_external: false,
        });
        if origin == egret_perm::plugin_report::ORIGIN_EXTERNAL {
            p.binds_external = true;
        } else if status == "running" {
            p.running.push((eid, ename));
        }
    }
    // An external engine is never dialled and never judged (spec §4.3).
    out.retain(|_, p| !p.running.is_empty());
    Ok(out)
}

const EXTERNAL_NOTE: &str =
    "egret does not run external engines and cannot tell whether they should be polling.";

async fn drift(c: &mut PgConnection, which: Drift) -> Result<Option<Finding>, SourceError> {
    let profiles = drift_profiles(c).await?;
    let ids: Vec<i64> = profiles.keys().copied().collect();
    let ledger = ledger_rows_via(c, &ids).await?;
    let expects_tags = expects_tags_via(c, &ids).await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *c)
        .await?;
    let mut sample = Vec::new();
    let mut sentences = Vec::new();
    for (sid, p) in &profiles {
        let rows: Vec<&LedgerRow> = ledger.iter().filter(|r| r.service_id == *sid).collect();
        let versions = ProfileVersions {
            policy_version: p.policy_version,
            tag_version: p.tag_version,
            updated_at: p.updated_at,
            expects_tags: expects_tags.contains(sid),
        };
        let j = judge_plugins(&versions, &rows, now);
        let (engine_id, engine) = p.running[0].clone();
        let sentence = match which {
            Drift::Silent if j.liveness == Liveness::Silent => Some(format!(
                "No plugin has polled profile {} in 24 hours, while engine {engine} is recorded \
                 as running. egret cannot tell a stopped engine from one that cannot reach egret \
                 or does not poll.",
                p.name
            )),
            Drift::Quiet => match (j.liveness, j.tags) {
                (Liveness::Quiet { age_secs }, _) => Some(format!(
                    "No plugin has polled profile {} for {} minutes.",
                    p.name,
                    age_secs / 60
                )),
                // Review M6: the tags kind on its own, polled 5 min to 24 h
                // ago while the policies are polled.
                (Liveness::Live, KindState::Quiet { age_secs }) => Some(format!(
                    "No plugin has polled the tags of profile {} for {} minutes, while its \
                     policies are polled.",
                    p.name,
                    age_secs / 60
                )),
                _ => None,
            },
            // The first kind behind, policies before tags.
            Drift::Behind => [(j.policies, "policy"), (j.tags, "tag")]
                .into_iter()
                .find_map(|(k, what)| match k {
                    KindState::Behind { version } => Some(format!(
                        "No plugin of profile {} has reported holding {what} version {version}, \
                         set at {}.",
                        p.name,
                        p.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
                    )),
                    _ => None,
                }),
            _ => None,
        };
        if let Some(mut s) = sentence {
            if p.binds_external {
                s = format!("{s} {EXTERNAL_NOTE}");
            }
            sentences.push(s.clone());
            sample.push(Sample {
                label: p.name.clone(),
                detail: Some(s),
                r#ref: Some(Ref::Engine {
                    id: engine_id,
                    name: engine,
                }),
            });
        }
    }
    let n = sample.len();
    let (kind, severity) = match which {
        Drift::Silent => ("plugin_silent", Severity::Critical),
        Drift::Behind => ("plugin_behind", Severity::Critical),
        Drift::Quiet => ("plugin_quiet", Severity::Warning),
    };
    let sentence = if n == 1 {
        sentences.remove(0)
    } else {
        match which {
            Drift::Silent => format!(
                "No plugin has polled {n} profiles in 24 hours, while an engine bound to each is \
                 recorded as running. egret cannot tell a stopped engine from one that cannot \
                 reach egret or does not poll."
            ),
            Drift::Behind => {
                format!("No plugin of {n} profiles has reported holding the version set for it.")
            }
            Drift::Quiet => format!("No plugin has polled {n} profiles for 5 minutes or more."),
        }
    };
    let r#ref = if n == 1 {
        // The finding's sentence is this profile's: its sample need not repeat it.
        sample[0].detail = None;
        Some(Ref::Profile {
            name: sample[0].label.clone(),
        })
    } else {
        Some(Ref::Profiles)
    };
    Ok(finding(kind, severity, n, sentence, sample, r#ref))
}

/// A rejection older than this is not reported: the engine stopped presenting
/// the credential (stopped, deleted outside egret, or reconfigured).
const AUTH_FAILURE_RECENT_SECS: i64 = 24 * 3600;

/// Engines whose Ranger credential is being rejected (spec
/// `2026-10-20-ranger-surface-auth` A12): the latest rejection is newer than
/// the engine's latest successful download, and within 24 h. Per engine, not
/// per profile: a healthy engine sharing the profile does not hide a failing
/// one. The count is a lower bound (rejections are coalesced per engine).
async fn engine_auth_failing(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let failures = egret_policy::auth_failures_via(&mut *c).await?;
    let names: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM engine_mgmt.engines ORDER BY name")
            .fetch_all(&mut *c)
            .await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *c)
        .await?;
    let at = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let mut rows: Vec<(String, Uuid, String)> = Vec::new();
    for (id, name) in names {
        let Some(f) = failures.iter().find(|f| f.engine_id == id) else {
            continue;
        };
        if !f.is_outstanding() || (now - f.last_at).num_seconds() >= AUTH_FAILURE_RECENT_SECS {
            continue;
        }
        let since = match f.ok_at {
            None => "no successful download since.".to_string(),
            Some(ok) => format!("the last successful download at {}.", at(ok)),
        };
        let detail = format!(
            "{}, the last at {}; {since}",
            plural(
                f.count as usize,
                "rejected credential",
                "rejected credentials"
            ),
            at(f.last_at)
        );
        rows.push((name, id, detail));
    }
    let n = rows.len();
    let (sentence, r#ref) = match rows.as_slice() {
        [(name, id, detail)] => (
            format!(
                "The Ranger credential of engine {name} is being rejected: {detail} Its plugin \
                 cannot download policies and runs on its last cached copy, if any."
            ),
            Some(Ref::Engine {
                id: *id,
                name: name.clone(),
            }),
        ),
        _ => (
            format!(
                "The Ranger credential of {n} engines is being rejected, so their plugins cannot \
                 download policies and run on their last cached copy, if any."
            ),
            Some(Ref::Engines),
        ),
    };
    let sample = rows
        .into_iter()
        .map(|(name, id, detail)| Sample {
            label: name.clone(),
            detail: (n > 1).then_some(detail),
            r#ref: Some(Ref::Engine { id, name }),
        })
        .collect();
    Ok(finding(
        "engine_auth_failing",
        Severity::Critical,
        n,
        sentence,
        sample,
        r#ref,
    ))
}

/// The rules a recorded compile error names: a refusal labels each rule by its
/// (unique) name in quotes, as the order-ambiguous refusal does for the two
/// rules of a tie (D2, user decision U6). Matched against `perm.rules`, so a
/// rule deleted since is not linked.
async fn rules_named_in(
    c: &mut PgConnection,
    error: &str,
) -> Result<Vec<(Uuid, String)>, SourceError> {
    Ok(sqlx::query_as(
        "SELECT id, name FROM perm.rules WHERE strpos($1, '\"' || name || '\"') > 0 ORDER BY name",
    )
    .bind(error)
    .fetch_all(&mut *c)
    .await?)
}

async fn compile_failed(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let runs = egret_perm_compile::compile_runs::failed_compile_runs_via(c).await?;
    let mut sample = Vec::new();
    let mut named: Vec<String> = Vec::new();
    for r in &runs {
        let error = r.error.as_deref().unwrap_or("");
        sample.push(Sample {
            label: r.service_name.clone(),
            detail: Some(format!(
                "{} (failed {}; last success {})",
                first_line(error),
                r.attempted_at.format("%Y-%m-%d %H:%M UTC"),
                r.last_ok_at.map_or_else(
                    || "none recorded".to_string(),
                    |t| t.format("%Y-%m-%d %H:%M UTC").to_string()
                )
            )),
            r#ref: Some(Ref::Profile {
                name: r.service_name.clone(),
            }),
        });
        // Each rule the refusal names, linked, right after its profile.
        for (id, name) in rules_named_in(c, error).await? {
            sample.push(Sample {
                label: name.clone(),
                detail: Some(format!(
                    "named by the refusal in profile {}",
                    r.service_name
                )),
                r#ref: Some(Ref::Rule { id }),
            });
            if !named.contains(&name) {
                named.push(name);
            }
        }
    }
    let n = runs.len();
    let r#ref = match runs.as_slice() {
        [one] => Some(Ref::Profile {
            name: one.service_name.clone(),
        }),
        _ => Some(Ref::Profiles),
    };
    let (subject, verb) = if n == 1 {
        ("The refusal", "names")
    } else {
        ("The refusals", "name")
    };
    let names = match named.as_slice() {
        [] => String::new(),
        [one] => format!(" {subject} {verb} rule {one}."),
        [init @ .., last] => format!(" {subject} {verb} rules {} and {last}.", init.join(", ")),
    };
    Ok(finding(
        "compile_failed",
        Severity::Critical,
        n,
        format!(
            "The last compile of {} failed; engines keep the previous compile.{names}",
            plural(n, "profile", "profiles")
        ),
        sample,
        r#ref,
    ))
}

async fn engine_failed(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM engine_mgmt.engines \
          WHERE origin = 'provisioned' AND status = 'failed' ORDER BY name",
    )
    .fetch_all(&mut *c)
    .await?;
    let n = rows.len();
    let sample = rows
        .into_iter()
        .map(|(id, name)| Sample {
            label: name.clone(),
            detail: None,
            r#ref: Some(Ref::Engine { id, name }),
        })
        .collect();
    Ok(finding(
        "engine_failed",
        Severity::Critical,
        n,
        format!(
            "{} egret provisions failed.",
            plural(n, "engine", "engines")
        ),
        sample,
        Some(Ref::Engines),
    ))
}

async fn rule_overlaps(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let pairs = egret_perm_compile::overlap::stored_overlaps_via(c).await?;
    let n = pairs.len();
    let sample = pairs
        .iter()
        .map(|o| Sample {
            label: o
                .rules
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(" and "),
            detail: Some(format!(
                "{}, in profile {}",
                coordinate_path(&o.coordinate),
                o.service_name
            )),
            r#ref: o.rules.iter().find_map(|m| m.id).map(|id| Ref::Rule { id }),
        })
        .collect();
    Ok(finding(
        "rule_overlaps",
        Severity::Warning,
        n,
        format!(
            "{} may decide one request differently; which one an engine applies is not \
             something either states.",
            plural(
                n,
                "pair of mask or row-filter rules",
                "pairs of mask or row-filter rules"
            )
        ),
        sample,
        Some(Ref::Profiles),
    ))
}

async fn lineage_failed(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT view_catalog, view_schema, view_name, detail FROM view_lineage_state \
          WHERE state = 'failed' ORDER BY view_catalog, view_schema, view_name",
    )
    .fetch_all(&mut *c)
    .await?;
    let n = rows.len();
    let sample: Vec<Sample> = rows
        .into_iter()
        .map(|(catalog, schema, object, detail)| Sample {
            label: format!("{catalog}.{schema}.{object}"),
            detail,
            r#ref: Some(Ref::Object {
                catalog,
                schema,
                object,
            }),
        })
        .collect();
    let r#ref = sample.first().and_then(|s| s.r#ref.clone());
    Ok(finding(
        "lineage_failed",
        Severity::Warning,
        n,
        if n == 1 {
            "egret could not compute the lineage of 1 view, so the tables it reads are not known."
                .to_string()
        } else {
            format!(
                "egret could not compute the lineage of {n} views, so the tables they read are not \
                 known."
            )
        },
        sample,
        r#ref,
    ))
}

async fn catalog_apply_failed(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT c.id, c.name, e.name, a.apply_error \
           FROM registry.engine_catalog_attachments a \
           JOIN registry.catalogs c ON c.id = a.catalog_id \
           JOIN engine_mgmt.engines e ON e.id = a.engine_id \
          WHERE a.apply_status = 'failed' ORDER BY c.name, e.name",
    )
    .fetch_all(&mut *c)
    .await?;
    let n = rows.len();
    let sample = rows
        .into_iter()
        .map(|(id, name, engine, error)| Sample {
            label: format!("{name} on {engine}"),
            detail: error,
            r#ref: Some(Ref::Catalog { id, name }),
        })
        .collect();
    Ok(finding(
        "catalog_apply_failed",
        Severity::Warning,
        n,
        if n == 1 {
            "1 catalog attachment could not be applied to its engine.".to_string()
        } else {
            format!("{n} catalog attachments could not be applied to their engines.")
        },
        sample,
        Some(Ref::Catalogs),
    ))
}

async fn policy_warnings(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let profiles: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM policy.authz_services ORDER BY name")
            .fetch_all(&mut *c)
            .await?;
    let mut warned: Vec<(usize, String)> = Vec::new();
    for (id, name) in profiles {
        let w = egret_perm_compile::classify::profile_warnings(c, id).await?;
        if w.warning_count > 0 {
            warned.push((w.warning_count, name));
        }
    }
    let total: usize = warned.iter().map(|(n, _)| n).sum();
    // The profiles with the most, then by name.
    warned.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let sample = warned
        .into_iter()
        .map(|(n, name)| Sample {
            label: name.clone(),
            detail: Some(plural(n, "warning", "warnings")),
            r#ref: Some(Ref::Profile { name }),
        })
        .collect();
    Ok(finding(
        "policy_warnings",
        Severity::Warning,
        total,
        format!(
            "Hand-written policies carry {}.",
            plural(total, "warning", "warnings")
        ),
        sample,
        Some(Ref::Profiles),
    ))
}

async fn plugin_unmodelled(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<(
        Uuid,
        String,
        String,
        String,
        Option<String>,
        serde_json::Value,
    )> = sqlx::query_as(
        "SELECT id, name, type, origin, ranger_plugin_version, spec \
               FROM engine_mgmt.engines WHERE type IN ('trino', 'starrocks') ORDER BY name",
    )
    .fetch_all(&mut *c)
    .await?;
    let sample: Vec<Sample> = rows
        .into_iter()
        .filter_map(|(id, name, ty, origin, declared, spec)| {
            let r = egret_perm::ranger_plugin_report(&ty, &origin, declared.as_deref(), &spec);
            (!r.modelled).then(|| Sample {
                label: name.clone(),
                detail: r.note,
                r#ref: Some(Ref::Engine { id, name }),
            })
        })
        .collect();
    let n = sample.len();
    Ok(finding(
        "plugin_unmodelled",
        Severity::Warning,
        n,
        format!(
            "{} a Ranger plugin egret's evaluator does not model; explain and migrations for \
             their profiles refuse.",
            if n == 1 {
                "1 engine runs".to_string()
            } else {
                format!("{n} engines run")
            }
        ),
        sample,
        Some(Ref::Engines),
    ))
}

async fn tags_without_clearance(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT t.id, t.name FROM tags t \
           LEFT JOIN classification_policies c ON c.tag_id = t.id \
          WHERE c.tag_id IS NULL \
             OR (cardinality(c.required_roles) = 0 AND cardinality(c.required_groups) = 0) \
          ORDER BY t.name, t.id",
    )
    .fetch_all(&mut *c)
    .await?;
    let n = rows.len();
    let sample = rows
        .into_iter()
        .map(|(id, name)| Sample {
            label: name.clone(),
            detail: None,
            r#ref: Some(Ref::Tag { id, name }),
        })
        .collect();
    Ok(finding(
        "tags_without_clearance",
        Severity::Info,
        n,
        if n == 1 {
            "1 classification has no clearance: it gates nothing.".to_string()
        } else {
            format!("{n} classifications have no clearance: they gate nothing.")
        },
        sample,
        Some(Ref::Tags),
    ))
}

/// One classified table (rows grouped by folded schema and name) of a
/// warehouse the embedded catalog serves: its catalog's name, the catalog's
/// spelling where rows disagree (the first row's), and its classifications.
pub(crate) struct ClassifiedTable {
    pub(crate) key: TableKey,
    pub(crate) catalog: Option<String>,
    pub(crate) tags: Vec<(Uuid, String)>,
}

/// Every classified table of an embedded-catalog warehouse, by label.
pub(crate) async fn classified_tables(
    c: &mut PgConnection,
) -> Result<Vec<ClassifiedTable>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String, Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT a.warehouse_id, a.schema_name, a.table_name, t.id, t.name, \
                (SELECT r.name FROM registry.catalogs r WHERE r.warehouse_id = a.warehouse_id \
                  ORDER BY r.name LIMIT 1) \
           FROM catalog_tag_assignments a \
           JOIN tags t ON t.id = a.tag_id \
           JOIN catalog.warehouses w ON w.id = a.warehouse_id \
          WHERE w.lakekeeper_warehouse_id IS NOT NULL \
          ORDER BY a.warehouse_id, lower(a.schema_name), lower(a.table_name), t.name, t.id",
    )
    .fetch_all(&mut *c)
    .await?;
    let mut out: Vec<ClassifiedTable> = Vec::new();
    for (warehouse_id, schema, table, tag_id, tag_name, catalog) in rows {
        let same = |t: &ClassifiedTable| {
            t.key.warehouse_id == warehouse_id
                && t.key.schema.to_lowercase() == schema.to_lowercase()
                && t.key.table.to_lowercase() == table.to_lowercase()
        };
        match out.iter_mut().find(|t| same(t)) {
            Some(t) => {
                if !t.tags.iter().any(|(id, _)| *id == tag_id) {
                    t.tags.push((tag_id, tag_name));
                }
            }
            None => out.push(ClassifiedTable {
                key: TableKey {
                    warehouse_id,
                    schema,
                    table,
                },
                catalog,
                tags: vec![(tag_id, tag_name)],
            }),
        }
    }
    out.sort_by_key(table_label);
    Ok(out)
}

/// `catalog.schema.table`, or `schema.table` with no registry catalog.
pub(crate) fn table_label(t: &ClassifiedTable) -> String {
    match &t.catalog {
        Some(catalog) => format!("{catalog}.{}.{}", t.key.schema, t.key.table),
        None => format!("{}.{}", t.key.schema, t.key.table),
    }
}

/// Rows on a name the catalog has no live table for (rename spec C14, C15):
/// `Missing` (renamed away, purged or pre-placed) is a warning, since the
/// renamed table may be unclassified under its new name; `Dropped` is info,
/// since an undrop needs those rows.
async fn classifications_on_tables(
    c: &mut PgConnection,
    state: TableState,
) -> Result<Option<Finding>, SourceError> {
    let tables = classified_tables(c).await?;
    if tables.is_empty() {
        return Ok(None);
    }
    let keys: Vec<TableKey> = tables.iter().map(|t| t.key.clone()).collect();
    let states = table_states_via(c, &keys).await?;
    let hits: Vec<&ClassifiedTable> = tables
        .iter()
        .zip(states)
        .filter(|(_, s)| *s == state)
        .map(|(t, _)| t)
        .collect();
    let n = hits.len();
    let sample = hits
        .iter()
        .map(|t| Sample {
            label: table_label(t),
            detail: Some(
                t.tags
                    .iter()
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            r#ref: t.tags.first().map(|(id, name)| Ref::Tag {
                id: *id,
                name: name.clone(),
            }),
        })
        .collect();
    let (kind, severity, sentence) = match state {
        TableState::Dropped => (
            "classifications_on_dropped_tables",
            Severity::Info,
            if n == 1 {
                "1 dropped table keeps its classifications, which apply again if it is restored."
                    .to_string()
            } else {
                format!(
                    "{n} dropped tables keep their classifications, which apply again if they \
                     are restored."
                )
            },
        ),
        _ => (
            "classifications_on_missing_tables",
            Severity::Warning,
            if n == 1 {
                "1 table carries classifications but does not exist in its catalog: renamed \
                 before egret moved classifications, purged, or classified before it was \
                 created."
                    .to_string()
            } else {
                format!(
                    "{n} tables carry classifications but do not exist in their catalog: \
                     renamed before egret moved classifications, purged, or classified before \
                     they were created."
                )
            },
        ),
    };
    Ok(finding(
        kind,
        severity,
        n,
        sentence,
        sample,
        Some(Ref::Tags),
    ))
}

async fn catalogs_without_engine(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT c.name FROM registry.catalogs c \
          WHERE NOT EXISTS (SELECT 1 FROM registry.engine_catalog_attachments a \
                             WHERE a.catalog_id = c.id) \
          ORDER BY c.name",
    )
    .fetch_all(&mut *c)
    .await?;
    let n = rows.len();
    let sample = rows
        .into_iter()
        .map(|name| Sample {
            label: name.clone(),
            detail: None,
            r#ref: Some(Ref::DataCatalog { name }),
        })
        .collect();
    Ok(finding(
        "catalogs_without_engine",
        Severity::Info,
        n,
        format!(
            "{} attached to no engine, so no engine serves {}.",
            if n == 1 {
                "1 catalog is".to_string()
            } else {
                format!("{n} catalogs are")
            },
            if n == 1 { "it" } else { "them" }
        ),
        sample,
        Some(Ref::Data),
    ))
}

#[cfg(test)]
mod tests {
    use super::reuse_sentence;

    #[test]
    fn the_reuse_sentence_claims_only_what_each_verdict_shows() {
        assert_eq!(
            reuse_sentence(2, 1, 0),
            "2 references or policies were written for a username before the person who holds \
             it now existed. 1 more may have been."
        );
        assert_eq!(
            reuse_sentence(0, 1, 0),
            "1 reference or policy may have been written for a username before the person who \
             holds it now existed."
        );
        assert_eq!(
            reuse_sentence(0, 0, 1),
            "1 egret-admin policy still gives engine ALL access to someone who is no longer an \
             active platform admin."
        );
    }
}
