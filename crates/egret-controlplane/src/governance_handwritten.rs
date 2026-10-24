//! DG #4b (spec `docs/en/specs/2026-10-19-dg-classification-design.md` §4.2,
//! §4.3, §4.5): the hand-written Ranger policies that reach a Data object
//! (route A), name a principal (route B) or name a classification's tag
//! (route D). Each read runs in one `REPEATABLE READ, READ ONLY` snapshot and
//! is decided by `egret_perm_compile::handwritten` (CL7); nothing here matches.
//! All three need `policy:read`.
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use egret_perm_compile::handwritten::{
    self, Bucket, EveryonePolicy, EveryoneSummary, HandWrittenCatalog, HandWrittenEngine,
    HandWrittenEngineRef, HandWrittenError, HandWrittenPolicy, HandWrittenProfile,
    HandWrittenProfileRef, HandWrittenTarget, ItemSummary, ListedState, MatchWord, Naming,
    NamingHow, NotDeterminedReason, ObjectHandWritten, ObjectTarget, PrincipalHandWritten,
    PrincipalMark, PrincipalPolicy, PrincipalRef, PrincipalState, SeededCount, TagHandWritten,
    TagOn, TagPolicyRow, TagPolicyState, TagRef, UnreadablePolicy, ViaTag,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::governance_overview::begin_snapshot;
use crate::{CpError, CpState};

pub(crate) fn router() -> Router<CpState> {
    Router::new()
        .route(
            "/governance/objects/{catalog_id}/handwritten",
            get(object_handwritten),
        )
        .route(
            "/governance/principals/{kind}/{id}/handwritten",
            get(principal_handwritten),
        )
        .route("/classifications/{id}/handwritten", get(tag_handwritten))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(object_handwritten, principal_handwritten, tag_handwritten),
    components(schemas(
        ObjectHandWritten,
        HandWrittenCatalog,
        HandWrittenTarget,
        ListedState,
        NotDeterminedReason,
        HandWrittenProfile,
        HandWrittenEngine,
        HandWrittenPolicy,
        MatchWord,
        ViaTag,
        TagOn,
        SeededCount,
        ItemSummary,
        Bucket,
        PrincipalMark,
        PrincipalHandWritten,
        PrincipalRef,
        PrincipalState,
        PrincipalPolicy,
        Naming,
        NamingHow,
        HandWrittenEngineRef,
        EveryoneSummary,
        EveryonePolicy,
        TagHandWritten,
        TagRef,
        TagPolicyState,
        TagPolicyRow,
        HandWrittenProfileRef,
        UnreadablePolicy
    ))
)]
pub(crate) struct ControlPlaneGovernanceHandWrittenApi;

fn map_err(e: HandWrittenError) -> CpError {
    match e {
        HandWrittenError::NotFound => CpError::NotFound,
        HandWrittenError::BadRequest(m) => CpError::BadRequest(m),
        other => {
            tracing::error!(error = %other, "governance hand-written read");
            CpError::Internal
        }
    }
}

/// The object a route A request names.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ObjectQuery {
    /// The schema (StarRocks: database); needed for `table`.
    pub schema: Option<String>,
    /// The table or view; needs `schema`, needed for `column`.
    pub table: Option<String>,
    /// The column; needs `table`.
    pub column: Option<String>,
}

/// Hand-written policies that reach a Data object.
#[utoipa::path(
    get,
    path = "/api/control-plane/governance/objects/{catalog_id}/handwritten",
    tag = "control-plane-governance",
    params(("catalog_id" = Uuid, Path, description = "Registry catalog id"), ObjectQuery),
    description = "Every hand-written (operator) Ranger policy of the profiles whose engines reach \
                   the catalog, and of their tag services, that reaches the object: `applies` \
                   (the policy names this object, or it and everything below it), `part` (it \
                   names something below), `parent` (it names a parent only, which engines do not \
                   apply to this object's data) or `unknown` (the body cannot be read). A resource \
                   match, not an access decision. egret-seeded policies are counted per profile. \
                   External engines are read from the database only. Requires policy:read.",
    responses(
        (status = 200, description = "The hand-written policies reaching the object", body = ObjectHandWritten),
        (status = 400, description = "A level named without its parent, or empty or `*`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Lacks policy:read", body = ErrorBody),
        (status = 404, description = "No catalog with this id", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    )
)]
async fn object_handwritten(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(catalog_id): Path<Uuid>,
    Query(q): Query<ObjectQuery>,
) -> Result<Json<ObjectHandWritten>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let target = ObjectTarget {
        catalog_id,
        schema: q.schema,
        table: q.table,
        column: q.column,
    };
    let mut tx = begin_snapshot(&st).await?;
    let out = handwritten::handwritten_for_object_via(&mut tx, &target).await;
    let _ = tx.rollback().await;
    Ok(Json(out.map_err(map_err)?))
}

/// Hand-written policies that name a principal.
#[utoipa::path(
    get,
    path = "/api/control-plane/governance/principals/{kind}/{id}/handwritten",
    tag = "control-plane-governance",
    params(
        ("kind" = String, Path, description = "`user`, `group` or `role`"),
        ("id" = String, Path, description = "A user's id, or a group's or role's name"),
    ),
    description = "Every hand-written (operator) Ranger policy, in every profile and tag service, \
                   with an item naming the principal: a user's username (`direct`), or a group or \
                   role the user holds (`group` / `role`, with `onEngines: false`: engines resolve \
                   no groups or roles, so such an item reaches nobody there). Items naming `public` \
                   or `{USER}` are reported under `everyone`, never attributed. A user with no \
                   username is `no_username` with only the `everyone` policies. Requires policy:read.",
    responses(
        (status = 200, description = "The hand-written policies naming the principal", body = PrincipalHandWritten),
        (status = 400, description = "An unknown kind", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Lacks policy:read", body = ErrorBody),
        (status = 404, description = "No such user, group or role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    )
)]
async fn principal_handwritten(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<PrincipalHandWritten>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut tx = begin_snapshot(&st).await?;
    let out = handwritten::handwritten_for_principal_via(&mut tx, &kind, &id).await;
    let _ = tx.rollback().await;
    Ok(Json(out.map_err(map_err)?))
}

/// Hand-written tag policies of a classification.
#[utoipa::path(
    get,
    path = "/api/control-plane/classifications/{id}/handwritten",
    tag = "control-plane-governance",
    params(("id" = Uuid, Path, description = "Tag id")),
    description = "Every hand-written (operator) policy of every usable tag service whose tag \
                   resource matches this classification's NAME through the tag service-def's \
                   matcher (wildcards yes, case folding no), with the Trino profiles and engines \
                   that download it. Classification names are unique ignoring case (migration \
                   0207), so no other classification shares the name. `no_tag_service` when no \
                   Trino profile has a tag service. \
                   Requires policy:read.",
    responses(
        (status = 200, description = "The hand-written tag policies naming the classification", body = TagHandWritten),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Lacks policy:read", body = ErrorBody),
        (status = 404, description = "No tag with this id", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    )
)]
async fn tag_handwritten(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<TagHandWritten>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut tx = begin_snapshot(&st).await?;
    let out = handwritten::handwritten_for_tag_via(&mut tx, id).await;
    let _ = tx.rollback().await;
    Ok(Json(out.map_err(map_err)?))
}
