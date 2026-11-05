//! Layer 1 of the OPA target (governance SP2b Task 18): the decision-level
//! differential between egret's OPA evaluator (`egret-perm-eval`) and the
//! real Ranger engine the Trino 483 `ranger` plugin runs.
//!
//! The Ranger artifact stays the oracle (S8). Each of [`STATES`] generated
//! Trino profiles is compiled (`recompile_service_in_tx`), its plugin download
//! is read back as JSON (`harness_support::served_artifact_json_via`), and its
//! OPA snapshot is built from the same database state
//! (`harness_support::build_snapshot_for_test`). [`QUESTIONS`] OPA questions
//! per state are then answered twice: by the evaluator, and by the real
//! `RangerPolicyEngineImpl` (`tests/ranger`, Maven) over the requests the
//! plugin would make for them (`opa_support/to_ranger.rs`).
//!
//! Every difference is either fixed in the evaluator or listed in
//! `opa_differential/explained.toml` with its mechanism. A difference in which
//! OPA allows what Ranger denies (access, reach, a filter or a mask it drops)
//! is never explainable: the run fails "fix the evaluator", whatever the file
//! says.
//!
//! **The tags of a Ranger request are the state's own classifications**
//! (I7): on a column ask the column's tags are `SELF` and its table's
//! `ANCESTOR`; on a table ask the table's tags are `SELF` and its columns'
//! `DESCENDANT`; on a schema or catalog `_any` ask every tag below is
//! `DESCENDANT`. Both sides read the same classifications, so this layer does
//! not measure classification resolution (the `''` table-level sentinel of C3,
//! for one); Layer 2 (Task 21) does.
//!
//! **Ranger 2.8.0**: the Trino 483 image ships
//! `plugin/ranger/org.apache.ranger_ranger-plugins-common-2.8.0.jar`
//! (measured: `docker run --rm --entrypoint sh trinodb/trino:483 -c 'ls
//! /usr/lib/trino/plugin/ranger'`), so the JVM half runs at that version.
//!
//! ```bash
//! env -u EGRET_ALLOW_DB_SKIP EGRET_TEST_RANGER_E2E=1 \
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!   cargo test -p egret-perm-compile --features harness --test opa_differential -- --nocapture
//! ```

include!("opa_support/fixtures_host.rs");
#[allow(dead_code)]
#[path = "golden/fixtures.rs"]
mod fixtures;
#[path = "../../egret-ranger-eval/tests/support/ranger_jvm.rs"]
mod ranger_jvm;
#[path = "opa_support/to_ranger.rs"]
mod to_ranger;

use std::collections::{BTreeMap, BTreeSet};

use egret_core::engine_principals::LINEAGE_PRINCIPAL;
use egret_metastore::Db;
use egret_perm::{Action, CompileError, Effect, Principal, Selector};
use egret_perm_eval::trino_ops::{classify, OpClass, TRINO_483_OPERATIONS};
use egret_perm_eval::{Decision, EvalError, Resource, Snapshot};
use fixtures::*;
use serde_json::{json, Value};
use uuid::Uuid;

/// The corpus seed; `EGRET_OPA_DIFF_SEED` (hex, `0x` optional) overrides it.
const SEED: u64 = 0x5b2b_0000_0001;
const STATES: usize = 40;
const QUESTIONS: usize = 50;
/// The `ranger-plugins-common` the Trino 483 `ranger` plugin bundles.
const RANGER_VERSION: &str = "2.8.0";
/// A state refused at compile is redrawn; this many refusals in a row for one
/// state is a generator that cannot produce a compilable state.
const MAX_REDRAWS: usize = 25;

const CATALOG: &str = "lake";
const SCHEMAS: [&str; 2] = ["s1", "s2"];
/// The tables questions and scopes name. `t3$files` is a metadata table of
/// `t3`, which only classifications name (carry 2).
const TABLES: [&str; 3] = ["t1", "t2", "t3$files"];
const CLASSIFIED_TABLES: [&str; 3] = ["t1", "t2", "t3"];
const COLUMNS: [&str; 4] = ["id", "email", "ssn", "region"];
const USERS: [&str; 5] = ["u0", "u1", "u2", "u3", "u4"];
const TAGS: [&str; 2] = ["pii", "secret"];
const CLEARING_ROLE: &str = "l1_cleared";
const GROUP: &str = "l1_analysts";
const SYSTEM_PROPERTIES: [&str; 3] = [
    "query_max_memory",
    "query_max_run_time",
    "join_distribution_type",
];
const MASK_TYPES: [&str; 5] = ["MASK", "MASK_HASH", "MASK_NULL", "MASK_NONE", "CUSTOM"];
const REGIONS: [&str; 2] = ["eu", "us"];

/// The engine specs a state binds, so the infrastructure statements vary.
const SPECS: [&str; 3] = ["open", "impersonate+auth", "jwt"];

fn spec_json(spec: &str) -> Value {
    match spec {
        "open" => json!({"trinoConfig": {"delegationMode": "open"}}),
        "impersonate+auth" => json!({"trinoConfig": {
            "coordinator": {}, "delegationMode": "impersonate", "auth": {"password": {}}
        }}),
        "jwt" => json!({"trinoConfig": {
            "coordinator": {}, "delegationMode": "jwt", "auth": {"jwt": {}}
        }}),
        other => panic!("unknown spec {other}"),
    }
}

// ── the deterministic generator ─────────────────────────────────────────────

/// SplitMix64, copied from `egret-ranger-eval/tests/differential.rs`: a
/// stream pinned in this file cannot change under a `cargo update`.
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

    /// A non-empty subset of `0..n`, ascending.
    fn subset(&mut self, n: usize) -> Vec<usize> {
        loop {
            let s: Vec<usize> = (0..n).filter(|_| self.chance(2, 5)).collect();
            if !s.is_empty() {
                return s;
            }
        }
    }
}

fn seed() -> u64 {
    match std::env::var("EGRET_OPA_DIFF_SEED") {
        Ok(v) => u64::from_str_radix(
            v.trim().trim_start_matches("0x").replace('_', "").as_str(),
            16,
        )
        .unwrap_or_else(|e| panic!("EGRET_OPA_DIFF_SEED={v:?} is not hex: {e}")),
        Err(_) => SEED,
    }
}

// ── one state ───────────────────────────────────────────────────────────────

/// A scope under catalog `lake`; a level is present only when every level
/// above it is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopeSpec {
    schema: Option<String>,
    table: Option<String>,
    column: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Who {
    Role,
    User(usize),
    Public,
    Group,
}

