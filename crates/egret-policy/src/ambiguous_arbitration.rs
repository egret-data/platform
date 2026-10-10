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
//! resource-specificity score, so two policies selecting the SAME resource
//! score the same, and the decision falls through to the policy NAME — which
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
//! Two policies collide when they land in the SAME evaluator band for the SAME
//! request, which `egret_ranger_eval` spells as `(policy_priority, has_deny)`
//! within one `policyType` list. This module's key is
//! `(service, policy_type, has_deny, policy_priority, resources)`:
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
//! * `resources` is canonicalised by [`canonical_resources`]: levels sorted by
//!   name, values sorted and de-duplicated (Ranger builds one matcher per value
//!   and matches if ANY matches, so their order cannot matter), and a level
//!   whose `values` is empty treated as ABSENT, because
//!   `RangerAbstractResourceMatcher.init()` drops an empty policy value before
//!   building a matcher for it and a level with no matchers is `isMatchAny` —
//!   the same reading `CompileError::EmptyScopeLevel` rests on. Treating the
//!   two as equal is the fail-CLOSED direction (it finds more collisions);
//!   nothing this compiler emits can reach it anyway, since
//!   `ranger_json::resources_object` drops empty levels and migration `0178`
//!   forbids an empty scope level upstream of it.
//!
//! # Why this is NARROWER than the evaluator's refusal, deliberately
//!
//! `EvalOrderAmbiguous` is raised per REQUEST: the evaluator refuses whenever
//! two policies in one band both DECIDE the request in front of it, whether or
//! not their resource maps are equal. A mask on `cat.sch.*.*` and a mask on
//! `cat.sch.tbl.col` both decide a request for that column, and the evaluator
//! refuses that pair too.
//!
//! This guard does **not** refuse it, and that is not an oversight. Those two
//! policies carry DIFFERENT resource specificity, so `getEvalOrder()` separates
//! them and the engine's choice is determined by the artifact — the evaluator
//! refuses because it does not reproduce that score, which is a modelling gap
//! in the reader, not a defect in the thing written. Refusing it here would
//! make an ordinary, correct configuration (a schema-wide mask plus a
//! column-specific one) unwritable.
//!
//! ⚠ The consequence, stated rather than hidden: **egret can still emit an
//! artifact its own evaluator declines.** The two must not be "tidied" into one
//! predicate — they answer different questions, and the day somebody reproduces
//! `computeEvalOrder()` the evaluator's refusal shrinks and this one does not
//! move at all.
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
//! The rule lives in ONE function, [`ambiguous_arbitration_in`], and every
//! surface calls it rather than re-deriving it:
//!
//! * **The rule compiler.** `egret_perm_compile::trigger::compile_desired`
//!   checks the whole desired set of one profile before it is diffed, and
//!   returns `CompileError::AmbiguousMaskOrFilterArbitration`. That ONE call
//!   site covers four surfaces, because every one of them funnels through a
//!   recompile: `POST`/`PATCH /api/control-plane/perm/rules`, `POST
//!   /api/control-plane/perm/preview` (which runs the real write in a
//!   rolled-back transaction, so the steward is refused BEFORE Save), the
//!   grant/role/clearance writes, and the tag, role-membership and
//!   workspace-membership writes that recompile on their own triggers.
//! * **The policy editor.** `POST /api/control-plane/policies` with `kind:
//!   "masking"`/`"rowfilter"`/`"tag-masking"`/`"tag-rowfilter"`, and `PUT
//!   /policies/{id}` with the two non-tag kinds (`editor_to_ranger_json`
//!   rejects every `tag-*` kind on the PUT path with "unknown policy kind", so
//!   a tag mask cannot be *replaced* into the shape, only created). Both
//!   compare the body being written against the profile's other ENABLED
//!   policies.
//! * **`PUT /policies/{id}/enabled` is NOT guarded, and that is a ruling.**
//!   Re-enabling a policy can complete the pair. The reason it is left alone is
//!   that it is also the only way OUT of a pair that already exists in an
//!   installation (disable one half, fix the model, re-enable), and the create
//!   and update paths already refuse the pair whether or not the body is
//!   enabled would be a different rule from the one above — this guard reads
//!   `isEnabled`, so a DISABLED policy is simply not part of any band. A pair
//!   that gets completed by re-enabling is therefore reachable, loudly refused
//!   by the next compile or editor write, and never silent: `explain` answers
//!   `EvalOrderAmbiguous` over the served artifact the moment it exists.
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
    fn policy_type(self) -> i64 {
        match self {
            Arbitrated::Mask => 1,
            Arbitrated::RowFilter => 2,
        }
    }

    fn from_policy_type(code: i64) -> Option<Self> {
        match code {
            1 => Some(Arbitrated::Mask),
            2 => Some(Arbitrated::RowFilter),
            _ => None,
        }
    }

    /// The noun a steward reads. "mask" and "row filter" are what the
    /// Governance UI calls a rule's two kinds.
    fn noun(self) -> &'static str {
        match self {
            Arbitrated::Mask => "mask",
            Arbitrated::RowFilter => "row filter",
        }
    }

    /// What the two policies disagree about, in the message.
    fn disagreement(self) -> &'static str {
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
    /// The resource both policies select, rendered from the canonical key so
    /// the message and the comparison cannot describe different things.
    pub resource: String,
    pub priority: i64,
}

