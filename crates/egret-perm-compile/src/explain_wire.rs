//! Wire DTOs for `egret_ranger_eval::eval`'s three-valued decision, mask, and
//! row-filter results (SP4d-2a Task 4).
//!
//! `explain` is being rerouted to evaluate the policy set each engine
//! actually downloads, using `egret-ranger-eval`. That evaluator returns
//! rich three-valued results — `Decision::{Allowed, Denied, Undecidable}`
//! plus mask and row-filter outcomes on the same contract — and those have
//! to cross an HTTP boundary with an OpenAPI schema. This module is the wire
//! types and the conversions; the `explain` handler itself is a later task's
//! (Task 5's) responsibility, and so is registering these in the route
//! crate's `#[derive(utoipa::OpenApi)]` `components(schemas(...))` list.
//!
//! **Why hand-written here, and not `#[derive(ToSchema)]` on
//! `egret-ranger-eval`'s own types.** That crate depends on `serde` and
//! `serde_json` and nothing else, deliberately — see its own `Cargo.toml`
//! doc comment and `src/lib.rs` module doc: it is a pure evaluator, and that
//! purity is a stated discipline. Adding `utoipa` there to save mirroring
//! work here would trade that discipline for a shortcut, so the DTOs live in
//! this route crate instead (`egret-perm-compile`, which already depends on
//! `egret-ranger-eval` — see Cargo.toml).
//!
//! **The property this module exists for.** Every `From<&eval::T>` below
//! matches on the source enum with NO wildcard arm. That is deliberate: the
//! day someone adds a variant to `Decision`, `DenyReason`, or
//! `UndecidableCause` upstream, THIS CRATE FAILS TO COMPILE, rather than
//! silently serialising the new case as something else — or worse, as a
//! case that reads as benign to a steward. A `_ =>` arm here would convert a
//! future fail-open defect in the evaluator into a green build here.
//!
//! **The mirroring goes all the way down, not just to `UndecidableCause`.**
//! `UndecidableCause::EnvelopeConstruct` / `::PolicyConstruct` carry an
//! `egret_ranger_eval::model::Unmodelled` payload (15 variants), and
//! `::ServiceDef` a `servicedef::ServiceDefIssue` (4 variants, one of which —
//! `InvalidResourceGraph` — carries a `PolicyType` and a `GraphProblem` of
//! its own). An earlier version of this module rendered those nested
//! payloads with `{:?}` instead of mirroring them, on the theory that a
//! frontend's `kind` switch only needs to reach the `UndecidableCause`
//! level. That was wrong, for two reasons a reviewer raised: first, the
//! operator-facing information — e.g. `Unmodelled::TagPolicies` naming HOW
//! MANY tag policies could decide a request, the exact fact
//! `SP4d-2b`/Task 8's steward UI needs to render — lives in that nested
//! payload, so hiding it behind `{:?}` would force the UI to pattern-match a
//! Rust `Debug` string, a worse coupling than the one avoiding `utoipa` in
//! `egret-ranger-eval` was meant to prevent. Second, and decisively: `{:?}`
//! is not a stable wire contract. Renaming a variant or a field inside
//! `Unmodelled` changes the wire string with no compile error, no OpenAPI
//! diff, and no test failure unless a test happens to pin that exact
//! string — exactly the class of silent drift the no-wildcard-arm
//! discipline exists to turn into a build break. Stopping that discipline
//! one level down, where the variants are newest and still moving (Task 1
//! landed additions to these very enums on this branch), would have
//! abandoned it exactly where it matters most.
//!
//! So `Unmodelled`, `ItemBucket`, `TagPolicyBlock`, `ServiceDefIssue`,
//! `PolicyType`, and `GraphProblem` are ALL mirrored the same way as
//! `Decision`/`DenyReason`/`UndecidableCause`: an enum per source enum, a
//! `From`/`From<&_>` conversion with no wildcard arm, and every variant name
//! surviving onto the wire as a real, switchable `kind` — never folded into
//! a formatted string. `Unmodelled::TagPolicies(TagPolicyBlock::Policies(n))`
//! in particular carries its count as a JSON number
//! (`WireTagPolicyBlock::Policies { count }`), not embedded in text, because
//! Task 8 renders it and a later package (SP4d-2b) changes what it means.
//! No exception was found: every leaf enum here (`ItemBucket`,
//! `TagPolicyBlock`, `PolicyType`, `GraphProblem`) carries a discriminator a
//! steward or a later UI can plausibly need to switch on, not one opaque
//! string with nothing beneath it — so none of them stayed a `Debug` string.
//! The only genuinely-opaque payloads left as plain typed fields are things
//! that are NOT discriminators: raw expression text (`expr`, `value`),
//! resource/zone names, and item-list indices.
//!
//! **`usize` fields (`item_index`, list indices) are narrowed to `u32` on
//! the wire, not mirrored as `usize`.** `utoipa::ToSchema`'s support for
//! bare `usize` is not exercised anywhere else in this codebase (checked
//! before choosing this), so `u32` is the safe, unambiguous choice for an
//! item-list index — small enough that the narrowing itself can never
//! truncate a real index. Do not "simplify" this back to `usize` without
//! first confirming `ToSchema` actually derives cleanly for it.
//!
//! **Where `policy_id`/`policy_name` live.** Every payload that names a
//! policy carries both as separate named fields on the same enum variant —
//! never folded into one string — specifically so Task 5 can attach a
//! `PolicyAttribution` (`attribute_policy_name`, [`crate::attribution`])
//! beside them later by adding a field to that variant, without reshaping
//! this DTO.
//!
//! **The `Vec` in `why` is never collapsed.** `Decision::Undecidable`,
//! `MaskDecision::Undecidable`, and `RowFilterDecision::Undecidable` all
//! carry `why: Vec<UndecidableCause>` because one profile can be undecidable
//! for several independent reasons at once; the wire mirrors keep the full
//! `Vec<WireUndecidableCause>` rather than reporting only the first cause.

