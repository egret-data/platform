# SP4d-1 — The evaluator and its oracle

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** a pure Rust evaluator that reproduces the access decision the Ranger
plugin makes over a served policy set, proven against the real Ranger engine at
the version that actually enforces, and against a real Trino.

**Architecture:** a new pure crate `egret-ranger-eval` (no DB, no I/O), mirroring
the existing `egret-perm` / `egret-perm-compile` split. It takes a typed policy
set, a service-def, and a request; it returns `Allowed | Denied | Undecidable`.
Glue that reads Postgres stays out of it. `explain` is **not** wired up in this
package — that is SP4d-2.

**Tech stack:** Rust (serde, no sqlx in the new crate), the existing Maven
harness under `tests/ranger/` for the differential oracle, kind + Trino 470 for
the live gate.

**Spec:** `docs/en/specs/2026-09-28-ranger-evaluator-sp4d-design.md`

## Global Constraints

- The new crate is **pure**: no sqlx, no tokio, no network, no filesystem in
  library code. Same discipline as `crates/egret-perm` (see its module doc at
  `crates/egret-perm/src/eval.rs:1-64`).
- The result type is **three-valued**. `Undecidable { why }` is a first-class
  outcome. Never return `Denied` for "I could not model this".
- **Refuse, never approximate.** Meeting an unmodelled construct produces
  `Undecidable` naming the construct — not a partial match.
- A policy naming fewer hierarchy levels than the request matches as `ANCESTOR`
  and **must not** produce an allow. This is measured live
  (`crates/egret-perm-compile/src/grants.rs:227-254`,
  `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3 T1). A
  naive "ancestor covers descendants" port is wrong in the fail-open direction.
- Case sensitivity comes from the service-def `matcherOptions.ignoreCase`, never
  hardcoded. `crates/egret-perm/src/eval.rs:355-395`'s `glob_match` is
  case-sensitive and must not be reused unchanged.
- Never run tests against the dev DB `egret`; use `egret_test`.
  `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`,
  `env -u EGRET_ALLOW_DB_SKIP`, `RUST_TEST_THREADS=4`.
- Never use `git stash`. Never override the git author.
- Long test suites are auto-backgrounded past ~120s. Commit **before** running
  one; a completion notification is a finished result, not a signal to wait.
- Do not modify the kind `demo-trino` engine's data.
- **The two engines run different Ranger plugin versions.** Measured
  2026-09-28 (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`):
  Trino 470 loads `ranger-plugins-common-2.5.0`, StarRocks fe-ubuntu 4.1 loads
  `2.8.0`. Plugin version is therefore an **input** to the evaluator alongside
  engine type, and any gate that certifies only one version certifies only one
  engine. All four `ranger-plugins-common` jars are already in `~/.m2`, but the
  2.8.0 build additionally resolves `javax.servlet:javax.servlet-api:4.0.0`,
  which a cold `~/.m2` does not have — so a **first** 2.8.0 run needs network.
  Once warmed, `mvn -o test -Dranger.version=2.8.0` runs fully offline
  (measured 2026-09-28: exit 0, `Tests run: 4, Failures: 0`).

---

### Task 1: Measure which Ranger is actually enforcing

**Files:**
- Create: `docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`

**Interfaces:**
- Produces: the plugin version string(s) every later task pins against.

This task exists because the spec's premise was wrong twice under measurement.
Do not skip it and do not infer a version from an image tag.

- [ ] **Step 1: Re-confirm Trino.** On the running provisioned coordinator:

```bash
command kubectl --context kind-egret exec -n egret-engines <trino-coordinator-pod> -- \
  sh -c 'ls /usr/lib/trino/plugin/apache-ranger/'
```

Expected (measured 2026-09-28 on `trinodb/trino:470`):
`org.apache.ranger_ranger-plugins-common-2.5.0.jar`. Note the pod has no
`find`; use `ls`.

- [ ] **Step 2: Measure StarRocks.** The provisioned FE image is
`starrocks/fe-ubuntu:4.1-latest` (`crates/egret-engine-mgmt/src/engines.rs:59-60`),
which is **not** the `allin1-ubuntu` image. If a StarRocks engine is running,
exec into its FE. Otherwise inspect the image directly:

