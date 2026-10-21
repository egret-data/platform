# Classification area: usability fixes (Package 1)

Status: design, 2026-10-21. Not implemented. Plan: [`../plans/2026-10-21-classification-ux.md`](../plans/2026-10-21-classification-ux.md). Vietnamese mirror: [`../../vi/specs/2026-10-21-classification-ux-design.md`](../../vi/specs/2026-10-21-classification-ux-design.md).
Programme: Control Plane & DG, follows #4b Classification (#702). Scope decisions were made by the controller before this spec; this document implements them and flags only what the code makes impossible (§12).

## 1. Problem

The user finds Governance > Classification "quite hard to use". Two browser walkthroughs on the demo stack (2026-10-21) measured why. Finding ids below are theirs: **F1..F8** (pass 1) and **N1..N26** (pass 2; pass 2 corrects F4, F5 and F8).

The job tried in pass 2 was: *mask the `email` column of `demo_warehouse.analytics.customers` for everyone except group `data-team`, with a new classification `uxwalk-pii`*. Result:

- **Not completed verifiably** (N1): no engine serves the demo catalogs, and Explain does not report masks on Trino.
- **36 clicks** for the job, plus 15 for a detour that the UI caused (N2, N3): 51 in all. About 9 typed fields, one of them typed twice.
- **8 surfaces in 3 areas**, **4 guesses**:
  1. where to attach a classification (F1);
  2. that a column name opens a side sheet;
  3. where to write a rule (F2, N11);
  4. how to remove a table-level classification (N2).
- **2 buttons disabled with no reason** (N6), and **3 unhelpful errors**:
  - a 600-character engine-internals refusal (N3);
  - a raw `conflict:` message (N10);
  - a toast naming `perm.rules` (N14).

Three causes stand behind most findings.

1. **The UI does not say what it knows.**
   - The tag page never says what the classification does today (F2).
   - Clearance states its limits only after the first add (N7).
   - Preview reports profile arithmetic instead of effect (N5).
   - A second mask on a column is accepted in silence (N4).
2. **Fixes live somewhere else.**
   - A classification attached in Governance can only be removed in Control Plane, 8 clicks away (N2).
   - A rule cannot be started from the classification it is about (N11).
   - The table-level conflict is discovered at Preview, after the attach (N3).
3. **One thing has many names.** The vocabulary table of pass 2 lists 12 concepts, each with 2 to 8 names (N18). The rule page's breadcrumb even says "People & groups" when reached from Classifications (N13).

What works and must be kept (pass 2 "Works well"):

- the create dialog at 390 px;
- the key auto-slug;
- Where used grouped by schema with per-column links;
- "Confidential from table" for inherited classifications;
- the column side sheet;
- the preview guard;
- the delete dialog's consequence list;
- the lock badge;
- "Nothing is enforcing" being said rather than "Allowed";
- no page-level horizontal scroll.

## 2. Principles

| # | Principle | What it rules out |
|---|---|---|
| P1 | **Say what the UI knows.** If the server or the page already knows a fact the user needs (a conflict, an overlap, a limit, an engine count), show it before the user acts, at the control that acts. | Banners that appear only after the write; refusals that are the first notice of a known conflict. |
| P2 | **Show effect, not mechanism.** Lead with what a reader of the data will see ("email is hashed for 12 people, 3 are exempt"). Put the mechanism (profiles, compile counts, usersync) behind a disclosure. | "Would recompile no profiles · created 0 …" as the main sentence. |
| P3 | **Fix where the user is.** Every object that shows a classification can remove it; every classification can start a rule; every refusal that names a remedy offers it as a control. | Detours to Control Plane; "edit the rule" before the rule exists. |
| P4 | **One word per thing.** The glossary (§4) is the only vocabulary for this area; a string that names one of its concepts uses its term. | "tag", "egret rule", "Masking rules", "Reaches this tag", "Carries it". |

## 3. Decisions

Each decision states what the code does today (verified, `file:line` on `origin/main` `265c81c6`), the new behaviour, any API change, a11y and responsive notes, and how it is tested. Paths are relative to the repo root. `FE` means `frontend/src/`.

### C1. One glossary, applied to every string in the area (N18, N13, F6, F7)

**Today.** The same concept has several names:

- Tab labels are "Mask & filter", "View exposure" and "Hand-written" (`FE/routes/governance/classifications/TagPage.tsx:23-29`).
- One section heading is "Mask and filter rules" (`FE/routes/governance/TagRulesSection.tsx:78`). Another is "Masking rules" (`FE/routes/governance/data/ColumnPanel.tsx:234`; column header at `FE/routes/governance/data/tabs/OverviewTab.tsx:306`).
- "egret rule" appears at `TagRulesSection.tsx:40,80`.
- The table tab is "Masks & filters".
- The rule dialog says "Selects by: Tag/Path" (`FE/routes/governance/RuleDialog.tsx:416-425`) and labels the field "Tag" (`:83`).
- The Data header lists "Tags" (`FE/routes/governance/data/ObjectHeader.tsx:164`).
- Where used shows a "Carries it" column (`FE/routes/governance/classifications/tabs/WhereUsedTab.tsx:144`).
- The rule list shows mask types raw: `formatRuleKind` renders `mask · MASK_HASH` (`FE/routes/governance/permFormat.ts:162-169`), while the dialog labels the same value "Hash" (`RuleDialog.tsx:41-50`).
- The rule page header links "People & groups" whatever the entry point (`FE/routes/governance/people/itemParts.tsx:16-18`).
- The in-app "Open in Data" link carries an external-link icon (`FE/routes/governance/openInData.tsx`, `ExternalLink` in `OpenInDataLink`).

**New.**

1. A module `FE/routes/governance/glossary.ts` exports the terms of §4 as constants, plus `MASK_TYPE_LABEL` (moved from `RuleDialog.tsx:41-50`).
2. Every string in the Classification area, the Data area's classification and rule surfaces, the rule dialog and the rule page uses those constants.
3. `formatRuleKind` renders `Mask · Hash` and `Row filter · region = 'us'`.
4. Tabs become **Where used · Clearance · Rules · Views · Manual policies**. The URL keys stay `where-used|clearance|rules|views|handwritten` (`FE/routes/governance/classifications/tagUrl.ts:14`), so links and bookmarks keep working.
5. The rule page's way back names where it came from. `ItemHeader` takes an optional `back: {to, label}`. `RuleDialog`'s callers and the Rules tab link to the rule with `?from=classification:<id>` or `?from=data:<encoded path>`. `RulePage` reads `from` and renders "← <classification name>" or "← <table>", defaulting to "← People & groups".
6. `OpenInDataLink` uses `ArrowRight` instead of `ExternalLink`, because every use is in-app. Importers: `openInData.tsx`, `people/RulePage.tsx`, `people/GrantPage.tsx`, `people/tabs/MasksFiltersTab.tsx`.

**UI language.** UI text stays English (FE-COPY-1). The frontend has no i18n layer: `grep -rln "i18next\|useTranslation" frontend/src` finds nothing. The glossary's Vietnamese column is for the Vietnamese docs (this spec's mirror and, later, `egret-data/docs`), not for UI strings. See §12, item 1.

**Testing.** For each surface, a vitest asserts the new name through `getByRole` and the absence of the retired names (FE-COPY-3). A unit test pins `formatRuleKind`.

### C2. The tag page says what the classification does: an Effect strip (F1, F2, N11)

**Today.** `TagPage` renders the header, then tabs (`TagPage.tsx:129-147`). Nothing summarises the effect.

- Where used, empty, says "No table or column carries this classification." with no way to attach (`WhereUsedTab.tsx:134`).
- Mask & filter, empty, says "No egret rule selects this tag." with no action (`TagRulesSection.tsx:40`).
- No entry point opens `RuleDialog` with this classification. `RuleDialog` accepts `initialSelector`/`initialKind` (`RuleDialog.tsx:163-179`); only the Data Masks tab passes them (`FE/routes/governance/data/tabs/MasksFiltersTab.tsx:239-246`).

**New.** An `EffectStrip` component (`FE/routes/governance/classifications/EffectStrip.tsx`) sits between `TagHeader` and the tabs. It renders a `<dl>` of three facts, each with an action.

| Fact | Source | Text (examples) | Action |
|---|---|---|---|
| Rules | `GET /perm/rules` + `rulesOnTag` (`FE/routes/governance/ruleMatch.ts:62`), only with `policy:read` | "Mask · Hash: Hash email" (and "and N more") / "No rule uses this classification yet." | **Add rule** (needs `policy:update`): opens `RuleDialog` with kind `mask`, mask type `MASK` (Redact), **Applies to** = this classification, name "Mask <name>". Without `policy:read`: "Reading rules needs the policy:read permission." |
| Clearance | `tag.required_roles` / `required_groups` (`FE/api/tags.ts:36-42`) | "Only 2 groups may open data with it" / "None: anyone who can read the data can open it." | **Set clearance** (needs `tag:manage`): selects `?tab=clearance`. |
| Where used | `GET /classifications/{id}/usage` (`tableCount`, `columnCount`, `bi.readable.length`) | "2 tables · 3 columns · 0 BI objects" / "Not on any data yet." | **Attach in Data**: a link to `/governance/data`, with the sentence "Open a table or a column and use Add classification." |

