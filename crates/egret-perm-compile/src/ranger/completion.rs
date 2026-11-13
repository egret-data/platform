//! The served-unit completion passes (governance sub-project 1, Task 10):
//! the denies a profile needs because of what the REST of its served unit
//! does, run by [`complete`] on the complete desired set, after every family
//! is rendered and before the overlap check. Which passes run is the
//! capability matrix's answer for the enforcement point ([`passes`]):
//!
//! - **M16, StarRocks** ([`Pass::HiddenColumns`]): one deny per masked table
//!   on `_file`/`_pos`, which a `SELECT *` still reads when every visible
//!   column is masked ([`hidden_column_denies`]).
//! - **M17, Trino** ([`Pass::MetadataTables`]): a deny on the Iceberg metadata
//!   tables (`t$files`, ...) of each table a restricting policy reaches
//!   ([`metadata_tables`]).
//!
//! A classification's CLEARANCE itself, which this file used to compile, is the IR's `withhold` effect (governance sub-project 1,
//! Task 7): the decisions are `egret_perm_ir::lower::withhold`'s, the reads
//! `crate::ir_load::load_withhold_inputs`', and the Ranger shape
//! `crate::ranger::withhold`'s, whose `clearance_input` the hidden-column deny
//! shares.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{CompileError, EngineType};
use egret_policy::admin::{access_to_ranger_json, ResourceLevel};
use egret_policy::COMPILER_HIDDEN_PREFIX;

use egret_perm_ir::ep::EpKind;
use egret_perm_ir::matrix::{support_for_kind, Capability, LoweringKind, Support};

use crate::ir_load::Loaded;
use crate::ranger::rules::stamp_service_type;
use crate::ranger::withhold::clearance_input;
use crate::table_columns::active_tables_via;
use crate::trigger::{DesiredPolicy, PolicyKey, SourceKind};

pub(crate) mod metadata_tables;

/// A completion pass, chosen by the capability matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pass {
    /// M16: `MaskCoversHiddenColumns` lowers to `DenyHiddenColumns`.
    HiddenColumns,
    /// M17: `PoliciesReachMetadataTables` lowers to `DenyMetadataTables`.
    MetadataTables,
}

/// The passes an enforcement point of `kind` runs, in the order [`complete`]
/// runs them: each pass whose capability the matrix answers with its deny
/// lowering. Both entries hold for every version and do not depend on the
/// classification channel.
pub(crate) fn passes(kind: EpKind) -> Vec<Pass> {
    [
        (
            Capability::MaskCoversHiddenColumns,
            LoweringKind::DenyHiddenColumns,
            Pass::HiddenColumns,
        ),
        (
            Capability::PoliciesReachMetadataTables,
            LoweringKind::DenyMetadataTables,
            Pass::MetadataTables,
        ),
    ]
    .into_iter()
    .filter(|(cap, lowering, _)| {
        support_for_kind(kind, None, None, false, cap) == Support::Lowered(*lowering)
    })
    .map(|(_, _, pass)| pass)
    .collect()
}

