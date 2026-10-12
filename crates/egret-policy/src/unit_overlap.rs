//! **The one overlap rule, widened to the EVALUATION UNIT** — a Trino profile's
//! resource download plus its `tagService` download, with the tag assignments
//! that decide where a tag policy applies (SP4d-4 Task 3; spec Part A, "Tag vs
//! anything").
//!
//! [`crate::ambiguous_arbitration`] decides pairs inside ONE download. Two
//! shapes it cannot see, because their two halves never share a download or
//! never share a resource map:
//!
//! * **Tag ↔ path.** A tag mask in the tag service and a resource mask in the
//!   profile, on a column that carries the tag. The plugin runs the tag phase
//!   first (`egret_ranger_eval::eval`, the tag phase before the resource
//!   candidates); with a tag mask on the result, a resource mask is shut out
//!   unless its priority is STRICTLY higher (`eval.rs`, the walk modelled from
//!   offset 529, `if_icmplt`). At equal priority the tag policy wins and the
//!   path policy is dead on every tagged column; above it the two phases split
//!   the decision. Neither is something the steward wrote, so the pair is
//!   refused at ANY priority (spec o3's row).
//! * **Tag ↔ tag.** `tag=[pii]` and `tag=[gdpr]` never intersect as resource
//!   maps, but a column carrying both tags is decided by both, and on the tag
//!   path the order is match type, priority, `hasDeny`, then the policy NAME
//!   (`eval.rs`'s tag comparator). Refused when the two are in one band
//!   (`policyPriority`, `hasDeny`) and differ.
//!
//! Every within-download pair still comes from
//! [`crate::ambiguous_arbitration::ambiguous_arbitrations_in`] — this module
//! adds the two cross shapes and does not restate that rule.
//!
//! # One function for every author (user ruling, SP4d-4 Task 3)
//!
//! [`unit_overlaps`] is called with whatever policies a caller has: the rule
//! compiler with its desired (compiled) set (`egret_perm_compile::overlap`),
//! the policy editor with the stored set and the body it is writing, and a tag
//! assignment with the stored set before and after its write and recompile. Compiled and hand-written policies are not
//! told apart here — only the caller's LABEL for a name differs.
//!
//! # Where a tag policy applies, and how coordinates meet
//!
//! A tag policy applies at a coordinate when a tag its `resources.tag` values
//! cover is assigned there in `catalog_tag_assignments`:
//!
//! * a column-level assignment applies to that column; a table-level one to
//!   every column of the table and to the table;
//! * for row filters, a column-level assignment also applies to its TABLE —
//!   conservative, and how StarRocks expands a tag row filter (`rules.rs`).
//!
//! "Covers" and "meets" are decided by the SAME may-intersect comparison the
//! within-download rule uses ([`crate::ambiguous_arbitration`]'s
//! `values_may_intersect` / `resources_may_intersect`), applied to the
//! assignment's literal coordinate as a resource map — `catalog`, `schema`,
//! `table`, plus `column` for a column-level mask coordinate. That comparison
//! folds under BOTH the evaluator's and Java's case folding and assumes any
//! wildcard it does not evaluate intersects, so it over-approximates Ranger's
//! matcher — the fail-closed direction for a refusal — and needs no
//! fold-divergence decline: where egret's fold and Java's can disagree, both
//! are tried. (The plan named `egret_ranger_eval::resource::match_policy_resources`
//! plus the `fold_divergent` gate; that matcher folds with `to_lowercase` only,
//! which is narrower than Java and would have needed the decline to stay sound.
//! Using the one may-intersect rule keeps a single spelling of "may decide the
//! same request" across both modules.)
//!
//! Assignments are literal: migration 0177's `CHECK` forbids `*`/`?` and an
//! empty schema or table, so a coordinate built from one never carries a
//! wildcard of its own.
//!
//! # What it does not see
//!
//! * A profile's unit is the profile and ITS tag service. Two Trino profiles
//!   that name one tag service share that tag service's policies; a pair
//!   between one profile's path policy and the shared tag policy is in THAT
//!   profile's unit and is checked by whatever write reaches that unit. The
//!   rule compiler checks the profile it compiles: its compiled set plus the
//!   enabled hand-written masks and row filters of that profile and its tag
//!   service, refusing every pair with a compiled member (SP4d-4 Task 3 fix
//!   round 1); a hand-written × hand-written pair is the editor's and the tag
//!   assignment's to refuse.
//! * Writes that bypass the routes, and `PUT /policies/{id}/enabled` (see
//!   [`crate::ambiguous_arbitration`]'s module doc for both).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::admin::item_key;
use crate::ambiguous_arbitration::{
    ambiguous_arbitrations_in, canonical_resource_map, has_deny, int_field, is_enabled, quoted,
    resources_may_intersect, values_may_intersect, AmbiguousArbitration, Arbitrated,
    PolicyUnderWrite, EDITOR_OVERRIDE_REMEDY,
};
use crate::store::{
    catalogs_for_service_via, enabled_policies_for_service_via, get_service_by_id_via,
    service_id_by_name_via, tag_assignments_for_catalogs_via,
};
use crate::PolicyStoreError;

