//! The neutral statement set (plan D2): what a lowering hands every target,
//! and why something was not lowered.

use egret_perm::CompileError;
use uuid::Uuid;

use crate::audience::Audience;

/// The model object a statement or a lowering came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Grant {
        grant_id: Uuid,
    },
    Rule {
        rule_id: Uuid,
        rule_name: String,
    },
    Withhold {
        classification_id: Uuid,
        classification_name: String,
    },
    EnginePrivilege {
        engine_grant_id: Uuid,
    },
}

/// One level of a [`Path`]: a value the author wrote, or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Named(String),
    Unnamed,
}

/// A path in the EP's scope. `catalog_id` is `None` only for a path the EP
/// owns outright (the StarRocks MV guard's `default_catalog`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub catalog_id: Option<Uuid>,
    pub catalog: String,
    pub schema: Level,
    pub table: Level,
    pub column: Level,
}

/// What a statement applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Path(Path),
    /// Resolved by the EP itself (matrix: `ClassificationSelector` native).
    Classification {
        name: String,
    },
    /// An engine-typed object (D8): the capability's own values.
    EngineObject {
        capability: String,
        values: Vec<String>,
    },
}

/// A privilege a statement allows, denies or withholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Privilege {
    Read,
    Write,
    CreateMaterializedView,
}

/// What a statement does to its audience on its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Allow {
        privileges: Vec<Privilege>,
    },
    Deny {
        privileges: Vec<Privilege>,
    },
    Mask {
        mask_type: String,
        expression: Option<String>,
    },
    RowFilter {
        expression: String,
    },
    /// Deny to `audience.include` except `audience.except` (today's clearance).
    Withhold {
        privileges: Vec<Privilege>,
    },
    /// The view guard of a withhold: everyone but the exempt sees NULL.
    WithholdMask,
    EnginePrivilege {
        capability: String,
    },
}

/// How the statement came to exist. Drives nothing but naming in a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Derivation {
    AsDeclared,
    /// The matrix lowered a classification selector to concrete paths.
    ExpandedFromClassification,
    /// A table-level classification on an EP that cannot select by classification.
    WholeTable,
    /// The matrix's materialized-view guard of a withhold.
    MaterializedViewGuard,
}

/// One neutral statement: who, on what, does what, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub source: Source,
    pub audience: Audience,
    pub target: Target,
    pub effect: Effect,
    pub derivation: Derivation,
}

/// Why something was not compiled (or compiled weaker), with today's exact text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonCode {
    EmptyAudience,
    UnexpandedPrincipal,
    UnknownClassification,
    NoClassificationChannel,
    WithholdExemptionByGroupOnly,
    ViewNotMeasured,
    TableNotInCatalog,
    NotMeasured,
    VersionsDisagree,
    MaskPrincipalGap,
    ImpersonationTarget,
    InvalidEnginePrivilege,
}

/// A source that lowered to nothing (or to something weaker), and why.
/// `detail` is the exact text a target reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lowering {
    pub source: Source,
    pub code: ReasonCode,
    pub detail: String,
}

/// The output of one lowering: statements and lowerings, each in source order.
#[derive(Debug, Clone, Default)]
pub struct Lowered {
    pub statements: Vec<Statement>,
    pub lowerings: Vec<Lowering>,
}

/// A refusal that aborts today aborts the same way.
pub type LowerResult = Result<Lowered, CompileError>;
