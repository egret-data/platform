//! **One rule, for every surface that can write a mask or a row-filter policy:
//! a profile must not serve two of them that arbitrate the same request and
//! would arbitrate it DIFFERENTLY.** SP4d debt Task 6, item 1.
//!
//! This is the WRITE side of [`egret_ranger_eval`'s
//! `UndecidableCause::EvalOrderAmbiguous`][read-side]. Until this module
//! existed, `admin/ranger_json.rs` carried the same observation as a COMMENT at
//! two sites — the oldest carried debt in this programme, recorded since SP4a —
//! and the compiler went on emitting the artifact.
//!
//! [read-side]: https://docs.rs/egret-ranger-eval
//!
//! # The measurement
//!
//! `docs/en/plans/2026-09-28-sp4d-1-live-result.md` §8, finding **F2**. The
//! input was a real download — `GET /service/plugins/policies/download/
//! sp4a-sr-crosscheck`, `policyVersion 54`, 34 policies, confirmed against the
//! StarRocks FE's own policy cache. In it, egret's own rule compiler had
//! emitted **two collision groups**:
//!
//! * policies `8777` and `8801`, both `policyType 1`, both `policyPriority 0`,
//!   both selecting `sp4a10_wh.sp4a.report.item`, both granting `select` to
//!   `groups: ["public"]` — and **both `dataMaskType: "MASK"` with no
//!   `valueExpr`**. An AGREEING tie: whichever evaluator runs first, the column
//!   is masked the same way.
//! * policies `8776` (`country = 'US'`) and `8806` (`id > 2`), both
//!   `policyType 2`, both `policyPriority 0`, both selecting the table
//!   `sp4a10_wh.sp4a.report` — a **DIFFERING** tie. Which rows a reader sees
//!   depends on which one the engine happens to apply.
//!
//! **This guard refuses the second and admits the first**, and the asymmetry is
//! the whole design — see "Why agreeing ties are admitted" below.
//!
//! # What actually decides the winner, stated more carefully than the comment
//! this replaces
//!
//! The two comments this module retires said "Ranger defines no order between
//! them". That is **stronger than anything this repository has measured**, and
//! the correction matters because it changes what a steward should be told.
//! What the repository records, from the jars, is
//! `RangerPolicyEvaluator.PolicyEvalOrderComparator` (quoted in
//! `egret_ranger_eval::eval::sort_eval_order` and in
//! `docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md` §1d):
//!
//! ```text
//! Integer.compare(other.getPolicyPriority(), me.getPolicyPriority())   // DESC
//! then: hasDeny() first
//! then: Integer.compare(me.getEvalOrder(), other.getEvalOrder())
//! then: compareStrings(policy name)
//! ```
//!
//! That is a TOTAL order, so the engine's answer is **deterministic**. It is
//! not, however, anything the steward authored: `getEvalOrder()` is a
//! wildcard-count score (more match-any `*` levels sort FIRST — see "Why
//! different resources are refused too"), so two policies selecting the SAME
//! resource score the same, and the decision falls through to the policy NAME — which
//! for a compiled policy is `perm-rule-<rule uuid>` (`COMPILER_RULE_PREFIX`).
//! The winner is therefore a uuid lottery, stable until somebody deletes and
//! re-creates a rule. (And if the jar's final key turns out to be the stable
//! sort's input order rather than the name — `Collections.sort` is stable, and
//! upstream also ships a separate `PolicyNameComparator` — then it is the
//! download's ROW ORDER instead. Either way it is not the model.)
//!
//! So the refusal does not say "nondeterministic". It says the artifact does
//! not express which rule wins, which is true under both readings and is the
//! thing the steward can act on.
//!
//! # The collision key, spelled one-for-one with the evaluator's own band
//!
//! Two policies collide when they land in the SAME evaluator band and MAY both
//! decide one request, which `egret_ranger_eval` spells as `(policy_priority,
//! has_deny)` within one `policyType` list. This module's band key is
//! `(service, policy_type, has_deny, policy_priority)`, and inside a band every
//! pair is compared with [`resources_may_intersect`] (see "Why different
//! resources are refused too" below):
//!
//! * `policy_type` ∈ {1, 2} only. `RangerPolicyRepository.init` files policies
//!   into three disjoint evaluator lists by the raw code, so a mask can never
//!   tie with a row filter, and type 0 (access) is not in this class at all:
//!   its arbitration is `>` rather than `>=` and deny-wins makes order
//!   immaterial on every arm but the tag-deny one.
//! * `has_deny` is `isDenyAllElse || !denyPolicyItems.is_empty()`, the spelling
//!   `RangerAbstractPolicyEvaluator.hasDeny()` uses and
//!   `egret_ranger_eval::eval::PolicyView::has_deny` reproduces. It is in the
//!   key because the comparator puts a `hasDeny` policy AHEAD of one without at
//!   equal priority — two policies that differ here are ordered, not tied. No
//!   body this compiler emits for type 1 or 2 carries either field non-empty,
//!   so today it is always `false`; it is in the key so that stays a
//!   measurement rather than an assumption.
//! * absent/`null` `isEnabled` means ENABLED, absent/`null` `policyPriority`
//!   means 0 and absent/`null` `policyType` means 0 — keyed one-for-one with
//!   `egret_ranger_eval::model::RawRangerPolicy`'s serde defaults
//!   (`default = "default_true"` / `null_as_true`, `null_as_default`), because
//!   two guards that must agree written to different shapes has produced a
//!   defect in this programme five times.
//! * `resources` are read through [`canonical_resources`]: levels sorted by
//!   name, values sorted and de-duplicated (Ranger builds one matcher per value
//!   and matches if ANY matches, so their order cannot matter), and a level
//!   whose `values` is empty treated as ABSENT, because
//!   `RangerAbstractResourceMatcher.init()` drops an empty policy value before
//!   building a matcher for it and a level with no matchers is `isMatchAny` —
//!   the same reading `CompileError::EmptyScopeLevel` rests on. An absent level
//!   may-intersect anything, which is the fail-CLOSED direction (it finds more
//!   collisions); nothing this compiler emits can reach the empty case anyway,
//!   since `ranger_json::resources_object` drops empty levels and migration
//!   `0178` forbids an empty scope level upstream of it.
//!
//! # Why different resources are refused too (SP4d-4 Part A)
//!
//! Until SP4d-4 the key above carried `canonical_resources`, so two policies
//! collided only when their resource maps were EQUAL, on the argument that a
//! mask on `cat.sch.*.col` and a mask on `cat.sch.tbl.col` carry different
//! specificity, that `getEvalOrder()` separates them, and that the engine's
//! choice is therefore "determined by the artifact". The first two halves are
//! true. The conclusion a steward would draw from them is not:
//!
//! * **The arithmetic.** `RangerOptimizedPolicyEvaluator.computeEvalOrder()`,
//!   read at both deployed tags (`release-ranger-2.5.0`, lines 55 and 193-228;
//!   byte-identical arithmetic at `release-ranger-2.8.0`): `evalOrder = 10000 −
//!   min(100, Σ resource discounts) − principal discount − access discount −
//!   condition discount`, where a level whose values contain the match-any `*`
//!   earns a discount of **25**, a level with any other `*` earns 10, and `?`
//!   earns 5. A level the policy does not name earns NO discount.
//! * **The sort.** `RangerPolicyEvaluator.PolicyEvalOrderComparator` (lines
//!   203-222 at both tags) orders `evalOrder` **ascending** — the policy with
//!   MORE wildcard values sorts FIRST.
//! * **The arbitration.** Masks and row filters are arbitrated with `>=`
//!   (`egret_ranger_eval::eval`, the mask/row-filter walk), so the FIRST
//!   matching evaluator in a band is final.
//! * **The consequence.** For two compiled rules every other discount is equal
//!   (both carry the `public` group, `select` only, no conditions), so the
//!   BROADER policy wins, and the narrower one never applies to anyone the
//!   broader one's items cover. A steward who writes `(sales, *, email) → MASK`
//!   and `(sales, orders, email) → MASK_NULL` gets `MASK` on `orders.email`.
//!
//! Measured, not only read: `docs/en/plans/2026-10-11-sp4d-4-task1-levels-
//! result.md` §4, pin 1 — on both jars and under both service-defs the broad
//! policy's mask is served, robust to swapping the two policies' names, ids,
//! download order and mask types. "Broader" there means more match-any `*`
//! values with equal principal/access/condition discounts — NOT "fewer levels
//! named", since an omitted level earns no discount (§4, §8 item 1).
//!
//! So the outcome of such a pair is decided by a score the steward did not
//! author and that inverts what the configuration reads as. The key no longer
//! carries the resources: inside one band, every pair whose resources
//! [`resources_may_intersect`] and whose decisive items differ is refused.
//! `resources_may_intersect` over-approximates (a wildcard it does not
//! evaluate, or an `isExcludes` level, is ASSUMED to intersect) — the
//! fail-closed direction for a refusal.
//!
//! ⚠ What is still true, stated rather than hidden: **egret can still emit an
//! artifact its own evaluator declines.** `EvalOrderAmbiguous` is raised for an
//! AGREEING tie too (see the next section), which this guard admits. The two
//! must not be "tidied" into one predicate — they answer different questions.
//!
//! # Why agreeing ties are admitted
//!
//! When the two colliding policies' decisive item buckets are EQUAL, every
//! principal gets the same mask (or the same filter) whichever evaluator runs
//! first: `getMatchingPolicyItem` walks the same items and returns the same
//! `dataMaskInfo`/`rowFilterInfo`. Nothing about enforcement is undetermined,
//! so there is nothing to refuse — the redundancy costs one policy against the
//! profile total and no correctness.
//!
//! The evaluator still declines that case (`mask_filter.rs::
//! a_same_priority_mask_tie_is_undecidable_when_the_masks_are_identical`) and
//! is right to: a `MaskResult` names the policy that produced it, and it cannot
//! say which. That is an EXPLAINABILITY limit, not an enforcement one, and
//! turning it into a compile-time refusal would have broken the one real
//! artifact this programme has measured — F2's agreeing mask pair — for no
//! enforcement gain.
//!
//! Equality is over the decisive bucket as an ORDERED array, because item order
//! inside a policy is load-bearing (the compiler's own exempt idiom puts a
//! specific item AHEAD of the general `public` one so it is matched first —
//! `egret_perm_compile::rules`' module doc). Two policies carrying the same
//! items in a different order therefore count as differing: conservative, and
//! not reachable from the compiler, which builds both buckets in one place.
//!
//! # Every write path that can produce the pair
//!
//! Stated BY CODE PATH and not by authorship, because five such claims in this
//! programme have been wrong while the behaviour claim beside them was right.
//! The rule lives in ONE function, [`ambiguous_arbitrations_in`] — widened to
//! a profile plus its tag service by `crate::unit_overlap::unit_overlaps`,
//! which calls it — and every surface calls that rather than re-deriving it:
//!
//! * **The rule compiler.** `egret_perm_compile::trigger::compile_desired`
//!   checks the whole desired set of one profile before it is diffed, and
//!   returns `CompileError::AmbiguousMaskOrFilterArbitration`. That ONE call
//!   site covers four surfaces, because every one of them funnels through a
//!   recompile: `POST`/`PATCH /api/control-plane/perm/rules`, `POST
//!   /api/control-plane/perm/preview` (which runs the real write in a
//!   rolled-back transaction, so the steward is refused BEFORE Save), the
//!   grant/role/clearance writes, and the tag, role-membership and
//!   workspace-membership writes that recompile on their own triggers. Since
//!   SP4d-4 Task 3 it checks the desired set as one unit — the profile's
//!   download and its tag service's, through the profile's tag assignments —
//!   with the hand-written policies stored there, and refuses every pair with
//!   a compiled member that the compile CREATES (fix round 2, user ruling
//!   2026-10-12: a stored pair is reported, not refused). A migration switches
//!   its originals off before its rule writes, so the compiled twin is not
//!   paired with its own original. A tag assignment additionally
//!   refuses any pair it CREATES across the whole stored unit — hand-written,
//!   compiled, or one of each (`egret-bi`'s `catalog_tags`, through
//!   `egret_perm_compile::overlap`).
//! * **The policy editor.** `POST /api/control-plane/policies` with `kind:
//!   "masking"`/`"rowfilter"`/`"tag-masking"`/`"tag-rowfilter"`, and `PUT
//!   /policies/{id}` with the same four kinds (SP4d debt Task 16 gave the PUT
//!   its tag branch, through `guard_tag_policy_authoring`). Both
//!   compare the body being written against the other ENABLED policies of
//!   every evaluation unit it lands in (`crate::unit_overlap`, SP4d-4 Task 3),
//!   and refuse only a pair the write CREATES — pairs after the write minus
//!   pairs already stored, identified by both names, both decisive item
//!   arrays and the coordinate — so a write that narrows one stored pair is
//!   never refused because of another (SP4d-4 Task 2 review I2).
//! * **`PUT /policies/{id}/enabled` is NOT guarded, and that is a ruling.**
//!   Re-enabling a policy can complete the pair. The reason it is left alone is
//!   that it is also the only way OUT of a pair that already exists in an
//!   installation (disable one half, fix the model, re-enable), and the create
//!   and update paths already refuse the pair whether or not the body is
//!   enabled would be a different rule from the one above — this guard reads
//!   `isEnabled`, so a DISABLED policy is simply not part of any band. A pair
//!   that gets completed by re-enabling is therefore reachable. Between two
//!   hand-written policies it is NOT refused by any later write: the editor
//!   refuses only a pair its own write creates (and re-enabling through
//!   `PUT /policies/{id}` with `isEnabled: true` IS such a write, and is
//!   refused), and the rule compiler refuses only pairs with a compiled
//!   member that its compile creates — a re-enabled policy is already stored
//!   when the next compile reads its BEFORE, so that pair is reported, not
//!   refused (SP4d-4 Task 3 fix round 2). It is never silent: `explain` answers `EvalOrderAmbiguous` over the served
//!   artifact the moment it exists, and SP4d-4 Task 4's read-only
//!   `GET /perm/overlaps` is to report every stored pair.
//! * **Everything that is not a route** — `psql`, an import, a route added
//!   later — can still create the pair. `refuse_if_generated` and
//!   `unreadable_table_mask_for` have the same shape of reach and the same
//!   limit.
//!
//! # Why no `CHECK`, and no trigger
//!
//! The same three reasons `unreadable_table_mask`'s module doc records, plus
//! one of this pair's own:
//!
//! 1. **A single-row `CHECK` cannot express it.** The pair is two rows of
//!    `policy.authz_policies`, compared on four derived JSON values.
//! 2. **A trigger is not a constraint**: it would evaluate its lookup under its
//!    own snapshot, so two concurrent transactions each writing one half would
//!    both pass. That is the same residual window the Rust guard has, bought
//!    with no atomicity.
//! 3. **A trigger on `policy.authz_policies` would wedge the recompile that
//!    REMOVES a pair**, because `apply_diff_in_tx` writes that table on every
//!    recompile — fail-closed with no product remedy.
//! 4. **And it would make the AGREEING tie unrepresentable too**, since a
//!    `CHECK`/trigger keyed on the collision could not cheaply read the other
//!    row's items. The population that matters is the differing half; a schema
//!    guard would refuse both.
//!
//! Measured, so that the "it would wedge an installation" claim above is not
//! rhetorical. **As first swept, this host then had 8** databases (`template0`
//! and `template1` excluded), **7** carrying both `policy.authz_policies` and
//! `perm.rules` and one (`postgres`) carrying neither; across those 7 there
//! were **0** enabled mask or row-filter policies, **0** collision groups,
//! **0** differing groups and **0** `perm.rules` rows of either kind.
//! **Re-measured 2026-10-10: 10 databases, 9 carrying both, `postgres`
//! carrying neither, and the same four zeroes.** The population count drifts
//! because the test harness creates and drops per-migration-set template
//! databases between runs, so it is dated rather than stated in the present
//! tense; what the argument needs is the ZEROES, and those reproduce. The sweep's own
//! predicates were falsified against a fabricated six-row set holding one
//! agreeing pair, one differing pair and two controls (a different resource, a
//! different priority), where they reported `6 | 2 | 1` — so the zeros above
//! are a measurement and not a broken instrument. The only real artifact ever
//! measured to hold the shape is F2's, on a kind cluster that no longer exists.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::admin::item_key;