#[derive(Debug, Clone)]
struct GrantSpec {
    deny: bool,
    read: bool,
    write: bool,
    scope: ScopeSpec,
    include: Vec<Who>,
    exclude: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuleTarget {
    Path(ScopeSpec),
    Tag(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuleEffect {
    Mask {
        mask_type: String,
        expression: Option<String>,
    },
    Filter(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleSpec {
    effect: RuleEffect,
    target: RuleTarget,
    exempt: Vec<usize>,
}

/// One classification: tag `TAGS[tag]` on `schema.table[.column]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassSpec {
    tag: usize,
    schema: String,
    table: String,
    column: Option<String>,
}

#[derive(Debug, Clone)]
enum EngineGrantSpec {
    Impersonate { holder: usize, target: usize },
    SessionProperty { holder: usize, property: String },
}

#[derive(Debug, Clone)]
struct StateSpec {
    grants: Vec<GrantSpec>,
    rules: Vec<RuleSpec>,
    classes: Vec<ClassSpec>,
    /// The tags (by index) gated: by `CLEARING_ROLE`, or (`true`) by the
    /// group alone, which the lowering does not withhold (M10).
    gated: Vec<(usize, bool)>,
    role_holders: Vec<usize>,
    group_members: Vec<usize>,
    engine_grants: Vec<EngineGrantSpec>,
    spec: &'static str,
    admin: Option<usize>,
    /// The index in `rules` of the duplicate an agreeing tie adds (I1).
    tie: Option<usize>,
}

impl StateSpec {
    fn tags_in_use(&self) -> BTreeSet<usize> {
        self.classes.iter().map(|c| c.tag).collect()
    }
}

fn draw_scope(rng: &mut Rng, min_depth: usize, max_depth: usize) -> ScopeSpec {
    let depth = min_depth + rng.below(max_depth - min_depth + 1);
    ScopeSpec {
        schema: (depth >= 1).then(|| rng.pick(&SCHEMAS).to_string()),
        table: (depth >= 2).then(|| rng.pick(&TABLES).to_string()),
        column: (depth >= 3).then(|| rng.pick(&COLUMNS).to_string()),
    }
}

fn draw_state(rng: &mut Rng, tie_state: bool) -> StateSpec {
    let mut classes = Vec::new();
    for _ in 0..rng.below(3) {
        let tag = rng.below(TAGS.len());
        classes.push(ClassSpec {
            tag,
            schema: rng.pick(&SCHEMAS).to_string(),
            table: rng.pick(&CLASSIFIED_TABLES).to_string(),
            column: (!rng.chance(1, 4)).then(|| rng.pick(&COLUMNS).to_string()),
        });
    }
    classes.dedup();
    let used: Vec<usize> = classes
        .iter()
        .map(|c| c.tag)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut gated: Vec<(usize, bool)> = Vec::new();
    for &t in &used {
        if rng.chance(3, 4) {
            gated.push((t, rng.chance(1, 5)));
        }
    }
    let role_holders: Vec<usize> = (0..USERS.len()).filter(|_| rng.chance(1, 2)).collect();
    let group_members: Vec<usize> = (0..USERS.len()).filter(|_| rng.chance(1, 2)).collect();

    let mut grants = Vec::new();
    for _ in 0..rng.below(5) {
        let deny = rng.chance(1, 3);
        let (read, write) = match rng.below(3) {
            0 => (true, false),
            1 => (false, true),
            _ => (true, true),
        };
        let include = match rng.below(8) {
            0 | 1 => vec![Who::Public],
            2 => vec![Who::Group],
            3 => vec![Who::Role],
            _ => rng.subset(USERS.len()).into_iter().map(Who::User).collect(),
        };
        let exclude = if rng.chance(1, 4) {
            vec![rng.below(USERS.len())]
        } else {
            vec![]
        };
        grants.push(GrantSpec {
            deny,
            read,
            write,
            scope: draw_scope(rng, 0, 3),
            include,
            exclude,
        });
    }

    let mut rules = Vec::new();
    for _ in 0..rng.below(4) {
        let by_tag = !used.is_empty() && rng.chance(1, 3);
        let filter = rng.chance(2, 5);
        let effect = if filter {
            RuleEffect::Filter(format!("region = '{}'", rng.pick(&REGIONS)))
        } else {
            let t = rng.pick(&MASK_TYPES).to_string();
            let expression = (t == "CUSTOM").then(|| "concat('x-', {col})".to_string());
            RuleEffect::Mask {
                mask_type: t,
                expression,
            }
        };
        let target = if by_tag {
            RuleTarget::Tag(*rng.pick(&used))
        } else if filter {
            RuleTarget::Path(draw_scope(rng, 1, 2))
        } else {
            // Mostly on one column; sometimes on a whole table, schema or
            // catalog (a mask naming no column, PR #837).
            let min = match rng.below(20) {
                0 => 0,
                1 => 1,
                2..=4 => 2,
                _ => 3,
            };
            RuleTarget::Path(draw_scope(rng, min, 3))
        };
        let exempt = if rng.chance(1, 2) {
            vec![rng.below(USERS.len())]
        } else {
            vec![]
        };
        rules.push(RuleSpec {
            effect,
            target,
            exempt,
        });
    }

    // I1: an agreeing tie. A mask of one type with no expression on one
    // column, or one filter on one table, written twice.
    let mut tie = None;
    if tie_state {
        let schema = rng.pick(&SCHEMAS).to_string();
        let table = rng.pick(&TABLES[..2]).to_string();
        let exempt = vec![rng.below(USERS.len())];
        let rule = if rng.chance(1, 2) {
            RuleSpec {
                effect: RuleEffect::Mask {
                    mask_type: rng.pick(&MASK_TYPES[..3]).to_string(),
                    expression: None,
                },
                target: RuleTarget::Path(ScopeSpec {
                    schema: Some(schema),
                    table: Some(table),
                    column: Some(rng.pick(&COLUMNS).to_string()),
                }),
                exempt,
            }
        } else {
            RuleSpec {
                effect: RuleEffect::Filter(format!("region = '{}'", rng.pick(&REGIONS))),
                target: RuleTarget::Path(ScopeSpec {
                    schema: Some(schema),
                    table: Some(table),
                    column: None,
                }),
                exempt,
            }
        };
        rules.push(rule.clone());
        tie = Some(rules.len());
        rules.push(rule);
    }

    let mut engine_grants: Vec<EngineGrantSpec> = Vec::new();
    for _ in 0..rng.below(3) {
        let g = if rng.chance(1, 2) {
            EngineGrantSpec::Impersonate {
                holder: rng.below(USERS.len()),
                target: rng.below(USERS.len()),
            }
        } else {
            EngineGrantSpec::SessionProperty {
                holder: rng.below(USERS.len()),
                property: rng.pick(&SYSTEM_PROPERTIES).to_string(),
            }
        };
        // One engine grant per (capability, values, target): a stored unique key.
        let key = |g: &EngineGrantSpec| match g {
            EngineGrantSpec::Impersonate { target, .. } => format!("i{target}"),
            EngineGrantSpec::SessionProperty { property, .. } => format!("p{property}"),
        };
        if !engine_grants.iter().any(|e| key(e) == key(&g)) {
            engine_grants.push(g);
        }
    }
    StateSpec {
        grants,
        rules,
        classes,
        gated,
        role_holders,
        group_members,
        engine_grants,
        spec: rng.pick(&SPECS),
        admin: rng.chance(1, 3).then(|| rng.below(USERS.len())),
        tie,
    }
}

// ── building a state in the database ────────────────────────────────────────

static SEEDED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

async fn db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    SEEDED
        .get_or_init(|| async {
            db.migrate().await.expect("migrate");
            seed_service_defs(&db).await;
        })
        .await;
    db
}

/// `fixtures::catalog` under the exact name `name` (the fixture prefixes
/// `gold_`): a registry catalog over `warehouse`, attached to `engine`.
async fn catalog_named(
    tx: &mut Tx<'_>,
    role: &str,
    name: &str,
    warehouse: Uuid,
    engine: Uuid,
    names: &mut Names,
) -> Uuid {
    let id = uid(names, role);
    sqlx::query(
        "INSERT INTO registry.catalogs (id, name, kind, config, warehouse_id) \
         VALUES ($1, $2, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $3::uuid::text)), $3)",
    )
    .bind(id)
    .bind(name)
    .bind(warehouse)
    .execute(&mut **tx)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine)
    .bind(id)
    .execute(&mut **tx)
    .await
    .expect("attach catalog");
    id
}

fn scope_of(cat: Uuid, s: &ScopeSpec) -> egret_perm::Scope {
    scope(
        cat,
        s.schema.as_deref(),
        s.table.as_deref(),
        s.column.as_deref(),
    )
}

/// The tag enricher is bypassed: the request carries the resolved tags
/// itself ([`tags_for`]), exactly as `egret-ranger-eval/tests/differential.rs`
/// does. The served service-def declares `RangerTagEnricher` with
/// `RangerAdminTagRetriever`, which would build a REST client to a Ranger
/// admin the harness does not run (and could overwrite the seeded tags), so
/// `contextEnrichers` is removed from both service-defs; nothing else is.
fn strip_enrichers(envelope: &mut Value) {
    for path in ["/serviceDef", "/tagPolicies/serviceDef"] {
        if let Some(o) = envelope.pointer_mut(path).and_then(Value::as_object_mut) {
            o.remove("contextEnrichers");
        }
    }
}

/// What one compiled state hands the comparison.
struct Built {
    envelope: Value,
    snapshot: Snapshot,
}

/// Build `spec` under `prefix` on a fresh transaction, compile it, read both
/// sides, roll back. `Err` carries a compile refusal.
async fn build_state(db: &Db, prefix: &str, spec: &StateSpec) -> Result<Built, CompileError> {
    let mut names = Names::default();
    let (profile, tag_service, _) = fixtures::profile(db, prefix, "trino", true, &mut names).await;
    assert!(tag_service.is_some(), "the profile has a tag service");
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, prefix, &mut names).await;
    let engine = engine_bound_spec(
        &mut tx,
        &format!("{prefix}.engine"),
        profile,
        "trino",
        spec_json(spec.spec),
        &mut names,
    )
    .await;
    let cat = catalog_named(
        &mut tx,
        &format!("{prefix}.lake"),
        CATALOG,
        wh,
        engine,
        &mut names,
    )
    .await;
    let mut user_ids = Vec::new();
    for u in USERS {
        user_ids.push(user(&mut tx, &format!("{prefix}.{u}"), Some(u), &mut names).await);
    }
    platform_role(&mut tx, CLEARING_ROLE, &mut names).await;
    for &h in &spec.role_holders {
        role_held(&mut tx, user_ids[h], CLEARING_ROLE).await;
    }
    let g = group(&mut tx, GROUP, &mut names).await;
    for &m in &spec.group_members {
        group_member(&mut tx, user_ids[m], g).await;
    }
    if let Some(a) = spec.admin {
        role_held(&mut tx, user_ids[a], "platform_admin").await;
    }
    let mut tag_ids = BTreeMap::new();
    for t in spec.tags_in_use() {
        tag_ids.insert(t, tag(&mut tx, TAGS[t], &mut names).await);
    }
    for c in &spec.classes {
        tag_column(
            &mut tx,
            tag_ids[&c.tag],
            wh,
            &c.schema,
            &c.table,
            c.column.as_deref().unwrap_or(""),
        )
        .await;
    }
    for &(t, by_group) in &spec.gated {
        if by_group {
            gate(&mut tx, tag_ids[&t], &[], &[g]).await;
        } else {
            gate(&mut tx, tag_ids[&t], &[CLEARING_ROLE], &[]).await;
        }
    }
    for (i, gs) in spec.grants.iter().enumerate() {
        let include: Vec<Principal> = gs
            .include
            .iter()
            .map(|w| match w {
                Who::User(u) => user_ref(USERS[*u]),
                Who::Public => group_ref("public"),
                Who::Group => group_ref(GROUP),
                Who::Role => role_ref(CLEARING_ROLE),
            })
            .collect();
        let exclude: Vec<Principal> = gs.exclude.iter().map(|u| user_ref(USERS[*u])).collect();
        let mut actions = Vec::new();
        if gs.read {
            actions.push(Action::Read);
        }
        if gs.write {
            actions.push(Action::Write);
        }
        grant(
            &mut tx,
            &format!("{prefix}.grant{i}"),
            if gs.deny { Effect::Deny } else { Effect::Allow },
            &actions,
            scope_of(cat, &gs.scope),
            set(&include, &exclude),
            &mut names,
        )
        .await;
    }
    for (i, rs) in spec.rules.iter().enumerate() {
        let kind = match &rs.effect {
            RuleEffect::Mask {
                mask_type,
                expression,
            } => mask(mask_type, expression.as_deref()),
            RuleEffect::Filter(e) => row_filter(e),
        };
        let selector = match &rs.target {
            RuleTarget::Path(s) => Selector::Path(scope_of(cat, s)),
            RuleTarget::Tag(t) => by_tag(TAGS[*t]),
        };
        let exempt: Vec<Principal> = rs.exempt.iter().map(|u| user_ref(USERS[*u])).collect();
        rule(
            &mut tx,
            &format!("{prefix}.rule{i}"),
            kind,
            selector,
            set(&exempt, &[]),
            &mut names,
        )
        .await;
    }
    for (i, eg) in spec.engine_grants.iter().enumerate() {
        match eg {
            EngineGrantSpec::Impersonate { holder, target } => {
                engine_grant(
                    &mut tx,
                    &format!("{prefix}.eg{i}"),
                    engine,
                    "trino.impersonate_user",
                    &[],
                    Some(user_ids[*target]),
                    &[user_ref(USERS[*holder])],
                    &mut names,
                )
                .await;
            }
            EngineGrantSpec::SessionProperty { holder, property } => {
                engine_grant(
                    &mut tx,
                    &format!("{prefix}.eg{i}"),
                    engine,
                    "trino.set_system_session_property",
                    &[property.as_str()],
                    None,
                    &[user_ref(USERS[*holder])],
                    &mut names,
                )
                .await;
            }
        }
    }
    let compiled = egret_perm_compile::recompile_service_in_tx(&mut tx, profile).await;
    if let Err(e) = compiled {
        tx.rollback().await.expect("rollback");
        return Err(e);
    }
    let mut envelope =
        egret_perm_compile::harness_support::served_artifact_json_via(&mut tx, profile)
            .await
            .expect("the served artifact");
    strip_enrichers(&mut envelope);
    let snapshot = egret_perm_compile::harness_support::build_snapshot_for_test(&mut tx, profile)
        .await
        .expect("the snapshot builds")
        .expect("a trino profile");
    tx.rollback().await.expect("rollback");
    Ok(Built { envelope, snapshot })
}

// ── anchors ─────────────────────────────────────────────────────────────────

/// What an anchor case must produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// No difference.
    Agree,
    /// A difference this `explained.toml` entry explains.
    Entry(&'static str),
}

/// A hand-built state with fixed questions, asked after the generated ones.
/// Anchors do not draw from the RNG, so the seeded corpus is unchanged by
/// them, and each explained entry (and each shape a mutant must be caught
/// on) is produced whatever the seed.
struct Anchor {
    name: &'static str,
    spec: StateSpec,
    questions: Vec<(Question, Expect)>,
}

fn empty_spec(spec: &'static str) -> StateSpec {
    StateSpec {
        grants: vec![],
        rules: vec![],
        classes: vec![],
        gated: vec![],
        role_holders: vec![],
        group_members: vec![],
        engine_grants: vec![],
        spec,
        admin: None,
        tie: None,
    }
}

fn at(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> ScopeSpec {
    ScopeSpec {
        schema: schema.map(str::to_string),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    }
}

fn grant_of(deny: bool, read: bool, write: bool, scope: ScopeSpec, include: Vec<Who>) -> GrantSpec {
    GrantSpec {
        deny,
        read,
        write,
        scope,
        include,
        exclude: vec![],
    }
}

fn ask(user: &str, op: &'static str, resource: Resource) -> Question {
    Question {
        user: user.to_string(),
        op,
        resource,
        target: None,
    }
}

fn column(schema: &str, table: &str, column: &str) -> Resource {
    Resource::Column {
        catalog: CATALOG.into(),
        schema: schema.into(),
        table: table.into(),
        column: column.into(),
        column_type: "varchar".into(),
    }
}

fn anchors() -> Vec<Anchor> {
    use Expect::{Agree, Entry};
    const LP: &str = LINEAGE_PRINCIPAL;
    let cols = |c: &[&str]| c.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    let lake = || Resource::Catalog(CATALOG.into());
    let schema = |s: &str| Resource::Schema {
        catalog: CATALOG.into(),
        schema: s.into(),
    };
    let t1 = || table("s1", "t1", vec![]);
    let files = || table("s1", "t3$files", vec![]);
    let reach_any = Entry("reach-any-is-answered-by-another-access-type");
    let metadata = Entry("a-metadata-table-is-reached-in-ranger");

    // u0 is denied Read on the catalog and allowed Write on it.
    // u1 is allowed Read on the catalog and denied Read on schema s1.
    let mut reach = empty_spec("open");
    reach.grants = vec![
        grant_of(true, true, false, at(None, None, None), vec![Who::User(0)]),
        grant_of(false, false, true, at(None, None, None), vec![Who::User(0)]),
        grant_of(false, true, false, at(None, None, None), vec![Who::User(1)]),
        grant_of(
            true,
            true,
            false,
            at(Some("s1"), None, None),
            vec![Who::User(1)],
        ),
    ];
    // u0 may write schema s1; a mask on t3 restricts u0, so M17 hides t3$files.
    let mut meta = empty_spec("open");
    meta.grants = vec![grant_of(
        false,
        false,
        true,
        at(Some("s1"), None, None),
        vec![Who::User(0)],
    )];
    meta.rules = vec![RuleSpec {
        effect: RuleEffect::Mask {
            mask_type: "MASK".into(),
            expression: None,
        },
        target: RuleTarget::Path(at(Some("s1"), Some("t3"), Some("email"))),
        exempt: vec![],
    }];
    // u0 reads the catalog, holds no clearance, and t1 carries a table-level
    // withheld classification (Task 3 mutation h).
    let mut withheld = empty_spec("open");
    withheld.grants = vec![grant_of(
        false,
        true,
        false,
        at(None, None, None),
        vec![Who::User(0)],
    )];
    withheld.classes = vec![ClassSpec {
        tag: 0,
        schema: "s1".into(),
        table: "t1".into(),
        column: None,
    }];
    withheld.gated = vec![(0, false)];
    // A JWT engine and public read and write on the catalog.
    let mut jwt = empty_spec("jwt");
    jwt.grants = vec![grant_of(
        false,
        true,
        true,
        at(None, None, None),
        vec![Who::Public],
    )];
    // A JWT engine and no grant: the transform's own read is Ranger's only.
    let jwt_bare = empty_spec("jwt");
    // An open engine and public write: no transform on Ranger.
    let mut open_write = empty_spec("open");
    open_write.grants = vec![grant_of(
        false,
        false,
        true,
        at(None, None, None),
        vec![Who::Public],
    )];
    // A table-scoped mask, a column mask exempting u1, and a row filter.
    let mut masks = empty_spec("open");
    let mask = |t: &str| RuleEffect::Mask {
        mask_type: t.into(),
        expression: None,
    };
    masks.rules = vec![
        RuleSpec {
            effect: mask("MASK"),
            target: RuleTarget::Path(at(Some("s1"), Some("t1"), Some("email"))),
            exempt: vec![1],
        },
        RuleSpec {
            effect: RuleEffect::Filter("region = 'eu'".into()),
            target: RuleTarget::Path(at(Some("s1"), Some("t1"), None)),
            exempt: vec![],
        },
    ];

    // Masks naming no column (PR #837): a table mask, and a catalog mask,
    // which Ranger renders with `information_schema` excluded and the
    // evaluator excludes alike.
    let mut table_mask = empty_spec("open");
    table_mask.rules = vec![RuleSpec {
        effect: mask("MASK_HASH"),
        target: RuleTarget::Path(at(Some("s1"), Some("t2"), None)),
        exempt: vec![],
    }];
    let mut catalog_mask = empty_spec("open");
    catalog_mask.rules = vec![RuleSpec {
        effect: mask("MASK"),
        target: RuleTarget::Path(at(None, None, None)),
        exempt: vec![],
    }];

    vec![
        Anchor {
            name: "table-mask",
            spec: table_mask,
            questions: vec![
                (ask("u0", "GetColumnMask", column("s1", "t2", "id")), Agree),
                (ask("u0", "GetColumnMask", column("s1", "t1", "id")), Agree),
            ],
        },
        Anchor {
            name: "catalog-mask",
            spec: catalog_mask,
            questions: vec![
                (ask("u0", "GetColumnMask", column("s1", "t2", "id")), Agree),
                (
                    ask(
                        "u0",
                        "GetColumnMask",
                        column("information_schema", "tables", "table_name"),
                    ),
                    Agree,
                ),
                (
                    ask(
                        "u0",
                        "GetColumnMask",
                        column("information_schema", "columns", "column_name"),
                    ),
                    Agree,
                ),
            ],
        },
        Anchor {
            name: "reach-any",
            spec: reach,
            questions: vec![
                (ask("u0", "AccessCatalog", lake()), reach_any),
                (ask("u0", "ShowSchemas", lake()), reach_any),
                (ask("u0", "FilterSchemas", schema("s1")), reach_any),
                (ask("u1", "FilterSchemas", schema("s1")), reach_any),
                (ask("u1", "AccessCatalog", lake()), Agree),
                (
                    ask("u0", "SelectFromColumns", table("s1", "t1", cols(&["id"]))),
                    Agree,
                ),
            ],
        },
        Anchor {
            name: "metadata",
            spec: meta,
            questions: vec![
                (ask("u0", "ShowColumns", files()), metadata),
                (ask("u0", "FilterTables", files()), metadata),
                (
                    ask(
                        "u0",
                        "FilterColumns",
                        table("s1", "t3$files", cols(&["id"])),
                    ),
                    metadata,
                ),
            ],
        },
        Anchor {
            name: "withheld-table",
            spec: withheld,
            questions: vec![
                // Measured: Ranger answers `_any` through the select allow
                // even though the SELF tag's clearance denies select, so
                // D2's withhold on reach is OPA-stricter. Mutation h (drop
                // classification withholds from reach) makes these agree,
                // which fails the anchor.
                (ask("u0", "FilterTables", t1()), reach_any),
                (ask("u0", "ShowColumns", t1()), reach_any),
                (
                    ask("u0", "FilterColumns", table("s1", "t1", cols(&["id"]))),
                    reach_any,
                ),
                (
                    ask("u0", "SelectFromColumns", table("s1", "t1", cols(&["id"]))),
                    Agree,
                ),
                (ask("u0", "FilterTables", table("s1", "t2", vec![])), Agree),
            ],
        },
        Anchor {
            name: "lineage-jwt",
            spec: jwt,
            questions: vec![
                (ask(LP, "TruncateTable", t1()), Agree),
                (ask(LP, "InsertIntoTable", t1()), Agree),
                (ask(LP, "CreateTable", t1()), Agree),
                (
                    ask(LP, "ImpersonateUser", Resource::User("u0".into())),
                    Agree,
                ),
                (
                    ask(LP, "SelectFromColumns", table("s1", "t1", cols(&["id"]))),
                    Agree,
                ),
                (ask("u0", "TruncateTable", t1()), Agree),
            ],
        },
        Anchor {
            name: "lineage-jwt-read",
            spec: jwt_bare,
            questions: vec![(
                ask(LP, "SelectFromColumns", table("s1", "t1", cols(&["id"]))),
                Entry("egret-lineage-is-served-only-by-ranger"),
            )],
        },
        Anchor {
            name: "lineage-open",
            spec: open_write,
            questions: vec![
                (
                    ask(LP, "TruncateTable", t1()),
                    Entry("egret-lineage-is-refused-on-every-engine"),
                ),
                (
                    ask(LP, "InsertIntoTable", t1()),
                    Entry("egret-lineage-is-refused-on-every-engine"),
                ),
                (ask("u0", "TruncateTable", t1()), Agree),
            ],
        },
        Anchor {
            name: "masks",
            spec: masks,
            questions: vec![
                (
                    ask("u1", "GetColumnMask", column("s1", "t1", "email")),
                    Entry("mask-none-is-no-mask"),
                ),
                (
                    ask("u0", "GetColumnMask", column("s1", "t1", "email")),
                    Agree,
                ),
                (ask("u0", "GetRowFilters", t1()), Agree),
            ],
        },
    ]
}

// ── questions ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Question {
    user: String,
    op: &'static str,
    resource: Resource,
    /// Rename*: the new name, which the plugin checks too (carry 7).
    target: Option<Resource>,
}

/// The coverage key of an operation: its class, with the data privilege and
/// the engine capability spelled out.
fn class_key(op: &str) -> String {
    format!("{:?}", classify(op).expect("a 483 operation"))
}

/// The class keys questions are drawn over, with their weights (of 100).
fn class_weights() -> Vec<(String, u32)> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for r in TRINO_483_OPERATIONS {
        keys.insert(class_key(r.name));
    }
    keys.into_iter()
        .map(|k| {
            let w = match k.as_str() {
                "ValueQuestion" => 42,
                "Data(Read)" => 15,
                "Reach" => 15,
                "AdminOnly" => 8,
                "Data(Write)" => 8,
                _ => 4,
            };
            (k, w)
        })
        .collect()
}