/// Run the completion passes for the profile `l` loaded on `desired`, the
/// complete set every family rendered: each pass adds its denies under the
/// profile. `tag_service_id` is the profile's tag service, when it has one.
pub(crate) async fn complete(
    conn: &mut sqlx::PgConnection,
    l: &Loaded,
    desired: &mut BTreeMap<PolicyKey, DesiredPolicy>,
    tag_service_id: Option<i64>,
) -> Result<(), CompileError> {
    let passes = passes(l.ep.kind);
    let service_id = l.ep.profile_id;
    let profile_name = &l.profile_name;

    // **§11.4: hide `_file` and `_pos` wherever the profile masks a table.** A
    // `SELECT *` on a masked StarRocks table still returns those two columns
    // (the Iceberg data-file path and the row position) with every visible
    // column nulled, so the masks alone leak the path. The masks are the
    // profile's compiled ones and its enabled hand-written ones, the same set
    // the clearance fold sees as `others`. Read [`hidden_column_denies`].
    if passes.contains(&Pass::HiddenColumns) {
        let is_mask = |b: &Value| b.get("policyType").and_then(Value::as_i64) == Some(1);
        let mut masks: Vec<Value> = desired
            .iter()
            .filter(|((sid, _), dp)| *sid == service_id && is_mask(&dp.body))
            .map(|(_, dp)| dp.body.clone())
            .collect();
        masks.extend(
            egret_policy::enabled_policies_for_service_via(conn, service_id)
                .await
                .map_err(crate::db_error::from_store)?
                .into_iter()
                .filter(|b| {
                    is_mask(b)
                        && !b
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(egret_policy::is_compiler_owned_policy_name)
                }),
        );
        for (name, body) in hidden_column_denies(conn, &masks, profile_name).await? {
            desired.insert(
                (service_id, name),
                DesiredPolicy {
                    body,
                    source_kind: SourceKind::Clearance,
                    grant_id: None,
                    rule_id: None,
                },
            );
        }
    }

    // **Trino: a mask, row filter or deny does not reach `t$files`.** Trino
    // asks Ranger about a metadata table under its own name (`orders$files`),
    // which a `table: *` grant matches and a policy on `orders` does not, so
    // the people a policy restricts read the bounds, partition values and
    // counts it withholds. Each restricting policy of the profile (compiled
    // and hand-written) and of its tag service gets a deny on the metadata
    // tables of the tables it reaches. Read [`metadata_tables`].
    if passes.contains(&Pass::MetadataTables) {
        let mut profile_sources: Vec<Value> = Vec::new();
        let mut tag_sources: Vec<Value> = Vec::new();
        for ((sid, _), dp) in desired.iter() {
            if *sid == service_id {
                profile_sources.push(dp.body.clone());
            } else if Some(*sid) == tag_service_id {
                tag_sources.push(dp.body.clone());
            }
        }
        let hand_written = |b: &Value| {
            !b.get("name")
                .and_then(Value::as_str)
                .is_some_and(egret_policy::is_compiler_owned_policy_name)
        };
        profile_sources.extend(
            egret_policy::enabled_policies_for_service_via(conn, service_id)
                .await
                .map_err(crate::db_error::from_store)?
                .into_iter()
                .filter(hand_written),
        );
        if let Some(tag_id) = tag_service_id {
            tag_sources.extend(
                egret_policy::enabled_policies_for_service_via(conn, tag_id)
                    .await
                    .map_err(crate::db_error::from_store)?
                    .into_iter()
                    .filter(hand_written),
            );
        }
        let tags: Vec<String> = tag_sources
            .iter()
            .flat_map(metadata_tables::tag_values)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let tagged = metadata_tables::tagged_tables_via(conn, service_id, &tags).await?;
        let denies = metadata_tables::resource_denies(&profile_sources, profile_name)
            .into_iter()
            .chain(metadata_tables::tag_denies(
                &tag_sources,
                &tagged,
                profile_name,
            ));
        for (name, body) in denies {
            desired.insert(
                (service_id, name),
                DesiredPolicy {
                    body,
                    source_kind: SourceKind::Clearance,
                    grant_id: None,
                    rule_id: None,
                },
            );
        }
    }
    Ok(())
}

/// The hidden StarRocks columns a masked table still exposes (§11.4). A
/// `SELECT *` on a table whose visible columns are all masked still reads them:
/// `_file` returns the Iceberg data-file path and `_pos` the row position.
pub(crate) const HIDDEN_COLUMNS: [&str; 2] = ["_file", "_pos"];

/// The access type a hidden-column deny withholds: `select` alone, the only
/// one StarRocks checks at column granularity (`ColumnPrivilege.java:152`,
/// and `starrocks.json`'s `column` resource restricts itself to it). Pinned to
/// `engine_accesses(Read, StarRocks)` by a test.
const HIDDEN_ACCESSES: [&str; 1] = ["select"];

