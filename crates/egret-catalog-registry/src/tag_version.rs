//! The single service-resolution behind Ranger tag delivery: which policy
//! services a warehouse's catalog-tag export reaches, and therefore whose
//! `tag_version` has to move when that export changes.
//!
//! Two places read this link and must agree. The BUMP — the assign/unassign
//! bump (`egret_bi::CatalogTagRepo`) — lives here, in the lowest crate that
//! reaches it. The other, the export
//! (`egret_policy::PolicyStore::tag_assignments_for_service`), is a SELECT of
//! a different shape (it returns catalog NAMES per tag assignment, keyed by
//! the polled service) and keeps its own SQL; the doc below states branch for
//! branch what it has to stay equal to, and
//! `egret-policy/tests/download_tags_*.rs` is what holds the two together.

use uuid::Uuid;

/// Bump `policy.authz_services.tag_version` for every policy service bound to
/// `warehouse_id`, AND for the tag service each such service is actually
/// POLLED through. A monotonic `tag_version` is how the Ranger tag-download
/// plugin (which polls `download_tags?lastKnownVersion=`) learns a warehouse's
/// catalog-tag assignments changed; without the bump it 304s forever. Runs on
/// any executor — a caller's transaction, or the pool.
///
/// `profiles` is the SAME source `PolicyStore::tag_assignments_for_service`
/// serves under (SP3 catalog-registry review C1/I1) — an engine's service
/// binds to this warehouse if the engine attaches this warehouse's registry
/// catalog through `registry.engine_catalog_attachments` (SP3 D5), NOT only
/// as its primary. Missing that reach was fail-open for UPDATES specifically:
/// a tag change on a warehouse an engine attaches only as a SECONDARY catalog
/// never bumped that engine's service, so `download_tags` kept answering 304
/// and the new mask/tag never reached the plugin until an unrelated restart.
///
/// One branch only, since SP3c-2: the engine attaches this warehouse's
/// registry catalog through `registry.engine_catalog_attachments`.
///
/// Until SP3c-2 a second branch read `engine_mgmt.engine_catalogs` for two
/// populations the registry could not see: engines whose backfill failed,
/// and a catalog renamed but not yet re-applied. Both are gone — the backfill
/// is deleted, so no engine can be in the first state, and
/// `registry.catalogs.name` is UNIQUE and immutable, so nothing renames a
/// catalog at all.
///
/// SP3 catalog-registry review N1: bumping only `profiles` is NOT enough. The
/// Trino Ranger tag plugin polls the TAG service, not the trino profile —
/// `download_tags` (`egret-policy/src/ranger/api/download.rs`) gates its 304
/// on **the polled service's own** `tag_version`, and a trino profile's tag
/// service is `profiles.config_jsonb->>'tagService'` (the SAME field
/// `tag_assignments_for_service`'s `LEFT JOIN … tp.config_jsonb->>'tagService'
/// = s.name` reads on the serving side, and the field
/// `egret-engine-mgmt/src/engines.rs`'s trino-profile auto-bind writes when it
/// creates the paired tag service). So the tag service named by each resolved
/// profile's `tagService` config is bumped too. Bumping the profile itself is
/// kept: a service with no trino/tag split (bound to directly) has no separate
/// tag service, and the direct bump is the only one that reaches it.
pub async fn bump_bound_tag_versions<'e, E>(
    executor: E,
    warehouse_id: Uuid,
) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query(
        "WITH profiles AS ( \
             SELECT DISTINCT e.authz_service_id AS id \
             FROM registry.catalogs c \
             JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
             JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
             WHERE c.warehouse_id = $1 AND e.authz_service_id IS NOT NULL \
         ) \
         UPDATE policy.authz_services s \
            SET tag_version = tag_version + 1, updated_at = now() \
          WHERE s.id IN (SELECT id FROM profiles) \
             OR s.name IN ( \
                 SELECT tp.config_jsonb ->> 'tagService' \
                 FROM policy.authz_services tp \
                 WHERE tp.id IN (SELECT id FROM profiles) \
                   AND tp.config_jsonb ->> 'tagService' IS NOT NULL \
             )",
    )
    .bind(warehouse_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// The policy-profile service ids `warehouse_id`'s engine(s) are bound
/// through — the SAME `profiles` resolution [`bump_bound_tag_versions`] uses,
/// minus its tag-service fan-out leg. SP4a Task 7's recompile trigger calls
/// this (from `egret-bi::catalog_tags`, alongside the `bump_bound_tag_versions`
/// call already there) to learn which `policy.authz_services` rows to
/// recompile after a catalog-tag write, and deliberately does NOT also
/// recompile the paired tag service: `perm.compiled`/`policy.authz_policies`
/// hold compiled access/masking/row-filter policies under the PROFILE
/// service, never under its tag service, so there is nothing there for a
/// recompile to touch. Runs on any executor — a caller's transaction, or the
/// pool.
/// Ordered by id (SP4a Task 9): the caller recompiles each of these in turn,
/// and `egret_perm_compile`'s `apply_diff_in_tx` takes a row lock per service.
/// Ascending acquisition order across every producer of such a list is what
/// keeps two overlapping recompiles queueing instead of deadlocking.
pub async fn services_bound_to_warehouse<'e, E>(
    executor: E,
    warehouse_id: Uuid,
) -> Result<Vec<i64>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT e.authz_service_id \
         FROM registry.catalogs c \
         JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
         JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
         WHERE c.warehouse_id = $1 AND e.authz_service_id IS NOT NULL \
         ORDER BY 1",
    )
    .bind(warehouse_id)
    .fetch_all(executor)
    .await
}
