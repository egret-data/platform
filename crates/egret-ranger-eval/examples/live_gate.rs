//! The live gate's prediction tool (SP4d-1 Task 8).
//!
//! Reads a `ServicePolicies` download exactly as a plugin polls it —
//! `GET /service/plugins/policies/download/<service>` — and prints the
//! evaluator's decision for each request the live gate is about to issue.
//! The point is the ORDER: this runs, and its output is pasted into
//! `docs/en/plans/2026-09-28-sp4d-1-live-result.md`, BEFORE the query runs.
//! A prediction written after the measurement proves nothing.
//!
//! ```text
//! cargo run -p egret-ranger-eval --example live_gate -- \
//!     <download.json> <trino|starrocks> <user> [catalog] [schema/table[/column] ...]
//! ```
//!
//! With no `schema/table` arguments it runs the fixed Trino case list below.
//! With them it asks all three questions (access, mask, row filter) about each
//! resource named, which is how the StarRocks cross-check is driven.
//!
//! # SP4d-2b Task 8: the resolved tag set
//!
//! A spec may carry a `#` suffix naming the tags egret resolved for that
//! resource — `sp4d1/t4/item#pii:ancestor` — and then the request is built with
//! `resolved_tags: Some(...)` instead of `None`. Each entry is built exactly as
//! `egret_perm_compile::routes::attachment_for` builds it (empty `attributes`,
//! empty `options`, zero `validityPeriods`, `is_enabled: true`, and the levels
//! `from_assignments` writes: `catalog` always, `schema`/`table` when non-empty,
//! `column` only for a `self` match) — ⚠ **a second copy of that mapping, which
//! is the shape this package normally refuses.** What keeps it honest is that
//! the ACCESS arm of every case is cross-checked against the live `explain`
//! endpoint, which builds the set through the real `attachment_for` from the real
//! `catalog_tag_assignments`: a divergence in the copy would show up there before
//! it could corrupt the mask and row-filter arms, which `explain` does not report.
//!
//! `#` with no tags (`sp4d1/t7/item#`) is the EMPTY resolved set — `Some(vec![])`,
//! the measured narrowing — which is not the same request as no `#` at all.
//!
//! The service-def is taken from the download's own `serviceDef`, because
//! that is the def the plugin builds its matchers from — not a def read from
//! anywhere else.

use std::collections::{BTreeMap, BTreeSet};

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, EngineConfig, EngineType,
    PluginVersion, ResolvedTag, TagServiceResource, UnenrichedTags,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::{MatchType, RequestResource};
use egret_ranger_eval::servicedef::ServiceDef;

/// One case: a label, the resource levels below the catalog, and the column
/// (`None` issues the request at TABLE granularity — which is what a
/// row-filter check must do, since `rowFilterDef.resources` names no
/// `column` on either engine).
struct Case {
    label: String,
    schema: String,
    table: String,
    column: Option<String>,
    /// Also ask the mask and row-filter questions for this case.
    ask_mask: bool,
    ask_filter: bool,
    /// `None` ⇒ `resolved_tags: None` (the refusal). `Some` ⇒ the set egret
    /// resolved, possibly empty.
    tags: Option<Vec<ResolvedTag>>,
}

/// `pii:self` / `tblwide:ancestor` → one [`ResolvedTag`], built the way
/// `attachment_for` builds it. Unknown match types PANIC rather than defaulting,
/// for the same reason the engine name does.
fn parse_tag(spec: &str, schema: &str, table: &str, column: &str) -> ResolvedTag {
    let (name, kind) = spec
        .split_once(':')
        .unwrap_or_else(|| panic!("tag spec must be `<name>:<self|ancestor>`, got {spec:?}"));
    let match_type = match kind {
        "self" => MatchType::Self_,
        "ancestor" => MatchType::Ancestor,
        other => panic!(
            "match type must be `self` or `ancestor`, got {other:?}; \
             refusing to default — the match type IS the mask order"
        ),
    };
    // `attachment_for`'s rule, verbatim: an ANCESTOR attachment carries no column.
    let attached_column = match match_type {
        MatchType::Self_ => column,
        _ => "",
    };
    let levels: BTreeSet<String> = std::iter::once("catalog")
        .chain((!schema.is_empty()).then_some("schema"))
        .chain((!table.is_empty()).then_some("table"))
        .chain((!attached_column.is_empty()).then_some("column"))
        .map(str::to_owned)
        .collect();
    ResolvedTag {
        name: name.to_string(),
        match_type,
        attributes: BTreeMap::new(),
        options: BTreeSet::new(),
        validity_periods: 0,
        attached_through: TagServiceResource {
            levels,
            is_enabled: true,
        },
    }
}

