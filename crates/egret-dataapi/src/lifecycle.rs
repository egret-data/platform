//! The publish lifecycle: validate, publish, roll back, disable, re-enable.
//!
//! This is the module that makes versioning real rather than decorative. A draft
//! ([`crate::endpoints`]) is edited freely; publishing FREEZES it into an
//! immutable `dataapi.endpoint_versions.snapshot` row, including the output
//! column schema resolved by [`egret_query_api::QueryExecPort::describe_as`],
//! and points
//! `endpoints.live_version_id` at it. Nothing an author does to the draft
//! afterwards can reach a published snapshot.
//!
//! # The contract P4's serving path must honour
//!
//! **Everything that decides what a request RETURNS comes from the snapshot** —
//! `sql`, `params`, `output`, `cache`, `limits`, `rate_limit` and the frozen
//! `columns`. Reading any of those from `dataapi.endpoints` would make an edit to
//! a draft change the behaviour of every client already calling the published
//! URL, which is exactly what versioning exists to prevent.
//!
//! **Routing is the one thing that comes from the endpoint row**, and
//! deliberately so. P4 must match an inbound request against
//! `dataapi.endpoints (method, path)` restricted to `state = 'live'`, and then
//! serve entirely from the snapshot reached through `live_version_id`. That pair
//! of columns is the one carrying `UNIQUE (workspace_id, method, path)`, so it is
//! the only place where "two endpoints claiming one URL" is impossible. The
//! snapshot's own `method`/`path` are the record of where that version was
//! published, not a routing key: routing on them would lose the constraint, and
//! an author who moved a live endpoint and then created a second one at the old
//! URL would have two live snapshots answering to the same path with nothing to
//! break the tie.
//!
//! That answers the question `endpoints.rs` left open for this task. A draft edit
//! CAN still move a live endpoint's URL, and that is the decision, not an
//! oversight: a URL is a property of the endpoint object (uniquely constrained,
//! renameable), whereas behaviour is a property of the version (frozen).
//!
//! **The snapshot-external input P4 must re-check is a PAIR:**
//! `EndpointSnapshot::delegated` and `EndpointSnapshot::impersonate` record the
//! two mutable things on `query.databases` that decide whether the engine runs a
//! query AS THE CALLER — see [`runs_per_principal`], which is the single
//! expression of that condition. Switching either on after a
//! `cache.scope = shared` version was published turns a legitimately-published
//! snapshot into the exact cross-principal leak [`check_cache_scope`] refuses —
//! with no re-validation anywhere. P4 must compare BOTH frozen halves against the
//! live row before serving from a shared cache, and refuse when either disagrees.
//! Everything else in the snapshot is self-contained; these two are a witness,
//! and a witness nobody questions is decoration.
//!
//! **It was `delegated` alone, and half a witness was worse than none** — it read
//! as though the question were being asked. `options.impersonate` makes the
//! engine run as the caller independently of `delegated`, so a connection could
//! be per-principal at the engine and look shared-safe to every check that
//! consulted `delegated`.
//!
//! # Why validation runs here and not at serve time
//!
//! Every check below refuses a configuration that would otherwise be discovered
//! by a client, in production, as a 500 — or, in the `cache.scope = shared` case,
//! not discovered at all, because leaking one principal's rows to another looks
//! exactly like a cache hit. Refusing at publish time means the unsafe shape can
//! never enter `endpoint_versions` in the first place.
//!
//! # Scope of the SQL checks — what they do NOT claim
//!
//! There are two layers here and they are not the same KIND of thing. Conflating
//! them is what produced a SEV-0.
//!
//! ## Layer 1 — [`reject_unscannable_bytes`], a proof
//!
//! Two bytes are refused in the RAW definition, with no lexing at all:
//!
//! * `;` anywhere but a trailing position.
//! * `#` anywhere.
//!
//! A byte test cannot disagree with an engine's lexer, because it makes no claim
//! about what the bytes mean. This is the layer that carries the
//! "only one statement" guarantee, and the only claim in this file that survives
//! the discovery of a new dialect quirk.
//!
//! ## Layer 2 — the scanner, a deny-list
//!
//! [`strip_comments_and_strings`] elides comments and quoted regions so
//! [`check_single_select`] can look at the head word and [`check_params`] can tell
//! whether a `{{param}}` stood where a VALUE stands. It is a shape check over a
//! scanner, not a parser. It is still not a proof, and the disclosed gaps below
//! are still open. What changed is the BLAST RADIUS: a layer-2 mistake can now
//! only mis-classify a single statement, never smuggle a second one.
//!
//! # Why `#` is refused rather than stripped — the SEV-0 of record
//!
//! `#` used to be stripped to end-of-line as a "free" standing guard for a future
//! MySQL-compatible front end. **The strip was the hole.** This definition passed
//! validation:
//!
//! ```text
//! SELECT 1 AS a #) AS _egret_ds LIMIT 0 ; DROP TABLE revtest.t ; SELECT * FROM (SELECT 1
//! ```
//!
//! The scanner read `#` as a comment, stripped to EOL, and saw `SELECT 1 AS a` —
//! a clean single SELECT. StarRocks reads `#` as **nothing**: accepted and
//! IGNORED, so it sees every byte. `Connector::describe` wraps its argument as
//! `SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0`
//! (`egret-query/src/execution/connector/mod.rs`, StarRocks override in
//! `connector/starrocks.rs`), so `#)` closed the wrapper's own paren and consumed
//! its `AS _egret_ds LIMIT 0`; the `;` became a **top-level** separator; and the
//! trailing `SELECT * FROM (SELECT 1` re-opened a subquery for the wrapper's
//! suffix to close. Three valid statements, the middle one a `DROP TABLE`.
//!
//! **Measured against live StarRocks 4.1.3 through the real connector: the table
//! was dropped.** It fired at `POST …/validate` — no publish — under the
//! connection's effective identity, reachable with `data_api_endpoint:manage`,
//! which migration `0122` grants to every `ws_admin`. Trino was unaffected: `#`
//! is a genuine parse error there.
//!
//! **The lesson, which is the point of this section.** A defensive strip is not
//! conservative. Stripping can only ever hide text from the CHECK; it can never
//! hide text from the ENGINE. So modelling a comment form that no supported
//! engine has is not a free precaution in the safe direction — it is a
//! guaranteed blind spot, and the cost was written off as "free" for four rounds
//! precisely because the direction was never stated.
//!
//! The corollary, applied above: when the engine's reading of a byte is
//! uncertain, REFUSE the byte. Do not model it.
//!
//! ## What the previous justification got wrong
//!
//! It said the describe wrapper was the guarantee — "an embedded `;` inside that
//! subquery is an error on the engine". That is true only while the `;` STAYS
//! inside the subquery, and the attacker's whole move is to close the subquery
//! first. The wrapper was never a boundary; it was a string the attacker could
//! finish.
//!
//! **What actually holds now**, in order of strength:
//!
//! 1. No `;` outside a trailing position can reach an engine from a definition,
//!    because layer 1 refuses the byte. Nothing about comments, literals or
//!    wrappers is assumed.
//! 2. Independently, `StarRocksConnector::describe` applies the same byte test to
//!    whatever it is handed, from any caller — Data API validate, dataset
//!    preview, typegen. So a future caller that forgets to validate is still
//!    covered.
//! 3. The describe wrapper is a convenience for resolving columns. It is **not**
//!    a security boundary and must never again be cited as one.
//!
//! **What was measured and does NOT hold: the MySQL capability flag.** The
//! obvious fix was to stop negotiating `CLIENT_MULTI_STATEMENTS` in the vendored
//! `mysql_async`. It was tried and it does nothing — StarRocks 4.1.3 executes
//! every statement in a submission with the flag cleared, and with
//! `CLIENT_MULTI_RESULTS` cleared as well. Pinned by
//! `egret-query/tests/connector_starrocks.rs::starrocks_ignores_the_multi_statement_capability`,
//! which fails if that ever changes. Detecting a second RESULT SET afterwards is
//! not a fix either: the `DROP TABLE` has already run by the time its packet
//! arrives. Detection is not prevention.
//!
//! # What the scanner still models, and on what evidence
//!
//! A `{{param}}` is refused inside:
//!
//! | Form | Scanner treats it as | Evidence, and how it was gathered |
//! |---|---|---|
//! | `'…'` literal | region ending at `''`, or at `\` under one of [`ESCAPING_READINGS`] | not engine-specific: both readings are checked, so neither dialect's rule is assumed |
//! | `"…"` / `` `…` `` identifier | region ending at the matching quote, same escape rules | as above |
//! | `--` comment | comment to LF or CR | Trino `SIMPLE_COMMENT` = `'--' ~[\r\n]*` (grammar). StarRocks 4.1.3, **`mysql -c`**: `SELECT 1 AS a --x AND nonexistent_col` → `a=1` (so the rest of the line IS commented out, and no trailing whitespace is required). **The CR terminator is measured too**, on the same engine and client — not inferred from the Trino rule that happens to agree with it |
//! | `/* … */` comment | comment to the first `*/` | nesting not modelled; leftovers can only over-reject |
//!
//! `#` no longer has a row, and that is the fix: it is refused, not modelled.
//! Every remaining row models a form that is real on at least one supported
//! engine. **A future edit that adds a row for a form no engine has is
//! reintroducing this bug** — the guard against that is that such a form should
//! go in [`reject_unscannable_bytes`] instead.
//!
//! **The methodological rule these rows are held to**, learned from `#` being
//! described wrongly in three successive rounds (a comment ending at CR or LF;
//! then LF only; then a syntax error): an error message names the token the
//! parser choked on, not the construct you were probing. The
//! `ERROR 1064: Unexpected input 'x'` that looked like proof `#` was illegal was
//! the engine objecting to `x` in `#x AND nonexistent_col`. **A row is only as
//! good as a measurement whose SUCCESS case distinguishes the hypotheses.**
//! `SELECT 1 #x` returning a column named `x` does that — it is compatible with
//! "`#` is ignored" and with nothing else. `mysql -c` (comments sent to the
//! server, as `mysql_async` does) is the only client mode that measures StarRocks
//! rather than the CLI.
//!
//! # Deliberate false rejections
//!
//! Layer 1 refuses bytes without asking whether they are inert, so it refuses
//! some legitimate SQL. Each is a clear 400 at authoring time, and each has a
//! spelling that works:
//!
//! * `WHERE label = 'a;b'` → `concat('a', char(59), 'b')`.
//! * `SELECT 1 -- note; more` → drop the `;` from the comment.
//! * any `#`, including `SELECT '#tag'` → `concat(char(35), 'tag')`.
//!
//! These are accepted deliberately. Deciding that a particular `;` or `#` is
//! inert means agreeing with the engine's lexer, which is precisely the
//! agreement that failed.
//!
//! # Known gaps in layer 2
//!
//! None of these can smuggle a second statement any more — layer 1 stops that
//! independently. They are mis-classifications of a SINGLE statement, and all
//! need `data_api_endpoint:manage` (an authoring permission) rather than a
//! serve-time token:
//!
//! * MySQL/StarRocks `/*! … */` executable comments are treated as ordinary block
//!   comments, so their contents are stripped rather than scanned. Measured on
//!   live StarRocks 4.1.3 **both with and without `-c`**: `/*!, 2 AS b */`,
//!   `/*!50000, …*/` and `/*!00000, …*/` all return ONE column either way, so the
//!   client flags never mattered here and that engine does not execute them. Kept
//!   on the list only because it is version-dependent and MySQL-family behaviour
//!   has historically differed.
//! * `WITH … DELETE` and `SELECT … INTO OUTFILE` pass the head-word check.
//! * `SELECT 1;;` passes, because `trim_end_matches(';')` strips every trailing
//!   semicolon rather than exactly one. Trailing separators cannot separate
//!   anything, so this is cosmetic.
//! * A Trino literal ending in a backslash (`'a\'`) is falsely REJECTED, because
//!   the escaping reading cannot terminate it. Rejection is the safe direction.
//!
//! # What is not validated at all
//!
//! `output`, `limits` and `rate_limit` have **no validation rule anywhere** — not
//! before the lock, not after it, not at serve time. The post-lock re-validation
//! ([`EndpointStore::publish`]) did not close that, and cannot: it re-runs
//! the endpoint validator, which covers the statements, `params` and `cache`
//! only. What
//! those three fields get from the lock is CONSISTENCY — they are frozen from the
//! same read that was validated — not validation. A nonsensical `max_rows` or an
//! unknown `output.shape` will be frozen exactly as written.

use std::collections::BTreeMap;

use egret_core::{Error, Result, UserId};
use egret_dataapi_api::{
    compare_column_types, CacheScope, CacheSpec, ColumnSpec, EndpointSnapshot, LimitsSpec,
    OutputSpec, ParamLocation, ParamSpec, ParamType, StatementSnapshot, StatementSpec,
    TypeAgreement, PARAM_GATE,
};
use egret_query_api::Param;
use uuid::Uuid;

use crate::endpoints::{DataApiEndpointDto, EndpointStore};
use crate::render::ParamValues;

/// Everything `validate` can refuse, as a value rather than a string, so a caller
/// can match on the reason and the tests can pin the specific rule that fired.
///
/// Every arm renders as [`Error::InvalidInput`] — a 400 the author can act on.
/// None of these is a 500: they are all statements about the definition the
/// author just wrote.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValidationError {
    #[error(
        "only one statement may be published: a `;` may appear only at the very end of the \
         definition. A `;` elsewhere is refused even where it looks inert — inside a string \
         literal or a comment — because deciding that it is inert means agreeing with the \
         engine's lexer, and that agreement is exactly what cannot be assumed"
    )]
    MultipleStatements,
    /// The statement LIST is malformed: an empty list, a name that is not an
    /// identifier, or two statements sharing one.
    ///
    /// # Folded in through `From` rather than surfaced as its own type
    ///
    /// [`egret_dataapi_api::validate_statements`] is also run by the create and
    /// patch routes, which map it to a 400 directly. Publish runs it again, and
    /// carrying it as an arm here rather than as a second error type at the
    /// publish boundary is deliberate: everything [`validate`] and
    /// [`EndpointStore::publish`] can refuse a definition for already converges
    /// on one `?`-convertible type, and a caller that had to remember a second
    /// one is a caller that can forget it. `RenderError` is folded in the same
    /// way one arm below, for the same reason.
    ///
    /// It is passed through transparently, so the author reads the rule's own
    /// message rather than a publish-flavoured paraphrase of it — the same
    /// sentence they would have got from the route that refused the write.
    #[error(transparent)]
    Statement(#[from] egret_dataapi_api::StatementError),
    #[error(
        "`#` may not appear in a published definition — delete it. StarRocks ACCEPTS AND IGNORES \
         `#` (`SELECT 1 #x` returns a column named `x`), so any meaning read into it here is a \
         guess the engine does not share; Trino has no `#` token at all, so it is a parse error \
         there. A definition that needs the character can build it with `char(35)`"
    )]
    HashNotAllowed,
    #[error("only SELECT (or a leading WITH … SELECT) may be published; found `{0}`")]
    NotASelect(String),
    #[error("malformed parameter placeholder: {0}")]
    MalformedPlaceholder(String),
    #[error("unterminated {0}; the statement is not valid SQL")]
    Unterminated(&'static str),
    #[error("the query references `{{{{{0}}}}}` but no parameter named `{0}` is declared")]
    UndeclaredPlaceholder(String),
    #[error("parameter `{0}` is declared but never referenced as `{{{{{0}}}}}` in the query")]
    UnreferencedParameter(String),
    #[error("parameter `{0}` is declared more than once")]
    DuplicateParameter(String),
    #[error(
        "parameter `{{{{{0}}}}}` is referenced from inside a string literal, a quoted identifier \
         or a comment. A placeholder there is not a value position: the renderer emits a bind \
         marker the engine reads as text, so the value would be bound to a position that does \
         not exist — and on a renderer that interpolates instead, the substituted literal closes \
         the enclosing one and lets any caller inject SQL. Write the placeholder as an operand \
         instead, e.g. `LIKE concat('%', {{{{{0}}}}}, '%')`"
    )]
    PlaceholderInsideQuotedText(String),
    // Names BOTH fields, because an author told only "delegated" on a connection
    // whose `delegated` is false has been told something they can verify is
    // untrue, and will go looking for a different problem.
    #[error(
        "cache.scope = shared is not allowed on a connection that runs queries as the caller \
         (delegated, or options.impersonate): the engine filters rows per principal, so a \
         shared cache would serve one caller's rows to another"
    )]
    SharedCacheOnPerPrincipalConnection,
    #[error("enum parameter `{0}` must declare a non-empty `allowed` list")]
    EnumWithoutAllowed(String),
    #[error(
        "enum parameter `{param}` reaches the statement as TEXT — it is used in an identifier \
         position through `| ident` — so every one of its `allowed` values must be an identifier: \
         letters, digits and `_`, not starting with a digit, optionally one dot. `{value}` is not. \
         An `allowed` entry is written into the statement verbatim and is the one thing a \
         definition can carry that no other rule scanned"
    )]
    EnumValueNotIdentifier { param: String, value: String },
    /// Everything the renderer refuses, at publish time: a definition that does
    /// not parse under the hardened environment, a capture construct, `| ident`
    /// on anything that is not an enum parameter with an `allowed` list, a
    /// comparison against a bare parameter, and the behavioural gate's
    /// "this definition INSPECTS a parameter".
    ///
    /// Passed through with the renderer's own message. Those messages are
    /// AUTHOR-facing by design — several of them quote the definition back —
    /// which is exactly right here and is why the serving path maps the same
    /// type to a generic failure instead.
    #[error("{0}")]
    Render(#[from] crate::render::RenderError),
    #[error(
        "parameter `{0}` is declared with `location: body`, which cannot be published. A REQUIRED \
         body parameter is a permanent 400 — nothing on the serving surface reads a request body, \
         so it is never supplied — and an OPTIONAL one is worse: it renders as absent, its \
         `{{% if %}}` block is omitted, and the endpoint returns UNFILTERED rows with a 200 on \
         every call. Declare it as `location: query`"
    )]
    BodyParameter(String),
    #[error(
        "parameter `{0}`'s `default` is not a valid `{1}`. The default is coerced by exactly the \
         same rule as a caller's value, so an unparseable one makes every call that OMITS this \
         parameter a 400 — blaming the caller for the definition, and logging it as `param_invalid` \
         so nothing points back at the endpoint"
    )]
    DefaultNotValidForType(String, &'static str),
    /// The `default` is well-typed and the constraint set is sound (spec §5:
    /// `check_constraints` already refused an unsound set before this is ever
    /// reached), and the default still fails one of its own declared
    /// constraints — `minimum`, `pattern`, `not_before`, and so on.
    ///
    /// Kept separate from [`Self::DefaultNotValidForType`] because the two
    /// point an author at different fields: a type failure says "your
    /// `default` isn't a `{ty}`", which is false here and sends them looking
    /// in the wrong place for a `minimum` problem.
    #[error(
        "parameter `{param}` has a `default` that violates its own `{constraint}` \
         of `{bound}`. A default that the endpoint would refuse means every call \
         omitting the parameter is a 400"
    )]
    DefaultViolatesConstraint {
        param: String,
        constraint: &'static str,
        bound: String,
    },
    /// A constraint field present on a `ParamSpec` whose declared `ty` cannot
    /// carry it — `pattern` on a `Boolean`, `minimum` on a `String`, and so on.
    ///
    /// Refused rather than ignored: a `pattern` that silently does nothing
    /// teaches the author a model where constraints apply everywhere, and the
    /// next one they write silently does nothing too.
    #[error(
        "parameter `{param}` declares `{constraint}`, which does not apply to a \
         `{ty}` parameter. Remove it, or change the parameter's type"
    )]
    ConstraintNotForType {
        param: String,
        constraint: &'static str,
        ty: &'static str,
    },
    /// A lower bound strictly above its own upper bound — `minimum > maximum`,
    /// `min_length > max_length`, or `not_before > not_after`.
    ///
    /// Strictly greater, never "not less": a parameter pinned to exactly one
    /// value (`minimum == maximum`) is a real thing an author writes.
    #[error("parameter `{param}` declares a `{constraint}` above its upper bound")]
    BoundsInverted {
        param: String,
        constraint: &'static str,
    },
    /// A `minimum`/`maximum`/`not_before`/`not_after` that does not parse in
    /// the shape its own constraint requires.
    ///
    /// # Why this exists, and why it is not folded into [`ValidationError::BoundsInverted`]
    ///
    /// Every one of these four is compared as TEXT (see `check_constraints`),
    /// and every one of those comparisons is sound only against a normalised
    /// spelling: `minimum`/`maximum` through
    /// [`crate::serving::params::compare_decimal_text`]'s digit-by-digit
    /// grammar, `not_before`/`not_after` through the zero-padded, big-endian
    /// spelling `coerce` produces (the restriction
    /// `check_temporal_bounds` documents for the serving side). The author's
    /// raw string has not been through either normalisation: an unpadded
    /// `"2026-9-1"` sorts AFTER `"2026-10-1"` as text despite being
    /// chronologically earlier, and a string that is not a decimal or a date
    /// at all (`"abc"`, `"not-a-date"`) sorts by Unicode code point, telling
    /// the author nothing about their actual bounds. So each bound is
    /// validated (and, for `not_before`/`not_after`, normalised) the same way
    /// `coerce` would before the pair is compared, and a bound that fails is
    /// refused HERE, naming which one, rather than compared as arbitrary
    /// bytes.
    #[error("parameter `{param}` declares `{constraint}` as `{value}`, which is not {expected}")]
    ConstraintBoundUnparseable {
        param: String,
        constraint: &'static str,
        value: String,
        /// A noun phrase completing "which is not …" — `"a valid decimal
        /// number"`, `"a valid date"`, `"a valid timestamp"`.
        expected: &'static str,
    },
    /// A `minimum`/`maximum` written with more digits than the generated
    /// contract can state exactly — refused at publish, where the author can
    /// still change it, rather than published as a bound nobody wrote.
    ///
    /// # Why a digit cap exists at all
    ///
    /// The bound is stored and compared as TEXT, so egret itself enforces it
    /// exactly at any width. The generated OpenAPI document cannot: JSON
    /// Schema's `minimum`/`maximum` take a JSON NUMBER, and this crate does
    /// not enable serde_json's `arbitrary_precision`, so a magnitude past
    /// `u64` goes through an `f64` on the way into the document. Measured:
    /// `minimum: "1234567890123456789012345678901234567890"` reaches the
    /// document as the nearest double, exactly
    /// `1234567890123456846996462118072609669120` — a HIGHER minimum than
    /// `coerce` enforces, so a generated client refuses values the server
    /// accepts. The same rounding on `maximum` goes the other way
    /// and makes the published contract more permissive than the code. Past
    /// `f64`'s range the keyword cannot be represented at all and vanishes,
    /// leaving a document that describes a wider endpoint than exists.
    ///
    /// Refusing here is the cheap correct fix: the document stays lossless by
    /// construction, and no runtime comparison is touched — in particular
    /// [`crate::serving::params::is_plain_decimal`] is unchanged, so this
    /// changes nothing about what a CALLER may send.
    #[error(
        "parameter `{param}` declares a `{constraint}` of `{value}`, which is {digits} digits \
         wide; at most {max} are accepted. The generated OpenAPI document states this bound as \
         a JSON number, and past that width a JSON number cannot hold it exactly — the \
         published contract would name a bound you never wrote, and a generated client would \
         disagree with the endpoint about which values are legal. Round the bound to {max} \
         digits or fewer"
    )]
    ConstraintBoundTooManyDigits {
        param: String,
        constraint: &'static str,
        value: String,
        digits: usize,
        max: usize,
    },
    /// A `pattern` that does not compile as a regex at all — the author's typo,
    /// not the caller's problem, so it is refused at publish rather than on
    /// whichever request happens to hit `coerce` first.
    #[error("parameter `{param}` declares a `pattern` that is not a valid regex: {detail}")]
    PatternInvalid { param: String, detail: String },
    /// A `pattern` that compiles, but past [`MAX_PATTERN_PROGRAM_BYTES`].
    ///
    /// `regex`'s own default cap is 10 MB of compiled program. A validation
    /// rule on an endpoint parameter that needs megabytes of compiled program
    /// is not a validation rule, and the cap is checked here — at publish —
    /// rather than compiled per request.
    #[error(
        "parameter `{param}` declares a `pattern` too large to compile. A validation \
         rule that needs this much compiled program is not a validation rule"
    )]
    PatternTooLarge { param: String },
    #[error(
        "parameter `{0}` is optional with no `default`, and the definition does not render without \
         it. An absent parameter is `none` in the template, so a construct that INSPECTS it — \
         `{{{{ {0} | ident }}}}` is the usual one — fails on every call that omits it, as a 500. \
         Guard it with `{{% if {0} %}}`, give it a `default`, or mark it `required`"
    )]
    OptionalParameterUnguarded(String),
    #[error(
        "a parameter appears in `LIMIT`/`OFFSET`. Neither engine accepts a bind marker there — \
         StarRocks rejects `LIMIT ?` at prepare time — so the endpoint would fail on every call. \
         Publish the limit as a constant, or apply it with `{{% if %}}`"
    )]
    ParameterInLimit,
    #[error(
        "a parameter appears in the select list without a `CAST`. Neither engine infers a bind \
         marker's type in a projection — Trino types it as `unknown`, StarRocks as `VAR_STRING` — \
         so the published column type would be frozen wrong. Write `CAST({{{{p}}}} AS <type>)`"
    )]
    UncastParameterInProjection,
    #[error(
        "the engine resolved no output columns for this definition. An endpoint with no columns \
         has no contract, and freezing an empty schema would make every call return empty objects \
         with a 200 — on StarRocks the text describe path answers exactly that, with no error, for \
         a statement containing a bind marker"
    )]
    DescribeReturnedNoColumns,
    /// One name, two types, across the branches publish described.
    ///
    /// There is no union of two types that is honest: declaring either one is a
    /// lie on the calls that take the other branch, and that lie is served as a
    /// `200` — the frozen contract says `varchar` and the row carries an
    /// integer. So the definition is refused, and the author is told both of the
    /// two repairs that exist.
    #[error(
        "column `{name}` is `{first}` in one branch of this definition and `{second}` in \
         another. A published contract cannot declare two types for one name — give the \
         branches a `CAST` to the same type, or return them under different names"
    )]
    ColumnTypeConflict {
        name: String,
        first: String,
        second: String,
    },
    /// One BRANCH returning two columns of the same name.
    ///
    /// # Regardless of their types, and that is the whole point
    ///
    /// This started as the same-name-DIFFERENT-type case, split off from
    /// [`ValidationError::ColumnTypeConflict`] so an author with no `{% if %}`
    /// in their definition would not be told to go and look at their branches.
    /// But the condition was the wrong one: the rationale below is about the
    /// NAME, and it does not care what the types are.
    ///
    /// `SELECT a.id, b.id FROM a JOIN b` with **both** `bigint` used to publish
    /// green. The union deduplicates by name to one column, and every call — not
    /// some branch, every one — is then a `503 schema_drift`, because a live
    /// result carrying two columns of one name is something no JSON object can
    /// express: a `Map` keeps the last value, so one of the two would simply
    /// vanish from every row. **The clause that refuses it moved in M1** and is
    /// worth naming, because the old one no longer exists: until M1 `shape_rows`
    /// compared `out.columns.len() == 2` against `snap.columns.len() == 1` and
    /// the refusal came out of that arithmetic. Its predicate is containment
    /// now and compares no counts, so the repeat is refused by an explicit
    /// duplicate-name clause instead. Same outcome, and this refusal does not
    /// depend on which one is doing it. That
    /// is the publish-green-then-always-fail class this whole gate exists to
    /// remove, one keystroke away from the differing-type case.
    ///
    /// So the check is on the name alone, and it runs per described branch
    /// before the union fold rather than inside it: a duplicate is a property of
    /// one branch's describe output, not of the fold.
    #[error(
        "this definition returns two columns named `{name}`. A JSON object has one key per \
         name, so only one of them could ever be served — alias them apart, e.g. \
         `SELECT a.{name} AS a_{name}, b.{name} AS b_{name}`"
    )]
    AmbiguousColumnName { name: String },
    /// More structurally distinct renderings than publish will describe.
    ///
    /// Refusing, not truncating. Truncating drops whichever branch fell off the
    /// end, and a branch that is not described is a column missing from the
    /// union — which is exactly the `503 schema_drift` this whole gate exists to
    /// remove. A loud publish is the honest failure; a quiet one is the bug.
    ///
    /// # `branches` is the ENDPOINT's total, summed across its statements
    ///
    /// Spec §4.1.1. Publish describes every branch of every statement, so a
    /// per-statement limit would let ten statements cost ten times the engine
    /// round trips one is allowed — the budget would grow with exactly the thing
    /// it exists to bound. The message says so, because with M2 the two words
    /// mean different things: a *statement* is one of the endpoint's SQL
    /// definitions, a *branch* is one rendered permutation of one of them, and
    /// an author told only "this renders to 114 branches" would go looking for
    /// 114 in one of their statements and not find it.
    #[error(
        "this endpoint renders to {branches} structurally distinct branches across its \
         statements, and publish describes every one of them against the engine so the frozen \
         contract covers every branch a caller can reach. The limit is {limit}, and it is the \
         endpoint's total rather than each statement's. Each optional parameter with no \
         `default` adds the branch its absence takes, and each one whose value can flip an \
         `{{% if %}}` — a boolean especially — multiplies the rest: give some of them a \
         `default` or mark them `required` so absence is not a separate branch, or split this \
         into two endpoints"
    )]
    TooManyBranches { branches: usize, limit: usize },
    /// More declared parameters than publish will render a definition for.
    ///
    /// # Why a second limit, when [`ValidationError::TooManyBranches`] already caps the cost
    ///
    /// Because that one caps the ENGINE half and is measured after the work.
    /// Publish renders every branch to find out how many there are, and the
    /// rendering is synchronous CPU on an async task — so a definition with
    /// enough parameters paid seconds of it and was then refused anyway.
    /// Measured in release, all-boolean, every parameter guarding its own
    /// block: 34 ms at 20 parameters, 284 ms at 50, 1.14 s at 100, 4.4 s at 200,
    /// **18.2 s at 400** — every one of them refused, after paying in full, and
    /// blocking a runtime worker rather than merely the caller.
    ///
    /// Nothing bounded the parameter count before this: the only `params.len()`
    /// in the whole validator was a `Vec::with_capacity`. So the cost is bounded
    /// at its own source, by a scan, before the first render.
    #[error(
        "this definition declares {declared} parameters, and the limit is {limit}. Publish \
         renders the definition once for every branch those parameters can take, so checking \
         it costs more the more of them there are. An endpoint with this many knobs is really \
         two endpoints: split it, or fold related filters into one `enum` parameter"
    )]
    TooManyParameters { declared: usize, limit: usize },
    /// A conditional definition under `output.shape: "arrays"`.
    ///
    /// `shape_rows` (`serving/router.rs`) zips the live row and the frozen
    /// column list by INDEX, and `data_schema` (`openapi_gen.rs`) documents the
    /// order in prose rather than in the schema, because OpenAPI 3.0 has no
    /// tuple type. Both exist because a positional row has no way to name which
    /// column a given slot holds. A definition whose branches describe
    /// different column lists has no single order to freeze — `[[1]]` under a
    /// declared `[id, name, email]` is ambiguous about which of the three is
    /// missing — so it is refused here rather than served under whichever
    /// branch happened to be described first.
    #[error(
        "this definition returns different columns depending on its parameters, which \
         `output.shape: arrays` cannot express — a positional row has no way to say which \
         column is missing. Use `output.shape: objects`, where a column is simply absent"
    )]
    BranchingArraysShape,
    /// Two DECLARED column names that `output.naming` maps onto one key.
    ///
    /// # This package is what made a cross-branch collision possible
    ///
    /// Four places in this crate share the "what is a column" concept, and
    /// three of them apply `apply_naming`: `shape_rows` refuses a collision
    /// over the frozen union with `503 output_naming_collision`, and
    /// `data_schema` refuses it with `OpenApiGenError::NamingCollision`. Only
    /// the publish-side duplicate check did not — it compared RAW names.
    ///
    /// Before the union, the frozen list was one branch's columns, so a
    /// collision needed both spellings inside one projection and the raw-name
    /// check caught it. `SELECT {% if d %}total_revenue{% else %}totalRevenue
    /// {% endif %} FROM t` under `naming: camel` puts both names in the union
    /// with neither branch repeating a name, publishes green, and then:
    ///
    /// * every call, on either branch, is `503 output_naming_collision`; and
    /// * the generated OpenAPI fails at the DOCUMENT level, taking every
    ///   unrelated endpoint in the workspace down with it — the one failure
    ///   here whose blast radius is not this endpoint.
    ///
    /// So it is refused where the author is standing, and both declared names
    /// are quoted back, because renaming either one is the repair.
    #[error(
        "columns `{first}` and `{second}` both become `{key}` under \
         `output.naming: {naming}`, and a JSON object has one key per name — so one of them \
         could never be served. They come from different branches of this definition, which \
         is why neither branch looks wrong on its own. Alias one of them apart, or set \
         `output.naming: as_is`"
    )]
    OutputNamingCollision {
        key: String,
        first: String,
        second: String,
        naming: String,
    },
    /// `output.transform.script` exceeds [`MAX_TRANSFORM_SCRIPT_BYTES`].
    ///
    /// A response transform reshapes what a statement already returned; it is
    /// not where an endpoint's business logic belongs. Carries both numbers —
    /// see every other `…TooLong`/`…TooMany` arm in this enum for why "too
    /// large" alone tells an author nothing about how much to cut.
    #[error(
        "output.transform.script is {bytes} bytes, over the {cap} byte cap. A response \
         transform reshapes what a statement already returned — it is not where an \
         endpoint's business logic belongs"
    )]
    TransformScriptTooLong { bytes: usize, cap: usize },
    /// `output.transform` is set alongside a non-default `output.shape` or
    /// `output.naming`.
    ///
    /// Once a script decides the shape of the response, `shape` (which rows the
    /// wire carries) and `naming` (which key spelling a JSON object gets) are
    /// either dead — the script's own `return` already fixed both — or
    /// ambiguous: does `naming: camel` apply to the rows the script SEES, or to
    /// the keys it WROTE? There is no reading that is obviously right, so this
    /// refuses rather than picks one.
    ///
    /// The field's DEFAULT value (`"objects"` for `shape`, `"as_is"` for
    /// `naming`) is not refused alongside a transform: an author who never
    /// touched either field has not asked for a shape, and refusing that would
    /// make every transform carry two fields it has no opinion about.
    #[error(
        "output.{0} may not be set alongside output.transform. A transform script decides \
         the shape of its own response, so output.{0} would either be dead or ambiguous \
         about whether it applies before or after the script runs. Remove output.{0}, or \
         leave it at its default"
    )]
    TransformWithShapeOrNaming(&'static str),
    /// `output.transform.returns` is empty.
    #[error(
        "output.transform.returns declares no columns. Serving checks a transform's response \
         against this list; an empty list is nothing to check against, and serving would fail \
         OPEN — serving whatever the script returned, unchecked — rather than refuse a \
         response that disagrees with nothing"
    )]
    TransformReturnsEmpty,
    /// Two entries in `output.transform.returns` declare the same `name`.
    #[error(
        "output.transform.returns declares `{0}` more than once. A JSON object has one key \
         per name, so a duplicate is not a stricter contract, it is an unresolvable one"
    )]
    TransformReturnsDuplicateName(String),
    /// `output.transform.script` does not parse under the transform dialect, or
    /// fails the same probe evaluation [`crate::transform::run_transform`]
    /// itself runs the module through.
    ///
    /// The author's 400, not the caller's 500: an endpoint publishes only after
    /// its script has been through the one evaluator serving will use, so a
    /// script this rejects can never reach a caller as a runtime failure.
    #[error("the transform script is not valid: {0}")]
    TransformScriptInvalid(String),
    /// A publish-time probe ran the script and what came back does not satisfy
    /// `output.transform.returns` — or the script failed outright on input
    /// publish knows it will be given.
    ///
    /// `context` names **which** probe. There are two ([`probe_transform`]) and
    /// they differ only in whether the statements carry a row, so a refusal that
    /// did not say which one an author is being told about would send them
    /// looking at the wrong half of their script — the `if len(rows) == 0`
    /// branch is the commonest place the two disagree, and it is invisible in
    /// the other probe's output.
    #[error("{context}: {detail}")]
    TransformOutputMismatch {
        /// Which probe produced this, in the words the probe is documented
        /// under: "the single-row probe" or "the empty-rows probe".
        context: String,
        /// What was wrong with what came back — from
        /// [`crate::transform::check_returns`] when the script ran and returned
        /// something that does not match `returns`, or the
        /// [`crate::transform::TransformError`] when it did not run at all.
        detail: String,
    },
    /// The transform's output GROWS with the rows it is given, and projecting
    /// that growth to the endpoint's own `limits.max_rows` puts it over
    /// [`crate::transform::MAX_TRANSFORM_OUTPUT_BYTES`].
    ///
    /// # What this projection proves, and what it does not
    ///
    /// It is measured, not assumed: `per_row` is the difference between the two
    /// probes [`probe_transform`] already runs — the serialised size with one
    /// row minus the size with none — so a script that aggregates (`return
    /// {"total": len(rows)}`) measures zero growth and is never refused here,
    /// however large `max_rows` is.
    ///
    /// It is an **estimate**, and it can be wrong in both directions. Saying so
    /// precisely matters more than the number:
    ///
    /// * **it can under-project.** Growth is assumed linear from two points, so
    ///   a script whose output grows faster than its input — a cross join, a
    ///   nested expansion — is projected too small and reaches serving anyway.
    /// * **it can over-project.** The probe row carries **every** column of the
    ///   branch UNION at a stand-in value as small as the column's type permits.
    ///   A real row can therefore be SMALLER than the probe row rather than
    ///   larger: `serving::router::shape_rows` omits a `required: false` column
    ///   from an `objects` row when the branch that ran did not return it, so a
    ///   response can carry a strict subset of the union the probe assumed. It
    ///   was previously claimed here that a real row is "larger, not smaller" —
    ///   that was wrong under M1, and this arm is not allowed to justify itself
    ///   with it.
    /// * **it cannot see saturation.** `return {"sample": rows[:10]}` grows for
    ///   the first ten rows and not after; two points have no way to find the
    ///   ceiling above them. Such an endpoint is refused although it would have
    ///   served.
    ///
    /// What remains true is the thing the refusal rests on: an endpoint whose
    /// own `limits.max_rows` says it may return N rows, carrying a script that
    /// measurably grows with rows, cannot honour both that number and the cap.
    /// `limits.max_rows` is the author's own declaration, and `fits` is the
    /// largest value of it this script can actually serve.
    #[error(
        "output.transform returns about {per_row} more bytes per row, so at \
         limits.max_rows = {max_rows} it would return about {projected} bytes, over the \
         {cap} byte cap — and the caller would get a 500 rather than a truncated \
         response. Lower limits.max_rows to at most {fits}, or aggregate in the script \
         instead of passing rows through; both change WHAT the endpoint returns and not \
         only how much of it — a lower limit truncates the result set, and aggregating \
         replaces the rows with a summary. The size is projected from ONE synthetic row \
         carrying every column of the union, not measured on your data, so a response \
         that omits an optional column can be smaller than this and a script that \
         expands its input larger"
    )]
    TransformOutputExceedsRowLimit {
        /// Serialised bytes the transform's output grew by when the probe's
        /// statements went from no rows to one.
        per_row: usize,
        /// The endpoint's `limits.max_rows`, **clamped exactly as serving
        /// clamps it** — see [`probe_transform`]. Projecting against an
        /// unclamped `u32` would refuse an endpoint over rows serving would
        /// never fetch.
        max_rows: u32,
        /// `base + per_row * max_rows`, where `base` is what the script returns
        /// with no rows at all.
        projected: usize,
        /// [`crate::transform::MAX_TRANSFORM_OUTPUT_BYTES`], repeated so the
        /// message is self-contained.
        cap: usize,
        /// The largest `limits.max_rows` this script's measured growth fits
        /// under `cap`.
        ///
        /// **At least 1, never 0.** `run_transform` refuses an output over
        /// `cap` outright (`CappedSink`), so a probe that RETURNED a value
        /// satisfies `populated <= cap`; therefore
        /// `cap - empty >= populated - empty = per_row` and the division below
        /// is at least one. A zero here would also be advice an author cannot
        /// take — `serving::router` clamps `max_rows` to a minimum of 1, so
        /// "lower it to 0" names a value the endpoint cannot hold.
        fits: u32,
    },
}

