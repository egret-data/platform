# Ranger policy-engine enforcement gate

Proves Egret's ServicePolicies enforce in the real Apache Ranger policy engine
(the same RangerPolicyEngineImpl the Trino plugin runs).

Requires: JDK 11+, Maven, network (pulls
`org.apache.ranger:ranger-plugins-common:2.5.0` from Maven Central, unless
already cached under `~/.m2`).

Two things run here, and they are deliberately different claims.

**1. The four hand-written anchor tests** (`EgretRangerEnforcementTest`), each
asserting a known-good expected value someone worked out:

```bash
EGRET_TEST_RANGER_E2E=1 cargo test -p egret-policy --test ranger_engine_gate
# or directly (both versions):
cd tests/ranger && mvn -q test
cd tests/ranger && mvn -q test -Dranger.version=2.8.0
```

**2. The differential gate** (`EgretCaseRunner` + the Rust generator), which
runs a generated corpus through the SAME real engine and through
`egret-ranger-eval`, and compares:

```bash
EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential -- --nocapture
```

The two are not interchangeable. The generated corpus asserts only
**agreement**, which is the weaker claim — two implementations can agree and
both be wrong — so the four anchors stay, and every Maven run executes them
alongside whatever cases are passed in. `EgretCaseRunner` is skipped (JUnit
`Assume`) when `-Degret.cases` is absent, so a plain `mvn test` is the anchors
alone.

The `ranger-plugins-common` version is a Maven property, overridable without
editing the POM:

```bash
cd tests/ranger && mvn -q test -Dranger.version=2.8.0
```

Optional — regenerate fixtures/service-policies.json from a running Egret:

```bash
curl -s -H "Authorization: Bearer devtoken" \
  "http://localhost:8080/service/plugins/policies/download/trino_prod" \
  > tests/ranger/fixtures/service-policies.json
```

The anchors assert: alice select allowed; bob denied; column email →
MASK_HASH; row filter "country = 'US'". (Default `ranger-plugins-common`:
**2.5.0**; the differential gate runs **both** 2.5.0 and 2.8.0 every time.)

## Why 2.5.0 is the default

The harness's default (`tests/ranger/pom.xml` `<ranger.version>`) is pinned to
**2.5.0** because that is the `ranger-plugins-common` version the Trino
coordinator egret actually provisions loads at runtime
(`trinodb/trino:470`), measured directly from a live pod's plugin directory —
see `docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`. It is not
2.4.0 (the harness's old, unverified pin) and not 2.8.0 (a figure earlier
misread from `trinodb/trino:latest`, an image egret does not provision).

That same result doc also measured StarRocks's `fe-ubuntu:4.1` image, which
loads `ranger-plugins-common-2.8.0` — a different version from Trino's. The
harness is runnable at 2.8.0 via `-Dranger.version=2.8.0` for that reason,
but 2.5.0 stays the default because Trino is the primary/only engine this
evaluator package targets per the SP4d-1 brief.

Both versions were run clean (`mvn clean test`) against the unmodified
harness and produced **`Tests run: 5, Failures: 0, Errors: 0, Skipped: 1`**
with no assertion differences between them — Ranger's behaviour for these four
cases (allow, deny, mask, row filter) has not changed between 2.5.0 and 2.8.0.
The fifth is `EgretCaseRunner`, `Assume`d out when no `-Degret.cases` is
given. The anchors themselves are the per-class line
`Tests run: 4, Failures: 0, Errors: 0, Skipped: 0 ... in
EgretRangerEnforcementTest`, which is what
`crates/egret-policy/tests/ranger_engine_gate.rs` asserts on — the totals line
alone cannot tell "four anchors passed" from "no anchor ran".

## The differential gate

`crates/egret-ranger-eval/tests/differential.rs` generates a seeded corpus,
writes it as `cases.json`, has `EgretCaseRunner` answer every case with a real
`RangerPolicyEngineImpl`, reads `verdicts.json` back and asserts the Rust
`Decision` matches. Paths travel as `-Degret.cases` / `-Degret.verdicts`
system properties, so nothing is bound to a working directory.

Recorded run, re-recorded 2026-10-01 (JDK 21.0.11 / Maven 3.9.11) with the tag
phase generated for the first time:

| | |
|---|---|
| Seed | `0x54d000007` (`EGRET_DIFF_SEED`, hex) |
| Cases | **2000 per engine — 4000 total** (`EGRET_DIFF_CASES`) |
| Versions | **2.5.0** against the Trino service-def, **2.8.0** against the StarRocks one |
| Cases carrying a `tagPolicies` block | **1908 of 4000** |
| Compared and agreeing | **3468** (86.70% of all cases) |
| `Undecidable` outside the six declared refusals | **0** (asserted) |
| Deliberate shapes | 539 eval-order duels (250 composed), 193 candidacy duels, 176 restore duels, 141 version-split duels |
| Reported ACCESS `policyId` fidelity | **1465 of 1602 = 91.45%**, the rest in the two declared divergences (47 + 90) |
| Reported ACCESS `policyId` divergences outside the declared list | **0** (asserted) |
| Wall time | **~79s** including seven Maven runs |

⚠ **This table is the ONLY recorded set of counts in this file.** It carried two
conflicting sets until the whole-branch review (1906 vs 1755 tag-carrying, 584 vs
634 duels) with a third in `differential.rs`, and plan open item 1 is decided on
these rates. Every later section now points here instead of restating. The numbers
moved from the fix-round-2 report because the review added the
`version_split_duel` draw, which takes cases the other draws would have had.

### The six measured refusal rates, over FOUR populations

⚠ **Fix round 1 corrected these.** They were printed over two populations —
"resource path" and "every tag-carrying case" — and the second denominator was
30% cases CONSTRUCTED to hit the refusal being measured. Re-measured over the
random half, the printed figures were wrong in BOTH directions, so it was not a
bias a reader could correct by a factor. Every rate below is over exactly one of
four populations.

⚠ **The whole-branch review deleted the sentence that used to close that
paragraph** — *"and `composed, random` is the column a real download is in"*. That
is a coverage claim stronger than the data: a real download CAN carry an override
policy (`AccessPolicyInput.isOverride` on `POST /api/control-plane/policies` emits
`policyPriority: 1`) and this column cannot, because `policy`'s `level_priority`
holds that shape out. `composed, random` is the column a real download **with no
override ACCESS policy** is in. The hold-out now has its own printed counter, the
`RANDOM composed ACCESS case with a resource policy above priority 0` axis, which
reads **0** — so the gate STATES the hole instead of leaving it to be derived from
two other columns.

Recorded corpus, seed `0x54d000007`, 2000 cases per engine:

| refusal | resource, random (1803) | resource, duel (289) | **composed, random (1148)** | composed, duel (760) |
|---|---|---|---|---|
| `EvalOrderAmbiguous` | 104 = **5.77%** | 0 | **25 = 2.18%** | 96 = 12.63% |
| `ResourceCandidacyNotReproducible` | 0 | 0 | **0 = 0%** | 184 = 24.21% |
| `TagSubResultSplitByResourcePolicy` | 0 | 0 | **17 = 1.48%** | 0 |
| `TagPriorityArbitrationNotModelled` | 0 | 0 | **23 = 2.00%** | 0 |
| `TagDenyReopenedDiffersByPluginVersion` | 0 | 0 | **0** | 73 = 9.61% |
| `AccessPriorityCompositeNotModelled` | 0 | 0 | **0** | 10 = 1.32% |

Plus the four deliberate shapes against their own denominators, which is the
only way their numbers mean anything:

* **candidacy duels: 184 of 193 REFUSE (95.3%)**, and the refusal fires **zero**
  times outside them. Two numbers, not one: a download that does not carry the
  four-way conjunction never reaches this refusal, and one that does reaches it
  almost always. `explain` over a live plugin's download is the population that
  carries it — `scope_to_resources`' `Mask` arm does not fill unnamed levels with
  `*`, and an ordinary table-level grant from a download egret did not author has
  the same shape — so 95% is the rate to plan against, not 0%.
* **composed eval-order duels: 144 of 250 agree** with the engine and the rest
  refuse; **none disagree**. One in four is built with a tag ALLOW instead of a
  deny as the falsifier: under an allow offset 505's `>` lets the same-priority
  rival run after the allow, so the deny wins whichever ran first and the crate
  must NOT refuse — and does not.
* **restore duels: 176 of 176 AGREE**, which is the point of that shape: it must
  ANSWER, and reverting the trie-excluded-view skip turns the agreement into a
  disagreement. Two floors, one that it is generated and one that it still agrees.
* **version-split duels: 79 generated at 2.5.0 of which 73 REFUSE (92.4%), and 62
  generated at 2.8.0 of which 0 refuse.** Both halves are asserted, in opposite
  directions, by `check_version_split_is_asymmetric`.

