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
| Cases carrying a `tagPolicies` block | **1755 of 4000** |
| Compared and agreeing | **3490** |
| `Undecidable` outside the five declared refusals | **0** (asserted) |
| Deliberate eval-order duels | **634**, of which 260 composed |
| Wall time | ~48s including five Maven runs |

### The five measured refusal rates

Each is a rate over the population it BELONGS to, and the split matters: the
resource path and the composed path are different questions, and the two
deliberately-built shapes have denominators of their own because a random draw
does not produce them at all.

| refusal | resource path | composed (tag block present) |
|---|---|---|
| `EvalOrderAmbiguous` | **95 / 2245 = 4.23%** | **114 / 1755 = 6.50%** |
| `ResourceCandidacyNotReproducible` | 0 / 2245 | 253 / 1755 = 14.42% — **read the note below, this figure is a composition of two rates** |
| `TagSubResultSplitByResourcePolicy` | 0 / 2245 | 19 / 1755 = 1.08% |
| `TagPriorityArbitrationNotModelled` | 0 / 2245 | 29 / 1755 = 1.65% |
| `TagDenyReopenedDiffersByPluginVersion` | 0 / 2245 | 0 / 1755 (1 / 364 at a 800-case run) |

**The `ResourceCandidacyNotReproducible` number needs its denominator read
carefully, and this is the one place a percentage would mislead.** All 253 of
them are deliberate CANDIDACY DUELS: `candidacy_duel` builds the four-way
conjunction offset 553's refusal needs, 264 of them exist in this corpus, and
**253 of those 264 refuse (95.8%) while the refusal fires ZERO times outside
them.** So the honest statement is two numbers, not one: a download that does
not carry the shape never reaches this refusal, and one that does reaches it
almost always. `explain` over a live plugin's download is the population that
carries it — `scope_to_resources`' `Mask` arm does not fill unnamed levels with
`*`, and an ordinary table-level grant from a download egret did not author has
the same shape — so the rate to plan against is the second one.

The two `EvalOrderAmbiguous` rates are also genuinely different questions.
Task 2 established the refusal cannot arise in the tag phase alone (both tag
comparators are fully reproduced). Task 3 found it arises once the phases
compose on ACCESS: `policyPriority` is live, so under a tag deny an allowing
resource policy raises offset 469's left-hand side and shuts out the rest of
its own band, and `getEvalOrder()` then picks the winner. The resource-path
rate is unchanged from the pre-tag corpus (3.4-4.0% historically, 4.23% here);
the composed rate is 6.50% and had never been measured because no corpus
generated a tag block.

These are the numbers AFTER the findings below were fixed and their
exclusions removed. Whether the corpus still REACHES the shapes those fixes
were written for is not a matter of opinion here — see the coverage table
below, which is printed on every run and fails it when an axis thins out.

**Exactly five refusals are counted and excluded from the comparison**, and the
line they sit on either side of is deliberate: each of the five is about an ORDER,
a CANDIDACY or a VERSION this crate does not reproduce, never about a construct it
does not model. A construct refusal would mean the generator had drifted into
shapes the evaluator declines to answer, which is the silent-green failure this
whole file exists to prevent — so anything outside the five still fails the run.
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
(`TagPriorityArbitrationNotModelled`). It is not modelled because the five-armed
branch structure at offsets 366-573 could not be read reliably from a
disassembly, and a rule fitted to thirteen rows without its mechanism is a guess
— in the allow/deny path a guess is a security defect. `egret-perm-compile` never
emits `policyPriority` on a tag policy and the live captured download carries 0
on all three, so a single-priority block cannot reach it.

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
promises the 2.8.0 behaviour and does not deliver it. The equal-priority row and
the deny-capable row agree across versions, which is what makes this specific
rather than general. This crate reproduces 2.8.0 and refuses on 2.5.0 and on
`plugin_version: None` (`TagDenyReopenedDiffersByPluginVersion`) — the one place
in the crate where an unstated plugin version costs an ACCESS answer, and it costs
it because the engines genuinely disagree.

### Follow-up work this left behind, with the rows to aim at

Named here rather than in a report nobody re-reads:

1. **Model `normalizeAndPrunePolicyItems`' prefix strip** instead of refusing
   `TagNormalisationProblem::Namespaced{AccessType,MaskType}`. The rule is fully
   measured (see T1); what is not done is the two prunes it feeds and their
   interaction with `hasDeny()` and the tag comparators.
2. **Model the `accessTypeResults` arbitration** (T4 and T5 are the same
   mechanism seen twice). It is live on the RESOURCE path too, where this crate
   models a different mechanism and is green over 4000 cases — the two coincide
   because the engine loop shuts out a lower-priority evaluator before it reaches
   the map, but that is an observation, not a proof, and the audit belongs in
   whatever task models this.
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

- **The reported policy id of an ACCESS decision can differ.** Within one
  `(policyPriority, hasDeny)` band Ranger picks by `getEvalOrder()` and this
  crate by a name-then-id tie-break, so the two can agree on allow/deny while
  naming different policies. Measured at the recorded seed: **39 of 1972
  compared ACCESS cases, of which 31 are DENIES** — not, as an earlier comment
  in `eval.rs` claimed, only allows. The allow/deny OUTCOME is unaffected (see
  the duels below); nothing in this crate asserts a reported ACCESS policy id,
  which is why the gate is green in spite of it.

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

  The allow/deny OUTCOME is identical — only the reported id differs, so this
  sits inside the ACCESS reported-id divergence above rather than adding a new
  class of disagreement, and the same "nothing asserts a reported ACCESS policy
  id" argument keeps the gate green. The reason for the choice is that Ranger's
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

**634 duels in the recorded 4000-case corpus, 552 agreeing and the rest
REFUSED — none disagreeing.** Task 5's claim survives a test that could have
killed it, and the `hasDeny` key in the comparator is why: it puts the denying
policy first, so the engine loop's
`if (ret.getIsAllowed()) { if (!evaluator.hasDeny()) break; }` cannot break
past a same-priority deny.

**260 of those duels are COMPOSED** (SP4d-2b Task 5): the same two resource
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

**The tag phase, since SP4d-2b Task 5.** 1755 of the 4000 cases carry a
`tagPolicies` block built from egret's own `tag.json`. 24 axes: a tag the block's
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
