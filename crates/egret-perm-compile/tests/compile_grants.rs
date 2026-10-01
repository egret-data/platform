//! The compiler is the security boundary: its output is what an engine
//! enforces. These tests pin the things a reader cannot check by eye — that
//! the subtract lands in the right exception block for both effects, that an
//! action an engine cannot express is an ERROR rather than a silent drop,
//! that a `User` ref is emitted verbatim rather than guessed at (round 1's
//! `split('@')` canonicaliser could collide two distinct humans onto one
//! engine principal), that a principal kind this compiler can't yet expand
//! REFUSES rather than silently vanishing (round 1 dropped a `WorkspaceMembers`
//! exclude, which fails OPEN — the subtract evaporates and the compiled
//! policy grants more than the model says), and that the resource-level
//! names used for each engine are checked against that engine's own
//! service-def, not just Trino's.
//!
//! Pure (no DB) — `compile_grant` is a function of its arguments only.

use uuid::Uuid;

use egret_perm::{
    engine_accesses, Action, CompileError, Effect, EngineType, Grant, GrantOrigin, Principal,
    PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::{compile_grant, validate_grant_shape};

// Dumb helpers only — a helper that computes anything is a second
// implementation of the thing under test.

fn group(r#ref: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Group,
        r#ref: r#ref.to_string(),
    }
}

fn user(r#ref: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: r#ref.to_string(),
    }
}

fn no_scope() -> Scope {
    Scope {
        catalog_id: Uuid::nil(),
        schema: None,
        table: None,
        column: None,
    }
}

fn grant_allow_read(include_groups: &[&str], exclude_groups: &[&str]) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: no_scope(),
        principals: PrincipalSet {
            include: include_groups.iter().map(|g| group(g)).collect(),
            exclude: exclude_groups.iter().map(|g| group(g)).collect(),
        },
        origin: GrantOrigin::Explicit,
    }
}

fn grant_allow_read_on(scope: Scope) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope,
        principals: PrincipalSet {
            include: vec![user("admin")],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}

fn grant_allow_read_for_user(r#ref: &str) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: no_scope(),
        principals: PrincipalSet {
            include: vec![user(r#ref)],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}

/// Read one embedded service-def's `resources` array from disk. `cargo test`
/// runs this binary with CWD = this crate's own manifest directory, not the
/// workspace root, so the path is built from `CARGO_MANIFEST_DIR` rather
/// than the workspace-root-relative spelling used to read these files by
/// hand (brief Step 3) — same two files.
fn service_def(engine: EngineType) -> serde_json::Value {
    let path = match engine {
        EngineType::Trino => concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/trino.json"
        ),
        EngineType::StarRocks => concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/starrocks.json"
        ),
    };
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_principal_subtract_compiles_to_allow_exceptions() {
    let g = grant_allow_read(&["finance"], &["contractors"]);
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].groups, vec!["finance"]);
    assert_eq!(
        out.allow_exceptions.len(),
        1,
        "the exclude set must become an allowException — that block is a \
         compilation target and must never surface in egret's own API"
    );
    assert_eq!(out.allow_exceptions[0].groups, vec!["contractors"]);
    assert!(out.deny_exceptions.is_empty());
}

#[test]
fn a_deny_grants_subtract_compiles_to_deny_exceptions() {
    let mut g = grant_allow_read(&["finance"], &["contractors"]);
    g.effect = Effect::Deny;
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.items[0].effect, "deny");
    assert_eq!(out.deny_exceptions.len(), 1);
    assert!(out.allow_exceptions.is_empty());
}

#[test]
fn write_maps_to_two_accesses_on_trino_and_three_on_starrocks() {
    assert_eq!(
        engine_accesses(Action::Read, EngineType::Trino).unwrap(),
        vec!["select"]
    );
    assert_eq!(
        engine_accesses(Action::Read, EngineType::StarRocks).unwrap(),
        vec!["select"]
    );

    // Trino's service-def declares NO `update` access type. Asserting the same
    // three on both engines would be asserting a mapping that cannot exist —
    // this test exists because the plan's first draft did exactly that.
    let mut t = engine_accesses(Action::Write, EngineType::Trino).unwrap();
    t.sort();
    assert_eq!(t, vec!["delete", "insert"]);

    let mut s = engine_accesses(Action::Write, EngineType::StarRocks).unwrap();
    s.sort();
    assert_eq!(s, vec!["delete", "insert", "update"]);
}

