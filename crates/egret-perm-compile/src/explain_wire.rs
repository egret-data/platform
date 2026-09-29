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
//! **What is, and is not, mirrored to that same depth.**
//! `UndecidableCause::EnvelopeConstruct` / `::PolicyConstruct` carry an
//! `egret_ranger_eval::model::Unmodelled` payload, and `::ServiceDef` a
//! `servicedef::ServiceDefIssue` — both themselves multi-variant enums, one
//! level further down in what the policy-body reader and the service-def
//! reader detect. Neither is named in this task's produces-list
//! (`Decision`, `DenyReason`, `UndecidableCause`, and the mask/row-filter
//! results), and a frontend's `kind` switch only needs to reach the
//! `UndecidableCause` level — the established convention
//! (`routes::ExplainReason::kind`) is a `kind` string a caller switches on,
//! not a nested switchable payload three enums deep. So `construct` and
//! `issue` below are that inner value's own `Debug` output, not a second
//! hand-mirrored enum. This is a scope decision, not an oversight: a `Debug`
//! string can never mislabel a new inner variant as an existing, possibly
//! benign one — it just is not itself a second `kind` a frontend can switch
//! on. If a later task needs that (a steward UI switching on `Unmodelled`'s
//! own variant), mirroring `Unmodelled`/`ServiceDefIssue` the same
//! exhaustive way is a follow-up, not a silent gap in this one.
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
    RowFilterResult, UndecidableCause,
};
use serde::Serialize;
use utoipa::ToSchema;

// ── small enums used as payload fields ──────────────────────────────────

/// Wire mirror of [`EngineType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
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
}

impl From<PluginVersion> for WirePluginVersion {
    fn from(version: PluginVersion) -> Self {
        match version {
            PluginVersion::Ranger2_5_0 => WirePluginVersion::Ranger2_5_0,
            PluginVersion::Ranger2_8_0 => WirePluginVersion::Ranger2_8_0,
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

// ── UndecidableCause (13 variants as of this task; see the module doc) ──

/// Wire mirror of [`UndecidableCause`]. `kind` is the discriminator a
/// frontend switches on; see the module doc for what is, and is not,
/// mirrored one level deeper for the constructs each cause names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireUndecidableCause {
    EnvelopeConstruct {
        construct: String,
    },
    PolicyConstruct {
        policy_id: Option<i64>,
        policy_name: String,
        construct: String,
    },
    ServiceDef {
        issue: String,
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
    EngineMismatch {
        declared: WireEngineType,
        service_def_name: String,
    },
    PluginVersionMismatch {
        engine: WireEngineType,
        declared: WirePluginVersion,
        measured: WirePluginVersion,
    },
}

impl From<&UndecidableCause> for WireUndecidableCause {
    fn from(cause: &UndecidableCause) -> Self {
        // NO wildcard arm: a variant added to `UndecidableCause` upstream
        // must fail this match at compile time, not fall through silently.
        match cause {
            UndecidableCause::EnvelopeConstruct(construct) => {
                WireUndecidableCause::EnvelopeConstruct {
                    construct: format!("{construct:?}"),
                }
            }
            UndecidableCause::PolicyConstruct {
                policy_id,
                policy_name,
                construct,
            } => WireUndecidableCause::PolicyConstruct {
                policy_id: *policy_id,
                policy_name: policy_name.clone(),
                construct: format!("{construct:?}"),
            },
            UndecidableCause::ServiceDef(issue) => WireUndecidableCause::ServiceDef {
                issue: format!("{issue:?}"),
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
            UndecidableCause::EngineMismatch {
                declared,
                service_def_name,
            } => WireUndecidableCause::EngineMismatch {
                declared: WireEngineType::from(*declared),
                service_def_name: service_def_name.clone(),
            },
            UndecidableCause::PluginVersionMismatch {
                engine,
                declared,
                measured,
            } => WireUndecidableCause::PluginVersionMismatch {
                engine: WireEngineType::from(*engine),
                declared: WirePluginVersion::from(*declared),
                measured: WirePluginVersion::from(*measured),
            },
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