```bash
docker run --rm --entrypoint sh starrocks/fe-ubuntu:4.1-latest \
  -c 'ls /opt/starrocks/fe/lib/ 2>/dev/null | grep -i ranger || ls /data/deploy/starrocks/fe/lib/ | grep -i ranger'
```

Check free disk first (`df -h /`); the image is several GB. If the pull is not
affordable, record **NOT MEASURED** explicitly and say so in the result doc —
do not guess from `allin1-ubuntu`.

- [ ] **Step 3: Check for super-users.** Ranger short-circuits on
`ranger.plugin.super.users` before any policy is consulted, and egret cannot see
it. Read the provisioned engine's Ranger config (for Trino,
`/etc/trino/ranger-trino-security.xml`) and record whether the key is set.

- [ ] **Step 4: Write the result doc.** Record each measurement with the exact
command and its verbatim output, and label anything unmeasured as NOT MEASURED.
State plainly which version the harness will be pinned to in Task 2.

- [ ] **Step 5: Commit**

```bash
git add docs/en/plans/2026-09-28-sp4d-plugin-version-result.md
git commit -m "docs(sp4d): measure which Ranger version actually enforces"
```

---

### Task 2: Retarget the harness to the enforcing versions

**Files:**
- Modify: `tests/ranger/pom.xml`
- Modify: `tests/ranger/README.md`
- Modify: `tests/ranger/src/test/java/EgretRangerEnforcementTest.java` (only if
  the version bump breaks compilation)

**Interfaces:**
- Consumes: the version from Task 1.
- Produces: a green harness at the enforcing version, the baseline Task 7 extends.

- [ ] **Step 1: Make the version a parameter, not a constant.**
`tests/ranger/pom.xml:6` currently pins `2.4.0`, which is neither engine's
version. Replace the literal with a property, e.g.
`<ranger.version>2.5.0</ranger.version>`, overridable as
`mvn test -Dranger.version=2.8.0`.

