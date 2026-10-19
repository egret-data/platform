//! The permission model's governance events (DG #3, spec §3.1–3.2): small,
//! stable summaries of grants and rules, and the one call that records an
//! event on the write's own transaction through
//! [`egret_audit::governance::record_change`].
use egret_audit::governance::{record_change, Change};
use egret_perm::{Grant, PrincipalSet, Rule, RuleKind, Scope, Selector};
use egret_policy::catalog_names_for_ids_via;
use serde_json::{json, Value};
use sqlx::PgConnection;

use crate::routes::PermHttpError;

/// Record one governance event on `conn` (the write's transaction). A failure
/// is the write's failure: an event that cannot be written takes its write
/// with it, so the feed never misses a committed change.
pub(crate) async fn record(
    conn: &mut PgConnection,
    action: &'static str,
    resource_type: &'static str,
    resource_id: String,
    summary: Value,
    principal: &egret_core::Principal,
) -> Result<(), PermHttpError> {
    record_change(
        conn,
        Change {
            action,
            resource_type,
            resource_id,
            summary,
            principal,
        },
    )
    .await
    .map_err(PermHttpError::db)
}

fn refs(set: &[egret_perm::Principal]) -> Vec<String> {
    set.iter()
        .map(|p| format!("{}:{}", p.kind.as_str(), p.r#ref))
        .collect()
}

/// The catalog's registry name, or its id when the row is gone.
async fn catalog_name(
    conn: &mut PgConnection,
    catalog_id: uuid::Uuid,
) -> Result<String, PermHttpError> {
    Ok(catalog_names_for_ids_via(conn, &[catalog_id])
        .await?
        .into_iter()
        .next()
        .map_or_else(|| catalog_id.to_string(), |c| c.name))
}

/// `{catalog, schema, table, column, effect, actions, include, exclude}`.
pub(crate) async fn grant_summary(
    conn: &mut PgConnection,
    g: &Grant,
) -> Result<Value, PermHttpError> {
    let PrincipalSet { include, exclude } = &g.principals;
    Ok(json!({
        "catalog": catalog_name(conn, g.scope.catalog_id).await?,
        "schema": g.scope.schema,
        "table": g.scope.table,
        "column": g.scope.column,
        "effect": g.effect.as_str(),
        "actions": g.actions.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
        "include": refs(include),
        "exclude": refs(exclude),
    }))
}

/// `catalog.schema.table.column`, as far as the scope names levels.
async fn path(conn: &mut PgConnection, s: &Scope) -> Result<String, PermHttpError> {
    let mut parts = vec![catalog_name(conn, s.catalog_id).await?];
    parts.extend(
        [&s.schema, &s.table, &s.column]
            .into_iter()
            .map_while(|l| l.clone()),
    );
    Ok(parts.join("."))
}

/// `{name, kind, selector, function, exempt, exempt_exclude}`. `selector` is
/// the path or `tag:<key>`; `function` the mask type or the filter expression.
pub(crate) async fn rule_summary(
    conn: &mut PgConnection,
    r: &Rule,
) -> Result<Value, PermHttpError> {
    let selector = match &r.selector {
        Selector::Tag { key } => format!("tag:{key}"),
        Selector::Path(scope) => path(conn, scope).await?,
    };
    let (kind, function, value_expr) = match &r.kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => ("mask", mask_type.clone(), value_expr.clone()),
        RuleKind::RowFilter { filter_expr } => ("row_filter", filter_expr.clone(), None),
    };
    let mut v = json!({
        "name": r.name,
        "kind": kind,
        "selector": selector,
        "function": function,
        "exempt": refs(&r.exempt.include),
        "exempt_exclude": refs(&r.exempt.exclude),
    });
    if let Some(expr) = value_expr {
        v["value_expr"] = json!(expr);
    }
    Ok(v)
}

/// `summary(before)` and `summary(after)` as `{before, after}`, `null` for an
/// absent side.
pub(crate) fn before_after(before: Option<Value>, after: Option<Value>) -> Value {
    json!({ "before": before, "after": after })
}
