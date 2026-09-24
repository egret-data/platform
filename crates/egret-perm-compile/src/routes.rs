//! The `perm` HTTP surface (SP4a Task 9): grants, rules, `explain`, and a
//! manual `compile` trigger, nested under `/api/control-plane`.
//!
//! **Why this module lives in `egret-perm-compile` and not in `egret-perm`,
//! where the brief put it.** Every mutating route here has to recompile the
//! services its write affects, in the same transaction as the write. The
//! compiler is `egret-perm-compile`, which already depends on `egret-perm`;
//! putting the routes in `egret-perm` would mean `egret-perm` depending on
//! `egret-perm-compile` and a dependency cycle Cargo rejects outright. The
//! alternative — routes that write a grant without recompiling, with the
//! recompile bolted on somewhere above — is the exact defect this task
//! exists to prevent, so the routes moved to the crate that can do both. This
//! is the same shape `egret-policy` already has (store plus its admin router
//! in one crate).
//!
//! **The thing to get right here is not CRUD, it is the sync.** Before this
//! task the compiler had exactly one production caller, the catalog-tag write
//! path (`egret-bi::catalog_tags`); no grant or rule mutation surface existed
//! at all, so nothing could make the compiled policy set drift from the model.
//! A mutation surface that writes `perm.grants` and stops would make the
//! engine enforce the previous policy set while egret's own UI lists the new
//! grant as live — a permission that reads as granted and is not, or (worse,
//! on a delete) one that reads as revoked and is not. So:
//!
//! 1. Every mutating handler opens ONE transaction, writes the row, resolves
//!    the affected services from the row itself, recompiles each of them on
//!    that same transaction, and only then commits.
//! 2. A recompile failure propagates with `?` BEFORE `commit()`, so the write
//!    rolls back with it. There is no state in which a grant exists and its
//!    policies do not.
//! 3. `explain` never writes; `compile` is the manual repair trigger and is
//!    the only route that may use the pool-based `recompile_service`.
//!
//! Which services a mutation affects is derived in
//! [`crate::trigger::affected_services_for_grant`] /
//! [`crate::trigger::affected_services_for_rule`] — read that module's Task 9
//! section for the derivation, including the plainly-stated consequence that
//! ONE tag-rule mutation recompiles every Trino and StarRocks profile an
//! engine binds in the whole installation.
//!
//! Gating reuses the existing `policy:read` / `policy:update` permission
//! family rather than adding a key: SP4c may fold the hand-written policy
//! editor away entirely, and a key added now would have to be retired twice.

use std::sync::Arc;

use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use egret_auth::principal::Auth;
use egret_auth::session::SessionService;
use egret_core::http::ErrorBody;
use egret_perm::{
    create_grant_in_tx, create_rule_in_tx, delete_grant_in_tx, delete_rule_in_tx, evaluate,
    get_grant_via, get_rule_via, list_grants_via, list_rules_via, Action, Decision, Effect, Grant,
    GrantOrigin, PermStore, PrincipalSet, Reason, Rule, RuleKind, Scope, Selector, Subject,
};
use egret_policy::{tag_keys_for_resource_via, PolicyStore};

use crate::grants::validate_grant_shape;
use crate::rules::validate_rule_shape;
use crate::trigger::{
    affected_services_for_grant, affected_services_for_rule, lock_services_in_tx,
    recompile_service, recompile_services_in_tx, CompileSummary, SourceKind,
};

mod error;
pub use error::PermHttpError;

/// State for the `perm` router. Both stores must reach the SAME physical
/// Postgres database — `perm.*` and `policy.*` are two schemas in it, and the
/// whole point of the write path is one transaction spanning both.
#[derive(Clone)]
pub struct PermState {
    pub perm: PermStore,
    pub policy: PolicyStore,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// Policy decision point; every route gates through
    /// `egret_core::authz::require`. `platform_admin` bypasses inside the PDP.
    pub pdp: Arc<dyn egret_core::authz::Pdp>,
}