/// One policy of an evaluation unit, as the caller has it.
#[derive(Debug, Clone, Copy)]
pub struct UnitPolicy<'a> {
    /// The `policy.authz_services` id the body is (or will be) stored under.
    pub service: i64,
    /// Whether `service` is the unit's TAG service.
    pub is_tag_service: bool,
    pub name: &'a str,
    pub body: &'a Value,
}

/// One `catalog_tag_assignments` row as names: `column == ""` is a TABLE-level
/// assignment. `tag` is `tags.name` (what the engine's tag policies match).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AssignedTag {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub column: String,
    pub tag: String,
}

/// How the two policies of a pair come to decide one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlapShape {
    /// Both in one download and one band — Task 2's pair, verbatim.
    SameDownload(AmbiguousArbitration),
    /// A tag policy and a resource policy meeting on an assigned coordinate.
    TagAndPath {
        tag_policy: String,
        tag: String,
        tag_priority: i64,
        path_priority: i64,
    },
    /// Two tag policies in one band whose tags are assigned on one coordinate.
    TagAndTag {
        /// The tag each of `first`/`second` covers there, in that order.
        tags: (String, String),
        priority: i64,
    },
}

/// One pair of mask (or row-filter) policies in one evaluation unit that may
/// decide one request differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitOverlap {
    pub arbitrated: Arbitrated,
    /// The two policy names, lexicographically ordered.
    pub first: String,
    pub second: String,
    pub first_items: Value,
    pub second_items: Value,
    /// Where both decide, as the sentence names it: the FIRST coordinate met
    /// (`catalog.schema.table[.column]`, assignments in sorted order) for a
    /// cross pair, the resource-map hint for a same-download pair.
    pub coordinate: String,
    /// EVERY coordinate where a cross pair meets (for a same-download pair,
    /// just its hint). The identity carries this set, not `coordinate`, so
    /// whether a write extends a stored pair does not depend on how column
    /// names sort (fix round 1, review I2).
    pub coordinates: BTreeSet<String>,
    pub shape: OverlapShape,
}

/// What makes two reports the SAME pair: both names, what each decides, and
/// where — the whole SET of coordinates where they meet. An edit that keeps a
/// partner but changes either side's decisive items, or where they meet, is a
/// different pair (SP4d-4 Task 3); so is an assignment that extends a stored
/// pair to one more column, whichever way that column's name sorts (fix round
/// 1, review I2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlapIdentity {
    kind: i64,
    first: String,
    first_items: Value,
    second: String,
    second_items: Value,
    coordinates: BTreeSet<String>,
}

impl UnitOverlap {
    pub fn identity(&self) -> OverlapIdentity {
        OverlapIdentity {
            kind: self.arbitrated.policy_type(),
            first: self.first.clone(),
            first_items: self.first_items.clone(),
            second: self.second.clone(),
            second_items: self.second_items.clone(),
            coordinates: self.coordinates.clone(),
        }
    }

