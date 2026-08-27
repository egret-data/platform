//! `POST …/endpoints/{id}/preview` — spec §2 of the P6 authoring-loop design.
//!
//! # What this is, in one line
//!
//! The author's on-screen draft, run as the AUTHOR, through the same validation
//! and render path publish and serving use, capped at 100 rows, with none of the
//! four consumer-facing side effects.
//!
//! # ONE statement runs; the endpoint's OTHER statements are still an input
//!
//! An endpoint has N statements (M2) and this route runs exactly one of them —
//! the `sql` on screen. But whether a parameter arrives as a scalar or a list is
//! a property of the WHOLE endpoint, so the body also carries the siblings' text
//! ([`PreviewEndpointInput::siblings`]) purely to decide that. They are never
//! rendered, bound or executed, and this module cannot read them: the shape
//! decision is made once by [`crate::lifecycle::validate_preview`], which returns
//! it, and the text stays inside `lifecycle::SiblingStatements`.
//!
//! # Not quite EVERY publish rule — two are about things a preview does not do
//!
//! [`crate::lifecycle::validate_preview`] omits the cache-scope rule (preview
//! touches no cache) and the unreferenced-parameter rule (it protects the OpenAPI
//! contract a PUBLISHED endpoint offers, not the statement about to run). The
//! second one is a browser finding: with it in place, SQL edited before the
//! parameter declarations caught up — the commonest state of the loop this route
//! exists to close — could not be previewed at all. Every other publish scan
//! still applies, including the statement check that keeps `:manage` from being
//! engine write access and the placeholder-inside-quoted-text rule. That
//! function's own comment is the authority on the split.
//!
//! # Why the body carries the draft rather than being read from the database
//!
//! Reading the saved draft would defeat the feature: the author would have to
//! Save before every preview, and the loop this route exists to close would
//! still be cut — by a button rather than by navigation. This opens no new
//! attack surface, because SQL Lab already executes arbitrary author-supplied
//! SQL at an equivalent permission, and the payload here passes the same gates
//! publish passes ([`crate::lifecycle::validate_preview`]).
//!
//! # The four things this must never do
//!
//! No `access_log` row (Activity measures real traffic; a preview click is not
//! traffic, and writing one would skew every percentile on that page). No
//! rate-limit consumption (an author iterating must not throttle a live
//! consumer). No cache read (it would show a stale answer for SQL just edited)
//! and no cache write (it would poison what consumers read with a result
//! produced under a different principal).
//!
//! # What actually asserts those four absences
//!
//! Two of the four are asserted end-to-end in `tests/preview.rs`:
//! `preview_writes_no_access_log_row` counts `dataapi.access_log` before and
//! after, and `preview_does_not_populate_the_cache` publishes a cache-enabled
//! `shared`-scope endpoint, previews it, and requires a consumer's next call to be
//! a MISS. **There is no cache-READ test and no rate-limit test** — do not read the
//! four as equally covered.
//!
//! The absence is nonetheless STRUCTURAL, which is the stronger guarantee and the
//! reason the two missing tests were not written: this module contains no cache,
//! rate-limit, `access_log` or audit reference of any kind. There is nothing to
//! disable and no flag to get wrong — [`run`] goes straight from validation to
//! `execute_as_with_params`. So the failure mode to watch for is a refactor that
//! ADDS one of the four (routing preview through a serving helper that does all of
//! them), not a switch flipping.
//!
//! Grepping this file for those four names is NOT that check, and cannot be: the
//! paragraphs above name all four, so such a grep matches whatever the code does.
//! Grep the CODE — every line that is not a comment — and the answer changes with
//! the property:
//!
//! ```text
//! grep -vE '^\s*//' crates/egret-dataapi/src/preview.rs \
//!   | grep -nE 'access_log|audit|cache|rate_limit'
//! ```
//!
//! No output is the property holding; any hit is the refactor to look at. Run the
//! same two commands against `serving/router.rs` for the positive control — that
//! is the file that legitimately does all four, it prints ~20 lines, and it is
//! what a preview must never be routed through.
//!
//! # Preview bypasses `decode`, so `values` is not bounded by `MAX_PARAM_BYTES`
//!
//! The serving path reaches `resolve` through `decode`, which enforces
//! `MAX_PARAM_BYTES` and `MAX_PARAM_REPEATS` on a query string. [`run`] calls
//! `crate::serving::params::resolve_supplied_with` with the body's already-parsed
//! map instead, so neither cap applies here. Accepted, deliberately:
//!
//! * The body is bounded by axum's request body limit, and what reaches the engine
//!   is bounded by `render`'s `CappedSink` — so neither this process nor the
//!   coordinator can be made to hold an unbounded statement.
//! * The route is `data_api_endpoint:manage`-gated. An author who wanted to send
//!   the engine a large statement can already do so with SQL Lab, at equivalent
//!   permission; the caps exist to stop an anonymous-ish CONSUMER, which is not
//!   who is here.
//! * `resolve_supplied` exists precisely because re-encoding the body's map into a
//!   query string and pushing it back through `decode` is lossy — a value
//!   containing `&`, `=` or `%` would not survive the round trip intact, so
//!   "reuse `decode` for the caps" would trade a bound for a correctness bug.
//!
//! # A response transform is REFUSED here, and deliberately not RUN here (C2)
//!
//! An endpoint may carry an `output.transform`: a Starlark script that takes
//! every statement's rows and returns the response. This route **checks** one
//! ([`crate::lifecycle::check_transform`], the same call `validate_endpoint`
//! makes for a publish) and **does not evaluate it against data**. Both halves
//! are decisions, and each is argued below because the seam between them is
//! where this feature has failed twice.
//!
//! ## Why it is checked: nothing else checks it before publish
//!
//! `create` and `PATCH` do not call `check_transform` — the only production
//! call site is [`crate::lifecycle::validate_endpoint`], which `/validate` and
//! `publish` reach. So a script that does not parse, a `returns` with nothing
//! in it, or a transform beside a non-default `shape` can all be SAVED, and
//! before this the author heard about it for the first time at publish, having
//! previewed green. Told "fine", then told "no" — the one thing this route
//! exists not to do. It is `output.transform`'s rules and no other rule over
//! `output`: `envelope` is already checked by the write routes, so an author
//! cannot get a bad one into the form to preview.
//!
//! ## Why it is not run: this route has one statement's rows, and no names
//!
//! `transform::run_transform` takes `statements` — a dict from statement NAME
//! to that statement's rows — because a script must not have to be rewritten
//! when its author adds a second statement. This route has neither half of
//! that:
//!
//! * **one statement's rows.** [`PreviewEndpointInput::sql`] is the only
//!   statement that runs; `siblings` is text that decides a parameter's shape
//!   and is never rendered, bound or executed. Handing the script the previewed
//!   statement's rows with every sibling's set to `[]` produces two wrong
//!   answers and only one of them announces itself. `statements["totals"][0]`
//!   raises on an empty list, so preview would show a REFUSAL for an endpoint
//!   that publishes and serves perfectly — the "told fine, then told no" defect
//!   inverted, and an author will edit a working script to make it stop. Worse
//!   is the quiet one: a script that reads a sibling it can find empty returns
//!   a well-formed `{"series": []}`, satisfies `check_returns`, and preview
//!   shows a GREEN transformed answer that is fabricated;
//! * **no names at all.** The body carries no statement name, for the previewed
//!   statement or for any sibling, so the dict could not even be keyed the way
//!   publish and serving key it.
//!
//! And a third reason that survives fixing both: these rows are capped at
//! [`PREVIEW_MAX_ROWS`], not at the endpoint's `limits.max_rows`. A table is
//! honest about that because `truncated` is beside it; an aggregate is not —
//! `{"total": 4213}` computed over a 100-row prefix looks exactly like the
//! answer a consumer gets.
//!
//! **Executing every statement** is the option that would make the data right,
//! and it is a different route, not a bigger version of this one: it would put
//! sibling text through the engine — precisely what
//! [`crate::lifecycle::SiblingStatements`] is a type in order to make
//! impossible, since a `:manage` body that can run N texts is an arbitrary
//! multi-statement run — and it would cost N engine round trips per press of a
//! keystroke-driven button.
//!
//! So preview answers with the statement's own rows and
//! [`DataApiPreviewDto::transform_declared`] says the response a consumer
//! receives is not this. What DOES show a transformed response, for real, is
//! `Try it` — it drives the actual serving router (`crate::try_it`), so the
//! script really runs, over the endpoint's real `limits`, with every statement
//! present. That is the same division this route already draws for
//! authorization: a green preview does not prove a consumer can call the
//! endpoint, and the note beside the button already sends the author to `Try
//! it` for that question.

