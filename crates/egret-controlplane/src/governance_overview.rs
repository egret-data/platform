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
use egret_perm_compile::table_columns::{
    missing_columns_via, table_states_ns_via, table_states_via, NsTableKey, TableKey, TableState,
};
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
const PLATFORM_ADMIN: &str = egret_core::context::PLATFORM_ADMIN_ROLE;

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
        kind: "classifications_on_missing_columns",
        what: "classified columns",
        requires: Requires::All(&[TAG_READ]),
        run: |c| Box::pin(classifications_on_missing_columns(c)),
    },
    Source {
        kind: "permissions_on_missing_tables",
        what: "tables named by grants, rules and hand-written policies",
        requires: POLICY,
        run: |c| Box::pin(permissions_on_missing_tables(c)),
    },
    Source {
        kind: "permissions_on_missing_columns",
        what: "columns named by grants and rules",
        requires: POLICY,
        run: |c| Box::pin(permissions_on_missing_columns(c)),
    },
    Source {
        kind: "catalog_grants_on_missing_tables",
        what: "tables named by catalog grants",
        requires: Requires::All(&[CATALOG_READ]),
        run: |c| Box::pin(catalog_grants_on_missing_tables(c)),
    },
    Source {
        kind: "renamed_names_lost_grants",
        what: "renamed tables' allow grants",
        requires: POLICY,
        run: |c| Box::pin(renamed_names_lost_grants(c)),
    },
    Source {
        kind: "renamed_names_lost_catalog_grants",
        what: "renamed tables' catalog grants",
        requires: Requires::All(&[CATALOG_READ]),
        run: |c| Box::pin(renamed_names_lost_catalog_grants(c)),
    },
    Source {
        kind: "renamed_names_reused",
        what: "renamed classified tables",
        requires: Requires::All(&[TAG_READ]),
        run: |c| Box::pin(renamed_names_reused(c)),
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
    if p.is_platform_admin() {
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
/// warehouse the embedded catalog serves: its catalog's name, the first row's
/// spelling, its classifications at either level, and its classified columns
/// (grouped folded) with theirs.
pub(crate) struct ClassifiedTable {
    pub(crate) key: TableKey,
    pub(crate) catalog: Option<String>,
    pub(crate) tags: Vec<(Uuid, String)>,
    pub(crate) columns: Vec<(String, Vec<(Uuid, String)>)>,
}

/// Every classified table of an embedded-catalog warehouse, by label.
pub(crate) async fn classified_tables(
    c: &mut PgConnection,
) -> Result<Vec<ClassifiedTable>, sqlx::Error> {
    type Row = (Uuid, String, String, String, Uuid, String, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT a.warehouse_id, a.schema_name, a.table_name, a.column_name, t.id, t.name, \
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
    for (warehouse_id, schema, table, column, tag_id, tag_name, catalog) in rows {
        let same = |t: &ClassifiedTable| {
            t.key.warehouse_id == warehouse_id
                && t.key.schema.to_lowercase() == schema.to_lowercase()
                && t.key.table.to_lowercase() == table.to_lowercase()
        };
        let i = match out.iter().position(same) {
            Some(i) => i,
            None => {
                out.push(ClassifiedTable {
                    key: TableKey {
                        warehouse_id,
                        schema,
                        table,
                    },
                    catalog,
                    tags: Vec::new(),
                    columns: Vec::new(),
                });
                out.len() - 1
            }
        };
        let t = &mut out[i];
        if !t.tags.iter().any(|(id, _)| *id == tag_id) {
            t.tags.push((tag_id, tag_name.clone()));
        }
        if column.is_empty() {
            continue;
        }
        let c = match t
            .columns
            .iter()
            .position(|(c, _)| c.to_lowercase() == column.to_lowercase())
        {
            Some(c) => c,
            None => {
                t.columns.push((column, Vec::new()));
                t.columns.len() - 1
            }
        };
        if !t.columns[c].1.iter().any(|(id, _)| *id == tag_id) {
            t.columns[c].1.push((tag_id, tag_name));
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

/// The classified columns of live tables that their current schema lacks
/// (rename spec R8), each with its rename candidate. A column of a dropped or
/// missing table is not counted here: its table is.
async fn classifications_on_missing_columns(
    c: &mut PgConnection,
) -> Result<Option<Finding>, SourceError> {
    let tables: Vec<ClassifiedTable> = classified_tables(c)
        .await?
        .into_iter()
        .filter(|t| !t.columns.is_empty())
        .collect();
    if tables.is_empty() {
        return Ok(None);
    }
    let keys: Vec<TableKey> = tables.iter().map(|t| t.key.clone()).collect();
    let states = table_states_via(c, &keys).await?;
    let live: Vec<&ClassifiedTable> = tables
        .iter()
        .zip(states)
        .filter(|(_, s)| *s == TableState::Live)
        .map(|(t, _)| t)
        .collect();
    let asked: Vec<(TableKey, Vec<String>)> = live
        .iter()
        .map(|t| {
            (
                t.key.clone(),
                t.columns.iter().map(|(c, _)| c.clone()).collect(),
            )
        })
        .collect();
    let missing = missing_columns_via(c, &asked).await?;
    let n = missing.len();
    let renamed = missing.iter().filter(|m| m.renamed_to.is_some()).count();
    let mut sample = Vec::new();
    for m in &missing {
        let Some(t) = live.iter().find(|t| t.key == m.key) else {
            continue;
        };
        let tag = t
            .columns
            .iter()
            .find(|(c, _)| *c == m.column)
            .and_then(|(_, tags)| tags.first());
        sample.push(Sample {
            label: format!("{}.{}", table_label(t), m.column),
            detail: Some(match &m.renamed_to {
                Some(to) => format!("renamed to {to}"),
                None => "not in the table".to_string(),
            }),
            r#ref: tag.map(|(id, name)| Ref::Tag {
                id: *id,
                name: name.clone(),
            }),
        });
    }
    sample.sort_by(|a, b| a.label.cmp(&b.label));
    let was = |k: usize| if k == 1 { "was" } else { "were" };
    let head = if n == 1 {
        "1 classified column is not in its table".to_string()
    } else {
        format!("{n} classified columns are not in their table")
    };
    let dropped = n - renamed;
    let sentence = match (renamed, dropped) {
        (0, _) => format!(
            "{head}: {} dropped or never existed.",
            if n == 1 { "it was" } else { "they were" }
        ),
        (_, 0) => format!(
            "{head}: {} renamed (egret names the new column).",
            if n == 1 { "it was" } else { "they were" }
        ),
        (r, d) => format!(
            "{head}: {r} {} renamed (egret names the new column), {d} {} dropped or never \
             existed.",
            was(r),
            was(d)
        ),
    };
    Ok(finding(
        "classifications_on_missing_columns",
        Severity::Warning,
        n,
        sentence,
        sample,
        Some(Ref::Tags),
    ))
}

/// Rename spec R10: a table renamed away took its classifications to its new
/// name (R1), and a new table now holds the old name without them (the swap
/// pattern `t -> t_old`, `t_new -> t`). Derived from each name's latest
/// `tag:move` of a table rename and the current catalog: the old name is a
/// live table that carries none of the moved classifications. Each sample
/// links to the renamed table, whose classifications a steward can re-assign.
async fn renamed_names_reused(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    use serde_json::Value;
    let events: Vec<(String, Value)> = sqlx::query_as(
        "SELECT DISTINCT ON (resource_id) resource_id, detail FROM audit.audit_log \
          WHERE kind = 'mutation' AND action = 'tag:move' AND resource_type = 'catalog' \
            AND detail->>'trigger' = 'iceberg_rename' \
          ORDER BY resource_id, ts DESC, id DESC",
    )
    .fetch_all(&mut *c)
    .await?;
    struct Moved {
        key: TableKey,
        to: (String, String),
        tags: Vec<(Uuid, String)>,
    }
    let mut moved = Vec::new();
    for (resource_id, d) in events {
        let Some(warehouse_id) = resource_id
            .split_once(':')
            .and_then(|(w, _)| w.parse::<Uuid>().ok())
        else {
            continue;
        };
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(schema), Some(table)) =
            (text(&d["before"], "schema"), text(&d["before"], "table"))
        else {
            continue;
        };
        let (Some(to_schema), Some(to_table)) =
            (text(&d["after"], "schema"), text(&d["after"], "table"))
        else {
            continue;
        };
        let tags: Vec<(Uuid, String)> = d["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                Some((
                    t.get("id")?.as_str()?.parse().ok()?,
                    text(t, "name").unwrap_or_default(),
                ))
            })
            .collect();
        moved.push(Moved {
            key: TableKey {
                warehouse_id,
                schema,
                table,
            },
            to: (to_schema, to_table),
            tags,
        });
    }
    if moved.is_empty() {
        return Ok(None);
    }
    let keys: Vec<TableKey> = moved.iter().map(|m| m.key.clone()).collect();
    let states = table_states_via(c, &keys).await?;
    let mut sample = Vec::new();
    for (m, state) in moved.iter().zip(states) {
        if state != TableState::Live {
            continue;
        }
        let ids: Vec<Uuid> = m.tags.iter().map(|(id, _)| *id).collect();
        let still: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM catalog_tag_assignments \
                             WHERE warehouse_id = $1 AND lower(schema_name) = lower($2) \
                               AND lower(table_name) = lower($3) AND tag_id = ANY($4))",
        )
        .bind(m.key.warehouse_id)
        .bind(&m.key.schema)
        .bind(&m.key.table)
        .bind(&ids)
        .fetch_one(&mut *c)
        .await?;
        if still {
            continue;
        }
        let catalog: Option<String> = sqlx::query_scalar(
            "SELECT name FROM registry.catalogs WHERE warehouse_id = $1 ORDER BY name LIMIT 1",
        )
        .bind(m.key.warehouse_id)
        .fetch_optional(&mut *c)
        .await?;
        let names: Vec<&str> = m.tags.iter().map(|(_, n)| n.as_str()).collect();
        let at = |schema: &str, table: &str| match &catalog {
            Some(cat) => format!("{cat}.{schema}.{table}"),
            None => format!("{schema}.{table}"),
        };
        sample.push(Sample {
            label: at(&m.key.schema, &m.key.table),
            detail: Some(format!(
                "{} moved to {} by a rename",
                names.join(", "),
                at(&m.to.0, &m.to.1)
            )),
            r#ref: catalog.map(|catalog| Ref::Object {
                catalog,
                schema: m.to.0.clone(),
                object: m.to.1.clone(),
            }),
        });
    }
    sample.sort_by(|a, b| a.label.cmp(&b.label));
    let n = sample.len();
    let sentence = if n == 1 {
        "1 table name lost its classifications when its table was renamed, and a new table now \
         holds the name without them: check whether it needs the classifications of the renamed \
         table."
            .to_string()
    } else {
        format!(
            "{n} table names lost their classifications when their tables were renamed, and new \
             tables now hold the names without them: check whether they need the \
             classifications of the renamed tables."
        )
    };
    Ok(finding(
        "renamed_names_reused",
        Severity::Warning,
        n,
        sentence,
        sample,
        Some(Ref::Tags),
    ))
}