/// One StarRocks deny per masked table on [`HIDDEN_COLUMNS`] (§11.4), named
/// `COMPILER_HIDDEN_PREFIX` plus the table's folded `catalog.database.table`.
/// `masks` are the profile's data-mask bodies (`policyType 1`), compiler-made
/// and hand-written alike.
///
/// **Which tables.** A mask that names one exact table denies that table. A
/// mask whose selector is a wildcard or an exclusion denies every table it
/// matches, found by enumerating the active tables of the catalogs its catalog
/// level matches (the same Lakekeeper source the rest of the compile reads).
/// A database scope is never denied as a whole: that would refuse `SELECT *` on
/// unmasked tables in the same database. Fail closed: when a wildcard catalog
/// selector matches no registered catalog, or a matched catalog has no
/// warehouse to list, the compile is refused rather than guessed.
///
/// Each deny is `public`, excepting the users who see CLEAR values on every
/// masked column of that table: the intersection of the `MASK_NONE` users of
/// every mask that matches it. A user cleared for only some masked columns is
/// refused `SELECT *` rather than handed a path.
pub(crate) async fn hidden_column_denies(
    conn: &mut sqlx::PgConnection,
    masks: &[Value],
    service: &str,
) -> Result<Vec<(String, Value)>, CompileError> {
    use egret_ranger_eval::resource::fold_identifier as fold;

    let sources = mask_sources(masks);
    let mut tables: BTreeSet<[String; 3]> = BTreeSet::new();
    let wildcard: Vec<&MaskSource> = sources
        .iter()
        .filter(|s| exact_triple(&s.selector).is_none())
        .collect();
    if !wildcard.is_empty() {
        let catalogs: Vec<(Option<Uuid>, String)> =
            sqlx::query_as("SELECT warehouse_id, name FROM registry.catalogs")
                .fetch_all(&mut *conn)
                .await
                .map_err(CompileError::from_db)?;
        let mut listed: HashMap<Uuid, Vec<(String, String)>> = HashMap::new();
        for source in wildcard {
            let matched: Vec<&(Option<Uuid>, String)> = catalogs
                .iter()
                .filter(|(_, name)| source.selector[0].matches(&fold(name)))
                .collect();
            if matched.is_empty() {
                return Err(unenumerable(
                    source,
                    "no registered catalog matches its catalog level",
                ));
            }
            for (warehouse, catalog) in matched {
                let Some(warehouse) = warehouse else {
                    return Err(unenumerable(source, "the matched catalog has no warehouse"));
                };
                if !listed.contains_key(warehouse) {
                    let found = active_tables_via(&mut *conn, *warehouse).await?;
                    listed.insert(*warehouse, found);
                }
                for (namespace, table) in &listed[warehouse] {
                    tables.insert([fold(catalog), fold(namespace), fold(table)]);
                }
            }
        }
    }
    let tables: Vec<[String; 3]> = tables.into_iter().collect();
    plan_hidden_denies(&sources, &tables, service)
}

/// The refusal for a wildcard mask whose tables cannot be listed.
fn unenumerable(source: &MaskSource, reason: &str) -> CompileError {
    CompileError::HiddenColumnsUnenumerable {
        selector: source.selector_text(),
        reason: reason.to_string(),
    }
}

/// The pure half of [`hidden_column_denies`]: given the masks and the active
/// tables of every catalog a wildcard mask can reach, one deny per table a mask
/// matches. `tables` are folded `[catalog, database, table]` triples.
fn plan_hidden_denies(
    sources: &[MaskSource],
    tables: &[[String; 3]],
    service: &str,
) -> Result<Vec<(String, Value)>, CompileError> {
    let mut targets: BTreeSet<[String; 3]> = BTreeSet::new();
    for source in sources {
        if let Some(exact) = exact_triple(&source.selector) {
            targets.insert(exact);
        }
    }
    targets.extend(tables.iter().cloned());

    let accesses: Vec<String> = HIDDEN_ACCESSES.iter().map(|a| a.to_string()).collect();
    let mut out = Vec::new();
    for target in targets {
        let levels = target.each_ref().map(|name| Selected::literal(name));
        let reaching: Vec<&MaskSource> = sources
            .iter()
            .filter(|s| {
                s.selector
                    .iter()
                    .zip(&levels)
                    .all(|(sel, lvl)| sel.matches(&lvl.values[0]))
            })
            .collect();
        if reaching.is_empty() {
            continue;
        }
        let exempt: BTreeSet<String> = reaching
            .iter()
            .map(|s| s.cleared.clone())
            .reduce(|a, b| a.intersection(&b).cloned().collect())
            .unwrap_or_default();
        let name = format!(
            "{COMPILER_HIDDEN_PREFIX}{}.{}.{}",
            target[0], target[1], target[2]
        );
        let resources = BTreeMap::from([
            ("catalog".to_string(), levels[0].ranger()),
            ("database".to_string(), levels[1].ranger()),
            ("table".to_string(), levels[2].ranger()),
            (
                "column".to_string(),
                ResourceLevel {
                    values: HIDDEN_COLUMNS.iter().map(|c| c.to_string()).collect(),
                    is_excludes: false,
                    is_recursive: false,
                },
            ),
        ]);
        let input = clearance_input(
            name.clone(),
            service.to_string(),
            resources,
            &accesses,
            exempt.into_iter().collect(),
        );
        out.push((
            name,
            stamp_service_type(access_to_ranger_json(&input), EngineType::StarRocks),
        ));
    }
    Ok(out)
}