    /// The refusal, with every policy name rendered by `label`. The compiler
    /// labels a compiled policy by its RULE; the editor quotes the name.
    /// `editor` adds the Override remedy where an authored priority can fix
    /// the pair (never for tag ↔ path: no priority makes that pair coherent).
    ///
    /// It never says "random", "undefined" or "may change between queries"
    /// (see [`crate::ambiguous_arbitration`]'s module doc on what the
    /// comparator actually does).
    pub fn sentence(&self, label: &dyn Fn(&str) -> String, editor: bool) -> String {
        let noun = self.arbitrated.noun();
        let disagreement = self.arbitrated.disagreement();
        let coord = &self.coordinate;
        match &self.shape {
            OverlapShape::SameDownload(pair) => {
                pair.sentence_with(label, if editor { EDITOR_OVERRIDE_REMEDY } else { "" })
            }
            OverlapShape::TagAndPath {
                tag_policy,
                tag,
                tag_priority,
                path_priority,
            } => {
                let path_policy = if tag_policy == &self.first {
                    &self.second
                } else {
                    &self.first
                };
                let (tl, pl) = (label(tag_policy), label(path_policy));
                let today = if tag_priority >= path_priority {
                    format!(
                        "Ranger runs the tag phase first, and a tag {noun} shuts out every \
                         resource {noun} at the same or a lower policyPriority, so today Ranger \
                         serves {tl} there and {pl} never applies to that column."
                    )
                } else {
                    format!(
                        "{pl} is at a strictly higher policyPriority ({path_priority} > \
                         {tag_priority}), so the tag phase and the resource phase split the \
                         decision between them — neither policy says which should win."
                    )
                };
                format!(
                    "{tl} (a tag {noun} on tag {tag:?}) and {pl} (a resource {noun}) both decide \
                     {coord}, where tag {tag:?} is assigned, with {disagreement}. {today} Narrow \
                     one of the two so they no longer meet there, or delete one, or make them \
                     agree, or unassign tag {tag:?} from {coord}."
                )
            }
            OverlapShape::TagAndTag { tags, priority } => {
                let (fl, sl) = (label(&self.first), label(&self.second));
                let extra = if editor { EDITOR_OVERRIDE_REMEDY } else { "" };
                format!(
                    "{fl} (a tag {noun} on tag {:?}) and {sl} (a tag {noun} on tag {:?}) are \
                     served at policyPriority {priority} and both decide {coord}, which carries \
                     both tags, with {disagreement}. On the tag path Ranger orders two such \
                     policies by match type, priority and then the policy name — none of which \
                     says which of the two should win. Narrow one of the two so they no longer \
                     meet there, or delete one, or make them agree, or unassign one of the two \
                     tags from {coord}{extra}.",
                    tags.0, tags.1
                )
            }
        }
    }

    /// [`Self::sentence`] with names quoted.
    pub fn refusal(&self, editor: bool) -> String {
        self.sentence(&quoted, editor)
    }

    /// The `kind` of the structured 409 body: `"mask"` / `"row_filter"`.
    pub fn kind_wire_name(&self) -> &'static str {
        match self.arbitrated {
            Arbitrated::Mask => "mask",
            Arbitrated::RowFilter => "row_filter",
        }
    }
}

/// The RULE behind every compiled `perm-rule-<uuid>[-suffix]` policy name in
/// `names`: policy name → (rule id, rule name), the name read from
/// `perm.rules` (the policy name when the row is gone). Names that are not a
/// compiled rule's are absent. Shared by the rule compiler's tag-assignment
/// check and the policy editor, so a 409 names a compiled member by the rule a
/// steward edits on every surface (SP4d-4 Task 3 fix round 1, M1).
pub async fn compiled_rule_names_via(
    conn: &mut sqlx::PgConnection,
    names: &[&str],
) -> Result<BTreeMap<String, (uuid::Uuid, String)>, PolicyStoreError> {
    let parse = |n: &str| -> Option<uuid::Uuid> {
        let rest = n.strip_prefix(crate::COMPILER_RULE_PREFIX)?;
        uuid::Uuid::parse_str(rest.get(..36)?).ok()
    };
    let ids: Vec<uuid::Uuid> = names.iter().filter_map(|n| parse(n)).collect();
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows: Vec<(uuid::Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM perm.rules WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await?;
    let by_id: BTreeMap<uuid::Uuid, String> = rows.into_iter().collect();
    Ok(names
        .iter()
        .filter_map(|n| {
            let id = parse(n)?;
            let name = by_id.get(&id).cloned().unwrap_or_else(|| n.to_string());
            Some((n.to_string(), (id, name)))
        })
        .collect())
}

/// One mask/row-filter policy, read once.
struct Member<'a> {
    name: &'a str,
    body: &'a Value,
    kind: Arbitrated,
    is_tag: bool,
    priority: i64,
    deny: bool,
    items: Value,
}