impl AmbiguousArbitration {
    /// **The refusal, and it is the same sentence from every write path** — one
    /// message, so the surfaces cannot describe the same state differently.
    ///
    /// It names both policies, the resource, the priority, what they disagree
    /// about, and the remedy. It deliberately does NOT say the outcome is
    /// random: see this module's doc on what the comparator actually does. What
    /// it says is that the artifact does not express which rule wins, which is
    /// true under every reading of the jar and is the part a steward can fix.
    ///
    /// Asserted by substring rather than against a golden string: a golden copy
    /// of the sentence is a second place to edit it.
    pub fn refusal(&self) -> String {
        let Self {
            arbitrated,
            first,
            second,
            resource,
            priority,
        } = self;
        let noun = arbitrated.noun();
        let disagreement = arbitrated.disagreement();
        format!(
            "two {noun} policies would be served at policyPriority {priority} selecting the same \
             resource ({resource}) with {disagreement}: {first:?} and {second:?}. Ranger \
             arbitrates {noun}s with `>=`, so the FIRST evaluator in its band is final, and \
             within a band two policies on one resource score the same `getEvalOrder()` — which \
             one applies falls through to a tie-break this artifact does not express (the policy \
             name, i.e. a rule's uuid). Narrow one of the two so they no longer select the same \
             resource, or delete one, or make them agree."
        )
    }
}

/// The canonical form of a policy's `resources` map — the collision key's
/// resource half, and the thing the refusal renders.
///
/// Levels sorted by name; each level's values sorted and de-duplicated (Ranger
/// builds one matcher per value and matches if ANY matches); a level whose
/// `values` is empty or absent treated as ABSENT, because
/// `RangerAbstractResourceMatcher.init()` drops an empty policy value and a
/// level with no matchers is `isMatchAny`. `isExcludes` and `isRecursive` are
/// part of the key because they change what the level matches.
fn canonical_resources(body: &Value) -> BTreeMap<String, (Vec<String>, bool, bool)> {
    let mut out = BTreeMap::new();
    let Some(obj) = body.get("resources").and_then(Value::as_object) else {
        return out;
    };
    for (level, spec) in obj {
        let mut values: Vec<String> = spec
            .get("values")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if values.is_empty() {
            continue;
        }
        values.sort();
        values.dedup();
        let flag = |k: &str| spec.get(k).and_then(Value::as_bool).unwrap_or(false);
        out.insert(
            level.clone(),
            (values, flag("isExcludes"), flag("isRecursive")),
        );
    }
    out
}

