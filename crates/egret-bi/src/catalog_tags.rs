//! Catalog-resource tag targets (Access P3.2) — attach a classification `tag`
//! (from P3.1's `tags` catalog) to a CATALOG table/column. Unlike a BI object, a
//! catalog resource is NOT uuid-keyed: it is identified by
//! warehouse + schema + table + column, where an empty column (`""`) means a
//! table-level tag. The repo methods run parameterized SQL against migration
//! 0081's `catalog_tag_assignments` table; the router exposes control-plane
//! endpoints gated on the catalog-manage permission (`catalog_registry:update`
//! for writes, `catalog_registry:read` for reads), mirroring the
//! warehouse_admin browse API. SP3c-2 retired the `warehouse:*` keys these
//! constants used to hold; 0153 re-ran 0148's grant and deleted them.
//!
//! A write here also recompiles every SP4a `perm` policy profile bound to the
//! warehouse, in the SAME transaction as the write (Task 7, D5; fix round 1)
//! — see [`CatalogTagRepo::recompile_bound_services_in_tx`].

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_metastore::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::BiApiError;
use crate::routes::BiState;
use crate::shares::TagDto;
use crate::tags::Tag;

/// The catalog-manage permission gating catalog-tag WRITES (assign/unassign);
/// the same `catalog_registry:update` used by the warehouse_admin mutating
/// browse ops.
const WAREHOUSE_WRITE: &str = "catalog_registry:update";
/// The permission gating catalog-tag READS (list); the warehouse_admin read perm.
const WAREHOUSE_READ: &str = "catalog_registry:read";

/// A classification tag attached to a specific catalog resource. `column` is
/// empty (`""`) for a table-level assignment. `tag` is the full P3.1 tag row (its
/// `assignment_count` is the tag's global BI-object count, as `TagRepo` computes).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogTagAssignment {
    pub schema: String,
    pub table: String,
    pub column: String,
    #[sqlx(flatten)]
    pub tag: Tag,
}

#[derive(Clone)]
pub struct CatalogTagRepo {
    db: Db,
}

