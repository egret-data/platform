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
        // `expect`, not `unwrap_err`: a bare `unwrap_err` panics without the
        // template, and every case in this loop shares one line number.
        let err = render(tpl, &ctx).expect_err(tpl);
        assert_eq!(
            err,
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

/// **Every rule here is opened by one character** if the scanner trims only `-`.
/// `Whitespace::from_byte` maps `+` to Preserve and the lexer skips it, so
/// `{%+ filter %}` is an ordinary filter block whose keyword a `-`-only scanner
/// reads as `"+"`. Three of these are backstopped by the placeholder==bind
/// invariant; the fourth is not — `{%+ if mode == 'fast' %}` rendered `""` with
/// no binds and no error, a publishable silently-false template.
#[test]
fn a_whitespace_control_marker_does_not_open_a_hole_in_any_rule() {
    let mut ctx = ParamValues::new();
    ctx.insert("a", Param::Str(EVIL.into()));
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert_list("xs", vec![Param::I64(1)]);

    for (tpl, want) in [
        (
            "{%+ filter upper %}{{ a }}{%+ endfilter %}",
            RenderError::CaptureConstruct("filter"),
        ),
        (
            "{%+ set x %}{{ a }}{%+ endset %}{{ x }}",
            RenderError::CaptureConstruct("endset"),
        ),
        (
            "{%+ macro m() %}{{ a }}{%+ endmacro %}",
            RenderError::CaptureConstruct("macro"),
        ),
        (
            "{%+ for x in xs recursive %}{{ x }}{%+ endfor %}",
            RenderError::RecursiveLoop,
        ),
        (
            "{%+ if mode == 'fast' %}FAST{%+ endif %}",
            RenderError::BareParameterComparison("mode".into()),
        ),
        (
            "{%- if mode == 'fast' -%}FAST{%- endif -%}",
            RenderError::BareParameterComparison("mode".into()),
        ),
    ] {
        assert_eq!(render(tpl, &ctx).unwrap_err(), want, "template: {tpl}");
    }
}

/// The operator spelling is not the only spelling. `eq`/`equalto`/`ne`/`lt`/`le`/
/// `gt`/`ge`/`in`/`sameas` are registered as TESTS routing to the same
/// `ops::coerce`, so `mode is eq('fast')` is `==` in disguise; and a comparison
/// is just as silently false inside `{% set %}`, inside a for-loop's condition
/// and inside a bare `{{ … }}` as it is inside `{% if %}`. Every one of these
/// took the wrong branch with no error before the lint reached it.
#[test]
fn comparison_shaped_constructs_are_refused_wherever_they_are_written() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert_list("xs", vec![Param::I64(1)]);

    for tpl in [
        "{% if mode is eq('fast') %}FAST{% endif %}",
        "{% if mode is equalto('fast') %}FAST{% endif %}",
        "{% if mode is not eq('fast') %}FAST{% endif %}",
        "{% if 'fast' is eq(mode) %}FAST{% endif %}",
        "{% if mode is ge('f') %}FAST{% endif %}",
        // The type tests are the same silence: a parameter is an object, so it
        // is never a string, a number or none.
        "{% if mode is string %}FAST{% endif %}",
        "{% if mode is none %}FAST{% endif %}",
        "{% set f = mode == 'fast' %}{% if f %}FAST{% endif %}",
        "{{ 'FAST' if mode == 'fast' else 'SLOW' }}",
        "{{ mode == 'fast' }}",
        "{% if mode in ['fast'] %}FAST{% endif %}",
        "{% if mode not in ['fast'] %}FAST{% endif %}",
        "{% for x in xs if mode == 'fast' %}{{ x }}{% endfor %}",
    ] {
        // `expect`, not `unwrap_err`: a bare `unwrap_err` panics without the
        // template, and every case in this loop shares one line number.
        let err = render(tpl, &ctx).expect_err(tpl);
        assert_eq!(
            err,
            RenderError::BareParameterComparison("mode".into()),
            "template: {tpl}"
        );
    }
}

