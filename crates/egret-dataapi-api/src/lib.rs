//! `egret-dataapi-api` — the Data API seam's contract crate (P2/P3).
//!
//! Mirrors `egret-query-api` / `egret-catalog-api` / `egret-policy-api`: a pure
//! leaf (no sqlx, axum, or heavy egret-* deps) so any cross-seam consumer can
//! depend on the Data API's wire vocabulary cheaply.
//!
//! P3 adds the endpoint-definition DTOs: `ParamSpec`/`ParamType`/`OutputSpec`
//! describe a draft endpoint's shape, and `EndpointSnapshot` is what a
//! publish freezes into `dataapi.endpoint_versions.snapshot` — self-contained
//! on purpose, since P4's serving path must answer a request from the
//! snapshot alone, without reading the draft row.

use serde::{Deserialize, Serialize};

mod port;
pub use port::DataApiEndpointPort;

/// The Cedar object KIND a Data API endpoint appears as in an authorization
/// decision, and the `access_grants.object_type` of an invoke grant (spec §5,
/// §6.2 step 4).
///
/// Canonical definition. `egret-dataapi`'s `endpoints::OBJECT_KIND`
/// re-exports this rather than keeping a second `"data_api_endpoint"`
/// literal — two string literals that must agree and are not the same
/// literal is exactly the drift a leaf contract crate exists to prevent.
/// `egret-automation` (Z2's alert authorization, Task 4) needs to name the
/// same kind to gate a `dataapi_alert` without depending on `egret-dataapi`
/// itself, which is why this lives here rather than in that crate.
pub const OBJECT_KIND: &str = "data_api_endpoint";

/// Permission that lets a caller list/inspect a Data API endpoint.
///
/// Canonical definition — see [`OBJECT_KIND`]'s doc comment for why this
/// lives here instead of in `egret-dataapi`, where it was previously a
/// private `const` in `routes.rs`.
pub const EP_READ: &str = "data_api_endpoint:read";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    String,
    Number,
    Boolean,
    Date,
    Timestamp,
    Enum,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Query,
    Body,
}

