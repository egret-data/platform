//! The publish lifecycle: validate, publish, roll back, disable, re-enable.
//!
//! This is the module that makes versioning real rather than decorative. A draft
//! ([`crate::endpoints`]) is edited freely; publishing FREEZES it into an
//! immutable `dataapi.endpoint_versions.snapshot` row, including the output
//! column schema resolved by [`QueryExecPort::describe_as`], and points
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
//! **The one snapshot-external input P4 must re-check:**
//! `EndpointSnapshot::delegated` records whether the connection delegated
//! per-user credentials when the version was published. `query.databases.delegated`
//! is mutable, and flipping it to `true` after a `cache.scope = shared` version
//! was published turns a legitimately-published snapshot into the exact
//! cross-principal leak [`check_cache_scope`] refuses — with no re-validation
//! anywhere. P4 must compare the frozen value against the live one before serving
//! from a shared cache, and refuse when they disagree. Everything else in the
//! snapshot is self-contained; this field is a witness, and a witness nobody
//! questions is decoration.
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
//! [`check_single_select`] and [`check_params`] are shape checks over a scanner,
//! not a parser, and they are deliberately not claimed to be a complete
//! injection or statement-smuggling defence. Known and accepted gaps, all of
//! which need `data_api_endpoint:manage` (an authoring permission) to reach:
//!
//! * `#` line comments and MySQL/StarRocks `/*! … */` executable comments are not
//!   recognised as comments.
//! * `WITH … DELETE` and `SELECT … INTO OUTFILE` pass the head-word check.
//! * A Trino literal ending in a backslash (`'a\'`) is falsely REJECTED, because
//!   the escaping reading cannot terminate it. Rejection is the safe direction.
//!
//! What IS claimed, and tested: no accepted definition lets a *serve-time*
//! caller — who holds only a Data API token and can supply nothing but parameter
//! VALUES — reach anything other than a value position. That is the privilege
//! boundary; the items above sit on the authoring side of it.

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
    #[error("only one statement may be published; found a statement separator")]
    MultipleStatements,
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
    #[error(
        "cache.scope = shared is not allowed on a delegated connection: the engine filters rows \
         per principal, so a shared cache would serve one caller's rows to another"
    )]
    SharedCacheOnDelegatedConnection,
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
/// the `;`.
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
/// both, because the endpoint's engine can be changed after publish — `database_id`
/// is editable on the draft, and a connection's engine is not this crate's to pin.
const ESCAPING_READINGS: [bool; 2] = [true, false];

