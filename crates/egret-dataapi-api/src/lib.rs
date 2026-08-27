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
    /// Inclusive lower bound for a `Number`. Ignored for every other type, and
    /// publish refuses it there (see `check_constraints`).
    ///
    /// **A string, not an `i64` or `f64`, and deliberately.** `coerce` already
    /// refuses to put a decimal through an `f64`, because a decimal that
    /// round-trips through a float stops comparing equal to itself on the
    /// engine. A bound stored as a float would reintroduce that on the
    /// comparison side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
    /// Inclusive upper bound for a `Number`. See [`ParamSpec::minimum`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<String>,
    /// Minimum length for a `String`, in **Unicode scalars, not bytes**. A
    /// `max_length: 10` that fails a Vietnamese name at seven letters is a bug
    /// the author cannot see from the declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    /// Maximum length for a `String`. See [`ParamSpec::min_length`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// A regex the **whole** value must match, for a `String`.
    ///
    /// Anchored implicitly: `is_match` is a search, so an unanchored `"abc"`
    /// would accept `xxabcxx`, which is not what an author writing a validation
    /// rule means. An author who writes their own `^…$` gets the same answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Inclusive earliest value for a `Date` or `Timestamp`.
    ///
    /// **Stored exactly as the author wrote it**, in any spelling the declared
    /// type parses — an unpadded `"2026-1-1"` is the same instant as
    /// `"2026-01-01"` and is enforced identically, because publish and `coerce`
    /// both normalise the bound before comparing it. This field is therefore
    /// NOT guaranteed to hold the spelling `coerce` produces.
    ///
    /// The spelling still reaches one reader unchanged: the generated OpenAPI
    /// document quotes this string RAW in the parameter's description, so
    /// `"2026-1-1"` is what a caller is shown — the right instant in a shape
    /// the endpoint never answers with. Prefer the canonical `%Y-%m-%d` (or
    /// `%Y-%m-%d %H:%M:%S%.f`) spelling so the published contract reads the way
    /// the endpoint replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    /// Inclusive latest value for a `Date` or `Timestamp`. See
    /// [`ParamSpec::not_before`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,
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
/// Unknown keys are refused for the same reason as [`ParamSpec`] and its four
/// siblings: this is a type an AUTHOR fills in, so a typo in the request body
/// must be a 400, not a silently-ignored field. It is NOT
/// `#[serde(deny_unknown_fields)]` any more — see the hand-written
/// [`Deserialize`] impl below for what replaced it and why.
#[derive(Debug, Clone, Serialize, PartialEq, utoipa::ToSchema)]
pub struct StatementSpec {
    pub name: String,
    /// Where this statement's SQL comes from. Flattened onto the object, so the
    /// wire form is `{"name": …, "sql": …}` or `{"name": …, "script": …}` —
    /// the first of those is byte-for-byte what every stored row already is.
    #[serde(flatten)]
    pub source: StatementSource,
}

/// Where one statement's SQL comes from: the author typed it, or a script
/// builds it (package S).
///
/// **Two cases and no third.** A statement carrying BOTH a literal and a script
/// has two definitions and no rule for which one runs; a statement carrying
/// NEITHER has nothing to execute and would discover that at request time, long
/// after the author left. Both are refused by the type rather than by a check
/// someone downstream has to remember — this codebase's repeated lesson is that
/// unrepresentable beats unforgettable.
///
/// The wire form is the ENCLOSING object's key, not a nested one:
/// `{"name": "rows", "sql": "…"}` or `{"name": "rows", "script": "…"}`. That is
/// not a style choice. [`StatementSpec`] is the persisted `jsonb` of
/// `dataapi.endpoints.statements` (migration `0129`) and every row already in
/// that column is `{"name": …, "sql": …}`, so the text case has to read
/// unchanged or every list, detail and serve breaks on READ — where no author
/// is present to see it. A tagged shape (`{"kind": "sql", …}`) would demand a
/// key no stored row carries.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StatementSource {
    /// SQL the author wrote, as a template the existing render pipeline runs.
    Sql(String),
    /// A Starlark script whose `build_sql(params)` returns that template.
    ///
    /// Nothing in this crate runs it — package S Task 2 is the evaluator and
    /// Task 3 is what wires generation into publish. Until then this case
    /// exists but no path produces SQL from it.
    Script(String),
}

impl StatementSource {
    /// The SQL text, when the author wrote it; `None` for a script.
    ///
    /// Deliberately NOT "the SQL, generating it if necessary": generation needs
    /// the parameter values and a Starlark evaluator, neither of which a leaf
    /// contract crate has. A caller that wants the text of a script statement
    /// has to go through the generator, which is exactly the seam that keeps
    /// generated SQL on the same path as written SQL.
    pub fn sql(&self) -> Option<&str> {
        match self {
            Self::Sql(sql) => Some(sql.as_str()),
            Self::Script(_) => None,
        }
    }

