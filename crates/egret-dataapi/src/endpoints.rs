//! Data API endpoint DRAFTS: the row an author edits, and the store behind it.
//!
//! This module owns the draft half of the split migration 0121 describes.
//! Publishing — freezing a draft into an immutable `dataapi.endpoint_versions`
//! snapshot — is a separate lifecycle and lives in [`crate::lifecycle`]. No
//! author-facing path here writes `state` or `live_version_id`, so no amount of
//! editing can put an endpoint live, and a published version's contents are
//! frozen against editing here. The one exception is [`EndpointStore::set_state`],
//! which is `pub(crate)` and reachable only from the lifecycle routes.
//!
//! Editing `path` and `method` on a live endpoint is allowed and moves nothing
//! yet: serving matches `live_method`/`live_path`, which only publish and
//! rollback write (KI-023, migration 0204). The draft's new URL goes live at the
//! next publish. See `lifecycle.rs`' module comment.
//!
//! **Workspace containment lives HERE, in exactly one place: the `WHERE id = $1
//! AND workspace_id = $2` that every statement below carries.** There is no
//! route-level pre-check to forget. That is the point — a store method taking a
//! bare id will happily read, mutate or delete another workspace's row, and the
//! only thing standing in the way is a human remembering to call a guard first.
//! Publish, rollback and disable are still to be written against this store, and
//! a missed call there would be silent and total.
//!
//! Enforcing it once, in the layer nothing can bypass, keeps the property that
//! made the route-level guard attractive: there is a single line to delete, and
//! deleting it turns the cross-workspace tests red. It also costs one query per
//! id route instead of two.
//!
//! Absence and denial are the same answer — [`Error::NotFound`] — so a caller in
//! another workspace cannot tell "no such endpoint" from "exists, but not yours".

use chrono::{DateTime, Utc};
use egret_core::{Error, Result, UserId};
use egret_dataapi_api::{
    CacheSpec, LimitsSpec, OutputSpec, ParamSpec, RateLimitSpec, StatementSpec, MAX_STATEMENTS,
};
use egret_metastore::Db;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

/// The projection every read in this module shares. A macro rather than a
/// `const` so it can be `concat!`-ed into `&'static str` literals — sqlx 0.9's
/// `query`/`query_as` take `&'static str`, and a `format!`-ed statement would
/// have to go through `AssertSqlSafe`.
macro_rules! endpoint_cols {
    () => {
        "id, folder_id, name, description, path, method, database_id, statements, params, output, \
         cache, limits, rate_limit, source_kind, source_ref, live_version_id, live_method, \
         live_path, state, created_by, created_at, updated_at, deleted_at"
    };
}

/// The stored row, jsonb columns still unparsed. Kept private: callers get
/// [`DataApiEndpointDto`], which has already turned those columns into the typed
/// specs and would have failed loudly if one of them were unreadable.
#[derive(Debug, sqlx::FromRow)]
struct Row {
    id: Uuid,
    folder_id: Option<Uuid>,
    name: String,
    description: String,
    path: String,
    method: String,
    database_id: Uuid,
    statements: serde_json::Value,
    params: serde_json::Value,
    output: serde_json::Value,
    cache: serde_json::Value,
    limits: serde_json::Value,
    rate_limit: serde_json::Value,
    source_kind: String,
    source_ref: Option<Uuid>,
    live_version_id: Option<Uuid>,
    live_method: Option<String>,
    live_path: Option<String>,
    state: String,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// [`Row`] plus the calling user's `bookmarked` flag, for [`EndpointStore::list_for`].
#[derive(Debug, sqlx::FromRow)]
struct RowWithBookmark {
    #[sqlx(flatten)]
    row: Row,
    bookmarked: bool,
}

/// An endpoint draft as the management UI sees it.
///
/// Named with the `DataApi` prefix for the same reason `DataApiClientDto` is:
/// utoipa keys components by Rust type name and those names are GLOBAL once
/// `openapi_doc()` merges the seams, so a bare `EndpointDto` is a collision
/// waiting for the next seam that wants the word.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiEndpointDto {
    pub id: Uuid,
    pub folder_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub path: String,
    pub method: String,
    pub database_id: Uuid,
    /// The endpoint's statements, in execution order.
    ///
    /// A LIST, not a string, and a one-statement endpoint is the same shape as
    /// a ten-statement one — there is no "simple" spelling to keep in step with
    /// the general one. Stored as `jsonb`, like every other structured column
    /// on this row.
    pub statements: Vec<StatementSpec>,
    pub params: Vec<ParamSpec>,
    pub output: OutputSpec,
    pub cache: CacheSpec,
    pub limits: LimitsSpec,
    pub rate_limit: RateLimitSpec,
    pub source_kind: String,
    pub source_ref: Option<Uuid>,
    /// The version currently served. Never written by this module — publishing
    /// is a separate lifecycle.
    pub live_version_id: Option<Uuid>,
    /// The method and path the LIVE version is served at, or `null` for an
    /// endpoint never published. Written only by publish and rollback, from the
    /// snapshot they make live; the serving router matches these, never
    /// `method`/`path`. When they differ from `method`/`path` the draft has a
    /// new URL that goes live at the next publish (KI-023).
    pub live_method: Option<String>,
    /// See [`Self::live_method`].
    pub live_path: Option<String>,
    pub state: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When this endpoint was moved to the trash, or `null` if it is live in the
    /// authoring sense — present, listed, editable.
    ///
    /// On the DTO rather than a bare `trashed: bool` because WHEN it was trashed
    /// is what a "Trash" screen sorts and ages by, and because the column itself
    /// is the timestamp — a boolean projection here would be a second
    /// representation of the same fact for the frontend to disagree with. The
    /// schema's `CHECK (deleted_at IS NULL OR state <> 'live')` (migration 0126)
    /// means a non-null value here can never coexist with `state = "live"`.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// One row of the endpoint LIST: the draft, plus the calling user's own
/// `bookmarked` flag.
///
/// A separate type rather than a field on [`DataApiEndpointDto`] because that
/// DTO is also what `GET /endpoints/{id}`, `POST /endpoints`, `PATCH
/// /endpoints/{id}`, disable/enable and the publish lifecycle's locked read
/// return — and half of those have no user to be per-user ABOUT. A
/// `bookmarked` on the create response would be a field that is always `false`
/// by construction, and one on the lifecycle's read would be a field with
/// nobody to answer for; either is a value a client could believe. The flag
/// exists where it is asked for and nowhere else.
///
/// `#[serde(flatten)]` so the wire shape stays FLAT — `{...endpoint,
/// "bookmarked": true}`, one object, exactly as if the field lived on the DTO —
/// and so a field added to [`DataApiEndpointDto`] tomorrow appears in the list
/// with nothing here to update. The same idiom `EngineDetailDto` uses; utoipa
/// renders it as an `allOf`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiEndpointListItemDto {
    #[serde(flatten)]
    pub endpoint: DataApiEndpointDto,
    /// Whether the CALLER has starred this endpoint. Per user, never global.
    pub bookmarked: bool,
}

