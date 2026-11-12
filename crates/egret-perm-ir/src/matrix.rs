//! The capability matrix: what each enforcement-point kind can enforce, and
//! how (governance spec, "Capability matrix"; plan D3).
//!
//! Every row is derived from what the compiler does today and names that code
//! in its `evidence` (line numbers against `origin/main` `91f7595e`). A
//! `(kind, version, capability)` with no row is missing, reason "not
//! measured" (spec D7).

use egret_perm::EngineType;

use crate::ep::EpInput;
/// Re-exported: a matrix question names its EP kind.
pub use crate::ep::{EpKind, Mechanism};
use crate::statement::{Privilege, ReasonCode};

/// The two rule effects a classification selector can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleEffectKind {
    Mask,
    RowFilter,
}

/// Something an enforcement point may or may not be able to enforce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    ClassificationSelector(RuleEffectKind),
    WithholdByClassification,
    ClassificationRowFilterOnColumnAttachment,
    GroupPrincipal,
    RolePrincipal,
    WithholdExemptionByGroup,
    WithholdPrivilege(Privilege),
    WithholdThroughView,
    WithholdThroughMaterializedView,
    MaskCoversHiddenColumns,
    PoliciesReachMetadataTables,
    EngineAsksAboutMetadataTables,
    MaskType(String),
}

/// How a capability the EP lacks natively is lowered into ones it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringKind {
    ExpandPerColumn,
    ExpandPerTable,
    ExpandColumnOnlyTablesPerTable,
    ExpandToUsers,
    ViewGuardMask,
    DenyCreateMaterializedView,
    DenyHiddenColumns,
    DenyMetadataTables,
}

/// The matrix's answer for one EP and one capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Native,
    Lowered(LoweringKind),
    Missing(ReasonCode),
}

/// A row's answer before the EP's classification channel is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Native,
    /// Native when the EP has a classification channel, else
    /// `Missing(NoClassificationChannel)`.
    NativeWithChannel,
    Lowered(LoweringKind),
    Missing(ReasonCode),
}

/// The mechanism versions a row was measured at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Versions {
    Any,
    Exactly(&'static [&'static str]),
}

/// Which capabilities a row answers: a fixed one, or every mask type the
/// engine's own declaration lists (M19).
#[derive(Debug, Clone, Copy)]
enum Key {
    Fixed(fn(&Capability) -> bool),
    MaskTypes(&'static [&'static str]),
}

/// One row of the matrix.
pub struct Entry {
    pub kind: EpKind,
    pub versions: Versions,
    /// The capability the row answers, for people reading the table.
    pub capability: &'static str,
    key: Key,
    answer: Answer,
    /// The code the row was derived from.
    pub evidence: &'static str,
}

const TRINO: EpKind = EpKind {
    engine: EngineType::Trino,
    mechanism: Mechanism::Ranger,
};
const STARROCKS: EpKind = EpKind {
    engine: EngineType::StarRocks,
    mechanism: Mechanism::Ranger,
};
/// A Trino profile whose engines egret answers through Trino's OPA plugin
/// (governance SP2b). Its rows are the `TRINO` rows, answer for answer (I5).
const OPA_TRINO: EpKind = EpKind {
    engine: EngineType::Trino,
    mechanism: Mechanism::Opa,
};

/// The mask types each service-def declares (`service-defs/{trino,starrocks}.json`
/// `dataMaskDef.maskTypes`); pinned to the file by
/// `egret-perm-compile`'s `ranger::rules::tests::matrix_mask_types_equal_the_service_def`.
pub const TRINO_MASK_TYPES: &[&str] = &[
    "MASK",
    "MASK_SHOW_LAST_4",
    "MASK_SHOW_FIRST_4",
    "MASK_HASH",
    "MASK_NULL",
    "MASK_NONE",
    "MASK_DATE_SHOW_YEAR",
    "CUSTOM",
];
pub const STARROCKS_MASK_TYPES: &[&str] = &[
    "MASK",
    "MASK_HASH",
    "MASK_NULL",
    "MASK_NONE",
    "MASK_DATE_SHOW_YEAR",
    "CUSTOM",
];

/// The StarRocks plugin versions the view guard (M13) was measured at.
const VIEW_GUARD_VERSIONS: &[&str] = &["2.8.0", "2.9.0"];

