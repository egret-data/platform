//! P4.5 Task 6 — spec §6.2 step 7 through the serving surface.
//!
//! **The one property this file exists for**: two calls to the same endpoint
//! with different parameter values must return different rows. Until step 7
//! landed they returned the same rows, because `snap.sql` — the template — went
//! to the engine verbatim and no caller input reached it at all. That is the
//! whole reason P4.5 exists, and every other test here is about keeping this one
//! from passing for the wrong reason.
//!
//! # What stops it passing vacuously
//!
//! * The fake engine's rows are `marker(principal) + bind_signature(params)`
//!   (`serving_common`). If the serving path stopped passing values through — an
//!   empty slice, a call to `execute_as` instead, `snap.sql` unrendered — the
//!   signature collapses to zero and every "these two differ" assertion here
//!   fails.
//! * `FakePort::last()` records the statement the port was asked to run, so the
//!   suite asserts on the RENDERED SQL as well as on the rows: bind markers
//!   present, template delimiters absent.
//! * The percent-decoding and cache-key tests each have a named mutation in the
//!   task report, and each was observed failing under it.
//!
//! DB-gated; self-skips when `DATABASE_URL` is unset — check the DURATION, not
//! just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use egret_query_api::Param;
use serving_common::{bind_signature, fixture, marker, Fixture};

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

fn per_principal_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" })
}

fn no_limits() -> serde_json::Value {
    serde_json::json!({})
}

/// The spec §4.3 shape: an optional filter behind an `{% if %}`.
///
/// P3 refuses to publish a parameter no placeholder references, so the SQL and
/// the declaration move together.
const OPTIONAL_REGION: &str =
    "SELECT 1 AS n WHERE 1=1 {% if region %} AND region = {{ region }} {% endif %}";

fn one_param(name: &str, ty: &str) -> serde_json::Value {
    serde_json::json!([{ "name": name, "type": ty, "location": "query" }])
}

async fn publish(f: &Fixture, path: &str, sql: &str, params: serde_json::Value) {
    f.publish_with_params(path, f.database_id, no_cache(), no_limits(), sql, params)
        .await;
}

// ---------------------------------------------------------------------------
// The property the phase exists for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_values_of_one_parameter_return_different_rows() {
    // Spec §9's live check, offline: call the same endpoint twice with two
    // values and confirm the results ACTUALLY DIFFER. This is precisely what did
    // not work before step 7 — `?region=EU` and `?region=US` returned the same
    // rows because neither value ever reached the statement.
    let f = skip_without_db!();
    publish(&f, "/r", OPTIONAL_REGION, one_param("region", "string")).await;

    let eu = f.call_as(f.principal_a, "r?region=EU").await;
    let us = f.call_as(f.principal_a, "r?region=US").await;
    assert_eq!(eu.status, StatusCode::OK, "{}", eu.body);
    assert_eq!(us.status, StatusCode::OK, "{}", us.body);
    assert_ne!(
        eu.ns(),
        us.ns(),
        "two values of a parameter produced identical rows — nothing was bound"
    );

    // And the value that reached the engine is the caller's, bound, not
    // interpolated: one marker in the statement, one value beside it.
    let (sql, binds) = f.port.last();
    assert_eq!(binds, vec![Param::Str("US".into())]);
    assert_eq!(sql.matches('?').count(), 1, "{sql}");
    assert!(
        !sql.contains("{{") && !sql.contains("{%"),
        "the TEMPLATE reached the engine, not the rendered statement: {sql}"
    );
    assert!(sql.contains("AND region = ?"), "{sql}");
}

#[tokio::test]
async fn an_absent_optional_parameter_omits_its_block_rather_than_binding_null() {
    // Spec §9's second live check. `UndefinedBehavior::Strict` makes an
    // undefined name an error even when merely tested for truth, so a
    // declared-but-unsupplied parameter has to be inserted as `none`
    // (`insert_absent`) — otherwise `{% if region %}` ERRORS and every optional
    // filter on every endpoint becomes a 500.
    let f = skip_without_db!();
    publish(&f, "/o", OPTIONAL_REGION, one_param("region", "string")).await;

    let res = f.call_as(f.principal_a, "o").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let (sql, binds) = f.port.last();
    assert!(binds.is_empty(), "an absent parameter bound {binds:?}");
    assert!(
        !sql.contains('?') && !sql.contains("region ="),
        "the `{{% if %}}` block was emitted for an absent parameter: {sql}"
    );
    assert_eq!(
        res.ns(),
        vec![marker(f.principal_a)],
        "no binds means no signature"
    );
}

