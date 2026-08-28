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
    /// Whether any declared parameter was supplied more than once. Read by
    /// [`super::router`]'s render-failure mapping — see there for why — and,
    /// since C2, by its transform-failure log line, because a repeat is the one
    /// way a parameter reaches a script in a shape publish's probe never showed
    /// the author.
    pub has_repeats: bool,
    /// The same resolved parameters as JSON — what a response transform's
    /// `params` dict carries (`crate::transform::run_transform`).
    ///
    /// A THIRD projection of the one resolution rather than a second
    /// resolution, and built in the same loop from the same `as_list` decision
    /// as [`Resolved::values`], so the script and the SQL cannot disagree about
    /// whether `region` was one value or a list. A declared parameter that
    /// resolved to no value at all is present here as `null` — Starlark `None` —
    /// rather than absent, matching `ParamValues::insert_absent`'s "strict
    /// undefined is `none`, not a missing name": a script asking
    /// `params["region"] == None` gets an answer instead of a KeyError, and the
    /// dict always carries exactly the endpoint's declared names.
    pub json: BTreeMap<String, serde_json::Value>,
    /// The same resolved parameters again, in the ONE shape a `build_sql`
    /// script is ever handed — see [`crate::transform::GenerationParams`].
    ///
    /// A fourth projection rather than a reuse of [`Resolved::json`], and the
    /// difference is the point. `json` follows `as_list` below, which is right
    /// for a response transform: publish probes a transform AFTER the SQL
    /// exists, so `lifecycle::probe_transform` wraps exactly the same set and
    /// the two agree. Generation runs BEFORE any SQL exists, so publish has no
    /// `iterated` to follow and hands the script scalars — and a serving dict
    /// that followed `as_list` would hand it a list for the same parameter.
    /// That disagreement was package S's one Critical: a silent wrong branch.
    ///
    /// So this one never carries a list, and cannot: its builder takes at most
    /// one [`Param`] per name. A parameter supplied several times reaches the
    /// script as its FIRST value and reaches the RENDER as all of them, which
    /// is what keeps `IN (?, ?, ?)` working for a scripted endpoint.
    pub generation: crate::transform::GenerationParams,
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
    // Every statement, not just the first one: one request's
    // parameters are shared by all of them (spec §4.2), and `resolve_supplied`
    // is where the union over `sqls` is computed — see its own comment.
    if snap.statements.is_empty() {
        return Err(ServingError::internal("snapshot carries no statements"));
    }
    // The shape decision. Package S Task 6 moved it: publish freezes the set it
    // decided across every template of every statement, and this reads it —
    // through `resolve_supplied_with`, the same entry point preview uses, for
    // the same reason both of them cannot compute it here.
    //
    // **A scripted statement cannot be scanned for it.** Its SQL is generated
    // from the caller's parameters, and this decision is what says whether
    // `?regions=EU` reaches the script (and the renderer) as a scalar or as a
    // one-element list — so the text does not exist until after the answer is
    // needed. See `EndpointSnapshot::iterated`.
    //
    // Empty means "published before the field existed", and it means "nothing
    // is iterated"; the fallback below answers the same set in both cases, which
    // is why one test can serve for both. A pre-S snapshot carries only written
    // statements, so scanning their text is exactly what this function did
    // before — byte for byte, through the same
    // `render::iterated_params_union`.
    if !snap.iterated.is_empty() {
        return resolve_supplied_with(&supplied, &snap.iterated, &snap.params);
    }
    let sqls: Vec<&str> = snap
        .statements
        .iter()
        .filter_map(|s| s.source.sql())
        .collect();
    resolve_supplied(&supplied, &sqls, &snap.params)
}

/// [`resolve`], from an ALREADY-DECODED map rather than a raw query string.
///
/// Preview calls this directly. The alternative — re-encoding the author's test
/// values into a query string so they could go through [`resolve`] — is a lossy
/// round trip: a value containing `&` or `=` splits into two parameters, and a
/// preview would then disagree with the real call about the same value. Serving
/// still reaches it through [`resolve`], so there is exactly one place where a
/// declared parameter becomes a [`Param`].
///
/// # `sqls` is the UNION source, not one statement
///
/// One request's parameters are shared by every statement an endpoint
/// declares (spec §4.2): a `rows` query may read `sort` and `limit` while a
/// `total` query alongside it reads neither. `sqls` carries every statement's
/// text so the shape a parameter takes — a plain value, or a list of one
/// because SOME statement iterates it in a `{% for %}` header — is decided
/// from the whole endpoint, not from whichever statement happened to be
/// first. Deciding it from only one statement is not merely incomplete: every
/// statement renders against this SAME [`ParamValues`] (Task 7), so a
/// parameter shaped as a plain value here because the statement that iterates
/// it went unread fails to render at all where it is actually used — a scalar
/// [`crate::render::render`] `Cell` is `ObjectRepr::Plain`, and `{% for %}`
/// over a `Plain` object is not iterable, so the render errors rather than
/// silently doing the wrong thing.
///
/// Preview does not call this one at all — it calls [`resolve_supplied_with`],
/// because its shape decision was already made (and already refused a draft
/// that fails it) by `lifecycle::validate_preview`, across the statement on
/// screen AND the siblings its request body now carries. It used to pass a
/// single-element `sqls` here, which was the one place the shape question was
/// still asked of one statement of several: previewing the `summary` of a
/// two-statement endpoint whose `by_region` loops over the same parameter
/// showed a green preview of a definition publish refuses. That is closed.
///
/// [`resolve`] hands this every statement's SQL but decides nothing about shape
/// itself, so serving has exactly one such decision.
///
/// # Publish makes the SAME decision, and now by the same route
///
/// Publish infers list-versus-scalar to build its representative values, and it
/// used to infer it from ONE statement's SQL at a time
/// ([`crate::render::iterated_params`], called per statement). That agrees with
/// the union only at N = 1: a `summary` validated as a scalar alongside a
/// `by_region` that loops the same parameter published green and then answered
/// 500 on every call, including the no-query-string default the generated
/// document advertises. Both sides now go through
/// [`crate::render::iterated_params_union`] — publish via
/// `lifecycle::IteratedParams`, whose constructor takes a slice of statements
/// exactly as this one does, so neither side can express "just this one
/// statement" of several.
pub(crate) fn resolve_supplied(
    supplied: &BTreeMap<String, Vec<String>>,
    sqls: &[&str],
    specs: &[ParamSpec],
) -> Result<Resolved, ServingError> {
    // Which declared parameters ANY statement loops over — the union across
    // `sqls`. One scan of each statement's template per request, and only when
    // parameters are declared at all — the render that follows does strictly
    // more work per statement anyway. It is the SAME FUNCTION publish uses, and
    // publish now asks it the same way: over every statement at once, through
    // `lifecycle::IteratedParams`. It used to ask per statement, which agrees
    // with this only at N = 1 — see the doc comment above.
    let iterated: Vec<String> = if specs.is_empty() {
        Vec::new()
    } else {
        let declared: Vec<String> = specs.iter().map(|p| p.name.clone()).collect();
        crate::render::iterated_params_union(sqls, &declared)
    };
    resolve_supplied_with(supplied, &iterated, specs)
}

