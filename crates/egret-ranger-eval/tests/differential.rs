//! The differential gate (SP4d-1 Task 7): the same policy set and the same
//! request through the REAL `RangerPolicyEngineImpl` and through this crate,
//! compared.
//!
//! Everything else that tests this evaluator was written by the people who
//! wrote it, against their own reading of the jars. Two implementations that
//! share a misreading agree perfectly. This test is the only one that can
//! falsify that reading, because the other side of the comparison is Apache
//! Ranger itself: `tests/ranger` builds a real `RangerPolicyEngineImpl` over
//! the generated `servicePolicies` and answers the generated request.
//!
//! Gated on `EGRET_TEST_RANGER_E2E=1` (it needs a JVM, Maven and the two
//! `ranger-plugins-common` jars), skipped otherwise.
//!
//! ```bash
//! EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential -- --nocapture
//! ```
//!
//! ## What the corpus covers, and why each axis is here
//!
//! Each axis exists because some earlier task's claim can only be falsified
//! here — not for coverage's sake:
//!
//! * **Wildcards.** Task 4 REIMPLEMENTED `FilenameUtils.wildcardMatch` rather
//!   than binding to it and said the two had never been compared. The value
//!   vocabularies below put `*` and `?` in leading, trailing, interior and
//!   repeated positions, include empty values and values that are only `*`,
//!   and lean on the case differences both engines' `matcherOptions` declare
//!   (`ignoreCase: true`).
//! * **Request-side `*` and `""`.** `getMatchType` short-circuits on
//!   `isAllValuesRequested` BEFORE reading a single policy value, so a request
//!   for `*` matches only a match-any policy. Easy to get backwards; no
//!   hand-written case covered it.
//! * **Several same-priority policies over one resource.** Task 5 did not
//!   reproduce `getEvalOrder()` and named that its weakest link; Task 6 then
//!   found masks are arbitrated with `>=`, so the FIRST yielding evaluator is
//!   final and order is load-bearing there. [`Shape`] and [`resource_map`] deliberately
//!   builds most cases from one shared resource shape at one priority.
//! * Hierarchy depth including the ANCESTOR shape, `isExcludes`, multi-value
//!   levels, `additionalResources`, `policyPriority`, all four access buckets
//!   with overlapping principals, `isDenyAllElse`, `impliedGrants` (Trino's
//!   `all`), the literal `public`, `{USER}`, disabled policies, a policy of
//!   the wrong `policyType`, and all three policy types.
//!
//! **Row-filter cases are issued at TABLE granularity.** `rowFilterDef`
//! declares no `column` on either engine, so a column-level request makes
//! every row-filter policy an ANCESTOR match and yields no filter at all —
//! the corpus would look green while testing nothing.
//!
//! ## The tag phase (SP4d-2b Task 5)
//!
//! A `tagPolicies` block and a resolved tag set are generated too, so the
//! phase Tasks 2-4 built and the composition Task 3 built are measured against
//! the real engine rather than against their authors' reading of it.
//!
//! **The enricher is bypassed on BOTH sides, symmetrically.** This crate takes
//! the resolved tag set as an INPUT ([`AccessRequest::resolved_tags`]) and
//! documents that producing a sound one is the caller's obligation; the Java
//! half is handed the identical set through
//! `RangerAccessRequestUtil.setRequestTagsInContext`, which is precisely what
//! `RangerTagEnricher.enrich` writes. So the question this corpus asks is what
//! `evaluateTagPolicies` DOES with a given set — and both sides get the same
//! one. Two things follow, and the generator honours them rather than papering
//! over them:
//!
//! * neither generated service-def declares `contextEnrichers` (see
//!   [`load_engine`]), so no enricher can overwrite the seeded set;
//! * `processServiceTags`' drop of an attachment whose service-resource key
//!   set no hierarchy admits does NOT run on the Java side, so the generator
//!   never emits one — a disagreement manufactured by the harness is not a
//!   finding. `enriched_tags` is covered by `tests/tag_refusals.rs` and by
//!   `EgretTagServiceResourceValidationTest`, which measures the same table
//!   off the real `createRangerServiceResourceMatcher`.
//!
//! Tag axes: a tag the block's policies can reach and one they cannot; each
//! admitted match type (`SELF`, `SELF_AND_ALL_DESCENDANTS`, `ANCESTOR`) and
//! each unadmitted one (`DESCENDANT`, `NONE`); the deny-capable tag ACCESS
//! policy, the tag MASK and the tag ROW FILTER; several tag policies at one
//! priority; the same tag name at two match types; the empty tag set (the
//! measured narrowing); `tag.json`'s `ignoreCase: false`, which makes
//! `PII` and `pii` different tags where every resource level is
//! case-insensitive; and the audit-disabled whole-policy PRUNE together with
//! its resource-side complement, since only the tag repository performs it.
//!
//! Two shapes are built DELIBERATELY, by [`composed_duel`] and
//! [`candidacy_duel`], because a random draw does not produce them: **a rate
//! over a population that cannot reach a refusal is not a measurement.** 800
//! random cases reached offset 553's refusal zero times; 264 deliberate
//! candidacy duels reach it 95.8% of the time.
//!
//! ## What the corpus deliberately does NOT contain
//!
//! Only constructs the evaluator claims to model. A generated `Undecidable`
//! proves nothing about agreement, so the generator avoids every construct
//! that produces one (`validitySchedules`, policy/item `conditions`,
//! `zoneName`, `securityZones`, `policyDeltas`, `serviceConfig`,
//! `${{ … }}` expressions, `{{ … }}` row filters, `{OWNER}` with no owner, the
//! `_any`/`_admin`/`_super_user` pseudo access types, access types that
//! differ from a policy's only by case, and — on the tag side — a tag carrying
//! `options` or `validityPeriods`, a tag policy resource the trie and the
//! matcher are measured to disagree on, and a deny-capable tag MASK or ROW
//! FILTER policy) and the test ASSERTS none appear — so the generator cannot
//! quietly drift into cases the evaluator declines to answer while the gate
//! reports green.
//!
//! The exceptions are the FIVE refusals that are about an ORDER, a CANDIDACY or
//! a plugin VERSION this crate does not reproduce, rather than about a construct
//! it does not model. They are generated ON PURPOSE, counted and reported, and
//! never compared — this crate has no answer to compare.
//!
//! ⚠ **Every name added to that list is a name the escape check stops
//! catching**, so the line is drawn deliberately and [`only_declined`] is
//! all-or-nothing per case: a `why` mixing one of the five with a construct
//! refusal is a generator escape wearing a disguise and still fails the run.
//!
//! * [`UndecidableCause::EvalOrderAmbiguous`] — two policies tie inside one
//!   `(priority, hasDeny)` band and `getEvalOrder()` decides the winner.
//!   Measured on the RESOURCE path and on the COMPOSED path separately, which
//!   are different questions: Task 2 established the refusal cannot arise in
//!   the tag phase alone (both tag comparators are fully reproduced), and Task
//!   3 found it arises once the phases compose on ACCESS, because under a tag
//!   DENY an allowing resource policy raises `policyPriority` and offset 469's
//!   `>=` then shuts out the rest of its own band.
//! * [`UndecidableCause::ResourceCandidacyNotReproducible`] — offset 553 reads
//!   `hasDeny()` and not the matcher, so an evaluator Ranger's trie holds and
//!   this crate's matcher does not can stop the loop. `trie_excludes` proves
//!   the ordinary case absent; what it cannot prove, this refuses. Its
//!   population is any download egret did not author, which is exactly what
//!   `explain` reads off a live plugin, so the RATE is the measurement that
//!   says whether the refusal is triageable.
//! * [`UndecidableCause::TagSubResultSplitByResourcePolicy`] — a tag mask or
//!   row filter whose DATA a reachable resource policy would overwrite while
//!   Ranger keeps the tag policy's IDENTITY.
//! * [`UndecidableCause::TagPriorityArbitrationNotModelled`] — two tag policies
//!   at different priorities both decide, and the
//!   `RangerAccessRequestUtil.getAccessTypeResults` map settles which survives.
//!   Task 5 measured that arbitration; it is refused rather than modelled, for the
//!   reason on that variant.
//! * [`UndecidableCause::AccessPriorityCompositeNotModelled`] — the SAME map on
//!   the RESOURCE path: a tag ALLOW under a strictly higher-priority resource
//!   allow. Found by the whole-branch review as a confident over-DENY; the
//!   `RANDOM composed ACCESS case with a resource policy above priority 0` axis states which part of
//!   the corpus can reach it and which cannot.
//! * [`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`] — the one
//!   composed shape 2.5.0 and 2.8.0 answer DIFFERENTLY. It fires on the Trino half
//!   of this corpus and never on the StarRocks half, and ⚠ **that asymmetry used to
//!   be described here as "an assertion in itself" while no code compared the
//!   halves — they were equal, at 0 == 0, the file's own failure condition.**
//!   [`version_split_duel`] now generates the shape and
//!   [`check_version_split_is_asymmetric`] is the assertion.
//!
//! The one measured version divergence — an `_any` request against an
//! ANCESTOR-matching `isDenyAllElse` policy denies at 2.5.0 and does not at
//! 2.8.0 — is out of the corpus by construction, since `_any` is
//! [`UndecidableCause::PseudoAccessType`]. It is recorded in
//! `tests/ranger/README.md` instead.
//!
//! ## The migration corpus (SP4d-3 Task 8)
//!
//! A second, separate question at the bottom of this file
//! (`a_migration_the_grammar_accepts_decides_identically_on_the_real_engine`):
//! does a hand-written policy the `egret-perm-compile` grammar ACCEPTS decide
//! identically on the real Java engine once it is replaced by what egret's
//! compiler emits for it? Java before against Java after, pairwise — this
//! crate's evaluator takes no part. Read that section's header for how the
//! corpus is enumerated from the service-defs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, EngineConfig,
    EngineType, MaskDecision, PluginVersion, ResolvedTag, RowFilterDecision, TagServiceResource,
    UndecidableCause, UnenrichedTags,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::{MatchType, RequestResource};
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Map, Value};

// SP4d-3 Task 8 only — the migration corpus at the bottom of this file.
use egret_perm::{EngineType as PermEngine, Grant, Rule, Selector};
use egret_perm_compile::grammar::{
    grants_from_access_policy, rules_from_mask_or_filter_policy, Blocker, ServiceKind,
};
use egret_perm_compile::{compile_grant, compile_rule, validate_grant_shape, ResolvedNames};
use egret_policy::admin::access_to_ranger_json;
use uuid::Uuid;

/// The seed the recorded run used. Overridable with `EGRET_DIFF_SEED` so a
/// reported failure can be reproduced exactly, and so a wider sweep can be run
/// by hand without editing the file.
const DEFAULT_SEED: u64 = 0x5_4d00_0007;

/// Cases per engine. Two engines, so the corpus is twice this. Overridable
/// with `EGRET_DIFF_CASES`.
///
/// 2000 is where runtime starts to be the binding constraint rather than
/// coverage: 4000 cases take ~37s wall including three Maven runs, and raising
/// it to 5000 per engine (10000 cases, ~63s, also run green) turned up nothing
/// the smaller corpus had not already found.
const DEFAULT_CASES: usize = 2000;

// ── the deterministic generator ──────────────────────────────────────────

/// SplitMix64. A fixed, inlined PRNG rather than a dependency: this crate is
/// deliberately dependency-light, and a generator whose stream is pinned in
/// this file cannot change under a `cargo update` and silently retire the
/// seed a failure was reported against.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        (self.next_u64() % n as u64) as usize
    }

    /// `numerator` times in `denominator`.
    fn chance(&mut self, numerator: u32, denominator: u32) -> bool {
        (self.next_u64() % u64::from(denominator)) < u64::from(numerator)
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Values a policy resource level can carry. Every wildcard shape Task 4's
/// reimplementation distinguishes is in here: leading (`*ve`), trailing
/// (`hi*`), interior (`a*b`), repeated non-contiguous (`*a*b*`), adjacent
/// (`a**b`), a pure run (`*`, `**`), `?` alone and mixed with `*`, the empty
/// value Ranger's `init()` drops before building a matcher, and names that
/// differ from a request value only by case.
/// `?*?`, `*?` and friends are BACK, and deliberately over-represented: they
/// are where finding F1 lived, and a corpus that avoids the shape a fix was
/// written for cannot confirm the fix. A `*` immediately followed by a `?`
/// consumes nothing in `FilenameUtils.wildcardMatch`, and `resource.rs` now
/// reproduces that instead of matching greedily.
const POLICY_VALUES: &[&str] = &[
    "", "*", "**", "?", "??", "?*?", "*?", "*??", "?*", "*?*", "**?", "?**", "a*?", "*a?", "?a*",
    "*?a", "a*b", "a**b", "*a*b*", "x?z", "sales", "SALES", "sal*es", "*_tmp", "tmp_*", "or?er",
    "**x", "x**", "hive", "HIVE", "h?ve", "h*e", "*ve", "hi*", "col*", "*d", "i?", "*s*", "s**s",
    "orders", "email",
    // The BACKTRACKING resume point: a literal token after a `*`, where the
    // first occurrence of that literal is a dead end and a later one works.
    // Review deleted `backtrack.push` and the whole corpus stayed green,
    // because almost every wildcard value here ended in `*` — and a trailing
    // `*` jumps to the end of the text, so no resume point is ever needed.
    // These do not end in `*`, and `REQUEST_VALUES` carries texts in which
    // their literal runs RECUR, which is what makes the rule reachable.
    "*ab*ab", "*a*a", "*s*s", "*ab*b", "*a*b*a", "*er*er",
];

/// Values a REQUEST resource level can carry. `*` and `""` are both here on
/// purpose: `RangerAbstractResourceMatcher.isAllValuesRequested` treats them
/// alike and short-circuits before any policy value is read.
const REQUEST_VALUES: &[&str] = &[
    "hive",
    "HIVE",
    "sales",
    "Sales",
    "orders",
    "id",
    "email",
    "a",
    "ab",
    "abc",
    "hive_tmp",
    "tmp_1",
    "*",
    "",
    "s",
    "xyz",
    // Texts whose literal runs RECUR, so a pattern like `*ab*ab` — or the
    // `a*b` that `covering_values` derives from `ababab` — has a first
    // occurrence that dead-ends and a later one that works. Without a text
    // like these, the resume point above is unreachable and the corpus is
    // green whether or not it exists.
    "ababab",
    "sisters",
    "banana",
    "assessors",
];

const POLICY_USERS: &[&str] = &["alice", "bob", "carol", "{USER}"];
const POLICY_GROUPS: &[&str] = &["public", "analysts", "admins"];
const POLICY_ROLES: &[&str] = &["r_a", "r_b"];
const REQUEST_USERS: &[&str] = &["alice", "bob", "dave"];
const REQUEST_GROUPS: &[&str] = &["analysts", "admins", "other"];
const REQUEST_ROLES: &[&str] = &["r_a", "r_c"];

// ── the tag vocabularies (SP4d-2b Task 5) ────────────────────────────────

/// The service-def the generated `tagPolicies` block carries — egret's own
/// `tag.json`, the file `ranger/api/download.rs` embeds for any engine whose
/// profile names a `tagService`.
///
/// Its `matcherOptions` are `{ wildCard: true, ignoreCase: FALSE }`, where
/// every resource level of `trino.json` and `starrocks.json` is
/// case-INSENSITIVE. That difference is the reason `PII` and `pii` are both in
/// the vocabularies below: on the resource path they are one value, and on the
/// tag path they are two.
const TAG_DEF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../egret-policy/src/service-defs/tag.json"
);

/// Where a TAG policy's `id` starts.
///
/// Resource policies are numbered from 1 and tag policies from here, so the two
/// **never collide**. That is not cosmetic: the differential gate compares the
/// reported `policyId` (see [`classify_id_divergence`]), and the only way to tell "the crate
/// named the tag policy and Ranger named the resource policy" — the declared
/// cross-phase restore divergence — from an ordinary mismatch is to read which
/// side of this line the id falls on. Before SP4d-2b Task 5 fix round 1 both
/// halves numbered from 1 and a reproducer's `policyId: 2` was ambiguous.
const TAG_POLICY_ID_BASE: i64 = 900;

/// Tag names a request's resolved set can carry. `p` is a PREFIX of `pii` and
/// `pii_x` has `pii` as a prefix, so a `p*` or `pii*` policy value separates
/// "covers this tag" from "is a prefix of its name"; `PII` is the case
/// control.
const TAG_NAMES: &[&str] = &["pii", "PII", "phi", "pci", "p", "pii_x", "secret"];

/// Values a TAG policy's `resources.tag` level may carry.
///
/// **Only the three shapes `trie_reproduces_matcher` admits**: a literal, a
/// literal with a single trailing `*`, and match-any. Every other shape
/// (`*ii`, `p*x`, `pi?`, `isExcludes`, `isRecursive`) makes this crate REFUSE
/// with `TagResourceNotReproducible`, which is a refusal about the policy's
/// SHAPE and not about an order — it is pinned against the real
/// `RangerResourceTrie` by `EgretTagTrieAgreementTest` and against the crate's
/// own table by `trie_agreement_matches_the_measured_table`. Generating it
/// here would add refusals the gate cannot compare while measuring nothing
/// those two do not already measure.
const TAG_POLICY_VALUES: &[&str] = &[
    "pii", "PII", "phi", "pci", "p", "pii_x", "secret", "*", "**", "p*", "pi*", "pii*", "P*",
    "PI*", "s*", "ph*", "x*",
];

/// Every `MatchType` a resolved tag can carry, admitted and not.
///
/// `DESCENDANT` and `NONE` are here BECAUSE they are not admitted: at the
/// default `SELF` scope with a concrete access type Ranger lets a tag reach the
/// policy items only at `SELF`, `SELF_AND_ALL_DESCENDANTS` and `ANCESTOR`, and
/// a corpus that never produced the other two could not tell that rule from
/// "admit everything". Admitting `DESCENDANT` is the fail-OPEN direction, and
/// the mutation in `tests/ranger/README.md` confirms this gate catches it.
const TAG_MATCH_TYPES: &[MatchType] = &[
    MatchType::Self_,
    MatchType::SelfAndAllDescendants,
    MatchType::Ancestor,
    MatchType::Descendant,
    MatchType::None,
];

/// The match types the phase ADMITS, for the cases that must decide something.
const ADMITTED_TAG_MATCH_TYPES: &[MatchType] = &[
    MatchType::Self_,
    MatchType::SelfAndAllDescendants,
    MatchType::Ancestor,
];

/// `MatchType` as `RangerPolicyResourceMatcher.MatchType.valueOf` spells it.
fn match_type_name(match_type: MatchType) -> &'static str {
    match match_type {
        MatchType::None => "NONE",
        MatchType::Self_ => "SELF",
        MatchType::Descendant => "DESCENDANT",
        MatchType::Ancestor => "ANCESTOR",
        MatchType::SelfAndAllDescendants => "SELF_AND_ALL_DESCENDANTS",
    }
}

/// Tag policy values that COVER `name`, for the same reason
/// [`covering_values`] exists: a block whose policies reach no tag in the
/// request's set agrees with Ranger on everything and decides nothing.
///
/// `tag.json` sets `ignoreCase: false`, so the name's own case is the only
/// literal that covers it — there is deliberately no `to_uppercase` row here,
/// and `TAG_POLICY_VALUES` carries `PII` and `P*` so the NON-covering case
/// variants are generated too.
fn covering_tag_values(name: &str) -> Vec<String> {
    let mut out = vec![name.to_string(), "*".to_string(), "**".to_string()];
    let chars: Vec<char> = name.chars().collect();
    for k in 1..=chars.len() {
        out.push(format!("{}*", chars[..k].iter().collect::<String>()));
    }
    out
}

/// One engine's half of the corpus.
struct Profile {
    engine: EngineType,
    plugin_version: PluginVersion,
    /// The `-Dranger.version=` the Java half runs at.
    ranger_version: &'static str,
    service_name: &'static str,
    /// The `tagPolicies` block's own `serviceName`. Non-empty is load-bearing:
    /// `PolicyEngine.<init>` builds the tag repository only when
    /// `tagPolicies != null && !isEmpty(getServiceName()) && getServiceDef() != null`
    /// (offsets 471-500, both jars). `bind_trino_profile` names it
    /// `{service}_tag`, which is the spelling used here.
    tag_service_name: &'static str,
    def_path: &'static str,
    /// The access / data-mask hierarchy, root first.
    hierarchy: &'static [&'static str],
    /// How many of `hierarchy` the `rowFilterDef` declares. Row-filter
    /// requests stop there; see the module doc.
    row_filter_depth: usize,
    access_types: &'static [&'static str],
    mask_types: &'static [&'static str],
}

fn trino() -> Profile {
    Profile {
        engine: EngineType::Trino,
        plugin_version: PluginVersion::Ranger2_5_0,
        ranger_version: "2.5.0",
        service_name: "trino_prod",
        tag_service_name: "trino_prod_tag",
        def_path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/trino.json"
        ),
        hierarchy: &["catalog", "schema", "table", "column"],
        row_filter_depth: 3,
        // `all` is here for the impliedGrants axis: Trino's def declares a
        // flat `all => [select, insert, …]` and nothing else does.
        access_types: &[
            "select", "insert", "create", "drop", "use", "execute", "all",
        ],
        mask_types: &["MASK", "MASK_HASH", "MASK_NULL", "MASK_NONE", "CUSTOM"],
    }
}

fn starrocks() -> Profile {
    Profile {
        engine: EngineType::StarRocks,
        plugin_version: PluginVersion::Ranger2_8_0,
        ranger_version: "2.8.0",
        service_name: "starrocks_prod",
        tag_service_name: "starrocks_prod_tag",
        def_path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/starrocks.json"
        ),
        // StarRocks names its middle level `database`, not `schema` — pairing
        // the wrong vocabulary with an engine answers "nothing matched" with
        // full confidence, which is exactly what this gate must not do.
        hierarchy: &["catalog", "database", "table", "column"],
        row_filter_depth: 3,
        // StarRocks declares no impliedGrants at all, and a multi-word access
        // type (`create table`) no other def has.
        access_types: &[
            "select",
            "insert",
            "delete",
            "update",
            "alter",
            "create table",
        ],
        mask_types: &["MASK", "MASK_HASH", "MASK_NULL", "MASK_NONE", "CUSTOM"],
    }
}

/// One generated case, in both the shape Java reads and the shape this crate
/// evaluates.
#[derive(Clone)]
struct Case {
    id: String,
    policy_type: i32,
    /// Built by [`access_order_duel`] rather than drawn at random. Counted
    /// separately so "the eval-order claim survived" can be attached to a
    /// number of cases that could actually have falsified it.
    duel: bool,
    /// …and that duel was issued UNDER a tag verdict, which is the only shape
    /// in which `getEvalOrder()` can decide an ACCESS outcome. Counted apart
    /// from `duel` because the two measure different claims: the resource-path
    /// duel falsifies "order cannot change the outcome", and this one
    /// falsifies "…and composing the phases does not change that".
    composed_duel: bool,
    /// Built by [`candidacy_duel`]: the only shape that reaches offset 553's
    /// candidate-set refusal. Counted so that its rate has a denominator.
    candidacy_duel: bool,
    /// Built by [`restore_duel`]: the only shape that reaches T3's skip. It
    /// ANSWERS rather than refusing, so it has no rate — what it has is a floor,
    /// and the property it carries is that reverting T3 makes the gate RED.
    restore_duel: bool,
    /// Built by [`version_split_duel`]: a tag DENY plus a strictly higher-priority
    /// resource allow with NO deny items of its own — the one shape the two plugin
    /// versions answer DIFFERENTLY, so it REFUSES at 2.5.0 and ANSWERS at 2.8.0.
    ///
    /// ⚠ It exists because [`Declined::TagDenyVersionSplit`] had 0/4000 coverage
    /// while three places in the tree described an assertion guarding it. See
    /// [`check_version_split_is_asymmetric`], which is that assertion, now real.
    version_split_duel: bool,
    service_policies: Value,
    request: AccessRequest,
}

impl Case {
    fn to_json(&self, service_type: &str) -> Value {
        let mut resource = Map::new();
        for key in self.request.resource.keys() {
            resource.insert(
                key.to_string(),
                Value::String(self.request.resource.get(key).unwrap_or("").to_string()),
            );
        }
        let mut request = Map::new();
        request.insert("user".to_string(), json!(self.request.user));
        request.insert(
            "groups".to_string(),
            json!(self.request.groups.iter().collect::<Vec<_>>()),
        );
        request.insert(
            "roles".to_string(),
            json!(self.request.roles.iter().collect::<Vec<_>>()),
        );
        request.insert("accessType".to_string(), json!(self.request.access_type));
        request.insert("resource".to_string(), Value::Object(resource));
        // The key is INSERTED, not written as a `null`, because the Java half
        // switches on `req.has("tags")` and an explicit `null` would pass that
        // test and then fail as a cast. A case with no tag phase therefore
        // serialises exactly as it did before Task 5.
        //
        // `Some(vec![])` is NOT the same as absent and must survive the round
        // trip: it is the measured narrowing (an empty tag set makes
        // `getLikelyMatchPolicyEvaluators` return `EMPTY_LIST` at
        // `isNotEmpty(tags)`), and `setRequestTagsInContext` reaches that state
        // by REMOVING the context key — the same state, by the other route.
        if let Some(tags) = &self.request.resolved_tags {
            request.insert(
                "tags".to_string(),
                Value::Array(tags.as_slice().iter().map(tag_to_json).collect()),
            );
        }
        json!({
            "id": self.id,
            "serviceType": service_type,
            "policyType": self.policy_type,
            "servicePolicies": self.service_policies,
            "request": Value::Object(request),
        })
    }
}

/// One [`ResolvedTag`] as the Java half reads it back into a
/// `RangerTagForEval`.
///
/// `options` and `validityPeriods` have no representation here BECAUSE the
/// generator never emits one: a tag carrying either is refused by
/// `tag_candidates` ([`UndecidableCause::TagAttachmentNotReproducible`]), so a
/// case carrying one could never be compared. `attached_through` likewise has
/// none — `RangerTagForEval` carries no service resource, and the field's only
/// effect on this crate is the provenance refusal the generator keeps clear of.
fn tag_to_json(tag: &ResolvedTag) -> Value {
    json!({
        "type": tag.name,
        "matchType": match_type_name(tag.match_type),
        "attributes": tag.attributes,
    })
}

/// Policy values that COVER `request_value`, so a generated policy has a real
/// chance of matching rather than the corpus degenerating into "nothing
/// matched" — which would be green and worthless.
///
/// For a request value Ranger treats as "all values" (`""` or `"*"`) only a
/// match-any policy value covers it, which is the short-circuit this axis
/// exists to test.
fn covering_values(request_value: &str) -> Vec<String> {
    if request_value.is_empty() || request_value == "*" {
        return vec!["*".to_string(), "**".to_string()];
    }
    let chars: Vec<char> = request_value.chars().collect();
    let mut out = vec![
        request_value.to_string(),
        request_value.to_uppercase(),
        request_value.to_lowercase(),
        "*".to_string(),
        "**".to_string(),
        format!("{request_value}*"),
        format!("*{request_value}"),
        format!("*{request_value}*"),
    ];
    if chars.len() >= 2 {
        let first: String = chars[..1].iter().collect();
        let last: String = chars[chars.len() - 1..].iter().collect();
        let body: String = chars[1..chars.len() - 1].iter().collect();
        out.push(format!("{first}*"));
        out.push(format!("*{last}"));
        out.push(format!("?{}", chars[1..].iter().collect::<String>()));
        out.push(format!(
            "{}?",
            chars[..chars.len() - 1].iter().collect::<String>()
        ));
        out.push(format!("{first}*{last}"));
        out.push(format!("{first}**{last}"));
        if chars.len() >= 3 {
            out.push(format!("{first}?{}", chars[2..].iter().collect::<String>()));
            out.push(format!("*{body}*"));
        }
    }
    out
}

/// Everything a case is built around. The REQUEST is generated first and the
/// policies are then biased towards it, because a corpus of policies that
/// never match agrees with Ranger on every case and proves nothing: the whole
/// value of a differential run is in the cases where both sides actually
/// decide something.
///
/// The bias is only a bias. Every draw can still miss, `covering_values` is
/// mostly wildcards rather than literals, and a third of resource values come
/// from the adversarial vocabulary regardless — so denies, ANCESTOR misses and
/// "nothing matched" are all still generated in quantity (the tally the test
/// prints is the evidence, and the degeneracy floors at the bottom of this
/// file are what stop a future edit from silently tipping it either way).
struct Shape {
    /// `(level name, request value)`, root first, contiguous unless a gap was
    /// deliberately punched.
    request_levels: Vec<(String, String)>,
    /// How many hierarchy levels the request reaches, gaps included. Policy
    /// maps of exactly this depth are SELF; shallower is ANCESTOR (no match);
    /// deeper with match-any extras is SELF_AND_ALL_DESCENDANTS.
    request_depth: usize,
    /// How deep the policy resource maps may go.
    max_depth: usize,
    /// The few access types this case draws from, on both sides. A per-case
    /// palette rather than the whole vocabulary: with 7 access types and 1-2
    /// per item, a uniform draw almost never collides and the corpus decides
    /// nothing. The palette is drawn from the full vocabulary, so the
    /// vocabulary is still covered ACROSS cases.
    palette: Vec<String>,
    user: String,
    groups: BTreeSet<String>,
    roles: BTreeSet<String>,
    access_type: String,
}

fn build_shape(rng: &mut Rng, profile: &Profile, policy_type: i32) -> Shape {
    let max_depth = if policy_type == 2 {
        profile.row_filter_depth
    } else {
        profile.hierarchy.len()
    };

    // Row-filter requests are issued at TABLE granularity, always: the
    // rowFilterDef names no `column`, so a column-level request turns every
    // row-filter policy into an ANCESTOR match and no filter is ever
    // reported. Task 6 lost six tests to exactly that before spotting it.
    let request_depth = if policy_type == 2 {
        profile.row_filter_depth
    } else {
        1 + rng.below(profile.hierarchy.len())
    };

    let mut request_levels = Vec::new();
    for name in profile.hierarchy.iter().take(request_depth) {
        // A gap in the request hierarchy (`catalog` and `table`, no `schema`)
        // is a shape `getMatchType`'s own loop handles, so it is modelled and
        // worth generating — rarely, because most of the corpus should be
        // well-formed.
        if request_levels.is_empty() || policy_type == 2 || !rng.chance(1, 14) {
            request_levels.push(((*name).to_string(), rng.pick(REQUEST_VALUES).to_string()));
        }
    }

    let palette_size = 1 + rng.below(2);
    let mut palette: Vec<String> = Vec::new();
    while palette.len() < palette_size {
        let candidate = rng.pick(profile.access_types).to_string();
        if !palette.contains(&candidate) {
            palette.push(candidate);
        }
    }

    let mut groups = BTreeSet::new();
    for g in REQUEST_GROUPS {
        if rng.chance(1, 4) {
            groups.insert((*g).to_string());
        }
    }
    let mut roles = BTreeSet::new();
    for r in REQUEST_ROLES {
        if rng.chance(1, 5) {
            roles.insert((*r).to_string());
        }
    }

    Shape {
        request_levels,
        request_depth,
        max_depth,
        // Never a pseudo access type, and always a def access type in its
        // declared case: `_any`/`_admin`/`_super_user` and a case-only
        // difference are both Undecidable by design.
        access_type: palette[rng.below(palette.len())].clone(),
        palette,
        user: rng.pick(REQUEST_USERS).to_string(),
        groups,
        roles,
    }
}

/// One policy `resources` (or `additionalResources[i]`) map.
fn resource_map(rng: &mut Rng, profile: &Profile, shape: &Shape, follow_shape: bool) -> Value {
    // The three match types a resource request can produce, generated in
    // roughly the proportions that keep all three populated: SELF (same
    // depth), SELF_AND_ALL_DESCENDANTS (deeper, every extra level match-any)
    // and ANCESTOR (shallower — NOT an allow, the fail-open direction this
    // whole gate is pointed at).
    let depth = if !follow_shape {
        1 + rng.below(shape.max_depth)
    } else if rng.chance(11, 20) || shape.request_depth >= shape.max_depth {
        shape.request_depth.min(shape.max_depth)
    } else if rng.chance(1, 2) {
        shape.request_depth + 1 + rng.below(shape.max_depth - shape.request_depth)
    } else {
        1 + rng.below(shape.request_depth)
    };

    let mut map = Map::new();
    for (i, name) in profile.hierarchy.iter().take(depth).enumerate() {
        let request_value = shape
            .request_levels
            .iter()
            .find(|(n, _)| n.as_str() == *name)
            .map(|(_, v)| v.clone());

        let mut values: Vec<String> = Vec::new();
        // A level BELOW the request's own depth decides between
        // SELF_AND_ALL_DESCENDANTS and DESCENDANT, so it is match-any most of
        // the time and a literal the rest.
        if follow_shape && i >= shape.request_depth {
            values.push(if rng.chance(3, 4) {
                rng.pick(&["*", "**"]).to_string()
            } else {
                rng.pick(POLICY_VALUES).to_string()
            });
        } else {
            let count = if rng.chance(1, 5) { 2 } else { 1 };
            for _ in 0..count {
                let value = match (&request_value, follow_shape) {
                    // Covering values are themselves wildcards four times out
                    // of five (`h*e`, `?ive`, `hiv?`, `*`, `HIVE` under
                    // ignoreCase …), so biasing towards a match does NOT
                    // reduce the corpus to literal equality.
                    (Some(rv), true) if rng.chance(4, 5) => {
                        let candidates = covering_values(rv);
                        candidates[rng.below(candidates.len())].clone()
                    }
                    _ => rng.pick(POLICY_VALUES).to_string(),
                };
                values.push(value);
            }
        }
        // A level present with no usable values at all: Ranger's `init()`
        // drops empty values before building a matcher, leaving `isMatchAny`.
        if rng.chance(1, 25) {
            values = vec![String::new()];
        }

        let mut level = Map::new();
        level.insert("values".to_string(), json!(values));
        // `isExcludes` is NOT plain negation (`applyExcludes` carves out a
        // request for all values against a non-match-any policy), which only
        // a differential run can confirm.
        if rng.chance(1, 10) {
            level.insert("isExcludes".to_string(), json!(true));
        }
        map.insert((*name).to_string(), Value::Object(level));
    }
    Value::Object(map)
}

