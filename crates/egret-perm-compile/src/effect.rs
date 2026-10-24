//! What a rule will do, in effect terms (classification UX spec C6, C7).
//!
//! Computed from the MODEL — rules, classification assignments, catalogs and
//! engine attachments — never from compiled downloads, so it answers on a
//! catalog no engine serves, which is exactly where the compiler's overlap
//! check (`crate::overlap`) sees nothing. Read-only; `routes::preview` runs it
//! inside the preview's rolled-back transaction, before `write_rule`, and
//! `GET /perm/rules/{id}/effect` runs it on a STORED rule. The enforcement
//! half alone ([`rules_enforcement`], `GET /perm/rules/enforcement`) answers a
//! list of stored rules through the same functions.
//!
//! **Bounded.** Reach is COUNTED in SQL (one aggregate per tag), never by
//! loading every assignment. The catalogs listed are the distinct registry
//! catalogs reached, and their engines come from ONE `catalog_id = ANY($1)`
//! query. The overlap check examines at most [`COORD_SCAN_CAP`] of a tag
//! rule's columns, reads the classifications on all of them in ONE query of
//! at most [`CLASSIFICATION_ROW_CAP`] rows (only classifications some stored
//! mask selects), and lists at most [`ALREADY_MASKED_CAP`] masks. Any of
//! those caps cutting a result sets [`RuleEffect::truncated`].

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::eval::{glob_match, resource_level_matches};
use egret_perm::model::{PrincipalKind, PrincipalSet, Rule, RuleKind, Selector};
use serde::Serialize;
use uuid::Uuid;

use crate::routes::{NewRule, PermHttpError};

/// At most this many overlapping masks are listed (spec §10).
pub const ALREADY_MASKED_CAP: usize = 20;

/// At most this many of a tag rule's columns are checked for other masks. The
/// reach COUNTS are not limited by it: they are SQL aggregates.
pub const COORD_SCAN_CAP: i64 = 50;

/// At most this many classification assignments are read for the overlap
/// check, across every target table, in one query.
pub const CLASSIFICATION_ROW_CAP: i64 = 1000;

/// At most this many distinct rule ids are answered by one
/// `GET /perm/rules/enforcement` request.
pub const RULE_ENFORCEMENT_CAP: usize = 100;

/// `mask` or `row_filter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Mask,
    RowFilter,
}

/// Why no query is affected by the rule yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotEnforced {
    /// The rule reaches no catalog: a classification assigned nowhere a
    /// registry catalog fronts (only on the rule as a whole, never per catalog).
    NoAssignment,
    /// No engine is attached to the catalog(s).
    NoEngine,
    /// Engines are attached, but none is bound to a policy profile.
    NoProfile,
    /// A classification (tag) rule whose catalog's engines are bound only to
    /// Trino profiles with no usable `tagService` (unset, or naming no
    /// existing profile): the compiler skips the rule there
    /// (`CompileError::MissingTagService`), so those engines do not apply it.
    NoTagService,
}

/// The rule's effect: reach, enforcement, exemptions and overlapping masks.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleEffect {
    pub kind: EffectKind,
    /// Every registry catalog the rule reaches, by name.
    pub catalogs: Vec<EffectCatalog>,
    /// Distinct tables reached; `null` when a path level is a pattern (or absent).
    pub table_count: Option<u32>,
    /// Masks only; `null` for a pattern, for "every column" (a path mask
    /// without a column, or a tag on a whole table) and for row filters.
    pub column_count: Option<u32>,
    /// Distinct engines, bound to a profile, attached to a reached catalog.
    pub enforced_by: u32,
    /// `null` when some engine enforces the rule (`enforcedBy > 0`); otherwise
    /// why not: `no_assignment` (nothing reached), `no_tag_service` (a
    /// classification rule, and some reached catalog's bound engines are all
    /// on Trino profiles with no usable tag service), `no_profile` (some
    /// reached catalog has engines, none bound) or `no_engine`.
    pub not_enforced: Option<NotEnforced>,
    /// Distinct users in `exempt.include`, groups, roles and workspaces expanded.
    pub exempt_members: u32,
    /// `exempt.include` names the `public` group: nobody is masked.
    pub exempt_everyone: bool,
    /// Other mask rules that reach a column this rule reaches (at most 20).
    pub already_masked: Vec<ExistingMask>,
    /// `alreadyMasked` may be incomplete: a cap cut the columns examined
    /// (50), the classifications read (1000) or the entries listed (20).
    pub truncated: bool,
    /// A classification ROW FILTER only: the tables it filters nothing on in
    /// Trino ([`TrinoColumnOnly`]); `null` for a mask, a path rule, or when
    /// there is none.
    pub trino_column_only: Option<TrinoColumnOnly>,
}