#[tokio::test]
async fn a_hostile_value_is_bound_as_data_and_never_becomes_sql() {
    // Not "there was no error" — a well-formed injected scalar parses fine and
    // never surfaces as a syntax error. The assertion is that the bytes arrive
    // as a VALUE, whole, with the statement's shape untouched.
    let f = skip_without_db!();
    publish(&f, "/h", OPTIONAL_REGION, one_param("region", "string")).await;

    let hostile = "EU' OR 1=1--";
    let res = f
        .call_as(f.principal_a, "h?region=EU%27%20OR%201%3D1--")
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let (sql, binds) = f.port.last();
    assert_eq!(binds, vec![Param::Str(hostile.into())]);
    assert!(
        !sql.contains("OR 1=1"),
        "caller text reached the SQL: {sql}"
    );
    assert_eq!(sql.matches('?').count(), 1, "{sql}");
}

#[tokio::test]
async fn a_repeated_parameter_becomes_one_bind_marker_per_value() {
    // The `IN (?)`-binds-one-value problem, which falls out of the formatter and
    // needs no construct of its own (spec §4.3).
    //
    // **The loop variable is named after the parameter on purpose, and it is a
    // workaround for a known open item, not a style choice.** P3's
    // `lifecycle::placeholders` treats the whole inside of `{{ }}` as a
    // parameter NAME, so spec §4.3's own example — `{% for r in regions %}{{ r }}`
    // — is refused at publish with "the query references `{{r}}` but no
    // parameter named `r` is declared". That is ledger item T7-3, and Task 7
    // owns reconciling the two scanners. Shadowing the parameter with the loop
    // target is what publishes today; if T7-3 lands, this test should go back to
    // the spec's spelling.
    let f = skip_without_db!();
    publish(
        &f,
        "/in",
        "SELECT 1 AS n WHERE region IN (\
         {% for region in region %}{{ region }}{% if not loop.last %},{% endif %}{% endfor %})",
        one_param("region", "string"),
    )
    .await;

    let res = f
        .call_as(f.principal_a, "in?region=EU&region=US&region=APAC")
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let (sql, binds) = f.port.last();
    assert_eq!(
        binds,
        vec![
            Param::Str("EU".into()),
            Param::Str("US".into()),
            Param::Str("APAC".into())
        ]
    );
    assert!(sql.contains("IN (?,?,?)"), "{sql}");
}

// ---------------------------------------------------------------------------
// Step 2 — percent-decoded NAMES.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_percent_encoded_parameter_name_reaches_the_same_parameter() {
    // P4 matched `p.name == k` against the RAW query key, so `?d%61y=` did not
    // match a declared `day`: it was dropped, and the request shared a cache
    // entry with one carrying no `day` at all. Inert while nothing rendered;
    // a wrong-rows bug the moment step 7 landed. `serving/router.rs` recorded it
    // as a landmine and this is its disarming.
    //
    // Asserted on BOTH halves the brief names: the same rendered SQL and the
    // same bind list.
    let f = skip_without_db!();
    publish(
        &f,
        "/d",
        "SELECT 1 AS n WHERE 1=1 {% if day %} AND day = {{ day }} {% endif %}",
        one_param("day", "string"),
    )
    .await;

    let plain = f.call_as(f.principal_a, "d?day=x").await;
    let (plain_sql, plain_binds) = f.port.last();
    let encoded = f.call_as(f.principal_a, "d?d%61y=x").await;
    let (encoded_sql, encoded_binds) = f.port.last();

    assert_eq!(plain.status, StatusCode::OK, "{}", plain.body);
    assert_eq!(encoded.status, StatusCode::OK, "{}", encoded.body);
    assert_eq!(plain_sql, encoded_sql, "two spellings rendered differently");
    assert_eq!(plain_binds, vec![Param::Str("x".into())]);
    assert_eq!(
        encoded_binds, plain_binds,
        "two spellings bound differently"
    );
    assert_eq!(plain.ns(), encoded.ns());

    // Anti-triviality: the encoded spelling must not merely have been DROPPED
    // and landed on the absent path, which would also make the two agree if the
    // plain one were broken too.
    assert!(plain_sql.contains("AND day = ?"), "{plain_sql}");
}