/// `deny_unknown_fields` on the author-supplied config types.
///
/// `#[serde(deny_unknown_fields)]` on the request-body struct does NOT reach a
/// nested one, and the nested one is where the damage was: posting the
/// documented-but-wrong `{"rate_limit": {"per_client_qps": 5}}` returned 201
/// with `per_key_qps` silently defaulted to `0`. A rate limit the author
/// believes they set and did not is worse than an error.
///
/// Applied to the six types an AUTHOR fills in — [`ParamSpec`], [`CacheSpec`],
/// [`LimitsSpec`], [`RateLimitSpec`], [`OutputSpec`], [`StatementSpec`] — and
/// deliberately NOT to [`EndpointSnapshot`], [`ColumnSpec`] or
/// [`StatementSnapshot`], which the server writes for itself and where a typo
/// is impossible.
///
/// **The cost, stated because it is a real constraint on a persisted format.**
/// These types are also how a stored `jsonb` column is read back. Adding a field
/// in some future version and then rolling the binary back would make the older
/// code refuse to deserialize rows the newer one wrote, as a 500 on read. That is
/// a loud failure rather than a silent one, and it is the standard price of
/// strict deserialization; a future field that must be readable by older code has
/// to be introduced some other way.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub location: ParamLocation,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Required when `ty` is `Enum`. The ONLY safe form for anything that
    /// reaches an identifier position — see spec §3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<String>>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    PerPrincipal,
    Shared,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheSpec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u32,
    /// `Shared` is refused at validate time on a delegated connection: Ranger
    /// filters per principal, so a shared cache would leak across identities.
    #[serde(default = "default_scope")]
    pub scope: CacheScope,
}
fn default_ttl() -> u32 {
    60
}
fn default_scope() -> CacheScope {
    CacheScope::PerPrincipal
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LimitsSpec {
    #[serde(default = "default_max_rows")]
    pub max_rows: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}
fn default_max_rows() -> u32 {
    10_000
}
fn default_timeout() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitSpec {
    #[serde(default)]
    pub per_key_qps: u32,
    #[serde(default)]
    pub burst: u32,
    #[serde(default)]
    pub per_endpoint_qps: u32,
}

/// What a published version freezes. Self-contained on purpose: P4's serving
/// path must be able to answer a request from this alone, without reading the
/// draft row.
///
/// **This type is read back out of an immutable column.** M2 does not migrate
/// `dataapi.endpoint_versions.snapshot` and must not: a published version is
/// the contract a live consumer is already coding against, and rewriting frozen
/// rows to suit a newer binary is the one thing "frozen" is supposed to
/// prevent. The compatibility therefore lives in the READER — see
/// [`EndpointSnapshotRepr`] — exactly as M1's `#[serde(default =
/// "default_required")]` on [`ColumnSpec::required`] did.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(try_from = "EndpointSnapshotRepr")]
pub struct EndpointSnapshot {
    /// The endpoint's N SQL definitions, in execution order, each with its own
    /// frozen columns (M1's per-branch union, now scoped to one statement).
    ///
    /// Never empty: [`EndpointSnapshotRepr`] refuses a snapshot that carries
    /// neither a `statements` list nor the pre-M2 `sql` + `columns` pair, and
    /// refuses an explicitly empty list.
    pub statements: Vec<StatementSnapshot>,
    pub method: String,
    pub path: String,
    pub database_id: uuid::Uuid,
    pub params: Vec<ParamSpec>,
    pub output: OutputSpec,
    pub cache: CacheSpec,
    pub limits: LimitsSpec,
    pub rate_limit: RateLimitSpec,
    /// Whether `database_id` delegated per-user credentials to the engine at the
    /// moment this version was published.
    ///
    /// **Half of a witness PAIR — see [`Self::impersonate`], and do not read
    /// this field alone.** Together they are the two mutable columns that decide
    /// whether the engine runs a query as the CALLER, which is the only thing
    /// that makes a shared cache unsafe. An earlier version of this comment, and
    /// of both checks that read it, treated `delegated` as the whole condition;
    /// it is not, and the gap was a live cross-principal leak.
    pub delegated: bool,
    /// Whether `query.databases.options.impersonate` was set at the moment this
    /// version was published. The other half of the witness pair.
    ///
    /// **`delegated` alone does not decide whether the engine runs per
    /// principal, and believing it did was a real leak.** `delegation_for`'s
    /// NON-delegated arm still returns `Some(principal_email)` as the session
    /// user (`egret-query/src/execution/mod.rs:86`), and both connectors act on
    /// `options.impersonate` independently of `delegated`:
    ///
    /// * `connector/trino.rs:43` — `bearer_token.is_some() || options.impersonate
    ///   == Some(true)` sends `X-Trino-User: <caller>`;
    /// * `connector/starrocks.rs:129` — `bearer_token.is_none() &&
    ///   options.impersonate == Some(true)` issues
    ///   `EXECUTE AS <caller> WITH NO REVERT`.
    ///
    /// So `{delegated: false, options: {impersonate: true}}` — which
    /// `DatabaseInput` accepts, the two fields being independent and unvalidated
    /// — runs every query under the caller's own Ranger policy while looking
    /// non-delegated to anything that reads `delegated`.
    ///
    /// **Both halves must be compared against the live row before serving from a
    /// shared cache.** Freezing only `delegated` let a `shared` endpoint on such
    /// a connection cache principal A's filtered rows under a principal-less key
    /// and serve them to principal B: a 200, `meta.cached: true`, nothing in
    /// `access_log` to distinguish it, and the witness silent because live
    /// `delegated` still equalled frozen `delegated`.
    ///
    /// `#[serde(default)]` so a snapshot written before this field existed
    /// deserialises as `false`. That is the fail-closed direction: such a
    /// snapshot can only have been published against a connection this check now
    /// refuses, so comparing `false` against a live `true` yields
    /// `cache_scope_stale` and a re-publish, which is the correct outcome.
    #[serde(default)]
    pub impersonate: bool,
    /// Which revision of the publish-time parameter gate this version passed.
    ///
    /// **A stamp, not a note.** P4.5's §7 said "every endpoint with parameters
    /// must be re-published after P4.5" — an instruction to an operator, which
    /// is not an enforcement. A version frozen before the gate existed carries
    /// `0` here (`#[serde(default)]`), so the serving path can REFUSE it rather
    /// than render a definition no rule ever looked at. That turns the migration
    /// from something someone has to remember into something the server checks.
    ///
    /// An integer rather than a bool, because the situation it exists for is
    /// exactly the one that recurs: when a future revision of the gate closes a
    /// rule the current one misses, bumping [`PARAM_GATE`] makes every snapshot
    /// stamped with the older gate refuse to serve until it is re-published and
    /// re-checked. A bool could only ever say "some gate ran", which is the claim
    /// that was not good enough this time either.
    ///
    /// Only consulted when the snapshot declares parameters: a version with no
    /// `params` renders nothing, so there is nothing for the gate to have missed
    /// and nothing to force a re-publish of.
    #[serde(default)]
    pub param_gate: u32,
}

impl EndpointSnapshot {
    /// **M2 BRIDGE. Task 9 deletes this** — parameter resolution (Task 6) and
    /// execution (Task 7) are already loops over [`Self::statements`], and
    /// `openapi_gen` is the last caller left; once document generation is a loop
    /// too, nothing calls this.
    ///
    /// It exists only so Task 4 can change the persisted shape without also
    /// changing how a request is served: every caller today assumes exactly one
    /// statement, and a raw `statements[0]` would spread an index-panic across
    /// the serving path to say so. `Option` instead of a panic because these
    /// callers are all fallible already, so "a snapshot with no statements"
    /// costs one `ok_or_else` and answers 500 rather than killing the task.
    ///
    /// `None` is unreachable for anything that came out of the database:
    /// [`EndpointSnapshotRepr`] refuses an empty list at deserialization.
    pub fn sole_statement(&self) -> Option<&StatementSnapshot> {
        self.statements.first()
    }
}

/// The name given to the single statement synthesized from a pre-M2 snapshot.
///
/// The same name migration 0129 gave every existing DRAFT, so a pre-M2
/// endpoint reads identically whether it is served from its frozen snapshot or
/// re-published from its migrated draft — two paths that would otherwise
/// disagree about what the endpoint's one statement is called.
///
/// A caller never sees it. Spec §4.4: `data` is an ARRAY for a one-statement
/// endpoint, and only becomes an object keyed by statement name once there is
/// more than one. So naming something that was previously anonymous changes
/// nothing on the wire for every endpoint this compatibility path exists for.
pub const LEGACY_STATEMENT_NAME: &str = "main";

/// How an [`EndpointSnapshot`] is READ. Not how it is written.
///
/// `dataapi.endpoint_versions.snapshot` is immutable, so the corpus contains
/// two shapes at once and always will: pre-M2 rows carrying a single `sql`
/// string with one `columns` list beside it, and M2 rows carrying
/// `statements`. Migrating the old ones was available and was rejected —
/// a published version is the frozen contract a live consumer is coding
/// against, and the precedent is M1's `#[serde(default = "default_required")]`
/// on [`ColumnSpec::required`], where the reader accommodated an old row
/// rather than an `UPDATE` rewriting it.
///
/// A separate representation type rather than keeping `sql`/`columns` as
/// `Option` fields on [`EndpointSnapshot`] itself: the legacy pair must be
/// readable and must never be WRITTEN again, and an `Option` field that
/// serde can still serialize is one `skip_serializing_if` away from putting
/// the old shape back into a new row. Here it is unrepresentable — the fields
/// exist only on the way in.
#[derive(Deserialize)]
struct EndpointSnapshotRepr {
    /// Present on every snapshot published from M2 onwards.
    #[serde(default)]
    statements: Option<Vec<StatementSnapshot>>,
    /// The pre-M2 pair. Required together, exactly as they were required
    /// before M2 — neither carried a serde default, so a snapshot missing
    /// either one has never deserialized and does not start now.
    #[serde(default)]
    sql: Option<String>,
    #[serde(default)]
    columns: Option<Vec<ColumnSpec>>,
    method: String,
    path: String,
    database_id: uuid::Uuid,
    params: Vec<ParamSpec>,
    output: OutputSpec,
    cache: CacheSpec,
    limits: LimitsSpec,
    rate_limit: RateLimitSpec,
    delegated: bool,
    #[serde(default)]
    impersonate: bool,
    #[serde(default)]
    param_gate: u32,
}

/// What the reader refuses. A snapshot in neither shape is corrupt data, and
/// saying so is better than an endpoint that deserializes with nothing to run
/// and discovers it at request time.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SnapshotShapeError {
    #[error(
        "a published snapshot must carry a non-empty `statements` list, or the pre-M2 `sql` \
         and `columns` pair it is read as one statement from; this one carries neither"
    )]
    NoStatements,
}

