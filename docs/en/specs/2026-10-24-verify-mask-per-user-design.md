# Verify a mask per user (Classification UX, Package 2): design

Status: design, 2026-10-24. Not implemented. Base: `origin/main` `af793f4a` (#738). Branch `feat/verify-mask-per-user`.
Plan: [`../plans/2026-10-24-verify-mask-per-user.md`](../plans/2026-10-24-verify-mask-per-user.md).
Programme: Control Plane & DG, Classification UX. Parent: [`2026-10-21-classification-ux-design.md`](2026-10-21-classification-ux-design.md) §14 (N1).

The user settled four questions on 2026-10-24 (§3, D1–D4). The controller set the other defaults (§4, C1–C13); the
user may revisit those. Every UI string uses the parent spec's glossary (§4 there).

## 1. Problem

The parent walkthrough could not verify its own job: "mask `email` for everyone except group `data-team`" (parent
§1, N1). Who can read? answers per policy profile, but:

- it never asks the mask question on Trino, and asks it on StarRocks only for a column read (E3);
- it never asks the row-filter question at all (E5);
- when it does report a mask, it names a Ranger policy and a rule uuid, not the rule, and never says *why* a person
  sees the real value (E8, E9);
- on StarRocks a cleared reader's mask is attributed to the clearance, never to the rule whose outcome it replays
  (E12);
- two measured engine gaps are invisible: a StarRocks table-level classification with a clearance compiles nothing
  (E13), and a Trino row filter on a column-level classification filters nothing (E14). One UI hint states the
  opposite of E13 (E15).

The steward wants one sentence per person and engine: "member-sso reads email as **hashed** (rule *Hash personal
data*)".

## 2. Evidence (verified at `af793f4a`)

Paths are relative to the repo root. `PR` = `crates/egret-perm-compile/src/routes.rs`, `EV` =
`crates/egret-ranger-eval/src/eval.rs`, `ER` = `frontend/src/components/perm/ExplainResult.tsx`. Rows marked
**corr.** correct the parent spec §14.