/// One policy item's `users` / `groups` / `roles`, biased towards the request's
/// own principals for the reason [`Shape`] gives, and carrying `{USER}` and the
/// literal `public` — the two names Ranger resolves rather than compares.
fn principals(rng: &mut Rng, shape: &Shape) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut users: Vec<String> = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    let mut roles: Vec<String> = Vec::new();

    if rng.chance(2, 5) {
        users.push(shape.user.clone());
    }
    // `{USER}` is `RangerPolicyEngine.USER_CURRENT`: `init` reads it with a
    // plain `List.contains`, so it matches every requesting user exactly.
    if rng.chance(1, 8) {
        users.push("{USER}".to_string());
    }
    for u in POLICY_USERS {
        if *u != "{USER}" && rng.chance(1, 5) {
            users.push((*u).to_string());
        }
    }
    // `public` matches whatever the request's groups are, including none.
    if rng.chance(1, 8) {
        groups.push("public".to_string());
    }
    if let Some(g) = shape.groups.iter().next() {
        if rng.chance(1, 4) {
            groups.push(g.clone());
        }
    }
    for g in POLICY_GROUPS {
        if *g != "public" && rng.chance(1, 8) {
            groups.push((*g).to_string());
        }
    }
    if let Some(r) = shape.roles.iter().next() {
        if rng.chance(1, 5) {
            roles.push(r.clone());
        }
    }
    for r in POLICY_ROLES {
        if rng.chance(1, 10) {
            roles.push((*r).to_string());
        }
    }

    if users.is_empty() && groups.is_empty() && roles.is_empty() {
        users.push(rng.pick(POLICY_USERS).to_string());
    }
    users.sort();
    users.dedup();
    groups.sort();
    groups.dedup();
    roles.sort();
    roles.dedup();
    (users, groups, roles)
}

fn accesses(rng: &mut Rng, profile: &Profile, shape: &Shape) -> Value {
    let count = 1 + rng.below(2);
    let mut out = Vec::new();
    for _ in 0..count {
        let access_type = if rng.chance(1, 10) && profile.access_types.contains(&"all") {
            // Trino's `all` is the only impliedGrants entry either def
            // declares: an item granting it must match a request for
            // `select`, through `computeWithImpliedGrants`.
            "all".to_string()
        } else if rng.chance(4, 5) {
            shape.palette[rng.below(shape.palette.len())].clone()
        } else {
            rng.pick(profile.access_types).to_string()
        };
        // `isAllowed: false` inside an allow bucket is modelled exactly
        // (`isMatch` tests `access.getIsAllowed()`), so it is a legitimate
        // axis rather than an unmodelled construct.
        let is_allowed = !rng.chance(1, 10);
        out.push(json!({"type": access_type, "isAllowed": is_allowed}));
    }
    // An access entry whose `type` Ranger reads as `""` — finding F3's most
    // surprising row. `RangerPolicyItemAccess.setType(null)` substitutes `""`
    // and an absent key leaves the constructor's own `""`, so all three
    // spellings below are ONE value to Ranger, and none of them is an error.
    // `""` equals no concrete access type under `equalsIgnoreCase`, so such
    // an entry grants nothing — but that is a DECISION, and until now only a
    // unit test said so. It is generated as an extra entry beside a real one
    // most of the time, so it probes the reading without starving the corpus
    // of matches.
    if rng.chance(1, 9) {
        let entry = match rng.below(3) {
            0 => json!({"type": "", "isAllowed": true}),
            1 => json!({"type": Value::Null, "isAllowed": true}),
            _ => json!({"isAllowed": true}),
        };
        if rng.chance(1, 6) {
            out.clear();
        }
        out.push(entry);
    }
    Value::Array(out)
}

fn access_item(rng: &mut Rng, profile: &Profile, shape: &Shape) -> Value {
    let (users, groups, roles) = principals(rng, shape);
    json!({
        "users": users,
        "groups": groups,
        "roles": roles,
        "accesses": accesses(rng, profile, shape),
    })
}

fn mask_item(rng: &mut Rng, profile: &Profile, shape: &Shape) -> Value {
    let (users, groups, roles) = principals(rng, shape);
    let mut item = json!({
        "users": users,
        "groups": groups,
        "roles": roles,
        "accesses": accesses(rng, profile, shape),
    });
    // An item with NO `dataMaskInfo` is generated again, and is finding F2's
    // own shape: Ranger's model class substitutes an empty
    // `RangerPolicyItemDataMaskInfo`, so such an item DETERMINES the result
    // with a null mask type rather than standing aside. Its row-filter
    // counterpart (an item with no `rowFilterInfo`) behaves the OPPOSITE way
    // and is generated by `row_filter_item`; both are in the corpus because
    // the fix had to tell them apart.
    if !rng.chance(1, 8) {
        let mask_type = rng.pick(profile.mask_types).to_string();
        let mut info = Map::new();
        info.insert("dataMaskType".to_string(), json!(mask_type));
        if mask_type == "CUSTOM" {
            // A CUSTOM mask's `{col}` is a LITERAL to Ranger's decision: no
            // token replacer runs in the mask evaluator, so the engine
            // reports it verbatim. Never `${{ … }}`, which would resolve
            // against request context and is out of the corpus.
            let expr = rng
                .pick(&["{col}", "concat({col}, '_x')", "'redacted'"])
                .to_string();
            info.insert("valueExpr".to_string(), json!(expr));
        }
        if rng.chance(1, 10) {
            info.insert("conditionExpr".to_string(), json!("dept = 'eng'"));
        }
        item.as_object_mut()
            .unwrap()
            .insert("dataMaskInfo".to_string(), Value::Object(info));
    }
    item
}

fn row_filter_item(rng: &mut Rng, profile: &Profile, shape: &Shape) -> Value {
    let (users, groups, roles) = principals(rng, shape);
    let mut item = json!({
        "users": users,
        "groups": groups,
        "roles": roles,
        "accesses": accesses(rng, profile, shape),
    });
    // Three distinct states Ranger keeps apart and a lesser reader folds
    // together: no `rowFilterInfo`; a `filterExpr` of null (the evaluator's
    // constructor stores null and `updateAccessResult` never marks the
    // result); and an explicit `""`, which is not null and DOES mark it.
    // Never `{{ … }}` — `JavaScriptEdits.replaceDoubleBrackets` rewrites
    // those and this crate refuses rather than reproducing the rewrite.
    let info = match rng.below(8) {
        0 => json!({}),
        1 => json!({ "filterExpr": Value::Null }),
        2 => json!({"filterExpr": ""}),
        3 | 4 => json!({"filterExpr": "country = 'US'"}),
        5 | 6 => json!({"filterExpr": "id > 10"}),
        _ => json!({"filterExpr": "dept in ('eng','ops')"}),
    };
    if !rng.chance(1, 12) {
        item.as_object_mut()
            .unwrap()
            .insert("rowFilterInfo".to_string(), info);
    }
    item
}

/// One resource policy.
///
/// `level_priority` forces `policyPriority: 0`. ⚠ **It is set for every RANDOM
/// tag-carrying case, and that is a held-out shape with a measured reason** — see
/// the ⚠ block in [`generate_case`]. The deliberate duels build their own
/// policies and keep their priorities, so every shape this gate is pointed at
/// still varies them.
fn policy(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    id: i64,
    policy_type: i32,
    level_priority: bool,
) -> Value {
    let mut p = Map::new();
    p.insert("id".to_string(), json!(id));
    // A name-only tie-break is what Task 5 stands in for `getEvalOrder()`, so
    // names must not encode the order a correct implementation would pick.
    p.insert("name".to_string(), json!(format!("p{id}")));
    p.insert("service".to_string(), json!(profile.service_name));
    p.insert("policyType".to_string(), json!(policy_type));
    p.insert("isEnabled".to_string(), json!(!rng.chance(1, 12)));
    p.insert(
        "policyPriority".to_string(),
        json!(if !level_priority && rng.chance(1, 7) {
            1
        } else {
            0
        }),
    );
    p.insert(
        "resources".to_string(),
        resource_map(rng, profile, shape, true),
    );
    if rng.chance(1, 7) {
        let follow = rng.chance(1, 2);
        p.insert(
            "additionalResources".to_string(),
            json!([resource_map(rng, profile, shape, follow)]),
        );
    }

    match policy_type {
        1 => {
            let count = 1 + rng.below(2);
            p.insert(
                "dataMaskPolicyItems".to_string(),
                Value::Array((0..count).map(|_| mask_item(rng, profile, shape)).collect()),
            );
            // `accessPerms` is the union over all SIX buckets, so an access
            // bucket on a MASK policy changes the optimized evaluator's
            // pre-filter without ever deciding the mask.
            if rng.chance(1, 5) {
                p.insert(
                    "policyItems".to_string(),
                    json!([access_item(rng, profile, shape)]),
                );
            }
        }
        2 => {
            let count = 1 + rng.below(2);
            p.insert(
                "rowFilterPolicyItems".to_string(),
                Value::Array(
                    (0..count)
                        .map(|_| row_filter_item(rng, profile, shape))
                        .collect(),
                ),
            );
        }
        _ => {
            let allow = 1 + rng.below(2);
            p.insert(
                "policyItems".to_string(),
                Value::Array(
                    (0..allow)
                        .map(|_| access_item(rng, profile, shape))
                        .collect(),
                ),
            );
            let deny = rng.below(3);
            p.insert(
                "denyPolicyItems".to_string(),
                Value::Array(
                    (0..deny)
                        .map(|_| access_item(rng, profile, shape))
                        .collect(),
                ),
            );
            if rng.chance(1, 4) {
                p.insert(
                    "allowExceptions".to_string(),
                    json!([access_item(rng, profile, shape)]),
                );
            }
            if rng.chance(1, 4) {
                p.insert(
                    "denyExceptions".to_string(),
                    json!([access_item(rng, profile, shape)]),
                );
            }
            if rng.chance(1, 8) {
                p.insert("isDenyAllElse".to_string(), json!(true));
            }
            // ⚠ The ASYMMETRY, generated deliberately. `isAuditEnabled: false`
            // with every item bucket empty is what makes the TAG repository
            // remove a policy — and `RangerPolicyRepository`'s other three
            // constructors call `normalizeAndPrunePolicies` at all, so in
            // `policies` the identical shape is KEPT and its `isDenyAllElse`
            // denies. A crate that pruned on both paths would look right on the
            // tag half and be wrong here, and nothing else in the corpus could
            // tell the two apart.
            if rng.chance(1, 12) {
                p.insert("policyItems".to_string(), json!([]));
                p.insert("denyPolicyItems".to_string(), json!([]));
                p.remove("allowExceptions");
                p.remove("denyExceptions");
                p.insert("isDenyAllElse".to_string(), json!(true));
                p.insert("isAuditEnabled".to_string(), json!(false));
            }
        }
    }
    if rng.chance(1, 8) {
        p.insert("isAuditEnabled".to_string(), json!(false));
    }
    Value::Object(p)
}

/// Two ACCESS policies at the SAME `policyPriority` that BOTH match, one
/// reaching an allow and the other a deny, built deliberately rather than
/// hoped for.
///
/// Task 5 did not reproduce `getEvalOrder()` and argued that within one
/// `(policyPriority, hasDeny)` band the allow/deny OUTCOME cannot depend on
/// order. It named that its weakest link, and 10000 generated cases produced
/// no counterexample — but the generator could not produce the shape a
/// counterexample needs, so that was absence of evidence, not evidence. This
/// builds it on purpose.
///
/// BOTH constructible `hasDeny` combinations are generated — there are two,
/// not three. A duel needs one policy that actually denies, and a policy that
/// denies necessarily carries `denyPolicyItems`, so `hasDeny == false` on the
/// denier is unrepresentable. What varies is the ALLOWER. The comparator
/// makes the two cases different questions:
/// `PolicyEvalOrderComparator.compareNormal` (disassembled from the 2.5.0
/// jar) is `hasDeny` FIRST, then `getEvalOrder()`, then the policy name. So:
///
/// * allower ALSO `hasDeny` (it carries deny items that match nobody): one
///   band, ordered only by the `getEvalOrder()` this crate does not compute —
///   but no evaluator in such a band can break the engine loop, so every one
///   runs and any deny wins. Order should not matter.
/// * allower WITHOUT deny items: the comparator itself puts the denier first,
///   which is what makes the claim true. If the `hasDeny` key were missing,
///   the loop could break on the allow
///   (`if (ret.getIsAllowed()) { if (!evaluator.hasDeny()) break; }`) and
///   never reach the deny — fail-open.
///
/// Both policies also carry deliberately conflicting name and id orders, so a
/// name-then-id tie-break cannot be right by luck: whichever of the two this
/// crate would put first, the other is first half the time.
fn access_order_duel(rng: &mut Rng, profile: &Profile, shape: &Shape, priority: i32) -> Vec<Value> {
    // Resources that certainly match: the request's own values, at the
    // request's own depth. The duel is about ORDER, so nothing else may be in
    // doubt.
    let exact: Value = Value::Object(
        shape
            .request_levels
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    json!({"values": [if value.is_empty() || value == "*" { "*" } else { value }]}),
                )
            })
            .collect(),
    );

    let grant = json!([{
        "users": [shape.user], "groups": [], "roles": [],
        "accesses": [{"type": shape.access_type, "isAllowed": true}],
    }]);
    // A deny item naming somebody else: it makes `hasDeny` true without
    // denying anything, which is the third combination.
    let inert_deny = json!([{
        "users": ["nobody_at_all"], "groups": [], "roles": [],
        "accesses": [{"type": shape.access_type, "isAllowed": true}],
    }]);

    let mut allower = json!({
        "id": 1, "name": "duel-a", "service": profile.service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": priority,
        "resources": exact, "policyItems": grant,
    });
    // Half the time the allow-policy also carries (non-matching) deny items,
    // which moves it into the SAME `hasDeny` band as its opponent.
    if rng.chance(1, 2) {
        allower["denyPolicyItems"] = inert_deny;
    }

    let denier = json!({
        "id": 2, "name": "duel-b", "service": profile.service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": priority,
        "resources": allower["resources"].clone(),
        "denyPolicyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });

    // Swap ids AND names together, so neither an id order nor a name order is
    // a reliable proxy for which one Ranger reaches first.
    let mut policies = vec![allower, denier];
    if rng.chance(1, 2) {
        policies.swap(0, 1);
        policies[0]["id"] = json!(1);
        policies[0]["name"] = json!("duel-a");
        policies[1]["id"] = json!(2);
        policies[1]["name"] = json!("duel-b");
    }
    policies
}

// ── the tag layer (SP4d-2b Task 5) ───────────────────────────────────────

/// A `tagPolicies` block and the resolved tag set it is evaluated against —
/// the two halves of the tag phase, generated together because neither decides
/// anything without the other.
struct TagLayer {
    /// The whole `tagPolicies` value, ready to be embedded in the envelope.
    block: Value,
    /// The request's `resolved_tags`. `Some(vec![])` is generated on purpose —
    /// it is the measured narrowing, not a degenerate case.
    tags: Vec<ResolvedTag>,
}

/// One tag ATTACHMENT, built so that nothing about it refuses.
///
/// Three fields are pinned rather than varied, and each for a reason the
/// generator cannot work around:
///
/// * `options` and `validity_periods` — `tag_candidates` REFUSES any admitted
///   tag carrying either ([`UndecidableCause::TagAttachmentNotReproducible`]),
///   so a case carrying one could never be compared. They are pinned to the
///   empty/zero values the Java half's `RangerTag(type, attributes)`
///   constructor also leaves, so the two sides describe the same
///   `RangerTagForEval`.
/// * `attached_through.levels` — always three or four levels of THIS profile's
///   own hierarchy. Fewer than three is the provenance refusal (D2); a key set
///   no hierarchy admits is dropped by `processServiceTags` on this crate's
///   side and NOT on the Java side, because the harness bypasses the enricher.
///   Either would be a disagreement the harness manufactured.
///
/// `is_enabled` varies although nothing reads it: `processServiceTags` never
/// consults it, and a corpus that only ever emitted `true` could not tell that
/// ruling from an omission.
fn resolved_tag(
    rng: &mut Rng,
    profile: &Profile,
    name: &str,
    match_type: MatchType,
) -> ResolvedTag {
    let depth = 3 + rng.below(2);
    let levels: BTreeSet<String> = profile
        .hierarchy
        .iter()
        .take(depth)
        .map(|n| (*n).to_string())
        .collect();
    // `attributes` is in `RangerTagForEval.equals`, so it SEPARATES two
    // members Ranger keeps apart — which is the whole reason this crate carries
    // it. Generated on one tag in four so the dedup is exercised both ways.
    let mut attributes = BTreeMap::new();
    if rng.chance(1, 4) {
        attributes.insert(
            "sensitivity".to_string(),
            rng.pick(&["high", "low"]).to_string(),
        );
    }
    ResolvedTag {
        name: name.to_string(),
        match_type,
        attributes,
        options: BTreeSet::new(),
        validity_periods: 0,
        attached_through: TagServiceResource {
            levels,
            is_enabled: !rng.chance(1, 5),
        },
    }
}

/// The request's resolved tag set.
///
/// `names` comes back alongside so the block's policies can be biased towards
/// the tags the request actually carries — the same bias, and the same reason,
/// as [`Shape`]: a block whose policies reach no tag in the set agrees with
/// Ranger on every case and decides nothing.
fn resolved_tags(rng: &mut Rng, profile: &Profile) -> Vec<ResolvedTag> {
    // The measured narrowing: an explicitly EMPTY set makes
    // `getLikelyMatchPolicyEvaluators` return `EMPTY_LIST` at
    // `isNotEmpty(tags)`, so no tag policy can write anything however many the
    // block holds. Both sides must answer from the resource policies alone.
    if rng.chance(1, 9) {
        return Vec::new();
    }
    let count = 1 + rng.below(3);
    let mut tags = Vec::new();
    for _ in 0..count {
        let name = rng.pick(TAG_NAMES).to_string();
        // Most attachments are admitted, or the phase decides nothing and the
        // corpus degenerates into "the resource policies answered".
        let match_type = if rng.chance(3, 4) {
            *rng.pick(ADMITTED_TAG_MATCH_TYPES)
        } else {
            *rng.pick(TAG_MATCH_TYPES)
        };
        tags.push(resolved_tag(rng, profile, &name, match_type));
    }
    // The same tag NAME at two match types: two members of Ranger's set, not
    // one, because `RangerTagForEval.equals` reads `matchType`. Collapsing them
    // by name would silently reorder masks — the mask comparator IS the match
    // type — so the shape has to be generated or that claim is untested.
    if rng.chance(1, 3) {
        let base = tags[rng.below(tags.len())].clone();
        let other = *rng.pick(TAG_MATCH_TYPES);
        if other != base.match_type {
            tags.push(resolved_tag(rng, profile, &base.name, other));
        }
    }
    tags
}

/// One tag policy.
///
/// The `resources` map names ONLY `tag` and carries neither `isExcludes` nor
/// `isRecursive`: all three are `TagResourceNotReproducible`, which is a
/// refusal about a construct rather than about an order — see
/// [`TAG_POLICY_VALUES`].
/// A `dataMaskPolicyItems` entry for a TAG policy.
///
/// The one thing it may never be is [`mask_item`]'s F2 shape — an item with no
/// `dataMaskInfo`. In `policies` that is legal and is SP4d-1's pinned finding;
/// in the TAG block Ranger's model class substitutes an empty
/// `RangerPolicyItemDataMaskInfo`, `normalizeAndPrunePolicyItems` reads
/// `dataMaskType.contains(":")` on its null type, and the plugin THROWS while
/// building its repository. That is not a verdict to compare, it is a crash, and
/// it is pinned on its own by [`check_measured_tag_crash`] rather than scattered
/// through the corpus as thousands of uncomparable cases.
fn tag_mask_item(rng: &mut Rng, profile: &Profile, shape: &Shape) -> Value {
    let mut item = mask_item(rng, profile, shape);
    let object = item.as_object_mut().expect("mask_item is an object");
    let needs_type = match object.get("dataMaskInfo") {
        None => true,
        Some(info) => info.get("dataMaskType").is_none_or(Value::is_null),
    };
    if needs_type {
        object.insert(
            "dataMaskInfo".to_string(),
            json!({"dataMaskType": rng.pick(profile.mask_types)}),
        );
    }
    item
}

fn tag_policy(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    names: &[String],
    id: i64,
    policy_type: i32,
    priority: i32,
) -> Value {
    let value = match names.is_empty() {
        // The CASE variant of a tag the request actually carries. Drawn from the
        // request's own names rather than from `TAG_POLICY_VALUES`, because that
        // list is fixed and the tag names the request draws are not: a value that
        // merely LOOKS like a case variant of some name in the vocabulary is not
        // one of a name in THIS request's set, and only the latter exercises
        // `tag.json`'s `ignoreCase: false`. At 3‰ from the ordinary draw this
        // axis was one seed away from starving.
        false if rng.chance(1, 5) => {
            let name = &names[rng.below(names.len())];
            let flipped: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_uppercase() {
                        c.to_ascii_lowercase()
                    } else {
                        c.to_ascii_uppercase()
                    }
                })
                .collect();
            if rng.chance(1, 2) {
                flipped
            } else {
                // A case-flipped PREFIX plus `*`, so the difference is a case
                // difference inside a shape that would otherwise cover.
                let take = 1 + rng.below(flipped.chars().count());
                format!("{}*", flipped.chars().take(take).collect::<String>())
            }
        }
        false if rng.chance(3, 4) => {
            let name = &names[rng.below(names.len())];
            let candidates = covering_tag_values(name);
            candidates[rng.below(candidates.len())].clone()
        }
        _ => rng.pick(TAG_POLICY_VALUES).to_string(),
    };

    let mut p = Map::new();
    let id = TAG_POLICY_ID_BASE + id;
    p.insert("id".to_string(), json!(id));
    p.insert("name".to_string(), json!(format!("t{id}")));
    p.insert("service".to_string(), json!(profile.tag_service_name));
    p.insert("policyType".to_string(), json!(policy_type));
    p.insert("isEnabled".to_string(), json!(!rng.chance(1, 12)));
    p.insert("policyPriority".to_string(), json!(priority));
    p.insert("resources".to_string(), json!({"tag": {"values": [value]}}));

    match policy_type {
        1 => {
            // ⚠ NO deny bucket and NO `isDenyAllElse` on a tag MASK or ROW
            // FILTER policy. `evaluate_tag_sub` refuses a yielding
            // deny-capable one outright
            // ([`UndecidableCause::TagSubPolicyIsDenyCapable`]) because
            // Ranger would report the first policy's id with the second's
            // mask, and `SubResult` cannot express that. It is a refusal about
            // a construct, so it stays out of the corpus.
            let count = 1 + rng.below(2);
            p.insert(
                "dataMaskPolicyItems".to_string(),
                Value::Array(
                    (0..count)
                        .map(|_| tag_mask_item(rng, profile, shape))
                        .collect(),
                ),
            );
        }
        2 => {
            let count = 1 + rng.below(2);
            p.insert(
                "rowFilterPolicyItems".to_string(),
                Value::Array(
                    (0..count)
                        .map(|_| row_filter_item(rng, profile, shape))
                        .collect(),
                ),
            );
        }
        // The item-LESS shape, generated on purpose and only for ACCESS: it is
        // the one shape in which the tag repository's whole-policy prune is
        // observable, because `isPolicyNeedsPruning` tests the six item buckets
        // and NOT `isDenyAllElse`. With `isAuditEnabled: false` Ranger removes
        // the policy and reaches no verdict; with it true the identical policy
        // denies everything. `tag_policy_is_pruned` reproduces that, and this is
        // what measures the reproduction against the real engine.
        0 if rng.chance(1, 4) => {
            p.insert("isDenyAllElse".to_string(), json!(true));
            // ⚠ The tag value is REPLACED with a resolved tag's own name here, and
            // that is what makes this shape coverage rather than decoration. FIX
            // ROUND 1: once the prune axis started requiring a COVERING value
            // (IMPORTANT 3) it fell to 3‰ against a 6‰ floor, because the ordinary
            // draw covers a tag only ~1 time in 5. A pruned policy no tag reaches
            // is indistinguishable from a pruned policy that was never there.
            if let Some(name) = names.first() {
                p.insert("resources".to_string(), json!({"tag": {"values": [name]}}));
            }
            // `isAuditEnabled` is DECIDED here rather than left to the tail
            // below, and both ways: with it false the policy is PRUNED and its
            // `isDenyAllElse` cannot deny, with it true the identical policy
            // denies everything. Leaving it to a 1-in-4 tail draw left the
            // pruned shape at 3‰, which is one seed from starving the only axis
            // that covers a rule this crate now REPRODUCES.
            // False two times in three: the PRUNED shape is the one the crate
            // reproduces and the one with a floor, and the KEPT shape beside it is
            // its falsifier — but the falsifier does not need as many cases as the
            // rule it falsifies.
            p.insert("isAuditEnabled".to_string(), json!(!rng.chance(2, 3)));
        }
        _ => {
            let allow = 1 + rng.below(2);
            p.insert(
                "policyItems".to_string(),
                Value::Array(
                    (0..allow)
                        .map(|_| access_item(rng, profile, shape))
                        .collect(),
                ),
            );
            // ⚠ A deny bucket only a QUARTER of the time, where the
            // resource-side `policy()` draws 0-2 uniformly. A tag DENY shuts out
            // every same-priority resource allow through offset 469's `>=`, so a
            // deny-happy tag layer starves the whole corpus of ALLOW outcomes:
            // at `rng.below(3)` the allow count fell to 78 in 1600 cases and
            // tripped the degeneracy floor below.
            //
            // FIX ROUND 1, MINOR 8: it is 1 OR 2 items when it fires, not 1. The
            // first version drew at most one, which removed the multi-deny-item
            // shape with nothing recording the removal; `multi_deny_access` is
            // now an axis with a floor. Expected items per policy is 0.375
            // against the old 0.333, so the allow rate barely moves.
            let deny = if rng.chance(1, 4) {
                1 + rng.below(2)
            } else {
                0
            };
            p.insert(
                "denyPolicyItems".to_string(),
                Value::Array(
                    (0..deny)
                        .map(|_| access_item(rng, profile, shape))
                        .collect(),
                ),
            );
            if rng.chance(1, 5) {
                p.insert(
                    "allowExceptions".to_string(),
                    json!([access_item(rng, profile, shape)]),
                );
            }
            if rng.chance(1, 5) {
                p.insert(
                    "denyExceptions".to_string(),
                    json!([access_item(rng, profile, shape)]),
                );
            }
            if rng.chance(1, 12) {
                p.insert("isDenyAllElse".to_string(), json!(true));
            }
        }
    }
    // `isAuditEnabled: false` — the prune's gate. Emitted on every policy type,
    // not only the item-less one, so that "the prune fires" and "the flag is
    // present" stay separable: a policy WITH items and auditing off must still
    // be kept. `entry` rather than `insert`, so the item-less arm above keeps the
    // value it chose deliberately.
    if rng.chance(1, 4) {
        p.entry("isAuditEnabled".to_string())
            .or_insert_with(|| json!(false));
    }
    Value::Object(p)
}

/// The envelope's `tagPolicies` value.
///
/// `serviceName` and `serviceDef` are both present and non-empty on every
/// generated block, and that is not decoration: `PolicyEngine.<init>` builds
/// the tag repository only when `tagPolicies != null &&
/// !StringUtils.isEmpty(getServiceName()) && getServiceDef() != null` (offsets
/// 471-500, identical in 2.5.0 and 2.8.0). A block missing either builds NO
/// repository, so Ranger runs no tag phase at all — while `tag_phase_input`,
/// which reads neither field, runs one. That divergence is REAL and is
/// recorded in `tests/ranger/README.md`; it is held out of the corpus because
/// a generated case cannot both measure it and be comparable.
fn tag_block(profile: &Profile, tag_def: &Value, policies: Vec<Value>) -> Value {
    json!({
        "serviceName": profile.tag_service_name,
        "serviceId": 2,
        "policyVersion": 1,
        "serviceDef": tag_def,
        "policies": policies,
    })
}

fn tag_layer(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    tag_def: &Value,
    policy_type: i32,
) -> TagLayer {
    let tags = resolved_tags(rng, profile);
    // ⚠ The bias is towards the names of ADMITTED tags (fix round 2). A tag policy
    // whose value covers only a `DESCENDANT` or `NONE` tag is a candidate the tag
    // phase never lets reach its items, so every axis gated on admission — which is
    // all of them bar two, see the `AXES` header — counted it as nothing. Biasing
    // towards names that can actually decide lifted `tag: ACCESS policy with 2+ deny
    // items` from 5‰, where a floor could not be set meaningfully, and it lifts the
    // rest with it. The unadmitted names are still generated on the REQUEST side,
    // which is where the `DESCENDANT` / `NONE` axes read them.
    let admitted: Vec<String> = tags
        .iter()
        .filter(|t| ADMITTED_TAG_MATCH_TYPES.contains(&t.match_type))
        .map(|t| t.name.clone())
        .collect();
    let names: Vec<String> = if admitted.is_empty() {
        tags.iter().map(|t| t.name.clone()).collect()
    } else {
        admitted
    };

    let count = 1 + rng.below(3);
    // Several tag policies at ONE priority is the shape the tag phase's own
    // comparators have to order, so it is the common case rather than a rare
    // one; the override exists to put one of them above the others.
    let base_priority = if rng.chance(1, 6) { 1 } else { 0 };
    let mut policies = Vec::new();
    for n in 0..count {
        // A tag policy of a DIFFERENT type must be filed into a different
        // evaluator list in the TAG repository, exactly as on the resource
        // side.
        let this_type = if rng.chance(1, 8) {
            (policy_type + 1 + rng.below(2) as i32) % 3
        } else {
            policy_type
        };
        let priority = if rng.chance(1, 8) {
            base_priority + 1
        } else {
            base_priority
        };
        policies.push(tag_policy(
            rng,
            profile,
            shape,
            &names,
            (n + 1) as i64,
            this_type,
            priority,
        ));
    }
    TagLayer {
        block: tag_block(profile, tag_def, policies),
        tags,
    }
}

/// The ONE shape in which `getEvalOrder()` can decide an ACCESS outcome: a tag
/// verdict on the result, and a deny-capable resource band strictly above it.
///
/// Under a tag DENY, offset 469's `>=` lets a strictly higher-priority
/// evaluator run; if that evaluator ALLOWS, `updateAccessResult`'s allow branch
/// raises `policyPriority` to its own, and the `>=` then shuts out the rest of
/// its own band — including a same-band denier. Which of the two ran first is
/// `RangerOptimizedPolicyEvaluator.computeEvalOrder()`, which this crate does
/// not compute, so it refuses.
///
/// One case in four is built with a tag ALLOW instead, and that is the
/// FALSIFIER rather than filler: under an allow, offset 505's `>` lets the
/// same-priority rival run after the allow, so the deny still wins whichever
/// ran first and the crate must NOT refuse. A corpus that only built the deny
/// variant would report a rate with nothing to compare it against.
fn composed_duel(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    tag_def: &Value,
) -> (Vec<Value>, TagLayer) {
    // Strictly above the tag policy's 0, or offset 469 shuts the whole band out
    // before it runs.
    let resource_priority = 1 + rng.below(2) as i32;
    let policies = access_order_duel(rng, profile, shape, resource_priority);

    let name = rng.pick(TAG_NAMES).to_string();
    let match_type = *rng.pick(ADMITTED_TAG_MATCH_TYPES);
    let tag = resolved_tag(rng, profile, &name, match_type);
    let denies = !rng.chance(1, 4);
    let bucket = if denies {
        "denyPolicyItems"
    } else {
        "policyItems"
    };
    let mut tag_policy = Map::new();
    tag_policy.insert("id".to_string(), json!(TAG_POLICY_ID_BASE + 90));
    tag_policy.insert("name".to_string(), json!("t-composed"));
    tag_policy.insert("service".to_string(), json!(profile.tag_service_name));
    tag_policy.insert("policyType".to_string(), json!(0));
    tag_policy.insert("isEnabled".to_string(), json!(true));
    tag_policy.insert("policyPriority".to_string(), json!(0));
    tag_policy.insert("resources".to_string(), json!({"tag": {"values": [name]}}));
    tag_policy.insert(
        bucket.to_string(),
        json!([{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }]),
    );
    (
        policies,
        TagLayer {
            block: tag_block(profile, tag_def, vec![Value::Object(tag_policy)]),
            tags: vec![tag],
        },
    )
}