/// Parse one stored jsonb spec.
///
/// A stored column that will not deserialize is a 500, not a silent default. The
/// specs here decide how much data an endpoint may return and how often it may
/// be called; quietly substituting `LimitsSpec::default()` for an unreadable
/// `limits` would hand back a draft that says one thing and behaves as another,
/// and the author would have no way to see it. `'{}'::jsonb` — the column
/// default — is not unreadable: every field of every spec carries a serde
/// default, which is exactly how an absent field gets its documented value.
fn spec<T: serde::de::DeserializeOwned>(field: &str, v: serde_json::Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| {
        Error::Internal(anyhow::anyhow!(
            "stored endpoint `{field}` is not readable: {e}"
        ))
    })
}

impl TryFrom<Row> for DataApiEndpointDto {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(Self {
            id: r.id,
            folder_id: r.folder_id,
            name: r.name,
            description: r.description,
            path: r.path,
            method: r.method,
            database_id: r.database_id,
            statements: spec("statements", r.statements)?,
            params: spec("params", r.params)?,
            output: spec("output", r.output)?,
            cache: spec("cache", r.cache)?,
            limits: spec("limits", r.limits)?,
            rate_limit: spec("rate_limit", r.rate_limit)?,
            source_kind: r.source_kind,
            source_ref: r.source_ref,
            live_version_id: r.live_version_id,
            live_method: r.live_method,
            live_path: r.live_path,
            state: r.state,
            created_by: r.created_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
        })
    }
}

// The `max_items = 16` written on both `statements` fields below IS
// `MAX_STATEMENTS`, and this is what makes that a fact rather than a hope.
//
// utoipa's `max_items` parses a LITERAL, so the schema cannot name the constant;
// without this, raising the limit in `egret-dataapi-api` would leave the
// published contract advertising the old number and nothing in the workspace
// would notice. A `const` assertion fails the BUILD, which is the earliest and
// loudest place to find out — `const _` because it is evaluated for its
// assertion and never read.
//
// The other end of the same number — the frontend's own `MAX_STATEMENTS`, which
// decides when the "Add statement" control stops offering one — is checked
// against the committed `openapi.json` by
// `frontend/src/routes/explore/dataapi/__tests__/Statements.test.tsx`. Between
// the two, the three copies cannot drift apart in any direction.
const _: () = assert!(
    MAX_STATEMENTS == 16,
    "MAX_STATEMENTS changed: update `#[schema(max_items = ...)]` on \
     CreateEndpointInput::statements and UpdateEndpointInput::statements, then \
     regenerate openapi.json"
);

/// The `POST` body.
///
/// The spec fields are `Option<T>` rather than `#[serde(default)] T` because
/// `CacheSpec`/`LimitsSpec`/`RateLimitSpec` deliberately have no `Default` impl —
/// `OutputSpec`'s hand-written one exists precisely because a derive would have
/// diverged from serde's defaults. An absent field is stored as `'{}'::jsonb` and
/// read back through the same serde defaults, so there is exactly one definition
/// of "the default" and no Rust-side copy of it to drift.
/// `deny_unknown_fields`: a misspelled TOP-LEVEL key is a 400 naming the field,
/// not a silently discarded value. Every field here is either required or
/// `#[serde(default)]`, so nothing legitimate is refused.
///
/// This attribute covers only this struct's own keys — it does not reach a
/// nested one. The nested config types carry their own; see the note above
/// `ParamSpec` in `egret-dataapi-api`, which is where the reported case
/// (`{"rate_limit": {"per_client_qps": 5}}` returning 201 with the limit
/// defaulted to `0`) is actually refused. Both layers are needed.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateEndpointInput {
    pub name: String,
    /// Free-form and user-typed. **Total-reject, not normalised**: a leading
    /// slash is optional and one trailing slash is absorbed, but anything else
    /// out of shape — an empty segment, a `.`/`..` segment, percent-encoding, a
    /// query string, a fragment, whitespace, a character outside
    /// `[A-Za-z0-9._~-]`, or a value over [`MAX_PATH_LEN`] bytes — is a 400
    /// naming the reason. Nothing is silently rewritten. See [`normalize_path`].
    pub path: String,
    /// The connection this endpoint queries. Validated against the CALLER's
    /// workspace by the route before it is stored — it arrives from the request
    /// body, so nothing else would stop it naming another workspace's database.
    pub database_id: Uuid,
    /// The statements this endpoint runs, in order. Required and non-empty:
    /// [`egret_dataapi_api::validate_statements`] refuses an empty list, a name
    /// that is not an identifier, and a repeated name, all as a 400 naming the
    /// reason — the schema's own `CHECK` behind it means an empty list is not
    /// merely refused here but unrepresentable.
    ///
    /// `max_items` puts [`MAX_STATEMENTS`] into the generated contract, which is
    /// the only place a client can learn it: it is a rule
    /// `validate_statements` applies, not a column, so nothing else in the
    /// document said the number at all. utoipa's `max_items` takes a LITERAL, so
    /// it cannot name the constant — the `const _: () = assert!(…)` above this
    /// struct is what stops the two drifting, and it does it at compile time.
    #[schema(max_items = 16)]
    pub statements: Vec<StatementSpec>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub folder_id: Option<Uuid>,
    #[serde(default)]
    pub params: Option<Vec<ParamSpec>>,
    #[serde(default)]
    pub output: Option<OutputSpec>,
    #[serde(default)]
    pub cache: Option<CacheSpec>,
    #[serde(default)]
    pub limits: Option<LimitsSpec>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitSpec>,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub source_ref: Option<Uuid>,
}

/// The `PATCH` body: every field absent means "leave it alone".
///
/// `folder_id` and `source_ref` are nullable columns, so "absent" and "set to
/// null" are different requests and a plain `Option<Uuid>` cannot tell them
/// apart — an author moving an endpoint back to the root would silently get "no
/// change". They use [`double_option`], where absent is `None` and an explicit
/// `null` is `Some(None)`.
///
/// `state` and `live_version_id` are NOT here. They are the publish lifecycle,
/// and a draft edit must never be able to put an endpoint live or repoint what a
/// live one serves.
/// `deny_unknown_fields` for the same reason as [`CreateEndpointInput`]: on a
/// PATCH the failure mode is worse, because a typo'd key looks exactly like
/// "leave it alone" and the author sees the OLD value persist rather than a
/// zeroed one.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateEndpointInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub database_id: Option<Uuid>,
    /// Replaces the whole list when present. There is no per-statement patch:
    /// the order is part of the definition, so "edit statement 2" and "insert a
    /// statement" would need an addressing scheme the list itself already is.
    ///
    /// Bounded in the contract the same way `CreateEndpointInput` is, and for
    /// the same reason: this route replaces the whole list, so it is the other
    /// way a client can send one that is too long.
    #[serde(default)]
    #[schema(max_items = 16)]
    pub statements: Option<Vec<StatementSpec>>,
    #[serde(default)]
    pub params: Option<Vec<ParamSpec>>,
    #[serde(default)]
    pub output: Option<OutputSpec>,
    #[serde(default)]
    pub cache: Option<CacheSpec>,
    #[serde(default)]
    pub limits: Option<LimitsSpec>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitSpec>,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<Uuid>)]
    pub folder_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<Uuid>)]
    pub source_ref: Option<Option<Uuid>>,
}