    /// The script, when one builds this statement's SQL; `None` for text.
    pub fn script(&self) -> Option<&str> {
        match self {
            Self::Sql(_) => None,
            Self::Script(script) => Some(script.as_str()),
        }
    }
}

impl StatementSpec {
    /// A statement whose SQL the author wrote.
    pub fn from_sql(name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: StatementSource::Sql(sql.into()),
        }
    }

    /// A statement whose SQL a script builds.
    pub fn from_script(name: impl Into<String>, script: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: StatementSource::Script(script.into()),
        }
    }
}

/// The three keys a statement object may carry, and the only three.
const STATEMENT_SPEC_FIELDS: &[&str] = &["name", "sql", "script"];

/// Hand-written, and the reason is measured rather than stylistic.
///
/// The derive for this shape is `#[serde(flatten)]` over [`StatementSource`]
/// plus `#[serde(deny_unknown_fields)]`, and serde documents those two as not
/// composing. On the pinned `serde 1.0.229` they were measured to compose — a
/// struct with both DID refuse `{"name":"rows","sql":"…","timeout_ms":5000}`,
/// and the same struct without `deny_unknown_fields` accepted it silently. But
/// the measurement also showed WHERE each property came from, and that is what
/// decided against the derive:
///
/// * "Unknown key refused" came from `deny_unknown_fields`, an attribute serde
///   documents as unsupported in this position. A future serde is free to make
///   it inert again, and it would do so silently — which is precisely the
///   failure `deny_unknown_fields` is here to prevent (a documented-but-wrong
///   `rate_limit` key once returned 201 with the real field defaulted to `0`).
/// * "Both refused" was not a one-of check at all: it was `deny_unknown_fields`
///   firing on whichever of `sql`/`script` came second. So the two properties
///   were not independent — losing the attribute would have lost BOTH at once,
///   and `{"sql": …, "script": …}` would have deserialized as `sql`, silently
///   discarding the script the author wrote.
/// * The messages were wrong for an author. "Both" reported ``unknown field
///   `script` `` — a key that is not unknown — and "neither" reported "no
///   variant of enum StatementSource found in flattened data", which names an
///   internal type. These are 400 bodies an author reads.
///
/// So this impl states all three rules itself: exactly one of `sql`/`script`,
/// no unknown key, no repeated key. `Serialize` stays derived, where `flatten`
/// has no such caveat and produces exactly the stored shape.
impl<'de> Deserialize<'de> for StatementSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error as DeError, MapAccess, Visitor};

        struct StatementSpecVisitor;

        impl<'de> Visitor<'de> for StatementSpecVisitor {
            type Value = StatementSpec;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a statement object with a `name` and exactly one of `sql` or `script`")
            }

            fn visit_map<A>(self, mut map: A) -> Result<StatementSpec, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut name: Option<String> = None;
                let mut sql: Option<String> = None;
                let mut script: Option<String> = None;
                // Every key is read, including the ones after the reader already
                // has what it needs: stopping early would accept a trailing typo
                // and JSON permits a repeated key, which is a malformed object
                // rather than a last-wins assignment.
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => {
                            if name.is_some() {
                                return Err(DeError::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        "sql" => {
                            if sql.is_some() {
                                return Err(DeError::duplicate_field("sql"));
                            }
                            sql = Some(map.next_value()?);
                        }
                        "script" => {
                            if script.is_some() {
                                return Err(DeError::duplicate_field("script"));
                            }
                            script = Some(map.next_value()?);
                        }
                        unknown => {
                            return Err(DeError::unknown_field(unknown, STATEMENT_SPEC_FIELDS));
                        }
                    }
                }
                let name = name.ok_or_else(|| DeError::missing_field("name"))?;
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
                Ok(StatementSpec { name, source })
            }
        }

        deserializer.deserialize_struct(
            "StatementSpec",
            STATEMENT_SPEC_FIELDS,
            StatementSpecVisitor,
        )
    }
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
    /// A name longer than [`MAX_STATEMENT_NAME_LEN`].
    ///
    /// The identifier rule bounds the ALPHABET a name may use and said nothing
    /// about its length, so a 4 KB name passed every rule above and then became
    /// a JSON object key in every response body, an OpenAPI property name in the
    /// generated document, and a `data.<name>` path in every consumer's code.
    /// None of those has a length limit either, which is what makes this the
    /// bound's only home.
    #[error(
        "statement name is {len} characters and the limit is {limit}. A name becomes a JSON \
         object key in every response and a property name in the generated OpenAPI document, \
         so it is part of the endpoint's public contract rather than a label"
    )]
    NameTooLong { len: usize, limit: usize },
    /// Two statements in the same endpoint declared the same name.
    #[error(
        "statement name `{0}` is used by more than one statement in this endpoint; the response \
         envelope keys results by name, so two statements cannot share one"
    )]
    DuplicateName(String),
    /// `output.envelope` is not one of [`ENVELOPES`].
    ///
    /// In [`StatementError`] rather than an error type of its own because it is
    /// refused by the same three call sites at the same moment — the create
    /// route, the patch route and publish — and every one of them converts this
    /// type to a 400 already. A second type would be a second thing each of them
    /// has to remember to call.
    #[error(
        "output.envelope {0:?} is not one of `wrapped` or `bare`. It decides whether the \
         response carries the {{status, message, data, meta}} wrapper, so a value that is \
         merely close — a capital letter, a trailing space — would be stored, echoed back, \
         and then served as the OTHER shape"
    )]
    UnknownEnvelope(String),
    /// The list had no statements at all.
    #[error("an endpoint must declare at least one statement")]
    Empty,
    /// More statements than [`MAX_STATEMENTS`].
    ///
    /// The message names the two costs the count multiplies rather than only
    /// the number, because neither is visible in the editor: the author sees a
    /// list of statements and a `timeout_seconds` field, and nothing on that
    /// screen says the two are multiplied.
    #[error(
        "this endpoint declares {count} statements and the limit is {limit}. Statements run \
         one after another, so an endpoint's worst-case response time is its \
         `limits.timeout_seconds` MULTIPLIED by how many it has, and publish renders and \
         describes every branch of every one of them. Fold statements a caller always reads \
         together into one query with a join or a `UNION ALL`, or split this into two \
         endpoints"
    )]
    TooManyStatements { count: usize, limit: usize },
}