use std::collections::BTreeMap;

use egret_core::UserId;
use egret_dataapi_api::{OutputSpec, ParamSpec};
use uuid::Uuid;

use crate::routes::{DataApiError, DataApiState};

/// Rows a preview returns, at most.
///
/// Fixed and not configurable, and the endpoint's own `limits.max_rows` does not
/// apply: an author wants the SHAPE of the data, not a million rows in a
/// browser. Named here rather than left to a call site so there is one number to
/// change and one number the UI's copy has to agree with.
pub(crate) const PREVIEW_MAX_ROWS: usize = 100;

/// Wall-clock budget for a preview.
///
/// Shorter than the serving ceiling (`serving/router.rs`'s 300 s) because the
/// person waiting is at a keyboard with an editor open, not a batch client. The
/// draft body carries no `limits`, so this is not clamping author input — it is
/// the whole budget.
pub(crate) const PREVIEW_TIMEOUT_SECS: u64 = 30;

/// The on-screen draft, as the request carries it.
///
/// No `cache`, `limits` or `rate_limit`: preview does not consult any of them,
/// and accepting fields it ignores would invite the reader to think it did.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewEndpointInput {
    /// What is in the editor right now — not what is stored. **The only
    /// statement this route runs.**
    pub sql: String,
    /// The SQL text of the endpoint's OTHER statements, as they are on screen.
    ///
    /// They decide ONE thing and are used for nothing else: whether a parameter
    /// arrives as a scalar or a list, which is a property of the endpoint (any
    /// statement's `{% for %}` header makes it a list) and not of the statement
    /// being previewed. They are never rendered, bound or executed. Omit the
    /// field for a one-statement endpoint. At most `MAX_STATEMENTS - 1` of them,
    /// since `sql` plus its siblings is an endpoint.
    // # Why the whole endpoint has to be here for one field's shape
    //
    // Serving binds a parameter as a list for EVERY statement as soon as one of
    // them loops over it, and publish validates against that same union. Judging
    // it from `sql` alone made preview disagree with publish in one direction —
    // green here, refused there — so the author was told "fine" and then told
    // "no". `crate::lifecycle::SiblingStatements` is what keeps "informs the
    // shape and nothing else" structural: its text is private to the module that
    // computes the union, so this file cannot read a byte of it.
    //
    // # `#[serde(default)]`, on a `deny_unknown_fields` struct
    //
    // An older client omits the field entirely and must keep working. It also
    // keeps TODAY'S behaviour exactly: an empty list makes the union
    // `IteratedParams::across(&[sql], …)`, the slice of one that was there
    // before.
    //
    // # Bounded by COUNT, not by length — and why that is the right pair
    //
    // `validate_preview` refuses `1 + siblings.len() > MAX_STATEMENTS`: the same
    // bound and the same error `validate_statements` gives a publish, so a body
    // cannot describe an endpoint that could not exist, and there is no new
    // number to keep in step. The per-text LENGTH gets no bound on purpose —
    // there is no per-statement SQL length limit anywhere in this feature
    // (`create` and `PATCH` accept a statement of any length under axum's body
    // cap), so inventing one here alone would let a preview refuse SQL the
    // endpoint it belongs to stores happily: the same "told fine, then told no"
    // defect in the other direction. What each text costs is one linear template
    // scan (`render::iterated_params`), not the `MAX_DECLARED_PARAMS`-driven
    // rendering, and the body is bounded by axum's limit exactly as `sql` is.
    #[serde(default)]
    #[schema(value_type = Vec<String>)]
    pub siblings: crate::lifecycle::SiblingStatements,
    /// The parameter declarations as they are on screen right now.
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// The author's test values, one list per parameter name. A list, not a
    /// scalar, because a repeated parameter is a list on the serving path too
    /// and preview must resolve it the same way.
    #[serde(default)]
    pub values: BTreeMap<String, Vec<String>>,
    /// The response contract as it is on screen — carried for ONE part of it,
    /// `transform`, and applied to nothing.
    ///
    /// `shape`, `naming` and `envelope` are still not applied here (see
    /// [`DataApiPreviewDto`]); they arrive because `transform`'s own publish
    /// rules are stated in terms of them — a transform beside a non-default
    /// `shape` is refused — and reconstructing an `OutputSpec` from a bare
    /// `TransformSpec` plus defaults would be a second, quietly weaker copy of
    /// [`crate::lifecycle::check_transform`]. The whole struct goes in, and
    /// that one function decides.
    ///
    // # REQUIRED, and this is the load-bearing part
    //
    // No `#[serde(default)]`, unlike all three fields above it. The reason is
    // the DIRECTION a default fails in: `OutputSpec::default()` has
    // `transform: None`, so an omitted `output` makes every transform refusal
    // vanish and preview goes green on exactly the drafts publish refuses. A
    // default here would not be a compatibility allowance, it would be the
    // defect — and it would be invisible, because every other test in
    // `tests/preview.rs` stays green under it.
    //
    // `siblings` documents the opposite trade and it is not a precedent: an
    // absent `siblings` is the union of one, which is the behaviour that
    // shipped, and it fails closed. There is no such reading of an absent
    // `output`.
    //
    // `tests/preview.rs::preview_will_not_read_a_default_output_for_a_body_that_omits_one`
    // asserts this at the wire, because the edit to catch is a serde attribute
    // rather than a call.
    pub output: OutputSpec,
    /// The database picker's current value. Workspace-scoped by
    /// `connection_per_principal_pair`, which answers `NotFound` for a database
    /// belonging to another workspace — this id comes from a request body, so it
    /// is exactly the shape that could name someone else's connection.
    pub database_id: Uuid,
}

