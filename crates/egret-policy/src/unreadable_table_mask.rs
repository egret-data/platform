//! **One rule, for every surface that can author half of it: a tag-addressed
//! MASK must not coexist with a TABLE-LEVEL attachment of the same tag.**
//! SP4d debt Task 5.
//!
//! # The measurement
//!
//! Live, on the Trino 470 pod, during SP4d-2b's live gate. A tag-addressed
//! `MASK` policy plus a tag attached to a whole TABLE (migration 0081's
//! `column_name = ''` sentinel) makes Trino type-check that one mask against
//! **every column of the table being scanned**, not against the columns the
//! query projects. The fixture table `sp4d1.t7` holds `item varchar` and
//! `id bigint`; with `tblmask` attached at table level and a `MASK` rule
//! addressed to `tblmask`, `SELECT item`, `SELECT id` and even `count(*)` all
//! fail. The table is **unreadable**, not masked.
//!
//! The obvious explanation was measured and **killed** in the same run: a clean
//! table-level MASK with **no row filter** fails identically, so the row filter
//! is not the cause. That is recorded here so nobody re-derives it.
//!
//! # Why this refusal is WIDER than that measurement, deliberately
//!
//! What failed is the *type check*: a `MASK` returns `varchar`, and `id` is a
//! `bigint`. A table whose every column is string-typed would type-check and
//! would genuinely be masked. So this guard refuses a configuration that, for
//! some tables, works.
//!
//! It is still the right rule at this boundary, and the reason is not caution —
//! it is that the narrower rule **is not computable here**. egret holds no
//! column types for a foreign catalog at authoring time; the tag attachment is
//! the only thing it knows about the table, and the user's ruling is that the
//! refusal happens when the shape is created, not after the fact. Narrowing by
//! `mask_type` instead (letting `MASK_NULL` through, say, on the theory that
//! `NULL` coerces to any type) would be a guess about an engine's coercion
//! rules that nobody has measured, and the one thing worse than a stated
//! over-refusal is an unmeasured under-refusal. The remedy the message names —
//! attach the tag to the columns — is never wrong, and never changes what any
//! other column sees.
//!
//! This same asymmetry is why there is **no migration** for this rule; see
//! "Why no CHECK, and no trigger either" below.
//!
//! # Every write path that can produce the pair
//!
//! Three surfaces author the MASK half or the ATTACHMENT half, and **either
//! half can be created second** — a guard keyed to one creation order leaves
//! the hole open, which is a defect this programme has produced four times. So
//! the rule lives in ONE function, [`unreadable_table_mask_for`], whose single
//! `match` has one arm per half; a surface cannot implement a different rule
//! because there is only one rule, and both arms build the same
//! [`UnreadableTableMask`].
//!
//! * **The MASK half, via the permission model.** `POST`/`PATCH
//!   /api/control-plane/perm/rules` → `egret_perm_compile::routes::write_rule`,
//!   a tag selector with `kind = mask`. `POST /perm/preview` funnels through the
//!   same function and so reports the refusal before the steward presses Save.
//! * **The MASK half, via the policy editor.** `POST
//!   /api/control-plane/policies` with `kind: "tag-masking"` →
//!   `egret_policy::admin::create_policy`'s tag branch, which writes a
//!   `policyType: 1` body under the profile's tag service. `PUT /policies/{id}`
//!   cannot reach this shape (`editor_to_ranger_json` rejects every `tag-*`
//!   kind with "unknown policy kind"), and `PUT /policies/{id}/enabled` cannot
//!   either, because this guard ignores `isEnabled` — a tag mask that would
//!   pair is refused at CREATE whether it is enabled or not, so there is never
//!   a disabled one waiting to be switched on.
//! * **The ATTACHMENT half.** `PUT
//!   /api/control-plane/warehouses/{id}/resource-tags` with `column` absent or
//!   `""` → `egret_bi::CatalogTagRepo::assign_catalog`. That is the only
//!   `INSERT` into `catalog_tag_assignments` in any `src/`.
//!
//! Stated BY CODE PATH and not by authorship, because five such claims in this
//! programme have been wrong while the behaviour claim beside them was right.
//!
//! **What is NOT in the class, measured rather than assumed.** A tag-addressed
//! ROW FILTER is table-granular on both engines by construction (Trino's tag
//! row-filter policy carries no path, and `column_resources` stops at `table`),
//! so a table-level attachment is its natural match, not a hazard. A tag
//! ACCESS policy — which is what a clearance compiles to
//! (`COMPILER_CLEARANCE_PREFIX`, `policyType: 0`) — denies rather than masks and
//! type-checks nothing. Neither is refused, and
//! `unreadable_table_mask_db.rs` pins both as accept-controls.
//!
//! # Why no CHECK, and no trigger either
//!
//! Tasks 2, 3, 12 and 14 each shipped the guard in the schema AND in Rust, and
//! each measured that neither was redundant. **This pair cannot take that
//! shape**, and the reasons are specific rather than a shrug:
//!
//! 1. **A single-row `CHECK` cannot express it.** The pair spans two tables in
//!    the `perm.rules` direction (`perm.rules` ⋈ `tags` ⋈
//!    `catalog_tag_assignments`) and three in the editor direction
//!    (`policy.authz_policies` ⋈ `policy.authz_services` ⋈ `tags` ⋈
//!    `catalog_tag_assignments`). A `CHECK` sees one row of one table.
//! 2. **A trigger is not a constraint.** The only schema-level alternative is a
//!    row trigger that queries the other table, and it would evaluate that
//!    query under its own transaction's snapshot — so two concurrent
//!    transactions each writing one half would both pass and the pair would
//!    commit. That is the SAME residual window this Rust guard has (see
//!    "Concurrency" below), so the trigger buys reach over non-route writers
//!    and **no atomicity at all**. It is not the instrument 0166/0167/0168 are.
//! 3. **A trigger on `policy.authz_policies` would wedge an installation that
//!    already holds the pair, measured.** Of the 44 databases on this
//!    development host, 35 carry both schemas (9 carry neither; 0 connection
//!    errors). Across those 35: **1** tag-addressed mask RULE, **4**
//!    hand-authored tag MASK policies, **4** table-level attachments — and
//!    **4 live instances of the dangerous pair**, all four in `egret_sp4d2b`,
//!    all four from the policy EDITOR (`tp-mask-pii`/`sp4d1.t4`,
//!    `tp-mask-tblmask`/`sp4d1.t7`, `tp-mask-tblwide`/`sp4d1.t6`,
//!    `ui-mask-pii`/`sp4d1.t4`), none from `perm.rules`. `policy.authz_policies`
//!    is also written by the compiler's own `apply_diff_in_tx` on every
//!    recompile, so a trigger there would make the recompile that REMOVES a
//!    pair fail too — fail-closed with no product remedy, which is the opposite
//!    of what 0166's operator note promises.
//! 4. **And the rule is wider than the defect** (see above), so it is the kind
//!    of conservative refusal that belongs at an authoring boundary and not in
//!    a static predicate that would make a working all-`varchar` configuration
//!    unrepresentable for every writer forever.
//!
//! The consequence, stated plainly because it is the cost of this ruling: a
//! writer that is not one of the three routes above — `psql`, a route added
//! later — can still create the pair. `egret_policy::admin`'s own
//! `refuse_if_generated` has the same shape of reach and the same limit.
//!
//! # Concurrency
//!
//! Both model paths already hold the `policy.authz_services` row locks their
//! write will recompile (`lock_services_in_tx`), and a tag-addressed rule's
//! affected set is every compiler-targeted profile in the installation, so in
//! any deployment where the tagged warehouse's engine binds a profile the two
//! halves serialize on that row and the second one sees the first. Where that
//! intersection is empty — no bound services at all — two concurrent
//! transactions can each write one half and both pass. Nothing is being
//! enforced in that state either (there is no profile to download a policy),
//! and the next write to either half through any of the three routes refuses,
//! so the window is narrow and loud rather than silent. It is not closed, and a
//! trigger would not close it (point 2 above).

