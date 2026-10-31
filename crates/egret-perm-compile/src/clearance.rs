//! Compiling a classification's CLEARANCE into an enforceable deny — SP4c
//! **D1**, on the branch D0's measurement forced.
//!
//! # The defect this closes
//!
//! A classification's clearance was enforced when someone CLICKED the object
//! and not enforced when they QUERIED the column. Cedar compiles a tag's
//! `required_roles`/`required_groups` into a `forbid` on control-plane object
//! actions (`egret_auth::pdp`); the permission model gates the data-plane query
//! through Ranger, and SP4a's rules read only a tag's `key`. So a user Cedar
//! forbids from opening a tagged dataset could read the same data through SQL
//! Lab or a notebook, and effective access was the UNION of whichever path the
//! user took rather than the intersection.
//!
//! # THIS AMENDS SP4a's D2 — "tags drive rules only, never access"
//!
//! **Stated here, at the site, so a future reader who finds a compiled tag
//! ACCESS policy does not conclude it is a bug.** D2 held that egret must never
//! emit one, because Ranger evaluates tag policies FIRST and short-circuits, so
//! a tag allow would end evaluation ahead of every compiled resource policy
//! including a compiled deny. A clearance cannot be expressed as either shape
//! D2 permits: a mask and a row filter both still ANSWER the query, and a
//! classification exists to WITHHOLD the data.
//!
//! The amendment is therefore narrow and one-directional. This module emits a
//! tag access policy that is **only ever a DENY** — never an allow, which is
//! the direction that could short-circuit ahead of a compiled deny grant — and
//! the tag-first short-circuit is exactly what makes a clearance deny WIN over
//! any resource grant, which is what a clearance has to do. See
//! `egret_policy::admin`'s re-export of `tag_access_to_ranger_json`, the other
//! half of this amendment.
//!
//! # Why the cleared principals are ENUMERATED, and why nothing names the role
//!
//! D0 (`docs/en/plans/2026-09-28-sp4c-d0-result.md`) measured, by mechanism and
//! corroborated by the live coordinator's own log, that **a Ranger policy item
//! naming a role matches nobody** on the Trino path egret provisions, and — the
//! part that decided the shape — that **an unmatched item is not neutral.** A
//! policy item matches only by set INTERSECTION and the role clause
//! short-circuits on an empty set, so an unresolvable role in `denyExceptions`
//! excepts NOBODY and the deny stands for EVERYONE. The obvious implementation,
//! *deny `{public}` except role `analyst`*, would have refused the tagged
//! column to every user INCLUDING the cleared ones.
//!
//! So: the deny item names `{public}` (measured to still match everybody with
//! no group provider — `RangerDefaultPolicyItemEvaluator` needs only a non-null
//! `userGroups`, and `setUserGroups` normalises `null` to an empty set), and
//! `denyExceptions` carries the clearing roles' MEMBERS, enumerated at compile
//! time as bare usernames read from `users.username`, a missing one minted
//! ([`crate::members::role_usernames`], 2026-10-18 design R5, R6: a holder
//! with no username is excepted at once, as Cedar's `classification_forbid`
//! clears them by role, instead of after some later recompile). The Ranger
//! `roles` bucket is not used
//! at all. Deny and exceptions ARE evaluated despite egret's service-defs
//! carrying `"options": null` — the hidden
//! `ranger.servicedef.enableDenyAndExceptionsInPolicies` default is `true`
//! (`ServiceDefUtil.getOption_enableDenyAndExceptionsInPolicies`), the second
//! adjacent fact D0 checked because it alone could have made this shape inert.
//!
//! Enumeration is why D3's third recompile trigger (role membership changing)
//! is mandatory rather than conditional — SP4c Task 3 owns it, for this module
//! and for the role-grant expansion Task 2b adds, which is the same expansion.
//!
//! # A tag gated ONLY by `required_groups` compiles to NOTHING, and says so
//!
//! D7 measured that a Ranger item naming a GROUP matches nobody either (no
//! group provider on Trino, no usersync in egret). A group-only gate would
//! therefore compile to a deny whose exceptions except nobody — a control that
//! withholds the column from everybody — or, if the group were put on the deny
//! side, to one that withholds nothing. Neither is the clearance that was
//! authored, so nothing is emitted and the skip is REPORTED
//! ([`crate::SkippedItem`]) and logged rather than being silent. SP4c Task 5
//! says the same thing in the UI, where the clearance is set.
//!
//! # Two shapes, because the two engines resolve a tag differently
//!
//! **Trino** has a Ranger TAG service, so a clearance is ONE tag policy
//! addressed to it (`config_jsonb->>'tagService'`), covering however many
//! columns currently carry the tag — O(1), installation-wide, and evaluated
//! before every resource policy.
//!
//! **StarRocks has no tag service** (upstream `starrocks#67458`), so a
//! clearance expands to **one ACCESS deny per tagged column**, reusing the very
//! same `column_resources` expansion SP4a's tag MASKS already use on this
//! engine ([`crate::rules::column_resources`]). D0b
//! (`docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md`) measured that this
//! works, and three of its findings constrain the shape:
//!
//! - **`select` only, by user ruling (2026-09-28).** `checkColumnAction` has
//!   exactly ONE caller and it passes `PrivilegeType.SELECT`
//!   (`ColumnPrivilege.java:152`), and egret's own `starrocks.json` restricts
//!   the `column` resource to `accessTypeRestrictions: ["select"]`. The
//!   `insert`/`update`/`delete` this compiler withholds on Trino can therefore
//!   NEVER match at column granularity here: emitting them would be decoration
//!   that reads as protection. **So a user without clearance can still
//!   OVERWRITE a column they cannot read** — and the live gate measured that
//!   this is true of **Trino too**, so it is not an asymmetry between the
//!   engines: the uncleared subject INSERTed and DELETEd through a live Trino
//!   clearance, because the deny names a *column* resource while the write check
//!   presents a *table* one. Neither engine withholds a write. Buying the write
//!   half needs a *table*-level deny, which would block every untagged column of
//!   that table too; that is a user ruling, not a compiler decision, and the
//!   ruling so far is "reads only".
//! - **No dedup, so the count is exactly N.** A tag-selected ROW FILTER dedups
//!   per table because `rowFilterDef` declares no column resource, so two
//!   tagged columns of one table are the same filter. An access deny DOES carry
//!   a column resource, so two tagged columns are two different denies and
//!   collapsing them would un-deny one of the columns.
//! - **Priority 0, `is_override: false`.** On Trino the tag-first
//!   short-circuit is what makes a clearance beat a resource grant. There is no
//!   such short-circuit here; deny-beats-allow comes instead from
//!   `PolicyEvalOrderComparator.compareNormal` sorting a `hasDeny()` evaluator
//!   first, and an allow becoming final only once `!evaluator.hasDeny()`
//!   (`RangerPolicyEngineImpl.java:718-722`) — which holds **only at EQUAL
//!   policy priority**, because priority dominates that comparator. An OVERRIDE
//!   here would break the very ordering it looks like it strengthens.
//!
//! **The expansion ceilings apply, and a clearance now competes for the same
//! budget as a mask.** `DEFAULT_EXPANSION_CEILING` bounds one clearance
//! ([`CompileError::ClearanceExpansionCeiling`], named after the TAG because a
//! clearance is not a `perm.rules` row), and `PROFILE_EXPANSION_CEILING` bounds
//! the profile's running total — which a StarRocks clearance counts against and
//! a Trino one does not, because the Trino body is filed under the TAG service
//! and the total is scoped to the profile's own `service_id`. A tag that is both
//! masked and gated therefore costs 2N against that total for N columns. Both
//! ceilings **refuse rather than truncate**: a truncated clearance is a control
//! that withholds some of the columns it claims to, which is the failure this
//! programme keeps refusing to ship.
//!
//! A clearance is **not** a view-proof control, on either engine, and this is
//! MEASURED (live case 4, `docs/en/plans/2026-09-28-sp4c-live-result.md`):
//! `ColumnPrivilege` routes a `View` to `checkViewPrivilege` and a materialized
//! view to `checkMaterializedViewAction`, neither of which descends to
//! base-table columns, and a StarRocks view returned every classified value to
//! the uncleared subject. **Trino is worse, not better**: its views are
//! DEFINER-rights, so a view owned by a CLEARED user hands the whole column to
//! an uncleared reader whose direct read is refused in the same minute, and a
//! view owned by an UNCLEARED user blocks everybody including the cleared. The
//! materialized-view half remains source-only — the gate recorded both MVs as
//! BLOCKED (§4c) and declined to call them measured. The ruling is to state the
//! limit, not to block view creation, so it is stated in D0b, in D5's
//! acceptance criterion, and in the UI where a clearance is authored.
//!
//! **Amended by view governance A2 (2026-10-13) for StarRocks.** The per-column
//! deny above is unchanged and still withholds only the DIRECT read. Each
//! tagged column now also gets a priority-1 `MASK_NULL` clearance mask, which a
//! base-column mask applies through every view kind; this module hands the
//! (tag, column, cleared) triples to `crate::trigger`, which folds them through
//! the evaluator once the whole desired set is known. See
//! `crate::clearance_mask`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{engine_accesses, Action, CompileError, EngineType, TableCount};
use egret_policy::admin::{
    access_to_ranger_json, tag_access_to_ranger_json, AccessItemInput, AccessPolicyInput,
    ResourceLevel,
};
use egret_policy::{
    tag_assignments_for_service_via, COMPILER_CLEARANCE_PREFIX, COMPILER_HIDDEN_PREFIX,
};