/// One rename would otherwise defeat the whole rule: a `{% with %}`/`{% set %}`
/// binding and a loop variable each hold the same opaque value, so comparing
/// them is silently false one level of indirection away.
#[test]
fn a_renamed_parameter_is_still_a_parameter() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);

    for (tpl, want) in [
        (
            "{% with m = mode %}{% if m == 'fast' %}FAST{% endif %}{% endwith %}",
            "m",
        ),
        ("{% set m = mode %}{% if m == 'fast' %}FAST{% endif %}", "m"),
        (
            "{% for r in regions %}{% if r == 'EU' %}X{% endif %}{% endfor %}",
            "r",
        ),
        (
            "{% for r in regions %}{% if r is eq('EU') %}X{% endif %}{% endfor %}",
            "r",
        ),
    ] {
        let err = render(tpl, &ctx).expect_err(tpl);
        assert_eq!(
            err,
            RenderError::BareParameterComparison(want.into()),
            "template: {tpl}"
        );
    }

    // …and a name bound to something that is NOT a parameter is left alone.
    let (sql, _) = render(
        "{% set label = 'x' %}{% if label == 'x' %}A{% endif %}",
        &ctx,
    )
    .unwrap();
    assert_eq!(sql, "A", "an ordinary template local must still compare");
}

/// The two things that must keep working: presence, and a loop header's `in`.
#[test]
fn presence_tests_and_the_loop_header_are_untouched() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert_absent("missing");
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);

    let (sql, _) = render("{% if mode is defined %}A{% endif %}", &ctx).unwrap();
    assert_eq!(sql, "A");
    let (sql, _) = render("{% if missing is not defined %}A{% endif %}", &ctx).unwrap();
    assert_eq!(sql, "");
    let (sql, binds) = render("{% for r in regions %}{{ r }}{% endfor %}", &ctx).unwrap();
    assert_eq!(sql, "?");
    assert_eq!(binds.len(), 1);
}

/// `{# … #}` is skipped by the lexer, so `{# {% #}` contains no tag. A
/// comment-blind scan pairs that `{%` with the NEXT `%}` and desyncs for the rest
/// of the template, stepping straight over a real `{% filter %}`.
#[test]
fn a_template_comment_cannot_desync_the_scanner() {
    let ctx = ctx_with_a("x");
    // The `filter` case below is caught by its CLOSING tag even when the scan
    // desyncs. This one is not: with the opening tag swallowed, the keyword read
    // is `#}` and the `recursive` rule never fires at all.
    let mut with_list = ParamValues::new();
    with_list.insert_list("xs", vec![Param::I64(1)]);
    assert_eq!(
        render(
            "{# {% #} {% for x in xs recursive %}{{ x }}{% endfor %}",
            &with_list
        )
        .unwrap_err(),
        RenderError::RecursiveLoop
    );
    assert_eq!(
        render("{# {% #} {% filter upper %}{{ a }}{% endfilter %}", &ctx).unwrap_err(),
        RenderError::CaptureConstruct("filter")
    );

    let (sql, binds) = render("SELECT {# a comment #}{{ a }}", &ctx).unwrap();
    assert_eq!(sql, "SELECT ?");
    assert_eq!(binds.len(), 1);
}

/// Inside a block the lexer tokenises strings, so a `%}` or `}}` in one does not
/// close it — and an operator in one is text, not a comparison.
#[test]
fn a_string_literal_inside_a_block_cannot_desync_the_scanner() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));

    let (sql, binds) = render("{{ '}}' }}", &ctx).unwrap();
    assert_eq!(sql, "?");
    assert_eq!(binds, vec![Param::Str("}}".into())]);

    let (sql, binds) = render("{% if mode %}{{ 'mode == x' }}{% endif %}", &ctx).unwrap();
    assert_eq!(sql, "?");
    assert_eq!(binds, vec![Param::Str("mode == x".into())]);
}