pub const ENTRIES: &[Entry] = &[
    // M1
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "ClassificationSelector(Mask)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::Mask)),
        answer: Answer::NativeWithChannel,
        evidence: "rules.rs:497-500 MissingTagService; trigger.rs:1173-1191; effect.rs:405-409",
    },
    // M2
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "ClassificationSelector(RowFilter)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::RowFilter)),
        answer: Answer::NativeWithChannel,
        evidence: "rules.rs:497-500; effect.rs:405-409",
    },
    // M3
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "ClassificationRowFilterOnColumnAttachment",
        key: Key::Fixed(|c| *c == Capability::ClassificationRowFilterOnColumnAttachment),
        answer: Answer::Lowered(LoweringKind::ExpandColumnOnlyTablesPerTable),
        evidence: "rules.rs:502-536 (KI-287)",
    },
    // M4
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "ClassificationSelector(Mask)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::Mask)),
        answer: Answer::Lowered(LoweringKind::ExpandPerColumn),
        evidence: "rules.rs:539-621",
    },
    // M5
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "ClassificationSelector(RowFilter)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::RowFilter)),
        answer: Answer::Lowered(LoweringKind::ExpandPerTable),
        evidence: "rules.rs:567-585",
    },
    // M6
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "WithholdByClassification",
        key: Key::Fixed(|c| *c == Capability::WithholdByClassification),
        answer: Answer::NativeWithChannel,
        evidence: "clearance.rs:334-345",
    },
    // M7
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "WithholdByClassification",
        key: Key::Fixed(|c| *c == Capability::WithholdByClassification),
        answer: Answer::Lowered(LoweringKind::ExpandPerColumn),
        evidence: "clearance.rs:350-363, 420-605",
    },
    // M8
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "GroupPrincipal",
        key: Key::Fixed(|c| *c == Capability::GroupPrincipal),
        answer: Answer::Lowered(LoweringKind::ExpandToUsers),
        evidence: "members.rs:690-692, 937-1013 (D7)",
    },
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "GroupPrincipal",
        key: Key::Fixed(|c| *c == Capability::GroupPrincipal),
        answer: Answer::Lowered(LoweringKind::ExpandToUsers),
        evidence: "members.rs:690-692, 937-1013 (D7)",
    },
    // M9
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "RolePrincipal",
        key: Key::Fixed(|c| *c == Capability::RolePrincipal),
        answer: Answer::Lowered(LoweringKind::ExpandToUsers),
        evidence: "members.rs:174, 964-980 (D0)",
    },
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "RolePrincipal",
        key: Key::Fixed(|c| *c == Capability::RolePrincipal),
        answer: Answer::Lowered(LoweringKind::ExpandToUsers),
        evidence: "members.rs:174, 964-980 (D0)",
    },
    // M10
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "WithholdExemptionByGroup",
        key: Key::Fixed(|c| *c == Capability::WithholdExemptionByGroup),
        answer: Answer::Missing(ReasonCode::WithholdExemptionByGroupOnly),
        evidence: "clearance.rs:289-299; people.rs:414,430,1023,1037,1234,1242,1322",
    },
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "WithholdExemptionByGroup",
        key: Key::Fixed(|c| *c == Capability::WithholdExemptionByGroup),
        answer: Answer::Missing(ReasonCode::WithholdExemptionByGroupOnly),
        evidence: "clearance.rs:289-299; people.rs:414,430,1023,1037,1234,1242,1322",
    },
    // M11
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "WithholdPrivilege(Read)",
        key: Key::Fixed(|c| *c == Capability::WithholdPrivilege(Privilege::Read)),
        answer: Answer::Native,
        evidence: "clearance.rs:1143-1146",
    },
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "WithholdPrivilege(Write)",
        key: Key::Fixed(|c| *c == Capability::WithholdPrivilege(Privilege::Write)),
        answer: Answer::Native,
        evidence: "clearance.rs:1143-1146",
    },
    // M12 (no Write row: missing, "not measured")
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "WithholdPrivilege(Read)",
        key: Key::Fixed(|c| *c == Capability::WithholdPrivilege(Privilege::Read)),
        answer: Answer::Native,
        evidence: "clearance.rs:1123-1158",
    },
    // M13
    Entry {
        kind: STARROCKS,
        versions: Versions::Exactly(VIEW_GUARD_VERSIONS),
        capability: "WithholdThroughView",
        key: Key::Fixed(|c| *c == Capability::WithholdThroughView),
        answer: Answer::Lowered(LoweringKind::ViewGuardMask),
        evidence: "clearance_mask.rs:425-460; trigger.rs:609-629",
    },
    // M15
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "WithholdThroughMaterializedView",
        key: Key::Fixed(|c| *c == Capability::WithholdThroughMaterializedView),
        answer: Answer::Lowered(LoweringKind::DenyCreateMaterializedView),
        evidence: "clearance.rs:971-1039",
    },
    // M16
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "MaskCoversHiddenColumns",
        key: Key::Fixed(|c| *c == Capability::MaskCoversHiddenColumns),
        answer: Answer::Lowered(LoweringKind::DenyHiddenColumns),
        evidence: "trigger.rs:631-669; clearance.rs:654-800",
    },
    // M17
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "PoliciesReachMetadataTables",
        key: Key::Fixed(|c| *c == Capability::PoliciesReachMetadataTables),
        answer: Answer::Lowered(LoweringKind::DenyMetadataTables),
        evidence: "trigger.rs:671-742; metadata_tables.rs",
    },
    // M18
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "EngineAsksAboutMetadataTables",
        key: Key::Fixed(|c| *c == Capability::EngineAsksAboutMetadataTables),
        answer: Answer::Missing(ReasonCode::NotMeasured),
        evidence: "metadata_tables.rs:39-43; StarRocksMetadataTablesWarning.tsx",
    },
    // M19
    Entry {
        kind: TRINO,
        versions: Versions::Any,
        capability: "MaskType(name)",
        key: Key::MaskTypes(TRINO_MASK_TYPES),
        answer: Answer::Native,
        evidence: "rules.rs:825-865",
    },
    Entry {
        kind: STARROCKS,
        versions: Versions::Any,
        capability: "MaskType(name)",
        key: Key::MaskTypes(STARROCKS_MASK_TYPES),
        answer: Answer::Native,
        evidence: "rules.rs:825-865",
    },
    // SP2b: one OPA_TRINO row per TRINO row (I5).
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "ClassificationSelector(Mask)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::Mask)),
        answer: Answer::NativeWithChannel,
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (tag mask rules, asked by GetColumnMask)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "ClassificationSelector(RowFilter)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::RowFilter)),
        answer: Answer::NativeWithChannel,
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (tag row-filter rules, asked by GetRowFilters)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "ClassificationRowFilterOnColumnAttachment",
        key: Key::Fixed(|c| *c == Capability::ClassificationRowFilterOnColumnAttachment),
        answer: Answer::Lowered(LoweringKind::ExpandColumnOnlyTablesPerTable),
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (tag row filters over column-only classified tables, asked by GetRowFilters)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "WithholdByClassification",
        key: Key::Fixed(|c| *c == Capability::WithholdByClassification),
        answer: Answer::NativeWithChannel,
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (role-gated classifications, asked by the data and reach operations)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "GroupPrincipal",
        key: Key::Fixed(|c| *c == Capability::GroupPrincipal),
        answer: Answer::Lowered(LoweringKind::ExpandToUsers),
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (grants to a group and to public)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "RolePrincipal",
        key: Key::Fixed(|c| *c == Capability::RolePrincipal),
        answer: Answer::Lowered(LoweringKind::ExpandToUsers),
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (grants to a platform role)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "WithholdExemptionByGroup",
        key: Key::Fixed(|c| *c == Capability::WithholdExemptionByGroup),
        answer: Answer::Missing(ReasonCode::WithholdExemptionByGroupOnly),
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (group-only clearance gates, which neither side withholds)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "WithholdPrivilege(Read)",
        key: Key::Fixed(|c| *c == Capability::WithholdPrivilege(Privilege::Read)),
        answer: Answer::Native,
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (role-gated classifications, asked by SelectFromColumns and reach)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "WithholdPrivilege(Write)",
        key: Key::Fixed(|c| *c == Capability::WithholdPrivilege(Privilege::Write)),
        answer: Answer::Native,
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (role-gated classifications, asked by the write operations)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "PoliciesReachMetadataTables",
        key: Key::Fixed(|c| *c == Capability::PoliciesReachMetadataTables),
        answer: Answer::Lowered(LoweringKind::DenyMetadataTables),
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (questions on t3$files under restrictions on t3)",
    },
    Entry {
        kind: OPA_TRINO,
        versions: Versions::Any,
        capability: "MaskType(name)",
        key: Key::MaskTypes(TRINO_MASK_TYPES),
        answer: Answer::Native,
        evidence: "crates/egret-perm-compile/tests/opa_differential.rs:opa_decisions_agree_with_ranger (MASK, MASK_HASH, MASK_NULL, MASK_NONE and CUSTOM masks, asked by GetColumnMask)",
    },
];