impl From<ValidationError> for Error {
    fn from(e: ValidationError) -> Self {
        Error::InvalidInput(e.to_string())
    }
}

/// Remove `--` line comments, `/* … */` block comments, and quoted regions,
/// leaving structure behind.
///
/// This exists so the statement-separator scan in [`check_single_select`] looks
/// only at SQL structure. Without it the naive `contains(';')` gets **both**
/// interesting cases backwards: `SELECT 'a;b'` — a perfectly good query — is
/// rejected, and `SELECT 1 -- ; DROP TABLE t` sails through because the scan
/// never learns the `;` was commented out. Stripping first answers both.
///
/// # One scanner cannot serve two dialects — so it is run twice
///
/// The engines disagree about exactly one thing: inside a literal, StarRocks
/// treats `\` as an escape and Trino does not (only `''` escapes there). A single
/// scan has to pick one, and **either pick is exploitable**, in opposite
/// directions:
///
/// * Reading `\` as an *ordinary character* ends a StarRocks literal early, so
///   text StarRocks would still be inside a literal for is scanned as structure.
/// * Reading `\` as an *escape* ends a Trino literal late, so structure Trino
///   would execute is scanned as text — and text is not scanned at all. That is
///   the direction that hides a second statement:
///   `SELECT 'a\' ; DROP TABLE t -- '` closes on the FINAL quote under this
///   reading, leaving `SELECT ''`, while Trino sees a complete literal `'a\'`,
///   a separator, and `DROP TABLE t`.
///
/// So `backslash_escapes` is a parameter, [`check_single_select`] runs the whole
/// check under **both** values, and a definition is accepted only if both
/// readings say the same thing. Nothing is assumed about which engine is behind
/// the endpoint, and the check does not have to be kept in sync with a dialect
/// table that would go stale the day a third engine lands. It costs one extra
/// linear pass, and it over-rejects only SQL whose literals actually contain
/// `\'` — noted as a deferred false-rejection.
///
/// An unterminated quoted region or block comment is refused rather than
/// swallowed, for the same reason: under the escaping reading an unterminated
/// literal runs to the end of the input and everything after it disappears from
/// the scan. Refusing is also the honest answer, since an unterminated literal is
/// a syntax error on both engines.
///
/// A `--` comment ends at `\r` as well as `\n`. Trino's lexer is
/// `'--' ~[\r\n]* '\r'? '\n'?`, so a bare CR ends the comment there; stopping
/// only at LF would let `SELECT 1 --x\r; DROP TABLE t` past a scan that never saw
/// the `;`. A `#` region ends at `\n` only, and is modelled on no engine at all —
/// see the arm's own comment, which carries the measurement and the false
/// rejection it costs.
///
/// Double-quoted and backtick-quoted regions are elided as well: they are
/// identifiers on Trino and (in some modes) strings on StarRocks, and either way
/// a `;` inside one is not a statement break. Each quoted region is replaced by
/// `''` rather than deleted, so adjacent tokens do not run together — and so
/// [`check_params`] can tell that a `{{param}}` fell inside one.
///
/// Nested block comments (which Postgres allows and Trino does not) end at the
/// first `*/`; the leftovers can only cause a rejection, never an acceptance.
/// `pub(crate)` for `render::structural_placeholders`, which counts the bind
/// markers standing in SQL structure in a RENDERED statement — the same question
/// this answers for a placeholder in an unrendered one.
pub(crate) fn strip_comments_and_strings(
    sql: &str,
    backslash_escapes: bool,
) -> std::result::Result<String, ValidationError> {
    let b = sql.as_bytes();
    // Bytes, not chars: every byte this function branches on is ASCII, and a
    // multi-byte UTF-8 sequence is copied whole (its continuation bytes are all
    // >= 0x80 and match no branch), so the output is always valid UTF-8.
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // NOTE: there is deliberately NO `#` arm. `#` is refused outright by
            // `reject_unscannable_bytes` before this function ever runs, because
            // stripping it was a REMOTE-DDL HOLE, not a free precaution — see
            // that function.
            // `--` — a line comment on both engines, ending at CR as well as LF.
            //
            // Trino's `SIMPLE_COMMENT` is `'--' ~[\r\n]* '\r'? '\n'?`, and
            // StarRocks follows it — measured with `mysql -c`, so the client is
            // not the one doing the commenting: `SELECT 1 AS a --x AND
            // nonexistent_col` returns `a=1` on 4.1.3, and a bare CR terminates.
            // So `--` needs no trailing whitespace there, contrary to MySQL's
            // rule; both facts fall out of the same Presto lexer rule rather than
            // being two coincidences. This is the one line-comment form that is
            // real on both engines, and the only one the scanner must get exactly
            // right.
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
                out.push(b' ');
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                if i >= b.len() {
                    return Err(ValidationError::Unterminated("block comment"));
                }
                i += 2;
                out.push(b' ');
            }
            q @ (b'\'' | b'"' | b'`') => {
                i += 1;
                let mut closed = false;
                while i < b.len() {
                    if backslash_escapes && b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        // A doubled quote is an escaped quote, not the end.
                        if b.get(i + 1) == Some(&q) {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    return Err(ValidationError::Unterminated("quoted literal"));
                }
                // A neutral, structurally valid stand-in, so `'a'||'b'` does not
                // become `||` with its operands merged into it.
                out.extend_from_slice(b"''");
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    // Infallible by construction (see above); the fallback is there so a future
    // edit that breaks the invariant degrades instead of panicking.
    Ok(String::from_utf8(out)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// The two escaping dialects every scan below must satisfy SIMULTANEOUSLY.
///
/// `true` is the StarRocks reading (a backslash escapes inside a literal),
/// `false` the Trino one. A definition is publishable only if it is safe under
/// both, because the endpoint's engine can be changed after publish —
/// `database_id` is editable on the draft, and a connection's engine is not this
/// crate's to pin.
///
/// These are the only two readings needed, because [`egret_query_api::Engine`]
/// has exactly two variants — `Trino` and `StarRocks`. The pair is complete for
/// anything this code can target; a third engine would need a third reading here
/// and a fresh row in the terminator table in the module comment.
const ESCAPING_READINGS: [bool; 2] = [true, false];

/// **The only rule in this file that is a PROOF rather than a deny-list.**
///
/// Two bytes are refused in the RAW definition, before any scanning happens.
/// Nothing here lexes, so nothing here can disagree with an engine's lexer —
/// which is the failure mode that produced this rule.
///
/// # `;` — anywhere but the very end
///
/// A definition with no `;` byte outside a trailing position cannot be split
/// into two statements, on any engine, by any lexer, ever. That is a property of
/// the bytes. Contrast the scan below, which decides whether a `;` is inert by
/// modelling comments and literals: correct only while the model agrees with the
/// server, and catastrophic the moment it does not.
///
/// It did not. `SELECT 1 AS a #) AS _egret_ds LIMIT 0 ; DROP TABLE t ; SELECT * FROM (SELECT 1`
/// passed [`check_single_select`], because the scanner read `#` as a comment and
/// saw only `SELECT 1 AS a`. StarRocks reads `#` as nothing at all, so `#)`
/// closed `Connector::describe`'s own `SELECT * FROM ( … ) AS _egret_ds LIMIT 0`
/// wrapper, the `;` became a TOP-LEVEL separator, and the trailing
/// `SELECT * FROM (SELECT 1` re-opened a subquery for the wrapper's suffix.
/// Three valid statements. **Measured against live StarRocks 4.1.3: the table
/// was dropped, at `POST …/validate`, with no publish, under the connection's
/// effective identity.**
///
/// The cost is a real false rejection: `WHERE label = 'a;b'` is refused even
/// though that `;` genuinely is inert. Taken deliberately — proving it inert
/// requires the lexer agreement this rule exists in order not to need. `char(59)`
/// builds the character when an author truly needs it.
///
/// # `#` — anywhere at all
///
/// `#` was previously STRIPPED to end-of-line as a "free" standing guard for a
/// future MySQL-compatible front end. **That strip is what created the hole
/// above**: it made the scanner blind to text the server executes. The direction
/// matters — a strip can only ever hide text from the check, never from the
/// engine, so modelling a comment form no engine has is not a conservative
/// choice, it is the dangerous one.
///
/// Refusing outright is safe in the same way a `;` refusal is: `#` is not a
/// comment on StarRocks (accepted and IGNORED — `SELECT 1 #x` returns a column
/// named `x`) and not a token at all on Trino (`SqlBase.g4` lexes no `#`;
/// `UNRECOGNIZED : .;` is the catch-all), so no legitimate definition needs it
/// outside a literal, and inside a literal `char(35)` serves.
///
/// This also settles the question three review rounds parked as open. It is not
/// open. Banning is required, and the "banning is BLOCKED by three assertions"
/// note was reading the tests as a constraint on the design rather than as a
/// record of it — those assertions were asserting the vulnerability.
fn reject_unscannable_bytes(sql: &str) -> std::result::Result<(), ValidationError> {
    if sql.contains('#') {
        return Err(ValidationError::HashNotAllowed);
    }
    if sql.trim_end().trim_end_matches(';').contains(';') {
        return Err(ValidationError::MultipleStatements);
    }
    Ok(())
}

/// Reject anything that is not exactly one read-only statement.
///
/// Two layers, in this order:
///
/// 1. [`reject_unscannable_bytes`] — a byte test on the RAW definition. This is
///    the layer that carries the multi-statement guarantee.
/// 2. The scan below — strips comments and string literals, then checks the head
///    word. A leading `WITH … SELECT` (CTE) is allowed; anything else (DML, DDL,
///    `SET`) is refused. Run under both entries of [`ESCAPING_READINGS`]; see
///    [`strip_comments_and_strings`] for why one pass is not enough.
///
/// Layer 2 is still a deny-list and still not a proof — see the module comment's
/// disclosed gaps. What changed is that its failures are no longer able to
/// smuggle a statement, because layer 1 does not depend on it being right.
pub fn check_single_select(sql: &str) -> std::result::Result<(), ValidationError> {
    reject_unscannable_bytes(sql)?;
    for backslash_escapes in ESCAPING_READINGS {
        check_single_select_under(sql, backslash_escapes)?;
    }
    Ok(())
}

fn check_single_select_under(
    sql: &str,
    backslash_escapes: bool,
) -> std::result::Result<(), ValidationError> {
    let stripped = strip_comments_and_strings(sql, backslash_escapes)?;
    let trimmed = stripped.trim().trim_end_matches(';').trim();
    if trimmed.contains(';') {
        return Err(ValidationError::MultipleStatements);
    }
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if head != "SELECT" && head != "WITH" {
        return Err(ValidationError::NotASelect(head));
    }
    Ok(())
}

/// Every `{{ name }}` reference in `sql`, in order of appearance.
///
/// Scans the RAW SQL, not the stripped form, because the renderer that
/// substitutes these at serve time — [`crate::render::render`] — is a template
/// engine that has no idea what SQL quoting is: it emits a bind marker wherever
/// the placeholder stands, string literal or not. Matching that view is what
/// keeps validation honest.
///
/// **The comment this replaces named `egret-query`'s
/// `execution::template::render`.** That renderer is SQL Lab's, it interpolates
/// through `quote_string`, and nothing on the Data API serving path has ever
/// called it — the rule below was written against a renderer that was never
/// wired in. The rule itself is right and P4.5 made it load-bearing: a
/// placeholder that survives [`strip_comments_and_strings`] stands where a value
/// stands, which is exactly where a bind marker may go.
/// # It reports EXPRESSIONS, not names — the T7-3 reconciliation
///
/// This used to require the whole inside of a `{{ … }}` to be a bare
/// identifier and to return it as a parameter name. That was correct against
/// the renderer it was written for, which only ever substituted `{{name}}`, and
/// P4.5 made it wrong: `{{ sort | ident }}` came back as an undeclared
/// placeholder called `"sort | ident"` and a loop variable `{{ r }}` as one
/// called `"r"`, so **spec §4.3's own canonical example could not be
/// published** — and Task 6's suite had to shadow a parameter with its own loop
/// variable to work around it.
///
/// So the two questions are now answered by the two instruments that can answer
/// them. *Which names does this definition read?* is minijinja's, through
/// [`crate::render::context_names`]. *Where in the SQL does a reference stand?*
/// is this scan's, and it needs no opinion at all about what is inside the
/// braces: an occurrence is an occurrence, and comparing occurrence counts
/// before and after [`strip_comments_and_strings`] is what tells a value
/// position from a quoted one. The expression text is carried only so the error
/// can quote the reference back to the author.
fn placeholders(sql: &str) -> std::result::Result<Vec<String>, ValidationError> {
    let mut found = Vec::new();
    let mut rest = sql;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .ok_or_else(|| ValidationError::MalformedPlaceholder("unterminated `{{`".into()))?;
        let raw = &after[..close];
        let expr = raw.trim();
        // An empty `{{}}` is the one shape still refused here: minijinja does
        // refuse it too, but this scan has to key each occurrence on something,
        // and an empty key would make two of them indistinguishable.
        if expr.is_empty() {
            return Err(ValidationError::MalformedPlaceholder("{{}}".into()));
        }
        found.push(expr.to_string());
        rest = &after[close + 2..];
    }
    Ok(found)
}

/// Which contract a parameter scan is enforcing.
///
/// The whole difference is [`ValidationError::UnreferencedParameter`], and it is
/// the only rule in [`check_params`] that is about the CONTRACT an endpoint
/// publishes rather than about the statement that runs. A declared parameter no
/// placeholder reads is a lie in the generated OpenAPI — a query parameter a
/// caller can send that changes nothing — so publish must go on refusing it. It
/// costs a preview nothing: the render simply never looks the name up.
///
/// Preview exists so an author sees what is on screen (spec §2.2), and the most
/// ordinary intermediate state of that loop is SQL edited before the declarations
/// catch up, or the reverse. Requiring the two halves to agree made preview
/// refuse exactly the draft it was built to run — measured in a browser against a
/// real engine, not reasoned about.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum ParamRules {
    /// Every rule, including the published-contract one. What
    /// [`validate_definition`] — `POST …/validate` and publish — enforces.
    Published,
    /// Every rule EXCEPT [`ValidationError::UnreferencedParameter`]. What
    /// [`validate_preview`] enforces over an unsaved draft.
    Draft,
}

/// Every `{{param}}` resolves to a declared parameter, and every declared
/// parameter is referenced. Both directions matter: an unreferenced parameter
/// is a lie in the generated OpenAPI, and an undeclared placeholder is a 500
/// waiting to happen at serve time.
///
/// This entry point enforces the PUBLISHED contract. Preview reaches the same
/// body through [`ParamRules::Draft`], which drops the unreferenced-parameter
/// rule and nothing else; the rules live here once, so the two cannot drift.
///
/// Duplicate declarations are refused too. Two `region` parameters would render
/// as one query parameter in OpenAPI while the renderer silently picked the
/// first — so the type or the `allowed` list a caller is validated against would
/// depend on declaration order.
///
/// # A placeholder must stand where a VALUE stands
///
/// The third rule is the one that carries a privilege boundary rather than a
/// correctness property, and its attacker is not the author: it is any holder of
/// a Data API token, at serve time, long after this ran.
///
/// A placeholder is replaced by a bind marker, and — before P4.5, on the
/// renderer these rules were written against — by `quote_string(engine, v)`, an
/// **already-quoted** literal. Either is safe exactly where a literal is what the
/// grammar expects, and neither is safe inside an open one. Take the
/// interpolating reading first, because it is the one that shows the injection:
///
/// ```sql
/// SELECT * FROM t WHERE a = 'x{{p}}y'
/// ```
///
/// With `p = "' OR 1=1 -- "` this renders to `WHERE a = 'x''' OR 1=1 -- 'y'`,
/// which tokenises as the literal `x'`, then a LIVE ` OR 1=1 `, then a comment.
/// A predicate-level break-out on both dialects — no second statement, so
/// [`check_single_select`] would never have seen it, and neither would any
/// downstream multi-statement guard. `probe_sql` renders the same template to
/// `'x''y'`, which describes perfectly, so publish succeeds too.
///
/// The test is cheap and total: a placeholder that stands where a value stands
/// SURVIVES [`strip_comments_and_strings`], and one that does not — because the
/// quoted region or comment containing it was replaced wholesale — did not stand
/// there. Comments are covered by the same rule and need it: a value substituted
/// into `/* {{p}} */` can carry `*/` and close the comment early.
///
/// Checked under both entries of [`ESCAPING_READINGS`], because which regions
/// count as quoted is exactly what the two dialects disagree about.
pub fn check_params(sql: &str, params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    check_params_with(sql, params, ParamRules::Published)
}

/// [`check_params`], told which contract it is enforcing.
///
/// One body, so a rule added below is in both publish and preview by
/// construction, and the ORDER the rules report in is the same for both — which
/// is what keeps `validate_definition_runs_the_statement_check_first` and every
/// test pinning a specific variant honest.
fn check_params_with(
    sql: &str,
    params: &[ParamSpec],
    rules: ParamRules,
) -> std::result::Result<(), ValidationError> {
    // Redundant inside `validate_definition`, which runs `check_single_select`
    // first — deliberately so. Both are `pub`, and the placeholder rule below is
    // a scan; a `#` reaching it would put the scanner back in the business of
    // deciding what the engine ignores. Cheap, and it means neither entry point
    // depends on being called in the right order.
    reject_unscannable_bytes(sql)?;
    let mut seen: Vec<&str> = Vec::with_capacity(params.len());
    for p in params {
        if seen.contains(&p.name.as_str()) {
            return Err(ValidationError::DuplicateParameter(p.name.clone()));
        }
        seen.push(&p.name);
    }
    // The RAW scan, matching what `crate::render::render` does at serve time: a
    // placeholder inside a literal still emits a bind marker there, so it counts
    // as referenced — pretending otherwise would report it as an unused
    // parameter and send the author to delete the declaration rather than move
    // the placeholder.
    let referenced = placeholders(sql)?;
    // Which NAMES the definition reads is minijinja's answer, not this scan's —
    // see [`placeholders`]' comment for why the two questions were split. It
    // covers names in `{% if %}` conditions and filter arguments, which the old
    // scan could not see at all, and it excludes loop targets and `{% set %}`
    // bindings, which the old scan reported as undeclared parameters.
    let read = crate::render::context_names(sql)?;
    for name in &read {
        if !seen.contains(&name.as_str()) {
            return Err(ValidationError::UndeclaredPlaceholder(name.clone()));
        }
    }
    // The published-contract rule, and the ONE rule preview omits — see
    // [`ParamRules`]. Its position in this sequence is unchanged, so which error
    // `validate_definition` reports for a draft breaking several rules at once is
    // unchanged too.
    if rules == ParamRules::Published {
        check_every_parameter_is_read(&read, params)?;
    }
    for backslash_escapes in ESCAPING_READINGS {
        let stripped = strip_comments_and_strings(sql, backslash_escapes)?;
        // `unwrap_or_default` rather than `?`: stripping only ever removes text,
        // so it cannot manufacture a placeholder — and if it somehow yielded a
        // malformed one, "fewer survived" is the answer we want anyway.
        let survived = placeholders(&stripped).unwrap_or_default();
        if let Some(name) = first_count_mismatch(&referenced, &survived) {
            return Err(ValidationError::PlaceholderInsideQuotedText(name));
        }
    }
    Ok(())
}

/// Every declared parameter is read by something, given the names that
/// something reads.
///
/// **The `read` list is the caller's to widen, and that is the whole reason
/// this is a function rather than four lines inside [`check_params_with`].**
/// This rule is the one part of the parameter scan that is about the CONTRACT
/// an endpoint publishes rather than about the statement that runs, so it is
/// also the one part that an endpoint with several statements has to ask
/// differently: a `sort` that only the rows query reads is not an unused query
/// parameter (spec §4.2). [`check_params_with`] passes one statement's names
/// and [`validate_endpoint`] passes the union across all of them; the
/// PREDICATE is written once, so the two cannot drift into disagreeing about
/// what "unreferenced" means.
fn check_every_parameter_is_read(
    read: &[String],
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    for p in params {
        if !read.iter().any(|r| r == &p.name) {
            return Err(ValidationError::UnreferencedParameter(p.name.clone()));
        }
    }
    Ok(())
}

/// The first name occurring a different number of times in `referenced` than in
/// `survived`.
///
/// Counted rather than compared as sets: `WHERE a = {{q}} OR b = '{{q}}'` has
/// `q` in both lists, and only the COUNT reveals that one of the two references
/// was inside a literal.
fn first_count_mismatch(referenced: &[String], survived: &[String]) -> Option<String> {
    referenced
        .iter()
        .find(|name| {
            let n = |v: &[String]| v.iter().filter(|x| x == name).count();
            n(referenced) != n(survived)
        })
        .cloned()
}

/// Does this connection make the engine run a query as the CALLER?
///
/// **The one predicate the shared-cache rules are actually about**, and the
/// reason it is a named function rather than an inline `&&` at two call sites:
/// those two sites disagreeing is precisely the defect this replaces. Both
/// [`check_cache_scope`] (publish time) and
/// [`crate::serving::shared_cache_allowed`] (serve time) route through it, so
/// "safe to share a cache entry" cannot mean two different things at the two
/// ends of a published endpoint's life.
///
/// **`delegated` alone is NOT this predicate**, which is what the earlier
/// version got wrong. `delegation_for`'s non-delegated arm still returns
/// `Some(principal_email)` as the session user
/// (`egret-query/src/execution/mod.rs:86`), and each connector then decides for
/// itself on `options.impersonate`:
///
/// * Trino (`connector/trino.rs:43`) sends `X-Trino-User: <caller>` when
///   `bearer_token.is_some() || options.impersonate == Some(true)`;
/// * StarRocks (`connector/starrocks.rs:129`) issues
///   `EXECUTE AS <caller> WITH NO REVERT` when
///   `bearer_token.is_none() && options.impersonate == Some(true)`.
///
/// The two database fields are independent and unvalidated — `create`/`update`
/// accept `{"delegated": false, "options": {"impersonate": true}}` — so a
/// connection can be per-principal at the engine while reading as non-delegated
/// to anything that consults `delegated`. That connection plus
/// `cache.scope: shared` was a cross-identity leak that both checks waved
/// through.
///
/// OR, not AND: either field on its own is sufficient to make the engine filter
/// per principal, and this must stay the conservative direction.
///
/// # The fix NOT taken, and why it is worth someone's time later
///
/// The obvious upstream fix is to stop the combination existing: have the
/// databases route reject `{"delegated": false, "options": {"impersonate":
/// true}}` at registration, since a connection that impersonates without being
/// marked delegated is arguably always a misconfiguration. That is defensible
/// and it is deliberately not done here — it changes an existing public API's
/// accepted inputs, which is outside P4's scope and would break any deployment
/// already carrying that shape, silently, at the next `update`.
///
/// Recorded rather than dropped: if someone does take it, this predicate stays
/// regardless. A registration-time check constrains rows written AFTER it, and
/// this one is the thing that keeps already-stored rows and hand-edited ones
/// from leaking. Defence at the decision point, not only at the door.
pub fn runs_per_principal(delegated: bool, impersonate: bool) -> bool {
    delegated || impersonate
}

/// Read `options.impersonate` out of a `query.databases.options` JSONB value.
///
/// This crate depends on no other feature seam (see the module comment), so
/// `ConnOptions` is not in scope and the column is read as raw JSON. The key is
/// spelled exactly as `egret_query_api::ConnOptions::impersonate` serialises it —
/// that field carries no `#[serde(rename)]`, so the wire name is the Rust name.
/// **That is a coupling by string**, which is why it lives in one function with
/// a test rather than being inlined at each of the three call sites.
///
/// **The test in THIS crate is the weaker half of the guard, deliberately
/// paired.** It can only assert that this crate still spells what it always
/// spelled; it cannot see a rename in `ConnOptions`, which is the change that
/// would break it — every consumer would keep compiling while this function
/// silently started returning `false`, re-permitting a `shared` cache on a
/// connection that runs queries as the caller. The guard that catches THAT is
/// `wire_names` in `egret-query-api/src/lib.rs`, raised by the crate that owns
/// the type at the point of the rename. Both are needed: one pins the reader,
/// the other pins the writer.
///
/// The real close is a shared type across the two crates, and it is
/// deliberately not done: it would cross the seam boundary that keeps this
/// crate free of `egret-query` (see the module comment), which is a P5-or-later
/// architectural decision rather than something to change while fixing a leak.
///
/// Anything that is not literally JSON `true` reads as `false`: absent, `null`,
/// the string `"true"`, a number. That is the fail-SAFE direction for the
/// PUBLISH check and the fail-OPEN direction for nothing — the serving-side
/// comparison uses the same function on both the frozen and the live value, so a
/// value this cannot parse is simply "not impersonating" at both ends and the
/// pair still agrees. If `ConnOptions` ever starts accepting a looser spelling,
/// this function is the one place that has to learn it.
pub fn impersonate_flag(options: &serde_json::Value) -> bool {
    options.get("impersonate") == Some(&serde_json::Value::Bool(true))
}

/// `Shared` cache is refused on any connection that runs queries per principal —
/// see [`runs_per_principal`] for why that is two fields and not one. The engine
/// filters rows for the caller, so one principal's rows would be served to
/// another. Refusing at VALIDATE time means the unsafe configuration can never
/// be published, rather than being caught (or not) at serve time.
///
/// Refused whether or not the cache is currently `enabled`. The pair is frozen
/// into the snapshot together, and a `scope = shared, enabled = false` version
/// is one flag away from leaking with no re-validation in between — while an
/// author who wants a shared cache on such a connection has no correct way to
/// get one, so there is nothing legitimate being blocked.
pub fn check_cache_scope(
    cache: &CacheSpec,
    delegated: bool,
    impersonate: bool,
) -> std::result::Result<(), ValidationError> {
    if runs_per_principal(delegated, impersonate) && cache.scope == CacheScope::Shared {
        return Err(ValidationError::SharedCacheOnPerPrincipalConnection);
    }
    Ok(())
}

/// An `Enum` parameter must carry a non-empty `allowed` list — it is the only
/// safe form for a value that may reach an identifier position.
pub fn check_enum_params(params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    for p in params {
        if p.ty == ParamType::Enum && !p.allowed.as_ref().is_some_and(|a| !a.is_empty()) {
            return Err(ValidationError::EnumWithoutAllowed(p.name.clone()));
        }
    }
    Ok(())
}

/// `location: body` cannot be published (spec §5, T7-5).
///
/// Nothing on the serving surface reads a request body — `serve_post` takes no
/// body extractor — so a body parameter is never supplied. What that means
/// depends on `required`, and the harmless-looking half is the dangerous one:
///
/// * **required** — `ParamMissing`, a permanent 400 on every call. Loud, and
///   disclosed.
/// * **optional** — the parameter resolves to absent, so `{% if approved %}` is
///   false, the block carrying the filter is OMITTED, and the endpoint answers
///   **200 with unfiltered rows, forever**. Nothing in the response says a
///   predicate went missing.
///
/// Refused rather than supported here because supporting it means parsing a
/// request body on the serving path, which is a surface with its own content
/// types, size limits and cache-key consequences — not something to add as a
/// side effect of publish-time validation.
pub fn check_param_locations(params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    for p in params {
        if p.location == ParamLocation::Body {
            return Err(ValidationError::BodyParameter(p.name.clone()));
        }
    }
    Ok(())
}

/// The compiled size a parameter's `pattern` may reach.
///
/// `regex`'s own default is 10 MB. A validation rule on an endpoint parameter
/// that needs megabytes of compiled program is not a validation rule, and the
/// cap is here rather than at serve time so an author learns at publish.
pub const MAX_PATTERN_PROGRAM_BYTES: usize = 64 * 1024;

/// How many digits a `minimum`/`maximum` bound may carry.
///
/// **Why 15, and why a cap at all.** The bound is stored and compared as text,
/// so egret enforces it exactly at any width; the generated OpenAPI document
/// cannot, because JSON Schema's `minimum`/`maximum` take a JSON NUMBER and
/// this crate does not enable serde_json's `arbitrary_precision`. A magnitude
/// past `u64` therefore lands in the document via an `f64`, which rounds — and
/// past `f64`'s range the keyword cannot be represented at all and disappears.
/// Either way the published contract stops matching the code, which is the one
/// thing a generated contract must never do (spec §4).
///
/// 15 is `f64`'s guaranteed decimal width (`DBL_DIG`): every decimal of at most
/// 15 significant digits survives the round trip through an `f64` unchanged, and
/// every integer of at most 15 digits is below 2^53 so it is exact as an integer
/// too. At 16 digits neither guarantee holds. A bound wider than that is not a
/// bound an endpoint parameter needs, so it is refused at publish — where the
/// author can still act — rather than quietly restated as a different number.
///
/// Not enforced through [`crate::serving::params::is_plain_decimal`], and
/// deliberately: that function gates a CALLER's value as well as an author's
/// bound, so widening or narrowing it would change what callers may send. This
/// cap applies to the author's bound only.
pub const MAX_BOUND_DIGITS: usize = 15;

/// How many digits a bound is written with, ignoring the sign and the leading
/// zeros `compare_decimal_text` already ignores.
///
/// `"007"` is one digit, not three: it compares as `7` at both publish and
/// serve time, and reaches the document as `7`. The integer part counts as at
/// least one digit (`"0.5"` is two), which is what makes the count agree with
/// the width the document actually has to hold.
///
/// Expects text that has already passed
/// [`crate::serving::params::is_plain_decimal`] — every remaining byte is a
/// digit or the single `.`.
fn bound_digit_width(text: &str) -> usize {
    let body = text.strip_prefix('-').unwrap_or(text);
    let (int, frac) = body.split_once('.').map_or((body, ""), |(i, f)| (i, f));
    let int = int.trim_start_matches('0');
    let int_digits = if int.is_empty() { 1 } else { int.len() };
    int_digits + frac.len()
}

/// Compile a `pattern` the way `coerce` will, so publish and serving cannot
/// disagree about which patterns exist.
///
/// **Anchored here, once.** The anchors are added in this one function, so a
/// pattern accepted at publish is byte-identical to the one matched at serve.
/// An author's own `^…$` is idempotent under it.
pub fn compile_pattern(pattern: &str) -> std::result::Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(MAX_PATTERN_PROGRAM_BYTES)
        .build()
}

