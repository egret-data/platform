# SP4d — A Ranger evaluator egret can be held to

**Date:** 2026-09-28
**Status:** design
**Predecessors:** SP4a (permission model → Ranger compiler), SP4b (permission UI), SP4c (classification clearance)

## Why this exists

`explain` answers "may this subject do this?" by evaluating egret's own
permission model — `perm.grants`, `perm.rules`, `perm.clearances`. It never
reads `policy.authz_policies`. Every hand-written policy, every seeded
infrastructure policy, and the compiler's own output *as stored* are invisible
to it.

That blindness has already produced a defect in production code, not merely in
theory: SP4c measured `explain` reporting a decision the engine did not make,
because the model said one thing and the compiled artifact did another.

The original framing of SP4d was "migrate hand-written policies". A survey
falsified that framing:

- egret's own infrastructure policies (`egret-admin - …`,
  `egret-delegator-impersonate`, `egret-query-timeout`) are **not** migration
  candidates. They are plumbing, and they are currently editable and deletable
  through the raw-JSON editor with no guardrail.
- Some hand-written policies **cannot** be migrated, because `Scope` cannot
  express the resource kinds they name (`trinouser`, `systemproperty`,
  `queryid`) and `Grant` cannot express `delegateAdmin`, `isDenyAllElse`,
  override priority, `isExcludes`, `additionalResources`, or multiple items
  per bucket.
- Migrating anything safely requires previewing its effect, and no preview is
  trustworthy while `explain` is blind to the rows it would be migrating.

So SP4d is first an evaluator, and only then a migration.

## Measured premises

Everything in this section was measured on 2026-09-28, not recalled. Anything
not measured is labelled so.

### egret *is* the Ranger Admin

There is no Apache Ranger server anywhere. The Trino plugin downloads policy
directly from egret:

```text
ranger.plugin.trino.policy.rest.url  = http://egret.egret-system.svc.cluster.local:8080
ranger.plugin.trino.policy.source.impl = org.apache.ranger.admin.client.RangerAdminRESTClient
ranger.plugin.trino.policy.pollIntervalMs = 30000
```

(read from `/etc/trino/ranger-trino-security.xml` in the running
`sp3c2-trino-live` coordinator). egret serves the Admin protocol itself from
`crates/egret-policy/src/ranger/routes.rs:15-31`.

**Consequence:** the thing to reproduce is not Ranger Admin. It is
`RangerPolicyEngineImpl` from the plugin jar bundled inside the engine image,
plus the engine-specific wrapper that builds requests for it.

### Three Ranger versions are in play, and none match

| Where | Version | How measured |
|---|---|---|
| `tests/ranger/pom.xml` (the existing harness) | **2.4.0** | file contents |
| `trinodb/trino:470` — what egret actually provisions | **2.5.0** | `ls /usr/lib/trino/plugin/apache-ranger/` in the running coordinator |
| `trinodb/trino:latest` (483) | 2.8.0 | image inspection |

`crates/egret-engine-mgmt/src/engines.rs:47` pins `DEFAULT_IMAGE_VERSION = "470"`.
The harness therefore currently certifies against a policy engine that is **not
the one enforcing**, and the algorithm description gathered for this spec was
read from 2.8.0 sources — a third version again.

`ranger-plugins-common` 2.4.0, 2.5.0, 2.7.0 and 2.8.0 are all already present in
`~/.m2/repository/org/apache/ranger/ranger-plugins-common/`, so retargeting the
harness to 2.5.0 needs no network.

**NOT MEASURED:** the Ranger version inside `starrocks/fe-ubuntu:4.1-latest`
(the provisioned StarRocks FE image). Only `starrocks/allin1-ubuntu:latest` was
inspected, which carries 2.8.0. Task 1 of SP4d-1 must measure the provisioned
image; the spec does not assume they agree.

### The oracle already exists

`tests/ranger/` is a working differential harness against the real engine:

- `tests/ranger/pom.xml` → `org.apache.ranger:ranger-plugins-common`
- `tests/ranger/src/test/java/EgretRangerEnforcementTest.java` — builds a
  `RangerPolicyEngineImpl` and calls `evaluatePolicies(...)` for ACCESS,
  DATAMASK and ROWFILTER
- `tests/ranger/fixtures/service-policies.json` — a real captured download
- `crates/egret-policy/tests/ranger_engine_gate.rs` — Rust driver, gated on
  `EGRET_TEST_RANGER_E2E=1`
- `tests/ranger/README.md` records it green: 4 tests, JDK 21, 2026-06-17

It is not built from scratch; it is retargeted and widened.

**Defect found while reading it:** `tests/ranger/README.md` documents the run
command as `cargo test -p egret-ranger ranger_engine_gate`. No crate named
`egret-ranger` exists; the gate lives in `crates/egret-policy/tests/`. Anyone
following the README gets "package not found" and may conclude the gate is
absent. Fix in SP4d-1.

### The semantics most likely to be got wrong