/// The `trie_excludes` / candidate-rule refusal, built on purpose.
///
/// Offset 553 reads `hasDeny()` and NOT the matcher, so an evaluator Ranger's
/// trie holds and this crate's matcher does not can stop the loop. Reaching it
/// needs FOUR things at once, which is why 800 generated cases produced none:
///
/// 1. `allowed` true when a non-matching evaluator is visited — and on the
///    resource path alone `allowed` only becomes true at a MATCHING evaluator,
///    which then determines. **A tag ALLOW is the only thing that seeds it**,
///    which is exactly why Task 3 could not measure this rate and this step can;
/// 2. that evaluator has no deny items, so offset 553 applies to it;
/// 3. this crate cannot place it in Ranger's candidate list — it matches nothing
///    (`provably_a_candidate` false) and `trie_excludes` cannot prove it absent,
///    which needs a level whose value is not trie-prefix-exact. `*z` is
///    `EndsWith`: the trie parks it at the ROOT and returns it for every value,
///    where the matcher rejects it;
/// 4. the verdict DIFFERS between "Ranger stopped there" and "Ranger did not",
///    which needs a later resource policy to DENY.
///
/// `sort_eval_order` puts every `hasDeny` member of a priority ahead of every
/// non-`hasDeny` one, so the unmatched no-deny policy has to sit at a STRICTLY
/// HIGHER priority than the denier or it is never reached. Both sit above the
/// tag allow's priority, or offset 505's `>` shuts them out.
///
/// A REFUSAL is the expected outcome, not a failure: it is counted on its own
/// axis and reported as a rate, which is the measurement this shape exists for.
/// A rate over a population that cannot produce the shape is not a measurement.
fn candidacy_duel(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    tag_def: &Value,
) -> (Vec<Value>, TagLayer) {
    let root = profile.hierarchy[0];
    let request_root = shape
        .request_levels
        .first()
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    // A value that CANNOT match the request's root level and that
    // `trie_reproduces_matcher` classifies as `EndsWith`, so `trie_excludes`
    // declines to prove absence. Built from the request's own value so a
    // generator change to `REQUEST_VALUES` cannot make it accidentally match.
    let unmatchable = format!("*{request_root}_no_such_suffix");

    let exact: Value = Value::Object(
        shape
            .request_levels
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    json!({"values": [if value.is_empty() || value == "*" { "*" } else { value }]}),
                )
            })
            .collect(),
    );

    let unmatched = json!({
        "id": 1, "name": "cand-a-unmatched", "service": profile.service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": 2,
        "resources": { root: {"values": [unmatchable]} },
        // No deny bucket at all: offset 553 applies only to `!hasDeny()`.
        "policyItems": [{
            "users": ["nobody_at_all"], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });
    let denier = json!({
        "id": 2, "name": "cand-b-deny", "service": profile.service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": 1,
        "resources": exact,
        "denyPolicyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });

    let name = rng.pick(TAG_NAMES).to_string();
    let match_type = *rng.pick(ADMITTED_TAG_MATCH_TYPES);
    let tag = resolved_tag(rng, profile, &name, match_type);
    let tag_policy = json!({
        "id": TAG_POLICY_ID_BASE + 91, "name": "t-candidacy",
        "service": profile.tag_service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {"tag": {"values": [name]}},
        "policyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });
    (
        vec![unmatched, denier],
        TagLayer {
            block: tag_block(profile, tag_def, vec![tag_policy]),
            tags: vec![tag],
        },
    )
}

/// The RESTORE shape: a tag DENY, a deny-capable resource ALLOW above it, and a
/// policy Ranger's trie provably does not return.
///
/// ⚠ **Added in fix round 1, RULING 1, and the reason is a measurement.** Reverting
/// T3's fix — the skip that keeps a trie-excluded view out of the three-way
/// arbitration — left the gate **GREEN at the recorded seed**. The corpus had a
/// blind spot for it: the shape needs a conjunction the random draw does not
/// produce, and the two refusals added after T3 (`EvalOrderAmbiguous` on a band,
/// and the version split) intercept most of what comes close. So the fix was
/// pinned by exactly one JVM-free unit test and by nothing the gate ran.
///
/// The three pieces, and why each is what it is:
///
/// * a tag DENY at priority 0, so the loop is in the `is_denied_by_tags` arm and
///   the restore at offsets 599-632 is what decides the case;
/// * a MATCHING resource ALLOW at priority 1 that is **deny-capable** (deny items
///   naming nobody). Deny-capable so offset 553 cannot determine for it and the
///   loop runs on; alone in its priority so T2's band refusal has no rival to
///   find;
/// * a NON-matching, NO-deny policy at priority 0 whose level value is a
///   trie-exact LITERAL — so `trie_excludes` can prove absence. Note this is the
///   OPPOSITE of what [`candidacy_duel`] needs, which wants a value
///   `trie_excludes` CANNOT prove: one shape is about what the trie provably
///   excludes and the other about what it cannot.
///
/// Expected: `Denied` by the tag policy, because nothing determines and the
/// restore puts the tag verdict back. Without T3's skip the third policy trips
/// offset 469, `determined` is set, the restore is skipped, and the answer is
/// `Allowed` — which is the divergence the review could not reproduce.
fn restore_duel(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    tag_def: &Value,
) -> (Vec<Value>, TagLayer) {
    let root = profile.hierarchy[0];
    // ⚠ The ROOT level's request value is forced CONCRETE, and that is a held-out
    // shape rather than a convenience. With an empty root value this duel lands on
    // the one row where `level_excluded_by_trie` and Ranger's trie are measured to
    // DISAGREE — see the ⚠ block on that function. The rule separating the rows is
    // not established, so the gate must not carry a standing disagreement over it;
    // the shape is recorded in `tests/ranger/README.md` with its measured rows
    // instead.
    let mut request_levels = shape.request_levels.clone();
    if let Some((_, value)) = request_levels.first_mut() {
        if value.is_empty() {
            *value = "restore_root".to_string();
        }
    }
    let exact: Value = Value::Object(
        request_levels
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    json!({"values": [if value == "*" { "*" } else { value }]}),
                )
            })
            .collect(),
    );

    let allower = json!({
        "id": 1, "name": "restore-a-allow", "service": profile.service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": 1,
        "resources": exact,
        "policyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
        // Deny items matching nobody: `hasDeny()` is true, so offset 553 cannot
        // determine and the loop must run out for the restore to happen.
        "denyPolicyItems": [{
            "users": ["nobody_at_all"], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });
    // A LITERAL the request's root level cannot be, so `trie_reproduces_matcher`
    // classifies it `Equals` and `trie_excludes` proves the policy absent.
    let excluded = json!({
        "id": 2, "name": "restore-b-trie-excluded", "service": profile.service_name,
        "policyType": 0, "isEnabled": true, "policyPriority": 0,
        "resources": { root: {"values": ["no_such_value_at_all"]} },
        "policyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });

    let name = rng.pick(TAG_NAMES).to_string();
    let match_type = *rng.pick(ADMITTED_TAG_MATCH_TYPES);
    let tag = resolved_tag(rng, profile, &name, match_type);
    let tag_policy = json!({
        "id": TAG_POLICY_ID_BASE + 92, "name": "t-restore",
        "service": profile.tag_service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {"tag": {"values": [name]}},
        "denyPolicyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });
    (
        vec![allower, excluded],
        TagLayer {
            block: tag_block(profile, tag_def, vec![tag_policy]),
            tags: vec![tag],
        },
    )
}

/// A tag DENY under a strictly higher-priority resource ALLOW that has NO deny
/// items of its own — row 1 of [`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`]'s
/// measured table, and the ONE shape the two plugin versions answer differently.
///
/// ⚠ **This exists because the refusal it reaches had 0/4000 coverage while three
/// places in the tree said the corpus guarded it** (whole-branch review). The
/// `restore_duel` is its near twin and differs in exactly one field: that one gives
/// the allower deny items matching nobody, so `hasDeny()` is true, offset 553
/// cannot determine, the loop runs out and the tag deny is RESTORED — both versions
/// then answer Denied and the duel agrees. Take the deny items away and 553
/// determines on the allow, the restore never runs, and the two versions part:
/// 2.8.0 keeps the allow (this crate's model) and 2.5.0 does not.
///
/// So the expected behaviour is asymmetric BY ENGINE, which is the property
/// [`check_version_split_is_asymmetric`] asserts:
///
/// * trino / ranger 2.5.0 → the crate REFUSES with
///   [`Declined::TagDenyVersionSplit`];
/// * starrocks / ranger 2.8.0 → the crate ANSWERS `Allowed` and is compared
///   against the real engine, which is what keeps the 2.8.0 arm honest rather
///   than merely unrefused.
///
/// The root level's request value is forced concrete for the same reason
/// [`restore_duel`] forces it — see the ⚠ note there.
fn version_split_duel(
    rng: &mut Rng,
    profile: &Profile,
    shape: &Shape,
    tag_def: &Value,
) -> (Vec<Value>, TagLayer) {
    let mut request_levels = shape.request_levels.clone();
    if let Some((_, value)) = request_levels.first_mut() {
        if value.is_empty() {
            *value = "restore_root".to_string();
        }
    }
    let exact: Value = Value::Object(
        request_levels
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    json!({"values": [if value == "*" { "*" } else { value }]}),
                )
            })
            .collect(),
    );

    // Priority 2 rather than 1 so the shape is the STRICTLY-higher one at more
    // than the minimum gap: T5's table measures both rows and they agree, and a
    // generator pinned to the smallest gap would hide a rule written as `== +1`.
    let allower = json!({
        "id": 1, "name": "vsplit-a-allow", "service": profile.service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": if rng.chance(1, 2) { 1 } else { 2 },
        "resources": exact,
        "policyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
        // NO `denyPolicyItems` and no `isDenyAllElse`: `hasDeny()` must be FALSE or
        // offset 553 cannot determine and this becomes a restore duel instead.
    });

    let name = rng.pick(TAG_NAMES).to_string();
    let match_type = *rng.pick(ADMITTED_TAG_MATCH_TYPES);
    let tag = resolved_tag(rng, profile, &name, match_type);
    let tag_policy = json!({
        "id": TAG_POLICY_ID_BASE + 93, "name": "t-vsplit",
        "service": profile.tag_service_name, "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {"tag": {"values": [name]}},
        "denyPolicyItems": [{
            "users": [shape.user], "groups": [], "roles": [],
            "accesses": [{"type": shape.access_type, "isAllowed": true}],
        }],
    });
    (
        vec![allower],
        TagLayer {
            block: tag_block(profile, tag_def, vec![tag_policy]),
            tags: vec![tag],
        },
    )
}

/// The REQUEST a [`restore_duel`] is issued for, which differs from the shape's
/// own in exactly one place — see the ⚠ note in `restore_duel`.
fn restore_duel_request(shape: &Shape) -> Vec<(String, String)> {
    let mut levels = shape.request_levels.clone();
    if let Some((_, value)) = levels.first_mut() {
        if value.is_empty() {
            *value = "restore_root".to_string();
        }
    }
    levels
}

fn generate_case(
    rng: &mut Rng,
    profile: &Profile,
    service_def: &Value,
    tag_def: &Value,
    index: usize,
) -> Case {
    let policy_type = match rng.below(4) {
        0 | 1 => 0,
        2 => 1,
        _ => 2,
    };
    let shape = build_shape(rng, profile, policy_type);

    // One ACCESS case in fourteen is the deliberate VERSION-SPLIT duel — the only
    // shape that reaches `TagDenyReopenedDiffersByPluginVersion` at all. Drawn
    // FIRST so it cannot be starved by the three draws below, and so the floor on
    // it is a floor on a draw and not on a residue.
    if policy_type == 0 && rng.chance(1, 14) {
        let (policies, tags) = version_split_duel(rng, profile, &shape, tag_def);
        let shape = Shape {
            request_levels: restore_duel_request(&shape),
            ..shape
        };
        return finish_case(Finish {
            profile,
            service_def,
            policies,
            shape,
            index,
            policy_type,
            duel: false,
            composed_duel: false,
            candidacy_duel: false,
            restore_duel: false,
            version_split_duel: true,
            tags: Some(tags),
        });
    }

    // One ACCESS case in ten is the deliberate RESTORE duel — the only shape that
    // reaches T3's fix (RULING 1).
    if policy_type == 0 && rng.chance(1, 10) {
        let (policies, tags) = restore_duel(rng, profile, &shape, tag_def);
        let shape = Shape {
            request_levels: restore_duel_request(&shape),
            ..shape
        };
        return finish_case(Finish {
            profile,
            service_def,
            policies,
            shape,
            index,
            policy_type,
            duel: false,
            composed_duel: false,
            candidacy_duel: false,
            restore_duel: true,
            version_split_duel: false,
            tags: Some(tags),
        });
    }

    // One ACCESS case in eight is the deliberate CANDIDACY duel — the only
    // shape that reaches offset 553's refusal at all.
    if policy_type == 0 && rng.chance(1, 8) {
        let (policies, tags) = candidacy_duel(rng, profile, &shape, tag_def);
        return finish_case(Finish {
            profile,
            service_def,
            policies,
            shape,
            index,
            policy_type,
            duel: false,
            composed_duel: false,
            candidacy_duel: true,
            restore_duel: false,
            version_split_duel: false,
            tags: Some(tags),
        });
    }

    // One ACCESS case in six is the deliberate COMPOSED order duel: the tag
    // phase writes a verdict and a deny-capable resource band sits strictly
    // above it. Drawn before the resource-only duel so the two are disjoint
    // populations and each rate has a denominator that means something.
    if policy_type == 0 && rng.chance(1, 6) {
        let (policies, tags) = composed_duel(rng, profile, &shape, tag_def);
        return finish_case(Finish {
            profile,
            service_def,
            policies,
            shape,
            index,
            policy_type,
            duel: true,
            composed_duel: true,
            candidacy_duel: false,
            restore_duel: false,
            version_split_duel: false,
            tags: Some(tags),
        });
    }

    // One ACCESS case in four is the deliberate order duel rather than a
    // random draw.
    if policy_type == 0 && rng.chance(1, 4) {
        let priority = if rng.chance(1, 3) { 1 } else { 0 };
        let policies = access_order_duel(rng, profile, &shape, priority);
        return finish_case(Finish {
            profile,
            service_def,
            policies,
            shape,
            index,
            policy_type,
            duel: true,
            composed_duel: false,
            candidacy_duel: false,
            restore_duel: false,
            version_split_duel: false,
            tags: None,
        });
    }

    // Two cases in five carry a tag phase. Not more: the resource-path
    // measurements this gate already reports — the `EvalOrderAmbiguous` rate
    // above all — are rates over the cases that have NO tag phase, and
    // shrinking that population would silently widen their error bars while
    // the numbers still printed.
    //
    // ⚠ **Decided BEFORE the resource policies, because a tag-carrying case
    // levels their priorities — a HELD-OUT shape with a measured reason (fix
    // round 1, finding T7).** A tag verdict plus resource ACCESS policies at two
    // priorities is a shape whose arbitration runs through the `accessTypeResults`
    // map that T4's doc already refuses to model: Ranger's `updateAccessResult`
    // writes into a FRESH per-access-type result whose `isAllowed` is false, so a
    // resource allow lands there and raises the slot's priority even when the
    // shared result was already allowed by a tag policy — and
    // `getCompositeAccessResult` then copies that priority back, shutting out
    // lower-priority evaluators this crate still runs. Measured at ranger 2.8.0,
    // corpus case `2.8.0-0848` at seed `0x54d000007`: a tag ALLOW at priority 0, an
    // `isDenyAllElse` resource policy at 1 that allows, and a resource policy at 0
    // that denies — Ranger answers `Allowed, policyId 2` and this crate, at the
    // time, answered `Denied by p1`.
    //
    // ⚠ **WHOLE-BRANCH REVIEW: this crate no longer answers it wrongly — it
    // REFUSES** ([`UndecidableCause::AccessPriorityCompositeNotModelled`]), because
    // the shape is reachable on a live engine by one supported API call and an
    // over-deny is a confident wrong answer. Two things in the paragraph that used
    // to stand here were wrong and are corrected rather than softened:
    //
    // * it said the refusal "would also refuse the four rows of T5's table that
    //   this crate reproduces exactly on 2.8.0". It does not. T5's rows are a tag
    //   DENY; this arm fires only when the tag phase ALLOWED, so the two are
    //   disjoint by construction and all three pinning unit tests still answer.
    // * it pointed at follow-up item 2. The audit is item 0.
    //
    // The LEVELLING stays, and is not a way of hiding the refusal. It is kept
    // because the resource-path rates this gate reports — `EvalOrderAmbiguous`
    // above all — are rates over cases with no tag phase, and because the random
    // composed population is what a download with no override policy looks like.
    // What the levelling costs is stated, counted and printed by the
    // `RANDOM composed ACCESS case with a resource policy above priority 0` axis instead of being left
    // for a reader to derive from two other columns. The DELIBERATE duels keep
    // their priorities, so every shape this gate was pointed at is still
    // generated.
    let tags = rng
        .chance(2, 5)
        .then(|| tag_layer(rng, profile, &shape, tag_def, policy_type));

    let policy_count = 1 + rng.below(4);
    let mut policies = Vec::new();
    for n in 0..policy_count {
        // A policy of a DIFFERENT type in the same download must be filed into
        // a different evaluator list and never reach this decision.
        let this_type = if rng.chance(1, 10) {
            (policy_type + 1 + rng.below(2) as i32) % 3
        } else {
            policy_type
        };
        // ⚠ ACCESS only. The mechanism held out below is the ACCESS loop's
        // (offsets 452-535 and the restore at 599-632); `evaluate_sub` models a
        // different walk, and levelling ITS priorities cost
        // `TagSubResultSplitByResourcePolicy` all of its corpus coverage — 19
        // cases became 0, measured. A held-out shape that silently takes a
        // refusal's only coverage with it is the silent green this file exists to
        // prevent.
        policies.push(policy(
            rng,
            profile,
            &shape,
            (n + 1) as i64,
            this_type,
            tags.is_some() && this_type == 0,
        ));
    }

    finish_case(Finish {
        profile,
        service_def,
        policies,
        shape,
        index,
        policy_type,
        duel: false,
        composed_duel: false,
        candidacy_duel: false,
        restore_duel: false,
        version_split_duel: false,
        tags,
    })
}

/// Everything [`finish_case`] needs, as one struct rather than eight
/// positional arguments: `duel` and `composed_duel` are both `bool` and
/// adjacent, and a call site that swapped them would compile and would move
/// both measured rates onto the wrong denominator.
struct Finish<'a> {
    profile: &'a Profile,
    service_def: &'a Value,
    policies: Vec<Value>,
    shape: Shape,
    index: usize,
    policy_type: i32,
    duel: bool,
    composed_duel: bool,
    candidacy_duel: bool,
    restore_duel: bool,
    version_split_duel: bool,
    tags: Option<TagLayer>,
}

fn finish_case(f: Finish<'_>) -> Case {
    let mut envelope = Map::new();
    envelope.insert("serviceName".to_string(), json!(f.profile.service_name));
    envelope.insert("serviceId".to_string(), json!(1));
    envelope.insert("policyVersion".to_string(), json!(1));
    envelope.insert("serviceDef".to_string(), f.service_def.clone());
    envelope.insert("policies".to_string(), json!(f.policies));
    // Absent, not null: `ServicePolicies.getTagPolicies()` is null for both
    // spellings and `classify_tag_policies` treats them alike, but a case with
    // no tag phase must serialise exactly as it did before Task 5 so that a
    // reported reproducer from either era still reads.
    let resolved_tags = f.tags.map(|layer| {
        envelope.insert("tagPolicies".to_string(), layer.block);
        UnenrichedTags::new(layer.tags)
    });
    let service_policies = Value::Object(envelope);

    let mut resource = RequestResource::new();
    for (name, value) in &f.shape.request_levels {
        resource.insert(name.clone(), value.clone());
    }

    Case {
        id: format!("{}-{:04}", f.profile.ranger_version, f.index),
        policy_type: f.policy_type,
        duel: f.duel,
        composed_duel: f.composed_duel,
        candidacy_duel: f.candidacy_duel,
        restore_duel: f.restore_duel,
        version_split_duel: f.version_split_duel,
        service_policies,
        request: AccessRequest {
            user: f.shape.user,
            groups: f.shape.groups,
            roles: f.shape.roles,
            access_type: f.shape.access_type,
            resource,
            // `{OWNER}` is never generated, so no decision depends on this.
            owner: None,
            resolved_tags,
        },
    }
}

// ── this crate's answer ──────────────────────────────────────────────────

/// What the Rust evaluator said, reduced to the fields the Java verdict
/// carries.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Access {
        allowed: bool,
    },
    Mask {
        mask_type: Option<String>,
        masked_value: Option<String>,
        mask_condition: Option<String>,
    },
    RowFilter {
        filter_expr: Option<String>,
    },
    /// Refused for one of the reasons the corpus generates ON PURPOSE.
    /// Counted, not compared.
    Declined(Declined),
    /// Any other refusal: a generator escape, and a hard failure.
    Refused(String),
}

/// The refusals this corpus is allowed to produce.
///
/// Each is about an ORDER or a CANDIDACY this crate does not reproduce, not
/// about a construct it does not model — the module doc draws the line and
/// says why it is drawn there. A construct refusal would mean the generator
/// had drifted into shapes the evaluator declines to answer, which is the
/// silent-green failure this whole file exists to prevent, so anything not
/// listed here is still an escape and still fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Declined {
    /// [`UndecidableCause::EvalOrderAmbiguous`].
    EvalOrder,
    /// [`UndecidableCause::ResourceCandidacyNotReproducible`] — offset 553
    /// might have stopped the loop at an evaluator this crate cannot place in
    /// Ranger's candidate list, and the two possibilities disagree.
    ResourceCandidacy,
    /// [`UndecidableCause::TagSubResultSplitByResourcePolicy`].
    TagSubResultSplit,
    /// [`UndecidableCause::TagPriorityArbitrationNotModelled`] — the tag phase's
    /// per-access-type priority arbitration, which this crate refuses rather than
    /// models. The corpus generates multi-priority tag blocks on purpose: this
    /// rate IS the over-refusal cost, and the follow-up task that models the
    /// arbitration is the one that gets to spend it.
    TagPriorityArbitration,
    /// [`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`] — the one
    /// composed shape the two plugin versions answer differently.
    ///
    /// ⚠ **WHOLE-BRANCH REVIEW: this doc used to claim the asymmetry "is itself
    /// the assertion", and NO SUCH ASSERTION EXISTED.** The two halves were equal,
    /// at 0 == 0 — the file's own stated failure condition — with no floor, and a
    /// ceiling of 10‰ over a measured 0‰ that nothing could exceed. Three places
    /// in the tree described a guard that no code performed.
    ///
    /// It is real now: [`VERSION_SPLIT_DUELS`] generates the shape, and
    /// [`check_version_split_is_asymmetric`] asserts a FLOOR on the 2.5.0 half and
    /// ZERO on the 2.8.0 half, per engine, against the counts this tally holds.
    TagDenyVersionSplit,
    /// [`UndecidableCause::AccessPriorityCompositeNotModelled`] — the tag phase
    /// allowed and a strictly higher-priority resource policy allowed too, which
    /// Ranger composites through `accessTypeResults` and this crate refuses.
    ///
    /// Admitted here rather than left to fail the run because the deliberate duels
    /// DO vary resource priorities, so the shape is reachable there on purpose. The
    /// random population cannot reach it at all — see the
    /// `RANDOM composed ACCESS case with a resource policy above priority 0` axis.
    AccessPriorityComposite,
}

impl Declined {
    fn label(self) -> &'static str {
        match self {
            Declined::EvalOrder => "EvalOrderAmbiguous",
            Declined::ResourceCandidacy => "ResourceCandidacyNotReproducible",
            Declined::TagSubResultSplit => "TagSubResultSplitByResourcePolicy",
            Declined::TagPriorityArbitration => "TagPriorityArbitrationNotModelled",
            Declined::TagDenyVersionSplit => "TagDenyReopenedDiffersByPluginVersion",
            Declined::AccessPriorityComposite => "AccessPriorityCompositeNotModelled",
        }
    }
}

/// Every [`Declined`] kind, in the order [`report_rates`] prints them and
/// [`DECLINED_CEILINGS`] bounds them. The two lists are checked against each
/// other at runtime for the same reason `AXES` and `axis_hits` are.
const DECLINED_KINDS: &[Declined] = &[
    Declined::EvalOrder,
    Declined::ResourceCandidacy,
    Declined::TagSubResultSplit,
    Declined::TagPriorityArbitration,
    Declined::TagDenyVersionSplit,
    Declined::AccessPriorityComposite,
];

/// The CEILING on each refusal, in cases per thousand of the whole corpus.
///
/// ⚠ FIX ROUND 1, IMPORTANT 1. **This file had no upper bound on the refusal rate
/// at all**, and the review proved it by injecting a 42% over-refusal: `compared`
/// fell 3490 → 2314 and the gate stayed GREEN, exit 0. `report_rates` only prints.
/// The degeneracy floors below measure against `compared`, which is right for what
/// they are about (a refused case can neither agree nor disagree) and is exactly
/// why they cannot see an over-refusal: the thing that shrinks is the denominator.
///
/// Over-refusal is the failure mode this crate is most likely to have and the one
/// the brief says "produces no failing test", so it gets an assertion rather than
/// a printed number.
///
/// ⚠ **WHOLE-BRANCH REVIEW: this doc claimed "~2× … so a doubling of any single
/// refusal fails the run", and the ceilings did not have that property.** The
/// measured multipliers were EvalOrder 2.1×, ResourceCandidacy 2.2×,
/// **TagSubResultSplit 4.0×**, **TagPriorityArbitration 3.6×** — and
/// TagDenyVersionSplit had NO multiplier at all, because it was 0‰ measured under
/// a 10‰ ceiling, so a refusal that never fired could have fired 40 times per run
/// and passed. Confirmed by injection: an exact doubling, 30 → 61 of 4000, stayed
/// GREEN.
///
/// Two things were wrong, and they are fixed differently:
///
/// * the ARITHMETIC. At exactly 2× a doubling lands ON the ceiling and the test is
///   `>`, so it passes. Every ceiling below is now **1.6×** the measured rate, so
///   an exact doubling is strictly over it. Checked for each entry against the
///   truncating integer `per_mille` the assertion actually computes, not against
///   the real-valued rate.
/// * the HONESTY about noise. The old sentence promised that seed variation could
///   not trip these. At 1.6× that promise cannot be kept for the three refusals
///   measured under ~25 cases in 4000: 1.6× is roughly two Poisson standard
///   deviations there, and `AccessPriorityComposite` at 10 cases is the tightest.
///   That is accepted rather than papered over — **at those counts a ceiling loose
///   enough that noise cannot trip it is loose enough that a real regression
///   cannot either.** On a trip the fix is to re-measure and re-record in the same
///   commit, which the failure message says.
///
/// Measured at seed `0x54d000007`, 2000 cases per engine, after the whole-branch
/// review's changes (the new `version_split_duel` draw moved every rate, which is
/// why none of these numbers match the fix-round-2 report).
const DECLINED_CEILINGS: &[(Declined, u32)] = &[
    // measured 225/4000 = 56‰ → 1.60×. A doubling is 450 = 112‰.
    (Declined::EvalOrder, 90),
    // measured 184/4000 = 46‰ → 1.59×. Bounded above anyway by the candidacy-duel
    // share of the corpus, because it fires nowhere else.
    (Declined::ResourceCandidacy, 73),
    // measured 17/4000 = 4‰ → 1.65×. A doubling is 34 = 8‰.
    (Declined::TagSubResultSplit, 7),
    // measured 23/4000 = 5‰ → 1.57×. A doubling is 46 = 11‰.
    (Declined::TagPriorityArbitration, 9),
    // measured 73/4000 = 18‰ → 1.59×, and no longer 0: `version_split_duel`
    // generates the shape, so this entry bounds something that actually fires.
    (Declined::TagDenyVersionSplit, 29),
    // measured 10/4000 = 2‰ → 1.60×, the tightest entry in the table and the one
    // most likely to flap on a seed change. See the ⚠ note above: that is the
    // trade, not an oversight.
    (Declined::AccessPriorityComposite, 4),
];

fn declined_kind(cause: &UndecidableCause) -> Option<Declined> {
    match cause {
        UndecidableCause::EvalOrderAmbiguous { .. } => Some(Declined::EvalOrder),
        UndecidableCause::ResourceCandidacyNotReproducible { .. } => {
            Some(Declined::ResourceCandidacy)
        }
        UndecidableCause::TagSubResultSplitByResourcePolicy { .. } => {
            Some(Declined::TagSubResultSplit)
        }
        UndecidableCause::TagPriorityArbitrationNotModelled { .. } => {
            Some(Declined::TagPriorityArbitration)
        }
        UndecidableCause::TagDenyReopenedDiffersByPluginVersion { .. } => {
            Some(Declined::TagDenyVersionSplit)
        }
        UndecidableCause::AccessPriorityCompositeNotModelled { .. } => {
            Some(Declined::AccessPriorityComposite)
        }
        _ => None,
    }
}

/// `Some` only when EVERY cause is one the corpus generates on purpose.
///
/// Deliberately all-or-nothing. A `why` mixing an admitted refusal with a
/// construct refusal is a generator escape wearing a disguise: reporting it as
/// the admitted one would retire the escape check for exactly the cases where
/// it matters most. The kind reported is the first cause's, and a mixed-kind
/// `why` is counted once — the case declined, whatever it declined for.
fn only_declined(why: &[UndecidableCause]) -> Option<Declined> {
    let first = declined_kind(why.first()?)?;
    why.iter()
        .all(|c| declined_kind(c).is_some())
        .then_some(first)
}

fn rust_outcome(
    case: &Case,
    set: &ServicePolicySet,
    def: &ServiceDef,
    engine: &EngineConfig,
) -> Outcome {
    match case.policy_type {
        0 => match evaluate(set, &case.request, def, engine) {
            Decision::Allowed { .. } => Outcome::Access { allowed: true },
            Decision::Denied { .. } => Outcome::Access { allowed: false },
            Decision::Undecidable { why } => match only_declined(&why) {
                Some(kind) => Outcome::Declined(kind),
                None => Outcome::Refused(format!("{why:?}")),
            },
        },
        1 => match evaluate_data_mask(set, &case.request, def, engine) {
            MaskDecision::NoMask => Outcome::Mask {
                mask_type: None,
                masked_value: None,
                mask_condition: None,
            },
            MaskDecision::Masked(m) => Outcome::Mask {
                mask_type: m.mask_type,
                masked_value: m.masked_value,
                mask_condition: m.mask_condition,
            },
            MaskDecision::Undecidable { why } => match only_declined(&why) {
                Some(kind) => Outcome::Declined(kind),
                None => Outcome::Refused(format!("{why:?}")),
            },
        },
        _ => match evaluate_row_filter(set, &case.request, def, engine) {
            RowFilterDecision::NoFilter => Outcome::RowFilter { filter_expr: None },
            RowFilterDecision::Filtered(f) => Outcome::RowFilter {
                filter_expr: Some(f.filter_expr),
            },
            RowFilterDecision::Undecidable { why } => match only_declined(&why) {
                Some(kind) => Outcome::Declined(kind),
                None => Outcome::Refused(format!("{why:?}")),
            },
        },
    }
}

/// The policy id this crate REPORTS for an ACCESS decision, or `None` for
/// "no policy".
///
/// ⚠ FIX ROUND 1, RULING 3. [`Outcome::Access`] carries only `allowed`, so until
/// now **a crate that reported the wrong policy id agreed with Ranger on all 4000
/// cases** — and the id is precisely what distinguishes T4's and T5's measured
/// rows from one another, and what `explain` puts in front of a steward. It is
/// compared on its own axis rather than folded into `Outcome`, because two of the
/// divergences are DECLARED and folding them in would either flood the gate or
/// force the comparison to be weakened; see [`ID_DIVERGENCES`].
///
/// Scoped to ACCESS, which is what the ruling asked for. The mask and row-filter
/// paths already compare their DATA, and their ids are the subject of
/// `EvalOrderAmbiguous` rather than of a separate axis.
fn rust_access_policy_id(
    case: &Case,
    set: &ServicePolicySet,
    def: &ServiceDef,
    engine: &EngineConfig,
) -> Option<Option<i64>> {
    if case.policy_type != 0 {
        return None;
    }
    match evaluate(set, &case.request, def, engine) {
        Decision::Allowed { policy_id, .. } | Decision::Denied { policy_id, .. } => Some(policy_id),
        Decision::Undecidable { .. } => None,
    }
}

/// Ranger's reported ACCESS policy id, with its "none" spelling normalised.
///
/// `RangerAccessResult`'s `policyId` starts at **-1** and only a writing
/// evaluator moves it, so -1 is Ranger's `None`. Reading it as an id would make
/// every no-policy-matched case a mismatch against the crate's `None`.
fn java_access_policy_id(verdict: &Value) -> Option<i64> {
    match verdict.get("policyId").and_then(Value::as_i64) {
        Some(id) if id >= 0 => Some(id),
        _ => None,
    }
}

/// One policy of a generated case, as the id comparison needs to see it.
struct PolicyBand {
    /// `id >= TAG_POLICY_ID_BASE`, i.e. the policy is in the `tagPolicies` block.
    tag: bool,
    priority: i64,
    has_deny: bool,
    /// A non-empty `policyItems`, i.e. the policy is capable of ALLOWING.
    ///
    /// Needed by [`classify_id_divergence`]'s restore arm: the shape it declares
    /// has Ranger naming a policy that ALLOWED and was then discarded, and
    /// "capable of allowing" is as close as a structural test gets to that.
    has_allow: bool,
}

