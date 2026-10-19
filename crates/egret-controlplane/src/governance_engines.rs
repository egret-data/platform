//! DG #4a (spec `docs/en/specs/2026-10-19-dg-engine-policy-design.md` §4.2):
//! `GET /governance/engines/{id}/plugin`, what the Ranger plugins of an
//! engine's profile report downloading, judged by the same function as the
//! Governance Overview's drift findings ([`crate::plugin_judgement`]), so the
//! engine page's Plugin tab and the Overview never disagree.
//!
//! The ledger records what plugins REPORT holding (`plugin_downloads.rs`), so
//! no sentence here says a plugin "enforces" anything. It is keyed per
//! profile, not per engine: on a legacy shared profile the rows may come from
//! any engine bound to it.
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::governance_overview::begin_snapshot;
use crate::plugin_judgement::{
    judge_plugins, ledger_rows_via, Judgement, KindState, LedgerRow, Liveness, NotReported,
    ProfileVersions,
};
use crate::{CpError, CpState};

pub(crate) fn router() -> Router<CpState> {
    Router::new().route("/governance/engines/{id}/plugin", get(engine_plugin))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(engine_plugin),
    components(schemas(
        EnginePluginResponse,
        PluginEngine,
        PluginEngineRef,
        PluginProfile,
        PluginReportDto,
        Judged,
        NotJudgedReason,
        KindReport,
        PluginKind,
        KindStateDto,
        NotReportedDto,
        LedgerRowDto
    ))
)]
pub(crate) struct ControlPlaneGovernanceEnginesApi;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnginePluginResponse {
    pub generated_at: DateTime<Utc>,
    pub engine: PluginEngine,
    /// The engine's policy profile; `null` when it has none.
    pub profile: Option<PluginProfile>,
    /// The other engines bound to the same profile (legacy shared), by name.
    pub shared_with: Vec<PluginEngineRef>,
    pub plugin: PluginReportDto,
    pub judged: Judged,
    /// One entry per kind (`policies`, then `tags`); empty when not judged.
    pub kinds: Vec<KindReport>,
    /// The profile's raw ledger rows, newest first; empty without a profile.
    pub rows: Vec<LedgerRowDto>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginEngine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    /// `provisioned` or `external`.
    pub origin: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginEngineRef {
    pub id: Uuid,
    pub name: String,
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginProfile {
    pub service_id: i64,
    pub name: String,
    pub policy_version: i64,
    pub tag_version: i64,
    pub updated_at: DateTime<Utc>,
}

/// Which Ranger plugin the engine runs (`egret_perm::ranger_plugin_report`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginReportDto {
    pub version: Option<String>,
    pub modelled: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotJudgedReason {
    /// egret does not run the engine.
    External,
    /// The profile has no provisioned engine recorded as running.
    NotRunning,
    /// The engine has no policy profile.
    NoProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Judged {
    Judged,
    NotJudged {
        reason: NotJudgedReason,
        sentence: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Policies,
    Tags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindStateDto {
    UpToDate,
    Pending,
    Behind,
    Quiet,
    Silent,
    NotReported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotReportedDto {
    NoDownload,
    VersionUnreported,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KindReport {
    pub kind: PluginKind,
    /// The profile's counter for this kind (`policy_version` or `tag_version`).
    pub current_version: i64,
    /// The newest poll of this kind by any plugin.
    pub last_download_at: Option<DateTime<Utc>>,
    /// The version the newest poll reported holding; `null` when it reported
    /// none (`-1`) or there is no poll.
    pub newest_reported_version: Option<i64>,
    /// The version egret served on the newest poll, kept for diagnosis; never
    /// counted as held.
    pub served_version: Option<i64>,
    pub state: KindStateDto,
    pub not_reported: Option<NotReportedDto>,
    pub sentence: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerRowDto {
    pub plugin_id: String,
    pub kind: String,
    pub last_known_version: Option<i64>,
    pub served_version: Option<i64>,
    pub at: DateTime<Utc>,
}

/// What the Ranger plugins of an engine's profile report downloading.
#[utoipa::path(
    get,
    path = "/api/control-plane/governance/engines/{id}/plugin",
    tag = "control-plane-governance",
    params(("id" = Uuid, Path, description = "Engine id")),
    description = "The plugin download ledger of the engine's policy profile, judged by the same \
                   rules as the Governance Overview's plugin findings. Records what plugins report \
                   holding. Requires policy:read and engine:read.",
    responses(
        (status = 200, description = "The engine's plugin download state", body = EnginePluginResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Lacks policy:read or engine:read", body = ErrorBody),
        (status = 404, description = "No engine with this id", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    )
)]
async fn engine_plugin(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<EnginePluginResponse>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let mut tx = begin_snapshot(&st).await?;
    let response = engine_plugin_via(&mut tx, id).await;
    tx.rollback().await.map_err(db("end the snapshot"))?;
    Ok(Json(response?))
}

fn db(what: &'static str) -> impl FnOnce(sqlx::Error) -> CpError {
    move |e| {
        tracing::error!(error = %e, "governance engine plugin: {what}");
        CpError::Internal
    }
}

type EngineRow = (
    Uuid,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    serde_json::Value,
);

async fn engine_plugin_via(
    c: &mut PgConnection,
    id: Uuid,
) -> Result<EnginePluginResponse, CpError> {
    let row: Option<EngineRow> = sqlx::query_as(
        "SELECT id, name, type, origin, status, authz_service_id, ranger_plugin_version, spec \
           FROM engine_mgmt.engines WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *c)
    .await
    .map_err(db("read the engine"))?;
    let Some((id, name, ty, origin, status, sid, declared, spec)) = row else {
        return Err(CpError::NotFound);
    };
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *c)
        .await
        .map_err(db("read the clock"))?;
    let report = egret_perm::ranger_plugin_report(&ty, &origin, declared.as_deref(), &spec);
    let engine = PluginEngine {
        id,
        name: name.clone(),
        r#type: ty.clone(),
        origin: origin.clone(),
        status,
    };
    let plugin = PluginReportDto {
        version: report.version,
        modelled: report.modelled,
        note: report.note,
    };
    let profile: Option<(i64, String, i64, i64, DateTime<Utc>)> = match sid {
        Some(sid) => sqlx::query_as(
            "SELECT id, name, policy_version, tag_version, updated_at \
               FROM policy.authz_services WHERE id = $1",
        )
        .bind(sid)
        .fetch_optional(&mut *c)
        .await
        .map_err(db("read the profile"))?,
        None => None,
    };
    let Some((sid, pname, pv, tv, updated_at)) = profile else {
        return Ok(EnginePluginResponse {
            generated_at: now,
            engine,
            profile: None,
            shared_with: Vec::new(),
            plugin,
            judged: Judged::NotJudged {
                reason: NotJudgedReason::NoProfile,
                sentence: "This engine has no policy profile, so no plugin downloads one.".into(),
            },
            kinds: Vec::new(),
            rows: Vec::new(),
        });
    };
    // Every engine bound to the profile, by name: the others are `sharedWith`;
    // the provisioned running ones are the Overview's population
    // (`drift_profiles`).
    let bound: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT id, name, origin, status FROM engine_mgmt.engines \
          WHERE authz_service_id = $1 ORDER BY name, id",
    )
    .bind(sid)
    .fetch_all(&mut *c)
    .await
    .map_err(db("read the bound engines"))?;
    let shared_with = bound
        .iter()
        .filter(|(eid, ..)| *eid != id)
        .map(|(eid, n, o, _)| PluginEngineRef {
            id: *eid,
            name: n.clone(),
            origin: o.clone(),
        })
        .collect();
    let running: Vec<&String> = bound
        .iter()
        .filter(|(_, _, o, s)| o != egret_perm::plugin_report::ORIGIN_EXTERNAL && s == "running")
        .map(|(_, n, ..)| n)
        .collect();
    let ledger = ledger_rows_via(c, &[sid])
        .await
        .map_err(db("read the plugin ledger"))?;
    let rows: Vec<LedgerRowDto> = ledger
        .iter()
        .map(|r| LedgerRowDto {
            plugin_id: r.plugin_id.clone(),
            kind: r.kind.clone(),
            last_known_version: r.last_known_version,
            served_version: r.served_version,
            at: r.at,
        })
        .collect();
    let profile_dto = PluginProfile {
        service_id: sid,
        name: pname.clone(),
        policy_version: pv,
        tag_version: tv,
        updated_at,
    };
    let not_judged = |reason, sentence: String| Judged::NotJudged { reason, sentence };
    let judged = if origin == egret_perm::plugin_report::ORIGIN_EXTERNAL {
        not_judged(
            NotJudgedReason::External,
            "egret does not judge external engines: it does not run them and cannot tell whether \
             they should be polling."
                .into(),
        )
    } else if running.is_empty() {
        not_judged(
            NotJudgedReason::NotRunning,
            format!(
                "No engine egret provisions for profile {pname} is recorded as running, so egret \
                 does not judge its plugins."
            ),
        )
    } else {
        Judged::Judged
    };
    let kinds = if judged == Judged::Judged {
        let versions = ProfileVersions {
            policy_version: pv,
            tag_version: tv,
            updated_at,
        };
        let refs: Vec<&LedgerRow> = ledger.iter().collect();
        let j = judge_plugins(&versions, &refs, now);
        let ctx = SentenceCtx {
            profile: &pname,
            first_running: running[0],
            engine_type: &ty,
            updated_at,
        };
        vec![
            kind_report(&ctx, &j, PluginKind::Policies, pv, &ledger),
            kind_report(&ctx, &j, PluginKind::Tags, tv, &ledger),
        ]
    } else {
        Vec::new()
    };
    Ok(EnginePluginResponse {
        generated_at: now,
        engine,
        profile: Some(profile_dto),
        shared_with,
        plugin,
        judged,
        kinds,
        rows,
    })
}

struct SentenceCtx<'a> {
    profile: &'a str,
    first_running: &'a str,
    engine_type: &'a str,
    updated_at: DateTime<Utc>,
}

fn kind_report(
    ctx: &SentenceCtx<'_>,
    j: &Judgement,
    kind: PluginKind,
    version: i64,
    ledger: &[LedgerRow],
) -> KindReport {
    let (name, what, state) = match kind {
        PluginKind::Policies => ("policies", "policy", j.policies),
        PluginKind::Tags => ("tags", "tag", j.tags),
    };
    // `ledger` is newest first.
    let newest = ledger.iter().find(|r| r.kind == name);
    let p = ctx.profile;
    let (dto, not_reported, sentence) = match state {
        KindState::UpToDate => (
            KindStateDto::UpToDate,
            None,
            format!("Plugins of profile {p} report holding {what} version {version}."),
        ),
        KindState::Pending => {
            let downloaded = ledger
                .iter()
                .find(|r| r.kind == name && r.served_version.is_some_and(|s| s >= version));
            let sentence = match downloaded {
                Some(r) => format!(
                    "A plugin downloaded {what} version {version} at {}; confirmation takes two \
                     polls.",
                    utc(r.at)
                ),
                None => format!(
                    "{} version {version} was set at {}; confirmation takes two polls.",
                    capitalise(what),
                    utc(ctx.updated_at)
                ),
            };
            (KindStateDto::Pending, None, sentence)
        }
        // The Overview's sentences (`governance_overview::drift`).
        KindState::Behind { version } => (
            KindStateDto::Behind,
            None,
            format!(
                "No plugin of profile {p} has reported holding {what} version {version}, set at \
                 {}.",
                utc(ctx.updated_at)
            ),
        ),
        KindState::Quiet { age_secs } => {
            let sentence = if matches!(j.liveness, Liveness::Quiet { .. }) {
                format!(
                    "No plugin has polled profile {p} for {} minutes.",
                    age_secs / 60
                )
            } else {
                format!(
                    "No plugin has polled the tags of profile {p} for {} minutes, while its \
                     policies are polled.",
                    age_secs / 60
                )
            };
            (KindStateDto::Quiet, None, sentence)
        }
        KindState::Silent => (
            KindStateDto::Silent,
            None,
            format!(
                "No plugin has polled profile {p} in 24 hours, while engine {} is recorded as \
                 running. egret cannot tell a stopped engine from one that cannot reach egret or \
                 does not poll.",
                ctx.first_running
            ),
        ),
        KindState::NotReported(NotReported::NoDownload) => {
            let mut s = format!("No plugin of profile {p} downloaded {name} in 24 hours.");
            if kind == PluginKind::Tags && ctx.engine_type == "starrocks" {
                s.push_str(" StarRocks never downloads tags.");
            }
            (
                KindStateDto::NotReported,
                Some(NotReportedDto::NoDownload),
                s,
            )
        }
        KindState::NotReported(NotReported::VersionUnreported) => (
            KindStateDto::NotReported,
            Some(NotReportedDto::VersionUnreported),
            format!(
                "Plugins polled since {what} version {version} was set, but none has reported a \
                 version yet."
            ),
        ),
    };
    KindReport {
        kind,
        current_version: version,
        last_download_at: newest.map(|r| r.at),
        newest_reported_version: newest
            .and_then(|r| r.last_known_version)
            .filter(|v| *v >= 0),
        served_version: newest.and_then(|r| r.served_version),
        state: dto,
        not_reported,
        sentence,
    }
}

fn utc(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}