fn version_matches(v: Versions, version: Option<&str>) -> bool {
    match (v, version) {
        (Versions::Any, _) => true,
        (Versions::Exactly(list), Some(x)) => list.contains(&x),
        (Versions::Exactly(_), None) => false,
    }
}

fn key_matches(k: Key, cap: &Capability) -> bool {
    match (k, cap) {
        (Key::Fixed(f), c) => f(c),
        (Key::MaskTypes(list), Capability::MaskType(t)) => list.contains(&t.as_str()),
        (Key::MaskTypes(_), _) => false,
    }
}

/// The matrix's answer for an EP kind at one mechanism version (`None`:
/// unknown) with or without a classification channel. Spec D7: an absent
/// combination is missing, reason "not measured".
pub fn support_for_kind(
    kind: EpKind,
    version: Option<&str>,
    classification_channel: bool,
    cap: &Capability,
) -> Support {
    let Some(e) = ENTRIES.iter().find(|e| {
        e.kind == kind && key_matches(e.key, cap) && version_matches(e.versions, version)
    }) else {
        return Support::Missing(ReasonCode::NotMeasured);
    };
    match e.answer {
        Answer::Native => Support::Native,
        Answer::NativeWithChannel if classification_channel => Support::Native,
        Answer::NativeWithChannel => Support::Missing(ReasonCode::NoClassificationChannel),
        Answer::Lowered(l) => Support::Lowered(l),
        Answer::Missing(r) => Support::Missing(r),
    }
}

