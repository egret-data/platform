//! Trino's Iceberg metadata tables (`t$files`, `t$partitions`, `t$entries`,
//! `t$manifests`, `t$snapshots`, …) under a mask, a row filter or a deny.
//!
//! **The leak.** Trino checks a read of `orders$files` as a read of a table
//! NAMED `orders$files`: the Ranger plugin builds its resource from the name
//! as written (`RangerSystemAccessControl.checkCanSelectFromColumns`), and
//! `getRowFilters`/`getColumnMask` ask about that same name. A grant on
//! `table: *` matches it, while a mask, row filter or deny on `table: orders`
//! does not. So a user whose `email` is masked reads the column's lower and
//! upper bounds from `orders$files`; a user filtered to `region = 'us'` reads
//! the partition values, bounds and record counts of the rows filtered away;
//! a user denied `orders` reads them too when another policy allows `*`.
//! Measured on Trino 483 (Ranger plugin 2.8.0); the plugin code is the same
//! in 470.
//!
//! **The fix.** One deny per restricting policy, on the same catalog and
//! schema, on `table: <each table value>$*` and `column: *`, for the same
//! people the source restricts, excepting the people the source exempts.
//! Users the source does not restrict keep their metadata tables. All of a
//! table's metadata tables are denied, not only the ones that carry values
//! today: Trino adds metadata tables across versions, and the ones without
//! values (`$history`, `$refs`, `$properties`) are of little use to someone
//! who cannot read the table's values anyway.
//!
//! **Who is restricted.** Ranger evaluates a mask's or a row filter's items in
//! order and the first match wins. The exceptions are the subjects of the
//! exempt items (`MASK_NONE`, an empty filter) that come BEFORE the first
//! restricting item: a person matching one of them is exempt whatever else
//! they match. Everyone named by a restricting item is denied. A person whose
//! first match is an exempt item AFTER a restricting one is denied although
//! the source shows them clear values: an over-refusal of metadata tables,
//! never a leak. A deny's people are its select-bearing deny items, its
//! exceptions its select-bearing deny exceptions; a deny-all-else policy
//! restricts `public` except its select-bearing allow items.
//!
//! **Tag policies.** A Trino tag mask and a Trino clearance (a tag deny) are
//! filed under the profile's tag service and match a column by its tag, so a
//! metadata table, whose columns carry no tag, escapes them too. They get the
//! same deny on every table carrying the tag that this profile reaches.
//!
//! **StarRocks** gets nothing from here: StarRocks 4.1 never asks Ranger about
//! a metadata table (`ColumnPrivilege.check` skips `MetadataTable`), so no
//! policy can refuse one. That exposure is a known issue, not a compiler
//! concern.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use egret_perm::CompileError;
use egret_policy::admin::{
    access_to_ranger_json, AccessItemInput, AccessPolicyInput, ResourceLevel,
};
use egret_policy::COMPILER_HIDDEN_PREFIX;

use crate::rules::PUBLIC_GROUP;

/// What follows [`COMPILER_HIDDEN_PREFIX`] in a metadata-table deny's name,
/// before the source policy's name: `perm-hidden-meta-{source}`.
pub(crate) const METADATA_DENY_INFIX: &str = "meta-";

/// The Trino access a metadata table is read with. Metadata tables are
/// read-only, so a write access would be decoration.
const SELECT: &str = "select";

/// The people a policy restricts and the people it exempts from that.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Subjects {
    users: BTreeSet<String>,
    groups: BTreeSet<String>,
    roles: BTreeSet<String>,
}

impl Subjects {
    fn add_item(&mut self, item: &Value) {
        for (key, set) in [
            ("users", &mut self.users),
            ("groups", &mut self.groups),
            ("roles", &mut self.roles),
        ] {
            set.extend(strings(item.get(key)));
        }
    }

    fn is_empty(&self) -> bool {
        self.users.is_empty() && self.groups.is_empty() && self.roles.is_empty()
    }