/// Whether a tag policy covers tag `tag`: some resource map's `tag` level is
/// absent (match-any), excluding (assumed), or holds a value that may match
/// `tag` under the within-download rule's own value comparison.
fn covers(body: &Value, tag: &str) -> bool {
    let mut maps = vec![body.get("resources")];
    if let Some(extra) = body.get("additionalResources").and_then(Value::as_array) {
        maps.extend(extra.iter().map(Some));
    }
    maps.into_iter().any(|m| {
        let m = canonical_resource_map(m);
        match m.get("tag") {
            None => true,
            Some((values, excludes, _)) => {
                *excludes || values.iter().any(|v| values_may_intersect(v, tag))
            }
        }
    })
}

/// A path policy's resource maps (`resources` + each `additionalResources`).
fn resource_maps(body: &Value) -> Vec<&Value> {
    let mut out = vec![body.get("resources").unwrap_or(&Value::Null)];
    if let Some(extra) = body.get("additionalResources").and_then(Value::as_array) {
        out.extend(extra);
    }
    out
}

/// The coordinate one assignment decides for `kind`, as a resource map, and
/// its rendering. A mask coordinate is the column (a table-level assignment:
/// every column, i.e. no `column` level); a row-filter coordinate is the table.
fn coordinate(a: &AssignedTag, kind: Arbitrated) -> (Value, String) {
    let lvl = |v: &str| json!({ "values": [v] });
    let mut map = json!({
        "catalog": lvl(&a.catalog),
        "schema": lvl(&a.schema),
        "table": lvl(&a.table),
    });
    let table = format!("{}.{}.{}", a.catalog, a.schema, a.table);
    match kind {
        Arbitrated::Mask if !a.column.is_empty() => {
            map["column"] = lvl(&a.column);
            (map, format!("{table}.{}", a.column))
        }
        Arbitrated::Mask => (map, format!("{table}.* (the tag is on the table)")),
        Arbitrated::RowFilter => (map, table),
    }
}