/// Distinguishes an absent JSON field from an explicit `null`. With
/// `#[serde(default)]`, absent stays `None` and `null` deserializes to
/// `Some(None)`.
fn double_option<'de, T, D>(de: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// The methods `dataapi.endpoints.method`'s CHECK constraint accepts. Checked in
/// the route so an unsupported verb is a 400 naming the allowed set, rather than
/// a 500 leaking `endpoints_method_check` at the author.
pub const ALLOWED_METHODS: [&str; 2] = ["GET", "POST"];

/// The `analytics.folders.entity_type` a folder must carry to hold an endpoint.
///
/// `analytics.folders` is polymorphic and has NO check on `entity_type` (0021,
/// and 0113 says so explicitly), so nothing at the database level stops an
/// endpoint being filed into a chart folder — it would then be invisible in the
/// endpoint list and appear in the chart one. The value follows the convention
/// the other seams use: the singular object name, as `alert` and `saved_query`
/// do.
///
/// Folders of this type are creatable through `egret-bi`'s generic folder
/// router, which maps a URL `kind` to `(entity_type, read_perm, manage_perm)`
/// in `kind_perms`; the `"data_api_endpoints" => ("data_api_endpoint",
/// "data_api_endpoint:read", "data_api_endpoint:manage")` arm is what makes
/// `folder_id` reachable from the UI, and both permissions are seeded by
/// migration 0122. The guard was written before that arm on purpose — an
/// unmatched rule is inert, but a field that lands before its guard is a hole.
pub const FOLDER_ENTITY_TYPE: &str = "data_api_endpoint";

/// The Cedar object KIND an endpoint appears as in an authorization decision,
/// and the `access_grants.object_type` of an invoke grant (spec §5, §6.2 step 4).
///
/// It happens to spell the same string as [`FOLDER_ENTITY_TYPE`] and is a
/// separate constant on purpose: one names a row in `analytics.folders`, the
/// other namespaces `Egret::Object::"<kind>:<uuid>"` in the PDP and is written
/// into `access_grants`. Aliasing them would make a rename of either silently
/// re-point every invoke grant at a kind nothing checks — grants that keep
/// matching the schema and stop matching the decision.
///
/// Re-exported from `egret-dataapi-api` (Task 4) rather than defined here: it
/// is now the ONE definition, and `egret-automation` names the same kind
/// through that crate without depending on this one.
pub use egret_dataapi_api::OBJECT_KIND;

/// The `analytics.bookmarks.entity_type` an endpoint's bookmarks are filed
/// under.
///
/// A macro rather than a bare `const` because the three statements that must
/// agree on it — the INSERT, the DELETE, and the list's `EXISTS` — are
/// `concat!`-ed `&'static str` literals (sqlx 0.9), and a `const` cannot be
/// spliced into one. If those three ever disagreed, bookmarks would be written
/// into one bucket and read from another: the star would take the click, the
/// row would be there, and the list would never light it — a silent failure
/// with nothing to grep for. Spelling it once makes that impossible.
///
/// A third constant spelling `data_api_endpoint`, beside [`FOLDER_ENTITY_TYPE`]
/// and [`OBJECT_KIND`], and separate for the same reason they are separate from
/// each other: this one names a row in `analytics.bookmarks`, and renaming any
/// one of the three must not silently re-point the other two.
macro_rules! bookmark_entity {
    () => {
        "data_api_endpoint"
    };
}

/// [`bookmark_entity!`] as a value, for callers that need it outside a SQL
/// literal.
pub const BOOKMARK_ENTITY: &str = bookmark_entity!();

/// The longest a stored endpoint path may be, in bytes.
///
/// Not a product limit — a limit that already exists further down, made visible.
/// `path` is a component of the btree index behind
/// `UNIQUE (workspace_id, method, path)`, and Postgres refuses an index entry
/// past roughly a third of a page (~2704 bytes at the default 8 kB). Without a
/// bound here that refusal arrives from the INSERT as an opaque 500; with one it
/// is a 400 that names the field and the cap. Set well below the real ceiling
/// rather than tracking it, since it is version- and page-size-dependent, and
/// far above any genuine API path.
pub const MAX_PATH_LEN: usize = 512;

