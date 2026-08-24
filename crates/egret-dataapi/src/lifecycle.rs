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
//! [`validate_definition`], which covers `sql`, `params` and `cache` only. What
//! those three fields get from the lock is CONSISTENCY — they are frozen from the
//! same read that was validated — not validation. A nonsensical `max_rows` or an
//! unknown `output.shape` will be frozen exactly as written.

use egret_core::{Error, Result, UserId};
use egret_dataapi_api::{
    CacheScope, CacheSpec, ColumnSpec, EndpointSnapshot, ParamLocation, ParamSpec, ParamType,
    PARAM_GATE,
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
    /// One STATEMENT returning two columns of the same name.
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
    /// So the check is on the name alone, and it runs per described statement
    /// before the union fold rather than inside it: a duplicate is a property of
    /// one statement's describe output, not of the fold.
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
    #[error(
        "this definition renders to {statements} structurally distinct statements, and publish \
         describes every one of them against the engine so the frozen contract covers every \
         branch a caller can reach. The limit is {limit}. Each optional parameter with no \
         `default` adds the branch its absence takes, and each one whose value can flip an \
         `{{% if %}}` — a boolean especially — multiplies the rest: give some of them a \
         `default` or mark them `required` so absence is not a separate branch, or split this \
         into two endpoints"
    )]
    TooManyBranches { statements: usize, limit: usize },
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
    /// with neither statement repeating a name, publishes green, and then:
    ///
    /// * every call, on either branch, is `503 output_naming_collision`; and
    /// * the generated OpenAPI fails at the DOCUMENT level, taking every
    ///   unrelated endpoint in the workspace down with it — the one failure
    ///   here whose blast radius is not this endpoint.
    ///
    /// So it is refused where the author is standing, and both declared names
    /// are quoted back, because renaming either one is the repair.
    #[error(
        "columns `{first}` and `{second}` both become `{key}` under          `output.naming: {naming}`, and a JSON object has one key per name — so one of them          could never be served. They come from different branches of this definition, which          is why neither statement looks wrong on its own. Alias one of them apart, or set          `output.naming: as_is`"
    )]
    OutputNamingCollision {
        key: String,
        first: String,
        second: String,
        naming: String,
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
        for p in params {
            if !read.iter().any(|r| r == &p.name) {
                return Err(ValidationError::UnreferencedParameter(p.name.clone()));
            }
        }
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
        if crate::serving::params::coerce(p, default).is_err() {
            return Err(ValidationError::DefaultNotValidForType(
                p.name.clone(),
                type_name(&p.ty),
            ));
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

/// Run every shape check over a definition. The order is deliberate: the
/// statement check first, so a multi-statement string is refused before anything
/// downstream (including the engine) is asked to look at it.
///
/// **The publish-time gate runs here, which is what makes it real.** This
/// function is the one both `POST …/validate` and [`EndpointStore::publish`] go
/// through — publish re-runs it inside its own transaction against the row it is
/// about to freeze — so "a definition that validates is exactly a definition
/// that publishes" keeps holding, and neither entry point can acquire the rules
/// without the other. Everything before [`check_render_rules`] is a scan;
/// [`check_render_rules`] renders, which is why it is last and why nothing on
/// the serving path calls it.
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
    check_render_rules(sql, params)?;
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
pub fn validate_preview(
    sql: &str,
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    check_definition_scans(sql, params, ParamRules::Draft)?;
    check_render_rules(sql, params)?;
    Ok(())
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
/// A parameter named as a `{% for %}` iterable is inserted as a one-element
/// LIST. `ParamSpec` has no multiplicity field — whether a parameter arrives as
/// a scalar or a list is decided by the request (a repeated query parameter
/// becomes a list) — so publish has to infer the shape the definition expects,
/// and it infers it from the definition. A wrong guess makes the render FAIL,
/// which refuses the publish; it cannot make a bad definition publish.
///
/// `absent` uses [`ParamValues::insert_absent`], not omission: under Strict
/// undefined a name that is merely missing is an ERROR even when tested for
/// truth, and "declared but not supplied" has to be `none` for `{% if %}` to be
/// false. That is the same distinction the serving path makes, and it is what
/// makes these renders representative of what will actually be served.
fn representative(sql: &str, params: &[ParamSpec], absent: &[&str]) -> ParamValues {
    context_with(sql, params, absent, representative_value)
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
/// the generated document advertises — see [`branch_statements`] for why it has
/// to be described in its own right.
fn resting(sql: &str, params: &[ParamSpec], absent: &[&str]) -> ParamValues {
    context_with(sql, params, absent, resting_value)
}

/// The shared body of [`representative`] and [`resting`]: the two differ only in
/// which value a present parameter carries, and everything else about building
/// the context — the list/scalar inference, and `insert_absent` rather than
/// omission — must not be able to drift between them.
fn context_with(
    sql: &str,
    params: &[ParamSpec],
    absent: &[&str],
    value: fn(&ParamSpec) -> Param,
) -> ParamValues {
    let declared: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
    let lists = crate::render::iterated_params(sql, &declared);
    let mut ctx = ParamValues::new();
    for p in params {
        if absent.contains(&p.name.as_str()) {
            ctx.insert_absent(p.name.clone());
        } else if lists.iter().any(|l| l == &p.name) {
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
fn check_render_rules(sql: &str, params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    check_ident_values(sql, params)?;
    let full = representative(sql, params, &[]);
    crate::render::check_publish(sql, &full)?;
    check_rendered(sql, &full)?;

    let optional: Vec<&str> = params
        .iter()
        .filter(|p| !p.required && p.default.is_none())
        .map(|p| p.name.as_str())
        .collect();
    for name in &optional {
        let ctx = representative(sql, params, &[name]);
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
        let none = representative(sql, params, &optional);
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
fn check_ident_values(sql: &str, params: &[ParamSpec]) -> std::result::Result<(), ValidationError> {
    for p in params.iter().filter(|p| p.ty == ParamType::Enum) {
        let Some(allowed) = p.allowed.as_ref().filter(|a| !a.is_empty()) else {
            continue;
        };
        if !enum_reaches_statement_text(sql, params, p, allowed) {
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
    p: &ParamSpec,
    allowed: &[String],
) -> bool {
    let render_with = |value: &str| {
        let mut ctx = representative(sql, params, &[]);
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

/// Validate a draft and resolve its output columns against the engine.
///
/// Shared by `POST /validate` and `POST /publish` so the two can never diverge:
/// a definition that validates is exactly a definition that publishes.
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
/// branch returns it. See [`branch_statements`] for which renderings that is and
/// [`describe_branches`] for how they are folded together.
pub async fn validate(
    port: &dyn egret_query_api::QueryExecPort,
    workspace: Uuid,
    principal: UserId,
    draft: &DataApiEndpointDto,
    delegated: bool,
    impersonate: bool,
) -> Result<Vec<ColumnSpec>> {
    validate_definition(
        &draft.sql,
        &draft.params,
        &draft.cache,
        delegated,
        impersonate,
    )?;
    let described = describe_branches(port, workspace, principal, draft).await?;
    // `arrays` is positional: `shape_rows` zips the live and frozen column
    // lists by index and `data_schema` documents the order in prose, because
    // OpenAPI 3.0 has no tuple type. A definition whose branches resolve to
    // different column lists — different names, different count, or different
    // order — has no single order to freeze, so it is refused here rather than
    // served under whichever branch happened to be described first. A
    // non-branching definition has exactly one entry in `per_statement`, so
    // this `any` never fires for it — the rule costs it nothing.
    if draft.output.shape == "arrays"
        && described
            .per_statement
            .iter()
            .any(|names| names != &described.per_statement[0])
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
    Ok(described.union)
}

/// What describing every branch of one definition produced.
pub(crate) struct DescribedBranches {
    /// Every column any branch returns, in first-seen order, with `required`
    /// set for the ones every branch returns.
    pub union: Vec<ColumnSpec>,
    /// The column NAMES each described statement resolved to, in describe
    /// order. The union throws away WHICH branch had which columns, and that is
    /// the question `output.shape = "arrays"` has to ask: an array row is
    /// positional, so two branches with different column lists cannot both be
    /// served under one frozen order. See [`ValidationError::BranchingArraysShape`].
    pub per_statement: Vec<Vec<String>>,
}

/// How many parameters one definition may declare.
///
/// # This is the bound on the half [`MAX_PUBLISHED_STATEMENTS`] does not cover
///
/// The statement limit is measured from the rendered set, so the rendering is
/// already paid for by the time it can refuse anything, and that rendering is
/// synchronous CPU inside an `async fn`. This limit is a scan over
/// `params.len()` in [`check_definition_scans`], before any render, and it is
/// what makes the multi-second case unrepresentable rather than merely refused.
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
/// describe (`branch_statements` plus [`check_render_rules`]), for *n*
/// parameters each guarding its own `{% if %}` block:
///
/// | *n* | all `string` (linear shape) | all `boolean` (the expensive shape) |
/// |---|---|---|
/// | 16 | 18 statements, 2.2 ms | 138 statements, 21 ms |
/// | 24 | 26 statements, 3.6 ms | 302 statements, 55 ms |
/// | **32** | **34 statements, 5.9 ms** | **530 statements, 122 ms** |
/// | 36 | — | 640 statements, 152 ms |
/// | 48 | 50 statements, 12.5 ms | — |
/// | 62 | 64 statements, 20.0 ms | — |
/// | 100 | — | 1.14 s |
/// | 400 | — | 18.2 s |
///
/// So `32`: eight times spec §4.3's canonical endpoint, which declares four. A
/// 32-parameter definition of the shape anyone actually writes costs 5.9 ms and
/// 34 statements, so it publishes comfortably — while the worst case the limit
/// still admits costs 122 ms of rendering before being refused, against the
/// 18.2 s that was reachable before. Above 32 the expensive curve is steep
/// (152 ms at 36, 284 ms at 50), and below it a legitimate wide endpoint starts
/// to feel it.
///
/// It also pulls the two limits apart, though it does not separate them
/// completely, and the doc used to claim it did. 62 purely linear parameters
/// render to exactly 64 statements, so without this bound an ordinary wide
/// endpoint could trip [`MAX_PUBLISHED_STATEMENTS`] on width alone; at 32 the
/// linear shape tops out at 34. But the `2n + 2` flag spelling —
/// `{% if p %} AND s = 'active' {% endif %}`, which `check_params` accepts
/// because `{% if p %}` counts as a reference — measures 58 at *n* = 28, 62 at
/// 30, **64 at 31 and 66 at 32**. So the two limits still MEET, at exactly the
/// width this bound admits: a 32-parameter definition of that shape is refused
/// by the statement limit rather than this one. That is a loud, actionable
/// refusal naming a real cost, not a correctness hole — but it is not the clean
/// separation the earlier wording promised.
const MAX_DECLARED_PARAMS: usize = 32;

/// How many structurally distinct statements one definition may publish.
///
/// # What it bounds, and what it does not
///
/// It bounds the ENGINE half: publish describes every distinct rendering, so
/// this is the number of round trips one publish may cost. It does **not** bound
/// the rendering that produces that number — the count has to be known before it
/// can be refused, so a definition still renders up to `64 × (optional + 2)`
/// times (`render::MAX_PROBES` assignments per context) before this limit can
/// say no. That residual is synchronous CPU on an async task, and it is bounded
/// by [`MAX_DECLARED_PARAMS`] instead, which is a scan and runs first. The two
/// limits are halves of one bound, not one limit and a spare.
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
/// | definition | distinct statements |
/// |---|---|
/// | spec §4.3's canonical endpoint (4 parameters, 3 `{% if %}`, a `{% for %}`) | 10 |
/// | *n* optional parameters whose blocks are OFF at the representative values | *n* + 1 or *n* + 2 — 22 at *n* = 20 |
/// | *n* optional parameters whose blocks are ON at the representative values | 12 at *n* = 4, 38 at *n* = 8, 57 at *n* = 10, **68 at *n* = 11**, 212 at *n* = 20 |
///
/// **The split is not by TYPE.** It is whether a parameter's candidate values
/// can turn its block OFF from a context where the representative values turn it
/// ON. When blocks start off, every probe turns exactly one on, and the same
/// single-block statement is reached from every context — they all collapse, and
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
/// statements), and admits ten independent always-on toggles while refusing
/// eleven. It is also `render::MAX_PROBES`, deliberately: that is already this
/// crate's stated answer to "how much may one definition cost", and two
/// different budgets would be two numbers to keep in step.
const MAX_PUBLISHED_STATEMENTS: usize = 64;

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
fn branch_statements(sql: &str, params: &[ParamSpec]) -> Vec<(String, usize)> {
    let optional: Vec<&str> = params
        .iter()
        .filter(|p| !p.required && p.default.is_none())
        .map(|p| p.name.as_str())
        .collect();
    let mut contexts = vec![representative(sql, params, &[])];
    for name in &optional {
        contexts.push(representative(sql, params, &[name]));
    }
    if optional.len() > 1 {
        contexts.push(representative(sql, params, &optional));
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
        contexts.push(resting(sql, params, &[]));
        if !optional.is_empty() {
            // The bare call in full: defaults applied AND every parameter that
            // has no default omitted.
            contexts.push(resting(sql, params, &optional));
        }
    }

    let mut statements: Vec<(String, usize)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ctx in &contexts {
        for (rendered, binds) in crate::render::render_permutations(sql, ctx) {
            if seen.insert(rendered.clone()) {
                // `binds.len()` is the `param_count` `describe_as_with_params`
                // wants — the same quantity the single-statement path passed.
                statements.push((rendered, binds.len()));
            }
        }
    }
    statements
}

/// Describe every branch and fold the results into one contract.
///
/// A `Vec` accumulator, not a map: the union is a PUBLISHED contract, and a
/// hash order would make two publishes of byte-identical input declare their
/// columns in different orders.
///
/// `required` is computed only once every statement has been described. Doing it
/// incrementally would mark the first statement's columns required before
/// anything knows whether a later branch omits them.
async fn describe_branches(
    port: &dyn egret_query_api::QueryExecPort,
    workspace: Uuid,
    principal: UserId,
    draft: &DataApiEndpointDto,
) -> Result<DescribedBranches> {
    let mut statements = branch_statements(&draft.sql, &draft.params);
    if statements.is_empty() {
        // Unreachable while `check_render_rules` has just rendered the
        // representative context, but `render_permutations` SKIPS an assignment
        // that does not render, so an empty result is silence rather than an
        // error. Render the representative context directly: its failure is the
        // one worth reporting, and its success is a statement to describe.
        let ctx = representative(&draft.sql, &draft.params, &[]);
        let (rendered, binds) =
            crate::render::render(&draft.sql, &ctx).map_err(ValidationError::Render)?;
        statements.push((rendered, binds.len()));
    }
    // Before the first describe, so an over-wide definition costs the author an
    // error rather than costing the engine a few hundred round trips.
    if statements.len() > MAX_PUBLISHED_STATEMENTS {
        return Err(ValidationError::TooManyBranches {
            statements: statements.len(),
            limit: MAX_PUBLISHED_STATEMENTS,
        }
        .into());
    }

    let mut union: Vec<ColumnSpec> = Vec::new();
    let mut per_statement: Vec<Vec<String>> = Vec::new();
    for (rendered, bind_count) in &statements {
        let columns = port
            .describe_as_with_params(
                workspace,
                draft.database_id,
                principal,
                rendered,
                *bind_count,
            )
            .await?;
        if columns.is_empty() {
            return Err(ValidationError::DescribeReturnedNoColumns.into());
        }
        // A duplicate WITHIN one statement, checked before the fold and without
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
        per_statement.push(names.iter().map(|n| (*n).to_string()).collect());
        for column in &columns {
            let spec = ColumnSpec::from(column);
            match union.iter().find(|u| u.name == spec.name) {
                // Reachable only ACROSS statements now — a duplicate inside one
                // was refused above, whatever its types.
                Some(existing) if existing.ty != spec.ty => {
                    return Err(ValidationError::ColumnTypeConflict {
                        name: spec.name,
                        first: existing.ty.clone(),
                        second: spec.ty,
                    }
                    .into());
                }
                Some(_) => {}
                None => union.push(spec),
            }
        }
    }
    for column in &mut union {
        column.required = per_statement
            .iter()
            .all(|names| names.contains(&column.name));
    }
    Ok(DescribedBranches {
        union,
        per_statement,
    })
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
    /// M1.** There are now two rules that read it —
    /// [`ValidationError::BranchingArraysShape`] reads `output.shape` and
    /// [`ValidationError::OutputNamingCollision`] reads `output.naming` — and
    /// both run in [`validate`], which happens BEFORE this transaction and
    /// against the pre-lock draft. Re-running `validate_definition` cannot
    /// re-check them either: it is the PURE validator and never sees `output`,
    /// while the two rules that do need the describe results. So `output` is
    /// handled the way `sql` is instead — by the identity check below, as a
    /// retryable conflict. Anyone adding a third `output` rule should put it
    /// beside those two and needs no new machinery here.
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
        columns: Vec<ColumnSpec>,
    ) -> Result<DataApiPublishedDto> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;

        let current = self.locked(&mut tx, workspace, id).await?;
        // The four inputs the resolved `columns` depend on. A change to any of
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
        if current.sql != draft.sql
            || current.database_id != draft.database_id
            || current.params != draft.params
            || current.output != draft.output
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
        validate_definition(
            &current.sql,
            &current.params,
            &current.cache,
            delegated,
            impersonate,
        )?;

        let snapshot = EndpointSnapshot {
            // The stored path, which `normalize_path` already canonicalised. The
            // snapshot must never carry a spelling the endpoint row does not have.
            sql: current.sql.clone(),
            method: current.method.clone(),
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
            columns,
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
    use egret_dataapi_api::{LimitsSpec, ParamLocation, RateLimitSpec};

    fn param(name: &str, ty: ParamType) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            ty,
            location: ParamLocation::Query,
            required: false,
            default: None,
            allowed: None,
            description: String::new(),
        }
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
        let (rendered, binds) =
            crate::render::render(sql, &representative(sql, &params, &[])).unwrap();
        assert_eq!(
            rendered, "SELECT * FROM t WHERE r = ? AND n > ? AND d < ?",
            "every branch a representative value reaches must be in the described statement"
        );
        assert_eq!(binds.len(), 3, "one bind per marker");
        // No parameters is identity.
        let (plain, none) =
            crate::render::render("SELECT 1", &representative("SELECT 1", &[], &[])).unwrap();
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
        assert!(validate_preview("SELECT 1; DROP TABLE t", &[]).is_err());
        assert!(validate_definition("SELECT 1; DROP TABLE t", &[], &cache, false, false).is_err());

        // A single statement that is not a SELECT, which is the half of
        // `check_single_select` NOTHING else re-runs. The separator case above is
        // covered twice over — `check_params` calls `reject_unscannable_bytes`
        // again on purpose (see its comment), so it alone cannot tell whether
        // `check_single_select` is still in the bundle. A bare DML statement can:
        // it carries no `;` and no `#`, and the render rules do not care what the
        // head word is.
        assert!(validate_preview("DELETE FROM t WHERE 1 = 1", &[]).is_err());
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
            description: String::new(),
        }];
        assert!(validate_preview("SELECT 1", &bad_enum).is_err());

        // A `location: body` parameter.
        let body_param = vec![ParamSpec {
            name: "region".into(),
            ty: ParamType::String,
            location: ParamLocation::Body,
            required: false,
            default: None,
            allowed: None,
            description: String::new(),
        }];
        assert!(validate_preview("SELECT 1", &body_param).is_err());

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
            validate_preview(&wide, &too_many),
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
        assert!(validate_preview("SELECT 1 AS n", &[]).is_ok());
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
            validate_preview(sql, &declared).is_ok(),
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
            validate_preview("SELECT * FROM t WHERE r = {{ nope }}", &[]).unwrap_err(),
            ValidationError::UndeclaredPlaceholder("nope".into())
        );
        // Still refused when OTHER parameters are declared, which is the shape
        // the loop above walks.
        assert_eq!(
            validate_preview(
                "SELECT * FROM t WHERE r = {{ nope }}",
                &[param("region", ParamType::String)]
            )
            .unwrap_err(),
            ValidationError::UndeclaredPlaceholder("nope".into())
        );
        // 2. A placeholder inside quoted text — the injection-adjacent rule.
        assert_eq!(
            validate_preview(
                "SELECT * FROM t WHERE a = 'x{{p}}y'",
                &[param("p", ParamType::String)]
            )
            .unwrap_err(),
            ValidationError::PlaceholderInsideQuotedText("p".into())
        );
        // 3. A duplicate declaration, which makes binding ambiguous.
        assert_eq!(
            validate_preview(
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
        assert!(validate_preview("SELECT 1 AS n # note", &[]).is_err());
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
        assert!(validate_preview("SELECT 1 AS n", &[]).is_ok());
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
}
