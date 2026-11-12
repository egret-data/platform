//! Who enforces a rule, and why nobody does: the capability matrix plus the
//! catalog scope. `NotEnforced` on the wire is generated from here.

use std::collections::BTreeSet;

use egret_perm::EngineType;
use uuid::Uuid;

use crate::ep::{EpKind, Mechanism};
use crate::matrix::{support_for_kind, Capability, RuleEffectKind, Support};
use crate::statement::ReasonCode;

/// An engine attached to a catalog, with its profile's kind when bound.
#[derive(Debug, Clone)]
pub struct AttachedEngine {
    pub catalog_id: Uuid,
    pub engine_id: Uuid,
    pub engine_name: String,
    pub engine_type: String,
    /// `None`: no profile. `Some(None)`: a profile of a type no EP kind models
    /// (today's SQL counts it as enforcing; kept, see Q2).
    pub profile: Option<Option<EngineType>>,
    /// The profile can resolve classifications itself (a `tagService` naming
    /// an existing service).
    pub classification_channel: bool,
    /// The egret fork patch level of the engine's image (`…-egret.N`); `None`
    /// when unknown (official image, Trino). StarRocks mask / row-filter
    /// selectors are enforced only at or above the matrix's minimum.
    pub egret_image_level: Option<u32>,
}

/// Why nothing enforces a rule, on one catalog or on the whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotEnforcedReason {
    /// Scope: the rule reaches no catalog (only on the rule as a whole).
    NoAssignment,
    /// Scope: no engine is attached to the catalog(s).
    NoEngine,
    /// Scope: engines are attached, none is bound to a profile.
    NoProfile,
    /// Matrix: every bound profile answers the classification selector with
    /// `Missing(NoClassificationChannel)` (M1, M2).
    NoClassificationChannel,
}

/// One catalog a rule reaches, and who enforces it there.
#[derive(Debug, Clone)]
pub struct CatalogEnforcement {
    pub catalog_id: Uuid,
    pub catalog_name: String,
    /// `(engine id, engine name, engine type)`, in `attached` order.
    pub engines: Vec<(Uuid, String, String)>,
    pub not_enforced: Option<NotEnforcedReason>,
}

fn enforces(a: &AttachedEngine, classification_selector: bool, effect: RuleEffectKind) -> bool {
    match a.profile {
        None => false,
        Some(None) => true,
        Some(Some(engine)) => {
            !classification_selector
                || !matches!(
                    support_for_kind(
                        EpKind {
                            engine,
                            mechanism: Mechanism::Ranger,
                        },
                        None,
                        a.egret_image_level,
                        a.classification_channel,
                        &Capability::ClassificationSelector(effect),
                    ),
                    Support::Missing(_)
                )
        }
    }
}

/// Who enforces a rule that reaches `reached` (`(catalog id, name)`), given
/// the `attached` engines: per catalog, the enforcing engines and why none
/// does; overall, the number of distinct enforcing engines and why none does.
/// A `classification_selector` rule is enforced only by engines whose kind
/// supports the selector for `effect` (moved from `effect::enforcement`).
pub fn rule_enforcement(
    reached: &[(Uuid, String)],
    attached: &[AttachedEngine],
    classification_selector: bool,
    effect: RuleEffectKind,
) -> (Vec<CatalogEnforcement>, u32, Option<NotEnforcedReason>) {
    let mut catalogs = Vec::with_capacity(reached.len());
    let mut engine_ids = BTreeSet::new();
    for (catalog_id, catalog_name) in reached {
        let here: Vec<&AttachedEngine> = attached
            .iter()
            .filter(|a| a.catalog_id == *catalog_id)
            .collect();
        let bound = here.iter().any(|a| a.profile.is_some());
        let engines: Vec<(Uuid, String, String)> = here
            .iter()
            .filter(|a| enforces(a, classification_selector, effect))
            .map(|a| (a.engine_id, a.engine_name.clone(), a.engine_type.clone()))
            .collect();
        engine_ids.extend(engines.iter().map(|e| e.0));
        let not_enforced = if here.is_empty() {
            Some(NotEnforcedReason::NoEngine)
        } else if !bound {
            Some(NotEnforcedReason::NoProfile)
        } else if engines.is_empty() {
            Some(NotEnforcedReason::NoClassificationChannel)
        } else {
            None
        };
        catalogs.push(CatalogEnforcement {
            catalog_id: *catalog_id,
            catalog_name: catalog_name.clone(),
            engines,
            not_enforced,
        });
    }
    let has = |r| catalogs.iter().any(|c| c.not_enforced == Some(r));
    let not_enforced = if catalogs.is_empty() {
        Some(NotEnforcedReason::NoAssignment)
    } else if !engine_ids.is_empty() {
        None
    } else if has(NotEnforcedReason::NoClassificationChannel) {
        Some(NotEnforcedReason::NoClassificationChannel)
    } else if has(NotEnforcedReason::NoProfile) {
        Some(NotEnforcedReason::NoProfile)
    } else {
        Some(NotEnforcedReason::NoEngine)
    };
    (
        catalogs,
        u32::try_from(engine_ids.len()).unwrap_or(u32::MAX),
        not_enforced,
    )
}

