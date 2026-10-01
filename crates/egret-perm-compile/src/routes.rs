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

use std::collections::BTreeSet;
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
    create_grant_in_tx, create_rule_in_tx, delete_grant_in_tx, delete_rule_in_tx, engine_accesses,
    get_grant_via, get_rule_via, list_grants_via, list_rules_via, update_grant_in_tx,
    update_rule_in_tx, Action, Effect, EngineType, Grant, GrantOrigin, PermStore, PrincipalKind,
    PrincipalSet, Rule, RuleKind, Scope, Selector,
};
use egret_policy::{
    catalog_names_for_ids_via, catalogs_for_service_via, clearance_gates_for_tags_via,
    get_service_by_id_via, services_for_catalog_via, tag_keys_by_ids_via,
    tag_keys_for_resource_via, tag_names_for_keys_via, PolicyStore, ServiceRow,
};
use egret_policy_api::ServicePolicies;
use egret_ranger_eval::eval::{AccessRequest, Decision, EngineConfig};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;
use sqlx::{Postgres, Transaction};

use crate::attribution::{attribute_policy_name, PolicyAttribution};
use crate::explain_wire::{WireDecision, WireEngineType};
use crate::grants::{resource_level_names, validate_grant_shape};
use crate::rules::validate_rule_shape;
use crate::trigger::{
    affected_services_for_grant, affected_services_for_rule, engine_type_for, lock_services_in_tx,
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
        ExplainOutcome,
        ExplainProfile,
        ExplainAccess,
        ExplainAttribution,
        // SP4d-2a Task 5: the Task 4 wire mirrors `ExplainResponse` can
        // actually carry, registered here (the route crate owns the document)
        // rather than in `explain_wire` itself. Every one of these is reachable
        // from `WireDecision`, which is why they are listed: `WireDecision`'s
        // `Undecidable` arm carries `WireUndecidableCause`, which carries
        // `WireUnmodelled`/`WireServiceDefIssue`, which carry the four leaf
        // enums. The mask and row-filter mirrors in that module
        // (`WireMaskDecision`, `WireRowFilterDecision`, and their results) are
        // deliberately NOT registered: no response shape carries one yet, and a
        // registered-but-unreachable schema is a contract claim nothing
        // honours. Register them in the same breath as the field that returns
        // one.
        crate::explain_wire::WireDecision,
        crate::explain_wire::WireDenyReason,
        crate::explain_wire::WireUndecidableCause,
        crate::explain_wire::WireUnmodelled,
        crate::explain_wire::WireItemBucket,
        crate::explain_wire::WireTagPolicyBlock,
        crate::explain_wire::WireServiceDefIssue,
        crate::explain_wire::WirePolicyType,
        crate::explain_wire::WireGraphProblem,
        crate::explain_wire::WireEngineType,
        crate::explain_wire::WirePluginVersion,
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
/// **It carries a `user` and nothing else**, and each of the three fields it
/// has lost was removed — never merely ignored — for one version of one
/// reason: a caller-supplied value cannot be allowed to stand in for a fact
/// only egret can verify.
///
/// **No `workspaces` field (SP4a Task 15, D3 defect 2).** It used to be
/// caller-supplied, `#[serde(default)]` to `vec![]`, and the retired model
/// evaluator trusted it verbatim — so a caller naming just a `user` (the
/// natural way to call this endpoint) silently got `denied` for every
/// `workspace_members` grant, however real the compiled Ranger policy made
/// that access; the live gate measured exactly that. SP4a's fix was for
/// `explain` to resolve the memberships itself. Under SP4d-2a there is nothing
/// left to resolve: a `workspace_members` grant reaches the artifact already
/// expanded into usernames by `crate::members::expand_enumerated_principals`,
/// so the subject's memberships are not an input to the answer at all.
///
/// **And no `roles` field either (SP4c Task 7 — the same defect, on the field
/// next to it).** The SP4c live gate
/// (`docs/en/plans/2026-09-28-sp4c-live-result.md`) measured both directions of
/// it: naming only a `user` returned `clearance_required` for the subject the
/// engine had just served the classified column to, and
/// `{"user":"analyst","roles":["sp4c_cleared"]}` returned `allowed` for the
/// subject the engine refuses — a caller asserting a role it does not hold was
/// believed, so the diagnostic could be told a lie and would repeat it. SP4c
/// fixed it by reading the subject's roles from `access_grants`. SP4d-2a keeps
/// the same rule by a stronger means: the evaluator is sent NO roles at all
/// (see below), so there is nothing to assert and nothing to filter. **This is
/// not a "what-if" endpoint**: D9 (SP4b) defines it as answering why a subject
/// **is** allowed or refused, and a field that let a caller invent the
/// subject's clearance would answer a different question while looking
/// authoritative.
///
/// **And no `groups` field either, as of SP4d-2a Task 5 — the third field
/// removed from this struct for the third version of one reason.** Until this
/// task `groups` was caller-supplied and matched by the model evaluator, on
/// the argument that the UI said so. `explain` now answers with the artifact
/// the engine downloads, and `roles` and `groups` reach that evaluator EMPTY:
/// egret serves no `/service/roles/download` and runs no usersync, so a Ranger
/// policy item naming a role or a group matches nobody on any engine egret
/// provisions (D0/D7), and Ranger's `public` — the one group name that DOES
/// match everybody — matches whatever the request carries. A field whose value
/// can change no answer is a field that lies about what the endpoint does, so
/// it is gone rather than ignored, exactly as `workspaces` and `roles` were.
///
/// Sending no roles at all is also the strongest available form of SP4c's rule
/// that a caller may not assert a role the subject does not hold: there is no
/// role to assert. A request that still carries `groups` is accepted and the
/// key ignored (nothing here sets `deny_unknown_fields`), so an older client
/// keeps working while answering the same question everyone else gets.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainSubject {
    pub user: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainRequest {
    pub subject: ExplainSubject,
    pub action: Action,
    pub target: Scope,
}

/// Which of `explain`'s two shapes a response carries.
///
/// **"No engine serves this catalog" is not a refusal.** Collapsing it into a
/// `Denied` would tell a steward the model is withholding an access when in
/// fact no plugin is ever consulted about the resource — nothing is enforcing,
/// which is a different fact. `profiles` is empty for exactly this outcome,
/// and this discriminator exists so a client never has to infer a
/// security-relevant fact from the absence of data (an empty list also reads
/// as "we did not look").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExplainOutcome {
    /// At least one policy profile serves the target's catalog; `profiles`
    /// carries one block per profile.
    PerProfile,
    /// Nothing is enforcing. Reached when `services_for_catalog_via` names no
    /// profile at all, when every profile it names is one this compiler does
    /// not target (a tag service, say), or when no profile it names has the
    /// catalog in its own `catalogs_for_service_via` map — the last is the
    /// inverse of the first query over the same attachments, so in practice it
    /// only differs under a concurrent detach.
    NoProfileServesTheCatalog,
}

