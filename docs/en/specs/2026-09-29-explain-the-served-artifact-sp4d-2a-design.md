# SP4d-2a — `explain` answers for the artifact the engine loads

**Date:** 2026-09-29
**Status:** design
**Predecessor:** SP4d-1 (`crates/egret-ranger-eval`, merged as `6acb1d68`)
**Successor:** SP4d-2b (tag-policy evaluation), SP4d-3 (classification and migration)

## Why this exists

`explain` answers "may this subject do this?" by evaluating egret's own
permission model. The engine answers by evaluating the compiled artifact it
downloads from egret. Those are two implementations of one question, and they
have already disagreed in production: SP4c measured `explain` reporting a
decision the engine did not make.

SP4d-1 built an evaluator that predicts the engine's answer and proved it
agrees with the real Java `RangerPolicyEngineImpl` over 10000 generated cases at
two plugin versions, and with a real Trino and a real StarRocks on ten live
cases. This package routes `explain` through it.

The point is not a second opinion. It is that after this change there is **one**
answer, and it is the engine's. The disagreement class disappears rather than
becoming visible.

## What the operator gets that they do not have today

Today the page carries four honest-limits paragraphs. The first says `explain`
cannot see hand-written Ranger policy. That is the paragraph this change
**deletes** — and with it a concrete blindness: auto-bind seeds thirteen
`egret-admin` policies granting `*.*.*.*` on every generated profile
(`crates/egret-policy/src/default_policies.rs`), and `explain` cannot see any
of them today. An operator asking "why can this person read that?" gets "no
grant matched" while the engine allows.

## Measured premises

Measured 2026-09-29 against the live `kind-egret` cluster and the Ranger jars.
Anything not measured is labelled.

### One catalog is served by several profiles, and their answers legitimately differ

Live: warehouse `sp4a10_wh` is attached to both engines, and both perm grants on
it compiled to `{"services":[52,55]}` — profile 52 (Trino) and 55 (StarRocks).
Three facts make a single verdict inexpressible:

1. Access types differ: `write` is `insert,delete` on Trino and
   `insert,update,delete` on StarRocks (`crates/egret-perm/src/actions.rs`).
2. The second resource level is **named** differently — `schema` vs `database`
   (`crates/egret-perm-compile/src/grants.rs`) — so one `RequestResource`
   cannot serve both.
3. They differ in what they even carry: 52 has a non-empty `tagPolicies` block,
   55 has no tag service at all.

**No test in the repo attaches one catalog to two engines on two profiles.**
That fixture is new work here; without it nothing tests the response shape.

### Roles and groups do not reach any engine egret provisions

egret serves no `/service/roles/download`, so the Trino plugin passes
`userRoles` as null; egret runs no usersync, so `getGroups(user)` is empty on
every engine egret provisions. The literal `public` is the exception — Ranger
matches it by name, not by membership.

### The `tagPolicies` block has three tiers, and two are provably inert

`crates/egret-policy/src/ranger/api/download.rs` embeds the block whenever a
service names a `tagService`, and `bind_trino_profile` always creates one. The
evaluator refuses any envelope carrying it. Measured in both jars:

| Tier | State | Engine behaviour |
|---|---|---|
| (a) | block absent | nothing to consider |
| (b) | block present, `policies` **empty** | `evaluateTagPolicies` returns at `isNotEmpty(evaluators)` — **the shared result is not touched** |
| (c) | block present, non-empty | tag policies run **first** and can deny |

A non-empty block is **also** inert when the request's tag set is empty
(`isNotEmpty(tags)` false ⇒ `EMPTY_LIST`) — but egret cannot currently establish
that its tag set IS empty. `tag_keys_for_resource_via` matches schema, table and
column by exact equality with **no hierarchy roll-up**, while Ranger's enricher
walks the hierarchy and emits ancestor tags that Ranger admits for a tag request.
So "egret found no tags" does not imply "the engine found no tags", and using that
narrowing on the existing resolution would be **fail-open**. Every live tag
assignment is column-level, so the ancestor path is unexercised by current data —
the shape that ships green and wrong.