/// Canonicalise a user-typed URL path, or say why it cannot be one.
///
/// **Contract for the serving path (P4).** The stored path is the canonical form
/// this function produces: one leading slash, no trailing slash, and every
/// segment drawn from `[A-Za-z0-9._~-]`. P4 serves at
/// `/data-api/v1/{ws}/{*path}`, and axum/matchit **percent-decode a wildcard
/// capture**, so an inbound path arrives already decoded. Route the inbound path
/// through this same function before looking a row up. Do not compare raw
/// strings, and do not store a path that has not been through here — if the two
/// sides disagree about encoding or about a trailing slash, the endpoint is
/// unreachable and nothing reports why.
///
/// **Why this rejects rather than rewrites.** The reason the path must be
/// canonical at all is that `UNIQUE (workspace_id, method, path)` compares TEXT:
/// any two spellings of one URL are two rows and one route, and the collision
/// surfaces only in P4, when serving has to pick one. Normalising the slashes
/// alone fixes one alias family out of several — dot segments (`/a/../b`),
/// interior `//`, percent-encoding (`%65` vs `e`, `%2f` vs `%2F`), and query or
/// fragment characters swallowed into the path all produce the same outcome, and
/// a partial canonicaliser is the dangerous shape because it looks total.
/// Rewriting them all correctly means implementing RFC 3986 normalisation;
/// rejecting them is total, needs nothing, and turns each case into a 400 the
/// author can read. It costs nothing in expressiveness: `ParamLocation` is
/// `Query | Body` only, so a path is a LITERAL — there are no path templates for
/// this charset to get in the way of.
///
/// Leading and trailing whitespace is trimmed (it is what a copy-paste brings,
/// and trimming is itself total); whitespace anywhere else is rejected.
///
/// `Err` carries the reason, which the route returns verbatim as a 400.
pub fn normalize_path(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    // Bounded here, or the btree index behind `UNIQUE (workspace_id, method,
    // path)` refuses the row instead — Postgres caps an index entry at roughly
    // 2704 bytes for an 8 kB page, and the failure surfaces as an opaque 500
    // from the INSERT rather than a 400 the author can act on. The exact cap is
    // version- and page-size-dependent, so this sits well under it rather than
    // trying to track it. 512 bytes is far past any real API path.
    if trimmed.len() > MAX_PATH_LEN {
        return Err(format!(
            "path must be at most {MAX_PATH_LEN} bytes (got {})",
            trimmed.len()
        ));
    }
    if let Some(c) = trimmed
        .chars()
        .find(|c| c.is_whitespace() || c.is_control())
    {
        return Err(format!(
            "path must not contain whitespace or control characters (found {c:?})"
        ));
    }
    // Named individually before the charset check below, which would also catch
    // them: "path must not be percent-encoded" tells an author what to do, and
    // "segment `%65` may contain only ..." does not.
    for (bad, why) in [
        ('%', "be percent-encoded — store the decoded path"),
        (
            '?',
            "contain a query string; declare query parameters in `params`",
        ),
        ('#', "contain a fragment"),
    ] {
        if trimmed.contains(bad) {
            return Err(format!("path must not {why}"));
        }
    }

    // At most ONE leading and ONE trailing slash is absorbed — not a run of them.
    // `trim_matches('/')` would have quietly rewritten `/a//` to `/a` while
    // `/a//b` was rejected, so the same typo would be an error in one position
    // and a silent edit in another. Stripping one each leaves every other slash
    // for the empty-segment check below, which answers all of them the same way.
    let body = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let body = body.strip_suffix('/').unwrap_or(body);
    if body.is_empty() {
        return Err("path must not be empty".into());
    }

    let mut out = String::with_capacity(body.len() + 1);
    for seg in body.split('/') {
        if seg.is_empty() {
            return Err("path must not contain an empty segment (`//`)".into());
        }
        if seg == "." || seg == ".." {
            return Err("path must not contain `.` or `..` segments".into());
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
        {
            return Err(format!(
                "path segment `{seg}` may contain only letters, digits and . _ ~ -"
            ));
        }
        out.push('/');
        out.push_str(seg);
    }
    Ok(out)
}

/// Persistence for endpoint drafts.
///
/// The publish lifecycle ([`crate::lifecycle`]) adds its own statements to this
/// same store rather than standing up a second one, so there is one place where
/// `AND workspace_id = $n` either is or is not present.
#[derive(Clone)]
pub struct EndpointStore {
    pub(crate) db: Db,
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// `None` unless `e` is the `UNIQUE (workspace_id, method, path)` violation.
/// Every other unique constraint on this table would be a different bug, and
/// reporting it as a path collision would send the author looking in the wrong
/// place.
fn is_path_conflict(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

/// The `UNIQUE (workspace_id, method, path)` refusal, as an author reads it.
///
/// Names the URL rather than the constraint: a bare "conflict" on a screen with a
/// dozen drafts is not actionable.
fn conflict(method: &str, path: &str) -> Error {
    Error::Conflict(format!(
        "an endpoint already exists at {method} {path} in this workspace"
    ))
}

/// The same refusal when the row holding the slot is IN THE TRASH.
///
/// `UNIQUE (workspace_id, method, path)` (0121) has no `WHERE deleted_at IS NULL`
/// — deliberately, per the decision recorded on this task: a trashed endpoint
/// keeps its URL so that restoring it can never fail with a conflict of its own.
/// The cost is that [`conflict`]'s message would point at a row the author cannot
/// see in any list, which is a dead end — the author looks at a screen with no
/// `GET /orders` on it and is told `GET /orders` exists. So the refusal has to
/// name the real cause AND the two ways out: restore that endpoint, or delete it
/// permanently to free the URL.
///
/// The method and path are still in the text, because
/// `duplicate_method_and_path_is_a_conflict` and
/// `renaming_a_path_onto_an_existing_one_is_a_conflict` require the colliding URL
/// in every 409 from this pair, whichever branch produced it.
fn trashed_conflict(method: &str, path: &str) -> Error {
    Error::Conflict(format!(
        "a trashed endpoint still holds {method} {path} in this workspace — \
         restore it, or delete it permanently to free the URL"
    ))
}

impl EndpointStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every draft in `workspace`, each carrying whether `user` has starred it.
    ///
    /// `user` is the CALLER — the route passes `ctx.principal`, never anything
    /// from the request — so the flag answers "did I star this", which is the
    /// only question a per-user bookmark can answer. There is no unfiltered
    /// variant of this read: a list without a caller has no `bookmarked` to
    /// report, so making one would only be a way to get the flag wrong.
    ///
    /// The `EXISTS` is a correlated sub-select rather than a join so a row
    /// appears exactly once whatever the bookmarks table holds.
    ///
    /// `trashed` partitions the workspace rather than widening it: `false` — the
    /// default the route applies when the query string says nothing — lists only
    /// rows with `deleted_at IS NULL`, and `true` lists only the trashed ones.
    /// The same shape `egret-bi`'s dataset and dashboard lists established, so
    /// "Trash" is a separate screen fed by the same endpoint and no list can show
    /// a trashed row beside a live one.
    ///
    /// The predicate is `$3 = (deleted_at IS NOT NULL)` and not two statements
    /// chosen at runtime: sqlx 0.9's `query_as` takes `&'static str`, so a
    /// branch would mean either two `concat!`-ed literals that can drift apart or
    /// a `format!` through `AssertSqlSafe`. One statement, one bound boolean, and
    /// the index added by 0126 is on `(workspace_id, deleted_at)` for exactly
    /// this read.
    pub async fn list_for(
        &self,
        workspace: Uuid,
        user: Uuid,
        trashed: bool,
    ) -> Result<Vec<DataApiEndpointListItemDto>> {
        let rows: Vec<RowWithBookmark> = sqlx::query_as(concat!(
            "SELECT ",
            endpoint_cols!(),
            ", EXISTS (SELECT 1 FROM analytics.bookmarks b \
               WHERE b.user_id = $2 AND b.entity_type = '",
            bookmark_entity!(),
            "' AND b.entity_id = endpoints.id) AS bookmarked \
             FROM dataapi.endpoints \
             WHERE workspace_id = $1 AND $3 = (deleted_at IS NOT NULL) \
             ORDER BY created_at DESC"
        ))
        .bind(workspace)
        .bind(user)
        .bind(trashed)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| {
                Ok(DataApiEndpointListItemDto {
                    endpoint: r.row.try_into()?,
                    bookmarked: r.bookmarked,
                })
            })
            .collect()
    }

    /// Confirm `id` names an endpoint in `workspace`, or [`Error::NotFound`].
    ///
    /// The bookmark routes need this and cannot get it the way every other
    /// statement in this module does: `analytics.bookmarks` is generic and has
    /// no `workspace_id` to put in a `WHERE`, so a bookmark write carries no
    /// containment of its own. Without this call a caller could file a row
    /// against any endpoint id in any workspace — and, because the list only
    /// ever joins its own workspace's rows, would get no feedback either way,
    /// which is exactly the shape of a bug nobody reports.
    ///
    /// Cheaper than [`Self::get`] on purpose: it answers existence, and does
    /// not parse five jsonb specs whose unreadability would turn a bookmark
    /// into a 500.
    pub async fn scoped(&self, workspace: Uuid, id: Uuid) -> Result<()> {
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        found
            .map(|_| ())
            .ok_or_else(|| Error::NotFound("endpoint".into()))
    }

    /// The workspace `id` belongs to, or `Ok(None)` if no such row exists at
    /// all — trashed or live, both answer with their real workspace, since
    /// `deleted_at` plays no part in this query.
    ///
    /// UNSCOPED by design, unlike [`Self::scoped`]/[`Self::get`]/every other
    /// read in this module: those all take a `workspace` the CALLER already
    /// claims to be in and filter by it, refusing anything outside it as
    /// `NotFound`. This one exists for the opposite direction — a caller
    /// (`egret-automation`, via the `egret_dataapi_api::DataApiEndpointPort`
    /// seam this implements) that does NOT yet know whether `id` belongs to
    /// its own workspace, and needs to find out before deciding what to do
    /// with the answer. See that trait's own doc comment for why the caller,
    /// not this function, is responsible for refusing a mismatch.
    pub async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>> {
        sqlx::query_scalar("SELECT workspace_id FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// Star `endpoint` for `user`.
    ///
    /// Idempotent: a second call is a no-op rather than an error, because the
    /// UI's star is a toggle and a double click must not 500. Containment is
    /// the CALLER's job — see [`Self::scoped`].
    pub async fn add_bookmark(&self, user: Uuid, endpoint: Uuid) -> Result<()> {
        sqlx::query(concat!(
            "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) \
             VALUES ($1, '",
            bookmark_entity!(),
            "', $2) ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING"
        ))
        .bind(user)
        .bind(endpoint)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Unstar `endpoint` for `user`. Deleting nothing is success — the other
    /// half of the toggle.
    pub async fn remove_bookmark(&self, user: Uuid, endpoint: Uuid) -> Result<()> {
        sqlx::query(concat!(
            "DELETE FROM analytics.bookmarks \
             WHERE user_id = $1 AND entity_type = '",
            bookmark_entity!(),
            "' AND entity_id = $2"
        ))
        .bind(user)
        .bind(endpoint)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// One draft, if it belongs to `workspace`. `NotFound` otherwise — see the
    /// module comment on why the predicate is here and not in the route.
    pub async fn get(&self, workspace: Uuid, id: Uuid) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            endpoint_cols!(),
            " FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("endpoint".into()))?
            .try_into()
    }

    /// The same read, inside a transaction and holding a row lock.
    ///
    /// Publishing needs this: the output columns it is about to freeze were
    /// resolved from a draft read moments earlier, and it has to know the row has
    /// not moved since. `FOR UPDATE` also serialises concurrent publishes of one
    /// endpoint, which is what makes `max(version) + 1` safe.
    ///
    /// `Row` stays private to this module — the lifecycle module gets the parsed
    /// DTO, so there is still exactly one place that knows the column list.
    pub(crate) async fn locked(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        workspace: Uuid,
        id: Uuid,
    ) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            endpoint_cols!(),
            " FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2 FOR UPDATE"
        ))
        .bind(id)
        .bind(workspace)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("endpoint".into()))?
            .try_into()
    }

    /// Write `state`, and nothing else.
    ///
    /// `pub(crate)` and deliberately not on the PATCH path: [`crate::lifecycle`]
    /// is the only caller, because moving an endpoint in or out of service is a
    /// lifecycle action with its own route and its own permission check, not a
    /// field an author can set by editing a draft. `live_version_id` is untouched
    /// here — disabling must not throw away what was being served.
    ///
    /// `state` is a caller-chosen `&'static str` rather than a bound parameter of
    /// unknown provenance; the column's CHECK constraint is the backstop.
    ///
    /// **`deleted_at IS NULL` is in the `WHERE`, and this is the statement that
    /// made it necessary.** [`crate::lifecycle::EndpointStore::enable`] calls this
    /// with `"live"` after checking only that a version exists; on a trashed row
    /// that would have set `state = 'live'`, and `live_snapshot` — which filters
    /// `state = 'live'` and nothing else — would then have served a trashed
    /// endpoint. Migration 0126's CHECK refuses it, which is the guarantee, but a
    /// CHECK violation reaches the author as a 500 naming
    /// `dataapi_endpoints_trashed_not_live`. The predicate here is what turns that
    /// into an answer, and it is in the UPDATE rather than in a preceding read so a
    /// concurrent trash cannot land between the two.
    pub(crate) async fn set_state(
        &self,
        workspace: Uuid,
        id: Uuid,
        state: &'static str,
    ) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "UPDATE dataapi.endpoints SET state = $3, updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL RETURNING ",
            endpoint_cols!()
        ))
        .bind(id)
        .bind(workspace)
        .bind(state)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        match row {
            Some(row) => row.try_into(),
            None => Err(self
                .trashed_conflict_or(
                    workspace,
                    id,
                    "changing whether it serves",
                    Error::NotFound("endpoint".into()),
                )
                .await),
        }
    }

    /// The answer a guarded writer owes when its statement matched no row.
    ///
    /// Every writer that grew an `AND deleted_at IS NULL` needs the same
    /// disambiguation, and none of them should invent its own message: a foreign or
    /// absent id must stay a 404 (this re-read is workspace-scoped, so
    /// [`Self::get`] answers that first and the 409 never confirms that an id names
    /// something), and only a row that really is in the trash becomes a 409.
    ///
    /// `action` completes "restore it before …", so the refusal names both the
    /// state the endpoint is in and the call to repeat once it is out — which is
    /// the whole difference between this and the 500 it replaces.
    ///
    /// `otherwise` is the answer for a row that exists and is NOT in the trash.
    /// That is reachable two ways: a genuine no-such-thing (rollback naming a
    /// version of another endpoint) and a restore that committed between the
    /// statement and this read. Both are the caller's own not-found, which is why
    /// the caller supplies it rather than this helper assuming "endpoint".
    ///
    /// Reads AFTER the write attempt, never before it: a read-then-write guard
    /// would leave a window for a concurrent trash, which is the shape
    /// [`Self::trash`] and [`Self::restore`] already avoid.
    pub(crate) async fn trashed_conflict_or(
        &self,
        workspace: Uuid,
        id: Uuid,
        action: &str,
        otherwise: Error,
    ) -> Error {
        match self.get(workspace, id).await {
            Ok(current) if current.deleted_at.is_some() => Error::Conflict(format!(
                "endpoint is in the trash; restore it before {action}"
            )),
            Ok(_) => otherwise,
            Err(e) => e,
        }
    }

    /// Insert a draft. `path` must already be normalised, and `database_id` and
    /// `folder_id` already confirmed to belong to `workspace`; those are the
    /// route's job, because they name OTHER tables and their absence has to be
    /// answered with an HTTP status this layer does not pick.
    pub async fn create(
        &self,
        workspace: Uuid,
        by: UserId,
        method: &str,
        path: &str,
        input: &CreateEndpointInput,
    ) -> Result<DataApiEndpointDto> {
        let row: std::result::Result<Row, sqlx::Error> = sqlx::query_as(concat!(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, folder_id, name, description, path, method, database_id, \
                statements, params, output, cache, limits, rate_limit, source_kind, source_ref, \
                created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
                     COALESCE($9, '[]'::jsonb), COALESCE($10, '{}'::jsonb), \
                     COALESCE($11, '{}'::jsonb), COALESCE($12, '{}'::jsonb), \
                     COALESCE($13, '{}'::jsonb), COALESCE($14, 'raw'), $15, $16) \
             RETURNING ",
            endpoint_cols!()
        ))
        .bind(workspace)
        .bind(input.folder_id)
        .bind(input.name.trim())
        .bind(&input.description)
        .bind(path)
        .bind(method)
        .bind(input.database_id)
        .bind(json_value(&input.statements)?)
        .bind(to_json(input.params.as_ref())?)
        .bind(to_json(input.output.as_ref())?)
        .bind(to_json(input.cache.as_ref())?)
        .bind(to_json(input.limits.as_ref())?)
        .bind(to_json(input.rate_limit.as_ref())?)
        .bind(input.source_kind.as_deref())
        .bind(input.source_ref)
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await;
        // A `match` rather than `map_err`: telling a trashed collision apart from
        // a visible one needs a second query, and a query can only be awaited.
        let row = match row {
            Ok(row) => row,
            Err(e) if is_path_conflict(&e) => {
                return Err(self.path_conflict(workspace, method, path).await)
            }
            Err(e) => return Err(db_err(e)),
        };
        row.try_into()
    }

    /// Which conflict this is: a visible endpoint holding the slot, or a trashed
    /// one the author cannot see.
    ///
    /// Called only after [`is_path_conflict`] has already established that the
    /// UNIQUE fired, so the row it looks for is there by construction. If it
    /// somehow is not — the holder was deleted between the failed write and this
    /// read — the answer falls back to [`conflict`], which is the message that was
    /// correct before trash existed. A missing row must never become "a trashed
    /// endpoint holds it": that would send the author to a Trash screen with
    /// nothing on it.
    async fn path_conflict(&self, workspace: Uuid, method: &str, path: &str) -> Error {
        let holder: std::result::Result<Option<Option<DateTime<Utc>>>, sqlx::Error> =
            sqlx::query_scalar(
                "SELECT deleted_at FROM dataapi.endpoints \
                 WHERE workspace_id = $1 AND method = $2 AND path = $3",
            )
            .bind(workspace)
            .bind(method)
            .bind(path)
            .fetch_optional(&self.db.pool)
            .await;
        match holder {
            Ok(Some(Some(_))) => trashed_conflict(method, path),
            _ => conflict(method, path),
        }
    }

    /// Move an endpoint to the trash: `deleted_at = now()`, the row and its
    /// published versions left where they are.
    ///
    /// **A `live` endpoint is refused with a 409 naming disable-first, and the
    /// refusal is in the statement's own `WHERE`.** Migration 0126's
    /// `CHECK (deleted_at IS NULL OR state <> 'live')` would refuse it too — that
    /// is the guarantee, and it holds against writers that never come through
    /// here — but a CHECK violation surfaces as an opaque 500 naming a constraint,
    /// which tells an author nothing about what to do. Both layers are wanted:
    /// this one for the message, the constraint for the promise. Putting the
    /// predicate in the UPDATE rather than in a read-then-write also means a
    /// publish landing in between cannot slip a live endpoint into the trash —
    /// the row simply stops matching.
    ///
    /// Trashing an already-trashed endpoint is a no-op returning the row as it
    /// stands, so a double click does not restamp `deleted_at` and lose when it
    /// was actually trashed.
    ///
    /// The invoke grants are NOT swept, unlike [`Self::delete`]: trash is
    /// reversible, and a restore that came back without its grants would silently
    /// break every client that had been calling it. What stops a trashed endpoint
    /// answering a request is `state <> 'live'`, not the absence of grants.
    pub async fn trash(&self, workspace: Uuid, id: Uuid) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "UPDATE dataapi.endpoints SET deleted_at = now(), updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 \
               AND deleted_at IS NULL AND state <> 'live' \
             RETURNING ",
            endpoint_cols!()
        ))
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        if let Some(row) = row {
            return row.try_into();
        }
        // No row matched, and the three reasons need three different answers. The
        // read is scoped to the workspace, so a foreign id is `NotFound` here and
        // never reaches the state check.
        let current = self.get(workspace, id).await?;
        if current.deleted_at.is_some() {
            return Ok(current);
        }
        Err(Error::Conflict(format!(
            "endpoint is {} and cannot be moved to the trash; disable it first",
            current.state
        )))
    }

    /// Bring an endpoint back from the trash, `disabled`.
    ///
    /// **`disabled`, never `live`, whatever it was before.** A restore that
    /// resumed serving would put an endpoint back on the internet as a side effect
    /// of a Trash screen's undo button, and the author who trashed it may not be
    /// the one restoring it. `live_version_id` is untouched, so `enable` is one
    /// click away and needs no re-publish — the same contract `disable` keeps.
    ///
    /// Refuses a row that is not in the trash rather than treating restore as a
    /// generic state setter: applied to a LIVE endpoint, an unconditional
    /// `state = 'disabled'` would take it out of service, which is `disable`'s job
    /// and its audit event. `deleted_at IS NOT NULL` is in the `WHERE` for the same
    /// reason it is in [`Self::trash`] — no read-then-write window.
    pub async fn restore(&self, workspace: Uuid, id: Uuid) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "UPDATE dataapi.endpoints \
                SET deleted_at = NULL, state = 'disabled', updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NOT NULL \
             RETURNING ",
            endpoint_cols!()
        ))
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        if let Some(row) = row {
            return row.try_into();
        }
        // Existence first — a foreign or absent id is a 404, not a 409 that would
        // confirm the id names something.
        self.get(workspace, id).await?;
        Err(Error::Conflict(
            "endpoint is not in the trash; there is nothing to restore".into(),
        ))
    }

    /// Apply a partial edit. Absent fields are left alone by `COALESCE`ing the
    /// column against itself, so this is a single statement with no read-modify-
    /// write window — two concurrent PATCHes of different fields both land.
    ///
    /// Scoped to `workspace`: a row belonging to anyone else is `NotFound` and is
    /// never touched, because the predicate is in the statement rather than in a
    /// guard someone has to remember to call.
    ///
    /// `path` (if present) must already be normalised, and `database_id` /
    /// `folder_id` (if present) already confirmed to belong to `workspace` — those
    /// name OTHER tables, so their absence has to be answered by the route, which
    /// is the layer that picks the status code.
    ///
    /// **A trashed endpoint is not editable, and `deleted_at IS NULL` in the `WHERE`
    /// is what says so.** This statement touches neither `state` nor
    /// `live_version_id`, so it cannot resurrect serving and migration 0126's CHECK
    /// never fires — which is precisely why it needed a guard: before this
    /// predicate it answered 200 and edited a row that appears on no list the
    /// author is looking at. The concrete harm is `path`: a trashed endpoint KEEPS
    /// its URL, because `UNIQUE (workspace_id, method, path)` has no
    /// `WHERE deleted_at IS NULL`, so an edit here silently changed WHICH URL was
    /// held and the 409 the next author got pointed at a row they could not see.
    pub async fn update(
        &self,
        workspace: Uuid,
        id: Uuid,
        input: &UpdateEndpointInput,
    ) -> Result<DataApiEndpointDto> {
        let res: std::result::Result<Option<Row>, sqlx::Error> = sqlx::query_as(concat!(
            "UPDATE dataapi.endpoints SET \
               name = COALESCE($2, name), \
               description = COALESCE($3, description), \
               path = COALESCE($4, path), \
               method = COALESCE($5, method), \
               database_id = COALESCE($6, database_id), \
               statements = COALESCE($7, statements), \
               params = COALESCE($8, params), \
               output = COALESCE($9, output), \
               cache = COALESCE($10, cache), \
               limits = COALESCE($11, limits), \
               rate_limit = COALESCE($12, rate_limit), \
               source_kind = COALESCE($13, source_kind), \
               folder_id = CASE WHEN $14 THEN $15 ELSE folder_id END, \
               source_ref = CASE WHEN $16 THEN $17 ELSE source_ref END, \
               updated_at = now() \
             WHERE id = $1 AND workspace_id = $18 AND deleted_at IS NULL \
             RETURNING ",
            endpoint_cols!()
        ))
        .bind(id)
        .bind(input.name.as_deref().map(str::trim))
        .bind(input.description.as_deref())
        .bind(input.path.as_deref())
        .bind(input.method.as_deref())
        .bind(input.database_id)
        .bind(to_json(input.statements.as_ref())?)
        .bind(to_json(input.params.as_ref())?)
        .bind(to_json(input.output.as_ref())?)
        .bind(to_json(input.cache.as_ref())?)
        .bind(to_json(input.limits.as_ref())?)
        .bind(to_json(input.rate_limit.as_ref())?)
        .bind(input.source_kind.as_deref())
        .bind(input.folder_id.is_some())
        .bind(input.folder_id.flatten())
        .bind(input.source_ref.is_some())
        .bind(input.source_ref.flatten())
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await;
        // A `match`, not `map_err`: naming the colliding URL needs a second query,
        // and after a partial PATCH that URL is half in the request and half in
        // the row — so the conflict message can only be built by awaiting.
        let row = match res {
            Ok(row) => row,
            Err(e) if is_path_conflict(&e) => {
                let (method, path) = self.effective_route(workspace, id, input).await;
                return Err(self.path_conflict(workspace, &method, &path).await);
            }
            Err(e) => return Err(db_err(e)),
        };
        // `fetch_optional` rather than `fetch_one`: no row means either no such
        // endpoint or not this workspace's — the SAME answer, deliberately — and
        // also covers a concurrent delete, which must read as 404 rather than as
        // an internal error. The trash is the one reason among those that is NOT
        // a 404, so it is separated out here and nowhere else.
        match row {
            Some(row) => row.try_into(),
            None => Err(self
                .trashed_conflict_or(
                    workspace,
                    id,
                    "editing it",
                    Error::NotFound("endpoint".into()),
                )
                .await),
        }
    }

    /// The effective method and path an update WOULD have written, for the
    /// conflict message.
    async fn effective_route(
        &self,
        workspace: Uuid,
        id: Uuid,
        input: &UpdateEndpointInput,
    ) -> (String, String) {
        let current: Option<(String, String)> = sqlx::query_as(
            "SELECT method, path FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace)
        .fetch_optional(&self.db.pool)
        .await
        .ok()
        .flatten();
        let (cur_method, cur_path) = current.unwrap_or_default();
        (
            input.method.clone().unwrap_or(cur_method),
            input.path.clone().unwrap_or(cur_path),
        )
    }

    /// Delete a draft belonging to `workspace`. `NotFound` if it belongs to
    /// anyone else, and — this is the part a bare `DELETE` gets wrong — the row
    /// is not deleted either.
    ///
    /// **PERMANENT, and unchanged by the arrival of trash.** `DELETE` did not
    /// become a soft delete when [`Self::trash`] was added: the trash is a
    /// separate, reversible state, and this is still the statement that destroys
    /// the endpoint and cascades its published versions away. It applies to a
    /// trashed row as readily as to a visible one — emptying the trash is this
    /// call — which is also what frees a URL that a trashed endpoint still holds.
    ///
    /// `dataapi.endpoint_versions` cascades from here (0121), so deleting an
    /// endpoint takes its published history with it — which is why the route
    /// gates this on `:manage` and nothing weaker, and why "deleted the wrong
    /// workspace's row" is not a recoverable mistake.
    ///
    /// **`RETURNING` is why this is not a bare `DELETE`, and it is an audit
    /// requirement rather than a convenience.** Every other audited route leaves
    /// its subject behind for an auditor to join to — `revoke` is a soft
    /// `UPDATE revoked_at`, `disable` keeps the row and the version pointer. This
    /// one destroys the endpoint AND cascades away every published version, so
    /// the `data_api_endpoint:delete` event is the only surviving record of the
    /// thing. Without these columns it would carry a uuid that resolves to
    /// nothing: an auditor could see that *something* was deleted and never
    /// which URL stopped being served. `RETURNING` also costs no second query
    /// and cannot race, since it reads the row the DELETE itself removed.
    /// **The invoke grants go with it, and that needs a statement of its own.**
    /// `access_grants.scope_id` is POLYMORPHIC with no FK to anything (migration
    /// 0068) and no cleanup trigger, so nothing at the database level removes an
    /// endpoint's grants when the endpoint is destroyed. This is the same shape
    /// `UserRepo::hard_delete` already handles from the PRINCIPAL side — it
    /// sweeps `access_grants` by `principal_id`, which is what keeps deleting a
    /// service account from leaving grants behind. This is the OBJECT side of the
    /// same gap, and without it a deleted endpoint leaves rows that the grants
    /// list would render with a null name, that accumulate without bound, and
    /// that would silently come back to life if `gen_random_uuid()` ever
    /// reissued the id (it will not — but a grant surviving its object is not a
    /// thing to reason about probabilistically).
    ///
    /// Both statements run in ONE transaction. Split, a failure between them
    /// leaves exactly the orphan this is here to prevent, and the endpoint would
    /// already be gone so nothing could retry the sweep.
    pub async fn delete(&self, workspace: Uuid, id: Uuid) -> Result<DeletedEndpoint> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let row: Option<(String, String, String, Uuid)> = sqlx::query_as(
            "DELETE FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2 \
             RETURNING name, path, method, database_id",
        )
        .bind(id)
        .bind(workspace)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        // Reported rather than swallowed: a DELETE that matched nothing is the
        // same event as a GET that found nothing, and must answer the same way.
        // Checked BEFORE the sweep, so a 404 cannot delete another workspace's
        // grants on an id it was never allowed to name.
        let (name, path, method, database_id) =
            row.ok_or_else(|| Error::NotFound("endpoint".into()))?;
        // Scoped by `object_type` as well as `scope_id`: a uuid is not unique
        // across kinds by construction, and this must not reach a dashboard
        // share that happens to carry the same scope id.
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE scope_type = 'object' AND object_type = $1 AND scope_id = $2",
        )
        .bind(OBJECT_KIND)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(DeletedEndpoint {
            name,
            path,
            method,
            database_id,
        })
    }
}