| id | fact | where |
|---|---|---|
| E1 | `POST /perm/explain` → `explain`, `POST /perm/explain/principal` → `people::explain_principal`; both gate `policy:read` and share `check_explain_target` | `PR:171`, `PR:176`, `PR:2299-2310`, `PR:2322`; `crates/egret-perm-compile/src/routes/people.rs:427` |
| E2 | `explain_users` resolves tags once per request (`resolved_tags_for_target`), walks `services_for_catalog_via`, builds one artifact per profile (`served_artifact_via`) and evaluates every user against it | `PR:2374-2609`; `PR:2388`, `PR:2408`, `PR:3138`, `PR:3411` |
| E3 **corr.** | The mask question is asked only when `action == Read && engine == StarRocks && target.column.is_some()`, with `resolved_tags: None` ("StarRocks has no tag service"). Access verdicts already get `resolved_tags.clone()`. Parent §14.3 cites `:2347-2349` | `PR:2502-2535`, `PR:2513-2514`, `PR:2566` |
| E4 **corr.** | The evaluator does **not** refuse Trino tag masks any more: `evaluate_sub` runs a tag phase for masks and row filters (`evaluate_tag_sub`). Only the doc on `ExplainProfile::mask` still says "the evaluator refuses (tier c)". So Trino masks need a handler change, not an evaluator change | `EV:4001`, `EV:4014-4031`, `EV:4660`; `PR:744-748`; `crates/egret-ranger-eval/tests/tag_tiers.rs:449` |
| E5 | No row-filter decision exists on the wire: no `ExplainRowFilter`, no `WireRowFilterDecision` (only `WireMaskResult`/`WireMaskDecision`). The evaluator has `evaluate_row_filter` → `RowFilterDecision::{NoFilter, Filtered(RowFilterResult{policy_id, policy_name, filter_expr}), Undecidable}`, at TABLE granularity | `crates/egret-perm-compile/src/explain_wire.rs:1012`, `:1035`; `EV:3931`, `EV:1708-1723` |
| E6 | `ExplainProfile` = `service_id, service_name, engine, accesses, tags, tag_policies_in_download, mask: Option<ExplainMask>` (`skip_serializing_if`), `masked_read_skips_access`. Fields serialize snake_case | `PR:674-781`, `PR:749`, `PR:778`; `ExplainMask` `PR:784-791` |
| E7 | `ExplainOutcome::NoProfileServesTheCatalog` iff no profile survived the walk; an unknown catalog id is a 400 | `PR:547-557`, `PR:2599-2604`; `PR:2350-2361` (findings) |
| E8 | `ExplainAttribution::Rule { rule_id, locator }` carries no name; `Clearance { tag_id, tag (tags.key), required_roles, locator }`; the UI renders "View rule {uuid}" | `PR:573-626`; `ER:621`, `ER:662` |
| E9 | A rule's compiled mask has the exempt item first with `MASK_NONE` (a row filter: empty `filterExpr`), then `{public}` with the real mask. So "masked `MASK_NONE` by `perm-rule-X`" means "exempt from X". Which principal exempted the person is lost: groups, roles and workspaces are expanded to usernames at compile time, and `MaskResult` carries no item index | `crates/egret-perm-compile/src/rules.rs:767-790`, `:795-814`; `crates/egret-perm-compile/src/members.rs:1-20`, `:683-810`; `EV:1672-1704` |
| E10 | Exempt principals are stored in `perm.rule_exemptions (rule_id, kind, ref, mode)`. The compiler's membership reads are `group_members_via`, `role_usernames` (with `MissingUsername::Omit` for a read that must not write) and `workspace_members_via` | `migrations/0154_perm_model.sql:99-105`; `members.rs:557-575`, `:178`, `:431-438`, `:529-550` |
| E11 | `EvalOrderAmbiguous { policy_ids, policy_names }` is raised for every same-band tie of masks or row filters, **agreeing ties included**, and for tied tag policies. It carries no tied outcome. The write guard admits agreeing ties | `EV:908-911`, `EV:4180-4215`, `EV:4720-4735`; `crates/egret-policy/src/ambiguous_arbitration.rs:152-154` |
| E12 | StarRocks clearance: per tagged column a `select` deny plus a priority-1 mask `perm-clearance-{tag}-mask-{c.s.t.col}` whose `{public}` item is `MASK_NULL` and whose cleared users get the **folded** outcome of the rule masks. So a cleared reader's mask attributes to the clearance, even when it is the rule's `MASK_HASH` | `crates/egret-perm-compile/src/clearance_mask.rs:17-35`, `:85-93`, `:179`; test `crates/egret-perm-compile/tests/routes.rs:4606-4625` |
| E13 | StarRocks table-level classification + clearance compiles **nothing**: the expansion reads `tagged_columns_for_service_via`, whose SQL has `a.column_name <> ''`. Observed live. Trino's single tag-service deny covers both | `crates/egret-policy/src/store.rs:1929-1941`, doc `:1225-1229`; `docs/en/plans/2026-10-19-dg-classification-live.md:69-72` |
| E14 | Trino: a tag row filter fires only for a TABLE-level attachment; a column-level classification is a descendant of the table request and filters nothing ("4 rows, not 2"). StarRocks compiles one row filter per table carrying a tagged column, and filters | `docs/en/plans/2026-09-30-sp4d-2b-live-result.md:243`; `rules.rs:361-371` (Trino), `:372-402` (StarRocks) |
| E15 | The hint "On a whole table, this classification counts for clearance on every column." is false on StarRocks (E13). `COMPILES_TO` says "a select deny and a NULL mask per tagged column" without saying a whole-table attachment is not one | `frontend/src/routes/governance/data/AddTagDialog.tsx:39`; `frontend/src/routes/governance/classifications/tabs/ClearanceTab.tsx:28-29` |
| E16 | `RuleEffect` (preview and stored-rule effect) carries `catalogs[].engines[].engineType`, but `columnCount` is `null` for row filters, so it cannot say whether a row filter's classification sits on columns or on tables | `crates/egret-perm-compile/src/effect.rs:74-127`; `tag_reach` `:484-500` |
| E17 | Group fan-out caps at `EXPLAIN_MEMBER_LIMIT = 200` members; one artifact per profile is shared by all members. There is no profile-count cap | `routes/people.rs:343`, `:464-475` |
| E18 | Clearance attribution costs two DB reads (`tag_key`, `gates_for`) per decision, uncached; Grant/Rule cost none. `ClearanceLookups` is the seam unit tests fake | `PR:3246-3290` |
| E19 | Explain holds no transaction: reads are not one snapshot. `/perm/rules/{id}/effect` uses `REPEATABLE READ, READ ONLY` | `PR:2305`; `PR:1807`, `PR:1838`, `PR:1926` |
| E20 | `ExplainResultView` renders `NothingEnforcing` first for that outcome, else one `ProfileBlock` per profile (access rows, `MaskLine` with `maskDecisionText` naming raw types), then the visible limit "Masks on Trino and row filters are not shown here …" and the folded paragraphs | `ER:1216-1243`, `ER:1245-1330`, `ER:874-946`, `ER:953-986`, `ER:1316-1326`, `ER:1421-1428` |
| E21 | `MembersExplainView.profileVerdictText` builds each member's line client-side from accesses and mask | `frontend/src/components/perm/MembersExplainView.tsx:69-74` |
| E22 **corr.** | The column panel already has "Who can read this column?" opening `WhoCanReadDialog`, which explains one user or a group/role's members. "Check a person" is a richer answer there, not a new route | `frontend/src/routes/governance/data/ColumnPanel.tsx:205-209`; `frontend/src/routes/governance/data/WhoCanReadDialog.tsx` |
| E23 | The rule dialog closes on save with a toast; there is no post-create state | `frontend/src/routes/governance/RuleDialog.tsx:555-567` |
| E24 | Glossary mask labels: Redact, Partial (last 4), Partial (first 4), Hash, Nullify, Unmasked, Year only, Custom | `frontend/src/routes/governance/glossary.ts:72-87` |
| E25 | Plugin versions modelled: Trino `2.8.0`; StarRocks `2.8.0`, `2.9.0` (not 2.5.0) | `EV:663-669` |
| E26 | Open PR #741 edits `PR`, `members.rs`, `trigger.rs`, `group_membership.rs` and adds `migrations/0209_username_ledger.sql`. Highest migration on main: `0207` | `git show origin/fix/username-takeover`; `migrations/` |

## 3. User decisions (2026-10-24, not reopened)

- **D1. An exemption names the rule and who exempts the person.** "email: real value (exempt from rule *Hash
  personal data* via group data-team)". The server rebuilds the reason from the perm model (the rule's exempt
  principals and the person's group, role and workspace memberships), because compiled policies hold usernames only
  (E9). Several exempting principals are all named, up to three, then "and N more". A user named directly reads
  "directly".
