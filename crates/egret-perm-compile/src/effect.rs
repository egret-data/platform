//! What a rule will do, in effect terms (classification UX spec C6, C7).
//!
//! Computed from the MODEL — rules, classification assignments, catalogs and
//! engine attachments — never from compiled downloads, so it answers on a
//! catalog no engine serves, which is exactly where the compiler's overlap
//! check (`crate::overlap`) sees nothing. Read-only; `routes::preview` runs it
//! inside the preview's rolled-back transaction, before `write_rule`.
//!
//! **Bounded.** Reach is COUNTED in SQL (one aggregate per tag), never by
//! loading every assignment; the catalogs listed are the distinct registry
//! catalogs reached (one row per `registry.catalogs` row at most); the
//! overlap check examines at most [`COORD_SCAN_CAP`] columns (one query each)
//! and lists at most [`ALREADY_MASKED_CAP`] masks; `tags` is read only for the
//! keys stored mask rules select.

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::eval::{glob_match, resource_level_matches};
use egret_perm::model::{PrincipalKind, PrincipalSet, Rule, RuleKind, Selector};
use serde::Serialize;
use uuid::Uuid;

use crate::routes::{NewRule, PermHttpError};

/// At most this many overlapping masks are listed (spec §10).
pub const ALREADY_MASKED_CAP: usize = 20;

/// At most this many of a rule's columns are checked for other masks. The
/// reach COUNTS are not limited by it: they are SQL aggregates.
pub const COORD_SCAN_CAP: i64 = 50;

/// The rule's effect: reach, enforcement, exemptions and overlapping masks.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleEffect {
    /// `mask` or `row_filter`.
    pub kind: String,
    /// Every registry catalog the rule reaches, by name.
    pub catalogs: Vec<EffectCatalog>,
    /// Distinct tables reached; `None` when a path level is a pattern (or absent).
    pub table_count: Option<u32>,
    /// Masks only; `None` for a pattern, for "every column" (a path mask
    /// without a column, or a tag on a whole table) and for row filters.
    pub column_count: Option<u32>,
    /// Distinct engines, bound to a profile, attached to a reached catalog.
    pub enforced_by: u32,
    /// Distinct users in `exempt.include`, groups, roles and workspaces expanded.
    pub exempt_members: u32,
    /// `exempt.include` names the `public` group: nobody is masked.
    pub exempt_everyone: bool,
    /// Other mask rules that reach a column this rule reaches (at most 20).
    pub already_masked: Vec<ExistingMask>,
}

/// One catalog a rule reaches, and who enforces it there.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectCatalog {
    pub catalog_id: Uuid,
    pub catalog_name: String,
    /// Attached engines bound to a policy profile, by name.
    pub engines: Vec<EffectEngine>,
    /// `no_engine` (nothing attached) or `no_profile` (engines attached, none
    /// bound to a profile); absent when some engine enforces the rule.
    pub not_enforced: Option<String>,
}

/// An engine that enforces a rule on a catalog.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectEngine {
    pub id: Uuid,
    pub name: String,
    /// `trino` | `starrocks` | ….
    pub engine_type: String,
}

/// Another mask rule that already reaches a column this rule reaches.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExistingMask {
    pub rule_id: Uuid,
    pub rule_name: String,
    pub mask_type: String,
    /// `catalog.schema.table.column`.
    pub coordinate: String,
    pub exempt: PrincipalSet,
}

/// One literal column a rule reaches.
#[derive(Debug, Clone)]
struct Coord {
    catalog_id: Uuid,
    catalog_name: String,
    schema: String,
    table: String,
    column: String,
}

fn is_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

fn to_u32(n: i64) -> u32 {
    u32::try_from(n.max(0)).unwrap_or(u32::MAX)
}

/// The reach of a rule: catalogs `(id, name)`, the literal columns the
/// overlap check examines (≤ [`COORD_SCAN_CAP`]), and the two counts.
struct Reach {
    catalogs: Vec<(Uuid, String)>,
    coords: Vec<Coord>,
    table_count: Option<u32>,
    column_count: Option<u32>,
}

