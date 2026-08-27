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
//! # The statement on screen may be a SCRIPT, and preview shows what it built
//!
//! Package S gave a statement a second source: a Starlark `build_sql(params)`
//! that returns the template. The body carries `sql` or `script` exactly as a
//! stored statement does, and [`DataApiPreviewDto::generated_sql`] is what the
//! script produced — the artefact an author debugging a branching script cannot
//! otherwise see, and the reason this route is worth extending rather than
//! leaving to `Try it`.
//!
//! **Generation happens twice, on two different questions**, and neither is a
//! second generator: [`crate::lifecycle::validate_preview`] generates through
//! `GeneratedSql::build` to ask publish's question of every branch, and
//! [`generate_off_the_worker`] generates through
//! `crate::serving::router::generate_one` to ask serving's question of the
//! author's own test values. That second function's two re-applied rules are
//! load-bearing here, not inherited decoration — see its call site.
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
//! # The one place preview and publish still diverge, and it is about SIBLINGS
//!
//! [`PreviewEndpointInput::siblings`] is a list of SQL **texts**, and package S
//! gave a statement a second source it cannot express: if one of the endpoint's
//! other statements is a `build_sql` script, no client can say so on this wire.
//! Whatever is sent for it is treated as one template and scanned for
//! `{% for %}` headers as though it were SQL. There are THREE consequences and
//! they do not all run the same way:
//!
//! * a sibling script emitting several templates is counted as ONE against the
//!   endpoint's template budget (see `lifecycle::GeneratedSql::build_after`);
//! * a `{% for %}` header that only appears in a sibling script's EMITTED text
//!   never joins the iterated union, so preview resolves a scalar where serving
//!   binds a list;
//! * **and the mirror image of that one, which runs the other way.** A client
//!   that sends a scripted sibling's SOURCE hands the scan a Starlark program,
//!   and a `{% for %}` header sitting in a string literal on a branch the
//!   script never emits joins the union anyway. Preview's set is then WIDER
//!   than the endpoint's — it binds a list where publish and serving bind a
//!   scalar — so preview refuses, or renders the wrong shape for, a draft that
//!   publishes green. Pinned by
//!   `lifecycle::tests::a_scripted_siblings_source_can_make_previews_shape_wider_than_publishs`.
//!
//! The first two are preview green then publish refused. The third is not, and
//! it is the one a client reaches by sending the most obvious thing it has.
//!
//! **It is reachable, and it was already reachable when this paragraph first
//! called it latent.** That sentence said "nothing authors a scripted statement
//! yet", which was a claim about the CONSOLE — and this divergence is a
//! property of the wire, not of the console. `StatementSpec` has accepted
//! `script` on the draft routes since package S Task 1, so an API client could
//! save an endpoint with a scripted sibling and preview it that same day. Task
//! 8 then shipped the editor, so a browser reaches it too. Neither date changed
//! any behaviour; what was wrong was a comment telling the next reader a gap
//! could not be entered.
//!
//! **What the CONSOLE reaches is only the first two**, because `Query.tsx`
//! sends `""` for a scripted sibling rather than its source — it says so, and
//! `a_scripted_siblings_source_can_make_previews_shape_wider_than_publishs`
//! asserts the `""` half from this side so that is a checked property rather
//! than a claim about another language's file. The third belongs to a direct
//! API client, which is the same audience that made this whole divergence
//! reachable on day one.
//!
//! It stays recorded rather than fixed under a task that does not own it:
//! closing it means `siblings` carrying sources rather than strings and
//! generating every one of them per preview — N author scripts per
//! keystroke-driven button — which is a decision about the wire type, not a
//! tidy-up of this module. Recorded here, and on
//! [`crate::lifecycle::validate_preview`], so the "exactly two omissions" above
//! is not read as "and nothing else differs".
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
//! `create` and `PATCH` do not call `check_transform` — the production call
//! sites are [`crate::lifecycle::validate_endpoint`], which `/validate` and
//! `publish` reach, and `EndpointStore::publish` itself, which since the C2
//! fix wave runs the script half of it ABOVE its own `begin()` rather than
//! inside the locked transaction (`lifecycle::ScriptEval`). So a script that
//! does not parse, a `returns` with nothing
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
//! * **one statement's rows.** [`PreviewEndpointInput::source`] is the only
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
use egret_dataapi_api::{OutputSpec, ParamSpec, StatementSource};
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
/// # Unknown keys, `sql`-or-`script`, and why [`Deserialize`] is hand-written
///
/// Package S Task 7 gave this body the same two-case source
/// [`egret_dataapi_api::StatementSpec`] has, flattened onto the object the same
/// way: `{"sql": …}` or `{"script": …}`, never both and never neither. That
/// shape and `#[serde(deny_unknown_fields)]` are the pair serde documents as not
/// composing, so this impl states all three rules itself — exactly what
/// `StatementSpec` does, for the reasons measured there. A body an older client
/// sends is unchanged: `sql` is still `sql`, in the same position, with the same
/// type.
#[derive(Debug, utoipa::ToSchema)]
pub struct PreviewEndpointInput {
    /// What is in the editor right now — not what is stored. **The only
    /// statement this route runs.**
    ///
    /// The author's SQL, or the Starlark script that builds it. A script is
    /// generated here through the same generator publish and serving use, and
    /// what it produced comes back in [`DataApiPreviewDto::generated_sql`].
    #[serde(flatten)]
    pub source: StatementSource,
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
    // # Optional, and an older client omits it
    //
    // A client that never learnt this field must keep working, and its body must
    // keep TODAY'S behaviour exactly: an empty list makes the union
    // `IteratedParams::across(&[sql], …)`, the slice of one that was there
    // before. The default is applied by the hand-written [`Deserialize`] below;
    // `#[serde(default)]` stays on the field because that is what utoipa reads
    // to leave it out of the schema's `required` list, and the two must agree.
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
    // No default, unlike all three fields above it. The reason is
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
    // asserts this at the wire, because the edit to catch is one word in the
    // [`Deserialize`] impl below — `unwrap_or_default()` where `ok_or_else` is —
    // rather than a call anything else would notice.
    pub output: OutputSpec,
    /// The database picker's current value. Workspace-scoped by
    /// `connection_per_principal_pair`, which answers `NotFound` for a database
    /// belonging to another workspace — this id comes from a request body, so it
    /// is exactly the shape that could name someone else's connection.
    pub database_id: Uuid,
}