When there is no rule, no clearance and no usage, the strip carries the status line **"Does nothing yet."** The rules and Where-used empty states name the same action (a link, FE-PAGE-3: entry points are links or buttons that open a dialog in place).

**A11y.** The strip is a `<section aria-label="Effect">`. Each action is a real `Button` or `Link`. A missing permission is stated as text beside the fact (FE-ACTION-4), never a silently missing control. **Responsive.** At 390 px the `<dl>` stacks one fact per row (`grid-cols-1 sm:grid-cols-3`).

**Testing.** `classifications/__tests__/EffectStrip.test.tsx` covers:

- the three states for each fact (data, empty, no permission);
- Add rule opening the dialog with the tag's key preselected and the rule body sent to preview;
- Set clearance moving to `?tab=clearance`.

### C3. First use: the empty Classifications page explains the job (m)

**Today.** `EmptyState` with "No classification yet." and the New button (`FE/routes/governance/classifications/ClassificationsIndex.tsx:194`).

**New.** The empty state carries three numbered steps:

1. **Create** a classification (the New button).
2. **Attach** it to tables or columns in **Data** (a link to `/governance/data`).
3. **Add a rule** (mask or row filter) or **set clearance** on the classification's page.

The steps also apply to a holder without `tag:manage`, minus step 1's button: the text instead says "Creating needs the tag:manage permission."

**Testing.** A vitest in `ClassificationsIndex.test.tsx` checks the three steps, the link target and the permission variant.

### C4. Classification chips: colour, link, lock, and remove where you see it (N2, N16, N17)

**Today.**

- Chips are grey `Badge variant="secondary"` with the name and the key as `title`. They have no colour, no link and no lock:
  - the object header (`ObjectHeader.tsx:175-181`);
  - the column panel (`ColumnPanel.tsx:266-279`);
  - the columns table (`OverviewTab.tsx:364-375`).
- `TagDto` already carries `color` (`FE/api/generated/model/tagDto.ts`).
- No Governance surface can remove an assignment. The only remove is Control Plane > Catalogs > Browse > Tags (`FE/components/access/CatalogTagEditor.tsx:186-193`, through `useUnassignResourceTag`, `FE/api/catalogTags.ts:71-91`).
- That remove calls `DELETE /api/control-plane/warehouses/{warehouse_id}/resource-tags` with a JSON body `{schema, table, column?, tag_id}`. The handler is `unassign_resource_tag`, gated on `catalog_registry:update` (`crates/egret-bi/src/catalog_tags.rs:711`, `WAREHOUSE_WRITE` at `:32`). It is idempotent (200 when nothing matched).
- Where used rows are links only (`WhereUsedTab.tsx:148-162`). `TableUsage` has `catalogId` but no `warehouseId` (`crates/egret-controlplane/src/governance_tag_usage.rs:56-65`).
- The Add-tag picker shows bare names (`FE/routes/governance/data/AddTagDialog.tsx:131-135`).

**New.**

1. **`ClassificationChip`** (`FE/routes/governance/ClassificationChip.tsx`, outside the lazy areas so both can import it). It shows:
   - a swatch in the classification's colour (inline `style={{ backgroundColor }}` with the stored value; the colour is user data, not a design token, as `TagSwatch` already does at `FE/routes/governance/classifications/tagParts.tsx:25-33`);
   - the name as a link to the classification's page (only with `tag:read`; otherwise plain text);
   - a `Lock` icon with the sr-only text "requires clearance" when the classification has clearance (read from `useTags()` by id);
   - "from table" for an inherited chip;
   - a remove button (`IconButton`, `aria-label="Remove <name> from <object>"`) when `onRemove` is given.

   It replaces the three grey chip renderings.
2. **Remove** is offered on a chip when the caller holds `catalog_registry:update` (or is platform admin) and the chip is the object's own (an inherited chip says "from table" and links to the table instead). It opens `RemoveClassificationDialog` (`FE/routes/governance/RemoveClassificationDialog.tsx`), an `AlertDialog` with no typing (C11 convention):
   - title "Remove <name> from <path>?";
   - body: "Rules that apply to <name> stop applying here." With `policy:read`, it lists those rules by name (the ones `rulesOnTag` returns), and for a table it adds "Its columns stop inheriting it.";
   - buttons **Cancel** and **Remove**.

   It calls `DELETE …/resource-tags` through the generated `useControlPlaneCatalogUnassignResourceTag`. It invalidates what `AddTagDialog` invalidates (`AddTagDialog.tsx:80-93`) and toasts "Removed <name> from <object>." (FE-ACTION-2: it stays on the page).
3. **Where used** rows gain a **Remove** action per assignment: "Table" and each column. The actions sit in a pinned `ACTIONS_CELL` (`FE/routes/governance/tabParts.tsx:51`). This needs the warehouse id, so **`TableUsage` gains `warehouseId: Uuid`** (backend B5).
4. **The Add-classification picker** renders each option with:
   - the swatch, the name, the key (mono), the first line of the description;
   - a lock badge "Clearance" when gated;
   - "N rules" when the caller has `policy:read` and `rulesOnTag` finds any.

   `SelectItem` keeps the name as its text value (typeahead), with the rest in a secondary line. The dialog title becomes **"Add classification"**, and the button on the header and column panel is renamed the same (C1).

**A11y.** The chip's link and remove button are two focus stops. The remove button's label names the classification and the object. The swatch is `aria-hidden` (the name carries the meaning, FE-A11Y-4).

**Testing.**

- `__tests__/ClassificationChip.test.tsx`: colour swatch style, link target, lock text, remove shown only with permission, inherited chip has no remove.
- `data/__tests__/ObjectPage.test.tsx` and `ColumnPanel.test.tsx`: removing sends `DELETE …/resource-tags` with the exact body and refetches.
- `classifications/__tests__/WhereUsedTab.test.tsx`: Remove on a column row sends `{schema, table, column, tag_id}` to the row's `warehouseId`.

### C5. Table-level classification vs classification mask: warn before, explain short (N3)

**Today.** egret refuses the pair on every write path with one 600-character sentence (`crates/egret-policy/src/unreadable_table_mask.rs:257-275`, `UnreadableTableMask::refusal`). The pair is: a classification attached to a whole table, plus a mask that selects by that classification. The engine type-checks a tag mask against every column, so the table becomes unreadable.

| Write | Where | Status today |
|---|---|---|
| Rule create, patch and **preview** (`Mask` + `Selector::Tag`) | `crates/egret-perm-compile/src/routes.rs:1477-1491` → `PermHttpError::BadRequest` | 400 `{"error":"bad request: <sentence>"}`, no `code` (`crates/egret-perm-compile/src/routes/error.rs:23-24`) |
| Table-level attach (`column == ""`) when a mask already selects the classification | `crates/egret-bi/src/catalog_tags.rs:119-136` → `BiError::BadRequest` | 400, no `code` |
| Rename into a masked name | `crates/egret-bi/src/tags.rs:198-207` | 400, no `code` |

The walkthrough attached first (accepted, because no mask existed yet) and then hit the refusal at Preview. `RuleDialog` shows the message verbatim (`RuleDialog.tsx:314`). The struct carries `mask_origin`, `mask`, `tag` (the name), `schema` and `table` (`unreadable_table_mask.rs:208-217`). It does not carry the tag id or the warehouse id, and `table_level_attachment_of` stops at the first table (`LIMIT 1`, `:403-428`).

**New, backend (B2).**

1. `UnreadableTableMask` gains `tag_id: Uuid`, `warehouse_id: Option<Uuid>` and `more_tables: bool`. `table_level_attachment_of` selects `t.id, a.warehouse_id` and uses `LIMIT 2`; a second row sets `more_tables`. The Mask and Rename arms fill `warehouse_id` from that row. The Attachment arm already knows `tag_id` and leaves `warehouse_id` as `None`: the write in flight *is* the table-level attachment, so there is nothing to remove. `TagWrite` itself is unchanged, so its 20 call sites in tests are untouched.
2. A new method `UnreadableTableMask::refusal_body() -> serde_json::Value` returns:

   ```json
   {
     "error": "<the existing sentence, unchanged>",
     "code": "table_tag_mask_conflict",
     "tag": "PII", "tagId": "…", "warehouseId": "… or null",
     "schema": "analytics", "table": "orders",
     "mask": "uxwalk mask email", "maskOrigin": "rule",
     "moreTables": false
   }
   ```

   `maskOrigin` is `rule` or `policy`.