/// **The whole rule for one evaluation unit.** Every pair of mask (or
/// row-filter) policies in `policies` that may decide one request differently:
/// Task 2's within-download pairs, then tag ↔ path and tag ↔ tag pairs through
/// `assignments`. One report per `(kind, first, second)`, in a deterministic
/// order (kind, then names). Pure.
pub fn unit_overlaps(policies: &[UnitPolicy<'_>], assignments: &[AssignedTag]) -> Vec<UnitOverlap> {
    let mut out: BTreeMap<(i64, String, String), UnitOverlap> = BTreeMap::new();

    for pair in ambiguous_arbitrations_in(policies.iter().map(|p| PolicyUnderWrite {
        service: p.service,
        name: p.name,
        body: p.body,
    })) {
        let key = (
            pair.arbitrated.policy_type(),
            pair.first.clone(),
            pair.second.clone(),
        );
        out.entry(key).or_insert_with(|| UnitOverlap {
            arbitrated: pair.arbitrated,
            first: pair.first.clone(),
            second: pair.second.clone(),
            first_items: pair.first_items.clone(),
            second_items: pair.second_items.clone(),
            coordinate: pair.coordinate_hint.clone(),
            coordinates: BTreeSet::from([pair.coordinate_hint.clone()]),
            shape: OverlapShape::SameDownload(pair),
        });
    }

    let members: Vec<Member<'_>> = policies
        .iter()
        .filter(|p| is_enabled(p.body))
        .filter_map(|p| {
            let kind = Arbitrated::from_policy_type(int_field(p.body, "policyType"))?;
            Some(Member {
                name: p.name,
                body: p.body,
                kind,
                is_tag: p.is_tag_service,
                priority: int_field(p.body, "policyPriority"),
                deny: has_deny(p.body),
                items: p
                    .body
                    .get(item_key(kind.policy_type()))
                    .cloned()
                    .unwrap_or(Value::Null),
            })
        })
        .collect();

    let mut sorted: Vec<&AssignedTag> = assignments.iter().collect();
    sorted.sort();
    sorted.dedup();

    for kind in [Arbitrated::Mask, Arbitrated::RowFilter] {
        let tags: Vec<&Member<'_>> = members
            .iter()
            .filter(|m| m.kind == kind && m.is_tag)
            .collect();
        if tags.is_empty() {
            continue;
        }
        let paths: Vec<&Member<'_>> = members
            .iter()
            .filter(|m| m.kind == kind && !m.is_tag)
            .collect();
        // Only assignments some tag policy of this kind covers can matter.
        let relevant: Vec<(&AssignedTag, Vec<&Member<'_>>, Value, String)> = sorted
            .iter()
            .filter_map(|a| {
                let cov: Vec<&Member<'_>> = tags
                    .iter()
                    .copied()
                    .filter(|t| covers(t.body, &a.tag))
                    .collect();
                if cov.is_empty() {
                    return None;
                }
                let (map, rendered) = coordinate(a, kind);
                Some((*a, cov, map, rendered))
            })
            .collect();

        let mut push = |a: &Member<'_>, b: &Member<'_>, coord: &str, shape: OverlapShape| {
            if a.name == b.name || a.items == b.items {
                return;
            }
            let (f, s) = if a.name <= b.name { (a, b) } else { (b, a) };
            let shape = match shape {
                OverlapShape::TagAndTag { tags, priority } if f.name != a.name => {
                    OverlapShape::TagAndTag {
                        tags: (tags.1, tags.0),
                        priority,
                    }
                }
                other => other,
            };
            let report = out
                .entry((kind.policy_type(), f.name.to_string(), s.name.to_string()))
                .or_insert_with(|| UnitOverlap {
                    arbitrated: kind,
                    first: f.name.to_string(),
                    second: s.name.to_string(),
                    first_items: f.items.clone(),
                    second_items: s.items.clone(),
                    coordinate: coord.to_string(),
                    coordinates: BTreeSet::new(),
                    shape,
                });
            // Every coordinate a cross pair meets on, not only the first
            // (review I2). A same-download pair already decides its whole
            // intersection whatever is assigned, so its set stays its hint.
            if !matches!(report.shape, OverlapShape::SameDownload(_)) {
                report.coordinates.insert(coord.to_string());
            }
        };

        // Tag ↔ path, at any priority.
        for (a, cov, map, rendered) in &relevant {
            for t in cov {
                for p in &paths {
                    if resource_maps(p.body)
                        .into_iter()
                        .any(|m| resources_may_intersect(m, map))
                    {
                        push(
                            t,
                            p,
                            rendered,
                            OverlapShape::TagAndPath {
                                tag_policy: t.name.to_string(),
                                tag: a.tag.clone(),
                                tag_priority: t.priority,
                                path_priority: p.priority,
                            },
                        );
                    }
                }
            }
        }

        // Tag ↔ tag, in one band, where two DIFFERENT tags meet.
        for (i, (a1, cov1, map1, r1)) in relevant.iter().enumerate() {
            for (a2, cov2, map2, r2) in &relevant[i..] {
                if a1.tag == a2.tag || !resources_may_intersect(map1, map2) {
                    continue;
                }
                // Name the narrower coordinate: a column over a whole table.
                let coord = if a1.column.is_empty() { r2 } else { r1 };
                for t1 in cov1 {
                    for t2 in cov2 {
                        if t1.priority != t2.priority || t1.deny != t2.deny {
                            continue;
                        }
                        push(
                            t1,
                            t2,
                            coord,
                            OverlapShape::TagAndTag {
                                tags: (a1.tag.clone(), a2.tag.clone()),
                                priority: t1.priority,
                            },
                        );
                    }
                }
            }
        }
    }

    out.into_values().collect()
}

/// The pairs in `after` whose identity is not in `before` — what a write
/// CREATES. A write that removes or narrows away a pair adds nothing here.
pub fn created_overlaps(before: &[UnitOverlap], after: Vec<UnitOverlap>) -> Vec<UnitOverlap> {
    let existed: Vec<OverlapIdentity> = before.iter().map(UnitOverlap::identity).collect();
    after
        .into_iter()
        .filter(|p| !existed.contains(&p.identity()))
        .collect()
}

// ── loading a unit from the database ─────────────────────────────────────────

/// One evaluation unit by id: a resource profile and/or its tag service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnitIds {
    pub resource: Option<i64>,
    pub tag_service: Option<i64>,
}

/// One unit's stored state: every ENABLED policy of both services (as served,
/// through [`enabled_policies_for_service_via`]) and, when the unit has a tag
/// service, the tag assignments on the resource profile's catalogs.
#[derive(Debug, Clone, Default)]
pub struct StoredUnit {
    pub ids: Option<UnitIds>,
    /// `(service, is_tag_service, body)`; the body carries `name` and the
    /// injected DB `id`.
    pub policies: Vec<(i64, bool, Value)>,
    pub assignments: Vec<AssignedTag>,
}

impl StoredUnit {
    /// The stored policies as [`UnitPolicy`]s.
    pub fn unit_policies(&self) -> Vec<UnitPolicy<'_>> {
        self.policies
            .iter()
            .map(|(service, is_tag, body)| UnitPolicy {
                service: *service,
                is_tag_service: *is_tag,
                name: body.get("name").and_then(Value::as_str).unwrap_or_default(),
                body,
            })
            .collect()
    }

    /// Every pair in the unit as stored.
    pub fn overlaps(&self) -> Vec<UnitOverlap> {
        unit_overlaps(&self.unit_policies(), &self.assignments)
    }
}