/// The decisive items a policy of `policy_type` is ARBITRATED on: its item
/// bucket, less a LEADING item that exempts exactly the lineage principal
/// (`egret_core::engine_principals::LINEAGE_PRINCIPAL`) with no transform.
///
/// **Why that item is not part of the arbitration (view governance R5).** The
/// compiler puts it first in every Trino mask and row filter it emits, so it
/// decides `egret-lineage` and nobody else, and identically in every compiled
/// body: two compiled policies cannot disagree through it. Comparing it would
/// make every pair stored before the item existed look "created" at the first
/// recompile after it, and every compiled policy look DIFFERENT from a
/// hand-written one that agreed with it, both of which refuse writes nobody
/// changed. The cost, stated: a hand-written mask that masks `egret-lineage`
/// beside a compiled one that does not is not reported for that principal
/// alone; a hand-written mask is outside what the compiler excepts anyway.
pub(crate) fn decisive_items(body: &Value, policy_type: i64) -> Value {
    let mut items = body
        .get(item_key(policy_type))
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(arr) = items.as_array_mut() {
        if arr.first().is_some_and(is_lineage_exemption) {
            arr.remove(0);
        }
    }
    items
}

/// An item naming only the lineage principal, with `MASK_NONE` (no value
/// expression) or an empty row filter, and no condition.
fn is_lineage_exemption(item: &Value) -> bool {
    let empty = |k: &str| {
        item.get(k)
            .is_none_or(|v| v.is_null() || v.as_array().is_some_and(Vec::is_empty))
    };
    let only_lineage = item
        .get("users")
        .and_then(Value::as_array)
        .is_some_and(|u| u.len() == 1 && u[0] == egret_core::engine_principals::LINEAGE_PRINCIPAL);
    let no_transform = match (item.get("dataMaskInfo"), item.get("rowFilterInfo")) {
        (Some(m), None) => {
            m.get("dataMaskType").and_then(Value::as_str) == Some("MASK_NONE")
                && m.get("valueExpr")
                    .is_none_or(|v| v.is_null() || v.as_str() == Some(""))
        }
        (None, Some(f)) => f
            .get("filterExpr")
            .is_none_or(|v| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty())),
        _ => false,
    };
    only_lineage && no_transform && empty("groups") && empty("roles") && empty("conditions")
}

/// Which of the two sub-decisions a colliding pair would arbitrate — the noun
/// the refusal uses, and the thing the steward has to go and edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arbitrated {
    /// `policyType 1`.
    Mask,
    /// `policyType 2`.
    RowFilter,
}

impl Arbitrated {
    /// The raw Ranger `policyType` code — the value
    /// `RangerPolicyRepository.init` files the evaluator lists by.
    pub(crate) fn policy_type(self) -> i64 {
        match self {
            Arbitrated::Mask => 1,
            Arbitrated::RowFilter => 2,
        }
    }

    pub(crate) fn from_policy_type(code: i64) -> Option<Self> {
        match code {
            1 => Some(Arbitrated::Mask),
            2 => Some(Arbitrated::RowFilter),
            _ => None,
        }
    }

    /// The noun a steward reads. "mask" and "row filter" are what the
    /// Governance UI calls a rule's two kinds.
    pub(crate) fn noun(self) -> &'static str {
        match self {
            Arbitrated::Mask => "mask",
            Arbitrated::RowFilter => "row filter",
        }
    }

    /// What the two policies disagree about, in the message.
    pub(crate) fn disagreement(self) -> &'static str {
        match self {
            Arbitrated::Mask => "different dataMaskPolicyItems",
            Arbitrated::RowFilter => "different rowFilterPolicyItems",
        }
    }
}

/// One policy a write would land in a service's downloaded policy set.
///
/// `service` is the id of the `policy.authz_services` row the policy is stored
/// under, **not** the profile being recompiled: a tag-selected rule's body is
/// filed under the profile's TAG service and is downloaded by a different
/// plugin, so it can never tie with a resource policy. Carrying the id here
/// rather than asking callers to group is deliberate — a caller that forgot
/// would compare across downloads.
#[derive(Debug, Clone, Copy)]
pub struct PolicyUnderWrite<'a> {
    pub service: i64,
    /// `policy.authz_policies.name`, which is also the body's `name` and the
    /// comparator's last tie-break.
    pub name: &'a str,
    /// The Ranger policy body as it will be served.
    pub body: &'a Value,
}