3. All three write paths return it with status 400, through a new variant on each error type that serves a coded body as is:
   - `PermHttpError::Coded { status, body }`;
   - `BiError::Coded { status, body }`, which maps to `CpError::Coded(status, body)`.

   The fourth caller, the Engine & policy editor (`crates/egret-policy/src/admin/mod.rs:1823-1826`), keeps its plain 400: it is outside this area, and its sentence already names the policy being edited.

   This is the same pattern `Overlap`/`RuleOverlap` already use (`crates/egret-perm-compile/src/routes/error.rs:38`, `crates/egret-bi/src/error.rs:27-28,102`, `crates/egret-controlplane/src/error.rs:35-36,150-152`). The sentence stays in `error`, so older clients keep working.

**New, frontend.**

1. **Rule dialog.** On `ApiError` with `code === "table_tag_mask_conflict"`, the preview area shows one sentence: **"<tag> is on the whole table <schema>.<table>, so masking by <tag> would make that table unreadable."** It offers two actions:
   - **Remove from table**. It needs `catalog_registry:update` and calls `DELETE …/resource-tags` with `{schema, table, column: "", tag_id}` on `warehouseId`, then re-runs the preview.
   - **Apply by path instead**, which switches Applies to = Path.

   When `moreTables` is true, it adds "Other tables carry it too; each will be shown after this one." The server's sentence sits under a "Details" disclosure (`<details>`). Without the permission, Remove is replaced by "Removing needs the catalog_registry:update permission."
2. **Add classification dialog, table target.**
   - With `policy:read`: when the picked classification has a mask rule (`rulesOnTag` → `kind.type === "mask"`), the dialog shows the conflict **before** submit, disables **Add** (aria-disabled, reason tied), and offers **Attach to columns instead**. That action switches the form to a checkbox list of the table's columns and sends one `PUT` per checked column.
   - Without `policy:read`, or when no mask exists yet, the dialog shows a short note under the picker: "On a whole table, this classification counts for clearance on every column. A rule that masks by classification needs it on columns, not on the table."
   - The column list comes from the object page, which already has it (`TagTarget` gains `columns?: string[]`).
   - A server 400 with the code is shown as the same short sentence beside the picker.
3. **Rename** (TagHeader edit form) shows the short sentence for the code.

**Testing.**

- Backend:
  - `crates/egret-perm-compile/tests/routes.rs`: preview of a tag mask over a table-level attachment answers 400 with `code`, `tagId`, `warehouseId`, `schema`, `table`, `maskOrigin` = `rule`, and an `error` that still starts with `tag "`.
  - `crates/egret-bi/tests/catalog_tags_routes.rs`: the table-level attach answers the same code, with `maskOrigin` per surface (the existing test `a_table_level_assignment_is_refused_when_a_tag_mask_names_the_tag` is extended).
  - `crates/egret-policy/src/unreadable_table_mask.rs` unit test: `refusal_body()["error"] == refusal()`.
- Frontend:
  - `__tests__/RuleDialog.test.tsx`: the short sentence, Remove from table sending the exact body and re-previewing, Apply by path.
  - `data/__tests__/AddTagDialog.test.tsx` (Create): the pre-submit warning with a mask rule, the columns path, and the note without `policy:read`.

### C6. A second mask on one column is never silent (N4)

**Today.**

- `GET /perm/overlaps` (`routes.rs:134`, `list_overlaps` at `:1769`) and the write-time 409 (`refuse_created_by_compile_via`, `crates/egret-perm-compile/src/overlap.rs:249-314`) detect pairs of masks that may decide one request differently, including tag-and-path and tag-and-tag on one column (`crates/egret-policy/src/unit_overlap.rs:402-590`).
- Both compare **compiled downloads per profile**. When no engine-bound profile serves the catalog, as on the demo stack, nothing is compiled, so nothing is detected. That is exactly the walkthrough's case: a second mask on `email` with a different exemption was accepted with no warning.
- Exemptions compile to a `MASK_NONE` item placed before the "everyone" item (`crates/egret-perm-compile/src/rules.rs:43-44,764-778`). With two mask policies on one column, which one applies is decided by evaluation order. That is why egret refuses the pair when it can see it.
- The 409 body is `{error, code: "rule_overlap", rules: [{id, name}], coordinate, kind}` (`crates/egret-perm/src/actions.rs:509-529`).

**New.**

1. **Model-level detection in Preview (B3).** `RuleEffect.alreadyMasked` lists the other mask rules that reach any column this rule reaches. It is computed from the rules and the classification assignments, independently of profiles. Each entry is `{ruleId, ruleName, maskType, coordinate, exempt}`, capped at 20 (spec of the function in §6, B3).
2. **The rule dialog shows it before Create:**
   - "email is already masked by **Hash personal data** (Hash, except group data-stewards)." The rule name links to its page.
   - **Effective result per group.** One line per principal named in any exemption, plus "Everyone else". It comes from a pure function `effectivePerPrincipal(newRule, alreadyMasked)` in `FE/routes/governance/ruleEffect.ts`:

     | Principal is exempt from | Line |
     |---|---|
     | every mask on the column | "sees the real value" |
     | none | "masked (<type>)". If the types differ: "masked; which mask applies is decided by engine order" |
     | some but not all | "**undecided**: the engine applies whichever mask it evaluates first" |

   - A required checkbox, **"Create anyway: I understand email will have two masks"**. Create stays aria-disabled until it is ticked, with the reason tied (C8).
3. **A 409 `rule_overlap`** from preview or save (an engine serves the catalog) renders as:
   - "This would make two masks decide <coordinate> differently: <rule A> and <rule B>.";
   - links to both rules;
   - the server's `error` under Details.

   The overlap here is refused by the server, so no checkbox is offered.

**Testing.**

- `crates/egret-perm-compile/tests/routes.rs`:
  - a preview of a path mask on a column that a tag mask already reaches, **on a catalog no engine serves** (`orphan_catalog`), returns `effect.alreadyMasked` naming the tag rule;
  - the same preview of a rule's own PATCH does not list itself.
- `FE/routes/governance/__tests__/ruleEffect.test.ts`: the three rows of the table above, with a fixture where the principals are not pre-sorted.
- `__tests__/RuleDialog.test.tsx`: the warning, the per-group lines, the checkbox gating Create, and the 409 rendering.

### C7. Preview speaks in effect (N5)

**Today.**

- `PreviewResponse` is `{affected: [{id, name}], compile: {services, created, updated, deleted, unchanged, skipped}}` (`crates/egret-perm-compile/src/routes.rs:953-1010`).
- The dialog prints "Would recompile <names|no profiles>" and the four counts (`RuleDialog.tsx:473-485`).
- The preview handler runs `write_rule` and `name_profiles` in a rolled-back transaction (`routes.rs:2061-2085`).
- Engines per catalog come from `registry.engine_catalog_attachments` ⋈ `engine_mgmt.engines` with `authz_service_id IS NOT NULL` (`services_for_catalog_via`, `crates/egret-policy/src/store.rs:1487`). No function counts engines.
- Exemption members are resolved at compile time by `expand_enumerated_principals` (`crates/egret-perm-compile/src/members.rs:678`). The read-only member queries are `group_members_via` (`:552`), `workspace_members_via` (`:524`) and `role_holders_via` (`:496`).

**New, backend (B3).** `PreviewResponse` gains `effect: Option<RuleEffect>`, present for a rule preview and absent for a grant preview. It is computed from the `NewRule` **before** `write_rule`, in the same transaction, excluding the rule's own `id` on a PATCH. Wire shape (camelCase):

```text
RuleEffect {
  kind: "mask" | "row_filter",
  catalogs: [ { catalogId, catalogName,
                engines: [ { id, name, engineType } ],      // engines attached to the catalog AND bound to a profile
                notEnforced: null | "no_engine" | "no_profile" } ],
  tableCount: number | null,     // null when a path level is a pattern
  columnCount: number | null,    // masks only; null for a pattern or a row filter
  enforcedBy: number,            // distinct engines across catalogs
  notEnforced: null | "no_assignment" | "no_engine" | "no_profile",   // the rule as a whole; null when enforcedBy > 0
  exemptMembers: number,         // distinct users in exempt.include (groups, roles, workspaces expanded; `public` counts as 0 and sets exemptEveryone)
  exemptEveryone: boolean,
  alreadyMasked: [ { ruleId, ruleName, maskType, coordinate, exempt: PrincipalSet } ]   // C6, masks only, ≤ 20
  truncated: boolean,            // alreadyMasked may be incomplete: a cap cut it (50 columns examined, 1000 classifications read, 20 entries)
}
```

