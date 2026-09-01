//! `workflow.flows` + `workflow.flow_versions`: the draft/version split migration
//! 0132 describes.
//!
//! Draft and version are two tables for the same reason `dataapi.endpoints` /
//! `dataapi.endpoint_versions` (0121) are: if execution read the draft,
//! editing a flow would change the behaviour of tonight's scheduled run, and
//! "versioning" would be decoration. Nothing in `egret-workflow` executes yet
//! (that is W2+), but the split is load-bearing from the first row, because
//! Task 6 (publish) freezes a version against this shape and Task 7 (routes)
//! serves drafts and versions through two different paths.
//!
//! **Workspace containment lives HERE, in exactly one place**, the same
//! convention `crates/egret-dataapi/src/endpoints.rs` documents: every
//! statement that reads or writes a *draft* (`workflow.flows`) carries
//! `WHERE id = $1 AND workspace_id = $2`, and there is no route-level
//! pre-check to forget.
//!
//! **The following methods are deliberate exceptions** -- each keys off a
//! bare id with no `workspace_id` predicate of its own, and each says so on
//! its own doc comment below. No count is kept here on purpose (Task 6 fix
//! round 3): a number maintained above the list it describes drifts every
//! time the list changes and nothing catches it: the list below is the
//! source of truth, so add a bullet, not a number, when this set changes
//! again. (Task 6 fix round 2, Finding 2: [`FlowRepo::list_versions`]
//! and [`FlowRepo::get_version`] used to be two more, undisclosed exceptions
//! -- unlike the writes below, they were **reads**, and an unscoped read
//! handed a foreign flow id returns that flow's version snapshots and notes
//! with no error at all, the exact shape migration 0128 is this programme's
//! recorded case of. "Nothing calls them yet, so the obligation stays with
//! the first caller" was the reasoning that kept them unscoped one fix round
//! ago; overruled, because a doc sentence must follow the code, not
//! constrain it -- both now take `ws` and join `workflow.flows` to check it,
//! same as every other read in this module, so there is no obligation left
//! for a future caller to remember.)
//!
//! - [`FlowRepo::workspace_of`] -- the scoping PRIMITIVE itself; a caller
//!   asks this before it knows which workspace an id belongs to, so it
//!   cannot also take `ws`.
//! - [`FlowRepo::insert_version`] and [`FlowRepo::set_live`] -- writes whose
//!   callers (Task 6's `publish`) have already confirmed containment via a
//!   prior scoped [`FlowRepo::get`] before either is reached.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result, UserId};
use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// The projection every read of `workflow.flows` shares.
macro_rules! flow_cols {
    () => {
        "id, workspace_id, folder_id, namespace, flow_id, source, plan, state, \
         live_version_id, created_by, created_at, updated_at, deleted_at"
    };
}

/// A flow draft row, as stored.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FlowRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub folder_id: Option<Uuid>,
    pub namespace: String,
    pub flow_id: String,
    /// The YAML the author wrote, byte for byte.
    pub source: String,
    /// The parsed, normalised model -- what `spec::validate` and (later,
    /// W3+) execution read. Round-trips through `models::Flow`'s hand-written
    /// `Serialize`/`Deserialize` pair (Task 3), which is why it is safe to
    /// store as `jsonb` rather than re-parsing `source` on every read.
    pub plan: serde_json::Value,
    pub state: String,
    pub live_version_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// A frozen, immutable published version.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FlowVersionRow {
    pub id: Uuid,
    pub flow_id: Uuid,
    pub version: i32,
    /// Source + normalised plan + resolved references, self-contained --
    /// Task 6's job to fill in. This repository only stores and returns
    /// whatever `snapshot` it is given.
    pub snapshot: serde_json::Value,
    pub note: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// Input to [`FlowRepo::create`].
#[derive(Debug, Clone)]
pub struct CreateFlowInput {
    pub namespace: String,
    /// The YAML's `id`. Named `flow_id` here (not `id`) because the
    /// surrogate key already owns that name -- same reasoning migration 0132
    /// records for the column.
    pub flow_id: String,
    pub source: String,
    pub plan: serde_json::Value,
    pub folder_id: Option<Uuid>,
}

/// Input to [`FlowRepo::update`]. Every field is optional -- absent means
/// "leave as is" -- except `folder_id`, which is `Option<Option<Uuid>>` so a
/// caller can distinguish "don't touch it" from "clear it", the same
/// double-option shape `UpdateEndpointInput` uses.
#[derive(Debug, Clone, Default)]
pub struct UpdateFlowInput {
    pub namespace: Option<String>,
    pub flow_id: Option<String>,
    pub source: Option<String>,
    pub plan: Option<serde_json::Value>,
    pub folder_id: Option<Option<Uuid>>,
}