- **D2. StarRocks table-level clearance is not fixed here.** Explain says the clearance is not enforced on
  StarRocks for that column, and the false hint (E15) is corrected. The compiler fix is the next package (§12).
- **D3. Trino row filter on a column-level classification is not fixed here.** Explain states it, and the rule
  dialog warns before saving such a rule (§5.8). The compiler fix comes later (§12).
- **D4. A StarRocks clearance replay credits both.** "email: hashed by rule *Hash personal data* (cleared for
  PII)".

## 4. Controller decisions (defaults; the user may revisit)

| # | Decision | Why |
|---|---|---|
| C1 | The effect line is built **server-side**, as structured parts plus the joined text, on `ExplainProfile.effect_lines` and `ExplainResponse.effect_line` | Every surface (Check access, Who can read?, member table) says it the same way; parts let the client bold and link without parsing a string |
| C2 | Masks are asked on **Trino and StarRocks** for every column read, with the resolved tags explain already computes | E3/E4: a handler change; StarRocks drops tags in its enricher, so passing them there changes nothing |
| C3 | A **row-filter decision** joins the wire, asked against the TABLE resource with a table-level tag resolution; a column check shows its table's row filter | E5/E14: row filters are table-granular on both engines; a column reader sees the table's rows |
| C4 | **Ties**: the evaluator reports each tied policy's outcome on `EvalOrderAmbiguous`; an agreeing tie reads "N rules agree"; a conflicting one names the rules and says the engine applies one of them, unpredictably | E11: only the evaluator knows each tied yield; re-deriving it in the handler would be a second copy |
| C5 | First line **"Not enforced: <catalog> has no engine."** (or "…'s engines have no policy profile.") when no profile serves the catalog; no per-engine lines | Parent §14.1; the glossary's "Not enforced"; the reason distinguishes the two remedies |
| C6 | **Surface**: the existing Who can read? dialog and every `ExplainResultView` / `MembersExplainView` render the lines. No second button. "Check with a person" after Create is a non-goal | E22, E23; a tag rule has no single target |
| C7 | **Performance**: keep the 200-member cap; cache clearance attribution, rule facts and exemption reasons per request; switch to `PreparedPolicySet` only if the plan's measurement exceeds 1 s for 200 members × 2 profiles | E17, E18 |
| C8 | Explain reads **one `REPEATABLE READ, READ ONLY` snapshot** | D1 reads the model beside the compiled artifact; every membership writer recompiles in its own transaction, so one snapshot makes the reason and the artifact agree (E19) |
| C9 | **D3 warning**: the preview's `RuleEffect` gains `trinoColumnOnly` (tables where the classification is on columns only, in catalogs a Trino engine enforces); a warning plus the C6 "Create anyway" tick. When the rule reaches nothing yet, a static hint, no tick | A count from the data beats a guess; a tick for a hypothetical is noise, and the hint is true for any future attachment |
| C10 | **D2 in explain**: a StarRocks profile lists clearances that the resolved tags reach only from the table (`clearance_not_enforced`) | Computed from the same resolution and gates explain already reads |
| C11 | `ExplainAttribution::Rule` gains `name` | The lines and `DecidedBy` name rules without a client lookup |
| C12 | **No migration.** Land after #741 or rebase onto it; conflicts expected in `PR` and `members.rs` (E26) | Read-side change only |
| C13 | Platform docs (`egret-data/docs`) in a separate docs PR after merge, as the previous package's D5 | Same flow the user chose on 2026-10-23 |

## 5. Design

### 5.1 The effect line

**Shape** (snake_case, like `ExplainProfile`):

```rust
pub struct ExplainEffectLine {
    pub about: EffectAbout,        // column | rows | access
    pub outcome: EffectOutcome,    // see table below
    pub text: String,              // the parts joined; for aria, tests, copy
    pub parts: Vec<EffectPart>,
}
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPart {
    Text { text: String },
    Strong { text: String },                      // the outcome word
    Code { text: String },                        // a column, a filter expression
    Subject { text: String },                     // the username
    Rule { text: String, rule_id: Uuid },         // rendered italic, links to the rule
    Clearance { text: String, tag_id: Uuid },     // links to the classification's Clearance tab
    Principal { text: String, principal_kind: PrincipalKind, r#ref: String },
    Policy { text: String, policy_id: Option<i64> }, // a manual policy
    Catalog { text: String, catalog_id: Uuid },
}
```

`EffectOutcome`: `real_value | masked | nullified | refused | undecided | all_rows | filtered | allowed |
not_enforced`.
`text` is always the concatenation of every part's `text`, asserted by a unit test. The client renders parts, never
parses `text`.

**Where.** `ExplainProfile.effect_lines: Vec<ExplainEffectLine>` (for `read`: the column or table line, then the rows
line; for `write`: one access line). `ExplainResponse.effect_line: Option<ExplainEffectLine>` carries C5's line,
`Some` exactly for `no_profile_serves_the_catalog`.

**Builder.** A pure module `crates/egret-perm-compile/src/explain_effect.rs`: `effect_lines(input) ->
Vec<ExplainEffectLine>`, where `input` is the profile's verdicts plus the facts of §5.4–5.8 (rule names, exemption
reasons, replay, gaps). No DB access, so the wording table below is a table-driven unit test.