/// One pair of mask / row-filter policies whose enforced outcome the artifact
/// does not express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousArbitration {
    pub arbitrated: Arbitrated,
    /// The two policy names, lexicographically ordered so the refusal is the
    /// same sentence whichever order the caller iterated in.
    pub first: String,
    pub second: String,
    /// Where the two may both decide one request. The rendered INTERSECTION of
    /// the two resource maps where this module can compute it exactly (every
    /// level is named by one side only, is the same on both sides, or is a
    /// literal value set on both), else both maps' renderings joined by
    /// `" ∩ "` — so the sentence never names a coordinate narrower than what
    /// was compared.
    pub coordinate_hint: String,
    pub priority: i64,
    /// The `policy.authz_services` id the pair is served under (one download).
    pub service: i64,
    /// Which of the two Ranger serves TODAY wherever both match, when this
    /// module can compute it — see [`served_today`]. `None` means the order
    /// falls to something this module does not compute (or to the name).
    pub served_today: Option<String>,
    /// The decisive item arrays of `first` and `second`, as compared. Part of
    /// a pair's IDENTITY for the editor's "only a pair this write creates"
    /// rule (SP4d-4 Task 3): an edit that keeps the same partner but changes
    /// what either side decides is a different pair.
    pub first_items: Value,
    pub second_items: Value,
}

/// The remedy only the policy editor can offer: a hand-written policy can
/// author its priority (`isOverride` → `policyPriority 1`, `ranger_json`), a
/// compiled rule cannot.
pub(crate) const EDITOR_OVERRIDE_REMEDY: &str =
    ", or set Override on the one that should win (an override is evaluated first)";

/// The default rendering of a policy name in a refusal: the name, quoted.
pub(crate) fn quoted(name: &str) -> String {
    format!("{name:?}")
}

impl AmbiguousArbitration {
    /// **The refusal, and it is the same sentence from every write path** — one
    /// message, so the surfaces cannot describe the same state differently.
    ///
    /// It names both policies, where they may both decide a request, the
    /// priority, what they disagree about, and the remedy. It deliberately does
    /// NOT say the outcome is random: see this module's doc on what the
    /// comparator actually does. What it says is that the order the engine uses
    /// is not something either policy expresses, which is true under every
    /// reading of the jar and is the part a steward can fix.
    ///
    /// Asserted by substring rather than against a golden string: a golden copy
    /// of the sentence is a second place to edit it.
    pub fn refusal(&self) -> String {
        self.sentence_with(&quoted, "")
    }

    /// [`Self::refusal`] with the one remedy only the policy editor has —
    /// setting Override on the policy that should win.
    pub fn editor_refusal(&self) -> String {
        self.sentence_with(&quoted, EDITOR_OVERRIDE_REMEDY)
    }

    /// The one sentence, with each policy NAME rendered by `label` — the
    /// compiler names a compiled policy by its RULE (`rule "mask emails"`),
    /// the editor by the policy name it shows. `extra_remedy` is
    /// [`EDITOR_OVERRIDE_REMEDY`] or empty.
    pub(crate) fn sentence_with(
        &self,
        label: &dyn Fn(&str) -> String,
        extra_remedy: &str,
    ) -> String {
        let Self {
            arbitrated,
            first,
            second,
            coordinate_hint,
            priority,
            service: _,
            served_today,
            first_items: _,
            second_items: _,
        } = self;
        let (first, second) = (label(first), label(second));
        let noun = arbitrated.noun();
        let disagreement = arbitrated.disagreement();
        let today = match served_today {
            Some(winner) => format!(
                " Today Ranger serves {} wherever both match, because it has more \
                 match-any `*` levels with the same users, groups and accesses — a score, not \
                 something either policy says.",
                label(winner)
            ),
            None => String::new(),
        };
        format!(
            "two {noun} policies would be served at policyPriority {priority} that may both \
             decide one request ({coordinate_hint}) with {disagreement}: {first} and \
             {second}. Ranger arbitrates {noun}s with `>=`, so the FIRST evaluator in its band \
             is final, and within a band the order is `getEvalOrder()` (a wildcard-count score: \
             with the same users, groups and accesses the broader policy runs first and wins \
             wherever both match) and then the policy name, i.e. a rule's uuid — neither of \
             which says which of the two should win.{today} Narrow one of the two so they no \
             longer select a common resource, or delete one, or make them agree{extra_remedy}."
        )
    }
}

/// One resource level as compared: sorted, de-duplicated values, then
/// `isExcludes`, then `isRecursive`.
pub(crate) type Level = (Vec<String>, bool, bool);

/// The canonical form of a policy's `resources` map — what the comparison
/// reads and what the refusal renders.
///
/// Levels sorted by name; each level's values sorted and de-duplicated (Ranger
/// builds one matcher per value and matches if ANY matches); a level whose
/// `values` is empty or absent treated as ABSENT, because
/// `RangerAbstractResourceMatcher.init()` drops an empty policy value and a
/// level with no matchers is `isMatchAny`.
fn canonical_resources(body: &Value) -> BTreeMap<String, Level> {
    canonical_resource_map(body.get("resources"))
}

/// [`canonical_resources`] over a bare `resources` map (or an
/// `additionalResources` entry).
pub(crate) fn canonical_resource_map(resources: Option<&Value>) -> BTreeMap<String, Level> {
    let mut out = BTreeMap::new();
    let Some(obj) = resources.and_then(Value::as_object) else {
        return out;
    };
    for (level, spec) in obj {
        let mut values: Vec<String> = spec
            .get("values")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    // ⚠ An EMPTY-STRING value is dropped here, before the
                    // emptiness check, exactly as `RangerAbstractResourceMatcher
                    // .init()` drops it before building a matcher — reproduced
                    // by `egret_ranger_eval::resource::LevelMatcher::build`
                    // (`if value.is_empty() { continue }`, then `is_match_any:
                    // value_matchers.is_empty()`). So `values: [""]` is a
                    // MATCH-ANY level, not a literal `""` that meets nothing;
                    // reading it as a literal admitted a fail-open pair (SP4d-4
                    // Task 2 review C1).
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if values.is_empty() {
            continue;
        }
        values.sort();
        values.dedup();
        // Absent / `null` is `false` (Ranger's default). ANY other non-boolean
        // — `"true"`, `1`, `"false"` — is read as `true`: for `isExcludes` that
        // means "assume it intersects", the fail-closed answer, where reading
        // the string `"true"` as `false` took a complement for a literal
        // (review M5). `isRecursive` never narrows what a level meets; reading
        // it as `true` only makes the coordinate hint non-exact.
        let flag = |k: &str| match spec.get(k) {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => true,
        };
        out.insert(
            level.clone(),
            (values, flag("isExcludes"), flag("isRecursive")),
        );
    }
    out
}