/// Filters for [`FlowRepo::list`]. `trashed = false` (the default) lists only
/// `deleted_at IS NULL` rows; `true` lists only the trashed ones -- the same
/// partition `dataapi.endpoints`'s list uses, so Trash is a separate screen
/// fed by the same query and no list can mix a trashed row in with a live one.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub folder_id: Option<Uuid>,
    pub namespace: Option<String>,
    pub state: Option<String>,
    pub trashed: bool,
}

/// Persistence for flow drafts and their published versions.
#[derive(Clone)]
pub struct FlowRepo {
    db: Db,
}

impl FlowRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert a draft.
    pub async fn create(
        &self,
        workspace: Uuid,
        by: UserId,
        input: &CreateFlowInput,
    ) -> Result<FlowRow> {
        sqlx::query_as(concat!(
            "INSERT INTO workflow.flows \
               (workspace_id, folder_id, namespace, flow_id, source, plan, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING ",
            flow_cols!()
        ))
        .bind(workspace)
        .bind(input.folder_id)
        .bind(&input.namespace)
        .bind(&input.flow_id)
        .bind(&input.source)
        .bind(&input.plan)
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// One draft, if it belongs to `workspace`. `NotFound` otherwise -- see
    /// the module comment on why the predicate is here and not in the caller.
    pub async fn get(&self, ws: Uuid, id: Uuid) -> Result<FlowRow> {
        let row: Option<FlowRow> = sqlx::query_as(concat!(
            "SELECT ",
            flow_cols!(),
            " FROM workflow.flows WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("flow".into()))
    }

    /// Every draft in `workspace` matching `filter`, newest first.
    pub async fn list(&self, ws: Uuid, filter: &ListFilter) -> Result<Vec<FlowRow>> {
        sqlx::query_as(concat!(
            "SELECT ",
            flow_cols!(),
            " FROM workflow.flows \
             WHERE workspace_id = $1 \
               AND ($2::uuid IS NULL OR folder_id = $2) \
               AND ($3::text IS NULL OR namespace = $3) \
               AND ($4::text IS NULL OR state = $4) \
               AND $5 = (deleted_at IS NOT NULL) \
             ORDER BY created_at DESC"
        ))
        .bind(ws)
        .bind(filter.folder_id)
        .bind(&filter.namespace)
        .bind(&filter.state)
        .bind(filter.trashed)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Patch a draft. `NotFound` covers both "no such flow" and "not this
    /// workspace's" -- the same answer, deliberately, per the module comment.
    ///
    /// `by` is accepted for interface parity with [`Self::create`] and
    /// because a future audit trail on drafts (there is none yet -- the
    /// schema has no `updated_by`) will need it; unused today.
    pub async fn update(
        &self,
        ws: Uuid,
        id: Uuid,
        by: UserId,
        input: &UpdateFlowInput,
    ) -> Result<FlowRow> {
        let _ = by;
        let row: Option<FlowRow> = sqlx::query_as(concat!(
            "UPDATE workflow.flows SET \
               namespace = COALESCE($3, namespace), \
               flow_id   = COALESCE($4, flow_id), \
               source    = COALESCE($5, source), \
               plan      = COALESCE($6, plan), \
               folder_id = CASE WHEN $7 THEN $8 ELSE folder_id END, \
               updated_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL \
             RETURNING ",
            flow_cols!()
        ))
        .bind(id)
        .bind(ws)
        .bind(&input.namespace)
        .bind(&input.flow_id)
        .bind(&input.source)
        .bind(&input.plan)
        .bind(input.folder_id.is_some())
        .bind(input.folder_id.flatten())
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("flow".into()))
    }

    /// Soft delete: `deleted_at = now()`. Idempotent-in-effect -- deleting an
    /// already-trashed flow (or one that never existed in this workspace)
    /// still answers `NotFound` rather than silently doing nothing, so a
    /// caller cannot mistake "already gone" for "just deleted".
    pub async fn soft_delete(&self, ws: Uuid, id: Uuid) -> Result<()> {
        let updated: Option<Uuid> = sqlx::query_scalar(
            "UPDATE workflow.flows SET deleted_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL \
             RETURNING id",
        )
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        updated
            .map(|_| ())
            .ok_or_else(|| Error::NotFound("flow".into()))
    }

    /// Freeze a new immutable version.
    ///
    /// The version number is computed in the SAME statement as the insert --
    /// `INSERT ... SELECT $1, COALESCE(MAX(version), 0) + 1, ... FROM
    /// workflow.flow_versions WHERE flow_id = $1`, not a prior `SELECT` --
    /// following `EndpointStore::publish` in
    /// `crates/egret-dataapi/src/lifecycle.rs`.
    ///
    /// **What this does and does not buy.** `UNIQUE (flow_id, version)` is
    /// what actually guarantees no two committed rows can ever share a
    /// version number (or leave a gap) -- that holds regardless of whether
    /// the next number is computed here or in a prior, separate `SELECT`; a
    /// two-round-trip `insert_version` would be equally correct in that
    /// sense, and fix round 1 of Task 5 confirmed there is no version-race
    /// *correctness* bug in either shape. What the single statement buys is
    /// concurrency THROUGHPUT: it holds one pooled connection for the whole
    /// read-then-write, so racers are naturally staggered through the
    /// connection pool instead of all reading the same stale `MAX` before any
    /// of them commit. Measured against this crate's tests
    /// (`concurrent_publishes_do_not_collide_on_a_version_number`, 5-connection
    /// pool, 8 concurrent callers): this shape lets roughly half of the 8
    /// succeed on the first attempt, every run; a genuine two-round-trip
    /// rewrite let only about a quarter through, every run -- both numbers
    /// recorded in `task-5-report.md`, "Fix round 1". A loser still returns
    /// `Err` either way; this function does not retry it, and neither does
    /// the caller -- publish (Task 6) decides what a failed publish means,
    /// not this repository.
    pub async fn insert_version(
        &self,
        flow: Uuid,
        by: UserId,
        snapshot: &serde_json::Value,
        note: &str,
    ) -> Result<FlowVersionRow> {
        sqlx::query_as(
            "INSERT INTO workflow.flow_versions (flow_id, version, snapshot, note, created_by) \
             SELECT $1, COALESCE(MAX(version), 0) + 1, $2, $3, $4 \
               FROM workflow.flow_versions WHERE flow_id = $1 \
             RETURNING id, flow_id, version, snapshot, note, created_by, created_at",
        )
        .bind(flow)
        .bind(snapshot)
        .bind(note)
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Point `flows.live_version_id` at `version` and mark the flow `live`.
    /// Unscoped by workspace, like `EndpointStore::set_state` -- the caller
    /// (Task 6's publish) has already resolved and locked `flow` in its own
    /// workspace before calling this.
    pub async fn set_live(&self, flow: Uuid, version: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE workflow.flows SET live_version_id = $2, state = 'live', updated_at = now() \
             WHERE id = $1",
        )
        .bind(flow)
        .bind(version)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Every version of `flow` in `ws`, newest first -- the index migration
    /// 0132 adds (`flow_id, version DESC`) is for exactly this read.
    ///
    /// SCOPED by `ws` since Task 6 fix round 2, Finding 2, via a join against
    /// `workflow.flows` -- `flow_versions` carries no `workspace_id` column
    /// of its own, so there is no bare predicate to add the way
    /// [`FlowRepo::get`] has one; the join is what a bare predicate would
    /// have been. A foreign `flow` (one that belongs to a different
    /// workspace) and a `flow` that does not exist at all both answer an
    /// empty `Vec`, indistinguishably -- neither joins a `workflow.flows` row
    /// in `ws`.
    ///
    /// Previously unscoped, with the obligation to check left to the first
    /// caller (Task 7's route handlers). Overruled in fix round 2: a doc
    /// sentence describing an obligation is weaker than closing it in the
    /// query, so nobody has to remember it.
    pub async fn list_versions(&self, ws: Uuid, flow: Uuid) -> Result<Vec<FlowVersionRow>> {
        sqlx::query_as(
            "SELECT v.id, v.flow_id, v.version, v.snapshot, v.note, v.created_by, v.created_at \
             FROM workflow.flow_versions v \
             JOIN workflow.flows f ON f.id = v.flow_id \
             WHERE v.flow_id = $1 AND f.workspace_id = $2 \
             ORDER BY v.version DESC",
        )
        .bind(flow)
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// One version of `flow` in `ws`, by its per-flow number.
    ///
    /// SCOPED by `ws`, same join and reasoning as [`Self::list_versions`]. A
    /// foreign `flow`/`version` pair and a pair that does not exist at all
    /// both answer `NotFound`, indistinguishably.
    pub async fn get_version(&self, ws: Uuid, flow: Uuid, version: i32) -> Result<FlowVersionRow> {
        let row: Option<FlowVersionRow> = sqlx::query_as(
            "SELECT v.id, v.flow_id, v.version, v.snapshot, v.note, v.created_by, v.created_at \
             FROM workflow.flow_versions v \
             JOIN workflow.flows f ON f.id = v.flow_id \
             WHERE v.flow_id = $1 AND v.version = $2 AND f.workspace_id = $3",
        )
        .bind(flow)
        .bind(version)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("flow version".into()))
    }

    /// The workspace `id` belongs to, or `Ok(None)` if no such row exists at
    /// all -- trashed or live, both answer with their real workspace, since
    /// `deleted_at` plays no part in this query.
    ///
    /// UNSCOPED by design, unlike every other read in this module: this is
    /// the scoping obligation every port in this crate carries, per
    /// `egret_dataapi::endpoints::EndpointStore::workspace_of`'s own doc --
    /// answer with the object's OWN workspace so the CALLER can compare and
    /// refuse a mismatch as `NotFound`, indistinguishable from "does not
    /// exist". A `subflow:` reference (spec §8) reaches this before its
    /// caller's workspace is known to match.
    pub async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>> {
        sqlx::query_scalar("SELECT workspace_id FROM workflow.flows WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(db_err)
    }
}
