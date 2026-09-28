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

Recorded run, re-recorded 2026-09-29 (JDK 21.0.11 / Maven 3.9.11):

| | |
|---|---|
| Seed | `0x54d000007` (`EGRET_DIFF_SEED`, hex) |
| Cases | **2000 per engine — 4000 total** (`EGRET_DIFF_CASES`) |
| Versions | **2.5.0** against the Trino service-def, **2.8.0** against the StarRocks one |
| Compared and agreeing | **3856** |
| `Undecidable` other than `EvalOrderAmbiguous` | **0** (asserted) |
| `EvalOrderAmbiguous` | 144 of 4000 — **3.6%** |
| Deliberate eval-order duels | **489, all agreeing** |
| Wall time | ~37s including three Maven runs |

A wider sweep at 5000 per engine (10000 cases, 64.6s) was also re-run green
on the same day: **9595 compared and agreeing, 405 `EvalOrderAmbiguous`
(4.0%), 1273 duels all agreeing.** The rate is stable across an order of
magnitude of corpus size.

These are the numbers AFTER the findings below were fixed and their
exclusions removed. Whether the corpus still REACHES the shapes those fixes
were written for is not a matter of opinion here — see the coverage table
below, which is printed on every run and fails it when an axis thins out.

`EvalOrderAmbiguous` is Task 6's refusal to guess a mask (or row filter) when
two policies tie inside one `(policyPriority, hasDeny)` band — the arbitration
there is `>=`, not `>`, so the first evaluator in `getEvalOrder()` is final,
and this crate does not reproduce `computeEvalOrder()`. Those cases are
counted and excluded from the comparison, because this crate has no answer to
compare; every other `Undecidable` fails the run, so the generator cannot
drift into constructs the evaluator declines to answer while the gate reports
green.

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

**1281 duels across the 10000-case sweep, all agreeing.** Task 5's claim
survives a test that could have killed it, and the `hasDeny` key in the
comparator is why: it puts the denying policy first, so the engine loop's
`if (ret.getIsAllowed()) { if (!evaluator.hasDeny()) break; }` cannot break
past a same-priority deny.

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

The corpus deliberately contains only constructs the evaluator claims to
model. It never generates `validitySchedules`, policy or item `conditions`,
`zoneName`, `securityZones`, `tagPolicies`, `policyDeltas`, `serviceConfig`,
`${{ … }}` expressions, `{{ … }}` row filters, `{OWNER}` with no owner
supplied, the `_any`/`_admin`/`_super_user` pseudo access types, or an access
type differing from a policy's only by case — each of those is `Undecidable`
by design, and a generated `Undecidable` proves nothing about agreement.
`contextEnrichers` are stripped from the embedded service-def, matching the
real captured download in `fixtures/service-policies.json`; the only one either
def declares is a tag enricher that would poll a Ranger admin from the test JVM.

### The one measured version divergence, recorded rather than tested

Task 5 measured that an `_any` request against an ANCESTOR-matching
`isDenyAllElse` policy **denies on Ranger 2.5.0** (Trino 470) and **does not on
2.8.0** (StarRocks 4.1). `_any` is `UndecidableCause::PseudoAccessType`, so
such cases are out of the corpus by construction. It is written down here
rather than generated.

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
