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
harness and produced **`Tests run: 4, Failures: 0, Errors: 0`** with no
assertion differences between them — Ranger's behaviour for these four cases
(allow, deny, mask, row filter) has not changed between 2.5.0 and 2.8.0.

## The differential gate

`crates/egret-ranger-eval/tests/differential.rs` generates a seeded corpus,
writes it as `cases.json`, has `EgretCaseRunner` answer every case with a real
`RangerPolicyEngineImpl`, reads `verdicts.json` back and asserts the Rust
`Decision` matches. Paths travel as `-Degret.cases` / `-Degret.verdicts`
system properties, so nothing is bound to a working directory.

Recorded run, 2026-09-28 (JDK 21 / Maven 3.9.11):

| | |
|---|---|
| Seed | `0x54d000007` (`EGRET_DIFF_SEED`, hex) |
| Cases | **2000 per engine — 4000 total** (`EGRET_DIFF_CASES`) |
| Versions | **2.5.0** against the Trino service-def, **2.8.0** against the StarRocks one |
| Compared and agreeing | 3812 |
| `Undecidable` other than `EvalOrderAmbiguous` | **0** (asserted) |
| `EvalOrderAmbiguous` | 188 of 4000 — **4.7%** |
| Wall time | ~37s including three Maven runs |

A wider sweep at 5000 per engine (10000 cases, ~63s) was also run green and
found nothing the 4000-case corpus had not; the `EvalOrderAmbiguous` rate held
at 4.6%, so the figure is stable across an order of magnitude of corpus size.

`EvalOrderAmbiguous` is Task 6's refusal to guess a mask (or row filter) when
two policies tie inside one `(policyPriority, hasDeny)` band and would yield
different results — the arbitration there is `>=`, not `>`, so the first
evaluator in `getEvalOrder()` is final, and this crate does not reproduce
`computeEvalOrder()`. **4.7% is the measured cost of that refusal.** Those
cases are counted and excluded from the comparison, because this crate has no
answer to compare; every other `Undecidable` fails the run, so the generator
cannot drift into constructs the evaluator declines to answer while the gate
reports green.

### The gate has been observed failing

A gate never seen failing is not known to be a gate. With
`MatchType::Ancestor` added to the "matched" test in
`crates/egret-ranger-eval/src/eval.rs` — i.e. making an ANCESTOR match an
allow, the fail-open direction — the differential caught it at only 300 cases
per engine: **7 of 284 compared cases disagreed** at ranger 2.5.0. The file was
then restored from a backup and verified byte-identical by `shasum`, not by
eye (`cp` is aliased to `cp -i` on this machine and silently refuses).

### Two divergences the gate FOUND, and what the corpus therefore excludes

The first run of the corpus disagreed with the real engine on 3 of 1200 cases.
Both mechanisms were read out of the 2.5.0 bytecode. Neither is fixed here —
this task builds the gate, and adjusting an evaluator to make its own gate pass
is how a misreading survives — so both constructs are excluded from the
generated corpus and kept under test as minimal reproducers
(`pinned_divergences`) that assert the **disagreement**. The day either defect
is fixed, the pin fails and says to widen the corpus again.

**F1 — `*` immediately followed by `?` (fail-open).** Task 4 reimplemented
`FilenameUtils.wildcardMatch` rather than binding to it, and said the two had
never been compared. They disagree. `splitOnTokens` turns `?*?` into
`["?", "*", "?"]`, and a `*` token advances the text index only when it is the
LAST token (`textIdx = filename.length()`) or when the next token is a literal
(via `checkIndexOf`). A `*` followed by `?` therefore consumes nothing:

```text
FilenameUtils.wildcardMatch("HIVE", "?*?", INSENSITIVE) == false   (egret: true)
FilenameUtils.wildcardMatch("HIVE", "*?",  INSENSITIVE) == false   (egret: true)
FilenameUtils.wildcardMatch("ab",   "?*?", INSENSITIVE) == true
```

egret's matcher is a classic greedy matcher with backtracking and says true for
all three. Reproducer `pin-f1-wildcard`: a policy allowing `catalog=?*?` grants
a `HIVE` request that the live plugin refuses.

**F2 — a `dataMaskPolicyItems` entry with no `dataMaskInfo`.**
`RangerPolicy$RangerDataMaskPolicyItem`'s no-arg constructor (the one Gson
uses) delegates to the all-args one, and `setDataMaskInfo(null)` substitutes
`new RangerPolicyItemDataMaskInfo()`. So `getDataMaskInfo()` is **never null**
on a deserialised item, the `ifnull` guard at offset 8 of
`RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` never fires, and
the item runs the whole body — `setMaskType(null)` and then
`policyEvaluator.updateAccessResult(result, matchType, true, …)`, which sets
`policyId`, `policyPriority` and `isAllowed`. The item DETERMINES the result
with a null mask type and the search stops. `egret-ranger-eval` reads the
absent key as `MaskInfo: None` and models it as "reports nothing, keep
looking", so it goes on to a lower-priority policy and reports a mask Ranger
never reaches. Reproducer `pin-f2-missing-datamaskinfo`.

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
