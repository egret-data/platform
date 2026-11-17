//! SP5b Task 5: the unified enforcement path for the embedded catalog.
//!
//! Behind a per-catalog switch ([`catalog_grant_source`]), the request guard
//! (`egret-bin`'s `EgretCatalogGuard::check`) and the lakekeeper authorizer
//! ([`crate::authorizer::EgretAuthorizer::client_may`]) decide a catalog
//! client's data access by evaluating its **service-account principal** against
//! the folded `perm.grants` of the catalog via `perm-eval`, instead of reading
//! `catalog.catalog_grants` by the token's role. The two must agree: Task 2's
//! fold wrote the equivalent grants, and [`covers`] reproduces the boolean shape
//! of [`crate::grant_match::catalog_grant_covers_by`] on the unified store.
//!
//! ## The op → decision mapping ([`covers`])
//!
//! A catalog request names `(namespace, table?, is_write)`. The namespace is
//! dot-joined into a single `perm` schema, exactly as Task 1's `map_securable`
//! folded it (`["sales","eu"]` → `sales.eu`), so a grant and a request that
//! named the same Iceberg namespace meet at the same schema string.
//!
//! | request | `catalog_grant_covers_by` | `perm-eval` |
//! |---|---|---|
//! | table, read  | read on the table | `decide_privilege(Read, table)` |
//! | table, write | write on the table | `decide_privilege(Write, table)` |
//! | namespace, write | write on the namespace | `decide_privilege(Write, schema)` |
//! | namespace, read (listing) | reach of the namespace | `reaches(schema)` |
//! | catalog root, read | any grant | `reaches(catalog)` |
//!
//! The namespace-read case is **reach**, not `decide_privilege(Read)`: a
//! table-grant holder may list/load the enclosing namespace
//! (`catalog_grant_covers_by(.., table=None, is_write=false)` returns true for a
//! `table` grant on that exact namespace), and only reach — "some privilege on
//! the object or below it" — reproduces that, since a `decide_privilege(Read)`
//! on the schema would find no allow ON the schema. Dot-join makes reach and the
//! legacy rule agree on the guard's inputs: there is no prefix relationship
//! between `sales` and `sales.eu` as whole schema strings, so neither rule sees
//! a parent namespace from a child grant (that broader `grant_sees_namespace`
//! visibility is `filter_listing`'s, which this task does not switch).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use egret_perm_eval::trino_ops::DataPrivilege;
use egret_perm_eval::{Decision, ObjectPath, Resource, Snapshot};
use uuid::Uuid;

use crate::store::CatalogGrant;

/// Which store the embedded-catalog enforcement reads for one catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `catalog.catalog_grants` by the token's role — the legacy path, and the
    /// default for a catalog with no switch row.
    CatalogRbac,
    /// The folded `perm.grants` of the catalog, via `perm-eval`.
    Unified,
}