use egret_ranger_eval::eval::{
    Decision, DenyReason, EngineType, MaskDecision, MaskResult, PluginVersion, RowFilterDecision,
    RowFilterResult, TagAttachmentProblem, TagNormalisationProblem, TagResourceProblem,
    UndecidableCause,
};
use egret_ranger_eval::model::{ItemBucket, TagPolicyBlock, Unmodelled};
use egret_ranger_eval::servicedef::{GraphProblem, PolicyType, ServiceDefIssue};
use serde::Serialize;
use utoipa::ToSchema;

// ── small enums used as payload fields ──────────────────────────────────

/// Wire mirror of [`EngineType`].
///
/// **`lowercase`, not `snake_case`, and that is the whole point (SP4d-2a
/// Task 5 review, I3).** `rename_all = "snake_case"` renders `StarRocks` as
/// `star_rocks`, and this API surface already publishes the other spelling:
/// `egret_query_api::Engine` is `rename_all = "lowercase"` and serialises
/// `starrocks`, which is also the literal stored in
/// `policy.authz_services.type` and matched by
/// `egret_perm_compile::trigger::engine_type_for`. A client correlating an
/// `ExplainProfile`'s `engine` with a profile's service type would otherwise
/// have to translate between two spellings of one word inside one document.
/// `lowercase` gives `trino` / `starrocks` and makes them the same string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireEngineType {
    Trino,
    StarRocks,
}

impl From<EngineType> for WireEngineType {
    fn from(engine: EngineType) -> Self {
        match engine {
            EngineType::Trino => WireEngineType::Trino,
            EngineType::StarRocks => WireEngineType::StarRocks,
        }
    }
}

/// Wire mirror of [`PluginVersion`]. Renamed explicitly rather than through
/// `rename_all`: the source identifiers (`Ranger2_5_0`) already embed
/// digits and underscores, and `rename_all`'s case-boundary heuristic has no
/// good answer for that shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub enum WirePluginVersion {
    #[serde(rename = "ranger_2_5_0")]
    Ranger2_5_0,
    #[serde(rename = "ranger_2_8_0")]
    Ranger2_8_0,
    #[serde(rename = "ranger_2_9_0")]
    Ranger2_9_0,
}

impl From<PluginVersion> for WirePluginVersion {
    fn from(version: PluginVersion) -> Self {
        match version {
            PluginVersion::Ranger2_5_0 => WirePluginVersion::Ranger2_5_0,
            PluginVersion::Ranger2_8_0 => WirePluginVersion::Ranger2_8_0,
            PluginVersion::Ranger2_9_0 => WirePluginVersion::Ranger2_9_0,
        }
    }
}

// ── DenyReason (3 variants, no payload) ──────────────────────────────────

/// Wire mirror of [`DenyReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireDenyReason {
    DenyPolicyItem,
    DenyAllElse,
    NoPolicyMatched,
}

impl From<DenyReason> for WireDenyReason {
    fn from(reason: DenyReason) -> Self {
        match reason {
            DenyReason::DenyPolicyItem => WireDenyReason::DenyPolicyItem,
            DenyReason::DenyAllElse => WireDenyReason::DenyAllElse,
            DenyReason::NoPolicyMatched => WireDenyReason::NoPolicyMatched,
        }
    }
}

// ── ItemBucket / TagPolicyBlock (payloads of `Unmodelled`) ───────────────

/// Wire mirror of [`ItemBucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireItemBucket {
    PolicyItems,
    DenyPolicyItems,
    AllowExceptions,
    DenyExceptions,
    DataMaskPolicyItems,
    RowFilterPolicyItems,
}

impl From<ItemBucket> for WireItemBucket {
    fn from(bucket: ItemBucket) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match bucket {
            ItemBucket::PolicyItems => WireItemBucket::PolicyItems,
            ItemBucket::DenyPolicyItems => WireItemBucket::DenyPolicyItems,
            ItemBucket::AllowExceptions => WireItemBucket::AllowExceptions,
            ItemBucket::DenyExceptions => WireItemBucket::DenyExceptions,
            ItemBucket::DataMaskPolicyItems => WireItemBucket::DataMaskPolicyItems,
            ItemBucket::RowFilterPolicyItems => WireItemBucket::RowFilterPolicyItems,
        }
    }
}