/// [`resolve_supplied`] with the shape decision ALREADY made.
///
/// The serving path computes it from the statements it is about to run, which
/// is what [`resolve_supplied`] does. Preview cannot: the statement it runs is
/// one of several and the shape is a property of the endpoint, so the union is
/// computed once by [`crate::lifecycle::validate_preview`] — which is also the
/// function that refuses the draft when that shape does not render — and handed
/// here as a `lifecycle::IteratedParams`' names.
///
/// Split for that caller alone, and split rather than duplicated: preview
/// computing its own union would be the second answer the M2 fix removed. The
/// two entry points differ only in where the set comes from; everything below
/// this line is one implementation.
pub(crate) fn resolve_supplied_with(
    supplied: &BTreeMap<String, Vec<String>>,
    iterated: &[String],
    specs: &[ParamSpec],
) -> Result<Resolved, ServingError> {
    let mut values = ParamValues::new();
    let mut key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut json: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut generation = crate::transform::GenerationParams::default();
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
                // And `None` to a script, for the same reason one line up: the
                // name is DECLARED, so it is in the dict — a script reading
                // `params["region"]` gets `None` rather than a KeyError, which
                // is the same distinction the renderer draws.
                json.insert(spec.name.clone(), serde_json::Value::Null);
                // And `None` to a GENERATING script too, for the same reason —
                // through the one constructor that decides that dict's shape.
                generation.insert(spec.name.clone(), None);
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
                // The constraint and its bound are DELIBERATELY dropped here.
                // `serving/error.rs` keeps `ParamInvalid` a unit variant so a
                // 400 cannot become a directory of an endpoint's parameters;
                // the detail exists for `check_defaults`, which the author
                // reads about their own endpoint. Widening this is a decision,
                // not a tidy-up.
                (CoerceError::Constraint { .. }, false) => ServingError::ParamInvalid,
                (CoerceError::Constraint { .. }, true) => ServingError::internal(
                    "a published `default` does not satisfy its own declared constraints",
                ),
                (CoerceError::Malformed(MalformedReason::EnumHasNoAllowedList), _) => {
                    ServingError::internal("enum parameter published with no `allowed` list")
                }
                (CoerceError::Malformed(MalformedReason::PatternDoesNotCompile), _) => {
                    ServingError::internal(
                        "string parameter published with a `pattern` that does not compile",
                    )
                }
                (CoerceError::Malformed(MalformedReason::TemporalBoundDoesNotParse), _) => {
                    ServingError::internal(
                        "date/timestamp parameter published with a `not_before`/`not_after` that does not parse",
                    )
                }
            })?);
        }

        key.insert(
            spec.name.clone(),
            params.iter().map(key_text).collect::<Vec<_>>(),
        );

        // One value stays a scalar so `{{ region }}` is one bind. More than one
        // becomes a list, which is what makes
        // `IN ({% for r in regions %}{{ r }}…{% endfor %})` render
        // `IN (?, ?, ?)` — the `IN (?)`-binds-one-value problem falls out of the
        // formatter and needs no construct of its own (spec §4.3).
        //
        // **Unless the definition ITERATES it**, in which case one value is
        // still a list of one. Arity is the caller's, and shape is the
        // definition's: a `{% for r in regions %}` over a scalar is not a
        // one-element loop, it is "plain is not iterable" — a 500 on the
        // perfectly ordinary request `?regions=EU`. Publish infers the same set
        // from the same function, so the shape a definition is validated against
        // and the shape it is served cannot disagree.
        //
        // **Decided ONCE and read twice.** C2 gives a script the same
        // parameters, and a second `match params.len()` computing the same
        // answer for `json` is precisely the seam the transform design exists to
        // close: `{{ region }}` binding a scalar while `params["region"]` is a
        // one-element list is a disagreement no error names.
        // A GENERATING script is handed the first value and never a list,
        // whatever `as_list` decides one line down. Not an oversight and not a
        // second answer to the same question: publish generates before any SQL
        // exists, so there is no `iterated` set for it to follow, and a script
        // shown a scalar at publish that meets a list at serve is the silent
        // wrong branch described on `crate::transform::GenerationParams`. The
        // renderer's shape is unaffected — every repeat still binds.
        generation.insert(spec.name.clone(), params.first());
        let as_list = params.len() != 1 || iterated.iter().any(|n| n == &spec.name);
        json.insert(
            spec.name.clone(),
            if as_list {
                serde_json::Value::Array(params.iter().map(crate::transform::param_json).collect())
            } else {
                crate::transform::param_json(&params[0])
            },
        );
        if as_list {
            values.insert_list(spec.name.clone(), params);
        } else {
            insert_one(&mut values, spec, params.pop().expect("len == 1"));
        }
    }

    Ok(Resolved {
        values,
        key,
        has_repeats,
        json,
        generation,
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
/// Which way a [`CoerceError::Malformed`] spec is broken.
///
/// Both `ParamType::Enum` and `ParamType::String` can fail to be honourable,
/// for unrelated reasons on unrelated types — and a single `Malformed` unit
/// variant made them indistinguishable at the one place that logs the
/// message. Before this existed, a non-compiling `pattern` produced a 500
/// whose log line said "enum parameter published with no `allowed` list",
/// because that was the only message `Malformed` had ever needed. Wire
/// response is unaffected either way (`ServingError::internal` redacts to
/// `"internal error"`); this is purely so an operator reading the log is told
/// the truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MalformedReason {
    /// An `Enum` parameter published with no non-empty `allowed` list.
    EnumHasNoAllowedList,
    /// A `String` parameter's `pattern` does not compile through
    /// [`crate::lifecycle::compile_pattern`].
    PatternDoesNotCompile,
    /// A `Date`/`Timestamp` parameter's `not_before` or `not_after` does not
    /// parse through [`normalize_temporal_bound`].
    ///
    /// `lifecycle::check_constraints` already refuses a bound that fails to
    /// parse (spec §5), so reaching this arm means a snapshot published
    /// before that check existed, or a hand-edited row — the exact situation
    /// `PatternDoesNotCompile` is already the precedent for: the spec itself
    /// is unhonourable, not the caller's request, so this is `Malformed`
    /// rather than `Constraint` or `Invalid`.
    TemporalBoundDoesNotParse,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CoerceError {
    /// The text does not satisfy the declared `ty`.
    Invalid,
    /// The `ParamSpec` itself cannot be honoured — an `Enum` with no `allowed`,
    /// or a `pattern` that does not compile.
    ///
    /// Carries [`MalformedReason`] so the two causes stay distinguishable at
    /// the point `resolve` turns this into a logged [`ServingError::internal`]
    /// message. Both reach the caller identically (`"internal error"`) — this
    /// detail is for whoever reads the log, not the wire.
    Malformed(MalformedReason),
    /// The value is well-typed and the spec is sound, and the value still fails
    /// a declared constraint.
    ///
    /// **The detail here is for the AUTHOR, at publish.** The serving mapping
    /// throws it away deliberately: `serving/error.rs` keeps `ParamInvalid` a
    /// unit variant so a 400 cannot become a directory of an endpoint's
    /// parameters. `check_defaults` is what reads these fields.
    Constraint {
        constraint: &'static str,
        bound: String,
    },
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
        ParamType::String => {
            // `chars().count()`, never `len()`: `len()` is BYTES, and a
            // `max_length: 10` that fails a Vietnamese name at seven letters is
            // a bug the author cannot see from the declaration.
            let chars = text.chars().count() as u32;
            if let Some(lo) = spec.min_length {
                if chars < lo {
                    return Err(CoerceError::Constraint {
                        constraint: "min_length",
                        bound: lo.to_string(),
                    });
                }
            }
            if let Some(hi) = spec.max_length {
                if chars > hi {
                    return Err(CoerceError::Constraint {
                        constraint: "max_length",
                        bound: hi.to_string(),
                    });
                }
            }
            if let Some(pattern) = &spec.pattern {
                // Compiled through the SAME function publish used, so a pattern
                // that published can always be matched, and the anchors are
                // added in exactly one place.
                //
                // `check_constraints` already refused a pattern that does not
                // compile, so a failure here means a snapshot published before
                // this check existed, or a hand-edited row. Treat it as the
                // spec being unhonourable rather than the caller being wrong —
                // `Malformed` is the variant that already means that.
                //
                // Compiled fresh on every call rather than cached: measured
                // (release build, 1,000-iteration throwaway benchmarks,
                // deleted after measuring, on 2026-08-26) at ~60
                // microseconds/call for a typical email-shaped pattern, and
                // ~650 microseconds/call for a pattern engineered to sit at
                // `MAX_PATTERN_PROGRAM_BYTES` (a 164-branch literal
                // alternation, the worst of several 64-KiB-cap shapes tried —
                // nested bounded repetition, Unicode-property alternation,
                // Unicode-aware `.` repetition all compiled faster). Both are
                // microseconds, not milliseconds, and the worst case is still
                // far below the cost of the request it is part of. A cache
                // would be complexity bought with nothing; if that changes,
                // it belongs beside the snapshot cache (router.rs:1289), not
                // here.
                let re = crate::lifecycle::compile_pattern(pattern)
                    .map_err(|_| CoerceError::Malformed(MalformedReason::PatternDoesNotCompile))?;
                if !re.is_match(text) {
                    return Err(CoerceError::Constraint {
                        constraint: "pattern",
                        bound: pattern.clone(),
                    });
                }
            }
            Ok(Param::Str(text.to_string()))
        }
        ParamType::Enum => {
            let allowed = spec
                .allowed
                .as_ref()
                .filter(|a| !a.is_empty())
                // An enum with no `allowed` is an author mistake publish
                // refuses (spec §5). Not the caller's fault, so not a 400.
                .ok_or(CoerceError::Malformed(
                    MalformedReason::EnumHasNoAllowedList,
                ))?;
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
            // Exact text, never an `f64`: a decimal that round-trips through a
            // float stops comparing equal to itself on the engine. The grammar
            // is deliberately narrow — no exponent, no `NaN`, no `inf` — so the
            // value is a literal both engines accept as a DECIMAL.
            //
            // `is_plain_decimal` alone, never `text.parse::<i64>().is_ok()` as
            // a second way in: `i64::from_str` accepts a leading `+` that
            // `is_plain_decimal` (deliberately) does not, and admitting
            // through EITHER check would let a `+`-prefixed value back in
            // through the gap between them — the exact bypass this function
            // exists to close. `is_plain_decimal` already accepts everything
            // `i64::from_str` does other than that, including magnitudes
            // outside `i64`'s range (handled below as `Param::Decimal`).
            let text_value = if is_plain_decimal(text) {
                text
            } else {
                return Err(CoerceError::Invalid);
            };
            if let Some(lo) = &spec.minimum {
                if compare_decimal_text(text_value, lo) == std::cmp::Ordering::Less {
                    return Err(CoerceError::Constraint {
                        constraint: "minimum",
                        bound: lo.clone(),
                    });
                }
            }
            if let Some(hi) = &spec.maximum {
                if compare_decimal_text(text_value, hi) == std::cmp::Ordering::Greater {
                    return Err(CoerceError::Constraint {
                        constraint: "maximum",
                        bound: hi.clone(),
                    });
                }
            }
            if let Ok(i) = text_value.parse::<i64>() {
                return Ok(Param::I64(i));
            }
            Ok(Param::Decimal(text_value.to_string()))
        }
        // Parsed and re-formatted rather than passed through, so ONE spelling
        // reaches the engine. chrono accepts an unpadded `2026-1-2`; it comes
        // back as `2026-01-02`, which is also why the two spellings share a
        // cache entry — they resolve to the same `Param`, so they render the
        // same statement and bind the same value. A value chrono cannot read at
        // all never reaches the engine to be interpreted (or not) by whatever
        // that engine's own parser happens to accept.
        //
        // `normalize_temporal_bound` — not an inline `chrono::NaiveDate::parse_from_str`
        // — because `check_temporal_bounds` below has to normalise the SPEC's
        // own `not_before`/`not_after` through the identical grammar, and a
        // second inline copy of this parse is the exact duplication a review
        // already caught once in this package (see that function's doc).
        ParamType::Date => {
            let normalised =
                normalize_temporal_bound(&spec.ty, text).ok_or(CoerceError::Invalid)?;
            check_temporal_bounds(spec, &normalised)?;
            Ok(Param::Date(normalised))
        }
        ParamType::Timestamp => {
            let normalised =
                normalize_temporal_bound(&spec.ty, text).ok_or(CoerceError::Invalid)?;
            check_temporal_bounds(spec, &normalised)?;
            Ok(Param::Timestamp(normalised))
        }
    }
}

/// Bounds for a `Date` or `Timestamp`, compared as normalised text.
///
/// Lexical comparison is correct here and only here: both sides are in the
/// ONE spelling [`normalize_temporal_bound`] produces (`%Y-%m-%d`, zero-
/// padded, big-endian), and that format sorts chronologically as text.
///
/// **The bound is normalised too, not compared as `spec.not_before` raw.**
/// `lifecycle::check_constraints` validates that an author's bound PARSES; it
/// does not rewrite the stored text into the normalised spelling. An author
/// who wrote `not_before: "2026-1-1"` publishes clean, and that string sits in
/// the snapshot unpadded forever. Comparing a normalised caller value like
/// `"2026-01-02"` against that raw bound gets the wrong answer: byte for byte,
/// `"2026-01-02"` is LESS than `"2026-1-1"` (`'0' < '1'` at the fifth byte),
/// so a legitimately later date would be refused. Both sides go through
/// [`normalize_temporal_bound`] for exactly the reason the caller's value
/// does — see this module's `bounds_compare_the_normalised_value_not_the_callers_spelling`
/// and `the_bound_itself_is_normalised_before_it_is_compared` tests, which pin
/// each side of this separately.
fn check_temporal_bounds(spec: &ParamSpec, normalised: &str) -> Result<(), CoerceError> {
    if let Some(lo) = &spec.not_before {
        let lo_n = normalize_temporal_bound(&spec.ty, lo).ok_or(CoerceError::Malformed(
            MalformedReason::TemporalBoundDoesNotParse,
        ))?;
        if normalised < lo_n.as_str() {
            return Err(CoerceError::Constraint {
                constraint: "not_before",
                bound: lo_n,
            });
        }
    }
    if let Some(hi) = &spec.not_after {
        let hi_n = normalize_temporal_bound(&spec.ty, hi).ok_or(CoerceError::Malformed(
            MalformedReason::TemporalBoundDoesNotParse,
        ))?;
        if normalised > hi_n.as_str() {
            return Err(CoerceError::Constraint {
                constraint: "not_after",
                bound: hi_n,
            });
        }
    }
    Ok(())
}

/// Compare two decimal texts without going through a float.
///
/// `coerce` refuses to put a decimal through an `f64` because a decimal that
/// round-trips through a float stops comparing equal to itself on the engine.
/// A bound compared as a float would reintroduce that on this side: a `minimum`
/// of `"0.3"` would reject the value `"0.3"`.
///
/// Both sides have already passed [`is_plain_decimal`] — either because
/// `coerce` validated a resolved value's digits, or because
/// `lifecycle::check_constraints` validated the author's `minimum`/`maximum`
/// bound the same way before comparing the pair — so the grammar is
/// **exactly** `-?digits[.digits]`, no `+`. `is_plain_decimal` is the only
/// gate either side ever passes through, and it refuses a leading `+`
/// precisely so that invariant holds here without this function having to
/// re-check it: a `+` counted as an integer-part digit would compare as
/// longer, hence "greater", than a same-magnitude bound with no sign.
pub(crate) fn compare_decimal_text(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    // A magnitude of all zeros (`"0"`, `"0.00"`, …) is arithmetically the
    // same value whether or not it carries a `-`: `"-0"` is not less than
    // `"0"`. Without this, the sign-shortcut below would say it is, because
    // it decides purely from the two signs before ever looking at magnitude.
    let is_zero_magnitude = |body: &str| body.bytes().all(|b| b == b'0' || b == b'.');
    let (a_neg, a_body) = a.strip_prefix('-').map_or((false, a), |r| (true, r));
    let (b_neg, b_body) = b.strip_prefix('-').map_or((false, b), |r| (true, r));
    let a_neg = a_neg && !is_zero_magnitude(a_body);
    let b_neg = b_neg && !is_zero_magnitude(b_body);
    match (a_neg, b_neg) {
        (false, true) => return Ordering::Greater,
        (true, false) => return Ordering::Less,
        _ => {}
    }
    let flip = |o: Ordering| if a_neg { o.reverse() } else { o };

    let split = |s: &str| {
        let mut it = s.splitn(2, '.');
        let int = it.next().unwrap_or("0").to_string();
        let frac = it.next().unwrap_or("").to_string();
        (int, frac)
    };
    let (a_int, a_frac) = split(a_body);
    let (b_int, b_frac) = split(b_body);

    // Integer part: longer (after stripping leading zeros) wins, else lexical.
    let a_i = a_int.trim_start_matches('0');
    let b_i = b_int.trim_start_matches('0');
    let by_int = a_i.len().cmp(&b_i.len()).then_with(|| a_i.cmp(b_i));
    if by_int != Ordering::Equal {
        return flip(by_int);
    }

    // Fraction: pad to equal length so "1.5" vs "1.45" compares digit by digit.
    let width = a_frac.len().max(b_frac.len());
    let pad = |s: &str| format!("{s:0<width$}");
    flip(pad(&a_frac).cmp(&pad(&b_frac)))
}

/// Parse and normalise a `not_before`/`not_after` bound the same way `coerce`
/// parses a caller's VALUE of the declared type.
///
/// Shared with `lifecycle::check_constraints`, which validates (and, for the
/// pair, compares) an author's bound this way at publish — the same rule that
/// put [`compare_decimal_text`] here rather than in `lifecycle.rs`: one
/// normalisation, used by publish and (from Task 5) by `coerce` itself, so
/// the two cannot drift apart about which spellings are valid.
///
/// **Never routed through `coerce` itself.** From Task 5 onward, `coerce`'s
/// `Date`/`Timestamp` arms enforce a spec's OWN `not_before`/`not_after`
/// against the value being coerced. Asking `coerce` to coerce a BOUND as a
/// value would check that bound against itself.
///
/// Returns `None` for anything not `Date`/`Timestamp` — unreachable through
/// `check_constraints`, which has already refused a temporal constraint on
/// any other type before this is ever called.
pub(crate) fn normalize_temporal_bound(ty: &ParamType, text: &str) -> Option<String> {
    match ty {
        ParamType::Date => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .map(|d| d.format("%Y-%m-%d").to_string()),
        ParamType::Timestamp => ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
            .iter()
            .find_map(|f| chrono::NaiveDateTime::parse_from_str(text, f).ok())
            .map(|t| t.format("%Y-%m-%d %H:%M:%S%.f").to_string()),
        _ => None,
    }
}

/// `-?digits[.digits]`, and nothing else.
///
/// `pub(crate)` for `lifecycle::check_constraints`, which validates an
/// author's `minimum`/`maximum` bound against this same grammar before
/// [`compare_decimal_text`] is asked to compare two of them — see that
/// function's own doc for why the invariant matters.
pub(crate) fn is_plain_decimal(s: &str) -> bool {
    // No `+`, deliberately: `compare_decimal_text` never strips one, so a
    // `+`-prefixed value that reached it would have its sign counted as an
    // integer-part digit and compare as longer (hence "greater") than a
    // same-magnitude bound with no sign at all — a `minimum` a caller could
    // step around with `?n=%2B100`. This function is the ONLY gate for both
    // a caller's value (`coerce`) and an author's bound
    // (`lifecycle::check_constraints`), so excluding `+` here is what makes
    // `compare_decimal_text`'s documented grammar actually true rather than
    // aspirational.
    let body = s.strip_prefix('-').unwrap_or(s);
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
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            not_before: None,
            not_after: None,
            description: String::new(),
        }
    }

    fn number_param(name: &str) -> ParamSpec {
        spec(name, ParamType::Number, false)
    }

    fn string_param(name: &str) -> ParamSpec {
        spec(name, ParamType::String, false)
    }

    fn date_param(name: &str) -> ParamSpec {
        ParamSpec {
            ty: ParamType::Date,
            ..number_param(name)
        }
    }

    fn timestamp_param(name: &str) -> ParamSpec {
        ParamSpec {
            ty: ParamType::Timestamp,
            ..number_param(name)
        }
    }

    /// The written SQL of a fixture's only statement.
    ///
    /// Every fixture in this module is written SQL — the file is about resolving
    /// a REQUEST's parameters, which happens before any statement exists — so
    /// the `expect` here is a statement about the fixtures rather than a
    /// possibility. A scripted statement has no text to render at this point at
    /// all; that is `router::statement_texts`' subject.
    fn written(snap: &EndpointSnapshot, i: usize) -> &str {
        snap.statements[i]
            .source
            .sql()
            .expect("these fixtures are written SQL")
    }

    fn sole_sql(snap: &EndpointSnapshot) -> &str {
        written(snap, 0)
    }

    fn snap(sql: &str, params: Vec<ParamSpec>) -> EndpointSnapshot {
        EndpointSnapshot {
            statements: vec![egret_dataapi_api::StatementSnapshot {
                name: "main".into(),
                source: egret_dataapi_api::StatementSource::Sql(sql.into()),
                columns: vec![],
            }],
            iterated: Vec::new(),
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
            param_gate: egret_dataapi_api::PARAM_GATE,
        }
    }

    const SQL: &str = "SELECT 1 AS n WHERE 1=1 {% if day %} AND day = {{ day }} {% endif %}";

    /// The canonical pagination pair the brief is shaped around: `total` reads
    /// no parameters at all, `rows` iterates `r` in a `{% for %}` header. `total`
    /// is listed FIRST — the one a `statements[0]` reader would see alone — so
    /// any code path that still looks at only the first statement's SQL sees no
    /// trace of `r`.
    ///
    /// **What this pair CANNOT catch, recorded because it did not:** `total`
    /// reads no parameters, so widening `r` to a list is invisible to it. The
    /// disagreement that matters is a sibling that uses the same parameter as a
    /// SCALAR — it then fails to render against the union this function
    /// computes. That is a publish-time rule and it is pinned where publish
    /// lives, in `lifecycle::tests::publish_infers_the_shape_serving_will_bind`,
    /// which drives this function and then renders the scalar statement with
    /// what it returned.
    const PAGINATION_TOTAL_SQL: &str = "SELECT count(*) AS n FROM t";
    const PAGINATION_ROWS_SQL: &str =
        "SELECT x AS n FROM t WHERE r IN ({% for x in r %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})";

    /// A two-statement snapshot over the pagination pair above, `total` first.
    fn pagination_snap(params: Vec<ParamSpec>) -> EndpointSnapshot {
        let mut s = snap(PAGINATION_TOTAL_SQL, params);
        s.statements = vec![
            egret_dataapi_api::StatementSnapshot {
                name: "total".into(),
                source: egret_dataapi_api::StatementSource::Sql(PAGINATION_TOTAL_SQL.into()),
                columns: vec![],
            },
            egret_dataapi_api::StatementSnapshot {
                name: "rows".into(),
                source: egret_dataapi_api::StatementSource::Sql(PAGINATION_ROWS_SQL.into()),
                columns: vec![],
            },
        ];
        s
    }

    fn rendered(query: &str, s: &EndpointSnapshot) -> (String, Vec<Param>) {
        let r = resolve(Some(query), s).expect("resolve");
        crate::render::render(sole_sql(s), &r.values).expect("render")
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
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            not_before: None,
            not_after: None,
            description: String::new(),
        };
        let mut supplied: BTreeMap<String, Vec<String>> = BTreeMap::new();
        supplied.insert("q".into(), vec!["a&b=c".into()]);
        let out = resolve_supplied(&supplied, &["SELECT {{ q }}"], std::slice::from_ref(&spec))
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
        let a = crate::render::render(sole_sql(&s), &present.values).expect("render");
        let b = crate::render::render(sole_sql(&s), &absent.values).expect("render");
        assert_ne!(a, b);
        assert!(b.1.is_empty(), "an absent parameter binds nothing");
    }

    /// **Step 1's property, at the level where it is decided.** `total` — the
    /// FIRST statement, and the only one a `statements[0]` reader would see —
    /// never mentions `r`; only `rows`, the SECOND, iterates it in a
    /// `{% for %}` header. A caller supplying ONE value for `r` must still
    /// resolve it as a list of one, because that is the shape `rows` needs to
    /// render at all (Task 7 renders every statement against this same
    /// `ParamValues`). Reading only the first statement's SQL leaves `r` a bare
    /// scalar, and a scalar `Cell` is `ObjectRepr::Plain` — not iterable — so
    /// `rows` fails to render with exactly that shape.
    #[test]
    fn a_parameter_only_the_second_statement_iterates_still_resolves_as_a_list() {
        let s = pagination_snap(vec![spec("r", ParamType::String, false)]);
        let resolved = resolve(Some("r=EU"), &s).expect("resolve");
        let (_, binds) = crate::render::render(PAGINATION_ROWS_SQL, &resolved.values)
            .expect("`rows` must render against the values `resolve` produced");
        assert_eq!(binds, vec![Param::Str("EU".into())]);
    }

    /// **Step 1's second half**, in the file's own convention (see
    /// `lifecycle.rs`'s `the_sev_0_payload_is_refused`, which asserts one rule
    /// through both its entry points in adjacent lines): [`resolve_supplied`] is
    /// where preview reaches this union directly, bypassing the query-string
    /// decode `resolve` does first. Handed the SAME statements and the SAME
    /// supplied value, it must agree with [`resolve`] byte for byte — nothing
    /// may compute the union anywhere upstream of `resolve_supplied` itself, or
    /// preview and serving could disagree about one endpoint, which is the
    /// defect this programme has already shipped once (see the module comment).
    #[test]
    fn resolve_supplied_agrees_with_resolve_on_the_same_union() {
        let s = pagination_snap(vec![spec("r", ParamType::String, false)]);
        let via_resolve = resolve(Some("r=EU"), &s).expect("resolve");

        let mut supplied: BTreeMap<String, Vec<String>> = BTreeMap::new();
        supplied.insert("r".into(), vec!["EU".into()]);
        let sqls = [PAGINATION_TOTAL_SQL, PAGINATION_ROWS_SQL];
        let via_supplied = resolve_supplied(&supplied, &sqls, &s.params).expect("resolve_supplied");

        assert_eq!(
            via_resolve.key, via_supplied.key,
            "the two entry points must key the same request identically"
        );
        let want = crate::render::render(PAGINATION_ROWS_SQL, &via_resolve.values)
            .expect("rows via resolve");
        let got = crate::render::render(PAGINATION_ROWS_SQL, &via_supplied.values)
            .expect("rows via resolve_supplied");
        assert_eq!(
            want, got,
            "the two entry points must bind `rows` identically"
        );
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
                    crate::render::render(sole_sql(&s), &ra.values).expect("render a"),
                    crate::render::render(sole_sql(&s), &rb.values).expect("render b"),
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

    /// `compare_decimal_text` has no caller in this crate's public surface —
    /// `lifecycle::check_constraints` and (from Task 3) `coerce` are its only
    /// two, and both exercise it only indirectly, through inputs shaped to
    /// pass their OWN grammar checks first. This is the direct test: the
    /// digit-by-digit algorithm itself, including the case a naive `str`
    /// `>` would get backwards.
    #[test]
    fn compare_decimal_text_orders_by_value_not_by_bytes() {
        use std::cmp::Ordering;
        // The whole reason this function exists rather than `str::cmp`:
        // `"9" > "10"` as bytes (`'9' > '1'`), but 9 < 10.
        assert_eq!(compare_decimal_text("9", "10"), Ordering::Less);
        assert_eq!(compare_decimal_text("10", "9"), Ordering::Greater);
        assert_eq!(compare_decimal_text("9", "9"), Ordering::Equal);

        // Negatives: more negative is smaller, and a bare `-` prefix flips
        // the rest of the comparison.
        assert_eq!(compare_decimal_text("-5", "-1"), Ordering::Less);
        assert_eq!(compare_decimal_text("-1", "-5"), Ordering::Greater);
        assert_eq!(compare_decimal_text("-1", "1"), Ordering::Less);
        assert_eq!(compare_decimal_text("1", "-1"), Ordering::Greater);

        // Differing fraction widths: padded to compare digit by digit, not
        // truncated or compared as strings of different lengths.
        assert_eq!(compare_decimal_text("1.5", "1.45"), Ordering::Greater);
        assert_eq!(compare_decimal_text("1.45", "1.5"), Ordering::Less);
        assert_eq!(compare_decimal_text("1.50", "1.5"), Ordering::Equal);

        // A difference below `f64` precision: the whole reason `coerce`
        // refuses to put a decimal through a float. These two digit strings
        // are independently verified (not merely asserted in prose) to parse
        // to the identical `f64` — `1.0` — so a pure-float reimplementation
        // of this function returns `Equal` here, not `Less`; a pair that
        // still differs under `f64` would let such a reimplementation pass
        // this assertion by accident.
        assert_eq!(
            compare_decimal_text("1.00000000000000001", "1.00000000000000002"),
            Ordering::Less
        );

        // "-0" is arithmetically "0", not less than it. The sign-only
        // shortcut above must not fire on a zero magnitude.
        assert_eq!(compare_decimal_text("-0", "0"), Ordering::Equal);
        assert_eq!(compare_decimal_text("0", "-0"), Ordering::Equal);
        assert_eq!(compare_decimal_text("-0.00", "0"), Ordering::Equal);
    }

    #[test]
    fn a_number_below_minimum_is_refused_and_says_which_constraint() {
        let mut p = number_param("n");
        p.minimum = Some("10".into());
        match coerce(&p, "9") {
            Err(CoerceError::Constraint { constraint, bound }) => {
                assert_eq!(constraint, "minimum");
                assert_eq!(bound, "10");
            }
            other => panic!("expected a constraint refusal, got {other:?}"),
        }
        assert!(coerce(&p, "10").is_ok(), "the bound itself is inclusive");
        assert!(coerce(&p, "11").is_ok());
    }

    #[test]
    fn a_number_above_maximum_is_refused() {
        let mut p = number_param("n");
        p.maximum = Some("100".into());
        assert!(matches!(
            coerce(&p, "101"),
            Err(CoerceError::Constraint { .. })
        ));
        assert!(coerce(&p, "100").is_ok(), "inclusive");
    }

    /// `is_plain_decimal` is the ONLY gate `coerce` admits a `Number` value
    /// through — never `text.parse::<i64>().is_ok()` as a second way in — and
    /// it deliberately refuses a leading `+`, because `compare_decimal_text`
    /// never strips one. Before this refusal, `+100` was admitted (via the
    /// `i64` half of the old two-way gate) with its `+` left in `text_value`,
    /// so the sign counted as an integer-part digit and made `"+100"` compare
    /// as LONGER — hence "greater" — than a same-magnitude bound with no
    /// sign at all: `minimum: "500"` accepted `?n=%2B100`, and `maximum: "5"`
    /// rejected the in-range value `"+5"`. Both are closed the same way: `+`
    /// is not admitted as a `Number` at all, regardless of any bound.
    #[test]
    fn a_leading_plus_is_refused_not_admitted_through_the_i64_gate() {
        let plain = number_param("n");
        assert!(
            matches!(coerce(&plain, "+100"), Err(CoerceError::Invalid)),
            "a `+`-prefixed value must never reach a constraint check at all"
        );

        // The exact bypass: a caller-controlled `+100` must not compare as
        // "at least 500".
        let mut floor = number_param("n");
        floor.minimum = Some("500".into());
        assert!(matches!(coerce(&floor, "+100"), Err(CoerceError::Invalid)));

        // And the mirror image: a `+`-prefixed value that WOULD be in range
        // if normalised must still be refused outright, not wrongly reported
        // as out of range.
        let mut ceiling = number_param("n");
        ceiling.maximum = Some("5".into());
        assert!(matches!(coerce(&ceiling, "+5"), Err(CoerceError::Invalid)));
    }

    #[test]
    fn bounds_compare_decimals_without_going_through_a_float() {
        // 0.1 + 0.2 != 0.3 in binary floating point. If the comparison round-trips
        // through an f64, a bound of "0.3" rejects the value "0.3".
        let mut p = number_param("n");
        p.minimum = Some("0.3".into());
        assert!(coerce(&p, "0.3").is_ok(), "a decimal must equal itself");

        // And a long decimal must not be truncated into equality. Both of these
        // parse to the exact same `f64` (`1.0`) — verified independently, not
        // merely asserted in prose — so an implementation that compares through
        // a float cannot tell them apart and this refusal is the only place
        // that fact can be caught.
        let mut q = number_param("n");
        q.maximum = Some("1.00000000000000001".into());
        assert!(
            matches!(
                coerce(&q, "1.00000000000000002"),
                Err(CoerceError::Constraint { .. })
            ),
            "a difference below f64 precision must still be a difference"
        );
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // "Nguyễn" is 6 characters and 8 bytes. A max_length of 6 must accept it.
        let mut p = string_param("s");
        p.max_length = Some(6);
        assert!(
            coerce(&p, "Nguyễn").is_ok(),
            "6 characters must fit in max_length 6; String::len() would say 8 bytes"
        );
        assert!(matches!(
            coerce(&p, "Nguyễnn"),
            Err(CoerceError::Constraint { .. })
        ));
    }

    #[test]
    fn min_length_is_enforced() {
        let mut p = string_param("s");
        p.min_length = Some(3);
        assert!(matches!(
            coerce(&p, "ab"),
            Err(CoerceError::Constraint { .. })
        ));
        assert!(coerce(&p, "abc").is_ok(), "inclusive");
    }

    #[test]
    fn a_pattern_must_match_the_whole_value() {
        // `is_match` is a SEARCH. Without anchoring, "abc" accepts "xxabcxx",
        // which is not what an author writing a validation rule means.
        let mut p = string_param("s");
        p.pattern = Some("abc".into());
        assert!(coerce(&p, "abc").is_ok());
        assert!(matches!(
            coerce(&p, "xxabcxx"),
            Err(CoerceError::Constraint { .. })
        ));
    }

    #[test]
    fn an_authors_own_anchors_give_the_same_answer() {
        let mut p = string_param("s");
        p.pattern = Some("^abc$".into());
        assert!(coerce(&p, "abc").is_ok());
        assert!(matches!(
            coerce(&p, "xxabcxx"),
            Err(CoerceError::Constraint { .. })
        ));
    }

    #[test]
    fn alternation_is_anchored_as_a_whole_not_just_its_first_branch() {
        // The trap in anchoring by concatenation: "^a|b$" parses as "(^a)|(b$)",
        // which accepts "zzzb". Wrapping in a non-capturing group is what makes
        // the anchors apply to the whole alternation.
        let mut p = string_param("s");
        p.pattern = Some("a|b".into());
        assert!(coerce(&p, "a").is_ok());
        assert!(coerce(&p, "b").is_ok());
        assert!(matches!(
            coerce(&p, "zzzb"),
            Err(CoerceError::Constraint { .. })
        ));
        assert!(matches!(
            coerce(&p, "azzz"),
            Err(CoerceError::Constraint { .. })
        ));
    }

    #[test]
    fn a_pattern_names_itself_in_the_coerce_error() {
        let mut p = string_param("s");
        p.pattern = Some("^[0-9]+$".into());
        match coerce(&p, "abc") {
            Err(CoerceError::Constraint { constraint, .. }) => {
                assert_eq!(constraint, "pattern")
            }
            other => panic!("expected a pattern refusal, got {other:?}"),
        }
    }

    /// A non-compiling `pattern` is `check_constraints`' job to have already
    /// refused — reaching `coerce` at all means a snapshot published before
    /// that gate existed, or a hand-edited row. Either way this is
    /// [`CoerceError::Malformed`], carrying [`MalformedReason::PatternDoesNotCompile`]
    /// rather than the pre-existing [`MalformedReason::EnumHasNoAllowedList`]
    /// the two used to share as one unlabelled variant.
    #[test]
    fn a_pattern_that_does_not_compile_is_malformed_with_its_own_reason() {
        let mut p = string_param("s");
        // Unbalanced: `compile_pattern` wraps this as `^(?:()$`, which never
        // closes the outer non-capturing group.
        p.pattern = Some("(".into());
        assert_eq!(
            coerce(&p, "anything"),
            Err(CoerceError::Malformed(
                MalformedReason::PatternDoesNotCompile
            ))
        );
    }

    /// **The regression this pins**: before `MalformedReason` existed, both
    /// `Enum`'s missing `allowed` and `String`'s non-compiling `pattern`
    /// collapsed into one `CoerceError::Malformed` unit variant, so `resolve`'s
    /// mapping used a single fixed log message — "enum parameter published
    /// with no `allowed` list" — for BOTH causes. A hand-edited row with an
    /// uncompilable `pattern` produced a 500 whose logged diagnosis blamed a
    /// missing enum list on an endpoint that has no enum parameter at all.
    ///
    /// The wire response was never wrong (`ServingError::internal` redacts to
    /// `"internal error"` regardless), so this only checks the internal
    /// diagnosis — the `Debug` rendering of the returned [`ServingError`],
    /// which is exactly the field `router::serve` logs as `error = ?e`
    /// (`router.rs`'s `THE ONLY place the internal detail is allowed to
    /// appear` comment).
    #[test]
    fn a_non_compiling_patterns_diagnostic_does_not_blame_a_missing_enum_list() {
        let mut p = string_param("s");
        p.pattern = Some("(".into());
        let supplied =
            std::collections::BTreeMap::from([("s".to_string(), vec!["anything".to_string()])]);
        let err = match resolve_supplied_with(&supplied, &[], std::slice::from_ref(&p)) {
            Err(e) => e,
            Ok(_) => panic!("expected a non-compiling pattern to refuse as internal"),
        };
        assert!(
            matches!(err, ServingError::Internal(_)),
            "a malformed spec is the author's 500, not the caller's 400: {err:?}"
        );
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("pattern") && rendered.contains("does not compile"),
            "the log line must name the pattern as the cause: {rendered}"
        );
        assert!(
            !rendered.contains("enum") && !rendered.contains("allowed"),
            "the log line must not repeat the UNRELATED enum diagnosis: {rendered}"
        );
    }

    /// The companion case, so a future edit that swaps the two match arms'
    /// message strings is still caught: an enum published with no `allowed`
    /// must keep naming itself, not borrow the pattern's message.
    #[test]
    fn an_enum_with_no_allowed_lists_diagnostic_does_not_blame_a_pattern() {
        let p = spec("e", ParamType::Enum, false);
        let supplied = std::collections::BTreeMap::from([("e".to_string(), vec!["x".to_string()])]);
        let err = match resolve_supplied_with(&supplied, &[], std::slice::from_ref(&p)) {
            Err(e) => e,
            Ok(_) => panic!("expected an enum with no allowed list to refuse as internal"),
        };
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("enum") && rendered.contains("allowed"),
            "the log line must name the missing allowed list as the cause: {rendered}"
        );
        assert!(
            !rendered.contains("pattern"),
            "the log line must not borrow the UNRELATED pattern diagnosis: {rendered}"
        );
    }

    #[test]
    fn length_is_checked_before_the_pattern() {
        // Cheap rejection first: a 10 MB value against a pattern is work that a
        // max_length would have avoided. Order is observable through which
        // constraint the error names.
        let mut p = string_param("s");
        p.max_length = Some(3);
        p.pattern = Some("^[a-z]+$".into());
        match coerce(&p, "abcdefgh") {
            Err(CoerceError::Constraint { constraint, .. }) => {
                assert_eq!(constraint, "max_length")
            }
            other => panic!("expected max_length to fire first, got {other:?}"),
        }
    }

    #[test]
    fn a_constraint_refusal_reaches_the_caller_as_a_bare_param_invalid() {
        // The whole point of §2.1: the caller learns "send something else" and
        // NOTHING about the endpoint's parameters. If this test ever needs the
        // bound, the change is wrong — read `serving/error.rs`'s doc comment first.
        let mut p = number_param("n");
        p.maximum = Some("100".into());
        let supplied =
            std::collections::BTreeMap::from([("n".to_string(), vec!["101".to_string()])]);
        let err = match resolve_supplied_with(&supplied, &[], std::slice::from_ref(&p)) {
            Err(e) => e,
            Ok(_) => panic!("expected a constraint refusal over the maximum"),
        };
        assert!(matches!(err, ServingError::ParamInvalid));
        // `ParamInvalid` is a unit variant, so its `Debug` rendering is exactly
        // its own name — nothing can be appended to it. Checking for a
        // substring like the bound ("100") or the parameter name ("n") is not
        // this test: "n" and even a two-digit bound can occur by coincidence
        // inside "ParamInvalid" itself (it contains an "n" in "Invalid"), which
        // would fail this test on a CORRECT implementation. The exact-equality
        // check below is what actually proves nothing was attached.
        let rendered = format!("{err:?}");
        assert_eq!(
            rendered, "ParamInvalid",
            "the constraint's detail leaked into the error: {rendered}"
        );
    }

    /// Distinguishes the `(CoerceError::Constraint, from_default: true)` arm
    /// from the caller-facing one right above it. Before this test, swapping
    /// that arm to `ServingError::ParamInvalid` (blaming the caller for the
    /// author's own out-of-bounds `default`) left all other tests in this
    /// module green, because none of them supplies no value at all and relies
    /// on a `default` that fails its own declared constraint.
    #[test]
    fn a_defaults_own_constraint_violation_is_the_authors_500_not_the_callers_400() {
        let mut p = number_param("n");
        p.maximum = Some("100".into());
        p.default = Some("101".into());
        let supplied = std::collections::BTreeMap::new();
        let err = match resolve_supplied_with(&supplied, &[], std::slice::from_ref(&p)) {
            Err(e) => e,
            Ok(_) => panic!("expected the author's own default to fail its own maximum"),
        };
        assert!(
            matches!(err, ServingError::Internal(_)),
            "an author's bad default must be the author's 500, not the caller's 400: {err:?}"
        );
    }

    #[test]
    fn a_date_before_not_before_is_refused() {
        let mut p = date_param("d");
        p.not_before = Some("2020-01-01".into());
        assert!(matches!(
            coerce(&p, "2019-12-31"),
            Err(CoerceError::Constraint { .. })
        ));
        assert!(coerce(&p, "2020-01-01").is_ok(), "inclusive");
    }

    #[test]
    fn a_date_after_not_after_is_refused() {
        let mut p = date_param("d");
        p.not_after = Some("2030-12-31".into());
        assert!(matches!(
            coerce(&p, "2031-01-01"),
            Err(CoerceError::Constraint { .. })
        ));
        assert!(coerce(&p, "2030-12-31").is_ok(), "inclusive");
    }

    /// **The test that actually carries this discrimination** — the brief's
    /// own example (`not_before: "2026-01-01"`, value `"2026-1-2"`) turns out
    /// NOT to: `"2026-1-2"` already sorts AFTER `"2026-01-01"` as raw bytes
    /// (unpadded `'1'` at the fifth byte beats padded `'0'`), so a broken
    /// implementation that compared the caller's raw text would still accept
    /// it, for the wrong reason. Verified by hand before writing this.
    ///
    /// The case that truly requires normalisation is the one
    /// `lifecycle::check_constraints`'s own doc names: an unpadded SINGLE
    /// digit that is lexically larger than a PADDED double digit's leading
    /// character. `"2026-9-30"` (Sept 30) vs a `"2026-10-01"` (Oct 1) bound —
    /// raw bytes put `'9'` ahead of `'1'` at the fifth byte, so a raw-text
    /// comparison sorts Sept 30 AFTER Oct 1 despite it being chronologically
    /// EARLIER, and would wrongly ACCEPT a date the bound exists to refuse.
    /// Comparing the normalised `"2026-09-30"` against `"2026-10-01"` gets it
    /// right.
    #[test]
    fn bounds_compare_the_normalised_value_not_the_callers_spelling() {
        let mut p = date_param("d");
        p.not_before = Some("2026-10-01".into());
        assert!(
            matches!(coerce(&p, "2026-9-30"), Err(CoerceError::Constraint { .. })),
            "Sept 30 is chronologically before the Oct 1 bound and must be \
             refused, even though its unpadded spelling sorts AFTER the \
             bound as raw text"
        );
        assert!(
            coerce(&p, "2026-10-2").is_ok(),
            "and an unpadded date genuinely after the bound must still be accepted"
        );
    }

    /// The other half of the same normalisation rule, aimed at the BOUND
    /// rather than the value: `check_constraints` validates that an author's
    /// `not_before`/`not_after` PARSES, but does not rewrite the stored text
    /// into the padded spelling. `not_before: "2026-1-1"` publishes clean and
    /// sits in the snapshot unpadded. Comparing a normalised caller value
    /// against that RAW bound gets the wrong answer: `"2026-01-02"` is
    /// lexically LESS than `"2026-1-1"` (unpadded `'1'` beats padded `'0'` at
    /// the fifth byte), which would wrongly refuse a date that is
    /// chronologically one day AFTER the bound.
    #[test]
    fn the_bound_itself_is_normalised_before_it_is_compared() {
        let mut p = date_param("d");
        p.not_before = Some("2026-1-1".into());
        assert!(
            coerce(&p, "2026-01-02").is_ok(),
            "Jan 2 is after the Jan 1 bound even though the bound's own raw \
             spelling is unpadded"
        );
    }

    #[test]
    fn a_timestamp_bound_is_enforced() {
        let mut p = timestamp_param("t");
        p.not_before = Some("2020-01-01 00:00:00".into());
        assert!(matches!(
            coerce(&p, "2019-12-31 23:59:59"),
            Err(CoerceError::Constraint { .. })
        ));
        assert!(coerce(&p, "2020-01-01 00:00:00").is_ok());
        assert!(
            coerce(&p, "2020-01-01T00:00:01").is_ok(),
            "the T spelling too"
        );
    }

    /// Same precedent as `a_pattern_that_does_not_compile_is_malformed_with_its_own_reason`:
    /// `check_constraints` already refuses a `not_before`/`not_after` that
    /// fails to parse (spec §5), so reaching `coerce` at all means a snapshot
    /// published before that gate existed, or a hand-edited row — the
    /// author's problem, not the caller's, hence `Malformed` rather than
    /// `Constraint` or `Invalid`.
    #[test]
    fn an_unparseable_bound_is_malformed_with_its_own_reason() {
        let mut p = date_param("d");
        p.not_before = Some("not-a-date".into());
        assert_eq!(
            coerce(&p, "2026-01-01"),
            Err(CoerceError::Malformed(
                MalformedReason::TemporalBoundDoesNotParse
            ))
        );
    }

    /// Same shape as `a_non_compiling_patterns_diagnostic_does_not_blame_a_missing_enum_list`:
    /// the wire response is unaffected either way (`ServingError::internal`
    /// redacts to `"internal error"`), so this checks only the internal
    /// diagnosis a future edit could accidentally collapse into one of the
    /// other two `Malformed` messages.
    #[test]
    fn an_unparseable_bounds_diagnostic_names_itself() {
        let mut p = date_param("d");
        p.not_after = Some("not-a-date".into());
        let supplied =
            std::collections::BTreeMap::from([("d".to_string(), vec!["2026-01-01".to_string()])]);
        let err = match resolve_supplied_with(&supplied, &[], std::slice::from_ref(&p)) {
            Err(e) => e,
            Ok(_) => panic!("expected an unparseable bound to refuse as internal"),
        };
        assert!(
            matches!(err, ServingError::Internal(_)),
            "an unparseable published bound is the author's 500, not the caller's 400: {err:?}"
        );
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("not_before") && rendered.contains("does not parse"),
            "the log line must name the temporal bound as the cause: {rendered}"
        );
        assert!(
            !rendered.contains("pattern") && !rendered.contains("enum"),
            "the log line must not borrow an UNRELATED `Malformed` diagnosis: {rendered}"
        );
    }

    /// **A script and the SQL are two readings of ONE request.**
    ///
    /// [`Resolved::json`] is what a C2 response transform's `params` dict
    /// carries; [`Resolved::values`] is what the statements bind. Both come from
    /// the single `as_list` decision in [`resolve_supplied_with`], and this is
    /// what says so by measurement rather than by comment.
    ///
    /// **Every case asserts BOTH sides**, including the two repeat cases: a
    /// `json` built by a second reading of the same request would agree with
    /// `values` in the easy first case and diverge in the ones after it — and
    /// those are the shapes publish's probe cannot show an author, so a
    /// disagreement there is invisible until a real call makes it.
    ///
    /// 3a and 3b are the probe's blind spot split into the half that is already
    /// closed and the half that is not, which is what makes "the gap is narrow"
    /// a measurement instead of a claim in a doc comment.
    #[test]
    fn the_script_reads_the_parameters_the_sql_bound() {
        // 1. One value for a parameter nothing iterates: a scalar on both sides.
        let s = snap(SQL, vec![number_param("day")]);
        let r = resolve(Some("day=3"), &s).expect("resolve");
        assert_eq!(r.json["day"], serde_json::json!(3));
        assert!(!r.has_repeats);
        let (_, binds) = crate::render::render(sole_sql(&s), &r.values).expect("render");
        assert_eq!(binds, vec![Param::I64(3)], "one value, one bind");

        // 2. A declared parameter the caller did not supply is `None` to the
        //    script and absent from the render — present in the dict either way,
        //    so `params["day"]` answers rather than raising.
        let r = resolve(Some(""), &s).expect("resolve");
        assert_eq!(
            r.json.keys().collect::<Vec<_>>(),
            vec!["day"],
            "the dict carries exactly the DECLARED names: {:?}",
            r.json
        );
        assert_eq!(r.json["day"], serde_json::Value::Null);
        let (_, binds) = crate::render::render(sole_sql(&s), &r.values).expect("render");
        assert!(
            binds.is_empty(),
            "the `{{% if day %}}` block did not render"
        );

        // 3a. A REPEAT of a parameter a statement uses as a SCALAR. Both sides
        //     see the list — and the SQL side is a REFUSAL, which is the 400
        //     that means no script ever runs for this request. This is the half
        //     of the probe's blind spot that is already closed one layer up, and
        //     it is asserted here so that "the gap is narrow" is a measurement
        //     rather than a claim in a doc comment.
        let r = resolve(Some("day=3&day=4"), &s).expect("resolve");
        assert!(r.has_repeats);
        assert_eq!(
            r.json["day"],
            serde_json::json!([3, 4]),
            "the script's side: both values, in the order the caller sent them"
        );
        assert_eq!(
            crate::render::render(sole_sql(&s), &r.values).unwrap_err(),
            crate::render::RenderError::SequenceBind,
            "the SQL's side: `{{{{ day }}}}` cannot bind a sequence, so this \
             request is refused before a transform is reached",
        );

        // 3b. The RESIDUAL gap, stated exactly: a repeat of a parameter no
        //     statement uses at all. Nothing refuses it, the SQL binds nothing,
        //     and the script is handed the list publish probed as a scalar. Both
        //     sides asserted, because this is the one case where they legitimately
        //     say different things — and what they say is still one reading of
        //     the request, not two.
        let unused = snap("SELECT 1 AS n", vec![number_param("day")]);
        let r = resolve(Some("day=3&day=4"), &unused).expect("resolve");
        assert!(r.has_repeats);
        assert_eq!(
            r.json["day"],
            serde_json::json!([3, 4]),
            "uncollapsed — the script is told what the caller actually sent"
        );
        let (sql, binds) = crate::render::render(sole_sql(&unused), &r.values).expect("render");
        assert_eq!(sql, "SELECT 1 AS n", "the statement never mentions it");
        assert!(
            binds.is_empty(),
            "so the SQL binds nothing, and the script's list is the only place \
             the repeat is visible: {binds:?}"
        );

        // 4. A parameter a statement ITERATES is a list even at one value —
        //    arity is the caller's, shape is the definition's. The script must
        //    read the same list the `{% for %}` header binds, or it is written
        //    against a shape the SQL says the endpoint does not have.
        let s = pagination_snap(vec![number_param("r")]);
        let r = resolve(Some("r=3"), &s).expect("resolve");
        assert!(!r.has_repeats, "one value is not a repeat");
        assert_eq!(
            r.json["r"],
            serde_json::json!([3]),
            "a one-element LIST, exactly as the definition iterates it"
        );
        let (_, binds) = crate::render::render(written(&s, 1), &r.values).expect("render");
        assert_eq!(binds, vec![Param::I64(3)], "and the loop bound it");
    }
}