use crate::clearance_mask::PendingMask;
use crate::grants::{level, PolicyTarget};
use crate::rules::{
    column_resources, group_by_table, stamp_service_type, TableTarget, TaggedColumn, PUBLIC_GROUP,
};
use crate::table_columns::{
    active_tables_via, current_columns_batch_via, Leaving, TableColumns, TableKey,
};
use crate::trigger::{SkippedItem, SourceKind, DEFAULT_EXPANSION_CEILING};

/// One `classification_policies` row joined to the tag it gates. A row whose
/// tag no longer exists is dropped by the join — there is no tag left to deny
/// on.
#[derive(sqlx::FromRow)]
struct GateRow {
    tag_id: Uuid,
    /// `tags.name`, never `tags.key`: the tag RESOURCE in a Ranger tag policy
    /// must match the tag type egret serves in its tag download
    /// (`ServiceTags::from_assignments` carries `tag_name`), which is the same
    /// resolution `crate::rules` uses via `tag_name_for_key_via`.
    tag_name: String,
    /// `tags.key` — the STABLE identifier, needed only on the StarRocks branch:
    /// `tag_assignments_for_service_via` selects assignments by key, the same
    /// lookup `crate::trigger` already does for a tag-selected rule. The Trino
    /// branch never uses it (a tag POLICY matches on the name).
    tag_key: String,
    required_roles: Vec<String>,
    required_groups: Vec<Uuid>,
}

