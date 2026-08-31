//! Package S Task 6 — serving GENERATES, then renders.
//!
//! **The one property this file exists for**: a statement whose SQL a script
//! built binds the caller's value exactly as a hand-written statement does. Not
//! "similarly" — the assertion below compares the two at the port and demands
//! the same bytes and the same bind list, because the whole safety argument of
//! package S is that generation changes what the SQL says and changes nothing
//! about how a caller's value reaches the engine. The script returns a TEMPLATE
//! with its `{{ }}` intact and that template goes into the same
//! `crate::render::render` call written SQL goes into; a second render path, or
//! an interpolation instead of a bind, is the defect the design exists to
//! prevent.
//!
//! # What stops these passing vacuously
//!
//! * The fake engine's rows are `marker(principal) + bind_signature(params)`
//!   (`serving_common`), so a serving path that bound nothing — or that sent the
//!   template unrendered — collapses the signature and the "two values differ"
//!   assertions fail.
//! * `FakePort::last()` records what the port was ASKED to run, so the
//!   comparison is on the statement itself rather than on rows that could agree
//!   for other reasons.
//! * `FakePort::executions()` is counted across every refusal, because "a 500"
//!   and "a 500 after running DML on the caller's behalf" are the same status
//!   code and very different events.
//! * The cache test rewrites the frozen script to an unrunnable one between two
//!   calls, and then proves the rewrite TOOK by making a third call whose key is
//!   cold and watching it fail. A cache test whose mutation silently did not
//!   happen is a cache test that passes with the cache deleted.
//!
//! DB-gated; self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set, and a run with no `DATABASE_URL` otherwise FAILS — check the DURATION, not
//! just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use egret_query_api::Param;
use serving_common::{bind_signature, fixture, marker, Fixture};

/// Self-skip when there is no database, which `egret_test_db::url()` now permits only under
/// `EGRET_ALLOW_DB_SKIP`. The DURATION is what says whether these ran; an `ok` alone does
/// not.
macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

/// The statement both halves of the comparison run, written by hand.
const WRITTEN: &str = "SELECT n FROM t WHERE region = {{ region }}";

/// A script whose EMITTED TEXT depends on the caller's value — the shape a
/// frozen generated template cannot express.
const BRANCHING_SCRIPT: &str = "def build_sql(params):\n    \
     if params[\"region\"] == \"US\":\n        \
     return \"SELECT n FROM t WHERE country = {{ region }}\"\n    \
     return \"SELECT n FROM t WHERE region = {{ region }}\"\n";

/// A script that returns exactly [`WRITTEN`], with its braces intact.
const SCRIPT: &str =
    "def build_sql(params):\n    return \"SELECT n FROM t WHERE region = {{ region }}\"\n";

fn one_string_param(name: &str) -> serde_json::Value {
    serde_json::json!([{ "name": name, "type": "string", "location": "query", "required": true }])
}