/// A coordinate a rule or classification names, for the value questions to
/// aim at: `(schema, table, column)`.
/// `mask` picks the mask rules' coordinates, else the filter rules'.
fn hot_coordinates(
    spec: &StateSpec,
    rng: &mut Rng,
    mask: bool,
) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    for r in &spec.rules {
        if matches!(r.effect, RuleEffect::Mask { .. }) != mask {
            continue;
        }
        match &r.target {
            RuleTarget::Path(s) => out.push((
                s.schema
                    .clone()
                    .unwrap_or_else(|| rng.pick(&SCHEMAS).to_string()),
                s.table
                    .clone()
                    .unwrap_or_else(|| rng.pick(&TABLES).to_string()),
                s.column.clone(),
            )),
            RuleTarget::Tag(t) => {
                for c in spec.classes.iter().filter(|c| c.tag == *t) {
                    if TABLES.contains(&c.table.as_str()) {
                        out.push((c.schema.clone(), c.table.clone(), c.column.clone()));
                    }
                }
            }
        }
    }
    out
}

fn table(schema: &str, table: &str, columns: Vec<String>) -> Resource {
    Resource::Table {
        catalog: CATALOG.to_string(),
        schema: schema.to_string(),
        table: table.to_string(),
        columns,
    }
}

fn draw_resource(rng: &mut Rng, op: &str, hot: &[(String, String, Option<String>)]) -> Resource {
    use to_ranger::Shape;
    let s = |v: &str| v.to_string();
    let aim = (!hot.is_empty() && rng.chance(4, 5)).then(|| rng.pick(hot).clone());
    match to_ranger::shape_of(op).unwrap_or_else(|| panic!("{op} has no shape")) {
        Shape::Catalog => Resource::Catalog(if rng.chance(1, 20) {
            s("other")
        } else {
            s(CATALOG)
        }),
        Shape::Schema => Resource::Schema {
            catalog: s(CATALOG),
            schema: s(rng.pick(&SCHEMAS)),
        },
        Shape::Table => match aim {
            Some((schema, t, _)) => table(&schema, &t, vec![]),
            None => table(rng.pick(&SCHEMAS), rng.pick(&TABLES), vec![]),
        },
        Shape::Columns => {
            let (schema, t) = match &aim {
                Some((schema, t, _)) => (schema.clone(), t.clone()),
                None => (s(rng.pick(&SCHEMAS)), s(rng.pick(&TABLES))),
            };
            let columns: Vec<String> = if op == "FilterColumns" {
                vec![s(rng.pick(&COLUMNS))]
            } else if rng.chance(1, 8) {
                // C2: `count(*)` asks with no columns.
                vec![]
            } else {
                let mut c: Vec<String> = COLUMNS
                    .iter()
                    .filter(|_| rng.chance(2, 5))
                    .map(|c| s(c))
                    .collect();
                if c.is_empty() {
                    c.push(s(rng.pick(&COLUMNS)));
                }
                c
            };
            table(&schema, &t, columns)
        }
        Shape::Column => {
            let (schema, t, c) = match aim {
                Some((schema, t, Some(c))) => (schema, t, c),
                Some((schema, t, None)) => (schema, t, s(rng.pick(&COLUMNS))),
                None => (
                    s(rng.pick(&SCHEMAS)),
                    s(rng.pick(&TABLES)),
                    s(rng.pick(&COLUMNS)),
                ),
            };
            Resource::Column {
                catalog: s(CATALOG),
                schema,
                table: t,
                column: c,
                column_type: s("varchar"),
            }
        }
        Shape::Function | Shape::Procedure => Resource::Function {
            catalog: s(CATALOG),
            schema: s(rng.pick(&SCHEMAS)),
            function: s("f1"),
        },
        Shape::User => Resource::User(if rng.chance(1, 8) {
            s("root")
        } else {
            s(rng.pick(&USERS))
        }),
        Shape::SystemProperty => Resource::SystemProperty(s(rng.pick(&SYSTEM_PROPERTIES))),
        Shape::CatalogSessionProperty => Resource::CatalogSessionProperty {
            catalog: s(CATALOG),
            property: s("p1"),
        },
        Shape::QueryId | Shape::SysInfo => Resource::None,
    }
}