/// Resources rendered for a human: `catalog=[wh] table=[report]`, with `!`
/// marking `isExcludes` and `/r` marking `isRecursive`.
///
/// Rendered from the canonical form rather than from the body, so the sentence
/// cannot name a resource the comparison did not use. Deliberately not
/// `admin::summarize_policy`'s `resource_path`: that one takes the FIRST value
/// of each of four hard-coded trino level names, which drops StarRocks'
/// `database` level and every value after the first — fine for a list view,
/// wrong for a refusal that has to be precise about what collided.
pub(crate) fn render_resources(key: &BTreeMap<String, Level>) -> String {
    if key.is_empty() {
        return "no resource levels".to_string();
    }
    key.iter()
        .map(|(level, (values, excludes, recursive))| {
            let bang = if *excludes { "!" } else { "" };
            let rec = if *recursive { "/r" } else { "" };
            format!("{level}{bang}=[{}]{rec}", values.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A value this module does not evaluate as a pattern: Ranger wildcards (`*`,
/// `?`), and the brace / escape characters of Ranger's token and request
/// expression syntax (`{USER}`, `${{…}}`, `\`), whose expansion is per request.
/// Any value containing one is ASSUMED to intersect, unless it is the one shape
/// [`trailing_star_prefix`] evaluates.
fn is_unevaluated(v: &str) -> bool {
    v.contains(['*', '?', '{', '\\'])
}

/// `Some(prefix)` when `v` is exactly `prefix*`: one trailing `*` and nothing
/// unevaluated before it. `"*"` itself is the empty prefix — the match-any.
fn trailing_star_prefix(v: &str) -> Option<&str> {
    let prefix = v.strip_suffix('*')?;
    (!is_unevaluated(prefix)).then_some(prefix)
}

/// The case folds a value is compared under — FULL Unicode, not ASCII (SP4d-4
/// Task 2 review I1: `ĐƠN_HÀNG` and `đơn_hàng` are one table to Ranger).
///
/// Every level of both service-defs egret serves is `ignoreCase: true`, and
/// Ranger compares those with `String.equalsIgnoreCase` / `IOCase.INSENSITIVE`,
/// which fold per UTF-16 unit as `Character.toLowerCase(Character.toUpperCase(c))`
/// — Java's SIMPLE (one char to one char) case mappings. `egret_ranger_eval`'s
/// own `fold` (`resource.rs`, private) is `str::to_lowercase()` and documents
/// the residue where the two differ (`resource.rs`, `chars_equal`'s doc: 104
/// ordered pairs in 21 classes, every one Java-true and Rust-false; adding an
/// uppercase step closes 96, and the 8 survivors are the six pairs involving
/// `İ` and `ϑ`/`ϴ`). A refusal must over-approximate BOTH, so two values are
/// taken to be equal when they agree under EITHER fold: the evaluator's
/// `to_lowercase()`, or [`java_fold_char`] per character.
fn folds(v: &str) -> [String; 2] {
    [v.to_lowercase(), v.chars().map(java_fold_char).collect()]
}

/// `Character.toLowerCase(Character.toUpperCase(c))`, one char to one char.
///
/// Rust exposes only the FULL case mappings, which can be longer than one code
/// point; Java's `Character` methods use the SIMPLE ones, which never are. The
/// two differ in exactly two shapes, and both are reproduced:
///
/// * a full UPPERCASE longer than one code point (`ß` → `SS`, `ŉ` → `ʼN`,
///   `ᾀ` → `ἈΙ`): Java's simple mapping either leaves the char alone or maps
///   it to a char whose lowercase is the original (`ᾀ` → `ᾈ` → `ᾀ`), so
///   keeping `c` reaches the same lowercase;
/// * a full LOWERCASE longer than one code point — only `İ` (U+0130), whose
///   full lowercase is `i` + U+0307 and whose simple lowercase is `i`. The
///   first code point IS the simple mapping. Taking the whole two-code-point
///   string instead (the fold before SP4d-4 Task 3) folded `İSTANBUL` to
///   `i̇stanbul`, so `("İSTANBUL", "istanbul")` — equal to Java — was admitted
///   as two tables: a fail-open.
fn java_fold_char(c: char) -> char {
    let mut up = c.to_uppercase();
    let upper = match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => c,
    };
    upper.to_lowercase().next().unwrap_or(upper)
}

/// Whether two single resource values may match one request value, under
/// [`folds`].
pub(crate) fn values_may_intersect(a: &str, b: &str) -> bool {
    let (fa, fb) = (folds(a), folds(b));
    fa.iter()
        .zip(fb.iter())
        .any(|(a, b)| folded_values_may_intersect(a, b))
}

fn folded_values_may_intersect(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (trailing_star_prefix(a), trailing_star_prefix(b)) {
        (Some(pa), Some(pb)) => pa.starts_with(pb) || pb.starts_with(pa),
        (Some(p), None) => is_unevaluated(b) || b.starts_with(p),
        (None, Some(p)) => is_unevaluated(a) || a.starts_with(p),
        (None, None) => is_unevaluated(a) || is_unevaluated(b),
    }
}

/// Whether two levels of the same name may both match one request value. A
/// level absent from either side matches anything; so, ASSUMED, does a level
/// carrying `isExcludes` (its complement is not evaluated here).
fn levels_may_intersect(a: Option<&Level>, b: Option<&Level>) -> bool {
    let (Some((av, aex, _)), Some((bv, bex, _))) = (a, b) else {
        return true;
    };
    if *aex || *bex {
        return true;
    }
    av.iter()
        .any(|x| bv.iter().any(|y| values_may_intersect(x, y)))
}

fn maps_may_intersect(a: &BTreeMap<String, Level>, b: &BTreeMap<String, Level>) -> bool {
    a.keys()
        .chain(b.keys())
        .all(|level| levels_may_intersect(a.get(level), b.get(level)))
}

/// **Whether two Ranger resource maps may select a common request
/// coordinate.** `a` and `b` are the `resources` objects themselves (not the
/// policy bodies).
///
/// Over-approximates; `true` is the fail-closed answer. For every level name in
/// the union of both maps, the two levels must may-intersect: a level that is
/// absent, empty or holding only empty strings (read as absent, as
/// [`canonical_resources`] and Ranger's `init()` read it) or
/// carries `isExcludes: true` may-intersect anything; otherwise some value pair
/// may-intersect — equal under Ranger's full-Unicode ignore-case comparison
/// ([`folds`]); or one is exactly `prefix*`
/// and the other starts with that prefix (two such: either prefix starts with
/// the other); or either value holds any other wildcard or token character,
/// which is ASSUMED to intersect.
pub fn resources_may_intersect(a: &Value, b: &Value) -> bool {
    maps_may_intersect(
        &canonical_resource_map(Some(a)),
        &canonical_resource_map(Some(b)),
    )
}

/// [`resources_may_intersect`] over every resource map of two policy bodies:
/// `resources`, then each `additionalResources` entry (Ranger matches a policy
/// if ANY of them matches). Neither surface emits `additionalResources` on a
/// mask or a row filter today; reading them is the fail-closed answer if one
/// ever does.
fn bodies_may_intersect(a: &Value, b: &Value) -> bool {
    fn maps(body: &Value) -> Vec<&Value> {
        let mut out = vec![body.get("resources").unwrap_or(&Value::Null)];
        if let Some(extra) = body.get("additionalResources").and_then(Value::as_array) {
            out.extend(extra);
        }
        out
    }
    let (am, bm) = (maps(a), maps(b));
    am.iter()
        .any(|x| bm.iter().any(|y| resources_may_intersect(x, y)))
}

/// The coordinate the refusal names: the exact intersection where every level
/// can be intersected exactly, else both renderings joined by `" ∩ "`.
fn coordinate_hint(a: &Value, b: &Value) -> String {
    let (ra, rb) = (canonical_resources(a), canonical_resources(b));
    let exact = |x: &BTreeMap<String, Level>, y: &BTreeMap<String, Level>| {
        let mut out = BTreeMap::new();
        for level in x.keys().chain(y.keys()) {
            let merged = match (x.get(level), y.get(level)) {
                (Some(l), None) | (None, Some(l)) => l.clone(),
                (Some(l), Some(r)) if l == r => l.clone(),
                (Some((lv, false, false)), Some((rv, false, false)))
                    if !lv.iter().chain(rv).any(|v| is_unevaluated(v)) =>
                {
                    let common: Vec<String> = lv
                        .iter()
                        .filter(|v| rv.iter().any(|w| values_may_intersect(v, w)))
                        .cloned()
                        .collect();
                    (common, false, false)
                }
                _ => return None,
            };
            out.insert(level.clone(), merged);
        }
        Some(out)
    };
    let single_maps = a
        .get("additionalResources")
        .is_none_or(|v| v.as_array().is_none_or(Vec::is_empty))
        && b.get("additionalResources")
            .is_none_or(|v| v.as_array().is_none_or(Vec::is_empty));
    match exact(&ra, &rb) {
        Some(key) if single_maps => render_resources(&key),
        _ => format!("{} ∩ {}", render_resources(&ra), render_resources(&rb)),
    }
}

/// The number of levels whose values carry the match-any `*`, when that is
/// the WHOLE of the policy's resource discount: one map, no `isExcludes`, and
/// every value either a literal or exactly `*`. `None` when any other wildcard
/// or token is present (it earns 10 or 5, or is expanded per request), which
/// this module does not reproduce.
fn match_any_levels(body: &Value) -> Option<u32> {
    if body
        .get("additionalResources")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        return None;
    }
    let mut n = 0;
    for (values, excludes, _) in canonical_resources(body).values() {
        if *excludes {
            return None;
        }
        if values.iter().any(|v| v == "*") {
            n += 1;
        } else if values.iter().any(|v| is_unevaluated(v)) {
            return None;
        }
    }
    Some(n)
}

/// The decisive bucket with each item's decision (`dataMaskInfo` /
/// `rowFilterInfo`) removed — what is left is the users, groups, roles,
/// accesses and conditions `computeEvalOrder()` discounts.
fn principal_shape(items: Option<&Value>) -> Option<Vec<Value>> {
    let items = items?.as_array()?;
    Some(
        items
            .iter()
            .map(|it| {
                let mut it = it.clone();
                if let Some(m) = it.as_object_mut() {
                    m.remove("dataMaskInfo");
                    m.remove("rowFilterInfo");
                }
                it
            })
            .collect(),
    )
}

/// **Which of a refused pair Ranger serves today, when it can be computed.**
///
/// `computeEvalOrder()` (both tags): `10000 − min(100, Σ resource discounts) −
/// principal − access − condition discounts`; a level holding the match-any `*`
/// earns 25; the comparator sorts ascending and the first matching mask/filter
/// is final (module doc, "Why different resources are refused too"; measured
/// in `docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md` §4). So when the
/// two carry the SAME users, groups, roles, accesses and conditions (their
/// buckets differ only in the decision) and every resource discount is a
/// match-any one, the policy with more `*` levels — the BROADER one — is
/// evaluated first and wins wherever both match. A level the policy does not
/// name earns nothing, so it does not count. Equal discounts fall to the name,
/// which is not reported as a winner.
fn served_today<'n>(
    (an, ab): (&'n str, &Value),
    (bn, bb): (&'n str, &Value),
    key: &str,
) -> Option<String> {
    let same_shape = principal_shape(ab.get(key))? == principal_shape(bb.get(key))?
        && ab.get("conditions") == bb.get("conditions");
    if !same_shape {
        return None;
    }
    let discount = |n: u32| (25 * n).min(100);
    let (da, db) = (
        discount(match_any_levels(ab)?),
        discount(match_any_levels(bb)?),
    );
    match da.cmp(&db) {
        std::cmp::Ordering::Greater => Some(an.to_string()),
        std::cmp::Ordering::Less => Some(bn.to_string()),
        std::cmp::Ordering::Equal => None,
    }
}

/// `RangerAbstractPolicyEvaluator.hasDeny()`: `policy.getIsDenyAllElse() ||
/// isNotEmpty(policy.getDenyPolicyItems())`. One spelling, the same one
/// `egret_ranger_eval::eval::PolicyView` reproduces.
pub(crate) fn has_deny(body: &Value) -> bool {
    let deny_all = body
        .get("isDenyAllElse")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let deny_items = body
        .get("denyPolicyItems")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    deny_all || deny_items
}

/// Absent or `null` means ENABLED — `RawRangerPolicy`'s `default_true` /
/// `null_as_true`.
pub(crate) fn is_enabled(body: &Value) -> bool {
    match body.get("isEnabled") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool().unwrap_or(true),
    }
}

/// Absent or `null` means 0 — `RawRangerPolicy`'s `null_as_default`.
pub(crate) fn int_field(body: &Value, field: &str) -> i64 {
    body.get(field).and_then(Value::as_i64).unwrap_or(0)
}

/// The band one policy lands in, within one download:
/// `(service, policy_type, has_deny, policy_priority)`.
type Band = (i64, i64, bool, i64);

/// **The whole rule.** Every pair in `policies` that would be served in one
/// band, may decide one request, and decides it differently — in a
/// deterministic order (by kind, then the pair's two names, then service and
/// priority). Empty = nothing to refuse.
///
/// Pure: no I/O, no clock, no ordering requirement on the input, so two
/// surfaces checking the same set report the same pairs in the same order.
///
/// Disabled policies, and policies of any `policyType` but 1 and 2, are not in
/// any band and are skipped — see this module's doc for both rulings.
pub fn ambiguous_arbitrations_in<'a>(
    policies: impl IntoIterator<Item = PolicyUnderWrite<'a>>,
) -> Vec<AmbiguousArbitration> {
    // (band → the name, body and decisive items of every policy in it).
    //
    // ⚠ The items are kept as a `Value` and compared with `==`, NOT rendered to
    // a string. `serde_json`'s `preserve_order` feature is ON in this workspace
    // (utoipa 5 turns it on — see the root `Cargo.toml`), so a `Map` is an
    // `IndexMap` and `to_string()` is INSERTION-ORDER dependent: a body that has
    // been through Postgres `jsonb` comes back with its keys in jsonb's own
    // order (by length, then bytes) while a freshly-serialized one carries the
    // struct's. `IndexMap`'s `PartialEq` is order-INdependent, so `Value == Value`
    // compares the two as maps and gets the right answer.
    //
    // This is not hypothetical: the editor route test
    // (`egret-policy/tests/admin_routes.rs::
    // a_second_row_filter_on_one_table_is_refused_only_when_it_disagrees`'s
    // agreeing control) caught the string version refusing two byte-equivalent
    // row filters, which would have made the guard refuse the harmless half of
    // SP4d-1's F2. Array order is still significant, which is what
    // `decisive_item_order_counts_as_a_difference` wants.
    let mut bands: BTreeMap<Band, Vec<(&'a str, &'a Value, Value)>> = BTreeMap::new();
    for p in policies {
        if !is_enabled(p.body) {
            continue;
        }
        let Some(arbitrated) = Arbitrated::from_policy_type(int_field(p.body, "policyType")) else {
            continue;
        };
        // The decisive bucket, named by the ONE function that maps a policy
        // type to its item key (`admin::item_key`) rather than by a second
        // copy of the two literals.
        let items = decisive_items(p.body, arbitrated.policy_type());
        bands
            .entry((
                p.service,
                arbitrated.policy_type(),
                has_deny(p.body),
                int_field(p.body, "policyPriority"),
            ))
            .or_default()
            .push((p.name, p.body, items));
    }

    let mut out = Vec::new();
    for ((service, policy_type, _, priority), members) in bands {
        // Every pair, not each against a representative: with resources that
        // only PARTLY overlap there is no representative — `a` and `c` can
        // intersect while neither intersects `b`.
        for (i, (an, ab, ai)) in members.iter().enumerate() {
            for (bn, bb, bi) in &members[i + 1..] {
                if ai == bi || !bodies_may_intersect(ab, bb) {
                    continue;
                }
                let ((first, fb, fi), (second, sb, si)) = if an <= bn {
                    ((an, ab, ai), (bn, bb, bi))
                } else {
                    ((bn, bb, bi), (an, ab, ai))
                };
                out.push(AmbiguousArbitration {
                    arbitrated: Arbitrated::from_policy_type(policy_type)
                        .expect("band keys are built from Arbitrated::policy_type"),
                    first: first.to_string(),
                    second: second.to_string(),
                    coordinate_hint: coordinate_hint(fb, sb),
                    priority,
                    service,
                    served_today: served_today((first, fb), (second, sb), item_key(policy_type)),
                    first_items: fi.clone(),
                    second_items: si.clone(),
                });
            }
        }
    }
    out.sort_by(|x, y| {
        (
            x.arbitrated.policy_type(),
            &x.first,
            &x.second,
            x.service,
            x.priority,
        )
            .cmp(&(
                y.arbitrated.policy_type(),
                &y.first,
                &y.second,
                y.service,
                y.priority,
            ))
    });
    out
}

/// One stored policy that an editor save's level fill moves this policy AHEAD
/// of within one Ranger band (SP4d-4 Task 11, the user's ordering ruling).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOrderShift {
    /// The other policy's name.
    pub other: String,
    /// Its `policy.authz_policies.id`, when the caller's body carries one.
    pub other_id: Option<i64>,
    /// Its Ranger `policyType` (0 access, 1 mask, 2 row filter).
    pub policy_type: i64,
    /// Where the two may both decide one request — the same rendering the
    /// overlap refusal names ([`coordinate_hint`]).
    pub coordinate_hint: String,
}

impl EvalOrderShift {
    /// The sentence a steward reads. It states the direction Ranger orders in
    /// — the BROADER policy (more `*` levels, other things equal) is evaluated
    /// first — and never claims the more specific one wins.
    pub fn sentence(&self, this: &str) -> String {
        let noun = match self.policy_type {
            1 => "mask",
            2 => "row filter",
            _ => "access policy",
        };
        format!(
            "Saving {this:?} with `*` in its emptied levels moves it AHEAD of the {noun} {other:?} \
             in Ranger's evaluation order: both are in the same priority band and may decide the \
             same request (where they meet: {coord}). Ranger evaluates the broader policy — the \
             one with more `*` levels, other things equal — first, not the more specific one, so \
             where the two differ {this:?} is now consulted before {other:?}. Check that the \
             result is what you intend; if it is not, narrow {this:?}, or set Override on the \
             policy that should be evaluated first.",
            other = self.other,
            coord = self.coordinate_hint,
        )
    }
}

/// `RangerPolicyRepository`'s access order matters for a pair only when one of
/// them can deny or carve an exception out: with allow items alone, any order
/// reaches the same union. A mask or a row filter is "first yielding evaluator
/// is final", so there it matters whenever the decisive items differ.
fn order_matters(a: &Value, b: &Value) -> bool {
    let policy_type = int_field(a, "policyType");
    match policy_type {
        0 => {
            let carves = |p: &Value| {
                has_deny(p)
                    || ["allowExceptions", "denyExceptions"].iter().any(|k| {
                        p.get(*k)
                            .and_then(Value::as_array)
                            .is_some_and(|x| !x.is_empty())
                    })
            };
            carves(a) || carves(b)
        }
        1 | 2 => {
            let key = item_key(policy_type);
            a.get(key) != b.get(key)
        }
        _ => false,
    }
}

/// **Does filling emptied levels with `*` move `filled` ahead of any policy in
/// `others`?** `unfilled` is the body the same form renders without the fill;
/// `filled` is the body that will be stored; `others` are the OTHER enabled
/// policies of the same download (the row a `PUT` replaces excluded by the
/// caller).
///
/// `computeEvalOrder()` gives each match-any `*` level a discount of 25,
/// capped at 100 in total, and an omitted level none; the comparator sorts
/// ascending after `(policyPriority, hasDeny)` (measured:
/// `docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md` §4, §7), so the
/// fill can only move the policy EARLIER. A pair is reported when
///
/// 1. it is in one band — same `policyType`, `hasDeny` and `policyPriority`;
/// 2. the two may select a common request ([`bodies_may_intersect`] — the
///    guard's own machinery, not a second spelling);
/// 3. their order matters ([`order_matters`]); and
/// 4. the fill carries `filled`'s resource discount from at or below the
///    other's to at or above it ([`match_any_levels`], the discount
///    [`served_today`] uses). A discount this module does not reproduce (any
///    wildcard but a bare `*`, an exclude, `additionalResources`) reads as
///    unknown and is reported — fail-closed.
///
/// Principal, access and condition discounts are not compared; the report is
/// therefore "may be consulted first", which is all the sentence claims.
pub fn eval_order_shifts<'a>(
    unfilled: &Value,
    filled: &Value,
    others: impl IntoIterator<Item = (&'a str, &'a Value)>,
) -> Vec<EvalOrderShift> {
    if !is_enabled(filled) {
        return Vec::new();
    }
    let discount = |n: u32| (25 * n).min(100);
    let (before, after) = (
        match_any_levels(unfilled).map(discount),
        match_any_levels(filled).map(discount),
    );
    if let (Some(b), Some(a)) = (before, after) {
        if a <= b {
            return Vec::new();
        }
    }
    let mut out: Vec<EvalOrderShift> = others
        .into_iter()
        .filter(|(_, o)| is_enabled(o))
        .filter(|(_, o)| {
            int_field(o, "policyType") == int_field(filled, "policyType")
                && has_deny(o) == has_deny(filled)
                && int_field(o, "policyPriority") == int_field(filled, "policyPriority")
        })
        .filter(|(_, o)| bodies_may_intersect(filled, o) && order_matters(filled, o))
        .filter(
            |(_, o)| match (before, after, match_any_levels(o).map(discount)) {
                (Some(b), Some(a), Some(other)) => b <= other && other <= a,
                _ => true,
            },
        )
        .map(|(name, o)| EvalOrderShift {
            other: name.to_string(),
            other_id: o.get("id").and_then(Value::as_i64),
            policy_type: int_field(o, "policyType"),
            coordinate_hint: coordinate_hint(filled, o),
        })
        .collect();
    out.sort_by(|x, y| x.other.cmp(&y.other));
    out
}