**Wording** (column read; `u` = username, `c` = column; *italic* = a rule/clearance/policy part, **bold** = Strong):

| case | line |
|---|---|
| no mask, access allowed | `u` reads `c` as **the real value** |
| rule mask | `u` reads `c` as **hashed** (rule *Hash personal data*) |
| exempt (D1) | `u` reads `c` as **the real value** (exempt from rule *X* via group data-team) |
| exempt, several | … (exempt from rule *X* via group data-team, role analyst and 2 more) |
| exempt, named directly | … (exempt from rule *X* directly) |
| exempt, reason not in the model | … (exempt from rule *X*; the current rule does not name them, so its compiled policy is older than the rule) |
| manual policy mask | `u` reads `c` as **redacted** (manual policy *pii-mask*) |
| Trino clearance deny | `u` is **refused** `c` (clearance *PII*) |
| other deny | `u` is **refused** `c` (grant *G* denies it) / (manual policy *P*) / (no policy allows it) |
| StarRocks uncleared | `u` reads `c` as **NULL** (clearance *PII*) |
| StarRocks cleared, replay (D4) | `u` reads `c` as **hashed** by rule *X* (cleared for *PII*) |
| StarRocks cleared, exempt | `u` reads `c` as **the real value** (exempt from rule *X* via group data-team; cleared for *PII*) |
| StarRocks cleared, no rule | `u` reads `c` as **the real value** (cleared for *PII*) |
| agreeing tie (C4) | `u` reads `c` as **hashed** (2 rules agree: *A*, *B*) |
| conflicting tie | `u`: **undecided** — rules *A* (Hash) and *B* (Redact) apply at the same priority; the engine applies one of them, and which one cannot be predicted here |
| other undecidable | `u`: **undecided** (<cause in words, §5.6>) |
| D2 gap (StarRocks) | appended sentence: Clearance *PII* is **not enforced** here: it is on the whole table, and StarRocks applies a clearance only where it is on a column. |

Mask type words come from the glossary (E24): Redact → "redacted", Partial (last 4) → "partly shown (last 4)",
Partial (first 4) → "partly shown (first 4)", Hash → "hashed", Nullify → "NULL", Unmasked → "the real value",
Year only → "year only", Custom → "a custom expression" (`Code` part with the value). An unlisted type is shown as
stored.

Rows line (read of a table or a column):

| case | line |
|---|---|
| no filter | `u` sees **every row** |
| filter | `u` sees **rows where** `region = 'us'` (rule *X*) |
| exempt | `u` sees **every row** (exempt from rule *X* via group data-team) |
| D3 gap (Trino) | `u` sees **every row**: rule *X* does not filter on Trino here, because *PII* is on a column, not on the table |
| tie / undecidable | as for masks |

A refused read has no rows line. The table target has no mask, so its first line is `u` **can read** /
**is refused** `schema.table` with the same reasons.

### 5.2 Masks on every engine (C2)

In `explain_users`, the mask block (`PR:2502-2535`) runs for `action == Read && target.column.is_some()` on any
engine and passes `resolved_tags.clone()`. `masked_read_skips_access` stays StarRocks-only (`PR:2539-2540`). The doc
at `PR:744-748` is rewritten (E4). A `TagsUnresolved` answer (resolution declined) reads "undecided: egret could
not resolve this column's classifications" with the decline's reason.

### 5.3 Row filters (C3)

- New `ExplainProfile.row_filter: Option<ExplainRowFilter { access_type: "select", decision:
  WireRowFilterDecision, attribution, exemption }>`, present for every `read` whose target names a table.
- `WireRowFilterDecision = no_filter | filtered { result: { policy_id, policy_name, filter_expr } } | undecidable {
  why }`, in `explain_wire.rs` beside `WireMaskDecision`.
- The request is `object_request(engine, catalog_name, &table_scope)`, `table_scope` = the target without column.
- Its tags come from a **second** `resolved_tags_for_target(table_scope)`, once per request (a column-level tag must
  not reach the table request; that is E14's mechanism, and the evaluator reproduces it only when given the
  table-level set).

### 5.4 Exemption reasons (D1)

When the deciding item is a rule's exempt item (a `MASK_NONE` mask or an empty filter from a policy that attributes
to `Rule`; E9), the server computes `ExplainExemption { via: Vec<ExemptVia>, more: u32 }`:

1. Read the rule's `perm.rule_exemptions` rows with `mode = include` (E10).
2. For each: `user` → matches when the ref equals the username ("directly"); `group` → `group_members_via`;
   `role` → `role_usernames(…, MissingUsername::Omit)`; `workspace_members` → `workspace_members_via`. These are the
   compiler's own reads, so the reason and the compiled body cannot disagree on one snapshot (C8). No mint.
3. Order: directly, then groups, roles, workspaces, each by name. Keep three; `more` counts the rest.
4. Empty `via` (the compiled policy names the person, the current rule does not) is stated, not hidden (§5.1).

Cached per request by `(rule_id, username)`; membership reads are cached per principal.

### 5.5 StarRocks clearance replay (D4)

When a StarRocks mask attributes to `Clearance` and its locator starts with `mask-` (E12), and the type is not
`MASK_NULL` (the person is cleared):

- re-evaluate `evaluate_data_mask` over the artifact **without** every `perm-clearance-*-mask-*` policy, the set
  `fold_clearance_mask` folds over (`clearance_mask.rs:17-35`);
