# SP4b — the permission UI: intent for stewards, structure for admins

**Status:** design, approved in sections 2026-09-27.
**Follows:** SP4a (PR #596), which built the model, the compiler and the HTTP surface but no UI.

## Goal

Make egret's data-permission model usable by someone who thinks in sentences — "the analytics team can read sales data except the PII columns" — without taking away what an administrator needs, which is the opposite: exactly which Ranger policies exist, in which profiles, and why a particular query was refused.

Ranger is the reference for **coverage** and the counter-example for **UX**. This spec copies neither its screens nor its vocabulary.

## Why this needs backend work before any UI

A survey of SP4a's HTTP surface found twelve things a UI cannot express against the API as it stands. Most are ordinary composition work. Three are not, and two of those were decided to require backend changes rather than UI workarounds.

**D1 — Grants and rules become editable in place (`PATCH`).** Today they are immutable and there is no `PATCH`, so any "Edit" button would really delete and recreate: a new id, two recompiles, and a window in which the row does not exist. A deep link from an `explain` reason to a grant id would break on every edit. **A UI control that claims to edit while actually replacing is the kind of lie this programme has refused repeatedly**, so the backend gains `PATCH` for grants and rules: the id survives, and the affected profiles recompile once, in the same transaction as the write, the way every other mutation on this model already does.

**D2 — A dry-run endpoint, because the blast radius is not local.** `affected_services_for_grant`/`_for_rule` run only inside the write transaction, so the API cannot answer "which profiles would this touch?" before committing. That matters more here than it would elsewhere, for a reason SP4a measured: **a Trino tag rule is catalog-unscoped by design, so one rule mutation recompiles every Trino profile in the installation** — and because a compile failure anywhere rolls the whole write back, the blast radius is also a failure radius. A steward who cannot see that before pressing the button learns it from a 400 naming a profile they have never heard of. The backend gains a preview that reports the affected profiles and any compile error, and writes nothing.

**D3 — A group grant compiles and enforces nothing, and only the UI can say so.** SP4a's D7 measured this live: Trino has no group provider, egret writes none and has no usersync, so a Ranger policy item naming a group matches nobody. The API has no way to express "this will not be enforced". The picker still offers `group` — the concept is one users expect and will eventually work — but it carries a warning at the point of choice. **This is hardcoded knowledge, and the spec records it as such** so that whoever adds a group provider knows to remove the warning rather than discovering it as a stale surprise.

## The two tiers are two places, not one place with a switch

**D4 — Four items under Data governance, each with one audience.**

```
Data governance
  Data permissions    ← steward: who can do what, as sentences
  Mask & filter rules ← steward: named rules that apply by classification
  Explain access      ← admin: why a subject can or cannot reach a target
  Policies            ← admin: the compiled Ranger structure (already exists)
```

A mode switch on a shared page was considered and rejected: a steward can wander into the structural mode and a switch is easy to forget, while the cost of separate pages — an admin moving between intent and result — is exactly what "Explain access" exists to collapse.

Every page uses the established Control Plane scaffold (`SummaryStrip` + `FilterableTable` + `FacetSelect`), not a new one. `SummaryStrip` is the standard for list pages; the unresolved `SummaryStrip`-versus-`StatCard` question for Engines and Warehouses is untouched by this work.

**D5 — A grant reads as a sentence, and the sentence is the primary column.** Not a resource map. `role analyst can read wh.sales.*` is one row, and `role analyst can read and write wh.sales.* except group contractors` is another; the structural form lives behind the row, and in the Policies page. The subtract (`exclude`) is rendered in the sentence because it changes its meaning, and SP4a chose an explicit principal subtract precisely so this could be said in one line instead of four Ranger policies.

**Corrected 2026-09-27, caught during implementation.** This decision originally illustrated itself with `analyst can read wh.sales.* except column pii`, which **cannot exist**: a grant's only subtract is `principals.exclude`, so `except` always names an excluded **principal**, never a column. Masking a column is a *rule*, on the neighbouring page. The original example conflated the two halves of the product into one sentence and then offered it as a single grant — the sort of plausible-looking example that gets implemented literally if nobody checks it against the model.

## What the UI must refuse on its own

The API accepts several shapes that always fail at write time. The type system says they are legal; the runtime says never. A form that offers them and reports a 400 is a worse form than one that does not offer them.

**D6 — Three combinations are disabled in the form, not merely validated:**
- `workspace_members` as a principal kind. It exists for compiler-internal assignment rows and **always** 400s from this surface, so the picker excludes it. The API exposes no flag distinguishing compiler-internal enum members from usable ones, so this too is hardcoded knowledge.
- `exempt.exclude`, and a `role` principal inside `exempt`. Ranger's mask and row-filter items have no exception block and no roles bucket, so both are always refused.
- A path-selected row filter that does not name exactly catalog, schema and table. `rowFilterDef` declares only those three levels, all mandatory, and no column — measured against the service-def during SP4a.

**D7 — `mask_type` is a fixed list in the UI even though the API takes a free string.** The API exposes no enumeration of valid Ranger mask types. The UI ships Ranger's vocabulary as a `Select`, with `CUSTOM` revealing an expression field, and the spec notes that this list is a hand-maintained mirror.

**D8 — A refusal must say which limit was hit and where.** SP4a's two ceilings refuse rather than truncate: a per-rule expansion cap, and a profile-total cap that refuses only a change which pushes a profile over. The messages already name the rule, the profile and the running total; the UI surfaces them as they are rather than replacing them with a generic failure.

## The admin tier

**D9 — "Explain access" answers the question an audit log cannot.** Subject, action, target in; `allowed`, the reason class, and the tags that were considered out. The reason **class** is the point, not the boolean: `no_grant_matched` and `explicit_deny` are different administrative problems, and Ranger's own audit shows `-` for the first. Reasons that carry a `grant_id` or `rule_id` link to that row, which is why D1's stable id matters.

Two honest limits, recorded because the UI must not imply otherwise: `explain` is blind to hand-written policy, so it can answer "denied" for a subject the engine allows through a seeded admin policy; and `tags` populate only when the target names a schema and a table.

**D10 — The Policies page finally tells an operator before the overwrite, not after.** `ProfileDto.origin` has carried `generated | manual` since SP4a with a comment saying the frontend must show it — and nothing has ever read it. SP4b is its first consumer: a generated profile is marked, and a refused write explains that the compiler owns that policy rather than surfacing a bare error. SP4a deliberately scoped the guard to the policy rather than the profile, so a foreign policy inside a generated profile stays editable; the UI must reflect that distinction instead of freezing the whole profile in appearance.

## Deliberately not in SP4b

- Migrating the hand-written policies that already exist, folding in Cedar classification clearance, and the per-workspace notebook role — that is SP4c.
- Pagination and `GET`-by-id on grants and rules. At present the whole model loads in one request and a detail view filters it client-side; this is a real limit and it is recorded, not fixed.
- A principal lookup under `/perm`. The raw-JSON editor's existing principal endpoint is reused rather than duplicated, and the UI resolves a display name to the bare username the model requires.
- Structured skip reasons. `SkipReport` is free text today, so a "some rows did not compile" banner can only show prose.