// ---------------------------------------------------------------------------
// Step 3 — the 400s, and what they may not say.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_missing_required_parameter_is_a_400_that_names_nothing() {
    let f = skip_without_db!();
    f.publish_with_params(
        "/req",
        f.database_id,
        no_cache(),
        no_limits(),
        OPTIONAL_REGION,
        serde_json::json!([
            { "name": "region", "type": "string", "location": "query", "required": true }
        ]),
    )
    .await;

    let res = f.call_as(f.principal_a, "req").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    assert_eq!(res.body["error"]["code"], "param_missing");
    let message = res.body["error"]["message"].as_str().expect("message");
    // The body text, not just the status: the parameter's name is the endpoint
    // AUTHOR's, and naming it turns the error surface into a directory of what
    // the endpoint declares (base spec §7).
    assert_eq!(message, "a required parameter is missing");
    assert!(
        !message.contains("region"),
        "named the parameter: {message}"
    );
    assert!(
        !res.body.to_string().contains("region"),
        "the parameter name appears somewhere in the body: {}",
        res.body
    );

    // Supplying it clears the refusal, so the 400 is about the value and not
    // about the endpoint being broken.
    let ok = f.call_as(f.principal_a, "req?region=EU").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
}

#[tokio::test]
async fn a_value_that_fails_its_type_is_a_400_that_names_nothing() {
    let f = skip_without_db!();
    publish(
        &f,
        "/typed",
        "SELECT 1 AS n WHERE 1=1 {% if day %} AND day = {{ day }} {% endif %}",
        one_param("day", "date"),
    )
    .await;

    for bad in ["not-a-date", "2026-02-30", "2026-01-02%2003:04:05"] {
        let res = f.call_as(f.principal_a, &format!("typed?day={bad}")).await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{bad}: {}", res.body);
        assert_eq!(res.body["error"]["code"], "param_invalid", "{bad}");
        assert_eq!(
            res.body["error"]["message"],
            "a parameter value is not valid for this endpoint"
        );
        assert!(
            !res.body.to_string().contains("day") && !res.body.to_string().contains(bad),
            "{bad}: the body echoed the parameter or the value: {}",
            res.body
        );
    }

    // The type is not merely a filter: a good value binds as a DATE, not as
    // text. Trino refuses `day >= VARCHAR '2026-01-02'`, which is why Tasks 1-3
    // built the typed variants at all.
    let ok = f.call_as(f.principal_a, "typed?day=2026-01-02").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(f.port.last().1, vec![Param::Date("2026-01-02".into())]);
}

#[tokio::test]
async fn an_enum_value_outside_allowed_is_a_400() {
    let f = skip_without_db!();
    f.publish_with_params(
        "/enum",
        f.database_id,
        no_cache(),
        no_limits(),
        "SELECT 1 AS n WHERE 1=1 {% if sort %} AND s = {{ sort }} {% endif %}",
        serde_json::json!([{
            "name": "sort", "type": "enum", "location": "query",
            "allowed": ["day", "region"]
        }]),
    )
    .await;

    let res = f.call_as(f.principal_a, "enum?sort=amount").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    assert_eq!(res.body["error"]["code"], "param_invalid");
    // The refusal must not leak WHICH values are allowed — that is the endpoint
    // author's list, and it is the most directory-shaped thing on this surface.
    for allowed in ["day", "region"] {
        assert!(
            !res.body.to_string().contains(allowed),
            "the body listed an allowed value: {}",
            res.body
        );
    }

    let ok = f.call_as(f.principal_a, "enum?sort=day").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    // The AUTHOR's string is what is kept, not the caller's bytes.
    assert_eq!(f.port.last().1, vec![Param::Str("day".into())]);
}

#[tokio::test]
async fn repeating_a_scalar_parameter_is_a_400_and_not_a_500() {
    // A repeat resolves to a LIST, and the formatter refuses to bind a sequence
    // as one value. The endpoint is not broken — it serves every single-valued
    // call — so the caller's duplicate must not read as an egret fault.
    let f = skip_without_db!();
    publish(&f, "/rep", OPTIONAL_REGION, one_param("region", "string")).await;

    let res = f.call_as(f.principal_a, "rep?region=EU&region=US").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    assert_eq!(res.body["error"]["code"], "param_invalid", "{}", res.body);
}

