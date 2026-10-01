# SP4d-2b — evaluate the tag policies the engine is enforcing

**Date:** 2026-09-30
**Status:** design
**Predecessors:** SP4d-1 (`crates/egret-ranger-eval`, `6acb1d68`), SP4d-2a (`explain` over the served artifact, `7c646c27`)
**Successor:** SP4d-3 (classification, migration, the infrastructure guardrail)

## Why this exists

SP4d-2a made `explain` answer for the artifact each engine downloads. It refuses
to answer for a profile whose tag service carries a policy, because tag policies
are not evaluated — and that refusal is installation-wide: one tag-addressed rule
or clearance anywhere puts every verdict on every catalog that profile serves
into `Undecidable`.

That refusal is correct and was measured, not argued. But the one engine-facing
Trino profile in the cluster is exactly such a profile: its plugin has **ingested**
three tag policies, one of them a deny-capable clearance, with `policyVersion`
matching egret's store on both halves. So the package that made `explain` honest
also left it silent about the profile that is actually enforcing.

This package lifts the refusal by doing the work: evaluating tag policies.

## Measured premises

Measured 2026-09-30 from `ranger-plugins-common` 2.5.0 and 2.8.0 bytecode, and
from payloads captured by the predecessor packages. Anything not measured is
labelled.

### The enricher uses the matcher this crate already has

`RangerTagEnricher.enrich` → `findMatchingTags` builds a real
`RangerDefaultPolicyResourceMatcher` per tag service resource
(`createRangerServiceResourceMatcher`: `setPolicyResources(resourceElements,
policyType)`, `setServiceDef`, `setServiceDefHelper`, `init`), wraps it in a
`RangerServiceResourceMatcher` whose `getMatchType` is a one-line delegation, and
calls it with the **tag's service resource in the policy position** and the
**query's resource in the request position**.

So SP4d-2b needs a **different caller, not a new algorithm**.
`crates/egret-ranger-eval/src/resource.rs` — `match_type`, `matching_hierarchy`,
`init` — already *is* the algorithm, ported and differentially tested against the
real Java engine over thousands of generated cases.

**One rule serves both engines.** `RangerDefaultPolicyResourceMatcher.getMatchType`
is byte-identical between 2.5.0 and 2.8.0 after constant-pool normalisation (181
instruction lines each), and so is `RangerTagForEval`. Trino 470 runs 2.5.0 and
StarRocks fe-ubuntu 4.1 runs 2.8.0; they need one implementation.

### The MatchType each attachment level produces

Request naming `catalog/schema/table/column`, default `SELF` scope, a concrete
access type:

| Tag attached at | MatchType | Admitted by the enricher |
|---|---|---|
| the exact column | `SELF` | yes |
| the table | `ANCESTOR` | yes |
| the schema | `ANCESTOR` | yes |
| the catalog | `ANCESTOR` | yes |
| deeper than the request | `DESCENDANT` | **no** |
| deeper, but wildcard (`column: "*"`) | `SELF_AND_ALL_DESCENDANTS` | yes |

The two versions write the admitted-set filter differently (`!= NONE` versus an
explicit four-way disjunct) and are **semantically identical**; the enum has five
members. 2.8.0 adds a null-guard defaulting the matching scope to `SELF`.

**Only the `SELF` row is exercised by live data.** All six live tag assignments
are column-level. The `ANCESTOR` rows, the `DESCENDANT` exclusion and the
wildcard row are derived from bytecode, not observed on an engine.

### egret's storage cannot express a schema- or catalog-level tag

`public.catalog_tag_assignments` has `column_name text NOT NULL DEFAULT ''` with a
documented convention that `''` means table-level. `schema_name` and `table_name`
are `NOT NULL` with **no such convention**, and the primary key is
`(tag_id, warehouse_id, schema_name, table_name, column_name)`.

So of the enricher's rule: **column-level and table-level are representable and
computable today; schema-level and catalog-level are not representable at all.**

### A live fail-open in the existing resolver

`egret_policy::tag_keys_for_resource_via` returns **`tags.key`**. The engine
matches tag policies on the wire tag's `type`, which
`ServiceTags::from_assignments` fills from **`tags.name`**. Those are two
different columns, and live they differ for one of three tags (`sp4cgate` versus
`sp4cgate2`). Same shape, different identifier — a silent fail-open if reused for
evaluation.

### Tag validity and attributes

`RangerTagForEval.isApplicable` returns `true` for every access time when the
tag carries no `options` and no `validityPeriods`, and egret emits neither — its
wire tag is `{"type": …, "owner": 0}`. `getLikelyMatchPolicyEvaluators` filters on
`isApplicable` **twice** (once per tag, once per policy), and both are neutral
under egret's emit.

But that is a property of **egret's emitter**, not of the model. And tag
*attributes* are different in kind: a tag policy referencing `${{TAG.<attr>}}`
expands against an empty map, so egret's answer would be an empty string where
Ranger's is a value.

## Decisions

### D1 — the resolved tag set is an input, computed with the crate's own matcher

The caller resolves the tag set and hands it in, as `resolved_tags` already
allows. The crate gains the ability to *evaluate* that set; it does not gain a
database. The resolution runs the same `match_type` the crate uses for policies,
with the tag assignment in the policy position.

### D2 — evaluate the two levels egret can express, and refuse the rest by shape

