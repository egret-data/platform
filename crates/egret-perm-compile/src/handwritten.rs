//! DG #4b (spec `docs/en/specs/2026-10-19-dg-classification-design.md` §4.1,
//! CL7): **the one place that decides how a hand-written Ranger policy reaches
//! a Data object, a principal or a classification.**
//!
//! Three read routes use it — hand-written policies reaching a Data object
//! (route A), naming a principal (route B) and naming a classification's tag
//! (route D) — and explain builds its request with [`object_request`], so
//! "what explain evaluated" and "what this module matched" are the same
//! resource. The match itself is `egret-ranger-eval`'s matcher
//! ([`PreparedPolicyResources::matches`], C2); nothing here re-implements a
//! value comparison.
//!
//! # The match levels (Q2, premise 6)
//!
//! For a RESOURCE request Ranger evaluates a policy's items only on `SELF` or
//! `SELF_AND_ALL_DESCENDANTS` (`egret_ranger_eval::resource` module doc,
//! measured live). So:
//!
//! | `MatchType` (best over every resource map) | [`ObjectMatch`] |
//! |---|---|
//! | `Self_`, `SelfAndAllDescendants` | `Applies` — the policy applies to this object |
//! | `Descendant` | `Part` — the policy names something below this object |
//! | `Ancestor` | `Parent` — it names a parent only; engines do not apply it to this object's data |
//! | `None` | `None` |
//!
//! Task 1 M2 of `docs/en/plans/2026-10-19-dg-classification-live.md` measured
//! each shape against real Trino 483 and StarRocks 4.1.6.
//!
//! # Groups and roles reach nobody on an engine (Q1, premise 4)
//!
//! egret runs no usersync, writes no group provider and serves no roles
//! download, so a hand-written item naming a group (other than `public`) or a
//! role matches nobody on the engine (Task 1 M3). [`item_summary`] marks every
//! such principal `on_engines: false`; `public` and the user `{USER}` are
//! "everyone".

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::EngineType;
use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, get_service_def_via, service_id_by_name_via,
    usable_tag_services_via, PolicyStoreError,
};
use egret_ranger_eval::model::{ItemBucket, PolicyItem, RangerPolicy, ResourceLevel, Unmodelled};
use egret_ranger_eval::resource::{
    MatchType, PreparedPolicyResources, RequestResource, ResourceMapSource,
};
use egret_ranger_eval::servicedef::ServiceDef;
use serde::Serialize;
use uuid::Uuid;

use crate::grants::resource_level_names;
use crate::trigger::engine_type_for;

/// The Data object a request names: a catalog, and optionally a schema, a
/// table and a column, narrowing progressively. The same shape explain's
/// target has, so the two cannot build different requests.
pub use egret_perm::Scope as ObjectTarget;

/// The group every user is in, on every engine.
const PUBLIC: &str = egret_perm::PUBLIC_GROUP;
/// Ranger's "the requesting user" token in an item's `users`.
const ANY_USER: &str = "{USER}";

/// The resource ONE access request names, in the levels the target engine's
/// service-def declares (Trino's middle level is `schema`, StarRocks' is
/// `database`).
///
/// **A level the target does not name is ABSENT, not `*`.** That is what an
/// engine's own request looks like: the plugin names exactly the levels the
/// query touched. It is also not the same thing as the compiler's `*`, which
/// is a POLICY value meaning "every child at this level" — a policy naming
/// `catalog/*/*/*` against a request naming only `catalog` is Ranger's
/// `SELF_AND_ALL_DESCENDANTS`, which matches; a request padded with `*` up to
/// the leaf would be asking about a different resource.
///
/// Stopping at the first absent level is the shape an engine request has, and
/// it is only correct because a gap below a named level cannot reach here:
/// `explain` refuses `{schema: None, table: Some(_)}` with a 400 at the top of
/// its handler, and route A refuses the same shapes with a 400 (spec §4.2).
/// Breaking here under a gap would answer about the nearest named ANCESTOR —
/// fail-open — so both callers must keep their guard.
pub fn object_request(
    engine: EngineType,
    catalog_name: &str,
    target: &ObjectTarget,
) -> RequestResource {
    let names = resource_level_names(engine);
    let mut resource = RequestResource::new();
    resource.insert(names[0], catalog_name);
    for (name, value) in [
        (names[1], target.schema.as_deref()),
        (names[2], target.table.as_deref()),
        (names[3], target.column.as_deref()),
    ] {
        match value {
            Some(v) => resource.insert(name, v),
            None => break,
        }
    }
    resource
}

/// How a policy's resources reach one object (Q2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectMatch {
    /// `Self_` or `SelfAndAllDescendants`: the policy applies to this object.
    /// `through_pattern` when a level the request names was matched by `*`,
    /// `?`, an exclude or `isRecursive` rather than by its literal value.
    Applies {
        through_pattern: bool,
    },
    /// `Descendant`: the policy names something below this object.
    Part,
    /// `Ancestor`: the policy names a parent of this object only.
    Parent,
    None,
}

fn rank(m: MatchType) -> u8 {
    match m {
        MatchType::Self_ | MatchType::SelfAndAllDescendants => 3,
        MatchType::Descendant => 2,
        MatchType::Ancestor => 1,
        MatchType::None => 0,
    }
}

fn resource_map(
    policy: &RangerPolicy,
    source: ResourceMapSource,
) -> Option<&BTreeMap<String, ResourceLevel>> {
    match source {
        ResourceMapSource::Primary => Some(&policy.resources),
        ResourceMapSource::Additional(i) => policy.additional_resources.get(i),
    }
}

/// Whether `map` reached `request` through a pattern at any level the request
/// names: a level whose values hold no literal equal (case-folded) to the
/// request's value, or a level marked `isExcludes`/`isRecursive`.
fn through_pattern(map: &BTreeMap<String, ResourceLevel>, request: &RequestResource) -> bool {
    request.keys().any(|key| {
        let Some(level) = map.get(key) else {
            return false;
        };
        if level.is_excludes || level.is_recursive {
            return true;
        }
        let value = request.get(key).unwrap_or_default();
        !level.values.iter().any(|v| v.eq_ignore_ascii_case(value))
    })
}