/// One data mask's table selector and the users it shows CLEAR values to.
struct MaskSource {
    selector: [Selected; 3],
    cleared: BTreeSet<String>,
}

impl MaskSource {
    /// `catalog.database.table` as the mask states it, for a refusal message.
    fn selector_text(&self) -> String {
        self.selector
            .iter()
            .map(Selected::text)
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// Every mask with policy items, as its selector and its `MASK_NONE` users. A
/// mask with no items masks nothing and contributes neither a deny nor an
/// exception.
fn mask_sources(masks: &[Value]) -> Vec<MaskSource> {
    masks
        .iter()
        .filter_map(|mask| {
            let items = mask.get("dataMaskPolicyItems").and_then(Value::as_array)?;
            let cleared: BTreeSet<String> = items
                .iter()
                .filter(|item| {
                    item.pointer("/dataMaskInfo/dataMaskType")
                        .and_then(Value::as_str)
                        == Some("MASK_NONE")
                })
                .flat_map(|item| {
                    item.get("users")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            Some(MaskSource {
                selector: selector(mask),
                cleared,
            })
        })
        .collect()
}

/// One level of a mask's `[catalog, database, table]` selector as Ranger reads
/// it: its values folded to lowercase (Ranger matches StarRocks names
/// case-insensitively), and whether it is an exclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Selected {
    values: Vec<String>,
    excludes: bool,
}

impl Selected {
    /// Every name at this level: what an absent or empty level means.
    fn any() -> Self {
        Self {
            values: vec!["*".to_string()],
            excludes: false,
        }
    }

    /// A single literal name at this level.
    fn literal(name: &str) -> Self {
        Self {
            values: vec![name.to_string()],
            excludes: false,
        }
    }

    /// The level as the mask states it. A level that is absent, empty, or holds
    /// a non-string is read as [`Selected::any`]: the wider reading, so the
    /// deny it feeds errs toward denying.
    fn from_mask(mask: &Value, level: &str) -> Self {
        let Some(r) = mask.pointer(&format!("/resources/{level}")) else {
            return Self::any();
        };
        let values: Option<Vec<String>> =
            r.get("values").and_then(Value::as_array).and_then(|vs| {
                vs.iter()
                    .map(|v| v.as_str().map(str::to_ascii_lowercase))
                    .collect()
            });
        match values {
            Some(values) if !values.is_empty() => Self {
                values,
                excludes: r
                    .get("isExcludes")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            _ => Self::any(),
        }
    }

    /// Whether the (folded) `name` is in this level: some value matches it as a
    /// Ranger glob, inverted for an exclusion.
    fn matches(&self, name: &str) -> bool {
        let hit = self.values.iter().any(|v| glob_matches(v, name));
        hit != self.excludes
    }

    fn has_wildcard(&self) -> bool {
        self.values
            .iter()
            .any(|v| v.contains('*') || v.contains('?'))
    }

    fn text(&self) -> String {
        let bang = if self.excludes { "!" } else { "" };
        format!("{bang}{}", self.values.join(","))
    }

    fn ranger(&self) -> ResourceLevel {
        ResourceLevel {
            values: self.values.clone(),
            is_excludes: self.excludes,
            is_recursive: false,
        }
    }
}

/// The `[catalog, database, table]` selector a data mask names.
fn selector(mask: &Value) -> [Selected; 3] {
    ["catalog", "database", "table"].map(|lvl| Selected::from_mask(mask, lvl))
}

/// The folded `[catalog, database, table]` a selector names exactly, or `None`
/// for a wildcard, an exclusion, or a level with other than one value.
fn exact_triple(selector: &[Selected; 3]) -> Option<[String; 3]> {
    if selector
        .iter()
        .any(|s| s.excludes || s.has_wildcard() || s.values.len() != 1)
    {
        return None;
    }
    Some(selector.each_ref().map(|s| s.values[0].clone()))
}

/// Ranger's glob for one name: `*` matches any run of characters, `?` one.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut resume) = (None::<usize>, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A data-mask body on `demo.sales.table`, column `column`, whose
    /// `MASK_NONE` outcome names `cleared`.
    fn mask_on(table: &str, column: &str, cleared: &[&str]) -> Value {
        mask_at(
            json!({"values": ["demo"]}),
            json!({"values": ["sales"]}),
            table,
            column,
            cleared,
        )
    }

    /// As [`mask_on`], with the catalog and database levels given verbatim.
    fn mask_at(
        catalog: Value,
        database: Value,
        table: &str,
        column: &str,
        cleared: &[&str],
    ) -> Value {
        json!({
            "policyType": 1,
            "resources": {
                "catalog": catalog,
                "database": database,
                "table": {"values": [table]},
                "column": {"values": [column]}
            },
            "dataMaskPolicyItems": [
                {
                    "users": cleared,
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": "MASK_NONE"}
                },
                {
                    "groups": ["public"],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": "MASK_NULL"}
                }
            ]
        })
    }

    /// A folded `[catalog, database, table]` triple, for the enumerated list.
    fn tbl(catalog: &str, database: &str, table: &str) -> [String; 3] {
        [catalog, database, table].map(str::to_string)
    }

    fn plan(masks: &[Value], tables: &[[String; 3]]) -> Vec<(String, Value)> {
        plan_hidden_denies(&mask_sources(masks), tables, "sr").expect("plans")
    }

    fn named<'a>(got: &'a [(String, Value)], name: &str) -> &'a Value {
        &got.iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no deny named {name}: {got:?}"))
            .1
    }

    /// The passes an enforcement point runs are the ones the matrix lowers.
    #[test]
    fn completion_passes_are_chosen_by_the_matrix() {
        use egret_perm_ir::ep::{EpKind, Mechanism};
        use egret_perm_ir::matrix::{support_for_kind, Capability, LoweringKind, Support};
        let k = |engine| EpKind {
            engine,
            mechanism: Mechanism::Ranger,
        };
        assert_eq!(
            passes(k(egret_perm::EngineType::Trino)),
            vec![Pass::MetadataTables]
        );
        assert_eq!(
            passes(k(egret_perm::EngineType::StarRocks)),
            vec![Pass::HiddenColumns]
        );
        assert_eq!(
            support_for_kind(
                k(egret_perm::EngineType::StarRocks),
                None,
                None,
                false,
                &Capability::MaskCoversHiddenColumns
            ),
            Support::Lowered(LoweringKind::DenyHiddenColumns)
        );
    }

    /// §11.4: a masked table gets one deny on the hidden columns, `public`
    /// denied, the cleared users excepted.
    #[test]
    fn a_masked_table_denies_public_the_hidden_columns_but_not_the_cleared() {
        let got = plan(&[mask_on("t", "ssn", &["ann"])], &[]);
        assert_eq!(got.len(), 1, "{got:?}");
        let (name, body) = &got[0];
        assert_eq!(name, "perm-hidden-demo.sales.t");
        assert_eq!(
            body["resources"]["column"]["values"],
            json!(["_file", "_pos"])
        );
        assert_eq!(body["denyPolicyItems"][0]["groups"], json!(["public"]));
        assert_eq!(body["denyExceptions"][0]["users"], json!(["ann"]));
    }

    /// Two masks on one table: the exception is the users cleared on EVERY
    /// masked column. Ann is cleared for `ssn` only, so she is not excepted.
    #[test]
    fn two_masks_on_one_table_except_only_the_users_cleared_on_both() {
        let masks = [
            mask_on("t", "ssn", &["ann", "bob"]),
            mask_on("t", "email", &["bob"]),
        ];
        let got = plan(&masks, &[]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].1["denyExceptions"][0]["users"], json!(["bob"]));
    }