Column-level and table-level attachments are resolved and evaluated. The
schema- and catalog-level arms of the enricher rule are **unreachable from
egret's own store**, and the package narrows on that — but **asserted, not
assumed**: if a download carries a tag service resource with fewer than three
resource elements, the payload did not come from `from_assignments`, and the
answer is `Undecidable` naming that.

Schema- and catalog-level tagging is a **feature**, not part of evaluation. It
needs a migration, a writer and a UI, and it gets its own package. Because the
MatchType rule is measured and implemented here, that package will only have to
make the rows expressible.

### D3 — evaluation matches on `tags.name`; `tags.key` stays on the attribution surface

The set handed to a tag evaluator carries `tags.name`, because that is what the
wire carries and what the engine matches. `tags.key` remains correct for
`explain`'s attribution display. A single function must not return both.

### D4 — the same tag at two levels appears twice, and must not be deduped

`RangerTagForEval`'s `equals` and `hashCode` include `matchType`, so a `HashSet`
does not collapse the same tag reached through two service resources at different
levels. That is not an implementation detail: the MASK evaluators are ordered by
`MATCH_TYPE_COMPARATOR`, ascending `SELF < SELF_AND_ALL_DESCENDANTS < ANCESTOR`,
so two attachments of one tag at different levels produce a **different masking
answer** depending on their match types. A resolution that deduped by tag name
would silently reorder masks.

### D5 — two phases, with Ranger's three-way priority arbitration

Tag policies are consulted first. The tag verdict's `isAccessDetermined` is then
cleared so resource policies can still decide, and restored afterwards if nothing
else determined. The arbitration is three-way and asymmetric:

- tag **deny** versus a resource policy: `>=` — only a strictly higher-priority
  resource policy still runs and can reopen;
- tag **allow** versus a resource **access** policy: `>` — a same-or-higher
  priority resource policy still runs and can deny;
- tag **allow** versus a resource **mask or row-filter** policy: `>=`.

### D6 — three comparators, by policy type

Access and audit order by eval order; **mask orders by tag match type**;
row-filter orders by name. The crate's existing `EvalOrderAmbiguous` refusal
gains a new axis on the mask path and must be re-measured, not assumed to hold
its current rate.

### D7 — validity periods are provably inert, and still asserted

The package asserts that a parsed tag carries no `options` and no
`validityPeriods`, and refuses if either appears. Ignoring them because egret
cannot emit them would be sound today and silently wrong the day a hand-written
`/tags/download` shim, an imported payload, or a new egret feature emits one.

### D8 — tag attributes are a refusal, not an omission

A matched tag policy whose condition, mask value or row-filter expression
references a tag attribute is `Undecidable`. Unlike a validity period, whose
absence means "no constraint", an absent attribute expands to an **empty string**
— a value, and a different one from Ranger's.

### D9 — a tag service resource whose hierarchy does not validate is dropped

`createRangerServiceResourceMatcher` returns null when no policy type validates
the resource's key set, and `processServiceTags` then removes both the resource
and its `resourceToTagIds` entry. `isEnabled` on a service resource is **never
consulted**. Both behaviours are reproduced.

### D10 — `required_roles` becomes an `Option` on the wire

SP4d-2a's frontend guards three cases the wire conflates, re-deriving the
distinction from a second field because the first cannot carry it. Making the
field an `Option` moves "could not resolve" from guarded to unrepresentable.

It is an honest 3→2, not 3→3: "declares only groups" and "no
`classification_policies` row" both legitimately produce `Some([])` and stay
conflated. Roughly eight files and forty lines of non-generated change, plus one
document and one client regeneration.

## Non-goals

- **Schema- and catalog-level tag assignment.** Its own package: migration,
  writer, UI. The MatchType rule for it is measured and implemented here.
- **Tag evaluation on StarRocks.** ⚠ **CORRECTED by this branch's own
  measurement (whole-branch review).** This non-goal was justified with "its
  service-def declares no enricher, so no tag reaches the engine" — and that is
  the **single-def reasoning this branch proved fail-open**. A download carries
  **two** defs (the resource service's and the tag block's),
  `RangerPolicyEngineImpl.<init>` builds context enrichers from both and
  **unions the two lists**, and Apache's stock tag def declares
  `RangerTagEnricher`. So a StarRocks profile given a `tagService` DOES get an
  enricher and tags DO reach the plugin; what stops them is one level further
  on — `from_assignments` writes the literal keys
  `catalog`/`schema`/`table`/`column` for every engine, the StarRocks def has no
  `schema` resource (its middle level is `database`), and
  `RangerTagEnricher.processServiceTags` DROPS a service resource whose key set no
  hierarchy of any policy type contains. Measured on the real jar by
  `EgretTagServiceResourceValidationTest`, and the same mechanism the plan's
  Item A describes.
  The non-goal stands; only its reason changes, and the reason matters because
  the old one would have licensed skipping the tag phase for a StarRocks profile
  — a fail-open. The gate stays on what the DOWNLOAD carries (both defs, and the
  attachment's own key set), never on the engine type and never on one def.
- Changing what the compiler emits into the tag service.
- The `explain` mask and row-filter surface. The wire mirrors stay unregistered
  until a field returns one.

## Open question for Task 1

Does the crate's `EvalOrderAmbiguous` rate change once mask evaluators are
ordered by tag match type? It was 3.4–4.0% for resource policies. Measure it;
do not carry the old number forward.