/// The switch for `catalog_id`, defaulting to [`Source::CatalogRbac`] when no
/// row exists (so shipping migration 0235 enables nothing on its own). An
/// unrecognised `source` value is also read as `CatalogRbac` (fail safe: the
/// legacy path is the proven one).
pub async fn catalog_grant_source(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Source, sqlx::Error> {
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM perm.catalog_grant_switch WHERE catalog_id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(match source.as_deref() {
        Some("unified") => Source::Unified,
        _ => Source::CatalogRbac,
    })
}

/// True when `catalog_id` has opted into iceberg-rest rule enforcement
/// (`perm.catalog_rule_enforcement_switch`, migration 0237). A catalog with no
/// row returns false: opt-in, so shipping the migration enables nothing.
pub async fn catalog_enforces_rules(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let on: Option<bool> = sqlx::query_scalar(
        "SELECT enforce FROM perm.catalog_rule_enforcement_switch WHERE catalog_id = $1",
    )
    .bind(catalog_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(on.unwrap_or(false))
}

/// Does `principal_user` (a `users.username`) cover the requested
/// `(namespace, table, is_write)` on `catalog_name`, under `snapshot`? The same
/// boolean [`crate::grant_match::catalog_grant_covers_by`] returns for the
/// matching legacy grant set. See the module docs for the mapping.
pub fn covers(
    snapshot: &Snapshot,
    principal_user: &str,
    catalog_name: &str,
    namespace: &[String],
    table: Option<&str>,
    is_write: bool,
) -> bool {
    let schema = (!namespace.is_empty()).then(|| namespace.join("."));
    let allowed = |d: Decision| matches!(d, Decision::Allow { .. });
    match (table, is_write) {
        // A specific table: read or write of that table.
        (Some(t), write) => {
            let object = ObjectPath {
                catalog: catalog_name.to_string(),
                schema,
                table: Some(t.to_string()),
                column: None,
            };
            let privilege = if write {
                DataPrivilege::Write
            } else {
                DataPrivilege::Read
            };
            allowed(snapshot.decide_privilege(principal_user, privilege, &object))
        }
        // A namespace write (create a table in it, rename into it, …): write ON
        // the namespace, which a `table` grant below it does not confer.
        (None, true) => {
            let object = ObjectPath {
                catalog: catalog_name.to_string(),
                schema,
                table: None,
                column: None,
            };
            allowed(snapshot.decide_privilege(principal_user, DataPrivilege::Write, &object))
        }
        // A namespace/catalog read (listing, loading a namespace): reach.
        (None, false) => {
            let resource = match schema {
                Some(s) => Resource::Schema {
                    catalog: catalog_name.to_string(),
                    schema: s,
                },
                None => Resource::Catalog(catalog_name.to_string()),
            };
            snapshot.reaches(principal_user, &resource)
        }
    }
}

/// The `users.username` of the service account that owns the catalog client
/// `client_id` (the token's `sub`), or `None` when the client names no service
/// account. `perm` matches a `user` principal by this username, so it is the
/// principal the unified path evaluates.
///
/// `ensure_username_via` returns the stored username (the one the fold wrote);
/// it mints only when none exists, which post-fold does not happen.
pub async fn service_account_username(
    conn: &mut sqlx::PgConnection,
    client_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let sa: Option<Uuid> = sqlx::query_scalar(
        "SELECT service_account_id FROM catalog.catalog_clients WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let Some(sa) = sa else { return Ok(None) };
    match egret_auth::users::ensure_username_via(conn, egret_core::UserId(sa)).await {
        Ok(username) => Ok(Some(username)),
        // No username and none mintable: the principal cannot be named, so no
        // unified grant can match it — deny (the caller treats `None` as deny).
        Err(_) => Ok(None),
    }
}

/// Resolve, in ONE indexed query, the registry catalog for `warehouse_id` and
/// which store decides it. `None` catalog ⇒ the warehouse has no registry
/// catalog; the source is then [`Source::CatalogRbac`]. Folding the switch read
/// into the catalog lookup keeps the legacy hot path at ONE query, not two.
pub async fn resolve_catalog_switch(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
) -> Result<(Option<(Uuid, String)>, Source), sqlx::Error> {
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT c.id, c.name, COALESCE(s.source, 'catalog_rbac') \
           FROM registry.catalogs c \
           LEFT JOIN perm.catalog_grant_switch s ON s.catalog_id = c.id \
          WHERE c.warehouse_id = $1",
    )
    .bind(warehouse_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(match row {
        Some((id, name, source)) => {
            let source = if source == "unified" {
                Source::Unified
            } else {
                Source::CatalogRbac
            };
            (Some((id, name)), source)
        }
        None => (None, Source::CatalogRbac),
    })
}

/// Reconstruct, from `perm.grants`, the catalog-shaped grants of the service
/// account `username` on `catalog_id` — the SAME shape
/// [`crate::store::CatalogStore::grants_for_role`] returns, so `filter_listing`
/// can run the EXACT legacy visibility rules
/// ([`crate::grant_match::grant_sees_namespace`] /
/// [`crate::grant_match::catalog_grant_covers_by`]) on a unified catalog whose
/// `catalog.catalog_grants` may already be dropped (Task 6 finalize). The
/// flattened `perm-eval` snapshot cannot reproduce `grant_sees_namespace`'s
/// above/under navigation (the dot-join erases the level hierarchy), so the
/// reconstruction is what keeps listings exact.
///
/// Only folded ALLOW grants naming this user (`user` principal, `include`, not
/// `exclude`) are returned — the fold writes only `user` principals, so this is
/// exact. `perm.securable_owners` is NOT reconstructed: legacy `grants_for_role`
/// has no owner concept, and a service account is never a securable owner.
pub async fn unified_listing_grants(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
    username: &str,
) -> Result<Vec<CatalogGrant>, sqlx::Error> {
    // (grant id, schema_name, table_name, actions).
    type GrantRow = (Uuid, Option<String>, Option<String>, Vec<String>);
    let rows: Vec<GrantRow> = sqlx::query_as(
        "SELECT g.id, g.schema_name, g.table_name, g.actions \
           FROM perm.grants g \
          WHERE g.catalog_id = $1 AND g.effect = 'allow' \
            AND EXISTS (SELECT 1 FROM perm.grant_principals p \
                         WHERE p.grant_id = g.id AND p.kind = 'user' \
                           AND p.mode = 'include' AND p.ref = $2) \
            AND NOT EXISTS (SELECT 1 FROM perm.grant_principals p \
                             WHERE p.grant_id = g.id AND p.kind = 'user' \
                               AND p.mode = 'exclude' AND p.ref = $2)",
    )
    .bind(catalog_id)
    .bind(username)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, schema, table, actions)| {
            let securable_type = match (&schema, &table) {
                (None, _) => "catalog",
                (Some(_), None) => "namespace",
                (Some(_), Some(_)) => "table",
            };
            // grant_sees_namespace ignores privilege; catalog_grant_covers_by
            // only asks grants_write (write|manage). "write" captures both.
            let privilege = if actions.iter().any(|a| a == "write" || a == "manage") {
                "write"
            } else {
                "read"
            };
            CatalogGrant {
                id,
                catalog_role_id: Uuid::nil(),
                securable_type: securable_type.to_string(),
                namespace: schema.map(|s| s.split('.').map(str::to_string).collect()),
                table_name: table,
                privilege: privilege.to_string(),
            }
        })
        .collect())
}