/// The new name of a Rename*: the same shape, another object.
fn rename_target(resource: &Resource) -> Resource {
    match resource {
        Resource::Schema { catalog, .. } => Resource::Schema {
            catalog: catalog.clone(),
            schema: "s9".into(),
        },
        Resource::Table {
            catalog, schema, ..
        } => Resource::Table {
            catalog: catalog.clone(),
            schema: schema.clone(),
            table: "t9".into(),
            columns: vec![],
        },
        other => panic!("a rename of {other:?}"),
    }
}

fn draw_questions(rng: &mut Rng, spec: &StateSpec) -> Vec<Question> {
    let weights = class_weights();
    let total: u32 = weights.iter().map(|(_, w)| w).sum();
    let hot_masks = hot_coordinates(spec, rng, true);
    // Reach and data questions also aim at the withheld classifications, so
    // a reach on an object a withhold covers is asked (Task 3 mutation h).
    let mut hot_withheld = Vec::new();
    for c in &spec.classes {
        let gated = spec
            .gated
            .iter()
            .any(|(t, by_group)| *t == c.tag && !by_group);
        if gated && TABLES.contains(&c.table.as_str()) {
            hot_withheld.push((c.schema.clone(), c.table.clone(), c.column.clone()));
        }
    }
    if hot_withheld.is_empty() {
        hot_withheld = hot_masks.clone();
    }
    let hot_filters = hot_coordinates(spec, rng, false);
    let mut out = Vec::new();
    for q in 0..QUESTIONS {
        let user = if rng.chance(1, 12) {
            LINEAGE_PRINCIPAL.to_string()
        } else {
            rng.pick(&USERS).to_string()
        };
        // In a tie state the first questions aim at the tie (I1).
        let tie_question = spec.tie.filter(|_| q < 6).map(|i| &spec.rules[i]);
        let op: &'static str = if let Some(r) = tie_question {
            match r.effect {
                RuleEffect::Mask { .. } => "GetColumnMask",
                RuleEffect::Filter(_) => "GetRowFilters",
            }
        } else {
            let mut x = rng.below(total as usize) as u32;
            let key = weights
                .iter()
                .find(|(_, w)| {
                    if x < *w {
                        true
                    } else {
                        x -= w;
                        false
                    }
                })
                .map(|(k, _)| k.clone())
                .expect("a class");
            let ops: Vec<&'static str> = TRINO_483_OPERATIONS
                .iter()
                .map(|r| r.name)
                .filter(|n| class_key(n) == key)
                .collect();
            rng.pick(&ops)
        };
        let resource = match tie_question.map(|r| &r.target) {
            Some(RuleTarget::Path(s)) => {
                let (schema, t) = (s.schema.clone().unwrap(), s.table.clone().unwrap());
                match &s.column {
                    Some(c) => Resource::Column {
                        catalog: CATALOG.into(),
                        schema,
                        table: t,
                        column: c.clone(),
                        column_type: "varchar".into(),
                    },
                    None => table(&schema, &t, vec![]),
                }
            }
            _ => {
                let hot = if op == "GetRowFilters" {
                    &hot_filters
                } else if op == "GetColumnMask" {
                    &hot_masks
                } else {
                    &hot_withheld
                };
                draw_resource(rng, op, hot)
            }
        };
        // `checkCanRenameColumn` asks ALTER on the table only; the other
        // renames ask ALTER on the source and on the new name.
        let target =
            (op.starts_with("Rename") && op != "RenameColumn").then(|| rename_target(&resource));
        out.push(Question {
            user,
            op,
            resource,
            target,
        });
    }
    out
}

