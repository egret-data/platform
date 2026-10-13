# Ranger 2.9.0 read against 2.8.0

Date: 2026-10-13
Why: StarRocks `fe-ubuntu:4.1.6` loads `ranger-plugins-common` 2.9.0
(`egret_perm::engine_images`). `egret-ranger-eval` is trusted only for
(engine type, plugin version) pairs it has been read against and proved on, so
this document is the READ half for StarRocks@2.9.0. The PROVED half is the real-jar
differential at 2.9.0 (`just test-ranger-gates`).

## Method

* `mvn dependency:copy` of `org.apache.ranger:ranger-plugins-common` 2.8.0 and 2.9.0
  from Maven Central.
* Every class of both jars (487 in 2.8.0, 458 in 2.9.0) disassembled with
  `javap -c -p -constants`, constant-pool indices (`#n`) and bytecode offsets'
  leading numbering stripped, whitespace collapsed, `commons.lang` / `commons.lang3`
  folded (neither jar references the old package any more; the fold changes
  nothing), and compared class by class.
* Two comparisons, because "same disassembly" is weaker than "same bytes": the raw
  `.class` files with `cmp`, and the normalised disassembly.
* 2.9.0 moved 28 utility classes out of `ranger-plugins-common` into the new
  `ranger-common-utils` jar (a Maven dependency of 2.9.0, so it is on the plugin's
  classpath). Those were located there and compared against their 2.8.0 originals.
  Four anonymous classes (`RangerRESTClient$2`, `RangerSslHelper$1`,
  `JsonUtilsV2$2`, `TimedEventUtil$1`) were removed by refactors of REST, SSL, JSON
  and timeout code; none is reachable from a policy decision.

## Result

**No class on any modelled path differs between 2.8.0 and 2.9.0.** Of the 455 classes
present in both jars, 443 have identical disassembly (439 are byte-for-byte identical
`.class` files, 4 differ only in constant-pool layout); 12 differ, listed below, and
none is on the path a Trino or StarRocks access, mask or row-filter check takes. 32
classes of 2.8.0 are absent from the 2.9.0 jar (28 moved to `ranger-common-utils`, 4
anonymous classes removed); 3 classes are new. Differing modelled paths: **0** (the
limit for STOP was more than 3).

Package totals (classes in both jars / raw-byte-identical):

| package | common | byte-identical |
|---|---|---|
| `policyengine` | 48 | 48 |
| `policyevaluator` | 38 | 35 (the three that differ: see table) |
| `resourcematcher` | 41 | 39 |
| `contextenricher` | 18 | 17 |
| `conditionevaluator` | 18 | 18 |
| `policyresourcematcher` | 8 | 8 |
| `model` | 70 | 69 |

## The classes the `PluginVersion` doc names

| class / method | 2.8.0 -> 2.9.0 | consequence |
|---|---|---|
| `RangerPolicyEngineImpl` (evaluation order, `isSuperUser` caller, `evaluatePoliciesNoAudit`, tag phase) | byte-identical | none |
| `PolicyEngine` (tag repository construction, `normalizeAndPrunePolicyItems` caller) | byte-identical | none |
| `RangerPolicyRepository` (`normalizeAndPrunePolicyItems`, `getLikelyMatchPolicyEvaluators`, eval-order sort) | byte-identical | none |
| `RangerDefaultPolicyEvaluator` (`evaluate`, `getMatchingPolicyItem`, `evaluatePolicyItems`, tableswitch) | byte-identical | none |
| `RangerOptimizedPolicyEvaluator` (`getEvalOrder`, `compareTo`) | byte-identical | access evalOrder untouched |
| `RangerAbstractPolicyEvaluator`, `RangerPolicyEvaluator` (`updateAccessResult`) | byte-identical | none |
| `RangerDefaultPolicyResourceMatcher`, `RangerAbstractResourceMatcher`, `RangerDefaultResourceMatcher`, `RangerURLResourceMatcher` | byte-identical | none |
| `RangerResourceTrie` and its inner classes (`getEvaluatorsForResource`, candidacy) | byte-identical | none |
| `RangerDefaultDataMaskPolicyItemEvaluator`, `RangerDefaultRowFilterPolicyItemEvaluator`, `updateAccessResult` | byte-identical | none |
| `RangerDefaultPolicyItemEvaluator`, `RangerAbstractPolicyItemEvaluator` (implied grants, principal match) | byte-identical | none |
| `RangerTagEnricher` and service-resource matching / match types | byte-identical | none |
| `RangerAccessResult` (arbitration by `policyPriority`) | byte-identical | none |
| `RangerAccessRequestUtil` (`accessTypeResults`) | byte-identical | none |
| `isSuperUser` (`RangerPolicyEngineImpl`, `RangerPluginConfig`) | `RangerPolicyEngineImpl` byte-identical; `RangerPluginConfig` byte-identical | none |
| `RangerRequestExprResolver.PATTERN` | byte-identical | none |
| `JavaScriptEdits` | MOVED to `ranger-common-utils`; constructor `public` -> `private`; the two `LOG.isDebugEnabled()` guards dropped around `LOG.debug` calls. Regex `DOUBLE_BRACKET_PATTERN`, `hasDoubleBrackets` and the replace logic unchanged | none; debug-log guard only. Double-bracket row filters are refused by egret anyway (`RowFilterDoubleBrackets`) |