/// The `perm` model object the DECIDING policy's Ranger NAME maps back to —
/// [`crate::attribute_policy_name`], resolved per decision.
///
/// **`None` on the owning [`ExplainAccess`] is a real answer, not a gap.**
/// Four live categories of policy have no model object at all — the
/// `egret-admin`/`all` hierarchies auto-bind seeds onto every generated
/// profile, `egret-engine-mgmt`'s delegator plumbing, operator-written raw
/// JSON, and Ranger's own `superuser` short-circuit — and every one of them
/// can decide a real request. They are reported by their Ranger name and id on
/// the decision itself and never dropped: dropping what egret's compiler does
/// not own is exactly the blindness this endpoint was rerouted to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExplainAttribution {
    /// `perm-grant-{uuid}`.
    Grant { grant_id: Uuid },
    /// `perm-rule-{uuid}`, or `perm-rule-{uuid}-{locator}` for StarRocks'
    /// per-column expansion, where ONE `perm.rules` row becomes N policies and
    /// the locator is the only thing that says which expansion decided.
    Rule {
        rule_id: Uuid,
        locator: Option<String>,
    },
    /// `perm-clearance-{tag_id}[-{locator}]`. A clearance has no Ranger
    /// analogue on the wire, so `tag` and `required_roles` are reconstructed
    /// here from the `tags` row the name points at (`tags.key`, not
    /// `tags.name`) and from `classification_policies`. `tag` is `None` when
    /// the tag row is gone — a compiled clearance can outlive its gate until
    /// the next recompile, and inventing a name for it would be worse than
    /// saying none is known.
    ///
    /// **`required_roles` is `None` when no gate could be looked up, and that
    /// is why it is an `Option` rather than a list.** The gate is keyed by
    /// `tags.key`, so when the `tags` row is gone there is no key to look one
    /// up by and nothing is known about the clearing roles. An empty list
    /// would state the opposite — that the gate names no role — which SP4d-2a
    /// measured live and found false: the gate declared `['sp4d2a_cleared']`
    /// and the page said it declared only groups. `None` makes "we could not
    /// look" unrepresentable as "there is nothing to find".
    ///
    /// **This is an honest 3→2, not 3→3.** `Some([])` still carries two facts
    /// this wire cannot separate: (1) the gate declares only
    /// `required_groups`, which compiles to no engine deny at all (D2/D7), and
    /// (2) there is no `classification_policies` row for the tag at all —
    /// `clearance_gates_for_tags_via` filters on
    /// `cardinality(cp.required_roles) > 0` and `clearing_roles` turns a miss
    /// into an empty list, so a DELETED gate whose compiled clearance
    /// policy is still being served reads exactly like a groups-only one.
    /// Separating those two needs a second query that distinguishes "no row"
    /// from "row with no roles"; until something asks for it, the page says
    /// both out loud.
    Clearance {
        tag_id: Uuid,
        tag: Option<String>,
        /// `None` = no gate was looked up because `tag` is `None`.
        /// `Some(roles)` = the lookup ran, and an empty `roles` is a fact
        /// about the gate rather than about the lookup. `attribution_for` is
        /// its only producer, via `clearing_roles`.
        required_roles: Option<Vec<String>>,
        locator: Option<String>,
    },
}

/// One Ranger ACCESS TYPE's verdict inside one profile.
///
/// **A `write` explain is several of these, and they are never collapsed.**
/// Ranger has no `write`: `AccessRequest.access_type` is one string, and
/// `egret_perm::engine_accesses` maps `write` to `insert,delete` on Trino and
/// `insert,update,delete` on StarRocks. An operator who can insert but not
/// delete has to be able to see that, which one merged verdict cannot say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ExplainAccess {
    /// The Ranger access type evaluated, verbatim — `select`, `insert`,
    /// `update`, `delete`.
    pub access_type: String,
    /// What `egret-ranger-eval` answered for the artifact this profile serves.
    /// Three-valued on purpose: `Undecidable` names what stands between the
    /// evaluator and a defensible answer rather than guessing in either
    /// direction.
    pub decision: WireDecision,
    /// The deciding policy's `perm` model object, or `None` — see
    /// [`ExplainAttribution`]. Always `None` for `Undecidable`, which has no
    /// single deciding policy.
    pub attribution: Option<ExplainAttribution>,
}

