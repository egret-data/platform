//! The renderer's safety properties, as assertions.
//!
//! Every test here was observed failing before it was believed. The three
//! mutations the plan asks for are named on the tests that die under them:
//! `set_formatter` made conditional, `{% filter %}` allowed through, and `ident`
//! emitting the caller's string.

use egret_dataapi::render::{check_static, render, ParamValues, RenderError};
use egret_query_api::Param;

/// The hostile value used throughout: it is a complete predicate break-out if it
/// ever reaches SQL text, and inert as a bound value.
const EVIL: &str = "EVIL' OR 1=1--";

fn ctx_with_a(v: &str) -> ParamValues {
    let mut ctx = ParamValues::new();
    ctx.insert("a", Param::Str(v.to_string()));
    ctx
}

/// **The whole task in one assertion**, and the test that dies if
/// `set_formatter` is ever put behind a condition: `Instruction::Emit`'s fast
/// path calls the default escape function and skips the formatter, so a
/// conditional `set_formatter` leaves `SELECT EVIL' OR 1=1--` in the SQL with
/// nothing bound.
#[test]
fn every_value_emission_becomes_a_placeholder_and_no_construct_bypasses_it() {
    for (tpl, want_sql, want_binds) in [
        ("SELECT {{ a }}", "SELECT ?", 1),
        // `safe` marks a value; it does not bypass the formatter.
        ("SELECT {{ a|safe }}", "SELECT ?", 1),
        ("SELECT {{ a|escape }}", "SELECT ?", 1),
        // The whole concatenation is ONE value, so it is one bind.
        ("SELECT {{ 'pre' ~ a ~ 'post' }}", "SELECT ?", 1),
        // `{% raw %}` is literal template text: it is `EmitRaw`, a borrowed slice
        // of the source, and no runtime value can reach it.
        (
            "SELECT {% raw %}{{ a }}{% endraw %}",
            "SELECT {{ a }}",
            0usize,
        ),
    ] {
        let (sql, binds) = render(tpl, &ctx_with_a(EVIL)).unwrap();
        assert_eq!(sql, want_sql, "template: {tpl}");
        assert_eq!(binds.len(), want_binds, "template: {tpl}");
        assert!(!sql.contains("EVIL"), "no caller text may reach the SQL");
    }
}

/// The bound value is the caller's, untouched and unquoted — the engine quotes
/// nothing here because nothing is interpolated.
#[test]
fn the_hostile_value_arrives_as_a_bound_value_verbatim() {
    let (sql, binds) = render("SELECT {{ a }}", &ctx_with_a(EVIL)).unwrap();
    assert_eq!(sql, "SELECT ?");
    assert_eq!(binds, vec![Param::Str(EVIL.to_string())]);
}

/// A parameter's SQL type survives the template. This is what Tasks 1-3 built
/// `Param::Date`/`Timestamp`/`Decimal` for: Trino refuses
/// `day >= VARCHAR '2020-02-29'`.
#[test]
fn a_parameter_keeps_its_type_through_the_render() {
    let mut ctx = ParamValues::new();
    ctx.insert("d", Param::Date("2020-02-29".into()));
    ctx.insert("t", Param::Timestamp("2020-02-29 13:45:00".into()));
    ctx.insert("m", Param::Decimal("1234567.891".into()));
    ctx.insert("n", Param::I64(7));
    ctx.insert("b", Param::Bool(true));
    ctx.insert("z", Param::Null);

    let (sql, binds) = render(
        "SELECT {{ d }},{{ t }},{{ m }},{{ n }},{{ b }},{{ z }}",
        &ctx,
    )
    .unwrap();
    assert_eq!(sql, "SELECT ?,?,?,?,?,?");
    assert_eq!(
        binds,
        vec![
            Param::Date("2020-02-29".into()),
            Param::Timestamp("2020-02-29 13:45:00".into()),
            Param::Decimal("1234567.891".into()),
            Param::I64(7),
            Param::Bool(true),
            Param::Null,
        ]
    );
}