/// The same parameter, bounded — which package 5 requires of every `string` on
/// an endpoint a SCRIPT builds SQL for.
///
/// A separate helper rather than a `pattern` added to the one above, so the
/// written half of every comparison in this file keeps declaring exactly what it
/// declared before package 5: the rule applies to generated statements only, and
/// a fixture that constrained both halves could not show that.
fn one_bounded_string_param(name: &str) -> serde_json::Value {
    serde_json::json!([{
        "name": name,
        "type": "string",
        "location": "query",
        "required": true,
        "pattern": "^[A-Za-z0-9_]*$",
    }])
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

async fn publish_written(f: &Fixture, path: &str, sql: &str) {
    f.publish_statements_with_params(
        path,
        f.database_id,
        serde_json::json!([{ "name": "main", "sql": sql }]),
        one_string_param("region"),
        no_cache(),
    )
    .await;
}

async fn publish_scripted(f: &Fixture, path: &str, script: &str) -> uuid::Uuid {
    f.publish_statements_with_params(
        path,
        f.database_id,
        serde_json::json!([{ "name": "main", "script": script }]),
        one_bounded_string_param("region"),
        no_cache(),
    )
    .await
}

// ---------------------------------------------------------------------------
// THE test.
// ---------------------------------------------------------------------------

/// **The caller's value reaches the engine as a BIND, exactly as it does for
/// hand-written SQL.**
///
/// Asserted by comparing the two at the port rather than by asserting a shape
/// twice: "the same" is the claim, so the test states it as an equality. A
/// generated statement that interpolated the value would still be one statement
/// with one value in it, and every shape assertion about it could still pass —
/// what could not pass is being byte-identical to the written one while binding
/// the same list.
#[tokio::test]
async fn a_generated_statement_reaches_the_engine_exactly_as_a_written_one_does() {
    let f = skip_without_db!();
    publish_written(&f, "/written", WRITTEN).await;
    publish_scripted(&f, "/generated", SCRIPT).await;

    let written = f.call_as(f.principal_a, "written?region=EU").await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);
    let written_call = f.port.last();

    let generated = f.call_as(f.principal_a, "generated?region=EU").await;
    assert_eq!(generated.status, StatusCode::OK, "{}", generated.body);
    let generated_call = f.port.last();

    assert_eq!(
        generated_call, written_call,
        "a generated statement must reach the engine as the same statement with \
         the same binds a written one does — anything else means the script's \
         text took a different path to the engine"
    );

    // And said explicitly, so a failure reads as what it is rather than as two
    // tuples that differ somewhere.
    let (sql, binds) = generated_call;
    assert_eq!(
        binds,
        vec![Param::Str("EU".into())],
        "the caller's value must travel as a BIND: {sql}"
    );
    assert_eq!(
        sql, "SELECT n FROM t WHERE region = ?",
        "the value must not be interpolated into the text, and the template must \
         not reach the engine unrendered"
    );

    // The rows a caller actually gets depend on the value, which is what makes
    // the binds above load-bearing rather than decorative.
    let us = f.call_as(f.principal_a, "generated?region=US").await;
    assert_eq!(us.status, StatusCode::OK, "{}", us.body);
    assert_ne!(
        us.ns(),
        generated.ns(),
        "two values of a parameter produced identical rows from a generated \
         statement — nothing was bound"
    );

    // **And the SQL itself is a function of the request**, which is the half of
    // this test that says the version cannot be serving a text frozen at publish
    // time. Without it, a snapshot that froze the generated baseline satisfies
    // every assertion above — measured: the mutation that freezes the generated
    // text left this test GREEN until this endpoint existed, because the fixture
    // script above ignores its parameters.
    publish_scripted(&f, "/branching", BRANCHING_SCRIPT).await;
    let eu = f.call_as(f.principal_a, "branching?region=EU").await;
    assert_eq!(eu.status, StatusCode::OK, "{}", eu.body);
    let eu_call = f.port.last();
    let xx = f.call_as(f.principal_a, "branching?region=US").await;
    assert_eq!(xx.status, StatusCode::OK, "{}", xx.body);
    let xx_call = f.port.last();

    assert_eq!(
        eu_call,
        (
            "SELECT n FROM t WHERE region = ?".to_string(),
            vec![Param::Str("EU".into())]
        ),
        "the branch publish's representative values take"
    );
    assert_eq!(
        xx_call,
        (
            "SELECT n FROM t WHERE country = ?".to_string(),
            vec![Param::Str("US".into())]
        ),
        "and the branch only this request takes — generated for THIS caller, \
         with the value still a bind"
    );
    assert_ne!(
        eu_call.0, xx_call.0,
        "a version that served a text frozen at publish would send one statement \
         to both callers"
    );
}