impl TryFrom<EndpointSnapshotRepr> for EndpointSnapshot {
    type Error = SnapshotShapeError;

    fn try_from(r: EndpointSnapshotRepr) -> Result<Self, Self::Error> {
        let statements = match (r.statements, r.sql, r.columns) {
            // M2 and later. The legacy pair is ignored if some future row
            // somehow carries both: `statements` is the authority, so a
            // multi-statement endpoint can never be flattened to its first.
            (Some(s), _, _) if !s.is_empty() => s,
            // Pre-M2: one anonymous SQL string and the columns frozen beside
            // it become one named statement. Both halves are required, which
            // is what the old struct already demanded of them.
            (_, Some(sql), Some(columns)) => vec![StatementSnapshot {
                name: LEGACY_STATEMENT_NAME.to_string(),
                sql,
                columns,
            }],
            _ => return Err(SnapshotShapeError::NoStatements),
        };
        Ok(EndpointSnapshot {
            statements,
            method: r.method,
            path: r.path,
            database_id: r.database_id,
            params: r.params,
            output: r.output,
            cache: r.cache,
            limits: r.limits,
            rate_limit: r.rate_limit,
            delegated: r.delegated,
            impersonate: r.impersonate,
            param_gate: r.param_gate,
        })
    }
}

/// The current revision of the publish-time parameter gate — what
/// [`EndpointSnapshot::param_gate`] is stamped with, and the minimum the serving
/// path accepts on a snapshot that declares parameters.
///
/// `0` is reserved for "published before the gate existed" and is never written.
pub const PARAM_GATE: u32 = 1;

fn default_required() -> bool {
    true
}

/// The frozen form of a `ColumnMeta`. Deliberately its own type rather than
/// re-using `egret_core::ColumnMeta`: this one is persisted inside a published
/// snapshot, so it must stay stable even if the engine-facing type gains or
/// renames a field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: String,
    /// Present in EVERY permutation described at publish.
    ///
    /// Defaults to `true` when absent, which is what a snapshot written before
    /// this field existed carries. Those columns were checked for equality on
    /// every call, so `true` is the only default that does not loosen an
    /// already-published contract.
    #[serde(default = "default_required")]
    pub required: bool,
}

impl From<&egret_core::ColumnMeta> for ColumnSpec {
    fn from(c: &egret_core::ColumnMeta) -> Self {
        // ColumnMeta is { name: String, ty: String } — verified at
        // crates/egret-core/src/tabular.rs:11-15.
        Self {
            name: c.name.clone(),
            ty: c.ty.clone(),
            // Publish downgrades this to `false` for a column that only some
            // permutation returns; a single describe means every column is
            // required, which is the pre-M1 behaviour.
            required: true,
        }
    }
}

/// One of an endpoint's N SQL definitions, as an author writes it (M2). The
/// canonical shape is a `rows` statement and a `total` statement for
/// pagination, but any number is allowed.
///
/// `name` is not decoration: once an endpoint has more than one statement,
/// the response envelope is a JSON object keyed by statement name and the
/// generated OpenAPI document names it as a property (M2 Tasks 8-9). A name a
/// caller writes into their own code is a contract, which is what
/// [`validate_statement_name`] and [`validate_statements`] enforce —
/// `^[a-z][a-z0-9_]*$` so the key never needs quoting in JSON and never reads
/// as private, and uniqueness within the endpoint so the key is unambiguous.
///
/// `#[serde(deny_unknown_fields)]` for the same reason as [`ParamSpec`] and
/// its four siblings: this is a type an AUTHOR fills in, so a typo in the
/// request body must be a 400, not a silently-ignored field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StatementSpec {
    pub name: String,
    pub sql: String,
}

