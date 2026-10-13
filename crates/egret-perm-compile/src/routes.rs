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

use std::collections::{BTreeMap, BTreeSet};
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
    catalog_is_assigned_to_workspace_via, catalog_names_for_ids_via, catalogs_for_service_via,
    clearance_gates_for_tags_via, enabled_policies_for_service_via, get_service_by_id_via,
    get_service_def_via, resolved_tags_for_resource_via, service_id_by_name_via,
    services_for_catalog_via, tag_assignment_coordinates_via, tag_keys_by_ids_via,
    tag_names_for_keys_via, unreadable_table_mask_for, MaskOrigin, PolicyStore, ServiceRow, TagRef,
    TagWrite,
};
use egret_policy_api::ServicePolicies;
use egret_ranger_eval::eval::{
    AccessRequest, Decision, EngineConfig, TagServiceResource, UnenrichedTags,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::{requests_all_values, RequestResource};
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
        .route("/perm/overlaps", get(list_overlaps))
        .route("/perm/preview", post(preview))
        .route("/perm/explain", post(explain))
        .route("/perm/compile/{service_id}", post(compile))
        .route(
            "/perm/policies/services/{service_id}/classify",
            get(classify_service_policies),
        )
        .route(
            "/perm/policies/services/{service_id}/warnings",
            get(service_policy_warnings),
        )
        .route(
            "/perm/policies/services/{service_id}/migrate",
            post(migrate_service_policies),
        )
        .route(
            "/perm/policies/services/{service_id}/migrate/undo",
            post(undo_service_policy_migration),
        )
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
        list_overlaps,
        preview,
        explain,
        compile,
        classify_service_policies,
        service_policy_warnings,
        migrate_service_policies,
        undo_service_policy_migration
    ),
    components(schemas(
        crate::overlap::StoredOverlap,
        crate::overlap::StoredMember,
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
        // SP4d-2b Task 7 fix round 3: reachable from `ExplainResponse::
        // tag_resolution_declined`, which is why they are registered — see the
        // note above about registered-but-unreachable schemas.
        ExplainTagDecline,
        ExplainTargetLevel,
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
        crate::classify::ClassifiedPolicy,
        crate::classify::PolicyWarning,
        crate::classify::ProfileWarnings,
        crate::classify::WarnedPolicy,
        crate::classify::ProposedObjects,
        crate::classify::MigrationRecord,
        crate::migrate::MigrateRequest,
        crate::migrate::MigrateResponse,
        crate::migrate::MigratedPolicy,
        crate::migrate::GatedProfile,
        crate::migrate::WireDisagreement,
        crate::migrate::WireCheck,
        crate::migrate::WireTagging,
        crate::migrate::UndoRequest,
        crate::migrate::UndoResponse,
        crate::migrate::UndonePolicy,
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
    /// The tag set **this profile's** verdicts could have been decided by.
    ///
    /// # ⚠ This lives on the PROFILE, and the move is the fix for three defects at once
    ///
    /// **Whole-branch review, H1 / H2 / H3 — one defect in three costumes: a
    /// value computed once per REQUEST, presented as though it were per-verdict.**
    /// It was `ExplainResponse::tags`, one list for the whole response, and the
    /// page rendered one heading under all profiles. Each costume was measured:
    ///
    /// * **H3, and reachable with no StarRocks, no second profile and no tag
    ///   service at all.** The list was returned even when `profiles` is EMPTY,
    ///   so a catalog nothing serves rendered "Nothing is enforcing" and "Tags
    ///   that decide this request — pii" on one screen, with no verdicts in
    ///   existence. `catalog_tag_assignments` has no catalog column, so a sibling
    ///   catalog's assignment on the same warehouse resolves for this target too.
    /// * **H1.** A catalog served by one profile with a tag service and one
    ///   without made the page say, once, that these tags decide this request —
    ///   false for the second, where nothing tag-related decided anything.
    /// * **H2.** A StarRocks profile given a `tagService` would list tags as
    ///   deciding while `enriched_tags` discards every one of them.
    ///
    /// Fixing the three sentences would have left a fourth instance free to
    /// appear. **With the set on the profile there is nowhere to put a tag list
    /// except beside a verdict**, so `no_profile_serves_the_catalog` cannot carry
    /// one — the payload the H3 probe rendered is now unrepresentable.
    ///
    /// **The F4/F5 property is kept, not traded.** This is still ONE resolution,
    /// `resolved_tags_for_target`'s, computed once before the loop and the same
    /// object every verdict was evaluated against — not a second query. What
    /// differs per profile is only the ONE narrowing this crate can apply
    /// soundly: see below.
    ///
    /// # What the narrowing is, and why it cannot drift from the evaluator
    ///
    /// Empty when [`ExplainProfile::tag_policies_in_download`] is `0`. That is a
    /// ONE-WAY implication, not a reimplementation: with no tag policy of any
    /// kind in the download, the evaluator's `of_kind` is empty for every policy
    /// type and no tag can reach any verdict, so an empty list here cannot
    /// disagree with it. The evaluator's actual per-access-type rule stays the
    /// evaluator's; reproducing it here would be the second-copy shape this
    /// package has refused four times.
    ///
    /// ⚠ **A non-empty list does NOT claim the tags were evaluated** — H2's drop
    /// happens inside `enriched_tags`, against this profile's own service-def,
    /// and is not visible from here. The page's wording is held to that: the set
    /// is what this profile's plugin resolves, and which verdict a tag decided is
    /// on the verdict's own attribution.
    pub tags: Vec<ExplainResolvedTag>,
    /// How many tag policies **this profile's download** carries —
    /// `tagPolicies.policies.len()`, or `0` when there is no tag block at all
    /// because the profile has no bound tag service.
    ///
    /// Carried so the page can say WHY a profile's tag set is empty while the
    /// column is tagged — the question H1's steward is left with otherwise — and
    /// it is the predicate [`ExplainProfile::tags`] is narrowed by.
    pub tag_policies_in_download: usize,
}

/// One tag egret resolved for the target, and **where the attachment that
/// produced it lives** — which is the half of the answer F4 measured missing.
///
/// # ⚠ Why this is a bool and not the evaluator's `MatchType`
///
/// `MatchType` has five variants and `attachment_for` can produce only two:
/// `Self_` for an attachment naming the column, `Ancestor` for one that stops at
/// the table. Putting the enum on the wire would need a word for three states
/// this path cannot reach — and inventing wire vocabulary for an unreachable
/// state is how the `clearance_required` string got rendered raw for two
/// releases.
///
/// So this reads back the field the mapping actually WROTE:
/// `attached_through.levels` is exactly what `attachment_for` built from
/// `from_assignments`' rule, and `column` is in it iff the attachment names the
/// column. One datum, read once — not a second derivation from a parallel
/// source, which is the two-comparisons defect this package has hit four times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ExplainResolvedTag {
    /// `tags.name` — the identifier the tag download carries as the tag's `type`
    /// and therefore the one the engine matches tag policies on. **Not
    /// `tags.key`**, which is what this field used to report and which can
    /// differ: live, the key and the name differ for one of three tags.
    pub name: String,
    /// `true` ⇒ the assignment names this column (Ranger `SELF`). `false` ⇒ it
    /// stops at the table and reaches this resource as an `ANCESTOR`, so the row
    /// to change is the TABLE's, not this column's. For a target that names no
    /// column this is always `false` and says nothing extra — the attachment and
    /// the target are the same resource.
    pub attached_at_column: bool,
}

/// What `explain` answers with: one verdict block per profile serving the
/// target's catalog, over the SERVED ARTIFACT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ExplainResponse {
    pub outcome: ExplainOutcome,
    /// Empty exactly when `outcome` is `no_profile_serves_the_catalog`.
    pub profiles: Vec<ExplainProfile>,
    // ⚠ **There is no response-level tag set, and its absence is a FIX**
    // (whole-branch review H1/H2/H3). `tags` used to live here, computed once per
    // request, and the page rendered one heading under every profile. All three
    // review Importants were that one fact presented as though it were
    // per-verdict — including a catalog nothing serves rendering "Nothing is
    // enforcing" and a tag list on the same screen, with no verdicts in
    // existence. The set now lives on `ExplainProfile::tags`, where a verdict is,
    // so there is nowhere to put a tag list when nothing was evaluated. The whole
    // argument is on that field.
    //
    // `tag_resolution_declined` STAYS here, and the asymmetry is deliberate: a
    // decline is a property of the TARGET and of egret's storage (fix round 3's
    // ruling), true exactly once per request, whereas the set that could decide
    // is per profile. The page renders the decline only inside a profile block,
    // so a response with no profiles shows nothing tag-related either way.
    /// Why `explain` declined to resolve this target's tags — absent when it
    /// resolved them.
    ///
    /// **This exists because the REASON was unreachable (fix round 3, IMPORTANT 1
    /// / LOW 3).** The evaluator's `UndecidableCause::TagsUnresolved` is a unit
    /// variant and says only "the caller did not resolve", which is all the
    /// evaluator can know; `resolved_tags_for_target` declines for more than one
    /// state and the states have different remedies and different REACHES, and
    /// `.is_some()` at the call site threw the distinction away. The page then
    /// rendered round 1's single sentence for every one of them — naming a stored
    /// row that does not exist, and claiming a per-catalog reach for the state that
    /// is per-REQUEST. (There were four states when that was measured; migration
    /// 0177 made two of them unrepresentable and left two.)
    ///
    /// **Carried here rather than as a payload on `TagsUnresolved`**, which was
    /// the other option: `egret-ranger-eval` is a pure crate that knows nothing of
    /// `catalog_tag_assignments`, and the reason is a property of egret's STORAGE
    /// and of this REQUEST, not of the evaluator's input. It is also per
    /// target-and-catalog rather than per access, so the response is where it is
    /// true exactly once. The 1:1 pairing is a property of one function: this is
    /// `Some` exactly when the handler passed `resolved_tags: None`, which is the
    /// only production state in which the evaluator raises `TagsUnresolved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_resolution_declined: Option<ExplainTagDecline>,
}

/// Split one tag resolution into the two things `explain` does with it: the
/// evaluator's `resolved_tags`, and the response's
/// [`ExplainResponse::tag_resolution_declined`].
///
/// # ⚠ Three lines in a function because a mutation survived at this exact spot
///
/// **Fix round 3.** This was inline in the handler, and a mutant that threw the
/// decline away (`Err(_) => (None, None)`) **survived the whole suite** — the
/// handler body is reachable only from DB-gated tests, which do not run on this
/// branch. That is the same finding Task 6 made about `attribution_for` and the
/// same one that moved the stored-coordinate ruling out of SQL: where a mutation cannot
/// reach, the code is in the wrong shape. Pulling the pairing out makes both arms
/// assertable with no database, which is what
/// `the_decline_reason_reaches_the_response_and_the_tags_do_not` does.
///
/// The two outputs are mutually exclusive by construction — a `Result` in, one
/// `Some` out — which is the invariant the page depends on: it renders the reason
/// only beside `UndecidableCause::TagsUnresolved`, and that cause is raised only
/// when `resolved_tags` is `None`.
fn explain_tag_inputs(
    resolution: Result<UnenrichedTags, ExplainTagDecline>,
) -> (Option<UnenrichedTags>, Option<ExplainTagDecline>) {
    match resolution {
        Ok(tags) => (Some(tags), None),
        Err(decline) => (None, Some(decline)),
    }
}

/// Which of the two states made `explain` decline to resolve the target's tags
/// — see [`ExplainResponse::tag_resolution_declined`] and
/// [`stored_coordinate_is_fold_divergent`].
///
/// The two have two different REACHES, which is the whole reason they are carried
/// apart: the first is a property of stored data and holds for every target in the
/// catalog until that row changes; the second is a property of the request and
/// holds for nothing else.
///
/// ⚠ **This carried FOUR variants until SP4d debt Task 3.** `StoredLevelEmpty` and
/// `StoredWildcard` were the other two, and migration 0177's
/// `catalog_tag_assignments_levels_are_present_and_literal` makes both states
/// unrepresentable: no running egret can read such a row, so no response can
/// report one. They are removed rather than left unreachable — an API variant a
/// client must still branch on for a state that cannot occur is the same defect as
/// a superseded comment. The enumeration of what the matcher does and the schema
/// now forbids survives on [`stored_coordinate_is_fold_divergent`], which is where
/// the next person needs it.
///
/// The stored variant carries the offending coordinate so the page can name the
/// row to fix. That is a `policy:read` surface already — the response's `tags`
/// field reports attachments on the target — and naming no row is what produced
/// the measured defect: a sentence telling a steward to re-store an assignment
/// that was already stored on a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExplainTagDecline {
    /// A stored assignment value carries one of the characters on which the two
    /// case folds can disagree — see [`FOLD_DIVERGENT`].
    StoredFoldDivergent {
        schema: String,
        table: String,
        column: String,
    },
    /// The TARGET carries one of those characters. **Per request, not per
    /// catalog** — nothing is stored wrongly and every other target in the catalog
    /// answers normally.
    TargetFoldDivergent {
        level: ExplainTargetLevel,
        value: String,
    },
}

