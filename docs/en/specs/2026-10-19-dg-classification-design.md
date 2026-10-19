# Data Governance frontend redesign #4b: Classification — design

Date: 2026-10-19. Status: design approved by the user on 2026-10-19 (C1 and sections 1–3 and testing, below); the rows
marked "default decision" are this document's. **Q1–Q5 are open**: each is a conflict between the approved design and
the code, found while checking the premises, and needs a user decision before the task it names starts. Each carries
a proposed answer that the plan assumes provisionally. Base: main `309405b1` (#4a Engine & policy merged as #697).
Branch `feat/dg-classification`, worktree `egret-dgcl`.

This is sub-project #4b of the programme spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` ("programme
spec"; decisions F1–F12) and the second half of #4 as split by #4a's EP1
(`docs/en/specs/2026-10-19-dg-engine-policy-design.md`). Part 1 of the programme spec gives the sidebar group
**Classification** "the tag and clearance catalogue", replacing Classifications and TagDetail; F12 adds that Data object
pages list the hand-written policies that reach an object, marked "hand-written", so they are never hidden. Every claim
about existing code carries a `file:line` from `309405b1`, read in this worktree.

## Premises checked against the code

The brief's survey was checked. Statements that were wrong or incomplete are listed first; the design follows the code.

**Wrong or incomplete**

1. **Catalog tags are keyed by warehouse, not by catalog, and only egret catalogs can carry them.**
   `catalog_tag_assignments` has `warehouse_id uuid NOT NULL` and no catalog column
   (`migrations/0081_catalog_tag_targets.sql:9-17`); every reader joins `registry.catalogs c ON c.warehouse_id =
   a.warehouse_id` (`crates/egret-policy/src/store.rs:1342`, `:1759`). Only an egret Iceberg catalog has a
   `warehouse_id` (`migrations/0147_catalog_registry.sql:24-33`, one catalog per warehouse `:37-38`), so an external
   catalog can never carry a tag (`crates/egret-controlplane/src/governance_tags.rs:15-19`). "External catalogs: DB
   only" in route C is therefore moot for Data. `warehouse_id` has **no foreign key**, so an assignment can outlive
   its catalog; route C reports such rows instead of dropping them (§4.4, CL11).
2. **No migration is needed.** `catalog_tag_assignments`' primary key is `(tag_id, warehouse_id, schema_name,
   table_name, column_name)` (`0081:17`) and `tag_assignments`' is `(tag_id, object_type, object_id)`
   (`migrations/0080_tags.sql`); both lead with `tag_id`, so a read by tag is an index scan. CL15 records the rule if
   that changes.
3. **TagDetail carries three warnings and a limit note, not two warnings.** Groups not enforced in the engine
   (`frontend/src/routes/governance/TagDetail.tsx:343-364`, shown with a required group), reads only on both engines
   (`:366-386`), Trino/StarRocks view coverage (`:387-401`), and the StarRocks 500-column expansion limit (`:402-408`),
   the last three shown with a required role. All four move to the Clearance tab unchanged (§5.2).
4. **A Ranger item naming a group or a role matches nobody on an engine.** egret runs no usersync, writes no
   `group-provider.properties` and serves no roles download, so a hand-written item naming group `g` or role `r`
   grants or denies nobody; only bare usernames and the group `public` match
   (`crates/egret-perm-compile/src/members.rs:3-40`, measured as D7; `crates/egret-perm-compile/src/clearance.rs`
   module doc, D0: "a Ranger policy item naming a role matches nobody", and an unmatched item in `denyExceptions`
   excepts nobody). The compiler expands egret's own group and role principals to usernames; a hand-written policy is
   not compiled, so nothing expands its groups or roles. The approved "via group X" / "via role Y" marking on route B
   would describe an egret membership the engine never applies (Q1).
5. **Server-side membership exists, in `egret-perm-compile`, not `egret-perm`.** `read_user_via`
   (`crates/egret-perm-compile/src/people.rs:788`) returns `StandsFor { kind, ref, via, on_engines, not_on_engines }`
   with `Via::{Itself, Group, Role, Workspace, Public, GroupRole, GroupWorkspace}` (`:294-314`, `:356-362`); the
   People pages read it as `standsFor` (`frontend/src/routes/governance/people/tabs/PermissionsTab.tsx:169`,
   `people/reach.ts:33`). It describes how **egret grant** principals reach a user. Route B reuses it to find the
   user's username, groups and roles.
6. **An ANCESTOR match is not "applies".** `egret_ranger_eval::resource::MatchType` is `None | Self_ | Descendant |
   Ancestor | SelfAndAllDescendants` (`crates/egret-ranger-eval/src/resource.rs:47-67`), and for a resource request
   Ranger evaluates a policy only on `SELF` or `SELF_AND_ALL_DESCENDANTS` (`resource.rs:14-28`, measured live: a grant
   compiled to `{catalog, schema}` refused a table select). A policy written on a schema with wildcards below
   (`ice.s.*.*`) is `SelfAndAllDescendants` for a table in `s` and applies; a policy naming only `{catalog, schema}`
   is `Ancestor` and does not apply to the table's data. The approved "parent catalog/schema as applies" holds only for
   the first shape (Q2).
7. **Route C's approved path collides in shape with an existing route.** `GET
   /api/control-plane/governance/tags/{catalog_id}` already exists, keyed by a **catalog** id
   (`governance_tags.rs:35`, `:62-64`). `…/governance/tags/{id}/usage` keyed by a **tag** id would not clash in axum
   (different segment counts) but makes one path prefix mean two ids. The tag-keyed precedent is
   `GET /api/control-plane/classifications/{id}/view-exposure` (`crates/egret-controlplane/src/view_exposure.rs:209-211`)
   (Q3).
8. **Tab (5) "Hand-written tag policies" has no approved route, and C2 forbids matching on the client.** A tag policy
   names its tag in `resources.tag`, matched by the tag service-def's matcher (`wildCard: true`, `ignoreCase: false`,
   `crates/egret-policy/src/service-defs/tag.json:21`, `:53`, `:117`); doing that in the browser would be a second
   matcher (Q4).
9. **Data's Masks & filters tab has no hand-written note today.** The note `EgretGrantsOnlyNote`
   (`frontend/src/routes/governance/tabParts.tsx:69-102`) is rendered on Data `PermissionsTab.tsx:135`, Data
   `ColumnPanel.tsx:336`, People `PermissionsTab.tsx:176` and People `MasksFiltersTab.tsx:69`. The Data Masks & filters
   tab gains the new section without a note to replace.
10. **`enabled` is a column, and the body's `isEnabled` can be stale.** The download injects `isEnabled` from
    `policy.authz_policies.enabled` (`store.rs:2187-2211`); #4a's live check found `PUT /policies/{id}/enabled` leaves
    `policy_jsonb.isEnabled` unchanged (`docs/en/plans/2026-10-19-dg-engine-policy-live.md` §2.4). Routes A, B and D
    read the column.
11. **The delete 409 comes from one foreign key.** `perm.rules.tag_key REFERENCES tags(key)` with no `ON DELETE`
    refuses the delete while a rule selects the tag's **key** (`crates/egret-bi/src/tags.rs:322-333`, `:364-381`). A
    rule written on another tag whose name matches (`TagRulesSection.tsx` `reachText`) does not block. The delete also
    cascades both assignment tables and `classification_policies` and recompiles in the same transaction
    (`tags.rs:309-388`); its 204 carries no skip report.
12. **Two query keys for one list.** The pages read tags through hand-written `apiJson` hooks keyed `["tags"]`
    (`frontend/src/api/tags.ts:41-95`); orval also generates `useControlPlaneTagsListTags`
    (`frontend/src/api/generated/control-plane-tags/control-plane-tags.ts:367`) with its own key. A page reading one
    and a mutation invalidating the other is #4a's defect D4 (stale list until reload). CL16 fixes one key.
13. **The classification pages are not lazy.** `Classifications` and `TagDetail` are imported statically
    (`frontend/src/App.tsx:199-200`) and routed at `:652-653`; #4b makes them a lazy area with a bundle guard
    (FE-ROUTE-2).
14. **The engine matches tag names; rules and the key are not names; names are not unique.** `tags.key` is `UNIQUE`,
    `tags.name` is not (`0080`); the tag download fills a tag's type from `tags.name` and explain resolves
    `tags.name` (`store.rs:1671-1677`, `:1214`), while `perm.rules` select `tags.key`. Two tags with one name share every
    engine tag policy on that name. Tab (5) says so when it happens (CL10).
15. **Hand-written policies can carry constructs egret does not evaluate.** `RangerPolicy.unmodelled`
    (`crates/egret-ranger-eval/src/model.rs:154-158`, variants `:403-…`: conditions, validity schedules, zones) and
    `isDenyAllElse` (`model.rs:145`). The resource match stays exact; the rows show these constructs so "applies"
    is not read as "decides" (CL9).
16. **explain's request builder is private.** `request_resource(engine, catalog_name, target)`
    (`crates/egret-perm-compile/src/routes.rs:2957-2986`) builds the request with `registry.catalogs.name` from
    `catalogs_for_service_via` (`routes.rs:2270-2280`, `store.rs:1363-1375`) and the level names of
    `resource_level_names` (`crates/egret-perm-compile/src/grants.rs:218-223`; StarRocks' second level is `database`).
    Route A must build the same request, so it moves to a shared module rather than being copied (CL7).
17. **A BI read check is one PDP call plus two reads per object.** `Pdp::authorize` takes one subject, action and
    resource (`crates/egret-core/src/authz.rs:70`, `:108-124`); there is no batch. `object_ref` reads the object's
    folder and workspace (`crates/egret-bi/src/shares.rs:116-135`) and a folder's gate reads its `entity_type`
    (`shares.rs:81-98`); `read_action` is private (`shares.rs:58-67`). A `WorkspaceContext` is not needed: the PDP is
    asked about the principal's subject and the object. The cost per object is measured in Task 1 (M4).
18. **Survey line numbers.** `usable_tag_services_via` is at `store.rs:1407` (not 1404); `resolved_tags_for_resource_via`
    at `store.rs:1696` (not ~1666).

**True as stated** (checked): routes `App.tsx:652-653`; `Classifications.tsx` 443 lines, `TagDetail.tsx` 630; tags API in
`crates/egret-controlplane/src/tags.rs` (`GET` `tag:read` `:111`; `POST`/`PATCH`/`DELETE` `tag:manage` `:156`/`:191`/`:222`;
access-policy `PUT`/`DELETE` `:291`/`:334` answering `{ok, skipped[]}`); `view_exposure.rs:211`; tables 0080
`tags`/`tag_assignments`, 0081 `catalog_tag_assignments` (`column_name = ''` is the table; only tables and columns,
0177), 0082/0083 `classification_policies`; clearance compiles deny only and a groups-only gate compiles to nothing,
reported skipped (`clearance.rs`); `covers` is case-insensitive (`crates/egret-perm/src/eval.rs:303-352`);
`services_for_catalog_via` `store.rs:1471`; `PolicySummary` `crates/egret-policy/src/admin/mod.rs:276-345`
(`resource_path` joins the first value of each level); hand-written = `managed_by = 'operator'` (#4a premise 1); Cedar
has only `tag:read`/`tag:manage` for tags (`crates/egret-core/src/rbac.rs:276-287`); `data_steward` holds both (0080
seed); `TagDto.assignment_count` counts BI objects only (`crates/egret-bi/src/tags.rs:36-38`).

## Decisions

| id | decision | by |
|---|---|---|
| C1 | The tag page gets a **Where used** tab covering both Data and BI | user, 2026-10-19 |
| C2 | Hand-written reach is computed with `egret-ranger-eval`'s matcher (`match_policy_resources` / `PreparedPolicyResources`, `resource.rs:147-262`), never a hand-rolled matcher | default decision (2026-10-19) |
| CL1 | Sidebar group **Classification** kept, gated `tag:read`. `/governance/classifications` index: tag table (name + colour, key, description, tables/columns carrying it, BI objects, clearance yes/no with role and group counts), summary strip (total, with clearance, unused), create (name → derived key; `tag:manage` or platform admin) | user, 2026-10-19 (section 1) |
| CL2 | `/governance/classifications/:id`: header (name, colour, key, description, edited inline) and tabs **Where used · Clearance · Mask & filter · View exposure · Hand-written**; delete confirmation states consequences and asks for the key; 409 handling kept; URLs unchanged; every existing link keeps working | user, 2026-10-19 (section 1) |
| CL3 | Route A `GET /api/control-plane/governance/objects/{catalog_id}/handwritten?schema=&table=&column=` (`policy:read`) and route B `GET /api/control-plane/governance/principals/{kind}/{id}/handwritten` (`policy:read`); Data and People get a "Hand-written policies (N)" section; without `policy:read` an explanation | user, 2026-10-19 (section 2) |
| CL4 | Route C (tag usage, `tag:read` OR `policy:read`) with a 500-check cap that says what it did not check; `GET /tags` gains the Data counts additively; no migration (premise 2) | user, 2026-10-19 (section 3) |
| CL5 | Testing as §9: DB tests per route, a differential test of route A against `POST /perm/explain`, vitest red-first, bundle guard `test:bundle:governance-classification`, live check with real Trino and StarRocks, predictions first | user, 2026-10-19 |
| CL6 | Out of scope: tag-assignment permissions (Data `catalog_registry:update`, BI per object) unchanged; the rule→`tags.key` vs engine→`tags.name` mismatch recorded as a follow-up | user, 2026-10-19 |
| CL7 | **One matching rule, in one module.** `crates/egret-perm-compile/src/handwritten.rs` owns `object_request` (moved from `routes.rs` `request_resource`, which explain then calls), `object_match` (a policy's `MatchType`s → `applies` / `part` / `parent` / `none`, §4.1) and `item_summary`. Routes A, B and D call it; no other file decides a match | default decision (2026-10-19) |
| CL8 | Route handlers live in `crates/egret-controlplane/src/governance_handwritten.rs` (A, B, D) and `governance_tag_usage.rs` (C), tag `control-plane-governance`; DB work in `egret-perm-compile` (A, B, D) and `egret-bi` (C) | default decision (2026-10-19) |
| CL9 | A row shows `isDenyAllElse` and every `unmodelled` construct as a sentence ("Has conditions egret does not evaluate"); the match is still reported, and the row never claims the policy decides | default decision (2026-10-19) |
| CL10 | Tab (5) states when another tag has the same name: "Tag K2 has this name too; engine tag policies on this name apply to both." | default decision (2026-10-19) |
| CL11 | Data rows of route C whose warehouse no catalog names are counted: "N assignments are in a warehouse no catalog names." BI rows whose object no longer exists are counted: "N assignments point at deleted objects." Neither is dropped silently | default decision (2026-10-19) |
| CL12 | Route C checks BI objects in a fixed order (workspace id, kind, object id) up to 500 per call; the rest is `{remaining: N}` and the tab says "Not all BI objects were checked: N remain." with no "show more" in #4b | default decision (2026-10-19) |
| CL13 | Saved queries and folders have no detail page: a readable one shows its name and kind without a link; datasets, charts, dashboards and apps link to `/explore/<workspace slug>/<kind path>/<id>` (Task 1 M6 confirms) | default decision (2026-10-19) |
| CL14 | The delete dialog's rule count is the rules whose selector key is this tag's key (the FK blockers, premise 11); without `policy:read` it says "Rules: needs policy:read to count". With rules > 0 it says the delete will be refused and keeps the button (the server decides; the 409 toast stays) | default decision (2026-10-19) |
| CL15 | No migration. If a later change needs one, its number is chosen with `ls migrations \| tail -3` right before merge (parallel workflow PRs take numbers) | user, 2026-10-19 |
| CL16 | One query key per list: the pages keep `useTags` (`["tags"]`); the `Tag` type gains the new fields; every tag mutation (legacy hooks and new dialogs) invalidates `["tags"]` **and** route C's key for that tag. New routes A–D use orval hooks (FE-DATA-2) | default decision (2026-10-19) |
| CL17 | The tag page's tab is in `?tab=` (`where-used` default, `clearance`, `rules`, `views`, `handwritten`); links that mean a tab pass it: Engine & policy's clearance source and People Clearances → `?tab=clearance`; People RulePage → `?tab=rules` | default decision (2026-10-19) |
| Q1 | **Open.** Route B "via group X / via role Y" (premise 4). Proposed: list such items marked "Names group X, which this user is in. Engines resolve no groups, so on the engine it reaches nobody." (role likewise), in their own group below the direct ones; route A's item summary marks every group and role principal the same way; `public` and `{USER}` are "applies to everyone" | needs user decision |
| Q2 | **Open.** Match levels (premise 6). Proposed: `Self_` or `SelfAndAllDescendants` → **applies to this object** (with "through a pattern" when a level matched by wildcard, exclude or recursive); `Descendant` → **applies to part of it**; `Ancestor` → listed as **set on a parent** with "Names <catalog[.schema]> only; engines do not apply it to this object's data." For a tag policy: the tag on the object or an ancestor → applies; on some columns below → part | needs user decision |
| Q3 | **Open.** Route C's path (premise 7). Proposed: `GET /api/control-plane/classifications/{id}/usage`, beside `…/{id}/view-exposure` | needs user decision |
| Q4 | **Open.** Tab (5)'s data (premise 8). Proposed: route D `GET /api/control-plane/classifications/{id}/handwritten` (`policy:read`): every usable tag service's `operator` policy whose tag resource matches this tag's name through the tag service-def matcher, with the profiles and engines that download it | needs user decision |
| Q5 | **Open.** Unreadable BI objects "grouped by workspace count" leaks how many workspaces hold a tagged object the viewer cannot read. Proposed: keep it (the approved wording), say only "N objects in M workspaces you cannot read", never a workspace name | needs user decision |

## 1. What exists

- **Pages.** `Classifications.tsx`: `FilterableTable` (Tag, Key, Description, BI objects, Gated badge), create dialog
  with `slugifyTagKey`, row menu delete; gated `platform_admin` or `tag:manage`. `TagDetail.tsx`: header (swatch, name,
  key, Gated badge, BI count), Details form auto-saving on blur, Clearance (`ClearanceTable`: one table of required
  roles and groups, written whole on every add/remove through `PUT`/`DELETE …/access-policy`; a non-empty `skipped`
  shows "Saved, but not compiled into any engine: …", `:131-164`), `ViewExposureSection`, `TagRulesSection`
  (`policy:read`), delete with `confirmText="delete"` (`:428-446`) and `toastTagDeleteFailure` (`tagDeleteFailure.ts`).
- **Routes.** `GET/POST /control-plane/tags`, `PATCH/DELETE /control-plane/tags/{id}`,
  `PUT/DELETE /control-plane/tags/{id}/access-policy` (`tags.rs`); `GET /control-plane/classifications/{id}/view-exposure`;
  BI object tags `GET/PUT/DELETE /ws/{ws}/objects/{kind}/{id}/tags[/{tag_id}]` (`shares.rs:544-666`); Data object tags
  `GET /governance/tags/{catalog_id}?schema=&table=` (`governance_tags.rs`); the rules list `GET /perm/rules`.
- **Links in.** `overview/refUrl.ts:44,46`, `overview/AttentionSection.tsx:111`, `engines/tabs/PoliciesTab.tsx:64`,
  `people/tabs/ClearancesTab.tsx:117`, `people/RulePage.tsx:154`; `/control-plane/classifications/*` →
  `LegacyGovernanceRedirect` (`App.tsx:687`).
- **Hand-written policies.** `policy.authz_policies` rows with `managed_by = 'operator'`; the main profile and its tag
  service (`usable_tag_services_via`, `store.rs:1407`); a catalog's profiles (`services_for_catalog_via`, `:1471`);
  the engine page's Advanced tab and editor URLs (`frontend/src/routes/governance/engines/engineUrl.ts:39-55`).

## 2. Information architecture

| URL | page | guard |
|---|---|---|
| `/governance/classifications` | index | `tag:read` |
| `/governance/classifications/:id?tab=` | tag page; `tab` ∈ `where-used` (default), `clearance`, `rules`, `views`, `handwritten` | `tag:read` (tabs `rules` and `handwritten` need `policy:read`, else an explanation) |

Both live in a lazy area `frontend/src/routes/governance/classifications/` (`ClassificationsRoutes.tsx`) mounted at
`classifications/*` inside `LazyRouteArea` under `RequirePermission perm="tag:read"`, with bundle guard
`npm run test:bundle:governance-classification` (`frontend/scripts/assert-governance-classification-lazy.mjs`), listed
in FE-ROUTE-2, FE-TEST-4 and `frontend/CLAUDE.md`. Sidebar unchanged (`Sidebar.tsx:113-114`). `Classifications.tsx`
and `TagDetail.tsx` are deleted once their behaviour has moved (their tests ported, not dropped).

## 3. The index (`/governance/classifications`)

`ListPageShell` (FE-PAGE-1): `h1` "Classifications", toolbar **New classification** (`tag:manage` or platform admin),
`SummaryStrip`: "N classifications · N with clearance · N unused" (unused = no Data and no BI assignment; FE-COPY-4
plurals). `FilterableTable` columns:

| column | source |
|---|---|
| Name (swatch + link) | `TagDto.name`, `.color` |
| Key | `.key`, mono |
| Description | `.description` or "—" |
| Data | "T tables · C columns" from `TagDto.catalog_tables`, `.catalog_columns` (new, §4.4); "—" when both 0 |
| BI objects | `.assignment_count` |
| Clearance | "None", or "R roles · G groups" with a lock icon and the word (FE-A11Y-4) |

Create dialog (FE-PAGE-3, one to three fields): Name, Key (derived from the name with `slugifyTagKey` until edited, then
kept), Description, Colour. A 409 (duplicate key) shows the server's message beside Key, dialog open (FE-ACTION-3).
Success navigates to the new tag page (no toast, FE-ACTION-2). The row menu keeps Delete, through the same dialog as the
tag page (§5.6). Empty: "No classification yet." with New classification when allowed; filter miss says so with Clear.

## 4. Backend

### 4.1 The matching rule (CL7, Q2)

`crates/egret-perm-compile/src/handwritten.rs`:

```rust
pub enum ObjectLevel { Catalog, Schema, Table, Column }
pub struct ObjectTarget { pub catalog_id: Uuid, pub schema: Option<String>, pub table: Option<String>, pub column: Option<String> }
pub fn object_request(engine: EngineType, catalog_name: &str, target: &ObjectTarget) -> RequestResource; // explain's request_resource, moved
pub enum ObjectMatch { Applies { through_pattern: bool }, Part, Parent, None }
pub fn object_match(prepared: &PreparedPolicyResources, policy: &RangerPolicy, request: &RequestResource) -> ObjectMatch;
pub struct ItemSummary { pub bucket: Bucket, pub users: Vec<String>, pub groups: Vec<PrincipalMark>, pub roles: Vec<PrincipalMark>,
                         pub everyone: bool, pub accesses: Vec<String>, pub mask_type: Option<String>, pub row_filter: Option<String> }
pub fn item_summary(policy: &RangerPolicy) -> Vec<ItemSummary>;
```

- `object_match` takes the best verdict over every resource map (`ResourceMapSource::Primary` and each
  `Additional(i)`): `Self_`/`SelfAndAllDescendants` > `Descendant` > `Ancestor` > `None`, mapped as Q2.
  `through_pattern` is true when a policy value at a level the request names is not that literal value (fold-equal),
  i.e. matched by `*`/`?`, an exclude or `isRecursive`.
- `explain` calls `object_request` instead of its private copy (no behaviour change; its tests stay green unedited).
- `Bucket` ∈ `allow`, `deny`, `allow_exception`, `deny_exception`, `mask`, `row_filter`. `everyone` is true for the
  group `public` or the user `{USER}`; `PrincipalMark` carries the name and `on_engines: false` for every group but
  `public` and every role (premise 4).

### 4.2 Route A: hand-written policies reaching a Data object (CL3)

`GET /api/control-plane/governance/objects/{catalog_id}/handwritten?schema=&table=&column=`, `policy:read` (403
otherwise). 400 for `table` without `schema`, `column` without `table`, or an empty value (browse's rule). 404 for an
unknown catalog. One `REPEATABLE READ, READ ONLY` snapshot.

1. Profiles: `services_for_catalog_via(catalog_id)`, keeping those `engine_type_for` names (Trino, StarRocks); their tag
   services: `usable_tag_services_via`. Engines per profile from `engine_mgmt.engines` (`id`, `name`, `type`, `origin`).
   External engines are read from the DB only; nothing is dialled.
2. Not determinable → `{ state: "not_determined", reason }`, never an empty list: `no_engine_attached` (no attachment),
   `no_profile` (attached engines bind no profile), `catalog_not_in_profile` (`catalogs_for_service_via` of every
   profile lacks the catalog).
3. Resource policies: every row of each profile with `managed_by = 'operator'` (disabled included, `enabled` from the
   column, premise 10), parsed through `ServicePolicySet` with the profile type's service-def
   (`get_service_def_via`); request = `object_request(engine, catalog name from catalogs_for_service_via, target)`;
   `object_match` ≠ `None` → a row.
4. Tag policies (Trino profiles with a tag service): the object's tags — for a table or column,
   `resolved_tags_for_resource_via` (table tags apply to its columns; ancestor), plus for a table, schema or catalog the
   tags on things below it (`tag_assignment_coordinates_via` / the assignment rows under the target) → "part". Each tag
   policy of the tag service (`operator` only) whose tag resource matches a tag name through the tag service-def's
   `PreparedPolicyResources` (request `{tag: <name>}`) → a row with `viaTag {name, on: "this" | "parent" | "below",
   column?}`; `this`/`parent` → applies, `below` → part.
5. Seeded: `managed_by = 'infra'` rows of the same profiles that match → counted per profile, not listed.
6. A row whose body cannot be parsed is listed with `match: "unknown"` and the parse error; never dropped.

```text
{ catalog: { id, name },
  target: { schema?, table?, column? },
  state: "listed" | "not_determined",
  reason: "no_engine_attached" | "no_profile" | "catalog_not_in_profile" | null,
  profiles: [ { serviceId, name, type, tagService: string|null,
                engines: [ { id, name, type, origin } ] } ],
  policies: [ { id, name, service, inTagService: bool, policyType: "access"|"masking"|"rowfilter",
                match: "applies"|"part"|"parent"|"unknown", throughPattern: bool,
                viaTag: { name, on: "this"|"parent"|"below", column: string|null } | null,
                resourceSummary: string, enabled: bool, denyAllElse: bool, unmodelled: [string],
                items: [ ItemSummary ], parseError: string|null } ],
  seeded: [ { service, count } ] }
```

Rows sorted: `applies` before `part` before `parent` before `unknown`, then profile name, then policy name.

### 4.3 Route B: hand-written policies naming a principal (CL3, Q1)

`GET /api/control-plane/governance/principals/{kind}/{id}/handwritten`, `policy:read`. `kind` ∈ `user` (`id` = user
uuid), `group`, `role` (`id` = name); 400 otherwise; 404 for an unknown principal (the People routes' rule).

- Identities: a user's `users.username` and, from `read_user_via`, the groups and roles it holds (`Via::Group`,
  `Via::Role`; not `GroupRole`/`GroupWorkspace`, which are egret-only twice over). A group: its name. A role: its name.
- Every `operator` policy of every profile and tag service whose items name the identity: `users` contains the
  username → `direct`; `groups` contains a group of the user (or the group itself on a group page) → `group` (Q1
  marking); `roles` likewise → `role`; `public`/`{USER}` → collected separately as `everyone`, never attributed.
- A user with no username: `{ state: "no_username" }` with only the `everyone` count, because no item can name them on
  an engine (`NotOnEngines::NoUsername`, `people.rs:328-330`).

```text
{ principal: { kind, id, label, username: string|null },
  state: "listed" | "no_username",
  policies: [ { id, name, service, inTagService, policyType, resourceSummary, enabled, denyAllElse, unmodelled,
                naming: [ { bucket, how: "direct"|"group"|"role", via: string|null, onEngines: bool } ],
                engines: [ { id, name } ] } ],
  everyone: { count, policies: [ { id, name, service, bucket } ] } }
```

### 4.4 Route C and the `/tags` counts (CL4, Q3)

`GET /api/control-plane/classifications/{id}/usage` (Q3; the approved path otherwise), gated `tag:read` OR
`policy:read` (`require_governance_read`, `governance_browse.rs:337-348`). 404 for an unknown tag.

- **Data**: `catalog_tag_assignments` by `tag_id`, joined `LEFT` to `registry.catalogs` on `warehouse_id`. Grouped by
  catalog then schema then table: `{ catalogId, catalogName, schema, table, onTable: bool, columns: [string] }`;
  rows whose warehouse no catalog names → `orphaned` count (CL11).
- **BI**: `tag_assignments` by `tag_id`, ordered `(workspace_id, object_type, object_id)` after resolving each object's
  workspace; for the first 500: `object_ref` and the gate kind, then `Pdp::authorize(subject, read_action, object)`.
  Readable → `{ kind, id, name, workspaceSlug }`; unreadable → counted with the distinct workspace count (Q5); missing
  object → `deleted` count (CL11); beyond 500 → `remaining` (CL12). `egret-bi` gains a `pub fn read_action_for_gate`
  (the private `read_action`) and a `pub async fn object_name` so the route does not copy them.

```text
{ tag: { id, key, name },
  data: { tables: [ { catalogId, catalogName, schema, table, onTable, columns: [string] } ],
          tableCount, columnCount, orphaned },
  bi: { readable: [ { kind, id, name, workspaceSlug } ],
        unreadable: { count, workspaces }, deleted, checked, remaining } }
```

**`GET /control-plane/tags`** gains two fields per `TagDto`, additively and snake_case like the DTO
(`tags.rs:38-61`): `catalog_tables` (distinct `(warehouse, schema, table)` with a table-level row) and
`catalog_columns` (rows with `column_name <> ''`), one grouped query joined in memory as the policies overlay is
(`tags.rs:113-133`). `assignment_count` unchanged. Create/update responses carry zeros (they already leave clearance
empty, `tags.rs:52-53`).

### 4.5 Route D: hand-written tag policies of a classification (Q4)

`GET /api/control-plane/classifications/{id}/handwritten`, `policy:read`. Every Trino profile with a usable tag service;
each tag service's `operator` rows whose tag resource matches this tag's **name** (tag service-def matcher, request
`{tag: name}`); per row the profiles (and engines) whose tag service it is, `items`, `enabled`, `unmodelled`; plus
`sameName: [ { id, key } ]` for other tags with this name (CL10). No tag service anywhere →
`{ state: "no_tag_service" }` ("No engine profile has a tag service, so no engine downloads tag policies.").

## 5. The tag page (`/governance/classifications/:id`)

**Header.** Swatch, name (`h1`), key (mono badge), description; "Clearance" badge with a lock when gated. With
`tag:manage` or platform admin: **Edit** opens an inline form (name, colour, description; Save/Cancel, not
auto-save-on-blur; `useUnsavedChangesWarning`, FE-ACTION-5) — a 4xx keeps the form with the server's message; and
**Delete** (§5.6). Unknown id → "No classification with this id" and a link to the index (not a generic error). A
failing tags request → error with Retry; never "not found".

**Tabs** (`ui/tabs`, `?tab=`, FE-PAGE-2): Where used · Clearance · Mask & filter · View exposure · Hand-written. Each has
its own skeleton, error (server message, Retry) and empty state (FE-STATE-1–3); one failing request never blanks
another tab.

### 5.1 Where used (C1)

From route C. Two sections.
- **Data**: grouped by catalog → schema; each table row: "Table" when `onTable`, and "Columns: a, b" when columns are
  tagged; links to `/governance/data/<catalog>/<schema>/<table>` (and `?column=` per column). `orphaned` → its
  sentence. Empty: "No table or column carries this classification."
- **BI**: readable objects (kind icon and word, name, workspace, link per CL13); then "N objects in M workspaces you
  cannot read." (Q5) when `unreadable.count > 0`; `deleted` sentence; `remaining > 0` → "Not all BI objects were
  checked: N remain." Empty: "No BI object carries this classification."
- A failed request shows the error with Retry, never the empty sentences (#4a lesson: a load failure is not a fact).

### 5.2 Clearance

The current clearance editor, behaviour unchanged: one table of required roles and groups, add by type + searchable
picker, remove per row; writes `PUT`/`DELETE …/access-policy`; the `skipped` outcome shown in the tab ("Saved, but
not compiled into any engine: …"). The four statements of premise 3, each under its existing condition. A new line
under the table, shown when a role is required: "Compiles to one deny tag policy in each Trino profile's tag service,
and on StarRocks to a select deny per tagged column." Picker focus returns to its trigger on close (#4a D3).

### 5.3 Mask & filter

`TagRulesSection` as today, each rule linking to its rule page (`/governance/rules/:id`). Without `policy:read`: "Rules
are read with the policy:read permission, which this account does not have."

### 5.4 View exposure

`ViewExposureSection` as today (`tag:read`).

### 5.5 Hand-written (Q4)

From route D. Rows: policy name → the engine's editor URL (`editorUrl({engineId}, kind, id)` with `?scope=tag`, or
the profile page's for an unbound profile), engines, type, items (principals with the Q1 marks, accesses / mask type /
filter), enabled, `unmodelled` sentences; `sameName` sentence (CL10). Without `policy:read`: the explanation. Empty:
"No hand-written tag policy names this classification."

### 5.6 Delete (CL2, CL14)

`ConfirmDeleteDialog` with `confirmText` = the tag's key. Body, from route C and the rules list:
"Deleting K removes it from T tables and C columns, B BI objects (and U you cannot read), and removes its clearance
(R roles, G groups). N rules select it: the delete will be refused until they are deleted or retargeted." Unknown
parts say why (no `policy:read` → the rules line of CL14; route C failed → "Usage could not be read: <message>").
Success: navigate to the index (no toast). 409: `toastTagDeleteFailure` as today, dialog closed, focus back on Delete.

## 6. Hand-written on Data and People (CL3)

A shared `HandWrittenSection` (`frontend/src/routes/governance/handwritten/HandWrittenSection.tsx`, in the Data and
People chunks through their own imports): title "Hand-written policies (N)", each row with a "Hand-written" badge, the
match level word ("Applies to this object", "Applies to part of it", "Set on a parent", "Could not be matched"), "through
a pattern" when set, the item summary, enabled, and a link to the engine's Advanced editor. Seeded counts: "N
egret-seeded policies also reach this object; see Engine & policy." with a link to each engine page's Policies tab.
`not_determined` → its reason sentence. Without `policy:read` → "Hand-written policies are read with the policy:read
permission, which this account does not have." Error → error with Retry, never "None".

- **Data**: `PermissionsTab` (access rows), `MasksFiltersTab` (masking and row-filter rows) and `ColumnPanel` (all,
  for the column) show the section below egret grants/rules, replacing `EgretGrantsOnlyNote` with: "egret grants are
  above; hand-written policies below. To see who can actually read, use Who can read?" (the link as today).
- **People**: `PermissionsTab` (access) and `MasksFiltersTab` (masking, row filter) from route B, same section, rows
  grouped Direct · Through a group or role (Q1) · Applies to everyone (count, expandable).

## 7. Links

URLs are unchanged, so every link keeps working; tab-meaning links gain `?tab=` (CL17): `engines/tabs/PoliciesTab.tsx:64`
and `people/tabs/ClearancesTab.tsx:117` → `?tab=clearance`; `people/RulePage.tsx:154` → `?tab=rules`.
`overview/refUrl.ts:44,46` and `AttentionSection.tsx:111` unchanged.

## 8. Errors and states

As #4a: each tab and section owns its states; an error is never rendered as empty; empty sentences state the fact;
controls without permission are hidden or say why (FE-ACTION-4); writes invalidate exactly the keys they change (CL16)
and any 2xx is success (#4a D2: create answers 201); dialogs return focus to their opener (#4a D3).

## 9. Testing (CL5)

- **Backend DB tests** (`egret_test_db::url()`, fixtures scoped to their own ids, real `CedarPdp`):
  - `handwritten.rs` (pure): `object_match` per `MatchType` and map source, `through_pattern` for `*`, `?`, exclude,
    recursive and a case-differing literal (not a pattern); `item_summary` per bucket, `public`/`{USER}` →
    `everyone`, groups and roles `on_engines: false`.
  - Route A: wildcard schema → applies (through a pattern); exclude excluding the table → none; recursive; case;
    `{catalog, schema=s, table=*, column=*}` → applies for `s.t`; `{catalog, schema}` only → parent (Q2); a policy on
    one column → part on the table and applies on that column; a tag policy via a table tag → applies on the table and
    on its column; via a column tag → part on the table; seeded rows counted only; a disabled row listed with
    `enabled: false` although its body says `isEnabled: true`; an unparsable body → `unknown`; catalog with no engine →
    `no_engine_attached`; engines without a profile → `no_profile`; an external engine's profile listed, nothing
    dialled; 403 without `policy:read`; 400s; 404.
  - **Differential**: for a fixture of hand-written policies only (allow, deny, mask) on Trino and StarRocks profiles
    and for each (object, user) pair of a grid, `POST /perm/explain`'s deciding hand-written policy (if any) is in route
    A's `applies` set for that object; and every enabled `applies` allow policy whose item names the user is either
    explain's decider or outranked by a decider explain names. Unmodelled fixtures excluded and listed.
  - Route B: direct; via group and via role marked per Q1; `public` and `{USER}` → `everyone` only; a user without
    username → `no_username`; group and role pages; 403; 404.
  - Route C: readable and unreadable BI objects for a user lacking a workspace; a gated BI object unreadable without
    clearance; the cap (501 objects → `checked: 500, remaining: 1`); deleted object → `deleted`; orphaned warehouse →
    `orphaned`; 403 with neither permission, 200 with `tag:read` only and with `policy:read` only; `/tags` counts
    (table-level and column rows; unrelated tags 0) with the existing tag-route tests green unedited.
  - Route D: name match, wildcard tag value, case (no fold: `ignoreCase: false`), `sameName`, no tag service.
- **Frontend vitest**, red first per task: routes and lazy area; index columns, summary strip, create (key derived,
  409 beside Key), filter miss; tag page header edit (save, cancel, 4xx kept, unsaved warning), not-found vs error;
  each tab's loading/empty/error, `?tab=`; Where used readable/unreadable/remaining/orphaned/deleted; Clearance writes
  and `skipped`, the four statements and the compile line; Mask & filter and Hand-written without `policy:read`; delete
  dialog consequences, typed key, rules line, 409; `HandWrittenSection` per match level, badge, links, seeded line,
  `not_determined`, missing permission, error ≠ empty; Data three placements; People two placements and the three
  groups; link `?tab=` updates; query-key invalidation after every tag write.
- **Bundle guard** `test:bundle:governance-classification`, one marker per module of the area.
- **Live** (required): isolated `dgcl-` stack with real Trino 483 and StarRocks 4.1.6, predictions committed first
  (plan Task 19).

## 10. Out of #4b

Tag-assignment permissions; the rule-key/engine-name mismatch (CL6); paging BI checks beyond 500; group and role
resolution on engines; editing hand-written policies from Data or People (they link to the editor).
