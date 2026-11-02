//! The neutral statement set. Sub-project 1 Task 2 holds only the two types
//! the capability matrix needs; the statements themselves arrive with the
//! first family lowered through the IR.

/// A privilege a statement allows, denies or withholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Privilege {
    Read,
    Write,
    CreateMaterializedView,
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