/// `IN (?)` binds exactly one value on both engines, so a list parameter has to
/// render N markers. It falls out of the formatter and needs no construct of its
/// own.
#[test]
fn a_list_parameter_renders_one_marker_per_element() {
    let mut ctx = ParamValues::new();
    ctx.insert_list(
        "regions",
        vec![
            Param::Str("EU".into()),
            Param::Str("US".into()),
            Param::Str(EVIL.into()),
        ],
    );
    let (sql, binds) = render(
        "IN ({% for r in regions %}{{ r }}{% if not loop.last %}, {% endif %}{% endfor %})",
        &ctx,
    )
    .unwrap();
    assert_eq!(sql, "IN (?, ?, ?)");
    assert_eq!(binds.len(), 3);
    assert!(!sql.contains("EVIL"));
}

/// An optional parameter with no value omits its block — which is why the
/// design needs no "required or default" rule. A name that was never declared
/// stays undefined and errors, so a typo is still loud.
#[test]
fn an_absent_parameter_omits_its_block_and_a_typo_is_an_error() {
    let mut ctx = ParamValues::new();
    ctx.insert_absent("region");
    let (sql, binds) = render(
        "WHERE 1=1{% if region %} AND region = {{ region }}{% endif %}",
        &ctx,
    )
    .unwrap();
    assert_eq!(sql, "WHERE 1=1");
    assert!(binds.is_empty());

    let mut supplied = ParamValues::new();
    supplied.insert("region", Param::Str("EU".into()));
    let (sql, binds) = render(
        "WHERE 1=1{% if region %} AND region = {{ region }}{% endif %}",
        &supplied,
    )
    .unwrap();
    assert_eq!(sql, "WHERE 1=1 AND region = ?");
    assert_eq!(binds, vec![Param::Str("EU".into())]);

    // Strict undefined: a name nobody declared is an error, not an empty bind.
    let err = render("SELECT {{ regoin }}", &supplied).unwrap_err();
    assert!(
        matches!(err, RenderError::Template(ref m) if m.contains("undefined")),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// The capture constructs, and the invariant behind the rule
// ---------------------------------------------------------------------------

/// **Mutation (b) kills this test.** With the rejection removed,
/// `{% filter upper %}{{ a }}{% endfilter %}` renders one marker and binds TWO
/// values, and the failure arrives as `PlaceholderBindMismatch` from `render`'s
/// own invariant instead of as `CaptureConstruct` from the rule. Both refuse; the
/// variant is what tells them apart, which is why this asserts the variant.
#[test]
fn the_capture_constructs_are_refused_by_the_rule() {
    let cases = [
        ("SELECT {% filter upper %}{{ a }}{% endfilter %}", "filter"),
        ("{% set x %}{{ a }}{% endset %}SELECT {{ x }}", "endset"),
        (
            "{% macro m() %}{{ a }}{% endmacro %}SELECT {{ m() }}",
            "macro",
        ),
        ("{% call m() %}{{ a }}{% endcall %}", "call"),
    ];
    for (tpl, want) in cases {
        let err = render(tpl, &ctx_with_a(EVIL)).unwrap_err();
        assert_eq!(err, RenderError::CaptureConstruct(want), "template: {tpl}");
    }
}

/// `{% macro %}`/`{% call %}`/`{% include %}` are gone with the `macros` and
/// `multi_template` features — but cargo features are ADDITIVE ACROSS THE
/// WORKSPACE, so one crate adding minijinja with default features would turn
/// them back on here without touching this file. The rule does not depend on the
/// feature set.
#[test]
fn the_feature_gated_constructs_are_refused_by_the_rule_too() {
    for tpl in [
        "{% include 'x' %}",
        "{% extends 'x' %}",
        "{% import 'x' as y %}",
        "{% block b %}{% endblock %}",
    ] {
        let err = render(tpl, &ParamValues::new()).unwrap_err();
        assert!(
            matches!(err, RenderError::CaptureConstruct(_)),
            "template {tpl} gave {err:?}"
        );
    }
}

/// The third capture path, not named in the spec and found by reading the VM:
/// a `recursive` for-loop whose `loop(…)` is used as a value captures the loop
/// body and re-emits it, desyncing exactly as `{% filter %}` does.
#[test]
fn a_recursive_for_loop_is_refused() {
    let mut ctx = ParamValues::new();
    ctx.insert_list("xs", vec![Param::I64(1)]);
    let err = render("{% for x in xs recursive %}{{ x }}{% endfor %}", &ctx).unwrap_err();
    assert_eq!(err, RenderError::RecursiveLoop);
}

/// **The invariant, observed firing.** A placeholder inside a string literal is
/// text to the engine — no marker stands there — while the formatter still bound
/// a value. P3's `PlaceholderInsideQuotedText` rejects this shape at publish
/// time; this is what catches it if one ever gets past.
///
/// It also pins the instrument: counting `?` in the RENDERED output (outside
/// quoted regions), not counting the formatter's own writes. Counting writes
/// would make the invariant trivially true and blind to the capture constructs.
#[test]
fn a_marker_that_does_not_stand_in_sql_structure_fails_the_invariant() {
    let err = render("SELECT * FROM t WHERE a = 'x{{ a }}y'", &ctx_with_a(EVIL)).unwrap_err();
    assert_eq!(
        err,
        RenderError::PlaceholderBindMismatch {
            placeholders: 0,
            binds: 1
        }
    );
}

/// The other direction of the same invariant: a literal `?` standing in SQL
/// structure is a bind marker the engine will demand a value for.
#[test]
fn a_literal_marker_in_the_definition_fails_the_invariant() {
    let err = render("SELECT ? , {{ a }}", &ctx_with_a("x")).unwrap_err();
    assert_eq!(
        err,
        RenderError::PlaceholderBindMismatch {
            placeholders: 2,
            binds: 1
        }
    );
}

/// A `?` inside a string literal or a comment is not a marker, and must not be
/// counted — otherwise every legitimate `WHERE q = 'why?'` would be refused.
#[test]
fn a_question_mark_in_text_is_not_counted_as_a_marker() {
    let (sql, binds) = render(
        "SELECT 'why?' /* really? */ , {{ a }} -- ?\n",
        &ctx_with_a("x"),
    )
    .unwrap();
    assert!(sql.contains("'why?'"));
    assert_eq!(binds.len(), 1);
}

// ---------------------------------------------------------------------------
// `ident` — the one deliberate hole
// ---------------------------------------------------------------------------

/// `{{ p | ident }}` emits the entry from the AUTHOR's `allowed` list. The
/// caller's bytes are compared and dropped.
#[test]
fn ident_emits_an_allowed_identifier_and_binds_nothing() {
    let mut ctx = ParamValues::new();
    ctx.insert_enum("sort", "total", vec!["day".into(), "total".into()]);
    let (sql, binds) = render("ORDER BY {{ sort | ident }}", &ctx).unwrap();
    assert_eq!(sql, "ORDER BY total");
    assert!(binds.is_empty());
}

/// **Mutation (c) kills this test.** Remove the membership match and emit the
/// caller's string, and this identifier-position injection lands in the SQL.
#[test]
fn ident_refuses_a_value_that_is_not_in_the_allowed_list() {
    let mut ctx = ParamValues::new();
    ctx.insert_enum(
        "sort",
        "total DESC, (SELECT max(secret) FROM vault)",
        vec!["day".into(), "total".into()],
    );
    let err = render("ORDER BY {{ sort | ident }}", &ctx).unwrap_err();
    assert_eq!(err, RenderError::Ident);
}

/// Two strings that are equal are not interchangeable here: the one written out
/// is the author's. The rule is structural — `allowed` reaches a render only
/// through [`ParamValues::insert_enum`] / `insert_for_spec`, so a parameter that
/// is not an enum can never satisfy `ident`.
#[test]
fn ident_refuses_a_parameter_that_is_not_an_enum() {
    let mut ctx = ParamValues::new();
    ctx.insert("sort", Param::Str("total".into()));
    let err = render("ORDER BY {{ sort | ident }}", &ctx).unwrap_err();
    assert_eq!(err, RenderError::Ident);

    // …and an enum with an empty `allowed` list is not an enum for this purpose.
    let mut empty = ParamValues::new();
    empty.insert_enum("sort", "total", vec![]);
    assert_eq!(
        render("ORDER BY {{ sort | ident }}", &empty).unwrap_err(),
        RenderError::Ident
    );
}

/// `ident` cannot be laundered: its result is author text only while it goes
/// straight to the output. Fed through any expression it becomes an ordinary
/// string and is bound like everything else.
#[test]
fn ident_output_cannot_be_re_entered_as_text() {
    let mut ctx = ParamValues::new();
    ctx.insert_enum("sort", "total", vec!["total".into()]);
    let (sql, binds) = render("ORDER BY {{ (sort | ident) ~ ' DESC' }}", &ctx).unwrap();
    assert_eq!(sql, "ORDER BY ?");
    assert_eq!(binds, vec![Param::Str("total DESC".into())]);

    // A literal cannot be turned into an identifier either.
    assert_eq!(
        render("ORDER BY {{ 'total' | ident }}", &ParamValues::new()).unwrap_err(),
        RenderError::Ident
    );
}

/// The error surface must not become a directory of what an endpoint declares.
#[test]
fn the_ident_refusal_names_neither_the_parameter_nor_the_value() {
    let mut ctx = ParamValues::new();
    ctx.insert_enum("sort_column", EVIL, vec!["day".into()]);
    let err = render("ORDER BY {{ sort_column | ident }}", &ctx).unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains("sort_column"), "{msg}");
    assert!(!msg.contains("EVIL"), "{msg}");
}

// ---------------------------------------------------------------------------
// What the template can reach
// ---------------------------------------------------------------------------

/// Python Jinja2's `__class__`/`__globals__`/`__subclasses__` escape depends on
/// objects carrying a reflective type graph. A minijinja value carries data, and
/// the parameter wrapper exposes no attributes and no methods at all.
#[test]
fn a_parameter_exposes_no_attributes_and_no_methods() {
    for tpl in [
        "SELECT {{ a.__class__ }}",
        "SELECT {{ a.__class__.__mro__ }}",
        "SELECT {{ a.upper() }}",
        "SELECT {{ a['x'] }}",
        "SELECT {{ a() }}",
    ] {
        let err = render(tpl, &ctx_with_a(EVIL)).unwrap_err();
        assert!(
            matches!(err, RenderError::Template(_)),
            "template {tpl} gave {err:?}"
        );
    }
}

/// `debug()` is `format!("{state:#?}")` — the entire template context plus an
/// inventory of every registered filter, test and global. `pprint` is the same
/// disclosure one value at a time. On a public endpoint both are context
/// disclosure, and both are removed.
#[test]
fn the_disclosure_and_construction_globals_are_gone() {
    for tpl in [
        "SELECT {{ debug() }}",
        "SELECT {{ a | pprint }}",
        "SELECT {% for i in range(3) %}{{ i }}{% endfor %}",
        "SELECT {{ dict(x=1) }}",
        "SELECT {% set ns = namespace(x=1) %}{{ ns.x }}",
    ] {
        let err = render(tpl, &ctx_with_a(EVIL)).unwrap_err();
        assert!(
            matches!(err, RenderError::Template(_)),
            "template {tpl} gave {err:?}"
        );
    }
}

/// Fuel is not a default minijinja feature; without it in the manifest this
/// loop runs to completion. Exhaustion is a clean error, not a truncated query.
#[test]
fn a_runaway_loop_runs_out_of_fuel() {
    let mut ctx = ParamValues::new();
    ctx.insert_list("xs", (0..200_000).map(Param::I64).collect());
    let err = render("SELECT {% for x in xs %}{{ x }},{% endfor %}", &ctx).unwrap_err();
    assert!(
        matches!(err, RenderError::Template(ref m) if m.contains("fuel")),
        "{err:?}"
    );
}

/// minijinja has no output-size limit of its own — fuel bounds instructions, not
/// bytes — so the sink is the only bound on how much SQL a render can produce.
#[test]
fn the_output_is_size_capped() {
    let huge = format!("SELECT {}", "x".repeat(2 * 1024 * 1024));
    assert_eq!(
        render(&huge, &ParamValues::new()).unwrap_err(),
        RenderError::TooLarge
    );
}

// ---------------------------------------------------------------------------
// The comparison limitation, made loud
// ---------------------------------------------------------------------------

/// A parameter is an opaque value so its SQL type can survive the template, and
/// minijinja compares an object against a string as NOT EQUAL. `{% if mode ==
/// 'fast' %}` would therefore be silently false forever. It is refused, and
/// `| value` is the supported spelling.
#[test]
fn comparing_a_bare_parameter_is_refused_and_value_is_the_way_to_do_it() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));

    for tpl in [
        "{% if mode == 'fast' %}A{% endif %}",
        "{% if 'fast' == mode %}A{% endif %}",
        "{% if mode != 'fast' %}A{% endif %}",
        "{% if 1 > 0 %}{% elif mode >= 'f' %}A{% endif %}",
    ] {
        assert_eq!(
            render(tpl, &ctx).unwrap_err(),
            RenderError::BareParameterComparison("mode".into()),
            "template: {tpl}"
        );
    }

    let (sql, _) = render("{% if mode | value == 'fast' %}FAST{% endif %}", &ctx).unwrap();
    assert_eq!(sql, "FAST", "the supported spelling must actually be true");
}