/// M10: why a withhold's group exemptions are not enforced on a Ranger EP,
/// or `None` when it names no group. A withhold whose exemptions are groups
/// only (`required_roles_empty`) is therefore not enforced there at all; one
/// with roles enforces only its role half. The answer is the same for every
/// Ranger kind (M10 has a row for each), so the kind asked is Trino.
pub fn withhold_engine_reason(
    required_roles_empty: bool,
    required_groups_empty: bool,
) -> Option<ReasonCode> {
    // Whether the role half is empty does not change why the group half is
    // unenforced; the caller combines the two (see `people.rs`).
    let _ = required_roles_empty;
    if required_groups_empty {
        return None;
    }
    let k = EpKind {
        engine: EngineType::Trino,
        mechanism: Mechanism::Ranger,
    };
    match support_for_kind(k, None, None, true, &Capability::WithholdExemptionByGroup) {
        Support::Missing(r) => Some(r),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(
        catalog: Uuid,
        n: u128,
        profile: Option<Option<EngineType>>,
        channel: bool,
    ) -> AttachedEngine {
        AttachedEngine {
            catalog_id: catalog,
            engine_id: Uuid::from_u128(n),
            engine_name: format!("e{n}"),
            engine_type: "trino".into(),
            profile,
            classification_channel: channel,
            egret_image_level: Some(4),
        }
    }

    #[test]
    fn a_profile_of_an_unmodelled_type_still_counts_as_enforcing() {
        let c = Uuid::from_u128(100);
        let reached = [(c, "sales".to_string())];
        let attached = [engine(c, 1, Some(None), false)];
        for effect in [RuleEffectKind::Mask, RuleEffectKind::RowFilter] {
            let (catalogs, by, why) = rule_enforcement(&reached, &attached, true, effect);
            assert_eq!(by, 1);
            assert_eq!(why, None);
            assert_eq!(catalogs[0].not_enforced, None);
        }
    }

    #[test]
    fn a_classification_rule_on_trino_without_a_channel_is_not_enforced() {
        let c = Uuid::from_u128(100);
        let reached = [(c, "sales".to_string())];
        let no_channel = [engine(c, 1, Some(Some(EngineType::Trino)), false)];
        let (_, by, why) = rule_enforcement(&reached, &no_channel, true, RuleEffectKind::Mask);
        assert_eq!(
            (by, why),
            (0, Some(NotEnforcedReason::NoClassificationChannel))
        );
        // A path rule needs no channel.
        let (_, by, why) = rule_enforcement(&reached, &no_channel, false, RuleEffectKind::Mask);
        assert_eq!((by, why), (1, None));
        // StarRocks lowers the selector, so it enforces without a channel.
        let sr = [engine(c, 2, Some(Some(EngineType::StarRocks)), false)];
        let (_, by, why) = rule_enforcement(&reached, &sr, true, RuleEffectKind::RowFilter);
        assert_eq!((by, why), (1, None));
    }

    #[test]
    fn scope_outcomes_and_their_precedence() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            rule_enforcement(&[], &[], false, RuleEffectKind::Mask).2,
            Some(NotEnforcedReason::NoAssignment)
        );
        let reached = [(a, "a".to_string()), (b, "b".to_string())];
        // `a` has nothing attached, `b` an unbound engine: no_profile wins.
        let attached = [engine(b, 1, None, false)];
        let (catalogs, by, why) =
            rule_enforcement(&reached, &attached, false, RuleEffectKind::Mask);
        assert_eq!(by, 0);
        assert_eq!(catalogs[0].not_enforced, Some(NotEnforcedReason::NoEngine));
        assert_eq!(catalogs[1].not_enforced, Some(NotEnforcedReason::NoProfile));
        assert_eq!(why, Some(NotEnforcedReason::NoProfile));
        assert_eq!(
            rule_enforcement(&reached, &[], false, RuleEffectKind::Mask).2,
            Some(NotEnforcedReason::NoEngine)
        );
    }

    #[test]
    fn only_a_withholds_group_half_is_unenforced() {
        assert_eq!(
            withhold_engine_reason(true, false),
            Some(ReasonCode::WithholdExemptionByGroupOnly)
        );
        assert_eq!(
            withhold_engine_reason(false, false),
            Some(ReasonCode::WithholdExemptionByGroupOnly)
        );
        assert_eq!(withhold_engine_reason(false, true), None);
    }

    #[test]
    fn a_starrocks_classification_rule_below_the_image_gate_is_not_enforced() {
        let c = Uuid::from_u128(100);
        let reached = [(c, "sales".to_string())];
        for (level, enforced) in [(Some(2), 0), (None, 0), (Some(3), 1), (Some(4), 1)] {
            let mut sr = engine(c, 2, Some(Some(EngineType::StarRocks)), false);
            sr.egret_image_level = level;
            let (_, by, _) = rule_enforcement(&reached, &[sr], true, RuleEffectKind::Mask);
            assert_eq!(by, enforced, "egret image level {level:?}");
        }
    }
}