/// What a preview answers with.
///
/// `columns` carries the ENGINE's own names — `output.naming` is deliberately not
/// applied, because an author debugging SQL needs the names they wrote, and the
/// published contract (naming transform included) is what `Response` is for. The
/// UI must say which is which.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DataApiPreviewDto {
    pub columns: Vec<egret_core::ColumnMeta>,
    #[schema(value_type = Vec<Vec<Object>>)]
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    /// True when the result was longer than [`PREVIEW_MAX_ROWS`] and this is a
    /// prefix of it. The connector fetches `cap + 1` and sets this rather than
    /// truncating silently, so `row_count == 100` alone cannot tell "exactly at"
    /// from "over".
    pub truncated: bool,
    /// True when the draft declares an `output.transform` — so `columns` and
    /// `rows` above are the STATEMENT's, and a consumer receives something else
    /// entirely (the script's output, described by `returns`).
    ///
    /// A fact, not a rendering: the UI owns the sentence, the way it does for
    /// `truncated`. But it must say something, because this is the one case
    /// where the gap between "what preview shows" and "what a consumer gets" is
    /// not a renaming — it is a different value of a different shape. See the
    /// module comment for why preview does not run the script, and for what
    /// does.
    pub transform_declared: bool,
}

/// One refusal from [`crate::serving::params::resolve_supplied`], as the sentence
/// the AUTHOR reads.
///
/// # Why this is not one message
///
/// Serving answers **both** parameter failures with a fixed, payload-free
/// sentence, and both reasons for that are about a CONSUMER: naming the parameter
/// turns the error surface into a directory of what an endpoint declares, and
/// echoing the value reflects caller bytes into a response. Neither applies to a
/// preview. The author owns the declarations, is looking at them on screen, and
/// typed the value themselves — so a message that will not say which input was
/// wrong, or that calls a blank required field a type error, sends them to edit
/// the wrong thing. That is the same defect this route exists to invert; see the
/// module comment.
///
/// # This pass decides nothing
///
/// It runs only after `resolve_supplied` has ALREADY refused, and it re-walks the
/// declarations for one purpose: to recover which one. It reaches that answer
/// through [`crate::serving::params::coerce`] — the same function that made the
/// decision, never a second copy of the rules — and when it cannot reproduce the
/// refusal it falls back to an unnamed sentence for the variant rather than
/// guessing. So a disagreement between this pass and `resolve_supplied` can only
/// ever cost precision in a message; it can never change the verdict.
fn param_error(
    e: crate::serving::ServingError,
    specs: &[ParamSpec],
    values: &BTreeMap<String, Vec<String>>,
) -> DataApiError {
    use crate::serving::ServingError as E;
    let supplied = |spec: &ParamSpec| values.get(&spec.name).filter(|v| !v.is_empty());
    match e {
        // A `required` declaration with no `default` and nothing typed in. The
        // fix is to fill the field in, which the old blanket message actively
        // pointed away from.
        E::ParamMissing => {
            let name = specs
                .iter()
                .find(|s| s.required && s.default.is_none() && supplied(s).is_none())
                .map(|s| s.name.clone());
            egret_core::Error::InvalidInput(match name {
                Some(name) => format!("`{name}` is declared required and has no test value"),
                None => "a parameter declared required has no test value".into(),
            })
        }
        // THREE different mistakes share this variant, and they have three
        // different fixes: a value the declared type cannot read, a value that
        // is well-typed and fails a declared constraint (C1), and more than one
        // value for an enum (which is a single choice — `serving::params`
        // refuses a repeat so `| ident` can only ever see one).
        E::ParamInvalid => {
            let repeated = specs
                .iter()
                .find(|s| {
                    s.ty == egret_dataapi_api::ParamType::Enum
                        && supplied(s).is_some_and(|v| v.len() > 1)
                })
                .map(|s| {
                    format!(
                        "`{}` is an enum and takes a single test value",
                        s.name.clone()
                    )
                });
            let uncoercible = || {
                specs.iter().find_map(|s| {
                    let v = supplied(s)?;
                    // The FIRST error, not merely "one of them errored":
                    // `ParamInvalid` covers two different author mistakes now
                    // that `coerce` enforces constraints, and they send the
                    // author to two different fields.
                    let err = v
                        .iter()
                        .find_map(|t| crate::serving::params::coerce(s, t).err())?;
                    Some(match err {
                        // Well-typed, out of range. Saying "not a valid number"
                        // about `5` is a type failure that did not happen: it
                        // points at the `type` column while the field that
                        // refused it is the `minimum` beside it. The detail is
                        // allowed out HERE and nowhere on the serving path —
                        // see this function's own doc comment, and
                        // `lifecycle::DefaultViolatesConstraint`, which is the
                        // same sentence for the same person at publish.
                        crate::serving::params::CoerceError::Constraint { constraint, bound } => {
                            format!(
                                "the test value for `{}` violates its `{constraint}` of `{bound}`",
                                s.name
                            )
                        }
                        // A genuine type failure, and — unreachably through
                        // this route — a `Malformed` spec, which
                        // `resolve_supplied_with` answers as a 500 before this
                        // pass is ever entered.
                        _ => format!(
                            "the test value for `{}` is not a valid {}",
                            s.name,
                            crate::lifecycle::type_name(&s.ty)
                        ),
                    })
                })
            };
            egret_core::Error::InvalidInput(
                repeated
                    .or_else(uncoercible)
                    .unwrap_or_else(|| "a test value is not valid for its parameter".into()),
            )
        }
        // Not reachable: both `Internal` arms in `resolve_supplied` are about a
        // published definition that `validate_preview` has already refused above
        // (a `default` that fails its own type, an enum with no `allowed`). A 500
        // rather than a 400 if it ever happens anyway — it would be egret's fault,
        // not the author's, and `Error::Internal` is what redacts the detail into
        // the log instead of the body.
        other => egret_core::Error::Internal(anyhow::anyhow!("{other}")),
    }
}