/// The lint reaches only the identifier adjacent to the operator, so ordinary
/// template arithmetic is untouched.
#[test]
fn the_comparison_lint_does_not_fire_on_non_parameters() {
    let mut ctx = ParamValues::new();
    ctx.insert_list("xs", vec![Param::I64(1), Param::I64(2)]);
    let (sql, binds) = render(
        "SELECT {% for x in xs %}{% if loop.index > 1 %}, {% endif %}{{ x }}{% endfor %}",
        &ctx,
    )
    .unwrap();
    assert_eq!(sql, "SELECT ?, ?");
    assert_eq!(binds.len(), 2);
}

/// `check_static` is what publish-time validation calls; it must refuse the same
/// definitions `render` refuses, without needing values.
#[test]
fn check_static_is_callable_without_rendering() {
    let ctx = ctx_with_a("x");
    assert_eq!(
        check_static("SELECT {% filter upper %}{{ a }}{% endfilter %}", &ctx),
        Err(RenderError::CaptureConstruct("filter"))
    );
    assert_eq!(check_static("SELECT {{ a }}", &ctx), Ok(()));
}

// ---------------------------------------------------------------------------
// The rule that cannot be enforced by feature selection
// ---------------------------------------------------------------------------

/// The `loader` cargo feature is a **no-op**: `set_loader` and `path_loader` are
/// compiled into minijinja unconditionally and cannot be removed by feature
/// selection. Filesystem reach is therefore one accidental call away,
/// permanently — so the workspace is asserted not to contain the call.
///
/// The needles are assembled at runtime so this file does not match itself.
#[test]
fn nothing_in_the_workspace_registers_a_template_loader() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/<crate>/..")
        .to_path_buf();
    let needles = [format!("{}_loader(", "set"), format!("{}_loader(", "path")];
    let mut offenders = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable crates directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if needles.iter().any(|n| text.contains(n.as_str())) {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a minijinja template loader is registered in: {offenders:?}. The `loader` cargo feature \
         does not remove these functions; only this check does."
    );
}
