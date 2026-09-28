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
/// `?*?` WAS in this list and is now excluded by FINDING F1 (see
/// `pinned_divergences`): a `*` immediately followed by a `?` is where this
/// crate's wildcard matcher and `FilenameUtils.wildcardMatch` genuinely
/// disagree, so the corpus cannot generate it while the gate is green. The
/// shape stays under test as a pinned reproducer, which fails the moment the
/// defect is fixed — so the exclusion cannot outlive it.
const POLICY_VALUES: &[&str] = &[
    "", "*", "**", "?", "??", "a*b", "a**b", "*a*b*", "x?z", "sales", "SALES", "sal*es", "*_tmp",
    "tmp_*", "or?er", "**x", "x**", "hive", "HIVE", "h?ve", "h*e", "*ve", "hi*", "col*", "*d",
    "i?", "*s*", "s**s", "orders", "email",
];

/// Values a REQUEST resource level can carry. `*` and `""` are both here on
/// purpose: `RangerAbstractResourceMatcher.isAllValuesRequested` treats them
/// alike and short-circuits before any policy value is read.
const REQUEST_VALUES: &[&str] = &[
    "hive", "HIVE", "sales", "Sales", "orders", "id", "email", "a", "ab", "abc", "hive_tmp",
    "tmp_1", "*", "", "s", "xyz",
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
                // FINDING F1: a policy value in which a `*` is immediately
                // followed by a `?` is a MEASURED divergence, excluded from
                // the corpus and pinned by `pinned_divergences` instead. The
                // guard sits here rather than only on the vocabulary because
                // `covering_values` composes values of its own.
                assert!(
                    !value.contains("*?"),
                    "generator emitted `{value}`, which trips FINDING F1"
                );
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
    // FINDING F2: a `dataMaskPolicyItems` entry with NO `dataMaskInfo` is a
    // MEASURED divergence and is excluded from the corpus, pinned by
    // `pinned_divergences` instead. This crate reads it as "reports nothing,
    // keep looking"; Ranger's own model class cannot represent the absence, so
    // the item DOES determine the result — with a null mask type — and the
    // search stops there.
    {
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

fn generate_case(rng: &mut Rng, profile: &Profile, service_def: &Value, index: usize) -> Case {
    let policy_type = match rng.below(4) {
        0 | 1 => 0,
        2 => 1,
        _ => 2,
    };
    let shape = build_shape(rng, profile, policy_type);

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
        service_policies,
        request: AccessRequest {
            user: shape.user.clone(),
            groups: shape.groups.clone(),
            roles: shape.roles.clone(),
            access_type: shape.access_type.clone(),
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

// ── the gate ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct Tally {
    cases: usize,
    compared: usize,
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

/// One construct the corpus excludes because Rust and the real engine were
/// measured to disagree about it, kept under test as a minimal reproducer.
///
/// An exclusion that is only a comment rots: the defect gets fixed, nobody
/// remembers the corpus was narrowed, and the axis stays dark. These pins
/// assert the DISAGREEMENT, so the day the evaluator is corrected this test
/// fails and says to widen the generator again.
struct Pinned {
    case: Case,
    finding: &'static str,
    /// What this crate answers, and what Ranger answers — recorded so a
    /// change in EITHER side is visible, not just a change in the verdict.
    egret: &'static str,
    ranger: &'static str,
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
            finding: "F1: `*` followed by `?` — egret's wildcard matcher is greedy, \
                      FilenameUtils.wildcardMatch is not",
            egret: "Allowed (classic wildcard match: `?*?` covers `HIVE`)",
            ranger: "Denied (wildcardMatch(\"HIVE\", \"?*?\") is false, so no policy matched)",
            case: Case {
                id: "pin-f1-wildcard".to_string(),
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
        // `RangerPolicy$RangerDataMaskPolicyItem`'s no-arg constructor (the
        // one Gson uses) delegates to the all-args constructor with nulls, and
        // `setDataMaskInfo(null)` substitutes `new RangerPolicyItemDataMaskInfo()`.
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
            finding: "F2: a dataMask item with no `dataMaskInfo` — Ranger's model class \
                      cannot represent the absence, so the item still determines",
            egret: "Masked(MASK_HASH) — the priority-1 policy reports nothing, so the priority-0 one decides",
            ranger: "NoMask — the priority-1 policy DETERMINES with a null maskType and the search stops",
            case: Case {
                id: "pin-f2-missing-datamaskinfo".to_string(),
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

/// Assert both measured divergences still reproduce, so neither exclusion can
/// quietly outlive the defect that justifies it.
fn check_pinned_divergences(def_json: &Value, def: &ServiceDef, engine: &EngineConfig) {
    let pins = pinned_divergences(def_json);
    let cases: Vec<Case> = pins.iter().map(|p| p.case.clone()).collect();
    let verdicts = ask_ranger(&cases, "trino", "2.5.0", "pinned");

    for (pin, verdict) in pins.iter().zip(&verdicts) {
        let set: ServicePolicySet = serde_json::from_value(pin.case.service_policies.clone())
            .expect("parse pinned policy set");
        let mine = rust_outcome(&pin.case, &set, def, engine);
        let theirs = java_outcome(pin.case.policy_type, verdict);
        assert_ne!(
            mine, theirs,
            "PINNED DIVERGENCE {} NO LONGER REPRODUCES.\n  \
             This is good news: the evaluator now agrees with Ranger here. Delete this pin \
             AND re-widen the generator, or the corpus stays narrower than the evaluator.\n  \
             recorded egret: {}\n  recorded ranger: {}\n  both now say: {mine:?}",
            pin.case.id, pin.egret, pin.ranger,
        );
        eprintln!(
            "differential: pinned divergence {} still reproduces — {}\n    egret:  {mine:?}\n    ranger: {theirs:?}",
            pin.case.id, pin.finding
        );
    }
}

/// Send `cases` through the real engine at one plugin version and read the
/// verdicts back. Each run also executes the four hand-written anchor tests,
/// which is deliberate: they carry known-good expected values and are the only
/// thing in this harness that does not merely assert agreement.
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

    let out = Command::new("mvn")
        .args([
            "-q".to_string(),
            "test".to_string(),
            format!("-Dranger.version={ranger_version}"),
            format!("-Degret.cases={}", cases_path.display()),
            format!("-Degret.verdicts={}", verdicts_path.display()),
        ])
        .current_dir(&dir)
        .output()
        .expect("run mvn");
    assert!(
        out.status.success(),
        "mvn failed at ranger {ranger_version} (this run also executes the four hand-written \
         anchor tests, so a failure here may be one of those):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
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

    // Rust first, so a generator escape (an `Undecidable` the corpus is not
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
             {} EvalOrderAmbiguous ({:.1}%); allowed {} / denied {} / masked {} / \
             no-mask {} / filtered {} / no-filter {}",
            profile.ranger_version,
            tally.cases,
            tally.compared,
            tally.eval_order_ambiguous,
            100.0 * tally.eval_order_ambiguous as f64 / tally.cases as f64,
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
        total.allowed += tally.allowed;
        total.denied += tally.denied;
        total.masked += tally.masked;
        total.no_mask += tally.no_mask;
        total.filtered += tally.filtered;
        total.no_filter += tally.no_filter;
    }

    eprintln!(
        "differential: seed {seed:#x}, {} cases across both engines, {} compared, \
         {} EvalOrderAmbiguous ({:.1}%)",
        total.cases,
        total.compared,
        total.eval_order_ambiguous,
        100.0 * total.eval_order_ambiguous as f64 / total.cases as f64,
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