/// One policy profile's answer: the artifact ITS plugin downloads, evaluated
/// for its own engine.
///
/// Per profile rather than once overall because a single answer cannot be
/// right for two: the same `perm` grant compiles into two different policy
/// sets, the two engines do not share the resource-level vocabulary the
/// request is built from (`schema` vs `database`), and their access-type
/// vocabularies are only partly the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ExplainProfile {
    /// `policy.authz_services.id`.
    pub service_id: i64,
    /// `policy.authz_services.name` — the name the plugin polls
    /// `/service/plugins/policies/download/{service}` with.
    pub service_name: String,
    pub engine: WireEngineType,
    pub accesses: Vec<ExplainAccess>,
}

/// What `explain` answers with: one verdict block per profile serving the
/// target's catalog, over the SERVED ARTIFACT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ExplainResponse {
    pub outcome: ExplainOutcome,
    /// Empty exactly when `outcome` is `no_profile_serves_the_catalog`.
    pub profiles: Vec<ExplainProfile>,
    /// The tag keys **egret** resolved as attached to `target`, for the
    /// steward's information only.
    ///
    /// **This is no longer an input to anything, and it is not a complete tag
    /// set.** It used to be what the retired model evaluator matched rule
    /// selectors against. Today the engine's own tags come from
    /// `RangerTagEnricher`, which walks the resource hierarchy and emits
    /// ANCESTOR tags, while `tag_keys_for_resource_via` matches
    /// schema/table/column by exact equality with no roll-up. So this field
    /// answers "what has egret got recorded on exactly this resource" — useful
    /// when asking why a mask did or did not apply — and deliberately does NOT
    /// reach `AccessRequest::resolved_tags`, whose narrowing would be unsound
    /// given a set narrower than the enricher's. See the handler.
    pub tags: Vec<String>,
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
            let Some(old) = get_grant_via(tx, id).await? else {
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
            (affected_services_for_grant(tx, &old).await?, old.origin)
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

    let new_services = affected_services_for_grant(tx, &g).await?;
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
    let Some(current) = get_grant_via(tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let now = affected_services_for_grant(tx, &current).await?;
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
    let Some(current) = get_rule_via(tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let now = affected_services_for_rule(tx, &current).await?;
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
            let Some(old) = get_rule_via(tx, id).await? else {
                return Err(PermHttpError::NotFound);
            };
            affected_services_for_rule(tx, &old).await?
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

    let new_services = affected_services_for_rule(tx, &r).await?;
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
///
/// # What this answers with, and why it changed (SP4d-2a)
///
/// Until SP4d-2a this handler evaluated egret's OWN permission model
/// (`egret_perm::eval::evaluate`) and never read `policy.authz_policies` at
/// all. The engine evaluates the compiled artifact it downloads from egret.
/// Those were two implementations of one question, and they had already been
/// measured disagreeing in production — a predecessor package caught `explain`
/// reporting a decision the engine did not make. So there is now ONE answer
/// and it is the engine's: the disagreement class disappears instead of
/// becoming visible.
///
/// It also removes a real blindness. Auto-bind seeds thirteen `egret-admin`
/// hierarchies granting `*` on everything onto every generated profile
/// (`egret_policy::default_policies`), and the model-only evaluator could not
/// see one of them: an operator asking "why can this person read that?" got
/// `no_grant_matched` while the engine allowed.
///
/// Four properties of the implementation below are load-bearing:
///
/// 1. **The artifact is assembled by [`artifact_for`], which mirrors
///    `download_policies` field for field and loads each profile's policy set
///    through `enabled_policies_for_service` — the same assembler the download
///    route uses, not `list_policies`.** "What explain evaluated" and "what the
///    engine received" must not be able to drift.
/// 2. **`roles` and `groups` are empty** — see [`ExplainSubject`].
/// 3. **`resolved_tags` is `None`, deliberately.** See the block at the call
///    site: egret cannot establish the antecedent the narrowing needs, so a
///    profile whose `tagPolicies` block could decide comes back `Undecidable`
///    naming that block, rather than answering confidently from a tag set
///    narrower than `RangerTagEnricher`'s.
/// 4. **A `write` explain is several evaluations**, one per Ranger access type
///    the action maps to on that engine, reported separately.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/explain",
    tag = "perm",
    request_body = ExplainRequest,
    responses(
        (status = 200, description = "One verdict per profile over the artifact each engine downloads", body = ExplainResponse),
        (status = 400, description = "The target skips a resource level (a column implies a table implies a schema)", body = ErrorBody),
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
    // **A target that skips a level is refused here, and it has to be refused
    // rather than answered** (whole-branch review, I-4 — fail-open).
    // `request_resource` stops at the first absent level, so
    // `{schema: None, table: Some(_)}` builds the resource `{catalog}` alone:
    // the caller asks about a table and the evaluator answers about its
    // catalog. That is not a harmless approximation — a catalog-wide grant
    // matches `{catalog}` as `SELF_AND_ALL_DESCENDANTS` (an allow) while the
    // table-level clearance deny that would have decided the real target is
    // only a `DESCENDANT` match, which the allow test does not admit. So the
    // silently-substituted answer is **Allowed, with full confidence**.
    //
    // Nothing upstream stops it: `ScopePicker` narrows progressively so the
    // page cannot send this, but the level-skip rule lives in a CHECK on the
    // STORED `perm.grants`/`perm.rules` rows and an `explain` body is never
    // stored.
    if body.target.schema.is_none() && (body.target.table.is_some() || body.target.column.is_some())
        || body.target.table.is_none() && body.target.column.is_some()
    {
        return Err(PermHttpError::BadRequest(
            "a target cannot skip a level: a column implies a table implies a schema. \
             An engine request names exactly the levels the query touched, so a gap \
             names no resource any engine could ask about — and answering it would \
             answer about the nearest named ancestor instead"
                .into(),
        ));
    }
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;

    // Reported for the steward, and NOT passed to the evaluator — see
    // `ExplainResponse::tags` and the `resolved_tags` block below. Tags attach
    // to tables and columns only, keyed `(warehouse_id, schema, table, column)`
    // with `column = ''` meaning the table itself, so a target that names no
    // table can carry none.
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

    let mut profiles: Vec<ExplainProfile> = Vec::new();
    for service_id in services_for_catalog_via(&mut conn, body.target.catalog_id).await? {
        let svc = get_service_by_id_via(&mut conn, service_id).await?;
        // A profile this compiler does not target (a tag service, anything
        // else) has no engine whose plugin downloads it as a RESOURCE policy
        // set, so there is no per-engine answer to give for it. Same
        // classification the compiler itself uses, so the two cannot disagree
        // about which profiles are in scope.
        let Some(engine) = engine_type_for(&svc.r#type) else {
            continue;
        };
        // The compiler's own reachability test: a catalog absent from this map
        // is out of scope for this profile, and a request naming it would be
        // built from a catalog name this profile's policies never mention.
        let Some(catalog_name) = catalogs_for_service_via(&mut conn, service_id)
            .await?
            .into_iter()
            .find(|c| c.id == body.target.catalog_id)
            .map(|c| c.name)
        else {
            continue;
        };

        let artifact = artifact_for(&st.policy, &svc).await?;
        // `ServiceDef::parse` of the envelope's OWN `serviceDef`, not of a def
        // looked up separately: the plugin evaluates against the def it was
        // handed, and the tag block's def (which the tag-policy tier check also
        // consults) travels inside the same envelope.
        let service_def = ServiceDef::parse(&artifact.service_def);
        let resource = request_resource(engine, &catalog_name, &body.target);
        // `EngineConfig::without_super_users()` records a fact about egret's
        // own provisioning, and an unknown super-user list would make every
        // decision unknowable (`isSuperUser` is checked before any policy and
        // wins with `policyPriority = Integer.MAX_VALUE`).
        //
        // **The source is the templating, not only the live pods.**
        // `docs/en/plans/2026-09-28-sp4d-plugin-version-result.md` measured two
        // live TRINO coordinators and says so explicitly — it did not measure
        // StarRocks, and it flags that it never read the templating source, so
        // on its own it does not license this value for a StarRocks profile.
        // It does license it here because the renderers themselves were read:
        // `egret_provisioner::trino`'s `ranger-trino-security.xml` and
        // `egret_provisioner::starrocks`'s `ranger-starrocks-security.xml` are
        // both written whole, property by property, and NEITHER emits
        // `ranger.plugin.super.users` or `.super.groups` — there is no
        // passthrough that could add one. The other half of the super-user
        // question, a `serviceConfig` map inside the DOWNLOAD, cannot arise
        // either: `ServicePolicies` has no such field, so the envelope
        // `artifact_for` builds never carries one, and
        // `Unmodelled::ServiceConfig` would refuse the decision if it did.
        //
        // The one field added on top is `engine`, which arms the def
        // cross-check:
        // Trino's middle resource level is `schema` and StarRocks' is
        // `database`, so pairing the wrong def with an engine answers "nothing
        // matched" with full confidence, and declaring the engine turns that
        // silent wrong answer into `UndecidableCause::EngineMismatch`.
        // ⚠ `plugin_version` is DECLARED, and this comment used to say the
        // opposite: *"stays `None` — `EngineType::plugin_version` is already the
        // measured value, so declaring it would only cross-check it against
        // itself."* That was true while the field's only reader was the
        // cross-check. It is not any more.
        //
        // SP4d-2b Task 5 measured a shape the two plugin versions answer
        // DIFFERENTLY — a tag DENY plus a strictly higher-priority resource ALLOW
        // with no deny items is `Allowed` on 2.8.0 and `Denied` on 2.5.0 — so
        // `evaluate` now reads `plugin_version`, and `None` there costs an ACCESS
        // answer (`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`).
        // Leaving it unset would refuse that shape on BOTH engines, including the
        // one whose answer this crate reproduces exactly. The value is
        // `EngineType::plugin_version`, which is the measured version per engine,
        // so the cross-check still passes trivially — that was never the point of
        // setting it.
        let config = EngineConfig {
            engine: Some(ranger_engine(engine)),
            plugin_version: Some(ranger_engine(engine).plugin_version()),
            ..EngineConfig::without_super_users()
        };

        let mut accesses = Vec::new();
        for access_type in engine_accesses(body.action, engine)? {
            let request = AccessRequest {
                user: body.subject.user.clone(),
                // **Both empty, and that is the answer, not a shortcut.** See
                // `ExplainSubject`: no engine egret provisions can match a
                // Ranger `roles` or `groups` entry, so sending either would
                // report an access no engine grants.
                groups: BTreeSet::new(),
                roles: BTreeSet::new(),
                access_type: access_type.to_string(),
                resource: resource.clone(),
                // egret does not track a table's Ranger `{OWNER}`. `None` makes
                // a decision that depends on one `Undecidable` rather than a
                // guess in either direction.
                owner: None,
                // ⚠ **`None`, and it must stay `None` until egret's tag
                // resolution is hierarchy-complete.** `Some(empty)` is a
                // licence for `egret-ranger-eval` to skip its `tagPolicies`
                // refusal, and that licence is sound only if the caller's tag
                // resolution is at least as broad as
                // `RangerTagEnricher`'s. egret's is not:
                // `tag_keys_for_resource_via` matches schema/table/column by
                // exact equality with no hierarchy roll-up, while the enricher
                // walks the hierarchy and emits ancestor tags Ranger ADMITS for
                // a tag request. So egret can report "no tags" for a column the
                // engine is masking through a TABLE-level attachment. Every
                // live tag assignment today is column-level, which is exactly
                // how passing `tags` here would ship green and fail open. The
                // cost of refusing is bounded and visible: a profile whose tag
                // block carries a policy comes back `Undecidable` with the
                // block and its policy COUNT named
                // (`tests/routes.rs::a_tag_policy_block_that_could_decide_makes_the_profile_undecidable`
                // pins it), which a steward can act on. Passing the existing
                // resolution's result as if it were complete is the one thing
                // that is not an option.
                resolved_tags: None,
            };
            let decision =
                egret_ranger_eval::eval::evaluate(&artifact, &request, &service_def, &config);
            // `&mut *conn` and not `&mut conn`: `attribution_for` is generic
            // over `ClearanceLookups`, which `sqlx::PgConnection` implements,
            // and a `PoolConnection` does not — inference needs the deref.
            let attribution = attribution_for(&mut *conn, &decision).await?;
            accesses.push(ExplainAccess {
                access_type: access_type.to_string(),
                decision: WireDecision::from(&decision),
                attribution,
            });
        }

        profiles.push(ExplainProfile {
            service_id,
            service_name: svc.name,
            engine: WireEngineType::from(ranger_engine(engine)),
            accesses,
        });
    }

    let outcome = if profiles.is_empty() {
        ExplainOutcome::NoProfileServesTheCatalog
    } else {
        ExplainOutcome::PerProfile
    };
    Ok(Json(ExplainResponse {
        outcome,
        profiles,
        tags,
    }))
}

/// `egret_perm::EngineType` → the evaluator's own. Two enums for one fact
/// because `egret-ranger-eval` is a pure crate that depends on neither
/// `egret-perm` nor `egret-policy`; this is the single crossing.
fn ranger_engine(engine: EngineType) -> egret_ranger_eval::eval::EngineType {
    match engine {
        EngineType::Trino => egret_ranger_eval::eval::EngineType::Trino,
        EngineType::StarRocks => egret_ranger_eval::eval::EngineType::StarRocks,
    }
}

/// The resource ONE access request names, in the levels the target engine's
/// service-def declares (`resource_level_names` — Trino's middle level is
/// `schema`, StarRocks' is `database`).
///
/// **A level the target does not name is ABSENT, not `*`.** That is what an
/// engine's own request looks like: the plugin names exactly the levels the
/// query touched. It is also not the same thing as the compiler's `*`, which
/// is a POLICY value meaning "every child at this level" — a policy naming
/// `catalog/*/*/*` against a request naming only `catalog` is Ranger's
/// `SELF_AND_ALL_DESCENDANTS`, which matches; a request padded with `*` up to
/// the leaf would be asking about a different resource.
fn request_resource(engine: EngineType, catalog_name: &str, target: &Scope) -> RequestResource {
    let names = resource_level_names(engine);
    let mut resource = RequestResource::new();
    resource.insert(names[0], catalog_name);
    for (name, value) in [
        (names[1], target.schema.as_deref()),
        (names[2], target.table.as_deref()),
        (names[3], target.column.as_deref()),
    ] {
        match value {
            Some(v) => resource.insert(name, v),
            // Stopping at the first absent level is the shape an engine
            // request has, and it is only correct because a gap below a named
            // level cannot reach here: `explain` refuses
            // `{schema: None, table: Some(_)}` with a 400 at the top of the
            // handler, and `ScopePicker` narrows progressively so the page
            // never sends one.
            //
            // **The guard is the handler's, not `validate_grant_shape`'s.**
            // This comment used to cite that function, and the citation was
            // wrong in a way that mattered: `validate_grant_shape` validates
            // PRINCIPALS only, and the real level-skip rule is a CHECK on the
            // stored `perm.grants`/`perm.rules` rows
            // (`migrations/0154_perm_model.sql`), which an unstored request
            // body never passes through. Breaking here under a gap answers
            // about the nearest named ANCESTOR — fail-open, see the handler.
            None => break,
        }
    }
    resource
}

/// Assemble the policy download `svc`'s plugin would receive, and read it back
/// through `egret-ranger-eval`'s own reader.
///
/// **This mirrors `egret_policy::ranger::api::download::download_policies`
/// deliberately and completely**, `tagPolicies` block included, and builds it
/// with the same `ServicePolicies` type that handler serialises rather than a
/// second hand-written copy of the envelope shape. The point of routing
/// `explain` through the evaluator is that what it evaluated and what the
/// engine received cannot differ; a private envelope builder here would be a
/// second thing to keep in step, and the day they diverged `explain` would
/// answer confidently about a download nobody serves. In particular the
/// policies come from `enabled_policies_for_service` (which injects the
/// DB-assigned `id`/`version`/`isEnabled` the plugin needs and drops disabled
/// rows), NOT from `list_policies`.
///
/// The one thing it does not reproduce is the `lastKnownVersion` 304
/// short-circuit, which is a caching protocol and carries no policy content.
async fn artifact_for(
    policy: &PolicyStore,
    svc: &ServiceRow,
) -> Result<ServicePolicySet, PermHttpError> {
    let def = policy.get_service_def(&svc.r#type).await?.def_jsonb;
    let policies = policy.enabled_policies_for_service(svc.id).await?;
    let tag_service = svc
        .config_jsonb
        .get("tagService")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let mut envelope = ServicePolicies::new(
        svc.name.clone(),
        svc.id,
        svc.policy_version,
        def,
        policies,
        None,
    );
    // Best-effort exactly as the download route is: a `tagService` naming
    // nothing just omits the block, because that is what the plugin would
    // receive.
    if let Some(name) = tag_service {
        if let Ok(tsvc) = policy.get_service(&name).await {
            let tdef = policy.get_service_def(&tsvc.r#type).await?.def_jsonb;
            let tpolicies = policy.enabled_policies_for_service(tsvc.id).await?;
            envelope.tag_policies = Some(Box::new(ServicePolicies::new(
                tsvc.name,
                tsvc.id,
                tsvc.policy_version,
                tdef,
                tpolicies,
                None,
            )));
        }
    }
    // Through JSON on purpose: the plugin reads JSON, and
    // `ServicePolicySet`'s `Deserialize` is where every `Unmodelled` construct
    // in the download is detected. Going round the serialiser would skip the
    // detection pass that makes the refusals possible.
    let wire = serde_json::to_value(&envelope).map_err(PermHttpError::db)?;
    serde_json::from_value::<ServicePolicySet>(wire).map_err(PermHttpError::db)
}

/// Map the DECIDING policy's Ranger name back to its `perm` model object, if
/// it has one.
///
/// `Undecidable` has no single deciding policy, so it attributes to nothing —
/// the causes in `why` name the policies they are about themselves. And a
/// policy egret's compiler does not own attributes to nothing either, which is
/// a real answer: see [`ExplainAttribution`].
///
/// **Generic over its lookups so this function — the WIRING — is testable with
/// no database.** A pure `clearing_roles` covers the decision, but fix round 1
/// measured that it left the call site uncovered: substituting
/// `clearing_roles(None, &gates)` here passed 10/0, because the only test that
/// reached this function was DB-gated. `ClearanceLookups` is the seam that
/// closes it; `sqlx::PgConnection` is the production implementation and the
/// tests at the bottom of this file hand it a fake that records what was asked.
async fn attribution_for<L: ClearanceLookups + Send>(
    conn: &mut L,
    decision: &Decision,
) -> Result<Option<ExplainAttribution>, PermHttpError> {
    let name = match decision {
        Decision::Allowed { policy_name, .. } | Decision::Denied { policy_name, .. } => policy_name,
        Decision::Undecidable { .. } => return Ok(None),
    };
    Ok(match attribute_policy_name(name) {
        None => None,
        Some(PolicyAttribution::Grant(grant_id)) => Some(ExplainAttribution::Grant { grant_id }),
        Some(PolicyAttribution::Rule { id, locator }) => Some(ExplainAttribution::Rule {
            rule_id: id,
            locator,
        }),
        // A clearance names a `tags.id` and nothing else. `tag` and
        // `required_roles` are the two facts a steward needs and neither is on
        // the wire the engine sees, so both are reconstructed here — `tags.key`
        // (not `tags.name`), and the gate's roles keyed by that same key.
        Some(PolicyAttribution::Clearance { tag_id, locator }) => {
            let tag = conn.tag_key(tag_id).await?;
            // The gate is keyed by `tags.key`, so with no key there is no
            // lookup to run. What the result MEANS is `clearing_roles`' job.
            let gates = match &tag {
                Some(key) => conn.gates_for(key).await?,
                None => Vec::new(),
            };
            Some(ExplainAttribution::Clearance {
                tag_id,
                required_roles: clearing_roles(tag.as_deref(), &gates),
                tag,
                locator,
            })
        }
    })
}

/// The two reads a clearance attribution needs, behind a trait so that
/// [`attribution_for`] can be driven with no database.
///
/// Both methods are `-> impl Future + Send` rather than `async fn` because the
/// caller is an axum handler: an `async fn` in a trait yields a future with no
/// `Send` bound, and the handler would stop compiling.
trait ClearanceLookups {
    /// `tags.key` for this `tags.id`, or `None` when the row is gone — a
    /// compiled clearance outlives its tag until the next recompile.
    fn tag_key(
        &mut self,
        tag_id: Uuid,
    ) -> impl std::future::Future<Output = Result<Option<String>, PermHttpError>> + Send;

    /// The clearance gates for one `tags.key`, exactly as
    /// `clearance_gates_for_tags_via` returns them — `(tag key, roles)` pairs,
    /// and empty when the tag has no gate with roles.
    fn gates_for(
        &mut self,
        tag_key: &str,
    ) -> impl std::future::Future<Output = Result<Vec<(String, Vec<String>)>, PermHttpError>> + Send;
}

/// The production implementation: the two store queries, unchanged. `tags.key`
/// (never `tags.name`) is what a gate is keyed by, which is why the first read
/// returns a key rather than a display name.
impl ClearanceLookups for sqlx::PgConnection {
    async fn tag_key(&mut self, tag_id: Uuid) -> Result<Option<String>, PermHttpError> {
        Ok(tag_keys_by_ids_via(self, &[tag_id])
            .await?
            .into_iter()
            .find(|(id, _)| *id == tag_id)
            .map(|(_, key)| key))
    }

    async fn gates_for(
        &mut self,
        tag_key: &str,
    ) -> Result<Vec<(String, Vec<String>)>, PermHttpError> {
        Ok(clearance_gates_for_tags_via(self, &[tag_key.to_string()]).await?)
    }
}

/// What [`ExplainAttribution::Clearance`]'s `required_roles` carries, given the
/// `tags.key` the policy name resolved to (if any) and whatever
/// [`ClearanceLookups::gates_for`] returned.
///
/// **`None` if and only if `tag` is `None`.** That is the guarantee the
/// `Option` exists for, pinned here directly and again through
/// [`attribution_for`], which is where the call site lives.
///
/// `gates` is ignored entirely when `tag` is `None`: a clearance whose tag row
/// is gone must not borrow another tag's gate, which is the one way a wrong
/// `Some([...])` could be invented rather than merely conflated.
fn clearing_roles(tag: Option<&str>, gates: &[(String, Vec<String>)]) -> Option<Vec<String>> {
    let key = tag?;
    // The lookup ran, so an empty result is a fact about the gate rather than
    // about the lookup — either it declares only `required_groups` (which
    // `clearance_gates_for_tags_via` filters out) or its row is already
    // deleted. `ExplainAttribution::Clearance` states that those two stay
    // conflated; what they no longer collide with is "nothing was looked up".
    Some(
        gates
            .iter()
            .find(|(gated, _)| gated == key)
            .map(|(_, roles)| roles.clone())
            .unwrap_or_default(),
    )
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

#[cfg(test)]
mod tests {
    use super::clearing_roles;

    fn gate(key: &str, roles: &[&str]) -> (String, Vec<String>) {
        (
            key.to_string(),
            roles.iter().map(|r| r.to_string()).collect(),
        )
    }

    /// The whole point of the `Option`. SP4d-2a's browser gate deleted a `tags`
    /// row while the compiled clearance policy kept being served; the gate had
    /// declared `['sp4d2a_cleared']`, and the page reported that it declared
    /// only groups — because an empty list was all the wire could say. `None`
    /// is now the only thing a missing key can produce.
    #[test]
    fn no_tag_key_means_the_roles_are_unknown_not_empty() {
        assert_eq!(clearing_roles(None, &[]), None);
    }

    /// `gates` is ignored when there is no key, so a key-less clearance cannot
    /// be handed somebody else's gate. Without this, "looked nothing up" and
    /// "found another tag's roles" are the same code path.
    #[test]
    fn no_tag_key_borrows_no_other_tags_gate() {
        assert_eq!(clearing_roles(None, &[gate("pii", &["pii_cleared"])]), None);
    }

    /// The lookup ran and found the gate: the roles are the answer, verbatim
    /// and in order, and `Some` says the list may be read as a fact.
    #[test]
    fn a_resolved_gate_reports_its_roles() {
        assert_eq!(
            clearing_roles(
                Some("pii"),
                &[
                    gate("other", &["nope"]),
                    gate("pii", &["pii_cleared", "auditor"])
                ]
            ),
            Some(vec!["pii_cleared".to_string(), "auditor".to_string()])
        );
    }

    /// The honest 3→2. The lookup ran and missed, which happens for a
    /// groups-only gate (`clearance_gates_for_tags_via` filters on
    /// `cardinality(required_roles) > 0`) and for a gate row already deleted.
    /// Those two stay conflated in `Some([])` — what matters is that neither
    /// can be confused with `None`.
    #[test]
    fn a_resolved_tag_with_no_matching_gate_reports_an_empty_list_not_none() {
        assert_eq!(clearing_roles(Some("pii"), &[]), Some(Vec::new()));
        assert_eq!(
            clearing_roles(Some("pii"), &[gate("other", &["nope"])]),
            Some(Vec::new())
        );
    }
}

// ── the wiring, with no database ─────────────────────────────────────────
//
// Fix round 1. The four tests above pin the DECISION; these pin the CALL
// SITE, which is the part a surviving mutant found uncovered. `attribution_for`
// is generic over `ClearanceLookups`, so the fake below stands in for the two
// store queries and also RECORDS them — "no key, so no second query" is a
// claim about behaviour, and a test that only looked at the return value could
// not tell a skipped query from one whose result was thrown away.

#[cfg(test)]
mod wiring_tests {
    use super::{attribution_for, ClearanceLookups, ExplainAttribution, PermHttpError};
    use egret_ranger_eval::eval::{Decision, DenyReason};
    use uuid::Uuid;

    /// Stands in for the two store queries, and records what it was asked.
    struct FakeLookups {
        tag: Option<String>,
        gates: Vec<(String, Vec<String>)>,
        tags_asked: Vec<Uuid>,
        gates_asked: Vec<String>,
    }

    impl FakeLookups {
        fn new(tag: Option<&str>, gates: &[(&str, &[&str])]) -> Self {
            Self {
                tag: tag.map(str::to_string),
                gates: gates
                    .iter()
                    .map(|(k, roles)| {
                        (
                            (*k).to_string(),
                            roles.iter().map(|r| (*r).to_string()).collect(),
                        )
                    })
                    .collect(),
                tags_asked: Vec::new(),
                gates_asked: Vec::new(),
            }
        }
    }

    impl ClearanceLookups for FakeLookups {
        async fn tag_key(&mut self, tag_id: Uuid) -> Result<Option<String>, PermHttpError> {
            self.tags_asked.push(tag_id);
            Ok(self.tag.clone())
        }

        async fn gates_for(
            &mut self,
            tag_key: &str,
        ) -> Result<Vec<(String, Vec<String>)>, PermHttpError> {
            self.gates_asked.push(tag_key.to_string());
            // The real query filters by key in SQL; the fake filters here, so a
            // caller that passed the wrong key gets the same empty answer it
            // would get from Postgres rather than a convenient one.
            Ok(self
                .gates
                .iter()
                .filter(|(k, _)| k == tag_key)
                .cloned()
                .collect())
        }
    }

    fn denied(policy_name: &str) -> Decision {
        Decision::Denied {
            policy_id: Some(30),
            policy_name: policy_name.to_string(),
            reason: DenyReason::DenyPolicyItem,
        }
    }

    const TAG_ID: &str = "88888888-8888-4888-8888-888888888888";

    fn clearance_name(locator: Option<&str>) -> String {
        match locator {
            Some(l) => format!("perm-clearance-{TAG_ID}-{l}"),
            None => format!("perm-clearance-{TAG_ID}"),
        }
    }

    /// **This is the test that kills the fix-round-1 survivor.** Substituting
    /// `clearing_roles(None, &gates)` at the call site makes `required_roles`
    /// `null` here while the gate plainly declares a role, so this assertion
    /// fails — with no `DATABASE_URL`, in 0.00s.
    #[tokio::test]
    async fn a_resolved_clearance_carries_the_gates_roles_onto_the_wire() {
        let mut lookups =
            FakeLookups::new(Some("sp4d2a_ssn"), &[("sp4d2a_ssn", &["sp4d2a_cleared"])]);
        let got = attribution_for(&mut lookups, &denied(&clearance_name(None)))
            .await
            .expect("no error");
        assert_eq!(
            got,
            Some(ExplainAttribution::Clearance {
                tag_id: Uuid::parse_str(TAG_ID).unwrap(),
                tag: Some("sp4d2a_ssn".to_string()),
                required_roles: Some(vec!["sp4d2a_cleared".to_string()]),
                locator: None,
            })
        );
        assert_eq!(
            lookups.gates_asked,
            vec!["sp4d2a_ssn".to_string()],
            "the gate must be looked up BY THE KEY the tag resolved to"
        );
    }

    /// The browser-gate state: the `tags` row is gone and the compiled policy is
    /// still served. `required_roles` is `None` — not `[]` — and the second
    /// query is never issued, because there is no key to issue it with.
    #[tokio::test]
    async fn an_unresolved_clearance_sends_null_roles_and_asks_no_gate() {
        let mut lookups = FakeLookups::new(None, &[("sp4d2a_ssn", &["sp4d2a_cleared"])]);
        let got = attribution_for(&mut lookups, &denied(&clearance_name(Some("wh.s.t.ssn"))))
            .await
            .expect("no error");
        assert_eq!(
            got,
            Some(ExplainAttribution::Clearance {
                tag_id: Uuid::parse_str(TAG_ID).unwrap(),
                tag: None,
                required_roles: None,
                locator: Some("wh.s.t.ssn".to_string()),
            })
        );
        assert!(
            lookups.gates_asked.is_empty(),
            "no key means no gate query, not a query with a borrowed key"
        );
    }

    /// The other half of the honest 3→2: the key resolved, the gate query ran
    /// and matched nothing. `Some([])` is a fact about the gate, and it must not
    /// become `None` just because the list is empty.
    #[tokio::test]
    async fn a_resolved_clearance_with_no_gate_sends_an_empty_list() {
        let mut lookups = FakeLookups::new(Some("sp4d2a_ssn"), &[]);
        let got = attribution_for(&mut lookups, &denied(&clearance_name(None)))
            .await
            .expect("no error");
        assert_eq!(
            got,
            Some(ExplainAttribution::Clearance {
                tag_id: Uuid::parse_str(TAG_ID).unwrap(),
                tag: Some("sp4d2a_ssn".to_string()),
                required_roles: Some(Vec::new()),
                locator: None,
            })
        );
        assert_eq!(lookups.gates_asked, vec!["sp4d2a_ssn".to_string()]);
    }

    /// A grant, a rule and an `Undecidable` all reach this function too, and
    /// none of them may touch the clearance lookups — a tag read per verdict on
    /// a profile that has no clearance at all would be pure cost.
    #[tokio::test]
    async fn the_other_attributions_issue_no_clearance_lookups() {
        let grant = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let rule = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        for (decision, want) in [
            (
                denied(&format!("perm-grant-{grant}")),
                Some(ExplainAttribution::Grant { grant_id: grant }),
            ),
            (
                denied(&format!("perm-rule-{rule}-wh.s.t.c")),
                Some(ExplainAttribution::Rule {
                    rule_id: rule,
                    locator: Some("wh.s.t.c".to_string()),
                }),
            ),
            // Ranger's own super-user short-circuit: a real decider with no
            // model object, reported by name on the decision itself.
            (denied("superuser"), None),
            (Decision::Undecidable { why: Vec::new() }, None),
        ] {
            let mut lookups = FakeLookups::new(Some("sp4d2a_ssn"), &[("sp4d2a_ssn", &["r"])]);
            let got = attribution_for(&mut lookups, &decision).await.expect("ok");
            assert_eq!(got, want, "for {decision:?}");
            assert_eq!(lookups.tags_asked, Vec::<Uuid>::new(), "for {decision:?}");
            assert!(lookups.gates_asked.is_empty(), "for {decision:?}");
        }
    }
}