/// Wire mirror of [`TagPolicyBlock`]. `Policies` carries its count as a
/// JSON number (`count`), never folded into a formatted string — Task 8's
/// steward UI renders it, and SP4d-2b changes what it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireTagPolicyBlock {
    Policies { count: u32 },
    Unreadable,
}

impl From<&TagPolicyBlock> for WireTagPolicyBlock {
    fn from(block: &TagPolicyBlock) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match block {
            TagPolicyBlock::Policies(count) => WireTagPolicyBlock::Policies {
                count: *count as u32,
            },
            TagPolicyBlock::Unreadable => WireTagPolicyBlock::Unreadable,
        }
    }
}

/// Wire mirror of [`TagResourceProblem`]: why a tag policy's resource map is
/// one the evaluator cannot filter the way Ranger's `RangerResourceTrie` does
/// (SP4d-2b Task 2).
///
/// Carried as its own `kind`, not a formatted string, for the same reason
/// [`WireTagPolicyBlock`] carries a `count`: a steward has to be able to tell
/// "this policy names a second resource level" from "this policy's tag value is
/// a wildcard", and the remedies differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireTagResourceProblem {
    NotOnlyTheTagResource,
    Excludes,
    ValueNotTriePrefixExact { value: String },
    Recursive,
}

impl From<&TagResourceProblem> for WireTagResourceProblem {
    fn from(problem: &TagResourceProblem) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match problem {
            TagResourceProblem::NotOnlyTheTagResource => {
                WireTagResourceProblem::NotOnlyTheTagResource
            }
            TagResourceProblem::Excludes => WireTagResourceProblem::Excludes,
            TagResourceProblem::ValueNotTriePrefixExact(value) => {
                WireTagResourceProblem::ValueNotTriePrefixExact {
                    value: value.clone(),
                }
            }
            TagResourceProblem::Recursive => WireTagResourceProblem::Recursive,
        }
    }
}

/// Wire mirror of [`TagNormalisationProblem`]: what a tag policy carries that
/// the TAG policy repository's `normalizeAndPrunePolicies` would rewrite or
/// reject (SP4d-2b Task 5).
///
/// Three `kind`s, and the third is not a variation on the first two: the two
/// namespaced arms are a WRONG ANSWER this crate declines to give, and
/// `mask_type_absent` is a CRASH — the plugin cannot build a policy engine from
/// that download, so nothing it serves is answered. A steward reading
/// "unsupported tag policy" would have no way to tell "rename the access type"
/// from "this download breaks the plugin outright", and the second is an
/// incident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireTagNormalisationProblem {
    NamespacedAccessType { access_type: String },
    NamespacedMaskType { mask_type: String },
    MaskTypeAbsent,
}

impl From<&TagNormalisationProblem> for WireTagNormalisationProblem {
    fn from(problem: &TagNormalisationProblem) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match problem {
            TagNormalisationProblem::NamespacedAccessType(access_type) => {
                WireTagNormalisationProblem::NamespacedAccessType {
                    access_type: access_type.clone(),
                }
            }
            TagNormalisationProblem::NamespacedMaskType(mask_type) => {
                WireTagNormalisationProblem::NamespacedMaskType {
                    mask_type: mask_type.clone(),
                }
            }
            TagNormalisationProblem::MaskTypeAbsent => WireTagNormalisationProblem::MaskTypeAbsent,
        }
    }
}

/// Wire mirror of [`TagAttachmentProblem`]: what one resolved tag ATTACHMENT
/// carries that the evaluator cannot reproduce (SP4d-2b Task 4).
///
/// Three `kind`s rather than one sentence, for the same reason
/// [`WireTagResourceProblem`] has four: the remedies differ and do not overlap.
/// `options` and `validity_periods` say the tag download carries a validity
/// construct egret neither emits nor evaluates; `resource_levels` says the
/// download's service resource is not a shape egret's own writer can produce at
/// all, which is a question about where the payload came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireTagAttachmentProblem {
    Options { keys: Vec<String> },
    ValidityPeriods { count: u32 },
    ResourceLevels { levels: Vec<String> },
}

impl From<&TagAttachmentProblem> for WireTagAttachmentProblem {
    fn from(problem: &TagAttachmentProblem) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match problem {
            TagAttachmentProblem::Options(keys) => {
                WireTagAttachmentProblem::Options { keys: keys.clone() }
            }
            TagAttachmentProblem::ValidityPeriods(count) => {
                WireTagAttachmentProblem::ValidityPeriods {
                    count: *count as u32,
                }
            }
            TagAttachmentProblem::ResourceLevels(levels) => {
                WireTagAttachmentProblem::ResourceLevels {
                    levels: levels.clone(),
                }
            }
        }
    }
}

