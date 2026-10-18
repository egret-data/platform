//! Engine-scoped grants (F11): the model and its store
//! (`migrations/0195_perm_engine_grants.sql`).
//!
//! An engine grant is an allow-only, include-only capability on ONE engine,
//! named by an id of the curated catalogue that lives in `egret-perm-compile`
//! (`engine_capabilities`), because this crate carries no `egret-policy`
//! dependency (see `lib.rs`). The store neither knows nor checks the
//! catalogue beyond what the table's `CHECK`s enforce; the route checks values
//! before writing and the compiler checks them again when it compiles.
//!
//! **The impersonation target is an id, not a name** (spec E21, amended
//! 2026-10-18). `trino.impersonate_user` and `starrocks.impersonate_user` store
//! their target in `target_user_id` and no `values`; the compiler renders the
//! user's CURRENT username. A target stored by name would be inherited by
//! whoever is next minted a freed username; an id cannot be, and the row goes
//! with the user (`ON DELETE CASCADE`).
//!
//! Same split as [`crate::store`]: connection-generic `*_via` reads, so the
//! compiler can read inside a caller's open transaction, and `*_in_tx` writes
//! on the caller's transaction, so a write and its recompile commit together.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::PermError;
use crate::model::{Principal, PrincipalKind, PrincipalSet};

/// One row of `perm.engine_grants` with its principals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineGrant {
    pub id: Uuid,
    pub engine_id: Uuid,
    /// A catalogue id, e.g. `trino.set_system_session_property`.
    pub capability: String,
    /// The capability's resource values in the catalogue's key order, WITHOUT
    /// an impersonation target (that is [`Self::target_user_id`]).
    pub values: Vec<String>,
    /// The impersonation target's `users.id`; `Some` exactly for the two
    /// `*.impersonate_user` capabilities (a table `CHECK`).
    pub target_user_id: Option<Uuid>,
    /// `exclude` is always empty (allow-only, include-only, spec E5).
    pub principals: PrincipalSet,
}