impl FromRef<PermState> for SessionService {
    fn from_ref(s: &PermState) -> Self {
        s.sessions.clone()
    }
}

/// Build the `perm` router (to be nested at `/api/control-plane`).
pub fn perm_router() -> Router<PermState> {
    Router::new()
        .route("/perm/grants", get(list_grants).post(create_grant))
        .route("/perm/grants/{id}", axum::routing::delete(delete_grant))
        .route("/perm/rules", get(list_rules).post(create_rule))
        .route("/perm/rules/{id}", axum::routing::delete(delete_rule))
        .route("/perm/explain", post(explain))
        .route("/perm/compile/{service_id}", post(compile))
}

/// The `perm` surface's OpenAPI document, merged into the top-level doc by
/// `egret-server` — the per-crate `openapi()` convention every annotated seam
/// follows.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    PermApi::openapi()
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_grants,
        create_grant,
        delete_grant,
        list_rules,
        create_rule,
        delete_rule,
        explain,
        compile
    ),
    components(schemas(
        NewGrant,
        NewRule,
        ExplainRequest,
        ExplainSubject,
        ExplainResponse,
        ExplainReason,
        CompileResponse,
        CreatedResponse,
        SkipReport,
        Grant,
        Rule,
        Scope,
        Selector,
        RuleKind,
        PrincipalSet,
        Action,
        Effect,
        ErrorBody,
    )),
    tags((name = "perm", description = "Data permission model (grants, rules, explain)"))
)]
struct PermApi;

// ---------------------------------------------------------------------------
// request/response shapes
// ---------------------------------------------------------------------------

/// A grant as authored over HTTP.
///
/// **There is no `origin` field, and that is deliberate.** `perm.grants.origin`
/// distinguishes a human's `explicit` grant from a `workspace_assignment` row
/// the assignment path owns and recreates from the assignment on demand (D3),
/// and `grants_one_per_assignment` allows at most one of the latter per
/// `(catalog_id, workspace_id)`. A human-authored grant is always `explicit`;
/// letting a request claim `workspace_assignment` would let the HTTP surface
/// mint rows the assignment sync believes it owns, which it would then
/// silently delete or refuse to recreate.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct NewGrant {
    pub effect: Effect,
    pub actions: Vec<Action>,
    pub scope: Scope,
    #[serde(default)]
    pub principals: PrincipalSet,
}

/// A named rule as authored over HTTP. `id` is minted by the database.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct NewRule {
    pub name: String,
    pub kind: RuleKind,
    pub selector: Selector,
    #[serde(default)]
    pub exempt: PrincipalSet,
}

/// The caller `explain` decides about.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainSubject {
    pub user: String,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub workspaces: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainRequest {
    pub subject: ExplainSubject,
    pub action: Action,
    pub target: Scope,
}

/// One reason, with its CLASS on the wire.
///
/// The class is the deliverable. Ranger's own Access audit shows `-` for the
/// most common denial — nothing matched — so a response that carried only
/// `allowed: false` would reproduce exactly the blindness `explain` exists to
/// remove. `no_grant_matched` and `explicit_deny` are different facts about
/// the model and stay different here: a subject a grant's `exclude` subtracted
/// matched no grant at all, which is not the same as being denied by one.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExplainReason {
    /// `allowed_by_grant` | `explicit_deny` | `no_grant_matched` |
    /// `masked_by_rule` | `filtered_by_rule`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_expr: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExplainResponse {
    pub allowed: bool,
    pub reasons: Vec<ExplainReason>,
    /// The tag keys egret resolved as present on `target` — the input the
    /// rule selectors were matched against. Returned so a steward can see
    /// WHY a mask did or did not apply, rather than having to guess whether
    /// the tag resolution or the rule was at fault.
    pub tags: Vec<String>,
}