#[test]
fn every_access_type_the_map_emits_is_declared_by_that_service_def() {
    // The map is only correct relative to the service-defs, so read them
    // rather than trusting the table above. An access type Ranger does not
    // know is silently ignored by the plugin: the policy compiles, deploys,
    // and grants nothing.
    for engine in [EngineType::Trino, EngineType::StarRocks] {
        let def = service_def(engine);
        let declared: Vec<String> = def["accessTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect();
        for action in [Action::Read, Action::Write] {
            for acc in engine_accesses(action, engine).unwrap() {
                assert!(
                    declared.iter().any(|d| d == acc),
                    "{engine:?} does not declare access type {acc:?}"
                );
            }
        }
    }
}

#[test]
fn the_scope_becomes_the_ranger_resource_levels_the_service_def_declares() {
    // Round 1's version of this test only ever compiled against Trino and
    // never opened a service-def, so a wrong StarRocks resource-level name
    // (e.g. hard-coding Trino's `schema` instead of StarRocks's `database`)
    // would have compiled, deployed, and granted nothing without this test
    // catching it — exactly the failure the test's own name promises to
    // catch. This version reads both service-defs' `resources` arrays and
    // checks the catalog/schema-or-database/table/column chain used by
    // `compile_grant` is a real root-to-leaf hierarchy declared by THAT
    // engine's own def.
    for (engine, expected) in [
        (EngineType::Trino, ["catalog", "schema", "table", "column"]),
        (
            EngineType::StarRocks,
            ["catalog", "database", "table", "column"],
        ),
    ] {
        let def = service_def(engine);
        let resources = def["resources"].as_array().unwrap();
        let parent_of = |name: &str| -> Option<String> {
            resources
                .iter()
                .find(|r| r["name"].as_str() == Some(name))
                .map(|r| r["parent"].as_str().unwrap_or("").to_string())
        };
        assert_eq!(
            parent_of(expected[0]).as_deref(),
            Some(""),
            "{engine:?}: {:?} is not declared as a root resource",
            expected[0]
        );
        for pair in expected.windows(2) {
            assert_eq!(
                parent_of(pair[1]).as_deref(),
                Some(pair[0]),
                "{engine:?}: {:?}'s declared parent is not {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    let g = grant_allow_read_on(Scope {
        catalog_id: Uuid::nil(),
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: None,
    });
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.resources["catalog"].values, vec!["warehouse"]);
    assert_eq!(out.resources["schema"].values, vec!["sales"]);
    assert_eq!(out.resources["table"].values, vec!["orders"]);
    assert_eq!(
        out.resources["column"].values,
        vec!["*"],
        "an unnamed level below the scope must be an explicit wildcard, NOT absent — \
         see `an_access_policy_fills_every_declared_level_below_the_scope`"
    );
}

/// **SP4a Task 11, defect 1 — the regression this file did not have.**
///
/// Round 1 left an unnamed resource level OUT of an access policy's map, on a
/// reading of Ranger's `RangerDefaultPolicyResourceMatcher.getMatchType()` that
/// the file itself flagged as unverified. Task 10 verified it against a real
/// Trino + Ranger plugin on kind and **it is false**: a grant of `read` on a
/// schema, compiled to `{catalog, schema}`, produced `Access Denied: Cannot
/// select from columns [v, id] in table or view sibling`, and a table-level
/// grant failed identically. Only a scope written down to `column: "*"`
/// enforced anything
/// (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, T1/T1b).
///
/// Every existing compile test asserted the OUTPUT SHAPE and passed over this:
/// the old body was well-formed Ranger, it deployed, egret listed it as live,
/// and it granted nothing. So this test does not ask "is the map the one the
/// code builds" — it asks **which levels the target engine's own service-def
/// declares for an access policy**, and then asserts the emitted map carries
/// exactly those, with `*` wherever the scope stopped.
#[test]
fn an_access_policy_fills_every_declared_level_below_the_scope() {
    for (engine, chain) in [
        (EngineType::Trino, ["catalog", "schema", "table", "column"]),
        (
            EngineType::StarRocks,
            ["catalog", "database", "table", "column"],
        ),
    ] {
        // 1. What the service-def actually requires, read from the file the
        //    binary embeds — Trino's is service-def id 203. The data
        //    hierarchy is the root-to-leaf chain ordered by the declared
        //    `level`, and every level on it is `mandatory: true`, which is
        //    what "an access policy names all four" means in Ranger's own
        //    vocabulary.
        let def = service_def(engine);
        let resources = def["resources"].as_array().unwrap();
        let by_name = |name: &str| -> serde_json::Value {
            resources
                .iter()
                .find(|r| r["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("{engine:?} declares no resource named {name:?}"))
                .clone()
        };
        let mut declared_levels: Vec<i64> = Vec::new();
        for (i, name) in chain.iter().enumerate() {
            let r = by_name(name);
            assert_eq!(
                r["mandatory"].as_bool(),
                Some(true),
                "{engine:?}: {name:?} is not declared mandatory, so filling it with a \
                 wildcard would be asserting a level the def does not require"
            );
            assert_eq!(
                r["parent"].as_str().unwrap_or(""),
                if i == 0 { "" } else { chain[i - 1] },
                "{engine:?}: {name:?} is not the child of {:?}",
                if i == 0 { "<root>" } else { chain[i - 1] }
            );
            declared_levels.push(r["level"].as_i64().unwrap());
        }
        let mut ascending = declared_levels.clone();
        ascending.sort_unstable();
        assert_eq!(
            declared_levels, ascending,
            "{engine:?}: the chain {chain:?} is not in the def's own declared level order"
        );

        // 2. What the compiler emits, at each depth a scope can stop at.
        let cases: [(Scope, [&str; 4]); 3] = [
            (
                Scope {
                    catalog_id: Uuid::nil(),
                    schema: None,
                    table: None,
                    column: None,
                },
                ["warehouse", "*", "*", "*"],
            ),
            (
                Scope {
                    catalog_id: Uuid::nil(),
                    schema: Some("sales".into()),
                    table: None,
                    column: None,
                },
                ["warehouse", "sales", "*", "*"],
            ),
            (
                Scope {
                    catalog_id: Uuid::nil(),
                    schema: Some("sales".into()),
                    table: Some("orders".into()),
                    column: None,
                },
                ["warehouse", "sales", "orders", "*"],
            ),
        ];
        for (scope, expected) in cases {
            let g = grant_allow_read_on(scope);
            let out = compile_grant(&g, "warehouse", "svc-1", engine).unwrap();
            let emitted: Vec<&str> = out.resources.keys().map(String::as_str).collect();
            let mut want: Vec<&str> = chain.to_vec();
            want.sort_unstable(); // the map is a BTreeMap, so keys come back sorted
            assert_eq!(
                emitted, want,
                "{engine:?}: an access policy must carry every declared level, no more and \
                 no fewer"
            );
            for (i, name) in chain.iter().enumerate() {
                assert_eq!(
                    out.resources[*name].values,
                    vec![expected[i]],
                    "{engine:?}: level {name:?}"
                );
            }
        }
    }
}

/// The other half of defect 1's fix, stated as its own assertion: a MASK and a
/// ROW-FILTER policy are NOT widened. Task 10 measured both working exactly as
/// emitted (T3b masked a column for real; T4 removed rows for real), and the
/// design spec's reading of Ranger says masking/row-filter policies take no
/// wildcards at all. Widening them would swap one unverified assumption for
/// another in the fail-OPEN direction. This test is what would fail if a later
/// reader "made it consistent" without a live measurement to justify it.
#[test]
fn a_path_selected_rule_is_not_widened_the_way_an_access_policy_is() {
    use egret_perm::{Rule, RuleKind, Selector};
    use egret_perm_compile::{compile_rule, ResolvedNames};

    let scope = Scope {
        catalog_id: Uuid::nil(),
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: None,
    };
    for kind in [
        RuleKind::Mask {
            mask_type: "MASK".to_string(),
            value_expr: None,
        },
        RuleKind::RowFilter {
            filter_expr: "country = 'US'".to_string(),
        },
    ] {
        let r = Rule {
            id: Uuid::nil(),
            name: "r".to_string(),
            kind,
            selector: Selector::Path(scope.clone()),
            exempt: PrincipalSet::default(),
        };
        let bodies = compile_rule(
            &r,
            EngineType::Trino,
            "trino-1",
            None,
            &ResolvedNames {
                catalog_name: "warehouse",
                tag_name: None,
            },
            &[],
            500,
        )
        .unwrap();
        assert_eq!(bodies.len(), 1);
        let resources = &bodies[0]["resources"];
        assert_eq!(resources["table"]["values"][0], "orders");
        assert!(
            resources.get("column").is_none(),
            "a masking/row-filter policy keeps today's measured shape: an unnamed level is \
             absent, not a wildcard"
        );
    }
}

#[test]
fn a_starrocks_grant_scopes_by_database_not_schema() {
    // No compile test targeted StarRocks in round 1 at all. StarRocks's
    // service-def names the schema-level resource `database`; emitting
    // Trino's `schema` there would compile, deploy, and grant nothing.
    let g = grant_allow_read_on(Scope {
        catalog_id: Uuid::nil(),
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: None,
    });
    let out = compile_grant(&g, "warehouse", "sr-1", EngineType::StarRocks).unwrap();
    assert_eq!(out.resources["catalog"].values, vec!["warehouse"]);
    assert_eq!(
        out.resources["database"].values,
        vec!["sales"],
        "StarRocks's service-def calls the schema-level resource `database`, not `schema`"
    );
    assert!(
        !out.resources.contains_key("schema"),
        "must not emit Trino's resource-level name on a StarRocks policy"
    );
    assert_eq!(out.resources["table"].values, vec!["orders"]);
}

/// **SP4d debt Task 12 — the privilege escalation, measured end to end on the
/// compiler's OWN artifact.**
///
/// The finding: `perm.grants` accepted `schema_name = ''` (the control pair is in
/// `egret-perm/tests/store.rs`), `scope_to_resources` copies a scope level
/// through verbatim, `RangerAbstractResourceMatcher.init()` DROPS an empty policy
/// value before building a matcher for it, and a level with no matchers is
/// `isMatchAny`. So a grant written for `sales.orders` was enforced on
/// `orders` in EVERY schema of the catalog.
///
/// **The bad resource map is DERIVED from a real compiler artifact, not typed out
/// here.** The test compiles the legal grant, then changes the ONE token the
/// finding is about — the `schema` level's value — and re-measures. A hand-written
/// map would be a second copy of `scope_to_resources`' output, free to drift from
/// it and still green.
///
/// **The control is the unedited artifact**, asserted to match the foreign schema
/// at `MatchType::None`. Without it, a `Self_` on the edited map could just as
/// well mean the matcher ignores policy resources altogether — the
/// "a zero is unfalsifiable alone" shape in its positive form.
///
/// **The third arm WAS a standing residual and SP4d debt Task 14 closed it.**
/// `'*'` at a level is a legitimate policy value — a steward scopes a grant to a
/// whole catalog or schema routinely, and `scope_to_resources` emits `*` itself
/// for every level the scope does not name — so neither `0167` nor `0168` refuses
/// it as such. What `0168` refuses is `*` with a NAMED level BENEATH it, because
/// `grants_check` has forbidden naming a table without a schema since `0154` and
/// `(schema = '*', table = 'orders')` expresses exactly that. The arm stays here
/// unchanged as the MEASUREMENT (`''` and `'*'` earn the identical `Self_`), and
/// the refusal loop below now covers both tokens — with the legal shapes that
/// keep `*` authorable living in
/// `a_wildcard_level_over_a_named_one_is_refused_and_a_wildcard_tail_is_not`.
#[test]
fn an_empty_scope_level_would_be_enforced_in_every_schema() {
    use std::collections::BTreeMap;

    use egret_policy::admin::access_to_ranger_json;
    use egret_ranger_eval::model::ResourceLevel;
    use egret_ranger_eval::resource::{match_resource, MatchType, RequestResource};
    use egret_ranger_eval::servicedef::ServiceDef;

    // The real Trino service-def, so `wildCard`/`ignoreCase` on each level come
    // from the file the engine is handed rather than from a default.
    let def = ServiceDef::parse(&service_def(EngineType::Trino));

    // One request, in a schema the grant below never named.
    let request = RequestResource::new()
        .with("catalog", "warehouse")
        .with("schema", "finance")
        .with("table", "orders")
        .with("column", "amount");

    let legal = grant_allow_read_on(Scope {
        catalog_id: Uuid::nil(),
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: None,
    });
    let artifact = compile_grant(&legal, "warehouse", "trino-1", EngineType::Trino)
        .expect("a fully named path is the ordinary grant");
    let wire = access_to_ranger_json(&artifact);

    // Re-read the compiler's own output through the evaluator's own reader.
    let read = |v: &serde_json::Value| -> BTreeMap<String, ResourceLevel> {
        serde_json::from_value(v["resources"].clone()).expect("the artifact's resources map")
    };

    assert_eq!(
        match_resource(&read(&wire), &request, &def),
        MatchType::None,
        "CONTROL: the unedited artifact for sales.orders must not cover \
         finance.orders.amount — if it does, nothing below means anything"
    );

    // The ONE token the finding is about.
    for value in ["", "*"] {
        let mut edited = wire.clone();
        edited["resources"]["schema"]["values"] = serde_json::json!([value]);
        assert_eq!(
            match_resource(&read(&edited), &request, &def),
            MatchType::Self_,
            "a {value:?} at the schema level makes the level isMatchAny, so the grant \
             written for sales.orders covers finance.orders.amount too"
        );
    }

    // And the compiler now refuses to emit EITHER of those two, each under the
    // variant that names what is wrong with it: `''` is not a resource name at
    // all (Task 12), while `'*'` is a legal value with a named level under it
    // (Task 14). Two tokens, identical measured enforcement, two different
    // refusals — which is why they are two predicates and not one.
    for (value, expected) in [
        ("", CompileError::EmptyScopeLevel { level: "schema" }),
        (
            "*",
            CompileError::NamedLevelUnderWildcard {
                wildcard: "schema",
                wildcard_value: "*".into(),
                named: "table",
                named_value: "orders".into(),
            },
        ),
    ] {
        let err = compile_grant(
            &grant_allow_read_on(Scope {
                catalog_id: Uuid::nil(),
                schema: Some(value.to_string()),
                table: Some("orders".into()),
                column: None,
            }),
            "warehouse",
            "trino-1",
            EngineType::Trino,
        )
        .unwrap_err();
        assert_eq!(
            err, expected,
            "{value:?} at the schema level with `orders` named under it must be refused,              and by the variant that names WHY"
        );
    }
}

/// Every level the guard covers, in both positions it runs in, and the shapes it
/// must NOT refuse.
///
/// `validate_grant_shape` is the authoring boundary (create, patch and the
/// rolled-back `/perm/preview` all reach it) and `compile_grant` is the last point
/// at which a wrong artifact becomes an enforced one, so both are asserted for
/// every level rather than one standing in for the other.
#[test]
fn every_scope_level_is_refused_empty_and_nothing_else_is() {
    let scope = |schema: Option<&str>, table: Option<&str>, column: Option<&str>| Scope {
        catalog_id: Uuid::nil(),
        schema: schema.map(str::to_string),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    };

    for (level, s) in [
        ("schema", scope(Some(""), None, None)),
        ("schema", scope(Some(""), Some("orders"), None)),
        ("table", scope(Some("sales"), Some(""), None)),
        ("table", scope(Some("sales"), Some(""), Some("email"))),
        ("column", scope(Some("sales"), Some("orders"), Some(""))),
    ] {
        let g = grant_allow_read_on(s);
        assert_eq!(
            validate_grant_shape(&g).unwrap_err(),
            CompileError::EmptyScopeLevel { level },
            "the authoring boundary must name the empty level"
        );
        assert_eq!(
            compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap_err(),
            CompileError::EmptyScopeLevel { level },
            "and so must the compiler, which is `pub` to callers that never went \
             near the store"
        );
    }

    // The shapes that must stay legal. `*` is a legitimate policy value (see
    // `an_empty_scope_level_would_be_enforced_in_every_schema`'s third arm and
    // migration 0167); whitespace is a literal resource name, because `init()`
    // drops `""` and nothing else; and an ABSENT level is the ordinary
    // catalog-or-schema-wide scope that `scope_to_resources` renders as `*`.
    //
    // ⚠ `('*', 'orders')` was in this list when Task 12 wrote it and has MOVED to
    // the refused set of `a_wildcard_level_over_a_named_one_is_refused_and_a_
    // wildcard_tail_is_not` (SP4d debt Task 14) — not because `*` stopped being
    // legitimate, but because a NAMED level under a match-any one is the level
    // skip `grants_check` already forbids. `('*', None)` keeps this test's
    // original point: `0167`'s predicate must not refuse `*`.
    for s in [
        scope(None, None, None),
        scope(Some("sales"), None, None),
        scope(Some("*"), None, None),
        scope(Some("sales"), Some("*"), Some("*")),
        scope(Some(" "), Some("orders"), None),
        scope(Some("Sales"), Some("Orders"), Some("Email")),
    ] {
        let g = grant_allow_read_on(s.clone());
        validate_grant_shape(&g).unwrap_or_else(|e| panic!("{s:?} must stay authorable: {e}"));
        compile_grant(&g, "warehouse", "trino-1", EngineType::Trino)
            .unwrap_or_else(|e| panic!("{s:?} must stay compilable: {e}"));
    }
}

/// **SP4d debt Task 14 — the rule, and above all the shapes it must NOT refuse.**
///
/// The rule is the authored-value form of `scope_to_resources`' own
/// `wildcard_from_here` latch: *a level may be a match-any wildcard only if every
/// level below it is a wildcard or absent.* It is NOT "`*` is illegitimate" — the
/// page asks catalog-wide and schema-wide questions routinely and the compiler
/// emits `*` itself for every unnamed level — so **the accept half of this test
/// carries as much weight as the refuse half**, and a predicate that refused any
/// of those shapes would be the over-refusal the plan warned against.
///
/// Both Rust positions are asserted for every shape, never one standing in for the
/// other: `validate_grant_shape` is the authoring boundary (create, patch and the
/// rolled-back `/perm/preview` all reach it) and `compile_grant` is the last point
/// at which a wrong artifact becomes an enforced one and is `pub` to callers that
/// never went near the store.
///
/// **A RUN of stars, not the single character.** `'**'` classifies to the same
/// match-any as `'*'` in Ranger's `buildResourceMatchers()` and in
/// `egret_ranger_eval::resource::ValueMatcher::classify`, so it is in the refused
/// set — keying on the literal `'*'` would close one spelling and leave the next
/// reaching identical enforcement from the identical free-text field. `'ord*'` is
/// in the ACCEPTED set for the opposite reason: it classifies to `StartsWith`,
/// which names a SET of schemas, so a table under it is not a gap.
#[test]
fn a_wildcard_level_over_a_named_one_is_refused_and_a_wildcard_tail_is_not() {
    let scope = |schema: Option<&str>, table: Option<&str>, column: Option<&str>| Scope {
        catalog_id: Uuid::nil(),
        schema: schema.map(str::to_string),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    };

    for (wildcard, wildcard_value, named, named_value, s) in [
        // The escalation as a steward can author it in the browser today.
        (
            "schema",
            "*",
            "table",
            "orders",
            scope(Some("*"), Some("orders"), None),
        ),
        // The same match-any, the next spelling.
        (
            "schema",
            "**",
            "table",
            "orders",
            scope(Some("**"), Some("orders"), None),
        ),
        // The topmost offending pair is the one reported, not the deepest.
        (
            "schema",
            "*",
            "table",
            "orders",
            scope(Some("*"), Some("orders"), Some("amount")),
        ),
        // A gap that only appears below a legal wildcard tail.
        (
            "schema",
            "*",
            "column",
            "amount",
            scope(Some("*"), Some("*"), Some("amount")),
        ),
        (
            "table",
            "*",
            "column",
            "amount",
            scope(Some("sales"), Some("*"), Some("amount")),
        ),
    ] {
        let expected = CompileError::NamedLevelUnderWildcard {
            wildcard,
            wildcard_value: wildcard_value.to_string(),
            named,
            named_value: named_value.to_string(),
        };
        let g = grant_allow_read_on(s.clone());
        assert_eq!(
            validate_grant_shape(&g).unwrap_err(),
            expected,
            "{s:?}: the authoring boundary must refuse it, naming BOTH levels"
        );
        assert_eq!(
            compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap_err(),
            expected,
            "{s:?}: and so must the compiler, which is `pub` to callers that never \
             went near the store"
        );

        // The message is what a steward reads, and the whole point of the framing
        // is that the refusal reads as CONSISTENCY rather than as a new
        // restriction: it must point at the decision the model already made.
        let msg = expected.to_string();
        assert!(
            msg.contains("0154_perm_model.sql") && msg.contains("does not express"),
            "the refusal must say the model does not express this scope, by \
             `grants_check`'s existing decision rather than by a new opinion — got: {msg}"
        );
        assert!(
            msg.contains(named_value) && msg.contains(wildcard_value),
            "and it must name both offending values so the author can fix one — got: {msg}"
        );
    }

    // ── THE SHAPES THAT MUST STAY AUTHORABLE ─────────────────────────────────
    // Every one of these is an ordinary thing a steward writes, and the plan is
    // explicit that an implementation refusing any of them is wrong.
    for s in [
        scope(None, None, None),                         // catalog-wide
        scope(Some("sales"), None, None),                // schema-wide
        scope(Some("*"), None, None),                    // every schema, nothing under it
        scope(Some("*"), Some("*"), None),               // ('*','*')
        scope(Some("*"), Some("*"), Some("*")),          // the compiler's own emission
        scope(Some("**"), Some("***"), None),            // runs of any length, still no gap
        scope(Some("sales"), Some("orders"), Some("*")), // every column of one table
        scope(Some("sales"), Some("*"), Some("*")),
        scope(Some("ord*"), Some("orders"), None), // StartsWith names a SET
        scope(Some("sales"), Some("ord*"), Some("amount")),
        scope(Some(" "), Some("orders"), None), // whitespace is a literal name
        scope(Some("sales"), Some("orders"), Some("amount")),
    ] {
        let g = grant_allow_read_on(s.clone());
        validate_grant_shape(&g).unwrap_or_else(|e| panic!("{s:?} must stay authorable: {e}"));
        compile_grant(&g, "warehouse", "trino-1", EngineType::Trino)
            .unwrap_or_else(|e| panic!("{s:?} must stay compilable: {e}"));
    }

    // And the latch this predicate mirrors is still the emission's own: the
    // compiler renders `*` below an ABSENT level by itself, so the accepted
    // `('sales', None, None)` and the refused `('sales','*','amount')` are not two
    // readings of one scope — the first HAS no column to name.
    let wide = compile_grant(
        &grant_allow_read_on(scope(Some("sales"), None, None)),
        "warehouse",
        "trino-1",
        EngineType::Trino,
    )
    .expect("a schema-wide grant compiles");
    assert_eq!(wide.resources["table"].values, vec!["*"]);
    assert_eq!(wide.resources["column"].values, vec!["*"]);
}

#[test]
fn a_user_principal_ref_is_emitted_verbatim() {
    // The compiler does no I/O and cannot resolve `users.username` itself,
    // so it must not rewrite a `User` ref at all — round 1's `split('@')`
    // "canonicaliser" could compile two distinct humans who slug to the
    // same local part (`bob`, `bob-2`) onto the SAME engine principal. A
    // ref that is already the bare username the store is supposed to hold
    // passes through byte-for-byte, including one with a collision suffix.
    let g = grant_allow_read_for_user("bob-2");
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.items[0].users, vec!["bob-2"]);
}

#[test]
fn a_user_ref_containing_at_is_refused_not_guessed() {
    // The literal below is the same D7-measured shape ("egret writes both
    // forms today") that round 1 mishandled by guessing a username from it.
    // The right answer for a compiler with no I/O is to refuse, not guess.
    let g = grant_allow_read_for_user("admin@egret.local");
    let err = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap_err();
    assert_eq!(
        err,
        CompileError::UnusableUserRef {
            r#ref: "admin@egret.local".to_string()
        }
    );
}

#[test]
fn an_empty_user_ref_is_refused() {
    // The live cluster has users with an empty stored username today
    // (`sp3c-reader@egret.local` and four `e2e-*` users) — an empty ref
    // reaching the compiler must not silently compile to an empty-user
    // item either.
    let g = grant_allow_read_for_user("");
    let err = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap_err();
    assert_eq!(
        err,
        CompileError::UnusableUserRef {
            r#ref: String::new()
        }
    );
}

#[test]
fn a_workspace_members_include_is_refused_not_silently_empty() {
    let mut g = grant_allow_read_for_user("bob");
    g.principals.include = vec![Principal {
        kind: PrincipalKind::WorkspaceMembers,
        r#ref: Uuid::nil().to_string(),
    }];
    let err = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap_err();
    assert_eq!(
        err,
        CompileError::UnexpandedPrincipalKind {
            kind: PrincipalKind::WorkspaceMembers
        }
    );
}

#[test]
fn a_workspace_members_exclude_is_refused_rather_than_silently_failing_open() {
    // The bug round 1 shipped: an exclude set consisting only of a
    // WorkspaceMembers principal was silently dropped, producing a
    // non-empty allowExceptions list containing one EMPTY item — Ranger
    // matches an empty item against nobody, so the subtract evaporates and
    // the compiled policy grants strictly more than the model states.
    let mut g = grant_allow_read(&["finance"], &[]);
    g.principals.exclude = vec![Principal {
        kind: PrincipalKind::WorkspaceMembers,
        r#ref: Uuid::nil().to_string(),
    }];
    let err = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap_err();
    assert_eq!(
        err,
        CompileError::UnexpandedPrincipalKind {
            kind: PrincipalKind::WorkspaceMembers
        }
    );
}

/// **`policyPriority` MUST be 0 on every RESOURCE policy the grant compiler
/// emits, and until SP4d-2b Task 5 fix round 2 nothing asserted it.**
///
/// This is the second half of the invariant T4's and T7's cost claims rest on.
/// Round 1 asserted it for TAG bodies (`tag_service_routing.rs`) and the clearance
/// path already had it (`clearance_starrocks.rs`); the grants path had nothing at
/// all between `compile_grant`'s hard-coded `is_override: false`
/// (`grants.rs`) and the emitted wire value.
///
/// Why it matters here rather than only on the tag side: the differential gate
/// measured that a tag verdict plus RESOURCE policies at two priorities is a shape
/// this crate answers wrongly (`tests/ranger/README.md`, finding T7), and the
/// corpus holds it out on the strength of "egret never emits a non-zero priority".
/// `AccessPolicyInput.is_override` is a PUBLIC wire field (`isOverride`), so one
/// flag flip makes that shape egret's own data.
///
/// Asserted on the EMITTED `policyPriority` rather than on the flag, so it survives
/// a refactor that renames or reroutes `is_override`, and on both engines because
/// the two take different `scope_to_resources` arms.
#[test]
fn a_compiled_grant_always_emits_policy_priority_zero() {
    for engine in [EngineType::Trino, EngineType::StarRocks] {
        for effect in [Effect::Allow, Effect::Deny] {
            let mut g = grant_allow_read(&["finance"], &["contractors"]);
            g.effect = effect;
            let input = compile_grant(&g, "warehouse", "svc-1", engine).unwrap();
            assert!(
                !input.is_override,
                "{engine:?}/{effect:?}: the compiler must not request an override"
            );
            let body = egret_policy::admin::access_to_ranger_json(&input);
            assert_eq!(
                body["policyPriority"], 0,
                "{engine:?}/{effect:?}: a non-zero priority on a compiled grant makes \
                 egret's own downloads reach the shape the differential corpus holds out \
                 (tests/ranger/README.md, T7), and the zero-cost claim behind that \
                 hold-out is exactly this value"
            );
        }
    }
}
