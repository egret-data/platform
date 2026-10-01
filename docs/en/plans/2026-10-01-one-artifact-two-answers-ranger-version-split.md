# One artifact, two answers: Ranger 2.5.0 and 2.8.0 enforce a tag deny differently

Date: 2026-10-01
Vietnamese reference:
[`docs/vi/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md`](../../vi/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md).
This English version is canonical.
Linked from
[`tests/ranger/README.md`](../../../tests/ranger/README.md) under "T5 — the two
plugin versions DISAGREE".
Measured by: the SP4d-2b differential gate (`crates/egret-ranger-eval/tests/differential.rs`
plus `tests/ranger/src/test/java/EgretCaseRunner.java`), against real
`RangerPolicyEngineImpl` instances from `ranger-plugins-common` 2.5.0 and 2.8.0.
Reproduced independently, row for row including policy ids, by the Task 5 review.

## The finding, in one sentence

**egret serves ONE policy download to both engines, and for a tag DENY plus a
strictly higher-priority resource ALLOW the two engines return OPPOSITE
answers** — Trino (Ranger plugin 2.5.0) denies, StarRocks (2.8.0) allows — and
nothing in egret tells a steward that.

This is not a bug in egret's compiler, and not a bug in
`egret-ranger-eval`: both sides of the download are correct and identical. It is
a behavioural difference between the two Apache Ranger plugin versions egret's
own engines run.

## What was measured

One minimal download, sent byte-identically to both engines through the
differential harness. Tag ACCESS policy (id 900) denying `select` to `bob` at
`policyPriority: 0`; one resource ACCESS policy (id 1) allowing `select` to
`bob`; request `hive.sales` for `bob`; the tag `pii` attached at `SELF`.

| resource allow | deny items on the allower | Ranger 2.5.0 (Trino 470) | Ranger 2.8.0 (StarRocks 4.1) |
|---|---|---|---|
| `policyPriority: 1` | none | **Denied**, `policyId: 900` (the TAG policy) | **Allowed**, `policyId: 1` (the resource policy) |
| `policyPriority: 2` | none | **Denied**, `policyId: 900` | **Allowed**, `policyId: 1` |
| `policyPriority: 1` | present, matching nobody | Denied, `policyId: 1` | Denied, `policyId: 1` |
| `policyPriority: 0` | none | Denied, `policyId: 900` | Denied, `policyId: 900` |
| (no `tagPolicies` block) | none | Allowed, `policyId: 1` | Allowed, `policyId: 1` |

Rows 3, 4 and 5 agree across versions. Only the strictly-higher-priority,
non-deny-capable allower diverges — which is what makes this a specific
divergence rather than a general "the versions differ" observation.

## The part that makes it worse than a version difference

`RangerPolicyEngineImpl.evaluatePoliciesNoAudit` logs, at **offset 224 in BOTH
jars**:

> `access denied by a tag policy. Higher priority resource policies will be
> evaluated to check for allow`

2.5.0 therefore *states* the 2.8.0 behaviour in its own log output and does not
deliver it. Anyone who reads that line — or reads Ranger's documentation, or
measures on StarRocks and generalises — will conclude that a higher-priority
override grant reopens a tag deny. On Trino it does not.

The mechanism was **not** established. The obvious candidate,
`RangerDefaultPolicyEvaluator.evaluatePolicyItems`' per-access-type
`accessTypeResults` map, is ruled out: that method is near-identical between the
two jars in the relevant region (the only deltas in the whole method are the
`ignoreDescendantDeny` arms and the `getCompositeAccessResult` signature). So
there is a second unmodelled mechanism in `RangerPolicyEngineImpl`, whose
normalised diff between the jars is ~210 lines — `RangerReadWriteLock`, inline
policies (`evaluateInlinePolicy`, `RangerInlinePolicyEvaluator`),
`RangerAccessRequest.ignoreDescendantDeny()`, and
`getServiceDefHelper().getAllAccessTypes()` replacing
`getServiceDef().getAccessTypes()`.

## What it means for the product

1. **A clearance (tag deny) plus an override grant is not portable between
   engines.** The exact governance gesture a steward reaches for — "deny this
   tag everywhere, except grant this one table back at a higher priority" —
   enforces on StarRocks and does not enforce on Trino. The same rows in
   `policy.authz_policies` produce different access.
2. **Measuring on one engine and generalising is unsafe for this shape.** Every
   live measurement in the SP4d series so far has been taken on one engine at a
   time. This is the first shape found where that is not enough.
3. **egret has no UI or docs surface that says so.** The only thing that says it
   today is `egret-ranger-eval`, which REFUSES the shape on a 2.5.0 plugin —
   `UndecidableCause::TagDenyReopenedDiffersByPluginVersion` — and whose
   `explain` page therefore shows a steward "this engine runs ranger 2.5.0, which
   keeps the tag deny here where 2.8.0 would reopen it". That is a correct
   refusal in one tool, not a product answer.

## What was done about it, and what was not

**Done (SP4d-2b Task 5):**

* `egret-ranger-eval` reproduces the 2.8.0 arm and REFUSES on 2.5.0 and on an
  unstated plugin version, rather than reporting one engine's answer for the
  other. The refusal carries both policy ids and the plugin version.
* `explain` declares `EngineType::plugin_version()` so the refusal is scoped to
  the engine that actually diverges, and the page's sentence names the version.
* The differential corpus generates the shape, so the refusal cannot silently
  stop firing. Rate at the recorded seed: 1 of 1263 random tag-carrying cases.
* `tests/ranger/README.md` carries the disassembly, the offsets and this table.

**Not done, and deliberately out of scope for a test-harness task:**

* **Nobody has decided what egret should DO.** Options, none chosen: refuse the
  compile (reject an override grant over a tag deny on a Trino profile); warn at
  compile time; compile differently per engine; or accept the divergence and
  document it for stewards. This needs a product decision, not a crate change.
* **No steward-facing surface warns at authoring time.** The refusal appears in
  `explain`, which a steward reaches only if they go looking.
* **The 2.5.0 arm is not modelled**, so `explain` on Trino refuses rather than
  answering. Modelling it is follow-up item 2 in `tests/ranger/README.md`, and it
  requires the `accessTypeResults` audit named there — the resource path models a
  different mechanism for the same map and is green over 4000 corpus cases, which
  is an observation and not a proof.

## How to reproduce

```bash
cd tests/ranger
# cases.json: the five-row table above, as EgretCaseRunner protocol
mvn -o test -Dranger.version=2.5.0 -Degret.cases=/tmp/cases.json -Degret.verdicts=/tmp/v250.json
mvn -o test -Dranger.version=2.8.0 -Degret.cases=/tmp/cases.json -Degret.verdicts=/tmp/v280.json
# the verdicts differ on rows 1 and 2, in isAllowed AND in policyId
```

Never pass `-q`: it suppresses the per-class surefire summary, and empty output
is not a pass.