/// A written endpoint is byte-for-byte what it was before package S.
///
/// The test most likely to catch a refactor that "just moves" the render call.
/// It asserts the statement, the binds, the body and `meta` — the four things a
/// consumer already codes against — for a definition that carries a `{% if %}`,
/// so the answer depends on the render rather than on a constant.
#[tokio::test]
async fn an_endpoint_with_written_sql_is_unchanged() {
    let f = skip_without_db!();
    publish_written(
        &f,
        "/plain",
        "SELECT n FROM t WHERE 1=1 {% if region %} AND region = {{ region }}{% endif %}",
    )
    .await;

    let res = f.call_as(f.principal_a, "plain?region=EU").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let (sql, binds) = f.port.last();
    assert_eq!(
        sql, "SELECT n FROM t WHERE 1=1  AND region = ?",
        "the rendered statement must be exactly what it was before generation \
         existed, whitespace included"
    );
    assert_eq!(binds, vec![Param::Str("EU".into())]);
    // **`data`, not only the envelope around it.** The rows are
    // `marker(principal) + bind_signature(binds)`, so this asserts the body a
    // consumer receives AND that the value reaching the engine is the caller's:
    // a serving path that bound nothing collapses the signature term, and the
    // literal below stops moving with it.
    assert_eq!(
        res.body["data"],
        serde_json::json!([{
            "n": marker(f.principal_a) + bind_signature(&[Param::Str("EU".into())])
        }]),
        "the body is byte-for-byte what it was: {}",
        res.body
    );
    assert_eq!(res.body["status"], "ok", "{}", res.body);
    assert_eq!(res.body["meta"]["cached"], false, "{}", res.body);
    assert_eq!(
        res.body["meta"]["row_count"], 1,
        "the envelope a consumer reads is unchanged: {}",
        res.body
    );
}

/// **A generated template that INSPECTS a parameter is refused, and the reason
/// it has to be is that serving it is a `200` rather than an error.**
///
/// A parameter reaches a template as an opaque `Cell` so its SQL type survives,
/// and minijinja compares an object against a string as NOT EQUAL. So
/// `{% if [region, 'x'][0] == 'ZZ' %}` is silently false, the `{% if %}` drops
/// the filter it guards, and the endpoint answers with rows nobody asked for.
/// `render::check_static` is the scanner that refuses such comparisons and it
/// has been bypassed by five recorded spellings; this is a sixth, pinned in
/// `render::tests::a_parameter_wrapped_in_a_list_literal_is_caught_only_by_the_gate`.
///
/// For a WRITTEN statement none of that reaches serving: publish runs the
/// behavioural gate (`render::check_dynamic`) over the exact bytes that will
/// execute. For a generated one, the bytes may be a branch publish never saw —
/// here, the arm taken only when `region == "XX"`, which no sentinel probe
/// supplies. So serving runs the gate on generated text.
///
/// The `region=EU` call is not decoration: it is what says this endpoint is
/// otherwise perfectly serviceable, so the refusal below is about the branch.
#[tokio::test]
async fn a_generated_template_that_inspects_a_parameter_is_refused_at_serve() {
    let f = skip_without_db!();
    publish_scripted(
        &f,
        "/inspects",
        "def build_sql(params):\n    if params[\"region\"] == \"XX\":\n        \
         return \"SELECT n FROM t WHERE 1=1 {% if [region, 'x'][0] == 'ZZ' %} AND \
         region = {{ region }}{% endif %}\"\n    return \"SELECT n FROM t WHERE \
         region = {{ region }}\"\n",
    )
    .await;

    let ok = f.call_as(f.principal_a, "inspects?region=EU").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(
        f.port.last(),
        (
            "SELECT n FROM t WHERE region = ?".to_string(),
            vec![Param::Str("EU".into())]
        )
    );

    let before = f.port.executions();
    let res = f.call_as(f.principal_a, "inspects?region=XX").await;
    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a template whose comparison is silently false must not be served: {}",
        res.body
    );
    assert_eq!(res.body["error"]["code"], "internal", "{}", res.body);
    assert_eq!(
        f.port.executions(),
        before,
        "and the statement with its filter dropped must never reach the engine — \
         that is the 200 this refusal exists to prevent"
    );
}

