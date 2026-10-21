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
//! **Routing comes from the endpoint row's LIVE route** —
//! `dataapi.endpoints (live_method, live_path)`, restricted to `state = 'live'`
//! — and the request is then served entirely from the snapshot reached through
//! `live_version_id`. The live route is the snapshot's own `method`/`path`,
//! copied onto the row by the only two writers of `live_version_id` (publish and
//! rollback) so that a `UNIQUE` index can hold it: routing through the snapshot
//! alone would lose the constraint, and an author who moved a live endpoint and
//! then published a second one at the old URL would have two live snapshots
//! answering to the same path with nothing to break the tie. With the index that
//! second publish is a 409.
//!
//! **A draft edit does not move a live endpoint's URL.** It used to: routing
//! matched the draft's `method`/`path`, so saving a new path changed the served
//! URL before publish (KI-023) — the old URL went 404 and the new one served the
//! old version. A URL is part of the published contract (`contract.rs` already
//! classifies a change of it as breaking), so the draft's new path takes effect
//! at the next publish, like every other draft edit.
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
    OutputSpec, ParamLocation, ParamSpec, ParamType, StatementSnapshot, StatementSource,
    StatementSpec, TypeAgreement, PARAM_GATE,
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
    /// A statement's `build_sql` script did not produce a template.
    ///
    /// The author's 400, not the caller's 500, for the same reason
    /// [`ValidationError::TransformScriptInvalid`] is: a script that does not
    /// parse, does not define `build_sql`, takes the wrong arguments, returns
    /// something that is not a string, or exceeds a budget is a statement about
    /// the definition the author just wrote.
    ///
    /// Names the STATEMENT, because an endpoint may have up to
    /// [`egret_dataapi_api::MAX_STATEMENTS`] of them and
    /// [`crate::transform::TransformError`] knows only about the one script it
    /// was handed.
    #[error("statement `{statement}` could not build its SQL: {error}")]
    StatementScriptFailed {
        /// The statement whose script failed, by name.
        statement: String,
        /// [`crate::transform::TransformError`], rendered.
        error: String,
    },
    /// A statement has no entry in the [`GeneratedSql`] this validation was
    /// handed — which can only mean the set was generated from a DIFFERENT
    /// statement list.
    ///
    /// **Not the Task 1 placeholder this replaced.** That one meant "nothing can
    /// generate a script yet"; this one means "the text supplied belongs to
    /// another endpoint's statements". The only caller that could produce it is
    /// [`EndpointStore::publish`], which generates from the DRAFT before it takes
    /// the row lock and then re-validates the LOCKED row (see [`ScriptEval`]) —
    /// so it is what the identity check between those two rows exists to make
    /// unreachable, held by the data rather than by that check alone.
    #[error(
        "statement `{0}` has no generated SQL: the endpoint was edited while it was being \
         published; validate and publish again"
    )]
    GenerationMismatch(String),
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
    /// A `build_sql` script put a parameter's VALUE into the SQL TEXT.
    ///
    /// Found by [`GeneratedSql::probe`], behaviourally: the script was run with
    /// [`SENTINEL`] in this parameter's place and the sentinel came back inside
    /// the template. See that function for what the probe does and — more
    /// importantly — what it does not prove.
    ///
    /// The message names both the statement and the parameter, and offers the
    /// three spellings that are safe, because "do not do that" without them
    /// sends an author to delete a feature they are entitled to use: `{{ x }}`
    /// for a value, an index into a table of names the SCRIPT contains for an
    /// identifier, and `{{ x | ident }}` for an enum parameter (spec §2.2).
    #[error(
        "statement `{statement}` builds its SQL by putting parameter `{parameter}`'s value into \
         the SQL TEXT: running the script with a sentinel in its place produced a template \
         containing the sentinel. A caller's value must reach the engine as a BIND, so emit the \
         placeholder `{{{{ {parameter} }}}}` and let the renderer bind it. If what you need is an \
         IDENTIFIER, pick it from a table of names the script itself contains \
         (`TABLES[params[\"{parameter}\"]]`), or — for an enum parameter — emit \
         `{{{{ {parameter} | ident }}}}`"
    )]
    ParameterValueInSqlText {
        /// The statement whose script emitted it, by name.
        statement: String,
        /// The parameter whose value reached the text.
        parameter: String,
    },
    /// A `String` parameter on an endpoint that GENERATES SQL carries no
    /// `pattern` (package 5).
    ///
    /// The publish probe ([`GeneratedSql::probe`]) can prove a value DID reach
    /// the SQL text; it cannot prove one never will, and its two documented
    /// bypasses are exactly that gap. So the rule is declarative instead: on an
    /// endpoint where a script builds SQL, a `String` parameter must be bounded
    /// to a character set that stays harmless whatever the script does to it.
    /// See [`check_script_parameter_constraints`] for which parameters this
    /// applies to and why it is every one of them.
    ///
    /// The message carries the whole recourse, because this rule refuses drafts
    /// that are not injections and the author has to be able to tell which of
    /// the two answers is theirs.
    #[error(
        "parameter `{parameter}` is a `string` on an endpoint where statement `{statement}` \
         BUILDS its SQL with a script, so no rule can promise the value stays out of the SQL \
         text: a script may transform it first, and the sentinel probe would then see nothing. \
         Give it a `pattern` accepting only {alphabet} (for example `^[A-Za-z0-9_]+$`), so the \
         value is harmless wherever the script puts it. If the parameter is genuinely free text \
         — a search term — it must be BOUND rather than built: write that statement as `sql` \
         with `{{{{ {parameter} }}}}` in it, on an endpoint with no script"
    )]
    StringParameterNeedsPattern {
        /// The parameter that needs the constraint.
        parameter: String,
        /// One statement that generates its SQL, so the author knows which one
        /// brought the rule with it.
        statement: String,
        /// [`SAFE_ALPHABET_PROSE`], so the message and the rule cannot drift.
        alphabet: &'static str,
    },
    /// A `String` parameter's `pattern` accepts a character that is not safe in
    /// SQL text, on an endpoint that generates SQL (package 5).
    ///
    /// Separate from [`ValidationError::StringParameterNeedsPattern`] because
    /// the author's action is different — narrow the pattern you have, rather
    /// than write one — and because naming the character is the difference
    /// between a rule and a riddle.
    ///
    /// `{witness:?}` rather than `{witness}`, and it is not cosmetic: the widest
    /// patterns an author writes (`.`, `[^x]`, `\s`) have their first unsafe
    /// character in the control range, and a `Display`ed `\0` or newline is
    /// either invisible in the message or breaks the line it is on. `Debug` for
    /// `char` quotes and escapes, so every witness arrives visible.
    #[error(
        "parameter `{parameter}`'s `pattern` accepts {witness:?}, which is not safe in SQL text, \
         and statement `{statement}` BUILDS its SQL with a script. A script may put the value \
         anywhere in the statement and may transform it on the way, so the pattern must accept \
         only {alphabet} — narrow it (for example `^[A-Za-z0-9_]+$`) or bind the parameter from \
         a `sql` statement instead"
    )]
    StringParameterPatternTooWide {
        /// The parameter whose `pattern` is too wide.
        parameter: String,
        /// One character the pattern would let through. One, not all of them:
        /// the author needs a witness, not an inventory.
        witness: char,
        /// One statement that generates its SQL.
        statement: String,
        /// [`SAFE_ALPHABET_PROSE`].
        alphabet: &'static str,
    },
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
    // Names both fields it will accept, for the same reason the arm above names
    // both: an author told only "turn on delegation" has been told one of two
    // answers, and the one they can actually reach may be the other.
    //
    // "published onto it OR PREVIEWED against it", because both refuse through
    // this one variant and an author who meets it at Run must not read a
    // sentence that is only about Publish — see [`crate::preview::run`] for why
    // preview is gated and what the note it replaced got wrong.
    #[error(
        "this database is marked `governed` — engine-side row filters and column masks apply to \
         its data — so a statement may only be published onto it, or previewed against it, \
         through a connection that runs queries as the CALLER. This connection has \
         `delegated: false` and no `options.impersonate`, so every request would run on the \
         shared service credential and those filters would be evaluated for the service account \
         rather than for the consumer — a preview included, which is why Run is refused too. \
         Turn on delegation (or `options.impersonate`) for the connection, point the endpoint at \
         a connection that already has it, or — if this data is not in fact governed — clear \
         `governed` on the database"
    )]
    GovernedDatabaseNeedsPerPrincipalConnection,
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
    /// More distinct SQL templates across the endpoint than publish will render.
    ///
    /// # This is [`egret_dataapi_api::MAX_STATEMENTS`], counting what that bound
    /// was always counting
    ///
    /// `MAX_STATEMENTS`'s own doc records what it bounds: "the publish CPU from
    /// unbounded … to a measured 1.97 s". The quantity in that measurement is
    /// **the number of TEXTS publish renders**, and until package S the two were
    /// the same thing — one statement was one text, so counting statements
    /// counted texts. A `build_sql` script that branches breaks the identity: one
    /// statement, up to `2 × MAX_DECLARED_PARAMS + 1` = 65 texts, each rendered
    /// and enumerated exactly as a written statement's is.
    ///
    /// So this is not a second budget. It is `MAX_STATEMENTS` asked about the
    /// thing it was measuring, and a script's branches draw on the SAME
    /// allowance a written statement does: sixteen texts per endpoint, whether
    /// they arrive as sixteen `sql` statements, as one script with fifteen
    /// branches, or as any mixture. An endpoint at the statement limit with no
    /// script is at exactly sixteen and is unaffected — which is what keeps this
    /// task invisible to every definition that has none.
    ///
    /// # Measured, because the multiplication is the whole point
    ///
    /// Release build, the most expensive text the other limits admit (32 boolean
    /// parameters each guarding its own `{% if %}`, ~129 ms of synchronous
    /// rendering), timing `validate` up to its refusal:
    ///
    /// | endpoint | texts | measured |
    /// |---|---|---|
    /// | 16 written statements — today's worst case, unchanged | 16 | **2.06 s** |
    /// | one script branching on 31 parameters, capped at `MAX_DESCRIBED_BRANCHES` | 64 | **9.29 s** |
    /// | the same script, capped here | 16 | **2.03 s** |
    ///
    /// The middle row is why the count is `MAX_STATEMENTS` and not the branch
    /// budget: 64 texts is a defensible number for the ENGINE half and a 4.5×
    /// regression on the CPU half, and the CPU half is the one that occupies a
    /// tokio worker rather than releasing it. Uncapped it is worse still —
    /// `MAX_STATEMENTS × 65` = 1040 texts, over two minutes.
    ///
    /// # Why the count and not [`ValidationError::TooManyBranches`]
    ///
    /// Because that one is measured AFTER the work, deliberately: its message
    /// quotes the endpoint's exact branch total and
    /// `the_describe_budget_is_the_endpoints_total_not_one_per_statement`
    /// derives the expected number from a real publish rather than hard-coding
    /// it, so the count cannot be made partial without making that message a
    /// lie. The rendering that produces it is what has to be bounded, and this
    /// is a `usize` addition over strings the script has already returned. Same
    /// shape as [`MAX_DECLARED_PARAMS`]: bound the cost at its source, with a
    /// scan, before the first render. It runs inside [`GeneratedSql::build`]'s
    /// own loop, so it also stops the remaining statements' scripts from being
    /// evaluated at all.
    ///
    /// # Why it refuses rather than truncates
    ///
    /// Identical to [`ValidationError::TooManyBranches`], and for the identical
    /// reason: a template that is dropped is a branch that is not described, and
    /// a branch that is not described is a column missing from the union — the
    /// `503 schema_drift` the union exists to remove.
    #[error(
        "this endpoint renders to {templates} distinct SQL templates across its statements, and \
         the limit is {limit}. A `build_sql` script that branches returns a different template \
         per branch, and publish renders and describes every one of them so the frozen contract \
         covers every branch a caller can reach — so a script's branches and an endpoint's \
         statements are drawn from the same allowance. Branch on fewer parameters, or split \
         this into two endpoints"
    )]
    TooManyGeneratedTemplates { templates: usize, limit: usize },
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
    /// `output.transform.returns` declares more columns than
    /// [`MAX_TRANSFORM_RETURNS`].
    ///
    /// Carries both numbers, like every other `…TooMany` arm here. See the
    /// constant for why serving, not publish, is where the cost lands.
    #[error(
        "output.transform.returns declares {count} columns, over the {limit} column cap. \
         Serving checks every response against this list on every request, and a response \
         transform reshapes a response — it is not a place to enumerate a wide table"
    )]
    TransformReturnsTooMany { count: usize, limit: usize },
    /// An entry in `output.transform.returns` has a blank `name`.
    ///
    /// No payload: the offending name is the empty string, and quoting it back
    /// (`declares `` more than once`, which is what the duplicate rule used to
    /// say for two unnamed rows) names nothing an author can find.
    #[error(
        "output.transform.returns has a column with no name. A declared column is matched \
         against the response by name, and a blank name matches nothing — name it, or \
         remove the row"
    )]
    TransformReturnsBlankName,
    /// An entry in `output.transform.returns` has a `name` with leading or
    /// trailing whitespace.
    ///
    /// Refused rather than trimmed. Trimming would silently publish a contract
    /// whose OpenAPI property name is not the string the author typed, and
    /// accepting it as-is makes `"total"` and `"total "` two columns that every
    /// reader sees as one.
    #[error(
        "output.transform.returns declares `{0}`, whose name begins or ends with whitespace. \
         The name becomes a JSON key and an OpenAPI property name exactly as written, so \
         `{0}` and its trimmed spelling would be two columns nobody can tell apart"
    )]
    TransformReturnsPaddedName(String),
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

/// The three `query.databases` columns a publish decision is made from, carried
/// as one value so a call site cannot transpose them.
///
/// Three adjacent `bool` parameters is a signature where `(governed,
/// delegated)` and `(delegated, governed)` both compile and one of them is a
/// silent hole; the pair this replaces was already two. Every field is a LIVE,
/// mutable column, which is why this type is read at each decision point rather
/// than cached anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionSafety {
    /// `query.databases.governed` — the data is filtered per principal at the
    /// ENGINE, so a query that does not run as the caller reads it under the
    /// service account's policy.
    pub governed: bool,
    /// `query.databases.delegated`.
    pub delegated: bool,
    /// `query.databases.options.impersonate`, read through [`impersonate_flag`].
    pub impersonate: bool,
}

impl ConnectionSafety {
    /// What a freshly registered connection is: ungoverned, non-delegated, no
    /// impersonation — the three column defaults in `query.databases`, and the
    /// shape every rule that is not ABOUT the connection is tested against.
    pub const UNGOVERNED: Self = Self {
        governed: false,
        delegated: false,
        impersonate: false,
    };

    /// Whether the engine runs this connection's queries as the caller. Thin
    /// wrapper over [`runs_per_principal`] so the predicate stays one
    /// expression — see its doc comment for why it is two fields and not one.
    pub fn runs_per_principal(&self) -> bool {
        runs_per_principal(self.delegated, self.impersonate)
    }
}

/// A `governed` database may only carry an endpoint whose connection runs each
/// query as the CALLER — see [`runs_per_principal`] for why that is two fields
/// and not one.
///
/// # The defect this closes
///
/// A served request executes through `execute_as`, which runs the SQL as the
/// SERVICE ACCOUNT unless the connection is delegated or impersonating
/// (`egret-query/src/execution/mod.rs:44-88`). Ranger row filters and column
/// masks are therefore evaluated at MACHINE-identity granularity: every
/// consumer of the endpoint reads exactly what the service account may read,
/// whatever their own policy says. Nothing required a delegated connection
/// before this check — publishing onto a non-delegated database was permitted
/// with no warning, so the bypass was not an edge case but the default.
///
/// # Why a per-database flag and not a blanket refusal
///
/// Refusing every non-delegated publish is one line and would break the
/// endpoints that work today, most of which are over data with no engine-side
/// policy at all. The design (§4, Decision A) took the per-database flag over a
/// workspace-wide switch and over an author acknowledgement because it is the
/// only one of the three where the SAFE answer does not depend on someone
/// remembering: the decision is recorded once, by the person who knows the
/// data, on the row the data is behind.
///
/// # Refused at VALIDATE time, exactly like [`check_cache_scope`]
///
/// Same position and the same reason: the unsafe configuration can never enter
/// `endpoint_versions`, rather than being caught (or not) on a request path.
/// The two checks sit side by side in [`validate_endpoint`] so both entry points
/// — `POST …/validate` and `EndpointStore::publish`'s in-transaction re-run —
/// get them by construction.
///
/// # And at PREVIEW, which is the third caller and not a publish
///
/// [`crate::preview::run`] calls this directly, off the `validate_endpoint`
/// path, because a preview is not a definition being frozen — it is a read of
/// the same governed data, executed as the SERVICE ACCOUNT exactly like a
/// served request. That doc comment carries the decision and the measurement
/// behind it; what matters here is that this predicate now guards every route
/// by which this crate reads a governed database: preview, try-it, publish,
/// serve. `runs_per_principal` remains the one expression of "runs as the
/// caller" and this remains the one expression of the rule built on it.
///
/// **This is not the whole guard, and cannot be.** Both `governed` and
/// `delegated` are mutable columns, so a legitimately published endpoint becomes
/// unsafe the moment someone marks the database governed or turns delegation
/// off. What happens then is decided on the serving side; see
/// [`crate::serving::governance_allows_serving`], which explains why it does NOT
/// use the frozen-witness shape the `shared`-cache drift check uses.
pub fn check_governed_connection(
    conn: ConnectionSafety,
) -> std::result::Result<(), ValidationError> {
    if conn.governed && !conn.runs_per_principal() {
        return Err(ValidationError::GovernedDatabaseNeedsPerPrincipalConnection);
    }
    Ok(())
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

/// What [`compile_pattern`] refuses, and the two are different author errors.
///
/// Split out rather than collapsed into `regex::Error` because the first one is
/// not a fact about the regex crate at all: it is a fact about how egret
/// ANCHORS an author's pattern, and an author told only "invalid regex" about a
/// pattern their own editor's tester accepted would have nothing to act on.
#[derive(Debug)]
pub enum PatternError {
    /// The author's pattern is not a complete regular expression **on its own**.
    ///
    /// See [`compile_pattern`] for why that is refused rather than wrapped.
    ///
    /// `Box`ed because `regex_syntax::Error` is ~128 bytes and this is the
    /// `Err` half of the `Result` every pattern compile returns, on the serving
    /// path as well as at publish (`clippy::result_large_err`).
    NotStandalone(Box<regex_syntax::Error>),
    /// The anchored form does not compile, or compiles past
    /// [`MAX_PATTERN_PROGRAM_BYTES`].
    Compile(regex::Error),
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStandalone(e) => write!(
                f,
                "it must be a complete regular expression ON ITS OWN, because egret matches it \
                 anchored — as `^(?:…)$` — and this one is not: {e}"
            ),
            Self::Compile(e) => write!(f, "{e}"),
        }
    }
}

/// Compile a `pattern` the way `coerce` will, so publish and serving cannot
/// disagree about which patterns exist.
///
/// **Anchored here, once.** The anchors are added in this one function, so a
/// pattern accepted at publish is byte-identical to the one matched at serve.
/// An author's own `^…$` is idempotent under it.
///
/// # The author's pattern must parse ON ITS OWN, and that IS the anchoring
///
/// [`anchored`] wraps author TEXT, and text can be written to re-balance
/// against a wrapper. `a)|(a` becomes `^(?:a)|(a)$`, which compiles — and the
/// wrapper's `^` and `$` have landed on DIFFERENT alternatives: `^(?:a)` keeps
/// only the start anchor, `(a)$` only the end, and neither constrains the whole
/// value any more. So the `is_match` in [`crate::serving::params::coerce`]
/// accepts `a' UNION SELECT password FROM users --` — it begins with `a`, which
/// is all the first alternative asks — while every character of the
/// pattern is inside [`SAFE_ALPHABET`] and
/// [`pattern_confines_to_safe_alphabet`] therefore answers `Ok`. Publish
/// returned 200 on that endpoint, and the caller's bytes reached the SQL text a
/// script builds: the whole of package 5's claim, defeated by two parentheses.
///
/// Requiring the RAW pattern to parse closes it by construction rather than by
/// looking for the shapes that do it. A pattern that parses standalone has
/// balanced groups, so `(?:` … `)` encloses exactly it and the anchors bind the
/// whole language; a pattern that does not is refused before anything wraps it.
/// **Nothing here inspects the pattern for `)`** — the check is that the
/// author's own string is a regex, which is the property that makes the wrapper
/// sound, and the only one that stays sound under a `regex-syntax` that grows
/// new constructs.
///
/// Two alternatives were considered and not taken. Anchoring STRUCTURALLY — a
/// full-match API instead of `is_match` over wrapped text — is the shape that
/// makes this class of bug unrepresentable, and `regex` has no such API:
/// `find` is leftmost-FIRST, so `a|ab` would "fully match" `ab` only by
/// accident of which alternative won, and a hand-rolled
/// `m.start() == 0 && m.end() == len` is unsound for exactly that reason.
/// Building the anchored HIR and printing it back
/// (`Hir::concat([Look::Start, hir, Look::End])`) IS a real structural
/// anchoring, but it rests on the HIR printer round-tripping every construct
/// faithfully — a wider trust than "the author's string parses" — and it would
/// make the pattern egret matches differ textually from the one the author
/// wrote and the one [`crate::openapi_gen`] publishes in the schema. The
/// raw-parse requirement is what makes the textual wrapper provably equal to a
/// full match, so the wrapper stays and the trust does not grow.
///
/// # It runs at SERVE time too, and that is deliberate
///
/// This is the function `coerce` compiles through, so an endpoint frozen before
/// this check existed carrying a breakout `pattern` now matches nothing: the
/// compile fails and `coerce` answers
/// [`crate::serving::params::CoerceError::Malformed`] — a redacted 500 rather
/// than an accepted injection. Fail closed on a stored row, which is the only
/// direction available once the row exists.
pub fn compile_pattern(pattern: &str) -> std::result::Result<regex::Regex, PatternError> {
    // Parsed with `regex_syntax`'s defaults, which are the defaults `regex`
    // itself parses with — the same agreement [`anchored`] documents, and the
    // reason `Cargo.toml` says those two crates must resolve to ONE copy
    // (`regex_and_regex_syntax_resolve_to_one_parser` asserts it).
    regex_syntax::parse(pattern).map_err(|e| PatternError::NotStandalone(Box::new(e)))?;
    regex::RegexBuilder::new(&anchored(pattern))
        .size_limit(MAX_PATTERN_PROGRAM_BYTES)
        .build()
        .map_err(PatternError::Compile)
}

/// The exact source text [`compile_pattern`] compiles.
///
/// Extracted so [`pattern_confines_to_safe_alphabet`] can parse **the same
/// string** the matcher is built from. Analysing the author's raw pattern
/// instead would be analysing a different regex from the one enforced, and the
/// gap between the two is where a wrapper's own metacharacters would live.
///
/// **The wrapper is only sound because the author's pattern is required to
/// parse ON ITS OWN** ([`compile_pattern`]). Wrapping arbitrary text in
/// `^(?:…)$` is not anchoring: `a)|(a` re-balances against the wrapper and
/// splits its two anchors across two alternatives, `^(?:a)` and `(a)$`, neither
/// of which bounds the whole value; write it with one more alternative
/// (`a)|([a-z]+)|(a`) and the middle one carries no anchor at all. With the raw
/// parse
/// demanded first, `(?:` … `)` encloses exactly the author's language and this
/// string's meaning is "the whole value matches the author's pattern" — which
/// is the sentence every doc below is written against.
///
/// The other half of that agreement is the parser's CONFIGURATION, and it is
/// inherited rather than restated: `RegexBuilder` above sets only
/// `size_limit`, and `regex_syntax::parse` is `ParserBuilder`'s defaults —
/// which are the same defaults `regex` itself parses with. A syntax flag added
/// to [`compile_pattern`] (`case_insensitive`, `octal`, `ignore_whitespace`)
/// would make the two read one string as two languages, so it belongs on both
/// sides or on neither.
fn anchored(pattern: &str) -> String {
    format!("^(?:{pattern})$")
}

/// The characters a caller's value may be made of if a script is allowed to put
/// it into SQL **text** (package 5).
///
/// # A whitelist, and the argument for these four blocks and nothing else
///
/// ASCII letters, ASCII digits and `_`. That is exactly the body of an unquoted
/// identifier on both engines, and it is chosen for what it EXCLUDES: none of
/// these bytes can open or close a string literal (`\'`, `"`, `` ` ``), begin a
/// comment (`-`, `/`, `*`, `#`), separate a statement (`;`), escape the next
/// byte (`\\` — the StarRocks half of the dual-dialect proof), qualify a name
/// into another schema (`.`), or end a token so a keyword can follow
/// (whitespace). A value drawn from this alphabet is a single bare word wherever
/// it lands.
///
/// # The property that actually matters: it survives what a script DOES to the
/// value
///
/// The gate this supports is not "the value reaches the text unchanged" — the
/// publish probe already cannot promise that, and `.upper()` is the standing
/// counter-example. The property needed is stronger and simpler: **whatever the
/// script does to the value, the result is still harmless.** This alphabet is
/// closed under every Starlark string operation that *selects* from its input:
///
/// * `upper()`, `lower()`, `title()`, `capitalize()` — case mapping maps ASCII
///   letters to ASCII letters and fixes digits and `_`, so `[a-z]+` becoming
///   `[A-Z]+` moves inside the set, which is the whole point;
/// * slicing, indexing, `reversed`, `strip`, `removeprefix`/`removesuffix`,
///   `partition`, `split`, `elems` — every character of every result was a
///   character of the input, so a list of pieces of a safe value is a list of
///   safe values;
/// * repetition (`value * n`) and concatenation of two safe values — the
///   multiset grows, the alphabet does not;
/// * concatenation with the AUTHOR's own text — the author's half is author
///   text, which is the same provenance `ident_filter` already blesses.
///
/// **`join` is NOT on that list, and it was**, until a reviewer read the
/// justification against the operation: `sep.join(pieces)` takes `sep` from
/// wherever the script's author put it and inserts it once per gap, and the
/// number of gaps is `len(pieces)` — which a caller controls whenever the
/// pieces came from their own value. `" ".join(params["q"].split("_"))` is
/// idiomatic humanisation, every operation on it looks safe, and `q=1_OR_true`
/// renders `WHERE flag = 1 OR true`. The separator is author text; the DECISION
/// to emit it, and how many times, is the caller's. It belongs in the paragraph
/// below with `replace` and `format`, not here.
///
/// # Where the assumption fails, stated rather than implied
///
/// It is **not** closed under operations that INTRODUCE bytes the value never
/// contained: `replace("a", "\'")`, `format()`, `chr(ord(c) + 1)`,
/// `sep.join(…)`, or a table lookup keyed by the value. A script can still emit
/// a quote that way, and the caller may well be the one who decides that it
/// appears — a table lookup is keyed by their value, and a `join`'s separator
/// is emitted once per piece of their value. So the property is narrower than
/// "no caller decides whether it appears", which is what this paragraph used to
/// claim while contradicting itself two clauses later:
///
/// > **every BYTE of the result was typed by the author or drawn from the safe
/// > alphabet.** The caller can choose which of the author's bytes appear, and
/// > how many times; they cannot introduce one the author never wrote.
///
/// That is the residue package S already documents and accepts
/// (`GeneratedSql::probe`): the author is authenticated, holds
/// `data_api_endpoint:manage`, and can type the injection directly into a
/// written statement — so an author who writes `join` with a `'` separator has
/// written an injection by hand, with extra steps. What this alphabet removes
/// is the other half — the one that mattered — where the CALLER supplies the
/// metacharacter and an honest script merely passes it along.
///
/// Non-ASCII is refused, and that is conservatism rather than a proof. No
/// Unicode case mapping produces a SQL metacharacter, so `\p{L}` would very
/// likely be safe too; "very likely" is not the standard for a rule whose whole
/// job is to hold under transformations nobody enumerated, and an author who
/// needs Vietnamese text in a parameter can still BIND it — see
/// [`ValidationError::StringParameterNeedsPattern`] for the recourse.
const SAFE_ALPHABET: [std::ops::RangeInclusive<char>; 4] =
    ['0'..='9', 'A'..='Z', '_'..='_', 'a'..='z'];

/// Describes [`SAFE_ALPHABET`] in an author-facing sentence.
const SAFE_ALPHABET_PROSE: &str = "ASCII letters, ASCII digits and `_`";

fn char_is_safe(c: char) -> bool {
    SAFE_ALPHABET.iter().any(|r| r.contains(&c))
}

/// Can `pattern` accept a value containing a character outside
/// [`SAFE_ALPHABET`]?
///
/// `Ok(())` means **no**: every string the anchored pattern matches is drawn
/// entirely from the safe alphabet. `Err(witness)` names one character it would
/// let through, so the author is told what to remove rather than "make it
/// safer".
///
/// # This answers a question about the ALPHABET, and nothing else
///
/// It is asked of `anchored(pattern)` and it is sound about that string. What
/// it cannot say is whether that string means what a reader thinks: `a)|(a` is
/// all safe characters and its anchored form still admits
/// `a' UNION SELECT …`, because the wrapper's own `^` and `$` ended up on a
/// different alternative from the author's text. That is [`compile_pattern`]'s
/// raw-parse requirement to refuse, and this function's guarantee is only worth
/// the sentence above BECAUSE that requirement exists — publish reaches it
/// through [`check_constraints`], serving through
/// [`crate::serving::params::coerce`], and
/// `a_pattern_that_escapes_the_anchors_is_refused_and_coerce_agrees` asserts
/// both halves over one pattern.
///
/// # Positive proof, refusing on doubt — the opposite of the P4.5 shape
///
/// This does not scan for dangerous constructs and pass what it fails to
/// recognise; it walks the parsed language and admits only what it can place
/// inside the whitelist. Every character the pattern can match has to be
/// accounted for by one of the arms below, and the arms are exhaustive over
/// `HirKind` with **no `_`** — so a `regex-syntax` upgrade that adds a way to
/// match a character fails the BUILD here rather than being waved through as
/// safe. That is the one property a byte scanner over emitted SQL could never
/// have.
///
/// # It parses, it never compiles
///
/// The compiled size limit is [`check_constraints`]' job
/// ([`MAX_PATTERN_PROGRAM_BYTES`]); this needs the syntax tree, not a matcher.
/// A pattern that does not parse is `Ok(())` here and is refused a few lines
/// later by the compile that [`check_constraints`] already performs — the order
/// is asserted by `an_unparseable_pattern_is_refused_as_invalid_rather_than_as_unsafe`, so a
/// broken pattern reads as broken rather than as unsafe.
fn pattern_confines_to_safe_alphabet(pattern: &str) -> std::result::Result<(), char> {
    let Ok(hir) = regex_syntax::parse(&anchored(pattern)) else {
        // Not this function's refusal to make: see the doc comment.
        return Ok(());
    };
    hir_confines_to_safe_alphabet(&hir)
}