/// Every bound engine's version must support `cap` the same way; with none
/// bound, egret's default image decides (as
/// [`crate::lower::withhold::view_guard_versions`] does).
pub fn support(ep: &EpInput, cap: &Capability) -> Support {
    let versions: Vec<Option<&str>> = if ep.engines.is_empty() {
        vec![ep.default_version.as_deref().ok()]
    } else {
        ep.engines
            .iter()
            .map(|e| e.version.as_deref().ok())
            .collect()
    };
    let mut answer = None;
    for v in versions {
        let s = support_for_kind(ep.kind, v, ep.classification_channel, cap);
        match answer {
            None => answer = Some(s),
            Some(a) if a == s => {}
            Some(_) => return Support::Missing(ReasonCode::VersionsDisagree),
        }
    }
    answer.unwrap_or(Support::Missing(ReasonCode::NotMeasured))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::EngineType;

    fn trino() -> EpKind {
        EpKind {
            engine: EngineType::Trino,
            mechanism: Mechanism::Ranger,
        }
    }
    fn starrocks() -> EpKind {
        EpKind {
            engine: EngineType::StarRocks,
            mechanism: Mechanism::Ranger,
        }
    }

    #[test]
    fn a_trino_classification_mask_needs_the_channel() {
        let cap = Capability::ClassificationSelector(RuleEffectKind::Mask);
        assert_eq!(support_for_kind(trino(), None, true, &cap), Support::Native);
        assert_eq!(
            support_for_kind(trino(), None, false, &cap),
            Support::Missing(ReasonCode::NoClassificationChannel)
        );
    }

    #[test]
    fn starrocks_expands_classification_rules() {
        assert_eq!(
            support_for_kind(
                starrocks(),
                None,
                false,
                &Capability::ClassificationSelector(RuleEffectKind::Mask)
            ),
            Support::Lowered(LoweringKind::ExpandPerColumn)
        );
        assert_eq!(
            support_for_kind(
                starrocks(),
                None,
                false,
                &Capability::ClassificationSelector(RuleEffectKind::RowFilter)
            ),
            Support::Lowered(LoweringKind::ExpandPerTable)
        );
    }

    #[test]
    fn the_view_guard_is_measured_at_two_starrocks_versions_only() {
        let cap = Capability::WithholdThroughView;
        for v in ["2.8.0", "2.9.0"] {
            assert_eq!(
                support_for_kind(starrocks(), Some(v), false, &cap),
                Support::Lowered(LoweringKind::ViewGuardMask)
            );
        }
        assert_eq!(
            support_for_kind(starrocks(), Some("2.10.0"), false, &cap),
            Support::Missing(ReasonCode::NotMeasured)
        );
    }

    #[test]
    fn an_unknown_combination_is_missing_not_measured() {
        assert_eq!(
            support_for_kind(trino(), None, true, &Capability::MaskCoversHiddenColumns),
            Support::Missing(ReasonCode::NotMeasured)
        );
    }

    #[test]
    fn group_only_withhold_exemptions_are_missing_on_ranger() {
        for k in [trino(), starrocks()] {
            assert_eq!(
                support_for_kind(k, None, true, &Capability::WithholdExemptionByGroup),
                Support::Missing(ReasonCode::WithholdExemptionByGroupOnly)
            );
        }
    }

    #[test]
    fn starrocks_withholds_read_only() {
        assert_eq!(
            support_for_kind(
                starrocks(),
                None,
                false,
                &Capability::WithholdPrivilege(Privilege::Read)
            ),
            Support::Native
        );
        assert_eq!(
            support_for_kind(
                starrocks(),
                None,
                false,
                &Capability::WithholdPrivilege(Privilege::Write)
            ),
            Support::Missing(ReasonCode::NotMeasured)
        );
        assert_eq!(
            support_for_kind(
                trino(),
                None,
                true,
                &Capability::WithholdPrivilege(Privilege::Write)
            ),
            Support::Native
        );
    }

    #[test]
    fn every_entry_names_its_evidence() {
        for e in ENTRIES {
            assert!(
                e.evidence.contains(".rs:") || e.evidence.contains(".tsx"),
                "{:?} has no evidence",
                e.capability
            );
        }
    }

    /// I5: an OPA Trino profile is answered from the same lowered statements,
    /// so every Trino row has an OPA twin that answers the same.
    #[test]
    fn every_trino_row_has_an_identical_opa_trino_row() {
        let trino: Vec<_> = ENTRIES.iter().filter(|e| e.kind == TRINO).collect();
        let opa: Vec<_> = ENTRIES.iter().filter(|e| e.kind == OPA_TRINO).collect();
        assert_eq!(trino.len(), opa.len());
        for t in &trino {
            assert!(
                opa.iter().any(|o| o.capability == t.capability
                    && o.versions == t.versions
                    && o.answer == t.answer),
                "no identical OPA_TRINO row for {}",
                t.capability
            );
        }
    }

    #[test]
    fn opa_trino_rows_answer_like_the_ranger_trino_rows() {
        let ranger = EpKind {
            engine: EngineType::Trino,
            mechanism: Mechanism::Ranger,
        };
        let opa = EpKind {
            engine: EngineType::Trino,
            mechanism: Mechanism::Opa,
        };
        let caps = [
            Capability::ClassificationSelector(RuleEffectKind::Mask),
            Capability::ClassificationSelector(RuleEffectKind::RowFilter),
            Capability::ClassificationRowFilterOnColumnAttachment,
            Capability::WithholdByClassification,
            Capability::GroupPrincipal,
            Capability::RolePrincipal,
            Capability::WithholdExemptionByGroup,
            Capability::WithholdPrivilege(Privilege::Read),
            Capability::WithholdPrivilege(Privilege::Write),
            Capability::PoliciesReachMetadataTables,
            Capability::MaskType("MASK_HASH".into()),
            Capability::MaskType("CUSTOM".into()),
        ];
        for channel in [true, false] {
            for c in &caps {
                assert_eq!(
                    support_for_kind(opa, Some("2.8.0"), channel, c),
                    support_for_kind(ranger, Some("2.8.0"), channel, c),
                    "{c:?} channel={channel}"
                );
            }
        }
    }

    fn ep(engines: &[Result<&str, &str>], default_version: Result<&str, &str>) -> EpInput {
        EpInput {
            profile_id: 1,
            profile_name: "sr".into(),
            kind: starrocks(),
            engines: engines
                .iter()
                .enumerate()
                .map(|(i, v)| crate::ep::BoundEngine {
                    name: format!("e{i}"),
                    version: v.map(str::to_string).map_err(str::to_string),
                    egret_image_level: None,
                })
                .collect(),
            default_version: default_version.map(str::to_string).map_err(str::to_string),
            default_image_level: None,
            classification_channel: false,
            catalogs: Default::default(),
        }
    }

    #[test]
    fn support_asks_every_bound_engine_and_falls_back_to_the_default_image() {
        let cap = Capability::WithholdThroughView;
        let guard = Support::Lowered(LoweringKind::ViewGuardMask);
        assert_eq!(support(&ep(&[], Ok("2.9.0")), &cap), guard);
        assert_eq!(
            support(&ep(&[], Err("no default image")), &cap),
            Support::Missing(ReasonCode::NotMeasured)
        );
        assert_eq!(
            support(&ep(&[Ok("2.8.0"), Ok("2.9.0")], Ok("x")), &cap),
            guard
        );
        assert_eq!(
            support(&ep(&[Ok("2.8.0"), Ok("2.10.0")], Ok("2.9.0")), &cap),
            Support::Missing(ReasonCode::VersionsDisagree)
        );
    }
}