/// What compiling this installation's clearances produced for ONE profile: the
/// rendered policy bodies keyed by the name they must exist under, and the
/// gated tags this profile could not express.
///
/// On Trino each body is already addressed to the TAG service by `tagify`; on
/// StarRocks each is addressed to the profile itself. Either way
/// `crate::trigger::target_service_id` routes it from the body's own `service`
/// field, so this struct does not need to say which.
pub(crate) struct Clearances {
    pub(crate) policies: Vec<(String, Value)>,
    pub(crate) skipped: Vec<SkippedItem>,
    /// StarRocks only (view governance A2): one entry per (gated tag, tagged
    /// column), for `crate::trigger` to fold into a priority-1 clearance MASK
    /// once the whole desired set is known — the fold reads the profile's other
    /// masks, which this function cannot see. Empty on Trino.
    pub(crate) masks: Vec<PendingMask>,
    /// StarRocks only (D2): the largest tables classified as a whole, for the
    /// profile ceiling's refusal to name (each costs a mask per column).
    pub(crate) whole_tables: Vec<TableCount>,
}

/// Compile every gated tag into a clearance deny for one profile.
///
/// On **Trino** a clearance is **installation-wide**, exactly like a
/// tag-selected rule: the policy carries no catalog resource at all and the tag
/// plugin resolves the tag wherever it has been assigned. So every gated tag
/// compiles into every Trino profile that has a tag service.
///
/// On **StarRocks** it is service-scoped, because the expansion is: only the
/// columns reachable from `service_id` (`tag_assignments_for_service_via`) can be
/// addressed by a policy served to that profile's plugin. A gated tag with no
/// reachable tagged column compiles to nothing there, and that is NOT a skip —
/// there is no column to withhold, which is the same answer SP4a's tag masks
/// give for the same input.
///
/// `service` is the profile's own Ranger service name (what a StarRocks body
/// stamps on itself); `tag_service` is its TAG service name, `None` for
/// StarRocks and for a Trino profile whose `tagService` is unconfigured or names
/// a profile that does not exist.
pub(crate) async fn compile_clearance(
    conn: &mut sqlx::PgConnection,
    engine: EngineType,
    service_id: i64,
    service: &str,
    tag_service: Option<&str>,
    leaving: &Leaving,
) -> Result<Clearances, CompileError> {
    let gates: Vec<GateRow> = sqlx::query_as(
        "SELECT cp.tag_id, t.name AS tag_name, t.key AS tag_key, cp.required_roles, \
                cp.required_groups \
           FROM classification_policies cp \
           JOIN tags t ON t.id = cp.tag_id \
          ORDER BY cp.tag_id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;

    let mut out = Clearances {
        policies: Vec::new(),
        skipped: Vec::new(),
        masks: Vec::new(),
        whole_tables: Vec::new(),
    };
    if gates.is_empty() {
        return Ok(out);
    }

    // One query for every role named by every gate, rather than one per tag.
    let wanted: Vec<String> = {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for g in &gates {
            set.extend(g.required_roles.iter().cloned());
        }
        set.into_iter().collect()
    };
    // `Mint`: a clearing-role holder with no username yet is excepted now,
    // not after the recompile that follows their first query (design R6).
    let members =
        crate::members::role_usernames(conn, &wanted, crate::members::MissingUsername::Mint)
            .await?;

    let accesses = withheld_accesses(engine)?;
    // StarRocks: compiled once every gate is known, so the column source is
    // asked about every whole-table attachment in one query.
    let mut starrocks: Vec<StarRocksGate<'_>> = Vec::new();

    for g in &gates {
        // BOTH lists empty is NO gate — an informational tag
        // (`egret_bi::tags::ClassificationPolicyRepo`'s own semantics, and the
        // Cedar loader's). Not a skip: there is no control here to report the
        // absence of.
        if g.required_roles.is_empty() && g.required_groups.is_empty() {
            continue;
        }
        if g.required_roles.is_empty() {
            out.skipped.push(skip(
                g,
                "the tag's clearance names only `required_groups`, and D7 measured that a Ranger \
                 item naming a group matches nobody on any engine egret provisions — so a deny \
                 emitted for it would withhold the column from everybody, and the gate stays \
                 enforced on object access (Cedar) only"
                    .to_string(),
            ));
            continue;
        }

        // Cleared = holds AT LEAST ONE of the required roles
        // (`ClassificationPolicyRepo`'s semantics: `>= 1` role OR `>= 1`
        // group), so the exception list is the UNION of their member sets.
        // Sorted + deduplicated so a recompile that changed nothing writes
        // nothing (`apply_diff_in_tx` diffs the body byte-for-byte).
        let mut cleared: Vec<String> = Vec::new();
        for role in &g.required_roles {
            if let Some(names) = members.get(role) {
                cleared.extend(names.iter().cloned());
            }
        }
        cleared.sort();
        cleared.dedup();
        if cleared.is_empty() {
            // NOT a skip, and not a defect: nobody holds the clearing role, so
            // nobody is cleared, and a deny standing for everyone is the
            // correct compilation of that — the same answer Cedar's `forbid …
            // unless context.roles.containsAny(...)` reaches. Logged because
            // "the column is refused to everybody" is worth being able to find
            // in a log when someone asks why.
            tracing::info!(
                tag_id = %g.tag_id,
                roles = ?g.required_roles,
                "a clearance's required roles have no holders — the compiled \
                 deny withholds the tagged column from everybody, which is what 'nobody is \
                 cleared' means"
            );
        }

        match engine {
            EngineType::Trino => {
                let Some(tag_service) = tag_service else {
                    out.skipped.push(skip(
                        g,
                        "this Trino profile has no usable `tagService` (unconfigured, or naming a \
                         profile that does not exist), and a Trino clearance is a TAG policy — \
                         nothing filed here could be served to any plugin"
                            .to_string(),
                    ));
                    continue;
                };
                let name = format!("{COMPILER_CLEARANCE_PREFIX}{}", g.tag_id);
                let input = clearance_input(
                    name.clone(),
                    tag_service.to_string(),
                    // `tagify` replaces the resources map with the single `tag`
                    // resource, exactly as it does for a compiled tag mask.
                    BTreeMap::new(),
                    &accesses,
                    cleared,
                );
                out.policies.push((
                    name,
                    tag_access_to_ranger_json(&input, &g.tag_name, tag_service),
                ));
            }
            // **SP4c Task 2d — the per-column expansion, on the engine that has
            // no tag service to do it for us.** The same `column_resources`
            // SP4a's tag masks use, at `PolicyTarget::Access`: one policy per
            // matched column and NO dedup, because an access deny DOES carry a
            // column resource and collapsing two columns would un-deny one.
            //
            // **D2 (2026-10-25): a whole-table attachment is ONE `column: *`
            // deny** (user decision U1), which needs no column list and also
            // covers columns added later, the hidden `_file`/`_pos` and
            // `___count___` StarRocks checks when every column is masked. It
            // covers the tag's per-column denies on the same table (C6): same
            // exceptions, wider resource.
            EngineType::StarRocks => {
                let targets = group_by_table(
                    tag_assignments_for_service_via(conn, service_id, &g.tag_key)
                        .await
                        .map_err(crate::db_error::from_store)?,
                );
                starrocks.push(StarRocksGate {
                    gate: g,
                    cleared,
                    targets,
                });
            }
        }
    }

    if engine == EngineType::StarRocks {
        let mv_cleared =
            compile_starrocks_gates(conn, service, &starrocks, &accesses, leaving, &mut out)
                .await?;
        for (name, input) in mv_deny_inputs(&mv_cleared, service) {
            out.policies.push((
                name,
                stamp_service_type(access_to_ranger_json(&input), engine),
            ));
        }
    }

    Ok(out)
}

/// One gated tag with required roles on StarRocks: its cleared users and the
/// tables it reaches, at either level.
struct StarRocksGate<'a> {
    gate: &'a GateRow,
    cleared: Vec<String>,
    targets: Vec<TableTarget>,
}