/// `canonical_resources` rendered for a human: `catalog=[wh] table=[report]`,
/// with `!` marking `isExcludes` and `/r` marking `isRecursive`.
///
/// Rendered from the KEY rather than from the body, so the sentence cannot name
/// a resource the comparison did not use. Deliberately not
/// `admin::summarize_policy`'s `resource_path`: that one takes the FIRST value
/// of each of four hard-coded trino level names, which drops StarRocks'
/// `database` level and every value after the first — fine for a list view,
/// wrong for a refusal that has to be precise about what collided.
fn render_resources(key: &BTreeMap<String, (Vec<String>, bool, bool)>) -> String {
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

/// `RangerAbstractPolicyEvaluator.hasDeny()`: `policy.getIsDenyAllElse() ||
/// isNotEmpty(policy.getDenyPolicyItems())`. One spelling, the same one
/// `egret_ranger_eval::eval::PolicyView` reproduces.
fn has_deny(body: &Value) -> bool {
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
fn is_enabled(body: &Value) -> bool {
    match body.get("isEnabled") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool().unwrap_or(true),
    }
}

/// Absent or `null` means 0 — `RawRangerPolicy`'s `null_as_default`.
fn int_field(body: &Value, field: &str) -> i64 {
    body.get(field).and_then(Value::as_i64).unwrap_or(0)
}

/// The band one policy lands in, within one download.
type Band = (
    i64,
    i64,
    bool,
    i64,
    BTreeMap<String, (Vec<String>, bool, bool)>,
);

/// **The whole rule.** `Some(_)` means the set would be served with a pair
/// whose arbitration the artifact does not express; the caller turns it into
/// its own refusal carrying [`AmbiguousArbitration::refusal`].
///
/// Pure: no I/O, no clock, no ordering requirement on the input. The result is
/// the lexicographically first offending pair regardless of the order the
/// caller iterated in, so two surfaces checking the same set report the same
/// pair.
///
/// Disabled policies, and policies of any `policyType` but 1 and 2, are not in
/// any band and are skipped — see this module's doc for both rulings.
pub fn ambiguous_arbitration_in<'a>(
    policies: impl IntoIterator<Item = PolicyUnderWrite<'a>>,
) -> Option<AmbiguousArbitration> {
    // (band → the names and decisive items of every policy in it). Collected
    // first and scanned after, so the answer does not depend on input order.
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
    let mut bands: BTreeMap<Band, Vec<(String, Value)>> = BTreeMap::new();
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
        let items = p
            .body
            .get(item_key(arbitrated.policy_type()))
            .cloned()
            .unwrap_or(Value::Null);
        bands
            .entry((
                p.service,
                arbitrated.policy_type(),
                has_deny(p.body),
                int_field(p.body, "policyPriority"),
                canonical_resources(p.body),
            ))
            .or_default()
            .push((p.name.to_string(), items));
    }

    for ((_, policy_type, _, priority, resources), mut members) in bands {
        if members.len() < 2 {
            continue;
        }
        // By NAME only — `Value` is not `Ord`, and the name is what the
        // comparator's own last tie-break uses.
        members.sort_by(|a, b| a.0.cmp(&b.0));
        // Compare every member against the first. If the first k agree, the
        // first disagreement is found against the representative; if none
        // disagrees, the whole band agrees and there is nothing undetermined.
        let (first_name, first_items) = members[0].clone();
        for (name, items) in members.into_iter().skip(1) {
            if items != first_items {
                return Some(AmbiguousArbitration {
                    arbitrated: Arbitrated::from_policy_type(policy_type)
                        .expect("band keys are built from Arbitrated::policy_type"),
                    first: first_name,
                    second: name,
                    resource: render_resources(&resources),
                    priority,
                });
            }
        }
    }
    None
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
        // 1. Different resource: different `getEvalOrder()`, and anyway no
        //    request is decided by both on the same column.
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

    /// `isExcludes` and `isRecursive` DO change what a level matches, so they
    /// split the band.
    #[test]
    fn excludes_and_recursive_split_a_band() {
        for flag in ["isExcludes", "isRecursive"] {
            let (an, a) = mask("a-mask", "item", "MASK");
            let (bn, mut b) = mask("b-mask", "item", "MASK_NULL");
            b["resources"]["column"][flag] = json!(true);
            assert_eq!(
                check(&[(an, a), (bn, b)]),
                None,
                "{flag} changes what the level matches, so it is a different resource"
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
}