/// How many SQL statements one endpoint may declare.
///
/// # Why a count and not a time budget
///
/// The cost this bounds is wall clock, and wall clock is what a budget would
/// bound directly — but a budget can only fail in the middle of the loop, with
/// some statements already executed, and **the response envelope cannot express
/// that**. `data` is an object keyed by statement name and `meta.statements`
/// reports one `row_count`/`columns` entry per statement; a body carrying three
/// of four keys is a client silently reading a hole, which is the exact failure
/// `serving::router`'s execution loop already refuses for a mid-loop drift
/// ("their rows are discarded rather than served beside a partial failure").
/// The alternative — run the budget down, then discard everything — spends the
/// resource in full before refusing, so it bounds nothing an abusive caller
/// cares about while turning a slow-but-correct endpoint into an intermittent
/// one. It would also bound only the serving half: the publish cost below is
/// paid before any statement executes, and no serving budget reaches it.
///
/// A count is checkable at publish, refuses once, loudly, to the author who can
/// fix it, and bounds both halves with one number. `limits.timeout_seconds`
/// stays the knob for how long ONE statement may take; this is the bound on the
/// multiplier.
///
/// # Where the number comes from
///
/// Measured in release, 2026-08-25, at N = 1…1024.
///
/// **Publish, before the first describe** — everything `validate_endpoint` and
/// the branch enumeration do, per statement, for the most expensive statement
/// the other two limits still admit (32 boolean parameters, 530 branches, which
/// is `egret_dataapi`'s `MAX_DECLARED_PARAMS` worst case):
///
/// | N | scans + render | enumerate | total |
/// |---|---|---|---|
/// | 1 | 10.1 ms | 111.3 ms | **121 ms** |
/// | 8 | 80.3 ms | 913.9 ms | 0.99 s |
/// | **16** | 166.0 ms | 1.8 s | **1.97 s** |
/// | 64 | 646.6 ms | 7.3 s | 7.9 s |
/// | 256 | 2.6 s | 29.0 s | **31.6 s** |
///
/// Linear in N, and every one of those is refused afterwards — the branch
/// budget answers `16 × 530 = 8480 > 64` — so the whole table is time paid for
/// a `400`, on a synchronous CPU path that OCCUPIES a tokio worker rather than
/// releasing it. That is the same relationship `MAX_DECLARED_PARAMS` has to
/// `MAX_DESCRIBED_BRANCHES`: this bound is a SCAN, it runs first, and it makes
/// the table above unreachable rather than merely refused.
///
/// **Serving** — one call to an endpoint of N statements, against a stub engine
/// costing 40 ms per execution: 99.0 ms at N = 2, 343.2 ms at 8, 682.6 ms at 16,
/// 2.7 s at 64 — **42 ms per statement at every N**, so egret's own share is
/// ~2 ms and the rest is the engine. Sequential and linear, exactly as
/// `serving::router` executes it. The endpoint's worst case is therefore
/// `N × min(timeout_seconds, TIMEOUT_CEILING_SECS)`, and nothing bounded N.
///
/// # So `16`
///
/// * **8× the canonical several-statement endpoint** — a `rows` query and the
///   `total` that paginates it (spec §4.4) — which is the multiplier
///   `MAX_DECLARED_PARAMS` takes over spec §4.3's four parameters.
/// * **It is where this bound and the branch budget MEET.** Every statement
///   costs at least one describe, so `MAX_DESCRIBED_BRANCHES = 64` already
///   capped N at 64 — but only for statements with no branches at all, and only
///   *after* all N had been rendered. 64 / 16 = 4 branches per statement: below
///   that width the statement count is what refuses, above it the branch budget
///   is. At 8 this bound would fire for lists of cheap statements the branch
///   budget was happy to describe; at 32 the branch budget binds first for
///   anything with two or more branches, so the statement count would almost
///   never speak.
/// * It takes the worst wall clock one call can buy from `64 × 300 s` (5 h
///   20 min) to `16 × 300 s`, and the publish CPU from unbounded — nothing
///   caps the list on the create or PATCH routes — to a measured 1.97 s.
///
/// # What it is not derived from
///
/// **Not from a target response time.** The deployed ingress gives a Data API
/// call 60 seconds: egret's `Ingress` sets no `proxy-read-timeout` annotation,
/// so ingress-nginx's default applies, and the running controller's generated
/// config reads `proxy_read_timeout 60s` (read out of the live pod, not assumed).
/// The default `limits.timeout_seconds` is 30, so **two** statements already
/// reach that cut-off in the worst case. No value of N makes the worst case fit
/// inside 60 s; keeping a real endpoint inside it is `timeout_seconds`'s job,
/// per statement. This bound exists because N was a multiplier on two costs that
/// were each already bounded, and it was itself bounded by nothing.
pub const MAX_STATEMENTS: usize = 16;

