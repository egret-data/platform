# SP4d-4: closing the Control Plane & Data Governance review — overlapping rules, editor levels, external engines, and a gate that scales

**Status:** design, drafted 2026-10-11 (awaiting the user's review)
**Programme:** `2026-09-15-control-plane-dg-review-programme.md`, item SP4 — the **last** package of
that programme (SP5 was dropped by the user on 2026-10-11; see the programme doc)
**Depends on (all merged):** SP4d-3 classify and migrate (#660, `07a61b263`, migrations 0180–0181) ·
the tag-service bump fix (#661, `eda7d164c`, migration 0182) · SP4d debt and guardrails (#656,
migrations 0176–0179) · SP4d-1/2a/2b `egret-ranger-eval` (#605, #606, #611)
**Plan:** `docs/en/plans/2026-10-11-sp4d-4-closing-the-review.md`

## Why this exists

egret **is** the Ranger Admin. No Apache Ranger server runs anywhere; the plugin inside each engine
downloads policy straight from egret (`/service/plugins/policies/download/{service}`,
`crates/egret-policy/src/ranger/routes.rs:15-31`), so a row in `policy.authz_policies` is not a
record of a decision — it **is** the decision the engine enforces. Trino 470 runs
`ranger-plugins-common-2.5.0`; StarRocks fe-ubuntu 4.1 runs `2.8.0`
(`crates/egret-ranger-eval/src/eval.rs:656-667`).

SP4d-3 left four items open. The user ruled on each on 2026-10-11 and asked for all four to close in
one package:

| | Item | User ruling (binding) |
|---|---|---|
| **A** | Two mask (or two row-filter) rules that select one coordinate with different expressions leave the engine's choice to a tie-break nobody authored | **Refuse overlap at write time** (409 naming the other rule), and the compiler refuses pre-existing overlaps and **reports** them |
| **B** | The policy editor drops empty resource levels on save; Ranger does not read an absent level as `*` | **Measure on the real engines first**, then decide between B1 (fix the editor) and B2 (keep it, correct the grammar) |
| **C** | egret knows only engines it provisions; SP4d-3's live check had to `INSERT` an engine row and an attachment by SQL | **API + UI** to register an external engine, bind it to a profile, attach catalogs, and show the plugin config; egret never provisions, scales or deletes the cluster |
| **D** | The SP4d-3 equivalence gate is about O(N³) and its budget admits ~20-policy profiles | **Coordinate pruning**, with a soundness argument and a fallback where the argument fails; re-measure and raise the budget only as far as measured |

## What is already true, measured before designing (read this first)

### A is half-closed already, and the half that is open is the larger one

The memory that framed this item (`egret-perm-compiler-emits-ambiguous-priority`) says the defect is
"not fixed". **The code says otherwise for its original shape.** SP4d debt Task 6 (#656) added
`crates/egret-policy/src/ambiguous_arbitration.rs` (903 lines), and the comment at both emission
sites now reads "the pair this used to warn about is now GUARDED rather than commented"
(`crates/egret-policy/src/admin/ranger_json.rs:207-219` mask, `:275-287` row filter). The emission
itself is unchanged — `"policyPriority": if input.is_override { 1 } else { 0 }` at `:143` (access),
`:220` (mask), `:288` (row filter) — and every compiled rule passes `is_override: false`
(`crates/egret-perm-compile/src/rules.rs:521,535,567,581`).

What the existing guard refuses, read off the code:

- **The key** is `(service, policy_type ∈ {1,2}, has_deny, policy_priority, canonical_resources)`
  (`ambiguous_arbitration.rs:436-504`): two policies collide only when their resource maps are
  **equal** after canonicalisation (levels sorted, values sorted and de-duplicated, empty level ≡
  absent; `:354-382`).
- **Agreeing ties are admitted**: a band whose decisive item arrays are equal (`Value ==`, array
  order significant) is not refused (`:506-529`). The evidence is SP4d-1's F2 (a real StarRocks
  download, `docs/en/plans/2026-09-28-sp4d-1-live-result.md` §8): two `MASK` policies on
  `sp4a10_wh.sp4a.report.item` agreed and are harmless; `country = 'US'` vs `id > 2` on one table
  disagreed.
- **Call sites**: `trigger::compile_desired` on the whole desired set of one profile
  (`crates/egret-perm-compile/src/trigger.rs:562-598`, returns
  `CompileError::AmbiguousMaskOrFilterArbitration`) and the editor's `refuse_ambiguous_arbitration`
  (`crates/egret-policy/src/admin/mod.rs:1171-1237`). Both answer **400**
  (`crates/egret-perm-compile/src/routes/error.rs:81-93` maps every non-`Db` `CompileError` to
  `BadRequest`; the editor returns `PolicyHttpError::BadRequest` at `admin/mod.rs:1234`). The user's
  ruling asks for **409**.

What it **does not** refuse — the open half, and the bulk of item A:

1. **Different resource maps that decide the same request.** The module says so deliberately
   (`ambiguous_arbitration.rs:106-126`): a schema-wide mask plus a column mask is admitted because
   "`getEvalOrder()` separates them and the engine's choice is determined by the artifact".
   The second half of that is true; **the conclusion a steward would draw from it is not.** Read
   off `RangerOptimizedPolicyEvaluator.computeEvalOrder()` at both deployed tags
   (`git -C ../ranger show release-ranger-2.5.0:agents-common/.../RangerOptimizedPolicyEvaluator.java`
   lines 55 and 193-228; byte-identical arithmetic at `release-ranger-2.8.0`):
   `evalOrder = 10000 − min(100, Σ resource discounts) − principal discount − access discount −
   condition discount`, where a level whose values contain the match-any `*` earns a discount of
   **25**, a level with any other `*` earns 10, and `?` earns 5. The comparator sorts evalOrder
   **ascending** (`RangerPolicyEvaluator.java:203-222` at both tags) — so a policy with more
   wildcards is evaluated **first**, and for masks and row filters the first matching evaluator is
   final (`>=` arbitration; `crates/egret-ranger-eval/src/eval.rs:3811-3830`). For two compiled rules every
   other discount is equal (both carry the `public` group → −25; both carry `select` only; neither
   carries conditions). **So the BROADER rule wins, and the narrower rule never applies to anyone
   the broader one's items cover.** A steward who writes `('sales','*','email') → MASK` and
   `('sales','orders','email') → MASK_NULL` gets `MASK` on `orders.email`. Read from source, not yet
   run through the Java harness — plan Task 1 pins it on both jars.
2. **A tag rule and a path rule on one column** (Trino only; a StarRocks tag rule compiles to
   per-column path policies — `rules.rs:5-14` — and is caught by the existing key). The two live in
   different downloads (the profile's resource service and its tag service), so the per-service key
   cannot pair them. The tag phase runs first; with a tag mask on the result, a resource mask is
   shut out unless its priority is **strictly** higher (offset 529, `if_icmplt`; modelled at
   `eval.rs:3892-3990`, the walk starting at `:3957`). At the compiler's uniform priority 0 the tag rule always wins and the path
   rule is dead on every tagged column — determinate, and nothing the steward authored.
3. **Two tag rules whose tags sit on one column** (Trino). Their tag policies' resources differ
   (`tag=[pii]` vs `tag=[gdpr]`), so the key never pairs them. On the tag path the comparator is
   match type, priority, `hasDeny`, then the policy **name** (`eval.rs:4442-4446`), and the name is
   `perm-rule-<uuid>` — the same uuid lottery F2 showed, reachable whenever a column carries both
   tags.

**Consequence for this package:** item A is not "add a guard"; it is "widen the one guard from
*equal resources* to *may decide the same request*, add the cross-service tag cases the compiler
alone can see (they need tag assignments), answer 409 naming rules, and add the report surface the
user asked for." The single-function discipline of `ambiguous_arbitration` is kept.

### B is already measured live for Trino, and the plugin sources predict the same for StarRocks

- **Ranger's matcher:** a policy naming fewer levels than the request earns `MatchType::Ancestor`,
  which is **not** an allow for a resource request — `isMatched = SELF || SELF_AND_ALL_DESCENDANTS
  || (ANCESTOR && request instanceof RangerTagAccessRequest)` (`crates/egret-ranger-eval/src/resource.rs:16-31`).
- **Measured live on Trino 470, 2026-09-24:** a grant compiled to `{catalog, schema}` and one
  compiled to `{catalog, schema, table}` both produced `Access Denied: Cannot select from columns
  [v, id] …`; the same grant written down to `column: "*"` returned rows
  (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, T1 and T1-table).
- **How the plugins form a SELECT request (read at the deployed tags):**
  - Trino 470: `checkCanSelectFromColumns` asks `select` once **per column**
    (`git -C ../trino show 470:plugin/trino-ranger/src/main/java/io/trino/plugin/ranger/RangerSystemAccessControl.java`
    `:523-530`), via `RangerTrinoResource.forColumns`, which falls back to a single **table**-level
    resource only when the column set is empty (`RangerTrinoResource.java:102-109`) — i.e.
    `SELECT count(*)`. Insert and delete are table-level (`:431`, `:439`); `show`/filter checks use
    `_any`, and the `filter*` methods widen the scope to `SELF_OR_DESCENDANTS` (`:972`, `:990`).
  - StarRocks 4.1.4: with an external (Ranger) access controller, a query is checked with
    `checkColumnAction(…, SELECT)` once per **scanned (pruned) column**
    (`git -C ../starrocks show 4.1.4:fe/fe-core/src/main/java/com/starrocks/authorization/ColumnPrivilege.java`
    `:89-140`), which builds a catalog/database/table/**column** resource
    (`RangerStarRocksAccessController.java:159-170`).
- **The editor really does save such bodies.** `resources_object` inserts a level only
  `if !level.values.is_empty()` (`ranger_json.rs:83-91`). `ResourceSelector` offers "— none —" on any
  level whose parent is `isValidLeaf` (`frontend/src/components/policy/ResourceSelector.tsx`, the
  `showNone` branch); in both service-defs `table` is `isValidLeaf: true` and `column` is
  `mandatory: true` (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`, measured with a
  one-off `python3 -I` read). `defaultResourcesFromDef` seeds the root level EMPTY and every other
  level `*` (`frontend/src/routes/governance/PolicyEditorPage.tsx:92-117`), so an operator who clears
  a `*` produces an empty level, which the save drops.

**Prediction, to be confirmed by plan Task 1:** branch **B1** — a table-stopping access policy that
grants `select` lets nobody read a column on either engine (it grants only `SELECT count(*)` on
Trino and table-level checks such as `insert`/`delete`). The prediction is strong but it is a
prediction: the jar-level matrix (both versions, every depth, every access type, `_any` at both
scopes) has not been run, and StarRocks' plugin behaviour has been **read, not run**.

### C is the largest item by a wide margin

- `engine_mgmt.engines` has no notion of "not ours" (`\d engine_mgmt.engines` on `egret_test`,
  migrated to 0181): `namespace` and `spec` are `NOT NULL`, `status` is
  `CHECK IN ('provisioning','running','failed','deleting')`, and the coordinator address is
  **derived** from name and namespace as Kubernetes DNS (`crates/egret-engine-mgmt/src/store.rs:64-81`:
  `{name}-trino.{ns}.svc:8080` / `{name}-fe-service.{ns}.svc:9030`).
- `crates/egret-engine-mgmt/src/engines.rs` is **11 811 lines**; the router
  (`:94-125`) carries 13 engine routes, and `engine_provisioner` / `kube` / `starrocks_ddl` are used
  by at least 20 handler sites (`rg 'engine_provisioner|\.kube|starrocks_ddl\.' crates/egret-engine-mgmt/src`).
  Every one of them must refuse or no-op for an external engine.
- **Attachments are applied, not declared.** Attaching a catalog to a Trino engine re-renders the
  cluster (`catalog_apply.rs:163-200`); to a StarRocks engine it runs `CREATE CATALOG` on the FE
  with the root secret (`catalog_apply.rs:201-270`, `:407-540`). `apply_status` is
  `CHECK IN ('applied','failed','pending')` (`\d registry.engine_catalog_attachments`). An external
  engine has no root secret egret holds, so its attachment can only be a **declaration**.
- **The plugin version of a profile is derived from engine TYPE.** `bound_plugin_versions`
  (`crates/egret-perm-compile/src/migrate.rs:377-411`) reads `DISTINCT type` and maps it through
  `EngineType::plugin_version` (`eval.rs:656-667`); `explain` does the same
  (`crates/egret-perm-compile/src/routes.rs:1832-1836`). An external Trino 480 or a StarRocks built
  on a different Ranger would be evaluated as the wrong version.
- **Two assumptions egret's explain and gate rest on are guaranteed only by egret's own rendering**:
  no `ranger.plugin.*.super.users` (`routes.rs:1794-1808`: "NEITHER emits … super.users"), and no
  group provider or usersync (SP4d-3 spec, "one group per synthetic member"). An external engine's
  plugin config is the admin's, not egret's.
- **The download is anonymous unless a token is configured**, and a stock `RangerAdminRESTClient`
  does not send egret's bearer token (`crates/egret-policy/src/ranger/auth.rs:9-36`). Registering an
  engine outside the cluster means exposing `/service` beyond it.
- The plugin config egret renders for its own engines points at `config.catalog.public_url`, which
  defaults to the in-cluster `http://egret:8080` (`crates/egret-bin/src/main.rs:1978-1985`); the
  existing per-profile `GET …/policies/services/{service}/trino-config`
  (`crates/egret-policy/src/admin/mod.rs:817-870`) uses the same base and is Trino-only.

### D is ~N³ for a measured reason

`derive_probes` takes the cross product of principals × access types × coordinates, all derived from
**both** whole policy sets (`crates/egret-perm-compile/src/equivalence.rs:279-310` doc, `:335-420` the factors), and every probe
asks the evaluator, which visits every policy. The measured table is in `migrate.rs:118-142`: 5
policies → 3 870 probes / 0.05 s; 20 → 51 345 / 2.3 s; 40 → 198 645 / 19.4 s (release, prepared
sets, one version). `MAX_PROBE_EVALUATIONS = 50_000` (`migrate.rs:142`). No timing harness is
committed — the table was measured ad hoc.

## Part A — overlapping rules are refused, and reported

### What "overlap" means, stated so the refusal is sound

Two policies **P** and **Q** that the same engine evaluates for one request **overlap** when all of
the following hold:

| | Condition | Why it is in the definition |
|---|---|---|
| o1 | Same evaluation unit: both in one download, **or** one in a Trino profile's resource service and the other in that profile's `tagService` | the engine's plugin runs the tag phase then the resource phase for one request (`eval.rs:3846-3862`, the tag phase at `:3851-3862` before the resource candidates) |
| o2 | Same `policyType`, and it is 1 (mask) or 2 (row filter) | Ranger files the three types into disjoint evaluator lists; access (type 0) arbitrates with `>` and deny-wins makes order immaterial (`ambiguous_arbitration.rs:75-79`) |
| o3 | Same band — equal `(policyPriority, hasDeny)` — **for two policies in one download**. Not required across the tag/resource boundary | within a band the order is the non-authored `getEvalOrder()` then the name; across bands an AUTHORED priority orders them. Across tag/resource the tag result shuts out every resource policy at `<=` its priority and is split by any above it, so no priority makes the pair coherent |
| o4 | **Some request coordinate may be decided by both**: a column for masks, a table for row filters | see "may decide" below |
| o5 | Their decisive item arrays differ (`Value ==`, array order significant) | equal arrays yield equal data whichever runs first — F2's agreeing pair, measured live |

**"May decide" (o4)**, computed so it over-approximates — the direction that is sound for a
refusal:

- **Path vs path.** Per level, the two value sets *may intersect*: equal under Ranger's own
  ignore-case comparison — **full Unicode, not ASCII** (every level of both service-defs is
  `ignoreCase: true`, and Ranger compares with `String.equalsIgnoreCase` / `IOCase.INSENSITIVE`,
  per UTF-16 unit as `Character.toLowerCase(Character.toUpperCase(c))` — Java's SIMPLE,
  one-char-to-one-char mappings, so `İ` folds to a single `i` and `("İSTANBUL", "istanbul")` is one
  name *(Task 3, N1)*; the guard takes two values as equal when they
  agree under that fold OR under `egret_ranger_eval`'s `to_lowercase()`, so it over-approximates
  both, and `ĐƠN_HÀNG` meets `đơn_hàng`) — or one is a `prefix*` and the other starts with that
  prefix (folded the same way), or either value contains any other `*`/`?` (or a `{`/`\` token
  character) — then assume they intersect. An absent level, a level whose values are all empty
  strings (Ranger's `init()` drops an empty value, so such a level is match-any), or `*`,
  intersects everything. All levels must may-intersect. `isExcludes` on either side — including
  any non-boolean value such as the string `"true"` — → assume they intersect. *(Amended in Task 2
  fix round 1, review C1/I1/M5; the user ruled full Unicode folding.)*
- **Tag vs anything.** A tag policy applies at a coordinate when a tag its `resources.tag` values
  cover (Ranger wildcard match, case-folded) is **resolved there from the profile's
  `catalog_tag_assignments`**: a column-level assignment applies to that column; a table-level
  assignment applies to every column of the table **and** to the table; for row filters a
  column-level assignment also applies to its table (conservative — this is how StarRocks expands a
  tag row filter, `rules.rs`, and it is the fail-closed direction for Trino). The pair overlaps when
  some assigned coordinate is one the other policy applies at (another tag, or a path glob matched
  with `egret_ranger_eval::resource::match_policy_resources` — Ranger's own matcher, not a second
  one).

**Identical expressions.** Admitted, as today. Evidence: F2's agreeing mask pair was measured on a
real StarRocks download and enforced identically whichever evaluator ran first, because the
first-matching evaluator is final and both yield the same `dataMaskInfo`. Refusing it would break the
only real artifact this programme has measured, for no enforcement gain. The evaluator still answers
`EvalOrderAmbiguous` for that pair because it cannot name the winning policy id — an explainability
limit, not an enforcement one, and unchanged by this package.

**Exemptions.** A rule's `exempt` set compiles to specific-then-general items: one `MASK_NONE`
(or empty `filterExpr`) item for the exempt users ahead of the `public` item (`rules.rs:30-47`). Two
rules with the same mask but different exempt sets therefore have different decisive arrays, and
**are** ambiguous: for a user exempt from one and not the other, one evaluator answers `MASK_NONE`
and the other `MASK`, and the winner is the tie-break. o5 catches this with no extra rule; the test
list names it so a future "compare only the mask type" simplification cannot pass.

**Soundness, stated as a claim with its limits.** Every pair that the engine could arbitrate by a
non-authored tie-break *for the current tag assignments* is refused: o4 over-approximates the true
intersection, o3 and o2 are exactly the evaluator's band, and o5 is a sufficient condition for "same
data either way". It is **not** sound against:

- writes that do not go through a route (`psql`, an import) — the same reach limit
  `ambiguous_arbitration.rs:185-188` already records;
- `PUT /policies/{id}/enabled`, which is deliberately unguarded so a pair can be broken
  (`ambiguous_arbitration.rs:175-184`) — a pair completed by re-enabling is refused by the next
  compile when one member is compiled, and is shown by the report below either way;
- a pair of two HAND-WRITTEN policies that is already stored (completed by a re-enable or a write
  outside the routes): the editor and the tag assignment refuse only pairs their write creates,
  and the compiler refuses only pairs with a compiled member, so such a pair is reported, not
  refused, and wedges nothing;
- a recompile whose caller does not take the profile locks before compiling reads the model and
  the tag assignments before `apply_diff_in_tx` locks the rows, so its check runs on a snapshot a
  concurrent transaction may still change (the rule and grant routes, the migration and the tag
  assignment pre-lock; the other recompiling callers were not audited for this).

*(Amended in Task 3 fix round 1.)* What this list used to name and is now closed: recompiles of two
profiles sharing a tag service serialise (`lock_services_in_tx` locks the expanded, sorted set, and
`apply_diff_in_tx` re-takes it); the compiler reads the enabled hand-written masks and row filters of
the profile and its tag service and refuses every pair with a compiled member, so a rule write, a
migration (whose originals are switched off before its rule writes) or a membership-driven recompile
cannot complete a compiled × hand-written pair (review C1); and the policy editor checks and writes
in one transaction under the same row locks, through the same lock helper, so it serialises with
tag assignments, recompiles and other editor writes on the unit (review I1).

### Which write catches what

Every surface that can create the pair recompiles **inside its own transaction** before commit, so
the compiler's check *is* the write-time check — there is no second predicate to keep in step. By
code path (`ambiguous_arbitration.rs:152-174`, re-verified in plan Task 3 with
`rg 'recompile_service' crates`):

| Write | Catches |
|---|---|
| `POST`/`PATCH /api/control-plane/perm/rules` (`routes.rs:1445`, `:1490` → `write_rule` `:1075`) and `POST /perm/preview` (`:1606`, rolled back) | every path×path pair; every tag pair whose co-occurrence already exists |
| SP4d-3 `POST …/migrate` (rules inserted, then `recompile_services_in_tx`) | the same, for migrated rules |
| tag assign / unassign (`crates/egret-bi/src/catalog_tags.rs:138`, `:176`, recompile in the same tx) | a tag pair or a tag/path pair the moment an assignment makes it real |
| tag rename, classification, grant, role/workspace membership, catalog attach, profile bind | whatever the recompile they already run would emit |
| the policy editor (`admin/mod.rs:1171-1237`) | hand-written pairs on manual profiles (widened by the same function), and a hand-written policy that would pair with a stored compiled one |

**Caught only at compile time, by design:** a pair of tag rules whose tags share no column yet is
**not** refused when the second rule is written. It becomes a refusal on the tag assignment that
would put both tags on one column. Refusing it earlier would refuse a configuration that is coherent
today on the guess that somebody will tag a column later; the refusal at assignment time names both
rules and the column, which is the moment the steward can act.

### The refusal

- `CompileError::AmbiguousMaskOrFilterArbitration` gains the rule ids and names and the coordinate,
  and maps to **409** with a body `{ "error": <sentence>, "code": "rule_overlap", "rules":
  [{id,name},{id,name}], "coordinate": "...", "kind": "mask"|"row_filter" }`. The sentence names
  **the other rule by name** (not `perm-rule-<uuid>`; `DesiredPolicy.rule_id` already carries the
  id, `trigger.rs:278-283`). For a tag assignment the same 409 comes back through `egret-bi`.
- The editor's refusal becomes **409** too, and its sentence suggests `isOverride` as a remedy, since
  hand-written policies *can* author priority. The editor refuses only a pair its write **creates**
  (pairs after the write minus pairs already stored), so a write that narrows or removes one stored
  pair succeeds while another stored pair exists *(Task 2 fix round 1, review I2)*.
- The sentence keeps the existing discipline: it does not say "random", "undefined" or "may change
  between queries" (`ambiguous_arbitration.rs:611-634`), and says which one wins today when the
  evaluator can say so (broader wins within a band).

### Pre-existing overlaps: refused, and reported

A pair that already exists (compiled before #656, completed by a re-enable, or created by a write
outside the routes) makes **every** recompile of that profile fail — the compiler refuses to compile
it, as ruled. That is a wedge for unrelated writes on that profile, accepted because the
alternatives are worse: skipping either rule serves the column unmasked (fail-open), and keeping the
last artifact serves exactly the ambiguity. The writes that *remove* the pair still succeed — deleting
or narrowing one rule, or unassigning the tag — because their recompile no longer sees the pair.

To make the wedge visible before someone trips it:

- `GET /api/control-plane/perm/overlaps` (`policy:read`, the permission `list_rules` uses,
  `routes.rs:1388`) runs the compiler's overlap check over every profile **without writing**, and
  returns every overlapping pair (not only the first), each naming its profile, kind, coordinate and
  both rules.
- The Mask & filter rules page (`frontend/src/routes/governance/MaskFilterRules.tsx`) shows a banner
  when the list is non-empty, naming each pair with links to both rules.

The function therefore returns **all** pairs; write paths report the first (deterministic order).

## Part B — the editor and absent levels

### Plan Task 1 measures, then the user picks

Task 1 runs the jars at **2.5.0 and 2.8.0, for both service-defs** (an external engine may run
either), over: policies that stop at catalog / schema / table, the `column: *` control, and the
gap shape the editor can produce (a named level below an absent one); requests at every depth;
access types `select`, `insert`, `delete` (and StarRocks `update`), plus `_any` at `SELF` and at
`SELF_OR_DESCENDANTS` (the Trino `show`/filter path). The harness needs one additive change: an
optional `resourceMatchingScope` on the request (`tests/ranger/src/test/java/EgretCaseRunner.java`,
`request()` `:192-212`). The Rust evaluator must agree on every concrete-access cell (it refuses
`_any`). Task 1 also re-reads the plugin sources at the deployed tags (cited above) and pins the
evalOrder finding of Part A on both jars. It ends with a **STOP**: the controller asks the user which
branch the table supports.

### B1 — the editor's saves do not grant what the editor implies (the predicted branch)

1. **New saves.** In the editor path only, a level that is **present with no values**:
   - at the **root** (catalog) → **400**, "name a catalog" — never a silent `*`, which would turn a
     forgotten catalog into every catalog;
   - anywhere else → saved as `["*"]`, and the editor says so **beside the field before save**
     ("Empty means every schema — saved as *"). This is the user's ruling ("an empty level is saved
     as `*`, with an explicit operator-visible note"), narrowed at the root because the root is the
     one place `*` would widen to every catalog the profile serves.
   A level the operator removed with "— none —" is **absent, not empty**, and stays absent: stopping
   at `table` is a legitimate Ranger policy for `insert`/`delete`/`alter`. When such a chain grants
   `select`, the editor shows a warning: "Stops above column: grants no column reads (only
   `count(*)`-style table checks)."
2. **Existing rows are flagged, never rewritten.** Rewriting a stored body would change what is
   enforced (it would *start* granting reads), which the ruling forbids doing silently. The classifier
   (`GET …/perm/policies/services/{id}/classify`, `crates/egret-perm-compile/src/classify.rs`) gains a
   per-policy `warnings` list with two kinds: `SelectStopsAboveColumn{level}` (an access policy whose
   resource chain stops above `column` and whose items allow `select`) and `LevelGap{missing}` (a
   named level below an absent one — matches nothing). The profile page shows a count and the list,
   each linking to the editor. The operator decides, per policy, by editing it.
3. **The grammar keeps refusing `MissingLevel`** — now with the measurement behind it — and its
   sentence (`classify.rs:395-410`) is corrected to stop calling dropped levels "how most
   editor-saved policies look" in the present tense, and to point at the warning.

### B2 — the saves do grant it (the alternative branch)

If Task 1 shows an absent trailing level decides **identically** to `*` for every access type on
both versions, the editor is kept, the grammar accepts a missing **trailing** level as `*` (the
compiled-back form then decides identically, which is the grammar's design oracle,
`crates/egret-perm-compile/src/grammar.rs:34-48`), and the `MissingLevel` sentence and the
resource.rs/grants.rs comments that state the opposite are corrected. **The gap shape is fixed in
both branches** — a named level below an absent one matches no hierarchy, so it is an editor defect
whatever Task 1 finds.

## Part C — registering an external engine

### What an external engine is in egret

A row in `engine_mgmt.engines` that egret **never** provisions, updates, restarts, scales, execs into,
or deletes the cluster of. It exists so that:

1. a policy profile can be bound to it (`authz_service_id`), so the perm compiler compiles grants
   for the catalogs it serves and the gate knows its plugin version;
2. registry catalogs can be **declared** attached, so `affected_services_for_grant` /
   `_for_rule` (`trigger.rs:1233-1320`) reach its profile — this is the attachment SP4d-3 Task 10
   had to `INSERT` by hand;
3. an operator can read the exact Ranger plugin configuration to install on it.

### Schema (one migration, number checked against `origin/main` at creation — 0183 today)

- `engines.origin text NOT NULL DEFAULT 'provisioned' CHECK (origin IN ('provisioned','external'))`.
- `engines.external_host text`, `engines.external_port integer`, `engines.ranger_plugin_version
  text`, with one `CHECK` making the shape unrepresentable otherwise: `origin = 'external'` ⇔
  (`external_host` and `external_port` and `ranger_plugin_version` are NOT NULL, `namespace = ''`,
  `spec = '{}'::jsonb`, and every `*_secret_id`/`tls_*`/`coordinator_cert` is NULL);
  `ranger_plugin_version ~ '^[0-9]+\.[0-9]+\.[0-9]+$'`.
- `engine_catalog_attachments.apply_status` admits `'declared'`. A declared attachment carries no
  client and no credential (`minted_client = false`, `catalog_client_id IS NULL`). That "declared ⇔
  the engine is external" is a cross-table fact a `CHECK` cannot express; it is enforced in the one
  Rust function that writes attachments and asserted by a test across the whole table (the
  `managed_by` agreement pattern from SP4d-3).
- `status` is not widened: an external engine's row holds `'running'` for the column's sake, and
  **every** reader that shows status shows `origin = external` instead ("External — egret does not
  monitor it"). Measured alternative rejected: adding a status value touches every status reader and
  the monitor's transition logic for no information gain.

### API (`engine:create` / `engine:update` / `engine:delete` / `engine:read`, as the existing routes)

- `POST /api/control-plane/engines/external` `{ name, type: trino|starrocks, host, port,
  rangerPluginVersion, authzServiceId?, catalogs: [catalogId] }`. Binding reuses
  `apply_authz_service_update` (`engines.rs:4990-5150`): profile type must equal engine type (409),
  one profile per engine (409), a Trino profile needs a usable `tagService` (refused, with the
  existing reasons). Catalog ids must exist in `registry.catalogs`; each becomes a `declared`
  attachment. One transaction; then the bound profile is recompiled through the existing
  `ProfileCompilerPort` so its download carries the catalogs' grants immediately.
- `GET /engines`, `GET /engines/{id}` carry `origin`, `host`, `port`, `rangerPluginVersion`, and for
  an external engine never call the provisioner.
- `PUT /engines/{id}` for an external engine accepts only `authzServiceId`, `host`, `port`,
  `rangerPluginVersion`; anything else is 409.
- `POST /engines/{id}/catalogs/{cid}` on an external engine writes a `declared` attachment and
  recompiles; detach removes it and recompiles. No DDL, no cluster render.
- `DELETE /engines/{id}` on an external engine: the SP1 dependents preflight
  (`engine_dependents`, `engines.rs:5583-5589`) still runs; then the row is deleted with no
  provisioner call and without requiring Kubernetes; the profile is unbound, not deleted (as today).
- **Refused with 409 `engine_is_external`**, naming the reason: restart, rotate-secrets, pod logs,
  pod logs follow, pod exec, the monitor routes (`monitor.rs:134-140`), `…/catalogs/{cid}/test`,
  registry catalog reapply/converge for that engine, and workspace assignment (out of scope — see
  below).
- `GET /api/control-plane/engines/{id}/ranger-plugin-config` → the files to install, rendered by the
  **same** functions that render egret's own engines (extracted from
  `crates/egret-provisioner/src/{trino,starrocks}.rs` so there is one spelling): Trino
  `access-control.properties` + `ranger-trino-security.xml`; StarRocks the `access_control = ranger`
  line for `fe.conf` + `ranger-starrocks-security.xml` + `ranger-starrocks-audit.xml`. The response
  also states the service name, the download URL (`{public_url}/service/plugins/policies/download/{service}`),
  the auth mode (anonymous, or "a bearer token is configured; a stock plugin cannot send it"), a
  warning when `public_url` is the in-cluster default, and the assumptions egret's explain and gate
  make about the plugin (no super-users, no group provider, audit off).

### The plugin version a profile is evaluated at

One function, `engine_plugin_version(row) -> Result<PluginVersion, String>`, in
`egret-perm-compile`: a provisioned engine maps its type as today; an external engine maps its
declared version if it is one `egret-ranger-eval` models (`2.5.0`, `2.8.0` — `eval.rs:670-680`
makes any other unrepresentable), else returns a sentence. `bound_plugin_versions`
(`migrate.rs:377-411`) and `explain` (`routes.rs:1832-1836`) both call it. An unmeasured version
makes the migration gate refuse on that profile and `explain` answer a refusal that names the
version — never an evaluation at a guessed version.

### The UI

`ControlPlaneEngines.tsx` gets a "Register external engine" entry beside "New engine", to a page
`control-plane/settings/engines/register` (FE-PAGE-3: several sections). `EngineDetail.tsx` shows,
for an external engine, an `External` chip instead of status, the address, the declared plugin
version, the bound profile, the declared catalogs, and a "Plugin config" tab with the files and the
warnings — and hides every tab whose route refuses (pods, logs, terminal, monitor, autoscaling,
config editors, rotate secrets).

### Out of scope for C, named

- **Workspace connections to an external engine** (`assignment.rs` builds connections from the
  derived coordinator address and egret-held delegation secrets). Assigning an external engine to
  a workspace is refused with 409 until a later package designs credentials for it.
- **Credentials of any kind for an external engine.** No typed secret is stored; egret never connects
  to it. (A TCP reachability probe is likewise out — a green dot egret cannot back would be a status
  indicator not derived from content, `state-indicators-must-be-content-derived`.)
- **Authenticating the download.** The anonymous `/service` surface is a pre-existing property; this
  package states it on the config page and in Risks, and does not redesign it.

## Part D — coordinate pruning for the equivalence gate

### The argument

Let `B` and `A` be one profile's download before and after (resource policies + `tagPolicies`).
Let **Δ** be the policies whose `(name, body without id/version)` differs between the two, or that
exist on one side only (both versions of a changed policy are in Δ). Let **E(S)** be everything in a
download except the two policy lists (service-def, tag service-def and name, `serviceConfig`, zones,
deltas).

**Lemma (candidate locality).** For a request `r`, Ranger's access, mask and row-filter decisions are
a function of `E(S)`, `r`, the engine config and plugin version, and the **ordered list of policies
in Ranger's candidate lists for `r`** — the resource trie's answer for the asked policy type, and the
tag trie's answer for `r`'s resolved tags. A policy in neither list is never invoked
(`RangerPolicyEngineImpl.evaluatePoliciesNoAudit` → `evaluateTagPolicies` over
`getLikelyMatchPolicyEvaluators(tags)`, then the resource repository's
`getLikelyMatchPolicyEvaluators(request)`; modelled by `egret_ranger_eval`'s `candidate_views` and
`tag_candidates`). Super-user and zone handling read only `E` and the engine config.

**Over-approximating candidacy.** For a resource policy, Ranger's list contains it only if some
resource map's match type is not `NONE` **or** the trie fails to exclude it (a trie false positive,
which matters at offset 553 where `hasDeny()` alone is read). `egret-ranger-eval` already has both
halves: `PolicyView::provably_a_candidate` (`eval.rs:3288-3317`, measured in
`tests/ranger/src/test/java/EgretResourceTrieCandidacyTest.java`) and `trie_excludes`
(`eval.rs:2826-2849`, private today). `may_be_candidate(p, r) := any_match(p, r) ∨ ¬trie_excludes(p,
r)` is exported from that crate — one spelling. For a tag policy, `may_be_candidate` is "some tag in
`r`'s resolved tags is covered by `p.resources.tag`".

**Theorem.** If `E(B) = E(A)` and no policy in Δ may be a candidate for `r`, then `B` and `A` decide
`r` identically. *Proof:* the policies that may be candidates for `r` are all outside Δ, hence
present and byte-identical on both sides; the actual candidate lists are therefore equal as sets;
their order is the comparator's (priority, `hasDeny`, `evalOrder`, name), every key of which is a
field of an identical policy, and the stable sort's input order is the download's `ORDER BY id`
(`crates/egret-policy/src/store.rs:1966-1967`), which preserves the relative order of unchanged
rows; by the lemma the decisions are equal. ∎

So the gate keeps every probe at a coordinate where some Δ policy may be a candidate, and drops the
rest — **only probes that provably agree are dropped**. Principals and access types are not pruned
(user ruling). The SP4d-3 requirement that probes reach past both sides' vocabulary survives where it
matters: a *sibling* coordinate is kept exactly when a Δ policy may match it, which is precisely
where a widening migration shows; siblings no Δ policy can reach are provably unchanged.

**Where the argument fails, and the gate falls back to the full probe set:**

| Condition | Why |
|---|---|
| `E(B) ≠ E(A)` | the lemma's other input moved |
| a Δ policy carries `isDenyAllElse: true`, or a non-zero `policyPriority` | not needed by the theorem (both are per-policy and reach only that policy's candidates), but the user named them and the cost is nil: the SP4d-3 grammar refuses both (`Blocker::DenyAllElse`, non-default `policyPriority`), so no grammar-produced migration triggers the fallback |
| the pruned set is empty for a profile whose Δ is not | fail-closed: `GateRefusal::NoProbes` as today, never "equivalent" |

Policy **ids** are excluded from Δ's equality: the evaluator's id tie-break (`eval.rs:3215-3224`) is
reached only inside bands it already refuses as `EvalOrderAmbiguous`, and the gate counts any
`Undecidable` as a disagreement.

### Cost, and what changes in the budget

Coordinates are O(N); kept coordinates are O(k) for k = |Δ| plus the descendants and ancestors of
Δ's coordinates; principals stay O(N). Probes fall from ~N² to ~k·N. Each evaluation still visits
every candidate policy, so time falls from ~N³ to ~k·N². A **tag** policy in Δ still keeps every
coordinate where its tag is resolved, actually or hypothetically — and hypothetical tagging probes
every table coordinate (`equivalence.rs:400-416`) — so migrating a tag rule stays ~N²; collapsing
coordinates with identical candidate signatures would fix that and is **out of scope** (named below).

The budget is re-measured with a committed, `#[ignore]`d timing test (release, one version, N ∈
{5, 20, 40, 80, 160}, with and without a tag mask, k = 1). `MAX_PROBE_EVALUATIONS` is raised only to
the largest probe count whose measured locked-transaction time is **≤ 2.5 s** — the latency the
current cap was accepted at (`migrate.rs:137-139`). Because per-evaluation cost grows with N, the
measurement table is written into the constant's doc as the SP4d-3 one was, and the evaluation count
is the cap's unit only if the table shows time tracking it; otherwise the cap becomes evaluations ×
policies, and the task says which.

### Proof obligations (tests)

- **Pruned ⊆ full, and the same disagreements.** Over the SP4d-3 gate tests and a seeded random
  corpus, `compare` with pruning returns exactly the disagreements `compare` without it returns.
- **An unchanged-but-interacting deny is still caught.** Before: an unchanged deny on
  `wh.sales.orders` for `alice` and an allow `H` on `wh.sales.*`; after: `H` replaced by `G` that
  differs only by a strictly higher `policyPriority`. The disagreement sits at
  `wh.sales.orders.*` — which G may match, so it is kept — and is reported.
- **A tag interaction is still caught.** SP4d-3's version-split shape (a tag DENY plus a strictly
  higher-priority no-deny resource ALLOW): Δ is the resource ALLOW, the interacting tag DENY is
  unchanged, and the disagreement at the tagged coordinate is reported on the version it appears on.
- **The real engine agrees with what was pruned.** For the SP4d-3 Task 8 corpus, the probes the
  pruning DROPS (a deterministic sample of at most 4 000 per Ranger version) are sent to the real
  2.5.0 and 2.8.0 jars before and after: every one must decide identically, and the dropped count
  must be non-zero. This is the oracle for the theorem; the pruned-vs-full equality above only tests
  the implementation against itself.
- **The Task 8 differential still reports 0 disagreements** through the full `just
  test-ranger-gates`.

## Risks

- **A's refusal is wider than today's, so it can refuse configurations that are in use.** A
  schema-wide mask plus a column mask was admitted until now; after this package the next write to
  that profile is refused until one of them is narrowed. This is the point — the narrow one never
  applied — but a steward will meet it as a new 409. The report route exists so the population is
  visible before anyone trips over it. The population on this host is zero (re-measured 2026-10-10,
  `ambiguous_arbitration.rs:209-224`); on a real cluster it is unknown.
- **A's tag cases depend on the assignment rows matching what the plugin resolves.** They do by
  construction (egret serves the plugin's tags from the same rows), but the case fold is egret's
  ASCII fold, and SP4d-3 found 25 code points where egret's fold and the engine's disagree; values
  carrying them are refused by the gate already and should be by this check too (Task 3 reuses
  `fold_divergent`).
- **B1 surfaces many warnings at once.** If most editor-saved policies stop above `column`, the
  profile page will flag most of them. That is the truth about what they enforce; the warning must
  say what to do, and nothing is rewritten.
- **C is the largest item and touches an 11 811-line file.** Every handler that reaches the
  provisioner, Kubernetes or the FE must learn `origin`; one missed handler is a 500 or, worse, a
  provisioner call against a cluster egret does not own (`delete` would call
  `prov.delete(type, '', name)`). Task 8 enumerates the handlers by `rg` and asserts each refusal by
  route test; the review must re-run the `rg`.
- **C weakens two assumptions silently if the admin edits the plugin config.** egret's explain and
  migration gate assume no super-users and no group provider. egret cannot verify an external
  plugin's config; the config page states the assumptions and `explain` for an external engine's
  profile carries the same sentence. A later group provider would turn SP4d-3's one-group probe
  member into a false-equivalence path, exactly as the SP4d-3 spec warned.
- **Registering an external engine exposes `/service` beyond the cluster.** The download is
  anonymous unless a bearer token is set, and a stock plugin cannot send one. Stated on the page;
  not redesigned here.
- **D's soundness rests on the evaluator's candidacy model.** If `trie_excludes` claims absence where
  Ranger's trie returns the policy, pruning drops a probe that disagrees. The real-engine check on
  the dropped probes is the countermeasure; it is a sample, not a proof.

## Out of scope, named so it is not mistaken for closed

- **SP5** — dropped from this programme by the user (2026-10-11). egret's own enforcement engine will
  be a separate engine, peer to Trino and StarRocks.
- **Emitting deterministic priorities instead of refusing** (e.g. more specific → higher priority).
  Ruled out by A's ruling; refusal is the design.
- **Grouping coordinates by candidate signature** to make tag-rule migrations sub-quadratic in D.
- **Workspace connections, credentials, monitoring and reachability probes for external engines.**
- **Download authentication** for `/service`.
- **Probing `_any`** in the gate (the evaluator refuses it; SP4d-3 named this limit).
- **Rewriting existing editor-saved bodies** under B1 — flagged, never rewritten.