- **Tag rule reach.** The `catalog_tag_assignments` rows of the classification's key, joined to `registry.catalogs` on `warehouse_id`. Rows whose warehouse no catalog names are ignored here; Where used already counts them as `orphaned`.
- **Path rule reach.** The scope's catalog. `tableCount` is 1 when `schema` and `table` are named without glob characters (`*`, `?`), else `null`. `columnCount` follows the same rule for `column`. An absent column on a mask means "every column": `columnCount` is `null`, and the UI says "every column of <table>" (see C8).
- **`notEnforced`.** Per catalog: `"no_engine"` when the catalog has no engine attachment, `"no_profile"` when engines are attached but none is bound to a profile, `null` when some engine enforces the rule there. On the rule as a whole: `"no_assignment"` when the rule reaches no catalog (a classification assigned nowhere a registry catalog fronts), else `null` when `enforcedBy > 0`, else `"no_profile"` when any reached catalog has that reason, else `"no_engine"`.
- **`alreadyMasked` reach.** An exact column, a column pattern and a whole table (a path mask without a column, on a literal table) are all checked. Against stored path masks the levels must match; two column patterns count as overlapping when either matches the other. Against stored tag masks, the classifications assigned to the reached columns, and to the whole table, are matched by name, case-sensitively. A tag rule checks its first 50 columns; `truncated` says when any cap applied.

**New, frontend.**

1. The preview block leads with one sentence built by `effectSentence(effect)` (`ruleEffect.ts`), for example:
   - "Masks 1 column on 1 table · enforced by 2 engines · 3 people exempt"
   - "Masks every column of 1 table" (a path mask without a column)
   - "Masks the columns matching the path" (a pattern)
   - "Filters rows of 4 tables · … · nobody exempt"
   - "Masks 12 columns on 5 tables · enforced by 1 engine · 1 person exempt"
2. **`enforcedBy === 0`** is a warning, not a footnote: "**Not enforced yet.** <catalog> has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines." For `no_profile`, the last sentence becomes: "Its engines have no policy profile; bind one in Governance > Engine & policy."
3. The profile list and the compile counts move under a `<details>` labelled "What egret will recompile". The `skipped` lines stay there too.

**A11y.** The warning uses a `role="status"` region tied to the Create button by `aria-describedby`, so a screen reader hears it when Create gets focus.

**Testing.**

- `crates/egret-perm-compile/tests/routes.rs`:
  - (a) the fixture catalog (one engine bound) gives `enforcedBy == 1` and `notEnforced == null`;
  - (b) `orphan_catalog` gives `enforcedBy == 0` and `notEnforced == "no_engine"`;
  - (c) exempting a group of 2 users plus one of them directly gives `exemptMembers == 2`;
  - (d) a tag rule gives `columnCount == 1` for the fixture's one tagged column.
- `ruleEffect.test.ts`: the effect sentence variants.
- `RuleDialog.test.tsx`: the warning, and the details disclosure holding the profile names.

### C8. The rule dialog: reasons, required fields, defaults, drafts, errors in view (N6, N11, N12, N26)

**Today.**

- **Disabled Create.** Create is `disabled` (`RuleDialog.tsx:525`, `disabled={!canSave || saving}`), so it leaves the tab order (N22). When `blockedReason` is set, the reason is an `Alert` above the preview box (`:447-451`). When only the preview is missing, the reason is grey text inside the preview box (`:507-512`). Neither is tied to the button.
- **Missing column.** The walkthrough read "Mask + Path silently disabled because Column isn't marked required". The code says otherwise: an absent column is legal and means *every column* (`ensure_scope_levels_are_named` skips absent levels, `crates/egret-perm-compile/src/grants.rs:445-460`, and `scope_to_resources` renders them `*`). The real defect is that the dialog never says so.
- **Defaults.** Kind defaults to `mask` (`:214`) and Applies to defaults to `tag` (`:224`). The Data Masks tab overrides both with a path selector and `row_filter` on a table (`MasksFiltersTab.tsx:245-246`), which is the "Filter rows + Path" the walkthrough met.
- **Draft lost on Cancel.** Cancel unmounts the form (`RuleDialog.tsx:180-186`, `{open && <RuleForm/>}`), losing every field.
- **Error out of view.** `saveError` renders at the top of the scroll body (`:347-352`). `previewError` renders inside the preview box at the bottom (`:467-471`). Either can be off-screen, and the fields scroll away (N26).

**New.**

1. **Applies to** replaces "Selects by", with the options **Classification** and **Path** (C1).
   - Required fields carry a visible "(required)" in their `Label` and `aria-required="true"`: Name; the expression for Custom and Row filter; Classification for Applies to = Classification; Catalog for Applies to = Path.
   - For a path mask, the Column field's help text reads "Leave empty to mask every column of the table."
2. **Create is never silently disabled.**
   - Create is rendered `aria-disabled` (not `disabled`) while blocked, so it keeps focus (the `ReasonButton` pattern, `FE/components/workflows/ReasonButton.tsx`). A click while blocked does nothing.
   - Its `aria-describedby` points at one reason line rendered **in the footer beside the button**: the first of `blockedReason`, "Preview the rule to enable Create." (no preview, or the form changed since), the C6 checkbox reason, or the preview error summary.
   - The Preview button follows the same pattern.
3. **Defaults per entry point.**

   | Entry point | Kind | Applies to | Prefilled |
   |---|---|---|---|
   | Classification page (Effect strip, Rules tab empty state and toolbar) | Mask, Redact | Classification | this classification; name "Mask <name>" |
   | Data, table, Rules tab | **Mask**, Redact | Path | catalog/schema/table; Column empty with the "every column" help, focused |
   | Data, column panel (new "Add rule" button, `policy:update`) | Mask, Redact | Path | the column; when the column carries classifications, a hint "Or apply it to <classification>" switches Applies to |
   | Rule page, Edit | the rule's own | the rule's own | the rule |

   Row filter stays one click away. The table default changes from `row_filter` to `mask` because the walkthrough's job, and the area's name, are about masking. The current reason for `row_filter` (a path selector naming a table completes a row filter, `RuleDialog.tsx:163-165`) is kept for the **Row filter** kind: switching to it on a table entry fills the scope.
4. **Drafts survive Cancel.**
   - The dialog keeps the form state in a module-level store keyed by the signed-in user and the entry point (`FE/routes/governance/ruleDraft.ts`: `ruleDraftKey(userId, entry)`, `loadRuleDraft`, `saveRuleDraft`, `clearRuleDraft`). It is in memory only, so it survives a detour within the SPA, such as removing a table classification in Data, but not a reload. It is not `localStorage`, which FE-DATA-3 keeps for preferences.
   - Closing a dirty form (Cancel, Escape, outside click) asks **"Discard this rule?"** with **Keep editing** and **Discard**. Discard clears the draft.
   - Reopening the same entry point restores the draft, with a line "Restored your unsaved rule · Start over".
   - A successful Create clears it.
   - `useUnsavedChangesWarning(dirty)` covers a reload (FE-ACTION-5).
5. **Errors stay with the form.**
   - Preview and save errors render in the footer region next to Create. That region is `sticky bottom-0` inside the scroll body, so it is always in view.
   - Focus moves to the error's heading.
   - The fields keep their values (FE-ACTION-3).
   - The group suggestion list of `PrincipalPicker` renders below its help line rather than over it (N26). This is a change local to `PrincipalPicker`'s exempt mode, tested with a DOM-order assertion.

**Testing.** `__tests__/RuleDialog.test.tsx` gains:

- the reason in the footer is the button's description, with the button focusable and `aria-disabled`;
- "(required)" on each required label;
- the path-mask help text;
- the entry-point defaults, one test per row;
- the discard confirmation, and the restore after Cancel → reopen;
- the error region following focus.

Playwright (Task 12) checks that the footer stays in view after an error at 1440 and 390 px (FE-TEST-2).

### C9. Clearance says what it does before the first add, and who is cleared (F3, N7, N21)

**Today.**

- The tab says objects "can only be opened by principals holding one of the required roles or belonging to one of the required groups" (`FE/routes/governance/classifications/tabs/ClearanceTab.tsx:145-148`).
- The enforcement limits appear only once a group or role is present (`:163-196`).
- The empty table reads "Not gated — anyone can open objects with this classification." (`:357-361`).
- The warnings are `Badge variant="warning"`, i.e. `bg-yellow-600 text-white` (`FE/components/ui/badge.tsx:22-23`), a raw palette class. Pass 2 computed about 2.9:1, which fails AA (N21).
- No endpoint returns the people a clearance admits. The parts exist:
  - role holders by **name** (`role_holders_via`, `members.rs:496`);
  - group members by name (`group_members_via`, `:552`);
  - `GET /groups/{id}/members` (`crates/egret-controlplane/src/access/mod.rs:745`, `group:read`).
- Clearances store roles by name and groups by **id** (`migrations/0082_classification_policies.sql`, `0083`).

**New.**

1. **Scope first, always visible** (before any add). A short `Callout` with tokens (see 3):
   - "**Clearance decides who may open data with this classification** in egret (tables, columns, BI objects).
   - On engines it is enforced only through **required roles**: their members are named in each engine's policy. A required **group** is enforced when the data is opened in egret, not in SQL Lab or notebooks.
   - To hide values from everyone without clearance, add a **mask rule** as well."

   "add a mask rule" is a button that opens `RuleDialog` as in C2. The existing measured warnings (writes not withheld, views, the StarRocks 500-column limit) move under a `<details>` "Engine details", unchanged in substance.