/// How long a statement name may be.
///
/// Sixty-four characters, which is Postgres's own identifier limit (63) rounded
/// to the nearest power of two — a number an author will never reach writing
/// `rows`, `total` or `by_region_last_quarter`, and small enough that the JSON
/// key it becomes stays readable in a response body. The alphabet rule above
/// bounds what a name may contain and bounded nothing about its size.
pub const MAX_STATEMENT_NAME_LEN: usize = 64;

/// `^[a-z][a-z0-9_]*$`, and no longer than [`MAX_STATEMENT_NAME_LEN`] — see
/// [`StatementSpec`]'s doc comment for why this exact shape and not, say,
/// camelCase or a leading underscore for "private".
///
/// The length is checked FIRST, and by `chars().count()` rather than `len()`:
/// the alphabet rule below is ASCII-only, so the two agree for every name it
/// accepts, and a rejected name is measured in characters the author can count
/// on screen rather than in UTF-8 bytes.
pub fn validate_statement_name(name: &str) -> Result<(), StatementError> {
    let len = name.chars().count();
    if len > MAX_STATEMENT_NAME_LEN {
        return Err(StatementError::NameTooLong {
            len,
            limit: MAX_STATEMENT_NAME_LEN,
        });
    }
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
    // The other end of the same question, and it is asked here — before the
    // per-name loop, and on every path that writes or publishes a list — for the
    // reason [`MAX_STATEMENTS`] records: it is the bound that makes the cost of
    // everything downstream a number rather than a function of the request, so
    // it has to run before any of it. `egret_dataapi`'s `validate_endpoint`
    // calls this function first, ahead of every scan and every render, which is
    // what makes it a publish-time refusal rather than a note.
    if statements.len() > MAX_STATEMENTS {
        return Err(StatementError::TooManyStatements {
            count: statements.len(),
            limit: MAX_STATEMENTS,
        });
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

/// An author's response transform: a Starlark script plus the output schema it
/// promises.
///
/// # Why `returns` is declared rather than inferred
///
/// Every other column egret freezes comes from `describe_as`, which works
/// because a SQL statement's shape is a function of its text and the branch a
/// parameter takes. A script's shape can depend on the DATA — `if len(rows) > 0`
/// — which publish cannot see. Inferring from a probe run would produce a
/// contract that is right for the probe data and wrong for a Tuesday. So the
/// author declares, publish checks the probes against the declaration, and
/// serving fails closed on every response that disagrees. See spec §3.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformSpec {
    /// The script source. Must define `transform(params, statements)`.
    pub script: String,
    /// The output schema this endpoint promises. `ColumnSpec` is reused rather
    /// than duplicated: its `required` flag already means "present in every
    /// permutation described at publish", which is exactly what it means here.
    pub returns: Vec<ColumnSpec>,
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
    /// A `String` like `shape` and `naming`, but — unlike them — **checked
    /// before it is stored**, by [`validate_envelope`]. Serving still falls back
    /// to the documented default and logs rather than refusing (see
    /// `serving::router::envelope_of`), because a frozen snapshot must keep
    /// answering; the check exists so no new row can reach that fallback. See
    /// [`validate_envelope`] for why this field and not its two neighbours.
    #[serde(default = "default_envelope")]
    pub envelope: String, // "wrapped" | "bare"
    /// Absent means no transform, which is every snapshot frozen before C2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformSpec>,
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

/// The two values [`OutputSpec::envelope`] may take.
pub const ENVELOPES: [&str; 2] = ["wrapped", "bare"];

/// `output.envelope` names an envelope that exists.
///
/// # Why this field and not `shape` or `naming`
///
/// All three are free `String`s, and only this one decides the **shape of the
/// wire body**: `"Bare"` or `"bare "` passes create, PATCH, publish and the
/// migration, and then serves `{status, message, data, meta}` to a client whose
/// author asked for the value on its own and read `envelope: "Bare"` back from
/// the API confirming it. The only trace is a `tracing::warn!` on the serving
/// side, in a log the author does not read. `shape` and `naming` fall back the
/// same way, but this field is also NEW (M2 Task 8) — there is no corpus of
/// rows carrying an unrecognised value, so a rule here refuses nothing that was
/// ever accepted. That is not true of the other two, and retro-fitting a rule to
/// them is a separate decision about existing drafts.
///
/// Not a serde-level rule and not an `enum`: `OutputSpec` is deserialized from a
/// FROZEN snapshot as well as from a request body, and an enum would make an old
/// row with an unrecognised value unreadable — a published endpoint that stops
/// serving entirely rather than serving its documented default. The fallback is
/// what a reader needs; a validator is what a WRITER needs. They are different
/// paths and this is the writer's.
pub fn validate_envelope(output: &OutputSpec) -> Result<(), StatementError> {
    if ENVELOPES.contains(&output.envelope.as_str()) {
        Ok(())
    } else {
        Err(StatementError::UnknownEnvelope(output.envelope.clone()))
    }
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
            transform: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_spec_without_constraints_round_trips_and_means_unconstrained() {
        // A snapshot written before C1 has none of these keys. Reading it back must
        // produce `None` everywhere, which is what "absent means today's behaviour"
        // has to mean in practice — `deny_unknown_fields` makes the opposite failure
        // loud, but a WRONG DEFAULT would be silent.
        let json = r#"{"name":"region","type":"string","location":"query"}"#;
        let spec: ParamSpec = serde_json::from_str(json).expect("pre-C1 spec must still read");
        assert_eq!(spec.minimum, None);
        assert_eq!(spec.maximum, None);
        assert_eq!(spec.min_length, None);
        assert_eq!(spec.max_length, None);
        assert_eq!(spec.pattern, None);
        assert_eq!(spec.not_before, None);
        assert_eq!(spec.not_after, None);

        // And it must not GROW keys on the way out: a snapshot rewritten by a newer
        // binary should stay byte-comparable for an unconstrained parameter.
        let back = serde_json::to_string(&spec).expect("serialize");
        assert!(
            !back.contains("minimum"),
            "unconstrained spec grew keys: {back}"
        );
        assert!(
            !back.contains("pattern"),
            "unconstrained spec grew keys: {back}"
        );
    }

    #[test]
    fn param_spec_round_trips_every_constraint() {
        let json = r#"{"name":"n","type":"number","location":"query",
                       "minimum":"1","maximum":"100","min_length":2,"max_length":10,
                       "pattern":"^[a-z]+$","not_before":"2020-01-01","not_after":"2030-12-31"}"#;
        let spec: ParamSpec = serde_json::from_str(json).expect("full spec must read");
        assert_eq!(spec.minimum.as_deref(), Some("1"));
        assert_eq!(spec.maximum.as_deref(), Some("100"));
        assert_eq!(spec.min_length, Some(2));
        assert_eq!(spec.max_length, Some(10));
        assert_eq!(spec.pattern.as_deref(), Some("^[a-z]+$"));
        assert_eq!(spec.not_before.as_deref(), Some("2020-01-01"));
        assert_eq!(spec.not_after.as_deref(), Some("2030-12-31"));

        let round: ParamSpec =
            serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(round, spec, "round trip must be lossless");
    }

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

    /// The LENGTH bound, from both sides and by the exact variant.
    ///
    /// Every character in the long name below is one the alphabet rule accepts,
    /// so nothing else in `validate_statement_name` can answer for it — which is
    /// the point: the identifier rule bounded the alphabet and bounded nothing
    /// about the size of the JSON key the name becomes.
    #[test]
    fn a_statement_name_has_a_length_bound() {
        let at_limit = "r".repeat(MAX_STATEMENT_NAME_LEN);
        assert_eq!(
            validate_statement_name(&at_limit),
            Ok(()),
            "the limit itself must be accepted, or the bound is off by one"
        );
        let past = "r".repeat(MAX_STATEMENT_NAME_LEN + 1);
        assert_eq!(
            validate_statement_name(&past),
            Err(StatementError::NameTooLong {
                len: MAX_STATEMENT_NAME_LEN + 1,
                limit: MAX_STATEMENT_NAME_LEN,
            })
        );
        // And through the list validator, which is what the routes and publish
        // actually call.
        assert_eq!(
            validate_statements(&[StatementSpec::from_sql(past.clone(), "SELECT 1")]),
            Err(StatementError::NameTooLong {
                len: MAX_STATEMENT_NAME_LEN + 1,
                limit: MAX_STATEMENT_NAME_LEN,
            })
        );
    }

    /// `output.envelope` decides the shape of the wire body, so the values that
    /// are ALMOST right are the ones worth pinning: each of them would be
    /// stored, echoed back as accepted, and then served as the other shape.
    #[test]
    fn only_the_two_documented_envelopes_are_accepted() {
        // The two values SPELLED OUT, not iterated from `ENVELOPES`. A loop over
        // the constant moves with it: narrowing the constant to
        // `["wrapped", "wrapped"]` — which stops `bare` being publishable at all
        // — left this test green, and only the two DB-backed tests caught it.
        assert_eq!(ENVELOPES, ["wrapped", "bare"]);
        for good in ["wrapped", "bare"] {
            let output = OutputSpec {
                envelope: good.into(),
                ..OutputSpec::default()
            };
            assert_eq!(validate_envelope(&output), Ok(()), "refused {good:?}");
        }
        for bad in ["Bare", "bare ", " bare", "", "WRAPPED", "none"] {
            let output = OutputSpec {
                envelope: bad.to_string(),
                ..OutputSpec::default()
            };
            assert_eq!(
                validate_envelope(&output),
                Err(StatementError::UnknownEnvelope(bad.to_string())),
                "accepted {bad:?}"
            );
        }
        // The DEFAULT — what an absent `output` deserializes to — must pass the
        // rule it is the default for.
        assert_eq!(validate_envelope(&OutputSpec::default()), Ok(()));
    }

    #[test]
    fn two_statements_may_not_share_a_name() {
        let statements = vec![
            StatementSpec::from_sql("rows", "SELECT 1"),
            StatementSpec::from_sql("rows", "SELECT 2"),
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

    /// The upper bound, from the side that must be refused.
    ///
    /// Reads WHICH refusal fired, not merely that one did: `validate_statements`
    /// has four now, and three of them are also reachable from a list this long.
    #[test]
    fn a_statement_list_past_the_limit_is_refused() {
        let statements: Vec<StatementSpec> = (0..=MAX_STATEMENTS)
            .map(|i| StatementSpec::from_sql(format!("s{i}"), "SELECT 1"))
            .collect();
        assert_eq!(
            validate_statements(&statements),
            Err(StatementError::TooManyStatements {
                count: MAX_STATEMENTS + 1,
                limit: MAX_STATEMENTS,
            })
        );
    }

    /// The other side, and deliberately its own test rather than a second
    /// assertion in the one above: a bound that refuses the work it exists to
    /// permit is not a bound, it is an outage, and the two failures are
    /// different mistakes.
    ///
    /// Two statements is the canonical several-statement endpoint — a `rows`
    /// query and the `total` that paginates it (spec §4.4) — and the list AT the
    /// limit is the widest thing an author may write.
    #[test]
    fn the_canonical_two_statement_endpoint_and_a_list_at_the_limit_are_accepted() {
        let build = |n: usize| -> Vec<StatementSpec> {
            (0..n)
                .map(|i| StatementSpec::from_sql(format!("s{i}"), "SELECT 1"))
                .collect()
        };
        assert_eq!(validate_statements(&build(2)), Ok(()));
        assert_eq!(validate_statements(&build(MAX_STATEMENTS)), Ok(()));
    }

    #[test]
    fn a_single_valid_statement_is_accepted() {
        let statements = vec![StatementSpec::from_sql("rows", "SELECT 1")];
        assert!(validate_statements(&statements).is_ok());
    }

    #[test]
    fn an_invalid_name_inside_the_list_is_refused_by_the_list_validator() {
        let statements = vec![StatementSpec::from_sql("Rows", "SELECT 1")];
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

    // --- OutputSpec.transform (C2 Task 2) ---

    #[test]
    fn an_output_spec_without_a_transform_round_trips_and_means_no_transform() {
        // A snapshot written before C2 has no `transform` key. Reading it back must
        // give `None`, and writing it back must not GROW the key — a snapshot that
        // gains fields on re-read is how two binaries disagree about one row.
        let json = r#"{"shape":"objects","naming":"as_is","envelope":"wrapped"}"#;
        let spec: OutputSpec = serde_json::from_str(json).expect("pre-C2 spec must still read");
        assert!(spec.transform.is_none());
        let back = serde_json::to_string(&spec).expect("serialize");
        assert!(!back.contains("transform"), "spec grew a key: {back}");
    }

    #[test]
    fn a_transform_round_trips_with_its_declared_returns() {
        let json = r#"{"shape":"objects","naming":"as_is","envelope":"wrapped",
                       "transform":{"script":"def transform(params, statements):\n  return {}",
                                    "returns":[{"name":"total","ty":"bigint","required":true}]}}"#;
        let spec: OutputSpec = serde_json::from_str(json).expect("must read");
        let t = spec.transform.as_ref().expect("transform present");
        assert!(t.script.contains("def transform"));
        assert_eq!(t.returns.len(), 1);
        assert_eq!(t.returns[0].name, "total");
        assert!(t.returns[0].required);

        let round: OutputSpec =
            serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(round, spec, "round trip must be lossless");
    }

    #[test]
    fn an_unknown_key_inside_a_transform_is_refused() {
        // `deny_unknown_fields` is what turned a documented-but-wrong rate_limit key
        // into an error instead of a silent default. The same protection has to
        // reach INSIDE the new struct, not just around it.
        let json = r#"{"shape":"objects","naming":"as_is","envelope":"wrapped",
                       "transform":{"script":"x","returns":[],"timeout_ms":5000}}"#;
        assert!(
            serde_json::from_str::<OutputSpec>(json).is_err(),
            "an unknown key inside `transform` must be refused, not defaulted"
        );
    }

    // ---------------------------------------------------------------------
    // Package S Task 1: a statement's SQL is TEXT or a SCRIPT.
    //
    // `StatementSpec` IS the persisted `jsonb` of `dataapi.endpoints.statements`
    // (migration 0129), so the first test below is the load-bearing one: every
    // stored row is `{"name":"main","sql":"…"}` and a shape that stops reading
    // it breaks every list, detail and serve — on READ, where no author is
    // present to notice.
    // ---------------------------------------------------------------------

    #[test]
    fn a_statement_with_sql_reads_as_text_and_round_trips() {
        let json = r#"{"name":"rows","sql":"SELECT 1 AS a"}"#;
        let s: StatementSpec = serde_json::from_str(json).expect("pre-S spec must still read");
        assert_eq!(s.name, "rows");
        assert_eq!(
            s.source,
            StatementSource::Sql("SELECT 1 AS a".to_string()),
            "a stored row with `sql` must observe the TEXT case"
        );
        // The wire form must be byte-identical to what is already in the column,
        // or a round trip through the API rewrites every stored row.
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            json,
            "the text form must serialize back to exactly the stored shape"
        );
        let round: StatementSpec =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(round, s, "round trip must be lossless");
    }

    #[test]
    fn a_statement_with_a_script_reads_as_a_script() {
        let json = r#"{"name":"rows","script":"def build_sql(params):\n  return \"SELECT 1\""}"#;
        let s: StatementSpec = serde_json::from_str(json).expect("script form must read");
        assert_eq!(s.name, "rows");
        assert_eq!(
            s.source,
            StatementSource::Script("def build_sql(params):\n  return \"SELECT 1\"".to_string()),
            "must observe the SCRIPT case, not the text one"
        );
        assert_eq!(
            s.source.sql(),
            None,
            "a script statement has no author-written SQL to hand a scanner"
        );
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            json,
            "the script form must round-trip through the same key it read"
        );
    }

    #[test]
    fn a_statement_carrying_both_is_refused() {
        // "Both" must be unrepresentable, not merely invalid-and-checked-later.
        let json = r#"{"name":"rows","sql":"SELECT 1","script":"def build_sql(p): return \"\""}"#;
        let err = serde_json::from_str::<StatementSpec>(json)
            .expect_err("a statement with both a literal and a script must not deserialize");
        assert_eq!(
            err.to_string(),
            "a statement carries either `sql` or `script`, never both; \
             `sql` is SQL the author wrote and `script` is a Starlark script that \
             builds it, so a statement with both has two definitions and no rule \
             for which one runs at line 1 column 73",
            "the message must say WHICH rule was broken, not name a field"
        );
    }

    /// `name` is required, and that rule moved from the derive into hand-written
    /// code — the three rules beside it each got a test and this one did not.
    ///
    /// Without the `ok_or_else` a nameless statement would deserialize with an
    /// EMPTY name, which is not a name `validate_statements` would ever accept
    /// from an author but which would have arrived through deserialization
    /// rather than through the route that checks.
    #[test]
    fn a_statement_without_a_name_is_refused() {
        let err = serde_json::from_str::<StatementSpec>(r#"{"sql":"SELECT 1"}"#)
            .expect_err("`name` is required");
        assert_eq!(err.to_string(), "missing field `name` at line 1 column 18");
    }

    /// The "both" refusal must not depend on which key arrives first.
    ///
    /// It did under the derive: `deny_unknown_fields` fired on whichever of the
    /// two came SECOND, so `{sql, script}` blamed `script` and `{script, sql}`
    /// blamed `sql`. The hand-written reader collects every key before deciding,
    /// so one rule gives one message either way.
    #[test]
    fn the_both_refusal_does_not_depend_on_key_order() {
        let forward = serde_json::from_str::<StatementSpec>(
            r#"{"name":"rows","sql":"SELECT 1","script":"S"}"#,
        )
        .expect_err("both, sql first");
        let reverse = serde_json::from_str::<StatementSpec>(
            r#"{"name":"rows","script":"S","sql":"SELECT 1"}"#,
        )
        .expect_err("both, script first");
        assert!(
            forward
                .to_string()
                .starts_with("a statement carries either"),
            "sql-first: {forward}"
        );
        assert!(
            reverse
                .to_string()
                .starts_with("a statement carries either"),
            "script-first: {reverse}"
        );
    }

    #[test]
    fn a_statement_carrying_neither_is_refused() {
        let json = r#"{"name":"rows"}"#;
        let err = serde_json::from_str::<StatementSpec>(json).expect_err("neither is refused");
        assert_eq!(
            err.to_string(),
            "a statement must carry either `sql`, the SQL the author wrote, or \
             `script`, a Starlark script that builds it; this one carries neither \
             at line 1 column 15"
        );
    }

    #[test]
    fn an_unknown_key_on_a_statement_is_still_refused() {
        // The protection `deny_unknown_fields` gives today must survive whatever
        // shape this type takes. A documented-but-wrong key once returned 201
        // with the real field silently defaulted to 0 — that is what this
        // prevents, and `StatementSpec` is now hand-deserialized, so it is this
        // test rather than a derive attribute that holds the property.
        let json = r#"{"name":"rows","sql":"SELECT 1","timeout_ms":5000}"#;
        let err = serde_json::from_str::<StatementSpec>(json)
            .expect_err("an unknown key must be refused, not ignored");
        assert_eq!(
            err.to_string(),
            "unknown field `timeout_ms`, expected one of `name`, `sql`, `script` \
             at line 1 column 44"
        );
    }

    /// The unknown key must be refused wherever it appears in the object, not
    /// only after the fields the reader happens to want. A reader that decides
    /// once it has what it needs would accept a leading typo.
    #[test]
    fn an_unknown_key_is_refused_before_the_known_ones_too() {
        for json in [
            r#"{"timeout_ms":5000,"name":"rows","sql":"SELECT 1"}"#,
            r#"{"name":"rows","timeout_ms":5000,"sql":"SELECT 1"}"#,
        ] {
            let err = serde_json::from_str::<StatementSpec>(json)
                .expect_err("an unknown key must be refused wherever it sits");
            assert!(
                err.to_string().starts_with("unknown field `timeout_ms`"),
                "wrong error for {json}: {err}"
            );
        }
    }

    /// A repeated key is a malformed object, not a last-wins assignment. JSON
    /// permits duplicates and `serde_json` hands them over one at a time, so
    /// nothing refuses this unless the reader does.
    #[test]
    fn a_repeated_key_on_a_statement_is_refused() {
        let err = serde_json::from_str::<StatementSpec>(
            r#"{"name":"rows","sql":"SELECT 1","sql":"DROP TABLE t"}"#,
        )
        .expect_err("a duplicate key must not silently win");
        assert!(
            err.to_string().starts_with("duplicate field `sql`"),
            "unexpected: {err}"
        );
    }

    /// A list of statements is how the column is actually read back — one
    /// `Vec<StatementSpec>` out of one `jsonb` value — so the pre-S row is
    /// pinned in the shape the three `endpoints.rs` read sites use, not only
    /// as a single object.
    #[test]
    fn the_stored_statements_column_still_reads_as_a_list() {
        let stored = r#"[{"name":"main","sql":"SELECT 1 AS a"},{"name":"total","sql":"SELECT 1"}]"#;
        let list: Vec<StatementSpec> =
            serde_json::from_str(stored).expect("every existing row must still read");
        assert_eq!(
            list,
            vec![
                StatementSpec::from_sql("main", "SELECT 1 AS a"),
                StatementSpec::from_sql("total", "SELECT 1"),
            ]
        );
        assert_eq!(serde_json::to_string(&list).unwrap(), stored);
    }

    /// `validate_statements` is defined over the LIST, not over the SQL, so it
    /// must apply its three rules to a script statement exactly as it does to a
    /// written one. A rule that quietly stopped seeing half the statements
    /// would let a duplicate name through.
    #[test]
    fn the_list_rules_apply_to_a_script_statement_too() {
        assert_eq!(
            validate_statements(&[StatementSpec::from_script("Rows", "def build_sql(p): pass")]),
            Err(StatementError::InvalidName("Rows".into()))
        );
        assert_eq!(
            validate_statements(&[
                StatementSpec::from_sql("rows", "SELECT 1"),
                StatementSpec::from_script("rows", "def build_sql(p): pass"),
            ]),
            Err(StatementError::DuplicateName("rows".into()))
        );
        assert_eq!(
            validate_statements(&[
                StatementSpec::from_sql("rows", "SELECT 1"),
                StatementSpec::from_script("total", "def build_sql(p): pass"),
            ]),
            Ok(()),
            "a mixed list is well-formed: the rules are about names"
        );
    }

    /// The two cases are distinguishable by an accessor, and neither answers for
    /// the other. Task 3 dispatches on exactly this.
    #[test]
    fn each_source_answers_only_for_its_own_case() {
        let text = StatementSource::Sql("SELECT 1".into());
        let script = StatementSource::Script("def build_sql(p): pass".into());
        assert_eq!(text.sql(), Some("SELECT 1"));
        assert_eq!(text.script(), None);
        assert_eq!(script.script(), Some("def build_sql(p): pass"));
        assert_eq!(script.sql(), None);
    }
}