/// [`pattern_confines_to_safe_alphabet`], over one node.
///
/// **No `_` arm, and `HirKind` is not `#[non_exhaustive]`, so that is enforced
/// by the compiler rather than by discipline.** A `regex-syntax` upgrade adding
/// a new way for a pattern to match a character breaks this build, which is the
/// only reading of "refuses what it cannot place" that survives a dependency
/// bump — the same rule `crate::serving::error::to_wire` keeps for the same
/// reason.
fn hir_confines_to_safe_alphabet(hir: &regex_syntax::hir::Hir) -> std::result::Result<(), char> {
    use regex_syntax::hir::{Class, HirKind};
    match hir.kind() {
        // Matches no character at all.
        HirKind::Empty => Ok(()),
        // `^`, `$`, `\b` and friends: zero-width, so they contribute nothing to
        // the value's alphabet.
        HirKind::Look(_) => Ok(()),
        // A literal run. In Unicode mode these bytes are UTF-8; a byte that is
        // not part of a valid encoding cannot be a safe ASCII character either,
        // so the lossy decode's replacement character is reported as the witness
        // and the pattern is refused.
        HirKind::Literal(lit) => match String::from_utf8_lossy(&lit.0)
            .chars()
            .find(|c| !char_is_safe(*c))
        {
            Some(c) => Err(c),
            None => Ok(()),
        },
        HirKind::Class(Class::Unicode(class)) => {
            for range in class.iter() {
                unsafe_char_in(range.start(), range.end())?;
            }
            Ok(())
        }
        HirKind::Class(Class::Bytes(class)) => {
            for range in class.iter() {
                unsafe_char_in(range.start() as char, range.end() as char)?;
            }
            Ok(())
        }
        HirKind::Repetition(rep) => hir_confines_to_safe_alphabet(&rep.sub),
        HirKind::Capture(cap) => hir_confines_to_safe_alphabet(&cap.sub),
        HirKind::Concat(subs) | HirKind::Alternation(subs) => {
            for sub in subs {
                hir_confines_to_safe_alphabet(sub)?;
            }
            Ok(())
        }
    }
}

/// One character of `start..=end` that is outside [`SAFE_ALPHABET`], if there is
/// one.
///
/// Constant time rather than a walk: a class range can be the whole of Unicode,
/// and iterating 1.1 million characters per range — for every range, of every
/// class, of every parameter — is a cost an author's pattern should not be able
/// to impose. A range is safe exactly when it fits inside ONE of the alphabet's
/// contiguous blocks, since the blocks are separated by unsafe characters; so
/// the first witness is either `start` itself or the character just past the
/// block `start` falls in.
fn unsafe_char_in(start: char, end: char) -> std::result::Result<(), char> {
    if !char_is_safe(start) {
        return Err(start);
    }
    let block = SAFE_ALPHABET
        .iter()
        .find(|r| r.contains(&start))
        .expect("`char_is_safe` said `start` is in some block");
    if end <= *block.end() {
        return Ok(());
    }
    // `block.end()` is `9`, `Z`, `_` or `z`, none of which is `char::MAX`, so
    // the successor exists; and it is `:`, `[`, `` ` `` or `{` — every one of
    // them unsafe, which is what makes the blocks separate blocks.
    let after = char::from_u32(*block.end() as u32 + 1)
        .expect("the alphabet's blocks all end well below `char::MAX`");
    Err(after)
}

/// **Package 5.** On an endpoint where a script BUILDS SQL, every `String`
/// parameter must carry a `pattern` whose language is inside
/// [`SAFE_ALPHABET`].
///
/// A no-op for an endpoint whose statements are all written SQL — which is
/// every endpoint that existed before package S, and every one where a caller's
/// value can only reach the engine as a bind.
///
/// # Why a declarative rule and not a better probe
///
/// [`GeneratedSql::probe`] runs the script with [`SENTINEL`] in each
/// parameter's place and refuses the definition if the sentinel comes back
/// inside the emitted template. That is a **positive** instrument: it can prove
/// a value DID reach the SQL text. It cannot prove one never will, and its two
/// documented bypasses are exactly that shape — `.upper()` transforms the
/// sentinel so the containment test finds nothing, and a script that
/// length-checks its input raises on the sentinel so the concatenation on the
/// accepting path is never observed. Both are pinned by tests, both are still
/// true of the probe, and **neither is fixed here**: making the probe scan the
/// emitted bytes harder is the P4.5 mistake the whole gate is shaped to avoid.
///
/// So the property is bought by construction instead. If the value cannot
/// contain a character that is dangerous in SQL text, then it does not matter
/// whether the script puts it in the text, transforms it first, or refuses the
/// probe's sentinel — every one of those paths ends with a bare word. See
/// [`SAFE_ALPHABET`] for the closure argument and for the transformations it
/// does NOT survive.
///
/// This is the same move [`check_enum_params`] already makes: an `Enum` must
/// carry a non-empty `allowed` because that is the only safe form for something
/// that may reach an identifier position. Package S opened the `String` case,
/// and this closes it the same way.
///
/// # Which parameters, and the honest cost of the answer
///
/// **Every `String` parameter on the endpoint, not the ones a script was
/// observed to use.** Deciding "this parameter reaches the text" is precisely
/// what the probe cannot do in general, so a rule built on that decision would
/// inherit the probe's blind spots — a parameter concatenated only on a branch
/// the representative values never take is invisible to `varies_with`, and
/// exempting it there would reopen the hole this rule exists to close. The
/// conservative set is the only one whose argument does not depend on the
/// instrument that was already shown to be incomplete.
///
/// The other five types need nothing, and that is a fact about
/// [`crate::serving::params::coerce`] rather than a judgement:
///
/// * `Number` is gated by `is_plain_decimal`, so `-?digits[.digits]`;
/// * `Boolean` is one of two words;
/// * `Date`/`Timestamp` are parsed by chrono and **re-formatted**, so the
///   caller's spelling never reaches the engine at all;
/// * `Enum` returns the AUTHOR's own `allowed` entry, never the caller's bytes,
///   and [`check_enum_params`] already requires the list.
///
/// The same four-way reading [`sentinel_probes`] makes to decide which
/// parameters carry a needle, applied to the same question one step later.
///
/// **What this refuses that is honest, stated plainly.** An endpoint with a
/// scripted statement and a free-text `String` parameter — a search term, a
/// customer name — is refused, even when the script only ever emits
/// `{{ q }}` and the value would have been bound. There is no pattern over the
/// safe alphabet that accepts a search phrase, so the author's recourse is the
/// second one the message offers: write that endpoint's statements as `sql`,
/// where `{{ q }}` binds and no script can reach the value. That is a real
/// capability lost — "generated SQL" and "free-text parameter" cannot be
/// combined on one endpoint — and it is the price of not resting the rule on a
/// detection nobody can make sound.
///
/// # `allowed` is NOT accepted as the constraint, and that is not an oversight
///
/// `coerce` reads `allowed` only for `ParamType::Enum`; a `String` carrying an
/// `allowed` list has it **silently ignored** on every served request. Accepting
/// it here would be a publish-time rule resting on an enforcement that does not
/// exist — the exact drift C1 moved the constraints inside `coerce` to make
/// impossible. An author who wants a closed set should declare the parameter as
/// an `Enum`, which is enforced.
///
/// # Where it runs, and why serving re-asks nothing
///
/// [`validate_endpoint`] and [`validate_preview`], the two entry points that
/// know whether a statement is a script. Serving needs no new check, and that is
/// the point of putting the constraint in `pattern` rather than anywhere else:
/// `coerce` already applies `pattern` to every `String` value on every request,
/// on the same path that produces the `Param` the script is handed. The rule the
/// author is refused by and the rule the caller is bounded by are one rule, so
/// they cannot drift.
fn check_script_parameter_constraints(
    statements: &[StatementSpec],
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    // The first scripted statement's name, and `None` when the endpoint has no
    // script at all — which is the no-op above.
    let scripted = statements
        .iter()
        .find(|s| matches!(s.source, StatementSource::Script(_)))
        .map(|s| s.name.clone());
    script_parameters_are_bounded(scripted, params)
}

/// [`check_script_parameter_constraints`], asked of a snapshot that is ALREADY
/// PUBLISHED — the serving path's half of the rule.
///
/// # Why serving asks at all, when publish's whole design was that it need not
///
/// Because publish only ever runs on a definition someone publishes. Every
/// scripted endpoint frozen before package 5 carries whatever parameters it was
/// published with, and an unconstrained `String` on one of them is exactly the
/// hole this package closes — indefinitely, since nothing forces a republish.
/// The snapshot carries `statements` and `params`, so the question is
/// answerable here, and an answer that is available is not a disclosure to
/// leave in a report.
///
/// **It refuses the request** ([`crate::serving::params::resolve`], before the
/// cache lookup, so a cached body cannot outlive the rule either). That breaks
/// such an endpoint until its author republishes it, which is the cost, and it
/// is the right side of the trade: the alternative is serving an injection on a
/// definition egret would no longer accept. Operators can find them ahead of
/// the first 500 — see the query in
/// `docs/en/specs/2026-08-30-dataapi-production-readiness-design.md` §5.1.
///
/// One rule and two adapters, differing only in where the scripted statement's
/// NAME comes from: `StatementSpec` at publish and preview, `StatementSnapshot`
/// here. The rule itself is [`script_parameters_are_bounded`] and is not
/// written twice, which is the same reason `coerce` owns the constraint in the
/// first place.
pub(crate) fn snapshot_script_parameters_are_bounded(
    statements: &[egret_dataapi_api::StatementSnapshot],
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    let scripted = statements
        .iter()
        .find(|s| matches!(s.source, StatementSource::Script(_)))
        .map(|s| s.name.clone());
    script_parameters_are_bounded(scripted, params)
}

/// The rule itself: `scripted` is the name of a statement that BUILDS its SQL,
/// or `None` for an endpoint that has none.
fn script_parameters_are_bounded(
    scripted: Option<String>,
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    let Some(statement) = scripted else {
        return Ok(());
    };
    for p in params {
        if p.ty != ParamType::String {
            continue;
        }
        let Some(pattern) = &p.pattern else {
            return Err(ValidationError::StringParameterNeedsPattern {
                parameter: p.name.clone(),
                statement,
                alphabet: SAFE_ALPHABET_PROSE,
            });
        };
        // The alphabet answer is sound about `anchored(pattern)` and says
        // nothing about whether that string means what it reads as — a pattern
        // that re-balances against those anchors passes it while matching
        // anything at all. The other half of the gate is `compile_pattern`'s
        // raw-parse requirement, and it is NOT restated here: publish reaches
        // it through `check_constraints` and serving through `coerce`, which
        // are the two places that already compile every pattern. A third copy
        // would be one no test could tell from the other two.
        if let Err(witness) = pattern_confines_to_safe_alphabet(pattern) {
            return Err(ValidationError::StringParameterPatternTooWide {
                parameter: p.name.clone(),
                witness,
                statement,
                alphabet: SAFE_ALPHABET_PROSE,
            });
        }
    }
    Ok(())
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
        // `allowed` is READ by `coerce` for exactly one type. On any other it
        // is the failure this table's comment describes, and it was missing
        // from the table that describes it: an author who writes `allowed` on
        // a `String` is declaring a closed set that no request is ever
        // measured against, and package 5's refusal already had to spend a
        // sentence telling them so.
        let listed = matches!(p.ty, ParamType::Enum);

        for (present, allowed_here, name) in [
            (p.minimum.is_some(), numeric, "minimum"),
            (p.maximum.is_some(), numeric, "maximum"),
            (p.min_length.is_some(), textual, "min_length"),
            (p.max_length.is_some(), textual, "max_length"),
            (p.pattern.is_some(), textual, "pattern"),
            (p.not_before.is_some(), temporal, "not_before"),
            (p.not_after.is_some(), temporal, "not_after"),
            (p.allowed.is_some(), listed, "allowed"),
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
                Err(PatternError::Compile(regex::Error::CompiledTooBig(_))) => {
                    return Err(ValidationError::PatternTooLarge {
                        param: p.name.clone(),
                    })
                }
                // Both remaining cases are "this text is not a pattern egret
                // can match with", and `PatternError`'s `Display` is what tells
                // them apart in the author's own terms: a pattern that does not
                // close its groups reads differently from one that closes them
                // against egret's own anchors.
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

/// The two script caps, in the one relation that must hold between them.
///
/// A statement's `script` decides that statement's STRUCTURE; a response
/// transform reshapes rows a statement already returned. So the generating one
/// may be larger and may not be smaller — and this is a `const` assertion
/// rather than a test because both sides are constants: it fails the build, at
/// the moment either number is edited, rather than on the next suite run.
const _: () = assert!(
    egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES >= MAX_TRANSFORM_SCRIPT_BYTES,
    "MAX_STATEMENT_SCRIPT_BYTES may not be smaller than MAX_TRANSFORM_SCRIPT_BYTES"
);

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
/// going to be refused anyway. [`check_transform_shape`] is 1-6 and
/// [`check_transform_script`] is 7:
///
/// 1. **script length** ([`MAX_TRANSFORM_SCRIPT_BYTES`]) — the cheapest
///    possible check, and it bounds how expensive every check after it can be;
/// 2. **`shape`/`naming`** — see [`ValidationError::TransformWithShapeOrNaming`];
/// 3. **empty `returns`** — see [`ValidationError::TransformReturnsEmpty`];
/// 4. **too many `returns`** — [`MAX_TRANSFORM_RETURNS`], the bound that makes
///    the two passes below, and `check_returns` on every served request, cost
///    something an operator can name;
/// 5. **blank or whitespace-padded `returns` names** — see
///    [`ValidationError::TransformReturnsBlankName`] and
///    [`ValidationError::TransformReturnsPaddedName`]. Before the duplicate
///    rule, so that by the time it runs a verbatim comparison and a trimmed one
///    are the same comparison;
/// 6. **duplicate `returns` names** — see
///    [`ValidationError::TransformReturnsDuplicateName`];
/// 7. **the script itself, parsed and evaluated** — last, because it is the
///    only check here that costs more than a comparison, and the only one a
///    caller may be told to skip ([`ScriptEval`]).
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
/// 16 times. Called once, the whole endpoint's transform cost is bounded by
/// ONE evaluation, matching every other endpoint-wide rule.
///
/// # Where that one evaluation is spent
///
/// **Not inside `EndpointStore::publish`'s transaction.** That was true until
/// this split and it was the more serious half of the same cost argument: 2 s
/// is a bound on ticks, not on time, and the worst case measured against it is
/// 28.8 s — of row lock and pool connection, held by author-supplied Starlark.
/// [`ScriptEval`] is what publish passes to say the evaluation has already
/// happened, outside the lock, over a script it then proves identical.
pub fn check_transform(output: &OutputSpec) -> std::result::Result<(), ValidationError> {
    check_transform_shape(output)?;
    check_transform_script(output)
}

/// Every [`check_transform`] rule that is a COMPARISON — no parse, no
/// evaluation, no interpreter.
///
/// Split out from [`check_transform_script`] for one reason, and it is a
/// property of `EndpointStore::publish` rather than of this function: publish
/// re-runs the whole validator against the row it has LOCKED, inside an open
/// transaction, and running a Starlark interpreter there holds the endpoint's
/// row lock and a pool connection for as long as the interpreter takes. See
/// [`ScriptEval`] for the measurement and for what publish does instead.
///
/// Everything here is a string length, an equality against a default, or a
/// pass over a list whose length this function is now what bounds
/// ([`MAX_TRANSFORM_RETURNS`]) — microseconds, and safe to repeat under a lock.
fn check_transform_shape(output: &OutputSpec) -> std::result::Result<(), ValidationError> {
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
    if transform.returns.len() > MAX_TRANSFORM_RETURNS {
        return Err(ValidationError::TransformReturnsTooMany {
            count: transform.returns.len(),
            limit: MAX_TRANSFORM_RETURNS,
        });
    }

    // BEFORE the duplicate rule, and that order is the whole point of this
    // block. `check_transform` used to compare `col.name` verbatim and treat
    // `""` as a name, so two unnamed rows published as
    // `TransformReturnsDuplicateName("")` — a refusal naming a column that does
    // not exist — while `"total"` and `"total "` were two different names to
    // publish and one name to every reader. Refusing blank and padded names
    // here means that by the time the duplicate rule runs, every name is
    // non-blank and already trimmed, so comparing verbatim and comparing
    // trimmed are the same comparison. That is what lets the editor's
    // `returnNameProblems` (`ResponseSection.tsx`) be a preview of this rule
    // rather than a second, differing one — see its comment.
    for col in &transform.returns {
        if col.name.trim().is_empty() {
            return Err(ValidationError::TransformReturnsBlankName);
        }
        if col.name.trim() != col.name {
            return Err(ValidationError::TransformReturnsPaddedName(
                col.name.clone(),
            ));
        }
    }

    let mut seen = std::collections::HashSet::with_capacity(transform.returns.len());
    for col in &transform.returns {
        if !seen.insert(col.name.as_str()) {
            return Err(ValidationError::TransformReturnsDuplicateName(
                col.name.clone(),
            ));
        }
    }

    Ok(())
}

/// The one [`check_transform`] rule that RUNS a Starlark interpreter.
///
/// Split out so that the one caller which must not run it — publish's
/// in-transaction re-validation — cannot do so by accident. See [`ScriptEval`].
fn check_transform_script(output: &OutputSpec) -> std::result::Result<(), ValidationError> {
    let Some(transform) = &output.transform else {
        return Ok(());
    };
    crate::transform::eval_probe(&transform.script)
        .map_err(|e| ValidationError::TransformScriptInvalid(e.to_string()))
}

/// Ceiling on how many columns `output.transform.returns` may declare.
///
/// # Why there is one at all
///
/// It was the only author-controlled list in this module with no bound —
/// `MAX_STATEMENTS` is 16, `MAX_DECLARED_PARAMS` is 32,
/// [`MAX_TRANSFORM_SCRIPT_BYTES`] is 8192, and `returns` was unbounded. It is
/// not a publish-time cost that makes that matter, it is a SERVING one:
/// [`crate::transform::check_returns`] runs against **every response of every
/// request**, and the undeclared-field half of it is a scan of `returns` per
/// returned field.
///
/// # Why 64
///
/// Twice `MAX_DECLARED_PARAMS`, on the argument that a response object may
/// reasonably carry more names than a request's query string does, and far
/// above anything the shape this feature exists for produces — a transform
/// reshapes a response, and the 8 KiB script cap already says an author writing
/// 64 output fields by hand has left that shape behind. It is a backstop, not a
/// design target: an honest transform never approaches it.
pub const MAX_TRANSFORM_RETURNS: usize = 64;

/// May [`validate_endpoint`] evaluate `output.transform.script`?
///
/// # The hazard this exists to remove, measured
///
/// `EndpointStore::publish` opens a transaction, takes a row lock with
/// `SELECT ... FOR UPDATE`, and then re-runs [`validate_endpoint`] against the
/// locked row. Until this enum existed, that re-validation called
/// [`check_transform`], which parses and evaluates the script's module top
/// level through a real Starlark interpreter.
///
/// [`crate::transform::MAX_WALL_CLOCK`] is 2 s, and it is **not a latency
/// guarantee** — `run_transform`'s own doc comment records a script measured at
/// **28.8 s** against that 2 s deadline, because the deadline is checked at
/// 1000-tick granularity and a script whose every tick is expensive overruns
/// it. So an authenticated author could hold an endpoint's row lock *and* a
/// database pool connection, on an async worker, for tens of seconds per
/// publish, from eight kilobytes of Starlark. That is strictly worse than the
/// preview hazard Task 8 moved off the worker.
///
/// # What publish does instead
///
/// It evaluates the script **before** `begin()`, against the DRAFT, and then
/// relies on the identity check it already performs: the transaction refuses
/// with `Conflict` unless `current.output == draft.output`. So the script that
/// was evaluated outside the lock is byte-identical to the script being frozen
/// — the guarantee is unchanged, only where it is paid for is.
///
/// The evaluation count is unchanged too: publish evaluated the script once
/// inside the transaction and now evaluates it once outside it.
///
/// # Package S widened this from the transform to every script an endpoint has
///
/// A statement's SQL may now be built by a `build_sql` script
/// ([`crate::transform::build_sql`]), and that evaluation is under exactly the
/// same budgets, so it carries exactly the same hazard: generating inside the
/// transaction would put author-supplied Starlark under the endpoint's row lock,
/// which is the thing this enum exists to prevent.
///
/// So the skip variant now **carries what was generated**. That is the point of
/// the payload: publish cannot ask to skip the evaluation without handing over
/// the text the skipped evaluation produced, and there is no spelling of "skip
/// it and generate anyway". `pin_no_starlark_evaluation_inside_publishs_transaction`
/// reads publish's own body for the same property; this makes it structural
/// rather than a text scan with a documented blind spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptEval<'a> {
    /// Evaluate the endpoint's scripts as part of this call — the transform's
    /// module top level AND every statement's `build_sql`. Every caller that
    /// holds no lock and no open transaction: `/validate`, `/preview`, and the
    /// tests.
    Here,
    /// Do **not** evaluate anything; the caller has already run
    /// [`check_transform_script`] and [`GeneratedSql::build`] over scripts it
    /// then proved identical to the ones being validated.
    /// `EndpointStore::publish` and nothing else.
    ///
    /// The structural rules ([`check_transform_shape`]) still run: they are
    /// comparisons, they cost microseconds, and running them against the locked
    /// row is what makes the re-validation a re-validation. So does every scan
    /// over `generated` — the whole point is that the locked row's statements go
    /// through the same rules, on text the caller generated a moment earlier and
    /// outside the lock.
    AlreadyRunBeforeTheLock {
        /// What generating the DRAFT's statements produced. Publish's identity
        /// check (`current.statements == draft.statements` and
        /// `current.params == draft.params`) is what makes that the same text
        /// generating the locked row would produce: generation reads those two
        /// and nothing else, and Starlark under [`crate::transform`]'s globals
        /// has no clock, no randomness and no I/O.
        generated: &'a GeneratedSql,
    },
}

/// Every statement's SQL text, with each scripted statement's `build_sql`
/// evaluated exactly once.
///
/// # Why text statements are in here too
///
/// A lookup that had to ask which kind of statement it was holding is a lookup
/// that can answer the two kinds differently, and "nothing downstream can tell
/// whether the text was typed or generated" is the property this whole package
/// rests on. So [`GeneratedSql::build`] copies a written statement's SQL in
/// beside a generated one, and [`statement_sql`] is one lookup by name with no
/// branch in it. The copy is at most [`egret_dataapi_api::MAX_STATEMENTS`]
/// strings on the publish path.
///
/// # The bound comes first, and this type is where that is enforced
///
/// [`build`](GeneratedSql::build) runs [`egret_dataapi_api::validate_statements`]
/// **before** it evaluates anything, so `MAX_STATEMENTS` bounds the number of
/// Starlark evaluations by construction rather than by the order of two
/// statements in a caller. Package S Task 1 measured what that ordering buys:
/// `the_statement_bound_is_checked_before_anything_renders` refuses a
/// 256-statement list in under 500 ms, and without the bound ahead of
/// generation those 256 statements are 256 evaluations of an author's script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedSql(BTreeMap<String, GeneratedStatement>);

/// One statement's generated text, and what the probes learnt about it.
///
/// # Why `varies_with` rides along with the text rather than being recomputed
///
/// Because recomputing it means evaluating the script again, and the one place
/// it would have to be recomputed is the one place no Starlark may run:
/// `EndpointStore::publish` re-validates the row it LOCKED, and
/// [`ScriptEval::AlreadyRunBeforeTheLock`] exists precisely so that
/// re-validation performs no evaluation. A `varies_with` the locked re-run had
/// to derive itself would put every probe under the row lock — or, worse, come
/// back empty there and make publish refuse a branching script with
/// [`ValidationError::UnreferencedParameter`] after `validate` had accepted it.
///
/// Carrying it in the payload publish already hands across the lock costs no new
/// field on [`ScriptEval`] and no new soundness argument: generation and probing
/// read `statements` and `params` and nothing else, and those two are exactly
/// what publish's identity check proves unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GeneratedStatement {
    /// The template the script returned — or the author's own SQL, copied.
    ///
    /// This is the one the snapshot freezes and the one every scan that names a
    /// single statement reads. The others are in `alternates`.
    sql: String,
    /// The OTHER distinct templates the same script emitted under
    /// [`GeneratedSql::probe`]'s substitutions, in first-seen order and never
    /// containing `sql`. Always empty for a written statement.
    ///
    /// # Why the contract cannot be frozen from `sql` alone (spec §3)
    ///
    /// A script that returns a different projection per branch has one output
    /// schema per branch, exactly as a `{% if %}` definition does — and M1
    /// already learnt what freezing one branch of those costs: every call that
    /// took a different branch became a `503 schema_drift`, including the call
    /// with no query string, which is the one the generated OpenAPI advertises.
    /// M1's answer is to describe every branch and freeze the UNION; the only
    /// thing missing for a generated statement was the SET of texts to run that
    /// answer over, because [`branch_renderings`] takes a `&str` and a script
    /// produces more than one.
    ///
    /// # These cost nothing to collect, and that is not a coincidence
    ///
    /// They are read off the runs Task 4's sentinel gate already performs —
    /// the same fold spec §2.4 makes for `varies_with`, from the same
    /// `2 × params` evaluations. **No new evaluation of an author's script.**
    /// What they are NOT is M1's probe assignments: `render::probe_assignments`
    /// derives its assignments FROM the SQL by harvesting the literals a
    /// definition compares against, and when `build_sql` runs that SQL is
    /// precisely what does not exist yet (spec §2.3, corrected in Task 4).
    ///
    /// # What they DO cost, and the budget that bounds it
    ///
    /// Each one is a template [`validate`] renders and describes like any
    /// other, so a statement's render and engine cost is now multiplied by how
    /// many there are. That product is bounded by
    /// [`egret_dataapi_api::MAX_STATEMENTS`], which
    /// [`GeneratedSql::build`] spends on TEXTS rather than on statements —
    /// measured, and explained, on
    /// [`ValidationError::TooManyGeneratedTemplates`].
    ///
    /// # The gap, stated at its real size
    ///
    /// The set is bounded — two substitutions per declared parameter — so a
    /// script can branch on more combinations than it covers, exactly as a
    /// definition can branch on more than `render::MAX_PROBES` reaches. A
    /// branch neither set visits is not described, and its columns are not in
    /// the frozen union. The backstop is unchanged and is not this function:
    /// `crate::serving::router::shape_rows` refuses a column nobody declared.
    alternates: Vec<String>,
    /// The declared parameters whose value **changed this statement's emitted
    /// template**, as [`GeneratedSql::probe`] measured it. Always empty for a
    /// written statement: its text is not a function of anything.
    ///
    /// This is half of spec §2.4's behavioural reading of "referenced"; the
    /// other half is `{{ x }}` in the emitted text, which
    /// [`crate::render::context_names`] answers for written and generated SQL
    /// alike.
    varies_with: Vec<String>,
}

/// The value a probe puts in a parameter's place to see whether it reaches the
/// SQL text.
///
/// Its shape is the gate, so it is asserted rather than assumed
/// (`the_sentinel_cannot_collide_with_an_ordinary_value`): long enough that no
/// author writes it by accident, and made only of bytes that survive being a
/// parameter value unchanged — an identifier-safe run, so nothing between here
/// and the emitted template can quote, escape or normalise it into something the
/// containment test would miss.
const SENTINEL: &str = "egret_sentinel_9f4c2ba7e1d05836";

/// What one probe substitutes for one parameter, and what — if anything — that
/// makes it possible to look for in the emitted template.
///
/// One struct rather than two functions because the value and the needle must
/// agree: a needle for a value that was never substituted would be a gate that
/// looks for nothing, and it would look exactly like a gate that works.
struct SentinelProbe {
    /// What the script sees in this parameter's place — `None` for the probe
    /// that substitutes an absent parameter.
    ///
    /// A [`Param`] rather than a `serde_json::Value` so that these substitute
    /// through [`crate::transform::GenerationParams::with`], the one door into a
    /// generating script's `params` dict. A probe that could carry a list would
    /// be a probe showing the author a shape serving cannot produce.
    value: Option<Param>,
    /// The text whose presence in the emitted template is the finding, when
    /// there is one. `None` for a probe that varies the value but has nothing
    /// improbable to look for — see [`sentinel_probes`].
    needle: Option<&'static str>,
}