/// [`crate::lifecycle::validate_preview`], run on a blocking thread.
///
/// # Why this is not called inline, and what changed to make that necessary
///
/// `MAX_DECLARED_PARAMS` records the rule this obeys. Everything
/// `validate_preview` does is synchronous CPU on an async task, and the reason
/// that was accepted is that it was **bounded and authenticated**: ~122 ms per
/// statement, with the constant itself being what makes the upper bound a
/// number rather than a function of the request. That note ends by saying, in
/// as many words, that if the bound ever rises the reasoning has to be redone
/// rather than carried over.
///
/// Adding [`crate::lifecycle::check_transform`] to this path raised it. The
/// check parses AND evaluates the script's module top level under a real
/// Starlark interpreter, whose only time-denominated budget is
/// `transform::MAX_WALL_CLOCK` — 2 s, i.e. sixteen times the accepted figure.
/// And 2 s is not the bound either: that budget is checked at 1000-tick
/// granularity, so a script whose every tick is expensive overruns it, measured
/// at **28.8 s** (see `run_transform`'s own comment). A tickless string-doubler
/// was not interrupted at 6.8 s at all.
///
/// The accepted argument does not survive that, and it is worth being precise
/// about WHICH leg broke. Not the authentication one — this is still
/// `data_api_endpoint:manage`. The occupancy one: synchronous CPU **occupies**
/// the worker, tokio cannot preempt it, every other task queued on that worker
/// eats the whole spike, and an author holding `:manage` can press this button
/// in a loop. At 122 ms that is a hiccup. At 28.8 s it is a worker gone for half
/// a minute, per in-flight request. `serving::router` moved this exact
/// evaluation onto `spawn_blocking` for precisely this reason, and preview is
/// not the case that gets to differ: it is the one an author can drive fastest.
///
/// # What bounds it now, since ~122 ms no longer does
///
/// * **on the async runtime: nothing at all.** This function's CPU is zero
///   inline — the render residual the old note accepted moves off with it,
///   which is a strict improvement over what that note settled for;
/// * **on the blocking pool: one thread per in-flight preview**, held for as
///   long as the script runs. That is the bound `spawn_blocking` exists to
///   offer, and it is the honest one: `MAX_WALL_CLOCK` bounds ticks after
///   expiry, not time, so no smaller number would be true.
///
/// # No deadline over the join, deliberately
///
/// Serving wraps its `spawn_blocking` in a request deadline. Preview does not,
/// and the difference is who is waiting. A dropped `JoinHandle` DETACHES — the
/// work keeps its thread and its heap either way — so a deadline here would buy
/// the author an early error while the runaway still ran, and
/// `serving::router`'s own comment records what that costs: answering early
/// **invites the retry**, and each retry stacks another uncancellable thread.
/// A consumer with an SLA is worth that trade. An author who pressed a button
/// and wants to know what their own script did is not: waiting is the correct
/// answer, and pressing again is the wrong one to encourage.
///
/// # The clone
///
/// `spawn_blocking` needs `'static`, so the draft is cloned in. It is bounded
/// by axum's body limit and by `MAX_STATEMENTS`, it happens once per preview,
/// and it is paid immediately before an engine round trip — so it is not
/// measurable against what follows it.
async fn validate_off_the_worker(
    body: &PreviewEndpointInput,
) -> Result<crate::lifecycle::IteratedParams, DataApiError> {
    let sql = body.sql.clone();
    let siblings = body.siblings.clone();
    let params = body.params.clone();
    let output = body.output.clone();
    let joined = tokio::task::spawn_blocking(move || {
        crate::lifecycle::validate_preview(&sql, &siblings, &params, &output)
    })
    .await
    // A panic in the validator, or a runtime shutting down. Neither is the
    // author's doing, so it is a 500 carrying the join error rather than a 400
    // — the same reading `serving::router::transform_outcome` gives it.
    .map_err(|join| {
        egret_core::Error::Internal(anyhow::anyhow!("the draft could not be validated: {join}"))
    })?;
    Ok(joined?)
}