Tier (b) is not hypothetical: every future auto-bound Trino profile lands there
with zero tag policies and is refused today. Tier (c) is exactly **one** live
profile — 52 — and its plugin has *ingested* the block: the cached
`trino_sp3c2-trino-live_tag.json` holds all three policies and its
`policyVersion 22` / `tagVersion 10` match the store on both halves. The engine
is enforcing tag policies right now, so tier (c) cannot be narrowed by argument.

### The trap SP4d-1 left, re-verified

`RangerPolicyEngineImpl.<init>` builds a `ServiceConfig` from
`ServicePolicies.getServiceConfig()`, carrying `ranger.plugin.super.users`,
`ranger.plugin.super.groups` and `ranger.plugin.service.admins`; `isSuperUser`
ORs the download-derived side against the plugin-side, and a super-user
short-circuits **before any policy** with `policyPriority = Integer.MAX_VALUE`.

egret's own DTO has no `serviceConfig` field, so `Unmodelled::ServiceConfig` is
unreachable on egret's downloads — which makes it look free to narrow. It is
not: it is the only guard for a captured or imported download, which is what the
differential corpus and `examples/live_gate.rs` consume.

## Decisions

### D1 — `explain` evaluates the served artifact, one verdict per profile

For each service that serves the target's catalog, the evaluator runs over
`enabled_policies_for_service` — the same assembler the download route uses, so
"what explain evaluated" and "what the engine received" cannot drift.

The response carries one verdict per profile, each naming the profile and its
engine type. "No profile serves this catalog" is its own outcome, distinct from
`Denied`: nothing is enforcing, which is a different fact from a refusal.

There is no honest single aggregate over `{Allowed, Denied, Undecidable}` × N
profiles, so the page's summary badge is **removed**, not recoloured.

### D2 — roles and groups are sent empty, and the UI says why

The evaluator receives `roles: {}` and `groups: {}`, because that is what the
engine sees. A policy that grants through a role or a group matches nobody
there, and reporting otherwise would answer a question the operator did not ask.

This also keeps SP4c's rule — a caller may not assert a role the subject does
not hold — by the strongest available means: no role is sent at all. The
`groups` request field stays on the wire for compatibility but is not forwarded
to the evaluator; the page states plainly that a role- or group-granted policy
matches nobody on these engines, and why.

### D3 — narrow the `tagPolicies` refusal to tier (c), and name the cause there

Tier (a) and tier (b) become decidable. A non-empty block with an empty resolved
tag set is likewise decidable. Tier (c) stays `Undecidable`, but with a cause
that says what is missing and how many policies could decide — not a generic
envelope refusal.

`Unmodelled::TagPolicies` currently carries no payload and cannot express the
distinction; it gains one.

**`ServiceConfig` is NOT narrowed in this package.** The two refusals have
opposite risk profiles: an empty `tagPolicies` is a measured no-op, while an
unmodelled `serviceConfig` fails **open** through a super-user allow that
nothing downstream can override.

### D4 — tag relevance is gated on the service-def, never on the engine type

Whether tag policies can decide is determined by whether the def being evaluated
declares a `RangerTagEnricher` in `contextEnrichers` — read from the download.
egret's `trino` def declares one; its `starrocks` def has `contextEnrichers: []`,
which is why two measured StarRocks FEs never fetched tags at all.

Gating on `EngineType::StarRocks` would be silently wrong the day that one field
changes. `crates/egret-ranger-eval/src/servicedef.rs` does not parse
`contextEnrichers` today; it must.

### D5 — a policy is mapped back to a model object where one exists, and named where none does

A decision attributed to `perm-grant-{uuid}` is reported as that Grant; likewise
rules and clearances. The mapping is **partial by design**:

