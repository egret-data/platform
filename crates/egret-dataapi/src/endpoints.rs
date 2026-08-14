//! Data API endpoint DRAFTS: the row an author edits, and the store behind it.
//!
//! This module owns the draft half of the split migration 0121 describes.
//! Publishing — freezing a draft into an immutable `dataapi.endpoint_versions`
//! snapshot — is a separate lifecycle and lives elsewhere; nothing here writes
//! `state` or `live_version_id`, so no amount of editing can put an endpoint
//! live or change what an already-published version serves.
//!
//! **Workspace containment lives in the ROUTES, not here.** `get`, `update` and
//! `delete` take an id that the caller has already resolved through
//! `routes::scoped_endpoint`, exactly as `ClientStore::revoke` takes an id
//! already resolved through `scoped_account`. Duplicating the `workspace_id`
//! predicate into these statements would look safer and be worse: the route's
//! guard would then be untestable — a cross-workspace test would stay green
//! after the guard was deleted, because the store had silently covered for it.
//! One place enforces it, and a test can prove that place is load-bearing.

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

/// Canonicalise a user-typed URL path: one leading slash, no trailing slash.
///
/// This is not cosmetic. `UNIQUE (workspace_id, method, path)` compares TEXT, so
/// `reports/revenue`, `/reports/revenue` and `/reports/revenue/` are three
/// distinct rows to Postgres and one route to a client. Without canonicalising
/// first, the constraint that is supposed to stop two endpoints claiming the same
/// URL would admit all three, and the collision would only appear when the
/// serving path had to pick one.
///
/// Returns `None` for anything with no path left after trimming — `""`, `"   "`,
/// `"/"` — which the route turns into a 400.
pub fn normalize_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("/{trimmed}"))
}

/// Persistence for endpoint drafts.
#[derive(Clone)]
pub struct EndpointStore {
    db: Db,
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

    /// Every draft in `workspace`. The only method that takes a workspace for a
    /// READ, because it is the only one with no id to have been resolved first.
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

    /// One draft by id. The id must already have been through
    /// `routes::scoped_endpoint` — see the module comment.
    pub async fn get(&self, id: Uuid) -> Result<DataApiEndpointDto> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            endpoint_cols!(),
            " FROM dataapi.endpoints WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("endpoint".into()))?
            .try_into()
    }

    /// Insert a draft. `path` must already be normalised and `database_id`
    /// already confirmed to belong to `workspace`; both are the route's job,
    /// because both need to answer with an HTTP status this layer does not pick.
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
    /// `path` (if present) must already be normalised, and `database_id` (if
    /// present) already confirmed to belong to the caller's workspace.
    pub async fn update(
        &self,
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
             WHERE id = $1 \
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
        .fetch_optional(&self.db.pool)
        .await;
        // A `match`, not `map_err`: naming the colliding URL needs a second query,
        // and after a partial PATCH that URL is half in the request and half in
        // the row — so the conflict message can only be built by awaiting.
        let row = match res {
            Ok(row) => row,
            Err(e) if is_path_conflict(&e) => {
                let (method, path) = self.effective_route(id, input).await;
                return Err(conflict(&method, &path));
            }
            Err(e) => return Err(db_err(e)),
        };
        // `fetch_optional` rather than `fetch_one`: the row was resolved by
        // `scoped_endpoint` a moment ago, but a concurrent delete between the two
        // statements must read as 404, not as an internal error.
        row.ok_or_else(|| Error::NotFound("endpoint".into()))?
            .try_into()
    }

    /// The effective method and path an update WOULD have written, for the
    /// conflict message.
    async fn effective_route(&self, id: Uuid, input: &UpdateEndpointInput) -> (String, String) {
        let current: Option<(String, String)> =
            sqlx::query_as("SELECT method, path FROM dataapi.endpoints WHERE id = $1")
                .bind(id)
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

    /// Delete a draft. The id must already have been through
    /// `routes::scoped_endpoint`.
    ///
    /// `dataapi.endpoint_versions` cascades from here (0121), so deleting an
    /// endpoint takes its published history with it — which is why the route
    /// gates this on `:manage` and nothing weaker.
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
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
        assert_eq!(
            normalize_path("reports/revenue").as_deref(),
            Some("/reports/revenue")
        );
        assert_eq!(
            normalize_path("/reports/revenue").as_deref(),
            Some("/reports/revenue")
        );
        assert_eq!(
            normalize_path("/reports/revenue/").as_deref(),
            Some("/reports/revenue")
        );
        assert_eq!(
            normalize_path("  reports/revenue  ").as_deref(),
            Some("/reports/revenue")
        );
    }

    /// The empty forms must be rejected rather than normalised into `"/"` — an
    /// endpoint at the workspace root is not a thing an author can have meant,
    /// and `""` would collide with every other empty path in the workspace.
    #[test]
    fn a_path_with_nothing_in_it_is_rejected() {
        assert_eq!(normalize_path(""), None);
        assert_eq!(normalize_path("   "), None);
        assert_eq!(normalize_path("/"), None);
        assert_eq!(normalize_path("///"), None);
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
