//! One `OWNER` per securable (concepts spec D4): "Every securable has exactly
//! one `OWNER`, by default its creator; the owner holds every privilege on it
//! and below it."
//!
//! The owner is modelled as an ordinary ALLOW [`Grant`] of every privilege on
//! the securable (grants inherit down the tree), synthesised at IR-load time
//! by [`owner_grant`]. Because it is a plain allow, D6 ("a deny anywhere on the
//! path beats any allow") applies to it unchanged: the owner is NOT exempt from
//! a deny.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::PermError;
use crate::model::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
};

/// One row of `perm.securable_owners`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub id: Uuid,
    pub scope: Scope,
    pub principal: Principal,
}

#[derive(sqlx::FromRow)]
struct OwnerRow {
    id: Uuid,
    catalog_id: Uuid,
    schema_name: Option<String>,
    table_name: Option<String>,
    column_name: Option<String>,
    principal_kind: String,
    principal_ref: String,
}

fn owner_from_row(r: OwnerRow) -> Result<Owner, PermError> {
    let kind = match r.principal_kind.as_str() {
        "user" => PrincipalKind::User,
        "group" => PrincipalKind::Group,
        "role" => PrincipalKind::Role,
        "workspace_members" => PrincipalKind::WorkspaceMembers,
        other => {
            return Err(PermError::Invalid(format!(
                "unknown principal kind: {other}"
            )))
        }
    };
    Ok(Owner {
        id: r.id,
        scope: Scope {
            catalog_id: r.catalog_id,
            schema: r.schema_name,
            table: r.table_name,
            column: r.column_name,
        },
        principal: Principal {
            kind,
            r#ref: r.principal_ref,
        },
    })
}

macro_rules! cols {
    () => {
        "id, catalog_id, schema_name, table_name, column_name, \
         principal_kind::text AS principal_kind, principal_ref"
    };
}

/// The owners of securables on one of `catalogs`.
pub async fn list_owners_for_catalogs_via(
    conn: &mut PgConnection,
    catalogs: &[Uuid],
) -> Result<Vec<Owner>, PermError> {
    let rows: Vec<OwnerRow> = sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM perm.securable_owners WHERE catalog_id = ANY($1) ORDER BY created_at, id"
    ))
    .bind(catalogs)
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter().map(owner_from_row).collect()
}

/// The owner of exactly `scope`, if one is recorded (no inheritance).
pub async fn get_owner_via(
    conn: &mut PgConnection,
    scope: &Scope,
) -> Result<Option<Owner>, PermError> {
    let row: Option<OwnerRow> = sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM perm.securable_owners \
         WHERE catalog_id = $1 \
           AND schema_name IS NOT DISTINCT FROM $2 \
           AND table_name IS NOT DISTINCT FROM $3 \
           AND column_name IS NOT DISTINCT FROM $4"
    ))
    .bind(scope.catalog_id)
    .bind(&scope.schema)
    .bind(&scope.table)
    .bind(&scope.column)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(owner_from_row).transpose()
}

/// Record `owner` as the owner of `scope`, or TRANSFER the securable to them if
/// it already has one: the securable is the key, so there is only ever one
/// holder. Returns the stored owner.
pub async fn set_owner_via(
    conn: &mut PgConnection,
    scope: &Scope,
    owner: &Principal,
) -> Result<Owner, PermError> {
    let row: OwnerRow = sqlx::query_as(concat!(
        "INSERT INTO perm.securable_owners \
             (catalog_id, schema_name, table_name, column_name, principal_kind, principal_ref) \
         VALUES ($1, $2, $3, $4, $5::perm.principal_kind, $6) \
         ON CONFLICT ON CONSTRAINT securable_owners_one_per_securable \
         DO UPDATE SET principal_kind = EXCLUDED.principal_kind, \
                       principal_ref = EXCLUDED.principal_ref, updated_at = now() \
         RETURNING ",
        cols!()
    ))
    .bind(scope.catalog_id)
    .bind(&scope.schema)
    .bind(&scope.table)
    .bind(&scope.column)
    .bind(owner.kind.as_str())
    .bind(&owner.r#ref)
    .fetch_one(&mut *conn)
    .await?;
    owner_from_row(row)
}

/// Every privilege an owner holds (`OWNER` = all of the above, D4).
pub const OWNER_ACTIONS: [Action; 5] = [
    Action::Browse,
    Action::Read,
    Action::Write,
    Action::Create,
    Action::Manage,
];

/// The owner as the allow grant the IR lowers: every privilege on the
/// securable, inherited down its subtree, to the single holder. Keyed by the
/// owner row's id so it is stable across recompiles.
pub fn owner_grant(o: &Owner) -> Grant {
    Grant {
        id: o.id,
        effect: Effect::Allow,
        actions: OWNER_ACTIONS.to_vec(),
        scope: o.scope.clone(),
        principals: PrincipalSet {
            include: vec![o.principal.clone()],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}