/// The best verdict of every resource map of `policy` on `request`, mapped as
/// Q2. `prepared` must have been built from `policy`.
pub fn object_match(
    prepared: &PreparedPolicyResources<'_>,
    policy: &RangerPolicy,
    request: &RequestResource,
) -> ObjectMatch {
    let matches = prepared.matches(request);
    let best = matches
        .iter()
        .map(|m| rank(m.match_type))
        .max()
        .unwrap_or(0);
    match best {
        3 => {
            // Literal when ANY applying map reached it literally.
            let through = matches.iter().filter(|m| rank(m.match_type) == 3).all(|m| {
                resource_map(policy, m.source).is_none_or(|map| through_pattern(map, request))
            });
            ObjectMatch::Applies {
                through_pattern: through,
            }
        }
        2 => ObjectMatch::Part,
        1 => ObjectMatch::Parent,
        _ => ObjectMatch::None,
    }
}

/// Which of a policy's six item lists an item came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Allow,
    Deny,
    AllowException,
    DenyException,
    Mask,
    RowFilter,
}

impl Bucket {
    fn of(bucket: ItemBucket) -> Bucket {
        match bucket {
            ItemBucket::PolicyItems => Bucket::Allow,
            ItemBucket::DenyPolicyItems => Bucket::Deny,
            ItemBucket::AllowExceptions => Bucket::AllowException,
            ItemBucket::DenyExceptions => Bucket::DenyException,
            ItemBucket::DataMaskPolicyItems => Bucket::Mask,
            ItemBucket::RowFilterPolicyItems => Bucket::RowFilter,
        }
    }
}

/// A group or role an item names. `on_engines` is false for every one: engines
/// resolve no groups (but `public`, which is reported as `everyone` instead)
/// and no roles (Q1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalMark {
    pub name: String,
    pub on_engines: bool,
}

/// One item of a hand-written policy, as a page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub bucket: Bucket,
    /// Usernames, `{USER}` excluded (it is `everyone`).
    pub users: Vec<String>,
    /// Groups, `public` excluded (it is `everyone`).
    pub groups: Vec<PrincipalMark>,
    pub roles: Vec<PrincipalMark>,
    /// The item names the group `public` or the user `{USER}`.
    pub everyone: bool,
    /// Access types, in the item's order.
    pub accesses: Vec<String>,
    /// A mask item's `dataMaskType`.
    pub mask_type: Option<String>,
    /// A row-filter item's `filterExpr`.
    pub row_filter: Option<String>,
}

/// Every item list of `policy` in Ranger's bucket order.
pub(crate) fn buckets(policy: &RangerPolicy) -> [(ItemBucket, &[PolicyItem]); 6] {
    [
        (ItemBucket::PolicyItems, &policy.policy_items),
        (ItemBucket::DenyPolicyItems, &policy.deny_policy_items),
        (ItemBucket::AllowExceptions, &policy.allow_exceptions),
        (ItemBucket::DenyExceptions, &policy.deny_exceptions),
        (
            ItemBucket::DataMaskPolicyItems,
            &policy.data_mask_policy_items,
        ),
        (
            ItemBucket::RowFilterPolicyItems,
            &policy.row_filter_policy_items,
        ),
    ]
}

fn item_names_everyone(item: &PolicyItem) -> bool {
    item.groups.iter().any(|g| g == PUBLIC) || item.users.iter().any(|u| u == ANY_USER)
}

/// One [`ItemSummary`] per item of `policy`, bucket by bucket.
pub fn item_summary(policy: &RangerPolicy) -> Vec<ItemSummary> {
    let mut out = Vec::new();
    for (bucket, items) in buckets(policy) {
        for item in items {
            out.push(ItemSummary {
                bucket: Bucket::of(bucket),
                users: item
                    .users
                    .iter()
                    .filter(|u| *u != ANY_USER)
                    .cloned()
                    .collect(),
                groups: item
                    .groups
                    .iter()
                    .filter(|g| *g != PUBLIC)
                    .map(|g| PrincipalMark {
                        name: g.clone(),
                        on_engines: false,
                    })
                    .collect(),
                roles: item
                    .roles
                    .iter()
                    .map(|r| PrincipalMark {
                        name: r.clone(),
                        on_engines: false,
                    })
                    .collect(),
                everyone: item_names_everyone(item),
                accesses: item
                    .accesses
                    .iter()
                    .map(|a| a.access_type.clone())
                    .collect(),
                mask_type: item
                    .data_mask_info
                    .as_ref()
                    .and_then(|m| m.mask_type.clone()),
                row_filter: item
                    .row_filter_info
                    .as_ref()
                    .and_then(|r| r.filter_expr.clone()),
            });
        }
    }
    out
}

/// `policyType` as the routes name it.
pub fn policy_type_word(policy_type: i32) -> &'static str {
    match policy_type {
        1 => "masking",
        2 => "rowfilter",
        _ => "access",
    }
}

/// The order levels are written in a resource summary.
const LEVEL_ORDER: [&str; 6] = ["catalog", "schema", "database", "table", "column", "tag"];