/// Reject anything that is not exactly one read-only statement.
///
/// This is a shape check, not a parser: it strips comments and string literals,
/// then looks at what is left. A leading `WITH … SELECT` (CTE) is allowed —
/// it is the normal way to write a readable analytical query. Anything else
/// (DML, DDL, `SET`, or a second statement after `;`) is refused.
///
/// Run under both entries of [`ESCAPING_READINGS`]; see
/// [`strip_comments_and_strings`] for why one pass is not enough.
pub fn check_single_select(sql: &str) -> std::result::Result<(), ValidationError> {
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

/// `Shared` cache is refused on a delegated connection: Ranger filters per
/// principal, so one principal's rows would be served to another. Refusing at
/// VALIDATE time means the unsafe configuration can never be published, rather
/// than being caught (or not) at serve time.
///
/// Refused whether or not the cache is currently `enabled`. The pair is frozen
/// into the snapshot together, and a `scope = shared, enabled = false` version
/// is one flag away from leaking with no re-validation in between — while an
/// author who wants a shared cache on a delegated connection has no correct way
/// to get one, so there is nothing legitimate being blocked.
pub fn check_cache_scope(
    cache: &CacheSpec,
    delegated: bool,
) -> std::result::Result<(), ValidationError> {
    if delegated && cache.scope == CacheScope::Shared {
        return Err(ValidationError::SharedCacheOnDelegatedConnection);
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
) -> std::result::Result<(), ValidationError> {
    check_single_select(sql)?;
    check_enum_params(params)?;
    check_params(sql, params)?;
    check_cache_scope(cache, delegated)?;
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
) -> Result<Vec<ColumnSpec>> {
    validate_definition(&draft.sql, &draft.params, &draft.cache, delegated)?;
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
    /// Whether `database` (which must belong to `workspace`) delegates per-user
    /// credentials to the engine.
    ///
    /// Read here rather than passed in: it is the input to
    /// [`check_cache_scope`], and a caller who supplied it would be supplying the
    /// answer to the safety question. `NotFound` for a database outside the
    /// workspace, matching every other cross-workspace answer in this crate.
    pub async fn connection_is_delegated(&self, workspace: Uuid, database: Uuid) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT delegated FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(database)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound("database".into()))
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
    /// unpublishable. `limits`, `rate_limit`, `output`, `method` and `path` went
    /// through the same gap.
    ///
    /// The fix is to re-run the whole PURE validator against `current`, inside
    /// the transaction. It costs no engine call and no extra pool time, and it
    /// makes the rule structural: what is frozen is what was validated, because
    /// they are now the same read. `delegated` is re-read under the same
    /// transaction and frozen into the snapshot, so the value validated, the value
    /// stored, and the value P4 will compare against are all one.
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

        // Same statement as `connection_is_delegated`, issued on the transaction
        // so the value cannot move between here and the commit.
        let delegated: bool = sqlx::query_scalar(
            "SELECT delegated FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(current.database_id)
        .bind(workspace)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound("database".into()))?;

        // The whole pure validator, against the row actually being frozen. See the
        // doc comment: this is what closes the window, not the identity check above.
        validate_definition(&current.sql, &current.params, &current.cache, delegated)?;

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
            delegated,
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

    #[test]
    fn a_semicolon_inside_a_literal_is_not_a_statement_break() {
        // The naive `contains(';')` check rejects this, which would make a
        // perfectly valid query unpublishable.
        assert!(check_single_select("SELECT 'a;b' AS s").is_ok());
    }

    #[test]
    fn a_semicolon_hidden_in_a_comment_does_not_smuggle_a_statement() {
        // ...and the mirror: stripping comments must happen before the scan, or
        // `-- ;` hides a second statement from the check.
        assert!(check_single_select("SELECT 1 -- ; DROP TABLE t\n").is_ok());
        assert!(check_single_select("SELECT 1 /* ; */ ; DROP TABLE t").is_err());
    }

    /// The escaping rules the two engines disagree about.
    #[test]
    fn quote_escapes_do_not_end_a_literal_early() {
        // `''` is the ANSI escape: the literal runs to the final quote, so the
        // `;` inside it is text.
        assert!(check_single_select("SELECT 'a''b;c' AS s").is_ok());
        // A `;` inside a quoted identifier is not a statement break either.
        assert!(check_single_select(r#"SELECT "a;b" FROM t"#).is_ok());
        assert!(check_single_select("SELECT `a;b` FROM t").is_ok());
    }

    /// **Neither escaping reading is safe alone, and this is why the scan runs
    /// twice.** Each payload is a genuine two-statement smuggle under exactly one
    /// dialect, and invisible to the scanner that assumes the other.
    #[test]
    fn a_backslash_smuggle_is_caught_by_the_other_reading() {
        // Terminating quote appended, so the ESCAPING reading closes on the final
        // `'` and sees `SELECT ''` — clean. Trino, which has no backslash escape,
        // reads `'a\'` then `;` then `DROP TABLE t`. Only the non-escaping pass
        // sees it, which is the whole point of running both.
        assert_eq!(
            check_single_select(r"SELECT 'a\' ; DROP TABLE t -- '"),
            Err(ValidationError::MultipleStatements)
        );
        // The same payload without the trailing quote: caught by the escaping
        // reading as an unterminated literal, and by the other as two statements.
        assert!(check_single_select(r"SELECT 'a\' ; DROP TABLE t").is_err());
        // And the mirror direction: a `;` that is genuinely inside a StarRocks
        // literal must not be read as a break just because the non-escaping pass
        // would end the literal early. Both passes must agree, so this is
        // rejected too — the safe direction, and a documented false rejection.
        assert!(check_single_select(r"SELECT 'a\'b;c' AS s").is_err());
    }

    /// Trino's `SIMPLE_COMMENT` is `'--' ~[\r\n]* '\r'? '\n'?`, so a bare CR ends
    /// the comment there. A scanner that stopped only at LF would swallow the
    /// rest of the line — including a `;` — into a "comment" Trino has already
    /// closed.
    #[test]
    fn a_line_comment_ends_at_a_bare_carriage_return() {
        assert_eq!(
            check_single_select("SELECT 1 --x\r; DROP TABLE t"),
            Err(ValidationError::MultipleStatements)
        );
        // The ordinary CRLF form still comments out its own line.
        assert!(check_single_select("SELECT 1 --x\r\nFROM t").is_ok());
    }

    #[test]
    fn an_unterminated_region_is_refused_rather_than_swallowed() {
        assert_eq!(
            check_single_select("SELECT 'oops"),
            Err(ValidationError::Unterminated("quoted literal"))
        );
        assert_eq!(
            check_single_select("SELECT 1 /* ; DROP TABLE t"),
            Err(ValidationError::Unterminated("block comment"))
        );
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

    #[test]
    fn shared_cache_is_refused_only_on_a_delegated_connection() {
        assert_eq!(
            check_cache_scope(&cache(CacheScope::Shared), true),
            Err(ValidationError::SharedCacheOnDelegatedConnection)
        );
        assert!(check_cache_scope(&cache(CacheScope::Shared), false).is_ok());
        assert!(check_cache_scope(&cache(CacheScope::PerPrincipal), true).is_ok());
    }

    /// A disabled shared cache is still refused — the flag is one edit away from
    /// enabled, and that edit does not re-run validation.
    #[test]
    fn a_disabled_shared_cache_is_still_refused_when_delegated() {
        let c = CacheSpec {
            enabled: false,
            ttl_seconds: 60,
            scope: CacheScope::Shared,
        };
        assert!(check_cache_scope(&c, true).is_err());
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
            ValidationError::SharedCacheOnDelegatedConnection,
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