// ── Unmodelled (15 variants as of this task) ─────────────────────────────

/// Wire mirror of [`Unmodelled`] — the construct a policy or envelope body
/// carries that this reader detected but does not model. See the module
/// doc for why this is mirrored exhaustively rather than rendered as a
/// `Debug` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireUnmodelled {
    ValiditySchedules,
    PolicyConditions,
    ItemConditions {
        bucket: WireItemBucket,
        index: u32,
    },
    ZoneName {
        zone_name: String,
    },
    SecurityZones,
    TagPolicies {
        block: WireTagPolicyBlock,
    },
    PolicyDeltas,
    ServiceConfig,
    MaskExpressionMarker {
        bucket: WireItemBucket,
        item_index: u32,
        expr: String,
    },
    MaskConditionExpr {
        bucket: WireItemBucket,
        item_index: u32,
    },
    RowFilterExpressionMarker {
        bucket: WireItemBucket,
        item_index: u32,
        expr: String,
    },
    ResourceValueExpressionMarker {
        additional_index: Option<u32>,
        resource_type: String,
        value: String,
    },
    UserExpressionMarker {
        bucket: WireItemBucket,
        index: u32,
        user: String,
    },
    ExplicitDeny {
        bucket: WireItemBucket,
        index: u32,
        access_type: String,
    },
    DelegateAdmin {
        bucket: WireItemBucket,
        index: u32,
    },
}

impl From<&Unmodelled> for WireUnmodelled {
    fn from(value: &Unmodelled) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match value {
            Unmodelled::ValiditySchedules => WireUnmodelled::ValiditySchedules,
            Unmodelled::PolicyConditions => WireUnmodelled::PolicyConditions,
            Unmodelled::ItemConditions { bucket, index } => WireUnmodelled::ItemConditions {
                bucket: WireItemBucket::from(*bucket),
                index: *index as u32,
            },
            Unmodelled::ZoneName(zone_name) => WireUnmodelled::ZoneName {
                zone_name: zone_name.clone(),
            },
            Unmodelled::SecurityZones => WireUnmodelled::SecurityZones,
            Unmodelled::TagPolicies(block) => WireUnmodelled::TagPolicies {
                block: WireTagPolicyBlock::from(block),
            },
            Unmodelled::PolicyDeltas => WireUnmodelled::PolicyDeltas,
            Unmodelled::ServiceConfig => WireUnmodelled::ServiceConfig,
            Unmodelled::MaskExpressionMarker {
                bucket,
                item_index,
                expr,
            } => WireUnmodelled::MaskExpressionMarker {
                bucket: WireItemBucket::from(*bucket),
                item_index: *item_index as u32,
                expr: expr.clone(),
            },
            Unmodelled::MaskConditionExpr { bucket, item_index } => {
                WireUnmodelled::MaskConditionExpr {
                    bucket: WireItemBucket::from(*bucket),
                    item_index: *item_index as u32,
                }
            }
            Unmodelled::RowFilterExpressionMarker {
                bucket,
                item_index,
                expr,
            } => WireUnmodelled::RowFilterExpressionMarker {
                bucket: WireItemBucket::from(*bucket),
                item_index: *item_index as u32,
                expr: expr.clone(),
            },
            Unmodelled::ResourceValueExpressionMarker {
                additional_index,
                resource_type,
                value,
            } => WireUnmodelled::ResourceValueExpressionMarker {
                additional_index: additional_index.map(|i| i as u32),
                resource_type: resource_type.clone(),
                value: value.clone(),
            },
            Unmodelled::UserExpressionMarker {
                bucket,
                index,
                user,
            } => WireUnmodelled::UserExpressionMarker {
                bucket: WireItemBucket::from(*bucket),
                index: *index as u32,
                user: user.clone(),
            },
            Unmodelled::ExplicitDeny {
                bucket,
                index,
                access_type,
            } => WireUnmodelled::ExplicitDeny {
                bucket: WireItemBucket::from(*bucket),
                index: *index as u32,
                access_type: access_type.clone(),
            },
            Unmodelled::DelegateAdmin { bucket, index } => WireUnmodelled::DelegateAdmin {
                bucket: WireItemBucket::from(*bucket),
                index: *index as u32,
            },
        }
    }
}

// ── PolicyType / GraphProblem (payloads of `ServiceDefIssue`) ────────────

/// Wire mirror of [`PolicyType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WirePolicyType {
    Access,
    DataMask,
    RowFilter,
}

impl From<PolicyType> for WirePolicyType {
    fn from(policy_type: PolicyType) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match policy_type {
            PolicyType::Access => WirePolicyType::Access,
            PolicyType::DataMask => WirePolicyType::DataMask,
            PolicyType::RowFilter => WirePolicyType::RowFilter,
        }
    }
}

/// Wire mirror of [`GraphProblem`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireGraphProblem {
    NoSourcesOrSinks,
    SinkIsNotAValidLeaf { resource: String },
    BrokenParentChain { resource: String },
}