impl CatalogTagRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Attach `tag_id` to a catalog resource. Idempotent — a re-assign is a no-op
    /// (ON CONFLICT DO NOTHING on the composite primary key). `column = ""` tags
    /// the whole table.
    ///
    /// Bumps `tag_version` on every policy service bound to this warehouse (see
    /// [`Self::bump_bound_tag_versions`]) and recompiles those same services'
    /// `perm` profiles (see [`Self::recompile_bound_services_in_tx`], SP4a
    /// Task 7 fix round 1), all in the SAME transaction — the Ranger
    /// tag-download plugin re-polls `download_tags` instead of 304-ing
    /// forever, and the compiled StarRocks masking policy set is never
    /// visible in a state this transaction hasn't fully applied yet.
    ///
    /// **Refuses an unresolvable coordinate up front** —
    /// [`unresolvable_coordinate`], SP4d debt Task 3. Migration 0177 is what makes
    /// the state impossible; this is what makes it a 400 naming the level instead
    /// of that constraint surfacing as an opaque 500 (`BiError::Db` maps to
    /// "internal error"). The two predicates are written to the same shape on
    /// purpose: "two guards that must agree, written to different shapes" is a
    /// defect this programme has produced four times.
    ///
    /// **And refuses the one pair that makes a table unreadable** — SP4d debt
    /// Task 5. A TABLE-level attachment (`column = ""`) of a tag that a
    /// tag-addressed MASK already names is refused here, which is the same rule,
    /// in the same function, that `egret_perm_compile::routes::write_rule` and
    /// `egret_policy::admin::create_policy` apply from the other direction; see
    /// `egret_policy::unreadable_table_mask`.
    pub async fn assign_catalog(
        &self,
        tag_id: Uuid,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
        assigned_by: Option<Uuid>,
    ) -> Result<(), BiApiError> {
        if let Some(why) = unresolvable_coordinate(schema, table, column) {
            return Err(crate::error::BiError::BadRequest(why).into());
        }
        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(crate::error::BiError::from)?;
        // **The OTHER half of SP4d debt Task 5's one rule**, and the half the
        // obvious implementation misses: a table-level attachment is refused
        // when a tag-addressed MASK already names this tag, exactly as a
        // tag-addressed mask is refused when the attachment already exists.
        // Either half can be created second; `egret_policy::unreadable_table_mask`
        // is the single function both directions call, and its module doc carries
        // the live Trino 470 measurement, the full write-path enumeration and why
        // there is no migration for this pair.
        //
        // ⚠ `column` is passed VERBATIM and this call site makes no judgement
        // about it. Only a TABLE-level attachment (`column_name = ''`,
        // migration 0081's sentinel, which 0177 keeps legal on purpose) is half
        // of the pair, and that decision lives beside the SQL that spells the
        // same thing `= ''` — a gate here plus a predicate there would be two
        // guards that must agree, written to different shapes. A column-level
        // assignment of a masked tag is the ordinary, working configuration and
        // the guard returns `None` for it.
        if let Some(pair) = egret_policy::unreadable_table_mask_for(
            // `Transaction` derefs to the connection the guard reads on, so the
            // check runs on THIS transaction's snapshot — the row this write is
            // about to insert is not in it yet, which is why the guard can
            // refuse before writing anything.
            &mut tx,
            egret_policy::TagWrite::Attachment {
                tag_id,
                schema,
                table,
                column,
            },
        )
        .await
        .map_err(crate::error::BiError::from)?
        {
            return Err(crate::error::BiError::Coded {
                status: axum::http::StatusCode::BAD_REQUEST,
                body: pair.refusal_body(),
            }
            .into());
        }
        // **SP4d-4 Task 3 (user ruling): an assignment that makes two mask (or
        // row-filter) policies decide one column differently is refused** —
        // between hand-written tag policies, compiled rules, or one of each.
        // The recompile below cannot refuse the pair an assignment completes:
        // it compares stored and compiled bodies under the SAME (already
        // changed) assignments on both of its sides (SP4d-4 Task 3 fix round
        // 2). The snapshot taken HERE, before the row exists, is what lets the
        // check after the recompile refuse exactly the pairs this write creates
        // across the whole stored unit (a profile and its tag service) —
        // compiled, hand-written, or one of each — without refusing a pair
        // that was already stored. One rule, `egret_policy::unit_overlaps`.
        //
        // View governance B3: the views that read this table are re-tagged in
        // this same transaction, and their warehouses join the bump and the
        // recompile, so the base and its views move in ONE tag version.
        let reach = ViewReach::of(&mut tx, warehouse_id, schema, table).await?;
        let overlaps_before =
            egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &reach.services)
                .await
                .map_err(crate::error::BiError::from)?;
        // An admin's assignment of a coordinate the lineage already derived
        // makes it a manual one (migration 0188), so no retag removes it.
        sqlx::query(
            "INSERT INTO catalog_tag_assignments \
                 (tag_id, warehouse_id, schema_name, table_name, column_name, assigned_by) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) \
             DO UPDATE SET from_lineage = false, assigned_by = EXCLUDED.assigned_by, \
                           assigned_at = now() \
             WHERE catalog_tag_assignments.from_lineage",
        )
        .bind(tag_id)
        .bind(warehouse_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .bind(assigned_by)
        .execute(&mut *tx)
        .await
        .map_err(crate::error::BiError::from)?;
        reach.retag_bump_and_recompile(&mut tx).await?;
        egret_perm_compile::overlap::refuse_created_overlaps_via(&mut tx, &overlaps_before)
            .await
            .map_err(|e| {
                let at = if column.is_empty() {
                    format!("{schema}.{table}")
                } else {
                    format!("{schema}.{table}.{column}")
                };
                crate::error::BiError::from_assignment_check(e, &format!("this tag to {at}"))
            })?;
        tx.commit().await.map_err(crate::error::BiError::from)?;
        Ok(())
    }

    /// Detach `tag_id` from a catalog resource (idempotent — deletes 0 or 1 row).
    ///
    /// Runs no overlap check of its own (SP4d-4 Task 3): removing a tag from a
    /// coordinate can only shrink where a tag policy applies, so it can remove
    /// a pair but never create one. Its recompile refuses only a pair the
    /// compile itself creates; a pair that remains stored is reported, not
    /// refused (user ruling 2026-10-12).
    /// Bumps the bound services' `tag_version` and recompiles them in the same
    /// transaction (see [`Self::assign_catalog`]).
    pub async fn unassign_catalog(
        &self,
        tag_id: Uuid,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<(), BiApiError> {
        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(crate::error::BiError::from)?;
        let reach = ViewReach::of(&mut tx, warehouse_id, schema, table).await?;
        sqlx::query(
            "DELETE FROM catalog_tag_assignments \
             WHERE tag_id = $1 AND warehouse_id = $2 AND schema_name = $3 \
                 AND table_name = $4 AND column_name = $5",
        )
        .bind(tag_id)
        .bind(warehouse_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .execute(&mut *tx)
        .await
        .map_err(crate::error::BiError::from)?;
        reach.retag_bump_and_recompile(&mut tx).await?;
        tx.commit().await.map_err(crate::error::BiError::from)?;
        Ok(())
    }

    /// Recompile every policy profile `warehouse_id`'s engine(s) are bound
    /// through (SP4a Task 7, D5): a catalog-tag write can add or remove a
    /// column from a StarRocks tag rule's compile-time enumeration, and
    /// StarRocks has no tag service to resolve that itself the way Trino's
    /// plugin does — egret must recompile, or the new/removed column stays
    /// un(un)masked until something else happens to trigger a recompile.
    ///
    /// **Fix round 1: runs INSIDE the tag write's own transaction**, on the
    /// SAME connection (`egret_perm_compile::recompile_service_in_tx`), not
    /// after it commits. Round 1 opened `recompile_service`'s own,
    /// SEPARATELY-connected transaction after this one committed, reasoning
    /// that its `policy_version` bump would otherwise block on the row
    /// `bump_bound_tag_versions` had already locked in the still-open outer
    /// transaction — true, but the fix for that is to stop opening a SECOND
    /// connection, not to defer the write past the boundary the brief asked
    /// for one transaction to cover. `recompile_service_in_tx` takes `&mut
    /// Transaction` directly (the same shape `sync_assignment_grants_in_tx`
    /// already established) and does every read AND write against it, so a
    /// crash between "the tag committed" and "the recompile committed" can no
    /// longer happen — there is only one commit. Any recompile FAILURE now
    /// rolls the tag write back too (via `?` before `tx.commit()`), rather
    /// than leaving a committed tag with a stale compiled policy set behind
    /// it; it still surfaces as a real error to the caller (`BiError::Engine`,
    /// see `crate::error`'s `From<egret_perm::CompileError>`), not a
    /// swallowed 500.
    ///
    /// A recompile that SKIPPED something (a `WorkspaceMembers` grant this
    /// compiler cannot yet expand — see `egret_perm_compile::trigger`'s
    /// module doc) is not itself a failure and does not roll anything back;
    /// `recompile_service_in_tx` already logs each skip via `tracing::warn!`,
    /// and this call site logs once more per service with the affected
    /// `warehouse_id`, since `CatalogTagRepo` is the one place that knows
    /// which tag write triggered it.
    ///
    /// View governance B3 (Task 9): `warehouse_ids` is the written warehouse
    /// and the warehouses of the views that read the written table
    /// ([`ViewReach`]); `service_ids` is the union of their bound services.
    async fn recompile_bound_services_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        warehouse_ids: &[Uuid],
        service_ids: &[i64],
    ) -> Result<(), BiApiError> {
        // SP4a Task 11, defect 2: a recompile now also locks and writes the
        // profile's TAG service, whose id can be lower than the profile's. Take
        // the whole expanded lock set ascending up front rather than letting
        // service N's tag service be locked after service N+1's profile —
        // `lock_services_in_tx` documents why that ordering is what keeps two
        // overlapping recompiles queueing instead of deadlocking.
        egret_perm_compile::lock_services_in_tx(tx, service_ids)
            .await
            .map_err(crate::error::BiError::from)?;
        for &service_id in service_ids {
            let summary = egret_perm_compile::recompile_service_in_tx(tx, service_id)
                .await
                .map_err(crate::error::BiError::from)?;
            if !summary.skipped.is_empty() {
                tracing::warn!(
                    warehouse_ids = ?warehouse_ids,
                    service_id,
                    skipped_count = summary.skipped.len(),
                    "catalog-tag write's recompile skipped some grants/rules for this service"
                );
            }
        }
        Ok(())
    }

    /// Bump the `tag_version` of every policy service that serves
    /// `warehouse_id`'s catalog tags, so the Ranger tag plugin stops 304ing and
    /// re-downloads. Runs on the caller's transaction (cross-seam, same DB).
    ///
    /// The resolution itself — the registry∪legacy `profiles` union and the
    /// `tagService` fan-out the plugin actually polls — lives in
    /// `egret_catalog_registry::bump_bound_tag_versions` (SP3
    /// catalog-registry review C1/I1/N1/F-1). One implementation, two
    /// callers (assign and unassign), no drift: read that function for why
    /// each branch is there.
    ///
    /// View governance B3 (Task 9): over the written warehouse and the
    /// dependent views' warehouses together, each service bumped once.
    async fn bump_bound_tag_versions(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        warehouse_ids: &[Uuid],
    ) -> Result<(), BiApiError> {
        egret_catalog_registry::bump_tag_versions_for_warehouses(tx, warehouse_ids)
            .await
            .map_err(crate::error::BiError::from)?;
        Ok(())
    }

    /// The tags carried by one EXACT resource (`column = ""` ⇒ the table itself),
    /// ordered by key. Distinguishes a table-level tag from a column-level tag.
    pub async fn tags_for_resource(
        &self,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<Vec<Tag>, BiApiError> {
        let sql = "SELECT t.id, t.key, t.name, t.description, t.color, \
                 COUNT(ta.tag_id) AS assignment_count, t.created_at \
             FROM tags t \
             JOIN catalog_tag_assignments cta ON cta.tag_id = t.id \
                 AND cta.warehouse_id = $1 AND cta.schema_name = $2 \
                 AND cta.table_name = $3 AND cta.column_name = $4 \
             LEFT JOIN tag_assignments ta ON ta.tag_id = t.id \
             GROUP BY t.id ORDER BY t.key";
        Ok(sqlx::query_as::<_, Tag>(sql)
            .bind(warehouse_id)
            .bind(schema)
            .bind(table)
            .bind(column)
            .fetch_all(&self.db.pool)
            .await
            .map_err(crate::error::BiError::from)?)
    }

    /// Every catalog-tag assignment on a single table — the table-level tags AND
    /// all of its columns' tags — as `{schema, table, column, tag}` rows.
    pub async fn tags_for_table(
        &self,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Vec<CatalogTagAssignment>, BiApiError> {
        let sql =
            list_select("cta.warehouse_id = $1 AND cta.schema_name = $2 AND cta.table_name = $3");
        Ok(
            sqlx::query_as::<_, CatalogTagAssignment>(sqlx::AssertSqlSafe(sql))
                .bind(warehouse_id)
                .bind(schema)
                .bind(table)
                .fetch_all(&self.db.pool)
                .await
                .map_err(crate::error::BiError::from)?,
        )
    }

    /// Every catalog-tag assignment in a warehouse, as `{schema, table, column,
    /// tag}` rows (the Task-2 `download_tags` builder enumerates these).
    pub async fn list_for_warehouse(
        &self,
        warehouse_id: Uuid,
    ) -> Result<Vec<CatalogTagAssignment>, BiApiError> {
        let sql = list_select("cta.warehouse_id = $1");
        Ok(
            sqlx::query_as::<_, CatalogTagAssignment>(sqlx::AssertSqlSafe(sql))
                .bind(warehouse_id)
                .fetch_all(&self.db.pool)
                .await
                .map_err(crate::error::BiError::from)?,
        )
    }
}

/// What a catalog-tag write on `schema.table` reaches (view governance B3):
/// the written warehouse, the views whose lineage reads that table and their
/// warehouses, and the services bound to any of them. Computed ONCE, before the
/// write and after [`egret_perm_compile::view_tags::lock_view_tags_in_tx`], so
/// no lineage can commit in between (Task 9 fix round 1, I2), and the overlap
/// snapshot covers every profile the write can recompile.
struct ViewReach {
    warehouse_id: Uuid,
    views: Vec<egret_perm_compile::lineage::ViewRef>,
    warehouses: Vec<Uuid>,
    services: Vec<i64>,
}

impl ViewReach {
    /// Takes the view-tag lock, so call it FIRST in the transaction.
    async fn of(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Self, BiApiError> {
        egret_perm_compile::view_tags::lock_view_tags_in_tx(tx)
            .await
            .map_err(crate::error::BiError::from)?;
        let tables =
            egret_perm_compile::view_tags::tables_of_warehouse(tx, warehouse_id, schema, table)
                .await
                .map_err(crate::error::BiError::from)?;
        let views = egret_perm_compile::view_tags::dependent_views(tx, &tables)
            .await
            .map_err(crate::error::BiError::from)?;
        let mut warehouses = egret_perm_compile::view_tags::warehouses_of_views(tx, &views)
            .await
            .map_err(crate::error::BiError::from)?;
        warehouses.push(warehouse_id);
        warehouses.sort();
        warehouses.dedup();
        let services = egret_catalog_registry::services_bound_to_warehouses(&mut **tx, &warehouses)
            .await
            .map_err(crate::error::BiError::from)?;
        // Before the base write, whose `assigned_by` holds `FOR KEY SHARE` on
        // the assigning user: the profile locks, then the compile-mint lock
        // the recompile mints under. Taken after that write, a concurrent
        // compile minting the same user would wait on this transaction while
        // it waited on that compile (`egret_perm_compile::lock_compile_mints_in_tx`).
        // The recompile's own set is a subset of `services` (its warehouses
        // are, `retag_bump_and_recompile` refuses otherwise), so it re-takes
        // only locks already held.
        if !services.is_empty() {
            egret_perm_compile::lock_services_in_tx(tx, &services)
                .await
                .map_err(crate::error::BiError::from)?;
            egret_perm_compile::lock_compile_mints_in_tx(tx)
                .await
                .map_err(crate::error::BiError::from)?;
        }
        Ok(Self {
            warehouse_id,
            views,
            warehouses,
            services,
        })
    }

    /// After the base write: re-tag the dependent views, then ONE bump over
    /// the written warehouse and the warehouses the re-tag actually changed,
    /// one recompile of their services, and a request, delivered at commit,
    /// for the sweep of the views that have no computed lineage yet.
    async fn retag_bump_and_recompile(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), BiApiError> {
        let summary = egret_perm_compile::view_tags::retag_views_in_tx(tx, &self.views)
            .await
            .map_err(crate::error::BiError::from)?;
        let mut touched: Vec<Uuid> = summary.warehouses.into_iter().collect();
        // The lock keeps the views from changing since `of`, so the re-tag
        // stays inside the snapshot. Refuse rather than leave a write the
        // snapshot and the bump did not cover.
        if let Some(outside) = touched.iter().find(|w| !self.warehouses.contains(w)) {
            return Err(crate::error::BiError::Conflict(format!(
                "a view in warehouse {outside} changed while this tag write ran; retry"
            ))
            .into());
        }
        touched.push(self.warehouse_id);
        touched.sort();
        touched.dedup();
        let services = egret_catalog_registry::services_bound_to_warehouses(&mut **tx, &touched)
            .await
            .map_err(crate::error::BiError::from)?;
        CatalogTagRepo::bump_bound_tag_versions(tx, &touched).await?;
        CatalogTagRepo::recompile_bound_services_in_tx(tx, &touched, &services).await?;
        egret_perm_compile::view_tags::notify_classification_in_tx(tx, &[self.warehouse_id])
            .await
            .map_err(crate::error::BiError::from)?;
        Ok(())
    }
}

/// Why this `(schema, table, column)` cannot be stored, or `None`.
///
/// **SP4d debt Task 3, and it is the write-side half of migration 0177's
/// `catalog_tag_assignments_levels_are_present_and_literal`.** The constraint is
/// what makes the state impossible; this exists so a caller is told WHICH level is
/// wrong, with a 400, rather than meeting a constraint violation that
/// `BiError::Db` renders as "internal error".
///
/// The two are keyed to the same shape deliberately — `schema <> ''` ↔
/// `!schema.is_empty()`, `strpos(x, '*') = 0 AND strpos(x, '?') = 0` ↔
/// `!x.contains(['*', '?'])` — because two guards that must agree, written to
/// different shapes, is the defect this programme has produced four times. A guard
/// here that refused LESS than the CHECK would turn into a 500; one that refused
/// MORE would reject a row the database accepts, which no error message makes
/// true.
///
/// # Why each level is ruled the way it is
///
/// * an empty `schema` or `table`: `ServiceTags::from_assignments` OMITS an empty
///   level, so the row ships as a SHORTER Ranger service resource — `('sales','','')`
///   as `{catalog, schema}` — which `RangerTagEnricher` admits as an ANCESTOR of
///   everything beneath it while egret's own exact resolver can never return it.
///   The engine would enforce a tag egret cannot see.
/// * an empty `column`: **allowed, and that is the point.** It is migration 0081's
///   documented table-level sentinel, it is what `assign_resource_tag` writes
///   whenever the request omits `column`, and the resolver's second arm IS that
///   ANCESTOR case. Refusing it would delete the table-level attachment feature.
/// * `*` or `?` anywhere: `trino.json` declares `{"wildCard": true}` on all four
///   resources, so the plugin matches those with `FilenameUtils.wildcardMatch` and
///   no SQL predicate reproduces it. ⚠ A stated over-refusal: a quoted identifier
///   MAY legally contain `*`, so a table genuinely named `ord*ers` cannot be
///   tagged. Allowing it is worse — one such row makes `explain` decline for every
///   target in the catalog — and the remedy would be a wildcard-equivalent
///   resolver, not a looser guard here.
///
/// **`unassign_catalog` deliberately has no such check.** A `DELETE` is not
/// constrained by the `CHECK`, and a deployment that somehow holds a bad row must
/// be able to remove it through the API — which also bumps `tag_version` and
/// recompiles, the two things a hand-written `DELETE` would skip.
fn unresolvable_coordinate(schema: &str, table: &str, column: &str) -> Option<String> {
    if schema.is_empty() {
        return Some("schema must not be empty".to_string());
    }
    if table.is_empty() {
        return Some("table must not be empty".to_string());
    }
    for (level, value) in [("schema", schema), ("table", table), ("column", column)] {
        if value.contains(['*', '?']) {
            return Some(format!(
                "{level} must not contain `*` or `?`: the engine would match it as a \
                 wildcard and egret could not resolve the tag"
            ));
        }
    }
    None
}

/// Build the shared `SELECT … {schema, table, column, tag}` statement. `predicate`
/// is a trusted (never user-interpolated) WHERE clause over aliased `cta`/`t`. A
/// single global-count subquery keeps `tag.assignment_count` meaningful without an
/// N-row LEFT JOIN blow-up.
fn list_select(predicate: &str) -> String {
    format!(
        "SELECT cta.schema_name AS schema, cta.table_name AS \"table\", \
             cta.column_name AS \"column\", \
             t.id, t.key, t.name, t.description, t.color, \
             COALESCE(cnt.assignment_count, 0) AS assignment_count, t.created_at \
         FROM catalog_tag_assignments cta \
         JOIN tags t ON t.id = cta.tag_id \
         LEFT JOIN ( \
             SELECT tag_id, COUNT(*) AS assignment_count \
             FROM tag_assignments GROUP BY tag_id \
         ) cnt ON cnt.tag_id = t.id \
         WHERE {predicate} \
         ORDER BY cta.schema_name, cta.table_name, cta.column_name, t.key"
    )
}

// ── Endpoints ────────────────────────────────────────────────────────────────

/// A catalog resource tag on the wire (snake_case). `column` is empty for a
/// table-level tag. The tag itself is nested under `tag` (the full P3.1 tag
/// shape, matching the FE `Tag` type) so the browse UI can read `a.tag`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResourceTagDto {
    pub schema: String,
    pub table: String,
    pub column: String,
    pub tag: TagDto,
}

impl From<CatalogTagAssignment> for ResourceTagDto {
    fn from(a: CatalogTagAssignment) -> Self {
        ResourceTagDto {
            schema: a.schema,
            table: a.table,
            column: a.column,
            tag: a.tag.into(),
        }
    }
}

/// Query params selecting the table whose resource-tags to list.
#[derive(Debug, Deserialize)]
pub struct ResourceQuery {
    pub schema: String,
    pub table: String,
}

/// Body of an assign/unassign: the resource (`column` optional ⇒ table-level) +
/// the tag id.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResourceTagBody {
    pub schema: String,
    pub table: String,
    /// Empty / omitted ⇒ a table-level tag.
    #[serde(default)]
    pub column: Option<String>,
    pub tag_id: Uuid,
}

/// Control-plane router for catalog table/column tag targets. Nested under
/// `/api` by `egret-server`, giving `/api/control-plane/warehouses/{id}/resource-tags`.
pub fn resource_tags_router() -> Router<BiState> {
    Router::new().route(
        "/control-plane/warehouses/{warehouse_id}/resource-tags",
        get(list_resource_tags)
            .put(assign_resource_tag)
            .delete(unassign_resource_tag),
    )
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{warehouse_id}/resource-tags",
    tag = "control-plane-catalog",
    params(
        ("warehouse_id" = Uuid, Path, description = "Warehouse id"),
        ("schema" = String, Query, description = "Schema name"),
        ("table" = String, Query, description = "Table name"),
    ),
    responses(
        (status = 200, description = "The table's tags plus its columns' tags", body = Vec<ResourceTagDto>),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_resource_tags(
    State(st): State<BiState>,
    Auth(p): Auth,
    Path(warehouse_id): Path<Uuid>,
    Query(q): Query<ResourceQuery>,
) -> Result<Json<Vec<ResourceTagDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &p, WAREHOUSE_READ).await?;
    let rows = CatalogTagRepo::new(st.db.clone())
        .tags_for_table(warehouse_id, &q.schema, &q.table)
        .await?;
    Ok(Json(rows.into_iter().map(ResourceTagDto::from).collect()))
}

/// A `…/resource-tags` write must name a warehouse and a classification that
/// exist (spec §11.1). `catalog_tag_assignments.warehouse_id` has no foreign
/// key (migration 0081), so an unknown warehouse stored a row no catalog serves
/// and answered 200; an unknown `tag_id` hit the tag foreign key and answered
/// 500. Both are a coded 404 instead.
async fn require_warehouse_and_tag(
    db: &Db,
    warehouse_id: Uuid,
    tag_id: Uuid,
) -> Result<(), crate::error::BiError> {
    use crate::error::BiError;
    use axum::http::StatusCode;
    let (warehouse, tag): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM catalog.warehouses WHERE id = $1), \
                EXISTS (SELECT 1 FROM tags WHERE id = $2)",
    )
    .bind(warehouse_id)
    .bind(tag_id)
    .fetch_one(&db.pool)
    .await?;
    if !warehouse {
        return Err(BiError::coded(
            StatusCode::NOT_FOUND,
            "warehouse_not_found",
            format!("No warehouse has the id {warehouse_id}."),
            serde_json::json!({ "warehouseId": warehouse_id }),
        ));
    }
    if !tag {
        return Err(BiError::coded(
            StatusCode::NOT_FOUND,
            "tag_not_found",
            format!("No classification has the id {tag_id}."),
            serde_json::json!({ "tagId": tag_id }),
        ));
    }
    Ok(())
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{warehouse_id}/resource-tags",
    tag = "control-plane-catalog",
    params(("warehouse_id" = Uuid, Path, description = "Warehouse id")),
    request_body = ResourceTagBody,
    responses(
        (status = 200, description = "Tag assigned to the catalog resource (idempotent)", body = egret_core::Ack),
        (status = 400, description = "The coordinate cannot be stored (an empty schema or table, or `*`/`?` in any level — see migration 0177), or the tag is being attached to a whole TABLE while a tag-addressed mask names it, which would make that table unreadable rather than masked (code `table_tag_mask_conflict`, with `tag`, `tagId`, `schema`, `table`, `mask`, `maskOrigin`; `warehouseId` is null)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = egret_core::http::ErrorBody),
        (status = 404, description = "`code` is `warehouse_not_found` (no warehouse has `warehouse_id`; the body adds `warehouseId`) or `tag_not_found` (no classification has `tag_id`; the body adds `tagId`)", body = egret_core::http::ErrorBody),
        (status = 409, description = "A rule overlap: the assignment would make two mask (or two row-filter) policies decide one column differently (compiled rules, hand-written tag policies, or one of each), or a profile it recompiles already holds such a pair. The body adds `code: \"rule_overlap\"`, `rules` (both members, `id` null for a hand-written policy, and `name`), `coordinate` and `kind` (`mask` | `row_filter`)", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn assign_resource_tag(
    State(st): State<BiState>,
    Auth(p): Auth,
    Path(warehouse_id): Path<Uuid>,
    Json(b): Json<ResourceTagBody>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &p, WAREHOUSE_WRITE).await?;
    require_warehouse_and_tag(&st.db, warehouse_id, b.tag_id).await?;
    let column = b.column.as_deref().unwrap_or("");
    CatalogTagRepo::new(st.db.clone())
        .assign_catalog(
            b.tag_id,
            warehouse_id,
            &b.schema,
            &b.table,
            column,
            Some(p.user_id.0),
        )
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "tag:assign",
        "catalog",
        Some(format!(
            "{warehouse_id}:{}.{}.{}",
            b.schema, b.table, column
        )),
        serde_json::json!({ "tag_id": b.tag_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/warehouses/{warehouse_id}/resource-tags",
    tag = "control-plane-catalog",
    params(("warehouse_id" = Uuid, Path, description = "Warehouse id")),
    request_body = ResourceTagBody,
    responses(
        (status = 200, description = "Tag removed from the catalog resource (idempotent)", body = egret_core::Ack),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = egret_core::http::ErrorBody),
        (status = 404, description = "`code` is `warehouse_not_found` or `tag_not_found`, as for the assignment", body = egret_core::http::ErrorBody),
        (status = 409, description = "A rule overlap that the unassignment does not remove: a profile it recompiles already holds two mask (or two row-filter) rules that decide one request differently. Same body as the assignment's 409", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn unassign_resource_tag(
    State(st): State<BiState>,
    Auth(p): Auth,
    Path(warehouse_id): Path<Uuid>,
    Json(b): Json<ResourceTagBody>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &p, WAREHOUSE_WRITE).await?;
    require_warehouse_and_tag(&st.db, warehouse_id, b.tag_id).await?;
    let column = b.column.as_deref().unwrap_or("");
    CatalogTagRepo::new(st.db.clone())
        .unassign_catalog(b.tag_id, warehouse_id, &b.schema, &b.table, column)
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "tag:unassign",
        "catalog",
        Some(format!(
            "{warehouse_id}:{}.{}.{}",
            b.schema, b.table, column
        )),
        serde_json::json!({ "tag_id": b.tag_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// OpenAPI fragment for the catalog resource-tag router (Access P3.2). Merged
/// into `egret_bi::openapi()`; reuses the `control-plane-catalog` tag.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_resource_tags, assign_resource_tag, unassign_resource_tag),
    components(schemas(ResourceTagDto, ResourceTagBody))
)]
pub struct CatalogTagsApi;

#[cfg(test)]
mod tests {
    use super::unresolvable_coordinate;

    /// **The write-side half of migration 0177, asserted by LEVEL.** This is the
    /// guard that keeps a bad `PUT …/resource-tags` a 400 instead of the CHECK
    /// surfacing as a 500, so what matters is that it refuses exactly the set the
    /// constraint refuses — no more, no less. A mutation that drops any arm shows up
    /// here; one that widens it (refusing an empty `column`) shows up in the
    /// sentinel assertions below, which are the ones the product depends on.
    #[test]
    fn it_refuses_exactly_what_the_check_refuses() {
        // The two empty levels, named so the caller knows which box to fix.
        assert_eq!(
            unresolvable_coordinate("", "orders", ""),
            Some("schema must not be empty".to_string())
        );
        assert_eq!(
            unresolvable_coordinate("sales", "", ""),
            Some("table must not be empty".to_string())
        );
        // Both metacharacters, at each of the three levels.
        for (schema, table, column, level) in [
            ("sal*s", "orders", "", "schema"),
            ("sale?", "orders", "", "schema"),
            ("sales", "ord*", "", "table"),
            ("sales", "order?", "", "table"),
            ("sales", "orders", "ema*", "column"),
            ("sales", "orders", "e?ail", "column"),
        ] {
            let got = unresolvable_coordinate(schema, table, column)
                .unwrap_or_else(|| panic!("({schema:?}, {table:?}, {column:?}) must be refused"));
            assert!(
                got.starts_with(level),
                "the message must name the offending LEVEL, got {got:?} for \
                 ({schema:?}, {table:?}, {column:?})"
            );
        }

        // ⚠ The sentinel. `column_name = ''` is migration 0081's table-level
        // attachment and `assign_resource_tag` writes it whenever the request omits
        // `column`, so a guard that refused every empty string would break the
        // product — and would also refuse rows the CHECK accepts.
        assert_eq!(unresolvable_coordinate("sales", "orders", ""), None);
        assert_eq!(unresolvable_coordinate("sales", "orders", "email"), None);
        // Case and non-ASCII are reproduced by the resolver's fold, not refused.
        assert_eq!(unresolvable_coordinate("SALES", "ORDERS", "EMAIL"), None);
        assert_eq!(unresolvable_coordinate("bán_hàng", "người", ""), None);
        // And a fold-divergent value STORES: that class is a read-time decline in
        // `egret_perm_compile`'s gate, never a write refusal, because the 25 code
        // points are legitimate identifiers.
        assert_eq!(unresolvable_coordinate("sales", "orders", "emaıl"), None);
    }
}