impl From<&Reason> for ExplainReason {
    fn from(r: &Reason) -> Self {
        let mut out = ExplainReason {
            kind: String::new(),
            grant_id: None,
            rule_id: None,
            mask_type: None,
            filter_expr: None,
        };
        match r {
            Reason::AllowedByGrant { grant_id } => {
                out.kind = "allowed_by_grant".into();
                out.grant_id = Some(*grant_id);
            }
            Reason::ExplicitDeny { grant_id } => {
                out.kind = "explicit_deny".into();
                out.grant_id = Some(*grant_id);
            }
            Reason::NoGrantMatched => out.kind = "no_grant_matched".into(),
            Reason::MaskedByRule { rule_id, mask_type } => {
                out.kind = "masked_by_rule".into();
                out.rule_id = Some(*rule_id);
                out.mask_type = Some(mask_type.clone());
            }
            Reason::FilteredByRule {
                rule_id,
                filter_expr,
            } => {
                out.kind = "filtered_by_rule".into();
                out.rule_id = Some(*rule_id);
                out.filter_expr = Some(filter_expr.clone());
            }
        }
        out
    }
}

impl From<Decision> for ExplainResponse {
    fn from(d: Decision) -> Self {
        ExplainResponse {
            allowed: d.allowed,
            reasons: d.reasons.iter().map(ExplainReason::from).collect(),
            tags: Vec::new(),
        }
    }
}

/// A grant or rule a recompile could not express and left uncompiled. Surfaced
/// on the response rather than only in the logs: a create that succeeded but
/// compiled nothing for a service is a fact the author needs at authoring
/// time, not one a steward discovers from a query that returns no rows.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SkipReport {
    /// `grant` | `rule`.
    pub source_kind: String,
    pub id: Uuid,
    pub reason: String,
}

/// What a mutation's (or `compile`'s) recompile pass did, aggregated over
/// every service it touched.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct CompileResponse {
    /// The services this pass recompiled.
    pub services: Vec<i64>,
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub skipped: Vec<SkipReport>,
}

impl CompileResponse {
    fn from_summaries(services: Vec<i64>, summaries: &[CompileSummary]) -> Self {
        let mut out = CompileResponse {
            services,
            ..Default::default()
        };
        for s in summaries {
            out.created += s.created;
            out.updated += s.updated;
            out.deleted += s.deleted;
            out.unchanged += s.unchanged;
            for sk in &s.skipped {
                out.skipped.push(SkipReport {
                    source_kind: match sk.source_kind {
                        SourceKind::Grant => "grant".into(),
                        SourceKind::Rule => "rule".into(),
                    },
                    id: sk.id,
                    reason: sk.reason.clone(),
                });
            }
        }
        out
    }
}

/// A create's response: the id the database minted, and what the recompile
/// that committed with it did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedResponse {
    pub id: Uuid,
    pub compile: CompileResponse,
}