impl From<&GraphProblem> for WireGraphProblem {
    fn from(problem: &GraphProblem) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match problem {
            GraphProblem::NoSourcesOrSinks => WireGraphProblem::NoSourcesOrSinks,
            GraphProblem::SinkIsNotAValidLeaf { resource } => {
                WireGraphProblem::SinkIsNotAValidLeaf {
                    resource: resource.clone(),
                }
            }
            GraphProblem::BrokenParentChain { resource } => WireGraphProblem::BrokenParentChain {
                resource: resource.clone(),
            },
        }
    }
}

// ── ServiceDefIssue (4 variants as of this task) ─────────────────────────

/// Wire mirror of [`ServiceDefIssue`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireServiceDefIssue {
    UnmodelledMatcherClass {
        resource: String,
        matcher: String,
    },
    QuotedCaseSensitive {
        resource: String,
    },
    ChainedImpliedGrants {
        implying: String,
        implied: String,
    },
    InvalidResourceGraph {
        policy_type: WirePolicyType,
        reason: WireGraphProblem,
    },
}

impl From<&ServiceDefIssue> for WireServiceDefIssue {
    fn from(issue: &ServiceDefIssue) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match issue {
            ServiceDefIssue::UnmodelledMatcherClass { resource, matcher } => {
                WireServiceDefIssue::UnmodelledMatcherClass {
                    resource: resource.clone(),
                    matcher: matcher.clone(),
                }
            }
            ServiceDefIssue::QuotedCaseSensitive { resource } => {
                WireServiceDefIssue::QuotedCaseSensitive {
                    resource: resource.clone(),
                }
            }
            ServiceDefIssue::ChainedImpliedGrants { implying, implied } => {
                WireServiceDefIssue::ChainedImpliedGrants {
                    implying: implying.clone(),
                    implied: implied.clone(),
                }
            }
            ServiceDefIssue::InvalidResourceGraph {
                policy_type,
                reason,
            } => WireServiceDefIssue::InvalidResourceGraph {
                policy_type: WirePolicyType::from(*policy_type),
                reason: WireGraphProblem::from(reason),
            },
        }
    }
}

// ── UndecidableCause (19 variants; see the module doc) ──────────────────
//
// ⚠ This count read `13` until SP4d-2b Task 4 and had been wrong since Task 2
// added the first tag cause — five variants stale, with nothing to catch it,
// because the exhaustiveness guard below is on the MATCH and not on any number
// written beside it. Recount when you add one; a number nobody checks is worse
// than no number.