/// Where a classification row filter filters nothing on Trino (spec §5.8, D3,
/// E14): tables where the classification is on at least one column and not on
/// the table, in catalogs a Trino engine enforces the rule on. A Trino tag row
/// filter fires only for a table-level attachment; StarRocks filters these.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrinoColumnOnly {
    /// The catalogs those tables are in, by name, sorted.
    pub catalogs: Vec<String>,
    /// Distinct tables.
    pub tables: u32,
}

/// One catalog a rule reaches, and who enforces it there.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectCatalog {
    pub catalog_id: Uuid,
    pub catalog_name: String,
    /// Attached engines that enforce the rule here, by name: bound to a policy
    /// profile, and for a classification rule on a Trino profile, one whose
    /// `tagService` names an existing profile.
    pub engines: Vec<EffectEngine>,
    /// `null` when some engine enforces the rule here; otherwise `no_engine`
    /// (nothing attached), `no_profile` (engines attached, none bound) or
    /// `no_tag_service` (bound, but a classification rule cannot compile for
    /// any of their profiles: Trino without a usable tag service).
    pub not_enforced: Option<NotEnforced>,
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
    /// `catalog.schema.table.column`; the column is the pattern, or `*`, when
    /// the overlap is on a pattern or a whole table.
    pub coordinate: String,
    pub exempt: PrincipalSet,
}

/// Which columns of a target table a rule reaches.
#[derive(Debug, Clone)]
enum ColumnSel {
    Exact(String),
    Pattern(String),
    /// Every column (a path mask without a column).
    Any,
}

/// One literal table a mask reaches, and which of its columns.
#[derive(Debug, Clone)]
struct Target {
    catalog_id: Uuid,
    catalog_name: String,
    schema: String,
    table: String,
    column: ColumnSel,
}

impl Target {
    /// Does this target's column selection reach a stored path level?
    /// `None` (every column) reaches everything; two patterns are taken to
    /// intersect when either matches the other — an approximation that can
    /// miss two patterns that only overlap in the middle (`e*` vs `*l`).
    fn column_meets(&self, stored: Option<&str>) -> bool {
        match (&self.column, stored) {
            (_, None) | (ColumnSel::Any, _) => true,
            (ColumnSel::Exact(c), Some(s)) => resource_level_matches(s, c),
            (ColumnSel::Pattern(p), Some(s)) => {
                resource_level_matches(s, p) || resource_level_matches(p, s)
            }
        }
    }

    /// Does this target reach an assigned column (`""` = the whole table)?
    fn reaches_assigned(&self, assigned: &str) -> bool {
        assigned.is_empty()
            || match &self.column {
                ColumnSel::Any => true,
                ColumnSel::Exact(c) => c.eq_ignore_ascii_case(assigned),
                ColumnSel::Pattern(p) => resource_level_matches(p, assigned),
            }
    }

    fn coordinate(&self, column: &str) -> String {
        format!(
            "{}.{}.{}.{}",
            self.catalog_name, self.schema, self.table, column
        )
    }

    /// The column to name for an overlap with `stored` (a stored path level,
    /// or an assigned column; `None`/`""` = every column).
    fn overlap_column(&self, stored: Option<&str>) -> String {
        match (&self.column, stored) {
            (ColumnSel::Exact(c), _) => c.clone(),
            (_, Some(s)) if !s.is_empty() && !is_pattern(s) => s.to_string(),
            (ColumnSel::Pattern(p), _) => p.clone(),
            (ColumnSel::Any, Some(s)) if !s.is_empty() => s.to_string(),
            (ColumnSel::Any, _) => "*".to_string(),
        }
    }
}

fn is_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

fn to_u32(n: i64) -> u32 {
    u32::try_from(n.max(0)).unwrap_or(u32::MAX)
}

/// The reach of a rule beyond its catalogs: the tables the overlap check
/// examines, and the two counts.
struct Reach {
    targets: Vec<Target>,
    /// The scan cap cut `targets`.
    targets_truncated: bool,
    table_count: Option<u32>,
    column_count: Option<u32>,
}