Default to **2.5.0** (Trino, the engine this package's live gate uses). The
harness must be runnable at **2.8.0** too, because that is what StarRocks
enforces. All of 2.4.0, 2.5.0, 2.7.0 and 2.8.0 are already in
`~/.m2/repository/org/apache/ranger/ranger-plugins-common/`, so this needs no
network.

- [ ] **Step 2: Run the existing four tests.**

```bash
cd tests/ranger && mvn -q test
cd tests/ranger && mvn -q test -Dranger.version=2.8.0
```

Expected at **both** versions: `Tests run: 4, Failures: 0, Errors: 0`.

If the two versions disagree on any of the four cases, that is a finding worth
recording, not a test to loosen: it means Ranger's behaviour changed between
the versions the two engines run, and the evaluator must model the difference.

The README documents two 2.4.0-specific workarounds that may no longer apply at
the new version: `JsonUtils.jsonToObject(Reader, Class)` did not exist, and
`new RangerPolicyEngineImpl(sp, null, null)` NPE'd on a null `pluginContext`.
If the bump changes either, fix the Java and update the README's API-notes
section — do not leave a note describing a quirk that no longer exists.

- [ ] **Step 3: Fix the README's run command.** It currently reads
`cargo test -p egret-ranger ranger_engine_gate`. **No crate named `egret-ranger`
exists**; the gate is `crates/egret-policy/tests/ranger_engine_gate.rs`. Anyone
following the README gets "package not found" and may conclude the gate is
absent. Correct it to `-p egret-policy`, and verify by running it:

```bash
EGRET_TEST_RANGER_E2E=1 cargo test -p egret-policy --test ranger_engine_gate
```

- [ ] **Step 4: Update the version string** recorded at the end of
`tests/ranger/README.md` ("ranger-plugins-common version used: …") and the
"Requires:" line, and state **why** that version — because it is what the
provisioned engine enforces, citing the Task 1 result doc.

- [ ] **Step 5: Commit**

```bash
git add tests/ranger
git commit -m "test(sp4d): certify against the Ranger version that actually enforces"
```

---

### Task 3: A typed reader for Ranger policy bodies

**Files:**
- Create: `crates/egret-ranger-eval/Cargo.toml`
- Create: `crates/egret-ranger-eval/src/lib.rs`
- Create: `crates/egret-ranger-eval/src/model.rs`
- Test: `crates/egret-ranger-eval/tests/model.rs`
- Modify: the workspace `Cargo.toml` members list

**Interfaces:**
- Produces: `RangerPolicy`, `ResourceLevel`, `PolicyItem`, `ServicePolicySet`,
  and `Unmodelled` — the enum of constructs that force `Undecidable`.

Today nothing models a Ranger policy body for *reading*: `PolicyRow.policy_jsonb`
is an untyped `serde_json::Value` (`crates/egret-policy/src/store.rs:935-943`)
and the only typed shapes are the *writer*'s
(`crates/egret-policy/src/admin/ranger_json.rs`), which the module doc at
`:8-15` says is lossy for policies egret did not author.

- [ ] **Step 1: Write the failing test.** A test that parses a real policy body
and one that parses a body carrying an unmodelled construct. Use
`tests/ranger/fixtures/service-policies.json` as the realistic input rather than
inventing a shape — it is a captured download.

Assert: a body with `validitySchedules` (or `policyConditions`, or `zoneName`)
parses **successfully** and reports the construct in `Unmodelled`, rather than
failing to parse or silently ignoring it. Silently ignoring is the defect this
task exists to prevent.

- [ ] **Step 2: Run it, confirm it fails** (crate does not exist yet).

- [ ] **Step 3: Implement the model.** Deserialize with `#[serde(default)]` and
tolerate unknown fields, but **detect** the ones that change the decision. At
minimum, from the spec's D2/D4 lists: `validitySchedules`, `policyConditions`,
`conditions` on an item, `zoneName`/`securityZones`, a mask value or row-filter
expression containing an expression marker, `isAllowed: false` on an access
entry, and `delegateAdmin: true`.

Fields that must round-trip because they decide outcomes: `policyType`,
`isEnabled`, `policyPriority`, `isDenyAllElse`, `resources` (with `values`,
`isExcludes`, `isRecursive`), `additionalResources`, `policyItems`,
`denyPolicyItems`, `allowExceptions`, `denyExceptions`,
`dataMaskPolicyItems`, `rowFilterPolicyItems`, and per item `users`, `groups`,
`roles`, `accesses[].type`, `accesses[].isAllowed`.

Verify each field name against `crates/egret-policy/src/admin/ranger_json.rs`
and the captured fixture before writing it — do not transcribe this list blind.

- [ ] **Step 4: Run tests, confirm green.**

- [ ] **Step 5: Commit**

---

### Task 4: Resource matching and MatchType

**Files:**
- Create: `crates/egret-ranger-eval/src/resource.rs`
- Create: `crates/egret-ranger-eval/src/servicedef.rs`
- Modify: `crates/egret-policy/src/default_policies.rs` (make the hierarchy
  helpers reachable)
- Test: `crates/egret-ranger-eval/tests/resource.rs`

**Interfaces:**
- Consumes: `model.rs` from Task 3.
- Produces: `MatchType { None, Self_, Descendant, Ancestor, SelfAndAllDescendants }`
  and `fn match_resource(policy_resources, request_resource, service_def) -> MatchType`.

- [ ] **Step 1: Write the failing tests.** The decisive ones, each asserting a
behaviour the spec measured rather than a shape:

1. A policy naming `catalog` + `schema` against a request naming
   `catalog/schema/table/column` yields `Ancestor` — and (Task 5) must not
   allow. This is the live-measured T1 case.
2. A policy naming all four levels with `column: ["*"]` against the same
   request yields `Self_` or `SelfAndAllDescendants`.
3. `ignoreCase: true` from the service-def makes `HIVE` match `hive`.
4. A glob value `ord*` matches `orders` — not only the literal `*`.
5. `isExcludes: true` flips a match, **except** in the
   `allValuesRequested && !isMatchAny` carve-out, which does not flip.
6. `additionalResources` gives a second independent hierarchy a chance to match.

- [ ] **Step 2: Run them, confirm they fail.**

- [ ] **Step 3: Lift the hierarchy walker.** `ResourceDef`, `parse_resources`,
`is_hierarchy_leaf` and `hierarchy_chain` already exist at
`crates/egret-policy/src/default_policies.rs:20-90` and are private. Lift them
into `egret-ranger-eval::servicedef` and have `default_policies.rs` use the
lifted copy. Do **not** reimplement — a second hierarchy walker that disagrees
with the seeder is a defect in waiting.

Watch the dependency direction: `egret-ranger-eval` must not depend on
`egret-policy`. If the lift would invert the direction, move the helpers into
the new crate and have `egret-policy` depend on it.

- [ ] **Step 4: Implement matching.** Wildcards and case folding come from each
resource's `matcherOptions` in the service-def
(`crates/egret-policy/src/service-defs/{trino,starrocks}.json`), which declare
`{wildCard: true, ignoreCase: true}`. `crates/egret-perm/src/eval.rs:355-395`
has working `*`/`?` backtracking to adapt — it is case-sensitive, so case
folding must be added, driven by the def.

- [ ] **Step 5: Run tests, confirm green. Mutation-check the excludes carve-out**
by inverting it and confirming a test dies for the right reason.

- [ ] **Step 6: Commit**

---

### Task 5: Item evaluation, precedence, and the three-valued result

**Files:**
- Create: `crates/egret-ranger-eval/src/eval.rs`
- Test: `crates/egret-ranger-eval/tests/eval.rs`

**Interfaces:**
- Consumes: Tasks 3 and 4.
- Produces: `Decision { Allowed { policy_id, .. } | Denied { policy_id, .. } | Undecidable { why } }`
  and `fn evaluate(policies, request, service_def, engine) -> Decision`.

- [ ] **Step 1: Write the failing tests**, one per rule the spec names:

1. Bucket order: `denyPolicyItems → denyExceptions → allowPolicyItems →
   allowExceptions`.
2. An exception **vetoes a match**; a cancelled deny does **not** become an
   allow, it falls through to the allow bucket.
3. `isDenyAllElse: true` denies when nothing matched — and only for
   `policyType 0`.
4. `policyPriority` override beats a normal policy.
5. `impliedGrants` expands `all` **one round only**.
6. Principal order users → groups → roles → owner, short-circuiting.
7. `public` in `groups` matches a user with **no** groups.
8. `{USER}` in `users` matches any user.
9. An ANCESTOR match does not allow (pairs with Task 4 test 1).
10. Nothing matched at the end ⇒ `Denied`, not `Undecidable`.
11. A policy carrying an unmodelled construct ⇒ `Undecidable` naming it, even
    when another policy would have allowed. Rationale: egret cannot know the
    construct did not change the outcome.

- [ ] **Step 2: Run them, confirm they fail.**

- [ ] **Step 3: Implement.** Keep `evaluate` pure and I/O-free.

- [ ] **Step 4: Run tests, confirm green.**

- [ ] **Step 5: Mutation-check the precedence.** Swap the deny and allow bucket
order and confirm a test dies; make exceptions additive instead of vetoing and
confirm a test dies. A mutation that **survives** here means an assertion is
missing, not that the code is safe.

- [ ] **Step 6: Commit**

---

### Task 6: Mask, row filter, and engine parameterisation

**Files:**
- Modify: `crates/egret-ranger-eval/src/eval.rs`
- Test: `crates/egret-ranger-eval/tests/mask_filter.rs`

- [ ] **Step 1: Write the failing tests.**

1. `policyType 1` returns the mask type as **data** (`MASK_HASH`), not a verdict.
2. `policyType 2` returns the filter expression as data.
3. Mask and row-filter policies have **no deny bucket and no exceptions** — a
   `denyPolicyItems` on a mask policy is ignored by Ranger; assert egret agrees.
4. A mask value or filter expression containing an expression marker ⇒
   `Undecidable`, because `RangerRequestExprResolver` resolves it at request
   time against context egret does not have.
5. Engine parameterisation: the same policy set and request evaluated as
   `Trino` vs `StarRocks` may differ, because the service-defs differ (`column`
   is a valid leaf in `starrocks.json`, not in `trino.json`), StarRocks' plugin
   has no tag policies at all, and the two run different Ranger versions
   (2.5.0 vs 2.8.0). Plugin version is an explicit input, not a constant.

- [ ] **Step 2: Run, confirm fail. Step 3: Implement. Step 4: Confirm green.**

- [ ] **Step 5: Commit**

---

### Task 7: Close Gap A — differential testing at scale

**Files:**
- Modify: `tests/ranger/src/test/java/EgretRangerEnforcementTest.java`
- Create: `tests/ranger/src/test/java/EgretCaseRunner.java`
- Create: `crates/egret-ranger-eval/tests/differential.rs`
- Modify: `crates/egret-policy/tests/ranger_engine_gate.rs`
- Modify: `tests/ranger/README.md`

**Interfaces:**
- Consumes: the whole evaluator.
- Produces: the gate that proves Rust agrees with the real Ranger engine.

The existing harness hard-codes four cases against one fixture. Turn it into a
case runner so generated cases can flow through it.

- [ ] **Step 1: Define the protocol.** Rust writes `cases.json`: an array of
`{ id, servicePolicies, request: { user, groups, roles, resource, accessType, policyType } }`.
Java reads it, evaluates each with `RangerPolicyEngineImpl`, and writes
`verdicts.json`: `{ id, isAllowed, maskType, filterExpr }`. Paths are passed as
system properties (`-Degret.cases=… -Degret.verdicts=…`) so nothing is
hardcoded to a working directory.

**Keep the four existing hand-written tests.** They are a regression anchor with
known-good expected values; the generated cases assert agreement, which is a
weaker claim on its own — two implementations can agree and both be wrong.

- [ ] **Step 2: Write the generator** in `differential.rs`. Deterministic, seeded,
so a failure is reproducible from its seed. Generate across the axes the live
gate cannot cover: hierarchy depth (including the ANCESTOR case), `isExcludes`,
multi-value levels, `additionalResources`, priority/override, all four buckets
with overlapping principals, `isDenyAllElse`, `impliedGrants`, `public`,
`{USER}`, and all three policy types.

**Wildcard matching must be a first-class axis of the corpus.** Task 4
reimplemented `FilenameUtils.wildcardMatch` rather than binding to it, and said
so: the algorithm agrees on standard `*`/`?` semantics but has never been
differentially checked against the Java one. Generate values exercising `*` and
`?` in leading, trailing, interior and repeated positions, empty segments,
values that are only `*`, and case differences under `ignoreCase: true` — this
gate is the only thing that will catch a divergence there.

Also generate **request**-side `*` and empty values. Task 4 measured that
`getMatchType` short-circuits before consulting policy values, so a request
value of `*` matches only a match-any policy — a rule that is easy to get
backwards and that no hand-written case currently covers.

**Several same-priority policies over one resource must be a first-class axis.**
Task 5 did not reproduce Ranger's `getEvalOrder()` (it stands in a name-then-id
tie-break) and said plainly that its argument for why order cannot change
allow/deny is reasoning from the loop, not a measurement. It named this its
weakest link. Nothing but a corpus with competing same-priority policies can
falsify it.

