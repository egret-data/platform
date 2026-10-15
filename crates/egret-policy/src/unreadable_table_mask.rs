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
//! the hole open, which is a defect this programme has produced four times. A
//! fourth surface writes NEITHER half and completes the pair anyway, by moving
//! the name the two are joined on (the RENAME, below). So the rule lives in ONE
//! function, [`unreadable_table_mask_for`], whose `match` has one arm per write
//! that can complete the pair, and the two half-predicates
//! (`table_level_attachment_of`, `tag_mask_naming`) are each spelled exactly
//! once and shared between the arms. A surface cannot implement a different
//! rule because there is only one rule, and every arm builds the same
//! [`UnreadableTableMask`].
//!
//! * **The MASK half, via the permission model.** `POST`/`PATCH
//!   /api/control-plane/perm/rules` → `egret_perm_compile::routes::write_rule`,
//!   a tag selector with `kind = mask`. `POST /perm/preview` funnels through the
//!   same function and so reports the refusal before the steward presses Save.
//! * **The MASK half, via the policy editor.** `POST
//!   /api/control-plane/policies` with `kind: "tag-masking"` →
//!   `egret_policy::admin::create_policy`'s tag branch, which writes a
//!   `policyType: 1` body under the profile's tag service.
//!
//!   **This entry's ruling CHANGED DIRECTION in SP4d debt Task 16, and the
//!   change is recorded here rather than silently applied.** Task 5 wrote that
//!   `PUT /policies/{id}` "cannot reach this shape (`editor_to_ranger_json`
//!   rejects every `tag-*` kind with 'unknown policy kind')". That was true and
//!   it was true *because of a defect*: the handler had no `is_tag_kind` branch,
//!   so an editor-authored tag mask could be created and deleted but never
//!   edited. Task 16 added the branch, so the `PUT` **can** reach the shape and
//!   is guarded — through `guard_tag_policy_authoring`, the one function both
//!   authoring verbs run. A write-path enumeration that rests on something being
//!   unimplemented has to be re-ruled when it gets implemented; this is that
//!   re-ruling.
//!
//!   `PUT /policies/{id}/enabled` still cannot reach it, for a reason that does
//!   not depend on any missing branch: this guard ignores `isEnabled`, so a tag
//!   mask that would pair is refused at authoring time whether it is enabled or
//!   not, and there is never a disabled one waiting to be switched on.
//! * **The ATTACHMENT half.** `PUT
//!   /api/control-plane/warehouses/{id}/resource-tags` with `column` absent or
//!   `""` → `egret_bi::CatalogTagRepo::assign_catalog`. That is the only
//!   `INSERT` into `catalog_tag_assignments` in any `src/`.
//! * **NEITHER half — the JOIN.** `PATCH /api/control-plane/tags/{id}` →
//!   `egret_bi::TagRepo::update`, the only production `UPDATE tags SET name`
//!   in any `src/`. An editor tag policy carries the tag NAME as a plain
//!   string that nothing resolves at authoring time, so a mask naming a tag
//!   that does not exist yet is legal, a table-level attachment of a
//!   differently-named tag is legal, and renaming that tag INTO the masked
//!   name completes the pair through neither guarded half. **SP4d debt Task
//!   15**, confirmed before being fixed: the sequence was driven through all
//!   three real routes and the resulting pair was read back out of the Ranger
//!   plugin's OWN downloads — the tag map carried the masked name on the whole
//!   table and `tagPolicies` carried the mask — in
//!   `egret-bi/tests/tag_rename_unreadable_pair.rs`. Renaming a tag OUT of a
//!   masked name is deliberately still allowed; see [`TagWrite::Rename`] for
//!   why that is the steward's only escape from a pair that already exists.
//!
//! Stated BY CODE PATH and not by authorship, because five such claims in this
//! programme have been wrong while the behaviour claim beside them was right —
//! and Task 5's own enumeration, written that way, still missed the rename,
//! which is why this list says what each path WRITES rather than who uses it.
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
//!    and **no atomicity at all**. It is not the instrument 0177/0178/0179 are.
//! 3. **A trigger on `policy.authz_policies` would wedge the recompile that
//!    REMOVES a pair.** `apply_diff_in_tx` writes that table on every recompile,
//!    so a trigger refusing the pair would refuse the write that CLEARS it:
//!    fail-closed with no product remedy, the opposite of what `0177`'s
//!    operator note promises. This leg is a property of the code and stays
//!    checkable by reading `apply_diff_in_tx`; it does not rest on any sweep.
//!
//!    The second leg — "an installation already holds the pair" — was a SWEEP,
//!    and a sweep is a statement about a moment. As measured during SP4d-2b's
//!    live gate it found four live instances, all from the policy EDITOR and
//!    none from `perm.rules`, all four in the `egret_sp4d2b` scratch database.
//!    **That measurement no longer reproduces: `egret_sp4d2b` was dropped
//!    between sessions.** Re-measured 2026-10-10 with this module's own
//!    predicates: of the 10 databases on this development host (`template0` and
//!    `template1` excluded), 9 carry both `policy.authz_policies` and
//!    `perm.rules` and one (`postgres`) carries neither, 0 connection errors;
//!    across those 9 there are **0** tag-addressed mask rules, **0**
//!    hand-authored tag mask policies, **0** table-level attachments and **0**
//!    live pairs. So the count is gone and only the direction survives: the
//!    pair HAS occurred on a real host, and the surface that produced it was
//!    the editor rather than `perm.rules` — which is why guarding only
//!    `POST /perm/rules` would have left the reachable half open. Nothing here
//!    needs the figure to be four; a trigger that can wedge an installation
//!    that holds the pair is unacceptable whether such installations number
//!    four or one.
//! 4. **And the rule is wider than the defect** (see above), so it is the kind
//!    of conservative refusal that belongs at an authoring boundary and not in
//!    a static predicate that would make a working all-`varchar` configuration
//!    unrepresentable for every writer forever.
//!
//! The consequence, stated plainly because it is the cost of this ruling: a
//! writer that is not one of the four routes above — `psql`, a route added
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
//! and the next write to either half through any of the four routes refuses,
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
    /// It leads with the REMEDY's subject and names the remedies: attach the tag
    /// to the columns, address the mask by path instead of by tag, or — **added
    /// in SP4d debt Task 16** — edit the existing policy so it addresses the
    /// table by path. The consequence gets one clause, because a steward who is
    /// told only "this is refused" files a bug, and one who is told only the
    /// mechanism still does not know which box to change.
    ///
    /// # Why the third remedy was missing, and why it is the best one
    ///
    /// Task 5 named two remedies, and the omission was not an oversight about
    /// wording: for an **editor-origin** mask the obvious third remedy did not
    /// exist. `PUT /policies/{id}` had no `is_tag_kind` branch, so the only way
    /// to change an existing tag mask was to DELETE and recreate it, and a
    /// message that said so would have documented a defect as a workflow. Task
    /// 16 added the branch; the sentence changes with it, in this ONE function,
    /// so every write path's refusal gains the remedy together — a clause added
    /// in a caller would have given the perm-model surface and the editor
    /// surface two different accounts of the same state.
    ///
    /// It is the best remedy of the three for an existing pair because it is the
    /// only one that changes nothing else: attaching the tag to columns instead
    /// alters what every OTHER policy keyed on that tag selects, and renaming
    /// the tag out of the masked name (the escape [`TagWrite::Rename`] keeps
    /// open) moves every attachment of it at once.
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
             Attach {tag:?} to the columns that need masking instead of to the table, address \
             {mask:?} by catalog/schema/table/column path instead of by tag, or edit {mask:?} \
             so it addresses {schema}.{table} by path."
        )
    }
}