/// Whether the unified path matches names IDENTICALLY to legacy for `catalog_id`:
/// true iff every folded name is ASCII, where `str::to_lowercase` (perm-eval's
/// name matching) reproduces lakekeeper's `case_insensitive` collation EXACTLY.
///
/// lakekeeper's collation is the FIXED ICU `und-u-ks-level2`: case-, width-,
/// compatibility- and canonical-insensitive, accent-SENSITIVE — never
/// case-sensitive, so the hypothesised "grant `sales`, request `Sales`" never
/// over-grants (both fold case). The unified path NEVER over-grants in general:
/// simple lowercase identifies only case variants, a subset of what the level-2
/// collation folds, so `to_lowercase`-equal ⟹ collation-equal. The ONE residual
/// divergence is a NON-ASCII name where the collation folds a width/compatibility/
/// canonical form that `to_lowercase` leaves distinct — there the unified path
/// UNDER-grants (fail-closed). This helper lets Task 6's finalize gate refuse to
/// drop `catalog.catalog_grants` for a catalog with any non-ASCII folded name, so
/// a finalized unified catalog is EXACTLY equivalent to legacy.
pub async fn unified_name_matching_is_exact(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let all_ascii: Option<bool> = sqlx::query_scalar(
        "SELECT bool_and(ascii_ok) FROM ( \
           SELECT (schema_name IS NULL OR schema_name ~ '^[[:ascii:]]*$') \
              AND (table_name  IS NULL OR table_name  ~ '^[[:ascii:]]*$') \
              AND (column_name IS NULL OR column_name ~ '^[[:ascii:]]*$') AS ascii_ok \
             FROM perm.grants WHERE catalog_id = $1 \
           UNION ALL \
           SELECT (schema_name IS NULL OR schema_name ~ '^[[:ascii:]]*$') \
              AND (table_name  IS NULL OR table_name  ~ '^[[:ascii:]]*$') \
              AND (column_name IS NULL OR column_name ~ '^[[:ascii:]]*$') \
             FROM perm.securable_owners WHERE catalog_id = $1 \
         ) q",
    )
    .bind(catalog_id)
    .fetch_one(&mut *conn)
    .await?;
    // No rows ⇒ nothing to match ⇒ trivially exact.
    Ok(all_ascii.unwrap_or(true))
}

/// A per-catalog freshness signal over the store the fold wrote: any add bumps a
/// count (and, for grants and owners, a max-timestamp), any delete bumps a
/// count, so a changed catalog never reuses a stale snapshot.
/// `principal_max` is always `None`: `perm.grant_principals` has no timestamp. Cheap: both tables are indexed by
/// `catalog_id` (`grants_catalog_idx`, the owners' unique key).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    grant_count: i64,
    grant_max: Option<DateTime<Utc>>,
    principal_count: i64,
    principal_max: Option<DateTime<Utc>>,
    owner_count: i64,
    owner_max: Option<DateTime<Utc>>,
}