## Every other difference found, and why it is off the modelled paths

Classes whose normalised disassembly differs, among those present in both jars (12):

| class | difference | on a modelled path? |
|---|---|---|
| `RangerRESTClient`, `RangerRESTClient$1`, `RangerSslHelper`, `RangerRESTUtils`, `BaseClient`, `RangerAdminRESTClient` | hostname verifier extracted to `RangerDefaultHostnameVerifier`, REST helpers reshaped | no: policy download transport, egret is the Admin |
| `RangerUserStoreRefresher` | reads the response body as a `String` then `JsonUtilsV2.jsonToObj` instead of `readResponse` | no: user-store download, not evaluation |
| `RoleStore` | two new `default` methods `createRole` / `updateRole` with an extra `Boolean` | no: admin-side role CRUD |
| `RangerInlinePolicy$Grant`, `RangerInlinePolicyEvaluator$GrantEvaluator` | new `actions` set on an inline grant | no: inline policies are a new feature egret never emits; the evaluator is not created for resource / tag / mask / row-filter policies |
| `RangerPathResourceMatcher$RecursiveWildcardResourceMatcher` | local-variable reshuffle | no: path matcher, HDFS-shaped services only (neither Trino nor StarRocks use `RangerPathResourceMatcher`) |
| `GraalScriptEngineCreator` | rewritten to reach GraalVM reflectively, so it no longer needs `hadoop-common` `Configuration` | no: builds a JavaScript engine for script conditions / `${{ }}` expressions, which egret refuses (`RowFilterExpression`, `RowFilterDoubleBrackets`, custom conditions) before they reach an engine. `ScriptEngineUtil`, `RangerScriptConditionEvaluator` and `RangerRequestExprResolver` are byte-identical |

Present in 2.9.0 only: `RangerActionMatcher`, `RangerActionListMatcher` (an action-list
condition evaluator referenced only by the new inline-grant evaluator and by name from
a policy condition type egret never writes) and `RangerDefaultHostnameVerifier`.

Moved to `ranger-common-utils` and identical: `AutoClosableLock` family,
`RangerCache$KeyNotFoundException`, `$RefreshMode`, `$ValueLoader`,
`RangerPerfCollectorTracer`, `RangerReadWriteLock$1`, `$RangerLock`.

Moved to `ranger-common-utils` and changed in ways that cannot affect a decision:
`RangerPerfTracer` / `RangerPerfTracerFactory` / `PerfDataRecorder` (performance
tracing), `RangerCache` and its inner classes (method order, synthetic accessor
numbering, `access$N` renumbering; a refresh cache used by role / user-store
providers), `RangerReadWriteLock` (`StringBuilder.append(Object)` in a log string),
`TimedEventUtil` (anonymous class -> lambda), `URLEncoderUtil`, `XMLUtils`,
`JsonUtilsV2` (response reading).

## Decision (plan Step 2)

* Paths that differ: **none**. No new `match` arm needs a different behaviour and no
  `UndecidableCause` refusal is added for 2.9.0.
* The one version-conditional in the crate (`TagDenyReopenedDiffersByPluginVersion`)
  keys on the access-phase restore at `RangerDefaultPolicyEvaluator.evaluate`, which is
  byte-identical, so 2.9.0 takes the 2.8.0 arm. It is written as an exhaustive `match`
  so that no future version inherits an arm.
* This is a READING. Byte-identity of the code egret reproduces does not prove egret's
  model of it: the proof is the real-jar differential at 2.9.0, recorded in
  `.superpowers/sdd/2026-10-13-engine-upgrade/task-4-report.md`.
