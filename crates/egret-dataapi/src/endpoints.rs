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
//! One thing editing CAN still do to a live endpoint: move it. `path` and
//! `method` live on this row and nothing below refuses a PATCH because
//! `state = 'live'`. That is now a decided contract rather than an open question
//! — a URL belongs to the endpoint object, behaviour belongs to the frozen
//! version. See `lifecycle.rs`' module comment for what P4 must do with it.
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
use egret_dataapi_api::{CacheSpec, LimitsSpec, OutputSpec, ParamSpec, RateLimitSpec};
use egret_metastore::Db;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

/// The projection every read in this module shares. A macro rather than a
/// `const` so it can be `concat!`-ed into `&'static str` literals — sqlx 0.9's
/// `query`/`query_as` take `&'static str`, and a `format!`-ed statement would
/// have to go through `AssertSqlSafe`.
macro_rules! endpoint_cols {
    () => {
        "id, folder_id, name, description, path, method, database_id, sql, params, output, \
         cache, limits, rate_limit, source_kind, source_ref, live_version_id, state, \
         created_by, created_at, updated_at"
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
    sql: String,
    params: serde_json::Value,
    output: serde_json::Value,
    cache: serde_json::Value,
    limits: serde_json::Value,
    rate_limit: serde_json::Value,
    source_kind: String,
    source_ref: Option<Uuid>,
    live_version_id: Option<Uuid>,
    state: String,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
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
    pub sql: String,
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
    pub state: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
            sql: r.sql,
            params: spec("params", r.params)?,
            output: spec("output", r.output)?,
            cache: spec("cache", r.cache)?,
            limits: spec("limits", r.limits)?,
            rate_limit: spec("rate_limit", r.rate_limit)?,
            source_kind: r.source_kind,
            source_ref: r.source_ref,
            live_version_id: r.live_version_id,
            state: r.state,
            created_by: r.created_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

/// The `POST` body.
///
/// The spec fields are `Option<T>` rather than `#[serde(default)] T` because
/// `CacheSpec`/`LimitsSpec`/`RateLimitSpec` deliberately have no `Default` impl —
/// `OutputSpec`'s hand-written one exists precisely because a derive would have
/// diverged from serde's defaults. An absent field is stored as `'{}'::jsonb` and
/// read back through the same serde defaults, so there is exactly one definition
/// of "the default" and no Rust-side copy of it to drift.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateEndpointInput {
    pub name: String,
    /// Free-form and user-typed; normalised to a single leading slash and no
    /// trailing slash before it is stored. See [`normalize_path`].
    pub path: String,
    /// The connection this endpoint queries. Validated against the CALLER's
    /// workspace by the route before it is stored — it arrives from the request
    /// body, so nothing else would stop it naming another workspace's database.
    pub database_id: Uuid,
    pub sql: String,
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
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
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
    pub sql: Option<String>,
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
/// Folders of this type are not creatable yet: `egret-bi`'s generic folder
/// router maps a URL `kind` to `(entity_type, read_perm, manage_perm)` in
/// `kind_perms`, and has no entry for endpoints. Adding
/// `"data_api_endpoints" => ("data_api_endpoint", "data_api_endpoint:read",
/// "data_api_endpoint:manage")` there is what makes `folder_id` reachable from
/// the UI. The guard is written first on purpose — an unmatched rule is inert,
/// but a field that lands before its guard is a hole.
pub const FOLDER_ENTITY_TYPE: &str = "data_api_endpoint";

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

fn conflict(method: &str, path: &str) -> Error {
    Error::Conflict(format!(
        "an endpoint already exists at {method} {path} in this workspace"
    ))
}

impl EndpointStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every draft in `workspace`.
    pub async fn list(&self, workspace: Uuid) -> Result<Vec<DataApiEndpointDto>> {
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT ",
            endpoint_cols!(),
            " FROM dataapi.endpoints WHERE workspace_id = $1 ORDER BY created_at DESC"
        ))
        .bind(workspace)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(DataApiEndpointDto::try_from).collect()
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
    pub(crate) async fn set_state(
        &self,
        workspace: Uuid,
        id: Uuid,
        state: &'static str,
    ) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "UPDATE dataapi.endpoints SET state = $3, updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 RETURNING ",
            endpoint_cols!()
        ))
        .bind(id)
        .bind(workspace)
        .bind(state)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("endpoint".into()))?
            .try_into()
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
        let row: Row = sqlx::query_as(concat!(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, folder_id, name, description, path, method, database_id, sql, \
                params, output, cache, limits, rate_limit, source_kind, source_ref, created_by) \
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
        .bind(&input.sql)
        .bind(to_json(input.params.as_ref())?)
        .bind(to_json(input.output.as_ref())?)
        .bind(to_json(input.cache.as_ref())?)
        .bind(to_json(input.limits.as_ref())?)
        .bind(to_json(input.rate_limit.as_ref())?)
        .bind(input.source_kind.as_deref())
        .bind(input.source_ref)
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| {
            if is_path_conflict(&e) {
                conflict(method, path)
            } else {
                db_err(e)
            }
        })?;
        row.try_into()
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
               sql = COALESCE($7, sql), \
               params = COALESCE($8, params), \
               output = COALESCE($9, output), \
               cache = COALESCE($10, cache), \
               limits = COALESCE($11, limits), \
               rate_limit = COALESCE($12, rate_limit), \
               source_kind = COALESCE($13, source_kind), \
               folder_id = CASE WHEN $14 THEN $15 ELSE folder_id END, \
               source_ref = CASE WHEN $16 THEN $17 ELSE source_ref END, \
               updated_at = now() \
             WHERE id = $1 AND workspace_id = $18 \
             RETURNING ",
            endpoint_cols!()
        ))
        .bind(id)
        .bind(input.name.as_deref().map(str::trim))
        .bind(input.description.as_deref())
        .bind(input.path.as_deref())
        .bind(input.method.as_deref())
        .bind(input.database_id)
        .bind(input.sql.as_deref())
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
                return Err(conflict(&method, &path));
            }
            Err(e) => return Err(db_err(e)),
        };
        // `fetch_optional` rather than `fetch_one`: no row means either no such
        // endpoint or not this workspace's — the SAME answer, deliberately — and
        // also covers a concurrent delete, which must read as 404 rather than as
        // an internal error.
        row.ok_or_else(|| Error::NotFound("endpoint".into()))?
            .try_into()
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
    /// `dataapi.endpoint_versions` cascades from here (0121), so deleting an
    /// endpoint takes its published history with it — which is why the route
    /// gates this on `:manage` and nothing weaker, and why "deleted the wrong
    /// workspace's row" is not a recoverable mistake.
    pub async fn delete(&self, workspace: Uuid, id: Uuid) -> Result<()> {
        let affected =
            sqlx::query("DELETE FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2")
                .bind(id)
                .bind(workspace)
                .execute(&self.db.pool)
                .await
                .map_err(db_err)?
                .rows_affected();
        // Reported rather than swallowed: a DELETE that matched nothing is the
        // same event as a GET that found nothing, and must answer the same way.
        if affected == 0 {
            return Err(Error::NotFound("endpoint".into()));
        }
        Ok(())
    }
}

/// Serialize an optional spec for binding. `None` binds SQL NULL, which every
/// statement above `COALESCE`s to the column's existing value (update) or its
/// declared default (create).
fn to_json<T: Serialize>(v: Option<&T>) -> Result<Option<serde_json::Value>> {
    v.map(|v| {
        serde_json::to_value(v)
            .map_err(|e| Error::Internal(anyhow::anyhow!("serialize endpoint spec: {e}")))
    })
    .transpose()
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

    /// Absent vs. explicit `null` must survive deserialization — this is the
    /// whole reason `folder_id` is a double option.
    #[test]
    fn an_absent_folder_id_is_not_an_explicit_null() {
        let absent: UpdateEndpointInput = serde_json::from_str(r#"{"sql":"SELECT 1"}"#).unwrap();
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