// ---------------------------------------------------------------------------
// Failure is the ENDPOINT's, never the caller's.
// ---------------------------------------------------------------------------

/// A script that raises is a 500, and the engine is never reached.
///
/// **Never a 400.** The caller sent a valid request — authenticated, authorized,
/// inside its rate limit, and its parameter coerced. What failed is the
/// author's script. A 400 would blame the caller for it and write
/// `param_invalid` into `access_log`, pointing every later investigation at the
/// wrong half of the system.
///
/// Publish cannot catch this one and that is the point: its probes supply two
/// values per parameter (a sentinel and `None`), and neither is `boom`.
#[tokio::test]
async fn a_generation_failure_is_an_internal_error_never_a_400() {
    let f = skip_without_db!();
    publish_scripted(
        &f,
        "/raises",
        "def build_sql(params):\n    if params[\"region\"] == \"boom\":\n        \
         fail(\"this endpoint's script is broken\")\n    return \"SELECT n FROM t WHERE \
         region = {{ region }}\"\n",
    )
    .await;

    // The other values still work, so the refusal below is about the branch and
    // not about the endpoint being broken outright.
    let fine = f.call_as(f.principal_a, "raises?region=EU").await;
    assert_eq!(fine.status, StatusCode::OK, "{}", fine.body);

    let before = f.port.executions();
    let res = f.call_as(f.principal_a, "raises?region=boom").await;
    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a script that raised is the endpoint failing, not the caller: {}",
        res.body
    );
    assert_eq!(res.body["error"]["code"], "internal", "{}", res.body);
    assert_eq!(
        f.port.executions(),
        before,
        "nothing may reach the engine when no statement could be built"
    );
}

/// **A branch publish's probes never took cannot smuggle DML past the gate.**
///
/// This is the asymmetry package S Task 6 creates and has to close. Publish
/// scans every template it can SEE, and for a script that is the baseline plus
/// what two sentinel probes per parameter emitted — so the `region == "XX"` arm
/// below is invisible to it, and the endpoint publishes green. Serving is where
/// that arm becomes a statement, executed with the CALLER's engine credentials
/// on a delegated connection.
///
/// Task 3's Mutation A measured the same asymmetry from the other side: `;` and
/// `#` have a backstop inside `render`, and the head word has none. So the head
/// word is checked where the text is built.
///
/// The publish succeeding is half the test. If a future publish rule learnt to
/// refuse this script, this test would fail at the publish rather than at the
/// call — and that is the right failure, because it would mean the assumption
/// this serve-time check rests on had changed.
#[tokio::test]
async fn a_script_cannot_emit_dml_on_a_branch_publish_never_probed() {
    let f = skip_without_db!();
    publish_scripted(
        &f,
        "/dml",
        "def build_sql(params):\n    if params[\"region\"] == \"XX\":\n        \
         return \"DELETE FROM t\"\n    return \"SELECT n FROM t WHERE region = \
         {{ region }}\"\n",
    )
    .await;

    let ok = f.call_as(f.principal_a, "dml?region=EU").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);

    let before = f.port.executions();
    let res = f.call_as(f.principal_a, "dml?region=XX").await;
    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a generated statement that is not a read is refused: {}",
        res.body
    );
    assert_eq!(res.body["error"]["code"], "internal", "{}", res.body);
    assert_eq!(
        f.port.executions(),
        before,
        "the DELETE must not reach the engine — this assertion is the whole \
         point of the one above"
    );
}