/// The write a caller is about to make. One variant per half of the pair,
/// because **either half can be created second** — plus the RENAME, which
/// writes neither half and completes the pair anyway by moving the name the two
/// are joined on.
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
    /// A tag is being attached to a catalog resource.
    ///
    /// **The caller passes its `column` verbatim and makes no judgement about
    /// it**, because only a TABLE-level attachment (migration 0081's
    /// `column_name = ''`) is half of this pair and that decision belongs
    /// beside the SQL that spells the same thing `= ''`. A guard whose caller
    /// decided "this one is table-level" and whose query decided it again is
    /// two guards that must agree, written to different shapes — the defect
    /// this programme has produced four times. **The sentinel itself is legal
    /// and stays legal** (0177 keeps it so on purpose); what is refused is the
    /// PAIR.
    Attachment {
        tag_id: Uuid,
        schema: &'a str,
        table: &'a str,
        /// `""` ⇒ the whole table. Anything else is a column attachment, which
        /// is the ordinary working configuration and never refused.
        column: &'a str,
    },
    /// A tag is being RENAMED — `egret_bi::TagRepo::update`, behind `PATCH
    /// /api/control-plane/tags/{id}`.
    ///
    /// **This is a THIRD authoring surface and neither of the other two sits on
    /// it** (SP4d debt Task 15). An editor tag policy stores the tag NAME as a
    /// plain string and nothing resolves it at authoring time, so: a mask
    /// addressing a name no tag carries yet is legal, a table-level attachment
    /// of some other tag is legal, and renaming that tag INTO the masked name
    /// completes the pair having passed through neither guard. Measured end to
    /// end through the real routes, down to what the Ranger plugin downloads,
    /// in `egret-bi/tests/tag_rename_unreadable_pair.rs`.
    ///
    /// **Renaming OUT of a masked name stays allowed, and that is a ruling
    /// rather than an omission.** The engine matches a tag policy on the name
    /// too, so moving the name away genuinely removes the pair (Task 5's
    /// concern 5, which judged that direction correctly). It is a product remedy
    /// for a pair that already exists — one a `psql` writer or a pre-guard
    /// installation left behind — so refusing it would trap the steward instead
    /// of protecting them. The same reasoning 0177 records for `DELETE
    /// …/resource-tags`: removing half the pair is the remedy, never the hazard.
    ///
    /// Task 5 called it the **only** product remedy, and **that is no longer
    /// true**: SP4d debt Task 16 gave `PUT /policies/{id}` a tag branch, so an
    /// editor-origin mask can now be edited to address its table by path —
    /// which the refusal now names as its third remedy, and which is a better
    /// one here because it touches one policy instead of every attachment of the
    /// tag. The rename is still the only remedy for a pair whose mask half is
    /// compiler-owned, where the editor cannot write at all. Task 5's "four such
    /// pairs exist on this development host" is also no longer checkable: the
    /// database holding them (`egret_sp4d2b`) was dropped between sessions — a
    /// sweep is a statement about a moment.
    ///
    /// The refusal reads the same sentence as the other two surfaces, naming
    /// the NEW name, because that is the state the write would land on and the
    /// string the steward just typed. A rename-specific prefix was considered
    /// and left out: one sentence from every path is what stops the three
    /// [`TagWrite`] WRITE PATHS describing the same state differently. (Not
    /// "the three directions": in this module a *direction* is one of the two
    /// MASK ORIGINS — `perm.rules` versus the editor, the sense used at
    /// [`MaskOrigin`] and in the module doc's `CHECK` argument — and those
    /// number two, not three. The surfaces number three here and four in the
    /// module doc, which counts the two mask origins separately; both are true
    /// of different things, so this says which one it means.)
    Rename {
        tag_id: Uuid,
        /// The name the tag is being renamed TO.
        new_name: &'a str,
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

/// How the two half-predicates below locate the tag: a `perm.rules` row names
/// it by `tags.key`, a tag policy by `tags.name` (D6) — and a RENAME by id,
/// because the name is the thing in flight and so identifies nothing.
#[derive(Debug, Clone, Copy)]
enum TagBy<'a> {
    Key(&'a str),
    Name(&'a str),
    Id(Uuid),
}

impl<'a> From<TagRef<'a>> for TagBy<'a> {
    fn from(t: TagRef<'a>) -> Self {
        match t {
            TagRef::Key(k) => TagBy::Key(k),
            TagRef::Name(n) => TagBy::Name(n),
        }
    }
}

/// **THE ATTACHMENT HALF, spelled once.** Is this tag attached to a whole
/// TABLE, and if so which one?
///
/// `column_name = ''` is migration 0081's table-level sentinel (which 0177
/// deliberately keeps storable — the sentinel is legal, the PAIR is not), and
/// **this is the one place that spelling appears**: the Mask arm asks the
/// question by key or by name, the Rename arm asks it by id, and the three
/// reach the same predicate rather than a copy of it. Task 5's own mutation M4
/// caught a second spelling of "table-level" inside its first fix (commit
/// `95c15014`), which is why this is a function and not three queries.
///
/// Returns `(tags.name, schema, table)`. Exactly one of the three bindings is
/// non-NULL by construction of [`TagBy`]; the `IS NULL OR` shape is what lets
/// one statement serve all three.
async fn table_level_attachment_of(
    conn: &mut sqlx::PgConnection,
    tag: TagBy<'_>,
) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let (key, name, id) = match tag {
        TagBy::Key(k) => (Some(k), None, None),
        TagBy::Name(n) => (None, Some(n), None),
        TagBy::Id(i) => (None, None, Some(i)),
    };
    sqlx::query_as(
        "SELECT t.name, a.schema_name, a.table_name \
           FROM catalog_tag_assignments a \
           JOIN tags t ON t.id = a.tag_id \
          WHERE a.column_name = '' \
            AND ($1::text IS NULL OR t.key = $1) \
            AND ($2::text IS NULL OR t.name = $2) \
            AND ($3::uuid IS NULL OR t.id = $3) \
          ORDER BY a.schema_name, a.table_name \
          LIMIT 1",
    )
    .bind(key)
    .bind(name)
    .bind(id)
    .fetch_optional(conn)
    .await
}