/// The first of [`ambiguous_arbitrations_in`], for write paths that report
/// one. `None` means the set would be served with no pair whose arbitration
/// the artifact does not express.
pub fn ambiguous_arbitration_in<'a>(
    policies: impl IntoIterator<Item = PolicyUnderWrite<'a>>,
) -> Option<AmbiguousArbitration> {
    ambiguous_arbitrations_in(policies).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mask(name: &str, column: &str, mask_type: &str) -> (String, Value) {
        (
            name.to_string(),
            json!({
                "name": name, "policyType": 1, "isEnabled": true, "policyPriority": 0,
                "resources": {
                    "catalog": {"values": ["wh"], "isExcludes": false, "isRecursive": false},
                    "schema":  {"values": ["sales"], "isExcludes": false, "isRecursive": false},
                    "table":   {"values": ["orders"], "isExcludes": false, "isRecursive": false},
                    "column":  {"values": [column], "isExcludes": false, "isRecursive": false},
                },
                "dataMaskPolicyItems": [{
                    "users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": mask_type},
                }],
                "policyItems": [], "denyPolicyItems": [],
            }),
        )
    }

    fn rowfilter(name: &str, expr: &str) -> (String, Value) {
        (
            name.to_string(),
            json!({
                "name": name, "policyType": 2, "isEnabled": true, "policyPriority": 0,
                "resources": {
                    "catalog": {"values": ["wh"], "isExcludes": false, "isRecursive": false},
                    "schema":  {"values": ["sales"], "isExcludes": false, "isRecursive": false},
                    "table":   {"values": ["orders"], "isExcludes": false, "isRecursive": false},
                },
                "rowFilterPolicyItems": [{
                    "users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "rowFilterInfo": {"filterExpr": expr},
                }],
                "policyItems": [], "denyPolicyItems": [],
            }),
        )
    }

    /// `p` with one resource level's values replaced.
    fn at(level: &str, value: &str, (name, mut body): (String, Value)) -> (String, Value) {
        body["resources"][level]["values"] = json!([value]);
        (name, body)
    }

    // ── SP4d-4 Task 11: the evaluation-order shift of a level fill ─────────

    fn access(name: &str, column: Option<&str>, deny: bool) -> Value {
        let mut res = serde_json::Map::new();
        for (l, v) in [("catalog", "wh"), ("schema", "sales"), ("table", "orders")] {
            res.insert(
                l.into(),
                json!({"values": [v], "isExcludes": false, "isRecursive": false}),
            );
        }
        if let Some(c) = column {
            res.insert(
                "column".into(),
                json!({"values": [c], "isExcludes": false, "isRecursive": false}),
            );
        }
        json!({
            "name": name, "policyType": 0, "isEnabled": true, "policyPriority": 0,
            "resources": res,
            "policyItems": [{"users": ["a"], "accesses": [{"type": "select", "isAllowed": true}]}],
            "denyPolicyItems": if deny {
                json!([{"users": ["b"], "accesses": [{"type": "select", "isAllowed": true}]}])
            } else { json!([]) },
        })
    }

    #[test]
    fn a_fill_that_passes_a_same_band_deny_policy_is_reported_and_its_direction_is_broader_first() {
        let (unfilled, filled) = (access("n", None, true), access("n", Some("*"), true));
        let other = access("o", Some("secret"), true);
        let shifts = eval_order_shifts(&unfilled, &filled, [("o", &other)]);
        assert_eq!(shifts.len(), 1);
        assert_eq!(shifts[0].other, "o");
        let sentence = shifts[0].sentence("n");
        assert!(
            sentence.contains("broader policy") && sentence.contains("not the more specific one")
        );
    }

    #[test]
    fn a_fill_is_not_reported_against_a_policy_it_does_not_pass() {
        let (unfilled, filled) = (access("n", None, true), access("n", Some("*"), true));
        // Already ahead: two `*` levels (50) beat the filled policy's one (25).
        let mut broader = access("o", Some("*"), true);
        broader["resources"]["table"]["values"] = json!(["*"]);
        // Allow-only pair: the filled policy has a deny, the other does not, so
        // they are different bands; and an allow-only pair is order-free.
        let allow = access("p", Some("secret"), false);
        let (u2, f2) = (access("n", None, false), access("n", Some("*"), false));
        // Another table: cannot decide the same request.
        let mut elsewhere = access("q", Some("secret"), true);
        elsewhere["resources"]["table"]["values"] = json!(["invoices"]);
        // Another priority band.
        let mut over = access("r", Some("secret"), true);
        over["policyPriority"] = json!(1);
        assert!(eval_order_shifts(
            &unfilled,
            &filled,
            [
                ("o", &broader),
                ("p", &allow),
                ("q", &elsewhere),
                ("r", &over)
            ]
        )
        .is_empty());
        assert!(eval_order_shifts(&u2, &f2, [("p", &allow)]).is_empty());
        // And a save that filled nothing moves nothing.
        assert!(eval_order_shifts(
            &filled,
            &filled,
            [("o", &access("o", Some("secret"), true))]
        )
        .is_empty());
    }

    #[test]
    fn a_mask_pair_is_reported_whatever_its_items_because_the_first_yielding_one_is_final() {
        let (_, mut stored) = mask("m1", "email", "MASK_NULL");
        stored["resources"]["table"]["values"] = json!(["orders"]);
        let (_, mut filled) = mask("m2", "*", "MASK_HASH");
        let mut unfilled = filled.clone();
        unfilled["resources"]
            .as_object_mut()
            .unwrap()
            .remove("column");
        filled["resources"]["column"]["values"] = json!(["*"]);
        let shifts = eval_order_shifts(&unfilled, &filled, [("m1", &stored)]);
        assert_eq!(shifts.len(), 1, "{shifts:?}");
        assert_eq!(shifts[0].policy_type, 1);
        // Agreeing items decide the same either way: nothing to report.
        let (_, same) = mask("m3", "email", "MASK_HASH");
        assert!(eval_order_shifts(&unfilled, &filled, [("m3", &same)]).is_empty());
    }

    fn check_all(set: &[(String, Value)]) -> Vec<AmbiguousArbitration> {
        ambiguous_arbitrations_in(set.iter().map(|(name, body)| PolicyUnderWrite {
            service: 1,
            name,
            body,
        }))
    }

    fn check(set: &[(String, Value)]) -> Option<AmbiguousArbitration> {
        ambiguous_arbitration_in(set.iter().map(|(name, body)| PolicyUnderWrite {
            service: 1,
            name,
            body,
        }))
    }

    /// F2's row-filter half, reconstructed from the live download: two policies
    /// selecting one table at one priority with DIFFERENT expressions.
    #[test]
    fn f2_s_differing_row_filter_pair_is_refused() {
        let set = [
            rowfilter("perm-rule-48fc5a0c", "country = 'US'"),
            rowfilter("perm-rule-570d7e80", "id > 2"),
        ];
        let got = check(&set).expect("the differing pair must be refused");
        assert_eq!(got.arbitrated, Arbitrated::RowFilter);
        assert_eq!(got.first, "perm-rule-48fc5a0c");
        assert_eq!(got.second, "perm-rule-570d7e80");
        assert_eq!(got.priority, 0);
        let msg = got.refusal();
        assert!(msg.contains("row filter"), "names the kind: {msg}");
        assert!(msg.contains("table=[orders]"), "names the resource: {msg}");
        assert!(
            msg.contains("\"perm-rule-48fc5a0c\"") && msg.contains("\"perm-rule-570d7e80\""),
            "names both policies: {msg}"
        );
        assert!(
            msg.contains("Narrow one of the two"),
            "names the remedy: {msg}"
        );
        // **The forbidden list must include the actual false alarm this branch
        // corrected, not only its neighbours.** "random" and "undefined" were
        // never the wording that shipped; "may change between queries" was, and
        // it is the one claim here that is affirmatively FALSE.
        // `PolicyEvalOrderComparator.compareNormal` is a TOTAL order — within a
        // band it falls through to the policy NAME, a rule's uuid, which is
        // stable for the life of the row — so the same two policies arbitrate
        // the same way on every query against one downloaded artifact. Telling a
        // steward their filter may vary between queries would be a false alarm
        // shipped to a user, and worse than the over-claim the other two words
        // guard: it describes a failure mode they cannot reproduce and would
        // send them looking for a nondeterminism that is not there.
        //
        // What IS true, and what the message says instead: which of the two
        // applies is decided by a tie-break the ARTIFACT does not express. It is
        // determinate and it is not authored — unpredictable to the steward, not
        // unstable at evaluation.
        for forbidden in ["random", "undefined", "may change between queries"] {
            assert!(
                !msg.to_lowercase().contains(forbidden),
                "must not over-claim nondeterminism — {forbidden:?} is a claim \
                 `compareNormal`'s total order falsifies: {msg}"
            );
        }
    }

    /// F2's MASK half, which AGREES — and is admitted. ⚠ This assertion is an
    /// accept-control, and it is the one that stops the guard being widened
    /// into something that would break the only real artifact ever measured to
    /// hold the shape (SP4d-1 §8: policies 8777 and 8801, both
    /// `dataMaskType: "MASK"`, no `valueExpr`).
    #[test]
    fn f2_s_agreeing_mask_pair_is_admitted() {
        let set = [
            mask("perm-rule-47ab3a6a", "item", "MASK"),
            mask("perm-rule-d48f048a", "item", "MASK"),
        ];
        assert_eq!(check(&set), None, "an agreeing tie is not undetermined");
    }

    fn with_lineage_first((name, mut body): (String, Value), mask_type: &str) -> (String, Value) {
        let lineage = json!({
            "users": ["egret-lineage"], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": mask_type},
        });
        body["dataMaskPolicyItems"]
            .as_array_mut()
            .unwrap()
            .insert(0, lineage);
        (name, body)
    }

    /// View governance R5: the compiler's leading `egret-lineage` `MASK_NONE`
    /// item decides that principal alone, so it does not make an agreeing
    /// pair differ (a compiled mask beside a hand-written twin, or beside one
    /// compiled before the item existed).
    #[test]
    fn the_lineage_exemption_does_not_make_an_agreeing_pair_differ() {
        let set = [
            with_lineage_first(mask("perm-rule-47ab3a6a", "item", "MASK"), "MASK_NONE"),
            mask("hand-written", "item", "MASK"),
        ];
        assert_eq!(check(&set), None);
    }

    /// A leading item for `egret-lineage` that TRANSFORMS is a real decision
    /// and stays in the comparison.
    #[test]
    fn a_lineage_item_that_masks_is_still_compared() {
        let set = [
            with_lineage_first(mask("perm-rule-47ab3a6a", "item", "MASK"), "MASK_NULL"),
            mask("hand-written", "item", "MASK"),
        ];
        assert!(check(&set).is_some());
    }

    /// The same two masks with different mask TYPES: refused.
    #[test]
    fn a_differing_mask_pair_on_one_column_is_refused() {
        let set = [
            mask("a-mask", "item", "MASK"),
            mask("b-mask", "item", "MASK_NULL"),
        ];
        let got = check(&set).expect("differing mask types must be refused");
        assert_eq!(got.arbitrated, Arbitrated::Mask);
        assert!(got.refusal().contains("column=[item]"));
    }

    /// Three in one band where the first two AGREE and the third differs. The
    /// obvious implementation — compare consecutive pairs — passes this only by
    /// accident; comparing each against the band's representative is what makes
    /// it hold. (Members are sorted first, so `a`/`b` agree and `c` differs.)
    #[test]
    fn a_third_member_that_differs_is_found_even_when_the_first_two_agree() {
        let set = [
            mask("a-mask", "item", "MASK"),
            mask("b-mask", "item", "MASK"),
            mask("c-mask", "item", "MASK_NULL"),
        ];
        let got = check(&set).expect("the odd member must be found");
        assert_eq!(got.first, "a-mask");
        assert_eq!(got.second, "c-mask");
    }

    /// Four accept-controls, each of which the guard must NOT refuse because
    /// Ranger orders the two deterministically (or they never meet at all).
    #[test]
    fn policies_ranger_orders_deterministically_are_admitted() {
        // 1. Two literal columns that differ: no request coordinate is
        //    selected by both, so the two never meet in one evaluation.
        assert_eq!(
            check(&[
                mask("a-mask", "item", "MASK"),
                mask("b-mask", "price", "MASK_NULL"),
            ]),
            None,
            "a different column is a different resource"
        );

        // 2. Different priority: the comparator's FIRST key separates them.
        let (n, mut high) = mask("b-mask", "item", "MASK_NULL");
        high["policyPriority"] = json!(1);
        assert_eq!(
            check(&[mask("a-mask", "item", "MASK"), (n, high)]),
            None,
            "a priority band is ordered against another band"
        );

        // 3. A mask and a row filter never tie: `RangerPolicyRepository.init`
        //    files them into disjoint evaluator lists.
        let (mn, mb) = mask("a", "item", "MASK");
        let (rn, rb) = rowfilter("b", "id > 2");
        assert_eq!(check(&[(mn, mb), (rn, rb)]), None, "disjoint policy types");

        // 4. A DISABLED half is in no band at all.
        let (n, mut off) = mask("b-mask", "item", "MASK_NULL");
        off["isEnabled"] = json!(false);
        assert_eq!(
            check(&[mask("a-mask", "item", "MASK"), (n, off)]),
            None,
            "a disabled policy is not downloaded as enabled"
        );
    }

    /// `has_deny` is in the band key, so a deny-carrying policy is ORDERED
    /// against one without rather than tied with it — `compareNormal` puts
    /// `hasDeny()` first at equal priority.
    #[test]
    fn a_deny_carrying_half_is_in_its_own_band() {
        let (n, mut with_deny) = mask("b-mask", "item", "MASK_NULL");
        with_deny["denyPolicyItems"] = json!([{"users": ["u"], "accesses": []}]);
        assert_eq!(
            check(&[mask("a-mask", "item", "MASK"), (n, with_deny)]),
            None,
            "hasDeny() separates the two bands"
        );
    }

    /// Two policies under DIFFERENT services are two different downloads and
    /// can never tie — the reason [`PolicyUnderWrite`] carries the service id
    /// instead of asking callers to group.
    #[test]
    fn two_services_are_two_downloads() {
        let (an, ab) = mask("a-mask", "item", "MASK");
        let (bn, bb) = mask("b-mask", "item", "MASK_NULL");
        assert_eq!(
            ambiguous_arbitration_in([
                PolicyUnderWrite {
                    service: 1,
                    name: &an,
                    body: &ab
                },
                PolicyUnderWrite {
                    service: 2,
                    name: &bn,
                    body: &bb
                },
            ]),
            None
        );
    }

    /// The answer does not depend on the order the caller iterated in — both
    /// which pair is reported and which of the two is `first`.
    #[test]
    fn the_reported_pair_does_not_depend_on_input_order() {
        let forward = [
            mask("a-mask", "item", "MASK"),
            mask("b-mask", "item", "MASK_NULL"),
        ];
        let mut backward = forward.clone();
        backward.reverse();
        assert_eq!(check(&forward), check(&backward));
        assert_eq!(check(&forward).expect("refused").first, "a-mask");
    }

    /// The serde defaults are keyed to `RawRangerPolicy`'s, so a body that
    /// omits `isEnabled` and `policyPriority` lands in the same band as one
    /// that spells them out as `true` and `0` — and a `null` does too.
    #[test]
    fn absent_and_null_enabled_and_priority_read_as_the_evaluator_reads_them() {
        let (an, mut absent) = mask("a-mask", "item", "MASK");
        let obj = absent.as_object_mut().expect("object");
        obj.remove("isEnabled");
        obj.remove("policyPriority");
        let (bn, mut nulls) = mask("b-mask", "item", "MASK_NULL");
        nulls["isEnabled"] = Value::Null;
        nulls["policyPriority"] = Value::Null;
        let got = check(&[(an, absent), (bn, nulls)]).expect(
            "absent/null isEnabled must read as enabled and absent/null priority as 0, \
             exactly as RawRangerPolicy deserializes them",
        );
        assert_eq!(got.priority, 0);
    }

    /// A level whose `values` is empty is treated as ABSENT, because
    /// `RangerAbstractResourceMatcher.init()` drops an empty policy value and a
    /// level with no matchers is `isMatchAny`. Fail-CLOSED: the two collide.
    #[test]
    fn an_empty_resource_level_is_the_same_band_as_an_absent_one() {
        let (an, mut empty) = mask("a-mask", "item", "MASK");
        empty["resources"]["column"] = json!({"values": []});
        let (bn, mut absent) = mask("b-mask", "item", "MASK_NULL");
        absent["resources"]
            .as_object_mut()
            .expect("object")
            .remove("column");
        assert!(
            check(&[(an, empty), (bn, absent)]).is_some(),
            "an empty level and an absent one are one band"
        );
    }

    /// Value ORDER inside a level cannot matter (Ranger builds one matcher per
    /// value and matches if any matches), so two policies listing the same two
    /// columns in opposite orders are one band.
    #[test]
    fn resource_value_order_does_not_split_a_band() {
        let (an, mut a) = mask("a-mask", "item", "MASK");
        a["resources"]["column"] = json!({"values": ["item", "price"]});
        let (bn, mut b) = mask("b-mask", "item", "MASK_NULL");
        b["resources"]["column"] = json!({"values": ["price", "item"]});
        assert!(
            check(&[(an, a), (bn, b)]).is_some(),
            "value order is not part of what Ranger matches"
        );
    }

    /// ⚠ **Changed in SP4d-4 Task 2, and the one existing test that was.** This
    /// was `excludes_and_recursive_split_a_band`, asserting `None`: under the
    /// old EQUAL-resources key a flag difference made two keys, so the pair was
    /// never compared. Under "may decide the same request" neither flag
    /// separates them — `isExcludes` is assumed to intersect (its complement is
    /// not evaluated here), and `isRecursive` on a literal value still selects
    /// that value — so the same fixtures are now a refusal.
    #[test]
    fn excludes_and_recursive_do_not_separate_two_policies_on_one_column() {
        for flag in ["isExcludes", "isRecursive"] {
            let (an, a) = mask("a-mask", "item", "MASK");
            let (bn, mut b) = mask("b-mask", "item", "MASK_NULL");
            b["resources"]["column"][flag] = json!(true);
            let got = check(&[(an, a), (bn, b)])
                .unwrap_or_else(|| panic!("{flag} must not hide a pair that may intersect"));
            // Not an exact intersection, so the hint names both sides.
            assert!(
                got.coordinate_hint.contains(" ∩ "),
                "{flag}: {}",
                got.coordinate_hint
            );
        }
    }

    /// Item ORDER inside the decisive bucket is load-bearing (the compiler's
    /// exempt idiom relies on a specific item being matched before the general
    /// `public` one), so two policies with the same items in opposite orders
    /// count as differing.
    #[test]
    fn decisive_item_order_counts_as_a_difference() {
        let (an, mut a) = mask("a-mask", "item", "MASK");
        let exempt = json!({
            "users": ["vip"], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NONE"},
        });
        let general = a["dataMaskPolicyItems"][0].clone();
        a["dataMaskPolicyItems"] = json!([exempt.clone(), general.clone()]);
        let (bn, mut b) = mask("b-mask", "item", "MASK");
        b["dataMaskPolicyItems"] = json!([general, exempt]);
        assert!(
            check(&[(an, a), (bn, b)]).is_some(),
            "the first matching item wins, so order is part of the decision"
        );
    }

    /// ⚠ **The defect the editor route test caught, pinned where it was fixed.**
    /// `serde_json`'s `preserve_order` is on in this workspace, so a `Map` is an
    /// `IndexMap` and `Value::to_string()` follows INSERTION order — a body that
    /// has been through Postgres `jsonb` comes back with jsonb's key order, a
    /// freshly-built one with the struct's. Comparing the decisive buckets as
    /// STRINGS therefore called two byte-equivalent row filters "differing" and
    /// refused the harmless half of SP4d-1's F2.
    ///
    /// The fixture reorders the keys explicitly rather than round-tripping
    /// through a database, so the assertion runs with no DATABASE_URL and
    /// cannot be weakened by a DB-gated skip. It is a genuine reordering: the
    /// two bodies' `to_string()`s are asserted DIFFERENT, so the test would
    /// still fail if a future change went back to comparing strings.
    #[test]
    fn two_bodies_whose_item_keys_are_in_different_orders_agree() {
        let (an, a) = rowfilter("a-filter", "country = 'US'");
        let (bn, mut b) = rowfilter("b-filter", "country = 'US'");
        // Rebuild b's single item with its keys in a different order — what a
        // `jsonb` round-trip does.
        let item = &a["rowFilterPolicyItems"][0];
        let mut reordered = serde_json::Map::new();
        for k in ["rowFilterInfo", "accesses", "roles", "groups", "users"] {
            reordered.insert(k.to_string(), item[k].clone());
        }
        b["rowFilterPolicyItems"] = json!([Value::Object(reordered)]);
        assert_ne!(
            a["rowFilterPolicyItems"].to_string(),
            b["rowFilterPolicyItems"].to_string(),
            "the fixture must actually reorder the keys, or this test proves nothing"
        );
        assert_eq!(
            check(&[(an, a), (bn, b)]),
            None,
            "key order is not a disagreement — two equal filters must not be refused"
        );
    }

    /// A single policy is never a pair, and an empty set is never a pair.
    #[test]
    fn one_policy_and_no_policy_are_not_pairs() {
        assert_eq!(check(&[]), None);
        assert_eq!(check(&[mask("only", "item", "MASK")]), None);
    }

    // -----------------------------------------------------------------------
    // SP4d-4 Task 2: "may decide the same request", not "equal resources".
    // -----------------------------------------------------------------------

    /// **The case the old equality key admitted.** A table-wildcard mask and a
    /// named-table mask on one column, at one priority, with different mask
    /// types. Task 1 measured (both jars, both service-defs) that the BROADER
    /// one is served on `orders.email` — `getEvalOrder()` sorts the policy with
    /// more match-any `*` values first and the first matching mask is final —
    /// which nothing in the two policies says.
    #[test]
    fn a_wildcard_mask_and_a_named_mask_on_one_column_are_refused() {
        let set = [
            at("table", "*", mask("m-broad", "email", "MASK")),
            mask("m-narrow", "email", "MASK_NULL"),
        ];
        let got = check(&set).expect("a wildcard and a name on one column may meet");
        assert_eq!(got.arbitrated, Arbitrated::Mask);
        assert_eq!(
            (got.first.as_str(), got.second.as_str()),
            ("m-broad", "m-narrow")
        );
        let msg = got.refusal();
        assert!(
            msg.contains("Narrow one of the two"),
            "names the remedy: {msg}"
        );
        assert!(
            !msg.to_lowercase().contains("more specific"),
            "Task 1 measured the BROADER policy winning; the sentence must not say \
             otherwise: {msg}"
        );
    }

    /// A `prefix*` intersects exactly the names that start with the prefix,
    /// case-folded (every level of both service-defs is `ignoreCase: true`).
    #[test]
    fn a_prefix_glob_and_a_disjoint_name_are_admitted() {
        let disjoint = [
            at("table", "ord*", mask("a-mask", "email", "MASK")),
            at("table", "items", mask("b-mask", "email", "MASK_NULL")),
        ];
        assert_eq!(check(&disjoint), None, "`ord*` cannot select `items`");

        let case_folded = [
            at("table", "ord*", mask("a-mask", "email", "MASK")),
            at("table", "ORDERS", mask("b-mask", "email", "MASK_NULL")),
        ];
        assert!(
            check(&case_folded).is_some(),
            "`ord*` selects `ORDERS` under ignoreCase"
        );
    }

    /// Accept-control 1 of `policies_ranger_orders_deterministically_are_admitted`,
    /// restated under the new rule: two literal columns never meet.
    #[test]
    fn two_different_columns_still_do_not_overlap() {
        assert_eq!(
            check(&[
                mask("a-mask", "item", "MASK"),
                mask("b-mask", "price", "MASK_NULL"),
            ]),
            None
        );
    }

    /// **Exemptions are part of the decision.** Both policies mask the column
    /// with `MASK` for `public`, but A carries the compiler's exempt idiom — a
    /// `MASK_NONE` item for `vip` AHEAD of the `public` one — and B does not.
    /// For `vip`, A answers `MASK_NONE` and B answers `MASK`, and which of the
    /// two `vip` gets is the tie-break. o5 (decisive arrays differ) catches it;
    /// this test exists so a "compare only the mask type" simplification
    /// cannot pass.
    #[test]
    fn different_exemptions_with_the_same_mask_are_refused() {
        let (an, mut a) = mask("a-mask", "item", "MASK");
        let general = a["dataMaskPolicyItems"][0].clone();
        let exempt = json!({
            "users": ["vip"], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NONE"},
        });
        a["dataMaskPolicyItems"] = json!([exempt, general]);
        let b = mask("b-mask", "item", "MASK");
        assert!(check(&[(an, a), b]).is_some());
    }

    /// **Identical buckets are admitted even when the resources differ** — the
    /// agreeing tie of F2, generalised: whichever of the two runs first, every
    /// principal gets the same mask.
    #[test]
    fn identical_buckets_on_intersecting_resources_are_admitted() {
        let set = [
            at("table", "*", mask("a-mask", "email", "MASK")),
            mask("b-mask", "email", "MASK"),
        ];
        assert_eq!(check(&set), None, "an agreeing pair is not undetermined");
    }

    /// `isExcludes` selects the COMPLEMENT of its values, which this module does
    /// not evaluate — assumed to intersect, the fail-closed answer. Under the
    /// old key `{table!=[orders]}` and `{table=[orders]}` were simply two keys.
    #[test]
    fn an_excluded_level_is_assumed_to_intersect() {
        let (an, mut a) = at("table", "orders", mask("a-mask", "email", "MASK"));
        a["resources"]["table"]["isExcludes"] = json!(true);
        let b = at("table", "items", mask("b-mask", "email", "MASK_NULL"));
        let got = check(&[(an, a), b]).expect("an excluded level may meet anything");
        assert_eq!(got.arbitrated, Arbitrated::Mask);
    }

    /// Every pair, in one order whatever the caller's: three masks on one
    /// column, pairwise different.
    #[test]
    fn every_pair_is_returned_in_a_stable_order() {
        let forward = [
            mask("a-mask", "item", "MASK"),
            mask("b-mask", "item", "MASK_NULL"),
            mask("c-mask", "item", "MASK_HASH"),
        ];
        let mut backward = forward.clone();
        backward.reverse();
        let names = |v: Vec<AmbiguousArbitration>| {
            v.into_iter()
                .map(|p| (p.first, p.second))
                .collect::<Vec<_>>()
        };
        let expected = vec![
            ("a-mask".to_string(), "b-mask".to_string()),
            ("a-mask".to_string(), "c-mask".to_string()),
            ("b-mask".to_string(), "c-mask".to_string()),
        ];
        assert_eq!(names(check_all(&forward)), expected);
        assert_eq!(names(check_all(&backward)), expected);
    }

    /// The editor's sentence is the shared one plus the remedy only a
    /// hand-written policy has.
    #[test]
    fn the_editor_refusal_adds_override_as_a_remedy() {
        let got = check(&[
            mask("a-mask", "item", "MASK"),
            mask("b-mask", "item", "MASK_NULL"),
        ])
        .expect("refused");
        assert!(!got.refusal().contains("Override"));
        let editor = got.editor_refusal();
        assert!(editor.starts_with(got.refusal().trim_end_matches('.')));
        assert!(
            editor.contains("or set Override on the one that should win"),
            "{editor}"
        );
    }

    /// Value-level may-intersect, the cases the route-level tests above do not
    /// spell out one by one.
    #[test]
    fn value_level_may_intersect_cases() {
        let lvl = |v: &str| json!({"t": {"values": [v]}});
        for (a, b, want) in [
            ("orders", "orders", true),
            ("orders", "ORDERS", true),
            ("orders", "items", false),
            ("*", "anything", true),
            ("ord*", "or*", true),
            ("ord*", "ix*", false),
            ("ord*", "x?z", true),
            ("o*s", "items", true),
            ("{USER}", "alice", true),
            // Review C1: Ranger drops an empty value, so `[""]` is match-any.
            ("", "items", true),
            // Review I1: full Unicode folding, as Ranger's ignoreCase does.
            ("ĐƠN_HÀNG", "đơn_hàng", true),
            ("Đ*", "đơn_hàng", true),
            ("ĐƠN_HÀNG", "hóa_đơn", false),
            // Java's per-char `toLowerCase(toUpperCase(c))` equates these;
            // `to_lowercase()` alone does not.
            ("ϑ", "ϴ", true),
            // SP4d-4 Task 3 (N1): `İ` folds to ONE `i` under Java's simple
            // mappings — the evaluator's residue lists six ordered pairs
            // involving it (`İ` with `i`, `I` and `ı`).
            ("İSTANBUL", "istanbul", true),
            ("İ", "i", true),
            ("İ", "I", true),
            ("İ", "ı", true),
            // …and the uppercase-step class it lists beside them.
            ("ς", "σ", true),
            ("ſ", "s", true),
            ("µ", "μ", true),
            ("ß", "ss", false),
        ] {
            assert_eq!(
                resources_may_intersect(&lvl(a), &lvl(b)),
                want,
                "{a:?} vs {b:?}"
            );
            assert_eq!(
                resources_may_intersect(&lvl(b), &lvl(a)),
                want,
                "symmetric: {b:?} vs {a:?}"
            );
        }
        // A level named by one side only, or an empty one, matches anything.
        assert!(resources_may_intersect(&lvl("orders"), &json!({})));
        assert!(resources_may_intersect(
            &lvl("orders"),
            &json!({"t": {"values": []}})
        ));
        // Every level must may-intersect.
        assert!(!resources_may_intersect(
            &json!({"s": {"values": ["sales"]}, "t": {"values": ["orders"]}}),
            &json!({"s": {"values": ["sales"]}, "t": {"values": ["items"]}}),
        ));
    }

    /// The hint is the exact intersection where it can be computed, and both
    /// sides otherwise.
    #[test]
    fn the_coordinate_hint_is_exact_only_where_it_can_be() {
        let exact = check(&[
            at("table", "ORDERS", mask("a-mask", "email", "MASK")),
            mask("b-mask", "email", "MASK_NULL"),
        ])
        .expect("refused");
        assert_eq!(
            exact.coordinate_hint,
            "catalog=[wh] column=[email] schema=[sales] table=[ORDERS]"
        );
        let both = check(&[
            at("table", "*", mask("a-mask", "email", "MASK")),
            mask("b-mask", "email", "MASK_NULL"),
        ])
        .expect("refused");
        assert!(
            both.coordinate_hint.contains("table=[*]")
                && both.coordinate_hint.contains(" ∩ ")
                && both.coordinate_hint.contains("table=[orders]"),
            "{}",
            both.coordinate_hint
        );
    }

    // -----------------------------------------------------------------------
    // SP4d-4 Task 2, fix round 1.
    // -----------------------------------------------------------------------

    /// Review C1: `values: [""]` is a MATCH-ANY level to Ranger (`init()` drops
    /// the empty value; `LevelMatcher::build` reproduces it), so it decides
    /// `orders.email` just as `orders` does.
    #[test]
    fn a_level_of_only_empty_strings_is_match_any() {
        let set = [
            at("table", "", mask("a-mask", "email", "MASK")),
            mask("b-mask", "email", "MASK_NULL"),
        ];
        assert!(check(&set).is_some(), "`[\"\"]` must not read as a literal");
        // …and an empty string BESIDE a real value is just dropped.
        let (an, mut a) = mask("a-mask", "email", "MASK");
        a["resources"]["table"]["values"] = json!(["orders", ""]);
        let b = at("table", "items", mask("b-mask", "email", "MASK_NULL"));
        assert_eq!(
            check(&[(an, a), b]),
            None,
            "`[orders, \"\"]` is `{{orders}}`"
        );
    }

    /// Review I1: a Vietnamese table name in two cases is one table to Ranger.
    #[test]
    fn vietnamese_names_fold_as_ranger_folds_them() {
        let set = [
            at("table", "ĐƠN_HÀNG", mask("a-mask", "email", "MASK")),
            at("table", "đơn_hàng", mask("b-mask", "email", "MASK_NULL")),
        ];
        let got = check(&set).expect("ĐƠN_HÀNG and đơn_hàng are one table");
        assert!(
            got.coordinate_hint.contains("table=[ĐƠN_HÀNG]"),
            "the exact intersection names it: {}",
            got.coordinate_hint
        );
    }

    /// Review M5: a NON-boolean `isExcludes` (the string `"true"`) is read as
    /// excluding — assumed to intersect — not as `false`.
    #[test]
    fn a_non_boolean_is_excludes_is_assumed_to_intersect() {
        for flag in [json!("true"), json!("false"), json!(1)] {
            let (an, mut a) = at("table", "orders", mask("a-mask", "email", "MASK"));
            a["resources"]["table"]["isExcludes"] = flag.clone();
            let b = at("table", "items", mask("b-mask", "email", "MASK_NULL"));
            assert!(
                check(&[(an, a), b]).is_some(),
                "isExcludes = {flag} must fail closed"
            );
        }
    }

    /// Review M1: when the two carry the same users, groups and accesses and
    /// every resource discount is a match-any one, the sentence names the
    /// policy Ranger serves today — the BROADER one (Task 1 pin 1) — and
    /// otherwise names none.
    #[test]
    fn the_refusal_names_the_policy_served_today_when_it_can() {
        let got = check(&[
            mask("m-narrow", "email", "MASK_NULL"),
            at("table", "*", mask("m-broad", "email", "MASK")),
        ])
        .expect("refused");
        assert_eq!(got.served_today.as_deref(), Some("m-broad"));
        let msg = got.refusal();
        assert!(
            msg.contains("Today Ranger serves \"m-broad\"")
                && msg.contains("same users, groups and accesses"),
            "{msg}"
        );

        // Same resources: the order falls to the name — no winner named.
        let tie = check(&[
            mask("a-mask", "item", "MASK"),
            mask("b-mask", "item", "MASK_NULL"),
        ])
        .expect("refused");
        assert_eq!(tie.served_today, None);
        assert!(!tie.refusal().contains("Today Ranger serves"));

        // Different principals (an exempt item): the discounts may differ —
        // no winner named.
        let (an, mut a) = at("table", "*", mask("m-broad", "email", "MASK"));
        let general = a["dataMaskPolicyItems"][0].clone();
        a["dataMaskPolicyItems"] = json!([{
            "users": ["vip"], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NONE"},
        }, general]);
        let shaped = check(&[(an, a), mask("m-narrow", "email", "MASK_NULL")]).expect("refused");
        assert_eq!(shaped.served_today, None);

        // A non-match-any wildcard earns a discount this module does not
        // reproduce — no winner named.
        let prefix = check(&[
            at("table", "ord*", mask("m-prefix", "email", "MASK")),
            mask("m-narrow", "email", "MASK_NULL"),
        ])
        .expect("refused");
        assert_eq!(prefix.served_today, None);
    }
}