/// The sink caps the SQL; nothing capped the other half of a render's memory.
/// `{{ 'a' * 20000000 }}` is an eight-byte statement and 20 MB of bound value,
/// produced in milliseconds — author-privileged rather than a caller vector, but
/// it runs at publish time too, where it would take the process with it.
#[test]
fn the_bound_values_are_size_capped() {
    // Matched rather than unwrapped: without the cap this binds 20 MB, and a
    // failing `unwrap_err` would print all of it.
    match render("SELECT {{ 'a' * 20000000 }}", &ParamValues::new()) {
        Err(e) => assert_eq!(e, RenderError::BindsTooLarge),
        Ok((sql, binds)) => panic!(
            "no refusal: {} value(s) bound, {} byte(s), sql {sql:?}",
            binds.len(),
            binds
                .iter()
                .map(|p| match p {
                    Param::Str(s) => s.len(),
                    _ => 0,
                })
                .sum::<usize>()
        ),
    }
}

/// The scanner walks bytes, and a `&str` cannot be sliced at a byte that is not
/// a char boundary. Every non-ASCII byte in a definition — a `'café'` literal, a
/// Vietnamese column comment — is one, and SQL carries plenty of them.
#[test]
fn a_definition_containing_non_ascii_text_does_not_panic() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("nhanh".into()));

    for tpl in [
        "SELECT 'café' , {{ mode }}",
        "SELECT {# lọc theo chế độ #}{{ mode }}",
        "{% if mode %}-- chế độ\n{{ mode }}{% endif %}",
        "{% if mode | value == 'nhanh' %}SELECT 'khách hàng'{% endif %}",
    ] {
        let out = render(tpl, &ctx);
        assert!(out.is_ok(), "template {tpl} gave {out:?}");
    }

    // Non-ASCII INSIDE a block, and outside a string literal: this is where the
    // scanner slices. The outcome is an undefined-name error either way — what
    // this asserts is that it is an error rather than a panic.
    for tpl in [
        "{{ chế_độ }}",
        "{% if chế_độ %}A{% endif %}",
        "{{ 'x' ~ chế }}",
    ] {
        assert!(render(tpl, &ctx).is_err(), "template: {tpl}");
    }
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

// ---------------------------------------------------------------------------
// The behavioural gate
// ---------------------------------------------------------------------------

/// **The rule that no source scan has held.** Five reviews found five bypasses
/// of the comparison lint, every one of them this module's scanner disagreeing
/// with minijinja's lexer. `check_publish` does not read the source for this: it
/// renders the definition with the opaque values serving uses and again with the
/// same values as plain scalars, and a construct that INSPECTS a parameter takes
/// a different branch. There is no spelling to evade.
///
/// Every template below is written so the byte scanner cannot see it — through a
/// parenthesis, a subscript, a filter, a test, a raw block, an alias.
#[test]
fn the_behavioural_gate_catches_inspection_whatever_its_spelling() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("slow".into()));
    ctx.insert_list("regions", vec![Param::Str("US".into())]);

    for tpl in [
        "{% if (mode) == 'fast' %}FAST{% endif %}",
        "{% if ((mode)) == 'fast' %}FAST{% endif %}",
        "{% if regions[0] == 'EU' %}X{% endif %}",
        "{% if regions | first == 'EU' %}X{% endif %}",
        "{% if mode is eq('fast') %}FAST{% endif %}",
        "{% if mode is string %}FAST{% endif %}",
        "{{ 'FAST' if (mode) == 'fast' else 'SLOW' }}",
        "{% for r in regions if (r) == 'EU' %}{{ r }}{% endfor %}",
        // The scanner's own blind spot from the last round, kept here because
        // the gate does not care how it was hidden.
        "{% raw %}{#{% endraw %}{% if mode == 'fast' %}FAST{% endif %}",
    ] {
        let err = egret_dataapi::render::check_publish(tpl, &ctx).expect_err(tpl);
        assert!(
            matches!(
                err,
                RenderError::ParameterInspected | RenderError::BareParameterComparison(_)
            ),
            "template {tpl} gave {err:?}"
        );
    }
}