/// The frozen form of a [`StatementSpec`]: what a publish freezes into
/// [`EndpointSnapshot`] for one statement, its own [`ColumnSpec`] union
/// included (M1's per-branch column union, now scoped to one statement of N).
///
/// No `deny_unknown_fields` — like [`EndpointSnapshot`] and [`ColumnSpec`],
/// the server writes this for itself; a typo here is not possible the way one
/// is in a request body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct StatementSnapshot {
    pub name: String,
    pub sql: String,
    pub columns: Vec<ColumnSpec>,
}

/// What [`validate_statement_name`] and [`validate_statements`] refuse.
///
/// One enum rather than two, because [`validate_statements`] is defined in
/// terms of [`validate_statement_name`] and a caller that matches on the
/// result of either should not have to match two different types for the
/// same underlying question ("is this name/list well-formed?").
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StatementError {
    /// `name` does not match `^[a-z][a-z0-9_]*$`.
    #[error(
        "statement name {0:?} is not a valid identifier: it must match `^[a-z][a-z0-9_]*$` — \
         lowercase ASCII letters, digits and `_`, starting with a lowercase letter. A name \
         becomes a JSON object key and an OpenAPI property once an endpoint has more than one \
         statement, so it may not need quoting and may not read as private"
    )]
    InvalidName(String),
    /// Two statements in the same endpoint declared the same name.
    #[error(
        "statement name `{0}` is used by more than one statement in this endpoint; the response \
         envelope keys results by name, so two statements cannot share one"
    )]
    DuplicateName(String),
    /// The list had no statements at all.
    #[error("an endpoint must declare at least one statement")]
    Empty,
}

/// `^[a-z][a-z0-9_]*$` — see [`StatementSpec`]'s doc comment for why this
/// exact shape and not, say, camelCase or a leading underscore for "private".
pub fn validate_statement_name(name: &str) -> Result<(), StatementError> {
    let mut chars = name.chars();
    let starts_with_lowercase_letter = matches!(chars.next(), Some(c) if c.is_ascii_lowercase());
    let rest_is_lowercase_alnum_or_underscore =
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if starts_with_lowercase_letter && rest_is_lowercase_alnum_or_underscore {
        Ok(())
    } else {
        Err(StatementError::InvalidName(name.to_string()))
    }
}

/// Every statement's name is a valid identifier, no two statements share a
/// name, and the list is non-empty — the three rules that make "keyed by
/// statement name" (M2 Tasks 8-9) a safe thing to build a response around.
pub fn validate_statements(statements: &[StatementSpec]) -> Result<(), StatementError> {
    if statements.is_empty() {
        return Err(StatementError::Empty);
    }
    let mut seen = std::collections::HashSet::with_capacity(statements.len());
    for statement in statements {
        validate_statement_name(&statement.name)?;
        if !seen.insert(statement.name.as_str()) {
            return Err(StatementError::DuplicateName(statement.name.clone()));
        }
    }
    Ok(())
}

/// Whether two SQL type names describe the same column, and — when they do but
/// are spelled differently — which spelling carries more information.
///
/// One type rather than a `bool` plus a separate "which is more specific?"
/// helper, because the two questions cannot be asked independently **safely**.
/// A helper that answers "the more specific of these two" has no correct answer
/// for a pair that does not agree at all, so it must either return something
/// misleading (the `more_specific_type` this replaced answered
/// `more_specific_type("varchar", "integer")` with `"varchar"`) or be
/// documented as "only call this once the other one said yes" — a convention,
/// which the next caller is free not to know. Folding the choice into the
/// answer makes the unguarded call **unrepresentable**: there is no way to
/// obtain a preferred spelling without having been handed, and having had to
/// handle, [`TypeAgreement::No`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeAgreement {
    /// Byte-identical.
    Same,
    /// They describe the same column, and the LEFT argument is the spelling
    /// that states the datetime precision.
    PreferLeft,
    /// They describe the same column, and the RIGHT argument is the spelling
    /// that states the datetime precision.
    PreferRight,
    /// They do not describe the same column. This is schema drift.
    No,
}

impl TypeAgreement {
    /// True unless this is [`TypeAgreement::No`] — for the call sites that need
    /// only the gate, not the spelling.
    pub fn agrees(self) -> bool {
        !matches!(self, TypeAgreement::No)
    }
}