/// What a deleted endpoint WAS, captured by [`EndpointStore::delete`]'s
/// `RETURNING` clause.
///
/// Not a response DTO — the route still answers [`egret_core::Ack`]. This exists
/// so the audit event can name the endpoint that no longer exists to be named,
/// which is the only reason the columns are read at all.
#[derive(Debug, Clone)]
pub struct DeletedEndpoint {
    pub name: String,
    pub path: String,
    pub method: String,
    pub database_id: Uuid,
}

/// Serialize an optional spec for binding. `None` binds SQL NULL, which every
/// statement above `COALESCE`s to the column's existing value (update) or its
/// declared default (create).
fn to_json<T: Serialize>(v: Option<&T>) -> Result<Option<serde_json::Value>> {
    v.map(json_value).transpose()
}

/// [`to_json`] for a column that is never absent — `statements` is required on
/// create, so binding it as `Option` would invite a `None` the NOT NULL column
/// would answer with a 500.
fn json_value<T: Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v)
        .map_err(|e| Error::Internal(anyhow::anyhow!("serialize endpoint spec: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_canonicalised_to_one_leading_slash() {
        for raw in [
            "reports/revenue",
            "/reports/revenue",
            "/reports/revenue/",
            "reports/revenue/",
            "  /reports/revenue  ",
        ] {
            assert_eq!(
                normalize_path(raw).as_deref(),
                Ok("/reports/revenue"),
                "{raw:?}"
            );
        }
        // The permitted charset, all of it, in one path.
        assert_eq!(
            normalize_path("a-b/c_d/e.f/g~h/I9").as_deref(),
            Ok("/a-b/c_d/e.f/g~h/I9")
        );
    }

    /// The empty forms must be rejected rather than normalised into `"/"` — an
    /// endpoint at the workspace root is not a thing an author can have meant,
    /// and `""` would collide with every other empty path in the workspace.
    #[test]
    fn a_path_with_nothing_in_it_is_rejected() {
        for raw in ["", "   ", "/", "///"] {
            assert!(normalize_path(raw).is_err(), "{raw:?}");
        }
    }

    /// Every alias family that would otherwise put two rows behind one URL. The
    /// slash rule alone handles the last of these; each of the others is a
    /// separate way to spell the same route, and a canonicaliser that fixed only
    /// the slashes would look total while admitting all of them.
    ///
    /// Percent-encoding is the one that also breaks serving outright: P4 matches
    /// `/data-api/v1/{ws}/{*path}`, and axum percent-DECODES a wildcard capture,
    /// so a stored `%65` could never be matched by any request.
    #[test]
    fn every_alias_family_is_rejected() {
        for (raw, why) in [
            ("/a/../b", "dot-dot segment"),
            ("/a/./b", "dot segment"),
            ("/..", "bare dot-dot"),
            ("/reports//x", "interior double slash"),
            ("/a//", "double slash before the trailing one"),
            ("/r%65ports/x", "percent-encoded letter"),
            ("/reports%2fx", "percent-encoded slash, lower case"),
            ("/reports%2Fx", "percent-encoded slash, upper case"),
            ("/reports/x?a=1", "query string"),
            ("/reports/x#frag", "fragment"),
            ("/reports/my report", "interior space"),
            ("/reports/x\ty", "tab"),
            ("/reports/x\ny", "newline"),
            ("/reports/x\u{7f}", "control character"),
            ("/reports/:id", "path template — params are Query|Body only"),
            ("/reports/{id}", "brace template"),
            (
                "/reports/é",
                "non-ASCII, which a client would percent-encode",
            ),
            ("/reports/a b", "space inside a segment"),
        ] {
            assert!(
                normalize_path(raw).is_err(),
                "{why}: {raw:?} was accepted as {:?}",
                normalize_path(raw)
            );
        }
    }

    /// The rejection has to say WHICH rule was broken — an author who typed a
    /// percent sign needs to be told that, not "invalid path".
    #[test]
    fn a_rejection_names_the_rule_it_broke() {
        assert!(normalize_path("/a%20b").unwrap_err().contains("percent"));
        assert!(normalize_path("/a?b=1").unwrap_err().contains("query"));
        assert!(normalize_path("/a#b").unwrap_err().contains("fragment"));
        assert!(normalize_path("/a b").unwrap_err().contains("whitespace"));
        assert!(normalize_path("/a//b")
            .unwrap_err()
            .contains("empty segment"));
        assert!(normalize_path("/a/../b").unwrap_err().contains("`..`"));
        assert!(normalize_path("/a/:id").unwrap_err().contains(":id"));
    }

    /// An over-long path is a 400 naming the cap, not the btree index's opaque
    /// 500 from the INSERT.
    #[test]
    fn an_over_long_path_is_refused_with_the_cap_in_the_message() {
        let long = format!("/{}", "a".repeat(MAX_PATH_LEN));
        let err = normalize_path(&long).unwrap_err();
        assert!(err.contains("512"), "the cap must be in the message: {err}");
        // Charset-legal, so nothing else could have refused it — this fails if
        // the bound is removed.
        assert!(long.chars().skip(1).all(|c| c.is_ascii_alphanumeric()));
        // And exactly at the cap is still accepted, so the bound is not off by
        // one in the direction that silently shortens the usable space.
        let at_cap = format!("/{}", "a".repeat(MAX_PATH_LEN - 1));
        assert_eq!(at_cap.len(), MAX_PATH_LEN);
        assert!(normalize_path(&at_cap).is_ok());
    }

    /// A misspelled key must 400 rather than land as a default. `per_client_qps`
    /// is the exact spelling the spec documented while the field is
    /// `per_key_qps`, so this is the real reported case, not an invented one.
    #[test]
    fn an_unknown_field_is_refused_rather_than_dropped() {
        let body = r#"{"name":"n","path":"/a","database_id":"00000000-0000-0000-0000-000000000000",
                       "statements":[{"name":"main","sql":"SELECT 1"}],
                       "rate_limit":{"per_client_qps":5}}"#;
        let err = serde_json::from_str::<CreateEndpointInput>(body).unwrap_err();
        assert!(
            err.to_string().contains("per_client_qps"),
            "the error must name the offending key: {err}"
        );
        // The PATCH body too, where a dropped key looks like "leave it alone".
        let err = serde_json::from_str::<UpdateEndpointInput>(r#"{"pth":"/a"}"#).unwrap_err();
        assert!(err.to_string().contains("pth"), "{err}");
        // Anti-triviality: the correctly-spelled bodies still deserialize.
        assert!(serde_json::from_str::<UpdateEndpointInput>(r#"{"path":"/a"}"#).is_ok());
    }

    /// Absent vs. explicit `null` must survive deserialization — this is the
    /// whole reason `folder_id` is a double option.
    #[test]
    fn an_absent_folder_id_is_not_an_explicit_null() {
        let absent: UpdateEndpointInput =
            serde_json::from_str(r#"{"statements":[{"name":"main","sql":"SELECT 1"}]}"#).unwrap();
        assert_eq!(absent.folder_id, None, "absent means leave it alone");

        let cleared: UpdateEndpointInput = serde_json::from_str(r#"{"folder_id":null}"#).unwrap();
        assert_eq!(
            cleared.folder_id,
            Some(None),
            "explicit null means clear it"
        );

        let set: UpdateEndpointInput =
            serde_json::from_str(r#"{"folder_id":"00000000-0000-0000-0000-000000000001"}"#)
                .unwrap();
        assert!(matches!(set.folder_id, Some(Some(_))));
    }
}