/// **A template past the length cap is refused on the branch that emits it, and
/// the engine is never asked to plan it** (package S Task 8.5, spec S5).
///
/// The same asymmetry as the DML test above, on a different rule and reached a
/// different way. Publish sees the baseline plus what two sentinel probes per
/// parameter emitted, and neither of those values is `XX` — so the megabyte arm
/// is invisible to it and the endpoint publishes green. `MAX_GENERATED_SQL_BYTES`
/// lives inside `build_sql`, which serving calls per request, so the branch is
/// refused when it is finally built.
///
/// **A 500 rather than a 400, and that is the answer this rule wants.** The
/// caller's request is valid — the `region=EU` call proves the endpoint is
/// otherwise perfectly serviceable — and what failed is the author's script.
/// The alternative to failing here is handing an engine a statement four orders
/// of magnitude past anything it was meant to plan.
///
/// The publish succeeding is half the test, for the reason the DML test gives:
/// if a future publish rule learnt to refuse this script, this would fail at the
/// publish rather than at the call, and that is the right failure.
#[tokio::test]
async fn a_generated_template_over_the_length_cap_is_refused_at_serve() {
    let f = skip_without_db!();
    publish_scripted(
        &f,
        "/toolong",
        "def build_sql(params):\n    if params[\"region\"] == \"XX\":\n        \
         return \"SELECT \" + \"z\" * 200000\n    return \"SELECT n FROM t WHERE \
         region = {{ region }}\"\n",
    )
    .await;

    let ok = f.call_as(f.principal_a, "toolong?region=EU").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);

    let before = f.port.executions();
    let res = f.call_as(f.principal_a, "toolong?region=XX").await;
    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a generated statement over the byte cap is refused: {}",
        res.body
    );
    assert_eq!(res.body["error"]["code"], "internal", "{}", res.body);
    assert_eq!(
        f.port.executions(),
        before,
        "the oversized statement must not reach the engine"
    );
    assert!(
        !res.body.to_string().contains("region"),
        "a serving refusal names no parameter of the endpoint: {}",
        res.body
    );
}

// ---------------------------------------------------------------------------
// Where generation sits: after the cache, and against the frozen shape.
// ---------------------------------------------------------------------------

/// **A cache hit runs no script at all.**
///
/// Generation has no side effect to count, so the instrument is the script
/// itself: it is rewritten to an unrunnable one between two identical calls. If
/// generation ran on a hit, the second call would be a 500.
///
/// The third call is what stops this passing for the wrong reason. It asks for a
/// different value — a cold key — against the SAME rewritten snapshot, and must
/// fail. Without it, a rewrite that silently did not happen and a cache hit that
/// skipped generation look exactly alike.
#[tokio::test]
async fn a_cache_hit_serves_without_running_the_script() {
    let f = skip_without_db!();
    let ep = f
        .publish_statements_with_params(
            "/cached",
            f.database_id,
            serde_json::json!([{ "name": "main", "script": SCRIPT }]),
            one_bounded_string_param("region"),
            serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" }),
        )
        .await;

    let first = f.call_as(f.principal_a, "cached?region=EU").await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert!(!first.cached(), "the first call cannot be a hit");
    let after_first = f.port.executions();

    // The frozen script becomes unrunnable. Nothing else about the version
    // changes — same id, same version number, same parameters, same key.
    f.rewrite_live_snapshot(ep, |mut snap| {
        snap["statements"][0]["script"] =
            serde_json::json!("def build_sql(params):\n    fail(\"never runs on a hit\")\n");
        snap
    })
    .await;

    let second = f.call_as(f.principal_a, "cached?region=EU").await;
    assert_eq!(
        second.status,
        StatusCode::OK,
        "a warm entry must not pay for generation: {}",
        second.body
    );
    assert!(second.cached(), "{}", second.body);
    assert_eq!(
        f.port.executions(),
        after_first,
        "a hit executes nothing either"
    );
    assert_eq!(second.ns(), first.ns(), "and it is the same response");

    // The control: the rewrite really happened, and a MISS really does generate.
    let cold = f.call_as(f.principal_a, "cached?region=US").await;
    assert_eq!(
        cold.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "if this is a 200 the rewrite above did not take, and the assertion \
         about the hit proved nothing: {}",
        cold.body
    );
}

