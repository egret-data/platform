//! Spec §6.2 step 7's first half: a request plus a snapshot's `ParamSpec`s
//! become the values the renderer binds — and the bytes the cache keys on.
//!
//! # One resolution, two consumers, and why that is the whole design
//!
//! The cache key and the render must agree about what a request's parameters
//! ARE. If they can disagree, two requests that render different SQL can share a
//! cache entry, and the second one is served the first one's rows. That is not a
//! hypothetical: P4's `declared_query_params` matched `p.name == k` against the
//! **raw, undecoded** query key, so `?d%61y=2026-01-01` was dropped from the key
//! and shared an entry with a request carrying no `day` at all. It was inert only
//! because nothing rendered parameters yet; `serving/router.rs` recorded it as a
//! landmine for this step.
//!
//! So [`resolve`] returns BOTH halves from ONE pass: [`Resolved::values`] is what
//! the renderer sees, and [`Resolved::key`] is a projection of the *same*
//! decisions. The key is derived from the resolved [`Param`]s themselves, not
//! from the request text, which makes the agreement structural rather than a
//! convention two functions have to keep:
//!
//! * two requests that resolve to the same `Param`s render the same SQL and the
//!   same bind list, and get the same key — so sharing an entry is correct;
//! * two requests that resolve differently differ in at least one keyed field.
//!
//! `?day=x` and `?d%61y=x` are the first case. `?region=EU` and a request with
//! no `region` are the second: an absent parameter is **omitted** from the key
//! map, and [`super::cache::cache_key`] length-prefixes the map's size and every
//! name, so absence is representable and distinct from presence (spec §6).
//!
//! # Percent-decoding, and why `form_urlencoded`
//!
//! Names and values are both decoded, with `+` read as a space — the
//! `application/x-www-form-urlencoded` reading, which is what `axum`'s own
//! `Query` extractor (through `serde_urlencoded`) and every mainstream client
//! library produce. Decoding only values would close the visible half of the bug
//! above and leave the half that has no symptom until it has the worst one.
//!
//! # Errors never name the parameter
//!
//! Base spec §7, restated in the parameters spec §5. The name is the endpoint
//! author's, and naming it in a 400 turns the error surface into a directory of
//! which parameters an endpoint declares — the same oracle the single
//! `Unauthorized` and the single `NotFound` exist to close. So there are exactly
//! two answers here, [`ServingError::ParamMissing`] and
//! [`ServingError::ParamInvalid`], both carrying no payload at all: there is
//! nothing for a future edit to interpolate.

use std::collections::BTreeMap;

use egret_dataapi_api::{EndpointSnapshot, ParamLocation, ParamSpec, ParamType};
use egret_query_api::Param;

use super::ServingError;
use crate::render::ParamValues;

/// Cap on the total decoded bytes a request's DECLARED parameters may carry.
///
/// The renderer has its own 1 MiB bind-value cap, but a value that trips it
/// comes back as a `RenderError` — an author-facing 500 for what is a caller's
/// oversized input. Refusing here makes it the 400 it is, and it bounds the work
/// done before anything is rendered at all. Generous for a query string: a URL
/// is bounded by the server's header limits well below this.
const MAX_PARAM_BYTES: usize = 64 * 1024;

/// Cap on how many times one parameter may repeat.
///
/// A repeated parameter becomes an `IN (?, ?, …)` list, so this is the bound on
/// how large a caller can make the rendered statement. Placeholder count is
/// linear in it, and both engines have their own (higher, engine-specific)
/// limits — this one is egret's, and it is a 400 rather than an engine error.
const MAX_PARAM_REPEATS: usize = 1_000;

/// One request's parameters, resolved once and projected two ways.
pub(crate) struct Resolved {
    /// What [`crate::render::render`] binds.
    pub values: ParamValues,
    /// What [`super::cache::cache_key`] hashes. A declared parameter that
    /// resolved to no value at all is **absent from this map** — see the module
    /// comment.
    pub key: BTreeMap<String, Vec<String>>,
    /// Whether any declared parameter was supplied more than once. Read only by
    /// [`super::router`]'s render-failure mapping; see there for why.
    pub has_repeats: bool,
}

