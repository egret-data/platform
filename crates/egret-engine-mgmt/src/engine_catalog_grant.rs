//! Per-engine catalog grant (SP5b, Task 4).
//!
//! The shared per-warehouse `engine` role carries catalog-wide `manage`, so
//! every engine on a warehouse shares one entitlement. SP5b replaces that with
//! an explicit `{read,write,create,manage}` allow in `perm.grants` held by each
//! engine client's own service account, so engines keep working once the shared
//! role is removed at finalize.
//!
//! This is ADDITIVE: the shared-role binding (`catalog_clients.catalog_role_id`
//! is NOT NULL and legacy-mode catalogs enforce through it) is untouched.

use egret_perm::model::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm::store::create_grant_in_tx;
use sqlx::PgPool;
use uuid::Uuid;

/// Ensure `service_account_id` (a `users.id`) holds the per-engine
/// `{read,write,create,manage}` allow on the catalog-level securable of
/// `catalog_id`. Returns the grant id. Idempotent: an existing identical grant
/// for the same principal is returned and nothing is written.
pub async fn ensure_engine_catalog_grant(
    pool: &PgPool,
    catalog_id: Uuid,
    service_account_id: Uuid,
) -> Result<Uuid, EngineGrantError> {
    let mut tx = pool.begin().await?;
    let username =
        egret_auth::users::ensure_username_via(&mut tx, egret_core::UserId(service_account_id))
            .await
            .map_err(|e| EngineGrantError::Username(service_account_id, e.to_string()))?;
    // Serialise concurrent ensures of one (catalog, principal).
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("engine-catalog-grant:{catalog_id}:{username}"))
        .execute(&mut *tx)
        .await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT g.id FROM perm.grants g \
           JOIN perm.grant_principals p ON p.grant_id = g.id \
          WHERE g.catalog_id = $1 AND g.schema_name IS NULL AND g.table_name IS NULL \
            AND g.column_name IS NULL AND g.effect = 'allow'::perm.effect \
            AND g.origin = 'explicit' \
            AND g.actions @> ARRAY['read','write','create','manage']::text[] \
            AND p.kind = 'user'::perm.principal_kind AND p.ref = $2 \
            AND p.mode = 'include'::perm.principal_mode \
          LIMIT 1",
    )
    .bind(catalog_id)
    .bind(&username)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = existing {
        tx.commit().await?;
        return Ok(id);
    }
    let id = create_grant_in_tx(
        &mut tx,
        &Grant {
            id: Uuid::nil(),
            effect: Effect::Allow,
            actions: vec![Action::Read, Action::Write, Action::Create, Action::Manage],
            scope: Scope {
                catalog_id,
                schema: None,
                table: None,
                column: None,
            },
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::User,
                    r#ref: username,
                }],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

#[derive(Debug, thiserror::Error)]
pub enum EngineGrantError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("perm store: {0}")]
    Perm(#[from] egret_perm::PermError),
    #[error("no username for service account {0}: {1}")]
    Username(Uuid, String),
}

impl From<EngineGrantError> for crate::EngineHttpError {
    fn from(e: EngineGrantError) -> Self {
        tracing::error!(error = %e, "per-engine catalog grant could not be ensured");
        crate::EngineHttpError::Internal
    }
}