/// Find a generated policy by id, in either list.
///
/// The id ranges are disjoint by construction ([`TAG_POLICY_ID_BASE`]) so a
/// single lookup cannot confuse the two halves — which is the whole reason that
/// constant exists.
fn policy_band(case: &Case, id: i64) -> Option<PolicyBand> {
    let lists = [
        case.service_policies.get("policies"),
        case.service_policies
            .get("tagPolicies")
            .and_then(|b| b.get("policies")),
    ];
    for list in lists.into_iter().flatten() {
        for policy in list.as_array().into_iter().flatten() {
            if policy["id"].as_i64() != Some(id) {
                continue;
            }
            return Some(PolicyBand {
                tag: id >= TAG_POLICY_ID_BASE,
                priority: policy["policyPriority"].as_i64().unwrap_or(0),
                has_deny: policy["isDenyAllElse"].as_bool() == Some(true)
                    || policy["denyPolicyItems"]
                        .as_array()
                        .is_some_and(|b| !b.is_empty()),
                has_allow: policy["policyItems"]
                    .as_array()
                    .is_some_and(|b| !b.is_empty()),
            });
        }
    }
    None
}

/// The DECLARED reasons an ACCESS policy id may differ, each with the place it is
/// documented.
///
/// ⚠ This is an ALLOW-LIST, not a relaxation: a mismatch that matches none of
/// these FAILS the run with a reproducer. The ruling is explicit that the
/// comparison must not be weakened to accommodate the known divergences, so each
/// arm is a structural test on the two policies the two sides named, not a
/// blanket "ids may differ".
const ID_DIVERGENCES: &[(&str, &str, u32)] = &[
    (
        "same (policyPriority, hasDeny) band — getEvalOrder() not reproduced",
        "tests/ranger/README.md, \"Declared divergences from the real engine\"; \
         eval.rs's sort_eval_order",
        // measured 47/4000 = 11‰ after the whole-branch review's changes (37/4000 = 9‰
        // before them). The floor is half the measured rate, as everywhere else here.
        5,
    ),
    (
        "tag deny restored over a resource allow — setIsAllowed(false) keeps the last writer's id",
        "eval.rs's restore at offsets 599-632; the fixture is \
         tag_arbitration.rs::nothing_else_determines_so_the_tag_verdict_is_restored",
        // measured 90/4000 = 22‰ after the whole-branch review's changes (102/4000 = 25‰
        // before them — the new `version_split_duel` draw takes cases that would
        // otherwise have been restore duels).
        11,
    ),
];

/// Classify an ACCESS policy-id mismatch, or `None` if it is a NEW divergence.
fn classify_id_divergence(
    case: &Case,
    mine: Option<i64>,
    theirs: Option<i64>,
) -> Option<&'static str> {
    let (Some(mine), Some(theirs)) = (mine, theirs) else {
        // One side named no policy at all and the other named one. That is not any
        // of the declared shapes — both of them are two policies disagreeing about
        // WHICH decided, not about WHETHER one did — so it fails.
        return None;
    };
    let (Some(a), Some(b)) = (policy_band(case, mine), policy_band(case, theirs)) else {
        // An id neither list carries: the gate cannot reason about it and must not
        // pretend to.
        return None;
    };
    // The cross-phase restore at offsets 599-632: the crate names the TAG policy
    // that denied and Ranger names the RESOURCE policy that allowed and was then
    // discarded, because `setIsAllowed(false)` does not touch `policyId`.
    //
    // ⚠ FIX ROUND 2. This arm was `if a.tag != b.tag`, i.e. "the two ids are in
    // different blocks" — which is not the restore, it is every cross-block
    // mismatch, and the re-review built a measured instance of a DIFFERENT
    // mechanism that it absorbed: a tag ALLOW at priority 0, a plain resource
    // allower at 1 and a denier at 0, where ranger 2.8.0 answers
    // `Allowed, policyId 2` and this crate answers `Allowed, policyId 901` with
    // **no tag deny anywhere in the case**. That is T7's `accessTypeResults`
    // composite, the mechanism this package is reporting as UNMODELLED, filed
    // under the one it has modelled.
    //
    // Splitting the arm and re-running the whole gate fired the split label 0
    // times, so nothing was mis-filed — the gap was latent. It is narrowed anyway,
    // because **an allow-list that can absorb an unknown mechanism is worse than
    // no allow-list**: it converts the one class of finding the axis exists to
    // catch into a counted, declared row.
    //
    // All four of the restore's conditions, in the direction the shape has them:
    if a.tag && !b.tag && a.has_deny && b.has_allow && b.priority > a.priority {
        return Some(ID_DIVERGENCES[1].0);
    }
    // One `(priority, hasDeny)` band, one list: `getEvalOrder()` decides which
    // member Ranger reached first and this crate does not compute it.
    if a.priority == b.priority && a.has_deny == b.has_deny {
        return Some(ID_DIVERGENCES[0].0);
    }
    None
}

/// Ranger's answer, in the same shape.
fn java_outcome(policy_type: i32, verdict: &Value) -> Outcome {
    let string = |key: &str| -> Option<String> {
        verdict.get(key).and_then(Value::as_str).map(str::to_string)
    };
    match policy_type {
        0 => Outcome::Access {
            allowed: verdict
                .get("isAllowed")
                .and_then(Value::as_bool)
                .expect("verdict.isAllowed"),
        },
        1 => Outcome::Mask {
            mask_type: string("maskType"),
            masked_value: string("maskedValue"),
            mask_condition: string("maskCondition"),
        },
        _ => Outcome::RowFilter {
            filter_expr: string("filterExpr"),
        },
    }
}

// ── coverage: what the corpus actually PRODUCED ──────────────────────────

/// A structural property of a generated case, counted on every run.
///
/// The gate's weakest point, once it is green, is that "green" and "still
/// testing the shape" become indistinguishable. A generator edit that quietly
/// stops emitting `isExcludes` costs nothing visible: the corpus stays green,
/// because agreement on cases that never exercise a rule is agreement about
/// nothing. That is a silent, green failure — the worst kind, and the kind
/// this whole package exists to eliminate elsewhere.
///
/// So every axis the plan names, and every axis a finding added, has a
/// counter here and a floor that FAILS the run. The predicates are
/// structural: they read the generated JSON and the request, never the
/// outcome, so a counter cannot be satisfied by a decision that happened to
/// come out a certain way.
struct Axis {
    name: &'static str,
    /// Floor in cases per thousand, so it scales with `EGRET_DIFF_CASES`
    /// instead of breaking the moment someone changes the corpus size.
    ///
    /// Each is set to roughly half of what the corpus measurably produces, so
    /// ordinary seed variation cannot trip it. A floor that flaps gets
    /// deleted by the next person, which is worse than no floor at all.
    min_per_mille: u32,
    /// `Some` when the axis only exists for one engine — Trino's `all` is the
    /// only `impliedGrants` entry either service-def declares, so flooring it
    /// for StarRocks would fail on a corpus that is perfectly healthy.
    only: Option<EngineType>,
}

/// The axes, in the order [`axis_hits`] reports them. The two lists are
/// checked against each other by name at runtime, because a silent
/// misalignment here would attach every floor to the wrong counter — the same
/// class of defect the table exists to catch.
const AXES: &[Axis] = &[
    // — policy types —
    Axis {
        name: "policyType 0 (access)",
        min_per_mille: 250,
        only: None,
    },
    Axis {
        name: "policyType 1 (data mask)",
        min_per_mille: 120,
        only: None,
    },
    Axis {
        name: "policyType 2 (row filter)",
        min_per_mille: 120,
        only: None,
    },
    Axis {
        name: "row-filter case at TABLE granularity",
        min_per_mille: 120,
        only: None,
    },
    Axis {
        name: "policy of a DIFFERENT type in the set",
        min_per_mille: 80,
        only: None,
    },
    // — resource shapes —
    Axis {
        name: "ANCESTOR-shaped policy (fewer levels than request)",
        min_per_mille: 60,
        only: None,
    },
    Axis {
        name: "policy DEEPER than the request",
        min_per_mille: 80,
        only: None,
    },
    Axis {
        name: "isExcludes",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "multi-value level",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "level with no usable values",
        min_per_mille: 40,
        only: None,
    },
    Axis {
        name: "additionalResources",
        min_per_mille: 100,
        only: None,
    },
    // — wildcards, by position —
    Axis {
        name: "wildcard: leading *",
        min_per_mille: 200,
        only: None,
    },
    Axis {
        name: "wildcard: trailing *",
        min_per_mille: 200,
        only: None,
    },
    Axis {
        name: "wildcard: interior *",
        min_per_mille: 150,
        only: None,
    },
    Axis {
        name: "wildcard: repeated *",
        min_per_mille: 150,
        only: None,
    },
    Axis {
        name: "wildcard: leading ?",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "wildcard: trailing ?",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "wildcard: interior ?",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "wildcard: repeated ?",
        min_per_mille: 50,
        only: None,
    },
    Axis {
        name: "wildcard: `*` immediately before `?` (F1)",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "wildcard: literal AFTER a `*` (backtracking reachable)",
        min_per_mille: 100,
        only: None,
    },
    Axis {
        name: "policy value that is only `*`",
        min_per_mille: 200,
        only: None,
    },
    Axis {
        name: "policy value differing from the request only by case",
        min_per_mille: 100,
        only: None,
    },
    // — request side —
    Axis {
        name: "request value `*`",
        min_per_mille: 50,
        only: None,
    },
    Axis {
        name: "request value empty",
        min_per_mille: 55,
        only: None,
    },
    Axis {
        name: "request with a gap in the hierarchy",
        min_per_mille: 30,
        only: None,
    },
    // — principals and buckets —
    Axis {
        name: "bucket policyItems",
        min_per_mille: 280,
        only: None,
    },
    Axis {
        name: "bucket denyPolicyItems",
        min_per_mille: 150,
        only: None,
    },
    Axis {
        name: "bucket allowExceptions",
        min_per_mille: 60,
        only: None,
    },
    Axis {
        name: "bucket denyExceptions",
        min_per_mille: 60,
        only: None,
    },
    Axis {
        name: "group `public`",
        min_per_mille: 80,
        only: None,
    },
    Axis {
        name: "user `{USER}`",
        min_per_mille: 80,
        only: None,
    },
    Axis {
        name: "policy item naming a role",
        min_per_mille: 60,
        only: None,
    },
    // — policy-level switches —
    Axis {
        name: "policyPriority override",
        min_per_mille: 140,
        only: None,
    },
    Axis {
        name: "isDenyAllElse",
        min_per_mille: 40,
        only: None,
    },
    Axis {
        name: "disabled policy",
        min_per_mille: 70,
        only: None,
    },
    Axis {
        name: "impliedGrants via trino `all`",
        min_per_mille: 60,
        only: Some(EngineType::Trino),
    },
    Axis {
        name: "access entry with isAllowed:false",
        min_per_mille: 150,
        only: None,
    },
    Axis {
        name: "access entry whose type reads as `\"\"` (F3)",
        min_per_mille: 40,
        only: None,
    },
    // — mask / row-filter payloads —
    Axis {
        name: "dataMask item with NO dataMaskInfo (F2)",
        min_per_mille: 40,
        only: None,
    },
    Axis {
        name: "rowFilter item with NO rowFilterInfo",
        min_per_mille: 20,
        only: None,
    },
    Axis {
        name: "CUSTOM mask carrying a valueExpr",
        min_per_mille: 40,
        only: None,
    },
    Axis {
        name: "mask conditionExpr",
        min_per_mille: 20,
        only: None,
    },
    Axis {
        name: "filterExpr explicitly empty or null",
        min_per_mille: 40,
        only: None,
    },
    // — the deliberate shape —
    Axis {
        name: "eval-order duel",
        min_per_mille: 55,
        only: None,
    },
    // — the tag phase (SP4d-2b Task 5) —
    //
    // Every floor here counts cases that REACH the axis, not cases generated
    // with the ingredient: "a tag the block can reach" needs an ADMITTED tag
    // AND a policy of THIS case's kind whose `resources.tag` covers it, because
    // a tag nothing evaluates is not coverage of the tag phase. That
    // distinction has already produced a false green in this package.
    //
    // ⚠ FIX ROUND 1, IMPORTANT 3 and FIX ROUND 2: that sentence was not true of
    // this block when it was written. Round 1 narrowed six axes (the five
    // match-type ones and the prune); round 2 found the prune still missing the
    // ADMITTED filter its siblings had, and about ten more axes counting pure
    // ingredient underneath the claim. **Every tag axis is now gated on a policy of
    // the case's kind ADMITTING a tag the request carries**, with exactly two
    // stated exceptions:
    //
    // * the five MATCH-TYPE axes gate on COVERAGE, not admission — `DESCENDANT`
    //   and `NONE` are axes precisely because they are not admitted, so gating them
    //   on admission would leave them counting nothing;
    // * `tag: empty resolved set` is a property of the request alone, because an
    //   empty set has no tag for any policy to cover and its whole content is that
    //   the phase is narrowed away before a policy is consulted.
    //
    // What these floors guard is CANDIDACY, and that is intrinsic: the rule two
    // blocks down is that `axis_hits` reads no DECISION, so "the answer would have
    // flipped" is not expressible here. The gap is quantified rather than left
    // implicit — Step 4's `DESCENDANT` mutation flips 6 of 1743 compared cases
    // (3‰) against a 14‰ candidacy floor, so roughly four in five cases that
    // satisfy that floor are candidates whose items do not decide. The mutation is
    // what guards the flip; the floor is what guards the candidacy. Neither
    // substitutes for the other and the file says so in both places.
    //
    // ⚠ The DENOMINATOR here is `cases`, not `compared`, and that is deliberate
    // (fix round 2). These floors guard the GENERATOR — `report_coverage` runs
    // before a JVM starts, precisely so a starved axis is reported before anything
    // downstream means what it says — and a case the crate later declines still
    // exercised the generator. The degeneracy floors at the bottom of this file use
    // `compared` for the opposite reason: they are about the population that can
    // agree or disagree. What bounds the overlap is the compared-FRACTION ceiling,
    // which caps the declined share at 25%, so an axis floor can at worst be
    // satisfied by a quarter of cases the gate did not compare.
    Axis {
        name: "tag: tagPolicies block present",
        min_per_mille: 232,
        only: None,
    },
    Axis {
        name: "tag: a tag the block's policies REACH",
        min_per_mille: 179,
        only: None,
    },
    Axis {
        name: "tag: a tag the block's policies do NOT reach",
        min_per_mille: 80,
        only: None,
    },
    Axis {
        name: "tag: match SELF",
        min_per_mille: 71,
        only: None,
    },
    Axis {
        name: "tag: match SELF_AND_ALL_DESCENDANTS",
        min_per_mille: 68,
        only: None,
    },
    Axis {
        name: "tag: match ANCESTOR",
        min_per_mille: 75,
        only: None,
    },
    Axis {
        name: "tag: match DESCENDANT (NOT admitted)",
        min_per_mille: 11,
        only: None,
    },
    Axis {
        name: "tag: match NONE (NOT admitted)",
        min_per_mille: 12,
        only: None,
    },
    Axis {
        name: "tag: empty resolved set (the narrowing)",
        min_per_mille: 20,
        only: None,
    },
    Axis {
        name: "tag: same NAME at two match types",
        min_per_mille: 32,
        only: None,
    },
    Axis {
        name: "tag: attachment carrying attributes",
        min_per_mille: 54,
        only: None,
    },
    Axis {
        name: "tag: deny-capable ACCESS policy",
        min_per_mille: 70,
        only: None,
    },
    // ⚠ The THINNEST floor in this table (measured 6‰, floor 3‰ = 12 cases in
    // 4000), because the shape needs two deny items on a tag policy that ALSO
    // admits a carried tag. It is kept rather than raised by generating more deny
    // items, which would cost the corpus its ALLOW outcomes — the trade the round-1
    // report records. It still catches the thing it is for: the draw going back to
    // at-most-one.
    Axis {
        name: "tag: ACCESS policy with 2+ deny items",
        min_per_mille: 3,
        only: None,
    },
    Axis {
        name: "tag: MASK policy of this kind",
        min_per_mille: 31,
        only: None,
    },
    Axis {
        name: "tag: ROW FILTER policy of this kind",
        min_per_mille: 31,
        only: None,
    },
    Axis {
        name: "tag: several policies of this kind at ONE priority",
        min_per_mille: 35,
        only: None,
    },
    Axis {
        name: "tag: policyPriority override",
        min_per_mille: 29,
        only: None,
    },
    Axis {
        name: "tag: policy of a DIFFERENT type in the block",
        min_per_mille: 30,
        only: None,
    },
    Axis {
        name: "tag: policy value is a prefix wildcard",
        min_per_mille: 86,
        only: None,
    },
    Axis {
        name: "tag: value differing from a resolved tag only by CASE",
        min_per_mille: 20,
        only: None,
    },
    Axis {
        name: "tag: composed eval-order duel",
        min_per_mille: 31,
        only: None,
    },
    // The prune, and its asymmetry. Three axes rather than one: a corpus that
    // only produced the flag would leave the prune untested, and one that only
    // produced the tag half would let a crate that pruned BOTH paths look
    // right.
    Axis {
        name: "tag: restore duel (T3's skip reachable)",
        // measured 187/4000 = 46‰. A real floor, because `min_per_mille: 0`
        // asserts nothing and the two floors in the test body are about the shape
        // AGREEING, not about it being generated (fix round 2).
        min_per_mille: 19,
        only: None,
    },
    Axis {
        name: "tag: version-split duel (T5's 2.5.0 arm reachable)",
        // Drawn at 1 in 14 of the ACCESS cases; floor at roughly half the measured
        // share. A floor rather than a printed count, because unlike the hold-out
        // axis below this shape IS supposed to be generated.
        min_per_mille: 16,
        only: None,
    },
    Axis {
        name: "tag: candidacy duel (offset 553 reachable)",
        min_per_mille: 23,
        only: None,
    },
    Axis {
        name: "tag: audit-disabled policy with NO items (pruned)",
        min_per_mille: 11,
        only: None,
    },
    Axis {
        name: "tag: audit-disabled policy WITH items (kept)",
        min_per_mille: 31,
        only: None,
    },
    Axis {
        name: "resource policy audit-disabled with NO items (NOT pruned)",
        min_per_mille: 21,
        only: None,
    },
    // ⚠ WHOLE-BRANCH REVIEW. A floor of 0 that EXISTS, for the one conjunction
    // this corpus deliberately does not generate.
    //
    // `policy`'s `level_priority` forces `policyPriority: 0` on every resource
    // policy of a random tag-carrying case — a held-out shape with a measured
    // reason (see [`generate_case`]). The consequence is not just that the shape
    // is absent: with every resource policy at 0, a tag DENY determines before any
    // resource evaluator runs and a tag ALLOW stands, so **the three-way
    // arbitration at offsets 469/505 is exercised against a resource evaluator
    // only in the `composed duel` column.** That is visible in the printed rates
    // (`COMPOSED RANDOM` EvalOrder 1.91% / ResourceCandidacy 0.00% against the
    // duel column's 16.54% / 29.52%) but it was only INFERABLE — nothing counted
    // the conjunction, so nothing in the gate could state the hole. `policyPriority
    // override` counts it over all cases (including the three in five with no tag
    // phase) and `tag: policyPriority override` counts the TAG side; neither is
    // this.
    //
    // ⚠ The scoping took TWO measurements to get right, and both wrong versions
    // read as healthy coverage of the shape that has none — the
    // `A correlated oracle is not an oracle` failure one step removed.
    //
    // * the bare conjunction counted 486 of 2000 = 243‰: every deliberate duel, all
    //   four kinds of which build a tag block over a resource policy at priority 1
    //   on purpose. Scoped to `Population::Composed`;
    // * still 123 of 2000 = 61‰, because the levelling in [`generate_case`] is
    //   **ACCESS only** (levelling the mask and row-filter priorities cost
    //   `TagSubResultSplitByResourcePolicy` all of its coverage, 19 cases to 0), so
    //   the mask and row-filter policies of a random tag-carrying case still carry
    //   priority 1. They are not in the ACCESS loop and not in this hole.
    //
    // So the axis is the ACCESS conjunction in the random composed population,
    // which is exactly the arbitration at offsets 469/505.
    //
    // The floor is 0 ON PURPOSE and must stay 0 while the levelling stands: the
    // axis exists so the COUNT is printed beside the rates it explains. If someone
    // un-levels [`policy`], this number becomes non-zero and the floor should then
    // be re-measured — it is the tripwire on the hold-out, not a coverage demand.
    // Same remedy the file already applies to `ID_DIVERGENCES`: declare the thing,
    // print its count, and let a change in it be visible.
    Axis {
        name: "RANDOM composed ACCESS case with a resource policy above priority 0",
        min_per_mille: 0,
        only: None,
    },
];

/// Does a tag policy value cover `name`, under `tag.json`'s
/// `{ wildCard: true, ignoreCase: FALSE }`?
///
/// Only the three shapes [`TAG_POLICY_VALUES`] contains need handling, because
/// they are the only ones generated. **This is a coverage predicate, never an
/// oracle**: nothing asserts on it, and the decisions it helps count are taken
/// by the crate and by Ranger. It exists so that "a tag the block reaches" is a
/// structural property of the generated JSON rather than a property of the
/// answer that came back.
fn tag_value_covers(value: &str, name: &str) -> bool {
    if !value.is_empty() && value.chars().all(|c| c == '*') {
        return true;
    }
    match value.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => value == name,
    }
}

/// Are all SIX item buckets empty — the half of `isPolicyNeedsPruning` that is
/// not `isAuditEnabled`?
///
/// `isDenyAllElse` is deliberately NOT consulted, because Ranger does not
/// consult it either: that omission is exactly what makes the prune observable
/// rather than vacuous.
fn all_item_buckets_empty(policy: &Value) -> bool {
    [
        "policyItems",
        "denyPolicyItems",
        "allowExceptions",
        "denyExceptions",
        "dataMaskPolicyItems",
        "rowFilterPolicyItems",
    ]
    .iter()
    .all(|bucket| {
        policy[*bucket]
            .as_array()
            .is_none_or(|items| items.is_empty())
    })
}

/// Everything the tag axes count, read off the generated JSON and the request.
#[derive(Default)]
struct TagFacts {
    block_present: bool,
    reached: bool,
    unreached: bool,
    /// Indexed by [`TAG_MATCH_TYPES`].
    ///
    /// ⚠ FIX ROUND 1, IMPORTANT 3. These used to be set from the resolved tag set
    /// ALONE — the presence of the ingredient — while the header of the axis table
    /// claims every floor counts cases that REACH the axis. They now require that
    /// the tag's NAME is covered by an enabled policy of THIS case's kind, which
    /// is `reached`'s predicate with the admitted-type filter removed. A tag no
    /// tag policy addresses cannot exercise the admission rule, whichever match
    /// type it carries.
    ///
    /// What this still is NOT is "the case where admitting this match type would
    /// CHANGE the answer". That needs the policy's items to decide too, and
    /// `axis_hits` reads no decision by design. The review measured the gap on the
    /// sharpest one: the DESCENDANT mutation disagrees in **6 of 1743 compared
    /// cases (3‰)** where this axis counts candidacy. So the floor guards
    /// candidacy and the Step 4 mutation guards the flip; neither substitutes for
    /// the other, and saying so is the point of this block.
    match_types: [bool; 5],
    empty_set: bool,
    repeated_name: bool,
    attributes: bool,
    deny_capable_access: bool,
    mask_policy: bool,
    row_filter_policy: bool,
    several_at_one_priority: bool,
    priority_override: bool,
    other_type: bool,
    prefix_value: bool,
    case_only_value: bool,
    /// `isAuditEnabled: false` with every item bucket empty, `isDenyAllElse` set,
    /// and a tag value that covers a resolved tag — the shape the TAG repository
    /// PRUNES and `policies` does not.
    ///
    /// ⚠ FIX ROUND 1, IMPORTANT 3. This used to test the flag and the six empty
    /// buckets only. Without `isDenyAllElse` the pruned policy decides nothing
    /// either way and the prune is unobservable; without a covering value nothing
    /// reaches it at all. Both are what make the axis about the prune rather than
    /// about the flag.
    audit_pruned: bool,
    /// `isAuditEnabled: false` with items present — the shape it KEEPS. Counted
    /// apart so that "the flag appears" and "the prune fires" cannot be read off
    /// one number.
    audit_kept: bool,
    /// A tag ACCESS policy of this kind with TWO OR MORE deny items.
    ///
    /// FIX ROUND 1, MINOR 8. The deny draw was cut to at most one item to stop the
    /// tag layer starving the corpus of allows, which removed this shape with
    /// nothing recording the removal. The draw now yields 0, 1 or 2 and this axis
    /// is what stops it silently going back to 0-or-1.
    multi_deny_access: bool,
}