/// Which level of the target [`ExplainTagDecline::TargetFoldDivergent`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExplainTargetLevel {
    Schema,
    Table,
    Column,
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
    pub(crate) fn from_summaries(services: Vec<i64>, summaries: &[CompileSummary]) -> Self {
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
pub(crate) struct WriteOutcome {
    pub(crate) id: Uuid,
    pub(crate) services: Vec<i64>,
    pub(crate) summaries: Vec<CompileSummary>,
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
pub(crate) async fn write_grant(
    tx: &mut Transaction<'_, Postgres>,
    id: Option<Uuid>,
    body: NewGrant,
    p: &egret_core::Principal,
) -> Result<WriteOutcome, PermHttpError> {
    // The workspace containment, first thing and on the BODY's catalog, so a
    // confined caller is refused before this function reads or writes anything —
    // see [`confine_to_overlay_workspace`]. It lives here rather than in the three
    // handlers because this is the one implementation of a model write, which is
    // also why it takes the principal it has no other use for.
    confine_to_overlay_workspace(tx, p, body.scope.catalog_id).await?;
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
            // The patch's OTHER end. The body's catalog was confined at the top;
            // a grant can MOVE between catalogs, so a caller confined to one
            // workspace must also not be able to drag a *foreign* grant into its
            // own workspace — which is what confining only the body would allow.
            confine_to_overlay_workspace(tx, p, old.scope.catalog_id).await?;
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
pub(crate) async fn write_rule(
    tx: &mut Transaction<'_, Postgres>,
    id: Option<Uuid>,
    body: NewRule,
    p: &egret_core::Principal,
) -> Result<WriteOutcome, PermHttpError> {
    // Same containment, same position and same reason as [`write_grant`]'s.
    confine_rule_selector(tx, p, &body.selector).await?;
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
            // The patch's other end — a rule can be moved between selectors, so
            // see the twin comment in `write_grant`.
            confine_rule_selector(tx, p, &old.selector).await?;
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
    // **The one shape that makes a table UNREADABLE instead of masked** — SP4d
    // debt Task 5. A tag-addressed MASK plus a table-level attachment of that
    // same tag makes the engine type-check the mask against every column of the
    // scanned table, so `count(*)` fails too; measured live on Trino 470 during
    // SP4d-2b's live gate. The rule, the measurement, every write path that can
    // reach the pair and why there is no migration for it all live in
    // `egret_policy::unreadable_table_mask` — ONE function, whose other arm
    // `egret_bi::CatalogTagRepo::assign_catalog` calls, because either half can
    // be created second and a guard keyed to one creation order is the defect
    // this programme has produced four times.
    //
    // Gated on the shape of the row being WRITTEN, not on the pair: a tag ROW
    // FILTER, a path-addressed mask, and a PATCH that turns a tag mask INTO
    // either of those are all untouched — the last one matters, because it is
    // one of the two remedies the refusal names and it has to keep working.
    // `validate_rule_shape` cannot host this: it is service- AND
    // database-independent by construction, and this predicate reads two tables.
    if let (RuleKind::Mask { .. }, Selector::Tag { key }) = (&r.kind, &r.selector) {
        if let Some(pair) = unreadable_table_mask_for(
            tx,
            TagWrite::Mask {
                tag: TagRef::Key(key),
                origin: MaskOrigin::Rule,
                name: &r.name,
            },
        )
        .await
        .map_err(PermHttpError::db)?
        {
            return Err(PermHttpError::BadRequest(pair.refusal()));
        }
    }

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
        // `perm.rules.name` is UNIQUE. A taken name is the caller's to fix, so
        // it is a 409 naming it, not a 500 — on the create route and on a
        // migration that lost a race for the name (SP4d-3 Task 7 review M-1).
        None => match create_rule_in_tx(tx, &r).await {
            Err(egret_perm::PermError::Db(sqlx::Error::Database(d)))
                if d.is_unique_violation() && d.constraint() == Some("rules_name_key") =>
            {
                return Err(PermHttpError::Conflict(format!(
                    "a rule named {:?} already exists; rule names are unique",
                    r.name
                )));
            }
            other => other?,
        },
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
        (status = 400, description = "The grant names a scope level as \"\" (Ranger's \"every value at that level\", not a resource name — it would be enforced across the whole catalog), names a level beneath a \"*\" one (a path cannot skip a level, and a \"*\" names none), or cannot be compiled for a profile it affects", body = ErrorBody),
        (status = 409, description = "A rule overlap: two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
    let (id, compile) = write_grant(&mut tx, None, body, &p)
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
        (status = 400, description = "The patched grant names a scope level as \"\" (Ranger's \"every value at that level\", not a resource name), names a level beneath a \"*\" one (a path cannot skip a level, and a \"*\" names none), cannot be compiled, names no principal or no action, or is owned by the workspace-assignment sync", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such grant", body = ErrorBody),
        (status = 409, description = "The grant changed concurrently and now affects a profile this request did not lock; retry; or two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
    let (_, compile) = write_grant(&mut tx, Some(id), body, &p)
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
        (status = 409, description = "A rule overlap: two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
    // The delete paths build no body, so the containment runs on the stored row —
    // see [`confine_to_overlay_workspace`].
    confine_to_overlay_workspace(&mut tx, &p, g.scope.catalog_id).await?;
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

/// Every mask / row-filter pair already stored in a profile's download. Writes
/// refuse only the pairs they create; this is where an operator sees the rest.
#[utoipa::path(
    get,
    path = "/api/control-plane/perm/overlaps",
    tag = "perm",
    responses(
        (status = 200, description = "Every overlapping mask or row-filter pair stored in a profile's download, compiled or hand-written, with the profile that serves it", body = Vec<crate::overlap::StoredOverlap>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_overlaps(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<Vec<crate::overlap::StoredOverlap>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    // One read-only snapshot: the report cannot write, and sees one state.
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(PermHttpError::db)?;
    let report = crate::overlap::stored_overlaps_via(&mut tx).await?;
    tx.rollback().await.map_err(PermHttpError::db)?;
    Ok(Json(report))
}

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
        (status = 400, description = "The rule names a path level as \"\" (Ranger's \"every value at that level\", not a resource name), names a path level beneath a \"*\" one (a path cannot skip a level, and a \"*\" names none), is a tag-addressed mask whose tag is attached to a whole TABLE (which makes that table unreadable rather than masked), or cannot be compiled for a profile it affects", body = ErrorBody),
        (status = 409, description = "A rule overlap: two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
    let (id, compile) = write_rule(&mut tx, None, body, &p)
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
        (status = 400, description = "The patched rule names a path level as \"\" (Ranger's \"every value at that level\", not a resource name), names a path level beneath a \"*\" one (a path cannot skip a level, and a \"*\" names none), is a tag-addressed mask whose tag is attached to a whole TABLE (which makes that table unreadable rather than masked), or cannot be compiled for a profile it affects", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such rule", body = ErrorBody),
        (status = 409, description = "The rule changed concurrently and now affects a profile this request did not lock; retry; or two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
    let (_, compile) = write_rule(&mut tx, Some(id), body, &p)
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
        (status = 409, description = "A rule overlap: two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
    // Same as `delete_grant`'s.
    confine_rule_selector(&mut tx, &p, &r.selector).await?;
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
        (status = 409, description = "The previewed row changed concurrently — the same refusal the write would have given; or two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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
        (Some(g), None) => write_grant(&mut tx, id, g, &p).await?,
        (None, Some(r)) => write_rule(&mut tx, id, r, &p).await?,
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
/// 1. **The artifact is assembled by [`artifact_via`], which mirrors
///    `download_policies` field for field and loads each profile's policy set
///    through `enabled_policies_for_service` — the same assembler the download
///    route uses, not `list_policies`.** "What explain evaluated" and "what the
///    engine received" must not be able to drift.
/// 2. **`roles` and `groups` are empty** — see [`ExplainSubject`].
/// 3. **`resolved_tags` carries egret's own resolution, and
///    [`resolved_tags_for_target`] is what decides whether it may.** Until
///    SP4d-2b Task 7 this was `None` unconditionally, which made every verdict on
///    every catalog a profile serves `Undecidable` while that profile's tag block
///    held any policy at all. It is now a real tag set for the one resolution
///    shown enricher-equivalent, and still `None` — so still a refusal — for
///    every state where it is not. The whole argument is on that function.
/// 4. **A `write` explain is several evaluations**, one per Ranger access type
///    the action maps to on that engine, reported separately.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/explain",
    tag = "perm",
    request_body = ExplainRequest,
    responses(
        (status = 200, description = "One verdict per profile over the artifact each engine downloads — including the `no_profile_serves_the_catalog` outcome for a catalog that exists and that no profile serves, which is an answer and not an error", body = ExplainResponse),
        (status = 400, description = "The target skips a resource level (a column implies a table implies a schema), names a level as \"\" or \"*\" (Ranger's \"every value\", not a resource name), or names a catalog id no registered catalog has", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission, or a workspace-scoped impersonation overlay asking about a catalog that workspace is not assigned", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn explain(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<ExplainRequest>,
) -> Result<Json<ExplainResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    validate_explain_target(&body.target)?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    confine_to_overlay_workspace(&mut conn, &p, body.target.catalog_id).await?;
    // **The catalog has to EXIST, and that is a different refusal from the one
    // below it.** `services_for_catalog_via` answers "which profiles bind this
    // catalog" with a `WHERE ea.catalog_id = $1` join, so a uuid naming no
    // `registry.catalogs` row returns the empty set — indistinguishable, in the
    // response, from a catalog that is real and that no profile serves. The
    // steward then reads `no_profile_serves_the_catalog` for a typo and believes
    // they have learned something about enforcement.
    //
    // ⚠ **`no_profile_serves_the_catalog` is NOT what this guard refuses, and it
    // must not be "tidied" into one.** "Nothing is enforcing here" is a real,
    // useful answer a steward asks for on purpose — it is pinned at
    // `a_catalog_no_engine_serves_is_its_own_outcome_not_a_denial`, and the whole
    // point of separating the two is that an EXISTING unserved catalog keeps
    // answering 200 with that outcome while a uuid naming nothing is refused.
    // The guard narrows the input, never the outcome vocabulary.
    //
    // A separate round trip rather than reusing the per-profile name lookup: that
    // one (`catalogs_for_service_via`) is the COMPILER's reachability test and is
    // deliberately scoped to one profile's engines, so it cannot answer "does this
    // row exist at all" — and making it do so would re-merge the two questions
    // this guard exists to separate.
    if catalog_names_for_ids_via(&mut conn, &[body.target.catalog_id])
        .await?
        .is_empty()
    {
        return Err(PermHttpError::BadRequest(format!(
            "no registered catalog has id {}: nothing can be explained about a \
             resource that does not exist. (A catalog that DOES exist but that no \
             engine profile serves is a different thing, and is answered with the \
             `no_profile_serves_the_catalog` outcome rather than refused.)",
            body.target.catalog_id
        )));
    }

    // ONE resolution for the whole response, not one per profile: the levels it
    // synthesises are the literal `catalog`/`schema`/`table`/`column` keys
    // `ServiceTags::from_assignments` writes, which do not vary by engine (that
    // is a defect in the WRITER, ruled on in `attachment_for` — it is why a
    // StarRocks profile's attachments are dropped by the enricher rather than
    // renamed). The resolution is a property of the target, so computing it once
    // also removes any way for two profiles to be answered from two tag sets.
    let (resolved_tags, tag_resolution_declined) =
        explain_tag_inputs(resolved_tags_for_target(&mut *conn, &body.target).await?);
    // The tag set as the steward reads it, derived from the resolution above and
    // from nothing else — Task 8's F4/F5. It was a second query
    // (`tag_keys_for_resource_via`, `tags.key`, exact-match) and the browser
    // measured the two disagreeing on a real engine: a tag-driven deny rendered
    // beside "None". Reading the evaluator's own input back is what makes "what
    // the page shows" and "what was evaluated" one object instead of two that
    // must agree.
    //
    // ⚠ Computed once because the RESOLUTION is per request, and then handed to
    // each profile below rather than to the response — review H1/H2/H3. A list on
    // the response is a claim about verdicts that may not exist (H3) or that could
    // not have used it (H1). `None` ⇒ empty, and `tag_resolution_declined` is what
    // says why.
    let resolved_for_display = resolved_tags
        .as_ref()
        .map(resolved_tags_for_display)
        .unwrap_or_default();

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
        // The version this profile's plugin runs at: the bound engine's when
        // exactly one is bound (an external engine declares its own), the plugin
        // version of egret's default image for the type when none is. Several bound engines of one profile are
        // refused to disagree — a profile is evaluated at ONE version here, and
        // a guess between two would be an evaluation nobody measured.
        let plugin_version = profile_plugin_version(&mut conn, service_id, engine).await?;
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

        let artifact = artifact_via(&mut conn, &svc).await?;
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
        // `artifact_via` builds never carries one, and
        // `Unmodelled::ServiceConfig` would refuse the decision if it did.
        //
        // The one field added on top is `engine`, which arms the def
        // cross-check:
        // Trino's middle resource level is `schema` and StarRocks' is
        // `database`, so pairing the wrong def with an engine answers "nothing
        // matched" with full confidence, and declaring the engine turns that
        // silent wrong answer into `UndecidableCause::EngineMismatch`.
        // ⚠ `plugin_version` is DECLARED, and this comment used to say the
        // opposite: *"stays `None` — the engine type already fixes the measured
        // value, so declaring it would only cross-check it against itself."*
        // That was true while the field's only reader was the cross-check. It
        // is not any more.
        //
        // SP4d-2b Task 5 measured a shape the two plugin versions answer
        // DIFFERENTLY — a tag DENY plus a strictly higher-priority resource ALLOW
        // with no deny items is `Allowed` on 2.8.0 and `Denied` on 2.5.0 — so
        // `evaluate` now reads `plugin_version`, and `None` there costs an ACCESS
        // answer (`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`).
        // Leaving it unset would refuse that shape on BOTH engines, including the
        // one whose answer this crate reproduces exactly. The value is
        // `profile_plugin_version`'s: the bound engines' plugin, resolved from
        // each one's exact image. `EngineType::models` then cross-checks it.
        let config = EngineConfig {
            engine: Some(ranger_engine(engine)),
            plugin_version: Some(plugin_version),
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
                // egret's own resolution, or `None` where
                // `resolved_tags_for_target` declined — never
                // `tag_keys_for_resource_via`'s `tags`, which is the
                // attribution list and is both the wrong identifier
                // (`tags.key`) and the wrong breadth (no roll-up). `None` here
                // still means "the caller did not resolve", which the evaluator
                // refuses with `TagsUnresolved`; it is never the empty set,
                // which is the measured narrowing.
                resolved_tags: resolved_tags.clone(),
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

        // Read off the artifact THIS profile's plugin downloads — the same
        // `artifact_via` the verdicts above were computed from, so the count and
        // the answers cannot be about different downloads. A profile with no
        // bound tag service has no tag block and counts `0`, and so does one
        // whose block is present but empty (`bind_trino_profile` creates exactly
        // that): in both cases no tag policy of any kind can decide here.
        let tag_policies_in_download = artifact.tag_policies.len();
        profiles.push(ExplainProfile {
            service_id,
            service_name: svc.name,
            engine: WireEngineType::from(ranger_engine(engine)),
            accesses,
            tags: tags_for_profile(&resolved_for_display, tag_policies_in_download),
            tag_policies_in_download,
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
        tag_resolution_declined,
    }))
}

/// Everything `explain` refuses about its target's SHAPE, in one pure function
/// so the rules sit together and can be exercised without a router.
///
/// # 1. A level the target skips
///
/// **Refused, and it has to be refused rather than answered** (whole-branch
/// review, I-4 — fail-open). `request_resource` stops at the first absent level,
/// so `{schema: None, table: Some(_)}` builds the resource `{catalog}` alone: the
/// caller asks about a table and the evaluator answers about its catalog. That is
/// not a harmless approximation — a catalog-wide grant matches `{catalog}` as
/// `SELF_AND_ALL_DESCENDANTS` (an allow) while the table-level clearance deny
/// that would have decided the real target is only a `DESCENDANT` match, which
/// the allow test does not admit. So the silently-substituted answer is
/// **Allowed, with full confidence**.
///
/// Nothing upstream stops it: `ScopePicker` narrows progressively so the page
/// cannot send this, but the level-skip rule lives in a CHECK on the STORED
/// `perm.grants`/`perm.rules` rows and an `explain` body is never stored.
///
/// # 2. A level present as `""` or `"*"`
///
/// Both are [`egret_ranger_eval::resource::requests_all_values`] — Ranger's "every
/// value at this level", not a name. **That is the evaluator's own exported rule
/// and this guard CALLS it**, rather than spelling it out a second time: two
/// guards that must agree, written to different shapes, is a defect this
/// programme has measured four times, so the pairing is a call and not a
/// convention (SP4d debt Task 4's fix round; it shipped as two tests naming each
/// other, which was the weaker form). One mutation of that function now kills
/// tests in BOTH crates — before the export, each mutation killed only the crate
/// it was made in.
///
/// The `None` arm of the predicate belongs to rule 1 above, so this loop unwraps
/// first: an absent TAIL level is a legitimate question answered by its parent.
///
/// The measured consequences of the two present forms differ, which is why
/// neither may be answered:
///
/// * `""` is where **egret's evaluator and the engine are known to
///   disagree**. `egret_ranger_eval::resource::level_excluded_by_trie` carries a
///   measured, deliberately-unclosed gap: on ranger 2.8.0 a request
///   `{catalog: ""}` reaches a policy whose only level is a literal and answers
///   `Allowed`, while the trie candidacy filter reports that policy absent. The
///   rule that separates it from the request shapes the trie gets right is not
///   established, so the gap stays open in the evaluator — and the honest place
///   to close it is here, at the only surface that lets a caller choose the
///   request.
///
///   ⚠ **That comment used to bound the shape by authorship**: *"an empty
///   REQUEST level is not something egret sends — `perm_compile`'s
///   `request_resource` builds concrete values from the target"*. It was wrong,
///   and this is the function that makes it true: `request_resource` copies
///   `target.schema`/`table`/`column` through verbatim, so until this guard
///   existed any `policy:read` caller sent `""` by putting it in the body.
///   `ScopePicker` maps its own empty input to `undefined`, which is why nothing
///   in the browser ever produced it — authorship, not reach.
///
/// * `"*"` the trie and the matcher agree about, and the answer is still about a
///   different question than the one asked. `isAllValuesRequested && !isMatchAny`
///   short-circuits to NONE *without reading a single policy value*, so a target
///   of `{schema: "*"}` reports "nothing matched" for every policy naming a real
///   schema, and matches only the match-any ones. A steward typing `*` into the
///   free-text Schema field — which `ScopePicker` passes through, and `*` is
///   exactly what they see in compiled GRANT resources — gets a verdict that is
///   neither "the schema named `*`" nor "all schemas".
///
/// Returning them as a 400 is the same argument the level skip already won: there
/// is no engine request this shape corresponds to.
///
/// Whitespace is deliberately NOT refused, and that follows from the predicate
/// rather than from taste: `" "` is not one of Ranger's three forms, so the
/// matcher compares it literally and the verdict really is about a resource with
/// that name.
fn validate_explain_target(target: &Scope) -> Result<(), PermHttpError> {
    if target.schema.is_none() && (target.table.is_some() || target.column.is_some())
        || target.table.is_none() && target.column.is_some()
    {
        return Err(PermHttpError::BadRequest(
            "a target cannot skip a level: a column implies a table implies a schema. \
             An engine request names exactly the levels the query touched, so a gap \
             names no resource any engine could ask about — and answering it would \
             answer about the nearest named ancestor instead"
                .into(),
        ));
    }
    for (level, value) in [
        ("schema", target.schema.as_deref()),
        ("table", target.table.as_deref()),
        ("column", target.column.as_deref()),
    ] {
        // Presence is tested HERE and not inside the predicate: `None` is one of
        // Ranger's three all-values forms, and an absent level is rule 1's
        // business — a target that names a schema and stops is the page's ordinary
        // catalog-or-schema-wide question.
        let Some(value) = value else { continue };
        if requests_all_values(Some(value)) {
            return Err(PermHttpError::BadRequest(format!(
                "a target's `{level}` is a resource NAME, and {value:?} is Ranger's \
                 \"every value at this level\" (`isAllValuesRequested` is null, \"\" or \
                 \"*\"). No engine request carries it: the plugin names the schema, table \
                 and column the query actually touched. Omit the level to ask about its \
                 parent instead — the verdict for \"all values\" is not the verdict for \
                 any one of them, and for \"\" egret's own evaluator is measured \
                 disagreeing with the engine"
            )));
        }
    }
    Ok(())
}

/// **The one workspace boundary that reaches this handler, and it arrives by
/// impersonation — not by the caller having a workspace.**
///
/// ⚠ Read this before adding or removing a workspace check here, because the
/// obvious reading of "scope `explain` to the caller's workspace" is not
/// implementable and must not be faked:
///
/// * `explain` is mounted under `/api/control-plane` with the bare [`Auth`]
///   extractor, never `WsContext`, so **no workspace id reaches it**.
///   `egret_core::Principal` has no workspace field at all.
/// * `policy:read` is `Scope::Platform` in `egret_core::rbac`, and
///   `egret_core::authz::require` gates it on `Resource::Platform`. The PDP loads
///   `Resource::Platform` with `ws = None`, and `egret_auth::grants::grants_for`
///   then admits only `scope_type = 'platform'` rows (`scope_id = $2` can never
///   match a NULL). **A workspace-confined caller holding `policy:read` is not
///   representable.**
/// * `registry.catalogs` is a platform resource with no `workspace_id`; a
///   workspace is *assigned* a catalog (`registry.catalog_workspaces`, many to
///   many). "The catalog's workspace" is not a function.
///
/// So a platform steward explaining any catalog is the designed behaviour, pinned
/// by `a_platform_steward_may_explain_a_catalog_of_any_workspace`.
///
/// What IS a boundary: an impersonation overlay started by a `ws_admin` carries
/// `Impersonator::workspace_scope = Some(ws)`, and
/// `egret_workspace::context::WsContext` confines it — but only on a route that
/// resolves a `{workspace}` path segment, which is the only thing that builds a
/// `WsContext`. **No `/api/control-plane` route resolves one**, so none is
/// confined there. (`WsContext`'s own comment used to claim the containment was
/// "airtight … no route can reach another workspace under a scoped overlay";
/// SP4d debt Task 13 corrected it at the site, and that comment now names which
/// routes it does and does not cover.)
///
/// The path that was reachable, measured, and is now closed at its source:
/// `validate_impersonation_target` refused only a target who was a
/// `platform_admin`, so a `ws_admin` of workspace A could impersonate a member of
/// A holding the platform `data_steward` role — which migration 0059 seeds with
/// the whole `policy:*` family — and both ask about and WRITE a grant on a catalog
/// of workspace B. Task 13 made that validator refuse a target holding ANY
/// platform-scoped role grant — **but only when the overlay was CREATED, and
/// that alone was escapable**: a reviewer showed the target could acquire the
/// platform role afterwards and the live overlay picked it up, because
/// `SessionService::validate` rebuilds the target's roles from live tables every
/// request. `391c3c3a6` (2026-10-10) moved the rule into
/// `egret_auth::impersonation::overlay_refused_by_platform_authority` and made
/// `validate` re-evaluate it on every request, rendering such an overlay inert.
/// Only with that second half does "such an overlay can no longer satisfy the
/// `policy:read` / `policy:update` gate at the top of these handlers" hold — see
/// the ⚠ paragraph on [`confine_to_overlay_workspace`] for the mechanism in full.
///
/// This confines that overlay to catalogs assigned to the workspace it was
/// started in, which is the same containment `WsContext` applies (`scope != ws →
/// Forbidden`) expressed over the relation the `perm` model itself uses for
/// catalog↔workspace: `registry.catalog_workspaces`, the table
/// `egret_perm::sync_assignment_grants_in_tx` derives its implicit grants from.
/// A catalog the overlay's workspace is not assigned is `Forbidden`, which is also
/// what a uuid naming no catalog gets — so a contained caller cannot use the
/// CATALOG uuid in a request body to probe which catalogs are real.
///
/// ⚠ **That non-disclosure covers the body's catalog uuid and nothing else.** An
/// earlier version of this paragraph stopped at "this route", which reads as a
/// claim about every uuid the route accepts; the whole-branch review (2026-10-10,
/// finding L-1) showed it is false for the GRANT and RULE uuids in the PATH,
/// which answer `404` for a uuid naming no row and `403` for one naming a row in
/// another workspace — so a contained caller can still learn that a grant id
/// exists somewhere. That gap is recorded as accepted debt, not covered here:
/// closing it means returning `404` for a row that does exist but is not the
/// caller's to see, which is a decision about every `perm` read, not about this
/// guard.
///
/// **An unscoped overlay (`workspace_scope == None`, a platform_admin acting as
/// someone) is unrestricted here, exactly as it is in `WsContext`.**
///
/// # Where this runs, by code path (SP4d debt Task 13)
///
/// Task 4 called it from `explain` only and recorded the write side as open. It
/// now runs on every route of `perm_router` that *writes* the model, and it runs
/// inside [`write_grant`] and [`write_rule`] rather than in each handler — those
/// two are the one implementation of a model write, shared by create, patch and
/// the rolled-back `preview`, so a future authoring route cannot acquire the hole
/// back by forgetting a call. A patch is confined TWICE, against the stored row's
/// catalog and against the body's, because a patch can move a grant between
/// catalogs and either end alone leaves half the move reachable. `delete_grant`
/// and `delete_rule` call it directly on the stored row, since they never build a
/// body.
///
/// Still **not** confined here, deliberately and named so it is not mistaken for
/// covered:
///
/// * `list_grants` / `list_rules` return the whole model. Confining a list means
///   FILTERING it, which is a different change with a different failure mode (a
///   silently short list), and it is a read.
/// * `compile` names a `service_id`, not a catalog. A profile serves many
///   catalogs, so there is no single catalog to compare; it also writes no model
///   row — it recompiles state some other authorized write already committed.
/// * Every other `/api/control-plane` router (`policy_admin_router`,
///   `catalog_admin_router`, `engine_mgmt_router`, `control_plane_router`).
///
/// All three are closed at the SOURCE instead, by
/// `egret_auth::impersonation::validate_impersonation_target` refusing a
/// ws-scoped overlay over a target that holds any platform-scoped role grant —
/// which is what makes such an overlay unable to satisfy the
/// `policy:read`/`policy:update` gate above this function at all.
///
/// ⚠ **Consequence, recorded because it changes how the tests here should be
/// read: no HTTP entry point reaches this guard, and so no refusal it performs
/// is reachable either.** State the MECHANISM, because this claim has already
/// been written once in the weaker form "nothing bypasses that validator today"
/// and a reviewer falsified that form: the escapable version never bypassed the
/// validator, it OUTLIVED the fact the validator had measured. Since
/// `391c3c3a6` (2026-10-10) two independent facts hold, and the pair — not
/// either one — is what closes this surface:
///
/// 1. `SessionService::validate` re-evaluates
///    `egret_auth::impersonation::overlay_refused_by_platform_authority` on
///    **every request** for the overlay's whole life and, when it refuses,
///    renders the overlay **inert** — the request is decided as the admin, with
///    `impersonator: None`, so [`overlay_workspace_scope`] here is `None`. A
///    ws-scoped overlay whose target holds any platform-scoped role grant
///    therefore never produces a principal on this router at all. **A
///    creation-time measurement alone was NOT enough**: `validate` rebuilds the
///    target's roles and permissions from live tables, so the target could gain
///    a platform role after the overlay began (`sync_user_oidc_groups` on the
///    next OIDC login, or a console grant — neither needing any permission from
///    the admin) and the already-issued overlay picked it up. That is the bug
///    `391c3c3a6` fixed, and it is why this paragraph names a re-evaluated
///    predicate rather than a validator that ran once.
/// 2. A ws-scoped overlay whose target holds NO platform-scoped role grant stays
///    live, but then it carries no platform authority: every handler on this
///    router gates on `Resource::Platform`, the PDP loads that with `ws = None`,
///    and `egret_auth::grants::grants_for`'s `WHERE` admits a `workspace`-scoped
///    row only for `scope_id = $ws` — NULL there matches nothing. So such a
///    principal cannot pass `require()` above this function.
///
/// Those two cover the whole domain of `workspace_scope = Some(_)`, so **the
/// measured protection on this surface is the inertness rule; this function is
/// defence in depth** — kept because it is the only half that would still hold
/// if a future entry point wrote an overlay row without going through either,
/// and because it is keyed to a different relation
/// (`registry.catalog_workspaces`) than the validator is.
///
/// That is why this function is `pub` and tested DIRECTLY, by
/// `confine_to_overlay_workspace_allows_its_own_workspace_and_refuses_another`
/// in `tests/routes.rs`, rather than through the router. Two route-level tests
/// that drove it (`a_workspace_scoped_overlay_is_confined_to_that_workspaces_catalogs`
/// and `…_cannot_create_a_grant_on_another_workspaces_catalog`) wrote the overlay
/// row straight into the database to reach a state the product forbids; after
/// `391c3c3a6` their ALLOW arm could not hold, which is exactly how an
/// unreachable guard announces itself. What the router now pins is the
/// **inertness** —
/// `a_ws_scoped_overlay_over_a_platform_role_target_is_decided_as_the_admin` —
/// at the layer the escalation actually lived on.
pub async fn confine_to_overlay_workspace(
    conn: &mut sqlx::PgConnection,
    p: &egret_core::Principal,
    catalog_id: Uuid,
) -> Result<(), PermHttpError> {
    let Some(scope) = overlay_workspace_scope(p) else {
        return Ok(());
    };
    if catalog_is_assigned_to_workspace_via(&mut *conn, catalog_id, scope.0).await? {
        return Ok(());
    }
    Err(PermHttpError::Forbidden)
}

/// The workspace a ws-scoped impersonation overlay is confined to, or `None` for
/// a caller that is not inside one (and for an UNSCOPED overlay, which is a
/// platform_admin acting as someone and is unrestricted — see
/// [`confine_to_overlay_workspace`]).
///
/// One accessor rather than the same `and_then` chain written at each use, so
/// "what counts as confined" has a single spelling in this module: a second one
/// that read `p.impersonator.is_some()` instead would confine a platform_admin's
/// overlay too, and a second one that forgot the `and_then` would confine nobody.
fn overlay_workspace_scope(p: &egret_core::Principal) -> Option<egret_core::WorkspaceId> {
    p.impersonator.as_ref().and_then(|i| i.workspace_scope)
}

/// [`confine_to_overlay_workspace`] for a rule, whose target is a [`Selector`]
/// rather than a catalog.
///
/// A `Path` selector names a catalog and is confined exactly like a grant's
/// scope. A **`Tag` selector names no catalog and is installation-wide** — it
/// fans out over every profile in the installation (`affected_services_for_rule`),
/// so there is nothing to compare against and no subset of it that stays inside
/// one workspace. A confined caller is therefore refused outright, which is the
/// only answer that is not a guess: silently narrowing the fan-out would author a
/// rule that does not mean what the author wrote, and permitting it would be the
/// widest cross-workspace write on this surface.
///
/// `pub` for the same reason [`confine_to_overlay_workspace`] is: no HTTP entry
/// point can reach it since `391c3c3a6`, so
/// `confine_rule_selector_refuses_a_foreign_path_and_every_tag` tests it
/// directly. Read that function's ⚠ paragraph before relying on either.
pub async fn confine_rule_selector(
    conn: &mut sqlx::PgConnection,
    p: &egret_core::Principal,
    selector: &Selector,
) -> Result<(), PermHttpError> {
    match selector {
        Selector::Path(scope) => confine_to_overlay_workspace(conn, p, scope.catalog_id).await,
        Selector::Tag { .. } if overlay_workspace_scope(p).is_some() => {
            Err(PermHttpError::Forbidden)
        }
        Selector::Tag { .. } => Ok(()),
    }
}

/// **The one place an engine's Ranger plugin version is derived.** A
/// provisioned engine resolves through its EXACT stored image
/// (`egret_perm::engine_images`): the same type can run different plugins as
/// its image changes, so the type alone says nothing. An external engine maps
/// the version its operator DECLARED. Either way the (type, version) pair must
/// be one `egret-ranger-eval` has been read against AND proved on
/// (`egret_perm::ranger_plugin_is_modelled`), else this returns a sentence: a
/// guess would be an evaluation nobody measured.
pub(crate) fn engine_plugin_version(
    name: &str,
    origin: &str,
    r#type: &str,
    declared: Option<&str>,
    spec: &serde_json::Value,
) -> Result<egret_ranger_eval::eval::PluginVersion, String> {
    use egret_ranger_eval::eval::PluginVersion;
    let version: &str = if origin == "external" {
        declared.ok_or_else(|| {
            format!("engine \"{name}\" is external but declares no Ranger plugin version")
        })?
    } else {
        if engine_type_for(r#type).is_none() {
            return Err(format!(
                "engine \"{name}\" is of type `{}`, for which egret has no measured Ranger \
                 plugin version",
                r#type
            ));
        }
        let image = egret_perm::stored_engine_image(r#type, spec).ok_or_else(|| {
            format!(
                "engine \"{name}\" ({}) has no image in its spec, so egret cannot tell which \
                 Ranger plugin it runs; pin its image",
                r#type
            )
        })?;
        egret_perm::plugin_version_for_image(r#type, &image)
            .map_err(|why| format!("engine \"{name}\": {why}"))?
    };
    if !egret_perm::ranger_plugin_is_modelled(r#type, version) {
        let who = if origin == "external" {
            "declaring"
        } else {
            "running"
        };
        return Err(format!(
            "engine \"{name}\" is a {} {who} Ranger plugin {version}, a (type, version) pair \
             egret's evaluator has not been verified against (it models {}), so a decision \
             cannot be proved on it",
            r#type,
            egret_perm::modelled_ranger_plugins_sentence()
        ));
    }
    match version {
        "2.5.0" => Ok(PluginVersion::Ranger2_5_0),
        "2.8.0" => Ok(PluginVersion::Ranger2_8_0),
        "2.9.0" => Ok(PluginVersion::Ranger2_9_0),
        other => Err(format!(
            "engine \"{name}\": plugin {other} is listed as modelled but the evaluator has no \
             variant for it"
        )),
    }
}

/// The engines bound to `service_id` as far as their plugin version goes.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct BoundEngine {
    pub(crate) name: String,
    pub(crate) origin: String,
    pub(crate) r#type: String,
    pub(crate) ranger_plugin_version: Option<String>,
    pub(crate) spec: serde_json::Value,
}

impl BoundEngine {
    pub(crate) fn plugin_version(&self) -> Result<egret_ranger_eval::eval::PluginVersion, String> {
        engine_plugin_version(
            &self.name,
            &self.origin,
            &self.r#type,
            self.ranger_plugin_version.as_deref(),
            &self.spec,
        )
    }
}

pub(crate) async fn bound_engines(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<BoundEngine>, PermHttpError> {
    sqlx::query_as::<_, BoundEngine>(
        "SELECT name, origin, type, ranger_plugin_version, spec FROM engine_mgmt.engines \
          WHERE authz_service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)
}

/// The Ranger plugin version `explain` evaluates profile `service_id` at: its
/// bound engines' (all must agree), or the plugin of egret's default image for the
/// type when none is bound. A version the evaluator has not been read against is a refusal that
/// names it, never an evaluation at a guessed version.
async fn profile_plugin_version(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    profile_engine: EngineType,
) -> Result<egret_ranger_eval::eval::PluginVersion, PermHttpError> {
    let mut versions = Vec::new();
    for e in bound_engines(conn, service_id).await? {
        let v = e.plugin_version().map_err(|sentence| {
            PermHttpError::Conflict(format!(
                "cannot explain profile #{service_id}: {sentence}. Only an engine of a modelled \
                 (type, version) pair can be explained: {}.",
                egret_perm::modelled_ranger_plugins_sentence()
            ))
        })?;
        if !versions.contains(&v) {
            versions.push(v);
        }
    }
    match versions.as_slice() {
        [] => default_image_plugin_version(ranger_engine(profile_engine)).map_err(|s| {
            PermHttpError::Conflict(format!("cannot explain profile #{service_id}: {s}"))
        }),
        [v] => Ok(*v),
        _ => Err(PermHttpError::Conflict(format!(
            "cannot explain profile #{service_id}: its bound engines run different Ranger plugin \
             versions, and a verdict is for one"
        ))),
    }
}

/// The plugin version of egret's own default image for an engine type: what an
/// UNBOUND profile is evaluated at. Refused, naming the image, when that pair
/// is not one the evaluator models.
fn default_image_plugin_version(
    engine: egret_ranger_eval::eval::EngineType,
) -> Result<egret_ranger_eval::eval::PluginVersion, String> {
    use egret_ranger_eval::eval::EngineType as E;
    let d = &egret_perm::DEFAULT_ENGINE_IMAGES;
    let (t, spec, image) = match engine {
        E::Trino => (
            "trino",
            serde_json::json!({"imageVersion": d.trino_image_version}),
            egret_perm::trino_image_ref(d.trino_image_version),
        ),
        E::StarRocks => (
            "starrocks",
            serde_json::json!({"starrocksConfig": {"feImage": d.starrocks_fe_image}}),
            d.starrocks_fe_image.to_string(),
        ),
    };
    engine_plugin_version(
        &format!("(no engine bound; egret's default image {image})"),
        "provisioned",
        t,
        None,
        &spec,
    )
}

/// `egret_perm::EngineType` → the evaluator's own. Two enums for one fact
/// because `egret-ranger-eval` is a pure crate that depends on neither
/// `egret-perm` nor `egret-policy`; this is the single crossing.
pub(crate) fn ranger_engine(engine: EngineType) -> egret_ranger_eval::eval::EngineType {
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
///
/// **Connection-generic since SP4d-3 Task 7**, so the migration gate assembles
/// the download through THIS function inside its own transaction — before the
/// write and again after the recompile — rather than through a second
/// assembler. `explain` passes its pooled connection.
pub(crate) async fn artifact_via(
    conn: &mut sqlx::PgConnection,
    svc: &ServiceRow,
) -> Result<ServicePolicySet, PermHttpError> {
    let def = get_service_def_via(conn, &svc.r#type).await?.def_jsonb;
    let policies = enabled_policies_for_service_via(conn, svc.id).await?;
    let tag_service = svc
        .config_jsonb
        .get(egret_policy_api::TAG_SERVICE_CONFIG_KEY)
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
        if let Some(tid) = service_id_by_name_via(conn, &name).await? {
            let tsvc = get_service_by_id_via(conn, tid).await?;
            let tdef = get_service_def_via(conn, &tsvc.r#type).await?.def_jsonb;
            let tpolicies = enabled_policies_for_service_via(conn, tsvc.id).await?;
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

/// The tag set `explain` may hand
/// [`egret_ranger_eval::eval::AccessRequest::resolved_tags`] for one target, or
/// `None` where egret's resolution cannot be shown equivalent to
/// `RangerTagEnricher`'s and the narrowing must therefore not be offered.
///
/// `None` costs an answer: the evaluator raises
/// `UndecidableCause::TagsUnresolved` for any profile whose tag block holds a
/// policy of the kind being decided. That is the price of the one thing that is
/// not an option — handing the evaluator a set NARROWER than the enricher's,
/// since `Some(empty)` is a licence to skip the tag phase outright
/// (`getLikelyMatchPolicyEvaluators` returns `Collections.EMPTY_LIST` at its
/// `isNotEmpty(tags)` branch).
///
/// # The equivalence, and the one precondition it rests on
///
/// `resolved_tags_for_resource_via` (SP4d-2b Task 1) reproduces
/// `RangerTagEnricher.findMatchingTags` for the two levels egret's storage was
/// designed to express: a COLUMN attachment is `SELF` for the request that names
/// that column, and a TABLE attachment (`column_name = ''`) is `ANCESTOR` for a
/// request naming any of its columns and `SELF` for the request naming the table.
/// Its predicate is `schema_name = $2 AND table_name = $3 AND (column_name = $4 OR
/// column_name = '')`.
///
/// Its comparison is case-folded (fix round 1), which reproduces the matcher's
/// `ignoreCase: true`; what it cannot reproduce is a stored value that is not a
/// LITERAL. So the set it returns is exactly the enricher's **provided every
/// stored coordinate in the catalog is one this predicate reproduces** — which was
/// three conditions, and is now ONE read plus one `CHECK`:
///
/// * an empty `schema_name` or `table_name`, and a `*` or `?` in any level, are
///   **unrepresentable** — migration 0177's
///   `catalog_tag_assignments_levels_are_present_and_literal`, VALIDATED, so no
///   running egret can read such a row and nothing here rules on one. The write
///   that used to produce them is refused by `egret_bi::catalog_tags` with a 400
///   keyed to the same predicate shape.
/// * a character on which the two case folds can disagree is still only
///   DETECTABLE, never forbidden — those 25 code points are legitimate identifiers
///   and which of them diverge depends on the deployment's collation. So
///   [`stored_coordinate_is_fold_divergent`] is the surviving ruling,
///   [`TagLookups::assignment_coordinates`] still reads every coordinate, and this
///   function still checks them FIRST, for the whole catalog, before any
///   resolution. See the ⚠ block on
///   `egret_policy::tag_assignment_coordinates_via` for the fail-open that closes
///   and why no mapping over the resolver's RESULT could close it instead.
///
/// ⚠ **The earlier version of this paragraph named ONE condition** ("provided
/// every stored row is at one of those two levels"), and the review measured two
/// more of the same shape — a `('sales','*','')` row and a `('SALES','ORDERS','')`
/// row are both `ANCESTOR`-admitted on the real jar and neither was returned or
/// counted. A proviso is only worth what its enumeration covers, which is why the
/// enumeration survives on [`stored_coordinate_is_fold_divergent`] — including the
/// two rows the schema now forbids — rather than being deleted along with the
/// branches, and why it was never a SQL `WHERE` clause nothing here can run.
///
/// With the precondition holding, a target that names no schema or no table is
/// `Some(empty)` rather than a refusal, and that is an argument rather than a
/// convenience: the request resource is then `{catalog}` or `{catalog, schema}`,
/// every stored attachment is at `{catalog, schema, table[, column]}`, and a
/// deeper attachment is a `DESCENDANT` — which `findMatchingTags` records and
/// `RangerDefaultPolicyEvaluator.evaluate` then does NOT admit for a tag request
/// at `ResourceMatchingScope.SELF` (see `tag_match_is_admitted`). The enricher
/// resolves nothing for such a request either.
///
/// # ⚠ Enumerated: every input this reads, and every input it could read
///
/// A new field on any of these is a new row here, and the ruling belongs at this
/// site rather than in a plan.
///
/// | input | read | ruling |
/// |---|---|---|
/// | `target.catalog_id` | **yes** | scopes both reads, and scopes them identically (`a.warehouse_id = c.warehouse_id` for `c.id = $1`), so a row the gate counts is a row the resolver could return |
/// | `target.schema` / `target.table` — PRESENCE | **yes** | `None` on either ⇒ `Some(empty)`, argued above. `Some("")` is NOT special-cased, and since migration 0177 that needs no gate to be sound: no stored coordinate CAN have an empty `schema_name` or `table_name`, so an empty request value matches nothing and the empty set is the right answer. ⚠ Before 0177 this row rested on the gate having refused a `('', '')` row; it now rests on the `CHECK`, which is a stronger claim about a smaller surface |
/// | `target.schema` / `target.table` / `target.column` — the VALUES' CASE | **yes, and ruled on since fix round 1** | the resolver folds both sides, so `SALES` against a row stored `sales` resolves. ⚠ This row did not exist, and its absence was the review's Critical: the values reach `request_resource` verbatim from `ScopePicker`'s free-text inputs, so differing case is what ORDINARY use produces — more reachable than the empty-string class that was gated |
/// | `target.*` — the VALUES' SYNTAX | **no, and that is a ruling** | a `*` or `?` the OPERATOR types is a request value, and Ranger matches a request value literally: the wildcard lives on the POLICY side of `getMatchType`, never the request side. So a typed `*` names a resource called `*` and resolves nothing, which is correct — and migration 0177 does NOT constrain the request, only what can be stored, so a steward can still type one. The syntax that matters is the STORED side's, and since 0177 the schema refuses it rather than this function detecting it |
/// | `target.*` — a [`FOLD_DIVERGENT`] character | **yes, since fix round 2** | ⚠ the divergence is a property of the PAIR, not of the stored side: `("ſ", "s")` is measured disagreeing, so an ASCII stored value and an odd REQUEST value are the same fail-open. Checked with the same `fold_divergent` the stored side uses |
/// | `target.column` | **yes** | `None` ⇒ `""`, which is migration 0081's "the table itself" convention in the REQUEST position, and is the value `resolved_tags_for_resource_via` is documented against |
/// | `assignment_coordinates` + [`stored_coordinate_is_fold_divergent`] | **yes, first** | a divergent coordinate ⇒ `None`. Per CATALOG and not per request: the coordinate that breaks the equivalence is one the resolver cannot return, so there is nothing per-request to test it against. ⚠ The read is NOT retired by 0177 — the `CHECK` takes the empty-level and wildcard classes, and the fold class is the one a `CHECK` must not take |
/// | `egret_policy::ResolvedTag::name` | **yes** | `tags.name`, which is what `from_assignments` puts in the wire tag's `type` and therefore what the engine matches. Never `tags.key` — live they differ for one of three tags |
/// | `egret_policy::ResolvedTag::match_type` | **yes** | both the evaluator's admission test AND, here, the only surviving record of WHICH level the attachment was at — see [`attachment_for`] |
/// | the engine (Trino / StarRocks) | **no** | and that is the ruling, not an omission: `ServiceTags::from_assignments` writes the keys `catalog`/`schema`/`table`/`column` for every service type. See [`attachment_for`] |
/// | `tag_keys_for_resource_via`'s result | **no** | the attribution list: `tags.key`, exact-match only. Reusing it here is the fail-open D3 names |
/// | `catalog_tag_assignments.warehouse_id`, `tags.id`, `tags.key` | **no** | not on the tag wire the plugin matches against |
/// | the profile, its service def, its `tagService` | **no** | one resolution per TARGET answers every profile; nothing about a profile changes what the enricher would resolve for a resource. The def-dependent half — the enricher's DROP — is the evaluator's `enriched_tags`, which holds the resource def this function does not |
async fn resolved_tags_for_target<L: TagLookups + Send>(
    conn: &mut L,
    target: &Scope,
) -> Result<Result<UnenrichedTags, ExplainTagDecline>, PermHttpError> {
    // THE GATE, first and before anything can return a set. Nothing below may
    // move above it: every argument on this function for why the resolution is
    // enricher-equivalent assumes it passed.
    //
    // ── the REQUEST side of the fold, first ────────────────────────────────
    //
    // ⚠ **Fix round 2.** The gate used to ask only about STORED values, and that
    // is not where the fold's divergence lives: the pair `("ſ", "s")` is measured
    // DISAGREEING, so the stored value can be ordinary ASCII while the request
    // carries the odd character. `fold_divergent` is the same predicate
    // `stored_coordinate_is_fold_divergent` applies, asked of the target's own
    // three values.
    //
    // Checked BEFORE the `Some(empty)` shortcut below, which is strictly
    // conservative: with no schema or table named, no stored attachment is
    // SELF-or-ANCESTOR of the request whatever either fold does, so this costs an
    // answer in a state that was already safe. Ordered this way on purpose — the
    // alternative is an argument per branch, and a gate that has to be re-argued
    // per branch is the shape that lost two classes in round 1.
    //
    // ⚠ The REASON is returned, not discarded. Fix round 3 measured that
    // `.is_some()` here threw away the only thing that could tell the decline
    // states apart, and the page then rendered round 1's single sentence for every
    // one of them — naming a stored row that did not exist and claiming a
    // per-catalog reach for this per-REQUEST one. (Four states then; two since
    // migration 0177.)
    for (level, value) in [
        (ExplainTargetLevel::Schema, target.schema.as_deref()),
        (ExplainTargetLevel::Table, target.table.as_deref()),
        (ExplainTargetLevel::Column, target.column.as_deref()),
    ] {
        if value.is_some_and(fold_divergent) {
            return Ok(Err(ExplainTagDecline::TargetFoldDivergent {
                level,
                value: value.unwrap_or_default().to_string(),
            }));
        }
    }
    // `find`, not `any`: one offending coordinate anywhere in the catalog is
    // enough — the resolver's predicate is the same predicate for every target in
    // it — and the one that offends is the one the page has to name.
    //
    // ⚠ **WHAT THIS STILL ASKS, after migration 0177.** The gate used to rule on
    // three classes; the CHECK that migration adds makes two of them
    // unrepresentable (an empty `schema_name`/`table_name`, and `*`/`?` in any
    // level), so the only class left for this read is the fold divergence — which
    // a CHECK must NOT take over, because those 25 code points are legitimate
    // identifiers and the set is collation-dependent. The migration's own text
    // carries that ruling. The read therefore STAYS: it is one `SELECT DISTINCT`
    // per `explain`, and it is the only thing that can see a stored coordinate the
    // resolver's fold and the matcher's fold could disagree about.
    //
    // ⚠ **WHICH one, when several offend: the FIRST in
    // `tag_assignment_coordinates_via`'s order**, which is
    // `ORDER BY schema_name, table_name, column_name`. Task 8 reported this as
    // unpinned and was half wrong — the SQL has always carried that `ORDER BY`;
    // what nothing pinned was the rule on THIS side, so `the_first_offending_
    // coordinate_is_the_one_named` now asserts it with two offenders in both
    // orders. The order is the database's collation's, so it is deterministic
    // within a deployment rather than universal; a catalog with two broken
    // assignments reports one of them and fixing it reveals the next.
    if let Some((schema, table, column)) = conn
        .assignment_coordinates(target.catalog_id)
        .await?
        .into_iter()
        .find(|(schema, table, column)| stored_coordinate_is_fold_divergent(schema, table, column))
    {
        return Ok(Err(ExplainTagDecline::StoredFoldDivergent {
            schema,
            table,
            column,
        }));
    }
    let (Some(schema), Some(table)) = (target.schema.as_deref(), target.table.as_deref()) else {
        // Not a shortcut — see the DESCENDANT argument in the doc above.
        return Ok(Ok(UnenrichedTags::new(Vec::new())));
    };
    let column = target.column.as_deref().unwrap_or("");
    let resolved = conn
        .resolved_tags(target.catalog_id, schema, table, column)
        .await?;
    // `collect::<Option<Vec<_>>>` so that ONE unmappable attachment declines the
    // whole resolution rather than silently shipping a set with a hole in it.
    //
    // ⚠ The decline it produces is reported as `StoredFoldDivergent` on the
    // target's own coordinate, and that is an approximation stated rather than
    // hidden: `attachment_for` returns `None` only for a match type no arm of
    // `resolved_tags_for_resource_via` produces, which no caller can reach today
    // (its two arms are `Self_` and `Ancestor`). If a producer ever reaches it,
    // the page will name the right resource and the wrong reason — which is why
    // the arm is also a refusal in the evaluator rather than only here.
    Ok(resolved
        .iter()
        .map(|tag| attachment_for(tag, schema, table, column))
        .collect::<Option<Vec<_>>>()
        .map(UnenrichedTags::new)
        .ok_or(ExplainTagDecline::StoredFoldDivergent {
            schema: schema.to_string(),
            table: table.to_string(),
            column: column.to_string(),
        }))
}

/// The 25 code points on which Postgres `lower()` and Java `equalsIgnoreCase` can
/// disagree, DERIVED by a sweep over every defined code point in Unicode and
/// asserted verbatim by
/// `tests/ranger/src/test/java/EgretCaseFoldAgreementTest.java::theDisagreementSetIsExactlyTheOneTheGateEncodes`.
///
/// **This list is the SINGLE SOURCE OF TRUTH, and the test reads it** — ⚠ fix
/// round 3, MINOR 1. The Java test used to hold its own copy of the set and assert
/// the sweep against THAT, so this constant was compared to nothing: deleting
/// U+1C85 (a fail-open) or mistyping U+03F1 (an over-refusal) failed no test, since
/// `[char; 25]` pins only the length and the Rust tests name 7 of the 25. The test
/// now parses this very declaration out of this file, so a code point the sweep
/// finds and this lacks is a fail-open, one this has and the sweep does not find is
/// an over-refusal, and it fails on either.
///
/// # ⚠ Measured on a real database, and TWO entries are stand-in-only
///
/// The sweep's `lower()` side is Java's `String.toLowerCase(Locale.ROOT)` standing
/// in for Postgres. Fix round 3 also ran the real thing — PostgreSQL 16.15,
/// `en_US.utf8`, read-only `SELECT lower(…)`, no template touched — over the 53
/// single-character pairs this set is derived from: **51 still disagree there, 2 do
/// not.** Both of the two are U+0130's.
///
/// * **U+0130 is therefore a MEASURED OVER-REFUSAL on `en_US.utf8`**: glibc's
///   `lower('İ')` is `'i'`, length 1, not the two-code-point full mapping the
///   stand-in produces, so the folds agree there and this declines anyway. It stays
///   because the direction is conservative and because **`tr_TR` is the collation
///   where it earns its place** — and `tr_TR` is also where this set as a whole
///   becomes too NARROW, since it would move `I`/`i` in.
/// * **U+03A3 stays load-bearing on both oracles, for different reasons.** On the
///   stand-in it is criterion 3 (the context-dependent `Final_Sigma`); on glibc,
///   which has no `Final_Sigma`, the string pair `("ΑΣ","ασ")` AGREES and what
///   catches it is its single-character pair with U+03C2, which disagrees there.
///   The criterion stays because an ICU collation does implement `Final_Sigma`.
///
/// Everything else behaves on the real database exactly as the sweep says,
/// Vietnamese included. **Falsified by** a case-tailoring collation, or an
/// ICU/glibc difference on one of the 25.
const FOLD_DIVERGENT: [char; 25] = [
    '\u{00B5}', // MICRO SIGN — uppercases to Greek Mu, which lowercases to Greek mu
    '\u{0130}', // LATIN CAPITAL LETTER I WITH DOT ABOVE — full lowercase is TWO code points
    '\u{0131}', // LATIN SMALL LETTER DOTLESS I — uppercases to ASCII `I`
    '\u{017F}', // LATIN SMALL LETTER LONG S — uppercases to ASCII `S`
    '\u{0345}', // COMBINING GREEK YPOGEGRAMMENI
    '\u{03A3}', // GREEK CAPITAL LETTER SIGMA — lowercase depends on the NEXT character
    '\u{03C2}', // GREEK SMALL LETTER FINAL SIGMA
    '\u{03D0}', // GREEK BETA SYMBOL
    '\u{03D1}', // GREEK THETA SYMBOL
    '\u{03D5}', // GREEK PHI SYMBOL
    '\u{03D6}', // GREEK PI SYMBOL
    '\u{03F0}', // GREEK KAPPA SYMBOL
    '\u{03F1}', // GREEK RHO SYMBOL
    '\u{03F5}', // GREEK LUNATE EPSILON SYMBOL
    '\u{1C80}', // CYRILLIC SMALL LETTER ROUNDED VE
    '\u{1C81}', // CYRILLIC SMALL LETTER LONG-LEGGED DE
    '\u{1C82}', // CYRILLIC SMALL LETTER NARROW O
    '\u{1C83}', // CYRILLIC SMALL LETTER WIDE ES
    '\u{1C84}', // CYRILLIC SMALL LETTER TALL TE
    '\u{1C85}', // CYRILLIC SMALL LETTER THREE-LEGGED TE
    '\u{1C86}', // CYRILLIC SMALL LETTER TALL HARD SIGN
    '\u{1C87}', // CYRILLIC SMALL LETTER TALL YAT
    '\u{1C88}', // CYRILLIC SMALL LETTER UNBLENDED UK
    '\u{1E9B}', // LATIN SMALL LETTER LONG S WITH DOT ABOVE
    '\u{1FBE}', // GREEK PROSGEGRAMMENI
];

/// Whether `value` carries a character from [`FOLD_DIVERGENT`].
///
/// **Asked of BOTH sides of the comparison**, and that is a measurement rather
/// than caution: the pair `("ſ", "s")` DISAGREES, so a stored value can be pure
/// ASCII while the REQUEST is the odd one — gating only the stored side would have
/// left exactly that pair open. `resolved_tags_for_target` checks the target's
/// three values and `stored_coordinate_is_fold_divergent` checks each stored
/// coordinate; one predicate, two call sites, so they cannot drift.
pub(crate) fn fold_divergent(value: &str) -> bool {
    value.chars().any(|c| FOLD_DIVERGENT.contains(&c))
}

/// The ruling on ONE stored coordinate: `true` when
/// `resolved_tags_for_resource_via`'s predicate cannot be shown to reproduce the
/// enricher for it, which after migration 0177 means exactly one thing — one of
/// its levels carries a character the two case folds can disagree on.
///
/// # ⚠ This ruled on THREE classes until migration 0177 took two of them
///
/// It was `stored_value_problem`, returning `Option<StoredValueProblem>` over
/// `EmptyLevel`, `WildcardMetacharacter` and `FoldDivergentCharacter`. Migration
/// 0177 adds `catalog_tag_assignments_levels_are_present_and_literal` — a
/// VALIDATED `CHECK` forbidding an empty `schema_name`/`table_name` and `*`/`?` in
/// any of the three levels — so the first two are no longer states a running
/// egret can read: unrepresentable beats unforgettable. The fold divergence is the
/// one class that stays here, and the migration's text carries the ruling on why a
/// `CHECK` must not take it over (those 25 code points are legitimate identifiers,
/// and which of them diverge is a property of the deployment's collation rather
/// than of the string).
///
/// # ⚠ Enumerated: every property of a stored value that reaches the comparison
///
/// The two retired classes keep their rows, because the ENUMERATION is what this
/// function is worth: the first version of this gate ruled on emptiness alone and
/// the review measured two more classes of the same shape. A property the matcher
/// applies and this table does not rule on is a fail-open. **A new matcher option
/// in `trino.json` or `starrocks.json`, or a new level in `from_assignments`, is a
/// new row here** — and a row whose ruling is "the schema forbids it" names the
/// constraint that does, so that dropping the constraint is visibly a fail-open
/// rather than a tidy-up.
///
/// | property of the stored value | the matcher's rule | here |
/// |---|---|---|
/// | the `catalog` LEVEL — its emptiness and its syntax | `from_assignments` writes `catalog` **unconditionally** (`egret-policy-api/src/dto.rs`, no `is_empty()` guard), and `trino.json` declares `{"wildCard": true}` on it like the other three — so a catalog named `ord*` would be the same fail-open one level UP, and 0177 does not constrain it | **closed, by a DIFFERENT mechanism, and that is why this row exists** — the value is `registry.catalogs.name` (both readers join `registry.catalogs c` and select `c.name`), and `migrations/0147_catalog_registry.sql` gives that column `CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$')`: an allow-list, so empty, `*` and `?` are all already impossible. ⚠ 0177 could NOT use that predicate for the other three levels — they hold FOREIGN-catalog identifiers where mixed case and non-ASCII are ordinary — so the catalog level is the one protected by an allow-list and the other three by a deny-list. Weakening 0147's regex re-opens this |
/// | emptiness of `schema` / `table` | `from_assignments` omits the level, and the shorter resource is an ANCESTOR the enricher admits — `('sales','','')` ships as `{catalog, schema}`, measured ANCESTOR of `hive.sales.orders.email` on the real jar, and an equality predicate on `table_name` can never return it | **UNREPRESENTABLE since 0177** — `catalog_tag_assignments_levels_are_present_and_literal`. Not ruled on here, and not reachable |
/// | emptiness of `column` | the same omission, but it is migration 0081's documented table-level sentinel and the resolver's second arm IS that ANCESTOR case | **ruled fine, and deliberately still writable** — the one emptiness that is reproduced, and the one 0177's predicate had to keep: a constraint forbidding every empty string would break the table-level attachment |
/// | `*` / `?` | `{"wildCard": true}` on all four `trino.json` resources ⇒ `FilenameUtils.wildcardMatch`, a token-based backtracking matcher that SQL `LIKE` is not (`%`/`_`, no `?`, different escaping) | **UNREPRESENTABLE since 0177**, same constraint. Standing up a second matcher implementation is the trade this package has declined three times — the tag trie, the candidate rule, T4 — so the state is refused at the WRITE instead of reproduced |
/// | letter case | `{"ignoreCase": true}` ⇒ `equalsIgnoreCase` | **reproduced, not refused** — the resolver folds both sides, measured on the Ranger side by `EgretTagAttachmentMatchTypeTest` |
/// | a character from [`FOLD_DIVERGENT`] | `equalsIgnoreCase`'s PER-CHARACTER fold versus `lower()`'s FULL-STRING one — the 25 code points on which the two can disagree, derived by a sweep over all of Unicode | **this function**, and the only class left in it |
/// | any other non-ASCII character | the two folds AGREE on it — measured over every defined code point by the Java sweep against a `lower()` STAND-IN, and over the derived pairs against a real PostgreSQL 16.15 `en_US.utf8`; the caveats for U+0130 and U+03A3 are on [`FOLD_DIVERGENT`] | **ruled fine** — ⚠ fix round 2 narrowed this from "all non-ASCII declines", which was an over-refusal on an un-measured premise in a Vietnamese-language deployment |
/// | leading / trailing whitespace | none: the matcher compares the value as given, and so does the predicate — `' sales'` matches `' sales'` and nothing else on BOTH sides | **ruled fine**, and it is a ruling rather than an omission |
/// | length, `NULL` | `NOT NULL` on all three columns (migration 0081); the matcher has no length rule | **ruled fine** |
/// | `isExcludes` / `isRecursive` | not expressible: `from_assignments` writes `{"values": [v]}` and no flag, so the matcher's defaults (`false`) are what the enricher gets | **ruled fine**, and `ServiceTags::from_assignments` is what makes it so |
///
/// # Why a divergence is a decline in BOTH directions
///
/// The resolver folds with Postgres `lower()`, a FULL-STRING Unicode lowercase;
/// the matcher folds with `StringUtils.equalsIgnoreCase`, a PER-CHARACTER
/// `toUpperCase`/`toLowerCase` pair after a length check. Two different
/// algorithms, so "the query reproduces the matcher" is a claim about where they
/// agree — and neither direction of a disagreement is safe: too wide invents a tag
/// the engine does not apply, too narrow misses one it does.
///
/// # ⚠ Fix round 2 narrowed this from "any non-ASCII value" by MEASURING
///
/// Round 1 declined every non-ASCII stored value, on the un-measured premise that
/// the two folds "are not provably identical outside ASCII". In a
/// Vietnamese-language deployment that is a recurring over-refusal: one tag on a
/// table named `người` would have made every `explain` in that catalog answer
/// `TagsUnresolved`.
///
/// The premise was much narrower than the gate.
/// `tests/ranger/src/test/java/EgretCaseFoldAgreementTest.java` sweeps **every
/// defined code point in Unicode** against three criteria (asymmetric 1:1 mapping,
/// length-changing full lowercase, context-dependent full lowercase) and finds
/// **25**: the Turkish `İ`/`ı`, the long `ſ`, the Greek final sigma and `Σ` itself,
/// the Greek and Cyrillic variant letters, the micro sign, and the Greek
/// ypogegrammeni pair. **No Vietnamese letter is in it** — that test asserts so
/// directly, over both the precomposed letters and the whole `U+1EA0..U+1EF9`
/// tone-mark range — and neither is German `ß`, Cyrillic, or the Kelvin sign.
/// Named pairs in that table, measured: `("I","ı")`, `("İ","i")`, `("ſ","s")`,
/// `("Σ","ς")`, `("ΑΣ","ασ")` and `("µ","μ")` DISAGREE; `("Tiếng","TIẾNG")`,
/// `("người","NGƯỜI")`, `("ß","ẞ")`, `("Д","д")` and `("K","k")` AGREE.
///
/// # ⚠ One side of that measurement is a STAND-IN, and it is unverified
///
/// The matcher's side is exact (`String.equalsIgnoreCase` is what
/// `StringUtils.equalsIgnoreCase` delegates to). The query's side is Java
/// `String.toLowerCase(Locale.ROOT)` standing in for Postgres `lower()`: both
/// implement Unicode's full `Lowercase_Mapping`, `SpecialCasing` entries and the
/// `Final_Sigma` rule included, and `Locale.ROOT` excludes the locale-tailored
/// mappings. **No Postgres runs in that harness** — but ⚠ **the stand-in is not
/// unverified, and the distinction is the point**: SP4d-2b cross-checked the derived
/// pairs against a REAL PostgreSQL 16.15 / `en_US.utf8` by hand, and **51 of the 53
/// disagree there too, with exactly two exceptions, both U+0130's and both
/// conservative** (see the ⚠ block on [`FOLD_DIVERGENT`] for each). So this is a
/// stand-in that has been MEASURED ONCE against the real thing with its divergences
/// named, not a proxy standing alone — and nobody should re-open it believing
/// otherwise. Building a Postgres connection into the Java harness is explicitly
/// NOT wanted: this class's independent ground is that the 25 code points are
/// legitimate identifiers, which does not depend on the proxy at all.
/// **Falsified by** a collation that tailors case (`tr_TR` would move `I`/`i` into
/// the divergent set and is the one deployment where this gate would be too narrow),
/// or by an ICU/glibc difference on one of the 25.
///
/// The REQUEST side is not this function's: see the `target.*` rows on
/// [`resolved_tags_for_target`]. A `*` an operator types is a literal there,
/// because `getMatchType` takes the wildcard from the POLICY position only — and
/// 0177 does not constrain the request, only what can be stored.
pub(crate) fn stored_coordinate_is_fold_divergent(schema: &str, table: &str, column: &str) -> bool {
    [schema, table, column].into_iter().any(fold_divergent)
}

/// `egret_policy::ResolvedTag` → `egret_ranger_eval::eval::ResolvedTag`: two
/// fields in, six out, and every one of the four synthesised fields is a claim
/// about what `ServiceTags::from_assignments` puts on the wire.
///
/// `None` means this attachment is not one this mapping models, which declines
/// the whole resolution in [`resolved_tags_for_target`]. It is not reachable from
/// `resolved_tags_for_resource_via`, whose two arms are `Self_` and `Ancestor`;
/// it exists so that a third arm is a refusal and not a guess.
///
/// # Where the ATTACHMENT's own levels come from
///
/// The levels are a property of the attachment, and
/// `egret_policy::ResolvedTag` carries only `name` and `match_type` — so the
/// match type is where the level survives, and that is sound in exactly one
/// direction:
///
/// | `match_type` | the attachment's stored coordinates | levels |
/// |---|---|---|
/// | `Self_` | the request's own `(schema, table, column)` — the `column_name = $4` arm matched | `catalog` + each of `schema` / `table` / `column` that is non-empty |
/// | `Ancestor` | `(schema, table, '')` — the only other row the query can return is the TABLE-level one | `catalog`, `schema`, `table` |
/// | `Descendant` / `SelfAndAllDescendants` / `None` | — | `None`: no arm of `resolved_tags_for_resource_via` produces one, and a producer that did would be one this mapping has not been measured against |
///
/// # ⚠ The keys are LITERAL, and for StarRocks that is a defect this mapping reproduces rather than hides
///
/// `from_assignments` writes `catalog` / `schema` / `table` / `column` for every
/// service type (`egret-policy-api/src/dto.rs`, and `download_tags` calls it with
/// no engine argument at all). StarRocks' service-def has no `schema` resource —
/// its middle level is `database` — so a StarRocks profile carrying a
/// `tagService` downloads attachments naming a level its own def does not know,
/// and `RangerTagEnricher.processServiceTags` DROPS them:
/// `createRangerServiceResourceMatcher` returns null when no hierarchy of any
/// policy type contains every key, which
/// `EgretTagServiceResourceValidationTest` measures on the real jar (the mirror
/// row, `catalog,database,table,column` against the Trino def, is `dropped`).
///
/// So synthesising `resource_level_names(engine)` here would be a FAIL-OPEN: it
/// would tell a steward the engine evaluates a tag policy that the engine's own
/// enricher has thrown away. The mapping writes what the wire carries, the
/// evaluator's `enriched_tags` drops it against the resource def, and a StarRocks
/// profile therefore answers with its tag policies correctly reported as reaching
/// nothing. **The defect is in the writer** — tag-selected masks and clearances
/// cannot reach a StarRocks engine at all — and it is recorded in Task 7's report,
/// not papered over here.
///
/// **SP4d debt Task 2 made that configuration unrepresentable.**
/// `PolicyStore::create_service` refuses a non-Trino profile carrying a
/// `tagService`, and migration 0176's VALIDATED `CHECK (config_jsonb ->>
/// 'tagService' IS NULL OR type = 'trino')` refuses it for every `INSERT` and every
/// `UPDATE`, from Rust or from `psql`, having first cleared the key from any row
/// that already carried it. This mapping still has to be right rather than merely
/// unused: the levels it synthesises reach `enriched_tags`'
/// `service_resource_validates` for EVERY StarRocks profile with a tagged column —
/// `resolved_tags` comes from `catalog_tag_assignments`, not from the `tagService`
/// — so the drop fires regardless of what any config says. What 0176 removed is the
/// state in which that drop could silently discard a tag policy somebody wrote.
///
/// # ⚠ Enumerated: the four fields with no column to come from
///
/// | field | value | ruling |
/// |---|---|---|
/// | `attributes` | empty | `from_assignments` writes each tag as `{"type": name, "owner": 0}`. There is no attribute on the wire, so an empty map is the payload and not a default |
/// | `options` | empty | likewise absent from the wire. Non-empty would REFUSE in `tag_candidates`, so inventing one would be an over-refusal |
/// | `validity_periods` | `0` | likewise absent |
/// | `attached_through.is_enabled` | `true` | `from_assignments` writes `"isEnabled": true` unconditionally. The field reaches no decision (`processServiceTags` never reads it, SP4d-2b D9); it is set to the wire's value so the inertness stays assertable |
///
/// A field added to `egret_ranger_eval::eval::ResolvedTag` is a compile error
/// here — there is no `Default` — and a new row in the table above.
fn attachment_for(
    tag: &egret_policy::ResolvedTag,
    schema: &str,
    table: &str,
    column: &str,
) -> Option<egret_ranger_eval::eval::ResolvedTag> {
    use egret_ranger_eval::resource::MatchType;

    let attached_column = match tag.match_type {
        MatchType::Self_ => column,
        MatchType::Ancestor => "",
        MatchType::Descendant | MatchType::SelfAndAllDescendants | MatchType::None => return None,
    };
    // Exactly `from_assignments`' rule: `catalog` unconditionally, and each of
    // the other three only when non-empty.
    let levels: BTreeSet<String> = std::iter::once("catalog")
        .chain((!schema.is_empty()).then_some("schema"))
        .chain((!table.is_empty()).then_some("table"))
        .chain((!attached_column.is_empty()).then_some("column"))
        .map(str::to_owned)
        .collect();
    Some(egret_ranger_eval::eval::ResolvedTag {
        name: tag.name.clone(),
        match_type: tag.match_type,
        attributes: BTreeMap::new(),
        options: BTreeSet::new(),
        validity_periods: 0,
        attached_through: TagServiceResource {
            levels,
            is_enabled: true,
        },
    })
}

/// The evaluator's own tag set, as the steward reads it — the list each
/// [`ExplainProfile::tags`] is narrowed from.
///
/// ⚠ This link read `[ExplainResponse::tags]` until SP4d debt Task 7. That field was
/// removed by the H3 fix **in the same squashed commit that wrote this doc**, and
/// `rustdoc` resolves an unknown path in a private item's doc silently, so nothing
/// mechanical saw it — the same commit, and the same blind spot, as the two
/// assertions that task repaired.
///
/// **Task 8's F4/F5 fix, and the direction matters.** This maps the set the
/// evaluator was handed; it does not re-query, re-derive or re-order anything. So
/// the page cannot show a tag the evaluator did not see, nor miss one it did,
/// which is the whole class of defect the two-query arrangement produced.
///
/// `attached_at_column` is `attached_through.levels.contains("column")`, the
/// field [`attachment_for`] wrote from `from_assignments`' rule — see
/// [`ExplainResolvedTag`] for why that, and not `MatchType`, is on the wire.
///
/// Order is the resolution's order, which is
/// `resolved_tags_for_resource_via`'s `ORDER BY`; nothing here sorts, because a
/// second ordering rule is a second thing that can drift from the first.
fn resolved_tags_for_display(tags: &UnenrichedTags) -> Vec<ExplainResolvedTag> {
    tags.as_slice()
        .iter()
        .map(|t| ExplainResolvedTag {
            name: t.name.clone(),
            attached_at_column: t.attached_through.levels.iter().any(|l| l == "column"),
        })
        .collect()
}

/// The resolved set as it reaches ONE profile — the only narrowing this crate can
/// apply to it without reimplementing the evaluator.
///
/// # ⚠ Three lines in a function because the mutation survived, for the fourth time
///
/// This was inline in the handler, and removing the narrowing entirely — handing
/// every profile the full set — **passed the whole suite, 27/0.** The handler body
/// is reachable only from DB-gated tests, which do not run on this branch, so the
/// fix for review H1 could have been undone by deleting four lines. That is the
/// same finding Task 6 made about `attribution_for`, Task 7's fix round 3 made
/// about `explain_tag_inputs`, and the one that moved the stored-coordinate ruling
/// out of SQL: **where a mutation cannot reach, the code is in the wrong shape.**
///
/// # Why zero is the whole predicate
///
/// With no tag policy of ANY kind in this profile's download, the evaluator's
/// `of_kind` is empty for every policy type, so no tag can reach any verdict here
/// and the empty list cannot disagree with it. It is a ONE-WAY implication, not a
/// reimplementation: anything finer — per access type, or the `enriched_tags` drop
/// review H2 names — is the evaluator's own rule and stays there, because a second
/// copy of it is the shape this package has refused four times.
fn tags_for_profile(
    resolved: &[ExplainResolvedTag],
    tag_policies_in_download: usize,
) -> Vec<ExplainResolvedTag> {
    if tag_policies_in_download == 0 {
        Vec::new()
    } else {
        resolved.to_vec()
    }
}

/// The two reads [`resolved_tags_for_target`] needs, behind a trait so that the
/// GATE, the mapping and the wiring between them are testable with no database.
///
/// Same seam and same reason as [`ClearanceLookups`]: fix round 1 of Task 6
/// measured that a `&mut PgConnection` parameter makes its call site reachable
/// only from a DB-gated test, where a mutation that drops a call survives in
/// silence. Dropping the gate call here would be a fail-open, so it is the first
/// thing that had to be coverable.
///
/// Both methods are `-> impl Future + Send` rather than `async fn` for the reason
/// on [`ClearanceLookups`]: the caller is an axum handler.
trait TagLookups {
    /// `tag_assignment_coordinates_via` — every distinct
    /// `(schema_name, table_name, column_name)` this catalog's tag assignments
    /// carry. The RULING on them is [`stored_coordinate_is_fold_divergent`]'s, in
    /// Rust, so that it is testable and mutable; this is the fact. ⚠ It used to be a
    /// three-class ruling; migration 0177 took two of the three into a `CHECK`, and
    /// the read survives for the one a `CHECK` must not take.
    fn assignment_coordinates(
        &mut self,
        catalog_id: Uuid,
    ) -> impl std::future::Future<Output = Result<Vec<(String, String, String)>, PermHttpError>> + Send;

    /// `resolved_tags_for_resource_via` — the enricher-equivalent set for ONE
    /// resource, as `(tags.name, MatchType)` pairs.
    fn resolved_tags(
        &mut self,
        catalog_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
    ) -> impl std::future::Future<Output = Result<Vec<egret_policy::ResolvedTag>, PermHttpError>> + Send;
}

/// The production implementation: the two store queries, unchanged.
impl TagLookups for sqlx::PgConnection {
    async fn assignment_coordinates(
        &mut self,
        catalog_id: Uuid,
    ) -> Result<Vec<(String, String, String)>, PermHttpError> {
        Ok(tag_assignment_coordinates_via(self, catalog_id).await?)
    }

    async fn resolved_tags(
        &mut self,
        catalog_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<Vec<egret_policy::ResolvedTag>, PermHttpError> {
        Ok(resolved_tags_for_resource_via(self, catalog_id, schema, table, column).await?)
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
        (status = 409, description = "A rule overlap: two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
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

/// SP4d-3 Task 5: every policy of one profile, classified — its owner,
/// whether it can be migrated into the permission model, and if not, every
/// reason as a sentence naming the offending value. Read-only; `infra` and
/// `compiler` rows are reported (as not migratable, with the reason), never
/// omitted. The `classify` module doc says how the service kind, the catalog
/// lookup and the tag lookup are derived from the profile.
#[utoipa::path(
    get,
    path = "/api/control-plane/perm/policies/services/{service_id}/classify",
    tag = "perm",
    params(("service_id" = i64, Path, description = "policy.authz_services id")),
    responses(
        (status = 200, description = "Every policy of the profile, ordered by id, with its owner, verdict and blocker sentences", body = Vec<crate::classify::ClassifiedPolicy>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn classify_service_policies(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
) -> Result<Json<Vec<crate::classify::ClassifiedPolicy>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    Ok(Json(
        crate::classify::classify_service(&mut conn, service_id).await?,
    ))
}

/// SP4d-4 Task 11: the policies of one profile whose stored resource levels
/// Ranger reads differently from how the editor shows them — a policy that
/// stops above `column` and allows `select` (it grants no column read), or one
/// with a gap (it decides nothing) — with `warningCount` for the profile.
/// Read-only: a stored row is flagged here, never rewritten. The same
/// `warnings` ride on each policy of `GET .../classify`.
#[utoipa::path(
    get,
    path = "/api/control-plane/perm/policies/services/{service_id}/warnings",
    tag = "perm",
    params(("service_id" = i64, Path, description = "policy.authz_services id")),
    responses(
        (status = 200, description = "`warningCount` and, ordered by id, the policies of the profile that carry a warning", body = crate::classify::ProfileWarnings),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn service_policy_warnings(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
) -> Result<Json<crate::classify::ProfileWarnings>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    Ok(Json(
        crate::classify::profile_warnings(&mut conn, service_id).await?,
    ))
}

/// SP4d-3 Task 7: migrate hand-written policies of one profile into the
/// permission model, PROVABLY — see `crate::migrate` for the transaction, its
/// order, and the refusal order. The equivalence gate runs inside the
/// transaction over every profile the new objects compile into, against the
/// downloads as they stand after the write.
///
/// **`dry_run` runs the identical code and differs in exactly one place: the
/// line below that decides to commit.** A proven migration answers 200; one
/// the gate did not prove answers 422 with the same body shape (its
/// disagreements, or why the gate refused), and nothing is written.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/policies/services/{service_id}/migrate",
    tag = "perm",
    params(("service_id" = i64, Path, description = "policy.authz_services id")),
    request_body = crate::migrate::MigrateRequest,
    responses(
        (status = 200, description = "Proved equivalent on every affected profile; committed unless `dry_run`", body = crate::migrate::MigrateResponse),
        (status = 400, description = "A named policy is not migratable (the classifier's sentences), or the request names no policy / a policy twice", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such profile, or no policy of that name in it", body = ErrorBody),
        (status = 409, description = "Already migrated, not hand-written (compiler or infra owned), or a rule name it would create is taken; or two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
        (status = 422, description = "Not proved: the disagreements on any affected profile, or why the gate could not answer. Nothing was written", body = crate::migrate::MigrateResponse),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn migrate_service_policies(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
    Json(body): Json<crate::migrate::MigrateRequest>,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::migrate::MigrateResponse>,
    ),
    PermHttpError,
> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let mut response = crate::migrate::migrate_in_tx(&mut tx, service_id, &body, &p).await?;
    // THE one place a dry run differs.
    let commit = response.equivalent && !body.dry_run;
    if commit {
        tx.commit().await.map_err(PermHttpError::db)?;
    } else {
        tx.rollback().await.map_err(PermHttpError::db)?;
    }
    response.committed = commit;
    let status = if response.equivalent {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    };
    Ok((status, Json(response)))
}

/// SP4d-3 Task 7: undo migrations — delete the grants and rules provenance
/// names, recompile, switch each original back on, delete the provenance. One
/// transaction.
#[utoipa::path(
    post,
    path = "/api/control-plane/perm/policies/services/{service_id}/migrate/undo",
    tag = "perm",
    params(("service_id" = i64, Path, description = "policy.authz_services id")),
    request_body = crate::migrate::UndoRequest,
    responses(
        (status = 200, description = "Every named migration undone", body = crate::migrate::UndoResponse),
        (status = 400, description = "The request names no policy, or a policy twice", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such profile, or no migration on record for a named policy", body = ErrorBody),
        (status = 409, description = "Nothing was changed: an object the migration created was edited or deleted since, or the original was edited or switched on (each change is named); or a profile became reachable from the migrated objects while undo waited for its locks (retry); or two mask (or two row-filter) rules would decide one request differently: this write would compile such a pair, or a profile it recompiles already holds one, which refuses every recompile of that profile until one rule is narrowed, deleted or made to agree, or a tag unassigned. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn undo_service_policy_migration(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
    Json(body): Json<crate::migrate::UndoRequest>,
) -> Result<Json<crate::migrate::UndoResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let response = crate::migrate::undo_in_tx(&mut tx, service_id, &body).await?;
    tx.commit().await.map_err(PermHttpError::db)?;
    Ok(Json(response))
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

/// `validate_explain_target` is pure, so the whole refusal matrix is exercised
/// here rather than one row per HTTP round trip. **The HTTP tests still exist**
/// (`tests/routes.rs`): a `--lib` green says the function refuses, not that the
/// handler calls it, and a guard nobody wired in is the defect this split is most
/// likely to hide.
#[cfg(test)]
mod explain_target_tests {
    use super::{requests_all_values, validate_explain_target, PermHttpError, Scope};
    use uuid::Uuid;

    fn target(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> Scope {
        Scope {
            catalog_id: Uuid::nil(),
            schema: schema.map(str::to_string),
            table: table.map(str::to_string),
            column: column.map(str::to_string),
        }
    }

    fn refusal(t: &Scope) -> String {
        match validate_explain_target(t) {
            Err(PermHttpError::BadRequest(m)) => m,
            other => panic!("expected a 400 for {t:?}, got {other:?}"),
        }
    }

    /// The domain this guard relies on, asserted against the EVALUATOR'S OWN
    /// exported predicate — `egret_ranger_eval::resource::requests_all_values` —
    /// because that is the function the guard calls. There is no local copy left
    /// to agree with.
    ///
    /// The two present arms are load-bearing from here: delete either one in
    /// `egret-ranger-eval` and three rows in THIS crate die, alongside the
    /// `applyExcludes` probes in that crate. That cross-crate kill is the whole
    /// reason the predicate is exported rather than re-spelled (coordinator
    /// overrule of concern 3) — before it, one mutation killed only the crate it
    /// was made in.
    #[test]
    fn all_values_requested_is_the_empty_string_and_the_star() {
        assert!(requests_all_values(Some("")));
        assert!(requests_all_values(Some("*")));
        // The `None` arm is part of the rule and NOT part of this guard: the loop
        // in `validate_explain_target` unwraps first, because an absent level is
        // the level-skip rule's business and an absent tail is answerable.
        assert!(requests_all_values(None));
        // Everything else is a NAME, including the shapes that look like one of
        // the two but are not: Ranger's `isAllValuesRequested` is an equality
        // test, not a wildcard test.
        for name in ["sales", " ", "**", "*a", "a*", "\t", "0"] {
            assert!(
                !requests_all_values(Some(name)),
                "{name:?} is a literal value the matcher compares literally"
            );
        }
    }

    #[test]
    fn an_empty_level_is_refused_at_every_level_that_can_carry_one() {
        for (label, t) in [
            ("schema", target(Some(""), None, None)),
            ("table", target(Some("sales"), Some(""), None)),
            ("column", target(Some("sales"), Some("orders"), Some(""))),
        ] {
            let msg = refusal(&t);
            assert!(
                msg.contains(&format!("`{label}`")),
                "the refusal has to name WHICH level: {msg}"
            );
            assert!(
                msg.contains("isAllValuesRequested"),
                "and why, by naming the Ranger predicate it is keyed to: {msg}"
            );
        }
    }

    #[test]
    fn a_star_level_is_refused_at_every_level_that_can_carry_one() {
        for (label, t) in [
            ("schema", target(Some("*"), None, None)),
            ("table", target(Some("sales"), Some("*"), None)),
            ("column", target(Some("sales"), Some("orders"), Some("*"))),
        ] {
            assert!(
                refusal(&t).contains(&format!("`{label}`")),
                "{label} named as `*` is Ranger's every-value, not a resource name"
            );
        }
    }

    /// The deepest level is checked too — a loop that stopped at the first
    /// `None` would pass every other row here and still let `{schema: "sales",
    /// table: "orders", column: ""}` through.
    #[test]
    fn a_deep_level_is_not_skipped_because_the_shallower_ones_are_fine() {
        assert!(
            validate_explain_target(&target(Some("sales"), Some("orders"), Some("amount"))).is_ok()
        );
        refusal(&target(Some("sales"), Some("orders"), Some("")));
        refusal(&target(Some("sales"), Some("orders"), Some("*")));
    }

    /// The level-skip rule, moved into this function unchanged. Its own HTTP test
    /// (`a_target_that_skips_a_level_is_refused`) stays where it is; this pins that
    /// moving it did not drop a row.
    #[test]
    fn a_skipped_level_is_still_refused_and_says_so_differently() {
        for t in [
            target(None, Some("orders"), None),
            target(Some("sales"), None, Some("amount")),
            target(None, None, Some("amount")),
        ] {
            assert!(
                refusal(&t).contains("cannot skip a level"),
                "a gap and an all-values level are different refusals and must read \
                 differently — a steward fixes them differently"
            );
        }
    }

    /// **The guard cannot be satisfied by refusing everything.** Each of these is a
    /// resource an engine request really carries, and one of them is the only
    /// shape the `Explain access` page sends for a catalog-wide question.
    #[test]
    fn every_shape_an_engine_request_can_carry_is_still_answerable() {
        for t in [
            target(None, None, None),
            target(Some("sales"), None, None),
            target(Some("sales"), Some("orders"), None),
            target(Some("sales"), Some("orders"), Some("amount")),
            // Case is NOT normalised and NOT refused: the tag resolver folds both
            // sides, and `request_resource` hands the value to a matcher whose
            // `ignore_case` comes from the service-def. Refusing it here would be
            // a second, divergent opinion about case.
            target(Some("SALES"), Some("Orders"), Some("AMOUNT")),
            // Whitespace and a `*` INSIDE a name are literals, not the all-values
            // form — see `egret_ranger_eval::resource::requests_all_values`.
            target(Some(" "), Some("a*b"), Some("**")),
        ] {
            assert!(
                validate_explain_target(&t).is_ok(),
                "{t:?} names a resource an engine request can carry"
            );
        }
    }
}

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

// ── the tag resolution, with no database ─────────────────────────────────
//
// SP4d-2b Task 7. Two things needed covering and neither could be reached from
// a DB-gated test: that the GATE runs before any resolution (dropping that call
// is a fail-open), and the `egret_policy::ResolvedTag` →
// `egret_ranger_eval::eval::ResolvedTag` mapping, which synthesises four fields
// out of nothing but a measurement of what `ServiceTags::from_assignments`
// writes. `TagLookups` is the seam; the fake records what it was asked, because
// "the resolver was never called" is a claim about behaviour that a test reading
// only the return value cannot make.

#[cfg(test)]
mod tag_resolution_tests {
    use super::{
        attachment_for, resolved_tags_for_display, resolved_tags_for_target,
        stored_coordinate_is_fold_divergent, ExplainResolvedTag, ExplainTagDecline,
        ExplainTargetLevel, PermHttpError, Scope, TagLookups, UnenrichedTags,
    };
    use egret_ranger_eval::resource::MatchType;
    use uuid::Uuid;

    const CATALOG: &str = "33333333-3333-4333-8333-333333333333";

    /// Stands in for the two store reads, and records both.
    struct FakeTagLookups {
        coordinates: Vec<(String, String, String)>,
        resolved: Vec<egret_policy::ResolvedTag>,
        gate_asked: Vec<Uuid>,
        resolved_asked: Vec<(Uuid, String, String, String)>,
    }

    impl FakeTagLookups {
        fn new(coordinates: &[(&str, &str, &str)], resolved: &[(&str, MatchType)]) -> Self {
            Self {
                coordinates: coordinates
                    .iter()
                    .map(|(s, t, c)| ((*s).to_string(), (*t).to_string(), (*c).to_string()))
                    .collect(),
                resolved: resolved
                    .iter()
                    .map(|(name, match_type)| egret_policy::ResolvedTag {
                        name: (*name).to_string(),
                        match_type: *match_type,
                    })
                    .collect(),
                gate_asked: Vec::new(),
                resolved_asked: Vec::new(),
            }
        }

        /// One ordinary column-level assignment: the coordinate every test that
        /// is not about the gate wants, so that "the gate let it through" is not
        /// the vacuous case of there being nothing stored.
        fn clean(resolved: &[(&str, MatchType)]) -> Self {
            Self::new(&[("sales", "orders", "email")], resolved)
        }
    }

    impl TagLookups for FakeTagLookups {
        async fn assignment_coordinates(
            &mut self,
            catalog_id: Uuid,
        ) -> Result<Vec<(String, String, String)>, PermHttpError> {
            self.gate_asked.push(catalog_id);
            Ok(self.coordinates.clone())
        }

        async fn resolved_tags(
            &mut self,
            catalog_id: Uuid,
            schema: &str,
            table: &str,
            column: &str,
        ) -> Result<Vec<egret_policy::ResolvedTag>, PermHttpError> {
            self.resolved_asked.push((
                catalog_id,
                schema.to_string(),
                table.to_string(),
                column.to_string(),
            ));
            Ok(self.resolved.clone())
        }
    }

    fn target(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> Scope {
        Scope {
            catalog_id: Uuid::parse_str(CATALOG).unwrap(),
            schema: schema.map(str::to_string),
            table: table.map(str::to_string),
            column: column.map(str::to_string),
        }
    }

    fn levels(tag: &egret_ranger_eval::eval::ResolvedTag) -> Vec<&str> {
        tag.attached_through
            .levels
            .iter()
            .map(String::as_str)
            .collect()
    }

    /// **The fail-open this seam exists for — the one class of it the schema
    /// cannot take.**
    ///
    /// Each row below is a stored coordinate the enricher's matcher ADMITS for the
    /// target `catalog/sales/orders/email` and the resolver's equality predicate
    /// can never return, so passing the resolved set would pass a set MISSING a tag
    /// the engine enforces — and an empty set is the licence to skip the tag phase
    /// entirely.
    ///
    /// ⚠ **This had EIGHT rows until SP4d debt Task 3, covering three classes.**
    /// Seven of them — `('sales','','')`, `('','','')`, `('sales','*','')`,
    /// `('sales','ord*','')`, `('*','*','')`, `('sales','order?','')` and
    /// `('sales','orders','ema*')` — are coordinates migration 0177's
    /// `catalog_tag_assignments_levels_are_present_and_literal` now refuses to
    /// store. A test that feeds them to the fake lookup asserts a branch no
    /// production read can produce, which is the shape that lets a stale
    /// expectation outlive its subject. They are not deleted silently: the
    /// enumeration of what the matcher admits, including those coordinates and the
    /// measurements behind them, is on `stored_coordinate_is_fold_divergent`, and
    /// `the_database_refuses_a_coordinate_the_resolver_cannot_reproduce` in
    /// `crates/egret-policy/tests/tag_resolution.rs` is where they are now measured
    /// — against the real `CHECK`, which is a stronger instrument than this fake.
    ///
    /// What is left is the class a `CHECK` must NOT take: a character on which
    /// Postgres `lower()` and Java `equalsIgnoreCase` can disagree. Deleting the
    /// gate call, or narrowing `fold_divergent`, makes this test report a tag set
    /// where it must report "did not resolve".
    #[tokio::test]
    async fn a_fold_divergent_stored_coordinate_declines_the_whole_resolution() {
        // ⚠ The decline each row must REPORT, written out per coordinate rather
        // than derived from the predicate under test (fix round 3). Deriving it
        // would be a correlated oracle — the function under test would be asked to
        // grade its own answer.
        for (coordinate, why) in [
            (
                ("sales", "orders", "ema\u{0131}l"),
                "the Turkish dotless i in the COLUMN: uppercases to ASCII `I`",
            ),
            (
                ("sale\u{017f}", "orders", ""),
                "the long s in the SCHEMA: uppercases to ASCII `S`",
            ),
            (
                ("sales", "order\u{03a3}", ""),
                "capital sigma in the TABLE: its lowercase depends on the next character",
            ),
        ] {
            let (schema, table, column) = coordinate;
            let mut lookups = FakeTagLookups::new(&[coordinate], &[("pii", MatchType::Self_)]);
            let got = resolved_tags_for_target(
                &mut lookups,
                &target(Some("sales"), Some("orders"), Some("email")),
            )
            .await
            .expect("no error");
            // ⚠ The VARIANT and the COORDINATE, not merely "declined" (fix round
            // 3). The states were collapsed by an `.is_some()` at the call site,
            // and the page then named a stored row that did not exist and offered a
            // remedy already satisfied. An assertion that only checked for a
            // decline could not have caught that — and did not.
            assert_eq!(
                got.as_ref().err(),
                Some(&ExplainTagDecline::StoredFoldDivergent {
                    schema: schema.to_string(),
                    table: table.to_string(),
                    column: column.to_string(),
                }),
                "{coordinate:?} must decline and name itself ({why}), got {got:?}"
            );
            assert_eq!(
                lookups.gate_asked,
                vec![Uuid::parse_str(CATALOG).unwrap()],
                "the gate is asked, and asked about the target's catalog"
            );
            assert!(
                lookups.resolved_asked.is_empty(),
                "nothing may be resolved once the equivalence is known not to hold: {coordinate:?}"
            );
        }
    }

    /// **The class that must NOT be gated, and this is the test that keeps it
    /// that way.** The matcher's `ignoreCase: true` is REPRODUCED — the resolver
    /// folds both sides — so a row stored `SALES`/`ORDERS`/`EMAIL` is a coordinate
    /// the predicate handles, not one it declines.
    ///
    /// Gating it instead would have been the obvious extra class and it would be
    /// wrong in a way no fail-open test catches: `ScopePicker` renders the three
    /// levels as free-text inputs and `request_resource` inserts them verbatim, so
    /// differing case is what ORDINARY use produces. A steward who typed `SALES`
    /// would get "cannot decide" for a question egret can answer.
    #[tokio::test]
    async fn differing_case_is_reproduced_rather_than_declined() {
        let mut lookups = FakeTagLookups::new(
            &[("SALES", "ORDERS", "EMAIL")],
            &[("pii", MatchType::Self_)],
        );
        let got = resolved_tags_for_target(
            &mut lookups,
            &target(Some("sales"), Some("orders"), Some("email")),
        )
        .await
        .expect("no error")
        .expect("a case difference is not a reason to decline");
        assert_eq!(got.as_slice().len(), 1, "and the tag still comes through");
        assert_eq!(
            lookups.resolved_asked.len(),
            1,
            "the resolver is asked — the fold is ITS job, not the gate's"
        );
    }

    /// **The wiring a surviving mutation found (fix round 3).** Dropping the
    /// decline on its way to the response — `Err(_) => (None, None)` — passed the
    /// entire suite, because the handler body is reachable only from DB-gated
    /// tests. Both arms are asserted here, and the second assertion is the one that
    /// matters: a reason that does not reach the response is a page that renders
    /// round 1's sentence for every decline state, which is the defect this round
    /// exists to fix.
    #[test]
    fn the_decline_reason_reaches_the_response_and_the_tags_do_not() {
        let decline = ExplainTagDecline::StoredFoldDivergent {
            schema: "sales".to_string(),
            table: "orders".to_string(),
            column: "ema\u{0131}l".to_string(),
        };
        let (tags, declined) = super::explain_tag_inputs(Err(decline.clone()));
        assert!(
            tags.is_none(),
            "a decline must not hand the evaluator a tag set"
        );
        assert_eq!(
            declined,
            Some(decline),
            "and the REASON must reach the response, verbatim"
        );

        let (tags, declined) = super::explain_tag_inputs(Ok(UnenrichedTags::new(Vec::new())));
        assert_eq!(
            tags.map(|t| t.as_slice().len()),
            Some(0),
            "a resolution hands the evaluator its set, empty or not"
        );
        assert!(
            declined.is_none(),
            "and reports no decline — the page keys the reason on this being present"
        );
    }

    /// **Task 8's F4/F5.** [`ExplainProfile::tags`] is the evaluator's own set now,
    /// and the field a steward acts on is `attached_at_column`: `true` says
    /// "change this column's assignment", `false` says "change the TABLE's". The
    /// browser measured the old field reporting "None." next to a deny driven by a
    /// table-level attachment, so the ANCESTOR row here is the one that matters.
    ///
    /// Asserted through `attachment_for`, not by hand-building a `ResolvedTag`:
    /// the bool reads back `attached_through.levels`, and building that literal
    /// here would assert the mapping against itself.
    #[test]
    fn the_reported_tag_set_says_whether_the_attachment_names_the_column() {
        let on_the_column = attachment_for(
            &egret_policy::ResolvedTag {
                name: "pii".to_string(),
                match_type: MatchType::Self_,
            },
            "sales",
            "orders",
            "email",
        )
        .expect("SELF maps");
        let on_the_table = attachment_for(
            &egret_policy::ResolvedTag {
                name: "ancdeny".to_string(),
                match_type: MatchType::Ancestor,
            },
            "sales",
            "orders",
            "email",
        )
        .expect("ANCESTOR maps");

        let reported =
            resolved_tags_for_display(&UnenrichedTags::new(vec![on_the_column, on_the_table]));
        assert_eq!(
            reported,
            vec![
                ExplainResolvedTag {
                    name: "pii".to_string(),
                    attached_at_column: true,
                },
                ExplainResolvedTag {
                    name: "ancdeny".to_string(),
                    attached_at_column: false,
                },
            ],
            "the column-level attachment reports true, the table-level one false, \
             and the resolution's ORDER is preserved rather than re-sorted"
        );
    }

    /// **Whole-branch review H1, and the seam exists because the mutation
    /// survived.** Removing the narrowing — handing every profile the full
    /// resolved set — passed the whole suite 27/0 while it was inline in the
    /// handler. Both arms are asserted here with no database, which is what makes
    /// the fix mutable; `0` is the load-bearing one, because that is the profile
    /// the page must say "none, and here is why" about rather than listing tags
    /// nothing there could consult.
    #[test]
    fn a_profile_with_no_tag_policies_is_handed_no_tags() {
        let resolved = vec![
            ExplainResolvedTag {
                name: "pii".to_string(),
                attached_at_column: true,
            },
            ExplainResolvedTag {
                name: "ancdeny".to_string(),
                attached_at_column: false,
            },
        ];
        assert!(
            super::tags_for_profile(&resolved, 0).is_empty(),
            "a download with no tag policy of any kind cannot have consulted a tag, \
             so the profile is handed none — the H1 claim that was false"
        );
        assert_eq!(
            super::tags_for_profile(&resolved, 1),
            resolved,
            "and with a tag policy present the profile gets the resolution verbatim, \
             in its order, neither re-sorted nor filtered further — anything finer is \
             the evaluator's rule, not this one's"
        );
    }

    /// **Task 8, concern 4 — and the concern was half wrong, which is why this
    /// test exists in this shape.**
    ///
    /// I reported that `assignment_coordinates`' row order was unpinned, so a
    /// catalog with two differently-broken assignments would name one of them
    /// arbitrarily. `tag_assignment_coordinates_via` has carried
    /// `ORDER BY a.schema_name, a.table_name, a.column_name` all along, so the
    /// SQL side was pinned before I said it was not. What was genuinely unpinned
    /// is the RULE on this side: `find_map` takes the first offender, and nothing
    /// said so or would have failed if it had become the last.
    ///
    /// Both orders are asserted, which is what makes this a test of the rule
    /// rather than of the fixture: the SAME pair of offenders handed over in the
    /// opposite order must name the OTHER row. A `find` changed to `.last()` fails
    /// one of the two.
    ///
    /// ⚠ **The pair used to be one wildcard row and one fold-divergent row**, which
    /// also disproved a per-CLASS precedence. Migration 0177 makes the wildcard row
    /// unstorable, so there is one class left and no precedence left to disprove;
    /// the two offenders are now two fold-divergent rows carrying the divergent
    /// character at DIFFERENT levels, which is what the rule still has to order.
    #[tokio::test]
    async fn the_first_offending_coordinate_is_the_one_named() {
        // Two offenders that differ in which LEVEL carries the divergent character,
        // so neither can be named by a rule that happens to look at one level.
        let schema_first = [("saleſ", "orders", ""), ("sales", "orders", "emaıl")];
        let column_first = [("sales", "orders", "emaıl"), ("saleſ", "orders", "")];

        let mut lookups = FakeTagLookups::new(&schema_first, &[]);
        let got = resolved_tags_for_target(
            &mut lookups,
            &target(Some("sales"), Some("orders"), Some("email")),
        )
        .await
        .expect("no error");
        assert_eq!(
            got,
            Err(ExplainTagDecline::StoredFoldDivergent {
                schema: "saleſ".to_string(),
                table: "orders".to_string(),
                column: String::new(),
            }),
            "the FIRST offender in the ordered list is the one the page names"
        );

        let mut lookups = FakeTagLookups::new(&column_first, &[]);
        let got = resolved_tags_for_target(
            &mut lookups,
            &target(Some("sales"), Some("orders"), Some("email")),
        )
        .await
        .expect("no error");
        assert_eq!(
            got,
            Err(ExplainTagDecline::StoredFoldDivergent {
                schema: "sales".to_string(),
                table: "orders".to_string(),
                column: "emaıl".to_string(),
            }),
            "and with the same two rows in the other order it is the other one — \
             so this pins first-wins rather than a preference between the levels"
        );
    }

    /// The ruling on the function that owns it. The end-to-end tests above drive it
    /// through `resolved_tags_for_target`; this one asks it directly, so that a
    /// mutation which widens or narrows the predicate is visible here rather than
    /// only through a decline.
    ///
    /// ⚠ **This was `stored_value_problem_rules_on_each_class_by_name` and asserted
    /// three classes.** Migration 0177 took two of them into the schema, so the
    /// rows that used to live here — `("", "orders", "")`, `("sales", "", "")`,
    /// `("sal*s", "orders", "")`, `("sales", "orders", "ema?l")` — are now measured
    /// against the real `CHECK` in `crates/egret-policy/tests/tag_resolution.rs`,
    /// which is a stronger instrument than a direct call. What is still a RULING on
    /// this side, and could still be got wrong here, is below: which characters
    /// diverge, and which of the reproduced properties must NOT decline.
    #[test]
    fn a_coordinate_is_fold_divergent_only_where_the_two_folds_can_disagree() {
        assert!(!stored_coordinate_is_fold_divergent(
            "sales", "orders", "email"
        ));
        // `column_name = ''` is migration 0081's table-level sentinel and the
        // resolver's own second arm — the one emptiness that is REPRODUCED, and the
        // one 0177's CHECK deliberately still allows.
        assert!(!stored_coordinate_is_fold_divergent("sales", "orders", ""));
        // ⚠ Fix round 2: an ordinary non-ASCII value is NOT a problem. The two
        // folds AGREE on every Vietnamese letter — measured over the precomposed
        // letters and the whole U+1EA0..U+1EF9 tone range by
        // `EgretCaseFoldAgreementTest::noVietnameseLetterIsInTheDivergentSet` — and
        // round 1 declined all of them, which in this deployment is a tag on
        // `người` refusing the whole catalog.
        assert!(!stored_coordinate_is_fold_divergent(
            "b\u{00e1}n_h\u{00e0}ng",
            "ng\u{01b0}\u{1edd}i",
            ""
        ));
        assert!(!stored_coordinate_is_fold_divergent(
            "Ti\u{1ebf}ng",
            "TI\u{1ebe}NG",
            ""
        ));
        // The 25 that do diverge, one per mechanism — and asked of each of the three
        // LEVELS, since the predicate's job is to look at all three.
        for divergent in [
            "\u{0130}", // full lowercase is TWO code points
            "\u{0131}", // uppercases to ASCII `I`
            "\u{017f}", // uppercases to ASCII `S`
            "\u{03c2}", // final sigma
            "\u{03a3}", // lowercase depends on the NEXT character
            "\u{00b5}", // micro sign
            "\u{1c80}", // Cyrillic rounded ve
        ] {
            let odd = format!("sa{divergent}les");
            assert!(
                stored_coordinate_is_fold_divergent(&odd, "orders", ""),
                "{divergent:?} in the SCHEMA is in FOLD_DIVERGENT"
            );
            assert!(
                stored_coordinate_is_fold_divergent("sales", &odd, ""),
                "{divergent:?} in the TABLE is in FOLD_DIVERGENT"
            );
            assert!(
                stored_coordinate_is_fold_divergent("sales", "orders", &odd),
                "{divergent:?} in the COLUMN is in FOLD_DIVERGENT"
            );
        }
        // Case is NOT a problem: it is reproduced by the resolver's fold.
        assert!(!stored_coordinate_is_fold_divergent(
            "SALES", "ORDERS", "EMAIL"
        ));
        // Whitespace is NOT a problem: both sides compare the value as given.
        assert!(!stored_coordinate_is_fold_divergent(
            " sales", "orders ", ""
        ));
    }

    /// **The REQUEST side of the fold, which round 2 added and round 1 did not
    /// have.** The divergence is a property of the PAIR: `("\u{017f}", "s")` is
    /// measured DISAGREEING, so a catalog whose every stored coordinate is ordinary
    /// ASCII is still unsafe for a target that carries one of the 25 — the stored
    /// side cannot see it, and nothing else would have declined.
    #[tokio::test]
    async fn a_fold_divergent_character_in_the_target_declines_the_resolution() {
        for value in ["\u{017f}ales", "\u{0131}", "\u{03a3}igma"] {
            for (level, t) in [
                (
                    ExplainTargetLevel::Schema,
                    target(Some(value), Some("orders"), Some("email")),
                ),
                (
                    ExplainTargetLevel::Table,
                    target(Some("sales"), Some(value), Some("email")),
                ),
                (
                    ExplainTargetLevel::Column,
                    target(Some("sales"), Some("orders"), Some(value)),
                ),
            ] {
                let mut lookups = FakeTagLookups::clean(&[("pii", MatchType::Self_)]);
                let got = resolved_tags_for_target(&mut lookups, &t)
                    .await
                    .expect("no error");
                // The LEVEL is part of the answer: the page names the box the
                // steward typed it in, and this is the one decline whose reach is
                // the REQUEST rather than the whole catalog.
                assert_eq!(
                    got.as_ref().err(),
                    Some(&ExplainTagDecline::TargetFoldDivergent {
                        level,
                        value: value.to_string(),
                    }),
                    "a divergent target value must decline and name its level: {t:?}"
                );
                assert!(
                    lookups.resolved_asked.is_empty(),
                    "and decline before resolving: {t:?}"
                );
            }
        }
    }

    /// …and the other half of that, which is the whole point of round 2: an
    /// ORDINARY non-ASCII target resolves. Without this the request-side check is
    /// indistinguishable from "decline anything with a diacritic", which is what
    /// round 1 did and what this deployment cannot afford.
    #[tokio::test]
    async fn an_ordinary_vietnamese_target_still_resolves() {
        let mut lookups = FakeTagLookups::new(
            &[("b\u{00e1}n_h\u{00e0}ng", "ng\u{01b0}\u{1edd}i", "")],
            &[("pii", MatchType::Self_)],
        );
        let got = resolved_tags_for_target(
            &mut lookups,
            &target(
                Some("B\u{00c1}N_H\u{00c0}NG"),
                Some("NG\u{01af}\u{1edc}I"),
                None,
            ),
        )
        .await
        .expect("no error")
        .expect("Vietnamese is not a reason to decline, in either case");
        assert_eq!(got.as_slice().len(), 1);
    }

    /// The gate is the FIRST read, not a second opinion on a set already
    /// fetched: with every coordinate reproducible the resolver is asked exactly
    /// once, with the target's own four values and `column = ""` for a
    /// table-level target.
    #[tokio::test]
    async fn a_clear_gate_resolves_once_with_the_targets_own_levels() {
        let mut lookups = FakeTagLookups::clean(&[]);
        let got = resolved_tags_for_target(&mut lookups, &target(Some("s"), Some("t"), None))
            .await
            .expect("no error");
        assert_eq!(got.map(|t| t.as_slice().len()), Ok(0));
        assert_eq!(
            lookups.resolved_asked,
            vec![(
                Uuid::parse_str(CATALOG).unwrap(),
                "s".to_string(),
                "t".to_string(),
                String::new()
            )]
        );
    }

    /// A COLUMN attachment and a TABLE attachment on one request, which is the
    /// shape `resolved_tags_for_resource_via` returns for a column target, and
    /// the two level sets `ServiceTags::from_assignments` writes for them. The
    /// match type is the only record of which level the attachment was at, so
    /// mapping `Ancestor` onto four levels (or `Self_` onto three) would ship a
    /// provenance claim that is false — and the four synthesised fields are the
    /// wire's values, not a `Default`.
    #[tokio::test]
    async fn self_maps_to_four_levels_and_ancestor_to_three() {
        let mut lookups =
            FakeTagLookups::clean(&[("pii", MatchType::Self_), ("ssn", MatchType::Ancestor)]);
        let got = resolved_tags_for_target(&mut lookups, &target(Some("s"), Some("t"), Some("c")))
            .await
            .expect("no error")
            .expect("the gate is clear, so a set is offered");
        let tags = got.as_slice();
        assert_eq!(tags.len(), 2);

        assert_eq!(tags[0].name, "pii");
        assert_eq!(tags[0].match_type, MatchType::Self_);
        assert_eq!(
            levels(&tags[0]),
            vec!["catalog", "column", "schema", "table"]
        );

        assert_eq!(tags[1].name, "ssn");
        assert_eq!(tags[1].match_type, MatchType::Ancestor);
        assert_eq!(levels(&tags[1]), vec!["catalog", "schema", "table"]);

        for tag in tags {
            assert!(tag.attributes.is_empty(), "no attribute is on the tag wire");
            assert!(tag.options.is_empty(), "no option is on the tag wire");
            assert_eq!(tag.validity_periods, 0, "no validity period is on the wire");
            assert!(
                tag.attached_through.is_enabled,
                "`from_assignments` writes `isEnabled: true` unconditionally"
            );
        }
    }

    /// A target naming no table carries no attachment the enricher would admit —
    /// every stored one is then `{catalog, schema, table[, column]}`, i.e. a
    /// DESCENDANT of the request, which a tag request at
    /// `ResourceMatchingScope.SELF` does not admit. So this is `Some(empty)`
    /// (the measured narrowing: the tag phase is skipped) and NOT `None` (a
    /// refusal), and the resolver is not asked at all.
    #[tokio::test]
    async fn a_target_above_the_table_narrows_to_the_empty_set() {
        for t in [target(None, None, None), target(Some("s"), None, None)] {
            let mut lookups = FakeTagLookups::clean(&[("pii", MatchType::Self_)]);
            let got = resolved_tags_for_target(&mut lookups, &t)
                .await
                .expect("no error")
                .expect("the narrowing is offered, not refused");
            assert!(got.as_slice().is_empty(), "for {t:?}");
            assert!(lookups.resolved_asked.is_empty(), "for {t:?}");
        }
    }

    /// **This input cannot arise from the live path, and the test says so rather
    /// than implying coverage that does not exist.** ⚠ **The reason CHANGED with
    /// SP4d debt Task 3, and the old reason is the point of this note.** It used to
    /// be: a `('', '')` assignment row IS storable (`PUT …/resource-tags
    /// {"schema": "", "table": ""}` — `assign_resource_tag` validated nothing and
    /// migration 0081 had no `CHECK`), `resolved_tags_for_resource_via(…, "", "",
    /// "")` DOES return it — the predicate is an exact `schema_name = $2 AND
    /// table_name = $3`, so an empty request matches a row written empty — and what
    /// kept it out of the mapping was neither the query nor the schema but the
    /// gate's `EmptyLevel` class.
    ///
    /// It is now the SCHEMA: migration 0177's
    /// `catalog_tag_assignments_levels_are_present_and_literal` means no such row
    /// can exist to be returned, and `egret_bi::catalog_tags` refuses that `PUT`
    /// with a 400. Nothing constrains `explain`'s own `schema`/`table` strings —
    /// the handler's 400 tests for an absent level, not an empty one — so an empty
    /// REQUEST still reaches `resolved_tags_for_resource_via`, which now matches
    /// nothing. This test is reached by calling the mapping directly.
    ///
    /// The mapping is still written to be right for it: the levels follow
    /// `from_assignments`' own rule (skip an empty level), so a `('', '')` row
    /// becomes the one-level `{catalog}` attachment, and
    /// `TagAttachmentProblem::ResourceLevels` is what answers in the evaluator
    /// (pinned at `egret-ranger-eval/tests/tag_refusals.rs`). A mapping that
    /// padded the levels out to four instead would turn a refusal into a
    /// confident verdict.
    #[test]
    fn an_empty_schema_and_table_map_to_the_one_level_attachment_the_evaluator_refuses() {
        let stored = egret_policy::ResolvedTag {
            name: "pii".to_string(),
            match_type: MatchType::Self_,
        };
        let got = attachment_for(&stored, "", "", "").expect("Self_ maps");
        assert_eq!(levels(&got), vec!["catalog"]);
        assert!(
            got.attached_through.levels.len() < 3,
            "fewer than three levels is what `tag_candidates` refuses on"
        );
    }

    /// The three match types no arm of `resolved_tags_for_resource_via`
    /// produces. They are a REFUSAL rather than a guess, and one of them
    /// declines the whole set — a producer this mapping has not been measured
    /// against must not have two of its tags trusted and one dropped.
    #[tokio::test]
    async fn an_unmodelled_match_type_declines_the_set_rather_than_being_dropped() {
        for mt in [
            MatchType::Descendant,
            MatchType::SelfAndAllDescendants,
            MatchType::None,
        ] {
            let stored = egret_policy::ResolvedTag {
                name: "pii".to_string(),
                match_type: mt,
            };
            assert!(attachment_for(&stored, "s", "t", "c").is_none(), "{mt:?}");

            let mut lookups = FakeTagLookups::clean(&[("ok", MatchType::Self_), ("bad", mt)]);
            let got =
                resolved_tags_for_target(&mut lookups, &target(Some("s"), Some("t"), Some("c")))
                    .await
                    .expect("no error");
            assert_eq!(
                got.as_ref().err(),
                Some(&ExplainTagDecline::StoredFoldDivergent {
                    schema: "s".to_string(),
                    table: "t".to_string(),
                    column: "c".to_string(),
                }),
                "one unmappable attachment must decline the whole set, not be skipped — and the \
                 reason it reports is the APPROXIMATION documented at the call site, because this \
                 arm is unreachable from `resolved_tags_for_resource_via`: {mt:?}"
            );
        }
    }
}

#[cfg(test)]
mod engine_plugin_version_tests {
    use super::engine_plugin_version;
    use egret_ranger_eval::eval::PluginVersion;

    #[test]
    fn every_version_the_operator_may_call_modelled_maps_to_a_variant() {
        for (t, v) in egret_perm::MODELLED_RANGER_PLUGINS {
            assert!(
                engine_plugin_version("e", "external", t, Some(v), &NO_SPEC).is_ok(),
                "{t}@{v} is listed as modelled but the evaluator has no variant for it"
            );
        }
    }

    const NO_SPEC: serde_json::Value = serde_json::Value::Null;

    /// The other direction of the test above: every version the EVALUATOR lists
    /// for an engine type is in `MODELLED_RANGER_PLUGINS`, so the two lists
    /// (`egret_perm`'s, which the API reports, and `egret-ranger-eval`'s, which
    /// decides) cannot drift apart in either direction.
    #[test]
    fn the_evaluators_modelled_versions_are_exactly_the_listed_pairs() {
        use egret_ranger_eval::eval::EngineType;
        let version_str = |v: PluginVersion| match v {
            PluginVersion::Ranger2_5_0 => "2.5.0",
            PluginVersion::Ranger2_8_0 => "2.8.0",
            PluginVersion::Ranger2_9_0 => "2.9.0",
        };
        let mut from_evaluator: Vec<(&str, &str)> = Vec::new();
        for engine in [EngineType::Trino, EngineType::StarRocks] {
            for v in engine.modelled_versions() {
                assert!(engine.models(*v));
                from_evaluator.push((engine.service_def_name(), version_str(*v)));
            }
        }
        let mut listed: Vec<(&str, &str)> = egret_perm::MODELLED_RANGER_PLUGINS.to_vec();
        from_evaluator.sort_unstable();
        listed.sort_unstable();
        assert_eq!(
            from_evaluator, listed,
            "`EngineType::models` and `MODELLED_RANGER_PLUGINS` must list the same pairs"
        );
    }

    #[test]
    fn an_external_engine_maps_its_declared_version() {
        assert_eq!(
            engine_plugin_version("e", "external", "trino", Some("2.5.0"), &NO_SPEC),
            Ok(PluginVersion::Ranger2_5_0)
        );
        assert_eq!(
            engine_plugin_version("e", "external", "starrocks", Some("2.8.0"), &NO_SPEC),
            Ok(PluginVersion::Ranger2_8_0)
        );
    }

    #[test]
    fn a_provisioned_engine_resolves_through_its_exact_image() {
        let trino470 = serde_json::json!({"imageVersion": "470"});
        assert_eq!(
            engine_plugin_version("e", "provisioned", "trino", None, &trino470),
            Ok(PluginVersion::Ranger2_5_0)
        );
        let sr414 =
            serde_json::json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1.4"}});
        assert_eq!(
            engine_plugin_version("e", "provisioned", "starrocks", None, &sr414),
            Ok(PluginVersion::Ranger2_8_0)
        );
    }

    #[test]
    fn a_provisioned_engine_on_a_floating_or_missing_image_is_refused_with_the_fix() {
        let floating =
            serde_json::json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1-latest"}});
        let err =
            engine_plugin_version("sr1", "provisioned", "starrocks", None, &floating).unwrap_err();
        assert!(
            err.contains("sr1") && err.contains("4.1-latest") && err.contains("pin"),
            "{err}"
        );
        let missing = serde_json::json!({});
        let err = engine_plugin_version("t1", "provisioned", "trino", None, &missing).unwrap_err();
        assert!(err.contains("t1") && err.contains("no image"), "{err}");
    }

    // Both engines' default images are modelled: Trino 483 runs Ranger 2.8.0
    // (proved in engine-upgrade Task 5) and StarRocks 4.1.6 runs 2.9.0 (Task 4).
    // No known image is unmodelled any more, so the "refused, not guessed" cover
    // below uses an UNKNOWN image (a floating or unrecognised tag).
    #[test]
    fn an_unbound_profile_is_evaluated_at_the_default_image() {
        use egret_ranger_eval::eval::EngineType;
        assert_eq!(
            super::default_image_plugin_version(EngineType::Trino),
            Ok(PluginVersion::Ranger2_8_0),
            "egret's default Trino image is 483, which runs Ranger 2.8.0"
        );
        assert_eq!(
            super::default_image_plugin_version(EngineType::StarRocks),
            Ok(PluginVersion::Ranger2_9_0),
            "egret's default StarRocks image is fe-ubuntu:4.1.6, which runs Ranger 2.9.0"
        );
    }

    #[test]
    fn a_provisioned_trino_483_resolves_to_ranger_2_8_0() {
        let trino483 = serde_json::json!({"imageVersion": "483"});
        assert_eq!(
            engine_plugin_version("t", "provisioned", "trino", None, &trino483),
            Ok(PluginVersion::Ranger2_8_0)
        );
    }

    #[test]
    fn an_image_the_table_does_not_know_is_refused_not_guessed() {
        let unknown = serde_json::json!({"imageVersion": "999"});
        let err = engine_plugin_version("t", "provisioned", "trino", None, &unknown).unwrap_err();
        assert!(
            err.contains("999") && err.contains("pin"),
            "names the image and the fix: {err}"
        );
    }

    #[test]
    fn a_provisioned_starrocks_4_1_6_resolves_to_ranger_2_9_0() {
        let sr416 =
            serde_json::json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1.6"}});
        assert_eq!(
            engine_plugin_version("s", "provisioned", "starrocks", None, &sr416),
            Ok(PluginVersion::Ranger2_9_0)
        );
    }

    /// Every (type, version) pairing of the three known versions: only the
    /// proved ones are modelled.
    #[test]
    fn only_the_proved_pairs_are_modelled() {
        for (t, v, modelled) in [
            ("trino", "2.5.0", true),
            ("trino", "2.8.0", true),
            ("trino", "2.9.0", false),
            ("starrocks", "2.5.0", false),
            ("starrocks", "2.8.0", true),
            ("starrocks", "2.9.0", true),
        ] {
            assert_eq!(
                engine_plugin_version("e", "external", t, Some(v), &NO_SPEC).is_ok(),
                modelled,
                "{t}@{v}"
            );
            assert_eq!(
                egret_perm::ranger_plugin_is_modelled(t, v),
                modelled,
                "{t}@{v}"
            );
        }
    }

    #[test]
    fn an_unmeasured_or_missing_version_is_a_sentence_naming_it() {
        let err = engine_plugin_version("edge", "external", "trino", Some("2.6.0"), &NO_SPEC)
            .unwrap_err();
        assert!(err.contains("edge") && err.contains("2.6.0"), "{err}");
        assert!(engine_plugin_version("edge", "external", "trino", None, &NO_SPEC).is_err());
        assert!(
            engine_plugin_version("edge", "provisioned", "risingwave", None, &NO_SPEC).is_err()
        );
    }
}