**Record, do not test, the one measured version divergence.** Task 5 measured
that an `_any` request against an ANCESTOR-matching `isDenyAllElse` policy
denies on Ranger 2.5.0 (Trino 470) and does not on 2.8.0 (StarRocks 4.1). The
evaluator refuses `_any` as `Undecidable`, so such cases are out of the corpus
by construction — note the divergence in the README rather than generating it.

Generate only constructs the evaluator claims to model. A generated
`Undecidable` proves nothing about agreement — count them and assert the count
is zero for the generated corpus, so the generator cannot quietly drift into
producing cases the evaluator declines to answer.

- [ ] **Step 3: Compare.** For each case assert the Rust `Decision` matches the
Java verdict. On mismatch, print the case id, the seed, and the full policy JSON
— a differential failure is useless without a reproducer.

- [ ] **Step 4: Run it.**

```bash
EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential
```

Run the corpus at **both** plugin versions — 2.5.0 against the Trino
service-def and 2.8.0 against the StarRocks one — since a gate at one version
certifies only one engine.

Start at a few hundred cases; raise until runtime is the binding constraint.
Record the case count, the seed, and both versions in the README.

**Measure the `EvalOrderAmbiguous` rate.** Task 6 found that the engine
arbitrates masks with `>=`, not `>` — the first matching evaluator is final —
so `getEvalOrder()` is load-bearing for masks where Task 5 could ignore it for
access. Rather than reproduce `computeEvalOrder()` and risk turning an honest
`Undecidable` into a confident wrong mask, Task 6 refuses a genuine tie inside
one `(priority, hasDeny)` band. Report how often the corpus hits that refusal;
if it is common, reproducing `computeEvalOrder()` becomes worth its risk.