/// A scripted statement that iterates a parameter gets it as a LIST — which is
/// only possible because the shape decision is frozen.
///
/// `?region=EU` is one value, and `{% for r in region %}` over a scalar is not a
/// one-element loop, it is "plain is not iterable" — a 500 on an ordinary
/// request. Serving used to decide this by scanning each statement's SQL, and a
/// scripted statement has no SQL to scan until its parameters are resolved,
/// which is the answer it needs first. So publish decides it across every
/// template and freezes it in `EndpointSnapshot::iterated`.
///
/// The second half rewrites that field away and shows the request failing, which
/// is what makes the first half an assertion about the frozen set rather than
/// about a default that happened to be right.
#[tokio::test]
async fn a_generated_loop_reads_the_frozen_shape_decision() {
    let f = skip_without_db!();
    let ep = publish_scripted(
        &f,
        "/loop",
        "def build_sql(params):\n    return \"SELECT n FROM t WHERE region IN \
         ({% for r in region %}{{ r }}{% if not loop.last %}, {% endif %}{% endfor %})\"\n",
    )
    .await;

    let one = f.call_as(f.principal_a, "loop?region=EU").await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.body);
    assert_eq!(
        f.port.last(),
        (
            "SELECT n FROM t WHERE region IN (?)".to_string(),
            vec![Param::Str("EU".into())]
        ),
        "one value of an iterated parameter is a list of one"
    );

    let two = f.call_as(f.principal_a, "loop?region=EU&region=US").await;
    assert_eq!(two.status, StatusCode::OK, "{}", two.body);
    assert_eq!(
        f.port.last(),
        (
            "SELECT n FROM t WHERE region IN (?, ?)".to_string(),
            vec![Param::Str("EU".into()), Param::Str("US".into())]
        ),
        "each repeat is its own bind marker and its own value"
    );

    // The frozen decision is load-bearing: without it the same request cannot be
    // served at all.
    f.rewrite_live_snapshot(ep, |mut snap| {
        snap["iterated"] = serde_json::json!([]);
        snap
    })
    .await;
    let broken = f.call_as(f.principal_a, "loop?region=EU").await;
    assert_eq!(
        broken.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a snapshot whose frozen shape decision is gone cannot render its own \
         loop — which is what says the field above is doing the work: {}",
        broken.body
    );
}

// ---------------------------------------------------------------------------
// The type a generating script is handed. C1, from the whole-branch review.
// ---------------------------------------------------------------------------

/// A script that BRANCHES on the parameter's value and emits a `{% for %}` on
/// the branch it does not take.
///
/// Both halves matter. The comparison is what reads the value, so the script's
/// answer depends on the TYPE it is handed; the `{% for %}` header is what puts
/// `region` in the endpoint's frozen `iterated` set, which is the thing that
/// used to change that type between publish and serve.
const BRANCHES_ON_ITS_VALUE: &str = "def build_sql(params):\n    \
     if params[\"region\"] == \"ALL\":\n        \
     return \"SELECT n FROM t\"\n    \
     return \"SELECT n FROM t WHERE region IN ({% for r in region %}{{ r }}\
     {% if not loop.last %}, {% endif %}{% endfor %})\"\n";