/// The values one parameter is probed with, in order.
///
/// **Two per parameter, and the second one is not about the sentinel gate at
/// all.** The first substitutes a distinctive value of the parameter's own type;
/// the second substitutes `None`, which is what a caller who supplies nothing
/// leaves behind. The second exists for spec §2.4: without it a script whose
/// only use of a parameter is `if params["region"]:` never varies — `"x"` and
/// the sentinel are both truthy — and the endpoint would be refused as
/// `UnreferencedParameter` for a script that plainly reads it. That is the
/// direction this whole task must not fail in.
///
/// It is the same pair the render probes already use: [`representative`] renders
/// each optional parameter present and then [`ParamValues::insert_absent`],
/// because "declared but not supplied" is a value a definition can branch on.
///
/// # The type is preserved, and that is not politeness
///
/// A string in a number parameter's place makes `params["n"] * 2` fail, and a
/// probe that fails is inconclusive rather than a finding — so type-punning
/// every parameter would quietly stop probing every script that does arithmetic.
/// For the same reason the temporal probe values are real dates rather than the
/// sentinel: a script that splits `params["day"]` on `-` should VARY, not raise.
///
/// # Which parameters carry a needle, and the one property that decides it
///
/// **A needle is for a parameter whose bytes reach [`Param`] VERBATIM, and
/// `String` is the only such type.** `crate::serving::params::coerce` is where
/// that is decided, and it is worth reading rather than assuming:
///
/// * `String` → `Param::Str(text.to_string())`. The caller's bytes, unchanged.
///   An author's `pattern`/`max_length` may narrow them, but nothing requires
///   one, so the default is a free run of bytes — the only kind that can open a
///   literal, close one, or form an identifier.
/// * `Number` → gated by `is_plain_decimal` first, so the value is
///   `-?digits[.digits]` and nothing else.
/// * `Boolean` → `Param::Bool`, one of two words.
/// * `Date` / `Timestamp` → parsed by chrono and **re-formatted**
///   (`normalize_temporal_bound`), so what survives is `%Y-%m-%d` /
///   `%Y-%m-%d %H:%M:%S%.f`. A caller's spelling does not reach the engine at
///   all; a normalisation of it does.
///
/// So the last three are exactly as constrained as each other, and none of them
/// can carry a quote, a backslash or an identifier byte. **They all go
/// needle-less, and refusing any one of them while permitting the others would
/// be an inconsistency, not caution.** `Date` and `Timestamp` did carry a needle
/// in this task's first draft, on the stated ground that a caller writes their
/// bytes freely — which `coerce` shows is false. Recorded because the code
/// disagreeing with its own comment is how this programme has lost time before.
///
/// One consequence rather than the reason, and it is the one that shows the cost
/// of getting this wrong: Trino will not bind `LIMIT`, so
/// `"LIMIT " + str(params["n"])` is one of the honest reasons to generate SQL at
/// all, and `"… >= DATE '" + params["day"] + "'"` is the same script one type
/// along.
///
/// `Enum` keeps a needle, and NOT because its bytes are the caller's — `coerce`
/// returns the author's own `allowed` entry (`Param::Str(a.clone())`), the same
/// provenance rule `ident_filter` enforces. It keeps one because spec §2.2
/// blesses exactly two spellings for choosing an identifier from a parameter —
/// `TABLES[params["grain"]]` and `{{ grain | ident }}` — and both survive this
/// gate untouched. Refusing the third spelling costs an author nothing and keeps
/// `allowed`, which is author text no rule scans, out of the SQL text.
fn sentinel_probes(p: &ParamSpec) -> [SentinelProbe; 2] {
    let first = match p.ty {
        // The one type whose bytes are the caller's, verbatim.
        ParamType::String => SentinelProbe {
            value: Some(Param::Str(SENTINEL.to_owned())),
            needle: Some(SENTINEL),
        },
        // The author's own string, kept out of the SQL text so that §2.2's two
        // spellings stay the way an identifier is chosen. See above.
        ParamType::Enum => SentinelProbe {
            value: Some(Param::Str(SENTINEL.to_owned())),
            needle: Some(SENTINEL),
        },
        // `representative_value` is `true`, so `false` is the other member of a
        // closed two-value space — the same reasoning `ParamValues::candidates`
        // applies to a boolean in the render probes.
        ParamType::Boolean => SentinelProbe {
            value: Some(Param::Bool(false)),
            needle: None,
        },
        // `representative_value` is `1`. Zero varies both the VALUE and its
        // truthiness, which is the branch a number is most often read for
        // (`if params["min_total"]:`).
        ParamType::Number => SentinelProbe {
            value: Some(Param::I64(0)),
            needle: None,
        },
        // `representative_value` is the epoch, so these are simply a different
        // valid instant. Valid, not a sentinel: with no needle to look for, the
        // value's only job is to VARY, and a real date varies a script that
        // parses or compares it instead of making it raise.
        ParamType::Date => SentinelProbe {
            value: Some(Param::Date("2038-01-19".to_owned())),
            needle: None,
        },
        ParamType::Timestamp => SentinelProbe {
            value: Some(Param::Timestamp("2038-01-19 03:14:07".to_owned())),
            needle: None,
        },
    };
    [
        first,
        SentinelProbe {
            value: None,
            needle: None,
        },
    ]
}

impl GeneratedSql {
    /// Generate every scripted statement's SQL, once.
    ///
    /// `params` is what the scripts see, and only that — see
    /// [`generation_params`] for the values and for why the shape decision
    /// cannot be one of the inputs.
    fn build(
        statements: &[StatementSpec],
        params: &[ParamSpec],
    ) -> std::result::Result<Self, ValidationError> {
        // The whole endpoint is `statements`, so nothing has been spent yet.
        Self::build_after(statements, params, 0)
    }

    /// [`build`](Self::build), for a caller holding only PART of the endpoint.
    ///
    /// # The one caller, and the divergence this parameter closes
    ///
    /// [`validate_preview`] validates the statement on screen and is told about
    /// its siblings only as text. `MAX_STATEMENTS` bounds the templates an
    /// ENDPOINT renders, and [`build`](Self::build) counts them across the
    /// slice it is handed — so a preview passing a slice of ONE started the
    /// count at zero and accepted a draft publish refuses: ten siblings beside a
    /// script emitting eight templates is eighteen for a publish and eight for a
    /// preview. Green here, refused there, which is the single defect that route
    /// exists to invert.
    ///
    /// `already` is what the caller has already spent — one template per
    /// sibling, because a sibling is one SQL text. It goes into the SAME counter
    /// rather than into a second comparison beside it, so the rule cannot be
    /// weakened in one place and not the other.
    ///
    /// # The refusal is the same SET, not the same number
    ///
    /// [`build`](Self::build) errors at the first statement that takes the
    /// running total over, in published order, so the `templates` it reports
    /// depends on where the offending statement sits. Preview spends its
    /// siblings first and then the one statement it holds. Both refuse exactly
    /// the drafts whose total exceeds the limit; only the number in the message
    /// can differ, and no caller branches on it.
    fn build_after(
        statements: &[StatementSpec],
        params: &[ParamSpec],
        already: usize,
    ) -> std::result::Result<Self, ValidationError> {
        // FIRST. See the type's doc comment: this is the bound on how many
        // evaluations the loop below can perform, and a caller that reached
        // here through `validate_endpoint` has already paid for it (the check is
        // a comparison over a list whose length is what it just bounded).
        egret_dataapi_api::validate_statements(statements)?;
        // The SECOND bound, and it is here for the same reason the first one is:
        // [`GeneratedSql::probe`] runs two evaluations per declared parameter,
        // so an unbounded `params` is an unbounded number of evaluations of an
        // author's script exactly as an unbounded `statements` would be.
        // `check_definition_scans` asks this again a few lines later in
        // `validate_endpoint` — it is a comparison over a `Vec`, and paying for
        // it twice is what lets the bound sit where the work is bounded rather
        // than where the scans happen to start.
        if params.len() > MAX_DECLARED_PARAMS {
            return Err(ValidationError::TooManyParameters {
                declared: params.len(),
                limit: MAX_DECLARED_PARAMS,
            });
        }
        let generation_params = generation_params(params);
        let mut out = BTreeMap::new();
        // The bound package S Task 5 needs, and it is `MAX_STATEMENTS` rather
        // than a new number: that limit's own measurement is of the TEXTS
        // publish renders, and until a script could branch, one statement was
        // one text. See `ValidationError::TooManyGeneratedTemplates` for the
        // measurement of what happens when the two costs multiply instead.
        //
        // A written statement is counted too, and must be: sixteen of them is
        // exactly the allowance, so an endpoint with no script is unaffected and
        // one with a script spends from the same pot. A count that skipped them
        // would be a second rule about which kind of statement this is, which is
        // the one thing this package may not introduce.
        let mut templates = already;
        for statement in statements {
            let generated = match statement.source.script() {
                None => GeneratedStatement {
                    sql: statement
                        .source
                        .sql()
                        // `StatementSource` is a two-case enum with "neither"
                        // unrepresentable (Task 1), so `script()` returning `None`
                        // means `sql()` returns `Some`.
                        .expect("a statement that is not a script carries SQL")
                        .to_owned(),
                    // A written statement's text is not a function of
                    // anything, so nothing can vary it — there is no second
                    // template and nothing can change it. The union in
                    // `validate` is then unchanged for an endpoint with no
                    // scripts, which is what makes this task invisible to every
                    // definition that has none.
                    alternates: Vec::new(),
                    varies_with: Vec::new(),
                },
                Some(script) => {
                    let sql = crate::transform::build_sql(script, &generation_params).map_err(
                        |error| ValidationError::StatementScriptFailed {
                            statement: statement.name.clone(),
                            error: error.to_string(),
                        },
                    )?;
                    Self::probe(&statement.name, script, params, &generation_params, sql)?
                }
            };
            templates += 1 + generated.alternates.len();
            if templates > egret_dataapi_api::MAX_STATEMENTS {
                return Err(ValidationError::TooManyGeneratedTemplates {
                    templates,
                    limit: egret_dataapi_api::MAX_STATEMENTS,
                });
            }
            out.insert(statement.name.clone(), generated);
        }
        Ok(Self(out))
    }

    /// Run `script` again with a sentinel in each parameter's place, and answer
    /// three questions from the same runs.
    ///
    /// **The gate (spec §2.3):** if the sentinel comes back inside the emitted
    /// template, the script put the caller's value into the SQL TEXT rather than
    /// leaving a placeholder for the renderer to bind, and the statement is
    /// refused by [`ValidationError::ParameterValueInSqlText`].
    ///
    /// **The contract (spec §3, package S Task 5):** every DISTINCT template
    /// these runs emitted is kept, and `validate` describes all of them and
    /// freezes their union. Nothing about the runs changes to collect them —
    /// the emitted text was already in hand for the two questions below, and
    /// throwing it away was what left a branching script's contract frozen from
    /// whichever branch the representative values happened to take. See
    /// [`GeneratedStatement::alternates`] for what that costs and what it does
    /// not cover.
    ///
    /// **The measurement (spec §2.4):** every parameter whose substitution
    /// changed the emitted template is returned. `validate_endpoint` adds those
    /// names to the `{{ x }}` names [`crate::render::context_names`] reads out of
    /// the generated text, and the union is what
    /// [`check_every_parameter_is_read`] is asked about. Without it, a script
    /// that branches on a parameter and emits no `{{ }}` for it — the case this
    /// package exists to enable — would be refused as
    /// [`ValidationError::UnreferencedParameter`].
    ///
    /// # What this does NOT prove, stated because a limit that reads like a
    /// guarantee is worse than a documented absence
    ///
    /// There are TWO ways past this, and both are ordinary Starlark:
    ///
    /// 1. **Transform the value first.** `params["region"].upper()` — or a hash,
    ///    a slice, a `replace` — emits something that is not the sentinel, and
    ///    the containment test finds nothing. Pinned by
    ///    `a_script_that_transforms_the_value_first_defeats_the_probe`.
    /// 2. **Refuse the sentinel before concatenating.** A script that
    ///    length-checks or format-checks its input RAISES on a 31-character
    ///    sentinel, the probe is inconclusive (see below), and the
    ///    concatenation on the accepting path is never observed. Pinned by
    ///    `a_script_that_rejects_the_sentinel_before_concatenating_also_defeats_the_probe`.
    ///
    /// The second is a direct consequence of treating a failed probe as
    /// inconclusive, which is the right call for the reasons below — so it is a
    /// cost of that decision, not an oversight in it, and it is written down
    /// here rather than left for a reader to discover.
    ///
    /// **TWO is still the count after package S Task 8.5 added a third way for
    /// an evaluation to fail**, and that is worth one sentence rather than a
    /// re-derivation. A script that concatenates the caller's value AND pads the
    /// result past [`crate::transform::MAX_GENERATED_SQL_BYTES`] never reaches
    /// the containment test below — the emission is refused before there is a
    /// string to search. It escapes `ParameterValueInSqlText` and it escapes
    /// nothing else: the length arm at the top of the loop RETURNS, so the
    /// definition is refused either way and no such statement publishes. A
    /// length refusal is the one `Err` that closes a door instead of opening
    /// one.
    ///
    /// **The probe catches an author who creates an injection hole by accident.
    /// It does not defend against an author who sets out to build one**, and it
    /// is not a barrier that could be made into one by looking harder at the
    /// emitted bytes: that is the P4.5 lesson (five independent reviews, five
    /// different bypasses of one byte scanner guarding a template language).
    ///
    /// That residue is acceptable for a reason that has nothing to do with this
    /// function's strength: the author is authenticated, holds
    /// `data_api_endpoint:manage`, and is already trusted to write the SQL by
    /// hand.
    ///
    /// **What used to be written here — "against the caller nothing changed,
    /// values are binds" — was false, and spec §2.3 now says so in an
    /// amendment.** A script that concatenates puts the caller's value into the
    /// SQL TEXT; that is the point of the two escape routes above. Against the
    /// caller what changed is package 5's declarative rule, and it is a bound
    /// on the value rather than a promise about where it lands — see the
    /// section below.
    ///
    /// # Package 5 closed the CALLER's half of both routes, and did not touch
    /// this function
    ///
    /// Both escape routes above are still exactly as described, and the two
    /// tests that pin them still assert this function accepts both scripts —
    /// read their doc comments before concluding otherwise. What changed is
    /// that a definition now has to pass a second, DECLARATIVE rule:
    /// [`check_script_parameter_constraints`] refuses a `String` parameter with
    /// no safe `pattern` on any endpoint a script builds SQL for. So on an
    /// endpoint that publishes, the value that a script may transform or
    /// concatenate is drawn from an alphabet in which none of the routes above
    /// buys the caller anything — see [`SAFE_ALPHABET`] for the closure
    /// argument and its limits.
    ///
    /// **The division of labour is the point.** This function proves a value DID
    /// reach the text and names the parameter, which no declarative rule can do;
    /// the declarative rule bounds the values, which no probe can do. Neither
    /// was made to do the other's job, and in particular nothing here scans the
    /// emitted bytes any harder than it did.
    ///
    /// It is bounded in a second direction too, and it is the same bound M1
    /// already lives with: a parameter read only inside a branch these
    /// substitutions never reach is invisible here — it neither trips the gate
    /// nor counts as referenced.
    ///
    /// # A probe that FAILS is inconclusive, and inconclusive means "referenced"
    ///
    /// **With exactly one exception, and the exception is about evidence.**
    /// [`crate::transform::TransformError::GeneratedSqlTooLarge`] is refused
    /// outright rather than read as inconclusive, because it is not a failure to
    /// produce an answer: the script ran to completion, returned a template, and
    /// this process measured it. Everything below argues that a script which
    /// RAISES under a synthetic value has proved nothing about the values it
    /// will really be sent — true, and it does not transfer to a branch publish
    /// generated and found too long. Swallowing that one published an endpoint
    /// green and left the first caller who reached the branch with a 500 for a
    /// defect publish was holding in its hand.
    ///
    /// `params["region"].split("-")[1]` raises under a sentinel and
    /// `params["n"] + 1` raises under `None`. The render probes already treat a
    /// failure as inconclusive rather than a finding
    /// (`ParamValues::probe_replace`), and this does the same — with the one
    /// addition that a script which CRASHES on a parameter's value has
    /// demonstrated that it reads it, so the name counts as referenced.
    ///
    /// **Only one of those halves is "safe" in the same sense.** Reading a
    /// failure as *referenced* leans safe for the AUTHOR — it cannot manufacture
    /// an `UnreferencedParameter` out of a probe that proved nothing. Declining
    /// to make a finding out of it is **permissive for the GATE**, and it is
    /// escape route 2 above. The alternative — refusing a definition whose
    /// script raises under a synthetic value it will never actually be sent —
    /// would refuse honest scripts wholesale, which is the worse failure.
    ///
    /// # The cost, and the two bounds that make it a number
    ///
    /// [`sentinel_probes`] is two values per parameter, so one scripted
    /// statement costs `2 × params.len()` evaluations — at most 64, since
    /// [`build`](GeneratedSql::build) refuses more than [`MAX_DECLARED_PARAMS`]
    /// declared parameters before it reaches here. Across an endpoint that is
    /// `MAX_STATEMENTS` times that, checked first, on the publish path only.
    /// **`crate::render::MAX_PROBES` is untouched** — it bounds renders of one
    /// definition and this bounds evaluations of one script; the two happen to
    /// arrive at the same 64.
    ///
    /// Returning the whole [`GeneratedStatement`] rather than the two lists,
    /// because the three answers come off the same runs and one of them is the
    /// baseline the caller already holds: a signature that handed back
    /// `alternates` on its own could be called with a `sql` the alternates were
    /// not measured against, and that mismatch is not observable in any single
    /// test — the union would simply be frozen from the wrong text.
    fn probe(
        name: &str,
        script: &str,
        params: &[ParamSpec],
        base: &crate::transform::GenerationParams,
        baseline: String,
    ) -> std::result::Result<GeneratedStatement, ValidationError> {
        let mut varies_with = Vec::new();
        let mut alternates: Vec<String> = Vec::new();
        for p in params {
            let mut varied = false;
            for probe in sentinel_probes(p) {
                let context = base.with(&p.name, probe.value.as_ref());
                match crate::transform::build_sql(script, &context) {
                    // The ONE failure that is neither inconclusive nor a
                    // reading: the script ran to completion and handed back a
                    // template this process MEASURED against
                    // `transform::MAX_GENERATED_SQL_BYTES`. Publish holds the
                    // evidence, so publish answers with it rather than dropping
                    // the alternate and leaving a real caller on this branch to
                    // meet the same cap as a 500. See the section on
                    // inconclusive failures in this function's doc.
                    Err(e @ crate::transform::TransformError::GeneratedSqlTooLarge { .. }) => {
                        return Err(ValidationError::StatementScriptFailed {
                            statement: name.to_owned(),
                            error: format!(
                                "on the branch a probe value for parameter \
                                 `{}` reaches: {e}",
                                p.name
                            ),
                        })
                    }
                    // Inconclusive as a gate, conclusive as a reading: the
                    // script looked at the value and could not cope with it.
                    Err(_) => varied = true,
                    Ok(emitted) => {
                        if probe.needle.is_some_and(|needle| emitted.contains(needle)) {
                            return Err(ValidationError::ParameterValueInSqlText {
                                statement: name.to_owned(),
                                parameter: p.name.clone(),
                            });
                        }
                        if emitted != baseline {
                            varied = true;
                            // Distinct, and by TEXT rather than by which
                            // parameter produced it: two parameters that turn
                            // the same block off emit the same template and it
                            // has one contract, one render and one describe.
                            // `branch_renderings` deduplicates its renderings
                            // the same way and for the same reason.
                            if !alternates.contains(&emitted) {
                                alternates.push(emitted);
                            }
                        }
                    }
                }
            }
            if varied {
                varies_with.push(p.name.clone());
            }
        }
        Ok(GeneratedStatement {
            sql: baseline,
            alternates,
            varies_with,
        })
    }

    /// The text generated for `name`, if this set was generated for a statement
    /// list containing it.
    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|g| g.sql.as_str())
    }

    /// Every template `name` may emit, [`get`](GeneratedSql::get)'s first.
    ///
    /// One statement's whole contract comes from this list: `validate`
    /// describes each of them and freezes the union. For a written statement it
    /// is a one-element list holding exactly what `get` returns, so nothing
    /// downstream can tell the two apart by asking a different question — the
    /// property the whole package rests on.
    fn templates(&self, name: &str) -> Option<Vec<&str>> {
        self.0.get(name).map(|g| {
            std::iter::once(g.sql.as_str())
                .chain(g.alternates.iter().map(String::as_str))
                .collect()
        })
    }

    /// Every parameter that changed SOME statement's emitted template, across
    /// the whole endpoint.
    ///
    /// The union, for the reason [`check_every_parameter_is_read`] records: a
    /// parameter only the rows query branches on is not an unused query
    /// parameter (spec §4.2), and asking per statement would refuse it.
    fn varying_params(&self) -> impl Iterator<Item = &str> {
        self.0
            .values()
            .flat_map(|g| g.varies_with.iter().map(String::as_str))
    }
}

/// The `params` dict a `build_sql` script is handed at publish time.
///
/// Every declared parameter at its [`representative_value`] — the same values
/// [`probe_params`] hands a transform probe, and for the same reason: a script
/// that indexes `params["region"]` must find something there, and a truthy
/// stand-in is what exercises a branch rather than skipping it.
///
/// # One difference from [`probe_params`], and serving makes the SAME one
///
/// [`probe_params`] wraps a parameter any statement ITERATES in a one-element
/// list, asking [`IteratedParams`] which those are. Generation cannot: the
/// iterated set is computed **from the SQL** ([`IteratedParams::across`]), and
/// here the SQL is what is being produced. So a generating script sees every
/// parameter as a scalar.
///
/// That used to be the end of the paragraph, and it was where package S's one
/// Critical lived. `serving::params::resolve` builds its script dict from the
/// frozen `iterated` set and from the caller's arity, so a parameter some
/// statement loops over arrived at the script as a one-element LIST — the
/// opposite of what this function shows the author. `params["region"] == "ALL"`
/// was true here and false for every caller; the script took the other branch
/// and the endpoint answered 200 with the wrong rows.
///
/// **It is not a difference any more, and not because this side moved.**
/// Serving follows this one: [`crate::transform::GenerationParams`] is the only
/// door into a generating script's `params`, its `insert` takes at most one
/// [`Param`], and `Param` has no list case — so neither side can express the
/// disagreement. Read that type for what the choice costs and why the reverse
/// (making this function follow serving) is not available: it would need
/// `iterated`, which is derived from the very SQL this call is producing.
///
/// # What this is not
///
/// It is one assignment, so it exercises one branch of a branching script — the
/// BASELINE branch. [`GeneratedSql::probe`] (Task 4) runs the script again from
/// this same base with a sentinel and then `None` in each parameter's place,
/// which is what refuses a script concatenating a caller's value into the SQL
/// text and what measures which parameters the script actually reads; Task 5
/// runs it under M1's probe assignments to freeze the column contract, and does
/// not exist yet.
///
/// The backstop for every branch none of those assignments reaches is the one
/// hand-written SQL already has: `render` re-runs the byte tests on what it
/// produced, on every request (`crate::render::check_rendered_statement`).
fn generation_params(params: &[ParamSpec]) -> crate::transform::GenerationParams {
    let mut out = crate::transform::GenerationParams::default();
    for p in params {
        out.insert(p.name.clone(), Some(&representative_value(p)));
    }
    out
}

/// What a green [`validate_endpoint`] hands back.
///
/// Two values rather than one because [`validate`] needs both and recomputing
/// either would be a second computation of something already decided: see
/// [`IteratedParams`] for what deciding the shape twice cost, and
/// [`GeneratedSql`] for why generating twice is worse than merely wasteful.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Validated {
    /// The one shape decision for this endpoint.
    pub iterated: IteratedParams,
    /// The text every rule above was applied to — generated where the statement
    /// carries a script, copied where it carries SQL.
    pub sql: GeneratedSql,
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
/// evaluation from `check_transform` and these two, plus one more module
/// evaluation from `EndpointStore::publish`'s own `check_transform_script`
/// call — and **all four are outside the transaction**. The count inside the
/// locked transaction is now zero; see [`ScriptEval`] for what it used to be
/// and what it cost.
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
/// The SQL text of a statement — the author's, or the one their script built.
///
/// One helper rather than seven `match`es, because seven copies of "and what
/// about the script case?" is seven chances for one of them to answer `""` and
/// keep going. Package S Task 1 put it at all seven sites while the body was a
/// refusal; Task 3 swapped the body for a lookup into [`GeneratedSql`] and moved
/// no call site.
///
/// **There is no branch on the statement's source here, deliberately.** Both
/// kinds are in `generated` by the time this runs, so every rule downstream is
/// applied to a `&str` that carries no evidence of where it came from — which is
/// the property the whole package rests on. See [`GeneratedSql`].
fn statement_sql<'a>(
    statement: &StatementSpec,
    generated: &'a GeneratedSql,
) -> std::result::Result<&'a str, ValidationError> {
    generated
        .get(&statement.name)
        .ok_or_else(|| ValidationError::GenerationMismatch(statement.name.clone()))
}

/// Every template `statement` may emit, [`statement_sql`]'s first.
///
/// The sibling of [`statement_sql`] and the same lookup, for the rules that are
/// about the SET of texts a statement can produce rather than about the one the
/// snapshot freezes: the scans that decide whether a text may be sent to the
/// engine at all, and the describe loop whose union is the contract. A written
/// statement returns a one-element list, so both callers below are unchanged for
/// every definition with no script.
fn statement_templates<'a>(
    statement: &StatementSpec,
    generated: &'a GeneratedSql,
) -> std::result::Result<Vec<&'a str>, ValidationError> {
    generated
        .templates(&statement.name)
        .ok_or_else(|| ValidationError::GenerationMismatch(statement.name.clone()))
}