/// The gate must be quiet on definitions that only BIND their parameters —
/// including the spec's own canonical endpoint, which uses every construct the
/// design offers.
#[test]
fn the_behavioural_gate_is_quiet_on_definitions_that_only_bind() {
    let mut ctx = ParamValues::new();
    ctx.insert("region", Param::Str("EU".into()));
    ctx.insert("from", Param::Date("2020-02-29".into()));
    ctx.insert_absent("to");
    ctx.insert_list(
        "regions",
        vec![Param::Str("EU".into()), Param::Str("US".into())],
    );
    ctx.insert_enum("sort", "total", vec!["day".into(), "total".into()]);

    let canonical = "\
SELECT region, SUM(amount) AS total
FROM sales
WHERE 1=1
  {% if region %} AND region = {{ region }} {% endif %}
  {% if from %}   AND day >= {{ from }}     {% endif %}
  {% if to %}     AND day <= {{ to }}       {% endif %}
  {% if regions %} AND region IN (
      {% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %}
  ) {% endif %}
GROUP BY region
ORDER BY {{ sort | ident }}";
    assert_eq!(
        egret_dataapi::render::check_publish(canonical, &ctx),
        Ok(())
    );

    for tpl in [
        "SELECT {{ region }}",
        "SELECT {% raw %}{{ literal }}{% endraw %} , {{ region }}",
        "{% if regions | length > 0 %}SELECT {{ region }}{% endif %}",
        "{% if region | value == 'EU' %}SELECT 1{% endif %}",
        "{% set label = 'x' %}{% if label == 'x' %}SELECT 1{% endif %}",
    ] {
        assert_eq!(
            egret_dataapi::render::check_publish(tpl, &ctx),
            Ok(()),
            "template: {tpl}"
        );
    }
}

/// `{% raw %}` makes the lexer emit everything up to `{% endraw %}` as data. A
/// scanner that does not know that reads `{% raw %}{#{% endraw %}` as an
/// unterminated comment, gives up, and every rule below is off for the rest of
/// the definition.
#[test]
fn a_raw_block_does_not_switch_the_rules_off() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert("a", Param::Str(EVIL.into()));

    assert_eq!(
        render(
            "{% raw %}{#{% endraw %}{% if mode == 'fast' %}FAST{% endif %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::BareParameterComparison("mode".into())
    );
    assert_eq!(
        render(
            "{% raw %}{#{%- endraw +%}{% filter upper %}{{ a }}{% endfilter %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::CaptureConstruct("filter")
    );
    // The raw block's own contents stay literal, and are not rules.
    let (sql, binds) = render("{% raw %}{% filter x %}{{ a }}{% endraw %}", &ctx).unwrap();
    assert_eq!(sql, "{% filter x %}{{ a }}");
    assert!(binds.is_empty());
    // An unterminated raw is refused rather than scanned on a guess.
    assert_eq!(
        render("{% raw %}{% if mode == 'fast' %}", &ctx).unwrap_err(),
        RenderError::UnterminatedRaw
    );
}

/// The three spellings that walked past an adjacency check.
#[test]
fn the_lint_reads_the_whole_operand_not_the_token_beside_the_operator() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);

    for (tpl, want) in [
        ("{% if (mode) == 'fast' %}A{% endif %}", "mode"),
        ("{% if regions[0] == 'EU' %}A{% endif %}", "regions"),
        ("{% if regions | first == 'EU' %}A{% endif %}", "regions"),
        ("{% if 'EU' == regions[0] %}A{% endif %}", "regions"),
    ] {
        let err = render(tpl, &ctx).expect_err(tpl);
        assert_eq!(
            err,
            RenderError::BareParameterComparison(want.into()),
            "template: {tpl}"
        );
    }

    // …and the correct forms still render.
    for tpl in [
        "{% if regions | length > 0 %}A{% endif %}",
        "{% if mode | value == 'fast' %}A{% endif %}",
    ] {
        assert_eq!(render(tpl, &ctx).unwrap().0, "A", "template: {tpl}");
    }
}

/// A loop variable compared in the very tag that binds it: the aliases have to
/// be collected before that tag's own condition is read.
#[test]
fn a_loop_variable_is_a_parameter_inside_its_own_header() {
    let mut ctx = ParamValues::new();
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);
    assert_eq!(
        render(
            "{% for r in regions if r == 'EU' %}{{ r }}{% endfor %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::BareParameterComparison("r".into())
    );
}