/// **THE MASK HALF, spelled once, both mask surfaces in one statement.** Does
/// any tag-addressed MASK name this tag?
///
/// `rules_tag` is the tag id a `perm.rules` row could select, or **`None` when
/// a `perm.rules` row cannot pair with the write being checked at all**. That
/// is the rename case, and it is a ruling rather than an optimisation:
/// `perm.rules.tag_key` references `tags.key` (migration 0154) and a rename
/// writes `name` only, so a rename can neither create nor remove the
/// rules-direction pair. Asking anyway would refuse the rename of a tag that is
/// ALREADY in that pair — a steward with no way out, which is the opposite of a
/// guard. A NULL `$1` matches no `perm.rules` row, which is exactly that and is
/// deliberate, not incidental.
///
/// **"A tag service" is `type = 'tag'` OR named by another profile's
/// `tagService`** (live finding I5's class). The admin API creates only `trino`
/// and `starrocks` profiles, so a hand-made tag service is a `trino` row; its
/// policies are still served to the linker's engine as `tagPolicies`, and a tag
/// mask there makes a table-attached tag's table unreadable exactly as one in a
/// `tag` row does. A `trino` profile nobody names stays out — its
/// `policyType = 1` rows are path masks (see the accept-control
/// `a_masking_policy_on_a_resource_profile_is_accepted`).
///
/// The `ORDER BY 1, 2` puts a `perm.rules` row ahead of an editor policy, which
/// matters for the message rather than for the verdict: a rule's own compiled
/// artifact IS one of the `policyType = 1` tag policies the second branch
/// finds, and naming the RULE is what points the steward at the thing they can
/// edit.
async fn tag_mask_naming(
    conn: &mut sqlx::PgConnection,
    rules_tag: Option<Uuid>,
    tag_name: &str,
) -> Result<Option<(MaskOrigin, String)>, sqlx::Error> {
    let row: Option<(i32, String)> = sqlx::query_as(
        "SELECT 0 AS origin, r.name \
           FROM perm.rules r \
           JOIN tags t ON t.key = r.tag_key \
          WHERE t.id = $1 \
            AND r.selector_kind::text = 'tag' \
            AND r.kind::text = 'mask' \
         UNION ALL \
         SELECT 1, p.name \
           FROM policy.authz_policies p \
           JOIN policy.authz_services s \
             ON s.id = p.service_id \
            AND (s.type = 'tag' \
                 OR EXISTS (SELECT 1 FROM policy.authz_services l \
                             WHERE l.config_jsonb ->> 'tagService' = s.name \
                               AND l.id <> s.id)) \
          WHERE (p.policy_jsonb ->> 'policyType') = '1' \
            AND (p.policy_jsonb -> 'resources' -> 'tag' -> 'values') \
                @> to_jsonb($2::text) \
          ORDER BY 1, 2 \
          LIMIT 1",
    )
    .bind(rules_tag)
    .bind(tag_name)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(origin, mask)| {
        let origin = if origin == 0 {
            MaskOrigin::Rule
        } else {
            MaskOrigin::Policy
        };
        (origin, mask)
    }))
}