Two independent sources agree, which is why this is stated with confidence: the
2.8.0 source reading (`RangerDefaultPolicyResourceMatcher.getMatchType`,
`RangerDefaultPolicyEvaluator.updateAccessResult`) and a **live Trino
measurement** recorded at `crates/egret-perm-compile/src/grants.rs:227-254` and
`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3 case T1.

A policy naming fewer hierarchy levels than the request matches as `ANCESTOR`,
and `updateAccessResult` **refuses to record an allow for an ANCESTOR match**.
Live, a grant compiled to `{catalog, schema}` produced
`Access Denied: Cannot select from columns [v, id] in table or view sibling`;
the same grant with `column: "*"` returned rows.

**A policy naming `catalog` alone does not govern a `catalog/schema/table/column`
select.** A naive "ancestor covers descendants" evaluator is wrong in the
fail-open direction. This rule is the single highest-risk item in the port.

Other semantics to encode, from the 2.8.0 reading (each to be confirmed against
2.5.0 by the differential gate, not assumed):

- Bucket order is `denyPolicyItems → denyExceptions → allowPolicyItems →
  allowExceptions`. Exceptions **veto a match**; they are not competing items,
  and a cancelled deny does **not** become an allow — it falls through.
- Priority interaction is asymmetric: for ACCESS an earlier tag allow is final
  only on strictly `>`, for mask/row-filter on `>=`.
- `isExcludes` is not plain negation: `allValuesRequested && !isMatchAny` is a
  carve-out that does not flip.
- `impliedGrants` (e.g. Trino's `all`) expands **one round only**; Ranger
  explicitly does not handle multi-level implied grants.
- Mask and row-filter policies have **no deny bucket and no exceptions**.
- Principal order is users → groups (incl. `public`) → roles → owner,
  short-circuiting on first hit. `{USER}` matches any user. `public` matches
  everyone regardless of group membership.
- Undetermined at the end means **deny**, unless the plugin enables fallback to
  the engine's native authorizer.

## Decisions

### D1 — `explain` evaluates the artifact the engine loads, not the model

The evaluator runs over the full served policy set for a service — compiler
output, seeded infrastructure, and hand-written rows alike — because that is
exactly what the plugin downloads. Readability is preserved by mapping policy
names back to model objects: a decision attributed to `perm-grant-42` is
reported as Grant #42.

This dissolves a defect class rather than adding a second opinion. Today the
model and the compiled artifact can disagree and `explain` reports the model;
SP4c measured that happening. After D1 there is one answer, and it is the
engine's.

The canonical view is `enabled_policies_for_service`
(`crates/egret-policy/src/store.rs:1188`) — the same assembler the download
route uses, which already injects `id`/`version`/`isEnabled` and embeds
`tagPolicies`. The evaluator must consume that, not `list_policies`, so that
"what explain evaluated" and "what the engine received" cannot drift.

### D2 — The result is three-valued

`Allowed | Denied | Undecidable { why }`.

The failure this whole design guards against is confident wrongness. Every
input the real engine consults that egret cannot know produces `Undecidable`
with a named cause, never a guess:

| Input | Why egret cannot know it |
|---|---|
| `ranger.plugin.super.users` | **corrected 2026-09-29:** the download CAN carry it. `RangerPolicyEngineImpl.<init>` builds its `ServiceConfig` from `ServicePolicies.getServiceConfig()` (javap offsets 125-136, identical at 2.5.0 and 2.8.0) and `isSuperUser` ORs that against the plugin config, so either side alone decides. Egret cannot know the PLUGIN side; it can read the download side but does not model it, and refuses any envelope carrying `serviceConfig` (`Unmodelled::ServiceConfig` → `EnvelopeConstruct`). That refusal, not the absence of the key, is the safety property |
| `{OWNER}` | resource ownership is supplied per request by the engine |
| user→group mapping | egret runs no usersync; measured empty on every provisioned engine |
| roles | the plugin fetches `/service/roles/download/<service>`, a route egret does not serve |
| `validitySchedules` | time-dependent; egret emits none but a hand-written policy may carry one |
| `policyConditions` / custom conditions | service-def-declared, pluggable |
| expression-bearing mask values / row-filter exprs | `RangerRequestExprResolver` resolves at request time |
| fallback authorizer | the engine's native decision when Ranger is undetermined |
| policy staleness | 30s poll + 304 caching; the plugin may be up to a poll behind |

`Undecidable` is a first-class outcome in the API and the UI, not an error.

### D3 — Two gates, neither substituting for the other

There are two distinct gaps, and the cheap instrument only closes one.

**Gap A — Rust evaluator vs `RangerPolicyEngineImpl`.** Closed by the
differential harness at 2.5.0, driven by generated cases rather than the
current four hand-written ones. Cheap, deterministic, runs on every change,
and can cover combinations of excludes / priority / exceptions / implied grants
that no live run could.

**Gap B — the Java engine as the harness configures it vs the plugin as Trino
configures and calls it.** Request construction, access-type naming, resource
keys and column granularity all live here. The harness cannot see this gap; the
T1 measurement above is proof it is real and non-obvious.

Gap B is closed by a small live gate on a real engine. It covers few cases by
design — its job is to catch a class the harness is structurally blind to, not
to achieve coverage.

Both are required before merge. This is the explicit lesson of SP4a, where
three Criticals passed roughly 200 green unit tests because the tests asserted
on the compiler's output *shape* rather than on where it landed.

### D4 — Refuse rather than approximate

When the evaluator meets a construct it does not model, it returns
`Undecidable`, naming the construct. It never falls back to a partial match.

It must also assert its preconditions and refuse if violated: no
`securityZones` (egret's download envelope has no field to carry one, so a zone
appearing at all means an assumption has broken), no `validitySchedules`, no
custom conditions, no expression-bearing mask values.

### D5 — Reuse the existing pieces; fix them where they are wrong

- **Hierarchy walking** already exists at
  `crates/egret-policy/src/default_policies.rs:20-90`
  (`ResourceDef`, `parse_resources`, `is_hierarchy_leaf`, `hierarchy_chain`),
  currently private. Lift, do not reimplement.
- **Service-defs** are embedded and seeded
  (`crates/egret-policy/src/service_def.rs`, `service-defs/{trino,starrocks,tag}.json`),
  so the resource hierarchy, matcher options and `impliedGrants` are all
  available without network.
- **Glob matching** exists at `crates/egret-perm/src/eval.rs:355-395` with real
  `*`/`?` backtracking. It is **case-sensitive**, while both service-defs
  declare `matcherOptions: {ignoreCase: true}`. Case folding must be driven by
  the service-def, not hardcoded.
- **catalog_id → service_id → catalog name** bridges exist at
  `crates/egret-policy/src/store.rs:671-749,885`. Ranger resources are keyed by
  catalog *name*; `Scope` carries a UUID.

### D6 — The evaluator is engine-parameterised, not Ranger-generic

Trino and StarRocks differ in service-def (`column` is a valid leaf in
`starrocks.json` and not in `trino.json`; access-type vocabularies are
disjoint) **and** in tag support (StarRocks' plugin has no tag policies at all,
which is why egret expands tag rules into per-column resource policies). The
same policy JSON therefore evaluates differently on the two engines. Engine
type and plugin version are inputs to the evaluator, not constants.

### D7 — Migration is classification, and most of it is not migration

Once `explain` is trustworthy, each hand-written policy falls into one of three
classes:

1. **Expressible** as a Grant or Rule → offer a migration, previewed by the
   evaluator before and after.
2. **Infrastructure** — egret's own plumbing → mark with a fourth compiler-owned
   prefix so `is_compiler_owned_policy_name`
   (`crates/egret-policy/src/store.rs:109-152`) protects it, reusing an
   existing tested guard. This closes a live gap: today an operator can
   hand-edit `egret-delegator-impersonate` and break delegation.
3. **Inexpressible** — names `trinouser`/`systemproperty`/`queryid`, or uses
   `delegateAdmin`, `isDenyAllElse`, override priority, `isExcludes`,
   `additionalResources`, or multiple items per bucket → stays raw, but is now
   visible in `explain` instead of silently governing.

Class 3 is not a failure. The editor exists because the permission model is
deliberately narrower than Ranger; the goal is that nothing governs invisibly,
not that everything becomes a Grant.

## Scope

Three sequential packages, each independently mergeable with its own gate.

**SP4d-1 — the evaluator and its oracle.** Backend only, no UI, no `explain`
change. Measures the provisioned StarRocks plugin version; retargets
`tests/ranger/` to the version that actually enforces; builds the Rust
evaluator with a three-valued result; drives the differential harness from
generated cases; runs the live Trino gate. Fixes the README crate name.

**SP4d-2 — `explain` tells the truth.** Routes `explain` through the evaluator
over the served artifact (D1), adds the reason variants carrying `policy_id`
and the `Undecidable` class, maps compiler-owned policy names back to model
objects, and surfaces all of it in the UI.

**SP4d-3 — classification and migration.** The fourth prefix for infrastructure
(D7.2), the classifier, and the migration flow with before/after preview.

## Non-goals

- Reimplementing Ranger Admin. egret already serves the protocol.
- Security zones. egret has none and its download envelope cannot carry one.
- Evaluating audit behaviour. Audit flags change how many policies are
  evaluated but not the verdict.
- A general Ranger policy editor. The raw-JSON editor stays as it is.
- Predicting the engine's native fallback authorizer.

## Open questions for SP4d-1 Task 1

- Which `ranger-plugins-common` does `starrocks/fe-ubuntu:4.1-latest` bundle?
- Do provisioned engines set `ranger.plugin.super.users`? (Not set in
  `deploy/starrocks/ranger/ranger-starrocks-security.xml:28-34`; the k8s path
  is unmeasured.) Note this asks only about the PLUGIN side — the policy
  download's `serviceConfig` can carry the same key, and the answer there is
  "the evaluator refuses the whole envelope", not "the key cannot be there".
  See the D2 row above.
- Does 2.5.0 share 2.8.0's bucket ordering and priority asymmetry verbatim?
  The differential harness answers this rather than a source reading.