// ---------------------------------------------------------------------------
// grants
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/grants",
    tag = "perm",
    responses(
        (status = 200, description = "Every grant in the model, with its principal subtract", body = Vec<Grant>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_grants(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<Vec<Grant>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    Ok(Json(list_grants_via(&mut conn).await?))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/perm/grants",
    tag = "perm",
    request_body = NewGrant,
    responses(
        (status = 201, description = "Grant created AND the profiles it affects recompiled, in one transaction"),
        (status = 400, description = "The grant cannot be compiled for a profile it affects", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_grant(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<NewGrant>,
) -> Result<(axum::http::StatusCode, Json<CreatedResponse>), PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    if body.actions.is_empty() {
        return Err(PermHttpError::BadRequest(
            "a grant must name at least one action".into(),
        ));
    }
    let g = Grant {
        id: Uuid::nil(), // the database mints it
        effect: body.effect,
        actions: body.actions,
        scope: body.scope,
        principals: body.principals,
        origin: GrantOrigin::Explicit, // see `NewGrant`'s doc comment
    };
    // The recompile below refuses a grant that cannot compile — but only for
    // the profiles it actually touches, and that set is EMPTY for a catalog no
    // engine attaches yet. Without this check such a grant commits and then
    // breaks every future recompile of the profile that eventually reaches it,
    // including `egret-bi::catalog_tags`'s, which rolls back tag writes on the
    // whole warehouse. See `validate_grant_shape`.
    validate_grant_shape(&g)?;

    // ONE transaction: the affected profiles' locks, the row, and every
    // recompile. The affected set is derived from the grant's own catalog, so
    // it can be resolved (and locked) before the row exists — and it MUST be,
    // or this transaction holds a `perm.grants` row lock while reaching for a
    // profile lock, which deadlocks against a concurrent recompile holding
    // that profile and reaching for the grant row. See `lock_services_in_tx`.
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let services = affected_services_for_grant(&mut tx, &g).await?;
    lock_services_in_tx(&mut tx, &services).await?;
    let id = create_grant_in_tx(&mut tx, &g).await?;
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    let compile = CompileResponse::from_summaries(services, &summaries);
    tx.commit().await.map_err(PermHttpError::db)?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreatedResponse { id, compile }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/perm/grants/{id}",
    tag = "perm",
    params(("id" = Uuid, Path, description = "Grant id")),
    responses(
        (status = 200, description = "Grant deleted AND its compiled policies reaped, in one transaction"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such grant", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_grant(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<CompileResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    // Read the grant BEFORE deleting it: the affected-service set is derived
    // from its own `scope.catalog_id`, which is gone once the row is.
    let Some(g) = get_grant_via(&mut tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let services = affected_services_for_grant(&mut tx, &g).await?;
    // Before the DELETE, never after — see `lock_services_in_tx`.
    lock_services_in_tx(&mut tx, &services).await?;
    if !delete_grant_in_tx(&mut tx, id).await? {
        return Err(PermHttpError::NotFound);
    }
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    let compile = CompileResponse::from_summaries(services, &summaries);
    tx.commit().await.map_err(PermHttpError::db)?;
    Ok(Json(compile))
}

// ---------------------------------------------------------------------------
// rules
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/rules",
    tag = "perm",
    responses(
        (status = 200, description = "Every named rule in the model, with its exemption set", body = Vec<Rule>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_rules(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<Vec<Rule>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    Ok(Json(list_rules_via(&mut conn).await?))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/perm/rules",
    tag = "perm",
    request_body = NewRule,
    responses(
        (status = 201, description = "Rule created AND the profiles it affects recompiled, in one transaction"),
        (status = 400, description = "The rule cannot be compiled for a profile it affects", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_rule(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<NewRule>,
) -> Result<(axum::http::StatusCode, Json<CreatedResponse>), PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    // `perm.rules.name` is `text NOT NULL UNIQUE` with no emptiness CHECK, and
    // the name is what every skip log, compile error and audit line identifies
    // the rule by. Refused here for the same reason `create_grant` refuses an
    // empty `actions` — the two write paths should reject a nonsense body the
    // same way.
    if body.name.trim().is_empty() {
        return Err(PermHttpError::BadRequest(
            "a rule must have a non-blank name".into(),
        ));
    }
    let r = Rule {
        id: Uuid::nil(), // the database mints it
        name: body.name,
        kind: body.kind,
        selector: body.selector,
        exempt: body.exempt,
    };
    // Same reason as the grant path: an empty affected set validates nothing.
    validate_rule_shape(&r)?;

    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let services = affected_services_for_rule(&mut tx, &r).await?;
    lock_services_in_tx(&mut tx, &services).await?;
    let id = create_rule_in_tx(&mut tx, &r).await?;
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    let compile = CompileResponse::from_summaries(services, &summaries);
    tx.commit().await.map_err(PermHttpError::db)?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreatedResponse { id, compile }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/perm/rules/{id}",
    tag = "perm",
    params(("id" = Uuid, Path, description = "Rule id")),
    responses(
        (status = 200, description = "Rule deleted AND its compiled policies reaped, in one transaction"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such rule", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_rule(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<CompileResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    // Same reason as the grant delete: a rule's affected-service set depends
    // on its SELECTOR, which the row carries and the deletion removes.
    let Some(r) = get_rule_via(&mut tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let services = affected_services_for_rule(&mut tx, &r).await?;
    lock_services_in_tx(&mut tx, &services).await?;
    if !delete_rule_in_tx(&mut tx, id).await? {
        return Err(PermHttpError::NotFound);
    }
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    let compile = CompileResponse::from_summaries(services, &summaries);
    tx.commit().await.map_err(PermHttpError::db)?;
    Ok(Json(compile))
}

// ---------------------------------------------------------------------------
// explain
// ---------------------------------------------------------------------------

/// POST because the request carries a subject, an action and a whole target
/// scope — a body, not a query string. **It is nonetheless read-only**: it
/// opens no transaction and issues no write. It gates on `policy:read`, not
/// `policy:update`, precisely so the method does not decide the permission.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/explain",
    tag = "perm",
    request_body = ExplainRequest,
    responses(
        (status = 200, description = "The decision and the reasons behind it", body = ExplainResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn explain(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<ExplainRequest>,
) -> Result<Json<ExplainResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;

    let grants = list_grants_via(&mut conn).await?;
    let rules = list_rules_via(&mut conn).await?;

    // `egret_perm::evaluate` does no I/O — resolving WHICH tags are on the
    // target is the caller's job by construction (see `eval::selector_hits`),
    // and here the caller is egret. Tags attach to tables and columns only,
    // keyed `(warehouse_id, schema, table, column)` with `column = ''` meaning
    // the table itself, so a target that names no table can carry none.
    let tags = match (&body.target.schema, &body.target.table) {
        (Some(schema), Some(table)) => {
            tag_keys_for_resource_via(
                &mut conn,
                body.target.catalog_id,
                schema,
                table,
                body.target.column.as_deref().unwrap_or(""),
            )
            .await?
        }
        _ => Vec::new(),
    };

    let subject = Subject {
        user: body.subject.user,
        groups: body.subject.groups,
        roles: body.subject.roles,
        workspaces: body.subject.workspaces,
    };
    let decision = evaluate(&subject, body.action, &body.target, &grants, &rules, &tags);
    let mut out = ExplainResponse::from(decision);
    out.tags = tags;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// manual compile
// ---------------------------------------------------------------------------

/// The manual/repair trigger. Unlike the mutating routes it has no write of
/// its own to be atomic with, so it uses the pool-based `recompile_service`,
/// which opens and commits its own transaction.
///
/// It exists because the affected-service sets above are derived from the
/// mutated ROW, and there are model-adjacent changes no row mutation covers —
/// an engine newly bound to a profile, a profile that had no binding when a
/// grant was written, a tag renamed out from under a rule. This is how an
/// operator brings one profile current without editing the model.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/compile/{service_id}",
    tag = "perm",
    params(("service_id" = i64, Path, description = "policy.authz_services id")),
    responses(
        (status = 200, description = "What the recompile wrote", body = CompileResponse),
        (status = 400, description = "The model cannot be compiled for this profile", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn compile(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
) -> Result<Json<CompileResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    // Resolve the profile first so an unknown id is a 404 rather than the
    // 500 a `CompileError::Db("not found: service id …")` would otherwise
    // render as — `recompile_service` flattens the store's typed NotFound
    // into its own opaque `Db` variant.
    st.policy.get_service_by_id(service_id).await?;
    let summary = recompile_service(&st.perm, &st.policy, service_id).await?;
    Ok(Json(CompileResponse::from_summaries(
        vec![service_id],
        std::slice::from_ref(&summary),
    )))
}