use uuid::Uuid;

/// Which surface authored the MASK half — the noun the refusal uses, and the
/// thing the steward has to go and edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskOrigin {
    /// A `perm.rules` row with a tag selector and `kind = mask`.
    Rule,
    /// A hand-authored `policyType: 1` policy under a tag service.
    Policy,
}

impl MaskOrigin {
    /// The noun the steward reads. `perm.rules` rows are "mask rule"s in the
    /// Governance UI; editor policies are "tag masking policy"s in the Policies
    /// console.
    fn noun(self) -> &'static str {
        match self {
            MaskOrigin::Rule => "mask rule",
            MaskOrigin::Policy => "tag masking policy",
        }
    }
}

/// One instance of the refused pair: the mask, the tag, and the table the tag
/// is attached to whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableTableMask {
    pub mask_origin: MaskOrigin,
    /// `perm.rules.name` or `policy.authz_policies.name`.
    pub mask: String,
    /// `tags.name` — what the engine matches a tag policy on, and what the
    /// steward sees in the tag picker.
    pub tag: String,
    pub schema: String,
    pub table: String,
}

impl UnreadableTableMask {
    /// **The refusal, and it is the same sentence from every write path** —
    /// one message, so the two directions cannot describe the same state
    /// differently.
    ///
    /// It leads with the REMEDY's subject and names both remedies: attach the
    /// tag to the columns, or address the mask by path instead of by tag. The
    /// consequence gets one clause, because a steward who is told only "this
    /// is refused" files a bug, and one who is told only the mechanism still
    /// does not know which box to change.
    ///
    /// Surfaced verbatim by all three UIs, checked rather than assumed:
    /// `RuleDialog` renders `(e as ApiError).message` into `saveError` and
    /// `previewError` (`frontend/src/routes/governance/RuleDialog.tsx`),
    /// `CatalogTagEditor` renders `Could not add that tag: {assign.error.message}`
    /// (`frontend/src/components/access/CatalogTagEditor.tsx`), and the Policies
    /// console falls through `refusalMessage` to `err.message` for anything that
    /// is not a 403 or the compiler-owned 409
    /// (`frontend/src/routes/governance/policyWrites.ts`).
    pub fn refusal(&self) -> String {
        let Self {
            mask_origin,
            mask,
            tag,
            schema,
            table,
        } = self;
        let noun = mask_origin.noun();
        format!(
            "tag {tag:?} is attached to the whole table {schema}.{table} (no column), and {noun} \
             {mask:?} masks that same tag. The engine type-checks a tag-addressed mask against \
             EVERY column of the table being read, so together these make {schema}.{table} \
             unreadable rather than masked — selecting any column, and even count(*), fails. \
             Attach {tag:?} to the columns that need masking instead of to the table, or address \
             {mask:?} by catalog/schema/table/column path instead of by tag."
        )
    }
}