/// `tags.name` for an id — the string a tag POLICY addresses (D6). Read on the
/// caller's own connection, so an attachment write checks against the state its
/// own transaction will land on.
async fn tag_name_of(
    conn: &mut sqlx::PgConnection,
    tag_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(tag_id)
        .fetch_optional(conn)
        .await
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
        TagWrite::Mask { tag, origin, name } => Ok(table_level_attachment_of(conn, tag.into())
            .await?
            .map(|(tag, schema, table)| UnreadableTableMask {
                mask_origin: origin,
                mask: name.to_string(),
                tag,
                schema,
                table,
            })),
        TagWrite::Attachment {
            tag_id,
            schema,
            table,
            column,
        } => {
            // Only the TABLE-level attachment is half of the pair. This is the
            // one place that decision is made — see `TagWrite::Attachment`.
            if !column.is_empty() {
                return Ok(None);
            }
            // The tag's own NAME is what an editor tag policy addresses. A tag
            // id with no row pairs with nothing, and the `INSERT` this guard
            // runs before would fail its foreign key anyway.
            let Some(tag_name) = tag_name_of(&mut *conn, tag_id).await? else {
                return Ok(None);
            };
            Ok(tag_mask_naming(conn, Some(tag_id), &tag_name)
                .await?
                .map(|(mask_origin, mask)| UnreadableTableMask {
                    mask_origin,
                    mask,
                    tag: tag_name,
                    schema: schema.to_string(),
                    table: table.to_string(),
                }))
        }
        TagWrite::Rename { tag_id, new_name } => {
            // THE ATTACHMENT HALF, asked about the tag being renamed — the
            // SAME predicate the Mask arm asks, located by id because the name
            // is the thing in flight. A tag with no table-level attachment
            // cannot be half of this pair whatever it is called.
            let Some((_old_name, schema, table)) =
                table_level_attachment_of(&mut *conn, TagBy::Id(tag_id)).await?
            else {
                return Ok(None);
            };
            // THE MASK HALF, asked about the name the tag is ABOUT to have —
            // which is the whole difference between this arm and the other two.
            // `None` for the rules side: see `tag_mask_naming`, and note that
            // this is what keeps renaming a tag that is ALREADY in the
            // rules-direction pair possible.
            Ok(tag_mask_naming(conn, None, new_name)
                .await?
                .map(|(mask_origin, mask)| UnreadableTableMask {
                    mask_origin,
                    mask,
                    // The NEW name: it is what the engine would match the
                    // policy on the moment the rename commits.
                    tag: new_name.to_string(),
                    schema,
                    table,
                }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal must carry **all three** remedies and the table, from either
    /// direction — the steward needs to know which box to change, and there are
    /// three boxes.
    ///
    /// **This assertion CHANGED DIRECTION in SP4d debt Task 16, deliberately,
    /// and the change is stated here at the site.** It was
    /// `the_refusal_names_the_table_and_both_remedies` and it asserted exactly
    /// two remedies. The third ("edit the policy so it addresses the table by
    /// path") did not exist as a remedy while `PUT /policies/{id}` had no
    /// `is_tag_kind` branch — the only way to change an editor-authored tag mask
    /// was delete-and-recreate. Task 16 added the branch, so a two-remedy
    /// assertion would now be pinning a worse message as the desired one.
    ///
    /// Asserted by substring and not against a golden string: a golden copy of
    /// the sentence is a second place to edit it, and the thing that must not
    /// regress is the presence of the three remedies, not the wording around
    /// them.
    #[test]
    fn the_refusal_names_the_table_and_all_three_remedies() {
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
            // Remedy 3 (Task 16): edit the mask that already exists, now that
            // `PUT /policies/{id}` can edit a tag policy at all.
            assert!(
                msg.contains("so it addresses sales.orders by path"),
                "names the edit-in-place remedy, and names the table in it: {msg}"
            );
        }
    }
}
