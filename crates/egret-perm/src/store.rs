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

use sqlx::{PgPool, Postgres, Transaction};
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

/// Connection-generic core of [`PermStore::list_grants`] — SP4a Task 7 fix
/// round 1 (Critical/Important 2). `egret-perm-compile`'s
/// `recompile_service_in_tx` has to read the CURRENT grants from inside a
/// caller-owned, still-open transaction (`egret-bi::catalog_tags`'s tag
/// write): a `PermStore` method bound to its own pool would acquire a
/// SECOND connection, which — under Postgres' default READ COMMITTED
/// isolation — cannot see that transaction's own uncommitted writes, i.e.
/// the very tag assignment the recompile exists to react to. Taking `&mut
/// PgConnection` (not a generic `PgExecutor`) lets the same query logic run
/// on either a pool-acquired connection (`list_grants`) or a borrowed
/// transaction (`&mut *tx`, the same `&mut *conn`-reborrow idiom
/// `insert_principals`/`insert_exemptions` above already use for their own
/// sequential queries) without duplicating the row-parsing logic a second
/// time — this is a permission model; two copies of "what a grant means"
/// that can drift is exactly the kind of defect this crate exists to avoid.
pub async fn list_grants_via(conn: &mut sqlx::PgConnection) -> Result<Vec<Grant>, PermError> {
    let rows: Vec<GrantRow> = sqlx::query_as(
        "SELECT id, effect::text AS effect, actions, catalog_id, schema_name, \
                table_name, column_name, origin, origin_workspace_id \
         FROM perm.grants",
    )
    .fetch_all(&mut *conn)
    .await?;

    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let principal_rows: Vec<GrantPrincipalRow> = sqlx::query_as(
        "SELECT grant_id, kind::text AS kind, ref AS principal_ref, mode::text AS mode \
         FROM perm.grant_principals WHERE grant_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
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

/// Connection-generic core of [`PermStore::list_rules`] — same rationale as
/// [`list_grants_via`].
pub async fn list_rules_via(conn: &mut sqlx::PgConnection) -> Result<Vec<Rule>, PermError> {
    let rows: Vec<RuleRow> = sqlx::query_as(
        "SELECT id, name, kind::text AS kind, mask_type, value_expr, filter_expr, \
                selector_kind::text AS selector_kind, tag_key, catalog_id, \
                schema_name, table_name, column_name \
         FROM perm.rules",
    )
    .fetch_all(&mut *conn)
    .await?;

    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let exemption_rows: Vec<RuleExemptionRow> = sqlx::query_as(
        "SELECT rule_id, kind::text AS kind, ref AS principal_ref, mode::text AS mode \
         FROM perm.rule_exemptions WHERE rule_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
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

/// Insert `g` and its principal subtract on the CALLER's open transaction —
/// SP4a Task 9. The HTTP create path (`egret_perm_compile::routes`) has to
/// write the grant and RECOMPILE the services it affects inside ONE
/// transaction: a grant that commits but whose recompile then fails leaves
/// the engine enforcing the previous policy set while egret's UI shows the
/// grant as live, which is the exact defect Task 7 closed on the tag-write
/// path. [`PermStore::create_grant`] is this function plus its own
/// `begin`/`commit`, so there is one implementation of what "create a grant"
/// means, not two that can drift.
///
/// `g.id` is ignored — `perm.grants.id` is `DEFAULT gen_random_uuid()` and
/// the returned id is the one the database minted.
pub async fn create_grant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    g: &Grant,
) -> Result<Uuid, PermError> {
    let actions: Vec<&str> = g.actions.iter().map(|a| a.as_str()).collect();
    let (origin, origin_workspace_id): (&str, Option<Uuid>) = match &g.origin {
        GrantOrigin::Explicit => ("explicit", None),
        GrantOrigin::WorkspaceAssignment { workspace_id } => {
            ("workspace_assignment", Some(*workspace_id))
        }
    };
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
    .fetch_one(&mut **tx)
    .await?;
    insert_principals(tx, id, &g.principals).await?;
    Ok(id)
}

/// Delete one grant on the caller's open transaction — same rationale as
/// [`create_grant_in_tx`]: the delete and the recompile that reaps the
/// policies it produced must commit together, or the engine keeps enforcing
/// a grant egret no longer has.
pub async fn delete_grant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<bool, PermError> {
    let result = sqlx::query("DELETE FROM perm.grants WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Insert `r` and its exemption set on the caller's open transaction — same
/// rationale as [`create_grant_in_tx`]. `r.id` is ignored in favor of the id
/// `perm.rules.id DEFAULT gen_random_uuid()` mints.
pub async fn create_rule_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    r: &Rule,
) -> Result<Uuid, PermError> {
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
    .fetch_one(&mut **tx)
    .await?;
    insert_exemptions(tx, id, &r.exempt).await?;
    Ok(id)
}

/// Delete one rule on the caller's open transaction — same rationale as
/// [`delete_grant_in_tx`].
pub async fn delete_rule_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<bool, PermError> {
    let result = sqlx::query("DELETE FROM perm.rules WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// One grant by id, with its principal subtract — the read the HTTP delete
/// path needs BEFORE the row is gone, because the affected-service set is
/// computed from the grant's own `scope.catalog_id` (SP4a Task 9). `None`
/// when no such grant exists.
pub async fn get_grant_via(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<Option<Grant>, PermError> {
    let Some(row): Option<GrantRow> = sqlx::query_as(
        "SELECT id, effect::text AS effect, actions, catalog_id, schema_name, \
                table_name, column_name, origin, origin_workspace_id \
         FROM perm.grants WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let principal_rows: Vec<GrantPrincipalRow> = sqlx::query_as(
        "SELECT grant_id, kind::text AS kind, ref AS principal_ref, mode::text AS mode \
         FROM perm.grant_principals WHERE grant_id = $1",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let mut sets = group_principal_sets(
        principal_rows
            .into_iter()
            .map(|r| (r.grant_id, r.kind, r.principal_ref, r.mode)),
    )?;
    let principals = sets.remove(&id).unwrap_or_default();
    grant_from_row(row, principals).map(Some)
}

/// One rule by id, with its exemption set — same rationale as
/// [`get_grant_via`]: a rule's affected-service set depends on its SELECTOR
/// (a tag selector reaches every compiler-targeted profile; a path selector
/// only the profiles bound to its catalog), so the delete path has to read
/// the rule before deleting it.
pub async fn get_rule_via(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<Option<Rule>, PermError> {
    let Some(row): Option<RuleRow> = sqlx::query_as(
        "SELECT id, name, kind::text AS kind, mask_type, value_expr, filter_expr, \
                selector_kind::text AS selector_kind, tag_key, catalog_id, \
                schema_name, table_name, column_name \
         FROM perm.rules WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let exemption_rows: Vec<RuleExemptionRow> = sqlx::query_as(
        "SELECT rule_id, kind::text AS kind, ref AS principal_ref, mode::text AS mode \
         FROM perm.rule_exemptions WHERE rule_id = $1",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let mut sets = group_principal_sets(
        exemption_rows
            .into_iter()
            .map(|r| (r.rule_id, r.kind, r.principal_ref, r.mode)),
    )?;
    let exempt = sets.remove(&id).unwrap_or_default();
    rule_from_row(row, exempt).map(Some)
}

#[derive(Clone)]
pub struct PermStore {
    pool: PgPool,
}

impl PermStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Widened from `pub(crate)` to `pub` in SP4a Task 7: `assignment::sync_assignment_grants`
    /// was the first consumer (to open its own transaction); the second is
    /// `egret-perm-compile`'s `trigger::recompile_service`, which needs ONE
    /// transaction spanning `perm.compiled` (this crate's table) and
    /// `policy.authz_policies` / `policy.authz_services` (`egret-policy`'s
    /// tables, in the SAME physical Postgres database) so a diff's writes and
    /// its single `policy_version` bump commit together. `egret-perm-compile`
    /// cannot reach a `pub(crate)` item here, and there is no third schema
    /// this pool needs to stay hidden from — every other `PermStore` method
    /// stays self-contained regardless.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // grants ---------------------------------------------------------------

    /// Inserts `g` and its principal subtract in one transaction. `g.id` is
    /// ignored — `perm.grants.id` is `DEFAULT gen_random_uuid()`, and the id
    /// this returns is the one the database minted, not whatever the caller
    /// put in `g.id` (tests pass `Uuid::nil()` there for exactly this
    /// reason).
    pub async fn create_grant(&self, g: &Grant) -> Result<Uuid, PermError> {
        let mut tx = self.pool.begin().await?;
        let id = create_grant_in_tx(&mut tx, g).await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn list_grants(&self) -> Result<Vec<Grant>, PermError> {
        let mut conn = self.pool.acquire().await?;
        list_grants_via(&mut conn).await
    }

    pub async fn delete_grant(&self, id: Uuid) -> Result<bool, PermError> {
        let result = sqlx::query("DELETE FROM perm.grants WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // rules ------------------------------------------------------------------

    /// Same contract as `create_grant`: `r.id` is ignored in favor of the id
    /// `perm.rules.id DEFAULT gen_random_uuid()` mints.
    pub async fn create_rule(&self, r: &Rule) -> Result<Uuid, PermError> {
        let mut tx = self.pool.begin().await?;
        let id = create_rule_in_tx(&mut tx, r).await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn list_rules(&self) -> Result<Vec<Rule>, PermError> {
        let mut conn = self.pool.acquire().await?;
        list_rules_via(&mut conn).await
    }

    pub async fn delete_rule(&self, id: Uuid) -> Result<bool, PermError> {
        let result = sqlx::query("DELETE FROM perm.rules WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