- the result (`Masked` by a rule, `NoMask`, or undecidable) becomes `ExplainMask.replayed: Option<ExplainReplay {
  decision, attribution, exemption }>`;
- the line credits the replayed rule and the clearance (§5.1). `MASK_NULL` from the clearance needs no replay: the
  person is not cleared.

### 5.6 Several masks (C4)

- `UndecidableCause::EvalOrderAmbiguous` gains `outcomes: Vec<TiedOutcome>`, one per tied policy, in the same order
  as `policy_names`. `TiedOutcome = Mask { mask_type, masked_value, mask_condition } | RowFilter { filter_expr }`.
  The access-path site (`EV:2405`) leaves it empty. The two sub-path sites (`EV:4207`, `EV:4732`) fill it.
- `WireUndecidableCause::EvalOrderAmbiguous` carries it (`explain_wire.rs:638`, `:842`).
- The builder: all outcomes equal → "N rules agree"; otherwise name each rule with its mask type. The decision on
  the wire stays `undecidable` (the evaluator still cannot name the policy). `masked_read_skips_access` is not set
  for an agreeing `MASK_NULL` tie, so the access verdict stays, with the existing provisional note (§10).
- Other causes in words: `TagPriorityArbitrationNotModelled` → "classification rules at different priorities decide
  this, which egret does not model"; `TagsUnresolved` (§5.2); `MaskExpression`/`RowFilterExpression` → "the policy
  uses an expression egret does not evaluate"; any other → "egret does not model this policy set (<kind>)".

### 5.7 Not enforced (C5)

For `no_profile_serves_the_catalog`, `ExplainResponse.effect_line` = "Not enforced: *<catalog>* has no engine." when no
engine is attached to the catalog, else "Not enforced: *<catalog>*'s engines have no policy profile." The reason
reuses `effect::NotEnforced::{NoEngine, NoProfile}` and the same attachment read as `attached_engines`
(`effect.rs:393`). The badge title becomes "Not enforced" (the glossary term); `NOTHING_ENFORCING_TEXT` stays under
the details.

### 5.8 Engine gaps (D2, D3)

**D2, explain.** On a StarRocks profile, `clearance_not_enforced: Vec<ExplainClearanceGap { tag_id, tag_name }>`
lists each resolved tag that (a) resolves only as `ANCESTOR` (no column attachment for the target) and (b) has a gate
with at least one required role (`clearance_gates_for_tags_via`, `crates/egret-policy/src/store.rs:1909`). The line
appends the D2 sentence. Trino needs nothing (its tag deny reaches columns as `ANCESTOR`).

**D2, copy.**

- `AddTagDialog.tsx:39` becomes: "On a whole table, this classification counts for clearance on every column on
  Trino. StarRocks applies a clearance only where the classification is on a column. A rule that masks by
  classification needs it on columns, not on the table."
- `COMPILES_TO` (`ClearanceTab.tsx:28-29`) gains: "On StarRocks, a classification on a whole table compiles no
  clearance."

**D3, explain.** On a Trino profile with a usable tag service, `row_filter_not_applied: Vec<ExplainRowFilterGap {
rule_id, rule_name, tag_name }>` lists each row-filter rule on a classification that is attached to a column of the
target's table and not to the table itself. The rows line states it (§5.1).

**D3, rule dialog (C9).**

- `RuleEffect.trinoColumnOnly: Option<TrinoColumnOnly { catalogs: Vec<String>, tables: u32 }>`: for a row filter on
  a classification, the tables where the classification is on at least one column and not on the table, in catalogs
  with an enforcing Trino engine. `null` otherwise.
- When present, the preview shows a warning: "On Trino this row filter filters nothing on N tables in <catalogs>:
  *<classification>* is on their columns, not on the table. StarRocks filters them. Attach *<classification>* to
  the table to filter on Trino." Create needs the existing "Create anyway" tick (`RuleDialog.tsx:391-392`).
- Always, for Applies to = Classification and kind Row filter, a one-line hint under the kind: "On Trino, a row
  filter by classification applies only where the classification is on the whole table."

### 5.9 Snapshot, cache, cost (C7, C8)