/// Resolve a request's query string against a snapshot's declared parameters.
///
/// Undeclared parameters are IGNORED, preserving P4 behaviour: a client that
/// appends tracking parameters keeps working, and — because they cannot affect
/// the response — they must not be able to affect the cache entry either.
///
/// `ParamLocation::Body` parameters are treated as not supplied. Nothing on this
/// surface parses a request body today (`serve_post` takes no body extractor),
/// so a `Body` parameter resolves through its `default`, or through `required`
/// to a 400. That is a gap, not a decision that body parameters are unsupported.
///
/// **Publish refuses `location: body` now** (`lifecycle::check_param_locations`),
/// because the OPTIONAL case is silent rather than loud: the parameter resolves
/// absent, its `{% if %}` block is omitted, and the endpoint serves unfiltered
/// rows with a 200 forever. This arm remains only for a snapshot published
/// before that rule — and such a snapshot is refused a layer up by the parameter
/// gate stamp anyway.
pub(crate) fn resolve(
    raw_query: Option<&str>,
    snap: &EndpointSnapshot,
) -> Result<Resolved, ServingError> {
    let supplied = decode(raw_query, snap)?;
    resolve_supplied(&supplied, &snap.sql, &snap.params)
}

/// [`resolve`], from an ALREADY-DECODED map rather than a raw query string.
///
/// Preview calls this directly. The alternative — re-encoding the author's test
/// values into a query string so they could go through [`resolve`] — is a lossy
/// round trip: a value containing `&` or `=` splits into two parameters, and a
/// preview would then disagree with the real call about the same value. Serving
/// still reaches it through [`resolve`], so there is exactly one place where a
/// declared parameter becomes a [`Param`].
pub(crate) fn resolve_supplied(
    supplied: &BTreeMap<String, Vec<String>>,
    sql: &str,
    specs: &[ParamSpec],
) -> Result<Resolved, ServingError> {
    // Which declared parameters this definition loops over. One scan of the
    // template per request, and only when it declares parameters at all — the
    // render that follows does strictly more work. It is the same function
    // publish uses to build its representative values, deliberately: the shape a
    // definition was validated against is the shape it is served.
    let iterated: Vec<String> = if specs.is_empty() {
        Vec::new()
    } else {
        crate::render::iterated_params(
            sql,
            &specs.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        )
    };

    let mut values = ParamValues::new();
    let mut key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut has_repeats = false;

    for spec in specs {
        let from_request = match spec.location {
            ParamLocation::Query => supplied.get(&spec.name),
            ParamLocation::Body => None,
        };
        // Whether the values below came from the CALLER or from the author's
        // `default`, which is the difference between a 400 and a 500 when one of
        // them fails to coerce. Publish refuses a `default` that does not satisfy
        // its own `ty` (`ValidationError::DefaultNotValidForType`), so a failure
        // on this branch is a broken snapshot, not a bad request — and answering
        // `param_invalid` blamed the caller for the author's mistake while
        // logging it under a code that points at nothing.
        let mut from_default = false;
        let texts: Vec<&str> = match (from_request, &spec.default) {
            (Some(v), _) if !v.is_empty() => v.iter().map(String::as_str).collect(),
            // The author's own default. Keyed by its resolved value like any
            // other, so `?region=EU` and an absent `region` defaulting to `EU`
            // share an entry — they render the same statement and bind the same
            // values, which is precisely when sharing is correct.
            (_, Some(d)) => {
                from_default = true;
                vec![d.as_str()]
            }
            (_, None) => {
                if spec.required {
                    return Err(ServingError::ParamMissing);
                }
                // Strict undefined: a declared-but-unsupplied name must be
                // `none` in the context, or `{% if region %}` ERRORS instead of
                // being false and every optional filter becomes a 500. See
                // `ParamValues`' own comment.
                values.insert_absent(spec.name.clone());
                continue;
            }
        };

        if texts.len() > 1 {
            has_repeats = true;
            // An enum parameter is a single choice — it is the only type that
            // can reach an identifier position through `| ident`, and a list of
            // identifiers is not a thing any statement wants. Refusing here also
            // keeps `ParamValues::insert_enum` the only way an enum enters a
            // render, which is what makes `ident`'s "must be an enum parameter"
            // rule structural.
            if spec.ty == ParamType::Enum {
                return Err(ServingError::ParamInvalid);
            }
        }

        let mut params = Vec::with_capacity(texts.len());
        for t in texts {
            params.push(coerce(spec, t).map_err(|e| match (e, from_default) {
                (CoerceError::Invalid, false) => ServingError::ParamInvalid,
                (CoerceError::Invalid, true) => ServingError::internal(
                    "a published `default` does not satisfy its own declared type",
                ),
                (CoerceError::Malformed, _) => {
                    ServingError::internal("enum parameter published with no `allowed` list")
                }
            })?);
        }

        key.insert(
            spec.name.clone(),
            params.iter().map(key_text).collect::<Vec<_>>(),
        );

        match params.len() {
            // One value stays a scalar so `{{ region }}` is one bind. More than
            // one becomes a list, which is what makes
            // `IN ({% for r in regions %}{{ r }}…{% endfor %})` render
            // `IN (?, ?, ?)` — the `IN (?)`-binds-one-value problem falls out of
            // the formatter and needs no construct of its own (spec §4.3).
            //
            // **Unless the definition ITERATES it**, in which case one value is
            // still a list of one. Arity is the caller's, and shape is the
            // definition's: a `{% for r in regions %}` over a scalar is not a
            // one-element loop, it is "plain is not iterable" — a 500 on the
            // perfectly ordinary request `?regions=EU`. Publish infers the same
            // set from the same function, so the shape a definition is validated
            // against and the shape it is served cannot disagree.
            1 if !iterated.iter().any(|n| n == &spec.name) => {
                insert_one(&mut values, spec, params.pop().expect("len == 1"))
            }
            _ => values.insert_list(spec.name.clone(), params),
        }
    }

    Ok(Resolved {
        values,
        key,
        has_repeats,
    })
}

