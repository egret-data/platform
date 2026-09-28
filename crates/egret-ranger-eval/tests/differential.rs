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
//! ## What the corpus deliberately does NOT contain
//!
//! Only constructs the evaluator claims to model. A generated `Undecidable`
//! proves nothing about agreement, so the generator avoids every construct
//! that produces one (`validitySchedules`, policy/item `conditions`,
//! `zoneName`, `securityZones`, `tagPolicies`, `policyDeltas`, `serviceConfig`,
//! `${{ … }}` expressions, `{{ … }}` row filters, `{OWNER}` with no owner, the
//! `_any`/`_admin`/`_super_user` pseudo access types, and access types that
//! differ from a policy's only by case) and the test ASSERTS none appear — so
//! the generator cannot quietly drift into cases the evaluator declines to
//! answer while the gate reports green.
//!
//! The one exception is [`UndecidableCause::EvalOrderAmbiguous`], which is
//! generated ON PURPOSE: it is Task 6's refusal to guess a mask when two
//! policies tie inside one `(priority, hasDeny)` band, and whether that
//! refusal is rare or common is the measurement that decides whether
//! reproducing `computeEvalOrder()` is worth its risk. Those cases are counted
//! and reported, not asserted on — this crate has no answer to compare.
//!
//! The one measured version divergence — an `_any` request against an
//! ANCESTOR-matching `isDenyAllElse` policy denies at 2.5.0 and does not at
//! 2.8.0 — is out of the corpus by construction, since `_any` is
//! [`UndecidableCause::PseudoAccessType`]. It is recorded in
//! `tests/ranger/README.md` instead.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, EngineConfig,
    EngineType, MaskDecision, PluginVersion, RowFilterDecision, UndecidableCause,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Map, Value};

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

/// One engine's half of the corpus.
struct Profile {
    engine: EngineType,
    plugin_version: PluginVersion,
    /// The `-Dranger.version=` the Java half runs at.
    ranger_version: &'static str,
    service_name: &'static str,
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
        json!({
            "id": self.id,
            "serviceType": service_type,
            "policyType": self.policy_type,
            "servicePolicies": self.service_policies,
            "request": {
                "user": self.request.user,
                "groups": self.request.groups.iter().collect::<Vec<_>>(),
                "roles": self.request.roles.iter().collect::<Vec<_>>(),
                "accessType": self.request.access_type,
                "resource": Value::Object(resource),
            }
        })
    }
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

