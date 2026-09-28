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
    get_grant_via, get_rule_via, list_grants_via, list_rules_via, update_grant_in_tx,
    update_rule_in_tx, Action, ClearanceGate, Decision, Effect, Grant, GrantOrigin, PermStore,
    PrincipalKind, PrincipalSet, Reason, Rule, RuleKind, Scope, Selector, Subject,
};
use egret_policy::{
    catalog_names_for_ids_via, clearance_gates_for_tags_via, tag_keys_for_resource_via,
    tag_names_for_keys_via, PolicyStore,
};
use sqlx::{Postgres, Transaction};

use crate::grants::validate_grant_shape;
use crate::rules::validate_rule_shape;
use crate::trigger::{
    affected_services_for_grant, affected_services_for_rule, lock_services_in_tx,
    merge_service_ids, recompile_service, recompile_services_in_tx, CompileSummary, SkippedItem,
    SourceKind,
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
        .route(
            "/perm/grants/{id}",
            axum::routing::patch(patch_grant).delete(delete_grant),
        )
        .route("/perm/rules", get(list_rules).post(create_rule))
        .route(
            "/perm/rules/{id}",
            axum::routing::patch(patch_rule).delete(delete_rule),
        )
        .route("/perm/preview", post(preview))
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
        patch_grant,
        delete_grant,
        list_rules,
        create_rule,
        patch_rule,
        delete_rule,
        preview,
        explain,
        compile
    ),
    components(schemas(
        NewGrant,
        NewRule,
        PreviewRequest,
        PreviewResponse,
        AffectedProfile,
        ExplainRequest,
        ExplainSubject,
        ExplainResponse,
        ExplainReason,
        CompileResponse,
        CreatedResponse,
        SkipReport,
        Grant,
        GrantView,
        Rule,
        RuleView,
        Scope,
        Selector,
        RuleKind,
        PrincipalSet,
        // Registered explicitly (it is only ever reached through
        // `PrincipalSet`'s `$ref`) so the merged document carries the shape
        // `PrincipalSet` actually points at. See `egret_perm::Principal`'s
        // doc comment: its schema name is `PermPrincipal` precisely because
        // `egret-policy`'s picker type already owns `Principal`, and before
        // the rename one silently overwrote the other.
        egret_perm::Principal,
        PrincipalKind,
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

/// A grant as the READ surface returns it: the stored [`Grant`], unchanged,
/// plus the display name of the catalog its `scope.catalog_id` points at.
///
/// **Why the name belongs on the payload (SP4b-2 finding 1).** These routes
/// gate on `policy:read`; `GET /control-plane/catalogs` gates on
/// `catalog_registry:read`. The Data permissions page rendered its one primary
/// column — D5's "a grant reads as a sentence" — by resolving `catalog_id`
/// against that second endpoint, so for a holder of `policy:read` alone (the
/// very persona the page was built for) the sentence degraded to
/// `role analyst cannot read 4a8fe34a-….sales.orders`. A page's main feature
/// must not need a permission that page's own gate does not require. The
/// compiler already resolves id→name internally (D6) — the backend knew this
/// name and simply never told the client.
///
/// **`#[serde(flatten)]`, so the wire shape stays flat** — `{...grant,
/// "catalog_name": "wh"}`, one object, and a field added to `Grant` tomorrow
/// appears here with nothing to update. The same idiom
/// `DataApiEndpointListItemDto` and `EngineDetailDto` use; utoipa renders it
/// as an `allOf`, which makes the generated TS type assignable to `Grant`.
///
/// `scope.catalog_id` is untouched: it is what a write sends back. This adds,
/// it does not replace. `catalog_name` is `None` only when the catalog row is
/// gone (an id whose catalog was deleted under a cascade this read raced) —
/// never a fabricated or key-derived stand-in.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GrantView {
    #[serde(flatten)]
    pub grant: Grant,
    /// `registry.catalogs.name` for `scope.catalog_id`, resolved server-side.
    pub catalog_name: Option<String>,
}

/// A rule as the READ surface returns it: the stored [`Rule`], unchanged, plus
/// whichever display name its selector needs — see [`GrantView`] for why the
/// resolution happens here and not in the client.
///
/// A rule's selector is one of two shapes, so exactly one of these is ever
/// `Some`: `catalog_name` for a `path` selector (the same resolution a grant's
/// scope gets), `tag_name` for a `tag` selector. A tag rule stores `tags.key`
/// (`pii`) because that is the stable identifier (D6), while a steward reads
/// the display name (`PII`) — and `GET /tags` needs `tag:read`, which
/// `policy:read` does not imply either.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleView {
    #[serde(flatten)]
    pub rule: Rule,
    /// `registry.catalogs.name` for a `path` selector's catalog; `None` for a
    /// `tag` selector.
    pub catalog_name: Option<String>,
    /// `tags.name` for a `tag` selector's `key`; `None` for a `path` selector.
    pub tag_name: Option<String>,
}