2. **Who is cleared now (B4).** A new read `GET /api/control-plane/perm/clearances/{tag_id}/people` (`policy:read`; it reveals people, which `/perm/people` also gates on `policy:read`). It returns:

   ```text
   ClearedPeople {
     count: number,               // everyone listed by the rules below (not everyone egret clears: platform admins are not listed)
     people: [ {                  // the first 200, sorted by label
       userId, label, username: string | null,
       status: string,            // "active" | "disabled"; a disabled account is listed
       serviceAccount: boolean,
       via: [ { kind: "role" | "group", name,
                scope: "platform" | "workspace" | "object" | null,   // role grants only
                scopeId: uuid | null,                                 // workspace or object id
                throughGroup: string | null } ],                      // a role held by this group, not by the person
       onEngines: boolean         // holds a required role DIRECTLY and has a username: the engine deny excepts them
     } ],
     truncated: boolean,          // more than 200 listed; count is exact
     enginesEnforce: boolean,     // the clearance names at least one required role
     onEnginesCount: number       // people with onEngines, independent of the cap
   }
   ```

   Who is listed, and what each population means:

   | Population | Listed as | Egret (object access) | Engines (SQL) |
   |---|---|---|---|
   | Holds a required role directly (live `allow`; `role_holders_via`) | `via.kind = role`, `throughGroup` null | cleared | cleared when they have a username (`onEngines`) |
   | Holds a required role through a group they are a direct member of (live `allow` group grant) | `via.kind = role`, `throughGroup` = group | cleared | refused |
   | Member of a required group, the clearance names **no** role | `via.kind = group`; `enginesEnforce` false | cleared | nothing is withheld from anyone (a group-only clearance compiles to nothing) |
   | Member of a required group, the clearance **also** names a role | `via.kind = group` | cleared | refused |
   | Role grant scoped to a workspace or object | `via.scope`, `via.scopeId` | cleared within that scope only | cleared everywhere (a column read has no scope) |
   | Platform admin | not listed | always cleared | no bypass |

   - The results are deduplicated by user id and sorted by label.
   - A tag without clearance answers `{count: 0, people: [], truncated: false, enginesEnforce: false, onEnginesCount: 0}`. An unknown tag is 404.

   **UI copy (qualified; never "can open it" for the count).** The tab shows an expandable list (a `Collapsible`/`<details>`) whose summary is "**N people hold a required role or belong to a required group**" ("1 person holds a required role or belongs to a required group"). Each item shows name, username, the ways it is listed, a "service account" or "disabled" marker, and an "engines" mark where `onEngines`. A scoped role reads "in workspace X only" / "on one object only". Fixed lines:
   - Always: "Platform admins can always open it in egret."
   - When `!enginesEnforce`: "Engines do not enforce this clearance: anyone with SQL access can read the data."
   - On a row listed only through a group-held role: "Cleared in egret, refused on engines".

   Without `policy:read` it shows the counts of roles and groups and the sentence "Seeing who is cleared needs the policy:read permission."
3. **Contrast (N21).** The warnings stop using `Badge variant="warning"`. The badge is a shared component, so it is not changed (FE-TEST-3); its raw palette is debt listed in the rulebook. A local `Callout` uses `bg-highlight/15 text-foreground border-highlight` with an `AlertTriangle` in `text-foreground`. `--highlight` and `--foreground` are tokens (`FE/index.css`), so no raw colour is involved (FE-VISUAL-1). Body text on a 15% tint of `--highlight` over `--background` keeps the foreground's contrast (above 7:1 in light and dark). The Playwright step measures it with `getComputedStyle` (FE-TEST-2).
4. "Not gated — …" becomes "None: anyone who can read the data can open it." (glossary).

**Testing.**

- `crates/egret-perm-compile/tests/people.rs`, `cleared_people_*`: role holder plus group member plus a user in both counted once; an expired grant not counted; `onEngines` true only through a role; gated on `policy:read` (403 for `nobody`); 404 for an unknown tag.
- `classifications/__tests__/ClearanceTab.test.tsx`: the scope text with an empty clearance, the count and list, the no-permission text, and the mask-rule button opening the dialog.

### C10. Create and edit a classification: duplicates, key rules, colour label (N8, N9, N10)

**Today.**

- **Key charset.** The server accepts keys of `[a-z0-9:-]` only (`crates/egret-bi/src/tags.rs:52-65`, a character loop). The UI hint promises `_` too (`ClassificationsIndex.tsx:398-401`), and `slugifyTagKey` keeps `_` (`tagParts.tsx:8-16`). Validation happens on submit only.
- **Duplicate key.** A duplicate key maps to `BiError::Conflict("a tag with that key already exists")` (`tags.rs:40-47`) → `CpError::Conflict` → 409 `{"error":"conflict: a tag with that key already exists"}` (`crates/egret-controlplane/src/error.rs:118,148-170`), with no `code`. The dialog shows any 409 under Key, verbatim (`ClassificationsIndex.tsx:325`).
- **Display names.** Names are not unique anywhere: there is no index (`migrations/0080_tags.sql:6-14`) and no check. **This matters for enforcement, not only for clarity:** engines match a classification rule by the classification's **name** (`ruleMatch.ts:55-64`; `TagRulesSection.tsx:80-81`). Two classifications called "Public" therefore share every rule written on either.
- **Colour.** The colour input is a native `type="color"` with a visible "Colour" label (`ClassificationsIndex.tsx:368-376`, `TagHeader.tsx:129-138`). Pass 2 heard it announced as "textbox" (N22).

**New (B1).**

1. **Duplicate names are refused, case-insensitively,** on create and on rename. The controlplane handlers `create_tag` and `update_tag` (`crates/egret-controlplane/src/tags.rs:167,201`) call a new `TagRepo::name_taken(name, except: Option<Uuid>) -> Result<Option<ExistingTag>, BiError>` (`ExistingTag { id, key, name }`) (`lower(name) = lower($1)`), and on a hit answer 409:

   ```json
   {"error":"A classification named \"Public\" already exists (key public).","code":"tag_name_taken",
    "name":"Public","existing":{"id":"…","key":"public","name":"Public"}}
   ```

   The check lives in the HTTP handlers, not in `TagRepo::create`. More than 50 test fixtures create classifications named "PII" through the repo in a shared test database, and the guard is a product rule for authors, not an invariant of the table (§9, §10).
2. **Coded key errors.**
   - The duplicate key becomes 409 `{"error":"A classification with the key \"pii\" already exists (PII).","code":"tag_key_taken","key":"pii","existing":{"id","name"}}`. `TagRepo::create` maps the unique violation to `BiError::Coded`, reading `existing` after it; `map_conflict_err` goes.
   - An invalid key becomes 400 `{"error":"…","code":"tag_key_invalid","key":"…","allowed":"a-z 0-9 : -"}`.