fn policy(rng: &mut Rng, profile: &Profile, shape: &Shape, id: i64, policy_type: i32) -> Value {
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
        json!(if rng.chance(1, 7) { 1 } else { 0 }),
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
        }
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
fn access_order_duel(rng: &mut Rng, profile: &Profile, shape: &Shape) -> Vec<Value> {
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

    let priority = if rng.chance(1, 3) { 1 } else { 0 };
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

fn generate_case(rng: &mut Rng, profile: &Profile, service_def: &Value, index: usize) -> Case {
    let policy_type = match rng.below(4) {
        0 | 1 => 0,
        2 => 1,
        _ => 2,
    };
    let shape = build_shape(rng, profile, policy_type);

    // One ACCESS case in four is the deliberate order duel rather than a
    // random draw.
    if policy_type == 0 && rng.chance(1, 4) {
        let policies = access_order_duel(rng, profile, &shape);
        return finish_case(
            profile,
            service_def,
            policies,
            shape,
            index,
            policy_type,
            true,
        );
    }

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
        policies.push(policy(rng, profile, &shape, (n + 1) as i64, this_type));
    }

    finish_case(
        profile,
        service_def,
        policies,
        shape,
        index,
        policy_type,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_case(
    profile: &Profile,
    service_def: &Value,
    policies: Vec<Value>,
    shape: Shape,
    index: usize,
    policy_type: i32,
    duel: bool,
) -> Case {
    let service_policies = json!({
        "serviceName": profile.service_name,
        "serviceId": 1,
        "policyVersion": 1,
        "serviceDef": service_def,
        "policies": policies,
    });

    let mut resource = RequestResource::new();
    for (name, value) in &shape.request_levels {
        resource.insert(name.clone(), value.clone());
    }

    Case {
        id: format!("{}-{index:04}", profile.ranger_version),
        policy_type,
        duel,
        service_policies,
        request: AccessRequest {
            user: shape.user,
            groups: shape.groups,
            roles: shape.roles,
            access_type: shape.access_type,
            resource,
            // `{OWNER}` is never generated, so no decision depends on this.
            owner: None,
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
    /// Refused because the mask/row-filter arbitration ties — the ONE
    /// refusal the corpus generates on purpose. Counted, not compared.
    EvalOrderAmbiguous,
    /// Any other refusal: a generator escape, and a hard failure.
    Refused(String),
}

fn only_eval_order(why: &[UndecidableCause]) -> bool {
    !why.is_empty()
        && why
            .iter()
            .all(|c| matches!(c, UndecidableCause::EvalOrderAmbiguous { .. }))
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
            Decision::Undecidable { why } => Outcome::Refused(format!("{why:?}")),
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
            MaskDecision::Undecidable { why } if only_eval_order(&why) => {
                Outcome::EvalOrderAmbiguous
            }
            MaskDecision::Undecidable { why } => Outcome::Refused(format!("{why:?}")),
        },
        _ => match evaluate_row_filter(set, &case.request, def, engine) {
            RowFilterDecision::NoFilter => Outcome::RowFilter { filter_expr: None },
            RowFilterDecision::Filtered(f) => Outcome::RowFilter {
                filter_expr: Some(f.filter_expr),
            },
            RowFilterDecision::Undecidable { why } if only_eval_order(&why) => {
                Outcome::EvalOrderAmbiguous
            }
            RowFilterDecision::Undecidable { why } => Outcome::Refused(format!("{why:?}")),
        },
    }
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
];

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

#[derive(Default)]
struct Tally {
    cases: usize,
    compared: usize,
    /// Deliberately-constructed eval-order duels, and how many of them the
    /// two sides agreed on. A claim about eval order is only as strong as the
    /// number of cases that could have falsified it.
    duels: usize,
    duels_agreeing: usize,
    eval_order_ambiguous: usize,
    allowed: usize,
    denied: usize,
    masked: usize,
    no_mask: usize,
    filtered: usize,
    no_filter: usize,
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
    // Dropping it keeps the harness hermetic without changing any decision the
    // corpus takes: this crate reports `tagPolicies` as unmodelled and the
    // corpus never generates one.
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

fn run_profile(profile: &Profile, seed: u64, count: usize) -> Tally {
    let (def_json, def, engine) = load_engine(profile);

    let mut rng = Rng::new(seed ^ (profile.ranger_version.len() as u64) ^ profile.engine as u64);
    let cases: Vec<Case> = (0..count)
        .map(|i| generate_case(&mut rng, profile, &def_json, i))
        .collect();

    // Coverage first, before a JVM is started or a single decision is taken:
    // if the corpus has stopped producing an axis, nothing downstream of here
    // means what it says.
    report_coverage(profile, &cases);

    // Then Rust, so a generator escape (an `Undecidable` the corpus is not
    // supposed to contain) is reported before a JVM is ever started.
    let mut outcomes = Vec::with_capacity(cases.len());
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

    for ((case, mine), verdict) in cases.iter().zip(&outcomes).zip(&verdicts) {
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

        if matches!(mine, Outcome::EvalOrderAmbiguous) {
            tally.eval_order_ambiguous += 1;
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
            Outcome::EvalOrderAmbiguous | Outcome::Refused(_) => unreachable!("handled above"),
        }
        tally.compared += 1;

        if case.duel && *mine == theirs {
            tally.duels_agreeing += 1;
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

    let mut total = Tally::default();
    for profile in [trino(), starrocks()] {
        let tally = run_profile(&profile, seed, count);
        eprintln!(
            "differential: ranger {} — {} cases, {} compared and AGREEING, \
             {} EvalOrderAmbiguous ({:.1}%), {} eval-order duels ({} agreeing); \
             allowed {} / denied {} / masked {} / no-mask {} / filtered {} / no-filter {}",
            profile.ranger_version,
            tally.cases,
            tally.compared,
            tally.eval_order_ambiguous,
            100.0 * tally.eval_order_ambiguous as f64 / tally.cases as f64,
            tally.duels,
            tally.duels_agreeing,
            tally.allowed,
            tally.denied,
            tally.masked,
            tally.no_mask,
            tally.filtered,
            tally.no_filter,
        );
        total.cases += tally.cases;
        total.compared += tally.compared;
        total.eval_order_ambiguous += tally.eval_order_ambiguous;
        total.duels += tally.duels;
        total.duels_agreeing += tally.duels_agreeing;
        total.allowed += tally.allowed;
        total.denied += tally.denied;
        total.masked += tally.masked;
        total.no_mask += tally.no_mask;
        total.filtered += tally.filtered;
        total.no_filter += tally.no_filter;
    }

    eprintln!(
        "differential: seed {seed:#x}, {} cases across both engines, {} compared, \
         {} EvalOrderAmbiguous ({:.1}%), {} eval-order duels of which {} agree",
        total.cases,
        total.compared,
        total.eval_order_ambiguous,
        100.0 * total.eval_order_ambiguous as f64 / total.cases as f64,
        total.duels,
        total.duels_agreeing,
    );

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

    // A corpus where nothing ever matches agrees with Ranger perfectly and
    // proves nothing. These floors are not thresholds on Ranger's behaviour —
    // they are a guard on the GENERATOR, so a change that makes every policy
    // miss cannot pass as green.
    assert!(
        total.allowed * 20 >= total.cases,
        "corpus degenerate: only {} allows in {} cases",
        total.allowed,
        total.cases
    );
    assert!(
        total.denied * 20 >= total.cases,
        "corpus degenerate: only {} denies in {} cases",
        total.denied,
        total.cases
    );
    assert!(
        total.masked * 40 >= total.cases,
        "corpus degenerate: only {} masks in {} cases",
        total.masked,
        total.cases
    );
    assert!(
        total.filtered * 40 >= total.cases,
        "corpus degenerate: only {} row filters in {} cases",
        total.filtered,
        total.cases
    );
}