// ── Grants follow a rename (spec 2026-10-27 §5.8, D16): stale references ──

/// One literal grant, path rule or hand-written policy that names a table.
struct NamedTable {
    key: TableKey,
    catalog: String,
    column: Option<String>,
    /// `deny grant`, `mask rule pii-email`, `hand-written policy p/n`.
    what: String,
    r#ref: Ref,
}

/// No `*` or `?`.
fn literal(level: &str) -> bool {
    !level.contains(['*', '?'])
}

/// Every explicit grant and path rule whose schema and table levels are
/// literal, on a catalog over an embedded warehouse.
async fn literal_perm_rows(c: &mut PgConnection) -> Result<Vec<NamedTable>, sqlx::Error> {
    type Row = (
        String,
        Uuid,
        String,
        Uuid,
        String,
        String,
        Option<String>,
        String,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT 'grant', g.id, g.effect::text, c.warehouse_id, g.schema_name, g.table_name, \
                g.column_name, c.name \
           FROM perm.grants g JOIN registry.catalogs c ON c.id = g.catalog_id \
          WHERE g.origin = 'explicit' AND c.warehouse_id IS NOT NULL \
            AND g.schema_name IS NOT NULL AND g.table_name IS NOT NULL \
         UNION ALL \
         SELECT r.kind::text, r.id, r.name, c.warehouse_id, r.schema_name, r.table_name, \
                r.column_name, c.name \
           FROM perm.rules r JOIN registry.catalogs c ON c.id = r.catalog_id \
          WHERE r.selector_kind = 'path' AND c.warehouse_id IS NOT NULL \
            AND r.schema_name IS NOT NULL AND r.table_name IS NOT NULL",
    )
    .fetch_all(&mut *c)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| literal(&r.4) && literal(&r.5))
        .map(
            |(kind, id, name, warehouse_id, schema, table, column, catalog)| NamedTable {
                key: TableKey {
                    warehouse_id,
                    schema,
                    table,
                },
                catalog,
                column,
                what: match kind.as_str() {
                    "grant" => format!("{name} grant"),
                    "mask" => format!("mask rule {name}"),
                    _ => format!("row-filter rule {name}"),
                },
                r#ref: if kind == "grant" {
                    Ref::Grant { id }
                } else {
                    Ref::Rule { id }
                },
            },
        )
        .collect())
}