3. **Inline validation in the dialog.**
   - The key field validates as you type against `TAG_KEY_PATTERN = /^[a-z0-9:-]+$/` (exported from `tagParts.tsx`, the server's rule written once on this side). The hint reads "Lowercase letters, digits, `-` and `:`."
   - `slugifyTagKey` maps `_` to `-`.
   - The name field checks the loaded list (`useTags`) case-insensitively and shows "A classification named Public already exists." with a link to it before submit. The server's `tag_name_taken` is the authority and shows the same sentence.
   - `tag_key_taken` shows "Key pii is taken by <name>." under Key.
   - Errors use `aria-invalid` and `aria-describedby` (FE-FORM-4).
4. **Colour.** The input keeps `type="color"` and its `Label`. It gains `aria-describedby` text "Colour of the swatch shown next to the name." The swatch preview beside it shows the hex value as text, so the value is readable without the picker. The "textbox" announcement comes from the browser's native control; the label is correct and stays.

**Testing.**

- `crates/egret-controlplane/tests/tags_counts.rs` (it already builds the controlplane router with a real PDP):
  - creating "PII" then "pii" gives 409 `tag_name_taken` with `existing.key`;
  - renaming another classification to "PII" gives the same;
  - renaming a classification to its own name in a different case gives 200;
  - a duplicate key gives `tag_key_taken`;
  - `bad_key` gives 400 `tag_key_invalid`.
- `crates/egret-bi/tests/tags_db.rs`: the duplicate-key assertion changes from `BiError::Conflict(_)` to `BiError::Coded { .. }` with the code.
- `classifications/__tests__/ClassificationsIndex.test.tsx`: the inline key error for `a_b`, the slug of "a_b c" is `a-b-c`, the duplicate-name warning before submit, and each server code placed beside its field.

### C11. Delete and remove flows: one convention (N13, N14, N15)

**Today.**

- **Classification delete** asks to type the key, case-sensitive (`FE/routes/governance/classifications/DeleteTagDialog.tsx:113-120`). It predicts the refusal ("1 rule selects it: the delete will be refused …", `:80-84`) and still enables Delete. On 409 it toasts the server's sentence, which names `perm.rules` (`crates/egret-bi/src/tags.rs:392-400`), and points at a section by name (`FE/routes/governance/tagDeleteFailure.ts:22-26`).
- **Rule delete** asks to type "delete" (`FE/routes/governance/people/RulePage.tsx:72-81`, `ConfirmDeleteDialog` default `confirmText`). After success it stays on the page, which then reads "No rule has this id" (`:41-45`).

**New convention, stated in the glossary.**

- **Delete** removes a definition (a classification, a rule). It uses `ConfirmDeleteDialog`, where you **type the name** shown in the title. One rule for both: the classification's name, not its key; the rule's name. Case-insensitive, the component default.
- **Remove** takes an assignment off an object, or a person off a clearance. It is a one-step `AlertDialog` naming what stops applying (C4), or no dialog for clearance rows (unchanged).

Concretely:

1. **Classification delete.**
   - When rules use the classification, the dialog's Delete is aria-disabled with the reason "Delete or change these rules first:", followed by links to each rule. The type-to-confirm field is not shown.
   - With `policy:read` the rules come from `GET /perm/rules`. Without it, the server's 409 is the guard, and its body now carries the rules (below).
   - **B1**: `TagRepo::delete` reads `SELECT id, name FROM perm.rules WHERE tag_key = (SELECT key FROM tags WHERE id = $1)` first and refuses with 409 `{"error":"Rules still use this classification: <names>. Delete them or change what they apply to first.","code":"tag_in_use","rules":[{id,name}]}`. The foreign-key mapping stays as a fallback, with the same code and `rules: []`.
   - The toast (when it still happens: a race, or no `policy:read`) says "<name> is still used by <n> rules." with the action **Show rules**. `perm.rules` never appears.
2. **Rule delete.** Type the rule's name. On success, navigate to where the rule page was opened from (the `from` of C1, defaulting to `/governance/classifications`), and toast "Rule <name> deleted." (FE-ACTION-2 allows a toast on navigation only when the response is the point; here it confirms an action whose page disappeared, so the toast is kept, matching `DeleteTagDialog` from the index).
3. `tagDeleteFailure.ts` uses the code, not the status, and its copy follows the glossary.

**Testing.**

- `classifications/__tests__/DeleteTagDialog.test.tsx`: disabled with linked rules, a name-typed confirm, and the toast for `tag_in_use` without `policy:read`.
- `people/__tests__/RulePage.test.tsx`: typing the name, then navigation to `from`.
- `crates/egret-bi/tests/tags_db.rs`: `deleting_a_tag_a_live_rule_selects_is_refused_not_silently_cascaded` asserts `code == "tag_in_use"` and the rule's id in `rules`.

### C12. Explain: verdict first, internals collapsed (N19; the rest is Package 2)

**Today.** `ExplainResultView` renders the verdict blocks, then an always-open `Alert` of up to five paragraphs (`FE/components/perm/ExplainResult.tsx:1209-1374`): tag-list mechanics, enricher direction, roles and usersync, hypotheticals, masks not reported, polling.

**New.**

- The verdict stays first.
- **One visible line** holds the limits that change what a verdict means: "Masks on Trino and row filters are not shown here, so *Allowed* can still return masked values or filtered rows, and a manual policy that names a role or a group other than `public` matches nobody, here or on the engines."
  - The role/group clause is the folded roles paragraph in short, and it must stay true to it. egret serves no roles download and runs no usersync, and explain sends neither, so a manual policy that names a role or a group matches nobody, here or on the engine. A *Denied* that such a policy was meant to lift is therefore also the engine's answer. Wording such as "a person with access through a role may still be allowed" is false: egret's own grants through a role or group are compiled to usernames and are evaluated here.
  - With no profile (`no_profile_serves_the_catalog`) there are no verdicts, so the line is only "Masks on Trino and row filters are not shown here." It names no *Allowed* and gives no verdict caveat.
- Everything else moves into `<details><summary>How this answer was worked out</summary>…</details>`, text unchanged.
- The "Nothing is enforcing" state keeps its badge. Its paragraph shortens to: "No engine with a policy profile serves this catalog, so nothing is enforced here (this is not a refusal). Attach an engine to govern it." The current long sentence moves under the details.

The component is shared by Check access and Who can read, so both get the change. That is intended (one explain answer, rendered one way).

**Testing.** `FE/components/perm/__tests__/ExplainResult.test.tsx` (existing suite): the short line is visible, the paragraphs are inside a closed `details`, and the copy assertions that referenced the moved paragraphs still find them after opening.

### C13. Responsive, a11y, 403 (N20, N22, N23)

1. **Classifications list at 390 px.** The Key, Description and BI objects columns get `className: "hidden md:table-cell"` (FilterableTable applies a column's `className` to its header and cells: `FE/components/explore/FilterableTable.tsx:237,240,271`). Name, Data, Clearance and the actions stay. The key is still on the classification's page, and the search still matches it.
2. **Data columns table at 390 px** (`OverviewTab.tsx:300-352`). It uses `STACKED_TABLE` (`tabParts.tsx:62-67`), the pattern People already uses: rows become cards and each cell is labelled by `data-label` ("Type", "Classifications", "Rules").
3. **The generic scroll cue** and the tab strip wrapping are U3's (KI-233/234 in `docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`) and are not duplicated here.
4. **Disabled with reason (N22).** Every disabled action this package touches uses `aria-disabled` with a tied reason (C8 pattern): rule dialog Preview and Create, classification Delete, Add classification, Add clearance. The schema tree's unnamed gridcells are outside this area (CatalogTree) and are listed in §11.
5. **403 (N23).**
   - `Forbidden` (`FE/routes/Forbidden.tsx:2-4`) gains optional props `{ reason?: string; back?: {to: string; label: string} }`, additively (FE-TEST-3).
   - It renders "You don't have access to this page.", the reason, and a link back. The default back link is "Go to the home page" → `/`.
   - `RequireGovernanceAccess` (`FE/auth/guards.tsx:91-94`) passes the reason "Governance needs a governance permission such as tag:read or policy:read.", and `RequirePermission` passes the permission it checked.
   - Rendering the 403 **inside** the app shell, and one wording for every page, are KI-230/231 (U3). This package does not move the guard.
6. **Tag page tabs at 390 px.** They wrap to 3 rows today. With the shorter labels of C1, they fit 2 rows. Measured in Task 12.

**Testing.** Vitest for the `Forbidden` props and the guards' reasons (`FE/__tests__/governanceRouting.test.tsx`). Playwright at 390 × 844 for the two tables: the Name and Classifications facts are visible without horizontal scroll, and `document.scrollingElement.scrollWidth <= innerWidth`.

## 4. Glossary

One term per concept. The UI uses the **Term** column. The Vietnamese column is for the Vietnamese docs.

| Concept | Term (UI) | Definition shown in help text | Retired names | Vietnamese (docs) |
|---|---|---|---|---|
| A label put on data | **Classification** (plural Classifications) | A label you attach to tables and columns (and BI objects) to say what kind of data they hold. | tag, Tags, "Pick a tag", "A tag to classify…" | Phân loại |
| Its stable identifier | **Key** | Fixed once created; lowercase letters, digits, `-` and `:`. | "stored as", "tag key" | Khoá |
| Putting it on data | **Attach** / **Remove** | Attach a classification to a table or column; remove it from one. | assign, unassign, add tag | Gắn / Gỡ |
| What a rule does to data | **Rule**, of kind **Mask** or **Row filter** | A rule masks a column's values or filters a table's rows for everyone except the people it exempts. | egret rule, Mask & filter, Masks & filters, Masking rules, MASK RULE, permission rule (perm.rules), policy | Quy tắc (Che / Lọc dòng) |
| How a mask hides | **Mask type**: Redact, Partial (last 4), Partial (first 4), Hash, Nullify, Unmasked, Year only, Custom | — | `MASK`, `MASK_HASH` … in lists | Kiểu che |
| Who a rule does not apply to | **Exempt** | People who see the real value (or every row). | Include, "except group" (kept only in sentences) | Miễn trừ |
| What a rule is written on | **Applies to**: Classification or Path | A rule applies to every column carrying a classification, or to one catalog path. | Selects by, Reaches this tag, Tagged: on column, by tag | Áp dụng cho |
| Who may open classified data | **Clearance** | The roles and groups allowed to open data with this classification. | Required roles and groups, gated, Not gated, opened by principals, gate | Quyền truy cập theo phân loại |
| Where it is attached | **Where used** | The tables, columns and BI objects that carry it. | Carries it | Nơi dùng |
| Policies written in the engine's own editor | **Manual policy** (tab "Manual policies") | A policy written directly in an engine profile's editor (Governance > Engine & policy), not generated from a rule or a grant. | Hand-written, Hand-written policies | Chính sách trong engine |
| Checking who can read | **Who can read?** | Asks each engine's policy whether a person can read this. | Explain (kept as the API name), per policy profile | Ai đọc được? |
| Taking a definition away | **Delete** (type the name to confirm) | — | "type delete", "type the key" | Xoá |
| Taking an assignment away | **Remove** (confirm, no typing) | — | unassign, x | Gỡ |
| Engines applying a rule | **Enforced by N engines** / **Not enforced yet** | — | "Would recompile no profiles", "Nothing is enforcing" (kept in Explain only) | Được N engine thực thi / Chưa được thực thi |

## 5. What stays

- URL keys and routes: `/governance/classifications/:id?tab=…`, `/governance/rules/:id`.
- The key's immutability.
- The delete consequence list.
- Where used grouping.
- "from table" for inherited classifications.
- The preview gate before Create.
- Explain as the only place that answers who can read (F10 of #2).

## 6. Backend changes (enumerated)

| # | Change | Where | Wire |
|---|---|---|---|
| B1 | **Coded errors for classification writes.** `BiError::Coded { status, body }`, `CpError::Coded(StatusCode, Value)` and `PermHttpError::Coded { status, body }`, each served as is. Codes: `tag_key_invalid` (400), `tag_key_taken` (409), `tag_name_taken` (409, a check in `create_tag`/`update_tag` via the new `TagRepo::name_taken`), `tag_in_use` (409 from `TagRepo::delete`, with `rules`). | `crates/egret-bi/src/{error.rs,tags.rs}`, `crates/egret-controlplane/src/{error.rs,tags.rs}`, `crates/egret-perm-compile/src/routes/error.rs`; the exhaustive match in `crates/egret-automation/src/render/payload.rs:347-361` gains the arm | `{error, code, …fields}`; the `error` sentence stays |
| B2 | **`table_tag_mask_conflict`** body from all three write paths; `UnreadableTableMask` gains `tag_id`, `warehouse_id`, `more_tables`. | `crates/egret-policy/src/unreadable_table_mask.rs`, `crates/egret-perm-compile/src/routes.rs:1477-1491`, `crates/egret-bi/src/catalog_tags.rs:119-136`, `crates/egret-bi/src/tags.rs:198-207` | C5 |
| B3 | **`PreviewResponse.effect: Option<RuleEffect>`**, computed by a new `crates/egret-perm-compile/src/effect.rs` (`rule_effect(conn, id, &NewRule)`), using `egret_perm::eval::glob_match` for path and name matching (case-insensitive for path levels, case-sensitive for classification names, as the engines do). | `crates/egret-perm-compile/src/{effect.rs,routes.rs}` | C7 |
| B4 | **`GET /api/control-plane/perm/clearances/{tag_id}/people`** (`policy:read`) → `ClearedPeople`. | `crates/egret-perm-compile/src/routes/people.rs`, route in `routes.rs` | C9 |
| B5 | **`TableUsage.warehouseId`**. | `crates/egret-controlplane/src/governance_tag_usage.rs:56-65` | C4 |

After B3, B4 and B5: regenerate `openapi.json` (`cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`), then `npm run gen:api` in `frontend/`, never bare `orval` (FE-DATA-1).

**Not in Package 1 (looked at and refused): the warehouse id check on `PUT …/resource-tags`.** It is confirmed that a PUT naming a warehouse that does not exist answers 200 and stores an orphan row. `catalog_tag_assignments.warehouse_id` has no foreign key (`migrations/0081_catalog_tag_targets.sql:11`), and `assign_resource_tag` (`crates/egret-bi/src/catalog_tags.rs:665-695`) checks nothing. The guard itself is a few lines. But the five route-level test files that PUT with an invented warehouse id would need a real `catalog.warehouses` row, which needs a workspace and a storage connection: `egret-bi/tests/catalog_tags_routes.rs`, `egret-bi/tests/tag_rename_unreadable_pair.rs`, `egret-controlplane/tests/governance_browse.rs`, `egret-perm-compile/tests/routes.rs` and `egret-policy/tests/unreadable_table_mask_db.rs`. That is not a one-line change. The UI always sends the warehouse id of a registered catalog. Listed in §11.

## 7. Wire and DTO shapes (frontend)

| Type | File | Notes |
|---|---|---|
| `RuleEffect`, `EffectCatalog`, `EffectEngine`, `ExistingMask`, `PreviewResponse.effect` | generated (`FE/api/generated/model/`) | after `npm run gen:api` |
| `ClearedPeople`, `ClearedPerson`, `ClearedVia`, `ClearedViaKind`, `ClearedScope`; hook `usePermClearedPeople(tagId)` | generated | orval names the hook from tag `perm` + fn `cleared_people`, as `list_overlaps` became `usePermListOverlaps` |
| `TableUsage.warehouseId` | generated | |
| `ApiErrorCode` = `"tag_key_invalid" \| "tag_key_taken" \| "tag_name_taken" \| "tag_in_use" \| "table_tag_mask_conflict" \| "rule_overlap"` and narrowers `tableTagMaskConflict(e)`, `ruleOverlap(e)`, `tagInUse(e)`, `tagNameTaken(e)`, `tagKeyTaken(e)`, `tagKeyInvalid(e)` | `FE/routes/governance/apiErrors.ts` (Create) | read `ApiError.code` and `ApiError.body` (`FE/api/client.ts:4-27`) |
| `RuleEntry` = `{kind: "classification", tagKey, tagName}` \| `{kind: "table", catalogId, schema, table}` \| `{kind: "column", catalogId, schema, table, column, classifications}` \| `{kind: "edit", rule}`; `seedRule(entry)`, `ruleDraftKey(userId, entry)` | `FE/routes/governance/ruleDraft.ts` | drives C8's defaults and the draft key |

## 8. Accessibility and responsive summary

- Every disabled action has a focusable `aria-disabled` control and a visible reason tied by `aria-describedby` (FE-ACTION-4, N22).
- Required fields carry "(required)" and `aria-required`.
- Chips have two named focus stops. Swatches are `aria-hidden`.
- Warnings use tokens only (FE-VISUAL-1) and are checked for contrast in light and dark in the browser (FE-VISUAL-4, FE-TEST-2).
- 390 px: the Effect strip stacks; the list hides three low-priority columns; the Data columns table stacks into cards; the dialog footer is sticky.

## 9. Migration and compatibility

- **No database migration.** Name uniqueness is a handler check (C10). The other backend changes are read-side or error bodies.
- **Error bodies stay compatible.** Every coded body keeps the `error` sentence the frontend and any script already read. A script that matched `"conflict: a tag with that key already exists"` sees `"A classification with the key \"pii\" already exists (PII)."` instead: the old text is not kept, as nothing is deployed to real users (the 2026-10-20 ruling in the Ranger spec).
- **Existing duplicate names** (if any install has them) keep working. A rename of one of them to a free name is allowed; a rename to another existing name is refused. An Overview attention item for existing duplicates is not in this package (§10).
- **`PreviewResponse.effect` is optional,** so the Engine & policy `GrantDialog`, which previews grants, is untouched.
- **Tab URL keys** do not change.

## 10. Risks

- **Name check race.** Two concurrent creates of the same name can both pass the handler check. Accepted: it is an authoring guard, and the effect of a duplicate is the one that already exists today.
- **`effect` cost.** A tag rule's reach reads every assignment of that classification plus every mask rule. It is bounded: `alreadyMasked` is capped at 20 and coordinates are deduplicated. It runs only on Preview, not on list reads.
- **The model-level overlap (C6) can over-warn.** It reports two masks on one column even where the compiler would order them unambiguously (different priority bands). This is accepted, because it only warns and asks for a tick, while the server's 409 remains the refusal.
- **Draft store in module memory** is lost on reload, which is stated in the restore line. It could also restore a draft for a different user after a session switch in the same tab, so the draft key includes the signed-in user's id (`me.user_id`); another user's key never matches.
- **Coordination with U3** (known-issues programme): 403-in-shell, scroll cues and the sidebar (KI-225, KI-230, KI-231, KI-233, KI-234) belong to U3. If U3 lands first, C13.5 reduces to passing reasons to its component.

## 11. Related defects found (not fixed here unless said)

1. **`PUT …/resource-tags` with an unknown warehouse id answers 200** and writes an orphan row. A DELETE answers 200 too. See §6 for why it is not in this package. An unknown `tag_id` hits the foreign key and answers **500** "internal error" instead of 400/404 (`crates/egret-bi/src/catalog_tags.rs:84-190` → `BiError::Db`).
2. **StarRocks clearance reads NULL instead of refusing** for an uncleared reader of a masked column (`ClearanceTab.tsx:20-21`, `COMPILES_TO`). This is a pre-existing engine behaviour, stated in the UI. Not changed.
3. **`Badge variant="warning"/"success"/"processing"` use raw Tailwind palette classes** (`FE/components/ui/badge.tsx:20-25`), and white on `yellow-600` fails AA. It is a shared component (FE-TEST-3), so it is fixed separately. This package stops using it in the Clearance tab.
4. **The schema tree's gridcells have no accessible name** (N22, `FE/routes/governance/data/CatalogTree.tsx`). Outside this area.
5. **3-5 s skeletons** on tabs (N24): partly backend restarts during the walkthrough. Not measured; not addressed.
6. **E2**: SSO started from `127.0.0.1` ends on a raw "missing oidc state" 400 page. It belongs to the demo stack docs (non-goal).
7. **`table_level_attachment_of` reports only the first table** (`LIMIT 1`). C5 reports `moreTables` but not the list.

## 12. Open questions

1. **"vi translations" for UI strings cannot be done as written.** The frontend has no i18n layer, and FE-COPY-1 makes UI text English. This spec gives the glossary a Vietnamese column for docs only. Adding UI i18n would be its own package. Does the user want that?
2. **A unique index on `lower(tags.name)`.** That would make duplicates unrepresentable (C10 checks only in handlers). It needs a decision on installs that already hold duplicates: refuse the migration, or rename. Renaming silently changes which rules reach which data, because engines match by name.
3. **Name of the tab.** Decided: **"Manual policies"** (controller decision, Task 5), replacing "Hand-written" and the earlier proposal "Engine policies", which sat too close to the sidebar entry "Engine & policy".

## 13. Non-goals

- **The sidebar's one-item groups**: KI-225 in `docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md` (U3).
- **Verifying a mask per user** (N1): Package 2, outlined in §14.
- **StarRocks clearance NULL-vs-deny** (§11.2).
- **The resource-tags wrong-id check** (§6, §11.1).
- **The demo stack's SSO/localtest note** (E2).
- **Updating `egret-data/docs`** (§15).

## 14. Package 2 outline: verify a mask per user (N1)

Written later as its own spec. What it must deliver:

1. **The first line answers "not enforced"** before anything else. When no engine with a profile serves the catalog, say "Not enforced: <catalog> has no engine", and the per-engine lines are omitted (today this state exists as `no_profile_serves_the_catalog`, `routes.rs:526-537`).
2. **A per-user effect line per engine**: "member-sso reads email as **hashed** (rule *Hash personal data*)" / "as the real value (exempt by group data-team)" / "**refused** (clearance PII)". Row filters get the same treatment: "sees rows where region = 'us' (rule X)".
3. **Trino masks.** Explain evaluates masks only for StarRocks column reads today (`routes.rs:2347-2349`). The Trino tag service's mask policies are refused by the evaluator (`:723-726`). Package 2 has the evaluator read the tag service download for the target column's resolved tags and call `evaluate_data_mask` / `evaluate_row_filter` (which the evaluator already implements, per `ExplainResult.tsx:1340-1356`), on Trino and StarRocks alike.
4. **API sketch.** `ExplainProfile` gains:
   - `mask: Option<ExplainMask>` for every engine (it is StarRocks-only today);
   - `row_filter: Option<ExplainRowFilter { filter_expr, attribution }>`;
   - `effect_line: String`, built server-side so every surface says it the same way.

   `POST /perm/explain/principal` already fans out per member, so a group gets one line per member.
5. **UI.** The column panel gets **"Check a person"** next to Who can read, showing the effect line for one user. The rule dialog's preview can offer "Check with a person" after Create.

## 15. Documentation

The repo rule (`CLAUDE.md`) requires a user-visible change to update `egret-data/docs` (Platform section) with refreshed screenshots. **That update is deferred, pending the user's decision.** This package changes:

- the names of tabs, buttons and sections (C1);
- the new Effect strip;
- remove-in-place;
- the rule dialog;
- the Clearance tab;
- the classification error messages;
- one new API route (B4) and new response fields (B3, B5).

Each of these would need a docs change and new screenshots once the user decides. This spec and its Vietnamese mirror are the only docs this package writes.

## 16. Addendum: live walkthrough fixes (Task 13)

The live walkthrough (`../plans/2026-10-21-classification-ux-live-result.md`) found two Major gaps and a list of Minor ones. This section records what changed and why.

### 16.1 A stored rule says when it is not enforced

After Create, only the transient preview said "Not enforced yet". Every surface that shows a stored rule now says it, from the same computation the preview uses (`crates/egret-perm-compile/src/effect.rs`).

**API.**

- `GET /api/control-plane/perm/rules/{id}/effect` (`policy:read`) returns the stored rule's `RuleEffect`, the same DTO and the same function (`rule_effect`) as `effect` on `POST /perm/preview`. It runs with `editing = id`, so the rule is not "already masked" by itself. Unknown id: 404. It reads one `REPEATABLE READ, READ ONLY` snapshot.
- `GET /api/control-plane/perm/rules/enforcement?ids=a,b,…` (`policy:read`) returns `[{ruleId, enforcedBy, notEnforced}]` (`RuleEnforcement`), one entry per known id in the order asked; a repeated id is answered once, an unknown id is left out. At most **100** distinct ids per request (`RULE_ENFORCEMENT_CAP`); more, or an id that is not a UUID, is a 400.

**Why a separate endpoint, not a field on `GET /perm/rules`.** The list is read by every governance page and is unbounded; computing reach for every rule there would make each of those reads pay for a fact only four surfaces show. The enforcement endpoint is asked only for the rules a surface lists, with a cap. Both endpoints share `reached_catalogs`, `attached_engines` and `enforcement` with `rule_effect`, so they cannot disagree; a test asserts that the stored rule's effect equals its preview's effect field for field.

**UI.** `NotEnforcedBadge` (`frontend/src/routes/governance/NotEnforced.tsx`) says "Not enforced yet — <reason>" in words with an icon (never colour alone). The reasons are `notEnforcedReason` in `ruleEffect.ts`: "no engine attached", "its engines have no policy profile", "this classification is not attached to any data a catalog serves" or "no catalog egret knows serves this path". It is shown on the Effect strip's Rules fact (one line for several rules, for example "1 of 2 rules: …"), on each row of a classification's Rules tab and a table's Rules tab, beside each rule in the column panel, and on the rule page as an **Effect** detail with the preview's full sentences. Nothing is said while the answer is loading, failed or not permitted. Rule writes and classification attaches refetch it.

### 16.2 The steward job: 20 clicks to 15

- **Exempt** has no kind switch: one field searches users and groups at once (`list_principals` without `kind`), and each result names its kind. The old "Include" label is gone in this mode.
- **Add rule from a column**, or from a table whose columns carry classifications, starts on the column's own classification (Applies to = Classification, name "Mask <classification>"). Each other classification, and the column or table alone, is one click away ("Or apply it to …"). A whole-table classification is never the default, because a mask on it is refused (`table_tag_mask_conflict`).
- **Attach in Data** on a classification remembers it in memory, and the next Add classification dialog that offers it preselects it once. The memory is cleared on every identity switch.
- `StewardJobClickBudget.test.tsx` drives the job from the column panel to a created rule and pins it at 7 named clicks. The 8 clicks before the panel (Governance, Classifications, New classification, Create classification, Attach in Data, the Go-to-path box, Go, `email`) are the live count on pages this addendum does not change, so the job takes **15** clicks. Two clicks could still go: Enter in the Go-to-path box replaces its Go click, and a deep link from Attach in Data to a known table would replace the tree steps.

### 16.3 Smaller fixes

- **Who can read?**
  - A typed user is looked up by exact username first (`GET /perm/people?username=`). When no such user exists, the card says "No user named X." instead of a verdict.
  - The server's "no group is named X" reads as a sentence.
  - The suggestions sit in the flow, so they cannot cover Explain.
  - Explain is `aria-disabled` with its reason.
  - Group mode shows the shared "nothing is enforcing" line.
- **Rule dialog.**
  - After Preview, the result is scrolled to the centre and focused.
  - The classification-wide sentence shows only for a rule on a classification.
  - Focus returns to the opener after Discard.
  - At 390 px the reason takes its own row, so Cancel and Create stay side by side.
- **Focus after a page-removing delete.** After a rule or classification delete, focus lands on the new page's `h1` (`frontend/src/lib/focusArrival.ts`). It never stays on `<body>`, and focus the user has moved is never taken.
- **Classification delete.** The deleted classification's own reads (usage, Manual policies) are dropped from the cache instead of refetched, so there is no stray 404.
- **N22.** Add clearance and the Data area's Add classification on a non-egret catalog are `aria-disabled` with their reason. Create classification and the dialog's Add classification were already `aria-disabled` at the walkthrough's commit.
- **Wording.**
  - "Include" is gone.
  - "egret grant" is now "grant" in the Overview count and the column panel. The line "egret grants are above; manual policies below" keeps "egret", where it distinguishes them from manual policies.
  - "No rule selects it." is now "No rule uses it.", and the remove confirm leads with what happens to rules.
  - "every assignment of it" is now "takes it off every table and column it is attached to".
  - "per policy profile" and "every profile it reaches recompiles" are reworded.
  - Overview Recent changes says "attached classification X to <path>" with no trailing dot, and names a mask by its glossary label.
- **Tab name.** "Manual policies" is final (§4, §12.3).
- **Not done here.** N1 (Package 2) and N25 (U3). The classifications list's clipped actions column at 390 px is U3's scroll-cue work. The Overview rewording is done client-side from the server's sentence; moving it into `governance_changes.rs` is a follow-up.