⚠ **`TagDenyReopenedDiffersByPluginVersion` used to have NO corpus coverage, and
three places in the tree said the corpus guarded it.** `differential.rs`'s own doc
claimed the 2.5.0/2.8.0 asymmetry *"is itself the assertion: a run where the two
halves' rates were equal would mean the version input had stopped being read"* —
and **no `assert!` anywhere compared the halves.** They were equal, at 0 == 0,
which is that sentence's own stated failure condition; the only bound was a
ceiling of 10‰ over a measured 0‰ that nothing could exceed. Nothing in the tree
resisted someone deleting the `plugin_version` read with every gate green.

It is real now. `version_split_duel` generates row 1 of T5's table — a tag DENY
under a strictly higher-priority resource allow with NO deny items of its own, the
one row the versions answer differently — and
`check_version_split_is_asymmetric` asserts three things that each fail for a
different reason: that the duel is still GENERATED (or 2 and 3 are vacuous, which
is how the last version of this claim passed), that essentially ALL of them refuse
at 2.5.0, and that NONE of them refuse at 2.8.0. The unit test
`a_tag_deny_is_reopened_by_a_strictly_higher_priority_resource_allow_on_2_8_0`
still pins the shape JVM-free.

The two `EvalOrderAmbiguous` rates are genuinely different questions. Task 2
established the refusal cannot arise in the tag phase alone (both tag comparators
are fully reproduced). Task 3 found it arises once the phases compose on ACCESS:
`policyPriority` is live, so under a tag deny an allowing resource policy raises
offset 469's left-hand side and shuts out the rest of its own band, and
`getEvalOrder()` then picks the winner.

### Two things the gate asserts that it did not before (fix round 1)

**A CEILING on every refusal.** This file had floors and no ceiling, and the
review proved the gap by injecting a 42% over-refusal: `compared` fell 3490 →
2314 and the gate stayed **GREEN, exit 0**. Every floor here is a floor, and the
thing an over-refusal shrinks is the denominator. Two assertions close it, and
they answer different questions — `DECLINED_CEILINGS` bounds each kind at ~2× its
measured rate (a single refusal widening is what a narrowing "fix" looks like from
outside), and `compared * 4 >= cases * 3` catches a refusal spread thinly across
every kind, which no per-kind ceiling can see. Measured compared fraction: 86.9%.

**The reported ACCESS `policyId`.** `Outcome::Access` compares only `isAllowed`,
so until fix round 1 **a crate that reported the wrong policy id agreed with
Ranger on all 4000 cases** — and the id is precisely what distinguishes the T4 and
T5 rows below from one another, and what `explain` puts in front of a steward. It
is now its own reported axis:

| | |
|---|---|
| ACCESS cases where both sides agreed on allow/deny and named an id | **1602** |
| agreeing on the id | **1465 = 91.45%** |
| declared divergence: same `(policyPriority, hasDeny)` band (`getEvalOrder()`) | 47 |
| declared divergence: tag deny restored over a resource allow | 90 |
| **NOT on the declared list** | **0** (asserted) |

The two declared shapes are an ALLOW-LIST, not a relaxation: each is a structural
test on the two policies the two sides named, and a mismatch matching neither
FAILS the run with a reproducer. `TAG_POLICY_ID_BASE` exists so those tests can be
made at all — resource policies are numbered from 1 and tag policies from 900, so
"the crate named the tag policy and Ranger named the resource policy" is
readable off the ids.

These are the numbers AFTER the findings below were fixed and their
exclusions removed. Whether the corpus still REACHES the shapes those fixes
were written for is not a matter of opinion here — see the coverage table
below, which is printed on every run and fails it when an axis thins out.

**Exactly six refusals are counted and excluded from the comparison**, and the
line they sit on either side of is deliberate: each of the six is about an ORDER,
a CANDIDACY, a PRIORITY ARBITRATION or a VERSION this crate does not reproduce,
never about a construct it does not model. A construct refusal would mean the
generator had drifted into shapes the evaluator declines to answer, which is the
silent-green failure this whole file exists to prevent — so anything outside the
six still fails the run.
The classification is all-or-nothing per case (`only_declined`): a `why` mixing an
admitted refusal with a construct refusal is a generator escape wearing a
disguise, and reporting it as the admitted one would retire the escape check for
exactly the cases where it matters most.

`EvalOrderAmbiguous` is Task 6's refusal to guess a mask (or row filter) when
two policies tie inside one `(policyPriority, hasDeny)` band — the arbitration
there is `>=`, not `>`, so the first evaluator in `getEvalOrder()` is final,
and this crate does not reproduce `computeEvalOrder()`.

**The refusal fires on ANY tie, not only on one where the tied policies yield
different masks** — `evaluate_sub` never compares the two `SubResult`s. That
is deliberate: a `MaskResult` names the policy that produced it, and a tie
this crate cannot order is a tie whose winner it cannot name. Both halves are
pinned by
`crates/egret-ranger-eval/tests/mask_filter.rs::a_same_priority_mask_tie_is_undecidable_when_the_masks_{differ,are_identical}`.

**The cost splits in two, and only one half is unavoidable.** Measured by
patching `evaluate_sub` to refuse only on DISAGREEMENT and re-running this
gate at the recorded seed:

| | Cases of 4000 | Rate |
|---|---|---|
| Ties (what is refused today) | **144** | **3.6%** |
| …of which the tied policies yield DIFFERENT results | 111 | 2.8% |
| …of which they agree on everything but the policy id | 33 | 0.8% |

The 33 are not a hidden divergence: with the patch in place all 33 were
compared against the real engine and **all 33 agreed**. So 0.8% of the corpus
is a tie where this crate has a defensible answer and only the reported policy
id is unknown — which is exactly the shape the one live StarRocks profile hits
(`docs/en/plans/2026-09-28-sp4d-1-live-result.md` §8: mask policies 8777 and
8801 BOTH `dataMaskType: "MASK"`, no `valueExpr`). The patch was a measurement
only and was reverted; the evaluator refuses both halves.

### Five divergences the gate FOUND in the tag phase (SP4d-2b Task 5)

The corpus generated a `tagPolicies` block for the first time on 2026-10-01 and
disagreed with the real engine within 150 cases. Five mechanisms, all measured on
the real engines before anything was changed, and **not one of them was visible
to the 127 unit tests written alongside the tag phase** — every one of those
tests and the evaluator were written from the same reading of the same jars.

**T1 — only the TAG policy repository NORMALISES its policies, and one arm of it
CRASHES the plugin.** `RangerPolicyRepository`'s `ServicePolicies$TagPolicies`
constructor calls `normalizeAndPrunePolicies(getPolicies(),
componentServiceDef.getName())` at offsets 90-103, before `init()` and therefore
before any disabled policy has been dropped. **No other constructor in either jar
calls it.** Three consequences:

* an `accesses[].type` or a `dataMaskType` containing `:` is STRIPPED of a
  `"<defName>:"` prefix or else DROPPED (offsets 109-177 and 243-325). That is
  the spelling a Ranger-authored tag policy uses (`trino:select`), and this crate
  compared the literal — so a tag DENY granting `trino:select` denies a `select`
  request in Ranger and matched nothing here. **Fail-open.**