- `explain` and `explain_principal` begin a transaction, `SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ
  ONLY`, and run `check_explain_target` and `explain_users` on it. Step 0 of the plan confirms that the path writes
  nothing (`served_artifact_via`'s lineage transform included).
- `ExplainCache` per request: clearance attribution by policy name (wrapping `ClearanceLookups`, so the unit-test
  seam stays), rule facts by id (`name`, kind, exemptions), exemption reasons by `(rule_id, username)`, membership
  reads by principal, tag name → `(tag_id, key)`.
- Added work per member × profile: one mask call on Trino, one row-filter call, at most one replay. Per request: one
  extra tag resolution, one row-filter-gap query, one clearance-gap query.
- Measurement (plan Task 8): 200 members, a Trino and a StarRocks profile, one mask rule with a group exemption, one
  row filter, one clearance. Bound: the `explain_principal` handler under **1 s** in a release test build. Above it,
  build `PreparedPolicySet` once per profile and call the `_prepared` variants (`EV:1925`, `:3904`, `:3957`).

### 5.10 Frontend

- `ExplainResultView`: `effect_line` first for the no-profile outcome (badge "Not enforced"); otherwise each
  `ProfileBlock` leads with its `effect_lines`, rendered from parts (`Strong` bold, `Rule` italic link to
  `/governance/rules/:id`, `Clearance` link to `/governance/classifications/:id?tab=clearance`, `Principal` link to
  the person/group/role page, `Code` mono). The access rows, mask line, row-filter line and tags move under a closed
  per-profile `<details><summary>Which policies decided this</summary>`.
- The visible limits line drops "Masks on Trino and row filters are not shown here"; it keeps the manual-policy
  role/group clause. The folded paragraph at `ER:1421-1428` is rewritten to what is now true (masks on both engines,
  row filters at table level, the polling lag).
- `DecidedBy` shows the rule's `name` (C11), falling back to the uuid when `null`.
- `MembersExplainView`: each member row shows that member's first effect line per profile instead of
  `profileVerdictText`; the counts stay.
- `WhoCanReadDialog`: no structural change; it inherits the lines. The column panel button keeps its label.
- Copy fixes of §5.8; the D3 warning and hint in `RuleDialog`/`RulePreview`.
- At 390 px lines wrap; links are real anchors; the outcome is a word, never colour alone (FE-COPY, FE-VISUAL rules
  in `docs/en/frontend-rules.md`).

## 6. Principles

- One sentence per person and engine, built once, on the server.
- Every reason is derived from what decided, with the model only filling in what the compiled body erased (who
  exempted, which rule a clearance replays).
- Say a known engine gap at the place the reader looks; never fix a compiler silently in an explain package.

## 7. Backend changes (enumerated)

1. `egret-ranger-eval`: `TiedOutcome`; `EvalOrderAmbiguous.outcomes` filled at `EV:4207` and `EV:4732`, empty at
   `EV:2405`.
2. `egret-perm-compile/src/explain_wire.rs`: `WireRowFilterDecision`, `WireRowFilterResult`, wire `outcomes` on the
   tie cause.
3. `egret-perm-compile/src/routes.rs`:
   - one read-only snapshot in `explain` and (via `people.rs`) `explain_principal`;
   - masks on every engine with resolved tags; the doc at `:744-748`;
   - row-filter request with a table-level resolution;
   - `ExplainRowFilter`, `ExplainExemption`, `ExplainReplay`, `ExplainClearanceGap`, `ExplainRowFilterGap`,
     `ExplainEffectLine`, `EffectPart`; new fields on `ExplainProfile`, `ExplainMask`, `ExplainResponse`;
   - `ExplainAttribution::Rule.name`;
   - `ExplainCache`.
4. `egret-perm-compile/src/explain_effect.rs` (new): the pure builder.
5. `egret-perm-compile/src/routes/people.rs`: the snapshot transaction for the fan-out.
6. `egret-perm-compile/src/effect.rs`: `RuleEffect.trino_column_only`.
7. `openapi.json` and `frontend/src/api/generated/**` regenerated (`npm run gen:api`).

## 8. Wire shapes

```json
{
  "outcome": "per_profile",
  "profiles": [
    {
      "service_id": 12, "service_name": "trino-main", "engine": "trino",
      "accesses": [{"access_type": "select", "decision": {"kind": "allowed", "policy_id": 40, "policy_name": "perm-grant-…"}, "attribution": {"kind": "grant", "grant_id": "…"}}],
      "mask": {
        "access_type": "select",
        "decision": {"kind": "masked", "result": {"policy_id": 51, "policy_name": "perm-rule-…", "mask_type": "MASK_NONE", "masked_value": null, "mask_condition": null}},
        "attribution": {"kind": "rule", "rule_id": "…", "locator": null, "name": "Hash personal data"},
        "exemption": {"via": [{"principal_kind": "group", "ref": "data-team", "label": "data-team"}], "more": 0}
      },
      "row_filter": {"access_type": "select", "decision": {"kind": "no_filter"}, "attribution": null},
      "row_filter_not_applied": [{"rule_id": "…", "rule_name": "US rows only", "tag_name": "PII"}],
      "effect_lines": [
        {"about": "column", "outcome": "real_value",
         "text": "member-sso reads email as the real value (exempt from rule Hash personal data via group data-team)",
         "parts": [
           {"kind": "subject", "text": "member-sso"}, {"kind": "text", "text": " reads "},
           {"kind": "code", "text": "email"}, {"kind": "text", "text": " as "},
           {"kind": "strong", "text": "the real value"}, {"kind": "text", "text": " (exempt from rule "},
           {"kind": "rule", "text": "Hash personal data", "rule_id": "…"}, {"kind": "text", "text": " via group "},
           {"kind": "principal", "text": "data-team", "principal_kind": "group", "ref": "data-team"},
           {"kind": "text", "text": ")"}
         ]},
        {"about": "rows", "outcome": "all_rows", "text": "member-sso sees every row: rule US rows only does not filter on Trino here, because PII is on a column, not on the table", "parts": ["…"]}
      ]
    }
  ]
}
```

`no_profile_serves_the_catalog`: `"profiles": []` and `"effect_line": {"about": "access", "outcome": "not_enforced",
"text": "Not enforced: demo_warehouse has no engine.", "parts": [...]}` (a `catalog` part names the catalog).

New optional fields use `skip_serializing_if` like `mask`, so an older client ignores them.

## 9. Migration and compatibility

- No database migration (C12).
- Additive wire: new optional fields; `ExplainAttribution::Rule.name` is added, nothing removed. The tie cause gains
  a field.
- `masked_read_skips_access` keeps its meaning (StarRocks only).
- Merge risk: PR #741 touches `PR` and `members.rs`. Land after it or rebase; no migration number to coordinate.

## 10. Risks

- **The model reason can lag the artifact** only outside one snapshot or when a writer forgot to recompile; the line
  then says so (§5.4 step 4) instead of inventing a principal.
- **Agreeing `MASK_NULL` tie on StarRocks** keeps the access verdict (provisional note). Conservative: may overstate a
  refusal, never an exposure.
- **Row filter on Trino for a column target** uses the table-level resolution; a lineage-tagged view row is
  resolved like any other assignment (`view_tags.rs:1-30`). Not measured for views; the live check covers tables.
- **Evaluator change** touches the tie cause the differential gate reads; `just test-ranger-gates` must pass.
- **Cost** grows by ~2 evaluator calls per member × profile; bounded by the cap and measured (§5.9).
- **A `CUSTOM` mask or `MASK` on a non-varchar column** can make Trino fail the query (live case 4d,
  `2026-09-30-sp4d-2b-live-result.md:250`); the line cannot predict an engine error.

## 11. Related defects found (not fixed here)

1. A groups-only clearance gate compiles nothing on either engine (`crates/egret-perm-compile/src/clearance.rs:64-71`);
   stated in the Clearance tab already.
2. The plugin serves the tag map it last fetched; explain reads current rows (`ER:1430-1433`). Unchanged.

## 12. Non-goals

- **The StarRocks table-level clearance compile** (D2): the next package. It must compile a per-column deny and mask
  for every column of a table carrying the classification at table level (`store.rs:1941`).
- **The Trino column-level row-filter compile** (D3): a later package.
- **"Check with a person" after Create** in the rule dialog (C6): the dialog closes on save (E23) and a tag rule has
  no single target.
- **Effect lines for `write`** beyond a summary of the access verdicts.
- `PreparedPolicySet` unless the measurement asks for it (C7).
- Platform docs in this PR (C13).

## 13. Open questions

None blocking. C1–C13 are defaults the user may revisit.

## 14. Implementation notes (append-only)

Deviations the code forces are recorded here, per task, as they happen.

### Step 0 (re-verification, 2026-10-24)

- Base moved to `origin/main` `93a9f135` (frontend-only dashboard changes); the branch was rebased. No backend file
  of §2 changed, so the line numbers hold. PR #741 is still open; highest migration is still `0207`.
- **E5 is half-wrong.** `WireRowFilterDecision` and `WireRowFilterResult` already exist in `explain_wire.rs`
  (since #606, with `From<&RowFilterDecision>` and wire tests in `tests/wire_dtos.rs`), unused by any response. What
  is true is that explain emits no row-filter decision (`ExplainProfile` has no such field). Task 4 reuses the
  existing types instead of adding them; no decision changes.
- E3, E4, E9, E11, E12, E13, E14, E15 hold as written.
- E19/C8: no write on the explain path (`check_explain_target`, `services_for_catalog_via`, `served_artifact_via`,
  `envelope_via`, `lineage_catalogs_for_poll_via`/`eligible_bound_engines_via`, `resolved_tags_for_target`,
  `profile_plugin_version`, `principal_members_via`). `routes/people.rs` already has a `snapshot!` macro
  (`REPEATABLE READ, READ ONLY`, rolled back) used by the people reads; `READ ONLY` is kept.

### Task 1 (tied outcomes)

- `TiedOutcome` is built by a private `TiedOutcome::of(&SubResult)`, not a `From` impl, because `SubResult` is
  private to the evaluator.
- The tag-band case is asserted on the existing tied pair `two_mask_policies_tied_on_every_key_ranger_has_refuse`
  (`src/tag_eval_tests.rs`, `MASK_HASH` vs `MASK_NULL`) instead of a new test in `tests/tag_arbitration.rs`; the
  access-tie "outcomes empty" check is added to `a_tag_deny_makes_a_resource_band_s_eval_order_decide_and_that_refuses`
  there.
- `WireTiedOutcome` (`kind: mask | row_filter`) is registered in the OpenAPI components beside
  `WireUndecidableCause`; the wire omits an empty `outcomes`.

### Task 2 (snapshot, cache, rule names)

- Merged `origin/main` `624ec61f` (#741) after Task 1: no conflict, no generated-client change yet.
- The cache holds only what is read so far: clearance attributions by policy name and `perm.rules` rows by id
  (`egret_perm::Rule` from `get_rule_via`, which already carries the exemptions, so no separate `RuleFacts`).
  Exemption, membership and tag entries join it in the tasks that read them.
- The rule read is a second seam trait `RuleLookups` beside `ClearanceLookups` (the wiring tests' fake implements
  both) rather than a method on `ClearanceLookups`, whose name and docs are about clearances.
- `ExplainAttribution::Rule.name` also reaches `POST /perm/engine-grants/explain` (`routes/engine_grants.rs`), which
  calls `attribution_for` with a fresh cache.
- Both snapshots end with `rollback`, as every other read-only snapshot in the crate does.

### Task 3 (masks on Trino)

- The helper `trino_tag_profile` returns the catalog and the **tag** service's id (not the Trino profile's), because
  the table-level and declined-resolution tests write a hand tag mask into it (`hand_tag_mask`). The declined
  resolution uses the fold-divergent column fixture (`emaıl`), as the existing access test does.

### Task 4 (row-filter decision)

- Reuses the existing `WireRowFilterDecision`/`WireRowFilterResult` (Step 0, E5) and registers them in the OpenAPI
  components now that `ExplainProfile.row_filter` returns one.
- `ExplainRowFilter` carries `access_type`, `decision`, `attribution`; `exemption` joins it in Task 5.
- The table-scope resolution's own decline is not reported separately: the table scope's values are a subset of the
  column target's, so it declines only where the column resolution already did (and `tag_resolution_declined` says
  so).
- Mutation: handing the column-level tag set to the table request turns
  `a_trino_row_filter_on_a_column_classification_filters_nothing` red (it filters), which is E14's mechanism.

### Task 5 (exemption reasons)

- **A rule exempts only users and groups.** `validate_rule_shape` → `exempt_principals` refuses a `role` (no roles
  bucket on a mask/row-filter item) and an unexpanded `workspace_members` at write, pinned by
  `a_rule_that_can_never_compile_is_refused_at_write_time` and `compile_rules::a_role_principal_in_exempt_is_refused`.
  So §5.4 step 2's `role_usernames`/`workspace_members_via` branches have nothing to read, and §5.1's "role analyst"
  example cannot occur. D1 is unchanged (the rule and who exempts, up to three, "directly" for a named user); the order
  is "directly, then groups by name". The plan's role holder (carol) became a user named directly who is also in
  the group.
- `ExemptVia` is `{principal_kind, ref}` without §8's `label`: with users and groups only, the label always equals
  the ref. A workspace exemption, if ever allowed, adds it back.
- Membership reads use the compiler's `group_members_via`, cached per group per request; reasons are cached per
  `(rule_id, username)`.

### Task 6 (StarRocks clearance replay, D2 gap)

- The tests live in `tests/routes.rs` (`a_cleared_starrocks_reader_s_mask_replays_the_rule`,
  `a_table_level_clearance_is_not_enforced_on_starrocks`, plus `replayed` assertions on the existing cleared and
  uncleared tests), not in `tests/clearance_starrocks.rs`, which drives the compiler without the router.
- The replay removes every policy the attribution parser names a clearance (deny and mask alike), which is the
  fold's own `source_kind != Clearance` set for a mask decision; the condition is "a clearance decided and the type
  is not `CLEARANCE_MASK_TYPE`", without a separate `mask-` locator check (only the clearance mask is a mask).
- The gap is computed once per request for a column read with a resolved tag set, from tags that never resolve at
  the column, joined to `tags` by name (unique since 0207) and to `clearance_gates_for_tags_via` (roles only), and
  attached to StarRocks profiles only.

### Task 7 (effect lines, Not enforced, D3 in explain)

- `explain_effect::effect_lines` reads the finished `ExplainProfile` plus two maps the profile does not carry: the
  model object of every policy a tie names (`tied`, by Ranger name) and `tags.name` by tag id for clearances (a
  `Clearance` attribution carries `tags.key`, and the line names the classification). Both come from the
  per-request cache (`ExplainCache::tag_name` is new).
- Wording decisions the §5.1 table left open:
  - A grant has no name, so a grant deny reads "(a grant denies it)"; an engine grant "(an engine grant denies it)".
  - A person named directly AND in an exempting group reads "(exempt from rule *X* directly and via group G)".
  - A refused read has no rows line; an undecided access keeps it.
  - A replay that is a manual policy reads "as **redacted** by manual policy *P* (cleared for *PII*)"; an
    undecidable replay keeps the clearance's folded word "(cleared for *PII*)".
  - The D3 sentence names the classification as plain text (there is no classification part kind), and several
    gap rules read "rules *X* and *Y* do not filter on Trino here, because their classifications are on a column,
    not on the table".
  - An agreeing tie of non-rule policies reads "N policies agree"; a conflicting row-filter tie shows each filter as
    code.
  - `write`: refused names the first refused access type ("is **refused** delete on `t` (…)"); otherwise undecided
    or "**can write** `t`".
- `row_filter_not_applied` is attached to a Trino profile whose download has tag policies (a rule compiles into the
  tag service only when it resolves). The gap query folds case on schema and table like the tag resolution.
- `ExplainResponse.effect_line` uses `effect::catalog_not_enforced` (new), the same attachment read as
  `attached_engines`: no attached engine → `NoEngine`, else `NoProfile`.

### Task 8 (cost)

- `explain_200_members_on_two_engines_timing` (`tests/explain_principal.rs`, `#[ignore]`), release build, local
  Postgres, 2026-10-24: 200 members × a Trino and a StarRocks profile, a mask rule and a row filter exempting a
  10-member group, a gated classification on the column. `POST /perm/explain/principal` for a column read, 5 runs:
  **min 61.6 ms, median 70.2 ms, max 88.9 ms**. Under the 1 s bound, so no `PreparedPolicySet` (C7). The test asserts
  the median stays under 1 s.

### Task 9 (D3 count on the rule effect)

- `RuleEffect.trino_column_only` (`trinoColumnOnly`) serialises as `null` when absent, like the other optional
  effect fields, rather than being omitted. "Catalogs with an enforcing Trino engine" reads the effect's own
  `catalogs[].engines` (for a classification rule these are already only engines whose profile hosts it). Levels
  compare case-folded. `null` also when no table qualifies.