/// Wire mirror of [`UndecidableCause`]. `kind` is the discriminator a
/// frontend switches on; see the module doc for how the constructs each
/// cause names (`Unmodelled`, `ServiceDefIssue`) are mirrored just as
/// exhaustively, one level deeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireUndecidableCause {
    EnvelopeConstruct {
        construct: WireUnmodelled,
    },
    PolicyConstruct {
        policy_id: Option<i64>,
        policy_name: String,
        construct: WireUnmodelled,
    },
    ServiceDef {
        issue: WireServiceDefIssue,
    },
    ResourceOwnerUnknown {
        policy_id: Option<i64>,
        policy_name: String,
    },
    SuperUsersUnknown,
    PseudoAccessType {
        access_type: String,
    },
    AccessTypeCaseMismatch {
        policy_id: Option<i64>,
        policy_name: String,
        access_type: String,
    },
    MaskExpression {
        policy_id: Option<i64>,
        policy_name: String,
        item_index: u32,
        expr: String,
    },
    RowFilterExpression {
        policy_id: Option<i64>,
        policy_name: String,
        item_index: u32,
        expr: String,
    },
    RowFilterDoubleBrackets {
        policy_id: Option<i64>,
        policy_name: String,
        item_index: u32,
        expr: String,
    },
    /// `policy_ids`/`policy_names` are the FULL tied set — see
    /// [`UndecidableCause::EvalOrderAmbiguous`]'s own doc for why collapsing
    /// this to one id would misreport a tie this crate cannot order.
    EvalOrderAmbiguous {
        policy_ids: Vec<Option<i64>>,
        policy_names: Vec<String>,
    },
    /// The `tagPolicies` block carries policies this decision's type would
    /// evaluate and the caller resolved no tag set. Not reachable through
    /// `explain` yet: the tag phase is not composed with the resource phase
    /// until SP4d-2b Task 3.
    TagsUnresolved,
    TagResourceNotReproducible {
        policy_id: Option<i64>,
        policy_name: String,
        problem: WireTagResourceProblem,
    },
    /// A tag mask or row-filter policy that produced a result is also
    /// deny-capable, so Ranger can report its id with a later policy's mask.
    TagSubPolicyIsDenyCapable {
        policy_id: Option<i64>,
        policy_name: String,
    },
    /// A tag policy the TAG policy repository would rewrite or remove on the way
    /// in, in a way the evaluator does not reproduce — or, for
    /// `mask_type_absent`, one that makes the plugin throw while building its
    /// repository so that it serves nothing at all.
    TagPolicyNotNormalisable {
        policy_id: Option<i64>,
        policy_name: String,
        problem: WireTagNormalisationProblem,
    },
    /// Two or more tag policies at DIFFERENT `policyPriority` each decide, and
    /// the engine settles which survives with an arbitration the evaluator does
    /// not model.
    ///
    /// The three lists are POSITIONAL — `policy_ids[i]`, `policy_names[i]` and
    /// `priorities[i]` describe one policy — for the same reason
    /// [`WireUndecidableCause::EvalOrderAmbiguous`] carries two: a steward needs
    /// to see which policies are in contention and at what priority, and
    /// flattening that into a sentence would lose the priorities, which are the
    /// thing to change.
    TagPriorityArbitrationNotModelled {
        policy_ids: Vec<Option<i64>>,
        policy_names: Vec<String>,
        priorities: Vec<i32>,
    },
    /// A tag ALLOW in force and a **resource** policy at a strictly higher
    /// priority that also allows — Ranger settles the composite through
    /// `accessTypeResults`, which the evaluator does not reproduce.
    ///
    /// # ⚠ Its own variant, because the alias put a wrong NOUN in front of a steward
    ///
    /// The evaluator's `AccessPriorityCompositeNotModelled` first rode
    /// [`WireUndecidableCause::TagPriorityArbitrationNotModelled`] as a documented
    /// alias — same arbitration, same three positional lists, no new schema — so
    /// that the Critical could land without racing the three files this split
    /// needs. The cost was one word: that sentence opens "Several **tag** policies
    /// …", and here one of the two is a RESOURCE policy. Names, ids, priorities and
    /// the remedy were all correct; the noun was not, which is this branch's
    /// recurring defect class (a label true of some of the states it is shown for).
    ///
    /// So the two fields are NAMED rather than positional: the steward-facing fact
    /// is *which* policy is the tag one, and a `Vec` cannot say that. Measured
    /// rarity, for the copy's sake: **0 of 1148** composed-random cases and 10 of
    /// 4000 overall, so the sentence must not imply it is common.
    AccessPriorityCompositeNotModelled {
        /// The tag policy whose ALLOW was in force. It can only be the tag
        /// phase's: `allowed` becomes true at one site, which also sets
        /// `priority`, and evaluation is priority-DESCENDING, so any later
        /// candidate has `found.priority <= priority` — proved, not sampled.
        tag_policy_id: Option<i64>,
        tag_policy_name: String,
        tag_priority: i32,
        /// The resource policy at a strictly higher priority that also allowed.
        resource_policy_id: Option<i64>,
        resource_policy_name: String,
        resource_priority: i32,
    },
    /// A tag DENY and a strictly higher-priority resource policy that allows —
    /// a shape Ranger 2.5.0 and 2.8.0 answer DIFFERENTLY, so it can only be
    /// decided once the running plugin version is known.
    TagDenyReopenedDiffersByPluginVersion {
        /// The plugin version the caller declared, or `null` if it declared none.
        ///
        /// The two states need DIFFERENT advice and the UI branches on it: a known
        /// 2.5.0 means the running plugin is identified and the refusal is that
        /// 2.5.0's arm is unmodelled — there is nothing to record. `null` means the
        /// caller did not say, and recording it would make half of these
        /// answerable. `2_8_0` never appears: that is the version the evaluator
        /// reproduces, so the refusal is not raised for it.
        plugin_version: Option<WirePluginVersion>,
        tag_policy_id: Option<i64>,
        tag_policy_name: String,
        resource_policy_id: Option<i64>,
        resource_policy_name: String,
    },
    /// A tag mask or row filter was displaced in part by a strictly
    /// higher-priority resource policy: Ranger reports the tag policy's id with
    /// the resource policy's mask.
    TagSubResultSplitByResourcePolicy {
        tag_policy_id: Option<i64>,
        tag_policy_name: String,
        resource_policy_id: Option<i64>,
        resource_policy_name: String,
    },
    /// Whether Ranger's candidate filter (its resource trie) returns this policy
    /// decides the request, and the evaluator cannot establish which way.
    ResourceCandidacyNotReproducible {
        policy_id: Option<i64>,
        policy_name: String,
    },
    /// One tag ATTACHMENT in this request's resolved set carries something the
    /// evaluator cannot reproduce. Named by TAG rather than by policy: the
    /// construct is on the tag download, and it changes every tag policy's
    /// candidacy at once.
    TagAttachmentNotReproducible {
        tag: String,
        problem: WireTagAttachmentProblem,
    },
    EngineMismatch {
        declared: WireEngineType,
        service_def_name: String,
    },
    PluginVersionMismatch {
        engine: WireEngineType,
        declared: WirePluginVersion,
    },
}