/// Does a live column's type name still honour the one frozen at publish?
///
/// Exact equality, plus ONE measured exception: **Trino suppresses a datetime
/// type's precision on the client protocol unless the caller declares the
/// `PARAMETRIC_DATETIME` capability**, and egret's `TrinoConnector` does not
/// declare it. Measured against Trino 483, same statement, one header apart:
///
/// ```text
/// no capability header                             : [('c','timestamp'),    ('t','time'),    ('d','decimal(10, 2)')]
/// X-Trino-Client-Capabilities: PARAMETRIC_DATETIME : [('c','timestamp(0)'), ('t','time(0)'), ('d','decimal(10, 2)')]
/// ```
///
/// `DESCRIBE OUTPUT` — the only way to type a *parameterised* query without
/// running it, and therefore the publish path for every endpoint that declares
/// a parameter — returns its answer as ordinary result **rows**, which the
/// capability flag does not touch. So one and the same column freezes as
/// `timestamp(0)` and serves as `timestamp`, and the endpoint answers
/// `503 schema_drift` on every call, forever: re-publishing re-runs
/// `DESCRIBE OUTPUT` and re-freezes `timestamp(0)`.
///
/// # Why this is a comparison and not a normalisation at the connector
///
/// Because **both spellings are already in the published corpus**, measured
/// live on one server: an endpoint with NO parameters freezes through
/// `Connector::describe` (whose default implementation *is* `execute` over a
/// `LIMIT 0` wrapper), so its snapshot carries the bare `timestamp`, while an
/// endpoint WITH parameters carries `timestamp(0)`. No change to what the
/// connector reports can satisfy both — making the live side spell out the
/// precision fixes the parameterised endpoints and breaks the others. Only the
/// comparison sees both sides, so only the comparison can reconcile them
/// without anybody re-publishing anything.
///
/// # What this deliberately does NOT relax
///
/// A *suppressed* precision and a *stated* one are reconciled; two stated
/// precisions are still compared. `timestamp(3)` vs `timestamp(6)` is
/// [`TypeAgreement::No`] — the pair is accepted only when one side never said
/// anything about precision, which is the case where there is no information to
/// compare rather than a case where it disagrees.
///
/// That is also **forward-safe**: if
/// `X-Trino-Client-Capabilities: PARAMETRIC_DATETIME` is ever adopted, both
/// sides state a precision, the relaxing clause stops firing, and full
/// detection returns for every re-published endpoint with no code change here.
///
/// StarRocks is structurally untouched: its connector names every column
/// through a lossy `coltype_name` that already throws away every type
/// parameter, so no StarRocks type string contains a parenthesis at all and
/// [`erase_datetime_precision`] is the identity on all of them.
pub fn compare_column_types(left: &str, right: &str) -> TypeAgreement {
    if left == right {
        return TypeAgreement::Same;
    }
    let l = erase_datetime_precision(left);
    let r = erase_datetime_precision(right);
    if l != r {
        return TypeAgreement::No;
    }
    // At least one side must have carried no precision to begin with. Without
    // this clause the function would also equate `timestamp(3)` with
    // `timestamp(6)`, which is a real type change nobody suppressed.
    //
    // `l == left` means nothing was erased, i.e. `left` is the precision-FREE
    // spelling — so the other one is the informative one.
    match (l == left, r == right) {
        (true, false) => TypeAgreement::PreferRight,
        (false, true) => TypeAgreement::PreferLeft,
        // Both precision-free and still unequal is impossible: the erased forms
        // matched and neither string was rewritten, so the originals were equal
        // and the first line already returned. Both precision-BEARING is the
        // `timestamp(3)` vs `timestamp(6)` case, which is genuine drift.
        (true, true) | (false, false) => TypeAgreement::No,
    }
}

/// Remove the precision argument from every `time`/`timestamp` token in a SQL
/// type name, leaving everything else — including `decimal(10, 2)`,
/// `varchar(20)` and the `with time zone` suffix — byte-identical.
///
/// A token scan rather than a type-grammar parser, and deliberately narrow in
/// three ways so it cannot erase anything else:
///
/// * it only fires on a complete identifier equal to `time` or `timestamp`, so
///   StarRocks' `datetime` (one token) is untouched, and so is the bare `time`
///   inside `with time zone` (no `(` follows it);
/// * it only removes a parenthesised group whose content is a run of ASCII
///   digits, which a precision always is and `decimal(10, 2)`'s argument list
///   never is;
/// * it recurses structurally for free, because it scans tokens rather than
///   the outermost type — `array(timestamp(0))` and `map(varchar(1),
///   timestamp(0))` normalise correctly without special cases.
fn erase_datetime_precision(ty: &str) -> String {
    let mut out = String::with_capacity(ty.len());
    let mut rest = ty;
    while let Some(first) = rest.chars().next() {
        if !(first.is_ascii_alphabetic() || first == '_') {
            out.push(first);
            rest = &rest[first.len_utf8()..];
            continue;
        }
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let (word, tail) = rest.split_at(end);
        out.push_str(word);
        rest = tail;
        if word.eq_ignore_ascii_case("time") || word.eq_ignore_ascii_case("timestamp") {
            if let Some(after) = strip_precision_arg(rest) {
                rest = after;
            }
        }
    }
    out
}

/// `"(3) with time zone"` → `Some(" with time zone")`. `None` for anything that
/// is not a parenthesised, non-empty run of ASCII digits — so a `(` that opens
/// an argument LIST, or a type that simply has no precision, is left alone.
fn strip_precision_arg(rest: &str) -> Option<&str> {
    let after_open = rest.strip_prefix('(')?;
    let close = after_open.find(')')?;
    let (arg, tail) = after_open.split_at(close);
    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(&tail[1..])
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    #[serde(default = "default_shape")]
    pub shape: String, // "objects" | "arrays"
    #[serde(default = "default_naming")]
    pub naming: String, // "as_is" | "camel" | "snake"
    /// Whether the response carries the `{status, message, data, meta}` wrapper
    /// (`"wrapped"`, the default) or `data`'s value on its own (`"bare"`) —
    /// spec §4.4.
    ///
    /// Defaulted rather than made an `Option`, and defaulted to the WRAPPER, so
    /// that every snapshot frozen before this field existed reads back as
    /// `wrapped`: the migration sets no rows and an unpublished endpoint cannot
    /// change shape by being re-read.
    ///
    /// `bare` is the success path only. A refusal still answers with
    /// `ServingErrorBody`, because an unwrapped error would be a bare `null` —
    /// nothing at all about what went wrong — under a status this toggle does
    /// not reach.
    ///
    /// A `String` like `shape` and `naming`, and unvalidated like them: spec
    /// §5.3 freezes `output` with no rule anywhere, so serving falls back to the
    /// documented default and logs rather than refusing (see
    /// `serving::router::envelope_of`).
    #[serde(default = "default_envelope")]
    pub envelope: String, // "wrapped" | "bare"
}
fn default_shape() -> String {
    "objects".into()
}
fn default_naming() -> String {
    "as_is".into()
}
fn default_envelope() -> String {
    "wrapped".into()
}