/// Insert a single resolved value, attaching `allowed` for an enum parameter.
fn insert_one(values: &mut ParamValues, spec: &ParamSpec, param: Param) {
    match (&spec.ty, &spec.allowed, &param) {
        (ParamType::Enum, Some(allowed), Param::Str(s)) => {
            values.insert_enum(spec.name.clone(), s.clone(), allowed.clone())
        }
        _ => values.insert_for_spec(spec, param),
    }
}

/// The query string as decoded, declared parameters only.
///
/// Percent-decoded and `+`-decoded on BOTH halves before the name is matched
/// against `ParamSpec.name` — the correction this module exists for.
fn decode(
    raw: Option<&str>,
    snap: &EndpointSnapshot,
) -> Result<BTreeMap<String, Vec<String>>, ServingError> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if snap.params.is_empty() {
        return Ok(out);
    }
    let mut budget: usize = 0;
    for (k, v) in url::form_urlencoded::parse(raw.unwrap_or("").as_bytes()) {
        if !snap
            .params
            .iter()
            .any(|p| p.location == ParamLocation::Query && p.name == k)
        {
            continue;
        }
        budget = budget.saturating_add(k.len()).saturating_add(v.len());
        if budget > MAX_PARAM_BYTES {
            return Err(ServingError::ParamInvalid);
        }
        let slot = out.entry(k.into_owned()).or_default();
        if slot.len() >= MAX_PARAM_REPEATS {
            return Err(ServingError::ParamInvalid);
        }
        slot.push(v.into_owned());
    }
    Ok(out)
}

/// One caller string, as the typed value the engine will bind.
///
/// Typed, not stringly: Trino refuses `day >= VARCHAR '2020-01-01'` and a
/// decimal bound as a float stops comparing equal to itself, which is the reason
/// [`Param::Date`], [`Param::Timestamp`] and [`Param::Decimal`] exist at all
/// (Tasks 1-3). A `String`-typed parameter binds as text and is checked for
/// nothing beyond the size caps — it does not need to be, because it can only
/// ever become a bound value.
/// Why [`coerce`] refused.
///
/// Two answers, because they are two different people's mistakes: `Invalid` is
/// the caller's text, `Malformed` is the published definition. The caller-facing
/// mapping is the caller's alone — see [`resolve`], which is also where the
/// author's own `default` failing is turned into a 500 rather than a 400.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CoerceError {
    /// The text does not satisfy the declared `ty`.
    Invalid,
    /// The `ParamSpec` itself cannot be honoured — an `Enum` with no `allowed`.
    Malformed,
}