/// The caller `explain` decides about.
///
/// **No `workspaces` field (SP4a Task 15, D3 defect 2 — removed, not just
/// ignored).** It used to be caller-supplied, `#[serde(default)]` to
/// `vec![]`, and the evaluator trusted it verbatim — so a caller naming just
/// a `user` (the natural way to call this endpoint) silently got `denied`
/// for every `workspace_members` grant, however real the compiled Ranger
/// policy made that access: the live gate measured exactly that. `explain`
/// now resolves `user`'s CURRENT workspace memberships itself
/// (`crate::members::workspaces_for_username`), off the same
/// `access_grants`/`users.username` truth the compiler's own expansion reads
/// — a caller-supplied list is a second, driftable copy of a fact the
/// database already answers, and keeping the field would let a caller
/// override that fact when what it should be describing is the caller's
/// principal, not a value only egret can verify.
///
/// **And no `roles` field either (SP4c Task 7 — the same defect, on the field
/// next to it, removed for the same reason).** The SP4c live gate
/// (`docs/en/plans/2026-09-28-sp4c-live-result.md`) measured both directions
/// of it: naming only a `user` returned `clearance_required` for the subject
/// the engine had just served the classified column to, and
/// `{"user":"analyst","roles":["sp4c_cleared"]}` returned `allowed` for the
/// subject the engine refuses — a caller asserting a role it does not hold was
/// believed, so the diagnostic could be told a lie and would repeat it.
/// `explain` now reads the subject's roles from `access_grants`
/// (`crate::members::roles_for_username`) through the same live/`allow`/
/// unexpired predicate the clearance compiler's own expansion uses. **This is
/// not a "what-if" endpoint**: D9 (SP4b) defines it as answering why a subject
/// **is** allowed or refused, and a field that let a caller invent the
/// subject's clearance would answer a different question while looking
/// authoritative.
///
/// `groups` remains caller-supplied and is deliberately left so: egret runs no
/// usersync and no group provider, so a group name reaches no engine at all
/// (D7/D2) and the UI already says the field is matched here and nowhere else.
/// Resolving it server-side would give the field an authority the data plane
/// does not honour, which is the opposite of the fix above.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainSubject {
    pub user: String,
    #[serde(default)]
    pub groups: Vec<String>,
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
    /// `allowed_by_grant` | `allowed_by_group_only` | `explicit_deny` |
    /// `no_grant_matched` | `masked_by_rule` | `filtered_by_rule` |
    /// `clearance_required`.
    ///
    /// `allowed_by_group_only` is an allow the DATA PLANE will refuse: the
    /// grant matched only through a `group` principal, and no engine egret
    /// provisions can see a group (D7). It is its own string because a caller
    /// parsing `kind` never sees the prose the page shows beside it.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_expr: Option<String>,
    /// `clearance_required` only: the gating tag's key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// `clearance_required` only: the role(s) that would clear it — holding
    /// any ONE is enough (`egret_perm_compile::clearance`'s "OR across
    /// roles" semantics).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_roles: Option<Vec<String>>,
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
            tag: None,
            required_roles: None,
        };
        match r {
            Reason::AllowedByGrant { grant_id } => {
                out.kind = "allowed_by_grant".into();
                out.grant_id = Some(*grant_id);
            }
            // SP4c review: the model allows it and no engine will, so it does
            // not get to wear `allowed_by_grant`'s wire string — a caller
            // parsing `kind` never sees the page's prose warning.
            Reason::AllowedByGroupOnly { grant_id } => {
                out.kind = "allowed_by_group_only".into();
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
            Reason::ClearanceRequired {
                tag,
                required_roles,
            } => {
                out.kind = "clearance_required".into();
                out.tag = Some(tag.clone());
                out.required_roles = Some(required_roles.clone());
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

/// What `preview` is asked to try — SP4b D2.
///
/// It is the create body plus an optional `id`, rather than a separate shape,
/// because the preview runs the REAL write: anything the preview could accept
/// and a write could not would be a lie by construction. Exactly one of `grant`
/// and `rule` must be present; two optional fields rather than a tagged union
/// so the generated client is one plain object with two optional members.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PreviewRequest {
    /// Present ⇒ preview a PATCH of this existing row. Absent ⇒ a create.
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub grant: Option<NewGrant>,
    #[serde(default)]
    pub rule: Option<NewRule>,
}

/// One policy profile a write would touch, named so a steward reads
/// "Trino-prod" rather than a bare id.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AffectedProfile {
    pub id: i64,
    pub name: String,
}

/// What the write WOULD have done. `compile` is the same summary the write
/// itself returns, produced by the same recompile — this endpoint rolls that
/// work back rather than recomputing it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewResponse {
    pub affected: Vec<AffectedProfile>,
    pub compile: CompileResponse,
}