- [ ] **Step 5: Verify the gate can fail.** Deliberately break one evaluator rule
(e.g. make ANCESTOR allow), re-run, confirm the differential test catches it,
then restore and confirm the restore is byte-identical (`diff` against a backup
— `cp` is aliased to `cp -i` and can silently refuse).

A gate never observed failing is not known to be a gate.

- [ ] **Step 6: Commit**

---

### Task 8: Close Gap B — the live Trino gate

**Files:**
- Create: `docs/en/plans/2026-09-28-sp4d-1-live-result.md`
- Modify: `docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md` (add
  the evaluator check to the existing recipe)

**Interfaces:**
- Consumes: the evaluator.
- Produces: measured agreement between the evaluator's prediction and what a
  real Trino actually does.

The Java harness constructs the request itself. Trino's own
`trino-ranger-470.jar` builds requests from Trino's SPI, and that layer is where
the T1 finding lived — a grant that the harness would call allowed was denied by
Trino. This gate covers few cases by design; its job is a class the harness is
structurally blind to.

- [ ] **Step 1: Pick the cases.** Small and deliberate, each chosen because it
exercises request construction rather than policy logic:
1. The T1 case: a policy naming `{catalog, schema}` vs a `SELECT` on a table.
2. The same with `column: ["*"]` — must allow.
3. A column-level mask, checked by reading the masked value back.
4. A row filter, checked by row count. **Issue this check at TABLE
   granularity, not column.** `rowFilterDef.resources` names no `column` on
   either engine, so a column-level request makes every row-filter policy an
   ANCESTOR match and yields no filter at all. Task 6 lost six tests to this
   before spotting it; a live gate that gets it wrong will read as "row filters
   do not work".