/// Two alias defects: a tuple target aliased only its last name, and unwrapping
/// with `| value` — the documented right thing to do — still produced an alias
/// and refused the comparison that followed.
#[test]
fn aliases_cover_every_target_and_stop_at_an_unwrap() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));
    ctx.insert("other", Param::Str("x".into()));

    assert_eq!(
        render(
            "{% set m, n = mode, other %}{% if m == 'fast' %}A{% endif %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::BareParameterComparison("m".into())
    );
    assert_eq!(
        render(
            "{% set m = mode | value %}{% if m == 'fast' %}A{% endif %}",
            &ctx
        )
        .unwrap()
        .0,
        "A",
        "unwrapping with `| value` must not leave an alias behind"
    );
}

/// `split_whitespace` never saw this one; the modifier needs no space.
#[test]
fn the_recursive_rule_does_not_need_whitespace() {
    let mut ctx = ParamValues::new();
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);
    assert_eq!(
        render("{% for x in (regions)recursive %}{{ x }}{% endfor %}", &ctx).unwrap_err(),
        RenderError::RecursiveLoop
    );
}

/// A single value larger than the whole bind budget is refused.
///
/// The budget is charged from the value's size BEFORE `to_param` clones it, so
/// the cap does not itself double the allocation it exists to prevent — but this
/// assertion cannot see that ordering, only the refusal. The ordering is a
/// code-level property with no observable difference, and no mutation is claimed
/// for it.
#[test]
fn a_value_larger_than_the_bind_cap_is_refused() {
    match render("SELECT {{ 'a' * 99000000 }}", &ParamValues::new()) {
        Err(e) => assert_eq!(e, RenderError::BindsTooLarge),
        Ok((sql, binds)) => panic!("no refusal: {} value(s), sql {sql:?}", binds.len()),
    }
}

// ---------------------------------------------------------------------------
// Round 3: names, shapes and budgets
// ---------------------------------------------------------------------------

/// **A parameter's NAME must never switch the rule off.** The filters that mark
/// a value as already-plain are honoured only in filter position, after a `|`.
/// Matched anywhere in the operand, an author who names a parameter `format` —
/// or `count`, `value`, `length`, `sum`, `title`, `string`, `int`, `float`,
/// `bool`, `join`, `trim`, `round`, `replace` — turns the lint off by writing
/// the plainest comparison there is. No evasion, no unusual spelling.
#[test]
fn a_parameter_named_like_a_filter_is_still_a_parameter() {
    for name in [
        "format", "count", "value", "length", "sum", "title", "string", "int", "float", "bool",
        "join", "trim", "round", "replace",
    ] {
        let mut ctx = ParamValues::new();
        ctx.insert(name, Param::Str("csv".into()));
        let tpl = format!("{{% if {name} == 'csv' %}}A{{% endif %}}");
        let err = render(&tpl, &ctx).expect_err(&tpl);
        assert_eq!(
            err,
            RenderError::BareParameterComparison(name.into()),
            "template: {tpl}"
        );

        // …and the same name aliased through a `set` is still a parameter.
        let tpl = format!("{{% set m = {name} %}}{{% if m == 'csv' %}}A{{% endif %}}");
        let err = render(&tpl, &ctx).expect_err(&tpl);
        assert_eq!(
            err,
            RenderError::BareParameterComparison("m".into()),
            "template: {tpl}"
        );
    }

    // The escape hatch still works, because `value` is in FILTER position there.
    let mut ctx = ParamValues::new();
    ctx.insert("format", Param::Str("csv".into()));
    assert_eq!(
        render("{% if format | value == 'csv' %}A{% endif %}", &ctx)
            .unwrap()
            .0,
        "A"
    );
}

/// A branch only the plain values take, containing something that cannot render,
/// is a DIVERGENCE — the opaque render succeeded and the plain one did not.
/// Treating it as inconclusive threw away a positive.
#[test]
fn the_gate_treats_a_failed_plain_render_as_a_divergence() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("slow".into()));
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);

    // `{{ regions }}` emits a whole list, which no engine can bind — so the
    // branch renders only when the comparison is true, i.e. only in the plain
    // render.
    let tpl = "{% if (mode) == 'fast' %}{{ regions }}{% endif %}SELECT 1";
    assert_eq!(
        egret_dataapi::render::check_dynamic(tpl, &ctx),
        Err(RenderError::ParameterInspected)
    );
}