// Hand-written, not `#[derive(Default)]`: a derive builds each field from
// ITS OWN `Default` impl (`String::default()` is `""`), which has nothing to
// do with the `#[serde(default = "...")]` functions above — a derive here
// would silently diverge from `serde_json::from_str::<OutputSpec>("{}")` and
// hand the first caller of `OutputSpec::default()` empty-string output config
// with no error. Routing through the same functions serde uses makes the two
// paths structurally unable to disagree.
impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            shape: default_shape(),
            naming: default_naming(),
            envelope: default_envelope(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins `OutputSpec::default()` to agree with what an absent `output`
    /// field deserializes to. A `#[derive(Default)]` here would pass every
    /// other test in this crate while quietly handing callers `{ shape: "",
    /// naming: "" }` — this is the one assertion that would catch it.
    #[test]
    fn default_output_spec_matches_the_serde_default() {
        let from_empty_json: OutputSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(OutputSpec::default(), from_empty_json);
        assert_eq!(OutputSpec::default().shape, "objects");
        assert_eq!(OutputSpec::default().naming, "as_is");
        // The one that matters on the WIRE: a snapshot frozen before M2 carries
        // no `envelope`, and reading it back as anything but `wrapped` would
        // change the response shape of every endpoint published since
        // 2026-08-16 without anybody re-publishing.
        assert_eq!(OutputSpec::default().envelope, "wrapped");
    }

    #[test]
    fn a_snapshot_written_before_required_existed_deserializes_as_required() {
        // Exactly the shape migration 0121 rows carry today.
        let old = r#"{"name":"id","ty":"integer"}"#;
        let c: ColumnSpec = serde_json::from_str(old).unwrap();
        assert!(
            c.required,
            "a column frozen before M1 was checked for equality on every call; \
             defaulting it to optional would loosen the gate on the whole fleet"
        );
    }

    #[test]
    fn a_described_column_is_required_until_publish_says_otherwise() {
        let meta = egret_core::ColumnMeta {
            name: "id".into(),
            ty: "integer".into(),
        };
        assert!(ColumnSpec::from(&meta).required);
    }

    // --- statement naming ------------------------------------------------
    //
    // A statement's name becomes a JSON object key in the response envelope
    // and an OpenAPI property name once an endpoint has more than one
    // statement (M2 Tasks 8-9), so the rule is a contract with a caller's
    // generated client, not a style preference.

    #[test]
    fn a_statement_name_must_be_an_identifier() {
        for bad in ["Rows", "1st", "row count", "rows-total", "", "_rows"] {
            assert!(validate_statement_name(bad).is_err(), "accepted {bad:?}");
        }
        for good in ["rows", "row_count", "r2"] {
            assert!(validate_statement_name(good).is_ok(), "refused {good:?}");
        }
    }

    #[test]
    fn two_statements_may_not_share_a_name() {
        let statements = vec![
            StatementSpec {
                name: "rows".into(),
                sql: "SELECT 1".into(),
            },
            StatementSpec {
                name: "rows".into(),
                sql: "SELECT 2".into(),
            },
        ];
        assert_eq!(
            validate_statements(&statements),
            Err(StatementError::DuplicateName("rows".into()))
        );
    }

    #[test]
    fn an_endpoint_needs_at_least_one_statement() {
        assert_eq!(validate_statements(&[]), Err(StatementError::Empty));
    }

    #[test]
    fn a_single_valid_statement_is_accepted() {
        let statements = vec![StatementSpec {
            name: "rows".into(),
            sql: "SELECT 1".into(),
        }];
        assert!(validate_statements(&statements).is_ok());
    }

    #[test]
    fn an_invalid_name_inside_the_list_is_refused_by_the_list_validator() {
        let statements = vec![StatementSpec {
            name: "Rows".into(),
            sql: "SELECT 1".into(),
        }];
        assert_eq!(
            validate_statements(&statements),
            Err(StatementError::InvalidName("Rows".into()))
        );
    }

    // --- the snapshot's statement list, and what an old row deserializes to ---
    //
    // `dataapi.endpoint_versions.snapshot` is immutable: a published version is
    // the contract a live consumer is already coding against, so M2 migrates
    // NOTHING there. The reader accommodates the old shape instead — the same
    // call M1 made with `#[serde(default = "default_required")]` above.

    /// Exactly the shape rows published before M2 carry: `sql` + `columns`,
    /// no `statements`.
    fn a_snapshot_as_published_before_m2() -> String {
        r#"{
            "sql": "SELECT 1 AS a",
            "columns": [{"name":"a","ty":"integer","required":true}],
            "method": "GET",
            "path": "old",
            "database_id": "00000000-0000-0000-0000-000000000001",
            "params": [],
            "output": {"shape":"objects","naming":"as_is"},
            "cache": {"enabled":false,"ttl_seconds":60,"scope":"per_principal"},
            "limits": {"max_rows":10000,"timeout_seconds":30},
            "rate_limit": {"per_key_qps":0,"burst":0,"per_endpoint_qps":0},
            "delegated": false,
            "param_gate": 1
        }"#
        .to_string()
    }

    #[test]
    fn a_snapshot_published_before_m2_reads_as_one_statement() {
        let s: EndpointSnapshot = serde_json::from_str(&a_snapshot_as_published_before_m2())
            .expect("a snapshot already serving traffic must still deserialize");
        assert_eq!(
            s.statements.len(),
            1,
            "one `sql` is one statement — not zero, and not a list the serving \
             path would iterate emptily over"
        );
        assert_eq!(s.statements[0].sql, "SELECT 1 AS a");
        assert_eq!(s.statements[0].columns.len(), 1);
        assert_eq!(s.statements[0].columns[0].name, "a");
        assert!(
            s.statements[0].columns[0].required,
            "M1's frozen contract must survive the fold, not be re-derived"
        );
    }

    /// The synthesized name is `main` — the same name migration 0129 gave every
    /// existing DRAFT, so a pre-M2 endpoint reads the same whether it is served
    /// from its frozen snapshot or re-published from its migrated draft.
    ///
    /// It is invisible to a caller: spec §4.4 says `data` is an ARRAY for a
    /// one-statement endpoint and only becomes an object keyed by name once
    /// there is more than one. Nothing a pre-M2 consumer receives changes.
    #[test]
    fn the_synthesized_statement_is_named_main() {
        let s: EndpointSnapshot =
            serde_json::from_str(&a_snapshot_as_published_before_m2()).unwrap();
        assert_eq!(s.statements[0].name, "main");
    }

    /// A snapshot that carries `statements` is read from `statements`, and the
    /// legacy fold does not get to overwrite it. Without this the compat path
    /// could quietly flatten every M2 endpoint to one statement.
    #[test]
    fn a_snapshot_carrying_statements_keeps_all_of_them() {
        let new = r#"{
            "statements": [
                {"name":"rows","sql":"SELECT 1 AS a","columns":[{"name":"a","ty":"integer","required":true}]},
                {"name":"total","sql":"SELECT 2 AS n","columns":[{"name":"n","ty":"bigint","required":true}]}
            ],
            "method": "GET",
            "path": "new",
            "database_id": "00000000-0000-0000-0000-000000000001",
            "params": [],
            "output": {"shape":"objects","naming":"as_is"},
            "cache": {"enabled":false,"ttl_seconds":60,"scope":"per_principal"},
            "limits": {"max_rows":10000,"timeout_seconds":30},
            "rate_limit": {"per_key_qps":0,"burst":0,"per_endpoint_qps":0},
            "delegated": false,
            "param_gate": 1
        }"#;
        let s: EndpointSnapshot = serde_json::from_str(new).unwrap();
        let names: Vec<&str> = s.statements.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["rows", "total"]);
        assert_eq!(s.statements[1].columns[0].ty, "bigint");
    }

    /// Round-trips through its own serialization: what publish writes is what
    /// serving reads back, with no legacy fields left behind.
    #[test]
    fn the_statement_list_round_trips() {
        let s: EndpointSnapshot =
            serde_json::from_str(&a_snapshot_as_published_before_m2()).unwrap();
        let written = serde_json::to_string(&s).unwrap();
        assert!(
            !written.contains("\"sql\":\"SELECT 1 AS a\"") || written.contains("\"statements\""),
            "a re-serialized snapshot must carry `statements`, not a top-level `sql`"
        );
        let again: EndpointSnapshot = serde_json::from_str(&written).unwrap();
        assert_eq!(s, again);
    }

    /// Neither shape present is corrupt data, and it was an error before M2
    /// too — `sql` and `columns` had no serde default. It stays a loud refusal
    /// rather than an endpoint with nothing to run.
    #[test]
    fn a_snapshot_with_neither_shape_is_refused() {
        let broken = r#"{
            "method": "GET",
            "path": "broken",
            "database_id": "00000000-0000-0000-0000-000000000001",
            "params": [],
            "output": {"shape":"objects","naming":"as_is"},
            "cache": {"enabled":false,"ttl_seconds":60,"scope":"per_principal"},
            "limits": {"max_rows":10000,"timeout_seconds":30},
            "rate_limit": {"per_key_qps":0,"burst":0,"per_endpoint_qps":0},
            "delegated": false
        }"#;
        let err = serde_json::from_str::<EndpointSnapshot>(broken).unwrap_err();
        assert!(
            err.to_string().contains("statements"),
            "the refusal must name what is missing, got {err}"
        );
    }

    /// An explicitly empty list is refused too. The serving path indexes the
    /// list; a snapshot that deserializes into zero statements would be an
    /// endpoint with nothing to run, discovered at request time.
    #[test]
    fn a_snapshot_with_an_empty_statement_list_is_refused() {
        let empty = r#"{
            "statements": [],
            "method": "GET",
            "path": "empty",
            "database_id": "00000000-0000-0000-0000-000000000001",
            "params": [],
            "output": {"shape":"objects","naming":"as_is"},
            "cache": {"enabled":false,"ttl_seconds":60,"scope":"per_principal"},
            "limits": {"max_rows":10000,"timeout_seconds":30},
            "rate_limit": {"per_key_qps":0,"burst":0,"per_endpoint_qps":0},
            "delegated": false
        }"#;
        assert!(serde_json::from_str::<EndpointSnapshot>(empty).is_err());
    }

    // --- compare_column_types ------------------------------------------------
    //
    // The pairs below are not invented: every Trino spelling in these tests was
    // read off a live Trino 483 coordinator, both with and without
    // `X-Trino-Client-Capabilities: PARAMETRIC_DATETIME`, and every StarRocks
    // spelling off a live StarRocks through `coltype_name`.

    /// The blocker itself: `DESCRIBE OUTPUT` states the precision, the client
    /// protocol suppresses it, and they are the two halves of one gate.
    #[test]
    fn a_suppressed_datetime_precision_matches_the_stated_one() {
        for (declared, returned) in [
            ("timestamp(0)", "timestamp"),
            ("timestamp(3)", "timestamp"),
            ("timestamp(6)", "timestamp"),
            ("time(0)", "time"),
            ("time(3)", "time"),
            ("timestamp(0) with time zone", "timestamp with time zone"),
            ("time(3) with time zone", "time with time zone"),
            ("array(timestamp(0))", "array(timestamp)"),
            (
                "map(varchar(1), timestamp(0))",
                "map(varchar(1), timestamp)",
            ),
            (
                "row(a timestamp(3), b varchar)",
                "row(a timestamp, b varchar)",
            ),
        ] {
            assert!(
                compare_column_types(declared, returned).agrees(),
                "{declared:?} vs {returned:?} is one column, one precision, and \
                 one client-capability header apart"
            );
            assert!(
                compare_column_types(returned, declared).agrees(),
                "the relation must not depend on which side is frozen"
            );
        }
    }

    /// The gate is still a gate. A genuine type change is refused, and so is a
    /// change between two STATED precisions — the relaxation is only for the
    /// side that said nothing.
    #[test]
    fn a_genuine_type_change_is_still_refused() {
        for (declared, returned) in [
            ("varchar", "integer"),
            ("integer", "bigint"),
            ("timestamp", "date"),
            ("timestamp", "timestamp with time zone"),
            ("timestamp(3)", "timestamp(6)"),
            ("time(0)", "time(3)"),
            ("array(timestamp(3))", "array(timestamp(6))"),
            ("decimal(10, 2)", "decimal(38, 10)"),
            ("varchar(20)", "varchar(40)"),
            ("datetime", "date"),
        ] {
            assert!(
                !compare_column_types(declared, returned).agrees(),
                "{declared:?} vs {returned:?} must still be schema drift"
            );
        }
    }

    /// StarRocks is untouched by construction, not by luck: `coltype_name` is
    /// lossy, so no StarRocks type string carries a parenthesis and the eraser
    /// is the identity on every one of them.
    #[test]
    fn every_starrocks_type_name_is_left_exactly_as_it_is() {
        for ty in [
            "tinyint", "smallint", "int", "bigint", "float", "double", "decimal", "date",
            "datetime", "time", "json", "varchar", "string",
        ] {
            assert_eq!(
                erase_datetime_precision(ty),
                ty,
                "{ty:?} is a StarRocks column type; nothing may rewrite it"
            );
            assert_eq!(compare_column_types(ty, ty), TypeAgreement::Same);
        }
    }

    /// `datetime` is one token and must never be read as `date` + `time`, and
    /// the bare `time` in `with time zone` has no precision to strip.
    #[test]
    fn the_eraser_only_fires_on_a_whole_token_followed_by_digits() {
        for ty in [
            "datetime",
            "timestamp with time zone",
            "time with time zone",
            "decimal(10, 2)",
            "varchar(20)",
            "map(varchar(1), integer)",
            // A `(` that is not a precision: an argument list, an empty group,
            // and a non-numeric argument. None may be stripped.
            "time(a)",
            "timestamp()",
            "timestamp(3, 4)",
        ] {
            assert_eq!(
                erase_datetime_precision(ty),
                ty,
                "{ty:?} carries no suppressible datetime precision"
            );
        }
    }

    /// The agreement must also say WHICH spelling is informative, and it must
    /// say it symmetrically — a caller folding a union sees the pair in
    /// whichever order the branches happened to be described.
    ///
    /// This is the property the union fold depends on, and it is asserted here
    /// rather than only through the fold, because a fixture whose first branch
    /// already carries the precise spelling exercises "keep the first" and
    /// "keep the more specific" identically.
    #[test]
    fn the_agreement_names_the_spelling_that_states_the_precision() {
        for (precise, bare) in [
            ("timestamp(0)", "timestamp"),
            ("timestamp(6)", "timestamp"),
            ("time(3)", "time"),
            ("timestamp(0) with time zone", "timestamp with time zone"),
            ("array(timestamp(0))", "array(timestamp)"),
        ] {
            assert_eq!(
                compare_column_types(precise, bare),
                TypeAgreement::PreferLeft,
                "{precise:?} is the informative spelling of {bare:?}"
            );
            assert_eq!(
                compare_column_types(bare, precise),
                TypeAgreement::PreferRight,
                "and the answer must not depend on argument order"
            );
        }
    }

    /// A pair that does not agree never names a preferred spelling. This is the
    /// property that made the two questions one type: the helper this replaced
    /// answered `more_specific_type("varchar", "integer")` with `"varchar"`,
    /// which is meaningless, and was safe only because its single call site
    /// happened to be guarded.
    #[test]
    fn a_disagreeing_pair_never_names_a_preferred_spelling() {
        for (a, b) in [
            ("varchar", "integer"),
            ("timestamp(3)", "timestamp(6)"),
            ("timestamp", "date"),
            ("decimal(10, 2)", "decimal(38, 10)"),
        ] {
            assert_eq!(compare_column_types(a, b), TypeAgreement::No, "{a} vs {b}");
            assert!(!compare_column_types(a, b).agrees());
        }
    }

    /// Applying it twice must change nothing — the frozen side of a comparison
    /// may already be in either spelling.
    #[test]
    fn erasing_is_idempotent() {
        for ty in [
            "timestamp(6)",
            "array(timestamp(0))",
            "timestamp(0) with time zone",
            "integer",
        ] {
            let once = erase_datetime_precision(ty);
            assert_eq!(erase_datetime_precision(&once), once);
        }
    }
}