/// The six scalars `version_of` reads in one row: a (count, max-timestamp) pair
/// for grants, grant principals and owners (the principal timestamp is a NULL
/// placeholder, see `version_of`).
type VersionRow = (
    i64,
    Option<DateTime<Utc>>,
    i64,
    Option<DateTime<Utc>>,
    i64,
    Option<DateTime<Utc>>,
);

async fn version_of(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Version, sqlx::Error> {
    let row: VersionRow = sqlx::query_as(
        "SELECT \
               (SELECT count(*) FROM perm.grants WHERE catalog_id = $1), \
               (SELECT max(created_at) FROM perm.grants WHERE catalog_id = $1), \
               (SELECT count(*) FROM perm.grant_principals gp \
                  JOIN perm.grants g ON g.id = gp.grant_id WHERE g.catalog_id = $1), \
               NULL::timestamptz, /* perm.grant_principals has no created_at */ \
               (SELECT count(*) FROM perm.securable_owners WHERE catalog_id = $1), \
               (SELECT max(updated_at) FROM perm.securable_owners WHERE catalog_id = $1)",
    )
    .bind(catalog_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Version {
        grant_count: row.0,
        grant_max: row.1,
        principal_count: row.2,
        principal_max: row.3,
        owner_count: row.4,
        owner_max: row.5,
    })
}

struct Cached {
    version: Version,
    snapshot: Option<Arc<Snapshot>>,
}

/// A per-catalog cache of the unified [`Snapshot`], keyed by `catalog_id` and
/// invalidated by [`Version`]. The build (grant load + IR lowering) runs only on
/// a miss or a version change; a hit is one cheap indexed [`version_of`] query,
/// comparable in cost to the legacy `grants_for_role` read.
///
/// A built snapshot can be `None` — the catalog has no registry row — which is
/// cached too, so a deleted catalog does not rebuild on every request; it is a
/// deny (nothing covers).
#[derive(Clone, Default)]
pub struct CatalogSnapshotCache {
    entries: Arc<Mutex<HashMap<Uuid, Cached>>>,
}

impl std::fmt::Debug for CatalogSnapshotCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogSnapshotCache")
            .finish_non_exhaustive()
    }
}

impl CatalogSnapshotCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_fresh(&self, catalog_id: Uuid, version: &Version) -> Option<Option<Arc<Snapshot>>> {
        let map = self.entries.lock().expect("cache mutex");
        map.get(&catalog_id)
            .filter(|c| &c.version == version)
            .map(|c| c.snapshot.clone())
    }

    /// The current unified snapshot for `catalog_id`, rebuilt only when the
    /// catalog's grants/owners have changed since the cached one. `Ok(None)`
    /// means the catalog has no registry row.
    pub async fn get(
        &self,
        conn: &mut sqlx::PgConnection,
        catalog_id: Uuid,
    ) -> Result<Option<Arc<Snapshot>>, egret_perm::CompileError> {
        let version = version_of(conn, catalog_id)
            .await
            .map_err(egret_perm::CompileError::from_db)?;
        if let Some(hit) = self.get_fresh(catalog_id, &version) {
            return Ok(hit);
        }
        let snapshot = egret_perm_compile::opa::build::build_catalog_snapshot_via(conn, catalog_id)
            .await?
            .map(Arc::new);
        self.entries.lock().expect("cache mutex").insert(
            catalog_id,
            Cached {
                version,
                snapshot: snapshot.clone(),
            },
        );
        Ok(snapshot)
    }
}

/// The freshness key of the enforcement snapshot: a content hash of every
/// stored input `build_catalog_enforcement_snapshot` reads directly (rules and
/// their exemptions, tags, this catalog's tag assignments, clearance policies).
/// Not [`Version`]: that tracks grants, which the enforcement snapshot never
/// reads, so a new mask or withhold rule would leave it unchanged and the stale
/// snapshot would report the newly restricted table as open. A content hash (not
/// count + max timestamp) because `perm.rules` has no `updated_at`: an in-place
/// edit of a mask expression must change the key too.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EnforcementVersion(String);

async fn enforcement_version_of(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<EnforcementVersion, sqlx::Error> {
    // Every rule (tag rules reach any catalog; withhold rules are read by the
    // clearance loader, not by catalog) is hashed whole: a superset of what the
    // build reads is safe, a subset is a leak.
    let hash: String = sqlx::query_scalar(
        "SELECT md5(concat_ws('#', \
           (SELECT coalesce(string_agg(r::text, '|' ORDER BY r.id), '') FROM perm.rules r), \
           (SELECT coalesce(string_agg(e::text, '|' ORDER BY e::text), '') \
              FROM perm.rule_exemptions e), \
           (SELECT coalesce(string_agg(t::text, '|' ORDER BY t.id), '') FROM tags t), \
           (SELECT coalesce(string_agg(a::text, '|' ORDER BY a::text), '') \
              FROM catalog_tag_assignments a \
              JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
             WHERE c.id = $1), \
           (SELECT coalesce(string_agg(cp::text, '|' ORDER BY cp::text), '') \
              FROM classification_policies cp)))",
    )
    .bind(catalog_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(EnforcementVersion(hash))
}

