//! `workflow.flows` + `workflow.flow_versions`: the draft/version split migration
//! 0132 describes -- and, since W10 Task 3, [`ExecutionRepo`]: the READ layer
//! over `workflow.executions` + `workflow.task_runs` (migration 0134). The two
//! live in one module because they share one rule, stated once below and
//! restated on [`ExecutionRepo`] itself: every statement carries a
//! `workspace_id` predicate. [`ExecutionRepo`] writes nothing -- the writes
//! belong to `worker::claim`, which holds the `SELECT ... FOR NO KEY UPDATE` this
//! module deliberately does not.
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

/// The 409 [`FlowRepo::update`] answers when its `if_updated_at` no longer
/// matches the row (W7a spec §7.3, verbatim).
pub const DRAFT_CHANGED: &str = "the draft was changed since you opened it";

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// `None` unless `e` is the `UNIQUE (workspace_id, namespace, flow_id)`
/// violation (migration 0132). Every other unique constraint on this table
/// is the primary key, which a fresh `gen_random_uuid()` cannot collide
/// with -- so this generic `is_unique_violation()` check is unambiguous,
/// same reasoning `egret-dataapi`'s identically-named `is_path_conflict`
/// gives for `dataapi.endpoints`.
///
/// Whole-branch review, IMPORTANT finding 3: before this, EVERY unique
/// violation on this table mapped through [`db_err`] to `Error::Internal` --
/// a bare 500 `{"error":"internal error"}` -- even though `openapi.json`
/// documents a `409` on `POST /flows` and `PUT /flows/{id}`. Measured:
/// creating a second flow with the same `(namespace, flow_id)` was a 500;
/// recreating one after soft-deleting the original (which still holds the
/// name -- `UNIQUE` has no `WHERE deleted_at IS NULL`, the same choice
/// `dataapi.endpoints`' identical constraint makes) was ALSO a 500, with no
/// way for an author to see why. Ported the resolution shape from
/// `crates/egret-dataapi/src/endpoints.rs`'s `is_path_conflict` +
/// `conflict()`/`trashed_conflict()` pair.
fn is_namespace_conflict(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

/// The `UNIQUE (workspace_id, namespace, flow_id)` refusal, as an author
/// reads it. Names the identity rather than the constraint -- a bare
/// "conflict" on a screen with a dozen drafts is not actionable.
fn conflict(namespace: &str, flow_id: &str) -> Error {
    Error::Conflict(format!(
        "a flow already exists at {namespace}/{flow_id} in this workspace"
    ))
}

/// The same refusal when the row holding the name is IN THE TRASH -- the
/// case a bare `db_err` could never distinguish from a live collision, and
/// the one this fix EXISTS to name: a soft-deleted flow keeps its identity,
/// so the author is told why, rather than reading an opaque 500 that gives
/// no hint the name is burned by a row they cannot see on any list.
///
/// Names both ways out, as `egret-dataapi`'s `trashed_conflict` does: the
/// trash can be listed (`GET /flows?trashed=true`), and a trashed flow can be
/// restored (`POST /flows/{id}/restore`) or deleted permanently
/// (`DELETE /flows/{id}/permanent`), which frees the name (KI-119).
fn trashed_conflict(namespace: &str, flow_id: &str) -> Error {
    Error::Conflict(format!(
        "a flow in the trash still holds {namespace}/{flow_id} in this workspace; \
         restore it, or delete it permanently to free the name"
    ))
}

/// The projection every read of `workflow.flows` shares.
macro_rules! flow_cols {
    () => {
        "id, workspace_id, folder_id, namespace, flow_id, source, plan, state, \
         live_version_id, created_by, created_at, updated_at, deleted_at"
    };
}
// `scheduler::fire` re-reads a whole `FlowRow` to re-run `sync`; sharing the
// projection keeps that read from drifting from every other one.
pub(crate) use flow_cols;

/// The creator's display name, for a row `r` read in a CTE: the profile
/// name, else the email -- the same expression `DashboardRepo::attribution_names`
/// uses (`egret-bi`). `LEFT JOIN`, so a NULL `created_by` or a user who no
/// longer exists answers NULL rather than dropping the row (W7a spec §7.4).
///
/// Every route that answers a `FlowDto`, `FlowListItemDto` or
/// `FlowVersionDto` reads its row through this, in the SAME statement that
/// reads (or writes) the row: the user-listing endpoints are admin-gated, so
/// the browser cannot resolve the id itself for an ordinary editor.
macro_rules! with_creator_name {
    ($cte:expr, $tail:expr) => {
        concat!(
            "WITH r AS (",
            $cte,
            ") SELECT r.*, COALESCE(NULLIF(u.name, ''), u.email::text) AS created_by_name \
             FROM r LEFT JOIN users u ON u.id = r.created_by",
            $tail
        )
    };
}

/// [`with_creator_name!`] for a FLOW row, plus its latest version number
/// (W7a spec §4, §5.3; the user's ruling of 2026-10-10 on the flow-state
/// chip): `MAX(version)` of its `flow_versions`, NULL when it has none. In the
/// same statement, so the number a list or header shows is the one this read
/// saw. The version reads keep [`with_creator_name!`]: their `r` is a version
/// row, whose `id` is not a flow's.
macro_rules! with_flow_names {
    ($cte:expr, $tail:expr) => {
        concat!(
            "WITH r AS (",
            $cte,
            ") SELECT r.*, COALESCE(NULLIF(u.name, ''), u.email::text) AS created_by_name, \
               (SELECT MAX(v.version) FROM workflow.flow_versions v WHERE v.flow_id = r.id) AS latest_version \
             FROM r LEFT JOIN users u ON u.id = r.created_by",
            $tail
        )
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
    /// The creator's display name (W7a spec §7.4). Filled by the reads that
    /// answer a route (`get`, `list`, `create`, `update`); `None` from the
    /// internal reads that only feed the scheduler, which never select it --
    /// hence `#[sqlx(default)]`.
    #[sqlx(default)]
    pub created_by_name: Option<String>,
    /// The flow's highest version number, `None` with no version (W7a spec §4,
    /// §5.3). Filled by the same reads as [`Self::created_by_name`]
    /// ([`with_flow_names!`]); `None` from the internal reads, hence
    /// `#[sqlx(default)]`.
    #[sqlx(default)]
    pub latest_version: Option<i32>,
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
    /// The publisher's display name (W7a spec §7.4); see
    /// [`FlowRow::created_by_name`].
    #[sqlx(default)]
    pub created_by_name: Option<String>,
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
    /// Write only if the row's `updated_at` is still this (W7a spec §7.3).
    /// `None`: unconditional, as before.
    pub if_updated_at: Option<chrono::DateTime<chrono::Utc>>,
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

/// One namespace of a workspace (W7d spec §3.1). egret has no namespace
/// table: a namespace is derived from what lives in it, see
/// [`NAMESPACES_SQL`].
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct NamespaceRow {
    pub namespace: String,
    /// This namespace's OWN counts -- never rolled up from its children.
    pub flows: i64,
    pub files: i64,
    pub kv_keys: i64,
    /// False when the namespace is present only as a parent by dots of one
    /// that is (its three counts are then all zero). True for a source, whose
    /// counts can still all be zero (a live empty directory).
    pub own: bool,
}

/// The namespaces of workspace `$1`, in one statement (W7d spec §3.1).
///
/// Three sources, each scoped to the workspace:
/// - flows not deleted (`deleted_at IS NULL`);
/// - live namespace entries: a path's state is its HIGHEST revision's row
///   (taken first, then judged: filtering `deleted` first would resurrect an
///   older revision), live when not `deleted`. A live DIRECTORY, even an
///   empty one, makes its namespace a source (a folder created in a new
///   namespace must not vanish from the list), but `files` counts files only;
/// - live KV keys: the highest revision's row, live when not `deleted` and
///   not expired (`expires_at <= now()`, the database's clock), as
///   `storage::kv::store`'s `list_live` judges it.
///
/// A source is `own` (its counts may all be zero: a live empty directory);
/// every parent by dots of one is added with `own = false` unless it is a
/// source itself.
///
/// Cost: linear in the workspace's revision rows of the three tables (or,
/// with `$2`, those of the prefix subtree) -- every revision of every path
/// and key is read to find the latest; there is no namespace index.
///
/// `$2` is the dot-anchored prefix (`a.b` matches `a.b` and `a.b.c`, never
/// `a.bc`; `starts_with`, not `LIKE`, as W7b's `namespace_prefix`), or NULL.
/// It is applied to the sources too: a namespace is in `$2`'s subtree only
/// if some source beneath it is, so the narrowing loses nothing. `$3` keeps
/// only the row named `$2` exactly (the detail route).
pub const NAMESPACES_SQL: &str = "\
WITH flows AS ( \
  SELECT namespace, count(*) AS flows, 0::bigint AS files, 0::bigint AS kv_keys \
    FROM workflow.flows \
   WHERE workspace_id = $1 AND deleted_at IS NULL \
     AND ($2::text IS NULL OR namespace = $2 OR starts_with(namespace, $2 || '.')) \
   GROUP BY namespace), \
files AS ( \
  SELECT namespace, 0::bigint AS flows, count(*) FILTER (WHERE NOT is_directory) AS files, \
         0::bigint AS kv_keys \
    FROM (SELECT DISTINCT ON (namespace, path) namespace, is_directory, deleted \
            FROM workflow.namespace_files \
           WHERE workspace_id = $1 \
             AND ($2::text IS NULL OR namespace = $2 OR starts_with(namespace, $2 || '.')) \
           ORDER BY namespace, path, revision DESC) latest \
   WHERE NOT deleted \
   GROUP BY namespace), \
keys AS ( \
  SELECT namespace, 0::bigint AS flows, 0::bigint AS files, count(*) AS kv_keys \
    FROM (SELECT DISTINCT ON (namespace, key) namespace, deleted, \
                 (expires_at IS NOT NULL AND expires_at <= now()) AS expired \
            FROM workflow.kv_entries \
           WHERE workspace_id = $1 \
             AND ($2::text IS NULL OR namespace = $2 OR starts_with(namespace, $2 || '.')) \
           ORDER BY namespace, key, revision DESC) latest \
   WHERE NOT deleted AND NOT expired \
   GROUP BY namespace), \
own AS ( \
  SELECT namespace, sum(flows)::bigint AS flows, sum(files)::bigint AS files, \
         sum(kv_keys)::bigint AS kv_keys \
    FROM (SELECT * FROM flows UNION ALL SELECT * FROM files UNION ALL SELECT * FROM keys) s \
   GROUP BY namespace), \
every AS ( \
  SELECT DISTINCT array_to_string(parts[1:n], '.') AS namespace \
    FROM own, string_to_array(own.namespace, '.') AS parts, \
         generate_series(1, cardinality(parts)) AS n) \
SELECT e.namespace, coalesce(o.flows, 0) AS flows, coalesce(o.files, 0) AS files, \
       coalesce(o.kv_keys, 0) AS kv_keys, o.namespace IS NOT NULL AS own \
  FROM every e LEFT JOIN own o ON o.namespace = e.namespace \
 WHERE ($2::text IS NULL OR e.namespace = $2 OR starts_with(e.namespace, $2 || '.')) \
   AND (NOT $3 OR e.namespace = $2) \
 ORDER BY e.namespace COLLATE \"C\"";

impl FlowRepo {
    /// Every namespace of `workspace` (under `prefix`, dot-anchored), by
    /// namespace in byte order. See [`NAMESPACES_SQL`].
    pub async fn namespaces(
        &self,
        workspace: Uuid,
        prefix: Option<&str>,
    ) -> Result<Vec<NamespaceRow>> {
        sqlx::query_as(NAMESPACES_SQL)
            .bind(workspace)
            .bind(prefix)
            .bind(false)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// One namespace of `workspace`: a source or a parent by dots of one,
    /// or `None`.
    pub async fn namespace(
        &self,
        workspace: Uuid,
        namespace: &str,
    ) -> Result<Option<NamespaceRow>> {
        sqlx::query_as(NAMESPACES_SQL)
            .bind(workspace)
            .bind(namespace)
            .bind(true)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// Every flow of `workspace` that is not deleted, by namespace then flow
    /// id (byte order), at most `limit` rows -- what the namespace
    /// dependencies graph reads (W7d spec §3.4). The caller passes its
    /// limit plus one to tell "exactly the limit" from "over it".
    pub async fn dependency_drafts(
        &self,
        workspace: Uuid,
        limit: i64,
    ) -> Result<Vec<crate::namespaces::dependencies::DraftRow>> {
        sqlx::query_as(
            "SELECT id, namespace, flow_id, source, state FROM workflow.flows \
              WHERE workspace_id = $1 AND deleted_at IS NULL \
              ORDER BY namespace COLLATE \"C\", flow_id COLLATE \"C\" \
              LIMIT $2",
        )
        .bind(workspace)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// The flows of `workspace` not deleted whose triggers the workspace
    /// trigger list reads (W7e spec §3.5, D9): the namespace filter
    /// dot-anchored (`a.b` is `a.b` and `a.b.c`, never `a.bc`; `starts_with`,
    /// not `LIKE`, as W7b's `namespace_prefix`) and the flow filter in SQL, by
    /// namespace then flow id (byte order), at most `limit` rows. The caller
    /// passes its bound plus one, so the bound applies AFTER the filters.
    pub async fn trigger_drafts(
        &self,
        workspace: Uuid,
        namespace_prefix: Option<&str>,
        flow: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<crate::namespaces::dependencies::DraftRow>> {
        sqlx::query_as(
            "SELECT id, namespace, flow_id, source, state FROM workflow.flows \
              WHERE workspace_id = $1 AND deleted_at IS NULL \
                AND ($2::text IS NULL OR namespace = $2 OR starts_with(namespace, $2 || '.')) \
                AND ($3::uuid IS NULL OR id = $3) \
              ORDER BY namespace COLLATE \"C\", flow_id COLLATE \"C\" \
              LIMIT $4",
        )
        .bind(workspace)
        .bind(namespace_prefix)
        .bind(flow)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }
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
        // W12: the insert and `scheduler::sync` commit together, so a flow
        // never exists without the trigger rows its YAML declares.
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let row: std::result::Result<FlowRow, sqlx::Error> = sqlx::query_as(with_flow_names!(
            concat!(
                "INSERT INTO workflow.flows \
                   (workspace_id, folder_id, namespace, flow_id, source, plan, created_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 RETURNING ",
                flow_cols!()
            ),
            ""
        ))
        .bind(workspace)
        .bind(input.folder_id)
        .bind(&input.namespace)
        .bind(&input.flow_id)
        .bind(&input.source)
        .bind(&input.plan)
        .bind(by.0)
        .fetch_one(&mut *tx)
        .await;
        // A `match` rather than `map_err`: telling a trashed collision apart
        // from a visible one needs a second query, and a query can only be
        // awaited -- same reasoning `EndpointStore::create` gives for the
        // identical shape.
        //
        // The failed statement has aborted `tx`, so it is rolled back (dropped)
        // before that second query, which stays on the pool.
        let row = match row {
            Ok(row) => row,
            Err(e) if is_namespace_conflict(&e) => {
                drop(tx);
                return Err(self
                    .namespace_conflict(workspace, &input.namespace, &input.flow_id)
                    .await);
            }
            Err(e) => return Err(db_err(e)),
        };
        crate::scheduler::sync(&mut tx, &row, false).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(row)
    }

    /// Which conflict this is: a visible flow holding the name, or a
    /// trashed one the author cannot see. Called only after
    /// [`is_namespace_conflict`] has already established that the `UNIQUE`
    /// fired, so the row it looks for is there by construction. If it
    /// somehow is not -- the holder was deleted between the failed write
    /// and this read -- the answer falls back to [`conflict`], the message
    /// that was correct before trash existed; a missing row must never
    /// become "a trashed flow holds it", which would send the author
    /// looking for a screen that shows nothing.
    async fn namespace_conflict(&self, workspace: Uuid, namespace: &str, flow_id: &str) -> Error {
        let holder: std::result::Result<Option<Option<DateTime<Utc>>>, sqlx::Error> =
            sqlx::query_scalar(
                "SELECT deleted_at FROM workflow.flows \
                 WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3",
            )
            .bind(workspace)
            .bind(namespace)
            .bind(flow_id)
            .fetch_optional(&self.db.pool)
            .await;
        match holder {
            Ok(Some(Some(_))) => trashed_conflict(namespace, flow_id),
            _ => conflict(namespace, flow_id),
        }
    }

    /// One draft, if it belongs to `workspace`. `NotFound` otherwise -- see
    /// the module comment on why the predicate is here and not in the caller.
    pub async fn get(&self, ws: Uuid, id: Uuid) -> Result<FlowRow> {
        Self::get_on(&self.db.pool, ws, id).await
    }

    /// [`Self::get`] on a given executor: a transaction reads the draft in
    /// its own snapshot, with the state it reads next.
    pub async fn get_on<'e, E: sqlx::PgExecutor<'e>>(
        executor: E,
        ws: Uuid,
        id: Uuid,
    ) -> Result<FlowRow> {
        let row: Option<FlowRow> = sqlx::query_as(with_flow_names!(
            concat!(
                "SELECT ",
                flow_cols!(),
                " FROM workflow.flows WHERE id = $1 AND workspace_id = $2"
            ),
            ""
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(executor)
        .await
        .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound("flow".into()))
    }

    /// Every draft in `workspace` matching `filter`, newest first.
    pub async fn list(&self, ws: Uuid, filter: &ListFilter) -> Result<Vec<FlowRow>> {
        sqlx::query_as(with_flow_names!(
            concat!(
                "SELECT ",
                flow_cols!(),
                " FROM workflow.flows \
                 WHERE workspace_id = $1 \
                   AND ($2::uuid IS NULL OR folder_id = $2) \
                   AND ($3::text IS NULL OR namespace = $3) \
                   AND ($4::text IS NULL OR state = $4) \
                   AND $5 = (deleted_at IS NOT NULL)"
            ),
            " ORDER BY r.created_at DESC"
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
        // W12: the update and `scheduler::sync` commit together -- also when
        // only `namespace`/`folder_id` changed, where `sync` is a no-op.
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let row: std::result::Result<Option<FlowRow>, sqlx::Error> =
            sqlx::query_as(with_flow_names!(
                concat!(
                    "UPDATE workflow.flows SET \
                   namespace = COALESCE($3, namespace), \
                   flow_id   = COALESCE($4, flow_id), \
                   source    = COALESCE($5, source), \
                   plan      = COALESCE($6, plan), \
                   folder_id = CASE WHEN $7 THEN $8 ELSE folder_id END, \
                   updated_at = now() \
                 WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL \
                   AND ($9::timestamptz IS NULL OR updated_at = $9) \
                 RETURNING ",
                    flow_cols!()
                ),
                ""
            ))
            .bind(id)
            .bind(ws)
            .bind(&input.namespace)
            .bind(&input.flow_id)
            .bind(&input.source)
            .bind(&input.plan)
            .bind(input.folder_id.is_some())
            .bind(input.folder_id.flatten())
            .bind(input.if_updated_at)
            .fetch_optional(&mut *tx)
            .await;
        // Whole-branch review, IMPORTANT finding 3: same `UNIQUE
        // (workspace_id, namespace, flow_id)` this table's `create` can
        // hit -- reachable here too, since an update may repoint a flow's
        // own `namespace`/`flow_id` onto a name another (or the same,
        // trashed) row already holds. `effective_identity` resolves what
        // the WOULD-BE name is when the caller left one half `None` (a
        // `PATCH`-shaped caller `create`'s `input.namespace`/`flow_id`
        // pair never needs), mirroring `EndpointStore::update`'s
        // `effective_route`.
        let row = match row {
            Ok(row) => row,
            Err(e) if is_namespace_conflict(&e) => {
                // The failed statement aborted `tx`: roll it back first, and
                // keep the two follow-up reads on the pool.
                drop(tx);
                let (namespace, flow_id) = self.effective_identity(ws, id, input).await;
                return Err(self.namespace_conflict(ws, &namespace, &flow_id).await);
            }
            Err(e) => return Err(db_err(e)),
        };
        let row = match row {
            Some(row) => row,
            // Zero rows with a precondition: the row is missing (404, as
            // without one) or it no longer matches (409). The comparison
            // already happened inside the `UPDATE` above, under its row
            // lock, so this read only names the refusal; it decides nothing.
            None if input.if_updated_at.is_some() => {
                let exists: Option<(Uuid,)> = sqlx::query_as(
                    "SELECT id FROM workflow.flows \
                     WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL",
                )
                .bind(id)
                .bind(ws)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
                return Err(match exists {
                    Some(_) => Error::Conflict(DRAFT_CHANGED.into()),
                    None => Error::NotFound("flow".into()),
                });
            }
            None => return Err(Error::NotFound("flow".into())),
        };
        crate::scheduler::sync(&mut tx, &row, false).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(row)
    }

    /// The effective `(namespace, flow_id)` an update WOULD have written,
    /// for the conflict message -- only needed when `input` leaves one half
    /// `None` (routes.rs's `PUT` always sends both, since it is a full
    /// replace; this exists so the repository's own contract stays correct
    /// for any future partial-update caller too).
    async fn effective_identity(
        &self,
        ws: Uuid,
        id: Uuid,
        input: &UpdateFlowInput,
    ) -> (String, String) {
        let current: Option<(String, String)> = sqlx::query_as(
            "SELECT namespace, flow_id FROM workflow.flows WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .ok()
        .flatten();
        let (cur_namespace, cur_flow_id) = current.unwrap_or_default();
        (
            input.namespace.clone().unwrap_or(cur_namespace),
            input.flow_id.clone().unwrap_or(cur_flow_id),
        )
    }

    /// Soft delete: `deleted_at = now()`. Idempotent-in-effect -- deleting an
    /// already-trashed flow (or one that never existed in this workspace)
    /// still answers `NotFound` rather than silently doing nothing, so a
    /// caller cannot mistake "already gone" for "just deleted".
    ///
    /// W12: the delete and `scheduler::sync(.., gone = true)` commit together,
    /// so a trashed flow keeps no trigger state. `ON DELETE CASCADE` does not
    /// fire on a soft delete, and kept state would make a future restore
    /// replay the whole time in the trash under `ALL`.
    pub async fn soft_delete(&self, ws: Uuid, id: Uuid) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let updated: Option<FlowRow> = sqlx::query_as(concat!(
            "UPDATE workflow.flows SET deleted_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL \
             RETURNING ",
            flow_cols!()
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let row = updated.ok_or_else(|| Error::NotFound("flow".into()))?;
        crate::scheduler::sync(&mut tx, &row, true).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// Bring a flow back from the trash (KI-119): `deleted_at = NULL`, and the
    /// draft keeps its source, state and versions.
    ///
    /// `scheduler::sync` runs in the same transaction, as on every write that
    /// changes what the scheduler reads: the soft delete removed the trigger
    /// rows, so the restore recreates them from the source, each schedule
    /// starting at its next tick after now -- the time in the trash is not
    /// replayed.
    ///
    /// `deleted_at IS NOT NULL` is in the `WHERE`: restoring a live flow is a
    /// 409, never a silent no-op, and an absent or foreign id is a 404 first.
    pub async fn restore(&self, ws: Uuid, id: Uuid) -> Result<FlowRow> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let restored: Option<FlowRow> = sqlx::query_as(with_flow_names!(
            concat!(
                "UPDATE workflow.flows SET deleted_at = NULL, updated_at = now() \
                 WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NOT NULL \
                 RETURNING ",
                flow_cols!()
            ),
            ""
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(row) = restored else {
            drop(tx);
            self.get(ws, id).await?;
            return Err(Error::Conflict(
                "the flow is not in the trash; there is nothing to restore".into(),
            ));
        };
        crate::scheduler::sync(&mut tx, &row, false).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(row)
    }

    /// Delete a trashed flow for good (KI-119), which frees its
    /// `(namespace, flow_id)`. Only a flow already in the trash: a live one is
    /// a 409, so this can never be the first step that takes a flow away.
    ///
    /// What goes with it, by the foreign keys: its versions, trigger rows,
    /// listeners, runtime switches and backfills (`ON DELETE CASCADE`). What
    /// stays: its executions, whose `flow_id` becomes `NULL`
    /// (`ON DELETE SET NULL`, migration 0134) and which keep their frozen
    /// source, namespace and flow key -- the record of what ran outlives the
    /// flow it ran from.
    pub async fn purge(&self, ws: Uuid, id: Uuid) -> Result<FlowRow> {
        let gone: Option<FlowRow> = sqlx::query_as(concat!(
            "DELETE FROM workflow.flows \
             WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NOT NULL \
             RETURNING ",
            flow_cols!()
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        match gone {
            Some(row) => Ok(row),
            None => {
                self.get(ws, id).await?;
                Err(Error::Conflict(
                    "the flow is not in the trash; move it to the trash before deleting it permanently"
                        .into(),
                ))
            }
        }
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
        sqlx::query_as(with_creator_name!(
            "INSERT INTO workflow.flow_versions (flow_id, version, snapshot, note, created_by) \
             SELECT $1, COALESCE(MAX(version), 0) + 1, $2, $3, $4 \
               FROM workflow.flow_versions WHERE flow_id = $1 \
             RETURNING id, flow_id, version, snapshot, note, created_by, created_at",
            ""
        ))
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
    ///
    /// Exempt from `scheduler::sync`: it changes neither `source` nor
    /// `deleted_at`, and `state` is read at fire time (W12 spec §3.3).
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
        sqlx::query_as(with_creator_name!(
            "SELECT v.id, v.flow_id, v.version, v.snapshot, v.note, v.created_by, v.created_at \
             FROM workflow.flow_versions v \
             JOIN workflow.flows f ON f.id = v.flow_id \
             WHERE v.flow_id = $1 AND f.workspace_id = $2",
            " ORDER BY r.version DESC"
        ))
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
        let row: Option<FlowVersionRow> = sqlx::query_as(with_creator_name!(
            "SELECT v.id, v.flow_id, v.version, v.snapshot, v.note, v.created_by, v.created_at \
             FROM workflow.flow_versions v \
             JOIN workflow.flows f ON f.id = v.flow_id \
             WHERE v.flow_id = $1 AND v.version = $2 AND f.workspace_id = $3",
            ""
        ))
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
    /// exist". (A `subflow:` reference no longer reaches this method: since
    /// debt spec §5.1, 2026-10-19, publish resolves one with the SCOPED
    /// [`Self::find_in_workspace`].)
    pub async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>> {
        sqlx::query_scalar("SELECT workspace_id FROM workflow.flows WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// The `source` of the draft named `(namespace, flow_id)` in `ws`, or
    /// `Ok(None)`: what a `Subflow` expands to in the topology (W7c spec
    /// §3.4, [`crate::graph::SubflowSource`]).
    ///
    /// SCOPED by `ws`: a `Subflow`
    /// resolves within its own workspace (as `worker::subflow::resolve`
    /// does at run time), so a flow of the same name in another workspace
    /// is "no such flow", never its graph. A trashed flow does not resolve;
    /// a disabled one does -- the graph shows what the flow IS, and
    /// disabling it changes whether it runs, not its shape.
    pub async fn draft_source_by_name(
        &self,
        ws: Uuid,
        namespace: &str,
        flow_id: &str,
    ) -> Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT source FROM workflow.flows \
              WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3 \
                AND deleted_at IS NULL",
        )
        .bind(ws)
        .bind(namespace)
        .bind(flow_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// The `id` of the flow named `(namespace, flow_id)` in `ws`, or
    /// `Ok(None)` if `ws` holds no such flow -- whether or not another
    /// workspace does.
    ///
    /// SCOPED by `ws`: this is what `publish` resolves a `Subflow` and a
    /// Flow trigger's `dependsOn` pair through (debt spec §5.1, 2026-10-19),
    /// so its answer depends on the caller's workspace alone and cannot probe
    /// another one, and a name two workspaces share resolves to the caller's
    /// own. It replaced the unscoped `find_by_name` (`(id, workspace_id)` from
    /// ANY workspace, tie broken by `ORDER BY id`), which was removed with
    /// it. `UNIQUE (workspace_id,
    /// namespace, flow_id)` (migration 0132) makes the row unique.
    ///
    /// A trashed flow still answers, the same choice the removed
    /// `find_by_name` made: `deleted_at` is not filtered here. The run-time lookup,
    /// `worker::subflow::resolve`, is what refuses a trashed or disabled
    /// target.
    pub async fn find_in_workspace(
        &self,
        ws: Uuid,
        namespace: &str,
        flow_id: &str,
    ) -> Result<Option<Uuid>> {
        sqlx::query_scalar(
            "SELECT id FROM workflow.flows \
              WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3",
        )
        .bind(ws)
        .bind(namespace)
        .bind(flow_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }
}

// ===========================================================================
// W10 Task 3: `ExecutionRepo` -- the read layer over `workflow.executions`
// and `workflow.task_runs`.
// ===========================================================================

/// The projection every read of `workflow.executions` shares.
///
/// **`source` is deliberately absent.** It is the frozen flow YAML -- a whole
/// document, per row -- and a fifty-row list page would carry fifty copies of
/// something no list column shows. [`ExecutionRepo::get`] selects it beside
/// this list, because a detail page draws its topology from it.
macro_rules! execution_cols {
    () => {
        "id, workspace_id, flow_id, namespace, flow_key, state, history, inputs, seq, \
         execution_trigger, trigger_execution_id, trigger_run_ix, \
         parent_execution_id, parent_run_ix, loop_index, loop_value, \
         attempt_number, origin_offset_ms, replayed_from_execution_id, \
         started_at, ended_at, created_at, updated_at, labels, backfill_id"
    };
}

/// The projection every read of `workflow.task_runs` shares, aliased `t`
/// because the only statement that selects it joins `workflow.executions`
/// for the workspace predicate (the table carries no `workspace_id` of its
/// own -- the join is what a bare predicate would have been, the same shape
/// [`FlowRepo::list_versions`] uses for `flow_versions`).
macro_rules! task_run_cols {
    () => {
        "t.id, t.execution_id, t.run_ix, t.task_ix, t.task_id, t.type_id, t.parent_ix, \
         t.value, t.rendered, t.rendered_run_if, t.state, t.history, t.attempts, \
         t.not_before_ms, t.claimed_at, t.outputs, t.message, t.created_seq, \
         t.ended_seq, t.resume_with, t.started_at, t.ended_at, t.created_at, t.updated_at"
    };
}

/// One `workflow.executions` row, minus `source`.
///
/// **`state` is a column, not a derivation.** Migration 0134 stores it
/// denormalised beside `history` precisely so a list page does not open the
/// array, and `worker::claim::persist` writes both in ONE expression, in one
/// statement, in one transaction -- so they cannot drift. Nothing here
/// re-derives a state from `history`: two representations of one value is a
/// defect this crate has already had to correct in prose twice.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExecutionRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// The draft this execution came from.
    ///
    /// A trashed flow keeps its id here: [`FlowRepo::soft_delete`] only sets
    /// `deleted_at`, so filtering by [`ExecutionFilter::flow_id`] still
    /// returns a trashed flow's executions. `executions_flow_id_fkey` is
    /// `ON DELETE SET NULL` (migration 0134), and [`FlowRepo::purge`] --
    /// deleting a trashed flow permanently (KI-119) -- is the `DELETE` that
    /// sets it. The record of what ran outlives the flow it ran from.
    pub flow_id: Option<Uuid>,
    pub namespace: String,
    /// The YAML's `id`; `flow_id` is taken by the FK above.
    pub flow_key: String,
    /// Kestra's wire spelling, e.g. `RUNNING`, `SUCCESS`, `FAILED`.
    pub state: String,
    /// `ExecutionState::history`, whole -- a detail page draws a timeline
    /// from it. Never read to answer "what state is this in"; see [`Self::state`].
    pub history: serde_json::Value,
    pub inputs: serde_json::Value,
    pub seq: i64,
    /// The trigger record (migration 0140), `NULL` for an API start.
    pub execution_trigger: Option<serde_json::Value>,
    pub trigger_execution_id: Option<Uuid>,
    pub trigger_run_ix: Option<i32>,
    pub parent_execution_id: Option<Uuid>,
    pub parent_run_ix: Option<i32>,
    pub loop_index: Option<i32>,
    pub loop_value: Option<String>,
    pub attempt_number: i32,
    pub origin_offset_ms: i64,
    /// The execution a manual replay was started from (migration 0166).
    pub replayed_from_execution_id: Option<Uuid>,
    /// The first time the execution entered `RUNNING` (migration 0183);
    /// `NULL` before that, and for a row that predates the column.
    pub started_at: Option<DateTime<Utc>>,
    /// When the execution entered a terminal state (migration 0183); `NULL`
    /// while it has not, and for a row that predates the column.
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The labels this execution was started with beyond its flow's own
    /// (migration 0192): a backfill's, a replay's copy. `NULL` for none.
    pub labels: Option<serde_json::Value>,
    /// The backfill that fired this execution (migration 0192), `NULL` otherwise.
    pub backfill_id: Option<Uuid>,
}

/// One `workflow.task_runs` row, whole.
///
/// Unlike [`ExecutionRow`] this keeps every column, `rendered` and `attempts`
/// included: it is only ever read for ONE execution at a time, so there is no
/// page-sized multiplier the way `executions.source` has one.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskRunRow {
    pub id: Uuid,
    pub execution_id: Uuid,
    /// The `RunIx` -- the index into `ExecutionState::runs`, i.e. creation
    /// order. The ordering key of every read in this module.
    pub run_ix: i32,
    /// The `TaskIx` -- the index into `Plan::tasks`.
    pub task_ix: i32,
    pub task_id: String,
    /// The task's `type:`, and `NULL` exactly when the planned kind is not
    /// `TaskKind::Runnable` -- i.e. a flowable the executor drives itself.
    pub type_id: Option<String>,
    pub parent_ix: Option<i32>,
    pub value: Option<String>,
    pub rendered: Option<String>,
    pub rendered_run_if: Option<String>,
    pub state: String,
    pub history: serde_json::Value,
    pub attempts: serde_json::Value,
    pub not_before_ms: Option<i64>,
    /// The lease. `NULL` means unclaimed.
    pub claimed_at: Option<DateTime<Utc>>,
    pub outputs: Option<serde_json::Value>,
    pub message: Option<String>,
    pub created_seq: Option<i64>,
    pub ended_seq: Option<i64>,
    pub resume_with: Option<String>,
    /// The first time the run entered `RUNNING` (migration 0183). `NULL` for a
    /// run that never ran (`SKIPPED`, or not yet started) and for a row that
    /// predates the column.
    pub started_at: Option<DateTime<Utc>>,
    /// When the run entered a terminal state (migration 0183); `NULL` while
    /// it has not.
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// [`ExecutionRow`] joined with the two things only a detail page needs: the
/// frozen `source` and every task run, ordered by `run_ix` -- and, for the
/// replay route, the execution's `variables` (group C2 D3).
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionDetail {
    pub execution: ExecutionRow,
    /// The flow source this execution runs, frozen at start (migration 0134).
    /// `executor::build_plan` is pure over it, which is why a detail page can
    /// draw the topology from this string alone.
    pub source: String,
    pub runs: Vec<TaskRunRow>,
    /// The execution's executor-level `variables` (migration 0196, group C2);
    /// `None` is "the flow's `variables:`". Read for the replay route, which
    /// copies it (D3); skipped by `Serialize`, so no response carries it.
    #[serde(skip)]
    pub variables: Option<serde_json::Value>,
    /// The sealed SECRET input values (migration 0203, KI-112), which only
    /// the replay route reads, to start the replay with them. Skipped by
    /// `Serialize`, so no response carries even the ciphertext.
    #[serde(skip)]
    pub secret_inputs: Option<Vec<u8>>,
}

/// The one statement that needs `source` beside the shared projection.
/// Private: [`ExecutionDetail`] is what a caller gets.
#[derive(sqlx::FromRow)]
struct ExecutionWithSource {
    #[sqlx(flatten)]
    execution: ExecutionRow,
    source: String,
    variables: Option<serde_json::Value>,
    secret_inputs: Option<Vec<u8>>,
}

/// Filters for [`ExecutionRepo::list`]. Every field `None` lists the whole
/// workspace, newest first.
///
/// The workspace is NOT a field here, deliberately -- it is a separate
/// argument on every method, so no caller can construct a filter that forgets
/// it or a `Default` that widens it.
#[derive(Debug, Clone, Default)]
pub struct ExecutionFilter {
    /// The draft the execution came from.
    ///
    /// **Trashing a flow does not hide its executions from this filter.**
    /// `FlowRepo`'s delete is SOFT, so `executions.flow_id` keeps pointing at
    /// the trashed draft -- see [`ExecutionRow::flow_id`] for the measurement.
    /// A row only goes `NULL` here if something outside this crate issues a
    /// real `DELETE`, and a `NULL` matches no value of this filter;
    /// [`Self::flow_key`] + [`Self::namespace`] is what still finds those.
    pub flow_id: Option<Uuid>,
    pub namespace: Option<String>,
    /// The YAML's `id` -- the human half of the identity, and the only filter
    /// that still finds an execution whose `flow_id` has been `NULL`ed by a
    /// real `DELETE` (which nothing in this crate issues; see
    /// [`ExecutionRow::flow_id`]).
    pub flow_key: Option<String>,
    /// Several, not one: a run list shows `FAILED` beside `RUNNING`. `Some`
    /// with an EMPTY vector matches nothing, which is what a UI that has
    /// deselected every state chip means.
    pub states: Option<Vec<String>>,
    /// Window over `created_at` -- when the execution STARTED, which is what
    /// a run list is ordered and filtered by. Half-open: `since` inclusive,
    /// `until` exclusive, so consecutive windows tile without double-counting
    /// a row on the boundary.
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Hide child executions: a loop iteration (`loop_index` set) and a
    /// subflow or flow-trigger child (`trigger_execution_id` set). `false`
    /// lists every row.
    ///
    /// **A `CREATE_NEW_EXECUTION` retry stays** (the user's ruling,
    /// 2026-10-11): it carries `parent_execution_id` but no `loop_index`, and
    /// it is the continuation of a root run, not a part of one. `loop_index`
    /// is the exact discriminator: migration 0138's
    /// `workflow_executions_loop_run_whole_or_absent` makes the loop triple
    /// whole or absent, so a parent with an index is an iteration and a
    /// parent without one is a replay (`worker/replay.rs`'s lineage table).
    pub root_only: bool,
    /// Only the direct children of this execution: rows whose
    /// `parent_execution_id` OR `trigger_execution_id` names it. Never widens
    /// past the workspace -- a foreign id matches nothing here, because the
    /// workspace predicate still applies and the composite FKs of migration
    /// 0138 make a cross-workspace child unrepresentable.
    pub parent: Option<Uuid>,
    /// A namespace and every namespace beneath it, dot-anchored: `a.b`
    /// matches `a.b` and `a.b.c`, never `a.bc`. Written with `starts_with`,
    /// not `LIKE`, so a `_` or `%` in a namespace is a character and not a
    /// wildcard. The route refuses it beside [`Self::namespace`] (400).
    pub namespace_prefix: Option<String>,
}

/// The `WHERE` both of [`ExecutionRepo::list`]'s statements carry, written
/// once so the page and its count cannot disagree. Parameters: `$1` the
/// workspace, `$2..$7` the [`ExecutionFilter`] fields in declaration order,
/// `$8` `root_only`, `$9` `parent`, `$10` `namespace_prefix`. The page's
/// `LIMIT`/`OFFSET` follow as `$11`/`$12`, so the count statement uses
/// `$1..$10` with no gap.
macro_rules! execution_list_where {
    () => {
        " FROM workflow.executions \
         WHERE workspace_id = $1 \
           AND ($2::uuid IS NULL OR flow_id = $2) \
           AND ($3::text IS NULL OR namespace = $3) \
           AND ($4::text IS NULL OR flow_key = $4) \
           AND ($5::text[] IS NULL OR state = ANY($5)) \
           AND ($6::timestamptz IS NULL OR created_at >= $6) \
           AND ($7::timestamptz IS NULL OR created_at < $7) \
           AND (NOT $8 OR (loop_index IS NULL AND trigger_execution_id IS NULL)) \
           AND ($9::uuid IS NULL OR parent_execution_id = $9 OR trigger_execution_id = $9) \
           AND ($10::text IS NULL OR namespace = $10 OR starts_with(namespace, $10 || '.'))"
    };
}

/// [`ExecutionRepo::list`]'s page statement, whole. `$11`/`$12` are
/// `LIMIT`/`OFFSET`.
const LIST_SQL: &str = concat!(
    "SELECT ",
    execution_cols!(),
    execution_list_where!(),
    " ORDER BY created_at DESC, id DESC LIMIT $11 OFFSET $12"
);

/// Test-only (`test-util`): [`ExecutionRepo::list`]'s page statement, for
/// `tests/execution_timestamps.rs`, which `EXPLAIN`s this exact text to show
/// that the `parent=` filter is served by the partial indexes of migrations
/// 0135/0138. Migration 0183 builds none.
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub const EXECUTION_LIST_SQL: &str = LIST_SQL;

/// The window [`ExecutionRepo::list`] returns, in rows.
#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

/// The largest page this repository will serve, whatever a caller asks for.
/// A clamp rather than an error because the caller that would hit it is a
/// URL query string, and a pager that asks for too much should get a page,
/// not a 400 -- Task 4 may still validate earlier and more loudly.
pub const MAX_PAGE_LIMIT: i64 = 200;

/// The default page: 50 rows from the top, matching this repo's other list
/// surfaces.
pub const DEFAULT_PAGE_LIMIT: i64 = 50;

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_PAGE_LIMIT,
            offset: 0,
        }
    }
}

impl Page {
    /// `(limit, offset)` as Postgres will accept them. A negative `LIMIT` or
    /// `OFFSET` is an ERROR in Postgres, not an empty page, so a repository
    /// that passed one straight through would turn a malformed query string
    /// into a 500.
    fn clamped(self) -> (i64, i64) {
        (self.limit.clamp(1, MAX_PAGE_LIMIT), self.offset.max(0))
    }
}

/// One page of executions plus the size of the whole match behind it.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionPage {
    pub rows: Vec<ExecutionRow>,
    /// Every row matching the filter, not just this page's -- without it a
    /// pager cannot draw a last page. Counted by a second statement carrying
    /// the SAME predicates, including the workspace one.
    pub total: i64,
}

/// Reads over `workflow.executions` and `workflow.task_runs`.
///
/// **Reads only, and that is a boundary rather than a scope note.**
/// `worker::claim::load(Lock::Yes)` takes `SELECT ... FOR NO KEY UPDATE` on the
/// execution row, and W10 Task 2 established that the lock is load-bearing:
/// it closes a stale-snapshot lost update on `persist`'s
/// `WHERE execution_id = $8 AND run_ix = $9`. Nothing here takes that lock,
/// because nothing here writes -- and no method here should ever become the
/// read half of a read-modify-write, because the writer that reused it would
/// reintroduce exactly that race. A future caller wanting to mutate an
/// execution goes through `worker::claim`, not through this type.
///
/// **Workspace containment lives here, in exactly one place**, the same
/// convention [`FlowRepo`] states above: every statement in this impl carries
/// a `workspace_id` predicate, and there is no route-level pre-check to
/// forget. `workflow.task_runs` has no `workspace_id` column of its own, so
/// its read joins `workflow.executions` for it -- the join IS the predicate.
/// There are no unscoped exceptions in this type, unlike [`FlowRepo`]: every
/// method takes `ws` and none is a scoping primitive.
///
/// **Three hazards here are closed by the SCHEMA, not by these queries**, and
/// are written down so nobody adds a clause that looks missing:
///
/// - **There is no soft delete on an execution.** `workflow.executions` has no
///   `deleted_at` column at all (verified against the live schema; `flows` is
///   the only table in the `workflow` schema that has one, which is why
///   [`FlowRepo`] carries four `deleted_at IS NULL` predicates and this type
///   carries none). A "trashed executions" filter would have nothing to read.
/// - **A sub-execution cannot belong to another workspace.**
///   `workflow_executions_parent_same_workspace` and
///   `workflow_executions_trigger_same_workspace` (migration 0138) are
///   COMPOSITE foreign keys onto `(id, workspace_id)`, so a parent or trigger
///   link that crossed a workspace is unrepresentable. A future recursive read
///   over those links inherits containment from the constraint and does not
///   need to re-derive it -- but it must still carry its OWN `workspace_id`
///   predicate on the root row, because nothing checks the root.
/// - **`executions_flow_id_fkey` is the exception, and it is a trap.** It is
///   SINGLE-column -- `FOREIGN KEY (flow_id) REFERENCES workflow.flows(id) ON
///   DELETE SET NULL` -- with **no workspace component**, unlike the two
///   above. Nothing stops `executions.flow_id` naming a `workflow.flows` row
///   in a different workspace (`worker/subflow.rs`'s own doc concedes the
///   same about this column). **Any statement that joins `workflow.flows`
///   must add its own `f.workspace_id = $n`**; this type never joins it, which
///   is why [`ExecutionRow::flow_id`] is returned as a bare id and
///   [`ExecutionFilter::flow_id`] is compared as a bare value.
///
/// See the Task 3 report's "For Task 4" section for what that last one costs a
/// list endpoint that wants to show a flow's name beside each execution.
#[derive(Clone)]
pub struct ExecutionRepo {
    db: Db,
}

impl ExecutionRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// One page of `ws`'s executions matching `filter`, newest first, plus
    /// the total behind it.
    ///
    /// Ordered `created_at DESC, id DESC`: the id is a tiebreak, not a
    /// preference -- without it two executions started in the same
    /// microsecond could swap places between two requests for the same page
    /// and a pager would show one twice and the other never.
    pub async fn list(
        &self,
        ws: Uuid,
        filter: &ExecutionFilter,
        page: Page,
    ) -> Result<ExecutionPage> {
        let (limit, offset) = page.clamped();
        let rows: Vec<ExecutionRow> = sqlx::query_as(LIST_SQL)
            .bind(ws)
            .bind(filter.flow_id)
            .bind(&filter.namespace)
            .bind(&filter.flow_key)
            .bind(filter.states.clone())
            .bind(filter.since)
            .bind(filter.until)
            .bind(filter.root_only)
            .bind(filter.parent)
            .bind(&filter.namespace_prefix)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)?;

        // A SECOND statement rather than `count(*) OVER ()`, so the window
        // clause cannot silently change what the projection above returns --
        // and so the predicate is written out twice IDENTICALLY, workspace
        // predicate included -- now literally, through
        // `execution_list_where!`. If the two ever diverge, the count is the
        // one that lies, which is why the cross-workspace test asserts on
        // `total` as well as on `rows`.
        let total: i64 = sqlx::query_scalar(concat!("SELECT count(*)", execution_list_where!()))
            .bind(ws)
            .bind(filter.flow_id)
            .bind(&filter.namespace)
            .bind(&filter.flow_key)
            .bind(filter.states.clone())
            .bind(filter.since)
            .bind(filter.until)
            .bind(filter.root_only)
            .bind(filter.parent)
            .bind(&filter.namespace_prefix)
            .fetch_one(&self.db.pool)
            .await
            .map_err(db_err)?;

        Ok(ExecutionPage { rows, total })
    }

    /// One execution in `ws`, with its frozen source and every task run.
    ///
    /// `NotFound` covers both "no such execution" and "not this workspace's",
    /// indistinguishably and on purpose: `Error::NotFound` maps to **404**
    /// (`egret_core::http`), and a 403 on a foreign id would confirm the id
    /// is real. That is the `chart_id` class -- migration 0128 shipped a
    /// plain FK with no workspace predicate and the check that followed was
    /// told the CALLER's workspace, never the object's.
    pub async fn get(&self, ws: Uuid, id: Uuid) -> Result<ExecutionDetail> {
        let row: Option<ExecutionWithSource> = sqlx::query_as(concat!(
            "SELECT ",
            execution_cols!(),
            ", source, variables, secret_inputs \
             FROM workflow.executions WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let row = row.ok_or_else(|| Error::NotFound("execution".into()))?;
        // Only reached once the execution above has been confirmed to be in
        // `ws`; `task_runs` re-checks anyway rather than trusting that, so it
        // is equally safe as a public entry point of its own.
        let runs = self.task_runs(ws, id).await?;
        Ok(ExecutionDetail {
            execution: row.execution,
            source: row.source,
            runs,
            variables: row.variables,
            secret_inputs: row.secret_inputs,
        })
    }

    /// Every task run of `execution`, ordered by `run_ix` -- their CREATION
    /// order, which is the order `advance()` addresses them in and the order
    /// the oracle compares.
    ///
    /// A foreign `execution` and one that never existed both answer an empty
    /// `Vec`, indistinguishably, for the same reason [`Self::get`] answers
    /// `NotFound` to both. `workflow.task_runs` carries no `workspace_id`, so
    /// the join to `workflow.executions` IS the predicate.
    ///
    /// Public beside [`Self::get`] so a detail page polling for progress can
    /// refresh the run table without re-reading the execution and its source
    /// every second.
    ///
    /// **The `ORDER BY` is not decoration and is not free.** `UNIQUE
    /// (execution_id, run_ix)` (migration 0134) backs an index in exactly
    /// that order, so the plan CAN hand the rows back sorted whether the
    /// statement asks or not.
    ///
    /// Measured, and stated no wider than the measurement: in the
    /// never-`ANALYZE`d per-process test clone, the three-row shape planned an
    /// `Index Scan using task_runs_execution_id_run_ix_key` and deleting this
    /// clause left `get_one_returns_its_task_runs_ordered_by_run_ix` **green**.
    /// That is a fact about a particular statistics state, not a rule about
    /// "small row counts": on freshly-`ANALYZE`d empty stats the same three
    /// rows plan a `Seq Scan` and come back `{2,0,1}`. The conclusion that
    /// survives either way is that **the test could not tell which of the two
    /// was sorting**, so it was not evidence for this clause.
    /// `task_runs_are_sorted_by_the_query_and_not_by_the_plan` is the test
    /// that actually holds it: 500 shuffled runs flip the plan to a `Seq
    /// Scan`, whose physical order is not `run_ix` order, and it checks that
    /// it flipped before it checks the result.
    pub async fn task_runs(&self, ws: Uuid, execution: Uuid) -> Result<Vec<TaskRunRow>> {
        sqlx::query_as(concat!(
            "SELECT ",
            task_run_cols!(),
            " FROM workflow.task_runs t \
             JOIN workflow.executions e ON e.id = t.execution_id \
             WHERE t.execution_id = $1 AND e.workspace_id = $2 \
             ORDER BY t.run_ix"
        ))
        .bind(execution)
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// What `GET …/executions/{id}/logs` needs of the execution, in one row
    /// and nothing else: no `source`, no task runs -- a logs panel polls this
    /// every few seconds, and [`get`](Self::get) loads both.
    ///
    /// **Same existence and ownership contract as [`get`](Self::get)**: an id
    /// in another workspace is [`Error::NotFound`], exactly as one that never
    /// existed.
    ///
    /// `complete` is computed from this row and its runs' ends: the
    /// execution is idle (`workflow.execution_is_idle`, migration 0210:
    /// terminal AND no task run live) AND its last work ended -- the later of
    /// `updated_at` and the runs' latest `COALESCE(ended_at, updated_at)` --
    /// longer ago than [`SETTLE_LAG`](crate::logs::SETTLE_LAG). A run's last lines were
    /// inserted before its outcome committed (`RunLog::finish`'s flush
    /// barrier), so once the settle lag has also passed since the last
    /// commit, the page has nothing more to reveal (spec §6). Idle, not
    /// terminal (package #2b, spec §6 of the end-hooks design): an
    /// `afterExecution:` run, or an `Exit` sibling, logs after its execution
    /// is terminal.
    pub async fn log_status(&self, ws: Uuid, id: Uuid) -> Result<ExecutionLogStatus> {
        let row: Option<(String, bool)> = sqlx::query_as(
            "SELECT e.state, \
                    COALESCE(workflow.execution_is_idle(e.id), false) \
                      AND GREATEST(e.updated_at, \
                                   (SELECT max(COALESCE(r.ended_at, r.updated_at)) \
                                      FROM workflow.task_runs r \
                                     WHERE r.execution_id = e.id)) \
                          < now() - make_interval(secs => $3) \
               FROM workflow.executions e WHERE e.id = $1 AND e.workspace_id = $2",
        )
        .bind(id)
        .bind(ws)
        .bind(crate::logs::SETTLE_LAG.as_secs_f64())
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let (state, complete) = row.ok_or_else(|| Error::NotFound("execution".into()))?;
        Ok(ExecutionLogStatus { state, complete })
    }
}

/// [`ExecutionRepo::graph_source`]'s run read: the runs of `$1` (workspace
/// `$2`) and of every `Loop` iteration beneath it, at any depth -- one
/// statement, whatever the number of nodes or iterations (spec §11).
///
/// - The family walks `parent_execution_id` over rows WITH a `loop_index`
///   only, so a `CREATE_NEW_EXECUTION` replay (a parent, no index) and a
///   `Subflow` child (`trigger_execution_id`) are not in it. The root carries
///   the workspace predicate; every step repeats it, though the composite FKs
///   of migration 0138 already make a cross-workspace child unrepresentable.
/// - `child_execution_id` is the newest execution the run started as a
///   `Subflow` (`trigger_execution_id` + `trigger_run_ix`, served by
///   `workflow_executions_trigger_idx`); a retried `Subflow` starts one per
///   attempt. A `Flow`-trigger child has no `trigger_run_ix`, so it never
///   matches.
/// - Each family row carries `top`/`top_index`: the iteration of `$1` it lies
///   under (itself at depth 1) and that iteration's `loop_index`, so a nested
///   `Loop`'s deeper runs are attributed to the outer iteration the graph
///   lists (`IterationRun`), with no further statement.
/// - Ordered root first, then each loop's iterations by `loop_index`, then
///   `run_ix`: an iteration list in loop order.
const GRAPH_RUNS_SQL: &str =
    "WITH RECURSIVE fam(id, depth, parent, parent_run_ix, loop_index, top, top_index) AS ( \
       SELECT id, 0, NULL::uuid, NULL::int, NULL::int, NULL::uuid, NULL::int \
         FROM workflow.executions WHERE id = $1 AND workspace_id = $2 \
     UNION ALL \
       SELECT e.id, fam.depth + 1, e.parent_execution_id, e.parent_run_ix, e.loop_index, \
              CASE WHEN fam.depth = 0 THEN e.id ELSE fam.top END, \
              CASE WHEN fam.depth = 0 THEN e.loop_index ELSE fam.top_index END \
         FROM workflow.executions e JOIN fam ON e.parent_execution_id = fam.id \
        WHERE e.loop_index IS NOT NULL AND e.workspace_id = $2) \
     SELECT t.execution_id, fam.depth > 0 AS iteration, \
            fam.top AS iteration_execution_id, fam.top_index AS iteration_index, \
            t.id AS run_id, t.task_id, t.state, \
            jsonb_array_length(t.attempts) AS attempts, t.started_at, t.ended_at, \
            (SELECT c.id FROM workflow.executions c \
              WHERE c.trigger_execution_id = t.execution_id AND c.trigger_run_ix = t.run_ix \
                AND c.workspace_id = $2 \
              ORDER BY c.created_at DESC, c.id DESC LIMIT 1) AS child_execution_id \
       FROM fam JOIN workflow.task_runs t ON t.execution_id = fam.id \
      ORDER BY fam.depth, fam.parent, fam.parent_run_ix, fam.loop_index, t.run_ix";

impl ExecutionRepo {
    /// What `GET …/executions/{id}/graph` draws an execution from: its frozen
    /// `source` and [`GRAPH_RUNS_SQL`]'s runs. Two statements, no per-node
    /// read. `None` for an id that is not `ws`'s, as [`Self::get`]'s
    /// `NotFound` -- indistinguishable from one that never existed.
    pub async fn graph_source(
        &self,
        ws: Uuid,
        id: Uuid,
    ) -> Result<Option<crate::graph::FrozenExecution>> {
        let source: Option<String> = sqlx::query_scalar(
            "SELECT source FROM workflow.executions WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let Some(source) = source else {
            return Ok(None);
        };
        let runs = sqlx::query_as(GRAPH_RUNS_SQL)
            .bind(id)
            .bind(ws)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(Some(crate::graph::FrozenExecution { source, runs }))
    }
}

/// [`ExecutionRepo::log_status`]'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionLogStatus {
    /// Kestra's wire spelling, as stored.
    pub state: String,
    /// Idle (terminal, no task run live), and its last work ended longer
    /// ago than the settle lag.
    pub complete: bool,
}