/// A grant, a rule, or a tag's classification clearance that a recompile could
/// not express and left uncompiled. Surfaced
/// on the response rather than only in the logs: a create that succeeded but
/// compiled nothing for a service is a fact the author needs at authoring
/// time, not one a steward discovers from a query that returns no rows.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SkipReport {
    /// `grant` | `rule` | `clearance`.
    pub source_kind: String,
    /// The `perm.grants`/`perm.rules` id — or, for `clearance`, the `tags.id`
    /// whose classification gate could not be expressed (SP4c Task 2).
    pub id: Uuid,
    pub reason: String,
}

impl From<SkippedItem> for SkipReport {
    /// The ONE place a `SourceKind` becomes a wire string. `egret-controlplane`
    /// reports clearance skips on the access-policy endpoint (SP4c review I5)
    /// off this same conversion, so the two surfaces cannot spell a kind
    /// differently for the same fact.
    fn from(sk: SkippedItem) -> Self {
        SkipReport {
            source_kind: match sk.source_kind {
                SourceKind::Grant => "grant".into(),
                SourceKind::Rule => "rule".into(),
                // SP4c Task 2: a gated tag whose clearance this profile could
                // not express — `id` is the `tags.id`, not a grant or rule id
                // (see `SkipReport::id`).
                SourceKind::Clearance => "clearance".into(),
            },
            id: sk.id,
            reason: sk.reason,
        }
    }
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
                out.skipped.push(SkipReport::from(sk.clone()));
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
// the one implementation of a model write
// ---------------------------------------------------------------------------

/// What a model write did: the id the row now has, the services the write
/// recompiled, and what each recompile wrote.
struct WriteOutcome {
    id: Uuid,
    services: Vec<i64>,
    summaries: Vec<CompileSummary>,
}

impl WriteOutcome {
    fn into_compile_response(self) -> (Uuid, CompileResponse) {
        let compile = CompileResponse::from_summaries(self.services, &self.summaries);
        (self.id, compile)
    }
}

/// Create (`id: None`) or patch in place (`id: Some(_)`) one grant on the
/// caller's open transaction, recompiling every profile the write can change.
/// Neither begins nor commits: the caller owns both, which is what lets the
/// create route, the patch route and the (rolled-back) preview route run the
/// same code rather than three implementations of the same rules.
///
/// **The patch-specific hazard this function exists to hold, once.** A patch
/// can move a grant to a different catalog, and the profile the grant USED to
/// reach must be recompiled too or its compiled policy stays behind, served to
/// that engine forever with nothing in the model producing it (SP4a's Critical,
/// through the edit door). So the affected set is the UNION of the set derived
/// from the row as it stands now and the set derived from the row as it will
/// stand — read before the write, merged by
/// [`crate::trigger::merge_service_ids`], locked ascending, recompiled once.
async fn write_grant(
    tx: &mut Transaction<'_, Postgres>,
    id: Option<Uuid>,
    body: NewGrant,
) -> Result<WriteOutcome, PermHttpError> {
    if body.actions.is_empty() {
        return Err(PermHttpError::BadRequest(
            "a grant must name at least one action".into(),
        ));
    }
    // **A grant with nothing in `principals.include` REVOKES, silently** (SP4b-1
    // review, I-2). `NewGrant.principals` is `#[serde(default)]`, so a body that
    // omits the field deserializes to an empty `PrincipalSet` — and
    // `compile_desired` treats an empty include as a SKIP (`trigger.rs`, "the
    // grant's principals name no engine principal"), which means the recompile
    // produces nothing for the grant and the sweep REAPS the policy it had.
    // Over a PATCH that is a 200 that quietly stops enforcing a live grant, and
    // because the field is optional in the schema the generated TypeScript types
    // it `principals?` and cannot catch it at the call site either. So the guard
    // is here, at the authoring boundary, refused the same way an empty
    // `actions` is.
    //
    // **It cannot go in the compiler instead**, which is why the skip stays: an
    // implicit `workspace_assignment` grant legitimately expands to an empty
    // include when its workspace has no members yet, and refusing that would
    // make every recompile of the profile fail. `exempt` is untouched for the
    // same kind of reason — empty there means "no exemptions", which is a
    // meaningful thing to say.
    if body.principals.include.is_empty() {
        return Err(PermHttpError::BadRequest(
            "a grant must name at least one principal in `principals.include` — a grant \
             that includes nobody enforces nothing, and on a patch it would silently \
             revoke the grant it replaces"
                .into(),
        ));
    }

    // The OLD affected set, and the origin the patched row keeps. Read from
    // the stored row BEFORE anything is written — after the UPDATE the old
    // catalog is gone and the profiles it reached are unrecoverable.
    let (old_services, origin) = match id {
        Some(id) => {
            let Some(old) = get_grant_via(&mut **tx, id).await? else {
                return Err(PermHttpError::NotFound);
            };
            // A `workspace_assignment` grant is the assignment path's row (D3):
            // it is derived from `(catalog, workspace)` and recreated from the
            // assignment on demand, so an edit here would either be reverted by
            // the next sync or collide with `grants_one_per_assignment`. The
            // create path refuses to MINT one for the same reason.
            if !matches!(old.origin, GrantOrigin::Explicit) {
                return Err(PermHttpError::BadRequest(
                    "this grant is derived from a workspace assignment and is owned by the \
                     assignment sync; change the assignment instead"
                        .into(),
                ));
            }
            (
                affected_services_for_grant(&mut **tx, &old).await?,
                old.origin,
            )
        }
        // The database mints the id on a create; `GrantOrigin::Explicit` is the
        // only origin this surface may write — see `NewGrant`'s doc comment.
        None => (Vec::new(), GrantOrigin::Explicit),
    };

    let g = Grant {
        id: id.unwrap_or_else(Uuid::nil),
        effect: body.effect,
        actions: body.actions,
        scope: body.scope,
        principals: body.principals,
        origin,
    };
    // The recompile below refuses a grant that cannot compile — but only for
    // the profiles it actually touches, and that set is EMPTY for a catalog no
    // engine attaches yet. Without this check such a grant commits and then
    // breaks every future recompile of the profile that eventually reaches it,
    // including `egret-bi::catalog_tags`'s, which rolls back tag writes on the
    // whole warehouse. See `validate_grant_shape`. Run on the patch path too:
    // a patch that could store an uncompilable row reopens at update exactly
    // the defect SP4a closed at create.
    validate_grant_shape(&g)?;

    let new_services = affected_services_for_grant(&mut **tx, &g).await?;
    let services = merge_service_ids(&old_services, &new_services);

    // The affected set is derived from the grant's own catalog, so it can be
    // resolved (and locked) before the row is written — and it MUST be, or
    // this transaction holds a `perm.grants` row lock while reaching for a
    // profile lock, which deadlocks against a concurrent recompile holding
    // that profile and reaching for the grant row. See `lock_services_in_tx`.
    lock_services_in_tx(tx, &services).await?;
    let id = match id {
        Some(id) => {
            // **The old row was read WITHOUT `FOR UPDATE`, before these locks
            // existed** (SP4b-1 review, I-3), because taking a `perm.grants` row
            // lock first is the deadlock `lock_services_in_tx` documents. So the
            // read can be stale: two overlapping PATCHes of one grant can both
            // see the same old catalog, and the second one's union then misses
            // the profile the first one moved the grant to — leaving that
            // profile's compiled policy unswept and enforced forever, SP4a's
            // Critical one door further in.
            //
            // Re-reading HERE closes it. Every profile either patch can touch is
            // locked by now, and each holds its locks until commit, so whichever
            // transaction got them second sees the other's committed row. If the
            // row still affects only profiles this request locked, the union is
            // sound and the write proceeds; if it has moved outside that set,
            // nothing is written and the caller gets a 409 to retry against the
            // state that won. Refusing is the conservative half: recompiling the
            // newly-discovered profile would mean taking a lock out of ascending
            // order, which is the deadlock again.
            ensure_grant_still_covered(tx, id, &services).await?;
            if !update_grant_in_tx(tx, id, &g).await? {
                return Err(PermHttpError::NotFound);
            }
            id
        }
        None => create_grant_in_tx(tx, &g).await?,
    };
    let summaries = recompile_services_in_tx(tx, &services).await?;
    Ok(WriteOutcome {
        id,
        services,
        summaries,
    })
}

/// Re-read grant `id` now that every profile in `locked` is locked, and refuse
/// if the row has moved outside that set — see the call site in [`write_grant`]
/// for why the first read cannot take a row lock and what a stale one costs.
/// `locked` is ascending, so `binary_search` is the membership test.
async fn ensure_grant_still_covered(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    locked: &[i64],
) -> Result<(), PermHttpError> {
    let Some(current) = get_grant_via(&mut **tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let now = affected_services_for_grant(&mut **tx, &current).await?;
    if let Some(escaped) = now.iter().find(|s| locked.binary_search(s).is_err()) {
        return Err(PermHttpError::Conflict(format!(
            "this grant changed while the request was waiting: it now affects policy \
             profile {escaped}, which this request did not lock, so patching it here \
             would leave that profile's compiled policy behind — retry"
        )));
    }
    Ok(())
}

/// The rule twin of [`ensure_grant_still_covered`], for the same reason.
async fn ensure_rule_still_covered(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    locked: &[i64],
) -> Result<(), PermHttpError> {
    let Some(current) = get_rule_via(&mut **tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let now = affected_services_for_rule(&mut **tx, &current).await?;
    if let Some(escaped) = now.iter().find(|s| locked.binary_search(s).is_err()) {
        return Err(PermHttpError::Conflict(format!(
            "this rule changed while the request was waiting: it now affects policy \
             profile {escaped}, which this request did not lock, so patching it here \
             would leave that profile's compiled policy behind — retry"
        )));
    }
    Ok(())
}

/// Create (`id: None`) or patch in place (`id: Some(_)`) one rule — the rule
/// twin of [`write_grant`], including the old-plus-new union, which matters
/// here for a second reason: a patch can turn a path selector into a tag
/// selector or the other way round, so the two sets can differ by the whole
/// installation rather than by one profile.
async fn write_rule(
    tx: &mut Transaction<'_, Postgres>,
    id: Option<Uuid>,
    body: NewRule,
) -> Result<WriteOutcome, PermHttpError> {
    // `perm.rules.name` is `text NOT NULL UNIQUE` with no emptiness CHECK, and
    // the name is what every skip log, compile error and audit line identifies
    // the rule by. Refused here for the same reason a grant's empty `actions`
    // is — the write paths should reject a nonsense body the same way.
    if body.name.trim().is_empty() {
        return Err(PermHttpError::BadRequest(
            "a rule must have a non-blank name".into(),
        ));
    }

    let old_services = match id {
        Some(id) => {
            let Some(old) = get_rule_via(&mut **tx, id).await? else {
                return Err(PermHttpError::NotFound);
            };
            affected_services_for_rule(&mut **tx, &old).await?
        }
        None => Vec::new(),
    };

    let r = Rule {
        id: id.unwrap_or_else(Uuid::nil),
        name: body.name,
        kind: body.kind,
        selector: body.selector,
        exempt: body.exempt,
    };
    // Same reason as the grant path: an empty affected set validates nothing.
    validate_rule_shape(&r)?;

    let new_services = affected_services_for_rule(&mut **tx, &r).await?;
    let services = merge_service_ids(&old_services, &new_services);

    lock_services_in_tx(tx, &services).await?;
    let id = match id {
        Some(id) => {
            // Same stale-read window as `write_grant`'s, and the same close —
            // read its call-site comment. A rule's version of "moved outside the
            // locked set" is a concurrent patch turning a path selector into a
            // tag one or back.
            ensure_rule_still_covered(tx, id, &services).await?;
            if !update_rule_in_tx(tx, id, &r).await? {
                return Err(PermHttpError::NotFound);
            }
            id
        }
        None => create_rule_in_tx(tx, &r).await?,
    };
    let summaries = recompile_services_in_tx(tx, &services).await?;
    Ok(WriteOutcome {
        id,
        services,
        summaries,
    })
}

// ---------------------------------------------------------------------------
// display names for the read surface
// ---------------------------------------------------------------------------

/// `catalog_id` → `registry.catalogs.name` for every DISTINCT id in `ids`, in
/// one query.
///
/// De-duplicated first: a realistic model has many grants over few catalogs,
/// and `= ANY($1)` with the same uuid repeated a hundred times asks Postgres to
/// do a hundred times the work for one answer. An id with no row is simply
/// absent from the map, so its view renders `catalog_name: null` and the client
/// falls back to the id — the same thing it displayed before this field
/// existed, never a guess.
async fn catalog_names(
    conn: &mut sqlx::PgConnection,
    ids: impl Iterator<Item = Uuid>,
) -> Result<std::collections::HashMap<Uuid, String>, PermHttpError> {
    let distinct: Vec<Uuid> = ids
        .collect::<std::collections::BTreeSet<Uuid>>()
        .into_iter()
        .collect();
    Ok(catalog_names_for_ids_via(conn, &distinct)
        .await?
        .into_iter()
        .map(|c| (c.id, c.name))
        .collect())
}

// ---------------------------------------------------------------------------
// grants
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/grants",
    tag = "perm",
    responses(
        (status = 200, description = "Every grant in the model, with its principal subtract and its catalog's resolved name", body = Vec<GrantView>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_grants(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<Vec<GrantView>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    let grants = list_grants_via(&mut conn).await?;
    let names = catalog_names(&mut conn, grants.iter().map(|g| g.scope.catalog_id)).await?;
    Ok(Json(
        grants
            .into_iter()
            .map(|g| GrantView {
                catalog_name: names.get(&g.scope.catalog_id).cloned(),
                grant: g,
            })
            .collect(),
    ))
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
    // ONE transaction: the affected profiles' locks, the row, and every
    // recompile — see `write_grant`, which is the same code the patch and
    // preview routes run.
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let (id, compile) = write_grant(&mut tx, None, body)
        .await?
        .into_compile_response();
    tx.commit().await.map_err(PermHttpError::db)?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreatedResponse { id, compile }),
    ))
}

/// **Edit in place, keeping the id — SP4b D1.** The body is the whole grant,
/// exactly the create route's shape: this replaces the grant's body and keeps
/// its identity, which is what a UI "Edit" must mean. Before this route the
/// only way to change a grant was delete-plus-create, i.e. a new id, two
/// recompiles and a window with no row — and an `explain` reason carries the
/// `grant_id` the UI deep-links from, so every such link broke on every edit.
#[utoipa::path(
    patch,
    path = "/api/control-plane/perm/grants/{id}",
    tag = "perm",
    params(("id" = Uuid, Path, description = "Grant id")),
    request_body = NewGrant,
    description = "Replace a grant's body, keeping its id, and recompile the UNION of the \
                   profiles it affected before and after the change — a patch that moves the \
                   grant to another catalog must reap the policy from the profile it used to \
                   reach, or that engine enforces it forever.\n\n\
                   **The body is the WHOLE grant, exactly the create route's shape.** This \
                   replaces the grant and keeps only its id; a field left out is cleared, not \
                   preserved. `principals.include` may not be empty — a grant that includes \
                   nobody compiles to nothing, so accepting one here would silently revoke \
                   the grant it replaces.",
    responses(
        (status = 200, description = "Grant updated AND every affected profile recompiled, in one transaction", body = CompileResponse),
        (status = 400, description = "The patched grant cannot be compiled, names no principal or no action, or is owned by the workspace-assignment sync", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such grant", body = ErrorBody),
        (status = 409, description = "The grant changed concurrently and now affects a profile this request did not lock; retry", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn patch_grant(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<NewGrant>,
) -> Result<Json<CompileResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let (_, compile) = write_grant(&mut tx, Some(id), body)
        .await?
        .into_compile_response();
    tx.commit().await.map_err(PermHttpError::db)?;
    Ok(Json(compile))
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
        (status = 200, description = "Every named rule in the model, with its exemption set and its selector's resolved display name", body = Vec<RuleView>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_rules(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<Vec<RuleView>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    let rules = list_rules_via(&mut conn).await?;
    let names = catalog_names(
        &mut conn,
        rules.iter().filter_map(|r| match &r.selector {
            Selector::Path(s) => Some(s.catalog_id),
            Selector::Tag { .. } => None,
        }),
    )
    .await?;
    let keys: Vec<String> = rules
        .iter()
        .filter_map(|r| match &r.selector {
            Selector::Tag { key } => Some(key.clone()),
            Selector::Path(_) => None,
        })
        .collect();
    let tags: std::collections::HashMap<String, String> = tag_names_for_keys_via(&mut conn, &keys)
        .await?
        .into_iter()
        .collect();
    Ok(Json(
        rules
            .into_iter()
            .map(|r| {
                let (catalog_name, tag_name) = match &r.selector {
                    Selector::Path(s) => (names.get(&s.catalog_id).cloned(), None),
                    Selector::Tag { key } => (None, tags.get(key).cloned()),
                };
                RuleView {
                    rule: r,
                    catalog_name,
                    tag_name,
                }
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/perm/rules",
    tag = "perm",
    description = "Create a named mask or row-filter rule and recompile the profiles it affects, \
                   in one transaction. A tag-selected rule is installation-wide: it recompiles \
                   every Trino and StarRocks profile an engine binds, and a compile failure on any \
                   one of them fails this request and rolls the rule back.",
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
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let (id, compile) = write_rule(&mut tx, None, body)
        .await?
        .into_compile_response();
    tx.commit().await.map_err(PermHttpError::db)?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreatedResponse { id, compile }),
    ))
}

/// Edit a rule in place, keeping the id — the rule half of SP4b D1. Same
/// old-plus-new union as [`patch_grant`], and the same reason: an `explain`
/// reason carries the `rule_id` a UI links from.
#[utoipa::path(
    patch,
    path = "/api/control-plane/perm/rules/{id}",
    tag = "perm",
    params(("id" = Uuid, Path, description = "Rule id")),
    request_body = NewRule,
    description = "Replace a rule's body, keeping its id, and recompile the UNION of the \
                   profiles it affected before and after the change. A patch that turns a path \
                   selector into a tag selector (or back) changes that set by the whole \
                   installation, not by one profile.\n\n\
                   **The body is the WHOLE rule, exactly the create route's shape.** This \
                   replaces the rule and keeps only its id; a field left out is cleared, not \
                   preserved — including `exempt`, where an omitted set means no exemptions \
                   at all.",
    responses(
        (status = 200, description = "Rule updated AND every affected profile recompiled, in one transaction", body = CompileResponse),
        (status = 400, description = "The patched rule cannot be compiled for a profile it affects", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such rule", body = ErrorBody),
        (status = 409, description = "The rule changed concurrently and now affects a profile this request did not lock; retry", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn patch_rule(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<NewRule>,
) -> Result<Json<CompileResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let (_, compile) = write_rule(&mut tx, Some(id), body)
        .await?
        .into_compile_response();
    tx.commit().await.map_err(PermHttpError::db)?;
    Ok(Json(compile))
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
// preview (the dry run)
// ---------------------------------------------------------------------------

/// Name the profiles in `service_ids`, in the order they were locked.
async fn name_profiles(
    tx: &mut Transaction<'_, Postgres>,
    service_ids: &[i64],
) -> Result<Vec<AffectedProfile>, PermHttpError> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM policy.authz_services WHERE id = ANY($1) ORDER BY id")
            .bind(service_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| AffectedProfile { id, name })
        .collect())
}

/// **The dry run — SP4b D2. It runs the REAL write and rolls it back.**
///
/// The blast radius of a write to this model is not local: a tag rule is
/// catalog-unscoped by design, so ONE rule mutation recompiles every Trino and
/// StarRocks profile an engine binds in the installation — and because a
/// compile failure anywhere rolls the whole write back, the blast radius is
/// also a failure radius. A steward who cannot see that before pressing the
/// button learns it from a 400 naming a profile they have never heard of.
///
/// **Why the real write and not a read-only computation of the affected set.**
/// A second, read-only implementation of "which profiles does this touch" is a
/// second answer to the same question, and two answers drift. A preview that
/// lies is worse than no preview, because the steward would trust it. So this
/// handler calls exactly the functions `create_grant`/`patch_grant` call, on a
/// transaction it then ROLLS BACK: the affected set it reports is the set the
/// write computes, the compile it reports is the compile the write runs, and
/// the error it returns is the error the write returns, because in every case
/// it is the same code. `preview_and_write_agree_about_a_tag_rules_fan_out` in
/// `tests/routes.rs` is what keeps that property from being merely intended.
///
/// Gated `policy:update`, not `policy:read`: it previews a write, it runs real
/// compiler work under real locks, and it is useless to someone who cannot
/// write.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/preview",
    tag = "perm",
    request_body = PreviewRequest,
    description = "Dry-run a grant or rule write: report the policy profiles it would recompile, \
                   and the compile error it would fail with, without storing anything. Runs the \
                   real write in a transaction that is rolled back, so the answer cannot differ \
                   from what the write would do.",
    responses(
        (status = 200, description = "The profiles the write would touch, and what its recompile would do", body = PreviewResponse),
        (status = 400, description = "The same refusal the write itself would have returned", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "Previewing a patch of an id that does not exist", body = ErrorBody),
        (status = 409, description = "The previewed row changed concurrently — the same refusal the write would have given", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn preview(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let PreviewRequest { id, grant, rule } = body;

    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    // Every exit from here rolls back: the explicit `rollback()` on success, and
    // dropping `tx` on any `?`. Nothing below commits.
    let outcome = match (grant, rule) {
        (Some(g), None) => write_grant(&mut tx, id, g).await?,
        (None, Some(r)) => write_rule(&mut tx, id, r).await?,
        _ => {
            return Err(PermHttpError::BadRequest(
                "a preview must name exactly one of `grant` or `rule`".into(),
            ))
        }
    };
    let affected = name_profiles(&mut tx, &outcome.services).await?;
    let (_, compile) = outcome.into_compile_response();
    tx.rollback().await.map_err(PermHttpError::db)?;

    Ok(Json(PreviewResponse { affected, compile }))
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

    // SP4a Task 15 (D3 defect 2): resolve the subject's CURRENT workspace
    // memberships from the same table the compiler's own `workspace_members`
    // expansion reads, rather than trusting a caller-supplied list — see
    // `ExplainSubject`'s doc comment for what trusting one instead cost.
    let workspaces = crate::members::workspaces_for_username(&mut conn, &body.subject.user).await?;
    // SP4c Task 7: the same, for the roles a clearance is checked against. A
    // `roles` in the request body is now an unknown field and is ignored, not
    // honoured — the live gate measured that honouring one answered `allowed`
    // for the very user both engines refuse.
    let roles = crate::members::roles_for_username(&mut conn, &body.subject.user).await?;

    // SP4c Task 4/D4: the clearance gates `evaluate` must agree with the
    // compiled engine about — only tags whose `required_roles` is non-empty,
    // since a `required_groups`-only gate compiles to no engine deny at all
    // (D2/D7) and reporting a denial for one here would disagree with the
    // engine in the direction opposite the bug this task fixes.
    let clearances: Vec<ClearanceGate> = clearance_gates_for_tags_via(&mut conn, &tags)
        .await?
        .into_iter()
        .map(|(tag, required_roles)| ClearanceGate {
            tag,
            required_roles,
        })
        .collect();

    let subject = Subject {
        user: body.subject.user,
        groups: body.subject.groups,
        roles,
        workspaces,
    };
    let decision = evaluate(
        &subject,
        body.action,
        &body.target,
        &grants,
        &rules,
        &tags,
        &clearances,
    );
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