// ---------------------------------------------------------------------------
// Step 4 — the cache key.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_present_and_an_absent_parameter_do_not_share_a_cache_entry() {
    // The brief's test, exactly: warm the cache with `?region=EU`, then call
    // with no `region`, and assert the second is a MISS returning different
    // rows. The two now execute different SQL, so sharing an entry serves one
    // request the other's rows.
    let f = skip_without_db!();
    f.publish_with_params(
        "/k",
        f.database_id,
        per_principal_cache(),
        no_limits(),
        OPTIONAL_REGION,
        one_param("region", "string"),
    )
    .await;

    let before = f.port.executions();
    let warm = f.call_as(f.principal_a, "k?region=EU").await;
    assert_eq!(warm.status, StatusCode::OK, "{}", warm.body);
    assert!(!warm.cached());

    let absent = f.call_as(f.principal_a, "k").await;
    assert_eq!(absent.status, StatusCode::OK, "{}", absent.body);
    assert!(
        !absent.cached(),
        "a call with no `region` hit the entry warmed by `?region=EU`: {}",
        absent.body
    );
    assert_eq!(
        f.port.executions(),
        before + 2,
        "presence and absence must be two entries and two executions"
    );
    assert_ne!(
        warm.ns(),
        absent.ns(),
        "…and the rows they produce really do differ"
    );
    // The absent call bound nothing, which is what made its rows differ.
    assert_eq!(absent.ns(), vec![marker(f.principal_a)]);
    assert_eq!(
        warm.ns(),
        vec![marker(f.principal_a).wrapping_add(bind_signature(&[Param::Str("EU".into())]))]
    );

    // Repeating either one still hits its OWN entry — the key distinguishes
    // them without becoming a cache that never hits.
    assert!(f.call_as(f.principal_a, "k?region=EU").await.cached());
    assert!(f.call_as(f.principal_a, "k").await.cached());
    assert_eq!(f.port.executions(), before + 2);
}

#[tokio::test]
async fn two_spellings_of_one_name_share_one_cache_entry() {
    // The other half of the percent-decoding fix, on the cache rather than on
    // the render. Two spellings of one request must not be two entries: that
    // costs an execution, and — worse — it is the symptom that would remain if
    // only the render were fixed.
    let f = skip_without_db!();
    f.publish_with_params(
        "/ks",
        f.database_id,
        per_principal_cache(),
        no_limits(),
        "SELECT 1 AS n WHERE 1=1 {% if day %} AND day = {{ day }} {% endif %}",
        one_param("day", "string"),
    )
    .await;

    let before = f.port.executions();
    assert!(!f.call_as(f.principal_a, "ks?day=x").await.cached());
    let encoded = f.call_as(f.principal_a, "ks?d%61y=x").await;
    assert!(
        encoded.cached(),
        "`?d%61y=x` missed the entry `?day=x` warmed: {}",
        encoded.body
    );
    assert_eq!(f.port.executions(), before + 1);
}

#[tokio::test]
async fn a_default_shares_the_entry_with_the_value_spelled_out() {
    // A default is the author's value. Absent-with-a-default and
    // supplied-with-that-value render the same statement and bind the same
    // values, which is exactly when sharing an entry is correct.
    let f = skip_without_db!();
    f.publish_with_params(
        "/def",
        f.database_id,
        per_principal_cache(),
        no_limits(),
        OPTIONAL_REGION,
        serde_json::json!([{
            "name": "region", "type": "string", "location": "query", "default": "EU"
        }]),
    )
    .await;

    let before = f.port.executions();
    let defaulted = f.call_as(f.principal_a, "def").await;
    assert_eq!(defaulted.status, StatusCode::OK, "{}", defaulted.body);
    assert_eq!(f.port.last().1, vec![Param::Str("EU".into())]);

    let spelled = f.call_as(f.principal_a, "def?region=EU").await;
    assert!(spelled.cached(), "{}", spelled.body);
    assert_eq!(f.port.executions(), before + 1);

    // And a DIFFERENT value is still its own entry.
    assert!(!f.call_as(f.principal_a, "def?region=US").await.cached());
}