/// The noun phrase completing "which is not …" for a `not_before`/`not_after`
/// bound, matched to the parameter's own declared type so the message says
/// which spelling was expected.
fn temporal_expected(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::Timestamp => "a valid timestamp",
        // `Date` and the unreachable default (see
        // `serving::params::normalize_temporal_bound`) both read as "date" —
        // `check_constraints` has already refused a temporal constraint on
        // any other type before this is ever called.
        _ => "a valid date",
    }
}

/// Refuse a constraint set that cannot be honoured, before anything tries to.
///
/// # Why this runs before `check_defaults`
///
/// `check_defaults` validates the author's `default` through `coerce`, and
/// `coerce` enforces these constraints. If the set itself is malformed — a
/// `minimum` above its `maximum` — the author would be told their `default` is
/// wrong when the default is the one part that is fine.
///
/// # Every bound is validated in its OWN grammar before it is compared
///
/// `min_length`/`max_length` are already `u32`, so there is nothing to
/// validate. The other two pairs are text, and comparing text directly is
/// unsound for both of them in the same way, for the same reason: a
/// comparison that is only correct against a NORMALISED spelling is asked of
/// the author's raw string instead.
///
/// * `minimum`/`maximum` go through
///   [`crate::serving::params::compare_decimal_text`], whose digit-by-digit
///   grammar assumes `-?digits[.digits]` — so each bound is checked against
///   [`crate::serving::params::is_plain_decimal`] first. Skipping this made
///   `minimum: "abc"` publish clean and compare as arbitrary bytes at serve
///   time forever after.
/// * `not_before`/`not_after` are meant to be the zero-padded, big-endian
///   spelling `coerce` produces, and THAT spelling sorts correctly as text —
///   but the author's raw string has not been through `coerce` yet. An
///   unpadded `"2026-9-1"` sorts AFTER `"2026-10-1"` lexically despite being
///   chronologically earlier, and nothing has yet confirmed either string is
///   even a valid date. So each bound is parsed and normalised through
///   [`crate::serving::params::normalize_temporal_bound`] first.
///
/// Either way, a bound that fails its own grammar is refused HERE, naming
/// which one, rather than compared as arbitrary bytes.
pub fn check_constraints(params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    for p in params {
        let ty = type_name(&p.ty);

        // Which constraints this type can carry at all. A `pattern` on a
        // `Boolean` is not harmless noise: it teaches the author a model where
        // constraints apply everywhere, and the next one they write silently
        // does nothing.
        let numeric = matches!(p.ty, ParamType::Number);
        let textual = matches!(p.ty, ParamType::String);
        let temporal = matches!(p.ty, ParamType::Date | ParamType::Timestamp);

        for (present, allowed_here, name) in [
            (p.minimum.is_some(), numeric, "minimum"),
            (p.maximum.is_some(), numeric, "maximum"),
            (p.min_length.is_some(), textual, "min_length"),
            (p.max_length.is_some(), textual, "max_length"),
            (p.pattern.is_some(), textual, "pattern"),
            (p.not_before.is_some(), temporal, "not_before"),
            (p.not_after.is_some(), temporal, "not_after"),
        ] {
            if present && !allowed_here {
                return Err(ValidationError::ConstraintNotForType {
                    param: p.name.clone(),
                    constraint: name,
                    ty,
                });
            }
        }

        // `minimum`/`maximum`: each bound must be a valid decimal in its own
        // right before the pair is compared as one — see the function doc.
        for (bound, constraint) in [(&p.minimum, "minimum"), (&p.maximum, "maximum")] {
            if let Some(text) = bound {
                if !crate::serving::params::is_plain_decimal(text) {
                    return Err(ValidationError::ConstraintBoundUnparseable {
                        param: p.name.clone(),
                        constraint,
                        value: text.clone(),
                        expected: "a valid decimal number",
                    });
                }
                // And it must be narrow enough for the generated contract to
                // state it exactly — see `MAX_BOUND_DIGITS`. Checked here, at
                // publish, rather than left for `openapi_gen` to round or drop
                // silently at document-build time, when the author is gone.
                let digits = bound_digit_width(text);
                if digits > MAX_BOUND_DIGITS {
                    return Err(ValidationError::ConstraintBoundTooManyDigits {
                        param: p.name.clone(),
                        constraint,
                        value: text.clone(),
                        digits,
                        max: MAX_BOUND_DIGITS,
                    });
                }
            }
        }
        // Inverted bounds. Equal bounds are legitimate — a parameter pinned to
        // one value — so this is strictly greater, never "not less".
        if let (Some(lo), Some(hi)) = (&p.minimum, &p.maximum) {
            if crate::serving::params::compare_decimal_text(lo, hi) == std::cmp::Ordering::Greater {
                return Err(ValidationError::BoundsInverted {
                    param: p.name.clone(),
                    constraint: "minimum",
                });
            }
        }
        if let (Some(lo), Some(hi)) = (p.min_length, p.max_length) {
            if lo > hi {
                return Err(ValidationError::BoundsInverted {
                    param: p.name.clone(),
                    constraint: "min_length",
                });
            }
        }
        // `not_before`/`not_after`: parse and normalise each bound ONCE, and
        // reuse the normalised value for the comparison below rather than
        // parsing it a second time — see the function doc.
        let mut not_before_n = None;
        if let Some(lo) = &p.not_before {
            not_before_n = crate::serving::params::normalize_temporal_bound(&p.ty, lo);
            if not_before_n.is_none() {
                return Err(ValidationError::ConstraintBoundUnparseable {
                    param: p.name.clone(),
                    constraint: "not_before",
                    value: lo.clone(),
                    expected: temporal_expected(&p.ty),
                });
            }
        }
        let mut not_after_n = None;
        if let Some(hi) = &p.not_after {
            not_after_n = crate::serving::params::normalize_temporal_bound(&p.ty, hi);
            if not_after_n.is_none() {
                return Err(ValidationError::ConstraintBoundUnparseable {
                    param: p.name.clone(),
                    constraint: "not_after",
                    value: hi.clone(),
                    expected: temporal_expected(&p.ty),
                });
            }
        }
        if let (Some(lo_n), Some(hi_n)) = (&not_before_n, &not_after_n) {
            if lo_n > hi_n {
                return Err(ValidationError::BoundsInverted {
                    param: p.name.clone(),
                    constraint: "not_before",
                });
            }
        }

        if let Some(pattern) = &p.pattern {
            match compile_pattern(pattern) {
                Ok(_) => {}
                Err(regex::Error::CompiledTooBig(_)) => {
                    return Err(ValidationError::PatternTooLarge {
                        param: p.name.clone(),
                    })
                }
                Err(e) => {
                    return Err(ValidationError::PatternInvalid {
                        param: p.name.clone(),
                        detail: e.to_string(),
                    })
                }
            }
        }
    }
    Ok(())
}

/// Ceiling on `output.transform.script`, in bytes.
///
/// # Sized against a real script, not a round number
///
/// Spec §1's canonical example — read `params["region"]`, index into
/// `statements["items"]`, return `{region, items, total}` — is about 200
/// bytes. 8192 (8 KiB) is ~40x that: enough room for the example plus real
/// comments, a helper `def` or two, and a second statement folded in, while
/// staying small enough that a script anywhere near the cap is visibly no
/// longer "reshape this response" — a response transform reshapes what a
/// statement already returned; it is not where an endpoint's business logic
/// belongs, and a cap an honest script never approaches is not a cap at all.
pub const MAX_TRANSFORM_SCRIPT_BYTES: usize = 8192;

/// Refuse an `output.transform` publish cannot honour.
///
/// A no-op when `output.transform` is absent — every snapshot frozen before
/// C2, and every endpoint that has no transform today.
///
/// # What this does NOT do
///
/// **It never runs the script against data.** It parses the script and
/// evaluates its module top level — through
/// [`crate::transform::eval_probe`], the same evaluator
/// [`crate::transform::run_transform`] is built on, so publish and serving
/// agree about which scripts exist — and stops there. Probing the script
/// against `returns` by actually calling `transform(params, statements)` is a
/// later task's job; this one parses and refuses, it does not run.
///
/// # The order, and why
///
/// Refusals are checked cheapest-and-most-structural first, so an author
/// editing a script that is both too long AND missing `returns` sees the
/// length problem rather than a starlark stack trace for a script that was
/// going to be refused anyway:
///
/// 1. **script length** ([`MAX_TRANSFORM_SCRIPT_BYTES`]) — the cheapest
///    possible check, and it bounds how expensive every check after it can be;
/// 2. **`shape`/`naming`** — see [`ValidationError::TransformWithShapeOrNaming`];
/// 3. **empty `returns`** — see [`ValidationError::TransformReturnsEmpty`];
/// 4. **duplicate `returns` names** — see
///    [`ValidationError::TransformReturnsDuplicateName`];
/// 5. **the script itself, parsed** — last, because it is the only check here
///    that costs more than a comparison.
///
/// # Called once per ENDPOINT, not once per statement
///
/// `output` is a property of the endpoint, not of any one statement, so
/// [`validate_endpoint`] calls this exactly once — beside
/// `egret_dataapi_api::validate_envelope`, the other endpoint-wide rule over
/// `output` — rather than from inside [`check_definition_scans`], which runs
/// once per statement. The distinction is cost, not correctness: parsing the
/// script also EVALUATES its module top level (see the section above), which
/// runs a real Starlark interpreter bounded only by
/// [`crate::transform::MAX_WALL_CLOCK`] (2s) per call. Called from the
/// per-statement loop, an endpoint at `MAX_STATEMENTS` would pay that up to
/// 16 times — worst case ~32s of evaluation held open inside `publish`'s
/// locked transaction. Called once, the whole endpoint's transform cost is
/// bounded by ONE evaluation, matching every other endpoint-wide rule.
pub fn check_transform(output: &OutputSpec) -> std::result::Result<(), ValidationError> {
    let Some(transform) = &output.transform else {
        return Ok(());
    };

    let bytes = transform.script.len();
    if bytes > MAX_TRANSFORM_SCRIPT_BYTES {
        return Err(ValidationError::TransformScriptTooLong {
            bytes,
            cap: MAX_TRANSFORM_SCRIPT_BYTES,
        });
    }

    // Compared against the field's own default rather than a literal
    // `"objects"`/`"as_is"` copied here: the two can never drift apart, because
    // `OutputSpec::default()` is built from the exact same `default_shape`/
    // `default_naming` functions serde falls back to.
    let defaults = OutputSpec::default();
    if output.shape != defaults.shape {
        return Err(ValidationError::TransformWithShapeOrNaming("shape"));
    }
    if output.naming != defaults.naming {
        return Err(ValidationError::TransformWithShapeOrNaming("naming"));
    }

    if transform.returns.is_empty() {
        return Err(ValidationError::TransformReturnsEmpty);
    }

    let mut seen = std::collections::HashSet::with_capacity(transform.returns.len());
    for col in &transform.returns {
        if !seen.insert(col.name.as_str()) {
            return Err(ValidationError::TransformReturnsDuplicateName(
                col.name.clone(),
            ));
        }
    }

    crate::transform::eval_probe(&transform.script)
        .map_err(|e| ValidationError::TransformScriptInvalid(e.to_string()))
}

/// The probe that supplies rows, in the words its refusal names it by.
const SINGLE_ROW_PROBE: &str = "the single-row probe";

/// The probe that supplies none. See [`probe_transform`] for why it exists.
const EMPTY_ROWS_PROBE: &str = "the empty-rows probe";

/// Run the transform against synthetic rows and check what it returns against
/// `output.transform.returns`.
///
/// A no-op when `output.transform` is absent, like [`check_transform`].
///
/// # What this proves, exactly
///
/// That the script RUNS, and that what it returns matches `returns`, for two
/// specific inputs: one row per statement, and no rows at all. Nothing more. A
/// script's output shape may depend on the data — that is why `returns` is
/// declared by the author rather than inferred (see `TransformSpec`) — so a
/// green probe is not a proof of conformance for every future call. Serving
/// checks every real response against the same rule
/// ([`crate::transform::check_returns`], Task 6) and fails closed; this is the
/// check that stops an author publishing something that was never going to
/// work, not a substitute for that one.
///
/// # Both probes, and why the refusal names which one
///
/// `if len(rows) == 0` is the commonest way a script's output shape diverges,
/// and it is invisible in the populated probe's output — a script can satisfy
/// `returns` with rows and return a bare list without them. So both are run.
/// The refusal carries which probe produced it because the two differ only in
/// their input: told merely "does not match `returns`", an author looking at a
/// script that demonstrably returns the right object has nowhere to go.
///
/// # The rows are SYNTHETIC, and publish makes no extra round trip
///
/// `describe_as` gives publish an endpoint's COLUMNS, never its data —
/// [`StatementSnapshot`] is `{ name, sql, columns }` — so the probe row is
/// built from those columns with a stand-in value per column type
/// ([`probe_value`]), the same way [`representative_value`] builds a stand-in
/// per parameter for M1's branch renders. Nothing here reads a row from an
/// engine, and no statement is executed.
///
/// Parameters carry [`representative_value`]s, in the SHAPE publish already
/// binds them in for its own SQL probes: a parameter the endpoint's SQL
/// iterates (`{% for x in region %}`) is a one-element LIST, every other one is
/// a scalar. That half is not inferred, it is known — [`IteratedParams`] is
/// computed once per endpoint by [`validate_endpoint`] and handed here by
/// [`validate`], and [`context_with`] wraps exactly the same set for the render
/// probes. Two probes of one endpoint disagreeing about whether `region` is a
/// list would be a defect on its own.
///
/// **The other half is genuinely uninferable, and this is its limit.** Whether
/// a parameter that NO statement iterates arrives as a scalar or a list is
/// decided by the REQUEST — a repeated query parameter becomes a list — and a
/// script's own expectation is written in no SQL, so there is nothing to read
/// it from. A script that iterates a parameter its SQL treats as a scalar is
/// therefore probed with a scalar and can still fail at serving. Task 6 owns
/// the serving-side shape; if it ever binds such a parameter as a list, this
/// must follow it.
///
/// # Two evaluations, and where they are spent
///
/// This adds exactly two `run_transform` calls per endpoint — not per statement
/// — for the reason [`check_transform`] records at length: an endpoint at
/// `MAX_STATEMENTS` inside the per-statement loop would pay each of them up to
/// 16 times, and `transform::MAX_WALL_CLOCK` is a 2 s bound on ticks after
/// expiry rather than a latency guarantee, so that is not a bounded cost.
///
/// They are spent in [`validate`], which runs BEFORE `EndpointStore::publish`
/// opens its transaction — deliberately, and this is the placement decision, not
/// an accident of where the described columns happen to be available. Publish's
/// in-transaction re-validation goes through [`validate_endpoint`], which does
/// NOT probe: the row it locked has already been through this, and the identity
/// check that guards that re-validation now covers `limits` as well as the four
/// inputs the columns depend on, precisely so the projection below cannot be
/// dodged by a PATCH landing between the two.
///
/// So a publish costs four evaluations of an endpoint's script — one module
/// evaluation from `check_transform` and these two, all outside the
/// transaction, plus one more module evaluation from the re-validation inside
/// it — and the count inside the locked transaction is unchanged from Task 4.
///
/// # `limits`, and the 1 MiB output cap
///
/// See [`ValidationError::TransformOutputExceedsRowLimit`] for what the
/// projection proves. It costs no additional evaluation: it is the difference
/// between the two probes that have already run.
pub(crate) fn probe_transform(
    output: &OutputSpec,
    described: &[StatementSnapshot],
    params: &[ParamSpec],
    iterated: &IteratedParams,
    limits: &LimitsSpec,
) -> std::result::Result<(), ValidationError> {
    let Some(transform) = &output.transform else {
        return Ok(());
    };

    let bound = probe_params(iterated, params);
    let with_rows: BTreeMap<String, Vec<serde_json::Value>> = described
        .iter()
        .map(|s| (s.name.clone(), vec![probe_row(&s.columns)]))
        .collect();
    let without_rows: BTreeMap<String, Vec<serde_json::Value>> = described
        .iter()
        .map(|s| (s.name.clone(), Vec::new()))
        .collect();

    // The populated probe first: it is the one an author is thinking about
    // while they write the script, so when both fail it is the one to report.
    let populated_bytes = one_probe(transform, &bound, &with_rows, SINGLE_ROW_PROBE)?;
    let empty_bytes = one_probe(transform, &bound, &without_rows, EMPTY_ROWS_PROBE)?;

    // What one row of input cost in output bytes. `saturating_sub`, not a
    // difference that can go negative: a script that returns MORE without rows
    // than with them ("no results" prose, say) does not grow with the row count,
    // and there is nothing to project.
    let per_row = populated_bytes.saturating_sub(empty_bytes);
    if per_row == 0 {
        return Ok(());
    }
    // Clamped exactly as `serving::router` clamps it before handing it to the
    // engine, because that is the number of rows the script can actually be
    // given. Projecting against the raw `u32` would refuse an endpoint whose
    // `max_rows: 4000000000` serving would silently cut to a million.
    let max_rows = limits
        .max_rows
        .clamp(1, crate::serving::router::MAX_ROWS_CEILING);
    let cap = crate::transform::MAX_TRANSFORM_OUTPUT_BYTES;
    let projected = empty_bytes.saturating_add(per_row.saturating_mul(max_rows as usize));
    if projected > cap {
        return Err(ValidationError::TransformOutputExceedsRowLimit {
            per_row,
            max_rows,
            projected,
            cap,
            // Integer division, so this is the largest count that FITS rather
            // than the first that does not, and it is at least 1 — see the
            // field's own doc, which is where that is argued from `CappedSink`.
            // `saturating_sub` cannot fire for the same reason (`empty <= cap`);
            // it is there so that if the sink's guarantee ever changed this
            // computes a wrong number rather than panicking in one build
            // profile and wrapping in the other. `u32::MAX` is likewise
            // unreachable — `fits < max_rows <= MAX_ROWS_CEILING` — but the
            // conversion is saturating rather than an `as` cast, because a
            // silent wrap here would print a helpful-looking number that is a
            // lie.
            fits: u32::try_from(cap.saturating_sub(empty_bytes) / per_row).unwrap_or(u32::MAX),
        });
    }
    Ok(())
}

/// One probe: run the script, check what came back against `returns`, and
/// report how many bytes it serialised to.
///
/// The bytes are what [`probe_transform`]'s projection is built from, and they
/// are measured on the SERIALISED value for the same reason
/// `MAX_TRANSFORM_OUTPUT_BYTES` is: that is the number that leaves the process.
fn one_probe(
    transform: &egret_dataapi_api::TransformSpec,
    params: &BTreeMap<String, serde_json::Value>,
    statements: &BTreeMap<String, Vec<serde_json::Value>>,
    context: &str,
) -> std::result::Result<usize, ValidationError> {
    let value = crate::transform::run_transform(transform, params, statements).map_err(|e| {
        ValidationError::TransformOutputMismatch {
            context: context.to_owned(),
            detail: e.to_string(),
        }
    })?;
    crate::transform::check_returns(&value, &transform.returns).map_err(|detail| {
        ValidationError::TransformOutputMismatch {
            context: context.to_owned(),
            detail,
        }
    })?;
    // `run_transform` has already serialised this value once, through the
    // capped sink that bounds it — so a value that reaches here is one
    // `serde_json` has written before, and the error arm is unreachable rather
    // than merely unlikely. Measuring 0 for it costs only the projection below,
    // which then refuses nothing and leaves the question to serving.
    Ok(serde_json::to_vec(&value).map_or(0, |bytes| bytes.len()))
}

/// One synthetic row, keyed by the described column names.
fn probe_row(columns: &[ColumnSpec]) -> serde_json::Value {
    serde_json::Value::Object(
        columns
            .iter()
            .map(|c| (c.name.clone(), probe_value(&c.ty)))
            .collect(),
    )
}

/// A stand-in value for one described column, by its engine type.
///
/// Routed through [`crate::openapi_gen::json_type`] rather than matching engine
/// type names here, so the value a script is probed with and the type the
/// generated document promises a consumer come from ONE reading of
/// `bigint`/`varchar(10)`/`timestamp(3)`. A second mapping would let publish
/// probe a script with a string for a column the document calls an integer.
///
/// The values are as small as each type allows, which keeps the measured
/// per-row growth of [`ValidationError::TransformOutputExceedsRowLimit`] small
/// for a row of this SHAPE. It does not make that projection a lower bound on a
/// real row, and the arm no longer claims it does: the probe row carries every
/// column of the branch union, while `serving::router::shape_rows` omits a
/// `required: false` column that the branch which actually ran did not return.
/// A real row can be narrower than this one.
fn probe_value(ty: &str) -> serde_json::Value {
    match crate::openapi_gen::json_type(ty) {
        "boolean" => serde_json::Value::Bool(true),
        "integer" => serde_json::Value::from(1),
        "number" => serde_json::Value::from(1.0),
        // Everything `json_type` does not recognise, which it maps to `string`.
        _ => serde_json::Value::from("x"),
    }
}

/// The `params` dict a probe hands the script: every declared parameter at its
/// [`representative_value`], as JSON, in the shape publish's SQL probes already
/// bind it in.
///
/// The `iterated` branch is [`context_with`]'s, one type along: that function
/// inserts a one-element LIST for a parameter any statement of the endpoint
/// iterates and a scalar for every other, and it is handed the same
/// [`IteratedParams`] this is. Asking the set rather than guessing is what stops
/// publish's two probes of ONE endpoint disagreeing about whether `region` is a
/// list — the script would then be probed against a shape the rendered SQL says
/// it is not.
fn probe_params(
    iterated: &IteratedParams,
    params: &[ParamSpec],
) -> BTreeMap<String, serde_json::Value> {
    params
        .iter()
        .map(|p| {
            let value = crate::transform::param_json(&representative_value(p));
            let value = if iterated.contains(&p.name) {
                serde_json::Value::Array(vec![value])
            } else {
                value
            };
            (p.name.clone(), value)
        })
        .collect()
}

/// An author's `default` must satisfy its own declared type (spec §5, T7-6).
///
/// Coerced through the **same function the serving path uses**, deliberately —
/// see `serving::params::coerce`. A second implementation written for publish
/// could accept something serving rejects, and the bug that costs is precisely
/// the one this check removes: `default: "yesterday"` on a `Date` parameter
/// publishes green, then every call that omits the parameter is a 400
/// `param_invalid`. That answer blames the CALLER for the author's mistake, and
/// `access_log` records it under a code that points at nothing.
pub fn check_defaults(params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    for p in params {
        let Some(default) = &p.default else { continue };
        match crate::serving::params::coerce(p, default) {
            Ok(_) => {}
            // The AUTHOR'S problem, not the type field: `coerce` already
            // enforced every constraint (Tasks 3-5), so this is the one place
            // the detail is allowed out — the serving mapping in
            // `resolve_supplied_with` throws away the same fields on purpose.
            Err(crate::serving::params::CoerceError::Constraint { constraint, bound }) => {
                return Err(ValidationError::DefaultViolatesConstraint {
                    param: p.name.clone(),
                    constraint,
                    bound,
                });
            }
            // `Invalid` is genuinely a type failure. `Malformed` reaching this
            // match would mean an unsound spec — no `allowed` list, a pattern
            // that doesn't compile, a temporal bound that doesn't parse — but
            // `check_definition_scans` runs `check_enum_params` and
            // `check_constraints` before `check_defaults`, and both already
            // refuse every `MalformedReason` this function's own `coerce` call
            // could hit. So `Malformed` is unreachable through the real
            // publish path; calling `check_defaults` directly (as a unit test
            // can) is the only way to reach it, and reporting it as a type
            // failure there is a lesser inaccuracy than inventing a message
            // for a case production can never produce.
            Err(_) => {
                return Err(ValidationError::DefaultNotValidForType(
                    p.name.clone(),
                    type_name(&p.ty),
                ));
            }
        }
    }
    Ok(())
}

/// The declared type as an author wrote it, for a message an author reads.
///
/// `pub(crate)` because [`crate::preview`]'s diagnostics name the type a test
/// value failed to satisfy, and the two surfaces must not describe the same
/// `ParamType` with two different words.
pub(crate) fn type_name(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::String => "string",
        ParamType::Number => "number",
        ParamType::Boolean => "boolean",
        ParamType::Date => "date",
        ParamType::Timestamp => "timestamp",
        ParamType::Enum => "enum value",
    }
}

/// Run every shape check over an ENDPOINT: each of its statements through the
/// same rules [`validate_definition`] applies to one, plus the two that are
/// properties of the endpoint rather than of any single statement.
///
/// **The publish-time gate runs here, which is what makes it real.** This
/// function is the one both `POST …/validate` and [`EndpointStore::publish`] go
/// through — publish re-runs it inside its own transaction against the row it is
/// about to freeze — so "an endpoint that validates is exactly an endpoint that
/// publishes" keeps holding, and neither entry point can acquire the rules
/// without the other.
///
/// # The two endpoint-wide rules, and why they cannot be in the loop
///
/// * **The statement list itself** — non-empty, every name an identifier, no two
///   names alike. It is asked once, of the list, and it is asked FIRST: a
///   duplicate name is cheaper to find than a `;` scan and it makes the rest of
///   the work pointless. See [`ValidationError::Statement`] for why publish asks
///   at all when the write routes already did.
/// * **[`ValidationError::UnreferencedParameter`]** — the one rule in the
///   parameter scan that is about the published CONTRACT rather than about the
///   statement that runs (see [`ParamRules`]). A parameter only the rows query
///   reads is not an unused query parameter, so the question is asked once
///   against the UNION of names every statement reads (spec §4.2). That is why
///   the loop below passes [`ParamRules::Draft`]: it is the same set of rules
///   minus this one, which is then applied endpoint-wide immediately after.
///
/// The statement-list rule is also the one that bounds what everything after it
/// COSTS. Each of the three loops below is per statement, so an unbounded list
/// multiplies the whole function; [`egret_dataapi_api::MAX_STATEMENTS`] is
/// checked inside that first call, before the first scan and long before the
/// first render. See its doc comment for the measurements.
///
/// For a ONE-statement endpoint this is the same sequence, in the same order, as
/// [`validate_definition`] — scans, then the cache rule, then the renders — with
/// one deliberate difference: `UnreferencedParameter` is now reported after
/// [`ValidationError::PlaceholderInsideQuotedText`] rather than before it, since
/// it has moved out of [`check_params_with`]. Only a definition breaking both at
/// once can tell, and it is refused either way.
fn validate_endpoint(
    statements: &[StatementSpec],
    params: &[ParamSpec],
    cache: &CacheSpec,
    output: &OutputSpec,
    delegated: bool,
    impersonate: bool,
) -> std::result::Result<IteratedParams, ValidationError> {
    egret_dataapi_api::validate_statements(statements)?;
    // Cheap, and about the whole endpoint rather than any one statement: which
    // envelope the response is written in. The create and patch routes check it
    // on the request body; this is the check against the STORED row, for the
    // same reason `validate_statements` runs twice — publish is where a value
    // becomes a frozen contract, and nothing in the schema constrains this
    // column.
    egret_dataapi_api::validate_envelope(output)?;
    // `output.transform`, checked ONCE here rather than once per statement in
    // the loop below. `output` is a property of the ENDPOINT, not of any one
    // statement — and unlike `check_constraints` over `params` (a pure
    // comparison, cheap to repeat), this runs a Starlark interpreter over the
    // script's module top level. Inside the loop, an endpoint at
    // `MAX_STATEMENTS` would parse-and-evaluate the same script up to 16
    // times, each bounded only by `transform::MAX_WALL_CLOCK` (2s) — worst
    // case ~32s of evaluation held open inside `publish`'s locked transaction
    // (`EndpointStore::publish`) and again on every `/validate` call. Once
    // here bounds the whole endpoint's transform cost by ONE evaluation,
    // matching `validate_envelope` right above it — both are endpoint-wide
    // properties of `output`, not per-statement rules.
    check_transform(output)?;
    // Every statement through every scan except the contract rule below. The
    // scans come before ANY render for the reason `check_definition_scans`
    // records: they are what bounds the cost of the rendering, and they cost
    // nothing themselves.
    for statement in statements {
        check_definition_scans(&statement.sql, params, ParamRules::Draft)?;
    }
    let mut read: Vec<String> = Vec::new();
    for statement in statements {
        read.extend(crate::render::context_names(&statement.sql)?);
    }
    check_every_parameter_is_read(&read, params)?;
    check_cache_scope(cache, delegated, impersonate)?;
    // ONE shape decision for the endpoint, made before the first render and
    // handed to every one of them. See [`IteratedParams`] for what deciding it
    // per statement cost. Returned rather than recomputed by [`validate`],
    // which renders the same statements again to describe them: two computations
    // of the same set are two things that can disagree.
    let sqls: Vec<&str> = statements.iter().map(|s| s.sql.as_str()).collect();
    let iterated = IteratedParams::across(&sqls, params);
    // Last, and once per statement: this is the half that renders.
    for statement in statements {
        check_render_rules(&statement.sql, params, &iterated)?;
    }
    Ok(iterated)
}

/// Run every shape check over ONE definition. The order is deliberate: the
/// statement check first, so a multi-statement string is refused before anything
/// downstream (including the engine) is asked to look at it.
///
/// **The per-DEFINITION half of the gate.** [`validate_endpoint`] is what the
/// two entry points call, and it is built from the same three pieces this is;
/// this one stays because it is the form every rule in this module is pinned
/// against, one definition at a time, and because it is what `sql`-shaped
/// callers outside publish would want. Everything before [`check_render_rules`]
/// is a scan; [`check_render_rules`] renders, which is why it is last and why
/// nothing on the serving path calls it.
pub fn validate_definition(
    sql: &str,
    params: &[ParamSpec],
    cache: &CacheSpec,
    delegated: bool,
    impersonate: bool,
) -> std::result::Result<(), ValidationError> {
    // Order preserved EXACTLY: `check_single_select` first (pinned by
    // `validate_definition_runs_the_statement_check_first`), the cache rule
    // between the scans and the renders, as it has always been.
    check_definition_scans(sql, params, ParamRules::Published)?;
    check_cache_scope(cache, delegated, impersonate)?;
    // A slice of one, because this entry point IS one definition: there are no
    // sibling statements whose `{% for %}` headers could widen the set.
    check_render_rules(sql, params, &IteratedParams::across(&[sql], params))?;
    Ok(())
}

/// Every publish rule that is a SCAN — no render, no engine, no connection.
///
/// Extracted so [`validate_preview`] and [`validate_definition`] cannot drift.
/// A rule added here is in both by construction; a rule that belongs to only one
/// of them has to be added to that one explicitly, which is a visible decision
/// rather than an omission.
///
/// `rules` carries the one such decision that exists today, and it is threaded
/// through rather than applied here so the sequence inside [`check_params`] —
/// including which error answers first — is identical for both callers. See
/// [`ParamRules`].
fn check_definition_scans(
    sql: &str,
    params: &[ParamSpec],
    rules: ParamRules,
) -> std::result::Result<(), ValidationError> {
    check_single_select(sql)?;
    // FIRST among the parameter rules, because it is the one that bounds what
    // every later rule costs — `check_render_rules` renders `optional + 2`
    // contexts and publish renders them again.
    if params.len() > MAX_DECLARED_PARAMS {
        return Err(ValidationError::TooManyParameters {
            declared: params.len(),
            limit: MAX_DECLARED_PARAMS,
        });
    }
    check_enum_params(params)?;
    check_param_locations(params)?;
    check_constraints(params)?;
    check_defaults(params)?;
    check_params_with(sql, params, rules)?;
    Ok(())
}