pub async fn rule_effect(
    conn: &mut sqlx::PgConnection,
    editing: Option<Uuid>,
    rule: &NewRule,
) -> Result<RuleEffect, PermHttpError> {
    let is_mask = matches!(rule.kind, RuleKind::Mask { .. });
    let reached = reached_catalogs(conn, &rule.selector).await?;

    let reach = match &rule.selector {
        Selector::Tag { key } => tag_reach(conn, key, is_mask).await?,
        Selector::Path(scope) => {
            // `reached` is this path's catalog, or nothing for an unknown id.
            let name = reached.first().map(|(_, n)| n.clone());
            let literal = |v: &Option<String>| v.as_deref().is_some_and(|s| !is_pattern(s));
            let table_exact = literal(&scope.schema) && literal(&scope.table);
            let column_exact = table_exact && literal(&scope.column);
            let column = match scope.column.as_deref() {
                None => ColumnSel::Any,
                Some(c) if is_pattern(c) => ColumnSel::Pattern(c.to_string()),
                Some(c) => ColumnSel::Exact(c.to_string()),
            };
            let targets = match (name, table_exact && is_mask) {
                (Some(catalog_name), true) => vec![Target {
                    catalog_id: scope.catalog_id,
                    catalog_name,
                    schema: scope.schema.clone().unwrap_or_default(),
                    table: scope.table.clone().unwrap_or_default(),
                    column,
                }],
                _ => Vec::new(),
            };
            Reach {
                targets,
                targets_truncated: false,
                table_count: table_exact.then_some(1),
                column_count: (is_mask && column_exact).then_some(1),
            }
        }
    };

    let ids: Vec<Uuid> = reached.iter().map(|(id, _)| *id).collect();
    let attached = attached_engines(conn, &ids).await?;
    let tag_rule = matches!(rule.selector, Selector::Tag { .. });
    let (catalogs, enforced_by, not_enforced) = enforcement(&reached, &attached, tag_rule);

    let (exempt_members, exempt_everyone) = count_exempt(conn, &rule.exempt).await?;

    let (already_masked, scan_truncated) = if is_mask {
        already_masked(conn, editing, &reach.targets).await?
    } else {
        (Vec::new(), false)
    };

    let trino_column_only = match &rule.selector {
        Selector::Tag { key } if !is_mask => trino_column_only(conn, key, &catalogs).await?,
        _ => None,
    };

    Ok(RuleEffect {
        kind: if is_mask {
            EffectKind::Mask
        } else {
            EffectKind::RowFilter
        },
        catalogs,
        table_count: reach.table_count,
        column_count: reach.column_count,
        enforced_by,
        not_enforced,
        exempt_members,
        exempt_everyone,
        already_masked,
        truncated: is_mask && (reach.targets_truncated || scan_truncated),
        trino_column_only,
    })
}

/// [`TrinoColumnOnly`] for a classification row filter keyed `key`, over the
/// reached `catalogs` whose enforcing engines include a Trino one. Levels
/// compare case-folded, as the tag resolution does.
async fn trino_column_only(
    conn: &mut sqlx::PgConnection,
    key: &str,
    catalogs: &[EffectCatalog],
) -> Result<Option<TrinoColumnOnly>, PermHttpError> {
    let trino: Vec<Uuid> = catalogs
        .iter()
        .filter(|c| c.engines.iter().any(|e| e.engine_type == "trino"))
        .map(|c| c.catalog_id)
        .collect();
    if trino.is_empty() {
        return Ok(None);
    }
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT c.name, lower(a.schema_name), lower(a.table_name) \
           FROM catalog_tag_assignments a \
           JOIN tags t ON t.id = a.tag_id \
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
          WHERE t.key = $1 AND c.id = ANY($2) AND a.column_name <> '' \
            AND NOT EXISTS (SELECT 1 FROM catalog_tag_assignments b \
                             WHERE b.tag_id = a.tag_id AND b.warehouse_id = a.warehouse_id \
                               AND lower(b.schema_name) = lower(a.schema_name) \
                               AND lower(b.table_name) = lower(a.table_name) \
                               AND b.column_name = '')",
    )
    .bind(key)
    .bind(&trino)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let names: BTreeSet<String> = rows.iter().map(|(name, _, _)| name.clone()).collect();
    Ok(Some(TrinoColumnOnly {
        catalogs: names.into_iter().collect(),
        tables: u32::try_from(rows.len()).unwrap_or(u32::MAX),
    }))
}