/// Run `body` as `principal` and return what the engine said.
///
/// Every error variant passes through unchanged, which is the deliberate
/// INVERSE of the serving path's `data_access_denied` sanitising: a consumer must
/// not learn the shape of the tables, and an author who owns the SQL cannot fix a
/// syntax error from a sanitised message. That inversion is why this expression
/// must never be reachable below `data_api_endpoint:manage`.
pub(crate) async fn run(
    st: &DataApiState,
    ws: Uuid,
    principal: UserId,
    body: &PreviewEndpointInput,
) -> Result<DataApiPreviewDto, DataApiError> {
    // Workspace containment for a body-supplied id, before anything is rendered
    // or executed. The pair itself is unused — preview consults no cache — but
    // this call is the scope check, so it is not replaceable with a cheaper one.
    let _ = st
        .endpoints
        .connection_per_principal_pair(ws, body.database_id)
        .await?;

    // Returns the shape decision — which parameters arrive as lists — made
    // across `sql` AND `siblings`, i.e. the endpoint, exactly as publish makes
    // it. Taking it from here rather than recomputing it below is what stops
    // this route holding two answers to one question; see `IteratedParams`.
    let iterated = validate_off_the_worker(body).await?;

    // The SAME function serving resolves through, so a value that renders one
    // way here cannot render another way for a consumer — `resolve_supplied_with`
    // is `resolve_supplied` with the union passed in instead of recomputed, and
    // everything after that line is one implementation.
    let resolved =
        crate::serving::params::resolve_supplied_with(&body.values, iterated.names(), &body.params)
            .map_err(|e| param_error(e, &body.params, &body.values))?;

    let (sql, binds) = crate::render::render(&body.sql, &resolved.values)
        .map_err(crate::lifecycle::ValidationError::Render)?;

    let out = st
        .query
        .execute_as_with_params(
            ws,
            body.database_id,
            principal,
            &sql,
            &binds,
            egret_query_api::Limits {
                max_rows: PREVIEW_MAX_ROWS,
                timeout: std::time::Duration::from_secs(PREVIEW_TIMEOUT_SECS),
            },
        )
        .await?;

    Ok(DataApiPreviewDto {
        columns: out.columns,
        rows: out.rows,
        row_count: out.row_count,
        truncated: out.truncated,
        // Read from the body the author is looking at, not from the stored
        // snapshot — for the same reason every other field of this route is:
        // the draft on screen is the thing being previewed, and a transform
        // added since the last Save is exactly the state that needs saying.
        transform_declared: body.output.transform.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A draft whose transform costs real module-evaluation time.
    ///
    /// The work is at the MODULE top level on purpose: `check_transform` runs
    /// `transform::eval_probe`, which evaluates the module and stops — it never
    /// calls the entry point — so work inside `transform()` would cost nothing
    /// here and the test would measure the wrong thing.
    fn slow_draft() -> PreviewEndpointInput {
        PreviewEndpointInput {
            sql: "SELECT 1 AS n".into(),
            siblings: crate::lifecycle::SiblingStatements::default(),
            params: Vec::new(),
            values: BTreeMap::new(),
            output: egret_dataapi_api::OutputSpec {
                transform: Some(egret_dataapi_api::TransformSpec {
                    script: "_warm = [i for i in range(40000)]\n\
                             def transform(params, statements):\n  return {\"total\": 1}"
                        .into(),
                    returns: vec![egret_dataapi_api::ColumnSpec {
                        name: "total".into(),
                        ty: "bigint".into(),
                        required: true,
                    }],
                }),
                ..egret_dataapi_api::OutputSpec::default()
            },
            database_id: Uuid::nil(),
        }
    }

    /// **Validating a draft must RELEASE the async worker**, not occupy it.
    ///
    /// The rule `MAX_DECLARED_PARAMS` states and this task had to redo: inline
    /// synchronous CPU on an async task was accepted at ~122 ms and is not
    /// acceptable at a Starlark evaluation's bound (2 s nominal, 28.8 s
    /// measured). See [`validate_off_the_worker`].
    ///
    /// # The instrument, and its one assumption
    ///
    /// A **current-thread** runtime, so there is exactly one worker and no
    /// second thread can hide the difference. A task is spawned and therefore
    /// QUEUED; on this flavour it can only run when something awaits. If
    /// validation were called inline it would run to completion without ever
    /// yielding, and the flag would still be false afterwards.
    ///
    /// The assumption is that the blocking task has not already finished by the
    /// time its `JoinHandle` is first polled — a ready handle would return
    /// without yielding. That is what the module-level work in [`slow_draft`]
    /// buys, and it is why this test uses a script that costs milliseconds
    /// rather than an empty one.
    #[tokio::test(flavor = "current_thread")]
    async fn validating_a_draft_releases_the_async_worker() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        tokio::spawn(async move { flag.store(true, Ordering::SeqCst) });

        let body = slow_draft();
        let out = validate_off_the_worker(&body).await;
        assert!(
            out.is_ok(),
            "the draft must really validate, or this measures a refusal instead of an \
             evaluation: {:?}",
            out.err()
        );
        assert!(
            ran.load(Ordering::SeqCst),
            "a task queued before the validation had not run by the time it finished, so \
             the validation occupied the only worker instead of releasing it"
        );
    }

    /// The negative control for the test above: with NO transform there is no
    /// Starlark evaluation, and the property must still hold.
    ///
    /// Without this, "released the worker" could be a fact about the script
    /// rather than about the call — and the whole of `validate_preview`
    /// (`check_definition_scans` and `check_render_rules`, the ~122 ms
    /// residual) is what moved, not just the transform check.
    #[tokio::test(flavor = "current_thread")]
    async fn a_draft_with_no_transform_releases_the_worker_too() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        tokio::spawn(async move { flag.store(true, Ordering::SeqCst) });

        let body = PreviewEndpointInput {
            output: egret_dataapi_api::OutputSpec::default(),
            ..slow_draft()
        };
        assert!(
            body.output.transform.is_none(),
            "the control must be untransformed"
        );
        assert!(validate_off_the_worker(&body).await.is_ok());
        assert!(
            ran.load(Ordering::SeqCst),
            "every draft's validation goes to the blocking pool, not only a transformed one"
        );
    }
}