    /// A wildcard table denies each table it matches, not the database scope:
    /// the unmasked `other` keeps its `SELECT *`.
    #[test]
    fn a_wildcard_table_mask_denies_each_matched_table_and_no_scope() {
        let tables = [
            tbl("demo", "sales", "t1"),
            tbl("demo", "sales", "t2"),
            tbl("demo", "sales", "other"),
        ];
        let got = plan(&[mask_on("t*", "ssn", &["ann"])], &tables);
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["perm-hidden-demo.sales.t1", "perm-hidden-demo.sales.t2"]
        );
        assert_eq!(
            named(&got, "perm-hidden-demo.sales.t1")["denyExceptions"][0]["users"],
            json!(["ann"])
        );
    }

    /// An excluded database matches the tables outside it only: `hr` is not
    /// denied, `sales` is.
    #[test]
    fn an_excluded_database_denies_only_the_tables_outside_it() {
        let masks = [mask_at(
            json!({"values": ["demo"]}),
            json!({"values": ["hr"], "isExcludes": true}),
            "t*",
            "ssn",
            &["ann"],
        )];
        let tables = [tbl("demo", "sales", "t1"), tbl("demo", "hr", "t1")];
        let got = plan(&masks, &tables);
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["perm-hidden-demo.sales.t1"]);
    }

    /// A wildcard mask on `sales` does not narrow the exception of the exact
    /// `hr` table deny: a mask on another database is disjoint from it.
    #[test]
    fn a_mask_on_another_database_does_not_narrow_its_deny() {
        let masks = [
            mask_at(
                json!({"values": ["demo"]}),
                json!({"values": ["hr"]}),
                "t",
                "ssn",
                &[],
            ),
            mask_on("t*", "ssn", &["ann"]),
        ];
        let tables = [tbl("demo", "sales", "t1")];
        let got = plan(&masks, &tables);
        assert_eq!(
            named(&got, "perm-hidden-demo.sales.t1")["denyExceptions"][0]["users"],
            json!(["ann"])
        );
        assert_eq!(
            named(&got, "perm-hidden-demo.hr.t")["denyExceptions"][0]["users"],
            json!([])
        );
    }

    /// A wildcard mask matches the exact table deny too, so the exception of
    /// that table is only the users cleared on both masks.
    #[test]
    fn a_wildcard_mask_narrows_the_exact_table_deny_it_matches() {
        let masks = [
            mask_on("t", "ssn", &["ann", "bob"]),
            mask_on("t*", "email", &["bob"]),
        ];
        let got = plan(&masks, &[]);
        assert_eq!(
            named(&got, "perm-hidden-demo.sales.t")["denyExceptions"][0]["users"],
            json!(["bob"])
        );
    }

    #[test]
    fn a_wildcard_with_no_listed_table_adds_no_deny() {
        assert!(plan(&[mask_on("t*", "ssn", &["ann"])], &[]).is_empty());
    }

    #[test]
    fn no_mask_means_no_hidden_deny() {
        assert!(plan(&[], &[tbl("demo", "sales", "t1")]).is_empty());
    }

    #[test]
    fn the_hidden_deny_withholds_what_a_starrocks_read_is() {
        assert_eq!(
            HIDDEN_ACCESSES.to_vec(),
            egret_perm::engine_accesses(egret_perm::Action::Read, EngineType::StarRocks).unwrap()
        );
    }

    #[test]
    fn glob_matches_ranger_stars_and_question_marks() {
        assert!(glob_matches("t*", "t1"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("a*b*c", "axxbyyc"));
        assert!(glob_matches("t?", "t1"));
        assert!(!glob_matches("t?", "t"));
        assert!(!glob_matches("t*", "x1"));
    }
}
