//! Postgres access: a pooled connection wrapper and the embedded migrator.
//! (touched to re-embed migrations: 0037 removes notebook engine column; 0040_user_username adds users.username citext; 0041_kernels adds kernels table; 0042_notebook_kernels adds notebook_kernels table; 0043_kernel_token adds token_hash + token_expires_at; 0044_oidc_refresh_tokens adds oidc_refresh_tokens table; 0045 adds unique index on workspace_warehouses.warehouse_id; 0046 adds notebook_cells.sql_target; 0047 drops notebook_cells.sql_target; 0090 re-adds notebook_cells.sql_target + adds sql_into)
// migrations: through 0067 (E5b: analytics + kernel schemas); 0088 typed_secrets (multi-field secret_data); 0089 engine_tls_cert_ref
// re-embed: 0081 catalog_tag_targets (catalog table/column tag assignments)
// re-embed: 0082 classification_policies (tag-conditioned BI object access; Access P3.3)
// re-embed: 0083 classification_required_groups (clearance by group/department; Access P3.4)
// re-embed: 0092 drop_oidc_refresh_tokens (STS cleanup: dead Keycloak refresh path)
// re-embed: 0129 dataapi_endpoint_statements (dataapi.endpoints.sql -> statements jsonb)

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

// migration 0023: per-workspace normalized catalog
// migration 0024: catalog perms
// migration 0025: workspace_warehouse mapping
// migration 0026: drop legacy catalog metadata tables (lakekeeper is the engine)
// migration 0027: per-token permissions on catalog_tokens
// migration 0028: per-token namespace_scope on catalog_tokens
// migration 0031: per-workspace DB connections (databases.workspace_id; drop workspace_database_grant)
// migration 0096: analytics.sql_snippets (SQL Lab reusable snippets)
// migration 0097: dataset column/metric semantic-layer config (groupby/filterable/format/certified)
// migration 0098: dataset semantic_type + certification metadata (dataset + metric certified_by/details)
// re-embed: 0104 dataset_semantic_type_allowlist (CHECK on dataset_columns.semantic_type; normalises unknown values to '' first)
// re-embed: 0105 dashboard_width_default_fixed (new dashboards default to Metabase's fixed canvas)
// re-embed: 0106 dashboard_attribution (dashboards.created_at nullable + updated_by)
// re-embed: 0107 dashboard_revisions (per-dashboard snapshot history, count-bounded at 20)
// re-embed: 0108 dashboard_certification (certified/certified_by/certification_details + the
// dashboard:certify seed for data_steward)
// re-embed: 0109 dataset_certify_permission (dataset:certify seed for data_steward; closes
// self-certification AND client-supplied certified_by on datasets and metrics)
// re-embed: 0128 dataapi_alerts (third schedules.type, dataapi_alert, watching
// dataapi.access_log; widens schedule_type_valid, adds 4 dataapi_* columns and
// their shape CHECKs, plus access_log_ws_ep_ts_idx — renumbered from 0127 to
// 0128 during the rebase onto main, which had independently taken 0127 for
// render_tokens_workspace_id)
// re-embed: 0130 database_governed (query.databases.governed — the per-database
// switch that makes a Data API publish require a per-principal connection;
// DEFAULT false so the migration changes no live endpoint's behaviour)
// re-embed: 0176-0179 (the four SP4d guardrail CHECKs: tag_service requires a
// schema level; catalog tag levels are present and literal; perm scope levels
// are named; perm wildcard levels name nothing beneath) — renumbered from
// 0165-0168 during this branch's rebase onto main, which had independently
// taken all four, exactly as it had taken 0127 above. Relative order is
// load-bearing: 0178 refuses the unnamed level that 0179's `^[*]+$` admits, so
// 0178 must still be applied first. See
// `no_two_migration_files_share_a_version`, which is the gate that now sees
// this class.
/// All SQL migrations, embedded at compile time from the workspace `migrations/` dir.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// A thin handle around the shared Postgres pool.
#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    /// Connect and build the pool.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Build a `Db` handle without opening a connection: the pool connects
    /// lazily on first use, so this only parses `url` and never touches the
    /// network. For tests that need a `Db` to satisfy a constructor (e.g.
    /// building a full `WorkspaceState` to check that routes are mounted)
    /// without ever issuing a query against a live database.
    pub fn connect_lazy(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new().connect_lazy(url)?;
        Ok(Self { pool })
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<(), MigrateError> {
        MIGRATOR.run(&self.pool).await
    }

    /// Cheap liveness probe used by `/readyz`.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every migration on disk is embedded in this binary.
    ///
    /// The previous form asserted `len() >= 7`, which is true of any tree that ever
    /// had seven migrations — so it stayed green while the binary carried 130 and
    /// the directory held 131, and the only symptom was every Data API Versions
    /// request answering `500 — column v.compared_against does not exist`.
    /// **A floor cannot see a gap; only equality can.**
    ///
    /// `build.rs` is what keeps the two equal by construction. This test is the
    /// backstop, because a `cargo:rerun-if-changed` that quietly stops working looks
    /// exactly like one that is doing its job.
    #[test]
    fn every_migration_on_disk_is_embedded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
        let on_disk: std::collections::BTreeSet<i64> = std::fs::read_dir(&dir)
            .expect("migrations/ is readable from the crate dir")
            .map(|e| e.expect("readable entry").file_name())
            .filter_map(|n| {
                // `0131_dataapi_version_breaking_changes.sql` -> 131
                let n = n.to_string_lossy().into_owned();
                n.strip_suffix(".sql")
                    .and_then(|s| s.split('_').next())
                    .and_then(|v| v.parse::<i64>().ok())
            })
            .collect();

        // Without this, an unreadable directory would yield an empty set that
        // trivially satisfies the check below — the instrument reporting "nothing
        // missing" because it saw nothing at all.
        assert!(
            !on_disk.is_empty(),
            "read no migrations from {dir:?}; the test's own instrument is broken"
        );

        let embedded: std::collections::BTreeSet<i64> =
            MIGRATOR.migrations.iter().map(|m| m.version).collect();
        let missing: Vec<i64> = on_disk.difference(&embedded).copied().collect();

        assert!(
            missing.is_empty(),
            "migrations {missing:?} exist on disk but are NOT embedded, so they will never \
             run: `sqlx::migrate!` reads the directory at COMPILE time. Rebuild — `build.rs` \
             exists to force that — and if a rebuild does not fix it, the \
             `cargo:rerun-if-changed` there has stopped working."
        );
    }

    /// No two migration files share a version number.
    ///
    /// **This closes a hole in the gate above, which cannot see this class at
    /// all.** `every_migration_on_disk_is_embedded` reduces the directory to a
    /// `BTreeSet<i64>` and asserts a set DIFFERENCE, so two files both numbered
    /// `0176` are ONE element: the duplicate is never represented, the difference
    /// is empty, and the gate reports success. A set cannot count, so the only
    /// gate that exists is blind to a duplicated integer by construction.
    ///
    /// The class is neither hypothetical nor rare in this repo.
    /// `0128_dataapi_alerts` was renumbered from 0127 during a rebase because main
    /// had independently taken 0127 for `render_tokens_workspace_id`, and SP4d's
    /// four guardrails were renumbered from 0165-0168 to 0176-0179 for the same
    /// reason — main had taken all four. A collision survives review because it is
    /// INVISIBLE from the branch that creates it: the merge-base's highest version
    /// is lower than every number the branch picks, so the branch's own tree looks
    /// consistent. Only main's tree disagrees.
    ///
    /// What a duplicate costs: `_sqlx_migrations.version` is a `BIGINT PRIMARY
    /// KEY`, so the SECOND file to carry a version aborts `migrate()` at
    /// `ExecuteMigration(.., version)` on a fresh database. Every migration after
    /// it never runs — and every `CHECK` constraint those migrations were going to
    /// add is silently absent while the Rust half of the same guardrail ships
    /// green, which is a branch enforcing half of what it claims.
    ///
    /// This asserts on FILENAMES rather than on `MIGRATOR.migrations` because
    /// `sqlx::migrate!` does not reject a duplicate version at compile time; by
    /// the time the set is embedded the two files are indistinguishable, so the
    /// directory is the only place the duplicate still exists.
    #[test]
    fn no_two_migration_files_share_a_version() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");

        let mut by_version: std::collections::BTreeMap<i64, Vec<String>> =
            std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(&dir).expect("migrations/ is readable from the crate dir") {
            let name = entry
                .expect("readable entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            // `0131_dataapi_version_breaking_changes.sql` -> 131
            let Some(version) = name
                .strip_suffix(".sql")
                .and_then(|s| s.split('_').next())
                .and_then(|v| v.parse::<i64>().ok())
            else {
                continue;
            };
            by_version.entry(version).or_default().push(name);
        }

        // The same guard the test above carries, for the same reason: an unreadable
        // or empty directory yields a map with no duplicate in it, and this
        // assertion would ACQUIT having measured nothing.
        assert!(
            !by_version.is_empty(),
            "read no migrations from {dir:?}; the test's own instrument is broken"
        );

        let duplicated: Vec<(i64, Vec<String>)> = by_version
            .into_iter()
            .filter(|(_, files)| files.len() > 1)
            .map(|(version, mut files)| {
                files.sort();
                (version, files)
            })
            .collect();

        assert!(
            duplicated.is_empty(),
            "two or more migration files share a version: {duplicated:?}. \
             `_sqlx_migrations.version` is a BIGINT PRIMARY KEY, so the second file \
             to carry a version aborts `migrate()` at `ExecuteMigration` and every \
             migration after it never runs. Renumber the newer file(s) above the \
             highest version on `origin/main` — check MAIN and not the merge-base, \
             because a branch cannot see the numbers main took after it forked."
        );
    }
}

// migration registry: 0033_databases_name_per_workspace
// migration registry: 0039_notebook_resource_access
// migration registry: 0040_user_username
// migration registry: 0062_engine_mgmt_schema (moves engines/engine_catalogs to engine_mgmt schema)
// migration registry: 0077_saved_query_objects (saved_queries.folder_id + saved_query:{read,manage} grants)
// migration registry: 0078_access_grant_expires_at (time-bound grants: access_grants.expires_at + partial index)
// migration registry: 0079 access_requests (join-request → approval workflow: access_requests table)
// migration registry: 0080 tags (classification model: tags + tag_assignments + tag:* perms)
// migration registry: 0085 engine_delegator_secret (impersonate-mode delegator secret ref)
// migration registry: 0086 engine_tls (jwt-mode coordinator TLS: tls_secret_id + coordinator_cert)
// migration registry: 0087 engine_root_secret (secure-by-default StarRocks FE root password ref)