#[derive(sqlx::FromRow)]
struct EngineGrantRow {
    id: Uuid,
    engine_id: Uuid,
    capability: String,
    resource_values: Vec<String>,
    target_user_id: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
struct EngineGrantPrincipalRow {
    engine_grant_id: Uuid,
    kind: String,
    principal_ref: String,
}

async fn attach_principals(
    conn: &mut PgConnection,
    rows: Vec<EngineGrantRow>,
) -> Result<Vec<EngineGrant>, PermError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let principal_rows: Vec<EngineGrantPrincipalRow> = sqlx::query_as(
        "SELECT engine_grant_id, kind::text AS kind, ref AS principal_ref \
           FROM perm.engine_grant_principals \
          WHERE engine_grant_id = ANY($1) \
          ORDER BY engine_grant_id, kind::text, ref",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut sets: HashMap<Uuid, PrincipalSet> = HashMap::new();
    for r in principal_rows {
        let kind = PrincipalKind::parse(&r.kind)
            .ok_or_else(|| PermError::Invalid(format!("unknown principal_kind: {}", r.kind)))?;
        sets.entry(r.engine_grant_id)
            .or_default()
            .include
            .push(Principal {
                kind,
                r#ref: r.principal_ref,
            });
    }
    Ok(rows
        .into_iter()
        .map(|row| EngineGrant {
            principals: sets.remove(&row.id).unwrap_or_default(),
            id: row.id,
            engine_id: row.engine_id,
            capability: row.capability,
            values: row.resource_values,
            target_user_id: row.target_user_id,
        })
        .collect())
}

/// Every engine grant, ordered by `(engine_id, capability, resource_values,
/// target_user_id, id)`; principals by `(kind, ref)`.
pub async fn list_engine_grants_via(
    conn: &mut PgConnection,
) -> Result<Vec<EngineGrant>, PermError> {
    let rows: Vec<EngineGrantRow> = sqlx::query_as(
        "SELECT id, engine_id, capability, resource_values, target_user_id \
           FROM perm.engine_grants \
          ORDER BY engine_id, capability, resource_values, target_user_id, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    attach_principals(conn, rows).await
}

/// The engine grants that compile into profile `service_id`: those of every
/// engine whose `authz_service_id` is that profile (one engine since I2; more
/// on a legacy shared profile, spec C5).
pub async fn list_engine_grants_for_profile_via(
    conn: &mut PgConnection,
    service_id: i64,
) -> Result<Vec<EngineGrant>, PermError> {
    let rows: Vec<EngineGrantRow> = sqlx::query_as(
        "SELECT g.id, g.engine_id, g.capability, g.resource_values, g.target_user_id \
           FROM perm.engine_grants g \
           JOIN engine_mgmt.engines e ON e.id = g.engine_id \
          WHERE e.authz_service_id = $1 \
          ORDER BY g.engine_id, g.capability, g.resource_values, g.target_user_id, g.id",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?;
    attach_principals(conn, rows).await
}

/// The engine grants of one engine.
pub async fn list_engine_grants_for_engine_via(
    conn: &mut PgConnection,
    engine_id: Uuid,
) -> Result<Vec<EngineGrant>, PermError> {
    let rows: Vec<EngineGrantRow> = sqlx::query_as(
        "SELECT id, engine_id, capability, resource_values, target_user_id \
           FROM perm.engine_grants \
          WHERE engine_id = $1 \
          ORDER BY engine_id, capability, resource_values, target_user_id, id",
    )
    .bind(engine_id)
    .fetch_all(&mut *conn)
    .await?;
    attach_principals(conn, rows).await
}

/// One engine grant, or `None`.
pub async fn get_engine_grant_via(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<EngineGrant>, PermError> {
    let rows: Vec<EngineGrantRow> = sqlx::query_as(
        "SELECT id, engine_id, capability, resource_values, target_user_id \
           FROM perm.engine_grants WHERE id = $1",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(attach_principals(conn, rows).await?.into_iter().next())
}

async fn insert_principals(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    set: &PrincipalSet,
) -> Result<(), PermError> {
    if !set.exclude.is_empty() {
        return Err(PermError::Invalid(
            "an engine grant is include-only: its principal set has no exclude".into(),
        ));
    }
    for p in &set.include {
        sqlx::query(
            "INSERT INTO perm.engine_grant_principals (engine_grant_id, kind, ref) \
             VALUES ($1, $2::perm.principal_kind, $3)",
        )
        .bind(id)
        .bind(p.kind.as_str())
        .bind(&p.r#ref)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Insert `g` and its principals on the caller's transaction. `g.id` is
/// ignored; the returned id is the one the database minted.
pub async fn create_engine_grant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    g: &EngineGrant,
) -> Result<Uuid, PermError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.engine_grants (engine_id, capability, resource_values, target_user_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(g.engine_id)
    .bind(&g.capability)
    .bind(&g.values)
    .bind(g.target_user_id)
    .fetch_one(&mut **tx)
    .await?;
    insert_principals(tx, id, &g.principals).await?;
    Ok(id)
}

/// Replace grant `g.id`'s capability, values, target and principals in place
/// (the engine is fixed: a patch cannot move a grant between engines). `false`
/// when no such grant exists. Principals are REPLACED, not merged.
pub async fn update_engine_grant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    g: &EngineGrant,
) -> Result<bool, PermError> {
    let result = sqlx::query(
        "UPDATE perm.engine_grants \
            SET capability = $2, resource_values = $3, target_user_id = $4, updated_at = now() \
          WHERE id = $1",
    )
    .bind(g.id)
    .bind(&g.capability)
    .bind(&g.values)
    .bind(g.target_user_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query("DELETE FROM perm.engine_grant_principals WHERE engine_grant_id = $1")
        .bind(g.id)
        .execute(&mut **tx)
        .await?;
    insert_principals(tx, g.id, &g.principals).await?;
    Ok(true)
}

/// Delete one engine grant (its principals cascade). `false` when absent.
pub async fn delete_engine_grant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<bool, PermError> {
    let result = sqlx::query("DELETE FROM perm.engine_grants WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