fn tag_facts(case: &Case) -> TagFacts {
    let mut facts = TagFacts::default();
    let Some(block) = case.service_policies.get("tagPolicies") else {
        return facts;
    };
    facts.block_present = true;

    let tags = case
        .request
        .resolved_tags
        .as_ref()
        .map(UnenrichedTags::as_slice)
        .unwrap_or_default();
    // The ONE axis that is a property of the request alone, and intrinsically so:
    // an empty tag set has no tag for any policy to cover, and its whole content is
    // that `getLikelyMatchPolicyEvaluators` returns `EMPTY_LIST` and the phase is
    // narrowed away before a policy is consulted. Every other tag axis is gated on
    // a policy of the kind ADMITTING a carried tag — see the `admits` block below.
    facts.empty_set = tags.is_empty();

    let policies = block["policies"].as_array().cloned().unwrap_or_default();
    let mut priorities_of_kind: Vec<i64> = Vec::new();
    for policy in &policies {
        if policy["isEnabled"].as_bool() == Some(false) {
            continue;
        }
        let policy_type = policy["policyType"].as_i64().unwrap_or(0);
        if policy_type != i64::from(case.policy_type) {
            facts.other_type = true;
            // Filed into a different evaluator list in the tag repository, so
            // nothing about it is coverage of THIS decision — the same rule the
            // resource loop above follows.
            continue;
        }
        let values: Vec<&str> = policy["resources"]["tag"]["values"]
            .as_array()
            .map(|v| v.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        // ⚠ FIX ROUND 2. Every fact below that is a property of THIS POLICY is
        // gated on `admits`, and the two words are not interchangeable:
        //
        // * `covers` — some tag the request carries has a name this policy's
        //   `resources.tag` matches, i.e. the trie would return it;
        // * `admits` — and that tag's match type is one
        //   `RangerDefaultPolicyEvaluator.evaluate` lets reach the policy items.
        //
        // The prune axis used `covers` alone, which is the gap the other five were
        // fixed for one round earlier: a case whose only covering tag is
        // `DESCENDANT` satisfied an 11‰ floor while `evaluate_tag_policies` admits
        // nothing and the prune cannot be observed. The generator makes that
        // reachable, because the pruned policy's value is overwritten with
        // `names.first()` and `names` includes non-admitted tags.
        //
        // The MATCH-TYPE axes below are the deliberate exception and use `covers`:
        // `DESCENDANT` and `NONE` exist as axes precisely because they are NOT
        // admitted, so gating them on admission would leave them counting nothing.
        let admits = tags.iter().any(|tag| {
            ADMITTED_TAG_MATCH_TYPES.contains(&tag.match_type)
                && values.iter().any(|v| tag_value_covers(v, &tag.name))
        });

        if admits {
            if policy["policyPriority"].as_i64().unwrap_or(0) != 0 {
                facts.priority_override = true;
            }
            priorities_of_kind.push(policy["policyPriority"].as_i64().unwrap_or(0));
        }
        if policy["isAuditEnabled"].as_bool() == Some(false) {
            if all_item_buckets_empty(policy) {
                // `isDenyAllElse` and an ADMITTED tag are both required: without the
                // first the prune changes nothing, without the second nothing
                // reaches the policy at all.
                if policy["isDenyAllElse"].as_bool() == Some(true) && admits {
                    facts.audit_pruned = true;
                }
            } else if admits {
                facts.audit_kept = true;
            }
        }
        if admits {
            match policy_type {
                1 => facts.mask_policy = true,
                2 => facts.row_filter_policy = true,
                _ => {
                    let deny_items = policy["denyPolicyItems"]
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or(0);
                    if policy["isDenyAllElse"].as_bool() == Some(true) || deny_items > 0 {
                        facts.deny_capable_access = true;
                    }
                    if deny_items >= 2 {
                        facts.multi_deny_access = true;
                    }
                }
            }
            // Properties of the request's tag SET, recorded only once some policy
            // of the kind admits one of them — otherwise the tag phase never looks
            // at the set at all. `empty_set` is the one exception and is set above,
            // because an empty set has no tag for any policy to admit and its whole
            // content is that the phase is narrowed away.
            for tag in tags {
                if !ADMITTED_TAG_MATCH_TYPES.contains(&tag.match_type) {
                    continue;
                }
                if !values.iter().any(|v| tag_value_covers(v, &tag.name)) {
                    continue;
                }
                if !tag.attributes.is_empty() {
                    facts.attributes = true;
                }
                if tags
                    .iter()
                    .any(|other| other.name == tag.name && other.match_type != tag.match_type)
                {
                    facts.repeated_name = true;
                }
            }
        }
        for value in &values {
            if value.len() > 1 && value.ends_with('*') && !value.ends_with("**") {
                facts.prefix_value = true;
            }
            for tag in tags {
                // `ignoreCase: false` on the tag def against `true` on every
                // resource level: the one place in this corpus where a pure
                // case difference is a DIFFERENT value rather than the same one.
                if !tag_value_covers(value, &tag.name)
                    && value.eq_ignore_ascii_case(&tag.name)
                    && *value != tag.name
                {
                    facts.case_only_value = true;
                }
            }
        }
        for tag in tags {
            let covered = values.iter().any(|v| tag_value_covers(v, &tag.name));
            // The match-type axes, at CANDIDACY granularity — see the field's doc.
            // Outside the `ADMITTED` filter deliberately: `DESCENDANT` and `NONE`
            // exist as axes precisely because they are NOT admitted, so filtering
            // them out here would leave them counting nothing.
            if covered {
                if let Some(index) = TAG_MATCH_TYPES.iter().position(|m| *m == tag.match_type) {
                    facts.match_types[index] = true;
                }
            }
            if !ADMITTED_TAG_MATCH_TYPES.contains(&tag.match_type) {
                continue;
            }
            if covered {
                facts.reached = true;
            } else {
                facts.unreached = true;
            }
        }
    }
    priorities_of_kind.sort_unstable();
    facts.several_at_one_priority = priorities_of_kind.windows(2).any(|pair| pair[0] == pair[1]);
    facts
}

/// Does `value` carry a wildcard in this position?
fn wildcard_positions(value: &str, hits: &mut [bool; 9]) {
    let chars: Vec<char> = value.chars().collect();
    let stars = chars.iter().filter(|c| **c == '*').count();
    let questions = chars.iter().filter(|c| **c == '?').count();
    let only_stars = !chars.is_empty() && chars.iter().all(|c| *c == '*');
    for (i, ch) in chars.iter().enumerate() {
        let leading = i == 0;
        let trailing = i + 1 == chars.len();
        match ch {
            // A value that is ONLY `*` is match-any and is counted on its own
            // axis; it would otherwise dominate every positional counter.
            '*' if !only_stars => {
                if leading {
                    hits[0] = true;
                } else if trailing {
                    hits[1] = true;
                } else {
                    hits[2] = true;
                }
            }
            '?' => {
                if leading {
                    hits[4] = true;
                } else if trailing {
                    hits[5] = true;
                } else {
                    hits[6] = true;
                }
            }
            _ => {}
        }
    }
    if stars >= 2 && !only_stars {
        hits[3] = true;
    }
    if questions >= 2 {
        hits[7] = true;
    }
    if only_stars {
        hits[8] = true;
    }
}

/// Every axis this case exercises, as `(name, hit)` in `AXES` order.
///
/// Reads the generated JSON and the request only. Nothing here consults a
/// decision: an axis must be satisfied by the case CONTAINING the shape, not
/// by the shape happening to change an answer.
fn axis_hits(case: &Case) -> Vec<(&'static str, bool)> {
    let policies = case.service_policies["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let request_levels: Vec<(&str, &str)> = case
        .request
        .resource
        .keys()
        .map(|k| (k, case.request.resource.get(k).unwrap_or("")))
        .collect();
    let request_depth = request_levels.len();

    let mut ancestor = false;
    let mut deeper = false;
    let mut excludes = false;
    let mut multi_value = false;
    let mut empty_values = false;
    let mut additional = false;
    let mut wc = [false; 9];
    let mut case_only_difference = false;
    let mut buckets = [false; 4];
    let mut public_group = false;
    let mut user_current = false;
    let mut role_named = false;
    let mut priority_override = false;
    let mut deny_all_else = false;
    let mut disabled = false;
    let mut implied_all = false;
    let mut explicit_deny = false;
    let mut empty_access_type = false;
    let mut mask_without_info = false;
    let mut filter_without_info = false;
    let mut custom_value_expr = false;
    let mut condition_expr = false;
    let mut filter_expr_blank = false;
    let mut other_policy_type = false;
    let mut star_then_question = false;
    // The ASYMMETRY: the same shape the TAG repository prunes, in `policies`,
    // where no constructor normalises and Ranger therefore KEEPS it.
    let mut resource_audit_pruneable = false;
    // `checkIndexOf` and its resume point are reached only when a LITERAL
    // token follows a `*`. A value ending in `*` never gets there, which is
    // exactly how the corpus stayed green with the resume point deleted.
    let mut literal_after_star = false;

    for policy in &policies {
        if policy["policyType"].as_i64() != Some(i64::from(case.policy_type)) {
            other_policy_type = true;
            // A policy of another type is filed into a different evaluator
            // list and cannot reach this decision, so its contents are not
            // coverage of anything.
            continue;
        }
        if policy["isEnabled"].as_bool() == Some(false) {
            disabled = true;
        }
        if policy["policyPriority"].as_i64().unwrap_or(0) != 0 {
            priority_override = true;
        }
        if policy["isDenyAllElse"].as_bool() == Some(true) {
            deny_all_else = true;
        }
        if policy["isAuditEnabled"].as_bool() == Some(false) && all_item_buckets_empty(policy) {
            resource_audit_pruneable = true;
        }

        let mut maps = vec![&policy["resources"]];
        if let Some(extra) = policy["additionalResources"].as_array() {
            additional = true;
            maps.extend(extra.iter());
        }
        for map in maps {
            let Some(map) = map.as_object() else { continue };
            if map.len() < request_depth {
                ancestor = true;
            }
            if map.len() > request_depth {
                deeper = true;
            }
            for (level_name, level) in map {
                if level["isExcludes"].as_bool() == Some(true) {
                    excludes = true;
                }
                let values: Vec<&str> = level["values"]
                    .as_array()
                    .map(|v| v.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if values.len() > 1 {
                    multi_value = true;
                }
                if values.iter().all(|v| v.is_empty()) {
                    empty_values = true;
                }
                for value in &values {
                    wildcard_positions(value, &mut wc);
                    if value.contains("*?") {
                        star_then_question = true;
                    }
                    if let Some((_, after)) = value.split_once('*') {
                        if after.chars().any(|c| c != '*' && c != '?') {
                            literal_after_star = true;
                        }
                    }
                    if let Some((_, request_value)) =
                        request_levels.iter().find(|(n, _)| n == level_name)
                    {
                        if !value.is_empty()
                            && value != request_value
                            && value.eq_ignore_ascii_case(request_value)
                        {
                            case_only_difference = true;
                        }
                    }
                }
            }
        }

        for (index, bucket) in [
            "policyItems",
            "denyPolicyItems",
            "allowExceptions",
            "denyExceptions",
        ]
        .iter()
        .enumerate()
        {
            if policy[*bucket].as_array().is_some_and(|b| !b.is_empty()) {
                buckets[index] = true;
            }
        }

        for bucket in [
            "policyItems",
            "denyPolicyItems",
            "allowExceptions",
            "denyExceptions",
            "dataMaskPolicyItems",
            "rowFilterPolicyItems",
        ] {
            let Some(items) = policy[bucket].as_array() else {
                continue;
            };
            for item in items {
                if item["groups"]
                    .as_array()
                    .is_some_and(|g| g.iter().any(|v| v.as_str() == Some("public")))
                {
                    public_group = true;
                }
                if item["users"]
                    .as_array()
                    .is_some_and(|u| u.iter().any(|v| v.as_str() == Some("{USER}")))
                {
                    user_current = true;
                }
                if item["roles"].as_array().is_some_and(|r| !r.is_empty()) {
                    role_named = true;
                }
                if let Some(accesses) = item["accesses"].as_array() {
                    for access in accesses {
                        if access["isAllowed"].as_bool() == Some(false) {
                            explicit_deny = true;
                        }
                        // All three spellings are `""` to Ranger.
                        match access.get("type") {
                            None | Some(Value::Null) => empty_access_type = true,
                            Some(Value::String(t)) if t.is_empty() => empty_access_type = true,
                            Some(Value::String(t)) if t == "all" => implied_all = true,
                            _ => {}
                        }
                    }
                }
                if bucket == "dataMaskPolicyItems" {
                    match item.get("dataMaskInfo") {
                        None | Some(Value::Null) => mask_without_info = true,
                        Some(info) => {
                            if info["valueExpr"].is_string() {
                                custom_value_expr = true;
                            }
                            if info["conditionExpr"].is_string() {
                                condition_expr = true;
                            }
                        }
                    }
                }
                if bucket == "rowFilterPolicyItems" {
                    match item.get("rowFilterInfo") {
                        None | Some(Value::Null) => filter_without_info = true,
                        Some(info) => match info.get("filterExpr") {
                            None | Some(Value::Null) => filter_expr_blank = true,
                            Some(Value::String(e)) if e.is_empty() => filter_expr_blank = true,
                            _ => {}
                        },
                    }
                }
            }
        }
    }

    let tags = tag_facts(case);
    let request_star = request_levels.iter().any(|(_, v)| *v == "*");
    let request_empty = request_levels.iter().any(|(_, v)| v.is_empty());
    // A gap: the request names a level whose parent it does not name.
    let names: Vec<&str> = request_levels.iter().map(|(n, _)| *n).collect();
    let request_gap = !names.is_empty() && {
        let mut seen_missing = false;
        let mut gap = false;
        for level in HIERARCHY_ORDER {
            if names.contains(level) {
                if seen_missing {
                    gap = true;
                }
            } else {
                seen_missing = true;
            }
        }
        gap
    };

    vec![
        ("policyType 0 (access)", case.policy_type == 0),
        ("policyType 1 (data mask)", case.policy_type == 1),
        ("policyType 2 (row filter)", case.policy_type == 2),
        (
            "row-filter case at TABLE granularity",
            case.policy_type == 2 && request_depth == 3 && !names.contains(&"column"),
        ),
        ("policy of a DIFFERENT type in the set", other_policy_type),
        (
            "ANCESTOR-shaped policy (fewer levels than request)",
            ancestor,
        ),
        ("policy DEEPER than the request", deeper),
        ("isExcludes", excludes),
        ("multi-value level", multi_value),
        ("level with no usable values", empty_values),
        ("additionalResources", additional),
        ("wildcard: leading *", wc[0]),
        ("wildcard: trailing *", wc[1]),
        ("wildcard: interior *", wc[2]),
        ("wildcard: repeated *", wc[3]),
        ("wildcard: leading ?", wc[4]),
        ("wildcard: trailing ?", wc[5]),
        ("wildcard: interior ?", wc[6]),
        ("wildcard: repeated ?", wc[7]),
        (
            "wildcard: `*` immediately before `?` (F1)",
            star_then_question,
        ),
        (
            "wildcard: literal AFTER a `*` (backtracking reachable)",
            literal_after_star,
        ),
        ("policy value that is only `*`", wc[8]),
        (
            "policy value differing from the request only by case",
            case_only_difference,
        ),
        ("request value `*`", request_star),
        ("request value empty", request_empty),
        ("request with a gap in the hierarchy", request_gap),
        ("bucket policyItems", buckets[0]),
        ("bucket denyPolicyItems", buckets[1]),
        ("bucket allowExceptions", buckets[2]),
        ("bucket denyExceptions", buckets[3]),
        ("group `public`", public_group),
        ("user `{USER}`", user_current),
        ("policy item naming a role", role_named),
        ("policyPriority override", priority_override),
        ("isDenyAllElse", deny_all_else),
        ("disabled policy", disabled),
        ("impliedGrants via trino `all`", implied_all),
        ("access entry with isAllowed:false", explicit_deny),
        (
            "access entry whose type reads as `\"\"` (F3)",
            empty_access_type,
        ),
        ("dataMask item with NO dataMaskInfo (F2)", mask_without_info),
        ("rowFilter item with NO rowFilterInfo", filter_without_info),
        ("CUSTOM mask carrying a valueExpr", custom_value_expr),
        ("mask conditionExpr", condition_expr),
        ("filterExpr explicitly empty or null", filter_expr_blank),
        ("eval-order duel", case.duel),
        ("tag: tagPolicies block present", tags.block_present),
        ("tag: a tag the block's policies REACH", tags.reached),
        (
            "tag: a tag the block's policies do NOT reach",
            tags.unreached,
        ),
        ("tag: match SELF", tags.match_types[0]),
        ("tag: match SELF_AND_ALL_DESCENDANTS", tags.match_types[1]),
        ("tag: match ANCESTOR", tags.match_types[2]),
        ("tag: match DESCENDANT (NOT admitted)", tags.match_types[3]),
        ("tag: match NONE (NOT admitted)", tags.match_types[4]),
        ("tag: empty resolved set (the narrowing)", tags.empty_set),
        ("tag: same NAME at two match types", tags.repeated_name),
        ("tag: attachment carrying attributes", tags.attributes),
        ("tag: deny-capable ACCESS policy", tags.deny_capable_access),
        (
            "tag: ACCESS policy with 2+ deny items",
            tags.multi_deny_access,
        ),
        ("tag: MASK policy of this kind", tags.mask_policy),
        (
            "tag: ROW FILTER policy of this kind",
            tags.row_filter_policy,
        ),
        (
            "tag: several policies of this kind at ONE priority",
            tags.several_at_one_priority,
        ),
        ("tag: policyPriority override", tags.priority_override),
        (
            "tag: policy of a DIFFERENT type in the block",
            tags.other_type,
        ),
        ("tag: policy value is a prefix wildcard", tags.prefix_value),
        (
            "tag: value differing from a resolved tag only by CASE",
            tags.case_only_value,
        ),
        ("tag: composed eval-order duel", case.composed_duel),
        ("tag: restore duel (T3's skip reachable)", case.restore_duel),
        (
            "tag: version-split duel (T5's 2.5.0 arm reachable)",
            case.version_split_duel,
        ),
        (
            "tag: candidacy duel (offset 553 reachable)",
            case.candidacy_duel,
        ),
        (
            "tag: audit-disabled policy with NO items (pruned)",
            tags.audit_pruned,
        ),
        (
            "tag: audit-disabled policy WITH items (kept)",
            tags.audit_kept,
        ),
        (
            "resource policy audit-disabled with NO items (NOT pruned)",
            resource_audit_pruneable,
        ),
        (
            "RANDOM composed ACCESS case with a resource policy above priority 0",
            // Read from the case rather than from `tags`, because the question is
            // about the two BLOCKS standing together and not about any one
            // policy's contents: a tag block that carries policies at all, and a
            // resource policy the engine loop would place above the tag verdict.
            //
            // `Population::of(case) == Composed` is the scoping, and it is the same
            // predicate the rates are grouped by — so this count and the column it
            // explains cannot drift apart.
            case.policy_type == 0
                && Population::of(case) == Population::Composed
                && case.service_policies["tagPolicies"]["policies"]
                    .as_array()
                    .is_some_and(|ps| !ps.is_empty())
                && policies.iter().any(|p| {
                    p["policyType"].as_i64().unwrap_or(0) == 0
                        && p["policyPriority"].as_i64().unwrap_or(0) > 0
                }),
        ),
    ]
}

/// Every level name either engine declares, root first — used only to spot a
/// gap in a request's hierarchy.
const HIERARCHY_ORDER: &[&str] = &["catalog", "schema", "database", "table", "column"];

/// Count the axes, print the table, and fail the run on any floor.
fn report_coverage(profile: &Profile, cases: &[Case]) {
    let mut counts = vec![0usize; AXES.len()];
    for case in cases {
        let hits = axis_hits(case);
        assert_eq!(
            hits.len(),
            AXES.len(),
            "AXES and axis_hits have drifted apart"
        );
        for (index, (name, hit)) in hits.iter().enumerate() {
            assert_eq!(
                *name, AXES[index].name,
                "AXES[{index}] is `{}` but axis_hits reports `{name}` — every floor below \
                 this point is attached to the wrong counter",
                AXES[index].name
            );
            if *hit {
                counts[index] += 1;
            }
        }
    }

    // Printed on EVERY run, not only on failure: a number nobody sees is a
    // number nobody checks, and the whole point is to notice an axis thinning
    // out before it reaches zero.
    eprintln!(
        "\ncoverage at ranger {} ({:?}), {} cases — cases exercising each axis:",
        profile.ranger_version,
        profile.engine,
        cases.len()
    );
    let mut starved = Vec::new();
    for (index, axis) in AXES.iter().enumerate() {
        let count = counts[index];
        let per_mille = 1000 * count / cases.len().max(1);
        let applies = axis.only.is_none_or(|e| e == profile.engine);
        let floor = axis.min_per_mille as usize;
        let flag = if !applies {
            "  n/a for this engine"
        } else if per_mille < floor {
            "  FLOOR"
        } else {
            ""
        };
        eprintln!(
            "  {:6} {:>5}‰  (floor {:>4}‰)  {}{}",
            count, per_mille, floor, axis.name, flag
        );
        if applies && per_mille < floor {
            starved.push(format!(
                "  {} — {count} cases ({per_mille}‰), floor {}‰",
                axis.name, floor
            ));
        }
    }
    assert!(
        starved.is_empty(),
        "ranger {}: {} coverage axes have thinned out below their floor. The corpus has \
         stopped exercising them, so agreement on it says nothing about them — a gate that \
         fails SILENTLY and GREEN:\n{}",
        profile.ranger_version,
        starved.len(),
        starved.join("\n")
    );
}

// ── the gate ─────────────────────────────────────────────────────────────

/// Which population a case belongs to. **Every rate this file prints is over one
/// of these, and never over their union.**
///
/// ⚠ FIX ROUND 1, IMPORTANT 2. Three of the five composed rates used to be
/// divided by every tag-carrying case — a denominator 30% of which is cases
/// CONSTRUCTED to hit the refusal being measured, or constructed so that they
/// cannot. Re-measured over the random half the printed figures were wrong in
/// BOTH directions (`EvalOrderAmbiguous` 6.50% printed against 2.60% actual,
/// `TagPriorityArbitrationNotModelled` 1.65% against 2.36%), so it was not a bias
/// a reader could correct by a factor — and the one a follow-up task has to budget
/// for was the one understated.
///
/// The resource-path duels are split out for the same reason even though they
/// contribute nothing to the ACCESS refusals today: a rate whose denominator
/// silently contains a constructed shape is exactly the defect above, and
/// "contributes nothing today" is a claim that needs re-checking every time
/// something is added, not an invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Population {
    /// No `tagPolicies` block, drawn at random.
    Resource,
    /// No tag block, and built by [`access_order_duel`].
    ResourceDuel,
    /// A tag block, drawn at random — **the population a real download is in**,
    /// and therefore the rate to plan against.
    Composed,
    /// A tag block, and built by [`composed_duel`] or [`candidacy_duel`].
    ComposedDuel,
}

impl Population {
    fn of(case: &Case) -> Population {
        let tagged = case.service_policies.get("tagPolicies").is_some();
        // ⚠ `composed_duel` is named explicitly (fix round 2). It happens to imply
        // `duel` today, through an unasserted coupling in `generate_case` — and a
        // builder that set `composed_duel: true, duel: false` would file a
        // CONSTRUCTED shape into `composed, random`, which is the exact defect the
        // four populations exist to fix. Naming every constructed flag costs
        // nothing and removes the coupling from the argument.
        match (
            tagged,
            case.duel
                || case.composed_duel
                || case.candidacy_duel
                || case.restore_duel
                || case.version_split_duel,
        ) {
            (false, false) => Population::Resource,
            (false, true) => Population::ResourceDuel,
            (true, false) => Population::Composed,
            (true, true) => Population::ComposedDuel,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Population::Resource => "resource, random",
            Population::ResourceDuel => "resource, duel",
            Population::Composed => "composed, random",
            Population::ComposedDuel => "composed, duel",
        }
    }
}

const POPULATIONS: &[Population] = &[
    Population::Resource,
    Population::ResourceDuel,
    Population::Composed,
    Population::ComposedDuel,
];

#[derive(Default)]
struct Tally {
    cases: usize,
    compared: usize,
    /// Deliberately-constructed eval-order duels, and how many of them the
    /// two sides agreed on. A claim about eval order is only as strong as the
    /// number of cases that could have falsified it.
    duels: usize,
    duels_agreeing: usize,
    /// …of which these were issued UNDER a tag verdict.
    composed_duels: usize,
    composed_duels_agreeing: usize,
    /// Deliberate candidacy duels, and the DENOMINATOR of the
    /// `ResourceCandidacyNotReproducible` rate. Over the whole corpus that rate
    /// is ~0, because 800 random cases produced the four-way conjunction zero
    /// times — and a rate of 0/800 measures the generator, not the refusal.
    candidacy_duels: usize,
    candidacy_duels_declined: usize,
    /// Deliberate RESTORE duels, and how many of them AGREED with the engine.
    /// Unlike the other two shapes this one must ANSWER: reverting T3's skip turns
    /// the agreement into a disagreement, which is the property it exists for.
    restore_duels: usize,
    restore_duels_agreeing: usize,
    /// Deliberate [`version_split_duel`]s generated, and how many of them DECLINED
    /// with [`Declined::TagDenyVersionSplit`]. Both are needed: the count alone
    /// cannot tell "the generator stopped" from "the refusal stopped", and that
    /// confusion is exactly what let this refusal sit at 0/4000 behind a described
    /// guard.
    version_split_duels: usize,
    version_split_duels_declined: usize,
    /// Cases carrying a `tagPolicies` block.
    tagged: usize,
    /// How many cases fell in each [`Population`] — the denominators.
    population: BTreeMap<Population, usize>,
    /// Each admitted refusal, per population. Reporting one number across them
    /// would answer none of the questions this gate is asked.
    declined: BTreeMap<(Declined, Population), usize>,
    /// ACCESS cases where both sides named a policy comparison was possible for,
    /// and how many of those AGREED on the id (RULING 3).
    ids_compared: usize,
    ids_agreeing: usize,
    /// Each DECLARED id divergence and how often it fired. A divergence that
    /// matches none of them is not counted here — it fails the run.
    id_divergences: BTreeMap<&'static str, usize>,
    allowed: usize,
    denied: usize,
    masked: usize,
    no_mask: usize,
    filtered: usize,
    no_filter: usize,
}

impl Tally {
    fn declined_count(&self, kind: Declined, population: Population) -> usize {
        self.declined.get(&(kind, population)).copied().unwrap_or(0)
    }

    /// Every firing of `kind`, across all four populations — the number the
    /// per-kind CEILINGS are checked against.
    fn declined_total(&self, kind: Declined) -> usize {
        self.declined
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|(_, n)| n)
            .sum()
    }

    fn population_count(&self, population: Population) -> usize {
        self.population.get(&population).copied().unwrap_or(0)
    }

    fn absorb(&mut self, other: &Tally) {
        self.cases += other.cases;
        self.compared += other.compared;
        self.duels += other.duels;
        self.duels_agreeing += other.duels_agreeing;
        self.composed_duels += other.composed_duels;
        self.composed_duels_agreeing += other.composed_duels_agreeing;
        self.candidacy_duels += other.candidacy_duels;
        self.candidacy_duels_declined += other.candidacy_duels_declined;
        self.restore_duels += other.restore_duels;
        self.restore_duels_agreeing += other.restore_duels_agreeing;
        self.version_split_duels += other.version_split_duels;
        self.version_split_duels_declined += other.version_split_duels_declined;
        self.tagged += other.tagged;
        for (key, count) in &other.population {
            *self.population.entry(*key).or_default() += count;
        }
        for (key, count) in &other.declined {
            *self.declined.entry(*key).or_default() += count;
        }
        self.ids_compared += other.ids_compared;
        self.ids_agreeing += other.ids_agreeing;
        for (key, count) in &other.id_divergences {
            *self.id_divergences.entry(*key).or_default() += count;
        }
        self.allowed += other.allowed;
        self.denied += other.denied;
        self.masked += other.masked;
        self.no_mask += other.no_mask;
        self.filtered += other.filtered;
        self.no_filter += other.no_filter;
    }
}

/// The assertion three places in the tree already CLAIMED, now real.
///
/// ⚠ **WHOLE-BRANCH REVIEW.** `Declined::TagDenyVersionSplit`'s doc said the
/// 2.5.0/2.8.0 asymmetry *"is itself the assertion: a run where the two halves'
/// rates were equal would mean the version input had stopped being read"* — and no
/// `assert!` anywhere compared the halves. The halves WERE equal, at 0 == 0, which
/// is the file's own stated failure condition; the only bound was a ceiling of 10‰
/// over a measured 0‰ that nothing could exceed. A described guard that no code
/// performs is the shape that has cost this branch the most, so this function
/// exists and is called once per engine.
///
/// Three things are asserted, and each fails for a different reason:
///
/// 1. the duel is still GENERATED — otherwise 2 and 3 are vacuous, which is how
///    the last version of this claim passed;
/// 2. at a plugin version that is NOT 2.8.0 the crate REFUSES essentially all of
///    them — so deleting the `plugin_version` read turns this red;
/// 3. at 2.8.0 it refuses NONE of them — so widening the refusal to cover the
///    version this crate actually reproduces turns this red too.
///
/// The 90% slack in 2 is for a duel whose random tag name or match type makes the
/// tag phase not decide at all; it is not slack on the version read, which 3 pins
/// from the other side at exactly zero.
fn check_version_split_is_asymmetric(profile: &Profile, tally: &Tally) {
    let generated = tally.version_split_duels;
    let declined = tally.version_split_duels_declined;
    assert!(
        generated * 60 >= tally.cases,
        "ranger {}: only {generated} version-split duels in {} cases — the ONLY shape that \
         reaches TagDenyReopenedDiffersByPluginVersion has stopped being generated, and every \
         other assertion about that refusal is now vacuous",
        profile.ranger_version,
        tally.cases,
    );
    if profile.ranger_version == "2.8.0" {
        assert_eq!(
            declined, 0,
            "ranger 2.8.0: {declined} of {generated} version-split duels REFUSED. 2.8.0 is the \
             version this crate reproduces, so this shape must be ANSWERED and compared against \
             the real engine — a refusal here means the version read has been widened past what \
             was measured"
        );
    } else {
        assert!(
            declined * 10 >= generated * 9 && declined > 0,
            "ranger {}: only {declined} of {generated} version-split duels refused. At a plugin \
             version other than 2.8.0 this crate's model is MEASURED WRONG (it would reopen a \
             deny 2.5.0 keeps), so a run that answers them is a run that has stopped reading \
             EngineConfig::plugin_version",
            profile.ranger_version,
        );
    }
    eprintln!(
        "differential: ranger {} — {declined} of {generated} version-split duels refused \
         (expected {})",
        profile.ranger_version,
        if profile.ranger_version == "2.8.0" {
            "NONE: 2.8.0 is the arm this crate reproduces"
        } else {
            "essentially all: 2.5.0's arm is unmodelled"
        },
    );
}

/// The refusal rates the package owes, each over the POPULATION it belongs to.
///
/// A percentage with no denominator is the shape in which "unmeasured" and
/// "measured at zero" look alike, and this file has been bitten by that twice:
/// once when `EvalOrderAmbiguous` was reported as one number over the whole
/// corpus while the composed half of it did not exist, and once — fix round 1,
/// IMPORTANT 2 — when three composed rates were divided by a denominator 30% of
/// which was cases CONSTRUCTED to hit the very refusal being measured. That
/// second one was wrong in BOTH directions (`EvalOrderAmbiguous` printed 6.50%
/// against 2.60% actual, `TagPriorityArbitrationNotModelled` 1.65% against
/// 2.36%), so it was not a bias a reader could correct by a factor, and the rate
/// a follow-up task has to budget for was the understated one.
///
/// So every number here carries its numerator, its denominator and the name of
/// its population.
///
/// ⚠ **WHOLE-BRANCH REVIEW: this doc used to close "and `COMPOSED RANDOM` is the
/// column a real download is in". That is a coverage claim stronger than the
/// data.** A real download CAN carry an override policy — one supported API call
/// emits `policyPriority: 1` — and this column cannot: `policy`'s `level_priority`
/// forces every resource policy of a random tag-carrying case to 0. So the
/// three-way arbitration at offsets 469/505 is exercised against a resource
/// evaluator only in the `composed duel` column, which the printed rates show
/// (`COMPOSED RANDOM` EvalOrder 1.91% / ResourceCandidacy 0.00% against the duel
/// column's 16.54% / 29.52%). `COMPOSED RANDOM` is the column a real download WITH
/// NO OVERRIDE POLICY is in; the hold-out and its count are on the
/// `RANDOM composed ACCESS case with a resource policy above priority 0` axis.
fn report_rates(label: &str, tally: &Tally) {
    let pct = |n: usize, d: usize| -> String {
        if d == 0 {
            "n/a (empty)".to_string()
        } else {
            format!("{n}/{d} = {:.2}%", 100.0 * n as f64 / d as f64)
        }
    };
    eprintln!("\nmeasured refusal rates — {label}");
    eprintln!(
        "  populations: {}",
        POPULATIONS
            .iter()
            .map(|p| format!("{} {}", tally.population_count(*p), p.label()))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    eprintln!(
        "  {:36} {:>20} {:>20} {:>20} {:>20}",
        "refusal", "resource random", "resource duel", "COMPOSED RANDOM", "composed duel",
    );
    for kind in DECLINED_KINDS {
        eprintln!(
            "  {:36} {:>20} {:>20} {:>20} {:>20}",
            kind.label(),
            pct(
                tally.declined_count(*kind, Population::Resource),
                tally.population_count(Population::Resource)
            ),
            pct(
                tally.declined_count(*kind, Population::ResourceDuel),
                tally.population_count(Population::ResourceDuel)
            ),
            pct(
                tally.declined_count(*kind, Population::Composed),
                tally.population_count(Population::Composed)
            ),
            pct(
                tally.declined_count(*kind, Population::ComposedDuel),
                tally.population_count(Population::ComposedDuel)
            ),
        );
    }
    // The two deliberate shapes against their OWN denominators, which is the only
    // way their numbers mean anything.
    eprintln!(
        "  {:36} {} of the deliberate composed duels agreed with the engine",
        "composed eval-order duels",
        pct(tally.composed_duels_agreeing, tally.composed_duels),
    );
    eprintln!(
        "  {:36} {} of the deliberate candidacy duels REFUSED",
        "candidate-set (offset 553) refusals",
        pct(tally.candidacy_duels_declined, tally.candidacy_duels),
    );
    eprintln!(
        "  {:36} {} of the deliberate restore duels AGREED with the engine",
        "trie-excluded view / restore (T3)",
        pct(tally.restore_duels_agreeing, tally.restore_duels),
    );
    // The COMPARED fraction, which the ceiling at the bottom of this file asserts
    // on: it is the one number that sees an over-refusal spread thinly across
    // every kind.
    eprintln!(
        "  {:36} {} of all cases were compared at all",
        "compared fraction",
        pct(tally.compared, tally.cases),
    );
    eprintln!(
        "  {:36} {} agreed on the reported policy id",
        "ACCESS policy-id fidelity",
        pct(tally.ids_agreeing, tally.ids_compared),
    );
    for (reason, count) in &tally.id_divergences {
        eprintln!("  {:36} {count} × {reason}", "  …declared divergence:");
    }
}

fn ranger_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/ranger"))
}

// ── the two divergences this gate MEASURED ───────────────────────────────

/// A construct the differential gate once caught this crate getting wrong,
/// kept under test as a minimal reproducer of the FIXED behaviour.
///
/// These pins were written asserting the DISAGREEMENT, while the corpus
/// excluded the shape that produced it. Both defects are now fixed, the
/// exclusions are gone, and the pins assert AGREEMENT on the same inputs —
/// which is what a fix is supposed to look like from here. They stay because
/// a randomly generated corpus is a poor place to notice that one specific
/// shape has quietly stopped being covered.
struct Pinned {
    case: Case,
    finding: &'static str,
    /// What BOTH sides must now answer, in prose — for the failure message.
    expected: &'static str,
    /// …and the same thing as a value, asserted separately.
    ///
    /// `assert_eq!(mine, theirs)` alone is not enough: a pin whose case
    /// degenerated so that BOTH sides trivially answer `Denied` for some
    /// unrelated reason would still pass, and the pin would have quietly
    /// stopped pinning anything. The absolute expectation is the half that
    /// notices.
    outcome: Outcome,
}

fn pinned_divergences(def_json: &Value) -> Vec<Pinned> {
    let service_policies = |policies: Value| {
        json!({
            "serviceName": "trino_prod",
            "serviceId": 1,
            "policyVersion": 1,
            "serviceDef": def_json,
            "policies": policies,
        })
    };

    vec![
        // ── F1 ────────────────────────────────────────────────────────────
        //
        // `FilenameUtils.wildcardMatch` is NOT a classic greedy matcher with
        // backtracking, which is what `crate::resource`'s reimplementation is.
        // `splitOnTokens` turns `?*?` into ["?", "*", "?"], and a `*` token
        // advances the text index only when it is the LAST token (`textIdx =
        // filename.length()`) or when the NEXT token is a literal (via
        // `checkIndexOf`). A `*` followed by `?` therefore consumes NOTHING:
        //
        //   FilenameUtils.wildcardMatch("HIVE", "?*?", INSENSITIVE) == false
        //   FilenameUtils.wildcardMatch("HIVE", "*?",  INSENSITIVE) == false
        //   FilenameUtils.wildcardMatch("ab",   "?*?", INSENSITIVE) == true
        //
        // (measured against commons-io on the 2.5.0 classpath). A classic
        // matcher says true for all three.
        //
        // This is the FAIL-OPEN direction: the policy below ALLOWS, so egret
        // grants a request the live Trino plugin refuses.
        Pinned {
            finding: "F1: `*` followed by `?` consumes nothing, so `?*?` does NOT cover `HIVE`",
            expected: "Denied — wildcardMatch(\"HIVE\", \"?*?\") is false, so no policy matched. \
                       A greedy matcher answers Allowed here, which is the fail-open direction \
                       this pin exists to keep shut.",
            outcome: Outcome::Access { allowed: false },
            case: Case {
                id: "pin-f1-wildcard".to_string(),
                duel: false,
                composed_duel: false,
                candidacy_duel: false,
                restore_duel: false,
                version_split_duel: false,
                policy_type: 0,
                service_policies: service_policies(json!([{
                    "id": 1, "name": "f1", "service": "trino_prod", "policyType": 0,
                    "isEnabled": true, "policyPriority": 0,
                    "resources": {
                        "catalog": {"values": ["?*?"]},
                        "schema":  {"values": ["hive"]},
                        "table":   {"values": ["ab"]},
                    },
                    "policyItems": [{
                        "users": ["dave"], "groups": [], "roles": [],
                        "accesses": [{"type": "select", "isAllowed": true}],
                    }],
                }])),
                request: AccessRequest {
                    user: "dave".to_string(),
                    groups: BTreeSet::new(),
                    roles: BTreeSet::new(),
                    access_type: "select".to_string(),
                    resource: RequestResource::new()
                        .with("catalog", "HIVE")
                        .with("schema", "hive")
                        .with("table", "ab"),
                    owner: None,
                    resolved_tags: None,
                },
            },
        },
        // ── F2 ────────────────────────────────────────────────────────────
        //
        // Ranger deserialises with Jackson, which binds through the SETTER:
        // `RangerPolicy$RangerDataMaskPolicyItem.setDataMaskInfo(null)`
        // substitutes `new RangerPolicyItemDataMaskInfo()`, and the no-arg
        // constructor reaches the same setter through the all-args one.
        // (Gson, which this HARNESS uses for its own cases/verdicts files,
        // assigns fields directly and would leave it null — hence the
        // confusion this comment used to carry.)
        // So `getDataMaskInfo()` is NEVER null on a deserialised item, the
        // `ifnull` guard at offset 8 of
        // `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` never
        // fires, and the item runs the whole body: `setMaskType(null)` and then
        // `policyEvaluator.updateAccessResult(result, matchType, true, …)`,
        // which sets policyId, policyPriority and isAllowed. The item DETERMINES
        // the result with a null mask type and the search stops.
        //
        // This crate reads the absent key as `MaskInfo: None` and models it as
        // "reports nothing, keep looking", so it goes on to the next item and
        // reports a mask Ranger never reaches.
        Pinned {
            finding: "F2: a dataMask item with no `dataMaskInfo` still DETERMINES, with a null \
                      mask type, because Ranger's model class cannot represent the absence",
            expected: "maskType null — the priority-1 policy determines and the priority-0 one \
                       is never reached. Reading the wire format literally answers MASK_HASH.",
            outcome: Outcome::Mask {
                mask_type: None,
                masked_value: None,
                mask_condition: None,
            },
            case: Case {
                id: "pin-f2-missing-datamaskinfo".to_string(),
                duel: false,
                composed_duel: false,
                candidacy_duel: false,
                restore_duel: false,
                version_split_duel: false,
                policy_type: 1,
                service_policies: service_policies(json!([
                    {
                        // Ranger reaches this one first (higher priority) and
                        // its ONLY matching item carries no `dataMaskInfo`.
                        "id": 1, "name": "f2-high", "service": "trino_prod", "policyType": 1,
                        "isEnabled": true, "policyPriority": 1,
                        "resources": {
                            "catalog": {"values": ["hive"]},
                            "schema":  {"values": ["sales"]},
                            "table":   {"values": ["orders"]},
                            "column":  {"values": ["id"]},
                        },
                        "dataMaskPolicyItems": [{
                            "users": ["dave"], "groups": [], "roles": [],
                            "accesses": [{"type": "select", "isAllowed": true}],
                        }],
                    },
                    {
                        // …and this one, which egret reaches and Ranger does
                        // not, carries a real mask.
                        "id": 2, "name": "f2-low", "service": "trino_prod", "policyType": 1,
                        "isEnabled": true, "policyPriority": 0,
                        "resources": {
                            "catalog": {"values": ["hive"]},
                            "schema":  {"values": ["sales"]},
                            "table":   {"values": ["orders"]},
                            "column":  {"values": ["id"]},
                        },
                        "dataMaskPolicyItems": [{
                            "users": ["dave"], "groups": [], "roles": [],
                            "accesses": [{"type": "select", "isAllowed": true}],
                            "dataMaskInfo": {"dataMaskType": "MASK_HASH"},
                        }],
                    },
                ])),
                request: AccessRequest {
                    user: "dave".to_string(),
                    groups: BTreeSet::new(),
                    roles: BTreeSet::new(),
                    access_type: "select".to_string(),
                    resource: RequestResource::new()
                        .with("catalog", "hive")
                        .with("schema", "sales")
                        .with("table", "orders")
                        .with("column", "id"),
                    owner: None,
                    resolved_tags: None,
                },
            },
        },
    ]
}

/// Assert both once-divergent shapes now AGREE, on the exact inputs that
/// caught them.
fn check_pinned_divergences(def_json: &Value, def: &ServiceDef, engine: &EngineConfig) {
    let pins = pinned_divergences(def_json);
    let cases: Vec<Case> = pins.iter().map(|p| p.case.clone()).collect();
    let verdicts = ask_ranger(&cases, "trino", "2.5.0", "pinned");

    for (pin, verdict) in pins.iter().zip(&verdicts) {
        let set: ServicePolicySet = serde_json::from_value(pin.case.service_policies.clone())
            .expect("parse pinned policy set");
        let mine = rust_outcome(&pin.case, &set, def, engine);
        let theirs = java_outcome(pin.case.policy_type, verdict);
        // Two assertions, not one. Agreement says the two implementations
        // still match each other; the absolute value says they still match
        // what was MEASURED, so a case that degenerated into both sides
        // answering the same thing for an unrelated reason cannot pass.
        assert_eq!(
            mine, theirs,
            "PINNED CASE {} HAS REGRESSED — {}\n  expected: {}\n  egret: {mine:?}\n  ranger: {theirs:?}",
            pin.case.id, pin.finding, pin.expected,
        );
        assert_eq!(
            mine, pin.outcome,
            "PINNED CASE {} no longer produces the outcome it was built to produce, even though \
             both sides agree on it — the case has degenerated and the pin has stopped pinning \
             anything.\n  expected: {}\n  both sides now say: {mine:?}",
            pin.case.id, pin.expected,
        );
        eprintln!(
            "differential: pinned case {} agrees — {}",
            pin.case.id, pin.finding
        );
    }
}

/// The one shape the corpus CANNOT carry, pinned on its own: a tag
/// `dataMaskPolicyItems` item with no `dataMaskInfo`.
///
/// This is not a divergence in a verdict. `RangerPolicyRepository`'s
/// `ServicePolicies$TagPolicies` constructor — and no other constructor in
/// either jar — calls `normalizeAndPrunePolicies`, which reads
/// `dataMaskType.contains(":")` with no null guard on a type Ranger's own model
/// class has just defaulted to null (finding F2's substitution). The plugin
/// THROWS while building its repository, so there is no engine and NOTHING it
/// serves is answered — an incident, not a gap.
///
/// Found by this gate on 2026-10-01, at 150 cases, the first time the corpus
/// generated a `tagPolicies` block at all. Pinned rather than generated because
/// a case Ranger cannot answer has no verdict to compare: thousands of them in
/// the corpus would be thousands of uncomparable cases, and one here is the
/// whole measurement.
///
/// **Both plugin versions**, because "identical in 2.8.0" is the claim, and the
/// crate refuses on the strength of it. The Trino service-def is used at both,
/// which the corpus itself does not do — `EgretTagServiceResourceValidationTest`
/// already runs the same def at both versions, and nothing in this claim is
/// about the resource def at all.
/// The whole-branch review's CRITICAL, pinned on the two cases it was measured
/// with, at both plugin versions.
///
/// ⚠ **The line this pins was pinned in NEITHER direction before.** Mutating
/// `} else if !allowed {` in [`evaluate`]'s allow arm to `} else {` compiles and
/// survived the whole suite, 259 passed / 0 failed — because nothing anywhere
/// exercised a tag ALLOW under a strictly higher-priority resource allow. The
/// corpus cannot reach it (`policy`'s `level_priority`), so the shape needs a
/// hand-built fixture or it has no coverage at all.
///
/// Both mutants are now red:
///
/// * `} else {` makes the composite arm dead, so case A is ANSWERED and the
///   refusal assertion below fails;
/// * `found.priority > priority` → `false` removes the refusal, so case A is
///   answered `Denied` by the low-priority denier and the same assertion fails.
///
/// Case B is the control that makes case A's cause legible: the SAME two resource
/// policies with the `tagPolicies` block removed. Both engines answer
/// `Allowed, policyId 1` for it, so the tag ALLOW is the sole trigger — without
/// the control, "ranger allows" would be consistent with the resource policies
/// alone and the finding would not be located.
fn check_measured_priority_composite(
    def_json: &Value,
    tag_def_json: &Value,
    def: &ServiceDef,
    engine: &EngineConfig,
) {
    // `p1` at priority 1 ALLOWS and is `isDenyAllElse` (so `hasDeny()` is true and
    // offset 553 cannot determine on it); `p2` at priority 0 DENIES the same thing.
    // Ranger composites p1's priority onto the already-allowed result and shuts p2
    // out at offset 505; this crate modelled the allow branch's `if (!isAllowed)`
    // guard as "writes nothing" and let p2 deny.
    let resource_policies = json!([
        {
            "id": 1, "name": "p1", "service": "trino_prod", "policyType": 0,
            "isEnabled": true, "policyPriority": 1, "isDenyAllElse": true,
            "resources": {
                "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}, "column": {"values": ["id"]},
            },
            "policyItems": [{
                "users": ["bob"], "groups": [], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
            }],
        },
        {
            "id": 2, "name": "p2", "service": "trino_prod", "policyType": 0,
            "isEnabled": true, "policyPriority": 0,
            "resources": {
                "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}, "column": {"values": ["id"]},
            },
            "denyPolicyItems": [{
                "users": ["bob"], "groups": [], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
            }],
        },
    ]);
    let tag_block = json!({
        "serviceName": "trino_prod_tag",
        "serviceId": 2,
        "policyVersion": 1,
        "serviceDef": tag_def_json,
        "policies": [{
            "id": 901, "name": "t901", "service": "trino_prod_tag", "policyType": 0,
            "isEnabled": true, "policyPriority": 0,
            "resources": {"tag": {"values": ["pii"]}},
            "policyItems": [{
                "users": ["bob"], "groups": [], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
            }],
        }],
    });
    let request = |tagged: bool| AccessRequest {
        user: "bob".to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "select".to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders")
            .with("column", "id"),
        owner: None,
        resolved_tags: Some(UnenrichedTags::new(if tagged {
            vec![ResolvedTag {
                name: "pii".to_string(),
                match_type: MatchType::Self_,
                attributes: BTreeMap::new(),
                options: BTreeSet::new(),
                validity_periods: 0,
                attached_through: TagServiceResource {
                    levels: ["catalog", "schema", "table"]
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                    is_enabled: true,
                },
            }]
        } else {
            Vec::new()
        })),
    };
    let envelope = |tagged: bool| {
        let mut e = Map::new();
        e.insert("serviceName".to_string(), json!("trino_prod"));
        e.insert("serviceId".to_string(), json!(1));
        e.insert("policyVersion".to_string(), json!(1));
        e.insert("serviceDef".to_string(), def_json.clone());
        e.insert("policies".to_string(), resource_policies.clone());
        if tagged {
            e.insert("tagPolicies".to_string(), tag_block.clone());
        }
        Value::Object(e)
    };
    let build = |id: &str, tagged: bool| Case {
        id: id.to_string(),
        policy_type: 0,
        duel: false,
        composed_duel: false,
        candidacy_duel: false,
        restore_duel: false,
        version_split_duel: false,
        service_policies: envelope(tagged),
        request: request(tagged),
    };
    let cases = vec![
        build("pin-t7-a-tag-allow", true),
        build("pin-t7-b-control", false),
    ];

    for (case, want_refusal) in cases.iter().zip([true, false]) {
        let set: ServicePolicySet =
            serde_json::from_value(case.service_policies.clone()).expect("parse t7 pin");
        let mine = rust_outcome(case, &set, def, engine);
        if want_refusal {
            assert!(
                matches!(&mine, Outcome::Declined(Declined::AccessPriorityComposite)),
                "the tag ALLOW under a strictly higher-priority resource allow must REFUSE with \
                 AccessPriorityCompositeNotModelled, not {mine:?}.\n  \
                 `Denied` means the refusal was removed and the over-deny is back; `Allowed` \
                 means the allow arm now writes unconditionally, which is a MODEL of the \
                 accessTypeResults composite and needs the audit in open item 0 before it can \
                 be trusted — the two-case fixture cannot tell a correct model from a lucky one"
            );
        } else {
            assert_eq!(
                mine,
                Outcome::Access { allowed: true },
                "the CONTROL — the same two resource policies with no tagPolicies block — must \
                 answer Allowed. If it does not, case A's refusal is not located by the tag \
                 ALLOW and the whole finding needs re-measuring"
            );
        }
    }

    for version in ["2.5.0", "2.8.0"] {
        let verdicts = ask_ranger(&cases, "trino", version, "t7pin");
        for (verdict, case) in verdicts.iter().zip(&cases) {
            assert_eq!(
                verdict.get("error").and_then(Value::as_str),
                None,
                "ranger {version} errored on {}: {verdict}",
                case.id
            );
            assert_eq!(
                (
                    verdict.get("isAllowed").and_then(Value::as_bool),
                    java_access_policy_id(verdict)
                ),
                (Some(true), Some(1)),
                "ranger {version} was measured to answer `Allowed, policyId 1` for {} — BOTH \
                 cases, at BOTH versions, which is what makes this a modelling gap and not a \
                 version split. It did not, so the refusal this crate now carries is resting \
                 on a measurement that no longer holds: re-measure before narrowing it.\n  \
                 verdict: {verdict}",
                case.id
            );
        }
        eprintln!(
            "differential: ranger {version} answers Allowed/policyId 1 for the tag ALLOW under a \
             higher-priority resource allow, as measured"
        );
    }
}

fn check_measured_tag_crash(
    def_json: &Value,
    tag_def_json: &Value,
    def: &ServiceDef,
    engine: &EngineConfig,
) {
    let case = Case {
        id: "pin-tag-mask-null-crash".to_string(),
        policy_type: 1,
        duel: false,
        composed_duel: false,
        candidacy_duel: false,
        restore_duel: false,
        version_split_duel: false,
        service_policies: json!({
            "serviceName": "trino_prod",
            "serviceId": 1,
            "policyVersion": 1,
            "serviceDef": def_json,
            "policies": [],
            "tagPolicies": {
                "serviceName": "trino_prod_tag",
                "serviceId": 2,
                "policyVersion": 1,
                "serviceDef": tag_def_json,
                "policies": [{
                    "id": 8901, "name": "tag-mask-no-info", "service": "trino_prod_tag",
                    "policyType": 1, "isEnabled": true, "policyPriority": 0,
                    "resources": {"tag": {"values": ["pii"]}},
                    // No `dataMaskInfo`. That is the whole fixture.
                    "dataMaskPolicyItems": [{
                        "users": ["dave"], "groups": [], "roles": [],
                        "accesses": [{"type": "select", "isAllowed": true}],
                    }],
                }],
            },
        }),
        request: AccessRequest {
            user: "dave".to_string(),
            groups: BTreeSet::new(),
            roles: BTreeSet::new(),
            access_type: "select".to_string(),
            resource: RequestResource::new()
                .with("catalog", "hive")
                .with("schema", "sales")
                .with("table", "orders")
                .with("column", "id"),
            owner: None,
            resolved_tags: Some(UnenrichedTags::new(vec![ResolvedTag {
                name: "pii".to_string(),
                match_type: MatchType::Self_,
                attributes: BTreeMap::new(),
                options: BTreeSet::new(),
                validity_periods: 0,
                attached_through: TagServiceResource {
                    levels: ["catalog", "schema", "table"]
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                    is_enabled: true,
                },
            }])),
        },
    };

    let set: ServicePolicySet =
        serde_json::from_value(case.service_policies.clone()).expect("parse crash pin");
    let mine = rust_outcome(&case, &set, def, engine);
    // The crate must REFUSE, and refuse for this reason — not merely be
    // `Undecidable`. `Refused` here is the escape category, which is right: the
    // corpus is not allowed to contain this shape, and `only_declined` must not
    // start admitting it.
    assert!(
        matches!(&mine, Outcome::Refused(why) if why.contains("MaskTypeAbsent")),
        "the tag mask crash must refuse with TagPolicyNotNormalisable{{MaskTypeAbsent}}, \
         not {mine:?} — and it must stay OUTSIDE `only_declined`, or the corpus would be \
         allowed to generate a shape the real plugin cannot answer at all"
    );

    for version in ["2.5.0", "2.8.0"] {
        let verdicts = ask_ranger(std::slice::from_ref(&case), "trino", version, "tagcrash");
        let error = verdicts[0]
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            error.contains("NullPointerException")
                && error.contains("normalizeAndPrunePolicyItems"),
            "ranger {version} was expected to THROW in \
             RangerPolicyRepository.normalizeAndPrunePolicyItems for a tag mask item with no \
             dataMaskInfo. It did not, so the refusal this crate now carries is resting on a \
             measurement that no longer holds — re-measure before narrowing it.\n  \
             verdict: {}",
            verdicts[0]
        );
        eprintln!("differential: ranger {version} crashes on the null tag mask type, as measured");
    }
}