/// **Shared with publish**, which is the entire point.
///
/// `lifecycle` validates an author's `default` by running it through this exact
/// function (spec §5, T7-6). A second copy written for publish would be free to
/// accept something the serving path rejects, and the failure mode of that
/// disagreement is the one this check exists to remove: a definition that
/// publishes green and then answers 400 on every call that omits the parameter.
pub(crate) fn coerce(spec: &ParamSpec, text: &str) -> Result<Param, CoerceError> {
    match spec.ty {
        ParamType::String => Ok(Param::Str(text.to_string())),
        ParamType::Enum => {
            let allowed = spec
                .allowed
                .as_ref()
                .filter(|a| !a.is_empty())
                // An enum with no `allowed` is an author mistake publish
                // refuses (spec §5). Not the caller's fault, so not a 400.
                .ok_or(CoerceError::Malformed)?;
            // The AUTHOR's entry is what is kept, never the caller's bytes —
            // the same provenance rule `ident_filter` enforces. Two equal
            // strings, and the one that survives is always the author's.
            allowed
                .iter()
                .find(|a| a.as_str() == text)
                .map(|a| Param::Str(a.clone()))
                .ok_or(CoerceError::Invalid)
        }
        ParamType::Boolean => match text.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Param::Bool(true)),
            "false" | "0" => Ok(Param::Bool(false)),
            _ => Err(CoerceError::Invalid),
        },
        ParamType::Number => {
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Param::I64(i));
            }
            // Exact text, never an `f64`: a decimal that round-trips through a
            // float stops comparing equal to itself on the engine. The grammar
            // is deliberately narrow — no exponent, no `NaN`, no `inf` — so the
            // value is a literal both engines accept as a DECIMAL.
            if is_plain_decimal(text) {
                return Ok(Param::Decimal(text.to_string()));
            }
            Err(CoerceError::Invalid)
        }
        // Parsed and re-formatted rather than passed through, so ONE spelling
        // reaches the engine. chrono accepts an unpadded `2026-1-2`; it comes
        // back as `2026-01-02`, which is also why the two spellings share a
        // cache entry — they resolve to the same `Param`, so they render the
        // same statement and bind the same value. A value chrono cannot read at
        // all never reaches the engine to be interpreted (or not) by whatever
        // that engine's own parser happens to accept.
        ParamType::Date => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map(|d| Param::Date(d.format("%Y-%m-%d").to_string()))
            .map_err(|_| CoerceError::Invalid),
        ParamType::Timestamp => ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
            .iter()
            .find_map(|f| chrono::NaiveDateTime::parse_from_str(text, f).ok())
            // One spelling reaches the engine whichever the caller used: `%.f`
            // prints nothing when the fraction is zero, so a plain second-
            // resolution timestamp stays plain.
            .map(|t| Param::Timestamp(t.format("%Y-%m-%d %H:%M:%S%.f").to_string()))
            .ok_or(CoerceError::Invalid),
    }
}

/// `-?digits[.digits]`, and nothing else.
fn is_plain_decimal(s: &str) -> bool {
    let body = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('+'))
        .unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// One resolved value as the bytes the cache key hashes.