/// The publish rules that apply to a PREVIEW of an unsaved draft.
///
/// Everything [`validate_definition`] enforces except TWO rules, both of them
/// about something a preview does not do:
///
/// * [`check_cache_scope`] — preview neither reads nor writes the cache, so a
///   `shared` scope cannot leak across principals through it. The draft body
///   carries no `CacheSpec` for the same reason.
/// * [`ValidationError::UnreferencedParameter`] — a declared parameter no
///   placeholder reads. That rule protects the OpenAPI contract a PUBLISHED
///   endpoint offers its callers, not the statement about to run; the render
///   never looks the name up. Requiring it here refused the commonest
///   intermediate state of the authoring loop — SQL edited before the
///   declarations caught up — which is the one thing preview exists to show. See
///   [`ParamRules`].
///
/// Everything else in [`check_params`] still applies, and each of the four is
/// about the statement: `reject_unscannable_bytes` keeps the scanner sound,
/// [`ValidationError::DuplicateParameter`] would make binding ambiguous,
/// [`ValidationError::UndeclaredPlaceholder`] is a render failure or a wrong
/// bind, and [`ValidationError::PlaceholderInsideQuotedText`] is the
/// injection-adjacent one a SEV-0 came from.
///
/// # `check_single_select` is authorization here, not tidiness
///
/// Preview executes author-supplied SQL under `data_api_endpoint:manage`, which
/// is NOT `sqllab:write`. Without this check a role holding only `:manage` would
/// have arbitrary write access to every engine the workspace can reach. It is
/// the first check for that reason.
///
/// # The shape decision is made across the SIBLINGS, and returned
///
/// `sql` is the statement that will RUN; `siblings` is the SQL text of the
/// other statements of the same endpoint, and it exists for one purpose — the
/// `{% for %}` scan that decides which parameters arrive as lists. Before it,
/// preview asked that question of the one statement on screen while publish
/// asked it of every statement, so previewing the `summary` of a two-statement
/// endpoint whose `by_region` loops the same parameter went GREEN and then
/// publish refused. It failed closed, but the author was told "fine" and then
/// told "no", which is the one thing this route exists not to do.
///
/// The union is **returned**, for the same reason [`validate_endpoint`] returns
/// it: [`crate::preview::run`] needs the same shape to resolve the author's
/// test values with, and a second computation of it is a second answer that can
/// disagree. There is no way for a caller to obtain a different one.
///
/// [`SiblingStatements`] is what keeps "informs the shape and nothing else"
/// structural rather than documented: its text is private to this module, so no
/// caller can render, bind or execute it. See its own comment.
///
/// # `output` is checked for its TRANSFORM and applied to nothing (C2)
///
/// [`check_transform`] runs here, and it is the same call [`validate_endpoint`]
/// makes — the only production call site there was. Everything it refuses (a
/// script that does not parse, an empty or duplicated `returns`, a transform
/// beside a non-default `shape` or `naming`) is a publish refusal that no write
/// route checks, so a draft could be saved, previewed GREEN, and refused only
/// at publish.
///
/// What does NOT happen here is running the script: `crate::preview` has one
/// statement's rows and no statement names, and `transform::run_transform`
/// takes every statement's rows keyed by name. The full argument, including the
/// two ways a partial `statements` dict answers wrongly, is in the
/// `crate::preview` module comment.
///
/// `pub(crate)` rather than `pub`, because it now returns [`IteratedParams`] —
/// and that type is deliberately crate-private, so that "the shape a definition
/// is validated against" cannot be constructed, or a second one computed,
/// outside this module. Nothing outside the crate called this.
pub(crate) fn validate_preview(
    sql: &str,
    siblings: &SiblingStatements,
    params: &[ParamSpec],
    output: &OutputSpec,
) -> std::result::Result<IteratedParams, ValidationError> {
    // FIRST, before the scan of `sql` and long before any render, and against
    // the same number publish uses: `siblings` is a list from a request body,
    // and every statement in it costs a template scan. An endpoint may have
    // `MAX_STATEMENTS` statements in total, so one of them plus its siblings is
    // bounded by exactly that — no new number, and a preview body that could not
    // describe a publishable endpoint is refused with the error publish would
    // give the same list. See the note in `preview::PreviewEndpointInput` on why
    // the LENGTH of each text needs no bound of its own here.
    let count = 1 + siblings.len();
    if count > egret_dataapi_api::MAX_STATEMENTS {
        return Err(ValidationError::Statement(
            egret_dataapi_api::StatementError::TooManyStatements {
                count,
                limit: egret_dataapi_api::MAX_STATEMENTS,
            },
        ));
    }
    // `output.transform`, in the position [`validate_endpoint`] checks it from
    // — after the statement-list rule and before any per-statement scan — so an
    // author who breaks both is told about the same one first on both paths.
    //
    // This is an endpoint-wide rule about `output`, exactly as it is for
    // publish, so it is called here rather than added to
    // `check_definition_scans`: that runs once per statement, and this parses
    // AND evaluates the script's module top level under a real Starlark
    // interpreter. See `check_transform`'s own comment for the cost argument.
    //
    // The other endpoint-wide `output` rule, `validate_envelope`, is
    // deliberately NOT here, and the asymmetry is a fact about where each is
    // enforced rather than a preference: `envelope` is checked by the create
    // and PATCH routes on the way in, so no draft an author can be looking at
    // carries a bad one. `check_transform` has exactly one production call
    // site (`validate_endpoint`), so before this a script that does not parse
    // could be SAVED, previewed green, and refused only at publish.
    check_transform(output)?;
    // Only `sql` is SCANNED. The siblings are not this preview's statement:
    // they are already stored, they were scanned when they were written, and
    // re-refusing one here would block previewing the statement the author is
    // actually looking at because a different one is mid-edit.
    check_definition_scans(sql, params, ParamRules::Draft)?;
    let iterated = siblings.union_with(sql, params);
    check_render_rules(sql, params, &iterated)?;
    Ok(iterated)
}

/// The SQL text of the other statements of the endpoint a preview belongs to.
///
/// # Why a type and not a `Vec<String>`
///
/// These statements inform ONE decision — which parameters a `{% for %}` header
/// iterates, and therefore which arrive as lists — and they must never be
/// rendered, bound or executed. A preview runs the statement on screen; a body
/// that could smuggle a second statement into the engine would turn a
/// `data_api_endpoint:manage` preview into an arbitrary multi-statement run.
///
/// The text is a private field of a type declared in THIS module, and the only
/// thing reachable from anywhere else is [`Self::union_with`], which returns a
/// set of parameter names. `crate::preview` deserialises the field, hands it to
/// [`validate_preview`], and cannot read a single byte of it. So "informs the
/// shape and nothing else" is a property of the type rather than a rule someone
/// has to remember — the same move [`IteratedParams`] makes for the shape
/// itself.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(transparent)]
pub struct SiblingStatements(Vec<String>);

impl SiblingStatements {
    /// How many there are. The count is public because the BOUND is a rule
    /// about the request; the text is not.
    fn len(&self) -> usize {
        self.0.len()
    }

    /// The iterated-parameter union across `sql` and every sibling — the same
    /// set [`validate_endpoint`] computes for the whole endpoint, which is the
    /// point.
    ///
    /// `sql` is passed IN rather than the siblings being handed out, so this is
    /// the only expression that ever sees the texts together and there is no
    /// accessor to misuse.
    fn union_with(&self, sql: &str, params: &[ParamSpec]) -> IteratedParams {
        let mut sqls: Vec<&str> = Vec::with_capacity(1 + self.0.len());
        sqls.push(sql);
        sqls.extend(self.0.iter().map(String::as_str));
        IteratedParams::across(&sqls, params)
    }
}

/// A type-correct stand-in value for one parameter, for the renders publish
/// performs.
///
/// No escaping is involved and none is needed — these never reach the SQL. The
/// renderer turns every emitted value into a bind marker, so what a
/// representative value decides is which BRANCHES a definition takes and what
/// type each bound value carries, not what any statement says. Every one of them
/// is deliberately TRUTHY, so `{% if region %}` takes its block: the point of the
/// full-values render is to exercise the whole definition, and the absent case
/// gets its own renders.
fn representative_value(p: &ParamSpec) -> Param {
    match p.ty {
        ParamType::Number => Param::I64(1),
        ParamType::Boolean => Param::Bool(true),
        ParamType::Date => Param::Date("1970-01-01".into()),
        ParamType::Timestamp => Param::Timestamp("1970-01-01 00:00:00".into()),
        ParamType::Enum => Param::Str(
            p.allowed
                .as_ref()
                .and_then(|a| a.first().cloned())
                // Unreachable after `check_enum_params`; a non-empty string
                // keeps this truthy rather than silently changing the branch a
                // probe takes.
                .unwrap_or_else(|| "x".into()),
        ),
        ParamType::String => Param::Str("x".into()),
    }
}

/// The context publish renders against: every declared parameter present with a
/// representative value, except those named in `absent`.
///
/// # Shape, not just value
///
/// A parameter named as a `{% for %}` iterable **by any statement of the
/// endpoint** is inserted as a one-element LIST — see [`IteratedParams`], which
/// is the only way to obtain that set and cannot be asked about one statement of
/// several. `ParamSpec` has no multiplicity field — whether a parameter arrives
/// as a scalar or a list is decided by the request (a repeated query parameter
/// becomes a list) — so publish has to infer the shape the definition expects,
/// and it infers it from the whole endpoint, because that is what the serving
/// path binds. A wrong guess makes the render FAIL, which refuses the publish;
/// it cannot make a bad definition publish.
///
/// `absent` uses [`ParamValues::insert_absent`], not omission: under Strict
/// undefined a name that is merely missing is an ERROR even when tested for
/// truth, and "declared but not supplied" has to be `none` for `{% if %}` to be
/// false. That is the same distinction the serving path makes, and it is what
/// makes these renders representative of what will actually be served.
fn representative(iterated: &IteratedParams, params: &[ParamSpec], absent: &[&str]) -> ParamValues {
    context_with(iterated, params, absent, representative_value)
}

/// The value a parameter carries when the CALLER supplies nothing: its declared
/// `default`, or the representative value when it has none.
///
/// Coerced by [`crate::serving::params::coerce`] — the same function the
/// serving path uses — so the value publish renders against and the value a
/// bare call binds are produced by one rule rather than two. The fallback is
/// unreachable rather than lenient: [`check_defaults`] refuses a `default` this
/// cannot coerce, and it runs inside [`check_definition_scans`], before any
/// render.
fn resting_value(p: &ParamSpec) -> Param {
    match &p.default {
        Some(d) => crate::serving::params::coerce(p, d).unwrap_or_else(|_| representative_value(p)),
        None => representative_value(p),
    }
}

/// [`representative`], with every defaulted parameter at its own `default`.
///
/// This is the context a call with NO query string produces, which is the one
/// the generated document advertises — see [`branch_renderings`] for why it has
/// to be described in its own right.
fn resting(iterated: &IteratedParams, params: &[ParamSpec], absent: &[&str]) -> ParamValues {
    context_with(iterated, params, absent, resting_value)
}

/// Which declared parameters arrive as a LIST, for ONE endpoint.
///
/// # Why a type, and why it is built from a slice of statements
///
/// Every statement of an endpoint renders against a SINGLE `ParamValues` (Task
/// 7) built from a SINGLE resolve (Task 6), and `serving::params::resolve`
/// decides list-versus-scalar from the union across every statement. Publish
/// used to decide it per statement, from the one SQL it happened to be
/// validating: [`crate::render::iterated_params`] on `summary` alone reports
/// nothing about the `by_region` statement beside it that loops over the same
/// parameter, so publish validated `summary` with a scalar, the endpoint
/// published green, and the first call — with no repeated query parameter, so
/// `serving::router::render_error` correctly reads it as an author fault —
/// answered 500. Including the call with no query string at all, which is the
/// one the generated document advertises.
///
/// So the set is a value with a name, obtained only from
/// [`IteratedParams::across`], which takes every statement's text the way
/// `resolve_supplied` does. The per-statement question is still askable — an
/// endpoint with one statement is a slice of one — but "the shape of THIS
/// statement's parameters, ignoring its siblings" is not a thing this module can
/// express any more.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct IteratedParams(Vec<String>);

impl IteratedParams {
    /// The union across `sqls`, computed by the same function
    /// [`crate::serving::params::resolve_supplied`] uses — one function, so the
    /// shape a definition is validated against and the shape it is served
    /// cannot disagree by construction rather than by two implementations
    /// agreeing.
    fn across(sqls: &[&str], params: &[ParamSpec]) -> Self {
        let declared: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        Self(crate::render::iterated_params_union(sqls, &declared))
    }

    fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|n| n == name)
    }

    /// The names, for the one caller that has to resolve VALUES against this
    /// set rather than ask about a single name: [`crate::preview::run`], which
    /// passes them to `serving::params::resolve_supplied_with`.
    ///
    /// Read-only and derived — there is still exactly one way to CONSTRUCT the
    /// set ([`Self::across`], via [`validate_endpoint`] or
    /// [`validate_preview`]), so handing the names out cannot produce a second
    /// answer, only a use of the first one.
    pub(crate) fn names(&self) -> &[String] {
        &self.0
    }
}

/// The shared body of [`representative`] and [`resting`]: the two differ only in
/// which value a present parameter carries, and everything else about building
/// the context — `insert_absent` rather than omission, and applying the
/// endpoint's [`IteratedParams`] — must not be able to drift between them.
fn context_with(
    iterated: &IteratedParams,
    params: &[ParamSpec],
    absent: &[&str],
    value: fn(&ParamSpec) -> Param,
) -> ParamValues {
    let mut ctx = ParamValues::new();
    for p in params {
        if absent.contains(&p.name.as_str()) {
            ctx.insert_absent(p.name.clone());
        } else if iterated.contains(&p.name) {
            ctx.insert_list(p.name.clone(), vec![value(p)]);
        } else {
            ctx.insert_for_spec(p, value(p));
        }
    }
    ctx
}

/// The publish-time rules of spec §5 that need the definition RENDERED.
///
/// # This is where Task 5's gate acquires a caller
///
/// [`crate::render::check_publish`] — the behavioural gate that renders a
/// definition with opaque values and again with plain ones and refuses any
/// divergence — had no production caller at all until this function. Every
/// safety property it establishes existed only in its own tests. It is called
/// here and **only** here: it renders up to sixty-four times per invocation,
/// which is right at publish (once per version) and would be absurd per request.
/// The serving path keeps [`crate::render::check_static`] and the
/// placeholder==bind invariant, which is what [`crate::render::render`] already
/// runs.
///
/// # Which rule each render carries
///
/// * **The template parses under the hardened environment** — the first thing
///   any render does, reported as [`crate::render::RenderError::Template`].
/// * **No `{% filter %}`, no `{% set x %}…{% endset %}`** — `check_static`, as
///   [`crate::render::RenderError::CaptureConstruct`].
/// * **Placeholder count equals bind count** — the invariant inside every
///   render, as [`crate::render::RenderError::PlaceholderBindMismatch`].
/// * **`| ident` only on an `Enum` carrying `allowed`** — the filter itself
///   refuses anything else, and the representative values are what put an
///   `allowed` list in reach of it. A non-enum parameter is a `Cell` with no
///   `allowed`; an absent one is `none`. Both are
///   [`crate::render::RenderError::Ident`].
/// * **No parameter in `LIMIT`/`OFFSET`**, and **a parameter in the projection
///   is wrapped in a `CAST`** — scans of the RENDERED statement, where a
///   parameter is a `?` standing in SQL structure rather than a `{{ … }}` that
///   may or may not survive a branch.
///
/// # And the absent case, which is not a variation but a second contract
///
/// An optional parameter with no `default` is `none` at serve time on every call
/// that omits it, which is most of them. `{{ sort | ident }}` outside an
/// `{% if %}` renders fine with a value and is a 500 without one — an endpoint
/// that publishes green and then fails every default call. So each optional
/// parameter is rendered absent on its own, and the gate is run once more with
/// all of them absent: those two extremes are what a caller can actually reach
/// with no query string at all.
fn check_render_rules(
    sql: &str,
    params: &[ParamSpec],
    iterated: &IteratedParams,
) -> std::result::Result<(), ValidationError> {
    check_ident_values(sql, params, iterated)?;
    let full = representative(iterated, params, &[]);
    crate::render::check_publish(sql, &full)?;
    check_rendered(sql, &full)?;

    let optional: Vec<&str> = params
        .iter()
        .filter(|p| !p.required && p.default.is_none())
        .map(|p| p.name.as_str())
        .collect();
    for name in &optional {
        let ctx = representative(iterated, params, &[name]);
        // The render failing HERE and not above is exactly the T7-7 shape, and
        // the parameter that was taken away is the one to name. The renderer's
        // own error would say `ident` may only be applied to an enum parameter,
        // which is true and points at the wrong fix.
        if check_rendered(sql, &ctx).is_err() {
            return Err(ValidationError::OptionalParameterUnguarded(
                (*name).to_string(),
            ));
        }
    }
    if optional.len() > 1 {
        let none = representative(iterated, params, &optional);
        crate::render::check_publish(sql, &none)?;
        check_rendered(sql, &none)?;
    }
    Ok(())
}