/// Send `cases` through the real engine at one plugin version and read the
/// verdicts back. Each run also executes the four hand-written anchor tests,
/// which is deliberate: they carry known-good expected values and are the only
/// thing in this harness that does not merely assert agreement.
///
/// **That claim is now checked rather than asserted in prose.** `mvn` exits 0
/// with zero anchors run (`tests/ranger/pom.xml` names surefire `<includes>`
/// by FILENAME, so a rename empties the run silently), which makes "four
/// anchors passed" and "no anchor ran" indistinguishable from the exit
/// status. So this reads the per-class summary line and requires it.
fn ask_ranger(cases: &[Case], service_type: &str, ranger_version: &str, tag: &str) -> Vec<Value> {
    let dir = ranger_dir();
    // The scratch path carries the pid: several agents share one temp
    // directory and an unprefixed name is another run's file.
    let work = std::env::temp_dir().join(format!(
        "egret-ranger-diff-{ranger_version}-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&work).expect("create work dir");
    let cases_path = work.join("cases.json");
    let verdicts_path = work.join("verdicts.json");
    // A stale verdicts file from an earlier run would be read as this run's
    // answer if Maven failed to write one.
    let _ = std::fs::remove_file(&verdicts_path);

    let payload: Vec<Value> = cases.iter().map(|c| c.to_json(service_type)).collect();
    std::fs::write(
        &cases_path,
        serde_json::to_vec(&Value::Array(payload)).expect("serialise cases"),
    )
    .expect("write cases.json");

    eprintln!(
        "differential: {} {tag} cases at ranger {ranger_version} ({service_type}); cases={}",
        cases.len(),
        cases_path.display()
    );

    // ONE `mvn test` at a time in `tests/ranger`. Since SP4d-3 Task 8 two
    // tests in this binary drive the harness, and cargo runs them on parallel
    // threads; two Mavens compiling into the same `target/` race each other.
    // A poisoned lock only means another test panicked, which that test
    // already reports.
    static MAVEN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_maven = MAVEN.lock().unwrap_or_else(|e| e.into_inner());
    // No `-q`: the per-class surefire summary this asserts on is logged at
    // INFO, and `-q` suppresses it.
    let out = Command::new("mvn")
        .args([
            "test".to_string(),
            format!("-Dranger.version={ranger_version}"),
            format!("-Degret.cases={}", cases_path.display()),
            format!("-Degret.verdicts={}", verdicts_path.display()),
        ])
        .current_dir(&dir)
        .output()
        .expect("run mvn");
    let log = String::from_utf8_lossy(&out.stdout).into_owned()
        + "\n--- stderr ---\n"
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "mvn failed at ranger {ranger_version} (this run also executes the four hand-written \
         anchor tests, so a failure here may be one of those):\n{log}"
    );
    // A green exit is not evidence the anchors ran; see this function's docs.
    assert!(
        log.lines().any(|l| l.contains("EgretRangerEnforcementTest")
            && l.contains("Tests run: 4, Failures: 0, Errors: 0, Skipped: 0")),
        "mvn exited 0 at ranger {ranger_version} but no line said `Tests run: 4, Failures: 0, \
         Errors: 0, Skipped: 0 ... in EgretRangerEnforcementTest`. The four hand-written anchors \
         did NOT run, so this harness is asserting agreement and nothing else — check \
         `tests/ranger/pom.xml`'s surefire <includes>.\n{log}"
    );

    let verdicts: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&verdicts_path).expect("read verdicts.json"))
            .expect("parse verdicts.json");
    assert_eq!(
        verdicts.len(),
        cases.len(),
        "ranger {ranger_version} answered {} of {} cases",
        verdicts.len(),
        cases.len()
    );
    verdicts
}

/// The engine's service-def as the corpus uses it, plus the parsed form and
/// the plugin config that goes with it.
fn load_engine(profile: &Profile) -> (Value, ServiceDef, EngineConfig) {
    let mut def_json: Value =
        serde_json::from_str(&std::fs::read_to_string(profile.def_path).expect("read service-def"))
            .expect("parse service-def");
    // The real captured download (`tests/ranger/fixtures/service-policies.json`)
    // carries no `contextEnrichers`, and the only one either def declares is a
    // tag enricher that would try to poll a Ranger admin from the test JVM.
    //
    // ⚠ FIX ROUND 1, MINOR 6: this comment used to continue *"the corpus never
    // generates a `tagPolicies` block, so the one thing the key now feeds —
    // `ServiceDef::has_tag_enricher` — is never consulted"*. The corpus does
    // generate one as of the commit that added the tag layer, so that sentence
    // described the corpus as it had been. The ACTION is still right, and for a
    // stronger reason spelled out in [`load_tag_def`]: an enricher on EITHER def
    // would make the Java half poll a Ranger admin AND overwrite the tag set
    // `EgretCaseRunner` seeds, while this crate read the seeded one — a
    // disagreement the harness manufactured rather than one found in Ranger.
    //
    // `has_tag_enricher` IS consulted now, and the direction is the safe one:
    // removing the key yields `None` (unknown), and `tag_policies_can_decide`
    // needs an explicit `Some(false)` on BOTH defs to skip the phase, so the
    // corpus gets the phase rather than a silent skip.
    if let Some(obj) = def_json.as_object_mut() {
        obj.remove("contextEnrichers");
    }
    let def = ServiceDef::parse(&def_json);
    assert!(
        def.issues().is_empty(),
        "{} service-def has issues the corpus cannot account for: {:?}",
        profile.ranger_version,
        def.issues()
    );

    let engine = EngineConfig {
        super_users: Some(BTreeSet::new()),
        super_groups: Some(BTreeSet::new()),
        engine: Some(profile.engine),
        plugin_version: Some(profile.plugin_version),
    };
    (def_json, def, engine)
}

/// The `tagPolicies` block's own service-def: egret's shipped `tag.json`,
/// verbatim but for `contextEnrichers`.
///
/// **Stripping the key is what keeps the two sides symmetric, and the
/// direction matters.** Ranger builds context enrichers from the resource def
/// AND from the tag block's own def and UNIONS the two lists, and Apache's
/// stock `ranger-servicedef-tag.json` — bundled inside `ranger-plugins-common`
/// itself — declares `RangerTagEnricher`. A def that declared one here would
/// make the Java half try to poll a Ranger admin from the test JVM and would
/// overwrite the tag set the runner seeds, while this crate read the seeded one:
/// a disagreement the harness manufactured.
///
/// `tag.json` happens to declare none today, so the `remove` is a no-op — and
/// it is written anyway, because the day it stops being one is the day this
/// harness would otherwise start measuring the enricher. On the crate's side
/// the absence reads as `has_tag_enricher() == None` (unknown), which does NOT
/// narrow: `tag_policies_can_decide` needs an explicit `Some(false)` on BOTH
/// defs to skip the phase, so the corpus gets the phase rather than a silent
/// skip.
fn load_tag_def() -> Value {
    let mut def_json: Value =
        serde_json::from_str(&std::fs::read_to_string(TAG_DEF_PATH).expect("read tag service-def"))
            .expect("parse tag service-def");
    if let Some(obj) = def_json.as_object_mut() {
        obj.remove("contextEnrichers");
    }
    // The phase refuses on any `ServiceDefIssue` this crate cannot account for,
    // so a def that did not parse cleanly would show up as an escape hundreds of
    // cases later with no hint of where it came from.
    let def = ServiceDef::parse(&def_json);
    assert!(
        def.issues().is_empty(),
        "tag.json has issues the corpus cannot account for: {:?}",
        def.issues()
    );
    def_json
}