// ── outcomes ────────────────────────────────────────────────────────────────

/// What one side answered. Masks keep the masked value only for `CUSTOM`,
/// the one type whose value the plugin reads (`getColumnMask`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Allow,
    Deny,
    Mask(Option<(String, Option<String>)>),
    Filter(Option<String>),
    /// The evaluator refused to answer (`EvalError::TwoMasks`): the query fails.
    Fails(String),
}

impl Outcome {
    /// The outcome's class, for grouping differences.
    fn kind(&self) -> String {
        match self {
            Outcome::Allow => "allow".into(),
            Outcome::Deny => "deny".into(),
            Outcome::Mask(None) => "no mask".into(),
            Outcome::Mask(Some((t, _))) => format!("mask {t}"),
            Outcome::Filter(None) => "no filter".into(),
            Outcome::Filter(Some(_)) => "filter".into(),
            Outcome::Fails(_) => "query fails".into(),
        }
    }
}

fn mask_outcome(m: Option<(String, Option<String>)>) -> Outcome {
    Outcome::Mask(m.map(|(t, v)| {
        let v = if t == "CUSTOM" { v } else { None };
        (t, v)
    }))
}

/// A single filter's outer parentheses, which the evaluator adds around each
/// conjunct; anything else is returned as is.
fn strip_single_parens(e: &str) -> String {
    let Some(inner) = e.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
        return e.to_string();
    };
    let mut depth = 0i32;
    for ch in inner.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return e.to_string();
                }
            }
            _ => {}
        }
    }
    if depth == 0 {
        inner.to_string()
    } else {
        e.to_string()
    }
}

/// The evaluator's answer, and why (the deciding statement or the reason).
fn opa_outcome(snap: &Snapshot, q: &Question) -> (Outcome, String) {
    let why = |d: Decision| match d {
        Decision::Allow { by } => by,
        Decision::Deny { reason, .. } => reason,
    };
    match (q.op, &q.resource) {
        ("GetColumnMask" | "GetRowFilters", _) => (value_outcome(snap, q), String::new()),
        (op, r) if op.starts_with("Filter") => {
            let o = if snap.filter(&q.user, op, std::slice::from_ref(r)) == [0] {
                Outcome::Allow
            } else {
                Outcome::Deny
            };
            (o, why(snap.decide(&q.user, op, r)))
        }
        (op, r) => match snap.decide(&q.user, op, r) {
            d @ Decision::Allow { .. } => (Outcome::Allow, why(d)),
            d @ Decision::Deny { .. } => (Outcome::Deny, why(d)),
        },
    }
}

fn value_outcome(snap: &Snapshot, q: &Question) -> Outcome {
    let fails = |e: EvalError| Outcome::Fails(e.to_string());
    match (q.op, &q.resource) {
        (
            "GetColumnMask",
            Resource::Column {
                catalog,
                schema,
                table,
                column,
                ..
            },
        ) => match snap.column_mask_choice(&q.user, catalog, schema, table, column) {
            Ok(m) => mask_outcome(m),
            Err(e) => fails(e),
        },
        (
            "GetRowFilters",
            Resource::Table {
                catalog,
                schema,
                table,
                ..
            },
        ) => match snap.row_filters(&q.user, catalog, schema, table) {
            Ok(v) => match v.as_slice() {
                [] => Outcome::Filter(None),
                [one] => Outcome::Filter(Some(strip_single_parens(one))),
                more => panic!("row_filters returned {} expressions: {more:?}", more.len()),
            },
            Err(e) => fails(e),
        },
        (op, r) => panic!("{op} on {r:?} is not a value question"),
    }
}