impl From<&UndecidableCause> for WireUndecidableCause {
    fn from(cause: &UndecidableCause) -> Self {
        // NO wildcard arm: a variant added to `UndecidableCause` upstream
        // must fail this match at compile time, not fall through silently.
        match cause {
            UndecidableCause::EnvelopeConstruct(construct) => {
                WireUndecidableCause::EnvelopeConstruct {
                    construct: WireUnmodelled::from(construct),
                }
            }
            UndecidableCause::PolicyConstruct {
                policy_id,
                policy_name,
                construct,
            } => WireUndecidableCause::PolicyConstruct {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                construct: WireUnmodelled::from(construct),
            },
            UndecidableCause::ServiceDef(issue) => WireUndecidableCause::ServiceDef {
                issue: WireServiceDefIssue::from(issue),
            },
            UndecidableCause::ResourceOwnerUnknown {
                policy_id,
                policy_name,
            } => WireUndecidableCause::ResourceOwnerUnknown {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
            },
            UndecidableCause::SuperUsersUnknown => WireUndecidableCause::SuperUsersUnknown,
            UndecidableCause::PseudoAccessType(access_type) => {
                WireUndecidableCause::PseudoAccessType {
                    access_type: access_type.clone(),
                }
            }
            UndecidableCause::AccessTypeCaseMismatch {
                policy_id,
                policy_name,
                access_type,
            } => WireUndecidableCause::AccessTypeCaseMismatch {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                access_type: access_type.clone(),
            },
            UndecidableCause::MaskExpression {
                policy_id,
                policy_name,
                item_index,
                expr,
            } => WireUndecidableCause::MaskExpression {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                item_index: *item_index as u32,
                expr: expr.clone(),
            },
            UndecidableCause::RowFilterExpression {
                policy_id,
                policy_name,
                item_index,
                expr,
            } => WireUndecidableCause::RowFilterExpression {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                item_index: *item_index as u32,
                expr: expr.clone(),
            },
            UndecidableCause::RowFilterDoubleBrackets {
                policy_id,
                policy_name,
                item_index,
                expr,
            } => WireUndecidableCause::RowFilterDoubleBrackets {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                item_index: *item_index as u32,
                expr: expr.clone(),
            },
            UndecidableCause::EvalOrderAmbiguous {
                policy_ids,
                policy_names,
            } => WireUndecidableCause::EvalOrderAmbiguous {
                policy_ids: policy_ids.clone(),
                policy_names: policy_names.clone(),
            },
            UndecidableCause::TagsUnresolved => WireUndecidableCause::TagsUnresolved,
            UndecidableCause::TagSubPolicyIsDenyCapable {
                policy_id,
                policy_name,
            } => WireUndecidableCause::TagSubPolicyIsDenyCapable {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
            },
            UndecidableCause::TagSubResultSplitByResourcePolicy {
                tag_policy_id,
                tag_policy_name,
                resource_policy_id,
                resource_policy_name,
            } => WireUndecidableCause::TagSubResultSplitByResourcePolicy {
                tag_policy_id: *tag_policy_id,
                tag_policy_name: tag_policy_name.clone(),
                resource_policy_id: *resource_policy_id,
                resource_policy_name: resource_policy_name.clone(),
            },
            UndecidableCause::ResourceCandidacyNotReproducible {
                policy_id,
                policy_name,
            } => WireUndecidableCause::ResourceCandidacyNotReproducible {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
            },
            UndecidableCause::TagResourceNotReproducible {
                policy_id,
                policy_name,
                problem,
            } => WireUndecidableCause::TagResourceNotReproducible {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                problem: WireTagResourceProblem::from(problem),
            },
            UndecidableCause::TagPolicyNotNormalisable {
                policy_id,
                policy_name,
                problem,
            } => WireUndecidableCause::TagPolicyNotNormalisable {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                problem: WireTagNormalisationProblem::from(problem),
            },
            UndecidableCause::TagPriorityArbitrationNotModelled {
                policy_ids,
                policy_names,
                priorities,
            } => WireUndecidableCause::TagPriorityArbitrationNotModelled {
                policy_ids: policy_ids.clone(),
                policy_names: policy_names.clone(),
                priorities: priorities.clone(),
            },
            UndecidableCause::TagDenyReopenedDiffersByPluginVersion {
                plugin_version,
                tag_policy_id,
                tag_policy_name,
                resource_policy_id,
                resource_policy_name,
            } => WireUndecidableCause::TagDenyReopenedDiffersByPluginVersion {
                plugin_version: plugin_version.map(WirePluginVersion::from),
                tag_policy_id: *tag_policy_id,
                tag_policy_name: tag_policy_name.clone(),
                resource_policy_id: *resource_policy_id,
                resource_policy_name: resource_policy_name.clone(),
            },
            // ⚠ **No longer an alias — SPLIT, and the reason is one word.** This
            // rode `TagPriorityArbitrationNotModelled` when the Critical landed, to
            // avoid racing `openapi.json`, the generated client and the exhaustive
            // TSX switch while they were uncommitted elsewhere. That was the right
            // call and it cost one noun: the other sentence says "Several TAG
            // policies", and here one of the two is a RESOURCE policy. Its own
            // variant now, with the two policies NAMED rather than positional,
            // because which is which is the steward-facing fact.
            UndecidableCause::AccessPriorityCompositeNotModelled {
                tag_policy_id,
                tag_policy_name,
                tag_priority,
                resource_policy_id,
                resource_policy_name,
                resource_priority,
            } => WireUndecidableCause::AccessPriorityCompositeNotModelled {
                tag_policy_id: *tag_policy_id,
                tag_policy_name: tag_policy_name.clone(),
                tag_priority: *tag_priority,
                resource_policy_id: *resource_policy_id,
                resource_policy_name: resource_policy_name.clone(),
                resource_priority: *resource_priority,
            },
            UndecidableCause::TagAttachmentNotReproducible { tag, problem } => {
                WireUndecidableCause::TagAttachmentNotReproducible {
                    tag: tag.clone(),
                    problem: WireTagAttachmentProblem::from(problem),
                }
            }
            UndecidableCause::EngineMismatch {
                declared,
                service_def_name,
            } => WireUndecidableCause::EngineMismatch {
                declared: WireEngineType::from(*declared),
                service_def_name: service_def_name.clone(),
            },
            UndecidableCause::PluginVersionMismatch { engine, declared } => {
                WireUndecidableCause::PluginVersionMismatch {
                    engine: WireEngineType::from(*engine),
                    declared: WirePluginVersion::from(*declared),
                }
            }
        }
    }
}