/// Every unit a write to `service_id` lands in. A Trino profile: itself plus
/// the tag service its `tagService` names (when that resolves). A tag service:
/// one unit per Trino profile naming it (or the tag service alone when none
/// does). Anything else: itself alone. Ordered by ids.
pub async fn units_for_service_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<UnitIds>, PolicyStoreError> {
    let svc = get_service_by_id_via(conn, service_id).await?;
    if svc.r#type == "tag" {
        let profiles: Vec<i64> = sqlx::query_scalar(
            r#"SELECT id FROM policy.authz_services
                WHERE config_jsonb ->> 'tagService' = $1 ORDER BY id"#,
        )
        .bind(&svc.name)
        .fetch_all(&mut *conn)
        .await?;
        if profiles.is_empty() {
            return Ok(vec![UnitIds {
                resource: None,
                tag_service: Some(service_id),
            }]);
        }
        return Ok(profiles
            .into_iter()
            .map(|p| UnitIds {
                resource: Some(p),
                tag_service: Some(service_id),
            })
            .collect());
    }
    let tag_service = match svc
        .config_jsonb
        .get(egret_policy_api::TAG_SERVICE_CONFIG_KEY)
        .and_then(Value::as_str)
    {
        Some(name) => service_id_by_name_via(conn, name).await?,
        None => None,
    };
    Ok(vec![UnitIds {
        resource: Some(service_id),
        tag_service,
    }])
}

/// The tag assignments a unit's tag policies are resolved through: those on
/// the resource profile's catalogs. Empty when the unit has no tag service
/// (nothing there resolves a tag) or no resource profile.
pub async fn unit_assignments_via(
    conn: &mut sqlx::PgConnection,
    ids: UnitIds,
) -> Result<Vec<AssignedTag>, PolicyStoreError> {
    let (Some(resource), Some(_)) = (ids.resource, ids.tag_service) else {
        return Ok(Vec::new());
    };
    let catalogs: Vec<uuid::Uuid> = catalogs_for_service_via(conn, resource)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    Ok(tag_assignments_for_catalogs_via(conn, &catalogs)
        .await?
        .into_iter()
        .map(|r| AssignedTag {
            catalog: r.catalog,
            schema: r.schema_name,
            table: r.table_name,
            column: r.column_name,
            tag: r.tag_name,
        })
        .collect())
}

/// Load one unit's stored state on `conn` (a caller's own transaction sees its
/// own uncommitted writes).
pub async fn load_unit_via(
    conn: &mut sqlx::PgConnection,
    ids: UnitIds,
) -> Result<StoredUnit, PolicyStoreError> {
    let mut policies = Vec::new();
    for (sid, is_tag) in [(ids.resource, false), (ids.tag_service, true)] {
        let Some(sid) = sid else { continue };
        for body in enabled_policies_for_service_via(conn, sid).await? {
            policies.push((sid, is_tag, body));
        }
    }
    let assignments = unit_assignments_via(conn, ids).await?;
    Ok(StoredUnit {
        ids: Some(ids),
        policies,
        assignments,
    })
}