/// Every `allowed` value of an enum that reaches the statement as TEXT must be
/// identifier-shaped.
///
/// # The hole this closes, and why the rendered-statement scan is not enough
///
/// `| ident` writes an `allowed` entry into the statement verbatim — that is what
/// it is for, since `ORDER BY ?` is a syntax error on both engines — and the
/// entry is the author's own string, which is the whole provenance argument. What
/// nobody had checked is the OTHER premise: that the author's string is safe
/// *because* it is the author's. It was not. `allowed = ["id", "id; DROP TABLE t"]`
/// published green: the definition-side `;` test reads the RAW definition, which
/// has no `;` in it, and publish renders only the FIRST entry, so the second
/// reached no scan and no `describe`.
///
/// [`crate::render::render`] now scans the rendered statement, which stops the
/// separator at serve time and at publish for whichever entries publish happens
/// to render — but "happens to render" is a bounded set and `allowed` is not.
/// This rule is the unbounded half: EVERY entry, checked by shape.
///
/// # Why it is scoped rather than applied to every enum
///
/// The brief said to require the shape of all `allowed` values everywhere.
/// Measured against the model, that refuses a legitimate and ordinary endpoint:
/// an enum is also how an author constrains a bound VALUE (`WHERE status =
/// {{ status }}` with `allowed = ["Pending Review"]`), and a value never reaches
/// the statement text at all. So the shape is required exactly of the enums whose
/// value DOES reach it, and reaching is decided by rendering rather than by
/// scanning for `| ident`:
///
/// * render with the first allowed entry and with the second; if the statements
///   differ, the value reached the text — nothing else can move it, because a
///   comparison against a parameter is refused;
/// * with one entry, or if the entry appears verbatim in the rendered statement,
///   assume it reached (over-strict, and safe in that direction);
/// * if a render fails at all, assume it reached — fail closed.
fn check_ident_values(
    sql: &str,
    params: &[ParamSpec],
    iterated: &IteratedParams,
) -> std::result::Result<(), ValidationError> {
    for p in params.iter().filter(|p| p.ty == ParamType::Enum) {
        let Some(allowed) = p.allowed.as_ref().filter(|a| !a.is_empty()) else {
            continue;
        };
        if !enum_reaches_statement_text(sql, params, iterated, p, allowed) {
            continue;
        }
        for value in allowed {
            if !is_identifier_shaped(value) {
                return Err(ValidationError::EnumValueNotIdentifier {
                    param: p.name.clone(),
                    value: value.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Does this enum's value reach the rendered statement as text? See
/// [`check_ident_values`]. Fails closed.
fn enum_reaches_statement_text(
    sql: &str,
    params: &[ParamSpec],
    iterated: &IteratedParams,
    p: &ParamSpec,
    allowed: &[String],
) -> bool {
    let render_with = |value: &str| {
        let mut ctx = representative(iterated, params, &[]);
        ctx.insert_enum(p.name.clone(), value.to_string(), allowed.to_vec());
        crate::render::render(sql, &ctx).map(|(sql, _)| sql)
    };
    let Ok(first) = render_with(&allowed[0]) else {
        return true;
    };
    if let Some(second) = allowed.get(1) {
        match render_with(second) {
            Ok(other) if other != first => return true,
            Err(_) => return true,
            _ => {}
        }
    }
    first.contains(allowed[0].as_str())
}

/// `[A-Za-z_][A-Za-z0-9_]*`, optionally one dot between two such parts.
///
/// A qualified name (`t.col`) is the most an identifier position needs; a
/// quoted identifier is not accepted, because accepting one would mean deciding
/// which dialect's quoting rules apply — the agreement this crate refuses to
/// assume everywhere else.
fn is_identifier_shaped(value: &str) -> bool {
    let mut parts = value.split('.');
    let ok = |part: &str| {
        let mut chars = part.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    parts.next().is_some_and(ok) && parts.clone().count() <= 1 && parts.all(ok)
}

/// Render once and apply the two rules that read the rendered statement.
fn check_rendered(sql: &str, ctx: &ParamValues) -> std::result::Result<(), ValidationError> {
    let (rendered, _binds) = crate::render::render(sql, ctx)?;
    check_no_parameter_in_limit(&rendered)?;
    check_projection_parameters_are_cast(&rendered)?;
    Ok(())
}

/// The rendered statement with comments and quoted regions elided, under both
/// escaping readings — so a `?` inside a literal is text and not a bind marker,
/// and neither dialect's reading of `\` is assumed.
///
/// Returns both readings because the two rules below must hold under each: a
/// scan that picked one reading would be evadable by the other, which is the
/// mistake this crate has already made once.
fn scannable_readings(rendered: &str) -> Vec<String> {
    ESCAPING_READINGS
        .iter()
        .filter_map(|e| strip_comments_and_strings(rendered, *e).ok())
        .collect()
}

/// No parameter in `LIMIT`/`OFFSET` (spec §3.2, §5).
///
/// Neither engine will bind there — StarRocks refuses `LIMIT ?` at prepare time
/// — so this is an endpoint that fails on every call, published green.
///
/// After the keyword, an operand made of digits and commas is fine (`LIMIT 10`,
/// `LIMIT 10, 20`); a `?` in it is not; and the first word-shaped token ends the
/// operand, so `LIMIT 10 OFFSET x` does not read `x` as part of `LIMIT`'s.
fn check_no_parameter_in_limit(rendered: &str) -> std::result::Result<(), ValidationError> {
    for text in scannable_readings(rendered) {
        let b = text.as_bytes();
        let lower = text.to_ascii_lowercase();
        for keyword in ["limit", "offset"] {
            let mut from = 0;
            while let Some(at) = lower[from..].find(keyword) {
                let start = from + at;
                let end = start + keyword.len();
                from = end;
                let before_ok = start == 0 || !is_word_byte(b[start - 1]);
                let after_ok = end >= b.len() || !is_word_byte(b[end]);
                if !(before_ok && after_ok) {
                    continue;
                }
                if operand_has_marker(&text[end..]) {
                    return Err(ValidationError::ParameterInLimit);
                }
            }
        }
    }
    Ok(())
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Does the operand immediately following a `LIMIT`/`OFFSET` keyword contain a
/// bind marker? Digits, commas and whitespace continue the operand; anything
/// else ends it.
fn operand_has_marker(rest: &str) -> bool {
    for c in rest.chars() {
        match c {
            '?' => return true,
            c if c.is_ascii_whitespace() || c.is_ascii_digit() || c == ',' => continue,
            _ => return false,
        }
    }
    false
}

/// A parameter in a select list must be wrapped in a `CAST` (spec §3.4, §5).
///
/// **Neither engine infers a bind marker's type in a projection**: Trino types a
/// bare `?` as `unknown`, StarRocks as `VAR_STRING`. Publish freezes the
/// describe-resolved columns into the snapshot, so an uncast parameter does not
/// fail — it succeeds, with the wrong type frozen into the published contract
/// forever. That is why it is a publish error rather than something left to the
/// engine.
///
/// Every `SELECT` is scanned, not only the outermost, so a CTE's projection is
/// covered too. The region ends at the first clause keyword at the same
/// parenthesis depth, so a `?` in a `WHERE` is never mistaken for a projection.
fn check_projection_parameters_are_cast(
    rendered: &str,
) -> std::result::Result<(), ValidationError> {
    for text in scannable_readings(rendered) {
        let lower = text.to_ascii_lowercase();
        let b = lower.as_bytes();
        let mut from = 0;
        while let Some(at) = lower[from..].find("select") {
            let start = from + at;
            let end = start + "select".len();
            from = end;
            let before_ok = start == 0 || !is_word_byte(b[start - 1]);
            let after_ok = end >= b.len() || !is_word_byte(b[end]);
            if before_ok && after_ok && projection_has_uncast_marker(&lower[end..]) {
                return Err(ValidationError::UncastParameterInProjection);
            }
        }
    }
    Ok(())
}

/// Clause keywords that end a select list at the depth the `SELECT` opened at.
const CLAUSE_KEYWORDS: [&str; 9] = [
    "from", "where", "group", "having", "order", "limit", "offset", "union", "into",
];

/// Walk one select list, tracking which enclosing parentheses were opened by a
/// `CAST`/`TRY_CAST`. A `?` with no such parenthesis around it is the finding.
///
/// `rest` is already lowercased.
fn projection_has_uncast_marker(rest: &str) -> bool {
    // One entry per open parenthesis: was it opened by a cast?
    let mut casts: Vec<bool> = Vec::new();
    let b = rest.as_bytes();
    let mut i = 0;
    // The end of the last word token seen, so `(` can ask what preceded it.
    let mut last_word: &str = "";
    while i < b.len() {
        let c = b[i];
        if is_word_byte(c) {
            let start = i;
            while i < b.len() && is_word_byte(b[i]) {
                i += 1;
            }
            let word = &rest[start..i];
            // A clause keyword at the SELECT's own depth ends the projection.
            if casts.is_empty() && CLAUSE_KEYWORDS.contains(&word) {
                return false;
            }
            last_word = word;
            continue;
        }
        match c {
            b'(' => {
                casts.push(matches!(last_word, "cast" | "try_cast"));
                last_word = "";
            }
            b')' => {
                if casts.pop().is_none() {
                    // The select list ended with the enclosing expression, e.g.
                    // a scalar subquery. Nothing further here is a projection.
                    return false;
                }
                last_word = "";
            }
            b'?' => {
                if !casts.iter().any(|c| *c) {
                    return true;
                }
                last_word = "";
            }
            _ if c.is_ascii_whitespace() => {}
            _ => last_word = "",
        }
        i += 1;
    }
    false
}

/// Validate a draft and resolve EVERY statement's output columns against the
/// engine.
///
/// Shared by `POST /validate` and `POST /publish` so the two can never diverge:
/// an endpoint that validates is exactly an endpoint that publishes.
///
/// # One statement, one frozen column set
///
/// Each statement is described in its own right and gets its own union, because
/// each one is its own result set on the wire (spec §4.4) and its own contract.
/// The list comes back in the author's order, carrying the author's names, so
/// the snapshot the caller freezes is positionally and nominally the draft it
/// validated.
///
/// # The describe budget is the ENDPOINT's, and it is spent before the first round trip
///
/// Spec §4.1.1. Every statement's renderings are enumerated FIRST, their counts
/// summed, and the total checked against [`MAX_DESCRIBED_BRANCHES`] before any
/// statement is described. Checking per statement would let N statements cost N
/// times one statement's budget; checking the sum only as it accumulated would
/// still hand the engine every branch of every statement up to the one that
/// tipped it over, which is the cost the limit exists to refuse. Enumerating is
/// pure CPU and is separately bounded by [`MAX_DECLARED_PARAMS`] per statement
/// and by [`egret_dataapi_api::MAX_STATEMENTS`] across them, both already
/// enforced above.
///
/// `describe_as` runs as `principal`, so a delegated connection resolves under
/// THEIR policy. Its error variants are passed through unchanged — a Ranger
/// denial stays a 403 and a foreign `database_id` stays a 404, because telling an
/// author "your SQL is invalid" when their SQL is fine and their grant is missing
/// sends them to fix the wrong thing.
///
/// # The engine is shown the statement it will actually be sent
///
/// `WHERE region = {{region}}` is not SQL, so something has to be rendered before
/// an engine will look at it. This used to substitute a typed literal for each
/// `{{name}}` — which was adequate while the definition was a substitution list
/// and stopped being adequate the moment it became a template: that substitution
/// left every `{% if %}` and `{% for %}` tag standing in the string, so **no
/// conditional endpoint could describe at all**, and spec §4.3's own example
/// could not publish.
///
/// So publish renders the real thing, through the real renderer, with
/// representative values — the same call the serving path makes — and describes
/// the `?`-carrying statement that comes out. That statement cannot go through
/// `describe_as`: on StarRocks the text path answers a statement containing `?`
/// with **zero columns and no error**, which would freeze an empty schema. It
/// goes through `describe_as_with_params`, which is what Task 4 built.
///
/// An empty column list is refused rather than frozen. An endpoint with no
/// columns has no contract, and the failure it produces — 200s carrying empty
/// objects — is the kind nobody reports as a bug.
///
/// # One rendering is not one contract
///
/// A conditional definition has one output schema per branch, and freezing the
/// one branch the representative values happened to take made every OTHER call
/// a `503 schema_drift` — including the call with no query string, which is the
/// one the generated OpenAPI advertises as the default. So every branch is
/// described and their UNION is frozen, each column carrying whether every
/// branch returns it. See [`branch_renderings`] for which renderings that is and
/// [`describe_branches`] for how they are folded together.
///
/// # And the transform is probe-RUN, after the columns are known
///
/// [`validate_endpoint`] has already refused a transform publish cannot parse.
/// What it could not do is run one: a probe needs rows, rows need columns, and
/// the columns are what the loop below resolves. So [`probe_transform`] is
/// called at the end of this function — still exactly once per endpoint, and
/// still before `EndpointStore::publish` opens its transaction.
pub async fn validate(
    port: &dyn egret_query_api::QueryExecPort,
    workspace: Uuid,
    principal: UserId,
    draft: &DataApiEndpointDto,
    delegated: bool,
    impersonate: bool,
) -> Result<Vec<StatementSnapshot>> {
    // The shape decision comes back from the validator that just used it, so the
    // renders that DESCRIBE each statement below and the renders that VALIDATED
    // it are against the same `ParamValues` shape.
    let iterated = validate_endpoint(
        &draft.statements,
        &draft.params,
        &draft.cache,
        &draft.output,
        delegated,
        impersonate,
    )?;

    // Enumerate every statement's branches before describing any of them, and
    // spend the endpoint's whole budget in one decision. See the doc comment.
    let mut planned: Vec<(&StatementSpec, Vec<(String, usize)>)> =
        Vec::with_capacity(draft.statements.len());
    let mut total = 0usize;
    for statement in &draft.statements {
        let renderings = renderings_for(&statement.sql, &draft.params, &iterated)?;
        total += renderings.len();
        planned.push((statement, renderings));
    }
    if total > MAX_DESCRIBED_BRANCHES {
        return Err(ValidationError::TooManyBranches {
            branches: total,
            limit: MAX_DESCRIBED_BRANCHES,
        }
        .into());
    }

    let mut frozen: Vec<StatementSnapshot> = Vec::with_capacity(planned.len());
    for (statement, renderings) in planned {
        let described =
            describe_branches(port, workspace, principal, draft.database_id, renderings).await?;
        // `arrays` is positional: `shape_rows` zips the live and frozen column
        // lists by index and `data_schema` documents the order in prose, because
        // OpenAPI 3.0 has no tuple type. A definition whose branches resolve to
        // different column lists — different names, different count, or different
        // order — has no single order to freeze, so it is refused here rather than
        // served under whichever branch happened to be described first. A
        // non-branching definition has exactly one entry in `per_branch`, so
        // this `any` never fires for it — the rule costs it nothing.
        //
        // Per STATEMENT, like every other rule in this loop: `output.shape` is
        // one setting for the endpoint, but what it cannot express is one
        // statement's branches disagreeing, and each statement is served as its
        // own positional result set.
        if draft.output.shape == "arrays"
            && described
                .per_branch
                .iter()
                .any(|names| names != &described.per_branch[0])
        {
            return Err(ValidationError::BranchingArraysShape.into());
        }
        // The SAME fold `shape_rows` and `data_schema` run, over the SAME frozen
        // union, run here first — where the author is present and the blast radius
        // is one endpoint. `draft.output` is already in hand for the check above.
        //
        // `describe_branches` refuses duplicates by RAW name; `output.naming` can
        // collapse two distinct raw names onto one key, and until the union existed
        // that needed both spellings inside ONE projection, which the raw-name check
        // already caught. See `ValidationError::OutputNamingCollision`.
        //
        // Folded per statement and NOT across statements: two statements may
        // each return a `total`, because §4.4 keys their result sets apart by
        // statement name and a key collision is only possible inside one JSON
        // object.
        let mut mapped: Vec<(String, &str)> = Vec::with_capacity(described.union.len());
        for column in &described.union {
            let key = crate::serving::router::apply_naming(&column.name, &draft.output.naming);
            if let Some((_, first)) = mapped.iter().find(|(k, _)| *k == key) {
                return Err(ValidationError::OutputNamingCollision {
                    key,
                    first: (*first).to_owned(),
                    second: column.name.clone(),
                    naming: draft.output.naming.clone(),
                }
                .into());
            }
            mapped.push((key, column.name.as_str()));
        }
        frozen.push(StatementSnapshot {
            name: statement.name.clone(),
            sql: statement.sql.clone(),
            columns: described.union,
        });
    }
    // Last, once for the endpoint, and only now: the probe builds its rows from
    // the columns the loop above resolved, so it cannot run before them. Two
    // script evaluations against synthetic rows — see `probe_transform` for why
    // both, for what a green probe does and does not prove, and for why this is
    // the right side of `EndpointStore::publish`'s transaction to spend them on.
    probe_transform(
        &draft.output,
        &frozen,
        &draft.params,
        &iterated,
        &draft.limits,
    )?;
    Ok(frozen)
}

/// What describing every branch of one definition produced.
pub(crate) struct DescribedBranches {
    /// Every column any branch returns, in first-seen order, with `required`
    /// set for the ones every branch returns.
    pub union: Vec<ColumnSpec>,
    /// The column NAMES each described branch resolved to, in describe
    /// order. The union throws away WHICH branch had which columns, and that is
    /// the question `output.shape = "arrays"` has to ask: an array row is
    /// positional, so two branches with different column lists cannot both be
    /// served under one frozen order. See [`ValidationError::BranchingArraysShape`].
    pub per_branch: Vec<Vec<String>>,
}

/// How many parameters one definition may declare.
///
/// # This is the bound on the half [`MAX_DESCRIBED_BRANCHES`] does not cover
///
/// The branch limit is measured from the rendered set, so the rendering is
/// already paid for by the time it can refuse anything, and that rendering is
/// synchronous CPU inside an `async fn`. This limit is a scan over
/// `params.len()` in [`check_definition_scans`], before any render, and it is
/// what makes the multi-second case unrepresentable rather than merely refused.
///
/// It bounds ONE statement's render. [`egret_dataapi_api::MAX_STATEMENTS`] is
/// the third of the set and bounds how many of them an endpoint may have, since
/// the residual below is paid once per statement.
///
/// # Where the number comes from
///
/// # The residual, and why it is accepted rather than moved off the runtime
///
/// What is left after this bound is up to ~122 ms of SYNCHRONOUS rendering on an
/// async task, and it is deliberately not wrapped in `spawn_blocking`.
///
/// The reason is that the cost is **bounded and authenticated** — publish and
/// preview both sit behind `data_api_endpoint:manage`, and this constant is what
/// makes the upper bound a number rather than a function of the request. It is
/// ~122 ms per STATEMENT, and it took
/// [`egret_dataapi_api::MAX_STATEMENTS`] to make the endpoint's total a number
/// too: measured at 121 ms × N, it reached 31.6 s at 256 statements. It is
/// **not** that an engine `describe` of similar duration happens nearby: an
/// awaited describe RELEASES the worker, so the runtime schedules other tasks
/// for those milliseconds and only the one caller waits. This rendering OCCUPIES
/// the worker — tokio cannot preempt it, every other task queued on that worker
/// eats the whole spike, and an author calling preview in a loop holds one
/// worker per in-flight request. Same magnitude, opposite effect on everything
/// else running. If the bound ever rises, that reasoning has to be redone rather
/// than carried over.
///
/// # Where the number comes from
///
/// Measured in release, timing everything publish does before its first
/// describe (`branch_renderings` plus [`check_render_rules`]), for *n*
/// parameters each guarding its own `{% if %}` block:
///
/// | *n* | all `string` (linear shape) | all `boolean` (the expensive shape) |
/// |---|---|---|
/// | 16 | 18 branches, 2.2 ms | 138 branches, 21 ms |
/// | 24 | 26 branches, 3.6 ms | 302 branches, 55 ms |
/// | **32** | **34 branches, 5.9 ms** | **530 branches, 122 ms** |
/// | 36 | — | 640 branches, 152 ms |
/// | 48 | 50 branches, 12.5 ms | — |
/// | 62 | 64 branches, 20.0 ms | — |
/// | 100 | — | 1.14 s |
/// | 400 | — | 18.2 s |
///
/// So `32`: eight times spec §4.3's canonical endpoint, which declares four. A
/// 32-parameter definition of the shape anyone actually writes costs 5.9 ms and
/// 34 branches, so it publishes comfortably — while the worst case the limit
/// still admits costs 122 ms of rendering before being refused, against the
/// 18.2 s that was reachable before. Above 32 the expensive curve is steep
/// (152 ms at 36, 284 ms at 50), and below it a legitimate wide endpoint starts
/// to feel it.
///
/// It also pulls the two limits apart, though it does not separate them
/// completely, and the doc used to claim it did. 62 purely linear parameters
/// render to exactly 64 branches, so without this bound an ordinary wide
/// endpoint could trip [`MAX_DESCRIBED_BRANCHES`] on width alone; at 32 the
/// linear shape tops out at 34. But the `2n + 2` flag spelling —
/// `{% if p %} AND s = 'active' {% endif %}`, which `check_params` accepts
/// because `{% if p %}` counts as a reference — measures 58 at *n* = 28, 62 at
/// 30, **64 at 31 and 66 at 32**. So the two limits still MEET, at exactly the
/// width this bound admits: a 32-parameter definition of that shape is refused
/// by the branch limit rather than this one. That is a loud, actionable
/// refusal naming a real cost, not a correctness hole — but it is not the clean
/// separation the earlier wording promised.
const MAX_DECLARED_PARAMS: usize = 32;

/// How many structurally distinct branches one ENDPOINT may publish, summed
/// across its statements.
///
/// **Endpoint-wide, not per statement** (spec §4.1.1). Publish describes every
/// branch of every statement, so a per-statement limit would grow with exactly
/// the thing it exists to bound: ten statements would license ten times one
/// statement's engine round trips. [`validate`] enumerates every statement's
/// renderings, sums the counts and checks the total once, before the first
/// describe. The table below is still measured per definition, because that is
/// how an author writes one; the budget is what they add up to.
///
/// # What it bounds, and what it does not
///
/// It bounds the ENGINE half: publish describes every distinct rendering, so
/// this is the number of round trips one publish may cost. It does **not** bound
/// the rendering that produces that number — the count has to be known before it
/// can be refused, so an endpoint still renders up to `64 × (optional + 2)`
/// times per statement (`render::MAX_PROBES` assignments per context) before this
/// limit can say no. That residual is synchronous CPU on an async task, and it is
/// bounded by [`MAX_DECLARED_PARAMS`] instead, which is a scan and runs first —
/// though only per statement, so an endpoint's worst-case render cost is that
/// bound times the number of statements. The spec asked for that to be
/// re-measured rather than carried over, and it was: **121 ms per statement,
/// linear in N**, which is why the third bound exists.
/// [`egret_dataapi_api::MAX_STATEMENTS`] caps N, and it is the only one of the
/// three that runs before `validate_endpoint`'s first scan. The three are parts
/// of one bound, not one limit and two spares.
///
/// # Why it refuses rather than truncates
///
/// Truncating drops whichever branch fell off the end, and an undescribed branch
/// is a column missing from the union — the exact `503 schema_drift` this gate
/// exists to remove. A loud publish is the honest failure; a quiet one is the
/// bug.
///
/// # Where the number comes from
///
/// Measured, by counting the describes one publish actually costs:
///
/// | definition | distinct branches |
/// |---|---|
/// | spec §4.3's canonical endpoint (4 parameters, 3 `{% if %}`, a `{% for %}`) | 10 |
/// | *n* optional parameters whose blocks are OFF at the representative values | *n* + 1 or *n* + 2 — 22 at *n* = 20 |
/// | *n* optional parameters whose blocks are ON at the representative values | 12 at *n* = 4, 38 at *n* = 8, 57 at *n* = 10, **68 at *n* = 11**, 212 at *n* = 20 |
///
/// **The split is not by TYPE.** It is whether a parameter's candidate values
/// can turn its block OFF from a context where the representative values turn it
/// ON. When blocks start off, every probe turns exactly one on, and the same
/// single-block branch is reached from every context — they all collapse, and
/// the cost is linear. When blocks start on, a probe from an absent context
/// turns off a SECOND block, and the pairs are what grow.
///
/// # Which side a parameter lands on is decided by its CANDIDATES, not its type
///
/// The question is only ever: does this parameter have a candidate value that
/// turns its block OFF from a context where the representative values turn it
/// ON? Where the candidates come from is what differs by type, and it is why no
/// type is reliably on either side. Measured, at *n* = 2/4/8/10/11/20:
///
/// * **`boolean`** — the expensive side, always. Candidates are `true` and
///   `false` unconditionally (they are appended rather than harvested), so
///   `{% if flag %}` starts on and a probe turns it off: **4/12/38/57/68/212**.
/// * **`enum` compared against `allowed[0]`** — the expensive side, because
///   `allowed` always supplies the other values: **4/12/38/57/68/212**, digit
///   for digit the boolean row, and independent of how many `allowed` entries
///   there are (2 and 8 measure identically). Compared against any OTHER allowed
///   value it starts off and measures *n* + 1 — 21 at *n* = 20.
/// * **`string`, `number`, `date`** — **either side, and the DEFINITION'S OWN
///   LITERALS decide**, because that is where a non-enum scalar's candidates are
///   harvested from. `{% if s | value == 'x' %} AND a = {{ s }} {% endif %}`
///   starts on but has no candidate that turns it off — the only literal in
///   reach is `'x'` itself, which re-asserts the block — so it measures
///   *n* + 2 (22 at *n* = 20). Add one literal the comparison is false for,
///   `{% if s | value == 'x' %} AND a = 'off' {% endif %}`, and the SAME
///   parameter of the SAME type measures **4/12/38/57/68/212**. One literal in
///   the block body is the whole difference.
/// * A **flag** spelling reaches a third rate: `{% if p %} AND s = 'active'
///   {% endif %}` on a string parameter measures `2n + 2` — the harvested
///   `'active'` cannot turn the block off, but it CAN turn one on from the
///   all-absent context. 18 at *n* = 8, 58 at *n* = 28, 66 at *n* = 32.
///
/// Two earlier revisions of this comment each named a type as the culprit —
/// first "booleans", then "booleans and inspected enums", with `string` called
/// out as always cheap. Both were generalisations from one spelling. The type
/// never decided it; the candidate set did.
///
/// So `64` is 6.4× the canonical endpoint, admits every linear-shape definition
/// at any width [`MAX_DECLARED_PARAMS`] permits (32 such parameters render to 34
/// branches), and admits ten independent always-on toggles while refusing
/// eleven. It is also `render::MAX_PROBES`, deliberately: that is already this
/// crate's stated answer to "how much may one definition cost", and two
/// different budgets would be two numbers to keep in step.
const MAX_DESCRIBED_BRANCHES: usize = 64;

/// Every rendering of the definition publish must describe, in describe order.
///
/// # Why one render is not enough
///
/// A `{% if %}` definition has more than one output schema, and the frozen
/// contract has to cover all of them or the serving path's schema check refuses
/// the row it just fetched — a `503 schema_drift` on a call the endpoint's own
/// generated OpenAPI advertises. Describing only the representative render
/// picked whichever branch the representative values happened to take.
///
/// # The permutation set is the probes PLUS the absent contexts
///
/// [`crate::render::render_permutations`] varies each parameter over the
/// definition's own literals, an enum's `allowed` values and a boolean's two
/// values — which reaches the other side of `{% if mode | value == 'full' %}`
/// but never the other side of `{% if region %}`, because every representative
/// value is deliberately truthy and a string has no closed value space. The
/// branch a caller reaches by sending NOTHING is only rendered by an ABSENT
/// context, and that is the branch most calls take. [`check_render_rules`]
/// builds the same set for its own rules, and for the same reason.
///
/// # …plus the resting contexts, for a parameter that HAS a default
///
/// A defaulted parameter is never absent, so it gets no absent context — which
/// left its `default` reachable only as a probe candidate, and for an enum not
/// even that. [`resting`] renders the definition with every default applied,
/// which is exactly the assignment a call with no query string produces. See
/// the comment at that push.
///
/// Deduplicated by the rendered statement across all of them, so a definition
/// with no branch costs exactly one describe however many parameters it
/// declares.
fn branch_renderings(
    sql: &str,
    params: &[ParamSpec],
    iterated: &IteratedParams,
) -> Vec<(String, usize)> {
    let optional: Vec<&str> = params
        .iter()
        .filter(|p| !p.required && p.default.is_none())
        .map(|p| p.name.as_str())
        .collect();
    let mut contexts = vec![representative(iterated, params, &[])];
    for name in &optional {
        contexts.push(representative(iterated, params, &[name]));
    }
    if optional.len() > 1 {
        contexts.push(representative(iterated, params, &optional));
    }
    // The RESTING contexts: every defaulted parameter at its own `default`.
    //
    // A parameter with a `default` is never absent — serving substitutes the
    // default — so it gets no absent context above, and the only other route
    // its default had into the render set was being a probe CANDIDATE. For an
    // enum that route does not exist at all: `candidates` answers an enum with
    // its `allowed` list and never consults the definition's literals, and the
    // list is cut at `render::MAX_ENUM_CANDIDATES`. So an enum whose `default`
    // sat past the cut was never rendered, its branch never described, and the
    // call with NO query string — the one `openapi_gen` advertises as the
    // default — was a permanent `503 schema_drift`. The §1.3 defect again,
    // through the one parameter type that is supposed to be the most tightly
    // described: a plain `string` would have been probed with the same value,
    // because a non-enum scalar harvests its candidates from the definition's
    // own literals.
    //
    // A CONTEXT rather than a candidate, deliberately. A candidate competes for
    // `render::MAX_PROBES` and can be starved by a wide definition; a context
    // is always rendered, it costs the same for every parameter type, and it is
    // literally the assignment a bare call produces. Guarded on there being a
    // default at all, so the common endpoint pays nothing.
    if params.iter().any(|p| p.default.is_some()) {
        contexts.push(resting(iterated, params, &[]));
        if !optional.is_empty() {
            // The bare call in full: defaults applied AND every parameter that
            // has no default omitted.
            contexts.push(resting(iterated, params, &optional));
        }
    }

    let mut renderings: Vec<(String, usize)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ctx in &contexts {
        for (rendered, binds) in crate::render::render_permutations(sql, ctx) {
            if seen.insert(rendered.clone()) {
                // `binds.len()` is the `param_count` `describe_as_with_params`
                // wants — the same quantity the single-statement path passed.
                renderings.push((rendered, binds.len()));
            }
        }
    }
    renderings
}

/// [`branch_renderings`], guaranteed non-empty.
///
/// **Split out of [`describe_branches`] so the budget can be an ENDPOINT's.**
/// The count has to be known for every statement before the first one is
/// described (spec §4.1.1), and the fallback below is part of the count: a
/// statement that reaches it costs one describe like any other. Leaving it
/// inside the describe loop would have made the sum wrong by one per statement
/// that took it — invisible at N = 1, where the number was never summed.
fn renderings_for(
    sql: &str,
    params: &[ParamSpec],
    iterated: &IteratedParams,
) -> std::result::Result<Vec<(String, usize)>, ValidationError> {
    let mut renderings = branch_renderings(sql, params, iterated);
    if renderings.is_empty() {
        // Unreachable while `check_render_rules` has just rendered the
        // representative context, but `render_permutations` SKIPS an assignment
        // that does not render, so an empty result is silence rather than an
        // error. Render the representative context directly: its failure is the
        // one worth reporting, and its success is a branch to describe.
        let ctx = representative(iterated, params, &[]);
        let (rendered, binds) =
            crate::render::render(sql, &ctx).map_err(ValidationError::Render)?;
        renderings.push((rendered, binds.len()));
    }
    Ok(renderings)
}

/// Describe every branch of ONE statement and fold the results into one
/// contract.
///
/// A `Vec` accumulator, not a map: the union is a PUBLISHED contract, and a
/// hash order would make two publishes of byte-identical input declare their
/// columns in different orders.
///
/// `required` is computed only once every branch has been described. Doing it
/// incrementally would mark the first branch's columns required before
/// anything knows whether a later branch omits them.
///
/// Takes the renderings rather than computing them, and takes `database_id`
/// rather than the draft: the caller has already enumerated every statement's
/// branches to spend the endpoint's describe budget, and handing this the draft
/// again would leave it able to disagree with the count that budget was checked
/// against.
async fn describe_branches(
    port: &dyn egret_query_api::QueryExecPort,
    workspace: Uuid,
    principal: UserId,
    database_id: Uuid,
    renderings: Vec<(String, usize)>,
) -> Result<DescribedBranches> {
    let mut union: Vec<ColumnSpec> = Vec::new();
    let mut per_branch: Vec<Vec<String>> = Vec::new();
    for (rendered, bind_count) in &renderings {
        let columns = port
            .describe_as_with_params(workspace, database_id, principal, rendered, *bind_count)
            .await?;
        if columns.is_empty() {
            return Err(ValidationError::DescribeReturnedNoColumns.into());
        }
        // A duplicate WITHIN one branch, checked before the fold and without
        // consulting the types: the fold deduplicates by name, so a second `id`
        // would vanish into the first and the frozen contract would be one
        // column narrower than every row the endpoint fetches.
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        if let Some(name) = names
            .iter()
            .enumerate()
            .find(|(i, name)| names[..*i].contains(name))
            .map(|(_, name)| (*name).to_owned())
        {
            return Err(ValidationError::AmbiguousColumnName { name }.into());
        }
        per_branch.push(names.iter().map(|n| (*n).to_string()).collect());
        for column in &columns {
            let spec = ColumnSpec::from(column);
            match union.iter().position(|u| u.name == spec.name) {
                None => union.push(spec),
                // `compare_column_types`, not `!=`, and the reason is specific
                // to this loop: `bind_count` is computed PER BRANCH, so a
                // conditional definition whose branches interpolate different
                // numbers of parameters describes one branch through
                // `Connector::describe` (client protocol) and another through
                // `describe_with_params` (`DESCRIBE OUTPUT`). On Trino those two
                // paths spell a datetime column's precision differently, so a
                // single unchanging `timestamp` column reported itself as
                // `timestamp` in one branch and `timestamp(0)` in the next and
                // this fold refused the publish outright. Measured live on Trino
                // 483, not argued.
                //
                // Matching on the AGREEMENT rather than calling a second
                // "which is more specific?" helper is what keeps the unguarded
                // case unrepresentable: the preferred spelling is only reachable
                // through an arm that is not `No`.
                Some(i) => match compare_column_types(&union[i].ty, &spec.ty) {
                    TypeAgreement::No => {
                        return Err(ValidationError::ColumnTypeConflict {
                            name: spec.name,
                            first: union[i].ty.clone(),
                            second: spec.ty,
                        }
                        .into());
                    }
                    // The accumulator holds the precision-FREE spelling and this
                    // branch states the precision. Keep the informative one, so
                    // the frozen contract — and therefore `meta.columns` and the
                    // generated document's `x-egret-engine-type` — does not
                    // depend on which branch happened to be described first.
                    //
                    // This also TIGHTENS the >= 3 branch case rather than merely
                    // tidying it. With `timestamp`, `timestamp(3)` and
                    // `timestamp(6)` across three branches, an accumulator that
                    // kept the bare `timestamp` agreed with both of the others
                    // and published a contract naming NEITHER precision;
                    // promoting it means the third branch meets `timestamp(3)`
                    // and is correctly refused as two stated precisions.
                    TypeAgreement::PreferRight => union[i].ty = spec.ty,
                    TypeAgreement::Same | TypeAgreement::PreferLeft => {}
                },
            }
        }
    }
    for column in &mut union {
        column.required = per_branch.iter().all(|names| names.contains(&column.name));
    }
    Ok(DescribedBranches { union, per_branch })
}

/// A published version, as the API reports it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DataApiEndpointVersionDto {
    pub id: Uuid,
    pub version: i32,
    pub note: String,
    /// True for the version `endpoints.live_version_id` points at.
    pub is_live: bool,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// The frozen definition. **This is the only thing P4 may read to decide what
    /// a request returns** — see the module comment.
    pub snapshot: EndpointSnapshot,
}

/// Which version a rollback names.
///
/// Two spellings because they have different reach, and the difference is the
/// point. A [`VersionRef::Number`] is scoped to the endpoint by construction —
/// `version` is only unique within one endpoint's history, so this form cannot
/// name someone else's version even in principle. A [`VersionRef::Id`] is a bare
/// uuid, which is exactly the shape that CAN name another endpoint's version (or
/// another workspace's), and so is the one the containment predicate in
/// [`EndpointStore::rollback`] exists for. Both are supported because the UI
/// lists versions with their ids to hand while a human types a number.
#[derive(Debug, Clone, Copy)]
pub enum VersionRef {
    Number(i32),
    Id(Uuid),
}

/// What `publish`/`rollback` report back about the version now being served.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DataApiPublishedDto {
    pub version_id: Uuid,
    pub version: i32,
    pub state: String,
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

impl EndpointStore {
    /// The `(delegated, options.impersonate)` pair for `database` (which must
    /// belong to `workspace`) — the two columns that decide whether the engine
    /// runs a query as the CALLER. See [`runs_per_principal`].
    ///
    /// Read here rather than passed in: it is the input to
    /// [`check_cache_scope`], and a caller who supplied it would be supplying the
    /// answer to the safety question. `NotFound` for a database outside the
    /// workspace, matching every other cross-workspace answer in this crate.
    ///
    /// **Returns the PAIR, and used to return `delegated` alone.** The old
    /// signature could not express the unsafe configuration at all, so every
    /// caller of it was structurally incapable of noticing
    /// `{delegated: false, impersonate: true}`.
    pub async fn connection_per_principal_pair(
        &self,
        workspace: Uuid,
        database: Uuid,
    ) -> Result<(bool, bool)> {
        let row: Option<(bool, serde_json::Value)> = sqlx::query_as(
            "SELECT delegated, options FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(database)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let (delegated, options) = row.ok_or_else(|| Error::NotFound("database".into()))?;
        Ok((delegated, impersonate_flag(&options)))
    }

    /// Freeze `draft` into a new immutable version and make it the live one.
    ///
    /// # Atomicity
    ///
    /// `describe_as` has already run by the time this is called, and that is
    /// deliberate. The requirement is that a failed describe cannot leave a
    /// half-published endpoint — which is satisfied absolutely by doing no write
    /// at all until the columns are in hand, and satisfied better than by holding
    /// a pooled Postgres connection open across a thirty-second engine round
    /// trip (which would let a handful of concurrent publishes against a slow
    /// coordinator exhaust the pool for every other request in the process).
    /// The two WRITES — insert the version, repoint the endpoint — are what needs
    /// a transaction, and they are in one.
    ///
    /// # Why the draft is re-read under a row lock, and re-VALIDATED
    ///
    /// `columns` were resolved from the draft as it was BEFORE the describe. If
    /// the author edited `sql` in that window, committing would freeze one
    /// version's SQL beside another version's output schema — a snapshot that
    /// lies about its own contract, and nothing downstream would ever notice. The
    /// lock also serialises concurrent publishes of the same endpoint, so
    /// `max(version) + 1` cannot produce two rows with the same number (the
    /// `UNIQUE (endpoint_id, version)` would catch that, but as a 500 rather than
    /// as the retryable conflict it is).
    ///
    /// Identity on those three fields is NOT enough on its own, and the gap was a
    /// live hole: the snapshot is built from `current` for **every** field, while
    /// `validate_definition` ran against the pre-lock draft. So a PATCH landing
    /// during the describe could set `cache.scope = shared` on a delegated
    /// connection, pass a check that never looked at `cache`, and be frozen —
    /// publishing the one configuration [`check_cache_scope`] exists to make
    /// unpublishable.
    ///
    /// The fix is to re-run the whole PURE validator against `current`, inside
    /// the transaction. It costs no engine call and no extra pool time, and it
    /// makes the rule structural: what is frozen is what was validated, because
    /// they are now the same read. `delegated` is re-read on the same transaction
    /// and frozen into the snapshot, so the value validated and the value stored
    /// are one (the live column can still move afterwards — see the comment at
    /// that read, and the module comment on why the frozen witness exists).
    ///
    /// `limits` and `rate_limit` went through the same window, and re-validation
    /// does NOT fix them — there is no rule for those fields anywhere in this
    /// crate, so there is nothing for the re-run to check. What they gain is
    /// consistency: they are frozen from the same read that was validated,
    /// rather than from a row nobody looked at. See the module comment's "What
    /// is not validated at all".
    ///
    /// **`output` used to be named in that list, and that has been false since
    /// M1.** Three rules read it now.
    /// [`ValidationError::BranchingArraysShape`] reads `output.shape` and
    /// [`ValidationError::OutputNamingCollision`] reads `output.naming`; both
    /// need the DESCRIBE results, so both run in [`validate`], before this
    /// transaction and against the pre-lock draft, and the re-run below cannot
    /// re-check them. They are handled the way `sql` is instead — by the
    /// identity check below, as a retryable conflict.
    ///
    /// The third is [`egret_dataapi_api::validate_envelope`], and it is a pure
    /// rule, so [`validate_endpoint`] takes `output` and re-checks it HERE
    /// against the locked row. That is the field that decides the shape of every
    /// response body, and it is what makes "what is frozen is what was
    /// validated" true of the envelope as well.
    ///
    /// # State
    ///
    /// `disabled` is PRESERVED. Publishing a new version of a deliberately
    /// disabled endpoint must not put it back in service — that is the same
    /// surprise [`EndpointStore::rollback`] refuses, and the two rules would
    /// otherwise disagree for no reason. A `draft` or `live` endpoint becomes
    /// `live`; taking a disabled one back into service is [`EndpointStore::enable`],
    /// which is explicit and has its own route.
    pub async fn publish(
        &self,
        workspace: Uuid,
        id: Uuid,
        by: UserId,
        note: &str,
        draft: &DataApiEndpointDto,
        described: Vec<StatementSnapshot>,
    ) -> Result<DataApiPublishedDto> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;

        let current = self.locked(&mut tx, workspace, id).await?;
        // The four inputs the resolved `columns` depend on, plus `limits`. A change to any of
        // them makes `columns` stale, which no amount of re-validation would
        // detect — only a fresh describe would, and that is the caller's to retry.
        //
        // `output` is the fourth and was added last. It reaches `validate()` —
        // the `arrays`-over-a-branching-union refusal reads `draft.output.shape`
        // — and that refusal runs BEFORE this transaction, against the pre-lock
        // draft, while `output` is independently patchable. So an author could
        // validate a branching definition as `objects`, have a PATCH set
        // `shape: arrays` during the describe round trips, and freeze an
        // `arrays` snapshot over a union no positional row can express:
        // `shape_rows` zips live against frozen by INDEX, so every branch but
        // the widest is a permanent `503`. Exactly the hole the paragraph above
        // describes closing for `cache.scope`, one field along.
        //
        // `limits` is the fifth, and it is here for a different reason: the
        // resolved `columns` do not depend on it at all. `probe_transform`
        // does — it projects a transform's measured per-row output growth
        // against `limits.max_rows` and refuses a publish that cannot fit under
        // the 1 MiB output cap — and that probe, like the describe, runs before
        // this transaction. Without this clause an author could validate a
        // transform at `max_rows: 100`, PATCH it to a million while the
        // describes were in flight, and freeze a snapshot whose own numbers say
        // it must fail. The re-validation below cannot catch it:
        // `probe_transform` deliberately does NOT run inside this transaction —
        // see its doc comment on where the evaluations are spent.
        if current.statements != draft.statements
            || current.database_id != draft.database_id
            || current.params != draft.params
            || current.output != draft.output
            || current.limits != draft.limits
        {
            return Err(Error::Conflict(
                "the endpoint was edited while it was being published; validate and publish again"
                    .into(),
            ));
        }

        // Same statement as `connection_per_principal_pair`, issued on the
        // transaction so the validation below and the values frozen into the
        // snapshot are the same read — and one statement, so the two halves of
        // the pair cannot come from two different moments.
        //
        // That is all it buys. It does NOT pin the column: there is no `FOR SHARE`
        // on `query.databases` and the isolation level is READ COMMITTED, so a
        // concurrent flip can commit before this transaction does. Locking it
        // would not help either, because the flip is equally free the instant
        // after the commit. That residual is precisely what the frozen witness
        // PAIR exists for — P4 compares both halves against the live row and
        // refuses a shared cache when either disagrees — and it is not something
        // publish can close on its own.
        //
        // Read in ONE statement so the two halves cannot come from two different
        // moments. `options` is JSONB and the key is spelled exactly as
        // `egret_query_api::ConnOptions::impersonate` serialises it — this crate
        // deliberately depends on no other feature seam, so the coupling is by
        // name, read through `impersonate_flag`, and pinned from the owning side
        // by `egret-query-api`'s `wire_names`.
        let (delegated, options): (bool, serde_json::Value) = sqlx::query_as(
            "SELECT delegated, options FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(current.database_id)
        .bind(workspace)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound("database".into()))?;
        let impersonate = impersonate_flag(&options);

        // The whole pure validator, against the row actually being frozen. See the
        // doc comment: this is what closes the window, not the identity check above.
        // Nothing here renders — `described` was computed by `validate` — so the
        // shape set it returns has no second use on this path.
        let _iterated = validate_endpoint(
            &current.statements,
            &current.params,
            &current.cache,
            &current.output,
            delegated,
            impersonate,
        )?;

        // The name and the SQL come from `current` — the LOCKED row — and only
        // the resolved columns come from the caller. `described` was computed
        // against the pre-lock draft, and the identity check above says the two
        // agree; taking the text from the locked row anyway is what makes that
        // an assertion rather than a promise, and it is the same reason `path`
        // below is read from `current` rather than echoed back.
        //
        // Paired by NAME, not by position. `described` carries the name each
        // column list was resolved for, and a positional zip discards it: the
        // one thing that could go wrong here — `total`'s columns frozen onto
        // `rows`' SQL, an endpoint that then answers `503 schema_drift` on
        // every call it does not answer wrong — is a crossing that pairing by
        // name cannot express. It is guarded today by the identity check above,
        // which is a guarantee held in a different function; this is the same
        // guarantee held by the data.
        //
        // A missing name is answered as the conflict it would have to be, not as
        // a panic on an index. `current.statements` has just been through
        // `validate_statements`, so its names are unique and the match is
        // unambiguous.
        let mut statements: Vec<StatementSnapshot> = Vec::with_capacity(current.statements.len());
        for statement in &current.statements {
            let Some(resolved) = described.iter().find(|d| d.name == statement.name) else {
                return Err(Error::Conflict(
                    "the endpoint was edited while it was being published; validate and publish \
                     again"
                        .into(),
                ));
            };
            statements.push(StatementSnapshot {
                // The name and the SQL come from `current` — the LOCKED row —
                // and only the resolved columns come from the caller.
                name: statement.name.clone(),
                sql: statement.sql.clone(),
                columns: resolved.columns.clone(),
            });
        }
        if described.len() != statements.len() {
            // Every statement of the locked row was matched, so this can only be
            // `described` carrying MORE than the row does — a describe of a
            // statement that no longer exists, which is the same edit-during-
            // publish and gets the same answer.
            return Err(Error::Conflict(
                "the endpoint was edited while it was being published; validate and publish again"
                    .into(),
            ));
        }

        let snapshot = EndpointSnapshot {
            statements,
            method: current.method.clone(),
            // The stored path, which `normalize_path` already canonicalised. The
            // snapshot must never carry a spelling the endpoint row does not have.
            path: current.path.clone(),
            database_id: current.database_id,
            params: current.params.clone(),
            output: current.output.clone(),
            cache: current.cache.clone(),
            limits: current.limits.clone(),
            rate_limit: current.rate_limit.clone(),
            // The witness PAIR, both halves from the one SELECT above. Freezing
            // only `delegated` — which is what this did — left
            // `{delegated: false, impersonate: true}` indistinguishable from a
            // genuinely shared-safe connection forever after.
            delegated,
            impersonate,
            // The stamp, written here and nowhere else, because this is the one
            // place `validate_definition` has just run against the row being
            // frozen. §7 of the parameters spec said "every endpoint with
            // parameters must be re-published after P4.5" — an instruction to
            // whoever reads it. This is the enforcement: a version frozen before
            // the gate existed carries `0`, and the serving path refuses it.
            param_gate: PARAM_GATE,
        };
        let payload = serde_json::to_value(&snapshot)
            .map_err(|e| Error::Internal(anyhow::anyhow!("serialize snapshot: {e}")))?;

        let (version_id, version): (Uuid, i32) = sqlx::query_as(
            "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot, note, created_by) \
             SELECT $1, COALESCE(MAX(version), 0) + 1, $2, $3, $4 \
               FROM dataapi.endpoint_versions WHERE endpoint_id = $1 \
             RETURNING id, version",
        )
        .bind(id)
        .bind(&payload)
        .bind(note)
        .bind(by.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        // `deleted_at IS NULL` guards a `CASE` that fails two different ways, and
        // both were measured. On a trashed DRAFT the `ELSE 'live'` branch violates
        // migration 0126's CHECK, so this arrived as a 500 naming
        // `dataapi_endpoints_trashed_not_live`. On a trashed DISABLED endpoint the
        // `CASE` leaves the state alone, the CHECK stays satisfied, and this
        // SUCCEEDED — freezing a new immutable version into the trash and reporting
        // it as published. The second is the worse of the two, and no constraint
        // could have caught it.
        //
        // `fetch_optional`, and the no-row path is a conflict rather than a 404
        // because `locked` above already matched this `id` in this `workspace` and
        // still holds `FOR UPDATE` on the row: the ONLY predicate that can have
        // stopped it matching is the one added here. The version row inserted just
        // above goes away with the transaction, which is why nothing has to undo it.
        let state: Option<String> = sqlx::query_scalar(
            "UPDATE dataapi.endpoints SET live_version_id = $1, \
               state = CASE WHEN state = 'disabled' THEN 'disabled' ELSE 'live' END, \
               updated_at = now() \
             WHERE id = $2 AND workspace_id = $3 AND deleted_at IS NULL RETURNING state",
        )
        .bind(version_id)
        .bind(id)
        .bind(workspace)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(state) = state else {
            return Err(Error::Conflict(
                "endpoint is in the trash; restore it before publishing a new version".into(),
            ));
        };

        tx.commit().await.map_err(db_err)?;
        Ok(DataApiPublishedDto {
            version_id,
            version,
            state,
        })
    }

    /// Point `live_version_id` at an existing version of THIS endpoint.
    ///
    /// `v.endpoint_id = e.id` in the statement below is the containment predicate:
    /// without it, a caller holding any version uuid could make one endpoint serve
    /// another's frozen SQL — including one from a different workspace, since a
    /// bare uuid carries no ownership. A version belongs to exactly one endpoint,
    /// and a version that is not this one's is reported as `NotFound`, the same
    /// answer as a version that does not exist.
    ///
    /// `state` is left alone. Rolling back is a statement about WHICH version is
    /// served, not about whether the endpoint is serving; a disabled endpoint that
    /// silently came back to life because someone corrected its version would be a
    /// surprise with no route that asked for it.
    ///
    /// **`e.deleted_at IS NULL` is in the same `WHERE`, and leaving `state` alone is
    /// exactly why it had to be.** Because this writes only `live_version_id`,
    /// migration 0126's CHECK never fires, so before this predicate a rollback of a
    /// TRASHED endpoint succeeded: it silently repointed which frozen SQL the
    /// endpoint would serve the moment anything enabled it. A guarantee that lives
    /// in a constraint on `state` cannot cover a writer that does not touch `state`.
    pub async fn rollback(
        &self,
        workspace: Uuid,
        id: Uuid,
        target: VersionRef,
    ) -> Result<DataApiPublishedDto> {
        let (number, version_id) = match target {
            VersionRef::Number(n) => (Some(n), None),
            VersionRef::Id(v) => (None, Some(v)),
        };
        let row: Option<(Uuid, i32, String)> = sqlx::query_as(
            "UPDATE dataapi.endpoints e SET live_version_id = v.id, updated_at = now() \
               FROM dataapi.endpoint_versions v \
              WHERE e.id = $1 AND e.workspace_id = $2 \
                AND e.deleted_at IS NULL \
                AND v.endpoint_id = e.id \
                AND (v.version = $3 OR v.id = $4) \
             RETURNING v.id, v.version, e.state",
        )
        .bind(id)
        .bind(workspace)
        .bind(number)
        .bind(version_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        // Three reasons for no row and only two answers: an absent endpoint, an
        // absent-or-foreign VERSION and a trashed endpoint. The first two stay the
        // 404 they were — a version of somebody else's endpoint must not be
        // distinguishable from one that does not exist — and only the trash becomes
        // a 409, which is what `trashed_conflict_or`'s `otherwise` is carrying here.
        let Some((version_id, version, state)) = row else {
            return Err(self
                .trashed_conflict_or(
                    workspace,
                    id,
                    "changing which version it serves",
                    Error::NotFound("endpoint version".into()),
                )
                .await);
        };
        Ok(DataApiPublishedDto {
            version_id,
            version,
            state,
        })
    }

    /// Stop serving, keeping the published history and the live pointer intact.
    ///
    /// `live_version_id` is untouched on purpose: re-enabling then costs nothing —
    /// no re-validation, no second describe, and no NEW version number for a
    /// definition that never changed. See [`EndpointStore::enable`].
    pub async fn disable(&self, workspace: Uuid, id: Uuid) -> Result<DataApiEndpointDto> {
        self.set_state(workspace, id, "disabled").await
    }

    /// Serve again from whatever version was live when it was disabled.
    ///
    /// Refuses an endpoint that has never been published: `state = 'live'` with a
    /// null `live_version_id` is an endpoint that routes and then has nothing to
    /// answer with. A 409, not a 404 — the endpoint exists, it is just not in a
    /// state this can be asked of.
    pub async fn enable(&self, workspace: Uuid, id: Uuid) -> Result<DataApiEndpointDto> {
        let current = self.get(workspace, id).await?;
        if current.live_version_id.is_none() {
            return Err(Error::Conflict(
                "endpoint has never been published; publish it instead".into(),
            ));
        }
        self.set_state(workspace, id, "live").await
    }

    /// Every published version of `id`, newest first.
    ///
    /// **The read path Step 7's mutation targets.** `v.snapshot` is selected
    /// verbatim; the endpoint row is joined ONLY to scope the query to the
    /// workspace and to mark which version is live. Sourcing any part of the
    /// returned snapshot from `e.*` instead would make a published version follow
    /// its draft, and is exactly the shape P4 must not grow.
    pub async fn versions(
        &self,
        workspace: Uuid,
        id: Uuid,
    ) -> Result<Vec<DataApiEndpointVersionDto>> {
        // Existence first, so a foreign or absent endpoint is a 404 rather than an
        // empty list — an empty list is a legitimate answer for an unpublished
        // endpoint, and the two must not be confused.
        self.get(workspace, id).await?;
        type Row = (
            Uuid,
            i32,
            String,
            bool,
            Option<Uuid>,
            chrono::DateTime<chrono::Utc>,
            serde_json::Value,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT v.id, v.version, v.note, COALESCE(e.live_version_id = v.id, false), \
                    v.created_by, v.created_at, v.snapshot \
               FROM dataapi.endpoint_versions v \
               JOIN dataapi.endpoints e ON e.id = v.endpoint_id \
              WHERE v.endpoint_id = $1 AND e.workspace_id = $2 \
              ORDER BY v.version DESC",
        )
        .bind(id)
        .bind(workspace)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(
                |(id, version, note, is_live, created_by, created_at, snapshot)| {
                    Ok(DataApiEndpointVersionDto {
                        id,
                        version,
                        note,
                        is_live,
                        created_by,
                        created_at,
                        // A stored snapshot that will not deserialize is a 500,
                        // never a default: it is a published contract, and quietly
                        // substituting one would misreport what clients are being
                        // served.
                        snapshot: serde_json::from_value(snapshot).map_err(|e| {
                            Error::Internal(anyhow::anyhow!(
                                "stored snapshot for version {version} is not readable: {e}"
                            ))
                        })?,
                    })
                },
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_dataapi_api::{LimitsSpec, ParamLocation, RateLimitSpec, TransformSpec};

    fn param(name: &str, ty: ParamType) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            ty,
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
        }
    }

    /// [`validate_preview`] with no siblings.
    ///
    /// Two things at once, which is why every existing preview test uses it
    /// unchanged: a ONE-statement endpoint, and the request an OLD CLIENT sends
    /// — one that never learnt the `siblings` field and whose body therefore
    /// deserialises to `SiblingStatements::default()`. So the whole preview
    /// suite below is also the assertion that the field's arrival changed
    /// nothing for a body that omits it.
    fn preview(
        sql: &str,
        params: &[ParamSpec],
    ) -> std::result::Result<IteratedParams, ValidationError> {
        preview_with(sql, params, &OutputSpec::default())
    }

    /// A preview of a draft whose `output` is not the default — the transform
    /// cases.
    fn preview_with(
        sql: &str,
        params: &[ParamSpec],
        output: &OutputSpec,
    ) -> std::result::Result<IteratedParams, ValidationError> {
        validate_preview(sql, &SiblingStatements::default(), params, output)
    }

    fn siblings(sqls: &[&str]) -> SiblingStatements {
        SiblingStatements(sqls.iter().map(|s| (*s).to_string()).collect())
    }

    /// The declared-parameter bound, from both sides.
    #[test]
    fn a_definition_declaring_more_parameters_than_the_limit_is_refused() {
        let build = |n: usize| {
            let mut sql = String::from("SELECT id FROM t WHERE 1=1");
            let mut params = Vec::new();
            for i in 0..n {
                sql.push_str(&format!(
                    " {{% if p{i} %}} AND a{i} = {{{{ p{i} }}}}{{% endif %}}"
                ));
                params.push(param(&format!("p{i}"), ParamType::String));
            }
            (sql, params)
        };

        // At the limit, and of the shape a real wide endpoint has: it publishes.
        let (sql, params) = build(MAX_DECLARED_PARAMS);
        assert!(
            validate_definition(
                &sql,
                &params,
                &cache(CacheScope::PerPrincipal),
                false,
                false
            )
            .is_ok(),
            "a definition AT the limit must still validate, or the bound is a \
             refusal of ordinary work"
        );

        let (sql, params) = build(MAX_DECLARED_PARAMS + 1);
        assert_eq!(
            validate_definition(
                &sql,
                &params,
                &cache(CacheScope::PerPrincipal),
                false,
                false
            ),
            Err(ValidationError::TooManyParameters {
                declared: MAX_DECLARED_PARAMS + 1,
                limit: MAX_DECLARED_PARAMS,
            })
        );
    }

    /// The STATEMENT bound, from publish's own gate, and from the side that
    /// makes it worth having: it must refuse before anything renders.
    ///
    /// `validate_endpoint` is what both `POST …/validate` and `publish` go
    /// through, so this is the publish-time refusal rather than a restatement of
    /// `validate_statements`' own unit test. The variant is asserted, not just
    /// the fact of an error: this function has a dozen refusals and several of
    /// them are reachable from the definition below.
    ///
    /// Measured rather than asserted structurally, for the same reason as
    /// [`the_parameter_bound_is_checked_before_anything_renders`] below. 256
    /// statements of this shape — 32 boolean parameters each, the most expensive
    /// statement the other two limits still admit — cost **31.6 s in release**
    /// (2.6 s of scans and renders, 29.0 s of branch enumeration) before this
    /// bound existed, and far longer in the debug build this test runs under.
    /// The budget is two orders of magnitude below that, so a reading inside it
    /// cannot coincide with the statements having been rendered.
    #[test]
    fn the_statement_bound_is_checked_before_anything_renders() {
        let mut sql = String::from("SELECT id FROM t WHERE 1=1");
        let mut params = Vec::new();
        for i in 0..MAX_DECLARED_PARAMS {
            sql.push_str(&format!(
                " {{% if p{i} %}} AND a{i} = {{{{ p{i} }}}}{{% endif %}}"
            ));
            params.push(param(&format!("p{i}"), ParamType::Boolean));
        }
        let statements: Vec<StatementSpec> = (0..256)
            .map(|i| StatementSpec {
                name: format!("s{i}"),
                sql: sql.clone(),
            })
            .collect();

        let started = std::time::Instant::now();
        let got = validate_endpoint(
            &statements,
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            false,
            false,
        );
        let elapsed = started.elapsed();
        assert_eq!(
            got,
            Err(ValidationError::Statement(
                egret_dataapi_api::StatementError::TooManyStatements {
                    count: 256,
                    limit: egret_dataapi_api::MAX_STATEMENTS,
                }
            ))
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "the refusal must precede the rendering; this took {elapsed:?}, which \
             means 256 statements were rendered before being refused"
        );
    }

    /// The other side of the statement bound, and its own test: a legitimate
    /// several-statement endpoint must still validate. The canonical one is a
    /// `rows` query and the `total` that paginates it (spec §4.4).
    #[test]
    fn the_canonical_two_statement_endpoint_still_validates() {
        let statements = vec![
            StatementSpec {
                name: "rows".into(),
                sql: "SELECT id FROM t WHERE region = {{ region }}".into(),
            },
            StatementSpec {
                name: "total".into(),
                sql: "SELECT count(*) AS n FROM t WHERE region = {{ region }}".into(),
            },
        ];
        let params = vec![param("region", ParamType::String)];
        assert_eq!(
            validate_endpoint(
                &statements,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                false,
                false
            ),
            // No statement loops over `region`, so it is a scalar for BOTH — the
            // set is what publish validated each of them against.
            Ok(IteratedParams(Vec::new()))
        );
    }

    /// A parameter's SHAPE is one endpoint-wide decision, and publish has to
    /// make the same one the serving path will.
    ///
    /// `serving::params::resolve_supplied` decides list-versus-scalar from the
    /// UNION across every statement (spec §4.2, Task 6): every statement renders
    /// against one `ParamValues`, so a parameter SOME statement iterates is a
    /// list for ALL of them. Publish inferring it per statement agrees only at
    /// N = 1 — and where they disagree the endpoint publishes green and then
    /// answers **500 on every call**, including the no-query-string default the
    /// generated document advertises: `render_error` sends a `SequenceBind` with
    /// no repeated parameter down the `internal` arm, correctly, because it is
    /// an author fault that publish was supposed to have refused.
    ///
    /// Both halves are measured here rather than compared as sets, because the
    /// property is not "the two functions agree" but "what publish validated is
    /// what a caller reaches".
    #[test]
    fn publish_infers_the_shape_serving_will_bind() {
        const SUMMARY: &str = "SELECT 1 AS n FROM t WHERE region = {{ region }}";
        const BY_REGION: &str = "SELECT region AS n FROM t WHERE region IN (\
             {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})";
        let params = vec![param("region", ParamType::String)];

        // What a CALLER reaches. `?region=EU` is single-valued, so nothing about
        // the request makes this a list; `by_region` iterating it does.
        let mut supplied: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        supplied.insert("region".into(), vec!["EU".into()]);
        let resolved =
            crate::serving::params::resolve_supplied(&supplied, &[SUMMARY, BY_REGION], &params)
                .expect("the request itself is valid");
        assert!(
            !resolved.has_repeats,
            "a single-valued query parameter — so `render_error` cannot read the \
             failure below as the caller's fault"
        );
        assert_eq!(
            crate::render::render(SUMMARY, &resolved.values).unwrap_err(),
            crate::render::RenderError::SequenceBind,
            "serving binds `region` as a list, and `summary` cannot bind one"
        );

        // So publish must refuse the endpoint, with THAT failure. Deciding
        // `summary`'s shape from `summary` alone makes this `Ok(())`.
        let statements = vec![
            StatementSpec {
                name: "summary".into(),
                sql: SUMMARY.into(),
            },
            StatementSpec {
                name: "by_region".into(),
                sql: BY_REGION.into(),
            },
        ];
        assert_eq!(
            validate_endpoint(
                &statements,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                false,
                false
            ),
            Err(ValidationError::Render(
                crate::render::RenderError::SequenceBind
            )),
            "publish must reach the same conclusion the first call would"
        );
    }

    /// The same §4.2 pair, through **preview** — the half M2 left open.
    ///
    /// `summary` uses `region` as a scalar and `by_region` loops over it, so the
    /// endpoint does not publish (the test above). Preview judged the shape from
    /// the statement on screen alone, so it went GREEN on `summary` and the
    /// author was told "fine" and then told "no" by publish. It failed closed —
    /// the refusal that counts still happened — which is exactly why this is a
    /// correctness-of-FEEDBACK bug and why the fix is to make preview see the
    /// siblings, not to relax anything.
    #[test]
    fn preview_judges_a_parameter_shape_across_the_siblings() {
        const SUMMARY: &str = "SELECT 1 AS n FROM t WHERE region = {{ region }}";
        const BY_REGION: &str = "SELECT region AS n FROM t WHERE region IN (\
             {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})";
        let params = vec![param("region", ParamType::String)];

        // An OLD CLIENT — no `siblings` field in the body — keeps exactly the
        // behaviour it had: the union of one, and `summary` green on its own.
        // That is the compatibility promise, and it is a green the author is
        // then refused, which is the defect the field exists to remove.
        assert!(preview(SUMMARY, &params).is_ok());

        // Told the siblings, preview reaches publish's answer — and reaches it
        // by the SAME route, `IteratedParams` across every statement, so it is
        // the same conclusion rather than a second rule that agrees today.
        assert_eq!(
            validate_preview(
                SUMMARY,
                &siblings(&[BY_REGION]),
                &params,
                &OutputSpec::default()
            ),
            Err(ValidationError::Render(
                crate::render::RenderError::SequenceBind
            )),
        );

        // And the direction the fix must NOT go: previewing `by_region` itself,
        // with `summary` as its sibling, still passes. The union is a union — a
        // sibling that uses a parameter as a scalar does not un-iterate it.
        assert!(validate_preview(
            BY_REGION,
            &siblings(&[SUMMARY]),
            &params,
            &OutputSpec::default()
        )
        .is_ok());
    }

    /// Preview's shape decision must be the ENDPOINT's, name for name — not a
    /// second computation that happens to agree on the case above.
    ///
    /// `validate_preview` returns the set for the same reason `validate_endpoint`
    /// does: [`crate::preview::run`] needs it to resolve the author's test values,
    /// and recomputing it there would be the second answer M2 removed.
    #[test]
    fn preview_returns_the_same_shape_the_endpoint_publishes_with() {
        const SUMMARY_ALL: &str = "SELECT count(*) AS n FROM t";
        const BY_REGION: &str = "SELECT region AS n FROM t WHERE region IN (\
             {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})";
        let params = vec![param("region", ParamType::String)];
        let statements = vec![
            StatementSpec {
                name: "summary_all".into(),
                sql: SUMMARY_ALL.into(),
            },
            StatementSpec {
                name: "by_region".into(),
                sql: BY_REGION.into(),
            },
        ];
        let published = validate_endpoint(
            &statements,
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            false,
            false,
        )
        .expect("the §4.2 pair publishes");
        // `summary_all` never mentions `region`, so on its own the union is
        // EMPTY and preview would resolve a scalar where serving binds a list.
        assert!(preview(SUMMARY_ALL, &params).unwrap().names().is_empty());
        assert_eq!(
            validate_preview(
                SUMMARY_ALL,
                &siblings(&[BY_REGION]),
                &params,
                &OutputSpec::default()
            )
            .unwrap()
            .names(),
            published.names(),
        );
        assert_eq!(published.names(), ["region"]);
    }

    /// A sibling is not a definition being validated — it informs the shape and
    /// nothing else.
    ///
    /// It is already stored (it was scanned when it was written) or it is
    /// mid-edit in another tab of the same form. Re-refusing it here would stop
    /// an author previewing the statement they are LOOKING at because a
    /// different one is temporarily broken, which is the same defect as the one
    /// above. Nothing is executed either way: the text never leaves
    /// [`SiblingStatements`], whose field is private to this module.
    #[test]
    fn a_sibling_that_would_not_publish_does_not_block_the_preview() {
        const ON_SCREEN: &str = "SELECT count(*) AS n FROM t";
        let params = vec![param("region", ParamType::String)];
        // A `;`, a `#` and a DELETE — each of which `check_definition_scans`
        // refuses outright when it is the statement being previewed.
        let broken = siblings(&["DELETE FROM t; -- half typed # note"]);
        assert!(validate_preview(ON_SCREEN, &broken, &params, &OutputSpec::default()).is_ok());
        // …while a sibling that DOES loop still counts, so "not scanned" has not
        // become "not read".
        let looping = siblings(&["SELECT region AS n FROM t WHERE region IN (\
             {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %}); # oops"]);
        assert_eq!(
            validate_preview(ON_SCREEN, &looping, &params, &OutputSpec::default())
                .unwrap()
                .names(),
            ["region"],
        );
    }

    /// The bound on the new field, and it is the bound publish already has.
    ///
    /// `siblings` is a list of texts from a request body and each one costs a
    /// template scan, so it needs a number. `MAX_STATEMENTS` is that number
    /// because an endpoint may not have more statements than that in total —
    /// `1 + siblings.len()` is exactly the list a publish would refuse, so the
    /// two cannot drift and no new constant exists to keep in step. Checked
    /// FIRST, before `sql` is scanned and long before anything renders.
    #[test]
    fn preview_bounds_its_siblings_by_the_endpoint_statement_limit() {
        let sqls: Vec<String> = (0..egret_dataapi_api::MAX_STATEMENTS)
            .map(|i| format!("SELECT {i} AS n"))
            .collect();
        let refs: Vec<&str> = sqls.iter().map(String::as_str).collect();
        // One statement plus `MAX_STATEMENTS - 1` siblings is a describable
        // endpoint, and passes.
        assert!(validate_preview(
            "SELECT 1 AS n",
            &siblings(&refs[1..]),
            &[],
            &OutputSpec::default()
        )
        .is_ok());
        // One more is an endpoint publish would refuse, refused here with the
        // same error and the same numbers.
        assert_eq!(
            validate_preview(
                "SELECT 1 AS n",
                &siblings(&refs),
                &[],
                &OutputSpec::default()
            ),
            Err(ValidationError::Statement(
                egret_dataapi_api::StatementError::TooManyStatements {
                    count: egret_dataapi_api::MAX_STATEMENTS + 1,
                    limit: egret_dataapi_api::MAX_STATEMENTS,
                }
            )),
        );
        // And it answers BEFORE the statement scan, the way `validate_endpoint`
        // does: a body carrying too many siblings is refused for the count even
        // when the statement on screen is itself unpublishable.
        assert_eq!(
            validate_preview(
                "SELECT 1; DROP TABLE t",
                &siblings(&refs),
                &[],
                &OutputSpec::default()
            ),
            Err(ValidationError::Statement(
                egret_dataapi_api::StatementError::TooManyStatements {
                    count: egret_dataapi_api::MAX_STATEMENTS + 1,
                    limit: egret_dataapi_api::MAX_STATEMENTS,
                }
            )),
        );
    }

    /// The §4.2 pair the union rule exists FOR, from publish's side: `by_region`
    /// iterates `region` and `summary_all` never mentions it, so the union makes
    /// `region` a list and the statement that does not read it renders anyway.
    /// This is what stops the fix above from being "refuse anything mixed".
    #[test]
    fn a_statement_that_ignores_an_iterated_parameter_still_publishes() {
        let statements = vec![
            StatementSpec {
                name: "summary_all".into(),
                sql: "SELECT count(*) AS n FROM t".into(),
            },
            StatementSpec {
                name: "by_region".into(),
                sql: "SELECT region AS n FROM t WHERE region IN (\
                     {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})"
                    .into(),
            },
        ];
        assert_eq!(
            validate_endpoint(
                &statements,
                &[param("region", ParamType::String)],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                false,
                false
            ),
            // ONE statement loops over `region` and that is enough: the set is
            // the endpoint's, and `summary_all` was validated against it.
            Ok(IteratedParams(vec!["region".into()]))
        );
    }

    /// The bound has to run BEFORE the rendering, or it bounds nothing that
    /// matters: the cost it exists to prevent is paid during the render.
    ///
    /// Measured rather than asserted structurally, because "runs first" is a
    /// claim about cost. 400 all-boolean parameters render in **18.2 s in
    /// release** and far longer in the debug build this test runs under. The
    /// budget here is two orders of magnitude below that, so it cannot coincide
    /// with the rendering happening — the two readings are not close enough to
    /// be confused.
    #[test]
    fn the_parameter_bound_is_checked_before_anything_renders() {
        let mut sql = String::from("SELECT id FROM t WHERE 1=1");
        let mut params = Vec::new();
        for i in 0..400 {
            sql.push_str(&format!(
                " {{% if p{i} %}} AND a{i} = {{{{ p{i} }}}}{{% endif %}}"
            ));
            params.push(param(&format!("p{i}"), ParamType::Boolean));
        }
        let started = std::time::Instant::now();
        let got = validate_definition(
            &sql,
            &params,
            &cache(CacheScope::PerPrincipal),
            false,
            false,
        );
        let elapsed = started.elapsed();
        assert_eq!(
            got,
            Err(ValidationError::TooManyParameters {
                declared: 400,
                limit: MAX_DECLARED_PARAMS,
            })
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "the refusal must precede the rendering; this took {elapsed:?}, which \
             means the definition was rendered before being refused"
        );
    }

    fn cache(scope: CacheScope) -> CacheSpec {
        CacheSpec {
            enabled: true,
            ttl_seconds: 60,
            scope,
        }
    }

    fn enum_param(name: &str, allowed: &[&str]) -> ParamSpec {
        ParamSpec {
            allowed: Some(allowed.iter().map(|s| (*s).to_string()).collect()),
            required: true,
            ..param(name, ParamType::Enum)
        }
    }

    /// **The `allowed` list was the one author-supplied string no rule scanned.**
    ///
    /// `| ident` writes it into the statement verbatim; the definition-side `;`
    /// test reads the RAW definition, which has no `;` in it; and publish renders
    /// only the FIRST entry, so a hostile second entry reached no scan and no
    /// `describe`. It bound zero values, so the placeholder==bind invariant was
    /// satisfied — and a parameterless statement reaches StarRocks through
    /// `execute`, which is COM_QUERY.
    #[test]
    fn an_allowed_value_in_an_identifier_position_must_be_an_identifier() {
        let sql = "SELECT id FROM t ORDER BY {{ sort | ident }}";
        let hostile = enum_param("sort", &["id", "id; DROP TABLE t"]);
        assert_eq!(
            validate_definition(
                sql,
                &[hostile],
                &cache(CacheScope::PerPrincipal),
                false,
                false
            ),
            Err(ValidationError::EnumValueNotIdentifier {
                param: "sort".into(),
                value: "id; DROP TABLE t".into()
            })
        );

        // A qualified name is the most an identifier position needs.
        let ok = enum_param("sort", &["id", "t.total"]);
        assert_eq!(
            validate_definition(sql, &[ok], &cache(CacheScope::PerPrincipal), false, false),
            Ok(())
        );

        // Payloads that carry no separator at all are refused by SHAPE — the
        // rendered-statement scan alone would not see this one.
        let union = enum_param("sort", &["id", "id UNION SELECT secret FROM vault"]);
        assert!(matches!(
            validate_definition(
                sql,
                &[union],
                &cache(CacheScope::PerPrincipal),
                false,
                false
            ),
            Err(ValidationError::EnumValueNotIdentifier { .. })
        ));
    }

    /// …and the shape is required only of the enums that reach the statement as
    /// TEXT. An enum is also how an author constrains a bound VALUE, and a bound
    /// value never reaches the statement at all — refusing `Pending Review`
    /// there would make `ParamType::Enum` unusable for its other purpose.
    #[test]
    fn an_allowed_value_that_is_only_bound_may_be_any_string() {
        let sql = "SELECT id FROM t WHERE status = {{ status }}";
        let spec = enum_param("status", &["Pending Review", "done; not really"]);
        assert_eq!(
            validate_definition(sql, &[spec], &cache(CacheScope::PerPrincipal), false, false),
            Ok(())
        );
    }

    #[test]
    fn identifier_shape_is_letters_digits_underscore_and_at_most_one_dot() {
        for ok in ["id", "_id", "a1", "t.col", "_a._b"] {
            assert!(is_identifier_shaped(ok), "{ok} should be an identifier");
        }
        for bad in [
            "", "1a", "a b", "a;b", "a.b.c", "a-b", "\"a\"", "a(", "id DESC",
        ] {
            assert!(
                !is_identifier_shaped(bad),
                "{bad} should NOT be an identifier"
            );
        }
    }

    #[test]
    fn accepts_select_and_cte() {
        assert!(check_single_select("SELECT 1").is_ok());
        assert!(check_single_select("  select 1  ").is_ok());
        assert!(check_single_select("WITH t AS (SELECT 1) SELECT * FROM t").is_ok());
        assert!(
            check_single_select("SELECT 1;").is_ok(),
            "one trailing semicolon is fine"
        );
    }

    #[test]
    fn rejects_everything_else() {
        assert!(check_single_select("INSERT INTO t VALUES (1)").is_err());
        assert!(check_single_select("DELETE FROM t").is_err());
        assert!(check_single_select("SET x = 1").is_err());
        assert!(check_single_select("SELECT 1; DROP TABLE t").is_err());
        assert!(check_single_select("").is_err());
    }

    /// **These three assertions were INVERTED by the SEV-0 fix, and the inversion
    /// is the point.**
    ///
    /// Each used to assert that a `;` the scanner judged inert was accepted. That
    /// is exactly the judgement the scanner is no longer trusted to make: the
    /// same machinery, applied to `#`, judged a live `; DROP TABLE t` inert and
    /// let it run on a real engine. So `reject_unscannable_bytes` now refuses the
    /// byte without asking, and these become documented FALSE REJECTIONS.
    ///
    /// The trade is stated plainly because it is a real cost, not a free win: a
    /// legitimate `SELECT 'a;b'` is now unpublishable and the author must write
    /// `concat('a', char(59), 'b')`. Accepting that cost buys a guarantee that no
    /// future dialect quirk can take away.
    #[test]
    fn an_inert_semicolon_is_refused_anyway() {
        for sql in [
            // Inside a literal — genuinely inert on both engines.
            "SELECT 'a;b' AS s",
            // Inside a `--` comment — genuinely inert on both engines.
            "SELECT 1 -- ; DROP TABLE t\n",
            // Inside `''`-escaped and quote-delimited regions.
            "SELECT 'a''b;c' AS s",
            r#"SELECT "a;b" FROM t"#,
            "SELECT `a;b` FROM t",
        ] {
            assert_eq!(
                check_single_select(sql),
                Err(ValidationError::MultipleStatements),
                "{sql}"
            );
        }
        // The same queries WITHOUT the semicolon must still publish, or the rule
        // above would simply ban string literals and quoted identifiers.
        for sql in [
            "SELECT 'a,b' AS s",
            "SELECT 1 -- note\n",
            "SELECT 'a''b,c' AS s",
            r#"SELECT "a,b" FROM t"#,
            "SELECT `a,b` FROM t",
        ] {
            assert!(check_single_select(sql).is_ok(), "{sql}");
        }
    }

    /// The quote-escape modelling still has to be right — layer 1 does not
    /// subsume it, because [`check_params`] uses the same scan to decide whether
    /// a placeholder stood where a VALUE stands, and that has nothing to do with
    /// `;`.
    #[test]
    fn quote_escapes_do_not_end_a_literal_early() {
        let p = [param("p", ParamType::String)];
        // `''` is the ANSI escape, so the literal runs to the FINAL quote and the
        // placeholder is inside it under both readings.
        assert_eq!(
            check_params("SELECT 'a''b{{p}}c' AS s", &p),
            Err(ValidationError::PlaceholderInsideQuotedText("p".into()))
        );
        // A placeholder inside a quoted identifier is inside it too.
        assert_eq!(
            check_params(r#"SELECT "a{{p}}b" FROM t"#, &p),
            Err(ValidationError::PlaceholderInsideQuotedText("p".into()))
        );
        assert_eq!(
            check_params("SELECT `a{{p}}b` FROM t", &p),
            Err(ValidationError::PlaceholderInsideQuotedText("p".into()))
        );
        // Anti-triviality: the literal really does END, so a placeholder after it
        // is still a value position.
        assert!(check_params("SELECT 'a''b' AS s, {{p}} AS v", &p).is_ok());
    }

    /// **Neither escaping reading is safe alone, and this is why `check_params`
    /// runs twice.** Each payload is a genuine two-statement smuggle under exactly
    /// one dialect, and invisible to the scanner that assumes the other.
    ///
    /// **Read the reason these now fail, not just that they fail.** Since the
    /// SEV-0 fix they are refused by `reject_unscannable_bytes` on the raw `;`,
    /// BEFORE either escaping pass runs — so this test no longer proves the
    /// dual-reading property for `check_single_select`. It is kept as a
    /// regression corpus, and the property it used to prove is now pinned by
    /// [`both_escaping_readings_are_still_required_for_placeholders`], where
    /// there is no `;` for layer 1 to catch.
    #[test]
    fn a_backslash_smuggle_is_caught_by_the_other_reading() {
        assert_eq!(
            check_single_select(r"SELECT 'a\' ; DROP TABLE t -- '"),
            Err(ValidationError::MultipleStatements)
        );
        assert!(check_single_select(r"SELECT 'a\' ; DROP TABLE t").is_err());
        assert!(check_single_select(r"SELECT 'a\'b;c' AS s").is_err());
    }

    /// The dual-reading property, pinned where layer 1 cannot answer for it.
    ///
    /// Under the ESCAPING (StarRocks) reading, `'a\'` runs on and the placeholder
    /// is inside a literal. Under the NON-escaping (Trino) reading the literal
    /// closes at `\'` and the placeholder looks like a value position. The two
    /// disagree, and `check_params` must take the refusing answer — a placeholder
    /// substituted inside a live StarRocks literal is the serve-time break-out.
    ///
    /// Delete either entry of [`ESCAPING_READINGS`] and this test fails.
    #[test]
    fn both_escaping_readings_are_still_required_for_placeholders() {
        let p = [param("p", ParamType::String)];
        // Delete `true` from ESCAPING_READINGS and this becomes `Ok(())`: the
        // Trino reading closes the literal at `\'`, so the placeholder looks like
        // a value. The StarRocks reading keeps the literal open to end-of-input.
        assert_eq!(
            check_params(r"SELECT 'a\' AS s, {{p}} AS v", &p),
            Err(ValidationError::Unterminated("quoted literal")),
            "the StarRocks reading must keep the literal open across `\\'`"
        );
        // Delete `false` and THIS becomes `Ok(())`: under the StarRocks reading
        // `'a\'b'` is one closed literal and the placeholder is a value. Under the
        // Trino reading the quotes pair differently and the tail is unterminated.
        // (This is the disclosed `'a\'` false rejection — refusing is the safe
        // direction when the readings disagree.)
        assert_eq!(
            check_params(r"SELECT 'a\'b' AS s, {{p}} AS v", &p),
            Err(ValidationError::Unterminated("quoted literal")),
            "the Trino reading must not be dropped either"
        );
        // Anti-triviality: a placeholder genuinely inside a literal is still
        // reported as such, so the two assertions above are not just "everything
        // with a backslash errors".
        assert_eq!(
            check_params(r"SELECT 'a{{p}}b' AS s", &p),
            Err(ValidationError::PlaceholderInsideQuotedText("p".into()))
        );
    }

    /// **The SEV-0, as a unit test.** This is the exact definition that dropped a
    /// table on live StarRocks 4.1.3 at `POST …/validate`.
    ///
    /// It used to be ACCEPTED here: the scanner read `#` as a comment, stripped to
    /// end-of-line, and saw `SELECT 1 AS a`. StarRocks accepts and IGNORES `#`, so
    /// `#)` closed `Connector::describe`'s own wrapper, the `;` became a top-level
    /// separator, and the trailing `SELECT * FROM (SELECT 1` re-opened a subquery
    /// for the wrapper's suffix.
    ///
    /// **Mutation proof.** Delete the `#` arm's replacement — i.e. put
    /// `if sql.contains('#')` back to `Ok(())` — and the FIRST assertion below
    /// still passes, because the raw `;` catches it. Delete the `;` rule instead
    /// and the first assertion still passes, because `#` catches it. Only the
    /// per-byte assertions that follow fail on exactly one mutation each, which is
    /// why they are here rather than the payload alone.
    #[test]
    fn the_sev_0_payload_is_refused() {
        let payload = "SELECT 1 AS a #) AS _egret_ds LIMIT 0 ; DROP TABLE revtest.t ; SELECT * FROM (SELECT 1";
        assert!(check_single_select(payload).is_err(), "{payload}");

        // `#` alone, with no `;` anywhere: only the hash rule can refuse this.
        assert_eq!(
            check_single_select("SELECT 1 AS a # trailing note"),
            Err(ValidationError::HashNotAllowed)
        );
        assert_eq!(
            check_single_select("SELECT '#tag' AS t"),
            Err(ValidationError::HashNotAllowed),
            "refused even inside a literal — `char(35)` is the spelling that works"
        );
        // `;` alone, with no `#` anywhere: only the semicolon rule can refuse this.
        assert_eq!(
            check_single_select("SELECT 1 AS a ; DROP TABLE t"),
            Err(ValidationError::MultipleStatements)
        );

        // And `check_params` refuses both on its own, so neither `pub` entry
        // point depends on being called after the other.
        assert_eq!(
            check_params("SELECT 1 AS n # {{p}}", &[param("p", ParamType::String)]),
            Err(ValidationError::HashNotAllowed)
        );
        assert_eq!(
            check_params(
                "SELECT {{p}} ; DROP TABLE t",
                &[param("p", ParamType::String)]
            ),
            Err(ValidationError::MultipleStatements)
        );
    }

    /// A trailing `;` cannot separate anything, so it must NOT be refused —
    /// otherwise every SQL editor's habitual terminator becomes a 400 and the
    /// rule reads as a blanket ban.
    #[test]
    fn a_trailing_semicolon_is_still_accepted() {
        assert!(check_single_select("SELECT 1 AS a;").is_ok());
        assert!(check_single_select("SELECT 1 AS a;  \n ").is_ok());
        assert!(check_single_select("SELECT 1 AS a;;").is_ok());
    }

    /// The `--` terminator rules still matter for [`check_params`], which is the
    /// only consumer of the scan that layer 1 does not cover. `--` is a real
    /// comment on both engines: Trino `SIMPLE_COMMENT = '--' ~[\r\n]*`, and
    /// StarRocks 4.1.3 measured via `mysql -c` (`SELECT 1 AS a --x AND
    /// nonexistent_col` returns `a=1`, so no trailing whitespace is required and
    /// the rest of the line is genuinely commented out).
    ///
    /// A CR ends the comment, so a placeholder after one is back in a value
    /// position — that is the assertion a wrong terminator would break.
    #[test]
    fn a_carriage_return_ends_a_dash_comment() {
        let p = [param("p", ParamType::String)];
        assert_eq!(
            check_params("SELECT 1 AS n -- note\r{{p}} AS v", &p),
            Ok(()),
            "a CR ends the `--` comment, so the placeholder is a value again"
        );
        assert_eq!(
            check_params("SELECT 1 AS n -- note {{p}}", &p),
            Err(ValidationError::PlaceholderInsideQuotedText("p".into())),
            "and without the CR it is still inside the comment"
        );
        assert!(
            check_single_select("SELECT 1 --x\r\nFROM t").is_ok(),
            "the ordinary CRLF form still comments out its own line"
        );
    }

    #[test]
    fn an_unterminated_region_is_refused_rather_than_swallowed() {
        assert_eq!(
            check_single_select("SELECT 'oops"),
            Err(ValidationError::Unterminated("quoted literal"))
        );
        // No `;` in the payload, deliberately: with one, `reject_unscannable_bytes`
        // would answer first and this would stop testing the scanner at all.
        assert_eq!(
            check_single_select("SELECT 1 /* DROP TABLE t"),
            Err(ValidationError::Unterminated("block comment"))
        );
    }

    /// `--` needs NO trailing whitespace, on either engine. Measured on live
    /// StarRocks 4.1.3: `SELECT 1 AS a --x AND nonexistent_col` returns `a=1`, so
    /// the unresolvable column was commented out — StarRocks follows Presto's
    /// `SIMPLE_COMMENT`, not MySQL's "`--` must be followed by whitespace" rule.
    ///
    /// This lived in the gaps test for one round, described as a possible
    /// smuggle. It is not one: accepting `SELECT 1 --x; DROP TABLE t` is CORRECT,
    /// because the `;` really is inside the comment on both engines. Pinned here
    /// as right behaviour, since a disclosure that invents a gap is as wrong as
    /// one that hides a real gap.
    #[test]
    fn a_dash_comment_needs_no_trailing_whitespace() {
        assert!(check_single_select("SELECT 1 AS a --x AND nonexistent_col").is_ok());
        // `--x; DROP TABLE t` USED to be accepted here, correctly: the `;` really
        // is inside the comment on both engines. It is now refused by the raw-byte
        // rule, which does not ask whether a `;` is inert. Listed among the
        // deliberate false rejections in the module comment.
        assert_eq!(
            check_single_select("SELECT 1 --x; DROP TABLE t"),
            Err(ValidationError::MultipleStatements)
        );
    }

    /// Pins the gaps the module comment discloses, so the disclosure cannot go
    /// stale in either direction: if someone closes one of these, this test fails
    /// and points them at the list that now overstates the gaps. It asserts
    /// current behaviour, NOT that the behaviour is right.
    #[test]
    fn the_disclosed_gaps_are_still_gaps() {
        // `trim_end_matches` strips EVERY trailing semicolon, not exactly one.
        assert!(check_single_select("SELECT 1;;").is_ok());
        // The head-word check looks at one word.
        assert!(check_single_select("WITH t AS (SELECT 1) DELETE FROM u").is_ok());
        assert!(check_single_select("SELECT 1 INTO OUTFILE '/tmp/x'").is_ok());
        // `/*! … */` is scanned as an ordinary block comment, so its contents are
        // stripped rather than checked. Harmless on StarRocks 4.1.3, which was
        // measured NOT to execute executable comments (`/*!, 2 AS b */` and the
        // `/*!50000 …*/` and `/*!00000 …*/` forms all return one column) — kept
        // because that is version-dependent.
        // `/*! … */` is still scanned as an ordinary block comment, so its
        // contents are stripped rather than checked. The `; DROP TABLE t` form
        // this used to carry is now refused on the raw `;`, so the gap is shown
        // with a payload that has none — which is the honest shape of what
        // remains: contents unscanned, but unable to add a statement.
        assert!(check_single_select("SELECT 1 /*! , 2 AS b */").is_ok());
    }

    /// Non-ASCII must survive the byte-wise scan — a naive `b[i] as char` copy
    /// would corrupt it into replacement characters or panic on a boundary.
    #[test]
    fn multibyte_text_survives_stripping() {
        assert!(check_single_select("SELECT 'doanh thu' AS \"Doanh thu\" FROM t").is_ok());
        assert_eq!(
            strip_comments_and_strings("SELECT é /* ; */ FROM t", true)
                .unwrap()
                .trim(),
            "SELECT é   FROM t".trim()
        );
    }

    /// **The serve-time privilege boundary.** A placeholder inside a quoted
    /// region is substituted there as an ALREADY-QUOTED literal, which closes the
    /// enclosing one: `WHERE a = 'x{{p}}y'` with `p = "' OR 1=1 -- "` renders to
    /// `WHERE a = 'x''' OR 1=1 -- 'y'` — literal, then LIVE predicate, then
    /// comment. No second statement, so `check_single_select` never sees it, and
    /// `probe_sql` renders it to `'x''y'` which describes perfectly. The attacker
    /// is any Data API token holder, not the author.
    #[test]
    fn a_placeholder_inside_a_quoted_region_is_refused() {
        let p = [param("p", ParamType::String)];
        for sql in [
            "SELECT * FROM t WHERE a = 'x{{p}}y'",
            "SELECT * FROM t WHERE a LIKE '%{{p}}%'",
            r#"SELECT "col{{p}}" FROM t"#,
            "SELECT `col{{p}}` FROM t",
            // Comments too: a value carrying `*/` closes a block comment early.
            "SELECT 1 AS n /* {{p}} */",
            "SELECT 1 AS n -- {{p}}",
            // The two `#` forms that used to live here are gone: `#` is now
            // refused outright by `reject_unscannable_bytes`, so these inputs
            // never reach the placeholder rule and would fail with
            // `HashNotAllowed` instead. They are pinned in
            // `the_sev_0_payload_is_refused`.
        ] {
            assert_eq!(
                check_params(sql, &p),
                Err(ValidationError::PlaceholderInsideQuotedText("p".into())),
                "{sql}"
            );
        }
    }

    /// The correct spelling the rejection points at must still publish, or the
    /// rule above would simply ban parameterized `LIKE`.
    #[test]
    fn a_placeholder_in_a_value_position_is_still_accepted() {
        let p = [param("q", ParamType::String)];
        for sql in [
            "SELECT * FROM t WHERE a = {{q}}",
            "SELECT * FROM t WHERE a LIKE concat('%', {{q}}, '%')",
            "SELECT * FROM t WHERE a = {{ q }} AND b = 'literal'",
        ] {
            assert!(check_params(sql, &p).is_ok(), "{sql}");
        }
    }

    /// Counting, not set membership: one safe reference must not vouch for an
    /// unsafe one with the same name.
    #[test]
    fn one_good_reference_does_not_excuse_a_second_bad_one() {
        assert_eq!(
            check_params(
                "SELECT * FROM t WHERE a = {{q}} OR b = 'x{{q}}y'",
                &[param("q", ParamType::String)]
            ),
            Err(ValidationError::PlaceholderInsideQuotedText("q".into()))
        );
    }

    /// A placeholder that is outside a literal on the Trino reading but INSIDE
    /// one on the StarRocks reading must be refused — which only the two-reading
    /// check can see.
    #[test]
    fn a_placeholder_is_checked_under_both_escaping_readings() {
        // Non-escaping: `'a\'` closes, so `{{p}}` is outside. Escaping: the
        // literal continues through `{{p}}` to the final quote.
        assert!(check_params(
            r"SELECT * FROM t WHERE a = 'a\' || {{p}} || 'b'",
            &[param("p", ParamType::String)]
        )
        .is_err());
    }

    #[test]
    fn an_undeclared_placeholder_is_rejected() {
        assert_eq!(
            check_params("SELECT * FROM t WHERE r = {{region}}", &[]),
            Err(ValidationError::UndeclaredPlaceholder("region".into()))
        );
    }

    #[test]
    fn an_unreferenced_parameter_is_rejected() {
        assert_eq!(
            check_params("SELECT 1", &[param("region", ParamType::String)]),
            Err(ValidationError::UnreferencedParameter("region".into()))
        );
    }

    #[test]
    fn params_matching_in_both_directions_are_accepted() {
        assert!(check_params(
            "SELECT * FROM t WHERE r = {{ region }} AND n > {{n}}",
            &[
                param("region", ParamType::String),
                param("n", ParamType::Number)
            ],
        )
        .is_ok());
        assert!(check_params("SELECT 1", &[]).is_ok());
    }

    #[test]
    fn a_duplicate_declaration_is_rejected() {
        assert_eq!(
            check_params(
                "SELECT {{r}}",
                &[param("r", ParamType::String), param("r", ParamType::Number)]
            ),
            Err(ValidationError::DuplicateParameter("r".into()))
        );
    }

    /// Two malformed shapes, refused by the two different instruments that can
    /// see them — which is the T7-3 split in one test.
    ///
    /// `{{a b}}` is a template SYNTAX error and is now minijinja's to report:
    /// this scan no longer requires the inside of a `{{ … }}` to be a bare
    /// identifier, because requiring that is what made `{{ sort | ident }}` an
    /// undeclared placeholder called `"sort | ident"`. `{{a` never reaches a
    /// parser as a placeholder at all — an unterminated `{{` is template DATA to
    /// minijinja, so the byte scan keeps that one.
    #[test]
    fn a_malformed_placeholder_is_rejected() {
        assert!(
            matches!(
                check_params("SELECT {{a b}}", &[]),
                Err(ValidationError::Render(
                    crate::render::RenderError::Template(_)
                ))
            ),
            "a template syntax error is the parser's finding, not the scanner's"
        );
        assert!(matches!(
            check_params("SELECT {{a", &[]),
            Err(ValidationError::MalformedPlaceholder(_))
        ));
        assert!(matches!(
            check_params("SELECT {{}}", &[]),
            Err(ValidationError::MalformedPlaceholder(_))
        ));
    }

    /// **T7-3, as a test.** Every one of these is refused by P3's scanner and
    /// publishable under the reconciled one; the first is spec §4.3's own
    /// canonical example, which could not be published at all.
    #[test]
    fn a_template_expression_is_no_longer_read_as_a_parameter_name() {
        let sort = ParamSpec {
            name: "sort".into(),
            ty: ParamType::Enum,
            location: ParamLocation::Query,
            required: true,
            default: None,
            allowed: Some(vec!["region".into(), "total".into()]),
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            not_before: None,
            not_after: None,
            description: String::new(),
        };
        // `{{ sort | ident }}` — the whole inside used to become the NAME.
        assert!(
            check_params("SELECT 1 AS n ORDER BY {{ sort | ident }}", &[sort]).is_ok(),
            "`sort | ident` was reported as an undeclared parameter of that name"
        );
        // A loop variable — `r` used to be reported as undeclared, and the
        // iterable `regions` as unreferenced.
        assert!(check_params(
            "SELECT 1 AS n WHERE r IN ({% for r in regions %}{{ r }},{% endfor %}'')",
            &[param("regions", ParamType::String)]
        )
        .is_ok());
        // And the rules it must NOT have relaxed: a name nothing declares is
        // still undeclared, now including one only a tag reads.
        assert_eq!(
            check_params("SELECT 1 AS n {% if nope %}{% endif %}", &[]),
            Err(ValidationError::UndeclaredPlaceholder("nope".into())),
            "a name read only by a tag was invisible to the old scan"
        );
        assert_eq!(
            check_params("SELECT 1 AS n WHERE a = 'x{{ p | ident }}y'", &[param("p", ParamType::String)]),
            Err(ValidationError::PlaceholderInsideQuotedText(
                "p | ident".into()
            )),
            "the quoted-text rule keys on the OCCURRENCE, so it survives an expression it cannot parse"
        );
    }

    /// All four corners of the pair, not just the `delegated` axis.
    ///
    /// The `(false, true)` case is the one that mattered: a connection with
    /// `delegated: false` and `options.impersonate: true` runs every query as the
    /// caller, and the earlier one-field check published it as shared-safe.
    #[test]
    fn shared_cache_is_refused_on_either_half_of_the_pair() {
        for (delegated, impersonate) in [(true, false), (false, true), (true, true)] {
            assert_eq!(
                check_cache_scope(&cache(CacheScope::Shared), delegated, impersonate),
                Err(ValidationError::SharedCacheOnPerPrincipalConnection),
                "shared must be refused for (delegated={delegated}, impersonate={impersonate})"
            );
        }
        // The one safe corner.
        assert!(check_cache_scope(&cache(CacheScope::Shared), false, false).is_ok());
        // …and `per_principal` is fine everywhere, which is what makes the
        // refusals above about the SCOPE rather than about the connection.
        for (delegated, impersonate) in [(true, false), (false, true), (true, true), (false, false)]
        {
            assert!(
                check_cache_scope(&cache(CacheScope::PerPrincipal), delegated, impersonate).is_ok()
            );
        }
    }

    /// The predicate is OR, and it is asserted directly so the corner above
    /// cannot be "fixed" by weakening it to AND somewhere else.
    #[test]
    fn a_connection_runs_per_principal_if_either_field_says_so() {
        assert!(!runs_per_principal(false, false));
        assert!(runs_per_principal(true, false));
        assert!(
            runs_per_principal(false, true),
            "impersonate alone is enough"
        );
        assert!(runs_per_principal(true, true));
    }

    /// The JSONB key is a string coupling to `egret_query_api::ConnOptions`, so it
    /// is pinned in one place rather than trusted at three call sites.
    #[test]
    fn the_impersonate_flag_reads_only_a_real_json_true() {
        assert!(impersonate_flag(&serde_json::json!({"impersonate": true})));
        assert!(!impersonate_flag(
            &serde_json::json!({"impersonate": false})
        ));
        assert!(!impersonate_flag(&serde_json::json!({})));
        assert!(!impersonate_flag(&serde_json::json!({"impersonate": null})));
        // A non-bool must not read as true. It also must not read as true at the
        // OTHER end — the frozen and the live value go through this same
        // function, so an unparseable value is "not impersonating" on both sides
        // and the pair still agrees rather than 503ing forever.
        assert!(!impersonate_flag(
            &serde_json::json!({"impersonate": "true"})
        ));
        assert!(!impersonate_flag(&serde_json::json!({"impersonate": 1})));
        // Neighbouring keys must not be mistaken for it.
        assert!(!impersonate_flag(
            &serde_json::json!({"impersonated": true})
        ));
    }

    /// A disabled shared cache is still refused — the flag is one edit away from
    /// enabled, and that edit does not re-run validation.
    #[test]
    fn a_disabled_shared_cache_is_still_refused_when_per_principal() {
        let c = CacheSpec {
            enabled: false,
            ttl_seconds: 60,
            scope: CacheScope::Shared,
        };
        assert!(check_cache_scope(&c, true, false).is_err());
        assert!(
            check_cache_scope(&c, false, true).is_err(),
            "the impersonate half must carry the disabled-cache rule too"
        );
    }

    #[test]
    fn an_enum_without_an_allowed_list_is_rejected() {
        let mut p = param("status", ParamType::Enum);
        assert_eq!(
            check_enum_params(std::slice::from_ref(&p)),
            Err(ValidationError::EnumWithoutAllowed("status".into()))
        );
        p.allowed = Some(vec![]);
        assert_eq!(
            check_enum_params(std::slice::from_ref(&p)),
            Err(ValidationError::EnumWithoutAllowed("status".into())),
            "an empty list is no list"
        );
        p.allowed = Some(vec!["open".into()]);
        assert!(check_enum_params(std::slice::from_ref(&p)).is_ok());
        // A non-enum needs none.
        assert!(check_enum_params(&[param("region", ParamType::String)]).is_ok());
    }

    #[test]
    fn a_constraint_on_the_wrong_type_is_refused() {
        let mut p = param("flag", ParamType::Boolean);
        p.pattern = Some("^x$".into());
        let err = check_constraints(std::slice::from_ref(&p)).expect_err("pattern on a boolean");
        let msg = err.to_string();
        assert!(msg.contains("pattern"), "must name the constraint: {msg}");
        assert!(msg.contains("flag"), "must name the parameter: {msg}");
    }

    #[test]
    fn inverted_bounds_are_refused() {
        let mut p = param("n", ParamType::Number);
        p.minimum = Some("100".into());
        p.maximum = Some("10".into());
        let err = check_constraints(std::slice::from_ref(&p)).expect_err("minimum above maximum");
        assert!(err.to_string().contains("minimum"), "{err}");

        let mut q = param("s", ParamType::String);
        q.min_length = Some(10);
        q.max_length = Some(2);
        assert!(
            check_constraints(std::slice::from_ref(&q)).is_err(),
            "min_length above max_length must be refused too"
        );
    }

    #[test]
    fn equal_bounds_are_allowed() {
        // A parameter pinned to exactly one value is a real thing an author writes;
        // "inverted" must mean strictly greater, not "not less".
        let mut p = param("n", ParamType::Number);
        p.minimum = Some("5".into());
        p.maximum = Some("5".into());
        assert!(check_constraints(std::slice::from_ref(&p)).is_ok());
    }

    /// The numeric twin of `unpadded_month_bounds_compare_chronologically_not_lexically`:
    /// `"9"` sorts AFTER `"10"` as bytes (`'9' > '1'`) despite being the
    /// smaller number. If `check_constraints` ever regressed to comparing
    /// `minimum`/`maximum` as raw text instead of through
    /// `compare_decimal_text`, this legitimate pair would be wrongly refused
    /// as inverted.
    #[test]
    fn numeric_bounds_compare_decimal_not_lexical() {
        let mut p = param("n", ParamType::Number);
        p.minimum = Some("9".into());
        p.maximum = Some("10".into());
        assert!(
            check_constraints(std::slice::from_ref(&p)).is_ok(),
            "9 before 10 must not be reported as inverted bounds"
        );
    }

    /// The numeric twin of `an_unparseable_temporal_bound_is_refused_by_name_not_compared_as_bytes`.
    /// Nothing validated `minimum`/`maximum` against `is_plain_decimal` before
    /// this fix, so `minimum: "abc"` published clean and was compared as
    /// arbitrary bytes at serve time forever after — this is the same class
    /// of gap the temporal bound closes, just on the other pair.
    #[test]
    fn an_unparseable_numeric_bound_is_refused_by_name_not_compared_as_bytes() {
        let mut p = param("n", ParamType::Number);
        p.minimum = Some("abc".into());
        let err = check_constraints(std::slice::from_ref(&p)).expect_err("minimum is not a number");
        let msg = err.to_string();
        assert!(msg.contains("`minimum`"), "must name the constraint: {msg}");
        assert!(msg.contains("`n`"), "must name the parameter: {msg}");

        // The same bound, unparseable, on the OTHER side of the pair — this
        // must not be silently compared against a valid `minimum` either.
        let mut q = param("n", ParamType::Number);
        q.minimum = Some("1".into());
        q.maximum = Some("zzz".into());
        assert!(check_constraints(std::slice::from_ref(&q)).is_err());
    }

    /// The cap counts the digits the DOCUMENT has to hold, not the bytes the
    /// author typed: `compare_decimal_text` already ignores leading zeros and
    /// `json_number` strips them, so a padded `"0…01"` is one digit wide
    /// however long the string is. Counting bytes instead would refuse a bound
    /// that publishes perfectly.
    #[test]
    fn the_digit_cap_counts_significant_digits_not_bytes() {
        let mut p = param("n", ParamType::Number);
        p.minimum = Some(format!("{}1", "0".repeat(40)));
        assert!(
            check_constraints(std::slice::from_ref(&p)).is_ok(),
            "a zero-padded `1` is one digit wide, not 41"
        );

        // And the message says how wide the bound actually was, so an author
        // who has to shorten one knows by how much.
        let mut q = param("n", ParamType::Number);
        q.maximum = Some("1".repeat(MAX_BOUND_DIGITS + 1));
        let err = check_constraints(std::slice::from_ref(&q)).expect_err("one digit past the cap");
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("{} digits", MAX_BOUND_DIGITS + 1)),
            "must say how wide the bound is: {msg}"
        );
        assert!(msg.contains("`maximum`"), "must name the constraint: {msg}");
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_the_authors_400_not_the_callers_500() {
        let mut p = param("s", ParamType::String);
        p.pattern = Some("([".into());
        let err = check_constraints(std::slice::from_ref(&p)).expect_err("unclosed class");
        let msg = err.to_string();
        // "valid regex" is unique to `PatternInvalid` — `PatternTooLarge` says
        // "too large to compile" instead — so this also proves the RIGHT
        // variant fired, not merely that something did.
        assert!(msg.contains("valid regex"), "wrong refusal reason: {msg}");
        assert!(msg.contains("`s`"), "must name the parameter: {msg}");
    }

    #[test]
    fn a_pattern_that_compiles_too_large_is_refused() {
        // `regex`'s own default cap is 10 MB of compiled program, and
        // `a{1000}{1000}` is too big even at THAT — measured with
        // `RegexBuilder::size_limit(10 * 1024 * 1024)` directly: it fails
        // before our own cap is ever consulted. That made the original
        // version of this test a false pass: it would still hold with
        // `.size_limit(MAX_PATTERN_PROGRAM_BYTES)` deleted from
        // `compile_pattern`, and would have verified nothing about the 64 KiB
        // cap. `a{5000}` is the discriminator: measured `default_ok=true,
        // small_ok=false` — it compiles fine under `regex`'s own 10 MB
        // default and only fails under our smaller one.
        let mut p = param("s", ParamType::String);
        p.pattern = Some("a{5000}".into());
        assert!(
            regex::RegexBuilder::new(&format!("^(?:{})$", p.pattern.as_ref().unwrap()))
                .size_limit(10 * 1024 * 1024)
                .build()
                .is_ok(),
            "sanity: this pattern must compile under regex's own default cap, \
             or it no longer discriminates our 64 KiB cap from that one"
        );
        match check_constraints(std::slice::from_ref(&p)) {
            Err(ValidationError::PatternTooLarge { param }) => assert_eq!(param, "s"),
            other => panic!("expected PatternTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn check_constraints_runs_before_check_defaults() {
        // The author's default is FINE. Their bounds are inverted. The error they
        // read must be about the bounds, or they will go looking in the wrong place.
        let mut p = param("n", ParamType::Number);
        p.minimum = Some("100".into());
        p.maximum = Some("10".into());
        p.default = Some("50".into());
        // `CAST` because a bare `{{ n }}` in the select list fails
        // `UncastParameterInProjection` regardless of `check_constraints` —
        // that unrelated failure would mask a missing ordering check just as
        // effectively as deleting the line would, so this SQL must publish
        // cleanly on its own before the inverted bounds are added.
        let err = validate_definition(
            "SELECT CAST({{ n }} AS INTEGER) AS out",
            std::slice::from_ref(&p),
            &cache(CacheScope::PerPrincipal),
            false,
            false,
        )
        .expect_err("inverted bounds");
        let msg = err.to_string();
        assert!(
            !msg.contains("default"),
            "the bounds are the problem, not the default: {msg}"
        );
    }

    /// The detail Task 3 put on [`crate::serving::params::CoerceError::Constraint`]
    /// is discarded by the serving path on purpose — `check_defaults` is the
    /// ONE place it is allowed to reach an author, because the author is
    /// reading about their own endpoint and there is no oracle to protect.
    #[test]
    fn a_default_below_its_own_minimum_names_the_constraint_and_the_bound() {
        let mut p = param("n", ParamType::Number);
        p.minimum = Some("10".into());
        p.default = Some("5".into());
        let err = check_defaults(std::slice::from_ref(&p)).expect_err("default below minimum");
        let msg = err.to_string();
        assert!(msg.contains('n'), "must name the parameter: {msg}");
        assert!(msg.contains("minimum"), "must name the constraint: {msg}");
        assert!(msg.contains("10"), "must name the bound: {msg}");
        assert!(
            !msg.contains("declared type"),
            "a constraint failure must not be reported as a TYPE failure: {msg}"
        );
        assert_eq!(
            err,
            ValidationError::DefaultViolatesConstraint {
                param: "n".into(),
                constraint: "minimum",
                bound: "10".into(),
            },
            "the variant itself must discriminate, not just its message: {msg}"
        );
    }

    /// The pre-C1 message must survive: `default: "yesterday"` on a `Date` is a
    /// TYPE problem, not a constraint problem, and conflating them sends the
    /// author to the wrong field. This is the test most likely to pass for the
    /// wrong reason, so it pins the exact variant rather than a substring —
    /// `DefaultNotValidForType`'s own message never contains the word "type"
    /// (it says "is not a valid `date`"), so a naive `contains("type")` check
    /// fails against the correct, unchanged message just as readily as it
    /// would pass vacuously against a wrong one; the variant is what actually
    /// discriminates, and the message assertion below adds the same
    /// discrimination in words instead of matching one that means nothing on
    /// its own.
    #[test]
    fn a_default_of_the_wrong_type_still_reports_a_type_failure() {
        let mut p = param("d", ParamType::Date);
        p.default = Some("yesterday".into());
        let err = check_defaults(std::slice::from_ref(&p)).expect_err("not a date");
        assert_eq!(
            err,
            ValidationError::DefaultNotValidForType("d".into(), "date"),
            "{err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("not a valid"), "{msg}");
        assert!(
            !msg.contains("violates its own"),
            "a type failure must not be reported as a CONSTRAINT failure: {msg}"
        );
    }

    /// The unsoundness `ConstraintBoundUnparseable` exists to close: comparing
    /// the author's RAW `not_before`/`not_after` text as bytes gets an unpadded
    /// month backwards. September (`"2026-9-1"`) sorts AFTER October
    /// (`"2026-10-1"`) as text — `'9' > '1'` — despite being chronologically
    /// earlier. If `check_constraints` compared raw text, this legitimate,
    /// correctly-ordered pair would be refused as "inverted".
    #[test]
    fn unpadded_month_bounds_compare_chronologically_not_lexically() {
        let mut p = param("d", ParamType::Date);
        p.not_before = Some("2026-9-1".into());
        p.not_after = Some("2026-10-1".into());
        assert!(
            check_constraints(std::slice::from_ref(&p)).is_ok(),
            "September before October must not be reported as inverted bounds"
        );
    }

    #[test]
    fn an_unparseable_temporal_bound_is_refused_by_name_not_compared_as_bytes() {
        let mut p = param("d", ParamType::Date);
        p.not_before = Some("not-a-date".into());
        let err =
            check_constraints(std::slice::from_ref(&p)).expect_err("not_before is not a date");
        let msg = err.to_string();
        assert!(
            msg.contains("`not_before`"),
            "must name the constraint: {msg}"
        );
        assert!(msg.contains("`d`"), "must name the parameter: {msg}");

        // The same bound, unparseable, on the OTHER side of the pair — this
        // must not be silently compared against a valid `not_before` either.
        let mut q = param("d", ParamType::Date);
        q.not_before = Some("2020-01-01".into());
        q.not_after = Some("garbage".into());
        assert!(check_constraints(std::slice::from_ref(&q)).is_err());
    }

    /// The render is what lets a PARAMETERIZED endpoint be described at all: an
    /// engine cannot plan `WHERE r = {{region}}`.
    ///
    /// This replaces the typed-literal probe it was written for. That probe
    /// substituted only `{{ … }}` and left every `{% … %}` tag standing, so it
    /// could not describe a conditional endpoint at all — it handed the engine a
    /// string with `{% if r %}` in it. The renderer produces the statement that
    /// will actually be served, `?` markers included, and the representative
    /// values decide which branches it contains.
    #[test]
    fn a_template_is_rendered_to_the_statement_that_will_be_served() {
        let params = [
            param("r", ParamType::String),
            param("n", ParamType::Number),
            param("d", ParamType::Date),
        ];
        let sql =
            "SELECT * FROM t WHERE r = {{r}} {% if n %}AND n > {{ n }}{% endif %} AND d < {{d}}";
        let iterated = IteratedParams::across(&[sql], &params);
        let (rendered, binds) =
            crate::render::render(sql, &representative(&iterated, &params, &[])).unwrap();
        assert_eq!(
            rendered, "SELECT * FROM t WHERE r = ? AND n > ? AND d < ?",
            "every branch a representative value reaches must be in the described statement"
        );
        assert_eq!(binds.len(), 3, "one bind per marker");
        // No parameters is identity.
        let (plain, none) = crate::render::render(
            "SELECT 1",
            &representative(&IteratedParams::across(&["SELECT 1"], &[]), &[], &[]),
        )
        .unwrap();
        assert_eq!(plain, "SELECT 1");
        assert!(none.is_empty());
    }

    #[test]
    fn validate_definition_runs_the_statement_check_first() {
        // Both rules are broken. The statement check must be the one that
        // answers, so a multi-statement string is never handed on to anything.
        let err = validate_definition(
            "SELECT 1; DROP TABLE t",
            &[param("unused", ParamType::String)],
            &cache(CacheScope::PerPrincipal),
            false,
            false,
        )
        .unwrap_err();
        assert_eq!(err, ValidationError::MultipleStatements);
    }

    /// Preview and publish must refuse the SAME definitions, minus the two rules
    /// that are about things a preview does not do (the cache scope, and the
    /// unreferenced-parameter rule that protects a PUBLISHED endpoint's OpenAPI —
    /// each with its own test below). If any other rule is ever added to
    /// `validate_definition` and not to `validate_preview`, this is what says so:
    /// preview would run a definition publish refuses.
    #[test]
    fn validate_preview_refuses_what_publish_refuses() {
        let cache = cache(CacheScope::PerPrincipal);
        // Not a single SELECT — the rule that stops `:manage` becoming engine
        // write access.
        assert!(preview("SELECT 1; DROP TABLE t", &[]).is_err());
        assert!(validate_definition("SELECT 1; DROP TABLE t", &[], &cache, false, false).is_err());

        // A single statement that is not a SELECT, which is the half of
        // `check_single_select` NOTHING else re-runs. The separator case above is
        // covered twice over — `check_params` calls `reject_unscannable_bytes`
        // again on purpose (see its comment), so it alone cannot tell whether
        // `check_single_select` is still in the bundle. A bare DML statement can:
        // it carries no `;` and no `#`, and the render rules do not care what the
        // head word is.
        assert!(preview("DELETE FROM t WHERE 1 = 1", &[]).is_err());
        assert!(
            validate_definition("DELETE FROM t WHERE 1 = 1", &[], &cache, false, false).is_err()
        );

        // An enum with no `allowed` list.
        let bad_enum = vec![ParamSpec {
            name: "sort".into(),
            ty: ParamType::Enum,
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
        }];
        assert!(preview("SELECT 1", &bad_enum).is_err());

        // A `location: body` parameter.
        let body_param = vec![ParamSpec {
            name: "region".into(),
            ty: ParamType::String,
            location: ParamLocation::Body,
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
        }];
        assert!(preview("SELECT 1", &body_param).is_err());

        // More parameters than the bound. Preview renders `optional + 2`
        // contexts exactly like publish does, so an unbounded preview is the
        // same unbounded CPU behind the same `:manage` permission — and preview
        // is the one an author can call in a loop without publishing anything.
        let too_many: Vec<ParamSpec> = (0..=MAX_DECLARED_PARAMS)
            .map(|i| param(&format!("p{i}"), ParamType::String))
            .collect();
        let wide: String = std::iter::once("SELECT id FROM t WHERE 1=1".to_owned())
            .chain(
                (0..=MAX_DECLARED_PARAMS)
                    .map(|i| format!(" {{% if p{i} %}} AND a{i} = {{{{ p{i} }}}}{{% endif %}}")),
            )
            .collect();
        assert_eq!(
            preview(&wide, &too_many),
            Err(ValidationError::TooManyParameters {
                declared: MAX_DECLARED_PARAMS + 1,
                limit: MAX_DECLARED_PARAMS,
            }),
            "the parameter bound must hold on the PREVIEW path too, not merely \
             because both paths happen to share `check_definition_scans` today"
        );
        assert_eq!(
            validate_definition(&wide, &too_many, &cache, false, false),
            Err(ValidationError::TooManyParameters {
                declared: MAX_DECLARED_PARAMS + 1,
                limit: MAX_DECLARED_PARAMS,
            })
        );

        // And a definition both accept.
        assert!(preview("SELECT 1 AS n", &[]).is_ok());
    }

    /// The second rule preview omits, and the browser finding that put it here.
    ///
    /// A stored draft declared `region` and `d`; the author replaced the SQL and
    /// clicked *Preview unsaved draft*, and preview refused with "parameter
    /// `region` is declared but never referenced". That intermediate state —
    /// SQL edited before the declarations catch up, or the reverse — is the
    /// commonest state of the loop this route exists to close, so preview could
    /// not run the one draft it was built for. The rule is about the OpenAPI
    /// contract a PUBLISHED endpoint offers, and publish keeps refusing it.
    #[test]
    fn validate_preview_accepts_sql_that_references_no_declared_parameter() {
        let declared = vec![
            param("region", ParamType::String),
            param("d", ParamType::String),
        ];
        // Nothing in the statement reads either name — the draft mid-edit.
        let sql = "SELECT 1 AS n";

        assert!(
            preview(sql, &declared).is_ok(),
            "preview must run the SQL on screen, declarations mid-edit and all"
        );
        // …and the published contract is unchanged: an endpoint may not offer a
        // query parameter that changes nothing.
        assert_eq!(
            validate_definition(
                sql,
                &declared,
                &cache(CacheScope::PerPrincipal),
                false,
                false
            )
            .unwrap_err(),
            ValidationError::UnreferencedParameter("region".into())
        );
        // The same pair through the parameter scan directly, so the boundary is
        // pinned at the rule and not only at the two wrappers.
        assert!(check_params_with(sql, &declared, ParamRules::Draft).is_ok());
        assert_eq!(
            check_params_with(sql, &declared, ParamRules::Published).unwrap_err(),
            ValidationError::UnreferencedParameter("region".into())
        );
    }

    /// Dropping ONE rule must not become dropping the scan. Each of the four
    /// rules [`check_params`] carries that is about the STATEMENT rather than
    /// about the published contract, refused through `validate_preview`.
    ///
    /// Without this, `validate_preview` could skip `check_params` altogether and
    /// every other test here would still pass — the test above only says the
    /// unreferenced rule is gone.
    #[test]
    fn validate_preview_keeps_every_parameter_rule_about_the_statement() {
        // 1. The SQL reads a name that is not declared. At serve time this is a
        //    render failure or a bind of the wrong thing.
        assert_eq!(
            preview("SELECT * FROM t WHERE r = {{ nope }}", &[]).unwrap_err(),
            ValidationError::UndeclaredPlaceholder("nope".into())
        );
        // Still refused when OTHER parameters are declared, which is the shape
        // the loop above walks.
        assert_eq!(
            preview(
                "SELECT * FROM t WHERE r = {{ nope }}",
                &[param("region", ParamType::String)]
            )
            .unwrap_err(),
            ValidationError::UndeclaredPlaceholder("nope".into())
        );
        // 2. A placeholder inside quoted text — the injection-adjacent rule.
        assert_eq!(
            preview(
                "SELECT * FROM t WHERE a = 'x{{p}}y'",
                &[param("p", ParamType::String)]
            )
            .unwrap_err(),
            ValidationError::PlaceholderInsideQuotedText("p".into())
        );
        // 3. A duplicate declaration, which makes binding ambiguous.
        assert_eq!(
            preview(
                "SELECT * FROM t WHERE r = {{ region }}",
                &[
                    param("region", ParamType::String),
                    param("region", ParamType::Number),
                ]
            )
            .unwrap_err(),
            ValidationError::DuplicateParameter("region".into())
        );
        // 4. `reject_unscannable_bytes`: a `#` comment, which the two dialects
        //    read differently, so the scanner refuses to guess.
        assert!(preview("SELECT 1 AS n # note", &[]).is_err());
    }

    /// The ONE rule preview deliberately omits, and the reason it is safe to
    /// omit: preview neither reads nor writes the cache, so a `shared` scope on
    /// a delegated connection cannot leak anything through it.
    #[test]
    fn validate_preview_omits_only_the_cache_scope_rule() {
        let shared = cache(CacheScope::Shared);
        // Publish refuses it on a delegated connection…
        assert!(validate_definition("SELECT 1 AS n", &[], &shared, true, false).is_err());
        // …and preview never sees a cache at all, so the same SQL is fine.
        assert!(preview("SELECT 1 AS n", &[]).is_ok());
    }

    /// Every arm is a 400 the author can act on — never a 500.
    #[test]
    fn every_validation_failure_is_invalid_input() {
        for e in [
            ValidationError::MultipleStatements,
            ValidationError::NotASelect("INSERT".into()),
            ValidationError::MalformedPlaceholder("{{a b}}".into()),
            ValidationError::Unterminated("quoted literal"),
            ValidationError::PlaceholderInsideQuotedText("p".into()),
            ValidationError::UndeclaredPlaceholder("r".into()),
            ValidationError::UnreferencedParameter("r".into()),
            ValidationError::DuplicateParameter("r".into()),
            ValidationError::SharedCacheOnPerPrincipalConnection,
            ValidationError::EnumWithoutAllowed("s".into()),
        ] {
            let msg = e.to_string();
            assert!(
                matches!(Error::from(e), Error::InvalidInput(_)),
                "{msg} must be a 400"
            );
        }
    }

    /// Keeps `LimitsSpec`/`RateLimitSpec` in the module's import graph: the
    /// snapshot freezes them, so a change to either shape has to compile here.
    #[test]
    fn the_snapshot_freezes_the_limit_specs() {
        let l = LimitsSpec {
            max_rows: 1,
            timeout_seconds: 1,
        };
        let r = RateLimitSpec {
            per_key_qps: 1,
            burst: 1,
            per_endpoint_qps: 1,
        };
        assert_eq!(l.max_rows, 1);
        assert_eq!(r.burst, 1);
    }

    // -----------------------------------------------------------------
    // check_transform (C2 Task 4) — publish refuses a transform it cannot
    // honour
    // -----------------------------------------------------------------

    /// Build an [`OutputSpec`] carrying `script` as its transform, with one
    /// declared `total: bigint` return column.
    ///
    /// `OutputSpec` does not derive `Default` (see its own doc comment, and
    /// `check_transform`'s), so this starts from `OutputSpec::default()` —
    /// the same construction every neighbouring test in this module already
    /// uses (`the_statement_bound_is_checked_before_anything_renders` and
    /// its siblings) — rather than naming every field by hand.
    fn transformed_output(script: &str) -> OutputSpec {
        OutputSpec {
            transform: Some(TransformSpec {
                script: script.into(),
                returns: vec![ColumnSpec {
                    name: "total".into(),
                    ty: "bigint".into(),
                    required: true,
                }],
            }),
            ..OutputSpec::default()
        }
    }

    #[test]
    fn a_transform_alongside_shape_or_naming_is_refused() {
        // Once a script decides the shape, `shape` and `naming` are dead or
        // ambiguous — does `naming: camel` apply before the script sees the
        // rows, or to the keys the script wrote? Refuse rather than pick.
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        o.shape = "arrays".into();
        let err = check_transform(&o).expect_err("arrays plus a transform");
        assert!(err.to_string().contains("shape"), "{err}");

        let mut p = transformed_output("def transform(params, statements):\n  return {}");
        p.naming = "camel".into();
        assert!(
            check_transform(&p).is_err(),
            "naming plus a transform must be refused too"
        );
    }

    #[test]
    fn a_transform_with_objects_and_as_is_is_accepted() {
        // The defaults must not be read as "the author asked for a shape".
        let o = transformed_output("def transform(params, statements):\n  return {}");
        assert_eq!(o.shape, "objects");
        assert_eq!(o.naming, "as_is");
        assert!(check_transform(&o).is_ok());
    }

    #[test]
    fn an_empty_returns_is_refused() {
        // A transform with no declared output has no contract at all, and
        // serving would have nothing to check against — it would fail OPEN.
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        o.transform.as_mut().unwrap().returns.clear();
        assert!(check_transform(&o).is_err());
    }

    #[test]
    fn two_returns_with_one_name_are_refused() {
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        let r = &mut o.transform.as_mut().unwrap().returns;
        r.push(ColumnSpec {
            name: "total".into(),
            ty: "bigint".into(),
            required: true,
        });
        r.push(ColumnSpec {
            name: "total".into(),
            ty: "varchar".into(),
            required: true,
        });
        let err = check_transform(&o).expect_err("duplicate name");
        assert!(err.to_string().contains("total"), "{err}");
    }

    #[test]
    fn a_script_that_does_not_parse_is_the_authors_400_not_the_callers_500() {
        let o = transformed_output("def transform(params, statements)\n  return {}");
        assert!(matches!(
            check_transform(&o),
            Err(ValidationError::TransformScriptInvalid(_))
        ));
    }

    #[test]
    fn a_script_longer_than_the_cap_is_refused_with_both_numbers() {
        let long = format!(
            "def transform(params, statements):\n  # {}\n  return {{}}",
            "x".repeat(MAX_TRANSFORM_SCRIPT_BYTES)
        );
        match check_transform(&transformed_output(&long)) {
            Err(ValidationError::TransformScriptTooLong { bytes, cap }) => {
                assert!(
                    bytes > cap,
                    "the error must carry both numbers: {bytes} vs {cap}"
                )
            }
            other => panic!("expected TransformScriptTooLong, got {other:?}"),
        }
    }

    /// **Proves `check_transform` is reachable from the real publish path,
    /// not only callable in isolation.**
    ///
    /// `validate_endpoint` is the private function both `POST …/validate` and
    /// `EndpointStore::publish` go through — see its own doc comment. Calling
    /// it directly here, with one valid statement and a transform declared
    /// alongside `output.shape: arrays`, is that path minus the database and
    /// the engine: everything before the first render, which is exactly
    /// where `check_transform` lives.
    ///
    /// A unit test on `check_transform` alone cannot tell whether anything
    /// still calls it — this is the test the Task 4 plan's Step 5 asks for.
    /// Commenting out `check_transform`'s call site inside
    /// `check_definition_scans` and re-running this test is what that step
    /// is: it must turn the endpoint below GREEN (published) for the missing
    /// call to be caught, rather than staying red for an unrelated reason.
    #[test]
    fn an_endpoint_with_a_transform_and_arrays_shape_is_refused_end_to_end() {
        let statements = vec![StatementSpec {
            name: "rows".into(),
            sql: "SELECT id FROM t".into(),
        }];
        let mut output = transformed_output("def transform(params, statements):\n  return {}");
        output.shape = "arrays".into();

        let got = validate_endpoint(
            &statements,
            &[],
            &cache(CacheScope::PerPrincipal),
            &output,
            false,
            false,
        );
        assert_eq!(
            got,
            Err(ValidationError::TransformWithShapeOrNaming("shape")),
            "a transform alongside output.shape: arrays must be refused at the function \
             both /validate and publish go through"
        );
    }

    /// **Preview and publish refuse a transform with the IDENTICAL error**
    /// (C2 Task 8).
    ///
    /// `check_transform` had exactly one production call site
    /// (`validate_endpoint`), which meant every rule above was invisible until
    /// publish: no write route checks a transform, so a script that does not
    /// parse could be saved, previewed GREEN, and refused only when the author
    /// pressed Publish. Told "fine", then told "no" — the defect this seam has
    /// produced twice.
    ///
    /// `assert_eq!` on the two `Result`s against EACH OTHER, not two separate
    /// `is_err()` calls: a preview that grew a second, weaker reading of the
    /// same rule would satisfy "both refuse" while disagreeing about which
    /// refusal, which is how the next disagreement gets in. Three rules rather
    /// than one, because they are refused by three different `if` blocks and a
    /// call site that reached only some of them is the plausible mistake.
    ///
    /// The `is_ok()` pair at the end is what stops the whole test passing on a
    /// preview that simply refuses every transform.
    #[test]
    fn preview_and_publish_refuse_a_transform_with_the_identical_error() {
        const SQL: &str = "SELECT id FROM t";
        let statements = vec![StatementSpec {
            name: "rows".into(),
            sql: SQL.into(),
        }];
        let publish = |output: &OutputSpec| {
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                output,
                false,
                false,
            )
            .map(|_| ())
        };

        // Rule 5: a script that does not parse.
        let unparseable = transformed_output("def transform(params, statements:\n");
        // Rule 2: a transform beside a non-default `naming`.
        let mut renamed = transformed_output("def transform(params, statements):\n  return {}");
        renamed.naming = "camel".into();
        // Rule 3: nothing declared for serving to check against.
        let mut nothing_declared =
            transformed_output("def transform(params, statements):\n  return {}");
        nothing_declared.transform.as_mut().unwrap().returns.clear();

        for output in [&unparseable, &renamed, &nothing_declared] {
            let previewed = preview_with(SQL, &[], output).map(|_| ());
            assert!(previewed.is_err(), "preview must refuse {output:?}");
            assert_eq!(
                previewed,
                publish(output),
                "preview and publish must reach the same refusal, from the same rule"
            );
        }

        // And the one both accept, so none of the above passes by refusing
        // transforms wholesale.
        let good =
            transformed_output("def transform(params, statements):\n  return {\"total\": 1}");
        assert!(preview_with(SQL, &[], &good).is_ok());
        assert!(publish(&good).is_ok());
    }

    /// **Pins the refusal ORDER `check_transform` documents.** Each rule is
    /// mutation-proven individually by the tests above, but none of them says
    /// anything about ORDER: swapping two `if` blocks inside `check_transform`
    /// would still make every one of those tests pass, because each feeds a
    /// definition that violates exactly one rule. Order is load-bearing for
    /// the same reason C1's was — an author whose script breaks two rules at
    /// once should be told the one they can act on FIRST, per the function's
    /// own doc comment — so this feeds definitions that violate TWO rules at
    /// once and asserts which refusal comes back, with `assert_eq!` on the
    /// exact payload rather than `matches!` with a wildcard, so a swap is
    /// caught by a value mismatch rather than merely an `is_err()`.
    #[test]
    fn the_documented_refusal_order_holds_when_multiple_rules_are_violated() {
        // Rule 1 (script length) vs rule 5 (parse), the two bookends of the
        // documented order. This script is BOTH over the cap AND unparseable
        // (missing the `:` after the `def` header) — length must still win.
        let unparseable_and_long = format!(
            "def transform(params, statements)
  # {}
  return {{}}",
            "x".repeat(MAX_TRANSFORM_SCRIPT_BYTES)
        );
        let bytes = unparseable_and_long.len();
        assert_eq!(
            check_transform(&transformed_output(&unparseable_and_long)),
            Err(ValidationError::TransformScriptTooLong {
                bytes,
                cap: MAX_TRANSFORM_SCRIPT_BYTES,
            }),
            "too long AND unparseable must be refused for its length, not its syntax"
        );

        // Rule 2 (shape/naming) vs rule 3 (empty returns): both violated at
        // once — shape/naming must win.
        let mut shape_and_empty_returns = transformed_output(
            "def transform(params, statements):
  return {}",
        );
        shape_and_empty_returns.shape = "arrays".into();
        shape_and_empty_returns
            .transform
            .as_mut()
            .unwrap()
            .returns
            .clear();
        assert_eq!(
            check_transform(&shape_and_empty_returns),
            Err(ValidationError::TransformWithShapeOrNaming("shape")),
            "output.shape: arrays AND an empty returns must be refused for the shape, \
             not the empty list"
        );

        // Rule 2 (shape/naming) vs rule 4 (duplicate returns names): both
        // violated at once — shape/naming must still win.
        let mut shape_and_duplicate_returns = transformed_output(
            "def transform(params, statements):
  return {}",
        );
        shape_and_duplicate_returns.shape = "arrays".into();
        shape_and_duplicate_returns
            .transform
            .as_mut()
            .unwrap()
            .returns
            .push(ColumnSpec {
                name: "total".into(),
                ty: "varchar".into(),
                required: true,
            });
        assert_eq!(
            check_transform(&shape_and_duplicate_returns),
            Err(ValidationError::TransformWithShapeOrNaming("shape")),
            "output.shape: arrays AND a duplicate returns name must be refused for the \
             shape, not the duplicate"
        );
    }

    // -----------------------------------------------------------------
    // probe_transform (C2 Task 5) — publish RUNS the script against
    // synthetic rows and checks what comes back against `returns`
    // -----------------------------------------------------------------

    /// One described statement, as `validate` would hand it over: a name, the
    /// SQL, and the columns `describe_as` resolved — never any data.
    fn one_statement(name: &str) -> StatementSnapshot {
        StatementSnapshot {
            name: name.into(),
            sql: "SELECT 1 AS a, 'x' AS b".into(),
            columns: vec![
                ColumnSpec {
                    name: "a".into(),
                    ty: "bigint".into(),
                    required: true,
                },
                ColumnSpec {
                    name: "b".into(),
                    ty: "varchar".into(),
                    required: true,
                },
            ],
        }
    }

    /// No statement of the endpoint iterates any parameter — the shape every
    /// probe test below wants except the one that is about the shape.
    ///
    /// Built through `IteratedParams::across`, the ONLY constructor, rather than
    /// a hand-made empty value: a test that built the set a second way would
    /// stop being a statement about the set publish actually computes.
    fn not_iterated() -> IteratedParams {
        IteratedParams::across(&[], &[])
    }

    /// The DEFAULT limits, spelled as `LimitsSpec`'s own serde defaults do —
    /// `max_rows: 10_000` — so the probe tests below are about the transform
    /// rather than about a row cap nobody set.
    fn probe_limits(max_rows: u32) -> LimitsSpec {
        LimitsSpec {
            max_rows,
            timeout_seconds: 30,
        }
    }

    #[test]
    fn a_script_that_does_not_produce_its_declared_returns_is_refused_at_publish() {
        let mut o =
            transformed_output("def transform(params, statements):\n  return {\"wrong\": 1}");
        o.transform.as_mut().unwrap().returns = vec![ColumnSpec {
            name: "total".into(),
            ty: "bigint".into(),
            required: true,
        }];
        let err = probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(10_000),
        )
        .expect_err("declared `total`, produced `wrong`");
        assert!(
            err.to_string().contains("total"),
            "must name the missing field: {err}"
        );
    }

    #[test]
    fn an_optional_field_may_be_absent_but_an_undeclared_one_may_not() {
        // M1's `required` flag, unchanged: absent-when-optional is fine, present-
        // when-undeclared never is.
        let mut o = transformed_output("def transform(params, statements):\n  return {\"a\": 1}");
        o.transform.as_mut().unwrap().returns = vec![
            ColumnSpec {
                name: "a".into(),
                ty: "bigint".into(),
                required: true,
            },
            ColumnSpec {
                name: "b".into(),
                ty: "bigint".into(),
                required: false,
            },
        ];
        assert!(
            probe_transform(
                &o,
                &[one_statement("rows")],
                &[],
                &not_iterated(),
                &probe_limits(10_000)
            )
            .is_ok(),
            "optional `b` may be absent"
        );

        let mut p =
            transformed_output("def transform(params, statements):\n  return {\"a\": 1, \"z\": 2}");
        p.transform.as_mut().unwrap().returns = vec![ColumnSpec {
            name: "a".into(),
            ty: "bigint".into(),
            required: true,
        }];
        assert!(
            probe_transform(
                &p,
                &[one_statement("rows")],
                &[],
                &not_iterated(),
                &probe_limits(10_000)
            )
            .is_err(),
            "undeclared `z` must be refused"
        );
    }

    #[test]
    fn the_empty_row_set_is_probed_too() {
        // The commonest way a script's shape diverges. This script conforms with
        // rows and returns a bare list without them — publish must catch that, and
        // it can only catch it by running the empty case.
        let mut o = transformed_output(
            "def transform(params, statements):\n  \
             if len(statements[\"rows\"]) == 0:\n    return []\n  \
             return {\"total\": 1}",
        );
        o.transform.as_mut().unwrap().returns = vec![ColumnSpec {
            name: "total".into(),
            ty: "bigint".into(),
            required: true,
        }];
        let err = probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(10_000),
        )
        .expect_err("the empty-rows branch returns a list, not the declared object");
        assert!(
            err.to_string().contains("empty"),
            "the refusal must say WHICH probe failed, or an author cannot find it: {err}"
        );
    }

    /// **What the populated probe supplies: exactly one row, keyed by the
    /// described column names.**
    ///
    /// Asserted through a REFUSAL, not a success, and that is the whole design
    /// of this test. The obvious version — a script that reads the row through
    /// a `for` and returns something conformant — is **vacuous**: every
    /// assertion inside the loop is conditional on a row existing, so stubbing
    /// `probe_row` to `{}` leaves it green and supplying the populated probe
    /// with NO rows leaves it green too. Measured, both times.
    ///
    /// No script can distinguish "the populated probe was given no rows" from
    /// "this is the empty probe" — the two inputs would be identical — so the
    /// only construction that pins row PRESENCE is one where seeing a row is
    /// what produces the observable outcome. This script raises when it sees
    /// one, carrying the count and the keys in its message; the empty probe
    /// returns the declared object and stays silent. So:
    ///
    /// * populated probe empty → no failure at all → `expect_err` fires;
    /// * `probe_row` stubbed to `{}` → the keys in the message change;
    /// * more than one row → the count in the message changes.
    #[test]
    fn probe_rows_are_built_from_the_described_columns_not_from_data() {
        let o = transformed_output(
            "def transform(params, statements):\n  \
             rows = statements[\"rows\"]\n  \
             if len(rows) > 0:\n    \
             fail(\"saw \" + str(len(rows)) + \" row(s) keyed \" + str(sorted(rows[0].keys())))\n  \
             return {\"total\": 0}",
        );
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(10_000),
        ) {
            Err(ValidationError::TransformOutputMismatch { context, detail }) => {
                assert_eq!(
                    context, SINGLE_ROW_PROBE,
                    "the empty probe returns the declared object and cannot be what failed"
                );
                // `one_statement("rows")` describes exactly `a` and `b`, and the
                // probe supplies exactly one row of them.
                assert!(
                    detail.contains("saw 1 row(s) keyed [\"a\", \"b\"]"),
                    "the probe row must be ONE row keyed by the described columns: {detail}"
                );
            }
            other => panic!(
                "the populated probe must have supplied a row for the script to see, got \
                 {other:?}"
            ),
        }
    }

    /// **A script that assumes at least one row is refused, and that is the
    /// point of the empty probe.**
    ///
    /// A query returning nothing is not an error — it is a Tuesday — so
    /// `statements["rows"][0]` is an endpoint that answers 500 the first time a
    /// filter matches nothing. Only the empty probe can see it: the script is
    /// perfectly well-formed, parses, and satisfies `returns` for every input
    /// that has a row in it.
    #[test]
    fn a_script_that_indexes_row_zero_without_checking_is_refused() {
        let o = transformed_output(
            "def transform(params, statements):\n  \
             return {\"total\": len(statements[\"rows\"][0])}",
        );
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(10_000),
        ) {
            Err(ValidationError::TransformOutputMismatch { context, detail }) => {
                assert_eq!(
                    context, EMPTY_ROWS_PROBE,
                    "the populated probe is perfectly happy with this script"
                );
                assert!(
                    detail.contains("out of bound") && detail.contains("transform.star:2"),
                    "the author needs starlark's own words, and the LINE: {detail}"
                );
            }
            other => panic!("expected the empty probe to refuse this, got {other:?}"),
        }
    }

    /// The probe row's VALUES: `a bigint` arrives as an int, `b varchar` as a
    /// string — what `probe_value` promises through `openapi_gen::json_type`,
    /// and what a second, drifting type mapping would break.
    ///
    /// Reported the same way as the test above, and for the same reason: a
    /// script that checks the types INSIDE a `for` asserts nothing when there
    /// is no row, so it passes with `probe_row` stubbed to `{}` and passes with
    /// the populated probe emptied. Raising with the types in the message means
    /// the absence of a row is a failed `expect_err`, not a silent pass.
    #[test]
    fn the_stand_in_value_matches_the_columns_declared_type() {
        let o = transformed_output(
            "def transform(params, statements):\n  \
             rows = statements[\"rows\"]\n  \
             if len(rows) > 0:\n    \
             fail(\"a=\" + type(rows[0][\"a\"]) + \" b=\" + type(rows[0][\"b\"]))\n  \
             return {\"total\": 0}",
        );
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(10_000),
        ) {
            Err(ValidationError::TransformOutputMismatch { context, detail }) => {
                assert_eq!(context, SINGLE_ROW_PROBE);
                assert!(
                    detail.contains("a=int b=string"),
                    "`a bigint` must arrive as an int and `b varchar` as a string: {detail}"
                );
            }
            other => panic!("the populated probe must have supplied a row, got {other:?}"),
        }
    }

    /// **A parameter the endpoint's SQL iterates reaches the script as a
    /// one-element LIST; every other one reaches it as a scalar.**
    ///
    /// Not a preference — an agreement. `context_with` already binds exactly
    /// this set as a one-element list for the renders that DESCRIBE the
    /// statement, from the same `IteratedParams`, so a probe that handed the
    /// script a bare string for `region` would be probing it against a shape
    /// the rendered SQL says the endpoint does not have. Two probes of one
    /// endpoint, one publish, two answers.
    ///
    /// `since` is in the same list of declared parameters and is NOT iterated
    /// by any statement, which is what makes this a test about the set rather
    /// than about wrapping everything.
    #[test]
    fn a_parameter_the_sql_iterates_reaches_the_script_as_a_list() {
        const SQL: &str = "SELECT 1 AS a FROM t WHERE r IN (\
             {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %}) \
             AND d > {{ since }}";
        let params = vec![
            param("region", ParamType::String),
            param("since", ParamType::String),
        ];
        let iterated = IteratedParams::across(&[SQL], &params);
        assert!(
            iterated.contains("region") && !iterated.contains("since"),
            "the premise: this SQL iterates `region` and binds `since` as a scalar"
        );

        let o = transformed_output(
            "def transform(params, statements):\n  \
             fail(\"region=\" + type(params[\"region\"]) + \" since=\" + type(params[\"since\"]))",
        );
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &params,
            &iterated,
            &probe_limits(10_000),
        ) {
            Err(ValidationError::TransformOutputMismatch { context, detail }) => {
                assert_eq!(context, SINGLE_ROW_PROBE);
                assert!(
                    detail.contains("region=list since=string"),
                    "the iterated parameter must arrive as a list and the scalar one as a \
                     scalar: {detail}"
                );
            }
            other => panic!("the script raises on every input, got {other:?}"),
        }
    }

    /// The exact refusal, as a VALUE — context and detail both.
    ///
    /// `assert_eq!` on the whole payload rather than a `contains`, because this
    /// is the test that pins three separate decisions at once and each of them
    /// is invisible to an `is_err()`: which probe is named, that a
    /// `required: false` field missing from the output is NOT reported, and that
    /// the undeclared names are listed in a sorted order rather than in
    /// whichever order the script's dict happened to iterate.
    #[test]
    fn the_refusal_names_the_probe_and_every_field_that_disagrees() {
        let mut o =
            transformed_output("def transform(params, statements):\n  return {\"z\": 1, \"a\": 2}");
        o.transform.as_mut().unwrap().returns = vec![
            ColumnSpec {
                name: "total".into(),
                ty: "bigint".into(),
                required: true,
            },
            ColumnSpec {
                name: "opt".into(),
                ty: "bigint".into(),
                required: false,
            },
        ];
        assert_eq!(
            probe_transform(
                &o,
                &[one_statement("rows")],
                &[],
                &not_iterated(),
                &probe_limits(10_000)
            ),
            Err(ValidationError::TransformOutputMismatch {
                context: SINGLE_ROW_PROBE.to_owned(),
                detail: "it does not return the declared field(s) `total`; and it returns the \
                         undeclared field(s) `a`, `z`"
                    .to_owned(),
            }),
        );
    }

    /// A script that RAISES is attributed to the probe that ran it, not
    /// reported as a shape mismatch.
    #[test]
    fn a_script_that_fails_while_running_names_the_probe_that_ran_it() {
        let o = transformed_output(
            "def transform(params, statements):\n  \
             if len(statements[\"rows\"]) == 0:\n    fail(\"boom\")\n  \
             return {\"total\": 1}",
        );
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(10_000),
        ) {
            Err(ValidationError::TransformOutputMismatch { context, detail }) => {
                assert_eq!(
                    context, EMPTY_ROWS_PROBE,
                    "only the empty probe reaches `fail`"
                );
                assert!(
                    detail.contains("boom"),
                    "the author's own message: {detail}"
                );
            }
            other => panic!("expected the empty probe to report the failure, got {other:?}"),
        }
    }

    /// **The 1 MiB output cap, surfaced at publish rather than at serving.**
    ///
    /// Task 3 flagged this exact case: `MAX_TRANSFORM_OUTPUT_BYTES` will refuse
    /// a legitimate pass-through transform on a large `max_rows` endpoint, and
    /// the author should learn it here — where they can still change something —
    /// rather than from a caller's 500.
    ///
    /// The last assertion is the one that makes the message honest: `fits` is
    /// not decoration, it is a `max_rows` this endpoint actually publishes at.
    #[test]
    fn a_pass_through_transform_over_a_large_max_rows_is_refused_at_publish() {
        let mut o = transformed_output(
            "def transform(params, statements):\n  return {\"rows\": statements[\"rows\"]}",
        );
        o.transform.as_mut().unwrap().returns = vec![ColumnSpec {
            name: "rows".into(),
            ty: "varchar".into(),
            required: true,
        }];
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(1_000_000),
        ) {
            Err(ValidationError::TransformOutputExceedsRowLimit {
                per_row,
                max_rows,
                projected,
                cap,
                fits,
            }) => {
                assert_eq!(cap, 1 << 20, "the cap the message quotes is the real one");
                assert_eq!(max_rows, 1_000_000);
                // `{"rows":[{"a":1,"b":"x"}]}` minus `{"rows":[]}`.
                assert_eq!(per_row, 15, "measured, not assumed");
                assert_eq!(
                    projected,
                    11 + 15 * 1_000_000,
                    "base bytes plus the growth, both measured"
                );
                assert!(fits > 0 && fits < max_rows, "a usable number: {fits}");
                assert_eq!(
                    probe_transform(
                        &o,
                        &[one_statement("rows")],
                        &[],
                        &not_iterated(),
                        &probe_limits(fits)
                    ),
                    Ok(()),
                    "the max_rows the refusal tells the author to use must actually publish"
                );
                assert!(
                    probe_transform(
                        &o,
                        &[one_statement("rows")],
                        &[],
                        &not_iterated(),
                        &probe_limits(fits + 1)
                    )
                    .is_err(),
                    "and it must be the LARGEST one that does"
                );
            }
            other => panic!("expected TransformOutputExceedsRowLimit, got {other:?}"),
        }
    }

    /// The projection is about GROWTH, so a script that aggregates is untouched
    /// however large `max_rows` is. Without this, "refuse what cannot fit" could
    /// be implemented as "refuse a big `max_rows`", which would refuse the one
    /// thing a response transform is actually for.
    #[test]
    fn an_aggregating_transform_is_never_refused_for_its_row_limit() {
        let o = transformed_output(
            "def transform(params, statements):\n  \
             return {\"total\": len(statements[\"rows\"])}",
        );
        assert_eq!(
            probe_transform(
                &o,
                &[one_statement("rows")],
                &[],
                &not_iterated(),
                &probe_limits(u32::MAX)
            ),
            Ok(()),
            "output that does not grow with the rows has nothing to project"
        );
    }

    /// The projection counts the rows SERVING will fetch, not the number the
    /// author typed. `serving::router` clamps `max_rows` to `MAX_ROWS_CEILING`
    /// before the engine ever sees it, so projecting against a raw `u32` would
    /// quote an author a number of rows their endpoint can never return.
    #[test]
    fn the_projection_uses_the_row_count_serving_will_actually_fetch() {
        let mut o = transformed_output(
            "def transform(params, statements):\n  return {\"rows\": statements[\"rows\"]}",
        );
        o.transform.as_mut().unwrap().returns = vec![ColumnSpec {
            name: "rows".into(),
            ty: "varchar".into(),
            required: true,
        }];
        match probe_transform(
            &o,
            &[one_statement("rows")],
            &[],
            &not_iterated(),
            &probe_limits(u32::MAX),
        ) {
            Err(ValidationError::TransformOutputExceedsRowLimit { max_rows, .. }) => {
                assert_eq!(
                    max_rows,
                    crate::serving::router::MAX_ROWS_CEILING,
                    "the refusal must quote the clamped count, not u32::MAX"
                );
            }
            other => panic!("expected TransformOutputExceedsRowLimit, got {other:?}"),
        }
    }

    /// An endpoint with no transform is not probed at all — the whole feature
    /// is invisible to every endpoint that does not use it.
    #[test]
    fn an_endpoint_without_a_transform_is_not_probed() {
        assert_eq!(
            probe_transform(
                &OutputSpec::default(),
                &[one_statement("rows")],
                &[],
                &not_iterated(),
                &probe_limits(u32::MAX)
            ),
            Ok(())
        );
    }

    /// A `QueryExecPort` that describes two columns and can do nothing else.
    ///
    /// `execute_as` is unreachable by construction: publish DESCRIBES, it never
    /// executes, and the probe rows are built from the description. If this
    /// panic ever fires, publish has started reading data.
    struct DescribeOnlyPort;

    #[async_trait::async_trait]
    impl egret_query_api::QueryExecPort for DescribeOnlyPort {
        async fn describe_as(
            &self,
            _ws: Uuid,
            _database_id: Uuid,
            _principal: UserId,
            _sql: &str,
        ) -> std::result::Result<Vec<egret_core::ColumnMeta>, Error> {
            Ok(vec![
                egret_core::ColumnMeta {
                    name: "a".into(),
                    ty: "bigint".into(),
                },
                egret_core::ColumnMeta {
                    name: "b".into(),
                    ty: "varchar".into(),
                },
            ])
        }

        /// Overridden rather than left to the trait default, which REFUSES a
        /// parameterised describe — and a conditional statement renders to one.
        /// The default's refusal would reach a test as an `Internal`, which is
        /// indistinguishable from the probe failing for a reason the test is
        /// about.
        async fn describe_as_with_params(
            &self,
            ws: Uuid,
            database_id: Uuid,
            principal: UserId,
            sql: &str,
            _param_count: usize,
        ) -> std::result::Result<Vec<egret_core::ColumnMeta>, Error> {
            self.describe_as(ws, database_id, principal, sql).await
        }

        async fn execute_as(
            &self,
            _ws: Uuid,
            _database_id: Uuid,
            _principal: UserId,
            _sql: &str,
            _limits: egret_query_api::Limits,
        ) -> std::result::Result<egret_core::TabularResult, Error> {
            unreachable!("publish must not execute a statement to probe a transform")
        }
    }

    /// A draft carrying `output`, with one statement and no parameters.
    fn draft_with(output: OutputSpec, limits: LimitsSpec) -> DataApiEndpointDto {
        DataApiEndpointDto {
            id: Uuid::new_v4(),
            folder_id: None,
            name: "probe".into(),
            description: String::new(),
            path: "probe".into(),
            method: "GET".into(),
            database_id: Uuid::new_v4(),
            statements: vec![StatementSpec {
                name: "rows".into(),
                sql: "SELECT 1 AS a, 'x' AS b FROM t".into(),
            }],
            params: vec![],
            output,
            cache: cache(CacheScope::PerPrincipal),
            limits,
            rate_limit: RateLimitSpec {
                per_key_qps: 0,
                burst: 0,
                per_endpoint_qps: 0,
            },
            source_kind: "manual".into(),
            source_ref: None,
            live_version_id: None,
            state: "draft".into(),
            created_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        }
    }

    /// **Proves the probe is reachable from the real publish path, not only
    /// callable in isolation** — the Task 4 test one function along, for the
    /// check that costs an evaluation rather than a comparison.
    ///
    /// `validate` is what `POST …/validate` and `EndpointStore::publish` both go
    /// through (via `validated_draft`), and it is where the described columns
    /// exist, so it is the only place this check can be wired. Deleting the
    /// `probe_transform` call at the end of `validate` must turn this test
    /// green-with-a-published-endpoint — that is the mutation, and a unit test
    /// on `probe_transform` alone cannot see it.
    #[tokio::test]
    async fn the_probe_runs_on_the_path_validate_and_publish_share() {
        let draft = draft_with(
            transformed_output("def transform(params, statements):\n  return {\"wrong\": 1}"),
            probe_limits(10_000),
        );
        let err = validate(
            &DescribeOnlyPort,
            Uuid::new_v4(),
            UserId::new(),
            &draft,
            false,
            false,
        )
        .await
        .expect_err("the script declares `total` and returns `wrong`");
        let message = err.to_string();
        assert!(
            message.contains("total") && message.contains(SINGLE_ROW_PROBE),
            "the publish path must carry the probe's own refusal: {message}"
        );
    }

    /// **The shape the probe binds comes from the ENDPOINT's own
    /// `IteratedParams`, not from a set the probe made up.**
    ///
    /// `a_parameter_the_sql_iterates_reaches_the_script_as_a_list` builds that
    /// set itself, so it cannot see a `validate` that passes a fresh empty one
    /// to `probe_transform` — the shape would then be decided twice, once for
    /// the renders and once, differently, for the script. This drives the real
    /// `validate`, whose SQL iterates `region`, and asserts the script saw a
    /// list.
    #[tokio::test]
    async fn the_probe_binds_the_shape_the_endpoints_own_sql_decided() {
        let mut draft = draft_with(
            transformed_output(
                "def transform(params, statements):\n  \
                 fail(\"region=\" + type(params[\"region\"]))",
            ),
            probe_limits(10_000),
        );
        draft.params = vec![param("region", ParamType::String)];
        draft.statements = vec![StatementSpec {
            name: "rows".into(),
            sql: "SELECT 1 AS a FROM t WHERE r IN (\
                  {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})"
                .into(),
        }];

        let err = validate(
            &DescribeOnlyPort,
            Uuid::new_v4(),
            UserId::new(),
            &draft,
            false,
            false,
        )
        .await
        .expect_err("the script raises on every input");
        assert!(
            err.to_string().contains("region=list"),
            "publish renders this statement with `region` as a one-element list, so the \
             script must see one too: {err}"
        );
    }

    /// The same path, with a script that conforms: it publishes, and the frozen
    /// snapshot is unchanged by the probe.
    #[tokio::test]
    async fn a_conforming_script_validates_and_freezes_the_described_columns() {
        let draft = draft_with(
            transformed_output(
                "def transform(params, statements):\n  \
                 return {\"total\": len(statements[\"rows\"])}",
            ),
            probe_limits(10_000),
        );
        let frozen = validate(
            &DescribeOnlyPort,
            Uuid::new_v4(),
            UserId::new(),
            &draft,
            false,
            false,
        )
        .await
        .expect("a conforming transform publishes");
        assert_eq!(frozen.len(), 1);
        assert_eq!(
            frozen[0]
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"],
            "the probe must not change what publish freezes"
        );
    }
}