fn run_profile(profile: &Profile, seed: u64, count: usize) -> Tally {
    let (def_json, def, engine) = load_engine(profile);
    let tag_def_json = load_tag_def();

    let mut rng = Rng::new(seed ^ (profile.ranger_version.len() as u64) ^ profile.engine as u64);
    let cases: Vec<Case> = (0..count)
        .map(|i| generate_case(&mut rng, profile, &def_json, &tag_def_json, i))
        .collect();

    // Coverage first, before a JVM is started or a single decision is taken:
    // if the corpus has stopped producing an axis, nothing downstream of here
    // means what it says.
    report_coverage(profile, &cases);

    // Then Rust, so a generator escape (an `Undecidable` the corpus is not
    // supposed to contain) is reported before a JVM is ever started.
    let mut outcomes = Vec::with_capacity(cases.len());
    // The reported ACCESS policy id per case, computed in the same pass so the
    // `ServicePolicySet` is parsed once (RULING 3). `None` for a non-ACCESS case
    // or a refusal.
    let mut ids: Vec<Option<Option<i64>>> = Vec::with_capacity(cases.len());
    let mut escapes: Vec<String> = Vec::new();
    for case in &cases {
        let set: ServicePolicySet =
            serde_json::from_value(case.service_policies.clone()).expect("parse generated set");
        let outcome = rust_outcome(case, &set, &def, &engine);
        if let Outcome::Refused(why) = &outcome {
            escapes.push(format!(
                "  {} (policyType {}): {why}",
                case.id, case.policy_type
            ));
        }
        ids.push(rust_access_policy_id(case, &set, &def, &engine));
        outcomes.push(outcome);
    }
    assert!(
        escapes.is_empty(),
        "seed {seed:#x}: the generated corpus produced {} Undecidable results other than \
         EvalOrderAmbiguous. The generator has drifted into constructs the evaluator declines \
         to answer, so the gate would be testing nothing:\n{}",
        escapes.len(),
        escapes.join("\n")
    );

    let verdicts = ask_ranger(
        &cases,
        profile.engine.service_def_name(),
        profile.ranger_version,
        "corpus",
    );

    let mut tally = Tally {
        cases: cases.len(),
        ..Tally::default()
    };
    let mut mismatches: Vec<String> = Vec::new();

    for (((case, mine), mine_id), verdict) in cases.iter().zip(&outcomes).zip(&ids).zip(&verdicts) {
        assert_eq!(
            verdict.get("id").and_then(Value::as_str),
            Some(case.id.as_str()),
            "verdicts are out of order"
        );
        if let Some(error) = verdict.get("error").and_then(Value::as_str) {
            mismatches.push(reproducer(
                case,
                seed,
                &format!("the Ranger engine threw: {error}"),
            ));
            continue;
        }

        if case.duel {
            tally.duels += 1;
        }
        if case.composed_duel {
            tally.composed_duels += 1;
        }
        if case.candidacy_duel {
            tally.candidacy_duels += 1;
        }
        if case.restore_duel {
            tally.restore_duels += 1;
        }
        if case.version_split_duel {
            tally.version_split_duels += 1;
        }
        if case.service_policies.get("tagPolicies").is_some() {
            tally.tagged += 1;
        }
        let population = Population::of(case);
        *tally.population.entry(population).or_default() += 1;

        if let Outcome::Declined(kind) = mine {
            *tally.declined.entry((*kind, population)).or_default() += 1;
            if case.candidacy_duel && *kind == Declined::ResourceCandidacy {
                tally.candidacy_duels_declined += 1;
            }
            if case.version_split_duel && *kind == Declined::TagDenyVersionSplit {
                tally.version_split_duels_declined += 1;
            }
            continue;
        }

        let theirs = java_outcome(case.policy_type, verdict);
        match mine {
            Outcome::Access { allowed: true } => tally.allowed += 1,
            Outcome::Access { allowed: false } => tally.denied += 1,
            Outcome::Mask { mask_type, .. } => {
                if mask_type.is_some() {
                    tally.masked += 1
                } else {
                    tally.no_mask += 1
                }
            }
            Outcome::RowFilter { filter_expr } => {
                if filter_expr.is_some() {
                    tally.filtered += 1
                } else {
                    tally.no_filter += 1
                }
            }
            Outcome::Declined(_) | Outcome::Refused(_) => unreachable!("handled above"),
        }
        tally.compared += 1;

        // ── the reported ACCESS policy id (RULING 3) ──────────────────────
        //
        // AFTER the verdict comparison and only when the verdicts AGREE. A case
        // whose allow/deny already disagrees will usually name a different policy
        // too, and reporting that as a second, undeclared id divergence turns one
        // finding into two and sends the reader to the wrong question.
        //
        // Its own axis, not part of `Outcome`: two of the divergences are
        // DECLARED, and folding them into the verdict comparison would either
        // flood the gate or force the comparison to be weakened. A mismatch that
        // matches none of the declared shapes lands in `mismatches` and fails the
        // run, which is what makes this an allow-list and not a relaxation.
        if let (Some(mine_id), true) = (*mine_id, *mine == theirs) {
            let theirs_id = java_access_policy_id(verdict);
            tally.ids_compared += 1;
            if mine_id == theirs_id {
                tally.ids_agreeing += 1;
            } else if let Some(reason) = classify_id_divergence(case, mine_id, theirs_id) {
                *tally.id_divergences.entry(reason).or_default() += 1;
            } else {
                mismatches.push(reproducer(
                    case,
                    seed,
                    &format!(
                        "the two sides agree on allow/deny and disagree on the reported \
                         ACCESS policy id, for a reason that is not on the declared list: \
                         egret named {mine_id:?}, ranger named {theirs_id:?}. The declared \
                         reasons are: {}",
                        ID_DIVERGENCES
                            .iter()
                            .map(|(what, where_, _)| format!("{what} (see {where_})"))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            }
        }

        if case.duel && *mine == theirs {
            tally.duels_agreeing += 1;
        }
        if case.composed_duel && *mine == theirs {
            tally.composed_duels_agreeing += 1;
        }
        if case.restore_duel && *mine == theirs {
            tally.restore_duels_agreeing += 1;
        }
        if *mine != theirs {
            mismatches.push(reproducer(
                case,
                seed,
                &format!("egret said {mine:?}\n  ranger said {theirs:?}\n  raw verdict: {verdict}"),
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "ranger {}: {} of {} compared cases DISAGREE with the real engine.\n\n{}",
        profile.ranger_version,
        mismatches.len(),
        tally.compared,
        // A differential failure without a reproducer is useless, but a
        // thousand-line dump is unreadable: the first few carry the full
        // policy JSON and the rest are named so they can be re-run.
        mismatches
            .iter()
            .take(3)
            .cloned()
            .chain((mismatches.len() > 3).then(|| format!("… and {} more", mismatches.len() - 3)))
            .collect::<Vec<_>>()
            .join("\n\n")
    );

    tally
}

/// Everything needed to re-run one failing case by hand.
fn reproducer(case: &Case, seed: u64, what: &str) -> String {
    format!(
        "case {} (policyType {}) — reproduce with EGRET_DIFF_SEED={seed:#x}\n  {what}\n  \
         request: user={} groups={:?} roles={:?} accessType={} resource={:?}\n  policies: {}",
        case.id,
        case.policy_type,
        case.request.user,
        case.request.groups,
        case.request.roles,
        case.request.access_type,
        case.request
            .resource
            .keys()
            .map(|k| (k, case.request.resource.get(k).unwrap_or("")))
            .collect::<BTreeMap<_, _>>(),
        serde_json::to_string_pretty(&case.service_policies["policies"])
            .unwrap_or_else(|e| e.to_string()),
    )
}

#[test]
fn rust_evaluator_agrees_with_the_real_ranger_engine() {
    if std::env::var("EGRET_TEST_RANGER_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_RANGER_E2E != 1 — skipping the Ranger differential gate");
        return;
    }
    let seed = std::env::var("EGRET_DIFF_SEED")
        .ok()
        .map(|s| {
            let s = s.trim();
            let s = s.strip_prefix("0x").unwrap_or(s);
            u64::from_str_radix(s, 16).expect("EGRET_DIFF_SEED must be hex")
        })
        .unwrap_or(DEFAULT_SEED);
    let count = std::env::var("EGRET_DIFF_CASES")
        .ok()
        .map(|s| s.trim().parse().expect("EGRET_DIFF_CASES must be a number"))
        .unwrap_or(DEFAULT_CASES);

    // The pins first: if a measured divergence no longer reproduces, the corpus
    // is narrower than the evaluator and must be widened before anything it
    // reports means what it says.
    let (pin_def_json, pin_def, pin_engine) = load_engine(&trino());
    check_pinned_divergences(&pin_def_json, &pin_def, &pin_engine);
    // …and the one shape the corpus is forbidden to carry, whose absence from it
    // would otherwise be indistinguishable from the behaviour having gone away.
    check_measured_tag_crash(&pin_def_json, &load_tag_def(), &pin_def, &pin_engine);
    // …and the whole-branch review's CRITICAL, which the corpus cannot reach
    // because `policy`'s `level_priority` holds the shape out on purpose.
    check_measured_priority_composite(&pin_def_json, &load_tag_def(), &pin_def, &pin_engine);

    let mut total = Tally::default();
    for profile in [trino(), starrocks()] {
        let tally = run_profile(&profile, seed, count);
        eprintln!(
            "differential: ranger {} — {} cases ({} with a tag phase), {} compared and \
             AGREEING, {} eval-order duels ({} agreeing) of which {} composed ({} agreeing); \
             allowed {} / denied {} / masked {} / no-mask {} / filtered {} / no-filter {}",
            profile.ranger_version,
            tally.cases,
            tally.tagged,
            tally.compared,
            tally.duels,
            tally.duels_agreeing,
            tally.composed_duels,
            tally.composed_duels_agreeing,
            tally.allowed,
            tally.denied,
            tally.masked,
            tally.no_mask,
            tally.filtered,
            tally.no_filter,
        );
        report_rates(&format!("ranger {}", profile.ranger_version), &tally);
        check_version_split_is_asymmetric(&profile, &tally);
        total.absorb(&tally);
    }

    eprintln!(
        "differential: seed {seed:#x}, {} cases across both engines ({} with a tag phase), \
         {} compared, {} eval-order duels of which {} agree; {} composed duels of which {} agree",
        total.cases,
        total.tagged,
        total.compared,
        total.duels,
        total.duels_agreeing,
        total.composed_duels,
        total.composed_duels_agreeing,
    );
    report_rates("both engines", &total);

    // The duels are the only cases that could falsify Task 5's claim that the
    // allow/deny outcome does not depend on `getEvalOrder()` within one band.
    // If the generator stops producing them, the claim stops being tested and
    // nothing else says so.
    assert!(
        total.duels * 40 >= total.cases,
        "only {} deliberate eval-order duels in {} cases — the shape that could falsify \
         Task 5's eval-order claim has stopped being generated",
        total.duels,
        total.cases
    );
    // …and the COMPOSED duel is the only shape in which `getEvalOrder()` can
    // decide an ACCESS outcome at all, so the composed rate below is a rate
    // over a population this floor is what keeps populated.
    assert!(
        total.composed_duels * 60 >= total.cases,
        "only {} composed eval-order duels in {} cases — the one shape in which \
         getEvalOrder() can decide an ACCESS outcome has stopped being generated, so the \
         composed EvalOrderAmbiguous rate below is a rate over nothing",
        total.composed_duels,
        total.cases
    );
    // …and the candidacy duel, whose whole purpose is to give offset 553's
    // refusal a denominator. Without this floor a generator edit that stopped
    // producing it would print `0/0` and read as "this refusal never fires".
    assert!(
        total.candidacy_duels * 80 >= total.cases,
        "only {} deliberate candidacy duels in {} cases — the shape that reaches offset 553's \
         candidate-set refusal has stopped being generated, so its rate is a rate over nothing",
        total.candidacy_duels,
        total.cases
    );
    // …and it must actually REACH the refusal. A duel that stopped refusing
    // would mean the shape had drifted, and the rate would then be measuring a
    // shape that is no longer the one described.
    assert!(
        total.candidacy_duels_declined * 2 >= total.candidacy_duels,
        "only {} of {} deliberate candidacy duels reached the candidate-set refusal — the \
         four-way conjunction the shape is built around has drifted, and the rate below is \
         no longer a rate of what it says",
        total.candidacy_duels_declined,
        total.candidacy_duels
    );
    // …and the restore duel, which is the ONLY shape that reaches T3's skip
    // (RULING 1: reverting that fix left the gate green because nothing generated
    // this). Two floors, for the same reason the candidacy duel has two: that it
    // is still generated, and that it still ANSWERS — a restore duel that started
    // REFUSING would stop testing the restore and the floor above would not say so.
    assert!(
        total.restore_duels * 100 >= total.cases,
        "only {} deliberate restore duels in {} cases — the only shape that reaches the \
         trie-excluded-view skip has stopped being generated, and reverting that fix would \
         leave this gate green",
        total.restore_duels,
        total.cases
    );
    assert!(
        total.restore_duels_agreeing * 10 >= total.restore_duels * 9,
        "only {} of {} restore duels AGREED with the engine — they are supposed to be \
         ANSWERED, and a duel that refuses is a duel that no longer tests the restore",
        total.restore_duels_agreeing,
        total.restore_duels
    );
    // A corpus with no tag phase reports two composed rates of 0/0 and looks
    // exactly like one where the tag generator was deleted.
    assert!(
        total.tagged * 5 >= total.cases,
        "only {} of {} cases carry a tagPolicies block — the tag phase has stopped being \
         generated and every composed measurement below is a rate over nothing",
        total.tagged,
        total.cases
    );

    // ── the CEILING on refusal, which this file did not have (FIX ROUND 1) ──
    //
    // The review injected a 42% over-refusal and the gate stayed GREEN, because
    // every floor in this file is a FLOOR and the thing an over-refusal shrinks is
    // the denominator. Two assertions close it, and they answer different
    // questions:
    //
    // * the per-kind ceilings catch a single refusal widening — which is what a
    //   narrowing "fix" to the evaluator looks like from outside;
    // * the compared FRACTION catches a refusal spread thinly across every kind,
    //   which no per-kind ceiling can see.
    //
    // ⚠ The per-kind ceilings are 1.6× measured, NOT "deliberately loose" as this
    // comment used to say — at 2× a doubling lands on the ceiling and passes, which
    // is what the whole-branch review proved by injection. The compared FRACTION
    // below stays loose, because it is the one that has to absorb every kind's
    // ordinary variation at once. See the ⚠ block on `DECLINED_CEILINGS` for what
    // that costs at small counts and why it is accepted.
    assert_eq!(
        DECLINED_KINDS.len(),
        DECLINED_CEILINGS.len(),
        "every Declined kind needs a ceiling, or a refusal can widen unbounded in \
         whichever kind was left out"
    );
    let mut over: Vec<String> = Vec::new();
    for (kind, ceiling) in DECLINED_CEILINGS {
        assert!(
            DECLINED_KINDS.contains(kind),
            "DECLINED_CEILINGS names {} which is not in DECLINED_KINDS",
            kind.label()
        );
        let count = total.declined_total(*kind);
        let per_mille = 1000 * count / total.cases.max(1);
        if per_mille > *ceiling as usize {
            over.push(format!(
                "  {} — {count} cases ({per_mille}‰), ceiling {ceiling}‰",
                kind.label()
            ));
        }
    }
    assert!(
        over.is_empty(),
        "{} refusal(s) fired far more often than when this ceiling was measured. An \
         over-refusal costs every one of those cases an answer and produces no other \
         failing test, which is why it has a ceiling and not just a printed rate. If the \
         widening is intended, re-measure and move the ceiling in the same commit:\n{}",
        over.len(),
        over.join("\n")
    );
    // 75% of cases, against ~88% measured — and the re-review bisected the
    // threshold exactly: an over-refusal spread thinly under every per-kind ceiling
    // FAILS here at 73.22% and PASSES at 75.17%. So the slack is real and worth
    // knowing: **roughly twice today's refusal rate, spread evenly across all five
    // kinds, still passes.** The per-kind ceilings are the sharp instrument (any
    // single kind doubling fails there); this is the backstop for the one shape
    // they cannot see, and it is set loose on purpose because a ceiling that flaps
    // gets deleted.
    assert!(
        total.compared * 4 >= total.cases * 3,
        "only {} of {} cases were compared at all ({:.1}%). The crate is refusing most of \
         the corpus, so agreement on the rest says very little — and every floor below is \
         measured against `compared`, so they CANNOT see this.",
        total.compared,
        total.cases,
        100.0 * total.compared as f64 / total.cases as f64
    );

    // ── the ACCESS policy-id axis (RULING 3) ───────────────────────────────
    //
    // A floor, because the axis is only a gate while cases reach it.
    assert!(
        total.ids_compared * 4 >= total.cases,
        "only {} of {} cases compared a reported ACCESS policy id — the axis RULING 3 \
         added is not reaching anything",
        total.ids_compared,
        total.cases
    );
    // ⚠ FIX ROUND 2. …and a floor on EACH DECLARED divergence, which the first
    // version admitted it lacked: a regression that stopped producing one of them
    // was green and silent, and a declared divergence nothing produces is a row in
    // an allow-list nobody is checking. These are the floors that make the
    // allow-list's narrowing above meaningful — narrowing a class nothing reaches
    // proves nothing.
    let mut unreached: Vec<String> = Vec::new();
    for (reason, _, floor) in ID_DIVERGENCES {
        let count = total.id_divergences.get(reason).copied().unwrap_or(0);
        let per_mille = 1000 * count / total.cases.max(1);
        if per_mille < *floor as usize {
            unreached.push(format!(
                "  {reason} — {count} cases ({per_mille}‰), floor {floor}‰"
            ));
        }
    }
    assert!(
        unreached.is_empty(),
        "{} DECLARED policy-id divergence(s) have stopped being produced. Each is an \
         allow-list entry, so one that nothing reaches is a class the gate would admit \
         without ever having seen it:\n{}",
        unreached.len(),
        unreached.join("\n")
    );

    // A corpus where nothing ever matches agrees with Ranger perfectly and
    // proves nothing. These floors are not thresholds on Ranger's behaviour —
    // they are a guard on the GENERATOR, so a change that makes every policy
    // miss cannot pass as green.
    //
    // ⚠ The denominator is `compared`, not `cases` — CHANGED in SP4d-2b Task 5,
    // and the reason is not that it was failing. A case the crate REFUSES is not
    // compared: it can neither agree nor disagree, so it is not part of the
    // population these floors are about. Measuring against every case made the
    // floors tighten as the refusal rate rose, for reasons that have nothing to
    // do with whether policies match — and the tag phase raised that rate from
    // ~2% to ~20%. The floors themselves are unchanged (1 in 20, 1 in 40).
    assert!(
        total.allowed * 20 >= total.compared,
        "corpus degenerate: only {} allows in {} compared cases",
        total.allowed,
        total.compared
    );
    assert!(
        total.denied * 20 >= total.compared,
        "corpus degenerate: only {} denies in {} compared cases",
        total.denied,
        total.compared
    );
    assert!(
        total.masked * 40 >= total.compared,
        "corpus degenerate: only {} masks in {} compared cases",
        total.masked,
        total.compared
    );
    assert!(
        total.filtered * 40 >= total.compared,
        "corpus degenerate: only {} row filters in {} compared cases",
        total.filtered,
        total.compared
    );
}

/// SP4d-3 Task 7 fix round 1 (review I-5): the PREPARED path decides every
/// corpus case exactly as the unprepared one — full results, all three
/// questions, whatever the case's own policy type. Runs WITHOUT the JVM (the
/// corpus generator needs none), on every `cargo test`.
///
/// One prepared set answers MANY requests in the gate, so each set is
/// prepared once and asked the requests of the next `REUSE` cases too, then
/// its own request again last — a matcher that carried state from one request
/// to the next would show up as a difference on the repeat.
#[test]
fn the_prepared_path_decides_every_corpus_case_exactly_as_the_unprepared_one() {
    use egret_ranger_eval::eval::{
        evaluate_data_mask_prepared, evaluate_prepared, evaluate_row_filter_prepared,
        PreparedPolicySet,
    };
    const REUSE: usize = 4;
    let mut compared = 0usize;
    for profile in [trino(), starrocks()] {
        let (def_json, def, engine) = load_engine(&profile);
        let tag_def_json = load_tag_def();
        let mut rng =
            Rng::new(DEFAULT_SEED ^ (profile.ranger_version.len() as u64) ^ profile.engine as u64);
        let cases: Vec<Case> = (0..DEFAULT_CASES)
            .map(|i| generate_case(&mut rng, &profile, &def_json, &tag_def_json, i))
            .collect();
        for (i, case) in cases.iter().enumerate() {
            let set: ServicePolicySet =
                serde_json::from_value(case.service_policies.clone()).expect("parse generated set");
            let prepared = PreparedPolicySet::new(&set, &def);
            let requests = cases[i..cases.len().min(i + 1 + REUSE)]
                .iter()
                .map(|c| &c.request)
                .chain(std::iter::once(&case.request));
            for request in requests {
                assert_eq!(
                    evaluate(&set, request, &def, &engine),
                    evaluate_prepared(&prepared, request, &engine),
                    "access: {} against {request:?}",
                    case.id
                );
                assert_eq!(
                    evaluate_data_mask(&set, request, &def, &engine),
                    evaluate_data_mask_prepared(&prepared, request, &engine),
                    "mask: {} against {request:?}",
                    case.id
                );
                assert_eq!(
                    evaluate_row_filter(&set, request, &def, &engine),
                    evaluate_row_filter_prepared(&prepared, request, &engine),
                    "row filter: {} against {request:?}",
                    case.id
                );
                compared += 3;
            }
        }
    }
    eprintln!("prepared == unprepared on {compared} evaluations");
    assert!(compared >= 2 * DEFAULT_CASES * 3, "{compared}");
}

// ── SP4d-3 Task 8: migrations proven against the REAL engines ───────────
//
// The grammar (`egret_perm_compile::grammar`) turns a hand-written Ranger
// policy into permission-model objects or names why it cannot. The equivalence
// gate then checks the migration with THIS crate's evaluator — Rust checking
// Rust. Nothing in that chain has been asked of Apache Ranger itself, and the
// finding this programme keeps reproducing is that only the real engine finds
// the defects. So this corpus asks it, and trusts none of the Rust under test:
//
// * **before** = a service policy set holding the hand-written policy (plus a
//   background the probes do not target, so the policy is decided inside a
//   set and not alone);
// * **after** = the same set with the original REMOVED — the download serves
//   only enabled policies (`enabled_policies_for_service`), so a switched-off
//   original is absent from what the engine receives — and the grammar's
//   output compiled by egret's REAL compiler (`compile_grant` +
//   `access_to_ranger_json`, `compile_rule`), routed by the compiled body's own
//   `service` field exactly as `trigger::target_service_id` routes it;
// * both are sent to `EgretCaseRunner` as two cases with paired ids, at 2.5.0
//   AND 2.8.0 for both engines' corpora, and compared pairwise here.
//
// **The corpus is enumerated from the service-defs.** Every resource hierarchy
// prefix the def declares × every access type it declares is generated as a
// single-access policy, so `create` / `drop` / `alter` / `execute` and the
// non-catalog hierarchies are REACHED and refused, not merely imagined; the
// DML access types are generated as their full power set, so every subset of
// an action's expansion is generated, not just the ones the author thought of.
// On top of that walk come the shapes the grammar SPLITS (allow + deny +
// exceptions, two items sharing one exception, several catalogs → N grants),
// every mask type either def declares, row filters, and tag-addressed masks and
// filters whose key equals their name.
//
// **Probes are not derived from the policy's vocabulary alone** (a correlated
// oracle is not an oracle): every probe set carries a sibling value at every
// level, the ancestors of the policy's coordinate, a user no policy names, case
// variants, and — at the policy's own coordinate — every access type the def
// declares.

const MIG_WH: Uuid = Uuid::from_u128(0x5d03_0008_0000_0000_0000_0000_0000_0001);
const MIG_LAKE: Uuid = Uuid::from_u128(0x5d03_0008_0000_0000_0000_0000_0000_0002);

/// The catalog registry the grammar is given: `wh` and `lake` exist, nothing
/// else does (so `ext` and `*` are unregistered).
fn mig_catalog_id(name: &str) -> Option<Uuid> {
    match name {
        "wh" => Some(MIG_WH),
        "lake" => Some(MIG_LAKE),
        _ => None,
    }
}

fn mig_catalog_name(id: Uuid) -> &'static str {
    match id {
        MIG_WH => "wh",
        MIG_LAKE => "lake",
        other => panic!("the grammar produced a catalog id the registry never issued: {other}"),
    }
}

/// The tag registry, `tags.key` → `tags.name`. `phi` is the one migratable
/// shape (key == name); `pii`'s name differs from its key.
fn mig_tag_name(key: &str) -> Option<String> {
    match key {
        "phi" => Some("phi".to_string()),
        "pii" => Some("Sensitive".to_string()),
        _ => None,
    }
}

const MIG_ORIGINAL_ID: i64 = 10;
const MIG_COMPILED_ID_BASE: i64 = 100;
/// A value at one level that no generated policy names.
const MIG_SIBLING: &str = "egret-probe-other";
/// The background's only principal. No probe uses it, so the background never
/// decides a probe; it is there so the policy is evaluated inside a SET.
const MIG_BG_USER: &str = "egret-bg-user";
/// Cases per Maven run. Every case carries its own service-def (~10 KB of
/// JSON), so the file is chunked rather than handed over whole.
const MIG_CHUNK: usize = 8000;

/// Every probe is issued for each of these. The corpus's policies name
/// `alice`, `bob`, `erin`, `analysts`, `contractors` and `public`; the list
/// adds a user no policy names, a case variant of a named user, and a user who
/// is both excepted by name and a member of an included group.
const MIG_PROBE_PRINCIPALS: &[(&str, &[&str])] = &[
    ("alice", &[]),
    ("Alice", &[]),
    ("bob", &[]),
    ("erin", &["analysts"]),
    ("egret-probe-member-of-analysts", &["analysts"]),
    ("egret-probe-member-of-contractors", &["contractors"]),
    ("egret-probe-unnamed-user", &[]),
];

/// The principals asked at the coordinates AROUND a policy's own (its
/// ancestors and siblings): the list above minus the case variant and the
/// contractors member, which only the policy's own coordinate needs. A cost
/// cut — every case carries a whole service-def through the JVM — taken where
/// the question is "which coordinates does it cover", not "whom".
const MIG_AROUND_PRINCIPALS: &[(&str, &[&str])] = &[
    ("alice", &[]),
    ("bob", &[]),
    ("erin", &["analysts"]),
    ("egret-probe-member-of-analysts", &["analysts"]),
    ("egret-probe-unnamed-user", &[]),
];

/// The verdict fields compared between before and after.
///
/// **`policyId` is deliberately NOT compared.** A migration replaces the
/// hand-written policy (id [`MIG_ORIGINAL_ID`]) with compiled ones (ids from
/// [`MIG_COMPILED_ID_BASE`]), so on every probe the original decided, the
/// deciding policy's id differs by construction — comparing it would fail every
/// correct migration and say nothing about the decision. It is still READ: the
/// reach check below requires the original to have decided at least one probe
/// before, and a compiled policy at least one after.
const MIG_COMPARED_FIELDS: &[&str] = &[
    "isAllowed",
    "isAccessDetermined",
    "maskType",
    "maskedValue",
    "maskCondition",
    "filterExpr",
];

struct MigPolicy {
    label: String,
    body: Value,
    service: ServiceKind,
}

/// What the grammar made of one policy.
enum MigAccepted {
    Grants(Vec<Grant>),
    Rules(Vec<Rule>),
}

fn mig_perm_engine(profile: &Profile) -> PermEngine {
    match profile.engine {
        EngineType::Trino => PermEngine::Trino,
        EngineType::StarRocks => PermEngine::StarRocks,
    }
}

fn mig_level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

fn mig_accesses(accesses: &[&str]) -> Value {
    Value::Array(
        accesses
            .iter()
            .map(|a| json!({"type": a, "isAllowed": true}))
            .collect(),
    )
}

fn mig_item(users: &[&str], groups: &[&str], accesses: &[&str]) -> Value {
    json!({"users": users, "groups": groups, "roles": [], "accesses": mig_accesses(accesses)})
}

/// A resources map over the engine's catalog hierarchy, root first; `levels`
/// may be shorter than the hierarchy.
fn mig_path(profile: &Profile, levels: &[&[&str]]) -> Value {
    let mut m = Map::new();
    for (name, values) in profile.hierarchy.iter().zip(levels) {
        m.insert((*name).to_string(), mig_level(values));
    }
    Value::Object(m)
}

fn mig_full(profile: &Profile, schema: &str, table: &str, column: &str) -> Value {
    mig_path(profile, &[&["wh"], &[schema], &[table], &[column]])
}

fn mig_access(label: String, resources: Value, buckets: [Vec<Value>; 4]) -> MigPolicy {
    let [allow, deny, allow_exc, deny_exc] = buckets;
    MigPolicy {
        label,
        service: ServiceKind::Resource,
        body: json!({
            "name": "hand-written",
            "policyType": 0,
            "isEnabled": true,
            "resources": resources,
            "policyItems": allow,
            "denyPolicyItems": deny,
            "allowExceptions": allow_exc,
            "denyExceptions": deny_exc,
        }),
    }
}

fn mig_allow(label: String, resources: Value, item: Value) -> MigPolicy {
    mig_access(label, resources, [vec![item], vec![], vec![], vec![]])
}

fn mig_mask_item(users: &[&str], groups: &[&str], mask_type: &str, value: Option<&str>) -> Value {
    let mut info = json!({"dataMaskType": mask_type});
    if let Some(v) = value {
        info["valueExpr"] = json!(v);
    }
    json!({"users": users, "groups": groups, "roles": [], "accesses": mig_accesses(&["select"]),
           "dataMaskInfo": info})
}

fn mig_filter_item(users: &[&str], groups: &[&str], expr: Option<&str>) -> Value {
    let mut item = json!({"users": users, "groups": groups, "roles": [],
                          "accesses": mig_accesses(&["select"])});
    if let Some(e) = expr {
        item["rowFilterInfo"] = json!({"filterExpr": e});
    }
    item
}

fn mig_rule(label: String, policy_type: i32, resources: Value, items: Vec<Value>) -> MigPolicy {
    let service = if resources.get("tag").is_some() {
        ServiceKind::Tag
    } else {
        ServiceKind::Resource
    };
    let bucket = if policy_type == 1 {
        "dataMaskPolicyItems"
    } else {
        "rowFilterPolicyItems"
    };
    let mut body = json!({
        "name": "hand-written",
        "policyType": policy_type,
        "isEnabled": true,
        "resources": resources,
        "policyItems": [],
        "denyPolicyItems": [],
    });
    body[bucket] = Value::Array(items);
    MigPolicy {
        label,
        body,
        service,
    }
}

/// A mask that the default value of a `CUSTOM` mask needs.
fn mig_mask_value(mask_type: &str) -> Option<&'static str> {
    (mask_type == "CUSTOM").then_some("concat('x', {col})")
}

fn mig_def_names(def: &Value, pointer: &str) -> Vec<String> {
    def.pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("service-def has no {pointer}"))
        .iter()
        .map(|t| t["name"].as_str().expect("named entry").to_string())
        .collect()
}

/// Every hierarchy PREFIX the def declares: one path per resource, from its
/// root down to it (each resource has one parent, so this is every node of
/// every hierarchy exactly once).
fn mig_hierarchy_paths(def: &Value) -> Vec<Vec<String>> {
    let resources = def["resources"].as_array().expect("resources");
    let parent: BTreeMap<String, String> = resources
        .iter()
        .map(|r| {
            (
                r["name"].as_str().expect("name").to_string(),
                r["parent"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    resources
        .iter()
        .map(|r| {
            let mut path = vec![r["name"].as_str().expect("name").to_string()];
            while let Some(p) = parent.get(path.last().expect("non-empty")) {
                if p.is_empty() {
                    break;
                }
                path.push(p.clone());
            }
            path.reverse();
            path
        })
        .collect()
}

fn mig_walk_value(level: &str) -> String {
    match level {
        "catalog" => "wh".to_string(),
        "schema" | "database" => "sales".to_string(),
        "table" => "orders".to_string(),
        "column" => "email".to_string(),
        other => format!("{other}_x"),
    }
}

/// The corpus for one engine. See the section header for what it covers.
fn mig_corpus(profile: &Profile, def: &Value, all_mask_types: &[String]) -> Vec<MigPolicy> {
    let mut out = Vec::new();
    let access_types = mig_def_names(def, "/accessTypes");
    let full = |s: &str, t: &str, c: &str| mig_full(profile, s, t, c);
    let base = || full("sales", "orders", "email");

    // A. The walk: every hierarchy prefix × every access type, one each.
    for path in mig_hierarchy_paths(def) {
        let mut res = Map::new();
        for level in &path {
            res.insert(level.clone(), mig_level(&[&mig_walk_value(level)]));
        }
        for at in &access_types {
            out.push(mig_allow(
                format!("walk {} [{at}]", path.join(".")),
                Value::Object(res.clone()),
                mig_item(&["alice"], &[], &[at]),
            ));
        }
    }

    // B. The power set of the DML access types the def declares — every
    // subset of every action's expansion — and the whole DML set plus each
    // non-DML type.
    let dml: Vec<&str> = ["select", "insert", "update", "delete"]
        .into_iter()
        .filter(|a| access_types.iter().any(|t| t == a))
        .collect();
    for bits in 1u32..(1 << dml.len()) {
        let set: Vec<&str> = dml
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, a)| *a)
            .collect();
        out.push(mig_allow(
            format!("dml {}", set.join("+")),
            base(),
            mig_item(&["alice"], &[], &set),
        ));
    }
    for extra in access_types.iter().filter(|t| !dml.contains(&t.as_str())) {
        let mut set = dml.clone();
        set.push(extra);
        out.push(mig_allow(
            format!("dml+{extra}"),
            base(),
            mig_item(&["alice"], &[], &set),
        ));
    }

    // C. Principals, on the full DML set.
    let principal_shapes: Vec<(&str, Value)> = vec![
        (
            "two users",
            json!({"users": ["alice", "bob"], "groups": [], "roles": []}),
        ),
        (
            "a group",
            json!({"users": [], "groups": ["analysts"], "roles": []}),
        ),
        (
            "public",
            json!({"users": [], "groups": ["public"], "roles": []}),
        ),
        (
            "user and group",
            json!({"users": ["alice"], "groups": ["contractors"], "roles": []}),
        ),
        (
            "{USER}",
            json!({"users": ["{USER}"], "groups": [], "roles": []}),
        ),
        (
            "a role",
            json!({"users": ["alice"], "groups": [], "roles": ["r_a"]}),
        ),
        ("nobody", json!({"users": [], "groups": [], "roles": []})),
    ];
    for (what, p) in principal_shapes {
        let mut item = p;
        item["accesses"] = mig_accesses(&dml);
        out.push(mig_allow(format!("principals {what}"), base(), item));
    }

    // D. Scopes, each for `select` and for the full DML set.
    let p = profile;
    for (acc_label, accs) in [("select", vec!["select"]), ("dml", dml.clone())] {
        let scopes: Vec<(&str, Value)> = vec![
            ("schema-wide", full("sales", "*", "*")),
            ("catalog-wide", full("*", "*", "*")),
            ("table-wide", full("sales", "orders", "*")),
            ("a ** run", full("sales", "**", "*")),
            ("wildcard characters", full("sal*", "ord?rs", "*")),
            ("mixed case", full("Sales", "ORDERS", "Email")),
            ("wildcard over named", full("*", "orders", "email")),
            (
                "two catalogs",
                mig_path(p, &[&["wh", "lake"], &["sales"], &["orders"], &["email"]]),
            ),
            (
                "unregistered catalog",
                mig_path(p, &[&["ext"], &["sales"], &["orders"], &["email"]]),
            ),
            (
                "registered + unregistered",
                mig_path(p, &[&["wh", "ext"], &["sales"], &["orders"], &["*"]]),
            ),
            ("catalog *", mig_path(p, &[&["*"], &["*"], &["*"], &["*"]])),
            (
                "two tables",
                mig_path(p, &[&["wh"], &["sales"], &["orders", "items"], &["*"]]),
            ),
            (
                "empty value",
                mig_path(p, &[&["wh"], &["sales"], &[""], &["*"]]),
            ),
            (
                "no values",
                mig_path(p, &[&["wh"], &["sales"], &[], &["*"]]),
            ),
        ];
        for (what, res) in scopes {
            out.push(mig_allow(
                format!("scope {acc_label} {what}"),
                res,
                mig_item(&["alice"], &["analysts"], &accs),
            ));
        }
        for flag in ["isExcludes", "isRecursive"] {
            let mut res = base();
            res[profile.hierarchy[2]][flag] = json!(true);
            out.push(mig_allow(
                format!("scope {acc_label} {flag}"),
                res,
                mig_item(&["alice"], &[], &accs),
            ));
        }
    }

    // E. The shapes the grammar SPLITS or folds.
    let sel = &["select"][..];
    let write: Vec<&str> = dml.iter().copied().filter(|a| *a != "select").collect();
    out.push(mig_access(
        "split allow + exception".into(),
        base(),
        [
            vec![mig_item(&[], &["analysts"], sel)],
            vec![],
            vec![mig_item(&["erin"], &[], sel)],
            vec![],
        ],
    ));
    out.push(mig_access(
        "split allow + deny + deny exception".into(),
        base(),
        [
            vec![mig_item(&["alice"], &["analysts"], &dml)],
            vec![mig_item(&["bob"], &["analysts"], &write)],
            vec![],
            vec![mig_item(&["erin"], &[], &write)],
        ],
    ));
    out.push(mig_access(
        "split allow + exception + deny + deny exception".into(),
        base(),
        [
            vec![mig_item(&[], &["analysts", "contractors"], &dml)],
            vec![mig_item(
                &["egret-probe-member-of-contractors"],
                &[],
                &write,
            )],
            vec![mig_item(&["erin"], &[], &dml)],
            vec![mig_item(&["alice"], &[], &write)],
        ],
    ));
    out.push(mig_access(
        "split two items share one exception".into(),
        base(),
        [
            vec![
                mig_item(&["alice"], &[], sel),
                mig_item(&[], &["analysts"], sel),
            ],
            vec![],
            vec![mig_item(&["erin"], &[], sel)],
            vec![],
        ],
    ));
    out.push(mig_access(
        "split two deny items share one exception".into(),
        base(),
        [
            vec![mig_item(&[], &["public"], &dml)],
            vec![
                mig_item(&["bob"], &[], &write),
                mig_item(&[], &["analysts"], &write),
            ],
            vec![],
            vec![mig_item(&["erin"], &[], &write)],
        ],
    ));
    out.push(mig_access(
        "split two items, exception access set differs".into(),
        base(),
        [
            vec![
                mig_item(&["alice"], &[], sel),
                mig_item(&[], &["analysts"], &dml),
            ],
            vec![],
            vec![mig_item(&["erin"], &[], sel)],
            vec![],
        ],
    ));
    out.push(mig_access(
        "split two exceptions".into(),
        base(),
        [
            vec![mig_item(&[], &["analysts"], sel)],
            vec![],
            vec![mig_item(&["erin"], &[], sel), mig_item(&["bob"], &[], sel)],
            vec![],
        ],
    ));
    out.push(mig_access(
        "split deny only".into(),
        base(),
        [vec![], vec![mig_item(&["bob"], &[], sel)], vec![], vec![]],
    ));
    out.push(mig_access(
        "split orphan exception".into(),
        base(),
        [vec![], vec![], vec![mig_item(&["erin"], &[], sel)], vec![]],
    ));
    out.push(mig_access(
        "split two catalogs with deny + exceptions".into(),
        mig_path(p, &[&["wh", "lake"], &["sales"], &["*"], &["*"]]),
        [
            vec![mig_item(&[], &["analysts"], &dml)],
            vec![mig_item(&["bob"], &[], &write)],
            vec![mig_item(&["erin"], &[], &dml)],
            vec![mig_item(&["alice"], &[], &write)],
        ],
    ));
    out.push(mig_access(
        "split one item, two actions".into(),
        base(),
        [
            vec![
                mig_item(&["alice"], &[], &dml),
                mig_item(&["bob"], &[], sel),
            ],
            vec![],
            vec![],
            vec![],
        ],
    ));

    // F. Decision-relevant header fields and unmodelled constructs.
    let headers: Vec<(&str, &str, Value)> = vec![
        ("disabled", "/isEnabled", json!(false)),
        ("priority 1", "/policyPriority", json!(1)),
        ("deny-all-else", "/isDenyAllElse", json!(true)),
        (
            "validity schedule",
            "/validitySchedules",
            json!([{"startTime": "2020/01/01 00:00:00", "endTime": "2099/01/01 00:00:00"}]),
        ),
        (
            "policy condition",
            "/conditions",
            json!([{"type": "ip-range", "values": ["10.0.0.0/8"]}]),
        ),
        ("zone", "/zoneName", json!("z1")),
        (
            "additional resources",
            "/additionalResources",
            json!([mig_full(p, "hr", "people", "*")]),
        ),
        ("options", "/options", json!({"k": "v"})),
        ("unknown field", "/egretUnknown", json!(1)),
        (
            "item delegateAdmin",
            "/policyItems/0/delegateAdmin",
            json!(true),
        ),
        (
            "item condition",
            "/policyItems/0/conditions",
            json!([{"type": "ip-range", "values": ["10.0.0.0/8"]}]),
        ),
        (
            "access isAllowed false",
            "/policyItems/0/accesses/0/isAllowed",
            json!(false),
        ),
        (
            "resource expression",
            "/resources/table/values",
            json!(["${{USER.dept}}"]),
        ),
    ];
    for (what, pointer, v) in headers {
        let mut pol = mig_allow(
            format!("header {what}"),
            base(),
            mig_item(&["alice"], &[], sel),
        );
        let (parent, key) = pointer.rsplit_once('/').expect("pointer");
        // `/resources/table/...` names Trino's level; on StarRocks `table` is
        // the same name, so the pointer holds for both.
        let target = if parent.is_empty() {
            &mut pol.body
        } else {
            pol.body
                .pointer_mut(parent)
                .unwrap_or_else(|| panic!("{pointer}"))
        };
        target[key] = v;
        out.push(pol);
    }

    // G. Masks: every mask type either def declares, plus the shapes the rule
    // grammar must refuse.
    for t in all_mask_types {
        let v = mig_mask_value(t);
        out.push(mig_rule(
            format!("mask {t}"),
            1,
            base(),
            vec![mig_mask_item(&[], &["public"], t, v)],
        ));
        out.push(mig_rule(
            format!("mask {t} with exemption"),
            1,
            base(),
            vec![
                mig_mask_item(&["alice"], &["analysts"], "MASK_NONE", None),
                mig_mask_item(&[], &["public"], t, v),
            ],
        ));
    }
    let hash = |users: &[&str], groups: &[&str]| mig_mask_item(users, groups, "MASK_HASH", None);
    let masks: Vec<(&str, Value, Vec<Value>)> = vec![
        (
            "stops at table",
            mig_path(p, &[&["wh"], &["sales"], &["orders"]]),
            vec![hash(&[], &["public"])],
        ),
        (
            "stops at schema",
            mig_path(p, &[&["wh"], &["sales"]]),
            vec![hash(&[], &["public"])],
        ),
        (
            "schema-wide",
            full("sales", "*", "*"),
            vec![hash(&[], &["public"])],
        ),
        (
            "wildcard over named",
            full("*", "orders", "email"),
            vec![hash(&[], &["public"])],
        ),
        (
            "two catalogs",
            mig_path(p, &[&["wh", "lake"], &["sales"], &["orders"], &["email"]]),
            vec![hash(&[], &["public"])],
        ),
        (
            "unregistered catalog",
            mig_path(p, &[&["ext"], &["sales"], &["orders"], &["email"]]),
            vec![hash(&[], &["public"])],
        ),
        (
            "two columns",
            mig_path(p, &[&["wh"], &["sales"], &["orders"], &["email", "phone"]]),
            vec![hash(&[], &["public"])],
        ),
        (
            "skips a level",
            mig_path(p, &[&["wh"], &["sales"]])
                .as_object()
                .cloned()
                .map(|mut m| {
                    m.insert(p.hierarchy[3].to_string(), mig_level(&["email"]));
                    Value::Object(m)
                })
                .expect("map"),
            vec![hash(&[], &["public"])],
        ),
        ("per-principal only", base(), vec![hash(&["alice"], &[])]),
        (
            "two masks",
            base(),
            vec![
                hash(&["alice"], &[]),
                mig_mask_item(&[], &["public"], "MASK_NULL", None),
            ],
        ),
        (
            "exemption after everyone",
            base(),
            vec![
                hash(&[], &["public"]),
                mig_mask_item(&["alice"], &[], "MASK_NONE", None),
            ],
        ),
        (
            "public exemption",
            base(),
            vec![
                mig_mask_item(&[], &["public"], "MASK_NONE", None),
                hash(&[], &["public"]),
            ],
        ),
        (
            "three items",
            base(),
            vec![
                mig_mask_item(&["alice"], &[], "MASK_NONE", None),
                mig_mask_item(&["bob"], &[], "MASK_NONE", None),
                hash(&[], &["public"]),
            ],
        ),
        (
            "value on a non-custom mask",
            base(),
            vec![mig_mask_item(&[], &["public"], "MASK_HASH", Some("x"))],
        ),
        (
            "custom without a value",
            base(),
            vec![mig_mask_item(&[], &["public"], "CUSTOM", None)],
        ),
        (
            "undeclared mask type",
            base(),
            vec![mig_mask_item(&[], &["public"], "MASK_EGRET_UNKNOWN", None)],
        ),
    ];
    for (what, res, items) in masks {
        out.push(mig_rule(format!("mask shape {what}"), 1, res, items));
    }
    {
        let mut pol = mig_rule(
            "mask shape insert access".into(),
            1,
            base(),
            vec![hash(&[], &["public"])],
        );
        pol.body["dataMaskPolicyItems"][0]["accesses"] = mig_accesses(&["insert"]);
        out.push(pol);
        let mut pol = mig_rule(
            "mask shape condition expr".into(),
            1,
            base(),
            vec![hash(&[], &["public"])],
        );
        pol.body["dataMaskPolicyItems"][0]["dataMaskInfo"]["conditionExpr"] = json!("1 = 1");
        out.push(pol);
        let mut pol = mig_rule(
            "mask shape no mask info".into(),
            1,
            base(),
            vec![hash(&[], &["public"])],
        );
        pol.body["dataMaskPolicyItems"][0]
            .as_object_mut()
            .expect("item")
            .remove("dataMaskInfo");
        out.push(pol);
        let mut pol = mig_rule(
            "mask shape priority 1".into(),
            1,
            base(),
            vec![hash(&[], &["public"])],
        );
        pol.body["policyPriority"] = json!(1);
        out.push(pol);
    }

    // H. Row filters (three levels: `rowFilterDef` stops at `table`).
    let table = || mig_path(p, &[&["wh"], &["sales"], &["orders"]]);
    let eu = Some("region = 'EU'");
    let filters: Vec<(&str, Value, Vec<Value>)> = vec![
        (
            "everyone",
            table(),
            vec![mig_filter_item(&[], &["public"], eu)],
        ),
        (
            "user exemption",
            table(),
            vec![
                mig_filter_item(&["bob"], &[], Some("")),
                mig_filter_item(&[], &["public"], eu),
            ],
        ),
        (
            "group exemption",
            table(),
            vec![
                mig_filter_item(&["alice"], &["contractors"], Some("")),
                mig_filter_item(&[], &["public"], eu),
            ],
        ),
        (
            "schema-wide",
            mig_path(p, &[&["wh"], &["sales"], &["*"]]),
            vec![mig_filter_item(&[], &["public"], eu)],
        ),
        (
            "two catalogs",
            mig_path(p, &[&["wh", "lake"], &["sales"], &["orders"]]),
            vec![mig_filter_item(&[], &["public"], eu)],
        ),
        (
            "with a column",
            base(),
            vec![mig_filter_item(&[], &["public"], eu)],
        ),
        (
            "stops at schema",
            mig_path(p, &[&["wh"], &["sales"]]),
            vec![mig_filter_item(&[], &["public"], eu)],
        ),
        (
            "no filter expr",
            table(),
            vec![mig_filter_item(&[], &["public"], None)],
        ),
        (
            "runtime expr",
            table(),
            vec![mig_filter_item(
                &[],
                &["public"],
                Some("owner = '{{USER}}'"),
            )],
        ),
        (
            "per-principal only",
            table(),
            vec![mig_filter_item(&["alice"], &[], eu)],
        ),
        (
            "two everyone items",
            table(),
            vec![
                mig_filter_item(&[], &["public"], eu),
                mig_filter_item(&[], &["public"], Some("1 = 0")),
            ],
        ),
    ];
    for (what, res, items) in filters {
        out.push(mig_rule(format!("filter {what}"), 2, res, items));
    }

    // I. Tag-addressed bodies, stored in the TAG service.
    let tag = |values: &[&str]| json!({"tag": mig_level(values)});
    for t in all_mask_types {
        out.push(mig_rule(
            format!("tag mask {t}"),
            1,
            tag(&["phi"]),
            vec![mig_mask_item(&[], &["public"], t, mig_mask_value(t))],
        ));
    }
    let tag_shapes: Vec<(&str, i32, Value, Vec<Value>)> = vec![
        (
            "tag mask with exemption",
            1,
            tag(&["phi"]),
            vec![
                mig_mask_item(&["alice"], &["analysts"], "MASK_NONE", None),
                hash(&[], &["public"]),
            ],
        ),
        (
            "tag filter",
            2,
            tag(&["phi"]),
            vec![mig_filter_item(&[], &["public"], eu)],
        ),
        (
            "tag filter with exemption",
            2,
            tag(&["phi"]),
            vec![
                mig_filter_item(&["bob"], &[], Some("")),
                mig_filter_item(&[], &["public"], eu),
            ],
        ),
        (
            "tag mask key differs from name",
            1,
            tag(&["pii"]),
            vec![hash(&[], &["public"])],
        ),
        (
            "tag mask unknown tag",
            1,
            tag(&["nosuch"]),
            vec![hash(&[], &["public"])],
        ),
        (
            "tag mask two tags",
            1,
            tag(&["phi", "pii"]),
            vec![hash(&[], &["public"])],
        ),
    ];
    for (what, pt, res, items) in tag_shapes {
        out.push(mig_rule(what.to_string(), pt, res, items));
    }
    let mut clearance = mig_allow(
        "tag access policy".into(),
        tag(&["phi"]),
        mig_item(&["alice"], &[], sel),
    );
    clearance.service = ServiceKind::Tag;
    out.push(clearance);
    // Placement: each body in the wrong kind of service.
    let mut misplaced = mig_rule(
        "tag mask in a resource service".into(),
        1,
        tag(&["phi"]),
        vec![hash(&[], &["public"])],
    );
    misplaced.service = ServiceKind::Resource;
    out.push(misplaced);
    let mut misplaced = mig_rule(
        "path mask in a tag service".into(),
        1,
        base(),
        vec![hash(&[], &["public"])],
    );
    misplaced.service = ServiceKind::Tag;
    out.push(misplaced);

    out
}

fn mig_classify(profile: &Profile, policy: &MigPolicy) -> Result<MigAccepted, Vec<Blocker>> {
    let engine = mig_perm_engine(profile);
    match policy.body["policyType"].as_i64().unwrap_or(0) {
        0 => grants_from_access_policy(&policy.body, engine, &mig_catalog_id, policy.service)
            .map(|c| MigAccepted::Grants(c.grants)),
        _ => rules_from_mask_or_filter_policy(
            &policy.body,
            engine,
            &mig_catalog_id,
            &mig_tag_name,
            policy.service,
        )
        .map(|c| MigAccepted::Rules(c.rules)),
    }
}

/// The variant name of a blocker, for the per-kind tally.
fn mig_blocker_kind(b: &Blocker) -> String {
    let s = format!("{b:?}");
    s.split([' ', '{', '(']).next().unwrap_or("").to_string()
}

/// The grammar's output compiled by egret's real compiler, as
/// `(resource-service policies, tag-service policies)`.
///
/// What `trigger::recompile_service` does with an accepted grant or rule, minus
/// the DB: ids are assigned (the grammar emits `Uuid::nil()`, `perm.grants` /
/// `perm.rules` assign the real ones), the body is compiled, StarRocks access
/// bodies are stamped `serviceType: starrocks` (`rules::stamp_service_type`,
/// `pub(crate)`, restated here as its one line), and each body is filed under
/// the service its own `service` field names (`trigger::target_service_id`).
fn mig_compile(profile: &Profile, accepted: &MigAccepted) -> (Vec<Value>, Vec<Value>) {
    let engine = mig_perm_engine(profile);
    let mut bodies: Vec<Value> = Vec::new();
    match accepted {
        MigAccepted::Grants(grants) => {
            for (n, g) in grants.iter().enumerate() {
                let mut g = g.clone();
                g.id = Uuid::from_u128(0x7000 + n as u128);
                validate_grant_shape(&g)
                    .unwrap_or_else(|e| panic!("the model refuses an accepted grant {g:?}: {e:?}"));
                let input = compile_grant(
                    &g,
                    mig_catalog_name(g.scope.catalog_id),
                    profile.service_name,
                    engine,
                )
                .unwrap_or_else(|e| panic!("compile_grant refused an accepted grant {g:?}: {e:?}"));
                let mut body = access_to_ranger_json(&input);
                if engine == PermEngine::StarRocks {
                    body["serviceType"] = json!("starrocks");
                }
                bodies.push(body);
            }
        }
        MigAccepted::Rules(rules) => {
            for (n, r) in rules.iter().enumerate() {
                let mut r = r.clone();
                r.id = Uuid::from_u128(0x8000 + n as u128);
                let (catalog, tag_name) = match &r.selector {
                    Selector::Path(s) => (mig_catalog_name(s.catalog_id), None),
                    Selector::Tag { key } => ("", mig_tag_name(key)),
                };
                let compiled = compile_rule(
                    &r,
                    engine,
                    profile.service_name,
                    Some(profile.tag_service_name),
                    &ResolvedNames {
                        catalog_name: catalog,
                        tag_name: tag_name.as_deref(),
                    },
                    &[],
                    100,
                )
                .unwrap_or_else(|e| panic!("compile_rule refused an accepted rule {r:?}: {e:?}"));
                bodies.extend(compiled);
            }
        }
    }
    // `recompile_service` keys its desired set by (service, name): two bodies
    // with one name would silently collapse to one there.
    let names: BTreeSet<&str> = bodies.iter().filter_map(|b| b["name"].as_str()).collect();
    assert_eq!(
        names.len(),
        bodies.len(),
        "compiled names collide: {bodies:?}"
    );

    let (mut resource, mut tag) = (Vec::new(), Vec::new());
    for (i, mut body) in bodies.into_iter().enumerate() {
        body["id"] = json!(MIG_COMPILED_ID_BASE + i as i64);
        match body["service"].as_str() {
            Some(s) if s == profile.service_name => resource.push(body),
            Some(s) if s == profile.tag_service_name => tag.push(body),
            other => panic!("compiled body names service {other:?}: {body}"),
        }
    }
    (resource, tag)
}

/// Policies every set carries, before and after alike, none of which any probe
/// principal can match (access) or any probe coordinate reaches (mask, filter).
fn mig_background(profile: &Profile) -> Vec<Value> {
    let h = profile.hierarchy;
    vec![
        json!({"id": 1, "name": "bg access", "service": profile.service_name, "policyType": 0,
               "isEnabled": true,
               "resources": {h[0]: mig_level(&["wh"]), h[1]: mig_level(&["*"]),
                             h[2]: mig_level(&["*"]), h[3]: mig_level(&["*"])},
               "policyItems": [mig_item(&[MIG_BG_USER], &[], &["select"])]}),
        json!({"id": 2, "name": "bg mask", "service": profile.service_name, "policyType": 1,
               "isEnabled": true,
               "resources": {h[0]: mig_level(&["wh"]), h[1]: mig_level(&["sales"]),
                             h[2]: mig_level(&["orders"]), h[3]: mig_level(&["egret_bg_col"])},
               "dataMaskPolicyItems": [mig_mask_item(&[], &["public"], "MASK_NULL", None)]}),
        json!({"id": 3, "name": "bg filter", "service": profile.service_name, "policyType": 2,
               "isEnabled": true,
               "resources": {h[0]: mig_level(&["wh"]), h[1]: mig_level(&["sales"]),
                             h[2]: mig_level(&["egret_bg_table"])},
               "rowFilterPolicyItems": [mig_filter_item(&[], &["public"], Some("1 = 0"))]}),
    ]
}

fn mig_envelope(
    profile: &Profile,
    def_json: &Value,
    tag_def_json: &Value,
    resource: Vec<Value>,
    tag: Option<Vec<Value>>,
) -> Value {
    let mut envelope = json!({
        "serviceName": profile.service_name,
        "serviceId": 1,
        "policyVersion": 1,
        "serviceDef": def_json,
        "policies": resource,
    });
    if let Some(tag) = tag {
        envelope["tagPolicies"] = tag_block(profile, tag_def_json, tag);
    }
    envelope
}

/// One question to ask of both sets.
struct MigProbe {
    policy_type: i32,
    user: String,
    groups: Vec<String>,
    access_type: String,
    resource: Vec<(String, String)>,
    tags: Option<Vec<String>>,
}

/// A value the policy's own (possibly wildcarded) value matches.
fn mig_concrete(v: &str) -> String {
    v.replace('*', "any").replace('?', "q")
}

/// The first value at each catalog-hierarchy level (absent → the walk value),
/// and every catalog value.
fn mig_levels(profile: &Profile, resources: &Value) -> (Vec<String>, Vec<String>) {
    let values = |level: &str| -> Vec<String> {
        resources[level]["values"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(mig_concrete)
                    .collect()
            })
            .unwrap_or_default()
    };
    let catalogs = values(profile.hierarchy[0]);
    let below = profile.hierarchy[1..]
        .iter()
        .map(|l| {
            values(l)
                .into_iter()
                .next()
                .unwrap_or_else(|| mig_walk_value(l))
        })
        .collect();
    (catalogs, below)
}

fn mig_push_principals(
    out: &mut Vec<MigProbe>,
    principals: &[(&str, &[&str])],
    policy_type: i32,
    access_type: &str,
    coord: &[(String, String)],
    tags: Option<Vec<String>>,
) {
    for (user, groups) in principals {
        out.push(MigProbe {
            policy_type,
            user: (*user).to_string(),
            groups: groups.iter().map(|g| (*g).to_string()).collect(),
            access_type: access_type.to_string(),
            resource: coord.to_vec(),
            tags: tags.clone(),
        });
    }
}

/// `values` at `depth` levels, with level `sibling` (if any) replaced by a
/// value nobody names.
fn mig_coord(
    profile: &Profile,
    values: &[String],
    depth: usize,
    sibling: Option<usize>,
) -> Vec<(String, String)> {
    (0..depth)
        .map(|i| {
            let v = if sibling == Some(i) {
                MIG_SIBLING.to_string()
            } else {
                values[i].clone()
            };
            (profile.hierarchy[i].to_string(), v)
        })
        .collect()
}

fn mig_probes(profile: &Profile, access_types: &[String], policy: &MigPolicy) -> Vec<MigProbe> {
    let mut out = Vec::new();
    let body = &policy.body;
    let policy_type = body["policyType"].as_i64().unwrap_or(0) as i32;

    if policy.service == ServiceKind::Tag {
        // The tag phase: the coordinate the original names nothing about, so
        // probe a plain column (and the background mask's column, where a
        // resource mask competes) carrying the policy's tag, another tag, no
        // tag, and both.
        let tag_name = body["resources"]["tag"]["values"][0]
            .as_str()
            .unwrap_or("phi")
            .to_string();
        let depth = if policy_type == 2 { 3 } else { 4 };
        let leaves: &[&str] = if policy_type == 2 {
            &["orders", "egret_bg_table"]
        } else {
            &["email", "egret_bg_col"]
        };
        for leaf in leaves {
            let mut values: Vec<String> = ["wh", "sales", "orders", "email"]
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            values[depth - 1] = (*leaf).to_string();
            let coord = mig_coord(profile, &values, depth, None);
            for tags in [
                vec![tag_name.clone()],
                vec!["Sensitive".to_string()],
                vec![],
                vec![tag_name.clone(), "Sensitive".to_string()],
            ] {
                mig_push_principals(
                    &mut out,
                    MIG_PROBE_PRINCIPALS,
                    policy_type,
                    "select",
                    &coord,
                    Some(tags),
                );
            }
        }
        return out;
    }

    let (catalogs, below) = mig_levels(profile, &body["resources"]);
    let mut own: BTreeSet<String> = BTreeSet::new();
    for bucket in [
        "policyItems",
        "denyPolicyItems",
        "allowExceptions",
        "denyExceptions",
    ] {
        for item in body[bucket].as_array().into_iter().flatten() {
            for a in item["accesses"].as_array().into_iter().flatten() {
                own.insert(a["type"].as_str().unwrap_or("").to_string());
            }
        }
    }
    let narrow: Vec<String> = access_types
        .iter()
        .filter(|t| own.contains(*t) || ["select", "insert", "delete"].contains(&t.as_str()))
        .cloned()
        .collect();

    for catalog in &catalogs {
        let mut values = vec![catalog.clone()];
        values.extend(below.iter().cloned());
        match policy_type {
            0 => {
                // The policy's own coordinate: EVERY access type the def declares.
                let exact = mig_coord(profile, &values, 4, None);
                for at in access_types {
                    mig_push_principals(&mut out, MIG_PROBE_PRINCIPALS, 0, at, &exact, None);
                }
                // Its ancestors and a sibling at every level: the access types
                // it names plus `select`, `insert` and `delete`.
                let mut others: Vec<Vec<(String, String)>> = (1..4)
                    .map(|d| mig_coord(profile, &values, d, None))
                    .collect();
                others.extend((0..4).map(|i| mig_coord(profile, &values, 4, Some(i))));
                for coord in &others {
                    for at in &narrow {
                        mig_push_principals(&mut out, MIG_AROUND_PRINCIPALS, 0, at, coord, None);
                    }
                }
            }
            1 => {
                // A column, a sibling at every level, and the table and schema
                // themselves — a mask may stop above the column, and then only
                // a request at its own depth reaches it.
                let mut coords: Vec<Vec<(String, String)>> = std::iter::once(None)
                    .chain((0..4).map(Some))
                    .map(|sibling| mig_coord(profile, &values, 4, sibling))
                    .collect();
                coords.extend((2..4).map(|d| mig_coord(profile, &values, d, None)));
                for coord in &coords {
                    mig_push_principals(&mut out, MIG_PROBE_PRINCIPALS, 1, "select", coord, None);
                }
            }
            _ => {
                for sibling in std::iter::once(None).chain((0..3).map(Some)) {
                    let coord = mig_coord(profile, &values, 3, sibling);
                    mig_push_principals(&mut out, MIG_PROBE_PRINCIPALS, 2, "select", &coord, None);
                }
            }
        }
    }
    out
}

fn mig_case(id: String, set: &Value, probe: &MigProbe) -> Case {
    let mut resource = RequestResource::new();
    for (k, v) in &probe.resource {
        resource.insert(k.clone(), v.clone());
    }
    let resolved_tags = probe.tags.as_ref().map(|names| {
        let levels: BTreeSet<String> = probe.resource.iter().map(|(k, _)| k.clone()).collect();
        UnenrichedTags::new(
            names
                .iter()
                .map(|n| ResolvedTag {
                    name: n.clone(),
                    match_type: MatchType::Self_,
                    attributes: BTreeMap::new(),
                    options: BTreeSet::new(),
                    validity_periods: 0,
                    attached_through: TagServiceResource {
                        levels: levels.clone(),
                        is_enabled: true,
                    },
                })
                .collect(),
        )
    });
    Case {
        id,
        policy_type: probe.policy_type,
        duel: false,
        composed_duel: false,
        candidacy_duel: false,
        restore_duel: false,
        version_split_duel: false,
        service_policies: set.clone(),
        request: AccessRequest {
            user: probe.user.clone(),
            groups: probe.groups.iter().cloned().collect(),
            roles: BTreeSet::new(),
            access_type: probe.access_type.clone(),
            resource,
            owner: None,
            resolved_tags,
        },
    }
}

/// One accepted policy, ready to ask.
struct MigPair {
    label: String,
    body: Value,
    service: ServiceKind,
    before: Value,
    after: Value,
    probes: Vec<MigProbe>,
}

/// Per engine: the population, for the report and the assertions.
#[derive(Default)]
struct MigTally {
    accepted: usize,
    refused: usize,
    probes: usize,
    /// Accepted bodies identical to an earlier accepted one, not asked again.
    duplicates: usize,
    blockers: BTreeMap<String, usize>,
}

fn mig_prepare(
    profile: &Profile,
    def_json: &Value,
    tag_def_json: &Value,
    all_mask_types: &[String],
    tally: &mut MigTally,
    refusals: &mut Vec<(String, Vec<Blocker>)>,
    accepted_labels: &mut Vec<String>,
) -> Vec<MigPair> {
    let access_types = mig_def_names(def_json, "/accessTypes");
    let mut pairs = Vec::new();
    for policy in mig_corpus(profile, def_json, all_mask_types) {
        let accepted = match mig_classify(profile, &policy) {
            Ok(a) => a,
            Err(blockers) => {
                assert!(
                    !blockers.is_empty(),
                    "{}: refused without naming a blocker",
                    policy.label
                );
                tally.refused += 1;
                // Policies per kind, not blocker instances: one body missing
                // three levels is one `MissingLevel` refusal.
                let kinds: BTreeSet<String> = blockers.iter().map(mig_blocker_kind).collect();
                for kind in kinds {
                    *tally.blockers.entry(kind).or_default() += 1;
                }
                refusals.push((policy.label.clone(), blockers));
                continue;
            }
        };
        tally.accepted += 1;
        accepted_labels.push(policy.label.clone());
        // The walk and the DML power set generate some bodies twice (the
        // walk's `select` on the full hierarchy IS `dml select`); asking the
        // engine the same thing twice measures nothing new.
        if pairs
            .iter()
            .any(|p: &MigPair| p.body == policy.body && p.service == policy.service)
        {
            tally.duplicates += 1;
            continue;
        }

        let (compiled_resource, compiled_tag) = mig_compile(profile, &accepted);
        let mut original = policy.body.clone();
        original["id"] = json!(MIG_ORIGINAL_ID);
        let mut before_resource = mig_background(profile);
        let mut before_tag = Vec::new();
        match policy.service {
            ServiceKind::Resource => {
                original["service"] = json!(profile.service_name);
                before_resource.push(original);
            }
            ServiceKind::Tag => {
                original["service"] = json!(profile.tag_service_name);
                before_tag.push(original);
            }
        }
        let mut after_resource = mig_background(profile);
        after_resource.extend(compiled_resource);
        let has_tags = !before_tag.is_empty() || !compiled_tag.is_empty();
        let before = mig_envelope(
            profile,
            def_json,
            tag_def_json,
            before_resource,
            has_tags.then_some(before_tag),
        );
        let after = mig_envelope(
            profile,
            def_json,
            tag_def_json,
            after_resource,
            has_tags.then_some(compiled_tag),
        );
        let probes = mig_probes(profile, &access_types, &policy);
        tally.probes += probes.len();
        pairs.push(MigPair {
            label: policy.label,
            body: policy.body,
            service: policy.service,
            before,
            after,
            probes,
        });
    }
    pairs
}

fn mig_refusal<'a>(refusals: &'a [(String, Vec<Blocker>)], label: &str) -> Option<&'a [Blocker]> {
    refusals
        .iter()
        .find(|(l, _)| l == label)
        .map(|(_, b)| b.as_slice())
}

/// The pins, both cells each, on the corpus the engine is asked about — so the
/// ACCEPTED cell of each is also proven equivalent by the real engine below.
///
/// Returned rather than asserted, so a grammar that broke a pin is ALSO run
/// through the engines: the mutation gate reads the engine's verdict on the
/// mutant, not only this function's.
fn mig_check_pins(
    profile: &Profile,
    def_json: &Value,
    refusals: &[(String, Vec<Blocker>)],
    accepted: &[String],
) -> Vec<String> {
    let mut failures = Vec::new();
    let engine = profile.engine.service_def_name();
    let is_accepted = |label: &str| accepted.iter().any(|l| l == label);
    let mut refused_naming = |label: &str, want: Blocker| match mig_refusal(refusals, label) {
        Some(b) if b.contains(&want) => {}
        Some(b) => failures.push(format!(
            "[{engine}] pin: `{label}` must be refused naming {want:?}, got {b:?}"
        )),
        None => failures.push(format!(
            "[{engine}] pin: `{label}` must be refused naming {want:?}, but the grammar ACCEPTED it"
        )),
    };
    // `Write` is atomic: the full expansion is accepted, a strict subset is
    // refused naming what it lacks.
    let (full_write, insert_missing) = match profile.engine {
        EngineType::Trino => ("dml insert+delete", vec!["delete".to_string()]),
        EngineType::StarRocks => (
            "dml insert+update+delete",
            vec!["update".to_string(), "delete".to_string()],
        ),
    };
    refused_naming(
        "dml insert",
        Blocker::PartialActionExpansion {
            action: egret_perm::Action::Write,
            missing: insert_missing,
        },
    );
    // A catalog outside the registry: registered accepted, unregistered
    // refused naming it.
    refused_naming(
        "scope select unregistered catalog",
        Blocker::UnregisteredCatalog {
            catalog: "ext".to_string(),
        },
    );
    // The non-expressible access types are REACHED and refused by name, on the
    // full catalog hierarchy, for every one of them the def declares.
    let full_path = profile.hierarchy.join(".");
    for at in mig_def_names(def_json, "/accessTypes")
        .iter()
        .filter(|t| !["select", "insert", "update", "delete"].contains(&t.as_str()))
    {
        refused_naming(
            &format!("walk {full_path} [{at}]"),
            Blocker::UnknownAccessType {
                access_type: at.clone(),
            },
        );
    }
    // Every non-catalog hierarchy is reached and refused.
    for path in mig_hierarchy_paths(def_json) {
        if path[0] != "catalog" {
            let label = format!("walk {} [select]", path.join("."));
            if mig_refusal(refusals, &label).is_none() {
                failures.push(format!("[{engine}] pin: `{label}` must be refused"));
            }
        }
    }
    for label in [full_write, "dml select"] {
        if !is_accepted(label) {
            failures.push(format!("[{engine}] pin: `{label}` must be accepted"));
        }
    }
    failures
}

/// Before vs after on every probe, at one Ranger version. Returns the number
/// of probes compared, plus the reach counts.
#[derive(Default)]
struct MigRun {
    compared: usize,
    allowed: usize,
    denied_by_original: usize,
    masked: usize,
    filtered: usize,
    disagreements: Vec<String>,
    unreached: Vec<String>,
}

fn mig_run(profile: &Profile, pairs: &[MigPair], ranger_version: &str) -> MigRun {
    let mut run = MigRun::default();
    // (pair index, probe index), in case order.
    let index: Vec<(usize, usize)> = pairs
        .iter()
        .enumerate()
        .flat_map(|(pi, p)| (0..p.probes.len()).map(move |qi| (pi, qi)))
        .collect();
    let mut original_decided = vec![false; pairs.len()];
    let mut compiled_decided = vec![false; pairs.len()];
    for (chunk_no, chunk) in index.chunks(MIG_CHUNK / 2).enumerate() {
        let mut cases = Vec::with_capacity(chunk.len() * 2);
        for &(pi, qi) in chunk {
            let pair = &pairs[pi];
            let probe = &pair.probes[qi];
            let id = format!("mig-{pi:04}-{qi:04}");
            cases.push(mig_case(format!("{id}-before"), &pair.before, probe));
            cases.push(mig_case(format!("{id}-after"), &pair.after, probe));
        }
        let verdicts = ask_ranger(
            &cases,
            profile.engine.service_def_name(),
            ranger_version,
            &format!("mig-{}-{chunk_no}", profile.engine.service_def_name()),
        );
        for (k, &(pi, qi)) in chunk.iter().enumerate() {
            let (before, after) = (&verdicts[2 * k], &verdicts[2 * k + 1]);
            let pair = &pairs[pi];
            let probe = &pair.probes[qi];
            assert_eq!(before["id"], json!(cases[2 * k].id), "verdict order");
            assert_eq!(after["id"], json!(cases[2 * k + 1].id), "verdict order");
            for v in [before, after] {
                assert!(
                    v["error"].is_null(),
                    "ranger {ranger_version} threw on {} ({}): {}\npolicy: {}",
                    v["id"],
                    pair.label,
                    v["error"],
                    pair.body
                );
            }
            run.compared += 1;
            if before["policyId"] == json!(MIG_ORIGINAL_ID) {
                original_decided[pi] = true;
            }
            if before["policyId"]
                .as_i64()
                .is_some_and(|id| id >= MIG_COMPILED_ID_BASE)
            {
                panic!("a compiled id decided a BEFORE case: {before}");
            }
            if after["policyId"]
                .as_i64()
                .is_some_and(|id| id >= MIG_COMPILED_ID_BASE)
            {
                compiled_decided[pi] = true;
            }
            match probe.policy_type {
                0 if before["isAllowed"] == json!(true) => run.allowed += 1,
                // Not `isAccessDetermined`: with no fallback authorizer Ranger
                // marks EVERY denial determined, matched or not.
                0 if before["policyId"] == json!(MIG_ORIGINAL_ID) => run.denied_by_original += 1,
                1 if before["maskType"]
                    .as_str()
                    .is_some_and(|t| t != "MASK_NONE") =>
                {
                    run.masked += 1
                }
                2 if before["filterExpr"].as_str().is_some_and(|f| !f.is_empty()) => {
                    run.filtered += 1
                }
                _ => {}
            }
            let differing: Vec<&str> = MIG_COMPARED_FIELDS
                .iter()
                .copied()
                .filter(|f| before.get(*f) != after.get(*f))
                .collect();
            if !differing.is_empty() && run.disagreements.len() < 20 {
                let after_policies = &pair.after["policies"];
                let after_tags = &pair.after["tagPolicies"]["policies"];
                run.disagreements.push(format!(
                    "ranger {ranger_version} DISAGREES on `{}` — fields {differing:?}\n  \
                     probe: policyType {} user {:?} groups {:?} access {:?} resource {:?} tags {:?}\n  \
                     before: {before}\n  after:  {after}\n  hand-written policy: {}\n  \
                     compiled (resource): {after_policies}\n  compiled (tag): {after_tags}",
                    pair.label,
                    probe.policy_type,
                    probe.user,
                    probe.groups,
                    probe.access_type,
                    probe.resource,
                    probe.tags,
                    pair.body,
                ));
            } else if !differing.is_empty() {
                run.disagreements.push(String::new());
            }
        }
    }
    for (i, pair) in pairs.iter().enumerate() {
        if !original_decided[i] || !compiled_decided[i] {
            run.unreached.push(format!(
                "`{}` (original decided a probe: {}, compiled decided a probe: {})",
                pair.label, original_decided[i], compiled_decided[i]
            ));
        }
    }
    run
}

#[test]
fn a_migration_the_grammar_accepts_decides_identically_on_the_real_engine() {
    if std::env::var("EGRET_TEST_RANGER_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_RANGER_E2E != 1 — skipping the migration differential");
        return;
    }
    let started = std::time::Instant::now();
    let tag_def_json = load_tag_def();
    // Every mask type EITHER def declares, on both engines — so a type one
    // engine lacks is generated there and refused.
    let mut all_mask_types: Vec<String> = Vec::new();
    for profile in [trino(), starrocks()] {
        let (def_json, _, _) = load_engine(&profile);
        for t in mig_def_names(&def_json, "/dataMaskDef/maskTypes") {
            if !all_mask_types.contains(&t) {
                all_mask_types.push(t);
            }
        }
    }

    let mut failures: Vec<String> = Vec::new();
    let mut total_refused = 0usize;
    let mut total_accepted = 0usize;
    let mut blockers_all: BTreeMap<String, usize> = BTreeMap::new();
    for profile in [trino(), starrocks()] {
        let (def_json, _, _) = load_engine(&profile);
        let mut tally = MigTally::default();
        let mut refusals = Vec::new();
        let mut accepted = Vec::new();
        let pairs = mig_prepare(
            &profile,
            &def_json,
            &tag_def_json,
            &all_mask_types,
            &mut tally,
            &mut refusals,
            &mut accepted,
        );
        failures.extend(mig_check_pins(&profile, &def_json, &refusals, &accepted));
        eprintln!(
            "migration differential [{}]: corpus {} policies, accepted={} ({} duplicates not \
             re-asked) refused={} probes={} (cases per version = {})",
            profile.engine.service_def_name(),
            tally.accepted + tally.refused,
            tally.accepted,
            tally.duplicates,
            tally.refused,
            tally.probes,
            tally.probes * 2
        );
        eprintln!("  accepted: {}", accepted.join(" | "));
        for (kind, n) in &tally.blockers {
            eprintln!("  refused, blocker {kind:32} ×{n}");
            *blockers_all.entry(kind.clone()).or_default() += n;
        }
        assert!(
            tally.accepted > 0,
            "a corpus the grammar wholly refuses proves nothing"
        );
        assert!(
            tally.refused > 0,
            "a corpus the grammar wholly accepts reached no refusal"
        );
        total_refused += tally.refused;
        total_accepted += tally.accepted;

        for version in ["2.5.0", "2.8.0"] {
            let t = std::time::Instant::now();
            let run = mig_run(&profile, &pairs, version);
            eprintln!(
                "migration differential [{} @ ranger {version}]: {} probes compared ({} cases) \
                 in {:.2} s; before-state allowed {} / denied by the original {} / masked {} / \
                 filtered {}; disagreements {}; unreached {}",
                profile.engine.service_def_name(),
                run.compared,
                run.compared * 2,
                t.elapsed().as_secs_f64(),
                run.allowed,
                run.denied_by_original,
                run.masked,
                run.filtered,
                run.disagreements.len(),
                run.unreached.len(),
            );
            assert_eq!(run.compared, tally.probes, "every probe answered");
            for (what, n) in [
                ("allowed", run.allowed),
                ("denied by the original's deny item", run.denied_by_original),
                ("masked", run.masked),
                ("filtered", run.filtered),
            ] {
                if n == 0 {
                    failures.push(format!(
                        "[{} @ {version}] no probe was {what} before the migration — the \
                         comparison on that question is vacuous",
                        profile.engine.service_def_name()
                    ));
                }
            }
            if !run.disagreements.is_empty() {
                failures.push(format!(
                    "[{} @ {version}] {} probe(s) decide differently after the migration:\n{}",
                    profile.engine.service_def_name(),
                    run.disagreements.len(),
                    run.disagreements
                        .iter()
                        .filter(|d| !d.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            if !run.unreached.is_empty() {
                failures.push(format!(
                    "[{} @ {version}] accepted policies no probe let decide — their equivalence \
                     is vacuous:\n  {}",
                    profile.engine.service_def_name(),
                    run.unreached.join("\n  ")
                ));
            }
        }
    }
    eprintln!(
        "migration differential: accepted={total_accepted} refused={total_refused} in {:.2} s",
        started.elapsed().as_secs_f64()
    );
    for (kind, n) in &blockers_all {
        eprintln!("  refusals naming {kind:32} ×{n}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