/// The StarRocks half of [`compile_clearance`]: per gate, the denies and the
/// pending masks, into `out`. Returns one cleared set per gate that reaches
/// the profile, for the materialized-view deny (C7).
///
/// **Whole-table attachments (D2).** The column source
/// ([`crate::table_columns`]) is asked once for every table any gate reaches
/// at table level. A table it knows gets a NULL mask per current column (C1),
/// and the catalog's spelling of its schema, table and columns for every
/// policy on it, whichever gate's (C5); spelled as the catalog spells it, two
/// spellings of one table can never become two policies.
async fn compile_starrocks_gates(
    conn: &mut sqlx::PgConnection,
    service: &str,
    gates: &[StarRocksGate<'_>],
    accesses: &[String],
    leaving: &Leaving,
    out: &mut Clearances,
) -> Result<Vec<Vec<String>>, CompileError> {
    use egret_ranger_eval::resource::fold_identifier as fold;
    type Key = (Uuid, String, String);
    let key_of = |t: &TableTarget| -> Key { (t.warehouse_id, fold(&t.schema), fold(&t.table)) };
    let mut keys: Vec<TableKey> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    for t in gates
        .iter()
        .flat_map(|g| g.targets.iter())
        .filter(|t| t.table_level)
    {
        index.entry(key_of(t)).or_insert_with(|| {
            keys.push(TableKey {
                warehouse_id: t.warehouse_id,
                schema: t.schema.clone(),
                table: t.table.clone(),
            });
            keys.len() - 1
        });
    }
    let found = current_columns_batch_via(conn, &keys).await?;
    let catalog_of = |t: &TableTarget| -> Option<&TableColumns> {
        index.get(&key_of(t)).and_then(|&i| found[i].as_ref())
    };

    let mut mv_cleared = Vec::new();
    let mut whole_tables: BTreeMap<String, usize> = BTreeMap::new();
    for g in gates {
        // C9: a whole-table attachment costs its `*` deny and one mask per
        // known column; a column-level one its per-column denies (column
        // masks stay uncounted per tag, as before D2). A held rename's
        // leaving table or column costs nothing (rename spec C8).
        let mut per_column_denies = 0;
        let mut whole_table_cost = 0;
        let mut tables: Vec<TableCount> = Vec::new();
        for t in &g.targets {
            let counts =
                |c: Option<&str>| leaving.counts_toward_ceiling(&t.catalog, &t.schema, &t.table, c);
            if t.table_level {
                let (schema, table, columns) = match catalog_of(t) {
                    Some(c) => (c.schema.as_str(), c.table.as_str(), c.columns.as_slice()),
                    None => (t.schema.as_str(), t.table.as_str(), &[][..]),
                };
                if counts(None) {
                    whole_table_cost +=
                        1 + columns.iter().filter(|c| counts(Some(c.as_str()))).count();
                }
                let name = format!("{}.{schema}.{table}", t.catalog);
                whole_tables.insert(name.clone(), columns.len());
                tables.push(TableCount {
                    table: name,
                    columns: columns.len(),
                });
            } else {
                per_column_denies += t.columns.iter().filter(|c| counts(Some(c))).count();
            }
        }
        let count = per_column_denies + whole_table_cost;
        // **Refuse, never truncate.** A truncated clearance withholds SOME of
        // the columns it claims to, and the ones silently dropped are readable
        // by exactly the people the classification exists to keep out. The
        // refusal fails the write that caused it (`crate::trigger` propagates
        // it, the caller's `?` rolls back) rather than committing a clearance
        // the UI shows as active.
        if count > DEFAULT_EXPANSION_CEILING {
            return Err(CompileError::ClearanceExpansionCeiling {
                tag: g.gate.tag_name.clone(),
                count,
                ceiling: DEFAULT_EXPANSION_CEILING,
                tables: largest(tables),
                per_column_denies,
            });
        }
        for t in &g.targets {
            let catalog = catalog_of(t);
            let (schema, table) = match catalog {
                Some(c) => (c.schema.as_str(), c.table.as_str()),
                None => (t.schema.as_str(), t.table.as_str()),
            };
            // (column, spelled as the catalog spells it)
            let mut columns: Vec<(TaggedColumn, bool)> = Vec::new();
            for c in &t.columns {
                let known = catalog.and_then(|cat| cat.columns.iter().find(|k| fold(k) == fold(c)));
                columns.push((
                    TaggedColumn::new(&t.catalog, schema, table, known.unwrap_or(c))?,
                    known.is_some(),
                ));
            }
            if t.table_level {
                let all = TaggedColumn::new(&t.catalog, schema, table, WHOLE_TABLE_COLUMN)?;
                out.policies.push(starrocks_deny(
                    g.gate.tag_id,
                    service,
                    &all,
                    accesses,
                    g.cleared.clone(),
                ));
                match catalog {
                    Some(cat) => {
                        for c in &cat.columns {
                            columns.push((TaggedColumn::new(&t.catalog, schema, table, c)?, true));
                        }
                    }
                    // U4: the deny needs no column list and is compiled; the
                    // masks do. The live spike measured that the deny alone
                    // lets a view read real values, so the skip says so.
                    None => out.skipped.push(skip(
                        g.gate,
                        format!(
                            "{}.{schema}.{table} is not a table in the catalog (unknown, a view, \
                             or not yet created): its whole-table select deny is compiled, its \
                             NULL masks are not, so a view over it is not protected until the \
                             table exists",
                            t.catalog
                        ),
                    )),
                }
            } else {
                for (col, _) in &columns {
                    out.policies.push(starrocks_deny(
                        g.gate.tag_id,
                        service,
                        col,
                        accesses,
                        g.cleared.clone(),
                    ));
                }
            }
            // The deny withholds a DIRECT read only; a plain view reads
            // straight through it. The column's mask (A2) is what holds
            // through views — see `crate::clearance_mask`.
            for (column, catalog_spelled) in columns {
                out.masks.push(PendingMask {
                    tag_id: g.gate.tag_id,
                    column,
                    cleared: g.cleared.clone(),
                    catalog_spelled,
                });
            }
        }
        // C7: every gated tag that reaches the profile, at either level,
        // narrows the materialized-view deny.
        if !g.targets.is_empty() {
            mv_cleared.push(g.cleared.clone());
        }
    }
    out.whole_tables = largest(
        whole_tables
            .into_iter()
            .map(|(table, columns)| TableCount { table, columns })
            .collect(),
    );
    Ok(mv_cleared)
}

/// The five tables with the most columns, most first, for a ceiling refusal.
fn largest(mut tables: Vec<TableCount>) -> Vec<TableCount> {
    tables.sort_by(|a, b| {
        b.columns
            .cmp(&a.columns)
            .then_with(|| a.table.cmp(&b.table))
    });
    tables.truncate(5);
    tables
}

/// The `column` value of a whole-table clearance deny (U1): every column of
/// the table, including ones added after the compile and the hidden ones.
pub(crate) const WHOLE_TABLE_COLUMN: &str = "*";

/// One StarRocks clearance deny on `col` (a named column, or
/// [`WHOLE_TABLE_COLUMN`]), named `perm-clearance-{tag}-{c}.{d}.{t}.{col}`.
pub(crate) fn starrocks_deny(
    tag_id: Uuid,
    service: &str,
    col: &TaggedColumn,
    accesses: &[String],
    cleared: Vec<String>,
) -> (String, Value) {
    let name = format!(
        "{COMPILER_CLEARANCE_PREFIX}{tag_id}-{}.{}.{}.{}",
        col.catalog, col.schema, col.table, col.column
    );
    let input = clearance_input(
        name.clone(),
        service.to_string(),
        column_resources(EngineType::StarRocks, col, PolicyTarget::Access),
        accesses,
        cleared,
    );
    // `access_to_ranger_json` hardcodes `serviceType: "trino"`; a
    // StarRocks-targeted body rendered without this stamp deploys cleanly and
    // enforces nothing (`stamp_service_type`'s own reason for existing).
    (
        name,
        stamp_service_type(access_to_ranger_json(&input), EngineType::StarRocks),
    )
}

/// The hidden StarRocks columns a masked table still exposes (§11.4). A
/// `SELECT *` on a table whose visible columns are all masked still reads them:
/// `_file` returns the Iceberg data-file path and `_pos` the row position.
pub(crate) const HIDDEN_COLUMNS: [&str; 2] = ["_file", "_pos"];

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

    let accesses = withheld_accesses(EngineType::StarRocks)?;
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

/// The access type that lets a user store a query result as a materialized
/// view. Spelled as in `service-defs/starrocks.json`, where it is an access type
/// of the `database` resource.
pub(crate) const CREATE_MV_ACCESS: &str = "create materialized view";

/// The only catalog StarRocks creates a materialized view in.
pub const MV_CATALOG: &str = "default_catalog";

/// What follows [`COMPILER_CLEARANCE_PREFIX`] in the name of the A3 deny
/// (`perm-clearance-mv`). `crate::attribute_policy_name` reads it back.
pub(crate) const MV_DENY_SUFFIX: &str = "mv";

/// A3 (revised 2026-10-14): at most ONE deny per StarRocks profile. Denying
/// `create materialized view` to `{public}` keeps an uncleared user from
/// building a view that STORES real values (a mask does not follow stored data).
///
/// **Why engine-wide.** Measured on StarRocks 4.1.6
/// (`docs/en/plans/2026-10-14-starrocks-mv-placement-measurement.md`): a
/// materialized view can only be created in `default_catalog`, and the privilege
/// is checked on the TARGET database `default_catalog.<db>`, never on the base
/// table's catalog or database. The first draft's deny on `{<external catalog>,
/// <database>}` therefore matched no real `CREATE MATERIALIZED VIEW`, and the
/// target database is any one the builder may write to, so the resource is
/// `{catalog: default_catalog, database: *}`. The first draft's
/// `…mv-{catalog}.{database}` policies are swept by the compiler prefix.
///
/// The exceptions are the users cleared for EVERY classification on the
/// profile: the intersection of `cleared`, one entry per gated tag that
/// reaches a column or (D2, C7) a whole table of the profile. An empty
/// intersection leaves no exception, so the deny holds for everyone.
/// **Cost (accepted by the user):** a user not cleared for every classification
/// on the engine cannot create any materialized view there, including over
/// unclassified data.
///
/// A group-only gate is skipped before it contributes, so it neither creates
/// the deny nor narrows its exceptions (the same skip as the column deny). A
/// profile with no classification gets no policy.
pub(crate) fn mv_deny_inputs(
    cleared_sets: &[Vec<String>],
    service: &str,
) -> Vec<(String, AccessPolicyInput)> {
    let mut cleared: Option<Vec<String>> = None;
    for set in cleared_sets {
        cleared = Some(match cleared.take() {
            None => set.clone(),
            Some(have) => have.into_iter().filter(|u| set.contains(u)).collect(),
        });
    }
    let Some(cleared) = cleared else {
        return Vec::new();
    };
    let name = format!("{COMPILER_CLEARANCE_PREFIX}{MV_DENY_SUFFIX}");
    let mut resources = BTreeMap::new();
    resources.insert("catalog".to_string(), level(MV_CATALOG.to_string()));
    resources.insert("database".to_string(), level("*".to_string()));
    let input = clearance_input(
        name.clone(),
        service.to_string(),
        resources,
        &[CREATE_MV_ACCESS.to_string()],
        cleared,
    );
    vec![(name, input)]
}

/// The one clearance policy shape, built once for both engines: deny `{public}`
/// the withheld accesses on `resources`, except the enumerated cleared
/// usernames.
///
/// Shared deliberately. The two branches differ ONLY in which resource they
/// address and which service they are filed under; every other field is a
/// property of what a clearance IS, and three of them are load-bearing in ways a
/// second copy would be free to get wrong — see the inline notes.
fn clearance_input(
    name: String,
    service: String,
    resources: BTreeMap<String, ResourceLevel>,
    accesses: &[String],
    cleared: Vec<String>,
) -> AccessPolicyInput {
    AccessPolicyInput {
        name,
        service,
        is_enabled: true,
        resources,
        items: vec![AccessItemInput {
            users: Vec::new(),
            // The ONE expressible half of the pair: `{public}` matches
            // everybody even with no group provider (D0/D0b, measured — the
            // group check needs only a non-null `userGroups`, and
            // `setUserGroups` normalises `null` to an empty set).
            groups: vec![PUBLIC_GROUP.to_string()],
            // Deliberately empty. Naming the clearing role here — the obvious
            // implementation — is what D0 measured matches nobody, and D0b
            // re-derived the same answer for StarRocks: `setUserRoles` is never
            // called anywhere in `fe/`.
            roles: Vec::new(),
            accesses: accesses.to_vec(),
            effect: "deny".to_string(),
        }],
        allow_exceptions: Vec::new(),
        deny_exceptions: vec![AccessItemInput {
            users: cleared,
            groups: Vec::new(),
            roles: Vec::new(),
            accesses: accesses.to_vec(),
            effect: "deny".to_string(),
        }],
        // MUST stay false: `access_to_ranger_json` blanks BOTH
        // `denyPolicyItems` and `denyExceptions` when `isDenyAllElse` is set,
        // which would turn this policy into a deny nobody can be excepted from.
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        // **MUST stay false on BOTH engines, for two DIFFERENT reasons.** On
        // Trino an override is simply unnecessary — the tag-first short-circuit
        // already wins. On StarRocks it would be actively HARMFUL: with no tag
        // service, deny-beats-allow comes from Ranger sorting `hasDeny()`
        // evaluators first, and priority DOMINATES that comparator
        // (`PolicyEvalOrderComparator.compareNormal`) — so raising the priority
        // breaks the very ordering it looks like it strengthens (D0b).
        is_override: false,
        additional: Vec::new(),
    }
}

fn skip(g: &GateRow, reason: String) -> SkippedItem {
    tracing::warn!(
        tag_id = %g.tag_id,
        reason = %reason,
        "classification clearance skipped during recompile"
    );
    SkippedItem {
        source_kind: SourceKind::Clearance,
        id: g.tag_id,
        reason,
    }
}

/// Every access type a clearance withholds on `engine`.
///
/// **Read from the `Action` → access-type map rather than spelled out**, so a
/// clearance cannot drift from what the model can express (an access type the
/// plugin does not declare is silently ignored: the policy compiles, deploys,
/// and withholds nothing — `egret_perm::actions`' own warning).
///
/// **Trino gets `select`/`insert`/`delete`, and the live gate measured that the
/// write halves are INERT.** The intent was that a classification withholds the
/// DATA and not only the `SELECT` — an uncleared user who could still `INSERT`
/// into a classified column would be writing into data they are not cleared to
/// see. What actually happens (live case 5,
/// `docs/en/plans/2026-09-28-sp4c-live-result.md`) is that the uncleared subject
/// INSERTed **and** DELETEd through a live clearance: the compiled deny names a
/// *column* resource while Trino's write check presents a *table* one, so
/// `insert`/`delete` match nothing. **They are kept, but they are not
/// protection**, and this comment says so rather than arguing for them: leaving
/// the earlier justification standing is how the next reader rebuilds the wrong
/// model of what a clearance does. Buying the write half means a *table*-level
/// deny, which over-blocks every untagged column of that table — a user ruling,
/// not a compiler decision, and the ruling is reads-only.
///
/// **StarRocks: `select` ALONE, by user ruling (2026-09-28) — and it is NOT an
/// asymmetry with Trino, because neither engine withholds a write.** D0b
/// measured that `checkColumnAction` has exactly ONE caller and it passes
/// `PrivilegeType.SELECT` (`ColumnPrivilege.java:152`), and that egret's own
/// `starrocks.json` restricts the `column` resource to
/// `accessTypeRestrictions: ["select"]`. So the `insert`/`update`/`delete` this
/// function yields for Trino can never MATCH at column granularity here either.
/// Emitting them anyway would be decoration that reads as protection — a policy
/// listing four withheld accesses while withholding one — which is precisely the
/// failure mode this programme keeps refusing to ship, and it is the same
/// judgement the Trino paragraph above now records against Trino's own body.
/// **The consequence is real on BOTH engines: a user without clearance can still
/// overwrite a column they cannot read.** SP4c Task 5/7 says exactly that in the
/// UI.
pub(crate) fn withheld_accesses(engine: EngineType) -> Result<Vec<String>, CompileError> {
    let actions: &[Action] = match engine {
        EngineType::Trino => &[Action::Read, Action::Write],
        EngineType::StarRocks => &[Action::Read],
    };
    let mut out: Vec<String> = Vec::new();
    for action in actions {
        for acc in engine_accesses(*action, engine)? {
            if !out.iter().any(|a| a == acc) {
                out.push((*acc).to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pending(cleared: &[&str]) -> Vec<String> {
        cleared.iter().map(|s| s.to_string()).collect()
    }

    fn exceptions(i: &AccessPolicyInput) -> Vec<String> {
        i.deny_exceptions[0].users.clone()
    }

    /// A3 (revised 2026-10-14): StarRocks creates a materialized view only in
    /// `default_catalog` and checks the privilege on the TARGET database, so the
    /// deny is one policy on `default_catalog`/`*` whatever databases the
    /// classified columns live in, excepting the users cleared for every
    /// classification on the service.
    #[test]
    fn a_service_with_two_classifications_gets_one_engine_wide_deny_excepting_the_intersection() {
        let got = mv_deny_inputs(
            &[
                pending(&["ann", "bob"]),
                pending(&["ann", "bob"]),
                pending(&["bob", "cy"]),
            ],
            "sr",
        );
        assert_eq!(got.len(), 1, "{got:?}");
        let (name, i) = &got[0];
        assert_eq!(name, &format!("{COMPILER_CLEARANCE_PREFIX}mv"));
        assert_eq!(i.name, *name);
        assert_eq!(i.service, "sr");
        assert_eq!(i.resources.len(), 2);
        assert_eq!(i.resources["catalog"].values, vec!["default_catalog"]);
        assert_eq!(i.resources["database"].values, vec!["*"]);
        assert_eq!(i.items[0].groups, vec!["public"]);
        assert_eq!(i.items[0].accesses, vec!["create materialized view"]);
        assert_eq!(i.items[0].effect, "deny");
        assert_eq!(exceptions(i), vec!["bob"]);
    }

    #[test]
    fn a_service_with_no_classified_column_gets_no_deny() {
        assert!(mv_deny_inputs(&[], "sr").is_empty());
    }

    #[test]
    fn an_empty_intersection_denies_everyone() {
        let got = mv_deny_inputs(&[pending(&["ann"]), pending(&["bob"])], "sr");
        assert_eq!(got.len(), 1);
        assert!(exceptions(&got[0].1).is_empty());
        assert_eq!(got[0].1.items[0].groups, vec!["public"]);
    }

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
    fn glob_matches_ranger_stars_and_question_marks() {
        assert!(glob_matches("t*", "t1"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("a*b*c", "axxbyyc"));
        assert!(glob_matches("t?", "t1"));
        assert!(!glob_matches("t?", "t"));
        assert!(!glob_matches("t*", "x1"));
    }
}