    fn item(&self) -> AccessItemInput {
        AccessItemInput {
            users: self.users.iter().cloned().collect(),
            groups: self.groups.iter().cloned().collect(),
            roles: self.roles.iter().cloned().collect(),
            accesses: vec![SELECT.to_string()],
            effect: "deny".to_string(),
        }
    }
}

/// Who a policy restricts on the tables it names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Restriction {
    restricted: Subjects,
    exempt: Subjects,
}

fn strings(v: Option<&Value>) -> impl Iterator<Item = String> + '_ {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
}

fn grants_select(item: &Value) -> bool {
    item.get("accesses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|a| {
            a.get("type").and_then(Value::as_str) == Some(SELECT)
                && a.get("isAllowed").and_then(Value::as_bool).unwrap_or(true)
        })
}

/// The restriction an enabled policy body places on the rows or columns it
/// names, or `None` when it restricts nobody's reads.
fn restriction(body: &Value) -> Option<Restriction> {
    if body.get("isEnabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let items = |key: &str| -> Vec<Value> {
        body.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut out = Restriction {
        restricted: Subjects::default(),
        exempt: Subjects::default(),
    };
    match body.get("policyType").and_then(Value::as_i64).unwrap_or(0) {
        // A mask or a row filter: first match wins, see the module doc.
        kind @ (1 | 2) => {
            let (key, is_exempt): (&str, fn(&Value) -> bool) = if kind == 1 {
                ("dataMaskPolicyItems", |i| {
                    i.pointer("/dataMaskInfo/dataMaskType")
                        .and_then(Value::as_str)
                        == Some("MASK_NONE")
                })
            } else {
                ("rowFilterPolicyItems", |i| {
                    i.pointer("/rowFilterInfo/filterExpr")
                        .and_then(Value::as_str)
                        .is_none_or(|f| f.trim().is_empty())
                })
            };
            let mut seen_restricting = false;
            for item in items(key) {
                if is_exempt(&item) {
                    if !seen_restricting {
                        out.exempt.add_item(&item);
                    }
                } else {
                    seen_restricting = true;
                    out.restricted.add_item(&item);
                }
            }
        }
        // An access policy: its select denies, or its deny-all-else.
        0 => {
            if body.get("isDenyAllElse").and_then(Value::as_bool) == Some(true) {
                out.restricted.groups.insert(PUBLIC_GROUP.to_string());
                for item in items("policyItems").iter().filter(|i| grants_select(i)) {
                    out.exempt.add_item(item);
                }
            } else {
                for item in items("denyPolicyItems").iter().filter(|i| grants_select(i)) {
                    out.restricted.add_item(item);
                }
                for item in items("denyExceptions").iter().filter(|i| grants_select(i)) {
                    out.exempt.add_item(item);
                }
            }
        }
        _ => return None,
    }
    (!out.restricted.is_empty()).then_some(out)
}

/// A resource level of `body`, or `None` when the policy has no such level.
fn resource_level(body: &Value, level: &str) -> Option<ResourceLevel> {
    serde_json::from_value(body.pointer(&format!("/resources/{level}"))?.clone()).ok()
}

/// The table level of a deny that reaches the metadata tables of every table
/// `table` names: each value with `$*` appended. An excluding level cannot be
/// turned around (`!orders$*` matches every ordinary table too), so it becomes
/// the metadata tables of every table: an over-refusal, never a leak.
fn metadata_table_level(table: &ResourceLevel) -> ResourceLevel {
    let values = if table.is_excludes || table.values.is_empty() {
        vec!["*$*".to_string()]
    } else {
        table.values.iter().map(|v| format!("{v}$*")).collect()
    };
    ResourceLevel {
        values,
        is_excludes: false,
        is_recursive: false,
    }
}

/// The deny body named `name` on `resources`, for `restriction`.
fn deny_body(
    name: String,
    service: &str,
    resources: BTreeMap<String, ResourceLevel>,
    restriction: &Restriction,
) -> Value {
    let input = AccessPolicyInput {
        name,
        service: service.to_string(),
        is_enabled: true,
        resources,
        items: vec![restriction.restricted.item()],
        allow_exceptions: Vec::new(),
        deny_exceptions: if restriction.exempt.is_empty() {
            Vec::new()
        } else {
            vec![restriction.exempt.item()]
        },
        // MUST stay false: `access_to_ranger_json` blanks the deny items and
        // exceptions of a deny-all-else policy.
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        is_override: false,
        additional: Vec::new(),
    };
    // `access_to_ranger_json` stamps `serviceType: "trino"`, which is right:
    // only Trino profiles get these.
    access_to_ranger_json(&input)
}

fn column_any() -> ResourceLevel {
    ResourceLevel {
        values: vec!["*".to_string()],
        is_excludes: false,
        is_recursive: false,
    }
}

/// One deny per restricting resource policy in `sources` (a Trino profile's
/// masks, row filters and access policies, compiled and hand-written), named
/// `perm-hidden-meta-{source name}`, on the metadata tables of the tables the
/// source names. A source without a table level (a catalog or schema policy,
/// a `trinouser` or function policy) is skipped: it addresses no table.
pub(crate) fn resource_denies(sources: &[Value], service: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for body in sources {
        let Some(name) = body.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(table) = resource_level(body, "table") else {
            continue;
        };
        let Some(restriction) = restriction(body) else {
            continue;
        };
        let mut resources = BTreeMap::new();
        for level in ["catalog", "schema"] {
            if let Some(l) = resource_level(body, level) {
                resources.insert(level.to_string(), l);
            }
        }
        resources.insert("table".to_string(), metadata_table_level(&table));
        resources.insert("column".to_string(), column_any());
        let deny = format!("{COMPILER_HIDDEN_PREFIX}{METADATA_DENY_INFIX}{name}");
        out.push((
            deny.clone(),
            deny_body(deny, service, resources, &restriction),
        ));
    }
    out
}

/// The tag names a tag policy body matches.
pub(crate) fn tag_values(body: &Value) -> Vec<String> {
    strings(body.pointer("/resources/tag/values")).collect()
}

/// One deny per (restricting tag policy, table carrying one of its tags), on
/// that table's metadata tables, named
/// `perm-hidden-meta-{source name}-{catalog}.{schema}.{table}`. `tagged` maps
/// each tag name to the `[catalog, schema, table]` this profile reaches that
/// carry it on a column or as a whole.
pub(crate) fn tag_denies(
    sources: &[Value],
    tagged: &BTreeMap<String, BTreeSet<[String; 3]>>,
    service: &str,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for body in sources {
        let Some(name) = body.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(restriction) = restriction(body) else {
            continue;
        };
        let tables: BTreeSet<&[String; 3]> = tag_values(body)
            .iter()
            .filter_map(|tag| tagged.get(tag))
            .flatten()
            .collect();
        for [catalog, schema, table] in tables {
            let literal = |v: String| ResourceLevel {
                values: vec![v],
                is_excludes: false,
                is_recursive: false,
            };
            let resources = BTreeMap::from([
                ("catalog".to_string(), literal(catalog.clone())),
                ("schema".to_string(), literal(schema.clone())),
                ("table".to_string(), literal(format!("{table}$*"))),
                ("column".to_string(), column_any()),
            ]);
            let deny = format!(
                "{COMPILER_HIDDEN_PREFIX}{METADATA_DENY_INFIX}{name}-{catalog}.{schema}.{table}"
            );
            out.push((
                deny.clone(),
                deny_body(deny, service, resources, &restriction),
            ));
        }
    }
    out
}

/// The `[catalog, schema, table]` that carry each of `tags` (by name, or by
/// key: either may be the Ranger tag resource) on a column or as a whole,
/// among the catalogs attached to an engine bound to `service_id`.
pub(crate) async fn tagged_tables_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    tags: &[String],
) -> Result<BTreeMap<String, BTreeSet<[String; 3]>>, CompileError> {
    let mut out: BTreeMap<String, BTreeSet<[String; 3]>> = BTreeMap::new();
    if tags.is_empty() {
        return Ok(out);
    }
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        r#"SELECT DISTINCT t.name, t.key, c.name, a.schema_name, a.table_name
           FROM catalog_tag_assignments a
           JOIN tags t ON t.id = a.tag_id
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE e.authz_service_id = $1 AND (t.name = ANY($2) OR t.key = ANY($2))"#,
    )
    .bind(service_id)
    .bind(tags)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    for (name, key, catalog, schema, table) in rows {
        for tag in [name, key] {
            if tags.contains(&tag) {
                out.entry(tag).or_default().insert([
                    catalog.clone(),
                    schema.clone(),
                    table.clone(),
                ]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn allow(accesses: &[&str]) -> Value {
        Value::Array(
            accesses
                .iter()
                .map(|a| json!({"type": a, "isAllowed": true}))
                .collect(),
        )
    }

    fn path(table: Value, column: Option<&str>) -> Value {
        let mut r = json!({
            "catalog": {"values": ["demo"], "isExcludes": false, "isRecursive": false},
            "schema": {"values": ["sales"], "isExcludes": false, "isRecursive": false},
            "table": table,
        });
        if let Some(c) = column {
            r["column"] = json!({"values": [c], "isExcludes": false, "isRecursive": false});
        }
        r
    }

    /// The shape `rules::compile_rule` emits for a path mask: the exempt users
    /// with `MASK_NONE` first, then `public` masked.
    fn mask(table: &str, exempt: &[&str]) -> Value {
        json!({
            "name": "perm-rule-m",
            "policyType": 1,
            "isEnabled": true,
            "resources": path(json!({"values": [table]}), Some("email")),
            "dataMaskPolicyItems": [
                {"users": exempt, "groups": [], "roles": [], "accesses": allow(&["select"]),
                 "dataMaskInfo": {"dataMaskType": "MASK_NONE"}},
                {"users": [], "groups": ["public"], "roles": [], "accesses": allow(&["select"]),
                 "dataMaskInfo": {"dataMaskType": "MASK_NULL"}}
            ]
        })
    }

    fn row_filter(table: &str, exempt: &[&str]) -> Value {
        json!({
            "name": "perm-rule-f",
            "policyType": 2,
            "isEnabled": true,
            "resources": path(json!({"values": [table]}), None),
            "rowFilterPolicyItems": [
                {"users": exempt, "groups": [], "roles": [], "accesses": allow(&["select"]),
                 "rowFilterInfo": {"filterExpr": ""}},
                {"users": [], "groups": ["public"], "roles": [], "accesses": allow(&["select"]),
                 "rowFilterInfo": {"filterExpr": "region = 'us'"}}
            ]
        })
    }

    fn only(got: &[(String, Value)]) -> &Value {
        assert_eq!(got.len(), 1, "{got:#?}");
        &got[0].1
    }

    /// The live leak: carol (masked on `orders.email`) read the column's
    /// bounds from `orders$files`. The deny names `orders$*`, every column,
    /// `public` denied, the mask's exempt users excepted.
    #[test]
    fn a_masked_table_denies_its_metadata_tables_to_the_masked() {
        let got = resource_denies(&[mask("orders", &["admin", "alice"])], "tr");
        assert_eq!(got[0].0, "perm-hidden-meta-perm-rule-m");
        let body = only(&got);
        assert_eq!(body["serviceType"], "trino");
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["resources"]["catalog"]["values"], json!(["demo"]));
        assert_eq!(body["resources"]["schema"]["values"], json!(["sales"]));
        assert_eq!(body["resources"]["table"]["values"], json!(["orders$*"]));
        assert_eq!(body["resources"]["column"]["values"], json!(["*"]));
        assert_eq!(body["denyPolicyItems"][0]["groups"], json!(["public"]));
        assert_eq!(
            body["denyPolicyItems"][0]["accesses"],
            json!([{"type": "select", "isAllowed": true}])
        );
        assert_eq!(
            body["denyExceptions"][0]["users"],
            json!(["admin", "alice"])
        );
        assert_eq!(body["policyItems"], json!([]));
    }

    /// The live leak: bob (filtered to `region = 'us'`) read the `eu`
    /// partition values and bounds from `orders$files` / `$partitions`.
    #[test]
    fn a_filtered_table_denies_its_metadata_tables_to_the_filtered() {
        let body = resource_denies(&[row_filter("orders", &["carol"])], "tr");
        let body = only(&body);
        assert_eq!(body["resources"]["table"]["values"], json!(["orders$*"]));
        assert_eq!(body["denyPolicyItems"][0]["groups"], json!(["public"]));
        assert_eq!(body["denyExceptions"][0]["users"], json!(["carol"]));
    }

    /// First match wins: an exempt item AFTER a restricting one does not
    /// except anybody (an over-refusal of metadata tables, never a leak).
    #[test]
    fn only_exempt_items_before_the_first_restricting_one_are_exceptions() {
        let body = json!({
            "name": "hand",
            "policyType": 1,
            "resources": path(json!({"values": ["orders"]}), Some("email")),
            "dataMaskPolicyItems": [
                {"users": ["ann"], "accesses": allow(&["select"]),
                 "dataMaskInfo": {"dataMaskType": "MASK_NONE"}},
                {"users": ["bob"], "groups": ["analysts"], "accesses": allow(&["select"]),
                 "dataMaskInfo": {"dataMaskType": "MASK_HASH"}},
                {"users": ["cy"], "accesses": allow(&["select"]),
                 "dataMaskInfo": {"dataMaskType": "MASK_NONE"}}
            ]
        });
        let got = resource_denies(&[body], "tr");
        let deny = only(&got);
        assert_eq!(deny["denyPolicyItems"][0]["users"], json!(["bob"]));
        assert_eq!(deny["denyPolicyItems"][0]["groups"], json!(["analysts"]));
        assert_eq!(deny["denyExceptions"][0]["users"], json!(["ann"]));
    }

    /// A deny on `orders` with an allow on `*` elsewhere: `orders$files`
    /// matched only the allow.
    #[test]
    fn a_table_deny_extends_to_its_metadata_tables_with_its_exceptions() {
        let body = json!({
            "name": "perm-grant-d",
            "policyType": 0,
            "resources": path(json!({"values": ["orders", "items"]}), Some("*")),
            "policyItems": [],
            "denyPolicyItems": [
                {"users": ["dave"], "accesses": allow(&["select", "insert"])},
                {"users": ["erin"], "accesses": allow(&["insert"])}
            ],
            "denyExceptions": [{"users": ["frank"], "accesses": allow(&["select"])}]
        });
        let got = resource_denies(&[body], "tr");
        let deny = only(&got);
        assert_eq!(
            deny["resources"]["table"]["values"],
            json!(["orders$*", "items$*"])
        );
        assert_eq!(deny["denyPolicyItems"][0]["users"], json!(["dave"]));
        assert_eq!(deny["denyExceptions"][0]["users"], json!(["frank"]));
    }

    #[test]
    fn an_allow_only_policy_or_a_mask_with_no_restricting_item_adds_nothing() {
        let grant = json!({
            "name": "perm-grant-a",
            "policyType": 0,
            "resources": path(json!({"values": ["*"]}), Some("*")),
            "policyItems": [{"users": ["alice"], "accesses": allow(&["select"])}],
            "denyPolicyItems": []
        });
        let clear = json!({
            "name": "clear",
            "policyType": 1,
            "resources": path(json!({"values": ["orders"]}), Some("email")),
            "dataMaskPolicyItems": [{"groups": ["public"], "accesses": allow(&["select"]),
                "dataMaskInfo": {"dataMaskType": "MASK_NONE"}}]
        });
        assert!(resource_denies(&[grant, clear], "tr").is_empty());
    }

    #[test]
    fn a_disabled_policy_or_one_without_a_table_level_adds_nothing() {
        let mut disabled = mask("orders", &[]);
        disabled["isEnabled"] = json!(false);
        let schema_deny = json!({
            "name": "s",
            "policyType": 0,
            "resources": {"catalog": {"values": ["demo"]}, "schema": {"values": ["sales"]}},
            "denyPolicyItems": [{"users": ["dave"], "accesses": allow(&["select"])}]
        });
        assert!(resource_denies(&[disabled, schema_deny], "tr").is_empty());
    }

    /// `!orders$*` would match every ordinary table, so an excluding table
    /// level denies the metadata tables of every table instead.
    #[test]
    fn an_excluding_table_level_denies_every_metadata_table() {
        let mut m = mask("x", &["ann"]);
        m["resources"]["table"] = json!({"values": ["orders"], "isExcludes": true});
        let got = resource_denies(&[m], "tr");
        let deny = only(&got);
        assert_eq!(deny["resources"]["table"]["values"], json!(["*$*"]));
        assert_eq!(deny["resources"]["table"]["isExcludes"], json!(false));
    }

    #[test]
    fn a_wildcard_table_keeps_its_pattern() {
        let got = resource_denies(&[mask("ord*", &[])], "tr");
        let deny = only(&got);
        assert_eq!(deny["resources"]["table"]["values"], json!(["ord*$*"]));
        assert!(deny["denyExceptions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn deny_all_else_restricts_public_except_its_select_allows() {
        let body = json!({
            "name": "dae",
            "policyType": 0,
            "isDenyAllElse": true,
            "resources": path(json!({"values": ["orders"]}), Some("*")),
            "policyItems": [{"users": ["ann"], "accesses": allow(&["select"])}]
        });
        let got = resource_denies(&[body], "tr");
        let deny = only(&got);
        assert_eq!(deny["denyPolicyItems"][0]["groups"], json!(["public"]));
        assert_eq!(deny["denyExceptions"][0]["users"], json!(["ann"]));
    }

    /// A Trino tag mask or clearance matches columns by tag; the deny goes on
    /// each table carrying the tag, in the profile.
    #[test]
    fn a_tag_policy_denies_the_metadata_tables_of_each_tagged_table() {
        let clearance = json!({
            "name": "perm-clearance-t1",
            "policyType": 0,
            "serviceType": "tag",
            "resources": {"tag": {"values": ["PII"]}},
            "denyPolicyItems": [{"groups": ["public"], "accesses": allow(&["select", "insert"])}],
            "denyExceptions": [{"users": ["ann"], "accesses": allow(&["select", "insert"])}]
        });
        let tagged = BTreeMap::from([(
            "PII".to_string(),
            BTreeSet::from([
                ["demo", "sales", "orders"].map(str::to_string),
                ["demo", "hr", "people"].map(str::to_string),
            ]),
        )]);
        let got = tag_denies(&[clearance], &tagged, "tr");
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "perm-hidden-meta-perm-clearance-t1-demo.hr.people",
                "perm-hidden-meta-perm-clearance-t1-demo.sales.orders"
            ]
        );
        let deny = &got[1].1;
        assert_eq!(deny["service"], "tr");
        assert_eq!(deny["serviceType"], "trino");
        assert_eq!(deny["resources"]["table"]["values"], json!(["orders$*"]));
        assert_eq!(deny["resources"]["schema"]["values"], json!(["sales"]));
        assert_eq!(deny["denyPolicyItems"][0]["groups"], json!(["public"]));
        assert_eq!(deny["denyExceptions"][0]["users"], json!(["ann"]));
    }

    #[test]
    fn a_tag_policy_whose_tag_no_table_carries_adds_nothing() {
        let got = tag_denies(&[mask("orders", &[])], &BTreeMap::new(), "tr");
        assert!(got.is_empty());
    }
}