5. A deny that an allow-exception cancels.
6. A `public` grant against a user with no groups.
7. **Whether StarRocks' Ranger plugin fetches tags at all.** A survey claimed
   StarRocks has no tag policies, and my Task 6 brief repeated it as fact. Task 6
   correctly REFUSED to encode it, because encoding an unverified claim would
   remove an `Undecidable` — fail-open — and the StarRocks FE authorizer jar was
   not available to check. egret serves `/tags/download/{service}`, so the
   measurement is direct: bring up a StarRocks engine and observe whether that
   route is ever called for its service. Record the verbatim evidence either way.
8. **Whether a Ranger-undetermined request falls through to Trino's own
   authorizer.** Task 5 kept `Denied { NoPolicyMatched }` as a distinct reason
   because `getIsFallbackSupported()` gates only `isAccessDetermined`, but it did
   NOT verify how the Trino plugin consumes that flag. If a fallback fires,
   "denied" is the wrong label for that branch and the evaluator is wrong in the
   fail-closed direction. Measure it; do not reason about it.

Then **two StarRocks cases** on the live FE (`sp3c2-sr-live-fe-0` was running
2026-09-28): the ANCESTOR case (1) and the column-wildcard case (2). StarRocks
runs Ranger 2.8.0 against a different service-def, so a Trino-only live gate
would leave the evaluator's StarRocks claim untested. If no StarRocks engine is
running, record the cases as NOT MEASURED and say so — do not assume Trino's
result carries over.

- [ ] **Step 2: For each case**, predict with the evaluator first, **write the
prediction down**, then run the query on Trino and record the verbatim engine
output. Predicting after seeing the result proves nothing.

- [ ] **Step 3: Record every disagreement as a finding**, not as a test to adjust.
A disagreement means the evaluator is wrong, the harness is wrong, or the spec's
reading is wrong — resolve which, and say so in the doc.

- [ ] **Step 4: Write the result doc** with, per case: the policy JSON, the
prediction, the exact query, the verbatim output, and the verdict.

- [ ] **Step 5: Commit**

---

## Definition of done

- The harness certifies against the version Task 1 measured as enforcing.
- `cargo test -p egret-ranger-eval` green; the differential gate green at
  **both** 2.5.0 (Trino) and 2.8.0 (StarRocks) over a recorded number of
  generated cases with a recorded seed and zero `Undecidable`.
- The differential gate has been **observed failing** on a deliberate break.
- The live gate's result doc records a prediction made before each measurement.
- `cargo fmt --check` and `cargo clippy` clean; full workspace build green.
- No change to `explain`, the API surface, or the frontend — those are SP4d-2.