/// The primary resource map as one line: `catalog=ice; schema=s; table=*` in
/// hierarchy order (other keys after, by name), several values joined with
/// `,`, an excluded level prefixed `not `, a recursive one suffixed
/// ` (recursive)`.
pub fn resource_summary(policy: &RangerPolicy) -> String {
    let mut keys: Vec<&String> = policy.resources.keys().collect();
    keys.sort_by_key(|k| {
        (
            LEVEL_ORDER
                .iter()
                .position(|o| o == k)
                .unwrap_or(LEVEL_ORDER.len()),
            (*k).clone(),
        )
    });
    keys.into_iter()
        .map(|k| {
            let level = &policy.resources[k];
            let mut s = format!(
                "{k}={}{}",
                if level.is_excludes { "not " } else { "" },
                level.values.join(",")
            );
            if level.is_recursive {
                s.push_str(" (recursive)");
            }
            s
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One sentence per construct of `policy` egret does not evaluate (CL9),
/// deduplicated, in first-seen order. The match is still reported; these say
/// why "applies" is not "decides".
pub fn unmodelled_sentences(policy: &RangerPolicy) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for u in &policy.unmodelled {
        let s = match u {
            Unmodelled::ValiditySchedules => {
                "Has a validity schedule egret does not evaluate.".to_string()
            }
            Unmodelled::PolicyConditions => "Has conditions egret does not evaluate.".to_string(),
            Unmodelled::ItemConditions { .. } => {
                "Has item conditions egret does not evaluate.".to_string()
            }
            Unmodelled::ZoneName(z) => {
                format!("Belongs to security zone {z}, which egret does not evaluate.")
            }
            Unmodelled::MaskExpressionMarker { .. } | Unmodelled::MaskConditionExpr { .. } => {
                "Has a mask expression egret does not evaluate.".to_string()
            }
            Unmodelled::RowFilterExpressionMarker { .. } => {
                "Has a row-filter expression egret does not evaluate.".to_string()
            }
            Unmodelled::ResourceValueExpressionMarker { .. } => {
                "Has a resource value expression egret does not evaluate.".to_string()
            }
            Unmodelled::UserExpressionMarker { .. } => {
                "Names users through an expression egret does not evaluate.".to_string()
            }
            Unmodelled::ExplicitDeny { .. } => {
                "Has a denied access inside an allow item, which egret does not evaluate."
                    .to_string()
            }
            Unmodelled::DelegateAdmin { .. } => {
                "Has delegated admin items, which egret does not evaluate.".to_string()
            }
            // Envelope-level constructs never sit on one policy; named anyway.
            other => format!("Has a construct egret does not evaluate ({other:?})."),
        };
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    out
}

// ── The routes' reads (spec §4.2, §4.3, §4.5) ────────────────────────────

/// Why a read could not answer.
#[derive(Debug, thiserror::Error)]
pub enum HandWrittenError {
    /// The catalog, principal or tag does not exist.
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("policy store: {0}")]
    Store(#[from] PolicyStoreError),
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenCatalog {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenTarget {
    pub schema: Option<String>,
    pub table: Option<String>,
    pub column: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListedState {
    Listed,
    NotDetermined,
}

/// Why route A cannot say which hand-written policies reach the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotDeterminedReason {
    /// No engine attaches the catalog.
    NoEngineAttached,
    /// The attached engines bind no Trino or StarRocks profile.
    NoProfile,
    /// No profile's engines reach the catalog.
    CatalogNotInProfile,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenEngine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    /// `provisioned` or `external`. External engines are read from the
    /// database only; nothing is dialled.
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenProfile {
    pub service_id: i64,
    pub name: String,
    pub r#type: String,
    /// The profile's usable tag service, if any.
    pub tag_service: Option<String>,
    pub engines: Vec<HandWrittenEngine>,
}

/// How a policy reaches the object (Q2), or `unknown` when its body cannot be
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchWord {
    Applies,
    Part,
    Parent,
    Unknown,
}

/// Where the tag a tag policy matched sits relative to the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TagOn {
    /// On the object itself.
    This,
    /// On the table of the column asked about.
    Parent,
    /// On something below the object (a column of the table, a table of the
    /// schema, …).
    Below,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViaTag {
    pub name: String,
    pub on: TagOn,
    /// The tagged column, when the tag is on a column.
    pub column: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenPolicy {
    pub id: i64,
    pub name: String,
    /// The profile (or tag service) that stores it.
    pub service: String,
    pub in_tag_service: bool,
    /// `access`, `masking` or `rowfilter`.
    pub policy_type: String,
    pub r#match: MatchWord,
    pub through_pattern: bool,
    pub via_tag: Option<ViaTag>,
    pub resource_summary: String,
    /// `policy.authz_policies.enabled`, never the body's `isEnabled`.
    pub enabled: bool,
    pub deny_all_else: bool,
    pub unmodelled: Vec<String>,
    pub items: Vec<ItemSummary>,
    /// Set when the body could not be read; `match` is then `unknown`.
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeededCount {
    pub service: String,
    pub count: i64,
}

/// Route A's answer.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectHandWritten {
    pub catalog: HandWrittenCatalog,
    pub target: HandWrittenTarget,
    pub state: ListedState,
    pub reason: Option<NotDeterminedReason>,
    pub profiles: Vec<HandWrittenProfile>,
    pub policies: Vec<HandWrittenPolicy>,
    /// egret-seeded (`infra`) policies that reach the object, counted per
    /// profile and never listed.
    pub seeded: Vec<SeededCount>,
}

/// One `policy.authz_policies` row a route reads.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PolicyRow {
    id: i64,
    name: String,
    enabled: bool,
    managed_by: String,
    policy_jsonb: serde_json::Value,
}

async fn policy_rows_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<PolicyRow>, HandWrittenError> {
    Ok(sqlx::query_as::<_, PolicyRow>(
        "SELECT id, name, enabled, managed_by, policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND managed_by IN ('operator', 'infra') ORDER BY name, id",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// Service-defs by type name, parsed once per request.
#[derive(Default)]
struct Defs(BTreeMap<String, ServiceDef>);

impl Defs {
    async fn get(
        &mut self,
        conn: &mut sqlx::PgConnection,
        r#type: &str,
    ) -> Result<&ServiceDef, HandWrittenError> {
        if !self.0.contains_key(r#type) {
            let row = get_service_def_via(conn, r#type).await?;
            self.0
                .insert(r#type.to_string(), ServiceDef::parse(&row.def_jsonb));
        }
        Ok(&self.0[r#type])
    }
}

fn parse_policy(row: &PolicyRow) -> Result<RangerPolicy, String> {
    serde_json::from_value::<RangerPolicy>(row.policy_jsonb.clone()).map_err(|e| e.to_string())
}

fn policy_row(
    row: &PolicyRow,
    service: &str,
    in_tag_service: bool,
    policy: Result<&RangerPolicy, &str>,
    matched: MatchWord,
    through_pattern: bool,
    via_tag: Option<ViaTag>,
) -> HandWrittenPolicy {
    match policy {
        Ok(p) => HandWrittenPolicy {
            id: row.id,
            name: row.name.clone(),
            service: service.to_string(),
            in_tag_service,
            policy_type: policy_type_word(p.policy_type).to_string(),
            r#match: matched,
            through_pattern,
            via_tag,
            resource_summary: resource_summary(p),
            enabled: row.enabled,
            deny_all_else: p.is_deny_all_else,
            unmodelled: unmodelled_sentences(p),
            items: item_summary(p),
            parse_error: None,
        },
        Err(e) => HandWrittenPolicy {
            id: row.id,
            name: row.name.clone(),
            service: service.to_string(),
            in_tag_service,
            policy_type: row
                .policy_jsonb
                .get("policyType")
                .and_then(|v| v.as_i64())
                .map(|c| policy_type_word(c as i32))
                .unwrap_or("access")
                .to_string(),
            r#match: MatchWord::Unknown,
            through_pattern: false,
            via_tag: None,
            resource_summary: String::new(),
            enabled: row.enabled,
            deny_all_else: false,
            unmodelled: Vec::new(),
            items: Vec::new(),
            parse_error: Some(e.to_string()),
        },
    }
}

/// The level a request may not name as `""` or `*`: either is Ranger's "every
/// value" (`requests_all_values`), not an object.
fn check_target(target: &ObjectTarget) -> Result<(), HandWrittenError> {
    for (name, v) in [
        ("schema", &target.schema),
        ("table", &target.table),
        ("column", &target.column),
    ] {
        if let Some(v) = v {
            if v.trim().is_empty() || v == "*" {
                return Err(HandWrittenError::BadRequest(format!(
                    "{name} must name one object, not {v:?}"
                )));
            }
        }
    }
    if target.table.is_some() && target.schema.is_none() {
        return Err(HandWrittenError::BadRequest(
            "table needs schema".to_string(),
        ));
    }
    if target.column.is_some() && target.table.is_none() {
        return Err(HandWrittenError::BadRequest(
            "column needs table".to_string(),
        ));
    }
    Ok(())
}

/// Every tag on the object, above it or below it, with where it sits.
/// Coordinates compare case-folded, as the tag resolver does
/// (`resolved_tags_for_resource_via`).
async fn object_tags_via(
    conn: &mut sqlx::PgConnection,
    target: &ObjectTarget,
) -> Result<Vec<ViaTag>, HandWrittenError> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT t.name, a.schema_name, a.table_name, a.column_name \
           FROM registry.catalogs c \
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id \
           JOIN tags t ON t.id = a.tag_id \
          WHERE c.id = $1 \
          ORDER BY 1, 2, 3, 4",
    )
    .bind(target.catalog_id)
    .fetch_all(&mut *conn)
    .await?;
    let eq = |a: &str, b: &Option<String>| {
        b.as_deref()
            .is_some_and(|b| a.to_lowercase() == b.to_lowercase())
    };
    let mut out = Vec::new();
    for (name, schema, table, column) in rows {
        let col = (!column.is_empty()).then(|| column.clone());
        let on = match (&target.schema, &target.table, &target.column) {
            (None, _, _) => Some(TagOn::Below),
            (Some(_), None, _) => eq(&schema, &target.schema).then_some(TagOn::Below),
            (Some(_), Some(_), None) => (eq(&schema, &target.schema) && eq(&table, &target.table))
                .then_some(if column.is_empty() {
                    TagOn::This
                } else {
                    TagOn::Below
                }),
            (Some(_), Some(_), Some(_)) => {
                if !(eq(&schema, &target.schema) && eq(&table, &target.table)) {
                    None
                } else if column.is_empty() {
                    Some(TagOn::Parent)
                } else if eq(&column, &target.column) {
                    Some(TagOn::This)
                } else {
                    None
                }
            }
        };
        if let Some(on) = on {
            out.push(ViaTag {
                name,
                on,
                column: col,
            });
        }
    }
    Ok(out)
}

async fn engines_of_profile_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<HandWrittenEngine>, HandWrittenError> {
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT id, name, type, origin FROM engine_mgmt.engines \
          WHERE authz_service_id = $1 ORDER BY name, id",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, r#type, origin)| HandWrittenEngine {
            id,
            name,
            r#type,
            origin,
        })
        .collect())
}

fn match_word(m: ObjectMatch) -> Option<(MatchWord, bool)> {
    match m {
        ObjectMatch::Applies { through_pattern } => Some((MatchWord::Applies, through_pattern)),
        ObjectMatch::Part => Some((MatchWord::Part, false)),
        ObjectMatch::Parent => Some((MatchWord::Parent, false)),
        ObjectMatch::None => None,
    }
}

/// Route A (spec §4.2): every hand-written (`operator`) policy of every
/// profile reaching `target.catalog_id`, and of each Trino profile's tag
/// service, that reaches the object. The caller runs it in one read-only
/// snapshot.
pub async fn handwritten_for_object_via(
    conn: &mut sqlx::PgConnection,
    target: &ObjectTarget,
) -> Result<ObjectHandWritten, HandWrittenError> {
    check_target(target)?;
    let catalog: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM registry.catalogs WHERE id = $1")
            .bind(target.catalog_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((catalog_id, catalog_name)) = catalog else {
        return Err(HandWrittenError::NotFound);
    };
    let mut out = ObjectHandWritten {
        catalog: HandWrittenCatalog {
            id: catalog_id,
            name: catalog_name,
        },
        target: HandWrittenTarget {
            schema: target.schema.clone(),
            table: target.table.clone(),
            column: target.column.clone(),
        },
        state: ListedState::NotDetermined,
        reason: None,
        profiles: Vec::new(),
        policies: Vec::new(),
        seeded: Vec::new(),
    };

    let attached: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT e.authz_service_id FROM registry.engine_catalog_attachments ea \
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
          WHERE ea.catalog_id = $1",
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?;
    if attached.is_empty() {
        out.reason = Some(NotDeterminedReason::NoEngineAttached);
        return Ok(out);
    }
    let service_ids: BTreeSet<i64> = attached.into_iter().flatten().collect();
    let mut profiles = Vec::new();
    for sid in service_ids {
        let svc = get_service_by_id_via(&mut *conn, sid).await?;
        if let Some(engine) = engine_type_for(&svc.r#type) {
            profiles.push((svc, engine));
        }
    }
    if profiles.is_empty() {
        out.reason = Some(NotDeterminedReason::NoProfile);
        return Ok(out);
    }
    // The catalog name each profile's engines carry in a request (M1: the
    // registry name on both engines).
    let mut reaching = Vec::new();
    for (svc, engine) in profiles {
        if let Some(c) = catalogs_for_service_via(&mut *conn, svc.id)
            .await?
            .into_iter()
            .find(|c| c.id == catalog_id)
        {
            reaching.push((svc, engine, c.name));
        }
    }
    if reaching.is_empty() {
        out.reason = Some(NotDeterminedReason::CatalogNotInProfile);
        return Ok(out);
    }
    out.state = ListedState::Listed;

    let mut defs = Defs::default();
    let object_tags = object_tags_via(&mut *conn, target).await?;
    let mut seen_tag_services = BTreeSet::new();
    for (svc, engine, catalog_name) in reaching {
        let tag_service = usable_tag_services_via(&mut *conn, &[svc.id])
            .await?
            .into_iter()
            .next()
            .map(|(_, name)| name);
        out.profiles.push(HandWrittenProfile {
            service_id: svc.id,
            name: svc.name.clone(),
            r#type: svc.r#type.clone(),
            tag_service: tag_service.clone(),
            engines: engines_of_profile_via(&mut *conn, svc.id).await?,
        });

        // Resource policies.
        let request = object_request(engine, &catalog_name, target);
        let rows = policy_rows_via(&mut *conn, svc.id).await?;
        let def = defs.get(&mut *conn, &svc.r#type).await?;
        let mut seeded = 0i64;
        for row in &rows {
            let parsed = parse_policy(row);
            let found = match &parsed {
                Ok(p) => {
                    let prepared = PreparedPolicyResources::new(p, def);
                    match_word(object_match(&prepared, p, &request))
                }
                Err(_) => Some((MatchWord::Unknown, false)),
            };
            let Some((word, through)) = found else {
                continue;
            };
            if row.managed_by == "infra" {
                if word != MatchWord::Unknown {
                    seeded += 1;
                }
                continue;
            }
            out.policies.push(policy_row(
                row,
                &svc.name,
                false,
                parsed.as_ref().map_err(String::as_str),
                word,
                through,
                None,
            ));
        }
        if seeded > 0 {
            out.seeded.push(SeededCount {
                service: svc.name.clone(),
                count: seeded,
            });
        }

        // Tag policies, once per tag service.
        let Some(tag_service) = tag_service else {
            continue;
        };
        if !seen_tag_services.insert(tag_service.clone()) {
            continue;
        }
        let Some(tag_sid) = service_id_by_name_via(&mut *conn, &tag_service).await? else {
            continue;
        };
        let tag_rows = policy_rows_via(&mut *conn, tag_sid).await?;
        let tag_def = defs.get(&mut *conn, "tag").await?;
        let mut tag_seeded = 0i64;
        for row in &tag_rows {
            let parsed = parse_policy(row);
            let found: Option<(MatchWord, bool, Option<ViaTag>)> = match &parsed {
                Ok(p) => {
                    let prepared = PreparedPolicyResources::new(p, tag_def);
                    object_tags
                        .iter()
                        .filter_map(|t| {
                            let request = RequestResource::new().with("tag", t.name.as_str());
                            match object_match(&prepared, p, &request) {
                                ObjectMatch::Applies { through_pattern } => {
                                    Some((through_pattern, t))
                                }
                                _ => None,
                            }
                        })
                        .min_by_key(|(_, t)| t.on)
                        .map(|(through, t)| {
                            let word = if t.on == TagOn::Below {
                                MatchWord::Part
                            } else {
                                MatchWord::Applies
                            };
                            (word, through, Some(t.clone()))
                        })
                }
                Err(_) => Some((MatchWord::Unknown, false, None)),
            };
            let Some((word, through, via)) = found else {
                continue;
            };
            if row.managed_by == "infra" {
                if word != MatchWord::Unknown {
                    tag_seeded += 1;
                }
                continue;
            }
            out.policies.push(policy_row(
                row,
                &tag_service,
                true,
                parsed.as_ref().map_err(String::as_str),
                word,
                through,
                via,
            ));
        }
        if tag_seeded > 0 {
            out.seeded.push(SeededCount {
                service: tag_service.clone(),
                count: tag_seeded,
            });
        }
    }
    out.policies
        .sort_by(|a, b| (a.r#match, &a.service, &a.name).cmp(&(b.r#match, &b.service, &b.name)));
    Ok(out)
}

// ── Route B: hand-written policies naming a principal (spec §4.3, Q1) ─────

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalRef {
    /// `user`, `group` or `role`.
    pub kind: String,
    /// The user's id, or the group's or role's name.
    pub id: String,
    pub label: String,
    /// A user's engine username; `null` for groups, roles and a user without one.
    pub username: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalState {
    Listed,
    /// The user has no username, so no hand-written item can name them on an
    /// engine; only the `everyone` policies are reported.
    NoUsername,
}

/// How an item names the principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamingHow {
    /// By username (a user page), or the group or role itself (its own page).
    Direct,
    /// By a group: engines resolve no groups, so it reaches nobody there (Q1).
    Group,
    /// By a role: engines resolve no roles, so it reaches nobody there (Q1).
    Role,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Naming {
    pub bucket: Bucket,
    pub how: NamingHow,
    /// The group or role named, for `group` and `role`.
    pub via: Option<String>,
    /// Whether an engine applies the item to this principal.
    pub on_engines: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenEngineRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalPolicy {
    pub id: i64,
    pub name: String,
    pub service: String,
    pub in_tag_service: bool,
    pub policy_type: String,
    pub resource_summary: String,
    pub enabled: bool,
    pub deny_all_else: bool,
    pub unmodelled: Vec<String>,
    pub naming: Vec<Naming>,
    /// The engines that download the policy.
    pub engines: Vec<HandWrittenEngineRef>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EveryonePolicy {
    pub id: i64,
    pub name: String,
    pub service: String,
    pub bucket: Bucket,
}

/// Policies with an item naming `public` or `{USER}`: they apply to everyone,
/// never attributed to this principal.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EveryoneSummary {
    /// Distinct policies.
    pub count: i64,
    /// One entry per (policy, bucket).
    pub policies: Vec<EveryonePolicy>,
}

/// Route B's answer.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalHandWritten {
    pub principal: PrincipalRef,
    pub state: PrincipalState,
    pub policies: Vec<PrincipalPolicy>,
    pub everyone: EveryoneSummary,
}

fn internal(e: impl std::fmt::Display) -> HandWrittenError {
    HandWrittenError::Internal(e.to_string())
}

/// The engines that download `service`'s policies: those bound to it, or —
/// for a tag service — those bound to a profile naming it as `tagService`.
async fn engines_downloading_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    service_name: &str,
) -> Result<Vec<HandWrittenEngineRef>, HandWrittenError> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT e.id, e.name FROM engine_mgmt.engines e \
           JOIN policy.authz_services s ON s.id = e.authz_service_id \
          WHERE s.id = $1 OR s.config_jsonb ->> 'tagService' = $2 \
          ORDER BY e.name, e.id",
    )
    .bind(service_id)
    .bind(service_name)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| HandWrittenEngineRef { id, name })
        .collect())
}

/// Route B (spec §4.3): every hand-written (`operator`) policy of every
/// profile and tag service whose items name the principal — `kind` is
/// `user` (`id` a user id), `group` or `role` (`id` a name).
pub async fn handwritten_for_principal_via(
    conn: &mut sqlx::PgConnection,
    kind: &str,
    id: &str,
) -> Result<PrincipalHandWritten, HandWrittenError> {
    // Who the principal is on an engine: a username, plus the groups and
    // roles an item could name (Q1: these reach nobody there).
    let (principal, username, groups, roles) = match kind {
        "user" => {
            let uid = Uuid::parse_str(id).map_err(|_| HandWrittenError::NotFound)?;
            let user = crate::people::read_user_via(&mut *conn, uid)
                .await
                .map_err(internal)?
                .ok_or(HandWrittenError::NotFound)?;
            let mut groups = BTreeSet::new();
            let mut roles = BTreeSet::new();
            for s in &user.stands_for {
                match &s.via {
                    crate::people::Via::Group { name } => {
                        groups.insert(name.clone());
                    }
                    crate::people::Via::Role { name } => {
                        roles.insert(name.clone());
                    }
                    _ => {}
                }
            }
            let label = if user.name.trim().is_empty() {
                user.email.clone()
            } else {
                user.name.clone()
            };
            (
                PrincipalRef {
                    kind: "user".into(),
                    id: uid.to_string(),
                    label,
                    username: user.username.clone(),
                },
                user.username,
                groups,
                roles,
            )
        }
        "group" => {
            let g = crate::people::read_group_via(&mut *conn, id)
                .await
                .map_err(internal)?
                .ok_or(HandWrittenError::NotFound)?;
            (
                PrincipalRef {
                    kind: "group".into(),
                    id: g.name.clone(),
                    label: g.name.clone(),
                    username: None,
                },
                None,
                BTreeSet::from([g.name]),
                BTreeSet::new(),
            )
        }
        "role" => {
            let r = crate::people::read_role_via(&mut *conn, id)
                .await
                .map_err(internal)?
                .ok_or(HandWrittenError::NotFound)?;
            (
                PrincipalRef {
                    kind: "role".into(),
                    id: r.name.clone(),
                    label: r.name.clone(),
                    username: None,
                },
                None,
                BTreeSet::new(),
                BTreeSet::from([r.name]),
            )
        }
        other => {
            return Err(HandWrittenError::BadRequest(format!(
                "kind must be user, group or role, not {other:?}"
            )))
        }
    };
    let no_username = kind == "user" && username.is_none();

    let services: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, type FROM policy.authz_services ORDER BY name, id")
            .fetch_all(&mut *conn)
            .await?;
    let mut policies = Vec::new();
    let mut everyone = Vec::new();
    let mut everyone_ids = BTreeSet::new();
    for (sid, sname, stype) in services {
        if !matches!(stype.as_str(), "trino" | "starrocks" | "tag") {
            continue;
        }
        let rows: Vec<PolicyRow> = policy_rows_via(&mut *conn, sid)
            .await?
            .into_iter()
            .filter(|r| r.managed_by == "operator")
            .collect();
        let mut engines: Option<Vec<HandWrittenEngineRef>> = None;
        for row in &rows {
            // A body that cannot be read names nobody we can see; route A
            // lists it as `unknown` on every object of its profile.
            let Ok(policy) = parse_policy(row) else {
                continue;
            };
            let mut naming = Vec::new();
            for (bucket, items) in buckets(&policy) {
                let bucket = Bucket::of(bucket);
                for item in items {
                    if item_names_everyone(item) {
                        everyone_ids.insert(row.id);
                        everyone.push(EveryonePolicy {
                            id: row.id,
                            name: row.name.clone(),
                            service: sname.clone(),
                            bucket,
                        });
                    }
                    if no_username {
                        continue;
                    }
                    if let Some(u) = &username {
                        if item.users.iter().any(|x| x == u) {
                            naming.push(Naming {
                                bucket,
                                how: NamingHow::Direct,
                                via: None,
                                on_engines: true,
                            });
                        }
                    }
                    for g in item
                        .groups
                        .iter()
                        .filter(|g| *g != PUBLIC && groups.contains(*g))
                    {
                        naming.push(Naming {
                            bucket,
                            how: NamingHow::Group,
                            via: Some(g.clone()),
                            on_engines: false,
                        });
                    }
                    for r in item.roles.iter().filter(|r| roles.contains(*r)) {
                        naming.push(Naming {
                            bucket,
                            how: NamingHow::Role,
                            via: Some(r.clone()),
                            on_engines: false,
                        });
                    }
                }
            }
            if naming.is_empty() {
                continue;
            }
            if engines.is_none() {
                engines = Some(engines_downloading_via(&mut *conn, sid, &sname).await?);
            }
            policies.push(PrincipalPolicy {
                id: row.id,
                name: row.name.clone(),
                service: sname.clone(),
                in_tag_service: stype == "tag",
                policy_type: policy_type_word(policy.policy_type).to_string(),
                resource_summary: resource_summary(&policy),
                enabled: row.enabled,
                deny_all_else: policy.is_deny_all_else,
                unmodelled: unmodelled_sentences(&policy),
                naming,
                engines: engines.clone().unwrap_or_default(),
            });
        }
    }
    Ok(PrincipalHandWritten {
        principal,
        state: if no_username {
            PrincipalState::NoUsername
        } else {
            PrincipalState::Listed
        },
        policies,
        everyone: EveryoneSummary {
            count: everyone_ids.len() as i64,
            policies: everyone,
        },
    })
}

// ── Route D: hand-written tag policies of a classification (spec §4.5, Q4) ─

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagRef {
    pub id: Uuid,
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TagPolicyState {
    Listed,
    /// No Trino profile has a usable tag service, so no engine downloads tag
    /// policies.
    NoTagService,
}

/// [`TagPolicyState`] from the number of usable tag services.
pub fn tag_policy_state(tag_services: usize) -> TagPolicyState {
    if tag_services == 0 {
        TagPolicyState::NoTagService
    } else {
        TagPolicyState::Listed
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandWrittenProfileRef {
    pub service_id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagPolicyRow {
    pub id: i64,
    pub name: String,
    /// The tag service that stores it.
    pub service: String,
    pub policy_type: String,
    pub resource_summary: String,
    /// The tag value matched by `*`/`?` rather than literally.
    pub through_pattern: bool,
    pub enabled: bool,
    pub deny_all_else: bool,
    pub unmodelled: Vec<String>,
    pub items: Vec<ItemSummary>,
    /// The Trino profiles whose tag service this is.
    pub profiles: Vec<HandWrittenProfileRef>,
    /// The engines bound to those profiles.
    pub engines: Vec<HandWrittenEngineRef>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SameNameTag {
    pub id: Uuid,
    pub key: String,
}

/// Route D's answer.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagHandWritten {
    pub tag: TagRef,
    pub state: TagPolicyState,
    pub policies: Vec<TagPolicyRow>,
    /// Other tags with this name: engine tag policies on the name apply to
    /// them too (CL10).
    pub same_name: Vec<SameNameTag>,
}

/// Route D (spec §4.5): every usable tag service's hand-written (`operator`)
/// policies whose tag resource matches this tag's NAME through the tag
/// service-def's matcher (no case folding: `ignoreCase: false`).
pub async fn handwritten_for_tag_via(
    conn: &mut sqlx::PgConnection,
    tag_id: Uuid,
) -> Result<TagHandWritten, HandWrittenError> {
    let tag: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, key, name FROM tags WHERE id = $1")
            .bind(tag_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((id, key, name)) = tag else {
        return Err(HandWrittenError::NotFound);
    };
    let same: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, key FROM tags WHERE name = $1 AND id <> $2 ORDER BY key")
            .bind(&name)
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;

    // Every Trino profile and its usable tag service.
    let trino: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM policy.authz_services WHERE type = 'trino' ORDER BY name, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<i64> = trino.iter().map(|(i, _)| *i).collect();
    let links = usable_tag_services_via(&mut *conn, &ids).await?;
    let profile_name: BTreeMap<i64, String> = trino.into_iter().collect();
    let mut by_tag_service: BTreeMap<String, Vec<HandWrittenProfileRef>> = BTreeMap::new();
    for (sid, tag_service) in links {
        by_tag_service
            .entry(tag_service)
            .or_default()
            .push(HandWrittenProfileRef {
                service_id: sid,
                name: profile_name.get(&sid).cloned().unwrap_or_default(),
            });
    }

    let mut out = TagHandWritten {
        tag: TagRef {
            id,
            key,
            name: name.clone(),
        },
        state: tag_policy_state(by_tag_service.len()),
        policies: Vec::new(),
        same_name: same
            .into_iter()
            .map(|(id, key)| SameNameTag { id, key })
            .collect(),
    };
    let request = RequestResource::new().with("tag", name.as_str());
    let mut defs = Defs::default();
    for (tag_service, mut profiles) in by_tag_service {
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        let Some(tsid) = service_id_by_name_via(&mut *conn, &tag_service).await? else {
            continue;
        };
        let mut engines = Vec::new();
        for p in &profiles {
            engines.extend(
                engines_of_profile_via(&mut *conn, p.service_id)
                    .await?
                    .into_iter()
                    .map(|e| HandWrittenEngineRef {
                        id: e.id,
                        name: e.name,
                    }),
            );
        }
        let rows = policy_rows_via(&mut *conn, tsid).await?;
        let def = defs.get(&mut *conn, "tag").await?;
        for row in rows.iter().filter(|r| r.managed_by == "operator") {
            let Ok(policy) = parse_policy(row) else {
                continue;
            };
            let prepared = PreparedPolicyResources::new(&policy, def);
            let ObjectMatch::Applies { through_pattern } =
                object_match(&prepared, &policy, &request)
            else {
                continue;
            };
            out.policies.push(TagPolicyRow {
                id: row.id,
                name: row.name.clone(),
                service: tag_service.clone(),
                policy_type: policy_type_word(policy.policy_type).to_string(),
                resource_summary: resource_summary(&policy),
                through_pattern,
                enabled: row.enabled,
                deny_all_else: policy.is_deny_all_else,
                unmodelled: unmodelled_sentences(&policy),
                items: item_summary(&policy),
                profiles: profiles.clone(),
                engines: engines.clone(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_ranger_eval::servicedef::ServiceDef;
    use serde_json::json;
    use uuid::Uuid;

    fn def(name: &str) -> ServiceDef {
        let v: serde_json::Value = match name {
            "trino" => serde_json::from_str(include_str!(
                "../../egret-policy/src/service-defs/trino.json"
            )),
            "starrocks" => serde_json::from_str(include_str!(
                "../../egret-policy/src/service-defs/starrocks.json"
            )),
            _ => serde_json::from_str(include_str!("../../egret-policy/src/service-defs/tag.json")),
        }
        .unwrap();
        ServiceDef::parse(&v)
    }

    fn policy(v: serde_json::Value) -> RangerPolicy {
        serde_json::from_value(v).expect("policy")
    }

    fn level(values: &[&str]) -> serde_json::Value {
        json!({ "values": values })
    }

    fn access(resources: serde_json::Value) -> RangerPolicy {
        policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": resources,
            "policyItems": [{ "users": ["ua"], "accesses": [{ "type": "select", "isAllowed": true }] }]
        }))
    }

    fn target(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> ObjectTarget {
        ObjectTarget {
            catalog_id: Uuid::nil(),
            schema: schema.map(Into::into),
            table: table.map(Into::into),
            column: column.map(Into::into),
        }
    }

    fn matched(p: &RangerPolicy, engine: EngineType, t: &ObjectTarget) -> ObjectMatch {
        let d = def(match engine {
            EngineType::Trino => "trino",
            EngineType::StarRocks => "starrocks",
        });
        let prepared = PreparedPolicyResources::new(p, &d);
        object_match(&prepared, p, &object_request(engine, "ice", t))
    }

    fn table_t() -> ObjectTarget {
        target(Some("s"), Some("t"), None)
    }

    #[test]
    fn object_request_names_each_level_and_stops_at_the_first_absent_one() {
        let r = object_request(EngineType::Trino, "ice", &target(None, None, None));
        assert_eq!(r, RequestResource::new().with("catalog", "ice"));
        let r = object_request(EngineType::Trino, "ice", &target(Some("s"), None, None));
        assert_eq!(
            r,
            RequestResource::new()
                .with("catalog", "ice")
                .with("schema", "s")
        );
        let r = object_request(EngineType::StarRocks, "ice", &table_t());
        assert_eq!(
            r,
            RequestResource::new()
                .with("catalog", "ice")
                .with("database", "s")
                .with("table", "t")
        );
        let r = object_request(
            EngineType::Trino,
            "ice",
            &target(Some("s"), Some("t"), Some("email")),
        );
        assert_eq!(
            r,
            RequestResource::new()
                .with("catalog", "ice")
                .with("schema", "s")
                .with("table", "t")
                .with("column", "email")
        );
        // A gap stops the request: nothing below it is named.
        let r = object_request(EngineType::Trino, "ice", &target(None, Some("t"), None));
        assert_eq!(r, RequestResource::new().with("catalog", "ice"));
    }

    #[test]
    fn an_exact_policy_applies_literally() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": level(&["t"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn a_wildcard_schema_applies_through_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["*"]),
            "table": level(&["*"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
    }

    #[test]
    fn a_question_mark_applies_through_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": level(&["t?"]), "column": level(&["*"])
        }));
        let t = target(Some("s"), Some("tx"), None);
        assert_eq!(
            matched(&p, EngineType::Trino, &t),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
    }

    #[test]
    fn an_exclude_applies_through_a_pattern_and_excludes_its_value() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": { "values": ["u"], "isExcludes": true }, "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
        assert_eq!(
            matched(&p, EngineType::Trino, &target(Some("s"), Some("u"), None)),
            ObjectMatch::None
        );
    }

    #[test]
    fn a_recursive_level_applies_through_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": { "values": ["s"], "isRecursive": true },
            "table": level(&["t"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
    }

    #[test]
    fn a_literal_differing_only_in_case_is_not_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ICE"]), "schema": level(&["S"]),
            "table": level(&["T"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn a_column_policy_is_part_of_its_table_and_applies_to_the_column() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": level(&["t"]), "column": level(&["email"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Part
        );
        assert_eq!(
            matched(
                &p,
                EngineType::Trino,
                &target(Some("s"), Some("t"), Some("email"))
            ),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn a_schema_only_policy_is_set_on_a_parent_of_a_table() {
        let p = access(json!({ "catalog": level(&["ice"]), "schema": level(&["s"]) }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Parent
        );
        // StarRocks names the level `database`.
        let p = access(json!({ "catalog": level(&["ice"]), "database": level(&["s"]) }));
        assert_eq!(
            matched(&p, EngineType::StarRocks, &table_t()),
            ObjectMatch::Parent
        );
    }

    #[test]
    fn the_best_verdict_over_every_resource_map_wins() {
        let p = policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": {
                "catalog": level(&["ice"]), "schema": level(&["other"]),
                "table": level(&["*"]), "column": level(&["*"])
            },
            "additionalResources": [{
                "catalog": level(&["ice"]), "schema": level(&["s"]),
                "table": level(&["t"]), "column": level(&["*"])
            }],
            "policyItems": []
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn item_summary_reads_every_bucket_and_marks_principals() {
        let p = policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": { "catalog": level(&["ice"]) },
            "policyItems": [{ "users": ["ua", "{USER}"], "groups": ["g1", "public"], "roles": ["r1"],
                              "accesses": [{ "type": "select", "isAllowed": true }] }],
            "denyPolicyItems": [{ "users": ["ub"], "accesses": [{ "type": "select", "isAllowed": true }] }],
            "allowExceptions": [{ "users": ["uc"], "accesses": [{ "type": "select", "isAllowed": true }] }],
            "denyExceptions": [{ "groups": ["g2"], "accesses": [{ "type": "select", "isAllowed": true }] }]
        }));
        let s = item_summary(&p);
        assert_eq!(s.len(), 4);
        assert_eq!(
            s[0],
            ItemSummary {
                bucket: Bucket::Allow,
                users: vec!["ua".into()],
                groups: vec![PrincipalMark {
                    name: "g1".into(),
                    on_engines: false
                }],
                roles: vec![PrincipalMark {
                    name: "r1".into(),
                    on_engines: false
                }],
                everyone: true,
                accesses: vec!["select".into()],
                mask_type: None,
                row_filter: None,
            }
        );
        assert_eq!(
            s.iter().map(|i| i.bucket).collect::<Vec<_>>(),
            vec![
                Bucket::Allow,
                Bucket::Deny,
                Bucket::AllowException,
                Bucket::DenyException
            ]
        );
        assert!(!s[1].everyone);
        assert_eq!(s[3].groups[0].name, "g2");
        assert!(!s[3].groups[0].on_engines);
    }

    #[test]
    fn item_summary_reads_mask_and_row_filter() {
        let mask = policy(json!({
            "name": "m", "service": "s", "policyType": 1, "isEnabled": true,
            "resources": { "catalog": level(&["ice"]) },
            "dataMaskPolicyItems": [{ "users": ["{USER}"], "accesses": [{ "type": "select" }],
                                      "dataMaskInfo": { "dataMaskType": "MASK_NULL" } }]
        }));
        let s = item_summary(&mask);
        assert_eq!(s[0].bucket, Bucket::Mask);
        assert_eq!(s[0].mask_type.as_deref(), Some("MASK_NULL"));
        assert!(s[0].everyone);
        assert!(s[0].users.is_empty());
        let rf = policy(json!({
            "name": "r", "service": "s", "policyType": 2, "isEnabled": true,
            "resources": { "catalog": level(&["ice"]) },
            "rowFilterPolicyItems": [{ "groups": ["public"], "accesses": [{ "type": "select" }],
                                       "rowFilterInfo": { "filterExpr": "country = 'VN'" } }]
        }));
        let s = item_summary(&rf);
        assert_eq!(s[0].bucket, Bucket::RowFilter);
        assert_eq!(s[0].row_filter.as_deref(), Some("country = 'VN'"));
        assert!(s[0].everyone);
        assert!(s[0].groups.is_empty());
    }

    #[test]
    fn a_tag_value_matches_by_the_tag_service_def_without_folding_case() {
        let d = def("tag");
        let p = policy(json!({
            "name": "t", "service": "x", "policyType": 0, "isEnabled": true,
            "resources": { "tag": level(&["PII"]) }, "policyItems": []
        }));
        let prepared = PreparedPolicyResources::new(&p, &d);
        let req = |n: &str| RequestResource::new().with("tag", n);
        assert_eq!(
            object_match(&prepared, &p, &req("PII")),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
        assert_eq!(object_match(&prepared, &p, &req("pii")), ObjectMatch::None);
    }

    #[test]
    fn resource_summary_and_unmodelled_sentences() {
        let p = policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": {
                "column": level(&["*"]), "table": { "values": ["u"], "isExcludes": true },
                "schema": level(&["s"]), "catalog": level(&["ice"])
            },
            "conditions": [{ "type": "ip", "values": ["1.2.3.4"] }],
            "validitySchedules": [{ "startTime": "2026/01/01 00:00:00" }],
            "policyItems": []
        }));
        assert_eq!(
            resource_summary(&p),
            "catalog=ice; schema=s; table=not u; column=*"
        );
        assert_eq!(
            unmodelled_sentences(&p),
            vec![
                "Has conditions egret does not evaluate.".to_string(),
                "Has a validity schedule egret does not evaluate.".to_string(),
            ]
        );
    }
}