/// **A generating script reads the caller's value in the same shape publish
/// probed it with.**
///
/// Publish hands every declared parameter a scalar `representative_value`.
/// Serving used to hand the script `Resolved::json`, whose shape follows the
/// frozen `iterated` set — so a parameter ANY statement loops over arrived at
/// the script as a one-element LIST while publish had shown the author a
/// scalar. `params["region"] == "ALL"` is then true for the author, false for
/// every caller, and the script silently takes the other branch.
///
/// **The failure this pins is a 200 with the wrong rows.** Both branches are
/// legitimate `SELECT`s, so `check_single_select` and `check_dynamic` pass, the
/// render binds `'ALL'` into an `IN` list nobody asked for, and nothing anywhere
/// says so. That is why the assertion is on the statement at the PORT rather
/// than on the status: the status was always 200.
///
/// The second half is the control. Without it a script that returned the
/// unfiltered text unconditionally would pass, and the comparison would be
/// proved live by nothing.
#[tokio::test]
async fn a_generating_script_reads_the_value_publish_probed_it_with() {
    let f = skip_without_db!();
    publish_scripted(&f, "/shape", BRANCHES_ON_ITS_VALUE).await;

    let all = f.call_as(f.principal_a, "shape?region=ALL").await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);
    assert_eq!(
        f.port.last(),
        ("SELECT n FROM t".to_string(), Vec::<Param>::new()),
        "`params[\"region\"]` must be the string the caller sent, not a \
         one-element list of it — a list compares unequal to \"ALL\", the \
         script takes the filtered branch, and the endpoint answers 200 with \
         the wrong rows"
    );

    // The control: the comparison really is what chose the text above.
    let eu = f.call_as(f.principal_a, "shape?region=EU").await;
    assert_eq!(eu.status, StatusCode::OK, "{}", eu.body);
    assert_eq!(
        f.port.last(),
        (
            "SELECT n FROM t WHERE region IN (?)".to_string(),
            vec![Param::Str("EU".into())]
        ),
        "a value that is not \"ALL\" still takes the filtered branch, and the \
         loop still binds"
    );
}

/// The loud half of the same defect: a string method on an iterated parameter.
///
/// `params["region"].upper()` runs under publish's scalar probe and raises on a
/// list, so the endpoint published green and then answered 500 to EVERY
/// request — a defect no probe, no scan and no test on this branch could see,
/// because the two sides never compared the type they each hand the script.
#[tokio::test]
async fn a_generating_script_may_call_a_string_method_on_an_iterated_parameter() {
    let f = skip_without_db!();
    publish_scripted(
        &f,
        "/upper",
        "def build_sql(params):\n    \
         if params[\"region\"].upper() == \"ALL\":\n        \
         return \"SELECT n FROM t\"\n    \
         return \"SELECT n FROM t WHERE region IN ({% for r in region %}{{ r }}\
         {% if not loop.last %}, {% endif %}{% endfor %})\"\n",
    )
    .await;

    let lower = f.call_as(f.principal_a, "upper?region=all").await;
    assert_eq!(
        lower.status,
        StatusCode::OK,
        "a script publish proved runs must not raise on every real request: {}",
        lower.body
    );
    assert_eq!(
        f.port.last(),
        ("SELECT n FROM t".to_string(), Vec::<Param>::new()),
        "and it must reach the branch its own `.upper()` chose"
    );

    // The REPEATED half of the same question, and the cost of the answer.
    // `?region=eu&region=us` is a list to the renderer and one value to the
    // script: `.upper()` runs (so the script was handed a string) and the loop
    // still binds BOTH repeats (so `IN (?, ?, ?)` is untouched for a scripted
    // endpoint). Whether a caller repeated a parameter is decided by the
    // REQUEST and is knowable to no publish, so this is the half no frozen set
    // could ever have made agree.
    let repeated = f.call_as(f.principal_a, "upper?region=eu&region=us").await;
    assert_eq!(
        repeated.status,
        StatusCode::OK,
        "a repeat must not turn the script's parameter into a list either: {}",
        repeated.body
    );
    assert_eq!(
        f.port.last(),
        (
            "SELECT n FROM t WHERE region IN (?, ?)".to_string(),
            vec![Param::Str("eu".into()), Param::Str("us".into())]
        ),
        "the script sees the first value; the RENDER still sees every one of them"
    );
}