///
/// Derived from the [`Param`], not from the request text, which is what makes
/// the key agree with the render by construction: two requests whose parameters
/// resolve to equal `Param`s bind equal values and render equal SQL, so they
/// SHOULD share an entry; two that resolve differently differ here.
///
/// The one-byte type tag is not decoration. `Param::Str("1")` and
/// `Param::I64(1)` bind differently on both engines, and while a single
/// `ParamSpec.ty` cannot produce both today, a key that could not tell them
/// apart would be one type-widening away from serving one as the other.
fn key_text(p: &Param) -> String {
    match p {
        Param::Null => "0:".to_string(),
        Param::Bool(b) => format!("b:{b}"),
        Param::I64(i) => format!("i:{i}"),
        Param::F64(f) => format!("f:{f}"),
        Param::Decimal(s) => format!("n:{s}"),
        Param::Str(s) => format!("s:{s}"),
        Param::Date(s) => format!("d:{s}"),
        Param::Timestamp(s) => format!("t:{s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_dataapi_api::LimitsSpec;
    use uuid::Uuid;

    fn spec(name: &str, ty: ParamType, required: bool) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            ty,
            location: ParamLocation::Query,
            required,
            default: None,
            allowed: None,
            description: String::new(),
        }
    }

    fn snap(sql: &str, params: Vec<ParamSpec>) -> EndpointSnapshot {
        EndpointSnapshot {
            sql: sql.into(),
            method: "GET".into(),
            path: "/x".into(),
            database_id: Uuid::nil(),
            params,
            output: serde_json::from_value(serde_json::json!({})).unwrap(),
            cache: serde_json::from_value(serde_json::json!({})).unwrap(),
            limits: LimitsSpec {
                max_rows: 100,
                timeout_seconds: 10,
            },
            rate_limit: serde_json::from_value(serde_json::json!({})).unwrap(),
            delegated: false,
            impersonate: false,
            columns: vec![],
            param_gate: egret_dataapi_api::PARAM_GATE,
        }
    }

    const SQL: &str = "SELECT 1 AS n WHERE 1=1 {% if day %} AND day = {{ day }} {% endif %}";

    fn rendered(query: &str, s: &EndpointSnapshot) -> (String, Vec<Param>) {
        let r = resolve(Some(query), s).expect("resolve");
        crate::render::render(&s.sql, &r.values).expect("render")
    }

    /// **Step 2's property, at the level where it is decided.** The percent-
    /// encoded NAME must reach the same parameter, or the two requests render
    /// different SQL — and, sharing a key, the second is served the first's rows.
    #[test]
    fn a_percent_encoded_name_resolves_to_the_same_parameter() {
        let s = snap(SQL, vec![spec("day", ParamType::String, false)]);
        let plain = resolve(Some("day=x"), &s).expect("resolve plain");
        let encoded = resolve(Some("d%61y=x"), &s).expect("resolve encoded");
        assert_eq!(
            plain.key, encoded.key,
            "the cache key must not distinguish two spellings of one name"
        );
        assert_eq!(rendered("day=x", &s), rendered("d%61y=x", &s));
    }

    /// Preview hands `resolve_supplied` an already-decoded map. Re-encoding it
    /// into a query string so it could go through `resolve` would be a lossy
    /// round trip: a value containing `&` or `=` would split into two
    /// parameters, so a preview and a real call would disagree about the same
    /// value. This is the assertion that preview does not take that route.
    #[test]
    fn resolve_supplied_takes_values_containing_query_separators() {
        let spec = ParamSpec {
            name: "q".into(),
            ty: ParamType::String,
            location: ParamLocation::Query,
            required: false,
            default: None,
            allowed: None,
            description: String::new(),
        };
        let mut supplied: BTreeMap<String, Vec<String>> = BTreeMap::new();
        supplied.insert("q".into(), vec!["a&b=c".into()]);
        let out = resolve_supplied(&supplied, "SELECT {{ q }}", std::slice::from_ref(&spec))
            .expect("resolves");
        // `key` holds `key_text`'s type-tagged form, so the tag is part of the
        // expected string — the property under test is that the separators
        // survived, not how the key spells a `Str`.
        assert_eq!(
            out.key.get("q").map(Vec::as_slice),
            Some(&["s:a&b=c".to_string()][..])
        );
        // And the same value reaches the engine as one bind rather than two.
        let (_, binds) = crate::render::render("SELECT {{ q }}", &out.values).expect("render");
        assert_eq!(binds, vec![Param::Str("a&b=c".into())]);
    }

    /// Values are decoded too — `+` as a space, per `form_urlencoded`.
    #[test]
    fn values_are_decoded_before_they_are_bound() {
        let s = snap(SQL, vec![spec("day", ParamType::String, false)]);
        let (_, binds) = rendered("day=a%20b%2Bc", &s);
        assert_eq!(binds, vec![Param::Str("a b+c".into())]);
        let (_, binds) = rendered("day=a+b", &s);
        assert_eq!(binds, vec![Param::Str("a b".into())]);
    }

    /// **Step 4's property, at the level where it is decided.** Present and
    /// absent render different SQL, so they must not resolve to the same key.
    #[test]
    fn an_absent_parameter_is_distinct_from_a_supplied_one() {
        let s = snap(SQL, vec![spec("day", ParamType::String, false)]);
        let present = resolve(Some("day=x"), &s).expect("resolve");
        let absent = resolve(Some(""), &s).expect("resolve");
        assert_ne!(present.key, absent.key);
        assert!(absent.key.is_empty(), "an absent parameter is omitted");

        // And the reason it matters: the two DO render differently.
        let a = crate::render::render(&s.sql, &present.values).expect("render");
        let b = crate::render::render(&s.sql, &absent.values).expect("render");
        assert_ne!(a, b);
        assert!(b.1.is_empty(), "an absent parameter binds nothing");
    }

    /// The invariant the whole module is shaped around: equal keys imply equal
    /// renders. Written over a spread of spellings, including the two the P4
    /// comment named, so a future edit to either projection has to keep them
    /// agreeing rather than only keep its own half correct.
    #[test]
    fn equal_keys_imply_equal_renders() {
        let s = snap(SQL, vec![spec("day", ParamType::String, false)]);
        let queries = [
            "",
            "day=x",
            "d%61y=x",
            "day=",
            "day=y",
            "other=1",
            "day=x&other=2",
            "day=a+b",
            "day=a%20b",
        ];
        for a in queries {
            for b in queries {
                let ra = resolve(Some(a), &s).expect(a);
                let rb = resolve(Some(b), &s).expect(b);
                if ra.key != rb.key {
                    continue;
                }
                assert_eq!(
                    crate::render::render(&s.sql, &ra.values).expect("render a"),
                    crate::render::render(&s.sql, &rb.values).expect("render b"),
                    "`?{a}` and `?{b}` share a cache key but render differently"
                );
            }
        }
    }

    /// A default is the author's value and is keyed like any other resolved
    /// value, so it shares the entry with a caller who spelled it out.
    #[test]
    fn a_default_keys_as_the_value_it_resolves_to() {
        let mut p = spec("day", ParamType::String, false);
        p.default = Some("x".into());
        let s = snap(SQL, vec![p]);
        assert_eq!(
            resolve(Some(""), &s).expect("absent").key,
            resolve(Some("day=x"), &s).expect("supplied").key
        );
    }

    #[test]
    fn a_missing_required_parameter_is_refused_before_anything_renders() {
        let s = snap(SQL, vec![spec("day", ParamType::String, true)]);
        assert!(matches!(
            resolve(Some(""), &s),
            Err(ServingError::ParamMissing)
        ));
        assert!(resolve(Some("day=x"), &s).is_ok());
    }

    /// Each type's grammar, in both directions. The rejections matter more than
    /// the acceptances: a value that fails here is a 400, and a value that
    /// wrongly passes is a typed bind the engine has to make sense of.
    #[test]
    fn values_are_checked_against_the_declared_type() {
        let cases: Vec<(ParamType, &str, Option<Param>)> = vec![
            (ParamType::Number, "42", Some(Param::I64(42))),
            (ParamType::Number, "-42", Some(Param::I64(-42))),
            (
                ParamType::Number,
                "1234567.891",
                Some(Param::Decimal("1234567.891".into())),
            ),
            (ParamType::Number, "1e5", None),
            (ParamType::Number, "NaN", None),
            (ParamType::Number, "", None),
            (ParamType::Number, "1.2.3", None),
            (ParamType::Boolean, "true", Some(Param::Bool(true))),
            (ParamType::Boolean, "FALSE", Some(Param::Bool(false))),
            (ParamType::Boolean, "yes", None),
            (
                ParamType::Date,
                "2026-01-02",
                Some(Param::Date("2026-01-02".into())),
            ),
            // Accepted and NORMALISED, not passed through. Both spellings bind
            // the same value and therefore share a cache entry.
            (
                ParamType::Date,
                "2026-1-2",
                Some(Param::Date("2026-01-02".into())),
            ),
            (ParamType::Date, "2026-02-30", None),
            (ParamType::Date, "2026-01-02 00:00:00", None),
            (
                ParamType::Timestamp,
                "2026-01-02 03:04:05",
                Some(Param::Timestamp("2026-01-02 03:04:05".into())),
            ),
            (
                // Both separators land on ONE spelling.
                ParamType::Timestamp,
                "2026-01-02T03:04:05",
                Some(Param::Timestamp("2026-01-02 03:04:05".into())),
            ),
            (
                ParamType::Timestamp,
                "2026-01-02 03:04:05.250",
                Some(Param::Timestamp("2026-01-02 03:04:05.250".into())),
            ),
            (ParamType::Timestamp, "2026-01-02", None),
            (
                ParamType::String,
                "'; DROP TABLE t--",
                Some(Param::Str("'; DROP TABLE t--".into())),
            ),
        ];
        for (ty, text, want) in cases {
            let s = spec("p", ty.clone(), false);
            match (coerce(&s, text), want) {
                (Ok(got), Some(want)) => assert_eq!(got, want, "{ty:?} {text:?}"),
                (Err(CoerceError::Invalid), None) => {}
                (got, want) => panic!("{ty:?} {text:?}: got {got:?}, wanted {want:?}"),
            }
        }
    }

    /// `ident`'s provenance rule, applied one layer earlier: the string kept is
    /// the author's entry, and a value outside the list is a 400.
    #[test]
    fn an_enum_keeps_the_authors_string_and_refuses_anything_else() {
        let mut p = spec("sort", ParamType::Enum, false);
        p.allowed = Some(vec!["day".into(), "region".into()]);
        assert_eq!(
            coerce(&p, "day").expect("allowed"),
            Param::Str("day".into())
        );
        assert!(matches!(coerce(&p, "amount"), Err(CoerceError::Invalid)));
    }

    /// Repeats become an `IN`-list; an enum repeat is refused because an enum is
    /// a single choice and is the only type that can reach `| ident`.
    #[test]
    fn repeats_become_a_list_except_for_an_enum() {
        let s = snap(
            "SELECT 1 AS n WHERE r IN ({% for x in r %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})",
            vec![spec("r", ParamType::String, false)],
        );
        let (sql, binds) = rendered("r=EU&r=US&r=APAC", &s);
        assert!(sql.contains("IN (?,?,?)"), "{sql}");
        assert_eq!(binds.len(), 3);

        let mut p = spec("r", ParamType::Enum, false);
        p.allowed = Some(vec!["EU".into(), "US".into()]);
        let s = snap(SQL, vec![p]);
        assert!(matches!(
            resolve(Some("r=EU&r=US"), &s),
            Err(ServingError::ParamInvalid)
        ));
    }

    /// An undeclared parameter cannot affect the response, so it must not be
    /// able to affect the entry either — P4's key-space bound, preserved.
    #[test]
    fn an_undeclared_parameter_changes_nothing() {
        let s = snap(SQL, vec![spec("day", ParamType::String, false)]);
        assert_eq!(
            resolve(Some("day=x"), &s).expect("a").key,
            resolve(Some("day=x&junk=1&junk=2"), &s).expect("b").key
        );
    }

    /// The caps are 400s, not renderer errors: an oversized caller value is the
    /// caller's problem and must not surface as an author-facing 500.
    #[test]
    fn oversized_input_is_refused_as_bad_input() {
        let s = snap(SQL, vec![spec("day", ParamType::String, false)]);
        let big = "x".repeat(MAX_PARAM_BYTES + 1);
        assert!(matches!(
            resolve(Some(&format!("day={big}")), &s),
            Err(ServingError::ParamInvalid)
        ));
        let many = (0..MAX_PARAM_REPEATS + 1)
            .map(|_| "day=1")
            .collect::<Vec<_>>()
            .join("&");
        assert!(matches!(
            resolve(Some(&many), &s),
            Err(ServingError::ParamInvalid)
        ));
    }
}