/// The tags of one Ranger ask, from the state's own classifications (I7).
fn tags_for(spec: &StateSpec, ask: &to_ranger::RangerAsk) -> Vec<Value> {
    let get = |k: &str| {
        ask.resource
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    let mut out: Vec<(String, &'static str)> = Vec::new();
    if get("catalog") == Some(CATALOG) {
        let (schema, table, column) = (get("schema"), get("table"), get("column"));
        let data_keys = ask
            .resource
            .iter()
            .all(|(k, _)| ["catalog", "schema", "table", "column"].contains(&k.as_str()));
        for c in &spec.classes {
            let name = TAGS[c.tag].to_string();
            let m = match (schema, table, column) {
                (Some(s), Some(t), Some(col)) if s == c.schema && t == c.table => match &c.column {
                    Some(cc) if cc == col => Some("SELF"),
                    Some(_) => None,
                    None => Some("ANCESTOR"),
                },
                (Some(s), Some(t), None) if s == c.schema && t == c.table => match &c.column {
                    None => Some("SELF"),
                    Some(_) => Some("DESCENDANT"),
                },
                (Some(s), None, None) if s == c.schema && ask.access_type == "_any" => {
                    Some("DESCENDANT")
                }
                (None, None, None) if ask.access_type == "_any" => Some("DESCENDANT"),
                _ => None,
            };
            if let (Some(m), true) = (m, data_keys) {
                if !out.contains(&(name.clone(), m)) {
                    out.push((name, m));
                }
            }
        }
    }
    out.into_iter()
        .map(|(n, m)| json!({"type": n, "matchType": m, "attributes": {}}))
        .collect()
}

/// The policy type of a question's asks (carry 4): row filter 2, mask 1,
/// access 0.
fn policy_type(op: &str) -> i64 {
    match op {
        "GetRowFilters" => 2,
        "GetColumnMask" => 1,
        _ => 0,
    }
}

fn ranger_case(
    id: String,
    envelope: &Value,
    op: &str,
    user: &str,
    ask: &to_ranger::RangerAsk,
    tags: Vec<Value>,
) -> Value {
    let resource: serde_json::Map<String, Value> = ask
        .resource
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mut request = json!({
        "user": user,
        "groups": [],
        "roles": [],
        "accessType": ask.access_type,
        "resource": resource,
    });
    if let Some(scope) = ask.matching_scope {
        request["resourceMatchingScope"] = json!(scope);
    }
    if !tags.is_empty() {
        request["tags"] = Value::Array(tags);
    }
    json!({
        "id": id,
        "serviceType": "trino",
        "policyType": policy_type(op),
        "servicePolicies": envelope,
        "request": request,
    })
}

fn ranger_outcome(op: &str, verdicts: &[&Value]) -> Outcome {
    for v in verdicts {
        assert!(
            v["error"].is_null(),
            "the Ranger engine threw on {}: {}",
            v["id"],
            v["error"]
        );
    }
    let s = |v: &Value| v.as_str().map(str::to_string);
    match op {
        "GetColumnMask" => {
            let v = verdicts[0];
            mask_outcome(s(&v["maskType"]).map(|t| (t, s(&v["maskedValue"]))))
        }
        "GetRowFilters" => Outcome::Filter(s(&verdicts[0]["filterExpr"]).filter(|e| !e.is_empty())),
        _ => {
            if verdicts.iter().all(|v| v["isAllowed"] == json!(true)) {
                Outcome::Allow
            } else {
                Outcome::Deny
            }
        }
    }
}

// ── differences and explained.toml ──────────────────────────────────────────

/// One case where the two sides disagree.
#[derive(Debug, Clone)]
struct Diff {
    state: usize,
    question: usize,
    q: Question,
    ranger: Outcome,
    opa: Outcome,
    /// The deciding Ranger policies' names, one per ask (`-` when none decided).
    ranger_why: Vec<String>,
    /// The evaluator's deciding statement or reason.
    opa_why: String,
    /// The state the case was asked in.
    spec: StateSpec,
}

/// A filter's top-level conjuncts as the evaluator writes them.
fn conjuncts(e: &str) -> Vec<String> {
    e.split(" AND ").map(strip_single_parens).collect()
}

/// Ruling (b): OPA allows, shows or unmasks what Ranger refuses, filters or
/// masks. Such a difference is never explainable.
fn widens(ranger: &Outcome, opa: &Outcome) -> bool {
    let masks =
        |m: &Option<(String, Option<String>)>| m.as_ref().is_some_and(|(t, _)| t != "MASK_NONE");
    match (ranger, opa) {
        (Outcome::Deny, Outcome::Allow) => true,
        (Outcome::Filter(Some(r)), Outcome::Filter(o)) => {
            o.as_ref().is_none_or(|o| !conjuncts(o).contains(r))
        }
        // Final review: any other mask than the one Ranger applies may be weaker
        // (MASK_SHOW_LAST_4 for MASK_HASH), so a different mask is never explainable.
        (Outcome::Mask(r), Outcome::Mask(o)) => masks(r) && o != r,
        _ => false,
    }
}

#[derive(Debug, serde::Deserialize)]
struct ExplainedFile {
    #[serde(default)]
    difference: Vec<Entry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    operations: Vec<String>,
    mechanism: String,
    right: String,
    why: String,
}

const EXPLAINED: &str = include_str!("opa_differential/explained.toml");

fn explained() -> Vec<Entry> {
    toml::from_str::<ExplainedFile>(EXPLAINED)
        .expect("explained.toml parses")
        .difference
}

type Predicate = fn(&Diff) -> bool;

/// One predicate per `explained.toml` id, named after it. A table rather than
/// a `match` on the id: a `match` over `&str` needs a wildcard arm, which is
/// what the pairing must not have; [`every_explained_entry_has_a_predicate`]
/// asserts the pairing both ways instead.
const PREDICATES: &[(&str, Predicate)] = &[
    (
        "egret-lineage-is-served-only-by-ranger",
        egret_lineage_is_served_only_by_ranger,
    ),
    ("mask-none-is-no-mask", mask_none_is_no_mask),
    (
        "egret-lineage-is-refused-on-every-engine",
        egret_lineage_is_refused_on_every_engine,
    ),
    (
        "reach-any-is-answered-by-another-access-type",
        reach_any_is_answered_by_another_access_type,
    ),
    (
        "a-metadata-table-is-reached-in-ranger",
        a_metadata_table_is_reached_in_ranger,
    ),
];

fn is_reach(op: &str) -> bool {
    classify(op) == Some(OpClass::Reach)
}

/// The levels `(schema, table, column)` a resource names under `lake`.
fn levels(r: &Resource) -> Option<[Option<&str>; 3]> {
    match r {
        Resource::Catalog(c) if c == CATALOG => Some([None, None, None]),
        Resource::Schema { catalog, schema } if catalog == CATALOG => {
            Some([Some(schema), None, None])
        }
        Resource::Table {
            catalog,
            schema,
            table,
            columns,
        } if catalog == CATALOG => Some([
            Some(schema),
            Some(table),
            columns.first().map(String::as_str),
        ]),
        _ => None,
    }
}

/// Whether a deny grant or a gated classification of `spec` reaches the
/// object from above (on it, or on an ancestor) and from strictly below.
fn denied_around(spec: &StateSpec, r: &Resource) -> (bool, bool) {
    let Some(obj) = levels(r) else {
        return (false, false);
    };
    let mut targets: Vec<[Option<&str>; 3]> = spec
        .grants
        .iter()
        .filter(|g| g.deny)
        .map(|g| {
            [
                g.scope.schema.as_deref(),
                g.scope.table.as_deref(),
                g.scope.column.as_deref(),
            ]
        })
        .collect();
    for c in &spec.classes {
        if spec
            .gated
            .iter()
            .any(|(t, by_group)| *t == c.tag && !by_group)
        {
            targets.push([Some(&c.schema), Some(&c.table), c.column.as_deref()]);
        }
    }
    let (mut above, mut below) = (false, false);
    for t in targets {
        // Agree on every level both name.
        let agree = (0..3).all(|i| match (obj[i], t[i]) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        });
        if !agree {
            continue;
        }
        let depth = |l: &[Option<&str>; 3]| l.iter().filter(|x| x.is_some()).count();
        if depth(&t) <= depth(&obj) {
            above = true;
        } else {
            below = true;
        }
    }
    (above, below)
}

/// Carry 1. The lineage transform (`egret_policy::lineage_transform`) is
/// applied to the Ranger download of a profile whose engine runs in JWT mode;
/// the OPA snapshot names no `egret-lineage` statement at all.
fn egret_lineage_is_served_only_by_ranger(d: &Diff) -> bool {
    d.q.user == LINEAGE_PRINCIPAL && d.spec.spec == "jwt"
}

/// The ruling of SP2b Task 18: on OPA the lineage principal is refused what
/// the lineage transform denies, on every engine. Ranger applies the
/// transform only for a JWT engine, so elsewhere it treats the name as an
/// ordinary user and allows what the grants allow.
fn egret_lineage_is_refused_on_every_engine(d: &Diff) -> bool {
    d.q.user == LINEAGE_PRINCIPAL
        && d.spec.spec != "jwt"
        && d.ranger == Outcome::Allow
        && d.opa == Outcome::Deny
}

/// Ranger answers `MASK_NONE`, the evaluator no mask: the plugin masks
/// neither.
fn mask_none_is_no_mask(d: &Diff) -> bool {
    matches!(&d.ranger, Outcome::Mask(Some((t, None))) if t == "MASK_NONE")
        && d.opa == Outcome::Mask(None)
}

/// A reach question the evaluator refuses because a deny or withhold on the
/// object or an ancestor hides it, and Ranger allows.
fn reach_any_is_answered_by_another_access_type(d: &Diff) -> bool {
    is_reach(d.q.op)
        && d.ranger == Outcome::Allow
        && d.opa == Outcome::Deny
        && d.opa_why.contains(" hides ")
}

/// Carry 2. A reach question on a `$` table the evaluator refuses by M17
/// (restricted on the base table), and Ranger allows.
fn a_metadata_table_is_reached_in_ranger(d: &Diff) -> bool {
    is_reach(d.q.op)
        && d.ranger == Outcome::Allow
        && d.opa == Outcome::Deny
        && d.opa_why.contains("is a metadata table of")
}

fn predicate(id: &str) -> Option<Predicate> {
    PREDICATES.iter().find(|(i, _)| *i == id).map(|(_, p)| *p)
}

/// The leaf and access type each 483 operation row names (what the lineage
/// refusal reads) are the ones the plugin's check asks.
#[test]
fn every_operation_row_names_the_plugins_leaf_and_access() {
    for r in TRINO_483_OPERATIONS {
        let asks = to_ranger::ranger_asks(r.name, &to_ranger::sample_resource(r.name))
            .unwrap_or_else(|| panic!("{} has no translation", r.name));
        for a in &asks {
            let leaf = &a.resource.last().expect("a resource").0;
            assert_eq!(
                (leaf.as_str(), a.access_type.as_str()),
                (r.leaf, r.access),
                "{}",
                r.name
            );
        }
    }
}

#[test]
fn every_explained_entry_has_a_predicate() {
    let entries = explained();
    let mut ids = BTreeSet::new();
    for e in &entries {
        assert!(ids.insert(e.id.as_str()), "{} is listed twice", e.id);
        assert!(predicate(&e.id).is_some(), "{} has no predicate", e.id);
        assert!(!e.operations.is_empty(), "{} names no operation", e.id);
        for op in &e.operations {
            assert!(
                classify(op).is_some(),
                "{}: {op} is not a 483 operation",
                e.id
            );
        }
        assert!(
            e.mechanism.trim().len() > 20,
            "{} does not state its mechanism",
            e.id
        );
        assert!(!e.why.trim().is_empty(), "{} does not say why", e.id);
        assert!(
            ["ranger", "egret", "both"].contains(&e.right.as_str()),
            "{}: right = {:?}",
            e.id,
            e.right
        );
    }
    for (id, _) in PREDICATES {
        assert!(
            ids.contains(id),
            "predicate {id} has no explained.toml entry"
        );
    }
}

fn reproducer(seed: u64) -> String {
    format!(
        "EGRET_OPA_DIFF_SEED={seed:#x} env -u EGRET_ALLOW_DB_SKIP EGRET_TEST_RANGER_E2E=1 \
         DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
         cargo test -p egret-perm-compile --features harness --test opa_differential -- --nocapture"
    )
}

fn describe(seed: u64, d: &Diff) -> String {
    format!(
        "seed {seed:#x} state {} question {}: {} {} on {:?}{}: ranger {:?}, opa {:?}",
        d.state,
        d.question,
        d.q.user,
        d.q.op,
        d.q.resource,
        d.q.target
            .as_ref()
            .map(|t| format!(" (target {t:?})"))
            .unwrap_or_default(),
        d.ranger,
        d.opa
    ) + &format!(" [ranger by {:?}; opa: {}]", d.ranger_why, d.opa_why)
}

/// Policy id → name over the resource and tag policies of one download.
fn policy_names_of(envelope: &Value) -> BTreeMap<i64, String> {
    let mut out = BTreeMap::new();
    for path in ["/policies", "/tagPolicies/policies"] {
        for p in envelope
            .pointer(path)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(id), Some(n)) = (p["id"].as_i64(), p["name"].as_str()) {
                out.insert(id, n.to_string());
            }
        }
    }
    out
}