/// The seven keys a preview body may carry, and the only seven.
const PREVIEW_INPUT_FIELDS: &[&str] = &[
    "sql",
    "script",
    "siblings",
    "params",
    "values",
    "output",
    "database_id",
];

impl<'de> serde::Deserialize<'de> for PreviewEndpointInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error as DeError, MapAccess, Visitor};

        struct PreviewInputVisitor;

        impl<'de> Visitor<'de> for PreviewInputVisitor {
            type Value = PreviewEndpointInput;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a preview body with an `output`, a `database_id` and exactly one of \
                     `sql` or `script`",
                )
            }

            fn visit_map<A>(self, mut map: A) -> Result<PreviewEndpointInput, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut sql: Option<String> = None;
                let mut script: Option<String> = None;
                let mut siblings: Option<crate::lifecycle::SiblingStatements> = None;
                let mut params: Option<Vec<ParamSpec>> = None;
                let mut values: Option<BTreeMap<String, Vec<String>>> = None;
                let mut output: Option<OutputSpec> = None;
                let mut database_id: Option<Uuid> = None;
                // Every key is read, including the ones after the reader already
                // has what it needs: stopping early would accept a trailing typo,
                // and JSON permits a repeated key, which is a malformed object
                // rather than a last-wins assignment.
                macro_rules! once {
                    ($slot:ident) => {{
                        if $slot.is_some() {
                            return Err(DeError::duplicate_field(stringify!($slot)));
                        }
                        $slot = Some(map.next_value()?);
                    }};
                }
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "sql" => once!(sql),
                        "script" => once!(script),
                        "siblings" => once!(siblings),
                        "params" => once!(params),
                        "values" => once!(values),
                        "output" => once!(output),
                        "database_id" => once!(database_id),
                        unknown => {
                            return Err(DeError::unknown_field(unknown, PREVIEW_INPUT_FIELDS))
                        }
                    }
                }
                let source = match (sql, script) {
                    (Some(sql), None) => StatementSource::Sql(sql),
                    (None, Some(script)) => StatementSource::Script(script),
                    (Some(_), Some(_)) => {
                        return Err(DeError::custom(
                            "a statement carries either `sql` or `script`, never both; \
                             `sql` is SQL the author wrote and `script` is a Starlark script \
                             that builds it, so a statement with both has two definitions and \
                             no rule for which one runs",
                        ))
                    }
                    (None, None) => {
                        return Err(DeError::custom(
                            "a statement must carry either `sql`, the SQL the author wrote, \
                             or `script`, a Starlark script that builds it; this one carries \
                             neither",
                        ))
                    }
                };
                Ok(PreviewEndpointInput {
                    source,
                    siblings: siblings.unwrap_or_default(),
                    params: params.unwrap_or_default(),
                    values: values.unwrap_or_default(),
                    // No default, and the whole reason is on the field above:
                    // `OutputSpec::default()` declares no transform, so an
                    // omitted `output` would make every transform refusal
                    // vanish and preview would go green on exactly the drafts
                    // publish refuses.
                    output: output.ok_or_else(|| DeError::missing_field("output"))?,
                    database_id: database_id
                        .ok_or_else(|| DeError::missing_field("database_id"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "PreviewEndpointInput",
            PREVIEW_INPUT_FIELDS,
            PreviewInputVisitor,
        )
    }
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
    /// The template the statement's `build_sql` script returned — `{{ }}`
    /// intact, before any render — or `None` when the author wrote the SQL
    /// themselves (package S Task 7).
    ///
    /// **Showing it is the point of previewing a script at all.** A script that
    /// branches emits a different statement per parameter value, and until this
    /// field the only way an author could see which one their draft produced was
    /// to publish it and read the engine's error. C2's response transform could
    /// not be previewed and this route says so (`transform_declared`); here the
    /// artefact exists, so it is on screen.
    ///
    /// # It is the TEMPLATE, not what the engine ran
    ///
    /// The rendered form has the author's values replaced by bind markers, which
    /// is the one thing they cannot edit and did not write. The template is what
    /// they would have typed by hand, so it is directly comparable with the
    /// editor beside it — and it is the artefact the script is responsible for.
    ///
    /// # It is generated from the author's own test `values`
    ///
    /// Not from publish's representative ones. A preview that showed the
    /// baseline branch while binding the author's values would show a statement
    /// their values do not take — the "told fine, then told no" defect this
    /// route exists to invert, in its quietest form.
    ///
    /// # `None`, not an echo, for a written statement
    ///
    /// The author is looking at that text in the editor. A field that always
    /// carried something would give the UI no way to tell "this is what your
    /// script built" from "this is what you typed".
    pub generated_sql: Option<String>,
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
    let source = body.source.clone();
    let siblings = body.siblings.clone();
    let params = body.params.clone();
    let output = body.output.clone();
    let joined = tokio::task::spawn_blocking(move || {
        crate::lifecycle::validate_preview(&source, &siblings, &params, &output)
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

/// Build this statement's SQL from the author's own test values, through the
/// function the SERVING path generates with.
///
/// `None` when the author wrote the SQL: there is nothing to generate, no
/// Starlark to evaluate, no thread to spawn, and the caller renders exactly the
/// `&str` it rendered before package S existed.
///
/// # Why generate a SECOND time, when validation already generated once
///
/// Because the two answer different questions, and preview is the one route that
/// has to ask both. `crate::lifecycle::validate_preview` generates from
/// `representative_value` and probes with a sentinel — that is publish's
/// question ("is every branch this draft can reach publishable?"). This
/// generates from the values the author typed — serving's question ("what does
/// this endpoint say for THESE parameters?"). A branching script gives two
/// different texts, and the one on screen has to be the second: showing the
/// baseline branch while binding the author's values is a preview of a statement
/// their values do not take.
///
/// Two evaluations of `build_sql` per preview, then. That is not a regression
/// against a one-evaluation design that could have worked: the union of a
/// publish and a serve is what a preview IS, and the shape decision the resolve
/// needs is computed from the SQL, which for a script does not exist until
/// something generates it.
///
/// # `serving::router::generate_one`, not a second generator
///
/// It is `build_sql` plus the two publish rules nothing downstream re-applies —
/// `check_single_select` on the head word and `crate::render::check_dynamic` on
/// the comparison the scanner can miss — and BOTH are needed here for the same
/// reason they are needed there: the branch the author's test values take may be
/// one no probe took, so no validation above has seen this text. Its own comment
/// carries the argument in full.
///
/// Preview needs it in both directions. A generated `DELETE FROM t` would run
/// **as the author**, against a real engine, from a route whose whole error
/// contract is "pass the engine's message through". And a generated template
/// carrying a comparison `check_static` misses would preview GREEN with
/// over-broad rows and serve exactly the same way — a silent wrong answer shown
/// to the one person who could have caught it.
///
/// # A 400, where serving answers 500
///
/// The same refusal means opposite things on the two paths. For a consumer the
/// request was fine and the ENDPOINT failed, so `generate_one` is 500 throughout
/// and says nothing on the wire. Here the script is the thing on screen, the
/// author owns it, and a 500 with a fixed message tells them nothing about the
/// branch they just took. So the detail comes out — the same inversion `run`'s
/// own comment makes about engine errors, and the same one [`param_error`]
/// makes about parameters. `ServingError` is not weakened to do it: the message
/// is read from the `Internal` variant's SOURCE, which that type already keeps
/// for the log.
///
/// # On the blocking pool, for the reason [`validate_off_the_worker`] is
///
/// `build_sql` is author-supplied Starlark and `MAX_WALL_CLOCK` bounds ticks
/// rather than time. Inline, it occupies the only worker its task is on.
async fn generate_off_the_worker(
    body: &PreviewEndpointInput,
    resolved: &crate::serving::params::Resolved,
) -> Result<Option<String>, DataApiError> {
    let Some(script) = body.source.script().map(str::to_owned) else {
        return Ok(None);
    };
    // `spawn_blocking` needs `'static`. Both are bounded by what
    // `validate_preview` has already accepted: the parameters by
    // `MAX_DECLARED_PARAMS`, the script by axum's body limit.
    let params = resolved.json.clone();
    let values = resolved.values.clone();
    let joined = tokio::task::spawn_blocking(move || {
        crate::serving::router::generate_one(&script, &params, &values)
    })
    .await
    .map_err(|join| {
        egret_core::Error::Internal(anyhow::anyhow!(
            "the draft's statement could not be generated: {join}"
        ))
    })?;
    joined.map(Some).map_err(|e| match e {
        // `Internal`'s source is where `generate_one` put the sentence; the
        // variant's own `Display` is the fixed "internal error" a consumer
        // reads, which is not what an author needs.
        crate::serving::ServingError::Internal(source) => {
            egret_core::Error::InvalidInput(source.to_string())
        }
        // Unreachable: `generate_one` constructs `Internal` and nothing else.
        // Answered as a 500 rather than stringified into a 400, because every
        // other variant's `Display` is a fixed consumer-facing sentence — "not
        // found", "unauthorized" — and putting one of those in front of an
        // author as though it were about their script would send them to debug
        // something that did not happen. If this ever fires, the seam moved.
        // Answered rather than `expect`ed for the reason `serving::router` gives
        // its own unreachable arms: a panic inside a request handler is a
        // dropped connection rather than a 500.
        other => egret_core::Error::Internal(anyhow::anyhow!(
            "generating a statement failed in a way generation cannot produce: {other}"
        )),
    })
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

    // What the author's own test values make this statement say. `None` for
    // written SQL — there is nothing to generate — and the script's answer
    // otherwise. See [`generate_off_the_worker`] for why it is generated a
    // second time here rather than taken from the validation above.
    let generated = generate_off_the_worker(body, &resolved).await?;
    let template = match &generated {
        Some(sql) => sql.as_str(),
        None => body
            .source
            .sql()
            // `StatementSource` is a two-case enum with "neither"
            // unrepresentable, so a source that generated nothing carries SQL.
            .expect("a statement that is not a script carries SQL"),
    };

    let (sql, binds) = crate::render::render(template, &resolved.values)
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
        generated_sql: generated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A body carrying `source` plus whatever `extra` says, as bytes.
    ///
    /// Text rather than `serde_json::json!`, because two of the rules below are
    /// about keys a `Value` cannot hold: a repeated key is dropped by
    /// `serde_json::Map` before any of this code would see it.
    fn body(source: &str, extra: &str) -> String {
        format!(
            "{{{source}{extra}\"output\":{{}},\
             \"database_id\":\"00000000-0000-0000-0000-000000000000\"}}"
        )
    }

    fn parse(json: &str) -> Result<PreviewEndpointInput, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The two accepted shapes, and that each reaches the right case.
    ///
    /// The control for every refusal below: without it, a `Deserialize` that
    /// refused everything would satisfy all four of them.
    #[test]
    fn a_body_carries_sql_or_script_and_the_case_says_which() {
        let written = parse(&body("\"sql\":\"SELECT 1\",", "")).expect("`sql` is a valid body");
        assert_eq!(written.source, StatementSource::Sql("SELECT 1".into()));
        let scripted =
            parse(&body("\"script\":\"def build_sql(p): pass\",", "")).expect("`script` too");
        assert_eq!(
            scripted.source,
            StatementSource::Script("def build_sql(p): pass".into())
        );
        // …and the three optional fields really are optional, at their
        // documented defaults.
        assert!(written.params.is_empty());
        assert!(written.values.is_empty());
        // `SiblingStatements` has no public length (its text is private to
        // `lifecycle`), so the default is asserted where it is observable: an
        // absent `siblings` validates exactly as an empty one does.
        assert_eq!(
            crate::lifecycle::validate_preview(
                &written.source,
                &written.siblings,
                &written.params,
                &written.output,
            ),
            crate::lifecycle::validate_preview(
                &written.source,
                &crate::lifecycle::SiblingStatements::default(),
                &written.params,
                &written.output,
            ),
        );
    }

    /// **A typo is a refusal, not a silently-ignored field.**
    ///
    /// This is the rule `#[serde(deny_unknown_fields)]` used to give this type
    /// for free, and it is load-bearing rather than tidy. `"sibling"` for
    /// `"siblings"` reads as an endpoint with no other statements, so the shape
    /// decision is the union of ONE — preview goes green and publish refuses the
    /// same draft for `SequenceBind`. Told "fine", then told "no", which is the
    /// one thing this route exists not to do.
    #[test]
    fn an_unknown_key_is_refused() {
        let typo = body("\"sql\":\"SELECT 1\",", "\"sibling\":[\"SELECT 2\"],");
        let err = parse(&typo).expect_err("a misspelt field must not be ignored");
        assert!(
            err.to_string().contains("sibling"),
            "the message must name the key the author typed: {err}"
        );
        // The correctly spelled field is accepted, so the refusal is about the
        // spelling and not about the field existing.
        assert!(parse(&body(
            "\"sql\":\"SELECT 1\",",
            "\"siblings\":[\"SELECT 2\"],"
        ))
        .is_ok());
    }

    /// Exactly one of `sql`/`script` — never both.
    ///
    /// Both is two definitions with no rule for which one runs. The message is
    /// `StatementSpec`'s own, because it is the same mistake about the same pair
    /// of keys and an author who met it once should not meet a second wording.
    #[test]
    fn a_body_carrying_both_sql_and_script_is_refused() {
        let err = parse(&body(
            "\"sql\":\"SELECT 1\",\"script\":\"def build_sql(p): pass\",",
            "",
        ))
        .expect_err("both is not a statement");
        assert!(
            err.to_string().contains("never both"),
            "the author must be told which of the two to delete: {err}"
        );
    }

    /// …and never neither.
    ///
    /// The direction that matters: a body with no statement at all must not
    /// deserialise into one holding an empty string, which would preview a
    /// blank statement and blame the engine for the syntax error.
    #[test]
    fn a_body_carrying_neither_sql_nor_script_is_refused() {
        let err = parse(&body("", "")).expect_err("neither is not a statement either");
        assert!(
            err.to_string().contains("neither"),
            "the message must say what is missing: {err}"
        );
    }

    /// A repeated key is a malformed object, not a last-wins assignment.
    ///
    /// `{"sql": …, "sql": …}` has two statements in it and no rule for which one
    /// the author meant. Asserted for `sql` and for a field with a default,
    /// because the two go through different arms of the same visitor.
    #[test]
    fn a_repeated_key_is_refused() {
        assert!(
            parse(&body("\"sql\":\"SELECT 1\",\"sql\":\"DELETE FROM t\",", "")).is_err(),
            "a repeated `sql` must not resolve to one of the two"
        );
        assert!(
            parse(&body(
                "\"sql\":\"SELECT 1\",",
                "\"params\":[],\"params\":[],"
            ))
            .is_err(),
            "a repeated key with a default is still a repeated key"
        );
    }

    /// A draft whose transform costs real module-evaluation time.
    ///
    /// The work is at the MODULE top level on purpose: `check_transform` runs
    /// `transform::eval_probe`, which evaluates the module and stops — it never
    /// calls the entry point — so work inside `transform()` would cost nothing
    /// here and the test would measure the wrong thing.
    fn slow_draft() -> PreviewEndpointInput {
        PreviewEndpointInput {
            source: StatementSource::Sql("SELECT 1 AS n".into()),
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