* a `dataMaskPolicyItems` item with no `dataMaskInfo` makes the constructor read
  `dataMaskType.contains(":")` on the null type Ranger's own model class just
  substituted (SP4d-1's finding F2) and **throw**. Measured on both jars:
  `NullPointerException ... at
  RangerPolicyRepository.normalizeAndPrunePolicyItems`. The plugin cannot build a
  policy engine from that download, so it answers NOTHING — of any policy type.
  The identical item in `policies` is legal and is still pinned as F2; only the
  tag repository normalises.
* `isPolicyNeedsPruning` removes a policy when `!getIsAuditEnabled()` and all SIX
  item buckets are empty. **`isDenyAllElse` is not one of the six**, which is what
  makes the prune observable: such a tag ACCESS policy denies everything here and
  does not exist there. Fail-CLOSED, which is why nothing noticed.

The first two REFUSE (`TagPolicyNotNormalisable`); the third is REPRODUCED
(`tag_policy_is_pruned`), because its gate is a plain read of the wire. The
refusal costs nothing on egret's own data: all three policies of
`crates/egret-ranger-eval/tests/fixtures/tag-policies-live.json` — captured from a
live plugin's own cache — carry BARE access types.

**T2 — the composed `EvalOrderAmbiguous` refusal was ORDER-DEPENDENT.** It lived
in the ALLOW arm of the resource loop alone, under a comment arguing that its scan
did not depend on this crate's order. The scan did not; *reaching* it did. When
the name tie-break put the DENIER first, the deny arm set `determined`, broke the
loop, and the crate answered `Denied` with full confidence over a band whose
winner `getEvalOrder()` picks. **6 of 387 compared cases at 2.8.0, every one
`egret said Denied / ranger said Allowed`.** The unit test that should have caught
it named the allower `a-allow` and the denier `b-deny`, so this crate sorted the
allower first and the test could not have failed; its mirror now sits beside it,
differing in nothing but two names.

**T3 — the three-way arbitration ran over evaluators Ranger's trie never
returns.** Offsets 452-535 set `determined` from the PRIORITY alone and never ask
the matcher, so a policy the trie removed could end the loop, skip the restore at
599-632 and change the verdict. Measured: Ranger answers the same thing with and
without such a policy; this crate answered `Allowed` with it and `Denied` without.
The loop now skips a view `trie_excludes` proves absent — a pure narrowing towards
Ranger's own list, since such a view could never have decided anything.

**T4 — the tag phase DOES arbitrate by `policyPriority`, and not where anyone
looked.** `evaluate_tag_policies` said *"unlike the resource loop this one
compares no priorities"*; reading `evaluateTagPolicies` is exactly why that looked
true. The arbitration is in
`RangerDefaultPolicyEvaluator.evaluatePolicyItems`, on the per-access-type
`accessTypeResults` map (`RangerAccessRequestUtil.getAccessTypeResults`) which
lives in the request CONTEXT and therefore survives from one tag evaluator to the
next. Measured on 2.8.0, one tag allow and one tag deny on the same tag:

| allow priority | deny priority | Ranger | this crate, before |
|---|---|---|---|
| 1 | 0 | **Allowed** by the allow | Denied ✗ |
| 0 | 0 | Denied by the deny | Denied ✓ |
| 0 | 1 | Denied by the deny | Denied ✓ |

A multi-priority tag candidate set now REFUSES
(`TagPriorityArbitrationNotModelled`).

⚠ **Fix round 1 removed one of the two reasons given for that, because it was
false.** The first version said the five-armed branch structure at offsets 366-573
"could not be read reliably from a disassembly". It can: `javap -c -p` prints the
whole sequence and `if_icmplt`/`if_icmple` operand order is fixed by the JVM spec.
The review decoded all five arms in one pass — `stored == null` → put; stored
ALLOWED and this DENIED → put when `thisPrio >= storedPrio`; stored ALLOWED, this
ALLOWED, ACCESS → `>`; stored ALLOWED, this ALLOWED, other type → `>=`; stored
DENIED → `>` (or `>=` with an ANY access type) — and the rule that falls out
reproduces all six measured rows in both jars.

**A refusal defended by a false reason is the thing this package has been bitten
by five times**, so the reason that holds is the only one left: the same
`accessTypeResults` map is live on the RESOURCE path, where this crate models a
DIFFERENT mechanism (the engine loop at 469/505/529), so a priority rule written in
the tag phase would be a second model of one mechanism.

⚠ **WHOLE-BRANCH REVIEW: the clause that used to close that sentence was
FALSIFIED.** It said the crate "is green over 4000 corpus cases — the two coincide
because the loop shuts out a lower-priority evaluator before it can reach the map".
They do not coincide. The green was over a population the divergent shape is
ABSENT from, because the corpus levels every random resource ACCESS policy to
priority 0, and the divergence (T7 below) was a confident over-DENY. The
coincidence held for every shape with no tag verdict and for every shape at one
priority; it failed for a tag ALLOW under a higher-priority resource allow, which
is the one shape that can leave `result` already-allowed at a priority BELOW a
later allower's. **The two models were not in agreement; one of them was
untested.**

And a fact that STRENGTHENS that reason: `evaluatePolicyItems` is near-identical
between the jars in the 366-573 region, so **this map cannot be what makes T5 a
version split.** There is a second unmodelled mechanism, and modelling one of them
would leave the other invisible.

The cost is measured: `egret-perm-compile` never emits `policyPriority` on a tag
policy and the live captured download carries 0 on all three, so a single-priority
block cannot reach the refusal — and as of fix round 1 that invariant is
ASSERTED, in `egret-perm-compile/tests/tag_service_routing.rs`, on the emitted
`policyPriority` rather than on the `is_override` flag it comes from. It had been
resting on three hard-coded `false`s with no test between them and the claim, and
`isOverride` is a public wire field.

**T5 — the two plugin versions DISAGREE.** A tag DENY plus a STRICTLY
higher-priority resource ALLOW with no deny items, on one minimal download
(request `hive.sales`, tag `pii` at `SELF`, tag policy denying `select` to bob at
priority 0):

| resource allow | deny items on it | 2.5.0 | 2.8.0 |
|---|---|---|---|
| priority 1 | none | **Denied**, the TAG policy's id | **Allowed**, the resource policy's id |
| priority 2 | none | **Denied**, the TAG policy's id | **Allowed**, the resource policy's id |
| priority 1 | present (matching nobody) | Denied, the resource policy's id | Denied, the resource policy's id |
| priority 0 | none | Denied, the TAG policy's id | Denied, the TAG policy's id |
| (no tag block) | none | Allowed | Allowed |

Ranger **2.5.0's own log line at offset 224** — *"access denied by a tag policy.
Higher priority resource policies will be evaluated to check for allow"* —
promises the 2.8.0 behaviour and does not deliver it. (The line is present at that
offset in BOTH jars.)

📄 **This one left the package.** It is a product-level fact — egret serves ONE
artifact to two engines that answer it differently — and it is written up for
people who do not read this file at
[`docs/en/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md`](../../docs/en/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md)
(Vietnamese reference: [`docs/vi/plans/…`](../../docs/vi/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md)).
That write-up carries the numeric policy id on every row, names the engines in the
column headers, and says what has NOT been decided: what egret should DO about it.
It cites this file four times, and this is the back-pointer, so the two are
findable from each other. The equal-priority row and
the deny-capable row agree across versions, which is what makes this specific
rather than general. This crate reproduces 2.8.0 and refuses on 2.5.0 and on
`plugin_version: None` (`TagDenyReopenedDiffersByPluginVersion`) — the one place
in the crate where an unstated plugin version costs an ACCESS answer, and it costs
it because the engines genuinely disagree.

### Two more gaps this gate measured — one still open, one CLOSED by the whole-branch review

Both were surfaced by the `restore_duel` shape, which exists because reverting
T3's fix left the gate GREEN — the corpus had no coverage for it at all.

**T6 — `level_excluded_by_trie` is wrong for an EMPTY request value, and the rule
that fixes it is not established.** Measured at ranger 2.8.0 on one fixture with
only the request changed:

| request | Ranger reaches a policy whose only level is the literal `no_such_value`? | `trie_excludes` says |
|---|---|---|
| `{catalog: ""}` | **yes** → `Allowed` | absent ✗ |
| `{catalog: "*"}` | no → `Denied` | absent ✓ |
| `{catalog: "", database: …, table: …, column: …}` | no → `Denied` | absent ✓ |

So `""` and `*` are NOT interchangeable to the trie, although
`isAllValuesRequested` treats them alike for the matcher — and a guard on "the
request value is empty" fixes row 1 and breaks row 3 (measured: 7 disagreements).
The depth of the request is in the rule somewhere (`RangerResourceEvaluatorsRetriever`
intersects over the request's keys and a shallower policy is an *inherited*
evaluator in the deeper ones), but three rows do not determine it, and this
function's job is to be a proof. Deferred rather than refused because the only
direction measured is fail-CLOSED and **an empty request level is not something
egret sends** — `perm_compile`'s `request_resource` builds concrete values and
`scope_to_resources`' `Access` arm fills unnamed levels with `*`. `restore_duel`
forces a concrete root value so the gate carries no standing disagreement.

**T7 — the `accessTypeResults` map reaches the RESOURCE loop, which is the audit
T4's refusal was deferred pending.** Measured at ranger 2.8.0, corpus case
`2.8.0-0848`: a tag ALLOW at priority 0, an `isDenyAllElse` resource policy at
priority 1 that allows, and a resource policy at priority 0 that denies. Ranger
answers `Allowed, policyId 2`; this crate answers `Denied by p1`. The reported id
is the tell — `updateAccessResult`'s allow branch is guarded by
`if (!result.getIsAllowed())`, and the id of a policy that ran under an
already-allowed tag verdict could only have been written into a FRESH
per-access-type result. So the map raises the slot's priority where this crate's
model says nothing moves, and `getCompositeAccessResult` copies that priority back
and shuts out the lower-priority denier this crate still runs.

⚠ **T7 does NOT reach `origin/main`, and that was settled by construction rather
than by analogy** (fix round 2). The re-review built the T7 shape with no
`tagPolicies` key at all and ran both engines: Ranger `Allowed, policyId 2`, crate
`Allowed, policyId 2` — **agree**. Same for an empty block, and for a tag policy
matching no tag the request carries. Its sharper control: a *lower*-priority
resource allower named first also agrees, because `getLikelyMatchPolicyEvaluators`
is priority-DESCENDING, so a resource allow can never seed `isAllowed` below a later
allower's priority. Re-adding the tag ALLOW restores the divergence. So T7 is a
BRANCH finding — it needs the tag phase this branch introduces — not a shipped
defect.

**The map is live on EVERY request, not only an `_any` one, and that is what makes
T4 and T7 one finding.** `evaluatePolicyItems`' map path is gated on
`isNotEmpty(getAllRequestedAccessTypes(request))` (offsets 61-72) and
`getAllRequestedAccessTypes` falls back to
`Collections.singleton(request.getAccessType())` (offsets 67-84), so there is no
single-access-type bypass; the composite is copied back at offsets 627-643,
`result.setAccessResultFrom(getCompositeAccessResult(…))`. That is also why the two
models COINCIDE on the resource path, which "green over 4000 cases" could not
explain: with no tag verdict the first allower's `if (!result.getIsAllowed())` guard
is open, so it writes id AND priority into both `result` and the map and the two stay
in lockstep. Only a tag ALLOW can leave `result` already-allowed at a priority BELOW
a later resource allower's.

**This invalidates part of the resource loop's model, not just the tag phase's.**

⚠ **WHOLE-BRANCH REVIEW: T7 is now REFUSED, and the reason it was only held out
was wrong.** The paragraph that stood here said the honest refusal — *"a resource
policy above the tag verdict's priority decides"* — would also swallow the four
rows of T5's table and the three unit tests that pin them. **It does not.** T5's
rows are a tag DENY; the arm that refuses T7 fires only when the tag phase
ALLOWED, so the two are disjoint by construction and all three of those tests
still answer. The refusal was drawn wider in the argument than it needed to be,
and the shape shipped unrefused on that argument.

What is refused, and what it cost:

* `UndecidableCause::AccessPriorityCompositeNotModelled`, raised in `evaluate`'s
  allow arm on `allowed && found.allowed && found.priority > priority`. `allowed`
  there can only be the TAG phase's: it is set inside the loop at one place that
  also sets `priority = found.priority`, and `sort_eval_order` is priority
  DESCENDING, so a later candidate's priority is `<= priority`.
* the direction was **always over-DENY** — a confident wrong answer, not a safe
  one — and the shape is live-reachable by one supported API call
  (`AccessPolicyInput.isOverride` emits `policyPriority: 1`).
* the cost is near zero on egret-authored data: Task 5 asserts
  `policyPriority == 0` on every compiled grant, rule, clearance and tag body, so
  only a HAND-AUTHORED override policy reaches it. In the corpus it is 10 of 4000,
  all in the composed-duel column.
* the ruling was REFUSE, not model. Modelling is open item 0. The candidate model
  is one token — making the allow arm write unconditionally — and the two-case
  fixture cannot tell a correct model from a lucky one, which is exactly why it
  waits for the audit.

**The line was pinned in NEITHER direction before.** Mutating
`} else if !allowed {` to `} else {` compiles and survived the whole suite, 259
passed / 0 failed. Both mutants are now killed by
`tag_arbitration.rs::a_tag_allow_under_a_higher_priority_resource_allow_refuses`
(JVM-free) and by `differential.rs::check_measured_priority_composite`, which
re-measures both real engines on the two cases below.

MEASURED, one download, byte-identical to both engines:

| | 2.5.0 | 2.8.0 |
|---|---|---|
| tag ACCESS 901 @0 ALLOW + resource `p1` id 1 @1 ALLOW `isDenyAllElse` + resource `p2` id 2 @0 DENY | **Allowed, `policyId` 1** | **Allowed, `policyId` 1** |
| control: the same two resource policies, no `tagPolicies` block | Allowed, `policyId` 1 | Allowed, `policyId` 1 |

Same answer at both versions, so it is not a version split; the control is what
locates the trigger on the tag ALLOW.

**The LEVELLING stays, and it is not a way of hiding the refusal.** Random
tag-carrying cases still emit `policyPriority: 0` on their resource ACCESS
policies — **ACCESS only**, because levelling the mask and row-filter priorities
took `TagSubResultSplitByResourcePolicy` from 19 corpus firings to 0, and a
held-out shape that silently removes a refusal's only coverage is the silent green
this file exists to prevent. It stays because the resource-path rates this gate
reports are rates over cases with no tag phase, and because un-levelling would
replace a stated hole with a refusal rate that tells a reader less. What it costs
is now COUNTED: the `RANDOM composed ACCESS case with a resource policy above
priority 0` axis prints 0, with a floor of 0 that is a tripwire on the hold-out
rather than a coverage demand. Getting that axis right took two measurements —
the bare conjunction read 243‰ (every deliberate duel) and the
population-scoped one still read 61‰ (the mask and row-filter policies, which are
not in the ACCESS loop) — and both wrong versions would have read as healthy
coverage of the shape that has none. Every DELIBERATE duel keeps its priorities.

### Follow-up work this left behind, with the rows to aim at

Named here rather than in a report nobody re-reads:

0. **Model the `accessTypeResults` arbitration, which is now three findings deep**
   (T4, T5's mechanism being ruled out, and T7). It is the single highest-value
   piece of work left in this area: it is live on BOTH paths, it is what T4 refuses
   rather than models, and T7 is a measured wrong answer on the resource loop. The
   review decoded all five of its arms from `javap -c` (see T4 above) and the rule
   reproduces six measured rows; what is missing is the resource-path audit, and
   T7 IS that audit's first result.

   ⚠ **Two things that task now inherits, both added by the whole-branch review.**
   First, T7 is REFUSED rather than wrong, so the task's deliverable is to replace
   two refusals (`TagPriorityArbitrationNotModelled` and
   `AccessPriorityCompositeNotModelled`) with one model — and the candidate model on
   the resource path is a ONE-TOKEN change, making the allow arm write
   unconditionally. The two-case fixture agrees with that model, which is exactly
   why it is not enough: a fixture cannot tell a correct model from a lucky one, and
   `setAccessResultFrom` copying `policyPriority` unconditionally also means it can
   LOWER the priority, a direction nothing has measured.
   Second, **the same-verdict FRACTION of `TagPriorityArbitrationNotModelled` is
   still not split** (23 of 1148 composed-random cases, unsplit) and it is the size
   of the prize. Splitting it needs the agreement on the cause payload, which is a
   wire-shape change through `explain_wire.rs`, `openapi.json` and the frontend
   switch — so it belongs in this task and not before it.
1. **Model `normalizeAndPrunePolicyItems`' prefix strip** instead of refusing
   `TagNormalisationProblem::Namespaced{AccessType,MaskType}`. The rule is fully
   measured (see T1); what is not done is the two prunes it feeds and their
   interaction with `hasDeny()` and the tag comparators.
2. ~~**Model the `accessTypeResults` arbitration.**~~ Promoted to **item 0** above
   by the whole-branch review: the "observation, not a proof" that used to sit here
   was falsified by T7, and the audit is no longer deferrable.
3. **A tag block missing `serviceName` or `serviceDef`.** `PolicyEngine.<init>`
   builds no tag repository unless `tagPolicies != null &&
   !isEmpty(getServiceName()) && getServiceDef() != null` (offsets 471-500), so
   Ranger runs no tag phase at all — while `tag_phase_input`, which reads neither
   field, runs one. Held out of the corpus because a generated case cannot both
   measure it and be comparable. Not yet reachable through egret's own writer,
   which always emits both.

### The gate has been observed failing

A gate never seen failing is not known to be a gate. Two breaks, both
restored from a backup and verified byte-identical by `shasum` rather than by
eye (`cp` is aliased to `cp -i` on this machine and silently refuses):

- `MatchType::Ancestor` added to the "matched" test in
  `crates/egret-ranger-eval/src/eval.rs` — making an ANCESTOR match an allow,
  the fail-open direction. Caught at only 300 cases per engine: **6 of 290
  compared cases disagreed** at ranger 2.5.0.
- `Token::Any` in `resource.rs` made to advance the text index
  unconditionally, i.e. back towards a greedy matcher. Caught by the
  JVM-free unit table alone, on `wildcardMatch("ab", "?*?")` — no Maven, no
  `EGRET_TEST_RANGER_E2E`.
- **`MatchType::Descendant` added to `tag_match_is_admitted`** (SP4d-2b Task 5) —
  the fail-open direction on the tag path, where `ANCESTOR` *is* admitted and
  `DESCENDANT` is not, the opposite of the resource path. The mutant COMPILES
  (checked; a mutation that does not build looks exactly like one no test caught).
  Caught at 400 cases per engine: **2 of 351 compared cases disagreed at ranger
  2.8.0**, and 5 JVM-free unit tests failed alongside. Restored from a backup and
  verified by `shasum -a 256 -c` rather than by `git diff` — a bare diff proves
  *a* change landed, not *the* change, and `cp` is aliased to `cp -i` on this
  machine and silently refuses to overwrite.
- **Both halves of the `accessTypeResults` composite arm** (whole-branch review).
  The line this fixes — `} else if !allowed {` in `evaluate`'s allow arm — was
  pinned in NEITHER direction before: mutating it to `} else {` compiles and
  survived 259 passed / 0 failed. Two mutants, each compiled (0 errors) and each
  restored by `shasum -a 256 -c`:
  - `} else if found.priority > priority {` → `} else if false {`, removing the
    refusal: **1 of 36 `tag_arbitration` tests failed**, JVM-free.
  - `} else if !allowed {` → `} else if true {`, i.e. the allow arm writing
    unconditionally — the coordinator's surviving mutant, which makes the refusal
    arm dead: **1 of 36 failed**, JVM-free.

  Neither needs Maven, which matters: the shape the corpus cannot generate is the
  shape whose pin must not depend on the corpus.

## Declared divergences from the real engine

Places where this crate is knowingly not byte-for-byte Ranger, with the reason
each is acceptable. Anything not on this list and not `Undecidable` is a bug.

- **Case folding is NARROWER than Java's: 104 ordered pairs, in 21
  classes.** Measured over **every BMP code point this JDK defines**, by two
  independent instruments that agree exactly:

  1. Java's fold key `toLowerCase(toUpperCase(c))`, computed for all 62034
     defined code points and grouped — Java's rule reduces to that key.
  2. The real `String.regionMatches(true, …)`, brute-forced over every
     ordered pair of the 2423 code points either language treats as cased (a
     pair outside that set cannot disagree; both mappings are the identity).

  **104 disagreements, and in every one Java says `true` while this crate
  says `false`. There are none the other way.**

  The direction is the point, and an earlier version of this section had it
  backwards. Narrower means egret finds no match where Ranger finds one:
  **fail-CLOSED for an allow rule, fail-OPEN for a deny or an `isExcludes`
  rule.** Calling it "wider" implied it was uniformly safe. It is not.

  An earlier version also said "exactly two classes … 22 disagreements".
  That was the honest answer to a 1922-pair hand-picked cross product — the
  claim was simply wider than the measurement behind it. The 21 classes:

  | # | Class | Pairs | Mechanism |
  |---|---|---|---|
  | 1 | `I i İ ı` (U+0049 U+0069 U+0130 U+0131) | 10 | Java's third step maps `İ` to a single `i`; Rust's `char::to_lowercase` yields TWO code points for it, so no single-character comparison can reach Java's answer. |
  | 2 | `S s ſ` (U+017F long s) | 4 | missing UPPERCASE step |
  | 3 | `Σ ς σ` (final sigma) | 4 | missing UPPERCASE step |
  | 4 | `Μ μ µ` (U+00B5 micro sign) | 4 | missing UPPERCASE step |
  | 5 | `Ι ι ͅ ι` (U+0345 iota subscript, U+1FBE prosgegrammeni) | 10 | missing UPPERCASE step |
  | 6-12 | the Greek symbol variants `ϐ ϵ ϑ/ϴ ϰ ϖ ϱ ϕ` | 4-6 each | missing UPPERCASE step |
  | 13-20 | the Cyrillic historic shapes U+1C80-U+1C88 (`ᲀ ᲁ ᲂ ᲃ ᲄ ᲅ ᲆ ᲇ ᲈ`) | 4-10 each | missing UPPERCASE step |
  | 21 | `Ṡ ṡ ẛ` (U+1E9B) | 4 | missing UPPERCASE step |

  So class 1 is a length problem and the other twenty are all one mechanism:
  `toUpperCase` collapses the variant onto the canonical letter, while
  lowercasing leaves them distinct. Adding a `to_uppercase` step closes **96
  of the 104** — measured by making that change and re-running the test, which
  then reports 8 survivors: the six pairs involving `İ` (Rust lowercases it to
  TWO code points, so no single-character step reaches Java's answer) and `ϑ`
  vs `ϴ`.

  No catalog/schema/database/table/column identifier on either engine admits
  any of these code points.

  The full set, its size and its direction are pinned in
  `resource.rs::case_folding_is_narrower_than_java_on_104_ordered_pairs_in_21_classes`,
  which enumerates every class and asserts both that exactly 104 ordered
  pairs split and that no CROSS-class pair folds equal. That second
  assertion is the direction, and it is the one worth keeping: the claim and
  the test now cover the same set, so they cannot drift apart.

  One caveat the brute force turned up and this crate does not own: 16
  ordered pairs run the OTHER way (Rust folds them, Java does not), and
  every one involves a code point `Character.isDefined` says JDK 21 does not
  know — U+A7CB, U+1C89, U+A7DC and their partners. That is Unicode-table
  skew between the JVM's tables and Rust std's, not a folding difference; it
  moves with the JVM. None of those code points is an identifier character
  on either engine either.

  `Equals`, `StartsWith` and `EndsWith` fold the whole string; the wildcard
  path deliberately does **not**, and the asymmetry is worth knowing about.
  `wildcard_match` walks token by token doing index arithmetic against the
  text, and a whole-string fold that changes the string's LENGTH would shift
  every index away from the ones Java computes, so it compares per position
  instead — which is what `IOCase.checkRegionMatches` does. Both paths
  diverge in the same direction and on the same code points.

- **The reported policy id of an ACCESS decision can differ, and the gate now
  ASSERTS on it.** Within one `(policyPriority, hasDeny)` band Ranger picks by
  `getEvalOrder()` and this crate by a name-then-id tie-break, so the two can
  agree on allow/deny while naming different policies.

  ⚠ **This entry said "nothing in this crate asserts a reported ACCESS policy id,
  which is why the gate is green in spite of it" and quoted `39 of 1972`. Both are
  superseded** (SP4d-2b Task 5, fix round 1 RULING 3 / round 2). The id IS compared
  now, on its own axis, and this section is the declared pointer the allow-list's
  first entry sends a reader to — so it was denying the existence of the thing it
  is cited for. Current, at the recorded seed:

  | | |
  |---|---|
  | ACCESS cases where the verdicts agree and both sides named an id | **1602** |
  | agreeing on the id | **1465 = 91.45%** |
  | this divergence (same `(policyPriority, hasDeny)` band) | **47**, floor 5‰ |
  | the restore divergence below | **90**, floor 11‰ |
  | anything else | **0**, and it FAILS the run with a reproducer |

  The allow/deny OUTCOME is unaffected (see the duels below). What keeps the gate
  green is that these two classes are a CLOSED allow-list, each tested
  structurally on the two policies the two sides named — not that the id is
  unchecked.

- **A tag deny restored over a discarded resource allow reports the TAG
  policy, deliberately.** SP4d-2b Task 3, and the one entry on this list that
  is a CHOICE this crate makes rather than a consequence of something it could
  not reproduce — recorded here so a later corpus run does not read it as a new
  finding and send someone hunting a regression.

  The shape: tag policies deny; a strictly higher-priority resource policy
  allows but is `hasDeny()`, so offset 553 cannot determine for it; the loop
  runs out and the restore at offsets 599-632 calls `setIsAllowed(false)`. That
  call does not touch `policyId`, so **Ranger names the resource policy that
  ALLOWED as the source of the deny.** This crate names the tag policy that
  actually denied.

  The allow/deny OUTCOME is identical — only the reported id differs, so this is
  the SECOND entry of the id allow-list above rather than a new class of
  disagreement. ⚠ Its classifier tests all four of this shape's conditions — the
  crate named a tag policy, Ranger named a resource policy, the tag policy is
  deny-capable, and the resource policy allows at a strictly higher priority.
  Round 1 tested only "the two ids are in different blocks", which absorbed a
  measured instance of T7's `accessTypeResults` mechanism with no tag deny anywhere
  in the case; round 2 narrowed it, and re-running the whole gate showed nothing
  had been mis-filed (the split label fired 0 times). The reason for the choice is that Ranger's
  answer sends a reader to a policy that allows, to explain a deny.
  `eval.rs`'s restore block carries the same note at the site;
  `tag_arbitration.rs::nothing_else_determines_so_the_tag_verdict_is_restored`
  is the fixture.

- **`isAccessDetermined` is not modelled.** Ranger sets it `true` even when
  nothing matched at all, alongside `policyId = -1`: measured in **716 of 1972
  compared ACCESS cases**. This crate reports that state as
  `DenyReason::NoPolicyMatched` and makes no claim about the flag.

  This vindicates the decision not to assert it. A naive assertion that
  `NoPolicyMatched` means `isAccessDetermined == false` would have
  manufactured ~716 failures that are not findings — and a gate that cries
  wolf at that rate gets switched off, taking the real findings with it. The
  verdict still records the flag, for a later task that measures the
  end-of-loop `setPolicyId(-1)` fallback properly.

- **Character indices stand in for Java's UTF-16 indices** throughout the
  matcher. They differ only for astral characters, which no engine's resource
  vocabulary admits.

- **`_any` against an ANCESTOR-matching `isDenyAllElse` policy** denies on
  2.5.0 and does not on 2.8.0 — see below. `_any` is
  `UndecidableCause::PseudoAccessType`, so this crate refuses rather than
  picking a version.

- **`getEvalOrder()` is not reproduced.** Within one
  `(policyPriority, hasDeny)` band this crate cannot say which of two
  same-priority mask or row-filter policies Ranger reaches first, and refuses
  with `EvalOrderAmbiguous` rather than guessing — on ANY tie, not only on
  one where the tied policies would yield different results. Measured cost:
  **3.6% of the corpus at the recorded seed (144 of 4000), of which 2.8% are
  ties that actually disagree and 0.8% ties that agree on everything but the
  policy id**; 4.0% over a 10000-case sweep, and 3.3-4.5% across four
  unrelated seeds at 1000 cases each (per engine the extremes are 3.0% and
  5.6%). See "The differential gate" above for the split and how it was
  measured. For ACCESS the outcome does not depend on it — see the duels
  below.

## Coverage: what the corpus actually produced

Once the gate is green, "green" and "still testing the shape" become
indistinguishable. A generator edit that quietly stops emitting `isExcludes`
costs nothing visible — the corpus stays green, because agreement on cases
that never exercise a rule is agreement about nothing. That is a **silent,
green failure**, which is the failure mode this whole package exists to
eliminate elsewhere.

So every axis has a structural counter and a floor that FAILS the run, and the
table prints on **every** run rather than only on failure: a number nobody
sees is a number nobody checks, and the point is to notice an axis thinning
out long before it reaches zero.

The predicates read the generated JSON and the request, never the outcome, so
a counter cannot be satisfied by a decision that happened to come out a
certain way. `axis_hits` and `AXES` are checked against each other BY NAME at
runtime, because a silent misalignment would attach every floor below it to
the wrong counter — the same class of defect the table exists to catch.

Floors are in cases per thousand, so they survive a change to
`EGRET_DIFF_CASES`, and each is set near half of what the corpus measurably
produces. A floor that flaps gets deleted by the next person, which is worse
than no floor: over the 89 floored rows of the recorded run (45 axes × 2
engines, less the one row that is `n/a` on StarRocks) the tightest margin is
**1.968x**, on `policyType 0 (access)`. Three floors were LOWERED when the
backtracking vocabulary diluted their axes — `wildcard: repeated ?` 60→50,
`request value *` 70→50 and `request value empty` 70→55 — rather than left to
flap; the table above shows the lowered values, because it is re-recorded from
a run rather than patched by hand.

Those floors were calibrated against the recorded seed, which on its own only
shows they hold for the run that set them. Four unrelated seeds
(`0xdead1`, `0xbeef2`, `0xc0ffee3`, `0x13579b`) were then re-run at 500 cases
against the 45-axis table: all pass, and across **356** floored rows (4 seeds ×
2 engines × 45 axes, less the 4 `n/a` rows) the tightest margin anywhere is
**1.733x** (`ANCESTOR-shaped policy` at `0xc0ffee3`, `policyType 2 (row
filter)` and `row-filter case at TABLE granularity` at `0xbeef2`). So the
margin survives seed variation with room to spare, rather than being expected
to.

**How this table was produced, so the next person regenerates it the same
way.** It is the `coverage at ranger 2.5.0 (Trino)` block the gate prints,
copied verbatim from one run of:

```bash
EGRET_TEST_RANGER_E2E=1 EGRET_DIFF_SEED=54d000007 EGRET_DIFF_CASES=2000 \
  cargo test -p egret-ranger-eval --test differential -- --nocapture
```

Do not hand-patch individual rows: the axis LIST comes from `AXES` in
`crates/egret-ranger-eval/tests/differential.rs`, so an added axis has to
appear here as a new row, and a hand-edit is how this table came to be missing
one. Re-record the whole block.

Measured at ranger 2.5.0 (Trino), 2000 cases, seed `0x54d000007` — **45 axes,
the same 45 the run prints**:

| Axis | Cases | Rate | Floor |
|---|---|---|---|
| policyType 0 (access) | 988 | 494‰ | 250‰ |
| policyType 1 (data mask) | 515 | 257‰ | 120‰ |
| policyType 2 (row filter) | 497 | 248‰ | 120‰ |
| row-filter case at TABLE granularity | 497 | 248‰ | 120‰ |
| policy of a DIFFERENT type in the set | 395 | 197‰ | 80‰ |
| ANCESTOR-shaped policy (fewer levels than request) | 248 | 124‰ | 60‰ |
| policy DEEPER than the request | 513 | 256‰ | 80‰ |
| isExcludes | 814 | 407‰ | 100‰ |
| multi-value level | 1140 | 570‰ | 100‰ |
| level with no usable values | 436 | 218‰ | 40‰ |
| additionalResources | 487 | 243‰ | 100‰ |
| wildcard: leading * | 1328 | 664‰ | 200‰ |
| wildcard: trailing * | 1258 | 629‰ | 200‰ |
| wildcard: interior * | 1167 | 583‰ | 150‰ |
| wildcard: repeated * | 1205 | 602‰ | 150‰ |
| wildcard: leading ? | 696 | 348‰ | 100‰ |
| wildcard: trailing ? | 768 | 384‰ | 100‰ |
| wildcard: interior ? | 672 | 336‰ | 100‰ |
| wildcard: repeated ? | 219 | 109‰ | 50‰ |
| wildcard: `*` immediately before `?` (F1) | 455 | 227‰ | 100‰ |
| wildcard: literal AFTER a `*` (backtracking reachable) | 1418 | 709‰ | 100‰ |
| policy value that is only `*` | 1253 | 626‰ | 200‰ |
| policy value differing from the request only by case | 536 | 268‰ | 100‰ |
| request value `*` | 211 | 105‰ | 50‰ |
| request value empty | 264 | 132‰ | 55‰ |
| request with a gap in the hierarchy | 1247 | 623‰ | 30‰ |
| bucket policyItems | 1166 | 583‰ | 280‰ |
| bucket denyPolicyItems | 860 | 430‰ | 150‰ |
| bucket allowExceptions | 321 | 160‰ | 60‰ |
| bucket denyExceptions | 319 | 159‰ | 60‰ |
| group `public` | 732 | 366‰ | 80‰ |
| user `{USER}` | 913 | 456‰ | 80‰ |
| policy item naming a role | 1135 | 567‰ | 60‰ |
| policyPriority override | 601 | 300‰ | 140‰ |
| isDenyAllElse | 171 | 85‰ | 40‰ |
| disabled policy | 330 | 165‰ | 70‰ |
| impliedGrants via trino `all` | 1123 | 561‰ | 60‰ |
| access entry with isAllowed:false | 816 | 408‰ | 150‰ |
| access entry whose type reads as `""` (F3) | 697 | 348‰ | 40‰ |
| dataMask item with NO dataMaskInfo (F2) | 165 | 82‰ | 40‰ |
| rowFilter item with NO rowFilterInfo | 120 | 60‰ | 20‰ |
| CUSTOM mask carrying a valueExpr | 231 | 115‰ | 40‰ |
| mask conditionExpr | 136 | 68‰ | 20‰ |
| filterExpr explicitly empty or null | 365 | 182‰ | 40‰ |
| eval-order duel | 266 | 133‰ | 55‰ |

`impliedGrants via trino \`all\`` is the one engine-specific row: StarRocks's
service-def declares no `impliedGrants` at all, so the axis prints `n/a for
this engine` there and is floored only for Trino.

**Floors prove a shape is GENERATED, not that a rule is REACHED — and review
found the difference the hard way.** Deleting the backtracking resume point in
`wildcard_match` (`backtrack.push((wcs_idx, repeat))`) left every unit test
green, the `FilenameUtils` oracle green **as it then stood, at 53 rows**, and
the whole 4000-case gate green at the recorded seed with identical tallies.
(That oracle is now 81 rows — see
`resource.rs::wildcard_match_reproduces_filenameutils` — and the added rows
are what kills the mutation.) The corpus emitted plenty
of wildcards; almost none of them could reach that line, because a value
ending in `*` jumps to the end of the text and never needs a resume point, and
the corpus's literal runs never recurred in a request value. Both halves are
now closed: 27 measured rows in the unit table, `POLICY_VALUES` entries like
`*ab*ab` that do NOT end in `*`, `REQUEST_VALUES` texts whose runs recur
(`ababab`, `sisters`, `banana`, `assessors`), and a
`wildcard: literal AFTER a *` axis with its own floor. Deleting the resume
point now fails the unit table AND the corpus (7 of 485 disagreed at 500
cases per engine).

**The floors have been observed failing.** With `isExcludes` generation
disabled the run stops with `isExcludes - 0 cases (0per-mille), floor
100per-mille` and names the axis; the file was restored and verified
byte-identical by `shasum`.

### Three divergences the gate FOUND, since fixed

The first run of the corpus disagreed with the real engine on 3 of 1200
cases. Both mechanisms were read out of the 2.5.0 bytecode, and chasing the
second one turned up a third. All three are fixed; the minimal reproducers
for the first two are kept as pins (`pinned_divergences`) that now assert
AGREEMENT on the exact inputs that caught them.

**F1 — `*` immediately followed by `?`. Was FAIL-OPEN.** Task 4 reimplemented
`FilenameUtils.wildcardMatch` rather than binding to it, and said the two had
never been compared. They disagreed. `wildcardMatch` is not a greedy matcher
with backtracking: `splitOnTokens` turns `?*?` into `["?", "*", "?"]`, and a
`*` token advances the text index only when it is the LAST token
(`textIdx = filename.length()`) or when the next token is a literal (via
`checkIndexOf`). A `*` followed by `?` consumes nothing:

```text
FilenameUtils.wildcardMatch("HIVE", "?*?", INSENSITIVE) == false
FilenameUtils.wildcardMatch("HIVE", "*?",  INSENSITIVE) == false
FilenameUtils.wildcardMatch("ab",   "?*?", INSENSITIVE) == true
```

`crates/egret-ranger-eval/src/resource.rs` now ports `splitOnTokens` and the
token walk instead of approximating them. Read from `javap -c` of
**commons-io 2.8.0** — what `mvn dependency:build-classpath` resolves for
`ranger-plugins-common:2.5.0` — and confirmed instruction-identical in
**commons-io 2.16.1**, which the same command resolves for
`ranger-plugins-common:2.8.0`. A 3348-pair cross product produced
byte-identical output from the two jars, so the exact commons-io version
changes no answer; the port agrees with all 3348, and the interesting rows are
a unit test that needs no JVM — **81 of them** today, in
`resource.rs::wildcard_match_reproduces_filenameutils`, grown from 54 when
review showed the original table shared the corpus's blind spot.

**F2 — a `dataMaskPolicyItems` entry with no `dataMaskInfo`.** The
deserialiser is **Jackson**, and which library it is IS the finding.
`JsonUtils.getMapper()` returns a
`com.fasterxml.jackson.databind.ObjectMapper` (`javap -c` of
`org.apache.ranger.authorization.utils.JsonUtils`), and neither
`ranger-plugins-common` jar contains a single Gson class — the only mention of
Gson anywhere in the 2.5.0 jar is a line in `META-INF/DEPENDENCIES`.

Jackson binds through the **setter**.
`RangerPolicy$RangerDataMaskPolicyItem.setDataMaskInfo` substitutes
`new RangerPolicyItemDataMaskInfo()` for a null argument (`ifnonnull` at
offset 2), and the no-arg constructor reaches the same setter through the
all-args one. So `getDataMaskInfo()` is **never null** on a deserialised item
whether the key was absent or explicitly null, the `ifnull` guard at offset 8
of `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` never fires,
and the item DETERMINES the result with a null mask type — stopping the
search.

Had it been Gson, the prediction would be the OPPOSITE: Gson assigns fields
directly and bypasses setters, so `dataMaskInfo` would stay null and the
`ifnull` guard WOULD fire. An earlier version of this section named Gson, and
that naming predicts the behaviour the gate measured as false.

Where the confusion came from is worth writing down, because it is a trap
anyone re-reading this harness can fall into: **`EgretCaseRunner` itself uses
Gson** — for its own `cases.json` / `verdicts.json`, which are the harness's
files and nothing to do with Ranger. The `ServicePolicies` under test goes
through `JsonUtils.jsonToObject` (line 112), i.e. Jackson, so the differential
gate does exercise Ranger's real binding.

Fixed in the READER (`model.rs`), which is where the defect was: the reader
was reading the wire format while Ranger reads the same bytes through classes
whose setters rewrite them.

**The row-filter path makes the identical substitution and behaves the
OPPOSITE way**, which is why the fix could not be uniform.
`RangerRowFilterPolicyItem.setRowFilterInfo(null)` substitutes an empty object
too, but `RangerDefaultRowFilterPolicyItemEvaluator.updateAccessResult` has no
guard at the top — it guards its `policyEvaluator.updateAccessResult(...)`
call with `if (result.getFilterExpr() != null)` (offsets 40-44) instead. A
null `filterExpr` therefore determines nothing and the next policy IS reached.

**F3 — the reader refused documents the live plugin enforces.** Chasing F2's
class turned this up. An ABSENT key is never bound at all, so the field keeps
whatever the constructor left, and `RangerPolicy`, `RangerPolicyItem`,
`RangerPolicyResource` and `RangerPolicyItemAccess` all normalise in their
constructors. An EXPLICIT `null` goes through Jackson's setter call, which for
these classes substitutes the same default, and reads back through the same
getter — so Ranger cannot tell "absent" from "null". Measured on both shapes
with `JsonUtils.jsonToObject` rather than derived from the binding rules:

| Class | Field | Ranger's value for absent AND null |
|---|---|---|
| `RangerPolicy` | `policyPriority` | `0` |
| | `isEnabled` | `true` |
| | `isAuditEnabled` | `true` (not modelled here) |
| | `isDenyAllElse` | `false` |
| | `resources` | empty map |
| | `policyItems`, `denyPolicyItems`, `allowExceptions`, `denyExceptions`, `dataMaskPolicyItems`, `rowFilterPolicyItems`, `validitySchedules` | empty list |
| | `policyType`, `additionalResources`, `conditions`, `zoneName`, `id`, `name`, `service` | **null** — no substitution |
| `RangerPolicyResource` | `values` | empty list |
| | `isExcludes`, `isRecursive` | `false` |
| `RangerPolicyItem` | `users`, `groups`, `roles`, `accesses`, `conditions` | empty list |
| | `delegateAdmin` | `false` |
| `RangerPolicyItemAccess` | `type` | `""` |
| | `isAllowed` | `true` |
| `RangerDataMaskPolicyItem` | `dataMaskInfo` | empty object (F2) |
| `RangerRowFilterPolicyItem` | `rowFilterInfo` | empty object |
| `RangerPolicyItemDataMaskInfo` | `dataMaskType`, `valueExpr`, `conditionExpr` | **null** |
| `RangerPolicyItemRowFilterInfo` | `filterExpr` | **null** |
| `ServicePolicies` | every field | **null** |

Before this, `serde` rejected every explicit null with `invalid type: null`,
and an absent `accesses[].type` with `missing field`. A reader that cannot
read is not fail-safe — it simply has no answer where one is needed. The whole
table is now a test,
`crates/egret-ranger-eval/tests/model.rs::absent_and_null_keys_take_rangers_own_defaults`.

Three of those rows are `null` rather than a substituted default, and they are
the reason the sweep could not be applied mechanically:
`additionalResources` and `conditions` come back null where the rest come back
empty, and `RangerPolicy.policyType` comes back null where
`RangerPolicyItemAccess.type` comes back `""`. Reading them as "empty" is
correct here only because an empty additional-resource list and an absent one
produce the same decision, and because `RangerPolicyRepository.init` files a
null `policyType` into the access bucket — i.e. `0`.

### The eval-order claim, now actually tested

Task 5 did not reproduce `getEvalOrder()` and argued that within one
`(policyPriority, hasDeny)` band the allow/deny OUTCOME cannot depend on
order. Ten thousand generated cases produced no counterexample — but the
generator could not produce the shape a counterexample needs, so that was not
evidence.

`access_order_duel` now builds it deliberately: one case in four of the ACCESS
corpus is two policies at the SAME priority, both certainly matching, one
reaching an allow and the other a deny, with ids AND names swapped half the
time so a name-then-id tie-break cannot be right by luck. Both constructible
`hasDeny` combinations are generated — there are two, not three: a duel needs
a policy that actually denies, and such a policy necessarily carries
`denyPolicyItems`, so `hasDeny == false` on the denier cannot be built. What
varies is whether the ALLOWER also carries (non-matching) deny items.
`PolicyEvalOrderComparator.compareNormal`, disassembled from the 2.5.0 jar,
sorts `hasDeny` FIRST and only then by `getEvalOrder()`, which makes those two
different questions.

**539 duels in the recorded 4000-case corpus, 433 agreeing and the rest
REFUSED — none disagreeing** (the counts are the recorded-run table's; this section
no longer keeps a second set). Task 5's claim survives a test that could have
killed it, and the `hasDeny` key in the comparator is why: it puts the denying
policy first, so the engine loop's
`if (ret.getIsAllowed()) { if (!evaluator.hasDeny()) break; }` cannot break
past a same-priority deny.

**250 of those duels are COMPOSED** (SP4d-2b Task 5): the same two resource
policies issued UNDER a tag verdict, at a priority strictly above the tag's. That
is the one shape in which `getEvalOrder()` CAN decide an ACCESS outcome, and it is
where the claim above stops holding — 178 of the 260 agree and the other 82
refuse. One case in four of the composed duels is built with a tag ALLOW instead
of a deny, and that quarter is the FALSIFIER rather than filler: under an allow
offset 505's `>` lets the same-priority rival run after the allow, so the deny
wins whichever ran first and the crate must NOT refuse. A corpus that built only
the deny variant would report a rate with nothing to compare it against.

### What the corpus covers

Each axis is here because some earlier claim can only be falsified by running
the real engine: wildcards (`*` and `?` leading, trailing, interior, repeated,
adjacent; empty values; values that are only `*`; case differences under
`ignoreCase: true`); **request-side** `*` and `""`, where `getMatchType`
short-circuits on `isAllValuesRequested` before reading any policy value;
several same-priority policies over one resource; hierarchy depth including
the ANCESTOR shape; `isExcludes`; multi-value levels; `additionalResources`;
`policyPriority`; all four access buckets with overlapping principals;
`isDenyAllElse`; `impliedGrants` (Trino's `all`); the literal `public`;
`{USER}`; disabled policies; a policy of the wrong `policyType`; and all three
policy types.

**Row-filter cases are issued at TABLE granularity.** `rowFilterDef` declares
no `column` on either engine, so a column-level request makes every row-filter
policy an ANCESTOR match and yields no filter at all — the corpus would look
green while testing nothing.

**The tag phase, since SP4d-2b Task 5.** 1908 of the 4000 cases carry a
`tagPolicies` block built from egret's own `tag.json` — the recorded-run table's
count, which is the only one this file keeps. 27 axes (26 `tag:` plus the hold-out
counter): a tag the block's
policies REACH and one they cannot; all five match types, admitted (`SELF`,
`SELF_AND_ALL_DESCENDANTS`, `ANCESTOR`) and not (`DESCENDANT`, `NONE` — generated
BECAUSE they are not admitted, since a corpus without them cannot tell that rule
from "admit everything"); the empty resolved set, which is the measured narrowing;
the same tag NAME at two match types, which `RangerTagForEval.equals` keeps apart;
`tag.json`'s `ignoreCase: false`, the one place in this corpus where a pure case
difference is a different value; the deny-capable tag ACCESS policy, the tag MASK
and the tag ROW FILTER; several tag policies at one priority; and the
audit-disabled prune together with its resource-side complement, which the tag
repository performs and no other repository does.

**The enricher is bypassed on BOTH sides, symmetrically**, and that is the design
rather than a shortcut. `egret-ranger-eval` takes the resolved tag set as an INPUT
(`AccessRequest::resolved_tags`) and documents producing a sound one as the
caller's obligation; `EgretCaseRunner` seeds the identical set through
`RangerAccessRequestUtil.setRequestTagsInContext`, which is exactly what
`RangerTagEnricher.enrich` writes. Measured with `javap -c` on both jars: on the
`evaluatePolicies → preProcess` path the only writer of
`RangerAccessRequestUtil.KEY_CONTEXT_TAGS` is `RangerContextEnricher.enrich`, and
no generated def declares one, so nothing overwrites the seeded set. Two
consequences the generator honours: `processServiceTags`' drop of a service
resource no hierarchy admits does not run on the Java side, so no attachment that
would be dropped is ever emitted (that table is measured separately by
`EgretTagServiceResourceValidationTest`), and the tag def is stripped of
`contextEnrichers` for the same reason the resource defs are — Apache's own stock
tag service-def, bundled inside `ranger-plugins-common`, declares
`RangerTagEnricher`, which would try to poll a Ranger admin from the test JVM.

### What every generated tag case holds CONSTANT

Recorded so the next task does not have to rediscover the boundary of what this
corpus can falsify. These are not defects — most are held out because the crate
refuses them, which is the honest reason — but two are simply never varied:

* **exactly one value** in `resources.tag.values` on every generated tag policy.
  A multi-value tag resource is never generated and no axis records that.
  **Not refused**, just absent.
* the tag service-def is **always** egret's own `tag.json`, so `wildCard: true`
  and `ignoreCase: false` are never varied. A crate that hard-coded either would
  pass this corpus. **Not refused**, just absent.
* `attached_through.levels` is always a 3- or 4-level **prefix** of the profile's
  own hierarchy, in order — never a non-prefix subset, never 5+, never fewer than
  3 (fewer refuses; a set no hierarchy admits would be dropped by the enricher on
  one side only, which the harness must not manufacture).
* no generated tag policy carries `validitySchedules`, `conditions`, `zoneName`,
  `options`, `validityPeriods`, `isExcludes`/`isRecursive`, or more than 3
  policies in the block; tag attributes are only ever
  `sensitivity ∈ {high, low}`.
* the block always carries both `serviceName` and `serviceDef` — held out
  deliberately, and follow-up item 3 above.

Two shapes are built DELIBERATELY, because a random draw does not produce them
and **a rate over a population that cannot reach a refusal is not a
measurement**: the COMPOSED eval-order duel (a tag verdict plus a deny-capable
resource band strictly above it — the only shape in which `getEvalOrder()` can
decide an ACCESS outcome), and the CANDIDACY duel (the four-way conjunction
offset 553's refusal needs; 800 random cases produced it zero times). Both have
floors, and the candidacy duel has a second floor requiring that it still
REACHES the refusal — a duel that stopped refusing would mean the shape had
drifted and the rate was no longer a rate of what it says.

The corpus deliberately contains only constructs the evaluator claims to
model. It never generates `validitySchedules`, policy or item `conditions`,
`zoneName`, `securityZones`, `policyDeltas`, `serviceConfig`,
`${{ … }}` expressions, `{{ … }}` row filters, `{OWNER}` with no owner
supplied, the `_any`/`_admin`/`_super_user` pseudo access types, an access
type differing from a policy's only by case, or — on the tag side — a tag
carrying `options` or `validityPeriods`, a tag policy resource the trie and the
matcher are measured to disagree on, a deny-capable tag MASK or ROW FILTER
policy, or a tag mask item with no mask type (that last one crashes the plugin
and is pinned on its own by `check_measured_tag_crash`, at BOTH versions, rather
than scattered through the corpus as uncomparable cases). Each of those is
`Undecidable` by design, and a generated `Undecidable` proves nothing about
agreement.

### The measured version divergences

**1.** SP4d-1 Task 5 measured that an `_any` request against an
ANCESTOR-matching `isDenyAllElse` policy **denies on Ranger 2.5.0** (Trino 470)
and **does not on 2.8.0** (StarRocks 4.1). `_any` is
`UndecidableCause::PseudoAccessType`, so such cases are out of the corpus by
construction. It is written down here rather than generated.

**2.** SP4d-2b Task 5 measured a second one, and this crate acts on it rather
than only recording it: a tag DENY plus a strictly higher-priority resource ALLOW
with no deny items is **Allowed on 2.8.0 and Denied on 2.5.0** — see T5 above for
the five-row table. That one IS generated, and it is the reason
`plugin_version: None` now costs an ACCESS answer in exactly one shape.

## API notes (ranger-plugins-common 2.5.0 / 2.8.0)

One adjustment from the task scaffold is still needed to compile/run against
the published 2.5.0 and 2.8.0 artifacts (the local ranger trunk checkout
differs):

- `new RangerPolicyEngineImpl(sp, null, null)` still NPEs in
  `PolicyEngine.<init>` (null pluginContext) at both 2.5.0 and 2.8.0 —
  confirmed by testing a null `pluginContext` directly against 2.5.0, which
  reproduces the same
  `NullPointerException: Cannot invoke "RangerPluginContext.getAuthContext()"`
  as 2.4.0. The test builds a minimal
  `RangerPluginContext(new RangerPluginConfig("trino","trino_prod","egret",null,null,new RangerPolicyEngineOptions()))`.
  Note `RangerPluginConfig` is in `org.apache.ranger.authorization.hadoop.config`.
- The row-filter request resource stops at `table` (no `column`), since the
  Trino `rowFilterDef` only defines catalog/schema/table.

The 2.4.0-only workaround from earlier versions of this doc no longer
applies and has been removed: `JsonUtils.jsonToObject(Reader, Class)` did
not exist in 2.4.0, but it **does exist** in 2.5.0 and 2.8.0 (verified via
`javap` against both jars) alongside the `(String, Class)` overload this
test still uses. The test's use of the `String` overload is not a
workaround at these versions — either overload would work — so it has been
left as-is rather than churned for no behavioural change.

Verified 2026-09-28, JDK 21 / Maven 3.9.11:

- `mvn clean test` at both `-Dranger.version=2.5.0` (default) and
  `-Dranger.version=2.8.0` → `Tests run: 5, Failures: 0, Errors: 0,
  Skipped: 1` (BUILD SUCCESS). The skip is `EgretCaseRunner`, which is
  `Assume`d out when no `-Degret.cases` is given; the four anchors run and
  pass at both versions.
- `EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential`
  → green, with the numbers in "The differential gate" above.