pub async fn rule_effect(
    conn: &mut sqlx::PgConnection,
    editing: Option<Uuid>,
    rule: &NewRule,
) -> Result<RuleEffect, PermHttpError> {
    let is_mask = matches!(rule.kind, RuleKind::Mask { .. });

    let reach = match &rule.selector {
        Selector::Tag { key } => tag_reach(conn, key, is_mask).await?,
        Selector::Path(scope) => {
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
                    .bind(scope.catalog_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(PermHttpError::db)?;
            let exact = |v: &Option<String>| v.as_deref().is_some_and(|s| !is_pattern(s));
            let table_exact = exact(&scope.schema) && exact(&scope.table);
            let column_exact = table_exact && exact(&scope.column);
            let coords = match (&name, column_exact) {
                (Some(catalog_name), true) => vec![Coord {
                    catalog_id: scope.catalog_id,
                    catalog_name: catalog_name.clone(),
                    schema: scope.schema.clone().unwrap_or_default(),
                    table: scope.table.clone().unwrap_or_default(),
                    column: scope.column.clone().unwrap_or_default(),
                }],
                _ => Vec::new(),
            };
            Reach {
                // An unknown catalog lists nothing; the write refuses it after.
                catalogs: name.map(|n| (scope.catalog_id, n)).into_iter().collect(),
                coords,
                table_count: table_exact.then_some(1),
                column_count: (is_mask && column_exact).then_some(1),
            }
        }
    };

    // Engines per catalog.
    let mut catalogs = Vec::with_capacity(reach.catalogs.len());
    let mut engine_ids = BTreeSet::new();
    for (catalog_id, catalog_name) in &reach.catalogs {
        let attached: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
            "SELECT e.id, e.name, e.type, e.authz_service_id IS NOT NULL \
               FROM registry.engine_catalog_attachments a \
               JOIN engine_mgmt.engines e ON e.id = a.engine_id \
              WHERE a.catalog_id = $1 ORDER BY e.name",
        )
        .bind(catalog_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(PermHttpError::db)?;
        let engines: Vec<EffectEngine> = attached
            .iter()
            .filter(|(_, _, _, bound)| *bound)
            .map(|(id, name, ty, _)| EffectEngine {
                id: *id,
                name: name.clone(),
                engine_type: ty.clone(),
            })
            .collect();
        engine_ids.extend(engines.iter().map(|e| e.id));
        let not_enforced = if attached.is_empty() {
            Some("no_engine".to_string())
        } else if engines.is_empty() {
            Some("no_profile".to_string())
        } else {
            None
        };
        catalogs.push(EffectCatalog {
            catalog_id: *catalog_id,
            catalog_name: catalog_name.clone(),
            engines,
            not_enforced,
        });
    }

    let (exempt_members, exempt_everyone) = count_exempt(conn, &rule.exempt).await?;

    let already_masked = if is_mask {
        already_masked(conn, editing, &reach.coords).await?
    } else {
        Vec::new()
    };

    Ok(RuleEffect {
        kind: if is_mask { "mask" } else { "row_filter" }.into(),
        catalogs,
        table_count: reach.table_count,
        column_count: reach.column_count,
        enforced_by: u32::try_from(engine_ids.len()).unwrap_or(u32::MAX),
        exempt_members,
        exempt_everyone,
        already_masked,
    })
}

/// A tag selector's reach: every assignment of the tag in a warehouse a
/// registry catalog fronts. Counted in SQL; only the first
/// [`COORD_SCAN_CAP`] columns are loaded.
async fn tag_reach(
    conn: &mut sqlx::PgConnection,
    key: &str,
    is_mask: bool,
) -> Result<Reach, PermHttpError> {
    let (tables, columns, any_table_level): (i64, i64, Option<bool>) = sqlx::query_as(
        "SELECT count(DISTINCT (c.id, a.schema_name, a.table_name)), \
                count(*) FILTER (WHERE a.column_name <> ''), \
                bool_or(a.column_name = '') \
           FROM catalog_tag_assignments a \
           JOIN tags t ON t.id = a.tag_id \
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
          WHERE t.key = $1",
    )
    .bind(key)
    .fetch_one(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;

    let catalogs: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT c.id, c.name \
           FROM catalog_tag_assignments a \
           JOIN tags t ON t.id = a.tag_id \
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
          WHERE t.key = $1 \
          ORDER BY c.name",
    )
    .bind(key)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;

    let coords = if is_mask {
        let rows: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(
            "SELECT c.id, c.name, a.schema_name, a.table_name, a.column_name \
               FROM catalog_tag_assignments a \
               JOIN tags t ON t.id = a.tag_id \
               JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
              WHERE t.key = $1 AND a.column_name <> '' \
              ORDER BY c.name, a.schema_name, a.table_name, a.column_name \
              LIMIT $2",
        )
        .bind(key)
        .bind(COORD_SCAN_CAP)
        .fetch_all(&mut *conn)
        .await
        .map_err(PermHttpError::db)?;
        rows.into_iter()
            .map(|(catalog_id, catalog_name, schema, table, column)| Coord {
                catalog_id,
                catalog_name,
                schema,
                table,
                column,
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Reach {
        catalogs,
        coords,
        table_count: Some(to_u32(tables)),
        column_count: (is_mask && !any_table_level.unwrap_or(false)).then_some(to_u32(columns)),
    })
}

/// Distinct people `exempt.include` names, with groups, roles and workspaces
/// expanded the way the compiler expands them (`crate::members`). `exclude`
/// is not subtracted: a non-empty `exclude` cannot compile on a rule (see
/// `egret_perm::model::Rule`). A `public` group exempts everyone and adds
/// nobody to the count.
async fn count_exempt(
    conn: &mut sqlx::PgConnection,
    exempt: &PrincipalSet,
) -> Result<(u32, bool), PermHttpError> {
    let mut everyone = false;
    let mut groups = Vec::new();
    let mut roles = Vec::new();
    let mut workspaces = Vec::new();
    let mut users = Vec::new();
    for p in &exempt.include {
        match p.kind {
            PrincipalKind::Group if p.r#ref == "public" => everyone = true,
            PrincipalKind::Group => groups.push(p.r#ref.clone()),
            PrincipalKind::Role => roles.push(p.r#ref.clone()),
            PrincipalKind::WorkspaceMembers => {
                if let Ok(id) = p.r#ref.parse::<Uuid>() {
                    workspaces.push(id);
                }
            }
            PrincipalKind::User => users.push(p.r#ref.clone()),
        }
    }

    let mut people: BTreeSet<Uuid> = BTreeSet::new();
    people.extend(
        crate::members::group_members_via(conn, &groups)
            .await?
            .into_iter()
            .map(|r| r.user_id),
    );
    people.extend(
        crate::members::role_holders_via(conn, &roles)
            .await?
            .into_iter()
            .map(|r| r.user_id),
    );
    people.extend(
        crate::members::workspace_members_via(conn, &workspaces)
            .await?
            .into_iter()
            .map(|r| r.user_id),
    );
    if !users.is_empty() {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE username = ANY($1::citext[])")
                .bind(&users)
                .fetch_all(&mut *conn)
                .await
                .map_err(PermHttpError::db)?;
        people.extend(ids);
    }
    Ok((u32::try_from(people.len()).unwrap_or(u32::MAX), everyone))
}

/// Other stored mask rules (not `editing`) that reach one of `coords`, at
/// most [`ALREADY_MASKED_CAP`], deduplicated by `(rule, coordinate)`.
async fn already_masked(
    conn: &mut sqlx::PgConnection,
    editing: Option<Uuid>,
    coords: &[Coord],
) -> Result<Vec<ExistingMask>, PermHttpError> {
    if coords.is_empty() {
        return Ok(Vec::new());
    }
    let masks: Vec<Rule> = egret_perm::list_rules_via(&mut *conn)
        .await
        .map_err(PermHttpError::db)?
        .into_iter()
        .filter(|r| matches!(r.kind, RuleKind::Mask { .. }) && Some(r.id) != editing)
        .collect();
    if masks.is_empty() {
        return Ok(Vec::new());
    }

    // `tags.name` for each key a stored tag mask selects — the string engines
    // match tag policies on.
    let keys: Vec<String> = masks
        .iter()
        .filter_map(|r| match &r.selector {
            Selector::Tag { key } => Some(key.clone()),
            Selector::Path(_) => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let tag_names: BTreeMap<String, String> = if keys.is_empty() {
        BTreeMap::new()
    } else {
        sqlx::query_as::<_, (String, String)>("SELECT key, name FROM tags WHERE key = ANY($1)")
            .bind(&keys)
            .fetch_all(&mut *conn)
            .await
            .map_err(PermHttpError::db)?
            .into_iter()
            .collect()
    };

    let mut out = Vec::new();
    let mut seen: BTreeSet<(Uuid, String)> = BTreeSet::new();
    for coord in coords {
        if out.len() >= ALREADY_MASKED_CAP {
            break;
        }
        // The classifications on this column, and on its whole table.
        let classifications: Vec<String> = if tag_names.is_empty() {
            Vec::new()
        } else {
            sqlx::query_scalar(
                "SELECT t.name FROM catalog_tag_assignments a JOIN tags t ON t.id = a.tag_id \
                   JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
                  WHERE c.id = $1 AND lower(a.schema_name) = lower($2) \
                    AND lower(a.table_name) = lower($3) \
                    AND (a.column_name = '' OR lower(a.column_name) = lower($4))",
            )
            .bind(coord.catalog_id)
            .bind(&coord.schema)
            .bind(&coord.table)
            .bind(&coord.column)
            .fetch_all(&mut *conn)
            .await
            .map_err(PermHttpError::db)?
        };
        let coordinate = format!(
            "{}.{}.{}.{}",
            coord.catalog_name, coord.schema, coord.table, coord.column
        );
        for rule in &masks {
            if out.len() >= ALREADY_MASKED_CAP {
                break;
            }
            let reaches = match &rule.selector {
                Selector::Path(s) => {
                    let level = |pattern: &Option<String>, value: &str| {
                        pattern
                            .as_deref()
                            .is_none_or(|p| resource_level_matches(p, value))
                    };
                    s.catalog_id == coord.catalog_id
                        && level(&s.schema, &coord.schema)
                        && level(&s.table, &coord.table)
                        && level(&s.column, &coord.column)
                }
                // Case-sensitive, as the engines match tag names.
                Selector::Tag { key } => tag_names
                    .get(key)
                    .is_some_and(|name| classifications.iter().any(|c| glob_match(name, c))),
            };
            if !reaches || !seen.insert((rule.id, coordinate.clone())) {
                continue;
            }
            let RuleKind::Mask { mask_type, .. } = &rule.kind else {
                continue;
            };
            out.push(ExistingMask {
                rule_id: rule.id,
                rule_name: rule.name.clone(),
                mask_type: mask_type.clone(),
                coordinate: coordinate.clone(),
                exempt: rule.exempt.clone(),
            });
        }
    }
    Ok(out)
}
