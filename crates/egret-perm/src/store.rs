//! `perm` schema store (SP4a). Runtime sqlx queries, schema-qualified. Every
//! query is a `&'static str` literal with bind parameters (sqlx 0.9 requires
//! it; `format!`-built SQL is forbidden by the programme's global
//! constraints) — a Postgres enum column is always read back cast to `::text`
//! and written with an explicit `::perm.<type>` cast on the bind parameter,
//! so no query text needs to vary with the value.
//!
//! Each `create_*` runs in one transaction: the row, then its principals —
//! see `grant_round_trips_with_its_principal_subtract` in `tests/store.rs`
//! for why the subtract surviving that round trip is the assertion that
//! matters.

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::PermError;
use crate::model::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind,
    Scope, Selector,
};

#[derive(sqlx::FromRow)]
struct GrantRow {
    id: Uuid,
    effect: String,
    actions: Vec<String>,
    catalog_id: Uuid,
    schema_name: Option<String>,
    table_name: Option<String>,
    column_name: Option<String>,
    origin: String,
    origin_workspace_id: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
struct GrantPrincipalRow {
    grant_id: Uuid,
    kind: String,
    principal_ref: String,
    mode: String,
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: Uuid,
    name: String,
    kind: String,
    mask_type: Option<String>,
    value_expr: Option<String>,
    filter_expr: Option<String>,
    selector_kind: String,
    tag_key: Option<String>,
    catalog_id: Option<Uuid>,
    schema_name: Option<String>,
    table_name: Option<String>,
    column_name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct RuleExemptionRow {
    rule_id: Uuid,
    kind: String,
    principal_ref: String,
    mode: String,
}

fn parse_principal_kind(s: &str) -> Result<PrincipalKind, PermError> {
    PrincipalKind::parse(s)
        .ok_or_else(|| PermError::Invalid(format!("unknown principal_kind: {s}")))
}

/// Fold a batch of `(owner_id, kind, ref, mode)` rows into one `PrincipalSet`
/// per owner (a grant's `grant_id`, or a rule's `rule_id`).
fn group_principal_sets<I>(rows: I) -> Result<HashMap<Uuid, PrincipalSet>, PermError>
where
    I: IntoIterator<Item = (Uuid, String, String, String)>,
{
    let mut sets: HashMap<Uuid, PrincipalSet> = HashMap::new();
    for (owner_id, kind, principal_ref, mode) in rows {
        let principal = Principal {
            kind: parse_principal_kind(&kind)?,
            r#ref: principal_ref,
        };
        let set = sets.entry(owner_id).or_default();
        match mode.as_str() {
            "include" => set.include.push(principal),
            "exclude" => set.exclude.push(principal),
            other => {
                return Err(PermError::Invalid(format!(
                    "unknown principal_mode: {other}"
                )))
            }
        }
    }
    Ok(sets)
}

fn grant_from_row(row: GrantRow, principals: PrincipalSet) -> Result<Grant, PermError> {
    let effect = Effect::parse(&row.effect)
        .ok_or_else(|| PermError::Invalid(format!("unknown effect: {}", row.effect)))?;
    let actions = row
        .actions
        .iter()
        .map(|a| Action::parse(a).ok_or_else(|| PermError::Invalid(format!("unknown action: {a}"))))
        .collect::<Result<Vec<_>, _>>()?;
    let origin = match row.origin.as_str() {
        "explicit" => GrantOrigin::Explicit,
        "workspace_assignment" => {
            let workspace_id = row.origin_workspace_id.ok_or_else(|| {
                PermError::Invalid("workspace_assignment grant with no origin_workspace_id".into())
            })?;
            GrantOrigin::WorkspaceAssignment { workspace_id }
        }
        other => return Err(PermError::Invalid(format!("unknown grant origin: {other}"))),
    };
    Ok(Grant {
        id: row.id,
        effect,
        actions,
        scope: Scope {
            catalog_id: row.catalog_id,
            schema: row.schema_name,
            table: row.table_name,
            column: row.column_name,
        },
        principals,
        origin,
    })
}

fn rule_from_row(row: RuleRow, exempt: PrincipalSet) -> Result<Rule, PermError> {
    let kind = match row.kind.as_str() {
        "mask" => RuleKind::Mask {
            mask_type: row
                .mask_type
                .ok_or_else(|| PermError::Invalid("mask rule with no mask_type".into()))?,
            value_expr: row.value_expr,
        },
        "row_filter" => RuleKind::RowFilter {
            filter_expr: row
                .filter_expr
                .ok_or_else(|| PermError::Invalid("row_filter rule with no filter_expr".into()))?,
        },
        other => return Err(PermError::Invalid(format!("unknown rule kind: {other}"))),
    };
    let selector = match row.selector_kind.as_str() {
        "tag" => Selector::Tag {
            key: row
                .tag_key
                .ok_or_else(|| PermError::Invalid("tag selector with no tag_key".into()))?,
        },
        "path" => Selector::Path(Scope {
            catalog_id: row
                .catalog_id
                .ok_or_else(|| PermError::Invalid("path selector with no catalog_id".into()))?,
            schema: row.schema_name,
            table: row.table_name,
            column: row.column_name,
        }),
        other => {
            return Err(PermError::Invalid(format!(
                "unknown selector kind: {other}"
            )))
        }
    };
    Ok(Rule {
        id: row.id,
        name: row.name,
        kind,
        selector,
        exempt,
    })
}

async fn insert_principals(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    grant_id: Uuid,
    set: &PrincipalSet,
) -> Result<(), sqlx::Error> {
    for p in &set.include {
        sqlx::query(
            "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
             VALUES ($1, $2::perm.principal_kind, $3, 'include'::perm.principal_mode)",
        )
        .bind(grant_id)
        .bind(p.kind.as_str())
        .bind(&p.r#ref)
        .execute(&mut **tx)
        .await?;
    }
    for p in &set.exclude {
        sqlx::query(
            "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
             VALUES ($1, $2::perm.principal_kind, $3, 'exclude'::perm.principal_mode)",
        )
        .bind(grant_id)
        .bind(p.kind.as_str())
        .bind(&p.r#ref)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn insert_exemptions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_id: Uuid,
    set: &PrincipalSet,
) -> Result<(), sqlx::Error> {
    for p in &set.include {
        sqlx::query(
            "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
             VALUES ($1, $2::perm.principal_kind, $3, 'include'::perm.principal_mode)",
        )
        .bind(rule_id)
        .bind(p.kind.as_str())
        .bind(&p.r#ref)
        .execute(&mut **tx)
        .await?;
    }
    for p in &set.exclude {
        sqlx::query(
            "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
             VALUES ($1, $2::perm.principal_kind, $3, 'exclude'::perm.principal_mode)",
        )
        .bind(rule_id)
        .bind(p.kind.as_str())
        .bind(&p.r#ref)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct PermStore {
    pool: PgPool,
}

impl PermStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // grants ---------------------------------------------------------------

    pub async fn create_grant(&self, g: &Grant) -> Result<Uuid, PermError> {
        let actions: Vec<&str> = g.actions.iter().map(|a| a.as_str()).collect();
        let (origin, origin_workspace_id): (&str, Option<Uuid>) = match &g.origin {
            GrantOrigin::Explicit => ("explicit", None),
            GrantOrigin::WorkspaceAssignment { workspace_id } => {
                ("workspace_assignment", Some(*workspace_id))
            }
        };

        let mut tx = self.pool.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO perm.grants \
               (effect, actions, catalog_id, schema_name, table_name, column_name, \
                origin, origin_workspace_id) \
             VALUES ($1::perm.effect, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING id",
        )
        .bind(g.effect.as_str())
        .bind(&actions)
        .bind(g.scope.catalog_id)
        .bind(&g.scope.schema)
        .bind(&g.scope.table)
        .bind(&g.scope.column)
        .bind(origin)
        .bind(origin_workspace_id)
        .fetch_one(&mut *tx)
        .await?;

        insert_principals(&mut tx, id, &g.principals).await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn list_grants(&self) -> Result<Vec<Grant>, PermError> {
        let rows: Vec<GrantRow> = sqlx::query_as(
            "SELECT id, effect::text AS effect, actions, catalog_id, schema_name, \
                    table_name, column_name, origin, origin_workspace_id \
             FROM perm.grants",
        )
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let principal_rows: Vec<GrantPrincipalRow> = sqlx::query_as(
            "SELECT grant_id, kind::text AS kind, ref AS principal_ref, mode::text AS mode \
             FROM perm.grant_principals WHERE grant_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        let mut sets = group_principal_sets(
            principal_rows
                .into_iter()
                .map(|r| (r.grant_id, r.kind, r.principal_ref, r.mode)),
        )?;

        rows.into_iter()
            .map(|row| {
                let principals = sets.remove(&row.id).unwrap_or_default();
                grant_from_row(row, principals)
            })
            .collect()
    }

    pub async fn delete_grant(&self, id: Uuid) -> Result<bool, PermError> {
        let result = sqlx::query("DELETE FROM perm.grants WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // rules ------------------------------------------------------------------

    pub async fn create_rule(&self, r: &Rule) -> Result<Uuid, PermError> {
        let (kind, mask_type, value_expr, filter_expr): (
            &str,
            Option<&str>,
            Option<&str>,
            Option<&str>,
        ) = match &r.kind {
            RuleKind::Mask {
                mask_type,
                value_expr,
            } => (
                "mask",
                Some(mask_type.as_str()),
                value_expr.as_deref(),
                None,
            ),
            RuleKind::RowFilter { filter_expr } => {
                ("row_filter", None, None, Some(filter_expr.as_str()))
            }
        };
        let (selector_kind, tag_key, catalog_id, schema_name, table_name, column_name): (
            &str,
            Option<&str>,
            Option<Uuid>,
            Option<&str>,
            Option<&str>,
            Option<&str>,
        ) = match &r.selector {
            Selector::Tag { key } => ("tag", Some(key.as_str()), None, None, None, None),
            Selector::Path(scope) => (
                "path",
                None,
                Some(scope.catalog_id),
                scope.schema.as_deref(),
                scope.table.as_deref(),
                scope.column.as_deref(),
            ),
        };

        let mut tx = self.pool.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO perm.rules \
               (name, kind, mask_type, value_expr, filter_expr, selector_kind, \
                tag_key, catalog_id, schema_name, table_name, column_name) \
             VALUES ($1, $2::perm.rule_kind, $3, $4, $5, $6::perm.selector_kind, \
                     $7, $8, $9, $10, $11) \
             RETURNING id",
        )
        .bind(&r.name)
        .bind(kind)
        .bind(mask_type)
        .bind(value_expr)
        .bind(filter_expr)
        .bind(selector_kind)
        .bind(tag_key)
        .bind(catalog_id)
        .bind(schema_name)
        .bind(table_name)
        .bind(column_name)
        .fetch_one(&mut *tx)
        .await?;

        insert_exemptions(&mut tx, id, &r.exempt).await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn list_rules(&self) -> Result<Vec<Rule>, PermError> {
        let rows: Vec<RuleRow> = sqlx::query_as(
            "SELECT id, name, kind::text AS kind, mask_type, value_expr, filter_expr, \
                    selector_kind::text AS selector_kind, tag_key, catalog_id, \
                    schema_name, table_name, column_name \
             FROM perm.rules",
        )
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let exemption_rows: Vec<RuleExemptionRow> = sqlx::query_as(
            "SELECT rule_id, kind::text AS kind, ref AS principal_ref, mode::text AS mode \
             FROM perm.rule_exemptions WHERE rule_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        let mut sets = group_principal_sets(
            exemption_rows
                .into_iter()
                .map(|r| (r.rule_id, r.kind, r.principal_ref, r.mode)),
        )?;

        rows.into_iter()
            .map(|row| {
                let exempt = sets.remove(&row.id).unwrap_or_default();
                rule_from_row(row, exempt)
            })
            .collect()
    }

    pub async fn delete_rule(&self, id: Uuid) -> Result<bool, PermError> {
        let result = sqlx::query("DELETE FROM perm.rules WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