/// The half of the pair a caller is about to write. One variant per half,
/// because **either half can be created second**.
#[derive(Debug, Clone, Copy)]
pub enum TagWrite<'a> {
    /// A tag-addressed MASK is being authored. The caller supplies the tag the
    /// way its own table names it and the name the refusal should use.
    Mask {
        tag: TagRef<'a>,
        origin: MaskOrigin,
        /// The mask's own name, for the message.
        name: &'a str,
    },
    /// A tag is being attached to a whole TABLE — migration 0081's
    /// `column_name = ''`. **The sentinel itself is legal and stays legal**
    /// (0166 keeps it so on purpose); what is refused is the PAIR.
    TableAttachment {
        tag_id: Uuid,
        schema: &'a str,
        table: &'a str,
    },
}

/// How the caller's own table names a tag. `perm.rules.tag_key` references
/// `tags.key`; a tag policy's `resources.tag.values` carries `tags.name` (D6 —
/// the key is stable but is not what the Ranger tag plugin matches on).
#[derive(Debug, Clone, Copy)]
pub enum TagRef<'a> {
    Key(&'a str),
    Name(&'a str),
}

/// **The whole rule.** `Ok(Some(_))` means the write would complete the refused
/// pair; the caller turns it into its own 400 carrying
/// [`UnreadableTableMask::refusal`].
///
/// Runs on the caller's connection or transaction, so a route that refuses has
/// written nothing: each arm queries only the table it is NOT writing, which is
/// what lets every caller check BEFORE its own `INSERT` and keeps the refusal a
/// plain 400 instead of a rollback.
///
/// Each half's shape is spelled exactly once — `column_name = ''` appears in one
/// SQL string, "a tag-addressed mask" in the other — and the conjunction of the
/// two is this function's `match`. That is the factoring the four-times defect
/// in this programme calls for: not "two guards keyed to the same shape", but
/// one rule with two entry points.
pub async fn unreadable_table_mask_for(
    conn: &mut sqlx::PgConnection,
    write: TagWrite<'_>,
) -> Result<Option<UnreadableTableMask>, sqlx::Error> {
    match write {
        TagWrite::Mask { tag, origin, name } => {
            let (key, tag_name) = match tag {
                TagRef::Key(k) => (Some(k), None),
                TagRef::Name(n) => (None, Some(n)),
            };
            // THE ATTACHMENT HALF. `column_name = ''` is migration 0081's
            // table-level sentinel and 0166 deliberately keeps it storable; this
            // is the one place that spelling appears.
            let row: Option<(String, String, String)> = sqlx::query_as(
                "SELECT t.name, a.schema_name, a.table_name \
                   FROM catalog_tag_assignments a \
                   JOIN tags t ON t.id = a.tag_id \
                  WHERE a.column_name = '' \
                    AND ($1::text IS NULL OR t.key = $1) \
                    AND ($2::text IS NULL OR t.name = $2) \
                  ORDER BY a.schema_name, a.table_name \
                  LIMIT 1",
            )
            .bind(key)
            .bind(tag_name)
            .fetch_optional(&mut *conn)
            .await?;
            Ok(row.map(|(tag, schema, table)| UnreadableTableMask {
                mask_origin: origin,
                mask: name.to_string(),
                tag,
                schema,
                table,
            }))
        }
        TagWrite::TableAttachment {
            tag_id,
            schema,
            table,
        } => {
            // THE MASK HALF, both surfaces, in one statement. The `ORDER BY 1`
            // puts a `perm.rules` row ahead of an editor policy, which matters
            // for the message rather than for the verdict: a rule's own compiled
            // artifact IS one of the `policyType = 1` tag policies this second
            // branch finds, and naming the RULE is what points the steward at the
            // thing they can edit.
            let row: Option<(i32, String, String)> = sqlx::query_as(
                "SELECT 0 AS origin, r.name, t.name \
                   FROM perm.rules r \
                   JOIN tags t ON t.key = r.tag_key \
                  WHERE t.id = $1 \
                    AND r.selector_kind::text = 'tag' \
                    AND r.kind::text = 'mask' \
                 UNION ALL \
                 SELECT 1, p.name, t.name \
                   FROM policy.authz_policies p \
                   JOIN policy.authz_services s \
                     ON s.id = p.service_id AND s.type = 'tag' \
                   JOIN tags t ON t.id = $1 \
                  WHERE (p.policy_jsonb ->> 'policyType') = '1' \
                    AND (p.policy_jsonb -> 'resources' -> 'tag' -> 'values') \
                        @> to_jsonb(t.name) \
                  ORDER BY 1, 2 \
                  LIMIT 1",
            )
            .bind(tag_id)
            .fetch_optional(&mut *conn)
            .await?;
            Ok(row.map(|(origin, mask, tag)| UnreadableTableMask {
                mask_origin: if origin == 0 {
                    MaskOrigin::Rule
                } else {
                    MaskOrigin::Policy
                },
                mask,
                tag,
                schema: schema.to_string(),
                table: table.to_string(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal must carry **both** remedies and the table, from either
    /// direction — the steward needs to know which box to change, and there are
    /// two boxes.
    ///
    /// Asserted by substring and not against a golden string: a golden copy of
    /// the sentence is a second place to edit it, and the thing that must not
    /// regress is the presence of the two remedies, not the wording around them.
    #[test]
    fn the_refusal_names_the_table_and_both_remedies() {
        for (origin, noun) in [
            (MaskOrigin::Rule, "mask rule"),
            (MaskOrigin::Policy, "tag masking policy"),
        ] {
            let msg = UnreadableTableMask {
                mask_origin: origin,
                mask: "mask-pii".into(),
                tag: "PII".into(),
                schema: "sales".into(),
                table: "orders".into(),
            }
            .refusal();
            assert!(msg.contains("sales.orders"), "names the table: {msg}");
            assert!(msg.contains(noun), "names what to edit: {msg}");
            assert!(msg.contains("\"mask-pii\""), "names the mask: {msg}");
            assert!(msg.contains("\"PII\""), "names the tag: {msg}");
            // Remedy 1: move the attachment down to the columns.
            assert!(
                msg.contains("to the columns that need masking instead of to the table"),
                "names the attach-to-column remedy: {msg}"
            );
            // Remedy 2: stop addressing the mask by tag.
            assert!(
                msg.contains("by catalog/schema/table/column path instead of by tag"),
                "names the resource-addressed remedy: {msg}"
            );
        }
    }
}
