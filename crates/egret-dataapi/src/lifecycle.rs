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
    CacheScope, CacheSpec, ColumnSpec, EndpointSnapshot, ParamSpec, ParamType,
};
use uuid::Uuid;

use crate::endpoints::{DataApiEndpointDto, EndpointStore};

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
         or a comment. Its value is substituted there as an ALREADY-QUOTED literal, which closes \
         the enclosing one and lets any caller inject SQL — write the placeholder as an operand \
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
fn strip_comments_and_strings(
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
/// Scans the RAW SQL, not the stripped form, because the renderer that will
/// substitute these at serve time is itself a raw scan (`egret-query`'s
/// `execution::template::render`). Matching its view is what keeps validation
/// honest: a placeholder inside a string literal WILL be substituted there, so
/// pretending it is inert here would declare a definition valid that behaves
/// differently in production.
fn placeholders(sql: &str) -> std::result::Result<Vec<String>, ValidationError> {
    let mut found = Vec::new();
    let mut rest = sql;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .ok_or_else(|| ValidationError::MalformedPlaceholder("unterminated `{{`".into()))?;
        let raw = &after[..close];
        let name = raw.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ValidationError::MalformedPlaceholder(format!(
                "{{{{{raw}}}}}"
            )));
        }
        found.push(name.to_string());
        rest = &after[close + 2..];
    }
    Ok(found)
}

/// Every `{{param}}` resolves to a declared parameter, and every declared
/// parameter is referenced. Both directions matter: an unreferenced parameter
/// is a lie in the generated OpenAPI, and an undeclared placeholder is a 500
/// waiting to happen at serve time.
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
/// `template::render` substitutes a placeholder with `quote_string(engine, v)` —
/// an **already-quoted** literal. That is safe exactly where a literal is what
/// the grammar expects. Inside an open literal it is the opposite:
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
    // The RAW scan, matching what `template::render` will do at serve time. A
    // placeholder inside a literal IS substituted there, so it counts as
    // referenced — pretending otherwise would report it as an unused parameter
    // and send the author to delete the declaration rather than move the
    // placeholder.
    let referenced = placeholders(sql)?;
    for name in &referenced {
        if !seen.contains(&name.as_str()) {
            return Err(ValidationError::UndeclaredPlaceholder(name.clone()));
        }
    }
    for p in params {
        if !referenced.iter().any(|r| r == &p.name) {
            return Err(ValidationError::UnreferencedParameter(p.name.clone()));
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

/// Run every shape check over a definition. The order is deliberate: the
/// statement check first, so a multi-statement string is refused before anything
/// downstream (including the engine) is asked to look at it.
pub fn validate_definition(
    sql: &str,
    params: &[ParamSpec],
    cache: &CacheSpec,
    delegated: bool,
    impersonate: bool,
) -> std::result::Result<(), ValidationError> {
    check_single_select(sql)?;
    check_enum_params(params)?;
    check_params(sql, params)?;
    check_cache_scope(cache, delegated, impersonate)?;
    Ok(())
}

/// A type-correct constant to stand in for a parameter while the engine resolves
/// the output schema.
///
/// No escaping is involved and none is needed: these are fixed constants chosen
/// here, never caller data. They exist because `describe` is given SQL to plan,
/// and `WHERE region = {{region}}` is not SQL — a template has to be rendered
/// before an engine will parse it. The VALUES do not matter (describe plans, it
/// does not execute); only the TYPES do, so that a comparison against a date or
/// numeric column still type-checks.
fn probe_literal(p: &ParamSpec) -> &'static str {
    match p.ty {
        ParamType::Number => "0",
        ParamType::Boolean => "true",
        ParamType::Date => "DATE '1970-01-01'",
        ParamType::Timestamp => "TIMESTAMP '1970-01-01 00:00:00'",
        ParamType::String | ParamType::Enum => "''",
    }
}

/// Render `sql` with every declared parameter replaced by [`probe_literal`], so
/// the engine has something it can plan.
///
/// Only meaningful after [`check_params`] has passed, which is what guarantees
/// every placeholder resolves; an unresolved one here would be a bug, so it is
/// reported as one rather than silently left in the string.
fn probe_sql(sql: &str, params: &[ParamSpec]) -> std::result::Result<String, ValidationError> {
    if !sql.contains("{{") {
        return Ok(sql.to_string());
    }
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .ok_or_else(|| ValidationError::MalformedPlaceholder("unterminated `{{`".into()))?;
        let name = after[..close].trim();
        let p = params
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| ValidationError::UndeclaredPlaceholder(name.to_string()))?;
        out.push_str(probe_literal(p));
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
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
    let probe = probe_sql(&draft.sql, &draft.params)?;
    let columns = port
        .describe_as(workspace, draft.database_id, principal, &probe)
        .await?;
    Ok(columns.iter().map(ColumnSpec::from).collect())
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
    /// `output`, `limits` and `rate_limit` went through the same window, and
    /// re-validation does NOT fix them — there is no rule for those fields
    /// anywhere in this crate, so there is nothing for the re-run to check. What
    /// they gain is consistency: they are frozen from the same read that was
    /// validated, rather than from a row nobody looked at. See the module
    /// comment's "What is not validated at all".
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
        // The three inputs the resolved `columns` depend on. A change to any of
        // them makes `columns` stale, which no amount of re-validation would
        // detect — only a fresh describe would, and that is the caller's to retry.
        if current.sql != draft.sql
            || current.database_id != draft.database_id
            || current.params != draft.params
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

        let state: String = sqlx::query_scalar(
            "UPDATE dataapi.endpoints SET live_version_id = $1, \
               state = CASE WHEN state = 'disabled' THEN 'disabled' ELSE 'live' END, \
               updated_at = now() \
             WHERE id = $2 AND workspace_id = $3 RETURNING state",
        )
        .bind(version_id)
        .bind(id)
        .bind(workspace)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

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
        let (version_id, version, state) =
            row.ok_or_else(|| Error::NotFound("endpoint version".into()))?;
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

    fn cache(scope: CacheScope) -> CacheSpec {
        CacheSpec {
            enabled: true,
            ttl_seconds: 60,
            scope,
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

    #[test]
    fn a_malformed_placeholder_is_rejected() {
        assert!(matches!(
            check_params("SELECT {{a b}}", &[]),
            Err(ValidationError::MalformedPlaceholder(_))
        ));
        assert!(matches!(
            check_params("SELECT {{a", &[]),
            Err(ValidationError::MalformedPlaceholder(_))
        ));
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

    /// The probe is what lets a PARAMETERIZED endpoint be described at all: an
    /// engine cannot plan `WHERE r = {{region}}`. The substituted values are
    /// typed, so a comparison against a date or numeric column still type-checks.
    #[test]
    fn a_template_is_rendered_to_typed_probe_literals() {
        let params = [
            param("r", ParamType::String),
            param("n", ParamType::Number),
            param("d", ParamType::Date),
        ];
        assert_eq!(
            probe_sql(
                "SELECT * FROM t WHERE r = {{r}} AND n > {{ n }} AND d < {{d}}",
                &params
            )
            .unwrap(),
            "SELECT * FROM t WHERE r = '' AND n > 0 AND d < DATE '1970-01-01'"
        );
        // No placeholders is identity.
        assert_eq!(probe_sql("SELECT 1", &[]).unwrap(), "SELECT 1");
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
