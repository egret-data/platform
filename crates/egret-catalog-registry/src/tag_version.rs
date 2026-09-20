//! The single service-resolution behind Ranger tag delivery: which policy
//! services a warehouse's catalog-tag export reaches, and therefore whose
//! `tag_version` has to move when that export changes.
//!
//! Three readers of this link exist and must agree: the export
//! (`egret_policy::PolicyStore::tag_assignments_for_service`), the
//! assign/unassign bump (`egret_bi::CatalogTagRepo`), and the boot backfill
//! (`crate::backfill`). The bump lives here, in the lowest crate all of them
//! can reach, so there is one implementation rather than three copies to drift.

use uuid::Uuid;

/// Bump `policy.authz_services.tag_version` for every policy service bound to
/// `warehouse_id`, AND for the tag service each such service is actually
/// POLLED through. A monotonic `tag_version` is how the Ranger tag-download
/// plugin (which polls `download_tags?lastKnownVersion=`) learns a warehouse's
/// catalog-tag assignments changed; without the bump it 304s forever. Runs on
/// any executor — a caller's transaction, or the pool.
///
/// `profiles` is the SAME union `PolicyStore::tag_assignments_for_service`
/// serves under (SP3 catalog-registry review C1/I1) — an engine's service
/// binds to this warehouse if EITHER:
///   (a) the legacy branch — `engine_mgmt.engines.authz_service_id`, this
///       warehouse is the engine's own primary (`engines.warehouse_id`); or
///   (b) the registry branch — the engine attaches this warehouse's registry
///       catalog through `registry.engine_catalog_attachments` (SP3 D5), NOT
///       only as its primary.
/// Missing (b) was fail-open for UPDATES specifically: a tag change on a
/// warehouse an engine attaches only as a SECONDARY catalog never bumped that
/// engine's service, so `download_tags` kept answering 304 and the new
/// mask/tag never reached the plugin until an unrelated restart. The legacy
/// branch stays until `engine_catalogs` is retired (SP3c), for the same
/// un-migrated / rename-window reasons documented on
/// `tag_assignments_for_service`.
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
             FROM engine_mgmt.engines e \
             WHERE e.warehouse_id = $1 AND e.authz_service_id IS NOT NULL \
             UNION \
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