/// Every distinct unit touching any of `service_ids`, ordered.
pub async fn units_for_services_via(
    conn: &mut sqlx::PgConnection,
    service_ids: &[i64],
) -> Result<Vec<UnitIds>, PolicyStoreError> {
    let mut out = BTreeSet::new();
    for &sid in service_ids {
        out.extend(units_for_service_via(conn, sid).await?);
    }
    Ok(out.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag_mask(name: &str, tag: &str, mask_type: &str, priority: i64) -> (String, Value) {
        (
            name.to_string(),
            json!({
                "name": name, "policyType": 1, "isEnabled": true, "policyPriority": priority,
                "resources": {"tag": {"values": [tag], "isExcludes": false, "isRecursive": false}},
                "dataMaskPolicyItems": [{
                    "users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": mask_type},
                }],
            }),
        )
    }

    fn path_mask(name: &str, table: &str, column: &str, mask_type: &str) -> (String, Value) {
        (
            name.to_string(),
            json!({
                "name": name, "policyType": 1, "isEnabled": true, "policyPriority": 0,
                "resources": {
                    "catalog": {"values": ["wh"]}, "schema": {"values": ["sales"]},
                    "table": {"values": [table]}, "column": {"values": [column]},
                },
                "dataMaskPolicyItems": [{
                    "users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": mask_type},
                }],
            }),
        )
    }

    fn tag_filter(name: &str, tag: &str, expr: &str) -> (String, Value) {
        (
            name.to_string(),
            json!({
                "name": name, "policyType": 2, "isEnabled": true, "policyPriority": 0,
                "resources": {"tag": {"values": [tag]}},
                "rowFilterPolicyItems": [{
                    "users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "rowFilterInfo": {"filterExpr": expr},
                }],
            }),
        )
    }

    fn assigned(table: &str, column: &str, tag: &str) -> AssignedTag {
        AssignedTag {
            catalog: "wh".into(),
            schema: "sales".into(),
            table: table.into(),
            column: column.into(),
            tag: tag.into(),
        }
    }

    fn run(
        tags: &[(String, Value)],
        paths: &[(String, Value)],
        a: &[AssignedTag],
    ) -> Vec<UnitOverlap> {
        let mut ps: Vec<UnitPolicy<'_>> = tags
            .iter()
            .map(|(n, b)| UnitPolicy {
                service: 2,
                is_tag_service: true,
                name: n,
                body: b,
            })
            .collect();
        ps.extend(paths.iter().map(|(n, b)| UnitPolicy {
            service: 1,
            is_tag_service: false,
            name: n,
            body: b,
        }));
        unit_overlaps(&ps, a)
    }

    #[test]
    fn two_tag_masks_on_one_column_are_a_pair_only_once_both_tags_are_there() {
        let tags = [
            tag_mask("t-pii", "pii", "MASK", 0),
            tag_mask("t-gdpr", "gdpr", "MASK_NULL", 0),
        ];
        assert!(run(&tags, &[], &[assigned("orders", "email", "pii")]).is_empty());
        assert!(run(
            &tags,
            &[],
            &[
                assigned("orders", "email", "pii"),
                assigned("orders", "phone", "gdpr")
            ]
        )
        .is_empty());
        let got = run(
            &tags,
            &[],
            &[
                assigned("orders", "email", "pii"),
                assigned("orders", "email", "gdpr"),
            ],
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].coordinate, "wh.sales.orders.email");
        assert!(matches!(got[0].shape, OverlapShape::TagAndTag { .. }));
        let s = got[0].refusal(false);
        assert!(s.contains("\"t-gdpr\"") && s.contains("\"t-pii\""), "{s}");
        assert!(s.contains("unassign one of the two tags"), "{s}");
        for banned in ["random", "undefined", "may change between queries"] {
            assert!(!s.contains(banned), "{banned}: {s}");
        }
    }

    #[test]
    fn a_table_level_tag_meets_a_column_level_tag_on_that_table() {
        let tags = [
            tag_mask("t-pii", "pii", "MASK", 0),
            tag_mask("t-gdpr", "gdpr", "MASK_NULL", 0),
        ];
        let got = run(
            &tags,
            &[],
            &[
                assigned("orders", "", "pii"),
                assigned("ORDERS", "email", "gdpr"),
            ],
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].coordinate, "wh.sales.ORDERS.email");
    }

    #[test]
    fn tag_masks_in_different_bands_or_agreeing_are_not_a_tag_pair() {
        let a = [
            assigned("orders", "email", "pii"),
            assigned("orders", "email", "gdpr"),
        ];
        let banded = [
            tag_mask("t-pii", "pii", "MASK", 1),
            tag_mask("t-gdpr", "gdpr", "MASK_NULL", 0),
        ];
        assert!(run(&banded, &[], &a).is_empty());
        let agree = [
            tag_mask("t-pii", "pii", "MASK", 0),
            tag_mask("t-gdpr", "gdpr", "MASK", 0),
        ];
        assert!(run(&agree, &[], &a).is_empty());
    }

    #[test]
    fn a_tag_mask_and_a_path_mask_on_a_tagged_column_pair_at_any_priority() {
        let tags = [tag_mask("t-pii", "pii", "MASK", 1)];
        let paths = [path_mask("p-email", "orders", "email", "MASK_NULL")];
        assert!(run(&tags, &paths, &[assigned("orders", "phone", "pii")]).is_empty());
        let got = run(&tags, &paths, &[assigned("orders", "email", "pii")]);
        assert_eq!(got.len(), 1, "{got:?}");
        let s = got[0].refusal(true);
        assert!(s.contains("today Ranger serves \"t-pii\""), "{s}");
        assert!(
            !s.contains("Override"),
            "no priority makes a tag/path pair coherent: {s}"
        );
        // A wildcard path policy meets a table-level tag.
        let wide = [path_mask("p-all", "*", "email", "MASK_NULL")];
        assert_eq!(run(&tags, &wide, &[assigned("orders", "", "pii")]).len(), 1);
        // A path policy at a strictly higher priority: no winner claimed.
        let (n, mut b) = path_mask("p-email", "orders", "email", "MASK_NULL");
        b["policyPriority"] = json!(2);
        let got = run(&tags, &[(n, b)], &[assigned("orders", "email", "pii")]);
        assert!(
            got[0].refusal(false).contains("split the decision"),
            "{got:?}"
        );
    }

    #[test]
    fn a_column_tag_puts_its_table_under_a_tag_row_filter() {
        let tags = [tag_filter("t-pii", "pii", "id > 2")];
        let (n, mut b) = path_mask("p-orders", "orders", "x", "MASK");
        b["policyType"] = json!(2);
        b["resources"].as_object_mut().unwrap().remove("column");
        b["rowFilterPolicyItems"] =
            json!([{"groups": ["public"], "rowFilterInfo": {"filterExpr": "country = 'US'"}}]);
        let got = run(&tags, &[(n, b)], &[assigned("orders", "email", "pii")]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].coordinate, "wh.sales.orders");
    }

    #[test]
    fn extending_a_stored_pair_to_another_column_is_created_whichever_way_it_sorts() {
        // A stored tag ↔ path pair on `phone` (the path mask covers every
        // column of `orders`); an assignment extends it to `email`, which sorts
        // BEFORE `phone`, or to `zip`, which sorts AFTER it. Both extend the
        // pair, so both are "created" — the answer must not depend on the
        // order (review I2: with the first coordinate as identity, `email` was
        // refused and `zip` admitted).
        let tags = [tag_mask("t-pii", "pii", "MASK", 0)];
        let paths = [path_mask("p-orders", "orders", "*", "MASK_NULL")];
        let stored = [assigned("orders", "phone", "pii")];
        let before = run(&tags, &paths, &stored);
        assert_eq!(before.len(), 1, "{before:?}");
        for column in ["email", "zip"] {
            let extended = [
                assigned("orders", "phone", "pii"),
                assigned("orders", column, "pii"),
            ];
            let after = run(&tags, &paths, &extended);
            assert_eq!(
                created_overlaps(&before, after).len(),
                1,
                "extending the pair to {column} is a new pair"
            );
        }
        // And the same assignments again create nothing.
        assert!(created_overlaps(&before, run(&tags, &paths, &stored)).is_empty());
    }

    #[test]
    fn the_identity_carries_items_and_coordinate() {
        let a = [assigned("orders", "email", "pii")];
        let paths = [path_mask("p-email", "orders", "email", "MASK_NULL")];
        let before = run(&[tag_mask("t-pii", "pii", "MASK", 0)], &paths, &a);
        let same = run(&[tag_mask("t-pii", "pii", "MASK", 0)], &paths, &a);
        assert!(created_overlaps(&before, same).is_empty());
        let changed = run(&[tag_mask("t-pii", "pii", "MASK_HASH", 0)], &paths, &a);
        assert_eq!(created_overlaps(&before, changed).len(), 1);
        assert!(created_overlaps(&before, Vec::new()).is_empty());
    }
}