/// How long an enforcement snapshot may be reused even when its content hash is
/// unchanged. The hash covers the rules, exemptions, tags, assignments and
/// clearances, but not what the build reads transitively (group and role
/// membership behind an exemption or clearance, held-move column overlays); the
/// TTL bounds how stale those can be.
const ENFORCEMENT_TTL: std::time::Duration = std::time::Duration::from_secs(5);

struct CachedEnforcement {
    version: EnforcementVersion,
    built_at: std::time::Instant,
    snapshot: Option<Arc<Snapshot>>,
}

/// Like [`CatalogSnapshotCache`] but caches the ENFORCEMENT snapshot
/// (`build_catalog_enforcement_snapshot`: rules + withhold + classified, no
/// grants). It is a separate cache with a separate key: it is rebuilt when
/// [`enforcement_version_of`] changes (or after [`ENFORCEMENT_TTL`]), not when
/// grants change.
#[derive(Clone, Default)]
pub struct CatalogEnforcementSnapshotCache {
    entries: Arc<Mutex<HashMap<Uuid, CachedEnforcement>>>,
}

impl std::fmt::Debug for CatalogEnforcementSnapshotCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogEnforcementSnapshotCache")
            .finish_non_exhaustive()
    }
}

impl CatalogEnforcementSnapshotCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_fresh(
        &self,
        catalog_id: Uuid,
        version: &EnforcementVersion,
    ) -> Option<Option<Arc<Snapshot>>> {
        let map = self.entries.lock().expect("cache mutex");
        map.get(&catalog_id)
            .filter(|c| &c.version == version && c.built_at.elapsed() < ENFORCEMENT_TTL)
            .map(|c| c.snapshot.clone())
    }

    /// The current enforcement snapshot for `catalog_id`. `Err` (a restricting
    /// rule was skipped, so the catalog cannot be enforced) is propagated and
    /// never cached or read as `None`. `Ok(None)` means the catalog has no
    /// registry row.
    pub async fn get(
        &self,
        conn: &mut sqlx::PgConnection,
        catalog_id: Uuid,
    ) -> Result<Option<Arc<Snapshot>>, egret_perm::CompileError> {
        let version = enforcement_version_of(conn, catalog_id)
            .await
            .map_err(egret_perm::CompileError::from_db)?;
        if let Some(hit) = self.get_fresh(catalog_id, &version) {
            return Ok(hit);
        }
        let snapshot =
            egret_perm_compile::opa::build::build_catalog_enforcement_snapshot(conn, catalog_id)
                .await?
                .map(Arc::new);
        self.entries.lock().expect("cache mutex").insert(
            catalog_id,
            CachedEnforcement {
                version,
                built_at: std::time::Instant::now(),
                snapshot: snapshot.clone(),
            },
        );
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm_eval::{Snapshot, SnapshotInput};
    use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
    use egret_perm_ir::statement::{
        Derivation, Effect, Level, Path, Privilege, Source as StmtSource, Statement, Target,
    };

    const CAT: &str = "sales";

    fn member(user: &str) -> Member {
        Member {
            name: MemberName::User(user.to_string()),
            via: Via::Direct,
        }
    }

    fn allow(user: &str, privileges: Vec<Privilege>, schema: Level, table: Level) -> Statement {
        Statement {
            source: StmtSource::Grant {
                grant_id: Uuid::new_v4(),
            },
            audience: Audience {
                include: vec![member(user)],
                except: vec![],
            },
            target: Target::Path(Path {
                catalog_id: None,
                catalog: CAT.to_string(),
                schema,
                table,
                column: Level::Unnamed,
            }),
            effect: Effect::Allow { privileges },
            derivation: Derivation::AsDeclared,
        }
    }

    fn named(v: &str) -> Level {
        Level::Named(v.to_string())
    }

    fn snapshot(statements: Vec<Statement>) -> Snapshot {
        Snapshot::build(SnapshotInput {
            service_id: 0,
            key: (0, 0),
            statements,
            classified: vec![],
            lineage_principal: "egret-lineage".to_string(),
        })
        .expect("snapshot")
    }

    fn ns(levels: &[&str]) -> Vec<String> {
        levels.iter().map(|s| s.to_string()).collect()
    }

    // A table grant: read+write on sales(schema "corp.fin").ledger for "u".
    fn table_grant_snapshot() -> Snapshot {
        snapshot(vec![allow(
            "u",
            vec![Privilege::Read, Privilege::Write],
            named("corp.fin"),
            named("ledger"),
        )])
    }

    #[test]
    fn a_table_grant_covers_its_table_read_and_write() {
        let s = table_grant_snapshot();
        assert!(covers(
            &s,
            "u",
            CAT,
            &ns(&["corp", "fin"]),
            Some("ledger"),
            false
        ));
        assert!(covers(
            &s,
            "u",
            CAT,
            &ns(&["corp", "fin"]),
            Some("ledger"),
            true
        ));
    }

    #[test]
    fn a_table_grant_does_not_cover_a_sibling_table() {
        let s = table_grant_snapshot();
        assert!(!covers(
            &s,
            "u",
            CAT,
            &ns(&["corp", "fin"]),
            Some("other"),
            false
        ));
    }

    #[test]
    fn a_table_grant_holder_sees_the_enclosing_namespace_but_not_a_sibling() {
        let s = table_grant_snapshot();
        // Reach of the enclosing namespace (listing visibility) — true.
        assert!(covers(&s, "u", CAT, &ns(&["corp", "fin"]), None, false));
        // A sibling namespace — false.
        assert!(!covers(&s, "u", CAT, &ns(&["corp", "hr"]), None, false));
    }

    #[test]
    fn a_table_grant_does_not_confer_namespace_write() {
        let s = table_grant_snapshot();
        // Creating a table in the namespace needs write ON the namespace.
        assert!(!covers(&s, "u", CAT, &ns(&["corp", "fin"]), None, true));
    }

    #[test]
    fn a_read_only_grant_does_not_cover_writes() {
        let s = snapshot(vec![allow(
            "u",
            vec![Privilege::Read],
            named("corp.fin"),
            named("ledger"),
        )]);
        assert!(covers(
            &s,
            "u",
            CAT,
            &ns(&["corp", "fin"]),
            Some("ledger"),
            false
        ));
        assert!(!covers(
            &s,
            "u",
            CAT,
            &ns(&["corp", "fin"]),
            Some("ledger"),
            true
        ));
    }

    #[test]
    fn a_namespace_grant_covers_its_tables_and_its_own_write() {
        let s = snapshot(vec![allow(
            "u",
            vec![Privilege::Read, Privilege::Write],
            named("corp.fin"),
            Level::Unnamed,
        )]);
        // A table within the namespace.
        assert!(covers(
            &s,
            "u",
            CAT,
            &ns(&["corp", "fin"]),
            Some("ledger"),
            true
        ));
        // Namespace-level write (create a table).
        assert!(covers(&s, "u", CAT, &ns(&["corp", "fin"]), None, true));
        // Namespace listing.
        assert!(covers(&s, "u", CAT, &ns(&["corp", "fin"]), None, false));
    }

    #[test]
    fn a_catalog_grant_covers_everything_and_any_user_match_is_exact() {
        let s = snapshot(vec![allow(
            "u",
            vec![Privilege::Read, Privilege::Write],
            Level::Unnamed,
            Level::Unnamed,
        )]);
        assert!(covers(&s, "u", CAT, &ns(&["anything"]), Some("t"), true));
        assert!(covers(&s, "u", CAT, &ns(&["anything"]), None, false));
        // Catalog-root read: any grant reaches the catalog.
        assert!(covers(&s, "u", CAT, &[], None, false));
        // A different principal gets nothing.
        assert!(!covers(
            &s,
            "other",
            CAT,
            &ns(&["anything"]),
            Some("t"),
            false
        ));
    }

    #[test]
    fn a_nested_namespace_is_matched_by_its_dot_joined_form() {
        let s = snapshot(vec![allow(
            "u",
            vec![Privilege::Read],
            named("a.b.c"),
            Level::Unnamed,
        )]);
        assert!(covers(
            &s,
            "u",
            CAT,
            &ns(&["a", "b", "c"]),
            Some("t"),
            false
        ));
        assert!(!covers(&s, "u", CAT, &ns(&["a", "b"]), Some("t"), false));
    }
}