/// The literal tables enabled operator policies name, on the profiles bound
/// to a catalog over an embedded warehouse (D5's reading: the catalog level
/// names the registry catalog).
async fn handwritten_tables(c: &mut PgConnection) -> Result<Vec<NamedTable>, sqlx::Error> {
    use serde_json::Value;
    let rows: Vec<(String, String, Value, String, Uuid)> = sqlx::query_as(
        "SELECT DISTINCT s.name, p.name, p.policy_jsonb, c.name, c.warehouse_id \
           FROM policy.authz_policies p \
           JOIN policy.authz_services s ON s.id = p.service_id \
           JOIN engine_mgmt.engines e ON e.authz_service_id = s.id \
           JOIN registry.engine_catalog_attachments a ON a.engine_id = e.id \
           JOIN registry.catalogs c ON c.id = a.catalog_id \
          WHERE p.enabled AND p.managed_by = 'operator' AND c.warehouse_id IS NOT NULL",
    )
    .fetch_all(&mut *c)
    .await?;
    let values = |body: &Value, level: &str| -> Vec<String> {
        body.get("resources")
            .and_then(|r| r.get(level))
            .and_then(|l| l.get("values"))
            .and_then(Value::as_array)
            .map(|vs| {
                vs.iter()
                    .filter_map(Value::as_str)
                    .filter(|v| literal(v))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for (profile, name, body, catalog, warehouse_id) in rows {
        if !values(&body, "catalog")
            .iter()
            .any(|v| v.to_lowercase() == catalog.to_lowercase())
        {
            continue;
        }
        let mut schemas = values(&body, "schema");
        schemas.extend(values(&body, "database"));
        for schema in &schemas {
            for table in values(&body, "table") {
                out.push(NamedTable {
                    key: TableKey {
                        warehouse_id,
                        schema: schema.clone(),
                        table,
                    },
                    catalog: catalog.clone(),
                    column: None,
                    what: format!("hand-written policy {profile}/{name}"),
                    r#ref: Ref::Profile {
                        name: profile.clone(),
                    },
                });
            }
        }
    }
    Ok(out)
}

/// `catalog.schema.table` (folded) → `schema.table` it was renamed to, from
/// the restrictive copies the table hook recorded (`perm_grant:copy`,
/// `perm_rule:copy`), newest first.
async fn renamed_to(
    c: &mut PgConnection,
) -> Result<std::collections::HashMap<(String, String, String), String>, sqlx::Error> {
    use serde_json::Value;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT detail FROM audit.audit_log \
          WHERE kind = 'mutation' AND action IN ('perm_grant:copy', 'perm_rule:copy') \
            AND detail ->> 'trigger' = 'iceberg_rename' \
          ORDER BY ts, id",
    )
    .fetch_all(&mut *c)
    .await?;
    let mut out = std::collections::HashMap::new();
    for d in rows {
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let (before, after) = (&d["before"], &d["after"]);
        let from = if before.get("selector").is_some() {
            let parts: Vec<String> = text(before, "selector")
                .split('.')
                .map(str::to_string)
                .collect();
            match parts.as_slice() {
                [cat, schema, table, ..] => (cat.clone(), schema.clone(), table.clone()),
                _ => continue,
            }
        } else {
            (
                text(before, "catalog"),
                text(before, "schema"),
                text(before, "table"),
            )
        };
        let to = d
            .get("rename")
            .map(|r| text(r, "to"))
            .unwrap_or_else(|| format!("{}.{}", text(after, "schema"), text(after, "table")));
        out.insert(
            (
                from.0.to_lowercase(),
                from.1.to_lowercase(),
                from.2.to_lowercase(),
            ),
            to,
        );
    }
    Ok(out)
}

/// D16: grants, path rules and hand-written policies on table names the
/// catalog has no table for (missing, not dropped: an undrop restores a
/// dropped one). One sample per name, naming each kind and, when egret kept
/// a restriction there on purpose, the table it was renamed to.
async fn permissions_on_missing_tables(
    c: &mut PgConnection,
) -> Result<Option<Finding>, SourceError> {
    let mut rows = literal_perm_rows(c).await?;
    rows.extend(handwritten_tables(c).await?);
    if rows.is_empty() {
        return Ok(None);
    }
    let keys: Vec<TableKey> = rows.iter().map(|r| r.key.clone()).collect();
    let states = table_states_via(c, &keys).await?;
    let renamed = renamed_to(c).await?;
    let mut by_name: BTreeMap<(String, String, String), Vec<&NamedTable>> = BTreeMap::new();
    for (r, state) in rows.iter().zip(states) {
        if state != TableState::Missing {
            continue;
        }
        by_name
            .entry((
                r.catalog.to_lowercase(),
                r.key.schema.to_lowercase(),
                r.key.table.to_lowercase(),
            ))
            .or_default()
            .push(r);
    }
    let n = by_name.len();
    let mut sample = Vec::new();
    for (name, named) in &by_name {
        let first = named[0];
        let mut what: Vec<&str> = named.iter().map(|r| r.what.as_str()).collect();
        what.dedup();
        let mut detail = what.join(", ");
        if let Some(to) = renamed.get(name) {
            detail.push_str(&format!("; kept when the table was renamed to {to}"));
        }
        sample.push(Sample {
            label: format!("{}.{}.{}", first.catalog, first.key.schema, first.key.table),
            detail: Some(detail),
            r#ref: Some(first.r#ref.clone()),
        });
    }
    let sentence = if n == 1 {
        "1 table name carries grants, rules or hand-written policies but has no table: left by a \
         rename (egret keeps restrictions on the old name on purpose), a purge, or written before \
         the table existed. A table created under that name is governed by them."
            .to_string()
    } else {
        format!(
            "{n} table names carry grants, rules or hand-written policies but have no table: left \
             by a rename (egret keeps restrictions on the old name on purpose), a purge, or \
             written before the table existed. A table created under such a name is governed by \
             them."
        )
    };
    Ok(finding(
        "permissions_on_missing_tables",
        Severity::Warning,
        n,
        sentence,
        sample,
        Some(Ref::Data),
    ))
}

/// D16: literal column-level grants and path rules of live tables whose
/// column is not in the table, each with its rename candidate.
async fn permissions_on_missing_columns(
    c: &mut PgConnection,
) -> Result<Option<Finding>, SourceError> {
    let rows: Vec<NamedTable> = literal_perm_rows(c)
        .await?
        .into_iter()
        .filter(|r| {
            r.column
                .as_deref()
                .is_some_and(|col| col != "*" && literal(col))
        })
        .collect();
    if rows.is_empty() {
        return Ok(None);
    }
    let keys: Vec<TableKey> = rows.iter().map(|r| r.key.clone()).collect();
    let states = table_states_via(c, &keys).await?;
    let live: Vec<&NamedTable> = rows
        .iter()
        .zip(states)
        .filter(|(_, s)| *s == TableState::Live)
        .map(|(r, _)| r)
        .collect();
    let asked: Vec<(TableKey, Vec<String>)> = live
        .iter()
        .map(|r| (r.key.clone(), vec![r.column.clone().unwrap_or_default()]))
        .collect();
    let missing = missing_columns_via(c, &asked).await?;
    let mut sample = Vec::new();
    let mut renamed = 0;
    for r in &live {
        let column = r.column.clone().unwrap_or_default();
        let Some(m) = missing
            .iter()
            .find(|m| m.key == r.key && m.column == column)
        else {
            continue;
        };
        if m.renamed_to.is_some() {
            renamed += 1;
        }
        sample.push(Sample {
            label: format!("{}.{}.{}.{column}", r.catalog, r.key.schema, r.key.table),
            detail: Some(match &m.renamed_to {
                Some(to) => format!("renamed to {to}"),
                None => "not in the table".to_string(),
            }),
            r#ref: Some(r.r#ref.clone()),
        });
    }
    sample.sort_by(|a, b| a.label.cmp(&b.label));
    let n = sample.len();
    let dropped = n - renamed;
    let was = |k: usize| if k == 1 { "was" } else { "were" };
    let head = if n == 1 {
        "1 grant or rule names a column that is not in its table".to_string()
    } else {
        format!("{n} grants or rules name columns that are not in their table")
    };
    let sentence = match (renamed, dropped) {
        (0, _) => format!(
            "{head}: {} dropped or never existed.",
            if n == 1 { "it was" } else { "they were" }
        ),
        (_, 0) => format!(
            "{head}: {} renamed (egret names the new column).",
            if n == 1 { "it was" } else { "they were" }
        ),
        (r, d) => format!(
            "{head}: {r} {} renamed (egret names the new column), {d} {} dropped or never \
             existed.",
            was(r),
            was(d)
        ),
    };
    Ok(finding(
        "permissions_on_missing_columns",
        Severity::Warning,
        n,
        sentence,
        sample,
        Some(Ref::Data),
    ))
}

/// D16: `table` catalog grants whose table does not exist (missing, at any
/// namespace depth).
async fn catalog_grants_on_missing_tables(
    c: &mut PgConnection,
) -> Result<Option<Finding>, SourceError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        role: String,
        privilege: String,
        namespace: Vec<String>,
        table_name: String,
        warehouse_id: Uuid,
        warehouse: String,
        catalog_id: Option<Uuid>,
        catalog: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT r.name AS role, g.privilege, g.namespace, g.table_name, w.id AS warehouse_id, \
                w.name AS warehouse, \
                (SELECT rc.id FROM registry.catalogs rc WHERE rc.warehouse_id = w.id \
                  ORDER BY rc.name LIMIT 1) AS catalog_id, \
                (SELECT rc.name FROM registry.catalogs rc WHERE rc.warehouse_id = w.id \
                  ORDER BY rc.name LIMIT 1) AS catalog \
           FROM catalog.catalog_grants g \
           JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
           JOIN catalog.warehouses w ON w.id = r.warehouse_id \
          WHERE g.securable_type = 'table' AND g.namespace IS NOT NULL \
            AND g.table_name IS NOT NULL AND w.lakekeeper_warehouse_id IS NOT NULL \
          ORDER BY w.name, g.namespace, g.table_name, r.name",
    )
    .fetch_all(&mut *c)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let keys: Vec<NsTableKey> = rows
        .iter()
        .map(|r| NsTableKey {
            warehouse_id: r.warehouse_id,
            namespace: r.namespace.clone(),
            table: r.table_name.clone(),
        })
        .collect();
    let states = table_states_ns_via(c, &keys).await?;
    let sample: Vec<Sample> = rows
        .iter()
        .zip(states)
        .filter(|(_, s)| *s == TableState::Missing)
        .map(|(r, _)| Sample {
            label: format!("{}.{}.{}", r.warehouse, r.namespace.join("."), r.table_name),
            detail: Some(format!("role {}, {}", r.role, r.privilege)),
            r#ref: r
                .catalog_id
                .zip(r.catalog.clone())
                .map(|(id, name)| Ref::Catalog { id, name }),
        })
        .collect();
    let n = sample.len();
    let sentence = if n == 1 {
        "1 catalog grant names a table that does not exist: it covers nothing until a table takes \
         the name."
            .to_string()
    } else {
        format!(
            "{n} catalog grants name tables that do not exist: they cover nothing until a table \
             takes the name."
        )
    };
    Ok(finding(
        "catalog_grants_on_missing_tables",
        Severity::Warning,
        n,
        sentence,
        sample,
        Some(Ref::Catalogs),
    ))
}