/// Whether a STORED rule is enforced: the `enforcedBy`/`notEnforced`/`catalogs`
/// half of [`RuleEffect`], for lists of rules (`GET /perm/rules/enforcement`).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleEnforcement {
    pub rule_id: Uuid,
    /// Distinct engines, bound to a profile, attached to a catalog the rule
    /// reaches (as `enforcedBy` in the rule's effect).
    pub enforced_by: u32,
    /// `null` when some engine enforces the rule; otherwise why not, as
    /// `notEnforced` in the rule's effect.
    pub not_enforced: Option<NotEnforced>,
    /// Each catalog the rule reaches and who enforces it THERE, as `catalogs`
    /// in the rule's effect: a page about one catalog reads its entry, since
    /// a rule enforced in one catalog may be enforced in no other.
    pub catalogs: Vec<EffectCatalog>,
}

/// [`RuleEnforcement`] for each of `rules`, in their order, computed by the
/// same functions [`rule_effect`] uses ([`reached_catalogs`],
/// [`attached_engines`], [`enforcement`]), so the two cannot disagree.
/// One catalog query per rule and ONE engine query for all of them; the
/// caller bounds `rules` by [`RULE_ENFORCEMENT_CAP`].
pub async fn rules_enforcement(
    conn: &mut sqlx::PgConnection,
    rules: &[Rule],
) -> Result<Vec<RuleEnforcement>, PermHttpError> {
    let mut reached = Vec::with_capacity(rules.len());
    for rule in rules {
        reached.push(reached_catalogs(conn, &rule.selector).await?);
    }
    let ids: Vec<Uuid> = reached
        .iter()
        .flatten()
        .map(|(id, _)| *id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let attached = attached_engines(conn, &ids).await?;
    Ok(rules
        .iter()
        .zip(&reached)
        .map(|(rule, reached)| {
            let tag_rule = matches!(rule.selector, Selector::Tag { .. });
            let (catalogs, enforced_by, not_enforced) = enforcement(reached, &attached, tag_rule);
            RuleEnforcement {
                rule_id: rule.id,
                enforced_by,
                not_enforced,
                catalogs,
            }
        })
        .collect())
}

/// Every registry catalog a selector reaches, `(id, name)`: a path's catalog
/// (nothing for an unknown id; the write refuses it after), or each catalog
/// fronting a warehouse the tag is assigned in, by name.
async fn reached_catalogs(
    conn: &mut sqlx::PgConnection,
    selector: &Selector,
) -> Result<Vec<(Uuid, String)>, PermHttpError> {
    match selector {
        Selector::Path(scope) => {
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
                    .bind(scope.catalog_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(PermHttpError::db)?;
            Ok(name.map(|n| (scope.catalog_id, n)).into_iter().collect())
        }
        Selector::Tag { key } => sqlx::query_as(
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
        .map_err(PermHttpError::db),
    }
}

/// An engine attached to a catalog: `(catalog id, engine id, engine name,
/// engine type, bound to a policy profile, a classification rule compiles for
/// that profile)`.
///
/// The last is what the compiler checks (`trigger.rs`, `tag_service`): a
/// `trino`-typed profile hosts a tag rule only when its
/// `config_jsonb.tagService` is a string naming an existing
/// `policy.authz_services` row; any other profile type needs nothing more.
type Attached = (Uuid, Uuid, String, String, bool, bool);

/// The engines attached to any of `catalog_ids`, in ONE query.
async fn attached_engines(
    conn: &mut sqlx::PgConnection,
    catalog_ids: &[Uuid],
) -> Result<Vec<Attached>, PermHttpError> {
    if catalog_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT a.catalog_id, e.id, e.name, e.type, e.authz_service_id IS NOT NULL, \
                (s.type IS DISTINCT FROM 'trino' \
                 OR (jsonb_typeof(s.config_jsonb -> 'tagService') = 'string' \
                     AND EXISTS (SELECT 1 FROM policy.authz_services t \
                                  WHERE t.name = s.config_jsonb ->> 'tagService'))) \
           FROM registry.engine_catalog_attachments a \
           JOIN engine_mgmt.engines e ON e.id = a.engine_id \
           LEFT JOIN policy.authz_services s ON s.id = e.authz_service_id \
          WHERE a.catalog_id = ANY($1) ORDER BY e.name, e.id",
    )
    .bind(catalog_ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)
}

/// Why nothing enforces a catalog no policy profile serves (explain's C5 line):
/// no engine attached to it, or engines with no policy profile. The same
/// attachment read as [`attached_engines`].
pub(crate) async fn catalog_not_enforced(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<NotEnforced, PermHttpError> {
    Ok(if attached_engines(conn, &[catalog_id]).await?.is_empty() {
        NotEnforced::NoEngine
    } else {
        NotEnforced::NoProfile
    })
}

/// Who enforces a rule that reaches `reached`, given the `attached` engines:
/// per catalog, the distinct enforcing engines, and why none does. A
/// `tag_rule` (classification selector) is enforced only by engines whose
/// profile can host it (the last field of [`Attached`]).
fn enforcement(
    reached: &[(Uuid, String)],
    attached: &[Attached],
    tag_rule: bool,
) -> (Vec<EffectCatalog>, u32, Option<NotEnforced>) {
    let mut catalogs = Vec::with_capacity(reached.len());
    let mut engine_ids = BTreeSet::new();
    for (catalog_id, catalog_name) in reached {
        let here: Vec<_> = attached.iter().filter(|a| a.0 == *catalog_id).collect();
        let bound = here.iter().any(|a| a.4);
        let engines: Vec<EffectEngine> = here
            .iter()
            .filter(|a| a.4 && (!tag_rule || a.5))
            .map(|a| EffectEngine {
                id: a.1,
                name: a.2.clone(),
                engine_type: a.3.clone(),
            })
            .collect();
        engine_ids.extend(engines.iter().map(|e| e.id));
        let not_enforced = if here.is_empty() {
            Some(NotEnforced::NoEngine)
        } else if !bound {
            Some(NotEnforced::NoProfile)
        } else if engines.is_empty() {
            Some(NotEnforced::NoTagService)
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
    let not_enforced = if catalogs.is_empty() {
        Some(NotEnforced::NoAssignment)
    } else if !engine_ids.is_empty() {
        None
    } else if catalogs
        .iter()
        .any(|c| c.not_enforced == Some(NotEnforced::NoTagService))
    {
        Some(NotEnforced::NoTagService)
    } else if catalogs
        .iter()
        .any(|c| c.not_enforced == Some(NotEnforced::NoProfile))
    {
        Some(NotEnforced::NoProfile)
    } else {
        Some(NotEnforced::NoEngine)
    };
    (
        catalogs,
        u32::try_from(engine_ids.len()).unwrap_or(u32::MAX),
        not_enforced,
    )
}

/// A tag selector's reach (its catalogs come from [`reached_catalogs`]): every
/// assignment of the tag in a warehouse a registry catalog fronts. Counted in SQL; only the first
/// [`COORD_SCAN_CAP`] columns are loaded as targets.
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

    let targets: Vec<Target> = if is_mask {
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
            .map(|(catalog_id, catalog_name, schema, table, column)| Target {
                catalog_id,
                catalog_name,
                schema,
                table,
                column: ColumnSel::Exact(column),
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Reach {
        targets,
        targets_truncated: is_mask && columns > COORD_SCAN_CAP,
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

/// One classification assignment on a target table: `(target index, column
/// — "" for the whole table, classification name)`.
type Classified = (i64, String, String);

/// The classifications, among `names`' keys, on every target table — ONE
/// query. A target's own column filter is applied in SQL for an exact column
/// and in Rust for a pattern. `lower()` on schema/table/column keeps the
/// engines' case-insensitive level matching, so of
/// `catalog_tag_assignments_res_idx (warehouse_id, schema_name, table_name)`
/// only the `warehouse_id` prefix is usable; the key filter on `tags` bounds
/// the rest, and the row cap bounds the whole read.
async fn classifications_on(
    conn: &mut sqlx::PgConnection,
    targets: &[Target],
    keys: &[String],
) -> Result<(Vec<Classified>, bool), PermHttpError> {
    let catalog_ids: Vec<Uuid> = targets.iter().map(|t| t.catalog_id).collect();
    let schemas: Vec<String> = targets.iter().map(|t| t.schema.clone()).collect();
    let tables: Vec<String> = targets.iter().map(|t| t.table.clone()).collect();
    let columns: Vec<Option<String>> = targets
        .iter()
        .map(|t| match &t.column {
            ColumnSel::Exact(c) => Some(c.clone()),
            ColumnSel::Pattern(_) | ColumnSel::Any => None,
        })
        .collect();
    let rows: Vec<Classified> = sqlx::query_as(
        "SELECT x.i, a.column_name, t.name \
           FROM unnest($1::uuid[], $2::text[], $3::text[], $4::text[]) \
                WITH ORDINALITY AS x(catalog_id, schema_name, table_name, column_name, i) \
           JOIN registry.catalogs c ON c.id = x.catalog_id \
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id \
                AND lower(a.schema_name) = lower(x.schema_name) \
                AND lower(a.table_name) = lower(x.table_name) \
                AND (a.column_name = '' OR x.column_name IS NULL \
                     OR lower(a.column_name) = lower(x.column_name)) \
           JOIN tags t ON t.id = a.tag_id \
          WHERE t.key = ANY($5) \
          ORDER BY x.i, a.column_name, t.name \
          LIMIT $6",
    )
    .bind(&catalog_ids)
    .bind(&schemas)
    .bind(&tables)
    .bind(&columns)
    .bind(keys)
    .bind(CLASSIFICATION_ROW_CAP)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;
    let truncated = rows.len() as i64 >= CLASSIFICATION_ROW_CAP;
    Ok((rows, truncated))
}

/// Other stored mask rules (not `editing`) that reach a column of one of
/// `targets`, at most [`ALREADY_MASKED_CAP`], deduplicated by `(rule,
/// coordinate)`. The bool says a cap cut the answer.
async fn already_masked(
    conn: &mut sqlx::PgConnection,
    editing: Option<Uuid>,
    targets: &[Target],
) -> Result<(Vec<ExistingMask>, bool), PermHttpError> {
    if targets.is_empty() {
        return Ok((Vec::new(), false));
    }
    let masks: Vec<Rule> = egret_perm::list_rules_via(&mut *conn)
        .await
        .map_err(PermHttpError::db)?
        .into_iter()
        .filter(|r| matches!(r.kind, RuleKind::Mask { .. }) && Some(r.id) != editing)
        .collect();
    if masks.is_empty() {
        return Ok((Vec::new(), false));
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
    let (classified, mut truncated) = if tag_names.is_empty() {
        (Vec::new(), false)
    } else {
        classifications_on(conn, targets, &keys).await?
    };

    let mut out = Vec::new();
    let mut seen: BTreeSet<(Uuid, String)> = BTreeSet::new();
    let mut push = |out: &mut Vec<ExistingMask>, rule: &Rule, coordinate: String| -> bool {
        if !seen.insert((rule.id, coordinate.clone())) {
            return true;
        }
        if out.len() >= ALREADY_MASKED_CAP {
            return false;
        }
        if let RuleKind::Mask { mask_type, .. } = &rule.kind {
            out.push(ExistingMask {
                rule_id: rule.id,
                rule_name: rule.name.clone(),
                mask_type: mask_type.clone(),
                coordinate,
                exempt: rule.exempt.clone(),
            });
        }
        true
    };

    'targets: for (index, target) in targets.iter().enumerate() {
        // `WITH ORDINALITY` counts from 1.
        let here: Vec<(&str, &str)> = classified
            .iter()
            .filter(|(i, column, _)| *i == index as i64 + 1 && target.reaches_assigned(column))
            .map(|(_, column, name)| (column.as_str(), name.as_str()))
            .collect();
        for rule in &masks {
            match &rule.selector {
                Selector::Path(s) => {
                    let level = |pattern: &Option<String>, value: &str| {
                        pattern
                            .as_deref()
                            .is_none_or(|p| resource_level_matches(p, value))
                    };
                    if s.catalog_id == target.catalog_id
                        && level(&s.schema, &target.schema)
                        && level(&s.table, &target.table)
                        && target.column_meets(s.column.as_deref())
                    {
                        let column = target.overlap_column(s.column.as_deref());
                        if !push(&mut out, rule, target.coordinate(&column)) {
                            truncated = true;
                            break 'targets;
                        }
                    }
                }
                // Case-sensitive, as the engines match tag names.
                Selector::Tag { key } => {
                    let Some(name) = tag_names.get(key) else {
                        continue;
                    };
                    for &(column, classification) in &here {
                        if glob_match(name, classification) {
                            let column = target.overlap_column(Some(column));
                            if !push(&mut out, rule, target.coordinate(&column)) {
                                truncated = true;
                                break 'targets;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok((out, truncated))
}