// ── the run ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn opa_decisions_agree_with_ranger() {
    if std::env::var("EGRET_TEST_RANGER_E2E").as_deref() != Ok("1") {
        eprintln!("skipped: EGRET_TEST_RANGER_E2E unset");
        return;
    }
    let seed = seed();
    let db = db().await;
    let mut rng = Rng::new(seed);

    struct CaseRef {
        state: usize,
        question: usize,
        q: Question,
        opa: Outcome,
        opa_why: String,
        asks: std::ops::Range<usize>,
        tie: bool,
        /// An anchor case: (anchor name, what it must produce).
        expect: Option<(&'static str, Expect)>,
    }
    let mut policy_names: Vec<BTreeMap<i64, String>> = Vec::new();
    let mut payload: Vec<Value> = Vec::new();
    let mut cases: Vec<CaseRef> = Vec::new();
    let mut specs: Vec<StateSpec> = Vec::new();
    let mut redraws = 0usize;
    let mut tie_states = 0usize;

    let anchors = anchors();
    for state in 0..STATES + anchors.len() {
        let anchor = state.checked_sub(STATES).map(|i| &anchors[i]);
        // I1: one state in five carries an agreeing tie.
        let tie_state = state % 5 == 2;
        let mut attempt = 0;
        let (spec, built) = loop {
            if let Some(a) = anchor {
                let built = build_state(&db, &format!("l1a{state}"), &a.spec)
                    .await
                    .unwrap_or_else(|e| panic!("anchor {}: the compile failed: {e}", a.name));
                break (a.spec.clone(), built);
            }
            let spec = draw_state(&mut rng, tie_state);
            match build_state(&db, &format!("l1s{state}r{attempt}"), &spec).await {
                Ok(b) => break (spec, b),
                Err(CompileError::AmbiguousMaskOrFilterArbitration { .. }) => {
                    redraws += 1;
                    attempt += 1;
                    assert!(
                        attempt < MAX_REDRAWS,
                        "seed {seed:#x} state {state}: {MAX_REDRAWS} refused draws in a row"
                    );
                }
                Err(e) => panic!(
                    "seed {seed:#x} state {state}: the compile failed for a reason other than \
                     an overlap refusal: {e}\nstate: {spec:#?}\nreproduce: {}",
                    reproducer(seed)
                ),
            }
        };
        if spec.tie.is_some() {
            tie_states += 1;
        }
        let (questions, expects): (Vec<Question>, Vec<Option<Expect>>) = match anchor {
            Some(a) => a
                .questions
                .iter()
                .map(|(q, e)| (q.clone(), Some(*e)))
                .unzip(),
            None => draw_questions(&mut rng, &spec)
                .into_iter()
                .map(|q| (q, None))
                .unzip(),
        };
        for (qi, (q, expect)) in questions.into_iter().zip(expects).enumerate() {
            let mut asks = to_ranger::ranger_asks(q.op, &q.resource).unwrap_or_else(|| {
                panic!(
                    "harness failure (carry 3): no Ranger translation for {} on {:?}",
                    q.op, q.resource
                )
            });
            if let Some(t) = &q.target {
                asks.extend(to_ranger::ranger_asks(q.op, t).unwrap_or_else(|| {
                    panic!(
                        "harness failure (carry 3): no Ranger translation for {} target {t:?}",
                        q.op
                    )
                }));
            }
            let start = payload.len();
            for (ai, ask) in asks.iter().enumerate() {
                payload.push(ranger_case(
                    format!("s{state}q{qi}a{ai}"),
                    &built.envelope,
                    q.op,
                    &q.user,
                    ask,
                    tags_for(&spec, ask),
                ));
            }
            let (opa, opa_why) = opa_outcome(&built.snapshot, &q);
            // A tie case: a model user both tie rules apply to.
            let tie = spec.tie.is_some_and(|i| {
                qi < 6
                    && USERS.contains(&q.user.as_str())
                    && !spec.rules[i].exempt.iter().any(|u| USERS[*u] == q.user)
            });
            cases.push(CaseRef {
                state,
                question: qi,
                q,
                opa,
                opa_why,
                asks: start..payload.len(),
                tie,
                expect: expect.map(|e| (anchor.map_or("", |a| a.name), e)),
            });
        }
        policy_names.push(policy_names_of(&built.envelope));
        specs.push(spec);
    }

    let verdicts = ranger_jvm::ask_ranger_json(payload, RANGER_VERSION, "opa-l1");

    // ── compare ──
    let mut asked: BTreeMap<String, usize> = BTreeMap::new();
    let mut outcomes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut select_ops, mut empty_selects, mut ties, mut tie_disagreements) =
        (0, 0, 0, Vec::new());
    // Reach questions on an object a deny or withhold reaches from above (or
    // on it), and from below.
    let (mut reach_denied_above, mut reach_denied_below) = (0, 0);
    // Reach questions the evaluator refuses by a classification withhold,
    // and of those the ones Ranger refuses too (Task 3 mutation h).
    let (mut reach_withheld, mut reach_withheld_agreed) = (0, 0);
    let mut diffs: Vec<Diff> = Vec::new();
    for c in &cases {
        *asked.entry(class_key(c.q.op)).or_default() += 1;
        if is_reach(c.q.op) {
            let (above, below) = denied_around(&specs[c.state], &c.q.resource);
            reach_denied_above += usize::from(above);
            reach_denied_below += usize::from(below);
        }
        if c.q.op == "SelectFromColumns" {
            select_ops += 1;
            if matches!(&c.q.resource, Resource::Table { columns, .. } if columns.is_empty()) {
                empty_selects += 1;
            }
        }
        let vs: Vec<&Value> = verdicts[c.asks.clone()].iter().collect();
        for (i, v) in vs.iter().enumerate() {
            let want = format!("s{}q{}a{}", c.state, c.question, i);
            assert_eq!(v["id"], json!(want), "verdict order");
        }
        let ranger = ranger_outcome(c.q.op, &vs);
        if is_reach(c.q.op) && c.opa_why.starts_with("withhold ") {
            reach_withheld += 1;
            reach_withheld_agreed += usize::from(ranger == Outcome::Deny);
        }
        let bucket = match &ranger {
            Outcome::Allow => "allow",
            Outcome::Deny => "deny",
            Outcome::Mask(Some((t, _))) if t != "MASK_NONE" => "masked",
            Outcome::Mask(_) => "unmasked",
            Outcome::Filter(Some(_)) => "filtered",
            Outcome::Filter(None) => "unfiltered",
            Outcome::Fails(_) => unreachable!("Ranger answers"),
        };
        *outcomes.entry(bucket).or_default() += 1;
        if c.tie {
            ties += 1;
        }
        if ranger != c.opa {
            let d = Diff {
                state: c.state,
                question: c.question,
                q: c.q.clone(),
                ranger,
                opa: c.opa.clone(),
                ranger_why: vs
                    .iter()
                    .map(|v| {
                        v["policyId"]
                            .as_i64()
                            .and_then(|id| policy_names[c.state].get(&id).cloned())
                            .unwrap_or_else(|| "-".into())
                    })
                    .collect(),
                opa_why: c.opa_why.clone(),
                spec: specs[c.state].clone(),
            };
            if c.tie {
                tie_disagreements.push(describe(seed, &d));
            }
            diffs.push(d);
        }
    }

    // ── classify ──
    let entries = explained();
    // explained.toml id → (cases, first example)
    let mut used: BTreeMap<&str, (usize, String)> = BTreeMap::new();
    let mut forbidden: Vec<String> = Vec::new();
    let mut unexplained: Vec<String> = Vec::new();
    // (operation, Ranger outcome, OPA outcome) → (cases, entry, first example)
    type Class<'a> = (usize, Option<&'a str>, String);
    let mut classes: BTreeMap<(String, String, String), Class> = BTreeMap::new();
    // (state, question) → the entry that explained it, or "forbidden".
    let mut verdict_of: BTreeMap<(usize, usize), String> = BTreeMap::new();
    for d in &diffs {
        let claimed: Vec<&str> = entries
            .iter()
            .filter(|e| e.operations.iter().any(|o| o == d.q.op))
            .filter(|e| predicate(&e.id).is_some_and(|p| p(d)))
            .map(|e| e.id.as_str())
            .collect();
        let key = (d.q.op.to_string(), d.ranger.kind(), d.opa.kind());
        let class = classes
            .entry(key)
            .or_insert_with(|| (0, claimed.first().copied(), describe(seed, d)));
        class.0 += 1;
        if matches!(d.opa, Outcome::Fails(_)) || widens(&d.ranger, &d.opa) {
            forbidden.push(format!(
                "{}{}",
                describe(seed, d),
                if claimed.is_empty() {
                    String::new()
                } else {
                    format!(" (explained.toml entries {claimed:?} claim it; they cannot)")
                }
            ));
            verdict_of.insert((d.state, d.question), "forbidden".into());
            continue;
        }
        if let Some(id) = claimed.first() {
            verdict_of.insert((d.state, d.question), (*id).to_string());
        }
        match claimed.first() {
            Some(id) => {
                used.entry(id).or_insert_with(|| (0, describe(seed, d))).0 += 1;
            }
            None => unexplained.push(describe(seed, d)),
        }
    }
    // The anchors (deterministic, independent of the seed) produce what
    // they are built for, so no explained entry can go stale with the corpus.
    let mut anchor_failures = Vec::new();
    for c in &cases {
        let Some((name, expect)) = c.expect else {
            continue;
        };
        let got = verdict_of.get(&(c.state, c.question)).map(String::as_str);
        let ok = match expect {
            Expect::Agree => {
                got.is_none()
                    && !diffs
                        .iter()
                        .any(|d| (d.state, d.question) == (c.state, c.question))
            }
            Expect::Entry(id) => got == Some(id),
        };
        if !ok {
            anchor_failures.push(format!(
                "anchor {name}, question {} ({} {} on {:?}): expected {expect:?}, got {got:?}",
                c.question, c.q.user, c.q.op, c.q.resource
            ));
        }
    }
    let stale_entries: Vec<&str> = entries
        .iter()
        .map(|e| e.id.as_str())
        .filter(|id| !used.contains_key(id))
        .collect();

    eprintln!(
        "opa_differential: seed {seed:#x}; {} states ({tie_states} with a tie), {} cases, {} \
         Ranger asks, {redraws} redraws; ties {ties}; differences {}",
        specs.len(),
        cases.len(),
        verdicts.len(),
        diffs.len()
    );
    eprintln!("opa_differential: asked per class {asked:?}");
    eprintln!("opa_differential: Ranger outcomes {outcomes:?}");
    eprintln!("opa_differential: SelectFromColumns {select_ops}, of which empty {empty_selects}");
    eprintln!(
        "opa_differential: reach questions with a deny or withhold above {reach_denied_above}, \
         below {reach_denied_below}; refused by a withhold {reach_withheld}, Ranger agreeing \
         {reach_withheld_agreed}"
    );
    for (id, (n, example)) in &used {
        eprintln!("opa_differential: entry {id}: {n} case(s); e.g. {example}");
    }
    for ((op, r, o), (n, id, example)) in &classes {
        eprintln!(
            "opa_differential: class {op} ranger={r} opa={o}: {n} case(s), entry {id:?}; e.g. {example}"
        );
    }

    for f in &forbidden {
        eprintln!("opa_differential: FORBIDDEN {f}");
    }
    for u in &unexplained {
        eprintln!("opa_differential: UNEXPLAINED {u}");
    }
    assert!(
        forbidden.is_empty(),
        "OPA allows what Ranger denies: fix the evaluator ({} case(s)):\n{}\nreproduce: {}",
        forbidden.len(),
        forbidden.join("\n"),
        reproducer(seed)
    );
    assert!(
        tie_disagreements.is_empty(),
        "an agreeing tie (I1) answered differently:\n{}",
        tie_disagreements.join("\n")
    );
    assert!(
        unexplained.is_empty(),
        "unexplained differences ({}):\n{}\nreproduce: {}",
        unexplained.len(),
        unexplained.join("\n"),
        reproducer(seed)
    );
    assert!(
        stale_entries.is_empty(),
        "explained.toml entries no case produced: {stale_entries:?}"
    );
    assert!(
        anchor_failures.is_empty(),
        "anchors did not produce what they are built for:\n{}",
        anchor_failures.join("\n")
    );

    // ── coverage ──
    for (key, _) in class_weights() {
        let n = asked.get(&key).copied().unwrap_or(0);
        assert!(n >= 20, "class {key} was asked {n} times, fewer than 20");
    }
    for bucket in ["allow", "deny", "masked", "filtered"] {
        let n = outcomes.get(bucket).copied().unwrap_or(0);
        assert!(n >= 100, "only {n} {bucket} cases, fewer than 100");
    }
    assert!(
        empty_selects * 20 >= select_ops,
        "{empty_selects} of {select_ops} SelectFromColumns questions have no column, under 5 %"
    );
    assert!(tie_states >= STATES / 10, "{tie_states} tie states");
    assert!(ties > 0, "no tie case was asked");
    assert!(
        reach_denied_above >= 20 && reach_denied_below >= 20,
        "reach questions with a deny or withhold above: {reach_denied_above}, below: \
         {reach_denied_below}; fewer than 20"
    );
}