/// R10's sentence for what a rename took away from a name (D17).
fn lost_sentence(n: usize, what: &str) -> String {
    if n == 1 {
        format!(
            "1 table name lost its {what} when its table was renamed, and a new table now holds \
             the name without them: check whether it needs the {what} of the renamed table."
        )
    } else {
        format!(
            "{n} table names lost their {what} when their tables were renamed, and new tables \
             now hold the names without them: check whether they need the {what} of the renamed \
             tables."
        )
    }
}

/// D17: a table renamed away took its allow grants to its new name (G2), and
/// a new table now holds the old name with none of them (the swap pattern).
/// From each `perm_grant:move` of a table rename, every move counts: a grant
/// moved on again leaves its earlier old name behind too. The old name is a
/// live table again and carries no allow with a moved grant's actions and
/// subjects; a name that still carries only some of them is reported.
async fn renamed_names_lost_grants(c: &mut PgConnection) -> Result<Option<Finding>, SourceError> {
    use serde_json::Value;
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT detail FROM audit.audit_log \
          WHERE kind = 'mutation' AND action = 'perm_grant:move' \
            AND resource_type = 'perm_grant' AND detail->>'trigger' = 'iceberg_rename' \
          ORDER BY ts DESC, id DESC",
    )
    .fetch_all(&mut *c)
    .await?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let list = |v: &Value, k: &str| -> Vec<String> {
        let mut out: Vec<String> = v
            .get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        out.sort();
        out
    };
    // Old name (folded catalog, schema, table) → (spelling, renamed to, moved
    // allows by grant id). Events come newest first, so the first `to` wins.
    type Moved = (Vec<String>, Vec<String>);
    type Name = (String, String, String);
    type Entry = (Name, String, BTreeMap<String, Moved>);
    let mut names: BTreeMap<Name, Entry> = BTreeMap::new();
    for d in &events {
        let before = &d["before"];
        let (catalog, schema, table) = (
            text(before, "catalog"),
            text(before, "schema"),
            text(before, "table"),
        );
        if catalog.is_empty() || schema.is_empty() || table.is_empty() {
            continue;
        }
        let to = format!(
            "{}.{}",
            text(&d["after"], "schema"),
            text(&d["after"], "table")
        );
        let entry = names
            .entry((
                catalog.to_lowercase(),
                schema.to_lowercase(),
                table.to_lowercase(),
            ))
            .or_insert_with(|| {
                (
                    (catalog.clone(), schema.clone(), table.clone()),
                    to,
                    BTreeMap::new(),
                )
            });
        entry
            .2
            .entry(text(before, "id"))
            .or_insert_with(|| (list(before, "actions"), list(before, "include")));
    }
    if names.is_empty() {
        return Ok(None);
    }
    let mut sample = Vec::new();
    for ((catalog, schema, table), to, moved) in names.values() {
        let warehouse: Option<Uuid> =
            sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE name = $1")
                .bind(catalog)
                .fetch_optional(&mut *c)
                .await?
                .flatten();
        let Some(warehouse_id) = warehouse else {
            continue;
        };
        let key = TableKey {
            warehouse_id,
            schema: schema.clone(),
            table: table.clone(),
        };
        if table_states_via(c, std::slice::from_ref(&key)).await? != [TableState::Live] {
            continue;
        }
        // The allows on the old name now: (actions, subjects), each sorted.
        let here: Vec<(Vec<String>, Vec<String>)> = sqlx::query_as(
            "SELECT (SELECT array_agg(a ORDER BY a) FROM unnest(g.actions) a), \
                    COALESCE((SELECT array_agg(p.kind::text || ':' || p.ref ORDER BY 1) \
                                FROM perm.grant_principals p \
                               WHERE p.grant_id = g.id AND p.mode = 'include'), '{}') \
               FROM perm.grants g JOIN registry.catalogs rc ON rc.id = g.catalog_id \
              WHERE rc.name = $1 AND g.effect = 'allow' \
                AND lower(g.schema_name) = lower($2) AND lower(g.table_name) = lower($3)",
        )
        .bind(catalog)
        .bind(schema)
        .bind(table)
        .fetch_all(&mut *c)
        .await?;
        if moved.values().all(|m| here.contains(m)) {
            continue;
        }
        let (to_schema, to_table) = to.split_once('.').unwrap_or((to.as_str(), ""));
        sample.push(Sample {
            label: format!("{catalog}.{schema}.{table}"),
            detail: Some(format!(
                "{} moved to {catalog}.{to} by a rename",
                plural(moved.len(), "allow grant", "allow grants")
            )),
            r#ref: Some(Ref::Object {
                catalog: catalog.clone(),
                schema: to_schema.to_string(),
                object: to_table.to_string(),
            }),
        });
    }
    let n = sample.len();
    Ok(finding(
        "renamed_names_lost_grants",
        Severity::Warning,
        n,
        lost_sentence(n, "allow grants"),
        sample,
        Some(Ref::Data),
    ))
}