// ── Decision (3 variants) ─────────────────────────────────────────────────

/// Wire mirror of [`Decision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireDecision {
    Allowed {
        policy_id: Option<i64>,
        policy_name: String,
    },
    Denied {
        policy_id: Option<i64>,
        policy_name: String,
        reason: WireDenyReason,
    },
    /// See the module doc: `why` stays a `Vec`, never collapsed to one
    /// cause.
    Undecidable { why: Vec<WireUndecidableCause> },
}

impl From<&Decision> for WireDecision {
    fn from(decision: &Decision) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match decision {
            Decision::Allowed {
                policy_id,
                policy_name,
            } => WireDecision::Allowed {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
            },
            Decision::Denied {
                policy_id,
                policy_name,
                reason,
            } => WireDecision::Denied {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                reason: WireDenyReason::from(*reason),
            },
            Decision::Undecidable { why } => WireDecision::Undecidable {
                why: why.iter().map(WireUndecidableCause::from).collect(),
            },
        }
    }
}

// ── mask result / decision ───────────────────────────────────────────────

/// Wire mirror of [`MaskResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WireMaskResult {
    pub policy_id: Option<i64>,
    pub policy_name: String,
    pub mask_type: Option<String>,
    pub masked_value: Option<String>,
    pub mask_condition: Option<String>,
}

impl From<&MaskResult> for WireMaskResult {
    fn from(result: &MaskResult) -> Self {
        WireMaskResult {
            policy_id: result.policy_id,
            policy_name: result.policy_name.clone(),
            mask_type: result.mask_type.clone(),
            masked_value: result.masked_value.clone(),
            mask_condition: result.mask_condition.clone(),
        }
    }
}

/// Wire mirror of [`MaskDecision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireMaskDecision {
    NoMask,
    Masked { result: WireMaskResult },
    Undecidable { why: Vec<WireUndecidableCause> },
}

impl From<&MaskDecision> for WireMaskDecision {
    fn from(decision: &MaskDecision) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match decision {
            MaskDecision::NoMask => WireMaskDecision::NoMask,
            MaskDecision::Masked(result) => WireMaskDecision::Masked {
                result: WireMaskResult::from(result),
            },
            MaskDecision::Undecidable { why } => WireMaskDecision::Undecidable {
                why: why.iter().map(WireUndecidableCause::from).collect(),
            },
        }
    }
}

// ── row-filter result / decision ─────────────────────────────────────────

/// Wire mirror of [`RowFilterResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WireRowFilterResult {
    pub policy_id: Option<i64>,
    pub policy_name: String,
    pub filter_expr: String,
}

impl From<&RowFilterResult> for WireRowFilterResult {
    fn from(result: &RowFilterResult) -> Self {
        WireRowFilterResult {
            policy_id: result.policy_id,
            policy_name: result.policy_name.clone(),
            filter_expr: result.filter_expr.clone(),
        }
    }
}

/// Wire mirror of [`RowFilterDecision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireRowFilterDecision {
    NoFilter,
    Filtered { result: WireRowFilterResult },
    Undecidable { why: Vec<WireUndecidableCause> },
}

impl From<&RowFilterDecision> for WireRowFilterDecision {
    fn from(decision: &RowFilterDecision) -> Self {
        // NO wildcard arm — see `WireUndecidableCause::from`'s doc.
        match decision {
            RowFilterDecision::NoFilter => WireRowFilterDecision::NoFilter,
            RowFilterDecision::Filtered(result) => WireRowFilterDecision::Filtered {
                result: WireRowFilterResult::from(result),
            },
            RowFilterDecision::Undecidable { why } => WireRowFilterDecision::Undecidable {
                why: why.iter().map(WireUndecidableCause::from).collect(),
            },
        }
    }
}