/// `{% with a = 1, b = mode %}` is several assignments, not a tuple. Taking the
/// first `=` aliased `a` and stopped, reopening the rename bypass.
#[test]
fn every_assignment_in_a_with_block_is_aliased() {
    let mut ctx = ParamValues::new();
    ctx.insert("mode", Param::Str("fast".into()));

    assert_eq!(
        render(
            "{% with a = 1, b = mode %}{% if b == 'fast' %}A{% endif %}{% endwith %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::BareParameterComparison("b".into())
    );
    // The tuple form — one `=`, commas on both sides — still binds both names.
    assert_eq!(
        render(
            "{% set m, n = mode, mode %}{% if n == 'fast' %}A{% endif %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::BareParameterComparison("n".into())
    );
    // One segment unwrapping with `| value` must not silence the others.
    assert_eq!(
        render(
            "{% with a = mode | value, b = mode %}{% if b == 'fast' %}A{% endif %}{% endwith %}",
            &ctx
        )
        .unwrap_err(),
        RenderError::BareParameterComparison("b".into())
    );
}

/// The probe budget must degrade evenly. Parameter-major spending exhausts it on
/// whichever parameter sorts first — `names()` is alphabetical — and every later
/// parameter is then never probed at all.
#[test]
fn the_probe_budget_reaches_the_alphabetically_last_parameter() {
    let mut ctx = ParamValues::new();
    // `zzz` sorts last, and the definition carries far more literals than the
    // per-parameter budget would allow before reaching it.
    for name in ["aaa", "bbb", "ccc", "ddd"] {
        ctx.insert(name, Param::Str("x".into()));
    }
    ctx.insert("zzz", Param::Str("slow".into()));
    let noise: String = (0..20).map(|i| format!("-- 'lit{i}'\n")).collect();
    let tpl = format!(
        "{noise}{{% if aaa %}}{{{{ aaa }}}}{{% endif %}}{{% if bbb %}}{{{{ bbb }}}}{{% endif %}}\
         {{% if ccc %}}{{{{ ccc }}}}{{% endif %}}{{% if ddd %}}{{{{ ddd }}}}{{% endif %}}\
         {{% if (zzz) == 'fast' %}}FAST{{% endif %}}"
    );
    assert_eq!(
        egret_dataapi::render::check_dynamic(&tpl, &ctx),
        Err(RenderError::ParameterInspected)
    );
}

/// `| ident` needs ONE enum opaque. Keeping every enum opaque blinds the gate to
/// all the others — and spec §4.3's canonical endpoint is exactly that shape.
#[test]
fn an_ident_parameter_does_not_blind_the_gate_to_other_enums() {
    let mut ctx = ParamValues::new();
    ctx.insert_enum("sort", "total", vec!["day".into(), "total".into()]);
    ctx.insert_enum("mode", "slow", vec!["slow".into(), "fast".into()]);

    let tpl = "SELECT {% if (mode) == 'fast' %}1{% else %}2{% endif %} ORDER BY {{ sort | ident }}";
    assert_eq!(
        egret_dataapi::render::check_dynamic(tpl, &ctx),
        Err(RenderError::ParameterInspected)
    );
}

/// The `is` branch reads the whole subject region too.
#[test]
fn a_test_applied_after_a_filter_is_still_an_inspection() {
    let mut ctx = ParamValues::new();
    ctx.insert_list("regions", vec![Param::Str("EU".into())]);
    assert_eq!(
        render("{% if regions | first is string %}A{% endif %}", &ctx).unwrap_err(),
        RenderError::BareParameterComparison("regions".into())
    );
    // …and a plainising filter in FILTER position still permits the test.
    assert_eq!(
        render("{% if regions | length is number %}A{% endif %}", &ctx)
            .unwrap()
            .0,
        "A"
    );
}