/// D17 for catalog grants: from each `catalog_grant:move`, newest first, the
/// old name is a live table again and the role holds no grant of that
/// privilege on it.
async fn renamed_names_lost_catalog_grants(
    c: &mut PgConnection,
) -> Result<Option<Finding>, SourceError> {
    use serde_json::Value;
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT detail FROM audit.audit_log \
          WHERE kind = 'mutation' AND action = 'catalog_grant:move' \
            AND resource_type = 'catalog_grant' AND detail->>'trigger' = 'iceberg_rename' \
          ORDER BY ts DESC, id DESC",
    )
    .fetch_all(&mut *c)
    .await?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let ns_of = |v: &Value| -> Vec<String> {
        v.get("namespace")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let mut sample = Vec::new();
    let mut seen = HashSet::new();
    for d in &events {
        let Ok(warehouse_id) = text(d, "warehouse_id").parse::<Uuid>() else {
            continue;
        };
        let (ns, table) = (ns_of(&d["before"]), text(&d["before"], "table"));
        let (role, privilege) = (text(d, "role"), text(d, "privilege"));
        if ns.is_empty() || table.is_empty() {
            continue;
        }
        let key = NsTableKey {
            warehouse_id,
            namespace: ns.clone(),
            table: table.clone(),
        };
        if table_states_ns_via(c, std::slice::from_ref(&key)).await? != [TableState::Live] {
            continue;
        }
        let still: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM catalog.catalog_grants g \
               JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
              WHERE r.warehouse_id = $1 AND r.name = $2 AND g.privilege = $3 \
                AND g.securable_type = 'table' \
                AND lower(array_to_string(g.namespace, chr(31))) = lower($4) \
                AND lower(g.table_name) = lower($5))",
        )
        .bind(warehouse_id)
        .bind(&role)
        .bind(&privilege)
        .bind(ns.join("\u{1f}"))
        .bind(&table)
        .fetch_one(&mut *c)
        .await?;
        if still {
            continue;
        }
        let (warehouse, catalog): (String, Option<(Uuid, String)>) = {
            let w: String = sqlx::query_scalar("SELECT name FROM catalog.warehouses WHERE id = $1")
                .bind(warehouse_id)
                .fetch_optional(&mut *c)
                .await?
                .unwrap_or_else(|| warehouse_id.to_string());
            let cat: Option<(Uuid, String)> = sqlx::query_as(
                "SELECT id, name FROM registry.catalogs WHERE warehouse_id = $1 ORDER BY name \
                  LIMIT 1",
            )
            .bind(warehouse_id)
            .fetch_optional(&mut *c)
            .await?;
            (w, cat)
        };
        let label = format!("{warehouse}.{}.{table}", ns.join("."));
        if !seen.insert(label.clone()) {
            continue;
        }
        let after = &d["after"];
        sample.push(Sample {
            label,
            detail: Some(format!(
                "role {role}, {privilege} moved to {}.{} by a rename",
                ns_of(after).join("."),
                text(after, "table")
            )),
            r#ref: catalog.map(|(id, name)| Ref::Catalog { id, name }),
        });
    }
    sample.sort_by(|a, b| a.label.cmp(&b.label));
    let n = sample.len();
    Ok(finding(
        "renamed_names_lost_catalog_grants",
        Severity::Warning,
        n,
        lost_sentence(n, "catalog grants"),
        sample,
        Some(Ref::Catalogs),
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