- a per-column rule emits N policies for one `perm.rules` row, so the suffix is
  the only locator for which expansion decided;
- a clearance names a `tags.id`, a third object class, and `tags.key` is not
  `tags.name`;
- a row can be deleted between compile and explain;
- egret's seeded infrastructure and operator-written raw JSON have no model
  object at all, and Ranger's super-user short-circuit reports
  `policy_id: -1, policy_name: "superuser"`, which is not a policy.

A policy with no model object is reported by its Ranger name and id and labelled
as outside the permission model. **It is never dropped from the reasons** —
dropping it would recreate, inside the new mechanism, the exact blindness this
change exists to remove.

### D6 — the model evaluator becomes internal, not removed

`egret_perm::evaluate` has exactly one production caller: the explain handler.
But `PUBLIC_GROUP` in the same module is genuine compiler code, and three tests
(`explain_and_the_compiled_policy_agree_*`) use `evaluate` as an independent
oracle against the real compiled body — the only cross-check that would catch a
compiler bug.

So `evaluate` stays as that oracle, is dropped from the public re-export, and the
three tests are renamed to say what they now compare. One **new** agreement test
asserts that the artifact-based answer and the model oracle agree on the happy
path: this redesign removes a drift alarm, and that test puts it back.

### D7 — wire DTOs are hand-written; the evaluator crate stays dependency-pure

`egret-ranger-eval` depends only on `serde`/`serde_json` and keeps it that way.
Its `Decision`, `UndecidableCause`, `DenyReason`, mask and row-filter results are
mirrored as utoipa DTOs in the route crate — roughly twenty variants. Adding
`utoipa` to a pure evaluator to save that work would trade a stated discipline
for a shortcut.

### D8 — a `write` explain answers per access type

Ranger has no "write". `write` is two access types on Trino and three on
StarRocks, and `AccessRequest.access_type` is one string, so a write explain is
several evaluations. Each is reported separately rather than collapsed: an
operator who can insert but not delete needs to see exactly that.

### D9 — the honest-limits paragraph this change disproves is deleted

Paragraph 1 ("cannot see hand-written Ranger policy") is dissolved and must be
removed, not reworded. The groups paragraph is replaced by D2's statement. A
limit that no longer holds is worse than no limit: it teaches a reader to
discount the ones that do.

### D10 — `clearance_required` gets the UI entry it never had

Pre-existing defect, verified: `REASON_META`
(`frontend/src/routes/governance/ExplainAccess.tsx`) has six kinds and
`clearance_required` is not among them, though the backend has emitted it since
SP4c. Today a clearance denial renders as the raw string with "Unrecognized
reason class.", and `reason.tag` and `reason.required_roles` — the gating tag and
the roles that would clear it — are rendered nowhere. Fixed here, since the map
is being rewritten regardless.

## Scope

Backend: route `explain` through the evaluator per profile, the tier narrowing,
`contextEnrichers` parsing, the name→model mapping, the wire DTOs, the new
two-profile fixture, and the test rewrites SP4d-1's survey enumerated.

Frontend: the per-profile result, the `Undecidable` class with named causes, raw
policies named without links, the honest-limits rewrite, and D10.

## Non-goals

- **Tag-policy evaluation** — SP4d-2b. It needs a two-phase engine, a tag input,
  a second service-def, three policy-type-dependent comparators, and it has an
  unmeasured prerequisite: which `MatchType` `RangerTagEnricher` produces for a
  column request against a table-level tag. Every live tag assignment is
  column-level, so that path is not exercised by current data and is easy to get
  wrong unnoticed. SP4d-2b opens by measuring it, and by capturing a real
  tag-policy fixture from the plugin's own cache rather than inventing one.
- **Narrowing `ServiceConfig`** — fail-open, see D3.
- **Migration and the infrastructure prefix** — SP4d-3.
- Changing what the compiler emits. The ambiguous same-priority row filters SP4d-1
  found are an SP4a/SP4c defect, recorded at the compiler site.