fn validate_endpoint(
    statements: &[StatementSpec],
    params: &[ParamSpec],
    cache: &CacheSpec,
    output: &OutputSpec,
    conn: ConnectionSafety,
    script_eval: ScriptEval<'_>,
) -> std::result::Result<Validated, ValidationError> {
    egret_dataapi_api::validate_statements(statements)?;
    // Package S Task 3: generate here — AFTER the statement-list rule above and
    // BEFORE every scan below. Both halves of that position are load-bearing.
    //
    // After, because `MAX_STATEMENTS` is what bounds how many Starlark
    // evaluations this line can perform;
    // `the_statement_bound_is_checked_before_anything_renders` measures a
    // 256-statement list refused in under 500 ms, and generating first would
    // make it 256 evaluations of an author's script.
    //
    // Before, because everything below reads TEXT and must not be able to tell
    // whether an author typed it or a script built it. A generated template that
    // reached the scans by some other route — or skipped one of them — is the
    // single defect this task exists to prevent: `;` is refused at
    // `reject_unscannable_bytes`, at the comment/literal scan and again after
    // render, and a script must not be a way around any of the three.
    //
    // The skip is not a skip of the RULES, only of the evaluation: see
    // `ScriptEval`, whose payload is the text publish generated outside its
    // transaction and hands in here to be scanned exactly like any other.
    let generated = match script_eval {
        ScriptEval::Here => GeneratedSql::build(statements, params)?,
        ScriptEval::AlreadyRunBeforeTheLock { generated } => generated.clone(),
    };
    // **Package 5**, and it is placed here — after generation, not before it —
    // for a reason worth stating. It is a pure scan over `statements` and
    // `params`, so it COULD run first. Running it after `GeneratedSql::build`
    // means an author who both concatenates a parameter outright and left it
    // unconstrained reads `ParameterValueInSqlText` — which names the statement,
    // the parameter and the three safe spellings — rather than the broader
    // "give it a pattern". The specific message wins where both apply; the
    // declarative rule is what catches everything the probe could not see. The
    // cost of that ordering is bounded by the same two limits `build` checks
    // first, so it is an ordering choice and not an unbounded one.
    //
    // It runs under `AlreadyRunBeforeTheLock` too, and must: publish's
    // in-transaction re-validation has to reach the same verdict `validate`
    // did, and this rule evaluates nothing, so there is nothing to skip.
    check_script_parameter_constraints(statements, params)?;
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
    // comparison, cheap to repeat), the second half of it runs a Starlark
    // interpreter over the script's module top level. Inside the loop, an
    // endpoint at `MAX_STATEMENTS` would parse-and-evaluate the same script up
    // to 16 times. Once here bounds the whole endpoint's transform cost by ONE
    // evaluation, matching `validate_envelope` right above it — both are
    // endpoint-wide properties of `output`, not per-statement rules.
    //
    // The two halves are separate calls because ONE caller must skip the second:
    // publish re-runs this whole function against a row it has LOCKED, inside an
    // open transaction, and `MAX_WALL_CLOCK` is a 2 s bound on ticks rather than
    // a latency guarantee — `run_transform` records a script measured at 28.8 s
    // against it. Evaluating there would hold the endpoint's row lock and a pool
    // connection for that long, per publish, from author-supplied Starlark. See
    // `ScriptEval`.
    check_transform_shape(output)?;
    if matches!(script_eval, ScriptEval::Here) {
        check_transform_script(output)?;
    }
    // Every statement through every scan except the contract rule below. The
    // scans come before ANY render for the reason `check_definition_scans`
    // records: they are what bounds the cost of the rendering, and they cost
    // nothing themselves.
    // EVERY template, not only the one the snapshot freezes. Task 3 left this as
    // a recorded residual — one assignment was generated, so one branch of a
    // branching script was scanned — and Task 5 is where it closes, because Task
    // 5 is what makes the other branches exist as text. It is not a tidy-up: a
    // template in `alternates` is one `validate` hands to
    // `describe_as_with_params`, so a generated `DELETE FROM t` on a branch the
    // representative values do not take would reach the engine unscanned. `;`
    // and `#` have the render-time backstop; the head word has none.
    //
    // The scans are the cheap half by construction — no render, no engine, no
    // connection — so running them per template costs comparisons, and the
    // budget that bounds the expensive half is spent in `GeneratedSql::build`.
    for statement in statements {
        for template in statement_templates(statement, &generated)? {
            check_definition_scans(template, params, ParamRules::Draft)?;
        }
    }
    let mut read: Vec<String> = Vec::new();
    for statement in statements {
        read.extend(crate::render::context_names(statement_sql(
            statement, &generated,
        )?)?);
    }
    // Spec §2.4, and the whole of what package S changes about this rule. A
    // written statement reads a parameter in exactly one way — `{{ x }}` in its
    // text — and the loop above finds every such name. A SCRIPT has a second
    // way: `if params["grain"] == "daily"` reads `grain` and may emit no
    // `{{ grain }}` at all. Applying the written-SQL test to generated text
    // would refuse the branching case this package exists to enable, so the
    // names measured behaviourally by [`GeneratedSql::probe`] join the union
    // here — the same question, asked in the only way a script can answer it.
    //
    // The names come out of `generated` rather than being measured here so that
    // publish's in-transaction re-validation gets them without evaluating
    // anything; see [`GeneratedStatement`].
    read.extend(generated.varying_params().map(str::to_owned));
    check_every_parameter_is_read(&read, params)?;
    check_cache_scope(cache, conn.delegated, conn.impersonate)?;
    // Beside the cache rule, and after it, deliberately. Both read the same
    // connection and neither renders; ordering them cache-then-governance keeps
    // `check_cache_scope`'s message first for the one draft that breaks both
    // (`scope: shared` on a governed, non-delegated connection), which is the
    // message that names a field on the DRAFT the author is editing. The
    // governance refusal names a field on the DATABASE, which is often a
    // different person's fix.
    check_governed_connection(conn)?;
    // ONE shape decision for the endpoint, made before the first render and
    // handed to every one of them. See [`IteratedParams`] for what deciding it
    // per statement cost. Returned rather than recomputed by [`validate`],
    // which renders the same statements again to describe them: two computations
    // of the same set are two things that can disagree.
    //
    // Across every TEMPLATE, for the same reason it is across every statement:
    // the shape is one decision for the endpoint and a `{% for %}` header in any
    // text that will be rendered is what widens it. A branch that iterates a
    // parameter the baseline only reads as a scalar would otherwise be rendered
    // under the wrong shape — and, since `renderings_for` falls back to a direct
    // render whose failure it reports, refused with a render error rather than
    // published.
    let sqls: Vec<&str> = statements
        .iter()
        .map(|statement| statement_templates(statement, &generated))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .concat();
    let iterated = IteratedParams::across(&sqls, params);
    // Last, and once per TEMPLATE: this is the half that renders.
    //
    // Every template rather than the baseline one, and package S Task 6 is where
    // that had to change. Task 5 left it deliberately: alternates were described
    // but never SERVED, so the gate over the baseline was the gate over
    // everything an engine could receive. Task 6 makes serving generate per
    // request, and a template the representative values do not take is then a
    // statement a real call sends — so the behavioural gate
    // (`render::check_publish`: render with opaque values, render again with
    // plain ones, refuse any divergence) has to have looked at it.
    //
    // `sqls` above is the same concatenation and is already built; this loop
    // walks it rather than re-deriving, so the set the shape was decided across
    // and the set that is gated cannot differ.
    for sql in &sqls {
        check_render_rules(sql, params, &iterated)?;
    }
    Ok(Validated {
        iterated,
        sql: generated,
    })
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
/// It is also where preview and publish still diverge, in one direction and
/// **reachably** — through the draft wire since package S Task 1, which is when
/// `StatementSpec` began accepting `script`, and through the console since Task
/// 8: a sibling arrives as SQL TEXT, so a sibling that is itself a `build_sql`
/// script cannot be expressed, is spent as one template against the endpoint's
/// budget, and contributes to the iterated union whatever its SOURCE happens to
/// scan as rather than what it would emit.
///
/// **That last one runs in BOTH directions**, which an earlier wording of this
/// paragraph denied. A header only the EMITTED text carries is missed, and
/// preview binds a scalar where serving binds a list; a header the SOURCE
/// carries on a branch the script never emits is counted, and preview binds a
/// list where publish and serving bind a scalar — preview refusing a draft that
/// publishes green. [`SiblingStatements::union_with`] is where both happen, and
/// `tests::a_scripted_siblings_source_can_make_previews_shape_wider_than_publishs`
/// is the pin. The full statement of it, including why closing it is a decision
/// about the wire type, is in the `crate::preview` module comment.
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
///
/// # The statement on screen may be a SCRIPT (package S Task 7)
///
/// It takes a [`StatementSource`] rather than a `&str` for the same reason
/// [`validate_endpoint`] takes [`StatementSpec`]s: the author's editor now holds
/// either SQL or a `build_sql` script, and the rules below are about TEXT. So
/// this generates first — through [`GeneratedSql::build`], the one generator —
/// and scans what came out. Nothing downstream of that line can tell whether the
/// author typed the text or a script built it, which is the property package S
/// rests on.
///
/// **Every template, not only the baseline one**, exactly as
/// [`validate_endpoint`] does since Task 6: a branching script emits more than
/// one, and a text this function did not scan is a text `crate::preview::run`
/// could send to the engine as the AUTHOR.
pub(crate) fn validate_preview(
    source: &StatementSource,
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
    // Generate, in the position [`validate_endpoint`] generates from: after the
    // statement-list rule that bounds how many evaluations are possible, and
    // before every scan below, which must read TEXT and must not be able to tell
    // where it came from. Both halves are the same argument that function's own
    // comment makes at length.
    //
    // A written statement pays a `BTreeMap` insert and a `String` clone here and
    // nothing else — `GeneratedSql::build` copies its SQL in rather than
    // branching on the kind — so every preview that existed before package S
    // takes exactly the path it took.
    let statement = StatementSpec {
        name: PREVIEW_STATEMENT_NAME.to_owned(),
        source: source.clone(),
    };
    //
    // `build_after`, not `build`: `MAX_STATEMENTS` bounds the templates an
    // ENDPOINT renders, and this call holds one statement of it. Each sibling is
    // one SQL text and therefore one template, so `siblings.len()` is what the
    // rest of the endpoint has already spent — passed into the same counter
    // publish uses rather than compared again beside it. Without it a preview
    // started that count at zero and went green on drafts publish refuses; see
    // `GeneratedSql::build_after`.
    let generated =
        GeneratedSql::build_after(std::slice::from_ref(&statement), params, siblings.len())?;
    let templates = statement_templates(&statement, &generated)?;
    // **Package 5**, in the position [`validate_endpoint`] runs it from: after
    // generation, before every scan below. A draft the rule refuses must be
    // refused HERE too, or an author previews a generating statement green and
    // is told no at publish — the divergence this whole route exists to invert.
    //
    // Asked about the statement ON SCREEN only, which is the one asymmetry with
    // publish and it is inherited rather than introduced: `SiblingStatements`
    // carries the other statements' TEXT and cannot say whether any of them is a
    // script (see its doc), and only the statement on screen is scanned here for
    // the same reason. So previewing a WRITTEN statement whose sibling is a
    // script is silent about this rule, and publish refuses it. That direction —
    // green here, refused there — is the wrong one, and it is bounded to the one
    // case where the author is not looking at the script at all; closing it
    // needs the preview body to carry each sibling's SOURCE, which is a wire
    // change this package does not make.
    check_script_parameter_constraints(std::slice::from_ref(&statement), params)?;
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
    // Only the statement on screen is SCANNED. The siblings are not this
    // preview's statement: they are already stored, they were scanned when they
    // were written, and re-refusing one here would block previewing the
    // statement the author is actually looking at because a different one is
    // mid-edit.
    for template in &templates {
        check_definition_scans(template, params, ParamRules::Draft)?;
    }
    let iterated = siblings.union_with(&templates, params);
    for template in &templates {
        check_render_rules(template, params, &iterated)?;
    }
    Ok(iterated)
}

/// The name a preview's one statement is generated under.
///
/// The body carries no statement name — this route runs the statement on screen
/// and the siblings are text with no names either (see [`SiblingStatements`]) —
/// but [`GeneratedSql`] is keyed by name, so generation needs one. It is a
/// constant rather than anything derived from the request: it never leaves this
/// function, it is looked up by the same value it was inserted under, and
/// `^[a-z][a-z0-9_]*$` is what [`egret_dataapi_api::validate_statements`]
/// demands of it.
const PREVIEW_STATEMENT_NAME: &str = "preview";

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
// `Clone` so `crate::preview::run` can move the whole draft onto a blocking
// thread (see that module's section on the inline evaluation). It does not
// weaken the containment argument above: cloning a `SiblingStatements` produces
// another `SiblingStatements`, which still has no accessor for its text.
#[derive(Debug, Default, Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct SiblingStatements(Vec<String>);

impl SiblingStatements {
    /// How many there are. The count is public because the BOUND is a rule
    /// about the request; the text is not.
    fn len(&self) -> usize {
        self.0.len()
    }

    /// The iterated-parameter union across the previewed statement's templates
    /// and every sibling — the same set [`validate_endpoint`] computes for the
    /// whole endpoint, which is the point.
    ///
    /// The texts are passed IN rather than the siblings being handed out, so
    /// this is the only expression that ever sees them together and there is no
    /// accessor to misuse.
    ///
    /// A SLICE of texts, not one, because a branching script emits more than one
    /// template and the shape is one decision for the endpoint: a branch whose
    /// `{% for %}` header iterates a parameter the baseline reads as a scalar
    /// has to widen the set, or that branch renders under the wrong shape. Same
    /// reason [`validate_endpoint`] decides it across every template rather than
    /// across every statement's first one. A written statement passes a slice of
    /// one and nothing changes.
    fn union_with(&self, sqls: &[&str], params: &[ParamSpec]) -> IteratedParams {
        let mut all: Vec<&str> = Vec::with_capacity(sqls.len() + self.0.len());
        all.extend(sqls.iter().copied());
        all.extend(self.0.iter().map(String::as_str));
        IteratedParams::across(&all, params)
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
    conn: ConnectionSafety,
) -> Result<ValidatedDraft> {
    // The shape decision comes back from the validator that just used it, so the
    // renders that DESCRIBE each statement below and the renders that VALIDATED
    // it are against the same `ParamValues` shape.
    let Validated {
        iterated,
        sql: generated,
    } = validate_endpoint(
        &draft.statements,
        &draft.params,
        &draft.cache,
        &draft.output,
        conn,
        ScriptEval::Here,
    )?;

    // Enumerate every statement's branches before describing any of them, and
    // spend the endpoint's whole budget in one decision. See the doc comment.
    let mut planned: Vec<(&StatementSpec, Vec<(String, usize)>)> =
        Vec::with_capacity(draft.statements.len());
    let mut total = 0usize;
    for statement in &draft.statements {
        // Every TEMPLATE the statement can emit, and `branch_renderings` over
        // each — package S Task 5, and the whole of what it needed. A generated
        // statement's SQL is a pure function of `params` (the script never sees
        // a row: that is C2's transform, a different phase), so the situation is
        // exactly the one M1 already solved. The only thing missing was the SET
        // of texts, because `branch_renderings` takes a `&str`.
        let mut renderings: Vec<(String, usize)> = Vec::new();
        for template in statement_templates(statement, &generated)? {
            for rendering in renderings_for(template, &draft.params, &iterated)? {
                // Deduplicated ACROSS the templates as well as within each one:
                // two templates that differ only where the parameters do not
                // reach can render to the same statement, and describing it
                // twice would spend two of the endpoint's branches on one
                // contract — and put the same column list into `per_branch`
                // twice, which is the list `required` and the `arrays` rule are
                // computed from.
                if !renderings.iter().any(|(text, _)| *text == rendering.0) {
                    renderings.push(rendering);
                }
            }
        }
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
            // The SOURCE, not the text just described. For a script that text is
            // one branch of many and serving generates its own per request — see
            // [`StatementSnapshot`]. `described.union` above is what carries
            // every branch's contract forward.
            source: statement.source.clone(),
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
    Ok(ValidatedDraft {
        statements: frozen,
        iterated: iterated.names().to_vec(),
    })
}

/// Everything [`validate`] resolved about a draft that is needed to say what
/// publishing it would freeze.
///
/// # Why `iterated` comes back rather than being recomputed
///
/// It was already discarded here once. [`EndpointSnapshot::iterated`] is the
/// endpoint's one shape decision, made across EVERY template of every
/// statement, and the reason it is frozen at all is that a scripted statement
/// has no text to derive it from before its parameters are resolved. Package 6
/// needs the snapshot `validate` would produce — so that `POST /validate` can
/// diff it against the live version and hand the author an acknowledgement for
/// exactly the breaks that publish will find — and deriving the set a second
/// time, from one of the texts, is the M2 bug one level down.
#[derive(Debug)]
pub struct ValidatedDraft {
    /// One entry per statement, in the author's order, each carrying the output
    /// schema `describe` resolved for it.
    pub statements: Vec<StatementSnapshot>,
    /// The parameter names some statement iterates.
    pub iterated: Vec<String>,
}

/// Everything [`EndpointStore::publish`] needs beyond the workspace and the
/// endpoint it is publishing.
///
/// A struct rather than five more positional parameters, and not only because
/// clippy counts them: `described` and `acknowledge_breaking` are a `Vec` and an
/// `Option<&str>` that no reader of a call site could tell apart from `note`
/// without opening this file. Naming them at the call site is what stops the
/// day a describe is passed where a note belongs.
pub struct PublishRequest<'a> {
    /// Who is publishing. Written to `endpoint_versions.created_by`, and
    /// therefore the record of who accepted any breaking change.
    pub by: UserId,
    /// Free-form "what changed", stored on the version.
    pub note: &'a str,
    /// The draft as it was read BEFORE the describe. `publish` re-reads the row
    /// under a lock and refuses unless the two still agree.
    pub draft: &'a DataApiEndpointDto,
    /// One entry per statement, carrying the columns `describe` resolved.
    pub described: Vec<StatementSnapshot>,
    /// The acknowledgement `POST /validate` issued for the breaking changes this
    /// publish would make. `None` is the only correct value when it makes none —
    /// see [`crate::contract::ContractDiff::check_acknowledgement`].
    pub acknowledge_breaking: Option<&'a str>,
}

/// `described` does not describe this endpoint's statements.
///
/// A distinct type rather than an `Error`, because the two callers owe the
/// reader different sentences for it. In `publish` it means the draft was edited
/// during the describe round trips and the answer is a retryable `409`; in
/// `POST /validate` the two halves come from one call and cannot disagree, so
/// reaching it there is a bug in this crate and is answered as a `500`. Folding
/// them into one `Error` at the point of failure would make one of the two
/// messages wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescribedMismatch;

/// What publishing this definition, with these described columns, would freeze.
///
/// # Why this is a function and not two blocks of code
///
/// It was one block, inside `publish`, and package 6 needed the same value
/// before the transaction: `POST /validate` has to diff the version an author is
/// about to publish against the one consumers are receiving, and a diff computed
/// against an approximation of the snapshot is a diff that can disagree with the
/// refusal publish issues a second later. `DataApiValidationDto`'s own doc
/// already claimed validate returns "exactly what `publish` would freeze"; this
/// is what makes the claim structural rather than a promise two call sites keep.
///
/// # The pairing is by NAME, not by position
///
/// `described` carries the name each column list was resolved for, and a
/// positional zip discards it: the one thing that could go wrong here —
/// `total`'s columns frozen onto `rows`' SQL, an endpoint that then answers
/// `503 schema_drift` on every call it does not answer wrong — is a crossing
/// that pairing by name cannot express. In `publish` it is also guarded by the
/// identity check against the locked row, which is a guarantee held in a
/// different function; this is the same guarantee held by the data.
///
/// A count that does not match afterwards is [`DescribedMismatch`] too: every
/// statement of the endpoint was matched, so a longer `described` can only be a
/// describe of a statement that no longer exists.
pub fn prospective_snapshot(
    endpoint: &DataApiEndpointDto,
    described: &[StatementSnapshot],
    iterated: &[String],
    conn: ConnectionSafety,
) -> std::result::Result<EndpointSnapshot, DescribedMismatch> {
    let mut statements: Vec<StatementSnapshot> = Vec::with_capacity(endpoint.statements.len());
    for statement in &endpoint.statements {
        let resolved = described
            .iter()
            .find(|d| d.name == statement.name)
            .ok_or(DescribedMismatch)?;
        statements.push(StatementSnapshot {
            // The name and the DEFINITION come from the endpoint row, and only
            // the resolved columns come from the describe. Since package S Task
            // 6 the definition is the author's source rather than the text
            // generated from it, which makes that sentence stronger rather than
            // weaker: what is frozen is exactly the bytes the row carries, with
            // nothing computed in between.
            name: statement.name.clone(),
            source: statement.source.clone(),
            columns: resolved.columns.clone(),
        });
    }
    if described.len() != statements.len() {
        return Err(DescribedMismatch);
    }
    Ok(EndpointSnapshot {
        statements,
        // The endpoint's one shape decision, made across every template of every
        // statement by the validation this was called with. Frozen because a
        // scripted statement has no text to derive it from before its parameters
        // are resolved, and resolving them needs it — see
        // [`EndpointSnapshot::iterated`].
        iterated: iterated.to_vec(),
        method: endpoint.method.clone(),
        // The stored path, which `normalize_path` already canonicalised. The
        // snapshot must never carry a spelling the endpoint row does not have.
        path: endpoint.path.clone(),
        database_id: endpoint.database_id,
        params: endpoint.params.clone(),
        output: endpoint.output.clone(),
        cache: endpoint.cache.clone(),
        limits: endpoint.limits.clone(),
        rate_limit: endpoint.rate_limit.clone(),
        // The witness PAIR, both halves from ONE read of `query.databases`.
        // Freezing only `delegated` — which is what this did — left
        // `{delegated: false, impersonate: true}` indistinguishable from a
        // genuinely shared-safe connection forever after.
        //
        // **`conn.governed` is deliberately NOT frozen beside them**, and the
        // asymmetry is the point rather than an omission. A witness is for a
        // question of the form "is the live row still what this version was
        // published against?", which is the whole of the `shared`-cache rule:
        // the unsafe half (`scope: shared`) lives in the snapshot, so the live
        // row is the only thing that can move. Governance is not that question.
        // BOTH halves of `governed && !runs_per_principal(…)` are live, mutable
        // columns, so an equality against a frozen witness answers the wrong
        // thing in the exact case that matters: an endpoint published on an
        // ungoverned, non-delegated connection freezes
        // `governed: false, delegated: false`, and when an admin marks the
        // database governed the pair `(delegated)` still AGREES — the witness
        // stays silent while the endpoint becomes the defect. So serving
        // evaluates the live triple instead; see
        // [`crate::serving::governance_allows_serving`].
        delegated: conn.delegated,
        impersonate: conn.impersonate,
        // The stamp, written here and nowhere else, because publish calls this
        // with the validation that has just run against the row being frozen.
        // §7 of the parameters spec said "every endpoint with parameters must be
        // re-published after P4.5" — an instruction to whoever reads it. This is
        // the enforcement: a version frozen before the gate existed carries `0`,
        // and the serving path refuses it.
        param_gate: PARAM_GATE,
    })
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
/// **Publish only, since C2 Task 8.** `crate::preview` now runs the whole of
/// [`validate_preview`] on a blocking thread, because adding
/// [`check_transform`] to that path raised the bound past what the argument
/// below can carry — a Starlark module evaluation is bounded by
/// `transform::MAX_WALL_CLOCK` (2 s nominal, 28.8 s measured for a
/// tick-expensive script), not by this constant. That is the redo this note
/// asks for, and it is written up in `crate::preview`. What is left inline is
/// `POST …/validate` and `publish` — which run the SAME Starlark evaluation, on
/// the async worker, and inherit the same problem. It is pre-existing (C2 Tasks
/// 4 and 5 put it there) and is recorded rather than fixed here.
///
/// **One half of it WAS fixed, and the distinction matters.** Until the C2 fix
/// wave, `publish` ran that evaluation from inside its own transaction, after
/// `SELECT … FOR UPDATE` — so a tick-expensive script held the endpoint's row
/// lock and a pool connection for the same 28.8 s, not merely a worker. That is
/// closed: the evaluation is hoisted above `begin()` and the in-transaction
/// re-validation is passed [`ScriptEval::AlreadyRunBeforeTheLock`], with
/// `pin_no_starlark_evaluation_inside_publishs_transaction` to keep it there.
/// What remains is worker occupancy on two `data_api_endpoint:manage` routes —
/// the same shape `crate::preview` moved to a blocking thread, and the obvious
/// next step for these two.
///
/// The reason is that the cost is **bounded and authenticated** — the paths
/// that still do this inline sit behind `data_api_endpoint:manage`, and this
/// constant is what makes the RENDERING's upper bound a number rather than a
/// function of the request. It is
/// ~122 ms per STATEMENT, and it took
/// [`egret_dataapi_api::MAX_STATEMENTS`] to make the endpoint's total a number
/// too: measured at 121 ms × N, it reached 31.6 s at 256 statements. It is
/// **not** that an engine `describe` of similar duration happens nearby: an
/// awaited describe RELEASES the worker, so the runtime schedules other tasks
/// for those milliseconds and only the one caller waits. This rendering OCCUPIES
/// the worker — tokio cannot preempt it, every other task queued on that worker
/// eats the whole spike, and an author calling one of these routes in a loop
/// holds one worker per in-flight request. Same magnitude, opposite effect on
/// everything else running. If the bound ever rises, that reasoning has to be
/// redone rather than carried over — which is exactly what happened above, so
/// this paragraph now covers the rendering alone and no longer covers every
/// caller.
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
/// times per TEXT (`render::MAX_PROBES` assignments per context) before this
/// limit can say no. That residual is synchronous CPU on an async task, and it is
/// bounded by [`MAX_DECLARED_PARAMS`] instead, which is a scan and runs first —
/// though only per text, so an endpoint's worst-case render cost is that
/// bound times the number of texts. The spec asked for that to be
/// re-measured rather than carried over, and it was: **121 ms per text,
/// linear in N**, which is why the third bound exists.
/// [`egret_dataapi_api::MAX_STATEMENTS`] caps N, and it is the only one of the
/// three that runs before `validate_endpoint`'s first scan. The three are parts
/// of one bound, not one limit and two spares.
///
/// **"Text", not "statement", since package S Task 5**, and the two used to be
/// the same word: a `build_sql` script that branches gives ONE statement more
/// than one text, so `MAX_STATEMENTS` is now counted over the templates
/// [`GeneratedSql::build`] produces rather than over the list it was handed.
/// Nothing about N changed — see [`ValidationError::TooManyGeneratedTemplates`]
/// for the measurement, which lands on the same 2.06 s this paragraph describes.
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
    /// What publishing this version did to the consumers of the one it replaced
    /// (package 6), and therefore what its `created_by` accepted when they
    /// acknowledged it.
    ///
    /// `None` and `Some([])` are different facts and are kept apart. `None` is a
    /// version published before this was recorded, or the FIRST version of an
    /// endpoint, where there was no live contract to break. `Some([])` is a
    /// version that was compared against the live one and broke nothing. A
    /// history that showed both as "no breaking changes" would be claiming
    /// something about the first that nobody ever checked.
    pub breaking_changes: Option<Vec<crate::contract::ContractChange>>,
    /// The version NUMBER [`Self::breaking_changes`] was compared against —
    /// whatever was live when this one was published.
    ///
    /// `None` exactly when `breaking_changes` is `None`. Usually the preceding
    /// version, and after a rollback NOT: `rollback` moves the live pointer
    /// backwards and adds no row, so `Some([])` beside a baseline two versions
    /// down is a comparison that happened and a set of consumers it says nothing
    /// about. The number rather than the id, because this is what a history
    /// renders and a number is what a reader can place.
    pub compared_against_version: Option<i32>,
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
    /// The breaking changes this publish made, which the caller acknowledged to
    /// get here (package 6).
    ///
    /// Echoed back rather than merely stored so the audit event the route emits
    /// carries the same list the version row does — one write, two records, and
    /// no second computation of what was accepted.
    ///
    /// `None` for a first publish and for a `rollback`, neither of which
    /// compares anything: a rollback re-serves a contract that was already
    /// published once, and the two live rules it DOES re-ask are the ones whose
    /// inputs are live (see [`EndpointStore::rollback`]).
    ///
    /// `skip_serializing_if` here and deliberately NOT on
    /// [`DataApiEndpointVersionDto::breaking_changes`], which is the same value
    /// read back out of the table. There, `null` is an answer — "nobody looked"
    /// — and a reader has to be able to tell it from `[]`. Here the field is
    /// absent from a `rollback`'s response because that response is not about a
    /// comparison at all, and a `null` would invite the reader to interpret it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breaking_changes: Option<Vec<crate::contract::ContractChange>>,
    /// The version [`Self::breaking_changes`] is a statement ABOUT: whatever was
    /// live at the instant of the publish.
    ///
    /// Echoed for the same reason it is stored — see migration `0131`. After a
    /// rollback this is not the version before this one, and an empty list read
    /// without it says more than it knows. `None` for a first publish, and for a
    /// `rollback`, which compares against the version it moved OFF and reports
    /// that under [`Self::rollback_breaks`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compared_against: Option<Uuid>,
    /// What a `rollback` did to the consumers of the version it moved OFF,
    /// classified by the same [`crate::contract::compare_contracts`]
    /// `publish` uses — and refusing nothing.
    ///
    /// A separate field from [`Self::breaking_changes`] because it answers a
    /// separate question. That one is read back out of
    /// `endpoint_versions.breaking_changes` and is a record of what an author
    /// ACCEPTED; this one is stored on no row, because a rollback creates no
    /// version. Sharing a field would let a history renderer show a rollback's
    /// consequences as an acknowledgement nobody gave.
    ///
    /// `None` for a `publish`, for a rollback onto an endpoint with nothing
    /// live, and when either snapshot cannot be read — a rollback must not fail
    /// because the sentence it was going to say could not be built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_breaks: Option<Vec<crate::contract::ContractChange>>,
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// The one unique constraint a publish or rollback can now meet: migration
/// 0204's `dataapi_endpoints_live_route_key`. Another endpoint is live at the
/// URL this version would take — typically one whose draft moved away from it
/// and has not been published since, so it still serves there. A 409 naming
/// the URL, not a 500 naming the index; any other error is still `db_err`'s.
fn live_route_err(e: sqlx::Error, method: &str, path: &str) -> Error {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("dataapi_endpoints_live_route_key") => {
            Error::Conflict(format!(
                "another endpoint in this workspace is served at {method} {path}; an endpoint \
                 keeps its published URL until it is published again, so choose another path \
                 or publish the move of the endpoint that holds it first"
            ))
        }
        _ => db_err(e),
    }
}

impl EndpointStore {
    /// The [`ConnectionSafety`] of `database` (which must belong to
    /// `workspace`) — the three columns that decide what may be published onto
    /// it. See [`runs_per_principal`] and [`check_governed_connection`].
    ///
    /// Read here rather than passed in: it is the input to
    /// [`check_cache_scope`] and [`check_governed_connection`], and a caller who
    /// supplied it would be supplying the answer to the safety question.
    /// `NotFound` for a database outside the workspace, matching every other
    /// cross-workspace answer in this crate.
    ///
    /// **It returned `delegated` alone once, then the
    /// `(delegated, impersonate)` pair, and now the triple.** Each widening was
    /// the same lesson: a signature that cannot express the unsafe configuration
    /// makes every caller structurally incapable of noticing it. All three
    /// columns are read in ONE statement so they cannot come from three
    /// different moments.
    pub async fn connection_safety(
        &self,
        workspace: Uuid,
        database: Uuid,
    ) -> Result<ConnectionSafety> {
        self.connection_safety_opt(workspace, database)
            .await?
            .ok_or_else(|| Error::NotFound("database".into()))
    }

    /// [`Self::connection_safety`] without the `NotFound`, for the one caller
    /// that has something to say about a database row that is GONE.
    ///
    /// Every other caller is deciding what may be written onto a connection, and
    /// for them "no such connection here" is the answer. [`Self::rollback`] is
    /// re-checking a version that was published long ago and whose database may
    /// since have been deleted; a 404 naming a database the caller never
    /// mentioned would send them looking for the wrong thing, and there is
    /// nothing to check anyway — a row that does not exist cannot be `governed`.
    /// Same asymmetry, same reason, as
    /// [`crate::serving::governance_allows_serving`].
    ///
    /// ONE statement for all three columns so they cannot come from three
    /// different moments; the public wrapper adds only the error.
    async fn connection_safety_opt(
        &self,
        workspace: Uuid,
        database: Uuid,
    ) -> Result<Option<ConnectionSafety>> {
        let row: Option<(bool, bool, serde_json::Value)> = sqlx::query_as(
            "SELECT delegated, governed, options \
               FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(database)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|(delegated, governed, options)| ConnectionSafety {
            governed,
            delegated,
            impersonate: impersonate_flag(&options),
        }))
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
    /// # What this does to the consumers of the version already live
    ///
    /// Everything above validates the draft against ITSELF. Until package 6 that
    /// was the whole of it: this function's only read of
    /// `dataapi.endpoint_versions` was the `COALESCE(MAX(version), 0) + 1`
    /// inside its own INSERT, so a removed column, a narrowed type, a renamed
    /// parameter or a new required parameter published in silence and every
    /// deployed consumer found out at its next call.
    ///
    /// So the live snapshot is read on this transaction and
    /// [`crate::contract::compare_contracts`] classifies every difference; a
    /// breaking one is refused unless `acknowledge_breaking` carries the digest
    /// [`crate::contract::ContractDiff::acknowledgement`] computes for exactly
    /// those changes. A long-lived endpoint may legitimately need a breaking
    /// change — the property is that nobody makes one without being told which
    /// one they are making.
    ///
    /// Three things about the placement, each of which was a choice:
    ///
    /// * It is read through **`live_version_id`**, not `MAX(version)`. A
    ///   rollback makes an older version live, and diffing against the newest
    ///   row would warn about a break nobody would feel and stay silent about
    ///   one they would.
    /// * It is read **inside this transaction**, after `locked` took
    ///   `FOR UPDATE`. The acknowledgement is bound to the live version id, so
    ///   the pointer moving between the check and the insert would let a token
    ///   issued for one contract authorise a break against another.
    /// * It is checked **after** the snapshot is built and **before** anything
    ///   is written, which is what makes the refusal free: the transaction rolls
    ///   back having inserted nothing, exactly like the trashed-endpoint
    ///   conflict below.
    ///
    /// The first publish of an endpoint reads `None` and compares nothing, and
    /// that is not the same as comparing and finding nothing — see the
    /// `breaking_changes` column's own comment in migration `0131`.
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
        req: PublishRequest<'_>,
    ) -> Result<DataApiPublishedDto> {
        let PublishRequest {
            by,
            note,
            draft,
            described,
            acknowledge_breaking,
        } = req;
        // BEFORE the transaction, deliberately, and this is the placement
        // decision rather than an accident of ordering. This is the only
        // Starlark evaluation `publish` performs. `transform::MAX_WALL_CLOCK`
        // is 2 s of TICKS and not a latency guarantee — `run_transform`
        // records 28.8 s measured against it — so running it after `begin()`
        // would hold the endpoint's row lock and a pool connection, on an async
        // worker, for as long as an author's script takes.
        //
        // Run against the DRAFT, which is sound because of the identity check
        // below: the transaction refuses with `Conflict` unless
        // `current.output == draft.output`, so the script evaluated here is
        // byte-identical to the script being frozen. The re-validation inside
        // the transaction therefore passes `ScriptEval::AlreadyRunBeforeTheLock`
        // and re-runs every structural rule and no interpreter.
        //
        // `pin_no_starlark_evaluation_inside_publishs_transaction` is what keeps
        // this here: it reads this function's own text and fails if an
        // evaluation appears between `begin()` and the commit.
        check_transform_script(&draft.output)?;
        // The OTHER Starlark this endpoint can carry, and it is here for exactly
        // the reason the line above is: a statement whose SQL a `build_sql`
        // script builds is an author's script under the same budgets, and
        // generating it after `begin()` would hold the endpoint's row lock and a
        // pool connection for as long as that script takes.
        //
        // Run against the DRAFT, sound for the same reason: the identity check
        // below refuses with `Conflict` unless `current.statements ==
        // draft.statements` AND `current.params == draft.params`, which are
        // generation's only two inputs. The in-transaction re-validation is then
        // handed this text (`ScriptEval::AlreadyRunBeforeTheLock`) and re-runs
        // every scan over it — the rules are re-applied against the locked row,
        // only the evaluation is not repeated.
        let generated = GeneratedSql::build(&draft.statements, &draft.params)?;

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

        // Same statement as `connection_safety`, issued on the transaction so the
        // validation below and the values frozen into the snapshot are the same
        // read — and one statement, so the three columns cannot come from three
        // different moments.
        //
        // That is all it buys. It does NOT pin the columns: there is no
        // `FOR SHARE` on `query.databases` and the isolation level is READ
        // COMMITTED, so a concurrent flip can commit before this transaction
        // does. Locking would not help either, because the flip is equally free
        // the instant after the commit. That residual is precisely what the
        // frozen witness PAIR exists for — P4 compares both halves against the
        // live row and refuses a shared cache when either disagrees — and for
        // `governed` it is what `serving::governance_allows_serving` exists for.
        // Neither is something publish can close on its own.
        //
        // Read in ONE statement so the three cannot come from three different
        // moments. `options` is JSONB and the key is spelled exactly as
        // `egret_query_api::ConnOptions::impersonate` serialises it — this crate
        // deliberately depends on no other feature seam, so the coupling is by
        // name, read through `impersonate_flag`, and pinned from the owning side
        // by `egret-query-api`'s `wire_names`.
        let (delegated, governed, options): (bool, bool, serde_json::Value) = sqlx::query_as(
            "SELECT delegated, governed, options \
               FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(current.database_id)
        .bind(workspace)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound("database".into()))?;
        let conn = ConnectionSafety {
            governed,
            delegated,
            impersonate: impersonate_flag(&options),
        };

        // The whole pure validator, against the row actually being frozen. See the
        // doc comment: this is what closes the window, not the identity check above.
        // Nothing here renders — `described` was computed by `validate`.
        //
        // The shape set it returns is FROZEN into the snapshot below (package S
        // Task 6), and this call is the right one to take it from for the same
        // reason the statements are taken from `current`: it is the run against
        // the locked row. Until Task 6 it had no second use and was discarded.
        let validated = validate_endpoint(
            &current.statements,
            &current.params,
            &current.cache,
            &current.output,
            conn,
            // The script was evaluated above, before this transaction opened,
            // and the identity check above says the row locked here carries the
            // same one. See `ScriptEval`.
            ScriptEval::AlreadyRunBeforeTheLock {
                generated: &generated,
            },
        )?;

        // ONE expression of "what publish would freeze", shared with
        // `POST /validate` (package 6) so that the contract an author is shown
        // before publishing and the contract that is actually frozen cannot be
        // two different answers. The definition comes from `current` — the
        // LOCKED row — and only the resolved columns come from the caller; see
        // `prospective_snapshot` for why that pairing is by NAME.
        let snapshot = prospective_snapshot(&current, &described, validated.iterated.names(), conn)
            .map_err(|DescribedMismatch| {
                Error::Conflict(
                    "the endpoint was edited while it was being published; validate and \
                         publish again"
                        .into(),
                )
            })?;
        // ── Package 6: what this publish does to the consumers of the version
        // currently being served ─────────────────────────────────────────────
        //
        // The LIVE version, reached through `endpoints.live_version_id`, and
        // NOT `MAX(version)` — which is the only row this function used to read
        // at all. A rollback makes an OLDER version live, so the newest row can
        // describe a contract nobody is calling; diffing against it would warn
        // an author about a break that does not exist and stay silent about one
        // that does.
        //
        // Read on THIS transaction, after `locked` took `FOR UPDATE` on the
        // endpoint row, so `live_version_id` cannot move between the diff and
        // the insert — which is what makes the acknowledgement, bound to that
        // id, mean the same thing when it is checked as when it was issued.
        //
        // `None` is a FIRST publish, and it is the one case where there is
        // genuinely nothing to break: no consumer has ever received anything
        // from this endpoint. It is a distinct absence rather than an empty
        // diff, and the two must not be confused — a fixture with no live
        // version exercises this branch and can never see a break at all.
        let live: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT v.id, v.snapshot FROM dataapi.endpoint_versions v \
               JOIN dataapi.endpoints e ON e.live_version_id = v.id \
              WHERE e.id = $1 AND e.workspace_id = $2",
        )
        .bind(id)
        .bind(workspace)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        //
        // A `409`, not a `400`, on every arm below. The very same body is
        // correct or incorrect depending on which version is live at this
        // instant, so "your request is malformed" would be false the moment a
        // rollback moved the pointer. What is true is that the request and the
        // server's state disagree, which is what a conflict is — and it is the
        // same class, and the same remedy, as the edit-during-publish conflict
        // above.
        // The baseline is carried out beside the diff, because `[]` is a claim
        // ABOUT a version and not about the endpoint: `rollback` moves
        // `live_version_id` backwards and writes no row, so the version this
        // publish compared against is not always the one before it. Stored, not
        // inferred — see migration `0131`.
        let mut compared_against: Option<Uuid> = None;
        let breaking: Option<Vec<crate::contract::ContractChange>> = match live {
            Some((live_version_id, live_snapshot)) => {
                // A stored snapshot that will not deserialize is corrupt data,
                // not an authoring error: refusing the publish is the
                // fail-closed direction, because the alternative is publishing
                // over a contract nobody could read and telling nobody.
                let live_snapshot: EndpointSnapshot = serde_json::from_value(live_snapshot)
                    .map_err(|e| Error::Internal(anyhow::anyhow!("read live snapshot: {e}")))?;
                let diff = crate::contract::compare_contracts(&live_snapshot, &snapshot);
                diff.check_acknowledgement(id, live_version_id, acknowledge_breaking)
                    .map_err(|e| Error::Conflict(e.to_string()))?;
                compared_against = Some(live_version_id);
                Some(diff.breaking)
            }
            None => {
                // A first publish compares nothing — and an EMPTY diff is the
                // truthful thing to ask, rather than skipping the question.
                // Skipping it is what this did first, and it left a hole in the
                // one property the acknowledgement has: a token sent against a
                // first publish was silently ignored, which is how a client
                // learns to send one always. Routed through the same function
                // as every other arm, so there is one rule and not two — the
                // ids are not read at all when there is nothing to acknowledge.
                crate::contract::ContractDiff::default()
                    .check_acknowledgement(id, Uuid::nil(), acknowledge_breaking)
                    .map_err(|e| Error::Conflict(e.to_string()))?;
                None
            }
        };
        let breaking_payload = breaking
            .as_ref()
            .map(|b| {
                serde_json::to_value(b)
                    .map_err(|e| Error::Internal(anyhow::anyhow!("serialize breaking: {e}")))
            })
            .transpose()?;

        let payload = serde_json::to_value(&snapshot)
            .map_err(|e| Error::Internal(anyhow::anyhow!("serialize snapshot: {e}")))?;

        // `breaking_changes` records the COMPARISON, and the distinction between
        // SQL `NULL` and `[]` is load-bearing: null means nobody looked — a
        // version published before this column existed, or a first version,
        // where there is no live contract to compare against — and `[]` means
        // this publish was compared and broke nothing.
        // A `DEFAULT '[]'` on the migration would have stamped the second claim
        // onto every row that only ever qualified for the first.
        //
        // `compared_against` is written in the same statement and is what makes
        // `[]` a true sentence rather than a nearly-true one. The diff is taken
        // against `live_version_id`, and `rollback` moves that pointer BACKWARDS
        // without writing a row here — so after a rollback from v1 to v0, this
        // publish can compare against v0, find nothing, and store `[]` while
        // every consumer of v1 has already been moved off a contract this table
        // never mentions. `[]` beside a baseline that is not the newest version
        // says exactly what happened; `[]` alone would have read as "broke
        // nobody". Recorded rather than gated: the rollback is the act that
        // moved them, it is deliberately not refused, and it now classifies and
        // audits what it did.
        //
        // The row already carries `created_by`, so together they are the audit
        // trail the package asked for: who accepted which break, against which
        // baseline, at which time — cheap to write here, and not recoverable
        // afterwards from anything else.
        let (version_id, version): (Uuid, i32) = sqlx::query_as(
            "INSERT INTO dataapi.endpoint_versions \
                 (endpoint_id, version, snapshot, note, created_by, breaking_changes, \
                  compared_against) \
             SELECT $1, COALESCE(MAX(version), 0) + 1, $2, $3, $4, $5, $6 \
               FROM dataapi.endpoint_versions WHERE endpoint_id = $1 \
             RETURNING id, version",
        )
        .bind(id)
        .bind(&payload)
        .bind(note)
        .bind(by.0)
        .bind(&breaking_payload)
        .bind(compared_against)
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
        //
        // `live_method` / `live_path` move HERE, with the pointer, and nowhere
        // else (KI-023, migration 0204): the URL is part of the contract this
        // version froze, so a draft's new path takes effect at publish and not
        // when it is saved. Written from the snapshot, which is the locked row's
        // own `method` / `path`.
        let state: Option<String> = sqlx::query_scalar(
            "UPDATE dataapi.endpoints SET live_version_id = $1, \
               live_method = $4, live_path = $5, \
               state = CASE WHEN state = 'disabled' THEN 'disabled' ELSE 'live' END, \
               updated_at = now() \
             WHERE id = $2 AND workspace_id = $3 AND deleted_at IS NULL RETURNING state",
        )
        .bind(version_id)
        .bind(id)
        .bind(workspace)
        .bind(&snapshot.method)
        .bind(&snapshot.path)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| live_route_err(e, &snapshot.method, &snapshot.path))?;
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
            breaking_changes: breaking,
            compared_against,
            rollback_breaks: None,
        })
    }

    /// The two rules a FROZEN version can fall out of without being touched,
    /// asked again against the connection as it is right now.
    ///
    /// Both read `query.databases` columns that no version owns:
    /// [`check_governed_connection`] (the database became `governed`, or the
    /// connection stopped running per principal) and [`check_cache_scope`] (the
    /// connection STARTED running per principal under a `shared` cache). Publish
    /// asks both; nothing re-asked them for a version made live by any other
    /// route, which is the gap [`Self::rollback`] closes by calling this.
    ///
    /// Only these two. It deliberately does NOT re-run [`validate_endpoint`]:
    /// every other rule there is a property of the frozen definition, which
    /// cannot have changed, and re-running the lot would make a rollback fail
    /// for a rule that did not exist when the version was published — turning
    /// "put yesterday's version back" into an upgrade gate during an incident,
    /// which is the one moment rollback has to work. What is checked here is the
    /// pair whose INPUTS are live.
    ///
    /// # `param_gate` is the near-miss, and it is left out on purpose
    ///
    /// A version frozen before the publish-time parameter gate also 503s
    /// forever once made live ([`crate::serving::ServingError::ParamGateStale`]),
    /// so it looks like the same shape. It is not, and the difference is the one
    /// that matters to whoever is holding the incident: `param_gate_stale`'s
    /// message tells the operator to RE-PUBLISH, and re-publishing works — the
    /// gate stamps the new version and the endpoint comes back. `governance_stale`
    /// cannot say that, because the publish it would send them to is refused by
    /// the very rule that produced it. Refusing at the point of action buys the
    /// most where the serve-time message can offer no way out; adding
    /// `param_gate` here would take a version offline at rollback that the
    /// operator could otherwise revive with the next thing they were told to do.
    ///
    /// # A snapshot this cannot READ is neither a refusal nor a 500
    ///
    /// The first version answered `Internal` on a snapshot that would not
    /// deserialise, reasoning that the only writer is `publish` so anything else
    /// is corruption worth naming. That is a worse trade than it looks, and a
    /// test caught it: such a version cannot serve at all —
    /// `serving::router::live_snapshot` already answers 500 for it — so making
    /// it live changes nothing this route is responsible for, while refusing the
    /// rollback takes away the action an operator reaches for during an
    /// incident, on a route that used to work. This check is for a version that
    /// is valid and has DRIFTED; a version whose snapshot cannot be read is a
    /// different failure, not one rollback caused and not one rollback can fix.
    ///
    /// Nor is it a hole worth guarding: writing an unreadable snapshot needs
    /// direct database access, which is already past every gate in this file.
    async fn check_version_still_publishable(
        &self,
        workspace: Uuid,
        raw: serde_json::Value,
    ) -> Result<()> {
        let Ok(snap) = serde_json::from_value::<EndpointSnapshot>(raw) else {
            return Ok(());
        };
        let Some(conn) = self
            .connection_safety_opt(workspace, snap.database_id)
            .await?
        else {
            return Ok(());
        };
        let conflict = |e: ValidationError| {
            Error::Conflict(format!(
                "this version cannot be made live as things stand: {e}"
            ))
        };
        // Governance first, for the same reason `validate_endpoint` puts the
        // cache rule first there and this one second: the ordering names which
        // fix the operator hears about, and here the two are the reverse of
        // publish's case. At publish the author is editing the DRAFT, so the
        // rule about a field they are holding comes first; at rollback the
        // version is immutable and BOTH fixes are on the database, so the one
        // that decides whether the data may be read at all leads.
        check_governed_connection(conn).map_err(conflict)?;
        check_cache_scope(&snap.cache, conn.delegated, conn.impersonate).map_err(conflict)?;
        Ok(())
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
    ///
    /// # The two LIVE-CONNECTION rules are re-checked, because rollback is a
    /// publish decision wearing different clothes
    ///
    /// [`check_governed_connection`] and [`check_cache_scope`] both read columns
    /// on `query.databases` that move independently of any version. A version
    /// frozen when the database was ungoverned, or when the connection did not
    /// run per principal, is a version `publish` would REFUSE today — and
    /// rollback used to make exactly that version live, whereupon the serving
    /// path answered `503 governance_stale` (or `cache_scope_stale`) to every
    /// consumer, forever, with the explanation arriving nowhere near the act
    /// that caused it. The operator's next move is a *different* endpoint's
    /// settings page, and nothing told them so.
    ///
    /// So both are asked here, against the TARGET version's frozen snapshot and
    /// the connection as it is right now, and a refusal is a `409` carrying the
    /// publish-time sentence. `Conflict`, not `InvalidInput`: the body named a
    /// version that exists and belongs to this endpoint, so nothing about the
    /// request is malformed — the endpoint is in a state this act cannot be
    /// asked of, which is the same thing the trash case answers here.
    ///
    /// **No transaction, and that is not an oversight.** `endpoint_versions` is
    /// immutable, so the snapshot read below cannot be stale by the time the
    /// UPDATE runs; and the connection columns are LIVE, so no lock held here
    /// could stop them moving a millisecond after commit — which is precisely
    /// why [`crate::serving::governance_allows_serving`] exists and stays the
    /// authority. This check is about telling the operator at the point of
    /// action, not about making the window smaller.
    ///
    /// A database row that has been DELETED is not refused, for the reason
    /// [`Self::connection_safety_opt`] gives.
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
        // Resolve the target's SNAPSHOT first, under exactly the predicates the
        // UPDATE below uses, so a version this cannot see is not one the UPDATE
        // would have written either. `None` here is left to the UPDATE to
        // report: it owns the 404/409 split, and duplicating that decision is
        // how the two answers drift apart.
        let target_snapshot: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT v.snapshot \
               FROM dataapi.endpoint_versions v \
               JOIN dataapi.endpoints e ON e.id = v.endpoint_id \
              WHERE e.id = $1 AND e.workspace_id = $2 \
                AND e.deleted_at IS NULL \
                AND (v.version = $3 OR v.id = $4)",
        )
        .bind(id)
        .bind(workspace)
        .bind(number)
        .bind(version_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        // What this rollback does to the consumers of the version it is moving
        // OFF, classified by the same function `publish` uses.
        //
        // It does not gate anything and must not: rollback is usually someone
        // ending an outage, and the reasoning above is why the one gate that
        // must not exist is this one. But not refusing is not the same as not
        // SAYING. A rollback from v1 to v0 breaks every consumer that coded
        // against v1, it writes no `endpoint_versions` row, and until this it
        // left no trace anywhere — so a later publish could compare against v0,
        // find nothing, and record an empty diff that an auditor would read as
        // "nobody was broken here". The caller gets the list, the audit event
        // carries it, and nothing is refused.
        //
        // Computed BEFORE the UPDATE, because it needs the pointer as it is now.
        // `None` when either snapshot is missing or unreadable — a rollback must
        // not fail because the thing it was going to TELL someone could not be
        // computed, which is the opposite of `publish`'s fail-closed direction
        // and for the opposite reason: there, the diff decides whether the write
        // happens at all.
        let live_snapshot: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT v.snapshot FROM dataapi.endpoint_versions v                JOIN dataapi.endpoints e ON e.live_version_id = v.id               WHERE e.id = $1 AND e.workspace_id = $2",
        )
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let moves_consumers_off: Option<Vec<crate::contract::ContractChange>> =
            match (&target_snapshot, live_snapshot) {
                (Some(to), Some(from)) => match (
                    serde_json::from_value::<EndpointSnapshot>(from),
                    serde_json::from_value::<EndpointSnapshot>(to.clone()),
                ) {
                    (Ok(from), Ok(to)) => {
                        Some(crate::contract::compare_contracts(&from, &to).breaking)
                    }
                    _ => None,
                },
                _ => None,
            };
        // The URL the target version would take, for the conflict message
        // below; read before the snapshot is handed on.
        let target_route = |k: &str| {
            target_snapshot
                .as_ref()
                .and_then(|s| s.get(k))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_owned()
        };
        let (target_method, target_path) = (target_route("method"), target_route("path"));
        if let Some(raw) = target_snapshot {
            self.check_version_still_publishable(workspace, raw).await?;
        }
        // The route moves with the pointer, as it does in `publish` (KI-023): a
        // rollback serves the older version on the URL that version was
        // published at. `COALESCE` only for a snapshot missing the keys, which
        // publish never writes; such a version cannot serve anyway (see this
        // function's doc on unreadable snapshots), so it keeps the draft's URL
        // rather than failing the rollback.
        let row: Option<(Uuid, i32, String)> = sqlx::query_as(
            "UPDATE dataapi.endpoints e SET live_version_id = v.id, \
                    live_method = COALESCE(v.snapshot->>'method', e.method), \
                    live_path = COALESCE(v.snapshot->>'path', e.path), \
                    updated_at = now() \
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
        .map_err(|e| live_route_err(e, &target_method, &target_path))?;
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
            // A rollback re-serves a version that was published once already, so
            // it records nothing on any version row and this field — which is
            // read back out of `endpoint_versions.breaking_changes` everywhere
            // else — stays `None`. What it DID to the consumers of the version
            // it moved off is `rollback_breaks`, a different question with a
            // different name so the two cannot be read as one.
            breaking_changes: None,
            compared_against: None,
            rollback_breaks: moves_consumers_off,
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
    /// The version a consumer is receiving right now, if any: its id and its
    /// frozen snapshot.
    ///
    /// **Reached through `endpoints.live_version_id`, never through
    /// `MAX(version)`.** They are the same row until the first rollback and
    /// different rows afterwards, and it is the live one every question in
    /// package 6 is about.
    ///
    /// `None` means the endpoint has never been published — or, after a `trash`
    /// followed by nothing, still points at nothing. Either way there is no
    /// contract to break.
    ///
    /// Read WITHOUT a lock, because its one caller is `POST /validate`, which
    /// answers a question about right now and writes nothing. `publish` issues
    /// its own copy of this SELECT inside its transaction, after `FOR UPDATE` on
    /// the endpoint row — the answer there has to be pinned for the length of
    /// the insert, and this one does not.
    pub async fn live_snapshot(
        &self,
        workspace: Uuid,
        id: Uuid,
    ) -> Result<Option<(Uuid, EndpointSnapshot)>> {
        let row: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT v.id, v.snapshot FROM dataapi.endpoint_versions v \
               JOIN dataapi.endpoints e ON e.live_version_id = v.id \
              WHERE e.id = $1 AND e.workspace_id = $2",
        )
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.map(|(version_id, snapshot)| {
            let snapshot: EndpointSnapshot = serde_json::from_value(snapshot).map_err(|e| {
                Error::Internal(anyhow::anyhow!("live snapshot is not readable: {e}"))
            })?;
            Ok((version_id, snapshot))
        })
        .transpose()
    }

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
            Option<serde_json::Value>,
            Option<i32>,
        );
        // `LEFT JOIN`, and the baseline is resolved to its NUMBER here rather
        // than in the caller: `compared_against` is a uuid, the history renders
        // numbers, and asking a reader to map one to the other is how the
        // baseline stops being read at all.
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT v.id, v.version, v.note, COALESCE(e.live_version_id = v.id, false), \
                    v.created_by, v.created_at, v.snapshot, v.breaking_changes, b.version \
               FROM dataapi.endpoint_versions v \
               JOIN dataapi.endpoints e ON e.id = v.endpoint_id \
               LEFT JOIN dataapi.endpoint_versions b ON b.id = v.compared_against \
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
                |(
                    id,
                    version,
                    note,
                    is_live,
                    created_by,
                    created_at,
                    snapshot,
                    breaking,
                    compared_against_version,
                )| {
                    Ok(DataApiEndpointVersionDto {
                        id,
                        version,
                        note,
                        is_live,
                        created_by,
                        created_at,
                        compared_against_version,
                        // Same rule as the snapshot one line down: a stored
                        // value that will not read is a 500, never a silent
                        // default. This one is the record of what an author
                        // accepted, and defaulting it to "nothing" would erase
                        // exactly the fact it exists to keep.
                        breaking_changes: breaking
                            .map(|b| {
                                serde_json::from_value(b).map_err(|e| {
                                    Error::Internal(anyhow::anyhow!(
                                        "stored breaking changes for version {version} are not \
                                         readable: {e}"
                                    ))
                                })
                            })
                            .transpose()?,
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
        validate_preview(
            &StatementSource::Sql(sql.to_owned()),
            &SiblingStatements::default(),
            params,
            output,
        )
    }

    /// The statement on screen, as the request now carries it: the author's own
    /// text. The scripted case has its own tests below.
    fn written(sql: &str) -> StatementSource {
        StatementSource::Sql(sql.to_owned())
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
            .map(|i| StatementSpec::from_sql(format!("s{i}"), sql.clone()))
            .collect();

        let started = std::time::Instant::now();
        let got = validate_endpoint(
            &statements,
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
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

    /// One scripted statement, through the real gate.
    ///
    /// `validate_endpoint` is what `POST …/validate` and `EndpointStore::publish`
    /// both go through, so a rule these tests see is a rule both entry points
    /// see. Nothing is stubbed and nothing is re-implemented: the only thing
    /// this adds to a call an author's request would make is the boilerplate
    /// around it.
    fn validate_generated(
        script: &str,
        params: &[ParamSpec],
    ) -> std::result::Result<Validated, ValidationError> {
        validate_endpoint(
            &[StatementSpec::from_script("rows", script)],
            params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
    }

    /// **THE test of package S Task 3.**
    ///
    /// `;` is refused for hand-written SQL at three places — `reject_unscannable_bytes`
    /// on the raw definition, the comment/literal scan, and again on the rendered
    /// output (`crate::render::check_rendered_statement`) — and a script must not
    /// be a way around any of them. The one refusal this asserts is the FIRST of
    /// the three, which is what says the generated text entered the pipeline at
    /// the top rather than downstream of a rule.
    ///
    /// Asserted with `assert_eq!` on the arm, not `is_err()`: bypassing the
    /// definition scan for generated text alone still fails this call, because
    /// the render-time test refuses the same bytes — with a DIFFERENT error. An
    /// `is_err()` here would go green on that mutation and report a gate that is
    /// no longer where it is documented to be.
    #[test]
    fn a_script_cannot_produce_a_multi_statement_definition() {
        let script = "def build_sql(params):\n  return \"SELECT 1; DROP TABLE t\"";
        let err = validate_generated(script, &[]).expect_err("two statements");
        assert_eq!(err, ValidationError::MultipleStatements);
    }

    /// `#` is the other byte layer 1 refuses outright, and for a measured reason:
    /// StarRocks accepts and ignores it while Trino has no such token, so any
    /// meaning read into it is a guess the engine does not share. Generated text
    /// is held to it identically.
    #[test]
    fn a_script_cannot_produce_a_comment_marker_the_scanner_forbids() {
        let script = "def build_sql(params):\n  return \"SELECT 1 # not a comment here\"";
        let err = validate_generated(script, &[]).expect_err("a `#` is refused");
        assert_eq!(err, ValidationError::HashNotAllowed);
    }

    /// **A script cannot produce a statement that is not a `SELECT`.**
    ///
    /// The sharpest of the three, and the one with no backstop behind it:
    /// `crate::render::check_rendered_statement` re-tests `;` and `#` on every
    /// render, so a definition that slipped past the definition scan carrying
    /// either is still refused later — measured, by running exactly that
    /// mutation. Nothing re-tests the HEAD WORD. Preview and describe execute
    /// author SQL under `data_api_endpoint:manage`, which is not
    /// `sqllab:write` — [`check_single_select`]'s own doc comment calls that
    /// authorization rather than tidiness — so a generated `DELETE` reaching an
    /// engine is a role holding an authoring permission getting arbitrary write
    /// access to it.
    #[test]
    fn a_script_cannot_produce_a_statement_that_is_not_a_select() {
        let script = "def build_sql(params):\n  return \"DELETE FROM t\"";
        assert_eq!(
            validate_generated(script, &[]).expect_err("a DELETE is not publishable"),
            ValidationError::NotASelect("DELETE".into())
        );
    }

    /// An undeclared `{{ }}` in generated SQL fails exactly as it would in
    /// written SQL — same rule, same arm, same name in the message.
    #[test]
    fn a_generated_statement_obeys_the_parameter_rules() {
        let script = "def build_sql(params):\n  return \"SELECT {{ nope }}\"";
        assert_eq!(
            validate_generated(script, &[]).expect_err("`nope` is not declared"),
            ValidationError::UndeclaredPlaceholder("nope".into())
        );
    }

    /// **The RENDER gate reaches every template, not only the baseline.**
    ///
    /// Task 5 left this deliberately: it moved the scans to every template and
    /// left `check_render_rules` on the baseline, because an alternate was
    /// described but never SERVED, so no engine could receive one. Package S
    /// Task 6 is what makes serving generate per request, and a branch the
    /// representative values do not take is then a statement a real call sends.
    ///
    /// The script below emits `ORDER BY {{ sort | ident }}` only when `region`
    /// is absent, which is the arm the `None` probe takes and the baseline does
    /// not. `| ident` writes an `allowed` entry into the statement as TEXT, so
    /// `check_ident_values` requires every entry to be identifier-shaped —
    /// `"b; DROP TABLE t"` is not, and that value reaches a caller who simply
    /// omits `region`.
    ///
    /// Asserted on the exact refusal rather than on `is_err()`. Task 3's
    /// Mutation A is why: with the gate bypassed, `;` and `#` still failed —
    /// through a DIFFERENT check — so an `is_err()` assertion here would go
    /// green with the loop back on the baseline.
    #[test]
    fn the_render_gate_reaches_a_template_the_representative_values_do_not() {
        let script = "def build_sql(params):\n  \
                      if params[\"region\"] == None:\n    \
                      return \"SELECT n FROM t WHERE 1=1 {% if region %} AND r = {{ region }}\
                      {% endif %} ORDER BY {{ sort | ident }}\"\n  \
                      return \"SELECT n FROM t WHERE 1=1 {% if region %} AND r = {{ region }}\
                      {% endif %} ORDER BY {{ sort }}\"";
        let sort = ParamSpec {
            allowed: Some(vec!["a".into(), "b; DROP TABLE t".into()]),
            required: true,
            ..param("sort", ParamType::Enum)
        };
        let params = vec![string_param("region"), sort];

        // The control comes FIRST, because it is what says the refusal below is
        // about the alternate template: the same script with an `allowed` list
        // that is identifier-shaped publishes.
        let mut safe = params.clone();
        safe[1].allowed = Some(vec!["a".into(), "b".into()]);
        assert!(
            validate_generated(script, &safe).is_ok(),
            "the script itself is publishable; only the `allowed` list is not"
        );

        assert_eq!(
            validate_generated(script, &params)
                .expect_err("an `allowed` value that is not identifier-shaped reaches the text"),
            ValidationError::EnumValueNotIdentifier {
                param: "sort".into(),
                value: "b; DROP TABLE t".into(),
            }
        );
    }

    /// The control. Without it every assertion above passes on a definition that
    /// could never publish for an unrelated reason — a scripted statement that
    /// is simply always refused would satisfy all three.
    #[test]
    fn a_generated_statement_that_is_fine_publishes() {
        let script = "def build_sql(params):\n  return \"SELECT CAST({{ n }} AS INTEGER) AS out\"";
        assert!(validate_generated(script, &[param("n", ParamType::Number)]).is_ok());
    }

    /// A generated statement and a written one, side by side, and the generated
    /// one carries the `{{ }}` the script emitted.
    ///
    /// Two claims in one endpoint because they are the same claim: the text a
    /// script produced is indistinguishable downstream from text an author
    /// typed. The braces surviving verbatim is what makes the caller's value a
    /// BIND rather than SQL text — spec §2.1, and the whole reason a script
    /// returns a template instead of finished SQL.
    #[test]
    fn a_generated_statement_is_indistinguishable_from_a_written_one() {
        let statements = vec![
            StatementSpec::from_sql("rows", "SELECT id FROM t WHERE region = {{ region }}"),
            StatementSpec::from_script(
                "total",
                "def build_sql(params):\n  \
                 return \"SELECT count(*) AS n FROM t WHERE region = {{ region }}\"",
            ),
        ];
        // Constrained, and this fixture is where package 5's cost is visible:
        // the script here only ever emits `{{ region }}`, so the value would
        // have been a bind — and the rule still requires the `pattern`, because
        // no instrument can promise that about the NEXT edit of the script. See
        // `check_script_parameter_constraints`.
        let params = vec![string_param("region")];
        let validated = validate_endpoint(
            &statements,
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
        .expect("a written and a generated statement publish together");
        assert_eq!(
            validated.sql.get("total"),
            Some("SELECT count(*) AS n FROM t WHERE region = {{ region }}"),
            "the placeholder must reach the renderer intact, or the value stops being a bind"
        );
        assert_eq!(
            validated.sql.get("rows"),
            Some("SELECT id FROM t WHERE region = {{ region }}"),
            "a written statement's text is carried through the same lookup"
        );
    }

    /// A script that fails is the AUTHOR's 400, and it names the statement.
    ///
    /// `transform::TransformError` knows about one script; an endpoint has up to
    /// `MAX_STATEMENTS` of them, so the name has to be added here or an author
    /// with several scripted statements is told only that "a" script failed.
    #[test]
    fn a_script_that_fails_is_refused_by_the_statements_name() {
        let statements = vec![
            StatementSpec::from_sql("rows", "SELECT id FROM t"),
            StatementSpec::from_script("total", "def build_sql(params):\n  fail(\"nope\")"),
        ];
        let err = validate_endpoint(
            &statements,
            &[],
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
        .expect_err("the script calls fail()");
        match &err {
            ValidationError::StatementScriptFailed { statement, error } => {
                assert_eq!(statement, "total");
                assert!(
                    error.contains("nope"),
                    "must carry the script's own words: {error}"
                );
            }
            other => panic!("expected StatementScriptFailed, got {other:?}"),
        }
    }

    /// **Publish inherits the emitted template's length cap without owning it**
    /// (package S Task 8.5, spec S5).
    ///
    /// `crate::transform::MAX_GENERATED_SQL_BYTES` is enforced inside
    /// `build_sql`, the one generator publish, preview and serving all reach —
    /// so there is no length check in this file to keep in step, and this test
    /// is what says the inheritance is real rather than assumed. It asserts the
    /// rule's own sentence arrives, because `StatementScriptFailed` passing the
    /// message through is the whole mechanism: an author over the cap must read
    /// how much to cut, not "the script failed".
    ///
    /// The endpoint is otherwise publishable — no parameters, one statement, a
    /// script that runs — so the refusal is about the template's size and
    /// nothing else.
    #[test]
    fn a_generated_template_over_the_cap_is_an_authors_refusal_at_publish() {
        let over = crate::transform::MAX_GENERATED_SQL_BYTES + 1;
        let statements = vec![StatementSpec::from_script(
            "rows",
            format!("def build_sql(params):\n  return \"SELECT \" + \"z\" * {over}"),
        )];
        let err = validate_endpoint(
            &statements,
            &[],
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
        .expect_err("the script emits more than the cap");
        match &err {
            ValidationError::StatementScriptFailed { statement, error } => {
                assert_eq!(statement, "rows");
                assert!(
                    error.contains(&crate::transform::MAX_GENERATED_SQL_BYTES.to_string())
                        && error.contains("byte template"),
                    "the cap's own sentence, with both numbers, must reach the \
                     author: {error}"
                );
            }
            other => panic!("expected StatementScriptFailed, got {other:?}"),
        }
    }

    /// **A probed branch over the cap is refused at PUBLISH, because publish
    /// built that template and measured it.**
    ///
    /// The baseline here is a perfectly good statement, so nothing in
    /// `build_after` refuses it — the oversized text is emitted only under the
    /// `None` probe `sentinel_probes` supplies second for every parameter. That
    /// makes it a branch publish GENERATED, and a refusal it holds the evidence
    /// for.
    ///
    /// `GeneratedSql::probe` reads a failed evaluation as inconclusive
    /// (`Err(_) => varied = true`) and that is right for a script that RAISES on
    /// a synthetic value it will never be sent. A length refusal is not that
    /// kind of failure: the script ran to completion and returned a template
    /// this process measured. Swallowing it published an endpoint green and left
    /// a real caller on that branch with a 500 — which is what this test would
    /// go back to asserting if the arm were removed.
    ///
    /// It names the PARAMETER as well as the statement, because an author whose
    /// baseline is fine needs to know which branch to look at, and a publish
    /// refusal is an author-facing message where that detail belongs.
    #[test]
    fn a_probed_branch_over_the_cap_is_refused_rather_than_swallowed() {
        let statements = vec![StatementSpec::from_script(
            "rows",
            "def build_sql(params):\n  \
             if params[\"region\"] == None:\n    \
             return \"SELECT \" + \"z\" * 70000\n  \
             return \"SELECT n FROM t WHERE region = {{ region }}\"",
        )];
        let params = vec![string_param("region")];
        let err = validate_endpoint(
            &statements,
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
        .expect_err("the None probe emits more than the cap");
        match &err {
            ValidationError::StatementScriptFailed { statement, error } => {
                assert_eq!(statement, "rows");
                assert!(
                    error.contains("region"),
                    "the author's baseline is fine — the message must say which \
                     branch: {error}"
                );
                assert!(
                    error.contains(&crate::transform::MAX_GENERATED_SQL_BYTES.to_string()),
                    "and it must still carry the cap: {error}"
                );
            }
            other => panic!("expected StatementScriptFailed, got {other:?}"),
        }
    }

    // ---- Package S Task 4: the sentinel probe -----------------------------

    /// A `String` parameter a SCRIPTED endpoint may declare — which, since
    /// package 5, means one carrying a `pattern` over the safe alphabet.
    ///
    /// The pattern is here rather than in each test because every test below is
    /// about the probe, the contract or the cost, and none of them is about
    /// package 5's rule; a fixture that could not publish would make each of
    /// them assert the wrong refusal. `SENTINEL` is itself drawn from this
    /// alphabet (`the_sentinel_cannot_collide_with_an_ordinary_value`), so the
    /// probe still substitutes it and the gate below still fires.
    ///
    /// [`unconstrained_string_param`] is the other one, for the tests that ARE
    /// about the rule.
    fn string_param(name: &str) -> ParamSpec {
        ParamSpec {
            pattern: Some("^[A-Za-z0-9_]*$".into()),
            ..param(name, ParamType::String)
        }
    }

    /// A `String` parameter with no constraint at all — publishable on a
    /// written endpoint, refused on a scripted one since package 5.
    fn unconstrained_string_param(name: &str) -> ParamSpec {
        param(name, ParamType::String)
    }

    /// The Task 4 tests' entry point, and it is [`validate_generated`] — the
    /// same `validate_endpoint` that `POST …/validate` and
    /// `EndpointStore::publish` both go through.
    ///
    /// Named for what these tests are about rather than aliased away, because
    /// the thing under test is not a probe function in isolation: the sentinel
    /// gate and spec §2.4's behavioural reading of `UnreferencedParameter` only
    /// compose correctly at this level. A test that called
    /// [`GeneratedSql::probe`] directly would pass with the union in
    /// `validate_endpoint` deleted, and the accepting case below would then be
    /// refused in production.
    fn probe_generated(
        script: &str,
        params: &[ParamSpec],
    ) -> std::result::Result<Validated, ValidationError> {
        validate_generated(script, params)
    }

    /// **THE test of package S Task 4.** A script that concatenates a caller's
    /// value into the SQL TEXT is refused, and the refusal names the parameter.
    ///
    /// The arm is asserted, not merely `is_err()`, and the reason is MEASURED
    /// rather than reasoned: deleting the containment test and running this
    /// prints
    ///
    /// ```text
    /// Validated { sql: GeneratedSql({"rows": GeneratedStatement {
    ///   sql: "SELECT * FROM sales_x", varies_with: ["region"] }}) }
    /// ```
    ///
    /// — the interpolating definition **publishes clean**. Nothing else refuses
    /// it. In particular it does NOT fall back on
    /// [`ValidationError::UnreferencedParameter`], because spec §2.4 correctly
    /// finds `region` referenced (varying it changes the emitted template), so
    /// this test is the whole of what stands between that script and an
    /// endpoint.
    ///
    /// The arm is still what is asserted, and deliberately. `is_err()` — or the
    /// plan's `to_string().contains("region")` — would be satisfied by any
    /// refusal naming the parameter, and `UnreferencedParameter`'s message
    /// contains the word `region` too. Which of the two halves happens to fire
    /// is not a thing this test should be able to be wrong about.
    #[test]
    fn a_script_that_concatenates_a_parameter_into_the_sql_is_refused() {
        let script =
            "def build_sql(params):\n  return \"SELECT * FROM sales_\" + params[\"region\"]";
        let err = probe_generated(script, &[string_param("region")]).expect_err("interpolated");
        assert_eq!(
            err,
            ValidationError::ParameterValueInSqlText {
                statement: "rows".into(),
                parameter: "region".into(),
            }
        );
        assert!(
            err.to_string().contains("region"),
            "must name the parameter: {err}"
        );
    }

    /// The discriminating half. Without it the rule could be "refuse every
    /// script that mentions a parameter", which would refuse the feature.
    #[test]
    fn a_script_that_passes_a_parameter_through_a_placeholder_is_accepted() {
        let script = "def build_sql(params):\n  return \"SELECT * FROM t WHERE r = {{ region }}\"";
        assert!(probe_generated(script, &[string_param("region")]).is_ok());
    }

    /// The second discriminating half, and the one spec §2.4 was written for: a
    /// script that reads a parameter for CONTROL FLOW and emits no `{{ grain }}`
    /// publishes.
    ///
    /// Before §2.4 this was refused by `UnreferencedParameter`, which reads the
    /// emitted text for `{{ grain }}` and finds none — the written-SQL test
    /// applied to generated text, refusing exactly the case this package exists
    /// to enable. The probe answers the same question behaviourally: varying
    /// `grain` changes the emitted template, so it is read.
    #[test]
    fn a_script_that_branches_on_a_parameter_without_emitting_it_is_accepted() {
        let script = "def build_sql(params):\n  \
                      if params[\"grain\"] == \"daily\":\n    return \"SELECT 1 FROM sales_daily\"\n  \
                      return \"SELECT 1 FROM sales_hourly\"";
        assert!(probe_generated(script, &[enum_param("grain", &["daily", "hourly"])]).is_ok());
    }

    /// If the sentinel were something an author might legitimately write, this
    /// gate would refuse honest scripts. Assert its shape, not just its use.
    #[test]
    fn the_sentinel_cannot_collide_with_an_ordinary_value() {
        assert!(
            SENTINEL.len() >= 16,
            "too short to be improbable: {SENTINEL}"
        );
        assert!(
            SENTINEL
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "the sentinel must survive being a parameter value unchanged: {SENTINEL}"
        );
    }

    /// A parameter NOTHING reads is still refused — the rule §2.4 rewrote, not
    /// the rule §2.4 removed.
    ///
    /// Without this, "referenced" could mean "declared" and every test above
    /// would still pass. The script is a constant, so no substitution changes
    /// the template and no `{{ }}` names the parameter: both halves of the union
    /// are empty, which is the only way this refusal can now be reached for a
    /// scripted statement.
    #[test]
    fn a_parameter_a_script_never_reads_is_still_unreferenced() {
        let script = "def build_sql(params):\n  return \"SELECT 1 AS a\"";
        assert_eq!(
            probe_generated(script, &[string_param("region")]).expect_err("nothing reads it"),
            ValidationError::UnreferencedParameter("region".into())
        );
    }

    /// A parameter a script reads only for its PRESENCE is referenced.
    ///
    /// This is what the `None` probe buys, and it is the reason
    /// [`sentinel_probes`] returns two values rather than one: `"x"` and the
    /// sentinel are both truthy, so a single-value probe would leave this
    /// script's template unchanged and refuse it as `UnreferencedParameter` —
    /// a gate refusing an honest script, which is the direction this task must
    /// not fail in.
    #[test]
    fn a_parameter_a_script_reads_only_for_its_presence_is_referenced() {
        let script = "def build_sql(params):\n  \
                      if params[\"region\"]:\n    return \"SELECT 1 FROM t WHERE r IS NOT NULL\"\n  \
                      return \"SELECT 1 FROM t\"";
        assert!(
            probe_generated(script, &[string_param("region")]).is_ok(),
            "a script branching on whether a parameter was supplied reads it"
        );
    }

    /// A NUMBER concatenated into the text is accepted, and that is a decision
    /// rather than an oversight.
    ///
    /// Trino will not bind `LIMIT`, so building that clause is one of the
    /// reasons to generate SQL at all. A number parameter's caller bytes are
    /// digits — they cannot open a literal, close one, or form an identifier —
    /// so there is nothing here for the gate to protect, and refusing it would
    /// cost a safe capability the feature exists to provide. See
    /// [`sentinel_probes`].
    #[test]
    fn a_script_may_put_a_numeric_parameter_into_the_sql_text() {
        let script = "def build_sql(params):\n  \
                      return \"SELECT id FROM t LIMIT \" + str(params[\"n\"])";
        assert!(
            probe_generated(script, &[param("n", ParamType::Number)]).is_ok(),
            "a number in the text is not an injection, and `LIMIT` cannot be bound on Trino"
        );
    }

    /// A `Date` in the SQL text is accepted, for the same measured reason a
    /// number is — and this test exists because the first draft of this task
    /// refused it.
    ///
    /// The draft's rationale was that a `Date` is "a parameter whose bytes the
    /// caller writes freely". `crate::serving::params::coerce` says otherwise: a
    /// date is parsed by chrono and **re-formatted** through
    /// `normalize_temporal_bound`, so the caller's spelling never reaches the
    /// engine — a normalisation of it does, in one closed grammar. That makes it
    /// exactly as constrained as the `Number` the gate expressly permits, and
    /// refusing one while permitting the other was an inconsistency dressed as
    /// caution.
    ///
    /// The test is the assertion; [`sentinel_probes`] carries the reasoning.
    #[test]
    fn a_script_may_put_a_temporal_parameter_into_the_sql_text() {
        let script = "def build_sql(params):\n  \
                      return \"SELECT id FROM t WHERE d >= DATE '\" + params[\"d\"] + \"'\"";
        assert!(
            probe_generated(script, &[param("d", ParamType::Date)]).is_ok(),
            "a date's bytes are re-serialised by `coerce`, so they cannot open or close a literal"
        );
    }

    /// **Escape route 2, asserted rather than only commented.**
    ///
    /// The script refuses the sentinel — it is 31 characters and the script
    /// accepts at most 4 — so both probes RAISE, both are inconclusive, and the
    /// concatenation on the accepting path is never observed.
    ///
    /// This is the cost of treating a failed probe as inconclusive, which is
    /// still the right call: refusing every script that raises under a synthetic
    /// value it will never be sent would refuse honest scripts wholesale. Two
    /// escape routes, both written down, is what "the gate's honest limits" has
    /// to mean.
    ///
    /// # Changed by package 5, and the change is in the SECOND half only
    ///
    /// Until package 5 this test ended at the first assertion and its sentence
    /// was "the definition publishes with `"sales_" + params["region"]`
    /// intact". The probe is untouched and that first assertion is unchanged —
    /// **it still finds nothing**, which is what the doc comment on
    /// [`GeneratedSql::probe`] disclaims and what this test exists to pin. What
    /// changed is that the probe is no longer the only rule the definition has
    /// to pass: [`check_script_parameter_constraints`] refuses an unconstrained
    /// `String` on a scripted endpoint outright, so the endpoint no longer
    /// publishes — for a reason that has nothing to do with what the probe saw.
    ///
    /// The third assertion is the point of the package: with a `pattern` over
    /// the safe alphabet the SAME script publishes, and the concatenation is
    /// harmless because no caller can put a quote, a `;` or a space into
    /// `sales_…` in the first place.
    #[test]
    fn a_script_that_rejects_the_sentinel_before_concatenating_also_defeats_the_probe() {
        let script = "def build_sql(params):\n  \
                      r = params[\"region\"]\n  \
                      if r == None or len(r) > 4:\n    fail(\"bad region\")\n  \
                      return \"SELECT * FROM sales_\" + r";
        let statements = vec![StatementSpec::from_script("rows", script)];
        assert!(
            GeneratedSql::build(&statements, &[unconstrained_string_param("region")]).is_ok(),
            "if this now FAILS the probe grew a capability its doc comment disclaims — update \
             the spec and the comment, do not just delete this test"
        );
        assert!(
            matches!(
                probe_generated(script, &[unconstrained_string_param("region")]),
                Err(ValidationError::StringParameterNeedsPattern { .. })
            ),
            "package 5: the probe still sees nothing, and the DEFINITION is refused anyway"
        );
        assert!(
            probe_generated(script, &[string_param("region")]).is_ok(),
            "and a constrained parameter publishes the same script, because the value is a \
             bare word wherever the script puts it"
        );
    }

    /// **What the probe does not prove, asserted rather than only commented.**
    ///
    /// The same concatenation as the refused test above, with `.upper()` in the
    /// way. The sentinel comes back transformed and the containment test finds
    /// nothing. That is spec §2.3's stated residue: the probe catches an author
    /// creating an injection hole by accident, not one who sets out to.
    ///
    /// Written down as a test because a documented absence that no test pins is
    /// a claim about behaviour nobody re-checks — and because the tempting
    /// "fix", scanning the emitted bytes harder, is the P4.5 mistake this whole
    /// gate is shaped to avoid.
    ///
    /// # Changed by package 5, and the change is in the SECOND half only
    ///
    /// The sentence "and the definition PUBLISHES" was true until package 5 and
    /// is now false — but **not because the probe grew a capability**. The first
    /// assertion is the old one, against [`GeneratedSql::probe`] itself, and it
    /// still holds: `.upper()` still defeats the containment test and always
    /// will. The definition is refused by a separate declarative rule
    /// ([`check_script_parameter_constraints`]) that never looks at the emitted
    /// bytes at all.
    ///
    /// The third assertion is why the rule works where a smarter probe could
    /// not: `.upper()` is exactly the transformation the safe alphabet is closed
    /// under, so with a `pattern` the same script publishes and `sales_` +
    /// anything the caller can send is still one bare word. See
    /// [`SAFE_ALPHABET`].
    #[test]
    fn a_script_that_transforms_the_value_first_defeats_the_probe() {
        let script =
            "def build_sql(params):\n  return \"SELECT * FROM sales_\" + params[\"region\"].upper()";
        let statements = vec![StatementSpec::from_script("rows", script)];
        assert!(
            GeneratedSql::build(&statements, &[unconstrained_string_param("region")]).is_ok(),
            "if this now FAILS the probe grew a capability its doc comment disclaims — update \
             the spec and the comment, do not just delete this test"
        );
        assert!(
            matches!(
                probe_generated(script, &[unconstrained_string_param("region")]),
                Err(ValidationError::StringParameterNeedsPattern { .. })
            ),
            "package 5: the probe still sees nothing, and the DEFINITION is refused anyway"
        );
        assert!(
            probe_generated(script, &[string_param("region")]).is_ok(),
            "and the transformation the probe cannot follow is one the alphabet is closed \
             under, so the constrained parameter publishes"
        );
    }

    // ---- Package 5: a script's caller values are bounded by construction ---

    /// One scripted statement, one written one, and the parameter list under
    /// test — the shape package 5's rule is about.
    fn scripted_endpoint(
        script: &str,
        params: &[ParamSpec],
    ) -> std::result::Result<Validated, ValidationError> {
        validate_endpoint(
            &[
                StatementSpec::from_sql("rows", "SELECT id FROM t"),
                StatementSpec::from_script("total", script),
            ],
            params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
    }

    /// A script that emits nothing but a placeholder for `region` — the most
    /// innocent scripted statement there is, and the one the rule still applies
    /// to.
    const BINDS_REGION: &str =
        "def build_sql(params):\n  return \"SELECT count(*) AS n FROM t WHERE r = {{ region }}\"";

    /// A pattern is enough exactly when its LANGUAGE is inside the safe
    /// alphabet, and these are the shapes an author writes.
    ///
    /// The classes matter as much as the literals: `[[:word:]]` is ASCII-only
    /// and is accepted, while `\w` is the same idea with Unicode switched on and
    /// is refused by the test below — a distinction no author would be told
    /// about by an error that said only "make it safer".
    #[test]
    fn a_pattern_whose_language_is_inside_the_safe_alphabet_is_accepted() {
        for pattern in [
            "^[A-Za-z0-9_]+$",
            "[a-z]*",
            "abc",
            "^(EU|US|APAC)$",
            "[a-z]{2,8}",
            "^(sales_)?[a-z0-9]+$",
            "[[:word:]]+",
            "^$",
            "[0-9]|[A-F]",
        ] {
            assert_eq!(
                pattern_confines_to_safe_alphabet(pattern),
                Ok(()),
                "`{pattern}` accepts only safe characters and must publish"
            );
        }
    }

    /// A pattern that can accept ONE character outside the alphabet is refused,
    /// and the refusal names that character.
    ///
    /// Every entry is a real shape an author writes and a real byte that matters
    /// in SQL: the quote that closes a literal, the backslash that escapes on
    /// StarRocks, the `-` that starts a comment, the `.` that reaches another
    /// schema, the space that ends a token so a keyword can follow. `\w` is
    /// here because it LOOKS like the safe alphabet and is not — with Unicode
    /// on it admits every word character in every script.
    #[test]
    fn a_pattern_that_admits_an_unsafe_character_is_refused_and_names_it() {
        for (pattern, witness) in [
            ("^[a-z']+$", '\''),
            (r"^[a-z\\]+$", '\\'),
            ("^[a-z-]+$", '-'),
            ("^[a-z.]+$", '.'),
            ("^[a-z ]+$", ' '),
            ("^[a-z;]+$", ';'),
            ("^.*$", '\0'),
        ] {
            assert_eq!(
                pattern_confines_to_safe_alphabet(pattern),
                Err(witness),
                "`{pattern}` admits `{witness}` and must be refused with it as the witness"
            );
        }
        assert!(
            pattern_confines_to_safe_alphabet(r"^\w+$").is_err(),
            "`\\w` is Unicode-aware and reaches far outside ASCII, however much it looks like \
             the safe alphabet"
        );
        assert!(
            pattern_confines_to_safe_alphabet("^[^']+$").is_err(),
            "a NEGATED class is everything-but, which is the widest thing an author can write"
        );
    }

    /// A range that spans two of the alphabet's blocks is refused at the GAP,
    /// not at its ends.
    ///
    /// `[A-z]` is the classic typo for "letters": both ends are safe characters,
    /// so a check that looked only at `start` and `end` would pass it — and it
    /// admits `[`, `\`, `]`, `^` and `` ` ``, including the backslash that
    /// escapes on StarRocks. The witness is the first character past the block
    /// `start` falls in, which is `[`.
    #[test]
    fn a_range_spanning_two_blocks_is_refused_at_the_gap() {
        assert_eq!(pattern_confines_to_safe_alphabet("^[A-z]+$"), Err('['));
        assert_eq!(pattern_confines_to_safe_alphabet("^[0-A]+$"), Err(':'));
        assert_eq!(pattern_confines_to_safe_alphabet("^[a-~]+$"), Err('{'));
    }

    /// A `pattern` that does not PARSE is refused as invalid, not as unsafe.
    ///
    /// `pattern_confines_to_safe_alphabet` answers `Ok` for a pattern it cannot
    /// parse, deliberately: `check_constraints` compiles the same string a few
    /// lines later and owns that refusal, and an author whose pattern has an
    /// unbalanced `(` must read about the `(` rather than about SQL safety. The
    /// ordering is the assertion — it is what makes the deliberate `Ok` above
    /// safe rather than a hole, since nothing can publish without the compile.
    #[test]
    fn an_unparseable_pattern_is_refused_as_invalid_rather_than_as_unsafe() {
        let broken = ParamSpec {
            pattern: Some("(".into()),
            ..param("region", ParamType::String)
        };
        assert_eq!(pattern_confines_to_safe_alphabet("("), Ok(()));
        match scripted_endpoint(BINDS_REGION, &[broken]) {
            Err(ValidationError::PatternInvalid { param, .. }) => assert_eq!(param, "region"),
            other => panic!("expected the compile to refuse it as invalid, got {other:?}"),
        }
    }

    /// **The breakout the alphabet analysis could not see.**
    ///
    /// [`anchored`] builds `^(?:{pattern})$` out of AUTHOR TEXT, and `coerce`
    /// enforces it with an UNANCHORED `is_match`. A pattern carrying `)` … `(`
    /// re-balances against that wrapper: `a)|(a` becomes `^(?:a)|(a)$`, where
    /// the two anchors have ended up on two different alternatives — `^(?:a)`
    /// bounds only the start, `(a)$` only the end, and with a third alternative
    /// (`a)|([a-z]+)|(a`) the middle one bounds nothing at all. Every character
    /// of it is inside [`SAFE_ALPHABET`], so
    /// [`pattern_confines_to_safe_alphabet`] answered `Ok`, publish returned
    /// 200, and a caller could then send arbitrary SQL
    /// through `"… sales_" + params["region"]` — the exact hole package 5
    /// claims to have removed, behind a green publish.
    ///
    /// Four assertions per pattern, in the order that makes the argument:
    ///
    /// 1. the ALPHABET analysis still answers `Ok` — it is sound about the
    ///    string it is handed, and it is not the leg that refuses this;
    /// 2. the anchored wrapper alone DOES admit the injection — the
    ///    vulnerability executed, not described, so this test cannot decay
    ///    into a restatement of the fix;
    /// 3. publish refuses the definition, naming the parameter and what to
    ///    fix ([`compile_pattern`]'s raw-parse requirement);
    /// 4. `coerce` — the serving half, and the only half a snapshot frozen
    ///    yesterday has — refuses the same value rather than accepting it.
    ///
    /// (3) and (4) together are the composition
    /// `the_constraint_the_gate_demands_is_the_one_coerce_enforces` performs,
    /// run over a pattern where it can FAIL: that test composes the same two
    /// halves over `^[A-Za-z0-9_]*$`, which no wrapper bug can make unsafe, and
    /// that is why this survived it. A fixture that can only pass is the shape
    /// this repository has shipped a green test over sixteen times.
    #[test]
    fn a_pattern_that_escapes_the_anchors_is_refused_and_coerce_agrees() {
        for (pattern, injection) in [
            ("a)|(a", "a' UNION SELECT password FROM users --"),
            ("a)|([a-z]+)|(a", "x'; DROP TABLE t --"),
            ("a)|([a-z]+)|(a", "'; DELETE FROM audit; --"),
        ] {
            assert_eq!(
                pattern_confines_to_safe_alphabet(pattern),
                Ok(()),
                "`{pattern}` is all safe characters: the alphabet analysis is not what refuses \
                 it, and a fix that only made THAT stricter would not have closed this"
            );
            let wrapper = regex::Regex::new(&anchored(pattern))
                .expect("a breakout re-balances the wrapper, so the anchored form COMPILES");
            assert!(
                wrapper.is_match(injection),
                "the vulnerability, run rather than described: `{}` matches `{injection}` \
                 exactly as `coerce` asks it to",
                anchored(pattern)
            );

            let spec = ParamSpec {
                pattern: Some(pattern.into()),
                ..param("region", ParamType::String)
            };
            match scripted_endpoint(BINDS_REGION, std::slice::from_ref(&spec)) {
                Err(ValidationError::PatternInvalid { param, detail }) => {
                    assert_eq!(param, "region");
                    assert!(
                        detail.contains("ON ITS OWN"),
                        "the author's pattern is a valid regex in their own tester, so the \
                         message has to name egret's anchoring as the thing it fails: {detail}"
                    );
                }
                other => panic!("`{pattern}` must not publish: {other:?}"),
            }
            assert_eq!(
                crate::serving::params::coerce(&spec, injection),
                Err(crate::serving::params::CoerceError::Malformed(
                    crate::serving::params::MalformedReason::PatternDoesNotCompile
                )),
                "and a snapshot frozen with `{pattern}` before this existed serves a redacted \
                 500 rather than the injection — `compile_pattern` is the one function both \
                 halves go through"
            );
        }
    }

    /// **`join` is on the "introduces bytes" side of [`SAFE_ALPHABET`], shown in
    /// the real evaluator rather than argued.**
    ///
    /// The closure list said "slicing, indexing, `reversed`, `strip`,
    /// `split` + `join`, `elems` — every character of the result was a
    /// character of the input", and that is false of `join`: the separator
    /// comes from AUTHOR text and is inserted once per gap, while the number of
    /// gaps is `len(pieces)` — the caller's, whenever the pieces came from
    /// their value.
    ///
    /// Everything here is a value the rule accepts. `q` carries the safe
    /// pattern; `1_OR_true` matches it; the probe is blind because [`SENTINEL`]
    /// itself contains `_`, so humanising it emits something that is not the
    /// sentinel and the containment test finds nothing. The endpoint publishes,
    /// and a caller then gets two SQL tokens out of one bare word.
    ///
    /// This pins the RESIDUE, like the two probe-bypass tests above it. The
    /// author typed the separator, so this is package S's accepted
    /// author-provenance limit rather than a new hole — but it is a limit the
    /// alphabet's own justification denied having, which is why it is executed
    /// here instead of described. **If this now FAILS, something narrowed what
    /// a script may do — say so in `SAFE_ALPHABET` and in the spec, do not just
    /// delete this test.**
    #[test]
    fn the_alphabet_does_not_survive_join_and_such_an_endpoint_still_publishes() {
        let script = "def build_sql(params):\n  \
                      return \"SELECT count(*) AS n FROM t WHERE flag = \" \
                      + \" \".join(params[\"q\"].split(\"_\"))";
        assert!(
            scripted_endpoint(script, &[string_param("q")]).is_ok(),
            "the sentinel carries `_`, so humanising it emits something that is not the \
             sentinel: nothing at publish sees this"
        );

        let mut params = crate::transform::GenerationParams::default();
        params.insert("q".to_string(), Some(&Param::Str("1_OR_true".to_string())));
        let sql = crate::transform::build_sql(script, &params)
            .expect("the script is the one that just published");
        assert!(
            sql.contains("flag = 1 OR true"),
            "one caller value, drawn entirely from the safe alphabet, became two SQL tokens \
             — which is exactly what `join` being on the closed-under list denied: {sql}"
        );
    }

    /// A pattern that is a complete regex on its own still publishes, including
    /// the ones whose parentheses look like the breakout above.
    ///
    /// **The direction the raw-parse requirement must not fail in.** It refuses
    /// a string that is not a regex; it must not refuse groups, alternation or
    /// nesting, which is most of what an author writes. Without this, the fix
    /// for the test above could have been "no `(` in a pattern" and the suite
    /// would still be green.
    #[test]
    fn a_balanced_pattern_still_publishes() {
        for pattern in [
            "^(EU|US|APAC)$",
            "(a)(b)",
            "^((a|b)+_)?[a-z]{2,8}$",
            "(?:a|b)",
            "^$",
        ] {
            assert!(
                compile_pattern(pattern).is_ok(),
                "`{pattern}` is a regular expression on its own and must compile"
            );
            assert!(
                scripted_endpoint(
                    BINDS_REGION,
                    &[ParamSpec {
                        pattern: Some(pattern.into()),
                        ..param("region", ParamType::String)
                    }]
                )
                .is_ok(),
                "`{pattern}` bounds the value to the safe alphabet and must publish"
            );
        }
    }

    /// The argument "the gate parses the SAME STRING the matcher compiles"
    /// rests on `regex` and `regex-syntax` being ONE parser, and `Cargo.toml`
    /// declares them as two independent carets.
    ///
    /// Nothing in the build says they must agree, so a `regex` bump that moved
    /// to a new `regex-syntax` minor would give the workspace two copies and
    /// two parsers: [`pattern_confines_to_safe_alphabet`] would then be reading
    /// one language while [`compile_pattern`] matched another, silently, with
    /// every test still green. The lock file is where that becomes visible, so
    /// it is asserted there.
    ///
    /// **If this fails, do not delete it and do not loosen the count** — align
    /// the two versions in `Cargo.toml` until the workspace resolves one copy
    /// of each, or the safe-alphabet argument has to be re-made against
    /// whatever the second parser accepts.
    #[test]
    fn regex_and_regex_syntax_resolve_to_one_parser() {
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .expect("the workspace lock file sits two directories above this crate");
        for name in ["regex", "regex-syntax"] {
            assert_eq!(
                lock.matches(&format!("name = \"{name}\"\n")).count(),
                1,
                "the workspace must resolve exactly one `{name}`, or the gate parses a \
                 different language from the one the matcher enforces"
            );
        }
    }

    /// **THE test of package 5.** A `String` parameter with no `pattern` cannot
    /// be declared on an endpoint where a script builds SQL.
    ///
    /// The script here is the innocent one — it emits `{{ region }}` and
    /// nothing else — because that is the case the rule is CONSERVATIVE about,
    /// and a fixture using a script that concatenated would be refused by the
    /// sentinel probe instead and could not tell whether this rule exists at
    /// all. The refusal must come from the rule, on a script that does nothing
    /// wrong.
    ///
    /// The message is asserted, not just the variant: it names the parameter, it
    /// names the statement that brought the rule, and it carries the recourse.
    /// A publish-time refusal is the one place an author is entitled to the
    /// detail — the serving path is unchanged and still leaks nothing.
    #[test]
    fn an_unconstrained_string_parameter_is_refused_on_a_scripted_endpoint() {
        let err = scripted_endpoint(BINDS_REGION, &[unconstrained_string_param("region")])
            .expect_err("a string parameter with no pattern, beside a generating statement");
        match &err {
            ValidationError::StringParameterNeedsPattern {
                parameter,
                statement,
                alphabet,
            } => {
                assert_eq!(parameter, "region");
                assert_eq!(
                    statement, "total",
                    "the SCRIPTED statement, not the written one"
                );
                assert_eq!(*alphabet, SAFE_ALPHABET_PROSE);
            }
            other => panic!("expected StringParameterNeedsPattern, got {other:?}"),
        }
        let message = err.to_string();
        assert!(
            message.contains("pattern") && message.contains("^[A-Za-z0-9_]+$"),
            "the author must be told what to write, not only that something is missing: {message}"
        );
        assert!(
            message.contains("BOUND"),
            "and the second recourse — bind it from a `sql` statement — is the one that \
             covers free text, which no pattern can: {message}"
        );

        assert!(
            scripted_endpoint(BINDS_REGION, &[string_param("region")]).is_ok(),
            "and the same endpoint publishes once the parameter is bounded"
        );
    }

    /// A `pattern` that is present but too wide is refused with the character
    /// that makes it too wide.
    ///
    /// Separate from the missing-pattern case because the author's action is
    /// different, and because a rule that answered "needs a pattern" to a
    /// parameter that HAS one would send them to add a second.
    #[test]
    fn a_string_pattern_that_admits_a_quote_is_refused_on_a_scripted_endpoint() {
        let loose = ParamSpec {
            pattern: Some("^[a-z']+$".into()),
            ..param("region", ParamType::String)
        };
        let err = scripted_endpoint(BINDS_REGION, &[loose])
            .expect_err("the pattern lets a quote through");
        match &err {
            ValidationError::StringParameterPatternTooWide {
                parameter,
                witness,
                statement,
                ..
            } => {
                assert_eq!(parameter, "region");
                assert_eq!(*witness, '\'');
                assert_eq!(statement, "total");
            }
            other => panic!("expected StringParameterPatternTooWide, got {other:?}"),
        }
        assert!(
            err.to_string().contains("accepts '\\''"),
            "the witness must arrive ESCAPED and visible — the widest patterns an author \
             writes have their first unsafe character in the control range, where a plain \
             `Display` is invisible or breaks the line: {err}"
        );
    }

    /// **The direction this rule must not fail in.** An endpoint whose
    /// statements are all WRITTEN keeps every unconstrained `String` parameter
    /// it ever had.
    ///
    /// Nothing about a written statement changed: `{{ region }}` is a bind on a
    /// path with no interpolating mode, so there is nothing for a constraint to
    /// protect. Without this test the rule could have been applied to every
    /// endpoint in the product and the suite would still be green — and every
    /// endpoint published before package S would stop republishing.
    #[test]
    fn a_written_endpoint_may_still_declare_an_unconstrained_string_parameter() {
        assert!(
            validate_endpoint(
                &[StatementSpec::from_sql(
                    "rows",
                    "SELECT id FROM t WHERE r = {{ region }}"
                )],
                &[unconstrained_string_param("region")],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            )
            .is_ok(),
            "a value that can only be bound needs no alphabet"
        );
    }

    /// Only `String` carries the rule, and the other five types are exempt for
    /// reasons `crate::serving::params::coerce` can be read for.
    ///
    /// This is the same four-way reading [`sentinel_probes`] makes when it
    /// decides which parameters carry a needle, one step later: a `Number` is
    /// `-?digits[.digits]`, a `Boolean` is one of two words, a `Date` and a
    /// `Timestamp` are chrono-reformatted, and an `Enum` yields the AUTHOR's own
    /// `allowed` entry. None of them can carry a quote or a space, so requiring
    /// a `pattern` on them would refuse honest endpoints for nothing — and
    /// `check_constraints` refuses a `pattern` on those types anyway, which
    /// would make the rule unsatisfiable rather than strict.
    #[test]
    fn only_string_parameters_carry_the_script_constraint_rule() {
        let script = "def build_sql(params):\n  \
                      return \"SELECT count(*) AS n FROM t WHERE a = {{ n }} AND b = {{ flag }} \
                      AND c = {{ d }} AND e = {{ ts }} AND f = {{ grain }}\"";
        let params = vec![
            param("n", ParamType::Number),
            param("flag", ParamType::Boolean),
            param("d", ParamType::Date),
            param("ts", ParamType::Timestamp),
            enum_param("grain", &["daily", "hourly"]),
        ];
        assert!(
            scripted_endpoint(script, &params).is_ok(),
            "every type whose `coerce` grammar is already closed publishes with no `pattern`"
        );
    }

    /// **What a smarter probe could not have caught, and what a `varies_with`
    /// exemption would have let through.**
    ///
    /// The script concatenates `q` into a string literal — but only on a branch
    /// no probe reaches, because the branch is chosen by an `enum` whose
    /// representative value and whose sentinel are both something other than
    /// `"raw"`. So:
    ///
    /// * the sentinel never lands in the emitted text, and
    ///   [`GeneratedSql::probe`] returns `Ok` — asserted first, because the
    ///   whole point is that the probe is blind here;
    /// * `q` does not change the emitted template either, so it is NOT in
    ///   `varies_with` — which is why the tempting narrowing ("require a
    ///   constraint only on the parameters the script was seen to use") would
    ///   have exempted exactly this parameter;
    /// * a caller sending `mode=raw` reaches a branch nothing scanned, with
    ///   their own bytes inside a string literal.
    ///
    /// The conservative rule refuses it, and with a safe alphabet the branch is
    /// harmless: `'` + a bare word + `'` closes where the author put it.
    #[test]
    fn the_rule_reaches_a_parameter_hidden_behind_a_branch_no_probe_takes() {
        // `{{ mode }}` appears in the branch that IS emitted, only so that
        // `check_every_parameter_is_read` sees it: `mode` selects a branch no
        // probe takes, so it changes no template and would otherwise be refused
        // as unreferenced — a refusal about a different rule entirely.
        let script = "def build_sql(params):\n  \
                      if params[\"mode\"] == \"raw\":\n    \
                      return \"SELECT count(*) AS n FROM t WHERE m = {{ mode }} AND x = '\" \
                      + params[\"q\"] + \"'\"\n  \
                      return \"SELECT count(*) AS n FROM t WHERE m = {{ mode }} AND x = {{ q }}\"";
        let mode = enum_param("mode", &["safe", "raw"]);
        let statements = vec![StatementSpec::from_script("total", script)];

        let blind = GeneratedSql::build(
            &statements,
            &[unconstrained_string_param("q"), mode.clone()],
        )
        .expect(
            "the probe must still be blind to a branch its representative values never \
             take. If this now FAILS, the probe grew a capability its doc comment \
             disclaims and the third blind spot is closed — say so in the spec and the \
             comment, do not just delete this test",
        );
        assert!(
            !blind.varying_params().any(|p| p == "q"),
            "and `q` does not even VARY the template, so a `varies_with` exemption would have \
             let this endpoint publish"
        );

        assert!(
            matches!(
                scripted_endpoint(script, &[unconstrained_string_param("q"), mode.clone()]),
                Err(ValidationError::StringParameterNeedsPattern { .. })
            ),
            "the declarative rule does not depend on the instrument that is blind here"
        );
        assert!(
            scripted_endpoint(script, &[string_param("q"), mode]).is_ok(),
            "and the bounded parameter makes the hidden branch harmless"
        );
    }

    /// Preview refuses what publish refuses, for this rule too.
    ///
    /// The authoring loop's one job is that a draft accepted on screen is a
    /// draft publish accepts (`validate_preview_refuses_what_publish_refuses`).
    /// A rule added to publish and not to preview shows the author green and
    /// then no — and package S's preview route exists because that had already
    /// happened once.
    #[test]
    fn preview_refuses_an_unconstrained_string_on_a_scripted_draft() {
        let refused = validate_preview(
            &StatementSource::Script(BINDS_REGION.to_owned()),
            &SiblingStatements::default(),
            &[unconstrained_string_param("region")],
            &OutputSpec::default(),
        );
        assert!(
            matches!(
                refused,
                Err(ValidationError::StringParameterNeedsPattern { .. })
            ),
            "preview must refuse it here, not leave it to publish: {refused:?}"
        );
        assert!(
            validate_preview(
                &StatementSource::Script(BINDS_REGION.to_owned()),
                &SiblingStatements::default(),
                &[string_param("region")],
                &OutputSpec::default(),
            )
            .is_ok(),
            "and accept the bounded one, or the rule would block the authoring loop"
        );
        assert!(
            validate_preview(
                &written("SELECT id FROM t WHERE r = {{ region }}"),
                &SiblingStatements::default(),
                &[unconstrained_string_param("region")],
                &OutputSpec::default(),
            )
            .is_ok(),
            "a WRITTEN draft is untouched by the rule, exactly as at publish"
        );
    }

    /// **The constraint publish demands is the one serving already enforces, so
    /// there is nothing for serving to re-ask.**
    ///
    /// This is C1's property, and package 5 is only sound because of it: the
    /// publish-time argument is "the caller's bytes are inside the safe
    /// alphabet", and the thing that makes that true on every request is
    /// `coerce` applying the SAME `pattern` to the SAME value it then hands the
    /// script. Without this assertion the gate would be a publish-time opinion
    /// about a serving-time fact.
    ///
    /// `allowed` is deliberately not accepted as an alternative constraint for a
    /// `String`, and the second half is why: `coerce` never reads it for this
    /// type, so a rule that accepted it would rest on an enforcement that does
    /// not exist. (`check_constraints` now refuses that declaration outright —
    /// `allowed_on_anything_but_an_enum_is_refused` — and this rule still gets
    /// there first, so the author reads the message with the recourse in it.)
    ///
    /// **This composition runs over `^[A-Za-z0-9_]*$`, where it cannot fail**,
    /// and that is worth saying out loud: no bug in how egret ANCHORS a pattern
    /// can make that one unsafe, so this test stayed green through a pattern
    /// that escaped the anchors entirely. The composition over a pattern where
    /// it can fail is
    /// `a_pattern_that_escapes_the_anchors_is_refused_and_coerce_agrees`, and
    /// neither test replaces the other.
    #[test]
    fn the_constraint_the_gate_demands_is_the_one_coerce_enforces() {
        let bounded = string_param("region");
        assert_eq!(
            crate::serving::params::coerce(&bounded, "eu_west"),
            Ok(Param::Str("eu_west".into())),
            "a bare word passes"
        );
        assert!(
            matches!(
                crate::serving::params::coerce(&bounded, "eu' OR 1=1--"),
                Err(crate::serving::params::CoerceError::Constraint {
                    constraint: "pattern",
                    ..
                })
            ),
            "and every character the publish argument relies on excluding is refused on the \
             serving path, by the pattern publish required"
        );

        let listed = ParamSpec {
            allowed: Some(vec!["eu".into()]),
            ..param("region", ParamType::String)
        };
        assert_eq!(
            crate::serving::params::coerce(&listed, "anything at all"),
            Ok(Param::Str("anything at all".into())),
            "`allowed` on a `String` is ignored by `coerce`, which is why the gate does not \
             accept it as a constraint"
        );
        assert!(
            matches!(
                scripted_endpoint(BINDS_REGION, &[listed]),
                Err(ValidationError::StringParameterNeedsPattern { .. })
            ),
            "so an author who writes `allowed` on a `String` is still told to write a pattern"
        );
    }

    /// The probes are the PUBLISH path's cost and nothing else's: publish's
    /// in-transaction re-validation must not run one.
    ///
    /// The endpoint's script would be refused by the gate if it were evaluated
    /// again — it concatenates the parameter — and the text handed in is
    /// innocent. Under the skip it must validate, which it can only do by
    /// evaluating nothing. Together with
    /// `a_generated_statement_is_not_regenerated_under_the_lock` this covers
    /// both evaluations a scripted statement can cost.
    ///
    /// The second half is the one that makes the payload's design load-bearing:
    /// the branching script's `grain` is referenced only behaviourally, so a
    /// re-validation that recomputed `varies_with` under the lock — or shrugged
    /// and left it empty — would refuse with `UnreferencedParameter` an endpoint
    /// `validate` had just accepted. Publishing would be impossible.
    #[test]
    fn the_sentinel_probe_does_not_run_under_the_lock() {
        let interpolating = vec![StatementSpec::from_script(
            "rows",
            "def build_sql(params):\n  return \"SELECT 1 FROM sales_\" + params[\"region\"]",
        )];
        let params = vec![string_param("region")];
        let innocent = GeneratedSql(BTreeMap::from([(
            "rows".to_string(),
            GeneratedStatement {
                sql: "SELECT 1 FROM t WHERE r = {{ region }}".to_string(),
                alternates: Vec::new(),
                varies_with: Vec::new(),
            },
        )]));
        assert!(
            validate_endpoint(
                &interpolating,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &innocent
                },
            )
            .is_ok(),
            "a sentinel probe under the lock puts an author's Starlark back inside publish's \
             transaction — the thing `ScriptEval` exists to prevent"
        );

        let branching = vec![StatementSpec::from_script(
            "rows",
            "def build_sql(params):\n  \
             if params[\"grain\"] == \"daily\":\n    return \"SELECT 1 FROM sales_daily\"\n  \
             return \"SELECT 1 FROM sales_hourly\"",
        )];
        let grain = vec![enum_param("grain", &["daily", "hourly"])];
        let generated =
            GeneratedSql::build(&branching, &grain).expect("the branching script builds");
        assert_eq!(
            generated.varying_params().collect::<Vec<_>>(),
            vec!["grain"],
            "the behavioural reading has to be carried across the lock, not recomputed there"
        );
        assert!(
            validate_endpoint(
                &branching,
                &grain,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &generated
                },
            )
            .is_ok(),
            "publish's locked re-validation must accept what `validate` accepted"
        );
    }

    /// The probe count is bounded by the two limits checked before it, and the
    /// bound is a WALL-CLOCK number rather than an argument.
    ///
    /// The worst case an author can construct: `MAX_STATEMENTS` scripted
    /// statements and `MAX_DECLARED_PARAMS` parameters, which is
    /// `16 × 32 × 2 = 1024` evaluations plus the 16 that generate. If that is
    /// ever not affordable, the number here is what says so — and the fix is a
    /// budget, not a quietly raised limit.
    ///
    /// Measured at 524 ms in a debug build on the author's machine. The bound is
    /// 8 s so a loaded CI box does not make it flaky; it is there to catch an
    /// ORDER of magnitude, which is what a third probe value or a lifted limit
    /// would cost.
    #[test]
    fn the_probe_cost_is_bounded_by_the_limits_checked_before_it() {
        let params: Vec<ParamSpec> = (0..MAX_DECLARED_PARAMS)
            .map(|i| string_param(&format!("p{i}")))
            .collect();
        // The script's CONTENT does not change the count: `GeneratedSql::probe`
        // substitutes both probe values for every declared parameter whatever
        // the script does with them, which is the point — the cost is a function
        // of the two limits and of nothing an author writes.
        let script = "def build_sql(params):\n  return \"SELECT 1 AS a\"";
        let statements: Vec<StatementSpec> = (0..egret_dataapi_api::MAX_STATEMENTS)
            .map(|i| StatementSpec::from_script(format!("s{i}"), script))
            .collect();

        let started = std::time::Instant::now();
        let generated = GeneratedSql::build(&statements, &params).expect("the worst case builds");
        let elapsed = started.elapsed();
        assert_eq!(
            generated.varying_params().count(),
            0,
            "nothing branches here"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "1024 sentinel probes took {elapsed:?}; publish pays this once, outside its \
             transaction, but a number climbing here is the signal for a budget"
        );
    }

    /// The wording fix package S Task 2 deferred and Task 3 is the task that
    /// makes visible: an author who did not define `build_sql` must be told
    /// about `build_sql`, not about `transform`.
    ///
    /// Asserted from the PUBLISH gate rather than from `transform`'s own unit
    /// test, because the gate is the surface an author actually reads and
    /// because the two could drift: `transform` could name the contract
    /// correctly while `lifecycle` wrapped it in a sentence that did not.
    #[test]
    fn a_script_without_build_sql_is_told_about_build_sql() {
        let err = validate_generated("x = 1", &[]).expect_err("`x = 1` defines no `build_sql`");
        let text = err.to_string();
        assert_eq!(
            text,
            "statement `rows` could not build its SQL: the script must define `build_sql(params)`"
        );
        assert!(
            !text.contains("transform"),
            "a build-SQL author must not be sent to define `transform`: {text}"
        );
    }

    /// The skip really skips GENERATION, not only the transform's evaluation.
    ///
    /// The endpoint's statement carries a script that cannot parse, and the
    /// caller hands in text generated a moment earlier from a script that could.
    /// Under `ScriptEval::Here` that is a refusal; under the skip it must
    /// validate, and the only way it can is by never evaluating the script — the
    /// observable difference between "publish generates under its row lock" and
    /// "publish does not". See `ScriptEval`.
    ///
    /// The second half is what stops the skip becoming a hole: the text handed
    /// in is still SCANNED, so a `;` in it is refused under the skip exactly as
    /// it is under `Here`.
    #[test]
    fn a_generated_statement_is_not_regenerated_under_the_lock() {
        let good = vec![StatementSpec::from_script(
            "rows",
            "def build_sql(params):\n  return \"SELECT 1 AS a\"",
        )];
        let generated = GeneratedSql::build(&good, &[]).expect("the good script generates");
        let unparseable = vec![StatementSpec::from_script(
            "rows",
            "def build_sql(params)\n  return \"SELECT 1 AS a\"",
        )];

        assert!(
            matches!(
                validate_endpoint(
                    &unparseable,
                    &[],
                    &cache(CacheScope::PerPrincipal),
                    &OutputSpec::default(),
                    ConnectionSafety::UNGOVERNED,
                    ScriptEval::Here,
                ),
                Err(ValidationError::StatementScriptFailed { .. })
            ),
            "the generating caller must still refuse a script that does not parse"
        );
        assert!(
            validate_endpoint(
                &unparseable,
                &[],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &generated
                },
            )
            .is_ok(),
            "publish's in-transaction re-validation must generate nothing — a refusal here \
             means an author's `build_sql` is back inside the locked transaction"
        );

        let separator = vec![StatementSpec::from_script(
            "rows",
            "def build_sql(params):\n  return \"SELECT 1; DROP TABLE t\"",
        )];
        let smuggled = GeneratedSql(BTreeMap::from([(
            "rows".to_string(),
            GeneratedStatement {
                sql: "SELECT 1; DROP TABLE t".to_string(),
                alternates: Vec::new(),
                varies_with: Vec::new(),
            },
        )]));
        assert_eq!(
            validate_endpoint(
                &separator,
                &[],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &smuggled
                },
            ),
            Err(ValidationError::MultipleStatements),
            "skipping the evaluation must not skip the scans: the text handed in is held to \
             every rule the text generated here would be"
        );
    }

    /// Text generated for a DIFFERENT statement list is refused, not silently
    /// skipped.
    ///
    /// Only `EndpointStore::publish` can produce this — it generates from the
    /// draft and re-validates the locked row — and its identity check is what
    /// makes it unreachable. This is the same guarantee held by the data: were
    /// the check removed or reordered, a statement renamed mid-publish would
    /// otherwise have to be answered by *some* text, and the only candidates are
    /// another statement's or none.
    #[test]
    fn generated_text_from_another_statement_list_is_refused() {
        let generated =
            GeneratedSql::build(&[StatementSpec::from_sql("rows", "SELECT id FROM t")], &[])
                .expect("generates");
        assert_eq!(
            validate_endpoint(
                &[StatementSpec::from_sql(
                    "total",
                    "SELECT count(*) AS n FROM t"
                )],
                &[],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &generated
                },
            ),
            Err(ValidationError::GenerationMismatch("total".into()))
        );
    }

    /// A script sees every DECLARED parameter, and can branch on it.
    ///
    /// The values are `representative_value`'s, so they are truthy and a
    /// branching script takes its first branch rather than failing on a missing
    /// key — see `generation_params`. What this pins is that the dict is
    /// populated at all: an empty one makes every `params["x"]` a runtime error
    /// and no branching script could ever publish.
    ///
    /// Both branches emit `{{ grain }}` because
    /// [`ValidationError::UnreferencedParameter`] is one of the rules a
    /// generated statement obeys: a parameter the script reads but no emitted
    /// template mentions is, to the published CONTRACT, a query parameter
    /// nothing uses. That is the same answer written SQL gets, which is the
    /// point of this task — but it is a spec-level question for package S,
    /// recorded in the task report rather than decided here.
    #[test]
    fn a_generating_script_is_handed_the_declared_parameters() {
        let script = "def build_sql(params):\n  \
                      if params[\"grain\"] == \"daily\":\n    \
                      return \"SELECT 1 AS daily FROM t WHERE g = {{ grain }}\"\n  \
                      return \"SELECT 1 AS hourly FROM t WHERE g = {{ grain }}\"";
        let mut grain = param("grain", ParamType::Enum);
        grain.allowed = Some(vec!["daily".into(), "hourly".into()]);
        let validated = validate_generated(script, std::slice::from_ref(&grain))
            .expect("a branching script publishes");
        assert_eq!(
            validated.sql.get("rows"),
            Some("SELECT 1 AS daily FROM t WHERE g = {{ grain }}"),
            "the script must have been handed `grain`, at its representative value"
        );
    }

    /// **`MAX_STATEMENTS` is ahead of generation**, and the identity of the
    /// error is what proves it.
    ///
    /// A timing assertion is the wrong instrument here — a trivial script
    /// evaluates in microseconds, so 256 of them would come in under any
    /// threshold worth asserting and the test would pass with the ordering
    /// reversed. So the script is one that CANNOT generate: if the list is
    /// bounded first the answer is `TooManyStatements`, and if generation runs
    /// first the answer is `StatementScriptFailed` for `s0`. Two different
    /// arms, no clock.
    ///
    /// Asserted at both levels because the ordering lives in both: inside
    /// [`GeneratedSql::build`], which is what publish calls directly before it
    /// takes its row lock, and inside [`validate_endpoint`], which is what the
    /// two entry points share.
    #[test]
    fn the_statement_bound_is_checked_before_any_script_runs() {
        let statements: Vec<StatementSpec> = (0..256)
            .map(|i| StatementSpec::from_script(format!("s{i}"), "x = 1"))
            .collect();
        let too_many =
            ValidationError::Statement(egret_dataapi_api::StatementError::TooManyStatements {
                count: 256,
                limit: egret_dataapi_api::MAX_STATEMENTS,
            });
        assert_eq!(
            GeneratedSql::build(&statements, &[]).unwrap_err(),
            too_many,
            "`GeneratedSql::build` must bound the list before it evaluates anything; a \
             `StatementScriptFailed` here means 256 Starlark evaluations were attempted first"
        );
        assert_eq!(
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            ),
            Err(too_many),
            "generation must sit AFTER the statement-list rule in `validate_endpoint` too"
        );
    }

    /// The other side of the statement bound, and its own test: a legitimate
    /// several-statement endpoint must still validate. The canonical one is a
    /// `rows` query and the `total` that paginates it (spec §4.4).
    #[test]
    fn the_canonical_two_statement_endpoint_still_validates() {
        let statements = vec![
            StatementSpec::from_sql("rows", "SELECT id FROM t WHERE region = {{ region }}"),
            StatementSpec::from_sql(
                "total",
                "SELECT count(*) AS n FROM t WHERE region = {{ region }}",
            ),
        ];
        let params = vec![param("region", ParamType::String)];
        assert_eq!(
            validate_endpoint(
                &statements,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            )
            .map(|v| v.iterated),
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
            StatementSpec::from_sql("summary", SUMMARY),
            StatementSpec::from_sql("by_region", BY_REGION),
        ];
        assert_eq!(
            validate_endpoint(
                &statements,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
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
                &written(SUMMARY),
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
            &written(BY_REGION),
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
            StatementSpec::from_sql("summary_all", SUMMARY_ALL),
            StatementSpec::from_sql("by_region", BY_REGION),
        ];
        let published = validate_endpoint(
            &statements,
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
        .expect("the §4.2 pair publishes")
        .iterated;
        // `summary_all` never mentions `region`, so on its own the union is
        // EMPTY and preview would resolve a scalar where serving binds a list.
        assert!(preview(SUMMARY_ALL, &params).unwrap().names().is_empty());
        assert_eq!(
            validate_preview(
                &written(SUMMARY_ALL),
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

    /// **The sibling divergence runs in BOTH directions, and this is the one
    /// that goes the other way** (package S Task 8.5).
    ///
    /// The two consequences `crate::preview`'s module comment used to list are
    /// under-reads: a sibling script's emitted `{% for %}` never joins the
    /// union, so preview resolves a scalar where serving binds a list. This is
    /// the OVER-read, and it is the mirror image. `siblings` is a list of texts,
    /// so a client that puts a scripted sibling's SOURCE in it hands
    /// `IteratedParams::across` a Starlark program to scan as though it were a
    /// template — and a `{% for %}` header sitting in a string literal on a
    /// branch the script never emits joins the union anyway.
    ///
    /// Preview's set is then WIDER than the endpoint's: preview binds `region`
    /// as a list where publish and serving bind a scalar, for a draft that
    /// publishes perfectly well. "Preview green, publish refuses, never the
    /// reverse" was therefore not true, and a two-item list that implied it was
    /// the kind of enumeration this task's other half exists to correct.
    ///
    /// The console does not reach it — `Query.tsx` sends `""` for a scripted
    /// sibling — and the second half asserts exactly that, so "the browser is
    /// safe" is a property this file checks rather than a claim about another
    /// language's file.
    #[test]
    fn a_scripted_siblings_source_can_make_previews_shape_wider_than_publishs() {
        const ON_SCREEN: &str = "SELECT count(*) AS n FROM t";
        // The `{% for %}` lives in a string literal on the `__never__` branch.
        // No probe supplies that value, so no template the endpoint can emit
        // carries the header — but the SOURCE does.
        const SCRIPTED: &str = "def build_sql(params):\n  \
             if params[\"region\"] == \"__never__\":\n    \
             return \"SELECT n FROM t WHERE region IN (\
             {% for x in region %}{{ x }}{% endfor %})\"\n  \
             return \"SELECT n FROM t WHERE region = {{ region }}\"";
        let params = vec![string_param("region")];

        let published = validate_endpoint(
            &[
                StatementSpec::from_sql("on_screen", ON_SCREEN),
                StatementSpec::from_script("sibling", SCRIPTED),
            ],
            &params,
            &cache(CacheScope::PerPrincipal),
            &OutputSpec::default(),
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
        )
        .expect("the endpoint publishes: every emitted template is a plain read")
        .iterated;
        assert!(
            published.names().is_empty(),
            "no template this endpoint can EMIT iterates anything: {:?}",
            published.names()
        );

        // The same endpoint previewed by a client that sends the sibling's
        // source, which is the only thing a `string[]` wire lets it send.
        assert_eq!(
            validate_preview(
                &written(ON_SCREEN),
                &siblings(&[SCRIPTED]),
                &params,
                &OutputSpec::default()
            )
            .expect("the preview is accepted")
            .names(),
            ["region"],
            "the scan read a header out of a branch that is never emitted"
        );

        // …and the console's `""`, which is what makes the browser safe.
        assert!(validate_preview(
            &written(ON_SCREEN),
            &siblings(&[""]),
            &params,
            &OutputSpec::default()
        )
        .expect("the preview is accepted")
        .names()
        .is_empty());
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
        assert!(validate_preview(
            &written(ON_SCREEN),
            &broken,
            &params,
            &OutputSpec::default()
        )
        .is_ok());
        // …while a sibling that DOES loop still counts, so "not scanned" has not
        // become "not read".
        let looping = siblings(&["SELECT region AS n FROM t WHERE region IN (\
             {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %}); # oops"]);
        assert_eq!(
            validate_preview(
                &written(ON_SCREEN),
                &looping,
                &params,
                &OutputSpec::default()
            )
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
            &written("SELECT 1 AS n"),
            &siblings(&refs[1..]),
            &[],
            &OutputSpec::default()
        )
        .is_ok());
        // One more is an endpoint publish would refuse, refused here with the
        // same error and the same numbers.
        assert_eq!(
            validate_preview(
                &written("SELECT 1 AS n"),
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
                &written("SELECT 1; DROP TABLE t"),
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
            StatementSpec::from_sql("summary_all", "SELECT count(*) AS n FROM t"),
            StatementSpec::from_sql("by_region", "SELECT region AS n FROM t WHERE region IN ({% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})"),
        ];
        assert_eq!(
            validate_endpoint(
                &statements,
                &[param("region", ParamType::String)],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            )
            .map(|v| v.iterated),
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

    /// Package 1's rule, over all eight corners.
    ///
    /// Exactly one of the eight is refused, and enumerating them is the point:
    /// the mutations this rule can suffer are all *widenings or narrowings* of
    /// the same expression, and each moves a DIFFERENT row. Dropping the
    /// `!conn.runs_per_principal()` half turns three currently-permitted rows
    /// red; dropping `conn.governed` turns three more red; replacing the `&&`
    /// with `||` does both; and `false &&` turns the refusing row green. A
    /// single-case assertion would survive two of those four.
    #[test]
    fn only_a_governed_database_without_a_per_principal_connection_is_refused() {
        for governed in [false, true] {
            for delegated in [false, true] {
                for impersonate in [false, true] {
                    let conn = ConnectionSafety {
                        governed,
                        delegated,
                        impersonate,
                    };
                    let got = check_governed_connection(conn);
                    if governed && !delegated && !impersonate {
                        assert_eq!(
                            got,
                            Err(ValidationError::GovernedDatabaseNeedsPerPrincipalConnection),
                            "a governed database on a service-account connection must not \
                             publish (governed={governed}, delegated={delegated}, \
                             impersonate={impersonate})"
                        );
                    } else {
                        assert!(
                            got.is_ok(),
                            "nothing else may be refused (governed={governed}, \
                             delegated={delegated}, impersonate={impersonate})"
                        );
                    }
                }
            }
        }
    }

    /// `impersonate` alone satisfies a governed database, which is the half a
    /// reader who knows only `delegated` gets wrong — it is exactly what made
    /// the shared-cache check a live cross-identity leak once already, and this
    /// rule was written against the same pair for the same reason.
    #[test]
    fn impersonate_alone_lets_a_governed_database_publish() {
        assert!(check_governed_connection(ConnectionSafety {
            governed: true,
            delegated: false,
            impersonate: true,
        })
        .is_ok());
    }

    /// `ConnectionSafety::UNGOVERNED` is what the other ~30 call sites in this
    /// file pass, so if it ever stopped meaning "all three columns at their
    /// `query.databases` default" every one of those tests would quietly be
    /// testing a different connection than it says it is.
    #[test]
    fn the_ungoverned_constant_is_all_three_column_defaults() {
        // `assert_eq!` against a constructed value rather than three
        // `assert!`s on the constant's fields: clippy refuses the latter as an
        // assertion with a constant value, and it is right — the version that
        // reads better also happens to be the one that states the whole shape
        // in one comparison rather than three that could each be forgotten.
        assert_eq!(
            ConnectionSafety::UNGOVERNED,
            ConnectionSafety {
                governed: false,
                delegated: false,
                impersonate: false,
            }
        );
        assert!(!ConnectionSafety::UNGOVERNED.runs_per_principal());
        assert!(check_governed_connection(ConnectionSafety::UNGOVERNED).is_ok());
    }

    /// **The gate is reached through the real validator, not only through its
    /// own unit.** `validate_endpoint` is what `POST …/validate` and
    /// `EndpointStore::publish` both run; a rule that passes its own test and is
    /// never called from there is a rule that does not exist. The fixture is a
    /// draft that is otherwise entirely publishable — the ONLY thing wrong with
    /// it is the connection — so the refusal cannot be an artefact of some other
    /// rule answering first.
    #[test]
    fn validate_endpoint_refuses_a_governed_service_account_connection() {
        let statements = [StatementSpec::from_sql("rows", "SELECT 1 AS n")];
        let unsafe_conn = ConnectionSafety {
            governed: true,
            delegated: false,
            impersonate: false,
        };
        assert_eq!(
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                unsafe_conn,
                ScriptEval::Here,
            )
            .err(),
            Some(ValidationError::GovernedDatabaseNeedsPerPrincipalConnection),
        );
        // ONE field flipped, nothing else, and the same draft publishes. That is
        // what makes the assertion above about the connection rather than about
        // the definition — a fixture that could never validate at all would
        // "prove" this rule with the rule deleted.
        for safe in [
            ConnectionSafety {
                governed: true,
                delegated: true,
                impersonate: false,
            },
            ConnectionSafety {
                governed: true,
                delegated: false,
                impersonate: true,
            },
            ConnectionSafety {
                governed: false,
                delegated: false,
                impersonate: false,
            },
        ] {
            assert!(
                validate_endpoint(
                    &statements,
                    &[],
                    &cache(CacheScope::PerPrincipal),
                    &OutputSpec::default(),
                    safe,
                    ScriptEval::Here,
                )
                .is_ok(),
                "{safe:?} must publish"
            );
        }
    }

    /// The two connection rules are independent, and the order they answer in is
    /// pinned because it is a decision rather than an accident: a draft that
    /// breaks both is told about `cache.scope` first, because that is the field
    /// on the DRAFT the author is editing.
    #[test]
    fn the_cache_rule_answers_before_the_governance_rule() {
        assert_eq!(
            validate_endpoint(
                &[StatementSpec::from_sql("rows", "SELECT 1 AS n")],
                &[],
                &cache(CacheScope::Shared),
                &OutputSpec::default(),
                ConnectionSafety {
                    governed: true,
                    delegated: true,
                    impersonate: false,
                },
                ScriptEval::Here,
            )
            .err(),
            Some(ValidationError::SharedCacheOnPerPrincipalConnection),
        );
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

    /// `allowed` is a constraint like the other seven, and the table that gates
    /// them by type had no row for it.
    ///
    /// `coerce` reads `allowed` for `ParamType::Enum` and for nothing else, so
    /// an author who writes it on a `String` declares a closed set that no
    /// request is ever measured against — verbatim the failure
    /// `check_constraints`' own comment describes ("teaches the author a model
    /// where constraints apply everywhere, and the next one they write silently
    /// does nothing"). Package 5's refusal already had to spend a sentence
    /// telling such an author to write a `pattern` instead; now the declaration
    /// itself is refused.
    #[test]
    fn allowed_on_anything_but_an_enum_is_refused() {
        for ty in [
            ParamType::String,
            ParamType::Number,
            ParamType::Boolean,
            ParamType::Date,
            ParamType::Timestamp,
        ] {
            let mut p = param("region", ty.clone());
            p.allowed = Some(vec!["eu".into()]);
            let err = check_constraints(std::slice::from_ref(&p))
                .expect_err("`allowed` is read for `Enum` and for nothing else");
            let msg = err.to_string();
            assert!(
                msg.contains("allowed") && msg.contains("region"),
                "it must name the constraint and the parameter, on a `{}`: {msg}",
                type_name(&ty)
            );
        }
        let mut e = param("status", ParamType::Enum);
        e.allowed = Some(vec!["open".into()]);
        assert!(
            check_constraints(std::slice::from_ref(&e)).is_ok(),
            "and the one type that reads it keeps it — this rule must not make an `Enum` \
             unpublishable, which is the only direction it could break the product in"
        );
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

    /// The `returns` cap, with both numbers.
    ///
    /// `returns` was the only author-controlled list in this module with no
    /// bound at all — `MAX_STATEMENTS` is 16, `MAX_DECLARED_PARAMS` is 32,
    /// `MAX_TRANSFORM_SCRIPT_BYTES` is 8192 — and the cost of an unbounded one
    /// lands on SERVING, not on publish: `check_returns` scans it once per
    /// returned field on every request.
    #[test]
    fn more_returns_than_the_cap_are_refused_with_both_numbers() {
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        let r = &mut o.transform.as_mut().unwrap().returns;
        r.clear();
        for i in 0..=MAX_TRANSFORM_RETURNS {
            r.push(ColumnSpec {
                name: format!("c{i}"),
                ty: "bigint".into(),
                required: true,
            });
        }
        match check_transform(&o) {
            Err(ValidationError::TransformReturnsTooMany { count, limit }) => {
                assert_eq!(count, MAX_TRANSFORM_RETURNS + 1);
                assert_eq!(limit, MAX_TRANSFORM_RETURNS);
            }
            other => panic!("expected TransformReturnsTooMany, got {other:?}"),
        }
    }

    /// Exactly at the cap is allowed, so the test above discriminates a limit
    /// rather than a blanket refusal.
    #[test]
    fn returns_exactly_at_the_cap_are_allowed() {
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        let r = &mut o.transform.as_mut().unwrap().returns;
        r.clear();
        for i in 0..MAX_TRANSFORM_RETURNS {
            r.push(ColumnSpec {
                name: format!("c{i}"),
                ty: "bigint".into(),
                required: true,
            });
        }
        assert_eq!(check_transform(&o), Ok(()));
    }

    /// A blank `name` is refused as blank, **not** as a duplicate.
    ///
    /// Two unnamed rows used to reach the duplicate rule and publish with
    /// ``declares `` more than once`` — a refusal quoting a column that does not
    /// exist, for an author who has two rows on screen and no idea which one is
    /// meant. And a SINGLE unnamed row published happily, becoming an OpenAPI
    /// property with an empty name.
    ///
    /// The single-row half is what makes this test worth having: it is refused
    /// now and could never have been refused by the duplicate rule.
    #[test]
    fn a_blank_return_name_is_refused_as_blank_even_when_there_is_only_one() {
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        let r = &mut o.transform.as_mut().unwrap().returns;
        r.clear();
        r.push(ColumnSpec {
            name: "   ".into(),
            ty: "bigint".into(),
            required: true,
        });
        assert_eq!(
            check_transform(&o),
            Err(ValidationError::TransformReturnsBlankName),
            "a whitespace-only name is blank, not a name"
        );

        let mut two = transformed_output("def transform(params, statements):\n  return {}");
        let r = &mut two.transform.as_mut().unwrap().returns;
        r.clear();
        for _ in 0..2 {
            r.push(ColumnSpec {
                name: String::new(),
                ty: "bigint".into(),
                required: true,
            });
        }
        assert_eq!(
            check_transform(&two),
            Err(ValidationError::TransformReturnsBlankName),
            "two unnamed rows are two unnamed rows, not a duplicate of `\"\"`"
        );
    }

    /// `"total"` and `"total "` are refused, not published as two columns.
    ///
    /// The editor already flagged this pair as a collision while publish
    /// accepted it — a disagreement in the direction that is hardest to notice,
    /// because the author sees a warning, publishes anyway, and gets a contract
    /// with two indistinguishable property names. Refusing the padded name is
    /// what makes the two agree, and it is also what makes
    /// `check_transform_shape`'s verbatim duplicate comparison and the editor's
    /// trimmed one the same comparison.
    #[test]
    fn a_whitespace_padded_return_name_is_refused_rather_than_trimmed() {
        let mut o = transformed_output("def transform(params, statements):\n  return {}");
        let r = &mut o.transform.as_mut().unwrap().returns;
        r.clear();
        r.push(ColumnSpec {
            name: "total".into(),
            ty: "bigint".into(),
            required: true,
        });
        r.push(ColumnSpec {
            name: "total ".into(),
            ty: "bigint".into(),
            required: true,
        });
        match check_transform(&o) {
            Err(ValidationError::TransformReturnsPaddedName(name)) => {
                assert_eq!(name, "total ", "the refusal must quote the name as written");
            }
            other => panic!("expected TransformReturnsPaddedName, got {other:?}"),
        }
    }

    /// `ScriptEval::AlreadyRunBeforeTheLock` really does skip the interpreter.
    ///
    /// Both halves matter and neither is enough alone. `Here` must refuse a
    /// script that does not parse — otherwise the skip is not a skip, it is the
    /// evaluation having quietly stopped happening at all. And
    /// `AlreadyRunBeforeTheLock` must ACCEPT the same endpoint, which is the
    /// only observable difference between "publish evaluates Starlark under its
    /// row lock" and "publish does not".
    ///
    /// The structural rules still run under the skip — the third assertion —
    /// so this is a narrower claim than "the transform is not checked".
    ///
    /// Package S: the skip now covers a statement's `build_sql` too, which is
    /// why the variant carries the [`GeneratedSql`] the caller produced outside
    /// the lock. `a_generated_statement_is_not_regenerated_under_the_lock` is
    /// that half.
    #[test]
    fn the_script_evaluation_is_skipped_only_when_the_caller_says_it_already_ran() {
        let statements = vec![StatementSpec::from_sql("rows", "SELECT id FROM t")];
        let generated = GeneratedSql::build(&statements, &[]).expect("written SQL copies through");
        let unparseable = transformed_output("def transform(params, statements)\n  return {}");

        assert!(
            matches!(
                validate_endpoint(
                    &statements,
                    &[],
                    &cache(CacheScope::PerPrincipal),
                    &unparseable,
                    ConnectionSafety::UNGOVERNED,
                    ScriptEval::Here,
                ),
                Err(ValidationError::TransformScriptInvalid(_))
            ),
            "the evaluating caller must still refuse a script that does not parse"
        );

        assert!(
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                &unparseable,
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &generated
                },
            )
            .is_ok(),
            "publish's in-transaction re-validation must run no interpreter — a refusal \
             here means the Starlark evaluation is back inside the locked transaction"
        );

        let mut shape_broken = unparseable.clone();
        shape_broken.shape = "arrays".into();
        assert_eq!(
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                &shape_broken,
                ConnectionSafety::UNGOVERNED,
                ScriptEval::AlreadyRunBeforeTheLock {
                    generated: &generated
                },
            ),
            Err(ValidationError::TransformWithShapeOrNaming("shape")),
            "skipping the interpreter must not skip the comparisons: the re-validation \
             against the locked row is still a re-validation"
        );
    }

    /// **No Starlark evaluation happens inside `EndpointStore::publish`'s
    /// transaction**, read off this file's own text.
    ///
    /// A source scan rather than a behavioural test, and the reason is that the
    /// property is about WHERE a call sits, not about what it returns: publish
    /// needs a database, and every observable difference between evaluating
    /// before `begin()` and evaluating after it is a matter of how long a lock
    /// is held. `there_is_exactly_one_place_that_parses_starlark`
    /// (`crate::transform`) is the same instrument for the same kind of claim.
    ///
    /// It asserts its own markers were found before it asserts anything about
    /// them, so a renamed function or a restructured transaction fails loudly
    /// here instead of passing on an empty search.
    ///
    /// **This is a text scan of `publish`'s own body, not of the functions it
    /// calls — it has a blind spot.** The second assertion below finds
    /// `check_transform_script(&draft.output)?;` in the slice of source text
    /// between `publish`'s signature and `begin()`. Anyone who lifts that
    /// in-transaction re-validation into a helper function moves the call site
    /// (and the `check_transform_script(`/`eval_probe(`/… markers the first
    /// loop searches for) out of the scanned window entirely. The evaluation
    /// could then move back inside the lock — the exact regression this test
    /// exists to catch — while this test reads a `publish` body that no longer
    /// mentions any forbidden call and goes green.
    #[test]
    fn pin_no_starlark_evaluation_inside_publishs_transaction() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("lifecycle.rs"),
        )
        .expect("this file must be readable");

        let start_marker = "let mut tx = self.db.pool.begin().await.map_err(db_err)?;";
        let begin = src
            .find(start_marker)
            .expect("publish's `begin()` must be findable; if it moved, this test must move too");
        let commit = src[begin..]
            .find("tx.commit()")
            .expect("publish's commit must be findable")
            + begin;
        assert!(
            commit > begin,
            "the commit must come after the begin, or the window below is empty"
        );

        let window = &src[begin..commit];
        for forbidden in [
            "check_transform(",
            "check_transform_script(",
            "eval_probe(",
            "run_transform(",
            "probe_transform(",
            // Package S: a statement's SQL may be built by a `build_sql`
            // script, under the same budgets and therefore with the same
            // hazard. `GeneratedSql::build` is the only way to reach it from
            // this module.
            "GeneratedSql::build(",
            "transform::build_sql(",
        ] {
            assert!(
                !window.contains(forbidden),
                "`{forbidden}` appears between publish's `begin()` and its commit. \
                 transform::MAX_WALL_CLOCK is 2 s of TICKS, not of time — 28.8 s measured \
                 for a tick-expensive script — so an evaluation there holds the endpoint's \
                 row lock and a pool connection for as long as an author's script takes. \
                 See `ScriptEval`."
            );
        }

        // The other half: the evaluation must still HAPPEN, above the lock.
        // Without this the property is satisfiable by deleting the check.
        let publish_start = src
            .find("    pub async fn publish(")
            .expect("`publish` must be findable by name");
        assert!(
            src[publish_start..begin].contains("check_transform_script(&draft.output)?;"),
            "publish must still evaluate the script — above `begin()`, against the draft the identity check then proves identical to the locked row"
        );
        // The same both-halves argument for generation: it must happen, and it
        // must happen above the lock. Without this the property is satisfiable
        // by deleting the generation, which would leave every scripted
        // statement with no text for the re-validation to scan.
        assert!(
            src[publish_start..begin]
                .contains("GeneratedSql::build(&draft.statements, &draft.params)?;"),
            "publish must still generate every scripted statement — above `begin()`, against \
             the draft the identity check then proves identical to the locked row"
        );
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
        let statements = vec![StatementSpec::from_sql("rows", "SELECT id FROM t")];
        let mut output = transformed_output("def transform(params, statements):\n  return {}");
        output.shape = "arrays".into();

        let got = validate_endpoint(
            &statements,
            &[],
            &cache(CacheScope::PerPrincipal),
            &output,
            ConnectionSafety::UNGOVERNED,
            ScriptEval::Here,
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
        let statements = vec![StatementSpec::from_sql("rows", SQL)];
        let publish = |output: &OutputSpec| {
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                output,
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
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
            source: egret_dataapi_api::StatementSource::Sql("SELECT 1 AS a, 'x' AS b".into()),
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
            statements: vec![StatementSpec::from_sql(
                "rows",
                "SELECT 1 AS a, 'x' AS b FROM t",
            )],
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
            live_method: None,
            live_path: None,
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
            ConnectionSafety::UNGOVERNED,
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
        draft.statements = vec![StatementSpec::from_sql("rows", "SELECT 1 AS a FROM t WHERE r IN ({% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})")];

        let err = validate(
            &DescribeOnlyPort,
            Uuid::new_v4(),
            UserId::new(),
            &draft,
            ConnectionSafety::UNGOVERNED,
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
            ConnectionSafety::UNGOVERNED,
        )
        .await
        .expect("a conforming transform publishes");
        assert_eq!(frozen.statements.len(), 1);
        assert_eq!(
            frozen.statements[0]
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"],
            "the probe must not change what publish freezes"
        );
    }

    /// A `QueryExecPort` that answers each statement with the columns THAT
    /// statement aliases.
    ///
    /// `DescribeOnlyPort` returns the same two columns for every SQL it is
    /// handed, which is right for the transform probe — and useless here, where
    /// the whole question is whether publish described more than one text. A
    /// port that cannot tell two statements apart cannot tell a union apart from
    /// a single describe. `Describe::PerSql` in `tests/lifecycle.rs` exists for
    /// the same reason and says so.
    ///
    /// It panics rather than returning an empty list for a statement that
    /// aliases nothing: an empty describe is refused by
    /// `ValidationError::DescribeReturnedNoColumns`, which would reach a test as
    /// a plausible-looking failure about columns rather than as "this fake was
    /// asked something it cannot answer".
    struct AliasPort;

    #[async_trait::async_trait]
    impl egret_query_api::QueryExecPort for AliasPort {
        async fn describe_as(
            &self,
            _ws: Uuid,
            _database_id: Uuid,
            _principal: UserId,
            sql: &str,
        ) -> std::result::Result<Vec<egret_core::ColumnMeta>, Error> {
            let mut columns = Vec::new();
            let mut words = sql.split_whitespace();
            while let Some(word) = words.next() {
                if word.eq_ignore_ascii_case("AS") {
                    if let Some(name) = words.next() {
                        columns.push(egret_core::ColumnMeta {
                            name: name.trim_end_matches(',').to_owned(),
                            ty: "bigint".into(),
                        });
                    }
                }
            }
            assert!(
                !columns.is_empty(),
                "AliasPort was handed a statement that aliases nothing: {sql:?}"
            );
            Ok(columns)
        }

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
            unreachable!("publish must not execute a statement to freeze its contract")
        }
    }

    /// A draft whose one statement is generated by `script`.
    fn generated_draft(script: &str, params: Vec<ParamSpec>) -> DataApiEndpointDto {
        DataApiEndpointDto {
            statements: vec![StatementSpec::from_script("rows", script)],
            params,
            ..draft_with(OutputSpec::default(), probe_limits(10_000))
        }
    }

    /// **THE test of package S Task 5.**
    ///
    /// The script returns a different projection per branch, so the endpoint has
    /// one output schema per branch exactly as a `{% if %}` definition does.
    /// Publish must freeze BOTH columns, with the branch-specific one marked
    /// not-required — the union M1 already computes, over the set of texts a
    /// script produces.
    ///
    /// Freezing the first probe's columns alone is the mutation, and it is the
    /// `503 schema_drift` M1 was written to remove: `b` would be declared
    /// required and every call sending `wide=false` would be refused by
    /// `shape_rows` on a contract it could never satisfy.
    #[tokio::test]
    async fn a_generated_endpoints_columns_are_the_union_across_probes() {
        let script = "def build_sql(params):\n  \
                      if params[\"wide\"]:\n    return \"SELECT 1 AS a, 2 AS b\"\n  \
                      return \"SELECT 1 AS a\"";
        let frozen = validate(
            &AliasPort,
            Uuid::new_v4(),
            UserId::new(),
            &generated_draft(script, vec![param("wide", ParamType::Boolean)]),
            ConnectionSafety::UNGOVERNED,
        )
        .await
        .expect("both branches describe");
        assert_eq!(frozen.statements.len(), 1);
        let columns = &frozen.statements[0].columns;
        assert_eq!(
            columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
            "both of the script's branches are described, and the union keeps \
             first-seen order"
        );
        let by = |n: &str| {
            columns
                .iter()
                .find(|c| c.name == n)
                .unwrap_or_else(|| panic!("no column {n}"))
        };
        assert!(by("a").required, "`a` is in every branch");
        assert!(
            !by("b").required,
            "`b` is only in the branch `wide` takes, so it is declared and optional"
        );
    }

    /// The same union, from the other side: the extra column is in the branch
    /// the representative values do NOT take.
    ///
    /// **Not a restatement, and the mutation is why.** Freezing only the first
    /// probe's columns leaves the test above with `[a, b]` — the representative
    /// `wide` is `true`, so the one branch it describes is the WIDE one — and
    /// only its `required` assertion can see the mutation. Here the same
    /// mutation freezes `[a]` and `b` is gone from the contract altogether,
    /// which is the failure an author actually meets: `503 schema_drift` on
    /// every call that reaches the branch, from a published endpoint whose own
    /// OpenAPI never mentions the column.
    ///
    /// A test that could not distinguish those two directions would have let a
    /// "describe the first probe and the last" implementation through.
    #[tokio::test]
    async fn a_column_only_a_non_representative_branch_returns_is_still_declared() {
        let script = "def build_sql(params):\n  \
                      if params[\"narrow\"]:\n    return \"SELECT 1 AS a\"\n  \
                      return \"SELECT 1 AS a, 2 AS b\"";
        let frozen = validate(
            &AliasPort,
            Uuid::new_v4(),
            UserId::new(),
            &generated_draft(script, vec![param("narrow", ParamType::Boolean)]),
            ConnectionSafety::UNGOVERNED,
        )
        .await
        .expect("both branches describe");
        let columns = &frozen.statements[0].columns;
        assert_eq!(
            columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
            "`b` is returned only when `narrow` is falsy, which is the branch the \
             representative values do not take — and it must still be declared"
        );
        assert!(
            !columns[1].required,
            "`b` is in one branch of two, so it is declared and optional"
        );
    }

    /// And a script with ONE branch freezes exactly its columns — the feature is
    /// invisible to a script that does not branch.
    ///
    /// Not a weaker restatement of the test above: it is what says the union is
    /// a UNION rather than a widening. A probe set that described some text the
    /// script never returns, or a `required` computed from anything but "every
    /// described branch had it", shows up here as a column that should not be
    /// declared or a `required` that should be true.
    #[tokio::test]
    async fn a_generated_endpoint_with_one_branch_freezes_exactly_its_columns() {
        let script = "def build_sql(params):\n  return \"SELECT 1 AS a\"";
        let frozen = validate(
            &AliasPort,
            Uuid::new_v4(),
            UserId::new(),
            &generated_draft(script, vec![]),
            ConnectionSafety::UNGOVERNED,
        )
        .await
        .expect("one branch describes");
        assert_eq!(frozen.statements.len(), 1);
        assert_eq!(
            frozen.statements[0]
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert!(frozen.statements[0].columns[0].required);
    }

    /// A branch the representative values do not take is SCANNED, not merely
    /// described — Task 3's recorded residual, closed here.
    ///
    /// Task 3 measured that `;` and `#` in a generated statement have a
    /// render-time backstop but the HEAD WORD has none: a generated
    /// `DELETE FROM t` validated. That was harmless while one assignment was
    /// generated and that one assignment was the only text anything saw. Task 5
    /// makes the other branches real — `validate` hands each of them to
    /// `describe_as_with_params` — so an unscanned branch is a `DELETE` publish
    /// sends to the engine.
    ///
    /// Asserted on the arm rather than `is_err()`, because Task 3 found the
    /// shape where that lies: with the scan bypassed for generated text, `;` and
    /// `#` still failed, via a different guard. `NotASelect` has no such
    /// backstop, which is exactly why it is the one to assert.
    #[test]
    fn a_branch_the_representative_values_do_not_take_is_scanned_too() {
        let script = "def build_sql(params):\n  \
                      if params[\"wide\"]:\n    return \"SELECT 1 AS a\"\n  \
                      return \"DELETE FROM t\"";
        let err = validate_generated(script, &[param("wide", ParamType::Boolean)])
            .expect_err("the `wide=false` branch is not a SELECT");
        assert_eq!(err, ValidationError::NotASelect("DELETE".into()));
    }

    /// A script's branches and an endpoint's statements are ONE budget, and it
    /// is spent before the first render.
    ///
    /// Each parameter this script branches on emits its own template, so a
    /// statement's template count is `params + 1`. Nine of them is under the
    /// limit; two such statements is eighteen and is not — the count is the
    /// ENDPOINT's, which is the half a per-statement limit would miss.
    ///
    /// **Both halves are proven publishable alone**, in the shape
    /// `the_describe_budget_is_the_endpoints_total_not_one_per_statement` uses:
    /// a limit that refused the pair for some unrelated reason would pass a test
    /// that only asserted the refusal.
    #[test]
    fn a_scripts_branches_and_an_endpoints_statements_share_one_budget() {
        // Each parameter adds its own projection when it is FALSY, so the
        // sentinel probes reach a distinct template per parameter: 8 alternates
        // plus the baseline, per statement.
        let script = "def build_sql(params):\n  \
                      sql = \"SELECT 1 AS a\"\n  \
                      for k in sorted(params):\n    \
                      if not params[k]:\n      sql = sql + \", 1 AS \" + k\n  \
                      return sql";
        let params: Vec<ParamSpec> = (0..8)
            .map(|i| param(&format!("p{i}"), ParamType::Boolean))
            .collect();
        let one = |names: &[&str]| {
            validate_endpoint(
                &names
                    .iter()
                    .map(|n| StatementSpec::from_script(*n, script))
                    .collect::<Vec<_>>(),
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            )
        };
        assert!(
            one(&["rows"]).is_ok(),
            "nine templates is inside the allowance on its own"
        );
        assert_eq!(
            one(&["rows", "more"]),
            Err(ValidationError::TooManyGeneratedTemplates {
                templates: 2 * (params.len() + 1),
                limit: egret_dataapi_api::MAX_STATEMENTS,
            })
        );
    }

    /// And the allowance is the SAME one written statements spend, so an
    /// endpoint at the statement limit with no script is untouched.
    ///
    /// This is what says the budget was shared rather than lifted: sixteen
    /// written statements is exactly `MAX_STATEMENTS` templates, and a
    /// seventeenth text — from a script branching ONCE — is what tips it. A
    /// count that exempted written statements would leave the seventeenth text
    /// unbudgeted, which is the render `MAX_STATEMENTS` was measured to bound.
    #[test]
    fn a_written_statement_spends_from_the_same_allowance() {
        let mut statements: Vec<StatementSpec> = (0..egret_dataapi_api::MAX_STATEMENTS)
            .map(|i| StatementSpec::from_sql(format!("s{i}"), "SELECT 1 AS a"))
            .collect();
        let full = || {
            validate_endpoint(
                &statements,
                &[],
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            )
        };
        assert!(
            full().is_ok(),
            "`MAX_STATEMENTS` written statements is exactly the allowance"
        );
        // The SAME endpoint, with its last statement generated by a script that
        // branches once. One more text, and only one.
        statements.pop();
        statements.push(StatementSpec::from_script(
            "s15",
            "def build_sql(params):\n  \
             if params[\"wide\"]:\n    return \"SELECT 1 AS a, 2 AS b\"\n  \
             return \"SELECT 1 AS a\"",
        ));
        let params = vec![param("wide", ParamType::Boolean)];
        assert_eq!(
            validate_endpoint(
                &statements,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            ),
            Err(ValidationError::TooManyGeneratedTemplates {
                templates: egret_dataapi_api::MAX_STATEMENTS + 1,
                limit: egret_dataapi_api::MAX_STATEMENTS,
            })
        );
    }

    /// **A preview spends the same budget the endpoint it belongs to spends.**
    ///
    /// `MAX_STATEMENTS` bounds the templates an ENDPOINT renders, and
    /// [`GeneratedSql::build`] counts them across the slice it is handed. A
    /// preview holds ONE statement of that endpoint and hears about the rest as
    /// text, so counting from zero made preview accept a draft publish refuses —
    /// green here, refused there, which is the one thing that route exists not
    /// to do. `build_after` is what closes it.
    ///
    /// Both sides are computed in this test rather than one being asserted
    /// twice: the claim is that the two AGREE, so a number that drifts in either
    /// function turns this red. And both the pass and the refusal are asserted,
    /// one template apart — a preview that simply refused everything with
    /// siblings would satisfy the second half alone.
    #[test]
    fn a_preview_and_its_endpoint_share_one_template_budget() {
        // Nine templates: the baseline, plus one per parameter the sentinel
        // probes make falsy. Same shape as
        // `a_scripts_branches_and_an_endpoints_statements_share_one_budget`.
        const SCRIPT: &str = "def build_sql(params):\n  \
                              sql = \"SELECT 1 AS a\"\n  \
                              for k in sorted(params):\n    \
                              if not params[k]:\n      sql = sql + \", 1 AS \" + k\n  \
                              return sql";
        const TEMPLATES: usize = 9;
        let params: Vec<ParamSpec> = (0..8)
            .map(|i| param(&format!("p{i}"), ParamType::Boolean))
            .collect();

        // The endpoint as PUBLISH sees it: the scripted statement first, then
        // `n` written siblings of one template each.
        let published = |n: usize| {
            let mut statements = vec![StatementSpec::from_script("preview", SCRIPT)];
            statements
                .extend((0..n).map(|i| StatementSpec::from_sql(format!("s{i}"), "SELECT 1 AS a")));
            validate_endpoint(
                &statements,
                &params,
                &cache(CacheScope::PerPrincipal),
                &OutputSpec::default(),
                ConnectionSafety::UNGOVERNED,
                ScriptEval::Here,
            )
            .map(|_| ())
        };
        // The same endpoint as PREVIEW sees it: that one statement on screen,
        // its siblings as text.
        let previewed = |n: usize| {
            let texts: Vec<String> = (0..n).map(|_| "SELECT 1 AS a".to_string()).collect();
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            validate_preview(
                &StatementSource::Script(SCRIPT.to_string()),
                &siblings(&refs),
                &params,
                &OutputSpec::default(),
            )
            .map(|_| ())
        };

        // Exactly the allowance. Both accept, which is what stops the refusal
        // below being a preview that refuses any sibling at all.
        let fits = egret_dataapi_api::MAX_STATEMENTS - TEMPLATES;
        assert!(published(fits).is_ok(), "the endpoint is inside the budget");
        assert!(previewed(fits).is_ok(), "so the preview of it must be too");

        // One text more. Publish refuses; preview has to refuse the same draft,
        // with the same variant and the same numbers.
        let over = Err(ValidationError::TooManyGeneratedTemplates {
            templates: egret_dataapi_api::MAX_STATEMENTS + 1,
            limit: egret_dataapi_api::MAX_STATEMENTS,
        });
        assert_eq!(published(fits + 1), over);
        assert_eq!(previewed(fits + 1), over);

        // …and the sibling COUNT rule is not what did it: nine statements is
        // well inside `MAX_STATEMENTS`, so only the template budget can refuse
        // this.
        assert!(1 + fits + 1 < egret_dataapi_api::MAX_STATEMENTS);
    }
}