/// `schema/table` or `schema/table/column` → one case that asks all three
/// questions. Used for the ad-hoc (StarRocks) cross-check.
fn parse_spec(spec: &str) -> Case {
    let (resource, tagspec) = match spec.split_once('#') {
        Some((r, t)) => (r, Some(t)),
        None => (spec, None),
    };
    let parts: Vec<&str> = resource.split('/').collect();
    let schema = parts.first().copied().unwrap_or_default().to_string();
    let table = parts.get(1).copied().unwrap_or_default().to_string();
    let column = parts.get(2).map(|c| c.to_string());
    let tags = tagspec.map(|t| {
        t.split(',')
            .filter(|s| !s.is_empty())
            .map(|s| parse_tag(s, &schema, &table, column.as_deref().unwrap_or("")))
            .collect()
    });
    Case {
        label: spec.to_string(),
        schema,
        table,
        column,
        ask_mask: true,
        ask_filter: true,
        tags,
    }
}

struct StaticCase {
    label: &'static str,
    schema: &'static str,
    table: &'static str,
    column: Option<&'static str>,
    ask_mask: bool,
    ask_filter: bool,
}

const CASES: &[StaticCase] = &[
    StaticCase {
        label: "1  ANCESTOR  {catalog,schema} policy vs column-granular select",
        schema: "sp4d1a",
        table: "t1",
        column: Some("item"),
        ask_mask: false,
        ask_filter: false,
    },
    StaticCase {
        label: "2  column wildcard",
        schema: "sp4d1",
        table: "t2",
        column: Some("item"),
        ask_mask: false,
        ask_filter: false,
    },
    StaticCase {
        label: "3  mask on column `item`",
        schema: "sp4d1",
        table: "t3",
        column: Some("item"),
        ask_mask: true,
        ask_filter: false,
    },
    StaticCase {
        label: "4  row filter (TABLE granularity)",
        schema: "sp4d1",
        table: "t4",
        column: None,
        ask_mask: false,
        ask_filter: true,
    },
    StaticCase {
        label: "5a deny cancelled by a denyException",
        schema: "sp4d1",
        table: "t5a",
        column: Some("item"),
        ask_mask: false,
        ask_filter: false,
    },
    StaticCase {
        label: "5b allow cancelled by an allowException",
        schema: "sp4d1",
        table: "t5b",
        column: Some("item"),
        ask_mask: false,
        ask_filter: false,
    },
    StaticCase {
        label: "6  grant to group `public`, requester in no groups",
        schema: "sp4d1",
        table: "t6",
        column: Some("item"),
        ask_mask: false,
        ask_filter: false,
    },
    StaticCase {
        label: "8  no policy names this table at all (fallback probe)",
        schema: "sp4d1",
        table: "t7",
        column: Some("item"),
        ask_mask: false,
        ask_filter: false,
    },
];

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: live_gate <download.json> <engine> <user> [catalog]");
    // Unknown engine names PANIC rather than defaulting. A typo'd `starocks`
    // would silently predict with Trino's service-def and Trino's middle
    // resource level (`schema`, not `database`), i.e. confidently answer
    // "nothing matched" about the wrong engine — which is the exact failure
    // this tool exists to prevent. The printed header is a mitigation, not a
    // guard: it is read after the run, if at all.
    let engine = match args.next().as_deref() {
        Some("trino") => EngineType::Trino,
        Some("starrocks") => EngineType::StarRocks,
        other => panic!(
            "engine must be `trino` or `starrocks`, got {other:?}; \
             refusing to default, because predicting for the wrong engine \
             uses the wrong service-def and the wrong resource hierarchy"
        ),
    };
    // The plugin version of the engine under test. `LIVE_GATE_PLUGIN_VERSION`
    // names it; unset means the engine type's first modelled version. A version
    // the evaluator does not model is not defaulted around: the run then
    // reports `PluginVersionMismatch`, which is the point.
    let plugin_version = match std::env::var("LIVE_GATE_PLUGIN_VERSION").as_deref() {
        Ok("2.5.0") => PluginVersion::Ranger2_5_0,
        Ok("2.8.0") => PluginVersion::Ranger2_8_0,
        Ok(other) => panic!("LIVE_GATE_PLUGIN_VERSION must be 2.5.0 or 2.8.0, got {other:?}"),
        Err(_) => engine.modelled_versions()[0],
    };
    let user = args.next().unwrap_or_else(|| "admin".to_string());
    let catalog = args.next().unwrap_or_else(|| "sp4a10_wh".to_string());
    let specs: Vec<String> = args.collect();
    let cases: Vec<Case> = if specs.is_empty() {
        CASES
            .iter()
            .map(|c| Case {
                label: c.label.to_string(),
                schema: c.schema.to_string(),
                table: c.table.to_string(),
                column: c.column.map(str::to_string),
                ask_mask: c.ask_mask,
                ask_filter: c.ask_filter,
                tags: None,
            })
            .collect()
    } else {
        specs.iter().map(|s| parse_spec(s)).collect()
    };

    let raw = std::fs::read_to_string(&path).expect("read download");
    let policies: ServicePolicySet = serde_json::from_str(&raw).expect("parse download");
    let def = ServiceDef::parse(&policies.service_def);
    let cfg = EngineConfig {
        engine: Some(engine),
        plugin_version: Some(plugin_version),
        ..EngineConfig::without_super_users()
    };
    // Trino's middle level is `schema`; StarRocks' is `database`.
    let mid = match engine {
        EngineType::Trino => "schema",
        EngineType::StarRocks => "database",
    };

    println!("download   : {path}");
    println!("service    : {}", policies.service_name);
    println!("policyVer  : {:?}", policies.policy_version);
    println!("engine     : {engine:?} / {plugin_version:?}");
    println!("user       : {user}  (groups {{}}, roles {{}})");
    println!("unmodelled : {:?}", policies.all_unmodelled());
    println!("defIssues  : {:?}", def.issues());
    println!();

    for case in &cases {
        let mut resource = RequestResource::new()
            .with("catalog", catalog.clone())
            .with(mid, case.schema.clone())
            .with("table", case.table.clone());
        if let Some(col) = &case.column {
            resource.insert("column", col.clone());
        }
        let req = AccessRequest {
            user: user.clone(),
            groups: BTreeSet::new(),
            roles: BTreeSet::new(),
            access_type: "select".to_string(),
            resource,
            owner: None,
            resolved_tags: case.tags.clone().map(UnenrichedTags::new),
        };
        println!("── case {}", case.label);
        println!(
            "   tags     {}",
            match &case.tags {
                None => "None (the caller did not resolve)".to_string(),
                Some(t) if t.is_empty() => "Some([]) — the measured narrowing".to_string(),
                Some(t) => t
                    .iter()
                    .map(|x| format!(
                        "{}:{:?}{:?}",
                        x.name, x.match_type, x.attached_through.levels
                    ))
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        );
        println!("   request  {:?}", req.resource);
        println!("   access   {:?}", evaluate(&policies, &req, &def, &cfg));
        if case.ask_mask {
            println!(
                "   mask     {:?}",
                evaluate_data_mask(&policies, &req, &def, &cfg)
            );
        }
        if case.ask_filter {
            println!(
                "   filter   {:?}",
                evaluate_row_filter(&policies, &req, &def, &cfg)
            );
        }
        println!();
    }
}
